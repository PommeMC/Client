pub mod block_entity_model;
pub mod boot;
pub mod camera;
pub mod chunk;
pub mod entity_model;
pub(crate) mod fog;
pub(crate) mod packing;
pub mod pipelines;
pub(crate) mod rhi;
mod screenshot;
pub(crate) mod shader;
pub(crate) mod util;

pub(crate) const MAX_FRAMES_IN_FLIGHT: usize = 3;
/// iOS keeps a backgrounded app's view and Metal layer, so its surface
/// outlives a suspend; Android takes the native window away.
const KEEPS_SURFACE_WHEN_SUSPENDED: bool = cfg!(target_os = "ios");

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use azalea_block::BlockState;
pub use camera::CloudMode;
use camera::{Camera, CameraUniform};
use chunk::atlas::{AtlasUVMap, TextureAtlas};
use chunk::buffer::ChunkBufferStore;
use chunk::mesh_source::MeshSource;
use chunk::mesher::ChunkMeshData;
use chunk::moving_blocks::{MovingBlockBuffers, MovingBlockRender};
use glam::dvec3;
use pipelines::block_entity::BlockEntityPipeline;
pub use pipelines::block_entity::BlockEntityRenderInfo;
use pipelines::block_overlay::BlockOverlayPipeline;
use pipelines::book_preview::BookPreviewPipeline;
use pipelines::chunk::ChunkPipeline;
use pipelines::clouds::CloudPipeline;
use pipelines::entity_outline::EntityOutline;
use pipelines::entity_renderer::{ArmorRenderInfo, EntityRenderInfo, EntityRenderer};
use pipelines::entity_shadow::{EntityShadowPipeline, ShadowScene};
use pipelines::hand::HandPipeline;
use pipelines::lightmap::{Lightmap, LightmapInfo};
use pipelines::menu_overlay::{MenuElement, MenuOverlayPipeline, SpriteId};
use pipelines::panorama::PanoramaPipeline;
pub use pipelines::particle::{ParticlePipeline, ParticleQuad};
use pipelines::post_effect::{PostEffect, PostEffects};
use pipelines::skin_preview::SkinPreviewPipeline;
pub use pipelines::sky::{SkyPipeline, SkyState};
pub use pipelines::weather::{WeatherColumn, WeatherPipeline};
use thiserror::Error;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::app::input::InputState;
use crate::assets::AssetIndex;
use crate::entity::components::{LookDirection, Position};
use crate::position::{BlockPos, ChunkPos};
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::renderer::pipelines::chunk_borders::ChunkBorderPipeline;
use crate::renderer::pipelines::held_item::{HandRenderSelection, HeldHand};
use crate::renderer::pipelines::item_entity::ItemEntityPipeline;
use crate::ui::font::{FontOptions, FontSources};
use crate::world::block::render_assets::{BlockRenderAssets, BlockRenderView};

/// The live atlas's layout, so meshes bake in UVs that match it.
impl MeshSource for Renderer {
    fn shared_render_assets(&self) -> Arc<BlockRenderAssets> {
        Arc::clone(&self.render_assets)
    }

    fn uv_map(&self) -> &AtlasUVMap {
        &self.atlas.uv_map
    }

    fn jar_assets_dir(&self) -> &Path {
        &self.jar_assets_dir
    }

    fn asset_index(&self) -> &Option<AssetIndex> {
        &self.asset_index
    }
}

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("failed to initialize GPU context: {0}")]
    Context(#[from] rhi::backend::ContextError),

    #[error("GPU error: {0}")]
    Gpu(#[from] rhi::backend::Error),

    #[error("failed to initialize Minecraft fonts: {0}")]
    Font(String),

    #[error("failed to build the texture atlas: {0}")]
    Atlas(String),
}

/// The loading splash: logo, progress bar and status line.
pub(crate) fn splash_elements(sw: f32, sh: f32, progress: f32, status: &str) -> Vec<MenuElement> {
    let gs = (sh / 400.0).max(1.0);
    let logo_size = 48.0 * gs;
    let status_size = 8.0 * gs;
    let bar_w = 200.0 * gs;
    let bar_h = 6.0 * gs;
    let bar_border = 1.0 * gs;
    let cx = sw / 2.0;
    let cy = sh / 2.0;

    vec![
        MenuElement::Image {
            x: cx - logo_size / 2.0,
            y: cy - logo_size - 16.0 * gs,
            w: logo_size,
            h: logo_size,
            sprite: SpriteId::PommeLogo,
            tint: [1.0; 4],
        },
        MenuElement::Rect {
            x: cx - bar_w / 2.0 - bar_border,
            y: cy - bar_border,
            w: bar_w + bar_border * 2.0,
            h: bar_h + bar_border * 2.0,
            corner_radius: (bar_h / 2.0 + bar_border),
            color: [0.3, 0.3, 0.3, 0.8],
        },
        MenuElement::Rect {
            x: cx - bar_w / 2.0,
            y: cy,
            w: bar_w * progress,
            h: bar_h,
            corner_radius: bar_h / 2.0,
            color: [0.39, 0.71, 1.0, 1.0],
        },
        MenuElement::Text {
            x: cx,
            y: cy + bar_h + 8.0 * gs,
            text: status.into(),
            scale: status_size,
            color: [0.6, 0.6, 0.6, 0.8],
            centered: true,
        },
    ]
}

#[derive(Clone)]
pub struct PlayerPreview {
    pub rect: [f32; 4],
    pub gui_scale: f32,
    /// Vanilla inventory entity-render size (30 normal inventory, 20 creative
    /// inventory tab).
    pub model_scale: f32,
    pub cursor: (f32, f32),
    pub armor: [Option<ArmorRenderInfo>; 4],
}

/// The enchanting table's 3D book box: where to draw it and the
/// partial-tick-interpolated animation inputs.
#[derive(Clone, Copy)]
pub struct BookPreview {
    pub rect: [f32; 4],
    pub gui_scale: f32,
    pub open: f32,
    pub flip: f32,
}

/// A GUI preview box's `[x, y, w, h]` clamped to the swapchain, as the scissor
/// rect its 3D content draws within; None when fully off screen.
fn preview_box_rect(rect: [f32; 4], extent: rhi::Extent) -> Option<rhi::Rect> {
    let x = rect[0].max(0.0) as u32;
    let y = rect[1].max(0.0) as u32;
    let width = (rect[2] as u32).min(extent.width.saturating_sub(x));
    let height = (rect[3] as u32).min(extent.height.saturating_sub(y));
    (width > 0 && height > 0).then_some(rhi::Rect {
        x,
        y,
        width,
        height,
    })
}

// Constructed once per frame and consumed immediately, never stored.
#[allow(clippy::large_enum_variant)]
enum RenderMode<'a> {
    World {
        block_render: BlockRenderView<'a>,
        overlay: Vec<MenuElement>,
        /// The main hand's then the offhand's, as for `held_items`.
        swing_progress: [f32; 2],
        use_anim: [Option<pipelines::held_item::UseAnim>; 2],
        /// The main hand's then the offhand's item.
        held_items: [Option<pipelines::held_item::HeldItemInfo>; 2],
        main_hand_right: bool,
        /// The local player's light, for the first-person arm (vanilla
        /// `renderHandsWithItems`).
        hand_light: LightLevels,
        shown_hands: HandRenderSelection,
        player_team_color: Option<u32>,
        render_first_person_hand: bool,
        destroy_info: Option<(BlockPos, u32, BlockState)>,
        show_chunk_borders: bool,
        sky: SkyState,
        shadows: ShadowScene<'a>,
        moving_blocks: &'a [MovingBlockRender],
        entities: &'a [EntityRenderInfo],
        item_entities: &'a [pipelines::item_entity::ItemRenderInfo],
        block_entities: &'a [BlockEntityRenderInfo],
        particles: &'a [ParticleQuad],
        weather: &'a [WeatherColumn],
        cloud_mode: CloudMode,
        render_distance: u32,
        player_preview: Option<PlayerPreview>,
        book_preview: Option<BookPreview>,
        eyes_in_water: bool,
    },
    MainMenu {
        scroll: f32,
        elements: Vec<MenuElement>,
        cursor: (f32, f32),
        skin: Option<MenuSkin>,
    },
}

/// Where a menu screen draws the player's rotating skin: its anchor as screen
/// fractions and a size factor.
#[derive(Clone, Copy)]
pub struct MenuSkin {
    pub at: (f32, f32),
    pub scale: f32,
}

#[derive(Default, Clone)]
pub struct RenderTimings {
    pub pace_ms: f32,
    pub frame_ms: f32,
    pub fence_ms: f32,
    pub acquire_ms: f32,
    pub cull_ms: f32,
    pub draw_ms: f32,
    pub present_ms: f32,
}

pub struct Renderer {
    ctx: rhi::backend::Context,
    swapchain: rhi::backend::Swapchain,
    camera: Camera,

    render_assets: Arc<BlockRenderAssets>,
    jar_assets_dir: PathBuf,
    asset_index: Option<AssetIndex>,

    chunk_pipeline: ChunkPipeline,
    lightmap: Lightmap,
    hand_pipeline: HandPipeline,
    block_overlay_pipeline: BlockOverlayPipeline,
    sky_pipeline: SkyPipeline,
    panorama_pipeline: PanoramaPipeline,
    menu_pipeline: MenuOverlayPipeline,
    /// The Menu Background Blur option: vanilla's `MenuBlurRadius`.
    menu_blur_radius: u8,
    entity_outline: EntityOutline,
    post_effects: PostEffects,
    skin_preview: SkinPreviewPipeline,
    book_preview: BookPreviewPipeline,
    chunk_border_pipeline: ChunkBorderPipeline,
    item_entity_pipeline: ItemEntityPipeline,
    held_item_pipeline: pipelines::held_item::HeldItemPipeline,
    weather_pipeline: WeatherPipeline,
    particle_pipeline: ParticlePipeline,
    cloud_pipeline: CloudPipeline,
    gui_item_pipeline: pipelines::gui_item::GuiItemPipeline,
    gui_item_atlas: pipelines::gui_item_atlas::GuiItemAtlas,
    /// Force Unicode Font also evens the Auto GUI scale the item atlas is sized
    /// for.
    font_options: FontOptions,

