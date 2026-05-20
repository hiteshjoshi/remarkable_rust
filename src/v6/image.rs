//! Image embedding for native v6 notebooks.
//!
//! Embedding one image on a page requires **five new blocks** that
//! current firmware introduced. We extracted the exact wire format by
//! diffing real image-bearing pages off the user's account:
//!
//! - `0x0E` `ImageRegistry` (min/cur 3/3) — one block per page. Lists
//!   every image: 16-byte id, filename, plus two LwwValue wrappers.
//! - `0x01` `SceneTree` — per image. Creates a new group node in the
//!   author-1 CRDT space parented to Layer 1's tree.
//! - `0x02` `TreeNode` (min/cur 1/2) — per image. Defines the new
//!   group's anchor: `anchor_id`, `anchor_type` (=2), `anchor_threshold`,
//!   `anchor_origin_x`. The anchor info is what makes the image stick to
//!   a position on the page.
//! - `0x04` `SceneGroupItem` — per image. Places the new group into the
//!   SceneTree under Layer 1. Subsequent groups chain via `left_id`.
//! - `0x0F` `ImageItem` (min/cur 2/2) — per image. References the
//!   registry id, carries four quad corners and triangle indices.
//!
//! Each image consumes a block of 13 sequential CrdtIds in the author-1
//! space. The layout per image (offsets from a per-image base):
//!
//! | +0  | filename LwwValue.timestamp (registry)         |
//! | +1  | u8 marker LwwValue.timestamp (registry)        |
//! | +2  | image-item value tag-2 id                      |
//! | +3  | image-item value tag-1 (hash subblock) ts      |
//! | +4  | TreeNode.anchor_id.value                       |
//! | +7  | group node_id (used everywhere this group is referenced) |
//! | +8  | SceneGroupItem.item_id                         |
//! | +9  | TreeNode.anchor_id.timestamp                   |
//! | +10 | TreeNode.anchor_type.timestamp                 |
//! | +11 | TreeNode.anchor_threshold.timestamp            |
//! | +12 | ImageItem.item_id                              |

use super::blocks::Block;
use super::stream::{CrdtId, Stream, TagType};

/// One image to embed on a page. Bundle code uploads `png_bytes` to
/// `<doc>/<page-uuid>/<filename>`; `build_image_blocks` emits the v6
/// blocks pointing at it.
pub struct ImageEntry {
    /// 16-byte identifier. Generated randomly per image — the device
    /// uses it only to match a registry entry to an ImageItem.
    pub image_id: [u8; 16],
    /// Display name on disk: `<uuid>.png`.
    pub filename: String,
    /// PNG file contents (kept here for the bundle, not used in the .rm).
    pub png_bytes: Vec<u8>,
    /// Top-left corner of the image in page units.
    pub x: f32,
    pub y: f32,
    /// Display size in page units.
    pub w: f32,
    pub h: f32,
}

/// IDs allocated per image. Each image gets a contiguous block of 13.
struct ImageIds {
    reg_ts1: CrdtId,
    reg_ts2: CrdtId,
    item_ref_id2: CrdtId,
    item_ref_id1: CrdtId,
    anchor_value: CrdtId,
    group_node: CrdtId,
    group_item: CrdtId,
    anchor_id_ts: CrdtId,
    anchor_type_ts: CrdtId,
    anchor_threshold_ts: CrdtId,
    image_item: CrdtId,
}

impl ImageIds {
    fn allocate(base: u64) -> Self {
        let make = |off: u64| CrdtId {
            part1: 1,
            part2: base + off,
        };
        Self {
            reg_ts1: make(0),
            reg_ts2: make(1),
            item_ref_id2: make(2),
            item_ref_id1: make(3),
            anchor_value: make(4),
            group_node: make(7),
            group_item: make(8),
            anchor_id_ts: make(9),
            anchor_type_ts: make(10),
            anchor_threshold_ts: make(11),
            image_item: make(12),
        }
    }
}

/// CRDT IDs consumed per image. Pick `base` to skip past whatever text
/// CRDT IDs the page already uses.
pub const PER_IMAGE_ID_SPAN: u64 = 13;

