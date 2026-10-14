# Editor Hints Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The language server marks, in every `.ns` file, a slot whose first value a statement assigns (hint A, "initialize in the declaration") and a slot nothing assigns again (hint B, "read-only"), each with a quick fix and one fix for the whole file, every fix preserving behaviour.

**Architecture:** A syntactic analysis in `ns-syntax` (`src/hints/`) walks the AST, finds the hints and builds each fix as minimal text edits in byte offsets; `apply_all` applies every hint in rounds. `ns-analyzer` publishes the hints as `HINT` diagnostics and answers `textDocument/codeAction`. A behaviour oracle in `ns-cli`'s tests runs every golden with every hint applied under both lowerings; a gate keeps the core library free of hints.

**Tech Stack:** Rust 1.98 (edition 2021), `ns-syntax`'s AST, `lsp-types` 0.97 / `lsp-server` 0.10, `libtest-mimic` for the oracle, `cargo-nextest`.

**Spec:** `docs/superpowers/specs/2026-10-14-editor-hints-design.md` -- read it before any task; it states every rule and why it is safe.

## Global Constraints

- Names are written out: no identifier of one or two characters, no truncated word (`CLAUDE.md`, "Conventions"; clippy's `min_ident_chars` enforces the short ones).
- Comments explain the decision, not the mechanics; English only.
- A chain of messages is walked link by link through `Send::receiver_chain`, never by recursing along a receiver (`CLAUDE.md`, "A chain of messages is walked link by link").
- `lsp-server`, `lsp-types` and `serde_json` stay in `ns-analyzer`; `ns-syntax` gains no dependency.
- A Newspeak module inline in a test is an `indoc! {"…"}`, indented with spaces with the Rust around it, the closing `"}` on its own line.
- After editing any `.ns` file under `crates/ns-runtime/core-library/`, run `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library`.
- Before every commit (`CLAUDE.md`, "Working in this repository"): `cargo fmt --all`; `cargo clippy --workspace --all-targets` warning-free; `cargo nextest run --workspace`; `cargo test --workspace --doc`; `cargo run -p ns-analyzer -- parse-corpus corpus/modules` answers "parsed 211, failed 0"; and, since every task touches `crates/ns-syntax` or `crates/ns-analyzer`, `./editors/jetbrains/gradlew -p editors/jetbrains check`.
- The end-of-batch gates (the five stress modes, `run-corpus` plain and under stress, the benchmarks profile, `verifyPlugin`) are **not** run per task: they belong to the `finishing-a-batch` skill after the last task. A task's commits are work in progress until then.
- Commit messages are conventional commits and end with the two attribution lines:

  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_019Bf4KwW5VtV99p2wu3An3U
  ```

- Work happens on the branch `feat/editor-hints` (already created, the spec committed on it).

## Review Focus

- A source with CRLF line endings: every edit (line deletion, continuation-line shift) keeps `\r\n` intact and the result parses -- tested in Task 3.
- A slot declared in a one-line section (`| count |`): the insertion gives `| count ::= 1. |`, which parses -- tested in Task 3.
- A statement followed by a `///` on its line, or a `(*` inside it: no hint, so no comment is ever deleted or moved to other code -- tested in Task 3.
- A file with a syntax error: no hint, so no code action, never a panic -- tested in Task 1 (`hints_of`); the server's actions come only from `Analysis::hints`.
- A moved statement whose value holds a closure with hints of its own: "apply all" applies the outer one first and the inner one in a later round, and the result parses -- tested in Task 5.

## File Structure

- `crates/ns-syntax/src/hints/mod.rs` -- `Hint`, `HintKind`, `hints`, `hints_of`, `apply_all`; the visitor that hands every section to the rules.
- `crates/ns-syntax/src/hints/walk.rs` -- the walk over a module: every section (a code body, a class header) with the names its code sees (`Scope`).
- `crates/ns-syntax/src/hints/names.rs` -- what code mentions and what it may assign, by name.
- `crates/ns-syntax/src/hints/edits.rs` -- `Edit`, `apply`, and the edits a fix makes (insert an initializer, delete a statement, find `::=`).
- `crates/ns-syntax/src/hints/read_only.rs` -- hint B.
- `crates/ns-syntax/src/hints/initialize.rs` -- hint A.
- `crates/ns-syntax/src/lib.rs` -- `pub mod hints;`.
- `crates/ns-analyzer/src/analysis.rs` -- `Analysis::hints`.
- `crates/ns-analyzer/src/code_actions.rs` (new) -- hints as LSP diagnostics and code actions.
- `crates/ns-analyzer/src/server.rs` -- publishes the hint diagnostics, announces and answers code actions.
- `crates/ns-analyzer/tests/protocol.rs` -- the protocol tests.
- `crates/ns-analyzer/tests/hint_gate.rs` (new) -- the core library has no hint.
- `crates/ns-cli/tests/common/mod.rs` -- `compare` moves here from `run_goldens.rs`.
- `crates/ns-cli/tests/hinted_goldens.rs` (new) -- the behaviour oracle.
- `crates/ns-cli/Cargo.toml` -- the oracle's `[[test]]` entry.
- `crates/ns-runtime/core-library/**/*.ns`, `crates/ns-runtime/tests/fixtures/golden-image.bin` -- the core library brought to no hint (Task 5).
- `docs/design/editor-support.md`, `CLAUDE.md`, the spec (deleted) -- Task 8.

---

### Task 1: The walk, the names, the edits, and hint B for locals

**Files:**
- Create: `crates/ns-syntax/src/hints/mod.rs`, `walk.rs`, `names.rs`, `edits.rs`, `read_only.rs`, `initialize.rs` (an empty rule for now)
- Modify: `crates/ns-syntax/src/lib.rs` (add `pub mod hints;` after `pub mod diagnostic;`)
- Test: unit tests in `crates/ns-syntax/src/hints/mod.rs`

**Interfaces:**
- Produces (public, used by Tasks 5-7):
  - `ns_syntax::hints::HintKind { InitializeInDeclaration, ReadOnly }` (`Debug, Clone, Copy, PartialEq, Eq`)
  - `ns_syntax::hints::Hint { pub kind: HintKind, pub name: String, pub span: Span, pub section: Span, pub edits: Vec<Edit> }` with `pub fn code(&self) -> &'static str`, `pub fn message(&self) -> String`, `pub fn title(&self) -> String`
  - `ns_syntax::hints::Edit { pub span: Span, pub text: String }`
  - `ns_syntax::hints::apply(text: &str, edits: &[Edit]) -> String`
  - `ns_syntax::hints::hints(module: &Module, text: &str) -> Vec<Hint>` and `hints_of(text: &str) -> Vec<Hint>`
- Produces (crate-internal, used by Tasks 2-4): `walk::{Section, SectionKind, Scope, Binding, Visitor, walk}`, `names::{mentions, expression_mentions, assigned_in, written_in_class}`, `edits::{overlap, holds_a_comment, declaration_end, initializer, delete_statement, assignment_operator}`

- [ ] **Step 1: Write `edits.rs`**

```rust
//! The text edits a fix makes, and applying them. Every edit is minimal:
//! a fix changes the code its hint names and leaves the layout around it
//! as it was, so a file that was never formatted is not reformatted by a
//! quick fix.

use crate::ast::{Expression, SlotDeclaration, Statement};
use crate::span::Span;

/// The text of `span` replaced by `text`; an empty span inserts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub text: String,
}

/// `text` with `edits` applied. The edits must not overlap.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.span.start, edit.span.end));
    let mut result = String::with_capacity(text.len());
    let mut position = 0;
    for edit in ordered {
        let start = edit.span.start as usize;
        debug_assert!(start >= position, "overlapping edits");
        result.push_str(&text[position..start]);
        result.push_str(&edit.text);
        position = edit.span.end as usize;
    }
    result.push_str(&text[position..]);
    result
}

/// Whether two edits touch the same text or insert at the same place:
/// two such edits cannot be applied together.
pub(super) fn overlap(first: &Edit, second: &Edit) -> bool {
    (first.span.start < second.span.end && second.span.start < first.span.end)
        || first.span.start == second.span.start
}

/// Whether `text` holds the start of a comment. A fix never deletes or
/// moves one: a comment belongs to the code beside it. A string that
/// spells `///` or `(*` counts too, which costs a hint, never correctness.
pub(super) fn holds_a_comment(text: &str) -> bool {
    text.contains("///") || text.contains("(*")
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

/// The offset of the `\n` that ends the line holding `offset`, or the
/// text's end.
fn line_end(text: &str, offset: usize) -> usize {
    text[offset..].find('\n').map_or(text.len(), |index| offset + index)
}

fn column(text: &str, offset: usize) -> usize {
    offset - line_start(text, offset)
}

/// Where a slot's declaration ends: after its `.` when it has an
/// initializer, else after its name or its type. A bare slot's span runs
/// on to the token after it (`parser/declaration.rs`), so it cannot say.
pub(super) fn declaration_end(slot: &SlotDeclaration) -> u32 {
    if slot.initializer.is_some() {
        return slot.span.end;
    }
    slot.type_annotation
        .as_ref()
        .map_or(slot.name.span.end, |annotation| annotation.span.end)
}

/// ` ::= value.`, the value as written. Its continuation lines move right
/// by as many columns as the declaration stands right of the statement
/// (left when it stands left of it), so they keep their place relative to
/// the code they continue.
pub(super) fn initializer(
    text: &str,
    value: &Expression,
    statement_start: u32,
    declaration_start: u32,
) -> String {
    let declaration_column = column(text, declaration_start as usize);
    let statement_column = column(text, statement_start as usize);
    let written = &text[value.span.start as usize..value.span.end as usize];
    let mut lines = written.split('\n');
    let mut moved = lines.next().unwrap_or_default().to_string();
    for line in lines {
        moved.push('\n');
        if declaration_column >= statement_column {
            moved.push_str(&" ".repeat(declaration_column - statement_column));
            moved.push_str(line);
        } else {
            let blanks = line.len() - line.trim_start_matches(' ').len();
            let removed = blanks.min(statement_column - declaration_column);
            moved.push_str(&line[removed..]);
        }
    }
    format!(" ::= {moved}.")
}

/// The edit that deletes `statement` with its `.`: its whole line, line
/// break included, when nothing else stands on it, else the statement and
/// the blanks after it. `None` when a comment stands in the statement or
/// on the rest of its line: deleting the one or leaving the other behind
/// would change what the comment is about.
pub(super) fn delete_statement(text: &str, statement: &Statement) -> Option<Edit> {
    let start = statement.span.start as usize;
    let mut end = statement.span.end as usize;
    let after = text[end..].trim_start_matches([' ', '\t', '\r', '\n']);
    if after.starts_with('.') {
        end = text.len() - after.len() + 1;
    }
    let rest = &text[end..line_end(text, end)];
    if holds_a_comment(&text[start..end]) || holds_a_comment(rest) {
        return None;
    }
    let before = &text[line_start(text, start)..start];
    let span = if before.trim().is_empty() && rest.trim().is_empty() {
        let finish = (line_end(text, end) + 1).min(text.len());
        Span::new(line_start(text, start) as u32, finish as u32)
    } else {
        let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        Span::new(start as u32, (end + blanks) as u32)
    };
    Some(Edit {
        span,
        text: String::new(),
    })
}

/// The span of the `::=` of a slot declared with it.
pub(super) fn assignment_operator(text: &str, slot: &SlotDeclaration) -> Option<Span> {
    let initializer = slot.initializer.as_ref()?;
    let from = slot
        .type_annotation
        .as_ref()
        .map_or(slot.name.span.end, |annotation| annotation.span.end) as usize;
    let index = text[from..initializer.span.start as usize].find("::=")?;
    let start = (from + index) as u32;
    Some(Span::new(start, start + 3))
}
```

Note on `delete_statement`: a statement's `.` follows its span after optional blanks; `after` is the text from the span's end with leading blanks removed, so `text.len() - after.len()` is the offset of the `.`.

- [ ] **Step 2: Write `names.rs`**

```rust
//! What code mentions and what it may assign, by name. Every test here is
//! by name alone, never by binding: an `x::` that writes a closure's own
//! `x`, or another class's, still counts as writing `x`. That
//! over-approximates, so it costs a hint at worst, never correctness.

use std::collections::HashSet;

use crate::ast::{
    ClassDeclaration, CodeBody, Expression, ExpressionKind, Member, MessageSend, Selector,
    SlotDeclaration, Statement, StatementKind,
};

/// One node `each_node` hands over.
enum Node<'tree> {
    Expression(&'tree Expression),
    Message(&'tree MessageSend),
}

/// Hands `visit` every expression and every message in `expression`, the
/// code of its closures included.
fn each_node<'tree>(expression: &'tree Expression, visit: &mut dyn FnMut(Node<'tree>)) {
    visit(Node::Expression(expression));
    match &expression.kind {
        ExpressionKind::Identifier(_) | ExpressionKind::Literal(_) => {}
        // A chain of messages is walked link by link: it nests as deep as
        // the source is long.
        ExpressionKind::Send(send) => {
            let (links, receiver) = send.receiver_chain();
            each_node(receiver, visit);
            for link in links.iter().rev() {
                visit(Node::Message(&link.message));
                for argument in &link.message.arguments {
                    each_node(argument, visit);
                }
            }
        }
        ExpressionKind::Cascade(cascade) => {
            each_node(&cascade.receiver, visit);
            for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                visit(Node::Message(message));
                for argument in &message.arguments {
                    each_node(argument, visit);
                }
            }
        }
        ExpressionKind::Assign { value, .. } => each_node(value, visit),
        ExpressionKind::Block(block) => each_node_in_body(&block.body, visit),
        ExpressionKind::DynamicArray { elements, .. } => {
            for element in elements {
                each_node(element, visit);
            }
        }
    }
}

fn each_node_in_statement<'tree>(statement: &'tree Statement, visit: &mut dyn FnMut(Node<'tree>)) {
    match &statement.kind {
        StatementKind::Expression(inner) | StatementKind::Return(inner) => each_node(inner, visit),
    }
}

fn each_node_in_slots<'tree>(slots: &'tree [SlotDeclaration], visit: &mut dyn FnMut(Node<'tree>)) {
    for slot in slots {
        if let Some(initializer) = &slot.initializer {
            each_node(initializer, visit);
        }
    }
}

fn each_node_in_body<'tree>(code: &'tree CodeBody, visit: &mut dyn FnMut(Node<'tree>)) {
    each_node_in_slots(&code.temporaries, visit);
    for statement in &code.statements {
        each_node_in_statement(statement, visit);
    }
}

/// Every node of `declaration`: its superclass clauses, its header, its
/// members -- nested classes included -- and its class side.
fn each_node_in_class<'tree>(
    declaration: &'tree ClassDeclaration,
    visit: &mut dyn FnMut(Node<'tree>),
) {
    for clause in declaration.superclass.iter().chain(&declaration.mixin_applications) {
        for message in &clause.messages {
            visit(Node::Message(message));
            for argument in &message.arguments {
                each_node(argument, visit);
            }
        }
    }
    let instance = &declaration.instance_side;
    each_node_in_slots(&instance.header_slots, visit);
    for statement in &instance.initializer_expressions {
        each_node_in_statement(statement, visit);
    }
    for side in std::iter::once(instance).chain(declaration.class_side.as_ref()) {
        for member in &side.members {
            match member {
                Member::Class(nested) => each_node_in_class(nested, visit),
                Member::LazySlot(slot) => {
                    if let Some(initializer) = &slot.initializer {
                        each_node(initializer, visit);
                    }
                }
                Member::Method(method) => each_node_in_body(&method.body, visit),
            }
        }
    }
}

/// Whether `message` is the setter of `name`: the one-keyword selector
/// `name:`.
fn is_setter_of(message: &MessageSend, name: &str) -> bool {
    matches!(&message.selector, Selector::Keyword(selector) if selector.strip_suffix(':') == Some(name))
}

/// Whether `node` reads, assigns or sends `name` (`name` or `name:`).
fn names(node: &Node<'_>, name: &str) -> bool {
    match node {
        Node::Expression(expression) => match &expression.kind {
            ExpressionKind::Identifier(identifier) => identifier.name == name,
            ExpressionKind::Assign { target, .. } => target.name == name,
            _ => false,
        },
        Node::Message(message) => {
            is_setter_of(message, name)
                || matches!(&message.selector, Selector::Unary(selector) if selector == name)
        }
    }
}

/// Whether `name` appears anywhere in `statement`.
pub(super) fn mentions(statement: &Statement, name: &str) -> bool {
    let mut found = false;
    each_node_in_statement(statement, &mut |node| found |= names(&node, name));
    found
}

/// Whether `name` appears anywhere in `expression`.
pub(super) fn expression_mentions(expression: &Expression, name: &str) -> bool {
    let mut found = false;
    each_node(expression, &mut |node| found |= names(&node, name));
    found
}

/// The names `slots`' initializers and `statements` assign with `name::`,
/// their closures included: the only way a local is written.
pub(super) fn assigned_in<'tree>(
    slots: &'tree [SlotDeclaration],
    statements: &'tree [Statement],
) -> HashSet<&'tree str> {
    let mut assigned = HashSet::new();
    let mut visit = |node: Node<'tree>| {
        if let Node::Expression(expression) = node {
            if let ExpressionKind::Assign { target, .. } = &expression.kind {
                assigned.insert(target.name.as_str());
            }
        }
    };
    each_node_in_slots(slots, &mut visit);
    for statement in statements {
        each_node_in_statement(statement, &mut visit);
    }
    assigned
}

