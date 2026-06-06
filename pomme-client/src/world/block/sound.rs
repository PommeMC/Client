//! Per-block mining hit sound.
//!
//! Vanilla plays a block's `SoundType` hit sound every few ticks while it is
//! being mined (`MultiPlayerGameMode.continueDestroyBlock`). `azalea-block`
//! does not expose sound type, so the block id -> hit sound table is generated
//! from the decompiled vanilla `Blocks.java` by `tools/gen_block_sounds.py` and
//! embedded here as `block_sounds.json`.

use std::collections::HashMap;
use std::sync::LazyLock;

use azalea_block::{BlockState, BlockTrait};

/// A block's vanilla mining hit sound: the `sounds.json` event and the block's
/// raw `SoundType` volume and pitch, scaled by the caller at play time.
#[derive(Clone)]
pub struct BlockHitSound {
    pub event: String,
    pub volume: f32,
    pub pitch: f32,
}

/// block id (no namespace) -> (hit event, volume, pitch). An empty event marks
/// a block whose `SoundType` hit sound is intentionally silent (e.g. cactus
/// flower, water).
static BLOCK_SOUNDS: LazyLock<HashMap<String, (String, f32, f32)>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("block_sounds.json"))
        .expect("embedded block_sounds.json must be valid")
});

/// The vanilla mining hit sound for `state`, or `None` when the block is silent
/// or has no hit sound. Unknown ids fall back to the vanilla `SoundType.STONE`
/// default.
pub fn block_hit_sound(state: BlockState) -> Option<BlockHitSound> {
    let block: Box<dyn BlockTrait> = state.into();
    let id = block.id();
    let key = id.strip_prefix("minecraft:").unwrap_or(id);

    let (event, volume, pitch) = BLOCK_SOUNDS
        .get(key)
        .map(|(e, v, p)| (e.as_str(), *v, *p))
        .unwrap_or(("block.stone.hit", 1.0, 1.0));

    if event.is_empty() {
        return None;
    }
    Some(BlockHitSound {
        event: event.to_string(),
        volume,
        pitch,
    })
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

        let event = |id: &str| BLOCK_SOUNDS.get(id).map(|(e, _, _)| e.as_str());
        assert_eq!(event("stone"), Some("block.stone.hit"));
        assert_eq!(event("oak_planks"), Some("block.wood.hit"));
        assert_eq!(event("oak_door"), Some("block.wood.hit")); // BlockSetType.OAK
        assert_eq!(event("dirt"), Some("block.gravel.hit"));
        assert_eq!(event("copper_block"), Some("block.copper.hit"));
        assert_eq!(event("waxed_oxidized_copper"), Some("block.copper.hit"));
        // METAL carries a non-default pitch (1.5).
        assert_eq!(
            BLOCK_SOUNDS.get("gold_block"),
            Some(&("block.metal.hit".to_string(), 1.0, 1.5))
        );
        assert_eq!(event("cactus_flower"), Some(""));
    }
}
