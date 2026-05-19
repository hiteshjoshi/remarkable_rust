//! Minimal EPUB 3 builder used to wrap markdown content before upload.
//!
//! The reMarkable Document API accepts an EPUB and the device renders it as a
//! native reMarkable document. The extension builds an EPUB this way and so
//! do we; details and references are in `docs/protocol/`.
//!
//! Layout:
//!
//! ```text
//! mimetype                    (stored, no compression, first entry)
//! META-INF/container.xml
//! OEBPS/content.opf
//! OEBPS/nav.xhtml
//! OEBPS/article.xhtml
//! ```
//!
//! Images are not yet embedded — the markdown converter strips them today.
//! When we add image embedding, drop them under `OEBPS/<name>` and add a
//! matching `<item>` to the manifest.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::error::{Error, Result};

/// Bibliographic and provenance fields surfaced on the device.
#[derive(Debug, Clone, Default)]
pub struct EpubMeta {
    pub title: String,
    pub author: String,
    pub description: String,
    pub publisher: String,
    pub language: String,
    pub published_time: String,
    pub source_url: String,
}

impl EpubMeta {
    pub fn from_title(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            language: "en".into(),
            ..Self::default()
        }
    }
}

/// A binary asset (image) embedded inside the EPUB alongside the article.
#[derive(Debug, Clone)]
pub struct EpubAsset {
    /// File name inside `OEBPS/` (e.g. `img-001.png`).
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Build an EPUB with the article and zero or more embedded assets.
pub fn build_article_epub_with_assets(
    meta: &EpubMeta,
    article_xhtml: &str,
    assets: &[EpubAsset],
) -> Result<Vec<u8>> {
    build_inner(meta, article_xhtml, assets)
}

/// Build an EPUB containing a single XHTML article.
///
/// `article_xhtml` must be a complete XHTML *body* fragment; the helper wraps
/// it into a valid `<html xmlns="http://www.w3.org/1999/xhtml">` document.
pub fn build_article_epub(meta: &EpubMeta, article_xhtml: &str) -> Result<Vec<u8>> {
    build_inner(meta, article_xhtml, &[])
}

fn build_inner(meta: &EpubMeta, article_xhtml: &str, assets: &[EpubAsset]) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::<u8>::new());
    {
        let mut zip = ZipWriter::new(&mut buf);

        // mimetype: stored (no compression), first, no extra fields.
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("mimetype", stored)
            .map_err(|e| Error::Convert(format!("epub mimetype: {e}")))?;
        zip.write_all(b"application/epub+zip")
            .map_err(Error::BareIo)?;

        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("META-INF/container.xml", deflated)
            .map_err(|e| Error::Convert(format!("epub container: {e}")))?;
        zip.write_all(CONTAINER_XML.as_bytes())
            .map_err(Error::BareIo)?;

        let opf = build_content_opf(meta, assets);
        zip.start_file("OEBPS/content.opf", deflated)
            .map_err(|e| Error::Convert(format!("epub opf: {e}")))?;
        zip.write_all(opf.as_bytes()).map_err(Error::BareIo)?;

        let nav = build_nav_xhtml(&meta.title);
        zip.start_file("OEBPS/nav.xhtml", deflated)
            .map_err(|e| Error::Convert(format!("epub nav: {e}")))?;
        zip.write_all(nav.as_bytes()).map_err(Error::BareIo)?;

        let article = wrap_article_xhtml(&meta.title, article_xhtml);
        zip.start_file("OEBPS/article.xhtml", deflated)
            .map_err(|e| Error::Convert(format!("epub article: {e}")))?;
        zip.write_all(article.as_bytes()).map_err(Error::BareIo)?;

        // Images and other binary assets: keep them under OEBPS/ next to the
        // article so relative `src="img-001.png"` references resolve.
        for asset in assets {
            let path = format!("OEBPS/{}", asset.name);
            zip.start_file(&path, deflated)
                .map_err(|e| Error::Convert(format!("epub asset {path}: {e}")))?;
            zip.write_all(&asset.bytes).map_err(Error::BareIo)?;
        }

        zip.finish()
            .map_err(|e| Error::Convert(format!("epub finalize: {e}")))?;
    }
    Ok(buf.into_inner())
}

