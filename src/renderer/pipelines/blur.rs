use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::renderer::shader;

pub struct BlurPipeline {
    image_a: vk::Image,
    view_a: vk::ImageView,
    alloc_a: Option<Allocation>,
    image_b: vk::Image,
    view_b: vk::ImageView,
    alloc_b: Option<Allocation>,
    sampler: vk::Sampler,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    desc_layout: vk::DescriptorSetLayout,
    width: u32,
    height: u32,
    format: vk::Format,
}

impl BlurPipeline {
    pub fn new(
        device: &ash::Device,
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

        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .expect("failed to create blur sampler")
        };

        let desc_layout = {
            let bindings = [vk::DescriptorSetLayoutBinding {
                binding: 0,
                descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::FRAGMENT,
                ..Default::default()
            }];
            let info = vk::DescriptorSetLayoutCreateInfo::default()
                .bindings(&bindings)
                .flags(vk::DescriptorSetLayoutCreateFlags::PUSH_DESCRIPTOR_KHR);
            unsafe { device.create_descriptor_set_layout(&info, None) }
                .expect("failed to create blur desc layout")
        };

        let push_range = [vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::FRAGMENT,
            offset: 0,
            size: 8,
        }];
        let layouts = [desc_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&push_range);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create blur pipeline layout");

        let pipeline = create_blur_graphics_pipeline(device, format, pipeline_layout);

        Self {
            image_a,
            view_a,
            alloc_a: Some(alloc_a),
            image_b,
            view_b,
            alloc_b: Some(alloc_b),
            sampler,
            pipeline,
            pipeline_layout,
            desc_layout,
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
        device: &ash::Device,
        push_desc: &ash::khr::push_descriptor::Device,
        cmd: vk::CommandBuffer,
        src_image: vk::Image,
        src_width: u32,
        src_height: u32,
        iterations: u32,
    ) {
        let bw = self.width;
        let bh = self.height;

        unsafe {
            let barrier_src = vk::ImageMemoryBarrier::default()
                .image(src_image)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            let barrier_dst = vk::ImageMemoryBarrier::default()
                .image(self.image_a)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier_src, barrier_dst],
            );

            let blit = vk::ImageBlit {
                src_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
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
                    aspect_mask: vk::ImageAspectFlags::COLOR,
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
            device.cmd_blit_image(
                cmd,
                src_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.image_a,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[blit],
                vk::Filter::LINEAR,
            );

            let barrier_a_read = vk::ImageMemoryBarrier::default()
                .image(self.image_a)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            let barrier_src_back = vk::ImageMemoryBarrier::default()
                .image(src_image)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER
                    | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
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

            let color_range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };

            for _ in 0..iterations {
                // Horizontal: read A -> write B
                // Transition B to COLOR_ATTACHMENT_OPTIMAL
                let barrier_b_write = vk::ImageMemoryBarrier::default()
                    .image(self.image_b)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::SHADER_READ)
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .subresource_range(color_range);
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier_b_write],
                );

                let color_attachment_b = vk::RenderingAttachmentInfo::default()
                    .image_view(self.view_b)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .store_op(vk::AttachmentStoreOp::STORE);
                let rendering_info = vk::RenderingInfo::default()
                    .render_area(scissor)
                    .layer_count(1)
                    .color_attachments(std::slice::from_ref(&color_attachment_b));
                device.cmd_begin_rendering(cmd, &rendering_info);
                device.cmd_set_viewport(cmd, 0, &[viewport]);
                device.cmd_set_scissor(cmd, 0, &[scissor]);
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
                let a_img_info = [vk::DescriptorImageInfo {
                    sampler: self.sampler,
                    image_view: self.view_a,
                    image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                }];
                let a_write = vk::WriteDescriptorSet::default()
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&a_img_info);
                push_desc.cmd_push_descriptor_set(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[a_write],
                );
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytemuck::cast_slice(&h_dir),
                );
                device.cmd_draw(cmd, 3, 1, 0, 0);
                device.cmd_end_rendering(cmd);

                // Transition B to SHADER_READ_ONLY, A to COLOR_ATTACHMENT
                let barrier_b_read = vk::ImageMemoryBarrier::default()
                    .image(self.image_b)
                    .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .subresource_range(color_range);
                let barrier_a_write = vk::ImageMemoryBarrier::default()
                    .image(self.image_a)
                    .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::SHADER_READ)
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .subresource_range(color_range);
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER
                        | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier_b_read, barrier_a_write],
                );

                // Vertical: read B -> write A
                let color_attachment_a = vk::RenderingAttachmentInfo::default()
                    .image_view(self.view_a)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .store_op(vk::AttachmentStoreOp::STORE);
                let rendering_info = vk::RenderingInfo::default()
                    .render_area(scissor)
                    .layer_count(1)
                    .color_attachments(std::slice::from_ref(&color_attachment_a));
                device.cmd_begin_rendering(cmd, &rendering_info);
                device.cmd_set_viewport(cmd, 0, &[viewport]);
                device.cmd_set_scissor(cmd, 0, &[scissor]);
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
                let b_img_info = [vk::DescriptorImageInfo {
                    sampler: self.sampler,
                    image_view: self.view_b,
                    image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                }];
                let b_write = vk::WriteDescriptorSet::default()
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&b_img_info);
                push_desc.cmd_push_descriptor_set(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[b_write],
                );
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytemuck::cast_slice(&v_dir),
                );
                device.cmd_draw(cmd, 3, 1, 0, 0);
                device.cmd_end_rendering(cmd);

                // Transition A back to SHADER_READ_ONLY for next iteration / sampling
                let barrier_a_read = vk::ImageMemoryBarrier::default()
                    .image(self.image_a)
                    .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .subresource_range(color_range);
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier_a_read],
                );
            }
        }
    }

    pub fn resize(
        &mut self,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        width: u32,
        height: u32,
    ) {
        let bw = (width / 2).max(1);
        let bh = (height / 2).max(1);
        if bw == self.width && bh == self.height {
            return;
        }

        unsafe {
            let _ = device.device_wait_idle();
        }

        self.destroy_images(device, allocator);

        let (ia, va, aa) = create_blur_image(device, allocator, bw, bh, self.format, "blur_a");
        let (ib, vb, ab) = create_blur_image(device, allocator, bw, bh, self.format, "blur_b");

        self.image_a = ia;
        self.view_a = va;
        self.alloc_a = Some(aa);
        self.image_b = ib;
        self.view_b = vb;
        self.alloc_b = Some(ab);

        self.width = bw;
        self.height = bh;
    }

    fn destroy_images(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        unsafe {
            device.destroy_image_view(self.view_a, None);
            device.destroy_image_view(self.view_b, None);
            device.destroy_image(self.image_a, None);
            device.destroy_image(self.image_b, None);
        }
        if let Some(a) = self.alloc_a.take() {
            alloc.free(a).ok();
        }
        if let Some(a) = self.alloc_b.take() {
            alloc.free(a).ok();
        }
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        self.destroy_images(device, allocator);
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_set_layout(self.desc_layout, None);
            device.destroy_sampler(self.sampler, None);
        }
    }
}

