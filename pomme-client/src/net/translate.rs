//! Wire translation for non-native protocol versions.
//!
//! The client speaks 26.2 natively (`pomme_protocol::version::NATIVE`). When
//! a connection negotiates a different wire version, inbound frames are
//! rewritten into the native layout before azalea's typed decode where the
//! wire format changed, and static-registry ids (which shift between versions)
//! are remapped in both directions so the rest of the client stays
//! single-version. Layouts were line-checked against the decompiled
//! references (`reference/<version>/decompiled/.../network/protocol/`).
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
//!   cushion, whose entity type (like the poplar boats) has no native id;
//!   `data_component_type` diverges from 40 (`swing_animation` and `map_color`
//!   removed, thirteen components added), and `pot_decorations`, inline trim
//!   materials and instruments, and `teleport_randomly` changed their value
//!   layout (patches carrying them can't be sized)
//! - `ByteBufCodecs.BIT_SET` (a byte array) replaced the long-array bitset in
//!   the light masks (`level_chunk_with_light`, `light_update`) and the
//!   `player_chat` partial filter mask
//! - `commands` inserted five singleton argument types (the parser ids are
//!   remapped by name; the 26.3-only ones become the parser 26.2 sent)
//! - the `tag` slot display carries a `HolderSet` rather than a tag id, and
//!   26.3 routes every ingredient through it (`recipe_book_add`,
//!   `place_ghost_recipe`, `update_recipes`)
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
//! - `change_difficulty` read an unsigned byte where 1.21.6 reads a varint;
//!   difficulty ids fit a single byte either way, so the wire bytes are
//!   identical
//! - 1.21.6 gave `HangingEntity` a synched `direction` at index 8, so an item
//!   frame's item/rotation sit at 8/9 here and a painting's variant at 8 (9/10
//!   and 9 on 26.2); the walker passes index bytes through unremapped and pomme
//!   renders neither entity, so the shift is left in place
//!
//! 1.21.4 -> 26.2 wire changes (all of 1.21.5's plus):
//! - chunk heightmaps were a network-NBT compound of named long arrays; 1.21.5
//!   packed them into a (type id, long array) list
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
//!   (`ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC`); Pomme's raw creative writer
//!   handles that framing boundary explicitly
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
//! - `cooldown` carries an item registry id where 26.2 names a cooldown group
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
//! - player inventory/equipment item patches get a targeted component rewrite:
//!   vanilla enchantments are remapped, mining metadata is preserved, Tool
//!   removals survive, and unsupported old Tool values degrade to native
//!   per-item defaults without dropping the surrounding packet
//! - serverbound `set_creative_mode_slot` rewrites the supported component
//!   subset back into 1.20.5/1.20.6 ids/layouts while scrubbing client-private
//!   mining metadata
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
//! - items are `bool + id + byte count + NBT`; inbound player-inventory and
//!   equipment stacks preserve their legacy NBT as latest `custom_data` so
//!   mining can still inspect enchantments, while several unrelated old-form
//!   item paths still translate bare (merchant offers, entity-data items, and
//!   outbound `container_click`/`set_creative_mode_slot`)
//! - the configuration phase diverges for the first time: ids remap, and the
//!   single whole-holder NBT `registry_data` packet fans out into the
//!   per-registry form (entries reordered by their explicit ids, which the
//!   client equates with wire order); the dimension-type order is kept for the
//!   spawn-info rewrites, whose dimension type is a resource key string at 765
//! - login-phase `hello` lacks the shouldAuthenticate bool (synthesized true)
//!   and game `login` lacks enforcesSecureChat
//! - `update_attributes` keys attributes by resource location and
//!   `update_mob_effect` has a byte amplifier plus trailing factor NBT
//! - `level_particles` leads with the particle type id; `chat_command` is
//!   always the signed form (empty signatures appended), so
//!   `chat_command_signed` goes out as it, checksum stripped
//! - `update_advancements` drops (old-form icons nested in display data);
//!   serializer ids from `particles` (18) up shift
//! - `player_chat`/`disguised_chat` carry a direct chat-type registry id where
//!   1.20.5 put a holder (bumped by one on the way through)
//!
//! 1.20.2 -> 26.2 wire changes (all of 1.20.4's plus; the pre-1.20.3 era):
//! - text components are length-prefixed JSON strings, not NBT
//!   (`FriendlyByteBuf.writeComponent`); `component_pass` transcodes every
//!   consumed component field via `json_to_nbt` (mixed arrays normalize to
//!   compound lists) and the entity-data component serializers (5/6) transcode
//!   in the metadata walk — `server_data` and `map_item_data` drop instead
//!   (pomme never reads them)
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
//! Component-era item patches on protocols 767-775 are normalized before
//! Azalea's native decode: outer component ids are remapped semantically,
//! known historical layout changes have exact walkers/transcoders, nested item
//! stacks recurse through the same normalizer, and registry-bearing payloads
//! that cannot be remapped losslessly are consumed and dropped individually.
//! Protocol 766 has its own targeted player-inventory/equipment rewrite. Tool
//! values on 766-773 degrade to Pomme's native per-item defaults; 774-775 keep
//! their layout-compatible Tool payloads and resolve direct block ids through
//! the negotiated block registry.
//!
//! Known limitation (accepted): protocol 777 / 26.3 component patches are not
//! normalized yet. Components at or after its first shifted id can therefore
//! decode under the wrong 26.2 codec, and 26.3-only value-layout changes can
//! misparse in the same way. Recipe displays also keep their item values in
//! wire space.
//!
//! Depends on azalea diverging from 26.2: these translations are correct only
//! because azalea encodes or decodes something differently from the reference,
//! so fixing azalea — or replacing it with pomme's own codec — breaks the older
//! versions unless the matching rewrite lands at the same time. Each site
//! carries a `TODO` pointing here.
//! - inbound `set_player_team` copies the color through as a plain
//!   `ChatFormatting` ordinal, where 26.2 writes an `Optional<TeamColor>`
//! - inbound `cooldown` carries an item registry id, where 26.2 names a
//!   cooldown group

use std::io::Cursor;
use std::sync::Mutex;

use azalea_buf::{AzBuf, AzBufVar};
use azalea_core::sound::CustomSound;
use azalea_inventory::components::{
    CreativeSlotLock, CustomData, CustomName, Damage, DataComponentUnion, EnchantmentGlintOverride,
    Enchantments, IntangibleProjectile, ItemName, MaxDamage, MaxStackSize, Profile, Rarity,
    RepairCost, Unbreakable,
};
use azalea_inventory::{DataComponentPatch, ItemStack, ItemStackData};
use azalea_protocol::packets::game::s_container_click::HashedStack;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_registry::builtin::{DataComponentKind, SoundEvent};
use azalea_registry::{Holder, Registry};
use glam::DVec3;
use pomme_protocol::version::NATIVE;
use pomme_protocol::{
    ClientRegistry, Direction, PacketTable, Phase, RegistryRemaps, RegistryTable, wire,
};

pub struct Translation {
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
    /// Game-phase packet-id translation and the rewrites tied to it; `None`
    /// when the wire version's ids match the native (26.1).
    game_ids: Option<GameIds>,
    /// Player inventory/equipment component normalization is independent of
    /// packet-id translation: 26.1's packet ids match 26.2, but its component
    /// registry still shifts after `sulfur_cube_content`.
    components: Option<ComponentIds>,
    /// Configuration-phase translation; `None` when the wire version's
    /// config ids match the native (766 up: additions were appended).
    config_ids: Option<ConfigIds>,
    /// Whether the wire version predates the configuration phase (1.20.2
    /// introduced it), so the join skips it rather than translating it.
    no_config_phase: bool,
}

/// Configuration-phase id tables and the registry-data rewrite for wire
/// versions whose config protocol diverged (765 and older).
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
    serializer_map: fn(u32) -> Option<u32>,
    set_entity_data_id: u32,
    /// The rewrites 26.1 introduced, for wire versions below it.
    v774: Option<Ids774>,
    /// Whether the wire version's `entity_effect`/`tinted_leaves` particles
    /// carry a color int (1.20.5 added it); see [`translate_particles`].
    color_particles: bool,
    /// The rewrites 26.3 introduced, for wire versions at or above it.
    v777: Option<Ids777>,
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
    /// version 765; player inventory/equipment preserve legacy NBT as
    /// `custom_data`, while unrelated old-form item paths may remain bare.
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

struct ComponentIds {
    protocol: i32,
    registry: &'static RegistryTable,
    container_set_content_id: u32,
    set_equipment_id: u32,
    container_set_slot_id: u32,
    set_cursor_item_id: u32,
    set_player_inventory_id: u32,
    creative_slot_id: u32,
    creative_slot_old_id: u32,
}

impl ComponentIds {
    fn build(protocol: i32, table: &PacketTable, native: &PacketTable) -> Option<Self> {
        if !(767..=775).contains(&protocol) {
            return None;
        }
        let id = |dir, name| required_id(native, Phase::Game, dir, name);
        Some(Self {
            protocol,
            registry: RegistryTable::for_protocol(protocol).expect("embedded registry table"),
            container_set_content_id: id(Direction::Clientbound, "container_set_content"),
            set_equipment_id: id(Direction::Clientbound, "set_equipment"),
            container_set_slot_id: id(Direction::Clientbound, "container_set_slot"),
            set_cursor_item_id: id(Direction::Clientbound, "set_cursor_item"),
            set_player_inventory_id: id(Direction::Clientbound, "set_player_inventory"),
            creative_slot_id: id(Direction::Serverbound, "set_creative_mode_slot"),
            creative_slot_old_id: required_id(
                table,
                Phase::Game,
                Direction::Serverbound,
                "set_creative_mode_slot",
            ),
        })
    }
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

impl Ids768 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        (id == self.level_particles_id).then_some(translate_level_particles_768 as FrameRewrite)
    }
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
}

impl Ids767 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.player_position_id => translate_player_position_767,
            i if i == self.respawn_id => translate_respawn_767,
            _ => return None,
        })
    }
}

/// Native-space dispatch ids for the frame rewrites protocol 766 needs.
struct Ids766 {
    projectile_power_id: u32,
    container_set_content_id: u32,
    set_equipment_id: u32,
    /// Serverbound ids for payloads that need a 1.20.5/1.20.6 layout rewrite.
    creative_slot_id: u32,
    creative_slot_old_id: u32,
    /// Serverbound `use_item`: native + wire ids for the rotation strip.
    use_item_id: u32,
    use_item_old_id: u32,
}

impl Ids766 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        (id == self.projectile_power_id).then_some(translate_projectile_power_766 as FrameRewrite)
    }
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

impl Ids765 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            // These two shadow 769's player_chat arm; the globalIndex
            // prepend is folded into the chat-type holder rewrite.
            i if i == self.player_chat_id => translate_player_chat_765,
            i if i == self.disguised_chat_id => translate_disguised_chat_765,
            i if i == self.update_mob_effect_id => translate_update_mob_effect_765,
            i if i == self.level_particles_id => translate_level_particles_765,
            i if i == self.respawn_id => translate_respawn_765,
            _ => return None,
        })
    }
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

impl Ids764 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.set_objective_id => translate_set_objective_764,
            i if i == self.resource_pack_push_id => translate_resource_pack_764,
            _ => return None,
        })
    }

    /// Rewrites the JSON components of a packet whose layout is otherwise
    /// unchanged, leaving the rest of the translation chain to run on the
    /// converted payload. Outer `None` = not a component packet;
    /// `Some(None)` = unparsable, drop the frame.
    #[allow(clippy::option_option)]
    fn component_pass(&self, id: u32, payload: &[u8]) -> Option<Option<Vec<u8>>> {
        let mut cur = Cursor::new(payload);
        let mut out = Vec::with_capacity(payload.len() + 16);
        let walked = match id {
            i if i == self.system_chat_id
                || i == self.set_action_bar_id
                || i == self.set_title_id
                || i == self.set_subtitle_id
                || i == self.disconnect_id =>
            {
                transcode_component(&mut cur, &mut out)
            }
            i if i == self.tab_list_id => transcode_component(&mut cur, &mut out)
                .and_then(|()| transcode_component(&mut cur, &mut out)),
            i if i == self.open_screen_id => copy_then_transcode(&mut cur, &mut out, 2),
            i if i == self.combat_kill_id => copy_then_transcode(&mut cur, &mut out, 1),
            i if i == self.boss_event_id => transcode_boss_event(&mut cur, &mut out),
            i if i == self.set_player_team_id => transcode_team(&mut cur, &mut out),
            i if i == self.player_chat_id => transcode_player_chat(&mut cur, &mut out),
            i if i == self.disguised_chat_id => transcode_component(&mut cur, &mut out)
                .and_then(|()| transcode_chat_type(&mut cur, &mut out)),
            i if i == self.player_info_id => transcode_player_info(&mut cur, &mut out),
            i if i == self.command_suggestions_id => transcode_suggestions(&mut cur, &mut out),
            _ => return None,
        };
        Some(walked.map(|()| {
            out.extend_from_slice(&payload[cur.position() as usize..]);
            out
        }))
    }
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

impl Ids769 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.player_chat_id => translate_player_chat,
            i if i == self.update_advancements_id => translate_update_advancements,
            _ => return None,
        })
    }
}

/// Dispatch ids for the one outbound rewrite protocol 770 needs (see
/// [`translate_player_command`]).
struct Ids770 {
    player_command_id: u32,
    player_command_old_id: u32,
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

impl Ids772 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.add_entity_id => translate_add_entity,
            i if i == self.set_entity_motion_id => translate_set_entity_motion,
            i if i == self.player_rotation_id => translate_player_rotation,
            i if i == self.set_default_spawn_id => translate_set_default_spawn,
            _ => return None,
        })
    }
}

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
    commands_id: u32,
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
    /// The teleport id of an `accept_teleportation` held back until the
    /// `move_player_pos_rot` carrying its pose; cleared on `login`.
    pending_accept: Mutex<Option<u32>>,
    /// Names the wire-space ids the rewrites size by (particles, command
    /// parsers, slot and recipe displays).
    wire_registries: &'static RegistryTable,
}

impl Ids777 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.move_entity_pos_id => translate_move_entity_pos_777,
            i if i == self.move_entity_pos_rot_id => translate_move_entity_pos_rot_777,
            i if i == self.move_entity_rot_id => translate_move_entity_rot_777,
            i if i == self.entity_position_sync_id => translate_entity_position_sync_777,
            i if i == self.login_id => translate_login_777,
            i if i == self.respawn_id => translate_respawn_777,
            i if i == self.animate_id => translate_animate_777,
            i if i == self.level_chunk_id => translate_chunk_777,
            i if i == self.light_update_id => translate_light_update_777,
            i if i == self.player_chat_id => translate_player_chat_777,
            _ => return None,
        })
    }
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

impl Ids763 {
    fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            // Shadows 765's respawn arm, which this one chains into.
            i if i == self.respawn_id => translate_respawn_763,
            i if i == self.forget_level_chunk_id => translate_forget_level_chunk_763,
            i if i == self.block_entity_data_id => translate_block_entity_data_763,
            i if i == self.tag_query_id => translate_tag_query_763,
            _ => return None,
        })
    }
}

/// Protocols the wire translation fully covers. A version with embedded
/// tables but no entry here (the staging state while its translation is
/// built) pings with the right version but stays un-joinable.
const TRANSLATED: &[i32] = &[
    777, 775, 774, 773, 772, 771, 770, 769, 768, 767, 766, 765, 764, 763,
];

/// Whether a server speaking `protocol` can be joined: the native version,
/// or another one with a complete wire translation. Gates both
/// wire-version negotiation and the server list's compatibility marker.
pub fn joinable(protocol: i32) -> bool {
    protocol == NATIVE.protocol || TRANSLATED.contains(&protocol)
}

/// The translation for the wire version negotiated with the current server,
/// or `None` for the native version.
pub fn active() -> Option<&'static Translation> {
    let protocol = crate::version::session_protocol();
    if protocol == NATIVE.protocol {
        return None;
    }
    // One leaked entry per old protocol ever spoken (bounded by the embedded
    // version set); consulted per packet, so the hit path is one short scan.
    static CACHE: Mutex<Vec<(i32, Option<&'static Translation>)>> = Mutex::new(Vec::new());
    let mut cache = CACHE.lock().unwrap();
    if let Some(&(_, translation)) = cache.iter().find(|&&(p, _)| p == protocol) {
        return translation;
    }
    let translation = Translation::for_protocol(protocol).map(|t| &*Box::leak(Box::new(t)));
    if translation.is_some() {
        tracing::info!("Translating protocol {protocol} <-> {}", NATIVE.protocol);
    }
    cache.push((protocol, translation));
    translation
}

/// Builds the version-keyed caches a translated join needs (registry remaps,
/// packet tables, block-state table) without activating anything, so a join
/// after a server-list ping finds them warm. No-op when already built.
pub fn prewarm(protocol: i32) {
    let _ = Translation::for_protocol(protocol);
    crate::world::block::prewarm_protocol(protocol);
}

impl Translation {
    /// The translation for one protocol number, or `None` outside
    /// `TRANSLATED`: the frame rewrites below are version-specific, so
    /// embedded data alone isn't enough (and the native version needs none).
    pub(crate) fn for_protocol(protocol: i32) -> Option<Translation> {
        if !TRANSLATED.contains(&protocol) {
            return None;
        }
        let table = PacketTable::for_protocol(protocol)?;
        let native = PacketTable::native();
        let id = |phase, name| required_id(native, phase, Direction::Clientbound, name);
        Some(Translation {
            to_native: RegistryRemaps::to_native(protocol)?,
            from_native: RegistryRemaps::from_native(protocol)?,
            // Login-phase ids are identical across all supported versions.
            login_finished_id: id(Phase::Login, "login_finished"),
            login_hello_id: id(Phase::Login, "hello"),
            // 1.20.5 appended strictErrorHandling to game_profile; 1.21.2
            // replaced it with the session-id UUID 26.2 still carries.
            login_profile_strict: matches!(protocol, 766 | 767),
            login_hello_bare: protocol <= 765,
            login_hello_optional_uuid: (protocol <= 763)
                .then(|| required_id(native, Phase::Login, Direction::Serverbound, "hello")),
            login_session_pad: protocol <= 775,
            v775: (protocol <= 775).then(|| Ids775 {
                game_login_id: id(Phase::Game, "login"),
                set_player_team_id: id(Phase::Game, "set_player_team"),
            }),
            update_attributes_id: id(Phase::Game, "update_attributes"),
            game_ids: GameIds::build(protocol, table, native),
            components: ComponentIds::build(protocol, table, native),
            config_ids: ConfigIds::build(protocol, table, native),
            no_config_phase: table
                .name_of(Phase::Configuration, Direction::Clientbound, 0)
                .is_none(),
        })
    }

    /// Rewrites a native-layout serverbound login frame into the wire
    /// version's. Only 763's `hello` differs, by wrapping the profile id in an
    /// optional.
    pub fn translate_outbound_login_frame(&self, frame: Vec<u8>) -> Vec<u8> {
        let Some(hello_id) = self.login_hello_optional_uuid else {
            return frame;
        };
        let mut pos = 0;
        if wire::read_varint(&frame, &mut pos) != Some(hello_id) {
            return frame;
        }
        let mut cur = Cursor::new(&frame[pos..]);
        if skip_utf(&mut cur).is_none() {
            return frame;
        }
        let uuid_at = pos + cur.position() as usize;
        let mut out = Vec::with_capacity(frame.len() + 1);
        out.extend_from_slice(&frame[..uuid_at]);
        out.push(1); // the profile id is present
        out.extend_from_slice(&frame[uuid_at..]);
        out
    }

    /// Rewrites a raw login-phase frame into the native layout.
    pub fn translate_login_frame(&self, raw: Box<[u8]>) -> Box<[u8]> {
        let mut cur = Cursor::new(&raw[..]);
        let id = u32::azalea_read_var(&mut cur).ok();
        if self.login_session_pad && id == Some(self.login_finished_id) {
            // 26.2 appended a session-id UUID; zero is fine, pomme only
            // reads the game profile. The 1.20.5/1.21-era trailing
            // strictErrorHandling bool goes first (the UUID took its place).
            let mut out = raw.into_vec();
            if self.login_profile_strict {
                out.pop();
            }
            out.extend_from_slice(&[0; 16]);
            return out.into_boxed_slice();
        }
        if self.login_hello_bare && id == Some(self.login_hello_id) {
            // 1.20.5 appended shouldAuthenticate; an encrypting server on
            // the older wire always authenticates.
            let mut out = raw.into_vec();
            out.push(1);
            return out.into_boxed_slice();
        }
        raw
    }

    /// Whether configuration frames need translation (765 and older).
    pub fn translates_config(&self) -> bool {
        self.config_ids.is_some()
    }

    /// Whether the wire version has no configuration phase at all, so the join
    /// sends no `login_acknowledged` and reads the registries out of the game
    /// `login` packet instead.
    pub fn no_config_phase(&self) -> bool {
        self.no_config_phase
    }

    /// Splits the registry codec out of a 1.20.1 game `login` frame into the
    /// per-registry `registry_data` packets the configuration phase carries
    /// from 1.20.2 on, capturing the dimension-type order on the way.
    pub fn split_login_registries(&self, raw: &[u8]) -> Option<Vec<Box<[u8]>>> {
        let id = required_id(
            PacketTable::native(),
            Phase::Configuration,
            Direction::Clientbound,
            "registry_data",
        );
        let mut pos = 0;
        wire::read_varint(raw, &mut pos)?;
        let mut cur = Cursor::new(&raw[pos..]);
        seek_login_codec_763(&mut cur)?;
        let mut codec = Vec::new();
        copy_unnamed_nbt(&mut cur, &mut codec, true)?;
        split_registry_data(id, &codec)
    }

    /// Rewrites a raw configuration-phase frame into native-layout frames;
    /// empty = dropped. 765's single registry_data packet fans out into one
    /// frame per registry.
    pub fn translate_config_frame(&self, raw: Box<[u8]>) -> Vec<Box<[u8]>> {
        let Some(ids) = &self.config_ids else {
            return vec![raw];
        };
        let mut pos = 0;
        let Some(wire_id) = wire::read_varint(&raw, &mut pos) else {
            return Vec::new();
        };
        let Some(id) = ids.inbound.get(wire_id as usize).copied().flatten() else {
            tracing::debug!("Dropping inbound config packet {wire_id} with no native id");
            return Vec::new();
        };
        if ids.split_registry_data == Some(id) {
            return match split_registry_data(id, &raw[pos..]) {
                Some(frames) => frames,
                None => {
                    tracing::warn!("Dropping unparsable registry data");
                    Vec::new()
                }
            };
        }
        if let Some(v) = &ids.v764 {
            let rewritten = if id == v.disconnect_id {
                let mut cur = Cursor::new(&raw[pos..]);
                let mut out = Vec::with_capacity(raw.len() + 8);
                wire::write_varint(&mut out, id);
                transcode_component(&mut cur, &mut out).map(|()| out)
            } else if id == v.resource_pack_push_id {
                translate_resource_pack_764(id, &raw[pos..])
            } else {
                return plain_config_frame(id, &raw[pos..]);
            };
            return match rewritten {
                Some(out) => vec![out.into_boxed_slice()],
                None => {
                    tracing::warn!("Dropping unparsable config packet {id}");
                    Vec::new()
                }
            };
        }
        plain_config_frame(id, &raw[pos..])
    }

    /// Translates a native-layout serverbound configuration frame into the
    /// wire version's; `None` suppresses it. Only 764's resource_pack reply
    /// changes layout; everything else is an id remap.
    pub fn translate_outbound_config_frame(&self, frame: Vec<u8>) -> Option<Vec<u8>> {
        let Some(ids) = &self.config_ids else {
            return Some(frame);
        };
        let mut pos = 0;
        let id = wire::read_varint(&frame, &mut pos)?;
        if ids
            .v764
            .as_ref()
            .is_some_and(|v| id == v.resource_pack_response_id)
        {
            let old = ids.outbound.get(id as usize).copied().flatten()?;
            return translate_resource_pack_response_764(old, &frame[pos..]).pop();
        }
        match ids.outbound.get(id as usize).copied().flatten() {
            Some(old) if old == id => Some(frame),
            Some(old) => {
                let mut out = Vec::with_capacity(frame.len() + 1);
                wire::write_varint(&mut out, old);
                out.extend_from_slice(&frame[pos..]);
                Some(out)
            }
            None => {
                tracing::debug!("Suppressing outbound config packet {id} the wire version lacks");
                None
            }
        }
    }

