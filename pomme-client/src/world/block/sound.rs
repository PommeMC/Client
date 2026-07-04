//! Per-block hit and break sounds.
//!
//! Vanilla plays a block's `SoundType` hit sound while it is being mined
//! (`MultiPlayerGameMode.continueDestroyBlock`) and its break sound when the
//! block is destroyed (level event 2001). `azalea-block` does not expose sound
//! type, so the block id -> sounds table was extracted from the decompiled
//! vanilla `Blocks.java` / `SoundType.java` and is embedded here as
//! `block_sounds.json`.

use std::collections::HashMap;
use std::sync::LazyLock;

use azalea_block::BlockState;

/// A block's vanilla `SoundType` sounds: the `sounds.json` hit and break events
/// plus the raw volume and pitch (the caller applies the play-time scaling). An
/// empty event marks an action that is intentionally silent for the block.
#[derive(Clone)]
pub struct BlockSounds {
    pub hit_event: String,
    pub break_event: String,
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
    let id = super::block_id(state);
    let key = id.strip_prefix("minecraft:").unwrap_or(id);

    let (hit, brk, volume, pitch) = BLOCK_SOUNDS
        .get(key)
        .map(|(h, b, v, p)| (h.as_str(), b.as_str(), *v, *p))
        .unwrap_or(("block.stone.hit", "block.stone.break", 1.0, 1.0));

    BlockSounds {
        hit_event: hit.to_string(),
        break_event: brk.to_string(),
        volume,
        pitch,
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
}
