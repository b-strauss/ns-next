# Code Folding, Comment Spans and a Fault-Tolerant Parse Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold comments, class bodies, class sides, method bodies, closures and slot sections in the JetBrains plugin, from fold ranges the language server computes from an AST that survives syntax errors.

**Architecture:** `ns-syntax` keeps one parser. It gains bracket spans in the AST (`ast::Delimiters`), comment spans beside the tokens, a recovery that resynchronises on balanced brackets and member starts, and a second entry, `parse_for_editor`, that always answers a module. `ns-lsp` analyses through that entry — resolver, semantic tokens and the new fold ranges on every version, recovered or not — and answers `textDocument/foldingRange`. The JetBrains platform's `LspFoldingBuilder` shows them with no plugin code.

**Tech Stack:** Rust 1.98 (edition 2021), `lsp-types` 0.97, `lsp-server` 0.10, `cargo-nextest`; the JetBrains plugin (Kotlin, Gradle) only for its description and gate.

**Spec:** `docs/superpowers/specs/2026-10-03-editor-folding-design.md`

## Global Constraints

- `ns_syntax::parse` keeps its contract: `Err` on any error-severity diagnostic. Nothing that compiles ever sees a recovered module.
- On a source without errors the parser's behaviour does not change: every recovery rule acts only where an error is reported. `cargo run -p ns-cli -- parse-corpus corpus/modules` stays at "parsed 211, failed 0" and every golden keeps its output.
- The token stream does not change; `crates/ns-syntax/src/lexer.rs` changes only to record comment spans, so `NewspeakLexer.kt` needs no mirror change and `LexerComparisonTest` stays green.
- Recovery never invents a declaration with an empty name; the one exception is a root class with no name.
- Names are written out (`CLAUDE.md`, "Conventions"): no identifier of one or two characters, no truncated words (`diagnostic`, not `diag`; `index`, not `i`). Clippy's `min_ident_chars` enforces it.
- Comments explain the decision, not the mechanics. Inline Newspeak in tests is an `indoc! {"…"}` block, indented with the Rust around it.
- No new external dependency.
- Per-commit gates for every task here (each touches `ns-syntax` or `ns-lsp`):
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace --exclude ns-lsp
  cargo test --workspace --exclude ns-lsp --doc
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  cargo nextest run -p ns-lsp
  ./editors/jetbrains/gradlew -p editors/jetbrains check
  ```
  Run every command in the foreground. A commit that has not seen the end-of-batch gates (the `finishing-a-batch` skill, Task 10) is work in progress.
- Commit messages are conventional commits ending with:
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_014iiGvJnqAeLJtzhwKA6Poa
  ```

## Review Focus

1. A source with lexer errors of every kind — unterminated string and comment, a bare `#`, `:=`, `#(`, an unexpected character — must parse for the editor without a panic and keep the members around the error (test in Task 2).
2. Fold positions on a file with `\r\n` line ends and non-ASCII text before the fold must be right in UTF-16 units on the right lines (test in Task 8).
3. An unterminated comment at the end of a file must fold to the end of the file, not vanish (test in Task 8).
4. An empty file, or one of whitespace only, must answer a module, no fold and no panic (tests in Tasks 2 and 8).
5. Text cut in the middle of a token — inside a string, a comment, a keyword — must not produce an internal error; the existing truncation test cuts only at token ends (test in Task 7).

---

## File Structure

- `crates/ns-syntax/src/ast.rs` — `Delimiters`, and the fields that hold it (Task 1).
- `crates/ns-syntax/src/lexer.rs` — comment spans, `Lexed`, `run_keeping_comments` (Task 2).
- `crates/ns-syntax/src/lib.rs` — `EditorParse`, `parse_for_editor` (Task 2).
- `crates/ns-syntax/src/parser/mod.rs` — `previous_span` (Task 4).
- `crates/ns-syntax/src/parser/declaration.rs` — delimiters (Task 1), `at_member_start`, `skip_balanced`, the recovery of bodies and class sides (Task 4), slot sections and type annotations (Task 5), no invented declarations (Task 6).
- `crates/ns-syntax/src/parser/expression.rs` — delimiters of blocks (Task 1), depth guards (Task 3), parentheses, blocks, tuples (Task 5), cascades and block parameters (Task 6).
- `crates/ns-syntax/tests/recovery.rs` — new: the recovery tests, through `parse_for_editor` (Tasks 2–6).
- `crates/ns-runtime/src/compiler/simultaneous.rs` — the two new fields on the block it builds (Task 1).
- `crates/ns-lsp/src/analysis.rs` — analysis through `parse_for_editor`, combined findings, folds (Tasks 7, 8).
- `crates/ns-lsp/src/folding.rs` — new: fold ranges and their LSP encoding (Task 8).
- `crates/ns-lsp/src/server.rs` — the capability and the request (Task 8).
- `crates/ns-lsp/tests/corpus.rs`, `crates/ns-lsp/tests/protocol.rs` — corpus-wide and protocol tests (Tasks 7, 8).
- `editors/jetbrains/src/main/resources/META-INF/plugin.xml`, `editors/jetbrains/README.md` — description (Task 9).
- `docs/design/editor-support.md`, `docs/design/frontend.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md` — documentation (Task 9).

---

### Task 1: Bracket spans in the AST

**Files:**
- Modify: `crates/ns-syntax/src/ast.rs`
- Modify: `crates/ns-syntax/src/parser/declaration.rs` (`parse_method_declaration`, `parse_slot_declarations`, `parse_slot_section`, `parse_temporaries`, `parse_code_body`, `parse_class_declaration`, `parse_header_body`, `parse_class_side_body`, the test helper `parse_slot_declarations_for_test`)
- Modify: `crates/ns-syntax/src/parser/expression.rs` (`parse_block`)
- Modify: `crates/ns-runtime/src/compiler/simultaneous.rs`
- Test: `crates/ns-syntax/src/parser/declaration.rs` (`class_tests`)

**Interfaces:**
- Produces:
  ```rust
  pub struct Delimiters { pub open: Span, pub close: Option<Span> }   // Debug, Clone, Copy, PartialEq, Eq
  ClassBody.delimiters: Option<Delimiters>        // instance side: G1 `(` .. G2 `)`; class side: `(` .. `)`
  ClassBody.slot_delimiters: Option<Delimiters>   // the header's slot section `| |` / `|| ||`
  MethodDeclaration.body_delimiters: Option<Delimiters>   // `(` .. `)`
  CodeBody.temporaries_delimiters: Option<Delimiters>     // `| |`
  BlockExpression.delimiters: Option<Delimiters>          // `[` .. `]`
  ```
  `parse_slot_declarations(&mut self) -> (Vec<SlotDeclaration>, Option<Delimiters>)`.

- [ ] **Step 1: Write the failing test**

Add to `mod class_tests` in `crates/ns-syntax/src/parser/declaration.rs`:

```rust
    /// The editor folds from an opening token to its closing one, so the
    /// AST records both for every construct that folds.
    #[test]
    fn every_folding_construct_records_its_brackets() {
        let source = indoc! {"
            class Module = (
                | first second |
            ) (
                run = (
                    | temporary |
                    ^[:each | each]
                )
            ) : (
                make = ( ^self new )
            )
        "};
        let module = module(source);
        let text_of = |delimiters: Option<Delimiters>| {
            let delimiters = delimiters.expect("the brackets are recorded");
            let close = delimiters.close.expect("the closing token is there");
            Span::new(delimiters.open.start, close.end).text(source).to_string()
        };
        let root = &module.root_class;
        let instance = text_of(root.instance_side.delimiters);
        assert!(instance.starts_with("(\n    | first second |\n) ("), "{instance}");
        assert!(instance.ends_with("^[:each | each]\n    )\n)"), "{instance}");
        assert_eq!(text_of(root.instance_side.slot_delimiters), "| first second |");
        let class_side = root.class_side.as_ref().expect("a class side");
        assert_eq!(
            text_of(class_side.delimiters),
            "(\n    make = ( ^self new )\n)"
        );
        let run = &root.instance_side.methods[0];
        let body = text_of(run.body_delimiters);
        assert!(body.starts_with("(\n        | temporary |"), "{body}");
        assert!(body.ends_with("each]\n    )"), "{body}");
        assert_eq!(text_of(run.body.temporaries_delimiters), "| temporary |");
        let Statement::Return(Expression::Block(block)) = &run.body.statements[0] else {
            panic!("a returned block");
        };
        assert_eq!(text_of(block.delimiters), "[:each | each]");
    }
```

