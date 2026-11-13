//! Wire translation for non-native protocol versions.
//!
//! The client speaks 26.2 natively (`pomme_protocol::version::NATIVE`). When
//! a connection negotiates a different wire version, inbound frames are
//! rewritten into the native layout before azalea's typed decode where the
//! wire format changed, and static-registry ids (which shift between versions)
//! are remapped in both directions so the rest of the client stays
//! single-version. Layouts were line-checked against the decompiled
//! references (`reference/<version>/decompiled/.../network/protocol/`), and
//! each tested version's real join traffic is replayed through them by
//! `net::wire_corpus`.
//!
//! 26.3 -> 26.2 wire changes (the first version above native; its frames
//! translate down):
//! - `post_effects` was inserted at configuration clientbound 10 and game
//!   clientbound 83, `add_transient_block` at 37 and `swing_animation` at 123,
//!   shifting every later id; serverbound `swing` (63) became the payload-less,
//!   main-hand-only `punch` (46). `post_effects` and `add_transient_block` have
//!   no native equivalent and drop through the id map
//! - `move_entity_pos`/`move_entity_pos_rot` pack onGround and a step count
//!   into a `properties` varint and carry a `VecDelta` (one short triple, or
//!   per-step `ticks` + triple), collapsed to the summed displacement;
//!   `move_entity_rot` moved onGround ahead of the rotation bytes
//! - `entity_position_sync` replaced the position and delta doubles with a
//!   `PositionPath` (a `Vec3`, or a counted list of `Vec3` + tick-offset steps
//!   ending at the last); a zero delta is synthesized
//! - `level_particles` leads with the particle, splits `maxSpeed` per axis,
//!   sends `count` as a varint and appends a randomization type; particle
//!   payloads are sized per option codec (byte-identical to 26.2's), so
//!   block-state, dust and item particles translate on this version
//! - `CommonPlayerSpawnInfo` (`login`, `respawn`) sends `previousGameType` as
//!   an optional varint (0 = none, else id + 1) where 26.2 reads a signed byte
//! - `animate` lost its swing actions to `swing_animation` (`entityId, hand,
//!   {type, duration}`, rewritten back onto `animate`) and renumbered the rest
//!   (wake up 0, critical hit 1, magic critical hit 2)
//! - `update_advancements` moved each advancement's screen position out of
//!   `DisplayInfo` to a float pair after the entry
//! - `explode` appended a `playSound` bool
//! - `EntityDataSerializers` appended `dye_color` (43), used only by the
//!   cushion, whose entity type (like the poplar boats) has no native id
//! - item ids diverge from 72 and `data_component_type` from 40
//!   (`swing_animation` and `map_color` removed, thirteen components added),
//!   and `pot_decorations`, inline trim materials and instruments, and
//!   `teleport_randomly` changed their value layout. Every item-carrying packet
//!   (containers, cursor, inventory, equipment, merchant offers, entity data,
//!   creative slots) goes through 26.3's item grammar (`pomme_protocol::item`),
//!   which remaps the ids, converts the four changed layouts and drops
//!   components 26.2 lacks
//! - `ByteBufCodecs.BIT_SET` (a byte array) replaced the long-array bitset in
//!   the light masks (`level_chunk_with_light`, `light_update`) and the
//!   `player_chat` partial filter mask
//! - `commands` inserted five singleton argument types (the parser ids are
//!   remapped by name; the 26.3-only ones become the parser 26.2 sent)
//! - the `tag` slot display carries a `HolderSet` rather than a tag id, and
//!   26.3 routes every ingredient through it (`recipe_book_add`,
//!   `place_ghost_recipe`, `update_recipes`)
//! - TODO: recipes, stonecutter options and fuels built on an item 26.2 doesn't
//!   have (poplar wood, wool stairs, cushions) are dropped, and such items
//!   leave the ingredient lists they appear in
//! - serverbound `player_action` inserted `CHANGE_DESTROY_DIRECTION` at 1,
//!   which pomme never sends (it only steers the server's new destroy-progress
//!   level events, 2019/2020, which drop client-side); `sign_update` reordered
//!   its slot field (pomme never sends it)
//! - `accept_teleportation` appends the accepted pose, and 26.3's
//!   `handleMovePlayer` no longer follows it with a `move_player_pos_rot` (a
//!   second position packet in one tick disconnects): the accept is held and
//!   folded together with the PosRot pomme sends right after it
//! - `punch` is sent only for attacks; use swings are server-side
//!   (`reports_use_swings`)
//! - TODO: 26.3 skips applying a teleport to a passenger and sends
//!   `ABORT_DESTROY_BLOCK` after one; pomme does neither
//! - `move_vehicle`, `open_sign_editor`, the chunk sections and the item and
//!   component-patch codecs are byte-identical
//!
//! 26.1 -> 26.2 wire changes:
//! - login `login_finished` gained a trailing session-id UUID
//! - game `login` gained an `onlineMode` bool before the trailing
//!   `enforcesSecureChat` bool; older clients prepared a chat key pair on an
//!   encrypted connection instead, so the game loop substitutes that
//! - game `set_player_team` reordered its `Parameters` fields and turned the
//!   color from a `ChatFormatting` ordinal into an `Optional<TeamColor>`
//! - serverbound slot 62 was replaced (`spectate_entity` ->
//!   `spectator_action`); pomme sends neither
//! - the `color` argument type was renamed `team_color` at the same id;
//!   `commands` remaps every parser id by name, so the rename is bridged
//! - 26.1 alone: item and component layouts are 26.2's, but item ids shift and
//!   components past `sulfur_cube_content` sit one lower, so its stacks go
//!   through 26.2's item grammar, which builds its game translation although
//!   its packet ids match
//!
//! 1.21.11 -> 26.2 wire changes (all of the above — 1.21.11 matches 26.1 on
//! those three layouts — plus):
//! - game packet ids diverge in both directions (handshake/status/login/
//!   configuration ids and layouts are identical), so frames get an id remap at
//!   the edge; every 1.21.11 packet still exists in 26.2 under the same name,
//!   and no other clientbound layout changed
//! - `set_entity_data` serializer ids shifted (26.x interleaved four
//!   `*_sound_variant` serializers into `EntityDataSerializers`), and particle
//!   values carry type ids in the wire version's registry space, remapped in
//!   place
//! - each chunk section in `level_chunk_with_light` gained a `fluidCount` short
//!   after `nonEmptyBlockCount`
//! - `set_time` replaced `dayTime`/`tickDayTime` with a world-clock map
//! - serverbound `attack` split out of `interact`, which now always carries the
//!   hand and a low-precision hit location; 26.2-only serverbound packets
//!   without an equivalent (`set_game_rule`, `spectator_action`) are suppressed
//!
//! 1.21.10 -> 26.2 wire changes (identical to 1.21.11's — the packet layouts
//! didn't change between the two — except):
//! - clientbound 40 is `horse_screen_open`, which 1.21.11 renamed
//!   `mount_screen_open` with identical fields; the id match aliases the pair
//! - `EntityDataSerializers` lacks 1.21.11's `zombie_nautilus_variant` (28) and
//!   trailing `humanoid_arm`, so the serializer remap differs
//!
//! 1.21.8 -> 26.2 wire changes (all of 1.21.10's plus 1.21.9's, which the id
//! remap and the rewrites below absorb):
//! - `add_entity` carried the velocity as three trailing shorts (1/8000 block
//!   per tick); 1.21.9 moved it to an `LpVec3` after the position.
//!   `set_entity_motion` made the same shorts -> `LpVec3` switch
//! - `player_rotation` gained a relative-rotation bool after each angle
//! - `set_default_spawn_position` went from `BlockPos + angle` to `RespawnData`
//!   (`GlobalPos`, yaw, pitch); the old packet has no dimension, so the
//!   overworld is synthesized
//! - `explode` gained `radius` and `blockCount` after the center and a trailing
//!   weighted block-particle list, and its particle and sound ids are remapped
//! - `EntityDataSerializers` still had `compound_tag` (16), which 1.21.9
//!   removed; entries using it (player shoulder parrots) are stripped, and
//!   every id from `particle` up shifts
//! - the `profile` item component was a bare name/uuid/properties triple, which
//!   1.21.9 wrapped in `ResolvableProfile` (full/partial profile either plus a
//!   skin patch)
//! - the clientbound `debug_*`/`game_test_highlight_pos` packets don't exist
//!   yet (pure id shifts); serverbound 22 was renamed
//!   (`debug_sample_subscription` -> `debug_subscription_request`), which pomme
//!   never sends
//!
//! 1.21.6 -> 26.2: identical to 1.21.8's translation. 1.21.7 changed no
//! packet layout, id, or serializer (the decompiled `network/protocol`
//! trees and `EntityDataSerializers` are byte-identical); it only added the
//! lava_chicken music disc item and sound, absorbed by the registry remap.
//!
//! 1.21.5 -> 26.2 wire changes (all of 1.21.8's — the pre-1.21.9 rewrites
//! and serializer set carry over unchanged — plus):
//! - 1.21.6 appended its dialog/waypoint clientbound packets and inserted
//!   `change_game_mode`/`custom_click_action` serverbound; the id remap absorbs
//!   the shifts and `change_game_mode` (which doesn't exist yet) is suppressed
//! - serverbound `player_command` still opens its action enum with
//!   PRESS/RELEASE_SHIFT_KEY, so newer action ordinals sit two higher
//! - 1.21.6 inserted the `hex_color` and `dialog` argument types (and 1.20.3
//!   `style`), shifting the `commands` parser ids after them
//! - `change_difficulty` read an unsigned byte where 1.21.6 reads a varint;
//!   difficulty ids fit a single byte either way, so the wire bytes are
//!   identical
//! - 1.21.5 alone: the chunk section buffer was still sized for the long-array
//!   length varints `writeFixedSizeLongArray` stopped writing
//!   (`PalettedContainer.Data.getSerializedSize`), so it ends in zero padding,
//!   which is dropped
//! - `chunks_biomes` carries the same biome containers, length-prefixed before
//!   1.21.5 and padded on it; the packet is dropped (TODO), so a `/fillbiome`
//!   shows only once its chunks are sent again
//! - 1.21.6 gave `HangingEntity` a synched `direction` at index 8, so an item
//!   frame's item/rotation sit at 8/9 here and a painting's variant at 8 (9/10
//!   and 9 on 26.2); the walker passes index bytes through unremapped and pomme
//!   renders neither entity, so the shift is left in place
//!
//! 1.21.4 -> 26.2 wire changes (all of 1.21.5's plus):
//! - chunk heightmaps were a network-NBT compound of named long arrays; 1.21.5
//!   packed them into a (type id, long array) list
//! - each chunk section's block and biome `PalettedContainer` prefixed its
//!   packed longs with a varint length, which 1.21.5 dropped (the bits imply
//!   it); it is checked and stripped
//! - `player_chat` gained a leading `globalIndex` varint; zero is synthesized
//! - serverbound `chat`'s and `chat_command_signed`'s last-seen update gained a
//!   trailing checksum byte in 1.21.5; it is stripped
//! - `update_advancements` gained a trailing `showAdvancements` bool; true is
//!   synthesized
//! - team `Parameters` carried nametag visibility and collision rule as
//!   strings, turned into enum ids by 1.21.5
//! - serverbound `container_click` carried full item stacks for the changed
//!   slots and carried item where 1.21.5 hashes them; the hashes can't be
//!   reversed, so bare component-less stacks are reconstructed (the server
//!   reconciles any mismatch by resyncing the slots)
//! - `EntityDataSerializers` had no cow/pig/chicken/wolf-sound variants and
//!   `optional_uuid` where 1.21.5 has `optional_living_entity_reference`
//!   (wire-identical); `compound_tag` sits at 16 like 1.21.8, stripped the same
//!   way
//! - `add_experience_orb` has no newer equivalent and pomme renders no XP orbs;
//!   the frame is dropped, like `add_entity` for the thrown `potion` entity
//!   1.21.5 split into splash/lingering
//! - serverbound `set_creative_mode_slot` wrote item component values bare,
//!   where 1.21.5 length-prefixes each one
//!   (`ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC`); untranslated — see the
//!   azalea-divergence list
//!
//! 1.21.3 -> 26.2 wire changes (all of 1.21.4's plus):
//! - `level_particles` lacks the `alwaysShow` bool 1.21.4 inserted after
//!   `overrideLimiter`; false is synthesized
//! - `player_info_update`'s action mask passes through unchanged: 1.21.4
//!   appended UPDATE_HAT after UPDATE_LIST_ORDER rather than inserting it, so
//!   every older mask is a prefix of 26.2's, and `writeFixedBitSet` is one byte
//!   at 6, 7 and 8 actions alike
//! - serverbound `move_vehicle` lacks the trailing onGround bool and 1.21.4
//!   split `pick_item` into the from_block/from_entity pair; the pair is
//!   suppressed quietly below 769 (TODO: port the slot-based `pick_item`)
//! - clientbound `set_held_slot` reads a byte where 26.2 reads a varint; hotbar
//!   slots encode identically
//!
//! 1.21.1 -> 26.2 wire changes (the 1.21.2 rework; all of 1.21.3's plus):
//! - `player_position` and `teleport_entity` predate PositionMoveRotation:
//!   positions reorder, rotations un-pack (teleport carried packed-degree
//!   bytes), and a zero delta is synthesized. Both versions resolve that delta
//!   against the relative bits, so `player_position` mirrors each position bit
//!   into its `DELTA_*` bit and `teleport_entity` rewrites onto
//!   `entity_position_sync`, whose handler leaves velocity alone as 1.21.1's
//!   did — without either, an old server's syncs zero the velocity
//! - `set_time` is two longs with a negated dayTime marking a frozen clock
//! - CommonPlayerSpawnInfo lacks the trailing `seaLevel` varint, inserted
//!   before the final byte of `login` and `respawn`
//! - `container_set_slot` reads a signed-byte container id whose -1/-2
//!   sentinels became `set_cursor_item`/`set_player_inventory`
//! - `cooldown` carries an item registry id where 26.2 names a cooldown group;
//!   it becomes the item's id, the group of every item without `use_cooldown`
//! - clientbound `set_carried_item` was renamed `set_held_slot` (identical byte
//!   layout); login `game_profile` -> `login_finished` needs no alias (login
//!   rewrites dispatch by id) but ends in a `strictErrorHandling` bool 1.21.2
//!   dropped, stripped before the session UUID is appended
//! - `update_recipes` and `place_ghost_recipe` restructured into 26.2's
//!   `RecipeDisplay` trees with no mechanical mapping, so they're dropped and
//!   the recipe book/stonecutter UIs stay empty on 1.21.1 servers (the
//!   1.21.1-only `recipe` packet drops via the id map); `explode` is dropped
//!   too, but only for want of a rewriter, so no explosions render either
//! - every attribute was renamed by dropping its category prefix
//!   (`generic.armor` -> `armor`), handled by the registry-remap alias;
//!   `boat`/`chest_boat` entities split per wood type and have no 26.2 ids, so
//!   boats aren't rendered
//! - serverbound `player_input` was the vehicle-steering packet (two axis
//!   floats + jump/shift flags) rather than the key bitfield, and the
//!   move_player flags byte was a plain onGround bool, so the
//!   horizontal-collision bit 1.21.2 added must be dropped. 1.21.1 sent
//!   `player_input` only while riding and pomme sends it every tick, but
//!   `ServerPlayer.setPlayerInput` is guarded by `isPassenger()`, so the
//!   unmounted frames are discarded server-side
//! - `client_tick_end` doesn't exist; its suppression is expected and logged
//!   quietly
//! - serverbound `client_information` lacks the trailing `particleStatus` enum
//!   1.21.2 appended; it is stripped in both phases
//! - `AbstractArrow` gained `in_ground` at index 10, so an arrow sends its
//!   effect color at 10 and a trident its loyalty/foil at 10/11, where 26.2
//!   reads 11 and 11/12. The indices pass through unshifted, which costs
//!   nothing while pomme reads metadata only for living entities; a lift
//!   belongs beside `normalize_player_index_at` in `entity/mod.rs`
//!
//! 1.20.6 -> 26.2 wire changes (all of 1.21.1's — 1.21 only appended
//! `custom_report_details`/`server_links`, so the tables are a strict
//! prefix and every pre-1.21.2 rewrite carries over — plus):
//! - `update_attributes` modifier ids were UUIDs, turned into resource
//!   locations by 1.21; a hex name is synthesized (pomme reads only the
//!   attribute values)
//! - `projectile_power` carried a per-axis acceleration vector, collapsed to
//!   its magnitude
//! - serverbound `use_item` lacks the rotation floats 1.21 appended
//! - the damage/effect/dimension holder codecs merely moved into their types
//!   (wire-identical), and the block set matches 1.21.1's
//! - `horse_screen_open`'s middle varint changed meaning rather than layout:
//!   1.20.6 sends the container's slot count where 1.21 sends the mount's
//!   inventory column count (`columns = (size - 1) / 3` — 1 for a plain horse,
//!   16 for a chested donkey, `1 + 3c` for a llama). Passed through untouched,
//!   which costs nothing while pomme has no mount-inventory screen
//!
//! 1.20.4 -> 26.2 wire changes (the 1.20.5 item-component rework; all of
//! 1.20.6's plus):
//! - items are `bool + id + byte count + NBT`; they translate bare (type and
//!   count survive, the NBT is dropped) in `container_set_content`,
//!   `container_set_slot`, `set_equipment`, `merchant_offers` (whose costs
//!   became `ItemCost`s), entity-data item values, and outbound
//!   `container_click`/`set_creative_mode_slot` — enchant glints, custom names
//!   and damage bars render plain on 765 servers
//! - the configuration phase diverges for the first time: ids remap, and the
//!   single whole-holder NBT `registry_data` packet fans out into the
//!   per-registry form (entries reordered by their explicit ids, which the
//!   client equates with wire order); the dimension-type order is kept for the
//!   spawn-info rewrites, whose dimension type is a resource key string at 765
//! - login-phase `hello` lacks the shouldAuthenticate bool (synthesized true)
//!   and game `login` lacks enforcesSecureChat
//! - `update_attributes` keys attributes by resource location and
//!   `update_mob_effect` has a byte amplifier plus trailing factor NBT
//! - TODO: sign messages in block entity NBT are JSON strings, where 1.20.5
//!   stores components; they aren't rewritten, so 765 signs show the JSON
//! - `level_particles` leads with the particle type id; `chat_command` is
//!   always the signed form (empty signatures appended), so
//!   `chat_command_signed` goes out as it, checksum stripped
//! - `update_advancements` drops (old-form icons nested in display data);
//!   serializer ids from `particles` (18) up shift
//! - `player_chat`/`disguised_chat` carry a direct chat-type registry id where
//!   1.20.5 put a holder (bumped by one on the way through)
//! - `server_data` ends in an `enforcesSecureChat` bool 1.20.5 dropped; it is
//!   stripped. It's this era's only copy of the flag (game `login` lacks it and
//!   gets false), so the game loop applies it from there, as 1.20.4's
//!   `handleServerData` does; the server sends it before any player info
//!
//! 1.20.2 -> 26.2 wire changes (all of 1.20.4's plus; the pre-1.20.3 era):
//! - text components are length-prefixed JSON strings, not NBT
//!   (`FriendlyByteBuf.writeComponent`); `component_pass` transcodes every
//!   consumed component field via `json_to_nbt` (mixed arrays normalize to
//!   compound lists) and the entity-data component serializers (5/6) transcode
//!   in the metadata walk — `server_data` and `map_item_data` drop instead
//!   (pomme never reads them), `server_data` once its secure-chat flag is read
//! - `set_score` carries a method byte (its REMOVE arm becomes the
//!   `reset_score` packet 1.20.3 added) and no display/numberFormat;
//!   `set_objective` ends at the render type
//! - one `resource_pack` packet serves both phases: it maps to
//!   `resource_pack_push` with a synthesized zero UUID, and the serverbound
//!   reply drops its UUID and clamps post-1.20.2 action values
//! - everything else — items, registry_data, spawn info, login, chunks,
//!   serializer order — matches 1.20.4 exactly (the diff is tiny)
//! - the `LEVEL_CHUNKS_LOAD_START` game event (13) doesn't exist yet (1.20.4
//!   added it), so the level load tracker treats the level as started at login
//!   instead of waiting for it (see `AppCore::tick_level_load`)
//!
//! 1.20.1 -> 26.2 wire changes (all of 1.20.2's plus; 1.20 shares them):
//! - there is no configuration phase, so the join sends no `login_acknowledged`
//!   and reads the registries out of the game `login` packet, which carries the
//!   whole codec inline; the brand and `client_information` move to the game
//!   phase, where 1.20.1's `ClientPacketListener.handleLogin` sends them.
//!   `login` itself predates `CommonPlayerSpawnInfo`, so its fields reorder
//!   (`portalCooldown` already exists), and `respawn` carried `dataToKeep`
//!   mid-packet rather than last
//! - NBT roots are written with an empty name (`FriendlyByteBuf.writeNbt` went
//!   through `NbtIo.write`, where 1.20.2 switched to `writeAnyTag`), so item
//!   tags, entity `compound_tag` values, chunk heightmaps, every chunk block
//!   entity and the `block_entity_data` / `tag_query` tags carry two extra
//!   bytes the native readers don't expect
//! - `add_player` spawned other players; 1.20.2 dropped it for `add_entity`,
//!   which the frame is rewritten onto (yaw and pitch swap, the head yaw
//!   repeats the yaw, and the data and velocity fields are zero)
//! - `forget_level_chunk` sent two ints where 1.20.2 packs a `ChunkPos` long,
//!   whose big-endian halves put z first
//! - the chunk section buffer was allocated by sizing each long array's length
//!   varint for its entry count, not its long count, so it ends in zero padding
//!   (one byte per single-value block container), which is dropped
//! - serverbound login `hello` wrapped its profile id in an optional, which
//!   1.20.2 made mandatory
//! - `update_enabled_features` is a game packet here (1.20.2 moved it into
//!   configuration) and drops through the id map; `update_tags` stayed a game
//!   packet, 1.20.2 only adding a configuration copy beside it. Pomme reads
//!   neither. `update_advancements` still nests its criteria map, but the 765
//!   gate already drops the packet for its old-form icons
//! - `set_display_objective` wrote a byte where 1.20.2 writes a varint; every
//!   slot id is under 128, so the bytes match. Every other packet 1.20.2
//!   touched (`player_info_update`, `merchant_offers`, `set_equipment`,
//!   `map_item_data`, `update_recipes`, `player_chat`, the chunk framing) was
//!   refactored without changing its layout
//!
//! Known limitation (accepted): an inbound item stack carrying a data
//! component at/after the first id the versions number differently (26.3:
//! 40, where 26.3 replaced `swing_animation` with `attack_animation`; 26.1:
//! 78, where 26.2 inserted `sulfur_cube_content`; 1.21.11: 41, where 26.x
//! inserted `additional_trade_cost`; 1.21.10 and every older version with a
//! component registry: 5, where 1.21.11 inserted `use_effects` — so even
//! `custom_name` and `enchantments` are affected on all of them) decodes under
//! the wrong 26.2 codec — usually a misparse that skips the packet via
//! `skip_malformed_packet`, though a coincidentally parsable layout yields a
//! silently wrong component. Common survival items only use earlier, unshifted
//! components. Items nested inside component values (bundles, containers) also
//! keep their source-version ids. On 26.3 a stack carrying one of the
//! components whose value layout changed (`pot_decorations`, inline `trim` or
//! `instrument`, a `teleport_randomly` consume effect) misdecodes the same
//! way, and recipe displays keep their items in wire space.
//!
//! Depends on azalea diverging from 26.2: these translations are correct only
//! because azalea encodes or decodes something differently from the reference,
//! so fixing azalea — or replacing it with pomme's own codec — breaks the older
//! versions unless the matching rewrite lands at the same time. Each site
//! carries a `TODO` pointing here.
//! - outbound `set_creative_mode_slot` leaves component values undelimited,
//!   which is 1.21.4's layout rather than 26.2's (native sessions and the
//!   versions with an item grammar encode it with pomme's grammar instead; see
//!   `net::item_codec`)

