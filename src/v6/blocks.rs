//! Decoders for known v6 block types.
//!
//! Phase 0 scope: read everything we need to round-trip-validate against
//! rmscene's fixtures. The crown jewel is [`RootTextBlock`] — that's the
//! block our `self_push` path will *write* in Phase 1+.
//!
//! Blocks we don't fully parse yet are stored as [`Block::Raw`] (block type
//! plus raw payload bytes). That keeps the decoder resilient: a fixture with
//! a stroke block we don't understand still parses, we just preserve the
//! bytes verbatim for later round-trip use.

use uuid::Uuid;

use crate::error::{Error, Result};

use super::reader::{BlockFrame, TaggedReader};
use super::stream::{CrdtId, LwwValue};
use super::writer::TaggedWriter;

/// One top-level block. We decode the types we care about and stash the rest
/// as raw bytes so we can still round-trip arbitrary fixtures.
///
/// Typed blocks carry an `extra_data` slot that captures any trailing bytes
/// the typed decoder didn't consume — newer firmware versions append fields
/// our decoder doesn't model yet, and preserving them is the only way to
/// guarantee byte-equal round-trips against arbitrary on-device files.
#[derive(Debug, Clone)]
pub enum Block {
    Migration(MigrationInfoBlock),
    AuthorIds(AuthorIdsBlock),
    PageInfo(PageInfoBlock),
    SceneInfo(SceneInfoBlock),
    RootText(RootTextBlock),
    Raw {
        block_type: u8,
        min_version: u8,
        current_version: u8,
        payload: Vec<u8>,
    },
}

impl Block {
    pub fn block_type(&self) -> u8 {
        match self {
            Block::Migration(_) => 0x00,
            Block::AuthorIds(_) => 0x09,
            Block::PageInfo(_) => 0x0A,
            Block::SceneInfo(_) => 0x0D,
            Block::RootText(_) => 0x07,
            Block::Raw { block_type, .. } => *block_type,
        }
    }

    /// `(min_version, current_version)` declared in the block envelope.
    pub fn versions(&self) -> (u8, u8) {
        match self {
            Block::Migration(b) => (b.min_version, b.current_version),
            Block::AuthorIds(b) => (b.min_version, b.current_version),
            Block::PageInfo(b) => (b.min_version, b.current_version),
            Block::SceneInfo(b) => (b.min_version, b.current_version),
            Block::RootText(b) => (b.min_version, b.current_version),
            Block::Raw {
                min_version,
                current_version,
                ..
            } => (*min_version, *current_version),
        }
    }

    pub fn read(reader: &mut TaggedReader<'_>) -> Result<Self> {
        let frame = reader
            .read_block_frame()?
            .ok_or_else(|| Error::V6Format("expected block, got EOF".into()))?;
        let mut block = match frame.block_type {
            0x00 => Block::Migration(MigrationInfoBlock::from_stream(reader, &frame)?),
            0x09 => Block::AuthorIds(AuthorIdsBlock::from_stream(reader, &frame)?),
            0x0A => Block::PageInfo(PageInfoBlock::from_stream(reader, &frame)?),
            0x0D => Block::SceneInfo(SceneInfoBlock::from_stream(reader, &frame)?),
            0x07 => Block::RootText(RootTextBlock::from_stream(reader, &frame)?),
            _ => {
                let bytes = reader
                    .stream
                    .read_bytes(frame.payload_len as usize)?
                    .to_vec();
                Block::Raw {
                    block_type: frame.block_type,
                    min_version: frame.min_version,
                    current_version: frame.current_version,
                    payload: bytes,
                }
            }
        };

        // Capture any bytes the typed decoder didn't consume. These are
        // fields newer firmware added that we don't model yet; preserving
        // them lets us round-trip arbitrary on-device files byte-for-byte.
        // (Raw blocks consume the whole payload above, so there's nothing
        // left to capture for them.)
        if let Some(slot) = block.extra_data_mut() {
            let remaining = frame.payload_end().saturating_sub(reader.stream.position());
            if remaining > 0 {
                *slot = reader.stream.read_bytes(remaining)?.to_vec();
            }
        }
        reader.finish_block(&frame)?;
        Ok(block)
    }

    fn extra_data_mut(&mut self) -> Option<&mut Vec<u8>> {
        match self {
            Block::Migration(b) => Some(&mut b.extra_data),
            Block::AuthorIds(b) => Some(&mut b.extra_data),
            Block::PageInfo(b) => Some(&mut b.extra_data),
            Block::SceneInfo(b) => Some(&mut b.extra_data),
            Block::RootText(b) => Some(&mut b.extra_data),
            Block::Raw { .. } => None,
        }
    }