Add `use crate::span::Span;` to the test module if `Span` is not in scope (the module has `use super::*;`, which brings `crate::ast::*` but not `Span`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p ns-syntax every_folding_construct_records_its_brackets`
Expected: compile error — `Delimiters` not found, no field `delimiters`.

- [ ] **Step 3: Add the type and the fields**

In `crates/ns-syntax/src/ast.rs`, after `Identifier`:

```rust
/// The opening and closing token of a bracketed construct, which an
/// editor folds between. `close` is `None` when the source has no closing
/// token there and the parser recovered without one; the construct then
/// ends where its node's span ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delimiters {
    pub open: Span,
    pub close: Option<Span>,
}
```

Fields (each with a one-line doc comment):

```rust
pub struct ClassBody {
    // … existing fields …
    /// The instance side's first `(` (the header's) and last `)` (the
    /// members'), or the class side's `( )`; `None` for a mixin
    /// application ended by `.`.
    pub delimiters: Option<Delimiters>,
    /// The header's slot section, `| |` or `|| ||`; `None` without one and
    /// on a class side.
    pub slot_delimiters: Option<Delimiters>,
}
pub struct MethodDeclaration {
    // … existing fields …
    /// The body's `( )`.
    pub body_delimiters: Option<Delimiters>,
}
pub struct CodeBody {
    // … existing fields …
    /// The temporaries' `| |`; `None` without any.
    pub temporaries_delimiters: Option<Delimiters>,
}
pub struct BlockExpression {
    // … existing fields …
    /// The block's `[ ]`; `None` for a block the compiler builds.
    pub delimiters: Option<Delimiters>,
}
```

- [ ] **Step 4: Record them in the parser**

In `declaration.rs`, `parse_slot_declarations` and `parse_slot_section` answer the section's delimiters:

```rust
    pub(crate) fn parse_slot_declarations(&mut self) -> (Vec<SlotDeclaration>, Option<Delimiters>) {
        if self.at(TokenKind::DoubleBar) {
            let checkpoint = self.checkpoint();
            let section = self.parse_slot_section(TokenKind::DoubleBar);
            if self.diagnostics.len() == checkpoint.1 {
                return section;
            }
            self.restore(checkpoint);
            let empty = self.bump(); // the empty sequential section
            return (
                Vec::new(),
                Some(Delimiters { open: empty.span, close: Some(empty.span) }),
            );
        }
        self.parse_slot_section(TokenKind::Bar)
    }

    fn parse_slot_section(&mut self, close: TokenKind) -> (Vec<SlotDeclaration>, Option<Delimiters>) {
        let Some(open) = self.eat_token(close) else {
            return (Vec::new(), None);
        };
        let mut slots = Vec::new();
        // … the loop, unchanged …
        let closing = self.expect(close);
        (
            slots,
            Some(Delimiters { open: open.span, close: closing.map(|token| token.span) }),
        )
    }
```

`eat_token` is new, next to `eat` in `parser/mod.rs`:

```rust
    /// `eat`, answering the token consumed.
    fn eat_token(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            None
        }
    }
```

`parse_temporaries` answers `(Vec<SlotDeclaration>, Option<Delimiters>)` the same way (`(Vec::new(), None)` without a section), and `parse_code_body` stores the second half in `temporaries_delimiters`. `parse_slot_declarations_for_test` answers `.0` of the pair so its callers stay as they are.

`parse_method_declaration`:

```rust
        self.expect(TokenKind::Equals);
        let open = self.expect(TokenKind::LeftParenthesis);
        let body = self.parse_code_body();
        let end = self.current().span;
        let close = self.expect(TokenKind::RightParenthesis);
        MethodDeclaration {
            access,
            pattern,
            body,
            body_delimiters: open.map(|open| Delimiters {
                open: open.span,
                close: close.map(|token| token.span),
            }),
            span: start.merge(end),
        }
```

`parse_header_body` answers four values, the slot delimiters second:
`-> (Vec<SlotDeclaration>, Option<Delimiters>, Vec<Statement>, Span)`, with
`let (slots, slot_delimiters) = if self.at(TokenKind::Bar) || self.at(TokenKind::DoubleBar) { self.parse_slot_declarations() } else { (Vec::new(), None) };`.

`parse_class_declaration`: keep the G1 `(` (`let instance_open = self.expect(TokenKind::LeftParenthesis);`), take the G2 `)` from the branch that parses G2 (`let close = self.expect(TokenKind::RightParenthesis);`, a fifth tuple element, `None` in the branch without G2), and build

```rust
        let instance_side = ClassBody {
            slots,
            initializer_expressions,
            methods,
            nested_classes,
            delimiters: instance_open.map(|open| Delimiters {
                open: open.span,
                close: instance_close.map(|token| token.span),
            }),
            slot_delimiters,
            span: g1_span.merge(g2_end),
        };
```

The class side:

```rust
        let class_side = if self.eat(TokenKind::Colon) {
            let open = self.expect(TokenKind::LeftParenthesis);
            // A class-side comment is allowed but not retained.
            let _ = self.take_class_comment();
            let mut body = self.parse_class_side_body();
            let close = self.expect(TokenKind::RightParenthesis);
            body.delimiters = open.map(|open| Delimiters {
                open: open.span,
                close: close.map(|token| token.span),
            });
            Some(body)
        } else {
            None
        };
