use std::slice;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pomme_gui::graphics::state::{DrawCmd, GuiRenderState, RectCmd};
use pyronyx::vk;

use crate::renderer::{shader, util};

mod vertex_mode {
    #[repr(transparent)]
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct VertexMode(f32);
    impl VertexMode {
        pub const RECT: Self = Self(0.0);
    }
}
use vertex_mode::VertexMode;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    color: [f32; 4],
    mode: VertexMode,
    rect_size: [f32; 2],
}

const MAX_VERTICES: usize = 8192;
const VERTEX_SIZE: usize = size_of::<Vertex>();

pub struct GuiRenderer {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,

    globals_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    globals_set: vk::DescriptorSet,
    globals_buffer: vk::Buffer,
    globals_allocation: Option<Allocation>,

    vertex_buffer: vk::Buffer,
    vertex_allocation: Option<Allocation>,
}

impl GuiRenderer {
    pub fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        render_pass: vk::RenderPass,
    ) -> Self {
        let globals_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex,
        );

        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &globals_layout,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create gui pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
        }];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: 1,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create gui descriptor pool");

        let alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &globals_layout,
            ..Default::default()
        };
        let mut globals_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&alloc_info, slice::from_mut(&mut globals_set))
            .expect("failed to allocate gui globals descriptor set");

        let (globals_buffer, globals_allocation) =
            util::create_uniform_buffer(device, allocator, 8, "gui_globals");

        let buf_info = vk::DescriptorBufferInfo {
            buffer: globals_buffer,
            offset: 0,
            range: 8,
        };
        let write = vk::WriteDescriptorSet {
            dst_set: globals_set,
            dst_binding: 0,
            descriptor_count: 1,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            buffer_info: &buf_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[write], &[]);

        let (vertex_buffer, vertex_allocation) = util::create_host_buffer(
            device,
            allocator,
            (MAX_VERTICES * VERTEX_SIZE) as u64,
            vk::BufferUsageFlags::VertexBuffer,
            "gui_vertices",
        );

        Self {
            pipeline,
            pipeline_layout,
            globals_layout,
            descriptor_pool,
            globals_set,
            globals_buffer,
            globals_allocation: Some(globals_allocation),
            vertex_buffer,
            vertex_allocation: Some(vertex_allocation),
        }
    }

    /// TODO: once the blur pass exists, `before_blur_cmds` should render into
    /// the scene target before the blur runs, and `after_blur_cmds` on top,
    /// as two separate calls with the blur pass in between. For now both are
    /// concatenated and drawn in one pass, in submission order.
    pub fn draw(
        &mut self,
        cmd: vk::CommandBuffer,
        screen_w: f32,
        screen_h: f32,
        gui_render_state: &mut GuiRenderState,
    ) {
        let globals: [f32; 2] = [screen_w, screen_h];
        self.globals_allocation
            .as_mut()
            .unwrap()
            .mapped_slice_mut()
            .unwrap()[..8]
            .copy_from_slice(bytemuck::cast_slice(&globals));

        let total =
            gui_render_state.before_blur_cmds.len() + gui_render_state.after_blur_cmds.len();
        let mut vertices: Vec<Vertex> = Vec::with_capacity(total * 6);

        for draw_cmd in gui_render_state
            .before_blur_cmds
            .iter()
            .chain(gui_render_state.after_blur_cmds.iter())
        {
            match draw_cmd {
                DrawCmd::Rect(rect) => push_rect(&mut vertices, rect),
            }
        }

        if vertices.is_empty() {
            return;
        }

        let count = vertices.len().min(MAX_VERTICES);
        if count < vertices.len() {
            tracing::warn!(
                "GUI vertex buffer overflow, dropping {} vertices",
                vertices.len() - count
            );
        }
        let byte_data = bytemuck::cast_slice(&vertices[..count]);
        self.vertex_allocation
            .as_mut()
            .unwrap()
            .mapped_slice_mut()
            .unwrap()[..byte_data.len()]
            .copy_from_slice(byte_data);

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.globals_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        cmd.draw(count as u32, 1, 0, 0);
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();

        device.destroy_buffer(self.globals_buffer, None);
        if let Some(a) = self.globals_allocation.take() {
            alloc.free(a).ok();
        }
        device.destroy_buffer(self.vertex_buffer, None);
        if let Some(a) = self.vertex_allocation.take() {
            alloc.free(a).ok();
        }

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.globals_layout, None);
    }
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("gui_rect.vert.spv");
    let frag_spv = shader::include_spirv!("gui_rect.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: VERTEX_SIZE as u32,
        input_rate: vk::VertexInputRate::Vertex,
    }];

    let attr_descs = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32B32A32Sfloat,
            offset: 8,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R32Sfloat,
            offset: 24,
        },
        vk::VertexInputAttributeDescription {
            location: 3,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 28,
        },
        vk::VertexInputAttributeDescription {
            location: 4,
            binding: 0,
            format: vk::Format::R32Sfloat,
            offset: 36,
        },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: binding_descs.len() as u32,
        vertex_binding_descriptions: binding_descs.as_ptr(),
        vertex_attribute_description_count: attr_descs.len() as u32,
        vertex_attribute_descriptions: attr_descs.as_ptr(),
        ..Default::default()
    };

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        ..Default::default()
    };

    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };

    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: vk::CullModeFlags::None,
        line_width: 1.0,
        ..Default::default()
    };

    let multisampling = vk::PipelineMultisampleStateCreateInfo {
        rasterization_samples: vk::SampleCountFlags::Type1,
        ..Default::default()
    };

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::FALSE,
        depth_write_enable: vk::FALSE,
        ..Default::default()
    };

    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::One,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    }];

    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: blend_attachment.len() as u32,
        attachments: blend_attachment.as_ptr(),
        ..Default::default()
    };

    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let pipeline_info = [vk::GraphicsPipelineCreateInfo {
        stage_count: stages.len() as u32,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = vk::Pipeline::null();
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &pipeline_info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create gui rect pipeline");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    pipeline
}

fn push_rect(verts: &mut Vec<Vertex>, rect: &RectCmd) {
    let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
    let positions = [
        [x, y],
        [x + w, y],
        [x, y + h],
        [x + w, y],
        [x + w, y + h],
        [x, y + h],
    ];
    for pos in positions {
        verts.push(Vertex {
            pos,
            color: rect.color,
            mode: VertexMode::RECT,
            rect_size: [w, h],
        });
    }
}