    /// Rewrites a raw game-phase frame into the native layout; `None` drops
    /// the packet (malformed beyond repair, or without a native equivalent).
    pub fn translate_game_frame(&self, raw: Box<[u8]>) -> Option<Box<[u8]>> {
        let mut id_end = 0;
        let wire_id = wire::read_varint(&raw, &mut id_end)?;
        // add_player has no native equivalent, so it becomes an add_entity
        // before the id map below would drop it.
        if let Some(v) = self.game_ids.as_ref().and_then(|g| g.v763.as_ref())
            && wire_id == v.add_player_old_id
        {
            let frame = translate_add_player_763(v, &raw[id_end..])?;
            return self.translate_game_frame(frame.into_boxed_slice());
        }
        if let Some(v) = self.game_ids.as_ref().and_then(|g| g.v777.as_ref())
            && wire_id == v.swing_animation_old_id
        {
            return translate_swing_animation_777(v.animate_id, &raw[id_end..]);
        }
        let id = match &self.game_ids {
            Some(ids) => {
                let Some(native) = ids.inbound.get(wire_id as usize).copied().flatten() else {
                    tracing::debug!("Dropping inbound game packet {wire_id} with no native id");
                    return None;
                };
                native
            }
            None => wire_id,
        };

        let v777 = self.game_ids.as_ref().and_then(|g| g.v777.as_ref());
        if let Some(v) = v777.filter(|v| id == v.login_id) {
            // A new session; an accept held over from the last one is stale.
            *v.pending_accept.lock().unwrap() = None;
        }
        let v769 = self.game_ids.as_ref().is_some_and(|g| g.v769.is_some());
        let v767 = self.game_ids.as_ref().and_then(|g| g.v767.as_ref());
        let v766 = self.game_ids.as_ref().and_then(|g| g.v766.as_ref());
        let v765 = self.game_ids.as_ref().and_then(|g| g.v765.as_ref());
        if v767.is_some_and(|v| v.drops.contains(&id)) {
            tracing::debug!("Dropping game packet {id} with no 1.21.1 equivalent layout");
            return None;
        }
        if v765.is_some_and(|v| id == v.update_advancements_id) {
            tracing::debug!("Dropping update_advancements with old-form icons");
            return None;
        }
        let v764 = self.game_ids.as_ref().and_then(|g| g.v764.as_ref());
        if v764.is_some_and(|v| v.drops.contains(&id)) {
            tracing::debug!("Dropping unconsumed component packet {id}");
            return None;
        }
        // The 764 JSON components convert first; the rest of the chain then
        // runs on the (765-form) converted payload.
        let converted = match v764.and_then(|v| v.component_pass(id, &raw[id_end..])) {
            Some(Some(payload)) => Some(payload),
            Some(None) => {
                tracing::warn!("Dropping game packet {id} with an unparsable component");
                return None;
            }
            None => None,
        };
        let payload: &[u8] = converted.as_deref().unwrap_or(&raw[id_end..]);
        let v775 = self.v775.as_ref();
        let rewritten = if v775.is_some_and(|v| id == v.game_login_id) {
            let v763 = self.game_ids.as_ref().and_then(|g| g.v763.as_ref());
            let old = if v763.is_some() {
                translate_game_login_763(payload)
                    .as_deref()
                    .and_then(translate_game_login_765)
            } else if v765.is_some() {
                translate_game_login_765(payload)
            } else {
                Some(payload.to_vec())
            };
            old.and_then(|p| {
                if v767.is_some() {
                    insert_sea_level(&p).and_then(|p| translate_game_login(id, &p))
                } else {
                    translate_game_login(id, &p)
                }
            })
        } else if id == self.update_attributes_id {
            // Oldest gate first: 766 and below carry UUID modifier ids the
            // shared rewrite would copy through as a resource location, and
            // 765 names the attribute by resource location rather than id.
            if v766.is_some() {
                translate_update_attributes_uuid(
                    v765.map(|v| v.registry),
                    self.to_native,
                    id,
                    payload,
                )
            } else {
                translate_update_attributes(self.to_native, id, payload)
            }
        } else if v775.is_some_and(|v| id == v.set_player_team_id) {
            translate_team(id, payload, v769)
        } else if self
            .components
            .as_ref()
            .is_some_and(|c| id == c.container_set_content_id)
        {
            translate_container_set_content_components(
                id,
                payload,
                self.components.as_ref().unwrap(),
                self.to_native,
            )
        } else if self
            .components
            .as_ref()
            .is_some_and(|c| id == c.set_equipment_id)
        {
            translate_set_equipment_components(
                id,
                payload,
                self.components.as_ref().unwrap(),
                self.to_native,
            )
        } else if let Some(c) = self
            .components
            .as_ref()
            .filter(|c| id == c.container_set_slot_id)
        {
            if c.protocol == 767 {
                v767.and_then(|v| {
                    translate_container_set_slot_components_767(v, payload, c, self.to_native)
                })
            } else {
                translate_container_set_slot_components(id, payload, c, self.to_native)
            }
        } else if let Some(c) = self
            .components
            .as_ref()
            .filter(|c| id == c.set_cursor_item_id)
        {
            translate_set_cursor_item_components(id, payload, c, self.to_native)
        } else if let Some(c) = self
            .components
            .as_ref()
            .filter(|c| id == c.set_player_inventory_id)
        {
            translate_set_player_inventory_components(id, payload, c, self.to_native)
        } else if let Some(ids) = &self.game_ids {
            // Pre-1.20.2 wire NBT carries an empty root name the native
            // version's readers don't expect.
            let named_nbt = ids.v763.is_some();
            if id == ids.set_entity_data_id {
                translate_entity_data(id, payload, ids, self.to_native)
            } else if ids.v774.as_ref().is_some_and(|v| id == v.level_chunk_id) {
                translate_chunk(id, payload, v769, named_nbt)
            } else if ids.v774.as_ref().is_some_and(|v| id == v.set_time_id) {
                if v767.is_some() {
                    translate_set_time_767(id, payload)
                } else {
                    translate_set_time(id, payload)
                }
            } else if let Some(v) = v764.filter(|v| id == v.set_score_id) {
                translate_set_score_764(v, payload)
            } else if v765.is_some_and(|v| id == v.container_set_content_id) {
                translate_container_set_content_765(id, payload, named_nbt)
            } else if v765.is_none() && v766.is_some_and(|v| id == v.container_set_content_id) {
                translate_container_set_content_766(id, payload, self.to_native)
            } else if v765.is_some_and(|v| id == v.set_equipment_id) {
                translate_set_equipment_765(id, payload, named_nbt)
            } else if v765.is_none() && v766.is_some_and(|v| id == v.set_equipment_id) {
                translate_set_equipment_766(id, payload, self.to_native)
            } else if v765.is_some_and(|v| id == v.merchant_offers_id) {
                translate_merchant_offers_765(id, payload, named_nbt)
            } else if v765.is_some_and(|v| id == v.container_set_slot_id) {
                v767.and_then(|v| translate_container_set_slot_765(v, payload, named_nbt))
            } else if v765.is_none()
                && v766.is_some()
                && v767.is_some_and(|v| id == v.container_set_slot_id)
            {
                v767.and_then(|v| translate_container_set_slot_766(v, payload, self.to_native))
            } else if ids.v772.as_ref().is_some_and(|v| id == v.explode_id) {
                translate_explode(id, payload, ids, self.to_native)
            } else if v777.is_some_and(|v| id == v.explode_id) {
                translate_explode_777(id, payload, ids, self.to_native)
            } else if let Some(v) = v777.filter(|v| id == v.level_particles_id) {
                match translate_level_particles_777(id, payload, v, self.to_native) {
                    Some(out) => Some(out),
                    None => {
                        tracing::debug!("Dropping level_particles with an unsizable payload");
                        return None;
                    }
                }
            } else if v777.is_some_and(|v| id == v.update_advancements_id) {
                translate_update_advancements_777(id, payload, self.to_native)
            } else if let Some(v) = v777.filter(|v| id == v.commands_id) {
                translate_commands_777(id, payload, v, self.to_native)
            } else if let Some(v) = v777.filter(|v| id == v.recipe_book_add_id) {
                translate_recipe_book_add_777(id, payload, v, self.to_native)
            } else if let Some(v) = v777.filter(|v| id == v.place_ghost_recipe_id) {
                translate_place_ghost_recipe_777(id, payload, v, self.to_native)
            } else if let Some(v) = v777.filter(|v| id == v.update_recipes_id) {
                translate_update_recipes_777(id, payload, v, self.to_native)
            } else if let Some(rewrite) = ids.version_rewrite(id) {
                rewrite(id, payload)
            } else if let Some(v) = v767.filter(|v| id == v.teleport_entity_id) {
                translate_teleport_entity_767(v, payload)
            } else if let Some(v) = v767.filter(|v| id == v.container_set_slot_id) {
                translate_container_set_slot_767(v, payload)
            } else if v767.is_some_and(|v| id == v.cooldown_id) {
                translate_cooldown_767(self.to_native, id, payload)
            } else if id == wire_id && converted.is_none() {
                return Some(raw);
            } else {
                let mut out = Vec::with_capacity(raw.len() + 1);
                wire::write_varint(&mut out, id);
                out.extend_from_slice(payload);
                return Some(out.into_boxed_slice());
            }
        } else {
            return Some(raw);
        };
        match rewritten {
            Some(out) => Some(out.into_boxed_slice()),
            None => {
                tracing::warn!("Dropping unparsable game packet {id}");
                None
            }
        }
    }

    /// Whether the wire version reports use swings: 26.3 replaced `swing`
    /// with `punch`, sent only for attacks (`Minecraft.startAttack`); the
    /// server swings on handling a use.
    pub fn reports_use_swings(&self) -> bool {
        self.game_ids.as_ref().is_none_or(|g| g.v777.is_none())
    }

    /// Whether outbound game frames need translation before hitting the
    /// wire (the version's serverbound ids or layouts diverge from native).
    pub fn translates_outbound(&self) -> bool {
        self.game_ids.is_some() || self.components.is_some()
    }

    /// Translates a native-layout serverbound game frame into the wire
    /// version's: id remap, `attack`/`interact` layout rewrites, and
    /// suppression of packets the older version lacks. Returns the frames to
    /// send (empty = suppressed, two for `interact`, one otherwise).
    pub fn translate_outbound_game_frame(&self, mut frame: Vec<u8>) -> Vec<Vec<u8>> {
        let mut pos = 0;
        let Some(id) = wire::read_varint(&frame, &mut pos) else {
            return Vec::new();
        };
        if let Some(components) = &self.components
            && id == components.creative_slot_id
        {
            return translate_creative_slot_components(
                components.creative_slot_old_id,
                &frame[pos..],
                components,
                self.from_native,
            )
            .into_iter()
            .collect();
        }
        let Some(ids) = &self.game_ids else {
            return vec![frame];
        };
        if let Some(v777) = &ids.v777 {
            if id == v777.swing_id {
                return translate_swing_777(v777.punch_old_id, &frame[pos..]);
            }
            if id == v777.player_action_id {
                return translate_player_action_777(v777.player_action_old_id, &frame[pos..]);
            }
            if id == v777.accept_teleportation_id {
                return translate_accept_teleportation_777(v777, &frame[pos..]);
            }
            if id == v777.move_player_pos_rot_id
                && let Some(frames) = translate_move_player_pos_rot_777(v777, &frame[pos..])
            {
                return frames;
            }
        }
        if let Some(v774) = &ids.v774 {
            if id == v774.attack_id {
                return translate_attack(v774.interact_old_id, &frame[pos..]);
            }
            if id == v774.interact_id {
                return translate_interact(v774.interact_old_id, &frame[pos..]);
            }
        }
        if let Some(v770) = &ids.v770
            && id == v770.player_command_id
        {
            return translate_player_command(v770.player_command_old_id, &frame[pos..]);
        }
        if let Some(v764) = &ids.v764
            && id == v764.resource_pack_response_id
        {
            return translate_resource_pack_response_764(
                v764.resource_pack_response_old_id,
                &frame[pos..],
            );
        }
        // The 765 arms precede 769's: both rewrite container_click, and the
        // older item form wins on the older wire.
        if let Some(v765) = &ids.v765 {
            if id == v765.container_click_id {
                return translate_container_click_765(v765.container_click_old_id, &frame[pos..]);
            }
            if id == v765.creative_slot_id {
                return translate_creative_slot_765(v765.creative_slot_old_id, &frame[pos..]);
            }
            if id == v765.chat_command_signed_id {
                return strip_last_seen_checksum(v765.chat_command_old_id, &frame[pos..]);
            }
            if id == v765.chat_command_id {
                return translate_chat_command_765(v765.chat_command_old_id, &frame[pos..]);
            }
        }
        if let Some(v769) = &ids.v769 {
            if id == v769.chat_id {
                return strip_last_seen_checksum(v769.chat_old_id, &frame[pos..]);
            }
            if id == v769.chat_command_signed_id
                && let Some(old_id) = v769.chat_command_signed_old_id
            {
                return strip_last_seen_checksum(old_id, &frame[pos..]);
            }
            if id == v769.container_click_id {
                return translate_container_click(v769.container_click_old_id, &frame[pos..]);
            }
        }
        if ids.v765.is_none()
            && let Some(v766) = &ids.v766
            && id == v766.creative_slot_id
        {
            return translate_creative_slot_766(
                v766.creative_slot_old_id,
                &frame[pos..],
                self.from_native,
            )
            .into_iter()
            .collect();
        }
        if let Some(v766) = &ids.v766
            && id == v766.use_item_id
        {
            return translate_use_item(v766.use_item_old_id, &frame[pos..]);
        }
        if let Some(v767) = &ids.v767 {
            if id == v767.player_input_id {
                return translate_player_input(v767.player_input_old_id, &frame[pos..]);
            }
            if v767.move_player_ids.contains(&id)
                && let Some(flags) = frame.last_mut()
            {
                // 1.21.2 turned the trailing onGround bool into a flag
                // bitfield; a 1.21.1 server reads any nonzero byte as
                // onGround, so the horizontal-collision bit must go.
                *flags &= 1;
            }
        }
        match ids.outbound.get(id as usize).copied().flatten() {
            Some(old) if old == id => vec![frame],
            Some(old) => {
                let mut out = Vec::with_capacity(frame.len() + 1);
                wire::write_varint(&mut out, old);
                out.extend_from_slice(&frame[pos..]);
                vec![out]
            }
            None => {
                if ids.quiet_suppressed.contains(&id) {
                    tracing::debug!("Suppressing outbound game packet {id} the wire version lacks");
                } else {
                    tracing::warn!("Suppressing outbound game packet {id} the wire version lacks");
                }
                Vec::new()
            }
        }
    }

    /// The native-version particle id for a source-version one, for the raw
    /// `level_particles` path; `None` drops the particle.
    pub fn remap_particle(&self, id: u32) -> Option<u32> {
        self.to_native.remap(ClientRegistry::ParticleType, id)
    }

    /// Remaps a decoded packet's static-registry ids into the native
    /// version's id space; `false` drops the packet (its subject no longer
    /// exists, e.g. the bed block entity removed in 26.2).
    pub fn remap_inbound(&self, packet: &mut ClientboundGamePacket) -> bool {
        use ClientRegistry as R;
        match packet {
            ClientboundGamePacket::AddEntity(p) => {
                remap_with(self.to_native, R::EntityType, &mut p.entity_type)
            }
            ClientboundGamePacket::Sound(p) => self.remap_sound(&mut p.sound),
            ClientboundGamePacket::SoundEntity(p) => self.remap_sound(&mut p.sound),
            // `update_attributes` is normalized at the raw-frame layer before
            // native-codec decode (including 26.1, whose packet ids otherwise
            // match native). Remapping here would apply the registry map twice.
            ClientboundGamePacket::UpdateAttributes(_) => true,
            ClientboundGamePacket::BlockEntityData(p) => {
                remap_with(self.to_native, R::BlockEntityType, &mut p.block_entity_type)
            }
            ClientboundGamePacket::LevelChunkWithLight(p) => {
                p.chunk_data
                    .block_entities
                    .retain_mut(|be| remap_with(self.to_native, R::BlockEntityType, &mut be.kind));
                true
            }
            ClientboundGamePacket::ContainerSetContent(p) => {
                for item in &mut p.items {
                    remap_stack(self.to_native, item);
                }
                remap_stack(self.to_native, &mut p.carried_item);
                true
            }
            ClientboundGamePacket::ContainerSetSlot(p) => {
                remap_stack(self.to_native, &mut p.item_stack);
                true
            }
            ClientboundGamePacket::SetCursorItem(p) => {
                remap_stack(self.to_native, &mut p.contents);
                true
            }
            ClientboundGamePacket::SetEntityData(p) => {
                for item in &mut p.packed_items.0 {
                    if let azalea_entity::EntityDataValue::ItemStack(stack) = &mut item.value {
                        remap_stack(self.to_native, stack);
                    }
                }
                true
            }
            ClientboundGamePacket::SetEquipment(p) => {
                for (_, stack) in &mut p.slots.slots {
                    remap_stack(self.to_native, stack);
                }
                true
            }
            ClientboundGamePacket::MerchantOffers(p) => {
                // An `ItemCost` has no empty form, so an untranslatable base
                // cost drops the offer rather than the whole trade list.
                p.offers.retain_mut(|offer| {
                    remap_stack(self.to_native, &mut offer.result);
                    if offer
                        .cost_b
                        .as_mut()
                        .is_some_and(|c| !remap_with(self.to_native, R::Item, &mut c.item))
                    {
                        offer.cost_b = None;
                    }
                    remap_with(self.to_native, R::Item, &mut offer.base_cost_a.item)
                });
                true
            }
            _ => true,
        }
    }

    /// Remaps an outbound packet's static-registry ids into the launched
    /// version's id space. Never drops the packet; entries the older version
    /// lacks degrade to empty (the server resyncs the slot).
    pub fn remap_outbound(&self, packet: &mut ServerboundGamePacket) {
        match packet {
            ServerboundGamePacket::ContainerClick(p) => {
                for (_, stack) in p.changed_slots.iter_mut() {
                    self.remap_hashed(stack);
                }
                self.remap_hashed(&mut p.carried_item);
            }
            ServerboundGamePacket::SetCreativeModeSlot(p) => {
                let component_protocol = self.components.is_some();
                let protocol_766 = self
                    .game_ids
                    .as_ref()
                    .is_some_and(|ids| ids.v766.is_some() && ids.v765.is_none());
                if component_protocol {
                    if let ItemStack::Present(data) = &mut p.item_stack
                        && !remap_with(self.from_native, ClientRegistry::Item, &mut data.kind)
                    {
                        p.item_stack = ItemStack::Empty;
                    }
                } else {
                    if protocol_766 && let ItemStack::Present(data) = &mut p.item_stack {
                        scrub_legacy_mining_metadata(data);
                        retain_creative_components_766(data);
                    }
                    remap_stack(self.from_native, &mut p.item_stack);
                    if !protocol_766 && let ItemStack::Present(data) = &mut p.item_stack {
                        strip_untranslatable_components(self.from_native, data);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn remap_sound(&self, sound: &mut Holder<SoundEvent, CustomSound>) -> bool {
        match sound {
            Holder::Reference(kind) => remap_with(self.to_native, ClientRegistry::SoundEvent, kind),
            Holder::Direct(_) => true,
        }
    }

    fn remap_hashed(&self, stack: &mut HashedStack) {
        use ClientRegistry as R;
        let Some(item) = &mut stack.0 else { return };
        if !remap_with(self.from_native, R::Item, &mut item.kind) {
            stack.0 = None;
            return;
        }
        item.components
            .added_components
            .retain_mut(|(kind, _)| remap_with(self.from_native, R::DataComponentType, kind));
        item.components
            .removed_components
            .retain_mut(|kind| remap_with(self.from_native, R::DataComponentType, kind));
    }
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

impl GameIds {
    /// The version-gated frame rewrite dispatching on `id`, if any. Chained
    /// oldest-first: every rewriter targets 26.2 directly, so where two gates
    /// claim a packet the older one subsumes the newer and must win (765's
    /// respawn over 767's, say). The gate above native comes last; it never
    /// shares a packet with the older ones.
    fn version_rewrite(&self, id: u32) -> Option<FrameRewrite> {
        self.v763
            .as_ref()
            .and_then(|v| v.rewrite(id))
            .or_else(|| self.v764.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v765.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v766.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v767.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v768.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v769.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v772.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v777.as_ref().and_then(|v| v.rewrite(id)))
    }

    /// Name-matched game-phase id tables between one wire version and the
    /// native. `None` when translation-by-id is a no-op: every inbound id
    /// maps to itself and every outbound id maps to itself or to nothing
    /// (26.1's only divergence is 26.2's `spectate_entity` ->
    /// `spectator_action` rename, which pomme never sends).
    fn build(protocol: i32, table: &PacketTable, native: &PacketTable) -> Option<GameIds> {
        use Direction::{Clientbound, Serverbound};
        let inbound = id_map(table, native, Phase::Game, Clientbound);
        let outbound = id_map(native, table, Phase::Game, Serverbound);
        if identity_maps(&inbound, &outbound) {
            return None;
        }
        let id = |dir, name| required_id(native, Phase::Game, dir, name);
        Some(GameIds {
            inbound,
            outbound,
            serializer_map: match protocol {
                777 => remap_serializer_777,
                774 => remap_serializer_774,
                773 => remap_serializer_773,
                // 1.21.5 through 1.21.8 register identical serializer sets,
                // as do 1.20.5 through 1.21.4.
                770..=772 => remap_serializer_772,
                766..=769 => remap_serializer_769,
                // 1.20.1 and 1.20.2 register serializers in the same order
                // as 1.20.4 (only the component read side changed).
                763..=765 => remap_serializer_765,
                p => panic!("no serializer map for protocol {p}"),
            },
            set_entity_data_id: id(Clientbound, "set_entity_data"),
            v774: (protocol <= 774).then(|| Ids774 {
                level_chunk_id: id(Clientbound, "level_chunk_with_light"),
                set_time_id: id(Clientbound, "set_time"),
                attack_id: id(Serverbound, "attack"),
                interact_id: id(Serverbound, "interact"),
                interact_old_id: required_id(table, Phase::Game, Serverbound, "interact"),
            }),
            color_particles: protocol >= 766,
            v777: (protocol >= 777).then(|| Ids777 {
                move_entity_pos_id: id(Clientbound, "move_entity_pos"),
                move_entity_pos_rot_id: id(Clientbound, "move_entity_pos_rot"),
                move_entity_rot_id: id(Clientbound, "move_entity_rot"),
                entity_position_sync_id: id(Clientbound, "entity_position_sync"),
                explode_id: id(Clientbound, "explode"),
                login_id: id(Clientbound, "login"),
                respawn_id: id(Clientbound, "respawn"),
                animate_id: id(Clientbound, "animate"),
                level_particles_id: id(Clientbound, "level_particles"),
                update_advancements_id: id(Clientbound, "update_advancements"),
                level_chunk_id: id(Clientbound, "level_chunk_with_light"),
                light_update_id: id(Clientbound, "light_update"),
                player_chat_id: id(Clientbound, "player_chat"),
                commands_id: id(Clientbound, "commands"),
                recipe_book_add_id: id(Clientbound, "recipe_book_add"),
                place_ghost_recipe_id: id(Clientbound, "place_ghost_recipe"),
                update_recipes_id: id(Clientbound, "update_recipes"),
                swing_animation_old_id: required_id(
                    table,
                    Phase::Game,
                    Clientbound,
                    "swing_animation",
                ),
                swing_id: id(Serverbound, "swing"),
                punch_old_id: required_id(table, Phase::Game, Serverbound, "punch"),
                player_action_id: id(Serverbound, "player_action"),
                player_action_old_id: required_id(table, Phase::Game, Serverbound, "player_action"),
                accept_teleportation_id: id(Serverbound, "accept_teleportation"),
                accept_teleportation_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "accept_teleportation",
                ),
                move_player_pos_rot_id: id(Serverbound, "move_player_pos_rot"),
                pending_accept: Mutex::new(None),
                wire_registries: RegistryTable::for_protocol(protocol).expect("wire registries"),
            }),
            v772: (protocol <= 772).then(|| Ids772 {
                add_entity_id: id(Clientbound, "add_entity"),
                set_entity_motion_id: id(Clientbound, "set_entity_motion"),
                player_rotation_id: id(Clientbound, "player_rotation"),
                set_default_spawn_id: id(Clientbound, "set_default_spawn_position"),
                explode_id: id(Clientbound, "explode"),
            }),
            v770: (protocol <= 770).then(|| Ids770 {
                player_command_id: id(Serverbound, "player_command"),
                player_command_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "player_command",
                ),
            }),
            v769: (protocol <= 769).then(|| Ids769 {
                player_chat_id: id(Clientbound, "player_chat"),
                update_advancements_id: id(Clientbound, "update_advancements"),
                chat_id: id(Serverbound, "chat"),
                chat_old_id: required_id(table, Phase::Game, Serverbound, "chat"),
                chat_command_signed_id: id(Serverbound, "chat_command_signed"),
                chat_command_signed_old_id: table.id(
                    Phase::Game,
                    Serverbound,
                    "chat_command_signed",
                ),
                container_click_id: id(Serverbound, "container_click"),
                container_click_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "container_click",
                ),
            }),
            v768: (protocol <= 768).then(|| Ids768 {
                level_particles_id: id(Clientbound, "level_particles"),
            }),
            v767: (protocol <= 767).then(|| Ids767 {
                player_position_id: id(Clientbound, "player_position"),
                teleport_entity_id: id(Clientbound, "teleport_entity"),
                entity_position_sync_id: id(Clientbound, "entity_position_sync"),
                respawn_id: id(Clientbound, "respawn"),
                container_set_slot_id: id(Clientbound, "container_set_slot"),
                cooldown_id: id(Clientbound, "cooldown"),
                set_cursor_item_id: id(Clientbound, "set_cursor_item"),
                set_player_inventory_id: id(Clientbound, "set_player_inventory"),
                drops: [
                    id(Clientbound, "explode"),
                    id(Clientbound, "update_recipes"),
                    id(Clientbound, "place_ghost_recipe"),
                ],
                player_input_id: id(Serverbound, "player_input"),
                player_input_old_id: required_id(table, Phase::Game, Serverbound, "player_input"),
                move_player_ids: [
                    "move_player_pos",
                    "move_player_pos_rot",
                    "move_player_rot",
                    "move_player_status_only",
                ]
                .map(|n| id(Serverbound, n)),
            }),
            v766: (protocol <= 766).then(|| Ids766 {
                projectile_power_id: id(Clientbound, "projectile_power"),
                container_set_content_id: id(Clientbound, "container_set_content"),
                set_equipment_id: id(Clientbound, "set_equipment"),
                creative_slot_id: id(Serverbound, "set_creative_mode_slot"),
                creative_slot_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "set_creative_mode_slot",
                ),
                use_item_id: id(Serverbound, "use_item"),
                use_item_old_id: required_id(table, Phase::Game, Serverbound, "use_item"),
            }),
            v765: (protocol <= 765).then(|| Ids765 {
                container_set_content_id: id(Clientbound, "container_set_content"),
                set_equipment_id: id(Clientbound, "set_equipment"),
                merchant_offers_id: id(Clientbound, "merchant_offers"),
                update_mob_effect_id: id(Clientbound, "update_mob_effect"),
                level_particles_id: id(Clientbound, "level_particles"),
                container_set_slot_id: id(Clientbound, "container_set_slot"),
                respawn_id: id(Clientbound, "respawn"),
                update_advancements_id: id(Clientbound, "update_advancements"),
                player_chat_id: id(Clientbound, "player_chat"),
                disguised_chat_id: id(Clientbound, "disguised_chat"),
                registry: RegistryTable::for_protocol(protocol).expect("embedded registry table"),
                container_click_id: id(Serverbound, "container_click"),
                container_click_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "container_click",
                ),
                creative_slot_id: id(Serverbound, "set_creative_mode_slot"),
                creative_slot_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "set_creative_mode_slot",
                ),
                chat_command_id: id(Serverbound, "chat_command"),
                chat_command_signed_id: id(Serverbound, "chat_command_signed"),
                chat_command_old_id: required_id(table, Phase::Game, Serverbound, "chat_command"),
            }),
            v764: (protocol <= 764).then(|| Ids764 {
                system_chat_id: id(Clientbound, "system_chat"),
                set_action_bar_id: id(Clientbound, "set_action_bar_text"),
                set_title_id: id(Clientbound, "set_title_text"),
                set_subtitle_id: id(Clientbound, "set_subtitle_text"),
                tab_list_id: id(Clientbound, "tab_list"),
                disconnect_id: id(Clientbound, "disconnect"),
                open_screen_id: id(Clientbound, "open_screen"),
                combat_kill_id: id(Clientbound, "player_combat_kill"),
                boss_event_id: id(Clientbound, "boss_event"),
                set_player_team_id: id(Clientbound, "set_player_team"),
                player_chat_id: id(Clientbound, "player_chat"),
                disguised_chat_id: id(Clientbound, "disguised_chat"),
                player_info_id: id(Clientbound, "player_info_update"),
                command_suggestions_id: id(Clientbound, "command_suggestions"),
                set_objective_id: id(Clientbound, "set_objective"),
                set_score_id: id(Clientbound, "set_score"),
                reset_score_id: id(Clientbound, "reset_score"),
                resource_pack_push_id: id(Clientbound, "resource_pack_push"),
                drops: [
                    id(Clientbound, "server_data"),
                    id(Clientbound, "map_item_data"),
                ],
                resource_pack_response_id: id(Serverbound, "resource_pack"),
                resource_pack_response_old_id: required_id(
                    table,
                    Phase::Game,
                    Serverbound,
                    "resource_pack",
                ),
            }),
            v763: (protocol <= 763).then(|| Ids763 {
                respawn_id: id(Clientbound, "respawn"),
                forget_level_chunk_id: id(Clientbound, "forget_level_chunk"),
                block_entity_data_id: id(Clientbound, "block_entity_data"),
                tag_query_id: id(Clientbound, "tag_query"),
                add_player_old_id: required_id(table, Phase::Game, Clientbound, "add_player"),
                add_entity_old_id: required_id(table, Phase::Game, Clientbound, "add_entity"),
                player_entity_type: RegistryTable::for_protocol(protocol)
                    .and_then(|r| {
                        r.names(ClientRegistry::EntityType)
                            .iter()
                            .position(|n| n == "player")
                    })
                    .expect("player entity type") as u32,
            }),
            quiet_suppressed: [
                "client_tick_end",
                "player_loaded",
                "chunk_batch_received",
                "pick_item_from_block",
                "pick_item_from_entity",
                "bundle_item_selected",
            ]
            .iter()
            .filter(|n| table.id(Phase::Game, Serverbound, n).is_none())
            .map(|n| required_id(native, Phase::Game, Serverbound, n))
            .collect(),
        })
    }
}

