//! Integration tests: decode rmscene's reference fixtures and assert
//! recognisable structure. These act as the regression suite for our v6
//! decoder against known-good files produced by the canonical reference
//! implementation.
//!
//! Fixtures live in `tools/v6-samples/` (copied from
//! <https://github.com/ricklupton/rmscene/tree/main/tests/data>).

use std::path::PathBuf;

use rr::v6::{self, Block};

fn sample(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tools/v6-samples")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

#[test]
fn header_is_43_bytes() {
    assert_eq!(v6::HEADER_V6.len(), 43);
}

#[test]
fn decode_bold_heading_bullet_normal() {
    let bytes = sample("Bold_Heading_Bullet_Normal.rm");
    let blocks = v6::parse(&bytes).expect("parse fixture");
    assert!(!blocks.is_empty(), "expected at least one block");

    // Block ordering isn't fixed across rmscene fixtures, so just assert
    // that the expected types are *present*.
    let has_migration = blocks.iter().any(|b| matches!(b, Block::Migration(_)));
    assert!(has_migration, "missing MigrationInfoBlock");

    let has_authors = blocks.iter().any(|b| matches!(b, Block::AuthorIds(_)));
    assert!(has_authors, "missing AuthorIdsBlock");

    let has_root_text = blocks.iter().any(|b| matches!(b, Block::RootText(_)));
    assert!(has_root_text, "missing RootTextBlock");

    if let Some(Block::RootText(rt)) = blocks.iter().find(|b| matches!(b, Block::RootText(_))) {
        // Render the items into a debug string and confirm the fixture's
        // characteristic content survives the round-trip.
        let mut text = String::new();
        for item in &rt.items {
            if let v6::blocks::TextItemValue::Run(s) = &item.value {
                text.push_str(s);
            }
        }
        // The fixture was named for its content; look for at least one of
        // the expected words.
        assert!(
            text.to_lowercase().contains("bullet")
                || text.to_lowercase().contains("heading")
                || text.to_lowercase().contains("bold")
                || text.contains("letter of the alphabet"),
            "RootText doesn't contain expected fixture text. Got: {text:?}"
        );
    }
}

#[test]
fn decode_normal_ab_smoke() {
    // Smallest fixture (358 bytes). If this fails, the framing is broken.
    let bytes = sample("Normal_AB.rm");
    let blocks = v6::parse(&bytes).expect("parse Normal_AB");
    assert!(!blocks.is_empty());
}

#[test]
fn decode_all_fixtures_no_panic() {
    // Sanity sweep: parse every fixture. We don't assert content because
    // many of them contain blocks we don't fully understand yet (strokes,
    // glyph ranges, etc.) — those go through Block::Raw without panicking.
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools/v6-samples");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("read v6-samples") {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rm") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let result = v6::parse(&bytes);
        assert!(result.is_ok(), "failed to parse {path:?}: {result:?}");
        count += 1;
    }
    assert!(count >= 10, "expected ≥10 fixtures, got {count}");
}
