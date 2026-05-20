//! Convert markdown source into a v6 [`RootTextBlock`] in the shape the
//! reMarkable Paper Pro device actually renders.
//!
//! The device is picky about the text-CRDT layout. After bisecting against
//! real on-device notebooks we converged on this canonical shape, which
//! matches both rmscene's `simple_text_document` reference and the
//! structure of typed-text notebooks the device writes itself:
//!
//! - **One [`TextItem`] per document** holding the entire text as a single
//!   [`TextItemValue::Run`] with embedded `\n` characters between
//!   paragraphs.
//! - `item_id = CrdtId { part1: 1, part2: 16 }`, `left_id = right_id =
//!   CrdtId::ZERO`. The base of 16 is conventional — the device's first
//!   character lives at that ID slot.
//! - **Per-paragraph styles** keyed by the `CrdtId` of the `\n` character
//!   that *starts* the paragraph. The very first paragraph keys at
//!   `CrdtId::ZERO` since there is no opening newline.
//! - Inline bold/italic format codes are dropped. Re-introducing them
//!   would require multiple text items in a doubly-linked sequence, which
//!   the device renders correctly only when the CRDT IDs line up exactly
//!   the way a real device-typed session would lay them out — non-trivial
//!   to reproduce from scratch.
//!
//! Markdown mappings:
//! - `# Heading` (any depth) → [`ParagraphStyle::Heading`]
//! - paragraphs → [`ParagraphStyle::Plain`]
//! - top-level `-` items → [`ParagraphStyle::Bullet`]
//! - nested `-` items → [`ParagraphStyle::Bullet2`]
//! - numbered list items → [`ParagraphStyle::Bullet`] (no native numbered style)
//! - inline `code`, **bold**, *italic* → rendered as plain text inline

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::blocks::{ParagraphStyle, RootTextBlock, TextItem, TextItemValue};
use super::page::Device;
use super::stream::{CrdtId, LwwValue};

/// Page layout defaults for Paper Pro, matching rmscene's
/// `simple_text_document`. Pass a [`Device`] to
/// [`markdown_to_root_text_for`] to override.
pub const DEFAULT_POS_X: f64 = -468.0;
pub const DEFAULT_POS_Y: f64 = 234.0;
pub const DEFAULT_WIDTH: f32 = 936.0;

/// First-character CrdtId. Both rmscene's reference and observed
/// device-written notebooks start the text CRDT at this slot, leaving room
/// at low IDs for system blocks.
const FIRST_CHAR_ID: CrdtId = CrdtId { part1: 1, part2: 16 };

/// Convert markdown text into a [`RootTextBlock`] using Paper Pro
/// defaults. Use [`markdown_to_root_text_for`] to target a different
/// device.
pub fn markdown_to_root_text(md: &str) -> RootTextBlock {
    markdown_to_root_text_for(md, Device::PaperPro)
}

/// Convert markdown into a [`RootTextBlock`] sized for the given device.
pub fn markdown_to_root_text_for(md: &str, device: Device) -> RootTextBlock {
    let (pos_x, pos_y, width) = device.text_frame();
    let paragraphs = parse_paragraphs(md);

    if paragraphs.is_empty() {
        return RootTextBlock {
            min_version: 1,
            current_version: 1,
            block_id: CrdtId::ZERO,
            items: Vec::new(),
            styles: Vec::new(),
            pos_x,
            pos_y,
            width,
            extra_data: Vec::new(),
        };
    }

    // Flatten into one text string with `\n` separators between paragraphs.
    let mut text = String::new();
    for p in &paragraphs {
        text.push_str(&p.text);
        text.push('\n');
    }

    // Compute per-paragraph style entries. The style key is the CrdtId of
    // the paragraph's opening character — `CrdtId::ZERO` for the first
    // paragraph, and the CrdtId of the `\n` that terminated the previous
    // paragraph for every subsequent one.
    let mut styles = Vec::with_capacity(paragraphs.len());
    let mut chars_before: u64 = 0;
    for (i, p) in paragraphs.iter().enumerate() {
        let key = if i == 0 {
            CrdtId::ZERO
        } else {
            // The `\n` that ENDS paragraph (i-1) is at position
            // (chars_before - 1) relative to text start; its CrdtId is
            // therefore `FIRST_CHAR_ID.part2 + (chars_before - 1)`.
            CrdtId {
                part1: FIRST_CHAR_ID.part1,
                part2: FIRST_CHAR_ID.part2 + chars_before - 1,
            }
        };
        // Style timestamp matches the observed device convention: one slot
        // past the style key, except for the implicit `(0, 0)` slot where
        // we use `FIRST_CHAR_ID.part2 - 1` (one slot before the first char).
        let timestamp = if i == 0 {
            CrdtId {
                part1: 1,
                part2: FIRST_CHAR_ID.part2 - 1,
            }
        } else {
            CrdtId {
                part1: key.part1,
                part2: key.part2 + 1,
            }
        };
        styles.push((
            key,
            LwwValue {
                timestamp,
                value: p.style,
            },
        ));
        chars_before += p.text.chars().count() as u64 + 1; // +1 for the trailing \n
    }

    let items = vec![TextItem {
        item_id: FIRST_CHAR_ID,
        left_id: CrdtId::ZERO,
        right_id: CrdtId::ZERO,
        deleted_length: 0,
        value: TextItemValue::Run(text),
    }];

    RootTextBlock {
        min_version: 1,
        current_version: 1,
        block_id: CrdtId::ZERO,
        items,
        styles,
        pos_x,
        pos_y,
        width,
        extra_data: Vec::new(),
    }
}

struct Para {
    text: String,
    style: ParagraphStyle,
}