const CONTAINER_XML: &str = r#"<?xml version="1.0"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
<rootfiles>
<rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
</rootfiles>
</container>"#;

fn build_content_opf(meta: &EpubMeta, assets: &[EpubAsset]) -> String {
    let title = xml_escape(&meta.title);
    let author = xml_escape(&meta.author);
    let description = xml_escape(&meta.description);
    let publisher = xml_escape(&meta.publisher);
    let language = xml_escape(if meta.language.is_empty() {
        "en"
    } else {
        &meta.language
    });
    let published = xml_escape(&meta.published_time);
    let source = xml_escape(&meta.source_url);
    let id = uuid::Uuid::new_v4();
    let modified = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="BookID">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="BookID">urn:uuid:{id}</dc:identifier>
    <dc:title>{title}</dc:title>
    <dc:creator>{author}</dc:creator>
    <dc:description>{description}</dc:description>
    <dc:publisher>{publisher}</dc:publisher>
    <dc:language>{language}</dc:language>
    <dc:date>{published}</dc:date>
    <dc:source>{source}</dc:source>
    <meta property="dcterms:modified">{modified}</meta>
  </metadata>
  <manifest>
    <item id="nav.xhtml" href="nav.xhtml" properties="nav" media-type="application/xhtml+xml"/>
    <item id="article.xhtml" href="article.xhtml" media-type="application/xhtml+xml"/>
{asset_items}  </manifest>
  <spine>
    <itemref idref="article.xhtml"/>
  </spine>
</package>"#,
        asset_items = build_manifest_assets(assets)
    )
}

fn build_manifest_assets(assets: &[EpubAsset]) -> String {
    let mut s = String::new();
    for (idx, asset) in assets.iter().enumerate() {
        // EPUB requires unique manifest ids; the filename usually suffices but
        // we also keep a guaranteed-unique numeric suffix.
        let id = format!("asset-{idx:03}");
        let href = xml_escape(&asset.name);
        let mime = xml_escape(&asset.mime);
        s.push_str(&format!(
            "    <item id=\"{id}\" href=\"{href}\" media-type=\"{mime}\"/>\n"
        ));
    }
    s
}

fn build_nav_xhtml(title: &str) -> String {
    let t = xml_escape(title);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="en" lang="en">
<head><title>{t}</title></head>
<body>
<nav epub:type="toc" id="toc">
<ol><li><a href="article.xhtml">{t}</a></li></ol>
</nav>
</body>
</html>"#
    )
}

fn wrap_article_xhtml(title: &str, body_fragment: &str) -> String {
    let t = xml_escape(title);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en" lang="en">
<head><meta charset="utf-8"/><title>{t}</title></head>
<body>
<h1>{t}</h1>
{body_fragment}
</body>
</html>"#
    )
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_valid_zip_with_mimetype_first() {
        let meta = EpubMeta::from_title("Hello");
        let bytes = build_article_epub(&meta, "<p>hi</p>").unwrap();
        // Local file header signature.
        assert_eq!(&bytes[0..4], b"PK\x03\x04");
        // The first stored entry is "mimetype"; its filename appears at byte 30.
        let name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
        let name = &bytes[30..30 + name_len];
        assert_eq!(name, b"mimetype");
    }

    #[test]
    fn escapes_xml_specials_in_title() {
        let meta = EpubMeta::from_title("A & B <C>");
        let bytes = build_article_epub(&meta, "<p>body</p>").unwrap();
        // Read back via the zip reader.
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        use std::io::Read;
        z.by_name("OEBPS/content.opf")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert!(s.contains("A &amp; B &lt;C&gt;"));
    }

    #[test]
    fn unicode_title_round_trips() {
        let meta = EpubMeta::from_title("日本語");
        let bytes = build_article_epub(&meta, "<p>本文</p>").unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        use std::io::Read;
        let mut s = String::new();
        z.by_name("OEBPS/article.xhtml")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert!(s.contains("日本語"));
        assert!(s.contains("本文"));
    }
}
