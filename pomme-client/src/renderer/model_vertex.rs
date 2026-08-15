use pyronyx::vk;

/// Shared expanded vertex format for entity, block-entity, and item meshes.
/// Chunk terrain uses packed face records and does not upload this format.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelVertex {
    pub position: [f32; 3],
    pub tex_coords: [u16; 2],
    pub light_tint: u32,
}

impl ModelVertex {
    pub const STRIDE: u32 = size_of::<Self>() as u32;

    pub fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: Self::STRIDE,
            input_rate: vk::VertexInputRate::Vertex,
        }
    }

    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 3] {
        [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32Sfloat,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R16G16Unorm,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R8G8B8A8Unorm,
                offset: 16,
            },
        ]
    }
}