```

`parse_class_side_body` and the mixin application's `.` form build their `ClassBody` with `delimiters: None, slot_delimiters: None`.

In `expression.rs`, `parse_block` (it is only reached at a `[`):

```rust
        let right_bracket = self.expect(TokenKind::RightBracket);
        let span = left_bracket
            .span
            .merge(right_bracket.unwrap_or(self.current()).span);
        Expression::Block(Box::new(BlockExpression {
            parameters,
            body,
            delimiters: Some(Delimiters {
                open: left_bracket.span,
                close: right_bracket.map(|token| token.span),
            }),
            span,
        }))
```

In `crates/ns-runtime/src/compiler/simultaneous.rs` the block it builds gets `delimiters: None` and its body `temporaries_delimiters: None`.

- [ ] **Step 5: Build the workspace and fix exhaustive destructurings**

Run: `cargo build --workspace --all-targets`
Expected: compiles. If a pattern elsewhere destructures `ClassBody`, `MethodDeclaration`, `CodeBody` or `BlockExpression` without `..`, add `..` to it; change nothing else.

- [ ] **Step 6: Run the test and the gates**

Run: `cargo nextest run -p ns-syntax every_folding_construct_records_its_brackets` — PASS. Then every gate of "Global Constraints".

- [ ] **Step 7: Commit**

```bash
git add crates/ns-syntax crates/ns-runtime/src/compiler/simultaneous.rs
git commit -m "feat: the AST records the brackets an editor folds between"
```

---

### Task 2: Comment spans and `parse_for_editor`

**Files:**
- Modify: `crates/ns-syntax/src/lexer.rs`
- Modify: `crates/ns-syntax/src/lib.rs`
- Create: `crates/ns-syntax/tests/recovery.rs`

**Interfaces:**
- Consumes: Task 1's `Delimiters` fields (the empty module sets them `None`).
- Produces:
  ```rust
  pub struct Lexed { pub tokens: Vec<Token>, pub comments: Vec<Span>, pub diagnostics: Vec<Diagnostic> }   // ns_syntax::lexer
  impl Lexer<'_> { pub fn run_keeping_comments(self) -> Lexed }   // `run` keeps its signature
  pub struct EditorParse { pub tokens: Vec<Token>, pub module: Module, pub comments: Vec<Span>, pub diagnostics: Vec<Diagnostic> }
  pub fn parse_for_editor(source: &str) -> EditorParse            // ns_syntax
  ```
  `tests/recovery.rs` helpers used by Tasks 3–6: `recovered(source) -> (Module, Vec<String>)`, `method_names(body: &ClassBody) -> Vec<String>`, `class_names(body: &ClassBody) -> Vec<String>`.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-syntax/src/lexer.rs`'s tests:

```rust
    /// A nested comment is one span, outer `(*` to outer `*)`; a `(*` in a
    /// string opens none; an unterminated one runs to the end.
    #[test]
    fn comment_spans_are_kept_on_request() {
        let source = "a (* outer (* inner *) still *) 'not (* a comment' (* open";
        let lexed = Lexer::new(source).run_keeping_comments();
        let texts: Vec<&str> = lexed.comments.iter().map(|span| span.text(source)).collect();
        assert_eq!(texts, ["(* outer (* inner *) still *)", "(* open"]);
        assert_eq!(lexed.diagnostics.len(), 1, "the unterminated comment");
    }
```

Create `crates/ns-syntax/tests/recovery.rs`:

```rust
//! The module `parse_for_editor` answers for a source with errors: what an
//! editor shows while a file is typed. Each test asserts that what follows
//! the error survives, with its name and span.

use indoc::indoc;
use ns_syntax::ast::{ClassBody, MessagePattern, Module};

fn recovered(source: &str) -> (Module, Vec<String>) {
    let parsed = ns_syntax::parse_for_editor(source);
    let messages = parsed
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    (parsed.module, messages)
}

fn selector(pattern: &MessagePattern) -> String {
    match pattern {
        MessagePattern::Unary { selector } | MessagePattern::Binary { selector, .. } => {
            selector.name.clone()
        }
        MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, _)| format!("{}:", keyword.name))
            .collect(),
    }
}

fn method_names(body: &ClassBody) -> Vec<String> {
    body.methods
        .iter()
        .map(|method| selector(&method.pattern))
        .collect()
}

fn class_names(body: &ClassBody) -> Vec<String> {
    body.nested_classes
        .iter()
        .map(|class| class.name.name.clone())
        .collect()
}

#[test]
fn a_clean_source_parses_as_parse_parses_it() {
    let source = indoc! {"
        (* A module. *)
        class Module = () (
            run = ( ^1 )
        ) : ()
    "};
    let parsed = ns_syntax::parse_for_editor(source);
    assert!(parsed.diagnostics.is_empty());
    assert_eq!(Ok(parsed.module), ns_syntax::parse(source).map_err(|_| ()));
    assert_eq!(Ok(parsed.tokens), ns_syntax::lex(source).map_err(|_| ()));
    assert_eq!(parsed.comments.len(), 1);
}

/// A lexer error does not stop the parse: the editor still gets the
/// members around it.
#[test]
fn every_lexer_error_still_answers_a_module() {
    for broken in ["^#", "^x := 1", "^#(1)", "^\u{00a7}"] {
        let source = format!(
            "class Module = () (\n    first = ( {broken} )\n    second = ( ^2 )\n) : ()\n"
        );
        let (module, messages) = recovered(&source);
        assert!(!messages.is_empty(), "{broken}");
        assert!(
            method_names(&module.root_class.instance_side).contains(&"second".to_string()),
            "{broken}: {messages:?}"
        );
    }
    for unterminated in ["^'open", "(* open"] {
        let source = format!("class Module = () (\n    first = ( {unterminated}\n");
        let (_, messages) = recovered(&source);
        assert!(!messages.is_empty(), "{unterminated}");
    }
}

#[test]
fn an_empty_source_answers_a_module() {
    for source in ["", "   \n\t\n"] {
        let (module, messages) = recovered(source);
        assert!(!messages.is_empty());
        assert!(module.root_class.instance_side.methods.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p ns-syntax comment_spans_are_kept_on_request a_clean_source every_lexer_error an_empty_source`
Expected: compile errors — `run_keeping_comments`, `parse_for_editor` not found.

- [ ] **Step 3: Record comment spans in the lexer**

`Lexer` gets `comments: Vec<Span>`, empty in `new`. `skip_block_comment` pushes `Span::new(start as u32, self.position as u32)` when the outermost comment closes (depth reaches 0) and, after its diagnostic, when it is unterminated. Then:

```rust
/// A source's tokens with its comments kept, for an editor: the parser and
/// the runtime never see a comment, so `run` drops them.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub comments: Vec<Span>,
    pub diagnostics: Vec<Diagnostic>,
}

    pub fn run(self) -> (Vec<Token>, Vec<Diagnostic>) {
        let lexed = self.run_keeping_comments();
        (lexed.tokens, lexed.diagnostics)
    }

    pub fn run_keeping_comments(mut self) -> Lexed {
        // the body of today's `run`, ending in:
        Lexed {
            tokens: self.tokens,
            comments: self.comments,
            diagnostics: self.diagnostics,
        }
    }
```

- [ ] **Step 4: Add the editor entry**

In `crates/ns-syntax/src/lib.rs`:

```rust
/// What [`parse_for_editor`] answers: a module for every source, recovered
/// where the source has errors, with what an editor reads beside it.
pub struct EditorParse {
    /// The lexer's tokens, before the parser splits any.
    pub tokens: Vec<Token>,
    pub module: Module,
    /// Every comment, in source order.
    pub comments: Vec<Span>,
    /// The lexer's diagnostics, then the parser's.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse `source` for an editor, which shows a file while it is typed: a
/// lexer error does not stop the parse, and the module is answered even
/// when the source has errors. Nothing compiles such a module; [`parse`]
/// is the entry for that.
pub fn parse_for_editor(source: &str) -> EditorParse {
    if let Some(diagnostic) = source_too_long(source.len()) {
        return EditorParse {
            tokens: Vec::new(),
            module: empty_module(),
            comments: Vec::new(),
            diagnostics: vec![diagnostic],
        };
    }
    let lexed = lexer::Lexer::new(source).run_keeping_comments();
    let mut parser = parser::Parser::new(source, lexed.tokens.clone());
    let module = parser.parse_module();
    let mut diagnostics = lexed.diagnostics;
    diagnostics.extend(parser.into_diagnostics());
    EditorParse {
        tokens: lexed.tokens,
        module,
        comments: lexed.comments,
        diagnostics,
    }
}

/// The module of a source too long to lex: a root class with no name and
/// nothing in it.
fn empty_module() -> Module {
    let span = Span::new(0, 0);
    let body = ast::ClassBody {
        slots: Vec::new(),
        initializer_expressions: Vec::new(),
        methods: Vec::new(),
        nested_classes: Vec::new(),
        delimiters: None,
        slot_delimiters: None,
        span,
    };
    Module {
        language_id: String::new(),
        category: String::new(),
        root_class: ast::ClassDeclaration {
            access: None,
            name: ast::Identifier { name: String::new(), span },
            factory: None,
            superclass: None,
            mixin_applications: Vec::new(),
            class_comment: None,
            instance_side: body,
            class_side: None,
            span,
        },
        span,
    }
}
```

`parse`'s comment about the lexer-error gate stays: it is `parse`'s contract. The parser is safe on such tokens (`lexeme_body` and `symbol_body` never slice past a lexeme), which `every_lexer_error_still_answers_a_module` pins.

- [ ] **Step 5: Run the tests and the gates**

Run: `cargo nextest run -p ns-syntax` — all PASS. Then every gate.

- [ ] **Step 6: Commit**

```bash
git add crates/ns-syntax
git commit -m "feat: ns-syntax answers comment spans and a module for every source, for the editor"
```

---

### Task 3: Depth guards on setter chains and pattern literals

**Files:**
- Modify: `crates/ns-syntax/src/parser/expression.rs` (`parse_expression`, `parse_pattern_literal`)
- Test: `crates/ns-syntax/src/parser/declaration.rs` (next to `deeply_nested_open_parentheses_do_not_overflow`)

**Interfaces:**
- Consumes: `Parser::enter`, `Parser::leave`, `Parser::skip_to_structural` (existing).

- [ ] **Step 1: Write the failing tests**

Next to `deeply_nested_open_parentheses_do_not_overflow` (read it first and use the same helper it uses to parse and collect messages):

```rust
    /// A chain of setter keywords recurses once per keyword; past the
    /// nesting limit it is a diagnostic, never a stack overflow.
    #[test]
    fn a_long_setter_chain_does_not_overflow() {
        let source = format!(
            "class Module = () ( run = ( {}1 ) ) : ()",
            "x:: ".repeat(100_000)
        );
        let Err(diagnostics) = crate::parse(&source) else {
            panic!("a chain past the limit is refused");
        };
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "expression nested too deeply"));
    }

    /// Nested pattern literals recurse once per level, past the limit a
    /// diagnostic as well.
    #[test]
    fn deeply_nested_pattern_literals_do_not_overflow() {
        let source = format!(
            "class Module = () ( run = ( ^{}1 ) ) : ()",
            "<a: ".repeat(100_000)
        );
        let Err(diagnostics) = crate::parse(&source) else {
            panic!("nesting past the limit is refused");
        };
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "pattern nested too deeply"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p ns-syntax a_long_setter_chain deeply_nested_pattern_literals`
Expected: the test process aborts with a stack overflow (nextest reports the test as failed by a signal or abort).

- [ ] **Step 3: Guard both recursions**

In `parse_expression`, around the setter branch:

```rust
        if self.at(TokenKind::SetterKeyword) {
            if !self.enter() {
                self.leave();
                let token = self.current();
                self.push_error(token.span, "expression nested too deeply");
                self.skip_to_structural();
                return Expression::Literal(Literal::Nil { span: token.span });
            }
            let keyword = self.bump();
            // … unchanged up to `let value = self.parse_expression();` …
            let value = self.parse_expression();
            self.leave();
            // … unchanged …
        }
```

In `parse_pattern_literal`, at its start:

```rust
    fn parse_pattern_literal(&mut self) -> Literal {
        if !self.enter() {
            self.leave();
            let token = self.current();
            self.push_error(token.span, "pattern nested too deeply");
            self.skip_to_structural();
            return Literal::Nil { span: token.span };
        }
        let literal = self.parse_pattern_literal_inner();
        self.leave();
        literal
    }
```

with today's body moved, unchanged, into `fn parse_pattern_literal_inner(&mut self) -> Literal`. Extend `MAX_DEPTH`'s doc comment in `parser/mod.rs` to list the two new guarded productions.

- [ ] **Step 4: Run the tests and the gates**

Run: `cargo nextest run -p ns-syntax` — PASS. Then every gate.

- [ ] **Step 5: Commit**

```bash
git add crates/ns-syntax
git commit -m "fix: setter chains and nested pattern literals are depth-guarded, as malformed source must answer diagnostics"
```

---

### Task 4: Recovery of method bodies, statements and class sides

**Files:**
- Modify: `crates/ns-syntax/src/parser/mod.rs` (`previous_span`)
- Modify: `crates/ns-syntax/src/parser/declaration.rs` (`at_member_start`, `skip_balanced`, `parse_code_body`, `parse_method_declaration`, `parse_class_declaration`, `parse_side_body`, `parse_class_side_body`)
- Test: `crates/ns-syntax/tests/recovery.rs`

**Interfaces:**
- Consumes: Task 1's fields, Task 2's `parse_for_editor` and test helpers.
- Produces (for Task 5): `pub(super) fn at_member_start(&mut self) -> bool`, `pub(super) fn skip_balanced(&mut self, stops: &[TokenKind]) -> bool`, `fn previous_span(&self) -> Span` (`pub(super)`).

- [ ] **Step 1: Write the failing tests**

Append to `crates/ns-syntax/tests/recovery.rs`:

```rust
/// The case that matters most while typing: a method whose body is not
/// closed yet keeps every member after it.
#[test]
fn a_half_typed_method_keeps_the_members_after_it() {
    let source = indoc! {"
        class Module = () (
            broken = (
                ^1
            second = (
                ^2
            )
            class Inner = () ()
        ) : (
            make = ( ^self new )
        )
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(method_names(&root.instance_side), ["broken", "second"], "{messages:?}");
    assert_eq!(class_names(&root.instance_side), ["Inner"]);
    let class_side = root.class_side.as_ref().expect("the class side survives");
    assert_eq!(method_names(class_side), ["make"]);
    let broken = &root.instance_side.methods[0];
    assert_eq!(broken.body_delimiters.and_then(|delimiters| delimiters.close), None);
    assert!(broken.span.text(source).ends_with("^1"), "{}", broken.span.text(source));
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[test]
fn a_statement_after_a_return_is_skipped_to_the_end_of_the_body() {
    let source = indoc! {"
        class Module = () (
            first = ( ^1. 2 + 3 )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(body.methods[0].body.statements.len(), 1);
    assert!(body.methods[0].body_delimiters.and_then(|delimiters| delimiters.close).is_some());
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[test]
fn junk_between_statements_is_skipped_to_the_next_statement() {
    let source = indoc! {"
        class Module = () (
            first = ( | x | x:: 1 'stray'. ^x )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(body.methods[0].body.statements.len(), 2);
    assert_eq!(messages.len(), 1, "{messages:?}");
}

/// A run of tokens no member starts with is one error, not one per token.
#[test]
fn a_run_of_junk_in_a_class_side_is_one_error() {
    let source = indoc! {"
        class Module = () (
            3 4 (5 6) 7
            first = ( ^1 )
        ) : (
            8 9
            make = ( ^self new )
        )
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(method_names(&root.instance_side), ["first"]);
    assert_eq!(method_names(root.class_side.as_ref().expect("a class side")), ["make"]);
    assert_eq!(messages.len(), 2, "{messages:?}");
}

/// Nothing but indentation says where a nested class missing its `)`
/// ends, so the members after it become its own; they survive all the
/// same, with their spans.
#[test]
fn a_nested_class_missing_its_close_keeps_the_members_after_it() {
    let source = indoc! {"
        class Module = () (
            class Inner = () (
                inside = ( ^1 )
            outside = ( ^2 )
        ) : ()
    "};
    let (module, _) = recovered(source);
    let inner = &module.root_class.instance_side.nested_classes[0];
    assert_eq!(method_names(&inner.instance_side), ["inside", "outside"]);
    assert_eq!(inner.instance_side.methods[1].span.text(source), "outside = ( ^2 )");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p ns-syntax --test recovery`
Expected: `a_half_typed_method…`, `a_statement_after_a_return…`, `junk_between_statements…` and `a_run_of_junk…` FAIL (lost members or more diagnostics); the nested-class test may already pass.

- [ ] **Step 3: Add the two recovery primitives**

In `parser/mod.rs`, after `bump`:

```rust
    /// The span of the last token consumed: where a node that recovery ended
    /// early ends, so its span never reaches an unrelated token after it.
    pub(super) fn previous_span(&self) -> crate::span::Span {
        match self.position.checked_sub(1) {
            Some(index) => self.tokens[index].span,
            None => self.current().span,
        }
    }
```

In `declaration.rs`, next to `looks_like_method`:

```rust
    /// Whether a member of a class side starts at the cursor -- a nested
    /// class, a lazy slot or a method, an access modifier before it
    /// included. A recovery skip stops here, so the member after an error is
    /// read as one rather than swallowed.
    pub(super) fn at_member_start(&mut self) -> bool {
        self.at_nested_class() || self.at_lazy_slot_prefix() || self.looks_like_method()
    }

    /// Skips what is left of a construct after an error, each bracketed
    /// group whole, so a recovery never stops at a bracket inside a group nor
    /// takes a bracket that closes an enclosing one. Stops -- consuming
    /// nothing more -- at a token of `stops` outside every group, at a
    /// closing bracket no skipped group opened, at a member start, or at the
    /// end of input. Answers whether it skipped anything.
    pub(super) fn skip_balanced(&mut self, stops: &[TokenKind]) -> bool {
        let start = self.position;
        let mut expected_closers: Vec<TokenKind> = Vec::new();
        loop {
            let kind = self.peek();
            if kind == TokenKind::EndOfInput || self.at_member_start() {
                break;
            }
            match kind {
                TokenKind::LeftParenthesis => expected_closers.push(TokenKind::RightParenthesis),
                TokenKind::LeftBracket => expected_closers.push(TokenKind::RightBracket),
                TokenKind::LeftBrace => expected_closers.push(TokenKind::RightBrace),
                TokenKind::RightParenthesis | TokenKind::RightBracket | TokenKind::RightBrace => {
                    match expected_closers.iter().rposition(|expected| *expected == kind) {
                        Some(index) => expected_closers.truncate(index),
                        None => break,
                    }
                }
                _ if expected_closers.is_empty() && stops.contains(&kind) => break,
                _ => {}
            }
            self.bump();
        }
        self.position != start
    }

    /// Whether the cursor is at what ends a code body: its `)` or `]`, or
    /// the end of input.
    fn at_body_close(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::RightParenthesis | TokenKind::RightBracket | TokenKind::EndOfInput
        )
    }
```

