use std::collections::{HashMap, HashSet};

use crate::renderer::rhi;

// Target capacity per atlas page in slots. The atlas is sized to fit this many
// at the active slot size, clamped to MIN_ATLAS_PX so very small slots still
// hit the device's framebuffer minimums. MAX_ATLAS_PX is a conservative cap;
// vanilla queries `RenderSystem.getDevice().limits().maxTextureSizeForFormat`.
const TARGET_SLOT_CAPACITY: u32 = 256;
const MIN_ATLAS_PX: u32 = 512;
const MAX_ATLAS_PX: u32 = 4096;

// TODO: vanilla marks an item animated when any of its quads samples an
// animated sprite (`CuboidItemModelWrapper`, material flag 2) and re-renders
// it every frame, so a magma block icon animates; pomme bakes those once.
pub fn is_animated_item(item_name: &str) -> bool {
    matches!(item_name, "compass" | "recovery_compass" | "clock")
}

const COLOR_FORMAT: rhi::TextureFormat = rhi::TextureFormat::Rgba8Unorm;

/// Returns the slot size in pixels matching `16 * gui_scale` at the Auto GUI
/// scale, the vanilla formula (`GuiRenderer.prepareItemElements`). 16 is the
/// base item size in logical GUI units; multiplying by gui_scale makes the bake
/// 1:1 with display.
pub fn slot_px_for_screen(screen_w: f32, screen_h: f32, enforce_unicode: bool) -> u32 {
    (16.0 * crate::ui::hud::gui_scale(screen_w, screen_h, 0, enforce_unicode)).round() as u32
}

/// Picks the smallest power-of-two atlas size that fits TARGET_SLOT_CAPACITY
/// slots of the given size, clamped to [MIN_ATLAS_PX, MAX_ATLAS_PX].
fn atlas_px_for_slot(slot_px: u32) -> u32 {
    let slots_per_side = (TARGET_SLOT_CAPACITY as f32).sqrt().ceil() as u32;
    let needed = slots_per_side * slot_px;
    needed.next_power_of_two().clamp(MIN_ATLAS_PX, MAX_ATLAS_PX)
}

#[derive(Clone, Copy)]
pub enum SlotState {
    Empty,
    Stale,
    Ready,
}

#[derive(Clone, Copy)]
pub struct Slot {
    x: u32,
    y: u32,
}

struct SlotInternal {
    x: u32,
    y: u32,
    fresh: bool,
    discard_after_frame: bool,
}

struct DynamicAtlasAllocator {
    slots: Vec<SlotInternal>,
    used_by_key: HashMap<String, usize>,
    free: Vec<bool>,
}

impl DynamicAtlasAllocator {
    fn new(width: u32, height: u32) -> Self {
        let total = (width * height) as usize;
        let mut slots = Vec::with_capacity(total);
        for y in 0..height {
            for x in 0..width {
                slots.push(SlotInternal {
                    x,
                    y,
                    fresh: true,
                    discard_after_frame: false,
                });
            }
        }
        Self {
            slots,
            used_by_key: HashMap::new(),
            free: vec![true; total],
        }
    }

    fn get_or_allocate(
        &mut self,
        key: &str,
        discard_after_frame: bool,
    ) -> Option<(Slot, SlotState)> {
        if let Some(&idx) = self.used_by_key.get(key) {
            let s = &mut self.slots[idx];
            s.discard_after_frame |= discard_after_frame;
            return Some((Slot { x: s.x, y: s.y }, SlotState::Ready));
        }
        let idx = self.free.iter().position(|f| *f)?;
        self.free[idx] = false;
        let s = &mut self.slots[idx];
        let state = if s.fresh {
            SlotState::Empty
        } else {
            SlotState::Stale
        };
        s.fresh = false;
        s.discard_after_frame = discard_after_frame;
        let slot = Slot { x: s.x, y: s.y };
        self.used_by_key.insert(key.to_string(), idx);
        Some((slot, state))
    }

    fn has_space_for_all(&self, keys: &HashSet<String>) -> bool {
        let mut total = self.used_by_key.len();
        for key in keys {
            if !self.used_by_key.contains_key(key) {
                total += 1;
            }
        }
        total <= self.slots.len()
    }