fn create_blur_image(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    w: u32,
    h: u32,
    format: vk::Format,
    name: &str,
) -> (vk::Image, vk::ImageView, Allocation) {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width: w,
            height: h,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(
            vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_DST,
        );

    let image = unsafe { device.create_image(&info, None) }.expect("failed to create blur image");
    let reqs = unsafe { device.get_image_memory_requirements(image) };

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

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    let view =
        unsafe { device.create_image_view(&view_info, None) }.expect("failed to create blur view");

    (image, view, alloc)
}

fn create_blur_graphics_pipeline(
    device: &ash::Device,
    color_format: vk::Format,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("blur.vert.spv");
    let frag_spv = shader::include_spirv!("blur.frag.spv");
    let vert_mod = shader::create_shader_module(device, vert_spv);
    let frag_mod = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_mod)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_mod)
            .name(c"main"),
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .line_width(1.0);
    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
    }];
    let color_blending =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state =
        vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [color_format];
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(vk::Format::UNDEFINED);

    let info = [vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisampling)
        .color_blend_state(&color_blending)
        .dynamic_state(&dynamic_state)
        .layout(layout)
        .push_next(&mut rendering_info)];

    let pipeline =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &info, None) }
            .expect("failed to create blur pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_mod, None);
        device.destroy_shader_module(frag_mod, None);
    }

    pipeline
}