use std::io::Cursor;
use std::sync::{Mutex, OnceLock};

use azalea_buf::AzBuf;
use azalea_inventory::components::Profile;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_protocol::packets::game::s_container_click::HashedStack;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_registry::Registry;
use azalea_registry::builtin::DataComponentKind;
use glam::DVec3;
use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::item::ItemCodec;
pub use pomme_protocol::version::joinable;
use pomme_protocol::version::{NATIVE, TRANSLATED};
use pomme_protocol::{
    ClientRegistry, Direction, PacketTable, Phase, RegistryRemaps, RegistryTable, wire,
};

use crate::recipe::MAX_SLOT_DISPLAY_DEPTH;

type ContextRewrite = fn(&Translation, u32, &[u8], &mut SessionState) -> Option<Vec<u8>>;

/// A fully resolved payload rewrite; dispatch never searches version gates.
#[derive(Clone, Copy)]
enum Rewrite {
    Context(ContextRewrite),
    Frame(FrameRewrite),
    SessionFrame(fn(u32, &[u8], &SessionState) -> Option<Vec<u8>>),
    Item(ItemRewrite),
    Recipe(RecipeTranslator),
}

pub struct Translation {
    rewrites: Box<[Option<Rewrite>]>,
    registry_data_id: u32,
    to_native: &'static RegistryRemaps,
    from_native: &'static RegistryRemaps,
    login_finished_id: u32,
    login_hello_id: u32,
    login_profile_strict: bool,
    login_hello_bare: bool,
    /// The native serverbound `hello` id when the wire version wraps its
    /// profile id in an optional (1.20.2 made it mandatory); `None` needs no
    /// outbound rewrite.
    login_hello_optional_uuid: Option<u32>,
    /// Whether `login_finished` lacks the session-id UUID 26.2 appended.
    login_session_pad: bool,
    /// The rewrites 26.2 introduced, for wire versions below it.
    v775: Option<Ids775>,
    /// Handled outside [`GameIds`]: the attribute ids need remapping even on
    /// a version whose packet ids all match the native (26.1).
    update_attributes_id: u32,
    /// Recipe bodies carry static-registry ids that still need remapping when
    /// packet ids are otherwise native-identical (notably 26.1).
    recipe_book_add_id: u32,
    update_recipes_id: u32,
    place_ghost_recipe_id: u32,
    /// Recipe frame layout from 1.21.2 (768) on; `None` before display ids.
    recipe_layout: Option<RecipeLayout>,
    /// Game-phase packet-id translation and the rewrites tied to it; `None`
    /// when the wire version's ids match the native (26.1).
    game_ids: Option<GameIds>,
    /// Configuration-phase translation; `None` when the wire version's
    /// config ids match the native (766 up: additions were appended).
    config_ids: Option<ConfigIds>,
    /// Whether the wire version predates the configuration phase (1.20.2
    /// introduced it), so the join skips it rather than translating it.
    no_config_phase: bool,
}