/// Result of `build_image_blocks` — the blocks that go in each phase of
/// the page's v6 stream.
pub struct ImageBlocks {
    /// One [`Block::Raw`] of type `0x0E`. Goes after `SceneInfo`.
    pub registry: Block,
    /// One per image. Goes after Layer 1's [`Block::Raw`] of type `0x01`.
    pub scene_trees: Vec<Block>,
    /// One per image. Goes after Layer 1's two TreeNode blocks.
    pub tree_nodes: Vec<Block>,
    /// One per image. Goes after Layer 1's [`Block::Raw`] of type `0x04`.
    pub scene_group_items: Vec<Block>,
    /// One per image. Goes at the very end of the block stream.
    pub image_items: Vec<Block>,
}

/// Build the full set of blocks needed to embed `images` into a page.
/// `id_base` is the CrdtId.part2 value used by the first image; each
/// subsequent image takes `PER_IMAGE_ID_SPAN` more.
pub fn build_image_blocks(images: &[ImageEntry], id_base: u64) -> ImageBlocks {
    // Per-image id blocks.
    let ids: Vec<ImageIds> = (0..images.len())
        .map(|i| ImageIds::allocate(id_base + (i as u64) * PER_IMAGE_ID_SPAN))
        .collect();

    let layer_one_group_root = CrdtId { part1: 0, part2: 11 };

    // 0x0E registry: one block listing every image.
    let registry = build_registry_block(images, &ids);

    // 0x01 SceneTree per image — attaches the image group under Layer 1.
    let scene_trees: Vec<Block> = ids
        .iter()
        .map(|id| build_image_scene_tree(id.group_node, layer_one_group_root))
        .collect();

    // 0x02 TreeNode per image — defines the group with its anchor info.
    let tree_nodes: Vec<Block> = ids.iter().map(build_image_tree_node).collect();

    // 0x04 SceneGroupItem per image — places the group in the SceneTree.
    // Successive groups chain via `left_id` to the previous group's
    // `item_id`, mirroring how the device writes them.
    let scene_group_items: Vec<Block> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let left_id = if i == 0 {
                CrdtId::ZERO
            } else {
                ids[i - 1].group_item
            };
            build_image_scene_group_item(layer_one_group_root, id.group_item, left_id, id.group_node)
        })
        .collect();

    // 0x0F ImageItem per image — the textured-quad payload.
    let image_items: Vec<Block> = images
        .iter()
        .zip(ids.iter())
        .map(|(img, id)| build_image_item(img, id))
        .collect();

    ImageBlocks {
        registry,
        scene_trees,
        tree_nodes,
        scene_group_items,
        image_items,
    }
}

// ---- per-block builders ------------------------------------------------

fn build_registry_block(images: &[ImageEntry], ids: &[ImageIds]) -> Block {
    let mut bytes = Vec::new();
    Stream::write_tag_into(&mut bytes, 1, TagType::Length4);
    let outer_len_pos = bytes.len();
    bytes.extend_from_slice(&[0u8; 4]);
    let outer_content_start = bytes.len();

    Stream::write_varuint_into(&mut bytes, images.len() as u64);
    for (img, id) in images.iter().zip(ids.iter()) {
        Stream::write_tag_into(&mut bytes, 0, TagType::Length4);
        let entry_len_pos = bytes.len();
        bytes.extend_from_slice(&[0u8; 4]);
        let entry_start = bytes.len();

        // 16-byte image id (raw).
        bytes.extend_from_slice(&img.image_id);
        // LwwValue<string> at tag 1 — filename.
        write_lww_string(&mut bytes, 1, id.reg_ts1, &img.filename);
        // LwwValue<u8-marker> at tag 2 — observed as a constant `17, 0`.
        write_lww_u8_marker(&mut bytes, 2, id.reg_ts2, 0);

        let entry_len = (bytes.len() - entry_start) as u32;
        bytes[entry_len_pos..entry_len_pos + 4].copy_from_slice(&entry_len.to_le_bytes());
    }

    let outer_len = (bytes.len() - outer_content_start) as u32;
    bytes[outer_len_pos..outer_len_pos + 4].copy_from_slice(&outer_len.to_le_bytes());

    Block::Raw {
        block_type: 0x0E,
        min_version: 3,
        current_version: 3,
        payload: bytes,
    }
}

