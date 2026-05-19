---
name: rr
description: >
  Publish documents to a reMarkable Paper Pro tablet as native handwriting-
  editable notebooks. Use whenever the user wants to push notes, summaries,
  meeting recaps, research, or any reference document to their reMarkable —
  including phrases like "save this for remarkable", "push to my tablet",
  "send these notes to remarkable", "rr upload", "make this readable on my
  reMarkable later".
---

# rr — Publish to reMarkable as a Native Notebook

`rr` is a Rust CLI that takes a markdown or XHTML file, packages it as an
EPUB locally, posts it to the reMarkable cloud's `/import/v1/files`
endpoint with `convert=true`, and the cloud converts it into a **native
reMarkable notebook** (the yellow-icon, handwriting-editable kind — NOT a
PDF, NOT an EPUB viewer document).

The agent's job is to (1) generate a well-structured markdown file, (2)
run `rr upload` against it, (3) report the new document's id to the user.

---

## When to use this skill

Activate when the user:

- Says any of: "send this to remarkable", "push this to my tablet", "save
  this for remarkable", "make a remarkable doc", "upload these notes",
  "send these to my reMarkable".
- Asks to compile/format/save a long-running conversation, meeting notes,
  technical decision log, research summary, or reading material in a way
  they can read offline on the tablet.
- Mentions `rr` in the context of document upload/sync.

Do NOT activate for:

- Reading existing tablet content (we don't sync down).
- Editing handwritten notes (the device owns those).
- Anything that should remain ephemeral chat output.

---

## Quick command reference

The `rr` binary lives on the user's `PATH` after install (typically
`/usr/local/bin/rr` or `~/bin/rr`). All paths below assume `rr` resolves.

```bash
rr auth                            # pair the machine (user-driven, see below)
rr status                          # connectivity + token state
rr upload notes.md                 # upload to root → native notebook
rr upload notes.md --background    # fire-and-forget; returns a job id immediately
rr upload notes.html               # same, but XHTML input (rich layout)
rr upload notes.md --title "..."   # override the document title
rr jobs                            # list background jobs
rr logs <job-id>                   # show a background job's output
rr cancel <job-id>                 # send SIGTERM to a running job
rr logout
```

**Currently working reliably:** `auth`, `upload` (to root), `status`,
`logout`, `jobs`, `logs`, `cancel`, `skills`.

**Currently limited:** `ls`, `mkdir`, `rm`, `upload --folder/--dir`.
These hit a different auth gate that the device-pairing token doesn't
satisfy. Until that's resolved, **uploads go to the root of the device**
and the user can drag them into folders on the tablet itself. Don't promise
folder placement.

---

## First-time setup (user must run)

If the user has never run `rr auth`, ask them to run it themselves in
their terminal — the flow needs a browser to enter a pairing code and
should never be initiated by the agent.

```bash
rr auth
# 1. CLI prints a one-time code.
# 2. User visits https://my.remarkable.com/device/browser/connect
# 3. User enters the code, presses pair.
# 4. CLI stores user + device tokens in ~/Library/Application Support/rr/config.toml
```

If `rr status` or `rr upload` returns "token expired — run `rr auth` to
re-pair", surface that exact message and ask the user to re-run. Do not
attempt to re-authenticate the user yourself.

---

## How `rr` renders documents — the things you MUST know

The reMarkable cloud's EPUB → notebook converter is opinionated about
what HTML it preserves. `rr` works around its quirks. Internally:

1. `rr` parses your markdown.
2. **Markdown tables are rasterized to PNG locally** (via a pure-Rust
   pipeline: SVG → resvg → tiny-skia → PNG) and embedded in the EPUB.
   Use regular markdown tables — they will appear as proper grids on
   the device.
3. Local images referenced in markdown (`![alt](path/to/img.png)`) are
   read off disk and embedded too.
4. The rest of the markdown becomes XHTML inside the EPUB.

### What renders well on the device

