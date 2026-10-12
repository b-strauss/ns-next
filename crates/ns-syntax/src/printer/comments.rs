//! Comments lowered to documents where the attachment put them
//! (`attach.rs`): before a node, after it, or between two of its own tokens.
//!
//! How a comment is written follows its placement and what must keep it
//! with its owner when the result is parsed again. A comment on a line of
//! its own stays on a line of its own. One that ended its line still ends
//! it: it never counts toward the width, breaks the groups around it, and
//! the next text starts a new line. One between two tokens stays between
//! them, a space on each side but next to an opening delimiter. A comment
//! that leads a node right after a sibling (no token of the parent
//! between) stood after a line break, or it would trail the sibling, and
//! so stood on a line of its own: it keeps that line.

use super::document::{concat, hard_line, text, Document};
use crate::ast::{Anchor, AttachedComment, Placement};
use crate::comment::CommentKind;

/// The comments leading a node, written in front of it.
pub(super) fn before(comments: &[AttachedComment]) -> Document {
    concat(
        anchored(comments, Anchor::Before)
            .map(|comment| match comment.placement {
                Placement::EndOfLine => ending_the_line(comment, false),
                Placement::Inline | Placement::OwnLine => leading_one(comment),
            })
            .collect(),
    )
}

/// One element of a sequence that a container lays out one per line, its
/// comments around it: those leading it that ended the line before,
/// written there; `line_break`; its other leading comments, each on a line
/// of its own above it or in front of it on its line; the element (its
/// period included); those trailing it.
pub(super) fn on_its_line(
    comments: &[AttachedComment],
    line_break: Document,
    element: Document,
) -> Document {
    concat(vec![
        before_line_break(comments),
        line_break,
        leading_on_its_line(comments),
        element,
        after(comments),
    ])
}

/// The comments leading a node that ended the line before it, written
/// there, before the line break its container puts the node after.
pub(super) fn before_line_break(comments: &[AttachedComment]) -> Document {
    concat(
        anchored(comments, Anchor::Before)
            .filter(|comment| comment.placement == Placement::EndOfLine)
            .map(|comment| ending_the_line(comment, true))
            .collect(),
    )
}

/// The other comments leading a node a container puts on a line of its
/// own, after the line break (`before_line_break`).
pub(super) fn leading_on_its_line(comments: &[AttachedComment]) -> Document {
    concat(
        anchored(comments, Anchor::Before)
            .filter(|comment| comment.placement != Placement::EndOfLine)
            .map(leading_one)
            .collect(),
    )
}

/// The comments trailing a node, written after it.
pub(super) fn after(comments: &[AttachedComment]) -> Document {
    concat(
        anchored(comments, Anchor::After)
            .map(|comment| match comment.placement {
                Placement::Inline => trailing(comment),
                Placement::EndOfLine => ending_the_line(comment, false),
                Placement::OwnLine => on_its_own_line(comment),
            })
            .collect(),
    )
}

/// A node's own tokens, counted as they are written as `attach.rs` counts
/// them, and the comments due between them (`Inner(n)`, after the `n`-th
/// and the children after it): each call answers the comments that stand
/// before the token it counts, to be written right
/// after what was written last, before any space or line break.
pub(super) struct OwnTokens<'comments> {
    comments: &'comments [AttachedComment],
    count: u32,
    last: Written,
    /// The tokens counted next stand in a part laid out one line per
    /// element: every comment between them gets a line of its own.
    on_own_lines: bool,
}

/// What was written since the last own token was counted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Written {
    Token,
    /// An opening delimiter, which no space follows.
    Opener,
    /// A child node: a comment between it and the next own token stood
    /// on a line after it, or it would trail the child.
    Child,
}

