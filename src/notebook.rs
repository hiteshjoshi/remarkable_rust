//! Assemble the directory layout xochitl expects for a notebook document.
//!
//! On the device, every notebook is five files keyed by a document UUID:
//!
//! ```text
//! <doc>.metadata          # JSON: top-level metadata (name, parent, type)
//! <doc>.content           # JSON: page list + format settings (cPages, etc.)
//! <doc>.pagedata          # plain text: one template name per page
//! <doc>/<page>.rm         # binary v6: the page's typed-text + strokes
//! <doc>/<page>-metadata.json  # JSON: per-page metadata (layer names)
//! ```
//!
//! Phase 4 will SCP the resulting directory into
//! `/home/root/.local/share/remarkable/xochitl/` and reload xochitl.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::v6::page::{Device, PageOptions};

/// One markdown page in a multi-page notebook.
#[derive(Debug, Clone)]
pub struct PageInput {
    pub markdown: String,
    /// PNG images to embed on this page. Each entry becomes a sibling
    /// file inside the page directory (`<doc>/<page>/<image-uuid>.png`)
    /// and an image block in the page's v6 stream.
    pub images: Vec<PageImageInput>,
}

impl PageInput {
    /// Build a single-page [`PageInput`] from markdown. GFM tables in the
    /// source are extracted, rasterized to PNG, and embedded as images on
    /// the page — the raw `| a | b |` lines never reach the typed-text
    /// renderer (which would otherwise emit them as literal pipe-delimited
    /// text, since the device's text engine has no concept of tables).
    ///
    /// Use [`Self::from_markdown_raw`] if you want the legacy "pass markdown
    /// through verbatim" behavior, e.g. for callers that pre-strip tables
    /// themselves.
    pub fn from_markdown(markdown: impl Into<String>) -> Self {
        Self::from_markdown_with_tables(markdown)
    }

    /// Build a [`PageInput`] from markdown without any table handling. The
    /// markdown is fed verbatim to the stroke renderer. Use this only if
    /// you've already stripped or transformed tables yourself; otherwise
    /// prefer [`Self::from_markdown`].
    pub fn from_markdown_raw(markdown: impl Into<String>) -> Self {
        Self {
            markdown: markdown.into(),
            images: Vec::new(),
        }
    }

    /// Split markdown source into multiple [`PageInput`]s on `---`
    /// horizontal-rule lines. Each chunk becomes one page, with its
    /// tables rendered to images via [`Self::from_markdown_with_tables`].
    /// A doc with no `---` lines becomes a single-page bundle.
    pub fn pages_from_markdown(markdown: &str) -> Vec<Self> {
        let chunks = split_on_hr(markdown);
        chunks
            .into_iter()
            .map(Self::from_markdown_with_tables)
            .collect()
    }

    /// Build a [`PageInput`] from markdown and additionally render any
    /// tables in the source to PNG images that get embedded on the page.
    ///
    /// Tables are *stripped* from the markdown text and rendered as
    /// raster images stacked vertically below the estimated text region.
    pub fn from_markdown_with_tables(markdown: impl Into<String>) -> Self {
        use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};

        let original: String = markdown.into();

        // Pass 1: extract tables. Collect cell text per row, alignments.
        let mut tables: Vec<(Vec<Alignment>, Vec<Vec<String>>)> = Vec::new();
        let mut in_table = false;
        let mut current_align: Vec<Alignment> = Vec::new();
        let mut current_rows: Vec<Vec<String>> = Vec::new();
        let mut current_row: Vec<String> = Vec::new();
        let mut in_cell = false;
        let mut cell_buf = String::new();
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        for ev in Parser::new_ext(&original, opts) {
            match ev {
                Event::Start(Tag::Table(a)) => {
                    in_table = true;
                    current_align = a;
                    current_rows.clear();
                }
                Event::End(TagEnd::Table) => {
                    if !current_rows.is_empty() {
                        tables.push((current_align.clone(), current_rows.clone()));
                    }
                    in_table = false;
                }
                Event::Start(Tag::TableHead | Tag::TableRow) if in_table => {
                    current_row.clear();
                }
                Event::End(TagEnd::TableHead | TagEnd::TableRow) if in_table => {
                    current_rows.push(current_row.clone());
                }
                Event::Start(Tag::TableCell) if in_table => {
                    in_cell = true;
                    cell_buf.clear();
                }
                Event::End(TagEnd::TableCell) if in_table => {
                    in_cell = false;
                    current_row.push(cell_buf.clone());
                }
                Event::Text(s) | Event::Code(s) if in_cell => {
                    cell_buf.push_str(&s);
                }
                _ => {}
            }
        }

