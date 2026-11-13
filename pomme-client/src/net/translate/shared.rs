use super::*;

impl Rewrite {
    /// Rewrites one payload using its connection's immutable protocol tables.
    pub(super) fn apply(
        self,
        t: &Translation,
        id: u32,
        payload: &[u8],
        state: &mut SessionState,
    ) -> Option<Vec<u8>> {
        match self {
            Self::Context(f) => f(t, id, payload, state),
            Self::SessionFrame(f) => f(id, payload, state),
            Self::Frame(f) => f(id, payload),
            Self::Item(f) => f(
                id,
                payload,
                t.game_ids.as_ref()?.items.as_ref()?.items(t.to_native),
            ),
            Self::Recipe(f) => f(
                id,
                payload,
                RecipeCtx {
                    remaps: t.to_native,
                    layout: t.recipe_layout?,
                },
            ),
        }
    }

    /// Compares resolved function identities for dispatch migration tests.
    #[cfg(test)]
    pub(super) fn same(self, other: Self) -> bool {
        match (self, other) {
            (Self::Context(a), Self::Context(b)) => std::ptr::fn_addr_eq(a, b),
            (Self::Frame(a), Self::Frame(b)) => std::ptr::fn_addr_eq(a, b),
            (Self::SessionFrame(a), Self::SessionFrame(b)) => std::ptr::fn_addr_eq(a, b),
            (Self::Item(a), Self::Item(b)) => std::ptr::fn_addr_eq(a, b),
            (Self::Recipe(a), Self::Recipe(b)) => std::ptr::fn_addr_eq(a, b),
            _ => false,
        }
    }
}

/// A wire id in `registry` as the native version numbers it: remapped for a
/// translated session, unchanged for a native one.
pub fn native_id(registry: ClientRegistry, id: u32) -> Option<u32> {
    match active() {
        Some(t) => t.remap_to_native(registry, id),
        None => Some(id),
    }
}

/// The translation for the wire version negotiated with the current server,
/// or `None` for the native version.
pub fn active() -> Option<&'static Translation> {
    cached(crate::version::session_protocol())
}

/// Returns immutable protocol tables cached in one OnceLock per version.
pub(super) fn cached(protocol: i32) -> Option<&'static Translation> {
    if protocol == NATIVE.protocol {
        return None;
    }
    static TABLES: [OnceLock<Translation>; TRANSLATED.len()] =
        [const { OnceLock::new() }; TRANSLATED.len()];
    let index = TRANSLATED.iter().position(|&p| p == protocol)?;
    Some(TABLES[index].get_or_init(|| {
        tracing::info!(
            "Prepared protocol translation {protocol} <-> {}",
            NATIVE.protocol
        );
        Translation::for_protocol(protocol).expect("translated protocol")
    }))
}

/// Builds the version-keyed caches a translated join needs (registry remaps,
/// packet tables, block-state table) without activating anything, so a join
/// after a server-list ping finds them warm. No-op when already built.
pub fn prewarm(protocol: i32) {
    let _ = cached(protocol);
    let _ = crate::world::block::BlockStateTable::for_protocol(protocol);
}

/// Consumes the empty name an NBT root carries when `named` (before 1.20.2,
/// `FriendlyByteBuf.writeNbt` went through `NbtIo.write`, which emits the tag
/// byte, an empty UTF name and then the payload; 1.20.2 switched to
/// `writeAnyTag`, which drops the name). `TAG_End` (an absent value) carries no
/// name on either side.
pub(super) fn skip_root_name(cur: &mut Cursor<&[u8]>, tag: u8, named: bool) -> Option<()> {
    if tag == 0 || !named {
        return Some(());
    }
    let len = read_u16(cur)?;
    advance(cur, len as usize)
}

/// Skips one NBT value whose root may be named; see [`skip_root_name`].
pub(super) fn skip_nbt_root(cur: &mut Cursor<&[u8]>, named: bool) -> Option<()> {
    let tag = read_u8(cur)?;
    skip_root_name(cur, tag, named)?;
    skip_nbt_payload(cur, tag, 0)
}

