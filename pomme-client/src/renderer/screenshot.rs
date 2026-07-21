use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use super::util;

/// A capture whose GPU copy was recorded during a given frame; its host
/// readback runs once that frame's fence signals (see
/// `Renderer::render_frame`), so the staging buffer is provably done being
/// written by then.
struct PendingCapture {
    frame: usize,
    buffer: vk::Buffer,
    allocation: Allocation,
    width: u32,
    height: u32,
    bgra: bool,
}

/// Vanilla F2 (`Screenshot.grab`): copies the presented swapchain image into a
/// host buffer, then encodes a PNG off-thread.
pub struct ScreenshotCapture {
    armed: bool,
    pending: Vec<PendingCapture>,
    result_tx: Sender<Result<String, String>>,
    result_rx: Receiver<Result<String, String>>,
}

impl Default for ScreenshotCapture {
    fn default() -> Self {
        let (result_tx, result_rx) = channel();
        Self {
            armed: false,
            pending: Vec::new(),
            result_tx,
            result_rx,
        }
    }
}

impl ScreenshotCapture {
    /// Arm a one-shot capture; recorded on the next presented frame.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// Drain completed captures: `Ok(relative filename)` or `Err(message)`.
    pub fn drain_results(&mut self) -> Vec<Result<String, String>> {
        self.result_rx.try_iter().collect()
    }

    /// If armed, record the image->buffer copy into `cmd` after the final
    /// render pass (image is in `PresentSrcKHR`), transitioning back to
    /// present after.
    #[allow(clippy::too_many_arguments)]
    pub fn record_if_armed(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        cmd: vk::CommandBuffer,
        frame: usize,
        image: vk::Image,
        extent: vk::Extent2D,
        format: vk::Format,
    ) {
        if !self.armed {
            return;
        }
        self.armed = false;

        let size = u64::from(extent.width) * u64::from(extent.height) * 4;
        let (buffer, allocation) = util::create_host_buffer(
            device,
            allocator,
            size,
            vk::BufferUsageFlags::TransferDst,
            "screenshot_readback",
        );

        let to_transfer = vk::ImageMemoryBarrier {
            image,
            old_layout: vk::ImageLayout::PresentSrcKHR,
            new_layout: vk::ImageLayout::TransferSrcOptimal,
            src_access_mask: vk::AccessFlags::ColorAttachmentWrite,
            dst_access_mask: vk::AccessFlags::TransferRead,
            subresource_range: util::COLOR_SUBRESOURCE_RANGE,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ColorAttachmentOutput,
            vk::PipelineStageFlags::Transfer,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer],
        );

        let region = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::Color,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            },
        };
        cmd.copy_image_to_buffer(
            image,
            vk::ImageLayout::TransferSrcOptimal,
            buffer,
            &[region],
        );

        let to_present = vk::ImageMemoryBarrier {
            image,
            old_layout: vk::ImageLayout::TransferSrcOptimal,
            new_layout: vk::ImageLayout::PresentSrcKHR,
            src_access_mask: vk::AccessFlags::TransferRead,
            dst_access_mask: vk::AccessFlags::empty(),
            subresource_range: util::COLOR_SUBRESOURCE_RANGE,
            ..Default::default()
        };
        // The buffer barrier makes the copy visible to the mapped host read that
        // follows the frame fence; without the HOST stage that read races the copy.
        let host_read = vk::BufferMemoryBarrier {
            buffer,
            offset: 0,
            size: vk::WHOLE_SIZE,
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::HostRead,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::BottomOfPipe | vk::PipelineStageFlags::Host,
            vk::DependencyFlags::empty(),
            &[],
            &[host_read],
            &[to_present],
        );

        self.pending.push(PendingCapture {
            frame,
            buffer,
            allocation,
            width: extent.width,
            height: extent.height,
            bgra: is_bgra(format),
        });
    }

    /// Read back and encode any capture recorded for this frame index (its
    /// fence has just signalled). Called from the per-frame fence wait, not
    /// idle-wait.
    pub fn collect_ready(
        &mut self,
        frame: usize,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
    ) {
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].frame == frame {
                let cap = self.pending.remove(i);
                self.read_and_spawn(cap, device, allocator);
            } else {
                i += 1;
            }
        }
    }

    fn read_and_spawn(
        &self,
        cap: PendingCapture,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
    ) {
        let px_bytes = cap.width as usize * cap.height as usize * 4;
        let pixels = cap
            .allocation
            .mapped_slice()
            .map(|s| s[..px_bytes].to_vec());

        device.destroy_buffer(cap.buffer, None);
        allocator.lock().unwrap().free(cap.allocation).ok();

        let Some(pixels) = pixels else {
            let _ = self
                .result_tx
                .send(Err("screenshot buffer was not host-visible".into()));
            return;
        };

        let tx = self.result_tx.clone();
        let (w, h, bgra) = (cap.width, cap.height, cap.bgra);
        std::thread::spawn(move || {
            let _ = tx.send(encode_and_write(&pixels, w, h, bgra));
        });
    }

    /// Free any buffers still awaiting readback (renderer teardown, after
    /// idle).
    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for cap in self.pending.drain(..) {
            device.destroy_buffer(cap.buffer, None);
            allocator.lock().unwrap().free(cap.allocation).ok();
        }
    }
}

fn is_bgra(format: vk::Format) -> bool {
    matches!(format, vk::Format::B8G8R8A8Srgb | vk::Format::B8G8R8A8Unorm)
}

fn encode_and_write(pixels: &[u8], width: u32, height: u32, bgra: bool) -> Result<String, String> {
    // Vanilla screenshots are opaque RGB; drop alpha and reorder BGRA if needed.
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for px in pixels.chunks_exact(4) {
        if bgra {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        } else {
            rgb.extend_from_slice(&[px[0], px[1], px[2]]);
        }
    }

    let (path, relative) = next_filename()?;
    write_png(&path, &rgb, width, height)?;
    Ok(relative)
}

/// `Screenshot.getFilename`: `screenshots/<timestamp>.png`, suffixed `_1`,
/// `_2`, ... on collision.
fn next_filename() -> Result<(PathBuf, String), String> {
    let dir = PathBuf::from("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let stamp = timestamp();
    let mut n = 0u32;
    loop {
        let name = if n == 0 {
            format!("{stamp}.png")
        } else {
            format!("{stamp}_{n}.png")
        };
        let path = dir.join(&name);
        if !path.exists() {
            return Ok((path, format!("screenshots/{name}")));
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
