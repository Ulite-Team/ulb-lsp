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
}