/// The wire version's dimension-type names in synced-registry order,
/// captured while splitting registry data: the pre-1.20.5 spawn-info
/// rewrites turn a dimension-type key string into this index.
///
/// Global because the rewrites reach it through `FrameRewrite` fn pointers,
/// and `Translation` is leaked per protocol rather than per connection, so a
/// field there would scope it no better. One connection at a time is an
/// assumption of this module.
///
/// TODO: a reconfiguration whose registry data omits dimension_type keeps the
/// previous list (as `config_sequence` keeps the previous `RegistryHolder`).
static DIMENSION_TYPES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Consumes the empty name an NBT root carries when `named` (before 1.20.2,
/// `FriendlyByteBuf.writeNbt` went through `NbtIo.write`, which emits the tag
/// byte, an empty UTF name and then the payload; 1.20.2 switched to
/// `writeAnyTag`, which drops the name). `TAG_End` (an absent value) carries no
/// name on either side.
fn skip_root_name(cur: &mut Cursor<&[u8]>, tag: u8, named: bool) -> Option<()> {
    if tag == 0 || !named {
        return Some(());
    }
    let len = read_u16(cur)?;
    advance(cur, len as usize)
}

/// Skips one NBT value whose root may be named; see [`skip_root_name`].
fn skip_nbt_root(cur: &mut Cursor<&[u8]>, named: bool) -> Option<()> {
    let tag = read_u8(cur)?;
    skip_root_name(cur, tag, named)?;
    skip_nbt_payload(cur, tag, 0)
}

/// Splits 765's single-NBT registry_data — a map of
/// `{registry: {type, value: [{name, id, element}]}}` — into the
/// per-registry packets 26.2 uses. Entries are ordered by their explicit
/// ids (the client derives protocol ids from wire order, which is
/// load-bearing for biome colors and dimension types).
fn split_registry_data(id: u32, payload: &[u8]) -> Option<Vec<Box<[u8]>>> {
    use simdnbt::owned::NbtTag;

    let mut cur = Cursor::new(payload);
    let NbtTag::Compound(root) = NbtTag::azalea_read(&mut cur).ok()? else {
        return None;
    };

    let mut frames = Vec::new();
    for (registry, value) in root.iter() {
        let registry = registry.to_str();
        let entries = value.compound()?.list("value")?.compounds()?;
        let mut ordered: Vec<(i32, String, &simdnbt::owned::NbtCompound)> = entries
            .iter()
            .map(|e| {
                Some((
                    e.int("id")?,
                    e.string("name")?.to_string(),
                    e.compound("element")?,
                ))
            })
            .collect::<Option<_>>()?;
        ordered.sort_unstable_by_key(|&(entry_id, ..)| entry_id);

        if registry.ends_with("dimension_type") {
            *DIMENSION_TYPES.lock().unwrap() =
                ordered.iter().map(|(_, name, _)| name.clone()).collect();
        }

        let mut out = Vec::new();
        wire::write_varint(&mut out, id);
        wire::write_varint(&mut out, registry.len() as u32);
        out.extend_from_slice(registry.as_bytes());
        wire::write_varint(&mut out, ordered.len() as u32);
        for (_, name, element) in ordered {
            wire::write_varint(&mut out, name.len() as u32);
            out.extend_from_slice(name.as_bytes());
            out.push(1);
            element.azalea_write(&mut out).ok()?;
        }
        frames.push(out.into_boxed_slice());
    }
    Some(frames)
}

/// The synced-registry index for a dimension-type key, for the pre-1.20.5
/// spawn-info rewrites.
fn dimension_type_index(name: &str) -> Option<u32> {
    DIMENSION_TYPES
        .lock()
        .unwrap()
        .iter()
        .position(|n| n == name)
        .map(|i| i as u32)
}

/// Name-matched id table from one version's phase/direction to another's,
/// with the `RENAMED` aliases; wire id == index, `None` = no equivalent.
fn id_map(
    from: &PacketTable,
    to: &PacketTable,
    phase: Phase,
    dir: Direction,
) -> Box<[Option<u32>]> {
    (0..)
        .map_while(|i| from.name_of(phase, dir, i))
        .map(|name| {
            to.id(phase, dir, name).or_else(|| {
                let alias = RENAMED.iter().find_map(|&(a, b)| {
                    if name == a {
                        Some(b)
                    } else if name == b {
                        Some(a)
                    } else {
                        None
                    }
                })?;
                to.id(phase, dir, alias)
            })
        })
        .collect()
}

impl ConfigIds {
    /// Name-matched configuration id tables; `None` when every id maps to
    /// itself or nothing (766 up: later versions only appended).
    fn build(protocol: i32, table: &PacketTable, native: &PacketTable) -> Option<ConfigIds> {
        use Direction::{Clientbound, Serverbound};
        let inbound = id_map(table, native, Phase::Configuration, Clientbound);
        let outbound = id_map(native, table, Phase::Configuration, Serverbound);
        if identity_maps(&inbound, &outbound) {
            return None;
        }
        let id = |dir, name| required_id(native, Phase::Configuration, dir, name);
        Some(ConfigIds {
            inbound,
            outbound,
            split_registry_data: (protocol <= 765).then(|| id(Clientbound, "registry_data")),
            v764: (protocol <= 764).then(|| ConfigIds764 {
                disconnect_id: id(Clientbound, "disconnect"),
                resource_pack_push_id: id(Clientbound, "resource_pack_push"),
                resource_pack_response_id: id(Serverbound, "resource_pack"),
            }),
        })
    }
}

/// Whether translation-by-id would be a no-op: every inbound id maps to
/// itself and every outbound id maps to itself or to nothing.
fn identity_maps(inbound: &[Option<u32>], outbound: &[Option<u32>]) -> bool {
    inbound
        .iter()
        .enumerate()
        .all(|(i, v)| *v == Some(i as u32))
        && outbound
            .iter()
            .enumerate()
            .all(|(i, v)| v.is_none() || *v == Some(i as u32))
}

/// A native-layout config frame with only its id rewritten.
fn plain_config_frame(id: u32, payload: &[u8]) -> Vec<Box<[u8]>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(payload);
    vec![out.into_boxed_slice()]
}

/// A packet id that must exist in the given table.
fn required_id(table: &PacketTable, phase: Phase, dir: Direction, name: &str) -> u32 {
    table
        .id(phase, dir, name)
        .unwrap_or_else(|| panic!("{name} missing from {phase:?} packet table"))
}

/// `ServerboundInteractPacket` action ordinals on versions where attacking
/// is an `interact` action (`INTERACT` carries a hand, `ATTACK` nothing,
/// `INTERACT_AT` a hit location then a hand).
const ACTION_INTERACT: u32 = 0;
const ACTION_ATTACK: u32 = 1;
const ACTION_INTERACT_AT: u32 = 2;

/// The shared `id, entityId, action` prefix of an old-layout `interact`
/// frame.
fn interact_frame(interact_old_id: u32, entity_id: u32, action: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    wire::write_varint(&mut out, interact_old_id);
    wire::write_varint(&mut out, entity_id);
    wire::write_varint(&mut out, action);
    out
}

/// Rewrites a native `player_command` payload (`entityId, action, data`
/// varints) for pre-1.21.6 wires, where PRESS/RELEASE_SHIFT_KEY still head
/// the action enum: every newer ordinal shifts up two.
fn translate_player_command(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let parse = || {
        let mut pos = 0;
        let entity_id = wire::read_varint(payload, &mut pos)?;
        let action = wire::read_varint(payload, &mut pos)?;
        Some((entity_id, action, pos))
    };
    let Some((entity_id, action, data_at)) = parse() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(payload.len() + 3);
    wire::write_varint(&mut out, old_id);
    wire::write_varint(&mut out, entity_id);
    wire::write_varint(&mut out, action + 2);
    out.extend_from_slice(&payload[data_at..]);
    vec![out]
}

/// Rewrites `level_particles`: 1.21.4 inserted the `alwaysShow` bool after
/// `overrideLimiter`; false matches the older client's behavior.
fn translate_level_particles_768(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let first = *payload.first()?;
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.push(first);
    out.push(0);
    out.extend_from_slice(&payload[1..]);
    Some(out)
}

/// Rewrites `player_position` from the 1.21.1 layout (`x/y/z, yRot, xRot,
/// u8 relative bits, teleport id`) to 26.2's (`id first, then
/// PositionMoveRotation with a zero delta, i32 relative bits`); the five
/// old bits keep their positions, and each position bit is mirrored into its
/// `DELTA_*` bit so 1.21.1's "keep the current velocity on a relative axis"
/// survives 26.2's `calculateDelta`.
fn translate_player_position_767(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let pos = payload.get(..24)?;
    let y_rot = f32::from_be_bytes(payload.get(24..28)?.try_into().ok()?);
    let x_rot = f32::from_be_bytes(payload.get(28..32)?.try_into().ok()?);
    let relatives = i32::from(*payload.get(32)?);
    let mut p = 33;
    let teleport_id = wire::read_varint(payload, &mut p)?;
    // X/Y/Z (0-2) -> DELTA_X/Y/Z (5-7); ROTATE_DELTA (8) stays clear.
    let relatives = relatives | ((relatives & 0b111) << 5);

    let mut out = Vec::with_capacity(64);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, teleport_id);
    write_position_move_rotation(&mut out, pos, y_rot, x_rot);
    out.extend_from_slice(&relatives.to_be_bytes());
    Some(out)
}

/// Rewrites 1.21.1's `teleport_entity` (`id, x/y/z, packed-degree rotation
/// bytes, onGround`) into 26.2's `entity_position_sync` — the packet whose
/// handler, like 1.21.1's, syncs position and rotation without touching the
/// entity's velocity. 26.2's own `teleport_entity` resolves the delta through
/// `calculateAbsolute`, so the synthesized zero would zero the velocity on
/// every sync. Same layout minus the relative-bit set.
fn translate_teleport_entity_767(v: &Ids767, payload: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0;
    let entity = wire::read_varint(payload, &mut p)?;
    let pos = payload.get(p..p + 24)?;
    let y_rot = *payload.get(p + 24)? as i8;
    let x_rot = *payload.get(p + 25)? as i8;
    let on_ground = *payload.get(p + 26)?;

    let mut out = Vec::with_capacity(72);
    wire::write_varint(&mut out, v.entity_position_sync_id);
    wire::write_varint(&mut out, entity);
    write_position_move_rotation(&mut out, pos, unpack_degrees(y_rot), unpack_degrees(x_rot));
    out.push(on_ground);
    Some(out)
}

/// Writes a 26.2 `PositionMoveRotation`: position, zero delta, rotation.
fn write_position_move_rotation(out: &mut Vec<u8>, pos: &[u8], y_rot: f32, x_rot: f32) {
    out.extend_from_slice(pos);
    out.extend_from_slice(&[0; 24]); // zero delta movement
    out.extend_from_slice(&y_rot.to_be_bytes());
    out.extend_from_slice(&x_rot.to_be_bytes());
}

/// Vanilla `Mth.unpackDegrees`: a packed rotation byte to float degrees.
fn unpack_degrees(b: i8) -> f32 {
    f32::from(b) * 360.0 / 256.0
}

/// Inserts the `seaLevel` varint 1.21.2 appended to CommonPlayerSpawnInfo,
/// which sits immediately before the final byte of both `login`
/// (enforcesSecureChat) and `respawn` (dataToKeep); vanilla's overworld
/// value is synthesized.
fn insert_sea_level(payload: &[u8]) -> Option<Vec<u8>> {
    let (last, body) = payload.split_last()?;
    let mut out = Vec::with_capacity(payload.len() + 1);
    out.extend_from_slice(body);
    wire::write_varint(&mut out, 63);
    out.push(*last);
    Some(out)
}

fn translate_respawn_767(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&insert_sea_level(payload)?);
    Some(out)
}

/// Rewrites 1.21.1's two-field `set_time` (`gameTime, dayTime` with a
/// negated dayTime marking a frozen clock, -1 for frozen zero) into the
/// triple the shared rewrite consumes.
fn translate_set_time_767(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let game_time = payload.get(..8)?;
    let day_time = i64::from_be_bytes(payload.get(8..16)?.try_into().ok()?);
    let (day_time, tick) = if day_time < 0 {
        (if day_time == -1 { 0 } else { -day_time }, 0)
    } else {
        (day_time, 1)
    };
    let mut tmp = Vec::with_capacity(17);
    tmp.extend_from_slice(game_time);
    tmp.extend_from_slice(&day_time.to_be_bytes());
    tmp.push(tick);
    translate_set_time(id, &tmp)
}

/// Rewrites `container_set_slot`: 1.21.2 widened the container id from a
/// signed byte to a varint and split its -1 (cursor) and -2 (player
/// inventory) sentinels into `set_cursor_item`/`set_player_inventory`,
/// which drop the state id.
fn translate_container_set_slot_767(v: &Ids767, payload: &[u8]) -> Option<Vec<u8>> {
    let container = *payload.first()? as i8;
    let mut p = 1;
    wire::read_varint(payload, &mut p)?; // state id
    let slot = i16::from_be_bytes(payload.get(p..p + 2)?.try_into().ok()?);
    let stack = payload.get(p + 2..)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    match container {
        -1 => {
            wire::write_varint(&mut out, v.set_cursor_item_id);
            out.extend_from_slice(stack);
        }
        -2 => {
            wire::write_varint(&mut out, v.set_player_inventory_id);
            wire::write_varint(&mut out, slot as u32);
            out.extend_from_slice(stack);
        }
        _ => {
            wire::write_varint(&mut out, v.container_set_slot_id);
            wire::write_varint(&mut out, container as u32);
            out.extend_from_slice(&payload[1..]);
        }
    }
    Some(out)
}

/// The 1.20.4 `container_set_slot`: 767's sentinel handling plus the
/// old-form item translation.
fn translate_container_set_slot_765(
    v: &Ids767,
    payload: &[u8],
    named_nbt: bool,
) -> Option<Vec<u8>> {
    let container = *payload.first()? as i8;
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 1)?;
    let state = varint_span(&mut cur)?;
    let slot_at = cur.position() as usize;
    let slot = i16::from_be_bytes(payload.get(slot_at..slot_at + 2)?.try_into().ok()?);
    advance(&mut cur, 2)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    match container {
        -1 => wire::write_varint(&mut out, v.set_cursor_item_id),
        -2 => {
            wire::write_varint(&mut out, v.set_player_inventory_id);
            wire::write_varint(&mut out, slot as u32);
        }
        _ => {
            wire::write_varint(&mut out, v.container_set_slot_id);
            wire::write_varint(&mut out, container as u32);
            out.extend_from_slice(&payload[state.start..slot_at + 2]);
        }
    }
    translate_item_765(&mut cur, &mut out, named_nbt)?;
    Some(out)
}

/// Rewrites `cooldown`'s item id into the native registry space. Vanilla
/// 26.2 names a cooldown group instead, but azalea still decodes the item
/// registry id and these frames feed azalea (like the team-color ordinal).
/// TODO: write the cooldown group once pomme owns the decoder (see the
/// azalea-divergence list).
fn translate_cooldown_767(remaps: &RegistryRemaps, id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0;
    let item = wire::read_varint(payload, &mut p)?;
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, remaps.remap(ClientRegistry::Item, item)?);
    out.extend_from_slice(&payload[p..]);
    Some(out)
}

/// Writes a 26.2 hashed or component stack as a bare 1.20.4 item
/// (`bool + item + i8 count + empty NBT`); components were already
/// stripped by `remap_outbound` (no 765 component registry exists), so
/// only the presence, item and count survive.
fn write_old_item(out: &mut Vec<u8>, stack: Option<(u32, u32)>) {
    match stack {
        Some((item, count)) => {
            out.push(1);
            wire::write_varint(out, item);
            out.push(count.min(127) as u8);
            out.push(0); // no NBT
        }
        None => out.push(0),
    }
}

/// The 1.20.4 `container_click`: hashed stacks become bare old-form items
/// (the shared head copies; byte and varint container ids agree for
/// vanilla's small ids).
fn translate_container_click_765(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        varint_span(&mut cur)?; // container id
        varint_span(&mut cur)?; // state id
        advance(&mut cur, 3)?; // slot, button
        varint_span(&mut cur)?; // click type

        let mut out = Vec::with_capacity(payload.len() + 8);
        wire::write_varint(&mut out, old_id);
        out.extend_from_slice(&payload[..cur.position() as usize]);

        let changed = u32::azalea_read_var(&mut cur).ok()?;
        wire::write_varint(&mut out, changed);
        for _ in 0..changed {
            let slot_at = cur.position() as usize;
            advance(&mut cur, 2)?;
            out.extend_from_slice(&payload[slot_at..cur.position() as usize]);
            write_old_item(&mut out, read_hashed_stack(&mut cur)?);
        }
        write_old_item(&mut out, read_hashed_stack(&mut cur)?);
        Some(out)
    };
    match rewrite() {
        Some(out) => vec![out],
        None => Vec::new(),
    }
}

/// The 1.20.4 `set_creative_mode_slot`: the component stack (whose patch
/// `remap_outbound` already cleared) becomes a bare old-form item.
fn translate_creative_slot_765(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        advance(&mut cur, 2)?; // slot short
        let count = u32::azalea_read_var(&mut cur).ok()?;

        let mut out = Vec::with_capacity(payload.len() + 4);
        wire::write_varint(&mut out, old_id);
        out.extend_from_slice(&payload[..2]);
        if count == 0 {
            out.push(0);
            return Some(out);
        }
        let item = u32::azalea_read_var(&mut cur).ok()?;
        write_old_item(&mut out, Some((item, count)));
        Some(out)
    };
    match rewrite() {
        Some(out) => vec![out],
        None => Vec::new(),
    }
}

/// Serverbound `chat` or `chat_command_signed` for 1.21.4 and older (1.20.4's
/// `chat_command` has the signed layout): drops the trailing last-seen
/// checksum byte 1.21.5 added.
fn strip_last_seen_checksum(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let Some((&_checksum, body)) = payload.split_last() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(body.len() + 5);
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(body);
    vec![out]
}

/// The 1.20.4 `chat_command`, which is always the signed form: empty
/// timestamp, salt, signatures and last-seen update are appended (offline
/// servers accept unsigned commands).
fn translate_chat_command_765(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 24);
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(payload);
    out.extend_from_slice(&[0; 16]); // timestamp, salt
    wire::write_varint(&mut out, 0); // no argument signatures
    wire::write_varint(&mut out, 0); // last-seen offset
    out.extend_from_slice(&[0; 3]); // last-seen acknowledged bit set
    vec![out]
}

/// Rewrites serverbound `use_item` for 1.20.6, dropping the rotation
/// floats 1.21 appended.
fn translate_use_item(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(&payload[..payload.len().saturating_sub(8)]);
    vec![out]
}

/// Rewrites `update_attributes`' attribute ids into the native registry
/// space. Each snapshot names its attribute by registry id
/// (`Attribute.STREAM_CODEC` is a plain `holderRegistry` varint), and those
/// ids shift between versions — 1.21.2 also dropped every category prefix, so
/// `generic.max_health` and `max_health` are the same entry at different
/// indices. Without the remap the client reads a different attribute
/// entirely. Modifier bodies are already the native layout on every version
/// reaching here, so they copy verbatim.
fn translate_update_attributes(
    remaps: &RegistryRemaps,
    id: u32,
    payload: &[u8],
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id
    let entries = u32::azalea_read_var(&mut cur).ok()?;

    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..entries {
        let attribute = u32::azalea_read_var(&mut cur).ok()?;
        wire::write_varint(
            &mut out,
            remaps.remap(ClientRegistry::Attribute, attribute)?,
        );
        let body_at = cur.position() as usize;
        advance(&mut cur, 8)?; // base
        let modifiers = u32::azalea_read_var(&mut cur).ok()?;
        for _ in 0..modifiers {
            let len = u32::azalea_read_var(&mut cur).ok()? as usize;
            advance(&mut cur, len)?; // modifier id
            advance(&mut cur, 8)?; // amount
            varint_span(&mut cur)?; // operation
        }
        out.extend_from_slice(&payload[body_at..cur.position() as usize]);
    }
    Some(out)
}

/// Rewrites serverbound `player_input` for 1.21.1, where it was the vehicle
/// steering packet (`xxa, zza floats + jump/shift flags`) rather than the
/// key bitfield 1.21.2 introduced; axes are synthesized at full strength.
/// Unmounted frames need no suppression: the server's `setPlayerInput` only
/// applies them to a passenger.
fn translate_player_input(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let Some(&bits) = payload.first() else {
        return Vec::new();
    };
    let axis = |pos, neg| match (bits & pos != 0, bits & neg != 0) {
        (true, false) => 1.0f32,
        (false, true) => -1.0f32,
        _ => 0.0f32,
    };
    let mut out = Vec::with_capacity(12);
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(&axis(4, 8).to_be_bytes()); // left / right -> xxa
    out.extend_from_slice(&axis(1, 2).to_be_bytes()); // forward / backward -> zza
    out.push((bits >> 4) & 3); // jump, shift
    vec![out]
}

/// Reads one 1.20.4 optional item stack (`bool present + item id +
/// i8 count + NBT`). The legacy NBT is retained so mining can still inspect
/// enchantments after the 1.20.5 component boundary; `None` is a malformed
/// stack, `Some(None)` an absent one.
fn read_old_item(
    cur: &mut Cursor<&[u8]>,
    named_nbt: bool,
) -> Option<Option<(u32, u8, simdnbt::owned::Nbt)>> {
    if read_u8(cur)? == 0 {
        return Some(None);
    }
    let item = u32::azalea_read_var(cur).ok()?;
    let count = read_u8(cur)?;
    let nbt = if named_nbt {
        simdnbt::owned::read(cur).ok()?
    } else {
        simdnbt::owned::read_unnamed(cur).ok()?
    };
    Some(Some((item, count, nbt)))
}

/// Translates one pre-1.20.5 optional item stack into a 26.2 stack. Legacy
/// NBT is preserved as `minecraft:custom_data` rather than discarded; this
/// keeps enchantments available to Pomme's protocol-aware mining logic while
/// avoiding a broad NBT-to-components data-fixer in the protocol bridge.
fn translate_item_765(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, named_nbt: bool) -> Option<()> {
    let Some((item, count, nbt)) = read_old_item(cur, named_nbt)? else {
        wire::write_varint(out, 0);
        return Some(());
    };
    wire::write_varint(out, u32::from(count));
    wire::write_varint(out, item);
    if nbt.is_some() {
        wire::write_varint(out, 1); // one added component
        wire::write_varint(out, 0); // no removed components
        wire::write_varint(out, COMPONENT_CUSTOM_DATA);
        nbt.azalea_write(out).ok()?;
    } else {
        wire::write_varint(out, 0);
        wire::write_varint(out, 0);
    }
    Some(())
}

/// Rewrites `container_set_content`: the head (byte container id == varint
/// for vanilla's small ids, state id, count) copies; each item translates
/// bare.
fn translate_container_set_content_765(
    id: u32,
    payload: &[u8],
    named_nbt: bool,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 1)?; // container id
    varint_span(&mut cur)?; // state id
    let count = u32::azalea_read_var(&mut cur).ok()?;

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..=count {
        // The trailing iteration is the carried item.
        translate_item_765(&mut cur, &mut out, named_nbt)?;
    }
    Some(out)
}

/// Rewrites `set_equipment`'s slot/item pairs (the slot byte's high bit
/// continues the list).
fn translate_set_equipment_765(id: u32, payload: &[u8], named_nbt: bool) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    loop {
        let slot = read_u8(&mut cur)?;
        out.push(slot);
        translate_item_765(&mut cur, &mut out, named_nbt)?;
        if slot & 0x80 == 0 {
            return Some(out);
        }
    }
}

/// Rewrites `merchant_offers`: 1.20.5 turned the cost stacks into
/// `ItemCost` (item + count + component predicate, no NBT) with an explicit
/// optional second cost; the result stays a plain (bare) stack and the
/// per-offer numeric tail copies verbatim.
fn translate_merchant_offers_765(id: u32, payload: &[u8], named_nbt: bool) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // container id
    let offers = u32::azalea_read_var(&mut cur).ok()?;

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..offers {
        translate_item_cost_765(&mut cur, &mut out, false, named_nbt)?;
        // Result: non-optional stack, bare.
        translate_item_765(&mut cur, &mut out, named_nbt)?;
        translate_item_cost_765(&mut cur, &mut out, true, named_nbt)?;
        let tail_at = cur.position() as usize;
        advance(&mut cur, 25)?; // outOfStock, 4 ints, multiplier, demand
        out.extend_from_slice(&payload[tail_at..cur.position() as usize]);
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// One 765 cost stack to a 26.2 `ItemCost` (`optional` wraps it in the
/// presence bool the second cost gained); an empty stack becomes an absent
/// cost, which only vanilla's costB ever is.
fn translate_item_cost_765(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    optional: bool,
    named_nbt: bool,
) -> Option<()> {
    let Some((item, count, _nbt)) = read_old_item(cur, named_nbt)? else {
        // Only the optional second cost can be absent; a missing base cost
        // has no `ItemCost` form, so the frame is unrepresentable.
        if !optional {
            return None;
        }
        out.push(0);
        return Some(());
    };
    if optional {
        out.push(1);
    }
    wire::write_varint(out, item);
    wire::write_varint(out, u32::from(count));
    wire::write_varint(out, 0); // empty component predicate
    Some(())
}

/// Rewrites `update_mob_effect`: 1.20.5 widened the amplifier from a byte
/// to a varint and dropped the trailing factor-data NBT. Vanilla reads that
/// byte signed, so an amplifier past 127 wraps negative on both sides; the
/// varint carries the sign through rather than clamping it.
fn translate_update_mob_effect_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let head = varint_span(&mut cur)?; // entity id
    let effect = varint_span(&mut cur)?;
    let amplifier = read_u8(&mut cur)? as i8;
    let duration = varint_span(&mut cur)?;
    let flags = read_u8(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[head.start..effect.end]);
    wire::write_varint(&mut out, amplifier as i32 as u32);
    out.extend_from_slice(&payload[duration]);
    out.push(flags);
    Some(out)
}

/// Rewrites `update_attributes` for the pre-1.21 layouts: the attribute id
/// is remapped into the native registry space and each modifier's UUID id
/// becomes a hex resource location. 1.20.4 keys the attribute by resource
/// location rather than registry id, so it passes its own table as
/// `key_table` to resolve the name first.
fn translate_update_attributes_uuid(
    key_table: Option<&RegistryTable>,
    remaps: &RegistryRemaps,
    id: u32,
    payload: &[u8],
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id
    let entries = u32::azalea_read_var(&mut cur).ok()?;

    let mut out = Vec::with_capacity(payload.len() + 64);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..entries {
        let attribute = match key_table {
            Some(table) => {
                let len = u32::azalea_read_var(&mut cur).ok()? as usize;
                let key_at = cur.position() as usize;
                advance(&mut cur, len)?;
                let key = std::str::from_utf8(&payload[key_at..key_at + len]).ok()?;
                let key = key.strip_prefix("minecraft:").unwrap_or(key);
                table
                    .names(ClientRegistry::Attribute)
                    .iter()
                    .position(|n| n == key)? as u32
            }
            None => u32::azalea_read_var(&mut cur).ok()?,
        };
        wire::write_varint(
            &mut out,
            remaps.remap(ClientRegistry::Attribute, attribute)?,
        );

        let body_at = cur.position() as usize;
        advance(&mut cur, 8)?; // base
        let modifiers = u32::azalea_read_var(&mut cur).ok()?;
        out.extend_from_slice(&payload[body_at..cur.position() as usize]);
        for _ in 0..modifiers {
            translate_modifier_uuid(&mut cur, &mut out, payload)?;
        }
    }
    Some(out)
}

/// One pre-1.21 attribute modifier: the 16-byte UUID id becomes a hex
/// resource location; the amount and operation (a 0-2 byte at 765, so its
/// own varint encoding) copy verbatim.
fn translate_modifier_uuid(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    payload: &[u8],
) -> Option<()> {
    let uuid_at = cur.position() as usize;
    advance(cur, 16)?;
    let name: String = payload[uuid_at..uuid_at + 16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let rl = format!("minecraft:{name}");
    wire::write_varint(out, rl.len() as u32);
    out.extend_from_slice(rl.as_bytes());
    let tail_at = cur.position() as usize;
    advance(cur, 8)?; // amount
    varint_span(cur)?; // operation
    out.extend_from_slice(&payload[tail_at..cur.position() as usize]);
    Some(())
}

/// Rewrites `level_particles` for 1.20.4, where the particle type id led
/// the packet; it moves to just before the payload, and the `alwaysShow`
/// bool later versions gained is synthesized.
fn translate_level_particles_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let particle = varint_span(&mut cur)?;
    let limiter_at = cur.position() as usize;
    advance(&mut cur, 1 + 24 + 16 + 4)?; // limiter, pos, dists, speed, count

    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.push(payload[limiter_at]);
    out.push(0); // alwaysShow
    out.extend_from_slice(&payload[limiter_at + 1..cur.position() as usize]);
    out.extend_from_slice(&payload[particle]);
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Writes the direct chat-type registry id as the holder 1.20.5 replaced it
/// with (id + 1, 0 being reserved for an inline value), then the rest of the
/// frame. The id is synced-registry order either way, so it needs no remap.
fn chat_type_holder(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, payload: &[u8]) -> Option<()> {
    let chat_type = u32::azalea_read_var(cur).ok()?;
    wire::write_varint(out, chat_type + 1);
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(())
}

/// Rewrites `player_chat` for 1.20.4, whose trailing `ChatType.Bound` needs
/// the holder bump; the 1.21.5 globalIndex prepend folds in, since this arm
/// shadows 769's.
fn translate_player_chat_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, 0); // globalIndex, 1.21.5+
    player_chat_head(&mut cur, &mut out, copy_nbt)?;
    chat_type_holder(&mut cur, &mut out, payload).map(|()| out)
}

