---
title: rr format test round 4 - PNG tables
author: rr
description: Tables are now rasterized to PNG locally. The cloud converter strips SVG and HTML tables, but renders embedded raster images.
lang: en
---

# rr format test round 4 — PNG tables

Tables in this document are NOT markdown tables. They are PNGs rendered
locally by the CLI (via SVG → resvg → tiny-skia) and embedded as image
assets in the EPUB. The cloud converter passes images through unchanged,
so what you see on the tablet is exactly what was rendered on the laptop.

---

## 1. A simple table

| Item | Quantity |   Price |
|------|---------:|--------:|
| Pens |        3 |   $4.50 |
| Pads |        1 |     $12 |
| Tags |       24 |   $0.05 |

---

## 2. A wider table with text

| Concept     | One-line definition       | Where it shows up        |
|-------------|---------------------------|--------------------------|
| Tectonic    | Per-region cloud shard    | JWT tectonic claim       |
| Wire format | Bytes we POST             | EPUB to /import/v1/files |
| Convert     | Server-side flag          | rM-Meta.convert=true     |

---

## 3. Centered + right-aligned columns

| Left col | Center col | Right col |
|:---------|:----------:|----------:|
| a        |     b      |         c |
| aa       |     bb     |        cc |
| aaa      |    bbb     |       ccc |
| longer   | wider one  | $1,234.56 |

---

## 4. Text wrapping inside a cell

The middle column has long values that should wrap onto multiple lines
inside the cell without breaking the table layout.

| Section | Notes                                                          | Status |
|---------|----------------------------------------------------------------|--------|
| Tables  | Now rendered as PNGs because the cloud strips SVG and tables.  | OK     |
| Code    | Looks like normal text on the device; no monospace currently.  | OK     |
| Math    | Untested — no LaTeX support.                                   | TBD    |

---

## 5. Single-column table

| Action |
|--------|
| Plan   |
| Build  |
| Ship   |

---

## 6. Bullet records, for comparison

The same data as section 1, but as bullet records (the round-2 winner):

- **Pens** — Quantity: 3 — Price: $4.50
- **Pads** — Quantity: 1 — Price: $12
- **Tags** — Quantity: 24 — Price: $0.05

---

## 7. Headings, paragraphs, lists (verify nothing regressed)

A paragraph with **bold**, *italic*, ***bold-italic***, `inline code`, and
[a link](https://remarkable.com).

### Subheading

1. First step
2. Second step
3. Third step

- top bullet
  - nested bullet

---

## 8. End

If sections 1–5 are real tables with visible grid lines and aligned text,
PNG-rendered tables work and we ship this as the default.
