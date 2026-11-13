use std::collections::HashMap;

use azalea_registry::builtin::BlockEntityKind;
use simdnbt::owned::NbtCompound;

use crate::chat_component::{Component, list_elements};
use crate::position::{BlockPos, ChunkPos};
use crate::world::block::BlockStateData;

#[derive(Clone)]
pub struct StoredBlockEntity {
    #[allow(dead_code)]
    pub kind: BlockEntityKind,
    #[allow(dead_code)]
    pub nbt: NbtCompound,
    /// A sign's text, decoded from `nbt` as it arrives.
    pub sign: Option<Box<SignTexts>>,
}

impl StoredBlockEntity {
    pub fn new(kind: BlockEntityKind, nbt: NbtCompound) -> Self {
        let sign = matches!(kind, BlockEntityKind::Sign | BlockEntityKind::HangingSign)
            .then(|| Box::new(SignTexts::from_nbt(&nbt)));
        Self { kind, nbt, sign }
    }
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

/// The block entity a block gets the moment it's placed, before (or without)
/// any data from the server: [`rendered_kind`]'s, and signs, whose editor
/// opens on the empty sign the placement made.
fn synthesized_kind(name: &str) -> Option<BlockEntityKind> {
    rendered_kind(name).or_else(|| {
        if name.ends_with("_hanging_sign") {
            Some(BlockEntityKind::HangingSign)
        } else {
            name.ends_with("_sign").then_some(BlockEntityKind::Sign)
        }
    })
}

/// Kinds [`synthesized_kind`] makes entries for; used to detect entries
/// gone stale when the block at their position stops mapping to them.
fn is_synthesized(kind: BlockEntityKind) -> bool {
    matches!(
        kind,
        BlockEntityKind::Chest
            | BlockEntityKind::TrappedChest
            | BlockEntityKind::EnderChest
            | BlockEntityKind::ShulkerBox
            | BlockEntityKind::Sign
            | BlockEntityKind::HangingSign
    )
}

/// Sync the client-side entry for `pos` after a block update. The server sends
/// no block-entity data for e.g. a freshly placed chest, so those blocks get
/// an entry synthesized from the block state (vanilla creates the client
/// `BlockEntity` from the state the same way); a position whose block is no
/// longer a block entity drops its stale entry.
pub fn sync_block_entity(
    map: &mut BlockEntities,
    pos: BlockPos,
    block_state_data: &BlockStateData,
) {
    let id = block_state_data.id();
    if let Some(kind) = synthesized_kind(id) {
        if map.get(&pos).is_none_or(|e| e.kind != kind) {
            map.insert(pos, StoredBlockEntity::new(kind, NbtCompound::default()));
        }
    } else if !is_block_entity_block(id) || map.get(&pos).is_some_and(|e| is_synthesized(e.kind)) {
        // A synthesized entry is also stale when the block swaps directly to a
        // different block-entity block (e.g. /setblock chest -> sign).
        map.remove(&pos);
    }
}

/// A level's block entities by column, as vanilla `LevelChunk.blockEntities`
/// holds them: a column loads and unloads without a pass over the rest, and
/// a frame lists only the columns around the camera.
#[derive(Default)]
pub struct BlockEntities {
    columns: HashMap<ChunkPos, HashMap<BlockPos, StoredBlockEntity>>,
}

impl BlockEntities {
    pub fn get(&self, pos: &BlockPos) -> Option<&StoredBlockEntity> {
        self.columns.get(&ChunkPos::containing(*pos))?.get(pos)
    }

    pub fn get_mut(&mut self, pos: &BlockPos) -> Option<&mut StoredBlockEntity> {
        self.columns
            .get_mut(&ChunkPos::containing(*pos))?
            .get_mut(pos)
    }

    pub fn insert(&mut self, pos: BlockPos, entity: StoredBlockEntity) {
        self.columns
            .entry(ChunkPos::containing(pos))
            .or_default()
            .insert(pos, entity);
    }

    pub fn remove(&mut self, pos: &BlockPos) {
        if let Some(column) = self.columns.get_mut(&ChunkPos::containing(*pos)) {
            column.remove(pos);
        }
    }