/// The names `declaration` assigns with `name::` or sends `name:` to,
/// anywhere in it, whatever the receiver: everything that could write a
/// private slot of the class.
pub(super) fn written_in_class(declaration: &ClassDeclaration) -> HashSet<&str> {
    let mut written = HashSet::new();
    each_node_in_class(declaration, &mut |node| match node {
        Node::Expression(expression) => {
            if let ExpressionKind::Assign { target, .. } = &expression.kind {
                written.insert(target.name.as_str());
            }
        }
        Node::Message(message) => {
            if let Selector::Keyword(selector) = &message.selector {
                if let Some(name) = selector.strip_suffix(':') {
                    if !name.contains(':') {
                        written.insert(name);
                    }
                }
            }
        }
    });
    written
}
```

- [ ] **Step 3: Write `walk.rs`**

```rust
//! The walk over a module that hands every section to the hints, with the
//! names its code sees.

use crate::ast::{
    ClassDeclaration, CodeBody, Expression, ExpressionKind, Member, MessagePattern, Module,
    SlotDeclaration, SlotMutability, Statement, StatementKind,
};
use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SectionKind {
    /// A method's or a closure's code: its temporaries and statements.
    Body,
    /// A class's instance-side header: its slots and initializer
    /// expressions.
    Header,
}

