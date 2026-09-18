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

const LEGACY_EFFICIENCY_KEY: &str = "pomme:legacy_efficiency";
const LEGACY_AQUA_AFFINITY_KEY: &str = "pomme:legacy_aqua_affinity";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyMiningEnchantments {
    pub efficiency: i32,
    pub aqua_affinity: bool,
}

/// Mining enchantments that were still read directly from item state before
/// 1.21 moved them into synced player attributes. Pre-1.20.5 stacks keep the
/// original `Enchantments` NBT list in CustomData; 1.20.5/1.20.6 stack
/// translation writes only the two mining-relevant values as Pomme metadata.
pub fn legacy_mining_enchantments(
    stack: &ItemStackData,
    protocol: i32,
) -> LegacyMiningEnchantments {
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

    pub fn mining_speed(&self, block: &str, tags: &BlockTags) -> f32 {
        self.rules
            .iter()
            .find_map(|rule| rule.speed.filter(|_| rule.blocks.contains(block, tags)))
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

        self.rules
            .iter()
            .find_map(|rule| {
                rule.correct_for_drops
                    .filter(|_| rule.blocks.contains(block, tags))
            })
            .unwrap_or(false)
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
            direct(&["cobweb"], Some(15.0), Some(true)),
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
            direct(&["cobweb"], Some(15.0), Some(true)),
            tag("shears_extreme_breaking_speed", Some(15.0), None),
            tag("shears_major_breaking_speed", Some(5.0), None),
            tag("shears_minor_breaking_speed", Some(2.0), None),
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

fn default_tool(kind: ItemKind) -> Option<&'static Tool> {
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
                        // Tool's component id is layout-compatible on 1.21.11
                        // and 26.1, but direct HolderSet block ids are still in
                        // that protocol's block registry. Azalea decoded the raw
                        // integer as a latest BlockKind, so reinterpret its
                        // numeric discriminant through Pomme's protocol table.
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

/// Resolve the effective Tool component for a stack without consulting
/// Azalea's generated default-component tables.
///
/// Explicit Tool patches are converted into Pomme's native model on lookup
/// when the negotiated protocol uses Tool's current component id (1.21.11+).
/// Direct holder block ids are resolved through Pomme's protocol-specific
/// registry table. On protocols 766-773 Tool value payloads are deliberately
/// skipped by the compatibility layer and gameplay uses Pomme's native per-item
/// default; payload-free Tool removals are normalized and preserved exactly.
pub fn stack_tool(stack: &ItemStackData, protocol: i32) -> Option<Cow<'static, Tool>> {
    if let Some((_, patch_value)) = stack
        .component_patch
        .iter()
        .find(|(kind, _)| *kind == DataComponentKind::Tool)
    {
        // The component-era compatibility translators normalize payload-free
        // Tool removals even when Tool value payloads are intentionally skipped.
        if (766..=773).contains(&protocol) && patch_value.is_none() {
            return None;
        }
        if protocol >= 774 {
            return match patch_value {
                Some(_) => stack
                    .component_patch
                    .get::<WireTool>()
                    .map(|tool| Cow::Owned(from_wire(tool, protocol))),
                None => None,
            };
        }
    }

    default_tool(stack.kind).map(Cow::Borrowed)
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

    #[test]
    fn legacy_nbt_enchantments_feed_mining_metadata() {
        use simdnbt::owned::{Nbt, NbtCompound, NbtList, NbtTag};

        let efficiency = NbtCompound::from_values(vec![
            ("id".into(), NbtTag::String("minecraft:efficiency".into())),
            ("lvl".into(), NbtTag::Short(3)),
        ]);
        let aqua = NbtCompound::from_values(vec![
            (
                "id".into(),
                NbtTag::String("minecraft:aqua_affinity".into()),
            ),
            ("lvl".into(), NbtTag::Short(1)),
        ]);
        let root = NbtCompound::from_values(vec![(
            "Enchantments".into(),
            NbtTag::List(NbtList::Compound(vec![efficiency, aqua])),
        )]);
        let mut stack = ItemStackData::new(ItemKind::IronPickaxe, 1);
        let custom = CustomData {
            nbt: Nbt::new("".into(), root),
        };
        unsafe {
            stack.component_patch.unchecked_insert_component(
                DataComponentKind::CustomData,
                Some(DataComponentUnion::from(custom)),
            );
        }

        assert_eq!(
            legacy_mining_enchantments(&stack, 765),
            LegacyMiningEnchantments {
                efficiency: 3,
                aqua_affinity: true,
            }
        );
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
            assert!(default_tool(kind).is_some(), "missing {kind:?}");
        }
    }

    #[test]
    fn sword_special_rules_match_vanilla_shape() {
        let tool = default_tool(ItemKind::IronSword).expect("iron sword tool");
        assert_eq!(tool.rules.len(), 3);
        assert_eq!(tool.damage_per_block, 2);
        assert!(!tool.can_destroy_blocks_in_creative);
        assert_eq!(tool.default_mining_speed, 1.0);
        assert_eq!(tool.rules[0], direct(&["cobweb"], Some(15.0), Some(true)));
    }

    #[test]
    fn mace_and_trident_cannot_destroy_blocks_in_creative() {
        for kind in [ItemKind::Mace, ItemKind::Trident] {
            let tool = default_tool(kind).expect("special tool");
            assert!(tool.rules.is_empty());
            assert_eq!(tool.damage_per_block, 2);
            assert!(!tool.can_destroy_blocks_in_creative);
        }
    }
}
