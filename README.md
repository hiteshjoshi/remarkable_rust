# rr — push anything to your reMarkable, from Claude / OpenCode / Codex

Tell your coding agent:

> *"Send this to my reMarkable."*

A few seconds later a **native reMarkable notebook** (handwriting-editable,
the yellow-icon kind, not a PDF) shows up on your tablet.

`rr` is a small Rust CLI that takes any markdown or HTML, packages it as
an EPUB, ships it to the reMarkable cloud's notebook converter, and
exits. Agents drive it through a SKILL file that `rr` installs for
Claude, OpenCode, and Codex.

---

## The agent flow (this is the main use case)

```bash
# 1. Install the binary + agent SKILL files (no Rust needed)
curl -fsSL https://raw.githubusercontent.com/hiteshjoshi/remarkable_rust/main/install.sh | bash

# 2. Pair with reMarkable (one-time, browser-based)
rr auth

# 3. Restart Claude / OpenCode / Codex so it picks up the new skill.

# 4. Just ask your agent.
```

If you skip step 3 the agent won't know the skill exists and will
default to its usual behaviour. Restart the agent process (close and
reopen the CLI / app) and you're good.

In Claude / OpenCode / Codex, things like:

- *"Summarize this thread and push it to my remarkable."*
- *"Save these meeting notes for my tablet."*
- *"Send this research as something I can read on my reMarkable later."*
- *"Push the Q2 plan to my reMarkable in the background."*

The skill activates, the agent writes a clean markdown file, runs
`rr upload`, and reports the document id. You pick up the tablet and the
document is already there, properly formatted, with real tables (more on
that below), embedded images, and the title at the top.

### What the SKILL gets the agent to do

Tables render as real grids because `rr` rasterizes markdown tables to
PNG locally before upload. The reMarkable converter strips `<table>` and
`<svg>` from EPUBs but passes through embedded images, so this is the
only path that actually works. The agent doesn't have to know any of
this; it writes a normal markdown table and the right thing happens.

The SKILL also tells the agent to stay in Latin script (no fonts ship on
the device for CJK, Devanagari, Arabic, Cyrillic, so anything else
renders as tofu boxes), avoid emojis and `<pre>` ASCII art (the converter
drops or mangles them), pick descriptive filenames with dates so you can
find docs on the tablet, and use `--background` for long uploads so the
chat stays responsive.

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/hiteshjoshi/remarkable_rust/main/install.sh | bash
```

This downloads the latest pre-built binary for your platform, installs
SKILL files into `~/.claude/skills/rr/`, `~/.opencode/skills/rr/`, and
`~/.codex/skills/rr/`, and drops the binary at `~/.local/bin/rr`.
Override the install location with `INSTALL_DIR=$HOME/bin`.

### From source (Rust installed)

```bash
git clone https://github.com/hiteshjoshi/remarkable_rust.git
cd remarkable_rust
./install.sh --dev      # builds release and installs
```

### Platform support

| Platform | Arch | Status |
|----------|------|--------|
| macOS    | Apple Silicon (`aarch64-apple-darwin`) | working |
| macOS    | Intel (`x86_64-apple-darwin`) | working |
| Linux    | `x86_64-unknown-linux-gnu` | working |
| Linux    | `aarch64-unknown-linux-gnu` | working |
| Windows  | – | not yet (binary builds, but the `rr auth` flow is untested) |

The release binary is statically linked from pure-Rust dependencies. No
cairo, no librsvg, no ImageMagick, no headless Chrome.

---

## Tables that look like tables

This was the hard part. The reMarkable cloud's EPUB → notebook converter
strips HTML structure aggressively. `<table>` rows flatten to plain text
with bare `|` separators. `<pre>` whitespace collapses. Inline `<svg>`
disappears entirely. After hours of probing the converter, the only
thing it preserves verbatim is embedded raster images.

So `rr` watches for markdown tables in the source, renders each one to a
PNG locally, and embeds the PNG in the EPUB. The agent just writes
ordinary markdown:

```markdown
| Item | Quantity |   Price |
|------|---------:|--------:|
| Pens |        3 |   $4.50 |
| Pads |        1 |     $12 |
| Tags |       24 |   $0.05 |
```

…and the device sees a sharp 1400-pixel grid with proper borders,
header weight, right-aligned numbers, and multi-line cell text. No
config, no flags.

The full pipeline, all in pure Rust:

```
markdown table  ─── pulldown-cmark events ───►  rows + alignments
                                                  │
                                                  │ build_table_svg
                                                  ▼
                                            SVG with borders, text,
                                            per-column widths, wrapping
                                                  │
                                                  │ usvg parses + lays out
                                                  │ text via system fonts
                                                  ▼
                                            usvg::Tree
                                                  │
                                                  │ resvg paints onto a
                                                  │ tiny-skia Pixmap
                                                  ▼
                                            1400×N PNG bytes
                                                  │
                                                  ▼
                                <img src="table-001.png"> in EPUB
                                + the PNG file under OEBPS/
