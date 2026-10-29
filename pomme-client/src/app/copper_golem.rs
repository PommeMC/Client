//! The copper golem's client-side sound: `CopperGolem.playHeadSpinSound`.

use azalea_registry::builtin::EntityKind;

use crate::app::phases::in_game::GameState;
use crate::audio::{AudioEngine, CATEGORY_NEUTRAL, SoundRef};

/// Runs after the entity tick, which flags each idle head spin.
pub(super) fn tick(audio: &AudioEngine, game: &mut GameState) {
    let spins: Vec<(i32, u32)> = game
        .entity_store
        .living
        .iter()
        .filter(|(_, e)| e.entity_type == EntityKind::CopperGolem && e.copper_golem.head_spun)
        .map(|(&id, e)| (id, e.copper_golem.weather))
        .collect();
    for (id, weather) in spins {
        if game.silent_entities.contains(&id) {
            continue;
        }
        let Some(&pos) = game.entity_positions.get(&id) else {
            continue;
        };
        // `CopperGolemOxidationLevels`: exposed shares the unaffected sound.
        let sound = SoundRef::event(match weather {
            0 | 1 => "entity.copper_golem.spin",
            2 => "entity.copper_golem_weathered.spin",
            _ => "entity.copper_golem_oxidized.spin",
        });
        #[cfg(feature = "test-harness")]
        game.record_sound(&sound);
        audio.play_world_sound(&sound, CATEGORY_NEUTRAL, pos, 1.0, 1.0, fastrand::u64(..));
    }
}