    atlas: TextureAtlas,
    entity_renderer: EntityRenderer,
    entity_shadow_pipeline: EntityShadowPipeline,
    moving_block_buffers: MovingBlockBuffers,
    block_entity_pipeline: BlockEntityPipeline,
    /// The selected shader pack, and its render path once built.
    shader_pack: Option<(
        crate::shaderpack::ShaderPack,
        Option<pipelines::shaderpack::PackRenderer>,
    )>,
    /// The pack last asked for, loaded or not, so a broken one isn't retried
    /// every frame.
    shader_pack_selected: Option<std::path::PathBuf>,
    /// What the selected pack reads about the player's surroundings.
    pack_world: pipelines::shaderpack::PackWorld,
    chunk_buffers: ChunkBufferStore,
    frames: rhi::backend::Frames,
    screenshot: screenshot::ScreenshotCapture,
    swapchain_dirty: bool,
    /// The native window is gone (Android backgrounded the app), and with it
    /// the surface and swapchain; nothing renders until `resume`.
    suspended: bool,
    vsync: bool,
    width: u32,
    height: u32,
    last_timings: RenderTimings,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        font_sources: FontSources<'_>,
        game_dir: &Path,
        cache_dir: &Path,
        vsync: bool,
        panorama_dir: &Path,
        bedrock_ui_dir: &Path,
    ) -> Result<Self, RendererError> {
        let FontSources {
            jar_assets_dir,
            asset_index,
            packs,
            options: font_options,
        } = font_sources;
        let render_assets_handle = {
            let jar_assets_dir = jar_assets_dir.to_path_buf();
            let asset_index = asset_index.clone();
            let cache_dir = cache_dir.to_path_buf();
            std::thread::spawn(move || {
                BlockRenderAssets::load(&jar_assets_dir, &asset_index, Some(&cache_dir), None)
            })
        };

        let (ctx, swapchain_state, mut frames, mut menu_pipeline) =
            boot::bring_up(&window, font_sources, Some(bedrock_ui_dir), vsync)?;
        // The swapchain may pick the surface's `current_extent` rather than the
        // requested size; track that actual extent so layout matches rendering.
        let swapchain_extent = swapchain_state.extent;

        let sw = swapchain_extent.width as f32;
        let sh = swapchain_extent.height as f32;

        let mut splash = |menu: &mut MenuOverlayPipeline, progress: f32, status: &str| {
            let _ = Self::render_splash(
                &ctx,
                &swapchain_state,
                &mut frames,
                menu,
                sw,
                sh,
                progress,
                status,
            );
        };

        splash(&mut menu_pipeline, 0.0, "Loading block models...");

        let camera = Camera::new(swapchain_state.extent.aspect_ratio());
        let render_assets = Arc::new(
            render_assets_handle
                .join()
                .expect("block render assets thread panicked"),
        );

        splash(&mut menu_pipeline, 0.2, "Building texture atlas...");

        let (texture_names, generated_item_textures) =
            chunk::atlas::atlas_texture_names(&render_assets);
        let atlas = TextureAtlas::build(
            &ctx.device,
            jar_assets_dir,
            asset_index,
            &texture_names,
            &generated_item_textures,
            None,
        )
        .map_err(RendererError::Atlas)?;

        splash(&mut menu_pipeline, 0.5, "Creating pipelines...");

        let lightmap = Lightmap::new(&ctx.device);
        let chunk_pipeline =
            ChunkPipeline::new(&ctx.device, swapchain_state.targets(), &atlas, &lightmap);

        let hand_pipeline = HandPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
            &lightmap,
        );

        let block_overlay_pipeline = BlockOverlayPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
        );

        splash(&mut menu_pipeline, 0.7, "Loading sky and panorama...");

        let sky_pipeline = SkyPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
        );

        let weather_pipeline = WeatherPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
            &lightmap,
        );

        let particle_pipeline =
            ParticlePipeline::new(&ctx.device, swapchain_state.targets(), &atlas, &lightmap);

        let cloud_pipeline = CloudPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
        );

        let panorama_pipeline = PanoramaPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            pipelines::panorama::resolve_panorama_faces(panorama_dir, jar_assets_dir, asset_index),
        );

        splash(&mut menu_pipeline, 0.9, "Finalizing...");

        // Wide arms until the profile's skin (and its model flag) is fetched.
        let skin_preview = SkinPreviewPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            hand_pipeline.skin_texture(),
            hand_pipeline.skin_sampler(),
            false,
        );

        let book_preview = BookPreviewPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
        );

        let entity_outline =
            EntityOutline::new(&ctx.device, swapchain_state.targets(), swapchain_extent);
        let post_effects =
            PostEffects::new(&ctx.device, swapchain_state.targets(), swapchain_extent);
        menu_pipeline.set_blur_texture(
            &ctx.device,
            post_effects.blurred_texture(),
            post_effects.blurred_sampler(),
        );

        let entity_renderer = EntityRenderer::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
            packs,
            &lightmap,
        );

        let moving_block_buffers = MovingBlockBuffers::new(&ctx.device);

        let entity_shadow_pipeline = EntityShadowPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
        );

        let block_entity_pipeline = BlockEntityPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            jar_assets_dir,
            asset_index,
            &lightmap,
        );

        let chunk_border_pipeline = pipelines::chunk_borders::ChunkBorderPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
        );

        let chunk_buffers = ChunkBufferStore::new(&ctx.device, ctx.indirect_draw);

        let mut item_entity_pipeline = pipelines::item_entity::ItemEntityPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            &atlas,
            &lightmap,
        );

        let held_item_pipeline = pipelines::held_item::HeldItemPipeline::new(
            &ctx.device,
            swapchain_state.targets(),
            &atlas,
            jar_assets_dir,
            &lightmap,
        );

        splash(&mut menu_pipeline, 0.95, "Caching item meshes...");

        let initial_slot_px =
            pipelines::gui_item_atlas::slot_px_for_screen(sw, sh, font_options.uniform);
        let gui_item_atlas = build_gui_item_atlas(&ctx.device, &mut menu_pipeline, initial_slot_px);

        let gui_item_pipeline = pipelines::gui_item::GuiItemPipeline::new(
            &ctx.device,
            gui_item_atlas.targets(),
            gui_item_atlas.atlas_px(),
            &atlas,
            &lightmap,
        );

        warm_item_meshes(
            &ctx.device,
            &mut item_entity_pipeline,
            &atlas.uv_map,
            &render_assets,
        );

        Ok(Self {
            ctx,
            swapchain: swapchain_state,
            camera,
            render_assets,
            jar_assets_dir: jar_assets_dir.to_path_buf(),
            asset_index: asset_index.clone(),
            atlas,
            chunk_pipeline,
            lightmap,
            hand_pipeline,
            block_overlay_pipeline,
            sky_pipeline,
            panorama_pipeline,
            menu_pipeline,
            menu_blur_radius: crate::ui::menu::DEFAULT_MENU_BACKGROUND_BLURRINESS,
            entity_outline,
            post_effects,
            skin_preview,
            book_preview,
            entity_renderer,
            entity_shadow_pipeline,
            moving_block_buffers,
            block_entity_pipeline,
            shader_pack: None,
            shader_pack_selected: None,
            pack_world: Default::default(),
            chunk_border_pipeline,
            item_entity_pipeline,
            held_item_pipeline,
            weather_pipeline,
            particle_pipeline,
            cloud_pipeline,
            gui_item_pipeline,
            gui_item_atlas,
            font_options,
            chunk_buffers,
            frames,
            screenshot: screenshot::ScreenshotCapture::new(game_dir.to_path_buf()),
            swapchain_dirty: false,
            suspended: false,
            vsync,
            width: swapchain_extent.width,
            height: swapchain_extent.height,
            last_timings: RenderTimings::default(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_splash(
        ctx: &rhi::backend::Context,
        swapchain: &rhi::backend::Swapchain,
        frames: &mut rhi::backend::Frames,
        menu: &mut MenuOverlayPipeline,
        sw: f32,
        sh: f32,
        progress: f32,
        status: &str,
    ) -> Result<(), RendererError> {
        let elements = splash_elements(sw, sh, progress, status);

        frames.wait(&ctx.device)?;
        let Ok(rhi::Acquired::Image {
            index: image_index, ..
        }) = frames.acquire(&ctx.device, swapchain)
        else {
            return Ok(());
        };
        let encoder = frames.begin(&ctx.device)?;

        let mut pass = encoder.begin_render_pass(
            &ctx.device,
            &swapchain.pass(image_index, rhi::LoadOp::Clear([0.0, 0.0, 0.0, 1.0])),
        );
        let empty_uvs: HashMap<String, [f32; 4]> = HashMap::new();
        let frame = frames.index();
        menu.draw(&ctx.device, &mut pass, frame, sw, sh, &elements, &empty_uvs);
        pass.end();
        swapchain.ready_to_present(&encoder, image_index);
        frames.submit_and_present(encoder, ctx, swapchain, image_index)?;
        ctx.device.wait_idle();

        Ok(())
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.width = new_size.width;
        self.height = new_size.height;
        self.swapchain_dirty = true;
        self.camera
            .set_aspect_ratio(new_size.width as f32 / new_size.height as f32);
    }

    /// Stops rendering while the app is in the background, dropping the
    /// surface and swapchain unless they outlive it, but keeping the device
    /// and every other resource.
    pub fn suspend(&mut self) {
        self.ctx.device.wait_idle();
        if !KEEPS_SURFACE_WHEN_SUSPENDED {
            self.swapchain.release_surface(&self.ctx);
            self.ctx.destroy_surface();
        }
        self.suspended = true;
    }

    /// Picks up rendering again, rebuilding the surface and swapchain if
    /// `suspend` dropped them.
    pub fn resume(&mut self, window: &Window) -> Result<(), RendererError> {
        if !self.suspended {
            return Ok(());
        }
        if KEEPS_SURFACE_WHEN_SUSPENDED {
            self.suspended = false;
            return Ok(());
        }
        self.ctx.recreate_surface(window)?;
        self.suspended = false;
        self.recreate_swapchain()
    }

    pub fn set_vsync(&mut self, vsync: bool) {
        if self.vsync != vsync {
            self.vsync = vsync;
            self.swapchain_dirty = true;
        }
    }

    /// Under FIFO, blocks until at most one frame is queued for display so the
    /// next frame's input isn't sampled ahead of a full queue.
    pub fn pace_frame(&mut self) {
        // Bounded so a minimized/occluded window can't stall the loop.
        const PACE_TIMEOUT_NS: u64 = 100_000_000;

        self.last_timings.pace_ms = 0.0;
        if self.suspended || self.swapchain_dirty || !self.swapchain.is_fifo() {
            return;
        }
        let t_pace = std::time::Instant::now();
        self.swapchain_dirty |= !self
            .frames
            .pace(&self.ctx, &self.swapchain, PACE_TIMEOUT_NS);
        self.last_timings.pace_ms = t_pace.elapsed().as_secs_f32() * 1000.0;
    }

    /// Reopens the frame's pass after an offscreen detour, keeping what it
    /// drew.
    pub fn set_menu_blur_radius(&mut self, radius: u8) {
        self.menu_blur_radius = radius;
    }

    /// Vanilla `GuiRenderer.draw`: draws `elements` up to the first blur
    /// marker, then blurs the frame there (`processBlurEffect`). The blur is
    /// written back over the frame for `BlurBackground`, and only kept as the
    /// frosted panels' texture for `BlurBackdrop`. Returns the reopened pass,
    /// the next vertex base and the elements still to draw.
    fn draw_until_blur<'e>(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        image_index: u32,
        frame: usize,
        mut pass: rhi::backend::RenderPassEncoder,
        elements: &'e [MenuElement],
        item_atlas_uvs: &HashMap<String, [f32; 4]>,
    ) -> (rhi::backend::RenderPassEncoder, u32, &'e [MenuElement]) {
        let Some(split) = elements
            .iter()
            .position(|e| matches!(e, MenuElement::BlurBackground | MenuElement::BlurBackdrop))
        else {
            return (pass, 0, elements);
        };
        let extent = self.swapchain.extent;
        let vbase = self.menu_pipeline.draw_from(
            &self.ctx.device,
            &mut pass,
            frame,
            extent.width as f32,
            extent.height as f32,
            &elements[..split],
            item_atlas_uvs,
            0,
        );
        let in_place = matches!(elements[split], MenuElement::BlurBackground);
        // Vanilla `extractBlurredBackground` skips a radius under 1.
        if !in_place || self.menu_blur_radius > 0 {
            pass.end();
            self.post_effects.blur(
                &self.ctx.device,
                encoder,
                self.swapchain.image(image_index),
                self.menu_blur_radius,
            );
            pass = self.resume_frame_pass(encoder, image_index);
            if in_place {
                self.post_effects.finish(&mut pass);
            }
        }
        (pass, vbase, &elements[split + 1..])
    }

    fn resume_frame_pass(
        &self,
        encoder: &rhi::backend::CommandEncoder,
        image_index: u32,
    ) -> rhi::backend::RenderPassEncoder {
        encoder.begin_render_pass(
            &self.ctx.device,
            &self.swapchain.pass(image_index, rhi::LoadOp::Load),
        )
    }

    /// Selects the shader pack at `path`, or turns packs off with `None`. The
    /// pack's programs compile on the next world frame.
    pub fn set_shader_pack(&mut self, path: Option<&std::path::Path>) -> Result<(), String> {
        if self.shader_pack_selected.as_deref() == path {
            return Ok(());
        }
        self.shader_pack_selected = path.map(std::path::Path::to_path_buf);
        if let Some((_, Some(mut pack))) = self.shader_pack.take() {
            self.ctx.device.wait_idle();
            pack.destroy(&self.ctx.device);
        }
        let Some(path) = path else {
            return Ok(());
        };
        let pack = crate::shaderpack::ShaderPack::open(path).map_err(|e| e.to_string())?;
        self.shader_pack = Some((pack, None));
        Ok(())
    }

    /// Sets what the shader pack reads about the player's surroundings.
    pub fn set_pack_world(&mut self, world: pipelines::shaderpack::PackWorld) {
        self.pack_world = world;
    }

    /// Draws a world frame's geometry and full-screen passes through the
    /// selected shader pack, before the swapchain pass draws its `final`;
    /// `false` when no pack draws this frame.
    fn render_pack_world(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        mode: &RenderMode,
    ) -> bool {
        use pipelines::shaderpack::{PackFrame, PackGeometry, PackRenderer};
        let RenderMode::World {
            sky,
            moving_blocks,
            entities,
            block_entities,
            render_distance,
            hand_light,
            eyes_in_water,
            ..
        } = mode
        else {
            return false;
        };
        if self.camera.top_down().is_some() {
            return false;
        }
        let dimension = match sky.skybox {
            crate::world::environment::Skybox::Overworld => "minecraft:overworld",
            crate::world::environment::Skybox::End => "minecraft:the_end",
            crate::world::environment::Skybox::None => "minecraft:the_nether",
        };
        let Some((files, built)) = &mut self.shader_pack else {
            return false;
        };
        if built.as_ref().is_some_and(|p| p.dimension() != dimension) {
            self.ctx.device.wait_idle();
            if let Some(mut pack) = built.take() {
                pack.destroy(&self.ctx.device);
            }
        }
        if built.is_none() {
            match PackRenderer::new(
                &self.ctx.device,
                files,
                dimension,
                self.swapchain.extent,
                self.swapchain.targets(),
                &self.atlas,
                &self.lightmap,
            ) {
                Ok(pack) => *built = Some(pack),
                Err(e) => {
                    tracing::error!("Shader pack {} failed: {e}", files.name);
                    self.shader_pack = None;
                    return false;
                }
            }
        }
        let Some(mut pack) = self.shader_pack.as_mut().and_then(|(_, p)| p.take()) else {
            return false;
        };

        let device = &self.ctx.device;
        let (view, projection) = self.camera.pack_matrices();
        let (near, far) = self.camera.planes();
        let anchor = self.camera.anchor();
        let eye = self.camera_render_position();
        let attributes = &sky.attributes;
        pack.prepare(
            frame,
            &PackFrame {
                view,
                projection,
                camera: eye,
                eye: self.camera.eye_from_anchor(),
                anchor: anchor.as_ivec3(),
                day_time: sky.day_time,
                sun_angle_degrees: attributes.sun_angle,
                moon_phase: attributes.moon_phase,
                rain: sky.rain(),
                fog_color: sky.fog_color,
                sky_color: attributes.sky_color,
                fog: [attributes.fog_start_distance, attributes.fog_end_distance],
                near,
                far,
                eye_light: [
                    f32::from(hand_light.block()) * 16.0,
                    f32::from(hand_light.sky()) * 16.0,
                ],
                eye_in_water: *eyes_in_water,
                screen_brightness: self.lightmap.brightness(),
                atlas_size: self.atlas.size,
                world: self.pack_world.clone(),
            },
        );
        pack.clear(encoder, sky.fog_color);
        self.moving_block_buffers
            .upload(device, frame, moving_blocks);

        let frustum = self.camera.frustum_planes();
        let cull_distance = entity_cull_distance(*render_distance);
        if let Some((pass, _)) = pack.begin_geometry(device, encoder, frame, PackGeometry::Terrain)
        {
            pass.set_bind_group(3, pack.atlas_group);
            for cutout in [false, true] {
                self.chunk_buffers.draw_indirect(&pass, frame, cutout);
                self.moving_block_buffers.draw(&pass, frame, cutout);
            }
            pack.end_geometry(encoder, pass, PackGeometry::Terrain);
        }
        if let Some((mut pass, binding)) =
            pack.begin_geometry(device, encoder, frame, PackGeometry::Entities)
        {
            self.entity_renderer.pack_binding = Some(binding);
            self.entity_renderer.draw(
                &mut pass,
                frame,
                entities,
                &frustum,
                anchor,
                eye,
                cull_distance,
            );
            self.entity_renderer.pack_binding = None;
            pack.end_geometry(encoder, pass, PackGeometry::Entities);
        }
        if let Some((mut pass, binding)) =
            pack.begin_geometry(device, encoder, frame, PackGeometry::BlockEntities)
        {
            self.block_entity_pipeline.pack_binding = Some(binding);
            self.block_entity_pipeline
                .draw(&mut pass, frame, anchor, block_entities);
            self.block_entity_pipeline.pack_binding = None;
            pack.end_geometry(encoder, pass, PackGeometry::BlockEntities);
        }
        pack.copy_opaque_depth(encoder);
        pack.run_full_screen(device, encoder, frame, true);
        if let Some((pass, _)) = pack.begin_geometry(device, encoder, frame, PackGeometry::Water) {
            pass.set_bind_group(3, pack.atlas_group);
            self.chunk_buffers.draw_water(&pass, &frustum, anchor, eye);
            pack.end_geometry(encoder, pass, PackGeometry::Water);
        }
        pack.run_full_screen(device, encoder, frame, false);
        // TODO: particles, weather, clouds, dropped items, the block-break
        // overlay and entity shadows through their `gbuffers_*` programs; the
        // block outline and chunk borders over the pack's depth.
        if let Some((_, slot)) = &mut self.shader_pack {
            *slot = Some(pack);
        }
        true
    }

    fn recreate_swapchain(&mut self) -> Result<(), RendererError> {
        self.ctx.device.wait_idle();

        let old_targets = self.swapchain.targets();
        self.swapchain.rebuild(
            &self.ctx,
            &mut self.frames,
            self.width,
            self.height,
            self.vsync,
        )?;

        // Adopt the swapchain's actual extent (may differ from the requested
        // window size, e.g. macOS fullscreen) so the viewport, menu layout, blur,
        // and camera aspect all agree — otherwise the image stretches/letterboxes.
        self.width = self.swapchain.extent.width;
        self.height = self.swapchain.extent.height;
        self.camera
            .set_aspect_ratio(self.width as f32 / self.height as f32);

        // Pipelines name only their targets' formats, which a resize keeps.
        let targets = self.swapchain.targets();
        if targets != old_targets {
            self.chunk_pipeline.destroy(&self.ctx.device);
            self.chunk_pipeline =
                ChunkPipeline::new(&self.ctx.device, targets, &self.atlas, &self.lightmap);

            self.hand_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.block_overlay_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.sky_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.panorama_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.menu_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.skin_preview
                .recreate_pipeline(&self.ctx.device, targets);
            self.book_preview
                .recreate_pipeline(&self.ctx.device, targets);
            self.entity_renderer
                .recreate_pipeline(&self.ctx.device, targets);
            self.entity_outline
                .recreate_pipeline(&self.ctx.device, targets);
            self.post_effects
                .recreate_pipeline(&self.ctx.device, targets);
            self.entity_shadow_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.block_entity_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.item_entity_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.held_item_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.weather_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.particle_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.cloud_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
            self.chunk_border_pipeline
                .recreate_pipeline(&self.ctx.device, targets);
        }
        self.entity_outline
            .resize(&self.ctx.device, self.swapchain.extent);
        self.post_effects
            .resize(&self.ctx.device, self.swapchain.extent);
        if let Some((_, slot)) = &mut self.shader_pack
            && let Some(pack) = slot
        {
            if targets == old_targets {
                pack.resize(&self.ctx.device, self.swapchain.extent);
            } else {
                // The final pass draws into the swapchain's format.
                pack.destroy(&self.ctx.device);
                *slot = None;
            }
        }
        self.menu_pipeline.set_blur_texture(
            &self.ctx.device,
            self.post_effects.blurred_texture(),
            self.post_effects.blurred_sampler(),
        );

        self.swapchain_dirty = false;
        Ok(())
    }

    pub fn screen_width(&self) -> u32 {
        self.width
    }

    pub fn screen_height(&self) -> u32 {
        self.height
    }

    #[inline]
    pub const fn last_timings(&self) -> &RenderTimings {
        &self.last_timings
    }

    pub fn update_camera(
        &mut self,
        input: &mut InputState,
        dt: f32,
        sensitivity: f32,
        invert_x: bool,
        invert_y: bool,
    ) {
        self.camera
            .update_look(input, dt, sensitivity, invert_x, invert_y);
    }

    pub fn sync_camera_pos(&mut self, position: Position) {
        self.camera.sync_pos(position);
    }

    pub fn set_sleeping_camera_look(&mut self, yaw_deg: Option<f32>) {
        self.camera.set_sleeping_look(yaw_deg);
    }

    pub fn tick_texture_animations(&mut self) {
        self.atlas.tick_animations();
    }

    pub fn hold_texture_animations(&mut self) {
        self.atlas.hold_animations();
    }

    /// Redraws the lightmap with `info` on the next frame, as vanilla does
    /// once per tick.
    pub fn tick_lightmap(&mut self, info: LightmapInfo) {
        self.lightmap.tick(info);
    }

    pub fn set_entity_camera_look(&mut self, look_dir: Option<LookDirection>) {
        self.camera.set_entity_look(look_dir);
    }

    /// The spectator view effect the world frame runs, if any.
    pub fn set_post_effect(&mut self, effect: Option<PostEffect>) {
        self.post_effects.set(&self.ctx.device, effect);
    }

    pub fn set_view_bob(&mut self, walk_dist: f32, bob: f32, enabled: bool) {
        self.camera.set_view_bob(walk_dist, bob, enabled);
    }

    pub fn set_hurt(&mut self, hurt_time: u8, hurt_dir: f32, damage_tilt_strength: f32) {
        self.camera
            .set_hurt(hurt_time, hurt_dir, damage_tilt_strength);
    }

    pub fn reset_camera(&mut self, position: Position, look_dir: LookDirection) {
        self.camera.reset(position, look_dir);
    }

    pub fn set_top_down_radius(&mut self, radius_blocks: f32) {
        self.camera.frame_top_down(radius_blocks);
    }

    pub fn clear_top_down(&mut self) {
        self.camera.clear_top_down();
    }

    pub fn update_third_person_distance(
        &mut self,
        block_render: BlockRenderView<'_>,
        eye_pos: Position,
        chunks: &crate::world::chunk::ChunkStore,
        max_distance: f32,
    ) {
        if self.camera.mode == camera::CameraMode::FirstPerson || self.camera.top_down().is_some() {
            return;
        }
        // TODO: vanilla `Camera.getMaxZoom` clips 8 rays offset by 0.1 against
        // visual shapes with no minimum; this marches 0.2 steps against full
        // cubes with a 0.4 pad and a 0.5 floor, so a distance of 0 sits 0.5 back.
        let max = max_distance as f64;
        let fwd = self.camera.render_look_dir().as_vec().as_dvec3();
        let dir = if self.camera.mode == camera::CameraMode::ThirdPersonFront {
            fwd
        } else {
            -fwd
        };

        let mut dist = max;

        let m = 0.4;
        let corners = [
            dvec3(m, m, m),
            dvec3(m, m, -m),
            dvec3(m, -m, m),
            dvec3(m, -m, -m),
            dvec3(-m, m, m),
            dvec3(-m, m, -m),
            dvec3(-m, -m, m),
            dvec3(-m, -m, -m),
        ];

        let step = 0.2;
        let mut t = step;
        while t <= max {
            let p = eye_pos + dir * t;
            let hit = corners.iter().any(|off| {
                let check = p + *off;
                let state = chunks.get_block_state(
                    check.x.floor() as i32,
                    check.y.floor() as i32,
                    check.z.floor() as i32,
                );
                block_render.is_opaque_full_cube(state)
            });
            if hit {
                dist = (t - 0.3).max(0.5);
                break;
            }
            t += step;
        }

        self.camera.third_person_dist = dist.max(0.5) as f32;
    }

    pub fn update_fov_mod(&mut self, modifier: f32) {
        self.camera.update_fov_modifier(modifier);
    }

    pub fn set_fluid_fov_factor(&mut self, factor: f32) {
        self.camera.set_fluid_fov_factor(factor);
    }

    pub fn set_death_time(&mut self, death_time: f32) {
        self.camera.set_death_time(death_time);
    }

    pub fn set_render_partial_tick(&mut self, partial_tick: f32) {
        self.camera.set_render_partial_tick(partial_tick);
    }

    pub fn set_base_fov(&mut self, degrees: f32) {
        self.camera.base_fov_degrees = degrees;
    }

    pub fn camera_look_dir(&self) -> LookDirection {
        self.camera.look_dir
    }

    /// Rotation-only view-projection for the locator bar's waypoint pitch test.
    pub fn locator_projection(&self) -> glam::Mat4 {
        self.camera.view_rotation_projection()
    }

    /// Effective camera (yaw, pitch) in degrees, mirrored-view adjusted.
    pub fn camera_effective_look_deg(&self) -> (f32, f32) {
        self.camera.effective_look_deg()
    }

    pub fn camera_fov_degrees(&self) -> f32 {
        self.camera.fov_degrees()
    }

    pub fn camera_pivot_position(&self) -> Position {
        self.camera.position
    }

    /// Camera position used for rendering (eye plus any third-person offset).
    pub fn camera_render_position(&self) -> glam::DVec3 {
        *self.camera.position + self.camera.third_person_offset().as_dvec3()
    }

    /// The render anchor (camera block position); world-space data uploaded
    /// to the GPU is rebased against this in f64 first (see `Camera::anchor`).
    pub fn camera_anchor(&self) -> glam::DVec3 {
        self.camera.anchor()
    }

    pub fn project_world_to_screen(&self, position: glam::DVec3) -> Option<(f32, f32)> {
        let clip = self.camera.view_rotation_projection()
            * (position - self.camera_render_position())
                .as_vec3()
                .extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        (ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0 && ndc.z.abs() <= 1.0).then_some((
            (ndc.x + 1.0) * self.width as f32 * 0.5,
            (1.0 - ndc.y) * self.height as f32 * 0.5,
        ))
    }

    /// Six normalized frustum planes (camera-relative, same convention as the
    /// GPU cull), for CPU-side visibility classification of
    /// mesh-scheduling.
    pub fn frustum_planes(&self) -> [[f32; 4]; 6] {
        self.camera.frustum_planes()
    }

    /// Frustum planes widened by `extra_radians` of FOV, for the tier-1 margin.
    pub fn frustum_planes_dilated(&self, extra_radians: f32) -> [[f32; 4]; 6] {
        self.camera.frustum_planes_dilated(extra_radians)
    }

    pub fn cycle_camera_mode(&mut self) {
        self.camera.mode = self.camera.mode.cycle();
    }

    pub fn is_first_person(&self) -> bool {
        self.camera.mode == camera::CameraMode::FirstPerson
    }

    pub fn gpu_name(&self) -> &str {
        &self.ctx.gpu_name
    }

    pub fn api_version(&self) -> &str {
        &self.ctx.api_version
    }

    pub fn loaded_chunk_count(&self) -> u32 {
        self.chunk_buffers.chunk_count()
    }

    /// Sections actually drawn after frustum culling (lags a few frames). The
    /// graph's occluded sections are omitted before the cull, so this also
    /// drops when occlusion hides geometry — useful for the F3 overlay.
    pub fn sections_drawn(&self) -> u32 {
        self.chunk_buffers.sections_drawn()
    }

    /// Push the CPU visibility graph's per-column visible-section masks to the
    /// chunk buffer store, which omits occluded sections from the GPU cull.
    pub fn set_chunk_visibility(&mut self, vis: HashMap<ChunkPos, u32>) {
        self.chunk_buffers.set_chunk_visibility(vis);
    }

    /// Arm a vanilla F2 screenshot; captured on the next presented frame.
    pub fn request_screenshot(&mut self) {
        self.screenshot.arm();
    }

    /// [`Renderer::request_screenshot`], saved to `path`.
    #[cfg(feature = "test-harness")]
    pub fn request_screenshot_to(&mut self, path: std::path::PathBuf) {
        self.screenshot.arm_to(path);
    }

    /// Drain finished screenshots: `Ok(relative filename)` or `Err(message)`.
    /// The caller turns each into a chat line.
    pub fn take_screenshot_messages(&mut self) -> Vec<Result<String, String>> {
        self.screenshot.drain_results()
    }

    /// A screenshot capture or encode is still in flight.
    pub fn screenshot_saving(&self) -> bool {
        self.screenshot.saving()
    }

    pub fn wait_for_all_frames(&self) {
        self.frames.wait_all(&self.ctx.device);
    }

    /// Upload a batch of chunk meshes in a single coalesced transfer. Returns,
    /// per mesh that hit pool exhaustion, the section indices dropped (need
    /// re-mesh); empty on success.
    pub fn upload_chunk_meshes(&mut self, meshes: &[ChunkMeshData]) -> Vec<(ChunkPos, Vec<i32>)> {
        self.chunk_buffers.upload_batch(&self.ctx.device, meshes)
    }

    pub fn remove_chunk_mesh(&mut self, pos: &ChunkPos) {
        self.chunk_buffers.remove(pos);
    }

    pub fn clear_chunk_meshes(&mut self) {
        self.wait_for_all_frames();
        self.chunk_buffers.clear();
    }

    pub fn render_assets(&self) -> &BlockRenderAssets {
        &self.render_assets
    }

    pub fn update_chunk_borders(&mut self, min_y: i32, max_y: i32) {
        self.chunk_border_pipeline.update_lines(
            *self.camera.position,
            self.camera_render_position(),
            min_y,
            max_y,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_world(
        &mut self,
        block_render: BlockRenderView<'_>,
        window: &Window,
        hide_cursor: bool,
        overlay: Vec<MenuElement>,
        swing_progress: [f32; 2],
        use_anim: [Option<pipelines::held_item::UseAnim>; 2],
        held_items: [Option<(String, azalea_inventory::ItemStackData)>; 2],
        main_hand_right: bool,
        hand_light: LightLevels,
        shown_hands: HandRenderSelection,
        player_team_color: Option<u32>,
        render_first_person_hand: bool,
        destroy_info: Option<(BlockPos, u32, BlockState)>,
        show_chunk_borders: bool,
        mut sky: SkyState,
        shadows: ShadowScene<'_>,
        moving_blocks: &[MovingBlockRender],
        entities: &[EntityRenderInfo],
        item_entities: &[pipelines::item_entity::ItemRenderInfo],
        block_entities: &[BlockEntityRenderInfo],
        particles: &[ParticleQuad],
        weather: &[WeatherColumn],
        cloud_mode: CloudMode,
        render_distance: u32,
        player_preview: Option<PlayerPreview>,
        book_preview: Option<BookPreview>,
        eyes_in_water: bool,
    ) -> Result<(), RendererError> {
        // Refresh the far plane before this frame's view/projection and fog.
        self.camera.set_render_distance(render_distance);
        let held_items = held_items.map(|held| {
            held.map(|(name, stack)| {
                let model = self.render_assets.stack_model_key(&name, &stack);
                let item_tints =
                    self.render_assets
                        .item_tint_palette(&model, Some(&stack), player_team_color);
                let has_3d_model = self.ensure_item_mesh(&model).is_block_model;
                pipelines::held_item::HeldItemInfo {
                    name,
                    model,
                    item_tints,
                    has_3d_model,
                    charged_crossbow: crate::player::interaction::is_charged_crossbow(&stack),
                }
            })
        });
        sky.fog_color = fog::fog_color(&sky, self.camera.forward(), render_distance);
        // Vanilla clears to the fog colour, which shows below the sky disc's
        // rim; underwater, the water fog colour.
        let [r, g, b] = if eyes_in_water {
            camera::WATER_FOG_COLOR
        } else {
            sky.fog_color.to_array()
        };
        self.render_frame(
            window,
            hide_cursor,
            [r, g, b, 1.0],
            RenderMode::World {
                block_render,
                overlay,
                swing_progress,
                use_anim,
                held_items,
                main_hand_right,
                hand_light,
                shown_hands,
                player_team_color,
                render_first_person_hand,
                destroy_info,
                show_chunk_borders,
                sky,
                shadows,
                moving_blocks,
                entities,
                item_entities,
                block_entities,
                particles,
                weather,
                cloud_mode,
                render_distance,
                player_preview,
                book_preview,
                eyes_in_water,
            },
        )
    }

    pub fn render_menu(
        &mut self,
        window: &Window,
        scroll: f32,
        elements: Vec<MenuElement>,
        cursor: (f32, f32),
        skin: Option<MenuSkin>,
    ) -> Result<(), RendererError> {
        self.render_frame(
            window,
            false,
            [0.0, 0.0, 0.0, 1.0],
            RenderMode::MainMenu {
                scroll,
                elements,
                cursor,
                skin,
            },
        )
    }

    pub fn reload_assets(
        &mut self,
        packs: &crate::resource_pack::ResourcePackManager,
        font_options: FontOptions,
    ) {
        self.ctx.device.wait_idle();

        self.render_assets = Arc::new(BlockRenderAssets::load(
            &self.jar_assets_dir,
            &self.asset_index,
            None,
            Some(packs),
        ));

        self.item_entity_pipeline.clear_meshes(&self.ctx.device);
        self.atlas.destroy(&self.ctx.device);
        let (texture_names, generated_item_textures) =
            chunk::atlas::atlas_texture_names(&self.render_assets);
        self.atlas = TextureAtlas::build(
            &self.ctx.device,
            &self.jar_assets_dir,
            &self.asset_index,
            &texture_names,
            &generated_item_textures,
            Some(packs),
        )
        .expect("failed to rebuild atlas");

        self.chunk_pipeline
            .rebind_atlas(&self.ctx.device, &self.atlas);
        self.item_entity_pipeline
            .rebind_atlas(&self.ctx.device, &self.atlas);
        self.gui_item_pipeline
            .rebind_atlas(&self.ctx.device, &self.atlas);
        self.held_item_pipeline
            .rebind_atlas(&self.ctx.device, &self.atlas);
        self.particle_pipeline
            .rebind_atlas(&self.ctx.device, &self.atlas);
        // The pack binds the old atlas; it rebuilds on the next frame.
        if let Some((_, pack)) = &mut self.shader_pack
            && let Some(mut pack) = pack.take()
        {
            pack.destroy(&self.ctx.device);
        }
        self.entity_renderer.reload_armor_assets(
            &self.ctx.device,
            &self.jar_assets_dir,
            &self.asset_index,
            packs,
        );

        self.reload_fonts(packs, font_options);

        warm_item_meshes(
            &self.ctx.device,
            &mut self.item_entity_pipeline,
            &self.atlas.uv_map,
            &self.render_assets,
        );

        // GUI item slots cache fully rendered pixels. Releasing their keys is
        // enough: reused slots are marked stale and cleared before rebaking.
        self.gui_item_atlas.invalidate_all();

        tracing::info!("Assets reloaded");
    }

    /// Vanilla `FontManager.updateOptions`: only the font sets change.
    pub fn reload_fonts(
        &mut self,
        packs: &crate::resource_pack::ResourcePackManager,
        font_options: FontOptions,
    ) {
        self.font_options = font_options;
        self.ctx.device.wait_idle();
        if let Err(error) = self.menu_pipeline.reload_minecraft_fonts(
            &self.ctx.device,
            FontSources {
                jar_assets_dir: &self.jar_assets_dir,
                asset_index: &self.asset_index,
                packs,
                options: font_options,
            },
        ) {
            tracing::warn!("Keeping previous Minecraft fonts after reload failure: {error}");
        }
    }

    pub fn reload_panorama(&mut self, panorama_dir: &Path) {
        let faces = pipelines::panorama::resolve_panorama_faces(
            panorama_dir,
            &self.jar_assets_dir,
            &self.asset_index,
        );
        self.panorama_pipeline
            .reload_cubemap(&self.ctx.device, faces);
    }

    /// Brings the Bedrock title and button sprites into the menu atlas when
    /// their files arrived after the atlas was built.
    pub fn load_bedrock_sprites(&mut self, bedrock_ui_dir: &Path) {
        if self.menu_pipeline.has_sprite(SpriteId::BedrockTitle) {
            return;
        }
        self.menu_pipeline.reload_sprite_atlas(
            &self.ctx.device,
            &self.jar_assets_dir,
            &self.asset_index,
            Some(bedrock_ui_dir),
        );
    }

    pub fn trigger_skin_swing(&mut self) {
        self.skin_preview.trigger_swing();
    }

    pub fn load_player_skin(&mut self, uuid: &uuid::Uuid, rt: &tokio::runtime::Runtime) {
        let uuid_str = uuid.to_string().replace('-', "");
        let skin = rt.block_on(async { fetch_skin_texture(&uuid_str).await });
        match skin {
            Ok(skin) => {
                // In-flight frames may still reference the old skin texture,
                // hand mesh, and preview pipeline about to be destroyed.
                self.ctx.device.wait_idle();
                self.hand_pipeline.reload_skin(&self.ctx.device, &skin);
                self.skin_preview.destroy(&self.ctx.device);
                self.skin_preview = SkinPreviewPipeline::new(
                    &self.ctx.device,
                    self.swapchain.targets(),
                    self.hand_pipeline.skin_texture(),
                    self.hand_pipeline.skin_sampler(),
                    skin.slim,
                );
                self.update_player_entity_skin(uuid, &skin);
            }
            Err(e) => tracing::warn!("Failed to load player skin: {e}"),
        }
    }

    pub fn update_player_entity_skin(&mut self, uuid: &uuid::Uuid, skin: &SkinData) {
        self.entity_renderer
            .update_player_skin(&self.ctx.device, uuid, skin);
    }

    pub fn remove_player_entity_skin(&mut self, uuid: &uuid::Uuid) {
        self.entity_renderer
            .remove_player_skin(&self.ctx.device, uuid);
    }

    pub fn clear_player_entity_skins(&mut self) {
        self.entity_renderer.clear_player_skins(&self.ctx.device);
    }

    pub fn update_favicon_atlas(&mut self, favicons: &[(String, Vec<u8>, u32)]) {
        self.menu_pipeline
            .update_favicon_atlas(&self.ctx.device, favicons);
    }

    /// Player faces reuse the string-keyed favicon atlas, which holds only
    /// the last upload; the menu sends faces and favicons together
    /// (`MainMenu::menu_atlas_images`).
    pub fn update_face_atlas(&mut self, faces: &[(String, Vec<u8>, u32)]) {
        self.update_favicon_atlas(faces);
    }

    /// The inline objects the menu text drew since the last call.
    pub fn drain_drawn_inline_objects(
        &mut self,
    ) -> std::collections::hash_map::Drain<'_, String, crate::ui::text::InlineObject> {
        self.menu_pipeline.geometry.drain_drawn_inline_objects()
    }

    /// Points an animated inline object at the frame showing now.
    pub fn set_inline_object_frame(&mut self, key: &str, frame_key: &str) {
        self.menu_pipeline
            .geometry
            .set_inline_object_frame(key, frame_key);
    }

    pub fn menu_text_width(&self, text: &str, scale: f32) -> f32 {
        self.menu_pipeline.geometry.text_width(text, scale)
    }

    /// Menu text width in the SGA (`minecraft:alt`) glyphs.
    pub fn menu_text_width_sga(&self, text: &str, scale: f32) -> f32 {
        self.menu_pipeline.geometry.mc_text_width_sga(text, scale)
    }

    pub fn menu_spans_width(&self, spans: &[crate::ui::text::TextSpan], scale: f32) -> f32 {
        self.menu_pipeline.geometry.spans_width(spans, scale)
    }

    /// Local item-mesh classification and bounds used by dropped-item layout.
    pub fn item_mesh_info(&self, name: &str) -> Option<pipelines::item_entity::ItemMeshInfo> {
        self.item_entity_pipeline.mesh_info(name)
    }

    pub fn ensure_item_mesh(&mut self, name: &str) -> pipelines::item_entity::ItemMeshInfo {
        if let Some(info) = self.item_entity_pipeline.mesh_info(name) {
            return info;
        }
        let is_block_model = if let Some(model) = self.render_assets.get_item_model(name) {
            self.item_entity_pipeline.ensure_mesh(
                &self.ctx.device,
                name,
                model,
                &self.atlas.uv_map,
            );
            true
        } else {
            self.item_entity_pipeline.ensure_flat_mesh(
                &self.ctx.device,
                name,
                &self.render_assets.flat_item_layers(name),
                &self.atlas.uv_map,
            );
            false
        };
        // Read back the real bounds; fall back to full-cube / flat defaults if the
        // mesh baked empty (missing asset) — nothing renders for it anyway.
        self.item_entity_pipeline
            .mesh_info(name)
            .unwrap_or(pipelines::item_entity::ItemMeshInfo {
                is_block_model,
                bounds_min: if is_block_model {
                    glam::Vec3::splat(-0.5)
                } else {
                    glam::Vec3::new(-0.5, -0.5, -1.0 / 32.0)
                },
                bounds_max: if is_block_model {
                    glam::Vec3::splat(0.5)
                } else {
                    glam::Vec3::new(0.5, 0.5, 1.0 / 32.0)
                },
            })
    }

    fn render_frame(
        &mut self,
        window: &Window,
        hide_cursor: bool,
        clear_color: [f32; 4],
        mut mode: RenderMode<'_>,
    ) -> Result<(), RendererError> {
        if self.suspended {
            return Ok(());
        }
        if self.swapchain_dirty {
            self.recreate_swapchain()?;
        }

        let frame = self.frames.index();

        let t_fence = std::time::Instant::now();
        self.frames.wait(&self.ctx.device)?;
        let fence_ms = t_fence.elapsed().as_secs_f32() * 1000.0;

        // Fence signalled: reclaim chunk slices the GPU is now provably done with,
        // and read back any screenshot copy recorded for this frame index.
        self.chunk_buffers.begin_frame();
        self.screenshot.collect_ready(frame, &self.ctx.device);

        let t_acquire = std::time::Instant::now();
        let (image_index, suboptimal) =
            match self.frames.acquire(&self.ctx.device, &self.swapchain)? {
                rhi::Acquired::Image { index, suboptimal } => (index, suboptimal),
                rhi::Acquired::OutOfDate => {
                    self.swapchain_dirty = true;
                    return Ok(());
                }
            };
        self.swapchain_dirty |= suboptimal && self.swapchain.surface_resized(&self.ctx);
        let acquire_ms = t_acquire.elapsed().as_secs_f32() * 1000.0;

        if let RenderMode::World {
            ref sky,
            render_distance,
            eyes_in_water,
            ..
        } = mode
        {
            let uniform = CameraUniform::new(&self.camera, sky, render_distance, eyes_in_water);
            self.chunk_pipeline.update_camera(frame, &uniform);
            self.block_overlay_pipeline.update_camera(frame, &uniform);
            self.entity_renderer.update_camera(frame, &uniform);
            self.entity_shadow_pipeline.update_camera(frame, &uniform);
            self.block_entity_pipeline.update_camera(frame, &uniform);
            self.chunk_border_pipeline.update_camera(frame, &uniform);
            self.item_entity_pipeline.update_camera(frame, &uniform);
            self.weather_pipeline.update_camera(frame, &uniform);
            self.particle_pipeline.update_camera(frame, &uniform);
            self.cloud_pipeline.update_camera(frame, &uniform);
        }

        if hide_cursor {
            window.set_cursor_visible(false);
        }

        let encoder = self.frames.begin(&self.ctx.device)?;
        self.atlas.record_animation_upload(&encoder, frame);
        self.lightmap.record(&self.ctx.device, &encoder);

        let scissor = rhi::Rect::covering(self.swapchain.extent);

        if matches!(&mode, RenderMode::World { .. }) {
            let frustum = self.camera.frustum_planes();
            // The eye (including the third-person offset) is the origin the chunk
            // vertex shader renders relative to, so the cull must use it too.
            self.chunk_buffers.dispatch_cull(
                &encoder,
                frame,
                &frustum,
                self.camera.anchor(),
                self.camera_render_position(),
            );
        }

        let player_team_color = match &mode {
            RenderMode::World {
                player_team_color, ..
            } => *player_team_color,
            RenderMode::MainMenu { .. } => None,
        };
        let menu_elements: &mut [MenuElement] = match &mut mode {
            RenderMode::World { overlay, .. } => overlay.as_mut_slice(),
            RenderMode::MainMenu { elements, .. } => elements.as_mut_slice(),
        };
        let mut unique_items: HashMap<String, (String, Vec<u32>)> = HashMap::new();
        let mut add_item = |name: &str, tints: &[u32]| {
            unique_items
                .entry(pipelines::menu_overlay::item_icon_atlas_key(name, tints))
                .or_insert_with(|| (name.to_owned(), tints.to_vec()));
        };
        for elem in menu_elements.iter_mut() {
            match elem {
                MenuElement::ItemIcon {
                    item_name,
                    item_stack,
                    use_player_team,
                    item_tints,
                    ..
                } => {
                    let owner_team_color = if *use_player_team {
                        player_team_color
                    } else {
                        None
                    };
                    if let Some(stack) = item_stack {
                        *item_name = self.render_assets.stack_model_key(item_name, stack);
                    }
                    *item_tints = self.render_assets.item_tint_palette(
                        item_name,
                        item_stack.as_ref(),
                        owner_team_color,
                    );
                    add_item(item_name, item_tints);
                }
                // Vanilla `ClientBundleTooltip` draws its contents with the
                // player as owner.
                MenuElement::BundleTooltip {
                    items, item_tints, ..
                } => {
                    *item_tints = items
                        .iter()
                        .map(|item| {
                            let azalea_inventory::ItemStack::Present(data) = item else {
                                return Vec::new();
                            };
                            let name = crate::player::inventory::item_resource_name(data.kind);
                            let tints = self.render_assets.item_tint_palette(
                                &name,
                                Some(data),
                                player_team_color,
                            );
                            add_item(&name, &tints);
                            tints
                        })
                        .collect();
                }
                _ => {}
            }
        }

        let target_slot_px = pipelines::gui_item_atlas::slot_px_for_screen(
            self.swapchain.extent.width as f32,
            self.swapchain.extent.height as f32,
            self.font_options.uniform,
        );
        if target_slot_px != self.gui_item_atlas.slot_px() {
            // Mid-cmd-recording wait_idle: this cmd buffer is unsubmitted so
            // holds no in-flight references, and `Device::submit` inside the
            // rebuild uses a separate primary cmd from the same pool.
            self.ctx.device.wait_idle();
            self.gui_item_atlas.destroy(&self.ctx.device);
            self.gui_item_atlas =
                build_gui_item_atlas(&self.ctx.device, &mut self.menu_pipeline, target_slot_px);
            self.gui_item_pipeline
                .recreate_pipeline(&self.ctx.device, self.gui_item_atlas.targets());
            self.gui_item_pipeline
                .set_atlas_px(self.gui_item_atlas.atlas_px());
        }

        let unique_keys: HashSet<String> = unique_items.keys().cloned().collect();
        if !self.gui_item_atlas.has_space_for_all(&unique_keys)
            && !self.gui_item_atlas.reclaim_space_for(&unique_keys)
        {
            tracing::warn!(
                "gui_item_atlas: out of slots for {} unique items; some icons will not render",
                unique_items.len()
            );
        }
        let mut item_atlas_uvs: HashMap<String, [f32; 4]> = HashMap::new();
        struct BakeJob {
            slot: pipelines::gui_item_atlas::Slot,
            name: String,
            item_tints: Vec<u32>,
            display: crate::world::block::model::DisplayTransform,
            needs_clear: bool,
        }
        let mut bake_list: Vec<BakeJob> = Vec::new();
        for (key, (name, item_tints)) in &unique_items {
            let discard = pipelines::gui_item_atlas::is_animated_item(name);
            if let Some((slot, state)) = self.gui_item_atlas.get_or_allocate(key, discard) {
                item_atlas_uvs.insert(key.clone(), self.gui_item_atlas.slot_uv(&slot));
                if !matches!(state, pipelines::gui_item_atlas::SlotState::Ready) {
                    bake_list.push(BakeJob {
                        slot,
                        name: name.clone(),
                        item_tints: item_tints.clone(),
                        display: self.render_assets.get_item_gui_transform(name),
                        needs_clear: matches!(state, pipelines::gui_item_atlas::SlotState::Stale),
                    });
                }
            }
        }
        if !bake_list.is_empty() {
            let mut bake = self
                .gui_item_atlas
                .begin_bake_pass(&self.ctx.device, &encoder);
            let required_tint_colors = bake_list.iter().map(|job| job.item_tints.len()).sum();
            self.gui_item_pipeline.bind_for_bake_pass(
                &self.ctx.device,
                &mut bake,
                frame,
                bake_list.len(),
                required_tint_colors,
            );
            for job in &bake_list {
                if job.needs_clear {
                    self.gui_item_atlas.clear_slot_color(&bake, &job.slot);
                }
                bake.set_scissor_rect(self.gui_item_atlas.scissor_rect(&job.slot));
                let (sx, sy) = self.gui_item_atlas.slot_origin_pixels(&job.slot);
                self.gui_item_pipeline.bake_to_slot(
                    &bake,
                    frame,
                    &self.item_entity_pipeline,
                    sx,
                    sy,
                    self.gui_item_atlas.slot_px(),
                    &job.name,
                    job.display,
                    &job.item_tints,
                );
            }
            self.gui_item_atlas.end_bake_pass(&encoder, bake);
        }

        let pack_drawn = self.render_pack_world(&encoder, frame, &mode);

        let mut pass = encoder.begin_render_pass(
            &self.ctx.device,
            &self
                .swapchain
                .pass(image_index, rhi::LoadOp::Clear(clear_color)),
        );

        let sw = self.swapchain.extent.width as f32;
        let sh = self.swapchain.extent.height as f32;

        let frame_start = std::time::Instant::now();

        match &mode {
            RenderMode::World {
                block_render,
                overlay,
                swing_progress,
                use_anim,
                held_items,
                main_hand_right,
                hand_light,
                shown_hands,
                player_team_color: _,
                render_first_person_hand,
                destroy_info,
                show_chunk_borders,
                sky,
                shadows,
                moving_blocks,
                entities,
                item_entities,
                block_entities,
                particles,
                weather,
                cloud_mode,
                render_distance,
                player_preview,
                book_preview,
                eyes_in_water,
            } => {
                let cull_ms = if pack_drawn {
                    if let Some((_, Some(pack))) = &mut self.shader_pack {
                        pack.draw_final(&self.ctx.device, &mut pass, frame);
                    }
                    0.0
                } else {
                    // Vanilla water fog hides the sky dome and clouds; the framebuffer
                    // is cleared to the water fog color, so skipping them tints the view
                    // when looking up out of geometry.
                    if !*eyes_in_water {
                        self.sky_pipeline.update_and_draw(
                            &mut pass,
                            frame,
                            &self.camera,
                            sky,
                            *render_distance,
                        );
                    }

                    let t_cull = std::time::Instant::now();
                    // Solid (no discard) first so it lays down depth and early-Z lets
                    // the front-to-back order reject occluded fragments; cutout after.
                    self.moving_block_buffers
                        .upload(&self.ctx.device, frame, moving_blocks);
                    self.chunk_pipeline.bind(&mut pass, frame, false);
                    self.chunk_buffers.draw_indirect(&pass, frame, false);
                    self.moving_block_buffers.draw(&pass, frame, false);
                    self.chunk_pipeline.bind(&mut pass, frame, true);
                    self.chunk_buffers.draw_indirect(&pass, frame, true);
                    self.moving_block_buffers.draw(&pass, frame, true);
                    let cull_ms = t_cull.elapsed().as_secs_f32() * 1000.0;

                    let anchor = self.camera.anchor();
                    let eye = self.camera_render_position();

                    if let Some((block_pos, stage, state)) = destroy_info {
                        self.block_overlay_pipeline.draw(
                            &mut pass,
                            frame,
                            *block_render,
                            *state,
                            block_pos,
                            anchor,
                            *stage,
                        );
                    }

                    let ent_frustum = self.camera.frustum_planes();
                    let ent_cull_dist = entity_cull_distance(*render_distance);

                    self.entity_shadow_pipeline
                        .draw(&mut pass, frame, shadows, anchor, eye);

                    self.entity_renderer.draw(
                        &mut pass,
                        frame,
                        entities,
                        &ent_frustum,
                        anchor,
                        eye,
                        ent_cull_dist,
                    );

                    self.block_entity_pipeline
                        .draw(&mut pass, frame, anchor, block_entities);

                    self.item_entity_pipeline.draw(
                        &self.ctx.device,
                        &mut pass,
                        frame,
                        item_entities,
                    );

                    // Break particles draw after entities but before translucent
                    // water: they write depth, and pomme's water doesn't, so this
                    // lets water blend over particles behind it (vanilla draws
                    // particles after all translucents into a depth-sharing
                    // target).
                    self.particle_pipeline.update_and_draw(
                        &mut pass,
                        frame,
                        &self.camera,
                        particles,
                    );

                    // Translucent water draws after opaque terrain and entities so it
                    // blends over them; depth-tested (occluded by geometry in front)
                    // but doesn't write depth. CPU frustum-culled, reusing the entity
                    // frustum/eye.
                    self.chunk_pipeline.bind_water(&mut pass, frame);
                    self.chunk_buffers
                        .draw_water(&pass, &ent_frustum, anchor, eye);

                    // Clouds draw after opaque world geometry (so terrain occludes
                    // them) and before weather, depth-tested against the scene.
                    if !*eyes_in_water {
                        self.cloud_pipeline.update_and_draw(
                            &mut pass,
                            frame,
                            &self.camera,
                            sky,
                            *cloud_mode,
                        );
                    }

                    // Weather draws after opaque world geometry (depth-tested against
                    // terrain) but before the depth clear for the hand pass.
                    self.weather_pipeline.update_and_draw(
                        &mut pass,
                        frame,
                        &self.camera,
                        sky,
                        weather,
                    );

                    if *show_chunk_borders {
                        self.chunk_border_pipeline.draw(&mut pass, frame);
                    }
                    cull_ms
                };

                pass.clear_depth(scissor);

                if *render_first_person_hand
                    && self.camera.mode == camera::CameraMode::FirstPerson
                    && self.camera.top_down().is_none()
                {
                    let aspect = sw / sh.max(1.0);
                    let hud_fov = self.camera.hud_fov_radians();
                    // Vanilla applies bobHurt (death + hurt) and bobView to the
                    // first-person arm/item pose stack as well as the world.
                    let view_effect = self.camera.view_effect_matrix();
                    let hand_pose = self.camera.hand_pose();
                    // Vanilla submitArmWithItem draws the bare arm only for an
                    // empty main hand; held items render alone, and an empty
                    // offhand draws nothing. The main arm is the options' one,
                    // which the server echoes back as `getMainArm`.
                    // TODO: map (`renderOneHandedMap`/`renderTwoHandedMap`)
                    // and the crossbow loading pose, once crossbow use is
                    // tracked.
                    if shown_hands.main && held_items[0].is_none() {
                        self.hand_pipeline.update_and_draw(
                            &mut pass,
                            frame,
                            aspect,
                            hud_fov,
                            swing_progress[0],
                            *main_hand_right,
                            view_effect,
                            hand_pose,
                            *hand_light,
                            sky.cardinal_light,
                        );
                    }
                    let shown = [shown_hands.main, shown_hands.off];
                    let hands: Vec<_> = (0..2)
                        .filter(|&i| shown[i])
                        .filter_map(|i| {
                            Some(HeldHand {
                                item: held_items[i].as_ref()?,
                                main_hand: i == 0,
                                right_arm: *main_hand_right == (i == 0),
                                swing_progress: swing_progress[i],
                                use_anim: use_anim[i],
                            })
                        })
                        .collect();
                    if !hands.is_empty() {
                        self.held_item_pipeline.update_and_draw(
                            &self.ctx.device,
                            &mut pass,
                            frame,
                            aspect,
                            hud_fov,
                            &hands,
                            &self.item_entity_pipeline,
                            view_effect,
                            hand_pose,
                            *hand_light,
                            sky.cardinal_light,
                        );
                    }
                }

                // Vanilla blits the outline after the hand, before the GUI.
                if self.entity_renderer.has_outlines() || self.item_entity_pipeline.has_outlines() {
                    pass.end();
                    let mut outline = self.entity_outline.begin(&self.ctx.device, &encoder);
                    self.entity_renderer.draw_outlines(&mut outline, frame);
                    self.item_entity_pipeline.draw_outlines(&mut outline, frame);
                    self.entity_outline
                        .finish(&self.ctx.device, &encoder, outline);
                    pass = self.resume_frame_pass(&encoder, image_index);
                    self.entity_outline.composite(&mut pass);
                }

                // Vanilla `applyPostEffects`, straight after the outline.
                if self.post_effects.effect().is_some() {
                    pass.end();
                    self.post_effects.apply(
                        &self.ctx.device,
                        &encoder,
                        self.swapchain.image(image_index),
                    );
                    pass = self.resume_frame_pass(&encoder, image_index);
                    self.post_effects.finish(&mut pass);
                }

                let (blurred_pass, vbase, overlay) = self.draw_until_blur(
                    &encoder,
                    image_index,
                    frame,
                    pass,
                    overlay,
                    &item_atlas_uvs,
                );
                pass = blurred_pass;
                self.menu_pipeline.draw_from(
                    &self.ctx.device,
                    &mut pass,
                    frame,
                    sw,
                    sh,
                    overlay,
                    &item_atlas_uvs,
                    vbase,
                );

                // Each preview box gets its depth cleared and its own scissor
                // while the 3D content draws.
                if let Some(p) = player_preview
                    && let Some(rect) = preview_box_rect(p.rect, self.swapchain.extent)
                {
                    pass.clear_depth(rect);
                    pass.set_scissor_rect(rect);
                    self.skin_preview.draw_in_box(
                        &mut pass,
                        frame,
                        p,
                        sw,
                        sh,
                        &self.entity_renderer,
                    );
                    pass.set_scissor_rect(scissor);
                }

                if let Some(p) = book_preview
                    && let Some(rect) = preview_box_rect(p.rect, self.swapchain.extent)
                {
                    pass.clear_depth(rect);
                    pass.set_scissor_rect(rect);
                    self.book_preview.draw_in_box(&mut pass, frame, *p, sw, sh);
                    pass.set_scissor_rect(scissor);
                }

                self.last_timings.cull_ms = cull_ms;
                self.last_timings.frame_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
            }
            RenderMode::MainMenu {
                scroll,
                elements,
                cursor,
                skin,
            } => {
                let aspect = sw / sh.max(1.0);
                self.panorama_pipeline
                    .draw(&mut pass, frame, *scroll, aspect, 0.0);

                let (blurred_pass, vbase, fg) = self.draw_until_blur(
                    &encoder,
                    image_index,
                    frame,
                    pass,
                    elements,
                    &item_atlas_uvs,
                );
                pass = blurred_pass;

                if let Some(skin) = *skin {
                    self.skin_preview.draw(
                        &mut pass, frame, aspect, skin.at.0, skin.at.1, skin.scale, cursor.0,
                        cursor.1, sw, sh,
                    );
                }

                self.menu_pipeline.draw_from(
                    &self.ctx.device,
                    &mut pass,
                    frame,
                    sw,
                    sh,
                    fg,
                    &item_atlas_uvs,
                    vbase,
                );
            }
        }

        pass.end();
        self.swapchain.ready_to_present(&encoder, image_index);

        // The image is ready to present; grab it first if F2 was pressed.
        self.screenshot.record_if_armed(
            &self.ctx.device,
            &encoder,
            frame,
            self.swapchain.image(image_index),
            self.swapchain.extent,
            self.swapchain.is_bgra(),
        );

        self.gui_item_atlas.end_frame();

        let t_present = std::time::Instant::now();
        match self
            .frames
            .submit_and_present(encoder, &self.ctx, &self.swapchain, image_index)?
        {
            rhi::Presented::Done => {}
            rhi::Presented::OutOfDate => self.swapchain_dirty = true,
            rhi::Presented::Suboptimal => {
                self.swapchain_dirty |= self.swapchain.surface_resized(&self.ctx)
            }
        }
        let present_ms = t_present.elapsed().as_secs_f32() * 1000.0;
        self.last_timings.fence_ms = fence_ms;
        self.last_timings.acquire_ms = acquire_ms;
        self.last_timings.present_ms = present_ms;
        Ok(())
    }
}

fn build_gui_item_atlas(
    device: &rhi::backend::Device,
    menu: &mut pipelines::menu_overlay::MenuOverlayPipeline,
    slot_px: u32,
) -> pipelines::gui_item_atlas::GuiItemAtlas {
    let atlas = pipelines::gui_item_atlas::GuiItemAtlas::new(device, slot_px);
    menu.set_item_atlas(device, atlas.color_texture(), atlas.sampler());
    atlas
}

fn warm_item_meshes(
    device: &rhi::backend::Device,
    item_entity_pipeline: &mut ItemEntityPipeline,
    uv_map: &chunk::atlas::AtlasUVMap,
    render_assets: &BlockRenderAssets,
) {
    for name in render_assets.item_names() {
        if let Some(model) = render_assets.get_item_model(name) {
            item_entity_pipeline.ensure_mesh(device, name, model, uv_map);
        } else {
            item_entity_pipeline.ensure_flat_mesh(
                device,
                name,
                &render_assets.flat_item_layers(name),
                uv_map,
            );
        }
    }
}

/// Decoded skin ready for upload: always a 64x64 RGBA sheet (legacy 64x32
/// skins are converted), plus the profile's arm model.
pub(crate) struct SkinData {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub slim: bool,
}

pub(crate) async fn fetch_skin_texture_by_name(name: &str) -> Result<SkinData, String> {
    #[derive(serde::Deserialize)]
    struct NamedProfile {
        id: String,
    }

    // The name goes in a path segment, so it is checked and encoded rather
    // than pasted into the URL.
    if !crate::player::valid_player_name(name) {
        return Err(format!("invalid player name {name:?}"));
    }
    let mut url = reqwest::Url::parse("https://api.mojang.com/users/profiles/minecraft/")
        .map_err(|e| e.to_string())?;
    url.path_segments_mut()
        .map_err(|()| "profile url cannot take a path".to_owned())?
        .pop_if_empty()
        .push(name);
    let response = crate::net::http::get(url)
        .await
        .map_err(pomme_auth::error_chain)?;
    if matches!(
        response.status(),
        reqwest::StatusCode::NO_CONTENT | reqwest::StatusCode::NOT_FOUND
    ) {
        return Err(format!("no profile for {name}"));
    }
    let profile: NamedProfile = response
        .error_for_status()
        .map_err(pomme_auth::error_chain)?
        .json()
        .await
        .map_err(pomme_auth::error_chain)?;
    fetch_skin_texture(&profile.id).await
}

pub(crate) async fn fetch_skin_texture(uuid: &str) -> Result<SkinData, String> {
    #[derive(serde::Deserialize)]
    struct SessionProfile {
        properties: Vec<ProfileProperty>,
    }
    #[derive(serde::Deserialize)]
    struct ProfileProperty {
        name: Option<String>,
        value: String,
    }

    let url = format!("https://sessionserver.mojang.com/session/minecraft/profile/{uuid}");
    let response = crate::net::http::get(&url)
        .await
        .map_err(pomme_auth::error_chain)?;
    if matches!(
        response.status(),
        reqwest::StatusCode::NO_CONTENT | reqwest::StatusCode::NOT_FOUND
    ) {
        return Err(format!("no profile for {uuid}"));
    }
    let profile: SessionProfile = response
        .error_for_status()
        .map_err(pomme_auth::error_chain)?
        .json()
        .await
        .map_err(pomme_auth::error_chain)?;

    let value = &profile
        .properties
        .iter()
        .find(|p| p.name.as_deref() == Some("textures"))
        // TODO: authlib reads only the `textures` property; drop this fallback.
        .or_else(|| profile.properties.first())
        .ok_or("No properties")?
        .value;

    fetch_skin_texture_from_profile_property(value).await
}

pub(crate) async fn fetch_skin_texture_from_profile_property(
    value: &str,
) -> Result<SkinData, String> {
    let (skin_url, slim) = skin_url_from_texture_property(value)?;
    let (pixels, width, height) = fetch_skin_image(&skin_url).await?;
    Ok(SkinData {
        pixels,
        width,
        height,
        slim,
    })
}

fn skin_url_from_texture_property(value: &str) -> Result<(String, bool), String> {
    #[derive(serde::Deserialize)]
    struct TexturesPayload {
        textures: Textures,
    }
    struct Textures(Vec<(String, Texture)>);
    #[derive(serde::Deserialize)]
    struct Texture {
        url: Option<String>,
        metadata: Option<SkinMetadata>,
    }
    #[derive(serde::Deserialize)]
    struct SkinMetadata {
        model: Option<String>,
    }

    /// Gson's map adapter fails on a repeated key. Keys are authlib's texture
    /// `Type`, so unknown names all collide as `null`.
    impl<'de> serde::Deserialize<'de> for Textures {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Textures;

                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a textures map")
                }

                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Textures, A::Error> {
                    fn slot(key: &str) -> Option<&str> {
                        matches!(key, "SKIN" | "CAPE" | "ELYTRA").then_some(key)
                    }
                    let mut entries: Vec<(String, Texture)> = Vec::new();
                    while let Some((key, texture)) = map.next_entry::<String, Texture>()? {
                        if entries.iter().any(|(k, _)| slot(k) == slot(&key)) {
                            return Err(serde::de::Error::custom(format!("duplicate key: {key}")));
                        }
                        entries.push((key, texture));
                    }
                    Ok(Textures(entries))
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(value))
        .map_err(pomme_auth::error_chain)?;
    let payload: TexturesPayload =
        serde_json::from_slice(&decoded).map_err(pomme_auth::error_chain)?;

    // authlib `unpackTextures`: one bad url empties the whole payload.
    // TODO: vanilla also shows the default skin for other players whose
    // textures property isn't SIGNED (`PlayerInfo.createSkinLookup`); pomme
    // doesn't verify the signature yet.
    let mut skin = None;
    for (key, texture) in payload.textures.0 {
        match texture.url {
            Some(url) if is_allowed_texture_url(&url) => {
                if key == "SKIN" {
                    let slim = texture.metadata.and_then(|m| m.model).as_deref() == Some("slim");
                    skin = Some((url, slim));
                }
            }
            url => return Err(format!("texture url not allowed: {url:?}")),
        }
    }
    skin.ok_or_else(|| "No skin texture".to_string())
}

/// authlib's `TextureUrlChecker.isAllowedTextureDomain`: a `java.net.URI`
/// (`parse_untrusted_url`'s rules) with a case-sensitive http(s) scheme and a
/// host of exactly `textures.minecraft.net`.
fn is_allowed_texture_url(url: &str) -> bool {
    const HOST: &str = "textures.minecraft.net";
    if crate::chat_component::parse_untrusted_url(url.to_owned()).is_err() {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = host_port.split_once(':').unwrap_or((host_port, ""));
    // `URI` scans the port as digits, then `Integer.parseInt`; an overflow
    // leaves no host.
    let port_ok = port.is_empty()
        || (port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<i32>().is_ok());
    matches!(scheme, "http" | "https") && host == HOST && port_ok
}

async fn fetch_skin_image(skin_url: &str) -> Result<(Vec<u8>, u32, u32), String> {
    let skin_bytes = crate::net::http::get(skin_url)
        .await
        .map_err(pomme_auth::error_chain)?
        .error_for_status()
        .map_err(pomme_auth::error_chain)?
        .bytes()
        .await
        .map_err(pomme_auth::error_chain)?;

    let img = image::load_from_memory(&skin_bytes).map_err(pomme_auth::error_chain)?;
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let h = rgba.height();
    process_legacy_skin(rgba.into_raw(), w, h)
}

const SKIN_W: u32 = 64;

fn skin_px(x: u32, y: u32) -> usize {
    ((y * SKIN_W + x) * 4) as usize
}

/// `SkinTextureDownloader.processLegacySkin`: rejects bad sizes, upgrades
/// legacy 64x32 sheets to 64x64 by mirroring the right limbs into the modern
/// left-limb slots, and applies the alpha fixups.
fn process_legacy_skin(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    if width != 64 || (height != 32 && height != 64) {
        return Err(format!(
            "Discarding incorrectly sized ({width}x{height}) skin texture"
        ));
    }
    let legacy = height == 32;
    let mut img = if legacy {
        let mut full = vec![0u8; (SKIN_W * SKIN_W * 4) as usize];
        full[..pixels.len()].copy_from_slice(&pixels);
        full
    } else {
        pixels
    };

    if legacy {
        // (x, y, dx, dy, w, h): mirror the right leg/arm into the left slots.
        const COPIES: [(u32, u32, i32, i32, u32, u32); 12] = [
            (4, 16, 16, 32, 4, 4),
            (8, 16, 16, 32, 4, 4),
            (0, 20, 24, 32, 4, 12),
            (4, 20, 16, 32, 4, 12),
            (8, 20, 8, 32, 4, 12),
            (12, 20, 16, 32, 4, 12),
            (44, 16, -8, 32, 4, 4),
            (48, 16, -8, 32, 4, 4),
            (40, 20, 0, 32, 4, 12),
            (44, 20, -8, 32, 4, 12),
            (48, 20, -16, 32, 4, 12),
            (52, 20, -8, 32, 4, 12),
        ];
        for (x, y, dx, dy, w, h) in COPIES {
            copy_rect_mirrored(&mut img, x, y, dx, dy, w, h);
        }
    }
    set_no_alpha(&mut img, 0, 0, 32, 16);
    if legacy {
        strip_alpha_if_opaque(&mut img, 32, 0, 64, 32);
    }
    set_no_alpha(&mut img, 0, 16, 64, 32);
    set_no_alpha(&mut img, 16, 48, 48, 64);
    Ok((img, SKIN_W, SKIN_W))
}

/// `NativeImage.copyRect(x, y, dx, dy, w, h, true, false)`: copies the rect at
/// (x, y) to (x + dx, y + dy) with each row written right-to-left.
fn copy_rect_mirrored(img: &mut [u8], x: u32, y: u32, dx: i32, dy: i32, w: u32, h: u32) {
    for row in 0..h {
        for col in 0..w {
            let src = skin_px(x + col, y + row);
            let dst_x = (x as i32 + dx) as u32 + (w - 1 - col);
            let dst_y = (y as i32 + dy) as u32 + row;
            let dst = skin_px(dst_x, dst_y);
            img.copy_within(src..src + 4, dst);
        }
    }
}

/// Forces the rect fully opaque (base layer regions never carry transparency).
fn set_no_alpha(img: &mut [u8], x0: u32, y0: u32, x1: u32, y1: u32) {
    for y in y0..y1 {
        for x in x0..x1 {
            img[skin_px(x, y) + 3] = 0xFF;
        }
    }
}

/// The "Notch transparency hack": a fully opaque hat region predates hat
/// transparency, so strip its alpha entirely instead of drawing a solid box.
fn strip_alpha_if_opaque(img: &mut [u8], x0: u32, y0: u32, x1: u32, y1: u32) {
    let all_opaque = (y0..y1).all(|y| (x0..x1).all(|x| img[skin_px(x, y) + 3] >= 128));
    if all_opaque {
        for y in y0..y1 {
            for x in x0..x1 {
                img[skin_px(x, y) + 3] = 0;
            }
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.ctx.device.wait_idle();

        self.screenshot.destroy(&self.ctx.device);
        if let Some((_, Some(mut pack))) = self.shader_pack.take() {
            pack.destroy(&self.ctx.device);
        }
        self.chunk_buffers.destroy(&self.ctx.device);
        self.chunk_pipeline.destroy(&self.ctx.device);
        self.lightmap.destroy(&self.ctx.device);
        self.hand_pipeline.destroy(&self.ctx.device);
        self.block_overlay_pipeline.destroy(&self.ctx.device);
        self.sky_pipeline.destroy(&self.ctx.device);
        self.panorama_pipeline.destroy(&self.ctx.device);
        self.menu_pipeline.destroy(&self.ctx.device);

        self.entity_outline.destroy(&self.ctx.device);
        self.post_effects.destroy(&self.ctx.device);
        self.skin_preview.destroy(&self.ctx.device);
        self.book_preview.destroy(&self.ctx.device);
        self.entity_renderer.destroy(&self.ctx.device);
        self.entity_shadow_pipeline.destroy(&self.ctx.device);
        self.moving_block_buffers.destroy(&self.ctx.device);
        self.block_entity_pipeline.destroy(&self.ctx.device);
        self.chunk_border_pipeline.destroy(&self.ctx.device);
        self.item_entity_pipeline.destroy(&self.ctx.device);
        self.held_item_pipeline.destroy(&self.ctx.device);
        self.weather_pipeline.destroy(&self.ctx.device);
        self.particle_pipeline.destroy(&self.ctx.device);
        self.cloud_pipeline.destroy(&self.ctx.device);
        self.gui_item_pipeline.destroy(&self.ctx.device);
        self.gui_item_atlas.destroy(&self.ctx.device);
        self.atlas.destroy(&self.ctx.device);

        self.frames.destroy(&self.ctx.device);

        self.swapchain.destroy(&self.ctx);
    }
}

/// Entities aren't sent beyond the server's tracking range; a generous
/// render-distance cap just trims anything stray.
fn entity_cull_distance(render_distance: u32) -> f32 {
    (render_distance * 16) as f32 + 16.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_skin_url_from_textures_property() {
        let classic = textures_property(
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/testskin"}}}"#,
        );
        let slim = textures_property(
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/testskin","metadata":{"model":"slim"}}}}"#,
        );
        for (value, is_slim) in [
            (classic.as_str(), false),
            (classic.trim_end_matches('='), false),
            (slim.as_str(), true),
        ] {
            assert_eq!(
                skin_url_from_texture_property(value).unwrap(),
                (
                    "https://textures.minecraft.net/texture/testskin".to_owned(),
                    is_slim
                ),
                "{value}"
            );
        }
    }

    fn textures_property(payload: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(payload)
    }

    #[test]
    fn texture_urls_follow_authlib_allow_list() {
        for ok in [
            "https://textures.minecraft.net/texture/abc",
            "http://textures.minecraft.net/texture/abc",
            "https://textures.minecraft.net:443/texture/abc",
        ] {
            assert!(is_allowed_texture_url(ok), "{ok}");
        }
        for bad in [
            "https://example.com/texture/abc",
            "https://textures.minecraft.net.evil.com/abc",
            "https://evil.com/textures.minecraft.net",
            "https://textures.minecraft.net@evil.com/abc",
            "https://evil.com#@textures.minecraft.net",
            "https://TEXTURES.minecraft.net/texture/abc",
            "HTTPS://textures.minecraft.net/texture/abc",
            "ftp://textures.minecraft.net/texture/abc",
            "file:///etc/passwd",
            "http://192.168.0.1/skin.png",
            "https://textures.minecraft.net/texture/a b",
            "https://textures.minecraft.net/texture/a|b",
            r"https://textures.minecraft.net\@evil.com/abc",
            "https://textures.minecraft.net:abc/texture/abc",
            "https://textures.minecraft.net:99999999999/texture/abc",
            "not a url",
            "",
        ] {
            assert!(!is_allowed_texture_url(bad), "{bad}");
        }
    }

    #[test]
    fn foreign_urls_and_duplicate_keys_reject_the_whole_payload() {
        for payload in [
            r#"{"textures":{"SKIN":{"url":"http://192.168.0.1/skin.png"}}}"#,
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a"},"CAPE":{"url":"http://192.168.0.1/cape.png"}}}"#,
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a"},"ELYTRA":{}}}"#,
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a"},"SKIN":{"url":"https://textures.minecraft.net/texture/b"}}}"#,
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a"},"FOO":{"url":"https://textures.minecraft.net/texture/b"},"BAR":{"url":"https://textures.minecraft.net/texture/c"}}}"#,
        ] {
            let value = textures_property(payload);
            assert!(skin_url_from_texture_property(&value).is_err(), "{payload}");
        }
    }

    fn set_px(img: &mut [u8], x: u32, y: u32, rgba: [u8; 4]) {
        img[skin_px(x, y)..skin_px(x, y) + 4].copy_from_slice(&rgba);
    }

    fn get_px(img: &[u8], x: u32, y: u32) -> [u8; 4] {
        img[skin_px(x, y)..skin_px(x, y) + 4].try_into().unwrap()
    }

    #[test]
    fn rejects_incorrectly_sized_skins() {
        assert!(process_legacy_skin(vec![0; 128 * 128 * 4], 128, 128).is_err());
        assert!(process_legacy_skin(vec![0; 64 * 16 * 4], 64, 16).is_err());
    }

    #[test]
    fn passes_64x64_skins_through() {
        let mut img = vec![0u8; 64 * 64 * 4];
        set_px(&mut img, 20, 50, [1, 2, 3, 200]);
        let (out, w, h) = process_legacy_skin(img, 64, 64).unwrap();
        assert_eq!((w, h), (64, 64));
        // Base region alpha is forced opaque, rgb untouched.
        assert_eq!(get_px(&out, 20, 50), [1, 2, 3, 255]);
    }

    #[test]
    fn converts_legacy_skins_to_64x64() {
        let mut img = vec![0u8; 64 * 32 * 4];
        // Right leg front, top-left pixel: (4, 20).
        set_px(&mut img, 4, 20, [10, 20, 30, 255]);
        // Right arm front, top-left pixel: (44, 20).
        set_px(&mut img, 44, 20, [40, 50, 60, 255]);
        let (out, w, h) = process_legacy_skin(img, 64, 32).unwrap();
        assert_eq!((w, h), (64, 64));

        // copyRect(4, 20, 16, 32, 4, 12, mirrored): left leg front spans
        // x 20..24, and mirroring puts the source's left edge on the right.
        assert_eq!(get_px(&out, 23, 52), [10, 20, 30, 255]);
        // copyRect(44, 20, -8, 32, 4, 12, mirrored): left arm front x 36..40.
        assert_eq!(get_px(&out, 39, 52), [40, 50, 60, 255]);
    }

    #[test]
    fn strips_alpha_of_fully_opaque_legacy_hat() {
        let mut img = vec![0u8; 64 * 32 * 4];
        for y in 0..32 {
            for x in 32..64 {
                set_px(&mut img, x, y, [9, 9, 9, 255]);
            }
        }
        let (out, _, _) = process_legacy_skin(img, 64, 32).unwrap();
        // A fully opaque hat region predates hat transparency: alpha stripped.
        assert_eq!(get_px(&out, 40, 8)[3], 0);
    }
}
