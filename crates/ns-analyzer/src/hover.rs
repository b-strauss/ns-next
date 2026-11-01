//! What the editor shows over a name: the declaration it names, as a
//! signature in a `newspeak` code block, and its doc comment, as Markdown.
//! The classifier (`semantic_tokens.rs`) decides which declaration each name
//! names, the same decision that colours it, and records each declaration's
//! parts here; this module renders them. See
//! `docs/design/editor-support.md`, "Hover".

use std::collections::HashMap;

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Range};
use ns_syntax::ast::{self, Anchor, Placement};
use ns_syntax::{Comment, CommentKind, Span};

use crate::documents::LineIndex;

/// A name in the source and the declaration it names, by the span of the
/// declaration's node (`Declared` is keyed by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reference {
    pub span: Span,
    pub declaration: Span,
}

/// The parts of one declaration a hover shows, as spans of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declared {
    pub kind: DeclaredKind,
    /// The source the signature is read from, whitespace runs shown as one
    /// space: a class's header up to its `(`, a method's pattern up to its
    /// `=`, a slot's or a local's declaration, a parameter with its type.
    pub signature: Span,
    /// A slot's initializer, which a signature too long to read leaves out.
    pub initializer: Option<Span>,
    /// The doc comment above the declaration (`doc_comment`).
    pub documentation: Option<Span>,
    /// The class whose member it is.
    pub container: Option<Container>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredKind {
    Class,
    Method,
    Slot,
    Parameter,
    Local,
}

/// The class a member is declared in, by its name, and which side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Container {
    pub name: Span,
    pub class_side: bool,
}

/// The longest signature shown whole, in characters; past it a slot's
/// initializer is left out.
const SIGNATURE_CHARACTERS: usize = 80;

/// The doc comment of a declaration whose node carries `comments`: the
/// block of two or more `///` lines on the lines right above it, as the
/// JetBrains plugin colours and renders one (`NewspeakDocComments`). Only
/// its candidate here; `hover` checks it ends on the line before the
/// declaration, which the attachment does not promise (`Before` takes in
/// a comment a blank line away too).
pub fn doc_comment(comments: &[ast::AttachedComment]) -> Option<Span> {
    comments
        .iter()
        .rev()
        .find(|comment| comment.anchor == Anchor::Before)
        .filter(|comment| {
            comment.kind == CommentKind::Line
                && comment.placement == Placement::OwnLine
                && comment.lines.len() >= 2
        })
        .map(|comment| comment.span)
}

/// The hover over the name at `offset`, if that name is one the classifier
/// placed at a declaration of this document.
pub fn hover(
    references: &[Reference],
    declarations: &HashMap<Span, Declared>,
    offset: u32,
    text: &str,
    line_index: &LineIndex,
) -> Option<Hover> {
    let reference = references
        .iter()
        .find(|reference| reference.span.start <= offset && offset < reference.span.end)?;
    let declared = declarations.get(&reference.declaration)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown(declared, text),
        }),
        range: Some(Range::new(
            line_index.position(text, reference.span.start),
            line_index.position(text, reference.span.end),
        )),
    })
}

/// The signature in a `newspeak` code block -- which an editor shows as the
/// definition, highlighted as Newspeak -- then the doc comment, then the
/// class that declares it.
fn markdown(declared: &Declared, text: &str) -> String {
    let mut markdown = format!("```newspeak\n{}\n```", signature(declared, text));
    let documentation = declared
        .documentation
        .filter(|comment| directly_above(text, *comment, declared.signature.start))
        .map(|span| {
            Comment {
                kind: CommentKind::Line,
                span,
            }
            .text(text)
        });
    if let Some(documentation) = documentation {
        markdown.push_str("\n\n");
        markdown.push_str(&documentation);
    }
    if let Some(container) = declared.container {
        let name = slice(text, container.name);
        let side = if container.class_side {
            "'s class side"
        } else {
            ""
        };
        markdown.push_str(&format!("\n\n*In* `{name}`{side}"));
    }
    markdown
}

fn signature(declared: &Declared, text: &str) -> String {
    let whole = collapse(slice(text, declared.signature));
    let whole = match declared.kind {
        // The pattern without the `=` that opens the body.
        DeclaredKind::Method => whole.trim_end_matches('=').trim_end().to_string(),
        // The declaration without the `.` that ends it.
        DeclaredKind::Slot | DeclaredKind::Local => whole.trim_end_matches('.').to_string(),
        DeclaredKind::Class | DeclaredKind::Parameter => whole,
    };
    match declared.initializer {
        Some(initializer) if whole.chars().count() > SIGNATURE_CHARACTERS => {
            let head = Span::new(declared.signature.start, initializer.start);
            format!("{} ...", collapse(slice(text, head)))
        }
        _ => whole,
    }
}

/// Whether only whitespace with one line break stands between the comment
/// and the declaration: the comment is on the lines right above it.
fn directly_above(text: &str, comment: Span, declaration: u32) -> bool {
    text.get(comment.end as usize..declaration as usize)
        .is_some_and(|gap| {
            gap.chars().all(char::is_whitespace)
                && gap.matches('\n').count() + gap.matches('\r').count()
                    - gap.matches("\r\n").count()
                    == 1
        })
}

/// The source of `span`, or nothing for a span a recovered tree got wrong.
fn slice(text: &str, span: Span) -> &str {
    text.get(span.start as usize..span.end as usize)
        .unwrap_or_default()
}