```

About 250 lines all up (`src/markdown.rs::build_table_svg` plus
`src/raster.rs`). Soft-wrapping happens before rasterization so long
cells don't blow out the grid. Column widths fit the longest cell,
capped at 1400px to match the reMarkable Paper Pro's reading area.
Header rows get bold weight and a thicker underline.

If rasterization fails for any reason `rr` falls back to bullet records
(`**Pens** — Quantity: 3 — Price: $4.50`), which the converter handles
fine. I haven't seen the fallback trigger in real use yet.

The same machinery is available for arbitrary diagrams: build the SVG
yourself, pipe it through `rr::raster::svg_to_png`, embed the PNG. The
plan is to wire this into XHTML input too so inline `<svg>` is
auto-rasterized.

---

## CLI reference

### One-time setup

```bash
rr auth                  # pair the machine with reMarkable cloud (browser-based)
rr status                # verify auth + cloud connectivity
rr logout                # forget credentials
```

### Uploading

```bash
rr upload notes.md                       # markdown → native reMarkable notebook
rr upload report.html                    # XHTML → native reMarkable notebook
rr upload doc.md --title "Custom Title"  # override inferred title
rr upload doc.md --background            # detached job; returns immediately
```

Uploads land at the root of the device by default and use the cloud's
EPUB → notebook conversion (the yellow-icon doc type that's
handwriting-editable on the tablet).

### Background jobs

```bash
rr upload big-research.md --background
# ✓ Background job 20260520143055-a3z8qp started.
#   pid:  64812
#   log:  ~/.../Application Support/rr/jobs/20260520143055-a3z8qp.log

rr jobs                  # list all jobs (running + recent)
rr logs <job-id>         # full log of a job
rr cancel <job-id>       # SIGTERM the job
```

Useful when the agent is uploading something large and you don't want
the chat hanging on EPUB build + network round-trip.

### Skill management

```bash
rr skills --target all          # install SKILL.md into claude/opencode/codex
rr skills --target claude       # one agent
rr skills --dry-run --target all
```

The SKILL files document the upload pipeline plus what renders well on
the device and what doesn't, so agents make the right choices when
generating content.

---

## What `rr` actually does

```
your.md / your.html
       │
       │  rr parses + transforms
       ▼
  XHTML body fragment (+ embedded PNG/JPG/GIF assets)
       │
       │  rr packages
       ▼
  EPUB 3 zip
       │
       │  POST /import/v1/files with rM-Meta.convert=true
       ▼
reMarkable cloud — server-side notebook converter
       │
       ▼
  Native .rm notebook on the device
```

Same pipeline the official "Read on reMarkable" Chrome extension uses
(reverse-engineered from its source maps), plus some extra work to make
markdown look good on the device:

- Markdown tables are rasterized to PNG locally because the cloud
  converter doesn't respect `<table>` or `<pre>` whitespace. Pipeline:
  SVG → `usvg` → `resvg` → `tiny-skia` → PNG.
- Local images (`![](images/x.png)`) are read off disk and embedded.
- XHTML input is accepted directly for cases markdown can't express
  (custom typography, structured sections). The `<body>` is extracted
  and wrapped automatically.

See [`docs/formatting-guide.md`](docs/formatting-guide.md) for the full
record of what survives the converter and what doesn't.

---

## Limitations

- One-way. Local → cloud. No download path.
- No update-in-place. Every upload creates a new document; re-uploading
  the same source makes a duplicate. Delete the old one on the device
  first.
- No folders yet from the CLI. Uploads land at root. `rr ls`, `rr mkdir`,
  and `rr rm` are stubbed pending Auth0 token support; the
  device-pairing token we currently use doesn't satisfy those endpoints.
- No native fonts for non-Latin scripts on the device. Anything outside
  Latin renders as tofu.

---

## Privacy and data flow

Your reMarkable user token is stored locally at
`~/Library/Application Support/rr/config.toml` (macOS) or
`~/.config/rr/config.toml` (Linux), and mirrored to the OS keychain when
possible. Uploads go directly to reMarkable's cloud
(`web.<tectonic>.tectonic.remarkable.com`). Nothing else is contacted.
No analytics, no telemetry, no third-party services.

---

## License

MIT.

---

## Acknowledgements

- The reMarkable team for shipping a great device and a usable cloud
  API.
- The "Read on reMarkable" Chrome extension for shipping source maps,
  which made the reverse-engineering quick.
- The `usvg` / `resvg` / `tiny-skia` crates — without them "tables to
  PNG" wouldn't be a 50-line module.
