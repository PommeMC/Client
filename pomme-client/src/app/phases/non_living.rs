//! Each frame's draws for the non-living entities
//! ([`crate::entity::non_living`]), as their vanilla renderers'
//! `extractRenderState` and `submit` build them: arrows and tridents through
//! the entity model pipeline, thrown items, fireworks and item frames through
//! the item one, paintings and orbs as custom geometry.

use std::sync::Arc;

use glam::{DVec3, Mat3, Mat4, Vec3};
use pomme_registry::builtin::EntityKind;

use crate::entity::components::Position;
use crate::entity::dimensions::EntityDimensions;
use crate::entity::light::entity_light;
use crate::entity::non_living::{self, NonLivingEntity, PaintingShape, is_arrow, is_item_frame};
use crate::physics::aabb::Aabb;
use crate::renderer::Renderer;
use crate::renderer::chunk::block_lighter::{self, LightLevels, WorldLight};
use crate::renderer::pipelines::custom_geometry::{CustomQuad, CustomTexture, CustomVertex};
use crate::renderer::pipelines::entity_renderer::EntityRenderInfo;
use crate::renderer::pipelines::entity_shadow::ShadowCaster;
use crate::renderer::pipelines::item_entity::ItemRenderInfo;

/// The draws one frame's non-living entities add.
#[derive(Default)]
pub struct NonLivingRenders {
    pub entities: Vec<EntityRenderInfo>,
    pub items: Vec<ItemRenderInfo>,
    pub quads: Vec<CustomQuad>,
    /// `ExperienceOrbRenderer`'s shadow casters.
    pub shadows: Vec<ShadowCaster>,
}

/// `ExperienceOrbRenderer`'s `shadowRadius` and `shadowStrength`.
const ORB_SHADOW: (f32, f32) = (0.15, 0.75);

pub struct Frame<'a> {
    pub world_light: &'a WorldLight<'a>,
    pub camera: DVec3,
    /// Planes relative to `camera`.
    pub frustum: [[f32; 4]; 6],
    pub anchor: DVec3,
    pub orientation: Mat3,
    pub partial: f32,
    /// The glowing outline's colour for an entity, by its glowing flag and
    /// UUID.
    pub outline: &'a dyn Fn(bool, uuid::Uuid) -> Option<u32>,
}

pub fn build(
    store: &non_living::NonLivingStore,
    renderer: &mut Renderer,
    frame: &Frame<'_>,
) -> NonLivingRenders {
    let mut out = NonLivingRenders::default();
    for (_, e) in store.entities() {
        let position = e.lerped_position(frame.partial);
        push_entity(&mut out, renderer, frame, e, position, frame.partial, true);
    }
    // `ItemPickupParticle`: the state extracted at the pickup, partial 1.
    for pickup in store.pickups() {
        let position = pickup.position(frame.partial);
        push_entity(
            &mut out,
            renderer,
            frame,
            &pickup.entity,
            position,
            1.0,
            false,
        );
    }
    out
}

fn push_entity(
    out: &mut NonLivingRenders,
    renderer: &mut Renderer,
    frame: &Frame<'_>,
    e: &NonLivingEntity,
    position: Position,
    partial: f32,
    outlined: bool,
) {
    let outline = if outlined {
        (frame.outline)(e.is_glowing(), e.uuid)
    } else {
        None
    };
    let light = || {
        let eye = position + DVec3::Y * f64::from(EntityDimensions::eye_height(e.kind));
        entity_light(frame.world_light, e.kind, eye, e.is_on_fire(), 0)
    };
    let relative = (*position - frame.anchor).as_vec3();
    // TODO: a burning entity's flame (`EntityRenderDispatcher.submit`'s
    // `submitFlame`), which every fireball shows: `AbstractHurtingProjectile`
    // sets itself alight each tick. Pomme draws no entity fire yet.
    if let Some(style) = non_living::thrown_item_style(e.kind) {
        // `ThrowableProjectile.shouldRenderAtSqrDistance`'s first two ticks,
        // and `Fireball`'s and `EyeOfEnder`'s copies of it.
        if e.tick_count >= 2 || position.distance_squared(frame.camera) >= 12.25 {
            let pose = Mat4::from_translation(relative)
                * Mat4::from_scale(Vec3::splat(style.scale))
                * Mat4::from_mat3(frame.orientation);
            push_ground_item(out, renderer, e, pose, light(), outline);
        }
        return;
    }
    match e.kind {
        kind if is_arrow(kind) => out
            .entities
            .push(arrow(e, position, partial, outline, light())),
        EntityKind::FireworkRocket => {
            // `FireworkRocketEntity.shouldRender`: never while boosting a flyer.
            if e.attached_to_entity {
                return;
            }
            let mut pose = Mat4::from_translation(relative) * Mat4::from_mat3(frame.orientation);
            if e.shot_at_angle {
                pose *= Mat4::from_rotation_z(std::f32::consts::PI)
                    * Mat4::from_rotation_y(std::f32::consts::PI)
                    * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2);
            }
            push_ground_item(out, renderer, e, pose, light(), outline);
        }
        kind if is_item_frame(kind) => item_frame(out, renderer, e, relative, light(), outline),
        EntityKind::ExperienceOrb => {
            if !should_render(frame, *position, Aabb::from_center(*position, 0.25, 0.25)) {
                return;
            }
            out.quads
                .push(orb(e, relative, frame.orientation, partial, light()));
            if outlined {
                out.shadows.push(ShadowCaster {
                    position: *position,
                    radius: ORB_SHADOW.0,
                    strength: ORB_SHADOW.1,
                    ..Default::default()
                });
            }
        }
        EntityKind::Painting => {
            if let Some(shape) = &e.painting
                && let Some(aabb) = e.bounding_box()
                && should_render(frame, *position, aabb)
                && renderer.has_custom_texture(&CustomTexture::Painting(shape.asset_id.clone()))
            {
                painting(&mut out.quads, frame.world_light, e, shape, relative);
            }
        }
        _ => {}
    }
}