    pub fn remove_column(&mut self, column: ChunkPos) {
        self.columns.remove(&column);
    }

    /// The block entities of every column within `blocks` of `center` on X
    /// and Z: a superset of those within `blocks` of it.
    pub fn near(
        &self,
        center: glam::DVec3,
        blocks: f64,
    ) -> impl Iterator<Item = (&BlockPos, &StoredBlockEntity)> {
        let chunks = move |at: f64| {
            ((at - blocks).floor() as i32 >> 4)..=((at + blocks).floor() as i32 >> 4)
        };
        chunks(center.x)
            .flat_map(move |x| chunks(center.z).map(move |z| ChunkPos::new(x, z)))
            .filter_map(|column| self.columns.get(&column))
            .flatten()
    }
}

/// Blocks vanilla backs with a block entity. Used to suppress missing-model
/// warnings and to detect stale block-entity map entries; the subset the BE
/// pipeline actually draws is [`rendered_kind`].
pub fn is_block_entity_block(name: &str) -> bool {
    rendered_kind(name).is_some() // chests, copper chests, shulker boxes
        || matches!(
        name,
        // Signs
        | "oak_sign" | "spruce_sign" | "birch_sign" | "jungle_sign" | "acacia_sign" | "dark_oak_sign"
        | "mangrove_sign" | "cherry_sign" | "pale_oak_sign" | "bamboo_sign"
        | "crimson_sign" | "warped_sign"
        | "oak_wall_sign" | "spruce_wall_sign" | "birch_wall_sign" | "jungle_wall_sign"
        | "acacia_wall_sign" | "dark_oak_wall_sign" | "mangrove_wall_sign" | "cherry_wall_sign"
        | "pale_oak_wall_sign" | "bamboo_wall_sign"
        | "crimson_wall_sign" | "warped_wall_sign"
        | "oak_hanging_sign" | "spruce_hanging_sign" | "birch_hanging_sign" | "jungle_hanging_sign"
        | "acacia_hanging_sign" | "dark_oak_hanging_sign" | "mangrove_hanging_sign"
        | "cherry_hanging_sign" | "pale_oak_hanging_sign" | "bamboo_hanging_sign"
        | "crimson_hanging_sign" | "warped_hanging_sign"
        | "oak_wall_hanging_sign" | "spruce_wall_hanging_sign" | "birch_wall_hanging_sign"
        | "jungle_wall_hanging_sign" | "acacia_wall_hanging_sign" | "dark_oak_wall_hanging_sign"
        | "mangrove_wall_hanging_sign" | "cherry_wall_hanging_sign" | "pale_oak_wall_hanging_sign"
        | "bamboo_wall_hanging_sign" | "crimson_wall_hanging_sign" | "warped_wall_hanging_sign"
        // Banners
        | "white_banner" | "orange_banner" | "magenta_banner" | "light_blue_banner"
        | "yellow_banner" | "lime_banner" | "pink_banner" | "gray_banner"
        | "light_gray_banner" | "cyan_banner" | "purple_banner" | "blue_banner"
        | "brown_banner" | "green_banner" | "red_banner" | "black_banner"
        | "white_wall_banner" | "orange_wall_banner" | "magenta_wall_banner" | "light_blue_wall_banner"
        | "yellow_wall_banner" | "lime_wall_banner" | "pink_wall_banner" | "gray_wall_banner"
        | "light_gray_wall_banner" | "cyan_wall_banner" | "purple_wall_banner" | "blue_wall_banner"
        | "brown_wall_banner" | "green_wall_banner" | "red_wall_banner" | "black_wall_banner"
        // Skulls / heads
        | "skeleton_skull" | "skeleton_wall_skull"
        | "wither_skeleton_skull" | "wither_skeleton_wall_skull"
        | "zombie_head" | "zombie_wall_head"
        | "player_head" | "player_wall_head"
        | "creeper_head" | "creeper_wall_head"
        | "dragon_head" | "dragon_wall_head"
        | "piglin_head" | "piglin_wall_head"
        // Misc block entities
        | "conduit" | "decorated_pot" | "end_portal" | "end_gateway"
        | "beacon" | "spawner" | "trial_spawner" | "vault"
        | "brewing_stand" | "lectern" | "campfire" | "soul_campfire"
        | "beehive" | "bee_nest" | "bell" | "suspicious_sand" | "suspicious_gravel"
        | "crafter"
    )
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

/// 16 dye colors in vanilla `DyeColor` ordinal order.
pub(crate) const DYE_COLOR_NAMES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// `DyeColor.getTextColor`, by [`DYE_COLOR_NAMES`] index.
const DYE_TEXT_COLORS: [u32; 16] = [
    0xFFFFFF, 0xFF681F, 0xFF00FF, 0x9AC0CD, 0xFFFF00, 0xBFFF00, 0xFF69B4, 0x808080, 0xD3D3D3,
    0x00FFFF, 0xA020F0, 0x0000FF, 0x8B4513, 0x00FF00, 0xFF0000, 0x000000,
];

const BLACK_DYE: usize = 15;

/// Sign `WoodType` names, in the order the block-entity pipeline's
/// `SIGN_TEXTURES` lists them.
pub(crate) const SIGN_WOOD_NAMES: [&str; 12] = [
    "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak",
    "bamboo", "crimson", "warped",
];

/// A sign block's shape, from `SignBlock`'s four subclasses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignBlock {
    pub hanging: bool,
    /// `WallSignBlock` or `WallHangingSignBlock`.
    pub wall: bool,
    /// Index into [`SIGN_WOOD_NAMES`] (`SignBlock.getWoodType`).
    pub wood: usize,
    /// The yaw its text faces, in degrees: a wall sign's `facing.toYRot()`,
    /// otherwise `RotationSegment.convertToDegrees(rotation)`.
    pub angle: f32,
}

impl SignBlock {
    pub fn of(name: &str, props: &crate::world::block::PropMap) -> Option<Self> {
        let (wood, hanging, wall) = [
            ("_wall_hanging_sign", true, true),
            ("_hanging_sign", true, false),
            ("_wall_sign", false, true),
            ("_sign", false, false),
        ]
        .into_iter()
        .find_map(|(suffix, hanging, wall)| Some((name.strip_suffix(suffix)?, hanging, wall)))?;
        let wood = SIGN_WOOD_NAMES.iter().position(|&w| w == wood)?;
        let angle = if wall {
            match props.get("facing")? {
                "south" => 0.0,
                "west" => 90.0,
                "north" => 180.0,
                _ => 270.0,
            }
        } else {
            props.get("rotation")?.parse::<f32>().ok()? * 22.5
        };
        Some(Self {
            hanging,
            wall,
            wood,
            angle,
        })
    }