- [ ] **Step 4: Recover in code bodies**

Replace `parse_code_body`'s loop and end:

```rust
        loop {
            match self.peek() {
                TokenKind::RightBracket | TokenKind::RightParenthesis | TokenKind::EndOfInput => {
                    break
                }
                TokenKind::Caret => {
                    self.bump(); // `^`
                    let expression = self.parse_expression();
                    statements.push(Statement::Return(expression));
                    self.eat(TokenKind::Dot); // optional trailing dot
                    // Nothing follows a return (§7.2). What does is skipped to
                    // the body's end, unless a member starts there: then the
                    // body was never closed, which its owner reports.
                    if !self.at_body_close() && !self.at_member_start() {
                        let token = self.current();
                        self.push_error(
                            token.span,
                            &format!(
                                "expected the end of the body after a return (§7.2), found {:?}",
                                token.kind
                            ),
                        );
                        self.skip_rest_of_body();
                    }
                    break;
                }
                TokenKind::Dot => {
                    let dot = self.bump();
                    self.push_error(dot.span, "a `.` ends a statement, and none is here");
                }
                _ => {
                    let before = self.position;
                    let expression = self.parse_expression();
                    statements.push(Statement::Expression(expression));
                    if self.eat(TokenKind::Dot) || self.at_body_close() {
                        continue;
                    }
                    if self.at_member_start() {
                        break;
                    }
                    // A statement the expression did not end: skip to the next
                    // one, so one error does not end the body. An expression
                    // that consumed nothing has reported its token already.
                    if self.position != before {
                        let token = self.current();
                        self.push_error(
                            token.span,
                            &format!("expected `.` between statements, found {:?}", token.kind),
                        );
                    }
                    self.skip_rest_of_statement();
                }
            }
        }
        let end = if self.at_body_close() && !self.at(TokenKind::EndOfInput) {
            self.current().span
        } else {
            self.previous_span()
        };
```

and add:

```rust
    /// After an error in a statement: skip to the `.` that ends it and past
    /// it. A stray `}` that closes nothing is skipped as well, so the body
    /// always moves on.
    fn skip_rest_of_statement(&mut self) {
        if !self.skip_balanced(&[TokenKind::Dot]) && self.at(TokenKind::RightBrace) {
            self.bump();
        }
        self.eat(TokenKind::Dot);
    }

    /// After a statement nothing may follow: skip to the body's close, or to
    /// a member start, stray `}` included.
    fn skip_rest_of_body(&mut self) {
        loop {
            self.skip_balanced(&[]);
            if !self.at(TokenKind::RightBrace) {
                break;
            }
            self.bump();
        }
    }
```

- [ ] **Step 5: Recover at a method's close**

```rust
    pub(crate) fn parse_method_declaration(&mut self) -> MethodDeclaration {
        let start = self.current().span;
        let access = self.parse_access_modifier();
        let pattern = self.parse_message_pattern();
        self.expect(TokenKind::Equals);
        let open = self.expect(TokenKind::LeftParenthesis);
        let body = self.parse_code_body();
        let close = self.close_method_body();
        let end = close.map_or_else(|| self.previous_span(), |token| token.span);
        MethodDeclaration {
            access,
            pattern,
            body,
            body_delimiters: open.map(|open| Delimiters {
                open: open.span,
                close: close.map(|token| token.span),
            }),
            span: start.merge(end),
        }
    }

    /// A method body's `)`. When it is missing and a member starts here, the
    /// method ends where its body did, so the member after it is read as
    /// one -- the method being typed keeps the class around it. Otherwise
    /// what is left is skipped, balanced, up to the method's `)`.
    fn close_method_body(&mut self) -> Option<Token> {
        if self.at(TokenKind::RightParenthesis) {
            return Some(self.bump());
        }
        let token = self.current();
        self.push_error(
            token.span,
            &format!("expected `)` to end the method body, found {:?}", token.kind),
        );
        self.skip_rest_of_body();
        self.eat_token(TokenKind::RightParenthesis)
    }
```

(`Token` needs `use crate::token::{Token, TokenKind};` at the top of `declaration.rs`.)

- [ ] **Step 6: Recover in class sides, and end groups at their last token**

In `parse_side_body`, the last arm:

```rust
                _ => {
                    let token = self.current();
                    self.push_error(
                        token.span,
                        "expected a method, nested class, or lazy slot declaration",
                    );
                    // One error for the whole run, up to the next member.
                    if !self.skip_balanced(&[]) && !self.at(TokenKind::RightParenthesis) {
                        self.bump(); // a stray `]` or `}`
                    }
                }
```

The same in `parse_class_side_body`'s last arm, with its message `"expected a method declaration"`.

Add

