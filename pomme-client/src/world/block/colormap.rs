//! Decoded biome colors belonging to one block-render asset generation.
//! Resource handles stay on the loading thread; only these CPU pixels are
//! shared.

use std::io::Cursor;
use std::sync::Arc;

use crate::resources::{ResourceId, ResourceManager, resource_id};

#[derive(Clone)]
pub(crate) struct Colormaps {
    grass: Arc<Colormap>,
    foliage: Arc<Colormap>,
    dry_foliage: Arc<Colormap>,
}

impl Colormaps {
    pub(crate) fn load(resources: &ResourceManager) -> Self {
        Self {
            grass: Arc::new(Colormap::load(
                resources,
                &resource_id!("minecraft:textures/colormap/grass.png"),
            )),
            foliage: Arc::new(Colormap::load(
                resources,
                &resource_id!("minecraft:textures/colormap/foliage.png"),
            )),
            dry_foliage: Arc::new(Colormap::load(
                resources,
                &resource_id!("minecraft:textures/colormap/dry_foliage.png"),
            )),
        }
    }

    pub(crate) fn grass(&self) -> &Arc<Colormap> {
        &self.grass
    }

    pub(crate) fn foliage(&self) -> &Arc<Colormap> {
        &self.foliage
    }

    pub(crate) fn dry_foliage(&self) -> &Arc<Colormap> {
        &self.dry_foliage
    }
}

impl Default for Colormaps {
    fn default() -> Self {
        let fallback = Arc::new(Colormap::fallback());
        Self {
            grass: Arc::clone(&fallback),
            foliage: Arc::clone(&fallback),
            dry_foliage: fallback,
        }
    }
}

pub(crate) struct Colormap {
    pixels: Vec<[u8; 3]>,
}

impl Colormap {
    fn load(resources: &ResourceManager, id: &ResourceId) -> Self {
        let load = || -> Result<Self, Box<dyn std::error::Error>> {
            let resource = resources.get_resource(id)?.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "colormap not found")
            })?;
            let bytes = resource.content().read_bytes(32 * 1024 * 1024)?;
            // Match the legacy PNG transforms: expand palettes/grayscale and
            // strip 16-bit channels, without gamma or color-space conversion.
            let mut decoder = png::Decoder::new(Cursor::new(bytes));
            decoder
                .set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
            let mut reader = decoder.read_info()?;
            let info = reader.info();
            if info.width != 256 || info.height != 256 {
                return Err(format!(
                    "expected 256x256 colormap, got {}x{}",
                    info.width, info.height
                )
                .into());
            }
            let mut buf = vec![0; reader.output_buffer_size()];
            let info = reader.next_frame(&mut buf)?;
            // An APNG frame can be smaller than the image header's canvas.
            if info.width != 256 || info.height != 256 {
                return Err(format!(
                    "expected 256x256 colormap frame, got {}x{}",
                    info.width, info.height
                )
                .into());
            }
            let data = &buf[..info.buffer_size()];
            // Alpha is not color data. Preserve row-major RGB byte order.
            let pixels = match info.color_type {
                png::ColorType::Rgba => data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| [c[0], c[1], c[2]])
                    .collect(),
                png::ColorType::Rgb => data.as_chunks::<3>().0.to_vec(),
                png::ColorType::GrayscaleAlpha => {
                    data.as_chunks::<2>().0.iter().map(|c| [c[0]; 3]).collect()
                }
                png::ColorType::Grayscale => data.iter().map(|&c| [c; 3]).collect(),
                other => return Err(format!("unsupported PNG color type {other:?}").into()),
            };
            Ok(Self::from_pixels(pixels))
        };
        load().unwrap_or_else(|error| {
            tracing::warn!(colormap = %id, %error, "Using generated fallback colormap");
            Self::fallback()
        })
    }

    /// A map filled with vanilla's default grass color, used when the PNG
    /// can't be loaded.
    pub(crate) fn fallback() -> Self {
        Self::from_pixels(vec![[145, 189, 89]; 256 * 256])
    }

    pub(crate) fn from_pixels(pixels: Vec<[u8; 3]>) -> Self {
        Self { pixels }
    }

    fn pixel(&self, temperature: f32, downfall: f32) -> [u8; 3] {
        let t = temperature.clamp(0.0, 1.0);
        let d = (downfall.clamp(0.0, 1.0)) * t;
        let x = ((1.0 - t) * 255.0) as usize;
        let y = ((1.0 - d) * 255.0) as usize;
        let idx = (y * 256 + x).min(256 * 256 - 1);
        self.pixels[idx]
    }

    pub(crate) fn lookup(&self, temperature: f32, downfall: f32) -> [f32; 3] {
        self.pixel(temperature, downfall)
            .map(|channel| channel as f32 / 255.0)
    }

    /// Vanilla `GrassColor.get` as packed `0xRRGGBB`.
    pub(crate) fn rgb(&self, temperature: f32, downfall: f32) -> u32 {
        let [r, g, b] = self.pixel(temperature, downfall);
        u32::from_be_bytes([0, r, g, b])
    }
}
