use std::collections::HashMap;

use azalea_registry::builtin::BlockEntityKind;
use simdnbt::owned::NbtCompound;

use crate::position::BlockPos;
use crate::renderer::chunk::block_lighter::LightLevels;
use crate::world::block::BlockStateData;
#[derive(Clone)]
pub struct StoredBlockEntity {
    pub kind: BlockEntityKind,
    pub nbt: NbtCompound,
}

/// Blocks the block-entity pipeline draws in place of chunk geometry. The
/// chunk mesher skips these (their block models are particle-texture-only,
/// which would otherwise fall back to a full cube of that texture); other
/// block-entity blocks either have real block models or placeholder cubes.
pub fn rendered_kind(name: &str) -> Option<BlockEntityKind> {
    match name {
        "chest" => Some(BlockEntityKind::Chest),
        "trapped_chest" => Some(BlockEntityKind::TrappedChest),
        "ender_chest" => Some(BlockEntityKind::EnderChest),
        // Copper chests share vanilla's `chest` block entity type; the
        // weathering stage only picks the texture.
        s if s.ends_with("copper_chest") => Some(BlockEntityKind::Chest),
        s if s == "shulker_box" || s.ends_with("_shulker_box") => Some(BlockEntityKind::ShulkerBox),
        _ => None,
    }
}

/// Kinds [`rendered_kind`] synthesizes entries for; used to detect entries
/// gone stale when the block at their position stops mapping to them.
fn is_rendered(kind: BlockEntityKind) -> bool {
    matches!(
        kind,
        BlockEntityKind::Chest
            | BlockEntityKind::TrappedChest
            | BlockEntityKind::EnderChest
            | BlockEntityKind::ShulkerBox
    )
}

/// Sync the client-side entry for `pos` after a block update. The server sends
/// no block-entity data for e.g. a freshly placed chest, so blocks the BE
/// pipeline renders get an entry synthesized from the block state (vanilla
/// creates the client `BlockEntity` from the state the same way); a position
/// whose block is no longer a block entity drops its stale entry.
pub fn sync_block_entity(
    map: &mut HashMap<BlockPos, StoredBlockEntity>,
    pos: BlockPos,
    block_state_data: &BlockStateData,
) {
    let id = block_state_data.id();
    if let Some(kind) = rendered_kind(id) {
        if map.get(&pos).is_none_or(|e| e.kind != kind) {
            map.insert(
                pos,
                StoredBlockEntity {
                    kind,
                    nbt: NbtCompound::default(),
                },
            );
        }
    } else if !is_block_entity_block(id) || map.get(&pos).is_some_and(|e| is_rendered(e.kind)) {
        // A synthesized entry is also stale when the block swaps directly to a
        // different block-entity block (e.g. /setblock chest -> sign).
        map.remove(&pos);
    }
}

/// Blocks vanilla backs with a block entity. Used to suppress missing-model
/// warnings and to detect stale block-entity map entries; the subset the BE
/// pipeline actually draws is [`rendered_kind`].
pub fn is_block_entity_block(name: &str) -> bool {
    rendered_kind(name).is_some()
        || matches!(name, |"oak_sign"| "spruce_sign"
            | "birch_sign"
            | "jungle_sign"
            | "acacia_sign"
            | "dark_oak_sign"
            | "mangrove_sign"
            | "cherry_sign"
            | "pale_oak_sign"
            | "bamboo_sign"
            | "crimson_sign"
            | "warped_sign"
            | "oak_wall_sign"
            | "spruce_wall_sign"
            | "birch_wall_sign"
            | "jungle_wall_sign"
            | "acacia_wall_sign"
            | "dark_oak_wall_sign"
            | "mangrove_wall_sign"
            | "cherry_wall_sign"
            | "pale_oak_wall_sign"
            | "bamboo_wall_sign"
            | "crimson_wall_sign"
            | "warped_wall_sign"
            | "oak_hanging_sign"
            | "spruce_hanging_sign"
            | "birch_hanging_sign"
            | "jungle_hanging_sign"
            | "acacia_hanging_sign"
            | "dark_oak_hanging_sign"
            | "mangrove_hanging_sign"
            | "cherry_hanging_sign"
            | "pale_oak_hanging_sign"
            | "bamboo_hanging_sign"
            | "crimson_hanging_sign"
            | "warped_hanging_sign"
            | "oak_wall_hanging_sign"
            | "spruce_wall_hanging_sign"
            | "birch_wall_hanging_sign"
            | "jungle_wall_hanging_sign"
            | "acacia_wall_hanging_sign"
            | "dark_oak_wall_hanging_sign"
            | "mangrove_wall_hanging_sign"
            | "cherry_wall_hanging_sign"
            | "pale_oak_wall_hanging_sign"
            | "bamboo_wall_hanging_sign"
            | "crimson_wall_hanging_sign"
            | "warped_wall_hanging_sign"
            | "white_banner"
            | "orange_banner"
            | "magenta_banner"
            | "light_blue_banner"
            | "yellow_banner"
            | "lime_banner"
            | "pink_banner"
            | "gray_banner"
            | "light_gray_banner"
            | "cyan_banner"
            | "purple_banner"
            | "blue_banner"
            | "brown_banner"
            | "green_banner"
            | "red_banner"
            | "black_banner"
            | "white_wall_banner"
            | "orange_wall_banner"
            | "magenta_wall_banner"
            | "light_blue_wall_banner"
            | "yellow_wall_banner"
            | "lime_wall_banner"
            | "pink_wall_banner"
            | "gray_wall_banner"
            | "light_gray_wall_banner"
            | "cyan_wall_banner"
            | "purple_wall_banner"
            | "blue_wall_banner"
            | "brown_wall_banner"
            | "green_wall_banner"
            | "red_wall_banner"
            | "black_wall_banner"
            | "skeleton_skull"
            | "skeleton_wall_skull"
            | "wither_skeleton_skull"
            | "wither_skeleton_wall_skull"
            | "zombie_head"
            | "zombie_wall_head"
            | "player_head"
            | "player_wall_head"
            | "creeper_head"
            | "creeper_wall_head"
            | "dragon_head"
            | "dragon_wall_head"
            | "piglin_head"
            | "piglin_wall_head"
            | "conduit"
            | "decorated_pot"
            | "end_portal"
            | "end_gateway"
            | "beacon"
            | "spawner"
            | "trial_spawner"
            | "vault"
            | "brewing_stand"
            | "lectern"
            | "campfire"
            | "soul_campfire"
            | "beehive"
            | "bee_nest"
            | "bell"
            | "suspicious_sand"
            | "suspicious_gravel"
            | "crafter")
}