| Construct | Render? | Notes |
|-----------|---------|-------|
| Headings `# … ######` | Yes | Use H1 for doc title (auto-prepended), H2 for sections |
| Paragraphs | Yes | Justified, e-reader feel |
| `**bold**`, `*italic*`, `~~strike~~` | Yes | Inline emphasis is reliable |
| `` `inline code` `` | Yes | NOT monospace on device, just inline |
| Links `[text](url)` | Yes | Underlined; tap doesn't open browser on device |
| Ordered / unordered lists | Yes | Nesting works |
| Nested bullets | Yes | Multi-level indentation respected |
| Blockquotes `>` | Yes | Single-level looks clean; nested less so |
| Horizontal rule `---` | Yes | Renders as a thin line |
| Local image refs `![](path.png)` | Yes | Embedded as EPUB asset, sharp at e-ink resolution |
| **Markdown tables** | **Yes — auto-rasterized to PNG** | Just write normal markdown tables; see below |
| Code blocks ``` ``` ``` | Yes-ish | Renders as plain paragraphs — no monospace, no syntax highlighting |

### What does NOT render

| Construct | What happens | Workaround |
|-----------|--------------|------------|
| Raw `<table>` in HTML | Flattened to text with `|` separators | Use a markdown table (auto-PNG) or bullet records |
| Inline `<svg>` | Stripped entirely | Generate a PNG yourself, reference as image |
| `<pre>` whitespace | Whitespace collapsed, newlines lost | Don't rely on `<pre>` for ASCII-art layouts |
| Footnotes `[^1]` | Vanish completely | Inline the reference text |
| Definition lists `<dl>` | Mostly works but plain | OK to use; bullet records also fine |
| Task list checkboxes `- [ ]` | Become `[ ]` text | Acceptable; just know the box isn't tap-able |
| Non-Latin scripts (中, 日, हि, ع, …) | Render as tofu (□) — no fonts on device | **Stay in English/Latin**; transliterate if needed |
| Math (LaTeX `$…$`) | Renders as raw `$…$` text | Convert to prose or render to image |
| Emojis | Hit-or-miss (often tofu) | Avoid; use words ("note", "warning", "todo") |

### When `rr upload` rasterizes vs not

- **Tables**: always rasterized to PNG (you don't need to do anything).
- **Local image references**: embedded as-is (PNG, JPG, GIF, SVG, WEBP).
- **Inline SVG in XHTML input**: NOT rasterized — gets stripped by the
  cloud. If you need a diagram, render it to PNG yourself and reference
  it as an image.

---

## Recommended workflow (the happy path)

When the user says "push this to my remarkable":

1. **Generate** a markdown file in `/tmp/` (or another temp dir).
   Pick a descriptive filename: `airwallex-negotiation-2026-05-20.md`,
   not `notes.md`.
2. **Write English-only content** with the constructs that render well
   (see table above).
3. **Upload it** via `rr upload`. For long-running uploads or to keep the
   chat unblocked, pass `--background` and report the job id back.
4. **Report** the document id (and job id if backgrounded) to the user.

### Template — meeting notes

```markdown
---
title: Team Sync 2026-05-20
author: rr
description: Decisions and action items from the weekly sync.
---

# Team Sync — May 20 2026

## Attendees
- Alice (eng)
- Bob (product)
- Charlie (design)

## Decisions
- Move to weekly sprints starting June.
- Hire 2 more backend engineers in Q3.

## Action items
| Owner   | Item                            | Due       |
|---------|---------------------------------|-----------|
| Alice   | Draft sprint cadence proposal   | May 27    |
| Bob     | Update hiring plan in Linear    | May 24    |
| Charlie | Design system audit             | June 5    |

## Discussion notes
Sprint cadence: the team prefers two-week cycles but we agreed to try
weekly for a month and re-evaluate. Bob raised concern about review
overhead; mitigation is to scope each sprint to a single shippable unit.
```

```bash
rr upload /tmp/team-sync-2026-05-20.md
```

### Template — research/thread summary

```markdown
---
title: GPU memory bandwidth notes
description: Why H100 outperforms A100 on attention-heavy workloads.
---

# GPU memory bandwidth notes

## Bottom line
For attention-heavy transformer inference, bandwidth dominates compute.
The H100 wins via HBM3 (3 TB/s) vs A100's HBM2e (2 TB/s) before any
tensor-core gains kick in.

## Numbers

| Card | HBM    | Bandwidth | TF32 | FP16 |
|------|--------|-----------|------|------|
| A100 | HBM2e  | 2 TB/s    | 156  | 312  |
| H100 | HBM3   | 3 TB/s    | 989  | 1979 |

