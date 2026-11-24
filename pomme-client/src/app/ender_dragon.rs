//! The ender dragon's client tick: the flap clock and flight history from
//! `EnderDragon.aiStep`, and the wing flap and growl it plays locally.

use pomme_registry::builtin::EntityKind;

use crate::app::phases::in_game::GameState;
use crate::audio::{AudioEngine, CATEGORY_HOSTILE, SoundRef};
use crate::world::chunk::HeightmapKind;

/// Runs after the living entities tick.
pub(super) fn tick(audio: &AudioEngine, game: &mut GameState) {
    // `EnderDragonFight.getPodiumLocation` of the client's fight origin,
    // which it never learns: the column at 0, 0.
    let egg_y = game
        .chunk_store
        .heightmap_y(HeightmapKind::MotionBlockingNoLeaves, 0, 0);
    let egg = glam::DVec3::new(0.5, f64::from(egg_y) + 0.5, 0.5);
    for (id, e) in &mut game.entity_store.living {
        if e.entity_type != EntityKind::EnderDragon {
            continue;
        }
        let sounds = e.ender_dragon.tick(
            *e.position,
            e.look_dir.y_rot_deg(),
            e.velocity,
            e.health <= 0.0,
            e.no_ai,
            game.silent_entities.contains(id),
        );
        e.ender_dragon.distance_to_egg = egg.distance_squared(*e.position);
        let play = |event, volume| {
            let sound = SoundRef::event(event);
            let pitch = 0.8 + fastrand::f32() * 0.3;
            audio.play_world_sound(
                &sound,
                CATEGORY_HOSTILE,
                e.position,
                volume,
                pitch,
                fastrand::u64(..),
            );
        };
        if sounds.flap {
            play("entity.ender_dragon.flap", 5.0);
        }
        if sounds.growl {
            play("entity.ender_dragon.growl", 2.5);
        }
    }
}
