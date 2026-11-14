//! The conversion between the byte offsets every `Span` holds and the lines
//! and columns the editor protocols speak. The language server and the debug
//! adapter share it, so both count a column the same way.

use ns_syntax::Span;

/// A count or offset as the `uinteger` the protocols speak. A text has not
/// necessarily passed `ns_syntax::source_too_long` when its positions are
/// computed, so a number past `u32::MAX` is clamped to it, never wrapped
/// into a wrong, small position.
fn protocol_number(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// A line and a column, both counted from 0, the column in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

/// Where each line of a text starts, as byte offsets. `\n`, `\r\n` and a lone
/// `\r` are all line breaks, as LSP counts them.
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
                line_starts.push(protocol_number(index + 1));
            }
        }
        LineIndex { line_starts }
    }

    /// The line and column of byte `offset` in `text`, the column counted in
    /// UTF-16 code units -- the one encoding every LSP client must accept.
    /// An offset past the end answers the end, and one inside a character
    /// answers that character's start, so a span that broke `ns-syntax`'s
    /// codepoint alignment is reported slightly off rather than panicking.
    pub fn position(&self, text: &str, offset: u32) -> LineColumn {
        let mut offset = (offset as usize).min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self
            .line_starts
            .partition_point(|start| *start as usize <= offset)
            - 1;
        let line_start = self.line_starts[line] as usize;
        let column = text[line_start..offset].encode_utf16().count();
        LineColumn {
            line: protocol_number(line),
            column: protocol_number(column),
        }
    }

    /// The byte offset of `position` in `text`, the inverse of `position`:
    /// a column past the end of its line answers the line's end, one inside a
    /// surrogate pair the character's start. `None` for a line the text does
    /// not have.
    pub fn offset(&self, text: &str, position: LineColumn) -> Option<u32> {
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
            if units > position.column as usize {
                return u32::try_from(line_start + index).ok();
            }
        }
        u32::try_from(line_start + line.len()).ok()
    }

    /// The byte offset at which `line` starts, `None` for a line the text does
    /// not have.
    pub fn line_start(&self, line: u32) -> Option<u32> {
        self.line_starts.get(line as usize).copied()
    }
}

/// The length of `span` in `text`, in UTF-16 code units.
pub fn utf16_length(text: &str, span: Span) -> u32 {
    let start = (span.start as usize).min(text.len());
    let end = (span.end as usize).clamp(start, text.len());
    text.get(start..end)
        .map_or(0, |slice| protocol_number(slice.encode_utf16().count()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position_of(text: &str, offset: u32) -> LineColumn {
        LineIndex::new(text).position(text, offset)
    }

    fn line_column(line: u32, column: u32) -> LineColumn {
        LineColumn { line, column }
    }

    #[test]
    fn ascii_counts_one_unit_per_byte() {
        assert_eq!(position_of("abc", 2), line_column(0, 2));
    }

    #[test]
    fn a_line_feed_starts_a_new_line() {
        let text = "ab\ncd";
        assert_eq!(position_of(text, 3), line_column(1, 0));
        assert_eq!(position_of(text, 5), line_column(1, 2));
    }

    #[test]
    fn carriage_return_line_feed_is_one_break() {
        let text = "ab\r\ncd";
        assert_eq!(position_of(text, 4), line_column(1, 0));
    }

    #[test]
    fn a_lone_carriage_return_is_a_break() {
        let text = "ab\rcd";
        assert_eq!(position_of(text, 3), line_column(1, 0));
    }

    #[test]
    fn an_umlaut_is_one_utf16_unit_but_two_bytes() {
        let text = "'ä' x";
        // `x` is at byte 5 and UTF-16 unit 4.
        assert_eq!(position_of(text, 5), line_column(0, 4));
    }

    #[test]
    fn an_emoji_is_two_utf16_units_but_four_bytes() {
        let text = "'😀' x";
        // `x` is at byte 7 and UTF-16 unit 5.
        assert_eq!(position_of(text, 7), line_column(0, 5));
    }

    #[test]
    fn the_end_of_the_text_is_a_position() {
        let text = "ab\n";
        assert_eq!(position_of(text, 3), line_column(1, 0));
    }

    #[test]
    fn an_offset_inside_a_character_answers_the_character_start() {
        // A span is codepoint-aligned by `ns-syntax`'s contract; this guards
        // the callers against a violation turning into a slicing panic.
        let text = "ä";
        assert_eq!(position_of(text, 1), line_column(0, 0));
    }

    #[test]
    fn an_offset_past_the_end_answers_the_end() {
        assert_eq!(position_of("ab", 99), line_column(0, 2));
    }

    #[test]
    fn a_position_past_u32_is_clamped() {
        // A four-GiB text cannot be built in a test, so the narrowing every
        // position goes through is checked directly.
        assert_eq!(protocol_number(u32::MAX as usize + 1), u32::MAX);
        assert_eq!(protocol_number(usize::MAX), u32::MAX);
        assert_eq!(protocol_number(7), 7);
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
        assert_eq!(index.offset(text, line_column(0, 99)), Some(6));
        // Inside the surrogate pair: the emoji's start.
        assert_eq!(index.offset(text, line_column(0, 2)), Some(1));
        assert_eq!(index.offset(text, line_column(1, 0)), Some(7));
        assert_eq!(index.offset(text, line_column(2, 0)), None);
    }

    #[test]
    fn utf16_length_counts_surrogate_pairs_twice() {
        let text = "'😀'";
        assert_eq!(utf16_length(text, Span::new(0, 6)), 4);
    }

    #[test]
    fn line_index_counts_columns_in_utf16_code_units() {
        let text = "a\n\"é😀\" x";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, 11), line_column(1, 6));
        assert_eq!(index.offset(text, line_column(1, 6)), Some(11));
    }

    #[test]
    fn line_start_answers_each_line_s_first_byte() {
        let index = LineIndex::new("a\n\"é😀\" x");
        assert_eq!(index.line_start(0), Some(0));
        assert_eq!(index.line_start(1), Some(2));
        assert_eq!(index.line_start(2), None);
    }
}
