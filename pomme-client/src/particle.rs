//! Legacy terrain/item particles and `ClientLevel.addDestroyBlockEffect`.
//! The retained `ServerParticleKind` maps native network particle IDs.

use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use glam::{DVec3, dvec3};

use crate::physics::aabb::Aabb;
use crate::physics::block_shape::{self, CollisionContext, LocalBox};
use crate::physics::collision::resolve_collision;
use crate::position::{BlockPos, ChunkPos};
use crate::renderer::ParticleQuad;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap};
use crate::renderer::chunk::block_lighter::{self, LightLevels, WorldLight};
use crate::renderer::chunk::mesher::{BiomeClimate, Colormap};
use crate::renderer::pipelines::particle::MAX_PARTICLE_QUADS as MAX_PARTICLES;
use crate::world::block::BlockStateTable;
use crate::world::block::block_colors::BlockColors;
use crate::world::block::particle_appearance::BlockParticleAppearance;
use crate::world::block::render_assets::{BlockRenderView, Tint};
use crate::world::chunk::ChunkStore;

/// Vanilla `ParticleGroup.RESERVOIR_START` — above this, new particles are
/// probabilistically dropped.
const RESERVOIR_START: usize = 12288;
/// Vanilla `Particle.MAXIMUM_COLLISION_VELOCITY_SQUARED` (100²).
const MAX_COLLISION_VELOCITY_SQ: f64 = 10000.0;
/// Terrain particles use the default 0.2-wide, 0.2-tall bounding box.
const HALF_WIDTH: f64 = 0.1;

pub struct Particle {
    /// Bounding-box bottom-center, like vanilla `Particle.setPos`.
    pos: DVec3,
    prev_pos: DVec3,
    vel: DVec3,
    age: i32,
    lifetime: i32,
    on_ground: bool,
    stopped_by_collision: bool,
    /// Vanilla `Particle.gravity` and `friction`.
    gravity: f64,
    friction: f64,
    /// Vanilla `quadSize`; the billboard spans twice this.
    size: f32,
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
    color: [f32; 3],
    alpha: f32,
}

impl Particle {
    /// Vanilla `TerrainParticle` constructor chain: velocity jitter +
    /// normalization from `Particle(level, x, y, z, xa, ya, za)`, then the
    /// terrain quad size and random quarter sub-tile.
    fn terrain(pos: DVec3, velocity_arg: DVec3, region: AtlasRegion, color: [f32; 3]) -> Self {
        let jitter = || (fastrand::f64() * 2.0 - 1.0) * 0.4;
        let dir = velocity_arg + DVec3::new(jitter(), jitter(), jitter());
        let speed = (fastrand::f64() + fastrand::f64() + 1.0) * 0.15;
        let mut vel = dir / dir.length() * speed * 0.4;
        vel.y += 0.1;

        let lifetime = (4.0 / (fastrand::f32() * 0.9 + 0.1)) as i32;
        // SingleQuadParticle base size, halved by TerrainParticle.
        let size = 0.1 * (fastrand::f32() * 0.5 + 0.5) * 2.0 / 2.0;

        // Random quarter sub-tile of the block sprite. The u0/u1 flip is
        // vanilla (`getU0` samples uo+1, `getU1` samples uo).
        let sprite_u = |f: f32| region.u_min + f * (region.u_max - region.u_min);
        let sprite_v = |f: f32| region.v_min + f * (region.v_max - region.v_min);
        let uo = fastrand::f32() * 3.0;
        let vo = fastrand::f32() * 3.0;

        Self {
            pos,
            prev_pos: pos,
            vel,
            age: 0,
            lifetime,
            on_ground: false,
            stopped_by_collision: false,
            gravity: 1.0,
            friction: 0.98,
            size,
            u0: sprite_u((uo + 1.0) / 4.0),
            u1: sprite_u(uo / 4.0),
            v0: sprite_v(vo / 4.0),
            v1: sprite_v((vo + 1.0) / 4.0),
            color,
            alpha: 1.0,
        }
    }

    /// Vanilla `getLightCoords`, read each frame at the current position.
    fn light(&self, world: &WorldLight) -> LightLevels {
        // `Particle.getLightCoords`: sky light alone outside loaded chunks.
        let at = self.pos.floor().as_ivec3();
        let loaded = world
            .chunks
            .get_chunk(&ChunkPos::new(at.x.div_euclid(16), at.z.div_euclid(16)))
            .is_some();
        if loaded {
            block_lighter::light_coords(world, at.to_array())
        } else {
            LightLevels::new(15, 0)
        }
    }

