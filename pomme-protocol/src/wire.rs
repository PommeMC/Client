//! Pomme-owned outbound packet encoding, sent through the connection's raw
//! write path (which still handles framing/compression/encryption). Grows as
//! packets migrate off azalea's serializers.

use glam::DVec3;

use crate::packets::{Direction, PacketTable, Phase};

const MAIN_HAND: u32 = 0;

fn game_serverbound_id(name: &str) -> u32 {
    PacketTable::latest()
        .id(Phase::Game, Direction::Serverbound, name)
        .unwrap_or_else(|| panic!("{name} missing from packet table"))
}

/// Vanilla `ServerboundInteractPacket`: right-click on an entity. `location`
/// is the hit point relative to the entity origin.
pub fn encode_interact(entity_id: i32, location: DVec3, sneaking: bool) -> Vec<u8> {
    let mut buf = Vec::new();
    write_varint(&mut buf, game_serverbound_id("interact"));
    write_varint(&mut buf, entity_id as u32);
    write_varint(&mut buf, MAIN_HAND);
    write_lp_vec3(&mut buf, location);
    buf.push(sneaking as u8);
    buf
}

/// Vanilla `ServerboundAttackPacket`: left-click on an entity. Encoded here
/// because azalea serializes the entity id as a fixed i32 instead of a
/// varint.
pub fn encode_attack(entity_id: i32) -> Vec<u8> {
    let mut buf = Vec::new();
    write_varint(&mut buf, game_serverbound_id("attack"));
    write_varint(&mut buf, entity_id as u32);
    buf
}

/// Reads one varint, advancing `pos`; `None` on truncation or overlong data.
pub fn read_varint(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let mut v = 0u32;
    for shift in 0..5 {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        v |= u32::from(byte & 0x7F) << (shift * 7);
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

pub fn write_varint(buf: &mut Vec<u8>, mut v: u32) {
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(byte);
            return;
        }
        buf.push(byte | 0x80);
    }
}

/// Vanilla `LpVec3.write`: a low-precision vec3. Each component is quantized
/// to 15 bits of the fraction `component / scale`, packed with the scale's low
/// 2 bits (plus a continuation flag and varint for larger scales) into 6
/// bytes.
pub fn write_lp_vec3(buf: &mut Vec<u8>, v: DVec3) {
    const ABS_MAX_VALUE: f64 = 1.717_986_918_3e10;
    const ABS_MIN_VALUE: f64 = 3.051_944_088_384_301e-5;

    fn sanitize(value: f64) -> f64 {
        if value.is_nan() {
            0.0
        } else {
            value.clamp(-ABS_MAX_VALUE, ABS_MAX_VALUE)
        }
    }
    // Java `Math.round`: round half up, not half to even.
    fn pack(value: f64) -> u64 {
        ((value * 0.5 + 0.5) * 32766.0 + 0.5).floor() as u64
    }

    let x = sanitize(v.x);
    let y = sanitize(v.y);
    let z = sanitize(v.z);
    let chessboard_length = x.abs().max(y.abs()).max(z.abs());
    if chessboard_length < ABS_MIN_VALUE {
        buf.push(0);
        return;
    }
    let scale = chessboard_length.ceil() as u64;
    let is_partial = (scale & 3) != scale;
    let markers = if is_partial { (scale & 3) | 4 } else { scale };
    let buffer = markers
        | pack(x / scale as f64) << 3
        | pack(y / scale as f64) << 18
        | pack(z / scale as f64) << 33;
    buf.push(buffer as u8);
    buf.push((buffer >> 8) as u8);
    buf.extend_from_slice(&((buffer >> 16) as u32).to_be_bytes());
    if is_partial {
        write_varint(buf, (scale >> 2) as u32);
    }
}

/// Vanilla `LpVec3.read`, the inverse of [`write_lp_vec3`]; advances `pos`.
pub fn read_lp_vec3(bytes: &[u8], pos: &mut usize) -> Option<DVec3> {
    fn unpack(value: u64) -> f64 {
        (value & 0x7FFF).min(32766) as f64 * 2.0 / 32766.0 - 1.0
    }

    let lowest = *bytes.get(*pos)?;
    *pos += 1;
    if lowest == 0 {
        return Some(DVec3::ZERO);
    }
    let middle = *bytes.get(*pos)?;
    *pos += 1;
    let highest = u32::from_be_bytes(bytes.get(*pos..*pos + 4)?.try_into().ok()?);
    *pos += 4;
    let buffer = (u64::from(highest) << 16) | (u64::from(middle) << 8) | u64::from(lowest);
    let mut scale = u64::from(lowest & 3);
    if lowest & 4 != 0 {
        scale |= u64::from(read_varint(bytes, pos)?) << 2;
    }
    Some(DVec3::new(
        unpack(buffer >> 3) * scale as f64,
        unpack(buffer >> 18) * scale as f64,
        unpack(buffer >> 33) * scale as f64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The azalea LpVec3 round-trip cross-check lives in pomme-client's
    // net::azalea_compat (this crate stays azalea-free).

    #[test]
    fn interact_packet_layout() {
        let bytes = encode_interact(42, DVec3::ZERO, true);
        // id 0x1A, entity id 42, main hand 0, LpVec3 zero byte, sneaking.
        assert_eq!(bytes, [0x1A, 42, 0, 0, 1]);
    }

    #[test]
    fn attack_packet_layout() {
        // id 0x01, entity id 42.
        assert_eq!(encode_attack(42), [0x01, 42]);
    }

    #[test]
    fn lp_vec3_round_trip() {
        for v in [
            DVec3::ZERO,
            DVec3::new(0.25, -0.5, 0.75),
            DVec3::new(1.5, -2.0, 3.25),
            DVec3::new(100.0, -250.5, 0.125),
        ] {
            let mut buf = Vec::new();
            write_lp_vec3(&mut buf, v);
            let mut pos = 0;
            let read = read_lp_vec3(&buf, &mut pos).unwrap();
            assert_eq!(pos, buf.len());
            // 15-bit quantization per axis, scaled by the chessboard length.
            let tolerance = v.abs().max_element().ceil().max(1.0) / 16383.0;
            assert!((read - v).abs().max_element() <= tolerance, "{v} -> {read}");
        }
    }

    #[test]
    fn varint_round_trip() {
        for v in [0u32, 1, 127, 128, 300, 25565, u32::MAX] {
            let mut buf = Vec::new();
            write_varint(&mut buf, v);
            let mut pos = 0;
            assert_eq!(read_varint(&buf, &mut pos), Some(v));
            assert_eq!(pos, buf.len());
        }
    }
}
