use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::rc::Rc;

use regex::Regex;
use serde_json::Value;

use super::super::atlas::Source;
use super::sprite_source::{SpriteLoader, SpriteSource, SpriteSourceOutput};
use crate::renderer::util;
use crate::resources::{Resource, ResourceContent, ResourceId, ResourceManager};

struct Region {
    sprite: ResourceId,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(super) fn field<'a>(value: &'a Value, name: &str) -> io::Result<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| invalid(format!("Missing field {name}")))
}

fn string(value: &Value) -> io::Result<&str> {
    value.as_str().ok_or_else(|| invalid("Expected string"))
}

fn number(value: &Value) -> io::Result<f64> {
    value.as_f64().ok_or_else(|| invalid("Expected number"))
}

pub(super) fn array(value: &Value) -> io::Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid("Expected array"))
}

pub(super) fn id(value: &Value) -> io::Result<ResourceId> {
    let value = string(value)?;
    let (namespace, path) = value.split_once(':').unwrap_or(("minecraft", value));
    let namespace = if namespace.is_empty() {
        "minecraft"
    } else {
        namespace
    };
    ResourceId::from_parts(namespace, path).map_err(io::Error::other)
}

fn optional_regex(value: Option<&Value>) -> io::Result<Option<Regex>> {
    // COMPAT: Rust regex search, not full Java Pattern syntax. Unsupported
    // lookaround/backreferences invalidate the complete definition.
    value
        .map(|v| Regex::new(string(v)?).map_err(io::Error::other))
        .transpose()
}

pub(super) struct DirectoryLister {
    source: String,
    prefix: String,
}

impl DirectoryLister {
    pub(super) fn decode(value: &Value) -> io::Result<Box<dyn SpriteSource>> {
        Ok(Box::new(Self {
            source: string(field(value, "source")?)?.to_owned(),
            prefix: string(field(value, "prefix")?)?.to_owned(),
        }))
    }
}

impl SpriteSource for DirectoryLister {
    fn run(
        self: Box<Self>,
        resources: &dyn ResourceManager,
        suppliers: &mut SpriteSourceOutput,
    ) -> io::Result<()> {
        let Self { source, prefix } = *self;
        let directory = format!("textures/{source}");
        let leading = format!("{directory}/");
        for (file, resource) in
            resources.list_resources(&directory, &|id| id.path().ends_with(".png"))?
        {
            let path = file
                .path()
                .strip_prefix(&leading)
                .and_then(|path| path.strip_suffix(".png"))
                .ok_or_else(|| invalid(format!("Texture outside atlas directory: {file}")))?;
            let sprite = ResourceId::from_parts(file.namespace(), &format!("{prefix}{path}"))
                .map_err(io::Error::other)?;
            suppliers.insert(
                sprite.clone(),
                Supplier::Resource(resource).into_loader(sprite),
            );
        }
        Ok(())
    }
}

pub(super) struct SingleFile {
    resource: ResourceId,
    sprite: ResourceId,
}

impl SingleFile {
    pub(super) fn decode(value: &Value) -> io::Result<Box<dyn SpriteSource>> {
        let resource = id(field(value, "resource")?)?;
        let sprite = value
            .get("sprite")
            .map(id)
            .transpose()?
            .unwrap_or_else(|| resource.clone());
        Ok(Box::new(Self { resource, sprite }))
    }
}

impl SpriteSource for SingleFile {
    fn run(
        self: Box<Self>,
        resources: &dyn ResourceManager,
        suppliers: &mut SpriteSourceOutput,
    ) -> io::Result<()> {
        let Self { resource, sprite } = *self;
        if let Some(resource) = texture(resources, &resource)? {
            suppliers.insert(
                sprite.clone(),
                Supplier::Resource(resource).into_loader(sprite),
            );
        }
        Ok(())
    }
}

pub(super) struct SourceFilter {
    namespace: Option<Regex>,
    path: Option<Regex>,
}

impl SourceFilter {
    pub(super) fn decode(value: &Value) -> io::Result<Box<dyn SpriteSource>> {
        let pattern = field(value, "pattern")?
            .as_object()
            .ok_or_else(|| invalid("Expected filter pattern object"))?;
        Ok(Box::new(Self {
            namespace: optional_regex(pattern.get("namespace"))?,
            path: optional_regex(pattern.get("path"))?,
        }))
    }
}

impl SpriteSource for SourceFilter {
    fn run(
        self: Box<Self>,
        _resources: &dyn ResourceManager,
        suppliers: &mut SpriteSourceOutput,
    ) -> io::Result<()> {
        let Self { namespace, path } = *self;
        suppliers.retain(|id, _| {
            !(namespace
                .as_ref()
                .is_none_or(|regex| regex.is_match(id.namespace()))
                && path.as_ref().is_none_or(|regex| regex.is_match(id.path())))
        });
        Ok(())
    }
}

pub(super) struct Unstitcher {
    resource: ResourceId,
    divisor_x: f64,
    divisor_y: f64,
    regions: Vec<Region>,
}