    /// Vanilla `BreakingItemParticle`: the base-constructor velocity (zero
    /// argument, jitter only) scaled to 10%, plus the spawn velocity. Shares
    /// the halved quad size and quarter sub-tile sampling with terrain.
    fn breaking_item(pos: DVec3, velocity: DVec3, region: AtlasRegion) -> Self {
        let mut p = Self::terrain(pos, DVec3::ZERO, region, [1.0; 3]);
        p.vel = p.vel * 0.1 + velocity;
        p
    }

    /// Vanilla `Particle.tick`. Returns false when the particle expires.
    fn tick(&mut self, chunks: &ChunkStore, block_state_table: BlockStateTable) -> bool {
        self.prev_pos = self.pos;
        if self.age >= self.lifetime {
            return false;
        }
        self.age += 1;
        self.vel.y -= 0.04 * self.gravity;
        self.move_with_collision(chunks, block_state_table);
        self.vel *= self.friction;
        if self.on_ground {
            self.vel.x *= 0.7;
            self.vel.z *= 0.7;
        }
        true
    }

    /// Vanilla `Particle.move`.
    fn move_with_collision(&mut self, chunks: &ChunkStore, block_state_table: BlockStateTable) {
        if self.stopped_by_collision {
            return;
        }
        let orig = self.vel;
        let mut delta = orig;
        if delta != DVec3::ZERO && delta.length_squared() < MAX_COLLISION_VELOCITY_SQ {
            let aabb = Aabb::from_center(self.pos, HALF_WIDTH, HALF_WIDTH);
            (delta, _) = resolve_collision(
                crate::physics::collision::CollisionWorld::new(chunks, block_state_table),
                aabb,
                orig.into(),
                0.0,
                false,
                &CollisionContext::position(self.pos.y),
            );
        }
        self.pos += delta;
        if orig.y.abs() >= 1e-5 && delta.y.abs() < 1e-5 {
            self.stopped_by_collision = true;
        }
        self.on_ground = orig.y != delta.y && orig.y < 0.0;
        if orig.x != delta.x {
            self.vel.x = 0.0;
        }
        if orig.z != delta.z {
            self.vel.z = 0.0;
        }
    }
}

/// Server-sent particle types pomme implements. `from_id` returning `None`
/// drops the packet in the network handler, before the event channel.
#[derive(Clone, Copy, Debug)]
pub enum ServerParticleKind {
    EndRod,
}

impl ServerParticleKind {
    /// Maps a particle registry id (`ParticleTypes` registration order in the
    /// 26.2 reference; ids shift between versions). Pomme owns this mapping
    /// because azalea's particle wire enum is out of sync with the registry.
    pub fn from_id(id: u32) -> Option<Self> {
        match id {
            27 => Some(Self::EndRod),
            _ => None,
        }
    }
}

pub struct ParticleStore {
    particles: Vec<Particle>,
    /// Spawned this tick; drained after live particles tick, so a particle's
    /// first physics tick is the tick after it spawns (vanilla
    /// `ParticleEngine.particlesToAdd`).
    pending: Vec<Particle>,
    uv_map: AtlasUVMap,
    block_colors: BlockColors,
}

impl ParticleStore {
    pub fn new(
        uv_map: AtlasUVMap,
        grass_colormap: Arc<Colormap>,
        foliage_colormap: Arc<Colormap>,
        dry_foliage_colormap: Arc<Colormap>,
    ) -> Self {
        Self {
            particles: Vec::new(),
            pending: Vec::new(),
            uv_map,
            block_colors: BlockColors::new(grass_colormap, foliage_colormap, dry_foliage_colormap),
        }
    }

