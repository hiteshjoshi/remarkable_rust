//! Dev tool: print the block sequence of a .rm v6 file.

use rr::v6::{self, Block};

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_rm <path.rm>");
    let bytes = std::fs::read(&path).expect("read");
    let blocks = v6::parse(&bytes).expect("parse");

    println!("{}: {} blocks, {} bytes", path, blocks.len(), bytes.len());
    let mut counts = std::collections::BTreeMap::new();
    for b in &blocks {
        *counts.entry(b.block_type()).or_insert(0_usize) += 1;
    }
    println!("Block type histogram:");
    for (t, c) in &counts {
        let name = match t {
            0x00 => "MigrationInfo",
            0x01 => "SceneTree",
            0x02 => "TreeNode",
            0x03 => "SceneGlyphItem",
            0x04 => "SceneGroupItem",
            0x05 => "SceneLineItem",
            0x06 => "SceneTextItem",
            0x07 => "RootText",
            0x08 => "SceneTombstoneItem",
            0x09 => "AuthorIds",
            0x0A => "PageInfo",
            0x0D => "SceneInfo",
            _ => "?",
        };
        println!("  0x{:02X} {:<22}  ×{}", t, name, c);
    }

    println!("\nFirst 20 blocks in order:");
    for (i, b) in blocks.iter().enumerate().take(20) {
        let (min_v, cur_v) = b.versions();
        println!(
            "  [{:>3}] 0x{:02X} min={} cur={}",
            i,
            b.block_type(),
            min_v,
            cur_v
        );
    }

    let has_root_text = blocks.iter().any(|b| matches!(b, Block::RootText(_)));
    println!("\nHas RootTextBlock: {has_root_text}");

    for (i, b) in blocks.iter().enumerate() {
        if let Block::Raw {
            block_type,
            min_version,
            current_version,
            payload,
        } = b
        {
            if matches!(block_type, 0x01 | 0x02 | 0x04 | 0x0E | 0x0F) {
                println!(
                    "\n[{i}] Raw 0x{:02X} min={} cur={} ({} bytes):",
                    block_type,
                    min_version,
                    current_version,
                    payload.len()
                );
                for (j, byte) in payload.iter().enumerate() {
                    if j % 16 == 0 {
                        print!("  ");
                    }
                    print!("{:02X} ", byte);
                    if j % 16 == 15 {
                        println!();
                    }
                }
                if payload.len() % 16 != 0 {
                    println!();
                }
            }
        }
    }

    for b in &blocks {
        if let Block::RootText(rt) = b {
            println!("\nRootText:");
            println!("  min/cur version: {}/{}", rt.min_version, rt.current_version);
            println!("  block_id: {:?}", rt.block_id);
            println!("  items: {}", rt.items.len());
            for (i, item) in rt.items.iter().enumerate().take(20) {
                println!("    [{i}] {:?}", item);
            }
            println!("  styles: {}", rt.styles.len());
            for (k, v) in rt.styles.iter().take(10) {
                println!("    {:?} -> {:?}", k, v);
            }
            println!("  pos_x: {}, pos_y: {}, width: {}", rt.pos_x, rt.pos_y, rt.width);
            println!("  extra_data: {} bytes", rt.extra_data.len());
            if !rt.extra_data.is_empty() {
                print!("    bytes: ");
                for (i, b) in rt.extra_data.iter().enumerate() {
                    if i > 0 && i % 16 == 0 {
                        print!("\n           ");
                    }
                    print!("{:02X} ", b);
                }
                println!();
            }
        }
    }

    for b in &blocks {
        if let Block::SceneInfo(si) = b {
            println!("\nSceneInfo:");
            println!("  current_layer: {:?}", si.current_layer);
            println!("  background_visible: {:?}", si.background_visible);
            println!("  root_document_visible: {:?}", si.root_document_visible);
            println!("  paper_size: {:?}", si.paper_size);
            println!("  extra_data: {} bytes", si.extra_data.len());
            if !si.extra_data.is_empty() {
                print!("  extra bytes: ");
                for (i, b) in si.extra_data.iter().enumerate() {
                    if i > 0 && i % 16 == 0 {
                        print!("\n                ");
                    }
                    print!("{:02X} ", b);
                }
                println!();
            }
        }
    }
}
