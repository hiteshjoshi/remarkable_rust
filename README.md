# rr — push anything to your reMarkable, from Claude / OpenCode / Codex

Tell your coding agent:

> *"Send this to my reMarkable."*

A few seconds later a **native reMarkable notebook** (handwriting-editable,
the yellow-icon kind, not a PDF) shows up on your tablet.

`rr` is a small Rust CLI that turns markdown into a native v6 reMarkable
notebook locally and uploads it via the device's own cloud sync API.
**Works on any reMarkable account — Connect subscription is not
required.** Agents drive it through a SKILL file that `rr` installs for
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
`rr push`, and reports the document id. You pick up the tablet and the
document is already there, properly formatted, with real tables (more on
that below), embedded images, and the title at the top.

### What the SKILL gets the agent to do

Tables render as real grids because `rr` rasterizes markdown tables to
PNG locally and embeds them as image blocks directly inside the v6
notebook page. Headings, paragraphs, and bullets ship as native typed
text. Split a document into pages by writing `---` between sections.

The SKILL also tells the agent to stay in Latin script (no fonts ship on
the device for CJK, Devanagari, Arabic, Cyrillic, so anything else
renders as tofu boxes), avoid emojis and ASCII art (the typed-text
engine drops or mangles them), and pick descriptive filenames with dates
so you can find docs on the tablet.

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

The v6 typed-text engine on Paper Pro doesn't have a table primitive —
just paragraphs and bullets. So `rr` watches for markdown tables in the
source, renders each one to a PNG locally, and embeds the PNG as an
image block in the page right below the typed text. The agent just
writes ordinary markdown:

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
                          ImageRegistry + ImageItem block in the
                          page's v6 stream (sibling .png on disk)
```

Soft-wrapping happens before rasterization so long cells don't blow out
the grid. Column widths fit the longest cell, capped at 1400px to match
the reMarkable Paper Pro's reading area. Header rows get bold weight and
a thicker underline.

The same machinery is available for arbitrary diagrams: build the SVG
yourself, pipe it through `rr::raster::svg_to_png`, and `rr` will embed
the PNG as a page image.

---

## CLI reference

### One-time setup

```bash
rr auth                  # pair the machine with reMarkable cloud (browser-based)
rr status                # verify auth + cloud connectivity
rr logout                # forget credentials
```

### Pushing

```bash
rr push notes.md                         # markdown → native v6 notebook
rr push doc.md --title "Custom Title"    # override inferred title
rr push doc.md --device paper-pro-move   # also: paper-pro (default), rm2
rr push - --title "From stdin"           # read markdown from stdin
```

Pushes land at the root of the device and produce a native v6 notebook
the device renders directly — no cloud-side conversion step. Split the
markdown into multiple pages with `---` horizontal-rule lines.

### Library management

```bash
rr ls                    # list documents in the cloud
rr ls --folders          # only show folders
rr mkdir "Work/2026"     # create a folder
rr rm <doc-uuid>         # delete by id
```

All of these talk to the same sync v3 endpoints `push` uses, so they
work on any reMarkable account — Connect not required.

### Legacy: EPUB → cloud convert

There's also a hidden `rr connect-push` command that builds an EPUB
locally and posts it to the reMarkable cloud's EPUB → notebook converter
(the original v0.2 pipeline). It's kept only as a fallback; `rr push`
produces the same native notebook with no cloud-side conversion.
`connect-push` is also the one with `--background` plus `rr jobs`,
`rr logs`, `rr cancel` for detached uploads.

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

## What `rr push` actually does

```
your.md
       │
       │  pulldown-cmark events → typed text + tables
       ▼
  RootText block (paragraphs / bullets / headings)
  + per-table PNG → ImageRegistry + ImageItem blocks
       │
       │  rr writes binary v6 streams
       ▼
  Per-page .rm v6 files + .metadata, .content, .pagedata
       │
       │  SHA-256 content-address every blob
       │  PUT /sync/v3/files/<hash>      (per blob)
       │  PUT /sync/v3/files/<doc-index>
       │  PUT /sync/v3/root              (412-retry on race)
       ▼
  Notebook appears on every paired device on next sync
```

The binary v6 format is the same one the device writes to its own
filesystem, so the cloud has nothing to convert — it just stores and
hands the blobs back to the tablet. That's why this path works without a
Connect subscription: it's the same sync protocol every reMarkable
device speaks to `internal.cloud.remarkable.com`.

Some details:

- Markdown tables are rasterized to PNG locally and embedded as v6 image
  blocks. Pipeline: SVG → `usvg` → `resvg` → `tiny-skia` → PNG.
- `--device {paper-pro|paper-pro-move|rm2}` picks the page dimensions
  and text-frame geometry. Default is Paper Pro.
- Splitting on `---` produces a multi-page notebook with one chunk per
  page.

See [`docs/formatting-guide.md`](docs/formatting-guide.md) for the full
record of what renders well on the device.

---

## Limitations

- One-way. Local → cloud. No download path.
- No update-in-place. Every push creates a new document; re-pushing the
  same source makes a duplicate. Delete the old one first.
- Pushes land at the root of the cloud library. `rr mkdir` creates
  folders and `rr rm` removes documents, but `rr push` doesn't take a
  `--folder` flag yet — move docs on the tablet, or fall back to
  `rr connect-push --dir` which does support folder placement.
- No inline emphasis. The v6 typed-text engine on Paper Pro doesn't have
  inline bold/italic/code styling; the text arrives, just without the
  styling. Code blocks, images embedded in markdown, and footnotes are
  silently skipped today.
- No native fonts for non-Latin scripts on the device. Anything outside
  Latin renders as tofu.

---

## Privacy and data flow

Your reMarkable user token is stored locally at
`~/Library/Application Support/rr/config.toml` (macOS) or
`~/.config/rr/config.toml` (Linux), and mirrored to the OS keychain when
possible. Pushes go directly to reMarkable's cloud sync API at
`internal.cloud.remarkable.com` — the same endpoint every reMarkable
device talks to. Nothing else is contacted. No analytics, no telemetry,
no third-party services.

---

## License

MIT.

---

## Acknowledgements

- The reMarkable team for shipping a great device and a usable cloud
  API.
- The [`rmscene`](https://github.com/ricklupton/rmscene) project, whose
  reverse-engineering of the v6 binary format made the native pipeline
  possible.
- The "Read on reMarkable" Chrome extension for shipping source maps,
  which sped up the original EPUB-pipeline reverse-engineering (now the
  hidden `connect-push` fallback).
- The `usvg` / `resvg` / `tiny-skia` crates — without them "tables to
  PNG" wouldn't be a 50-line module.
