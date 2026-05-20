//! Byte-level cursor over a `.rm` v6 file.
//!
//! Mirrors `rmscene.tagged_block_common.DataStream` but operates on a borrowed
//! `&[u8]` rather than a Python `BinaryIO`. All multi-byte integers are
//! little-endian. Errors use [`crate::Error::V6Format`] with the byte offset
//! where the parse went sideways — debugging a binary format without offsets
//! is misery.

use crate::error::{Error, Result};

use super::HEADER_V6;

/// A reMarkable CRDT identifier. Two parts: a small "author"-shaped byte and
/// a varuint payload. Treat as an opaque, ordered identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CrdtId {
    pub part1: u8,
    pub part2: u64,
}

impl CrdtId {
    pub const ZERO: CrdtId = CrdtId { part1: 0, part2: 0 };
}

/// Last-write-wins container, stamped with a `CrdtId` clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LwwValue<T> {
    pub timestamp: CrdtId,
    pub value: T,
}

/// Low 4-bit tag classifying the data that follows.
///
/// The encoded tag byte is `(field_index << 4) | tag_type`, packed as a
/// varuint so high field indices stay compact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TagType {
    /// CrdtId.
    Id = 0xF,
    /// u32 length + that many bytes (a subblock).
    Length4 = 0xC,
    /// 8 raw bytes (f64).
    Byte8 = 0x8,
    /// 4 raw bytes (u32 / f32).
    Byte4 = 0x4,
    /// 1 raw byte (bool / u8).
    Byte1 = 0x1,
}

impl TagType {
    fn from_nibble(n: u8) -> Result<Self> {
        match n {
            0xF => Ok(TagType::Id),
            0xC => Ok(TagType::Length4),
            0x8 => Ok(TagType::Byte8),
            0x4 => Ok(TagType::Byte4),
            0x1 => Ok(TagType::Byte1),
            _ => Err(Error::V6Format(format!("unknown tag type 0x{n:X}"))),
        }
    }
}

/// Borrowed cursor over a v6 byte stream.
pub struct Stream<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Stream<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn is_eof(&self) -> bool {
        self.pos >= self.buf.len()
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.buf.len() {
            return Err(Error::V6Format(format!(
                "seek to {pos} past end {}",
                self.buf.len()
            )));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| {
            Error::V6Format(format!("read_bytes({n}) overflow at {}", self.pos))
        })?;
        if end > self.buf.len() {
            return Err(Error::V6Format(format!(
                "read_bytes({n}) past eof at {} (len={})",
                self.pos,
                self.buf.len()
            )));
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn read_header(&mut self) -> Result<()> {
        let h = self.read_bytes(HEADER_V6.len())?;
        if h != HEADER_V6 {
            return Err(Error::V6Format(format!(
                "wrong header: got {:?}, expected reMarkable v6 header",
                std::str::from_utf8(h).unwrap_or("<invalid utf-8>")
            )));
        }
        Ok(())
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_bytes(1)?[0])
    }

    pub fn read_u16(&mut self) -> Result<u16> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    pub fn read_f64(&mut self) -> Result<f64> {
        let b = self.read_bytes(8)?;
        let arr = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        Ok(f64::from_le_bytes(arr))
    }

    pub fn read_bool(&mut self) -> Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    /// LEB128 varuint, 7 bits per byte, high bit = continuation. Cap at 10
    /// bytes (more would overflow u64).
    pub fn read_varuint(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0;
        for _ in 0..10 {
            let b = self.read_u8()?;
            result |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err(Error::V6Format(format!(
            "varuint overflow at offset {}",
            self.pos - 10
        )))
    }

    pub fn read_crdt_id(&mut self) -> Result<CrdtId> {
        let part1 = self.read_u8()?;
        let part2 = self.read_varuint()?;
        Ok(CrdtId { part1, part2 })
    }

    /// Encode a varuint into the supplied vector.
    pub fn write_varuint_into(buf: &mut Vec<u8>, mut value: u64) {
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                buf.push(byte);
                return;
            }
            buf.push(byte | 0x80);
        }
    }

    /// Encode `(index << 4) | tag_type` as a varuint.
    pub fn write_tag_into(buf: &mut Vec<u8>, index: u32, ty: TagType) {
        let raw = (u64::from(index) << 4) | (ty as u64);
        Self::write_varuint_into(buf, raw);
    }

    /// Encode a CrdtId (u8 + varuint).
    pub fn write_crdt_id_into(buf: &mut Vec<u8>, id: CrdtId) {
        buf.push(id.part1);
        Self::write_varuint_into(buf, id.part2);
    }

    /// Peek the next tag (index, tag_type) without consuming it.
    pub fn peek_tag(&mut self) -> Result<Option<(u32, TagType)>> {
        if self.is_eof() {
            return Ok(None);
        }
        let saved = self.pos;
        let result = self.read_tag_raw();
        self.pos = saved;
        result.map(Some)
    }

    /// Read the next tag and decode (index, tag_type). Advances the cursor.
    pub fn read_tag_raw(&mut self) -> Result<(u32, TagType)> {
        let raw = self.read_varuint()?;
        let tag_type = TagType::from_nibble((raw & 0xF) as u8)?;
        let index = u32::try_from(raw >> 4).map_err(|_| {
            Error::V6Format(format!("tag index {} overflows u32", raw >> 4))
        })?;
        Ok((index, tag_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varuint_roundtrip_examples() {
        let cases: &[(&[u8], u64)] = &[
            (&[0x00], 0),
            (&[0x01], 1),
            (&[0x7F], 127),
            (&[0x80, 0x01], 128),
            (&[0xAC, 0x02], 300),
        ];
        for (bytes, expected) in cases {
            let mut s = Stream::new(bytes);
            assert_eq!(s.read_varuint().unwrap(), *expected, "decoding {bytes:?}");
        }
    }

    #[test]
    fn tag_byte_splits_into_index_and_type() {
        // (index=1, type=Length4=0xC) → (1<<4)|0xC = 0x1C, varuint = single byte 0x1C
        let mut s = Stream::new(&[0x1C]);
        let (idx, ty) = s.read_tag_raw().unwrap();
        assert_eq!(idx, 1);
        assert_eq!(ty, TagType::Length4);
    }

    #[test]
    fn varuint_write_then_read_roundtrips() {
        let cases: &[u64] = &[0, 1, 127, 128, 300, 16_383, 16_384, u64::MAX / 2];
        for &v in cases {
            let mut buf = Vec::new();
            Stream::write_varuint_into(&mut buf, v);
            let mut s = Stream::new(&buf);
            assert_eq!(s.read_varuint().unwrap(), v, "value {v}");
            assert!(s.is_eof(), "trailing bytes after writing {v}");
        }
    }

    #[test]
    fn tag_write_then_read_roundtrips() {
        let mut buf = Vec::new();
        Stream::write_tag_into(&mut buf, 5, TagType::Byte4);
        let mut s = Stream::new(&buf);
        let (idx, ty) = s.read_tag_raw().unwrap();
        assert_eq!(idx, 5);
        assert_eq!(ty, TagType::Byte4);
    }
}
