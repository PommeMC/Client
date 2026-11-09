use super::*;

/// Rewrites `level_chunk_with_light` by inserting the `fluidCount` short
/// 26.2 added after each section's `nonEmptyBlockCount` (zero: pomme
/// doesn't consume it and the client recounts on block changes). The
/// heightmaps before the section buffer are copied verbatim — or, with
/// `v769` (pre-1.21.5), converted from the network-NBT compound to the
/// packed list, and each section's packed longs lose their length prefix.
/// The buffer's end `padding` is dropped. When the wire version names its
/// NBT roots (1.20.1), each block entity tag after the buffer is rewritten;
/// otherwise the block entities are copied verbatim. The light data that
/// follows is always verbatim.
pub(super) fn translate_chunk(
    id: u32,
    payload: &[u8],
    v769: bool,
    named_nbt: bool,
    padding: SectionPadding,
) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(payload);
    advance(&mut cur, 8)?; // chunk x/z ints

    let mut head = Vec::with_capacity(64);
    head.extend_from_slice(&payload[..8]);
    if v769 {
        convert_nbt_heightmaps(&mut cur, &mut head, named_nbt)?;
    } else {
        let maps_at = cur.position() as usize;
        skip_heightmaps(&mut cur)?;
        head.extend_from_slice(&payload[maps_at..cur.position() as usize]);
    }
    let buffer_len = var_u32(&mut cur)? as usize;
    let buffer_at = cur.position() as usize;
    let buffer_end = buffer_at.checked_add(buffer_len)?;
    if buffer_end > payload.len() {
        return None;
    }

    let mut buffer = Vec::with_capacity(buffer_len + 3 * 26 * 2);
    let mut bcur = Cursor::new(&payload[..buffer_end]);
    bcur.set_position(buffer_at as u64);
    let mut padded = 0;
    while (bcur.position() as usize) + padded < buffer_end {
        let section_at = bcur.position() as usize;
        advance(&mut bcur, 2)?; // nonEmptyBlockCount
        buffer.extend_from_slice(&payload[section_at..bcur.position() as usize]);
        buffer.extend_from_slice(&[0, 0]); // fluidCount, new in 26.2
        for (entries, max_indirect_bits) in [(4096, 8), (64, 3)] {
            let longs =
                copy_paletted_container(&mut bcur, &mut buffer, entries, max_indirect_bits, v769)?;
            padded += padding.bytes(entries, longs);
        }
    }
    if bcur.position() as usize + padded != buffer_end {
        return None;
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
pub(super) fn skip_heightmaps(cur: &mut Cursor<&[u8]>) -> Option<()> {
    let heightmaps = var_u32(cur)?;
    for _ in 0..heightmaps {
        varint_span(cur)?; // heightmap type
        let longs = var_u32(cur)?;
        advance(cur, (longs as usize).checked_mul(8)?)?;
    }
    Some(())
}

/// Rewrites one 26.3 `ByteBufCodecs.BIT_SET` (`BitSet.toByteArray`: a varint
/// byte count, little-endian bytes) as the 26.2 `writeBitSet` form
/// (`toLongArray`: a varint long count, big-endian longs).
pub(super) fn repack_bit_set(cur: &mut Cursor<&[u8]>, out: &mut Vec<u8>) -> Option<()> {
    let len = var_u32(cur)? as usize;
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
pub(super) fn copy_block_entities(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    named_nbt: bool,
) -> Option<()> {
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
pub(super) fn copy_unnamed_nbt(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    named_nbt: bool,
) -> Option<()> {
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
pub(super) fn convert_nbt_heightmaps(
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

/// Copies one `PalettedContainer`: bits-per-entry byte, palette (single
/// value: one id; indirect while `bits <= max_indirect_bits`: id list;
/// global: nothing), then the packed-long array. With `length_prefixed`
/// (pre-1.21.5 `writeLongArray`) the array's varint length is checked
/// against the one the bits imply and dropped, as 1.21.5's
/// `writeFixedSizeLongArray` sends none. Returns the array's long count.
pub(super) fn copy_paletted_container(
    cur: &mut Cursor<&[u8]>,
    out: &mut Vec<u8>,
    entries: usize,
    max_indirect_bits: u8,
    length_prefixed: bool,
) -> Option<usize> {
    let at = cur.position() as usize;
    let bits = read_u8(cur)?;
    match bits {
        0 => {
            varint_span(cur)?;
        }
        _ if bits <= max_indirect_bits => {
            let palette_len = var_u32(cur)?;
            for _ in 0..palette_len {
                varint_span(cur)?;
            }
        }
        _ => {}
    }
    out.extend_from_slice(&cur.get_ref()[at..cur.position() as usize]);
    let longs = match bits {
        0 => 0,
        _ => entries.div_ceil(64 / bits as usize),
    };
    if length_prefixed && var_u32(cur)? as usize != longs {
        return None;
    }
    let longs_at = cur.position() as usize;
    advance(cur, longs.checked_mul(8)?)?;
    out.extend_from_slice(&cur.get_ref()[longs_at..cur.position() as usize]);
    Some(longs)
}

impl SectionPadding {
    pub(super) fn bytes(self, entries: usize, longs: usize) -> usize {
        match self {
            Self::None => 0,
            Self::EntrySized => varint_len(entries) - varint_len(longs),
            Self::UnwrittenLength => varint_len(longs),
        }
    }
}

/// The bytes `value` takes as a varint.
pub(super) fn varint_len(value: usize) -> usize {
    (usize::BITS - value.max(1).leading_zeros()).div_ceil(7) as usize
}