    fn extra_data(&self) -> &[u8] {
        match self {
            Block::Migration(b) => &b.extra_data,
            Block::AuthorIds(b) => &b.extra_data,
            Block::PageInfo(b) => &b.extra_data,
            Block::SceneInfo(b) => &b.extra_data,
            Block::RootText(b) => &b.extra_data,
            Block::Raw { .. } => &[],
        }
    }

    pub fn write(&self, w: &mut TaggedWriter) -> Result<()> {
        let (min_v, cur_v) = self.versions();
        let block_type = self.block_type();
        w.write_block(block_type, min_v, cur_v, |w| {
            match self {
                Block::Migration(b) => b.write_payload(w)?,
                Block::AuthorIds(b) => b.write_payload(w)?,
                Block::PageInfo(b) => b.write_payload(w)?,
                Block::SceneInfo(b) => b.write_payload(w)?,
                Block::RootText(b) => b.write_payload(w)?,
                Block::Raw { payload, .. } => w.write_raw_slice(payload),
            }
            w.write_raw_slice(self.extra_data());
            Ok(())
        })
    }
}

// ---------- 0x00 MigrationInfoBlock --------------------------------------

#[derive(Debug, Clone)]
pub struct MigrationInfoBlock {
    pub min_version: u8,
    pub current_version: u8,
    pub migration_id: CrdtId,
    pub is_device: bool,
    /// Added in firmware v3.2.2. `None` means the field was not present in
    /// the source file — preserve that so re-encode produces byte-identical
    /// output for older fixtures.
    pub unknown: Option<bool>,
    /// Trailing bytes inside the block envelope that the typed decoder
    /// didn't consume — newer firmware may add fields we don't model.
    pub extra_data: Vec<u8>,
}

impl MigrationInfoBlock {
    fn from_stream(r: &mut TaggedReader<'_>, frame: &BlockFrame) -> Result<Self> {
        let migration_id = r.read_id(1)?;
        let is_device = r.read_bool(2)?;
        let unknown = if r.bytes_remaining() > 0 {
            Some(r.read_bool(3)?)
        } else {
            None
        };
        Ok(Self {
            min_version: frame.min_version,
            current_version: frame.current_version,
            migration_id,
            is_device,
            unknown,
            extra_data: Vec::new(),
        })
    }

    fn write_payload(&self, w: &mut TaggedWriter) -> Result<()> {
        w.write_id(1, self.migration_id);
        w.write_bool(2, self.is_device);
        if let Some(u) = self.unknown {
            w.write_bool(3, u);
        }
        Ok(())
    }
}

// ---------- 0x09 AuthorIdsBlock ------------------------------------------

#[derive(Debug, Clone)]
pub struct AuthorIdsBlock {
    pub min_version: u8,
    pub current_version: u8,
    /// Author entries in original file order. Using a `Vec` rather than a
    /// map: xochitl writes authors in a specific order and round-trip
    /// byte-equality depends on preserving it.
    pub authors: Vec<(u16, Uuid)>,
    pub extra_data: Vec<u8>,
}

impl AuthorIdsBlock {
    fn from_stream(r: &mut TaggedReader<'_>, frame: &BlockFrame) -> Result<Self> {
        let n = r.stream.read_varuint()? as usize;
        let mut authors = Vec::with_capacity(n);
        for _ in 0..n {
            r.read_subblock(0, |r| {
                let uuid_len = r.stream.read_varuint()? as usize;
                if uuid_len != 16 {
                    return Err(Error::V6Format(format!(
                        "expected 16-byte UUID, got {uuid_len}"
                    )));
                }
                let bytes = r.stream.read_bytes(16)?;
                // The UUID is stored "bytes_le" — same layout used by the
                // .NET Guid binary form. uuid::Uuid::from_bytes_le matches.
                let mut buf = [0u8; 16];
                buf.copy_from_slice(bytes);
                let uuid = Uuid::from_bytes_le(buf);
                let author_id = r.stream.read_u16()?;
                authors.push((author_id, uuid));
                Ok(())
            })?;
        }
        Ok(Self {
            min_version: frame.min_version,
            current_version: frame.current_version,
            authors,
            extra_data: Vec::new(),
        })
    }

    fn write_payload(&self, w: &mut TaggedWriter) -> Result<()> {
        w.write_raw_varuint(self.authors.len() as u64);
        for (author_id, uuid) in &self.authors {
            w.write_subblock(0, |w| {
                let bytes = uuid.to_bytes_le();
                w.write_raw_varuint(bytes.len() as u64);
                w.write_raw_slice(&bytes);
                w.write_raw_u16(*author_id);
                Ok(())
            })?;
        }
        Ok(())
    }
}