/// Configuration-phase id tables and payload rewrites for wire versions whose
/// config protocol diverged: ids and registry data up to 765, and
/// `client_information`'s layout up to 767.
struct ConfigIds {
    /// Wire-version clientbound id -> native id; `None` drops the frame.
    inbound: Box<[Option<u32>]>,
    /// Native serverbound id -> wire-version id; `None` suppresses.
    outbound: Box<[Option<u32>]>,
    /// Native-space `registry_data` on wire versions at or below 765, whose
    /// form is one packet holding every registry as a single NBT map.
    split_registry_data: Option<u32>,
    /// 764's config payload rewrites: disconnect's JSON component and the
    /// unsplit resource_pack (both phases share the layouts).
    v764: Option<ConfigIds764>,
    /// Native `client_information` id, which loses 1.21.2's `particleStatus`.
    v767_client_information_id: Option<u32>,
    /// Whether every id maps to itself (766 and 767, kept only for the
    /// outbound rewrite), so inbound frames pass through untouched.
    inbound_identity: bool,
}

/// Native config-space ids for the 764 payload rewrites.
struct ConfigIds764 {
    disconnect_id: u32,
    resource_pack_push_id: u32,
    resource_pack_response_id: u32,
}

/// Game-phase id tables for a wire version whose ids diverged from the
/// native, plus the native-space ids its frame rewrites dispatch on.
struct GameIds {
    /// Wire-version clientbound id -> native id; `None` drops the frame
    /// (no native equivalent — none exist for 1.21.11/1.21.10, kept for
    /// safety).
    inbound: Box<[Option<u32>]>,
    /// Native serverbound id -> wire-version id; `None` suppresses the
    /// frame (the packet doesn't exist on the older version).
    outbound: Box<[Option<u32>]>,
    /// The wire version's `EntityDataSerializers` interleave (the
    /// registration order shifts between versions).
    serializer_map: &'static pomme_protocol::entity_serializers::SerializerTable,
    set_entity_data_id: u32,
    commands_id: u32,
    /// Names the wire-space ids the rewrites size by (command parsers,
    /// particles, slot and recipe displays).
    wire_registries: &'static RegistryTable,
    /// The rewrites 26.1 introduced, for wire versions below it.
    v774: Option<Ids774>,
    /// Whether the wire version's `entity_effect`/`tinted_leaves` particles
    /// carry a color int (1.20.5 added it); see [`translate_particles`].
    color_particles: bool,
    /// The rewrites 26.3 introduced, for wire versions at or above it.
    v777: Option<Ids777>,
    /// The item-carrying packets, for wire versions with a ported item
    /// grammar.
    items: Option<ItemIds>,
    /// The rewrites 1.21.9 introduced, for wire versions at or below it.
    v772: Option<Ids772>,
    /// The rewrites 1.21.6 introduced, for wire versions below it.
    v770: Option<Ids770>,
    /// The rewrites 1.21.5 introduced, for wire versions below it. Its
    /// presence also flags the NBT chunk heightmaps and string team scopes.
    v769: Option<Ids769>,
    /// The rewrites 1.21.4 introduced, for wire versions below it.
    v768: Option<Ids768>,
    /// The rewrites 1.21.2 introduced, for wire versions below it.
    v767: Option<Ids767>,
    /// The rewrites 1.21 introduced, for wire versions below it.
    v766: Option<Ids766>,
    /// The rewrites 1.20.5 introduced (the item-component era), for wire
    /// version 765; old-form items translate bare (type + count, NBT
    /// dropped).
    v765: Option<Ids765>,
    /// The rewrites 1.20.3 introduced (NBT text components, the scoreboard
    /// rework, the resource_pack split), for wire version 764.
    v764: Option<Ids764>,
    /// The rewrites 1.20.2 introduced, for wire version 763. Its presence
    /// also flags the named NBT roots, which every walker below reads.
    v763: Option<Ids763>,
    /// Native serverbound ids whose packet is knowingly absent on this wire
    /// version (`client_tick_end`, `player_loaded`, the pick pair); suppressed
    /// quietly.
    quiet_suppressed: Box<[u32]>,
}