/// Vanilla `RenderShape.INVISIBLE`: blocks with no model of their own.
pub fn is_invisible_block(name: &str) -> bool {
    matches!(
        name,
        "air"
            | "cave_air"
            | "void_air"
            | "barrier"
            | "light"
            | "structure_void"
            | "moving_piston"
            | "water"
            | "lava"
            | "bubble_column"
            | "end_portal"
            | "end_gateway"
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignLineData {
    pub text: String,
    pub color_rgb: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignSideData {
    pub messages: [SignLineData; 4],
    pub color_rgb: u32,
    pub glowing: bool,
}

impl SignSideData {
    pub fn is_empty(&self) -> bool {
        self.messages.iter().all(|l| l.text.is_empty())
    }
}

#[derive(Clone, Debug)]
pub struct SignRenderInfo {
    pub pos: BlockPos,
    pub is_wall: bool,
    pub is_hanging: bool,
    pub angle: f32,
    pub front: SignSideData,
    pub back: SignSideData,
    pub light: LightLevels,
}

pub fn dye_color_rgb(name: &str) -> Option<u32> {
    match name {
        "white" => Some(0xFFFFFF),
        "orange" => Some(0xFF681F),
        "magenta" => Some(0xFF00FF),
        "light_blue" => Some(0x9AC0CD),
        "yellow" => Some(0xFFFF00),
        "lime" => Some(0xBFFF00),
        "pink" => Some(0xFF69B4),
        "gray" => Some(0x808080),
        "light_gray" => Some(0xD3D3D3),
        "cyan" => Some(0x00FFFF),
        "purple" => Some(0xA020F0),
        "blue" => Some(0x0000FF),
        "brown" => Some(0x8B4513),
        "green" => Some(0x00FF00),
        "red" => Some(0xFF0000),
        "black" => Some(0x000000),
        _ => None,
    }
}

pub fn named_color_rgb(name: &str) -> Option<u32> {
    match name {
        "black" => Some(0x000000),
        "dark_blue" => Some(0x0000AA),
        "dark_green" => Some(0x00AA00),
        "dark_aqua" => Some(0x00AAAA),
        "dark_red" => Some(0xAA0000),
        "dark_purple" => Some(0xAA00AA),
        "gold" => Some(0xFFAA00),
        "gray" => Some(0xAAAAAA),
        "dark_gray" => Some(0x555555),
        "blue" => Some(0x5555FF),
        "green" => Some(0x55FF55),
        "aqua" => Some(0x55FFFF),
        "red" => Some(0xFF5555),
        "light_purple" => Some(0xFF55FF),
        "yellow" => Some(0xFFFF55),
        "white" => Some(0xFFFFFF),
        _ => None,
    }
}

pub fn code_color_rgb(code: char) -> Option<u32> {
    match code.to_ascii_lowercase() {
        '0' => Some(0x000000),
        '1' => Some(0x0000AA),
        '2' => Some(0x00AA00),
        '3' => Some(0x00AAAA),
        '4' => Some(0xAA0000),
        '5' => Some(0xAA00AA),
        '6' => Some(0xFFAA00),
        '7' => Some(0xAAAAAA),
        '8' => Some(0x555555),
        '9' => Some(0x5555FF),
        'a' => Some(0x55FF55),
        'b' => Some(0x55FFFF),
        'c' => Some(0xFF5555),
        'd' => Some(0xFF55FF),
        'e' => Some(0xFFFF55),
        'f' => Some(0xFFFFFF),
        _ => None,
    }
}

fn extract_json_text(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
                || (trimmed.starts_with('[') && trimmed.ends_with(']')))
                && let Ok(nested) = serde_json::from_str::<serde_json::Value>(trimmed)
            {
                extract_json_text(&nested, out);
                return;
            }
            out.push_str(s);
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_json_text(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(t) = map.get("text") {
                extract_json_text(t, out);
            }
            if let Some(extra) = map.get("extra") {
                extract_json_text(extra, out);
            }
            if let Some(t) = map.get("translate").and_then(|v| v.as_str())
                && map.get("text").is_none()
            {
                out.push_str(t);
            }
        }
        _ => {}
    }
}

fn extract_json_color(v: &serde_json::Value) -> Option<u32> {
    match v {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
                || (trimmed.starts_with('[') && trimmed.ends_with(']')))
                && let Ok(nested) = serde_json::from_str::<serde_json::Value>(trimmed)
            {
                return extract_json_color(&nested);
            }
            None
        }
        serde_json::Value::Object(map) => {
            if let Some(c) = map.get("color").and_then(|v| v.as_str()) {
                if let Some(rgb) = dye_color_rgb(c).or_else(|| named_color_rgb(c)) {
                    return Some(rgb);
                }
                if let Some(hex) = c.strip_prefix('#')
                    && let Ok(rgb) = u32::from_str_radix(hex, 16)
                {
                    return Some(rgb);
                }
            }
            if let Some(extra) = map.get("extra")
                && let Some(rgb) = extract_json_color(extra)
            {
                return Some(rgb);
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(rgb) = extract_json_color(item) {
                    return Some(rgb);
                }
            }
            None
        }
        _ => None,
    }
}