    /// `SignBlockEntity.getMaxTextLineWidth`.
    pub fn max_text_line_width(self) -> f32 {
        if self.hanging { 60.0 } else { 90.0 }
    }

    /// `SignBlockEntity.getTextLineHeight`.
    pub fn text_line_height(self) -> f32 {
        if self.hanging { 9.0 } else { 10.0 }
    }
}

/// A sign's two faces, vanilla `SignBlockEntity.frontText` and `backText`.
#[derive(Clone, Debug, Default)]
pub struct SignTexts {
    pub front: SignText,
    pub back: SignText,
}

impl SignTexts {
    /// `SignBlockEntity.loadAdditional`: a face that fails to decode is
    /// empty.
    fn from_nbt(nbt: &NbtCompound) -> Self {
        let face = |key| {
            nbt.compound(key)
                .and_then(SignText::from_nbt)
                .unwrap_or_default()
        };
        Self {
            front: face("front_text"),
            back: face("back_text"),
        }
    }

    pub fn face(&self, front: bool) -> &SignText {
        if front { &self.front } else { &self.back }
    }

    pub fn face_mut(&mut self, front: bool) -> &mut SignText {
        if front {
            &mut self.front
        } else {
            &mut self.back
        }
    }
}

/// Vanilla `SignText`: one face's lines, dye and glow.
// TODO: `filtered_messages`, shown in place of `messages` when the
// account's text filtering is on.
#[derive(Clone, Debug)]
pub struct SignText {
    pub messages: [Component; 4],
    /// Index into [`DYE_COLOR_NAMES`].
    pub color: usize,
    pub has_glowing_text: bool,
}

impl Default for SignText {
    /// `SignText.EMPTY`.
    fn default() -> Self {
        Self {
            messages: std::array::from_fn(|_| Component::text("")),
            color: BLACK_DYE,
            has_glowing_text: false,
        }
    }
}

impl SignText {
    /// `SignText.CODEC`: four messages, a dye defaulting to black and the
    /// glow flag.
    // TODO: servers before 1.20.5 send each message as a JSON string, which
    // `net::translate` doesn't rewrite yet, so those show as literal text.
    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let messages = list_elements(nbt.list("messages")?)
            .iter()
            .map(|tag| Component::from_nbt_tag(tag).ok())
            .collect::<Option<Vec<_>>>()?
            .try_into()
            .ok()?;
        let color = match nbt.string("color") {
            Some(name) => DYE_COLOR_NAMES.iter().position(|&n| n == name.to_str())?,
            None => BLACK_DYE,
        };
        Some(Self {
            messages,
            color,
            has_glowing_text: nbt.byte("has_glowing_text").is_some_and(|b| b != 0),
        })
    }

