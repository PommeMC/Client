use std::path::Path;

use super::{MAX_FRAMES_IN_FLIGHT, rhi};
use crate::assets::{AssetIndex, resolve_asset_path};

/// A texture holding `pixels`, ready for shaders to read.
pub fn create_sampled_texture(
    device: &rhi::backend::Device,
    label: &str,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> rhi::backend::Texture {
    let texture = rhi::backend::Texture::new(device, &sampled_texture_desc(label, width, height));
    upload_image(
        device,
        texture.handle(),
        pixels,
        width,
        height,
        &format!("{label}_staging"),
    );
    texture
}

/// The texture at `key`, or a 16x16 white one if it won't load.
pub fn load_asset_texture(
    device: &rhi::backend::Device,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
    key: &str,
) -> rhi::backend::Texture {
    let path = resolve_asset_path(jar_assets_dir, asset_index, key);
    let (pixels, w, h) = load_png(&path).unwrap_or_else(|| {
        tracing::warn!("Failed to load {key}, using fallback");
        (vec![255u8; 16 * 16 * 4], 16, 16)
    });
    let texture = create_sampled_texture(device, key, &pixels, w, h);
    tracing::info!("Loaded {key} ({w}x{h})");
    texture
}

/// A texture filled by copies and read by shaders: plain RGBA8 like
/// vanilla's, which samples the gamma-space texels as they are.
pub fn sampled_texture_desc(label: &str, width: u32, height: u32) -> rhi::TextureDesc<'_> {
    rhi::TextureDesc::new(
        label,
        width,
        height,
        rhi::TextureFormat::Rgba8Unorm,
        rhi::TextureUsages::COPY_DST | rhi::TextureUsages::TEXTURE_BINDING,
    )
}

/// One host-visible buffer per frame in flight.
pub fn create_per_frame_host_buffers(
    device: &rhi::backend::Device,
    size: u64,
    usage: rhi::BufferUsages,
    name: &str,
) -> Vec<rhi::backend::Buffer> {
    (0..MAX_FRAMES_IN_FLIGHT)
        .map(|_| rhi::backend::Buffer::new(device, name, size, usage, rhi::MemoryLocation::Host))
        .collect()
}

/// A uniform buffer for the vertex stage, the layout [`create_uniform_sets`]
/// binds.
pub const VERTEX_UNIFORM: &[rhi::BindGroupLayoutEntry] = &[rhi::BindGroupLayoutEntry::uniform(
    rhi::ShaderStages::VERTEX,
)];
/// A uniform buffer for the vertex and fragment stages.
pub const VERTEX_FRAGMENT_UNIFORM: &[rhi::BindGroupLayoutEntry] =
    &[rhi::BindGroupLayoutEntry::uniform(
        rhi::ShaderStages::VERTEX_FRAGMENT,
    )];
/// A texture for the fragment stage, then its sampler.
pub const FRAGMENT_TEXTURE: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::texture(rhi::ShaderStages::FRAGMENT),
    rhi::BindGroupLayoutEntry::sampler(rhi::ShaderStages::FRAGMENT),
];

/// The bindings of a [`FRAGMENT_TEXTURE`] group: `texture`, sampled with
/// `sampler`.
pub fn texture_binding(
    texture: rhi::backend::TextureHandle,
    sampler: rhi::backend::Sampler,
) -> [(u32, rhi::backend::BindingResource); 2] {
    [
        (0, rhi::backend::BindingResource::Texture(texture)),
        (1, rhi::backend::BindingResource::Sampler(sampler)),
    ]
}

/// A [`FRAGMENT_TEXTURE`] group pointed at `texture`.
pub fn create_texture_group(
    device: &rhi::backend::Device,
    layout: rhi::backend::BindGroupLayout,
    texture: rhi::backend::TextureHandle,
    sampler: rhi::backend::Sampler,
) -> rhi::backend::BindGroup {
    device.create_bind_group(layout, &texture_binding(texture, sampler))
}

