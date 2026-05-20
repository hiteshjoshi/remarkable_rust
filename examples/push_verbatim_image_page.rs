//! Diagnostic: take a real image-bearing `.rm` plus its PNG files and
//! push them to the device verbatim — same filenames the .rm references,
//! same bytes everywhere — wrapped only in our own metadata/content JSON.
//!
//! If this renders images on the device, our cloud-bundle layer is fine
//! and the 0x0E/0x0F bytes we generate must be wrong. If it doesn't,
//! something more fundamental is missing.

use std::path::PathBuf;

use rr::{
    notebook::{Bundle, BundleImage, BundleOptions, PageInput},
    sync_v3::SyncClient,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let token = std::env::args()
        .nth(1)
        .expect("usage: push_verbatim_image_page <token>");

    // The .rm from a real image-bearing page in the user's account.
    let rm_bytes = std::fs::read("/tmp/img-page.rm")?;

    // The two PNGs referenced from that .rm's ImageRegistry block.
    // Filenames must match the registry's filename field exactly, otherwise
    // the device won't find them.
    let png1 = std::fs::read("/tmp/img1.png")?;
    let png2 = std::fs::read("/tmp/img2.png")?;

    let opts = BundleOptions::new(
        "verbatim image-page diagnostic",
        vec![PageInput::from_markdown("placeholder text")],
    );
    let mut bundle = Bundle::build(&opts)?;

    // Override the page bytes with the real image-bearing .rm and attach
    // its referenced PNGs at the exact filenames.
    let page = bundle.pages.first_mut().expect("one page");
    page.rm_bytes = rm_bytes;
    page.images = vec![
        BundleImage {
            filename: "6c8f470b-e75a-4dc6-b40c-f09cf7dd5a96.png".into(),
            png_bytes: png1,
        },
        BundleImage {
            filename: "02962cf5-13d7-401e-8e07-13783255520a.png".into(),
            png_bytes: png2,
        },
    ];

    let client = SyncClient::new(token)?;
    let result = client.upload_bundle(&bundle).await?;
    println!("doc {} (gen {})", result.doc_id, result.new_generation);

    // Also write to a local dir for inspection.
    let out = PathBuf::from("/tmp/verbatim-bundle");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;
    bundle.write_to(&out)?;
    println!("local copy written to {}", out.display());
    Ok(())
}