/// `text` on one line: each run of whitespace one space.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::cast_possible_truncation,
        reason = "a test's values are small literals and lengths it built"
    )]

    use super::*;
    use indoc::indoc;

    const MODULE: &str = indoc! {"
        /// A module of shapes.
        /// It grows.
        class Shapes usingPlatform: platform = Object new (
          | private count ::= 0. private limit = 10. private table = Array new: 1000 withAll: 'a value long enough to leave out of the signature'. |
        ) (
          /// A shape inside.
          /// Nested.
          class Inner = Object new () ()
          /// Grows by `amount`,
          /// *never* shrinks.
          public grow: amount <Integer> by: step = (
            | total ::= 0. |
            total:: amount + step.
            count:: count + total.
            ^limit
          )
          /// A remark on one line.
          public make = ( ^Inner new )

          /// Separated by a blank line.

          public again = ( ^grow: 3 by: 1 )
          public other = ( ^self printString size + table size )
        ) : (
          /// Makes one.
          /// Fresh.
          public fresh = ( ^usingPlatform: nil )
        )
    "};

    /// The hover's Markdown over the `occurrence`-th (0-based) `name` in `MODULE`.
    fn hover_at(name: &str, occurrence: usize) -> Option<String> {
        let start = MODULE
            .match_indices(name)
            .nth(occurrence)
            .unwrap_or_else(|| panic!("`{name}` #{occurrence} is not in the module"))
            .0 as u32;
        let analysis = crate::analysis::analyze(MODULE);
        assert!(analysis.findings.is_empty(), "the module is clean");
        let line_index = LineIndex::new(MODULE);
        hover(
            &analysis.references,
            &analysis.declarations,
            start + 1,
            MODULE,
            &line_index,
        )
        .map(|hover| match hover.contents {
            HoverContents::Markup(markup) => {
                assert_eq!(markup.kind, MarkupKind::Markdown);
                markup.value
            }
            _ => panic!("a hover is Markdown"),
        })
    }

    #[test]
    fn a_method_shows_its_pattern_its_doc_comment_and_its_class() {
        let expected = "```newspeak\npublic grow: amount <Integer> by: step\n```\n\n\
                        Grows by `amount`,\n*never* shrinks.\n\n*In* `Shapes`";
        assert_eq!(hover_at("grow:", 0).as_deref(), Some(expected));
        // Every keyword of the pattern, and a send the resolver binds to it.
        assert_eq!(hover_at("by:", 0).as_deref(), Some(expected));
        assert_eq!(hover_at("grow:", 1).as_deref(), Some(expected));
        assert_eq!(hover_at("by:", 1).as_deref(), Some(expected));
    }

    #[test]
    fn a_class_shows_its_header_and_its_doc_comment() {
        let inner = "```newspeak\nclass Inner = Object new\n```\n\nA shape inside.\nNested.\n\n*In* `Shapes`";
        assert_eq!(hover_at("Inner", 0).as_deref(), Some(inner));
        assert_eq!(hover_at("Inner", 1).as_deref(), Some(inner));
        assert_eq!(
            hover_at("Shapes", 0).as_deref(),
            Some(
                "```newspeak\nclass Shapes usingPlatform: platform = Object new\n```\n\n\
                 A module of shapes.\nIt grows."
            )
        );
    }

    #[test]
    fn a_slot_shows_its_declaration_and_a_long_one_leaves_its_initializer_out() {
        let count = "```newspeak\nprivate count ::= 0\n```\n\n*In* `Shapes`";
        assert_eq!(hover_at("count", 0).as_deref(), Some(count));
        assert_eq!(hover_at("count", 1).as_deref(), Some(count), "its setter");
        assert_eq!(hover_at("count", 2).as_deref(), Some(count), "a read");
        assert_eq!(
            hover_at("table", 1).as_deref(),
            Some("```newspeak\nprivate table = ...\n```\n\n*In* `Shapes`")
        );
    }

    #[test]
    fn a_parameter_and_a_local_show_their_declarations() {
        let amount = "```newspeak\namount <Integer>\n```";
        assert_eq!(hover_at("amount", 1).as_deref(), Some(amount));
        assert_eq!(hover_at("amount", 2).as_deref(), Some(amount));
        assert_eq!(
            hover_at("total", 1).as_deref(),
            Some("```newspeak\ntotal ::= 0\n```")
        );
    }

    #[test]
    fn a_single_line_or_a_comment_a_blank_line_away_is_no_doc_comment() {
        assert_eq!(
            hover_at("make", 0).as_deref(),
            Some("```newspeak\npublic make\n```\n\n*In* `Shapes`")
        );
        assert_eq!(
            hover_at("again", 0).as_deref(),
            Some("```newspeak\npublic again\n```\n\n*In* `Shapes`")
        );
    }

    #[test]
    fn a_class_side_method_and_the_factory_say_where_they_are() {
        assert_eq!(
            hover_at("fresh", 0).as_deref(),
            Some("```newspeak\npublic fresh\n```\n\nMakes one.\nFresh.\n\n*In* `Shapes`'s class side")
        );
        // `usingPlatform:` there is the factory, a send the resolver binds to the class.
        let factory = hover_at("usingPlatform:", 1).expect("the factory");
        assert!(
            factory.starts_with("```newspeak\nclass Shapes usingPlatform: platform"),
            "{factory}"
        );
    }

    #[test]
    fn a_send_to_an_object_and_an_inherited_member_have_none() {
        assert_eq!(hover_at("printString", 0), None);
        assert_eq!(hover_at("size", 0), None);
    }
}