// ---------- 0x0A PageInfoBlock -------------------------------------------

#[derive(Debug, Clone)]
pub struct PageInfoBlock {
    pub min_version: u8,
    pub current_version: u8,
    pub loads_count: u32,
    pub merges_count: u32,
    pub text_chars_count: u32,
    pub text_lines_count: u32,
    /// Type code stamped on the page by xochitl. Optional in older files.
    pub type_code: Option<u32>,
    pub extra_data: Vec<u8>,
}

impl PageInfoBlock {
    fn from_stream(r: &mut TaggedReader<'_>, frame: &BlockFrame) -> Result<Self> {
        let loads_count = r.read_u32(1)?;
        let merges_count = r.read_u32(2)?;
        let text_chars_count = r.read_u32(3)?;
        let text_lines_count = r.read_u32(4)?;
        let type_code = if r.bytes_remaining() > 0 {
            Some(r.read_u32(5)?)
        } else {
            None
        };
        Ok(Self {
            min_version: frame.min_version,
            current_version: frame.current_version,
            loads_count,
            merges_count,
            text_chars_count,
            text_lines_count,
            type_code,
            extra_data: Vec::new(),
        })
    }

    fn write_payload(&self, w: &mut TaggedWriter) -> Result<()> {
        w.write_u32_tagged(1, self.loads_count);
        w.write_u32_tagged(2, self.merges_count);
        w.write_u32_tagged(3, self.text_chars_count);
        w.write_u32_tagged(4, self.text_lines_count);
        if let Some(code) = self.type_code {
            w.write_u32_tagged(5, code);
        }
        Ok(())
    }
}

// ---------- 0x0D SceneInfoBlock ------------------------------------------

#[derive(Debug, Clone)]
pub struct SceneInfoBlock {
    pub min_version: u8,
    pub current_version: u8,
    pub current_layer: LwwValue<CrdtId>,
    pub background_visible: Option<LwwValue<bool>>,
    pub root_document_visible: Option<LwwValue<bool>>,
    pub paper_size: Option<(u32, u32)>,
    pub extra_data: Vec<u8>,
}

impl SceneInfoBlock {
    fn from_stream(r: &mut TaggedReader<'_>, frame: &BlockFrame) -> Result<Self> {
        let current_layer = r.read_lww_id(1)?;
        let background_visible = if r.bytes_remaining() > 0 {
            Some(r.read_lww_bool(2)?)
        } else {
            None
        };
        let root_document_visible = if r.bytes_remaining() > 0 {
            Some(r.read_lww_bool(3)?)
        } else {
            None
        };
        let paper_size = if r.bytes_remaining() > 0 {
            Some(r.read_int_pair(5)?)
        } else {
            None
        };
        Ok(Self {
            min_version: frame.min_version,
            current_version: frame.current_version,
            current_layer,
            background_visible,
            root_document_visible,
            paper_size,
            extra_data: Vec::new(),
        })
    }

    fn write_payload(&self, w: &mut TaggedWriter) -> Result<()> {
        w.write_lww_id(1, self.current_layer)?;
        if let Some(v) = self.background_visible {
            w.write_lww_bool(2, v)?;
        }
        if let Some(v) = self.root_document_visible {
            w.write_lww_bool(3, v)?;
        }
        if let Some(pair) = self.paper_size {
            w.write_int_pair(5, pair)?;
        }
        Ok(())
    }
}

// ---------- 0x07 RootTextBlock — the typed-text payload ------------------

/// Paragraph styles recognised by xochitl's typed-text renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ParagraphStyle {
    Basic = 0,
    Plain = 1,
    Heading = 2,
    Bold = 3,
    Bullet = 4,
    Bullet2 = 5,
    Checkbox = 6,
    CheckboxChecked = 7,
}

impl ParagraphStyle {
    pub fn from_u8(n: u8) -> Result<Self> {
        match n {
            0 => Ok(Self::Basic),
            1 => Ok(Self::Plain),
            2 => Ok(Self::Heading),
            3 => Ok(Self::Bold),
            4 => Ok(Self::Bullet),
            5 => Ok(Self::Bullet2),
            6 => Ok(Self::Checkbox),
            7 => Ok(Self::CheckboxChecked),
            // Newer firmware introduced style codes beyond 7 (e.g. 10
            // observed on Paper Pro). We don't model them but want decode
            // to survive — fall back to Plain so callers can still read
            // the rest of the file.
            _ => Ok(Self::Plain),
        }
    }
}

