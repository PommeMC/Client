//! Wire translation for older protocol versions.
//!
//! The client speaks the latest supported version (26.2) internally. When a
//! connection negotiates an older wire version, inbound frames are rewritten
//! into the latest layout before azalea's typed decode where the wire format
//! changed, and static-registry ids (which shift between versions) are
//! remapped in both directions so the rest of the client stays
//! single-version. Layouts were line-checked against the decompiled
//! references (`reference/<version>/decompiled/.../network/protocol/`).
//!
//! 26.1 -> 26.2 wire changes:
//! - login `login_finished` gained a trailing session-id UUID
//! - game `login` gained an `onlineMode` bool before the trailing
//!   `enforcesSecureChat` bool
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
//!   split `pick_item` into the from_block/from_entity pair; pomme sends
//!   neither
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
//! - `AbstractArrow` gained `in_ground` at index 10, so an arrow's effect color
//!   sits at 10 where 26.2 reads 11, and a trident's loyalty/foil at 10/11
//!   where 26.2 reads 11/12. The indices pass through unshifted, which costs
//!   nothing today because pomme reads metadata only for living entities; a
//!   lift belongs beside `normalize_player_index_at` in `entity/mod.rs`, not
//!   here
//!
//! Known limitation (accepted): an inbound item stack carrying a data
//! component at/after the first id the versions number differently (26.1:
//! 78, where 26.2 inserted `sulfur_cube_content`; 1.21.11: 41, where 26.x
//! inserted `additional_trade_cost`; 1.21.10 and every older version with a
//! component registry: 5, where 1.21.11 inserted `use_effects` — so even
//! `custom_name` and `enchantments` are affected on all of them) decodes under
//! the wrong 26.2 codec — usually a misparse that skips the packet via
//! `skip_malformed_packet`, though a coincidentally parsable layout yields a
//! silently wrong component. Common survival items only use earlier, unshifted
//! components. Items nested inside component values (bundles, containers) also
//! keep their source-version ids.
//!
//! Depends on azalea diverging from 26.2: these translations are correct only
//! because azalea encodes or decodes something differently from the reference,
//! so fixing azalea — or replacing it with pomme's own codec — breaks the older
//! versions unless the matching rewrite lands at the same time. Each site
//! carries a `TODO` pointing here.
//! - inbound `set_player_team` copies the color through as a plain
//!   `ChatFormatting` ordinal, where 26.2 writes an `Optional<TeamColor>`
//! - outbound `set_creative_mode_slot` leaves component values undelimited,
//!   which is 1.21.4's layout rather than 26.2's
//! - inbound `cooldown` carries an item registry id, where 26.2 names a
//!   cooldown group

use std::io::Cursor;
use std::sync::Mutex;

use azalea_buf::{AzBuf, AzBufVar};
use azalea_core::sound::CustomSound;
use azalea_inventory::components::Profile;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_protocol::packets::game::s_container_click::HashedStack;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_registry::builtin::{DataComponentKind, SoundEvent};
use azalea_registry::{Holder, Registry};
use glam::DVec3;
use pomme_protocol::version::LATEST;
use pomme_protocol::{
    ClientRegistry, Direction, PacketTable, Phase, RegistryRemaps, RegistryTable, wire,
};

pub struct Translation {
    to_latest: &'static RegistryRemaps,
    from_latest: &'static RegistryRemaps,
    login_finished_id: u32,
    /// Whether the wire version's `login_finished` ends in the
    /// `strictErrorHandling` bool 1.21.2 dropped; see
    /// [`Translation::translate_login_frame`].
    login_strict_error_handling: bool,
    /// Latest-space; game-frame rewrites dispatch after the id remap.
    game_login_id: u32,
    set_player_team_id: u32,
    /// Game-phase packet-id translation and the rewrites tied to it; `None`
    /// when the wire version's ids match the latest (26.1).
    game_ids: Option<GameIds>,
}

