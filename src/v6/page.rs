//! Assemble a complete v6 page from markdown.
//!
//! A reMarkable typed-text page is a sequence of about a dozen blocks; only
//! the [`RootTextBlock`] varies meaningfully between an empty page and one
//! we'd want to ship. The surrounding `SceneTree`, `TreeNode`, and
//! `SceneGroupItem` blocks describe the layer-tree CRDT state, which is
//! uniform across "single-layer typed-text" pages.
//!
//! Rather than reverse-engineer the layer CRDT from scratch, we use raw
//! byte constants extracted from a known-working empty page (see
//! [`SKELETON_PROVENANCE`] for attribution). We do generate the typed
//! blocks — Migration, AuthorIds, PageInfo, SceneInfo — from scratch so
//! the document metadata reflects our actual content.

use uuid::Uuid;

use super::blocks::{
    AuthorIdsBlock, Block, MigrationInfoBlock, PageInfoBlock, RootTextBlock,
    SceneInfoBlock, TextItemValue,
};
use super::stream::{CrdtId, LwwValue};

/// Where the SceneTree/TreeNode/SceneGroupItem skeleton bytes came from.
///
/// The skeleton payload bytes themselves are identical to those in
/// `Normal_AB.rm` from the rmscene fixtures, but Paper Pro firmware
/// requires the two `TreeNode` blocks to declare `current_version = 2`
/// (older firmware shipped them at version 1). The 112-byte SceneInfo
/// `extra_data` blob was also added in newer firmware and the device
/// renders pages as blank without it. Both were captured from a real
/// minimum-viable native notebook off the device's own cloud.
pub const SKELETON_PROVENANCE: &str =
    "skeleton bytes from Normal_AB.rm (MIT-licensed), version+extra_data corrections \
     captured from a fresh Paper Pro native notebook";

// SceneTreeBlock (type 0x01, min=1, cur=1) — adds the root group to the tree.
#[rustfmt::skip]
const SCENE_TREE_ROOT: &[u8] = &[
    0x1F, 0x00, 0x0B, 0x2F, 0x00, 0x00, 0x31, 0x01,
    0x4C, 0x03, 0x00, 0x00, 0x00, 0x1F, 0x00, 0x01,
];

// TreeNodeBlock (type 0x02, min=1, cur=1) — first tree node.
#[rustfmt::skip]
const TREE_NODE_ROOT: &[u8] = &[
    0x1F, 0x00, 0x01, 0x2C, 0x0A, 0x00, 0x00, 0x00,
    0x1F, 0x00, 0x00, 0x2C, 0x02, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x3C, 0x05, 0x00, 0x00, 0x00, 0x1F,
    0x00, 0x00, 0x21, 0x01,
];

// TreeNodeBlock (type 0x02, min=1, cur=1) — node named "Layer 1".
#[rustfmt::skip]
const TREE_NODE_LAYER_1: &[u8] = &[
    0x1F, 0x00, 0x0B, 0x2C, 0x11, 0x00, 0x00, 0x00,
    0x1F, 0x00, 0x0C, 0x2C, 0x09, 0x00, 0x00, 0x00,
    0x07, 0x01, 0x4C, 0x61, 0x79, 0x65, 0x72, 0x20,
    0x31, 0x3C, 0x05, 0x00, 0x00, 0x00, 0x1F, 0x00,
    0x00, 0x21, 0x01,
];

// SceneGroupItemBlock (type 0x04, min=1, cur=1) — Layer 1 group entry.
#[rustfmt::skip]
const SCENE_GROUP_LAYER_1: &[u8] = &[
    0x1F, 0x00, 0x01, 0x2F, 0x00, 0x0D, 0x3F, 0x00,
    0x00, 0x4F, 0x00, 0x00, 0x54, 0x00, 0x00, 0x00,
    0x00, 0x6C, 0x04, 0x00, 0x00, 0x00, 0x02, 0x2F,
    0x00, 0x0B,
];