/// `EntityRenderer.shouldRender`: within `Entity.shouldRenderAtSqrDistance`'s
/// reach for the box's size (at the default entity distance), and the box,
/// grown half a block, in the frustum.
fn should_render(frame: &Frame<'_>, position: DVec3, aabb: Aabb) -> bool {
    let reach = (aabb.max - aabb.min).element_sum() / 3.0 * 64.0;
    if position.distance_squared(frame.camera) >= reach * reach {
        return false;
    }
    let corner = |at: DVec3| (at - frame.camera).as_vec3().to_array();
    crate::renderer::chunk::occlusion_graph::aabb_in_frustum(
        &corner(aabb.min - 0.5),
        &corner(aabb.max + 0.5),
        &frame.frustum,
    )
}

/// `ArrowRenderer` and `ThrownTridentRenderer`: yaw - 90 about Y, then the
/// pitch about Z (the trident's a quarter turn further).
fn arrow(
    e: &NonLivingEntity,
    position: Position,
    partial: f32,
    outline_color: Option<u32>,
    light: LightLevels,
) -> EntityRenderInfo {
    let y_rot = e.lerped_y_rot(partial);
    let mut x_rot = e.lerped_x_rot(partial);
    if e.kind == EntityKind::Trident {
        x_rot += 90.0;
    }
    EntityRenderInfo {
        position,
        entity_kind: e.kind,
        // `entity_matrix` turns by 180 - body.
        body_y_rot_deg: 270.0 - y_rot,
        body_transform: Some(Mat4::from_rotation_z(x_rot.to_radians())),
        arrow_shake: e.shake_time as f32 - partial,
        // `TippableArrowRenderer`: a colour above 0 is tipped.
        variant_index: u32::from(e.kind == EntityKind::Arrow && e.arrow_color > 0),
        outline_color,
        light,
        ..Default::default()
    }
}

/// An item submitted under `ItemDisplayContext.GROUND` after `pose`.
fn push_ground_item(
    out: &mut NonLivingRenders,
    renderer: &mut Renderer,
    e: &NonLivingEntity,
    pose: Mat4,
    light: LightLevels,
    outline_color: Option<u32>,
) {
    let Some((name, stack)) = &e.item else {
        return;
    };
    let (model, tints) = renderer.item_draw(name, stack);
    let ground = super::in_game::dropped_item_geometry(renderer, &model).0;
    out.items.push(ItemRenderInfo {
        item_name: model,
        outline_color,
        tints,
        model_matrix: pose * ground,
        light,
    });
}