fn extract_first_code_color(text: &str) -> (String, Option<u32>) {
    let mut out = String::with_capacity(text.len());
    let mut color = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '§'
            && let Some(&code) = chars.peek()
        {
            chars.next();
            if color.is_none() {
                color = code_color_rgb(code);
            }
            continue;
        }
        out.push(c);
    }
    (out, color)
}

fn extract_nbt_compound_text(c: &NbtCompound, out: &mut String, color: &mut Option<u32>) {
    if let Some(s) = c.string("text") {
        out.push_str(&s.to_str());
    }
    if color.is_none()
        && let Some(col) = c.string("color")
    {
        *color = dye_color_rgb(&col.to_str()).or_else(|| named_color_rgb(&col.to_str()));
    }
    if let Some(extra) = c.list("extra") {
        if let Some(extra_strings) = extra.strings() {
            for es in extra_strings.iter() {
                out.push_str(&es.to_str());
            }
        } else if let Some(extra_compounds) = extra.compounds() {
            for ec in extra_compounds {
                extract_nbt_compound_text(ec, out, color);
            }
        }
    }
}

pub fn parse_line(raw: &str) -> SignLineData {
    let mut current = raw.trim().to_string();
    if current.is_empty() || current == "\"\"" || current == "null" {
        return SignLineData::default();
    }
    for _ in 0..5 {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&current) {
            match val {
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                    let mut text = String::new();
                    extract_json_text(&val, &mut text);
                    let color_rgb = extract_json_color(&val);
                    let (clean_text, code_color) = extract_first_code_color(&text);
                    return SignLineData {
                        text: clean_text,
                        color_rgb: color_rgb.or(code_color),
                    };
                }
                serde_json::Value::String(s) => {
                    let s_trimmed = s.trim();
                    if (s_trimmed.starts_with('{') && s_trimmed.ends_with('}'))
                        || (s_trimmed.starts_with('[') && s_trimmed.ends_with(']'))
                    {
                        current = s_trimmed.to_string();
                        continue;
                    }
                    let (clean_text, code_color) = extract_first_code_color(&s);
                    return SignLineData {
                        text: clean_text,
                        color_rgb: code_color,
                    };
                }
                _ => break,
            }
        } else {
            break;
        }
    }
    let trimmed = current.trim();
    let s = if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    let (clean_text, code_color) = extract_first_code_color(s);
    SignLineData {
        text: clean_text,
        color_rgb: code_color,
    }
}

#[allow(dead_code)]
pub fn parse_component_text(raw: &str) -> String {
    parse_line(raw).text
}