/// Splits 765's single-NBT registry_data — a map of
/// `{registry: {type, value: [{name, id, element}]}}` — into the
/// per-registry packets 26.2 uses. Entries are ordered by their explicit
/// ids (the client derives protocol ids from wire order, which is
/// load-bearing for biome colors and dimension types).
pub(super) fn split_registry_data(
    id: u32,
    payload: &[u8],
    state: &mut SessionState,
) -> Option<Vec<Box<[u8]>>> {
    use simdnbt::owned::NbtTag;

    let mut cur = Cursor::new(payload);
    let NbtTag::Compound(root) = simdnbt::owned::read_tag(&mut cur).ok()? else {
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
            state.dimension_types = ordered.iter().map(|(_, name, _)| name.clone()).collect();
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
            NbtTag::Compound(element.clone()).write(&mut out);
        }
        frames.push(out.into_boxed_slice());
    }
    Some(frames)
}

/// The synced-registry index for a dimension-type key, for the pre-1.20.5
/// spawn-info rewrites.
pub(super) fn dimension_type_index(name: &str, state: &SessionState) -> Option<u32> {
    state
        .dimension_types
        .iter()
        .position(|n| n == name)
        .map(|i| i as u32)
}

/// Name-matched id table from one version's phase/direction to another's,
/// with the `RENAMED` aliases; wire id == index, `None` = no equivalent.
pub(super) fn id_map(
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
    /// itself or nothing (from 766 up later versions only appended, and 763
    /// has no configuration phase), except on 766 and 767, whose
    /// `client_information` still needs its layout rewritten.
    pub(super) fn build(
        protocol: i32,
        table: &PacketTable,
        native: &PacketTable,
    ) -> Option<ConfigIds> {
        use Direction::{Clientbound, Serverbound};
        let inbound = id_map(table, native, Phase::Configuration, Clientbound);
        let outbound = id_map(native, table, Phase::Configuration, Serverbound);
        let inbound_identity = identity_maps(&inbound, &outbound);
        if inbound_identity && !(766..=767).contains(&protocol) {
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
            v767_client_information_id: (protocol <= 767)
                .then(|| id(Serverbound, "client_information")),
            inbound_identity,
        })
    }
}

/// Whether translation-by-id would be a no-op: every inbound id maps to
/// itself and every outbound id maps to itself or to nothing.
pub(super) fn identity_maps(inbound: &[Option<u32>], outbound: &[Option<u32>]) -> bool {
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
pub(super) fn plain_config_frame(id: u32, payload: &[u8]) -> Vec<Box<[u8]>> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(payload);
    vec![out.into_boxed_slice()]
}

/// A packet id that must exist in the given table.
pub(super) fn required_id(table: &PacketTable, phase: Phase, dir: Direction, name: &str) -> u32 {
    table
        .id(phase, dir, name)
        .unwrap_or_else(|| panic!("{name} missing from {phase:?} packet table"))
}

/// The shared `id, entityId, action` prefix of an old-layout `interact`
/// frame.
pub(super) fn interact_frame(interact_old_id: u32, entity_id: u32, action: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    wire::write_varint(&mut out, interact_old_id);
    wire::write_varint(&mut out, entity_id);
    wire::write_varint(&mut out, action);
    out
}

/// Writes a 26.2 `PositionMoveRotation`: position, zero delta, rotation.
pub(super) fn write_position_move_rotation(out: &mut Vec<u8>, pos: &[u8], y_rot: f32, x_rot: f32) {
    out.extend_from_slice(pos);
    out.extend_from_slice(&[0; 24]); // zero delta movement
    out.extend_from_slice(&y_rot.to_be_bytes());
    out.extend_from_slice(&x_rot.to_be_bytes());
}

/// Inserts the `seaLevel` varint 1.21.2 appended to CommonPlayerSpawnInfo,
/// which sits immediately before the final byte of both `login`
/// (enforcesSecureChat) and `respawn` (dataToKeep); vanilla's overworld
/// value is synthesized.
pub(super) fn insert_sea_level(payload: &[u8]) -> Option<Vec<u8>> {
    let (last, body) = payload.split_last()?;
    let mut out = Vec::with_capacity(payload.len() + 1);
    out.extend_from_slice(body);
    wire::write_varint(&mut out, 63);
    out.push(*last);
    Some(out)
}

