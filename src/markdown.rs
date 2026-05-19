//! Markdown parsing.
//!
//! Produces an XHTML body fragment fit for inclusion in an EPUB article,
//! a parsed title (from frontmatter `title:` or the first H1), and
//! YAML-ish frontmatter metadata. Output is plain XHTML — no `<style>`,
//! no `<script>` — so the cloud's notebook converter handles it cleanly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pulldown_cmark::{html, Alignment, CowStr, Event, Options, Parser, Tag, TagEnd};

/// A binary asset (image) collected while rendering markdown, ready to be
/// embedded into the EPUB. `name` is the relative path inside `OEBPS/`.
#[derive(Debug, Clone)]
pub struct InlineAsset {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Result of rendering markdown: the XHTML fragment plus any embedded assets.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub xhtml: String,
    pub assets: Vec<InlineAsset>,
}

use crate::error::{Error, Result};

/// A markdown source bundled with its resolved title and frontmatter.
#[derive(Debug, Clone)]
pub struct Document {
    pub title: String,
    pub body_markdown: String,
    pub metadata: HashMap<String, String>,
}

impl Document {
    pub fn from_path(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let fallback = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_owned();
        Ok(Self::from_string(content, &fallback))
    }

    pub fn from_string(content: String, fallback_title: &str) -> Self {
        let (metadata, body) = parse_frontmatter(&content);
        let title = metadata
            .get("title")
            .cloned()
            .unwrap_or_else(|| extract_h1_title(&body, fallback_title));
        Self {
            title,
            body_markdown: body,
            metadata,
        }
    }

    /// Render to an XHTML fragment + collected image assets.
    ///
    /// `base_dir` is the directory used to resolve relative image paths
    /// (typically the parent directory of the markdown source). When `None`,
    /// only absolute paths and `data:` URLs are embeddable.
    pub fn render(&self, base_dir: Option<&Path>) -> Rendered {
        render_markdown(&self.body_markdown, base_dir)
    }

    /// Convenience: render and discard any assets.
    pub fn to_xhtml_fragment(&self) -> String {
        self.render(None).xhtml
    }
}

/// The HTML rendering pipeline used by [`Document::render`].
///
/// Two non-default behaviours are notable:
///
/// * Markdown tables are rewritten as box-drawn ASCII inside `<pre>` blocks.
///   The reMarkable EPUB→notebook converter flattens real `<table>`s into
///   plain text with bare `|` separators; monospace pre-formatted text keeps
///   the columns visually aligned on the device.
/// * Local image references are inlined: the file is read, given a unique
///   name, and emitted alongside the HTML as an [`InlineAsset`] for the EPUB
///   to embed. The image's `src` is rewritten to the relative name.
pub fn render_markdown(content: &str, base_dir: Option<&Path>) -> Rendered {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let events: Vec<Event<'_>> = Parser::new_ext(content, options).collect();

    let mut state = RenderState::new(base_dir);

    let mut i = 0;
    while i < events.len() {
        if matches!(&events[i], Event::Start(Tag::Table(_))) {
            let (j, alignments, rows) = extract_table(&events, i);
            state.emit_table(&alignments, &rows);
            i = j + 1;
        } else if matches!(&events[i], Event::Start(Tag::Image { .. })) {
            // Find matching End event and walk the inner events to build alt text.
            let mut depth = 1;
            let mut k = i + 1;
            let mut alt = String::new();
            while k < events.len() && depth > 0 {
                match &events[k] {
                    Event::End(TagEnd::Image) => depth -= 1,
                    Event::Start(Tag::Image { .. }) => depth += 1,
                    Event::Text(t) | Event::Code(t) => alt.push_str(t),
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                k += 1;
            }
            let (dest, title) = match &events[i] {
                Event::Start(Tag::Image {
                    dest_url, title, ..
                }) => (dest_url.clone(), title.clone()),
                _ => unreachable!(),
            };
            state.emit_image(&dest, &title, &alt);
            i = k + 1;
        } else {
            // Stream the next chunk of normal events up to the next table/image.
            let mut j = i;
            while j < events.len()
                && !matches!(
                    &events[j],
                    Event::Start(Tag::Table(_)) | Event::Start(Tag::Image { .. })
                )
            {
                j += 1;
            }
            html::push_html(&mut state.out, events[i..j].iter().cloned());
            i = j;
        }
    }

    Rendered {
        xhtml: state.out,
        assets: state.assets,
    }
}

struct RenderState<'a> {
    out: String,
    assets: Vec<InlineAsset>,
    base_dir: Option<&'a Path>,
    image_counter: usize,
}