/// Slots and the statements that run after their initializers.
pub(super) struct Section<'tree> {
    pub(super) kind: SectionKind,
    pub(super) slots: &'tree [SlotDeclaration],
    pub(super) statements: &'tree [Statement],
    pub(super) span: Span,
}

/// What a name a section's code reads is bound to, as far as the hints
/// need to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Binding {
    /// A method's, a closure's or a factory's parameter: it never changes.
    Parameter,
    /// A local declared with `=`: it never changes once initialized.
    ReadOnlyLocal,
    /// A local that may change.
    MutableLocal,
}

/// The names declared around a section, the innermost frame last. A class
/// starts afresh: a method sees no enclosing method's names.
#[derive(Default)]
pub(super) struct Scope<'tree> {
    frames: Vec<Vec<(&'tree str, Binding)>>,
}

impl<'tree> Scope<'tree> {
    fn push(&mut self, parameters: Vec<&'tree str>, temporaries: &'tree [SlotDeclaration]) {
        let mut frame: Vec<(&'tree str, Binding)> = parameters
            .into_iter()
            .map(|name| (name, Binding::Parameter))
            .collect();
        frame.extend(temporaries.iter().map(|slot| {
            let binding = if slot.mutability == SlotMutability::Immutable {
                Binding::ReadOnlyLocal
            } else {
                Binding::MutableLocal
            };
            (slot.name.name.as_str(), binding)
        }));
        self.frames.push(frame);
    }

    fn pop(&mut self) {
        self.frames.pop();
    }

    /// What `name` is bound to, the innermost declaration deciding; `None`
    /// for a name no enclosing code declares, which is a send.
    pub(super) fn binding(&self, name: &str) -> Option<Binding> {
        self.frames.iter().rev().find_map(|frame| {
            frame
                .iter()
                .find(|(declared, _)| *declared == name)
                .map(|(_, binding)| *binding)
        })
    }
}

pub(super) trait Visitor<'tree> {
    fn class(&mut self, declaration: &'tree ClassDeclaration);
    fn section(&mut self, section: &Section<'tree>, scope: &Scope<'tree>);
}

pub(super) fn walk<'tree>(module: &'tree Module, visitor: &mut impl Visitor<'tree>) {
    class(&module.root_class, visitor);
}

fn parameters_of(pattern: &MessagePattern) -> Vec<&str> {
    match pattern {
        MessagePattern::Unary { .. } => Vec::new(),
        MessagePattern::Binary { parameter, .. } => vec![parameter.name.name.as_str()],
        MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| part.parameter.name.name.as_str())
            .collect(),
    }
}

fn class<'tree>(declaration: &'tree ClassDeclaration, visitor: &mut impl Visitor<'tree>) {
    visitor.class(declaration);
    let instance = &declaration.instance_side;
    // The superclass clauses and the header see the factory's parameters
    // (§6.1); the members do not.
    let mut scope = Scope::default();
    scope.push(
        declaration.factory.as_ref().map_or_else(Vec::new, parameters_of),
        &[],
    );
    for clause in declaration.superclass.iter().chain(&declaration.mixin_applications) {
        for message in &clause.messages {
            for argument in &message.arguments {
                expression(argument, &mut scope, visitor);
            }
        }
    }
    for slot in &instance.header_slots {
        if let Some(initializer) = &slot.initializer {
            expression(initializer, &mut scope, visitor);
        }
    }
    for statement in &instance.initializer_expressions {
        self::statement(statement, &mut scope, visitor);
    }
    visitor.section(
        &Section {
            kind: SectionKind::Header,
            slots: &instance.header_slots,
            statements: &instance.initializer_expressions,
            span: instance.span,
        },
        &scope,
    );
    for side in std::iter::once(instance).chain(declaration.class_side.as_ref()) {
        for member in &side.members {
            match member {
                Member::Class(nested) => class(nested, visitor),
                Member::LazySlot(slot) => {
                    if let Some(initializer) = &slot.initializer {
                        expression(initializer, &mut Scope::default(), visitor);
                    }
                }
                Member::Method(method) => body(
                    &method.body,
                    parameters_of(&method.pattern),
                    &mut Scope::default(),
                    visitor,
                ),
            }
        }
    }
}

fn body<'tree>(
    code: &'tree CodeBody,
    parameters: Vec<&'tree str>,
    scope: &mut Scope<'tree>,
    visitor: &mut impl Visitor<'tree>,
) {
    scope.push(parameters, &code.temporaries);
    for slot in &code.temporaries {
        if let Some(initializer) = &slot.initializer {
            expression(initializer, scope, visitor);
        }
    }
    for statement in &code.statements {
        self::statement(statement, scope, visitor);
    }
    visitor.section(
        &Section {
            kind: SectionKind::Body,
            slots: &code.temporaries,
            statements: &code.statements,
            span: code.span,
        },
        scope,
    );
    scope.pop();
}

fn statement<'tree>(
    statement: &'tree Statement,
    scope: &mut Scope<'tree>,
    visitor: &mut impl Visitor<'tree>,
) {
    match &statement.kind {
        StatementKind::Expression(inner) | StatementKind::Return(inner) => {
            expression(inner, scope, visitor);
        }
    }
}

