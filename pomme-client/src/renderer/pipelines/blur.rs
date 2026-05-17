use std::slice;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::MemoryLocation;
use pomme_gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use pyronyx::vk;

use crate::renderer::shader;

pub struct BlurPipeline {
    image_a: vk::Image,
    view_a: vk::ImageView,
    alloc_a: Option<Allocation>,
    image_b: vk::Image,
    view_b: vk::ImageView,
    alloc_b: Option<Allocation>,
    sampler: vk::Sampler,
    render_pass: vk::RenderPass,
    fb_a: vk::Framebuffer,
    fb_b: vk::Framebuffer,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    desc_layout: vk::DescriptorSetLayout,
    desc_pool: vk::DescriptorPool,
    set_read_a: vk::DescriptorSet,
    set_read_b: vk::DescriptorSet,
    width: u32,
    height: u32,
    format: vk::Format,
}

impl BlurPipeline {
    pub fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Self {
        let blur_w = (width / 2).max(1);
        let blur_h = (height / 2).max(1);

        let (image_a, view_a, alloc_a) =
            create_blur_image(device, allocator, blur_w, blur_h, format, "blur_a");
        let (image_b, view_b, alloc_b) =
            create_blur_image(device, allocator, blur_w, blur_h, format, "blur_b");

        let sampler = device
            .create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::Linear,
                    min_filter: vk::Filter::Linear,
                    address_mode_u: vk::SamplerAddressMode::ClampToEdge,
                    address_mode_v: vk::SamplerAddressMode::ClampToEdge,
                    address_mode_w: vk::SamplerAddressMode::ClampToEdge,
                    ..Default::default()
                },
                None,
            )
            .expect("failed to create blur sampler");

        let render_pass = create_blur_render_pass(device, format);
        let fb_a = create_blur_framebuffer(device, render_pass, view_a, blur_w, blur_h);
        let fb_b = create_blur_framebuffer(device, render_pass, view_b, blur_w, blur_h);

        let desc_layout = {
            let binding = vk::DescriptorSetLayoutBinding {
                binding: 0,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            };
            let info = vk::DescriptorSetLayoutCreateInfo {
                binding_count: 1,
                bindings: &binding,
                ..Default::default()
            };
            device
                .create_descriptor_set_layout(&info, None)
                .expect("failed to create blur desc layout")
        };

        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Fragment,
            offset: 0,
            size: 8,
        };
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &desc_layout,
            push_constant_range_count: 1,
            push_constant_ranges: &push_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create blur pipeline layout");

        let pipeline = create_blur_graphics_pipeline(device, render_pass, pipeline_layout);

        let pool_size = vk::DescriptorPoolSize {
            ty: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 2,
        };
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: 2,
            pool_size_count: 1,
            pool_sizes: &pool_size,
            ..Default::default()
        };
        let desc_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create blur desc pool");

        let alloc_layouts = [desc_layout, desc_layout];
        let alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: desc_pool,
            descriptor_set_count: alloc_layouts.len() as u32,
            set_layouts: alloc_layouts.as_ptr(),
            ..Default::default()
        };
        let mut sets = [vk::DescriptorSet::null(), vk::DescriptorSet::null()];
        device
            .allocate_descriptor_sets(&alloc_info, &mut sets)
            .expect("failed to allocate blur desc sets");
        let set_read_a = sets[0];
        let set_read_b = sets[1];

        write_blur_descriptor(device, set_read_a, view_a, sampler);
        write_blur_descriptor(device, set_read_b, view_b, sampler);

        Self {
            image_a,
            view_a,
            alloc_a: Some(alloc_a),
            image_b,
            view_b,
            alloc_b: Some(alloc_b),
            sampler,
            render_pass,
            fb_a,
            fb_b,
            pipeline,
            pipeline_layout,
            desc_layout,
            desc_pool,
            set_read_a,
            set_read_b,
            width: blur_w,
            height: blur_h,
            format,
        }
    }

    pub fn blurred_view(&self) -> vk::ImageView {
        self.view_a
    }

    pub fn blurred_sampler(&self) -> vk::Sampler {
        self.sampler
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        cmd: vk::CommandBuffer,
        src_image: vk::Image,
        src_width: u32,
        src_height: u32,
        iterations: u32,
    ) {
        let bw = self.width;
        let bh = self.height;

        let barrier_src = vk::ImageMemoryBarrier {
            image: src_image,
            old_layout: vk::ImageLayout::ColorAttachmentOptimal,
            new_layout: vk::ImageLayout::TransferSrcOptimal,
            src_access_mask: vk::AccessFlags::ColorAttachmentWrite,
            dst_access_mask: vk::AccessFlags::TransferRead,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        let barrier_dst_a = vk::ImageMemoryBarrier {
            image: self.image_a,
            old_layout: vk::ImageLayout::Undefined,
            new_layout: vk::ImageLayout::TransferDstOptimal,
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::TransferWrite,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        let barrier_dst_b = vk::ImageMemoryBarrier {
            image: self.image_b,
            old_layout: vk::ImageLayout::Undefined,
            new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::ShaderRead,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ColorAttachmentOutput | vk::PipelineStageFlags::TopOfPipe,
            vk::PipelineStageFlags::Transfer | vk::PipelineStageFlags::FragmentShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier_src, barrier_dst_a, barrier_dst_b],
        );

        let blit = vk::ImageBlit {
            src_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::Color,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            src_offsets: [
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: src_width as i32,
                    y: src_height as i32,
                    z: 1,
                },
            ],
            dst_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::Color,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            dst_offsets: [
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: bw as i32,
                    y: bh as i32,
                    z: 1,
                },
            ],
        };
        cmd.blit_image(
            src_image,
            vk::ImageLayout::TransferSrcOptimal,
            self.image_a,
            vk::ImageLayout::TransferDstOptimal,
            &[blit],
            vk::Filter::Linear,
        );

        let barrier_a_read = vk::ImageMemoryBarrier {
            image: self.image_a,
            old_layout: vk::ImageLayout::TransferDstOptimal,
            new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        let barrier_src_back = vk::ImageMemoryBarrier {
            image: src_image,
            old_layout: vk::ImageLayout::TransferSrcOptimal,
            new_layout: vk::ImageLayout::ColorAttachmentOptimal,
            src_access_mask: vk::AccessFlags::TransferRead,
            dst_access_mask: vk::AccessFlags::ColorAttachmentWrite,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::FragmentShader | vk::PipelineStageFlags::ColorAttachmentOutput,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier_a_read, barrier_src_back],
        );

        let h_dir: [f32; 2] = [1.0 / bw as f32, 0.0];
        let v_dir: [f32; 2] = [0.0, 1.0 / bh as f32];

        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: bw as f32,
            height: bh as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: bw,
                height: bh,
            },
        };

        for _ in 0..iterations {
            // Horizontal: read A → write B
            cmd.begin_render_pass(
                &vk::RenderPassBeginInfo {
                    render_pass: self.render_pass,
                    framebuffer: self.fb_b,
                    render_area: scissor,
                    ..Default::default()
                },
                vk::SubpassContents::Inline,
            );
            cmd.set_viewport(0, &[viewport]);
            cmd.set_scissor(0, &[scissor]);
            cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Graphics,
                self.pipeline_layout,
                0,
                &[self.set_read_a],
                &[],
            );
            cmd.push_constants(
                self.pipeline_layout,
                vk::ShaderStageFlags::Fragment,
                0,
                bytemuck::cast_slice(&h_dir),
            );
            cmd.draw(3, 1, 0, 0);
            cmd.end_render_pass();

            // Vertical: read B → write A
            cmd.begin_render_pass(
                &vk::RenderPassBeginInfo {
                    render_pass: self.render_pass,
                    framebuffer: self.fb_a,
                    render_area: scissor,
                    ..Default::default()
                },
                vk::SubpassContents::Inline,
            );
            cmd.set_viewport(0, &[viewport]);
            cmd.set_scissor(0, &[scissor]);
            cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Graphics,
                self.pipeline_layout,
                0,
                &[self.set_read_b],
                &[],
            );
            cmd.push_constants(
                self.pipeline_layout,
                vk::ShaderStageFlags::Fragment,
                0,
                bytemuck::cast_slice(&v_dir),
            );
            cmd.draw(3, 1, 0, 0);
            cmd.end_render_pass();
        }
    }

    pub fn resize(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        width: u32,
        height: u32,
    ) {
        let bw = (width / 2).max(1);
        let bh = (height / 2).max(1);
        if bw == self.width && bh == self.height {
            return;
        }

        let _ = device.wait_idle();

        self.destroy_images(device, allocator);

        let (ia, va, aa) = create_blur_image(device, allocator, bw, bh, self.format, "blur_a");
        let (ib, vb, ab) = create_blur_image(device, allocator, bw, bh, self.format, "blur_b");

        self.image_a = ia;
        self.view_a = va;
        self.alloc_a = Some(aa);
        self.image_b = ib;
        self.view_b = vb;
        self.alloc_b = Some(ab);

        device.destroy_framebuffer(self.fb_a, None);
        device.destroy_framebuffer(self.fb_b, None);
        self.fb_a = create_blur_framebuffer(device, self.render_pass, va, bw, bh);
        self.fb_b = create_blur_framebuffer(device, self.render_pass, vb, bw, bh);

        write_blur_descriptor(device, self.set_read_a, va, self.sampler);
        write_blur_descriptor(device, self.set_read_b, vb, self.sampler);

        self.width = bw;
        self.height = bh;
    }

    fn destroy_images(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        device.destroy_image_view(self.view_a, None);
        device.destroy_image_view(self.view_b, None);
        device.destroy_image(self.image_a, None);
        device.destroy_image(self.image_b, None);
        if let Some(a) = self.alloc_a.take() {
            alloc.free(a).ok();
        }
        if let Some(a) = self.alloc_b.take() {
            alloc.free(a).ok();
        }
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.destroy_images(device, allocator);
        device.destroy_framebuffer(self.fb_a, None);
        device.destroy_framebuffer(self.fb_b, None);
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.desc_pool, None);
        device.destroy_descriptor_set_layout(self.desc_layout, None);
        device.destroy_render_pass(self.render_pass, None);
        device.destroy_sampler(self.sampler, None);
    }
}