/// Writes a 26.2 hashed or component stack as a bare 1.20.4 item
/// (`bool + item + i8 count + empty NBT`); components were already
/// stripped by `remap_outbound` (no 765 component registry exists), so
/// only the presence, item and count survive.
pub(super) fn write_old_item(out: &mut Vec<u8>, stack: Option<(u32, u32)>) {
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

/// A serverbound packet whose newer layout appended one byte: the last-seen
/// checksum 1.21.5 added to `chat` and `chat_command_signed` (1.20.4's
/// `chat_command` has the signed layout), or the `particleStatus` 1.21.2
/// added to `client_information`.
pub(super) fn strip_last_byte(old_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let Some((_, body)) = payload.split_last() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(body.len() + 5);
    wire::write_varint(&mut out, old_id);
    out.extend_from_slice(body);
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
pub(super) fn translate_update_attributes(
    remaps: &RegistryRemaps,
    id: u32,
    payload: &[u8],
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id
    let entries = var_u32(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 8);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..entries {
        let attribute = var_u32(&mut cur)?;
        wire::write_varint(
            &mut out,
            remaps.remap(ClientRegistry::Attribute, attribute)?,
        );
        let body_at = cur.position() as usize;
        advance(&mut cur, 8)?; // base
        let modifiers = var_u32(&mut cur)?;
        for _ in 0..modifiers {
            let len = var_u32(&mut cur)? as usize;
            advance(&mut cur, len)?; // modifier id
            advance(&mut cur, 8)?; // amount
            varint_span(&mut cur)?; // operation
        }
        out.extend_from_slice(&payload[body_at..cur.position() as usize]);
    }
    Some(out)
}

/// Reads one 1.20.4 optional item stack (`bool present + item id +
/// i8 count + NBT`), skipping the NBT per the bare-items decision; `None`
/// is a malformed stack, `Some(None)` an absent one.
/// TODO: translate the legacy NBT (enchantments, custom names, damage)
/// into 26.2 data components instead of dropping it.
pub(super) fn read_old_item(cur: &mut Cursor<&[u8]>, named_nbt: bool) -> Option<Option<(u32, u8)>> {
    if read_u8(cur)? == 0 {
        return Some(None);
    }
    let item = var_u32(cur)?;
    let count = read_u8(cur)?;
    skip_nbt_root(cur, named_nbt)?;
    Some(Some((item, count)))
}

/// Rewrites `update_attributes` for the pre-1.21 layouts: the attribute id
/// is remapped into the native registry space and each modifier's UUID id
/// becomes a hex resource location. 1.20.4 keys the attribute by resource
/// location rather than registry id, so it passes its own table as
/// `key_table` to resolve the name first.
pub(super) fn translate_update_attributes_uuid(
    key_table: Option<&RegistryTable>,
    remaps: &RegistryRemaps,
    id: u32,
    payload: &[u8],
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    varint_span(&mut cur)?; // entity id
    let entries = var_u32(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 64);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..cur.position() as usize]);
    for _ in 0..entries {
        let attribute = match key_table {
            Some(table) => {
                let len = var_u32(&mut cur)? as usize;
                let key_at = cur.position() as usize;
                advance(&mut cur, len)?;
                let key = std::str::from_utf8(&payload[key_at..key_at + len]).ok()?;
                let key = key.strip_prefix("minecraft:").unwrap_or(key);
                table
                    .names(ClientRegistry::Attribute)
                    .iter()
                    .position(|n| n == key)? as u32
            }
            None => var_u32(&mut cur)?,
        };
        wire::write_varint(
            &mut out,
            remaps.remap(ClientRegistry::Attribute, attribute)?,
        );

        let body_at = cur.position() as usize;
        advance(&mut cur, 8)?; // base
        let modifiers = var_u32(&mut cur)?;
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
pub(super) fn translate_modifier_uuid(
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

/// Replaces the dimension-type resource key at `spawn_at` (the start of a
/// 765 CommonPlayerSpawnInfo) with its synced-registry index, copying
/// everything else verbatim.
pub(super) fn convert_spawn_info_dimension(
    payload: &[u8],
    spawn_at: usize,
    state: &SessionState,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    cur.set_position(spawn_at as u64);
    let len = var_u32(&mut cur)? as usize;
    let key_at = cur.position() as usize;
    advance(&mut cur, len)?;
    let key = std::str::from_utf8(&payload[key_at..key_at + len]).ok()?;
    let index = dimension_type_index(key, state).unwrap_or_else(|| {
        tracing::warn!("Unknown dimension type {key}; defaulting to 0");
        0
    });

    let mut out = Vec::with_capacity(payload.len());
    out.extend_from_slice(&payload[..spawn_at]);
    wire::write_varint(&mut out, index);
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Vanilla `VarInt.read` off the cursor.
pub(super) fn var_u32(cur: &mut Cursor<&[u8]>) -> Option<u32> {
    Reader::on_cursor(cur, Reader::varint).ok()
}

pub(super) fn var_i32(cur: &mut Cursor<&[u8]>) -> Option<i32> {
    Reader::on_cursor(cur, Reader::var_i32).ok()
}

/// Copies `n` bytes from the cursor's payload to `out`.
pub(super) fn copy_bytes(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>, n: usize) -> Option<()> {
    let at = cur.position() as usize;
    advance(cur, n)?;
    out.extend_from_slice(&cur.get_ref()[at..at + n]);
    Some(())
}

pub(super) fn copy_varint(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<u32> {
    let at = cur.position() as usize;
    let v = var_u32(cur)?;
    out.extend_from_slice(&cur.get_ref()[at..cur.position() as usize]);
    Some(v)
}

pub(super) fn copy_utf(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let len = copy_varint(cur, out)?;
    copy_bytes(cur, out, len as usize)
}

/// Copies a nullable field, running `inner` on a present value.
pub(super) fn copy_optional(
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

/// A component field copied verbatim (already network NBT at 765).
pub(super) fn copy_nbt(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let span = nbt_span(cur)?;
    out.extend_from_slice(&cur.get_ref()[span]);
    Some(())
}

/// Copies one `Holder<SoundEvent>` (varint id + 1, or 0 followed by the
/// inline definition: location string plus an optional fixed range),
/// remapping a referenced id into 26.2's space.
pub(super) fn translate_sound_holder(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
) -> Option<()> {
    let holder = var_u32(cur)?;
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

pub(super) fn skip_utf(cur: &mut Cursor<&[u8]>) -> Option<()> {
    let len = var_u32(cur)?;
    advance(cur, len as usize)
}

pub(super) fn skip_nbt(cur: &mut Cursor<&[u8]>) -> Option<()> {
    skip_nbt_root(cur, false)
}

pub(super) fn skip_optional(
    cur: &mut Cursor<&[u8]>,
    inner: impl Fn(&mut Cursor<&[u8]>) -> Option<()>,
) -> Option<()> {
    if read_u8(cur)? != 0 {
        inner(cur)
    } else {
        Some(())
    }
}

pub(super) fn advance(cur: &mut Cursor<&[u8]>, n: usize) -> Option<()> {
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
pub(super) fn collapse_vec_delta(cur: &mut Cursor<&[u8]>, step_count: u32) -> Option<[i16; 3]> {
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

/// Advances past a 26.3 `PositionPath` (varint tag: 0 = one `Vec3`, 1 = a
/// counted list of `Vec3` + tick-offset varint steps), returning the 24
/// position bytes it ends at.
pub(super) fn read_position_path_end<'a>(cur: &mut Cursor<&'a [u8]>) -> Option<&'a [u8]> {
    let stepped = match var_u32(cur)? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let steps = if stepped { var_u32(cur)? } else { 1 };
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

/// Rewrites the `previousGameType` of a `CommonPlayerSpawnInfo` starting at
/// `spawn_info_at`: 26.3 sends an optional varint (0 = none, else id + 1)
/// where 26.2 reads a signed byte (-1 = none). `gameType` before it went
/// from a byte to a varint, byte-identical for its four values.
pub(super) fn rewrite_previous_game_type(
    id: u32,
    payload: &[u8],
    spawn_info_at: usize,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    cur.set_position(spawn_info_at as u64);
    varint_span(&mut cur)?; // dimension type
    skip_utf(&mut cur)?; // dimension
    advance(&mut cur, 8)?; // seed
    varint_span(&mut cur)?; // game type
    let previous_at = cur.position() as usize;
    let previous = var_u32(&mut cur)?;

    let mut out = Vec::with_capacity(payload.len() + 1);
    wire::write_varint(&mut out, id);
    out.extend_from_slice(&payload[..previous_at]);
    out.push(previous.checked_sub(1).map_or(0xFF, |g| g as u8));
    out.extend_from_slice(&payload[cur.position() as usize..]);
    Some(out)
}

/// Advances past a `DisplayInfo` body: two NBT components, an
/// `ItemStackTemplate` (item, count, component patch), a varint type, an
/// i32 flag set and a background identifier when its bit 0 is set.
pub(super) fn skip_display_info(cur: &mut Cursor<&[u8]>, remaps: &RegistryRemaps) -> Option<()> {
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

pub(super) fn read_f64s<const N: usize>(cur: &mut Cursor<&[u8]>) -> Option<[f64; N]> {
    let mut v = [0.0; N];
    for c in &mut v {
        let at = cur.position() as usize;
        advance(cur, 8)?;
        *c = f64::from_be_bytes(cur.get_ref()[at..at + 8].try_into().ok()?);
    }
    Some(v)
}

/// Rewrites `commands`, whose argument-type registry (`ArgumentTypeInfos`)
/// gains and renames parsers between versions: every parser id is remapped
/// by name, and a parser 26.2 lacks (26.3's five singletons) becomes the one a
/// 26.2 server sent for the same argument. Node and property layouts are
/// unchanged from 1.20.1 to 26.3 (`ClientboundCommandsPacket`), so node
/// indices never move.
pub(super) fn translate_commands(
    id: u32,
    payload: &[u8],
    wire_registries: &RegistryTable,
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
            let parser = var_u32(&mut cur)?;
            let name = wire_registries.name_of(ClientRegistry::CommandArgumentType, parser)?;
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
/// serializers; unchanged from 1.20.1 to 26.3).
pub(super) fn skip_parser_properties(cur: &mut Cursor<&[u8]>, parser: &str) -> Option<()> {
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
pub(super) fn write_substitute_parser(out: &mut Vec<u8>, parser: &str) -> Option<()> {
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

/// Normalizes one item `HolderSet` (`ByteBufCodecs.holderSet`: 0 then a tag
/// id, or count + 1 then that many concrete item ids) into native item-id
/// space. Named tags keep their resource id; direct holders are remapped,
/// and those with no native item dropped. Whether any holder is left.
pub(super) fn translate_item_holder_set(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    remaps: &RegistryRemaps,
) -> Option<bool> {
    let encoded = var_u32(cur)?;
    if encoded == 0 {
        wire::write_varint(out, 0);
        copy_utf(cur, out)?;
        return Some(true);
    }
    let items = native_items(cur, encoded - 1, remaps)?;
    wire::write_varint(out, items.len() as u32 + 1);
    for &item in &items {
        wire::write_varint(out, item);
    }
    Some(!items.is_empty())
}

/// Reads `count` item ids and keeps the ones with a native item.
pub(super) fn native_items(
    cur: &mut Cursor<&[u8]>,
    count: u32,
    remaps: &RegistryRemaps,
) -> Option<Vec<u32>> {
    let mut items = Vec::new();
    for _ in 0..count {
        if let Some(item) = remaps.remap(ClientRegistry::Item, var_u32(cur)?) {
            items.push(item);
        }
    }
    Some(items)
}

/// The byte range of one varint, advancing past it.
pub(super) fn varint_span(cur: &mut Cursor<&[u8]>) -> Option<std::ops::Range<usize>> {
    let start = cur.position() as usize;
    var_u32(cur)?;
    Some(start..cur.position() as usize)
}

/// The byte range of one network-NBT value (type byte + unnamed payload),
/// advancing past it.
pub(super) fn nbt_span(cur: &mut Cursor<&[u8]>) -> Option<std::ops::Range<usize>> {
    let start = cur.position() as usize;
    skip_nbt(cur)?;
    Some(start..cur.position() as usize)
}

pub(super) fn read_u8(cur: &mut Cursor<&[u8]>) -> Option<u8> {
    let b = *cur.get_ref().get(cur.position() as usize)?;
    cur.set_position(cur.position() + 1);
    Some(b)
}

pub(super) fn read_u16(cur: &mut Cursor<&[u8]>) -> Option<u16> {
    Some(u16::from_be_bytes([read_u8(cur)?, read_u8(cur)?]))
}

pub(super) fn read_i32(cur: &mut Cursor<&[u8]>) -> Option<i32> {
    let b = [read_u8(cur)?, read_u8(cur)?, read_u8(cur)?, read_u8(cur)?];
    Some(i32::from_be_bytes(b))
}

/// Skips one NBT payload of the given tag type; see
/// [`pomme_protocol::codec::Reader::skip_nbt_payload`].
pub(super) fn skip_nbt_payload(cur: &mut Cursor<&[u8]>, tag: u8, depth: u32) -> Option<()> {
    pomme_protocol::codec::Reader::on_cursor(cur, |r| r.skip_nbt_payload(tag, depth)).ok()
}
