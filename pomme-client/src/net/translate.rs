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
//! Known 26.1 limitation (accepted): an inbound item stack carrying a data
//! component at/after the first shifted component id (78, where 26.2
//! inserted `sulfur_cube_content`) decodes under the wrong 26.2 codec —
//! usually a misparse that skips the packet via `skip_malformed_packet`,
//! though a coincidentally parsable layout yields a silently wrong
//! component. Common survival items only use earlier, unshifted components.
//! Items nested inside component values (bundles, containers) also keep
//! their source-version ids.

use std::io::Cursor;
use std::sync::Mutex;

use azalea_buf::AzBufVar;
use azalea_core::sound::CustomSound;
use azalea_inventory::{ItemStack, ItemStackData};
use azalea_protocol::packets::game::s_container_click::HashedStack;
use azalea_protocol::packets::game::{ClientboundGamePacket, ServerboundGamePacket};
use azalea_registry::builtin::{DataComponentKind, SoundEvent};
use azalea_registry::{Holder, Registry};
use pomme_protocol::version::LATEST;
use pomme_protocol::{ClientRegistry, Direction, PacketTable, Phase, RegistryRemaps};

pub struct Translation {
    to_latest: &'static RegistryRemaps,
    from_latest: &'static RegistryRemaps,
    login_finished_id: u32,
    game_login_id: u32,
    set_player_team_id: u32,
}

/// Protocols the wire translation fully covers. 1.21.11 (774) has embedded
/// tables, but its game translation hasn't landed yet, so it stays
/// un-joinable and untranslated.
const TRANSLATED: &[i32] = &[775];

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

impl Translation {
    /// The translation for one protocol number, or `None` outside
    /// `TRANSLATED`: the frame rewrites below are version-specific, so
    /// embedded data alone isn't enough (and the latest version needs none).
    pub(crate) fn for_protocol(protocol: i32) -> Option<Translation> {
        if !TRANSLATED.contains(&protocol) {
            return None;
        }
        let table = PacketTable::for_protocol(protocol)?;
        let id = |phase, name: &str| {
            table
                .id(phase, Direction::Clientbound, name)
                .unwrap_or_else(|| panic!("{name} missing from {phase:?} packet table"))
        };
        Some(Translation {
            to_latest: RegistryRemaps::to_latest(protocol)?,
            from_latest: RegistryRemaps::from_latest(protocol)?,
            login_finished_id: id(Phase::Login, "login_finished"),
            game_login_id: id(Phase::Game, "login"),
            set_player_team_id: id(Phase::Game, "set_player_team"),
        })
    }

    /// Rewrites a raw login-phase frame into the latest layout.
    pub fn translate_login_frame(&self, raw: Box<[u8]>) -> Box<[u8]> {
        let mut cur = Cursor::new(&raw[..]);
        if u32::azalea_read_var(&mut cur).ok() == Some(self.login_finished_id) {
            // 26.2 appended a session-id UUID; zero is fine, pomme only
            // reads the game profile.
            let mut out = raw.into_vec();
            out.extend_from_slice(&[0; 16]);
            return out.into_boxed_slice();
        }
        raw
    }

    /// Rewrites a raw game-phase frame into the latest layout; `None` drops
    /// the packet (malformed beyond repair).
    pub fn translate_game_frame(&self, raw: Box<[u8]>) -> Option<Box<[u8]>> {
        let mut cur = Cursor::new(&raw[..]);
        let id = u32::azalea_read_var(&mut cur).ok()?;
        if id == self.game_login_id {
            // 26.2 added `onlineMode` before the trailing
            // `enforcesSecureChat` bool.
            let mut out = raw.into_vec();
            if out.is_empty() {
                return None;
            }
            out.insert(out.len() - 1, 0);
            return Some(out.into_boxed_slice());
        }
        if id == self.set_player_team_id {
            return match translate_team(&raw, cur.position() as usize) {
                Some(out) => Some(out.into_boxed_slice()),
                None => {
                    tracing::warn!("Dropping unparsable 26.1 set_player_team packet");
                    None
                }
            };
        }
        Some(raw)
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

/// Rewrites `set_player_team` from the 26.1 `Parameters` layout
/// (`displayName, options, visibility, collision, color, prefix, suffix`
/// with color as a `ChatFormatting` ordinal) to the 26.2 one
/// (`displayName, prefix, suffix, visibility, collision, color, options`
/// with color as `Optional<TeamColor>`). `payload_start` is the offset past
/// the packet-id varint; the surrounding name/method/player-list fields are
/// copied verbatim.
fn translate_team(raw: &[u8], payload_start: usize) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(raw);
    cur.set_position(payload_start as u64);

    let name_len = u32::azalea_read_var(&mut cur).ok()?;
    advance(&mut cur, name_len as usize)?;
    let method_at = cur.position() as usize;
    let method = *raw.get(method_at)?;
    advance(&mut cur, 1)?;

    let mut out = Vec::with_capacity(raw.len() + 2);
    out.extend_from_slice(&raw[..method_at + 1]);

    // Methods 0 (add) and 2 (change) carry Parameters.
    if method == 0 || method == 2 {
        let display = nbt_span(&mut cur)?;
        let options_at = cur.position() as usize;
        let options = *raw.get(options_at)?;
        advance(&mut cur, 1)?;
        let visibility = varint_span(&mut cur)?;
        let collision = varint_span(&mut cur)?;
        let color = u32::azalea_read_var(&mut cur).ok()?;
        let prefix = nbt_span(&mut cur)?;
        let suffix = nbt_span(&mut cur)?;

        out.extend_from_slice(&raw[display]);
        out.extend_from_slice(&raw[prefix]);
        out.extend_from_slice(&raw[suffix]);
        out.extend_from_slice(&raw[visibility]);
        out.extend_from_slice(&raw[collision]);
        // ChatFormatting ordinals 0..=15 are the colors, in TeamColor id
        // order; formats (16..=20) and RESET (21) have no team color.
        if color <= 15 {
            out.push(1);
            out.push(color as u8);
        } else {
            out.push(0);
        }
        out.push(options);
    }

    // Player list (methods 0/3/4) and anything after: verbatim.
    out.extend_from_slice(&raw[cur.position() as usize..]);
    Some(out)
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
    let tag = read_u8(cur)?;
    skip_nbt_payload(cur, tag, 0)?;
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
