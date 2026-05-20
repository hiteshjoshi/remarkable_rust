# reMarkable v6 `.rm` binary format — working notes

These notes describe only the subset of the format we need to write typed-text
notebooks from markdown. Stroke / handwriting blocks are out of scope for now
(we read them only to round-trip-validate against fixtures).

Primary reference: <https://github.com/ricklupton/rmscene>, which in turn
credits ddvk's Go reader. The format is reverse-engineered — there is no spec.

## File layout

```
+----------------+
| 43-byte header |  ASCII: "reMarkable .lines file, version=6          "
+----------------+
|   block 0      |  variable-length, framed (see below)
+----------------+
|   block 1      |
+----------------+
|   ...          |
+----------------+
```

Byte order is **little-endian** everywhere.

## Per-block framing

Each block starts with an 8-byte header:

| offset | size | field             | notes                                |
|--------|------|-------------------|--------------------------------------|
| +0     | u32  | block_length      | length of payload (excludes header)  |
| +4     | u8   | unknown           | always 0                             |
| +5     | u8   | min_version       |                                      |
| +6     | u8   | current_version   |                                      |
| +7     | u8   | block_type        | see table below                      |
| +8     | ...  | payload           | `block_length` bytes of tagged data  |

### Known block types

| type | block                  | notes                              |
|------|------------------------|------------------------------------|
| 0x00 | MigrationInfoBlock     | first block in most files          |
| 0x01 | SceneTreeBlock         | tree structure (CRDT)              |
| 0x02 | TreeNodeBlock          | named node in tree (e.g. "Layer 1")|
| 0x03 | SceneGlyphItemBlock    | highlight                          |
| 0x04 | SceneGroupItemBlock    | group/layer                        |
| 0x05 | SceneLineItemBlock     | handwriting stroke                 |
| 0x06 | SceneTextItemBlock     | inline text                        |
| 0x07 | RootTextBlock          | **typed-text page content**        |
| 0x08 | SceneTombstoneItemBlock| deleted item                       |
| 0x09 | AuthorIdsBlock         | UUID → author_id (u16) map         |
| 0x0A | PageInfoBlock          |                                    |
| 0x0D | SceneInfo              | layer, paper size, visibility      |

For our use case (markdown → typed-text) the **must-write** set is:
`MigrationInfoBlock`, `AuthorIdsBlock`, `PageInfoBlock`, `SceneTreeBlock`
(empty), `TreeNodeBlock` (Layer 1), `SceneInfo`, and the actual
`RootTextBlock` carrying the document text.

## Tagged values inside a block

The block payload is a stream of *tagged* values. Each value is preceded by a
varuint tag where:

- low 4 bits: tag type
- high bits:  field index (semantics depend on the containing block)

Tag types:

| value | name     | what follows the tag                                      |
|-------|----------|-----------------------------------------------------------|
| 0x1   | Byte1    | 1 byte (bool or u8)                                       |
| 0x4   | Byte4    | 4 bytes (u32 or f32)                                      |
| 0x8   | Byte8    | 8 bytes (f64)                                             |
| 0xC   | Length4  | u32 length + that many bytes (a *subblock*)               |
| 0xF   | ID       | a CrdtId (u8 + varuint)                                   |

A `varuint` is LEB128 (each byte stores 7 bits, high bit = continuation).

A `CrdtId` is `{ part1: u8, part2: varuint }`.

A `LwwValue<T>` is encoded as a subblock containing `{ timestamp: CrdtId,
value: T }` — used wherever the device tracks "who set this and when."

A string is a subblock containing `{ length: varuint, is_ascii: u8, utf8:
[u8; length] }`. `is_ascii` is always 1 in observed files; the bytes are
actually UTF-8.

## RootTextBlock (type 0x07) — our target

This is the entire typed-text payload of a notebook page. Structure:

```
block_id: CrdtId (tag index=1)           # always (0, 0)
subblock @ index=2 {
    subblock @ index=1 {                 # text items wrapper
        subblock @ index=1 {
            num_items: varuint
            text_item[num_items]         # see below
        }
    }
    subblock @ index=2 {                 # formatting wrapper
        subblock @ index=1 {
            num_formats: varuint
            text_format[num_formats]     # CrdtId → ParagraphStyle
        }
    }
}
subblock @ index=3 {
    pos_x: f64
    pos_y: f64
}
width: f32 (tag index=4)
```

A **text item** is a CRDT sequence entry — `{ item_id, left_id, right_id,
deleted_length, value }` — where `value` is either a string run or an int
formatting code:

| code | meaning            |
|------|--------------------|
| 1    | bold on            |
| 2    | bold off           |
| 3    | italic on          |
| 4    | italic off         |

Paragraph styles (indexed by the CrdtId of the paragraph's first character):

| value | style              |
|-------|--------------------|
| 0     | BASIC              |
| 1     | PLAIN              |
| 2     | HEADING            |
| 3     | BOLD (paragraph)   |
| 4     | BULLET             |
| 5     | BULLET2 (indent)   |
| 6     | CHECKBOX           |
| 7     | CHECKBOX_CHECKED   |

Paragraphs are delimited by `\n` characters inside the text item stream.