// SceneInfo trailing fields added in Paper Pro firmware. Three subblocks
// at field indices 6, 7, and 8 carry page-bounds geometry the device
// requires before it will render a page. Captured verbatim from a fresh
// native notebook because the format inside isn't documented; the bytes
// reference the standard 1620 × 2160 Paper Pro drawable surface.
#[rustfmt::skip]
const SCENE_INFO_EXTRA: &[u8] = &[
    0x6C, 0x28, 0x00, 0x00, 0x00, 0x1F, 0x00, 0x00,
    0x2C, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x7C, 0x10, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50,
    0x99, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0xE0,
    0xA0, 0x40, 0x8C, 0x01, 0x18, 0x00, 0x00, 0x00,
    0x1F, 0x01, 0x0F, 0x2C, 0x10, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x99, 0x40,
    0x00, 0x00, 0x00, 0x00, 0x00, 0xE0, 0xA0, 0x40,
    0x9C, 0x01, 0x0A, 0x00, 0x00, 0x00, 0x1F, 0x01,
    0x9B, 0x11, 0x2C, 0x01, 0x00, 0x00, 0x00, 0x00,
];

/// Page-build options. The only required input is the markdown source.
#[derive(Debug, Clone)]
pub struct PageOptions {
    /// Stable UUID stamped on the AuthorIdsBlock. Generate once per
    /// notebook bundle, reuse for all pages in that bundle.
    pub author_uuid: Uuid,
    /// Target device model. Drives `SceneInfo.paper_size`,
    /// `RootTextBlock` frame geometry, and image width caps.
    pub device: Device,
}

impl Default for PageOptions {
    fn default() -> Self {
        Self {
            author_uuid: Uuid::new_v4(),
            device: Device::default(),
        }
    }
}

/// An image to embed on a page, with display geometry resolved.
#[derive(Debug, Clone)]
pub struct PageImage {
    /// The PNG file name used inside the bundle (`<uuid>.png`).
    pub filename: String,
    /// PNG file contents.
    pub png_bytes: Vec<u8>,
    /// Top-left x in page units (origin = page center).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Paper Pro drawable surface in device units. The classic reMarkable 2
/// uses 1404 × 1872; Paper Pro is bigger. SceneInfo and the
/// `customZoom*` fields in `<doc>.content` both need to agree on this.
///
/// These constants are kept for backward-compatibility; new code should
/// prefer [`Device::dimensions`] which switches by model.
pub const PAPER_PRO_WIDTH: u32 = 1620;
pub const PAPER_PRO_HEIGHT: u32 = 2160;

/// reMarkable device model. Each model has its own drawable surface and
/// default text-frame layout. `push` uses this to set `SceneInfo`
/// `paper_size`, the `RootTextBlock` frame dimensions, and the
/// `customZoom*` fields in `<doc>.content`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Device {
    /// reMarkable Paper Pro (10.5" color). 1620 × 2160 drawable area.
    #[default]
    PaperPro,
    /// reMarkable Paper Pro Move (8" color). ~954 × 1696 drawable area.
    PaperProMove,
    /// reMarkable 2 (10.3"). 1404 × 1872 drawable area.
    Rm2,
}

impl Device {
    /// `(width, height)` in device units.
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Device::PaperPro => (1620, 2160),
            Device::PaperProMove => (954, 1696),
            Device::Rm2 => (1404, 1872),
        }
    }

    /// `(pos_x, pos_y, width)` defaults for a typed-text frame on this
    /// device. The frame is centred horizontally at roughly 58 % of the
    /// drawable width, leaving comfortable margins on both sides.
    pub fn text_frame(self) -> (f64, f64, f32) {
        let (w, _h) = self.dimensions();
        let text_w = (w as f32) * 0.58;
        (-(text_w as f64) / 2.0, 234.0, text_w)
    }

    /// Max width to use when embedding raster images (tables) on this
    /// device. About 55 % of the drawable width.
    pub fn max_image_width(self) -> f32 {
        let (w, _h) = self.dimensions();
        (w as f32) * 0.55
    }
}