/// Rewrites `disguised_chat` for 1.20.4: the same chat-type holder bump
/// after the message component.
fn translate_disguised_chat_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    skip_nbt(&mut cur)?; // message

    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    chat_type_holder(&mut cur, &mut out, payload).map(|()| out)
}

/// Rewrites `respawn` for 1.20.4: the spawn info's dimension type is a
/// resource key string, turned into the synced-registry index captured
/// from registry data; the seaLevel insert then applies like 767's.
fn translate_respawn_765(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let converted = convert_spawn_info_dimension(payload, 0)?;
    let mut out = Vec::with_capacity(converted.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&insert_sea_level(&converted)?);
    Some(out)
}

/// Rewrites `respawn`: 1.20.2 moved `dataToKeep` from the middle of the spawn
/// info to the end of the packet. Chains into the 765 rewrite.
fn translate_respawn_763(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    skip_utf(&mut cur)?; // dimension type
    skip_utf(&mut cur)?; // dimension
    advance(&mut cur, 12)?; // seed, game types, isDebug, isFlat
    let at = cur.position() as usize;
    let data_to_keep = *payload.get(at)?;

    let mut moved = Vec::with_capacity(payload.len());
    moved.extend_from_slice(&payload[..at]);
    moved.extend_from_slice(payload.get(at + 1..)?);
    moved.push(data_to_keep);
    translate_respawn_765(id, &moved)
}

/// Rewrites `forget_level_chunk`: 1.20.2 packed the two coordinate ints into a
/// `ChunkPos` long, whose big-endian bytes put z ahead of x.
fn translate_forget_level_chunk_763(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(payload.get(4..8)?);
    out.extend_from_slice(payload.get(..4)?);
    Some(out)
}

/// Rewrites `block_entity_data`, whose tag is written straight through
/// `writeNbt` and so carries the pre-1.20.2 root name. The chunk's inline
/// block entities go through [`copy_block_entities`] instead.
fn translate_block_entity_data_763(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    copy_bytes(&mut cur, &mut out, 8)?; // block pos
    copy_varint(&mut cur, &mut out)?; // block entity type
    copy_unnamed_nbt(&mut cur, &mut out, true)?;
    Some(out)
}

/// Rewrites `tag_query`, the other packet writing a bare `writeNbt` tag.
/// Pomme never sends the queries that prompt one, but the layout is covered
/// rather than left as a trap.
fn translate_tag_query_763(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len());
    wire::write_varint(&mut out, id);
    copy_varint(&mut cur, &mut out)?; // transaction id
    copy_unnamed_nbt(&mut cur, &mut out, true)?;
    Some(out)
}

/// Rewrites 1.20.1's `add_player` onto `add_entity`, which 1.20.2 started
/// spawning players with. The head yaw repeats the body yaw, as
/// `ClientPacketListener.handleAddPlayer` did; the data field and velocity are
/// zero, neither being read for a player. Emits the wire version's own
/// `add_entity`, which the caller feeds back through the chain.
fn translate_add_player_763(ids: &Ids763, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id
    advance(&mut cur, 16)?; // uuid
    let head_end = cur.position() as usize;
    advance(&mut cur, 24)?; // position doubles
    let pos_end = cur.position() as usize;
    let y_rot = read_u8(&mut cur)?;
    let x_rot = read_u8(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, ids.add_entity_old_id);
    out.extend_from_slice(&payload[..head_end]);
    wire::write_varint(&mut out, ids.player_entity_type);
    out.extend_from_slice(&payload[head_end..pos_end]);
    out.extend_from_slice(&[x_rot, y_rot, y_rot]);
    wire::write_varint(&mut out, 0); // data
    out.extend_from_slice(&[0; 6]); // velocity shorts
    Some(out)
}

/// The two fixed spans a 1.20.1 `login` opens with: the player id and hardcore
/// bool, which 1.20.2 kept in place, then the game types it moved into the
/// spawn info.
const LOGIN_HEAD_763: usize = 5;
const LOGIN_PREFIX_763: usize = 7;

/// Advances past a 1.20.1 `login` payload's prefix and level list, leaving the
/// cursor on the registry codec.
fn seek_login_codec_763(cur: &mut Cursor<&[u8]>) -> Option<()> {
    advance(cur, LOGIN_PREFIX_763)?;
    let levels = u32::azalea_read_var(cur).ok()?;
    for _ in 0..levels {
        skip_utf(cur)?;
    }
    Some(())
}

/// Rewrites the 1.20.1 game `login` payload into the 765 form: the registry
/// codec rides inline here (read separately, before the game phase), and the
/// spawn-info fields hadn't been gathered into `CommonPlayerSpawnInfo` yet.
fn translate_game_login_763(payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    seek_login_codec_763(&mut cur)?;
    let levels_end = cur.position() as usize;
    skip_nbt_root(&mut cur, true)?; // the registry codec, read before the join
    let spawn_at = cur.position() as usize;
    skip_utf(&mut cur)?; // dimension type
    skip_utf(&mut cur)?; // dimension
    advance(&mut cur, 8)?; // seed
    // Also where the counts begin.
    let spawn_end = cur.position() as usize;
    varint_span(&mut cur)?; // max players
    varint_span(&mut cur)?; // chunk radius
    varint_span(&mut cur)?; // simulation distance
    advance(&mut cur, 2)?; // reducedDebugInfo, showDeathScreen
    let counts_end = cur.position() as usize;

    let mut out = Vec::with_capacity(payload.len());
    out.extend_from_slice(&payload[..LOGIN_HEAD_763]);
    out.extend_from_slice(&payload[LOGIN_PREFIX_763..levels_end]);
    out.extend_from_slice(&payload[spawn_end..counts_end]);
    out.push(0); // doLimitedCrafting, absent at 763
    out.extend_from_slice(&payload[spawn_at..spawn_end]);
    out.extend_from_slice(&payload[LOGIN_HEAD_763..LOGIN_PREFIX_763]);
    out.extend_from_slice(payload.get(counts_end..)?); // isDebug .. portalCooldown
    Some(out)
}

/// Rewrites the 1.20.4 game `login` payload up to the 766 form (dimension
/// key string -> registry index, trailing enforcesSecureChat synthesized);
/// the shared seaLevel/onlineMode chain runs after.
fn translate_game_login_765(payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 5)?; // player id, hardcore
    let levels = u32::azalea_read_var(&mut cur).ok()?;
    for _ in 0..levels {
        skip_utf(&mut cur)?;
    }
    varint_span(&mut cur)?; // max players
    varint_span(&mut cur)?; // chunk radius
    varint_span(&mut cur)?; // simulation distance
    advance(&mut cur, 3)?; // reducedDebug, showDeathScreen, doLimitedCrafting

    let mut converted = convert_spawn_info_dimension(payload, cur.position() as usize)?;
    converted.push(0); // enforcesSecureChat, absent at 765
    Some(converted)
}

/// Replaces the dimension-type resource key at `spawn_at` (the start of a
/// 765 CommonPlayerSpawnInfo) with its synced-registry index, copying
/// everything else verbatim.
fn convert_spawn_info_dimension(payload: &[u8], spawn_at: usize) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    cur.set_position(spawn_at as u64);
    let len = u32::azalea_read_var(&mut cur).ok()? as usize;
    let key_at = cur.position() as usize;
    advance(&mut cur, len)?;
    let key = std::str::from_utf8(&payload[key_at..key_at + len]).ok()?;
    let index = dimension_type_index(key).unwrap_or_else(|| {
        tracing::warn!("Unknown dimension type {key}; defaulting to 0");
        0
    });

    let mut out = Vec::with_capacity(payload.len());
    out.extend_from_slice(&payload[..spawn_at]);
    wire::write_varint(&mut out, index);
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// The native serializer id for a 1.20.4 `EntityDataSerializers` id:
/// 1.20.5 inserted `particles` (18), `wolf_variant` (23) and
/// `armadillo_state` (28), whose set then held through 1.21.4; mapping
/// through the 766-era ids covers the rest.
fn remap_serializer_765(old: u32) -> Option<u32> {
    match old {
        0..=17 => remap_serializer_769(old),
        18..=21 => remap_serializer_769(old + 1),
        22 => remap_serializer_769(24),
        23..=25 => remap_serializer_769(old + 2),
        26..=27 => remap_serializer_769(old + 3),
        _ => None,
    }
}

/// Copies `n` bytes from the cursor's payload to `out`.
fn copy_bytes(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, n: usize) -> Option<()> {
    let at = cur.position() as usize;
    advance(cur, n)?;
    out.extend_from_slice(&cur.get_ref()[at..at + n]);
    Some(())
}

fn copy_varint(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<u32> {
    let at = cur.position() as usize;
    let v = u32::azalea_read_var(cur).ok()?;
    out.extend_from_slice(&cur.get_ref()[at..cur.position() as usize]);
    Some(v)
}

fn copy_utf(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let len = copy_varint(cur, out)?;
    copy_bytes(cur, out, len as usize)
}

/// Copies a nullable field, running `inner` on a present value.
fn copy_optional(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    inner: impl FnOnce(&mut Cursor<&[u8]>, &mut Vec<u8>) -> Option<()>,
) -> Option<()> {
    let present = read_u8(cur)?;
    out.push(present);
    if present != 0 {
        inner(cur, out)
    } else {
        Some(())
    }
}

/// Reads one pre-1.20.3 length-prefixed JSON component and writes it as the
/// network-NBT form 1.20.3 introduced.
fn transcode_component(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let len = u32::azalea_read_var(cur).ok()? as usize;
    let at = cur.position() as usize;
    advance(cur, len)?;
    let json = std::str::from_utf8(&cur.get_ref()[at..at + len]).ok()?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    json_to_nbt(&value).azalea_write(out).ok()
}

/// A JSON chat-component value as an owned NBT tag: the codec shape is
/// identical (vanilla runs the same component codec through JsonOps and
/// NbtOps), except NBT lists are homogeneous — mixed arrays normalize to
/// compound lists with primitives wrapped as `{text}`.
fn json_to_nbt(value: &serde_json::Value) -> simdnbt::owned::NbtTag {
    use simdnbt::owned::{NbtList, NbtTag};
    match value {
        serde_json::Value::Null => NbtTag::String("".into()),
        serde_json::Value::Bool(b) => NbtTag::Byte(*b as i8),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => match i32::try_from(i) {
                Ok(i) => NbtTag::Int(i),
                Err(_) => NbtTag::Long(i),
            },
            None => NbtTag::Double(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => NbtTag::String(s.as_str().into()),
        serde_json::Value::Array(items) => NbtTag::List(NbtList::Compound(
            items.iter().map(json_to_compound).collect(),
        )),
        serde_json::Value::Object(_) => NbtTag::Compound(json_to_compound(value)),
    }
}

fn json_to_compound(value: &serde_json::Value) -> simdnbt::owned::NbtCompound {
    let mut compound = simdnbt::owned::NbtCompound::new();
    match value {
        serde_json::Value::Object(map) => {
            for (key, entry) in map {
                compound.insert(key.as_str(), json_to_nbt(entry));
            }
        }
        // A primitive inside a component list renders as its text form.
        other => {
            let text = match other {
                serde_json::Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            compound.insert("text", simdnbt::owned::NbtTag::String(text.as_str().into()));
        }
    }
    compound
}

/// Copies `varints` leading varints, then transcodes one trailing component.
fn copy_then_transcode(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, varints: u32) -> Option<()> {
    for _ in 0..varints {
        copy_varint(cur, out)?;
    }
    transcode_component(cur, out)
}

/// `boss_event`'s name sits in the Add (0) and UpdateName (3) op bodies
/// (`ClientboundBossEventPacket`, order unchanged since 1.20.2).
fn transcode_boss_event(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    copy_bytes(cur, out, 16)?; // boss bar uuid
    let op = copy_varint(cur, out)?;
    if matches!(op, 0 | 3) {
        transcode_component(cur, out)?;
    }
    Some(())
}

/// The 764 team `Parameters` (display, options, visibility, collision,
/// color, prefix, suffix) precede the player list; the shared
/// `translate_team` reorder runs on the converted payload afterwards.
fn transcode_team(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    copy_utf(cur, out)?; // team name
    let method = read_u8(cur)?;
    out.push(method);
    if matches!(method, 0 | 2) {
        transcode_component(cur, out)?; // display name
        copy_bytes(cur, out, 1)?; // options
        copy_utf(cur, out)?; // nametag visibility
        copy_utf(cur, out)?; // collision rule
        copy_varint(cur, out)?; // color
        transcode_component(cur, out)?; // prefix
        transcode_component(cur, out)?; // suffix
    }
    Some(())
}

/// The shared `player_chat` walk up to the trailing chat type; `component`
/// handles the nullable unsignedContent (a verbatim copy at 765, the
/// JSON -> NBT transcode at 764).
fn player_chat_head(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    component: impl FnOnce(&mut Cursor<&[u8]>, &mut Vec<u8>) -> Option<()>,
) -> Option<()> {
    copy_bytes(cur, out, 16)?; // sender uuid
    copy_varint(cur, out)?; // index
    copy_optional(cur, out, |c, o| copy_bytes(c, o, 256))?; // signature
    copy_utf(cur, out)?; // content
    copy_bytes(cur, out, 16)?; // timestamp, salt
    let last_seen = copy_varint(cur, out)?;
    for _ in 0..last_seen {
        // A zero id carries a full signature instead of a cache reference.
        if copy_varint(cur, out)? == 0 {
            copy_bytes(cur, out, 256)?;
        }
    }
    copy_optional(cur, out, component)?; // unsigned content
    let filter = copy_varint(cur, out)?;
    if filter == 2 {
        let longs = copy_varint(cur, out)?; // partially-filtered bit set
        copy_bytes(cur, out, longs as usize * 8)?;
    }
    Some(())
}

/// A component field copied verbatim (already network NBT at 765).
fn copy_nbt(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let span = nbt_span(cur)?;
    out.extend_from_slice(&cur.get_ref()[span]);
    Some(())
}

/// The `player_chat` walk through its three component sites (nullable
/// unsignedContent, then the chat-type name/target pair).
fn transcode_player_chat(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    player_chat_head(cur, out, transcode_component)?;
    transcode_chat_type(cur, out)
}

/// `ChatType.BoundNetwork`: chat-type id, name, nullable target name.
fn transcode_chat_type(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    copy_varint(cur, out)?;
    transcode_component(cur, out)?;
    copy_optional(cur, out, transcode_component)
}

/// `player_info_update`: per entry, per action bit, the display name
/// (bit 5, last) is the one component; every earlier action's payload
/// copies (order per the 1.20.2 `ClientboundPlayerInfoUpdatePacket`, whose
/// first six actions match azalea's).
fn transcode_player_info(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let actions = read_u8(cur)?;
    out.push(actions);
    let entries = copy_varint(cur, out)?;
    for _ in 0..entries {
        copy_bytes(cur, out, 16)?; // uuid
        if actions & 0x01 != 0 {
            copy_utf(cur, out)?; // name
            let properties = copy_varint(cur, out)?;
            for _ in 0..properties {
                copy_utf(cur, out)?;
                copy_utf(cur, out)?;
                copy_optional(cur, out, copy_utf)?; // signature
            }
        }
        if actions & 0x02 != 0 {
            // initialize chat: session uuid, expiry, public key, signature
            copy_optional(cur, out, |c, o| {
                copy_bytes(c, o, 24)?;
                let key = copy_varint(c, o)?;
                copy_bytes(c, o, key as usize)?;
                let sig = copy_varint(c, o)?;
                copy_bytes(c, o, sig as usize)
            })?;
        }
        if actions & 0x04 != 0 {
            copy_varint(cur, out)?; // game mode
        }
        if actions & 0x08 != 0 {
            copy_bytes(cur, out, 1)?; // listed
        }
        if actions & 0x10 != 0 {
            copy_varint(cur, out)?; // latency
        }
        if actions & 0x20 != 0 {
            copy_optional(cur, out, transcode_component)?; // display name
        }
    }
    Some(())
}

/// `command_suggestions`: nullable tooltip component per suggestion.
fn transcode_suggestions(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    for _ in 0..3 {
        copy_varint(cur, out)?; // id, range start, range length
    }
    let suggestions = copy_varint(cur, out)?;
    for _ in 0..suggestions {
        copy_utf(cur, out)?;
        copy_optional(cur, out, transcode_component)?;
    }
    Some(())
}

/// Rewrites the 1.20.2 `set_score`: the method byte is gone (a REMOVE
/// becomes the `reset_score` packet 1.20.3 added) and the trailing nullable
/// display/numberFormat pair 1.20.3 added is synthesized absent.
fn translate_set_score_764(v: &Ids764, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut owner = Vec::new();
    copy_utf(&mut cur, &mut owner)?;
    let method = u32::azalea_read_var(&mut cur).ok()?;
    let mut objective = Vec::new();
    copy_utf(&mut cur, &mut objective)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    if method == 1 {
        // An empty objective resets the owner's scores in every objective.
        wire::write_varint(&mut out, v.reset_score_id);
        out.extend_from_slice(&owner);
        if objective == [0] {
            out.push(0);
        } else {
            out.push(1);
            out.extend_from_slice(&objective);
        }
        return Some(out);
    }
    wire::write_varint(&mut out, v.set_score_id);
    out.extend_from_slice(&owner);
    out.extend_from_slice(&objective);
    copy_varint(&mut cur, &mut out)?; // score
    out.extend_from_slice(&[0, 0]); // no display, no number format
    Some(out)
}

/// Rewrites the 1.20.2 `set_objective`, which ends at the render type; the
/// nullable numberFormat 1.20.3 appended is synthesized absent and the
/// display name transcodes.
fn translate_set_objective_764(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    copy_utf(&mut cur, &mut out)?; // objective name
    let method = read_u8(&mut cur)?;
    out.push(method);
    if matches!(method, 0 | 2) {
        transcode_component(&mut cur, &mut out)?;
        copy_varint(&mut cur, &mut out)?; // render type
        // Vanilla 26.2 wraps the numberFormat optional; azalea (ffedf17)
        // reads a bare format kind. Zero decodes as absent/blank either way.
        out.push(0);
    }
    Some(out)
}

/// Rewrites the unsplit 1.20.2 `resource_pack` into `resource_pack_push`:
/// a zero pack UUID is synthesized (the serverbound reply strips it) and
/// the nullable prompt transcodes. Shared by the game and config phases.
fn translate_resource_pack_764(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 18);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&[0; 16]); // synthesized pack uuid
    copy_utf(&mut cur, &mut out)?; // url
    copy_utf(&mut cur, &mut out)?; // hash
    copy_bytes(&mut cur, &mut out, 1)?; // required
    copy_optional(&mut cur, &mut out, transcode_component)?; // prompt
    Some(out)
}

/// Rewrites the serverbound `resource_pack` reply for 1.20.2: the pack
/// UUID 1.20.3 prepended is stripped and the post-1.20.2 action values
/// clamp to the original four (downloaded -> accepted, failures -> failed
/// download, discarded -> declined).
fn translate_resource_pack_response_764(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        advance(&mut cur, 16)?; // pack uuid
        let action = u32::azalea_read_var(&mut cur).ok()?;
        let action = match action {
            0..=3 => action,
            4 => 3,
            5 | 6 => 2,
            _ => 1,
        };
        let mut out = Vec::with_capacity(4);
        wire::write_varint(&mut out, old_id);
        wire::write_varint(&mut out, action);
        Some(out)
    };
    match rewrite() {
        Some(out) => vec![out],
        None => Vec::new(),
    }
}
const COMPONENT_766_CUSTOM_DATA: u32 = 0;
const COMPONENT_766_MAX_STACK_SIZE: u32 = 1;
const COMPONENT_766_MAX_DAMAGE: u32 = 2;
const COMPONENT_766_DAMAGE: u32 = 3;
const COMPONENT_766_UNBREAKABLE: u32 = 4;
const COMPONENT_766_CUSTOM_NAME: u32 = 5;
const COMPONENT_766_ITEM_NAME: u32 = 6;
const COMPONENT_766_RARITY: u32 = 8;
const COMPONENT_766_ENCHANTMENTS: u32 = 9;
const COMPONENT_766_HIDE_ADDITIONAL_TOOLTIP: u32 = 14;
const COMPONENT_766_HIDE_TOOLTIP: u32 = 15;
const COMPONENT_766_REPAIR_COST: u32 = 16;
const COMPONENT_766_CREATIVE_SLOT_LOCK: u32 = 17;
const COMPONENT_766_GLINT_OVERRIDE: u32 = 18;
const COMPONENT_766_INTANGIBLE_PROJECTILE: u32 = 19;
const COMPONENT_766_FIRE_RESISTANT: u32 = 21;
const COMPONENT_766_TOOL: u32 = 22;

// BuiltInRegistries.ENCHANTMENT registration ids in 1.20.5/1.20.6.
const ENCHANTMENT_766_AQUA_AFFINITY: u32 = 6;
const ENCHANTMENT_766_EFFICIENCY: u32 = 20;
const ENCHANTMENT_766_RIPTIDE: u32 = 32;
const ENCHANTMENT_766_VANISHING_CURSE: u32 = 41;
const LEGACY_EFFICIENCY_KEY: &str = "pomme:legacy_efficiency";
const LEGACY_AQUA_AFFINITY_KEY: &str = "pomme:legacy_aqua_affinity";

fn skip_holder_set_766(cur: &mut Cursor<&[u8]>) -> Option<()> {
    let encoded = u32::azalea_read_var(cur).ok()?;
    if encoded == 0 {
        skip_utf(cur)?;
    } else {
        for _ in 0..encoded - 1 {
            u32::azalea_read_var(cur).ok()?;
        }
    }
    Some(())
}

fn skip_tag_key(cur: &mut Cursor<&[u8]>) -> Option<()> {
    skip_utf(cur)
}

fn skip_tool_component(cur: &mut Cursor<&[u8]>, protocol: i32) -> Option<()> {
    let rules = u32::azalea_read_var(cur).ok()?;
    for _ in 0..rules {
        skip_holder_set_766(cur)?;
        if read_u8(cur)? != 0 {
            advance(cur, 4)?;
        }
        if read_u8(cur)? != 0 {
            advance(cur, 1)?;
        }
    }
    advance(cur, 4)?;
    u32::azalea_read_var(cur).ok()?;
    if protocol >= 770 {
        advance(cur, 1)?; // canDestroyBlocksInCreative (added in 1.21.5)
    }
    Some(())
}

fn skip_tool_766(cur: &mut Cursor<&[u8]>) -> Option<()> {
    skip_tool_component(cur, 766)
}

fn translate_tool_component_to_target(cur: &mut Cursor<&[u8]>, protocol: i32) -> Option<Vec<u8>> {
    let rules = u32::azalea_read_var(cur).ok()?;
    let mut translated_rules = Vec::with_capacity(rules as usize);
    for _ in 0..rules {
        let mut rule = Vec::new();
        let encoded = u32::azalea_read_var(cur).ok()?;
        let mut representable = true;
        if encoded == 0 {
            wire::write_varint(&mut rule, 0);
            let start = cur.position() as usize;
            skip_utf(cur)?;
            rule.extend_from_slice(&cur.get_ref()[start..cur.position() as usize]);
        } else {
            let mut blocks = Vec::with_capacity((encoded - 1) as usize);
            for _ in 0..encoded - 1 {
                let latest_id = u32::azalea_read_var(cur).ok()?;
                let Some(name) =
                    crate::world::block::block_registry_name(NATIVE.protocol, latest_id)
                else {
                    representable = false;
                    continue;
                };
                let Some(target_id) = crate::world::block::block_registry_id(protocol, name) else {
                    representable = false;
                    continue;
                };
                blocks.push(target_id);
            }
            if representable {
                wire::write_varint(&mut rule, blocks.len() as u32 + 1);
                for block in blocks {
                    wire::write_varint(&mut rule, block);
                }
            }
        }

        let has_speed = read_u8(cur)?;
        rule.push(has_speed);
        if has_speed != 0 {
            let start = cur.position() as usize;
            advance(cur, 4)?;
            rule.extend_from_slice(&cur.get_ref()[start..cur.position() as usize]);
        }
        let has_correct = read_u8(cur)?;
        rule.push(has_correct);
        if has_correct != 0 {
            rule.push(read_u8(cur)?);
        }
        if representable {
            translated_rules.push(rule);
        }
    }

    let mut payload = Vec::new();
    wire::write_varint(&mut payload, translated_rules.len() as u32);
    for rule in translated_rules {
        payload.extend_from_slice(&rule);
    }
    let start = cur.position() as usize;
    advance(cur, 4)?; // default mining speed
    payload.extend_from_slice(&cur.get_ref()[start..cur.position() as usize]);
    let damage = u32::azalea_read_var(cur).ok()?;
    wire::write_varint(&mut payload, damage);
    // Protocols 774/775 and 26.2 all include this field.
    payload.push(read_u8(cur)?);
    Some(payload)
}

fn copy_latest_component_payload(
    cur: &mut Cursor<&[u8]>,
    kind: DataComponentKind,
) -> Option<Vec<u8>> {
    let start = cur.position() as usize;
    let mut value = DataComponentUnion::azalea_read_as(kind, cur).ok()?;
    // SAFETY: `value` was constructed by `azalea_read_as` for exactly `kind`.
    unsafe { value.drop_as(kind) };
    Some(cur.get_ref()[start..cur.position() as usize].to_vec())
}

/// Component payloads that contain numeric ids from registries whose ordering
/// differs across the supported component-era protocols. These must never be
/// copied verbatim into another protocol's registry space.
fn historical_payload_has_unremapped_registry_ids(kind: DataComponentKind) -> bool {
    matches!(
        kind,
        DataComponentKind::Enchantments
            | DataComponentKind::CanPlaceOn
            | DataComponentKind::CanBreak
            | DataComponentKind::AttributeModifiers
            | DataComponentKind::Consumable
            | DataComponentKind::DamageResistant
            | DataComponentKind::Tool
            | DataComponentKind::Repairable
            | DataComponentKind::DeathProtection
            | DataComponentKind::BlocksAttacks
            | DataComponentKind::StoredEnchantments
            | DataComponentKind::PotionContents
            | DataComponentKind::SuspiciousStewEffects
            | DataComponentKind::Trim
            | DataComponentKind::EntityData
            | DataComponentKind::BlockEntityData
            | DataComponentKind::Instrument
            | DataComponentKind::ProvidesTrimMaterial
            | DataComponentKind::JukeboxPlayable
            | DataComponentKind::ProvidesBannerPatterns
            | DataComponentKind::BannerPatterns
            | DataComponentKind::PotDecorations
            | DataComponentKind::BreakSound
            | DataComponentKind::VillagerVariant
            | DataComponentKind::WolfVariant
            | DataComponentKind::WolfSoundVariant
            | DataComponentKind::PigVariant
            | DataComponentKind::PigSoundVariant
            | DataComponentKind::CowVariant
            | DataComponentKind::CowSoundVariant
            | DataComponentKind::ChickenVariant
            | DataComponentKind::ChickenSoundVariant
            | DataComponentKind::ZombieNautilusVariant
            | DataComponentKind::FrogVariant
            | DataComponentKind::PaintingVariant
            | DataComponentKind::CatVariant
            | DataComponentKind::CatSoundVariant
            | DataComponentKind::DamageType
            | DataComponentKind::PiercingWeapon
            | DataComponentKind::KineticWeapon
            | DataComponentKind::TooltipDisplay
    )
}

fn skip_old_attribute_modifiers(cur: &mut Cursor<&[u8]>, protocol: i32) -> Option<()> {
    let count = u32::azalea_read_var(cur).ok()?;
    for _ in 0..count {
        u32::azalea_read_var(cur).ok()?; // attribute registry id
        skip_utf(cur)?; // modifier identifier
        advance(cur, 8)?; // amount f64
        u32::azalea_read_var(cur).ok()?; // operation
        u32::azalea_read_var(cur).ok()?; // equipment slot group
    }
    if protocol <= 769 {
        advance(cur, 1)?; // showInTooltip, removed in 1.21.5
    }
    Some(())
}

fn translate_tooltip_display_payload(
    cur: &mut Cursor<&[u8]>,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let hide_tooltip = read_u8(cur)?;
    let count = u32::azalea_read_var(cur).ok()?;
    let mut hidden = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let component = u32::azalea_read_var(cur).ok()?;
        if let Some(component) = remaps.remap(ClientRegistry::DataComponentType, component) {
            hidden.push(component);
        }
    }
    let mut payload = Vec::new();
    payload.push(hide_tooltip);
    wire::write_varint(&mut payload, hidden.len() as u32);
    for component in hidden {
        wire::write_varint(&mut payload, component);
    }
    Some(payload)
}

