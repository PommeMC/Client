//! The frame layer under every packet: a length prefix, and compression once
//! the server turns it on. Vanilla `Varint21FrameDecoder`,
//! `Varint21LengthFieldPrepender`, `CompressionDecoder` and
//! `CompressionEncoder`; encryption wraps the result in `conn`.

use std::io::{self, Read as _};

use flate2::Compression;
use flate2::bufread::{ZlibDecoder, ZlibEncoder};
use pomme_protocol::wire::{read_varint, write_varint};
use thiserror::Error;

/// `Varint21FrameDecoder.MAX_VARINT21_BYTES`: the length prefix's width, so a
/// frame holds under 2 MiB.
const MAX_LENGTH_BYTES: usize = 3;
/// `CompressionDecoder.MAXIMUM_UNCOMPRESSED_LENGTH`.
pub const MAX_UNCOMPRESSED_LENGTH: usize = 0x80_0000;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("length wider than 21-bit")]
    LengthTooWide,
    #[error("Frame length cannot be zero")]
    ZeroLength,
    #[error("Badly compressed packet: {0}")]
    Compression(String),
    #[error("Packet too big (is {0}, should be less than {MAX_UNCOMPRESSED_LENGTH})")]
    TooBig(usize),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("Connection closed")]
    Closed,
}

/// `Varint21FrameDecoder.decode`: the whole frame at the start of `bytes` and
/// how many bytes it took with its prefix; `None` until all of it is there.
pub fn split(bytes: &[u8]) -> Result<Option<(&[u8], usize)>, FrameError> {
    let prefix = &bytes[..bytes.len().min(MAX_LENGTH_BYTES)];
    let Some(prefix_len) = prefix.iter().position(|byte| byte & 0x80 == 0) else {
        return if prefix.len() == MAX_LENGTH_BYTES {
            Err(FrameError::LengthTooWide)
        } else {
            Ok(None)
        };
    };
    let start = prefix_len + 1;
    let length = read_varint(prefix, &mut 0).expect("a terminated varint") as usize;
    if length == 0 {
        return Err(FrameError::ZeroLength);
    }
    Ok(bytes
        .get(start..start + length)
        .map(|frame| (frame, start + length)))
}

/// `CompressionDecoder.decode`: a frame's packet, inflated when the server
/// compressed it. `compressed` is whether a threshold is set.
// Vanilla's client leaves the declared size unchecked
// (`validateDecompressed` is false); the cap keeps a hostile one from
// allocating gigabytes, and no server sends more, since its encoder refuses.
pub fn unpack(frame: &[u8], compressed: bool) -> Result<Box<[u8]>, FrameError> {
    if !compressed {
        return Ok(frame.into());
    }
    let mut pos = 0;
    let declared = read_varint(frame, &mut pos)
        .ok_or_else(|| FrameError::Compression("no uncompressed length".into()))?
        as usize;
    let body = &frame[pos..];
    if declared == 0 {
        return Ok(body.into());
    }
    if declared > MAX_UNCOMPRESSED_LENGTH {
        return Err(FrameError::Compression(format!(
            "size of {declared} is larger than protocol maximum of {MAX_UNCOMPRESSED_LENGTH}"
        )));
    }
    let mut packet = Vec::with_capacity(declared);
    // One byte past the declared size, to tell a longer payload apart.
    ZlibDecoder::new(body)
        .take(declared as u64 + 1)
        .read_to_end(&mut packet)
        .map_err(|e| FrameError::Compression(e.to_string()))?;
    if packet.len() != declared {
        return Err(FrameError::Compression(format!(
            "actual length of uncompressed payload {} does not match declared size {declared}",
            packet.len()
        )));
    }
    Ok(packet.into())
}

