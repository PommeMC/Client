//! Pomme's render hardware interface: backend-neutral descriptions of GPU
//! state, which the backend translates into its own objects. The shapes follow
//! WebGPU's (bind groups, vertex buffer layouts), apart from push constants;
//! Vulkan is the backend.

pub mod vulkan;

/// The backend this build draws with; renderer code reaches the GPU only
/// through it.
pub use vulkan as backend;

/// A shader as [`crate::renderer::shader::include_shader`] gives it to the
/// backend.
pub type Shader<'a> = &'a [u8];

/// A set of flags with `contains` and `|`, like WebGPU's flag types.
macro_rules! flags {
    ($(#[$doc:meta])* $name:ident { $($flag:ident = $bit:literal),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub struct $name(u8);

        impl $name {
            $(pub const $flag: Self = Self(1 << $bit);)*

            pub fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }
        }

        impl std::ops::BitOr for $name {
            type Output = Self;

            fn bitor(self, rhs: Self) -> Self {
                Self(self.0 | rhs.0)
            }
        }
    };
}

flags! {
    /// Shader stages a binding or push constant is visible to.
    ShaderStages { VERTEX = 0, FRAGMENT = 1, COMPUTE = 2 }
}

impl ShaderStages {
    pub const VERTEX_FRAGMENT: Self = Self(Self::VERTEX.0 | Self::FRAGMENT.0);
}

flags! {
    /// What a buffer can be used for.
    BufferUsages {
        VERTEX = 0,
        INDEX = 1,
        UNIFORM = 2,
        STORAGE = 3,
        INDIRECT = 4,
        COPY_SRC = 5,
        COPY_DST = 6,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

impl Extent {
    pub fn aspect_ratio(self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }
}

/// A rectangle in texels from the top-left: what scissors, viewports and
/// mid-pass clears cover.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn covering(extent: Extent) -> Self {
        Self {
            x: 0,
            y: 0,
            width: extent.width,
            height: extent.height,
        }
    }
}

/// Mip levels and array layers of a color texture, as WebGPU's texture views
/// select them.
#[derive(Clone, Copy)]
pub struct TextureRange {
    pub first_mip: u32,
    pub mips: u32,
    pub first_layer: u32,
    pub layers: u32,
}

impl TextureRange {
    pub const BASE: Self = Self {
        first_mip: 0,
        mips: 1,
        first_layer: 0,
        layers: 1,
    };
}

/// A span copied between buffers, as WebGPU's `copyBufferToBuffer` takes it.
#[derive(Clone, Copy)]
pub struct BufferCopy {
    pub src_offset: u64,
    pub dst_offset: u64,
    pub size: u64,
}

/// One layer of one mip level of a texture, whole, and where its texels sit
/// tightly packed in a buffer: the buffer-texture copies Pomme makes.
#[derive(Clone, Copy)]
pub struct TexelCopy {
    pub buffer_offset: u64,
    pub mip_level: u32,
    pub layer: u32,
    /// The top-left texel written in the level.
    pub origin: [u32; 2],
    pub width: u32,
    pub height: u32,
}

/// A filtered, scaling copy of a mip level of the first `layers` layers into
/// another level, each whole.
#[derive(Clone, Copy)]
pub struct Blit {
    pub src_level: u32,
    pub src_size: [u32; 2],
    pub dst_level: u32,
    pub dst_size: [u32; 2],
    pub layers: u32,
}

/// How a texture is used between commands. Vulkan moves between uses with
/// barriers; WebGPU tracks them itself and ignores transitions.
#[derive(Clone, Copy)]
pub enum TextureUse {
    /// Contents don't matter; only valid as the use being left.
    Undefined,
    CopySrc,
    CopyDst,
    ShaderRead,
    ColorAttachment,
    Present,
}

/// `range` of `texture` moving from one use to the next.
#[derive(Clone, Copy)]
pub struct TextureTransition<Texture> {
    pub texture: Texture,
    pub range: TextureRange,
    pub from: TextureUse,
    pub to: TextureUse,
}

impl<Texture> TextureTransition<Texture> {
    /// The single mip and layer of a color `texture`.
    pub fn color(texture: Texture, from: TextureUse, to: TextureUse) -> Self {
        Self {
            texture,
            range: TextureRange::BASE,
            from,
            to,
        }
    }
}

/// How buffer memory is used between commands; see [`TextureUse`].
#[derive(Clone, Copy)]
pub enum BufferUse {
    CopySrc,
    CopyDst,
    StorageWrite,
    Indirect,
    HostRead,
}

