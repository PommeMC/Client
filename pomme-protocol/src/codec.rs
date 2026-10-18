//! Bounds-checked readers for vanilla's `ByteBufCodecs` primitives, used by
//! pomme's own decoders. Every read fails cleanly on truncated or hostile
//! input; nothing allocates more than the bytes left in the frame.

use std::fmt;

use glam::DVec3;

/// Vanilla `NbtAccounter`'s nesting limit.
const MAX_NBT_DEPTH: u32 = 512;

/// Vanilla `Utf8String.MAX_STRING_LENGTH`, the `STRING_UTF8` default.
pub const MAX_STRING_LENGTH: usize = 32767;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub what: &'static str,
    pub at: usize,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

impl std::error::Error for DecodeError {}

pub type DecodeResult<T> = Result<T, DecodeError>;

pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Runs `read` on the rest of `cur`, advancing it past what was read.
    pub fn on_cursor<T>(
        cur: &mut std::io::Cursor<&'a [u8]>,
        read: impl FnOnce(&mut Reader<'a>) -> DecodeResult<T>,
    ) -> DecodeResult<T> {
        let start = (cur.position() as usize).min(cur.get_ref().len());
        let mut r = Reader::new(&cur.get_ref()[start..]);
        let value = read(&mut r)?;
        cur.set_position((start + r.pos) as u64);
        Ok(value)
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    /// The bytes consumed since `start`, a position this reader returned.
    pub fn since(&self, start: usize) -> &'a [u8] {
        &self.bytes[start..self.pos]
    }

    pub fn error(&self, what: &'static str) -> DecodeError {
        DecodeError { what, at: self.pos }
    }

    pub fn bytes(&mut self, n: usize) -> DecodeResult<&'a [u8]> {
        if n > self.remaining() {
            return Err(self.error("truncated"));
        }
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> DecodeResult<[u8; N]> {
        Ok(self.bytes(N)?.try_into().expect("length checked"))
    }

    pub fn u8(&mut self) -> DecodeResult<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn i8(&mut self) -> DecodeResult<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> DecodeResult<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub fn i16(&mut self) -> DecodeResult<i16> {
        Ok(i16::from_be_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> DecodeResult<i32> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    pub fn i64(&mut self) -> DecodeResult<i64> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    pub fn f32(&mut self) -> DecodeResult<f32> {
        Ok(f32::from_be_bytes(self.array()?))
    }

    pub fn f64(&mut self) -> DecodeResult<f64> {
        Ok(f64::from_be_bytes(self.array()?))
    }

    /// Vanilla `ByteBufCodecs.BOOL`: any non-zero byte is true.
    pub fn bool(&mut self) -> DecodeResult<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn varint(&mut self) -> DecodeResult<u32> {
        let mut v = 0u32;
        for shift in 0..5 {
            let byte = self.u8()?;
            v |= u32::from(byte & 0x7F) << (shift * 7);
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(self.error("varint too long"))
    }

    pub fn var_i32(&mut self) -> DecodeResult<i32> {
        Ok(self.varint()? as i32)
    }

    pub fn varlong(&mut self) -> DecodeResult<u64> {
        let mut v = 0u64;
        for shift in 0..10 {
            let byte = self.u8()?;
            v |= u64::from(byte & 0x7F) << (shift * 7);
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(self.error("varlong too long"))
    }

    /// A collection length, as vanilla `ByteBufCodecs.readCount`: at most
    /// `max`, and never more than one element per remaining byte so a forged
    /// length can't drive a large allocation.
    pub fn len(&mut self, max: usize) -> DecodeResult<usize> {
        let n = self.varint()? as usize;
        if n > max {
            return Err(self.error("collection over its maximum"));
        }
        if n > self.remaining() {
            return Err(self.error("collection longer than the frame"));
        }
        Ok(n)
    }

    /// `ByteBufCodecs.list(max)` / `collection`: a [`Reader::len`], then
    /// that many elements.
    pub fn list<T>(
        &mut self,
        max: usize,
        mut read: impl FnMut(&mut Self) -> DecodeResult<T>,
    ) -> DecodeResult<Vec<T>> {
        let n = self.len(max)?;
        (0..n).map(|_| read(self)).collect()
    }

    /// A count read some other way, checked against the remaining bytes as
    /// [`Reader::len`] does.
    pub fn len_of(&self, n: usize) -> DecodeResult<usize> {
        if n > self.remaining() {
            return Err(self.error("collection longer than the frame"));
        }
        Ok(n)
    }

    /// Vanilla `Utf8String.read`: a varint byte length of at most `max * 3`
    /// and at most `max` UTF-16 code units.
    pub fn utf(&mut self, max: usize) -> DecodeResult<&'a str> {
        let n = self.varint()? as usize;
        if n > max.saturating_mul(3) {
            return Err(self.error("string over its maximum"));
        }
        let start = self.pos;
        let s = std::str::from_utf8(self.bytes(n)?).map_err(|_| DecodeError {
            what: "invalid UTF-8",
            at: start,
        })?;
        if s.encode_utf16().count() > max {
            return Err(DecodeError {
                what: "string over its maximum",
                at: start,
            });
        }
        Ok(s)
    }

    pub fn identifier(&mut self) -> DecodeResult<&'a str> {
        self.utf(MAX_STRING_LENGTH)
    }

    /// Vanilla `Vec3.STREAM_CODEC`: three doubles.
    pub fn vec3(&mut self) -> DecodeResult<DVec3> {
        Ok(DVec3::new(self.f64()?, self.f64()?, self.f64()?))
    }

    /// Vanilla `Vec3.LP_STREAM_CODEC` ([`crate::wire::read_lp_vec3`]).
    pub fn lp_vec3(&mut self) -> DecodeResult<DVec3> {
        let mut pos = self.pos;
        let v = crate::wire::read_lp_vec3(self.bytes, &mut pos)
            .ok_or_else(|| self.error("truncated"))?;
        self.pos = pos;
        Ok(v)
    }

    /// Vanilla `UUIDUtil.STREAM_CODEC`: two big-endian longs.
    pub fn uuid(&mut self) -> DecodeResult<u128> {
        Ok(u128::from_be_bytes(self.array()?))
    }

    /// `BlockPos.STREAM_CODEC`: x, y and z packed into a long
    /// (`BlockPos.getX`/`getY`/`getZ`).
    pub fn block_pos(&mut self) -> DecodeResult<(i32, i32, i32)> {
        let packed = self.i64()?;
        Ok((
            (packed >> 38) as i32,
            (packed << 52 >> 52) as i32,
            (packed << 26 >> 38) as i32,
        ))
    }

    /// `SectionPos.STREAM_CODEC`: x, y and z packed into a long
    /// (`SectionPos.x`/`y`/`z`).
    pub fn section_pos(&mut self) -> DecodeResult<(i32, i32, i32)> {
        let packed = self.i64()?;
        Ok((
            (packed >> 42) as i32,
            (packed << 44 >> 44) as i32,
            (packed << 22 >> 42) as i32,
        ))
    }

    /// One network-NBT value (type byte + unnamed payload), returned as its
    /// exact bytes.
    pub fn nbt(&mut self) -> DecodeResult<&'a [u8]> {
        let start = self.pos;
        let tag = self.u8()?;
        self.skip_nbt_payload(tag, 0)?;
        Ok(self.since(start))
    }

    /// Skips one NBT payload of the given tag type (vanilla `TagTypes`
    /// layout). Named tags only appear inside compounds.
    pub fn skip_nbt_payload(&mut self, tag: u8, depth: u32) -> DecodeResult<()> {
        if depth > MAX_NBT_DEPTH {
            return Err(self.error("NBT nested too deeply"));
        }
        let array = |r: &mut Self, width: usize| {
            let n = usize::try_from(r.i32()?).map_err(|_| r.error("negative NBT length"))?;
            let n = n
                .checked_mul(width)
                .ok_or_else(|| r.error("NBT array too long"))?;
            r.bytes(n).map(drop)
        };
        match tag {
            0 => Ok(()),
            1 => self.bytes(1).map(drop),
            2 => self.bytes(2).map(drop),
            3 | 5 => self.bytes(4).map(drop),
            4 | 6 => self.bytes(8).map(drop),
            7 => array(self, 1),
            8 => {
                let n = self.u16()?;
                self.bytes(n as usize).map(drop)
            }
            9 => {
                let elem = self.u8()?;
                let n = self.i32()?;
                for _ in 0..n.max(0) {
                    self.skip_nbt_payload(elem, depth + 1)?;
                }
                Ok(())
            }
            10 => loop {
                let elem = self.u8()?;
                if elem == 0 {
                    return Ok(());
                }
                let name_len = self.u16()?;
                self.bytes(name_len as usize)?;
                self.skip_nbt_payload(elem, depth + 1)?;
            },
            11 => array(self, 4),
            12 => array(self, 8),
            _ => Err(self.error("unknown NBT tag type")),
        }
    }

    pub fn ensure_eof(&self) -> DecodeResult<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(self.error("trailing bytes"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_are_big_endian() {
        let mut r = Reader::new(&[0, 0, 0, 7, 0x3F, 0x80, 0, 0, 1]);
        assert_eq!(r.i32(), Ok(7));
        assert_eq!(r.f32(), Ok(1.0));
        assert_eq!(r.bool(), Ok(true));
        assert_eq!(r.ensure_eof(), Ok(()));
    }

    #[test]
    fn varints_reject_overlong_and_truncated_input() {
        assert_eq!(Reader::new(&[0xAC, 0x02]).varint(), Ok(300));
        assert_eq!(
            Reader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).var_i32(),
            Ok(-1)
        );
        assert!(Reader::new(&[0xFF; 6]).varint().is_err());
        assert!(Reader::new(&[0x80]).varint().is_err());
    }

    #[test]
    fn lengths_are_capped_by_maximum_and_frame() {
        assert_eq!(Reader::new(&[2, 0, 0]).len(4), Ok(2));
        assert!(Reader::new(&[5, 0, 0]).len(4).is_err());
        assert!(Reader::new(&[3, 0, 0]).len(100).is_err());
    }

    #[test]
    fn strings_check_byte_and_char_limits() {
        assert_eq!(Reader::new(&[2, b'h', b'i']).utf(2), Ok("hi"));
        assert!(Reader::new(&[3, b'a', b'b', b'c']).utf(2).is_err());
        assert!(Reader::new(&[1, 0xFF]).utf(10).is_err());
    }

    #[test]
    fn nbt_spans_cover_nested_compounds_and_lists() {
        // {a: [1b, 2b]} then a trailing byte.
        let bytes = [10, 9, 0, 1, b'a', 1, 0, 0, 0, 2, 1, 2, 0, 0xEE];
        let mut r = Reader::new(&bytes);
        assert_eq!(r.nbt(), Ok(&bytes[..13]));
        assert_eq!(r.u8(), Ok(0xEE));
        // A bare end tag is the empty value.
        assert_eq!(Reader::new(&[0]).nbt(), Ok(&[0u8][..]));
    }

    #[test]
    fn nbt_rejects_hostile_input() {
        assert!(Reader::new(&[7, 0xFF, 0xFF, 0xFF, 0xFF]).nbt().is_err());
        assert!(Reader::new(&[13]).nbt().is_err());
        let nested = |levels| {
            let mut bytes = vec![9u8];
            bytes.extend(std::iter::repeat_n([9, 0, 0, 0, 1], levels).flatten());
            bytes.extend([0, 0, 0, 0, 0]);
            bytes
        };
        assert!(Reader::new(&nested(100)).nbt().is_ok());
        assert_eq!(
            Reader::new(&nested(600)).nbt().map_err(|e| e.what),
            Err("NBT nested too deeply")
        );
    }
}