/// Build a full v6 page block list from markdown, with optional images.
///
/// Block ordering matches the layout the device itself emits for native
/// typed-text pages with images:
/// `AuthorIds, Migration, PageInfo, SceneInfo, ImageRegistry?, SceneTree,
/// RootText, TreeNode, TreeNode, SceneGroupItem, ImageItem*`. The
/// `ImageRegistry` block (0x0E) sits between `SceneInfo` and the first
/// `SceneTree`; `ImageItem` blocks (0x0F) sit at the very end, one per
/// embedded image.
pub fn build_page_with_images(
    md: &str,
    opts: &PageOptions,
    images: &[PageImage],
) -> Vec<Block> {
    use super::image::{build_image_blocks, ImageEntry, PER_IMAGE_ID_SPAN};

    let mut blocks = build_page(md, opts);

    if images.is_empty() {
        return blocks;
    }

    let entries: Vec<ImageEntry> = images
        .iter()
        .map(|i| ImageEntry {
            image_id: rand::random(),
            filename: i.filename.clone(),
            png_bytes: i.png_bytes.clone(),
            x: i.x,
            y: i.y,
            w: i.w,
            h: i.h,
        })
        .collect();

    // Pick a CRDT id base safely past whatever the text content used.
    // Text occupies ids 16..16+len(text); reserve generous headroom plus
    // a slot for each image's 13-id span.
    let text_chars = approximate_text_chars(md);
    let id_base = (16 + text_chars + 100)
        .max(1000)
        .min(u64::MAX - entries.len() as u64 * PER_IMAGE_ID_SPAN);

    let pack = build_image_blocks(&entries, id_base);

    // ----- splice the per-image blocks into the right positions ---------
    //
    // Layout (1-indexed for clarity, 0-indexed below):
    //   0  AuthorIds
    //   1  Migration
    //   2  PageInfo
    //   3  SceneInfo
    //   4  ImageRegistry (← insert here)
    //   5  SceneTree (Layer 1)
    //   6  +N image SceneTrees (← insert here, before RootText)
    //   7  RootText
    //   8  TreeNode (root group)
    //   9  TreeNode (Layer 1)
    //  10  +N image TreeNodes (← insert here, after the layer TreeNodes)
    //  11  SceneGroupItem (Layer 1)
    //  12  +N image SceneGroupItems (← insert here)
    //  13  +N ImageItems (← append at the end)

    // Registry goes right after SceneInfo (index 3 + 1).
    blocks.insert(4, pack.registry);

    // Insert per-image SceneTrees after the Layer 1 SceneTree, which is
    // now at index 5 (was 4 before registry insertion).
    for (offset, st) in pack.scene_trees.into_iter().enumerate() {
        blocks.insert(6 + offset, st);
    }
    // After scene_trees insertion, the original RootText/TreeNode/etc
    // have shifted right by N. Find the position of the second-to-last
    // block in the original layout (SceneGroupItem for Layer 1) and
    // insert image TreeNodes after the two Layer TreeNodes.
    //
    // Easier: scan and insert by type. The Layer 1 SceneGroupItem (type
    // 0x04) is unique in the original block set, so we can find it and
    // insert image TreeNodes BEFORE it; image SceneGroupItems AFTER it.
    let layer_group_idx = blocks
        .iter()
        .position(|b| b.block_type() == 0x04)
        .expect("Layer 1 SceneGroupItem present");
    // Image TreeNodes go just before the Layer 1 SceneGroupItem (after
    // Layer 1's two TreeNodes).
    for (offset, tn) in pack.tree_nodes.into_iter().enumerate() {
        blocks.insert(layer_group_idx + offset, tn);
    }
    // Image SceneGroupItems go just after the Layer 1 SceneGroupItem.
    let layer_group_idx2 = blocks
        .iter()
        .position(|b| b.block_type() == 0x04)
        .expect("Layer 1 SceneGroupItem still present")
        + 1;
    for (offset, sgi) in pack.scene_group_items.into_iter().enumerate() {
        blocks.insert(layer_group_idx2 + offset, sgi);
    }
    // ImageItems at the end.
    for ii in pack.image_items {
        blocks.push(ii);
    }
    blocks
}

/// Rough upper bound on how many CRDT-id slots the text portion uses,
/// without re-running the markdown parser. We want a conservative number;
/// over-allocating wastes nothing because CRDT IDs are sparse.
fn approximate_text_chars(md: &str) -> u64 {
    md.chars().count() as u64
}

