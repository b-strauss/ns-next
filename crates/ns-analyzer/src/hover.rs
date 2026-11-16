//! What the editor shows over a name: the declaration it names, as a
//! signature in a `newspeak` code block, and its doc comment, as Markdown.
//! The classifier (`semantic_tokens.rs`) decides which declaration each name
//! names, the same decision that colours it, and records each declaration's
//! parts here; this module renders them. See
//! `docs/design/editor-support.md`, "Hover".

use std::collections::HashMap;

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Range};
use ns_syntax::ast::{self, Anchor, Placement};
use ns_syntax::doc_comment::{DocComment, DocTag, DocTagKind};
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
    /// `=`, a factory's pattern (empty for the default `new`), a slot's or
    /// a local's declaration, a parameter with its type.
    pub signature: Span,
    /// A slot's initializer, which a signature too long to read leaves out.
    pub initializer: Option<Span>,
    /// The doc comment above the declaration (`doc_comment`).
    pub documentation: Option<Span>,
    /// The class whose member it is.
    pub container: Option<Container>,
    /// A factory's class, by its declaration's node: the factory is
    /// documented in the class's doc comment, by `@factory` and `@param`.
    pub class: Option<Span>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredKind {
    Class,
    /// A class's primary factory, a member of its class side.
    Factory,
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
            value: markdown(declared, declarations, text),
        }),
        range: Some(Range::new(
            line_index.position(text, reference.span.start),
            line_index.position(text, reference.span.end),
        )),
    })
}