/// Native-space dispatch ids for the frame rewrites protocols at or below
/// 775 need: 26.2 inserted `onlineMode` into game `login` and reordered
/// the team `Parameters`.
struct Ids775 {
    game_login_id: u32,
    set_player_team_id: u32,
}

/// Dispatch ids for the frame rewrites protocols at or below 774 need: 26.1
/// added the chunk-section `fluidCount`, the `set_time` clock map and the
/// serverbound `attack` split.
struct Ids774 {
    level_chunk_id: u32,
    set_time_id: u32,
    attack_id: u32,
    interact_id: u32,
    interact_old_id: u32,
}

/// Native-space dispatch ids for the frame rewrites protocols at or below
/// 768 need.
struct Ids768 {
    level_particles_id: u32,
}

/// Dispatch ids and synthesis targets for the 1.21.2 rework (protocol 767).
struct Ids767 {
    player_position_id: u32,
    teleport_entity_id: u32,
    /// The id `teleport_entity` rewrites onto; see
    /// [`translate_teleport_entity_767`].
    entity_position_sync_id: u32,
    respawn_id: u32,
    container_set_slot_id: u32,
    cooldown_id: u32,
    /// The ids the -1/-2 `container_set_slot` sentinels map onto.
    set_cursor_item_id: u32,
    set_player_inventory_id: u32,
    /// Inbound packets dropped quietly (`explode`, `update_recipes`,
    /// `place_ghost_recipe`).
    /// TODO: translate `explode` — 1.21.1 carries every field 26.2 needs, with
    /// an empty block-particle list like [`translate_explode`].
    drops: [u32; 3],
    player_input_id: u32,
    player_input_old_id: u32,
    /// The four `move_player_*` ids whose flags byte must drop to onGround.
    move_player_ids: [u32; 4],
    client_information_id: u32,
}