/// One `size`-byte uniform buffer per frame in flight, each bound to its own
/// bind group at binding 0, so a frame never rewrites one the GPU may still
/// be reading.
pub fn create_uniform_sets(
    device: &rhi::backend::Device,
    layout: rhi::backend::BindGroupLayout,
    size: u64,
    name: &str,
) -> (Vec<rhi::backend::BindGroup>, Vec<rhi::backend::Buffer>) {
    let buffers = create_per_frame_host_buffers(device, size, rhi::BufferUsages::UNIFORM, name);
    let groups = buffers
        .iter()
        .map(|buffer| {
            device.create_bind_group(
                layout,
                &[(0, rhi::backend::BindingResource::uniform(buffer, size))],
            )
        })
        .collect();
    (groups, buffers)
}

/// Destroys buffers made by [`create_per_frame_host_buffers`] or
/// [`create_uniform_sets`] and frees their memory.
pub fn destroy_buffers(device: &rhi::backend::Device, buffers: &mut Vec<rhi::backend::Buffer>) {
    for buffer in buffers.drain(..) {
        buffer.destroy(device);
    }
}

/// Frees bind groups made by [`create_uniform_sets`] or
/// [`create_texture_group`].
pub fn destroy_bind_groups(
    device: &rhi::backend::Device,
    groups: impl IntoIterator<Item = rhi::backend::BindGroup>,
) {
    for group in groups {
        group.destroy(device);
    }
}