impl<'a> RenderState<'a> {
    fn new(base_dir: Option<&'a Path>) -> Self {
        Self {
            out: String::new(),
            assets: Vec::new(),
            base_dir,
            image_counter: 0,
        }
    }

    fn emit_table(&mut self, aligns: &[Alignment], rows: &[Vec<String>]) {
        // The reMarkable EPUB→notebook converter strips <table>, <svg>, and
        // collapses <pre> whitespace — every layout-bearing element fails
        // *except* embedded raster images. So we render the table to a PNG
        // locally and embed that. Falls back to bullet records if anything
        // in the rasterization path errors out.
        if let Some(()) = self.try_emit_table_as_png(aligns, rows) {
            return;
        }
        emit_bullet_table(&mut self.out, rows);
    }

    fn try_emit_table_as_png(&mut self, aligns: &[Alignment], rows: &[Vec<String>]) -> Option<()> {
        if rows.is_empty() {
            return None;
        }
        let svg = build_table_svg(rows, aligns);
        let png = match crate::raster::svg_to_png(&svg) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "table rasterization failed; falling back to bullets");
                return None;
            }
        };
        self.image_counter += 1;
        let name = format!("table-{:03}.png", self.image_counter);
        self.assets.push(InlineAsset {
            name: name.clone(),
            mime: "image/png".to_owned(),
            bytes: png,
        });
        self.out.push_str("<p><img src=\"");
        push_html_text(&mut self.out, &name);
        self.out.push_str("\" alt=\"table\"/></p>\n");
        Some(())
    }

    fn emit_image(&mut self, src: &str, title: &str, alt: &str) {
        // Resolve and embed; fall back to a rendered placeholder if unavailable.
        match resolve_local_image(src, self.base_dir) {
            Some((bytes, mime, ext)) => {
                self.image_counter += 1;
                let name = format!("img-{:03}.{ext}", self.image_counter);
                self.assets.push(InlineAsset {
                    name: name.clone(),
                    mime,
                    bytes,
                });
                self.out.push_str("<p><img src=\"");
                push_html_text(&mut self.out, &name);
                self.out.push_str("\" alt=\"");
                push_html_text(&mut self.out, alt);
                if !title.is_empty() {
                    self.out.push_str("\" title=\"");
                    push_html_text(&mut self.out, title);
                }
                self.out.push_str("\"/></p>\n");
            }
            None if src.starts_with("data:") => {
                // Inline data URL — leave as-is and hope the converter accepts it.
                self.out.push_str("<p><img src=\"");
                push_html_text(&mut self.out, src);
                self.out.push_str("\" alt=\"");
                push_html_text(&mut self.out, alt);
                self.out.push_str("\"/></p>\n");
            }
            None => {
                tracing::warn!(src, "image not embeddable; replaced with alt text");
                self.out.push_str("<p><em>[image: ");
                push_html_text(&mut self.out, alt);
                self.out.push_str("]</em></p>\n");
            }
        }
    }
}