    /// Vanilla `ClientLevel.addDestroyBlockEffect`: a grid of terrain
    /// particles across each box of the block's shape (4x4x4 for a full
    /// cube).
    pub fn add_destroy_block_effect(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        registry: BlockRenderView<'_>,
        chunks: &ChunkStore,
        biome_climate: &HashMap<u32, BiomeClimate>,
    ) {
        let Some((region, color)) =
            self.terrain_appearance(pos, state, registry, chunks, biome_climate)
        else {
            return;
        };
        let block_state_data = registry.block_state_table().block_state_data(state);
        // Vanilla `ParticleEngine.destroy` scatters over the outline shape's
        // boxes, so a block with an empty outline emits nothing.
        let boxes: &[LocalBox] = block_shape::outline_shape(block_state_data);
        let shape_origin = block_state_data.outline_shape_position(pos.x, pos.y, pos.z);

        for b in boxes {
            let width_x = (b[3] - b[0]).min(1.0);
            let width_y = (b[4] - b[1]).min(1.0);
            let width_z = (b[5] - b[2]).min(1.0);
            let count_x = ((width_x / 0.25).ceil() as i32).max(2);
            let count_y = ((width_y / 0.25).ceil() as i32).max(2);
            let count_z = ((width_z / 0.25).ceil() as i32).max(2);
            for xx in 0..count_x {
                for yy in 0..count_y {
                    for zz in 0..count_z {
                        let rel_x = (xx as f64 + 0.5) / count_x as f64;
                        let rel_y = (yy as f64 + 0.5) / count_y as f64;
                        let rel_z = (zz as f64 + 0.5) / count_z as f64;
                        let spawn = DVec3::new(
                            shape_origin.x + rel_x * width_x + b[0],
                            shape_origin.y + rel_y * width_y + b[1],
                            shape_origin.z + rel_z * width_z + b[2],
                        );
                        self.push(Particle::terrain(
                            spawn,
                            DVec3::new(rel_x - 0.5, rel_y - 0.5, rel_z - 0.5),
                            region,
                            color,
                        ));
                    }
                }
            }
        }
    }

    /// `TerrainParticle`'s sprite and color for `state` at `pos`; `None` for
    /// blocks that spawn no terrain particles.
    fn terrain_appearance(
        &self,
        pos: BlockPos,
        state: BlockState,
        registry: BlockRenderView<'_>,
        chunks: &ChunkStore,
        biome_climate: &HashMap<u32, BiomeClimate>,
    ) -> Option<(AtlasRegion, [f32; 3])> {
        let block_state_data = registry.block_state_table().block_state_data(state);
        if block_state_data.is_air() {
            return None;
        }
        let block_id = block_state_data.id();
        // The only vanilla `noTerrainParticles` blocks.
        if matches!(block_id, "barrier" | "structure_void") {
            return None;
        }

        let appearance =
            BlockParticleAppearance::new(registry, &self.uv_map, &self.block_colors, biome_climate);
        // TerrainParticle base grey, multiplied by the block's biome tint.
        // grass_block is exempt (vanilla `colorAsTerrainParticle` returns
        // white for it — its particle texture is dirt).
        let mut color = [0.6f32; 3];
        let faces = registry.get_textures(state);
        if let Some(faces) = faces
            && faces.tint != Tint::None
            && block_id != "grass_block"
        {
            let tint = appearance.tint(state, pos, chunks);
            for (c, t) in color.iter_mut().zip(tint) {
                *c *= t;
            }
        }

        let region = appearance.sprite(state);
        Some((region, color))
    }

    /// `texture`'s atlas region, if the atlas has it.
    fn item_region(&self, texture: &str) -> Option<AtlasRegion> {
        self.uv_map
            .has_region(texture)
            .then(|| self.uv_map.get_region(texture))
    }

    /// Vanilla `LivingEntity.spawnItemParticles`: item crumbs thrown from the
    /// mouth along the look direction while eating.
    pub fn add_item_use_particles(
        &mut self,
        count: u32,
        texture: &str,
        eye_pos: DVec3,
        x_rot_deg: f32,
        y_rot_deg: f32,
    ) {
        let Some(region) = self.item_region(texture) else {
            return;
        };
        let x_rot = -(x_rot_deg as f64).to_radians();
        let y_rot = -(y_rot_deg as f64).to_radians();
        for _ in 0..count {
            let p = dvec3(
                (fastrand::f64() - 0.5) * 0.3,
                -fastrand::f64() * 0.6 - 0.3,
                0.6,
            );
            let p = rot_y(rot_x(p, x_rot), y_rot) + eye_pos;
            self.push(Particle::breaking_item(
                p,
                crumb_velocity(x_rot, y_rot),
                region,
            ));
        }
    }

