//! The fold ranges of one document: what an editor can collapse. They come
//! from the AST, so they name what they fold -- a class, its instance
//! side, its class side, a method body, a closure, a slot section -- and
//! from the comments beside it. See `docs/design/editor-support.md`, "Folding".

use lsp_types::{FoldingRange, FoldingRangeKind};
use ns_syntax::ast::{self, Delimiters};
use ns_syntax::{Comment, CommentKind, Span};

use crate::documents::LineIndex;

/// One fold, in byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    pub span: Span,
    pub collapsed_text: String,
    pub comment: bool,
}

/// The longest first line a collapsed comment shows.
const COMMENT_PREVIEW_CHARACTERS: usize = 60;

/// Every fold of `module` and `comments` that spans more than one line,
/// ordered by start, an enclosing fold before those inside it.
pub fn folds(module: &ast::Module, comments: &[Comment], text: &str) -> Vec<Fold> {
    let mut collector = Collector {
        text,
        folds: Vec::new(),
    };
    collector.class(&module.root_class);
    for comment in comments {
        collector.push(comment.span, comment_preview(*comment, text), true);
    }
    collector
        .folds
        .sort_by_key(|fold| (fold.span.start, std::cmp::Reverse(fold.span.end)));
    collector.folds
}

/// The LSP form of `folds`, positions in UTF-16 units. A code fold carries
/// no kind: LSP's `region` names `#region` markers, which Newspeak has not.
pub fn encode(folds: &[Fold], text: &str, line_index: &LineIndex) -> Vec<FoldingRange> {
    folds
        .iter()
        .map(|fold| {
            let start = line_index.position(text, fold.span.start);
            let end = line_index.position(text, fold.span.end);
            FoldingRange {
                start_line: start.line,
                start_character: Some(start.character),
                end_line: end.line,
                end_character: Some(end.character),
                kind: fold.comment.then_some(FoldingRangeKind::Comment),
                collapsed_text: Some(fold.collapsed_text.clone()),
            }
        })
        .collect()
}

/// The comment's marker and its text's first non-blank line, cut to
/// `COMMENT_PREVIEW_CHARACTERS`, then ` ...`: `(* first ...*)` or
/// `/// first ...`. Every placeholder writes three ASCII dots rather than
/// `…`, which the editor would draw in a fallback font beside its own.
fn comment_preview(comment: Comment, text: &str) -> String {
    let content = comment.text(text);
    let first = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            line.chars()
                .take(COMMENT_PREVIEW_CHARACTERS)
                .collect::<String>()
        });
    match (comment.kind, first) {
        (CommentKind::Block, Some(line)) => format!("(* {line} ...*)"),
        (CommentKind::Block, None) => "(*...*)".to_string(),
        (CommentKind::Line, Some(line)) => format!("/// {line} ..."),
        (CommentKind::Line, None) => "///...".to_string(),
    }
}

struct Collector<'a> {
    text: &'a str,
    folds: Vec<Fold>,
}

