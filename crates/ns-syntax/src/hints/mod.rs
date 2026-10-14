//! Hints: what the editor points at that a fix can change without changing
//! what the program does -- a slot whose first value a statement assigns
//! (`initialize.rs`), and a slot nothing assigns again (`read_only.rs`).
//! Each hint carries its fix as text edits; `apply_all` applies every hint
//! of a source. Syntactic on purpose: everything a rule needs is in the
//! tree, and the behaviour oracle in `ns-cli`'s tests uses it without the
//! language server. See `docs/design/editor-support.md`, "Hints".

mod edits;
mod initialize;
mod names;
mod read_only;
mod walk;

pub use edits::{apply, Edit};

use crate::ast::{ClassDeclaration, Module};
use crate::span::Span;
use walk::{Scope, Section, SectionKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintKind {
    /// A bare slot whose first value a statement assigns can take it in
    /// its declaration.
    InitializeInDeclaration,
    /// A slot declared with `::=` that nothing assigns again can be
    /// declared with `=`.
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub kind: HintKind,
    /// The slot's name.
    pub name: String,
    /// The slot's name in the source, where the editor shows the hint.
    pub span: Span,
    /// The code body or class header the slot is declared in. Two hints of
    /// one section are never applied in the same round of `apply_all`.
    pub section: Span,
    /// The fix, valid on its own against the source the hint was found in.
    pub edits: Vec<Edit>,
}

impl Hint {
    /// The diagnostic's code.
    pub fn code(&self) -> &'static str {
        match self.kind {
            HintKind::InitializeInDeclaration => "initialize-in-declaration",
            HintKind::ReadOnly => "read-only",
        }
    }

    /// What the editor says at the slot's name.
    pub fn message(&self) -> String {
        match self.kind {
            HintKind::InitializeInDeclaration => {
                format!("`{}` can be initialized in its declaration", self.name)
            }
            HintKind::ReadOnly => {
                format!(
                    "`{}` is never assigned again and can be read-only",
                    self.name
                )
            }
        }
    }

    /// The quick fix's title.
    pub fn title(&self) -> String {
        match self.kind {
            HintKind::InitializeInDeclaration => {
                format!("Initialize `{}` in its declaration", self.name)
            }
            HintKind::ReadOnly => format!("Make `{}` read-only", self.name),
        }
    }
}

struct Collector<'text> {
    text: &'text str,
    hints: Vec<Hint>,
}

impl<'tree> walk::Visitor<'tree> for Collector<'_> {
    fn class(&mut self, _declaration: &'tree ClassDeclaration) {}

    fn section(&mut self, section: &Section<'tree>, scope: &Scope<'tree>) {
        initialize::hints(section, scope, self.text, &mut self.hints);
        if section.kind == SectionKind::Body {
            read_only::local_hints(section, self.text, &mut self.hints);
        }
    }
}

/// Every hint of `module`, which `text` parsed to without error, ordered
/// by where the slot's name stands.
pub fn hints(module: &Module, text: &str) -> Vec<Hint> {
    let mut collector = Collector {
        text,
        hints: Vec::new(),
    };
    walk::walk(module, &mut collector);
    collector
        .hints
        .sort_by_key(|hint| (hint.span.start, hint.kind == HintKind::ReadOnly));
    collector.hints
}

/// Every hint of `text`; none when it does not parse.
pub fn hints_of(text: &str) -> Vec<Hint> {
    match crate::parse(text) {
        Ok(module) => hints(&module, text),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    /// The names of `text`'s hints of `kind`, in order.
    pub(super) fn names_of(text: &str, kind: HintKind) -> Vec<String> {
        hints_of(text)
            .into_iter()
            .filter(|hint| hint.kind == kind)
            .map(|hint| hint.name)
            .collect()
    }

    /// `text` with the one hint of `kind` named `name` applied.
    pub(super) fn fixed(text: &str, kind: HintKind, name: &str) -> String {
        let hint = hints_of(text)
            .into_iter()
            .find(|hint| hint.kind == kind && hint.name == name)
            .unwrap_or_else(|| panic!("no hint for `{name}`"));
        let result = apply(text, &hint.edits);
        assert!(crate::parse(&result).is_ok(), "the fix parses:\n{result}");
        result
    }

    #[test]
    fn a_local_never_assigned_again_can_be_read_only() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count ::= 1.
                  total ::= 0.
                |
                total:: total + count.
                ^total
              )
            )
        "};
        assert_eq!(names_of(text, HintKind::ReadOnly), ["count"]);
        assert_eq!(
            fixed(text, HintKind::ReadOnly, "count"),
            text.replace("count ::= 1.", "count = 1.")
        );
    }

    /// The test is by name: a closure's own `count::` keeps the outer
    /// `count` from the hint too.
    #[test]
    fn an_assignment_of_the_name_in_a_closure_keeps_a_local_mutable() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count ::= 1.
                |
                ^[
                  |
                    count ::= 2.
                  |
                  count:: 3.
                  count
                ] value + count
              )
            )
        "};
        assert!(names_of(text, HintKind::ReadOnly).is_empty());
    }

    #[test]
    fn a_bare_local_and_a_read_only_one_get_no_hint_b() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                  limit = 3.
                |
                ^limit
              )
            )
        "};
        assert!(names_of(text, HintKind::ReadOnly).is_empty());
    }

    #[test]
    fn a_source_that_does_not_parse_has_no_hint() {
        assert!(hints_of("class Module = () ( public run = ( | count ::= 1. | ").is_empty());
        assert!(hints_of("").is_empty());
    }

    #[test]
    fn edits_apply_in_order_whatever_order_they_come_in() {
        let edits = [
            Edit {
                span: Span::new(4, 5),
                text: "X".to_string(),
            },
            Edit {
                span: Span::new(0, 1),
                text: "Y".to_string(),
            },
        ];
        assert_eq!(apply("abcdef", &edits), "YbcdXf");
    }
}