/// Game-phase id tables for a wire version whose ids diverged from the
/// latest, plus the latest-space ids its frame rewrites dispatch on.
struct GameIds {
    /// Wire-version clientbound id -> latest id; `None` drops the frame
    /// (no latest equivalent — none exist for 1.21.11/1.21.10, kept for
    /// safety).
    inbound: Box<[Option<u32>]>,
    /// Latest serverbound id -> wire-version id; `None` suppresses the
    /// frame (the packet doesn't exist on the older version).
    outbound: Box<[Option<u32>]>,
    /// The wire version's `EntityDataSerializers` interleave (the
    /// registration order shifts between versions).
    serializer_map: fn(u32) -> Option<u32>,
    set_entity_data_id: u32,
    level_chunk_id: u32,
    set_time_id: u32,
    attack_id: u32,
    interact_id: u32,
    interact_old_id: u32,
    /// Whether the wire version's `entity_effect`/`tinted_leaves` particles
    /// carry a color int (1.20.5 added it); see [`translate_particles`].
    color_particles: bool,
    /// The rewrites 1.21.9 introduced, for wire versions at or below it.
    v772: Option<Ids772>,
    /// The rewrites 1.21.6 introduced, for wire versions below it.
    v770: Option<Ids770>,
    /// The rewrites 1.21.5 introduced, for wire versions below it. Its
    /// presence also flags the NBT chunk heightmaps and string team scopes.
    v769: Option<Ids769>,
    /// The rewrites 1.21.4 introduced, for wire versions below it.
    v768: Option<Ids768>,
    /// The rewrites 1.21.2 introduced, for wire version 767.
    v767: Option<Ids767>,
    /// Latest serverbound ids whose packet is knowingly absent on this wire
    /// version (`client_tick_end`, `player_loaded`); suppressed quietly.
    quiet_suppressed: Box<[u32]>,
}

/// Latest-space dispatch ids for the frame rewrites protocols at or below
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