fn translate_attribute_modifiers_payload(
    cur: &mut Cursor<&[u8]>,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let count = u32::azalea_read_var(cur).ok()?;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let attribute = u32::azalea_read_var(cur).ok()?;
        let body_start = cur.position() as usize;
        skip_utf(cur)?; // modifier identifier
        advance(cur, 8)?; // amount f64
        u32::azalea_read_var(cur).ok()?; // operation
        u32::azalea_read_var(cur).ok()?; // equipment slot group
        let display =
            azalea_inventory::components::AttributeModifierDisplay::azalea_read(cur).ok()?;
        drop(display);
        let body_end = cur.position() as usize;
        if let Some(attribute) = remaps.remap(ClientRegistry::Attribute, attribute) {
            let mut entry = Vec::with_capacity(body_end - body_start + 5);
            wire::write_varint(&mut entry, attribute);
            entry.extend_from_slice(&cur.get_ref()[body_start..body_end]);
            entries.push(entry);
        }
    }

    let mut payload = Vec::new();
    wire::write_varint(&mut payload, entries.len() as u32);
    for entry in entries {
        payload.extend_from_slice(&entry);
    }
    Some(payload)
}

fn skip_old_custom_model_data(cur: &mut Cursor<&[u8]>) -> Option<()> {
    i32::azalea_read_var(cur).ok()?;
    Some(())
}

fn skip_old_mob_effect_details(cur: &mut Cursor<&[u8]>) -> Option<()> {
    i32::azalea_read_var(cur).ok()?; // amplifier
    i32::azalea_read_var(cur).ok()?; // duration
    advance(cur, 3)?; // ambient, visible, showIcon
    if read_u8(cur)? != 0 {
        skip_old_mob_effect_details(cur)?;
    }
    Some(())
}

fn skip_old_mob_effect_instance(cur: &mut Cursor<&[u8]>) -> Option<()> {
    u32::azalea_read_var(cur).ok()?; // raw registry holder id
    skip_old_mob_effect_details(cur)
}

fn skip_old_food(
    cur: &mut Cursor<&[u8]>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<()> {
    i32::azalea_read_var(cur).ok()?; // nutrition
    advance(cur, 4)?; // saturation modifier
    advance(cur, 1)?; // canAlwaysEat
    advance(cur, 4)?; // eatSeconds
    let mut sink = Vec::new();
    translate_item_components(cur, &mut sink, ids, remaps)?; // usingConvertsTo ItemStack
    let effects = u32::azalea_read_var(cur).ok()?;
    for _ in 0..effects {
        skip_old_mob_effect_instance(cur)?;
        advance(cur, 4)?; // probability
    }
    Some(())
}

fn skip_old_potion_contents_767(cur: &mut Cursor<&[u8]>) -> Option<()> {
    if read_u8(cur)? != 0 {
        u32::azalea_read_var(cur).ok()?; // raw potion holderRegistry id
    }
    if read_u8(cur)? != 0 {
        advance(cur, 4)?; // custom color int
    }
    let effects = u32::azalea_read_var(cur).ok()?;
    for _ in 0..effects {
        skip_old_mob_effect_instance(cur)?;
    }
    Some(())
}

fn skip_old_equippable(cur: &mut Cursor<&[u8]>, protocol: i32) -> Option<()> {
    azalea_inventory::components::EquipmentSlot::azalea_read(cur).ok()?;
    u32::azalea_read_var(cur).ok()?; // raw sound-event registry id
    skip_optional(cur, skip_utf)?; // model/asset id
    skip_optional(cur, skip_utf)?; // camera overlay
    skip_optional(cur, skip_holder_set_766)?; // allowed entities
    advance(cur, 3)?; // dispensable, swappable, damageOnHurt
    if protocol >= 770 {
        advance(cur, 1)?; // equipOnInteract
    }
    if protocol >= 771 {
        advance(cur, 1)?; // canBeSheared
        u32::azalea_read_var(cur).ok()?; // raw shearing sound-event registry id
    }
    Some(())
}

fn skip_old_jukebox_song_direct(cur: &mut Cursor<&[u8]>) -> Option<()> {
    azalea_inventory::components::JukeboxSongData::azalea_read(cur).ok()?;
    Some(())
}

fn skip_old_instrument_direct(cur: &mut Cursor<&[u8]>, protocol: i32) -> Option<()> {
    if protocol == 767 {
        azalea_registry::Holder::<
            azalea_registry::builtin::SoundEvent,
            azalea_core::sound::CustomSound,
        >::azalea_read(cur)
        .ok()?;
        i32::azalea_read_var(cur).ok()?; // use duration
        advance(cur, 4)?; // range
    } else {
        azalea_inventory::components::InstrumentData::azalea_read(cur).ok()?;
    }
    Some(())
}

fn skip_either_holder(
    cur: &mut Cursor<&[u8]>,
    mut skip_direct: impl FnMut(&mut Cursor<&[u8]>) -> Option<()>,
) -> Option<()> {
    if read_u8(cur)? != 0 {
        let holder = u32::azalea_read_var(cur).ok()?;
        if holder == 0 {
            skip_direct(cur)?;
        }
    } else {
        skip_utf(cur)?; // resource key
    }
    Some(())
}

fn skip_either_registry_holder(cur: &mut Cursor<&[u8]>) -> Option<()> {
    if read_u8(cur)? != 0 {
        u32::azalea_read_var(cur).ok()?; // raw holderRegistry id
    } else {
        skip_utf(cur)?; // resource key
    }
    Some(())
}

fn skip_old_jukebox_playable(cur: &mut Cursor<&[u8]>, protocol: i32) -> Option<()> {
    skip_either_holder(cur, skip_old_jukebox_song_direct)?;
    if protocol <= 769 {
        advance(cur, 1)?; // showInTooltip
    }
    Some(())
}

fn skip_old_instrument(cur: &mut Cursor<&[u8]>, protocol: i32) -> Option<()> {
    if protocol <= 769 {
        // Through 1.21.4 the component stores a registry Holder directly.
        let holder = u32::azalea_read_var(cur).ok()?;
        if holder == 0 {
            skip_old_instrument_direct(cur, protocol)?;
        }
    } else {
        // 1.21.5+ wraps the holder/resource key in InstrumentComponent.
        skip_either_holder(cur, |cur| skip_old_instrument_direct(cur, protocol))?;
    }
    Some(())
}

fn skip_sound_holder(cur: &mut Cursor<&[u8]>) -> Option<()> {
    azalea_registry::Holder::<
        azalea_registry::builtin::SoundEvent,
        azalea_core::sound::CustomSound,
    >::azalea_read(cur)
    .ok()?;
    Some(())
}

fn skip_old_blocks_attacks(cur: &mut Cursor<&[u8]>) -> Option<()> {
    advance(cur, 8)?; // blockDelaySeconds, disableCooldownScale
    let reductions = u32::azalea_read_var(cur).ok()?;
    for _ in 0..reductions {
        advance(cur, 4)?; // horizontalBlockingAngle
        if read_u8(cur)? != 0 {
            skip_holder_set_766(cur)?; // optional damage-type HolderSet
        }
        advance(cur, 8)?; // base, factor
    }
    advance(cur, 12)?; // ItemDamageFunction: threshold, base, factor
    if read_u8(cur)? != 0 {
        skip_tag_key(cur)?; // optional bypassedBy damage-type TagKey through 1.21.11
    }
    if read_u8(cur)? != 0 {
        skip_sound_holder(cur)?;
    }
    if read_u8(cur)? != 0 {
        skip_sound_holder(cur)?;
    }
    Some(())
}

fn skip_old_provides_trim_material(cur: &mut Cursor<&[u8]>) -> Option<()> {
    skip_either_holder(cur, |cur| {
        azalea_inventory::components::DirectTrimMaterial::azalea_read(cur).ok()?;
        Some(())
    })
}

fn translate_nested_item_stack_payload(
    cur: &mut Cursor<&[u8]>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut payload = Vec::new();
    if ids.protocol == 775 {
        translate_item_template_to_native_stack(cur, &mut payload, ids, remaps)?;
    } else {
        translate_item_components_inner(cur, &mut payload, ids, remaps, true)?;
    }
    Some(payload)
}

fn translate_nested_item_vec_payload(
    cur: &mut Cursor<&[u8]>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
    container: bool,
) -> Option<Vec<u8>> {
    let count = u32::azalea_read_var(cur).ok()?;
    let mut payload = Vec::new();
    wire::write_varint(&mut payload, count);
    for _ in 0..count {
        if ids.protocol == 775 {
            if container {
                let present = read_u8(cur)?;
                if present == 0 {
                    wire::write_varint(&mut payload, 0);
                    continue;
                }
            }
            translate_item_template_to_native_stack(cur, &mut payload, ids, remaps)?;
        } else {
            translate_item_components_inner(cur, &mut payload, ids, remaps, true)?;
        }
    }
    Some(payload)
}

fn translate_item_components(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<()> {
    translate_item_components_inner(cur, out, ids, remaps, false)
}

fn historical_unmapped_unit_component(protocol: i32, name: &str) -> bool {
    ((767..=769).contains(&protocol) && matches!(name, "hide_additional_tooltip" | "hide_tooltip"))
        || (protocol == 767 && name == "fire_resistant")
}

fn translate_component_patch(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let added = u32::azalea_read_var(cur).ok()?;
    let removed = u32::azalea_read_var(cur).ok()?;
    let mut added_out: Vec<(u32, Vec<u8>)> = Vec::with_capacity(added as usize);
    for _ in 0..added {
        let component = u32::azalea_read_var(cur).ok()?;
        let name = ids
            .registry
            .name_of(ClientRegistry::DataComponentType, component)?;
        if name == "tool" && ids.protocol <= 773 {
            skip_tool_component(cur, ids.protocol)?;
            continue;
        }
        if historical_unmapped_unit_component(ids.protocol, name) {
            // These old-only components use unit StreamCodecs, so their
            // network payload is empty. Drop only the unsupported component;
            // the rest of the item patch remains valid.
            continue;
        }
        let latest = remaps.remap(ClientRegistry::DataComponentType, component)?;
        let latest_kind = DataComponentKind::from_u32(latest)?;
        let payload = if ids.protocol <= 769
            && matches!(
                name,
                "unbreakable"
                    | "enchantments"
                    | "stored_enchantments"
                    | "can_place_on"
                    | "can_break"
                    | "dyed_color"
                    | "trim"
            ) {
            let payload = copy_latest_component_payload(cur, latest_kind)?;
            advance(cur, 1)?; // pre-1.21.5 showInTooltip
            if historical_payload_has_unremapped_registry_ids(latest_kind) {
                continue;
            }
            payload
        } else if ids.protocol <= 770 && name == "attribute_modifiers" {
            skip_old_attribute_modifiers(cur, ids.protocol)?;
            continue;
        } else if (771..=775).contains(&ids.protocol) && name == "attribute_modifiers" {
            translate_attribute_modifiers_payload(cur, remaps)?
        } else if (770..=775).contains(&ids.protocol) && name == "tooltip_display" {
            translate_tooltip_display_payload(cur, remaps)?
        } else if (774..=775).contains(&ids.protocol) && name == "tool" {
            // Direct HolderSet block ids intentionally remain in source-version
            // numeric space here; stack_tool reinterprets them through Pomme's
            // negotiated block registry before gameplay uses them.
            copy_latest_component_payload(cur, latest_kind)?
        } else if ids.protocol == 767 && name == "food" {
            skip_old_food(cur, ids, remaps)?;
            continue;
        } else if name == "use_remainder" {
            translate_nested_item_stack_payload(cur, ids, remaps)?
        } else if matches!(name, "charged_projectiles" | "bundle_contents") {
            translate_nested_item_vec_payload(cur, ids, remaps, false)?
        } else if name == "container" {
            translate_nested_item_vec_payload(cur, ids, remaps, true)?
        } else if ids.protocol <= 768 && name == "custom_model_data" {
            skip_old_custom_model_data(cur)?;
            continue;
        } else if ids.protocol == 767 && name == "potion_contents" {
            skip_old_potion_contents_767(cur)?;
            continue;
        } else if (768..=775).contains(&ids.protocol) && name == "equippable" {
            skip_old_equippable(cur, ids.protocol)?;
            continue;
        } else if ids.protocol <= 774 && name == "jukebox_playable" {
            skip_old_jukebox_playable(cur, ids.protocol)?;
            continue;
        } else if ids.protocol <= 772 && name == "profile" {
            let mut payload = Vec::new();
            translate_old_profile(cur, &mut payload)?;
            payload
        } else if ids.protocol <= 772 && matches!(name, "entity_data" | "block_entity_data") {
            skip_nbt(cur)?;
            continue;
        } else if ids.protocol <= 774 && name == "instrument" {
            skip_old_instrument(cur, ids.protocol)?;
            continue;
        } else if (768..=774).contains(&ids.protocol) && name == "damage_resistant" {
            skip_tag_key(cur)?;
            continue;
        } else if (770..=774).contains(&ids.protocol) && name == "blocks_attacks" {
            skip_old_blocks_attacks(cur)?;
            continue;
        } else if (770..=774).contains(&ids.protocol) && name == "provides_trim_material" {
            skip_old_provides_trim_material(cur)?;
            continue;
        } else if (770..=774).contains(&ids.protocol) && name == "provides_banner_patterns" {
            skip_tag_key(cur)?;
            continue;
        } else if ((770..=774).contains(&ids.protocol) && name == "chicken/variant")
            || (ids.protocol == 774 && matches!(name, "damage_type" | "zombie_nautilus/variant"))
        {
            skip_either_registry_holder(cur)?;
            continue;
        } else {
            let payload = copy_latest_component_payload(cur, latest_kind)?;
            if historical_payload_has_unremapped_registry_ids(latest_kind) {
                continue;
            }
            payload
        };
        added_out.push((latest, payload));
    }
    let mut removed_out = Vec::with_capacity(removed as usize);
    for _ in 0..removed {
        let component = u32::azalea_read_var(cur).ok()?;
        let name = ids
            .registry
            .name_of(ClientRegistry::DataComponentType, component)?;
        if let Some(component) = remaps.remap(ClientRegistry::DataComponentType, component) {
            removed_out.push(component);
        } else if !historical_unmapped_unit_component(ids.protocol, name) {
            return None;
        }
    }
    wire::write_varint(out, added_out.len() as u32);
    wire::write_varint(out, removed_out.len() as u32);
    for (component, payload) in added_out {
        wire::write_varint(out, component);
        out.extend_from_slice(&payload);
    }
    for component in removed_out {
        wire::write_varint(out, component);
    }
    Some(())
}

fn translate_item_components_inner(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
    remap_item_kind: bool,
) -> Option<()> {
    let count = i32::azalea_read_var(cur).ok()?;
    wire::write_varint(out, count as u32);
    if count <= 0 {
        return Some(());
    }
    let item = u32::azalea_read_var(cur).ok()?;
    let item = if remap_item_kind {
        remaps.remap(ClientRegistry::Item, item)?
    } else {
        item
    };
    wire::write_varint(out, item);
    translate_component_patch(cur, out, ids, remaps)
}

fn translate_item_template_to_native_stack(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let item = u32::azalea_read_var(cur).ok()?;
    let count = i32::azalea_read_var(cur).ok()?;
    if count <= 0 {
        return None;
    }
    let item = remaps.remap(ClientRegistry::Item, item)?;
    wire::write_varint(out, count as u32);
    wire::write_varint(out, item);
    translate_component_patch(cur, out, ids, remaps)
}

fn translate_container_set_content_components(
    id: u32,
    payload: &[u8],
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, id);
    if ids.protocol == 767 {
        let container = read_u8(&mut cur)?;
        wire::write_varint(&mut out, u32::from(container));
    } else {
        let container = varint_span(&mut cur)?;
        out.extend_from_slice(&payload[container]);
    }
    let state = varint_span(&mut cur)?;
    out.extend_from_slice(&payload[state]);
    let count = u32::azalea_read_var(&mut cur).ok()?;
    wire::write_varint(&mut out, count);
    for _ in 0..=count {
        translate_item_components(&mut cur, &mut out, ids, remaps)?;
    }
    Some(out)
}

fn translate_set_equipment_components(
    id: u32,
    payload: &[u8],
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    loop {
        let slot = read_u8(&mut cur)?;
        out.push(slot);
        translate_item_components(&mut cur, &mut out, ids, remaps)?;
        if slot & 0x80 == 0 {
            return Some(out);
        }
    }
}

fn translate_container_set_slot_components(
    id: u32,
    payload: &[u8],
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?;
    varint_span(&mut cur)?;
    advance(&mut cur, 2)?;
    let mut out = Vec::with_capacity(payload.len() + 12);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    translate_item_components(&mut cur, &mut out, ids, remaps)?;
    Some(out)
}

fn translate_container_set_slot_components_767(
    v: &Ids767,
    payload: &[u8],
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let container = *payload.first()? as i8;
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 1)?;
    let state = varint_span(&mut cur)?;
    let slot_at = cur.position() as usize;
    let slot = i16::from_be_bytes(payload.get(slot_at..slot_at + 2)?.try_into().ok()?);
    advance(&mut cur, 2)?;
    let mut out = Vec::with_capacity(payload.len() + 12);
    match container {
        -1 => wire::write_varint(&mut out, v.set_cursor_item_id),
        -2 => {
            wire::write_varint(&mut out, v.set_player_inventory_id);
            wire::write_varint(&mut out, slot as u32);
        }
        _ => {
            wire::write_varint(&mut out, v.container_set_slot_id);
            wire::write_varint(&mut out, container as u32);
            out.extend_from_slice(&payload[state.start..slot_at + 2]);
        }
    }
    translate_item_components(&mut cur, &mut out, ids, remaps)?;
    Some(out)
}

fn translate_set_cursor_item_components(
    id: u32,
    payload: &[u8],
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    translate_item_components(&mut cur, &mut out, ids, remaps)?;
    Some(out)
}

fn translate_set_player_inventory_components(
    id: u32,
    payload: &[u8],
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let slot = varint_span(&mut cur)?;
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[slot]);
    translate_item_components(&mut cur, &mut out, ids, remaps)?;
    Some(out)
}

fn remap_enchantment_766(id: u32) -> Option<u32> {
    // 1.20.5/1.20.6's built-in order matches 26.2 through Riptide. 26.2
    // inserts Lunge immediately after Riptide, shifting every older vanilla
    // enchantment from Channeling through Vanishing Curse by one.
    match id {
        0..=ENCHANTMENT_766_RIPTIDE => Some(id),
        33..=ENCHANTMENT_766_VANISHING_CURSE => Some(id + 1),
        _ => None,
    }
}

fn unmap_enchantment_766(id: u32) -> Option<u32> {
    match id {
        0..=ENCHANTMENT_766_RIPTIDE => Some(id),
        34..=42 => Some(id - 1),
        _ => None, // Lunge and later 26.2-only enchantments do not exist on 766.
    }
}

fn translate_item_to_766(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let count_at = cur.position() as usize;
    let count = i32::azalea_read_var(cur).ok()?;
    out.extend_from_slice(&cur.get_ref()[count_at..cur.position() as usize]);
    if count <= 0 {
        return Some(());
    }
    let item = varint_span(cur)?;
    out.extend_from_slice(&cur.get_ref()[item]);
    let added = u32::azalea_read_var(cur).ok()?;
    let removed = u32::azalea_read_var(cur).ok()?;
    let mut added_out: Vec<(u32, Vec<u8>)> = Vec::with_capacity(added as usize);
    for _ in 0..added {
        let component = u32::azalea_read_var(cur).ok()?;
        let old = remaps.remap(ClientRegistry::DataComponentType, component)?;
        let mut payload = Vec::new();
        if component == DataComponentKind::CustomData.to_u32() {
            let value = nbt_span(cur)?;
            payload.extend_from_slice(&cur.get_ref()[value]);
        } else if matches!(component, c if c == DataComponentKind::MaxStackSize.to_u32()
            || c == DataComponentKind::MaxDamage.to_u32()
            || c == DataComponentKind::Damage.to_u32()
            || c == DataComponentKind::Rarity.to_u32()
            || c == DataComponentKind::RepairCost.to_u32())
        {
            let value = varint_span(cur)?;
            payload.extend_from_slice(&cur.get_ref()[value]);
        } else if component == DataComponentKind::Unbreakable.to_u32() {
            payload.push(1);
        } else if component == DataComponentKind::CustomName.to_u32()
            || component == DataComponentKind::ItemName.to_u32()
        {
            let value = nbt_span(cur)?;
            payload.extend_from_slice(&cur.get_ref()[value]);
        } else if component == DataComponentKind::Enchantments.to_u32() {
            let entries = u32::azalea_read_var(cur).ok()?;
            let mut old_entries = Vec::with_capacity(entries as usize);
            for _ in 0..entries {
                let enchantment = u32::azalea_read_var(cur).ok()?;
                let level = i32::azalea_read_var(cur).ok()?;
                if let Some(old_enchantment) = unmap_enchantment_766(enchantment) {
                    old_entries.push((old_enchantment, level));
                }
            }
            wire::write_varint(&mut payload, old_entries.len() as u32);
            for (enchantment, level) in old_entries {
                wire::write_varint(&mut payload, enchantment);
                wire::write_varint(&mut payload, level as u32);
            }
            payload.push(1);
        } else if component == DataComponentKind::EnchantmentGlintOverride.to_u32() {
            payload.push(read_u8(cur)?);
        } else if component == DataComponentKind::CreativeSlotLock.to_u32()
            || component == DataComponentKind::IntangibleProjectile.to_u32()
        {
        } else {
            return None;
        }
        added_out.push((old, payload));
    }
    let mut removed_out = Vec::with_capacity(removed as usize);
    for _ in 0..removed {
        let component = u32::azalea_read_var(cur).ok()?;
        if let Some(old) = remaps.remap(ClientRegistry::DataComponentType, component) {
            removed_out.push(old);
        }
    }
    wire::write_varint(out, added_out.len() as u32);
    wire::write_varint(out, removed_out.len() as u32);
    for (component, payload) in added_out {
        wire::write_varint(out, component);
        out.extend_from_slice(&payload);
    }
    for component in removed_out {
        wire::write_varint(out, component);
    }
    Some(())
}

fn creative_component_supported(protocol: i32, kind: DataComponentKind) -> bool {
    if protocol <= 770 && kind == DataComponentKind::AttributeModifiers {
        return false;
    }
    if historical_payload_has_unremapped_registry_ids(kind)
        && kind != DataComponentKind::AttributeModifiers
        && kind != DataComponentKind::TooltipDisplay
        && !((774..=775).contains(&protocol) && kind == DataComponentKind::Tool)
    {
        return false;
    }
    if protocol == 767 && kind == DataComponentKind::Food {
        return false;
    }
    if protocol <= 768 && kind == DataComponentKind::CustomModelData {
        return false;
    }
    if protocol == 767 && kind == DataComponentKind::PotionContents {
        return false;
    }
    if protocol <= 775 && kind == DataComponentKind::Equippable {
        return false;
    }
    if protocol <= 774
        && matches!(
            kind,
            DataComponentKind::JukeboxPlayable
                | DataComponentKind::Instrument
                | DataComponentKind::DamageResistant
                | DataComponentKind::BlocksAttacks
                | DataComponentKind::ProvidesTrimMaterial
                | DataComponentKind::ProvidesBannerPatterns
        )
    {
        return false;
    }
    if (770..=774).contains(&protocol) && kind == DataComponentKind::ChickenVariant {
        return false;
    }
    if protocol == 774
        && matches!(
            kind,
            DataComponentKind::DamageType | DataComponentKind::ZombieNautilusVariant
        )
    {
        return false;
    }
    if protocol <= 772
        && matches!(
            kind,
            DataComponentKind::Profile
                | DataComponentKind::EntityData
                | DataComponentKind::BlockEntityData
        )
    {
        return false;
    }
    true
}

fn translate_nested_item_to_target(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
    container: bool,
) -> Option<()> {
    // Pinned Azalea still serializes the source value as an ordinary ItemStack
    // (count, item, patch). Mojang 26.1 switched these component payloads to
    // ItemStackTemplate (item, count, patch), with Optional<Template> slots for
    // container. The nested patch itself always uses
    // DataComponentPatch.STREAM_CODEC, never the outer creative packet's
    // length-delimited value framing.
    let count = i32::azalea_read_var(cur).ok()?;
    if ids.protocol == 775 {
        if container {
            out.push(u8::from(count > 0));
        }
        if count <= 0 {
            return container.then_some(());
        }
        let item = u32::azalea_read_var(cur).ok()?;
        let item = remaps.remap(ClientRegistry::Item, item)?;
        wire::write_varint(out, item);
        wire::write_varint(out, count as u32);
        return translate_component_patch_to_target(cur, out, ids, remaps, false);
    }

    wire::write_varint(out, count as u32);
    if count <= 0 {
        return Some(());
    }
    let item = u32::azalea_read_var(cur).ok()?;
    let item = remaps.remap(ClientRegistry::Item, item)?;
    wire::write_varint(out, item);
    translate_component_patch_to_target(cur, out, ids, remaps, false)
}

fn translate_nested_item_stack_to_target(
    cur: &mut Cursor<&[u8]>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut payload = Vec::new();
    translate_nested_item_to_target(cur, &mut payload, ids, remaps, false)?;
    Some(payload)
}

fn translate_nested_item_vec_to_target(
    cur: &mut Cursor<&[u8]>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
    container: bool,
) -> Option<Vec<u8>> {
    let count = u32::azalea_read_var(cur).ok()?;
    let mut payload = Vec::new();
    wire::write_varint(&mut payload, count);
    for _ in 0..count {
        translate_nested_item_to_target(cur, &mut payload, ids, remaps, container)?;
    }
    Some(payload)
}

fn creative_component_payload(
    cur: &mut Cursor<&[u8]>,
    ids: &ComponentIds,
    kind: DataComponentKind,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let protocol = ids.protocol;
    let mut payload =
        if (771..=775).contains(&protocol) && kind == DataComponentKind::AttributeModifiers {
            translate_attribute_modifiers_payload(cur, remaps)?
        } else if (770..=775).contains(&protocol) && kind == DataComponentKind::TooltipDisplay {
            translate_tooltip_display_payload(cur, remaps)?
        } else if (774..=775).contains(&protocol) && kind == DataComponentKind::Tool {
            translate_tool_component_to_target(cur, protocol)?
        } else if kind == DataComponentKind::UseRemainder {
            translate_nested_item_stack_to_target(cur, ids, remaps)?
        } else if matches!(
            kind,
            DataComponentKind::ChargedProjectiles | DataComponentKind::BundleContents
        ) {
            translate_nested_item_vec_to_target(cur, ids, remaps, false)?
        } else if kind == DataComponentKind::Container {
            translate_nested_item_vec_to_target(cur, ids, remaps, true)?
        } else {
            copy_latest_component_payload(cur, kind)?
        };
    if protocol <= 769
        && matches!(
            kind,
            DataComponentKind::Unbreakable
                | DataComponentKind::Enchantments
                | DataComponentKind::StoredEnchantments
                | DataComponentKind::CanPlaceOn
                | DataComponentKind::CanBreak
                | DataComponentKind::DyedColor
                | DataComponentKind::Trim
        )
    {
        payload.push(1); // old showInTooltip
    }
    Some(payload)
}

fn translate_item_to_components_target(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let count = i32::azalea_read_var(cur).ok()?;
    wire::write_varint(out, count as u32);
    if count <= 0 {
        return Some(());
    }
    let item = u32::azalea_read_var(cur).ok()?;
    wire::write_varint(out, item);
    translate_component_patch_to_target(cur, out, ids, remaps, true)
}