/// A single entry in the text CRDT sequence. Either a run of characters
/// (`Run`) or an inline formatting code (`Format`).
#[derive(Debug, Clone)]
pub enum TextItemValue {
    /// Run of UTF-8 characters. The `item_id` identifies the first char;
    /// subsequent chars have implicit sequential ids.
    Run(String),
    /// Inline formatting: 1=bold-on, 2=bold-off, 3=italic-on, 4=italic-off.
    Format(u32),
}

#[derive(Debug, Clone)]
pub struct TextItem {
    pub item_id: CrdtId,
    pub left_id: CrdtId,
    pub right_id: CrdtId,
    pub deleted_length: u32,
    pub value: TextItemValue,
}

#[derive(Debug, Clone)]
pub struct RootTextBlock {
    pub min_version: u8,
    pub current_version: u8,
    pub block_id: CrdtId,
    pub items: Vec<TextItem>,
    /// Paragraph styles keyed by the CrdtId of the first character of the
    /// paragraph. `CrdtId::ZERO` is a sentinel meaning "the trailing
    /// paragraph after the last `\n`." Stored as a Vec to preserve file
    /// order for byte-equal round-trips.
    pub styles: Vec<(CrdtId, LwwValue<ParagraphStyle>)>,
    pub pos_x: f64,
    pub pos_y: f64,
    pub width: f32,
    pub extra_data: Vec<u8>,
}

impl RootTextBlock {
    fn from_stream(r: &mut TaggedReader<'_>, frame: &BlockFrame) -> Result<Self> {
        let block_id = r.read_id(1)?;

        let (items, styles) = r.read_subblock(2, |r| {
            let items = r.read_subblock(1, |r| {
                r.read_subblock(1, |r| {
                    let n = r.stream.read_varuint()? as usize;
                    let mut out = Vec::with_capacity(n);
                    for _ in 0..n {
                        out.push(read_text_item(r)?);
                    }
                    Ok(out)
                })
            })?;

            let styles = r.read_subblock(2, |r| {
                r.read_subblock(1, |r| {
                    let n = r.stream.read_varuint()? as usize;
                    let mut out = Vec::with_capacity(n);
                    for _ in 0..n {
                        out.push(read_text_format(r)?);
                    }
                    Ok(out)
                })
            })?;

            Ok((items, styles))
        })?;

        let (pos_x, pos_y) = r.read_subblock(3, |r| {
            let x = r.stream.read_f64()?;
            let y = r.stream.read_f64()?;
            Ok((x, y))
        })?;
        let width = r.read_f32(4)?;

        Ok(Self {
            min_version: frame.min_version,
            current_version: frame.current_version,
            block_id,
            items,
            styles,
            pos_x,
            pos_y,
            width,
            extra_data: Vec::new(),
        })
    }

    fn write_payload(&self, w: &mut TaggedWriter) -> Result<()> {
        w.write_id(1, self.block_id);

        w.write_subblock(2, |w| {
            // Text items wrapper (field 1) → inner subblock (field 1)
            w.write_subblock(1, |w| {
                w.write_subblock(1, |w| {
                    w.write_raw_varuint(self.items.len() as u64);
                    for item in &self.items {
                        write_text_item(item, w)?;
                    }
                    Ok(())
                })
            })?;

            // Formatting wrapper (field 2) → inner subblock (field 1)
            w.write_subblock(2, |w| {
                w.write_subblock(1, |w| {
                    w.write_raw_varuint(self.styles.len() as u64);
                    for (char_id, lww) in &self.styles {
                        write_text_format(*char_id, *lww, w)?;
                    }
                    Ok(())
                })
            })?;

            Ok(())
        })?;

        w.write_subblock(3, |w| {
            w.bytes.extend_from_slice(&self.pos_x.to_le_bytes());
            w.bytes.extend_from_slice(&self.pos_y.to_le_bytes());
            Ok(())
        })?;

        w.write_f32_tagged(4, self.width);
        Ok(())
    }
}