        // Pass 2: strip table source from the markdown so the typed-text
        // path doesn't render `| ... | ... |` lines as literal text.
        // A table in GFM starts with a header row containing pipes and is
        // followed by a delimiter row of dashes (`|---|---|`). We drop
        // the entire run by scanning lines.
        let stripped = strip_table_lines(&original);

        // Estimate how far down the text region extends, so we can drop
        // images right after it instead of overlapping. The numbers below
        // are coarse: ~50 chars per line at width=936, ~52 device-units
        // per line height. Headings add a bit extra. Pages with little
        // text get tables high up; text-heavy pages get them low.
        let text_height_estimate = estimate_text_height(&stripped);
        let first_image_y = (234.0 + text_height_estimate + 80.0).max(280.0);

        // Render each table to PNG. Page coord origin is page-centre.
        // MAX_W matches what's expected later in `Bundle::build` when the
        // bundle's `device` is known; this default fits Paper Pro and is
        // tightened per-device downstream if needed.
        let mut images = Vec::new();
        let mut y_cursor: f32 = first_image_y;
        const MAX_W: f32 = 900.0;
        for (aligns, rows) in tables {
            let svg = crate::markdown::build_table_svg(&rows, &aligns);
            let Ok(png) = crate::raster::svg_to_png(&svg) else {
                continue;
            };
            let (w_px, h_px) = png_dimensions(&png).unwrap_or((900, 240));
            let scale = (MAX_W / w_px as f32).min(1.0);
            let w = (w_px as f32) * scale;
            let h = (h_px as f32) * scale;
            images.push(PageImageInput {
                png_bytes: png,
                x: -w / 2.0,
                y: y_cursor,
                w,
                // Gap proportional to image height — keeps stacked tables
                // visually balanced regardless of size.
                h,
            });
            y_cursor += h + (h * 0.15).max(40.0);
        }

        Self {
            markdown: stripped,
            images,
        }
    }
}