fn translate_component_patch_to_target(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
    delimit_values: bool,
) -> Option<()> {
    let added = u32::azalea_read_var(cur).ok()?;
    let removed = u32::azalea_read_var(cur).ok()?;
    let mut added_out = Vec::with_capacity(added as usize);
    for _ in 0..added {
        let native_id = u32::azalea_read_var(cur).ok()?;
        let kind = DataComponentKind::from_u32(native_id)?;
        let payload = creative_component_payload(cur, ids, kind, remaps)?;
        if !creative_component_supported(ids.protocol, kind) {
            continue;
        }
        let Some(target_id) = remaps.remap(ClientRegistry::DataComponentType, native_id) else {
            continue;
        };
        added_out.push((target_id, payload));
    }

    let mut removed_out = Vec::with_capacity(removed as usize);
    for _ in 0..removed {
        let native_id = u32::azalea_read_var(cur).ok()?;
        if let Some(target_id) = remaps.remap(ClientRegistry::DataComponentType, native_id) {
            removed_out.push(target_id);
        }
    }

    wire::write_varint(out, added_out.len() as u32);
    wire::write_varint(out, removed_out.len() as u32);
    for (component, payload) in added_out {
        wire::write_varint(out, component);
        if delimit_values && ids.protocol >= 770 {
            wire::write_varint(out, payload.len() as u32);
        }
        out.extend_from_slice(&payload);
    }
    for component in removed_out {
        wire::write_varint(out, component);
    }
    Some(())
}

fn translate_creative_slot_components(
    old_id: u32,
    payload: &[u8],
    ids: &ComponentIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let slot_at = cur.position() as usize;
    advance(&mut cur, 2)?;
    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(&payload[slot_at..slot_at + 2]);
    translate_item_to_components_target(&mut cur, &mut out, ids, remaps)?;
    (cur.position() as usize == payload.len()).then_some(out)
}

fn translate_creative_slot_766(
    old_id: u32,
    payload: &[u8],
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let slot_at = cur.position() as usize;
    advance(&mut cur, 2)?;
    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(&payload[slot_at..slot_at + 2]);
    translate_item_to_766(&mut cur, &mut out, remaps)?;
    (cur.position() as usize == payload.len()).then_some(out)
}

/// Rewrites one 1.20.5/1.20.6 component-era stack into the latest component
/// id space. Stable scalar/text fields pass through, the full vanilla
/// enchantment map is remapped into 26.2's dynamic enchantment-id order, and
/// Efficiency/Aqua Affinity are additionally mirrored into client-private
/// custom-data fields for the historical mining formula.
fn translate_item_766(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let count_at = cur.position() as usize;
    let count = i32::azalea_read_var(cur).ok()?;
    out.extend_from_slice(&cur.get_ref()[count_at..cur.position() as usize]);
    if count <= 0 {
        return Some(());
    }

    // Item ids are left in wire space here. `remap_inbound` runs after typed
    // decode and moves the ItemKind into latest space exactly once.
    let item = varint_span(cur)?;
    out.extend_from_slice(&cur.get_ref()[item]);

    let added = u32::azalea_read_var(cur).ok()?;
    let removed = u32::azalea_read_var(cur).ok()?;
    let mut added_out: Vec<(u32, Vec<u8>)> = Vec::with_capacity(added as usize + 1);
    let mut custom_data: Option<simdnbt::owned::NbtCompound> = None;
    let mut enchantments_766 = Vec::new();
    let mut efficiency = 0i32;
    let mut aqua_affinity = false;

    for _ in 0..added {
        let component = u32::azalea_read_var(cur).ok()?;
        match component {
            COMPONENT_766_CUSTOM_DATA => {
                let nbt = simdnbt::owned::read_unnamed(cur).ok()?;
                custom_data = Some(match nbt {
                    simdnbt::owned::Nbt::Some(root) => root.as_compound(),
                    simdnbt::owned::Nbt::None => simdnbt::owned::NbtCompound::new(),
                });
            }
            COMPONENT_766_ENCHANTMENTS => {
                let entries = u32::azalea_read_var(cur).ok()?;
                for _ in 0..entries {
                    let enchantment = u32::azalea_read_var(cur).ok()?;
                    let level = i32::azalea_read_var(cur).ok()?;
                    if let Some(latest) = remap_enchantment_766(enchantment) {
                        enchantments_766.push((latest, level));
                    } else {
                        tracing::warn!(
                            "Dropping unknown 1.20.6 enchantment id {enchantment} while translating item stack"
                        );
                    }
                    match enchantment {
                        ENCHANTMENT_766_EFFICIENCY => efficiency = efficiency.max(level),
                        ENCHANTMENT_766_AQUA_AFFINITY if level > 0 => aqua_affinity = true,
                        _ => {}
                    }
                }
                advance(cur, 1)?; // showInTooltip
                let latest = remaps.remap(ClientRegistry::DataComponentType, component)?;
                let mut payload = Vec::new();
                wire::write_varint(&mut payload, enchantments_766.len() as u32);
                for (enchantment, level) in &enchantments_766 {
                    wire::write_varint(&mut payload, *enchantment);
                    wire::write_varint(&mut payload, *level as u32);
                }
                added_out.push((latest, payload));
            }
            COMPONENT_766_UNBREAKABLE => {
                advance(cur, 1)?; // old showInTooltip; latest is a unit value
                let latest = remaps.remap(ClientRegistry::DataComponentType, component)?;
                added_out.push((latest, Vec::new()));
            }
            COMPONENT_766_MAX_STACK_SIZE
            | COMPONENT_766_MAX_DAMAGE
            | COMPONENT_766_DAMAGE
            | COMPONENT_766_RARITY
            | COMPONENT_766_REPAIR_COST => {
                let value = varint_span(cur)?;
                let latest = remaps.remap(ClientRegistry::DataComponentType, component)?;
                added_out.push((latest, cur.get_ref()[value].to_vec()));
            }
            COMPONENT_766_CUSTOM_NAME | COMPONENT_766_ITEM_NAME => {
                let value = nbt_span(cur)?;
                let latest = remaps.remap(ClientRegistry::DataComponentType, component)?;
                added_out.push((latest, cur.get_ref()[value].to_vec()));
            }
            COMPONENT_766_GLINT_OVERRIDE => {
                let value_at = cur.position() as usize;
                advance(cur, 1)?;
                let latest = remaps.remap(ClientRegistry::DataComponentType, component)?;
                added_out.push((
                    latest,
                    cur.get_ref()[value_at..cur.position() as usize].to_vec(),
                ));
            }
            COMPONENT_766_CREATIVE_SLOT_LOCK | COMPONENT_766_INTANGIBLE_PROJECTILE => {
                let latest = remaps.remap(ClientRegistry::DataComponentType, component)?;
                added_out.push((latest, Vec::new()));
            }
            COMPONENT_766_HIDE_ADDITIONAL_TOOLTIP
            | COMPONENT_766_HIDE_TOOLTIP
            | COMPONENT_766_FIRE_RESISTANT => {
                // Old-only 1.20.6 unit components have zero-byte stream
                // payloads and no native 26.2 registry entry. Drop only the
                // unsupported marker; keep translating the surrounding patch.
            }
            COMPONENT_766_TOOL => {
                // The old Tool payload predates latest-only fields. Skip it
                // precisely so one unsupported override cannot drop the whole
                // inventory/equipment packet; gameplay uses the native item
                // default for protocol-766 Tool value overrides.
                skip_tool_766(cur)?;
            }
            _ => return None,
        }
    }

    let mut removed_out = Vec::with_capacity(removed as usize);
    for _ in 0..removed {
        let component = u32::azalea_read_var(cur).ok()?;
        if matches!(
            component,
            COMPONENT_766_HIDE_ADDITIONAL_TOOLTIP
                | COMPONENT_766_HIDE_TOOLTIP
                | COMPONENT_766_FIRE_RESISTANT
        ) {
            continue;
        }
        removed_out.push(remaps.remap(ClientRegistry::DataComponentType, component)?);
    }

    if let Some(compound) = custom_data.as_mut() {
        // These names are client-private transport metadata, never trusted
        // from server-provided custom_data.
        compound.remove(LEGACY_EFFICIENCY_KEY);
        compound.remove(LEGACY_AQUA_AFFINITY_KEY);
    }
    if efficiency > 0 || aqua_affinity {
        let compound = custom_data.get_or_insert_with(simdnbt::owned::NbtCompound::new);
        if efficiency > 0 {
            compound.insert(LEGACY_EFFICIENCY_KEY, efficiency);
        }
        if aqua_affinity {
            compound.insert(LEGACY_AQUA_AFFINITY_KEY, 1i8);
        }
    }
    if let Some(compound) = custom_data {
        let mut payload = Vec::new();
        simdnbt::owned::Nbt::new("".into(), compound)
            .azalea_write(&mut payload)
            .ok()?;
        added_out.push((COMPONENT_CUSTOM_DATA, payload));
        removed_out.retain(|component| *component != COMPONENT_CUSTOM_DATA);
    }

    wire::write_varint(out, added_out.len() as u32);
    wire::write_varint(out, removed_out.len() as u32);
    for (component, payload) in added_out {
        wire::write_varint(out, component);
        out.extend_from_slice(&payload);
    }
    for component in removed_out {
        wire::write_varint(out, component);
    }
    Some(())
}

/// Rewrites the 1.20.5/1.20.6 `container_set_content` byte container id and
/// every component-era stack into the latest packet/patch layout.
fn translate_container_set_content_766(
    id: u32,
    payload: &[u8],
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let container = read_u8(&mut cur)?;
    let state = u32::azalea_read_var(&mut cur).ok()?;
    let count = u32::azalea_read_var(&mut cur).ok()?;

    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, u32::from(container));
    wire::write_varint(&mut out, state);
    wire::write_varint(&mut out, count);
    for _ in 0..=count {
        // The trailing iteration is the carried item.
        translate_item_766(&mut cur, &mut out, remaps)?;
    }
    Some(out)
}

/// Rewrites a 1.20.5/1.20.6 `set_equipment` list, translating each stack.
fn translate_set_equipment_766(
    id: u32,
    payload: &[u8],
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    loop {
        let slot = read_u8(&mut cur)?;
        out.push(slot);
        translate_item_766(&mut cur, &mut out, remaps)?;
        if slot & 0x80 == 0 {
            return Some(out);
        }
    }
}

/// Rewrites 1.20.5/1.20.6 `container_set_slot`, including the -1/-2 cursor
/// and player-inventory sentinel split introduced later.
fn translate_container_set_slot_766(
    v: &Ids767,
    payload: &[u8],
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let container = *payload.first()? as i8;
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 1)?;
    let state = u32::azalea_read_var(&mut cur).ok()?;
    let slot_at = cur.position() as usize;
    let slot = i16::from_be_bytes(payload.get(slot_at..slot_at + 2)?.try_into().ok()?);
    advance(&mut cur, 2)?;

    let mut out = Vec::with_capacity(payload.len() + 12);
    match container {
        -1 => wire::write_varint(&mut out, v.set_cursor_item_id),
        -2 => {
            wire::write_varint(&mut out, v.set_player_inventory_id);
            wire::write_varint(&mut out, slot as u32);
        }
        _ => {
            wire::write_varint(&mut out, v.container_set_slot_id);
            wire::write_varint(&mut out, container as u32);
            wire::write_varint(&mut out, state);
            out.extend_from_slice(&payload[slot_at..slot_at + 2]);
        }
    }
    translate_item_766(&mut cur, &mut out, remaps)?;
    Some(out)
}

/// Rewrites `projectile_power`: 1.21 collapsed the per-axis acceleration
/// vector into its magnitude.
fn translate_projectile_power_766(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let magnitude = DVec3::from_array(read_f64s(&mut cur)?).length();

    let mut out = Vec::with_capacity(16);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.extend_from_slice(&magnitude.to_be_bytes());
    Some(out)
}

/// Rewrites a native `container_click` payload for 1.21.4, which carries
/// full item stacks where 1.21.5 hashes them (`ServerboundContainerClick-
/// Packet` in both references). A hash can't be reversed, so each stack is
/// reconstructed bare (item + count, no components); the server reconciles
/// any component mismatch by resyncing the slot. Item ids are already in
/// the wire version's space (`remap_hashed` runs before encoding).
fn translate_container_click(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let rewrite = || {
        let mut cur = Cursor::new(payload);
        // containerId, stateId varints; slot short, button byte, clickType
        // varint: identical on both sides.
        varint_span(&mut cur)?;
        varint_span(&mut cur)?;
        advance(&mut cur, 3)?;
        varint_span(&mut cur)?;

        let mut out = Vec::with_capacity(payload.len() + 8);
        wire::write_varint(&mut out, old_id);
        out.extend_from_slice(&payload[..cur.position() as usize]);

        let changed = u32::azalea_read_var(&mut cur).ok()?;
        wire::write_varint(&mut out, changed);
        for _ in 0..changed {
            let slot_at = cur.position() as usize;
            advance(&mut cur, 2)?; // slot short
            out.extend_from_slice(&payload[slot_at..cur.position() as usize]);
            write_bare_stack(&mut out, read_hashed_stack(&mut cur)?);
        }
        write_bare_stack(&mut out, read_hashed_stack(&mut cur)?);
        Some(out)
    };
    match rewrite() {
        Some(out) => vec![out],
        None => Vec::new(),
    }
}

/// Reads a `HashedStack` (present bool, item id, count, hashed added map,
/// removed set), returning `Some((item, count))` for a present stack.
fn read_hashed_stack(cur: &mut Cursor<&[u8]>) -> Option<Option<(u32, u32)>> {
    if read_u8(cur)? == 0 {
        return Some(None);
    }
    let item = u32::azalea_read_var(cur).ok()?;
    let count = u32::azalea_read_var(cur).ok()?;
    let added = u32::azalea_read_var(cur).ok()?;
    for _ in 0..added {
        varint_span(cur)?; // component id
        advance(cur, 4)?; // hash
    }
    let removed = u32::azalea_read_var(cur).ok()?;
    for _ in 0..removed {
        varint_span(cur)?;
    }
    Some(Some((item, count)))
}

/// Writes a pre-1.21.5 optional item stack with no components: count, item,
/// empty added/removed patch (or the zero count marking empty).
fn write_bare_stack(out: &mut Vec<u8>, stack: Option<(u32, u32)>) {
    match stack {
        Some((item, count)) => {
            wire::write_varint(out, count);
            wire::write_varint(out, item);
            wire::write_varint(out, 0);
            wire::write_varint(out, 0);
        }
        None => wire::write_varint(out, 0),
    }
}

/// Rewrites `player_chat`: 1.21.5 prepended a `globalIndex` varint; zero
/// keeps azalea's ordering checks happy.
fn translate_player_chat(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, 0);
    out.extend_from_slice(payload);
    Some(out)
}

/// Rewrites `update_advancements`: 1.21.5 appended a `showAdvancements`
/// bool, true in every vanilla send path.
fn translate_update_advancements(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(payload);
    out.push(1);
    Some(out)
}

/// Rewrites a native `attack` payload (`entityId`) into an old-layout
/// `interact` frame with the `ATTACK` action. The old packet's trailing
/// `usingSecondaryAction` bool doesn't exist on the new one and the server
/// ignores it for attacks, so it's synthesized as false.
fn translate_attack(interact_old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut pos = 0;
    let Some(entity_id) = wire::read_varint(payload, &mut pos) else {
        return Vec::new();
    };
    let mut out = interact_frame(interact_old_id, entity_id, ACTION_ATTACK);
    out.push(0);
    vec![out]
}

/// Rewrites a native `interact` payload (`entityId, hand, LpVec3 location,
/// usingSecondaryAction`) into old-layout `interact` frames. Old clients
/// always send `INTERACT_AT` (raw-float hit location, then hand) and follow
/// with `INTERACT` (hand only) unless the client-side `interactAt` result
/// consumed the action (`Minecraft.startUseItem` in the reference). The
/// translator can't evaluate that, so it always emits both — matching
/// vanilla for the many entities whose `interactAt` passes, but sending an
/// extra `INTERACT` to those that consume it (e.g. armor stands).
fn translate_interact(interact_old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let parse = || {
        let mut pos = 0;
        let entity_id = wire::read_varint(payload, &mut pos)?;
        let hand = wire::read_varint(payload, &mut pos)?;
        let location = wire::read_lp_vec3(payload, &mut pos)?;
        let secondary = *payload.get(pos)?;
        Some((entity_id, hand, location, secondary))
    };
    let Some((entity_id, hand, location, secondary)) = parse() else {
        return Vec::new();
    };

    let mut at = interact_frame(interact_old_id, entity_id, ACTION_INTERACT_AT);
    for c in [location.x as f32, location.y as f32, location.z as f32] {
        at.extend_from_slice(&c.to_be_bytes());
    }
    wire::write_varint(&mut at, hand);
    at.push(secondary);

    let mut plain = interact_frame(interact_old_id, entity_id, ACTION_INTERACT);
    wire::write_varint(&mut plain, hand);
    plain.push(secondary);

    vec![at, plain]
}

/// 26.3's appended `dye_color` serializer, which no native entity uses.
const DYE_COLOR_SERIALIZER: u32 = 43;

/// The native serializer id for a 26.3 `EntityDataSerializers` id: 26.3
/// appended `dye_color` and shifted nothing (both registration blocks
/// line-checked).
fn remap_serializer_777(old: u32) -> Option<u32> {
    (old < DYE_COLOR_SERIALIZER).then_some(old)
}

/// The native serializer id for a 1.21.11 `EntityDataSerializers` id: 26.x
/// interleaved `cat/cow/pig/chicken_sound_variant` at ids 22/24/29/31
/// (line-checked against both versions' `EntityDataSerializers.java`
/// registration blocks; anchored by tests in `azalea_compat`).
fn remap_serializer_774(old: u32) -> Option<u32> {
    Some(match old {
        0..=21 => old,
        22 => 23,
        23..=26 => old + 2,
        27 => 30,
        28..=38 => old + 4,
        _ => return None,
    })
}

/// The native serializer id for a 1.21.10 `EntityDataSerializers` id:
/// 1.21.11 inserted `zombie_nautilus_variant` right above `chicken_variant`
/// (27), shifting everything past it by one more slot; below that the
/// 1.21.11 interleave applies unchanged (its trailing `humanoid_arm`
/// addition shifts nothing).
fn remap_serializer_773(old: u32) -> Option<u32> {
    match old {
        0..=27 => remap_serializer_774(old),
        28..=36 => Some(old + 5),
        _ => None,
    }
}

/// The pre-1.21.9 wire id of the `compound_tag` entity-data serializer,
/// which 1.21.9 removed; `remap_serializer_772` maps it to `None` and
/// `translate_entity_data` strips entries using it.
const COMPOUND_TAG_SERIALIZER: u32 = 16;

/// The native serializer id for a 1.21.8 `EntityDataSerializers` id: 1.21.9
/// removed `compound_tag` (16), shifting everything above it down one, and
/// inserted `copper_golem_state`/`weathering_copper_state` right below
/// `vector3`; on either side the 1.21.10 interleave applies unchanged.
fn remap_serializer_772(old: u32) -> Option<u32> {
    match old {
        0..=15 => Some(old),
        COMPOUND_TAG_SERIALIZER => None,
        17..=32 => remap_serializer_773(old - 1),
        33..=34 => remap_serializer_773(old + 1),
        _ => None,
    }
}

/// The native serializer id for a 1.21.4 `EntityDataSerializers` id: 1.21.5
/// interleaved the cow/pig/chicken/wolf-sound variant serializers (and
/// renamed `optional_uuid` to the wire-identical
/// `optional_living_entity_reference`); mapping through the 1.21.8 ids
/// covers the rest, `compound_tag` (16) included.
fn remap_serializer_769(old: u32) -> Option<u32> {
    match old {
        0..=22 => remap_serializer_772(old),
        23 => remap_serializer_772(24), // wolf_variant
        24 => remap_serializer_772(26), // frog_variant
        25..=30 => remap_serializer_772(old + 4),
        _ => None,
    }
}

/// Rewrites the game `login` payload: 26.2 added `onlineMode` before the
/// trailing `enforcesSecureChat` bool. It's written false here; the game loop
/// substitutes whether the connection is encrypted, which gated
/// `prepareKeyPair` before 26.2.
fn translate_game_login(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let (secure_chat, body) = payload.split_last()?;
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(body);
    out.push(0);
    out.push(*secure_chat);
    Some(out)
}