/// The optional features behind the chunk pass's indirect draws.
#[derive(Clone, Copy, Debug)]
pub struct IndirectDrawFeatures {
    /// A draw count read from a buffer (`vkCmdDrawIndexedIndirectCount`,
    /// core in Vulkan 1.2; WebGPU has none). Without it the whole draw list
    /// is drawn, its unfilled slots zeroed into no-op draws.
    pub count: bool,
    /// Several indirect draws per call (`multiDrawIndirect`). Without it each
    /// draw-list slot is its own call.
    pub multi_draw: bool,
}

pub enum Acquired {
    Image {
        index: u32,
        suboptimal: bool,
    },
    /// The swapchain no longer matches its surface and needs rebuilding.
    OutOfDate,
}

pub enum Presented {
    Done,
    Suboptimal,
    OutOfDate,
}

/// Where a buffer's memory lives.
#[derive(Clone, Copy)]
pub enum MemoryLocation {
    /// Written by the CPU.
    Host,
    /// GPU-only, filled by copies.
    Device,
    /// A copy destination the CPU reads back, starting zeroed.
    Readback,
}

flags! {
    /// What a texture can be used for.
    TextureUsages {
        COPY_SRC = 0,
        COPY_DST = 1,
        TEXTURE_BINDING = 2,
        RENDER_ATTACHMENT = 3,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    R8Unorm,
    Rgba8Unorm,
    Rgba8UnormSrgb,
    Depth32Float,
}

impl TextureFormat {
    /// Bytes per texel, as copies into the texture count them.
    pub fn texel_size(self) -> u32 {
        match self {
            Self::R8Unorm => 1,
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb | Self::Depth32Float => 4,
        }
    }
}

/// How a texture's view sees its layers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextureViewDimension {
    D2,
    D2Array,
    /// Six layers, one per face.
    Cube,
}

pub struct TextureDesc<'a> {
    pub label: &'a str,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub mip_levels: u32,
    pub format: TextureFormat,
    pub usage: TextureUsages,
    pub dimension: TextureViewDimension,
}

impl TextureDesc<'static> {
    /// A swapchain's depth buffer.
    pub fn swapchain_depth(extent: Extent) -> Self {
        Self::new(
            "depth_image",
            extent.width,
            extent.height,
            TextureFormat::Depth32Float,
            TextureUsages::RENDER_ATTACHMENT,
        )
    }
}

impl<'a> TextureDesc<'a> {
    /// One 2D layer, without mips.
    pub fn new(
        label: &'a str,
        width: u32,
        height: u32,
        format: TextureFormat,
        usage: TextureUsages,
    ) -> Self {
        Self {
            label,
            width,
            height,
            layers: 1,
            mip_levels: 1,
            format,
            usage,
            dimension: TextureViewDimension::D2,
        }
    }
}

#[derive(Clone, Copy)]
pub enum LoadOp<T> {
    Clear(T),
    Load,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreOp {
    Store,
    Discard,
}

/// A pass's color target, in the shape of WebGPU's
/// `GPURenderPassColorAttachment`.
#[derive(Clone, Copy)]
pub struct ColorAttachment<Texture, Format> {
    pub texture: Texture,
    pub format: Format,
    pub load: LoadOp<[f32; 4]>,
    pub store: StoreOp,
}

/// A pass's `Depth32Float` target.
#[derive(Clone, Copy)]
pub struct DepthAttachment<Texture> {
    pub texture: Texture,
    pub load: LoadOp<f32>,
    pub store: StoreOp,
}

impl<Texture> DepthAttachment<Texture> {
    /// Cleared to the far plane, and dropped after the pass.
    pub fn cleared(texture: Texture) -> Self {
        Self {
            texture,
            load: LoadOp::Clear(1.0),
            store: StoreOp::Discard,
        }
    }
}

/// The formats a pipeline draws into, as WebGPU pipelines name their targets'
/// formats rather than a pass.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PipelineTargets<Format> {
    pub color: Format,
    /// Whether there's a `Depth32Float` target.
    pub depth: bool,
}

pub struct RenderPassDesc<Texture, Format> {
    pub color: ColorAttachment<Texture, Format>,
    pub depth: Option<DepthAttachment<Texture>>,
    pub extent: Extent,
}

impl<Texture, Format> RenderPassDesc<Texture, Format> {
    /// Drawing into a swapchain `image` over its `depth` buffer, which starts
    /// cleared; `load` clears the image or keeps what an earlier pass drew.
    pub fn swapchain(
        image: Texture,
        format: Format,
        load: LoadOp<[f32; 4]>,
        depth: Texture,
        extent: Extent,
    ) -> Self {
        Self {
            color: ColorAttachment {
                texture: image,
                format,
                load,
                store: StoreOp::Store,
            },
            depth: Some(DepthAttachment::cleared(depth)),
            extent,
        }
    }
}

#[derive(Clone, Copy)]
pub enum FilterMode {
    Nearest,
    Linear,
}

#[derive(Clone, Copy)]
pub enum AddressMode {
    ClampToEdge,
    Repeat,
}