```rust
    /// Where a class's `( … )` group ends: its `)`, or, when recovery found
    /// none, the last token it consumed.
    fn end_of_group(&self) -> crate::span::Span {
        if self.at(TokenKind::RightParenthesis) {
            self.current().span
        } else {
            self.previous_span()
        }
    }
```

and use it for `g1_end`, for the G2 branch's `end`, and for `parse_class_side_body`'s `end`, in place of `self.current().span`.

- [ ] **Step 7: Run the tests and the gates**

Run: `cargo nextest run -p ns-syntax` — PASS, the existing parser tests included. Then every gate; `parse-corpus` must print "parsed 211, failed 0".

- [ ] **Step 8: Commit**

```bash
git add crates/ns-syntax
git commit -m "feat: a method body, a statement or a class side with an error no longer takes the class with it"
```

---

### Task 5: Recovery of brackets, slot sections and type annotations

**Files:**
- Modify: `crates/ns-syntax/src/parser/expression.rs` (`parse_primary_inner`'s `(` arm, `parse_block`, `parse_dynamic_array`)
- Modify: `crates/ns-syntax/src/parser/declaration.rs` (`parse_slot_section`, remove `recover_to_dot`)
- Modify: `crates/ns-syntax/src/parser/mod.rs` (`skip_type_annotation`)
- Test: `crates/ns-syntax/tests/recovery.rs`

**Interfaces:**
- Consumes: Task 4's `skip_balanced`, `previous_span`, `at_body_close` (make it `pub(super)`).

- [ ] **Step 1: Write the failing tests**

Append to `tests/recovery.rs`:

```rust
#[test]
fn an_unclosed_block_ends_at_the_method_close() {
    let source = indoc! {"
        class Module = () (
            first = ( ^[:each | each
            )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert!(body.methods[0].body_delimiters.and_then(|delimiters| delimiters.close).is_some());
    let ns_syntax::ast::Statement::Return(ns_syntax::ast::Expression::Block(block)) =
        &body.methods[0].body.statements[0]
    else {
        panic!("a returned block");
    };
    assert_eq!(block.delimiters.and_then(|delimiters| delimiters.close), None);
    assert!(block.span.text(source).ends_with("each"), "{}", block.span.text(source));
}

#[test]
fn an_unclosed_parenthesis_stops_at_the_end_of_its_statement() {
    let source = indoc! {"
        class Module = () (
            first = ( | x | x:: (1 + 2. ^x )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(body.methods[0].body.statements.len(), 2);
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[test]
fn a_tuple_with_junk_keeps_its_elements_and_its_close() {
    let source = indoc! {"
        class Module = () (
            first = ( ^{ 1. 2 3. 4 } )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    let ns_syntax::ast::Statement::Return(ns_syntax::ast::Expression::DynamicArray {
        elements,
        span,
    }) = &body.methods[0].body.statements[0]
    else {
        panic!("a returned tuple");
    };
    assert_eq!(elements.len(), 3);
    assert_eq!(span.text(source), "{ 1. 2 3. 4 }");
}

#[test]
fn an_unclosed_slot_section_stops_at_the_header_close() {
    let source = indoc! {"
        class Module = (
            | first second
        ) (
            run = ( ^first )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    let slot_names: Vec<&str> = body.slots.iter().map(|slot| slot.name.name.as_str()).collect();
    assert_eq!(slot_names, ["first", "second"], "{messages:?}");
    assert_eq!(body.slot_delimiters.and_then(|delimiters| delimiters.close), None);
    assert_eq!(method_names(body), ["run"]);
}

/// A type annotation's scan ends at a bracket the annotation did not open,
/// rather than taking it and the rest of the class with it.
#[test]
fn a_half_typed_annotation_does_not_swallow_the_class() {
    let source = indoc! {"
        class Module = (
            | first <Foo |
        ) (
            run = ( ^first )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(body.slots.len(), 1, "{messages:?}");
    assert_eq!(method_names(body), ["run"], "{messages:?}");
}

#[test]
fn half_a_keyword_message_stays_inside_its_statement() {
    let source = indoc! {"
        class Module = () (
            first = ( ^self at: )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(messages.len(), 1, "{messages:?}");
}

/// `| public |` declares no slot; the access modifier takes `public`, and
/// the section's closing bar must stay the close.
#[test]
fn a_slot_section_of_a_modifier_alone_keeps_its_close() {
    let source = indoc! {"
        class Module = (
            | public |
        ) (
            run = ( ^1 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert!(body.slots.is_empty());
    assert!(body.slot_delimiters.and_then(|delimiters| delimiters.close).is_some(), "{messages:?}");
    assert_eq!(method_names(body), ["run"]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p ns-syntax --test recovery`
Expected: the parenthesis, tuple, slot-section, annotation and `| public |` tests FAIL; the block and keyword-message tests may already pass.

- [ ] **Step 3: Parentheses, blocks, tuples**

`parse_primary_inner`'s `(` arm:

```rust
            TokenKind::LeftParenthesis => {
                self.bump();
                let expression = self.parse_expression();
                if !self.eat(TokenKind::RightParenthesis) {
                    let token = self.current();
                    self.push_error(
                        token.span,
                        &format!("expected `)` to close the parenthesis, found {:?}", token.kind),
                    );
                    // A parenthesis holds one expression and no `.`: a `.`
                    // means it was never closed, so the skip stops there and
                    // leaves an enclosing body's `)` alone.
                    self.skip_balanced(&[TokenKind::Dot]);
                    self.eat(TokenKind::RightParenthesis);
                }
                expression
            }
```

`parse_block`'s end, so a missing `]` ends the block at its last token:

```rust
        let body = self.parse_code_body();
        let right_bracket = self.expect(TokenKind::RightBracket);
        let end = right_bracket.map_or_else(|| self.previous_span(), |token| token.span);
        let span = left_bracket.span.merge(end);
```

`parse_dynamic_array`:

```rust
    fn parse_dynamic_array(&mut self) -> Expression {
        let left_brace = self.bump(); // {
        let mut elements = Vec::new();
        while !self.at(TokenKind::RightBrace) && !self.at_body_close() {
            elements.push(self.parse_expression());
            if self.eat(TokenKind::Dot) || self.at(TokenKind::RightBrace) {
                continue;
            }
            let token = self.current();
            self.push_error(
                token.span,
                &format!("expected `.` between a tuple's elements, found {:?}", token.kind),
            );
            if !self.skip_balanced(&[TokenKind::Dot]) || !self.eat(TokenKind::Dot) {
                break;
            }
        }
        let right_brace = self.expect(TokenKind::RightBrace);
        let end = right_brace.map_or_else(|| self.previous_span(), |token| token.span);
        Expression::DynamicArray {
            elements,
            span: left_brace.span.merge(end),
        }
    }
```

(`skip_balanced(&[Dot])` stops at the tuple's own `}`, which closes no group it skipped; the loop then sees it.)

- [ ] **Step 4: Slot sections**

In `parse_slot_section`, the loop condition and the not-a-name branch:

```rust
        while !self.at_slot_section_end(close) {
            let start = self.current().span;
            let access = self.parse_access_modifier();
            if !self.at_name() {
                let token = self.current();
                self.push_error(token.span, "expected a slot name");
                let before = self.position;
                self.skip_balanced(&[TokenKind::Dot, close]);
                if self.position == before && !self.at_slot_section_end(close) {
                    self.bump();
                }
                self.eat(TokenKind::Dot);
                continue;
            }
            // … unchanged …
        }
```

with

```rust
    /// What ends a slot section's loop: its closing bar, or a closing
    /// bracket, which no slot declaration holds at its top level -- the
    /// section was never closed, and the bracket is its owner's.
    fn at_slot_section_end(&self, close: TokenKind) -> bool {
        self.at(close)
            || matches!(
                self.peek(),
                TokenKind::RightParenthesis
                    | TokenKind::RightBracket
                    | TokenKind::RightBrace
                    | TokenKind::EndOfInput
            )
    }
```

Delete `recover_to_dot`, whose one caller this replaces.

- [ ] **Step 5: Type annotations close only what they opened**

In `parser/mod.rs`, `skip_type_annotation` tracks the closers it expects:

```rust
        let open = self.bump(); // `<`
        let mut expected_closers = vec![TokenKind::GreaterThan];
        loop {
            match self.peek() {
                TokenKind::EndOfInput => {
                    self.push_error(open.span, "unterminated `<...>` type annotation");
                    return;
                }
                TokenKind::LessThan => {
                    expected_closers.push(TokenKind::GreaterThan);
                    self.bump();
                }
                // The bound operator `<:` … (comment unchanged)
                TokenKind::LeftBracket => {
                    expected_closers.push(TokenKind::RightBracket);
                    self.bump();
                }
                TokenKind::LeftBrace => {
                    expected_closers.push(TokenKind::RightBrace);
                    self.bump();
                }
                TokenKind::LeftParenthesis => {
                    expected_closers.push(TokenKind::RightParenthesis);
                    self.bump();
                }
                kind @ (TokenKind::GreaterThan
                | TokenKind::RightBracket
                | TokenKind::RightBrace
                | TokenKind::RightParenthesis) => {
                    // A closer the annotation did not open belongs to the
                    // declaration around it: `| a <Foo |` then `)` must leave
                    // the header's `)` to the header.
                    if expected_closers.last() != Some(&kind) {
                        self.push_error(open.span, "unterminated `<...>` type annotation");
                        return;
                    }
                    self.bump();
                    expected_closers.pop();
                    if expected_closers.is_empty() {
                        return;
                    }
                }
                // Safety net … (comment and arm unchanged)
                TokenKind::Equals | TokenKind::ColonColonEquals => { … }
                _ => {
                    self.bump();
                }
            }
        }
```