/// `ItemFrameRenderer.submit`: the frame model against the wall, then the
/// item (a map draws as its item too: pomme keeps no map data).
// TODO: maps in frames (`MapRenderer`, the `map=true` frame model and the
// 1x1 box) once the client keeps `MapItemSavedData`.
fn item_frame(
    out: &mut NonLivingRenders,
    renderer: &mut Renderer,
    e: &NonLivingEntity,
    relative: Vec3,
    light: LightLevels,
    outline_color: Option<u32>,
) {
    let glow = e.kind == EntityKind::GlowItemFrame;
    // The render offset `getRenderOffset` adds is cancelled before the frame.
    // Its turns are the frame's own rotation (`ItemFrame.setDirection`).
    let pose = Mat4::from_translation(relative + e.direction.step().as_vec3() * 0.46875)
        * Mat4::from_rotation_x(e.x_rot.to_radians())
        * Mat4::from_rotation_y((180.0 - e.y_rot).to_radians());
    if !e.is_invisible() {
        let key = crate::world::block::model::item_frame_model_key(glow);
        renderer.ensure_item_mesh(key);
        out.items.push(ItemRenderInfo {
            item_name: key.into(),
            outline_color,
            tints: Arc::from([]),
            model_matrix: pose,
            light,
        });
    }
    let Some((name, stack)) = &e.item else {
        return;
    };
    let depth = if e.is_invisible() { 0.5 } else { 0.4375 };
    let (model, tints) = renderer.item_draw(name, stack);
    let fixed = renderer.item_fixed_transform(name);
    out.items.push(ItemRenderInfo {
        item_name: model,
        outline_color,
        tints,
        model_matrix: pose
            * Mat4::from_translation(Vec3::new(0.0, 0.0, depth))
            * Mat4::from_rotation_z((e.frame_rotation as f32 * 45.0).to_radians())
            * Mat4::from_scale(Vec3::splat(0.5))
            * fixed,
        light: if glow {
            LightLevels::FULL_BRIGHT
        } else {
            light
        },
    });
}

/// `ExperienceOrbRenderer.submit`: one camera-facing quad from the 4x4 icon
/// sheet, its red and blue pulsing with age.
fn orb(
    e: &NonLivingEntity,
    relative: Vec3,
    orientation: Mat3,
    partial: f32,
    light: LightLevels,
) -> CustomQuad {
    let icon = e.xp_icon();
    let u0 = (icon % 4 * 16) as f32 / 64.0;
    let u1 = (icon % 4 * 16 + 16) as f32 / 64.0;
    let v0 = (icon / 4 * 16) as f32 / 64.0;
    let v1 = (icon / 4 * 16 + 16) as f32 / 64.0;
    let age = e.tick_count as f32 + partial;
    let rr = age / 2.0;
    let red = (((rr.sin() + 1.0) * 0.5) * 255.0) as u8;
    let blue = (((rr + 4.188_790_3).sin() + 1.0) * 0.1 * 255.0) as u8;
    let pose = Mat4::from_translation(relative + Vec3::new(0.0, 0.1, 0.0))
        * Mat4::from_mat3(orientation)
        * Mat4::from_scale(Vec3::splat(0.3));
    let normal = crate::renderer::entity_model::pack_normal(orientation * Vec3::Y);
    let vertex = |x: f32, y: f32, u: f32, v: f32| CustomVertex {
        position: pose.transform_point3(Vec3::new(x, y, 0.0)).into(),
        uv: [u, v],
        color: [red, 255, blue, 128],
        normal,
        light: light.lane(),
    };
    CustomQuad {
        texture: CustomTexture::ExperienceOrb,
        translucent: true,
        corners: [
            vertex(-0.5, -0.25, u0, v1),
            vertex(0.5, -0.25, u1, v1),
            vertex(0.5, 0.75, u1, v0),
            vertex(-0.5, 0.75, u0, v0),
        ],
    }
}