/// One filter for magnifying and minifying, and one address mode on every
/// axis.
#[derive(Clone, Copy)]
pub struct SamplerDesc {
    pub filter: FilterMode,
    pub address_mode: AddressMode,
    pub mipmap_filter: FilterMode,
    pub lod_max_clamp: f32,
}

impl SamplerDesc {
    pub const NEAREST: Self = Self::new(FilterMode::Nearest, AddressMode::ClampToEdge);
    pub const NEAREST_REPEAT: Self = Self::new(FilterMode::Nearest, AddressMode::Repeat);
    pub const LINEAR: Self = Self::new(FilterMode::Linear, AddressMode::ClampToEdge);
    pub const LINEAR_REPEAT: Self = Self::new(FilterMode::Linear, AddressMode::Repeat);

    /// Samples the base level only.
    pub const fn new(filter: FilterMode, address_mode: AddressMode) -> Self {
        Self {
            filter,
            address_mode,
            mipmap_filter: FilterMode::Nearest,
            lod_max_clamp: 0.0,
        }
    }

    /// Blends between the levels of a texture with `mip_levels` of them.
    pub fn with_mips(self, mip_levels: u32) -> Self {
        Self {
            mipmap_filter: if mip_levels > 1 {
                FilterMode::Linear
            } else {
                FilterMode::Nearest
            },
            lod_max_clamp: mip_levels.saturating_sub(1) as f32,
            ..self
        }
    }
}

#[derive(Clone, Copy)]
pub enum BindingType {
    UniformBuffer,
    StorageBuffer,
    /// A sampled texture; its sampler is a separate [`BindingType::Sampler`].
    Texture,
    Sampler,
}

/// One binding of a bind group, numbered by its index in the layout.
#[derive(Clone, Copy)]
pub struct BindGroupLayoutEntry {
    pub ty: BindingType,
    pub visibility: ShaderStages,
}

impl BindGroupLayoutEntry {
    pub const fn uniform(visibility: ShaderStages) -> Self {
        Self {
            ty: BindingType::UniformBuffer,
            visibility,
        }
    }

    pub const fn storage(visibility: ShaderStages) -> Self {
        Self {
            ty: BindingType::StorageBuffer,
            visibility,
        }
    }

    pub const fn texture(visibility: ShaderStages) -> Self {
        Self {
            ty: BindingType::Texture,
            visibility,
        }
    }

    pub const fn sampler(visibility: ShaderStages) -> Self {
        Self {
            ty: BindingType::Sampler,
            visibility,
        }
    }
}

/// Push constants, from offset 0.
#[derive(Clone, Copy)]
pub struct PushConstants {
    pub visibility: ShaderStages,
    pub size: u32,
}

#[derive(Clone, Copy)]
pub enum VertexFormat {
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
    Sint32x3,
    Uint32,
    Sint16x2,
    Uint16,
    Unorm16,
    Unorm16x2,
    Uint8x2,
    Unorm8x4,
    Snorm8x4,
}

impl VertexFormat {
    pub const fn size(self) -> u32 {
        match self {
            Self::Uint8x2 | Self::Uint16 | Self::Unorm16 => 2,
            Self::Float32
            | Self::Uint32
            | Self::Sint16x2
            | Self::Unorm16x2
            | Self::Unorm8x4
            | Self::Snorm8x4 => 4,
            Self::Float32x2 => 8,
            Self::Float32x3 | Self::Sint32x3 => 12,
            Self::Float32x4 => 16,
        }
    }
}

#[derive(Clone, Copy)]
pub struct VertexAttribute {
    pub location: u32,
    pub format: VertexFormat,
    pub offset: u32,
}

/// Attributes laid out back to back from offset 0, at consecutive locations
/// from `first_location`.
pub const fn packed_attributes<const N: usize>(
    first_location: u32,
    formats: [VertexFormat; N],
) -> [VertexAttribute; N] {
    let mut attributes = [VertexAttribute {
        location: 0,
        format: VertexFormat::Float32,
        offset: 0,
    }; N];
    let mut offset = 0;
    let mut i = 0;
    while i < N {
        attributes[i] = VertexAttribute {
            location: first_location + i as u32,
            format: formats[i],
            offset,
        };
        offset += formats[i].size();
        i += 1;
    }
    attributes
}

/// A per-vertex buffer of `$vertex`s, its attributes packed as in
/// [`packed_attributes`] from location 0.
macro_rules! vertex_buffer {
    ($vertex:ty, [$($format:ident),+ $(,)?]) => {
        $crate::renderer::rhi::VertexBufferLayout {
            stride: size_of::<$vertex>() as u32,
            step_mode: $crate::renderer::rhi::VertexStepMode::Vertex,
            attributes: &$crate::renderer::rhi::packed_attributes(
                0,
                [$($crate::renderer::rhi::VertexFormat::$format),+],
            ),
        }
    };
}

