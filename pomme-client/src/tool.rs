use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use azalea_inventory::ItemStackData;
use azalea_inventory::components::{CustomData, Tool as WireTool};
use azalea_registry::builtin::{BlockKind, DataComponentKind, ItemKind};
use azalea_registry::identifier::Identifier;
use azalea_registry::{HolderSet, Registry};
use pomme_protocol::version::NATIVE;

use crate::world::block::BlockTags;

/// First protocol whose shears use the `shears_*_breaking_speed` tags (26.2).
const SHEARS_SPEED_TAGS_PROTOCOL: i32 = 776;

const LEGACY_EFFICIENCY_KEY: &str = "pomme:legacy_efficiency";
const LEGACY_AQUA_AFFINITY_KEY: &str = "pomme:legacy_aqua_affinity";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyMiningEnchantments {
    pub efficiency: i32,
    pub aqua_affinity: bool,
}

/// A stack's mining enchantments before 1.21: the `Enchantments` NBT list in
/// CustomData before 1.20.5, Pomme's translated metadata keys on 1.20.5/6.
fn legacy_mining_enchantments(stack: &ItemStackData, protocol: i32) -> LegacyMiningEnchantments {
    if protocol > 766 {
        return LegacyMiningEnchantments::default();
    }
    let Some(custom) = stack.component_patch.get::<CustomData>() else {
        return LegacyMiningEnchantments::default();
    };

    if protocol == 766 {
        return LegacyMiningEnchantments {
            efficiency: custom.nbt.int(LEGACY_EFFICIENCY_KEY).unwrap_or_default(),
            aqua_affinity: custom
                .nbt
                .byte(LEGACY_AQUA_AFFINITY_KEY)
                .unwrap_or_default()
                != 0,
        };
    }

    let mut result = LegacyMiningEnchantments::default();
    let Some(enchantments) = custom
        .nbt
        .list("Enchantments")
        .and_then(|list| list.compounds())
    else {
        return result;
    };
    for enchantment in enchantments {
        let Some(id) = enchantment.string("id") else {
            continue;
        };
        let level = enchantment
            .short("lvl")
            .map(i32::from)
            .or_else(|| enchantment.int("lvl"))
            .unwrap_or_default();
        match id.to_str().as_ref() {
            "minecraft:efficiency" if level > 0 => result.efficiency = result.efficiency.max(level),
            "minecraft:aqua_affinity" if level > 0 => result.aqua_affinity = true,
            _ => {}
        }
    }
    result
}

/// The mining enchantments `Player.getDestroySpeed` reads directly before
/// 1.21: Efficiency from the main hand (`EquipmentSlot.MAINHAND`) and Aqua
/// Affinity from the armor (`ARMOR_SLOTS`).
pub fn legacy_mining<'a>(
    held: Option<&ItemStackData>,
    armor: impl IntoIterator<Item = &'a ItemStackData>,
    protocol: i32,
) -> LegacyMiningEnchantments {
    LegacyMiningEnchantments {
        efficiency: held.map_or(0, |stack| {
            legacy_mining_enchantments(stack, protocol).efficiency
        }),
        aqua_affinity: armor
            .into_iter()
            .any(|stack| legacy_mining_enchantments(stack, protocol).aqua_affinity),
    }
}

