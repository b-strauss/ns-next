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
    /// From the first comment character to the last: `(*` to `*)`, or the
    /// first line's `///` to the end of the last line's text.
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

/// `text` with each of `\r\n`, `\r` and `\n` read as one line break, so a
/// source's line endings never reach the text.
fn lines_of(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect()
}

fn line_text(raw: &str) -> String {
    lines_of(raw)
        .iter()
        .map(|line| {
            let line = line.trim_start();
            let line = line.strip_prefix("///").unwrap_or(line);
            line.strip_prefix(' ')
                .unwrap_or(line)
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn block_text(raw: &str) -> String {
    let inner = raw.strip_prefix("(*").unwrap_or(raw);
    let inner = inner.strip_suffix("*)").unwrap_or(inner);
    let lines = lines_of(inner);
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
    let mut text: Vec<String> = Vec::with_capacity(lines.len());
    text.push(first.trim().to_string());
    for line in rest {
        if line.trim().is_empty() {
            text.push(String::new());
        } else {
            text.push(line[shared_indentation..].trim_end().to_string());
        }
    }
    while text.first().is_some_and(String::is_empty) {
        text.remove(0);
    }
    while text.last().is_some_and(String::is_empty) {
        text.pop();
    }
    text.join("\n")
}

/// Whether a `///` comment starting at `start` continues the line-comment
/// block `previous`: both start their lines and only whitespace with one
/// line break lies between them. A `///` after code is a remark on that
/// line, so it neither joins the block above nor takes the next line in.
pub(crate) fn continues_line_block(source: &str, previous: Span, start: usize) -> bool {
    let gap = &source[previous.end as usize..start];
    let line_breaks =
        gap.matches('\n').count() + gap.matches('\r').count() - gap.matches("\r\n").count();
    line_breaks == 1
        && gap.bytes().all(|byte| byte.is_ascii_whitespace())
        && starts_its_line(source, previous.start as usize)
}

/// Whether only whitespace stands between the start of `offset`'s line and
/// `offset`.
fn starts_its_line(source: &str, offset: usize) -> bool {
    source[..offset]
        .bytes()
        .rev()
        .take_while(|&byte| byte != b'\n' && byte != b'\r')
        .all(|byte| byte.is_ascii_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(kind: CommentKind, source: &str) -> String {
        Comment {
            kind,
            span: Span::new(0, source.len() as u32),
        }
        .text(source)
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