/// Split markdown source on `---` horizontal-rule lines. Each segment is
/// returned trimmed; empty segments are dropped so consecutive HRs don't
/// create blank pages.
fn split_on_hr(md: &str) -> Vec<String> {
    let is_hr = |l: &str| {
        let t = l.trim();
        !t.is_empty() && t.chars().all(|c| c == '-') && t.len() >= 3
    };
    let mut out = Vec::new();
    let mut buf = String::new();
    for line in md.lines() {
        if is_hr(line) {
            if !buf.trim().is_empty() {
                out.push(buf.trim().to_string());
            }
            buf.clear();
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf.trim().to_string());
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Coarse text-height estimate in device units. Used to position images
/// just below the text region instead of overlapping it. We approximate:
/// ~50 chars per body line at the default font, ~52 units of line height,
/// ~70 units extra for each heading line.
fn estimate_text_height(md: &str) -> f32 {
    let mut total: f32 = 0.0;
    for line in md.lines() {
        let t = line.trim();
        if t.is_empty() {
            total += 20.0;
            continue;
        }
        let is_heading = t.starts_with('#');
        let chars = t.chars().count();
        let wrapped_lines = ((chars as f32 / 50.0).ceil()).max(1.0);
        let per_line = if is_heading { 70.0 } else { 52.0 };
        total += wrapped_lines * per_line;
    }
    total
}

/// Remove GFM table source lines from a markdown string so the typed-text
/// path doesn't render them as literal pipe-separated text. A table is a
/// run of contiguous lines where the second line matches the delimiter
/// pattern (`| --- | --- |`); we drop the header line, the delimiter,
/// and all following data rows that look table-shaped.
fn strip_table_lines(md: &str) -> String {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = String::with_capacity(md.len());
    let mut i = 0;
    let is_delim = |line: &str| {
        let t = line.trim();
        t.contains('|')
            && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
            && t.contains("---")
    };
    let looks_like_row = |line: &str| {
        let t = line.trim();
        !t.is_empty() && t.starts_with('|') && t.ends_with('|')
    };
    while i < lines.len() {
        // A table appears as: header line (with pipes), delim line, then
        // zero or more data rows. Detect by peeking the next line.
        if i + 1 < lines.len() && lines[i].contains('|') && is_delim(lines[i + 1]) {
            // Skip the header + delim + every following table-shaped row.
            i += 2;
            while i < lines.len() && looks_like_row(lines[i]) {
                i += 1;
            }
            continue;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// Parse the 8-byte PNG signature + 13-byte IHDR chunk to extract width
/// and height. Returns `None` if the bytes aren't a recognisable PNG.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    // IHDR data starts at byte 16 (8 sig + 4 len + 4 type), width then height.
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

/// One image to embed on a page. The bundle assigns a UUID-shaped filename
/// and stores the PNG bytes alongside the page's `.rm` file; the v6 page
/// builder emits the registry + image-item blocks pointing at it.
#[derive(Debug, Clone)]
pub struct PageImageInput {
    pub png_bytes: Vec<u8>,
    /// Top-left x in device units (page origin = center).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Inputs for assembling a notebook bundle. The single required field is
/// at least one [`PageInput`]; the rest have sensible defaults.
#[derive(Debug, Clone)]
pub struct BundleOptions {
    pub visible_name: String,
    pub pages: Vec<PageInput>,
    /// Document UUID. Defaults to a fresh random v4.
    pub doc_uuid: Uuid,
    /// Author UUID stamped on every page's `AuthorIdsBlock`. Defaults to a
    /// fresh random v4; reuse the same value for all pages in a bundle.
    pub author_uuid: Uuid,
    /// Page template name (e.g. `Blank`, `Lined`, `Grid`). Written into
    /// `<doc>.pagedata` and the per-page metadata.
    pub page_template: String,
    /// Parent folder UUID. Empty string places the document at the root,
    /// which is what users see in the device's Files view.
    pub parent: String,
    /// Target device model. Drives `paper_size`, `customZoom*`, and text
    /// frame geometry. Default: Paper Pro.
    pub device: Device,
}

impl BundleOptions {
    pub fn new(visible_name: impl Into<String>, pages: Vec<PageInput>) -> Self {
        Self {
            visible_name: visible_name.into(),
            pages,
            doc_uuid: Uuid::new_v4(),
            author_uuid: Uuid::new_v4(),
            page_template: "Blank".into(),
            parent: String::new(),
            device: Device::default(),
        }
    }

    pub fn with_device(mut self, device: Device) -> Self {
        self.device = device;
        self
    }
}

/// A materialised notebook ready to be written to disk.
pub struct Bundle {
    pub doc_uuid: Uuid,
    /// Top-level `<doc>.metadata` JSON contents.
    pub metadata_json: String,
    /// Top-level `<doc>.content` JSON contents.
    pub content_json: String,
    /// `<doc>.pagedata` text — one template name per page, newline-separated.
    pub pagedata: String,
    /// One entry per page in document order.
    pub pages: Vec<BundlePage>,
}

pub struct BundlePage {
    pub uuid: Uuid,
    /// Per-page `<page>-metadata.json` JSON contents.
    pub metadata_json: String,
    /// Per-page `<page>.rm` binary contents.
    pub rm_bytes: Vec<u8>,
    /// PNG image attachments living under `<doc>/<page>/<image-uuid>.png`.
    /// Empty for text-only pages.
    pub images: Vec<BundleImage>,
}

#[derive(Debug, Clone)]
pub struct BundleImage {
    /// `<image-uuid>.png` — referenced by the v6 ImageRegistry block.
    pub filename: String,
    pub png_bytes: Vec<u8>,
}

impl Bundle {
    /// Build a bundle from markdown pages without writing anything to disk.
    pub fn build(opts: &BundleOptions) -> Result<Self> {
        let now_ms = Utc::now().timestamp_millis().to_string();

        let mut bundle_pages = Vec::with_capacity(opts.pages.len());
        let mut content_pages = Vec::with_capacity(opts.pages.len());
        let mut pagedata_lines = Vec::with_capacity(opts.pages.len());

        for (idx, page) in opts.pages.iter().enumerate() {
            let page_uuid = Uuid::new_v4();
            let page_opts = PageOptions {
                author_uuid: opts.author_uuid,
                device: opts.device,
            };

            // Build per-image filenames + carry the geometry into the v6
            // image builder. We assign each image a fresh UUID-shaped
            // filename so the page directory has stable per-image names.
            let mut page_images_v6: Vec<crate::v6::page::PageImage> =
                Vec::with_capacity(page.images.len());
            let mut bundle_images: Vec<BundleImage> = Vec::with_capacity(page.images.len());
            for img in &page.images {
                let filename = format!("{}.png", Uuid::new_v4());
                page_images_v6.push(crate::v6::page::PageImage {
                    filename: filename.clone(),
                    png_bytes: img.png_bytes.clone(),
                    x: img.x,
                    y: img.y,
                    w: img.w,
                    h: img.h,
                });
                bundle_images.push(BundleImage {
                    filename,
                    png_bytes: img.png_bytes.clone(),
                });
            }

            let rm_bytes = crate::v6::page::build_page_bytes_with_images(
                &page.markdown,
                &page_opts,
                &page_images_v6,
            )?;
            let per_page_meta = serde_json::to_string_pretty(&PerPageMetadata {
                layers: vec![Layer {
                    name: "Layer 1".into(),
                }],
            })?;
            bundle_pages.push(BundlePage {
                uuid: page_uuid,
                metadata_json: per_page_meta,
                rm_bytes,
                images: bundle_images,
            });

            // Per-page CRDT timestamps mirror the on-device pattern:
            // `idx` and `template` get unique sequential ids; `scrollTime`
            // and `verticalScroll` reuse a constant `1:1` bookkeeping
            // timestamp regardless of page. This matches the single-page
            // notebooks the device itself writes.
            let idx_ts = format!("1:{}", 2 + idx as u64 * 2);
            let template_ts = format!("1:{}", 3 + idx as u64 * 2);
            content_pages.push(ContentPage {
                id: page_uuid.to_string(),
                idx: TimestampedString {
                    timestamp: idx_ts,
                    value: page_idx_label(idx),
                },
                modifed: now_ms.clone(),
                scroll_time: TimestampedString {
                    timestamp: "1:1".into(),
                    value: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                },
                template: TimestampedString {
                    timestamp: template_ts,
                    value: opts.page_template.clone(),
                },
                vertical_scroll: TimestampedInt {
                    timestamp: "1:1".into(),
                    value: 0,
                },
            });

            pagedata_lines.push(opts.page_template.clone());
        }

        let first_page_id = bundle_pages
            .first()
            .map(|p| p.uuid.to_string())
            .unwrap_or_default();

        let metadata = DocMetadata {
            type_field: "DocumentType".into(),
            visible_name: opts.visible_name.clone(),
            parent: opts.parent.clone(),
            deleted: false,
            metadatamodified: false,
            modified: false,
            pinned: false,
            synced: false,
            version: 1,
            last_modified: now_ms.clone(),
            last_opened: now_ms,
            last_opened_page: 0,
        };

        let _ = first_page_id; // formerly used for last_opened; native uses empty.

        // Total bytes the device will show in the doc list. Approximated
        // from the page rm files plus per-page metadata.
        let size_in_bytes: usize = bundle_pages
            .iter()
            .map(|p| p.rm_bytes.len() + p.metadata_json.len())
            .sum();

        let content = DocContent {
            cover_page_number: -1,
            // Page geometry comes from the chosen device model.
            // Mismatched values here make pages render empty because
            // xochitl can't map content into the viewport.
            custom_zoom_center_x: 0,
            custom_zoom_center_y: (opts.device.dimensions().1 as i32) / 2,
            custom_zoom_orientation: "portrait".into(),
            custom_zoom_page_height: opts.device.dimensions().1 as i32,
            custom_zoom_page_width: opts.device.dimensions().0 as i32,
            custom_zoom_scale: 1,
            document_metadata: serde_json::Map::new(),
            extra_metadata: serde_json::Map::new(),
            file_type: "notebook".into(),
            font_name: String::new(),
            format_version: 2,
            // keyboardMetadata is the device's "this notebook has typed
            // text content" flag. Without it the notebook is treated as
            // pure-handwriting and the device never loads the RootTextBlock.
            keyboard_metadata: KeyboardMetadata {
                count: 1,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
            line_height: -1,
            orientation: "portrait".into(),
            page_count: opts.pages.len() as i32,
            page_tags: vec![],
            size_in_bytes: size_in_bytes.to_string(),
            tags: vec![],
            text_alignment: "justify".into(),
            text_scale: 1,
            zoom_mode: "bestFit".into(),
            c_pages: CPages {
                last_opened: TimestampedString {
                    timestamp: "0:0".into(),
                    value: String::new(),
                },
                original: TimestampedInt {
                    timestamp: "0:0".into(),
                    value: -1,
                },
                pages: content_pages,
                uuids: vec![CPageUuid {
                    first: opts.author_uuid.to_string(),
                    second: 1,
                }],
            },
        };

        Ok(Bundle {
            doc_uuid: opts.doc_uuid,
            metadata_json: serde_json::to_string_pretty(&metadata)?,
            content_json: serde_json::to_string_pretty(&content)?,
            pagedata: pagedata_lines.join("\n") + "\n",
            pages: bundle_pages,
        })
    }

    /// Write all bundle files into `parent_dir`. Creates the per-document
    /// subdirectory. Does *not* create `parent_dir` itself — caller's
    /// responsibility.
    pub fn write_to(&self, parent_dir: &Path) -> Result<BundlePaths> {
        let doc_str = self.doc_uuid.to_string();
        let metadata_path = parent_dir.join(format!("{doc_str}.metadata"));
        let content_path = parent_dir.join(format!("{doc_str}.content"));
        let pagedata_path = parent_dir.join(format!("{doc_str}.pagedata"));
        let pages_dir = parent_dir.join(&doc_str);

        std::fs::write(&metadata_path, &self.metadata_json).map_err(|source| crate::Error::Io {
            path: metadata_path.clone(),
            source,
        })?;
        std::fs::write(&content_path, &self.content_json).map_err(|source| crate::Error::Io {
            path: content_path.clone(),
            source,
        })?;
        std::fs::write(&pagedata_path, &self.pagedata).map_err(|source| crate::Error::Io {
            path: pagedata_path.clone(),
            source,
        })?;
        std::fs::create_dir_all(&pages_dir).map_err(|source| crate::Error::Io {
            path: pages_dir.clone(),
            source,
        })?;

        let mut page_paths = Vec::with_capacity(self.pages.len());
        for page in &self.pages {
            let rm_path = pages_dir.join(format!("{}.rm", page.uuid));
            let pmeta_path = pages_dir.join(format!("{}-metadata.json", page.uuid));
            std::fs::write(&rm_path, &page.rm_bytes).map_err(|source| crate::Error::Io {
                path: rm_path.clone(),
                source,
            })?;
            std::fs::write(&pmeta_path, &page.metadata_json).map_err(|source| {
                crate::Error::Io {
                    path: pmeta_path.clone(),
                    source,
                }
            })?;

            // Image attachments — each page has its own subdirectory
            // `<doc>/<page-uuid>/<image-uuid>.png` that mirrors the layout
            // the device itself writes for image-bearing notebooks.
            let mut image_paths = Vec::with_capacity(page.images.len());
            if !page.images.is_empty() {
                let page_subdir = pages_dir.join(page.uuid.to_string());
                std::fs::create_dir_all(&page_subdir).map_err(|source| crate::Error::Io {
                    path: page_subdir.clone(),
                    source,
                })?;
                for img in &page.images {
                    let img_path = page_subdir.join(&img.filename);
                    std::fs::write(&img_path, &img.png_bytes).map_err(|source| {
                        crate::Error::Io {
                            path: img_path.clone(),
                            source,
                        }
                    })?;
                    image_paths.push(img_path);
                }
            }

            page_paths.push(PagePaths {
                rm: rm_path,
                metadata: pmeta_path,
                images: image_paths,
            });
        }

        Ok(BundlePaths {
            metadata: metadata_path,
            content: content_path,
            pagedata: pagedata_path,
            pages_dir,
            pages: page_paths,
        })
    }
}

/// Paths produced by [`Bundle::write_to`]. Useful for tests and for the
/// Phase 4 SCP transport, which uploads exactly these files.
#[derive(Debug, Clone)]
pub struct BundlePaths {
    pub metadata: PathBuf,
    pub content: PathBuf,
    pub pagedata: PathBuf,
    pub pages_dir: PathBuf,
    pub pages: Vec<PagePaths>,
}

#[derive(Debug, Clone)]
pub struct PagePaths {
    pub rm: PathBuf,
    /// Per-image file paths under `<doc>/<page-uuid>/`.
    pub images: Vec<PathBuf>,
    pub metadata: PathBuf,
}

/// xochitl sorts pages by the `idx.value` string in document order. The
/// device only cares that values are lex-sortable; we use a simple
/// fixed-width "first letter increments every 26 pages" scheme — `ba`,
/// `bb`, ..., `bz`, `ca`, `cb`, ..., `zz`. Plenty of room (650 pages) for
/// any realistic markdown → notebook conversion.
fn page_idx_label(idx: usize) -> String {
    const MAX: usize = 25 * 26;
    assert!(
        idx < MAX,
        "page_idx_label supports up to {MAX} pages, got {idx}"
    );
    let first = b'b' + (idx / 26) as u8;
    let second = b'a' + (idx % 26) as u8;
    format!("{}{}", first as char, second as char)
}

// ---- JSON shapes ---------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
struct DocMetadata {
    #[serde(rename = "type")]
    type_field: String,
    #[serde(rename = "visibleName")]
    visible_name: String,
    parent: String,
    deleted: bool,
    metadatamodified: bool,
    modified: bool,
    pinned: bool,
    synced: bool,
    version: i32,
    #[serde(rename = "lastModified")]
    last_modified: String,
    #[serde(rename = "lastOpened")]
    last_opened: String,
    #[serde(rename = "lastOpenedPage")]
    last_opened_page: i32,
}

/// `.content` JSON shape that current Paper Pro firmware emits for native
/// notebooks. Field order doesn't matter to the device but matters for diff
/// noise when comparing to reference notebooks — we keep it close to the
/// observed layout.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct DocContent {
    #[serde(rename = "coverPageNumber")]
    cover_page_number: i32,
    #[serde(rename = "customZoomCenterX")]
    custom_zoom_center_x: i32,
    #[serde(rename = "customZoomCenterY")]
    custom_zoom_center_y: i32,
    #[serde(rename = "customZoomOrientation")]
    custom_zoom_orientation: String,
    #[serde(rename = "customZoomPageHeight")]
    custom_zoom_page_height: i32,
    #[serde(rename = "customZoomPageWidth")]
    custom_zoom_page_width: i32,
    #[serde(rename = "customZoomScale")]
    custom_zoom_scale: i32,
    #[serde(rename = "documentMetadata")]
    document_metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(rename = "extraMetadata")]
    extra_metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(rename = "fileType")]
    file_type: String,
    #[serde(rename = "fontName")]
    font_name: String,
    #[serde(rename = "formatVersion")]
    format_version: i32,
    #[serde(rename = "keyboardMetadata")]
    keyboard_metadata: KeyboardMetadata,
    #[serde(rename = "lineHeight")]
    line_height: i32,
    orientation: String,
    #[serde(rename = "pageCount")]
    page_count: i32,
    #[serde(rename = "pageTags")]
    page_tags: Vec<String>,
    #[serde(rename = "sizeInBytes")]
    size_in_bytes: String,
    tags: Vec<String>,
    #[serde(rename = "textAlignment")]
    text_alignment: String,
    #[serde(rename = "textScale")]
    text_scale: i32,
    #[serde(rename = "zoomMode")]
    zoom_mode: String,
    #[serde(rename = "cPages")]
    c_pages: CPages,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct KeyboardMetadata {
    count: i32,
    timestamp: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CPages {
    #[serde(rename = "lastOpened")]
    last_opened: TimestampedString,
    original: TimestampedInt,
    pages: Vec<ContentPage>,
    uuids: Vec<CPageUuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ContentPage {
    id: String,
    idx: TimestampedString,
    /// Yes, typo: "modifed" not "modified". The device emits it this way
    /// and matching it keeps round-trips clean against on-device notebooks.
    modifed: String,
    #[serde(rename = "scrollTime")]
    scroll_time: TimestampedString,
    template: TimestampedString,
    #[serde(rename = "verticalScroll")]
    vertical_scroll: TimestampedInt,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TimestampedString {
    timestamp: String,
    value: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TimestampedInt {
    timestamp: String,
    value: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CPageUuid {
    first: String,
    second: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PerPageMetadata {
    layers: Vec<Layer>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Layer {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_idx_label_increments() {
        assert_eq!(page_idx_label(0), "ba");
        assert_eq!(page_idx_label(1), "bb");
        assert_eq!(page_idx_label(25), "bz");
        assert_eq!(page_idx_label(26), "ca");
        assert_eq!(page_idx_label(51), "cz");
        assert_eq!(page_idx_label(52), "da");
    }

    #[test]
    fn single_page_bundle_has_expected_files() {
        let opts = BundleOptions::new("Test", vec![PageInput::from_markdown("# Hi\n\nWorld")]);
        let bundle = Bundle::build(&opts).unwrap();
        assert_eq!(bundle.pages.len(), 1);
        assert!(bundle.metadata_json.contains("\"visibleName\": \"Test\""));
        assert!(bundle.content_json.contains("\"fileType\": \"notebook\""));
        assert!(bundle.content_json.contains("\"formatVersion\": 2"));
        assert!(bundle.pagedata.contains("Blank"));
        assert!(!bundle.pages[0].rm_bytes.is_empty());
        // The .rm bytes must parse back as a v6 file.
        crate::v6::parse(&bundle.pages[0].rm_bytes).expect("rm bytes parse");
    }

    #[test]
    fn multi_page_bundle_uses_sortable_idx_labels() {
        let opts = BundleOptions::new(
            "Multi",
            (0..3)
                .map(|i| PageInput::from_markdown(format!("# Page {i}")))
                .collect(),
        );
        let bundle = Bundle::build(&opts).unwrap();
        // Sortable labels for the first three pages should be ba, bb, bc.
        for needle in [
            "\"value\": \"ba\"",
            "\"value\": \"bb\"",
            "\"value\": \"bc\"",
        ] {
            assert!(
                bundle.content_json.contains(needle),
                "content.json missing {needle}"
            );
        }
    }

    #[test]
    fn from_markdown_extracts_tables_as_images() {
        // The supern /convert call shape: a single-page bundle built from
        // markdown that contains a GFM table. Tables MUST be rasterized to
        // PNG images on the page; the pipe-delimited source lines MUST
        // NOT leak into the typed-text stream.
        let md = "# Title\n\n\
                  | Col A | Col B |\n\
                  | ----- | ----- |\n\
                  | 1     | 2     |\n\n\
                  More text after the table.";
        let page = PageInput::from_markdown(md);

        // At least one PNG image was produced for the table.
        assert_eq!(page.images.len(), 1, "expected one image for one table");
        let img = &page.images[0];
        assert_eq!(
            &img.png_bytes[0..8],
            b"\x89PNG\r\n\x1a\n",
            "image bytes must be a real PNG"
        );
        assert!(img.w > 0.0 && img.h > 0.0, "image needs positive size");

        // The stroke-text path must not see any of the table source.
        assert!(
            !page.markdown.contains("Col A"),
            "table header leaked into typed text: {:?}",
            page.markdown
        );
        assert!(
            !page.markdown.contains("---"),
            "table delimiter leaked into typed text: {:?}",
            page.markdown
        );
        assert!(
            !page.markdown.contains("| 1"),
            "table data row leaked into typed text: {:?}",
            page.markdown
        );

        // Surrounding prose still flows through.
        assert!(page.markdown.contains("Title"));
        assert!(page.markdown.contains("More text after the table."));

        // And the whole thing assembles into a valid v6 bundle.
        let opts = BundleOptions::new("TableTest", vec![page]);
        let bundle = Bundle::build(&opts).unwrap();
        assert_eq!(bundle.pages.len(), 1);
        assert_eq!(
            bundle.pages[0].images.len(),
            1,
            "image must travel through to the bundle page"
        );
        crate::v6::parse(&bundle.pages[0].rm_bytes).expect("page rm parses");
    }

    #[test]
    fn from_markdown_raw_preserves_table_source() {
        // Escape hatch: callers that want the old "no table extraction"
        // behavior can opt in. The markdown is forwarded verbatim and no
        // images are attached.
        let md = "| A | B |\n| - | - |\n| 1 | 2 |\n";
        let page = PageInput::from_markdown_raw(md);
        assert!(page.images.is_empty());
        assert_eq!(page.markdown, md);
    }

    #[test]
    fn write_to_creates_full_directory_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = BundleOptions::new("Layout", vec![PageInput::from_markdown("alpha")]);
        let bundle = Bundle::build(&opts).unwrap();
        let paths = bundle.write_to(tmp.path()).unwrap();

        assert!(paths.metadata.exists());
        assert!(paths.content.exists());
        assert!(paths.pagedata.exists());
        assert!(paths.pages_dir.exists());
        for page in &paths.pages {
            assert!(page.rm.exists(), "missing {}", page.rm.display());
            assert!(
                page.metadata.exists(),
                "missing {}",
                page.metadata.display()
            );
        }

        // Read the .rm back and confirm it's a parseable v6 file.
        let rm = std::fs::read(&paths.pages[0].rm).unwrap();
        crate::v6::parse(&rm).expect("written .rm parses");
    }
}