/// The signature in a `newspeak` code block -- which an editor shows as the
/// definition, highlighted as Newspeak -- then the doc comment's description
/// and its tags as the platform's section table, then the class that
/// declares it.
fn markdown(declared: &Declared, declarations: &HashMap<Span, Declared>, text: &str) -> String {
    let mut markdown = format!("```newspeak\n{}\n```", signature(declared, text));
    let (description, rows) = match declared.kind {
        DeclaredKind::Factory => factory_documentation(declared, declarations, text),
        _ => documentation(declared, text).map_or_else(
            || (String::new(), Vec::new()),
            |documentation| {
                let rows = tag_rows(&documentation.tags);
                (documentation.description, rows)
            },
        ),
    };
    if !description.is_empty() {
        markdown.push_str("\n\n");
        markdown.push_str(&description);
    }
    if !rows.is_empty() {
        markdown.push_str("\n\n");
        markdown.push_str(&section_table(&rows));
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

/// The doc comment on the lines right above `declared`, read.
fn documentation(declared: &Declared, text: &str) -> Option<DocComment> {
    declared
        .documentation
        .filter(|comment| directly_above(text, *comment, declared.signature.start))
        .map(|span| {
            DocComment::parse(
                Comment {
                    kind: CommentKind::Line,
                    span,
                },
                text,
            )
        })
}

/// One row of the section table: its label and its items.
type SectionRow = (&'static str, Vec<String>);

/// A factory has no comment of its own: its class's says what it does, by
/// `@factory`, and what its parameters are, by `@param`. What it answers
/// needs no saying, so that row stands with a comment or without one.
fn factory_documentation(
    factory: &Declared,
    declarations: &HashMap<Span, Declared>,
    text: &str,
) -> (String, Vec<SectionRow>) {
    let class_documentation = factory
        .class
        .and_then(|class| declarations.get(&class))
        .and_then(|class| documentation(class, text))
        .unwrap_or_default();
    let description = class_documentation
        .tags
        .iter()
        .filter(|tag| tag.kind == DocTagKind::Factory)
        .map(|tag| tag.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut rows = tag_rows(&class_documentation.tags);
    rows.retain(|(label, _)| *label == PARAMS);
    if let Some(container) = factory.container {
        let name = slice(text, container.name);
        rows.push((ANSWERS, vec![format!("a new instance of `{name}`.")]));
    }
    (description, rows)
}

const FACTORY: &str = "Factory:";
const PARAMS: &str = "Params:";
const ANSWERS: &str = "Answers:";
const SIGNALS: &str = "Signals:";
const SEE_ALSO: &str = "See Also:";

/// The sections of `tags` in one order, whatever the comment's, each
/// with its items in the comment's order; a section without a tag is left
/// out. An unknown tag is no section: the description shows it.
fn tag_rows(tags: &[DocTag]) -> Vec<SectionRow> {
    let items = |kind: DocTagKind| -> Vec<String> {
        tags.iter()
            .filter(|tag| tag.kind == kind)
            .map(section_item)
            .collect()
    };
    [
        (FACTORY, items(DocTagKind::Factory)),
        (PARAMS, items(DocTagKind::Param)),
        (ANSWERS, items(DocTagKind::Answers)),
        (SIGNALS, items(DocTagKind::Signals)),
        (SEE_ALSO, items(DocTagKind::See)),
    ]
    .into_iter()
    .filter(|(_, items)| !items.is_empty())
    .collect()
}

/// A tag as an item of its section: a parameter's or an error's name in
/// code, a dash, then the text.
fn section_item(tag: &DocTag) -> String {
    match &tag.argument {
        Some((name, _)) if tag.text.is_empty() => format!("`{name}`"),
        Some((name, _)) => format!("`{name}` \u{2013} {}", tag.text),
        None => tag.text.clone(),
    }
}

/// The platform's section table (`DocumentationMarkup`'s `sections`), which
/// a JetBrains IDE lays out as it does its own documentation. Each cell's
/// text stands between blank lines, which end the HTML block, so the
/// platform's converter renders it as Markdown and an item's code reads as
/// the description's does (pinned by the plugin's `NewspeakHoverSectionsTest`).
fn section_table(rows: &[SectionRow]) -> String {
    let mut table = String::from("<table class='sections'>\n");
    for (label, items) in rows {
        table.push_str("<tr><td valign='top' class='section'><p>");
        table.push_str(label);
        table.push_str("</td><td valign='top'>\n\n");
        for item in items {
            table.push_str(item);
            table.push_str("\n\n");
        }
        table.push_str("</td></tr>\n");
    }
    table.push_str("</table>");
    table
}

fn signature(declared: &Declared, text: &str) -> String {
    let whole = collapse(slice(text, declared.signature));
    let whole = match declared.kind {
        // The pattern without the `=` that opens the body.
        DeclaredKind::Method => whole.trim_end_matches('=').trim_end().to_string(),
        // The declaration without the `.` that ends it.
        DeclaredKind::Slot | DeclaredKind::Local => whole.trim_end_matches('.').to_string(),
        // The default factory has no pattern in the source.
        DeclaredKind::Factory if whole.is_empty() => "new".to_string(),
        DeclaredKind::Class | DeclaredKind::Factory | DeclaredKind::Parameter => whole,
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
pub(crate) fn directly_above(text: &str, comment: Span, declaration: u32) -> bool {
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
        /// @factory Makes the module from `platform`.
        /// @param platform the platform it runs on.
        class Shapes usingPlatform: platform = Object new (
          | private count ::= 0. private limit = 10. private table = Array new: 1000 withAll: 'a value long enough to leave out of the signature'. |
        ) (
          /// A shape inside.
          /// Nested.
          class Inner = Object new () ()
          /// A point.
          /// @factory Makes the origin.
          class Point = Object new () () : (
            public origin = ( ^new )
          )
          /// {@summary Grows by `amount`.} Adds `step` too,
          /// *never* shrinks.
          /// @see `make`
          /// @signals ArgumentError when `amount` is no number.
          /// @answers the limit.
          /// @param amount how much to grow.
          /// @param step how much more.
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

    /// One row of the section table, its items as the hover writes them.
    fn row(label: &str, items: &[&str]) -> String {
        let mut row =
            format!("<tr><td valign='top' class='section'><p>{label}</td><td valign='top'>\n\n");
        for item in items {
            row.push_str(item);
            row.push_str("\n\n");
        }
        row.push_str("</td></tr>\n");
        row
    }

    fn table(rows: &[String]) -> String {
        format!("<table class='sections'>\n{}</table>", rows.concat())
    }

    #[test]
    fn the_tags_become_sections_after_the_description() {
        let expected = format!(
            "```newspeak\npublic grow: amount <Integer> by: step\n```\n\n\
             Grows by `amount`. Adds `step` too,\n*never* shrinks.\n\n{}\n\n*In* `Shapes`",
            table(&[
                row(
                    "Params:",
                    &["`amount` – how much to grow.", "`step` – how much more."]
                ),
                row("Answers:", &["the limit."]),
                row(
                    "Signals:",
                    &["`ArgumentError` – when `amount` is no number."]
                ),
                row("See Also:", &["`make`"]),
            ])
        );
        assert_eq!(hover_at("grow:", 0).as_deref(), Some(expected.as_str()));
        // Every keyword of the pattern, and a send the resolver binds to it.
        assert_eq!(hover_at("by:", 0).as_deref(), Some(expected.as_str()));
        assert_eq!(hover_at("grow:", 1).as_deref(), Some(expected.as_str()));
        assert_eq!(hover_at("by:", 1).as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn a_comment_without_tags_has_no_table() {
        let inner = "```newspeak\nclass Inner = Object new\n```\n\nA shape inside.\nNested.\n\n*In* `Shapes`";
        assert_eq!(hover_at("Inner", 0).as_deref(), Some(inner));
        assert_eq!(hover_at("Inner", 1).as_deref(), Some(inner));
    }

    #[test]
    fn the_inline_summary_reads_as_its_text() {
        let grow = hover_at("grow:", 0).expect("the method");
        assert!(
            grow.contains("\n\nGrows by `amount`. Adds `step` too,"),
            "{grow}"
        );
        assert!(!grow.contains("@summary"), "{grow}");
    }

    #[test]
    fn the_factory_shows_its_signature_its_text_its_params_and_answers() {
        let expected = format!(
            "```newspeak\nusingPlatform: platform\n```\n\n\
             Makes the module from `platform`.\n\n{}\n\n*In* `Shapes`'s class side",
            table(&[
                row("Params:", &["`platform` – the platform it runs on."]),
                row("Answers:", &["a new instance of `Shapes`."]),
            ])
        );
        // In the header, and a class-side send the resolver binds to it.
        assert_eq!(
            hover_at("usingPlatform:", 0).as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            hover_at("usingPlatform:", 1).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn the_default_factory_is_new() {
        let expected = format!(
            "```newspeak\nnew\n```\n\nMakes the origin.\n\n{}\n\n*In* `Point`'s class side",
            table(&[row("Answers:", &["a new instance of `Point`."])])
        );
        assert_eq!(hover_at("^new", 0).as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn the_class_shows_its_factory_section() {
        let expected = format!(
            "```newspeak\nclass Shapes usingPlatform: platform = Object new\n```\n\n\
             A module of shapes.\nIt grows.\n\n{}",
            table(&[
                row("Factory:", &["Makes the module from `platform`."]),
                row("Params:", &["`platform` – the platform it runs on."]),
            ])
        );
        assert_eq!(hover_at("Shapes", 0).as_deref(), Some(expected.as_str()));
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
        assert_eq!(hover_at("amount", 3).as_deref(), Some(amount));
        assert_eq!(hover_at("amount", 4).as_deref(), Some(amount));
        assert_eq!(
            hover_at("total", 1).as_deref(),
            Some("```newspeak\ntotal ::= 0\n```")
        );
    }

    #[test]
    fn a_single_line_or_a_comment_a_blank_line_away_is_no_doc_comment() {
        assert_eq!(
            hover_at("make", 1).as_deref(),
            Some("```newspeak\npublic make\n```\n\n*In* `Shapes`")
        );
        assert_eq!(
            hover_at("again", 0).as_deref(),
            Some("```newspeak\npublic again\n```\n\n*In* `Shapes`")
        );
    }

    #[test]
    fn a_class_side_method_says_where_it_is() {
        assert_eq!(
            hover_at("fresh", 0).as_deref(),
            Some(
                "```newspeak\npublic fresh\n```\n\nMakes one.\nFresh.\n\n*In* `Shapes`'s class side"
            )
        );
    }

    #[test]
    fn a_send_to_an_object_and_an_inherited_member_have_none() {
        assert_eq!(hover_at("printString", 0), None);
        assert_eq!(hover_at("size", 0), None);
    }
}