/// Before 1.21.5's `sword_instantly_mines` tag, `BambooStalkBlock` and
/// `BambooSaplingBlock.getDestroyProgress` returned 1 for any `SwordItem`,
/// bypassing the destroy-speed chain.
pub fn sword_instantly_mines_legacy(held: ItemKind, block: &str, protocol: i32) -> bool {
    protocol < 770
        && matches!(block, "bamboo" | "bamboo_sapling")
        && held.to_str().ends_with("_sword")
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolBlocks {
    Tag(String),
    Direct(Vec<String>),
}

impl ToolBlocks {
    fn contains(&self, block: &str, tags: &BlockTags) -> bool {
        match self {
            Self::Tag(tag) => tags.contains(tag, block),
            Self::Direct(blocks) => blocks.iter().any(|candidate| candidate == block),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolRule {
    pub blocks: ToolBlocks,
    pub speed: Option<f32>,
    pub correct_for_drops: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub rules: Vec<ToolRule>,
    pub default_mining_speed: f32,
    pub damage_per_block: i32,
    pub can_destroy_blocks_in_creative: bool,
    /// Pre-1.20.5 material tier used with the historical needs_*_tool tags.
    /// None for tools whose drop-correctness is fully described by rules.
    legacy_tier: Option<u8>,
}

impl Tool {
    fn new(
        rules: Vec<ToolRule>,
        damage_per_block: i32,
        can_destroy_blocks_in_creative: bool,
    ) -> Self {
        Self {
            rules,
            default_mining_speed: 1.0,
            damage_per_block,
            can_destroy_blocks_in_creative,
            legacy_tier: None,
        }
    }

    /// Vanilla `Tool.getMiningSpeed`.
    pub fn mining_speed(&self, block: &str, tags: &BlockTags) -> f32 {
        self.first_rule(block, tags, |rule| rule.speed)
            .unwrap_or(self.default_mining_speed)
    }

    pub fn correct_for_drops(&self, block: &str, tags: &BlockTags, protocol: i32) -> bool {
        // Before 1.20.5 there were no incorrect_for_*_tool tags. Vanilla's
        // DiggerItem checked the three needs_*_tool tags against the tier
        // level before accepting the mineable/* rule.
        if protocol <= 765
            && let Some(tier) = self.legacy_tier
            && ((tier < 3 && tags.contains("minecraft:needs_diamond_tool", block))
                || (tier < 2 && tags.contains("minecraft:needs_iron_tool", block))
                || (tier < 1 && tags.contains("minecraft:needs_stone_tool", block)))
        {
            return false;
        }

        self.first_rule(block, tags, |rule| rule.correct_for_drops)
            .unwrap_or(false)
    }

    /// The first rule with a value for `field` that covers the block; each
    /// field resolves independently.
    fn first_rule<T>(
        &self,
        block: &str,
        tags: &BlockTags,
        field: impl Fn(&ToolRule) -> Option<T>,
    ) -> Option<T> {
        self.rules
            .iter()
            .find_map(|rule| field(rule).filter(|_| rule.blocks.contains(block, tags)))
    }
}

fn tag(name: &str, speed: Option<f32>, correct_for_drops: Option<bool>) -> ToolRule {
    ToolRule {
        blocks: ToolBlocks::Tag(format!("minecraft:{name}")),
        speed,
        correct_for_drops,
    }
}

fn direct(blocks: &[&str], speed: Option<f32>, correct_for_drops: Option<bool>) -> ToolRule {
    ToolRule {
        blocks: ToolBlocks::Direct(blocks.iter().map(|block| (*block).to_owned()).collect()),
        speed,
        correct_for_drops,
    }
}

fn cobweb_rule() -> ToolRule {
    direct(&["cobweb"], Some(15.0), Some(true))
}

fn material_tool(speed: f32, incorrect_tag: &str, mineable_tag: &str, legacy_tier: u8) -> Tool {
    let mut tool = Tool::new(
        vec![
            tag(incorrect_tag, None, Some(false)),
            tag(mineable_tag, Some(speed), Some(true)),
        ],
        1,
        true,
    );
    tool.legacy_tier = Some(legacy_tier);
    tool
}

fn sword_tool() -> Tool {
    Tool::new(
        vec![
            cobweb_rule(),
            tag("sword_instantly_mines", Some(f32::MAX), None),
            tag("sword_efficient", Some(1.5), None),
        ],
        2,
        false,
    )
}

fn shears_tool() -> Tool {
    Tool::new(
        vec![
            cobweb_rule(),
            tag("shears_extreme_breaking_speed", Some(15.0), None),
            tag("shears_major_breaking_speed", Some(5.0), None),
            tag("shears_minor_breaking_speed", Some(2.0), None),
        ],
        1,
        true,
    )
}

/// Shears before 26.2: the same speeds keyed by the `leaves` and `wool` tags
/// plus a direct vine list.
fn legacy_shears_tool() -> Tool {
    Tool::new(
        vec![
            cobweb_rule(),
            tag("leaves", Some(15.0), None),
            tag("wool", Some(5.0), None),
            direct(&["vine", "glow_lichen"], Some(2.0), None),
        ],
        1,
        true,
    )
}

struct Material {
    item_prefix: &'static str,
    speed: f32,
    incorrect_tag: &'static str,
    legacy_tier: u8,
}

const MATERIALS: [Material; 7] = [
    Material {
        item_prefix: "wooden",
        speed: 2.0,
        incorrect_tag: "incorrect_for_wooden_tool",
        legacy_tier: 0,
    },
    Material {
        item_prefix: "stone",
        speed: 4.0,
        incorrect_tag: "incorrect_for_stone_tool",
        legacy_tier: 1,
    },
    Material {
        item_prefix: "copper",
        speed: 5.0,
        incorrect_tag: "incorrect_for_copper_tool",
        legacy_tier: 1,
    },
    Material {
        item_prefix: "iron",
        speed: 6.0,
        incorrect_tag: "incorrect_for_iron_tool",
        legacy_tier: 2,
    },
    Material {
        item_prefix: "diamond",
        speed: 8.0,
        incorrect_tag: "incorrect_for_diamond_tool",
        legacy_tier: 3,
    },
    Material {
        item_prefix: "golden",
        speed: 12.0,
        incorrect_tag: "incorrect_for_gold_tool",
        legacy_tier: 0,
    },
    Material {
        item_prefix: "netherite",
        speed: 9.0,
        incorrect_tag: "incorrect_for_netherite_tool",
        legacy_tier: 3,
    },
];

const TOOL_FAMILIES: [(&str, &str); 4] = [
    ("pickaxe", "mineable/pickaxe"),
    ("shovel", "mineable/shovel"),
    ("axe", "mineable/axe"),
    ("hoe", "mineable/hoe"),
];

static DEFAULT_TOOLS: LazyLock<HashMap<String, Tool>> = LazyLock::new(|| {
    let mut tools = HashMap::with_capacity(38);
    for material in MATERIALS {
        tools.insert(
            format!("minecraft:{}_sword", material.item_prefix),
            sword_tool(),
        );
        for (item_suffix, mineable_tag) in TOOL_FAMILIES {
            tools.insert(
                format!("minecraft:{}_{item_suffix}", material.item_prefix),
                material_tool(
                    material.speed,
                    material.incorrect_tag,
                    mineable_tag,
                    material.legacy_tier,
                ),
            );
        }
    }
    tools.insert("minecraft:shears".to_owned(), shears_tool());
    tools.insert("minecraft:mace".to_owned(), Tool::new(vec![], 2, false));
    tools.insert("minecraft:trident".to_owned(), Tool::new(vec![], 2, false));
    tools
});

static LEGACY_SHEARS: LazyLock<Tool> = LazyLock::new(legacy_shears_tool);

fn default_tool(kind: ItemKind, protocol: i32) -> Option<&'static Tool> {
    if kind == ItemKind::Shears && protocol < SHEARS_SPEED_TAGS_PROTOCOL {
        return Some(&LEGACY_SHEARS);
    }
    DEFAULT_TOOLS.get(kind.to_str())
}

fn block_name(kind: BlockKind) -> String {
    kind.to_str()
        .strip_prefix("minecraft:")
        .unwrap_or(kind.to_str())
        .to_owned()
}

fn wire_blocks(blocks: &HolderSet<BlockKind, Identifier>, protocol: i32) -> ToolBlocks {
    match blocks {
        HolderSet::Direct { contents } => ToolBlocks::Direct(
            contents
                .iter()
                .copied()
                .map(|kind| {
                    if protocol == NATIVE.protocol {
                        block_name(kind)
                    } else {
                        // azalea decoded the raw id as a native BlockKind;
                        // map it through that protocol's block registry.
                        crate::world::block::block_registry_name(protocol, kind.to_u32())
                            .map(str::to_owned)
                            .unwrap_or_else(|| block_name(kind))
                    }
                })
                .collect(),
        ),
        HolderSet::Named { key, .. } => ToolBlocks::Tag(key.to_string()),
    }
}

fn from_wire(tool: &WireTool, protocol: i32) -> Tool {
    Tool {
        rules: tool
            .rules
            .iter()
            .map(|rule| ToolRule {
                blocks: wire_blocks(&rule.blocks, protocol),
                speed: rule.speed,
                correct_for_drops: rule.correct_for_drops,
            })
            .collect(),
        default_mining_speed: tool.default_mining_speed,
        damage_per_block: tool.damage_per_block,
        can_destroy_blocks_in_creative: tool.can_destroy_blocks_in_creative,
        legacy_tier: None,
    }
}

/// The stack's effective Tool: its patch (a removal, or a value on 1.21.11+),
/// else Pomme's per-item default.
pub fn stack_tool(stack: &ItemStackData, protocol: i32) -> Option<Cow<'static, Tool>> {
    if let Some((_, patch_value)) = stack
        .component_patch
        .iter()
        .find(|(kind, _)| *kind == DataComponentKind::Tool)
    {
        patch_value?;
        // TODO: Tool values on 766-773 (1.20.5 through 1.21.10) are skipped,
        // so server-customised tools there mine at the item's default speed.
        if protocol >= 774 {
            return stack
                .component_patch
                .get::<WireTool>()
                .map(|tool| Cow::Owned(from_wire(tool, protocol)));
        }
    }

    default_tool(stack.kind, protocol).map(Cow::Borrowed)
}

#[cfg(test)]
mod tests {
    use azalea_inventory::components::{DataComponentUnion, ToolRule as WireToolRule};

    use super::*;

    fn test_tags() -> BlockTags {
        crate::world::block::init("26.2");
        crate::world::block::block_tags_for_test(&[
            ("mineable/pickaxe", &["stone", "obsidian"]),
            ("incorrect_for_iron_tool", &["obsidian"]),
        ])
    }

    #[test]
    fn native_rules_keep_vanilla_first_match_per_field_semantics() {
        let tags = test_tags();
        let tool = Tool {
            rules: vec![
                direct(&["obsidian"], None, Some(false)),
                direct(&["stone", "obsidian"], Some(4.0), Some(true)),
            ],
            default_mining_speed: 1.5,
            damage_per_block: 1,
            can_destroy_blocks_in_creative: true,
            legacy_tier: None,
        };

        assert_eq!(tool.mining_speed("stone", &tags), 4.0);
        assert!(tool.correct_for_drops("stone", &tags, NATIVE.protocol));
        assert_eq!(tool.mining_speed("obsidian", &tags), 4.0);
        assert!(!tool.correct_for_drops("obsidian", &tags, NATIVE.protocol));
        assert_eq!(tool.mining_speed("dirt", &tags), 1.5);
        assert!(!tool.correct_for_drops("dirt", &tags, NATIVE.protocol));
    }

    #[test]
    fn native_named_tag_rules_use_server_block_tags() {
        let tags = test_tags();
        let tool = material_tool(6.0, "incorrect_for_iron_tool", "mineable/pickaxe", 2);

        assert_eq!(tool.mining_speed("stone", &tags), 6.0);
        assert!(tool.correct_for_drops("stone", &tags, NATIVE.protocol));
        // The first rule denies drops for obsidian, while speed resolution
        // independently skips that speedless rule and uses the pickaxe rule.
        assert_eq!(tool.mining_speed("obsidian", &tags), 6.0);
        assert!(!tool.correct_for_drops("obsidian", &tags, NATIVE.protocol));
    }

    #[test]
    fn legacy_tiers_use_needs_tool_tags_before_1_20_5() {
        crate::world::block::init("26.2");
        let tags = crate::world::block::block_tags_for_test(&[
            (
                "mineable/pickaxe",
                &["stone", "iron_ore", "diamond_ore", "obsidian"],
            ),
            ("needs_stone_tool", &["iron_ore"]),
            ("needs_iron_tool", &["diamond_ore"]),
            ("needs_diamond_tool", &["obsidian"]),
        ]);
        let wooden = material_tool(2.0, "incorrect_for_wooden_tool", "mineable/pickaxe", 0);
        let stone = material_tool(4.0, "incorrect_for_stone_tool", "mineable/pickaxe", 1);
        let iron = material_tool(6.0, "incorrect_for_iron_tool", "mineable/pickaxe", 2);
        let diamond = material_tool(8.0, "incorrect_for_diamond_tool", "mineable/pickaxe", 3);

        assert!(wooden.correct_for_drops("stone", &tags, 765));
        assert!(!wooden.correct_for_drops("iron_ore", &tags, 765));
        assert!(stone.correct_for_drops("iron_ore", &tags, 765));
        assert!(!stone.correct_for_drops("diamond_ore", &tags, 765));
        assert!(iron.correct_for_drops("diamond_ore", &tags, 765));
        assert!(!iron.correct_for_drops("obsidian", &tags, 765));
        assert!(diamond.correct_for_drops("obsidian", &tags, 765));
    }

    /// A pre-1.20.5 stack with its `Enchantments` NBT list in CustomData.
    fn enchanted_stack(kind: ItemKind, enchantments: &[(&str, i16)]) -> ItemStackData {
        use simdnbt::owned::{Nbt, NbtCompound, NbtList, NbtTag};

        let list = enchantments
            .iter()
            .map(|(id, level)| {
                NbtCompound::from_values(vec![
                    (
                        "id".into(),
                        NbtTag::String(format!("minecraft:{id}").into()),
                    ),
                    ("lvl".into(), NbtTag::Short(*level)),
                ])
            })
            .collect();
        let root = NbtCompound::from_values(vec![(
            "Enchantments".into(),
            NbtTag::List(NbtList::Compound(list)),
        )]);
        let mut stack = ItemStackData::new(kind, 1);
        let custom = CustomData {
            nbt: Nbt::new("".into(), root),
        };
        // SAFETY: the union variant matches DataComponentKind::CustomData.
        unsafe {
            stack.component_patch.unchecked_insert_component(
                DataComponentKind::CustomData,
                Some(DataComponentUnion::from(custom)),
            );
        }
        stack
    }

    #[test]
    fn legacy_nbt_enchantments_feed_mining_metadata() {
        let stack = enchanted_stack(
            ItemKind::IronPickaxe,
            &[("efficiency", 3), ("aqua_affinity", 1)],
        );
        assert_eq!(
            legacy_mining_enchantments(&stack, 765),
            LegacyMiningEnchantments {
                efficiency: 3,
                aqua_affinity: true,
            }
        );
    }

    #[test]
    fn legacy_mining_reads_efficiency_from_hand_and_aqua_affinity_from_armor() {
        let held = enchanted_stack(
            ItemKind::IronPickaxe,
            &[("efficiency", 2), ("aqua_affinity", 1)],
        );
        let helmet = enchanted_stack(ItemKind::IronHelmet, &[("aqua_affinity", 1)]);

        assert_eq!(
            legacy_mining(Some(&held), [], 765),
            LegacyMiningEnchantments {
                efficiency: 2,
                aqua_affinity: false,
            }
        );
        assert!(legacy_mining(None, [&helmet], 765).aqua_affinity);
        assert_eq!(
            legacy_mining(Some(&held), [&helmet], 767),
            LegacyMiningEnchantments::default()
        );
    }

    #[test]
    fn shears_use_leaves_and_wool_tags_before_26_2() {
        crate::world::block::init("26.2");
        let tags = crate::world::block::block_tags_for_test(&[
            ("leaves", &["oak_leaves"]),
            ("wool", &["white_wool"]),
        ]);
        let shears = default_tool(ItemKind::Shears, 775).expect("26.1 shears");
        assert_eq!(*shears, legacy_shears_tool());
        assert_eq!(shears.mining_speed("cobweb", &tags), 15.0);
        assert_eq!(shears.mining_speed("oak_leaves", &tags), 15.0);
        assert_eq!(shears.mining_speed("white_wool", &tags), 5.0);
        assert_eq!(shears.mining_speed("vine", &tags), 2.0);
        assert_eq!(shears.mining_speed("stone", &tags), 1.0);

        assert_eq!(
            *default_tool(ItemKind::Shears, NATIVE.protocol).expect("26.2 shears"),
            shears_tool()
        );
    }

    #[test]
    fn swords_instantly_mine_bamboo_before_1_21_5() {
        for block in ["bamboo", "bamboo_sapling"] {
            assert!(sword_instantly_mines_legacy(
                ItemKind::IronSword,
                block,
                769
            ));
            assert!(!sword_instantly_mines_legacy(
                ItemKind::IronSword,
                block,
                770
            ));
            assert!(!sword_instantly_mines_legacy(ItemKind::IronAxe, block, 769));
        }
        assert!(!sword_instantly_mines_legacy(
            ItemKind::IronSword,
            "stone",
            769
        ));
    }

    #[test]
    fn translated_protocols_only_trust_layout_compatible_tool_patches() {
        let mut stack = ItemStackData::new(ItemKind::IronPickaxe, 1);
        unsafe {
            stack
                .component_patch
                .unchecked_insert_component(DataComponentKind::Tool, None);
        }
        assert!(stack_tool(&stack, NATIVE.protocol).is_none());
        assert!(stack_tool(&stack, 775).is_none());
        assert!(stack_tool(&stack, 774).is_none());
        assert!(stack_tool(&stack, 773).is_none());
        assert!(stack_tool(&stack, 767).is_none());
        assert!(stack_tool(&stack, 766).is_none());
    }

    #[test]
    fn direct_holder_ids_use_the_negotiated_block_registry() {
        let raw_old_id = 998u32;
        assert_eq!(
            crate::world::block::block_registry_name(775, raw_old_id),
            Some("calcite")
        );
        let latest_interpretation = BlockKind::from_u32(raw_old_id).expect("latest block id");
        assert_eq!(block_name(latest_interpretation), "sulfur");

        let mut stack = ItemStackData::new(ItemKind::IronPickaxe, 1);
        let wire = WireTool {
            rules: vec![WireToolRule {
                blocks: HolderSet::Direct {
                    contents: vec![latest_interpretation],
                },
                speed: Some(17.0),
                correct_for_drops: Some(true),
            }],
            default_mining_speed: 1.0,
            damage_per_block: 1,
            can_destroy_blocks_in_creative: true,
        };
        unsafe {
            stack.component_patch.unchecked_insert_component(
                DataComponentKind::Tool,
                Some(DataComponentUnion::from(wire)),
            );
        }

        let tool = stack_tool(&stack, 775).expect("26.1 Tool patch");
        let tags = test_tags();
        assert_eq!(tool.mining_speed("calcite", &tags), 17.0);
        assert_eq!(tool.mining_speed("sulfur", &tags), 1.0);
    }

    #[test]
    fn explicit_tool_removal_suppresses_native_default() {
        let mut stack = ItemStackData::new(ItemKind::IronPickaxe, 1);
        // SAFETY: a None payload represents removal and carries no union value.
        unsafe {
            stack
                .component_patch
                .unchecked_insert_component(DataComponentKind::Tool, None);
        }
        assert!(stack_tool(&stack, NATIVE.protocol).is_none());
    }

    #[test]
    fn explicit_wire_tool_override_translates_once_to_native_rules() {
        let mut stack = ItemStackData::new(ItemKind::IronPickaxe, 1);
        let wire = WireTool {
            rules: vec![WireToolRule {
                blocks: HolderSet::Named {
                    key: Identifier::new("minecraft:mineable/pickaxe"),
                    contents: vec![],
                },
                speed: Some(23.0),
                correct_for_drops: Some(true),
            }],
            default_mining_speed: 2.0,
            damage_per_block: 7,
            can_destroy_blocks_in_creative: false,
        };
        // SAFETY: the union variant matches DataComponentKind::Tool.
        unsafe {
            stack.component_patch.unchecked_insert_component(
                DataComponentKind::Tool,
                Some(DataComponentUnion::from(wire)),
            );
        }

        let tool = stack_tool(&stack, NATIVE.protocol).expect("explicit tool override");
        let tags = test_tags();
        assert_eq!(tool.mining_speed("stone", &tags), 23.0);
        assert_eq!(tool.default_mining_speed, 2.0);
        assert_eq!(tool.damage_per_block, 7);
        assert!(!tool.can_destroy_blocks_in_creative);
    }

    #[test]
    fn vanilla_26_2_default_tool_set_is_complete() {
        assert_eq!(DEFAULT_TOOLS.len(), 38);
        for kind in [
            ItemKind::WoodenSword,
            ItemKind::StoneShovel,
            ItemKind::CopperPickaxe,
            ItemKind::IronAxe,
            ItemKind::DiamondHoe,
            ItemKind::GoldenPickaxe,
            ItemKind::NetheriteSword,
            ItemKind::Shears,
            ItemKind::Mace,
            ItemKind::Trident,
        ] {
            assert!(
                default_tool(kind, NATIVE.protocol).is_some(),
                "missing {kind:?}"
            );
        }
    }

    #[test]
    fn sword_special_rules_match_vanilla_shape() {
        let tool = default_tool(ItemKind::IronSword, NATIVE.protocol).expect("iron sword tool");
        assert_eq!(tool.rules.len(), 3);
        assert_eq!(tool.damage_per_block, 2);
        assert!(!tool.can_destroy_blocks_in_creative);
        assert_eq!(tool.default_mining_speed, 1.0);
        assert_eq!(tool.rules[0], cobweb_rule());
    }

    #[test]
    fn mace_and_trident_cannot_destroy_blocks_in_creative() {
        for kind in [ItemKind::Mace, ItemKind::Trident] {
            let tool = default_tool(kind, NATIVE.protocol).expect("special tool");
            assert!(tool.rules.is_empty());
            assert_eq!(tool.damage_per_block, 2);
            assert!(!tool.can_destroy_blocks_in_creative);
        }
    }
}