/// Build a typed-text page with no images. Kept for backward compatibility
/// and as a focused fast-path for the markdown-only flow.
pub fn build_page(md: &str, opts: &PageOptions) -> Vec<Block> {
    let root_text = super::markdown::markdown_to_root_text_for(md, opts.device);
    let (text_chars, text_lines) = count_chars_and_lines(&root_text);
    let zero_lww_bool = |v: bool| LwwValue {
        timestamp: CrdtId::ZERO,
        value: v,
    };
    let zero_lww_id = LwwValue {
        timestamp: CrdtId::ZERO,
        value: CrdtId::ZERO,
    };

    vec![
        Block::AuthorIds(AuthorIdsBlock {
            min_version: 1,
            current_version: 1,
            authors: vec![(1, opts.author_uuid)],
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
        Block::PageInfo(PageInfoBlock {
            min_version: 0,
            current_version: 1,
            loads_count: 1,
            merges_count: 0,
            text_chars_count: text_chars,
            text_lines_count: text_lines,
            type_code: Some(0),
            extra_data: Vec::new(),
        }),
        Block::SceneInfo(SceneInfoBlock {
            min_version: 0,
            current_version: 1,
            current_layer: zero_lww_id,
            background_visible: Some(zero_lww_bool(true)),
            root_document_visible: Some(zero_lww_bool(true)),
            paper_size: Some(opts.device.dimensions()),
            extra_data: SCENE_INFO_EXTRA.to_vec(),
        }),
        Block::Raw {
            block_type: 0x01,
            min_version: 1,
            current_version: 1,
            payload: SCENE_TREE_ROOT.to_vec(),
        },
        Block::RootText(root_text),
        // TreeNode blocks were bumped to current_version = 2 in newer
        // firmware. Paper Pro renders pages as blank when these claim
        // version 1.
        Block::Raw {
            block_type: 0x02,
            min_version: 1,
            current_version: 2,
            payload: TREE_NODE_ROOT.to_vec(),
        },
        Block::Raw {
            block_type: 0x02,
            min_version: 1,
            current_version: 2,
            payload: TREE_NODE_LAYER_1.to_vec(),
        },
        Block::Raw {
            block_type: 0x04,
            min_version: 1,
            current_version: 1,
            payload: SCENE_GROUP_LAYER_1.to_vec(),
        },
    ]
}

/// Convenience: build a page and serialize it to bytes in one call.
pub fn build_page_bytes(md: &str, opts: &PageOptions) -> crate::Result<Vec<u8>> {
    let blocks = build_page(md, opts);
    super::encode(&blocks)
}

/// Build a page with embedded images and serialize to bytes.
pub fn build_page_bytes_with_images(
    md: &str,
    opts: &PageOptions,
    images: &[PageImage],
) -> crate::Result<Vec<u8>> {
    let blocks = build_page_with_images(md, opts, images);
    super::encode(&blocks)
}

/// Count total characters and paragraphs in a RootTextBlock. PageInfo uses
/// these for the "summary" the device displays in document lists.
fn count_chars_and_lines(rt: &RootTextBlock) -> (u32, u32) {
    let mut chars: u32 = 0;
    for item in &rt.items {
        match &item.value {
            TextItemValue::Run(s) => {
                chars = chars.saturating_add(s.chars().count() as u32);
            }
            TextItemValue::Format(_) => {
                // Inline format codes occupy a CRDT slot but don't render
                // as visible text; xochitl's char count excludes them.
            }
        }
    }
    let lines = u32::try_from(rt.styles.len().max(1)).unwrap_or(u32::MAX);
    (chars, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_page_produces_nine_blocks_in_canonical_order() {
        let blocks = build_page("hello", &PageOptions::default());
        assert_eq!(blocks.len(), 9);
        let kinds: Vec<u8> = blocks.iter().map(Block::block_type).collect();
        assert_eq!(
            kinds,
            vec![0x09, 0x00, 0x0A, 0x0D, 0x01, 0x07, 0x02, 0x02, 0x04]
        );
    }

    #[test]
    fn build_page_bytes_round_trips_via_v6_parse() {
        let bytes = build_page_bytes("# Hi\n\nworld", &PageOptions::default()).unwrap();
        let parsed = crate::v6::parse(&bytes).expect("re-parse");
        assert_eq!(parsed.len(), 9);
    }

    #[test]
    fn page_info_counts_match_content() {
        // "Hi\n" → 3 chars (H, i, \n), 1 paragraph
        let blocks = build_page("Hi", &PageOptions::default());
        let pi = blocks
            .iter()
            .find_map(|b| match b {
                Block::PageInfo(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(pi.text_chars_count, 3);
        assert_eq!(pi.text_lines_count, 1);
    }

    #[test]
    fn provenance_string_is_set() {
        assert!(SKELETON_PROVENANCE.contains("Normal_AB.rm"));
    }
}
