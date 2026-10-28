//! The sniffer's client-side sounds and particles: `SnifferSoundInstance`
//! (entity event 63) and the client half of `Sniffer.tick`.

use azalea_registry::builtin::EntityKind;
use glam::DVec3;

use crate::app::phases::in_game::GameState;
use crate::audio::{AudioEngine, CATEGORY_NEUTRAL, SoundRef};
use crate::entity::SnifferState;
use crate::position::BlockPos;
use crate::world::block::model::Direction;
use crate::world::block::sound::block_sounds_for_id;

/// `ClientLevel.playLocalSound(Entity, ..)` with the sniffer's sound source.
fn play_bound(
    audio: &mut AudioEngine,
    game: &mut GameState,
    id: i32,
    event: &str,
    volume: f32,
    pitch: f32,
) -> Option<u64> {
    super::core::play_entity_sound(
        audio,
        game,
        id,
        &SoundRef::event(event),
        CATEGORY_NEUTRAL,
        volume,
        pitch,
        fastrand::u64(..),
    )
}

/// `ClientPacketListener.handleEntityEvent` case 63.
pub(super) fn start_digging_sound(audio: &mut AudioEngine, game: &mut GameState, id: i32) {
    if game.entity_store.living.get(&id).map(|e| e.entity_type) != Some(EntityKind::Sniffer) {
        return;
    }
    if let Some(sound) = play_bound(audio, game, id, "entity.sniffer.digging", 1.0, 1.0)
        && let Some(e) = game.entity_store.living.get_mut(&id)
    {
        e.sniffer_dig_sounds.push(sound);
    }
}

/// Runs after the entity tick, so ages are the post-increment `tickCount`.
pub(super) fn tick(audio: &mut AudioEngine, game: &mut GameState, camera_pos: DVec3) {
    let sniffers: Vec<i32> = game
        .entity_store
        .living
        .iter()
        .filter(|(_, e)| e.entity_type == EntityKind::Sniffer)
        .map(|(&id, _)| id)
        .collect();
    for id in sniffers {
        let Some(e) = game.entity_store.living.get_mut(&id) else {
            continue;
        };
        // `SnifferSoundInstance.tick`; `getTarget()` is never set client-side.
        if !e.sniffer_state.can_play_digging_sound() {
            for sound in e.sniffer_dig_sounds.drain(..) {
                audio.stop(sound);
            }
        }
        match e.sniffer_state {
            SnifferState::Digging => emit_digging_particles(audio, game, id, camera_pos),
            SnifferState::Searching if e.age_in_ticks.is_multiple_of(20) => {
                play_bound(audio, game, id, "entity.sniffer.searching", 1.0, 1.0);
            }
            _ => {}
        }
    }
}

/// `Sniffer.emitDiggingParticles`.
fn emit_digging_particles(
    audio: &mut AudioEngine,
    game: &mut GameState,
    id: i32,
    camera_pos: DVec3,
) {
    let e = &game.entity_store.living[&id];
    let Some(start) = e.sniffer_anim_start else {
        return;
    };
    let millis = (i64::from(e.age_in_ticks) - i64::from(start)) * 50;
    if millis <= 1700 || millis >= 6000 {
        return;
    }
    // `getHeadBlock`.
    let forward = e.look_dir.as_vec().as_dvec3() * 2.25;
    let feet = DVec3::from(e.position);
    let head = BlockPos::containing(DVec3::new(
        feet.x + forward.x,
        feet.y + f64::from(0.2f32),
        feet.z + forward.z,
    ));
    let age = e.age_in_ticks;
    let below = head.relative(Direction::Down);
    let state = game.chunk_store.get_block_state(below.x, below.y, below.z);
    let data = game.block_registry.block_state_data(state);
    if data.is_invisible() {
        return;
    }
    let center = DVec3::new(
        head.x as f64 + 0.5,
        head.y as f64 + 0.5 - f64::from(0.65f32),
        head.z as f64 + 0.5,
    );
    for _ in 0..30 {
        game.particle_store.add_block_particle(
            center,
            state,
            game.block_registry.render_view(),
            &game.chunk_store,
            &game.biome_climate,
            camera_pos,
        );
    }
    if age.is_multiple_of(10) {
        let hit = block_sounds_for_id(data.id()).hit_event;
        if !hit.is_empty() {
            play_bound(audio, game, id, &hit, 0.5, 0.5);
        }
    }
}