    /// `getColor().getTextColor()`.
    pub fn text_color(&self) -> u32 {
        DYE_TEXT_COLORS[self.color]
    }

    pub fn is_black(&self) -> bool {
        self.color == BLACK_DYE
    }

    /// `AbstractSignRenderer.getDarkColor`.
    pub fn dark_color(&self) -> u32 {
        if self.is_black() && self.has_glowing_text {
            return 0xF0EBCC;
        }
        let scale = |shift: u32| (((self.text_color() >> shift) & 0xFF) as f32 * 0.4) as u32;
        scale(16) << 16 | scale(8) << 8 | scale(0)
    }

    /// The colour its text is written in, the screen's `textColor`: the dye
    /// when glowing, else the dark colour.
    pub fn ink_color(&self) -> u32 {
        if self.has_glowing_text {
            self.text_color()
        } else {
            self.dark_color()
        }
    }
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::{NbtList, NbtTag};

    use super::*;

    #[test]
    fn native_beds_are_modeled_blocks_not_block_entities() {
        assert!(!is_block_entity_block("red_bed"));
        assert!(!is_block_entity_block("white_bed"));
        assert!(rendered_kind("red_bed").is_none());
    }

    fn face(messages: Vec<NbtTag>, extra: Vec<(&str, NbtTag)>) -> NbtCompound {
        let mut face = NbtCompound::new();
        face.insert("messages", NbtTag::List(NbtList::from(messages)));
        for (key, tag) in extra {
            face.insert(key, tag);
        }
        face
    }

    fn string(s: &str) -> NbtTag {
        NbtTag::String(s.into())
    }

    #[test]
    fn sign_text_reads_messages_dye_and_glow() {
        let mut styled = NbtCompound::new();
        styled.insert("text", string("Gold"));
        styled.insert("color", string("gold"));
        let mut nbt = NbtCompound::new();
        nbt.insert(
            "front_text",
            NbtTag::Compound(face(
                vec![
                    string("Hello"),
                    NbtTag::Compound(styled),
                    string(""),
                    string(""),
                ],
                vec![
                    ("color", string("red")),
                    ("has_glowing_text", NbtTag::Byte(1)),
                ],
            )),
        );
        let texts = SignTexts::from_nbt(&nbt);
        let front = &texts.front;
        assert_eq!(front.messages[0].plain_text(), "Hello");
        assert_eq!(front.messages[1].plain_text(), "Gold");
        assert_eq!(front.text_color(), 0xFF0000);
        assert!(front.has_glowing_text);
        assert_eq!(front.ink_color(), 0xFF0000);
        // The back face is missing, so empty and black.
        assert!(
            texts
                .back
                .messages
                .iter()
                .all(|m| m.plain_text().is_empty())
        );
        assert!(texts.back.is_black());
    }

