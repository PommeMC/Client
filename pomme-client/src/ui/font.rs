use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
#[cfg(feature = "ttf-fonts")]
use std::ffi::CStr;
use std::io::{BufRead, BufReader, Cursor, Read};
#[cfg(feature = "ttf-fonts")]
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
#[cfg(feature = "ttf-fonts")]
use std::ptr;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::assets::{AssetId, AssetIndex, resolve_asset_path_with_packs, resource_stack_paths};
use crate::resource_pack::ResourcePackManager;

#[cfg(feature = "ttf-fonts")]
unsafe extern "C" {
    fn FT_Get_Font_Format(face: freetype::ffi::FT_Face) -> *const c_char;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FontOptions {
    pub uniform: bool,
    pub japanese_variants: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct FontSources<'a> {
    pub jar_assets_dir: &'a Path,
    pub asset_index: &'a Option<AssetIndex>,
    pub packs: &'a ResourcePackManager,
    pub options: FontOptions,
}

impl FontSources<'_> {
    fn resolve(&self, asset_key: &str) -> PathBuf {
        resolve_asset_path_with_packs(
            self.jar_assets_dir,
            self.asset_index,
            asset_key,
            Some(self.packs),
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GlyphInfo {
    /// Pixel rectangle in the glyph atlas.
    pub atlas_layer: u32,
    pub colored: bool,
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub pixel_w: u32,
    pub pixel_h: u32,
    /// Glyph geometry in font pixels.
    pub draw_w: f32,
    pub draw_h: f32,
    pub left: f32,
    pub top: f32,
    pub advance: f32,
    pub bold_offset: f32,
    pub shadow_offset: f32,
}

impl GlyphInfo {
    /// Vanilla `FontSet.hasFishyAdvance`.
    fn fishy(&self) -> bool {
        let fishy = |advance: f32| !(0.0..=32.0).contains(&advance);
        fishy(self.advance) || fishy(self.advance + self.bold_offset)
    }
}

#[derive(Clone)]
struct UnihexGlyph {
    ch: char,
    rows: [u32; 16],
    left: u8,
    right: u8,
}

impl UnihexGlyph {
    fn pixel_width(&self) -> u32 {
        u32::from(self.right - self.left + 1)
    }
}

pub const GLYPH_ATLAS_SIZE: u32 = 2048;
/// Glyphs are baked eagerly into fixed layers, so cap the atlas memory: 64
/// gray layers are 256 MiB, 32 colored layers 512 MiB.
// TODO: bake lazily into 256px pages like vanilla `GlyphStitcher`.
const MAX_GRAYSCALE_FONT_LAYERS: u32 = 64;
const MAX_COLORED_FONT_LAYERS: u32 = 32;
/// Vanilla `FontTexture.SIZE`: a glyph larger than one page can't be stitched
/// and renders as MISSING.
const FONT_TEXTURE_SIZE: u32 = 256;
const DEFAULT_FONT: &str = "minecraft:default";
const MAX_FONT_DEFINITION_BYTES: usize = 4 * 1024 * 1024;
#[cfg(feature = "ttf-fonts")]
const MAX_TTF_BYTES: usize = 64 * 1024 * 1024;
const MAX_BITMAP_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_BITMAP_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_UNIHEX_ZIP_BYTES: u64 = 64 * 1024 * 1024;
const MAX_UNIHEX_ZIP_ENTRIES: usize = 256;
const MAX_UNIHEX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_UNIHEX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_UNIHEX_LINE_BYTES: usize = 512;
const MAX_PROVIDER_GLYPHS: usize = 262_144;

fn read_file_bounded(path: &Path, max_bytes: usize, kind: &str) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("failed to open {kind} {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to stat {kind} {}: {error}", path.display()))?;
    if metadata.len() > max_bytes as u64 {
        return Err(format!(
            "{kind} {} is {} bytes, exceeding the {max_bytes}-byte limit",
            path.display(),
            metadata.len()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {kind} {}: {error}", path.display()))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "{kind} {} exceeds the {max_bytes}-byte limit",
            path.display()
        ));
    }
    Ok(bytes)
}

fn read_text_file_bounded(path: &Path, max_bytes: usize, kind: &str) -> Result<String, String> {
    String::from_utf8(read_file_bounded(path, max_bytes, kind)?)
        .map_err(|error| format!("{kind} {} is not valid UTF-8: {error}", path.display()))
}

fn decoded_rgba_bytes(width: u32, height: u32) -> Option<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
}

fn validate_unihex_archive_header(zip_bytes: u64, entry_count: usize) -> Result<(), String> {
    if zip_bytes > MAX_UNIHEX_ZIP_BYTES {
        return Err(format!(
            "Unihex archive is {zip_bytes} bytes, exceeding the {MAX_UNIHEX_ZIP_BYTES}-byte limit"
        ));
    }
    if entry_count > MAX_UNIHEX_ZIP_ENTRIES {
        return Err(format!(
            "Unihex archive contains {entry_count} entries, exceeding the {MAX_UNIHEX_ZIP_ENTRIES}-entry limit"
        ));
    }
    Ok(())
}

fn read_unihex_line_bounded<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
    entry_name: &str,
    line_number: usize,
    actual_entry_bytes: &mut u64,
    actual_total_bytes: &mut u64,
) -> Result<usize, String> {
    line.clear();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| format!("failed reading Unihex member {entry_name}: {error}"))?;
        if available.is_empty() {
            return Ok(line.len());
        }
        let take = available
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        let next_len = line
            .len()
            .checked_add(take)
            .ok_or_else(|| "Unihex line length overflow".to_owned())?;
        if next_len > MAX_UNIHEX_LINE_BYTES {
            return Err(format!(
                "Unihex member {entry_name} line {line_number} exceeds {MAX_UNIHEX_LINE_BYTES} bytes"
            ));
        }
        let take_u64 = take as u64;
        let next_entry = actual_entry_bytes
            .checked_add(take_u64)
            .ok_or_else(|| "Unihex member byte count overflow".to_owned())?;
        let next_total = actual_total_bytes
            .checked_add(take_u64)
            .ok_or_else(|| "Unihex total byte count overflow".to_owned())?;
        if next_entry > MAX_UNIHEX_ENTRY_BYTES || next_total > MAX_UNIHEX_TOTAL_BYTES {
            return Err(format!(
                "Unihex decompressed data exceeds configured byte budget in {entry_name}"
            ));
        }
        line.extend_from_slice(&available[..take]);
        let has_newline = available[take - 1] == b'\n';
        reader.consume(take);
        *actual_entry_bytes = next_entry;
        *actual_total_bytes = next_total;
        if has_newline {
            return Ok(line.len());
        }
    }
}

fn load_bitmap_image_bounded(path: &Path) -> Result<image::DynamicImage, String> {
    let bytes = read_file_bounded(path, MAX_BITMAP_FILE_BYTES, "bitmap font texture")?;
    let reader = image::ImageReader::new(Cursor::new(bytes.as_slice()))
        .with_guessed_format()
        .map_err(|error| format!("failed to identify bitmap font {}: {error}", path.display()))?;
    let (width, height) = reader.into_dimensions().map_err(|error| {
        format!(
            "failed to read bitmap font dimensions {}: {error}",
            path.display()
        )
    })?;
    let decoded_bytes = decoded_rgba_bytes(width, height)
        .ok_or_else(|| format!("bitmap font {} dimensions overflow", path.display()))?;
    if decoded_bytes > MAX_BITMAP_DECODED_BYTES {
        return Err(format!(
            "bitmap font {} would decode to {decoded_bytes} bytes, exceeding the {MAX_BITMAP_DECODED_BYTES}-byte limit",
            path.display()
        ));
    }
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("failed to identify bitmap font {}: {error}", path.display()))?
        .decode()
        .map_err(|error| format!("failed to decode bitmap font {}: {error}", path.display()))
}

#[derive(Clone, Copy)]
struct AtlasPacker {
    x: u32,
    y: u32,
    row_height: u32,
    layer: u32,
}

impl AtlasPacker {
    fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            row_height: 0,
            layer: 0,
        }
    }

    fn place(
        &mut self,
        width: u32,
        height: u32,
        max_layers: u32,
    ) -> Result<(u32, u32, u32), String> {
        if width > GLYPH_ATLAS_SIZE || height > GLYPH_ATLAS_SIZE {
            return Err(format!(
                "font glyph {width}x{height} exceeds {GLYPH_ATLAS_SIZE}px atlas page"
            ));
        }
        if self.x + width > GLYPH_ATLAS_SIZE {
            self.new_row();
        }
        if self.y + height > GLYPH_ATLAS_SIZE {
            self.layer += 1;
            self.x = 0;
            self.y = 0;
            self.row_height = 0;
        }
        if self.layer >= max_layers {
            return Err(format!("font atlas exceeds {max_layers} layers"));
        }
        let position = (self.layer, self.x, self.y);
        self.x += width;
        self.row_height = self.row_height.max(height);
        Ok(position)
    }

    fn new_row(&mut self) {
        if self.x != 0 {
            self.x = 0;
            self.y += self.row_height;
            self.row_height = 0;
        }
    }

    fn layers(&self) -> u32 {
        self.layer + 1
    }
}

#[derive(Clone, Copy)]
struct AtlasCheckpoint {
    packer: AtlasPacker,
    pixel_len: usize,
}

struct AtlasBuilder {
    packer: AtlasPacker,
    pixels: Vec<u8>,
    bytes_per_pixel: usize,
    max_layers: u32,
}

impl AtlasBuilder {
    fn new(bytes_per_pixel: usize, max_layers: u32) -> Self {
        Self {
            packer: AtlasPacker::new(),
            pixels: Vec::new(),
            bytes_per_pixel,
            max_layers,
        }
    }

    fn checkpoint(&self) -> AtlasCheckpoint {
        AtlasCheckpoint {
            packer: self.packer,
            pixel_len: self.pixels.len(),
        }
    }