fn extract_table<'a>(
    events: &'a [Event<'a>],
    start: usize,
) -> (usize, Vec<Alignment>, Vec<Vec<String>>) {
    let alignments = match &events[start] {
        Event::Start(Tag::Table(a)) => a.clone(),
        _ => Vec::new(),
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut in_cell = false;
    let mut depth = 1;

    let mut j = start + 1;
    while j < events.len() && depth > 0 {
        match &events[j] {
            Event::Start(Tag::Table(_)) => depth += 1,
            Event::End(TagEnd::Table) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            // pulldown-cmark emits header cells as direct children of
            // TableHead (no TableRow wrapper), so we treat both as row
            // boundaries.
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                current_row = Vec::new();
            }
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                rows.push(std::mem::take(&mut current_row));
            }
            Event::Start(Tag::TableCell) => {
                in_cell = true;
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                in_cell = false;
                current_row.push(std::mem::take(&mut current_cell));
            }
            Event::Text(t) | Event::Code(t) if in_cell => current_cell.push_str(t),
            Event::SoftBreak | Event::HardBreak if in_cell => current_cell.push(' '),
            _ => {}
        }
        j += 1;
    }
    (j, alignments, rows)
}

/// Render a markdown table as a `<ul>` of records — the only layout
/// the reMarkable EPUB→notebook converter currently respects.
///
/// Format rules:
/// * If there's no header row, every cell joins with " — ".
/// * Otherwise headers become "Header: value" pairs, the first column is
///   bolded as the record label, and the rest follow em-dash separated.
/// * Single-column tables degrade to a plain bullet list.
fn emit_bullet_table(out: &mut String, rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }
    out.push_str("<ul class=\"rr-table\">\n");
    let header = &rows[0];
    let body = &rows[1..];
    if header.len() == 1 {
        for r in rows {
            out.push_str("  <li>");
            push_html_text(out, r.first().map(String::as_str).unwrap_or(""));
            out.push_str("</li>\n");
        }
    } else if body.is_empty() {
        // Just a header row — render as a single line with em-dashes.
        out.push_str("  <li>");
        let joined = header
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" — ");
        push_html_text(out, &joined);
        out.push_str("</li>\n");
    } else {
        for row in body {
            out.push_str("  <li><strong>");
            push_html_text(out, row.first().map(String::as_str).unwrap_or(""));
            out.push_str("</strong>");
            for (i, cell) in row.iter().enumerate().skip(1) {
                let hdr = header.get(i).map(String::as_str).unwrap_or("");
                out.push_str(" — ");
                if !hdr.is_empty() {
                    push_html_text(out, hdr);
                    out.push_str(": ");
                }
                push_html_text(out, cell);
            }
            out.push_str("</li>\n");
        }
    }
    out.push_str("</ul>\n");
}

