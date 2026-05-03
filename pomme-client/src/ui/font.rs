use std::collections::HashMap;
use std::path::Path;

use crate::assets::{AssetIndex, load_image, resolve_asset_path};

const GRID_COLS: u32 = 16;
const GRID_ROWS: u32 = 16;

const ASCII_CHARS: [&str; 16] = [
    "\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}",
    "\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}",
    "\u{0020}\u{0021}\u{0022}\u{0023}\u{0024}\u{0025}\u{0026}\u{0027}\u{0028}\u{0029}\u{002a}\u{002b}\u{002c}\u{002d}\u{002e}\u{002f}",
    "\u{0030}\u{0031}\u{0032}\u{0033}\u{0034}\u{0035}\u{0036}\u{0037}\u{0038}\u{0039}\u{003a}\u{003b}\u{003c}\u{003d}\u{003e}\u{003f}",
    "\u{0040}\u{0041}\u{0042}\u{0043}\u{0044}\u{0045}\u{0046}\u{0047}\u{0048}\u{0049}\u{004a}\u{004b}\u{004c}\u{004d}\u{004e}\u{004f}",
    "\u{0050}\u{0051}\u{0052}\u{0053}\u{0054}\u{0055}\u{0056}\u{0057}\u{0058}\u{0059}\u{005a}\u{005b}\u{005c}\u{005d}\u{005e}\u{005f}",
    "\u{0060}\u{0061}\u{0062}\u{0063}\u{0064}\u{0065}\u{0066}\u{0067}\u{0068}\u{0069}\u{006a}\u{006b}\u{006c}\u{006d}\u{006e}\u{006f}",
    "\u{0070}\u{0071}\u{0072}\u{0073}\u{0074}\u{0075}\u{0076}\u{0077}\u{0078}\u{0079}\u{007a}\u{007b}\u{007c}\u{007d}\u{007e}\u{0000}",
    "\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}",
    "\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{00a3}\u{0000}\u{0000}\u{0192}",
    "\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{00aa}\u{00ba}\u{0000}\u{0000}\u{00ac}\u{0000}\u{0000}\u{0000}\u{00ab}\u{00bb}",
    "\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}",
    "\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}",
    "\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}",
    "\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{0000}\u{2205}\u{2208}\u{0000}",
    "\u{2261}\u{00b1}\u{2265}\u{2264}\u{2320}\u{2321}\u{00f7}\u{2248}\u{00b0}\u{2219}\u{0000}\u{221a}\u{207f}\u{00b2}\u{25a0}\u{0000}",
];

#[derive(Clone)]
pub(crate) struct GlyphInfo {
    pub col: u32,
    pub row: u32,
    pub width: u32,
    pub y_offset: u32,
    pub height: u32,
}

pub struct GlyphMap {
    pub(crate) glyphs: HashMap<char, GlyphInfo>,
    pub(crate) cell_w: u32,
    pub(crate) cell_h: u32,
    pixels: Vec<u8>,
    tex_w: u32,
    tex_h: u32,
}

impl GlyphMap {
    pub fn load(jar_assets_dir: &Path, asset_index: &Option<AssetIndex>) -> Option<Self> {
        let path = resolve_asset_path(
            jar_assets_dir,
            asset_index,
            "minecraft/textures/font/ascii.png",
        );
        let img = load_image(&path)
            .map_err(|e| tracing::warn!("Failed to load MC font: {e}"))
            .ok()?
            .to_rgba8();

        let tex_w = img.width();
        let tex_h = img.height();
        let cell_w = tex_w / GRID_COLS;
        let cell_h = tex_h / GRID_ROWS;

        let mut glyphs = HashMap::new();
        for (row, line) in ASCII_CHARS.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                if ch == '\0' {
                    continue;
                }
                let bounds = detect_glyph_bounds(
                    &img,
                    col as u32 * cell_w,
                    row as u32 * cell_h,
                    cell_w,
                    cell_h,
                );
                if let Some((width, y_offset, height)) = bounds {
                    glyphs.insert(
                        ch,
                        GlyphInfo {
                            col: col as u32,
                            row: row as u32,
                            width,
                            y_offset,
                            height,
                        },
                    );
                }
            }
        }
        glyphs.insert(
            ' ',
            GlyphInfo {
                col: 0,
                row: 2,
                width: cell_w / 2,
                y_offset: 0,
                height: cell_h,
            },
        );

        let pixels = img.into_raw();
        Some(Self {
            glyphs,
            cell_w,
            cell_h,
            pixels,
            tex_w,
            tex_h,
        })
    }

    pub fn raw_pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.tex_w, self.tex_h)
    }
}

fn detect_glyph_bounds(
    img: &image::RgbaImage,
    x0: u32,
    y0: u32,
    cell_w: u32,
    cell_h: u32,
) -> Option<(u32, u32, u32)> {
    let mut max_x: u32 = 0;
    let mut min_y: u32 = cell_h;
    let mut max_y: u32 = 0;

    for dy in 0..cell_h {
        for dx in 0..cell_w {
            if img.get_pixel(x0 + dx, y0 + dy)[3] > 0 {
                max_x = max_x.max(dx + 1);
                min_y = min_y.min(dy);
                max_y = max_y.max(dy + 1);
            }
        }
    }

    if max_x == 0 {
        return None;
    }

    Some((max_x, min_y, max_y - min_y))
}
