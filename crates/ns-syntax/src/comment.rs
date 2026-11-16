//! A comment as an editor reads it: its form, its span, and its text
//! without comment markers (`docs/design/frontend.md`, "Comments").

use crate::span::Span;

/// Which of the two comment forms a comment is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// §4.2's `(* … *)`, nested ones inside it.
    Block,
    /// ns-next's `///` lines, consecutive ones merged into one comment.
    Line,
}

/// One comment of a source, in source order among its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment {
    pub kind: CommentKind,
    /// `(*` to `*)`, or the first line's `///` up to, not including, the
    /// last line's line break (or the end of the source): a `Line` block's
    /// span takes in its last line's trailing whitespace, which `text`
    /// drops.
    pub span: Span,
}

impl Comment {
    /// The comment's text without its markers, its lines joined with `\n`,
    /// for a reader that shows a comment rather than the source: a `Line`
    /// block without each line's indentation, `///` and one space after
    /// it; a `Block` without `(*` and `*)` and without the indentation its
    /// lines share. Rendering it is the reader's business.
    pub fn text(self, source: &str) -> String {
        let raw = self.span.text(source);
        match self.kind {
            CommentKind::Line => line_text(raw),
            CommentKind::Block => block_text(raw),
        }
    }
}

/// The lines of `text`, each of `\r\n`, `\r` and `\n` read as one line
/// break, so a source's line endings never reach the text: one line more
/// than there are breaks, the last empty after a final break. Borrowed, so
/// that a caller copies only what it keeps of each line.
pub(crate) fn lines(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(text);
    std::iter::from_fn(move || {
        let current = rest?;
        match first_line_break(current.as_bytes()) {
            Some(index) => {
                let width = if current.as_bytes()[index..].starts_with(b"\r\n") {
                    2
                } else {
                    1
                };
                rest = Some(&current[index + width..]);
                Some(&current[..index])
            }
            None => {
                rest = None;
                Some(current)
            }
        }
    })
}

/// The offset of the first `\r` or `\n` in `bytes`; the lexer and the
/// attachment run it over every comment line.
pub(crate) fn first_line_break(bytes: &[u8]) -> Option<usize> {
    first_of_either(bytes, b'\n', b'\r')
}

/// The offset of the first `first` or `second` in `bytes`, both ASCII, so
/// that the offset is a character boundary of the source the bytes are
/// from. A search over bytes, not characters, and over eight of them at a
/// time until a word holds one: it runs over the whole text of every
/// comment.
pub(crate) fn first_of_either(bytes: &[u8], first: u8, second: u8) -> Option<usize> {
    const ONES: u64 = u64::from_ne_bytes([0x01; 8]);
    const HIGHS: u64 = u64::from_ne_bytes([0x80; 8]);
    debug_assert!(first.is_ascii() && second.is_ascii());
    let firsts = u64::from_ne_bytes([first; 8]);
    let seconds = u64::from_ne_bytes([second; 8]);
    // Whether a byte of `word` is zero: only a zero byte borrows into its
    // own high bit while that bit is clear.
    let has_zero_byte = |word: u64| word.wrapping_sub(ONES) & !word & HIGHS != 0;
    let (words, _) = bytes.as_chunks::<8>();
    let skipped = words
        .iter()
        .take_while(|word| {
            let word = u64::from_ne_bytes(**word);
            !has_zero_byte(word ^ firsts) && !has_zero_byte(word ^ seconds)
        })
        .count()
        * 8;
    bytes[skipped..]
        .iter()
        .position(|&byte| byte == first || byte == second)
        .map(|offset| skipped + offset)
}

fn line_text(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    for (index, line) in lines(raw).enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let line = line.trim_start();
        let line = line.trim_prefix("///");
        text.push_str(line.trim_prefix(' ').trim_end());
    }
    text
}

fn block_text(raw: &str) -> String {
    let inner = raw.trim_prefix("(*");
    let inner = inner.trim_suffix("*)");
    let lines: Vec<&str> = lines(inner).collect();
    let Some((first, rest)) = lines.split_first() else {
        return String::new();
    };
    // The continuation lines are usually indented to sit under the first
    // line's text; what they share is layout, not content.
    let shared_indentation = rest
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    let mut text: Vec<&str> = Vec::with_capacity(lines.len());
    text.push(first.trim());
    for line in rest {
        if line.trim().is_empty() {
            text.push("");
        } else {
            text.push(line[shared_indentation..].trim_end());
        }
    }
    let leading_blanks = text.iter().take_while(|line| line.is_empty()).count();
    text.drain(..leading_blanks);
    while text.last().is_some_and(|line| line.is_empty()) {
        text.pop();
    }
    text.join("\n")
}

