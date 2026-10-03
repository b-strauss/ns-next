//! The fold ranges of one document: what an editor can collapse. They come
//! from the AST, so they name what they fold -- a class body, a class
//! side, a method body, a closure, a slot section -- and from the comment
//! spans beside it. See `docs/design/editor-support.md`, "Folding".

use lsp_types::{FoldingRange, FoldingRangeKind};
use ns_syntax::ast::{self, Delimiters};
use ns_syntax::Span;

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
pub fn folds(module: &ast::Module, comments: &[Span], text: &str) -> Vec<Fold> {
    let mut collector = Collector {
        text,
        folds: Vec::new(),
    };
    collector.class(&module.root_class);
    for comment in comments {
        collector.push(*comment, comment_preview(comment.text(text)), true);
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

/// `(* ` and a comment's first non-blank line, cut to
/// `COMMENT_PREVIEW_CHARACTERS`, then ` …*)`.
fn comment_preview(comment: &str) -> String {
    let inner = comment.strip_prefix("(*").unwrap_or(comment);
    let inner = inner.strip_suffix("*)").unwrap_or(inner);
    match inner.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some(line) => {
            let preview: String = line.chars().take(COMMENT_PREVIEW_CHARACTERS).collect();
            format!("(* {preview} …*)")
        }
        None => "(*…*)".to_string(),
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
        self.bracketed(instance.delimiters, instance.span.end, "(…)");
        self.slot_section(instance.slot_delimiters, &instance.slots);
        for slot in &instance.slots {
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
        for method in &instance.methods {
            self.method(method);
        }
        for nested in &instance.nested_classes {
            self.class(nested);
        }
        if let Some(class_side) = &class.class_side {
            self.bracketed(class_side.delimiters, class_side.span.end, "(…)");
            for method in &class_side.methods {
                self.method(method);
            }
        }
    }

    fn slot_section(&mut self, delimiters: Option<Delimiters>, slots: &[ast::SlotDeclaration]) {
        let end = delimiters.map_or(0, |delimiters| {
            slots
                .last()
                .map_or(delimiters.open.end, |slot| slot.span.end)
        });
        self.bracketed(delimiters, end, "|…|");
    }

    fn slot(&mut self, slot: &ast::SlotDeclaration) {
        if let Some(initializer) = &slot.initializer {
            self.expression(initializer);
        }
    }

    fn method(&mut self, method: &ast::MethodDeclaration) {
        self.bracketed(method.body_delimiters, method.span.end, "(…)");
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
        match statement {
            ast::Statement::Expression(expression) | ast::Statement::Return(expression) => {
                self.expression(expression);
            }
        }
    }

    fn expression(&mut self, expression: &ast::Expression) {
        match expression {
            ast::Expression::Identifier(_) | ast::Expression::Literal(_) => {}
            ast::Expression::Send(send) => {
                self.expression(&send.receiver);
                send.message
                    .arguments
                    .iter()
                    .for_each(|argument| self.expression(argument));
            }
            ast::Expression::Cascade(cascade) => {
                self.expression(&cascade.receiver);
                for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                    message
                        .arguments
                        .iter()
                        .for_each(|argument| self.expression(argument));
                }
            }
            ast::Expression::Assign { value, .. } => self.expression(value),
            ast::Expression::Block(block) => {
                let collapsed = if block.parameters.is_empty() {
                    "[…]".to_string()
                } else {
                    let parameters: Vec<String> = block
                        .parameters
                        .iter()
                        .map(|parameter| format!(":{}", parameter.name))
                        .collect();
                    format!("[{} | …]", parameters.join(" "))
                };
                self.bracketed(block.delimiters, block.span.end, &collapsed);
                self.body(&block.body);
            }
            ast::Expression::DynamicArray { elements, .. } => {
                elements.iter().for_each(|element| self.expression(element));
            }
        }
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn a_class_body_and_a_class_side_fold() {
        let text = indoc! {"
            class Module = (
                | first |
            ) (
                run = ( ^first )
            ) : (
                make = ( ^self new )
            )
        "};
        assert_eq!(lines_of(text), [fold(0, 4, "(…)"), fold(4, 6, "(…)")]);
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
                fold(0, 7, "(…)"),
                fold(1, 6, "(…)"),
                fold(2, 3, "|…|"),
                fold(4, 5, "[:each | …]"),
            ]
        );
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
                (0, 1, "(* A module …*)".to_string(), true),
                fold(2, 5, "(…)"),
            ]
        );
    }

    #[test]
    fn an_unterminated_comment_folds_to_the_end() {
        let text = "class Module = () ()\n(* open\nstill open";
        let folds = lines_of(text);
        assert_eq!(folds, [(1, 2, "(* open …*)".to_string(), true)]);
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
                fold(0, 6, "(…)"),
                fold(1, 2, "(…)"),
                fold(3, 5, "(…)"),
                fold(6, 10, "(…)"),
                fold(7, 9, "(…)"),
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
        assert_eq!(method.collapsed_text.as_deref(), Some("(…)"));
    }
}