/// Rewrites `set_entity_data` (`entityId`, then `(u8 index, varint
/// serializer, value)` entries terminated by `0xFF`) by remapping each
/// entry's serializer id through the wire version's `serializer_map`. Value
/// layouts are identical between the versions (verified serializer by
/// serializer); they're skipped, not decoded, except particle values, whose
/// type ids are remapped in place ([`translate_particles`]). An entry using
/// a serializer the native version dropped (1.21.8's `compound_tag`) is
/// stripped rather than failing the packet. An item-stack value can't
/// always be walked without full component codecs — the remainder is copied
/// verbatim, which is correct unless a shifted serializer follows one (no
/// vanilla entity sends one after an untranslatable stack).
/// TODO: translate shoulder-parrot NBT to 26.2's OptionalInt variant
/// instead of stripping it, so shoulder parrots show on old servers.
/// TODO: lift pre-1.21.6 hanging-entity indices (an item frame's item/rotation
/// at 8/9, a painting's variant at 8) into 26.2's numbering instead of passing
/// the index byte through, so those entities can render on those wires.
fn translate_entity_data(
    id: u32,
    payload: &[u8],
    ids: &GameIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    loop {
        let index = read_u8(&mut cur)?;
        if index == 0xFF {
            out.push(index);
            break;
        }
        let old = u32::azalea_read_var(&mut cur).ok()?;
        let Some(new) = (ids.serializer_map)(old) else {
            if ids.v772.is_some() && old == COMPOUND_TAG_SERIALIZER {
                // compound_tag values are network NBT; drop the entry.
                skip_nbt_root(&mut cur, ids.v763.is_some())?;
                continue;
            }
            if ids.v777.is_some() && old == DYE_COLOR_SERIALIZER {
                // Only the cushion sends one, and its add_entity is dropped.
                varint_span(&mut cur)?;
                continue;
            }
            return None;
        };
        out.push(index);
        wire::write_varint(&mut out, new);
        let value_at = cur.position() as usize;
        if ids.v764.is_some() && matches!(new, 5 | 6) {
            // 764 component values are JSON strings; a skip would desync
            // the rest of the list, so a bad one drops the packet.
            let done = if new == 6 {
                copy_optional(&mut cur, &mut out, transcode_component)
            } else {
                transcode_component(&mut cur, &mut out)
            };
            done?;
            continue;
        }
        if new == 7 {
            let mut stack = Vec::new();
            let translated = if ids.v765.is_some() {
                translate_item_765(&mut cur, &mut stack, ids.v763.is_some())
            } else {
                translate_item_stack(&mut cur, &mut stack, remaps, ids.v772.is_some())
            };
            if translated.is_some() {
                out.extend_from_slice(&stack);
                continue;
            }
            tracing::debug!("Copying entity data tail verbatim past an item stack");
            out.extend_from_slice(&payload[value_at..]);
            return Some(out);
        }
        if new == 16 || new == 17 {
            let mut particles = Vec::new();
            translate_particles(&mut cur, &mut particles, ids, remaps, new == 17)?;
            out.extend_from_slice(&particles);
            continue;
        }
        if !skip_metadata_value(&mut cur, new)? {
            tracing::debug!("Copying entity data tail verbatim past serializer {old}");
            out.extend_from_slice(&payload[value_at..]);
            return Some(out);
        }
        out.extend_from_slice(&payload[value_at..cur.position() as usize]);
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Native-registry component ids whose payloads the stack walker can advance
/// past (26.2 `DataComponents` registration order; anchored in
/// `component_id_anchors` in `azalea_compat`). Matching happens after the
/// remap, so one set of ids serves every wire version.
pub(crate) const COMPONENT_CUSTOM_DATA: u32 = 0;
pub(crate) const COMPONENT_MAP_ID: u32 = 46;
pub(crate) const COMPONENT_PROFILE: u32 = 70;

/// Remaps one entity-data item stack (count, item id, component patch) into
/// the native registry space. `old_profile` marks the pre-1.21.9 `profile`
/// component layout (see [`translate_old_profile`]). `None` means a
/// component payload the walker doesn't know (or a malformed stack); the
/// caller falls back to the verbatim-tail copy.
fn translate_item_stack(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
    old_profile: bool,
) -> Option<()> {
    let count_at = cur.position() as usize;
    let count = i32::azalea_read_var(cur).ok()?;
    out.extend_from_slice(&cur.get_ref()[count_at..cur.position() as usize]);
    if count <= 0 {
        return Some(());
    }
    let item = u32::azalea_read_var(cur).ok()?;
    wire::write_varint(out, remaps.remap(ClientRegistry::Item, item)?);
    let added_at = cur.position() as usize;
    let added = u32::azalea_read_var(cur).ok()?;
    out.extend_from_slice(&cur.get_ref()[added_at..cur.position() as usize]);
    let removed_at = cur.position() as usize;
    let removed = u32::azalea_read_var(cur).ok()?;
    out.extend_from_slice(&cur.get_ref()[removed_at..cur.position() as usize]);
    for _ in 0..added {
        let component = u32::azalea_read_var(cur).ok()?;
        let native = remaps.remap(ClientRegistry::DataComponentType, component)?;
        wire::write_varint(out, native);
        let value_at = cur.position() as usize;
        match native {
            COMPONENT_MAP_ID => {
                varint_span(cur)?;
            }
            COMPONENT_PROFILE if old_profile => {
                translate_old_profile(cur, out)?;
                continue;
            }
            COMPONENT_PROFILE => {
                Profile::azalea_read(cur).ok()?;
            }
            _ => return None,
        }
        out.extend_from_slice(&cur.get_ref()[value_at..cur.position() as usize]);
    }
    for _ in 0..removed {
        let component = u32::azalea_read_var(cur).ok()?;
        wire::write_varint(
            out,
            remaps.remap(ClientRegistry::DataComponentType, component)?,
        );
    }
    Some(())
}

/// Rewrites a pre-1.21.9 `profile` component value (optional name, optional
/// uuid, property map) into 26.2's `ResolvableProfile`: an either bool
/// picking full/partial profile — the old triple matches the partial arm
/// byte for byte — plus a skin patch, empty here (four absent optionals).
fn translate_old_profile(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let value_at = cur.position() as usize;
    skip_optional(cur, skip_utf)?; // name
    skip_optional(cur, |c| advance(c, 16))?; // uuid
    let properties = u32::azalea_read_var(cur).ok()?;
    for _ in 0..properties {
        skip_utf(cur)?; // property name
        skip_utf(cur)?; // value
        skip_optional(cur, skip_utf)?; // signature
    }
    out.push(0); // either: the partial-profile arm
    out.extend_from_slice(&cur.get_ref()[value_at..cur.position() as usize]);
    out.extend_from_slice(&[0; 4]); // empty PlayerSkin.Patch
    Some(())
}

/// Advances past one entity-data value of the given native-version
/// serializer (the caller remaps first). `Some(false)` means the value (and
/// thus anything after it) can't be walked; `None` means the data is
/// malformed.
fn skip_metadata_value(cur: &mut Cursor<&[u8]>, serializer: u32) -> Option<bool> {
    match serializer {
        0 | 8 => advance(cur, 1)?, // byte, boolean
        3 => advance(cur, 4)?,     // float
        9 => advance(cur, 12)?,    // rotations
        10 => advance(cur, 8)?,    // block_pos
        39 => advance(cur, 12)?,   // vector3
        40 => advance(cur, 16)?,   // quaternion
        41 => {
            Profile::azalea_read(cur).ok()?;
        }
        // varint-shaped: int, enums, registry/holder ids, optional ints
        1 | 12 | 14 | 15 | 19 | 20 | 21 | 22..=32 | 35..=38 | 42 => {
            varint_span(cur)?;
        }
        2 => {
            u64::azalea_read_var(cur).ok()?; // var_long
        }
        4 => skip_utf(cur)?,                           // string
        5 => skip_nbt(cur)?,                           // component
        6 => skip_optional(cur, skip_nbt)?,            // optional component
        11 => skip_optional(cur, |c| advance(c, 8))?,  // optional block_pos
        13 => skip_optional(cur, |c| advance(c, 16))?, // optional entity ref (UUID)
        18 => {
            // villager data: type + profession holder ids, level
            varint_span(cur)?;
            varint_span(cur)?;
            varint_span(cur)?;
        }
        33 => {
            // optional global pos: dimension key + block pos
            skip_optional(cur, |c| {
                skip_utf(c)?;
                advance(c, 8)
            })?;
        }
        34 => {
            // painting variant holder: id + 1, or 0 followed by the direct
            // form (width, height, asset id, optional title/author)
            if u32::azalea_read_var(cur).ok()? == 0 {
                varint_span(cur)?;
                varint_span(cur)?;
                skip_utf(cur)?;
                skip_optional(cur, skip_nbt)?;
                skip_optional(cur, skip_nbt)?;
            }
        }
        // item stacks (7) and particles (16/17) have their own translation
        // paths in `translate_entity_data`; anything else can't be walked
        // without its full value codec
        _ => return Some(false),
    }
    Some(true)
}

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

/// Rewrites one particle (or, with `list`, a counted particle list):
/// each type id is remapped into 26.2's space, a color payload
/// (`entity_effect`/`tinted_leaves`, an ARGB int on wire versions that have
/// it) is copied through, and payload-less particles pass bare. `None` (a
/// particle 26.2 dropped, or a payload the walker can't rewrite) fails the
/// packet — a verbatim fallback would leave wire-space ids and, in entity
/// data, any following entries' shifted serializer ids in place.
/// TODO: rewrite the remaining `PAYLOAD_PARTICLES` payloads (block states,
/// dust colors, items) so entities and explosions using them translate.
fn translate_particles(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    ids: &GameIds,
    remaps: &RegistryRemaps,
    list: bool,
) -> Option<()> {
    let count = if list {
        let count = u32::azalea_read_var(cur).ok()?;
        wire::write_varint(out, count);
        count
    } else {
        1
    };
    for _ in 0..count {
        let old = u32::azalea_read_var(cur).ok()?;
        let new = remaps.remap(ClientRegistry::ParticleType, old)?;
        wire::write_varint(out, new);
        let name = RegistryTable::native().name_of(ClientRegistry::ParticleType, new)?;
        match name {
            "entity_effect" | "tinted_leaves" if ids.color_particles => {
                let color_at = cur.position() as usize;
                advance(cur, 4)?;
                out.extend_from_slice(&cur.get_ref()[color_at..cur.position() as usize]);
            }
            n if PAYLOAD_PARTICLES.contains(&n) => {
                if ids.v777.is_none() {
                    tracing::debug!("Dropping a packet with an untranslatable {n} particle");
                    return None;
                }
                copy_particle_payload(cur, out, n, remaps)?;
            }
            _ => {}
        }
    }
    Some(())
}

/// Copies one 26.3 particle payload through (`net/minecraft/core/particles`
/// option codecs, byte-identical to 26.2's): block states stay in wire space
/// like every block id pomme reads, and an `item`'s ids are remapped. `None`
/// for a payload that can't be sized (an item component the native version
/// lacks).
fn copy_particle_payload(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    name: &str,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let at = cur.position() as usize;
    match name {
        "block" | "block_crumble" | "block_marker" | "dust_pillar" | "falling_dust" | "shriek" => {
            varint_span(cur)?;
        }
        "dragon_breath" | "sculk_charge" | "entity_effect" | "tinted_leaves" | "flash"
        | "geyser" | "geyser_plume" => advance(cur, 4)?,
        "dust" | "effect" | "instant_effect" | "geyser_base" | "geyser_poof" => advance(cur, 8)?,
        "dust_color_transition" => advance(cur, 12)?,
        "trail" => {
            advance(cur, 28)?; // target, color
            varint_span(cur)?; // duration
        }
        "vibration" => {
            match u32::azalea_read_var(cur).ok()? {
                0 => advance(cur, 8)?, // block position
                1 => {
                    varint_span(cur)?; // entity id
                    advance(cur, 4)?; // y offset
                }
                _ => return None,
            }
            varint_span(cur)?; // arrival ticks
        }
        "item" => {
            let item = u32::azalea_read_var(cur).ok()?;
            wire::write_varint(out, remaps.remap(ClientRegistry::Item, item)?);
            let count = varint_span(cur)?;
            out.extend_from_slice(&cur.get_ref()[count]);
            return copy_component_patch(cur, out, remaps);
        }
        _ => return None,
    }
    out.extend_from_slice(&cur.get_ref()[at..cur.position() as usize]);
    Some(())
}

/// Shared components whose 26.3 value layout differs from 26.2's, so
/// azalea's decoder would mis-size them: `pot_decorations` became four
/// optional item templates, inline trim materials gained a `paletteId`,
/// inline instruments a `durabilityDamage`, and `teleport_randomly` consume
/// effects a `directionalParticles` bool.
/// TODO: rewrite these values instead of refusing the patch.
const CHANGED_COMPONENTS_777: &[DataComponentKind] = &[
    DataComponentKind::PotDecorations,
    DataComponentKind::Trim,
    DataComponentKind::ProvidesTrimMaterial,
    DataComponentKind::Instrument,
    DataComponentKind::Consumable,
    DataComponentKind::DeathProtection,
];

/// Copies a 26.3 component patch whose type ids are in the wire version's
/// space, writing native ids and sizing each value with azalea's decoder for
/// the native component it maps to. `None` for a component the native
/// version lacks or whose layout changed ([`CHANGED_COMPONENTS_777`]).
fn copy_component_patch(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let added = u32::azalea_read_var(cur).ok()?;
    let removed = u32::azalea_read_var(cur).ok()?;
    wire::write_varint(out, added);
    wire::write_varint(out, removed);
    for _ in 0..added {
        let component = u32::azalea_read_var(cur).ok()?;
        let native = remaps.remap(ClientRegistry::DataComponentType, component)?;
        let kind = DataComponentKind::from_u32(native)?;
        if CHANGED_COMPONENTS_777.contains(&kind) {
            return None;
        }
        wire::write_varint(out, native);
        let value_at = cur.position() as usize;
        DataComponentUnion::azalea_read_as(kind, cur).ok()?;
        out.extend_from_slice(&cur.get_ref()[value_at..cur.position() as usize]);
    }
    for _ in 0..removed {
        let component = u32::azalea_read_var(cur).ok()?;
        wire::write_varint(
            out,
            remaps.remap(ClientRegistry::DataComponentType, component)?,
        );
    }
    Some(())
}

/// Copies one `Holder<SoundEvent>` (varint id + 1, or 0 followed by the
/// inline definition: location string plus an optional fixed range),
/// remapping a referenced id into 26.2's space.
fn translate_sound_holder(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let holder = u32::azalea_read_var(cur).ok()?;
    if holder == 0 {
        let inline_at = cur.position() as usize;
        skip_utf(cur)?;
        skip_optional(cur, |c| advance(c, 4))?;
        out.push(0);
        out.extend_from_slice(&cur.get_ref()[inline_at..cur.position() as usize]);
        return Some(());
    }
    let new = remaps.remap(ClientRegistry::SoundEvent, holder - 1)?;
    wire::write_varint(out, new + 1);
    Some(())
}

fn skip_utf(cur: &mut Cursor<&[u8]>) -> Option<()> {
    let len = u32::azalea_read_var(cur).ok()?;
    advance(cur, len as usize)
}

fn skip_nbt(cur: &mut Cursor<&[u8]>) -> Option<()> {
    skip_nbt_root(cur, false)
}

fn skip_optional(
    cur: &mut Cursor<&[u8]>,
    inner: impl Fn(&mut Cursor<&[u8]>) -> Option<()>,
) -> Option<()> {
    if read_u8(cur)? != 0 {
        inner(cur)
    } else {
        Some(())
    }
}

/// Rewrites `level_chunk_with_light` by inserting the `fluidCount` short
/// 26.2 added after each section's `nonEmptyBlockCount` (zero: pomme
/// doesn't consume it and the client recounts on block changes). The
/// heightmaps before the section buffer are copied verbatim — or, with
/// `nbt_heightmaps` (pre-1.21.5), converted from the network-NBT compound
/// to the packed list. The block entities after it are copied verbatim unless
/// the wire version names its NBT roots, in which case each tag is rewritten;
/// the light data that follows is always verbatim.
fn translate_chunk(
    id: u32,
    payload: &[u8],
    nbt_heightmaps: bool,
    named_nbt: bool,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 8)?; // chunk x/z ints

    let mut head = Vec::with_capacity(64);
    head.extend_from_slice(&payload[..8]);
    if nbt_heightmaps {
        convert_nbt_heightmaps(&mut cur, &mut head, named_nbt)?;
    } else {
        let maps_at = cur.position() as usize;
        skip_heightmaps(&mut cur)?;
        head.extend_from_slice(&payload[maps_at..cur.position() as usize]);
    }
    let buffer_len = u32::azalea_read_var(&mut cur).ok()? as usize;
    let buffer_at = cur.position() as usize;
    let buffer_end = buffer_at.checked_add(buffer_len)?;
    if buffer_end > payload.len() {
        return None;
    }

    let mut buffer = Vec::with_capacity(buffer_len + 3 * 26 * 2);
    let mut bcur = Cursor::new(&payload[..buffer_end]);
    bcur.set_position(buffer_at as u64);
    while (bcur.position() as usize) < buffer_end {
        let section_at = bcur.position() as usize;
        advance(&mut bcur, 2)?; // nonEmptyBlockCount
        buffer.extend_from_slice(&payload[section_at..bcur.position() as usize]);
        buffer.extend_from_slice(&[0, 0]); // fluidCount, new in 26.2
        let rest_at = bcur.position() as usize;
        skip_paletted_container(&mut bcur, 4096, 8)?;
        skip_paletted_container(&mut bcur, 64, 3)?;
        buffer.extend_from_slice(&payload[rest_at..bcur.position() as usize]);
    }

    let mut out = Vec::with_capacity(head.len() + buffer.len() + payload.len() - buffer_end + 8);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&head);
    wire::write_varint(&mut out, buffer.len() as u32);
    out.extend_from_slice(&buffer);
    if named_nbt {
        let mut tail = Cursor::new(payload);
        tail.set_position(buffer_end as u64);
        copy_block_entities(&mut tail, &mut out, named_nbt)?;
        out.extend_from_slice(&payload[tail.position() as usize..]);
    } else {
        out.extend_from_slice(&payload[buffer_end..]);
    }
    Some(out)
}

/// Advances past a chunk's packed heightmap list (type id, long array).
fn skip_heightmaps(cur: &mut Cursor<&[u8]>) -> Option<()> {
    let heightmaps = u32::azalea_read_var(cur).ok()?;
    for _ in 0..heightmaps {
        varint_span(cur)?; // heightmap type
        let longs = u32::azalea_read_var(cur).ok()?;
        advance(cur, (longs as usize).checked_mul(8)?)?;
    }
    Some(())
}

/// Rewrites `level_chunk_with_light` from 26.3, whose chunk data is
/// byte-identical and whose light masks changed codec (see
/// [`copy_light_data_777`]).
fn translate_chunk_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 8)?; // chunk x/z ints
    skip_heightmaps(&mut cur)?;
    let buffer_len = u32::azalea_read_var(&mut cur).ok()?;
    advance(&mut cur, buffer_len as usize)?;

    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    copy_block_entities(&mut cur, &mut out, false)?;
    copy_light_data_777(&mut cur, &mut out)?;
    Some(out)
}

/// Rewrites `light_update` from 26.3: chunk x/z varints, then light data.
fn translate_light_update_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?;
    varint_span(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 16);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    copy_light_data_777(&mut cur, &mut out)?;
    Some(out)
}

/// Copies `ClientboundLightUpdatePacketData`: 26.3 sends its four section
/// masks with `ByteBufCodecs.BIT_SET` where 26.2 wrote long arrays; the two
/// light-layer lists after them copy verbatim.
fn copy_light_data_777(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    for _ in 0..4 {
        repack_bit_set(cur, out)?;
    }
    out.extend_from_slice(&cur.get_ref()[cur.position() as usize..]);
    Some(())
}

/// Rewrites one 26.3 `ByteBufCodecs.BIT_SET` (`BitSet.toByteArray`: a varint
/// byte count, little-endian bytes) as the 26.2 `writeBitSet` form
/// (`toLongArray`: a varint long count, big-endian longs).
fn repack_bit_set(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let len = u32::azalea_read_var(cur).ok()? as usize;
    let at = cur.position() as usize;
    advance(cur, len)?;
    let bytes = &cur.get_ref()[at..at + len];
    wire::write_varint(out, len.div_ceil(8) as u32);
    for chunk in bytes.chunks(8) {
        let mut word = [0; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        out.extend_from_slice(&u64::from_le_bytes(word).to_be_bytes());
    }
    Some(())
}

/// Copies a chunk's block-entity list (`packedXZ`, `y`, type, tag), rewriting
/// each tag into the unnamed NBT form. Leaves the cursor at the light data.
fn copy_block_entities(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, named_nbt: bool) -> Option<()> {
    let count = copy_varint(cur, out)?;
    for _ in 0..count {
        copy_bytes(cur, out, 3)?; // packed xz, y
        copy_varint(cur, out)?; // block entity type
        copy_unnamed_nbt(cur, out, named_nbt)?;
    }
    Some(())
}

/// Copies one NBT value, dropping the root name the wire version writes so the
/// output is the unnamed network form the native version expects.
fn copy_unnamed_nbt(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, named_nbt: bool) -> Option<()> {
    let tag = read_u8(cur)?;
    out.push(tag);
    skip_root_name(cur, tag, named_nbt)?;
    let at = cur.position() as usize;
    skip_nbt_payload(cur, tag, 0)?;
    out.extend_from_slice(&cur.get_ref()[at..cur.position() as usize]);
    Some(())
}

/// Converts a pre-1.21.5 network-NBT heightmap compound (named long-array
/// tags) into the packed `(type id, long array)` list, advancing past the
/// NBT. Type ids from `Heightmap.Types` in the 1.21.5 reference; entries
/// under other names or tags are dropped (vanilla only sends the three
/// client-usage types, all mapped).
fn convert_nbt_heightmaps(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    named_nbt: bool,
) -> Option<()> {
    let mut entries: Vec<(u32, u32, std::ops::Range<usize>)> = Vec::new();
    let root = read_u8(cur)?;
    skip_root_name(cur, root, named_nbt)?;
    if root == 10 {
        loop {
            let tag = read_u8(cur)?;
            if tag == 0 {
                break;
            }
            let name_len = read_u16(cur)? as usize;
            let name_at = cur.position() as usize;
            advance(cur, name_len)?;
            let type_id = match &cur.get_ref()[name_at..name_at + name_len] {
                b"WORLD_SURFACE_WG" => Some(0),
                b"WORLD_SURFACE" => Some(1),
                b"OCEAN_FLOOR_WG" => Some(2),
                b"OCEAN_FLOOR" => Some(3),
                b"MOTION_BLOCKING" => Some(4),
                b"MOTION_BLOCKING_NO_LEAVES" => Some(5),
                _ => None,
            };
            if tag != 12 {
                skip_nbt_payload(cur, tag, 0)?;
                continue;
            }
            let longs = u32::try_from(read_i32(cur)?).ok()?;
            let data_at = cur.position() as usize;
            advance(cur, (longs as usize).checked_mul(8)?)?;
            if let Some(type_id) = type_id {
                entries.push((type_id, longs, data_at..cur.position() as usize));
            }
        }
    } else if root != 0 {
        skip_nbt_payload(cur, root, 0)?;
    }

    wire::write_varint(out, entries.len() as u32);
    for (type_id, longs, range) in entries {
        wire::write_varint(out, type_id);
        wire::write_varint(out, longs);
        out.extend_from_slice(&cur.get_ref()[range]);
    }
    Some(())
}

/// Advances past one `PalettedContainer`: bits-per-entry byte, palette
/// (single value: one id; indirect while `bits <= max_indirect_bits`:
/// id list; global: nothing), then the unprefixed packed-long array.
fn skip_paletted_container(
    cur: &mut Cursor<&[u8]>,
    entries: usize,
    max_indirect_bits: u8,
) -> Option<()> {
    let bits = read_u8(cur)?;
    match bits {
        0 => {
            varint_span(cur)?;
        }
        _ if bits <= max_indirect_bits => {
            let palette_len = u32::azalea_read_var(cur).ok()?;
            for _ in 0..palette_len {
                varint_span(cur)?;
            }
        }
        _ => {}
    }
    if bits > 0 {
        let values_per_long = 64 / bits as usize;
        let longs = entries.div_ceil(values_per_long);
        advance(cur, longs.checked_mul(8)?)?;
    }
    Some(())
}

/// Rewrites `set_time` from `gameTime, dayTime, tickDayTime` to 26.2's
/// `gameTime` plus a world-clock map: one entry for clock id 0 carrying
/// `dayTime` as its total ticks and a rate of 1 or 0 for `tickDayTime`
/// (vanilla `ClockNetworkState`: var-long totalTicks, float partialTick,
/// float rate). Pomme reads day time from the first map entry.
fn translate_set_time(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let game_time = payload.get(..8)?;
    let day_time = u64::from_be_bytes(payload.get(8..16)?.try_into().ok()?);
    let tick_day_time = *payload.get(16)?;

    let mut out = Vec::with_capacity(32);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(game_time);
    out.push(1); // one clock update
    out.push(0); // world clock id 0
    day_time.azalea_write_var(&mut out).ok()?;
    out.extend_from_slice(&0f32.to_be_bytes()); // partial tick
    let rate: f32 = if tick_day_time != 0 { 1.0 } else { 0.0 };
    out.extend_from_slice(&rate.to_be_bytes());
    Some(out)
}

/// An `add_entity`/`set_entity_motion` velocity: three shorts in 1/8000ths
/// of a block per tick.
fn read_velocity(cur: &mut Cursor<&[u8]>) -> Option<DVec3> {
    let mut v = [0.0; 3];
    for c in &mut v {
        *c = f64::from(read_u16(cur)? as i16) / 8000.0;
    }
    Some(DVec3::from_array(v))
}

/// Rewrites `add_entity`: 1.21.9 moved the velocity from three trailing
/// shorts to an `LpVec3` between the position and the rotation bytes
/// (`ClientboundAddEntityPacket` read/write in both references). The entity
/// type id stays in the wire version's space; `remap_inbound` remaps it on
/// the decoded packet.
fn translate_add_entity(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id
    advance(&mut cur, 16)?; // uuid
    varint_span(&mut cur)?; // entity type
    advance(&mut cur, 24)?; // position doubles
    let rot_at = cur.position() as usize;
    advance(&mut cur, 3)?; // x/y/head rotation bytes
    varint_span(&mut cur)?; // data
    let rot_end = cur.position() as usize;
    let velocity = read_velocity(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..rot_at]);
    wire::write_lp_vec3(&mut out, velocity);
    out.extend_from_slice(&payload[rot_at..rot_end]);
    Some(out)
}

/// Rewrites `set_entity_motion` from `entityId` + three velocity shorts to
/// `entityId` + `LpVec3`.
fn translate_set_entity_motion(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let velocity = read_velocity(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    wire::write_lp_vec3(&mut out, velocity);
    Some(out)
}

/// Rewrites `player_rotation`: 1.21.9 added a relative-rotation bool after
/// each angle, absolute here matching the old packet's semantics.
fn translate_player_rotation(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let y_rot = payload.get(..4)?;
    let x_rot = payload.get(4..8)?;
    let mut out = Vec::with_capacity(12);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(y_rot);
    out.push(0);
    out.extend_from_slice(x_rot);
    out.push(0);
    Some(out)
}

/// Rewrites `set_default_spawn_position` from `BlockPos + angle` to 26.2's
/// `RespawnData` (`GlobalPos`, yaw, pitch). The old packet carries no
/// dimension or pitch: the overworld and zero are synthesized, and the
/// angle becomes the yaw.
/// TODO: carry the login/respawn dimension instead of synthesizing the
/// overworld, so compasses point right in other dimensions.
fn translate_set_default_spawn(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let pos = payload.get(..8)?;
    let angle = payload.get(8..12)?;
    const DIMENSION: &str = "minecraft:overworld";
    let mut out = Vec::with_capacity(40);
    wire::write_varint(&mut out, id);
    wire::write_varint(&mut out, DIMENSION.len() as u32);
    out.extend_from_slice(DIMENSION.as_bytes());
    out.extend_from_slice(pos);
    out.extend_from_slice(angle); // yaw
    out.extend_from_slice(&0f32.to_be_bytes()); // pitch
    Some(out)
}

/// Rewrites `explode`: 1.21.9 inserted `radius` and `blockCount` after the
/// center and appended a weighted block-particle list (zero/empty here),
/// and the particle and sound registry ids between the knockback and the
/// end are remapped into 26.2's space (vanilla sends
/// `explosion`/`explosion_emitter` and `entity.generic.explode`, all of
/// which shift).
fn translate_explode(
    id: u32,
    payload: &[u8],
    ids: &GameIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 24)?; // center doubles
    skip_optional(&mut cur, |c| advance(c, 24))?; // player knockback
    let knockback_end = cur.position() as usize;

    let mut out = Vec::with_capacity(payload.len() + 10);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..24]);
    out.extend_from_slice(&0f32.to_be_bytes()); // radius
    out.extend_from_slice(&0i32.to_be_bytes()); // block count
    out.extend_from_slice(&payload[24..knockback_end]);
    translate_particles(&mut cur, &mut out, ids, remaps, false)?;
    translate_sound_holder(&mut cur, &mut out, remaps)?;
    out.push(0); // no block particles
    Some(out)
}

fn remap_with<T: Registry>(remaps: &RegistryRemaps, reg: ClientRegistry, value: &mut T) -> bool {
    match remaps.remap(reg, value.to_u32()).and_then(T::from_u32) {
        Some(v) => {
            *value = v;
            true
        }
        None => false,
    }
}

fn retain_creative_components_766(data: &mut ItemStackData) {
    let original = &data.component_patch;
    let mut retained = DataComponentPatch::default();
    let mut kept = 0usize;

    macro_rules! keep_component {
        ($kind:ident, $ty:ty) => {
            if let Some((_, value)) = original
                .iter()
                .find(|(kind, _)| *kind == DataComponentKind::$kind)
            {
                unsafe {
                    match value {
                        Some(_) => {
                            if let Some(component) = original.get::<$ty>().cloned() {
                                retained.unchecked_insert_component(
                                    DataComponentKind::$kind,
                                    Some(DataComponentUnion::from(component)),
                                );
                                kept += 1;
                            }
                        }
                        None => {
                            retained.unchecked_insert_component(DataComponentKind::$kind, None);
                            kept += 1;
                        }
                    }
                }
            }
        };
    }

    keep_component!(CustomData, CustomData);
    keep_component!(MaxStackSize, MaxStackSize);
    keep_component!(MaxDamage, MaxDamage);
    keep_component!(Damage, Damage);
    keep_component!(Unbreakable, Unbreakable);
    keep_component!(CustomName, CustomName);
    keep_component!(ItemName, ItemName);
    keep_component!(Rarity, Rarity);
    keep_component!(Enchantments, Enchantments);
    keep_component!(RepairCost, RepairCost);
    keep_component!(CreativeSlotLock, CreativeSlotLock);
    keep_component!(EnchantmentGlintOverride, EnchantmentGlintOverride);
    keep_component!(IntangibleProjectile, IntangibleProjectile);
    if let Some((_, value)) = original
        .iter()
        .find(|(kind, _)| *kind == DataComponentKind::Tool)
        && value.is_none()
    {
        unsafe {
            retained.unchecked_insert_component(DataComponentKind::Tool, None);
        }
        kept += 1;
    }

    let total = original.iter().count();
    if kept != total {
        tracing::warn!(
            dropped = total - kept,
            "Dropping unsupported creative item components before protocol-766 encoding"
        );
    }
    data.component_patch = retained;
}

/// Removes protocol-766 mining bookkeeping before a creative stack can be
/// serialized back to the server. Real server custom_data survives unchanged.
fn scrub_legacy_mining_metadata(data: &mut ItemStackData) {
    let Some(custom) = data.component_patch.get::<CustomData>().cloned() else {
        return;
    };
    let simdnbt::owned::Nbt::Some(root) = custom.nbt else {
        return;
    };
    let mut compound = root.as_compound();
    let removed = compound.remove(LEGACY_EFFICIENCY_KEY).is_some()
        | compound.remove(LEGACY_AQUA_AFFINITY_KEY).is_some();
    if !removed {
        return;
    }

    // SAFETY: the union payload matches DataComponentKind::CustomData.
    unsafe {
        if compound.is_empty() {
            data.component_patch
                .unchecked_insert_component(DataComponentKind::CustomData, None);
        } else {
            let custom = CustomData {
                nbt: simdnbt::owned::Nbt::new("".into(), compound),
            };
            data.component_patch.unchecked_insert_component(
                DataComponentKind::CustomData,
                Some(DataComponentUnion::from(custom)),
            );
        }
    }
}

/// azalea's typed encoder always writes native-version component-type ids,
/// and `DataComponentPatch` is opaque (single entries can't be rewritten or
/// removed), so a patch touching any component the target version numbers
/// differently is cleared wholesale rather than sent misencoded.
fn strip_untranslatable_components(remaps: &RegistryRemaps, data: &mut ItemStackData) {
    let translates = |kind: DataComponentKind| {
        remaps.remap(ClientRegistry::DataComponentType, kind.to_u32()) == Some(kind.to_u32())
    };
    if !data
        .component_patch
        .iter()
        .all(|(kind, _)| translates(kind))
    {
        tracing::warn!("Dropping creative item components the wire version numbers differently");
        data.component_patch = Default::default();
    }
}

/// Remaps a stack's item kind, clearing the stack when the target version
/// has no such item.
fn remap_stack(remaps: &RegistryRemaps, stack: &mut ItemStack) {
    let cleared = match stack {
        ItemStack::Present(data) => !remap_with(remaps, ClientRegistry::Item, &mut data.kind),
        ItemStack::Empty => false,
    };
    if cleared {
        *stack = ItemStack::Empty;
    }
}

/// Rewrites `set_player_team` from the pre-26.2 `Parameters` layout
/// (`displayName, options, visibility, collision, color, prefix, suffix`
/// with color as a `ChatFormatting` ordinal) to the 26.2 one
/// (`displayName, prefix, suffix, visibility, collision, color, options`
/// with color as `Optional<TeamColor>`); the surrounding name/method/
/// player-list fields are copied verbatim. `string_scopes` marks the
/// pre-1.21.5 layout where nametag visibility and collision rule are
/// strings rather than enum ids.
fn translate_team(id: u32, payload: &[u8], string_scopes: bool) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    skip_utf(&mut cur)?; // team name
    let method_at = cur.position() as usize;
    let method = *payload.get(method_at)?;
    advance(&mut cur, 1)?;

    let mut out = Vec::with_capacity(payload.len() + 3);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..method_at + 1]);

    // Methods 0 (add) and 2 (change) carry Parameters.
    if method == 0 || method == 2 {
        let display = nbt_span(&mut cur)?;
        let options_at = cur.position() as usize;
        let options = *payload.get(options_at)?;
        advance(&mut cur, 1)?;
        let visibility = read_scope(&mut cur, string_scopes)?;
        let collision = read_scope(&mut cur, string_scopes)?;
        let color = u32::azalea_read_var(&mut cur).ok()?;
        let prefix = nbt_span(&mut cur)?;
        let suffix = nbt_span(&mut cur)?;

        out.extend_from_slice(&payload[display]);
        out.extend_from_slice(&payload[prefix]);
        out.extend_from_slice(&payload[suffix]);
        out.push(visibility);
        out.push(collision);
        // Vanilla 26.2 changed color from a ChatFormatting ordinal to
        // Optional<TeamColor>, but azalea still decodes the plain ordinal,
        // and these frames feed azalea — copy it through unchanged (all
        // ordinals fit one varint byte). See the team tests in
        // azalea_compat.
        // TODO: write the Optional<TeamColor> form once pomme owns the
        // decoder (see the azalea-divergence list).
        out.push(color as u8);
        out.push(options);
    }

    // Player list (methods 0/3/4) and anything after: verbatim.
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// One team scope (nametag visibility / collision rule) as its enum id:
/// read directly, or mapped from the pre-1.21.5 name (`Team.Visibility`/
/// `CollisionRule` in the 1.21.5 reference; unknown names fall back to
/// ALWAYS like vanilla's byName).
fn read_scope(cur: &mut Cursor<&[u8]>, string_scopes: bool) -> Option<u8> {
    if !string_scopes {
        return Some(u32::azalea_read_var(cur).ok()? as u8);
    }
    let len = u32::azalea_read_var(cur).ok()? as usize;
    let start = cur.position() as usize;
    advance(cur, len)?;
    Some(
        match std::str::from_utf8(&cur.get_ref()[start..start + len]).ok()? {
            "never" => 1,
            "hideForOtherTeams" | "pushOtherTeams" => 2,
            "hideForOwnTeam" | "pushOwnTeam" => 3,
            _ => 0,
        },
    )
}

