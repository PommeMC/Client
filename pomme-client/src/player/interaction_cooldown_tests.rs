use std::sync::Arc;

use azalea_inventory::ItemStack;
use azalea_inventory::components::{DataComponentTrait, UseCooldown};
use glam::dvec3;

use super::*;
use crate::net::sender::Outbound;
use crate::renderer::chunk::atlas::AtlasUVMap;
use crate::renderer::chunk::mesher::Colormap;

fn grouped(kind: ItemKind, group: Option<&str>) -> ItemStackData {
    ItemStack::new(kind, 1)
        .with_component(UseCooldown {
            seconds: 1.0,
            cooldown_group: group.map(Into::into),
        })
        .as_present()
        .unwrap()
        .clone()
}

#[test]
fn cooldown_groups_default_override_and_tombstone() {
    let mut state = InteractionState::new();
    let mut pearl = ItemStackData::new(ItemKind::EnderPearl, 1);
    assert_eq!(item_cooldown_group(&pearl), "minecraft:ender_pearl");
    assert!(get_default_component::<UseCooldown>(pearl.kind).is_some());
    state.set_item_cooldown("ender_pearl".into(), 5);
    assert!(state.is_on_item_cooldown(&pearl));
    pearl = grouped(ItemKind::EnderPearl, Some("example:shared"));
    let stick = grouped(ItemKind::Stick, Some("example:shared"));
    assert!(!state.is_on_item_cooldown(&pearl));
    state.set_item_cooldown("example:shared".into(), 3);
    assert!(state.is_on_item_cooldown(&pearl));
    assert!(state.is_on_item_cooldown(&stick));
    assert!(!state.is_on_item_cooldown(&ItemStackData::new(ItemKind::Stick, 1)));
    assert_eq!(
        item_cooldown_group(&grouped(ItemKind::EnderPearl, None)),
        "minecraft:ender_pearl"
    );
    // SAFETY: a tombstone contains no union value to mismatch with its kind.
    unsafe {
        pearl
            .component_patch
            .unchecked_insert_component(UseCooldown::KIND, None);
    }
    assert!(
        pearl
            .component_patch
            .iter()
            .any(|(kind, value)| kind == UseCooldown::KIND && value.is_none())
    );
    assert_eq!(item_cooldown_group(&pearl), "minecraft:ender_pearl");
    state.set_item_cooldown("minecraft:ender_pearl".into(), 0);
    assert!(!state.is_on_item_cooldown(&pearl));
    assert!(state.is_on_item_cooldown(&stick));
}

#[test]
fn cooldown_replacement_zero_negative_and_expiry() {
    let stack = ItemStackData::new(ItemKind::Stone, 1);
    let mut state = InteractionState::new();
    assert!(!state.is_on_item_cooldown(&stack));
    state.set_item_cooldown("minecraft:stone".into(), 3);
    state.tick_player_state(false, None);
    assert!(state.is_on_item_cooldown(&stack));
    state.set_item_cooldown("minecraft:stone".into(), 1);
    assert!(state.is_on_item_cooldown(&stack));
    state.tick_player_state(true, None);
    assert!(!state.is_on_item_cooldown(&stack));
    state.set_item_cooldown("minecraft:stone".into(), 1);
    state.set_item_cooldown("minecraft:stone".into(), 3);
    for _ in 0..2 {
        state.tick_player_state(true, None);
        assert!(state.is_on_item_cooldown(&stack));
    }
    state.tick_player_state(true, None);
    assert!(!state.is_on_item_cooldown(&stack));
    state.set_item_cooldown("minecraft:stone".into(), 5);
    state.set_item_cooldown("minecraft:stone".into(), 0);
    assert!(state.item_cooldowns.is_empty());
    for duration in [-1, i32::MIN] {
        state.set_item_cooldown("minecraft:stone".into(), 5);
        state.set_item_cooldown("minecraft:stone".into(), duration);
        assert!(!state.is_on_item_cooldown(&stack));
        assert_eq!(
            state.item_cooldowns.len(),
            1,
            "nonzero replaces the interval"
        );
        state.tick_player_state(true, None);
        assert!(state.item_cooldowns.is_empty());
    }
}

#[test]
fn cooldown_live_dead_final_tick_and_fresh_player_reset() {
    let mut state = InteractionState::new();
    let stack = ItemStackData::new(ItemKind::Stone, 1);
    state.set_item_cooldown("minecraft:stone".into(), 3);
    state.tick_player_state(false, None);
    assert_eq!(state.cooldown_tick, 1);
    state.tick_dead_player_state(None, true);
    assert_eq!(state.cooldown_tick, 2);
    assert!(state.is_on_item_cooldown(&stack));
    state.tick_dead_player_state(None, false);
    assert_eq!(state.cooldown_tick, 3);
    assert!(!state.is_on_item_cooldown(&stack));
    state.set_item_cooldown("minecraft:stone".into(), 10);
    state.seq = 7;
    state.reset_player_transients_for_respawn();
    assert_eq!(state.cooldown_tick, 0);
    assert!(state.item_cooldowns.is_empty());
    assert_eq!(state.seq, 7, "reset only player-owned transients");
    assert!(InteractionState::new().item_cooldowns.is_empty());
}

