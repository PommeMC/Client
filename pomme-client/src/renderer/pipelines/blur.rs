use crate::renderer::rhi::TextureUse;
use crate::renderer::rhi::backend::TextureTransition;
use crate::renderer::{rhi, shader, util};

const FORMAT: rhi::TextureFormat = rhi::TextureFormat::Rgba8UnormSrgb;

pub struct BlurPipeline {
    texture_a: rhi::backend::Texture,
    texture_b: rhi::backend::Texture,
    sampler: rhi::backend::Sampler,
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    texture_layout: rhi::backend::BindGroupLayout,
    set_read_a: rhi::backend::BindGroup,
    set_read_b: rhi::backend::BindGroup,
    width: u32,
    height: u32,
}

impl BlurPipeline {
    pub fn new(device: &rhi::backend::Device, width: u32, height: u32) -> Self {
        let blur_w = (width / 2).max(1);
        let blur_h = (height / 2).max(1);

        let texture_a = create_blur_texture(device, blur_w, blur_h, "blur_a");
        let texture_b = create_blur_texture(device, blur_w, blur_h, "blur_b");

        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR);

        let texture_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[texture_layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::FRAGMENT,
                size: 8,
            }),
        );

        let pipeline = create_blur_graphics_pipeline(
            device,
            rhi::backend::PipelineTargets {
                color: rhi::backend::texture_format(FORMAT),
                depth: false,
            },
            pipeline_layout,
        );

        let set_read_a =
            util::create_texture_group(device, texture_layout, texture_a.handle(), sampler);
        let set_read_b =
            util::create_texture_group(device, texture_layout, texture_b.handle(), sampler);

        Self {
            texture_a,
            texture_b,
            sampler,
            pipeline,
            pipeline_layout,
            texture_layout,
            set_read_a,
            set_read_b,
            width: blur_w,
            height: blur_h,
        }
    }

    pub fn blurred_texture(&self) -> rhi::backend::TextureHandle {
        self.texture_a.handle()
    }

    pub fn blurred_sampler(&self) -> rhi::backend::Sampler {
        self.sampler
    }

    pub fn execute(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        src_image: rhi::backend::TextureHandle,
        src_width: u32,
        src_height: u32,
        iterations: u32,
    ) {
        let bw = self.width;
        let bh = self.height;
        util::copy_frame(
            encoder,
            src_image,
            [src_width, src_height],
            self.texture_a.handle(),
            [bw, bh],
        );

        let h_dir: [f32; 2] = [1.0 / bw as f32, 0.0];
        let v_dir: [f32; 2] = [0.0, 1.0 / bh as f32];

        for _ in 0..iterations {
            self.blur_pass(
                device,
                encoder,
                self.texture_b.handle(),
                self.set_read_a,
                h_dir,
            );
            self.blur_pass(
                device,
                encoder,
                self.texture_a.handle(),
                self.set_read_b,
                v_dir,
            );
        }
    }

    /// Draws into `target`, leaving it for the next pass to read.
    fn blur_pass(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        target: rhi::backend::TextureHandle,
        read_set: rhi::backend::BindGroup,
        dir: [f32; 2],
    ) {
        let desc = rhi::backend::RenderPassDesc {
            color: rhi::backend::ColorAttachment {
                texture: target,
                format: rhi::backend::texture_format(FORMAT),
                load: rhi::LoadOp::Clear([0.0; 4]),
                store: rhi::StoreOp::Store,
            },
            depth: None,
            extent: rhi::Extent {
                width: self.width,
                height: self.height,
            },
        };
        let mut pass = encoder.begin_render_pass(device, &desc);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, read_set);
        pass.set_push_constants(rhi::ShaderStages::FRAGMENT, 0, bytemuck::cast_slice(&dir));
        pass.draw(3, 1, 0, 0);
        pass.end();
        encoder.transition_textures(&[TextureTransition::color(
            target,
            TextureUse::ColorAttachment,
            TextureUse::ShaderRead,
        )]);
    }

    pub fn resize(&mut self, device: &rhi::backend::Device, width: u32, height: u32) {
        let bw = (width / 2).max(1);
        let bh = (height / 2).max(1);
        if bw == self.width && bh == self.height {
            return;
        }

        device.wait_idle();

        self.destroy_images(device);

        self.texture_a = create_blur_texture(device, bw, bh, "blur_a");
        self.texture_b = create_blur_texture(device, bw, bh, "blur_b");

        self.set_read_a.replace(
            device,
            self.texture_layout,
            &util::texture_binding(self.texture_a.handle(), self.sampler),
        );
        self.set_read_b.replace(
            device,
            self.texture_layout,
            &util::texture_binding(self.texture_b.handle(), self.sampler),
        );

        self.width = bw;
        self.height = bh;
    }

    fn destroy_images(&mut self, device: &rhi::backend::Device) {
        std::mem::take(&mut self.texture_a).destroy(device);
        std::mem::take(&mut self.texture_b).destroy(device);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.destroy_images(device);
        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        self.set_read_a.destroy(device);
        self.set_read_b.destroy(device);
        self.texture_layout.destroy(device);
        self.sampler.destroy(device);
    }
}

/// A blur target, ready to be sampled.
fn create_blur_texture(
    device: &rhi::backend::Device,
    w: u32,
    h: u32,
    name: &str,
) -> rhi::backend::Texture {
    let texture = rhi::backend::Texture::new(
        device,
        &rhi::TextureDesc::new(
            name,
            w,
            h,
            FORMAT,
            rhi::TextureUsages::RENDER_ATTACHMENT
                | rhi::TextureUsages::TEXTURE_BINDING
                | rhi::TextureUsages::COPY_DST,
        ),
    );
    util::transition_image_to_shader_read(device, texture.handle());
    texture
}

fn create_blur_graphics_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc::new(
            "blur",
            shader::include_shader!("blur.vert"),
            shader::include_shader!("blur.frag"),
            &[],
        ),
    )
}