    fn reclaim_space_for(&mut self, keys: &HashSet<String>) -> bool {
        let preexisting = keys
            .iter()
            .filter(|k| self.used_by_key.contains_key(*k))
            .count();
        if preexisting == keys.len() {
            return true;
        }
        let mut needed = keys.len() - preexisting;
        self.free_slot_if(|key, _| {
            if needed == 0 || keys.contains(key) {
                false
            } else {
                needed -= 1;
                true
            }
        });
        needed == 0
    }

    fn end_frame(&mut self) {
        self.free_slot_if(|_, slot| slot.discard_after_frame);
    }

    fn invalidate_all(&mut self) {
        self.used_by_key.clear();
        self.free.fill(true);
        for slot in &mut self.slots {
            slot.discard_after_frame = false;
        }
    }

    fn free_slot_if(&mut self, mut predicate: impl FnMut(&str, &SlotInternal) -> bool) {
        let to_remove: Vec<String> = self
            .used_by_key
            .iter()
            .filter_map(|(k, &idx)| {
                if predicate(k, &self.slots[idx]) {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        for key in to_remove {
            if let Some(idx) = self.used_by_key.remove(&key) {
                self.free[idx] = true;
                self.slots[idx].discard_after_frame = false;
            }
        }
    }
}

pub struct GuiItemAtlas {
    color: rhi::backend::Texture,
    depth: rhi::backend::Texture,
    sampler: rhi::backend::Sampler,
    slot_px: u32,
    atlas_px: u32,
    allocator: DynamicAtlasAllocator,
}

impl GuiItemAtlas {
    pub fn new(device: &rhi::backend::Device, slot_px: u32) -> Self {
        let atlas_px = atlas_px_for_slot(slot_px);
        let slots_per_side = atlas_px / slot_px;

        let color = rhi::backend::Texture::new(
            device,
            &rhi::TextureDesc::new(
                "gui_item_atlas_color",
                atlas_px,
                atlas_px,
                COLOR_FORMAT,
                rhi::TextureUsages::RENDER_ATTACHMENT
                    | rhi::TextureUsages::TEXTURE_BINDING
                    | rhi::TextureUsages::COPY_DST,
            ),
        );
        let depth = rhi::backend::Texture::new(
            device,
            &rhi::TextureDesc::new(
                "gui_item_atlas_depth",
                atlas_px,
                atlas_px,
                rhi::TextureFormat::Depth32Float,
                rhi::TextureUsages::RENDER_ATTACHMENT,
            ),
        );

        init_color_clear_and_transition(device, color.handle());

        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);

        Self {
            color,
            depth,
            sampler,
            slot_px,
            atlas_px,
            allocator: DynamicAtlasAllocator::new(slots_per_side, slots_per_side),
        }
    }

    pub fn slot_px(&self) -> u32 {
        self.slot_px
    }

    pub fn atlas_px(&self) -> u32 {
        self.atlas_px
    }

    pub fn targets(&self) -> rhi::backend::PipelineTargets {
        rhi::backend::PipelineTargets::new(rhi::backend::texture_format(COLOR_FORMAT), true)
    }

    pub fn get_or_allocate(
        &mut self,
        item_name: &str,
        discard_after_frame: bool,
    ) -> Option<(Slot, SlotState)> {
        self.allocator
            .get_or_allocate(item_name, discard_after_frame)
    }

    pub fn has_space_for_all(&self, keys: &HashSet<String>) -> bool {
        self.allocator.has_space_for_all(keys)
    }

    pub fn reclaim_space_for(&mut self, keys: &HashSet<String>) -> bool {
        self.allocator.reclaim_space_for(keys)
    }

    pub fn end_frame(&mut self) {
        self.allocator.end_frame();
    }

    pub fn invalidate_all(&mut self) {
        self.allocator.invalidate_all();
    }

    /// Top-origin pixel coordinates of the slot's upper-left corner; pair with
    /// `slot_px()` for the size.
    pub fn slot_origin_pixels(&self, slot: &Slot) -> (u32, u32) {
        (slot.x * self.slot_px, slot.y * self.slot_px)
    }

    /// Bottom-origin framebuffer rect for the bake-pass scissor and slot clear.
    pub fn scissor_rect(&self, slot: &Slot) -> rhi::Rect {
        rhi::Rect {
            x: slot.x * self.slot_px,
            y: self.atlas_px - (slot.y + 1) * self.slot_px,
            width: self.slot_px,
            height: self.slot_px,
        }
    }

    pub fn slot_uv(&self, slot: &Slot) -> [f32; 4] {
        // V decreases because the bake projection Y-flips the mesh into the
        // atlas image, so the slot's top-of-mesh lands at the higher V row.
        let step = self.slot_px as f32 / self.atlas_px as f32;
        let u0 = slot.x as f32 * step;
        let v0 = 1.0 - slot.y as f32 * step;
        [u0, v0, u0 + step, v0 - step]
    }

    /// Draws into the atlas over a cleared depth buffer, keeping the slots
    /// already baked. [`Self::end_bake_pass`] hands it back to shaders.
    pub fn begin_bake_pass(
        &self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
    ) -> rhi::backend::RenderPassEncoder {
        self.transition_color(
            encoder,
            rhi::TextureUse::ShaderRead,
            rhi::TextureUse::ColorAttachment,
        );
        let desc = rhi::backend::RenderPassDesc {
            color: rhi::backend::ColorAttachment {
                texture: self.color.handle(),
                format: rhi::backend::texture_format(COLOR_FORMAT),
                load: rhi::LoadOp::Load,
                store: rhi::StoreOp::Store,
            },
            extra_colors: Vec::new(),
            depth: Some(rhi::backend::DepthAttachment::cleared(self.depth.handle())),
            extent: rhi::Extent {
                width: self.atlas_px,
                height: self.atlas_px,
            },
        };
        encoder.begin_render_pass(device, &desc)
    }

    pub fn end_bake_pass(
        &self,
        encoder: &rhi::backend::CommandEncoder,
        pass: rhi::backend::RenderPassEncoder,
    ) {
        pass.end();
        self.transition_color(
            encoder,
            rhi::TextureUse::ColorAttachment,
            rhi::TextureUse::ShaderRead,
        );
    }

    fn transition_color(
        &self,
        encoder: &rhi::backend::CommandEncoder,
        from: rhi::TextureUse,
        to: rhi::TextureUse,
    ) {
        encoder.transition_textures(&[rhi::backend::TextureTransition::color(
            self.color.handle(),
            from,
            to,
        )]);
    }

    pub fn clear_slot_color(&self, pass: &rhi::backend::RenderPassEncoder, slot: &Slot) {
        pass.clear_color(self.scissor_rect(slot), [0.0; 4]);
    }

    pub fn color_texture(&self) -> rhi::backend::TextureHandle {
        self.color.handle()
    }

    pub fn sampler(&self) -> rhi::backend::Sampler {
        self.sampler
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        self.sampler.destroy(device);
        std::mem::take(&mut self.color).destroy(device);
        std::mem::take(&mut self.depth).destroy(device);
    }
}

fn init_color_clear_and_transition(
    device: &rhi::backend::Device,
    image: rhi::backend::TextureHandle,
) {
    device.submit(|encoder| {
        encoder.transition_textures(&[rhi::backend::TextureTransition::color(
            image,
            rhi::TextureUse::Undefined,
            rhi::TextureUse::CopyDst,
        )]);
        encoder.clear_texture(image, rhi::TextureRange::BASE, [0.0; 4]);
        encoder.transition_textures(&[rhi::backend::TextureTransition::color(
            image,
            rhi::TextureUse::CopyDst,
            rhi::TextureUse::ShaderRead,
        )]);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidate_all_releases_cached_slots_as_stale() {
        let mut atlas = DynamicAtlasAllocator::new(2, 1);
        let (_, first_state) = atlas.get_or_allocate("cocoa_beans", false).unwrap();
        assert!(matches!(first_state, SlotState::Empty));
        let (_, ready_state) = atlas.get_or_allocate("cocoa_beans", false).unwrap();
        assert!(matches!(ready_state, SlotState::Ready));

        atlas.invalidate_all();

        let (_, rebake_state) = atlas.get_or_allocate("cocoa_beans", false).unwrap();
        assert!(matches!(rebake_state, SlotState::Stale));
    }
}
