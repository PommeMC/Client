//! Particles: a port of vanilla `TerrainParticle`, `BreakingItemParticle`,
//! `EndRodParticle`, `Particle`, `ClientLevel.addDestroyBlockEffect`, and the
//! `ClientboundLevelParticles` spawn path.

use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::position::BlockPos;
use glam::{DVec3, dvec3};

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
use crate::world::block::{block_id, is_air};
use crate::world::chunk::ChunkStore;

/// Vanilla `ParticleGroup.RESERVOIR_START` — above this, new particles are
/// probabilistically dropped.
const RESERVOIR_START: usize = 12288;
/// Vanilla `Particle.MAXIMUM_COLLISION_VELOCITY_SQUARED` (100²).
const MAX_COLLISION_VELOCITY_SQ: f64 = 10000.0;
/// Terrain particles use the default 0.2-wide, 0.2-tall bounding box.
const HALF_WIDTH: f64 = 0.1;

enum Kind {
    /// `TerrainParticle` / `BreakingItemParticle`: collision physics,
    /// world-lit, fixed sprite, opaque layer.
    Terrain,
    /// `EndRodParticle` (a `SimpleAnimatedParticle`): no collision,
    /// full-bright, 8-frame animation, fades after half-life, translucent
    /// layer.
    EndRod,
}

impl Kind {
    /// Vanilla `SingleQuadParticle.getLayer`.
    fn translucent(&self) -> bool {
        matches!(self, Kind::EndRod)
    }
}

