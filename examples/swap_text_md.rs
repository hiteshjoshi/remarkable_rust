//! Diagnostic C: take a markdown file, flatten it to plain text (with
//! `\n\n` between blocks), and build an rmscene-canonical single-item
//! RootText from that flattened text. Swap into a known-good skeleton.
//!
//! Goal: prove that the right *shape* + our content renders correctly.
//! If this works but our current `markdown_to_root_text` doesn't, the fix
//! is to make our generator produce this shape too — not to redo the
//! markdown parsing or the skeleton.
//!
//! Usage:
//!   cargo run --example swap_text_md -- <ref.rm> <input.md> <out.rm>

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use rr::v6::blocks::{Block, ParagraphStyle, RootTextBlock, TextItem, TextItemValue};
use rr::v6::stream::{CrdtId, LwwValue};
use rr::v6::{encode, parse};

fn main() {
    let mut args = std::env::args().skip(1);
    let ref_path = args.next().expect("usage: swap_text_md <ref.rm> <md> <out.rm>");
    let md_path = args.next().expect("missing md path");
    let out_path = args.next().expect("missing out path");

    let ref_bytes = std::fs::read(&ref_path).expect("read ref");
    let mut blocks = parse(&ref_bytes).expect("parse ref");
    let md = std::fs::read_to_string(&md_path).expect("read md");

    // Flatten markdown to plain text. Block-level breaks emit "\n\n"; inline
    // soft breaks become single spaces. No emphasis encoding — strip all
    // inline format codes for this diagnostic.
    let text = flatten(&md);

    let item_id = CrdtId { part1: 1, part2: 16 };
    let style_ts = CrdtId { part1: 1, part2: 15 };

    let new_rt = RootTextBlock {
        min_version: 1,
        current_version: 1,
        block_id: CrdtId::ZERO,
        items: vec![TextItem {
            item_id,
            left_id: CrdtId::ZERO,
            right_id: CrdtId::ZERO,
            deleted_length: 0,
            value: TextItemValue::Run(text),
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

    for b in blocks.iter_mut() {
        if matches!(b, Block::RootText(_)) {
            *b = Block::RootText(new_rt.clone());
            break;
        }
    }

    let out_bytes = encode(&blocks).expect("encode");
    std::fs::write(&out_path, &out_bytes).expect("write");
    println!("wrote {} ({} bytes)", out_path, out_bytes.len());
}

/// Flatten markdown to plain text. Each block (paragraph, heading, list
/// item) is terminated with a single `\n`. We mark blocks with leading
/// glyphs so the on-device output is still scannable: `# ` for headings,
/// `• ` for bullets.
fn flatten(md: &str) -> String {
    let mut out = String::new();
    let mut depth_lists: u32 = 0;
    let mut in_item = false;
    let mut buf = String::new();

    let flush = |buf: &mut String, out: &mut String| {
        if !buf.is_empty() {
            out.push_str(buf.trim_end());
            out.push('\n');
            buf.clear();
        }
    };

    for ev in Parser::new(md) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                let hashes = "#".repeat(level as usize);
                buf.push_str(&hashes);
                buf.push(' ');
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut buf, &mut out);
                out.push('\n');
            }
            Event::Start(Tag::Paragraph) => {
                // List items prefix themselves; nothing extra here.
                let _ = in_item;
            }
            Event::End(TagEnd::Paragraph) => {
                flush(&mut buf, &mut out);
                if !in_item {
                    out.push('\n');
                }
            }
            Event::Start(Tag::List(_)) => {
                depth_lists += 1;
            }
            Event::End(TagEnd::List(_)) => {
                depth_lists = depth_lists.saturating_sub(1);
                if depth_lists == 0 {
                    out.push('\n');
                }
            }
            Event::Start(Tag::Item) => {
                in_item = true;
                let indent = "  ".repeat(depth_lists.saturating_sub(1) as usize);
                buf.push_str(&indent);
                buf.push_str("• ");
            }
            Event::End(TagEnd::Item) => {
                flush(&mut buf, &mut out);
                in_item = false;
            }
            Event::Text(s) | Event::Code(s) => {
                buf.push_str(&s);
            }
            Event::SoftBreak | Event::HardBreak => buf.push(' '),
            _ => {}
        }
    }
    flush(&mut buf, &mut out);
    // collapse 3+ newlines to 2 so blank-line separators stay clean
    let mut cleaned = String::with_capacity(out.len());
    let mut nl_run = 0usize;
    for c in out.chars() {
        if c == '\n' {
            nl_run += 1;
            if nl_run <= 2 {
                cleaned.push(c);
            }
        } else {
            nl_run = 0;
            cleaned.push(c);
        }
    }
    cleaned.trim_end().to_string() + "\n"
}