/// Build an SVG depicting `rows` as a bordered table. Column widths follow
/// the longest cell in each column, scaled by an approximate character
/// width. The header row gets a bold weight and a thicker underline.
pub(crate) fn build_table_svg(rows: &[Vec<String>], aligns: &[Alignment]) -> String {
    // Layout constants (px). Tuned for the reMarkable Paper Pro reading
    // width: 1404px usable horizontal at 1× zoom.
    const FONT_SIZE: u32 = 28;
    const ROW_H: u32 = 44;
    const PAD_X: u32 = 18;
    const CHAR_W: u32 = 16; // approx for serif at FONT_SIZE
    const MIN_COL_W: u32 = 80;
    const TARGET_TOTAL_W: u32 = 1400;

    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return svg_placeholder("(empty table)");
    }

    // Initial column widths: text length × CHAR_W + padding, then bump to
    // MIN_COL_W. We don't shrink overly-wide cells — we soft-wrap text
    // into multiple lines instead via a simple character-budget algorithm.
    let mut col_w: Vec<u32> = (0..cols)
        .map(|i| {
            rows.iter()
                .map(|r| r.get(i).map(|s| s.chars().count() as u32).unwrap_or(0))
                .max()
                .unwrap_or(0)
        })
        .map(|chars| chars.saturating_mul(CHAR_W).saturating_add(PAD_X * 2))
        .map(|w| w.max(MIN_COL_W))
        .collect();

    // If sum > target, scale down by reducing widest columns first.
    let mut total: u32 = col_w.iter().sum();
    while total > TARGET_TOTAL_W {
        let (idx, _) = col_w
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| *w)
            .map(|(i, w)| (i, *w))
            .unwrap_or((0, 0));
        if col_w[idx] <= MIN_COL_W {
            break;
        }
        col_w[idx] -= (col_w[idx] / 10).max(8);
        total = col_w.iter().sum();
    }

    // Wrap each cell's text into lines that fit in (col_w - 2*PAD_X).
    let wrapped: Vec<Vec<Vec<String>>> = rows
        .iter()
        .map(|row| {
            (0..cols)
                .map(|i| {
                    let raw = row.get(i).map(String::as_str).unwrap_or("");
                    let max_chars = ((col_w[i].saturating_sub(PAD_X * 2)) / CHAR_W).max(4) as usize;
                    wrap_text(raw, max_chars)
                })
                .collect()
        })
        .collect();
    let row_heights: Vec<u32> = wrapped
        .iter()
        .map(|cells| {
            let max_lines = cells.iter().map(|c| c.len().max(1)).max().unwrap_or(1) as u32;
            ROW_H + (max_lines.saturating_sub(1)) * (FONT_SIZE + 6)
        })
        .collect();

    let width = total;
    let height: u32 = row_heights.iter().sum::<u32>() + 4; // border allowance
    let mut svg = String::with_capacity(2048);
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="Georgia, serif" font-size="{FONT_SIZE}">
"#
    ));

    // Outer border
    svg.push_str(&format!(
        r#"<rect x="1" y="1" width="{w}" height="{h}" fill="white" stroke="black" stroke-width="2"/>
"#,
        w = width.saturating_sub(2),
        h = height.saturating_sub(2),
    ));

    // Vertical separators
    let mut x: u32 = 0;
    for (i, w) in col_w.iter().enumerate() {
        x += w;
        if i + 1 < cols {
            svg.push_str(&format!(
                r#"<line x1="{x}" y1="1" x2="{x}" y2="{h}" stroke="black" stroke-width="1"/>
"#,
                h = height.saturating_sub(1),
            ));
        }
    }

    // Horizontal separators + content
    let mut y: u32 = 0;
    for (idx, row_wrapped) in wrapped.iter().enumerate() {
        let rh = row_heights[idx];
        let is_header = idx == 0;
        // Bottom border of this row
        let bottom = y + rh;
        let stroke_w = if is_header { 2 } else { 1 };
        if bottom < height {
            svg.push_str(&format!(
                r#"<line x1="1" y1="{bottom}" x2="{w}" y2="{bottom}" stroke="black" stroke-width="{stroke_w}"/>
"#,
                w = width.saturating_sub(1),
            ));
        }

        // Cell text
        let mut cx: u32 = 0;
        for (i, lines) in row_wrapped.iter().enumerate() {
            let cw = col_w[i];
            let align = aligns.get(i).copied().unwrap_or(Alignment::Left);
            let line_h = FONT_SIZE + 6;
            // Vertically center the block of lines.
            let block_h = (lines.len().max(1) as u32) * line_h;
            let baseline_start = y + (rh - block_h) / 2 + FONT_SIZE - 4;
            for (li, line) in lines.iter().enumerate() {
                let baseline = baseline_start + (li as u32) * line_h;
                let (tx, anchor) = match align {
                    Alignment::Right => (cx + cw - PAD_X, "end"),
                    Alignment::Center => (cx + cw / 2, "middle"),
                    _ => (cx + PAD_X, "start"),
                };
                let weight = if is_header { "bold" } else { "normal" };
                svg.push_str(&format!(
                    r#"<text x="{tx}" y="{baseline}" text-anchor="{anchor}" font-weight="{weight}">{}</text>
"#,
                    xml_escape(line)
                ));
            }
            cx += cw;
        }
        y = bottom;
    }

    svg.push_str("</svg>");
    svg
}

