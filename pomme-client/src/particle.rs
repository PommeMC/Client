//! Block-break particles: a port of vanilla `TerrainParticle`,
//! `Particle`, and `ClientLevel.addDestroyBlockEffect`.

use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::position::BlockPos;
use glam::DVec3;

use crate::physics::aabb::Aabb;
use crate::physics::block_shape::{self, LocalBox};
use crate::physics::collision::resolve_collision;
use crate::renderer::ParticleQuad;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap};
use crate::renderer::chunk::mesher::{
    BiomeClimate, Colormap, blend_color, dry_foliage_color, foliage_color, grass_color,
    world_brightness,
};
use crate::renderer::pipelines::particle::MAX_PARTICLE_QUADS as MAX_PARTICLES;
use crate::world::block::registry::{BlockRegistry, Tint};
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
    /// Vanilla `quadSize`; the billboard spans twice this.
    size: f32,
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
    color: [f32; 3],
    light: f32,
}

impl Particle {
    /// Vanilla `TerrainParticle` constructor chain: velocity jitter +
    /// normalization from `Particle(level, x, y, z, xa, ya, za)`, then the
    /// terrain quad size and random quarter sub-tile.
    fn terrain(
        pos: DVec3,
        velocity_arg: DVec3,
        region: AtlasRegion,
        color: [f32; 3],
        light: f32,
    ) -> Self {
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
            size,
            u0: sprite_u((uo + 1.0) / 4.0),
            u1: sprite_u(uo / 4.0),
            v0: sprite_v(vo / 4.0),
            v1: sprite_v((vo + 1.0) / 4.0),
            color,
            light,
        }
    }

    /// Vanilla `Particle.tick` (gravity 1.0, friction 0.98). Returns false
    /// when the particle expires.
    fn tick(&mut self, chunks: &ChunkStore) -> bool {
        self.prev_pos = self.pos;
        if self.age >= self.lifetime {
            return false;
        }
        self.age += 1;
        self.vel.y -= 0.04;
        self.move_with_collision(chunks);
        self.vel *= 0.98;
        if self.on_ground {
            self.vel.x *= 0.7;
            self.vel.z *= 0.7;
        }
        self.light = world_brightness(
            chunks,
            self.pos.x.floor() as i32,
            self.pos.y.floor() as i32,
            self.pos.z.floor() as i32,
        );
        true
    }

    /// Vanilla `Particle.move`.
    fn move_with_collision(&mut self, chunks: &ChunkStore) {
        if self.stopped_by_collision {
            return;
        }
        let orig = self.vel;
        let mut delta = orig;
        if delta != DVec3::ZERO && delta.length_squared() < MAX_COLLISION_VELOCITY_SQ {
            let aabb = Aabb::from_center(self.pos, HALF_WIDTH, HALF_WIDTH);
            (delta, _) = resolve_collision(chunks, aabb, orig.into(), 0.0);
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

pub struct ParticleStore {
    particles: Vec<Particle>,
    /// Spawned this tick; drained after live particles tick, so a particle's
    /// first physics tick is the tick after it spawns (vanilla
    /// `ParticleEngine.particlesToAdd`).
    pending: Vec<Particle>,
    uv_map: AtlasUVMap,
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    dry_foliage_colormap: Arc<Colormap>,
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
            grass_colormap,
            foliage_colormap,
            dry_foliage_colormap,
        }
    }

    /// Vanilla `ClientLevel.addDestroyBlockEffect`: a grid of terrain
    /// particles across each box of the block's shape (4x4x4 for a full
    /// cube).
    pub fn add_destroy_block_effect(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        registry: &BlockRegistry,
        chunks: &ChunkStore,
        biome_climate: &HashMap<u32, BiomeClimate>,
    ) {
        if state.is_air() {
            return;
        }
        let block: Box<dyn azalea_block::BlockTrait> = state.into();
        // The only vanilla `noTerrainParticles` blocks.
        if matches!(block.id(), "barrier" | "structure_void") {
            return;
        }

        // TerrainParticle base grey, multiplied by the block's biome tint.
        // grass_block is exempt (vanilla `colorAsTerrainParticle` returns
        // white for it — its particle texture is dirt).
        let mut color = [0.6f32; 3];
        let faces = registry.get_textures(state);
        if let Some(faces) = faces
            && faces.tint != Tint::None
            && block.id() != "grass_block"
        {
            let tint = self.blend_tint(faces.tint, pos, chunks, biome_climate);
            for (c, t) in color.iter_mut().zip(tint) {
                *c *= t;
            }
        }

        let region = match faces {
            Some(faces) => self
                .uv_map
                .get_region(faces.particle.as_deref().unwrap_or(&faces.top)),
            // No face textures at all: the missing-texture region.
            None => self.uv_map.get_region(""),
        };
        let light = world_brightness(chunks, pos.x, pos.y, pos.z);

        const FULL_CUBE: LocalBox = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let boxes: &[LocalBox] = match block_shape::partial_shape(state) {
            Some(boxes) if !boxes.is_empty() => boxes,
            // None = full cube. Some(&[]) = no collision (flowers, torches):
            // vanilla scatters over the outline shape, which pomme doesn't
            // have, so fall back to the full cube.
            _ => &[FULL_CUBE],
        };

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
                            pos.x as f64 + rel_x * width_x + b[0],
                            pos.y as f64 + rel_y * width_y + b[1],
                            pos.z as f64 + rel_z * width_z + b[2],
                        );
                        self.push(Particle::terrain(
                            spawn,
                            DVec3::new(rel_x - 0.5, rel_y - 0.5, rel_z - 0.5),
                            region,
                            color,
                            light,
                        ));
                    }
                }
            }
        }
    }

    /// The block's biome tint averaged over the vanilla 5x5 biome blend.
    fn blend_tint(
        &self,
        tint: Tint,
        pos: BlockPos,
        chunks: &ChunkStore,
        biome_climate: &HashMap<u32, BiomeClimate>,
    ) -> [f32; 3] {
        blend_color(pos.x, pos.z, |x, z| {
            let climate = biome_climate
                .get(&chunks.biome_id(x, pos.y, z))
                .copied()
                .unwrap_or_default();
            match tint {
                Tint::Grass => grass_color(&climate, &self.grass_colormap, x, z),
                Tint::Foliage => foliage_color(&climate, &self.foliage_colormap),
                Tint::DryFoliage => dry_foliage_color(&climate, &self.dry_foliage_colormap),
                Tint::None => [1.0; 3],
            }
        })
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

    pub fn tick(&mut self, chunks: &ChunkStore) {
        self.particles.retain_mut(|p| p.tick(chunks));
        self.particles.append(&mut self.pending);
    }

    pub fn extract(&self, partial_tick: f32) -> Vec<ParticleQuad> {
        self.particles
            .iter()
            .map(|p| {
                let pos = p.prev_pos.lerp(p.pos, partial_tick as f64).as_vec3();
                let channel = |c: f32| (c * p.light * 255.0).round() as u8;
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
                        255,
                    ]),
                }
            })
            .collect()
    }
}