impl Collector<'_> {
    fn push(&mut self, span: Span, collapsed_text: String, comment: bool) {
        let folded = self
            .text
            .get(span.start as usize..span.end as usize)
            .is_some_and(|folded| folded.contains(['\n', '\r']));
        if folded {
            self.folds.push(Fold {
                span,
                collapsed_text,
                comment,
            });
        }
    }

    /// From the opening token to the closing one, or, when recovery found
    /// none, to where the node ends.
    fn bracketed(&mut self, delimiters: Option<Delimiters>, node_end: u32, collapsed_text: &str) {
        if let Some(delimiters) = delimiters {
            let end = delimiters.close.map_or(node_end, |close| close.end);
            let span = Span::new(delimiters.open.start, end.max(delimiters.open.end));
            self.push(span, collapsed_text.to_string(), false);
        }
    }

    fn class(&mut self, class: &ast::ClassDeclaration) {
        let instance = &class.instance_side;
        let class_side = class
            .class_side
            .as_ref()
            .and_then(|class_side| class_side.delimiters);
        // The whole class, from the header's `(` to its last `)`.
        let whole = instance.delimiters.map(|delimiters| Delimiters {
            open: delimiters.open,
            close: class_side.map_or(delimiters.close, |class_side| class_side.close),
        });
        self.bracketed(whole, class.span.end, "(...)");
        let class_side_folds = class_side.filter(|class_side| {
            let end = class_side.close.map_or(class.span.end, |close| close.end);
            !self.on_one_line(class_side.open.start, end)
        });
        self.members(instance, class_side_folds);
        self.slot_section(instance.slot_delimiters, &instance.header_slots);
        for slot in instance.slots() {
            self.slot(slot);
        }
        for statement in &instance.initializer_expressions {
            self.statement(statement);
        }
        for clause in class.superclass.iter().chain(&class.mixin_applications) {
            for message in &clause.messages {
                message
                    .arguments
                    .iter()
                    .for_each(|argument| self.expression(argument));
            }
        }
        for method in instance.methods() {
            self.method(method);
        }
        for nested in instance.nested_classes() {
            self.class(nested);
        }
        if let Some(class_side) = &class.class_side {
            self.bracketed(class_side.delimiters, class_side.span.end, "(...)");
            for method in class_side.methods() {
                self.method(method);
            }
        }
    }

    /// The instance side's members, `(` to `)`. An editor shows one fold
    /// marker a line, the smaller fold's, so the members have no fold after
    /// a header of one line, whose line is the whole class's; and before a
    /// class side that folds and opens on the `)`'s line they end with the
    /// line before, whose line is the class side's.
    fn members(&mut self, instance: &ast::ClassBody, class_side: Option<Delimiters>) {
        let Some(members) = instance.members_delimiters else {
            return;
        };
        let header_open = instance
            .delimiters
            .map_or(members.open, |header| header.open);
        if self.on_one_line(header_open.start, members.open.start) {
            return;
        }
        let closer_shares_its_line = members.close.zip(class_side).and_then(|(close, side)| {
            let line_start = self.text[..close.start as usize].rfind('\n')? + 1;
            let alone = self.text[line_start..close.start as usize]
                .trim()
                .is_empty();
            (alone && self.on_one_line(close.end, side.open.start)).then_some(line_start)
        });
        match closer_shares_its_line {
            Some(line_start) => {
                let end = u32::try_from(self.text[..line_start].trim_end().len()).expect(
                    "internal: folding runs over a parsed module, whose source is at most u32::MAX bytes (source_too_long)",
                );
                let span = Span::new(members.open.start, end.max(members.open.end));
                self.push(span, "(...".to_string(), false);
            }
            None => self.bracketed(Some(members), instance.span.end, "(...)"),
        }
    }

    fn on_one_line(&self, start: u32, end: u32) -> bool {
        self.text
            .get(start as usize..end as usize)
            .is_some_and(|between| !between.contains('\n'))
    }

    fn slot_section(&mut self, delimiters: Option<Delimiters>, slots: &[ast::SlotDeclaration]) {
        let end = delimiters.map_or(0, |delimiters| {
            slots
                .last()
                .map_or(delimiters.open.end, |slot| slot.span.end)
        });
        self.bracketed(delimiters, end, "|...|");
    }

    fn slot(&mut self, slot: &ast::SlotDeclaration) {
        if let Some(initializer) = &slot.initializer {
            self.expression(initializer);
        }
    }

    fn method(&mut self, method: &ast::MethodDeclaration) {
        self.bracketed(method.body_delimiters, method.span.end, "(...)");
        self.body(&method.body);
    }

    fn body(&mut self, body: &ast::CodeBody) {
        self.slot_section(body.temporaries_delimiters, &body.temporaries);
        for temporary in &body.temporaries {
            self.slot(temporary);
        }
        for statement in &body.statements {
            self.statement(statement);
        }
    }

    fn statement(&mut self, statement: &ast::Statement) {
        match &statement.kind {
            ast::StatementKind::Expression(expression) | ast::StatementKind::Return(expression) => {
                self.expression(expression);
            }
        }
    }

    fn expression(&mut self, expression: &ast::Expression) {
        match &expression.kind {
            ast::ExpressionKind::Identifier(_) | ast::ExpressionKind::Literal(_) => {}
            // A chain of messages is walked link by link: it nests as deep
            // as the source is long.
            ast::ExpressionKind::Send(send) => {
                let (links, receiver) = send.receiver_chain();
                self.expression(receiver);
                for link in links.iter().rev() {
                    link.message
                        .arguments
                        .iter()
                        .for_each(|argument| self.expression(argument));
                }
            }
            ast::ExpressionKind::Cascade(cascade) => {
                self.expression(&cascade.receiver);
                for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                    message
                        .arguments
                        .iter()
                        .for_each(|argument| self.expression(argument));
                }
            }
            ast::ExpressionKind::Assign { value, .. } => self.expression(value),
            ast::ExpressionKind::Block(block) => {
                let collapsed = if block.parameters.is_empty() {
                    "[...]".to_string()
                } else {
                    let parameters: Vec<String> = block
                        .parameters
                        .iter()
                        .map(|parameter| format!(":{}", parameter.name.name))
                        .collect();
                    format!("[{} | ...]", parameters.join(" "))
                };
                self.bracketed(block.delimiters, block.span.end, &collapsed);
                self.body(&block.body);
            }
            ast::ExpressionKind::DynamicArray { elements, .. } => {
                elements.iter().for_each(|element| self.expression(element));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::cast_possible_truncation,
        reason = "a test's values are small literals and lengths it built"
    )]

    use super::*;
    use indoc::indoc;

    /// Each fold as its first and last line, its collapsed text and whether
    /// it is a comment.
    fn lines_of(text: &str) -> Vec<(usize, usize, String, bool)> {
        let parsed = ns_syntax::parse_for_editor(text);
        folds(&parsed.module, &parsed.comments, text)
            .into_iter()
            .map(|fold| {
                let line_of = |offset: u32| text[..offset as usize].matches('\n').count();
                (
                    line_of(fold.span.start),
                    line_of(fold.span.end),
                    fold.collapsed_text,
                    fold.comment,
                )
            })
            .collect()
    }

    fn fold(first: usize, last: usize, collapsed: &str) -> (usize, usize, String, bool) {
        (first, last, collapsed.to_string(), false)
    }

    /// The members end on the line before `) : (`, so that line is the
    /// class side's alone: an editor shows one fold marker a line.
    #[test]
    fn a_class_its_instance_side_and_its_class_side_fold() {
        let text = indoc! {"
            class Module = (
                | first |
            ) (
                run = ( ^first )
            ) : (
                make = ( ^self new )
            )
        "};
        assert_eq!(
            lines_of(text),
            [fold(0, 6, "(...)"), fold(2, 3, "(..."), fold(4, 6, "(...)")]
        );
    }

    #[test]
    fn the_members_after_a_header_of_several_lines_fold_to_their_closer() {
        let text = indoc! {"
            class Module = (
                | first |
            ) (
                run = ( ^first )
            )
        "};
        assert_eq!(lines_of(text), [fold(0, 4, "(...)"), fold(2, 4, "(...)")]);
    }

    /// A header of one line leaves its line to the whole class, which
    /// takes the editor's one marker there.
    #[test]
    fn the_members_after_a_header_of_one_line_do_not_fold() {
        let without = indoc! {"
            class Module = () (
                run = ( ^1 )
            )
        "};
        assert_eq!(lines_of(without), [fold(0, 2, "(...)")]);
        let one_line = indoc! {"
            class Module = () (
                run = ( ^1 )
            ) : ( make = ( ^self new ) )
        "};
        assert_eq!(lines_of(one_line), [fold(0, 2, "(...)")]);
        let with = indoc! {"
            class Module = () (
                run = ( ^1 )
            ) : (
                make = ( ^self new )
            )
        "};
        assert_eq!(lines_of(with), [fold(0, 4, "(...)"), fold(2, 4, "(...)")]);
    }

    /// A member that ends on the `)`'s line would be cut by a fold that
    /// stops on the line before.
    #[test]
    fn members_closed_after_a_member_on_its_line_fold_to_their_closer() {
        let text = indoc! {"
            class Module = (
                | first |
            ) (
                run = ( ^1 ) ) : (
                make = ( ^self new )
            )
        "};
        assert_eq!(
            lines_of(text),
            [
                fold(0, 5, "(...)"),
                fold(2, 3, "(...)"),
                fold(3, 5, "(...)")
            ]
        );
    }

    #[test]
    fn a_method_its_temporaries_and_a_closure_fold() {
        let text = indoc! {"
            class Module = () (
                run = (
                    | first
                      second |
                    ^[:each |
                        each]
                )
            )
        "};
        assert_eq!(
            lines_of(text),
            [
                fold(0, 7, "(...)"),
                fold(1, 6, "(...)"),
                fold(2, 3, "|...|"),
                fold(4, 5, "[:each | ...]"),
            ]
        );
    }

    #[test]
    fn a_closure_without_parameters_collapses_to_brackets() {
        let text = indoc! {"
            class Module = () (
                run = ( ^[
                    1 ] )
            )
        "};
        assert_eq!(
            lines_of(text),
            [
                fold(0, 3, "(...)"),
                fold(1, 2, "(...)"),
                fold(1, 2, "[...]")
            ]
        );
    }

    #[test]
    fn a_class_header_slot_section_folds() {
        let text = indoc! {"
            class Module = (
                | first
                  second |
            ) () : ()
        "};
        assert_eq!(lines_of(text), [fold(0, 3, "(...)"), fold(1, 2, "|...|")]);
    }

    /// Without its closer a slot section folds to its last slot and a
    /// closure to its last token, never to the method's `)` after them.
    #[test]
    fn an_unclosed_slot_section_and_closure_fold_to_their_last_token() {
        let slots = indoc! {"
            class Module = () (
                run = (
                    | first
                      second
                )
            )
        "};
        assert_eq!(
            lines_of(slots),
            [
                fold(0, 5, "(...)"),
                fold(1, 4, "(...)"),
                fold(2, 3, "|...|")
            ]
        );
        let closure = indoc! {"
            class Module = () (
                run = (
                    ^[:each |
                        each
                )
            )
        "};
        assert_eq!(
            lines_of(closure),
            [
                fold(0, 5, "(...)"),
                fold(1, 4, "(...)"),
                fold(2, 3, "[:each | ...]")
            ]
        );
    }

    #[test]
    fn a_comment_is_encoded_with_the_comment_kind() {
        let text = indoc! {"
            (* A module
               with a header. *)
            class Module = () ()
        "};
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = crate::documents::LineIndex::new(text);
        let ranges = encode(
            &folds(&parsed.module, &parsed.comments, text),
            text,
            &line_index,
        );
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].kind, Some(FoldingRangeKind::Comment));
    }

    #[test]
    fn a_comment_folds_to_its_first_line() {
        let text = indoc! {"
            (* A module
               with a header. *)
            class Module = () (
                (* one line *)
                run = ( ^1 )
            )
        "};
        assert_eq!(
            lines_of(text),
            [
                (0, 1, "(* A module ...*)".to_string(), true),
                fold(2, 5, "(...)"),
            ]
        );
    }

    #[test]
    fn a_line_comment_block_folds_as_one_comment() {
        let text = indoc! {"
            /// A module
            /// with a header.
            class Module = () (
                /// one line
                run = ( ^1 )
            )
        "};
        assert_eq!(
            lines_of(text),
            [
                (0, 1, "/// A module ...".to_string(), true),
                fold(2, 5, "(...)"),
            ]
        );
    }

    #[test]
    fn an_unterminated_comment_folds_to_the_end() {
        let text = "class Module = () ()\n(* open\nstill open";
        let folds = lines_of(text);
        assert_eq!(folds, [(1, 2, "(* open ...*)".to_string(), true)]);
    }

    #[test]
    fn single_line_constructs_do_not_fold() {
        assert!(lines_of("class Module = () ( run = ( ^[:x | x] ) ) : ()").is_empty());
        assert!(lines_of("").is_empty());
        assert!(lines_of("  \n  ").is_empty());
    }

    #[test]
    fn the_folds_after_a_half_typed_method_survive() {
        let text = indoc! {"
            class Module = () (
                broken = (
                    ^1
                second = (
                    ^2
                )
            ) : (
                make = (
                    ^self new
                )
            )
        "};
        assert_eq!(
            lines_of(text),
            [
                fold(0, 10, "(...)"),
                fold(1, 2, "(...)"),
                fold(3, 5, "(...)"),
                fold(6, 10, "(...)"),
                fold(7, 9, "(...)"),
            ]
        );
    }

    /// Positions are UTF-16 units on lines split at `\r\n` as at `\n`.
    #[test]
    fn a_fold_is_encoded_in_utf16_on_crlf_lines() {
        let text =
            "class Module = () (\r\n    (* Grüße 😀 *) run = (\r\n        ^1\r\n    )\r\n)\r\n";
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = crate::documents::LineIndex::new(text);
        let ranges = encode(
            &folds(&parsed.module, &parsed.comments, text),
            text,
            &line_index,
        );
        let method = &ranges[1];
        let open = text.find("= (\r\n        ^1").expect("the body") + 2;
        let line_start = text.find("    (* Grüße").expect("line 1");
        let character = text[line_start..open].encode_utf16().count() as u32;
        assert_eq!(
            (method.start_line, method.start_character),
            (1, Some(character))
        );
        assert_eq!((method.end_line, method.end_character), (3, Some(5)));
        assert_eq!(method.kind, None);
        assert_eq!(method.collapsed_text.as_deref(), Some("(...)"));
    }
}
