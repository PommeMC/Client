use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use super::{rhi, util};

/// A capture whose GPU copy was recorded during a given frame; its host
/// readback runs once that frame's fence signals (see
/// `Renderer::render_frame`), so the staging buffer is provably done being
/// written by then.
struct PendingCapture {
    frame: usize,
    buffer: rhi::backend::Buffer,
    width: u32,
    height: u32,
    bgra: bool,
    target: Option<PathBuf>,
}

/// Vanilla F2 (`Screenshot.grab`): copies the presented swapchain image into a
/// host buffer, then encodes a PNG off-thread.
pub struct ScreenshotCapture {
    armed: bool,
    /// Where the armed capture goes instead of `screenshots/<timestamp>.png`.
    target: Option<PathBuf>,
    pending: Vec<PendingCapture>,
    /// Encodes spawned but not yet drained from `result_rx`.
    in_flight: u32,
    game_dir: PathBuf,
    result_tx: Sender<Result<String, String>>,
    result_rx: Receiver<Result<String, String>>,
}

impl ScreenshotCapture {
    pub fn new(game_dir: PathBuf) -> Self {
        let (result_tx, result_rx) = channel();
        Self {
            armed: false,
            target: None,
            pending: Vec::new(),
            in_flight: 0,
            game_dir,
            result_tx,
            result_rx,
        }
    }

    /// Arm a one-shot capture; recorded on the next presented frame.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// [`ScreenshotCapture::arm`], writing the PNG to `path`.
    #[cfg(feature = "test-harness")]
    pub fn arm_to(&mut self, path: PathBuf) {
        self.armed = true;
        self.target = Some(path);
    }

    /// A capture is somewhere between armed and written to disk; drives the
    /// HUD saving indicator.
    pub fn saving(&self) -> bool {
        self.armed || !self.pending.is_empty() || self.in_flight > 0
    }

    /// Drain completed captures: `Ok(bare filename)` or `Err(message)`.
    pub fn drain_results(&mut self) -> Vec<Result<String, String>> {
        let results: Vec<_> = self.result_rx.try_iter().collect();
        self.in_flight = self.in_flight.saturating_sub(results.len() as u32);
        results
    }

    /// If armed, record the image->buffer copy into `encoder` after the final
    /// render pass, while the image is ready to present, and hand it back to
    /// present after.
    pub fn record_if_armed(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        image: rhi::backend::TextureHandle,
        extent: rhi::Extent,
        bgra: bool,
    ) {
        if !self.armed {
            return;
        }
        self.armed = false;

        let size = u64::from(extent.width) * u64::from(extent.height) * 4;
        let buffer = rhi::backend::Buffer::new(
            device,
            "screenshot_readback",
            size,
            rhi::BufferUsages::COPY_DST,
            rhi::MemoryLocation::Readback,
        );
        encoder.transition_textures(&[rhi::backend::TextureTransition::color(
            image,
            rhi::TextureUse::Present,
            rhi::TextureUse::CopySrc,
        )]);

        encoder.copy_texture_to_buffer(
            image,
            buffer.handle(),
            &util::layer_copies(extent.width, extent.height, 1, 0),
        );

        encoder.transition_textures(&[rhi::backend::TextureTransition::color(
            image,
            rhi::TextureUse::CopySrc,
            rhi::TextureUse::Present,
        )]);
        // Makes the copy visible to the mapped host read that follows the frame
        // fence; without the HOST stage that read races the copy.
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::HostRead);

        self.pending.push(PendingCapture {
            frame,
            buffer,
            width: extent.width,
            height: extent.height,
            bgra,
            target: self.target.take(),
        });
    }

    /// Read back and encode any capture recorded for this frame index (its
    /// fence has just signalled). Called from the per-frame fence wait, not
    /// idle-wait.
    pub fn collect_ready(&mut self, frame: usize, device: &rhi::backend::Device) {
        let mut i = 0;
        while i < self.pending.len() {
            let cap = &mut self.pending[i];
            let px_bytes = cap.width as usize * cap.height as usize * 4;
            let pixels =
                (cap.frame == frame).then(|| cap.buffer.read(|bytes| bytes[..px_bytes].to_vec()));
            match pixels {
                Some(pixels) => {
                    let cap = self.pending.remove(i);
                    self.spawn_encode(cap, pixels, device);
                }
                None => i += 1,
            }
        }
    }

    fn spawn_encode(
        &mut self,
        cap: PendingCapture,
        pixels: Vec<u8>,
        device: &rhi::backend::Device,
    ) {
        self.in_flight += 1;
        cap.buffer.destroy(device);

        let tx = self.result_tx.clone();
        let (w, h, bgra) = (cap.width, cap.height, cap.bgra);
        let dir = self.game_dir.join("screenshots");
        let target = cap.target;
        std::thread::spawn(move || {
            let _ = tx.send(encode_and_write(&pixels, w, h, bgra, &dir, target));
        });
    }

    /// Free any buffers still awaiting readback (renderer teardown, after
    /// idle).
    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        for cap in self.pending.drain(..) {
            cap.buffer.destroy(device);
        }
    }
}

fn encode_and_write(
    pixels: &[u8],
    width: u32,
    height: u32,
    bgra: bool,
    dir: &Path,
    target: Option<PathBuf>,
) -> Result<String, String> {
    // Vanilla screenshots are opaque RGB; drop alpha and reorder BGRA if needed.
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for px in pixels.as_chunks::<4>().0 {
        if bgra {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        } else {
            rgb.extend_from_slice(&[px[0], px[1], px[2]]);
        }
    }

    let (path, name) = match target {
        Some(path) => {
            let name = path.display().to_string();
            (path, name)
        }
        None => next_filename(dir)?,
    };
    write_png(&path, &rgb, width, height)?;
    Ok(name)
}

/// `Screenshot.getFile`: `<game dir>/screenshots/<timestamp>.png`; on
/// collision the counter starts at 2 (`name.png`, `name_2.png`, ...).
fn next_filename(dir: &Path) -> Result<(PathBuf, String), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let stamp = timestamp();
    let mut n = 1u32;
    loop {
        let name = if n == 1 {
            format!("{stamp}.png")
        } else {
            format!("{stamp}_{n}.png")
        };
        let path = dir.join(&name);
        if !path.exists() {
            return Ok((path, name));
        }
        n += 1;
    }
}

fn timestamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    now.format(time::macros::format_description!(
        "[year]-[month]-[day]_[hour].[minute].[second]"
    ))
    .unwrap_or_else(|_| "screenshot".into())
}

fn write_png(path: &Path, rgb: &[u8], width: u32, height: u32) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgb).map_err(|e| e.to_string())?;
    Ok(())
}
