//! Input dispatcher: figure out whether the user gave us markdown or HTML
//! and turn either into an XHTML fragment + an asset list ready for the
//! EPUB builder.
//!
//! HTML is treated as a first-class input format. Agents that emit rich
//! XHTML artifacts (with `<section>`s, inline SVG, hand-laid layouts) can
//! upload them directly without round-tripping through markdown. This is
//! the recommended path for any document where layout matters.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Error, Result};
use crate::markdown::{render_markdown, Document as MdDocument, InlineAsset, Rendered};

/// The unified result of preparing any input.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub title: String,
    pub metadata: HashMap<String, String>,
    pub xhtml: String,
    pub assets: Vec<InlineAsset>,
}

/// Decide whether `path` is markdown or HTML based on its extension.
pub fn prepare_from_path(path: &Path) -> Result<Prepared> {
    let kind = SourceKind::from_path(path);
    let raw = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_owned();
    let base = path.parent().map(Path::to_path_buf);
    prepare_from_string(raw, kind, &fallback, base.as_deref())
}

/// Prepare from raw text. `fallback_title` is used when no title can be
/// derived from the content itself.
pub fn prepare_from_string(
    raw: String,
    kind: SourceKind,
    fallback_title: &str,
    base_dir: Option<&Path>,
) -> Result<Prepared> {
    match kind {
        SourceKind::Markdown => {
            let md = MdDocument::from_string(raw, fallback_title);
            let Rendered { xhtml, assets } = render_markdown(&md.body_markdown, base_dir);
            Ok(Prepared {
                title: md.title,
                metadata: md.metadata,
                xhtml,
                assets,
            })
        }
        SourceKind::Html => {
            let body = extract_body(&raw);
            let metadata = extract_html_meta(&raw);
            let title = metadata
                .get("title")
                .cloned()
                .unwrap_or_else(|| fallback_title.to_owned());
            // We don't embed assets from HTML automatically yet — agents are
            // expected to inline everything (SVG, data URLs). Anything left
            // pointing at the filesystem must be added explicitly.
            Ok(Prepared {
                title,
                metadata,
                xhtml: body,
                assets: Vec::new(),
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Markdown,
    Html,
}

impl SourceKind {
    pub fn from_path(p: &Path) -> Self {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "md" | "markdown" | "mdown" | "mkd" => SourceKind::Markdown,
            "html" | "xhtml" | "htm" => SourceKind::Html,
            _ => SourceKind::Markdown,
        }
    }
}

/// Pull the contents of `<body>...</body>` out of an HTML/XHTML document.
/// If no `<body>` is found the whole input is returned (assumed to already
/// be a body fragment).
pub fn extract_body(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(open_idx) = lower.find("<body") else {
        return html.to_string();
    };
    let after_open_tag = match lower[open_idx..].find('>') {
        Some(i) => open_idx + i + 1,
        None => return html.to_string(),
    };
    let close_idx = lower[after_open_tag..]
        .find("</body>")
        .map(|i| after_open_tag + i);
    match close_idx {
        Some(end) => html[after_open_tag..end].trim().to_string(),
        None => html[after_open_tag..].trim().to_string(),
    }
}

/// Extract `<title>...</title>` and `<meta name="..." content="...">` pairs.
pub fn extract_html_meta(html: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let lower = html.to_ascii_lowercase();
    if let Some(open) = lower.find("<title>") {
        if let Some(rel_close) = lower[open + 7..].find("</title>") {
            let title = html[open + 7..open + 7 + rel_close].trim().to_string();
            if !title.is_empty() {
                out.insert("title".to_string(), title);
            }
        }
    }
    // Very small <meta name=foo content=bar> extractor — agents have no
    // reason to ship anything exotic here.
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<meta ") {
        let tag_start = cursor + rel;
        let tag_end = match lower[tag_start..].find('>') {
            Some(i) => tag_start + i,
            None => break,
        };
        let tag = &html[tag_start..tag_end];
        let name = attr_value(tag, "name").or_else(|| attr_value(tag, "property"));
        let content = attr_value(tag, "content");
        if let (Some(n), Some(c)) = (name, content) {
            out.insert(n.to_ascii_lowercase(), c);
        }
        cursor = tag_end + 1;
    }
    out
}

fn attr_value(tag: &str, key: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{key}=");
    let idx = lower.find(&needle)?;
    let rest = &tag[idx + needle.len()..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let after = &rest[1..];
        let end = after.find(quote)?;
        Some(after[..end].to_string())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_dispatch_by_extension() {
        assert_eq!(
            SourceKind::from_path(Path::new("/x/a.md")),
            SourceKind::Markdown
        );
        assert_eq!(
            SourceKind::from_path(Path::new("/x/a.HTML")),
            SourceKind::Html
        );
        assert_eq!(
            SourceKind::from_path(Path::new("/x/a.xhtml")),
            SourceKind::Html
        );
        assert_eq!(
            SourceKind::from_path(Path::new("/x/a.unknown")),
            SourceKind::Markdown
        );
    }

    #[test]
    fn body_extracted_when_full_document() {
        let html =
            "<!doctype html><html><head><title>T</title></head><body><p>hi</p></body></html>";
        assert_eq!(extract_body(html).trim(), "<p>hi</p>");
    }

    #[test]
    fn body_passthrough_when_already_fragment() {
        assert_eq!(extract_body("<p>hi</p>"), "<p>hi</p>");
    }

    #[test]
    fn title_and_meta_extracted() {
        let html = r#"<html><head>
            <title>Hello</title>
            <meta name="author" content="someone">
            <meta property="og:source" content='https://x.test'>
        </head><body></body></html>"#;
        let m = extract_html_meta(html);
        assert_eq!(m.get("title"), Some(&"Hello".to_string()));
        assert_eq!(m.get("author"), Some(&"someone".to_string()));
        assert_eq!(m.get("og:source"), Some(&"https://x.test".to_string()));
    }

    #[test]
    fn html_preparation_round_trip() {
        let html =
            "<html><head><title>Demo</title></head><body><h1>Demo</h1><svg></svg></body></html>";
        let p = prepare_from_string(html.to_string(), SourceKind::Html, "fb", None).unwrap();
        assert_eq!(p.title, "Demo");
        assert!(p.xhtml.contains("<svg>"));
    }
}