pub fn parse_sign_render_info(
    pos: BlockPos,
    kind: BlockEntityKind,
    nbt: &NbtCompound,
    block_id: &str,
    props: &crate::world::block::PropMap,
    light: LightLevels,
) -> Option<SignRenderInfo> {
    if !matches!(kind, BlockEntityKind::Sign | BlockEntityKind::HangingSign) {
        return None;
    }
    let is_wall = block_id.ends_with("_wall_sign") || block_id.ends_with("_wall_hanging_sign");
    let is_hanging = block_id.contains("hanging_sign");
    let angle = if is_wall {
        let facing = props.get("facing").unwrap_or("north");
        match facing {
            "south" => 0.0,
            "west" => 90.0,
            "north" => 180.0,
            "east" => 270.0,
            _ => 0.0,
        }
    } else {
        props
            .get("rotation")
            .and_then(|r| r.parse::<f32>().ok())
            .unwrap_or(0.0)
            * 22.5
    };

    let parse_side_and_log = |tag: Option<&NbtCompound>| -> SignSideData {
        let mut messages = [
            SignLineData::default(),
            SignLineData::default(),
            SignLineData::default(),
            SignLineData::default(),
        ];
        let mut color_rgb = 0u32;
        let mut glowing = false;
        if let Some(t) = tag {
            if let Some(color_name) = t.string("color")
                && let Some(rgb) = dye_color_rgb(&color_name.to_str())
            {
                color_rgb = rgb;
            }
            if let Some(b) = t.byte("has_glowing_text") {
                glowing = b != 0;
            }
            if let Some(list) = t.list("messages") {
                if let Some(strings) = list.strings() {
                    for (i, s) in strings.iter().enumerate().take(4) {
                        let raw = s.to_str();
                        messages[i] = parse_line(&raw);
                        if messages[i].text.is_empty() && !raw.trim().is_empty() {
                            tracing::debug!("sign line {i} parsed to empty text from raw {raw:?}");
                        }
                    }
                } else if let Some(compounds) = list.compounds() {
                    for (i, c) in compounds.iter().enumerate().take(4) {
                        let mut text = String::new();
                        let mut color = None;
                        extract_nbt_compound_text(c, &mut text, &mut color);
                        let mut parsed = parse_line(&text);
                        if parsed.color_rgb.is_none() {
                            parsed.color_rgb = color;
                        }
                        if parsed.text.is_empty() && !text.trim().is_empty() {
                            tracing::debug!("sign compound line {i} parsed to empty from {text:?}");
                        }
                        messages[i] = parsed;
                    }
                }
            }
        }
        SignSideData {
            messages,
            color_rgb,
            glowing,
        }
    };

    let (front, back) = if let Some(front_compound) = nbt.compound("front_text") {
        let back_compound = nbt.compound("back_text");
        (
            parse_side_and_log(Some(front_compound)),
            parse_side_and_log(back_compound),
        )
    } else {
        let mut messages = [
            SignLineData::default(),
            SignLineData::default(),
            SignLineData::default(),
            SignLineData::default(),
        ];
        for (i, key) in ["Text1", "Text2", "Text3", "Text4"].iter().enumerate() {
            if let Some(s) = nbt.string(key) {
                messages[i] = parse_line(&s.to_str());
            }
        }
        let color_rgb = nbt
            .string("Color")
            .and_then(|c| dye_color_rgb(&c.to_str()))
            .unwrap_or(0);
        let glowing = nbt.byte("GlowingText").is_some_and(|b| b != 0);
        (
            SignSideData {
                messages,
                color_rgb,
                glowing,
            },
            SignSideData::default(),
        )
    };

    if front.is_empty() && back.is_empty() {
        return None;
    }

    Some(SignRenderInfo {
        pos,
        is_wall,
        is_hanging,
        angle,
        front,
        back,
        light,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_beds_are_modeled_blocks_not_block_entities() {
        assert!(!is_block_entity_block("red_bed"));
        assert!(!is_block_entity_block("white_bed"));
        assert!(rendered_kind("red_bed").is_none());
    }

    #[test]
    fn test_parse_component_text() {
        assert_eq!(
            parse_component_text("{\"text\":\"Leaderboard\"}"),
            "Leaderboard"
        );
        assert_eq!(parse_component_text("\"Hello\""), "Hello");
        assert_eq!(parse_component_text("\"\""), "");
        assert_eq!(
            parse_component_text("[\"\",{\"text\":\"лидеров\",\"color\":\"gold\"}]"),
            "лидеров"
        );
        assert_eq!(
            parse_component_text(r#""[\"\",{\"text\":\"лидеров\",\"color\":\"gold\"}]""#),
            "лидеров"
        );
        assert_eq!(
            parse_component_text(r#"{"text":"","extra":[{"text":"Топ "},{"text":"лидеров"}]}"#),
            "Топ лидеров"
        );
    }
}