fn expression<'tree>(
    expression: &'tree Expression,
    scope: &mut Scope<'tree>,
    visitor: &mut impl Visitor<'tree>,
) {
    match &expression.kind {
        ExpressionKind::Identifier(_) | ExpressionKind::Literal(_) => {}
        // A chain of messages is walked link by link: it nests as deep as
        // the source is long.
        ExpressionKind::Send(send) => {
            let (links, receiver) = send.receiver_chain();
            self::expression(receiver, scope, visitor);
            for link in links.iter().rev() {
                for argument in &link.message.arguments {
                    self::expression(argument, scope, visitor);
                }
            }
        }
        ExpressionKind::Cascade(cascade) => {
            self::expression(&cascade.receiver, scope, visitor);
            for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                for argument in &message.arguments {
                    self::expression(argument, scope, visitor);
                }
            }
        }
        ExpressionKind::Assign { value, .. } => self::expression(value, scope, visitor),
        ExpressionKind::Block(block) => {
            let parameters = block
                .parameters
                .iter()
                .map(|parameter| parameter.name.name.as_str())
                .collect();
            body(&block.body, parameters, scope, visitor);
        }
        ExpressionKind::DynamicArray { elements, .. } => {
            for element in elements {
                self::expression(element, scope, visitor);
            }
        }
    }
}
```

- [ ] **Step 4: Write `read_only.rs` (locals only for now) and an empty `initialize.rs`**

`read_only.rs`:

```rust
//! Hint B: a slot declared with `::=` that nothing assigns again can be
//! declared with `=`. Only `name::` writes a local -- an implicit-receiver
//! send `name: value` is resolved through the classes, never the locals
//! (`ns-runtime`'s `resolver/mod.rs`) -- so a local is read-only when no
//! `name::` stands in its body.

use crate::ast::SlotMutability;
use crate::span::Span;

use super::edits::{self, Edit};
use super::names;
use super::walk::Section;
use super::{Hint, HintKind};

/// Hint B for the locals of a body's section.
pub(super) fn local_hints(section: &Section<'_>, text: &str, hints: &mut Vec<Hint>) {
    if section.slots.iter().any(|slot| slot.simultaneous) {
        return;
    }
    let assigned = names::assigned_in(section.slots, section.statements);
    for slot in section.slots {
        if slot.mutability != SlotMutability::MutableInitialized
            || assigned.contains(slot.name.name.as_str())
        {
            continue;
        }
        push(slot, section.span, text, hints);
    }
}

fn push(
    slot: &crate::ast::SlotDeclaration,
    section: Span,
    text: &str,
    hints: &mut Vec<Hint>,
) {
    let Some(operator) = edits::assignment_operator(text, slot) else {
        return;
    };
    hints.push(Hint {
        kind: HintKind::ReadOnly,
        name: slot.name.name.clone(),
        span: slot.name.span,
        section,
        edits: vec![Edit {
            span: operator,
            text: "=".to_string(),
        }],
    });
}
```

`initialize.rs`:

```rust
//! Hint A: a bare slot whose first value a statement assigns can take that
//! value in its declaration.

use super::walk::{Scope, Section};
use super::Hint;

pub(super) fn hints(_section: &Section<'_>, _scope: &Scope<'_>, _text: &str, _hints: &mut Vec<Hint>) {}
```

(Task 3 replaces this file.)

- [ ] **Step 5: Write `mod.rs`**

```rust
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
                format!("`{}` is never assigned again and can be read-only", self.name)
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
```

- [ ] **Step 6: Write the failing tests** (append to `mod.rs`)

```rust
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
            Edit { span: Span::new(4, 5), text: "X".to_string() },
            Edit { span: Span::new(0, 1), text: "Y".to_string() },
        ];
        assert_eq!(apply("abcdef", &edits), "YbcdXf");
    }
}
```

- [ ] **Step 7: Run the tests**

Run: `cargo nextest run -p ns-syntax hints`
Expected: all five PASS (they were written against the code above; if one fails, fix the code, not the expectation, unless the expectation contradicts the spec).

- [ ] **Step 8: Run the per-commit gates** (Global Constraints), then commit

```bash
git add crates/ns-syntax/src/hints crates/ns-syntax/src/lib.rs
git commit -m "feat: find the locals that can be read-only, with their fixes" # plus the attribution lines
```

---

### Task 2: Hint B for private header slots

**Files:**
- Modify: `crates/ns-syntax/src/hints/read_only.rs`, `crates/ns-syntax/src/hints/mod.rs` (`Collector::class`)
- Test: `crates/ns-syntax/src/hints/mod.rs` (tests module)

**Interfaces:**
- Consumes: `names::written_in_class(&ClassDeclaration) -> HashSet<&str>`, `read_only::push` (Task 1)
- Produces: `read_only::header_hints(declaration: &ClassDeclaration, text: &str, hints: &mut Vec<Hint>)`

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-syntax hints::tests::a_private`
Expected: FAIL -- no hint for `limit`.

- [ ] **Step 3: Implement**

In `read_only.rs` add:

```rust
use crate::ast::{AccessModifier, ClassDeclaration};

/// Hint B for a class's private header slots. A private member is reached
/// only by sends from within the class that declares it (§5.8, §5.9,
/// §6.3.1); `Message>>sendTo:` is an ordinary public send (§5.5), and no
/// program has a reflective slot write. So a private slot nothing in its
/// class declaration writes -- by `name::` or by a send of `name:` -- is
/// never written. A public or protected slot's setter may be sent from
/// anywhere, so it gets no hint.
pub(super) fn header_hints(declaration: &ClassDeclaration, text: &str, hints: &mut Vec<Hint>) {
    let header = &declaration.instance_side;
    if header.header_slots.iter().any(|slot| slot.simultaneous) {
        return;
    }
    let mut written = None;
    for slot in &header.header_slots {
        if slot.access != Some(AccessModifier::Private)
            || slot.mutability != SlotMutability::MutableInitialized
        {
            continue;
        }
        let written = written.get_or_insert_with(|| names::written_in_class(declaration));
        if !written.contains(slot.name.name.as_str()) {
            push(slot, header.span, text, hints);
        }
    }
}
```

In `mod.rs`, `Collector::class` becomes:

```rust
    fn class(&mut self, declaration: &'tree ClassDeclaration) {
        read_only::header_hints(declaration, self.text, &mut self.hints);
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-syntax hints`
Expected: all PASS.

- [ ] **Step 5: Gates, then commit**

```bash
git add crates/ns-syntax/src/hints
git commit -m "feat: find the private header slots that can be read-only" # plus the attribution lines
```

---

### Task 3: Hint A at a section's first statement

**Files:**
- Modify: `crates/ns-syntax/src/hints/initialize.rs` (replace the empty rule)
- Test: `crates/ns-syntax/src/hints/mod.rs` (tests module)

**Interfaces:**
- Consumes: `walk::{Section, SectionKind, Scope}`, `edits::{declaration_end, initializer, delete_statement, holds_a_comment, Edit}`, `names::expression_mentions`
- Produces: `initialize::hints(section, scope, text, hints)`; `initialize::settled` stays private (Task 4 fills it in)

- [ ] **Step 1: Write the failing tests**

```rust
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
            assert!(names_of(&text, HintKind::InitializeInDeclaration).is_empty(), "{statement}");
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
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-syntax hints`
Expected: the new tests FAIL -- no hint A yet.

- [ ] **Step 3: Implement `initialize.rs`**

