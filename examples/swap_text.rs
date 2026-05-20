//! Diagnostic: take the known-good small-native.rm, replace ONLY the
//! RootText block with a freshly-built one matching rmscene's canonical
//! single-item structure, and write the result.
//!
//! This isolates whether the device's "blank page" rendering is caused by
//! our multi-item RootText output or by something else.
//!
//! Usage:
//!   cargo run --example swap_text -- <ref.rm> "<text>" <out.rm>

use rr::v6::blocks::{Block, ParagraphStyle, RootTextBlock, TextItem, TextItemValue};
use rr::v6::stream::{CrdtId, LwwValue};
use rr::v6::{self, encode, parse};

fn main() {
    let mut args = std::env::args().skip(1);
    let ref_path = args.next().expect("usage: swap_text <ref.rm> <text> <out.rm>");
    let text = args.next().expect("missing text");
    let out_path = args.next().expect("missing out path");

    let ref_bytes = std::fs::read(&ref_path).expect("read ref");
    let mut blocks = parse(&ref_bytes).expect("parse ref");

    // Build a single-item RootText in rmscene's canonical shape.
    // item_id starts at (1, 16); style timestamp is item_id - 1; style
    // keyed at (0,0) → PLAIN.
    let item_id = CrdtId { part1: 1, part2: 16 };
    let style_ts = CrdtId { part1: 1, part2: 15 };

    let new_root_text = RootTextBlock {
        min_version: 1,
        current_version: 1,
        block_id: CrdtId::ZERO,
        items: vec![TextItem {
            item_id,
            left_id: CrdtId::ZERO,
            right_id: CrdtId::ZERO,
            deleted_length: 0,
            value: TextItemValue::Run(text.clone()),
        }],
        styles: vec![(
            CrdtId::ZERO,
            LwwValue {
                timestamp: style_ts,
                value: ParagraphStyle::Plain,
            },
        )],
        pos_x: -468.0,
        pos_y: 234.0,
        width: 936.0,
        extra_data: Vec::new(),
    };

    let mut replaced = false;
    for b in blocks.iter_mut() {
        if matches!(b, Block::RootText(_)) {
            *b = Block::RootText(new_root_text.clone());
            replaced = true;
            break;
        }
    }
    if !replaced {
        panic!("no RootText block in {ref_path}");
    }

    let out_bytes = encode(&blocks).expect("encode");
    std::fs::write(&out_path, &out_bytes).expect("write");
    println!(
        "wrote {} ({} bytes, was {} bytes)",
        out_path,
        out_bytes.len(),
        ref_bytes.len()
    );
    println!("text inserted: {:?}", text);
    assert!(v6::parse(&out_bytes).is_ok(), "round-trip parse failed");
}