Update the doc comment's sentence "Balanced `<`/`[` pairs are consumed until the matching `>`" to say each closer must match what it closes.

- [ ] **Step 6: Run the tests and the gates**

Run: `cargo nextest run -p ns-syntax` — PASS. Then every gate; `parse-corpus` must print "parsed 211, failed 0" (it pins that no clean module relied on a mismatched closer inside an annotation).

- [ ] **Step 7: Commit**

```bash
git add crates/ns-syntax
git commit -m "feat: an unclosed bracket, slot section or type annotation stops before its owner's close"
```

---

### Task 6: No invented declarations

**Files:**
- Modify: `crates/ns-syntax/src/parser/mod.rs` (`declared_identifier`)
- Modify: `crates/ns-syntax/src/parser/declaration.rs` (`parse_message_pattern`, `parse_method_declaration`, `parse_type_parameter_declaration`, `at_binary_equals_factory`, `parse_class_declaration`, `parse_superclass_clause`, `parse_lazy_slot_declaration`, `parse_side_body`, `parse_class_side_body`)
- Modify: `crates/ns-syntax/src/parser/expression.rs` (`parse_block`, `parse_cascade_message`, `parse_cascade_on`)
- Test: `crates/ns-syntax/tests/recovery.rs`

**Interfaces:**
- Produces: `declared_identifier(&mut self, role: &str) -> Option<Identifier>`; `parse_message_pattern(&mut self) -> Option<MessagePattern>`; `parse_method_declaration(&mut self) -> Option<MethodDeclaration>`; `parse_superclass_clause(&mut self) -> Option<SuperclassClause>`; `parse_lazy_slot_declaration(&mut self) -> Option<SlotDeclaration>`; `parse_cascade_message(&mut self) -> Option<MessageSend>`. `parse_method_declaration_for_test` answers `Option<MethodDeclaration>` and its callers `.expect("a method")` it.

- [ ] **Step 1: Write the failing tests**

Append to `tests/recovery.rs`:

```rust
/// A declaration whose name is missing is left out, so the resolver never
/// sees two empty names to call duplicates.
#[test]
fn a_missing_name_declares_nothing() {
    let source = indoc! {"
        class Module () (
            class Inner = outer () ()
            first = ( ^[: | 1] )
            second = ( ^self foo; )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(root.factory, None, "{messages:?}");
    assert_eq!(root.instance_side.nested_classes[0].superclass, None);
    assert_eq!(method_names(&root.instance_side), ["first", "second"]);
    let ns_syntax::ast::Statement::Return(ns_syntax::ast::Expression::Block(block)) =
        &root.instance_side.methods[0].body.statements[0]
    else {
        panic!("a returned block");
    };
    assert!(block.parameters.is_empty());
    let ns_syntax::ast::Statement::Return(cascade) = &root.instance_side.methods[1].body.statements[0]
    else {
        panic!("a returned cascade");
    };
    if let ns_syntax::ast::Expression::Cascade(cascade) = cascade {
        assert!(cascade.rest.is_empty());
    }
}

/// The root class is the one declaration kept without a name: without it
/// there is no module, and no class holds it as a member.
#[test]
fn a_root_class_without_a_name_is_kept() {
    let source = indoc! {"
        class = () (
            run = ( ^1 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    assert_eq!(module.root_class.name.name, "", "{messages:?}");
    assert_eq!(method_names(&module.root_class.instance_side), ["run"]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p ns-syntax --test recovery a_missing_name a_root_class`
Expected: `a_missing_name_declares_nothing` FAILS (the factory is `Some` with an empty selector, the clause has an empty receiver, the block an empty parameter).

- [ ] **Step 3: Answer `None` for a missing name, and leave the declaration out**

`declared_identifier` answers `Option<Identifier>`, `None` (after its diagnostic, consuming nothing) when no name is there.

`parse_message_pattern` answers `Option<MessagePattern>`: each `declared_identifier` result is collected; any `None` makes the whole pattern `None`, after the pattern's tokens (and its type annotations, return type and `where` clause) are consumed as today. Binary: `let parameter = self.declared_identifier("name a parameter");` … `selector`-and-`parameter.map(|parameter| MessagePattern::Binary { selector, parameter })`. Keyword: a `missing` flag set when any part's parameter is `None`; answer `(!missing).then_some(MessagePattern::Keyword { parts })`. Unary: `selector.map(|selector| MessagePattern::Unary { selector })`.

`at_binary_equals_factory` and `looks_like_method` discard the result as today (`let _ = …`).

`parse_method_declaration` parses everything as in Task 4 and answers `pattern.map(|pattern| MethodDeclaration { … })`; `parse_side_body` and `parse_class_side_body` push only `Some`:

```rust
                _ if self.looks_like_method() => {
                    methods.extend(self.parse_method_declaration());
                }
```

`parse_class_declaration`:

```rust
        // The root class is kept without a name: without it there is no
        // module. A nested one always has its name (`at_nested_class`).
        let name = self.declared_identifier("name a class").unwrap_or_else(|| {
            let offset = self.current().span.start;
            Identifier {
                name: String::new(),
                span: crate::span::Span::new(offset, offset),
            }
        });
        // …
        let factory = if self.at(TokenKind::Equals) && !self.at_binary_equals_factory() {
            None
        } else {
            self.parse_message_pattern()
        };
        self.expect(TokenKind::Equals);
        let superclass = if self.at(TokenKind::LeftParenthesis) {
            None
        } else {
            self.parse_superclass_clause()
        };
        let mut mixin_applications = Vec::new();
        while self.eat(TokenKind::MixinOperator) {
            mixin_applications.extend(self.parse_superclass_clause());
        }
```

`parse_superclass_clause` answers `Option<SuperclassClause>`: an `outer` prefix whose name is missing, or a missing receiver, answers `None` right after the `declared_identifier` that failed, before `parse_trailing_sends`:

```rust
                "outer" => {
                    self.bump();
                    Some(SuperclassPrefix::Outer(
                        self.declared_identifier("name an enclosing class")?,
                    ))
                }
        // …
        let receiver = self.declared_identifier("name a superclass")?;
```

`parse_type_parameter_declaration` keeps `let _ = self.declared_identifier(…);`.

`parse_lazy_slot_declaration` answers `Option<SlotDeclaration>`: it parses as today with `let name = self.declared_identifier(…);` and answers `name.map(|name| SlotDeclaration { … })`; both callers in `parse_side_body` use `lazy_slots.extend(…)`.

In `expression.rs`, `parse_block` -- the `|` after the parameters is
expected whenever a `:` was there, so `[: | 1]` does not read its `|` as a
temporaries section once the nameless parameter is left out:

```rust
        let mut has_parameters = false;
        while self.at(TokenKind::Colon) {
            self.bump();
            has_parameters = true;
            parameters.extend(self.declared_identifier("name a block parameter"));
            self.skip_type_annotation();
        }
        if has_parameters {
            self.expect(TokenKind::Bar);
        }
```

`parse_cascade_message` answers `Option<MessageSend>`, `None` in the final case after its diagnostic; `parse_cascade_on` uses `rest.extend(self.parse_cascade_message())` and, in the non-send branch, `let _ = …` as today.

- [ ] **Step 4: Build and fix the remaining callers**

Run: `cargo build --workspace --all-targets`
Expected: compile errors only at the callers listed in this task and in the parser's own tests; adapt each (`.expect("a method")` in tests that parse a valid method, `Some(…)` in assertions on factories).

- [ ] **Step 5: Run the tests and the gates**

Run: `cargo nextest run -p ns-syntax` — PASS. Then every gate.

- [ ] **Step 6: Commit**

```bash
git add crates/ns-syntax
git commit -m "feat: recovery leaves a declaration without a name out instead of naming it empty"
```

---

### Task 7: The server analyses every version through the editor parse

**Files:**
- Modify: `crates/ns-lsp/src/analysis.rs`
- Modify: `crates/ns-lsp/tests/corpus.rs`

**Interfaces:**
- Consumes: `ns_syntax::parse_for_editor`, `EditorParse`.
- Produces: `analyze` answers every finding of the lexer, the parser and the resolver together, and `classified` for every source. Task 8 adds `folds` to `Analysis`.

- [ ] **Step 1: Write the failing tests**

In `analysis.rs`'s tests, replace `a_lexer_error_is_reported_where_it_is` and add a test that semantic tokens survive a syntax error:

```rust
    #[test]
    fn a_lexer_error_is_reported_where_it_is() {
        let text = indoc::indoc! {"
            class Module = Object new () ( public broken = ( ^'unterminated ) ) : ()
        "};
        let analysis = analyze(text);
        // The lexer's findings come first; the parser's follow, since the
        // string runs to the end of the file.
        assert_eq!(analysis.findings[0].message, "unterminated string literal");
        assert_eq!(
            analysis.findings[0].span.start as usize,
            text.find("'unterminated").expect("the literal")
        );
    }

    /// A syntax error no longer takes the semantic colours with it.
    #[test]
    fn a_syntax_error_keeps_the_names_classified() {
        let text = indoc::indoc! {"
            class Module = () (
                broken = (
                    ^1
                echo: argument = ( ^argument )
            ) : ()
        "};
        let analysis = analyze(text);
        assert!(!analysis.findings.is_empty());
        let parameter_use = text.rfind("argument").expect("the use") as u32;
        assert!(analysis
            .classified
            .iter()
            .any(|classified| classified.span.start == parameter_use));
    }
```