```rust
//! Hint A: a bare slot whose first value a statement assigns can take that
//! value in its declaration. A fix must keep the order of evaluation:
//! every initializer of the section first, then the moved value, as when
//! it was a statement. At the section's first statement that holds
//! whatever the value is; after other statements only for a value that
//! can neither fail nor see what they change (`settled`). Every fix stands
//! on its own: once the first statement has moved, the next one is first.

use crate::ast::{Expression, ExpressionKind, SlotDeclaration, SlotMutability, Statement, StatementKind};
use crate::span::Span;

use super::edits::{self, Edit};
use super::names;
use super::walk::{Scope, Section, SectionKind};
use super::{Hint, HintKind};

pub(super) fn hints(section: &Section<'_>, scope: &Scope<'_>, text: &str, hints: &mut Vec<Hint>) {
    if section.slots.iter().any(|slot| slot.simultaneous) {
        return;
    }
    let last_initialized = section.slots.iter().rposition(|slot| slot.initializer.is_some());
    for (index, statement) in section.statements.iter().enumerate() {
        let Some((target, value)) = assignment(statement) else {
            continue;
        };
        let Some(position) = bare_slot(section.slots, target) else {
            continue;
        };
        if names::expression_mentions(value, target) {
            continue;
        }
        let movable = index == 0
            || (settled(value, scope)
                && !section.statements[..index]
                    .iter()
                    .any(|earlier| names::mentions(earlier, target)));
        if !movable {
            continue;
        }
        let Some(deletion) = edits::delete_statement(text, statement) else {
            continue;
        };
        let placed = match last_initialized {
            Some(last) if position < last => {
                if section.kind != SectionKind::Body {
                    continue;
                }
                reordered(text, section.slots, position, last, statement, value)
            }
            _ => Some(in_place(text, &section.slots[position], statement, value)),
        };
        let Some(mut fix) = placed else {
            continue;
        };
        fix.push(deletion);
        hints.push(Hint {
            kind: HintKind::InitializeInDeclaration,
            name: target.to_string(),
            span: section.slots[position].name.span,
            section: section.span,
            edits: fix,
        });
    }
}

/// The target and value of a statement that is one assignment `x:: e`, not
/// parenthesized and not a chain.
fn assignment(statement: &Statement) -> Option<(&str, &Expression)> {
    let StatementKind::Expression(expression) = &statement.kind else {
        return None;
    };
    let ExpressionKind::Assign { target, value, .. } = &expression.kind else {
        return None;
    };
    if !expression.parentheses.is_empty() || matches!(value.kind, ExpressionKind::Assign { .. }) {
        return None;
    }
    Some((target.name.as_str(), value))
}

/// The position of the bare slot `name` in `slots`.
fn bare_slot(slots: &[SlotDeclaration], name: &str) -> Option<usize> {
    slots.iter().position(|slot| {
        slot.name.name == name
            && slot.initializer.is_none()
            && slot.mutability == SlotMutability::MutableBare
    })
}

/// Whether `value` can be evaluated earlier than where it stands without
/// anyone seeing the difference. Task 4 widens it; at the first statement
/// nothing is evaluated earlier, so no value needs it there.
fn settled(_value: &Expression, _scope: &Scope<'_>) -> bool {
    false
}

fn in_place(text: &str, slot: &SlotDeclaration, statement: &Statement, value: &Expression) -> Vec<Edit> {
    let end = edits::declaration_end(slot);
    vec![Edit {
        span: Span::new(end, end),
        text: edits::initializer(text, value, statement.span.start, slot.span.start),
    }]
}

/// The slot at `position` moved, with its initializer, after the slot at
/// `last`, the last initialized one; `None` when a comment stands in what
/// the move deletes.
fn reordered(
    text: &str,
    slots: &[SlotDeclaration],
    position: usize,
    last: usize,
    statement: &Statement,
    value: &Expression,
) -> Option<Vec<Edit>> {
    let slot = &slots[position];
    let following = &slots[position + 1];
    let separator = &text[edits::declaration_end(slot) as usize..following.span.start as usize];
    let removal = if position == 0 {
        Span::new(slot.span.start, following.span.start)
    } else {
        Span::new(edits::declaration_end(&slots[position - 1]), edits::declaration_end(slot))
    };
    if edits::holds_a_comment(&text[removal.start as usize..removal.end as usize]) {
        return None;
    }
    let destination = slots[last].span.end;
    let declaration = format!(
        "{separator}{}{}",
        &text[slot.span.start as usize..edits::declaration_end(slot) as usize],
        edits::initializer(text, value, statement.span.start, slots[last].span.start)
    );
    Some(vec![
        Edit {
            span: removal,
            text: String::new(),
        },
        Edit {
            span: Span::new(destination, destination),
            text: declaration,
        },
    ])
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-syntax hints`
Expected: all PASS. If `a_one_line_section_takes_the_initializer_inline` or `carriage_returns_survive_a_fix` fails, inspect `delete_statement`'s span arithmetic against the spec's "its line when that leaves the line blank" -- the expectation is the spec's.

- [ ] **Step 5: Gates, then commit**

```bash
git add crates/ns-syntax/src/hints
git commit -m "feat: find the bare slots a first statement initializes, with their fixes" # plus the attribution lines
```

---

### Task 4: Hint A after other statements, for settled values

**Files:**
- Modify: `crates/ns-syntax/src/hints/initialize.rs` (`settled`)
- Test: `crates/ns-syntax/src/hints/mod.rs`

**Interfaces:**
- Consumes: `walk::{Scope, Binding}` (`Scope::binding(&str) -> Option<Binding>`)

- [ ] **Step 1: Write the failing tests**

```rust
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
    fn a_factory_parameter_is_settled_in_a_header() {
        let text = indoc! {"
            class Module withLimit: limit = (
              |
                used ::= 0.
                cap
              |
              used printString.
              cap:: limit.
            ) ()
        "};
        assert_eq!(names_of(text, HintKind::InitializeInDeclaration), ["cap"]);
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-syntax hints`
Expected: `a_settled_value_after_a_guard_moves_into_its_declaration` and `a_factory_parameter_is_settled_in_a_header` FAIL; the other two already PASS.

- [ ] **Step 3: Implement `settled`**

Replace `settled` in `initialize.rs` (and add `use super::walk::Binding;`):

```rust
/// Whether `value` can be evaluated earlier than where it stands without
/// anyone seeing the difference: a literal, `self`, or a name bound to a
/// parameter or a read-only local, none of which can fail or change. A read-only
/// local of the same section is initialized before the moved slot, since
/// the slot then stands after the last initialized one.
fn settled(value: &Expression, scope: &Scope<'_>) -> bool {
    match &value.kind {
        ExpressionKind::Literal(_) => true,
        ExpressionKind::Identifier(identifier) => {
            identifier.name == "self"
                || matches!(
                    scope.binding(&identifier.name),
                    Some(Binding::Parameter | Binding::ReadOnlyLocal)
                )
        }
        _ => false,
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-syntax hints`
Expected: all PASS.

- [ ] **Step 5: Gates, then commit**

```bash
git add crates/ns-syntax/src/hints
git commit -m "feat: move settled values after a guard into their declarations" # plus the attribution lines
```

---

### Task 5: Apply all, and the core library without hints

**Files:**
- Modify: `crates/ns-syntax/src/hints/mod.rs` (`apply_all`)
- Create: `crates/ns-analyzer/tests/hint_gate.rs`
- Modify: `crates/ns-runtime/core-library/**/*.ns`, `crates/ns-runtime/tests/fixtures/golden-image.bin`
- Test: `crates/ns-syntax/src/hints/mod.rs`, `crates/ns-analyzer/tests/hint_gate.rs`

**Interfaces:**
- Consumes: `hints_of`, `apply`, `edits::overlap`
- Produces: `ns_syntax::hints::apply_all(text: &str) -> String`