    #[test]
    fn a_sign_face_that_fails_to_decode_is_empty() {
        let mut nbt = NbtCompound::new();
        // Three lines where the codec wants four.
        let short = face(vec![string("a"), string("b"), string("c")], Vec::new());
        nbt.insert("front_text", NbtTag::Compound(short));
        let unknown_dye = face(vec![string("a"); 4], vec![("color", string("teal"))]);
        nbt.insert("back_text", NbtTag::Compound(unknown_dye));
        let texts = SignTexts::from_nbt(&nbt);
        assert!(texts.front.messages[0].plain_text().is_empty());
        assert!(texts.back.messages[0].plain_text().is_empty());
    }

    #[test]
    fn dark_color_scales_the_dye_or_lights_glowing_black() {
        let text = |color, glowing| SignText {
            color,
            has_glowing_text: glowing,
            ..SignText::default()
        };
        // Red, 0xFF0000, at 0.4.
        assert_eq!(text(14, false).dark_color(), 0x660000);
        assert_eq!(text(14, false).ink_color(), 0x660000);
        // Light blue, 0x9AC0CD, truncating per channel.
        assert_eq!(text(3, false).dark_color(), 0x3D4C52);
        assert_eq!(text(BLACK_DYE, false).dark_color(), 0x000000);
        assert_eq!(text(BLACK_DYE, true).dark_color(), 0xF0EBCC);
    }

    #[test]
    fn sign_blocks_by_name() {
        let props = |pairs| crate::world::block::PropMap::from_pairs(pairs);
        let standing = SignBlock::of("dark_oak_sign", &props(vec![("rotation", "4")])).unwrap();
        assert_eq!(
            standing,
            SignBlock {
                hanging: false,
                wall: false,
                wood: 5,
                angle: 90.0,
            }
        );
        let wall = SignBlock::of("spruce_wall_sign", &props(vec![("facing", "north")])).unwrap();
        assert!(wall.wall && !wall.hanging);
        assert_eq!((wall.wood, wall.angle), (1, 180.0));
        let hanging = SignBlock::of(
            "pale_oak_wall_hanging_sign",
            &props(vec![("facing", "west")]),
        )
        .unwrap();
        assert!(hanging.wall && hanging.hanging);
        assert_eq!((hanging.wood, hanging.angle), (8, 90.0));
        assert_eq!(hanging.max_text_line_width(), 60.0);
        assert_eq!(hanging.text_line_height(), 9.0);
        assert!(SignBlock::of("oak_planks", &props(Vec::new())).is_none());
    }

    #[test]
    fn a_placed_sign_gets_an_empty_entry_until_its_data_arrives() {
        let table = crate::world::block::BlockStateTable::native();
        let mut map = BlockEntities::default();
        let pos = BlockPos::new(0, 0, 0);
        let state = table.find_state("oak_sign", &[("rotation", "0")]);
        sync_block_entity(&mut map, pos, table.block_state_data(state));
        let entry = map.get(&pos).unwrap();
        assert_eq!(entry.kind, BlockEntityKind::Sign);
        assert!(entry.sign.is_some());
    }

    #[test]
    fn block_entities_are_found_and_dropped_by_column() {
        let entity = || StoredBlockEntity::new(BlockEntityKind::Chest, NbtCompound::default());
        let (here, next_column, far) = (
            BlockPos::new(-1, 70, 15),
            BlockPos::new(0, 70, 15),
            BlockPos::new(200, 70, 0),
        );
        let mut map = BlockEntities::default();
        for pos in [here, next_column, far] {
            map.insert(pos, entity());
        }
        let near = |map: &BlockEntities| {
            let mut found: Vec<BlockPos> = map
                .near(glam::DVec3::new(0.5, 70.0, 0.5), 64.0)
                .map(|(pos, _)| *pos)
                .collect();
            found.sort_by_key(|pos| pos.x);
            found
        };
        assert_eq!(near(&map), [here, next_column]);

        map.remove_column(ChunkPos::containing(here));
        assert_eq!(near(&map), [next_column]);
        assert!(map.get(&here).is_none() && map.get(&far).is_some());
        map.remove(&next_column);
        assert!(near(&map).is_empty());
    }
}
