//! Dev tool: print raw skeleton bytes (SceneTree, TreeNode×2, SceneGroupItem)
//! and SceneInfo extra_data from a sample .rm so we can paste them into
//! `src/v6/page.rs` as constants.
//!
//! Run with: `cargo run --example extract_skeleton -- <path.rm>`

use rr::v6::{self, Block};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tools/v6-samples/Normal_AB.rm".into());
    let bytes = std::fs::read(&path).expect("read fixture");
    let blocks = v6::parse(&bytes).expect("parse fixture");

    println!("Block sequence in {path}:");
    for (i, b) in blocks.iter().enumerate() {
        let (min_v, cur_v) = b.versions();
        println!(
            "  [{}] type=0x{:02X}  min_version={}  current_version={}",
            i,
            b.block_type(),
            min_v,
            cur_v
        );
    }
    println!();

    for (i, b) in blocks.iter().enumerate() {
        if let Block::Raw {
            block_type,
            min_version,
            current_version,
            payload,
        } = b
        {
            println!(
                "// Block [{i}] type=0x{block_type:02X} min={min_version} cur={current_version} ({} bytes)",
                payload.len()
            );
            print!("pub const SKEL_{i}: &[u8] = &[");
            for (j, byte) in payload.iter().enumerate() {
                if j % 16 == 0 {
                    print!("\n    ");
                }
                print!("0x{byte:02X}, ");
            }
            println!("\n];\n");
        }
    }

    for (i, b) in blocks.iter().enumerate() {
        if let Block::SceneInfo(si) = b {
            println!(
                "// Block [{i}] SceneInfo extra_data: {} bytes",
                si.extra_data.len()
            );
            print!("pub const SCENE_INFO_EXTRA: &[u8] = &[");
            for (j, byte) in si.extra_data.iter().enumerate() {
                if j % 16 == 0 {
                    print!("\n    ");
                }
                print!("0x{byte:02X}, ");
            }
            println!("\n];");
        }
    }
}