    /// Vanilla `Fox.handleEntityEvent(45)`: eight crumbs of the food in its
    /// mouth, half its look vector ahead of its feet.
    pub fn add_fox_eating_particles(
        &mut self,
        texture: &str,
        pos: DVec3,
        look: crate::entity::components::LookDirection,
        camera_pos: DVec3,
    ) {
        let Some(region) = self.item_region(texture) else {
            return;
        };
        let x_rot = -f64::from(look.x_rot_rad());
        let y_rot = -f64::from(look.y_rot_rad());
        let ahead = look.as_vec().as_dvec3() / 2.0;
        let p = dvec3(pos.x + ahead.x, pos.y, pos.z + ahead.z);
        if beyond_particle_range(camera_pos, p) {
            return;
        }
        for _ in 0..8 {
            self.push(Particle::breaking_item(
                p,
                crumb_velocity(x_rot, y_rot),
                region,
            ));
        }
    }

    /// Vanilla `Panda.addEatingParticles`: six crumbs of the held item in
    /// front of the head. `ClientLevel.addParticle` culls each past 32 blocks.
    #[allow(clippy::too_many_arguments)]
    pub fn add_panda_eating_particles(
        &mut self,
        texture: &str,
        pos: DVec3,
        eye_y: f64,
        x_rot_deg: f32,
        y_rot_deg: f32,
        body_y_rot_deg: f32,
        camera_pos: DVec3,
    ) {
        let Some(region) = self.item_region(texture) else {
            return;
        };
        let rand = || f64::from(fastrand::f32());
        let x_rot = f64::from(-x_rot_deg.to_radians());
        let y_rot = f64::from(-y_rot_deg.to_radians());
        let body_rot = f64::from(-body_y_rot_deg.to_radians());
        for _ in 0..6 {
            let v = dvec3(
                (rand() - 0.5) * 0.1,
                rand() * 0.1 + 0.1,
                (rand() - 0.5) * 0.1,
            );
            let v = rot_y(rot_x(v, x_rot), y_rot);
            let p = dvec3(
                (rand() - 0.5) * 0.8,
                -rand() * 0.6 - 0.3,
                1.0 + (rand() - 0.5) * 0.4,
            );
            let p = rot_y(p, body_rot) + dvec3(pos.x, eye_y + 1.0, pos.z);
            if beyond_particle_range(camera_pos, p) {
                continue;
            }
            self.push(Particle::breaking_item(
                p,
                dvec3(v.x, v.y + 0.05, v.z),
                region,
            ));
        }
    }

    /// Vanilla `ParticleGroup` caps: hard limit plus probabilistic rejection
    /// once the reservoir fills.
    fn push(&mut self, particle: Particle) {
        let count = self.particles.len() + self.pending.len();
        if count >= MAX_PARTICLES {
            return;
        }
        if count >= RESERVOIR_START {
            let free = (MAX_PARTICLES - count) as f32 / 4096.0;
            if fastrand::f32() >= free * free {
                return;
            }
        }
        self.pending.push(particle);
    }

    pub fn clear(&mut self) {
        self.particles.clear();
        self.pending.clear();
    }

    pub fn tick(&mut self, chunks: &ChunkStore, block_state_table: BlockStateTable) {
        self.particles
            .retain_mut(|p| p.tick(chunks, block_state_table));
        self.particles.append(&mut self.pending);
    }

    /// Quad positions are anchor-relative, subtracted in f64 (see
    /// `Camera::anchor`).
    pub fn extract(
        &self,
        partial_tick: f32,
        anchor: DVec3,
        world: &WorldLight,
    ) -> Vec<ParticleQuad> {
        self.particles
            .iter()
            .map(|p| {
                let pos = (p.prev_pos.lerp(p.pos, partial_tick as f64) - anchor).as_vec3();
                let channel = |c: f32| (c * 255.0).round() as u8;
                ParticleQuad {
                    pos: pos.into(),
                    size: p.size,
                    u0: p.u0,
                    u1: p.u1,
                    v0: p.v0,
                    v1: p.v1,
                    color: u32::from_le_bytes([
                        channel(p.color[0]),
                        channel(p.color[1]),
                        channel(p.color[2]),
                        (p.alpha * 255.0).round() as u8,
                    ]),
                    light: p.light(world).lane(),
                    translucent: false,
                }
            })
            .collect()
    }
}

/// `LevelRenderer.addParticleInternal`'s 32-block camera cull.
fn beyond_particle_range(camera_pos: DVec3, pos: DVec3) -> bool {
    camera_pos.distance_squared(pos) > 1024.0
}

/// `java.util.Random.nextGaussian` (Marsaglia polar method), minus the
/// second-sample cache.
pub(crate) fn next_gaussian() -> f64 {
    loop {
        let v1 = 2.0 * fastrand::f64() - 1.0;
        let v2 = 2.0 * fastrand::f64() - 1.0;
        let s = v1 * v1 + v2 * v2;
        if s < 1.0 && s != 0.0 {
            return v1 * (-2.0 * s.ln() / s).sqrt();
        }
    }
}