fn build_image_scene_tree(image_group_node: CrdtId, parent_id: CrdtId) -> Block {
    let mut bytes = Vec::new();
    write_id(&mut bytes, 1, image_group_node);
    write_id(&mut bytes, 2, CrdtId::ZERO);
    write_bool(&mut bytes, 3, true);
    // Parent subblock.
    Stream::write_tag_into(&mut bytes, 4, TagType::Length4);
    let len_pos = bytes.len();
    bytes.extend_from_slice(&[0u8; 4]);
    let start = bytes.len();
    write_id(&mut bytes, 1, parent_id);
    let len = (bytes.len() - start) as u32;
    bytes[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());

    Block::Raw {
        block_type: 0x01,
        min_version: 1,
        current_version: 1,
        payload: bytes,
    }
}

fn build_image_tree_node(id: &ImageIds) -> Block {
    let mut bytes = Vec::new();
    // node_id at tag 1.
    write_id(&mut bytes, 1, id.group_node);
    // label LwwValue<string> at tag 2 — empty.
    write_lww_string(&mut bytes, 2, CrdtId::ZERO, "");
    // visible LwwValue<bool> at tag 3 — true.
    write_lww_bool(&mut bytes, 3, CrdtId::ZERO, true);
    // anchor_id LwwValue<CrdtId> at tag 7.
    write_lww_id(&mut bytes, 7, id.anchor_id_ts, id.anchor_value);
    // anchor_type LwwValue<u8> at tag 8 — observed as 2.
    write_lww_byte(&mut bytes, 8, id.anchor_type_ts, 2);
    // anchor_threshold LwwValue<f32> at tag 9 — observed as 0x420EFDFC ≈ 35.748.
    write_lww_f32(&mut bytes, 9, id.anchor_threshold_ts, f32::from_le_bytes([0xFC, 0xFD, 0x0E, 0x42]));
    // anchor_origin_x LwwValue<f32> at tag 10 — observed as -464.0.
    write_lww_f32(&mut bytes, 10, id.group_node, -464.0);

    Block::Raw {
        block_type: 0x02,
        min_version: 1,
        current_version: 2,
        payload: bytes,
    }
}

fn build_image_scene_group_item(
    parent_id: CrdtId,
    item_id: CrdtId,
    left_id: CrdtId,
    value: CrdtId,
) -> Block {
    let mut bytes = Vec::new();
    write_id(&mut bytes, 1, parent_id);
    write_id(&mut bytes, 2, item_id);
    write_id(&mut bytes, 3, left_id);
    write_id(&mut bytes, 4, CrdtId::ZERO);
    write_u32_tagged(&mut bytes, 5, 0);
    // Value subblock at tag 6: `02` byte then a tagged CrdtId at tag 2.
    Stream::write_tag_into(&mut bytes, 6, TagType::Length4);
    let len_pos = bytes.len();
    bytes.extend_from_slice(&[0u8; 4]);
    let start = bytes.len();
    bytes.push(0x02);
    write_id(&mut bytes, 2, value);
    let len = (bytes.len() - start) as u32;
    bytes[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());

    Block::Raw {
        block_type: 0x04,
        min_version: 1,
        current_version: 1,
        payload: bytes,
    }
}