/// Walk pulldown-cmark events and emit one [`Para`] per logical block.
fn parse_paragraphs(md: &str) -> Vec<Para> {
    let mut paragraphs = Vec::new();
    let mut list_depth: u32 = 0;
    let mut current: Option<Para> = None;

    let push_to_current = |current: &mut Option<Para>, s: &str| {
        if let Some(p) = current {
            p.text.push_str(s);
        }
    };

    let open = |current: &mut Option<Para>, style: ParagraphStyle| {
        // If a paragraph was open without being closed (rare), commit it.
        if current.is_some() {
            // Don't bother — let it get flushed by the next close. Replacing
            // it would discard text we may still be receiving events for.
        }
        if current.is_none() {
            *current = Some(Para {
                text: String::new(),
                style,
            });
        }
    };

    let close = |current: &mut Option<Para>, paragraphs: &mut Vec<Para>| {
        if let Some(p) = current.take() {
            // Trim trailing whitespace; empty paragraphs are still kept so
            // markdown blank-line breaks survive.
            paragraphs.push(p);
        }
    };

    for ev in Parser::new(md) {
        match ev {
            Event::Start(Tag::Heading { .. }) => {
                close(&mut current, &mut paragraphs);
                open(&mut current, ParagraphStyle::Heading);
            }
            Event::End(TagEnd::Heading(_)) => {
                close(&mut current, &mut paragraphs);
            }
            Event::Start(Tag::Paragraph) if current.is_none() => {
                open(&mut current, ParagraphStyle::Plain);
            }
            Event::End(TagEnd::Paragraph) if list_depth == 0 => {
                close(&mut current, &mut paragraphs);
            }
            Event::Start(Tag::List(_)) => {
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
            }
            Event::Start(Tag::Item) => {
                close(&mut current, &mut paragraphs);
                let style = if list_depth >= 2 {
                    ParagraphStyle::Bullet2
                } else {
                    ParagraphStyle::Bullet
                };
                open(&mut current, style);
            }
            Event::End(TagEnd::Item) => {
                close(&mut current, &mut paragraphs);
            }
            Event::Text(s) | Event::Code(s) => {
                if current.is_none() {
                    open(&mut current, ParagraphStyle::Plain);
                }
                push_to_current(&mut current, &s);
            }
            Event::SoftBreak | Event::HardBreak => {
                push_to_current(&mut current, " ");
            }
            // Inline emphasis events (Start/End Strong/Emphasis) are ignored
            // — the device's typed-text engine can express them, but only
            // through a multi-item CRDT layout we don't yet generate
            // correctly. Plain text gets the content across reliably.
            _ => {}
        }
    }
    close(&mut current, &mut paragraphs);

    // Drop empty paragraphs that pulldown-cmark sometimes emits between
    // structural blocks. They contribute no visible content and adding a
    // style entry for them just clutters the output.
    paragraphs.retain(|p| !p.text.is_empty());

    paragraphs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_block() {
        let rt = markdown_to_root_text("");
        assert!(rt.items.is_empty());
        assert!(rt.styles.is_empty());
    }

    #[test]
    fn single_paragraph_produces_one_item_one_style() {
        let rt = markdown_to_root_text("hello world");
        assert_eq!(rt.items.len(), 1);
        assert_eq!(rt.styles.len(), 1);
        assert_eq!(rt.styles[0].0, CrdtId::ZERO);
        assert_eq!(rt.styles[0].1.value, ParagraphStyle::Plain);
        match &rt.items[0].value {
            TextItemValue::Run(s) => assert_eq!(s, "hello world\n"),
            _ => panic!("expected Run"),
        }
        // Canonical IDs.
        assert_eq!(rt.items[0].item_id, FIRST_CHAR_ID);
        assert_eq!(rt.items[0].left_id, CrdtId::ZERO);
        assert_eq!(rt.items[0].right_id, CrdtId::ZERO);
    }

    #[test]
    fn heading_then_paragraph_keys_styles_at_right_crdt_ids() {
        let rt = markdown_to_root_text("# Title\n\nbody");
        assert_eq!(rt.items.len(), 1);
        assert_eq!(rt.styles.len(), 2);
        // First style is at (0, 0) → Heading
        assert_eq!(rt.styles[0].0, CrdtId::ZERO);
        assert_eq!(rt.styles[0].1.value, ParagraphStyle::Heading);
        // Second style is keyed at the CrdtId of the newline that ended
        // "Title": position 5 (T=0, i=1, t=2, l=3, e=4, \n=5).
        let expected = CrdtId {
            part1: 1,
            part2: FIRST_CHAR_ID.part2 + 5,
        };
        assert_eq!(rt.styles[1].0, expected);
        assert_eq!(rt.styles[1].1.value, ParagraphStyle::Plain);

        // The flat text should be "Title\nbody\n".
        let TextItemValue::Run(s) = &rt.items[0].value else {
            panic!()
        };
        assert_eq!(s, "Title\nbody\n");
    }

    #[test]
    fn bullets_and_nested_bullets_use_bullet_styles() {
        let rt = markdown_to_root_text("- outer\n  - inner\n");
        let styles: Vec<_> = rt.styles.iter().map(|(_, l)| l.value).collect();
        assert!(styles.contains(&ParagraphStyle::Bullet));
        assert!(styles.contains(&ParagraphStyle::Bullet2));
    }

    #[test]
    fn inline_emphasis_is_dropped_to_plain() {
        let rt = markdown_to_root_text("hi **bold** there");
        // Single Run, no Format items.
        assert_eq!(rt.items.len(), 1);
        let TextItemValue::Run(s) = &rt.items[0].value else {
            panic!()
        };
        // Inline emphasis text survives without format codes.
        assert!(s.contains("hi"));
        assert!(s.contains("bold"));
        assert!(s.contains("there"));
    }
}