fn read_text_item(r: &mut TaggedReader<'_>) -> Result<TextItem> {
    r.read_subblock(0, |r| {
        let item_id = r.read_id(2)?;
        let left_id = r.read_id(3)?;
        let right_id = r.read_id(4)?;
        let deleted_length = r.read_u32(5)?;
        // Field 6 is either a string (subblock) or a 4-byte int format code.
        // String comes through `read_string_with_format`: text + optional
        // format code in field 2. For our purposes, a deleted-length>0
        // item carries an empty string; a normal item carries either a
        // string or a single format-int that lives at index 6 directly.
        // rmscene's read flow: try string first; if not present, value is
        // the format int. We mirror that.
        let value = if r.has_subblock(6) {
            let (text, fmt) = read_string_with_format(r, 6)?;
            match fmt {
                Some(code) if text.is_empty() => TextItemValue::Format(code),
                _ => TextItemValue::Run(text),
            }
        } else {
            // Some items legitimately have no value (deleted markers).
            TextItemValue::Run(String::new())
        };
        Ok(TextItem {
            item_id,
            left_id,
            right_id,
            deleted_length,
            value,
        })
    })
}

fn read_string_with_format(r: &mut TaggedReader<'_>, index: u32) -> Result<(String, Option<u32>)> {
    use super::stream::TagType;
    r.read_subblock(index, |r| {
        let len = r.stream.read_varuint()? as usize;
        let is_ascii = r.stream.read_u8()?;
        if is_ascii != 1 {
            return Err(Error::V6Format(format!(
                "string is_ascii flag expected 1, got {is_ascii}"
            )));
        }
        let bytes = r.stream.read_bytes(len)?;
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|e| Error::V6Format(format!("invalid utf-8: {e}")))?;
        // Format code is optional. rmscene checks the tag explicitly rather
        // than guessing from "is there data left," because the trailing
        // bytes of the subblock may legitimately not be a format code.
        let fmt = match r.stream.peek_tag()? {
            Some((2, TagType::Byte4)) => Some(r.read_u32(2)?),
            _ => None,
        };
        Ok((text, fmt))
    })
}

/// Decode one text-format entry from the formats wrapper.
///
/// The entries in this section are appended back-to-back — they are *not*
/// individually wrapped in a subblock. From rmscene:
///
/// 1. `char_id`: a **raw** CrdtId (no preceding tag).
/// 2. `timestamp`: a tagged ID at field index 1.
/// 3. Subblock at field index 2 containing two raw bytes: the first must
///    be 17 (0x11) — its meaning is undocumented — and the second is the
///    `ParagraphStyle` enum value.
fn read_text_format(r: &mut TaggedReader<'_>) -> Result<(CrdtId, LwwValue<ParagraphStyle>)> {
    let char_id = r.stream.read_crdt_id()?;
    let timestamp = r.read_id(1)?;
    let value = r.read_subblock(2, |r| {
        let marker = r.stream.read_u8()?;
        if marker != 17 {
            return Err(Error::V6Format(format!(
                "text format marker byte expected 17, got {marker}"
            )));
        }
        let style_byte = r.stream.read_u8()?;
        ParagraphStyle::from_u8(style_byte)
    })?;
    Ok((char_id, LwwValue { timestamp, value }))
}

fn write_text_item(item: &TextItem, w: &mut TaggedWriter) -> Result<()> {
    w.write_subblock(0, |w| {
        w.write_id(2, item.item_id);
        w.write_id(3, item.left_id);
        w.write_id(4, item.right_id);
        w.write_u32_tagged(5, item.deleted_length);
        match &item.value {
            // rmscene only emits field 6 when the value is non-empty (str or
            // int format code). Empty runs produce no subblock at all.
            TextItemValue::Run(s) if s.is_empty() => {}
            TextItemValue::Run(s) => {
                w.write_string(6, s)?;
            }
            TextItemValue::Format(code) => {
                // Format codes are stored as an empty string + trailing
                // u32-tagged format code, per rmscene's
                // `write_string_with_format(6, "", item.value)`.
                w.write_string_with_format(6, "", Some(*code))?;
            }
        }
        Ok(())
    })
}

fn write_text_format(
    char_id: CrdtId,
    lww: LwwValue<ParagraphStyle>,
    w: &mut TaggedWriter,
) -> Result<()> {
    // Raw char_id (no tag), tagged timestamp at field 1, then a subblock
    // (field 2) holding [0x11, style_code]. Mirrors the rmscene encoder.
    w.write_raw_crdt_id(char_id);
    w.write_id(1, lww.timestamp);
    w.write_subblock(2, |w| {
        w.write_raw_u8(17);
        w.write_raw_u8(lww.value as u8);
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v6::HEADER_V6;

    #[test]
    fn rejects_bad_header() {
        let bytes = b"not a remarkable file";
        let mut r = TaggedReader::new(bytes);
        assert!(r.read_header().is_err());
    }

    #[test]
    fn accepts_v6_header() {
        let mut r = TaggedReader::new(HEADER_V6);
        r.read_header().unwrap();
        assert!(r.is_eof());
    }
}