In `tests/corpus.rs`, add (reusing `modules`, `read`):

```rust
#[test]
fn the_editor_parse_agrees_with_parse() {
    for path in modules() {
        let text = read(&path);
        let Ok(module) = ns_syntax::parse(&text) else {
            continue;
        };
        let parsed = ns_syntax::parse_for_editor(&text);
        assert!(parsed.diagnostics.is_empty(), "{}", path.display());
        assert!(parsed.module == module, "{}: the modules differ", path.display());
    }
}
```

Refactor the truncation loop into a helper and add the mid-token cuts and the empty-name check:

```rust
/// The cut points of a module, as `no_truncated_module_produces_an_internal_error`
/// documents: every token end for the goldens and the kernel, 64 spread
/// evenly for the corpus modules. With `mid_token`, the middle of each
/// token too -- a string, comment or keyword half typed.
fn cut_points(path: &Path, text: &str, mid_token: bool) -> Vec<usize> {
    let (tokens, _) = ns_syntax::lexer::Lexer::new(text).run();
    let in_corpus = path
        .components()
        .any(|component| component.as_os_str() == "corpus");
    let stride = if in_corpus { (tokens.len() / 64).max(1) } else { 1 };
    let mut cuts = Vec::new();
    for token in tokens.iter().step_by(stride) {
        cuts.push(token.span.end as usize);
        if mid_token {
            let middle = ((token.span.start + token.span.end) / 2) as usize;
            if text.is_char_boundary(middle) {
                cuts.push(middle);
            }
        }
    }
    cuts
}

#[test]
fn no_truncated_module_produces_an_internal_error() {
    for path in modules() {
        let text = read(&path);
        for end in cut_points(&path, &text, true) {
            let context = format!("{} cut at byte {end}", path.display());
            assert_no_internal_error(&text[..end], &context);
        }
    }
}

/// Recovery leaves a declaration without a name out; only a root class
/// keeps an empty one.
#[test]
fn no_truncated_module_declares_an_empty_name() {
    for path in modules() {
        let text = read(&path);
        for end in cut_points(&path, &text, false) {
            let module = ns_syntax::parse_for_editor(&text[..end]).module;
            let mut names = Vec::new();
            declared_names_of_class(&module.root_class, &mut names);
            names.remove(0); // the root class's own name
            assert!(
                names.iter().all(|name| !name.is_empty()),
                "{} cut at byte {end}",
                path.display()
            );
        }
    }
}
```

and the walker it needs, in the same file:

```rust
use ns_syntax::ast;

fn declared_names_of_class(class: &ast::ClassDeclaration, names: &mut Vec<String>) {
    names.push(class.name.name.clone());
    if let Some(factory) = &class.factory {
        declared_names_of_pattern(factory, names);
    }
    for clause in class.superclass.iter().chain(&class.mixin_applications) {
        names.push(clause.receiver.name.clone());
        if let Some(ast::SuperclassPrefix::Outer(name)) = &clause.prefix {
            names.push(name.name.clone());
        }
        for message in &clause.messages {
            message.arguments.iter().for_each(|argument| declared_names_of_expression(argument, names));
        }
    }
    for body in std::iter::once(&class.instance_side).chain(&class.class_side) {
        for slot in &body.slots {
            declared_names_of_slot(slot, names);
        }
        for statement in &body.initializer_expressions {
            declared_names_of_statement(statement, names);
        }
        for method in &body.methods {
            declared_names_of_pattern(&method.pattern, names);
            declared_names_of_body(&method.body, names);
        }
        for nested in &body.nested_classes {
            declared_names_of_class(nested, names);
        }
    }
}

fn declared_names_of_pattern(pattern: &ast::MessagePattern, names: &mut Vec<String>) {
    match pattern {
        ast::MessagePattern::Unary { selector } => names.push(selector.name.clone()),
        ast::MessagePattern::Binary { selector, parameter } => {
            names.push(selector.name.clone());
            names.push(parameter.name.clone());
        }
        ast::MessagePattern::Keyword { parts } => {
            for (keyword, parameter) in parts {
                names.push(keyword.name.clone());
                names.push(parameter.name.clone());
            }
        }
    }
}

fn declared_names_of_slot(slot: &ast::SlotDeclaration, names: &mut Vec<String>) {
    names.push(slot.name.name.clone());
    if let Some(initializer) = &slot.initializer {
        declared_names_of_expression(initializer, names);
    }
}

fn declared_names_of_body(body: &ast::CodeBody, names: &mut Vec<String>) {
    for temporary in &body.temporaries {
        declared_names_of_slot(temporary, names);
    }
    for statement in &body.statements {
        declared_names_of_statement(statement, names);
    }
}

fn declared_names_of_statement(statement: &ast::Statement, names: &mut Vec<String>) {
    match statement {
        ast::Statement::Expression(expression) | ast::Statement::Return(expression) => {
            declared_names_of_expression(expression, names);
        }
    }
}

fn declared_names_of_expression(expression: &ast::Expression, names: &mut Vec<String>) {
    match expression {
        ast::Expression::Identifier(_) | ast::Expression::Literal(_) => {}
        ast::Expression::Send(send) => {
            declared_names_of_expression(&send.receiver, names);
            send.message.arguments.iter().for_each(|argument| declared_names_of_expression(argument, names));
        }
        ast::Expression::Cascade(cascade) => {
            declared_names_of_expression(&cascade.receiver, names);
            for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                message.arguments.iter().for_each(|argument| declared_names_of_expression(argument, names));
            }
        }
        ast::Expression::Assign { value, .. } => declared_names_of_expression(value, names),
        ast::Expression::Block(block) => {
            names.extend(block.parameters.iter().map(|parameter| parameter.name.clone()));
            declared_names_of_body(&block.body, names);
        }
        ast::Expression::DynamicArray { elements, .. } => {
            elements.iter().for_each(|element| declared_names_of_expression(element, names));
        }
    }
}
```

Update the file's doc comment to say the editor parse is held to `parse` as well.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p ns-lsp`
Expected: `a_syntax_error_keeps_the_names_classified` FAILS (no classification while the file does not parse).

- [ ] **Step 3: Analyse through the editor parse**

```rust
/// Every error `ns-next run` would refuse `text` for before running it --
/// the lexer's, the parser's and the resolver's, together -- and every name
/// the resolver can place. A source with errors is analysed as recovered,
/// so the colours survive while a file is typed. Never panics: see
/// `guarded`.
pub fn analyze(text: &str) -> Analysis {
    guarded(text, analyze_unguarded)
}

fn analyze_unguarded(text: &str) -> Analysis {
    let parsed = ns_syntax::parse_for_editor(text);
    // The resolver runs on a recovered module too. None of its errors can
    // come from the recovery, which never invents a declaration
    // (`docs/design/frontend.md`, "Recovery").
    let resolution = ns_runtime::resolve_for_editor(parsed.module);
    let classified =
        semantic_tokens::classify(&resolution.module, &resolution.bindings, &parsed.tokens);
    let mut findings: Vec<Finding> = parsed.diagnostics.into_iter().map(Finding::from).collect();
    findings.extend(resolution.errors.into_iter().map(|error| Finding {
        span: error.span,
        message: error.message,
        severity: Severity::Error,
    }));
    Analysis {
        findings,
        classified,
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-lsp`
Expected: PASS. If a truncation answers an internal error, the panic is a real defect in the resolver or the classifier on a recovered module: use the systematic-debugging skill, fix it where it panics, and add the cut that found it as a unit test beside that code.

- [ ] **Step 5: Run the gates and commit**

Every gate, then:

```bash
git add crates/ns-lsp
git commit -m "feat: the language server analyses a file with syntax errors as recovered, colours and resolver errors included"
```

---

### Task 8: Fold ranges in the server

**Files:**
- Create: `crates/ns-lsp/src/folding.rs`
- Modify: `crates/ns-lsp/src/lib.rs`, `crates/ns-lsp/src/analysis.rs`, `crates/ns-lsp/src/server.rs`
- Modify: `crates/ns-lsp/tests/protocol.rs`

**Interfaces:**
- Consumes: `EditorParse.comments`, the `Delimiters` fields, `LineIndex::position`.
- Produces:
  ```rust
  pub struct Fold { pub span: Span, pub collapsed_text: String, pub comment: bool }   // Debug, Clone, PartialEq, Eq
  pub fn folds(module: &ast::Module, comments: &[Span], text: &str) -> Vec<Fold>
  pub fn encode(folds: &[Fold], text: &str, line_index: &LineIndex) -> Vec<lsp_types::FoldingRange>
  Analysis.folds: Vec<Fold>
  ```

- [ ] **Step 1: Write the failing tests**

Create `crates/ns-lsp/src/folding.rs` with only its tests module first:

```rust
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
        let text = "class Module = () (\r\n    (* Grüße 😀 *) run = (\r\n        ^1\r\n    )\r\n)\r\n";
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = crate::documents::LineIndex::new(text);
        let ranges = encode(&folds(&parsed.module, &parsed.comments, text), text, &line_index);
        let method = &ranges[1];
        let open = text.find("= (\r\n        ^1").expect("the body") + 2;
        let line_start = text.find("    (* Grüße").expect("line 1");
        let character = text[line_start..open].encode_utf16().count() as u32;
        assert_eq!((method.start_line, method.start_character), (1, Some(character)));
        assert_eq!((method.end_line, method.end_character), (3, Some(5)));
        assert_eq!(method.kind, None);
        assert_eq!(method.collapsed_text.as_deref(), Some("(…)"));
    }
}
```