fn build_image_item(img: &ImageEntry, id: &ImageIds) -> Block {
    let mut bytes = Vec::new();
    write_id(&mut bytes, 1, id.group_node);
    write_id(&mut bytes, 2, id.image_item);
    write_id(&mut bytes, 3, CrdtId::ZERO);
    write_id(&mut bytes, 4, CrdtId::ZERO);
    write_u32_tagged(&mut bytes, 5, 0);

    Stream::write_tag_into(&mut bytes, 6, TagType::Length4);
    let val_len_pos = bytes.len();
    bytes.extend_from_slice(&[0u8; 4]);
    let val_start = bytes.len();

    // Leading marker. Observed as 0x07.
    bytes.push(0x07);

    // Subblock at tag 1: { CrdtId timestamp, 16-byte image_id at tag 2 }.
    Stream::write_tag_into(&mut bytes, 1, TagType::Length4);
    let s1_len_pos = bytes.len();
    bytes.extend_from_slice(&[0u8; 4]);
    let s1_start = bytes.len();
    write_id(&mut bytes, 1, id.item_ref_id1);
    Stream::write_tag_into(&mut bytes, 2, TagType::Length4);
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&img.image_id);
    let s1_len = (bytes.len() - s1_start) as u32;
    bytes[s1_len_pos..s1_len_pos + 4].copy_from_slice(&s1_len.to_le_bytes());

    // Tagged CrdtId at tag 2.
    write_id(&mut bytes, 2, id.item_ref_id2);

    // Quad at tag 3: { count=16, 4 vertices × (x, y, u, v) f32 }.
    Stream::write_tag_into(&mut bytes, 3, TagType::Length4);
    let q_len_pos = bytes.len();
    bytes.extend_from_slice(&[0u8; 4]);
    let q_start = bytes.len();
    bytes.push(16);
    let (x0, y0, x1, y1) = (img.x, img.y, img.x + img.w, img.y + img.h);
    let verts: [(f32, f32, f32, f32); 4] = [
        (x0, y0, 0.0, 0.0),
        (x1, y0, 1.0, 0.0),
        (x1, y1, 1.0, 1.0),
        (x0, y1, 0.0, 1.0),
    ];
    for (vx, vy, vu, vv) in verts {
        bytes.extend_from_slice(&vx.to_le_bytes());
        bytes.extend_from_slice(&vy.to_le_bytes());
        bytes.extend_from_slice(&vu.to_le_bytes());
        bytes.extend_from_slice(&vv.to_le_bytes());
    }
    let q_len = (bytes.len() - q_start) as u32;
    bytes[q_len_pos..q_len_pos + 4].copy_from_slice(&q_len.to_le_bytes());

    // Triangle indices at tag 4: { count=6, [0,1,2,2,3,0] u32 }.
    Stream::write_tag_into(&mut bytes, 4, TagType::Length4);
    let i_len_pos = bytes.len();
    bytes.extend_from_slice(&[0u8; 4]);
    let i_start = bytes.len();
    bytes.push(6);
    for v in [0u32, 1, 2, 2, 3, 0] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let i_len = (bytes.len() - i_start) as u32;
    bytes[i_len_pos..i_len_pos + 4].copy_from_slice(&i_len.to_le_bytes());

    let val_len = (bytes.len() - val_start) as u32;
    bytes[val_len_pos..val_len_pos + 4].copy_from_slice(&val_len.to_le_bytes());

    Block::Raw {
        block_type: 0x0F,
        min_version: 2,
        current_version: 2,
        payload: bytes,
    }
}

// ---- low-level helpers -------------------------------------------------

fn write_id(buf: &mut Vec<u8>, index: u32, id: CrdtId) {
    Stream::write_tag_into(buf, index, TagType::Id);
    buf.push(id.part1);
    Stream::write_varuint_into(buf, id.part2);
}

fn write_u32_tagged(buf: &mut Vec<u8>, index: u32, value: u32) {
    Stream::write_tag_into(buf, index, TagType::Byte4);
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_bool(buf: &mut Vec<u8>, index: u32, v: bool) {
    Stream::write_tag_into(buf, index, TagType::Byte1);
    buf.push(if v { 1 } else { 0 });
}

fn write_lww_string(buf: &mut Vec<u8>, index: u32, ts: CrdtId, s: &str) {
    Stream::write_tag_into(buf, index, TagType::Length4);
    let len_pos = buf.len();
    buf.extend_from_slice(&[0u8; 4]);
    let start = buf.len();
    write_id(buf, 1, ts);
    Stream::write_tag_into(buf, 2, TagType::Length4);
    let inner_len_pos = buf.len();
    buf.extend_from_slice(&[0u8; 4]);
    let inner_start = buf.len();
    Stream::write_varuint_into(buf, s.len() as u64);
    buf.push(1); // is_ascii
    buf.extend_from_slice(s.as_bytes());
    let inner_len = (buf.len() - inner_start) as u32;
    buf[inner_len_pos..inner_len_pos + 4].copy_from_slice(&inner_len.to_le_bytes());
    let total = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&total.to_le_bytes());
}

