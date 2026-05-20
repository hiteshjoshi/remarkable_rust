# A morning with the reMarkable Paper Pro

This is a deliberately-long document. It exists to push the typed-text
renderer through every paragraph style, every inline emphasis, every
list shape — and to stress the page-rollover logic if the content
spills past one page. If you can read this paragraph as flowing body
text on your device, the v6 writer is doing its job.

## What this test covers

Each section below is annotated. If a section is missing on the device
or renders with the wrong style, that section's name tells you which
part of the pipeline misbehaved.

- headings at three depths
- plain prose paragraphs of mixed length
- inline **bold**, *italic*, and **bold *with italic inside***
- inline code (currently falls back to plain text)
- bullet lists, including nested bullets
- numbered lists (collapse to bullets in this writer)
- a long-form section to force paragraph rollover
- mixed inline emphasis inside list items

If something looks wrong, the first place to look is `src/v6/markdown.rs`
where pulldown-cmark events are mapped to v6 paragraph styles and CRDT
text-item entries.

## Section one: prose

The Paper Pro screen is 1620 by 2160 device units. The drawable area
inside that, after margins, is roughly 936 by 1872. Typed text wraps
at the column boundary the way a normal word processor does — soft
breaks become spaces, hard breaks would force a new line if we
emitted them (we don't, currently).

This second paragraph in the same section exists so you can verify
that there's vertical spacing between paragraphs, and that two plain
paragraphs in a row look the same as one paragraph with manual
linebreaks would *not*.

And a third paragraph, just to make sure the styles map keys are
attaching to the right starting newlines. Under the hood, every
paragraph after the first one is keyed by the CRDT id of the newline
character that terminated the previous paragraph. Get that wrong and
either the wrong style applies or no style applies.

### Section one point one: emphasis primitives

This sentence has a *single italic* word. This one has a **single
bold** word. This sentence has **both bold and *nested italic*
together** which renders as bold-italic. This sentence has `inline
code` that we treat as plain text — there is no monospace face on
the device's typed-text engine.

Mixing emphasis across longer runs: **bold across several words to
verify** that the format-on / format-off CRDT codes apply to a span
and don't bleed into the rest of the paragraph. Same idea with
*italic spanning several words*. And finally **bold here then
*flipping to italic mid-sentence* and back**. Each of those
flips is one v6 format-code item — code 1 turns bold on, 2 turns
it off, 3 turns italic on, 4 turns it off.

### Section one point two: line wrapping

Here is a deliberately long sentence with no internal line breaks
that we feed to the device in order to verify the text engine wraps
sensibly across multiple lines without losing words or stamping
extra blank lines into the rendered output, which is the kind of
thing that goes wrong silently if pos_x or pos_y is off by a few
device units relative to what the page geometry expects.

## Section two: structured lists

A flat bullet list with three items:

- the first item is a short one
- the second item has **inline bold** inside it
- the third item has *italic* and ends with `code` that becomes plain

A nested bullet list — outer items render with the `Bullet` style,
inner items render with the `Bullet2` style (a single indentation
level on the device):

- buying groceries
  - vegetables
  - bread and pastries
  - coffee beans
- house chores
  - vacuum the rug
  - water the plants
  - take out the trash
- weekend plans
  - long walk
  - read a book
  - cook something new

A numbered list collapses to bullets in this writer because v6
typed-text has no numbered-list style:

1. wake up
2. drink water
3. plan the day

A list with longer items that should wrap inside each bullet:

- the first bullet has enough text to flow across more than one line
  on the device, which exercises the text engine's ability to keep
  the bullet glyph anchored to the first line of the item
- the second bullet is similarly long; if both bullets render cleanly
  across line wraps, the markdown-to-v6 mapping is producing one
  paragraph per item with the correct paragraph style, and the text
  engine is applying the bullet glyph based on that style
- a short third item to close the list

## Section three: long prose for rollover

Long paragraphs help test page rollover. A reMarkable page has a
fixed amount of vertical space; once typed text overflows it, the
device should automatically continue onto the next page. Our writer
doesn't yet split pages — it emits everything into one RootTextBlock
and lets the device handle layout. If the content here spills past
the visible area on the device, that's the device handling
overflow, not us.

A second long-form paragraph. The architectural decision behind
"emit one RootTextBlock for the whole document" rather than
"pre-compute page breaks at write time" is that the device knows
its own screen dimensions, fonts, and margins better than we do.
Forcing breaks at the writer level would require us to replicate
the device's font metrics, which is a brittle path.

A third long paragraph. If you see this paragraph at all, page
overflow is being handled by the device rather than truncated by
the writer. If you see it but the text gets cut off mid-word or
mid-line at the bottom of the visible area, the device is doing
its best with the constraints we gave it.

## Section four: edge-case inline content

Sentences with punctuation: hyphens-and-em-dashes — like that one
— should render as plain characters. Parentheses (such as these)
and quotes — both "double" and 'single' — go through verbatim.
Numbers like 3.14159 and dates like 2026-05-20 are just text to
the device.

Inline emphasis that abuts punctuation: **bold,** *italic;*
**bold-then-comma**, *italic-then-period.*

Adjacent emphasis ranges: **first bold**, then *first italic*,
back to **second bold**. Each transition is its own format-code
pair in the v6 stream.

## Section five: closing notes

If you've read this far on the device, the typed-text writer is
working end-to-end. Bold, italic, bullets, nested bullets, headings,
and prose all flowed through the cloud sync API and arrived as a
native notebook page you can write annotations on top of with the
stylus.

If something rendered wrong, the document's `.content` JSON and the
v6 page bytes are both worth inspecting:

- `.content` controls how the device discovers the page and its
  template
- the v6 page bytes (`<page-uuid>.rm`) carry the CRDT text-item
  stream and the paragraph-styles map

The next milestone is embedding raster images as v6 scene items so
markdown tables (rendered locally as PNGs) can appear on these
typed-text pages. That work lives behind a different code path —
the existing `markdown.rs` rasterizer feeds into the EPUB pipeline
today; we'd need to attach the same PNG bytes as image blocks in
the page's v6 stream.

End of test document. If there is a paragraph after this one, page
rollover happened.