fn wrap_text(s: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 || s.is_empty() {
        return vec![s.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in s.split_whitespace() {
        let fits = if current.is_empty() {
            word.chars().count() <= max_chars
        } else {
            current.chars().count() + 1 + word.chars().count() <= max_chars
        };
        if fits {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            continue;
        }
        // The word doesn't fit. Flush current line first.
        if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        // Now place the (potentially overlong) word on its own line(s),
        // hard-breaking at the character boundary if needed.
        if word.chars().count() <= max_chars {
            current.push_str(word);
        } else {
            let mut buf = String::new();
            for c in word.chars() {
                if buf.chars().count() == max_chars {
                    out.push(std::mem::take(&mut buf));
                }
                buf.push(c);
            }
            if !buf.is_empty() {
                current = buf;
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
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

fn svg_placeholder(text: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="60" viewBox="0 0 400 60" font-family="Georgia, serif" font-size="20">
<rect x="1" y="1" width="398" height="58" fill="white" stroke="black"/>
<text x="200" y="38" text-anchor="middle" fill="gray">{}</text>
</svg>"#,
        xml_escape(text)
    )
}

#[allow(dead_code)]
fn format_ascii_table(rows: &[Vec<String>], aligns: &[Alignment]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let w = display_width(cell);
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    let mut out = String::new();
    let border = build_border(&widths);
    out.push_str(&border);
    out.push('\n');

    for (idx, row) in rows.iter().enumerate() {
        out.push('|');
        for (i, w) in widths.iter().enumerate() {
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            let align = aligns.get(i).copied().unwrap_or(Alignment::Left);
            let padded = pad_cell(cell, *w, align);
            out.push(' ');
            out.push_str(&padded);
            out.push_str(" |");
        }
        out.push('\n');
        // Header underline after the first row.
        if idx == 0 {
            out.push_str(&border);
            out.push('\n');
        }
    }
    out.push_str(&border);
    out
}

fn build_border(widths: &[usize]) -> String {
    let mut s = String::from("+");
    for w in widths {
        s.push_str(&"-".repeat(w + 2));
        s.push('+');
    }
    s
}

fn pad_cell(content: &str, width: usize, align: Alignment) -> String {
    let cw = display_width(content);
    if cw >= width {
        return content.to_string();
    }
    let pad = width - cw;
    match align {
        Alignment::Right => format!("{}{}", " ".repeat(pad), content),
        Alignment::Center => {
            let left = pad / 2;
            let right = pad - left;
            format!("{}{}{}", " ".repeat(left), content, " ".repeat(right))
        }
        _ => format!("{}{}", content, " ".repeat(pad)),
    }
}

fn display_width(s: &str) -> usize {
    // Treat each char as width 1. The reMarkable converter uses a monospace
    // font without CJK width discrimination, and we don't ship those glyphs
    // anyway; ASCII is the design point.
    s.chars().count()
}

fn push_html_text(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
}

fn resolve_local_image(src: &str, base_dir: Option<&Path>) -> Option<(Vec<u8>, String, String)> {
    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
        return None;
    }
    let path = if Path::new(src).is_absolute() {
        PathBuf::from(src)
    } else {
        base_dir?.join(src)
    };
    let bytes = std::fs::read(&path).ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "bin".to_string());
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    Some((bytes, mime.to_owned(), ext))
}

#[allow(dead_code)]
fn _silence_unused_cow(_: CowStr<'_>) {}

/// Legacy: render markdown with no image embedding, returning just XHTML.
pub fn markdown_to_html_fragment(content: &str) -> String {
    render_markdown(content, None).xhtml
}

pub fn extract_h1_title(content: &str, fallback: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return title.to_owned();
            }
        }
    }
    fallback.to_owned()
}

/// Parse a `---`-delimited YAML-ish frontmatter block, char-safe for UTF-8.
pub fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let mut metadata = HashMap::new();
    let mut lines = content.lines();

    let Some(first) = lines.next() else {
        return (metadata, String::new());
    };
    if first.trim() != "---" {
        return (metadata, content.to_owned());
    }

    let mut consumed: usize = first.len();
    consumed += newline_width_after(content, consumed);

    let mut closed = false;
    for line in lines {
        let line_len = line.len();
        if line.trim() == "---" {
            consumed += line_len;
            consumed += newline_width_after(content, consumed);
            closed = true;
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            metadata.insert(key.trim().to_owned(), value.trim().to_owned());
        }
        consumed += line_len;
        consumed += newline_width_after(content, consumed);
    }

    if !closed {
        return (HashMap::new(), content.to_owned());
    }

    let remaining = content.get(consumed..).unwrap_or("").to_owned();
    (metadata, remaining)
}