/// `CompressionEncoder.encode` then `Varint21LengthFieldPrepender.encode`:
/// `packet` as it goes on the wire, before encryption. With a `threshold`, a
/// packet at least that long is deflated.
pub fn pack(packet: &[u8], threshold: Option<u32>) -> Result<Vec<u8>, FrameError> {
    if packet.len() > MAX_UNCOMPRESSED_LENGTH {
        return Err(FrameError::TooBig(packet.len()));
    }
    let mut body = Vec::new();
    match threshold {
        None => body.extend_from_slice(packet),
        Some(threshold) if packet.len() < threshold as usize => {
            write_varint(&mut body, 0);
            body.extend_from_slice(packet);
        }
        Some(_) => {
            write_varint(&mut body, packet.len() as u32);
            ZlibEncoder::new(packet, Compression::default()).read_to_end(&mut body)?;
        }
    }
    let mut frame = Vec::with_capacity(body.len() + MAX_LENGTH_BYTES);
    write_varint(&mut frame, body.len() as u32);
    if frame.len() > MAX_LENGTH_BYTES {
        return Err(FrameError::TooBig(body.len()));
    }
    frame.extend_from_slice(&body);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_split_once_it_has_fully_arrived() {
        assert!(matches!(split(&[]), Ok(None)));
        // A two-byte length still missing its second byte.
        assert!(matches!(split(&[0x80]), Ok(None)));
        assert!(matches!(split(&[2, 7]), Ok(None)));
        assert_eq!(split(&[2, 7, 8, 9]).unwrap(), Some((&[7, 8][..], 3)));

        let long = [vec![0x80, 0x01], vec![5; 128], vec![1]].concat();
        let (frame, taken) = split(&long).unwrap().unwrap();
        assert_eq!((frame.len(), taken), (128, 130));
    }

    #[test]
    fn a_bad_length_is_an_error() {
        assert!(matches!(split(&[0, 1]), Err(FrameError::ZeroLength)));
        assert!(matches!(
            split(&[0x80, 0x80, 0x80, 0x01]),
            Err(FrameError::LengthTooWide)
        ));
        // Three bytes is the widest length there is.
        assert!(matches!(split(&[0xff, 0xff, 0x7f]), Ok(None)));
    }

    #[test]
    fn packets_round_trip_at_every_compression_setting() {
        let small = vec![1, 2, 3];
        let large: Vec<u8> = (0..4096u32).map(|i| (i % 7) as u8).collect();
        for threshold in [None, Some(256), Some(1)] {
            for packet in [&small, &large] {
                let wire = pack(packet, threshold).unwrap();
                let (frame, taken) = split(&wire).unwrap().unwrap();
                assert_eq!(taken, wire.len());
                assert_eq!(
                    *unpack(frame, threshold.is_some()).unwrap(),
                    packet[..],
                    "threshold {threshold:?}"
                );
            }
        }
    }

    #[test]
    fn the_layout_is_length_then_size_then_body() {
        assert_eq!(pack(&[7, 8], None).unwrap(), [2, 7, 8]);
        // Under the threshold: a zero size, then the packet as it is.
        assert_eq!(pack(&[7, 8], Some(256)).unwrap(), [3, 0, 7, 8]);
        // At the threshold: the size, then a zlib stream.
        let wire = pack(&[7; 64], Some(64)).unwrap();
        assert_eq!((wire[1], wire[2]), (64, 0x78));
        assert!(wire.len() < 64);
    }

    #[test]
    fn a_payload_that_isnt_its_declared_size_is_an_error() {
        let wire = pack(&[7; 64], Some(1)).unwrap();
        let (frame, _) = split(&wire).unwrap().unwrap();
        for declared in [63, 65] {
            let mut frame = frame.to_vec();
            frame[0] = declared;
            assert!(matches!(
                unpack(&frame, true),
                Err(FrameError::Compression(_))
            ));
        }
        // Not a zlib stream at all.
        assert!(matches!(
            unpack(&[4, 1, 2, 3, 4], true),
            Err(FrameError::Compression(_))
        ));
        // A declared size past the protocol's maximum.
        let mut huge = Vec::new();
        write_varint(&mut huge, MAX_UNCOMPRESSED_LENGTH as u32 + 1);
        assert!(matches!(
            unpack(&huge, true),
            Err(FrameError::Compression(_))
        ));
    }

    #[test]
    fn a_packet_past_the_maximum_isnt_written() {
        let packet = vec![0; MAX_UNCOMPRESSED_LENGTH + 1];
        assert!(matches!(pack(&packet, None), Err(FrameError::TooBig(_))));
        // Incompressible and too long for a 21-bit length.
        let noise: Vec<u8> = (0..3_000_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        assert!(matches!(pack(&noise, None), Err(FrameError::TooBig(_))));
    }
}
