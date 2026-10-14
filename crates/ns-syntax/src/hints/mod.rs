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
    fn class(&mut self, declaration: &'tree ClassDeclaration) {
        read_only::header_hints(declaration, self.text, &mut self.hints);
    }

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

    /// A comment between the name and the initializer may spell `::=`:
    /// the fix rewrites the operator, never the comment.
    #[test]
    fn a_comment_spelling_the_operator_is_left_alone() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count (* was ::= *) ::= 1.
                |
                ^count
              )
            )
        "};
        assert_eq!(names_of(text, HintKind::ReadOnly), ["count"]);
        assert_eq!(
            fixed(text, HintKind::ReadOnly, "count"),
            text.replace("*) ::= 1.", "*) = 1.")
        );
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
    fn a_private_header_slot_nothing_writes_can_be_read_only() {
        let text = indoc! {"
            class Module = (
              |
                private limit ::= 10.
                private used ::= 0.
                protected shown ::= 1.
                public total ::= 2.
              |
            ) (
              public take = (
                used:: used + 1.
                ^limit - used + shown + total
              )
            )
        "};
        assert_eq!(names_of(text, HintKind::ReadOnly), ["limit"]);
        assert_eq!(
            fixed(text, HintKind::ReadOnly, "limit"),
            text.replace("private limit ::= 10.", "private limit = 10.")
        );
    }

    /// A private slot is written by a send of its setter as much as by
    /// `name::`, from a nested class as much as from the class itself.
    #[test]
    fn a_setter_send_or_a_nested_class_keeps_a_private_slot_mutable() {
        let text = indoc! {"
            class Module = (
              |
                private limit ::= 10.
                private used ::= 0.
              |
            ) (
              public reset = (
                self limit: 5.
              )
              class Inner = () (
                public clear = (
                  used:: 0.
                )
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

    #[test]
    fn the_first_assignment_of_a_bare_local_moves_into_its_declaration() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                |
                count:: 1.
                ^count
              )
            )
        "};
        assert_eq!(names_of(text, HintKind::InitializeInDeclaration), ["count"]);
        assert_eq!(
            fixed(text, HintKind::InitializeInDeclaration, "count"),
            indoc! {"
                class Module = () (
                  public run = (
                    |
                      count ::= 1.
                    |
                    ^count
                  )
                )
            "}
        );
    }

    /// Every existing initializer runs before the moved value, as it did
    /// before the statement: the local moves after the last initialized one.
    #[test]
    fn a_local_declared_before_an_initialized_one_moves_after_it() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                  total ::= 0.
                |
                count:: 1.
                ^count + total
              )
            )
        "};
        assert_eq!(
            fixed(text, HintKind::InitializeInDeclaration, "count"),
            indoc! {"
                class Module = () (
                  public run = (
                    |
                      total ::= 0.
                      count ::= 1.
                    |
                    ^count + total
                  )
                )
            "}
        );
    }

    #[test]
    fn a_header_slot_takes_the_first_initializer_expression() {
        let text = indoc! {"
            class Module = (
              |
                count
              |
              count:: 1.
            ) (
              public run = (
                ^count
              )
            )
        "};
        assert_eq!(
            fixed(text, HintKind::InitializeInDeclaration, "count"),
            indoc! {"
                class Module = (
                  |
                    count ::= 1.
                  |
                ) (
                  public run = (
                    ^count
                  )
                )
            "}
        );
    }

    /// Header slots are the class's layout, so they are never reordered.
    #[test]
    fn a_header_slot_declared_before_an_initialized_one_gets_no_hint() {
        let text = indoc! {"
            class Module = (
              |
                count
                total ::= 0.
              |
              count:: 1.
            ) ()
        "};
        assert!(names_of(text, HintKind::InitializeInDeclaration).is_empty());
    }

    #[test]
    fn a_continued_value_keeps_its_place_under_the_declaration() {
        let text = indoc! {"
            class Module = () (
              public sum: values = (
                |
                  result
                |
                result:: values
                    inject: 0
                    into: [:total :each | total + each].
                ^result
              )
            )
        "};
        // The statement stood at column 4, the declaration stands at 6: the
        // continuation lines move from column 8 to 10.
        let result = fixed(text, HintKind::InitializeInDeclaration, "result");
        assert!(
            result.contains(
                "      result ::= values\n          inject: 0\n          into: [:total :each | total + each].\n    |\n"
            ),
            "{result}"
        );
    }

    #[test]
    fn a_one_line_section_takes_the_initializer_inline() {
        let text = "class Module = () ( public run = ( | count | count:: 1. ^count ) )\n";
        assert_eq!(
            fixed(text, HintKind::InitializeInDeclaration, "count"),
            "class Module = () ( public run = ( | count ::= 1. | ^count ) )\n"
        );
    }

    #[test]
    fn carriage_returns_survive_a_fix() {
        let text = "class Module = () (\r\n  public run = (\r\n    |\r\n      count\r\n    |\r\n    count:: 1.\r\n    ^count\r\n  )\r\n)\r\n";
        assert_eq!(
            fixed(text, HintKind::InitializeInDeclaration, "count"),
            "class Module = () (\r\n  public run = (\r\n    |\r\n      count ::= 1.\r\n    |\r\n    ^count\r\n  )\r\n)\r\n"
        );
    }

    #[test]
    fn a_comment_in_or_beside_the_statement_keeps_it_where_it_is() {
        for statement in ["count:: 1. /// the first", "count:: (* one *) 1."] {
            let text = format!(
                "class Module = () (\n  public run = (\n    |\n      count\n    |\n    {statement}\n    ^count\n  )\n)\n"
            );
            assert!(
                names_of(&text, HintKind::InitializeInDeclaration).is_empty(),
                "{statement}"
            );
        }
    }

    #[test]
    fn only_the_first_statement_moves_when_its_value_is_a_send() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                  index
                |
                count:: self size.
                index:: self start.
                ^count + index
              )
            )
        "};
        assert_eq!(names_of(text, HintKind::InitializeInDeclaration), ["count"]);
    }

    #[test]
    fn a_chain_a_parenthesized_assignment_or_a_value_naming_the_slot_gets_no_hint() {
        for statement in ["count:: index:: 1.", "(count:: 1).", "count:: count."] {
            let text = format!(
                "class Module = () (\n  public run = (\n    |\n      count\n      index\n    |\n    {statement}\n    ^count\n  )\n)\n"
            );
            assert!(
                !names_of(&text, HintKind::InitializeInDeclaration).contains(&"count".to_string()),
                "{statement}"
            );
        }
    }
}