fn newline_width_after(content: &str, idx: usize) -> usize {
    match content.as_bytes().get(idx) {
        Some(b'\r') if content.as_bytes().get(idx + 1) == Some(&b'\n') => 2,
        Some(b'\n') | Some(b'\r') => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h1_title_extracted() {
        assert_eq!(extract_h1_title("# Hello\n", "fb"), "Hello");
    }

    #[test]
    fn falls_back_when_no_h1() {
        assert_eq!(extract_h1_title("no heading", "fb"), "fb");
    }

    #[test]
    fn frontmatter_basic() {
        let (m, body) = parse_frontmatter("---\ntitle: T\nauthor: A\n---\nbody\n");
        assert_eq!(m.get("title"), Some(&"T".to_string()));
        assert_eq!(m.get("author"), Some(&"A".to_string()));
        assert!(body.contains("body"));
    }

    #[test]
    fn frontmatter_unicode_safe() {
        // Would panic under naive byte slicing
        let (m, body) = parse_frontmatter("---\ntitle: 日本語\n---\n本文\n");
        assert_eq!(m.get("title"), Some(&"日本語".to_string()));
        assert!(body.contains("本文"));
    }

    #[test]
    fn frontmatter_crlf() {
        let (m, _) = parse_frontmatter("---\r\nkey: value\r\n---\r\nbody");
        assert_eq!(m.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn frontmatter_unclosed_returns_full() {
        let raw = "---\ntitle: X\nbody";
        let (m, body) = parse_frontmatter(raw);
        assert!(m.is_empty());
        assert_eq!(body, raw);
    }

    #[test]
    fn table_renders_as_embedded_png_with_asset() {
        let md = "| Item | Qty | Price |\n|------|----:|------:|\n| Pens | 3 | $4.50 |\n";
        let r = render_markdown(md, None);
        assert!(
            r.xhtml.contains("<img src=\"table-001.png\""),
            "expected <img> to PNG, got: {}",
            r.xhtml
        );
        assert_eq!(r.assets.len(), 1, "expected exactly one PNG asset");
        assert_eq!(r.assets[0].mime, "image/png");
        assert!(!r.assets[0].bytes.is_empty());
        assert_eq!(&r.assets[0].bytes[0..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn strikethrough_still_works() {
        let html = markdown_to_html_fragment("~~old~~");
        assert!(html.contains("<del>"));
    }

    #[test]
    fn wrap_text_breaks_long_words() {
        let lines = super::wrap_text("alpha betagamma delta", 6);
        assert!(lines.len() >= 3, "expected multiple lines, got {lines:?}");
        for l in &lines {
            assert!(l.chars().count() <= 6, "line too long: {l:?}");
        }
    }

    #[test]
    fn build_table_svg_includes_outer_border() {
        let rows = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let svg = super::build_table_svg(&rows, &[Alignment::Left, Alignment::Right]);
        assert!(svg.contains("<svg"));
        assert!(svg.contains("stroke=\"black\""));
        assert!(svg.contains(">A<"));
        assert!(svg.contains(">2<"));
    }

    #[test]
    fn image_with_missing_local_falls_back_to_alt() {
        let md = "![an image](does-not-exist.png)";
        let rendered = render_markdown(md, Some(std::path::Path::new("/tmp/nonexistent")));
        assert!(rendered.assets.is_empty());
        assert!(
            rendered.xhtml.contains("[image: an image]"),
            "expected alt-text fallback, got: {}",
            rendered.xhtml
        );
    }

    #[test]
    fn image_with_local_file_is_embedded() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pic.png"), b"\x89PNG\r\n\x1a\nfakebytes").unwrap();
        let md = "![pic](pic.png)";
        let rendered = render_markdown(md, Some(tmp.path()));
        assert_eq!(rendered.assets.len(), 1);
        assert_eq!(rendered.assets[0].mime, "image/png");
        assert!(rendered.xhtml.contains("img-001.png"));
    }
}