/// Native-space dispatch ids for the frame rewrites protocol 766 needs.
struct Ids766 {
    projectile_power_id: u32,
    /// Serverbound `use_item`: native + wire ids for the rotation strip.
    use_item_id: u32,
    use_item_old_id: u32,
}

/// Native-space dispatch ids for the frame rewrites protocol 765 needs.
struct Ids765 {
    container_set_content_id: u32,
    set_equipment_id: u32,
    merchant_offers_id: u32,
    update_mob_effect_id: u32,
    level_particles_id: u32,
    container_set_slot_id: u32,
    respawn_id: u32,
    /// Dropped quietly: the advancement icons are old-form items nested in
    /// display NBT the walker can't rewrite.
    /// TODO: rewrite the icons bare so 765 advancement toasts show.
    update_advancements_id: u32,
    /// Pre-1.20.5 chat packets carry a direct chat-type registry id where
    /// 1.20.5 put a holder (id + 1).
    player_chat_id: u32,
    disguised_chat_id: u32,
    server_data_id: u32,
    /// The wire version's registry names, for the attribute-key lookup.
    registry: &'static RegistryTable,
    /// Serverbound: native + wire ids for the item-form rewrites.
    container_click_id: u32,
    container_click_old_id: u32,
    creative_slot_id: u32,
    creative_slot_old_id: u32,
    chat_command_id: u32,
    chat_command_signed_id: u32,
    chat_command_old_id: u32,
}