fn create_blur_image(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    w: u32,
    h: u32,
    format: vk::Format,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation) {
    let info = vk::ImageCreateInfo {
        image_type: vk::ImageType::Type2D,
        format,
        extent: vk::Extent3D {
            width: w,
            height: h,
            depth: 1,
        },
        mip_levels: 1,
        array_layers: 1,
        samples: vk::SampleCountFlags::Type1,
        tiling: vk::ImageTiling::Optimal,
        usage: vk::ImageUsageFlags::ColorAttachment
            | vk::ImageUsageFlags::Sampled
            | vk::ImageUsageFlags::TransferDst,
        ..Default::default()
    };

    let image = device
        .create_image(&info, None)
        .expect("failed to create blur image");
    let reqs = device.get_image_memory_requirements(image);

    let alloc = allocator
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name,
            requirements: reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .expect("failed to allocate blur image");

    unsafe {
        device
            .bind_image_memory(image, alloc.memory(), alloc.offset())
            .expect("failed to bind blur image");
    }

    let view_info = vk::ImageViewCreateInfo {
        image,
        view_type: vk::ImageViewType::Type2D,
        format,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        },
        ..Default::default()
    };
    let view = device
        .create_image_view(&view_info, None)
        .expect("failed to create blur view");

    (image, view, alloc)
}