fn advance(cur: &mut Cursor<&[u8]>, n: usize) -> Option<()> {
    let end = cur.position().checked_add(n as u64)?;
    if end > cur.get_ref().len() as u64 {
        return None;
    }
    cur.set_position(end);
    Some(())
}

/// Reads a 26.3 `VecDelta` (`ClientboundMoveEntityPacket`) for `step_count`
/// steps — zero is one plain `xa/ya/za` short triple, otherwise `ticks, xa,
/// ya, za` per step — and collapses it to the end displacement 26.2 carries.
/// Each step's shorts are relative to the previous step's position
/// (`VecDelta.Stepped.decode` advances the codec base per step), so the sum
/// is exact; a sum past the short range saturates until the next
/// `entity_position_sync`. The tick offsets shape only the intermediate
/// positions, which 26.2's readers never had.
/// TODO: keep the per-step path once entities interpolate along one.
fn collapse_vec_delta(cur: &mut Cursor<&[u8]>, step_count: u32) -> Option<[i16; 3]> {
    let mut sum = [0i32; 3];
    for _ in 0..step_count.max(1) {
        if step_count > 0 {
            varint_span(cur)?; // ticks
        }
        for c in &mut sum {
            *c += i32::from(read_u16(cur)? as i16);
        }
    }
    Some(sum.map(|c| c.clamp(i16::MIN.into(), i16::MAX.into()) as i16))
}

/// Rewrites `move_entity_pos` / `move_entity_pos_rot`: 26.3 packs onGround
/// (bit 0) and the step count (the rest) into a `properties` varint after
/// the entity id, followed by a `VecDelta`; 26.2 reads three shorts, the
/// rotation bytes if any, and a trailing onGround bool.
fn translate_move_entity_777(id: u32, payload: &[u8], rotation: bool) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let properties = u32::azalea_read_var(&mut cur).ok()?;
    let delta = collapse_vec_delta(&mut cur, properties >> 1)?;
    let rot_at = cur.position() as usize;
    if rotation {
        advance(&mut cur, 2)?;
    }

    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    for c in delta {
        out.extend_from_slice(&c.to_be_bytes());
    }
    out.extend_from_slice(&payload[rot_at..cur.position() as usize]);
    out.push((properties & 1) as u8);
    Some(out)
}

fn translate_move_entity_pos_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    translate_move_entity_777(id, payload, false)
}

fn translate_move_entity_pos_rot_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    translate_move_entity_777(id, payload, true)
}

/// Rewrites `move_entity_rot`: 26.3 moved onGround ahead of the rotation
/// bytes.
fn translate_move_entity_rot_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let on_ground = read_u8(&mut cur)?;
    let rot_at = cur.position() as usize;
    advance(&mut cur, 2)?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.extend_from_slice(&payload[rot_at..rot_at + 2]);
    out.push(on_ground);
    Some(out)
}

/// Advances past a 26.3 `PositionPath` (varint tag: 0 = one `Vec3`, 1 = a
/// counted list of `Vec3` + tick-offset varint steps), returning the 24
/// position bytes it ends at.
fn read_position_path_end<'a>(cur: &mut Cursor<&'a [u8]>) -> Option<&'a [u8]> {
    let stepped = match u32::azalea_read_var(cur).ok()? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let steps = if stepped {
        u32::azalea_read_var(cur).ok()?
    } else {
        1
    };
    let mut end = None;
    for _ in 0..steps {
        let at = cur.position() as usize;
        advance(cur, 24)?;
        end = Some(at);
        if stepped {
            varint_span(cur)?; // tick offset
        }
    }
    let data: &'a [u8] = cur.get_ref();
    end.map(|at| &data[at..at + 24])
}

/// Rewrites `entity_position_sync`: 26.3 replaced the position and delta
/// doubles with a `PositionPath`. The delta is synthesized as zero; the
/// handler leaves velocity alone for this packet.
fn translate_entity_position_sync_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let end = read_position_path_end(&mut cur)?;
    let tail_at = cur.position() as usize;
    advance(&mut cur, 9)?; // yRot, xRot, onGround

    let mut out = Vec::with_capacity(payload.len() + 24);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[entity]);
    out.extend_from_slice(end);
    out.extend_from_slice(&[0; 24]); // zero delta movement
    out.extend_from_slice(&payload[tail_at..]);
    Some(out)
}

/// Rewrites `explode` from 26.3: the particle and sound ids (the weighted
/// block-particle list's too) move into native space, and the trailing
/// `playSound` bool 26.3 appended is dropped.
/// TODO: honour a false `playSound` once explosions render.
fn translate_explode_777(
    id: u32,
    payload: &[u8],
    ids: &GameIds,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 32)?; // center, radius, block count
    skip_optional(&mut cur, |c| advance(c, 24))?; // player knockback
    let head_end = cur.position() as usize;

    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..head_end]);
    translate_particles(&mut cur, &mut out, ids, remaps, false)?;
    translate_sound_holder(&mut cur, &mut out, remaps)?;
    let entries = u32::azalea_read_var(&mut cur).ok()?;
    wire::write_varint(&mut out, entries);
    for _ in 0..entries {
        translate_particles(&mut cur, &mut out, ids, remaps, false)?;
        let scaling_at = cur.position() as usize;
        advance(&mut cur, 8)?; // scaling, speed
        let weight = varint_span(&mut cur)?;
        out.extend_from_slice(&payload[scaling_at..weight.end]);
    }
    Some(out)
}

/// Rewrites the `previousGameType` of a `CommonPlayerSpawnInfo` starting at
/// `spawn_info_at`: 26.3 sends an optional varint (0 = none, else id + 1)
/// where 26.2 reads a signed byte (-1 = none). `gameType` before it went
/// from a byte to a varint, byte-identical for its four values.
fn rewrite_previous_game_type(id: u32, payload: &[u8], spawn_info_at: usize) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    cur.set_position(spawn_info_at as u64);
    varint_span(&mut cur)?; // dimension type
    skip_utf(&mut cur)?; // dimension
    advance(&mut cur, 8)?; // seed
    varint_span(&mut cur)?; // game type
    let previous_at = cur.position() as usize;
    let previous = u32::azalea_read_var(&mut cur).ok()?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..previous_at]);
    out.push(previous.checked_sub(1).map_or(0xFF, |g| g as u8));
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// `login` carries its spawn info after the player id, hardcore flag, level
/// list, three varints and three bools.
fn translate_login_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 5)?; // player id, hardcore
    let levels = u32::azalea_read_var(&mut cur).ok()?;
    for _ in 0..levels {
        skip_utf(&mut cur)?;
    }
    for _ in 0..3 {
        varint_span(&mut cur)?; // max players, chunk radius, simulation distance
    }
    advance(&mut cur, 3)?; // reduced debug info, death screen, limited crafting
    rewrite_previous_game_type(id, payload, cur.position() as usize)
}

fn translate_respawn_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    rewrite_previous_game_type(id, payload, 0)
}

/// Rewrites `animate`: 26.3 moved the swings to `swing_animation` (see
/// [`translate_swing_animation_777`]) and renumbered the rest — wake up 2 ->
/// 0, critical hit 4 -> 1, magic critical hit 5 -> 2.
fn translate_animate_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let (&action, entity) = payload.split_last()?;
    let action = match action {
        0 => 2,
        1 => 4,
        2 => 5,
        _ => return None,
    };
    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(entity);
    out.push(action);
    Some(out)
}

/// Rewrites 26.3's `swing_animation` (`entityId, hand, {type, duration}`)
/// onto `animate` with the swing action 26.2 keyed by hand (0 main, 3 off).
/// A `none` type animates nothing and drops.
fn translate_swing_animation_777(animate_id: u32, payload: &[u8]) -> Option<Box<[u8]>> {
    let mut cur = Cursor::new(payload);
    let entity = varint_span(&mut cur)?;
    let hand = u32::azalea_read_var(&mut cur).ok()?;
    if u32::azalea_read_var(&mut cur).ok()? == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(entity.end + 2);
    wire::write_varint(&mut out, animate_id);
    out.extend_from_slice(&payload[entity]);
    out.push(if hand == 0 { 0 } else { 3 });
    Some(out.into_boxed_slice())
}

/// Rewrites `level_particles`: 26.3 leads with the particle, splits
/// `maxSpeed` per axis, sends `count` as a varint and appends a
/// randomization type; 26.2 ends with the particle and reads one speed (the
/// x axis here — vanilla servers send one value on all three) and an int
/// count. The particle keeps its wire-space id, remapped by the raw handler
/// like every version's, so its payload is only sized, never rewritten.
/// TODO: per-axis speed and the randomization type once particles use them.
fn translate_level_particles_777(
    id: u32,
    payload: &[u8],
    v: &Ids777,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let particle = u32::azalea_read_var(&mut cur).ok()?;
    let name = v
        .wire_registries
        .name_of(ClientRegistry::ParticleType, particle)?;
    if PAYLOAD_PARTICLES.contains(&name) {
        copy_particle_payload(&mut cur, &mut Vec::new(), name, remaps)?;
    }
    let particle_end = cur.position() as usize;
    advance(&mut cur, 38)?; // flags, position, spread
    let speed_at = cur.position() as usize;
    advance(&mut cur, 12)?;
    let count = u32::azalea_read_var(&mut cur).ok()?;
    varint_span(&mut cur)?; // randomization type

    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[particle_end..speed_at + 4]);
    out.extend_from_slice(&(count as i32).to_be_bytes());
    out.extend_from_slice(&payload[..particle_end]);
    Some(out)
}

/// Rewrites `update_advancements`: 26.3 moved an advancement's screen
/// position (two floats) out of `DisplayInfo` to a pair after each entry,
/// sent even without a display. 26.2 reads the pair at the end of the
/// display, so each entry is walked to put it back or drop it; the rest of
/// the packet copies verbatim. Item ids stay in wire space like every
/// version's.
/// TODO: remap advancement icon items in `remap_inbound`.
fn translate_update_advancements_777(
    id: u32,
    payload: &[u8],
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 1)?; // reset
    let count = u32::azalea_read_var(&mut cur).ok()?;
    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..count {
        let entry_at = cur.position() as usize;
        skip_utf(&mut cur)?; // id
        skip_optional(&mut cur, skip_utf)?; // parent
        let display_end = if read_u8(&mut cur)? != 0 {
            skip_display_info(&mut cur, remaps)?;
            Some(cur.position() as usize)
        } else {
            None
        };
        for _ in 0..u32::azalea_read_var(&mut cur).ok()? {
            for _ in 0..u32::azalea_read_var(&mut cur).ok()? {
                skip_utf(&mut cur)?; // requirement
            }
        }
        advance(&mut cur, 1)?; // sends telemetry
        let entry_end = cur.position() as usize;
        advance(&mut cur, 8)?; // x, y
        match display_end {
            Some(end) => {
                out.extend_from_slice(&payload[entry_at..end]);
                out.extend_from_slice(&payload[entry_end..entry_end + 8]);
                out.extend_from_slice(&payload[end..entry_end]);
            }
            None => out.extend_from_slice(&payload[entry_at..entry_end]),
        }
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Advances past a `DisplayInfo` body: two NBT components, an
/// `ItemStackTemplate` (item, count, component patch), a varint type, an
/// i32 flag set and a background identifier when its bit 0 is set.
fn skip_display_info(cur: &mut Cursor<&[u8]>, remaps: &RegistryRemaps) -> Option<()> {
    skip_nbt(cur)?; // title
    skip_nbt(cur)?; // description
    varint_span(cur)?; // item
    varint_span(cur)?; // count
    copy_component_patch(cur, &mut Vec::new(), remaps)?;
    varint_span(cur)?; // type
    let flags = read_i32(cur)?;
    if flags & 1 != 0 {
        skip_utf(cur)?; // background
    }
    Some(())
}

fn read_f64s<const N: usize>(cur: &mut Cursor<&[u8]>) -> Option<[f64; N]> {
    let mut v = [0.0; N];
    for c in &mut v {
        let at = cur.position() as usize;
        advance(cur, 8)?;
        *c = f64::from_be_bytes(cur.get_ref()[at..at + 8].try_into().ok()?);
    }
    Some(v)
}

/// Holds a 26.3 `accept_teleportation` back: 26.3 appends the accepted pose
/// and no longer follows it with a `move_player_pos_rot`
/// (`ClientPacketListener.handleMovePlayer`), whose pose pomme sends right
/// after the accept.
fn translate_accept_teleportation_777(v: &Ids777, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut p = 0;
    *v.pending_accept.lock().unwrap() = wire::read_varint(payload, &mut p);
    Vec::new()
}

/// Folds the `move_player_pos_rot` following a held-back accept into it
/// (`id, x, y, z, yRot, xRot`); `None` for an ordinary move.
fn translate_move_player_pos_rot_777(v: &Ids777, payload: &[u8]) -> Option<Vec<Vec<u8>>> {
    let teleport = v.pending_accept.lock().unwrap().take()?;
    let mut out = Vec::with_capacity(38);
    wire::write_varint(&mut out, v.accept_teleportation_old_id);
    wire::write_varint(&mut out, teleport);
    out.extend_from_slice(payload.get(..32)?); // x, y, z, yRot, xRot
    Some(vec![out])
}

/// Rewrites `swing` onto 26.3's payload-less `punch`, which is main-hand
/// only; an off-hand swing has no equivalent and is dropped.
fn translate_swing_777(punch_old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut p = 0;
    if wire::read_varint(payload, &mut p) != Some(0) {
        tracing::debug!("Suppressing an off-hand swing 26.3 can't express");
        return Vec::new();
    }
    let mut out = Vec::with_capacity(1);
    wire::write_varint(&mut out, punch_old_id);
    vec![out]
}

/// Rewrites `player_action` for 26.3, which inserted `CHANGE_DESTROY_DIRECTION`
/// at action 1, shifting every later action up by one.
fn translate_player_action_777(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let mut p = 0;
    let Some(action) = wire::read_varint(payload, &mut p) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, old_id);
    wire::write_varint(&mut out, action + u32::from(action >= 1));
    out.extend_from_slice(&payload[p..]);
    vec![out]
}

/// Rewrites `player_chat` from 26.3, whose `PARTIALLY_FILTERED` mask moved
/// to `ByteBufCodecs.BIT_SET` (`FilterMask`); everything around it copies
/// verbatim.
fn translate_player_chat_777(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // global index
    advance(&mut cur, 16)?; // sender
    varint_span(&mut cur)?; // index
    skip_optional(&mut cur, |c| advance(c, 256))?; // signature
    skip_utf(&mut cur)?; // content
    advance(&mut cur, 16)?; // timestamp, salt
    for _ in 0..u32::azalea_read_var(&mut cur).ok()? {
        // A last-seen entry: cache id + 1, or 0 and the full signature.
        if u32::azalea_read_var(&mut cur).ok()? == 0 {
            advance(&mut cur, 256)?;
        }
    }
    skip_optional(&mut cur, skip_nbt)?; // unsigned content
    let mask = u32::azalea_read_var(&mut cur).ok()?;

    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    if mask == 2 {
        repack_bit_set(&mut cur, &mut out)?;
    }
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Rewrites `commands` from 26.3, whose argument-type registry inserted five
/// singleton parsers (`ArgumentTypeInfos`): every parser id is remapped by
/// name, and a 26.3-only parser becomes the one a 26.2 server sent for the
/// same argument. Node and property layouts are unchanged
/// (`ClientboundCommandsPacket`), so node indices never move.
fn translate_commands_777(
    id: u32,
    payload: &[u8],
    v: &Ids777,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 32);
    wire::write_varint(&mut out, id);
    for _ in 0..copy_varint(&mut cur, &mut out)? {
        let flags = read_u8(&mut cur)?;
        out.push(flags);
        for _ in 0..copy_varint(&mut cur, &mut out)? {
            copy_varint(&mut cur, &mut out)?; // child
        }
        if flags & 0x08 != 0 {
            copy_varint(&mut cur, &mut out)?; // redirect
        }
        let node_type = flags & 0x03;
        if node_type == 1 || node_type == 2 {
            copy_utf(&mut cur, &mut out)?; // name
        }
        if node_type == 2 {
            let parser = u32::azalea_read_var(&mut cur).ok()?;
            let name = v
                .wire_registries
                .name_of(ClientRegistry::CommandArgumentType, parser)?;
            match remaps.remap(ClientRegistry::CommandArgumentType, parser) {
                Some(native) => {
                    wire::write_varint(&mut out, native);
                    let at = cur.position() as usize;
                    skip_parser_properties(&mut cur, name)?;
                    out.extend_from_slice(&payload[at..cur.position() as usize]);
                }
                None => write_substitute_parser(&mut out, name)?,
            }
            if flags & 0x10 != 0 {
                copy_utf(&mut cur, &mut out)?; // suggestions
            }
        }
    }
    copy_varint(&mut cur, &mut out)?; // root index
    Some(out)
}

/// Advances past an argument parser's properties (`ArgumentTypeInfos`
/// serializers; unchanged between 26.2 and 26.3).
fn skip_parser_properties(cur: &mut Cursor<&[u8]>, parser: &str) -> Option<()> {
    let bound = match parser {
        "brigadier:float" | "brigadier:integer" => 4,
        "brigadier:double" | "brigadier:long" => 8,
        "brigadier:string" => return varint_span(cur).map(drop),
        "entity" | "score_holder" => return advance(cur, 1),
        "time" => return advance(cur, 4),
        "resource_or_tag"
        | "resource_or_tag_key"
        | "resource"
        | "resource_key"
        | "resource_selector" => return skip_utf(cur),
        _ => return Some(()),
    };
    // Number bounds: a flag byte (0x01 min, 0x02 max), then the set bounds.
    let flags = read_u8(cur)?;
    for bit in [1, 2] {
        if flags & bit != 0 {
            advance(cur, bound)?;
        }
    }
    Some(())
}

/// Writes the native parser standing in for a 26.3-only one, as a 26.2
/// server sent the same argument: `/place feature` read a `resource_key`
/// over configured features (26.2 `PlaceCommand`), slot sources item slots,
/// and the swing animation (an unquoted `none`/`whack`/`stab`) a single
/// word. The number providers take an id or inline SNBT; a resource
/// location covers the id form.
fn write_substitute_parser(out: &mut Vec<u8>, parser: &str) -> Option<()> {
    let native = match parser {
        "feature" => "resource_key",
        "slot_source" => "item_slots",
        "swing_animation" => "brigadier:string",
        "context_float_provider" | "context_int_provider" => "resource_location",
        _ => return None,
    };
    let native = RegistryTable::native().id_of(ClientRegistry::CommandArgumentType, native)?;
    wire::write_varint(out, native);
    match parser {
        "feature" => {
            let registry = "minecraft:worldgen/configured_feature";
            wire::write_varint(out, registry.len() as u32);
            out.extend_from_slice(registry.as_bytes());
        }
        "swing_animation" => wire::write_varint(out, 0), // SINGLE_WORD
        _ => {}
    }
    Some(())
}

/// Copies one item `HolderSet` (`ByteBufCodecs.holderSet`: 0 then a tag
/// id, or count + 1 then that many item ids).
fn copy_holder_set(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    match copy_varint(cur, out)? {
        0 => copy_utf(cur, out),
        n => (1..n).try_for_each(|_| copy_varint(cur, out).map(drop)),
    }
}

/// Copies one 26.3 `SlotDisplay`, recursively. Only `tag` changed: 26.3
/// sends a `HolderSet` where 26.2 sent the tag id, and 26.3's
/// `Ingredient.display()` routes direct item lists through it too, which
/// 26.2 sent as a `composite` of `item` displays. The other types' layouts
/// are unchanged (`SlotDisplay`); item ids stay in wire space.
/// TODO: remap recipe-display item ids once `remap_inbound` covers recipes.
fn copy_slot_display_777(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    wire_registries: &RegistryTable,
    remaps: &RegistryRemaps,
) -> Option<()> {
    use ClientRegistry::SlotDisplay;
    let kind = u32::azalea_read_var(cur).ok()?;
    let name = wire_registries.name_of(SlotDisplay, kind)?;
    if name == "tag" {
        let set = u32::azalea_read_var(cur).ok()?;
        if set > 0 {
            let native = RegistryTable::native();
            let item = native.id_of(SlotDisplay, "item")?;
            wire::write_varint(out, native.id_of(SlotDisplay, "composite")?);
            wire::write_varint(out, set - 1);
            for _ in 1..set {
                wire::write_varint(out, item);
                copy_varint(cur, out)?;
            }
            return Some(());
        }
    }
    wire::write_varint(out, remaps.remap(SlotDisplay, kind)?);
    // Nested displays lead every type that has any.
    let nested = match name {
        "with_any_potion" | "only_with_component" => 1,
        "dyed" | "with_remainder" | "smithing_trim" => 2,
        "composite" => copy_varint(cur, out)?,
        _ => 0,
    };
    for _ in 0..nested {
        copy_slot_display_777(cur, out, wire_registries, remaps)?;
    }
    match name {
        "empty" | "any_fuel" | "with_any_potion" | "dyed" | "with_remainder" | "composite" => {}
        "tag" => copy_utf(cur, out)?,
        "item" => {
            copy_varint(cur, out)?;
        }
        "item_stack" => {
            copy_varint(cur, out)?; // item
            copy_varint(cur, out)?; // count
            copy_component_patch(cur, out, remaps)?;
        }
        "only_with_component" => {
            let component = u32::azalea_read_var(cur).ok()?;
            let native = remaps.remap(ClientRegistry::DataComponentType, component)?;
            wire::write_varint(out, native);
        }
        "smithing_trim" => {
            // The pattern holder: id + 1, or 0 and an inline pattern.
            if copy_varint(cur, out)? == 0 {
                copy_utf(cur, out)?; // asset id
                copy_nbt(cur, out)?; // description
                copy_bytes(cur, out, 1)?; // decal
            }
        }
        _ => return None,
    }
    Some(())
}

/// Copies one 26.3 `RecipeDisplay`, rewriting its slot displays
/// (`*RecipeDisplay` stream codecs, unchanged from 26.2).
fn copy_recipe_display_777(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    wire_registries: &RegistryTable,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let kind = u32::azalea_read_var(cur).ok()?;
    let name = wire_registries.name_of(ClientRegistry::RecipeDisplay, kind)?;
    wire::write_varint(out, remaps.remap(ClientRegistry::RecipeDisplay, kind)?);
    let slots = |cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, n: u32| {
        (0..n).try_for_each(|_| copy_slot_display_777(cur, out, wire_registries, remaps))
    };
    match name {
        // Ingredients, then the result and crafting station.
        "crafting_shapeless" => {
            let ingredients = copy_varint(cur, out)?;
            slots(cur, out, ingredients + 2)
        }
        "crafting_shaped" => {
            copy_varint(cur, out)?; // width
            copy_varint(cur, out)?; // height
            let ingredients = copy_varint(cur, out)?;
            slots(cur, out, ingredients + 2)
        }
        "furnace" => {
            slots(cur, out, 4)?; // ingredient, fuel, result, station
            copy_varint(cur, out)?; // duration
            copy_bytes(cur, out, 4) // experience
        }
        "stonecutter" => slots(cur, out, 3),
        "smithing" => slots(cur, out, 5),
        _ => None,
    }
}

/// Rewrites `recipe_book_add` from 26.3: each entry's display is rewritten
/// (see [`copy_slot_display_777`]); the ingredient `HolderSet`s of its
/// crafting requirements kept their codec and copy through.
fn translate_recipe_book_add_777(
    id: u32,
    payload: &[u8],
    v: &Ids777,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 64);
    wire::write_varint(&mut out, id);
    for _ in 0..copy_varint(&mut cur, &mut out)? {
        copy_varint(&mut cur, &mut out)?; // display id
        copy_recipe_display_777(&mut cur, &mut out, v.wire_registries, remaps)?;
        copy_varint(&mut cur, &mut out)?; // group (optional varint)
        copy_varint(&mut cur, &mut out)?; // category
        copy_optional(&mut cur, &mut out, |cur, out| {
            (0..copy_varint(cur, out)?).try_for_each(|_| copy_holder_set(cur, out))
        })?;
        copy_bytes(&mut cur, &mut out, 1)?; // flags
    }
    copy_bytes(&mut cur, &mut out, 1)?; // replace
    Some(out)
}

/// Rewrites `place_ghost_recipe` from 26.3: container id, then a display.
fn translate_place_ghost_recipe_777(
    id: u32,
    payload: &[u8],
    v: &Ids777,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 32);
    wire::write_varint(&mut out, id);
    copy_varint(&mut cur, &mut out)?;
    copy_recipe_display_777(&mut cur, &mut out, v.wire_registries, remaps)?;
    Some(out)
}

/// Rewrites `update_recipes` from 26.3: the property sets copy through, and
/// each stonecutter entry's option display is rewritten after its
/// ingredient `HolderSet` (`SelectableRecipe.SingleInputEntry`).
fn translate_update_recipes_777(
    id: u32,
    payload: &[u8],
    v: &Ids777,
    remaps: &RegistryRemaps,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    let mut out = Vec::with_capacity(payload.len() + 64);
    wire::write_varint(&mut out, id);
    for _ in 0..copy_varint(&mut cur, &mut out)? {
        copy_utf(&mut cur, &mut out)?; // property set key
        for _ in 0..copy_varint(&mut cur, &mut out)? {
            copy_varint(&mut cur, &mut out)?; // item
        }
    }
    for _ in 0..copy_varint(&mut cur, &mut out)? {
        copy_holder_set(&mut cur, &mut out)?;
        copy_slot_display_777(&mut cur, &mut out, v.wire_registries, remaps)?;
    }
    Some(out)
}

/// The byte range of one varint, advancing past it.
fn varint_span(cur: &mut Cursor<&[u8]>) -> Option<std::ops::Range<usize>> {
    let start = cur.position() as usize;
    u32::azalea_read_var(cur).ok()?;
    Some(start..cur.position() as usize)
}

/// The byte range of one network-NBT value (type byte + unnamed payload),
/// advancing past it.
fn nbt_span(cur: &mut Cursor<&[u8]>) -> Option<std::ops::Range<usize>> {
    let start = cur.position() as usize;
    skip_nbt(cur)?;
    Some(start..cur.position() as usize)
}

fn read_u8(cur: &mut Cursor<&[u8]>) -> Option<u8> {
    let b = *cur.get_ref().get(cur.position() as usize)?;
    cur.set_position(cur.position() + 1);
    Some(b)
}

fn read_u16(cur: &mut Cursor<&[u8]>) -> Option<u16> {
    Some(u16::from_be_bytes([read_u8(cur)?, read_u8(cur)?]))
}

fn read_i32(cur: &mut Cursor<&[u8]>) -> Option<i32> {
    let b = [read_u8(cur)?, read_u8(cur)?, read_u8(cur)?, read_u8(cur)?];
    Some(i32::from_be_bytes(b))
}

/// Skips one NBT payload of the given tag type (vanilla `TagTypes` wire
/// layout). Named tags only appear inside compounds; the depth cap matches
/// vanilla's nesting limit.
fn skip_nbt_payload(cur: &mut Cursor<&[u8]>, tag: u8, depth: u32) -> Option<()> {
    const MAX_DEPTH: u32 = 512;
    if depth > MAX_DEPTH {
        return None;
    }
    match tag {
        0 => Some(()),        // End (empty root / list of End)
        1 => advance(cur, 1), // Byte
        2 => advance(cur, 2), // Short
        3 => advance(cur, 4), // Int
        4 => advance(cur, 8), // Long
        5 => advance(cur, 4), // Float
        6 => advance(cur, 8), // Double
        7 => {
            let n = read_i32(cur)?;
            advance(cur, usize::try_from(n).ok()?)
        }
        8 => {
            let n = read_u16(cur)?;
            advance(cur, n as usize)
        }
        9 => {
            let elem = read_u8(cur)?;
            let n = read_i32(cur)?;
            for _ in 0..n.max(0) {
                skip_nbt_payload(cur, elem, depth + 1)?;
            }
            Some(())
        }
        10 => loop {
            let elem = read_u8(cur)?;
            if elem == 0 {
                return Some(());
            }
            let name_len = read_u16(cur)?;
            advance(cur, name_len as usize)?;
            skip_nbt_payload(cur, elem, depth + 1)?;
        },
        11 => {
            let n = read_i32(cur)?;
            advance(cur, usize::try_from(n).ok()?.checked_mul(4)?)
        }
        12 => {
            let n = read_i32(cur)?;
            advance(cur, usize::try_from(n).ok()?.checked_mul(8)?)
        }
        _ => None,
    }
}
