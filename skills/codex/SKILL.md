---
name: rr
description: >
  Push markdown to a reMarkable tablet as a native handwriting-editable
  notebook. Use when the user wants to send notes, summaries, meeting
  recaps, research, or any reference doc to their reMarkable — including
  phrases like "save this for remarkable", "push to my tablet", "send
  these notes to remarkable", or any mention of `rr push`.
---

# rr — Publish markdown as a native reMarkable notebook

`rr` is a Rust CLI that turns a markdown file into a native v6 reMarkable
notebook (the yellow-icon, handwriting-editable kind) and uploads it via
the device's cloud sync API. **Works on any reMarkable account — Connect
not required.** Headings, paragraphs, bullets, and prose render as native
typed text; tables render as raster PNGs embedded directly into the
notebook page.

The agent's job:
1. Build a well-structured markdown file from the conversation.
2. Run `rr push <file>`.
3. Report the new document's UUID back.

## When to use

Trigger on user phrases like: "send this to remarkable", "push to my
tablet", "save for reMarkable", "make a remarkable doc", "upload these
notes". Also trigger any mention of the `rr` command in an upload context.

Do **not** activate for: reading existing tablet content (this is a write
path only), editing handwritten notes, or output that should remain in
chat.

## Commands

```bash
rr auth                                  # one-time device pairing
rr status                                # check auth state
rr push <file.md>                        # push a markdown file as a native notebook
rr push <file.md> --title "Custom Name"  # override the doc title
rr push <file.md> --device paper-pro     # default; also: paper-pro-move, rm2
rr push - --title "From stdin"           # read markdown from stdin
rr ls                                    # list cloud documents
rr mkdir "Folder"                        # create a folder
rr rm <doc-uuid>                         # delete by id
```

There's also a hidden legacy command — `rr connect-push` — kept only as
a fallback to the older EPUB→cloud-convert pipeline. Don't use it unless
the user explicitly asks; `push` produces the same native notebook with
no cloud-side conversion.

## What lands on the device

A push delivers a multi-page native notebook:

- **Title** comes from `--title`, else the first `# H1` in the file, else
  the filename stem.
- **Pages** are split on `---` horizontal-rule lines. Each chunk becomes
  one page in the notebook. No `---` ⇒ a single-page notebook.
- **Headings** (`#`, `##`, `###`) all render as the device's Heading style.
- **Paragraphs**, **bullet lists**, **nested bullets** render as native
  typed text in the appropriate style (Plain / Bullet / Bullet2).
- **Tables** are rendered to PNG locally and embedded as image blocks
  below the prose on the page.
- **Inline bold / italic / code** survive as plain text (the v6 typed-text
  engine doesn't have inline emphasis on Paper Pro yet; the content
  arrives, just without the styling).
- **Code blocks, images, footnotes** are silently skipped today.

## Authoring markdown for rr push

A clean structure produces the best on-device result:

```markdown
# Document title

A short intro paragraph or two.

## First section

Body text. Lists work:

- bullet one
- bullet two
  - nested bullet
- bullet three

A small data table:

| Item | Count |
|------|-------|
| pens | 12    |
| pads | 3     |

---

## Second page

More content. Pages split on `---` rules above.

A bigger table on a fresh page lets it claim more space:

| Quarter | Revenue | Notes              |
|---------|---------|--------------------|
| Q1      | $1.2M   | seed round closed  |
| Q2      | $1.6M   | first hires        |
| Q3      | $2.1M   | pricing change     |
```

### Style rules to follow

- **English-only content.** The device has no fallback fonts for CJK,
  Devanagari, etc. — non-Latin scripts render as tofu boxes.
- **No emojis.** They render as tofu too.
- **Plain markdown tables.** No HTML tables, no `<table>` tags, no SVG.
  Tables are detected from `|`-separated rows + a `|---|` delimiter row.
- **Don't write tables in `<pre>` blocks** — they'll be skipped.
- **One H1 at the top.** Use H2/H3 for sections.
- **Use `---` for page breaks** when you want explicit page boundaries.
  A document with no `---` becomes a single (scrollable) page.

## Device targeting

The default is `paper-pro` (Paper Pro, 1620×2160 drawable). Two other
options when the user has a different device:

- `--device paper-pro-move` — Paper Pro Move (8″ color, ~954×1696)
- `--device rm2` — reMarkable 2 (10.3″, 1404×1872)

These set the page dimensions, text frame width, and image width caps to
match the device's screen. Defaulting to paper-pro is fine when the user
doesn't specify; ask only if they bring up a different device.

## Output expectations

Successful `rr push` prints:

```
Building bundle 'Title' for PaperPro...
  doc: <uuid> | pages: N | bytes to upload: M
Uploading via cloud sync v3...
✓ doc <uuid> (root gen <generation>)
```

Report the UUID back to the user. They can find the notebook in their
device's Files view (top-level by default).

## Error handling

- `not authenticated; run rr auth first` — pair first via `rr auth`.
- `token refresh failed` — re-pair via `rr auth`.
- `cloud returned 429` — back off and retry; rr already retries internally.
- `cloud returned 5xx` — transient; same as above.
- Anything else — print the error verbatim, suggest `rr -v push ...` for
  full debug trace.

## What's deliberately not supported (yet)

| Feature | Status | Why |
|---------|--------|-----|
| Inline bold / italic | Renders as plain text | v6 typed-text inline formatting requires a multi-item CRDT layout that the device renders inconsistently |
| Images via `![alt](path)` | Skipped | Image embedding works for tables; arbitrary inline images need additional plumbing |
| Footnotes | Skipped | No native equivalent in v6 typed-text |
| Numbered lists | Collapse to bullets | v6 has no numbered-list style |
| Code blocks | Skipped | No monospace face on device typed-text |
| Non-Latin scripts | Renders as tofu | Device has no fallback fonts |

For unsupported content the agent should either omit it cleanly or rewrite
it as prose / tables / bullets.

## Mental model in three lines

1. Build markdown that uses headings, paragraphs, bullets, and pipe tables.
2. Run `rr push <file.md>`.
3. The user opens the notebook on their device and can annotate it with the
   stylus on top of the native text.
