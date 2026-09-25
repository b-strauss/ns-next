//! The open documents' text, and the conversion from the byte offsets every
//! `Span` holds to the positions LSP speaks.

use lsp_types::Position;
use ns_syntax::Span;

/// Where each line of a text starts, as byte offsets. LSP counts `\n`,
/// `\r\n` and a lone `\r` as line breaks, so all three do here.
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let bytes = text.as_bytes();
        let mut line_starts = vec![0];
        for (index, byte) in bytes.iter().enumerate() {
            let breaks_here = match byte {
                b'\n' => true,
                b'\r' => bytes.get(index + 1) != Some(&b'\n'),
                _ => false,
            };
            if breaks_here {
                line_starts.push(index as u32 + 1);
            }
        }
        LineIndex { line_starts }
    }

    /// The LSP position of byte `offset` in `text`, its character counted in
    /// UTF-16 code units -- the one encoding every LSP client must accept.
    /// An offset past the end answers the end, and one inside a character
    /// answers that character's start, so a span that broke `ns-syntax`'s
    /// codepoint alignment is reported slightly off rather than panicking.
    pub fn position(&self, text: &str, offset: u32) -> Position {
        let mut offset = (offset as usize).min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self
            .line_starts
            .partition_point(|start| *start as usize <= offset)
            - 1;
        let line_start = self.line_starts[line] as usize;
        let character = text[line_start..offset].encode_utf16().count();
        Position::new(line as u32, character as u32)
    }
}

/// The length of `span` in `text`, in UTF-16 code units.
pub fn utf16_length(text: &str, span: Span) -> u32 {
    let start = (span.start as usize).min(text.len());
    let end = (span.end as usize).clamp(start, text.len());
    text.get(start..end)
        .map_or(0, |slice| slice.encode_utf16().count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn position_of(text: &str, offset: u32) -> Position {
        LineIndex::new(text).position(text, offset)
    }

    #[test]
    fn ascii_counts_one_unit_per_byte() {
        assert_eq!(position_of("abc", 2), Position::new(0, 2));
    }

    #[test]
    fn a_line_feed_starts_a_new_line() {
        let text = "ab\ncd";
        assert_eq!(position_of(text, 3), Position::new(1, 0));
        assert_eq!(position_of(text, 5), Position::new(1, 2));
    }

    #[test]
    fn carriage_return_line_feed_is_one_break() {
        let text = "ab\r\ncd";
        assert_eq!(position_of(text, 4), Position::new(1, 0));
    }

    #[test]
    fn a_lone_carriage_return_is_a_break() {
        let text = "ab\rcd";
        assert_eq!(position_of(text, 3), Position::new(1, 0));
    }

    #[test]
    fn an_umlaut_is_one_utf16_unit_but_two_bytes() {
        let text = "'ä' x";
        // `x` is at byte 5 and UTF-16 unit 4.
        assert_eq!(position_of(text, 5), Position::new(0, 4));
    }

    #[test]
    fn an_emoji_is_two_utf16_units_but_four_bytes() {
        let text = "'😀' x";
        // `x` is at byte 7 and UTF-16 unit 5.
        assert_eq!(position_of(text, 7), Position::new(0, 5));
    }

    #[test]
    fn the_end_of_the_text_is_a_position() {
        let text = "ab\n";
        assert_eq!(position_of(text, 3), Position::new(1, 0));
    }

    #[test]
    fn an_offset_inside_a_character_answers_the_character_start() {
        // A span is codepoint-aligned by `ns-syntax`'s contract; this guards
        // the server against a violation turning into a slicing panic.
        let text = "ä";
        assert_eq!(position_of(text, 1), Position::new(0, 0));
    }

    #[test]
    fn an_offset_past_the_end_answers_the_end() {
        assert_eq!(position_of("ab", 99), Position::new(0, 2));
    }

    #[test]
    fn utf16_length_counts_surrogate_pairs_twice() {
        let text = "'😀'";
        assert_eq!(utf16_length(text, ns_syntax::Span::new(0, 6)), 4);
    }
}
