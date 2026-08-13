//! Byte-span to UTF-16 LSP range conversion.
//!
//! [`ulb_lang::span::Span`]s are byte ranges into the source text; the LSP
//! protocol addresses characters as UTF-16 code units. The mapping is
//! trivial for ASCII-only lines and only becomes interesting when a line
//! contains multi-byte characters before the span's start.

use lsp_types::{Position, Range};
use ulb_lang::span::{LineIndex, Span};

/// Converts the byte span `span` within `source` into a zero-based UTF-16
/// [`Range`].
///
/// # Examples
///
/// ```
/// use lsp_types::{Position, Range};
/// use ulb_lang::span::Span;
/// use ulb_lsp::utf16::span_to_range;
///
/// let source = "compileSdk 37";
/// let range = span_to_range(source, Span { start: 0, end: 11 });
/// assert_eq!(range, Range::new(Position::new(0, 0), Position::new(0, 11)));
/// ```
#[must_use]
pub fn span_to_range(source: &str, span: Span) -> Range {
    let lines = LineIndex::new(source);
    Range::new(
        offset_to_position(source, &lines, span.start),
        offset_to_position(source, &lines, span.end),
    )
}

/// Maps a byte `offset` to a zero-based `(line, utf16-character)` position.
fn offset_to_position(source: &str, lines: &LineIndex, offset: u32) -> Position {
    let (line, _) = lines.line_col(offset);
    let offset = usize::try_from(offset.min(source.len() as u32)).expect("offset fits usize");
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let character = source[line_start..offset]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    Position::new(line - 1, character)
}

/// Converts a zero-based UTF-16 `Position` within `source` to a byte
/// offset — the inverse of [`span_to_range`].
///
/// A column past the end of its line clamps to the line end; a column in
/// the middle of a surrogate pair clamps to the pair's start. Lines are
/// split on `\n`, matching [`ulb_lang::span::LineIndex`]; a trailing `\r`
/// counts as line content. Returns `None` when `line` is past the last
/// line of the source.
///
/// # Examples
///
/// ```
/// use lsp_types::Position;
/// use ulb_lsp::utf16::position_to_offset;
///
/// let source = "compileSdk 37";
/// assert_eq!(position_to_offset(source, Position::new(0, 11)), Some(11));
/// assert_eq!(position_to_offset(source, Position::new(1, 0)), None);
/// ```
#[must_use]
pub fn position_to_offset(source: &str, position: Position) -> Option<u32> {
    let line = usize::try_from(position.line).ok()?;
    let mut line_start = 0usize;
    let mut current_line = 0usize;
    for (i, byte) in source.bytes().enumerate() {
        if current_line == line {
            break;
        }
        if byte == b'\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }
    if current_line != line {
        return None;
    }
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |i| line_start + i);
    let mut byte = line_start;
    let mut units = 0u32;
    for ch in source[line_start..line_end].chars() {
        if units == position.character {
            return Some(byte as u32);
        }
        let len = ch.len_utf16() as u32;
        if units + len > position.character {
            return Some(byte as u32);
        }
        units += len;
        byte += ch.len_utf8();
    }
    Some(byte as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn pos(line: u32, character: u32) -> Position {
        Position::new(line, character)
    }

    #[test]
    fn ascii_span_maps_directly() {
        let source = "compileSdk 37";
        assert_eq!(
            span_to_range(source, Span { start: 0, end: 11 }),
            Range::new(pos(0, 0), pos(0, 11))
        );
    }

    #[test]
    fn span_on_second_line() {
        let source = "line one\ncompileSdk 37";
        assert_eq!(
            span_to_range(source, Span { start: 9, end: 15 }),
            Range::new(pos(1, 0), pos(1, 6))
        );
    }

    #[test]
    fn multibyte_characters_before_span_advance_utf16_column() {
        let source = "تصيير x";
        let span = Span { start: 11, end: 12 };
        assert_eq!(
            span_to_range(source, span),
            Range::new(pos(0, 6), pos(0, 7))
        );
    }

    #[test]
    fn emoji_counts_two_utf16_units() {
        let source = "🦀 x";
        let span = Span { start: 5, end: 6 };
        assert_eq!(
            span_to_range(source, span),
            Range::new(pos(0, 3), pos(0, 4))
        );
    }

    #[test]
    fn position_to_offset_roundtrips_spans() {
        let source = "line one\ncompileSdk 37";
        for span in [
            Span { start: 0, end: 4 },
            Span { start: 9, end: 15 },
            Span { start: 19, end: 21 },
        ] {
            let range = span_to_range(source, span);
            assert_eq!(position_to_offset(source, range.start), Some(span.start));
            assert_eq!(position_to_offset(source, range.end), Some(span.end));
        }
    }

    #[test]
    fn position_to_offset_clamps_past_line_end_and_rejects_past_lines() {
        let source = "a\nbb";
        assert_eq!(position_to_offset(source, pos(0, 5)), Some(1));
        assert_eq!(position_to_offset(source, pos(1, 5)), Some(4));
        assert_eq!(position_to_offset(source, pos(2, 0)), None);
    }

    #[test]
    fn position_to_offset_multibyte_columns() {
        let source = "تصيير x";
        // Five BMP Arabic letters: one utf16 unit but two utf8 bytes each.
        assert_eq!(position_to_offset(source, pos(0, 6)), Some(11));
    }

    #[test]
    fn position_to_offset_surrogate_pair() {
        let source = "🦀 x";
        // '🦀' is two utf16 units; a column inside the pair clamps to its start.
        assert_eq!(position_to_offset(source, pos(0, 1)), Some(0));
        assert_eq!(position_to_offset(source, pos(0, 2)), Some(4));
    }
}