    fn rollback(&mut self, checkpoint: AtlasCheckpoint) {
        self.packer = checkpoint.packer;
        self.pixels.truncate(checkpoint.pixel_len);
    }

    fn place(&mut self, width: u32, height: u32) -> Result<(u32, u32, u32), String> {
        let position = self.packer.place(width, height, self.max_layers)?;
        let needed = self.packer.layers() as usize
            * GLYPH_ATLAS_SIZE as usize
            * GLYPH_ATLAS_SIZE as usize
            * self.bytes_per_pixel;
        if self.pixels.len() < needed {
            self.pixels.resize(needed, 0);
        }
        Ok(position)
    }

    fn layers(&self) -> u32 {
        if self.pixels.is_empty() {
            0
        } else {
            self.packer.layers()
        }
    }
}

/// The gray (R8) and colored (RGBA) glyph atlases.
struct Atlases {
    gray: AtlasBuilder,
    color: AtlasBuilder,
}

impl Atlases {
    fn new(device_layer_limit: u32) -> Self {
        Self {
            gray: AtlasBuilder::new(1, MAX_GRAYSCALE_FONT_LAYERS.min(device_layer_limit)),
            color: AtlasBuilder::new(4, MAX_COLORED_FONT_LAYERS.min(device_layer_limit)),
        }
    }

    /// Runs `load`, rolling both atlases back if it fails.
    fn transaction<T>(
        &mut self,
        load: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let gray = self.gray.checkpoint();
        let color = self.color.checkpoint();
        let result = load(self);
        if result.is_err() {
            self.gray.rollback(gray);
            self.color.rollback(color);
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FontFilter {
    uniform: Option<bool>,
    japanese_variants: Option<bool>,
}

impl FontFilter {
    fn from_json(value: Option<&serde_json::Value>) -> Self {
        let Some(map) = value.and_then(serde_json::Value::as_object) else {
            return Self::default();
        };
        Self {
            uniform: map.get("uniform").and_then(serde_json::Value::as_bool),
            japanese_variants: map.get("jp").and_then(serde_json::Value::as_bool),
        }
    }

    /// Vanilla FontOption.Filter#merge: the outer/reference filter overrides
    /// the same option on the referenced provider.
    fn merge(self, inner: Self) -> Self {
        Self {
            uniform: self.uniform.or(inner.uniform),
            japanese_variants: self.japanese_variants.or(inner.japanese_variants),
        }
    }

    fn active(self, options: FontOptions) -> bool {
        self.uniform
            .is_none_or(|required| required == options.uniform)
            && self
                .japanese_variants
                .is_none_or(|required| required == options.japanese_variants)
    }
}

struct LoadedProvider {
    glyphs: HashMap<char, Arc<GlyphInfo>>,
}

enum UnresolvedProvider {
    Loaded {
        provider: Arc<LoadedProvider>,
        filter: FontFilter,
    },
    Reference {
        id: String,
        filter: FontFilter,
    },
    Invalid(String),
}

#[derive(Clone)]
struct ResolvedProvider {
    provider: Arc<LoadedProvider>,
    filter: FontFilter,
}

struct FontSetData {
    glyphs: HashMap<char, Arc<GlyphInfo>>,
    /// Vanilla `SelectedGlyphs.nonFishy` for codepoints whose first glyph has
    /// a fishy advance: the first later non-fishy glyph, or MISSING.
    non_fishy: HashMap<char, Option<Arc<GlyphInfo>>>,
    /// Vanilla `FontSet.glyphsByWidth`.
    obfuscation_glyphs: HashMap<i32, Vec<char>>,
}

/// Atlas pixels for upload; the glyph map keeps only placements.
pub struct GlyphAtlasPixels {
    pub gray: Vec<u8>,
    pub gray_layers: u32,
    pub color: Vec<u8>,
    pub color_layers: u32,
}

pub struct GlyphMap {
    /// Font sets keyed by normalized id (`namespace:path`). Unknown ids render
    /// only MISSING, like vanilla `FontManager.getFontSetRaw`.
    font_sets: HashMap<String, FontSetData>,
    /// Vanilla bakes `SpecialGlyphs.MISSING` into every font set.
    missing_glyph: Arc<GlyphInfo>,
    /// Font pixels per line (the default font's 8px cell).
    pub(crate) cell_h: u32,
}

impl GlyphMap {
    /// Loads every font in the resource stack (vanilla `FontManager.prepare`).
    pub fn load(
        sources: FontSources<'_>,
        device_layer_limit: u32,
    ) -> Result<(Self, GlyphAtlasPixels), String> {
        let options = sources.options;
        let mut atlases = Atlases::new(device_layer_limit);
        let missing_position = atlases.gray.place(5, 8)?;
        let missing_glyph = Arc::new(append_missing_glyph(
            &mut atlases.gray.pixels,
            missing_position,
        ));
        atlases.gray.packer.new_row();
        let ctx = LoadContext {
            sources,
            missing: missing_glyph.clone(),
        };

        let font_ids = discover_font_ids(&sources);
        let unresolved: HashMap<String, Vec<UnresolvedProvider>> = font_ids
            .iter()
            .map(|id| (id.clone(), load_unresolved_font(id, &ctx, &mut atlases)))
            .collect();

        let mut resolved_cache = HashMap::new();
        let mut font_sets = HashMap::new();
        for id in font_ids {
            let mut resolved = match resolve_font_providers(
                &id,
                &unresolved,
                &mut resolved_cache,
                &mut HashSet::new(),
            ) {
                Ok(resolved) => resolved,
                Err(error) => {
                    tracing::warn!("Rejecting font `{id}`: {error}");
                    continue;
                }
            };
            resolved.reverse();
            resolved.retain(|provider| provider.filter.active(options));
            if !resolved.is_empty() {
                font_sets.insert(id, build_font_set(&resolved));
            }
        }
        if !font_sets.contains_key(DEFAULT_FONT) {
            return Err("Default font failed to load".into());
        }

        let pixels = GlyphAtlasPixels {
            gray_layers: atlases.gray.layers(),
            color_layers: atlases.color.layers(),
            gray: atlases.gray.pixels,
            color: atlases.color.pixels,
        };
        tracing::debug!(
            atlas_layers = pixels.gray_layers,
            colored_atlas_layers = pixels.color_layers,
            font_sets = font_sets.len(),
            "loaded Minecraft font atlas"
        );
        let map = Self {
            font_sets,
            missing_glyph,
            cell_h: 8,
        };
        Ok((map, pixels))
    }

    /// `SpecialGlyphs.MISSING`, which vanilla's missing font set renders.
    pub(crate) fn missing(&self) -> &GlyphInfo {
        &self.missing_glyph
    }

    /// The glyph for `ch` in `font`; unknown fonts render MISSING rather than
    /// falling back to the default, like vanilla `FontManager.getFontSetRaw`.
    pub(crate) fn glyph(&self, ch: char, font: Option<&str>) -> &GlyphInfo {
        self.font_sets
            .get(font.unwrap_or(DEFAULT_FONT))
            .and_then(|set| set.glyphs.get(&ch))
            .unwrap_or(&self.missing_glyph)
    }

    /// Vanilla `FontSet.getRandomGlyph`: `pick` chooses an index among the
    /// codepoints of this width, whose non-fishy glyph is returned.
    pub(crate) fn random_glyph(
        &self,
        width: i32,
        font: Option<&str>,
        pick: impl FnOnce(usize) -> usize,
    ) -> &GlyphInfo {
        let Some(set) = self.font_sets.get(font.unwrap_or(DEFAULT_FONT)) else {
            return &self.missing_glyph;
        };
        let Some(bucket) = set.obfuscation_glyphs.get(&width) else {
            return &self.missing_glyph;
        };
        let ch = bucket[pick(bucket.len())];
        match set.non_fishy.get(&ch) {
            Some(glyph) => glyph.as_deref().unwrap_or(&self.missing_glyph),
            None => &set.glyphs[&ch],
        }
    }
}

struct LoadContext<'a> {
    sources: FontSources<'a>,
    missing: Arc<GlyphInfo>,
}

/// The asset key of resource `id` under `prefix` (`Identifier.withPrefix`).
fn asset_key(id: &str, prefix: &str) -> Result<String, String> {
    let AssetId { namespace, path } = AssetId::parse(id);
    let key = format!("{namespace}/{prefix}{path}");
    if !crate::assets::valid_asset_key(&key) {
        return Err(format!("invalid Minecraft resource location `{id}`"));
    }
    Ok(key)
}

fn normalize_resource_id(id: &str) -> Result<String, String> {
    asset_key(id, "")?;
    let AssetId { namespace, path } = AssetId::parse(id);
    Ok(format!("{namespace}:{path}"))
}

fn font_asset_key(id: &str) -> Result<String, String> {
    Ok(format!("{}.json", asset_key(id, "font/")?))
}

fn font_id_from_asset_key(asset_key: &str) -> Option<String> {
    let (namespace, rest) = asset_key.split_once('/')?;
    let path = rest.strip_prefix("font/")?.strip_suffix(".json")?;
    Some(format!("{namespace}:{path}"))
}

/// Every font id in the jar, the asset index and the active packs, sorted.
fn discover_font_ids(sources: &FontSources<'_>) -> Vec<String> {
    let mut ids = HashSet::new();
    let asset_roots = std::iter::once(sources.jar_assets_dir.to_path_buf()).chain(
        sources
            .packs
            .active_pack_dirs()
            .map(|root| root.join("assets")),
    );
    for assets in asset_roots {
        let Ok(namespaces) = std::fs::read_dir(assets) else {
            continue;
        };
        for namespace in namespaces.flatten() {
            if namespace.file_type().is_ok_and(|kind| kind.is_dir()) {
                let namespace_name = namespace.file_name().to_string_lossy().into_owned();
                collect_font_ids_from_dir(
                    &namespace.path().join("font"),
                    &namespace_name,
                    "",
                    &mut ids,
                );
            }
        }
    }
    if let Some(index) = sources.asset_index {
        ids.extend(index.keys().filter_map(font_id_from_asset_key));
    }

    let mut ids: Vec<_> = ids.into_iter().collect();
    ids.sort();
    ids
}

fn collect_font_ids_from_dir(dir: &Path, namespace: &str, prefix: &str, out: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let next_prefix = if prefix.is_empty() {
                entry.file_name().to_string_lossy().into_owned()
            } else {
                format!("{prefix}/{}", entry.file_name().to_string_lossy())
            };
            collect_font_ids_from_dir(&path, namespace, &next_prefix, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let font_path = if prefix.is_empty() {
            stem.to_owned()
        } else {
            format!("{prefix}/{stem}")
        };
        out.insert(format!("{namespace}:{font_path}"));
    }
}

/// A provider definition as decoded by vanilla's `GlyphProviderDefinition`
/// codecs; decoding failures reject the whole font file.
enum ProviderDefinition {
    Space(Vec<(char, f32)>),
    Bitmap(BitmapDefinition),
    Unihex {
        hex_file: String,
        overrides: Vec<UnihexOverride>,
    },
    Ttf(TtfDefinition),
    Reference(String),
}

struct BitmapDefinition {
    file: String,
    height: i32,
    ascent: i32,
    chars: Vec<Vec<char>>,
}

#[derive(Clone, Copy)]
struct UnihexOverride {
    from: u32,
    to: u32,
    left: u8,
    right: u8,
}

#[cfg_attr(not(feature = "ttf-fonts"), allow(dead_code))]
struct TtfDefinition {
    file: String,
    size: f32,
    oversample: f32,
    shift: (f32, f32),
    skip: HashSet<char>,
}

fn required_str<'a>(map: &'a Map<String, Value>, key: &str, kind: &str) -> Result<&'a str, String> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{kind} provider has no `{key}` string"))
}