- [ ] **Step 1: Write the failing tests** (in `mod.rs`'s tests)

```rust
    #[test]
    fn apply_all_moves_a_run_of_assignments_and_then_makes_them_read_only() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  activation
                  count
                  handlers
                |
                count:: 0.
                activation:: self start.
                ^count + activation
              )
            )
        "};
        let result = apply_all(text);
        assert_eq!(
            result,
            indoc! {"
                class Module = () (
                  public run = (
                    |
                      count = 0.
                      activation = self start.
                      handlers
                    |
                    ^count + activation
                  )
                )
            "}
        );
        assert_eq!(apply_all(&result), result, "a second pass changes nothing");
    }

    /// The outer statement moves first; the closure's own hint, whose
    /// edits it would carry along, waits for the next round.
    #[test]
    fn apply_all_reaches_the_hints_inside_a_moved_value() {
        let text = indoc! {"
            class Module = () (
              public run = (
                |
                  action
                |
                action:: [
                  |
                    count
                  |
                  count:: 1.
                  count
                ].
                ^action value
              )
            )
        "};
        let result = apply_all(text);
        assert!(crate::parse(&result).is_ok(), "{result}");
        assert!(hints_of(&result).is_empty(), "{result}");
        assert!(result.contains("count = 1."), "{result}");
        assert!(result.contains("action = ["), "{result}");
    }

    #[test]
    fn apply_all_leaves_a_source_without_hints_alone() {
        let text = "class Module = () ( public run = ( ^1 ) )\n";
        assert_eq!(apply_all(text), text);
        assert_eq!(apply_all("class Module = ("), "class Module = (");
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-syntax hints::tests::apply_all`
Expected: FAIL to compile -- `apply_all` does not exist.

- [ ] **Step 3: Implement `apply_all`** (in `mod.rs`)

```rust
/// `text` with every hint applied, or `text` itself when it does not
/// parse. It works in rounds: each applies at most one hint per section,
/// and none whose edits overlap another's -- a moved statement may hold a
/// closure with hints of its own, whose edits the move would carry along
/// -- then finds the hints of the result. Every fix is valid on its own,
/// so every round keeps the behaviour, and each removes an assignment or
/// a `::=`, so the rounds end.
pub fn apply_all(text: &str) -> String {
    let mut current = text.to_string();
    loop {
        let found = hints_of(&current);
        if found.is_empty() {
            return current;
        }
        let mut taken: Vec<&Hint> = Vec::new();
        for hint in &found {
            let clashes = taken.iter().any(|other| {
                other.section == hint.section
                    || other
                        .edits
                        .iter()
                        .any(|edit| hint.edits.iter().any(|mine| edits::overlap(edit, mine)))
            });
            if !clashes {
                taken.push(hint);
            }
        }
        let round: Vec<Edit> = taken
            .iter()
            .flat_map(|hint| hint.edits.iter().cloned())
            .collect();
        let next = apply(&current, &round);
        debug_assert!(crate::parse(&next).is_ok(), "a round of fixes does not parse:\n{next}");
        current = next;
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-syntax hints`
Expected: all PASS.

- [ ] **Step 5: Write the core-library gate** (`crates/ns-analyzer/tests/hint_gate.rs`)

```rust
//! The shipped core library has no hint: every slot it can initialize in
//! its declaration is, and every slot that can be read-only is. Only the
//! core library is held to it; goldens pin a program's form along with
//! its behaviour, and the benchmarks stay close to the programs they
//! adapt.

const CORE_LIBRARY: &str = "crates/ns-runtime/core-library";

fn core_library_sources() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    ns_tooling::source::collect_ns_files(&root.join(CORE_LIBRARY), &mut files);
    files.sort();
    assert!(files.len() >= 7, "the gate found only {} sources", files.len());
    files
}

#[test]
fn the_core_library_has_no_hint() {
    let mut hinted = Vec::new();
    for file in core_library_sources() {
        let source = std::fs::read_to_string(&file).expect("a readable source");
        for hint in ns_syntax::hints::hints_of(&source) {
            let line = source[..hint.span.start as usize].matches('\n').count() + 1;
            hinted.push(format!("{}:{line}: {}", file.display(), hint.message()));
        }
    }
    assert!(
        hinted.is_empty(),
        "run `cargo nextest run -p ns-analyzer --run-ignored only apply_every_hint_to_the_core_library`:\n{}",
        hinted.join("\n")
    );
}

/// Rewrites the core library with every hint applied, in the formatter's
/// layout. Run deliberately, then regenerate the image fixture: its header
/// carries the fingerprint of these sources.
#[test]
#[ignore = "rewrites crates/ns-runtime/core-library; run deliberately"]
fn apply_every_hint_to_the_core_library() {
    for file in core_library_sources() {
        let source = std::fs::read_to_string(&file).expect("a readable source");
        let applied = ns_syntax::hints::apply_all(&source);
        let formatted = ns_syntax::format(&applied).unwrap_or(applied);
        if formatted != source {
            std::fs::write(&file, formatted).expect("write the source");
        }
    }
}
```

- [ ] **Step 6: Run the gate to see what the core library still has**

Run: `cargo nextest run -p ns-analyzer --test hint_gate`
Expected: FAIL, listing the hints left after the earlier manual pass (private header slots, `settled` cases).

- [ ] **Step 7: Bring the core library to no hint**

```bash
cargo nextest run -p ns-analyzer --run-ignored only apply_every_hint_to_the_core_library
cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library
git diff --stat crates/ns-runtime/core-library
cargo nextest run -p ns-runtime --run-ignored only regenerate_the_golden_fixture
cargo nextest run --workspace
```

Read the core-library diff before going on: every change is a slot taking its value or `=`. Expected: the gate PASSES and the whole suite PASSES. A failing golden here is a wrong fix -- stop and find the rule that allowed it; never change a golden.

- [ ] **Step 8: Gates, then commit**

```bash
git add crates/ns-syntax/src/hints crates/ns-analyzer/tests/hint_gate.rs crates/ns-runtime/core-library crates/ns-runtime/tests/fixtures/golden-image.bin
git commit -m "feat: apply every hint of a source; keep the core library free of hints" # plus the attribution lines
```

---

### Task 6: The behaviour oracle over every golden

**Files:**
- Modify: `crates/ns-cli/tests/common/mod.rs` (add `compare`, `normalize_newlines`), `crates/ns-cli/tests/run_goldens.rs` (use them), `crates/ns-cli/Cargo.toml`
- Create: `crates/ns-cli/tests/hinted_goldens.rs`

**Interfaces:**
- Consumes: `ns_syntax::hints::{apply_all, hints_of}`, `common::{golden_programs, golden_name, expectation_of, program_arguments, ScratchDirectory}`
- Produces: `common::compare(output: &std::process::Output, expectation: &Expectation, condition: &str) -> Result<(), String>`

- [ ] **Step 1: Move `compare` and `normalize_newlines` into `common/mod.rs`**

Cut both functions from `run_goldens.rs` into `common/mod.rs`, unchanged in body except the return type: `compare` returns `Result<(), String>` (drop each `.into()` after `format!(...)`). In `run_goldens.rs`, the two callers become `common::compare(&output, &expectation, ...).map_err(Failed::from)` and the uses of `normalize_newlines` become `common::normalize_newlines`.

Run: `cargo nextest run -p ns-cli --test run_goldens`
Expected: PASS, as before.

- [ ] **Step 2: Register the oracle** -- in `crates/ns-cli/Cargo.toml`, after the `golden_round_trips` entry:

```toml
# One test per golden whose source the hints change, as `run_goldens`.
[[test]]
name = "hinted_goldens"
harness = false
```

- [ ] **Step 3: Write `hinted_goldens.rs`**

```rust
//! Every golden with every hint applied answers exactly what its `.toml`
//! records, under both lowerings: the hints' fixes change no program's
//! behaviour (`docs/design/editor-support.md`, "Hints"). The goldens
//! themselves stay as they are written; the rewritten program runs from a
//! scratch copy. Every golden's sources also reach a fixed point: the
//! result parses and a second pass changes nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

use libtest_mimic::{Arguments, Failed, Trial};

mod common;

const LOWERINGS: [&str; 2] = ["optimized", "reference"];

/// The `.ns` files of a golden program: the file itself, or every one in a
/// project's directory.
fn sources_of(program: &Path) -> Vec<PathBuf> {
    if program.is_dir() {
        let mut files = Vec::new();
        ns_tooling::source::collect_ns_files(program, &mut files);
        files.sort();
        files
    } else {
        vec![program.to_path_buf()]
    }
}

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    for program in common::golden_programs() {
        let name = common::golden_name(&program);
        let sources = sources_of(&program);
        let changed = sources.iter().any(|source| {
            let text = std::fs::read_to_string(source).expect("a readable source");
            ns_syntax::hints::apply_all(&text) != text
        });
        let checked = sources.clone();
        trials.push(Trial::test(format!("fixed_point::{name}"), move || {
            reaches_a_fixed_point(&checked)
        }));
        if !changed || common::expectation_of(&program).skipped().is_some() {
            continue;
        }
        for lowering in LOWERINGS {
            let program = program.clone();
            trials.push(Trial::test(format!("hinted::{lowering}::{name}"), move || {
                runs_alike(&program, lowering)
            }));
        }
    }
    libtest_mimic::run(&arguments, trials).exit();
}

fn reaches_a_fixed_point(sources: &[PathBuf]) -> Result<(), Failed> {
    for source in sources {
        let text = std::fs::read_to_string(source).expect("a readable source");
        if ns_syntax::parse(&text).is_err() {
            continue;
        }
        let applied = ns_syntax::hints::apply_all(&text);
        if ns_syntax::parse(&applied).is_err() {
            return Err(format!("{}: the hinted source does not parse:\n{applied}", source.display()).into());
        }
        if !ns_syntax::hints::hints_of(&applied).is_empty() {
            return Err(format!("{}: hints are left after applying every hint", source.display()).into());
        }
    }
    Ok(())
}

/// Copies `program` into a scratch directory with every hint applied to its
/// sources, runs it under `lowering`, and compares with the recorded outcome.
fn runs_alike(program: &Path, lowering: &str) -> Result<(), Failed> {
    let scratch = common::ScratchDirectory::new("hinted");
    let file_name = program.file_name().expect("a golden has a name");
    let copy = scratch.join(&file_name.to_string_lossy());
    if program.is_dir() {
        copy_directory(program, &copy);
    } else {
        std::fs::copy(program, &copy).expect("copy the golden");
    }
    for source in sources_of(&copy) {
        let text = std::fs::read_to_string(&source).expect("a readable source");
        std::fs::write(&source, ns_syntax::hints::apply_all(&text)).expect("write the hinted source");
    }
    let expectation = common::expectation_of(program);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(&copy)
        .arg("--lowering")
        .arg(lowering)
        .args(common::program_arguments(program))
        .output()
        .map_err(|error| format!("run ns-next: {error}"))?;
    common::compare(&output, &expectation, &format!("hinted, under the {lowering} lowering"))
        .map_err(Failed::from)
}

fn copy_directory(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create the copy");
    for entry in std::fs::read_dir(from).expect("read the golden").flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_directory(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy a file");
        }
    }
}
```

- [ ] **Step 4: Run the oracle**

Run: `cargo nextest run -p ns-cli --test hinted_goldens`
Expected: PASS -- every `fixed_point::` trial and every `hinted::` trial. A failing `hinted::` trial is a wrong fix: find the rule that allowed it, add a unit test in `crates/ns-syntax/src/hints/mod.rs` that reproduces it, fix the rule. Never change a golden.

- [ ] **Step 5: Gates, then commit**

```bash
git add crates/ns-cli
git commit -m "test: hold every golden with every hint applied to its recorded outcome" # plus the attribution lines
```

---

### Task 7: Hints in the language server

**Files:**
- Modify: `crates/ns-analyzer/src/analysis.rs`, `crates/ns-analyzer/src/server.rs`, `crates/ns-analyzer/src/lib.rs` (`pub mod code_actions;` in the alphabetical list, after `pub mod analysis;`)
- Create: `crates/ns-analyzer/src/code_actions.rs`
- Test: `crates/ns-analyzer/tests/protocol.rs`

**Interfaces:**
- Consumes: `ns_syntax::hints::{Hint, hints_of, apply_all}`, `crate::documents::LineIndex::position(&self, text: &str, offset: u32) -> Position`
- Produces: `Analysis::hints: Vec<Hint>`; `code_actions::diagnostic(hint: &Hint, text: &str, line_index: &LineIndex) -> Diagnostic`; `code_actions::actions(uri: &Uri, hints: &[Hint], text: &str, line_index: &LineIndex, range: Range) -> Vec<CodeActionOrCommand>`

- [ ] **Step 1: Write the failing protocol test** (append to `tests/protocol.rs`)

```rust
#[test]
fn a_hint_is_published_with_its_quick_fix_and_the_fix_for_the_file() {
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
    let (client, server_thread) = initialized_server();
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "newspeak", "version": 1, "text": text } }),
    );
    let published = next_diagnostics(&client);
    let diagnostics = published["diagnostics"].as_array().expect("an array");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["severity"], 4, "a hint");
    assert_eq!(diagnostics[0]["code"], "initialize-in-declaration");
    assert_eq!(diagnostics[0]["source"], "ns-next");
    assert_eq!(
        diagnostics[0]["range"],
        json!({ "start": { "line": 3, "character": 6 }, "end": { "line": 3, "character": 11 } })
    );

    let actions = request(
        &client,
        2,
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": URI },
            "range": { "start": { "line": 3, "character": 7 }, "end": { "line": 3, "character": 7 } },
            "context": { "diagnostics": diagnostics },
        }),
    );
    let actions = actions.as_array().expect("an array of actions");
    let titles: Vec<&str> = actions
        .iter()
        .map(|action| action["title"].as_str().expect("a title"))
        .collect();
    assert_eq!(titles, ["Initialize `count` in its declaration", "Apply all hints in this file"]);
    assert_eq!(actions[0]["kind"], "quickfix");
    let edits = actions[0]["edit"]["changes"][URI].as_array().expect("edits for the document");
    assert!(
        edits.iter().any(|edit| edit["newText"] == " ::= 1."),
        "{edits:?}"
    );

    let elsewhere = request(
        &client,
        3,
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": URI },
            "range": { "start": { "line": 6, "character": 4 }, "end": { "line": 6, "character": 4 } },
            "context": { "diagnostics": [] },
        }),
    );
    assert_eq!(elsewhere, json!([]), "no action away from a hint");
    shut_down(&client, server_thread);
}
```

In `an_editor_session`, after the `documentFormattingProvider` assertion, add:

```rust
    assert_eq!(capabilities["capabilities"]["codeActionProvider"], true);
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-analyzer --test protocol`
Expected: FAIL -- no hint diagnostic, no `codeActionProvider`.

- [ ] **Step 3: Add `Analysis::hints`**

In `analysis.rs`: add `pub hints: Vec<ns_syntax::hints::Hint>,` to `Analysis` (after `symbols`); in `guarded`'s fallback add `hints: Vec::new(),`; in `analyze_unguarded` compute, before building `Analysis`:

```rust
    // Only a source that parses without error gets hints: their fixes are
    // made against the tree `parse` answers.
    let hints = ns_syntax::hints::hints_of(text);
```

and add `hints,` to the struct literal.

- [ ] **Step 4: Write `code_actions.rs`**

```rust
//! A document's hints as LSP diagnostics and code actions: the quick fix
//! of each hint the requested range touches, and beside them one action
//! that applies every hint of the file. The edits are plain `changes`:
//! with full synchronisation the server always holds the text the client
//! asks about.

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, DiagnosticSeverity,
    NumberOrString, Position, Range, TextEdit, Uri, WorkspaceEdit,
};
use ns_syntax::hints::Hint;
use ns_syntax::Span;

use crate::documents::LineIndex;

fn range_of(span: Span, text: &str, line_index: &LineIndex) -> Range {
    Range::new(
        line_index.position(text, span.start),
        line_index.position(text, span.end),
    )
}

/// `hint` as the diagnostic the editor shows at the slot's name: a hint,
/// the weakest severity, which IntelliJ shows as a weak warning.
pub fn diagnostic(hint: &Hint, text: &str, line_index: &LineIndex) -> Diagnostic {
    Diagnostic {
        range: range_of(hint.span, text, line_index),
        severity: Some(DiagnosticSeverity::HINT),
        code: Some(NumberOrString::String(hint.code().to_string())),
        source: Some("ns-next".to_string()),
        message: hint.message(),
        ..Diagnostic::default()
    }
}

fn ordered(position: Position) -> (u32, u32) {
    (position.line, position.character)
}

fn touches(first: Range, second: Range) -> bool {
    ordered(first.start) <= ordered(second.end) && ordered(second.start) <= ordered(first.end)
}

fn workspace_edit(uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(HashMap::from([(uri.clone(), edits)])),
        ..WorkspaceEdit::default()
    }
}

/// The quick fixes of the hints `range` touches, then the fix for the
/// whole file; nothing when `range` touches no hint.
pub fn actions(
    uri: &Uri,
    hints: &[Hint],
    text: &str,
    line_index: &LineIndex,
    range: Range,
) -> Vec<CodeActionOrCommand> {
    let touched: Vec<&Hint> = hints
        .iter()
        .filter(|hint| touches(range_of(hint.span, text, line_index), range))
        .collect();
    if touched.is_empty() {
        return Vec::new();
    }
    let mut actions: Vec<CodeActionOrCommand> = touched
        .iter()
        .map(|hint| {
            let edits = hint
                .edits
                .iter()
                .map(|edit| TextEdit {
                    range: range_of(edit.span, text, line_index),
                    new_text: edit.text.clone(),
                })
                .collect();
            CodeActionOrCommand::CodeAction(CodeAction {
                title: hint.title(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diagnostic(hint, text, line_index)]),
                edit: Some(workspace_edit(uri, edits)),
                is_preferred: Some(true),
                ..CodeAction::default()
            })
        })
        .collect();
    let applied = ns_syntax::hints::apply_all(text);
    let changed = changed_span(text, &applied);
    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Apply all hints in this file".to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(
            touched
                .iter()
                .map(|hint| diagnostic(hint, text, line_index))
                .collect(),
        ),
        edit: Some(workspace_edit(
            uri,
            vec![TextEdit {
                range: range_of(changed.0, text, line_index),
                new_text: changed.1,
            }],
        )),
        ..CodeAction::default()
    }));
    actions
}

/// The smallest span of `before` whose replacement gives `after`, and that
/// replacement: the two texts with their common prefix and suffix left
/// out, both cut on character boundaries.
fn changed_span(before: &str, after: &str) -> (Span, String) {
    let mut prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(first, second)| first == second)
        .count();
    while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = before[prefix..]
        .bytes()
        .rev()
        .zip(after[prefix..].bytes().rev())
        .take_while(|(first, second)| first == second)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    (
        Span::new(prefix as u32, (before.len() - suffix) as u32),
        after[prefix..after.len() - suffix].to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::changed_span;
    use ns_syntax::Span;

    #[test]
    fn the_changed_span_leaves_out_the_common_prefix_and_suffix() {
        assert_eq!(
            changed_span("a count b", "a count ::= 1. b"),
            (Span::new(8, 8), "::= 1. ".to_string())
        );
        assert_eq!(changed_span("same", "same"), (Span::new(4, 4), String::new()));
        // `ü` and `ß` are two bytes each: the change is `ß` (bytes 4..6).
        assert_eq!(changed_span("Grüße", "Grüne"), (Span::new(4, 6), "n".to_string()));
    }
}
```

Note on the first assertion in that test: "a count b" and "a count ::= 1. b" share the prefix "a count " (8 bytes) and then the suffix "b" (1 byte); the replacement is "::= 1. ".

- [ ] **Step 5: Wire the server** (`server.rs`)

- Imports: add `CodeActionRequest` to the `lsp_types::request` list; add `CodeActionParams`, `CodeActionProviderCapability` to the `lsp_types` list; `use crate::code_actions;`.
- `capabilities()`: add `code_action_provider: Some(CodeActionProviderCapability::Simple(true)),`.
- `answer`: add `CodeActionRequest::METHOD => code_actions_of(documents, request),`.
- `diagnostics_of` becomes (the findings mapped as before, the hints after them):

```rust
/// The diagnostics to publish for `uri`: its findings and its hints, or
/// none once closed.
fn diagnostics_of(uri: &Uri, document: Option<&Document>) -> PublishDiagnosticsParams {
    let diagnostics = document.map_or_else(Vec::new, |document| {
        let mut diagnostics: Vec<Diagnostic> = document
            .analysis
            .findings
            .iter()
            .map(|finding| Diagnostic {
                range: Range::new(
                    document
                        .line_index
                        .position(&document.text, finding.span.start),
                    document
                        .line_index
                        .position(&document.text, finding.span.end),
                ),
                severity: Some(match finding.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                }),
                source: Some("ns-next".to_string()),
                message: finding.message.clone(),
                ..Diagnostic::default()
            })
            .collect();
        diagnostics.extend(document.analysis.hints.iter().map(|hint| {
            code_actions::diagnostic(hint, &document.text, &document.line_index)
        }));
        diagnostics
    });
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    }
}
```

- New handler, beside `folding_ranges_of`:

```rust
/// The code actions at a range of an open document: its hints' quick fixes
/// and the fix for the whole file. None for a document never opened, as
/// for its folds.
fn code_actions_of(documents: &HashMap<String, Document>, request: Request) -> Response {
    let params: CodeActionParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            )
        }
    };
    let uri = &params.text_document.uri;
    let actions = documents
        .get(uri.as_str())
        .map(|document| {
            code_actions::actions(
                uri,
                &document.analysis.hints,
                &document.text,
                &document.line_index,
                params.range,
            )
        })
        .unwrap_or_default();
    Response::new_ok(request.id, actions)
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p ns-analyzer`
Expected: all PASS, the existing protocol tests included (a document with an error has no hint, so their diagnostic counts are unchanged; if `an_editor_session`'s `folded` module gains a hint, its diagnostics assertions read only the error count -- check and keep them exact).

- [ ] **Step 7: Gates, then commit**

```bash
git add crates/ns-analyzer
git commit -m "feat: publish the hints and answer their quick fixes in the language server" # plus the attribution lines
```

---

### Task 8: Documentation

**Files:**
- Modify: `docs/design/editor-support.md`, `CLAUDE.md`
- Delete: `docs/superpowers/specs/2026-10-14-editor-hints-design.md`

- [ ] **Step 1: `editor-support.md`**
  - In the feature list (the numbered list near the top, after "Folding"), add an item: **Hints** -- a slot whose first value a statement assigns, and a slot nothing assigns again, marked as weak warnings with a quick fix each and one for the whole file ("Hints").
  - Add a section `### Hints` after `### Folding`, carrying the spec's "Goal", "The rules" (both hints, each rule with its reason), "The server" and the tests (the unit tests, the oracle `crates/ns-cli/tests/hinted_goldens.rs`, the protocol test, the gate `crates/ns-analyzer/tests/hint_gate.rs` and why only the core library is held to it). Present tense, the current state only.
  - In the module list where `folding.rs` is described, add `code_actions.rs` -- the hints as diagnostics and code actions ("Hints").
  - In the list of protocol tests, add: hints -- a hint published with its quick fix and the fix for the file, nothing away from a hint.
  - Record the check by hand under `runIde` (Step 3) as the Folding section records its own.

- [ ] **Step 2: `CLAUDE.md`**
  - `crates/ns-syntax`: after the printer's sentence, add: the hints (`hints/`: a slot that can take its first value in its declaration, a slot that can be read-only, each with its fix; `apply_all`) (`docs/design/editor-support.md`, "Hints").
  - `crates/ns-analyzer`: in `lsp`'s list, after "formatting", add "hints and their quick fixes".

- [ ] **Step 3: The check by hand** -- ask the user to run `./editors/jetbrains/gradlew -p editors/jetbrains runIde`, open a module with a bare local assigned by its first statement, and confirm: the name shows a weak warning; Alt+Enter offers "Initialize `x` in its declaration" and "Apply all hints in this file"; each does what its title says. Write the date and the outcome into the section.

- [ ] **Step 4: Delete the spec** -- `git rm docs/superpowers/specs/2026-10-14-editor-hints-design.md` (its content now lives in `editor-support.md`).

- [ ] **Step 5: Commit** (Markdown only: no gates, `git diff --stat` to confirm)

```bash
git add docs/design/editor-support.md CLAUDE.md
git commit -m "docs: describe the editor hints" # plus the attribution lines
```

After this task: the `finishing-a-batch` skill (the end-of-batch gates, the whole-branch review, the merge).
