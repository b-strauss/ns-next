# Formatter — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A lossless AST in `ns-syntax` (everything but layout), a pretty
printer over it with fixed rules and no options, `ns-next-analyzer fmt` and
the language server's `textDocument/formatting` on top of it, and every
Newspeak source of the repository outside `corpus/` held to its output.

**Architecture:** The AST gains what it drops today — type annotations,
parentheses, `<-:`, literal spellings, the prologue, the source order of
members, and every comment attached to a node — without changing what any
existing consumer sees (`Expression` becomes an envelope whose `kind` is
today's enum). A printer in `crates/ns-syntax/src/printer/` lowers the AST
to a Wadler/Prettier document and lays it out at 100 columns, two spaces per
level. The analyzer exposes it as a command and an LSP request; a test holds
the repository's sources to it.

**Tech Stack:** Rust 1.98.0 (`crates/ns-syntax`, `crates/ns-runtime`,
`crates/ns-analyzer`, `crates/ns-cli` tests), Newspeak (the reformatted
sources), Kotlin/Gradle only as a gate (`editors/jetbrains`, no code change).

**Spec:** none on file. The design was agreed with the user on 2026-10-11
and 2026-10-12 and is recorded here in full ("The design" below); the
living documents receive it in Task 13.

## The design

Decided with the user. Numbers 1–25 are the style rules; "Architecture"
records the four design sections.

### Purpose

- A tool (editor *Reformat Code*, `ns-next-analyzer fmt`) **and a repository
  gate**: every `.ns` outside `corpus/` and `prior-art/` is a fixed point of
  the formatter, enforced by a test.
- A **full pretty printer with fixed rules and no options**. Request options
  (`tabSize`, `insertSpaces`) are ignored.
- The AST must be **lossless except for layout** because the runtime will
  later generate source from ASTs it builds or edits; the printer therefore
  lives in `ns-syntax`, which the runtime reaches.

### Style rules

1. **Width 100 columns, two spaces per indentation level**, for Newspeak only.
   A column is one `char`. Code is never indented with tabs.
2. **Prologue as written.** `Newspeak3` and the category string each on their
   own line before the class when present, nothing added when absent, no
   blank line after them.
3. **Class header.** `[access] class Name [type parameters] [factory] = [superclass [message]] [<: mixin …] (`
   on one line when it fits. Otherwise, in this order until it fits: break
   before `=` (the `= Superclass … (` continuation at +4); then put every
   factory keyword on its own line at +4 as well:
   ```
   class NewspeakPrettyPrinter
       usingPlatform: platform
       ast: astModule
       parser: commonParser
       metadata: mdModule
       = Object new (
   ```
   A long `<:` chain breaks before each `<:`, at +4.
4. **Class comment** (the first comment in the header) on its own lines at
   +2 right after the header's `(`, then the slots. A comment above `class`
   stays above it, directly, no blank line. The formatter never moves a
   comment from one owner to another.
5. **Slots.** Opening and closing bar each on their own line at +2, one slot
   per line at +4; `||` and access modifiers as written; an initializer
   breaks by the expression rules relative to the slot. An empty slot list
   (`| |`, `|| ||`) is dropped. No alignment of `=`/`::=`. Never one line,
   however short.
6. **Periods.** In a multi-line sequence (method body, multi-line block,
   header initializer expressions, multi-line tuple) a period after every
   element, the last included. A one-line block or tuple has none after its
   last element. Header initializer expressions: one per line at +2 after
   the closing bar.
7. **Members** in source order (no reordering). Exactly one blank line
   between the members of a class side; no blank line anywhere else — not
   between `) (` and the first member, not before `) : (`, not inside
   bodies. A member's leading comment sits directly above it; the blank line
   goes above the comment.
8. **Empty parts.** An empty header or empty instance side is `()` inline
   (`class Top = () ()`; `class Foo = () (` with members below). An empty
   class side `: ()` is dropped. A part holding only a comment is not empty:
   it is opened up with the comment on its own line at +2.
9. **Method header** `[access] pattern [^<Type>] = (` on one line when it
   fits; otherwise the first keyword stays on the line and every further
   keyword goes on its own line at +4, ` = (` after the last:
   ```
     public copyReplaceFrom: start
         to: stop
         with: replacement
         startingAt: replacementStart
         ifAbsent: absentBlock = (
   ```
10. **Method body** always multi-line (`= (` … `)`), even for one statement.
    An empty body is `= ()` inline. **Temporaries always opened up**, in
    methods and blocks alike: each bar on its own line at the body's
    indentation, one temporary per line one level (two spaces) deeper, no
    blank line after:
    ```
      public countUpTo: limit = (
        |
          current
        |
        current:: 0.
      )
    ```
11. **Statements** one per line; `^count` without a space; a statement
    ending in a multi-line block puts its period right after the `]` (`].`).
12. **Breaking in general.** Outside in: a statement too long breaks at its
    outermost (lowest-precedence) level first — cascade, keyword, binary,
    unary — and only a part still too long breaks inside. All or nothing:
    once a level breaks, every link of that level is on its own line.
    Continuation lines at +4 relative to the line the expression starts on.
    Binary chains break before the operator (operator leads the line):
    ```
      message:: 'expected '
          , expected printString
          , ' but got '
          , actual printString.
    ```
    Unary chains likewise, one selector per line at +4.
13. **Keyword messages.** First keyword on the receiver's line, every further
    keyword on its own line at +4 when the message breaks. **A block
    argument does not break its message by itself**: a multi-line block keeps
    `[` at the end of the line, its body at +2 relative to the statement, `]`
    at the statement's indentation, and the next keyword follows the `]` on
    the same line. Only when the parts outside the block bodies do not fit
    does the message break by the keyword rule. When a message is laid out
    this way, every block argument of it is multi-line (as in the example,
    where `[count:: count + 1]` alone would fit): the blocks of one message
    break together, all or nothing.
    ```
      count > limit ifTrue: [
        self reset.
        ^nil.
      ] ifFalse: [
        count:: count + 1.
      ].
    ```
14. **Cascades** always broken: the receiver alone on the first line, each
    message on its own line at +4, `;` at the end of each line but the last.
15. **Blocks.** No inner spaces (`[x]`, `[:each | each foo]`, `[]`,
    `[:each |]`). One line only with at most one statement, no temporaries,
    and when it fits; otherwise multi-line with the parameters on the `[`
    line. A block that is not a keyword argument (a receiver, an assigned
    value) follows the same shape: `[` at the end of the line, body +2, `]`
    at the statement's indentation.
16. **Parentheses as written**, redundant ones included; no inner spaces.
17. **Tuples** like blocks: one line when it fits (`{a. b. c}`, `{}`, no
    period after the last), otherwise `{` at the end of the line, one element
    per line at +2, a period after each, `}` at the statement's indentation.
18. **Literals exactly as written** (numbers, strings, symbols, characters).
    A multi-line string is never re-indented, its line breaks kept byte for
    byte; a literal is never broken.
19. **Setters** `count:: value` (no space before `::`); slot initializers
    `name = value`, `name ::= value`. Never break right after `::` or `=`.
20. **Type annotations.** One space before `<`, none inside: `name <String>`,
    `at: index <Integer>`, `size ^<Integer> = (`; inside a type: `List[String]`,
    `Map[K, V]` (space after the comma), `String | Nil` (spaces around `|`),
    `[:Integer | Boolean]`. A type is never broken; it stays with its name.
    The spelling inside a type is kept; only these spaces are normalised.
21. **Comments.** Attached to nodes: *leading* (own lines, belongs to the
    following node, moves with it), *trailing* (after code on the same line,
    belongs to that construct), *dangling* (after the last element before a
    closer, belongs to the container, printed at the content's indentation).
    The form is kept (`///` stays `///`, `(* … *)` stays `(* … *)`). A `///`
    block is re-indented line by line, its text after `///` kept exactly. A
    multi-line `(* … *)` moves its continuation lines by the same amount as
    its `(*`; a line with less indentation than is taken away starts at
    column 0. Metadata comments (`(* :exemplar: … *)`) are comments. A
    trailing comment gets one space before it, is never aligned, and **does
    not count toward the width**; a `///` or a line-ending comment inside an
    expression forces the broken form of the level it stands in and stays
    after the token it followed. An inline `(* … *)` between two tokens stays
    there with one space on each side and counts toward the width.
22. **An unbreakable long line stays long**, without a message; the gate
    checks only that `fmt` changes nothing.
23. **File level.** The **line ending is taken from the file**: the first
    line ending of the input decides every line ending of the output (LF
    when the input has none). Line breaks inside a string literal stay byte
    for byte. No trailing whitespace on any line (also comment lines; never
    inside a string). Exactly one line ending at the end of the file. No tabs
    in indentation.
24. **Other declarations.** A lazy slot is a member like a slot, on its own
    line, `lazy` and the access modifier in written order. A mixin
    application without a body (`class Combined = Base <: Mixin.`) is one line,
    breaking before each `<:` at +4 when too long. Nested classes follow the
    same rules, indented.
25. **Spacing between tokens.** One space around a binary operator, one after
    a keyword, none before `.` or `;`, `receiver <-: message`.

### Architecture

**Section 1 — the lossless AST (`ns-syntax`).** Reprinting a module from its
AST alone, without the source, yields source that parses back to the same AST
(positions aside). Three things carry no content and are normalised by the
parser rather than kept: an empty slot list, an empty class side, and the
period after the last statement. What is added:

| Information | Today | New |
|---|---|---|
| Prologue | `language_id`, `category` as `String`, empty when absent | `Option`s; the category keeps its spelling |
| Literals | numbers keep text; strings unescaped; symbols without `#`/quotes | every literal keeps its source text as a field |
| `<-:` | swallowed | `MessageSend::eventual`; the runtime still ignores it (the documented §5.6 deviation is unchanged) |
| Type annotations | skipped | `TypeAnnotation { tokens }` at every place the parser skips one today |
| Parentheses | vanish | recorded on the expression envelope |
| Member order | three separate lists | one ordered `members`, with accessors answering today's per-kind order |
| Comments | only the class comment; the class side's discarded | every comment attached to a node |
| Implicit receiver | synthetic `self` with a zero-width span | an explicit `synthesized` flag on the envelope |

`Expression` becomes an envelope `Expression { kind: ExpressionKind,
parentheses, type_arguments, synthesized, comments, span }`: parentheses and
comments sit beside the shape, so every existing match on the shape sees
exactly what it sees today (a new `Parenthesized` variant was rejected: about
fifteen `if let`/`matches!` shape checks — `(outer N) m` in the resolver,
`ifTrue: ([…])` inlining, primitive fast paths — would silently change
meaning). Types are token sequences, not a structured type AST: the
specification gives no type grammar and calls the annotations documentation
only; the parser accepts exactly what it accepts today.

**Section 2 — the printer (`ns-syntax`).** `crates/ns-syntax/src/printer/`:
`print_module(&Module, LineEnding) -> String` (the runtime's entry) and
`format(&str) -> Result<String, Vec<Diagnostic>>` (parse, detect the line
ending, print; refuses a source with any error and never prints a recovered
AST). Algorithm: a Wadler/Oppen document (text, line breaks, indentation,
groups that are flat or broken as a whole; outer groups break first; forced
breaks from `///`, multi-statement blocks, temporaries, cascades), laid out in
time linear in the output; a block argument is fitted only by the text
outside its body (Prettier's last-argument expansion, applied to every block
argument). Guarantees: total (every AST `parse` answers prints, without a
panic or a stack overflow — chains walked link by link), deterministic,
idempotent, preserving (the re-parse equals the parse, positions aside;
every comment kept with its owner).

**Section 3 — CLI and LSP (`ns-analyzer`).** `ns-next-analyzer fmt [--check]
[paths…]` (current directory by default, `.ns` files collected through
`ns-tooling`; rewrites only changed files, through a temporary file renamed
into place; `--check` writes nothing, lists the files that would change and
exits 1; a file with a syntax error is left alone, its diagnostics printed,
exit 1, the others still processed). The server announces
`documentFormattingProvider`, formats the open document's text, answers one
`TextEdit` over the whole document or none (unchanged, or a syntax error),
ignores the request's options. Range and on-type formatting are left open.
The JetBrains plugin needs no code change.

**Section 4 — gate, tests, documentation.** A test in
`crates/ns-analyzer/tests/` requires `format(file) == file` for every `.ns`
under `goldens/`, `benchmarks/` and `crates/ns-runtime/core-library/`. The
repository is reformatted in a commit of its own before the gate. Every
golden must keep its outcome; none pins a line or column.
`source_formatting.rs` keeps only "`///`, never `(* *)`". Documentation:
`frontend.md`, a new `docs/design/formatting.md`, `editor-support.md`,
`docs/design/README.md`'s map, `CLAUDE.md`, `OPEN-WORK.md`, `PROGRESS.md`.

## Global Constraints

- Every commit runs, from the repository root: `cargo fmt --all`;
  `cargo clippy --workspace --all-targets` (warning-free);
  `cargo nextest run --workspace`; `cargo test --workspace --doc`;
  `cargo run -p ns-analyzer -- parse-corpus corpus/modules` → `parsed 211, failed 0`.
- Every commit touching `crates/ns-syntax` or `crates/ns-analyzer` (every task
  but Task 13's Markdown-only part) also runs
  `./editors/jetbrains/gradlew -p editors/jetbrains check`.
- Pass `timeout: 600000` to every long command (`cargo nextest run`,
  `cargo build`, `gradlew`) and run every command in the foreground — never
  `run_in_background`, and never stop while work of your own still runs.
- Names are written out (`CLAUDE.md`, "Names are written out"); clippy's
  `min_ident_chars` enforces the short ones.
- A Newspeak module inline in a test is an `indoc! {"…"}` indented with
  spaces with the Rust around it, the closing `"}` on its own line.
- Program outcomes are pinned: no golden's `.toml` changes in this plan. A
  golden that changes outcome is a defect of the change, never a reason to
  update the golden.
- `vm.rs` holds no dispatch logic; compiling an already-resolved module never
  raises a `NewspeakError`; a chain of messages is walked link by link, never
  by recursing along a receiver (`CLAUDE.md`, "Invariants").
- No new external dependency.
- The runtime keeps ignoring `<-:` (the §5.6 deviation stays as documented).
- Comments in code explain the decision, not the mechanics.
- Conventional commits. Every commit message ends with exactly these two
  lines, the first naming the model that wrote the commit:
  ```
  Co-Authored-By: Claude <your model name and version> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01FVyKYkAz63zaBxtxeEM59m
  ```
- Never pass `-c user.name`/`-c user.email` or otherwise override the git
  identity; never `--no-verify`.
- A commit that never saw the end-of-batch gates (`finishing-a-batch` skill:
  the five stress modes, `run-corpus` plain and under stress, the benchmarks
  profile, `verifyPlugin`) is work in progress; those gates run once, after
  Task 13, not per task.

## Review Focus

1. **A comment between any two tokens** — inside a type annotation, between a
   method pattern and `=` (metadata), right after `^`, between a keyword and
   its argument, inside an empty block, tuple, body or class side, after the
   last member — survives formatting at the same anchor, and the output is
   idempotent. Pinned in Task 5 (attachment) and Task 9 (printing).
2. **A 100,000-link chain** (`1 + 1 + …`, `x foo foo …`) and the deepest
   nesting the parser accepts format on a 256 KiB stack without overflow.
   Pinned in Tasks 5, 8 and 9.
3. **A CRLF source** (a Windows checkout under `core.autocrlf=true`) formats
   to CRLF, and the gate passes on it; a multi-line string literal keeps its
   own line breaks byte for byte, also when they differ from the file's.
   Pinned in Task 9 and Task 12.
4. **A source with errors** is never printed: `format` answers its
   diagnostics, `fmt` leaves the file untouched and exits 1, the server
   answers no edit; nothing panics on a recovered tree. Pinned in Tasks 9,
   10 and 11.
5. **Runtime semantics after the AST rework**: `(outer N) m`,
   `x ifTrue: ([…])`, implicit-receiver sends, primitive fast paths and
   `<-:` behave exactly as before. Pinned by every golden under both
   lowerings in Tasks 1–4, plus the explicit tests named there.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ns-syntax/src/ast.rs` | The lossless AST (modified throughout Tasks 1–5) |
| `crates/ns-syntax/src/attach.rs` | New: attaches each comment to its node after parsing (Task 5) |
| `crates/ns-syntax/src/positions.rs` | New: `Module::without_positions`, the comparison the round-trip tests use (Task 5) |
| `crates/ns-syntax/src/parser/*.rs` | Records what it drops today (Tasks 1–4), hands its final tokens to the attachment (Task 5) |
| `crates/ns-syntax/src/printer/mod.rs` | New: `print_module`, `format`, `LineEnding` (Task 9) |
| `crates/ns-syntax/src/printer/document.rs` | New: the document algebra and its layout; knows nothing of Newspeak (Task 6) |
| `crates/ns-syntax/src/printer/declarations.rs` | New: module, prologue, classes, slots, members, methods, bodies (Task 7) |
| `crates/ns-syntax/src/printer/expressions.rs` | New: statements and expressions (Task 8) |
| `crates/ns-syntax/src/printer/comments.rs` | New: emitting attached comments (Task 9) |
| `crates/ns-syntax/tests/format_rules.rs` | New: one test per style rule (Tasks 7–9) |
| `crates/ns-syntax/tests/format_properties.rs` | New: idempotence and preservation over every source (Task 9) |
| `crates/ns-runtime/src/**`, `crates/ns-analyzer/src/**`, `crates/ns-cli/src/program.rs`, tests | Mechanical adaptation to the envelope and accessors (Tasks 1–5) |
| `crates/ns-analyzer/src/commands/fmt.rs` | New: `ns-next-analyzer fmt` (Task 10) |
| `crates/ns-analyzer/src/server.rs` | `textDocument/formatting` (Task 11) |
| `crates/ns-analyzer/tests/formatting_gate.rs` | New: the repository gate (Task 12) |
| `crates/ns-cli/tests/source_formatting.rs` | Keeps only the comment-form rule (Task 12) |
| `docs/design/formatting.md` and the other documents | Task 13 |

---

### Task 1: The expression envelope, with parentheses

**Files:**
- Modify: `crates/ns-syntax/src/ast.rs`, `crates/ns-syntax/src/parser/expression.rs`,
  `crates/ns-syntax/src/parser/declaration.rs`, `crates/ns-syntax/src/parser/mod.rs`,
  `crates/ns-syntax/src/lib.rs`
- Modify (mechanical): every file that names `Expression::…` — the survey found
  `ns-analyzer/src/{folding,semantic_tokens,document_symbols}.rs`,
  `ns-analyzer/tests/corpus.rs`, `ns-runtime/src/compiler/{mod,lower,reference,capture,control_flow,primitive_method,simultaneous,unwind_marker}.rs`,
  `ns-runtime/src/resolver/{mod,sends,classes,scope}.rs`,
  `ns-runtime/src/interpreter/{kernel,nested,instantiate}.rs`, `ns-runtime/tests/resolver.rs`,
  `ns-syntax/tests/recovery.rs`; let the compiler find the rest.
- Test: `crates/ns-syntax/src/parser/mod.rs` (tests module), `crates/ns-syntax/tests/snapshots/parse_snapshot__hello_world_ast.snap` (accept the new shape)

**Interfaces:**
- Produces (later tasks rely on these names exactly):

```rust
/// An expression: its shape, and what surrounds the shape in the source
/// without changing what it means.
#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    /// The parentheses around the shape, innermost first.
    pub parentheses: Vec<Parenthesis>,
    /// Value-site type arguments written right after the shape, `Box[Integer]`
    /// (Task 4 fills it; `None` until then).
    pub type_arguments: Option<TypeAnnotation>,
    /// The parser made this node up; nothing in the source spells it (the
    /// implicit receiver of a send).
    pub synthesized: bool,
    /// Comments attached to this expression (Task 5 fills it).
    pub comments: Vec<AttachedComment>,
    /// The shape's span, widened over the parentheses.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parenthesis {
    pub delimiters: Delimiters,
    /// Type arguments written right after this `)` (Task 4 fills it).
    pub type_arguments: Option<TypeAnnotation>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionKind { /* exactly today's `Expression` variants, renamed */ }

impl Expression {
    /// An expression of `kind` with nothing around it, spanning `span`.
    pub fn bare(kind: ExpressionKind, span: Span) -> Expression;
}
```

Declare `TypeAnnotation`, `TypeToken`, `AttachedComment`, `Anchor` and
`Placement` fully in this task, exactly as Tasks 4 and 5 define them below
(copy the definitions), so their shape never changes later. Task 1 leaves
every field of those types `None` or empty.

- [ ] **Step 1: Write the failing tests** (in `crates/ns-syntax/src/parser/mod.rs`'s tests module, beside `generic_instantiation_drops_type_arguments`):

```rust
#[test]
fn parentheses_are_recorded_innermost_first() {
    let expression = parse_expression("((a foo)) bar");
    let ExpressionKind::Send(send) = &expression.kind else {
        panic!("a send");
    };
    assert!(expression.parentheses.is_empty());
    let receiver = &send.receiver;
    assert_eq!(receiver.parentheses.len(), 2);
    let source = "((a foo)) bar";
    assert_eq!(receiver.parentheses[0].delimiters.open.text(source), "(");
    assert_eq!(receiver.parentheses[0].delimiters.open.start, 1);
    assert_eq!(receiver.parentheses[1].delimiters.open.start, 0);
    assert_eq!(receiver.span.text(source), "((a foo))");
    assert!(matches!(receiver.kind, ExpressionKind::Send(_)));
}

#[test]
fn the_implicit_receiver_is_marked_synthesized() {
    let expression = parse_expression("foo: 3");
    let ExpressionKind::Send(send) = &expression.kind else {
        panic!("a send");
    };
    assert!(send.receiver.synthesized);
    assert!(!expression.synthesized);
}
```

(`parse_expression` is the existing test helper that wraps
`Parser::parse_expression_for_test`; if it is named differently in that
module, use the existing one.)

- [ ] **Step 2: Run them to see them fail to compile**

Run: `cargo nextest run -p ns-syntax parentheses_are_recorded` (timeout 600000)
Expected: compile error — `ExpressionKind` not found.

- [ ] **Step 3: Introduce the envelope**

In `ast.rs`: rename `enum Expression` to `enum ExpressionKind`, add the
`Expression`/`Parenthesis` structs above, `Expression::bare`. `Send`, `Cascade`,
`Assign`, `BlockExpression`, `DynamicArray` keep their fields; `Assign.value`
stays `Box<Expression>` (the envelope). `Send::drop`, `into_receiver_and_message`,
`receiver_chain` and `placeholder` walk `expression.kind`:

```rust
impl Drop for Send {
    fn drop(&mut self) {
        let mut receiver = std::mem::replace(&mut self.receiver, placeholder(self.span));
        while let ExpressionKind::Send(mut link) = receiver.kind {
            receiver = std::mem::replace(&mut link.receiver, placeholder(link.span));
        }
    }
}

fn placeholder(span: Span) -> Expression {
    Expression::bare(ExpressionKind::Literal(Literal::Nil { span }), span)
}
```

`receiver_chain` follows `while let ExpressionKind::Send(link) = &receiver.kind`.
The chain still stops at a parenthesised send, exactly as today — today `(a b)
c` also nests `a b` as the receiver, so the chain is unchanged; only a
parenthesised link's envelope now carries a `Parenthesis`. Keep the existing
small-stack drop test passing with the envelope.

In the parser: every place that builds an expression builds
`Expression::bare(kind, span)`. `parse_primary_inner`'s parenthesised case
pushes `Parenthesis { delimiters: Delimiters { open, close }, type_arguments:
None }` onto the inner expression and widens `span` to `open.start..close.end`
(`close` is `None` only in a recovered parse; then widen to the inner end).
The synthesized implicit receiver (`expression.rs` ~465–472) gets
`synthesized: true` and keeps its zero-width span, because
`resolver/sends.rs:66` still reads it.

`expression_span` answers `expression.span`. **Every consumer that keyed a
side table by an expression's span** (`capture.rs:96`, `control_flow.rs:25`,
`lower.rs:281`, the resolver's `HashMap<Span, Binding>`) must key by the
*shape's* span, not the widened one, or a parenthesised identifier would miss
its binding. Give `ExpressionKind` a `pub fn span(&self) -> Span` (today's
`expression_span` body over the kind) and make those consumers call
`expression.kind.span()`. Leave `Expression::span` for the printer and the
editor.

Every consumer: `match &expression.kind { ExpressionKind::… }`,
`if let ExpressionKind::Identifier(identifier) = &expression.kind`, and so
on. This is a rename only: no consumer may start looking at `parentheses`.
`compiler/simultaneous.rs` and `interpreter/nested.rs`, which build
expressions, use `Expression::bare`.

- [ ] **Step 4: Run the new tests, then everything**

Run: `cargo nextest run -p ns-syntax` then the Global Constraints' full list
(timeout 600000 each). Accept the changed `parse_snapshot__hello_world_ast.snap`
with `cargo insta accept` only after reading the diff: only the envelope may
differ. Expected: all green, every golden unchanged under both lowerings,
`parsed 211, failed 0`.

- [ ] **Step 5: Pin the semantics the envelope must not change**

Add to `crates/ns-runtime/tests/resolver.rs` (or the golden family that
already pins them — check `goldens/language/` first and add a golden only if
none covers it):

```rust
#[test]
fn a_parenthesised_outer_reference_resolves_as_the_bare_one() {
    let parenthesised = indoc! {"
        class Outer = (
        ) (
            class Inner = (
            ) (
                public answer = ( ^(outer Outer) yourself )
            )
        )
    "};
    let bare = parenthesised.replace("(outer Outer)", "outer Outer");
    assert_eq!(
        receiver_binding_of_yourself(parenthesised),
        receiver_binding_of_yourself(&bare)
    );
}
```

`receiver_binding_of_yourself` is a helper you add to `tests/resolver.rs`:
resolve the source with the file's existing resolve helper, find the send
of `yourself` in `Inner>>answer`, and answer the binding the resolution
records for its receiver — the receiver's `kind.span()` is the key (Step 3),
so look it up by that span in the resolution's binding table, and answer the
binding's `{:?}` text as a `String` so the two compare without depending on
spans (the binding names `outer Outer`'s level, not a position; if its
`Debug` holds a span, compare the variant and the level instead). Then a golden
`goldens/language/expression/parenthesised_inlinable_block.ns` printing the
result of `true ifTrue: ([1]) ifFalse: ([2])` and of `(true) ifTrue: [3]`,
with its `.toml` (status 0, stdout `1\n3\n`) — under both lowerings it must
answer the same; use the printing idiom of the neighbouring goldens.

Run the full list again. Expected: green.

- [ ] **Step 6: Commit**

```bash
git add -A crates goldens
git commit -m "refactor: wrap expressions in an envelope that records parentheses"
```
(with the two trailer lines from Global Constraints)

---

### Task 2: Statements with spans, members in source order, empty parts normalised

**Files:**
- Modify: `crates/ns-syntax/src/ast.rs`, `crates/ns-syntax/src/parser/declaration.rs`, `crates/ns-syntax/src/lib.rs` (`empty_module`)
- Modify (mechanical): every reader of `Statement::…`, `ClassBody.slots/.methods/.nested_classes` (survey: `document_symbols.rs:90-240`, `folding.rs:114-139`, `semantic_tokens.rs:143-190,300-370`, `resolver/mod.rs:175-281`, `resolver/classes.rs:26-63,116-124`, `interpreter/instantiate.rs`, `interpreter/kernel.rs:675-686`, `primitive_method.rs:215`, `ns-cli/src/program.rs`, `commands/run_corpus.rs`, the tests listed in the survey)
- Test: `crates/ns-syntax/src/parser/declaration.rs` tests

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub comments: Vec<AttachedComment>,
    /// From the statement's first token (`^` for a return) to its last,
    /// the separating period excluded.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatementKind {
    Expression(Expression),
    Return(Expression), // `^ expression`
}

#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    Class(ClassDeclaration),
    LazySlot(SlotDeclaration),
    Method(MethodDeclaration),
}

pub struct ClassBody {
    /// The header's slots (instance side only), in written order.
    pub header_slots: Vec<SlotDeclaration>,
    pub initializer_expressions: Vec<Statement>,
    /// The side's members in source order.
    pub members: Vec<Member>,
    pub delimiters: Option<Delimiters>,
    pub slot_delimiters: Option<Delimiters>,
    pub comments: Vec<AttachedComment>,
    pub span: Span,
}

impl ClassBody {
    /// The header's slots, then the lazy slots in source order: the order
    /// slot indices and image rows have always used.
    pub fn slots(&self) -> impl Iterator<Item = &SlotDeclaration>;
    pub fn methods(&self) -> impl Iterator<Item = &MethodDeclaration>;
    pub fn nested_classes(&self) -> impl Iterator<Item = &ClassDeclaration>;
    pub fn lazy_slots(&self) -> impl Iterator<Item = &SlotDeclaration>;
}
```

`ClassDeclaration`, `SlotDeclaration`, `MethodDeclaration`, `CodeBody`,
`BlockExpression`, `MessageSend`, `SuperclassClause` and `Module` each gain
`pub comments: Vec<AttachedComment>` in this task (empty until Task 5).

The field is renamed `slots` → `header_slots` on purpose: every reader that
meant "all slots" fails to compile and is switched to `slots()`, which
answers exactly today's order. Readers that index (`slots[i]`, `.len()`)
collect or count the iterator.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn members_keep_their_source_order_across_kinds() {
    let module = module(indoc! {"
        class Outer = (
        ) (
            public b = ( ^2 )
            class Inner = () ()
            lazy cache = 3.
            public a = ( ^1 )
        )
    "});
    let kinds: Vec<&str> = module
        .root_class
        .instance_side
        .members
        .iter()
        .map(|member| match member {
            Member::Method(_) => "method",
            Member::Class(_) => "class",
            Member::LazySlot(_) => "lazy",
        })
        .collect();
    assert_eq!(kinds, ["method", "class", "lazy", "method"]);
    let methods: Vec<&str> = module
        .root_class
        .instance_side
        .methods()
        .map(|method| method_selector(method))
        .collect();
    assert_eq!(methods, ["b", "a"]);
}

#[test]
fn slots_answers_header_slots_then_lazy_slots() {
    let module = module(indoc! {"
        class Outer = (
            |
              first = 1.
            |
        ) (
            lazy second = 2.
            public m = ( ^first )
            lazy third = 3.
        )
    "});
    let names: Vec<&str> = module
        .root_class
        .instance_side
        .slots()
        .map(|slot| slot.name.name.as_str())
        .collect();
    assert_eq!(names, ["first", "second", "third"]);
}

#[test]
fn an_empty_slot_list_and_an_empty_class_side_are_not_kept() {
    let module = module("class Outer = ( | | ) ( ) : ( )");
    assert_eq!(module.root_class.instance_side.slot_delimiters, None);
    assert!(module.root_class.class_side.is_none());
}

#[test]
fn a_statement_spans_its_caret() {
    let module = module(indoc! {"
        class Outer = () (
            public m = ( ^3 )
        )
    "});
    let source = "class Outer = () (\n    public m = ( ^3 )\n)\n";
    let method = module.root_class.instance_side.methods().next().expect("m");
    let statement = &method.body.statements[0];
    assert!(matches!(statement.kind, StatementKind::Return(_)));
    assert_eq!(statement.span.text(source), "^3");
}
```

(`method_selector` is a three-line helper in the tests module answering the
unary selector's name; `module` is the existing helper.) The empty-parts test
extends to temporaries: `public m = ( | | ^3 )` answers
`temporaries_delimiters == None`. The rule: the parser records no
`slot_delimiters`/`temporaries_delimiters` for a list with no declaration,
and no `class_side` for `: ( )` with no member — unless a comment lies between
the delimiters (Task 5 adds that test; here, with no comments recorded yet,
simply check "no declaration" at the point the delimiters close and keep a
`TODO`-free note in the doc comment that a comment inside keeps them).

Before normalising `class_side`, establish that the runtime treats
`Some(empty)` and `None` identically: grep every reader of `class_side`
(`resolver/classes.rs`, `instantiate.rs`, `document_symbols.rs`) and confirm
each answers the same for an empty body; if one does not, stop and report.

- [ ] **Step 2: Run them to see them fail** (`cargo nextest run -p ns-syntax members_keep`, timeout 600000; expected: compile error).

- [ ] **Step 3: Implement**

`parse_side_body` (`declaration.rs:1163-1246`) pushes each member into one
`members` vector as it parses it; the header's slots go to `header_slots`
(today's G1 slots, `declaration.rs:893` merges lazy ones in — stop merging).
The class side (`parse_class_side_body`, methods only per spec §… `classSideDecl →
colon lparen methodDecl* rparen`) pushes `Member::Method`. `Statement` gets its
span in `parse_code_body`: from the statement's first token (the `^` for a
return) to its expression's `span.end`. Apply the empty-parts normalisation.
Adapt every consumer: `ClassBody.slots` → `.slots()`, `.methods` →
`.methods()`, `.nested_classes` → `.nested_classes()`,
`Statement::Return(expression)` → `StatementKind::Return(expression)` on
`statement.kind`, constructions of `Statement` take `comments: Vec::new()` and
a span (a compiler-built statement in `simultaneous.rs` takes its
expression's span).

The order of `nested_classes()` and `methods()` must equal today's vectors:
mixin rows and image rows depend on it (`instantiate.rs:272-273`). Since each
filters `members` in source order and the parser appended each kind in source
order before, they are identical; the goldens and the image tests confirm it.

- [ ] **Step 4: Run the full list** (Global Constraints). Expected: green; snapshot diff limited to the new fields.

- [ ] **Step 5: Commit** — `refactor: keep statements' spans and members in source order`

---

### Task 3: Literal spellings, the prologue, eventual sends

**Files:**
- Modify: `crates/ns-syntax/src/ast.rs`, `crates/ns-syntax/src/parser/{expression,declaration}.rs`, `crates/ns-syntax/src/lib.rs`
- Modify (mechanical): `compiler/lower.rs:2031-2049`, `compiler/mod.rs:477-485`, `semantic_tokens.rs:727-735`, `primitive_method.rs:83-89`, every `Literal::String {…}`/`Literal::Symbol {…}` pattern without `..`

**Interfaces:**
- Produces:

```rust
pub struct Module {
    pub language_id: Option<Identifier>,
    pub category: Option<Category>,
    pub root_class: ClassDeclaration,
    pub comments: Vec<AttachedComment>,
    pub span: Span,
}

/// The module's category string (§8.1's `classCategory`).
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub value: String,
    /// The string as written, quotes included.
    pub text: String,
    pub span: Span,
}

pub enum Literal {
    Integer { text: String, span: Span },
    Float { text: String, span: Span },
    /// `value` unescaped; `text` as written, quotes included (`'it''s'`, `"a"`).
    String { value: String, text: String, span: Span },
    /// `value` without `#` and quotes; `text` as written (`#foo`, `#'a b'`, `#at:put:`).
    Symbol { value: String, text: String, span: Span },
    Boolean { value: bool, span: Span },
    Nil { span: Span },
    Pattern { text: String, span: Span },
}

pub struct MessageSend {
    pub selector: Selector,
    pub arguments: Vec<Expression>,
    /// Sent with `<-:` (§5.6). The runtime runs it as an ordinary send.
    pub eventual: bool,
    /// Type arguments written after a unary selector, `foo bar[E]` (Task 4).
    pub type_arguments: Option<TypeAnnotation>,
    pub comments: Vec<AttachedComment>,
    pub span: Span,
}
```

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn literals_keep_their_spelling() {
    let source = "{'it''s'. \"a\". #foo. #'foo'. #at:put:. 16r1F}";
    let expression = parse_expression(source);
    let ExpressionKind::DynamicArray { elements, .. } = &expression.kind else {
        panic!("a tuple");
    };
    let texts: Vec<String> = elements
        .iter()
        .map(|element| match &element.kind {
            ExpressionKind::Literal(Literal::String { text, .. })
            | ExpressionKind::Literal(Literal::Symbol { text, .. })
            | ExpressionKind::Literal(Literal::Integer { text, .. }) => text.clone(),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(texts, ["'it''s'", "\"a\"", "#foo", "#'foo'", "#at:put:", "16r1F"]);
}

#[test]
fn the_prologue_is_kept_when_present_and_absent_otherwise() {
    let with = module("Newspeak3\n'Root'\nclass A = () ()");
    assert_eq!(with.language_id.expect("an id").name, "Newspeak3");
    assert_eq!(with.category.expect("a category").text, "'Root'");
    let without = module("class A = () ()");
    assert!(without.language_id.is_none());
    assert!(without.category.is_none());
}

#[test]
fn an_eventual_send_is_marked() {
    let expression = parse_expression("a <-: foo: 3");
    let ExpressionKind::Send(send) = &expression.kind else {
        panic!("a send");
    };
    assert!(send.message.eventual);
}
```

Rename the existing `eventual_send_operator_is_dropped` test to
`an_eventual_send_runs_as_an_ordinary_one` and keep what it asserts about the
send's shape.

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** `text` is the token's span text; the category
  from the existing `string_lexeme_body` call site, keeping the span. `eat_eventual_send_operator` answers whether it ate the operator; the message built after it records `eventual`. Consumers read `value` as before (patterns gain `..`). Nothing outside `ns-syntax` reads `language_id`/`category` (verify with grep).
- [ ] **Step 4: Run the full list.** Expected: green.
- [ ] **Step 5: Commit** — `feat: keep literal spellings, the prologue and eventual sends in the AST`

---

### Task 4: Type annotations

**Files:**
- Modify: `crates/ns-syntax/src/ast.rs`, `crates/ns-syntax/src/parser/{mod,declaration,expression}.rs`
- Modify (mechanical): readers of `MessagePattern::Keyword { parts }`, `MessagePattern::Binary { parameter }`, `BlockExpression.parameters` (survey: document_symbols 6, semantic_tokens 8, analyzer tests/corpus 3, ns-cli program.rs 3, lower.rs 3, primitive_method 5, instantiate 6, resolver/mod 8, tests/core_library_documentation 3, ns-syntax tests/recovery 2)
- Docs touched in Task 13 only.

**Interfaces:**
- Produces:

```rust
/// A type annotation, kept as the tokens written: the specification gives
/// no type grammar and the annotations document only (§… "optional type
/// annotation syntax based on the Strongtalk type system").
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAnnotation {
    /// Every token from the opening `<`, `[` or `where` through the
    /// matching closer, in order.
    pub tokens: Vec<TypeToken>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeToken {
    pub kind: TokenKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: Identifier,
    pub type_annotation: Option<TypeAnnotation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeywordPart {
    /// The keyword without its colon.
    pub keyword: Identifier,
    pub parameter: Parameter,
}

pub enum MessagePattern {
    Unary { selector: Identifier, type_parameters: Option<TypeAnnotation> },
    Binary { selector: Identifier, parameter: Parameter },
    Keyword { parts: Vec<KeywordPart> },
}
```

New fields: `SlotDeclaration.type_annotation: Option<TypeAnnotation>` (slots,
temporaries, lazy slots); `MethodDeclaration.return_type: Option<TypeAnnotation>`
(the `<…>` after `^`, the caret not included) and
`MethodDeclaration.where_clause: Option<TypeAnnotation>`;
`ClassDeclaration.type_parameters: Option<TypeAnnotation>` (`<[K, V]>` or
`[E]`), `ClassDeclaration.factory_where_clause: Option<TypeAnnotation>` only if
the parser accepts a where clause after a factory pattern (check
`try_parse_message_pattern`; if it does not, omit the field);
`SuperclassClause.receiver_type_arguments: Option<TypeAnnotation>`;
`BlockExpression.parameters: Vec<Parameter>`; `Expression.type_arguments`,
`Parenthesis.type_arguments`, `MessageSend.type_arguments` (declared in
Tasks 1 and 3) are filled here.

**The rule for the parser:** every call of `skip_where_clause`,
`skip_type_arguments`, `skip_type_annotation`, `skip_return_type_annotation`
and `parse_type_parameter_declaration` becomes a call that answers the
skipped tokens as a `TypeAnnotation` and stores it at the place listed above
(call sites: `declaration.rs:415, 434, 447-448, 453-454, 461-474, 492-497,
605, 798-801, 1023, 1354, 1404, 1502-1538`; `expression.rs:360, 524-530,
580-581`). What is accepted and every diagnostic text stay exactly as they
are.

- [ ] **Step 1: Write the failing tests**

```rust
fn type_text(annotation: &TypeAnnotation) -> String {
    annotation
        .tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn every_type_annotation_is_kept() {
    let module = module(indoc! {"
        class Map <[K, V]> new: capacity <Integer> = (
            |
              size <Integer> ::= 0.
            |
        ) (
            public at: key <K> put: value <V> ^<V> = (
                | previous <V | Nil> |
                ^[:each <K> | each] value: key
            )
            public items ^<List[V]> = ( ^Box[Integer] new )
        )
    "});
    let class = &module.root_class;
    assert_eq!(type_text(class.type_parameters.as_ref().expect("<[K, V]>")), "< [ K , V ] >");
    let header_slot = &class.instance_side.header_slots[0];
    assert_eq!(type_text(header_slot.type_annotation.as_ref().expect("<Integer>")), "< Integer >");
    let methods: Vec<&MethodDeclaration> = class.instance_side.methods().collect();
    let MessagePattern::Keyword { parts } = &methods[0].pattern else {
        panic!("at:put:");
    };
    assert_eq!(type_text(parts[1].parameter.type_annotation.as_ref().expect("<V>")), "< V >");
    assert_eq!(type_text(methods[0].return_type.as_ref().expect("^<V>")), "< V >");
    let temporary = &methods[0].body.temporaries[0];
    assert_eq!(type_text(temporary.type_annotation.as_ref().expect("<V | Nil>")), "< V | Nil >");
    assert_eq!(type_text(methods[1].return_type.as_ref().expect("^<List[V]>")), "< List [ V ] >");
    let StatementKind::Return(value) = &methods[1].body.statements[0].kind else {
        panic!("a return");
    };
    let ExpressionKind::Send(send) = &value.kind else {
        panic!("Box[Integer] new");
    };
    assert_eq!(type_text(send.receiver.type_arguments.as_ref().expect("[Integer]")), "[ Integer ]");
}
```

Add one test per remaining site: a binary parameter (`+ other <Number>`), a
unary pattern's type parameters (`foo[E]`), a where clause, a lazy slot's
type, a superclass receiver's type arguments (`= Collection[E] new (`), a
type argument after a unary selector (`a foo[E] bar`), and type arguments
after a parenthesis (`(Box)[Integer] new`).

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** per the rule above; adapt consumers (`parts[i].0`
  → `parts[i].keyword`, `parts[i].1` → `parts[i].parameter.name`, block
  parameters `.name`). The resolver and compiler ignore every annotation.
- [ ] **Step 4: Run the full list.** `parse-corpus` must still answer `parsed 211, failed 0`. Expected: green.
- [ ] **Step 5: Commit** — `feat: keep type annotations in the AST`

---

### Task 5: Comments attached to nodes

**Files:**
- Create: `crates/ns-syntax/src/attach.rs`, `crates/ns-syntax/src/positions.rs`
- Modify: `crates/ns-syntax/src/ast.rs` (remove `class_comment`, add `ClassDeclaration::class_comment()`), `crates/ns-syntax/src/lib.rs` (`parse` and `parse_for_editor` keep comments and attach them), `crates/ns-syntax/src/parser/mod.rs` (`into_parts`), `crates/ns-syntax/src/parser/declaration.rs` (drop `take_class_comment`; the empty-parts normalisation keeps delimiters that enclose a comment)
- Test: `crates/ns-syntax/src/attach.rs` tests, `crates/ns-syntax/tests/attachment.rs`

**Interfaces:**
- Produces:

```rust
/// A comment, kept in the AST at the node it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachedComment {
    pub kind: CommentKind,
    /// `Line`: the text after `///` on each line of the block, trailing
    /// whitespace removed. `Block`: the comment's lines from `(*` through
    /// `*)`, trailing whitespace removed; continuation lines keep their own
    /// leading whitespace.
    pub lines: Vec<String>,
    /// `Block`: the column, in `char`s from the start of its line, at which
    /// `(*` stood; the printer shifts the continuation lines by the
    /// difference to where it prints `(*`. `Line`: 0.
    pub column: u32,
    pub anchor: Anchor,
    pub placement: Placement,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Before the node, after whatever precedes it.
    Before,
    /// After the node, before whatever follows it.
    After,
    /// Inside the node, after its `n`-th own token (see `attach.rs`).
    Inner(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Nothing but whitespace before it on its line.
    OwnLine,
    /// Code before it on its line, a line break (or the end) after it.
    EndOfLine,
    /// Code, or a delimiter, on both sides on the same line.
    Inline,
}

impl ClassDeclaration {
    /// The class comment: the first comment in the header, before any slot
    /// or initializer expression.
    pub fn class_comment(&self) -> Option<&AttachedComment>;
}

impl Module {
    /// This module with every position erased — spans, delimiters' spans and
    /// block comments' columns zeroed — so two parses of differently laid
    /// out sources compare equal when they say the same.
    pub fn without_positions(&self) -> Module;
}

// parser/mod.rs
impl Parser<'_> {
    /// The tokens as the parser ended with them (after it split `-4` and
    /// `>>`), and its diagnostics.
    pub(crate) fn into_parts(self) -> (Vec<Token>, Vec<Diagnostic>);
}

// attach.rs
pub(crate) fn attach_comments(module: &mut Module, tokens: &[Token], comments: &[Comment], source: &str);
```

**The attachment rule** (write it as `attach.rs`'s module documentation):

- The *commentable nodes* are `Module`, `ClassDeclaration`, `ClassBody`,
  `SlotDeclaration`, `MethodDeclaration`, `CodeBody`, `Statement`,
  `Expression`, `MessageSend`, `BlockExpression`, `SuperclassClause`. Every
  other part (an `Identifier` in a pattern, a `TypeAnnotation`, a
  `Category`, a keyword, a delimiter) consists of *own tokens* of the
  innermost commentable node around it.
- For each comment, find the innermost commentable node `N` whose span
  contains it. Among `N`'s commentable children and own tokens, in source
  order, let `P` be the last one ending before the comment and `Q` the first
  one starting after it.
  - `P` is a child and no line break lies between `P`'s end and the comment →
    `After` on `P`.
  - otherwise `Q` is a child → `Before` on `Q`.
  - otherwise `P` is a child → `After` on `P`.
  - otherwise → `Inner(n)` on `N`, `n` = the number of `N`'s own tokens
    before the comment.
  - A comment outside the root class's span is `Before`/`After` on the root
    class, or `Inner(n)` on the module (`n` counts the prologue's tokens).
- `Placement` from the source: `OwnLine` when only whitespace precedes it on
  its line; else `EndOfLine` when only whitespace follows it up to a line
  break or the end (a `///` block is always `OwnLine` or `EndOfLine`); else
  `Inline`.
- Several comments on one node keep source order.
- The walk visits a chain of sends link by link (`Send::receiver_chain`),
  never recursing along a receiver. It never panics on a recovered tree
  (zero-width spans, `Literal::Nil` placeholders, missing delimiters): a
  comment it cannot place goes to the module as `Inner(0)`.
- Own tokens are the parser's final tokens (`into_parts`), so the printer,
  which prints `-` and `4` separately, counts as the attachment did.
- The parser's empty-parts normalisation (Task 2) keeps `slot_delimiters`,
  `temporaries_delimiters` and `class_side` when a comment lies between the
  delimiters.

- [ ] **Step 1: Write the failing tests** (`crates/ns-syntax/tests/attachment.rs`)

```rust
use indoc::indoc;
use ns_syntax::ast::*;

fn parsed(source: &str) -> Module {
    match ns_syntax::parse(source) {
        Ok(module) => module,
        Err(diagnostics) => panic!("{diagnostics:?}"),
    }
}

fn lines_of(comment: &AttachedComment) -> Vec<&str> {
    comment.lines.iter().map(String::as_str).collect()
}

#[test]
fn a_comment_above_a_member_leads_it() {
    let module = parsed(indoc! {"
        class A = () (
            /// Answers one.
            public one = ( ^1 )
        )
    "});
    let method = module.root_class.instance_side.methods().next().expect("one");
    assert_eq!(method.comments.len(), 1);
    let comment = &method.comments[0];
    assert_eq!(comment.anchor, Anchor::Before);
    assert_eq!(comment.placement, Placement::OwnLine);
    assert_eq!(lines_of(comment), [" Answers one."]);
}

#[test]
fn a_comment_after_a_statement_on_its_line_trails_it() {
    let module = parsed(indoc! {"
        class A = () (
            public m = (
                count:: 0. /// restart
                ^count
            )
        )
    "});
    let method = module.root_class.instance_side.methods().next().expect("m");
    let first = &method.body.statements[0];
    assert_eq!(first.comments.len(), 1);
    assert_eq!(first.comments[0].anchor, Anchor::After);
    assert_eq!(first.comments[0].placement, Placement::EndOfLine);
}

#[test]
fn a_comment_after_the_last_member_dangles_in_the_side() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^1 )

            (* the end *)
        )
    "});
    let side = &module.root_class.instance_side;
    let method = side.methods().next().expect("m");
    assert!(method.comments.is_empty());
    assert_eq!(side.comments.len(), 1);
    assert!(matches!(side.comments[0].anchor, Anchor::Inner(_)));
}

#[test]
fn metadata_between_a_pattern_and_its_equals_sign_is_inner() {
    let module = parsed(indoc! {"
        class A = () (
            public foo: x (* :exemplar: A new foo: 3 *) = ( ^x )
        )
    "});
    let method = module.root_class.instance_side.methods().next().expect("foo:");
    assert_eq!(method.comments.len(), 1);
    assert!(matches!(method.comments[0].anchor, Anchor::Inner(_)));
    assert_eq!(method.comments[0].placement, Placement::Inline);
}

#[test]
fn a_comment_between_a_keyword_and_its_argument_leads_the_argument() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^self at: (* index *) 1 put: 2 )
        )
    "});
    let method = module.root_class.instance_side.methods().next().expect("m");
    let StatementKind::Return(value) = &method.body.statements[0].kind else {
        panic!("a return");
    };
    let ExpressionKind::Send(send) = &value.kind else {
        panic!("at:put:");
    };
    let argument = &send.message.arguments[0];
    assert_eq!(argument.comments.len(), 1);
    assert_eq!(argument.comments[0].anchor, Anchor::Before);
    assert_eq!(argument.comments[0].placement, Placement::Inline);
}

#[test]
fn a_block_comment_keeps_its_lines_and_column() {
    let module = parsed("class A = () (\n    (* one\n       two *)\n    public m = ()\n)\n");
    let method = module.root_class.instance_side.methods().next().expect("m");
    let comment = &method.comments[0];
    assert_eq!(comment.column, 4);
    assert_eq!(lines_of(comment), ["(* one", "       two *)"]);
}

#[test]
fn the_class_comment_is_the_first_comment_in_the_header() {
    let module = parsed(indoc! {"
        class A = (
            (* A counter. *)
            |
              count ::= 0.
            |
        ) ()
    "});
    let comment = module.root_class.class_comment().expect("a class comment");
    assert_eq!(lines_of(comment), ["(* A counter. *)"]);
}

#[test]
fn a_comment_inside_empty_bars_keeps_them() {
    let module = parsed("class A = ( | (* none yet *) | ) ()");
    assert!(module.root_class.instance_side.slot_delimiters.is_some());
}

#[test]
fn the_editor_parse_attaches_like_parse() {
    let source = indoc! {"
        class A = () (
            /// one
            public one = ( ^1 ) (* trailing *)
        )
    "};
    let editor = ns_syntax::parse_for_editor(source);
    assert_eq!(editor.module, parsed(source));
}

#[test]
fn without_positions_equates_layouts_that_say_the_same() {
    let narrow = parsed("class A = () (\npublic m = ( ^1 ) )");
    let wide = parsed("class A = () (\n        public m = (\n            ^1\n        )\n)\n");
    assert_eq!(narrow.without_positions(), wide.without_positions());
}

#[test]
fn a_long_chain_attaches_on_a_small_stack() {
    let mut source = String::from("class A = () (\npublic m = ( ^1");
    for _ in 0..100_000 {
        source.push_str(" + 1");
    }
    source.push_str(" (* end *) )\n)\n");
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let module = parsed(&source);
            drop(module.without_positions());
        })
        .expect("spawn")
        .join()
        .expect("attaching and erasing a long chain stays on a small stack");
}
```

Plus one test each for: a comment right after `^` (Before on the returned
expression), a comment inside a type annotation (Inner on the slot), a
comment in an empty block `[ (* nothing *) ]` (Inner on the block or its
`CodeBody`), a comment before the class (`Before` on the root class), a
class-side comment that is kept (today it is discarded), and a recovered
parse (`parse_for_editor("class A = ( (* x *) public m = ( ^")`) that
attaches without panicking.

If `parsed(...)` needs a `{:?}` of diagnostics, that is allowed: `Diagnostic`
is not a `Value`.

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** `parse` lexes with `run_keeping_comments` (still
  refusing any lexer error), parses, takes `into_parts`, and calls
  `attach_comments` with the parser's tokens. `parse_for_editor` does the same
  and still answers `comments` (the side table the folding uses). Remove
  `class_comment` and `take_class_comment`; `ClassDeclaration::class_comment()`
  answers the first comment attached in the header (to the instance side's
  `ClassBody` as `Inner` before the slot bar, or `Before` on the first header
  slot or initializer). The parser learns of the comments for the
  empty-parts rule only: `Parser::new` takes the comments' spans as a third
  argument, and the normalisation keeps the delimiters when one of them lies
  between the opening and the closing delimiter. `positions.rs` implements `without_positions` with
  one mutable walk (chains link by link). Keep `Comment` (span-based) for the
  editor.
- [ ] **Step 4: Run the full list.** Expected: green; `the_editor_parse_agrees_with_parse` still holds (both sides now carry comments).
- [ ] **Step 5: Commit** — `feat: attach every comment to its node in the AST`

---

### Task 6: The document algebra and its layout

**Files:**
- Create: `crates/ns-syntax/src/printer/mod.rs` (only `mod document;` and the
  `pub(crate)` re-export for now), `crates/ns-syntax/src/printer/document.rs`
- Modify: `crates/ns-syntax/src/lib.rs` (`mod printer;`)

**Interfaces:**
- Produces:

```rust
/// What the printer lowers a module to before it decides where lines break.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Document {
    /// Text without a line break.
    Text(String),
    /// Text that may hold line breaks (a multi-line string literal), written
    /// byte for byte; never trimmed.
    Verbatim(String),
    /// A space when its group is flat, a line break when it is broken.
    Line,
    /// Nothing when flat, a line break when broken.
    SoftLine,
    /// Always a line break; breaks every group around it.
    HardLine,
    /// A line break to column 0, whatever the indentation (a block comment's
    /// continuation line, which carries its own indentation).
    LiteralLine,
    /// `broken` when the enclosing group is broken, else `flat`.
    IfBreak { broken: String, flat: String },
    /// Text written at the end of the current line, before its line break,
    /// that never counts toward the width (a trailing comment).
    LineSuffix(String),
    /// Breaks every group around it and writes nothing.
    BreakParent,
    /// Line breaks inside start `columns` further in.
    Indent(u32, Box<Document>),
    /// Flat if it fits on the line and holds nothing that forces a break,
    /// else broken, as a whole.
    Group(Box<Document>),
    /// The first alternative whose first line fits, else the last one.
    Choice(Vec<Document>),
    Concat(Vec<Document>),
}

/// Lays `document` out within `width` columns, each line break written as
/// `line_ending`, trailing spaces removed from every line.
pub(crate) fn layout(document: &Document, width: u32, line_ending: &str) -> String;
```

Constructors as free functions for readability in the lowering:
`text(impl Into<String>)`, `concat(Vec<Document>)`, `group(Document)`,
`indent(u32, Document)`, `line()`, `soft_line()`, `hard_line()`.

- [ ] **Step 1: Write the failing tests** (in `document.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn words(parts: &[&str]) -> Document {
        let mut items = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                items.push(Document::Line);
            }
            items.push(text(*part));
        }
        group(concat(items))
    }

    #[test]
    fn a_group_that_fits_stays_flat() {
        assert_eq!(layout(&words(&["a", "b", "c"]), 10, "\n"), "a b c");
    }

    #[test]
    fn a_group_that_does_not_fit_breaks_as_a_whole() {
        assert_eq!(layout(&words(&["aaaa", "bbbb", "cccc"]), 10, "\n"), "aaaa\nbbbb\ncccc");
    }

    #[test]
    fn the_outer_group_breaks_before_the_inner_one() {
        let document = group(concat(vec![
            text("receiver"),
            indent(4, concat(vec![Document::Line, words(&["at:", "1"]), Document::Line, words(&["put:", "2"])])),
        ]));
        assert_eq!(layout(&document, 14, "\n"), "receiver\n    at: 1\n    put: 2");
    }

    #[test]
    fn a_hard_line_breaks_every_group_around_it() {
        let document = group(concat(vec![text("a"), Document::Line, text("b"), Document::HardLine, text("c")]));
        assert_eq!(layout(&document, 100, "\n"), "a\nb\nc");
    }

    #[test]
    fn if_break_follows_its_group() {
        let flat = group(concat(vec![text("{a"), Document::IfBreak { broken: ".".into(), flat: String::new() }, text("}")]));
        assert_eq!(layout(&flat, 100, "\n"), "{a}");
        let broken = group(concat(vec![text("{a"), Document::IfBreak { broken: ".".into(), flat: String::new() }, Document::HardLine, text("}")]));
        assert_eq!(layout(&broken, 100, "\n"), "{a.\n}");
    }

    #[test]
    fn a_line_suffix_does_not_count_and_lands_before_the_break() {
        let document = concat(vec![
            group(concat(vec![text("aaaaaaaa"), Document::LineSuffix(" /// note".into()), Document::Line, text("b")])),
            Document::HardLine,
            text("c"),
        ]);
        // the suffix is not measured: "aaaaaaaa b" fits in 10
        assert_eq!(layout(&document, 10, "\n"), "aaaaaaaa b /// note\nc");
    }

    #[test]
    fn break_parent_breaks_its_group() {
        let document = group(concat(vec![text("a"), Document::LineSuffix(" /// x".into()), Document::BreakParent, Document::Line, text("b")]));
        assert_eq!(layout(&document, 100, "\n"), "a /// x\nb");
    }

    #[test]
    fn choice_takes_the_first_alternative_whose_first_line_fits() {
        let long = group(concat(vec![text("x"), Document::Line, text("yyyyyyyyyyyy")]));
        let hugged = concat(vec![text("x ["), indent(2, concat(vec![Document::HardLine, text("body")])), Document::HardLine, text("]")]);
        let document = Document::Choice(vec![long, hugged, text("fallback")]);
        assert_eq!(layout(&document, 8, "\n"), "x [\n  body\n]");
    }

    #[test]
    fn indentation_is_two_spaces_per_level_and_blank_lines_carry_none() {
        let document = concat(vec![text("("), indent(2, concat(vec![Document::HardLine, text("a"), Document::HardLine, Document::HardLine, text("b")])), Document::HardLine, text(")")]);
        assert_eq!(layout(&document, 100, "\n"), "(\n  a\n\n  b\n)");
    }

    #[test]
    fn line_endings_are_the_callers_and_verbatim_text_is_untouched() {
        let document = concat(vec![text("a"), Document::HardLine, Document::Verbatim("'x  \ny'".into()), Document::HardLine, text("b")]);
        assert_eq!(layout(&document, 100, "\r\n"), "a\r\n'x  \ny'\r\nb");
    }

    #[test]
    fn a_literal_line_returns_to_column_zero() {
        let document = indent(4, concat(vec![text("(* a"), Document::LiteralLine, text("   b *)")]));
        assert_eq!(layout(&document, 100, "\n"), "(* a\n   b *)");
    }

    #[test]
    fn width_counts_chars_not_bytes() {
        assert_eq!(layout(&words(&["äöü", "äöü"]), 7, "\n"), "äöü äöü");
    }
}
```

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** the Prettier algorithm (`printDocToString`):
  a stack of `(indentation, mode, &Document)`, modes `Flat`/`Break`.
  First a pass that marks every `Group` holding a `HardLine`, a `BreakParent`
  or a `Verbatim` with a line break (outside a `Choice`) as broken — store the
  marks in a `HashSet` of node addresses or rebuild the tree with a
  `Group { content, broken }` internal form; keep `Document` itself as
  declared. `Group` in `Break` mode is laid out flat when not marked and
  `fits`. `fits` measures from the current column: the candidate first, then
  the rest of the stack in its own modes, until the first line break in
  `Break` mode, a `HardLine`/`LiteralLine`, or the end; inside it a group
  marked broken is measured in `Break` mode, an unmarked one in its parent's
  mode; `LineSuffix` and `BreakParent` measure nothing; a `Verbatim`
  measures up to its first line break and then answers "fits". A `Group` in
  `Flat` mode is laid out flat unless marked, then broken. Breaking
  propagation does not pass through a `Choice` (a `HardLine` inside an
  alternative marks only the groups inside that alternative). `Choice` in
  `Flat` mode lays out its first alternative flat. `Choice` in `Break` mode
  tries each alternative but the last with `fits` in `Flat` mode — so an
  unmarked group must fit entirely flat, while a marked one ends the
  measurement at its first line break — and lays out the first that fits in
  `Flat` mode (its marked groups still break); if none fits, the last one in
  `Break` mode. Before every line break, write the pending line suffixes, then
  remove trailing spaces back to the last `Verbatim`'s end, then the line
  ending and the indentation (none for `LiteralLine`). At the end, flush the
  suffixes and trim. The whole layout is iterative (an explicit stack), so a
  deep document cannot overflow the stack.
- [ ] **Step 4: Run the full list.**
- [ ] **Step 5: Commit** — `feat: a document algebra and layout for the printer`

---

### Task 7: Printing declarations

**Files:**
- Create: `crates/ns-syntax/src/printer/declarations.rs`, `crates/ns-syntax/tests/format_rules.rs`
- Modify: `crates/ns-syntax/src/printer/mod.rs`, `crates/ns-syntax/src/lib.rs`

**Interfaces:**
- Consumes: Task 6's `Document`, `layout`; the AST of Tasks 1–5.
- Produces (crate root):

```rust
/// How the printer ends its lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding { Lf, CrLf, Cr }

impl LineEnding {
    /// The first line ending of `source`, `Lf` when it has none.
    pub fn of(source: &str) -> LineEnding;
    pub fn as_str(self) -> &'static str;
}

/// `module` printed in the one layout this printer has
/// (`docs/design/formatting.md`), ending with one line ending.
pub fn print_module(module: &Module, line_ending: LineEnding) -> String;

/// `source` reprinted, or the diagnostics refusing it: a source with any
/// error is never printed.
pub fn format(source: &str) -> Result<String, Vec<Diagnostic>>;
```

In this task `declarations.rs` lowers everything but expressions. Create
`expressions.rs` with a first real version — identifiers, literals (their
`text`), unary, binary and keyword sends and setters, all on one line —
enough for the declaration tests; Task 8 adds breaking, blocks, cascades,
tuples and parentheses. Comments are printed from Task 9 on; until then the
tests here use sources without comments.

- [ ] **Step 1: Write the failing tests** (`format_rules.rs`; every test is
  `assert_eq!(formatted(input), expected)` with `fn formatted(source: &str)
  -> String { match ns_syntax::format(source) { Ok(text) => text, Err(d) =>
  panic!("{d:?}") } }`). One test per rule 2, 3, 5, 6 (header initializers),
  7, 8, 9, 10, 20 (patterns and slots), 24. For example:

```rust
#[test]
fn rule_5_slots_are_one_per_line_between_bars() {
    let input = "class Counter = ( | private count ::= 0. public step = 1. | ) ()";
    let expected = indoc! {"
        class Counter = (
          |
            private count ::= 0.
            public step = 1.
          |
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_7_members_keep_their_order_with_one_blank_line_between() {
    let input = "class A = () ( public b = ( ^2 ) public a = ( ^1 ) )";
    let expected = indoc! {"
        class A = () (
          public b = (
            ^2.
          )

          public a = (
            ^1.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_8_empty_parts() {
    assert_eq!(formatted("class Top = ( ) ( ) : ( )"), "class Top = () ()\n");
}

#[test]
fn rule_3_a_long_header_breaks_before_equals_then_at_every_keyword() {
    let input = "class NewspeakPrettyPrinter usingPlatform: platform ast: astModule parser: commonParser metadata: mdModule = Object new ()()";
    let expected = indoc! {"
        class NewspeakPrettyPrinter
            usingPlatform: platform
            ast: astModule
            parser: commonParser
            metadata: mdModule
            = Object new () ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_3_a_header_that_fits_after_breaking_before_equals() {
    let input = "class LongEnoughName usingPlatform: platform ast: astModule parser: commonParser metadata: mdModule = Object new ()()";
    // the pattern alone is 99 columns, the whole line 118
    let expected = indoc! {"
        class LongEnoughName usingPlatform: platform ast: astModule parser: commonParser metadata: mdModule
            = Object new () ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_9_a_long_method_pattern_puts_each_keyword_on_its_own_line() {
    let input = "class A = () ( public copyReplaceFrom: start to: stop with: replacement startingAt: replacementStart ifAbsent: absentBlock = ( ^start ) )";
    let expected = indoc! {"
        class A = () (
          public copyReplaceFrom: start
              to: stop
              with: replacement
              startingAt: replacementStart
              ifAbsent: absentBlock = (
            ^start.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_10_temporaries_are_always_opened_up_and_an_empty_body_is_inline() {
    let input = "class A = () ( public m = ( | current | current:: 0. ^current ) public noop = ( ) )";
    let expected = indoc! {"
        class A = () (
          public m = (
            |
              current
            |
            current:: 0.
            ^current.
          )

          public noop = ()
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_2_the_prologue_as_written() {
    assert_eq!(formatted("Newspeak3 'Root' class A = () ()"), "Newspeak3\n'Root'\nclass A = () ()\n");
    assert_eq!(formatted("class A = () ()"), "class A = () ()\n");
}

#[test]
fn rule_20_types_in_patterns_and_slots() {
    let input = "class Map <[K,V]> new: capacity<Integer> = ( | size<Integer> ::= 0. | ) ( public at: key<K> ^<V|Nil> = ( ^nil ) )";
    let expected = indoc! {"
        class Map <[K, V]> new: capacity <Integer> = (
          |
            size <Integer> ::= 0.
          |
        ) (
          public at: key <K> ^<V | Nil> = (
            ^nil.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}
```

Also: rule 6 (header initializer expressions one per line at +2 after the
closing bar, each with a period), a class side with members
(`) : (` … `)`), a nested class indented one level with a blank line around
it as a member, a lazy slot member (`lazy public cache = 3.`), a
mixin application without a body (`class C = A <: B.` and one past 100
columns breaking before each `<:`), and `||` simultaneous slots kept.

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** `declarations.rs` as functions from AST nodes to
  `Document` (`module`, `class_declaration`, `class_header`, `slots`,
  `side`, `member`, `method`, `pattern`, `code_body`, `type_annotation`),
  `print_module` (lower, `layout` at width 100 with the line ending, then
  ensure exactly one final line ending), `format` (`parse`, then
  `print_module(&module, LineEnding::of(source))`). Type annotation spacing
  (rule 20): join tokens with no space, except one space after `,`, one on
  each side of a `|` and of `<:`; one space before the annotation's opening
  `<` when it follows a name, none after `^`. The class header (rule 3) is a
  `Choice` of: everything on one line; break before `=`; every factory
  keyword on its own line plus the break before `=`.
- [ ] **Step 4: Run the full list.**
- [ ] **Step 5: Commit** — `feat: print declarations`

---

### Task 8: Printing expressions

**Files:**
- Modify: `crates/ns-syntax/src/printer/expressions.rs`, `crates/ns-syntax/tests/format_rules.rs`

**Interfaces:**
- Consumes: Task 6, Task 7's `declarations.rs` (calls `expressions::statement`
  and `expressions::expression`).
- Produces: `pub(super) fn statement(&Statement) -> Document`,
  `pub(super) fn expression(&Expression) -> Document`.

- [ ] **Step 1: Write the failing tests**, one per rule 11–19 and 25, built
  from the examples in "The design". In every test the expression stands in
  a method `m` of `class A = () (…)` so the expected text carries its real
  indentation. The two helpers, in `format_rules.rs`:

```rust
/// `statements` as the body of a method `m` of a class `A`, unformatted.
fn in_method(statements: &str) -> String {
    format!("class A = () (\npublic m = (\n{statements}\n)\n)\n")
}

/// What `in_method` formats to when the body formats to `lines` (each line
/// ending with a line break, indented from column 0).
fn method_text(lines: &str) -> String {
    let body: String = lines
        .lines()
        .map(|line| if line.is_empty() { "\n".to_string() } else { format!("    {line}\n") })
        .collect();
    format!("class A = () (\n  public m = (\n{body}  )\n)\n")
}
```

  Required cases:

```rust
#[test]
fn rule_12_a_long_binary_chain_breaks_before_each_operator() {
    let input = in_method("message:: 'expected ' , expected printString , ' but got ' , actual printString , ' instead of it'");
    let expected = method_text(indoc! {"
        message:: 'expected '
            , expected printString
            , ' but got '
            , actual printString
            , ' instead of it'.
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_13_block_arguments_hug_their_keywords() {
    let input = in_method("count > limit ifTrue: [ self reset. ^nil ] ifFalse: [ count:: count + 1 ]");
    let expected = method_text(indoc! {"
        count > limit ifTrue: [
          self reset.
          ^nil.
        ] ifFalse: [
          count:: count + 1.
        ].
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_13_a_keyword_message_that_does_not_fit_breaks_after_the_first_keyword() {
    let input = in_method("result:: dictionary at: someRatherLongKeyName ifAbsent: someRatherLongDefaultValue ifPresent: transformation");
    let expected = method_text(indoc! {"
        result:: dictionary at: someRatherLongKeyName
            ifAbsent: someRatherLongDefaultValue
            ifPresent: transformation.
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_14_cascades_always_break() {
    let input = in_method("stream nextPutAll: 'a'; print: 3; yourself");
    let expected = method_text(indoc! {"
        stream
            nextPutAll: 'a';
            print: 3;
            yourself.
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_15_blocks_are_tight_and_one_line_only_with_one_statement() {
    assert_eq!(
        formatted(&in_method("items do: [ :each | each reset ]")),
        method_text("items do: [:each | each reset].\n")
    );
    let expected = method_text(indoc! {"
        items do: [:each |
          each reset.
          each start.
        ].
    "});
    assert_eq!(formatted(&in_method("items do: [:each | each reset. each start]")), expected);
    assert_eq!(formatted(&in_method("x:: [ ]. y:: [ :each | ]")), method_text("x:: [].\ny:: [:each |].\n"));
}

#[test]
fn rule_15_a_block_with_temporaries_is_multi_line() {
    let expected = method_text(indoc! {"
        items do: [:each |
          |
            sum
          |
          sum:: each * 2.
          total:: total + sum.
        ].
    "});
    assert_eq!(formatted(&in_method("items do: [:each | | sum | sum:: each * 2. total:: total + sum]")), expected);
}

#[test]
fn rule_15_a_receiver_block_opens_at_the_line_end() {
    let expected = method_text(indoc! {"
        [
          self step.
          self isDone not.
        ] whileTrue.
    "});
    assert_eq!(formatted(&in_method("[self step. self isDone not] whileTrue")), expected);
}

#[test]
fn rule_16_parentheses_are_kept() {
    assert_eq!(formatted(&in_method("^( ( a foo ) bar )")), method_text("^((a foo) bar).\n"));
}

#[test]
fn rule_17_tuples_are_blocks_of_elements() {
    assert_eq!(formatted(&in_method("x:: { 1 . 2 . 3 . }")), method_text("x:: {1. 2. 3}.\n"));
    let expected = method_text(indoc! {"
        expected:: {
          'first element of a tuple that is long enough'.
          'second element of a tuple that is long enough'.
        }.
    "});
    assert_eq!(formatted(&in_method("expected:: {'first element of a tuple that is long enough'. 'second element of a tuple that is long enough'}")), expected);
}

#[test]
fn rule_18_literals_are_kept_as_written() {
    assert_eq!(formatted(&in_method("^{16r1f. #'foo'. 'it''s'. \"c\". 1.5e3}")), method_text("^{16r1f. #'foo'. 'it''s'. \"c\". 1.5e3}.\n"));
}

#[test]
fn rule_18_a_multi_line_string_is_not_reindented() {
    let input = "class A = () (\n        public m = ( ^'one\n  two' )\n)\n";
    let expected = "class A = () (\n  public m = (\n    ^'one\n  two'.\n  )\n)\n";
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_19_and_25_spacing() {
    assert_eq!(formatted(&in_method("count::count+1. a   <-:   foo. ^ x at:1")), method_text("count:: count + 1.\na <-: foo.\n^x at: 1.\n"));
}

#[test]
fn rule_12_a_long_unary_chain_puts_each_selector_on_its_own_line() {
    let input = in_method("result:: someCollectionWithAVeryLongName asSortedCollection reversed asArray asOrderedCollection asSet");
    let expected = method_text(indoc! {"
        result:: someCollectionWithAVeryLongName
            asSortedCollection
            reversed
            asArray
            asOrderedCollection
            asSet.
    "});
    assert_eq!(formatted(&input), expected);
}
```

Also: a value-site type argument (`^Box[Integer] new`), an implicit-receiver
send printed without `self` (`foo: 3`), an explicit `self foo` kept, a
setter whose long value breaks inside the value, not after `::` (rule 19),
a binary operand that itself breaks its unary chain only after the binary
level broke (rule 12 "mixed"), and a 100,000-link chain formatted on a
256 KiB stack.

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** Statements: `^` + expression; the period after
  every statement of a multi-line sequence comes from the sequence (Task 7's
  `code_body`), the `IfBreak` for one-line blocks and tuples. Unary chain:
  `group(concat[receiver, indent(4, [line, sel1, line, sel2, …])])`, walked
  with `Send::receiver_chain` (never recursing along receivers). Binary:
  `group(concat[first, indent(4, [line, "op ", operand, …])])` over the
  left-nested chain, also via the chain walk. Keyword: the standard form
  `group(concat[receiver, " ", "kw1: ", arg1, indent(4, [line, "kw2: ", arg2, …])])`;
  when any argument is a block, a `Choice` of:
  1. the standard group — **left out when any block argument is forced
     multi-line** (temporaries, two or more statements, a comment forcing a
     break), because a marked group would "fit" on its first line;
  2. the hugged form: keyword parts separated by `" "` (no `Line`), every
     block argument lowered forced broken (a `BreakParent` inside its group),
     so its first line ends at `[`/`[:each |`;
  3. the standard form broken: `HardLine` instead of `Line` between keyword
     parts, blocks as ordinary groups.

  The `Choice` semantics of Task 6 then pick 1 when the whole message fits
  flat, 2 when the line up to the first `[` fits, else 3.
  A block: `group(["[", params, " |" if params, indent(2, [soft_line,
  statements…]), soft_line, "]"])` with a `HardLine` between statements and
  forced breaking when it has temporaries or more than one statement; a block
  with parameters and a flat body prints `[:each | body]` (the `Line` after
  `|` flat is a space). Cascades: `concat[receiver, indent(4, [hard_line,
  message, ";", hard_line, …])]`. Parentheses: `"(" … ")"` around the kind's
  document plus any `type_arguments`. Literals print `text` (`Verbatim` when
  the text holds a line break); `Boolean`/`Nil` their keyword. Synthesized
  receivers print nothing (and no space).
- [ ] **Step 4: Run the full list.**
- [ ] **Step 5: Commit** — `feat: print expressions`

---

### Task 9: Printing comments, line endings, and the properties over every source

**Files:**
- Create: `crates/ns-syntax/src/printer/comments.rs`, `crates/ns-syntax/tests/format_properties.rs`
- Modify: `crates/ns-syntax/src/printer/{mod,declarations,expressions}.rs`, `crates/ns-syntax/tests/format_rules.rs`

**Interfaces:**
- Consumes: Task 5's `AttachedComment`, `Anchor`, `Placement`.
- Produces: `pub(super) fn before(&[AttachedComment]) -> Document`,
  `after(&[AttachedComment]) -> Document`, `inner(&[AttachedComment], n: u32) -> Document`
  — each node's lowering wraps its document with `before` and `after` and
  calls `inner` after its `n`-th own token, counting exactly as `attach.rs`
  does.

- [ ] **Step 1: Write the failing tests** — rule 4 and 21 cases in
  `format_rules.rs`:

```rust
#[test]
fn rule_21_a_line_block_is_reindented_and_its_text_kept() {
    let input = "class A = () (\n        ///   indented text\n        /// second\n        public m = ()\n)\n";
    let expected = "class A = () (\n  ///   indented text\n  /// second\n  public m = ()\n)\n";
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_21_a_block_comment_moves_its_continuation_lines_with_it() {
    let input = "class A = () (\n        (* one\n           two *)\n        public m = ()\n)\n";
    let expected = "class A = () (\n  (* one\n     two *)\n  public m = ()\n)\n";
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_21_a_trailing_comment_does_not_count_toward_the_width() {
    let statement = "x:: someReceiverWithAName at: firstArgumentValue put: secondArgumentValue.";
    let input = in_method(&format!("{statement} /// a remark that pushes the line well past one hundred columns"));
    let expected = method_text(&format!("{statement} /// a remark that pushes the line well past one hundred columns\n"));
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_21_a_line_comment_inside_an_expression_breaks_its_level() {
    let input = in_method("result:: dictionary at: key /// the caller guarantees presence\n ifAbsent: [nil]");
    let expected = method_text(indoc! {"
        result:: dictionary at: key /// the caller guarantees presence
            ifAbsent: [nil].
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_21_an_inline_comment_stays_between_its_tokens() {
    assert_eq!(formatted(&in_method("self foo: (* unused *) nil bar: 3")), method_text("self foo: (* unused *) nil bar: 3.\n"));
}

#[test]
fn rule_21_metadata_stays_between_pattern_and_equals() {
    let input = "class A = () ( public foo: x (* :exemplar: A new foo: 3 *) = ( ^x ) )";
    let expected = "class A = () (\n  public foo: x (* :exemplar: A new foo: 3 *) = (\n    ^x.\n  )\n)\n";
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_4_the_class_comment_follows_the_open_parenthesis() {
    let input = "class Foo = Object new ( (* A counter. *) | private count ::= 0. | ) ()";
    let expected = indoc! {"
        class Foo = Object new (
          (* A counter. *)
          |
            private count ::= 0.
          |
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_8_a_part_with_only_a_comment_is_opened_up() {
    assert_eq!(formatted("class A = () () : ( (* nothing yet *) )"), "class A = () () : (\n  (* nothing yet *)\n)\n");
}

#[test]
fn rule_23_the_first_line_ending_decides() {
    assert_eq!(formatted("class A = () (\r\npublic m = ( ^1 ) )"), "class A = () (\r\n  public m = (\r\n    ^1.\r\n  )\r\n)\r\n");
}

#[test]
fn rule_23_a_string_keeps_its_own_line_breaks() {
    let input = "class A = () (\r\npublic m = ( ^'a\nb' ) )";
    let expected = "class A = () (\r\n  public m = (\r\n    ^'a\nb'.\r\n  )\r\n)\r\n";
    assert_eq!(formatted(input), expected);
}

#[test]
fn a_source_with_an_error_is_refused() {
    assert!(ns_syntax::format("class A = ( public m = ( ^").is_err());
}
```

And `format_properties.rs`, over every `.ns` under `corpus/modules`,
`goldens`, `benchmarks` and `crates/ns-runtime/core-library` (paths from
`env!("CARGO_MANIFEST_DIR")`, collected recursively, sorted; corpus files are
read, never written):

```rust
fn all_sources() -> Vec<(std::path::PathBuf, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for directory in ["corpus/modules", "goldens", "benchmarks", "crates/ns-runtime/core-library"] {
        collect(&root.join(directory), &mut files);
    }
    files.sort();
    assert!(files.len() > 590, "found only {} sources", files.len());
    files
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(&path).expect("a readable source");
            (path, source)
        })
        .collect()
}

/// Every `.ns` file under `directory`, recursively (`ns-syntax` cannot
/// depend on `ns-tooling`, which has the same walk).
fn collect(directory: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("a readable directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "ns") {
            files.push(path);
        }
    }
}

#[test]
fn formatting_is_idempotent() {
    for (path, source) in all_sources() {
        let once = ns_syntax::format(&source).unwrap_or_else(|_| panic!("{} parses", path.display()));
        let twice = ns_syntax::format(&once).unwrap_or_else(|_| panic!("{} reparses", path.display()));
        assert!(once == twice, "{} is not a fixed point after one format", path.display());
    }
}

#[test]
fn formatting_preserves_the_module() {
    for (path, source) in all_sources() {
        let before = ns_syntax::parse(&source).unwrap_or_else(|_| panic!("{} parses", path.display()));
        let formatted = ns_syntax::format(&source).unwrap_or_else(|_| panic!("{} formats", path.display()));
        let after = ns_syntax::parse(&formatted).unwrap_or_else(|_| panic!("{} reparses", path.display()));
        assert!(
            before.without_positions() == after.without_positions(),
            "{} says something else after formatting",
            path.display()
        );
    }
}

#[test]
fn a_long_chain_formats_on_a_small_stack() {
    let mut source = String::from("class A = () (\npublic m = ( ^1");
    for _ in 0..100_000 {
        source.push_str(" + 1");
    }
    source.push_str(" )\n)\n");
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let once = ns_syntax::format(&source).unwrap_or_else(|_| panic!("the chain formats"));
            let twice = ns_syntax::format(&once).unwrap_or_else(|_| panic!("the chain reformats"));
            assert!(once == twice, "the chain is not a fixed point");
        })
        .expect("spawn")
        .join()
        .expect("formatting a long chain stays on a small stack");
}
```

On a failure, the message must name the file; for the preservation test,
also print the first differing comment or statement (write a small
`first_difference` helper over the two modules' `{:?}` strings: the first
differing line with three lines of context). `Diagnostic`'s and the AST's
`Debug` are fine to print — neither is a `Value`.

- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** `comments.rs`: `OwnLine` → a `HardLine` (unless
  at the start of a line already), the comment, a `HardLine`; `EndOfLine` →
  `LineSuffix(" " + comment)` and `BreakParent`; `Inline` on `Before` → the
  comment and `" "` in front of the node (the node's own leading separator
  supplies the space before the comment); `Inline` on `After` → `" "` and the
  comment after it; `Inline` on `Inner` → the comment between single spaces,
  none next to an opening delimiter. So `foo: (* unused *) nil` and
  `( (* x *) a)` → `((* x *) a)`; pin both in tests. A `Line` block prints
  `///` + each stored line; a `Block` prints its first line, then each
  continuation line after a `LiteralLine`, shifted by the difference between
  the column it is printed at and `column` (rule 21; never below column 0).
  A member's leading comments sit between the blank line and the member. Wire
  `before`/`after`/`inner` into every node's lowering. `print_module` takes
  the line ending from the caller; `format` from `LineEnding::of`.
  Fix every failure the property tests report in the printer — never by
  excluding a file.
- [ ] **Step 4: Run the full list** (the property tests run in it).
- [ ] **Step 5: Commit** — `feat: print comments and hold the printer to every source`

---

### Task 10: `ns-next-analyzer fmt`

**Files:**
- Create: `crates/ns-analyzer/src/commands/fmt.rs`, `crates/ns-analyzer/tests/fmt_command.rs`
- Modify: `crates/ns-analyzer/src/main.rs`, `crates/ns-analyzer/src/commands/mod.rs`

**Interfaces:**
- Consumes: `ns_syntax::format`, `ns_tooling::source::{collect_ns_files, print_diagnostics}`.
- Produces: `Command::Fmt { check: bool, paths: Vec<PathBuf> }` →
  `commands::fmt::execute(&paths, check) -> ExitCode`.

- [ ] **Step 1: Write the failing tests** (`tests/fmt_command.rs`, running
  the binary with `env!("CARGO_BIN_EXE_ns-next-analyzer")` over a temporary
  directory made with `std::env::temp_dir()` plus a unique name — copy the
  pattern of an existing analyzer CLI test if one exists):
  - `fmt` rewrites an unformatted file and leaves a formatted one's
    modification time unchanged; exit 0.
  - `fmt --check` on an unformatted file writes nothing, prints its path,
    exits 1; on formatted files exits 0 and prints nothing.
  - a file with a syntax error is left byte for byte, its diagnostic printed
    to stderr, exit 1, and a second, valid file in the same run is still
    formatted.
  - no path → the current directory (run the binary with `current_dir`).
  - a CRLF file stays CRLF.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement.** Paths: a file is taken as given; a directory
  through `collect_ns_files`; sort and deduplicate. Write by creating
  `<name>.fmt-temporary` beside the file and `std::fs::rename` over it. Read
  failures print `error: cannot read <path>: <error>` and count as failures.
  `--help` text: "Formats Newspeak sources in the one layout ns-next has".
- [ ] **Step 4: Run the full list.**
- [ ] **Step 5: Commit** — `feat: add ns-next-analyzer fmt`

---

### Task 11: `textDocument/formatting`

**Files:**
- Modify: `crates/ns-analyzer/src/server.rs`
- Test: the server's existing test file (find where `document_symbols_of` or
  folding requests are tested — `crates/ns-analyzer/tests/` — and add beside it)

**Interfaces:**
- Consumes: `ns_syntax::format`, the server's `Document { text, line_index }`.
- Produces: capability `document_formatting_provider: Some(OneOf::Left(true))`;
  request `Formatting::METHOD` answered by `formatting_of`.

- [ ] **Step 1: Write the failing tests:** an open unformatted document
  answers one `TextEdit` whose range spans the whole document and whose
  `new_text` equals `ns_syntax::format(text)`; a formatted document answers
  an empty list; a document with a syntax error answers an empty list; an
  unopened URI answers an empty list; the request's `options` (tab size 8,
  spaces false) change nothing; `initialize` announces the capability.
- [ ] **Step 2: Run them to see them fail.**
- [ ] **Step 3: Implement** `formatting_of` after the pattern of
  `folding_ranges_of` (params error → `InvalidParams`). The range ends at the
  `line_index` position of `text.len()`. Answer `Vec<TextEdit>` (serialised
  as an array).
- [ ] **Step 4: Run the full list, including `gradlew check`** (the plugin's
  tests start the server).
- [ ] **Step 5: Commit** — `feat: answer textDocument/formatting`

---

### Task 12: Reformat the repository and gate it

**Files:**
- Modify: every `.ns` under `goldens/`, `benchmarks/`, `crates/ns-runtime/core-library/` (mechanically, by the tool)
- Create: `crates/ns-analyzer/tests/formatting_gate.rs`
- Modify: `crates/ns-cli/tests/source_formatting.rs`

- [ ] **Step 1: Reformat.** `cargo run -p ns-analyzer -- fmt goldens benchmarks crates/ns-runtime/core-library` (timeout 600000). Expected: exit 0.
- [ ] **Step 2: Check what changed beyond layout.** `git diff --stat`; then
  run the full list. Every golden must pass unchanged under both lowerings
  (no `.toml` may change); `crates/ns-runtime/tests/core_library_documentation.rs`
  must pass. A snapshot holding spans or lines of the core library may change
  (`cargo insta` diff): read each, accept only position shifts. Anything else
  that changes is a printer defect: fix it in the printer (with a rule test)
  and repeat from Step 1 on a clean checkout of those files.
- [ ] **Step 3: Commit the reformat alone** — `style: format every Newspeak source with ns-next-analyzer fmt`
- [ ] **Step 4: Write the gate** (`formatting_gate.rs`):

```rust
//! Every Newspeak source of the repository outside `corpus/` and
//! `prior-art/` is written exactly as `ns-next-analyzer fmt` writes it.

#[test]
fn every_repository_source_is_formatted() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for directory in ["goldens", "benchmarks", "crates/ns-runtime/core-library"] {
        ns_tooling::source::collect_ns_files(&root.join(directory), &mut files);
    }
    files.sort();
    assert!(files.len() > 380, "the gate found only {} sources", files.len());
    let mut unformatted = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("a readable source");
        match ns_syntax::format(&source) {
            Ok(formatted) if formatted == source => {}
            Ok(formatted) => unformatted.push(format!(
                "{}: first difference at line {}",
                file.display(),
                first_differing_line(&source, &formatted)
            )),
            Err(_) => unformatted.push(format!("{}: does not parse", file.display())),
        }
    }
    assert!(unformatted.is_empty(), "run `ns-next-analyzer fmt`:\n{}", unformatted.join("\n"));
}

fn first_differing_line(left: &str, right: &str) -> usize {
    left.lines()
        .zip(right.lines())
        .position(|(first, second)| first != second)
        .unwrap_or_else(|| left.lines().count().min(right.lines().count()))
        + 1
}
```

  (`ns-tooling` is already a dependency of `ns-analyzer`; check
  `Cargo.toml` — if only a normal dependency, it is usable from tests.)
- [ ] **Step 5: Trim `source_formatting.rs`** to the rule the formatter does
  not enforce: no `(* … *)` outside `corpus/` except
  `goldens/language/syntax/line_comment.ns`. Delete the four-spaces and
  no-tabs checks and update its module doc.
- [ ] **Step 6: Run the full list.**
- [ ] **Step 7: Commit** — `test: hold every repository source to the formatter`

---

### Task 13: Documentation and finishing

**Files:**
- Create: `docs/design/formatting.md`
- Modify: `docs/design/frontend.md`, `docs/design/editor-support.md`,
  `docs/design/README.md`, `CLAUDE.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`

- [ ] **Step 1: `docs/design/formatting.md`** — the living document of the
  formatter: purpose (tool and gate, no options, why), the 25 rules with
  their reasons (from "The design"), the algorithm (document algebra, outside
  in, all or nothing, block hugging via `Choice`, line suffixes, linear
  layout), the guarantees and the tests that hold them, the line-ending rule,
  what is deliberately not done (range and on-type formatting, wrapping
  comments, normalising literals, reordering members). Current state only;
  name no other implementation in a way that makes it a source (the prior-art
  comparison may be cited as `prior-art/newspeak/NewspeakPrettyPrinter.ns`
  in a "Prior art" paragraph, per `docs/design/README.md`'s practice).
- [ ] **Step 2: `frontend.md`** — "AST": the envelope, statements, members,
  type annotations, literal spellings, the prologue, `eventual`,
  `synthesized`; "Comments": attachment (the rule from Task 5); "What is
  parsed and dropped": only the empty slot list, the empty class side and
  the last period remain dropped; `<-:` is recorded but run synchronously.
  `editor-support.md`: formatting moves from "Planned later" to the present
  capabilities (the request, its answers, the untested paths: the plugin's
  Reformat Code end to end unless Task 11 tested it). `README.md`: the map
  gains `formatting.md`; the deviation table's `<-:` row stays as it is.
- [ ] **Step 3: `CLAUDE.md`** — `ns-syntax`'s description gains the printer
  (`printer/`, `print_module`, `format`) and the lossless AST; Conventions:
  "Newspeak sources (`.ns`) outside `corpus/` are formatted by
  `ns-next-analyzer fmt` (two spaces, `docs/design/formatting.md`) and
  commented with `///`…"; `ns-analyzer`'s command list gains `fmt`, its LSP
  list formatting. Keep every invariant paragraph unchanged except where it
  names `Expression::…` variants (now `ExpressionKind::…`).
- [ ] **Step 4: `OPEN-WORK.md`** — remove the Formatting item; add
  "Range formatting (`textDocument/rangeFormatting`) and formatting on type"
  under "Planned: editor support and the analyzer". `PROGRESS.md`: one entry
  for the formatter batch in the existing style.
- [ ] **Step 5: Commit** — `docs: describe the lossless AST and the formatter`
  (Markdown only: no gates; check with `git diff --stat`).
- [ ] **Step 6: Finish the batch** with the `finishing-a-batch` skill (the
  five stress modes, `run-corpus` plain and under stress, the benchmarks
  profile, `verifyPlugin`, the fresh-session documentation check).