/// Uploads RGBA8 `pixels` into `image` through a staging buffer, freed once
/// the copy has finished.
pub fn upload_image(
    device: &rhi::backend::Device,
    image: rhi::backend::TextureHandle,
    pixels: &[u8],
    width: u32,
    height: u32,
    label: &str,
) {
    let staging =
        rhi::backend::Buffer::with_data(device, label, pixels, rhi::BufferUsages::COPY_SRC);
    upload_image_mipmapped(
        device,
        staging.handle(),
        rgba8_bytes(width, height),
        image,
        width,
        height,
        1,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    staging.destroy(device);
}

#[derive(Clone, Copy)]
pub struct ImageArrayExtent {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

pub fn upload_image_array(
    device: &rhi::backend::Device,
    staging_buffer: rhi::backend::BufferHandle,
    image: rhi::backend::TextureHandle,
    extent: ImageArrayExtent,
    bytes_per_pixel: u64,
) -> Result<(), String> {
    let ImageArrayExtent {
        width,
        height,
        layers,
    } = extent;
    let layer_bytes = u64::from(width) * u64::from(height) * bytes_per_pixel;
    device.try_submit(|encoder| {
        let range = rhi::TextureRange {
            layers,
            ..rhi::TextureRange::BASE
        };
        encoder.upload_texture(
            staging_buffer,
            image,
            range,
            &layer_copies(width, height, layers, layer_bytes),
        );
    })
}

/// Base level copies of `layers` layers, each `layer_bytes` after the last.
pub fn layer_copies(width: u32, height: u32, layers: u32, layer_bytes: u64) -> Vec<rhi::TexelCopy> {
    (0..layers)
        .map(|layer| rhi::TexelCopy {
            buffer_offset: layer_bytes * u64::from(layer),
            mip_level: 0,
            layer,
            origin: [0, 0],
            width,
            height,
        })
        .collect()
}

/// Blits `frame`, a colour attachment between passes, into `dst` (scaled to
/// `dst_size`), leaving `dst` for shaders to read and `frame` for the next
/// pass.
pub fn copy_frame(
    encoder: &rhi::backend::CommandEncoder,
    frame: rhi::backend::TextureHandle,
    frame_size: [u32; 2],
    dst: rhi::backend::TextureHandle,
    dst_size: [u32; 2],
) {
    use rhi::TextureUse;
    use rhi::backend::TextureTransition;
    encoder.transition_textures(&[
        TextureTransition::color(frame, TextureUse::ColorAttachment, TextureUse::CopySrc),
        TextureTransition::color(dst, TextureUse::Undefined, TextureUse::CopyDst),
    ]);
    encoder.blit(
        frame,
        dst,
        &rhi::Blit {
            src_level: 0,
            src_size: frame_size,
            dst_level: 0,
            dst_size,
            layers: 1,
        },
    );
    encoder.transition_textures(&[
        TextureTransition::color(dst, TextureUse::CopyDst, TextureUse::ShaderRead),
        TextureTransition::color(frame, TextureUse::CopySrc, TextureUse::ColorAttachment),
    ]);
}

pub fn transition_image_to_shader_read(
    device: &rhi::backend::Device,
    image: rhi::backend::TextureHandle,
) {
    device.submit(|encoder| {
        encoder.transition_textures(&[rhi::backend::TextureTransition::color(
            image,
            rhi::TextureUse::Undefined,
            rhi::TextureUse::ShaderRead,
        )]);
    });
}

pub fn load_png(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;

    let data = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            let pixels = info.width as usize * info.height as usize;
            let mut rgba = Vec::with_capacity(pixels * 4);
            for chunk in buf[..pixels * 3].as_chunks::<3>().0 {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        png::ColorType::GrayscaleAlpha => {
            let pixels = info.width as usize * info.height as usize;
            let mut rgba = Vec::with_capacity(pixels * 4);
            for chunk in buf[..pixels * 2].as_chunks::<2>().0 {
                rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            rgba
        }
        png::ColorType::Grayscale => {
            let pixels = info.width as usize * info.height as usize;
            let mut rgba = Vec::with_capacity(pixels * 4);
            for &g in &buf[..pixels] {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
            rgba
        }
        other => {
            tracing::warn!("Unsupported PNG color type {other:?}: {}", path.display());
            return None;
        }
    };

    Some((data, info.width, info.height))
}

/// Uploads an image whose mip levels are all present in the staging buffer,
/// packed tightly with level 0 first.
pub fn upload_image_mipmapped(
    device: &rhi::backend::Device,
    staging_buffer: rhi::backend::BufferHandle,
    staging_size: u64,
    image: rhi::backend::TextureHandle,
    width: u32,
    height: u32,
    mip_levels: u32,
) -> Result<(), String> {
    device.try_submit(|encoder| {
        record_image_upload(
            encoder,
            staging_buffer,
            staging_size,
            image,
            width,
            height,
            mip_levels,
        );
    })
}

pub struct PendingImageUpload {
    pub staging_buffer: rhi::backend::BufferHandle,
    pub staging_size: u64,
    pub image: rhi::backend::TextureHandle,
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
}

pub fn upload_images_batched(device: &rhi::backend::Device, uploads: &[PendingImageUpload]) {
    if uploads.is_empty() {
        return;
    }
    device.submit(|encoder| {
        for u in uploads {
            record_image_upload(
                encoder,
                u.staging_buffer,
                u.staging_size,
                u.image,
                u.width,
                u.height,
                u.mip_levels,
            );
        }
    });
}

fn rgba8_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 4
}

fn record_image_upload(
    encoder: &rhi::backend::CommandEncoder,
    staging_buffer: rhi::backend::BufferHandle,
    staging_size: u64,
    image: rhi::backend::TextureHandle,
    width: u32,
    height: u32,
    mip_levels: u32,
) {
    let mut buffer_offset = 0u64;
    let copies: Vec<_> = (0..mip_levels)
        .map(|level| {
            let w = (width >> level).max(1);
            let h = (height >> level).max(1);
            let copy = rhi::TexelCopy {
                buffer_offset,
                mip_level: level,
                layer: 0,
                origin: [0, 0],
                width: w,
                height: h,
            };
            buffer_offset += rgba8_bytes(w, h);
            copy
        })
        .collect();

    debug_assert_eq!(
        buffer_offset, staging_size,
        "staging buffer ({staging_size} bytes) must hold exactly all {mip_levels} mip levels \
         ({buffer_offset} bytes); a mismatch would copy out of bounds"
    );

    encoder.upload_texture(
        staging_buffer,
        image,
        rhi::TextureRange {
            mips: mip_levels,
            ..rhi::TextureRange::BASE
        },
        &copies,
    );
}