/// Latest-space dispatch ids for the frame rewrites protocol 769 needs.
struct Ids769 {
    player_chat_id: u32,
    update_advancements_id: u32,
    /// Latest-space serverbound `container_click` id and the wire
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

/// Latest-space dispatch ids for the frame rewrites only protocols at or
/// below 772 need (the layouts 1.21.9 changed). Its presence also flags the
/// pre-1.21.9 entity-data serializer set and `profile` component layout.
struct Ids772 {
    add_entity_id: u32,
    set_entity_motion_id: u32,
    player_rotation_id: u32,
    set_default_spawn_id: u32,
    explode_id: u32,
}

/// A version-specific frame rewriter: latest-space id + payload to the
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

/// Protocols the wire translation fully covers. A version with embedded
/// tables but no entry here (the staging state while its translation is
/// built) pings with the right version but stays un-joinable.
const TRANSLATED: &[i32] = &[775, 774, 773, 772, 771, 770, 769, 768, 767];

/// Whether a server speaking `protocol` can be joined: the native latest
/// version, or an older one with a complete wire translation. Gates both
/// wire-version negotiation and the server list's compatibility marker.
pub fn joinable(protocol: i32) -> bool {
    protocol == LATEST.protocol || TRANSLATED.contains(&protocol)
}

/// The translation for the wire version negotiated with the current server,
/// or `None` when the client speaks it natively (the latest version, or one
/// outside `TRANSLATED`, which connects untranslated as before).
pub fn active() -> Option<&'static Translation> {
    let protocol = crate::version::session_protocol();
    if protocol == LATEST.protocol {
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
        tracing::info!("Translating protocol {protocol} <-> {}", LATEST.protocol);
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
    /// embedded data alone isn't enough (and the latest version needs none).
    pub(crate) fn for_protocol(protocol: i32) -> Option<Translation> {
        if !TRANSLATED.contains(&protocol) {
            return None;
        }
        let table = PacketTable::for_protocol(protocol)?;
        let latest = PacketTable::latest();
        let id = |phase, name| required_id(latest, phase, Direction::Clientbound, name);
        Some(Translation {
            to_latest: RegistryRemaps::to_latest(protocol)?,
            from_latest: RegistryRemaps::from_latest(protocol)?,
            // Login-phase ids are identical across all supported versions.
            login_finished_id: id(Phase::Login, "login_finished"),
            login_strict_error_handling: protocol <= 767,
            game_login_id: id(Phase::Game, "login"),
            set_player_team_id: id(Phase::Game, "set_player_team"),
            game_ids: GameIds::build(protocol, table, latest),
        })
    }

    /// Rewrites a raw login-phase frame into the latest layout.
    pub fn translate_login_frame(&self, raw: Box<[u8]>) -> Box<[u8]> {
        let mut cur = Cursor::new(&raw[..]);
        if u32::azalea_read_var(&mut cur).ok() == Some(self.login_finished_id) {
            // 1.21.1 ends the packet in a `strictErrorHandling` bool 1.21.2
            // dropped, and 26.2 appended a session-id UUID in its place; a
            // zero UUID is fine, pomme only reads the game profile.
            let mut out = raw.into_vec();
            if self.login_strict_error_handling {
                out.pop();
            }
            out.extend_from_slice(&[0; 16]);
            return out.into_boxed_slice();
        }
        raw
    }

    /// Rewrites a raw game-phase frame into the latest layout; `None` drops
    /// the packet (malformed beyond repair, or without a latest equivalent).
    pub fn translate_game_frame(&self, raw: Box<[u8]>) -> Option<Box<[u8]>> {
        let mut id_end = 0;
        let wire_id = wire::read_varint(&raw, &mut id_end)?;
        let id = match &self.game_ids {
            Some(ids) => {
                let Some(latest) = ids.inbound.get(wire_id as usize).copied().flatten() else {
                    tracing::debug!("Dropping inbound game packet {wire_id} with no latest id");
                    return None;
                };
                latest
            }
            None => wire_id,
        };

        let v769 = self.game_ids.as_ref().is_some_and(|g| g.v769.is_some());
        let v767 = self.game_ids.as_ref().and_then(|g| g.v767.as_ref());
        if v767.is_some_and(|v| v.drops.contains(&id)) {
            tracing::debug!("Dropping game packet {id} with no 1.21.1 equivalent layout");
            return None;
        }
        let v767 = v767.is_some();
        let payload = &raw[id_end..];
        let rewritten = if id == self.game_login_id {
            if v767 {
                insert_sea_level(payload).and_then(|p| translate_game_login(id, &p))
            } else {
                translate_game_login(id, payload)
            }
        } else if id == self.set_player_team_id {
            translate_team(id, payload, v769)
        } else if let Some(ids) = &self.game_ids {
            if id == ids.set_entity_data_id {
                translate_entity_data(id, payload, ids, self.to_latest)
            } else if id == ids.level_chunk_id {
                translate_chunk(id, payload, v769)
            } else if id == ids.set_time_id {
                if v767 {
                    translate_set_time_767(id, payload)
                } else {
                    translate_set_time(id, payload)
                }
            } else if ids.v772.as_ref().is_some_and(|v| id == v.explode_id) {
                translate_explode(id, payload, ids, self.to_latest)
            } else if let Some(rewrite) = ids.version_rewrite(id) {
                rewrite(id, payload)
            } else if let Some(v) = ids.v767.as_ref().filter(|v| id == v.teleport_entity_id) {
                translate_teleport_entity_767(v, payload)
            } else if let Some(v) = ids.v767.as_ref().filter(|v| id == v.container_set_slot_id) {
                translate_container_set_slot_767(v, payload)
            } else if ids.v767.as_ref().is_some_and(|v| id == v.cooldown_id) {
                translate_cooldown_767(self.to_latest, id, payload)
            } else if id == wire_id {
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

    /// Whether outbound game frames need translation before hitting the
    /// wire (the version's serverbound ids or layouts diverge from latest).
    pub fn translates_outbound(&self) -> bool {
        self.game_ids.is_some()
    }

    /// Translates a latest-layout serverbound game frame into the wire
    /// version's: id remap, `attack`/`interact` layout rewrites, and
    /// suppression of packets the older version lacks. Returns the frames to
    /// send (empty = suppressed, two for `interact`, one otherwise).
    pub fn translate_outbound_game_frame(&self, mut frame: Vec<u8>) -> Vec<Vec<u8>> {
        let Some(ids) = &self.game_ids else {
            return vec![frame];
        };
        let mut pos = 0;
        let Some(id) = wire::read_varint(&frame, &mut pos) else {
            return Vec::new();
        };
        if id == ids.attack_id {
            return translate_attack(ids.interact_old_id, &frame[pos..]);
        }
        if id == ids.interact_id {
            return translate_interact(ids.interact_old_id, &frame[pos..]);
        }
        if let Some(v770) = &ids.v770
            && id == v770.player_command_id
        {
            return translate_player_command(v770.player_command_old_id, &frame[pos..]);
        }
        if let Some(v769) = &ids.v769
            && id == v769.container_click_id
        {
            return translate_container_click(v769.container_click_old_id, &frame[pos..]);
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

    /// The latest-version particle id for a source-version one, for the raw
    /// `level_particles` path; `None` drops the particle.
    pub fn remap_particle(&self, id: u32) -> Option<u32> {
        self.to_latest.remap(ClientRegistry::ParticleType, id)
    }

    /// Remaps a decoded packet's static-registry ids into the latest
    /// version's id space; `false` drops the packet (its subject no longer
    /// exists, e.g. the bed block entity removed in 26.2).
    pub fn remap_inbound(&self, packet: &mut ClientboundGamePacket) -> bool {
        use ClientRegistry as R;
        match packet {
            ClientboundGamePacket::AddEntity(p) => {
                remap_with(self.to_latest, R::EntityType, &mut p.entity_type)
            }
            ClientboundGamePacket::Sound(p) => self.remap_sound(&mut p.sound),
            ClientboundGamePacket::SoundEntity(p) => self.remap_sound(&mut p.sound),
            ClientboundGamePacket::UpdateAttributes(p) => {
                p.values
                    .retain_mut(|v| remap_with(self.to_latest, R::Attribute, &mut v.attribute));
                true
            }
            ClientboundGamePacket::BlockEntityData(p) => {
                remap_with(self.to_latest, R::BlockEntityType, &mut p.block_entity_type)
            }
            ClientboundGamePacket::LevelChunkWithLight(p) => {
                p.chunk_data
                    .block_entities
                    .retain_mut(|be| remap_with(self.to_latest, R::BlockEntityType, &mut be.kind));
                true
            }
            ClientboundGamePacket::ContainerSetContent(p) => {
                for item in &mut p.items {
                    remap_stack(self.to_latest, item);
                }
                remap_stack(self.to_latest, &mut p.carried_item);
                true
            }
            ClientboundGamePacket::ContainerSetSlot(p) => {
                remap_stack(self.to_latest, &mut p.item_stack);
                true
            }
            ClientboundGamePacket::SetCursorItem(p) => {
                remap_stack(self.to_latest, &mut p.contents);
                true
            }
            ClientboundGamePacket::SetEntityData(p) => {
                for item in &mut p.packed_items.0 {
                    if let azalea_entity::EntityDataValue::ItemStack(stack) = &mut item.value {
                        remap_stack(self.to_latest, stack);
                    }
                }
                true
            }
            _ => true,
        }
    }

    /// Remaps an outbound packet's static-registry ids into the launched
    /// version's id space. Never drops the packet; entries the older version
    /// lacks degrade to empty (the server resyncs the slot).
    /// TODO: delimit `set_creative_mode_slot` component values for 1.21.5 and
    /// up once pomme owns the encoder (see the azalea-divergence list).
    pub fn remap_outbound(&self, packet: &mut ServerboundGamePacket) {
        match packet {
            ServerboundGamePacket::ContainerClick(p) => {
                for (_, stack) in p.changed_slots.iter_mut() {
                    self.remap_hashed(stack);
                }
                self.remap_hashed(&mut p.carried_item);
            }
            ServerboundGamePacket::SetCreativeModeSlot(p) => {
                remap_stack(self.from_latest, &mut p.item_stack);
                if let ItemStack::Present(data) = &mut p.item_stack {
                    strip_untranslatable_components(self.from_latest, data);
                }
            }
            _ => {}
        }
    }

    fn remap_sound(&self, sound: &mut Holder<SoundEvent, CustomSound>) -> bool {
        match sound {
            Holder::Reference(kind) => remap_with(self.to_latest, ClientRegistry::SoundEvent, kind),
            Holder::Direct(_) => true,
        }
    }

    fn remap_hashed(&self, stack: &mut HashedStack) {
        use ClientRegistry as R;
        let Some(item) = &mut stack.0 else { return };
        if !remap_with(self.from_latest, R::Item, &mut item.kind) {
            stack.0 = None;
            return;
        }
        item.components
            .added_components
            .retain_mut(|(kind, _)| remap_with(self.from_latest, R::DataComponentType, kind));
        item.components
            .removed_components
            .retain_mut(|kind| remap_with(self.from_latest, R::DataComponentType, kind));
    }
}

/// Packets renamed between versions with identical fields; name matching
/// treats each pair as the same packet.
const RENAMED: &[(&str, &str)] = &[
    // `ClientboundHorseScreenOpenPacket` vs `ClientboundMountScreenOpenPacket`
    // in the references, byte-identical write() bodies.
    ("horse_screen_open", "mount_screen_open"),
    // Renamed by 1.21.2, byte-identical single-slot bodies.
    ("set_carried_item", "set_held_slot"),
];

impl GameIds {
    /// The version-gated frame rewrite dispatching on `id`, if any. Chained
    /// oldest-first: every rewriter targets 26.2 directly, so where two gates
    /// claim a packet the older one subsumes the newer and must win.
    fn version_rewrite(&self, id: u32) -> Option<FrameRewrite> {
        self.v767
            .as_ref()
            .and_then(|v| v.rewrite(id))
            .or_else(|| self.v768.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v769.as_ref().and_then(|v| v.rewrite(id)))
            .or_else(|| self.v772.as_ref().and_then(|v| v.rewrite(id)))
    }

    /// Name-matched game-phase id tables between one wire version and the
    /// latest. `None` when translation-by-id is a no-op: every inbound id
    /// maps to itself and every outbound id maps to itself or to nothing
    /// (26.1's only divergence is 26.2's `spectate_entity` ->
    /// `spectator_action` rename, which pomme never sends).
    fn build(protocol: i32, table: &PacketTable, latest: &PacketTable) -> Option<GameIds> {
        use Direction::{Clientbound, Serverbound};
        let map = |from: &PacketTable, to: &PacketTable, dir| -> Box<[Option<u32>]> {
            (0..)
                .map_while(|i| from.name_of(Phase::Game, dir, i))
                .map(|name| {
                    to.id(Phase::Game, dir, name).or_else(|| {
                        let alias = RENAMED.iter().find_map(|&(a, b)| {
                            if name == a {
                                Some(b)
                            } else if name == b {
                                Some(a)
                            } else {
                                None
                            }
                        })?;
                        to.id(Phase::Game, dir, alias)
                    })
                })
                .collect()
        };
        let inbound = map(table, latest, Clientbound);
        let outbound = map(latest, table, Serverbound);
        let same_ids = inbound
            .iter()
            .enumerate()
            .all(|(i, v)| *v == Some(i as u32))
            && outbound
                .iter()
                .enumerate()
                .all(|(i, v)| v.is_none() || *v == Some(i as u32));
        if same_ids {
            return None;
        }
        let id = |dir, name| required_id(latest, Phase::Game, dir, name);
        Some(GameIds {
            inbound,
            outbound,
            serializer_map: match protocol {
                774 => remap_serializer_774,
                773 => remap_serializer_773,
                // 1.21.5 through 1.21.8 register identical serializer sets,
                // as do 1.21.1 through 1.21.4.
                770..=772 => remap_serializer_772,
                767..=769 => remap_serializer_769,
                p => panic!("no serializer map for protocol {p}"),
            },
            set_entity_data_id: id(Clientbound, "set_entity_data"),
            level_chunk_id: id(Clientbound, "level_chunk_with_light"),
            set_time_id: id(Clientbound, "set_time"),
            attack_id: id(Serverbound, "attack"),
            interact_id: id(Serverbound, "interact"),
            interact_old_id: required_id(table, Phase::Game, Serverbound, "interact"),
            color_particles: protocol >= 766,
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
            quiet_suppressed: ["client_tick_end", "player_loaded"]
                .iter()
                .filter(|n| table.id(Phase::Game, Serverbound, n).is_none())
                .map(|n| required_id(latest, Phase::Game, Serverbound, n))
                .collect(),
        })
    }
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

/// Rewrites a latest `player_command` payload (`entityId, action, data`
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

/// Rewrites `cooldown`'s item id into the latest registry space. Vanilla
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

/// Rewrites a latest `container_click` payload for 1.21.4, which carries
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

/// Rewrites a latest `attack` payload (`entityId`) into an old-layout
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

/// Rewrites a latest `interact` payload (`entityId, hand, LpVec3 location,
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

/// The latest serializer id for a 1.21.11 `EntityDataSerializers` id: 26.x
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

/// The latest serializer id for a 1.21.10 `EntityDataSerializers` id:
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

/// The latest serializer id for a 1.21.8 `EntityDataSerializers` id: 1.21.9
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

/// The latest serializer id for a 1.21.4 `EntityDataSerializers` id: 1.21.5
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
/// trailing `enforcesSecureChat` bool.
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
/// a serializer the latest version dropped (1.21.8's `compound_tag`) is
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
                skip_nbt(&mut cur)?;
                continue;
            }
            return None;
        };
        out.push(index);
        wire::write_varint(&mut out, new);
        let value_at = cur.position() as usize;
        if new == 7 {
            let mut stack = Vec::new();
            if translate_item_stack(&mut cur, &mut stack, remaps, ids.v772.is_some()).is_some() {
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

/// Latest-registry component ids whose payloads the stack walker can advance
/// past (26.2 `DataComponents` registration order; anchored in
/// `component_id_anchors` in `azalea_compat`). Matching happens after the
/// remap, so one set of ids serves every wire version.
pub(crate) const COMPONENT_MAP_ID: u32 = 46;
pub(crate) const COMPONENT_PROFILE: u32 = 70;

/// Remaps one entity-data item stack (count, item id, component patch) into
/// the latest registry space. `old_profile` marks the pre-1.21.9 `profile`
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
        let latest = remaps.remap(ClientRegistry::DataComponentType, component)?;
        wire::write_varint(out, latest);
        let value_at = cur.position() as usize;
        match latest {
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

/// Advances past one entity-data value of the given latest-version
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
        let name = RegistryTable::latest().name_of(ClientRegistry::ParticleType, new)?;
        match name {
            "entity_effect" | "tinted_leaves" if ids.color_particles => {
                let color_at = cur.position() as usize;
                advance(cur, 4)?;
                out.extend_from_slice(&cur.get_ref()[color_at..cur.position() as usize]);
            }
            n if PAYLOAD_PARTICLES.contains(&n) => {
                tracing::debug!("Dropping a packet with an untranslatable {n} particle");
                return None;
            }
            _ => {}
        }
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
    let tag = read_u8(cur)?;
    skip_nbt_payload(cur, tag, 0)
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
/// to the packed list — and the block entities / light data after it are
/// copied verbatim.
fn translate_chunk(id: u32, payload: &[u8], nbt_heightmaps: bool) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 8)?; // chunk x/z ints

    let mut head = Vec::with_capacity(64);
    head.extend_from_slice(&payload[..8]);
    if nbt_heightmaps {
        convert_nbt_heightmaps(&mut cur, &mut head)?;
    } else {
        let maps_at = cur.position() as usize;
        let heightmaps = u32::azalea_read_var(&mut cur).ok()?;
        for _ in 0..heightmaps {
            varint_span(&mut cur)?; // heightmap type
            let longs = u32::azalea_read_var(&mut cur).ok()?;
            advance(&mut cur, (longs as usize).checked_mul(8)?)?;
        }
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
    out.extend_from_slice(&payload[buffer_end..]);
    Some(out)
}

/// Converts a pre-1.21.5 network-NBT heightmap compound (named long-array
/// tags) into the packed `(type id, long array)` list, advancing past the
/// NBT. Type ids from `Heightmap.Types` in the 1.21.5 reference; entries
/// under other names or tags are dropped (vanilla only sends the three
/// client-usage types, all mapped).
fn convert_nbt_heightmaps(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let mut entries: Vec<(u32, u32, std::ops::Range<usize>)> = Vec::new();
    let root = read_u8(cur)?;
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

/// azalea's typed encoder always writes latest-version component-type ids,
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