/// `PaintingRenderer`: per block of the painting, its front and back, and
/// the edge strips on the outer blocks, each lit from the block it covers.
fn painting(
    quads: &mut Vec<CustomQuad>,
    world_light: &WorldLight<'_>,
    e: &NonLivingEntity,
    shape: &PaintingShape,
    relative: Vec3,
) {
    let (width, height) = (shape.width.max(1), shape.height.max(1));
    let rotation = Mat3::from_rotation_y((180.0 - e.y_rot).to_radians());
    let pose = Mat4::from_translation(relative) * Mat4::from_mat3(rotation);
    let along = e.direction.counter_clockwise().step();
    let light_at = |segment_x: i32, segment_y: i32| {
        // `extractRenderState`'s sample: the block each segment covers, to
        // the painting's right as it faces out.
        let offset_x = segment_x as f64 - width as f64 / 2.0 + 0.5;
        let offset_y = segment_y as f64 - height as f64 / 2.0 + 0.5;
        let at = *e.position - along * offset_x + DVec3::Y * offset_y;
        block_lighter::light_coords(world_light, at.floor().as_ivec3().to_array()).lane()
    };
    const EDGE: f32 = 0.03125;
    // The edges take a one-texel strip of the back sprite.
    const STRIP: f32 = 0.0625;
    let front = CustomTexture::Painting(shape.asset_id.clone());
    let back = CustomTexture::PaintingBack;
    let offset_x = -width as f32 / 2.0;
    let offset_y = -height as f32 / 2.0;
    let (du, dv) = (1.0 / width as f64, 1.0 / height as f64);
    for segment_x in 0..width {
        for segment_y in 0..height {
            let x0 = offset_x + (segment_x + 1) as f32;
            let x1 = offset_x + segment_x as f32;
            let y0 = offset_y + (segment_y + 1) as f32;
            let y1 = offset_y + segment_y as f32;
            let light = light_at(segment_x, segment_y);
            let vertex = |x: f32, y: f32, u: f32, v: f32, z: f32, normal: Vec3| CustomVertex {
                position: pose.transform_point3(Vec3::new(x, y, z)).into(),
                uv: [u, v],
                color: [255; 4],
                normal: crate::renderer::entity_model::pack_normal(rotation * normal),
                light,
            };
            let quad = |texture: &CustomTexture, corners| CustomQuad {
                texture: texture.clone(),
                translucent: false,
                corners,
            };
            let front_u0 = (du * (width - segment_x) as f64) as f32;
            let front_u1 = (du * (width - (segment_x + 1)) as f64) as f32;
            let front_v0 = (dv * (height - segment_y) as f64) as f32;
            let front_v1 = (dv * (height - (segment_y + 1)) as f64) as f32;
            quads.push(quad(
                &front,
                [
                    vertex(x0, y1, front_u1, front_v0, -EDGE, Vec3::NEG_Z),
                    vertex(x1, y1, front_u0, front_v0, -EDGE, Vec3::NEG_Z),
                    vertex(x1, y0, front_u0, front_v1, -EDGE, Vec3::NEG_Z),
                    vertex(x0, y0, front_u1, front_v1, -EDGE, Vec3::NEG_Z),
                ],
            ));
            quads.push(quad(
                &back,
                [
                    vertex(x0, y0, 1.0, 0.0, EDGE, Vec3::Z),
                    vertex(x1, y0, 0.0, 0.0, EDGE, Vec3::Z),
                    vertex(x1, y1, 0.0, 1.0, EDGE, Vec3::Z),
                    vertex(x0, y1, 1.0, 1.0, EDGE, Vec3::Z),
                ],
            ));
            if segment_y == height - 1 {
                quads.push(quad(
                    &back,
                    [
                        vertex(x0, y0, 0.0, 0.0, -EDGE, Vec3::Y),
                        vertex(x1, y0, 1.0, 0.0, -EDGE, Vec3::Y),
                        vertex(x1, y0, 1.0, STRIP, EDGE, Vec3::Y),
                        vertex(x0, y0, 0.0, STRIP, EDGE, Vec3::Y),
                    ],
                ));
            }
            if segment_y == 0 {
                quads.push(quad(
                    &back,
                    [
                        vertex(x0, y1, 0.0, 0.0, EDGE, Vec3::NEG_Y),
                        vertex(x1, y1, 1.0, 0.0, EDGE, Vec3::NEG_Y),
                        vertex(x1, y1, 1.0, STRIP, -EDGE, Vec3::NEG_Y),
                        vertex(x0, y1, 0.0, STRIP, -EDGE, Vec3::NEG_Y),
                    ],
                ));
            }
            if segment_x == width - 1 {
                quads.push(quad(
                    &back,
                    [
                        vertex(x0, y0, STRIP, 0.0, EDGE, Vec3::NEG_X),
                        vertex(x0, y1, STRIP, 1.0, EDGE, Vec3::NEG_X),
                        vertex(x0, y1, 0.0, 1.0, -EDGE, Vec3::NEG_X),
                        vertex(x0, y0, 0.0, 0.0, -EDGE, Vec3::NEG_X),
                    ],
                ));
            }
            if segment_x == 0 {
                quads.push(quad(
                    &back,
                    [
                        vertex(x1, y0, STRIP, 0.0, -EDGE, Vec3::X),
                        vertex(x1, y1, STRIP, 1.0, -EDGE, Vec3::X),
                        vertex(x1, y1, 0.0, 1.0, EDGE, Vec3::X),
                        vertex(x1, y0, 0.0, 0.0, EDGE, Vec3::X),
                    ],
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The orb's sheet cells: column `icon % 4`, row `icon / 4`.
    #[test]
    fn orb_uvs_pick_the_icon_cell() {
        let mut store = non_living::NonLivingStore::new();
        store.spawn(
            1,
            EntityKind::ExperienceOrb,
            uuid::Uuid::nil(),
            Position::new(0.0, 0.0, 0.0),
            DVec3::ZERO,
            0.0,
            0.0,
            0,
        );
        // 37 xp is icon 4: column 0, row 1.
        store.apply_entity_data(1, 8, crate::entity::MetaValue::Int(37));
        let quad = orb(
            store.get(1).unwrap(),
            Vec3::ZERO,
            Mat3::IDENTITY,
            0.0,
            LightLevels::FULL_BRIGHT,
        );
        assert_eq!(quad.corners[0].uv, [0.0, 0.5]);
        assert_eq!(quad.corners[2].uv, [0.25, 0.25]);
        // Age 0: red at its midpoint, blue near its trough, alpha half.
        assert_eq!(quad.corners[0].color, [127, 255, 3, 128]);
    }
}