## Sources
- NVIDIA H100 datasheet (link not preserved on device)
- "Tri Dao, FlashAttention-2" Section 3.2
```

```bash
rr upload /tmp/gpu-memory-notes.md --background
# returns: ✓ Background job 20260520143055-a3z8qp started. (...)
```

---

## Background mode — when to use it

Use `--background` whenever:

- You're uploading something larger (multi-page doc with images) where
  the EPUB build + network round-trip might take 10+ seconds.
- You're chaining multiple uploads and don't want each to block.
- The user said "fire and forget" or "in the background".
- You're inside an agent loop that should remain responsive.

The CLI returns immediately with a job id. Output streams to a log file
under `~/Library/Application Support/rr/jobs/<id>.log`. Check status
later with `rr jobs` or `rr logs <id>`.

```bash
rr upload /tmp/big-doc.md --background
# ✓ Background job 20260520143055-a3z8qp started.
#   pid:  64812
#   log:  /Users/.../Application Support/rr/jobs/20260520143055-a3z8qp.log
# Watch with: rr logs 20260520143055-a3z8qp
```

For a normal one-shot upload that you want to confirm completed in the
same turn, omit `--background` — it returns the document id directly.

---

## Choosing markdown vs XHTML — the rule

This is important and easy to get wrong.

**If the document contains any tabular data, write markdown.** The
rasterizer that turns tables into PNGs only fires on markdown table
syntax (`| col | col |`). XHTML `<table>` tags are passed through
verbatim and the cloud converter flattens them to broken pipe-text on
the device.

**Use XHTML only when the document has no tables** but needs structure
markdown can't express (specific section headers, custom typography,
hand-laid layouts). XHTML input bypasses the markdown pipeline entirely,
including the table rasterizer.

Quick decision tree:

- Summary, report, notes, research, anything with rows of data → **markdown**.
- Pure prose with bespoke layout, no tables → XHTML is fine.
- Mix of prose and tables → markdown (you can still use raw HTML inline
  for things markdown can't express, except for tables which must use
  markdown syntax).

```bash
rr upload report.md          # markdown path — tables auto-rasterize
rr upload report.html        # XHTML path — no table rasterization
```

XHTML rules (when you do use it):

- The `<body>` content is extracted automatically; you can ship a full
  document or just a fragment.
- `<title>` populates the document title (unless `--title` overrides).
- `<meta name="author/description/lang/source" …>` populates EPUB metadata.
- **Don't use `<table>`** — it will be flattened by the cloud. If you
  need a table, switch the whole document to markdown.
- **Don't bother with `<svg>`** — it gets stripped. Generate PNGs
  yourself and reference them via `<img src="local.png">`.
- **Don't bother with custom `<style>`** — almost no CSS survives the
  converter.

---

## Content rules (so the device looks good)

1. **Stay in English / Latin script.** No fonts ship on the device for
   CJK, Devanagari, Arabic, etc. — they render as empty boxes (tofu).
   Transliterate proper nouns if needed.
2. **Use descriptive filenames** with dates: `2026-05-20-topic.md`. The
   filename surfaces in the cloud index.
3. **One topic per document.** Long documents are slower to render and
   harder to skim on e-ink. Split if needed.
4. **No emojis.** Use words: "Note:", "Warning:", "Todo:".
5. **Avoid horizontal scrolling.** Long lines of code overflow the page.
   Wrap or shorten before upload.
6. **Front-load the conclusion.** E-ink users skim; put the answer in
   the first paragraph and the supporting detail after.
7. **Use tables for tabular data** — they're auto-rasterized, so they
   look better than `key: value` lists for structured data.
8. **Set `--title`** if the inferred title is generic. Document titles
   are how the user finds the file on the tablet.

---

## Error responses you should map cleanly

| Error message | Cause | Tell the user |
|---------------|-------|---------------|
| `not authenticated; run \`rr auth\` first` | No config or never paired | "Run `rr auth` in your terminal to pair." |
| `token expired — run \`rr auth\` to re-pair` | User token + device token both expired | Same — user must re-pair. |
| `error: network error: …` | Network failure | "Cloud connection failed; retry in a moment." |
| `error: api error: HTTP 5xx …` | Cloud side issue | "reMarkable cloud is having issues; retry shortly." |
| `error: api error: HTTP 4xx …` (other) | Probably content type or meta | Report verbatim; the body usually explains. |

---

## Examples

**"Send this conversation to my remarkable"**

```bash
cat > /tmp/conversation-summary-2026-05-20.md << 'EOF'
---
title: Conversation summary 2026-05-20
description: Topics covered and decisions reached during today's session.
---

# Conversation summary — May 20 2026

## Topics
- ...

## Key decisions
- ...

## Next steps
- ...
EOF

rr upload /tmp/conversation-summary-2026-05-20.md
```

**"Push these meeting notes in the background"**

```bash
rr upload /tmp/q2-planning-meeting.md --background
# Tell the user: "Started background job <id>. Check status with `rr jobs`."
```

**"What's on my remarkable?"**

```bash
rr status
# Note: full listing currently requires Auth0 tokens we don't have.
# Tell the user: "Listing isn't available yet; check your device directly."
```

---

## Limitations (be honest with the user)

- **One-way sync** (local → cloud). No download path.
- **No update-in-place** — every upload creates a new document. Editing
  on the laptop and re-uploading produces a duplicate. Ask the user
  whether they want a fresh copy or to update an existing doc (which
  means deleting the old one on-device first).
- **Folders not yet supported via this CLI.** Uploads land at the
  tablet's root.
- **No `rr ls` / `rr rm` yet.** Use the device directly.

---

## Related

- `/remarkable` — turn the current conversation into a print-ready HTML
  doc (different path; complementary).
- `remarkable-cli` — full SSH-based device management (lower-level).
