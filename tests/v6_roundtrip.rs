//! Decode → encode → byte-equality on the rmscene fixtures.
//!
//! This proves that for any v6 file we can fully model, our encoder
//! produces the *exact* bytes the original came in with. For fixtures that
//! contain block types we only stash as `Block::Raw`, byte-equality still
//! holds because those blocks are written back verbatim — that's the whole
//! point of preserving raw payloads.

use std::path::PathBuf;

use rr::v6;

fn samples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools/v6-samples")
}

#[test]
fn encode_empty_block_list_yields_header_only() {
    let bytes = v6::encode(&[]).unwrap();
    assert_eq!(bytes.len(), v6::HEADER_V6.len());
    assert_eq!(&bytes[..], &v6::HEADER_V6[..]);
}

#[test]
fn roundtrip_normal_ab() {
    let orig = std::fs::read(samples_dir().join("Normal_AB.rm")).unwrap();
    let blocks = v6::parse(&orig).expect("decode");
    let out = v6::encode(&blocks).expect("encode");
    assert_eq!(
        out.len(),
        orig.len(),
        "length mismatch: encoded={} original={}",
        out.len(),
        orig.len()
    );
    assert_bytes_equal(&orig, &out, "Normal_AB.rm");
}

#[test]
fn roundtrip_bold_heading_bullet_normal() {
    let orig = std::fs::read(samples_dir().join("Bold_Heading_Bullet_Normal.rm")).unwrap();
    let blocks = v6::parse(&orig).expect("decode");
    let out = v6::encode(&blocks).expect("encode");
    assert_bytes_equal(&orig, &out, "Bold_Heading_Bullet_Normal.rm");
}

#[test]
fn roundtrip_all_fixtures() {
    let dir = samples_dir();
    let mut count = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read v6-samples") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("rm") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let orig = std::fs::read(&path).unwrap();
        let blocks = match v6::parse(&orig) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{name}: decode failed: {e}"));
                continue;
            }
        };
        let out = match v6::encode(&blocks) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{name}: encode failed: {e}"));
                continue;
            }
        };
        if out != orig {
            failures.push(format!(
                "{name}: byte mismatch (orig={}B, out={}B, first diff at {})",
                orig.len(),
                out.len(),
                first_diff(&orig, &out)
            ));
        }
        count += 1;
    }
    assert!(count >= 10, "expected ≥10 fixtures, got {count}");
    assert!(
        failures.is_empty(),
        "round-trip failures:\n  - {}",
        failures.join("\n  - ")
    );
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            return format!("byte {i} (orig=0x{x:02X}, out=0x{y:02X})");
        }
    }
    if a.len() != b.len() {
        return format!("length differs (orig={}, out={})", a.len(), b.len());
    }
    "no difference".to_string()
}

#[track_caller]
fn assert_bytes_equal(orig: &[u8], out: &[u8], name: &str) {
    if orig == out {
        return;
    }
    // Print a small hex context around the first difference for debugging.
    let mut idx = orig.len().min(out.len());
    for (i, (x, y)) in orig.iter().zip(out.iter()).enumerate() {
        if x != y {
            idx = i;
            break;
        }
    }
    let start = idx.saturating_sub(16);
    let end_orig = (idx + 16).min(orig.len());
    let end_out = (idx + 16).min(out.len());
    panic!(
        "{name} byte mismatch at offset {idx} (orig.len={}, out.len={}).\n  orig[{start}..{end_orig}] = {:02X?}\n  out [{start}..{end_out}]  = {:02X?}",
        orig.len(),
        out.len(),
        &orig[start..end_orig],
        &out[start..end_out]
    );
}
