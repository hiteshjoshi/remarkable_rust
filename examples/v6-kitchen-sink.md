# rr self-push kitchen-sink test

This document is one big sanity check for our v6 typed-text writer.
Every paragraph, list, and inline style below should land on the device
as native typed text — not as a PDF, not as an EPUB-converted notebook,
but as a real reMarkable notebook page you can write on top of.

## Plain paragraph styling

A normal paragraph. Just words, no formatting. The default paragraph
style on reMarkable typed text is **Plain** — this whole paragraph
should render with the body font.

Sentences can wrap softly across lines without forcing a paragraph
break. Pulldown-cmark folds soft line breaks into spaces, so this
chunk of text remains one logical paragraph regardless of how the
source file is wrapped.

## Inline emphasis

This sentence has **bold** in it. This sentence has *italic* in it.
This sentence has **bold _with nested italic_ inside** and then plain
after. Bold and italic together render as bold-italic on the device.

Inline code like `let x = 42;` falls back to plain text because v6
typed text has no monospace style. The content survives, just without
the monospace look.

## Headings

The H1 at the top of this document is the document title. H2 and H3
both render as Heading paragraphs on the device — v6 typed text has
only one heading style, not a six-level hierarchy.

### Third-level heading

Below an H3 there is body text. The heading and the paragraph that
follows it should be two distinct paragraph entries with different
styles in the styles map.

## Bullet lists

A flat bullet list:

- first item
- second item
- third item with **bold** inside

A nested bullet list (top level is Bullet, nested level is Bullet2):

- outer one
  - inner a
  - inner b
- outer two
  - inner c

A list with mixed inline styles:

- plain item
- *italic item*
- **bold item**
- item with `inline code` that falls back to plain

## Numbered lists

Numbered lists collapse to bullets in our model because v6 typed text
has no numbered-list style. The content still arrives:

1. first
2. second
3. third

## Long-form prose

Here is a longer paragraph to stress paragraph rollover. The device's
text-engine flows words across lines automatically, so very long
paragraphs are a good way to check that pos_x, pos_y, and width are
all reasonable. If a paragraph wraps oddly or runs off the page, the
default layout constants in v6::markdown need tightening for Paper
Pro's screen geometry.

Another long paragraph follows. The CRDT chain underneath each
paragraph is a sequence of text-item entries plus a trailing newline
whose CrdtId becomes the style key of the next paragraph. The styles
map at the end of the RootTextBlock keys each paragraph to its
opening newline (or to (0, 0) for the very first paragraph).

## Wrap-up

If everything above renders as typed text on the device, the v6 path
is working end-to-end. If the headings look like body text, the
paragraph-style mapping is broken. If only the first paragraph shows,
the CRDT left_id / right_id chain is malformed. If nothing shows,
something fundamental is wrong with the page bundle or the .content
JSON pointing at it.
