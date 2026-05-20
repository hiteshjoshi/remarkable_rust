//! Tagged-value layer on top of [`Stream`].
//!
//! Block payloads are a sequence of tagged fields. Each field has an "index"
//! (which slot in the parent block it belongs to) and a "tag type" (how to
//! decode the bytes that follow). This module wraps the raw stream with
//! helpers that enforce both: `read_id(1)` reads field 1 expecting it to be
//! a CrdtId; `read_subblock(2)` reads field 2 expecting it to be a length-
//! prefixed nested region; and so on.
//!
//! There's also a `block_frame` helper that handles the 8-byte top-level
//! block envelope (length + version triplet + type), since that framing is
//! one level above the tag stream.

use crate::error::{Error, Result};

use super::stream::{CrdtId, LwwValue, Stream, TagType};

/// Top-level block envelope.
#[derive(Debug, Clone, Copy)]
pub struct BlockFrame {
    pub block_type: u8,
    pub min_version: u8,
    pub current_version: u8,
    /// Absolute byte offset of the payload start.
    pub payload_start: usize,
    /// Payload length in bytes (does not include the 8-byte header).
    pub payload_len: u32,
}

impl BlockFrame {
    pub fn payload_end(&self) -> usize {
        self.payload_start + self.payload_len as usize
    }
}

/// Reader that knows the v6 tag conventions.
pub struct TaggedReader<'a> {
    pub stream: Stream<'a>,
    /// End offset of the active subblock or block; reads aren't allowed to
    /// cross this. `None` means "no active boundary" (we're between blocks).
    boundary: Option<usize>,
}