fn optional_number(map: &Map<String, Value>, key: &str, kind: &str) -> Result<Option<f64>, String> {
    map.get(key)
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| format!("{kind} provider `{key}` is not a number"))
        })
        .transpose()
}

fn single_codepoint(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

/// Reads one font JSON (vanilla `FontManager.loadResourceStack` decodes a
/// whole file or skips it).
fn read_font_definition(path: &Path) -> Result<Vec<(ProviderDefinition, FontFilter)>, String> {
    let text = read_text_file_bounded(path, MAX_FONT_DEFINITION_BYTES, "font definition")?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    value
        .get("providers")
        .and_then(Value::as_array)
        .ok_or_else(|| "font definition has no `providers` list".to_owned())?
        .iter()
        .map(|entry| {
            let map = entry
                .as_object()
                .ok_or_else(|| "font provider is not an object".to_owned())?;
            Ok((
                parse_provider(map)?,
                FontFilter::from_json(map.get("filter")),
            ))
        })
        .collect()
}

fn parse_provider(map: &Map<String, Value>) -> Result<ProviderDefinition, String> {
    let kind = map
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "font provider has no `type`".to_owned())?;
    Ok(match kind {
        "reference" => {
            ProviderDefinition::Reference(normalize_resource_id(required_str(map, "id", kind)?)?)
        }
        "space" => {
            let advances = map
                .get("advances")
                .and_then(Value::as_object)
                .ok_or_else(|| "space provider has no `advances` map".to_owned())?;
            let advances = advances
                .iter()
                .map(|(key, value)| {
                    let ch = single_codepoint(key).ok_or_else(|| {
                        format!("space provider key `{key}` is not one codepoint")
                    })?;
                    let advance = value.as_f64().ok_or_else(|| {
                        format!("space provider advance for `{key}` is not a number")
                    })?;
                    Ok((ch, advance as f32))
                })
                .collect::<Result<_, String>>()?;
            ProviderDefinition::Space(advances)
        }
        "bitmap" => ProviderDefinition::Bitmap(parse_bitmap_definition(map)?),
        "unihex" => ProviderDefinition::Unihex {
            hex_file: required_str(map, "hex_file", kind)?.to_owned(),
            overrides: parse_unihex_overrides(map.get("size_overrides"))?,
        },
        "ttf" => ProviderDefinition::Ttf(parse_ttf_definition(map)?),
        other => return Err(format!("unknown font provider type `{other}`")),
    })
}

