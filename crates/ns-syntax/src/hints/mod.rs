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

use crate::ast::Module;
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
    fn section(&mut self, section: &Section<'tree>, scope: &Scope<'tree>) {
        initialize::hints(section, scope, self.text, &mut self.hints);
        if section.kind != SectionKind::Header {
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

    /// A public or protected slot's setter may be sent from anywhere, and a
    /// private slot's fix could lift a value object's refusal of a mutable
    /// slot: a header slot never gets hint B.
    #[test]
    fn a_header_slot_gets_no_hint_b() {
        let text = indoc! {"
            class Module = (
              |
                private limit ::= 10.
                protected shown ::= 1.
                public total ::= 2.
              |
            ) (
              public take = (
                ^limit + shown + total
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
                private count
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
                    private count ::= 1.
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
                private count
                private total ::= 0.
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

    /// In a header `count:: 1` sends the setter `count:`, which a subclass
    /// may override; an initializer stores directly. Only a private send
    /// is never overridden.
    #[test]
    fn a_protected_or_public_header_slot_gets_no_hint() {
        let text = indoc! {"
            class Module = (
              |
                count
                protected limit
                public total
              |
              count:: 1.
              limit:: 2.
              total:: 3.
            ) ()
        "};
        assert!(names_of(text, HintKind::InitializeInDeclaration).is_empty());
    }

    /// A bare slot may end in its optional `.`: the initializer goes before
    /// it, so the declaration keeps one period.
    #[test]
    fn a_bare_local_written_with_its_period_takes_the_initializer_before_it() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count.
                |
                count:: 1.
                ^count
              )
            )
        "};
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

    #[test]
    fn a_local_written_with_its_period_moves_without_it() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count.
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

    /// A previous bare slot keeps its own period when the one after it
    /// moves.
    #[test]
    fn a_moved_local_leaves_the_period_of_the_one_before_it() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  first.
                  count.
                  total ::= 0.
                |
                count:: 1.
                ^count + total + first
              )
            )
        "};
        assert_eq!(
            fixed(text, HintKind::InitializeInDeclaration, "count"),
            indoc! {"
                class Module = () (
                  public run = (
                    |
                      first.
                      total ::= 0.
                      count ::= 1.
                    |
                    ^count + total + first
                  )
                )
            "}
        );
    }

    /// A closure answers the value of its last statement; an initializer
    /// has none.
    #[test]
    fn the_last_statement_of_a_closure_keeps_its_place() {
        let text = indoc! {"
            class Module = () (
              public run = (
                ^[
                  |
                    count
                  |
                  count:: 5
                ] value
              )
            )
        "};
        assert!(names_of(text, HintKind::InitializeInDeclaration).is_empty());
    }

    #[test]
    fn a_closure_statement_followed_by_another_moves() {
        let text = indoc! {"
            class Module = () (
              public run = (
                ^[
                  |
                    count
                  |
                  count:: 5.
                  count + 1
                ] value
              )
            )
        "};
        assert_eq!(names_of(text, HintKind::InitializeInDeclaration), ["count"]);
    }

    /// A method answers `self` without `^`, so its last statement may move.
    #[test]
    fn the_last_statement_of_a_method_moves() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                |
                count:: 5
              )
            )
        "};
        assert_eq!(names_of(text, HintKind::InitializeInDeclaration), ["count"]);
    }

    /// A comment stays beside the code it was written beside: none moves or
    /// is copied with a reordered declaration, and none is left describing
    /// the statement after a deleted one.
    #[test]
    fn a_comment_beside_a_reordered_declaration_or_on_the_statement_keeps_the_code() {
        let separator = indoc! {"
            class Module = () (
              public run = (
                |
                  first
                  count
                  /// running total
                  total ::= 0.
                |
                count:: 1.
                ^count + total + first
              )
            )
        "};
        let destination = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                  total ::= 0. /// the sum
                |
                count:: 1.
                ^count + total
              )
            )
        "};
        let attached = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                |
                /// set the count
                count:: 1.
                ^count
              )
            )
        "};
        for text in [separator, destination, attached] {
            assert!(
                !names_of(text, HintKind::InitializeInDeclaration).contains(&"count".to_string()),
                "{text}"
            );
        }
    }

    #[test]
    fn a_settled_value_after_a_guard_moves_into_its_declaration() {
        let text = indoc! {"
            class Module = () (
              public run: limit = (
                |
                  count
                  index
                  start = 1.
                  step
                |
                limit < 0 ifTrue: [^nil].
                count:: 0.
                index:: limit.
                step:: start.
                ^count + index + step
              )
            )
        "};
        assert_eq!(
            names_of(text, HintKind::InitializeInDeclaration),
            ["count", "index", "step"]
        );
        let result = fixed(text, HintKind::InitializeInDeclaration, "count");
        assert!(result.contains("      count ::= 0.\n"), "{result}");
        assert!(!result.contains("    count:: 0.\n"), "{result}");
        let index = fixed(text, HintKind::InitializeInDeclaration, "index");
        assert_eq!(
            index,
            indoc! {"
                class Module = () (
                  public run: limit = (
                    |
                      count
                      start = 1.
                      index ::= limit.
                      step
                    |
                    limit < 0 ifTrue: [^nil].
                    count:: 0.
                    step:: start.
                    ^count + index + step
                  )
                )
            "}
        );
        let step = fixed(text, HintKind::InitializeInDeclaration, "step");
        assert_eq!(
            step,
            indoc! {"
                class Module = () (
                  public run: limit = (
                    |
                      count
                      index
                      start = 1.
                      step ::= start.
                    |
                    limit < 0 ifTrue: [^nil].
                    count:: 0.
                    index:: limit.
                    ^count + index + step
                  )
                )
            "}
        );
    }

    #[test]
    fn a_value_that_may_fail_or_change_stays_after_the_guard() {
        let text = indoc! {"
            class Module = () (
              public run: limit = (
                |
                  count
                  index
                  total ::= 0.
                |
                limit < 0 ifTrue: [^nil].
                count:: self size.
                index:: total.
                ^count + index
              )
            )
        "};
        assert!(names_of(text, HintKind::InitializeInDeclaration).is_empty());
    }

    /// A guard that reads the slot would see the value instead of nil.
    #[test]
    fn a_guard_that_mentions_the_slot_keeps_its_assignment() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                |
                count isNil ifFalse: [^count].
                count:: 0.
                ^count
              )
            )
        "};
        assert!(names_of(text, HintKind::InitializeInDeclaration).is_empty());
    }

    #[test]
    fn a_header_slot_after_another_statement_gets_no_hint() {
        let text = indoc! {"
            class Module withLimit: limit = (
              |
                used ::= 0.
                private cap
              |
              used printString.
              cap:: limit.
            ) ()
        "};
        assert!(names_of(text, HintKind::InitializeInDeclaration).is_empty());
    }

    #[test]
    fn a_factory_parameter_as_the_first_header_statement_moves() {
        let text = indoc! {"
            class Module withLimit: limit = (
              |
                used ::= 0.
                private cap
              |
              cap:: limit.
            ) ()
        "};
        assert_eq!(names_of(text, HintKind::InitializeInDeclaration), ["cap"]);
    }

    /// A closure an initializer made would read the slot before the statement.
    #[test]
    fn an_initializer_closure_that_mentions_the_slot_keeps_the_assignment() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  count
                  reader = [count].
                |
                reader value printString.
                count:: 0.
                ^count
              )
            )
        "};
        assert!(names_of(text, HintKind::InitializeInDeclaration).is_empty());
    }
}
