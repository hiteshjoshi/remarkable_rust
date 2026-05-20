//! reMarkable v6 `.rm` binary format — decoder + (future) encoder.
//!
//! See `tools/v6-samples/FORMAT.md` for the bytewise format reference. The
//! module is laid out as:
//!
//! - [`stream`]: byte-level cursor over a `.rm` file (varuint, CrdtId, header).
//! - [`reader`]: tagged-value layer on top of `stream` (subblock + tag types).
//! - [`blocks`]: top-level block decoders, including [`blocks::RootTextBlock`]
//!   which carries the typed-text payload our `self_push` path will write.
//!
//! Phase 0 ships read-only support. Writers land in Phase 1 (`encoder.rs`)
//! once we have round-trip parity against the rmscene fixtures.

pub mod blocks;
pub mod image;
pub mod markdown;
pub mod page;
pub mod reader;
pub mod stream;
pub mod writer;

pub use blocks::Block;
pub use reader::TaggedReader;
pub use stream::{CrdtId, LwwValue, TagType};
pub use writer::TaggedWriter;

/// File header. Exactly 43 bytes. The trailing spaces are required.
pub const HEADER_V6: &[u8; 43] = b"reMarkable .lines file, version=6          ";

/// Parse a complete `.rm` v6 file into a vector of [`Block`].
pub fn parse(bytes: &[u8]) -> crate::Result<Vec<Block>> {
    let mut reader = TaggedReader::new(bytes);
    reader.read_header()?;
    let mut blocks = Vec::new();
    while !reader.is_eof() {
        blocks.push(Block::read(&mut reader)?);
    }
    Ok(blocks)
}

/// Serialize a slice of [`Block`]s into a complete `.rm` v6 file (header +
/// block stream). Round-trips: `parse(encode(parse(bytes))) == parse(bytes)`,
/// and on fixtures whose payload we fully model, `encode(parse(bytes)) ==
/// bytes` byte-for-byte.
pub fn encode(blocks: &[Block]) -> crate::Result<Vec<u8>> {
    let mut w = TaggedWriter::new();
    w.write_header();
    for block in blocks {
        block.write(&mut w)?;
    }
    Ok(w.into_bytes())
}