/// Vanilla `BitmapProvider.Definition` codec, including its validators.
fn parse_bitmap_definition(map: &Map<String, Value>) -> Result<BitmapDefinition, String> {
    let file = required_str(map, "file", "bitmap")?.to_owned();
    let height = optional_number(map, "height", "bitmap")?.map_or(8, |height| height as i32);
    let ascent = optional_number(map, "ascent", "bitmap")?
        .ok_or_else(|| "bitmap provider has no `ascent`".to_owned())? as i32;
    if ascent > height {
        return Err(format!("ascent {ascent} higher than height {height}"));
    }
    let chars = map
        .get("chars")
        .and_then(Value::as_array)
        .ok_or_else(|| "bitmap provider has no `chars` list".to_owned())?
        .iter()
        .map(|row| {
            row.as_str()
                .map(|row| row.chars().collect::<Vec<_>>())
                .ok_or_else(|| "bitmap provider `chars` has a non-string row".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let width = chars.first().map_or(0, Vec::len);
    if width == 0 {
        return Err("expected to find data in codepoint grid".into());
    }
    if chars.iter().any(|row| row.len() != width) {
        return Err("lines in codepoint grid have to be the same length".into());
    }
    if chars.len() * width > MAX_PROVIDER_GLYPHS {
        return Err(format!(
            "bitmap provider declares more than {MAX_PROVIDER_GLYPHS} cells"
        ));
    }
    Ok(BitmapDefinition {
        file,
        height,
        ascent,
        chars,
    })
}

/// Vanilla `UnihexProvider.OverrideRange` codec.
fn parse_unihex_overrides(value: Option<&Value>) -> Result<Vec<UnihexOverride>, String> {
    let Some(ranges) = value else {
        return Ok(Vec::new());
    };
    let ranges = ranges
        .as_array()
        .ok_or_else(|| "unihex `size_overrides` is not a list".to_owned())?;
    ranges
        .iter()
        .map(|range| {
            let invalid = || format!("invalid unihex size override {range}");
            let map = range.as_object().ok_or_else(invalid)?;
            let codepoint = |key| map.get(key).and_then(json_codepoint).ok_or_else(invalid);
            let bound = |key| {
                map.get(key)
                    .and_then(Value::as_u64)
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(invalid)
            };
            let (from, to) = (codepoint("from")?, codepoint("to")?);
            let (left, right) = (bound("left")?, bound("right")?);
            if from >= to || left > right || right >= 32 {
                return Err(invalid());
            }
            Ok(UnihexOverride {
                from,
                to,
                left,
                right,
            })
        })
        .collect()
}

/// Vanilla `TrueTypeGlyphProviderDefinition` codec.
fn parse_ttf_definition(map: &Map<String, Value>) -> Result<TtfDefinition, String> {
    let file = required_str(map, "file", "ttf")?.to_owned();
    let size = optional_number(map, "size", "ttf")?.unwrap_or(11.0) as f32;
    let oversample = optional_number(map, "oversample", "ttf")?.unwrap_or(1.0) as f32;
    let shift = match map.get("shift") {
        None => (0.0, 0.0),
        Some(value) => {
            let shift: Option<Vec<f32>> = value.as_array().and_then(|values| {
                values
                    .iter()
                    .map(|value| value.as_f64().map(|value| value as f32))
                    .collect()
            });
            match shift.as_deref() {
                Some(&[x, y]) if (-512.0..=512.0).contains(&x) && (-512.0..=512.0).contains(&y) => {
                    (x, y)
                }
                _ => return Err("ttf `shift` must be two numbers in [-512, 512]".into()),
            }
        }
    };
    let skip = match map.get("skip") {
        None => HashSet::new(),
        Some(Value::String(value)) => value.chars().collect(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or("ttf `skip` list has a non-string value")
            })
            .collect::<Result<Vec<_>, _>>()?
            .concat()
            .chars()
            .collect(),
        Some(_) => return Err("ttf `skip` must be a string or list of strings".into()),
    };
    Ok(TtfDefinition {
        file,
        size,
        oversample,
        shift,
        skip,
    })
}

fn load_unresolved_font(
    font_id: &str,
    ctx: &LoadContext<'_>,
    atlases: &mut Atlases,
) -> Vec<UnresolvedProvider> {
    let asset_key = match font_asset_key(font_id) {
        Ok(asset_key) => asset_key,
        Err(error) => return vec![UnresolvedProvider::Invalid(error)],
    };
    let sources = &ctx.sources;
    let mut providers = Vec::new();
    for path in resource_stack_paths(
        sources.jar_assets_dir,
        sources.asset_index,
        &asset_key,
        Some(sources.packs),
    ) {
        let definitions = match read_font_definition(&path) {
            Ok(definitions) => definitions,
            Err(error) => {
                tracing::warn!(
                    "Unable to load font `{font_id}` in {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        // Vanilla stores every resource's providers reversed, resolves the
        // dependency graph, then reverses the final list into lookup priority.
        for (definition, filter) in definitions.into_iter().rev() {
            if let ProviderDefinition::Reference(id) = definition {
                providers.push(UnresolvedProvider::Reference { id, filter });
                continue;
            }
            match atlases.transaction(|atlases| load_provider(&definition, ctx, atlases)) {
                Ok(Some(provider)) => providers.push(UnresolvedProvider::Loaded {
                    provider: Arc::new(provider),
                    filter,
                }),
                Ok(None) => {}
                // Vanilla `safeLoad`: a provider that fails to load rejects
                // its whole font.
                Err(error) => providers.push(UnresolvedProvider::Invalid(format!(
                    "{}: {error}",
                    path.display()
                ))),
            }
        }
    }
    providers
}

fn load_provider(
    definition: &ProviderDefinition,
    ctx: &LoadContext<'_>,
    atlases: &mut Atlases,
) -> Result<Option<LoadedProvider>, String> {
    Ok(Some(match definition {
        ProviderDefinition::Space(advances) => LoadedProvider {
            glyphs: advances
                .iter()
                .map(|&(ch, advance)| (ch, Arc::new(space_glyph(advance))))
                .collect(),
        },
        ProviderDefinition::Bitmap(definition) => {
            load_bitmap_provider(definition, ctx, &mut atlases.color)?
        }
        ProviderDefinition::Unihex {
            hex_file,
            overrides,
        } => load_unihex_provider(hex_file, overrides, ctx, &mut atlases.gray)?,
        #[cfg(feature = "ttf-fonts")]
        ProviderDefinition::Ttf(definition) => {
            load_ttf_provider(definition, ctx, &mut atlases.gray)?
        }
        #[cfg(not(feature = "ttf-fonts"))]
        ProviderDefinition::Ttf(definition) => {
            // TODO: a pure-Rust rasterizer so ttf fonts load without FreeType.
            tracing::warn!(
                "Skipping ttf font {}: built without the ttf-fonts feature",
                definition.file
            );
            return Ok(None);
        }
        ProviderDefinition::Reference(_) => unreachable!("references are resolved, not loaded"),
    }))
}

fn resolve_font_providers(
    id: &str,
    unresolved: &HashMap<String, Vec<UnresolvedProvider>>,
    cache: &mut HashMap<String, Vec<ResolvedProvider>>,
    visiting: &mut HashSet<String>,
) -> Result<Vec<ResolvedProvider>, String> {
    if let Some(cached) = cache.get(id) {
        return Ok(cached.clone());
    }
    let Some(entries) = unresolved.get(id) else {
        return Err(format!("font reference `{id}` does not exist"));
    };
    if !visiting.insert(id.to_owned()) {
        return Err(format!("font reference cycle includes `{id}`"));
    }

    let result = (|| {
        let mut resolved = Vec::new();
        for entry in entries {
            match entry {
                UnresolvedProvider::Loaded { provider, filter } => {
                    resolved.push(ResolvedProvider {
                        provider: provider.clone(),
                        filter: *filter,
                    });
                }
                UnresolvedProvider::Reference {
                    id: referenced,
                    filter,
                } => {
                    for provider in resolve_font_providers(referenced, unresolved, cache, visiting)?
                    {
                        resolved.push(ResolvedProvider {
                            provider: provider.provider,
                            filter: filter.merge(provider.filter),
                        });
                    }
                }
                UnresolvedProvider::Invalid(error) => return Err(error.clone()),
            }
        }
        Ok(resolved)
    })();
    visiting.remove(id);
    if let Ok(resolved) = &result {
        cache.insert(id.to_owned(), resolved.clone());
    }
    result
}

fn build_font_set(providers: &[ResolvedProvider]) -> FontSetData {
    let mut glyphs: HashMap<char, Arc<GlyphInfo>> = HashMap::new();
    let mut non_fishy: HashMap<char, Option<Arc<GlyphInfo>>> = HashMap::new();
    let mut supported = FastutilIntOpenHashSet::new();
    for resolved in providers {
        for (&ch, glyph) in &resolved.provider.glyphs {
            match glyphs.entry(ch) {
                Entry::Vacant(entry) => {
                    if glyph.fishy() {
                        non_fishy.insert(ch, None);
                    }
                    entry.insert(glyph.clone());
                }
                Entry::Occupied(_) => {
                    if let Some(slot @ None) = non_fishy.get_mut(&ch)
                        && !glyph.fishy()
                    {
                        *slot = Some(glyph.clone());
                    }
                }
            }
        }

        // FontSet gathers each provider's supported glyphs into a fastutil
        // IntOpenHashSet, then unions those sets. The resulting iteration order
        // is observable because obfuscated text uses a persistent Java RNG.
        let mut provider_chars: Vec<u32> = resolved
            .provider
            .glyphs
            .keys()
            .map(|&ch| ch as u32)
            .collect();
        provider_chars.sort_unstable();
        let mut provider_set = FastutilIntOpenHashSet::new();
        for codepoint in provider_chars {
            provider_set.add(codepoint);
        }
        supported.add_all(&provider_set);
    }

    let mut obfuscation_glyphs: HashMap<i32, Vec<char>> = HashMap::new();
    for codepoint in supported.iter_order() {
        let Some(ch) = char::from_u32(codepoint) else {
            continue;
        };
        let Some(glyph) = glyphs.get(&ch) else {
            continue;
        };
        obfuscation_glyphs
            .entry(glyph.advance.ceil() as i32)
            .or_default()
            .push(ch);
    }

    FontSetData {
        glyphs,
        non_fishy,
        obfuscation_glyphs,
    }
}

fn load_bitmap_provider(
    definition: &BitmapDefinition,
    ctx: &LoadContext<'_>,
    atlas: &mut AtlasBuilder,
) -> Result<LoadedProvider, String> {
    let rows = definition.chars.len() as u32;
    let cols = definition.chars[0].len() as u32;
    let path = ctx
        .sources
        .resolve(&asset_key(&definition.file, "textures/")?);
    let image = load_bitmap_image_bounded(&path)?.to_rgba8();
    // Vanilla `BitmapProvider.Definition.load` floors the cell size.
    let pixel_w = image.width() / cols;
    let pixel_h = image.height() / rows;
    if pixel_w == 0 || pixel_h == 0 {
        return Err(format!(
            "bitmap {} is smaller than its {cols}x{rows} grid",
            path.display()
        ));
    }
    let pixel_scale = definition.height as f32 / pixel_h as f32;
    let too_large = pixel_w > FONT_TEXTURE_SIZE || pixel_h > FONT_TEXTURE_SIZE;
    let mut glyphs = HashMap::new();

    for (row, row_chars) in definition.chars.iter().enumerate() {
        for (col, &ch) in row_chars.iter().enumerate() {
            if ch == '\0' {
                continue;
            }
            let glyph = if too_large {
                ctx.missing.clone()
            } else {
                let source = (col as u32 * pixel_w, row as u32 * pixel_h, pixel_w, pixel_h);
                let actual_w = actual_glyph_width(&image, source);
                // Vanilla: (int)(0.5 + actualWidth * pixelScale) + 1.
                let advance = (0.5 + actual_w as f32 * pixel_scale) as u32 + 1;
                let placement = atlas.place(pixel_w, pixel_h)?;
                blit_bitmap_cell_rgba(&mut atlas.pixels, placement, &image, source);
                Arc::new(GlyphInfo {
                    atlas_layer: placement.0,
                    colored: true,
                    atlas_x: placement.1,
                    atlas_y: placement.2,
                    pixel_w,
                    pixel_h,
                    draw_w: pixel_w as f32 * pixel_scale,
                    draw_h: definition.height as f32,
                    left: 0.0,
                    top: 7.0 - definition.ascent as f32,
                    advance: advance as f32,
                    bold_offset: 1.0,
                    shadow_offset: 1.0,
                })
            };
            if glyphs.insert(ch, glyph).is_some() {
                tracing::warn!(
                    "Codepoint U+{:04X} declared multiple times in {}",
                    ch as u32,
                    path.display()
                );
            }
        }
    }
    Ok(LoadedProvider { glyphs })
}

/// Byte offset of pixel (`dx`, `dy`) of a glyph placed at `placement`.
fn atlas_offset(placement: (u32, u32, u32), dx: u32, dy: u32, bytes_per_pixel: usize) -> usize {
    let (layer, x, y) = placement;
    let size = GLYPH_ATLAS_SIZE as usize;
    ((layer as usize * size + (y + dy) as usize) * size + (x + dx) as usize) * bytes_per_pixel
}

fn blit_bitmap_cell_rgba(
    atlas: &mut [u8],
    placement: (u32, u32, u32),
    image: &image::RgbaImage,
    source: (u32, u32, u32, u32),
) {
    let (src_x, src_y, width, height) = source;
    for row in 0..height {
        for col in 0..width {
            let pixel = image.get_pixel(src_x + col, src_y + row).0;
            let offset = atlas_offset(placement, col, row, 4);
            atlas[offset..offset + 4].copy_from_slice(&pixel);
        }
    }
}

fn load_unihex_provider(
    hex_file: &str,
    overrides: &[UnihexOverride],
    ctx: &LoadContext<'_>,
    atlas: &mut AtlasBuilder,
) -> Result<LoadedProvider, String> {
    let path = ctx.sources.resolve(&asset_key(hex_file, "")?);
    let unihex = load_unihex_zip(&path, overrides)?;
    let mut glyphs = HashMap::with_capacity(unihex.len());
    for glyph in &unihex {
        let placement = atlas.place(glyph.pixel_width(), 16)?;
        blit_unihex_glyph(&mut atlas.pixels, placement, glyph);
        glyphs.insert(glyph.ch, Arc::new(unihex_glyph_info(placement, glyph)));
    }
    Ok(LoadedProvider { glyphs })
}

#[cfg(feature = "ttf-fonts")]
fn load_ttf_provider(
    definition: &TtfDefinition,
    ctx: &LoadContext<'_>,
    atlas: &mut AtlasBuilder,
) -> Result<LoadedProvider, String> {
    let TtfDefinition {
        size,
        oversample,
        shift,
        ref skip,
        ..
    } = *definition;
    if !size.is_finite() || size <= 0.0 {
        return Err(format!("invalid ttf size {size}"));
    }
    if !(oversample.is_finite() && oversample > 0.0) {
        return Err(format!("invalid ttf oversample {oversample}"));
    }

    let path = ctx.sources.resolve(&asset_key(&definition.file, "font/")?);
    let bytes = read_file_bounded(&path, MAX_TTF_BYTES, "TTF font")?;
    let library = freetype::Library::init()
        .map_err(|error| format!("failed to initialize FreeType: {error:?}"))?;
    let mut face = library
        .new_memory_face(bytes, 0)
        .map_err(|error| format!("failed to parse ttf font {}: {error:?}", path.display()))?;
    let face_ptr = face.raw_mut() as freetype::ffi::FT_Face;

    let format_ptr = unsafe { FT_Get_Font_Format(face_ptr) };
    if format_ptr.is_null() {
        return Err(format!(
            "could not determine font format for {}",
            path.display()
        ));
    }
    let format = unsafe { CStr::from_ptr(format_ptr) }
        .to_string_lossy()
        .into_owned();
    if format != "TrueType" {
        return Err(format!(
            "font {} is not in TTF format, was {format}",
            path.display()
        ));
    }

    let charmap_error =
        unsafe { freetype::ffi::FT_Select_Charmap(face_ptr, freetype::ffi::FT_ENCODING_UNICODE) };
    if charmap_error != freetype::ffi::FT_Err_Ok {
        return Err(format!(
            "failed to select Unicode charmap for {}: FreeType error {charmap_error}",
            path.display()
        ));
    }

    let pixels_per_em_f = (size * oversample).round();
    if !pixels_per_em_f.is_finite() || pixels_per_em_f <= 0.0 || pixels_per_em_f > u32::MAX as f32 {
        return Err(format!("invalid ttf pixel size {pixels_per_em_f}"));
    }
    let pixels_per_em = pixels_per_em_f as u32;
    face.set_pixel_sizes(pixels_per_em, pixels_per_em)
        .map_err(|error| format!("failed to set TTF pixel size: {error:?}"))?;

    let mut delta = freetype::Vector {
        x: (shift.0 * oversample * 64.0).round() as freetype::ffi::FT_Pos,
        y: (-shift.1 * oversample * 64.0).round() as freetype::ffi::FT_Pos,
    };
    unsafe {
        freetype::ffi::FT_Set_Transform(face_ptr, ptr::null_mut(), &mut delta);
    }

    let mut supported_entries = Vec::new();
    for (codepoint, index) in face.chars() {
        let Some(ch) = u32::try_from(codepoint).ok().and_then(char::from_u32) else {
            continue;
        };
        if skip.contains(&ch) {
            continue;
        }
        if supported_entries.len() >= MAX_PROVIDER_GLYPHS {
            return Err(format!(
                "TTF provider {} exposes more than {MAX_PROVIDER_GLYPHS} Unicode glyphs",
                path.display()
            ));
        }
        supported_entries.push((ch, index.get()));
    }
    let mut glyphs = HashMap::with_capacity(supported_entries.len());

    // Vanilla loads metrics with the raw flags 0x400008
    // (FT_LOAD_BITMAP_METRICS_ONLY | FT_LOAD_NO_BITMAP); freetype-rs 0.38 has
    // no named constant for the former.
    const VANILLA_METRICS_LOAD_FLAGS: freetype::ffi::FT_Int32 = 0x400008;

    for (ch, index) in supported_entries {
        let load_error =
            unsafe { freetype::ffi::FT_Load_Glyph(face_ptr, index, VANILLA_METRICS_LOAD_FLAGS) };
        if load_error != freetype::ffi::FT_Err_Ok {
            return Err(format!(
                "failed to load TTF metrics for U+{:06X}: FreeType error {load_error}",
                ch as u32
            ));
        }
        let slot = face.glyph();
        let advance = slot.advance().x as f32 / 64.0 / oversample;
        let bitmap = slot.bitmap();
        let width = u32::try_from(bitmap.width())
            .map_err(|_| format!("negative TTF bitmap width for U+{:06X}", ch as u32))?;
        let height = u32::try_from(bitmap.rows())
            .map_err(|_| format!("negative TTF bitmap height for U+{:06X}", ch as u32))?;
        let bearing_left = slot.bitmap_left() as f32 / oversample;
        let bearing_top = slot.bitmap_top() as f32 / oversample;
        if width == 0 || height == 0 {
            glyphs.insert(ch, Arc::new(space_glyph(advance)));
            continue;
        }
        if width > FONT_TEXTURE_SIZE || height > FONT_TEXTURE_SIZE {
            glyphs.insert(ch, ctx.missing.clone());
            continue;
        }

        face.load_glyph(index, freetype::face::LoadFlag::RENDER)
            .map_err(|error| {
                format!("failed to render TTF glyph U+{:06X}: {error:?}", ch as u32)
            })?;
        let rendered = face.glyph().bitmap();
        if rendered.pixel_mode() != Ok(freetype::bitmap::PixelMode::Gray) {
            return Err(format!(
                "rendered TTF glyph U+{:06X} was not 8-bit grayscale",
                ch as u32
            ));
        }
        if rendered.width() != width as i32 || rendered.rows() != height as i32 {
            return Err(format!(
                "rendered TTF glyph U+{:06X} changed size from {width}x{height} to {}x{}",
                ch as u32,
                rendered.width(),
                rendered.rows()
            ));
        }
        let required = width as usize * height as usize;
        let buffer = rendered.buffer();
        if buffer.len() < required {
            return Err(format!(
                "rendered TTF glyph U+{:06X} has a truncated bitmap buffer",
                ch as u32
            ));
        }
        let (layer, atlas_x, atlas_y) = atlas.place(width, height)?;
        blit_r8_bitmap(
            &mut atlas.pixels,
            (layer, atlas_x, atlas_y),
            (width, height),
            &buffer[..required],
        );
        glyphs.insert(
            ch,
            Arc::new(GlyphInfo {
                atlas_layer: layer,
                colored: false,
                atlas_x,
                atlas_y,
                pixel_w: width,
                pixel_h: height,
                draw_w: width as f32 / oversample,
                draw_h: height as f32 / oversample,
                left: bearing_left,
                top: 7.0 - bearing_top,
                advance,
                bold_offset: 1.0,
                shadow_offset: 1.0,
            }),
        );
    }

    Ok(LoadedProvider { glyphs })
}

#[cfg(feature = "ttf-fonts")]
fn blit_r8_bitmap(
    atlas: &mut [u8],
    placement: (u32, u32, u32),
    dimensions: (u32, u32),
    bitmap: &[u8],
) {
    let (width, height) = dimensions;
    for row in 0..height {
        let src = row as usize * width as usize;
        let dst = atlas_offset(placement, 0, row, 1);
        atlas[dst..dst + width as usize].copy_from_slice(&bitmap[src..src + width as usize]);
    }
}

fn json_codepoint(value: &serde_json::Value) -> Option<u32> {
    if let Some(number) = value.as_u64() {
        return number.try_into().ok();
    }
    let text = value.as_str()?;
    let mut chars = text.chars();
    let codepoint = chars.next()? as u32;
    chars.next().is_none().then_some(codepoint)
}

fn load_unihex_zip(path: &Path, overrides: &[UnihexOverride]) -> Result<Vec<UnihexGlyph>, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let zip_bytes = file.metadata().map_err(|error| error.to_string())?.len();
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    validate_unihex_archive_header(zip_bytes, archive.len())?;

    let mut raw: HashMap<u32, ([u32; 16], u8)> = HashMap::new();
    let mut actual_uncompressed = 0u64;
    let mut parsed_records = 0usize;

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if !entry.name().ends_with(".hex") {
            continue;
        }
        let entry_name = entry.name().to_owned();
        let mut reader = BufReader::new(entry);
        let mut line = Vec::with_capacity(128);
        let mut line_number = 0usize;
        let mut actual_entry_bytes = 0u64;
        loop {
            let next_line_number = line_number + 1;
            let read = read_unihex_line_bounded(
                &mut reader,
                &mut line,
                &entry_name,
                next_line_number,
                &mut actual_entry_bytes,
                &mut actual_uncompressed,
            )?;
            if read == 0 {
                break;
            }
            line_number = next_line_number;
            let line = std::str::from_utf8(&line).map_err(|error| {
                format!("invalid UTF-8 in Unihex member {entry_name} line {line_number}: {error}")
            })?;
            if parse_unihex_line(line, line_number, &mut raw)? {
                parsed_records += 1;
                if parsed_records > MAX_PROVIDER_GLYPHS {
                    return Err(format!(
                        "Unihex provider contains more than {MAX_PROVIDER_GLYPHS} glyph records"
                    ));
                }
            }
        }
    }

    let mut glyphs: Vec<_> = raw
        .into_iter()
        .filter_map(|(codepoint, (rows, bit_width))| {
            let ch = char::from_u32(codepoint)?;
            let (left, right) = overrides
                .iter()
                .find(|range| (range.from..=range.to).contains(&codepoint))
                .map(|range| (range.left, range.right))
                .unwrap_or_else(|| calculate_unihex_bounds(&rows, bit_width));
            Some(UnihexGlyph {
                ch,
                rows,
                left,
                right,
            })
        })
        .collect();
    glyphs.sort_unstable_by_key(|glyph| glyph.ch as u32);
    Ok(glyphs)
}

fn parse_unihex_line(
    line: &str,
    line_number: usize,
    out: &mut HashMap<u32, ([u32; 16], u8)>,
) -> Result<bool, String> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return Ok(false);
    }
    let Some((codepoint_text, bitmap)) = line.split_once(':') else {
        return Err(format!("invalid Unihex line {line_number}: missing colon"));
    };
    if !matches!(codepoint_text.len(), 4..=6) {
        return Err(format!("invalid Unihex codepoint at line {line_number}"));
    }
    let codepoint = u32::from_str_radix(codepoint_text, 16)
        .map_err(|_| format!("invalid Unihex codepoint at line {line_number}"))?;
    let bit_width: u8 = match bitmap.len() {
        32 => 8,
        64 => 16,
        96 => 24,
        128 => 32,
        _ => {
            return Err(format!("invalid Unihex bitmap width at line {line_number}"));
        }
    };
    let digits_per_row = bitmap.len() / 16;
    let mut rows = [0u32; 16];
    for (row, slot) in rows.iter_mut().enumerate() {
        let start = row * digits_per_row;
        let end = start + digits_per_row;
        let value = u32::from_str_radix(&bitmap[start..end], 16)
            .map_err(|_| format!("invalid Unihex bitmap at line {line_number}"))?;
        *slot = if bit_width == 32 {
            value
        } else {
            value << (32 - bit_width)
        };
    }
    out.insert(codepoint, (rows, bit_width));
    Ok(true)
}

#[cfg(test)]
fn parse_unihex_text(text: &str, out: &mut HashMap<u32, ([u32; 16], u8)>) -> Result<(), String> {
    for (line_number, line) in text.lines().enumerate() {
        parse_unihex_line(line, line_number + 1, out)?;
    }
    Ok(())
}

fn calculate_unihex_bounds(rows: &[u32; 16], bit_width: u8) -> (u8, u8) {
    let mask = rows.iter().fold(0u32, |mask, &row| mask | row);
    if mask == 0 {
        return (0, bit_width);
    }
    (
        mask.leading_zeros() as u8,
        (31 - mask.trailing_zeros()) as u8,
    )
}

fn unihex_glyph_info(placement: (u32, u32, u32), glyph: &UnihexGlyph) -> GlyphInfo {
    let (atlas_layer, atlas_x, atlas_y) = placement;
    GlyphInfo {
        atlas_layer,
        colored: false,
        atlas_x,
        atlas_y,
        pixel_w: glyph.pixel_width(),
        pixel_h: 16,
        draw_w: glyph.pixel_width() as f32 / 2.0,
        draw_h: 8.0,
        left: 0.0,
        top: 0.0,
        advance: glyph.pixel_width() as f32 / 2.0 + 1.0,
        bold_offset: 0.5,
        shadow_offset: 0.5,
    }
}

fn blit_unihex_glyph(atlas: &mut [u8], placement: (u32, u32, u32), glyph: &UnihexGlyph) {
    for (row_index, &row) in glyph.rows.iter().enumerate() {
        for column in 0..glyph.pixel_width() {
            let bit = u32::from(glyph.left) + column;
            let on = bit < 32 && (row & (1u32 << (31 - bit))) != 0;
            // Clear unset bits too: a rolled-back provider may have drawn here.
            atlas[atlas_offset(placement, column, row_index as u32, 1)] = if on { 255 } else { 0 };
        }
    }
}

/// Vanilla `SpecialGlyphs.MISSING`: a 5x8 white border with a transparent
/// interior, advance 6.
fn append_missing_glyph(atlas: &mut [u8], placement: (u32, u32, u32)) -> GlyphInfo {
    let (atlas_layer, atlas_x, atlas_y) = placement;
    for y in 0..8u32 {
        for x in 0..5u32 {
            let edge = x == 0 || x == 4 || y == 0 || y == 7;
            atlas[atlas_offset(placement, x, y, 1)] = if edge { 255 } else { 0 };
        }
    }
    GlyphInfo {
        atlas_layer,
        colored: false,
        atlas_x,
        atlas_y,
        pixel_w: 5,
        pixel_h: 8,
        draw_w: 5.0,
        draw_h: 8.0,
        left: 0.0,
        top: 0.0,
        advance: 6.0,
        bold_offset: 1.0,
        shadow_offset: 1.0,
    }
}

fn space_glyph(advance: f32) -> GlyphInfo {
    GlyphInfo {
        atlas_layer: 0,
        colored: false,
        atlas_x: 0,
        atlas_y: 0,
        pixel_w: 0,
        pixel_h: 0,
        draw_w: 0.0,
        draw_h: 0.0,
        left: 0.0,
        top: 0.0,
        advance,
        bold_offset: 1.0,
        shadow_offset: 1.0,
    }
}

fn actual_glyph_width(image: &image::RgbaImage, cell: (u32, u32, u32, u32)) -> u32 {
    let (x0, y0, cell_w, cell_h) = cell;
    for x in (0..cell_w).rev() {
        if (0..cell_h).any(|y| image.get_pixel(x0 + x, y0 + y)[3] != 0) {
            return x + 1;
        }
    }
    0
}

/// Minimal emulation of fastutil 8.5.18's `IntOpenHashSet`, limited to the
/// operations Minecraft's `FontSet` uses while building obfuscation buckets.
const FASTUTIL_LOAD_NUM: usize = 3;
const FASTUTIL_LOAD_DEN: usize = 4;

struct FastutilIntOpenHashSet {
    key: Vec<u32>,
    n: usize,
    mask: usize,
    max_fill: usize,
    size: usize,
    contains_null: bool,
}

impl FastutilIntOpenHashSet {
    fn new() -> Self {
        let n = fastutil_array_size(16);
        Self {
            key: vec![0; n + 1],
            n,
            mask: n - 1,
            max_fill: fastutil_max_fill(n),
            size: 0,
            contains_null: false,
        }
    }

    fn add(&mut self, value: u32) -> bool {
        if value == 0 {
            if self.contains_null {
                return false;
            }
            self.contains_null = true;
            let old_size = self.size;
            self.size += 1;
            if old_size >= self.max_fill {
                self.rehash(fastutil_array_size(self.size + 1));
            }
            return true;
        }

        let mut pos = fastutil_mix(value) as usize & self.mask;
        loop {
            let current = self.key[pos];
            if current == 0 {
                self.key[pos] = value;
                let old_size = self.size;
                self.size += 1;
                if old_size >= self.max_fill {
                    self.rehash(fastutil_array_size(self.size + 1));
                }
                return true;
            }
            if current == value {
                return false;
            }
            pos = (pos + 1) & self.mask;
        }
    }

    fn add_all(&mut self, other: &Self) {
        let needed = fastutil_capacity_for(self.size + other.size);
        if needed > self.n {
            self.rehash(needed);
        }
        for value in other.iter_order() {
            self.add(value);
        }
    }

    fn iter_order(&self) -> impl Iterator<Item = u32> + '_ {
        std::iter::once(0).filter(|_| self.contains_null).chain(
            self.key[..self.n]
                .iter()
                .rev()
                .copied()
                .filter(|&value| value != 0),
        )
    }

    fn rehash(&mut self, new_n: usize) {
        let old_key = std::mem::replace(&mut self.key, vec![0; new_n + 1]);
        let old_n = self.n;
        self.n = new_n;
        self.mask = new_n - 1;
        self.max_fill = fastutil_max_fill(new_n);

        for value in old_key[..old_n].iter().rev().copied().filter(|&v| v != 0) {
            let mut pos = fastutil_mix(value) as usize & self.mask;
            while self.key[pos] != 0 {
                pos = (pos + 1) & self.mask;
            }
            self.key[pos] = value;
        }
    }
}