pub(crate) use vertex_buffer;

#[derive(Clone, Copy)]
pub enum VertexStepMode {
    Vertex,
    Instance,
}

#[derive(Clone, Copy)]
pub struct VertexBufferLayout<'a> {
    pub stride: u32,
    pub step_mode: VertexStepMode,
    pub attributes: &'a [VertexAttribute],
}

#[derive(Clone, Copy)]
pub enum Topology {
    TriangleList,
    LineList,
}

#[derive(Clone, Copy)]
pub enum CullMode {
    None,
    Front,
    Back,
}

#[derive(Clone, Copy)]
pub enum FrontFace {
    Ccw,
    Cw,
}

#[derive(Clone, Copy)]
pub enum CompareFunction {
    Less,
    LessEqual,
}

#[derive(Clone, Copy)]
pub struct DepthBias {
    pub constant: i32,
    pub slope: f32,
}

#[derive(Clone, Copy)]
pub struct DepthState {
    pub write: bool,
    pub compare: CompareFunction,
    pub bias: Option<DepthBias>,
}

impl DepthState {
    pub const WRITE: Self = Self {
        write: true,
        compare: CompareFunction::LessEqual,
        bias: None,
    };
    /// Tested but not written, so the geometry never hides what's behind it.
    pub const READ_ONLY: Self = Self {
        write: false,
        ..Self::WRITE
    };
}

#[derive(Clone, Copy)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
}

/// `src * src_factor + dst * dst_factor`.
#[derive(Clone, Copy)]
pub struct BlendComponent {
    pub src: BlendFactor,
    pub dst: BlendFactor,
}

impl BlendComponent {
    pub const REPLACE: Self = Self {
        src: BlendFactor::One,
        dst: BlendFactor::Zero,
    };
    const OVER: Self = Self {
        src: BlendFactor::SrcAlpha,
        dst: BlendFactor::OneMinusSrcAlpha,
    };
    const PREMULTIPLIED_OVER: Self = Self {
        src: BlendFactor::One,
        dst: BlendFactor::OneMinusSrcAlpha,
    };
}

/// The named states match vanilla's `BlendFunction` constants.
#[derive(Clone, Copy)]
pub struct BlendState {
    pub color: BlendComponent,
    pub alpha: BlendComponent,
}

impl BlendState {
    pub const TRANSLUCENT: Self = Self {
        color: BlendComponent::OVER,
        alpha: BlendComponent::PREMULTIPLIED_OVER,
    };
    pub const TRANSLUCENT_PREMULTIPLIED_ALPHA: Self = Self {
        color: BlendComponent::PREMULTIPLIED_OVER,
        alpha: BlendComponent::PREMULTIPLIED_OVER,
    };
    pub const OVERLAY: Self = Self {
        color: BlendComponent {
            src: BlendFactor::SrcAlpha,
            dst: BlendFactor::One,
        },
        alpha: BlendComponent::REPLACE,
    };
    pub const INVERT: Self = Self {
        color: BlendComponent {
            src: BlendFactor::OneMinusDstColor,
            dst: BlendFactor::OneMinusSrcColor,
        },
        alpha: BlendComponent::REPLACE,
    };
    /// Not one of vanilla's: the color blends over, the source alpha is
    /// written as is.
    pub const OVER_REPLACE_ALPHA: Self = Self {
        color: BlendComponent::OVER,
        alpha: BlendComponent::REPLACE,
    };
}

/// A graphics pipeline drawing to one color target, with the viewport and
/// scissor set per pass. Shaders are SPIR-V with a `main` entry point.
#[derive(Clone, Copy)]
pub struct RenderPipelineDesc<'a> {
    pub label: &'a str,
    pub vertex_shader: Shader<'a>,
    pub fragment_shader: Shader<'a>,
    pub vertex_buffers: &'a [VertexBufferLayout<'a>],
    pub topology: Topology,
    pub cull_mode: CullMode,
    pub front_face: FrontFace,
    /// `None` disables the depth test.
    pub depth: Option<DepthState>,
    /// `None` writes the color unblended.
    pub blend: Option<BlendState>,
}

impl<'a> RenderPipelineDesc<'a> {
    /// Unculled triangles with no depth test or blending; callers override
    /// the rest.
    pub const fn new(
        label: &'a str,
        vertex_shader: Shader<'a>,
        fragment_shader: Shader<'a>,
        vertex_buffers: &'a [VertexBufferLayout<'a>],
    ) -> Self {
        Self {
            label,
            vertex_shader,
            fragment_shader,
            vertex_buffers,
            topology: Topology::TriangleList,
            cull_mode: CullMode::None,
            front_face: FrontFace::Ccw,
            depth: None,
            blend: None,
        }
    }
}
