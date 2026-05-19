# reMarkable formatting guide

Empirical notes on what the reMarkable cloud's EPUB → notebook converter
actually does with the HTML inside an uploaded EPUB. Updated as we hit new
quirks. The companion `SKILL.md` is the agent-facing summary; this file is
the long version with the reasoning.

## Pipeline at a glance

```
your.md / your.html
       │
       │ rr parses + transforms
       ▼
  XHTML body fragment (+ embedded PNG/JPG/GIF/SVG assets)
       │
       │ rr packages
       ▼
  EPUB 3 zip (mimetype, container, opf, nav, article, assets)
       │
       │ POST /import/v1/files with rM-Meta.convert=true
       ▼
reMarkable cloud — server-side notebook converter
       │
       ▼
  Native .rm notebook on the device
```

The interesting part for layout is the **server-side notebook converter**:
it controls what survives. The rules below are observational.

## What renders, what doesn't

### Survives
- Headings `<h1>`–`<h6>`
- Paragraphs `<p>`
- Inline emphasis: `<strong>`, `<em>`, `<del>`, `<u>`, `<code>` (but
  `<code>` is NOT monospace on the device — just inline)
- Lists: `<ul>`, `<ol>`, `<li>` with nesting
- Blockquotes `<blockquote>`
- Definition lists `<dl>/<dt>/<dd>`
- Horizontal rule `<hr>`
- Links `<a href=…>` (underlined; tap doesn't navigate)
- **Raster images** `<img src="local.png">` — sharp at e-ink resolution

### Stripped or flattened
- `<table>` — collapsed to plain text with bare `|` separators (or worse)
- `<svg>` — stripped entirely; treated as no-op
- `<pre>` — content kept but whitespace collapsed, newlines lost
- Footnotes — removed silently
- `<style>` / `class` — almost all CSS is dropped
- Custom fonts — only the device's bundled fonts work, which means
  effectively Latin script only

### Special cases
- Inline data-URL images (`<img src="data:image/png;base64,…">`) — unverified
- Math (LaTeX `$…$`, MathML) — renders as raw source text
- Emoji — usually appear as tofu (□)
- CJK / Devanagari / Arabic / Cyrillic — tofu (no fonts shipped)

## Working around the limitations

### Tables → PNG (automatic in `rr`)

The CLI intercepts markdown tables, builds an SVG version (with text laid
out by `usvg`, borders + alignment by `tiny-skia`), rasterizes to PNG via
`resvg`, and embeds it as an EPUB asset. The device sees a `<p><img/></p>`
which it renders as a sharp image.

Implementation: `src/markdown.rs::build_table_svg` + `src/raster.rs`.

Trade-offs:
- ✅ Real grid lines, proper alignment, header weight, multi-line cells.
- ✅ Single sharp PNG per table.
- ❌ Text inside the rendered table isn't selectable/searchable on the device.
- ❌ Slightly larger document.

### Diagrams → bring your own PNG

There is no automatic diagram → PNG step yet. If you need a diagram,
generate a PNG yourself and reference it from your markdown/HTML:

```markdown
![Pipeline](images/pipeline.png)
```

The image is embedded the same way table PNGs are.

### Code samples → expect plain text

Code blocks render with no monospace and no syntax highlighting. Acceptable
for short illustrative snippets; for longer code, consider rendering to PNG
externally (e.g., a screenshot of your editor) and embedding as an image.

### Tabular data without a real table

When you have tabular-shaped data but don't want to render a grid (e.g.
because it's small and prose-y), use **bullet records** — the first column
becomes the bold lead-in and the rest are em-dash separated:

```markdown
- **Pens** — qty 3 at $4.50
- **Pads** — qty 1 at $12
- **Tags** — qty 24 at $0.05
```

This renders cleanly and is fully selectable on the device.

## Why we went markdown-first

Internally we evaluated three input formats:

1. **Markdown** — simple, agent-friendly, supports 95% of doc shapes.
   `rr` extends it with auto-rasterized tables.
2. **XHTML** — accepted as a first-class input format for cases markdown
   can't express. Use when you need bespoke layout.
3. **PDF** — rejected. The cloud converter accepts PDFs but renders them
   as PDFs (not notebooks); the result on-device is read-only and feels
   wrong for editable notes.

Default rule for agents: write markdown. Reach for XHTML only when
markdown literally can't express what you need.

## Empirical history (so we don't relearn this)

| Round | What we tried | Result |
|-------|---------------|--------|
| 1     | markdown2pdf → upload as PDF via sync/v3 | Lands as PDF, not a notebook. Rejected. |
| 2     | Local EPUB → /import/v1/files with convert=true | Lands as native .rm notebook. ✓ |
| 3     | `<pre>`-formatted ASCII tables | Whitespace collapsed; reads as a single line. ✗ |
| 4     | Inline `<svg>` table grid | Stripped entirely. ✗ |
| 5     | Bullet records for tables | Works, but feels less structured for dense data. |
| 6     | **Rasterize tables to PNG locally** | Works perfectly. **Shipped.** |

## Useful CLI patterns

```bash
# Quick upload
rr upload notes.md

# Background upload (returns immediately with a job id)
rr upload big-research.md --background

# Override the inferred title
rr upload meeting.md --title "Q2 Planning Sync — 2026-05-20"

# Verify auth
rr status
```

## Things we'd still like to fix

- Folder operations (`mkdir`, `--folder`, `--dir`) — currently broken
  because the relevant endpoints require Auth0 tokens. Either implement
  Auth0 PKCE in `rr auth`, or route folder ops through `sync/v3`.
- Listing (`ls`) and deletion (`rm`) — same auth gate.
- Auto-rasterize inline SVG inside XHTML input — so agents can keep
  using SVG for diagrams and `rr` handles the conversion transparently.
- Diff-based update-in-place: today every upload is a new document, so
  re-uploading the same source creates duplicates.

PRs welcome.