fn use_with_cooldown(
    kind: ItemKind,
    block: Option<&str>,
    active: bool,
) -> (bool, Vec<ServerboundGamePacket>, bool, bool) {
    let registry = BlockRegistry::native();
    let table = registry.block_state_table();
    let chunks = ChunkStore::with_origin_chunk();
    let hit = BlockHitResult {
        block_pos: BlockPos::new(1, 64, 1),
        face: Direction::Up,
        hit_point: dvec3(1.5, 65.0, 1.5),
    };
    let mut state = InteractionState::new();
    if let Some(block) = block {
        chunks.set_block_state(1, 64, 1, table.first_state_of(block).unwrap(), table);
        state.target = Some(HitResult::Block(hit));
    }
    let main = ItemStackData::new(kind, 1);
    let off = ItemStackData::new(ItemKind::Snowball, 1);
    if active {
        state.set_item_cooldown(item_cooldown_group(&main), 20);
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sender = PacketSender::new(tx);
    let audio = AudioEngine::silent_for_test();
    let colormap = Arc::new(Colormap::fallback());
    let mut particles = ParticleStore::new(
        AtlasUVMap::for_test(),
        colormap.clone(),
        colormap.clone(),
        colormap,
    );
    let biome_climate = HashMap::new();
    let mut effects = BreakEffects {
        particles: &mut particles,
        registry: registry.render_view(),
        biome_climate: &biome_climate,
    };
    let player_pos = dvec3(1.5, 64.0, 3.5);
    let mut dirty = Vec::new();
    let used = state.start_use_item(
        &registry,
        &sender,
        &audio,
        &chunks,
        player_pos,
        Aabb::from_center(player_pos, 0.3, 0.9),
        player_pos + dvec3(0.0, 1.62, 0.0),
        LookDirection::default(),
        [
            HandItem {
                stack: Some(&main),
                place_block: (kind == ItemKind::Stone)
                    .then(|| table.first_state_of("stone").unwrap()),
                worn: Worn::default(),
            },
            HandItem {
                stack: Some(&off),
                place_block: None,
                worn: Worn::default(),
            },
        ],
        10,
        false,
        false,
        &mut effects,
        &mut dirty,
    );
    assert_eq!(
        state.item_cooldowns.len(),
        usize::from(active),
        "using an item cannot create local cooldowns"
    );
    assert_eq!(
        state.cooldown_tick, 0,
        "actions do not advance the player clock"
    );
    let mut packets = Vec::new();
    while let Ok(outbound) = rx.try_recv() {
        let Outbound::Packet(packet) = outbound else {
            panic!("structured use packet expected");
        };
        packets.push(*packet);
    }
    let placed = chunks.get_block_state(1, 65, 1) == table.first_state_of("stone").unwrap();
    assert_eq!(placed, !dirty.is_empty());
    (used, packets, placed, state.using_item.is_some())
}

#[test]
fn cooldown_gate_placement_pass_preserves_offhand_fallback() {
    let (used, packets, placed, using) = use_with_cooldown(ItemKind::Stone, Some("stone"), true);
    assert!(
        !placed && !using,
        "cooldown must prevent local placement/use"
    );
    assert!(used, "offhand snowball must still succeed");
    assert!(
        matches!(packets.as_slice(), [
        ServerboundGamePacket::UseItemOn(a), ServerboundGamePacket::UseItem(b),
        ServerboundGamePacket::UseItemOn(c), ServerboundGamePacket::UseItem(d),
        ServerboundGamePacket::Swing(e),
    ] if a.hand == HANDS[0] && a.seq == 1 && b.hand == HANDS[0] && b.seq == 2
        && c.hand == HANDS[1] && c.seq == 3 && d.hand == HANDS[1] && d.seq == 4 && e.hand == HANDS[1]),
        "{packets:?}"
    );
}

#[test]
fn cooldown_gate_food_equip_and_bundle_pass_after_packet() {
    for kind in [ItemKind::Apple, ItemKind::DiamondHelmet, ItemKind::Bundle] {
        let (used, packets, placed, using) = use_with_cooldown(kind, None, true);
        assert!(
            used && !placed && !using,
            "{kind:?}: cooldown cannot start local use"
        );
        assert!(
            matches!(packets.as_slice(), [ServerboundGamePacket::UseItem(a), ServerboundGamePacket::UseItem(b), ServerboundGamePacket::Swing(c)]
            if a.hand == HANDS[0] && a.seq == 1 && b.hand == HANDS[1] && b.seq == 2 && c.hand == HANDS[1]),
            "{kind:?}: {packets:?}"
        );
        let (_, packets, _, _) = use_with_cooldown(kind, None, false);
        assert!(
            matches!(packets.first(), Some(ServerboundGamePacket::UseItem(p)) if p.hand == HANDS[0])
        );
        assert!(
            !packets
                .iter()
                .any(|p| matches!(p, ServerboundGamePacket::UseItem(p) if p.hand == HANDS[1])),
            "without cooldown main hand succeeds"
        );
    }
}

#[test]
fn cooldown_gate_does_not_suppress_menu_or_ordinary_placement() {
    let (used, packets, placed, _) =
        use_with_cooldown(ItemKind::Stone, Some("crafting_table"), true);
    assert!(used && !placed);
    assert!(
        matches!(packets.as_slice(), [ServerboundGamePacket::UseItemOn(a), ServerboundGamePacket::Swing(b)] if a.hand == HANDS[0] && b.hand == HANDS[0])
    );
    let (used, packets, placed, _) = use_with_cooldown(ItemKind::Stone, Some("stone"), false);
    assert!(used && placed);
    assert!(matches!(
        packets.as_slice(),
        [
            ServerboundGamePacket::UseItemOn(_),
            ServerboundGamePacket::Swing(_)
        ]
    ));
}
