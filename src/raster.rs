//! SVG → PNG rasterization with system fonts.
//!
//! Used to convert structured visual elements (tables, diagrams) into PNGs
//! that the reMarkable EPUB→notebook converter actually renders. The
//! converter strips `<svg>` and flattens `<table>`/`<pre>`, but it does
//! pass embedded raster images through to the device unchanged.
//!
//! Implementation: `usvg` parses + lays out the SVG (including text using
//! the host's installed fonts), `resvg` rasterizes onto a `tiny-skia`
//! pixmap, which we encode to PNG. No system libraries — fully static.

use std::sync::{Arc, OnceLock};

use crate::error::{Error, Result};

// usvg re-exports its own `fontdb` so we route through it; otherwise we
// end up with two versions of the crate at link time and the types diverge.
type Fontdb = usvg::fontdb::Database;

static FONTDB: OnceLock<Arc<Fontdb>> = OnceLock::new();

fn fontdb() -> Arc<Fontdb> {
    FONTDB
        .get_or_init(|| {
            let mut db = Fontdb::new();
            db.load_system_fonts();
            db.set_serif_family("Georgia");
            db.set_sans_serif_family("Helvetica");
            db.set_monospace_family("Menlo");
            Arc::new(db)
        })
        .clone()
}

/// Rasterize an SVG string to PNG bytes. Returns an error if the SVG fails
/// to parse or if pixmap allocation fails (e.g. zero-sized).
pub fn svg_to_png(svg: &str) -> Result<Vec<u8>> {
    let opt = usvg::Options {
        fontdb: fontdb(),
        ..usvg::Options::default()
    };

    let tree =
        usvg::Tree::from_str(svg, &opt).map_err(|e| Error::Convert(format!("svg parse: {e}")))?;

    let size = tree.size().to_int_size();
    let (w, h) = (size.width(), size.height());
    if w == 0 || h == 0 {
        return Err(Error::Convert("svg has zero dimension".into()));
    }
    let mut pixmap = tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| Error::Convert(format!("pixmap alloc {w}×{h}")))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap
        .encode_png()
        .map_err(|e| Error::Convert(format!("png encode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_simple_rectangle() {
        // Pure shapes — no text, no fonts needed.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10">
            <rect x="0" y="0" width="10" height="10" fill="black"/>
        </svg>"#;
        let png = svg_to_png(svg).expect("rasterize");
        assert!(!png.is_empty());
        assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n", "must be a PNG");
    }
}