/// The velocity vanilla throws item crumbs with: a small random push up and
/// sideways, turned by the eater's `x_rot` and `y_rot` (radians, negated).
fn crumb_velocity(x_rot: f64, y_rot: f64) -> DVec3 {
    let d = dvec3(
        (fastrand::f64() - 0.5) * 0.1,
        fastrand::f64() * 0.1 + 0.1,
        0.0,
    );
    rot_y(rot_x(d, x_rot), y_rot) + dvec3(0.0, 0.05, 0.0)
}

/// Vanilla `Vec3.xRot`: rotation about the X axis.
fn rot_x(v: DVec3, angle: f64) -> DVec3 {
    let (sin, cos) = angle.sin_cos();
    dvec3(v.x, v.y * cos - v.z * sin, v.y * sin + v.z * cos)
}

/// Vanilla `Vec3.yRot`: rotation about the Y axis.
fn rot_y(v: DVec3, angle: f64) -> DVec3 {
    let (sin, cos) = angle.sin_cos();
    dvec3(v.x * cos + v.z * sin, v.y, -v.x * sin + v.z * cos)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::rc::Rc;

    use azalea_protocol::common::client_information::ParticleStatus;

    use super::*;
    use crate::particles::{
        MappedParticleResources, ParticleAtlasSprites, ParticleDescription, ParticleEngine,
        ParticleLight, ParticleResources, ParticleTypes, PreparedParticleDescriptions,
        SimpleParticleType, register_builtin_providers,
    };
    use crate::renderer::chunk::atlas::TextureAtlas;
    use crate::resources::managers::ReloadableResourceManager;
    use crate::resources::pack_resources::PackType;
    use crate::resources::resource_id;
    use crate::util::random_source::RandomSource;
    use crate::world::block::block_registry::BlockRegistry;
    use crate::world::level::LevelContext;

    #[test]
    fn particles_take_vanilla_light_coords() {
        let chunks = ChunkStore::new(8);
        let world = WorldLight {
            chunks: &chunks,
            table: BlockStateTable::native(),
        };
        let region = AtlasRegion::UNIT;
        let terrain = Particle::terrain(DVec3::new(0.5, 64.0, 0.5), DVec3::ZERO, region, [1.0; 3]);
        // No chunk loaded: `Particle.getLightCoords`' sky-only fallback.
        assert_eq!(terrain.light(&world), LightLevels::new(15, 0));
        let types = ParticleTypes::new().unwrap();
        let mut resources = MappedParticleResources::new();
        register_builtin_providers(&types, &mut resources);
        let resources = Rc::new(resources);
        let mut engine = ParticleEngine::new(Rc::clone(&resources));
        let sprite = resource_id!("minecraft:glitter_7");
        resources.apply(
            PreparedParticleDescriptions {
                descriptions: BTreeMap::from([(
                    types.end_rod.info().name().clone(),
                    ParticleDescription {
                        textures: vec![sprite.clone()],
                    },
                )]),
            },
            &ParticleAtlasSprites::new(BTreeMap::from([(sprite, region)]), region),
            &mut || engine.clear_particles(),
        );
        let random = RandomSource::new();
        let engine_access = engine.engine_access();
        let registry = BlockRegistry::native();
        let colors = BlockColors::new(
            Arc::new(Colormap::fallback()),
            Arc::new(Colormap::fallback()),
            Arc::new(Colormap::fallback()),
        );
        let biome_climate = HashMap::new();
        let manager = ReloadableResourceManager::new(PackType::ClientResources);
        let atlas = TextureAtlas::build_cpu(&manager, &HashSet::new(), &HashSet::new()).uv_map;
        let appearance =
            BlockParticleAppearance::new(registry.render_view(), &atlas, &colors, &biome_climate);
        let mut level = LevelContext::new(
            &chunks,
            &appearance,
            DVec3::ZERO,
            ParticleStatus::All,
            &random,
            engine_access.as_ref(),
        );
        let options = SimpleParticleType::new(Rc::clone(&types.end_rod));
        let end_rod = engine
            .create_particle(&mut level, &options, DVec3::ZERO, DVec3::ZERO)
            .unwrap();
        assert_eq!(
            end_rod.borrow().get_light_coords(&level, 0.0),
            ParticleLight {
                block: 240,
                sky: 240
            }
        );
    }
}