pub struct Particle {
    kind: Kind,
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
            kind: Kind::Terrain,
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
            light,
        }
    }

    /// Vanilla `EndRodParticle`: velocity taken verbatim from the spawn call
    /// (the 3-arg `Particle` constructor adds no jitter), tiny gravity, no
    /// collision, warm fade color.
    fn end_rod(pos: DVec3, vel: DVec3, frames: &[AtlasRegion; 8]) -> Self {
        // SingleQuadParticle base quadSize, then EndRodParticle *= 0.75.
        let size = 0.1 * (fastrand::f32() * 0.5 + 0.5) * 2.0 * 0.75;
        let mut p = Self {
            kind: Kind::EndRod,
            pos,
            prev_pos: pos,
            vel,
            age: 0,
            lifetime: 60 + fastrand::i32(0..12),
            on_ground: false,
            stopped_by_collision: false,
            gravity: f64::from(0.0125f32),
            friction: f64::from(0.91f32),
            size,
            u0: 0.0,
            u1: 0.0,
            v0: 0.0,
            v1: 0.0,
            color: [1.0; 3],
            alpha: 1.0,
            // SimpleAnimatedParticle.getLightCoords is always full-bright.
            light: 1.0,
        };
        p.set_sprite(&frames[0]);
        p
    }

    /// Vanilla `SingleQuadParticle.setSprite`.
    fn set_sprite(&mut self, frame: &AtlasRegion) {
        self.u0 = frame.u_min;
        self.u1 = frame.u_max;
        self.v0 = frame.v_min;
        self.v1 = frame.v_max;
    }

    /// Vanilla `BreakingItemParticle`: the base-constructor velocity (zero
    /// argument, jitter only) scaled to 10%, plus the spawn velocity. Shares
    /// the halved quad size and quarter sub-tile sampling with terrain.
    fn breaking_item(pos: DVec3, velocity: DVec3, region: AtlasRegion, light: f32) -> Self {
        let mut p = Self::terrain(pos, DVec3::ZERO, region, [1.0; 3], light);
        p.vel = p.vel * 0.1 + velocity;
        p
    }

    /// Vanilla `Particle.tick`. Returns false when the particle expires.
    fn tick(&mut self, chunks: &ChunkStore, end_rod_frames: &[AtlasRegion; 8]) -> bool {
        self.prev_pos = self.pos;
        if self.age >= self.lifetime {
            return false;
        }
        self.age += 1;
        self.vel.y -= 0.04 * self.gravity;
        match self.kind {
            Kind::Terrain => self.move_with_collision(chunks),
            // EndRodParticle.move() skips collision entirely.
            Kind::EndRod => self.pos += self.vel,
        }
        self.vel *= self.friction;
        if self.on_ground {
            self.vel.x *= 0.7;
            self.vel.z *= 0.7;
        }
        match self.kind {
            Kind::Terrain => {
                self.light = world_brightness(
                    chunks,
                    self.pos.x.floor() as i32,
                    self.pos.y.floor() as i32,
                    self.pos.z.floor() as i32,
                );
            }
            // SimpleAnimatedParticle.tick: advance the sprite frame, then
            // after half-life fade alpha out and lerp toward the fade color.
            Kind::EndRod => {
                self.set_sprite(&end_rod_frames[(self.age * 7 / self.lifetime) as usize]);
                if self.age > self.lifetime / 2 {
                    self.alpha = 1.0 - (self.age - self.lifetime / 2) as f32 / self.lifetime as f32;
                    for (c, f) in self.color.iter_mut().zip(END_ROD_FADE) {
                        *c += (f - *c) * 0.2;
                    }
                }
            }
        }
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

/// `SimpleAnimatedParticle.setFadeColor(0xF2DEC9)` in `EndRodParticle`.
const END_ROD_FADE: [f32; 3] = [242.0 / 255.0, 222.0 / 255.0, 201.0 / 255.0];

/// Frame order from `assets/minecraft/particles/end_rod.json`: frame 0 is
/// `glitter_7` and the animation walks toward `glitter_0`.
pub const END_ROD_SPRITES: [&str; 8] = [
    "particle/glitter_7",
    "particle/glitter_6",
    "particle/glitter_5",
    "particle/glitter_4",
    "particle/glitter_3",
    "particle/glitter_2",
    "particle/glitter_1",
    "particle/glitter_0",
];

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

    /// Vanilla `ParticleType.getOverrideLimiter`.
    fn override_limiter(self) -> bool {
        match self {
            Self::EndRod => false,
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
    end_rod_frames: [AtlasRegion; 8],
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
        let end_rod_frames = END_ROD_SPRITES.map(|k| uv_map.get_region(k));
        Self {
            particles: Vec::new(),
            pending: Vec::new(),
            uv_map,
            end_rod_frames,
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
        if is_air(state) {
            return;
        }
        let block_id = block_id(state);
        // The only vanilla `noTerrainParticles` blocks.
        if matches!(block_id, "barrier" | "structure_void") {
            return;
        }

        // TerrainParticle base grey, multiplied by the block's biome tint.
        // grass_block is exempt (vanilla `colorAsTerrainParticle` returns
        // white for it — its particle texture is dirt).
        let mut color = [0.6f32; 3];
        let faces = registry.get_textures(state);
        if let Some(faces) = faces
            && faces.tint != Tint::None
            && block_id != "grass_block"
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

    /// Vanilla `LivingEntity.spawnItemParticles`: item crumbs thrown from the
    /// mouth along the look direction while eating.
    pub fn add_item_use_particles(
        &mut self,
        count: u32,
        texture: &str,
        eye_pos: DVec3,
        x_rot_deg: f32,
        y_rot_deg: f32,
        chunks: &ChunkStore,
    ) {
        if !self.uv_map.has_region(texture) {
            return;
        }
        let region = self.uv_map.get_region(texture);
        let x_rot = -(x_rot_deg as f64).to_radians();
        let y_rot = -(y_rot_deg as f64).to_radians();
        let light = world_brightness(
            chunks,
            eye_pos.x.floor() as i32,
            eye_pos.y.floor() as i32,
            eye_pos.z.floor() as i32,
        );
        for _ in 0..count {
            let d = dvec3(
                (fastrand::f64() - 0.5) * 0.1,
                fastrand::f64() * 0.1 + 0.1,
                0.0,
            );
            let d = rot_y(rot_x(d, x_rot), y_rot);
            let p = dvec3(
                (fastrand::f64() - 0.5) * 0.3,
                -fastrand::f64() * 0.6 - 0.3,
                0.6,
            );
            let p = rot_y(rot_x(p, x_rot), y_rot) + eye_pos;
            self.push(Particle::breaking_item(
                p,
                dvec3(d.x, d.y + 0.05, d.z),
                region,
                light,
            ));
        }
    }

    /// Vanilla `ClientPacketListener.handleParticleEvent`: count 0 is a
    /// single directional particle (velocity = dist * max_speed), otherwise a
    /// gaussian scatter of `count` particles around the position.
    #[allow(clippy::too_many_arguments)]
    pub fn add_particles_from_packet(
        &mut self,
        kind: ServerParticleKind,
        override_limiter: bool,
        pos: DVec3,
        dist: DVec3,
        max_speed: f64,
        count: u32,
        camera_pos: DVec3,
    ) {
        if count == 0 {
            self.add_server_particle(kind, override_limiter, pos, dist * max_speed, camera_pos);
            return;
        }
        for _ in 0..count {
            let scatter = dvec3(
                next_gaussian() * dist.x,
                next_gaussian() * dist.y,
                next_gaussian() * dist.z,
            );
            let vel = dvec3(next_gaussian(), next_gaussian(), next_gaussian()) * max_speed;
            self.add_server_particle(kind, override_limiter, pos + scatter, vel, camera_pos);
        }
    }

    /// Vanilla `ClientLevel.doAddParticle`. Pomme reports
    /// `ParticleStatus::All` in client information, so the MINIMAL/DECREASED
    /// branches are unreachable and only the 32-block camera cull applies.
    fn add_server_particle(
        &mut self,
        kind: ServerParticleKind,
        override_limiter: bool,
        pos: DVec3,
        vel: DVec3,
        camera_pos: DVec3,
    ) {
        if !(override_limiter || kind.override_limiter())
            && camera_pos.distance_squared(pos) > 1024.0
        {
            return;
        }
        match kind {
            ServerParticleKind::EndRod => {
                let frames = self.end_rod_frames;
                self.push(Particle::end_rod(pos, vel, &frames));
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
        let frames = self.end_rod_frames;
        self.particles.retain_mut(|p| p.tick(chunks, &frames));
        self.particles.append(&mut self.pending);
    }

    /// Quad positions are anchor-relative, subtracted in f64 (see
    /// `Camera::anchor`).
    pub fn extract(&self, partial_tick: f32, anchor: DVec3) -> Vec<ParticleQuad> {
        self.particles
            .iter()
            .map(|p| {
                let pos = (p.prev_pos.lerp(p.pos, partial_tick as f64) - anchor).as_vec3();
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
                        (p.alpha * 255.0).round() as u8,
                    ]),
                    translucent: p.kind.translucent(),
                }
            })
            .collect()
    }
}

/// `java.util.Random.nextGaussian` (Marsaglia polar method), minus the
/// second-sample cache.
fn next_gaussian() -> f64 {
    loop {
        let v1 = 2.0 * fastrand::f64() - 1.0;
        let v2 = 2.0 * fastrand::f64() - 1.0;
        let s = v1 * v1 + v2 * v2;
        if s < 1.0 && s != 0.0 {
            return v1 * (-2.0 * s.ln() / s).sqrt();
        }
    }
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