fn create_blur_render_pass(device: &vk::Device, format: vk::Format) -> vk::RenderPass {
    let attachment = vk::AttachmentDescription {
        format,
        samples: vk::SampleCountFlags::Type1,
        load_op: vk::AttachmentLoadOp::DontCare,
        store_op: vk::AttachmentStoreOp::Store,
        initial_layout: vk::ImageLayout::Undefined,
        final_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        ..Default::default()
    };

    let color_ref = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::ColorAttachmentOptimal,
    };

    let subpass = vk::SubpassDescription {
        pipeline_bind_point: vk::PipelineBindPoint::Graphics,
        color_attachments: &color_ref,
        color_attachment_count: 1,
        ..Default::default()
    };

    let dependency = vk::SubpassDependency {
        src_subpass: vk::SUBPASS_EXTERNAL,
        dst_subpass: 0,
        src_stage_mask: vk::PipelineStageFlags::FragmentShader,
        src_access_mask: vk::AccessFlags::ShaderRead,
        dst_stage_mask: vk::PipelineStageFlags::ColorAttachmentOutput,
        dst_access_mask: vk::AccessFlags::ColorAttachmentWrite,
        ..Default::default()
    };

    let info = vk::RenderPassCreateInfo {
        attachment_count: 1,
        attachments: &attachment,
        subpass_count: 1,
        subpasses: &subpass,
        dependency_count: 1,
        dependencies: &dependency,
        ..Default::default()
    };

    device
        .create_render_pass(&info, None)
        .expect("failed to create blur render pass")
}

