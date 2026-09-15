use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use crate::assets::{AssetIndex, resolve_asset_path_with_packs, resource_stack_paths};
use crate::resource_pack::ResourcePackManager;

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
    pub packs: Option<&'a ResourcePackManager>,
    pub options: FontOptions,
}

#[derive(Clone, Debug)]
pub(crate) struct GlyphInfo {
    /// Absolute pixel rectangle in Pomme's combined Minecraft-font atlas.
    pub atlas_layer: u32,
    pub colored: bool,
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub pixel_w: u32,
    pub pixel_h: u32,
    /// Logical Vanilla glyph geometry in font pixels.
    pub draw_w: f32,
    pub draw_h: f32,
    pub left: f32,
    pub top: f32,
    pub advance: f32,
    pub bold_offset: f32,
    pub shadow_offset: f32,
}

#[cfg(test)]
struct BitmapSheet {
    image: image::RgbaImage,
    chars: Vec<String>,
    logical_height: u32,
    ascent: i32,
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

struct UnihexProvider {
    glyphs: Vec<UnihexGlyph>,
}

const MC_FONT_ATLAS_SIZE: u32 = 2048;
const MAX_GRAYSCALE_FONT_LAYERS: u32 = 32;
const MAX_COLORED_FONT_LAYERS: u32 = 4;
const MAX_FONT_DEFINITION_BYTES: usize = 4 * 1024 * 1024;
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

fn account_unihex_entry(total: &mut u64, entry_size: u64) -> Result<(), String> {
    if entry_size > MAX_UNIHEX_ENTRY_BYTES {
        return Err(format!(
            "Unihex member is {entry_size} bytes, exceeding the {MAX_UNIHEX_ENTRY_BYTES}-byte limit"
        ));
    }
    *total = total
        .checked_add(entry_size)
        .ok_or_else(|| "Unihex uncompressed byte count overflow".to_owned())?;
    if *total > MAX_UNIHEX_TOTAL_BYTES {
        return Err(format!(
            "Unihex archive exceeds the {MAX_UNIHEX_TOTAL_BYTES}-byte total uncompressed limit"
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
        if width > MC_FONT_ATLAS_SIZE || height > MC_FONT_ATLAS_SIZE {
            return Err(format!(
                "font glyph {width}x{height} exceeds {MC_FONT_ATLAS_SIZE}px atlas page"
            ));
        }
        if self.x + width > MC_FONT_ATLAS_SIZE {
            self.new_row();
        }
        if self.y + height > MC_FONT_ATLAS_SIZE {
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

struct AtlasBuilder {
    packer: AtlasPacker,
    pixels: Vec<u8>,
}

struct ColorAtlasBuilder {
    packer: AtlasPacker,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy)]
struct AtlasCheckpoint {
    packer: AtlasPacker,
    pixel_len: usize,
}

impl ColorAtlasBuilder {
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

    fn new() -> Self {
        Self {
            packer: AtlasPacker::new(),
            pixels: Vec::new(),
        }
    }

    fn place(&mut self, width: u32, height: u32) -> Result<(u32, u32, u32), String> {
        let position = self.packer.place(width, height, MAX_COLORED_FONT_LAYERS)?;
        let needed = self.packer.layers() as usize
            * MC_FONT_ATLAS_SIZE as usize
            * MC_FONT_ATLAS_SIZE as usize
            * 4;
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

impl AtlasBuilder {
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

    fn new() -> Self {
        Self {
            packer: AtlasPacker::new(),
            pixels: Vec::new(),
        }
    }

    fn place(&mut self, width: u32, height: u32) -> Result<(u32, u32, u32), String> {
        let position = self
            .packer
            .place(width, height, MAX_GRAYSCALE_FONT_LAYERS)?;
        let needed = self.packer.layers() as usize
            * MC_FONT_ATLAS_SIZE as usize
            * MC_FONT_ATLAS_SIZE as usize;
        if self.pixels.len() < needed {
            self.pixels.resize(needed, 0);
        }
        Ok(position)
    }

    fn new_row(&mut self) {
        self.packer.new_row();
    }

    fn layers(&self) -> u32 {
        self.packer.layers()
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
    glyphs: HashMap<char, GlyphInfo>,
    supported: Vec<char>,
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
    glyphs: HashMap<char, GlyphInfo>,
    obfuscation_glyphs: HashMap<u32, Vec<char>>,
}

pub struct GlyphMap {
    /// Resource font sets keyed by normalized Identifier (`namespace:path`).
    /// Unknown IDs intentionally resolve to only SpecialGlyphs.MISSING, like
    /// Vanilla FontManager#getFontSetRaw rather than falling back to default.
    font_sets: HashMap<String, FontSetData>,
    /// Vanilla bakes `SpecialGlyphs.MISSING` into every FontSet.
    missing_glyph: GlyphInfo,
    /// Base logical Minecraft font cell. Chat/UI layout remains the normal
    /// 8px font even though individual providers (e.g. accented.png) can draw
    /// taller glyphs above/below that line.
    pub(crate) cell_w: u32,
    pub(crate) cell_h: u32,
    pixels: Vec<u8>,
    colored_pixels: Vec<u8>,
    tex_w: u32,
    tex_h: u32,
    tex_layers: u32,
    colored_tex_layers: u32,
}

impl GlyphMap {
    pub fn load_required(sources: FontSources<'_>) -> Result<Self, String> {
        Self::load(sources).ok_or_else(|| "Minecraft default font set is unavailable".to_owned())
    }

    pub fn load(sources: FontSources<'_>) -> Option<Self> {
        let jar_assets_dir = sources.jar_assets_dir;
        let asset_index = sources.asset_index;
        let packs = sources.packs;
        let mut atlas = AtlasBuilder::new();
        let mut colored_atlas = ColorAtlasBuilder::new();
        let missing_position = atlas
            .place(5, 8)
            .expect("special missing glyph fits font atlas");
        let missing_glyph = append_missing_glyph(
            &mut atlas.pixels,
            MC_FONT_ATLAS_SIZE,
            MC_FONT_ATLAS_SIZE,
            missing_position.0,
            missing_position.1,
            missing_position.2,
        );
        atlas.new_row();

        let font_ids = discover_font_ids(jar_assets_dir, asset_index, packs);
        let mut unresolved: HashMap<String, Vec<UnresolvedProvider>> = HashMap::new();
        for id in &font_ids {
            let providers = load_unresolved_font(
                id,
                jar_assets_dir,
                asset_index,
                packs,
                &mut atlas,
                &mut colored_atlas,
            );
            unresolved.insert(id.clone(), providers);
        }

        let mut resolved_cache: HashMap<String, Vec<ResolvedProvider>> = HashMap::new();
        let mut font_sets = HashMap::new();
        for id in font_ids {
            let mut visiting = HashSet::new();
            let mut resolved = match resolve_font_providers(
                &id,
                &unresolved,
                &mut resolved_cache,
                &mut visiting,
            ) {
                Ok(resolved) => resolved,
                Err(error) => {
                    tracing::warn!("Rejecting font `{id}`: {error}");
                    continue;
                }
            };
            resolved.reverse();
            let active: Vec<_> = resolved
                .into_iter()
                .filter(|provider| provider.filter.active(sources.options))
                .collect();
            if active.is_empty() {
                continue;
            }
            font_sets.insert(id, build_font_set(&active));
        }

        if !font_sets.contains_key("minecraft:default") {
            tracing::warn!("Minecraft default font set is unavailable");
            return None;
        }

        let tex_layers = atlas.layers();
        let colored_tex_layers = colored_atlas.layers();
        tracing::debug!(
            atlas_width = MC_FONT_ATLAS_SIZE,
            atlas_height = MC_FONT_ATLAS_SIZE,
            atlas_layers = tex_layers,
            colored_atlas_layers = colored_tex_layers,
            font_sets = font_sets.len(),
            "loaded Minecraft font atlas"
        );

        Some(Self {
            font_sets,
            missing_glyph,
            cell_w: 8,
            cell_h: 8,
            pixels: atlas.pixels,
            colored_pixels: colored_atlas.pixels,
            tex_w: MC_FONT_ATLAS_SIZE,
            tex_h: MC_FONT_ATLAS_SIZE,
            tex_layers,
            colored_tex_layers,
        })
    }

    /// Resolve a glyph from the selected Vanilla font set. Unknown font IDs
    /// intentionally render SpecialGlyphs.MISSING rather than falling back to
    /// `minecraft:default`, matching FontManager#getFontSetRaw.
    pub(crate) fn glyph(&self, ch: char, font: Option<&str>) -> &GlyphInfo {
        let id = font.unwrap_or("minecraft:default");
        self.font_sets
            .get(id)
            .and_then(|set| set.glyphs.get(&ch))
            .unwrap_or(&self.missing_glyph)
    }

    pub fn raw_pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn dimensions(&self) -> (u32, u32, u32) {
        (self.tex_w, self.tex_h, self.tex_layers)
    }

    pub fn colored_raw_pixels(&self) -> &[u8] {
        &self.colored_pixels
    }

    pub fn colored_dimensions(&self) -> (u32, u32, u32) {
        (self.tex_w, self.tex_h, self.colored_tex_layers)
    }

    pub(crate) fn obfuscation_bucket(&self, advance: u32, font: Option<&str>) -> Option<&[char]> {
        let id = font.unwrap_or("minecraft:default");
        self.font_sets
            .get(id)?
            .obfuscation_glyphs
            .get(&advance)
            .map(Vec::as_slice)
    }
}

fn normalize_resource_id(id: &str) -> Result<String, String> {
    let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
    if path.contains(':') {
        return Err(format!(
            "resource location `{id}` has more than one namespace separator"
        ));
    }
    let asset_key = format!("{namespace}/{path}");
    if !crate::assets::valid_asset_key(&asset_key) {
        return Err(format!("invalid Minecraft resource location `{id}`"));
    }
    Ok(format!("{namespace}:{path}"))
}

fn font_asset_key(id: &str) -> Result<String, String> {
    let id = normalize_resource_id(id)?;
    let (namespace, path) = id.split_once(':').expect("validated resource id");
    let key = format!("{namespace}/font/{path}.json");
    if !crate::assets::valid_asset_key(&key) {
        return Err(format!("invalid Minecraft font resource `{id}`"));
    }
    Ok(key)
}

fn font_id_from_asset_key(asset_key: &str) -> Option<String> {
    let (namespace, rest) = asset_key.split_once('/')?;
    let path = rest.strip_prefix("font/")?.strip_suffix(".json")?;
    Some(format!("{namespace}:{path}"))
}

fn discover_font_ids(
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&ResourcePackManager>,
) -> Vec<String> {
    let mut ids = HashSet::new();

    if let Ok(namespaces) = std::fs::read_dir(jar_assets_dir) {
        for namespace in namespaces.flatten() {
            let Ok(file_type) = namespace.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let namespace_name = namespace.file_name().to_string_lossy().into_owned();
            collect_font_ids_from_dir(
                &namespace.path().join("font"),
                &namespace_name,
                "",
                &mut ids,
            );
        }
    }

    if let Some(index) = asset_index {
        for key in index.keys() {
            if let Some(id) = font_id_from_asset_key(key) {
                ids.insert(id);
            }
        }
    }

    if let Some(packs) = packs {
        for root in packs.active_pack_dirs() {
            let assets = root.join("assets");
            let Ok(namespaces) = std::fs::read_dir(assets) else {
                continue;
            };
            for namespace in namespaces.flatten() {
                let Ok(file_type) = namespace.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
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

fn load_unresolved_font(
    font_id: &str,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&ResourcePackManager>,
    atlas: &mut AtlasBuilder,
    colored_atlas: &mut ColorAtlasBuilder,
) -> Vec<UnresolvedProvider> {
    let asset_key = match font_asset_key(font_id) {
        Ok(asset_key) => asset_key,
        Err(error) => return vec![UnresolvedProvider::Invalid(error)],
    };
    let mut providers = Vec::new();
    for path in resource_stack_paths(jar_assets_dir, asset_index, &asset_key, packs) {
        let Ok(text) = read_text_file_bounded(
            &path,
            MAX_FONT_DEFINITION_BYTES,
            "Minecraft font definition",
        ) else {
            tracing::warn!(
                "Failed to read bounded Minecraft font definition {}",
                path.display()
            );
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            tracing::warn!(
                "Failed to parse Minecraft font definition {}",
                path.display()
            );
            continue;
        };
        let Some(entries) = value.get("providers").and_then(serde_json::Value::as_array) else {
            continue;
        };

        // Vanilla stores every resource's providers reversed, resolves the
        // dependency graph, then reverses the final list into lookup priority.
        for entry in entries.iter().rev() {
            let Some(map) = entry.as_object() else {
                continue;
            };
            let filter = FontFilter::from_json(map.get("filter"));
            if map.get("type").and_then(serde_json::Value::as_str) == Some("reference") {
                if let Some(id) = map.get("id").and_then(serde_json::Value::as_str) {
                    match normalize_resource_id(id) {
                        Ok(id) => providers.push(UnresolvedProvider::Reference { id, filter }),
                        Err(error) => providers.push(UnresolvedProvider::Invalid(error)),
                    }
                } else {
                    providers.push(UnresolvedProvider::Invalid(
                        "font reference provider has no id".to_owned(),
                    ));
                }
                continue;
            }
            match load_provider_transactionally(atlas, colored_atlas, |atlas, colored_atlas| {
                load_concrete_provider(
                    map,
                    jar_assets_dir,
                    asset_index,
                    packs,
                    atlas,
                    colored_atlas,
                )
            }) {
                Ok(Some(provider)) => providers.push(UnresolvedProvider::Loaded {
                    provider: Arc::new(provider),
                    filter,
                }),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    "Failed to load font provider from {}: {error}",
                    path.display()
                ),
            }
        }
    }
    providers
}

fn load_provider_transactionally<T>(
    atlas: &mut AtlasBuilder,
    colored_atlas: &mut ColorAtlasBuilder,
    load: impl FnOnce(&mut AtlasBuilder, &mut ColorAtlasBuilder) -> Result<T, String>,
) -> Result<T, String> {
    let gray_checkpoint = atlas.checkpoint();
    let color_checkpoint = colored_atlas.checkpoint();
    match load(atlas, colored_atlas) {
        Ok(value) => Ok(value),
        Err(error) => {
            atlas.rollback(gray_checkpoint);
            colored_atlas.rollback(color_checkpoint);
            Err(error)
        }
    }
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
    let mut glyphs = HashMap::new();
    let mut supported = FastutilIntOpenHashSet::new();

    for resolved in providers {
        let provider = &resolved.provider;
        for (&ch, glyph) in &provider.glyphs {
            glyphs.entry(ch).or_insert_with(|| glyph.clone());
        }
        let mut provider_set = FastutilIntOpenHashSet::new();
        for &ch in &provider.supported {
            provider_set.add(ch as u32);
        }
        supported.add_all(&provider_set);
    }

    let mut obfuscation_glyphs: HashMap<u32, Vec<char>> = HashMap::new();
    for codepoint in supported.iter_order() {
        let Some(ch) = char::from_u32(codepoint) else {
            continue;
        };
        let Some(glyph) = glyphs.get(&ch) else {
            continue;
        };
        obfuscation_glyphs
            .entry(glyph.advance.ceil() as u32)
            .or_default()
            .push(ch);
    }

    FontSetData {
        glyphs,
        obfuscation_glyphs,
    }
}

fn load_concrete_provider(
    map: &serde_json::Map<String, serde_json::Value>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&ResourcePackManager>,
    atlas: &mut AtlasBuilder,
    colored_atlas: &mut ColorAtlasBuilder,
) -> Result<Option<LoadedProvider>, String> {
    match map.get("type").and_then(serde_json::Value::as_str) {
        Some("space") => load_space_provider(map).map(Some),
        Some("bitmap") => {
            load_bitmap_provider(map, jar_assets_dir, asset_index, packs, colored_atlas).map(Some)
        }
        Some("unihex") => {
            load_unihex_provider(map, jar_assets_dir, asset_index, packs, atlas).map(Some)
        }
        Some("ttf") => load_ttf_provider(map, jar_assets_dir, asset_index, packs, atlas).map(Some),
        Some(other) => {
            tracing::warn!("Unsupported Minecraft font provider type `{other}`");
            Ok(None)
        }
        None => Ok(None),
    }
}

fn load_space_provider(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<LoadedProvider, String> {
    let Some(advances) = map.get("advances").and_then(serde_json::Value::as_object) else {
        return Err("space provider has no advances map".into());
    };
    let mut glyphs = HashMap::new();
    let mut supported = Vec::new();
    for (key, value) in advances {
        let mut chars = key.chars();
        let Some(ch) = chars.next() else { continue };
        if chars.next().is_some() {
            return Err(format!("space provider key `{key}` is not one codepoint"));
        }
        let Some(advance) = value.as_f64() else {
            return Err(format!("space provider advance for `{key}` is not numeric"));
        };
        glyphs.insert(ch, space_glyph(advance as f32));
        supported.push(ch);
    }
    supported.sort_unstable();
    Ok(LoadedProvider { glyphs, supported })
}

fn load_bitmap_provider(
    map: &serde_json::Map<String, serde_json::Value>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&ResourcePackManager>,
    atlas: &mut ColorAtlasBuilder,
) -> Result<LoadedProvider, String> {
    let file_id = map
        .get("file")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "bitmap provider has no file".to_owned())?;
    let rows = map
        .get("chars")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "bitmap provider has no chars".to_owned())?;
    let chars = rows
        .iter()
        .map(|row| row.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "bitmap provider chars contains a non-string row".to_owned())?;
    let logical_height = map
        .get("height")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(8) as u32;
    let ascent = map
        .get("ascent")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| "bitmap provider has no ascent".to_owned())? as i32;
    if ascent > logical_height as i32 {
        return Err(format!(
            "bitmap ascent {ascent} exceeds height {logical_height}"
        ));
    }

    let row_count = chars.len() as u32;
    let col_count = chars.first().map(|row| row.chars().count()).unwrap_or(0) as u32;
    let consistent_rows = chars
        .iter()
        .all(|row| row.chars().count() as u32 == col_count);
    let glyph_cells = u64::from(row_count) * u64::from(col_count);
    if row_count == 0 || col_count == 0 || !consistent_rows {
        return Err("bitmap provider has invalid grid dimensions".into());
    }
    if glyph_cells > MAX_PROVIDER_GLYPHS as u64 {
        return Err(format!(
            "bitmap provider declares {glyph_cells} cells, exceeding the {MAX_PROVIDER_GLYPHS}-glyph limit"
        ));
    }

    let texture_key = bitmap_texture_asset_key(file_id);
    let path = resolve_asset_path_with_packs(jar_assets_dir, asset_index, &texture_key, packs);
    let image = load_bitmap_image_bounded(&path)?.to_rgba8();
    if image.width() == 0
        || image.height() == 0
        || !image.width().is_multiple_of(col_count)
        || !image.height().is_multiple_of(row_count)
    {
        return Err("bitmap provider has invalid grid dimensions".into());
    }

    let pixel_w = image.width() / col_count;
    let pixel_h = image.height() / row_count;
    if pixel_w > MC_FONT_ATLAS_SIZE || pixel_h > MC_FONT_ATLAS_SIZE {
        return Err(format!(
            "bitmap glyph cell {pixel_w}x{pixel_h} exceeds {MC_FONT_ATLAS_SIZE}px atlas page"
        ));
    }
    let pixel_scale = logical_height as f32 / pixel_h as f32;
    let draw_w = pixel_w as f32 * pixel_scale;
    let draw_h = logical_height as f32;
    let top = 7.0 - ascent as f32;
    let mut glyphs = HashMap::new();

    for (row, row_chars) in chars.iter().enumerate() {
        for (col, ch) in row_chars.chars().enumerate() {
            if ch == '\0' {
                continue;
            }
            let duplicate = glyphs.contains_key(&ch);
            let src_x = col as u32 * pixel_w;
            let src_y = row as u32 * pixel_h;
            let actual_w = actual_glyph_width(&image, src_x, src_y, pixel_w, pixel_h);
            let advance = (0.5 + actual_w as f32 * pixel_scale) as u32 + 1;
            let (layer, atlas_x, atlas_y) = atlas.place(pixel_w, pixel_h)?;
            blit_bitmap_cell_rgba(
                &mut atlas.pixels,
                MC_FONT_ATLAS_SIZE,
                (layer, atlas_x, atlas_y),
                &image,
                (src_x, src_y, pixel_w, pixel_h),
            );
            glyphs.insert(
                ch,
                GlyphInfo {
                    atlas_layer: layer,
                    colored: true,
                    atlas_x,
                    atlas_y,
                    pixel_w,
                    pixel_h,
                    draw_w,
                    draw_h,
                    left: 0.0,
                    top,
                    advance: advance as f32,
                    bold_offset: 1.0,
                    shadow_offset: 1.0,
                },
            );
            if duplicate {
                tracing::warn!(
                    "Bitmap font {file_id} declares U+{:04X} more than once",
                    ch as u32
                );
            }
        }
    }
    let mut supported: Vec<_> = glyphs.keys().copied().collect();
    supported.sort_unstable();
    Ok(LoadedProvider { glyphs, supported })
}

fn blit_bitmap_cell_rgba(
    atlas: &mut [u8],
    atlas_size: u32,
    placement: (u32, u32, u32),
    image: &image::RgbaImage,
    source: (u32, u32, u32, u32),
) {
    let (layer, dst_x, dst_y) = placement;
    let (src_x, src_y, width, height) = source;
    let layer_base = (layer * atlas_size * atlas_size * 4) as usize;
    for row in 0..height {
        for col in 0..width {
            let pixel = image.get_pixel(src_x + col, src_y + row).0;
            let offset = layer_base + ((((dst_y + row) * atlas_size) + dst_x + col) * 4) as usize;
            atlas[offset..offset + 4].copy_from_slice(&pixel);
        }
    }
}

fn load_unihex_provider(
    map: &serde_json::Map<String, serde_json::Value>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&ResourcePackManager>,
    atlas: &mut AtlasBuilder,
) -> Result<LoadedProvider, String> {
    let hex_file = map
        .get("hex_file")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "unihex provider has no hex_file".to_owned())?;
    let overrides = parse_unihex_overrides(map.get("size_overrides"));
    let asset_key = resource_location_asset_key(hex_file)?;
    let path = resolve_asset_path_with_packs(jar_assets_dir, asset_index, &asset_key, packs);
    let provider = load_unihex_zip(&path, &overrides)?;
    let mut glyphs = HashMap::with_capacity(provider.glyphs.len());
    let mut supported = Vec::with_capacity(provider.glyphs.len());
    for glyph in &provider.glyphs {
        let (layer, atlas_x, atlas_y) = atlas.place(glyph.pixel_width(), 16)?;
        blit_unihex_glyph(
            &mut atlas.pixels,
            MC_FONT_ATLAS_SIZE,
            MC_FONT_ATLAS_SIZE,
            layer,
            atlas_x,
            atlas_y,
            glyph,
        );
        glyphs.insert(glyph.ch, unihex_glyph_info(layer, atlas_x, atlas_y, glyph));
        supported.push(glyph.ch);
    }
    Ok(LoadedProvider { glyphs, supported })
}

fn load_ttf_provider(
    map: &serde_json::Map<String, serde_json::Value>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    packs: Option<&ResourcePackManager>,
    atlas: &mut AtlasBuilder,
) -> Result<LoadedProvider, String> {
    let file_id = map
        .get("file")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "ttf provider has no file".to_owned())?;
    let size = map
        .get("size")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(11.0) as f32;
    let oversample = map
        .get("oversample")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0) as f32;
    if !size.is_finite() || size <= 0.0 {
        return Err(format!("invalid ttf size {size}"));
    }
    if !(oversample.is_finite() && oversample > 0.0) {
        return Err(format!("invalid ttf oversample {oversample}"));
    }
    let shift = map
        .get("shift")
        .and_then(serde_json::Value::as_array)
        .and_then(|values| {
            if values.len() != 2 {
                return None;
            }
            Some((values[0].as_f64()? as f32, values[1].as_f64()? as f32))
        })
        .unwrap_or((0.0, 0.0));
    if !shift.0.is_finite()
        || !shift.1.is_finite()
        || shift.0 < -512.0
        || shift.0 > 512.0
        || shift.1 < -512.0
        || shift.1 > 512.0
    {
        return Err("ttf shift is outside Vanilla's finite [-512,512] bounds".into());
    }

    let mut skip = HashSet::new();
    if let Some(value) = map.get("skip") {
        match value {
            serde_json::Value::String(value) => skip.extend(value.chars()),
            serde_json::Value::Array(values) => {
                for value in values {
                    let Some(value) = value.as_str() else {
                        return Err("ttf skip list contains a non-string value".into());
                    };
                    skip.extend(value.chars());
                }
            }
            _ => return Err("ttf skip must be a string or list of strings".into()),
        }
    }

    let asset_key = prefixed_resource_location_asset_key(file_id, "font/")?;
    let path = resolve_asset_path_with_packs(jar_assets_dir, asset_index, &asset_key, packs);
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
    let mut supported = Vec::with_capacity(supported_entries.len());
    let mut glyphs = HashMap::with_capacity(supported_entries.len());

    // Vanilla 26.2 uses the raw flag value 0x400008 here:
    // FT_LOAD_BITMAP_METRICS_ONLY | FT_LOAD_NO_BITMAP. The former is newer
    // than freetype-rs 0.38's named LoadFlag constants, so call the bundled
    // FreeType FFI directly to preserve Mojang's exact metric pass.
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
        supported.push(ch);

        if width == 0 || height == 0 {
            glyphs.insert(ch, space_glyph(advance));
            continue;
        }
        if width > MC_FONT_ATLAS_SIZE || height > MC_FONT_ATLAS_SIZE {
            return Err(format!(
                "ttf glyph U+{:04X} rasterized to {width}x{height}, exceeding {MC_FONT_ATLAS_SIZE}px atlas page",
                ch as u32
            ));
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
            MC_FONT_ATLAS_SIZE,
            (layer, atlas_x, atlas_y),
            (width, height),
            &buffer[..required],
        );
        glyphs.insert(
            ch,
            GlyphInfo {
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
            },
        );
    }

    supported.sort_unstable();
    Ok(LoadedProvider { glyphs, supported })
}

fn blit_r8_bitmap(
    atlas: &mut [u8],
    atlas_size: u32,
    placement: (u32, u32, u32),
    dimensions: (u32, u32),
    bitmap: &[u8],
) {
    let (layer, dst_x, dst_y) = placement;
    let (width, height) = dimensions;
    let layer_base = (layer * atlas_size * atlas_size) as usize;
    for row in 0..height {
        let src = row as usize * width as usize;
        let dst = layer_base + (((dst_y + row) * atlas_size) + dst_x) as usize;
        atlas[dst..dst + width as usize].copy_from_slice(&bitmap[src..src + width as usize]);
    }
}

fn resource_location_asset_key(id: &str) -> Result<String, String> {
    let id = normalize_resource_id(id)?;
    let (namespace, path) = id.split_once(':').expect("validated resource id");
    Ok(format!("{namespace}/{path}"))
}

fn prefixed_resource_location_asset_key(id: &str, prefix: &str) -> Result<String, String> {
    let id = normalize_resource_id(id)?;
    let (namespace, path) = id.split_once(':').expect("validated resource id");
    let key = format!("{namespace}/{prefix}{path}");
    if !crate::assets::valid_asset_key(&key) {
        return Err(format!("invalid prefixed Minecraft resource `{id}`"));
    }
    Ok(key)
}

fn parse_unihex_overrides(value: Option<&serde_json::Value>) -> Vec<(u32, u32, u8, u8)> {
    let Some(ranges) = value.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    ranges
        .iter()
        .filter_map(|range| {
            let map = range.as_object()?;
            let from = json_codepoint(map.get("from")?)?;
            let to = json_codepoint(map.get("to")?)?;
            let left: u8 = map.get("left")?.as_u64()?.try_into().ok()?;
            let right: u8 = map.get("right")?.as_u64()?.try_into().ok()?;
            if from > to || left > right || right >= 32 {
                return None;
            }
            Some((from, to, left, right))
        })
        .collect()
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

fn load_unihex_zip(
    path: &Path,
    overrides: &[(u32, u32, u8, u8)],
) -> Result<UnihexProvider, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let zip_bytes = file.metadata().map_err(|error| error.to_string())?.len();
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    validate_unihex_archive_header(zip_bytes, archive.len())?;

    let mut raw: HashMap<u32, ([u32; 16], u8)> = HashMap::new();
    let mut total_uncompressed = 0u64;
    let mut actual_uncompressed = 0u64;
    let mut parsed_records = 0usize;

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if !entry.name().ends_with(".hex") {
            continue;
        }
        let entry_size = entry.size();
        account_unihex_entry(&mut total_uncompressed, entry_size)?;

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
                if raw.len() > MAX_PROVIDER_GLYPHS {
                    return Err(format!(
                        "Unihex provider contains more than {MAX_PROVIDER_GLYPHS} unique glyphs"
                    ));
                }
            }
        }
    }

    let mut glyphs = Vec::with_capacity(raw.len());
    for (codepoint, (rows, bit_width)) in raw {
        let Some(ch) = char::from_u32(codepoint) else {
            continue;
        };
        let (left, right) = overrides
            .iter()
            .find(|&&(from, to, _, _)| (from..=to).contains(&codepoint))
            .map(|&(_, _, left, right)| (left, right))
            .unwrap_or_else(|| calculate_unihex_bounds(&rows, bit_width));
        glyphs.push(UnihexGlyph {
            ch,
            rows,
            left,
            right,
        });
    }
    glyphs.sort_unstable_by_key(|glyph| glyph.ch as u32);
    Ok(UnihexProvider { glyphs })
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

fn parse_unihex_text(text: &str, out: &mut HashMap<u32, ([u32; 16], u8)>) -> Result<(), String> {
    let mut records = 0usize;
    for (line_number, line) in text.lines().enumerate() {
        if parse_unihex_line(line, line_number + 1, out)? {
            records += 1;
            if records > MAX_PROVIDER_GLYPHS || out.len() > MAX_PROVIDER_GLYPHS {
                return Err(format!(
                    "Unihex provider contains more than {MAX_PROVIDER_GLYPHS} glyphs"
                ));
            }
        }
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

fn unihex_glyph_info(layer: u32, atlas_x: u32, atlas_y: u32, glyph: &UnihexGlyph) -> GlyphInfo {
    GlyphInfo {
        atlas_layer: layer,
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

fn blit_unihex_glyph(
    atlas: &mut [u8],
    atlas_w: u32,
    atlas_h: u32,
    layer: u32,
    dst_x: u32,
    dst_y: u32,
    glyph: &UnihexGlyph,
) {
    debug_assert!(dst_x + glyph.pixel_width() <= atlas_w);
    debug_assert!(dst_y + 16 <= atlas_h);
    let layer_base = (layer * atlas_w * atlas_h) as usize;
    for (row_index, &row) in glyph.rows.iter().enumerate() {
        for column in 0..glyph.pixel_width() {
            let bit = u32::from(glyph.left) + column;
            let on = bit < 32 && (row & (1u32 << (31 - bit))) != 0;
            if !on {
                continue;
            }
            let offset =
                layer_base + (((dst_y + row_index as u32) * atlas_w) + dst_x + column) as usize;
            atlas[offset] = 255;
        }
    }
}

fn bitmap_texture_asset_key(file_id: &str) -> String {
    let (namespace, path) = file_id.split_once(':').unwrap_or(("minecraft", file_id));
    format!("{namespace}/textures/{path}")
}

#[cfg(test)]
fn insert_bitmap_sheet(
    glyphs: &mut HashMap<char, GlyphInfo>,
    sheet: &BitmapSheet,
    layer: u32,
    atlas_x: u32,
    atlas_y: u32,
    replace: bool,
) {
    let rows = sheet.chars.len() as u32;
    if rows == 0 {
        return;
    }
    let cols = sheet
        .chars
        .first()
        .map(|row| row.chars().count())
        .unwrap_or(0) as u32;
    if cols == 0
        || !sheet.image.width().is_multiple_of(cols)
        || !sheet.image.height().is_multiple_of(rows)
    {
        tracing::warn!(
            "Invalid Minecraft bitmap font grid {}x{} for {}x{} image",
            cols,
            rows,
            sheet.image.width(),
            sheet.image.height()
        );
        return;
    }
    if sheet
        .chars
        .iter()
        .any(|row| row.chars().count() as u32 != cols)
    {
        tracing::warn!("Minecraft bitmap font grid rows have inconsistent codepoint counts");
        return;
    }

    let pixel_w = sheet.image.width() / cols;
    let pixel_h = sheet.image.height() / rows;
    let pixel_scale = sheet.logical_height as f32 / pixel_h as f32;
    let draw_w = pixel_w as f32 * pixel_scale;
    let draw_h = sheet.logical_height as f32;
    let top = 7.0 - sheet.ascent as f32;

    for (row, chars) in sheet.chars.iter().enumerate() {
        for (col, ch) in chars.chars().enumerate() {
            if ch == '\0' {
                continue;
            }
            let actual_w = actual_glyph_width(
                &sheet.image,
                col as u32 * pixel_w,
                row as u32 * pixel_h,
                pixel_w,
                pixel_h,
            );
            // Vanilla BitmapProvider: (int)(0.5 + actualWidth * pixelScale) + 1.
            let advance = (0.5 + actual_w as f32 * pixel_scale) as u32 + 1;
            let glyph = GlyphInfo {
                atlas_layer: layer,
                colored: true,
                atlas_x: atlas_x + col as u32 * pixel_w,
                atlas_y: atlas_y + row as u32 * pixel_h,
                pixel_w,
                pixel_h,
                draw_w,
                draw_h,
                left: 0.0,
                top,
                advance: advance as f32,
                bold_offset: 1.0,
                shadow_offset: 1.0,
            };
            if replace {
                glyphs.insert(ch, glyph);
            } else {
                glyphs.entry(ch).or_insert(glyph);
            }
        }
    }
}

fn append_missing_glyph(
    atlas: &mut [u8],
    atlas_w: u32,
    atlas_h: u32,
    layer: u32,
    atlas_x: u32,
    atlas_y: u32,
) -> GlyphInfo {
    // net.minecraft.client.gui.font.glyphs.SpecialGlyphs.MISSING: a 5x8 white
    // border with transparent interior, advance = width + 1.
    let layer_base = (layer * atlas_w * atlas_h) as usize;
    for y in 0..8u32 {
        for x in 0..5u32 {
            let edge = x == 0 || x == 4 || y == 0 || y == 7;
            let offset = layer_base + (((atlas_y + y) * atlas_w) + atlas_x + x) as usize;
            atlas[offset] = if edge { 255 } else { 0 };
        }
    }
    GlyphInfo {
        atlas_layer: layer,
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

fn actual_glyph_width(image: &image::RgbaImage, x0: u32, y0: u32, cell_w: u32, cell_h: u32) -> u32 {
    for x in (0..cell_w).rev() {
        if (0..cell_h).any(|y| image.get_pixel(x0 + x, y0 + y)[3] != 0) {
            return x + 1;
        }
    }
    0
}

/// Minimal emulation of fastutil 8.5.18's `IntOpenHashSet`, limited to the
/// operations Minecraft's FontSet uses while building `glyphsByWidth`.
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
        // IntOpenHashSet.addAll(IntCollection) calls tryCapacity(size + incoming)
        // at the default 0.75 load factor before iterating the source set.
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

        // fastutil rehashes by walking the previous table from high to low.
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
    // ceil(expected / 0.75), then next power of two, minimum 2.
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

        let mut with_null = FastutilIntOpenHashSet::new();
        for value in [7, 0, 3, 12] {
            with_null.add(value);
        }
        assert_eq!(with_null.iter_order().collect::<Vec<_>>(), [0, 12, 3, 7]);
    }

    #[test]
    fn bitmap_advance_matches_vanilla_rounding() {
        // Vanilla: (int)(0.5 + actualWidth * pixelScale) + 1.
        let actual_width = 5u32;
        let pixel_scale = 1.5f32;
        let advance = (0.5 + actual_width as f32 * pixel_scale) as u32 + 1;
        assert_eq!(advance, 9);
    }

    #[test]
    fn provider_input_budgets_reject_oversized_metadata() {
        assert!(
            validate_unihex_archive_header(MAX_UNIHEX_ZIP_BYTES, MAX_UNIHEX_ZIP_ENTRIES).is_ok()
        );
        assert!(validate_unihex_archive_header(MAX_UNIHEX_ZIP_BYTES + 1, 1).is_err());
        assert!(validate_unihex_archive_header(1, MAX_UNIHEX_ZIP_ENTRIES + 1).is_err());

        let mut total = 0;
        assert!(account_unihex_entry(&mut total, MAX_UNIHEX_ENTRY_BYTES).is_ok());
        assert!(account_unihex_entry(&mut total, MAX_UNIHEX_ENTRY_BYTES + 1).is_err());
        let mut total = MAX_UNIHEX_TOTAL_BYTES;
        assert!(account_unihex_entry(&mut total, 1).is_err());

        assert_eq!(
            decoded_rgba_bytes(4096, 4096),
            Some(MAX_BITMAP_DECODED_BYTES)
        );
        assert!(decoded_rgba_bytes(u32::MAX, u32::MAX).is_none());
        assert!(decoded_rgba_bytes(4097, 4096).unwrap() > MAX_BITMAP_DECODED_BYTES);
    }

    #[test]
    fn bounded_file_reader_rejects_input_past_limit() {
        let path = std::env::temp_dir().join(format!("pomme-font-budget-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"12345").unwrap();
        assert_eq!(read_file_bounded(&path, 5, "test").unwrap(), b"12345");
        assert!(read_file_bounded(&path, 4, "test").is_err());
        std::fs::remove_file(path).unwrap();
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
        let text = format!("2603:{bitmap}\n");
        let mut parsed = HashMap::new();
        parse_unihex_text(&text, &mut parsed).unwrap();
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
        let info = unihex_glyph_info(2, 13, 29, &glyph);
        assert_eq!((info.pixel_w, info.pixel_h), (8, 16));
        assert_eq!((info.draw_w, info.draw_h), (4.0, 8.0));
        assert_eq!(info.advance, 5.0);
        assert_eq!(info.bold_offset, 0.5);
        assert_eq!(info.shadow_offset, 0.5);
    }

    #[test]
    fn space_provider_uses_exact_four_pixel_advance() {
        assert_eq!(space_glyph(4.0).advance, 4.0);
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
        let root =
            std::env::temp_dir().join(format!("pomme-font-required-{}", uuid::Uuid::new_v4()));
        let font_dir = root.join("minecraft/font");
        std::fs::create_dir_all(&font_dir).unwrap();
        std::fs::write(
            font_dir.join("default.json"),
            r#"{"providers":[{"type":"reference","id":"example:missing"}]}"#,
        )
        .unwrap();

        let result = GlyphMap::load_required(FontSources {
            jar_assets_dir: &root,
            asset_index: &None,
            packs: None,
            options: FontOptions::default(),
        });
        assert!(result.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn font_reference_cycles_and_missing_targets_reject_the_bundle() {
        let unresolved = HashMap::from([
            (
                "example:a".to_owned(),
                vec![UnresolvedProvider::Reference {
                    id: "example:b".to_owned(),
                    filter: FontFilter::default(),
                }],
            ),
            (
                "example:b".to_owned(),
                vec![UnresolvedProvider::Reference {
                    id: "example:a".to_owned(),
                    filter: FontFilter::default(),
                }],
            ),
            (
                "example:missing-wrapper".to_owned(),
                vec![UnresolvedProvider::Reference {
                    id: "example:not-present".to_owned(),
                    filter: FontFilter::default(),
                }],
            ),
        ]);
        let mut cache = HashMap::new();
        let mut visiting = HashSet::new();
        assert!(
            resolve_font_providers("example:a", &unresolved, &mut cache, &mut visiting).is_err()
        );
        visiting.clear();
        assert!(
            resolve_font_providers(
                "example:missing-wrapper",
                &unresolved,
                &mut cache,
                &mut visiting
            )
            .is_err()
        );
    }

    #[test]
    fn failed_provider_transaction_restores_shared_atlas_capacity() {
        let mut gray = AtlasBuilder::new();
        let mut color = ColorAtlasBuilder::new();
        let gray_before = gray.checkpoint();
        let color_before = color.checkpoint();

        let result: Result<(), String> =
            load_provider_transactionally(&mut gray, &mut color, |gray, color| {
                gray.place(64, 64)?;
                color.place(64, 64)?;
                Err("synthetic provider failure".into())
            });
        assert!(result.is_err());
        assert_eq!(gray.packer.layer, gray_before.packer.layer);
        assert_eq!(gray.packer.x, gray_before.packer.x);
        assert_eq!(gray.packer.y, gray_before.packer.y);
        assert_eq!(gray.pixels.len(), gray_before.pixel_len);
        assert_eq!(color.packer.layer, color_before.packer.layer);
        assert_eq!(color.packer.x, color_before.packer.x);
        assert_eq!(color.packer.y, color_before.packer.y);
        assert_eq!(color.pixels.len(), color_before.pixel_len);
    }

    #[test]
    fn unihex_binary_coverage_matches_vanilla_colored_white_pixels() {
        // Vanilla stores Unihex as RGBA white/transparent; Pomme stores the
        // same binary coverage in R8 and applies the style tint in the shader.
        // For alpha in {0,1}, both paths produce identical premultiplied tint.
        let tint = [0.25_f32, 0.5, 0.75, 0.8];
        for coverage in [0.0_f32, 1.0] {
            let r8 = [
                tint[0] * coverage,
                tint[1] * coverage,
                tint[2] * coverage,
                tint[3] * coverage,
            ];
            let rgba = [
                tint[0] * coverage,
                tint[1] * coverage,
                tint[2] * coverage,
                tint[3] * coverage,
            ];
            assert_eq!(r8, rgba);
        }
    }

    #[test]
    fn atlas_packer_refuses_layers_beyond_budget() {
        let mut packer = AtlasPacker::new();
        packer
            .place(MC_FONT_ATLAS_SIZE, MC_FONT_ATLAS_SIZE, 2)
            .unwrap();
        packer
            .place(MC_FONT_ATLAS_SIZE, MC_FONT_ATLAS_SIZE, 2)
            .unwrap();
        assert!(
            packer
                .place(MC_FONT_ATLAS_SIZE, MC_FONT_ATLAS_SIZE, 2)
                .is_err()
        );
    }

    #[test]
    fn bitmap_duplicate_codepoint_uses_last_declared_cell() {
        let root = std::env::temp_dir().join(format!("pomme-bitmap-{}", uuid::Uuid::new_v4()));
        let texture = root.join("example/textures/font/duplicate.png");
        std::fs::create_dir_all(texture.parent().unwrap()).unwrap();
        let mut image = image::RgbaImage::new(16, 8);
        for y in 0..8 {
            image.put_pixel(0, y, image::Rgba([255, 0, 0, 255]));
            for x in 8..16 {
                image.put_pixel(x, y, image::Rgba([0, 0, 255, 255]));
            }
        }
        image.save(&texture).unwrap();
        let provider = serde_json::json!({
            "type": "bitmap",
            "file": "example:font/duplicate.png",
            "height": 8,
            "ascent": 7,
            "chars": ["AA"]
        });
        let mut atlas = ColorAtlasBuilder::new();
        let loaded = load_bitmap_provider(
            provider.as_object().unwrap(),
            &root,
            &None,
            None,
            &mut atlas,
        )
        .unwrap();
        let glyph = loaded.glyphs.get(&'A').unwrap();
        assert_eq!(glyph.advance, 9.0, "last 8px-wide cell must win");
        let offset = (glyph.atlas_layer * MC_FONT_ATLAS_SIZE * MC_FONT_ATLAS_SIZE * 4
            + (glyph.atlas_y * MC_FONT_ATLAS_SIZE + glyph.atlas_x) * 4)
            as usize;
        assert_eq!(&atlas.pixels[offset..offset + 4], &[0, 0, 255, 255]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ttf_provider_uses_font_prefix_list_skip_and_freetype_metrics() {
        let root = std::env::temp_dir().join(format!("pomme-ttf-{}", uuid::Uuid::new_v4()));
        let jar_font = root.join("minecraft/font");
        let custom_font = root.join("example/font");
        std::fs::create_dir_all(&jar_font).unwrap();
        std::fs::create_dir_all(&custom_font).unwrap();
        std::fs::write(
            jar_font.join("default.json"),
            r#"{"providers":[{"type":"space","advances":{"A":5.0}}]}"#,
        )
        .unwrap();
        std::fs::write(
            custom_font.join("custom.json"),
            r#"{"providers":[{"type":"ttf","file":"example:test.ttf","size":8.0,"oversample":1.0,"skip":["A"]}]}"#,
        )
        .unwrap();
        std::fs::write(
            custom_font.join("test.ttf"),
            include_bytes!("../renderer/fonts/Montserrat-Medium.ttf"),
        )
        .unwrap();

        assert_eq!(
            prefixed_resource_location_asset_key("example:test.ttf", "font/").unwrap(),
            "example/font/test.ttf"
        );
        let map = GlyphMap::load(FontSources {
            jar_assets_dir: &root,
            asset_index: &None,
            packs: None,
            options: FontOptions::default(),
        })
        .unwrap();
        let set = map.font_sets.get("example:custom").unwrap();
        assert!(!set.glyphs.contains_key(&'A'), "list-form skip must apply");
        let b = set.glyphs.get(&'B').expect("FreeType-loaded B glyph");
        assert!(b.advance > 0.0);
        assert!(b.pixel_w > 0 && b.pixel_h > 0);
        assert!(!b.colored);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resource_pack_stack_overrides_fonts_and_preserves_colored_bitmap_pixels() {
        let root = std::env::temp_dir().join(format!("pomme-font-stack-{}", uuid::Uuid::new_v4()));
        let jar = root.join("jar_assets");
        let jar_font = jar.join("minecraft/font");
        std::fs::create_dir_all(&jar_font).unwrap();
        std::fs::write(
            jar_font.join("default.json"),
            r#"{"providers":[{"type":"space","advances":{"A":1.0}}]}"#,
        )
        .unwrap();

        let instance = root.join("instance");
        let cache = instance.join("resourcepacks/.server_cache");
        let low = cache.join("low");
        let high = cache.join("high");
        std::fs::create_dir_all(low.join("assets/minecraft/font")).unwrap();
        std::fs::write(
            low.join("assets/minecraft/font/default.json"),
            r#"{"providers":[{"type":"space","advances":{"A":3.0}}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(high.join("assets/minecraft/font")).unwrap();
        std::fs::write(
            high.join("assets/minecraft/font/default.json"),
            r#"{"providers":[{"type":"space","advances":{"A":5.0}}]}"#,
        )
        .unwrap();

        std::fs::create_dir_all(high.join("assets/example/font")).unwrap();
        std::fs::write(
            high.join("assets/example/font/fancy.json"),
            r#"{"providers":[{"type":"space","advances":{"B":7.0}}]}"#,
        )
        .unwrap();
        std::fs::write(
            high.join("assets/example/font/wrapper.json"),
            r#"{"providers":[{"type":"reference","id":"example:fancy"}]}"#,
        )
        .unwrap();

        let texture_dir = high.join("assets/example/textures/font");
        std::fs::create_dir_all(&texture_dir).unwrap();
        let mut image = image::RgbaImage::new(8, 8);
        image.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        image.save(texture_dir.join("color.png")).unwrap();
        std::fs::write(
            high.join("assets/example/font/color.json"),
            r#"{"providers":[{"type":"bitmap","file":"example:font/color.png","height":8,"ascent":7,"chars":["X"]}]}"#,
        )
        .unwrap();

        let mut packs = ResourcePackManager::new(&instance);
        packs.apply_server_pack(uuid::Uuid::from_u128(1), "low");
        packs.apply_server_pack(uuid::Uuid::from_u128(2), "high");
        let no_index = None;
        let map = GlyphMap::load(FontSources {
            jar_assets_dir: &jar,
            asset_index: &no_index,
            packs: Some(&packs),
            options: FontOptions::default(),
        })
        .unwrap();

        assert_eq!(normalize_resource_id("alt").unwrap(), "minecraft:alt");
        assert_eq!(
            font_asset_key("example:fancy").unwrap(),
            "example/font/fancy.json"
        );
        assert_eq!(map.glyph('A', None).advance, 5.0);
        assert_eq!(map.glyph('B', Some("example:fancy")).advance, 7.0);
        assert_eq!(map.glyph('B', Some("example:wrapper")).advance, 7.0);

        let colored = map.glyph('X', Some("example:color"));
        assert!(colored.colored);
        let layer_stride = (map.tex_w * map.tex_h * 4) as usize;
        let offset = colored.atlas_layer as usize * layer_stride
            + ((colored.atlas_y * map.tex_w + colored.atlas_x) * 4) as usize;
        assert_eq!(&map.colored_pixels[offset..offset + 4], &[10, 20, 30, 255]);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accented_provider_keeps_vanilla_twelve_pixel_geometry() {
        let mut image = image::RgbaImage::new(9, 12);
        for y in 0..12 {
            for x in 0..5 {
                image.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let sheet = BitmapSheet {
            image,
            chars: vec!["Á".to_owned()],
            logical_height: 12,
            ascent: 10,
        };
        let mut glyphs = HashMap::new();
        insert_bitmap_sheet(&mut glyphs, &sheet, 0, 0, 37, false);
        let glyph = glyphs.get(&'Á').unwrap();
        assert_eq!((glyph.atlas_x, glyph.atlas_y), (0, 37));
        assert_eq!((glyph.pixel_w, glyph.pixel_h), (9, 12));
        assert_eq!((glyph.draw_w, glyph.draw_h), (9.0, 12.0));
        assert_eq!(glyph.top, -3.0);
        assert_eq!(glyph.advance, 6.0);
    }

    #[test]
    fn alt_font_missing_codepoint_uses_special_missing_box() {
        let mut pixels = vec![0u8; 16 * 8];
        let missing = append_missing_glyph(&mut pixels, 16, 8, 0, 0, 0);
        let mut glyphs = HashMap::new();
        glyphs.insert('1', space_glyph(5.0));
        let mut sga_glyphs = HashMap::new();
        sga_glyphs.insert(' ', space_glyph(4.0));
        let map = GlyphMap {
            font_sets: HashMap::from([
                (
                    "minecraft:default".to_owned(),
                    FontSetData {
                        glyphs,
                        obfuscation_glyphs: HashMap::new(),
                    },
                ),
                (
                    "minecraft:alt".to_owned(),
                    FontSetData {
                        glyphs: sga_glyphs,
                        obfuscation_glyphs: HashMap::new(),
                    },
                ),
            ]),
            missing_glyph: missing,
            cell_w: 8,
            cell_h: 8,
            pixels,
            colored_pixels: Vec::new(),
            tex_w: 16,
            tex_h: 8,
            tex_layers: 1,
            colored_tex_layers: 1,
        };

        assert_eq!(map.glyph('1', None).pixel_w, 0);
        let alt_digit = map.glyph('1', Some("minecraft:alt"));
        assert_eq!((alt_digit.pixel_w, alt_digit.pixel_h), (5, 8));
        assert_eq!(alt_digit.advance, 6.0);
        assert_eq!(map.glyph(' ', Some("minecraft:alt")).advance, 4.0);
    }

    #[test]
    fn resource_pack_font_stack_overrides_default_and_adds_custom_font() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pomme-font-pack-test-{}-{nonce}",
            std::process::id()
        ));
        let jar_assets = root.join("jar_assets");
        let instance = root.join("instance");
        let pack = instance.join("resourcepacks/testpack");
        std::fs::create_dir_all(jar_assets.join("minecraft/font")).unwrap();
        std::fs::create_dir_all(pack.join("assets/minecraft/font")).unwrap();
        std::fs::create_dir_all(pack.join("assets/demo/font")).unwrap();

        std::fs::write(
            jar_assets.join("minecraft/font/default.json"),
            r#"{"providers":[{"type":"space","advances":{"A":5.0}}]}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            r#"{"pack":{"pack_format":55,"description":"font test"}}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("assets/minecraft/font/default.json"),
            r#"{"providers":[{"type":"space","advances":{"A":6.0}}]}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("assets/demo/font/custom.json"),
            r#"{"providers":[{"type":"space","advances":{"B":7.0}}]}"#,
        )
        .unwrap();

        let mut packs = ResourcePackManager::new(&instance);
        packs.enable_local_pack("testpack");
        let asset_index = None;
        let map = GlyphMap::load(FontSources {
            jar_assets_dir: &jar_assets,
            asset_index: &asset_index,
            packs: Some(&packs),
            options: FontOptions::default(),
        })
        .unwrap();
        assert_eq!(map.glyph('A', None).advance, 6.0);
        assert_eq!(map.glyph('B', Some("demo:custom")).advance, 7.0);

        packs.disable_local_pack("testpack");
        let reloaded = GlyphMap::load(FontSources {
            jar_assets_dir: &jar_assets,
            asset_index: &asset_index,
            packs: Some(&packs),
            options: FontOptions::default(),
        })
        .unwrap();
        assert_eq!(reloaded.glyph('A', None).advance, 5.0);
        assert!(!reloaded.font_sets.contains_key("demo:custom"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bitmap_provider_order_is_first_provider_wins() {
        let mut first_image = image::RgbaImage::new(8, 8);
        first_image.put_pixel(1, 0, image::Rgba([255, 255, 255, 255]));
        let first = BitmapSheet {
            image: first_image,
            chars: vec!["X".to_owned()],
            logical_height: 8,
            ascent: 7,
        };
        let mut second_image = image::RgbaImage::new(8, 8);
        second_image.put_pixel(6, 0, image::Rgba([255, 255, 255, 255]));
        let second = BitmapSheet {
            image: second_image,
            chars: vec!["X".to_owned()],
            logical_height: 8,
            ascent: 7,
        };
        let mut glyphs = HashMap::new();
        insert_bitmap_sheet(&mut glyphs, &first, 0, 0, 0, false);
        insert_bitmap_sheet(&mut glyphs, &second, 0, 0, 8, false);
        let glyph = glyphs.get(&'X').unwrap();
        assert_eq!(glyph.atlas_y, 0);
        assert_eq!(glyph.advance, 3.0);
    }
}