impl<'comments> OwnTokens<'comments> {
    /// Nothing is written yet, as after an opening delimiter: only the
    /// module has a comment before its first own token.
    pub(super) fn new(comments: &'comments [AttachedComment]) -> OwnTokens<'comments> {
        OwnTokens {
            comments,
            count: 0,
            last: Written::Opener,
            on_own_lines: false,
        }
    }

    /// Whether the tokens counted next stand in a part laid out one line
    /// per element.
    pub(super) fn on_own_lines(&mut self, on_own_lines: bool) {
        self.on_own_lines = on_own_lines;
    }

    /// The comments before a token that is no closer, which is counted.
    pub(super) fn next(&mut self) -> Document {
        self.counted(false, Written::Token)
    }

    /// The comments before an opening delimiter, which is counted.
    pub(super) fn next_opener(&mut self) -> Document {
        self.counted(false, Written::Opener)
    }

    /// The comments before a closing delimiter, which is counted.
    pub(super) fn next_closer(&mut self) -> Document {
        self.counted(true, Written::Token)
    }

    /// The comments before `tokens` own tokens written as one text (a
    /// pattern literal), each counted.
    pub(super) fn skip(&mut self, tokens: u32) -> Document {
        let mut parts = Vec::new();
        for _ in 0..tokens {
            parts.push(self.next());
        }
        concat(parts)
    }

    /// A child node was written.
    pub(super) fn child(&mut self) {
        self.last = Written::Child;
    }

    /// The comments after the last own token, before the node ends.
    pub(super) fn end(&self) -> Document {
        let style = self.style(true);
        between(self.comments, self.count, style)
    }

    fn counted(&mut self, closer: bool, then: Written) -> Document {
        let style = self.style(closer);
        let document = between(self.comments, self.count, style);
        self.count += 1;
        self.last = then;
        document
    }

    fn style(&self, closer: bool) -> Between {
        if self.on_own_lines {
            return Between::Lines;
        }
        match (self.last, closer) {
            (Written::Token, _) => Between::Tokens,
            (Written::Opener, false) => Between::AfterOpener,
            (Written::Opener, true) => Between::Alone,
            (Written::Child, _) => Between::Lines,
        }
    }
}

/// Where an inner comment stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Between {
    /// After a token: a space before it.
    Tokens,
    /// After an opening delimiter: a space after it.
    AfterOpener,
    /// Between an opening and a closing delimiter: no space.
    Alone,
    /// On a line of its own.
    Lines,
}

fn anchored(
    comments: &[AttachedComment],
    anchor: Anchor,
) -> impl Iterator<Item = &AttachedComment> {
    comments
        .iter()
        .filter(move |comment| comment.anchor == anchor)
}

/// A comment leading a node: in front of it on its line, or on a line of
/// its own above it.
fn leading_one(comment: &AttachedComment) -> Document {
    match comment.placement {
        Placement::Inline => concat(vec![
            Document::LeadingStart,
            body(comment),
            Document::LeadingEnd,
        ]),
        Placement::OwnLine | Placement::EndOfLine => on_its_own_line(comment),
    }
}

/// The comments at `Inner(count)`, written as `place` asks.
fn between(comments: &[AttachedComment], count: u32, place: Between) -> Document {
    concat(
        anchored(comments, Anchor::Inner(count))
            .map(|comment| match (comment.placement, place) {
                (Placement::Inline, Between::Tokens) => trailing(comment),
                (Placement::Inline, Between::AfterOpener) => concat(vec![body(comment), text(" ")]),
                (Placement::Inline, Between::Alone) => body(comment),
                (Placement::EndOfLine, _) => ending_the_line(comment, false),
                (Placement::OwnLine, _) | (Placement::Inline, Between::Lines) => {
                    on_its_own_line(comment)
                }
            })
            .collect(),
    )
}

/// A comment after a token on its line, a space between: one that ends up
/// ending the line counts toward the width no more than one written so.
fn trailing(comment: &AttachedComment) -> Document {
    match (comment.kind, comment.lines.as_slice()) {
        (CommentKind::Block, [line]) => Document::Trailing(format!(" {line}")),
        _ => concat(vec![text(" "), body(comment)]),
    }
}

/// A comment alone on its line.
fn on_its_own_line(comment: &AttachedComment) -> Document {
    concat(vec![
        Document::FreshLine,
        body(comment),
        Document::FreshLine,
    ])
}

/// A comment at the end of the current line, not counted toward the width;
/// whatever follows starts a new line. `before_a_line_break`: the
/// container writes that line break right after it.
fn ending_the_line(comment: &AttachedComment, before_a_line_break: bool) -> Document {
    let mut parts = match comment.lines.as_slice() {
        [line] => {
            let marker = match comment.kind {
                CommentKind::Line => "///",
                CommentKind::Block => "",
            };
            vec![
                Document::LineSuffix(format!(" {marker}{line}")),
                Document::BreakParent,
            ]
        }
        // A block comment over several lines cannot wait for the line
        // break: it would be written after it.
        _ => vec![text(" "), body(comment)],
    };
    if !before_a_line_break {
        parts.push(Document::FreshLine);
    }
    concat(parts)
}

/// The comment's text: a `///` block re-indented line by line, its text
/// after `///` kept; a block comment's continuation lines moved with its
/// first.
fn body(comment: &AttachedComment) -> Document {
    match comment.kind {
        CommentKind::Line => {
            let mut parts = Vec::new();
            for (index, line) in comment.lines.iter().enumerate() {
                if index > 0 {
                    parts.push(hard_line());
                }
                parts.push(text(format!("///{line}")));
            }
            concat(parts)
        }
        CommentKind::Block => match comment.lines.as_slice() {
            [line] => text(line),
            lines => Document::Shifted {
                lines: lines.to_vec(),
                column: comment.column,
            },
        },
    }
}
