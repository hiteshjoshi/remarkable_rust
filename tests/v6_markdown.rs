//! End-to-end: markdown → `RootTextBlock` → encode → re-decode.
//!
//! This proves the Phase 2 path produces a well-formed v6 text block: bytes
//! that round-trip through our own parser without losing structure. It
//! does *not* yet prove the bytes work on a real Paper Pro — that's Phase
//! 4 territory once we have SSH transport.

use rr::v6::blocks::{Block, ParagraphStyle, TextItemValue};
use rr::v6::markdown::markdown_to_root_text;
use rr::v6::stream::CrdtId;
use rr::v6::{self};

/// Wrap a `RootTextBlock` in just enough of a block list to be a valid
/// v6 file. For the test we only need the RootText itself plus the
/// MigrationInfo/AuthorIds header that every observed fixture carries —
/// xochitl wouldn't *open* this without a full notebook bundle around it,
/// but our decoder doesn't care, and that's what we're validating today.
fn wrap_minimal(root: rr::v6::blocks::RootTextBlock) -> Vec<Block> {
    use rr::v6::blocks::{AuthorIdsBlock, MigrationInfoBlock};
    use uuid::Uuid;

    vec![
        Block::AuthorIds(AuthorIdsBlock {
            min_version: 1,
            current_version: 1,
            authors: vec![(1, Uuid::nil())],
            extra_data: Vec::new(),
        }),
        Block::Migration(MigrationInfoBlock {
            min_version: 1,
            current_version: 1,
            migration_id: CrdtId { part1: 1, part2: 1 },
            is_device: false,
            unknown: Some(false),
            extra_data: Vec::new(),
        }),
        Block::RootText(root),
    ]
}

#[test]
fn empty_markdown_encodes_to_a_file_with_only_metadata() {
    let blocks = wrap_minimal(markdown_to_root_text(""));
    let bytes = v6::encode(&blocks).unwrap();
    let parsed = v6::parse(&bytes).unwrap();
    assert_eq!(parsed.len(), 3);
    match &parsed[2] {
        Block::RootText(rt) => {
            assert!(rt.items.is_empty());
            assert!(rt.styles.is_empty());
        }
        _ => panic!("expected RootText at index 2"),
    }
}

#[test]
fn heading_plus_paragraph_plus_bullets_round_trips() {
    let md = "# Title\n\nIntro line with **bold** and *italic*.\n\n- first\n- second\n";
    let blocks = wrap_minimal(markdown_to_root_text(md));
    let bytes = v6::encode(&blocks).unwrap();
    let parsed = v6::parse(&bytes).unwrap();

    let rt = parsed
        .iter()
        .find_map(|b| match b {
            Block::RootText(rt) => Some(rt),
            _ => None,
        })
        .expect("RootText present");

    // Paragraph styles, in document order:
    let style_seq: Vec<_> = rt.styles.iter().map(|(_, l)| l.value).collect();
    assert_eq!(
        style_seq,
        vec![
            ParagraphStyle::Heading,
            ParagraphStyle::Plain,
            ParagraphStyle::Bullet,
            ParagraphStyle::Bullet,
        ]
    );

    // Inline emphasis (bold/italic) is intentionally dropped — the device
    // doesn't render our multi-item CRDT layout correctly, so we keep the
    // text content as a single Run and rely on plain text. Format codes
    // (1/2/3/4) should NOT appear.
    let format_codes: Vec<u32> = rt
        .items
        .iter()
        .filter_map(|i| match &i.value {
            TextItemValue::Format(c) => Some(*c),
            _ => None,
        })
        .collect();
    assert!(format_codes.is_empty(), "format codes should be dropped, got {format_codes:?}");

    // Concatenated text runs preserve the source content (modulo soft
    // breaks → spaces and trailing paragraph \n).
    let text: String = rt
        .items
        .iter()
        .filter_map(|i| match &i.value {
            TextItemValue::Run(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("Title"));
    assert!(text.contains("Intro line with"));
    assert!(text.contains("bold"));
    assert!(text.contains("italic"));
    assert!(text.contains("first"));
    assert!(text.contains("second"));
}

#[test]
fn double_encode_is_idempotent() {
    let md = "## Section\n\nA paragraph.\n\n- one\n- two\n";
    let blocks_a = wrap_minimal(markdown_to_root_text(md));
    let bytes_a = v6::encode(&blocks_a).unwrap();

    // Decode then re-encode; should produce identical bytes.
    let blocks_b = v6::parse(&bytes_a).unwrap();
    let bytes_b = v6::encode(&blocks_b).unwrap();

    assert_eq!(bytes_a, bytes_b, "encode is not idempotent");
}

#[test]
fn nested_bullets_use_bullet2_style() {
    let md = "- outer\n  - inner\n";
    let blocks = wrap_minimal(markdown_to_root_text(md));
    let bytes = v6::encode(&blocks).unwrap();
    let parsed = v6::parse(&bytes).unwrap();
    let rt = parsed
        .iter()
        .find_map(|b| match b {
            Block::RootText(rt) => Some(rt),
            _ => None,
        })
        .unwrap();
    let styles: Vec<_> = rt.styles.iter().map(|(_, l)| l.value).collect();
    assert!(styles.contains(&ParagraphStyle::Bullet2));
    assert!(styles.contains(&ParagraphStyle::Bullet));
}