/// Whether a `///` comment starting at `start` continues the line-comment
/// block `previous`: both start their lines and only whitespace with one
/// line break lies between them. A `///` after code is a remark on that
/// line, so it neither joins the block above nor takes the next line in.
pub(crate) fn continues_line_block(source: &str, previous: Span, start: usize) -> bool {
    // One pass over the gap's bytes, not a search per line ending: the
    // lexer asks this for every `///` line.
    let gap = &source.as_bytes()[previous.end as usize..start];
    let mut line_breaks = 0usize;
    for (index, &byte) in gap.iter().enumerate() {
        match byte {
            b'\n' => line_breaks += 1,
            b'\r' if gap.get(index + 1) != Some(&b'\n') => line_breaks += 1,
            _ if byte.is_ascii_whitespace() => {}
            _ => return false,
        }
    }
    line_breaks == 1 && starts_its_line(source, previous.start as usize)
}

/// Whether only whitespace stands between the start of `offset`'s line and
/// `offset`.
pub(crate) fn starts_its_line(source: &str, offset: usize) -> bool {
    source[..offset]
        .bytes()
        .rev()
        .take_while(|&byte| byte != b'\n' && byte != b'\r')
        .all(|byte| byte.is_ascii_whitespace())
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::cast_possible_truncation,
        reason = "a test's values are small literals and lengths it built"
    )]

    use super::*;

    fn whole(kind: CommentKind, source: &str) -> String {
        Comment {
            kind,
            span: Span::new(0, source.len() as u32),
        }
        .text(source)
    }

    /// The lines answer what replacing each `\r\n` and then each `\r` by
    /// `\n` and splitting at `\n` answers.
    #[test]
    fn the_lines_break_as_the_line_endings_read_as_newlines() {
        for text in [
            "",
            "no break",
            "\n",
            "\r",
            "\r\n",
            "\n\n\r\r\n\r\n",
            "one\r\ntwo\rthree\nfour",
            "a\r\r\nb",
            "a\n\rb",
            "trailing\n",
            "trailing\r",
            "trailing\r\n",
            "\r\nleading",
            "  (* x *)  \r\n\t///  y \r",
        ] {
            let expected: Vec<String> = text
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .split('\n')
                .map(str::to_string)
                .collect();
            let actual: Vec<&str> = lines(text).collect();
            assert_eq!(actual, expected, "{text:?}");
        }
    }

    /// The word-at-a-time search finds the first break wherever it stands
    /// in or after a word, beside bytes one bit away from `\n` and `\r` and
    /// their high-bit twins.
    #[test]
    fn the_first_line_break_is_found_at_every_offset() {
        for filler in [b'a', b'\t', 0x0b, 0x0c, 0x0e, 0x8a, 0x8d, 0xff, 0x00] {
            for length in 0..40 {
                let mut bytes = vec![filler; length];
                assert_eq!(first_line_break(&bytes), None, "{filler} {length}");
                for position in 0..length {
                    for line_break in *b"\n\r" {
                        bytes[position] = line_break;
                        assert_eq!(
                            first_line_break(&bytes),
                            Some(position),
                            "{filler} {length} {position}"
                        );
                        bytes[position] = filler;
                    }
                }
            }
        }
    }

    /// A `///` line continues the block above it exactly when the gap
    /// between them is whitespace holding one line break, `\r\n` one break.
    #[test]
    fn a_line_block_continues_across_one_line_break_of_any_ending() {
        for (gap, continues) in [
            ("\n", true),
            ("\r", true),
            ("\r\n", true),
            ("\n  \t", true),
            ("  \r\n  ", true),
            ("\n\n", false),
            ("\r\r", false),
            ("\n\r", false),
            ("\r\n\r\n", false),
            ("", false),
            ("  ", false),
            ("\n x ", false),
        ] {
            let source = format!("/// a{gap}/// b");
            let start = "/// a".len() + gap.len();
            assert_eq!(
                continues_line_block(&source, Span::new(0, 5), start),
                continues,
                "{gap:?}"
            );
        }
    }

    #[test]
    fn a_line_block_loses_its_markers_and_one_space() {
        let source = "/// First line.\n    ///   indented\n    ///\n    /// last  ";
        assert_eq!(
            whole(CommentKind::Line, source),
            "First line.\n  indented\n\nlast"
        );
    }

    #[test]
    fn a_fourth_slash_is_text() {
        assert_eq!(whole(CommentKind::Line, "////x"), "/x");
        assert_eq!(whole(CommentKind::Line, "///x"), "x");
    }

    #[test]
    fn a_crlf_line_block_has_no_carriage_return() {
        assert_eq!(whole(CommentKind::Line, "/// one\r\n/// two"), "one\ntwo");
    }

    #[test]
    fn a_block_loses_its_markers_and_common_indentation() {
        assert_eq!(
            whole(CommentKind::Block, "(* A module\n   with a header. *)"),
            "A module\nwith a header."
        );
        assert_eq!(
            whole(CommentKind::Block, "(*\n    one\n      two\n*)"),
            "one\n  two"
        );
        assert_eq!(
            whole(CommentKind::Block, "(* a (* b *) c *)"),
            "a (* b *) c"
        );
        assert_eq!(whole(CommentKind::Block, "(**)"), "");
    }

    #[test]
    fn an_unterminated_block_keeps_everything_after_its_opening() {
        assert_eq!(whole(CommentKind::Block, "(* open\nstill"), "open\nstill");
    }
}
