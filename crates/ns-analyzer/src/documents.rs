//! The open documents' text, and the conversion from the byte offsets every
//! `Span` holds to the positions LSP speaks.

use lsp_types::Position;
use ns_syntax::Span;

/// A count or offset as the `uinteger` LSP speaks. An editor's text has not
/// necessarily passed `ns_syntax::source_too_long` when its positions are
/// computed, so a number past `u32::MAX` is clamped to it, never wrapped
/// into a wrong, small position.
fn lsp_number(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

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
                line_starts.push(lsp_number(index + 1));
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
        Position::new(lsp_number(line), lsp_number(character))
    }

    /// The byte offset of LSP `position` in `text`, the inverse of
    /// `position`: its character counted in UTF-16 code units, one past the
    /// end of its line answering the line's end, one inside a surrogate pair
    /// the character's start. `None` for a line the text does not have.
    pub fn offset(&self, text: &str, position: Position) -> Option<u32> {
        let line_start = *self.line_starts.get(position.line as usize)? as usize;
        let line_end = self
            .line_starts
            .get(position.line as usize + 1)
            .map_or(text.len(), |next| *next as usize);
        let line = text.get(line_start..line_end)?;
        let line = line.trim_end_matches(['\n', '\r']);
        let mut units = 0;
        for (index, character) in line.char_indices() {
            units += character.len_utf16();
            if units > position.character as usize {
                return u32::try_from(line_start + index).ok();
            }
        }
        u32::try_from(line_start + line.len()).ok()
    }
}

/// The length of `span` in `text`, in UTF-16 code units.
pub fn utf16_length(text: &str, span: Span) -> u32 {
    let start = (span.start as usize).min(text.len());
    let end = (span.end as usize).clamp(start, text.len());
    text.get(start..end)
        .map_or(0, |slice| lsp_number(slice.encode_utf16().count()))
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
    fn a_position_past_u32_is_clamped() {
        // A four-GiB text cannot be built in a test, so the narrowing every
        // position goes through is checked directly.
        assert_eq!(lsp_number(u32::MAX as usize + 1), u32::MAX);
        assert_eq!(lsp_number(usize::MAX), u32::MAX);
        assert_eq!(lsp_number(7), 7);
    }

    /// Every offset at a character's start, on every kind of line break,
    /// comes back from its position.
    #[test]
    fn an_offset_is_its_position_read_back() {
        let text = "a'😀'ä\r\nb\rc\n";
        let index = LineIndex::new(text);
        for (offset, _) in text.char_indices() {
            let offset = u32::try_from(offset).expect("a short text");
            let position = index.position(text, offset);
            let line_end = text[offset as usize..].starts_with(['\r', '\n']);
            if !line_end {
                assert_eq!(index.offset(text, position), Some(offset), "{position:?}");
            }
        }
    }

    #[test]
    fn a_position_past_its_line_or_inside_a_pair_is_clamped() {
        let text = "'😀'\nb";
        let index = LineIndex::new(text);
        assert_eq!(index.offset(text, Position::new(0, 99)), Some(6));
        // Inside the surrogate pair: the emoji's start.
        assert_eq!(index.offset(text, Position::new(0, 2)), Some(1));
        assert_eq!(index.offset(text, Position::new(1, 0)), Some(7));
        assert_eq!(index.offset(text, Position::new(2, 0)), None);
    }

    #[test]
    fn utf16_length_counts_surrogate_pairs_twice() {
        let text = "'😀'";
        assert_eq!(utf16_length(text, ns_syntax::Span::new(0, 6)), 4);
    }
}