fn write_lww_bool(buf: &mut Vec<u8>, index: u32, ts: CrdtId, value: bool) {
    Stream::write_tag_into(buf, index, TagType::Length4);
    let len_pos = buf.len();
    buf.extend_from_slice(&[0u8; 4]);
    let start = buf.len();
    write_id(buf, 1, ts);
    write_bool(buf, 2, value);
    let total = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&total.to_le_bytes());
}

fn write_lww_byte(buf: &mut Vec<u8>, index: u32, ts: CrdtId, value: u8) {
    Stream::write_tag_into(buf, index, TagType::Length4);
    let len_pos = buf.len();
    buf.extend_from_slice(&[0u8; 4]);
    let start = buf.len();
    write_id(buf, 1, ts);
    Stream::write_tag_into(buf, 2, TagType::Byte1);
    buf.push(value);
    let total = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&total.to_le_bytes());
}

fn write_lww_id(buf: &mut Vec<u8>, index: u32, ts: CrdtId, value: CrdtId) {
    Stream::write_tag_into(buf, index, TagType::Length4);
    let len_pos = buf.len();
    buf.extend_from_slice(&[0u8; 4]);
    let start = buf.len();
    write_id(buf, 1, ts);
    write_id(buf, 2, value);
    let total = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&total.to_le_bytes());
}

fn write_lww_f32(buf: &mut Vec<u8>, index: u32, ts: CrdtId, value: f32) {
    Stream::write_tag_into(buf, index, TagType::Length4);
    let len_pos = buf.len();
    buf.extend_from_slice(&[0u8; 4]);
    let start = buf.len();
    write_id(buf, 1, ts);
    Stream::write_tag_into(buf, 2, TagType::Byte4);
    buf.extend_from_slice(&value.to_le_bytes());
    let total = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&total.to_le_bytes());
}

fn write_lww_u8_marker(buf: &mut Vec<u8>, index: u32, ts: CrdtId, value: u8) {
    Stream::write_tag_into(buf, index, TagType::Length4);
    let len_pos = buf.len();
    buf.extend_from_slice(&[0u8; 4]);
    let start = buf.len();
    write_id(buf, 1, ts);
    Stream::write_tag_into(buf, 2, TagType::Length4);
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.push(17);
    buf.push(value);
    let total = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&total.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_image() -> ImageEntry {
        ImageEntry {
            image_id: [0xAB; 16],
            filename: "test.png".into(),
            png_bytes: vec![],
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        }
    }

    #[test]
    fn build_image_blocks_emits_full_set_per_image() {
        let blocks = build_image_blocks(&[dummy_image()], 1000);
        assert_eq!(blocks.scene_trees.len(), 1);
        assert_eq!(blocks.tree_nodes.len(), 1);
        assert_eq!(blocks.scene_group_items.len(), 1);
        assert_eq!(blocks.image_items.len(), 1);
        // Registry block type 0x0E.
        match &blocks.registry {
            Block::Raw { block_type, .. } => assert_eq!(*block_type, 0x0E),
            _ => panic!("registry should be Raw"),
        }
    }

    #[test]
    fn allocation_skips_unused_slots() {
        let id = ImageIds::allocate(100);
        assert_eq!(id.reg_ts1.part2, 100);
        assert_eq!(id.group_node.part2, 107);
        assert_eq!(id.image_item.part2, 112);
    }

    #[test]
    fn second_image_chains_via_left_id() {
        let blocks = build_image_blocks(&[dummy_image(), dummy_image()], 1000);
        // Each SceneGroupItem after the first should reference the prior
        // group's item_id via left_id (CrdtId(3,Id) tag).
        let payload = match &blocks.scene_group_items[1] {
            Block::Raw { payload, .. } => payload,
            _ => panic!(),
        };
        // tag 3 = (3<<4|0xF) = 0x3F is the left_id tag. Find that byte;
        // the next bytes are the CrdtId of the previous item. We just
        // assert it's non-zero (i.e. not CrdtId::ZERO).
        let tag3_idx = payload.iter().position(|&b| b == 0x3F).unwrap();
        let part1 = payload[tag3_idx + 1];
        let part2_first_byte = payload[tag3_idx + 2];
        assert!(part1 != 0 || part2_first_byte != 0);
    }
}