impl<'a> TaggedReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            stream: Stream::new(buf),
            boundary: None,
        }
    }

    pub fn read_header(&mut self) -> Result<()> {
        self.stream.read_header()
    }

    pub fn is_eof(&self) -> bool {
        self.stream.is_eof()
    }

    pub fn bytes_remaining(&self) -> usize {
        match self.boundary {
            Some(end) => end.saturating_sub(self.stream.position()),
            None => self.stream.len().saturating_sub(self.stream.position()),
        }
    }

    /// Read the next 8-byte top-level block header and set the boundary so
    /// subsequent tagged reads stay inside this block.
    pub fn read_block_frame(&mut self) -> Result<Option<BlockFrame>> {
        if self.stream.is_eof() {
            return Ok(None);
        }
        let payload_len = self.stream.read_u32()?;
        let unknown = self.stream.read_u8()?;
        if unknown != 0 {
            return Err(Error::V6Format(format!(
                "block header byte 4 expected 0, got {unknown}"
            )));
        }
        let min_version = self.stream.read_u8()?;
        let current_version = self.stream.read_u8()?;
        let block_type = self.stream.read_u8()?;
        let payload_start = self.stream.position();
        let frame = BlockFrame {
            block_type,
            min_version,
            current_version,
            payload_start,
            payload_len,
        };
        self.boundary = Some(frame.payload_end());
        Ok(Some(frame))
    }

    /// Advance the cursor to the end of the current block, dropping any
    /// unread trailing bytes. Resets the boundary.
    pub fn finish_block(&mut self, frame: &BlockFrame) -> Result<()> {
        self.stream.seek(frame.payload_end())?;
        self.boundary = None;
        Ok(())
    }

    // ---- field reads -------------------------------------------------------
    //
    // Each read asserts on the expected (index, tag_type) pair. The Python
    // reader recovers from mismatches by rewinding; we do the same — peek
    // first, then commit — so optional fields are cheap.

    fn expect_tag(&mut self, index: u32, ty: TagType) -> Result<()> {
        let pos = self.stream.position();
        let (got_idx, got_ty) = self.stream.read_tag_raw()?;
        if got_idx != index || got_ty != ty {
            return Err(Error::V6Format(format!(
                "at offset {pos}: expected tag (idx={index}, type={ty:?}), got (idx={got_idx}, type={got_ty:?})"
            )));
        }
        Ok(())
    }

    fn check_tag(&mut self, index: u32, ty: TagType) -> bool {
        match self.stream.peek_tag() {
            Ok(Some((idx, t))) => idx == index && t == ty,
            _ => false,
        }
    }

    pub fn read_id(&mut self, index: u32) -> Result<CrdtId> {
        self.expect_tag(index, TagType::Id)?;
        self.stream.read_crdt_id()
    }

    pub fn read_bool(&mut self, index: u32) -> Result<bool> {
        self.expect_tag(index, TagType::Byte1)?;
        self.stream.read_bool()
    }

    pub fn read_byte(&mut self, index: u32) -> Result<u8> {
        self.expect_tag(index, TagType::Byte1)?;
        self.stream.read_u8()
    }

    pub fn read_u32(&mut self, index: u32) -> Result<u32> {
        self.expect_tag(index, TagType::Byte4)?;
        self.stream.read_u32()
    }

    pub fn read_f32(&mut self, index: u32) -> Result<f32> {
        self.expect_tag(index, TagType::Byte4)?;
        self.stream.read_f32()
    }

    pub fn read_f64(&mut self, index: u32) -> Result<f64> {
        self.expect_tag(index, TagType::Byte8)?;
        self.stream.read_f64()
    }

    /// Read a length-prefixed subblock, run `inner` with the boundary set,
    /// then advance past any trailing unread bytes.
    pub fn read_subblock<R>(
        &mut self,
        index: u32,
        inner: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        self.expect_tag(index, TagType::Length4)?;
        let len = self.stream.read_u32()? as usize;
        let start = self.stream.position();
        let end = start + len;
        let saved_boundary = self.boundary.replace(end);
        let result = inner(self);
        // Always restore boundary even on error.
        self.boundary = saved_boundary;
        let value = result?;
        // Caller may not have consumed everything; jump to end.
        self.stream.seek(end)?;
        Ok(value)
    }

    /// `true` if the next field is a subblock at the given index.
    pub fn has_subblock(&mut self, index: u32) -> bool {
        if self.bytes_remaining() == 0 {
            return false;
        }
        self.check_tag(index, TagType::Length4)
    }

    pub fn read_string(&mut self, index: u32) -> Result<String> {
        self.read_subblock(index, |r| {
            let len = r.stream.read_varuint()? as usize;
            let is_ascii = r.stream.read_u8()?;
            if is_ascii != 1 {
                return Err(Error::V6Format(format!(
                    "string is_ascii flag expected 1, got {is_ascii}"
                )));
            }
            let bytes = r.stream.read_bytes(len)?;
            String::from_utf8(bytes.to_vec())
                .map_err(|e| Error::V6Format(format!("invalid utf-8 in string: {e}")))
        })
    }

    pub fn read_lww_id(&mut self, index: u32) -> Result<LwwValue<CrdtId>> {
        self.read_subblock(index, |r| {
            let timestamp = r.read_id(1)?;
            let value = r.read_id(2)?;
            Ok(LwwValue { timestamp, value })
        })
    }

    pub fn read_lww_bool(&mut self, index: u32) -> Result<LwwValue<bool>> {
        self.read_subblock(index, |r| {
            let timestamp = r.read_id(1)?;
            let value = r.read_bool(2)?;
            Ok(LwwValue { timestamp, value })
        })
    }

    pub fn read_lww_byte(&mut self, index: u32) -> Result<LwwValue<u8>> {
        self.read_subblock(index, |r| {
            let timestamp = r.read_id(1)?;
            let value = r.read_byte(2)?;
            Ok(LwwValue { timestamp, value })
        })
    }

    pub fn read_int_pair(&mut self, index: u32) -> Result<(u32, u32)> {
        self.read_subblock(index, |r| {
            let a = r.stream.read_u32()?;
            let b = r.stream.read_u32()?;
            Ok((a, b))
        })
    }
}