Add `pub mod folding;` to `src/lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p ns-lsp folding`
Expected: compile error — `folds`, `encode` not found.

- [ ] **Step 3: Compute the folds**

Above the tests in `folding.rs`:

```rust
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
    let mut collector = Collector { text, folds: Vec::new() };
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
            self.folds.push(Fold { span, collapsed_text, comment });
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
                message.arguments.iter().for_each(|argument| self.expression(argument));
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
            slots.last().map_or(delimiters.open.end, |slot| slot.span.end)
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
                send.message.arguments.iter().for_each(|argument| self.expression(argument));
            }
            ast::Expression::Cascade(cascade) => {
                self.expression(&cascade.receiver);
                for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                    message.arguments.iter().for_each(|argument| self.expression(argument));
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
```

- [ ] **Step 4: Run the folding tests**

Run: `cargo nextest run -p ns-lsp folding`
Expected: PASS. If `a_fold_is_encoded_in_utf16_on_crlf_lines` fails on the method's index, print `ranges` and fix the test's index to the method fold (the instance side is `ranges[0]`; the comment is single-line and does not fold).

- [ ] **Step 5: Wire it into the analysis and the server**

`Analysis` gets `pub folds: Vec<folding::Fold>`; `analyze_unguarded` computes `let folds = folding::folds(&resolution.module, &parsed.comments, text);` (take `parsed.comments` before `parsed.module` is moved: `let ns_syntax::EditorParse { tokens, module, comments, diagnostics } = ns_syntax::parse_for_editor(text);`); `guarded`'s panic branch answers `folds: Vec::new()`.

In `server.rs`, `capabilities()` gains

```rust
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
```

and `answer` dispatches on the method:

```rust
fn answer(documents: &HashMap<String, Document>, request: Request) -> Response {
    match request.method.as_str() {
        SemanticTokensFullRequest::METHOD => semantic_tokens_of(documents, request),
        FoldingRangeRequest::METHOD => folding_ranges_of(documents, request),
        _ => Response::new_err(
            request.id,
            ErrorCode::MethodNotFound as i32,
            format!("ns-next-lsp does not implement {}", request.method),
        ),
    }
}
```

`semantic_tokens_of` is today's body after the method check. `folding_ranges_of` mirrors it:

```rust
/// A document's folds; none for a document the client never opened or has
/// closed, as for its tokens.
fn folding_ranges_of(documents: &HashMap<String, Document>, request: Request) -> Response {
    let params: FoldingRangeParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(request.id, ErrorCode::InvalidParams as i32, error.to_string())
        }
    };
    let ranges = documents
        .get(params.text_document.uri.as_str())
        .map(|document| folding::encode(&document.analysis.folds, &document.text, &document.line_index))
        .unwrap_or_default();
    Response::new_ok(request.id, ranges)
}
```

Imports: `lsp_types::request::FoldingRangeRequest`, `lsp_types::{FoldingRangeParams, FoldingRangeProviderCapability}`, `crate::folding`. Update the module doc of `lib.rs` ("diagnostics, semantic tokens and fold ranges").

- [ ] **Step 6: Extend the protocol test**

In `tests/protocol.rs`'s `an_editor_session`, after the capability assertion on `semanticTokensProvider`:

```rust
    assert_eq!(capabilities["capabilities"]["foldingRangeProvider"], true);
```

and after the semantic-tokens requests, before `didClose`:

```rust
    let folded = indoc! {"
        class Module = Object new () (
            public run = (
                ^1
            )
        ) : ()
    "};
    notify(
        &client,
        "textDocument/didChange",
        json!({ "textDocument": { "uri": URI, "version": 3 }, "contentChanges": [{ "text": folded }] }),
    );
    let _ = next_diagnostics(&client);
    let ranges = request(
        &client,
        5,
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": URI } }),
    );
    let lines: Vec<(u64, u64)> = ranges
        .as_array()
        .expect("an array of ranges")
        .iter()
        .map(|range| {
            (
                range["startLine"].as_u64().expect("a start line"),
                range["endLine"].as_u64().expect("an end line"),
            )
        })
        .collect();
    assert_eq!(lines, [(0, 4), (1, 3)]);
```

Renumber the later `shutdown` request's id to `6`.

- [ ] **Step 7: Run the tests and the gates**

Run: `cargo nextest run -p ns-lsp` — PASS. Then every gate.

- [ ] **Step 8: Commit**

```bash
git add crates/ns-lsp
git commit -m "feat: the language server answers fold ranges for comments, classes, methods, closures and slot sections"
```

---

### Task 9: The plugin, checked by hand, and the documentation

**Files:**
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml`
- Modify: `editors/jetbrains/README.md`
- Modify: `docs/design/editor-support.md`, `docs/design/frontend.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`

- [ ] **Step 1: The plugin's description**

In `plugin.xml`'s `<description>` list, after the semantic colouring item:

```html
      <li>Code folding from ns-next's parser: comments, class bodies and class sides, method
          bodies, closures and slot sections — kept while a file has syntax errors.</li>
```

`<change-notes>`: `<p>0.1.0: highlighting, errors as you type, semantic colouring and code folding.</p>`. In `editors/jetbrains/README.md`'s first paragraph, "Syntax highlighting, diagnostics, semantic tokens and code folding".

- [ ] **Step 2: Check folding in a sandbox IDE**

Run `./editors/jetbrains/gradlew -p editors/jetbrains runIde` in the background and ask the user to open a `.ns` module (for example `crates/ns-runtime/core-library/Kernel.ns`) and confirm: the gutter shows fold markers on comments, class bodies, class sides, methods, closures and slot sections; a folded method shows `(…)`; typing `foo = (` inside a class keeps the folds of the methods after it. If no fold markers appear, the platform needs folding enabled for a language with its own parser definition: override `foldingRangeCustomizer` in `NewspeakLspClientDescriptor`'s `lspCustomization` with an `LspFoldingRangeSupport` subclass (confirm its members against `intellij.platform.lsp.jar` with `javap`, as `editor-support.md` describes for the other LSP names), add a `NewspeakEditorTest`-style test if the platform test framework can reach it, and recheck. Record the outcome in `editor-support.md` (Step 3).

- [ ] **Step 3: `docs/design/editor-support.md`**

- Status line and "Scope": a third in-scope capability, **Folding** — comments, class bodies, class sides, method bodies, closures and slot sections, from the server.
- "The language server": `analysis.rs` runs `parse_for_editor`, then `resolve_for_editor` on its module whether or not it was recovered, then the classification and the folds, and reports every finding together; add `folding.rs` to the module list.
- "Protocol surface": add `textDocument/foldingRange` and the `foldingRangeProvider` capability.
- A new subsection **Folding**: the table of the spec's Part 3 (range, from–to, collapsed text, kind), multi-line only, no kind for code folds and why, the platform's `LspFoldingBuilder` registered for every language and the outcome of Step 2.
- Replace "While a file does not parse there is no AST and so no semantic tokens…" with: a file with syntax errors is analysed as recovered (`frontend.md`, "Recovery"), so its colours, resolver errors and folds survive; a construct recovery cannot place (an unfinished body whose next line is a valid expression) folds worse, never wrongly.
- "Testing": the folding tests, the editor-parse agreement, the mid-token cuts and the empty-name check.
- "Planned later", **Navigation**: the fault-tolerant parse exists; what remains is `definition`, `references` and `documentSymbol` in the server. **Formatting**: comment spans exist (`parse_for_editor`); what remains is attaching them to nodes and the formatter.

- [ ] **Step 4: `docs/design/frontend.md`**

- "AST": `Delimiters` and the five fields that carry it, for the editor's folds; nothing in the runtime reads them.
- "Lexer": comments are skipped as trivia; `run_keeping_comments` also answers their spans, one per outermost comment, an unterminated one to the end.
- A new section **Recovery** after "Errors": the two entries (`parse` refuses on any error; `parse_for_editor` always answers a module, lexer errors included); the two resynchronisation means (balanced skipping, member starts); rules 1–7 of the spec's Part 2 stated as current behaviour; and "Left as it is".

- [ ] **Step 5: `docs/OPEN-WORK.md` and `docs/PROGRESS.md`**

`OPEN-WORK.md`, "Planned: editor support": **Formatting** — needs comments attached to AST nodes (their spans are kept, `parse_for_editor`) and an `ns-next fmt` command; **Navigation** — `definition`, `references` and `documentSymbol` in the server, on the fault-tolerant parse that exists. Add under "Open design questions" (or the editor section) only what review or Step 2 left open. `PROGRESS.md`: one milestone line for code folding and the fault-tolerant editor parse, in its table's form.

- [ ] **Step 6: Gates and commit**

Every gate (the plugin's `check` included), then:

```bash
git add editors/jetbrains docs
git commit -m "docs: folding and the editor parse in the design, the plugin's description and the open work"
```

---

### Task 10: Finish the batch

- [ ] **Step 1:** Invoke the `finishing-a-batch` skill and follow it: the stress modes, `run-corpus` plain and under stress, the benchmarks profile, `./editors/jetbrains/gradlew -p editors/jetbrains verifyPlugin buildPlugin`, the release-build size tests, and the documentation check that a fresh session can start from the documents alone.