/// Native-space dispatch ids for the frame rewrites protocol 764 needs:
/// the pre-1.20.3 JSON text components (`component_pass`), the scoreboard
/// rework, and the unsplit resource_pack packet.
struct Ids764 {
    system_chat_id: u32,
    set_action_bar_id: u32,
    set_title_id: u32,
    set_subtitle_id: u32,
    tab_list_id: u32,
    disconnect_id: u32,
    open_screen_id: u32,
    combat_kill_id: u32,
    boss_event_id: u32,
    set_player_team_id: u32,
    player_chat_id: u32,
    disguised_chat_id: u32,
    player_info_id: u32,
    command_suggestions_id: u32,
    set_objective_id: u32,
    set_score_id: u32,
    reset_score_id: u32,
    resource_pack_push_id: u32,
    /// Dropped quietly: component-bearing packets pomme never consumes
    /// (`server_data`, `map_item_data`).
    drops: [u32; 2],
    /// Serverbound: native + wire ids for the resource_pack reply rewrite.
    resource_pack_response_id: u32,
    resource_pack_response_old_id: u32,
}

/// Native-space dispatch ids for the frame rewrites protocol 769 needs.
struct Ids769 {
    player_chat_id: u32,
    update_advancements_id: u32,
    /// Native-space serverbound `chat` id and the wire version's, for the
    /// last-seen checksum strip.
    chat_id: u32,
    chat_old_id: u32,
    chat_command_signed_id: u32,
    chat_command_signed_old_id: Option<u32>,
    /// Native-space serverbound `container_click` id and the wire
    /// version's, for the hashed-stack rewrite.
    container_click_id: u32,
    container_click_old_id: u32,
}

