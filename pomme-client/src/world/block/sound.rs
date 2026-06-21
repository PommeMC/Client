//! Per-block hit, break, and place sounds.
//!
//! Vanilla plays a block's `SoundType` hit sound while it is being mined
//! (`MultiPlayerGameMode.continueDestroyBlock`), its break sound when the
//! block is destroyed (level event 2001), and its place sound when a block is
//! placed (`BlockItem.place`). `azalea-block` does not expose sound type, so
//! the block id -> sounds table was extracted from the decompiled vanilla
//! `Blocks.java` / `SoundType.java` and is embedded here as
//! `block_sounds.json`. The table holds only hit and break events; the place
//! event is derived from the break event (see `place_event_for`).

use std::collections::HashMap;
use std::sync::LazyLock;

use azalea_block::{BlockState, BlockTrait};

/// A block's vanilla `SoundType` sounds: the `sounds.json` hit, break, and
/// place events plus the raw volume and pitch (the caller applies the play-time
/// scaling). An empty event marks an action that is intentionally silent for
/// the block.
#[derive(Clone)]
pub struct BlockSounds {
    pub hit_event: String,
    pub break_event: String,
    pub place_event: String,
    pub volume: f32,
    pub pitch: f32,
}

/// block id (no namespace) -> (hit event, break event, volume, pitch).
static BLOCK_SOUNDS: LazyLock<HashMap<String, (String, String, f32, f32)>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("block_sounds.json"))
        .expect("embedded block_sounds.json must be valid")
});

/// The vanilla `SoundType` sounds for `state`. Unknown ids fall back to the
/// vanilla `SoundType.STONE` default. An empty event field means that action is
/// silent for the block.
pub fn block_sounds(state: BlockState) -> BlockSounds {
    let block: Box<dyn BlockTrait> = state.into();
    let id = block.id();
    let key = id.strip_prefix("minecraft:").unwrap_or(id);

    let (hit, brk, volume, pitch) = BLOCK_SOUNDS
        .get(key)
        .map(|(h, b, v, p)| (h.as_str(), b.as_str(), *v, *p))
        .unwrap_or(("block.stone.hit", "block.stone.break", 1.0, 1.0));

    BlockSounds {
        hit_event: hit.to_string(),
        break_event: brk.to_string(),
        place_event: place_event_for(key, brk),
        volume,
        pitch,
    }
}

/// Derives a block's place sound event from its break event. Vanilla
/// `SoundType`s use `block.<family>.place` to match their break sound, so the
/// place event is the break event with its action swapped to `place`.
fn place_event_for(id: &str, break_event: &str) -> String {
    // LILY_PAD breaks with the big_dripleaf family but has its own place sound.
    if id == "lily_pad" {
        return "block.lily_pad.place".to_string();
    }
    match break_event.rsplit_once('.') {
        Some((prefix, _)) => format!("{prefix}.place"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_table_parses_and_matches_vanilla() {
        assert!(
            BLOCK_SOUNDS.len() > 1000,
            "expected the full vanilla block set, got {}",
            BLOCK_SOUNDS.len()
        );

        let hit = |id: &str| BLOCK_SOUNDS.get(id).map(|(h, _, _, _)| h.as_str());
        let brk = |id: &str| BLOCK_SOUNDS.get(id).map(|(_, b, _, _)| b.as_str());
        assert_eq!(hit("stone"), Some("block.stone.hit"));
        assert_eq!(brk("stone"), Some("block.stone.break"));
        assert_eq!(hit("oak_door"), Some("block.wood.hit")); // BlockSetType.OAK
        assert_eq!(brk("oak_door"), Some("block.wood.break"));
        assert_eq!(hit("dirt"), Some("block.gravel.hit"));
        assert_eq!(hit("copper_block"), Some("block.copper.hit"));
        // METAL carries a non-default pitch (1.5).
        assert_eq!(
            BLOCK_SOUNDS.get("gold_block"),
            Some(&(
                "block.metal.hit".to_string(),
                "block.metal.break".to_string(),
                1.0,
                1.5
            ))
        );
        // Silent hit, but the break sound is still present.
        assert_eq!(hit("cactus_flower"), Some(""));
        assert_eq!(brk("cactus_flower"), Some("block.cactus_flower.break"));
    }

    #[test]
    fn place_event_derives_from_break_family() {
        assert_eq!(
            place_event_for("stone", "block.stone.break"),
            "block.stone.place"
        );
        assert_eq!(
            place_event_for("oak_planks", "block.wood.break"),
            "block.wood.place"
        );
        assert_eq!(
            place_event_for("lily_pad", "block.big_dripleaf.break"),
            "block.lily_pad.place"
        );
        assert_eq!(place_event_for("anything", ""), "");
    }
}