fn fastutil_mix(value: u32) -> u32 {
    let h = (value as i32).wrapping_mul(-1_640_531_527);
    (h ^ ((h as u32 >> 16) as i32)) as u32
}

fn fastutil_array_size(expected: usize) -> usize {
    fastutil_capacity_for(expected)
}

fn fastutil_capacity_for(expected: usize) -> usize {
    let needed = (expected * FASTUTIL_LOAD_DEN).div_ceil(FASTUTIL_LOAD_NUM);
    needed.max(2).next_power_of_two()
}

fn fastutil_max_fill(n: usize) -> usize {
    (n * FASTUTIL_LOAD_NUM)
        .div_ceil(FASTUTIL_LOAD_DEN)
        .min(n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp dir with an empty pack manager, removed on drop.
    struct Fixture {
        root: PathBuf,
        packs: ResourcePackManager,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("pomme-font-{}", uuid::Uuid::new_v4()));
            let packs = ResourcePackManager::new(&root.join("instance"));
            Self { root, packs }
        }

        fn jar(&self) -> PathBuf {
            self.root.join("jar_assets")
        }

        fn write(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
            let path = self.root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }

        fn write_png(&self, path: impl AsRef<Path>, image: &image::RgbaImage) {
            let path = self.root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            image.save(path).unwrap();
        }

        fn load(&self) -> Result<(GlyphMap, GlyphAtlasPixels), String> {
            GlyphMap::load(
                FontSources {
                    jar_assets_dir: &self.jar(),
                    asset_index: &None,
                    packs: &self.packs,
                    options: FontOptions::default(),
                },
                u32::MAX,
            )
        }

        fn context(&self) -> LoadContext<'_> {
            LoadContext {
                sources: FontSources {
                    jar_assets_dir: self.root.as_path(),
                    asset_index: &None,
                    packs: &self.packs,
                    options: FontOptions::default(),
                },
                missing: Arc::new(space_glyph(6.0)),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const DEFAULT_SPACE: &str = r#"{"providers":[{"type":"space","advances":{"A":5.0}}]}"#;

    fn bitmap(file: &str, chars: &[&str], height: i32, ascent: i32) -> BitmapDefinition {
        BitmapDefinition {
            file: file.to_owned(),
            height,
            ascent,
            chars: chars.iter().map(|row| row.chars().collect()).collect(),
        }
    }

    fn glyph(advance: f32) -> Arc<GlyphInfo> {
        Arc::new(space_glyph(advance))
    }

    #[test]
    fn fastutil_set_iteration_matches_8_5_18() {
        let mut one = FastutilIntOpenHashSet::new();
        for value in 1..=20 {
            one.add(value);
        }
        assert_eq!(
            one.iter_order().collect::<Vec<_>>(),
            [
                2, 6, 4, 15, 14, 12, 13, 8, 9, 11, 10, 1, 3, 7, 5, 16, 17, 19, 18, 20
            ]
        );

        let mut first = FastutilIntOpenHashSet::new();
        for value in [1, 2, 3, 100, 200, 300] {
            first.add(value);
        }
        let mut second = FastutilIntOpenHashSet::new();
        for value in [3, 4, 5, 101, 201, 301] {
            second.add(value);
        }
        let mut union = FastutilIntOpenHashSet::new();
        union.add_all(&first);
        union.add_all(&second);
        assert_eq!(
            union.iter_order().collect::<Vec<_>>(),
            [200, 101, 2, 4, 201, 1, 100, 300, 3, 5, 301]
        );
    }

    #[test]
    fn provider_input_budgets_reject_oversized_metadata() {
        assert!(
            validate_unihex_archive_header(MAX_UNIHEX_ZIP_BYTES, MAX_UNIHEX_ZIP_ENTRIES).is_ok()
        );
        assert!(validate_unihex_archive_header(MAX_UNIHEX_ZIP_BYTES + 1, 1).is_err());
        assert!(validate_unihex_archive_header(1, MAX_UNIHEX_ZIP_ENTRIES + 1).is_err());

        assert_eq!(
            decoded_rgba_bytes(4096, 4096),
            Some(MAX_BITMAP_DECODED_BYTES)
        );
        assert!(decoded_rgba_bytes(u32::MAX, u32::MAX).is_none());
        assert!(decoded_rgba_bytes(4097, 4096).unwrap() > MAX_BITMAP_DECODED_BYTES);
    }

    #[test]
    fn bounded_file_reader_rejects_input_past_limit() {
        let fixture = Fixture::new();
        fixture.write("budget", b"12345");
        let path = fixture.root.join("budget");
        assert_eq!(read_file_bounded(&path, 5, "test").unwrap(), b"12345");
        assert!(read_file_bounded(&path, 4, "test").is_err());
    }

    #[test]
    fn unihex_line_reader_rejects_oversized_line_before_growing_buffer() {
        let mut bytes = vec![b'A'; MAX_UNIHEX_LINE_BYTES + 1];
        bytes.push(b'\n');
        let mut reader = Cursor::new(bytes);
        let mut line = Vec::with_capacity(64);
        let mut entry_bytes = 0;
        let mut total_bytes = 0;
        assert!(
            read_unihex_line_bounded(
                &mut reader,
                &mut line,
                "oversized.hex",
                1,
                &mut entry_bytes,
                &mut total_bytes,
            )
            .is_err()
        );
        assert!(line.len() <= MAX_UNIHEX_LINE_BYTES);
    }

    #[test]
    fn unihex_provider_uses_vanilla_two_x_oversample_metrics() {
        let bitmap = "0FF0".repeat(16);
        let mut parsed = HashMap::new();
        parse_unihex_text(&format!("2603:{bitmap}\n"), &mut parsed).unwrap();
        let (rows, bit_width) = parsed[&0x2603];
        assert_eq!(bit_width, 16);
        let (left, right) = calculate_unihex_bounds(&rows, bit_width);
        assert_eq!((left, right), (4, 11));

        let glyph = UnihexGlyph {
            ch: '☃',
            rows,
            left,
            right,
        };
        let info = unihex_glyph_info((2, 13, 29), &glyph);
        assert_eq!((info.pixel_w, info.pixel_h), (8, 16));
        assert_eq!((info.draw_w, info.draw_h), (4.0, 8.0));
        assert_eq!(info.advance, 5.0);
        assert_eq!(info.bold_offset, 0.5);
        assert_eq!(info.shadow_offset, 0.5);
    }

    #[test]
    fn unihex_blit_clears_pixels_left_by_a_rolled_back_provider() {
        let mut atlas = vec![255u8; (GLYPH_ATLAS_SIZE * GLYPH_ATLAS_SIZE) as usize];
        let glyph = UnihexGlyph {
            ch: 'x',
            rows: [0x8000_0000; 16],
            left: 0,
            right: 1,
        };
        blit_unihex_glyph(&mut atlas, (0, 0, 0), &glyph);
        assert_eq!(&atlas[..2], &[255, 0]);
    }

    #[test]
    fn unihex_override_ranges_follow_the_vanilla_codec() {
        let range = |from: u32, to: u32| serde_json::json!([{ "from": from, "to": to, "left": 0, "right": 7 }]);
        assert!(parse_unihex_overrides(Some(&range(1, 2))).is_ok());
        assert!(parse_unihex_overrides(Some(&range(2, 2))).is_err());
        assert!(parse_unihex_overrides(Some(&serde_json::json!([{ "from": 1 }]))).is_err());
    }

    #[test]
    fn font_filters_follow_runtime_uniform_and_japanese_options() {
        let any = FontFilter::default();
        for uniform in [false, true] {
            for japanese_variants in [false, true] {
                assert!(any.active(FontOptions {
                    uniform,
                    japanese_variants,
                }));
            }
        }

        let uniform = FontFilter {
            uniform: Some(true),
            japanese_variants: None,
        };
        assert!(uniform.active(FontOptions {
            uniform: true,
            japanese_variants: false,
        }));
        assert!(!uniform.active(FontOptions::default()));

        let jp_false = FontFilter {
            uniform: None,
            japanese_variants: Some(false),
        };
        assert!(jp_false.active(FontOptions::default()));
        assert!(!jp_false.active(FontOptions {
            uniform: false,
            japanese_variants: true,
        }));

        let inner = FontFilter {
            uniform: Some(false),
            japanese_variants: Some(true),
        };
        let outer = FontFilter {
            uniform: Some(true),
            japanese_variants: None,
        };
        assert_eq!(
            outer.merge(inner).uniform,
            Some(true),
            "reference filter must override the referenced provider"
        );
        assert_eq!(outer.merge(inner).japanese_variants, Some(true));
    }

    #[test]
    fn required_default_font_fails_closed_when_reference_is_missing() {
        let fixture = Fixture::new();
        fixture.write(
            "jar_assets/minecraft/font/default.json",
            r#"{"providers":[{"type":"reference","id":"example:missing"}]}"#,
        );
        assert!(fixture.load().is_err());
    }

    #[test]
    fn font_reference_cycles_and_missing_targets_reject_the_bundle() {
        let reference = |id: &str| {
            vec![UnresolvedProvider::Reference {
                id: id.to_owned(),
                filter: FontFilter::default(),
            }]
        };
        let unresolved = HashMap::from([
            ("example:a".to_owned(), reference("example:b")),
            ("example:b".to_owned(), reference("example:a")),
            (
                "example:missing-wrapper".to_owned(),
                reference("example:not-present"),
            ),
        ]);
        let mut cache = HashMap::new();
        assert!(
            resolve_font_providers("example:a", &unresolved, &mut cache, &mut HashSet::new())
                .is_err()
        );
        assert!(
            resolve_font_providers(
                "example:missing-wrapper",
                &unresolved,
                &mut cache,
                &mut HashSet::new()
            )
            .is_err()
        );
    }

    #[test]
    fn failed_provider_transaction_restores_shared_atlas_capacity() {
        let mut atlases = Atlases::new(u32::MAX);
        let gray_before = atlases.gray.checkpoint();
        let color_before = atlases.color.checkpoint();

        let result: Result<(), String> = atlases.transaction(|atlases| {
            atlases.gray.place(64, 64)?;
            atlases.color.place(64, 64)?;
            Err("synthetic provider failure".into())
        });
        assert!(result.is_err());
        for (atlas, before) in [(&atlases.gray, gray_before), (&atlases.color, color_before)] {
            assert_eq!(
                (atlas.packer.layer, atlas.packer.x, atlas.packer.y),
                (before.packer.layer, before.packer.x, before.packer.y)
            );
            assert_eq!(atlas.pixels.len(), before.pixel_len);
        }
    }

    #[test]
    fn atlas_packer_refuses_layers_beyond_budget() {
        let mut packer = AtlasPacker::new();
        packer.place(GLYPH_ATLAS_SIZE, GLYPH_ATLAS_SIZE, 2).unwrap();
        packer.place(GLYPH_ATLAS_SIZE, GLYPH_ATLAS_SIZE, 2).unwrap();
        assert!(packer.place(GLYPH_ATLAS_SIZE, GLYPH_ATLAS_SIZE, 2).is_err());
    }

    #[test]
    fn bitmap_duplicate_codepoint_uses_last_declared_cell() {
        let fixture = Fixture::new();
        let mut image = image::RgbaImage::new(16, 8);
        for y in 0..8 {
            image.put_pixel(0, y, image::Rgba([255, 0, 0, 255]));
            for x in 8..16 {
                image.put_pixel(x, y, image::Rgba([0, 0, 255, 255]));
            }
        }
        fixture.write_png("example/textures/font/duplicate.png", &image);
        let mut atlas = AtlasBuilder::new(4, 1);
        let loaded = load_bitmap_provider(
            &bitmap("example:font/duplicate.png", &["AA"], 8, 7),
            &fixture.context(),
            &mut atlas,
        )
        .unwrap();
        let glyph = &loaded.glyphs[&'A'];
        assert_eq!(glyph.advance, 9.0, "last 8px-wide cell must win");
        let offset = atlas_offset((glyph.atlas_layer, glyph.atlas_x, glyph.atlas_y), 0, 0, 4);
        assert_eq!(&atlas.pixels[offset..offset + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn bitmap_grid_floors_a_texture_that_does_not_divide_evenly() {
        let fixture = Fixture::new();
        fixture.write_png(
            "example/textures/font/uneven.png",
            &image::RgbaImage::new(130, 8),
        );
        let loaded = load_bitmap_provider(
            &bitmap("example:font/uneven.png", &["ABCDEFGHIJKLMNOP"], 8, 7),
            &fixture.context(),
            &mut AtlasBuilder::new(4, 1),
        )
        .unwrap();
        assert_eq!(loaded.glyphs[&'A'].pixel_w, 8);
    }

    #[test]
    fn bitmap_cell_larger_than_a_font_page_renders_missing() {
        let fixture = Fixture::new();
        fixture.write_png(
            "example/textures/font/huge.png",
            &image::RgbaImage::new(257, 8),
        );
        let ctx = fixture.context();
        let loaded = load_bitmap_provider(
            &bitmap("example:font/huge.png", &["A"], 8, 7),
            &ctx,
            &mut AtlasBuilder::new(4, 1),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&loaded.glyphs[&'A'], &ctx.missing));
    }

    #[test]
    fn accented_provider_keeps_vanilla_twelve_pixel_geometry() {
        let fixture = Fixture::new();
        let mut image = image::RgbaImage::new(9, 12);
        for y in 0..12 {
            for x in 0..5 {
                image.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        fixture.write_png("example/textures/font/accented.png", &image);
        let loaded = load_bitmap_provider(
            &bitmap("example:font/accented.png", &["Á"], 12, 10),
            &fixture.context(),
            &mut AtlasBuilder::new(4, 1),
        )
        .unwrap();
        let glyph = &loaded.glyphs[&'Á'];
        assert_eq!((glyph.pixel_w, glyph.pixel_h), (9, 12));
        assert_eq!((glyph.draw_w, glyph.draw_h), (9.0, 12.0));
        assert_eq!(glyph.top, -3.0);
        assert_eq!(glyph.advance, 6.0);
    }

    #[test]
    fn first_provider_in_a_font_wins() {
        let fixture = Fixture::new();
        let mut first = image::RgbaImage::new(8, 8);
        first.put_pixel(1, 0, image::Rgba([255, 255, 255, 255]));
        let mut second = image::RgbaImage::new(8, 8);
        second.put_pixel(6, 0, image::Rgba([255, 255, 255, 255]));
        fixture.write_png("jar_assets/minecraft/textures/font/first.png", &first);
        fixture.write_png("jar_assets/minecraft/textures/font/second.png", &second);
        fixture.write(
            "jar_assets/minecraft/font/default.json",
            r#"{"providers":[
                {"type":"bitmap","file":"minecraft:font/first.png","ascent":7,"chars":["X"]},
                {"type":"bitmap","file":"minecraft:font/second.png","ascent":7,"chars":["X"]}
            ]}"#,
        );
        let (map, _) = fixture.load().unwrap();
        assert_eq!(map.glyph('X', None).advance, 3.0);
    }

    #[test]
    fn failing_provider_rejects_its_font_and_bad_file_is_skipped() {
        let fixture = Fixture::new();
        fixture.write("jar_assets/minecraft/font/default.json", DEFAULT_SPACE);
        fixture.write(
            "jar_assets/example/font/broken.json",
            r#"{"providers":[
                {"type":"space","advances":{"B":7.0}},
                {"type":"bitmap","file":"example:font/missing.png","ascent":7,"chars":["C"]}
            ]}"#,
        );
        fixture.write(
            "jar_assets/example/font/mixed.json",
            r#"{"providers":[{"type":"space","advances":{"B":7.0}}]}"#,
        );
        let mut packs = ResourcePackManager::new(&fixture.root.join("instance"));
        let pack = fixture.root.join("instance/resourcepacks/bad");
        std::fs::create_dir_all(pack.join("assets/example/font")).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            r#"{"pack":{"pack_format":55,"description":"bad"}}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("assets/example/font/mixed.json"),
            r#"{"providers":[{"type":"bogus"},{"type":"space","advances":{"B":9.0}}]}"#,
        )
        .unwrap();
        packs.enable_local_pack("bad");

        let (map, _) = GlyphMap::load(
            FontSources {
                jar_assets_dir: &fixture.jar(),
                asset_index: &None,
                packs: &packs,
                options: FontOptions::default(),
            },
            u32::MAX,
        )
        .unwrap();
        assert!(!map.font_sets.contains_key("example:broken"));
        assert_eq!(map.glyph('B', Some("example:mixed")).advance, 7.0);
    }

    #[test]
    fn resource_pack_stack_overrides_fonts_and_preserves_colored_bitmap_pixels() {
        let fixture = Fixture::new();
        fixture.write("jar_assets/minecraft/font/default.json", DEFAULT_SPACE);
        let cache = "instance/resourcepacks/.server_cache";
        fixture.write(
            format!("{cache}/low/assets/minecraft/font/default.json"),
            r#"{"providers":[{"type":"space","advances":{"A":3.0}}]}"#,
        );
        fixture.write(
            format!("{cache}/high/assets/minecraft/font/default.json"),
            r#"{"providers":[{"type":"space","advances":{"A":6.0}}]}"#,
        );
        fixture.write(
            format!("{cache}/high/assets/example/font/fancy.json"),
            r#"{"providers":[{"type":"space","advances":{"B":7.0}}]}"#,
        );
        fixture.write(
            format!("{cache}/high/assets/example/font/wrapper.json"),
            r#"{"providers":[{"type":"reference","id":"example:fancy"}]}"#,
        );
        let mut image = image::RgbaImage::new(8, 8);
        image.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        fixture.write_png(
            format!("{cache}/high/assets/example/textures/font/color.png"),
            &image,
        );
        fixture.write(
            format!("{cache}/high/assets/example/font/color.json"),
            r#"{"providers":[{"type":"bitmap","file":"example:font/color.png","ascent":7,"chars":["X"]}]}"#,
        );

        let mut packs = ResourcePackManager::new(&fixture.root.join("instance"));
        packs.apply_server_pack(uuid::Uuid::from_u128(1), "low");
        packs.apply_server_pack(uuid::Uuid::from_u128(2), "high");
        let load = |packs: &ResourcePackManager| {
            GlyphMap::load(
                FontSources {
                    jar_assets_dir: &fixture.jar(),
                    asset_index: &None,
                    packs,
                    options: FontOptions::default(),
                },
                u32::MAX,
            )
            .unwrap()
        };
        let (map, pixels) = load(&packs);
        assert_eq!(map.glyph('A', None).advance, 6.0);
        assert_eq!(map.glyph('B', Some("example:fancy")).advance, 7.0);
        assert_eq!(map.glyph('B', Some("example:wrapper")).advance, 7.0);
        let colored = map.glyph('X', Some("example:color"));
        assert!(colored.colored);
        let offset = atlas_offset(
            (colored.atlas_layer, colored.atlas_x, colored.atlas_y),
            0,
            0,
            4,
        );
        assert_eq!(&pixels.color[offset..offset + 4], &[10, 20, 30, 255]);

        let (reloaded, _) = load(&ResourcePackManager::new(&fixture.root.join("instance")));
        assert_eq!(reloaded.glyph('A', None).advance, 5.0);
        assert!(!reloaded.font_sets.contains_key("example:fancy"));
    }

    #[test]
    fn alt_font_missing_codepoint_uses_special_missing_box() {
        let mut pixels = vec![0u8; (GLYPH_ATLAS_SIZE * GLYPH_ATLAS_SIZE) as usize];
        let missing = Arc::new(append_missing_glyph(&mut pixels, (0, 0, 0)));
        let set = |glyphs: Vec<(char, f32)>| FontSetData {
            glyphs: glyphs
                .into_iter()
                .map(|(ch, advance)| (ch, glyph(advance)))
                .collect(),
            non_fishy: HashMap::new(),
            obfuscation_glyphs: HashMap::new(),
        };
        let map = GlyphMap {
            font_sets: HashMap::from([
                (DEFAULT_FONT.to_owned(), set(vec![('1', 5.0)])),
                ("minecraft:alt".to_owned(), set(vec![(' ', 4.0)])),
            ]),
            missing_glyph: missing,
            cell_h: 8,
        };

        assert_eq!(map.glyph('1', None).pixel_w, 0);
        let alt_digit = map.glyph('1', Some("minecraft:alt"));
        assert_eq!((alt_digit.pixel_w, alt_digit.pixel_h), (5, 8));
        assert_eq!(alt_digit.advance, 6.0);
        assert_eq!(map.glyph(' ', Some("minecraft:alt")).advance, 4.0);
    }

    #[test]
    fn obfuscation_follows_vanilla_glyphs_by_width() {
        let provider = |glyphs: Vec<(char, f32)>| ResolvedProvider {
            provider: Arc::new(LoadedProvider {
                glyphs: glyphs
                    .into_iter()
                    .map(|(ch, advance)| (ch, glyph(advance)))
                    .collect(),
            }),
            filter: FontFilter::default(),
        };
        let set = build_font_set(&[
            provider(vec![('n', -1.0), ('a', 6.0), ('f', 40.0), ('g', 40.0)]),
            provider(vec![('f', 6.0)]),
        ]);
        assert_eq!(set.obfuscation_glyphs[&-1], ['n']);
        assert_eq!(set.obfuscation_glyphs[&40], ['g', 'f']);

        let map = GlyphMap {
            font_sets: HashMap::from([(DEFAULT_FONT.to_owned(), set)]),
            missing_glyph: glyph(123.0),
            cell_h: 8,
        };
        // Fastutil iteration places `g` before `f`: the fishy `g` has no
        // non-fishy replacement, while `f` falls through to the second provider.
        assert_eq!(map.random_glyph(40, None, |_| 0).advance, 123.0);
        assert_eq!(map.random_glyph(40, None, |_| 1).advance, 6.0);
        assert_eq!(map.random_glyph(6, None, |_| 0).advance, 6.0);
        assert_eq!(map.random_glyph(99, None, |_| 0).advance, 123.0);
    }

    #[cfg(feature = "ttf-fonts")]
    #[test]
    fn ttf_provider_uses_font_prefix_list_skip_and_freetype_metrics() {
        let fixture = Fixture::new();
        fixture.write("jar_assets/minecraft/font/default.json", DEFAULT_SPACE);
        fixture.write(
            "jar_assets/example/font/custom.json",
            r#"{"providers":[{"type":"ttf","file":"example:test.ttf","size":8.0,"skip":["A"]}]}"#,
        );
        fixture.write(
            "jar_assets/example/font/test.ttf",
            include_bytes!("../renderer/fonts/Montserrat-Medium.ttf"),
        );
        let (map, _) = fixture.load().unwrap();
        let set = &map.font_sets["example:custom"];
        assert!(!set.glyphs.contains_key(&'A'), "list-form skip must apply");
        let b = &set.glyphs[&'B'];
        assert!(b.advance > 0.0 && b.pixel_w > 0 && b.pixel_h > 0 && !b.colored);
    }
}