/// Dispatch ids for the one outbound rewrite protocol 770 needs (see
/// [`translate_player_command`]), and the `chunks_biomes` it drops.
struct Ids770 {
    player_command_id: u32,
    player_command_old_id: u32,
    chunks_biomes_id: u32,
}

/// Native-space dispatch ids for the frame rewrites only protocols at or
/// below 772 need (the layouts 1.21.9 changed). Its presence also flags the
/// pre-1.21.9 entity-data serializer set and `profile` component layout.
struct Ids772 {
    add_entity_id: u32,
    set_entity_motion_id: u32,
    player_rotation_id: u32,
    set_default_spawn_id: u32,
    explode_id: u32,
}

/// A version-specific frame rewriter: native-space id + payload to the
/// rewritten frame, `None` when malformed.
type FrameRewrite = fn(u32, &[u8]) -> Option<Vec<u8>>;

/// Dispatch ids for the frame rewrites 26.3 (the first wire version above
/// native) needs; `version_rewrite` consults it last, after every older
/// gate. Native-space unless named `_old_id`.
struct Ids777 {
    move_entity_pos_id: u32,
    move_entity_pos_rot_id: u32,
    move_entity_rot_id: u32,
    entity_position_sync_id: u32,
    explode_id: u32,
    login_id: u32,
    respawn_id: u32,
    animate_id: u32,
    level_particles_id: u32,
    update_advancements_id: u32,
    level_chunk_id: u32,
    light_update_id: u32,
    player_chat_id: u32,
    recipe_book_add_id: u32,
    place_ghost_recipe_id: u32,
    update_recipes_id: u32,
    /// Rewritten onto `animate` before the id map would drop it.
    swing_animation_old_id: u32,
    swing_id: u32,
    punch_old_id: u32,
    player_action_id: u32,
    player_action_old_id: u32,
    accept_teleportation_id: u32,
    accept_teleportation_old_id: u32,
    move_player_pos_rot_id: u32,
}

/// Native-space dispatch ids for the frame rewrites protocol 763 needs, plus
/// the wire ids and entity type the `add_player` rewrite synthesizes with.
struct Ids763 {
    respawn_id: u32,
    forget_level_chunk_id: u32,
    block_entity_data_id: u32,
    tag_query_id: u32,
    add_player_old_id: u32,
    add_entity_old_id: u32,
    /// The wire version's `player` entity type; `remap_inbound` turns it into
    /// the native id after the frame decodes.
    player_entity_type: u32,
}