fn create_blur_framebuffer(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    view: vk::ImageView,
    w: u32,
    h: u32,
) -> vk::Framebuffer {
    let info = vk::FramebufferCreateInfo {
        render_pass,
        attachments: &view,
        attachment_count: 1,
        width: w,
        height: h,
        layers: 1,
        ..Default::default()
    };
    device
        .create_framebuffer(&info, None)
        .expect("failed to create blur framebuffer")
}

fn create_blur_graphics_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("blur.vert.spv");
    let frag_spv = shader::include_spirv!("blur.frag.spv");
    let vert_mod = shader::create_shader_module(device, vert_spv);
    let frag_mod = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let info = [vk::GraphicsPipelineCreateInfo {
        stages: stages.as_ptr(),
        stage_count: 2,
        vertex_input_state: &vk::PipelineVertexInputStateCreateInfo::default(),
        input_assembly_state: &vk::PipelineInputAssemblyStateCreateInfo {
            topology: vk::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        viewport_state: &vk::PipelineViewportStateCreateInfo {
            viewport_count: 1,
            scissor_count: 1,
            ..Default::default()
        },
        rasterization_state: &vk::PipelineRasterizationStateCreateInfo {
            polygon_mode: vk::PolygonMode::Fill,
            cull_mode: vk::CullModeFlags::None,
            line_width: 1.0,
            ..Default::default()
        },
        multisample_state: &vk::PipelineMultisampleStateCreateInfo {
            rasterization_samples: vk::SampleCountFlags::Type1,
            ..Default::default()
        },
        color_blend_state: &vk::PipelineColorBlendStateCreateInfo {
            attachment_count: 1,
            attachments: &vk::PipelineColorBlendAttachmentState {
                color_write_mask: vk::ColorComponentFlags::RGBA,
                ..Default::default()
            },
            ..Default::default()
        },
        dynamic_state: &vk::PipelineDynamicStateCreateInfo {
            dynamic_states: [vk::DynamicState::Viewport, vk::DynamicState::Scissor].as_ptr(),
            dynamic_state_count: 2,
            ..Default::default()
        },
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = vk::Pipeline::null();
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create blur pipeline");

    device.destroy_shader_module(vert_mod, None);
    device.destroy_shader_module(frag_mod, None);

    pipeline
}

fn write_blur_descriptor(
    device: &vk::Device,
    set: vk::DescriptorSet,
    view: vk::ImageView,
    sampler: vk::Sampler,
) {
    let image_info = vk::DescriptorImageInfo {
        sampler,
        image_view: view,
        image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
    };
    let write = vk::WriteDescriptorSet {
        dst_set: set,
        dst_binding: 0,
        descriptor_type: vk::DescriptorType::CombinedImageSampler,
        descriptor_count: 1,
        image_info: &image_info,
        ..Default::default()
    };
    device.update_descriptor_sets(&[write], &[]);
}