impl Unstitcher {
    pub(super) fn decode(value: &Value) -> io::Result<Box<dyn SpriteSource>> {
        let regions = array(field(value, "regions")?)?
            .iter()
            .map(|region| {
                Ok(Region {
                    sprite: id(field(region, "sprite")?)?,
                    x: number(field(region, "x")?)?,
                    y: number(field(region, "y")?)?,
                    width: number(field(region, "width")?)?,
                    height: number(field(region, "height")?)?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        if regions.is_empty() {
            return Err(invalid("Unstitch regions must not be empty"));
        }
        Ok(Box::new(Self {
            resource: id(field(value, "resource")?)?,
            divisor_x: value
                .get("divisor_x")
                .map(number)
                .transpose()?
                .unwrap_or(1.0),
            divisor_y: value
                .get("divisor_y")
                .map(number)
                .transpose()?
                .unwrap_or(1.0),
            regions,
        }))
    }
}

impl SpriteSource for Unstitcher {
    fn run(
        self: Box<Self>,
        resources: &dyn ResourceManager,
        suppliers: &mut SpriteSourceOutput,
    ) -> io::Result<()> {
        let Self {
            resource,
            divisor_x,
            divisor_y,
            regions,
        } = *self;
        if let Some(resource) = texture(resources, &resource)? {
            let image = Rc::new(LazyImage::new(resource.content().clone()));
            for region in regions {
                let sprite = region.sprite.clone();
                suppliers.insert(
                    sprite.clone(),
                    Supplier::Unstitch {
                        image: Rc::clone(&image),
                        region,
                        divisor_x,
                        divisor_y,
                    }
                    .into_loader(sprite),
                );
            }
        }
        Ok(())
    }
}

pub(super) struct PalettedPermutations {
    textures: Vec<ResourceId>,
    palette_key: ResourceId,
    permutations: BTreeMap<String, ResourceId>,
    separator: String,
}

impl PalettedPermutations {
    pub(super) fn decode(value: &Value) -> io::Result<Box<dyn SpriteSource>> {
        Ok(Box::new(Self {
            textures: array(field(value, "textures")?)?
                .iter()
                .map(id)
                .collect::<io::Result<_>>()?,
            palette_key: id(field(value, "palette_key")?)?,
            permutations: field(value, "permutations")?
                .as_object()
                .ok_or_else(|| invalid("Expected permutations object"))?
                .iter()
                .map(|(key, value)| Ok((key.clone(), id(value)?)))
                .collect::<io::Result<_>>()?,
            separator: value
                .get("separator")
                .map(string)
                .transpose()?
                .unwrap_or("_")
                .to_owned(),
        }))
    }
}

impl SpriteSource for PalettedPermutations {
    fn run(
        self: Box<Self>,
        resources: &dyn ResourceManager,
        suppliers: &mut SpriteSourceOutput,
    ) -> io::Result<()> {
        let Self {
            textures,
            palette_key,
            permutations,
            separator,
        } = *self;
        let key = Rc::new(Palette {
            id: palette_key,
            pixels: OnceCell::new(),
        });
        let mappings: Vec<_> = permutations
            .into_iter()
            .map(|(name, target)| {
                (
                    name,
                    Rc::new(PaletteMapping {
                        key: Rc::clone(&key),
                        target,
                        colors: OnceCell::new(),
                    }),
                )
            })
            .collect();
        for base in textures {
            if let Some(resource) = texture(resources, &base)? {
                let image = Rc::new(LazyImage::new(resource.content().clone()));
                for (name, mapping) in &mappings {
                    let sprite = ResourceId::from_parts(
                        base.namespace(),
                        &format!("{}{separator}{name}", base.path()),
                    )
                    .map_err(io::Error::other)?;
                    suppliers.insert(
                        sprite.clone(),
                        Supplier::Paletted {
                            image: Rc::clone(&image),
                            mapping: Rc::clone(mapping),
                        }
                        .into_loader(sprite),
                    );
                }
            }
        }
        Ok(())
    }
}

fn texture(resources: &dyn ResourceManager, id: &ResourceId) -> io::Result<Option<Resource>> {
    let file = ResourceId::from_parts(id.namespace(), &format!("textures/{}.png", id.path()))
        .map_err(io::Error::other)?;
    let resource = resources.get_resource(&file)?;
    if resource.is_none() {
        tracing::warn!(%id, "Missing atlas sprite");
    }
    Ok(resource)
}

struct LazyImage {
    content: ResourceContent,
    pixels: OnceCell<Option<image::RgbaImage>>,
}

impl LazyImage {
    fn new(content: ResourceContent) -> Self {
        Self {
            content,
            pixels: OnceCell::new(),
        }
    }

    fn get(&self) -> Option<&image::RgbaImage> {
        self.pixels
            .get_or_init(|| decode_image(&self.content))
            .as_ref()
    }
}

fn decode_image(content: &ResourceContent) -> Option<image::RgbaImage> {
    let (data, width, height) = util::load_resource_png(content)?;
    image::RgbaImage::from_raw(width, height, data)
}

struct Palette {
    id: ResourceId,
    pixels: OnceCell<Option<image::RgbaImage>>,
}

fn load_palette(resources: &dyn ResourceManager, id: &ResourceId) -> Option<image::RgbaImage> {
    let load = || -> io::Result<image::RgbaImage> {
        let file = ResourceId::from_parts(
            id.namespace(),
            &format!("textures/palettes/{}.png", id.path()),
        )
        .map_err(io::Error::other)?;
        let resource = resources
            .get_resource(&file)?
            .ok_or_else(|| invalid(format!("Missing palette: {file}")))?;
        decode_image(resource.content())
            .ok_or_else(|| invalid(format!("Cannot decode palette: {file}")))
    };
    match load() {
        Ok(image) => Some(image),
        Err(error) => {
            tracing::warn!(%id, %error, "Cannot load atlas palette");
            None
        }
    }
}

struct PaletteMapping {
    key: Rc<Palette>,
    target: ResourceId,
    colors: OnceCell<Option<HashMap<[u8; 3], [u8; 4]>>>,
}

impl PaletteMapping {
    fn get(&self, resources: &dyn ResourceManager) -> Option<&HashMap<[u8; 3], [u8; 4]>> {
        self.colors.get_or_init(|| {
            let key = self.key.pixels.get_or_init(|| load_palette(resources, &self.key.id)).as_ref()?;
            let target = load_palette(resources, &self.target)?;
            if key.as_raw().len() != target.as_raw().len() {
                tracing::warn!(key = %self.key.id, target = %self.target, "Atlas palette pixel counts differ");
                return None;
            }
            let mut colors = HashMap::new();
            for (key, target) in key.pixels().zip(target.pixels()) {
                if key[3] != 0 {
                    colors.insert([key[0], key[1], key[2]], target.0);
                }
            }
            Some(colors)
        }).as_ref()
    }
}

enum Supplier {
    Resource(Resource),
    Unstitch {
        image: Rc<LazyImage>,
        region: Region,
        divisor_x: f64,
        divisor_y: f64,
    },
    Paletted {
        image: Rc<LazyImage>,
        mapping: Rc<PaletteMapping>,
    },
}

impl Supplier {
    fn into_loader(self, id: ResourceId) -> SpriteLoader {
        Box::new(move |resources| self.load(&id, resources))
    }

    fn load(self, id: &ResourceId, resources: &dyn ResourceManager) -> Option<Source> {
        let pixels = match self {
            Self::Resource(resource) => {
                return Source::from_resource(&id.to_string(), &resource, false, false);
            }
            Self::Unstitch {
                image,
                region,
                divisor_x,
                divisor_y,
            } => {
                let cropped = image
                    .get()
                    .and_then(|image| crop(image, &region, divisor_x, divisor_y));
                let Some(cropped) = cropped else {
                    // Failed crops have missingno contents, never a realized alias.
                    // The unconditional missing tile already supplies those contents.
                    tracing::warn!(%id, "Failed to unstitch atlas region; using missingno");
                    return None;
                };
                cropped
            }
            Self::Paletted { image, mapping } => {
                let mapped = || {
                    let image = image.get()?;
                    let colors = mapping.get(resources)?;
                    let mut output = image.clone();
                    for pixel in output.pixels_mut() {
                        if pixel[3] != 0
                            && let Some(target) = colors.get(&[pixel[0], pixel[1], pixel[2]])
                        {
                            let alpha = u16::from(pixel[3]) * u16::from(target[3]) / 255;
                            pixel.0 = [target[0], target[1], target[2], u8::try_from(alpha).ok()?];
                        }
                    }
                    Some(output)
                };
                let Some(output) = mapped() else {
                    tracing::warn!(%id, "Unable to apply atlas palette");
                    return None;
                };
                output
            }
        };
        let (width, height) = pixels.dimensions();
        Some(Source::from_rgba(
            &id.to_string(),
            pixels.into_raw(),
            width,
            height,
        ))
    }
}

fn crop(
    image: &image::RgbaImage,
    region: &Region,
    divisor_x: f64,
    divisor_y: f64,
) -> Option<image::RgbaImage> {
    if !divisor_x.is_finite() || !divisor_y.is_finite() || divisor_x == 0.0 || divisor_y == 0.0 {
        return None;
    }
    let sx = f64::from(image.width()) / divisor_x;
    let sy = f64::from(image.height()) / divisor_y;
    if !sx.is_finite() || !sy.is_finite() {
        return None;
    }
    let coordinate = |value: f64| {
        let value = value.floor();
        if !value.is_finite() || value < 0.0 || value > f64::from(u32::MAX) {
            return None;
        }
        Some(value as u32)
    };
    let x = coordinate(region.x * sx)?;
    let y = coordinate(region.y * sy)?;
    let width = coordinate(region.width * sx)?;
    let height = coordinate(region.height * sy)?;
    if width == 0
        || height == 0
        || x.checked_add(width)? > image.width()
        || y.checked_add(height)? > image.height()
    {
        return None;
    }
    Some(image::imageops::crop_imm(image, x, y, width, height).to_image())
}