/// Packets renamed between versions with identical fields; name matching
/// treats each pair as the same packet.
const RENAMED: &[(&str, &str)] = &[
    // `ClientboundHorseScreenOpenPacket` vs `ClientboundMountScreenOpenPacket`
    // in the references, byte-identical write() bodies.
    // TODO: convert 766's slot count to 1.21's column count once a mount
    // inventory screen exists (see the 1.20.6 changelog above).
    ("horse_screen_open", "mount_screen_open"),
    // Renamed by 1.21.2, byte-identical single-slot bodies.
    ("set_carried_item", "set_held_slot"),
    // 1.20.3 split the clientbound packet into push/pop; the unsplit 1.20.2
    // form maps to push (`translate_resource_pack_764` synthesizes its
    // UUID). The serverbound reply kept the name and matches directly.
    ("resource_pack", "resource_pack_push"),
];

/// `ServerboundInteractPacket` action ordinals on versions where attacking
/// is an `interact` action (`INTERACT` carries a hand, `ATTACK` nothing,
/// `INTERACT_AT` a hit location then a hand).
const ACTION_INTERACT: u32 = 0;
const ACTION_ATTACK: u32 = 1;
const ACTION_INTERACT_AT: u32 = 2;

/// The two fixed spans a 1.20.1 `login` opens with: the player id and hardcore
/// bool, which 1.20.2 kept in place, then the game types it moved into the
/// spawn info.
const LOGIN_HEAD_763: usize = 5;
const LOGIN_PREFIX_763: usize = 7;

/// Native-registry component ids whose payloads the stack walker can advance
/// past (26.2 `DataComponents` registration order; anchored in
/// `component_id_anchors` in `azalea_compat`). Matching happens after the
/// remap, so one set of ids serves every wire version.
pub(crate) const COMPONENT_MAP_ID: u32 = 46;
pub(crate) const COMPONENT_PROFILE: u32 = 70;

/// 26.2 particle names whose options carry a payload after the type id;
/// every older supported version's payload set is a strict subset of this
/// one, so a name outside it is a bare type id on both sides.
const PAYLOAD_PARTICLES: &[&str] = &[
    "block",
    "block_crumble",
    "block_marker",
    "dragon_breath",
    "dust",
    "dust_color_transition",
    "dust_pillar",
    "effect",
    "entity_effect",
    "falling_dust",
    "flash",
    "geyser",
    "geyser_base",
    "geyser_plume",
    "geyser_poof",
    "instant_effect",
    "item",
    "sculk_charge",
    "shriek",
    "tinted_leaves",
    "trail",
    "vibration",
];

/// Native-space ids of the packets carrying item stacks, rewritten through
/// the wire version's item grammar for versions that have one ported.
struct ItemIds {
    codec: &'static ItemCodec,
    container_set_content_id: u32,
    container_set_slot_id: u32,
    set_cursor_item_id: u32,
    set_player_inventory_id: u32,
    set_equipment_id: u32,
    merchant_offers_id: u32,
    set_creative_mode_slot_id: u32,
}

/// Translates item stacks from the wire version's grammar into native ids.
#[derive(Clone, Copy)]
struct Items<'a> {
    codec: &'static ItemCodec,
    remaps: &'a RegistryRemaps,
}

type ItemRewrite = fn(u32, &[u8], Items<'_>) -> Option<Vec<u8>>;

/// The zero bytes a version's chunk section buffer ends in, per container:
/// `getSerializedSize` sized it for length varints that don't match what
/// `write` sent.
#[derive(Clone, Copy)]
enum SectionPadding {
    None,
    /// 1.20.1 sized each length for the entry count, not the long count.
    EntrySized,
    /// 1.21.5 sized a length it no longer writes.
    UnwrittenLength,
}

/// Normalizes post-1.21.2 recipe-book entries into 26.2's static-registry
/// id space before azalea parses them. The recipe-display registry itself is
/// stable across these versions, but slot-display, item and component ids
/// are not (1.21.11's `item` slot is id 2; 26.2 id 2 means
/// `with_any_potion`).
/// How a pre-26.2 recipe frame's slot displays differ from 26.2's, beyond
/// the registry remap (decompiled 1.21.4 / 1.21.5 / 1.21.11 `SlotDisplay`).
#[derive(Clone, Copy)]
struct RecipeLayout {
    /// Pre-1.21.9 `profile` component layout.
    old_profile: bool,
    /// Through 1.21.11 an `item_stack` display wraps a non-empty `ItemStack`
    /// (count, item, patch) rather than 26.1's `ItemStackTemplate`.
    item_stack_count_first: bool,
    /// Through 1.21.4 `smithing_trim`'s pattern is a `SlotDisplay` (the
    /// template item), not a `Holder<TrimPattern>`.
    trim_pattern_display: bool,
}

#[derive(Clone, Copy)]
struct RecipeCtx<'a> {
    remaps: &'a RegistryRemaps,
    layout: RecipeLayout,
}

type RecipeTranslator = fn(u32, &[u8], RecipeCtx<'_>) -> Option<Vec<u8>>;

/// Native `smithing_trim` slot-display id.
const SMITHING_TRIM_DISPLAY: u32 = 8;

mod shared;
use shared::*;

mod v768;

mod v767;
use v767::*;

mod v766;
use v766::*;

mod v765;
use v765::*;

mod v764;
use v764::*;

mod v769;
use v769::*;

mod v772;
use v772::*;

mod v777;
use v777::*;

mod v763;
use v763::*;

mod tables;

mod inbound;

mod outbound;

mod remaps;

mod v770;
use v770::*;

mod components;
use components::*;

mod v774;
use v774::*;

mod v775;
use v775::*;

mod metadata;
use metadata::*;

mod items;
use items::*;

mod chunks;
use chunks::*;

mod recipes;

#[cfg(test)]
mod recipe_translation_tests;

#[cfg(test)]
mod dispatch_tests;

pub use shared::{active, native_id, prewarm};

mod session;
use session::SessionState;
pub use session::TranslationSession;
