//! Tagged-value layer on the write side.
//!
//! Mirrors [`super::reader::TaggedReader`]. Output goes into an owned
//! `Vec<u8>` — we never write in place. Block envelope and subblock length
//! prefixes are handled with a placeholder-and-patch pattern: reserve four
//! zero bytes for the length, write the content, then go back and fill in
//! the correct value once we know the size.

use crate::error::{Error, Result};

use super::stream::{CrdtId, LwwValue, Stream, TagType};

/// Builder over an owned `Vec<u8>`. Cheap to construct, no internal cursor —
/// position is always `bytes.len()`.
pub struct TaggedWriter {
    pub bytes: Vec<u8>,
}

impl TaggedWriter {
    pub fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn write_header(&mut self) {
        self.bytes.extend_from_slice(super::HEADER_V6);
    }

    fn write_u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn write_f32(&mut self, v: f32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn write_f64(&mut self, v: f64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn write_varuint(&mut self, v: u64) {
        Stream::write_varuint_into(&mut self.bytes, v);
    }

    fn write_tag(&mut self, index: u32, ty: TagType) {
        Stream::write_tag_into(&mut self.bytes, index, ty);
    }

    fn write_crdt_id_raw(&mut self, id: CrdtId) {
        Stream::write_crdt_id_into(&mut self.bytes, id);
    }

    // ---- tagged scalars ---------------------------------------------------

    pub fn write_id(&mut self, index: u32, id: CrdtId) {
        self.write_tag(index, TagType::Id);
        self.write_crdt_id_raw(id);
    }

    pub fn write_bool(&mut self, index: u32, v: bool) {
        self.write_tag(index, TagType::Byte1);
        self.write_u8(u8::from(v));
    }

    pub fn write_byte(&mut self, index: u32, v: u8) {
        self.write_tag(index, TagType::Byte1);
        self.write_u8(v);
    }

    pub fn write_u32_tagged(&mut self, index: u32, v: u32) {
        self.write_tag(index, TagType::Byte4);
        self.write_u32(v);
    }

    pub fn write_f32_tagged(&mut self, index: u32, v: f32) {
        self.write_tag(index, TagType::Byte4);
        self.write_f32(v);
    }

    pub fn write_f64_tagged(&mut self, index: u32, v: f64) {
        self.write_tag(index, TagType::Byte8);
        self.write_f64(v);
    }

    // ---- subblock framing -------------------------------------------------

    /// Write a length-prefixed subblock. The closure writes the content; we
    /// take care of the tag, the 4-byte length placeholder, and patching the
    /// length once the content size is known.
    pub fn write_subblock<R>(
        &mut self,
        index: u32,
        inner: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        self.write_tag(index, TagType::Length4);
        let len_pos = self.bytes.len();
        self.write_u32(0);
        let content_start = self.bytes.len();
        let value = inner(self)?;
        let content_end = self.bytes.len();
        let len = u32::try_from(content_end - content_start).map_err(|_| {
            Error::V6Format("subblock content exceeds 4 GiB".into())
        })?;
        self.bytes[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());
        Ok(value)
    }

    // ---- LWW helpers ------------------------------------------------------

    pub fn write_lww_id(&mut self, index: u32, lww: LwwValue<CrdtId>) -> Result<()> {
        self.write_subblock(index, |w| {
            w.write_id(1, lww.timestamp);
            w.write_id(2, lww.value);
            Ok(())
        })
    }

    pub fn write_lww_bool(&mut self, index: u32, lww: LwwValue<bool>) -> Result<()> {
        self.write_subblock(index, |w| {
            w.write_id(1, lww.timestamp);
            w.write_bool(2, lww.value);
            Ok(())
        })
    }

    pub fn write_lww_byte(&mut self, index: u32, lww: LwwValue<u8>) -> Result<()> {
        self.write_subblock(index, |w| {
            w.write_id(1, lww.timestamp);
            w.write_byte(2, lww.value);
            Ok(())
        })
    }

    pub fn write_int_pair(&mut self, index: u32, pair: (u32, u32)) -> Result<()> {
        self.write_subblock(index, |w| {
            w.write_u32(pair.0);
            w.write_u32(pair.1);
            Ok(())
        })
    }

    pub fn write_string(&mut self, index: u32, s: &str) -> Result<()> {
        self.write_subblock(index, |w| {
            w.write_varuint(s.len() as u64);
            w.write_u8(1); // is_ascii flag — always 1 in observed files
            w.bytes.extend_from_slice(s.as_bytes());
            Ok(())
        })
    }

    /// Write a string subblock with an optional trailing 4-byte format code.
    pub fn write_string_with_format(
        &mut self,
        index: u32,
        s: &str,
        fmt: Option<u32>,
    ) -> Result<()> {
        self.write_subblock(index, |w| {
            w.write_varuint(s.len() as u64);
            w.write_u8(1);
            w.bytes.extend_from_slice(s.as_bytes());
            if let Some(code) = fmt {
                w.write_u32_tagged(2, code);
            }
            Ok(())
        })
    }

    // ---- block envelope ---------------------------------------------------

    /// Write a top-level block: 4-byte length, 1 byte 0, min_version,
    /// current_version, block_type, then payload from `inner`.
    pub fn write_block<R>(
        &mut self,
        block_type: u8,
        min_version: u8,
        current_version: u8,
        inner: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        let len_pos = self.bytes.len();
        self.write_u32(0); // placeholder for payload_len
        self.write_u8(0); // unknown — always 0
        self.write_u8(min_version);
        self.write_u8(current_version);
        self.write_u8(block_type);
        let payload_start = self.bytes.len();
        let value = inner(self)?;
        let payload_end = self.bytes.len();
        let payload_len = u32::try_from(payload_end - payload_start)
            .map_err(|_| Error::V6Format("block payload exceeds 4 GiB".into()))?;
        self.bytes[len_pos..len_pos + 4].copy_from_slice(&payload_len.to_le_bytes());
        Ok(value)
    }

    /// Append raw bytes verbatim. Used by `Block::Raw` for block types we
    /// don't model fully yet.
    pub fn write_raw_bytes(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
    }

    /// Append a raw `u8` (no tag). Used inside fixed-shape subblocks.
    pub fn write_raw_u8(&mut self, v: u8) {
        self.write_u8(v);
    }

    /// Append a raw `u16` (no tag).
    pub fn write_raw_u16(&mut self, v: u16) {
        self.write_u16(v);
    }

    /// Append a varuint without a tag.
    pub fn write_raw_varuint(&mut self, v: u64) {
        self.write_varuint(v);
    }

    /// Append a raw CrdtId (no tag) — used for fixed-shape structures where
    /// the parent's frame implies the type.
    pub fn write_raw_crdt_id(&mut self, id: CrdtId) {
        self.write_crdt_id_raw(id);
    }

    /// Append `n` raw bytes verbatim from a slice.
    pub fn write_raw_slice(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
    }
}

impl Default for TaggedWriter {
    fn default() -> Self {
        Self::new()
    }
}
