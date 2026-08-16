# UTF-16 position handling

`src/utf16.rs` converts between `ulb-lang` byte spans and LSP UTF-16
positions/ranges. This is the one place where an Arabic identifier or an
emoji in a comment is a correctness bug rather than a cosmetic one.

## Why the conversions exist

- `ulb_lang::span::Span` is a byte range into the source (`u32` offsets,
  plus a `LineIndex`).
- The LSP protocol addresses characters as **UTF-16 code units**
  (`Position { line, character }`, `Range`).
- For ASCII-only text they are identical. A BMP character (Arabic letter)
  is 2 UTF-8 bytes but 1 UTF-16 unit; an emoji is 4 UTF-8 bytes but 2
  UTF-16 units. Both directions must count code units, not bytes.

## `span_to_range` (byte span → UTF-16 range)

Builds a `LineIndex`, maps both offsets to `Position` via
`offset_to_position`, returns the range. The position's `line` comes from
`LineIndex::line_col`; the column is the sum of `char::len_utf16()` over
the line's characters before the offset.

## `position_to_offset` (UTF-16 position → byte offset)

Walks lines (splitting on `\n`, a trailing `\r` counts as content — same
rule as `ulb_lang`'s `LineIndex`), then walks the line's characters
adding `len_utf16` per char. Boundary rules:

- a column past the end of its line clamps to the line end;
- a column inside a surrogate pair clamps to the pair's start;
- a line past the last line returns `None`.

`None` is not a soft failure: `range_to_offsets` treats it as a clamp to
source end, and the server never panics on a client position.

## `range_to_offsets` (UTF-16 range → byte offset pair)

Inverse of `span_to_range`, used by `Document::apply` to find the region
an incremental change replaces. Start and end are each clamped; the end is
further bounded to `max(end, start)` so a degenerate range can never
produce a backwards slice.

## Round-trip guarantee

The tests pin the invariants that matter:

- ASCII spans map directly (`ascii_span_maps_directly`).
- Multibyte characters before the span advance the UTF-16 column
  (`multibyte_characters_before_span_advance_utf16_column`).
- An emoji counts two UTF-16 units and a column inside the pair clamps to
  its start (`emoji_counts_two_utf16_units`, `position_to_offset_surrogate_pair`).
- Round trips: `span → range → offsets` reproduces the original span
  (`position_to_offset_roundtrips_spans`, `range_to_offsets_roundtrips_span_to_range`).

These conversions are used on the incremental-change path on every
`didChange`, so any regression here corrupts the buffer mid-edit.
