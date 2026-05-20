# Multi-page tables stress test

Six pages, each with different table sizes plus surrounding prose.
Use this to evaluate native v6 layout: text styling, image positioning,
gaps between text and table, gaps between stacked tables.

Pages are split on `---` horizontal rules. Each page can have its own
heading, body text, and one or more tables. Tables come through as
raster PNGs embedded in the v6 page; everything else is native typed
text the device renders with its own font.

---

## Page 2: a tiny table

A 3×3 table — should sit just below this paragraph with a small gap.

| Item | Count | Status |
|------|-------|--------|
| pens |   12  | ok     |
| pads |    3  | low    |
| tags |   55  | fine   |

That's it for this page. The next one has a medium-sized table.

---

## Page 3: a medium 6×4 table

Slightly bigger. Should still fit comfortably with a paragraph above
and breathing room below.

| Quarter | Revenue | Cost   | Margin | Notes              |
|---------|---------|--------|--------|--------------------|
| Q1 2024 | $1.2M   | $0.7M  | 41%    | seed round closed  |
| Q2 2024 | $1.6M   | $0.8M  | 50%    | first hires        |
| Q3 2024 | $2.1M   | $0.9M  | 57%    | pricing change     |
| Q4 2024 | $2.8M   | $1.1M  | 60%    | enterprise wave    |
| Q1 2025 | $3.4M   | $1.3M  | 61%    | partner channel    |
| Q2 2025 | $4.0M   | $1.5M  | 62%    | european launch    |

Below the table: notes that the device should render as a normal
paragraph after enough gap so they don't collide with the image.

---

## Page 4: a wide reference table

This page has a single table with wide columns of mostly text. The
image render scales to fit width 900, so wider columns get smaller cells.

| Concept    | Definition                           | Where it appears        |
|------------|--------------------------------------|-------------------------|
| Tectonic   | Per-region cloud shard               | JWT `tectonic` claim    |
| RootText   | Typed-text payload of one page       | v6 block type 0x07      |
| SceneTree  | CRDT-ordered scene graph             | v6 block type 0x01      |
| ImageItem  | Textured quad referencing a registry | v6 block type 0x0F      |
| Sync v3    | Free content-addressed cloud API     | `internal.cloud...`     |

---

## Page 5: stacked small tables

Two tiny tables stacked vertically. Each should have a gap proportional
to its height so they don't crash into each other.

| Plan  | Price |
|-------|-------|
| Free  | $0    |
| Pro   | $20   |

| Limit  | Free | Pro    |
|--------|------|--------|
| Devices| 1    | 5      |
| Sync   | no   | yes    |

Both tables should appear below the paragraph, one after the other,
with a reasonable gap between them.

---

## Page 6: a heavy data table

Bigger, denser. Tests whether the writer handles a 10-row, 5-column
table without truncating cells or running off the page.

| Date       | Country | Product | Units | Revenue |
|------------|---------|---------|-------|---------|
| 2025-01-03 | US      | Lite    |   140 | $2,800  |
| 2025-01-08 | UK      | Pro     |    62 | $1,860  |
| 2025-01-15 | DE      | Pro     |    49 | $1,470  |
| 2025-01-19 | US      | Pro     |    88 | $2,640  |
| 2025-01-22 | JP      | Lite    |   210 | $4,200  |
| 2025-02-01 | US      | Lite    |   175 | $3,500  |
| 2025-02-07 | UK      | Pro     |    91 | $2,730  |
| 2025-02-14 | DE      | Lite    |   124 | $2,480  |
| 2025-02-20 | US      | Pro     |   103 | $3,090  |
| 2025-02-26 | JP      | Pro     |    78 | $2,340  |

Last page. If everything looks right — text styled, tables sized and
spaced cleanly, no overlap — image embedding is production-ready.
