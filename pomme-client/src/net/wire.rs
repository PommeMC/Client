//! Pomme-owned outbound packet encoding, sent through the connection's raw
//! write path (which still handles framing/compression/encryption). Grows as
//! packets migrate off azalea's serializers.

use glam::DVec3;

/// Serverbound game packet ids, MC 26.2 (`GameProtocols.java` registration
/// order).
const INTERACT_PACKET_ID: u32 = 0x1A;

const MAIN_HAND: u32 = 0;

/// Vanilla `ServerboundInteractPacket`: right-click on an entity. `location`
/// is the hit point relative to the entity origin.
pub fn encode_interact(entity_id: i32, location: DVec3, sneaking: bool) -> Vec<u8> {
    let mut buf = Vec::new();
    write_varint(&mut buf, INTERACT_PACKET_ID);
    write_varint(&mut buf, entity_id as u32);
    write_varint(&mut buf, MAIN_HAND);
    write_lp_vec3(&mut buf, location);
    buf.push(sneaking as u8);
    buf
}

fn write_varint(buf: &mut Vec<u8>, mut v: u32) {
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
fn write_lp_vec3(buf: &mut Vec<u8>, v: DVec3) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip through azalea's `LpVec3` decoder to cross-check the port.
    fn decode_lp_vec3(bytes: &[u8]) -> DVec3 {
        use azalea_buf::AzBuf;
        let mut cursor = std::io::Cursor::new(bytes);
        let lp = azalea_core::delta::LpVec3::azalea_read(&mut cursor).unwrap();
        assert_eq!(cursor.position() as usize, bytes.len(), "leftover bytes");
        let v = azalea_core::position::Vec3::from(lp);
        DVec3::new(v.x, v.y, v.z)
    }

    #[test]
    fn lp_vec3_roundtrip() {
        let cases = [
            DVec3::ZERO,
            DVec3::new(0.3, 1.62, -0.21),
            DVec3::new(-0.5, -0.001, 0.5),
            DVec3::new(2.75, -3.5, 1.0),
            DVec3::new(120.0, -64.25, 300.5),
        ];
        for v in cases {
            let mut buf = Vec::new();
            write_lp_vec3(&mut buf, v);
            let decoded = decode_lp_vec3(&buf);
            // Quantization error is bounded by scale / 32766 per component.
            let tolerance = (v.abs().max_element().ceil() / 32766.0).max(1e-9) * 1.01;
            assert!(
                (decoded - v).abs().max_element() <= tolerance,
                "{v:?} decoded as {decoded:?} (tolerance {tolerance})"
            );
        }
    }

    #[test]
    fn interact_packet_layout() {
        let bytes = encode_interact(42, DVec3::ZERO, true);
        // id 0x1A, entity id 42, main hand 0, LpVec3 zero byte, sneaking.
        assert_eq!(bytes, [0x1A, 42, 0, 0, 1]);
    }
}
