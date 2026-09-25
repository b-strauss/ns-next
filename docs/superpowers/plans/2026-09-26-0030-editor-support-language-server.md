# Editor Support, Part 1: the Language Server — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `ns-next-lsp`, a language server that reports every lexer, parser and resolver error of an open `.ns` file and classifies its names as semantic tokens, plus a `dump-tokens` mode the JetBrains plugin's lexer is checked against.

**Architecture:** A new crate `crates/ns-lsp` with its own binary, on `lsp-server` (rust-analyzer's synchronous transport), `lsp-types` and `serde_json`. It analyses a document with `ns-syntax`'s lexer and parser and a new resolver entry in `ns-runtime` that answers bindings and errors together; it never starts the VM. One thread with an 8 MiB stack runs the whole server, because the symbol interner is thread-local and the front end recurses.

**Tech Stack:** Rust 1.98.0 (edition 2021), `lsp-server` 0.10.0, `lsp-types` 0.97.0, `serde_json` 1.

**Spec:** `docs/design/editor-support.md` — read it first; this plan argues from it. Part 2, the JetBrains plugin, is `docs/superpowers/plans/2026-09-26-0031-editor-support-jetbrains-plugin.md` and depends on this part being complete.

## Global Constraints

- Work in the worktree `D:\Workspace\ns-next-editor` on branch `feature/editor-support`. Never `cd` into `D:\Workspace\ns-next`: another session works there.
- All code, identifiers, comments, commit messages and developer docs in English.
- Names are written out: no identifier of one or two characters, no truncated word (`diagnostic`, not `diag`; `position`, not `pos`; `index`, not `idx`). `cargo clippy` enforces the length through `min_ident_chars`; the truncation rule is yours to keep. This applies to test code and closures too (`|diagnostic| …`, never `|d| …`).
- Conventional commits (`feat:`, `fix:`, `test:`, `docs:`, `chore:`). Every commit ends with the `Co-Authored-By:` trailer naming the model that actually wrote the code, as the dispatch instructions give it.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult` (not reachable from this crate, but the rule is workspace-wide).
- `ns-syntax` does not change in this part.
- The new dependencies live in `crates/ns-lsp/Cargo.toml` only. `ns-runtime`, `ns-syntax` and `ns-cli` gain no dependency.
- The resolver entry never records `Binding::MirrorSend` and never calls `resolve_kernel` (`CLAUDE.md`, "The VM mirror is a capability").
- A Newspeak module written inline in a test is an `indoc! {"…"}`, indented with the Rust around it, the closing `"}` on its own line.
- Per-commit gate for every task: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (warning-free), `cargo test -p ns-lsp`, and — for Task 1, which touches `ns-runtime` — `cargo test --workspace`. The corpus gates (`parse-corpus`, `run-corpus`) and the stress modes run once at the end of the part (Task 7), not per commit.
- Run every command in the foreground. Do not hand back while a background command is still running.

## Review Focus

1. **Text that is not ASCII.** Umlauts, CJK and emoji (surrogate pairs in UTF-16) in strings and comments: every position the server sends must count UTF-16 code units, never bytes or chars. Pinned in Task 2 and by the end-to-end test in Task 6.
2. **A file mid-edit.** Every prefix of a real module — cut at a token boundary — must analyse without an "internal error" finding and without the server dying. Pinned in Task 3's robustness test.
3. **`\r\n` and lone `\r` line ends.** LSP counts both as line breaks; a Windows file must not shift every position after its first line. Pinned in Task 2.
4. **A document the server never opened.** A `semanticTokens/full` request for an unknown URI must answer an empty result, not crash or hang. Pinned in Task 6.
5. **An explicit receiver inside a keyword send's span.** `MessageSend.span` of a keyword send starts at its *receiver* (`crates/ns-syntax/src/parser/expression.rs`, `parse_keyword_message`), so `self`, `outer N` or any receiver expression lies inside the message's span and must not be coloured as the selector. Pinned in Task 4.

---

## File structure

```
crates/ns-runtime/src/types.rs          + EditorResolution (resolver output for the editor)
crates/ns-runtime/src/resolver/mod.rs   + resolve_for_editor; resolve_with split around a shared walk
crates/ns-runtime/src/lib.rs            + re-export resolve_for_editor
Cargo.toml                              + crates/ns-lsp in [workspace] members
crates/ns-lsp/Cargo.toml                the crate and its binary `ns-next-lsp`
crates/ns-lsp/src/lib.rs                module declarations
crates/ns-lsp/src/documents.rs          LineIndex: byte offset -> LSP Position (UTF-16)
crates/ns-lsp/src/analysis.rs           analyze(): findings + classified names, panic-guarded
crates/ns-lsp/src/semantic_tokens.rs    classification of names, LSP encoding, the legend
crates/ns-lsp/src/token_dump.rs         the JSON `dump-tokens` prints
crates/ns-lsp/src/server.rs             the LSP message loop and the open documents
crates/ns-lsp/src/main.rs               argument handling; the 8 MiB server thread
crates/ns-lsp/tests/corpus.rs           resolver-entry equivalence and robustness over every module
crates/ns-lsp/tests/protocol.rs         end-to-end test over an in-memory connection
```

---

### Task 1: The resolver entry `resolve_for_editor`

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (the "Resolver output" section, next to `ResolvedModule`)
- Modify: `crates/ns-runtime/src/resolver/mod.rs:20-65` (`resolve`, `resolve_kernel`, `resolve_with`)
- Modify: `crates/ns-runtime/src/lib.rs` (`pub use resolver::resolve;`)
- Test: `crates/ns-runtime/src/resolver/mod.rs` (its `#[cfg(test)] mod tests`; create the module at the end of the file if it has none)

**Interfaces:**
- Produces:
  ```rust
  pub struct EditorResolution {
      pub module: ns_syntax::ast::Module,
      pub bindings: std::collections::HashMap<ns_syntax::Span, Binding>,
      pub errors: Vec<ResolveError>,
  }
  pub fn resolve_for_editor(module: ns_syntax::ast::Module) -> EditorResolution;
  ```
  re-exported as `ns_runtime::resolve_for_editor` and `ns_runtime::EditorResolution`.

- [ ] **Step 1: Write the failing tests**

Add to the resolver's test module:

```rust
#[test]
fn the_editor_entry_answers_bindings_and_errors_together() {
    let source = indoc::indoc! {"
        Newspeak3
        'Test'
        class Module = Object new (
        ) (
          public broken = ( ^outer Missing foo )
          public echo: argument = ( ^argument )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("the fixture parses");
    let resolution = crate::resolve_for_editor(module);
    assert_eq!(resolution.errors.len(), 1, "one `outer Missing`");
    assert!(
        resolution.errors[0].message.contains("outer Missing"),
        "{}",
        resolution.errors[0].message
    );
    let start = source.rfind("argument )").expect("the use of `argument`") as u32;
    let use_of_argument = ns_syntax::Span::new(start, start + "argument".len() as u32);
    assert_eq!(
        resolution.bindings.get(&use_of_argument),
        Some(&Binding::Local(0)),
        "the error in `broken` must not cost `echo:` its bindings"
    );
}

#[test]
fn the_editor_entry_resolves_the_kernel_without_the_mirror_mark() {
    let source = include_str!("../../core-library/Kernel.ns");
    let module = ns_syntax::parse(source).expect("Kernel.ns parses");
    let resolution = crate::resolve_for_editor(module);
    assert!(
        resolution.errors.is_empty(),
        "Kernel.ns resolves through the user path too"
    );
    assert!(
        !resolution
            .bindings
            .values()
            .any(|binding| *binding == Binding::MirrorSend),
        "only resolve_kernel may record MirrorSend"
    );
}
```

If the test module does not import `Binding` yet, add `use crate::types::Binding;` inside it.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime the_editor_entry`
Expected: FAIL to compile — `cannot find function resolve_for_editor in the crate root`.

- [ ] **Step 3: Add `EditorResolution` to `types.rs`**

Directly below `pub struct ResolvedModule { … }`:

```rust
/// What the editor's language server needs from a resolution: every binding
/// the walk recorded *and* every error it found, together. `resolve` answers
/// one or the other, which is right for compiling -- a module with an error
/// must not run -- but an editor must keep colouring the rest of a file while
/// one name in it is wrong. Never compiled from: only `ResolvedModule` is.
pub struct EditorResolution {
    pub module: ast::Module,
    pub bindings: HashMap<Span, Binding>,
    pub errors: Vec<ResolveError>,
}
```

Use whatever path `types.rs` already uses for `ast`, `HashMap` and `Span` in `ResolvedModule` (it names `ast::Module`, `HashMap` and `Span` there already).

- [ ] **Step 4: Split `resolve_with` around one shared walk and add the entry**

Replace `resolve_with` in `crates/ns-runtime/src/resolver/mod.rs` with:

```rust
/// The one body of `resolve` and `resolve_kernel`: `kernel` is the only
/// thing that differs, so a user module and the kernel can never be resolved
/// by two drifting copies of the walk.
fn resolve_with(
    module: ast::Module,
    source: Rc<str>,
    origin: SourceOrigin,
    kernel: bool,
) -> Result<ResolvedModule, Vec<ResolveError>> {
    let (bindings, errors) = walk(&module, kernel);
    if errors.is_empty() {
        Ok(ResolvedModule {
            module: Rc::new(module),
            source,
            origin,
            bindings: Rc::new(bindings),
            kernel,
        })
    } else {
        Err(errors)
    }
}

/// `resolve`'s walk for the editor's language server, answering what it
/// found whether or not it found errors. Always the user path (`kernel` is
/// false): no `Binding::MirrorSend` is ever recorded here, so this entry
/// cannot be used to reach the VM mirror (`docs/design/core-library.md`,
/// "The VM mirror is the whole Rust boundary"), and `Kernel.ns` opened in an
/// editor shows its `vmMirror` sends as the ordinary sends they are to it.
pub fn resolve_for_editor(module: ast::Module) -> EditorResolution {
    let (bindings, errors) = walk(&module, false);
    EditorResolution {
        module,
        bindings,
        errors,
    }
}

/// The walk every entry shares.
fn walk(module: &ast::Module, kernel: bool) -> (HashMap<Span, Binding>, Vec<ResolveError>) {
    let mut resolver = Resolver {
        bindings: HashMap::new(),
        errors: Vec::new(),
        kernel,
    };
    resolver.resolve_class(&module.root_class, &[]);
    (resolver.bindings, resolver.errors)
}
```

Add `EditorResolution` to the `use crate::types::{…}` line at the top of the file. Keep every field of the `ResolvedModule` literal that exists today — if `ResolvedModule` has gained fields since this plan was written, carry them over unchanged.

- [ ] **Step 5: Re-export it**

In `crates/ns-runtime/src/lib.rs` change `pub use resolver::resolve;` to:

```rust
pub use resolver::{resolve, resolve_for_editor};
```

`EditorResolution` is already exported through `pub use types::*;`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime the_editor_entry`
Expected: PASS, 2 tests.

- [ ] **Step 7: Run the per-commit gate**

Run: `cargo fmt --all`, then `cargo clippy --workspace --all-targets`, then `cargo test --workspace`
Expected: clippy prints no warning; every test passes.

- [ ] **Step 8: Commit**

```bash
git add crates/ns-runtime/src/types.rs crates/ns-runtime/src/resolver/mod.rs crates/ns-runtime/src/lib.rs
git commit -m "feat: resolver entry answering bindings and errors together, for the editor"
```

---

### Task 2: The crate, and positions in UTF-16

**Files:**
- Modify: `Cargo.toml` (workspace `members`)
- Create: `crates/ns-lsp/Cargo.toml`
- Create: `crates/ns-lsp/src/lib.rs`
- Create: `crates/ns-lsp/src/main.rs` (a stub this task; Task 5 and 6 fill it)
- Create: `crates/ns-lsp/src/documents.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct LineIndex { /* private */ }
  impl LineIndex {
      pub fn new(text: &str) -> LineIndex;
      /// `offset` is a byte offset into the `text` the index was built from.
      pub fn position(&self, text: &str, offset: u32) -> lsp_types::Position;
  }
  /// UTF-16 length of `text[start..end]`.
  pub fn utf16_length(text: &str, span: ns_syntax::Span) -> u32;
  ```

- [ ] **Step 1: Add the crate to the workspace**

In the root `Cargo.toml`:

```toml
members = ["crates/ns-syntax", "crates/ns-runtime", "crates/ns-cli", "crates/ns-lsp"]
```

Create `crates/ns-lsp/Cargo.toml`:

```toml
[package]
name = "ns-lsp"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
description = "Language server for Newspeak, on ns-next's front end and resolver"

# A binary of its own, so that the LSP transport and JSON never reach the
# `ns-next` binary: the editor pays for them, the runtime does not.
[[bin]]
name = "ns-next-lsp"
path = "src/main.rs"

[dependencies]
ns-syntax = { path = "../ns-syntax" }
ns-runtime = { path = "../ns-runtime" }
# The LSP protocol is large and still growing; writing its types and
# transport by hand would be code with nothing to do with Newspeak, unlike
# the image format, where owning every byte is the point.
lsp-server = "0.10"
lsp-types = "0.97"
serde_json = "1"

[dev-dependencies]
indoc.workspace = true

[lints]
workspace = true
```

Create `crates/ns-lsp/src/lib.rs`:

```rust
#![warn(clippy::all)]
#![forbid(unsafe_code)]

//! The language server behind ns-next's editor support: diagnostics and
//! semantic tokens for `.ns` files. See `docs/design/editor-support.md`.

pub mod documents;
```

Create `crates/ns-lsp/src/main.rs`:

```rust
fn main() {}
```

- [ ] **Step 2: Write the failing tests**

Create `crates/ns-lsp/src/documents.rs` with only the tests first:

```rust
//! The open documents' text, and the conversion from the byte offsets every
//! `Span` holds to the positions LSP speaks.

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn position_of(text: &str, offset: u32) -> Position {
        LineIndex::new(text).position(text, offset)
    }

    #[test]
    fn ascii_counts_one_unit_per_byte() {
        assert_eq!(position_of("abc", 2), Position::new(0, 2));
    }

    #[test]
    fn a_line_feed_starts_a_new_line() {
        let text = "ab\ncd";
        assert_eq!(position_of(text, 3), Position::new(1, 0));
        assert_eq!(position_of(text, 5), Position::new(1, 2));
    }

    #[test]
    fn carriage_return_line_feed_is_one_break() {
        let text = "ab\r\ncd";
        assert_eq!(position_of(text, 4), Position::new(1, 0));
    }

    #[test]
    fn a_lone_carriage_return_is_a_break() {
        let text = "ab\rcd";
        assert_eq!(position_of(text, 3), Position::new(1, 0));
    }

    #[test]
    fn an_umlaut_is_one_utf16_unit_but_two_bytes() {
        let text = "'ä' x";
        // `x` is at byte 5 and UTF-16 unit 4.
        assert_eq!(position_of(text, 5), Position::new(0, 4));
    }

    #[test]
    fn an_emoji_is_two_utf16_units_but_four_bytes() {
        let text = "'😀' x";
        // `x` is at byte 7 and UTF-16 unit 5.
        assert_eq!(position_of(text, 7), Position::new(0, 5));
    }

    #[test]
    fn the_end_of_the_text_is_a_position() {
        let text = "ab\n";
        assert_eq!(position_of(text, 3), Position::new(1, 0));
    }

    #[test]
    fn an_offset_inside_a_character_answers_the_character_start() {
        // A span is codepoint-aligned by `ns-syntax`'s contract; this guards
        // the server against a violation turning into a slicing panic.
        let text = "ä";
        assert_eq!(position_of(text, 1), Position::new(0, 0));
    }

    #[test]
    fn an_offset_past_the_end_answers_the_end() {
        assert_eq!(position_of("ab", 99), Position::new(0, 2));
    }

    #[test]
    fn utf16_length_counts_surrogate_pairs_twice() {
        let text = "'😀'";
        assert_eq!(utf16_length(text, ns_syntax::Span::new(0, 6)), 4);
    }
}
```

And in `lib.rs` the module is already declared.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p ns-lsp`
Expected: FAIL to compile — `cannot find type LineIndex`.

- [ ] **Step 4: Implement `LineIndex` and `utf16_length`**

Above the test module in `documents.rs`:

```rust
use lsp_types::Position;
use ns_syntax::Span;

/// Where each line of a text starts, as byte offsets. LSP counts `\n`,
/// `\r\n` and a lone `\r` as line breaks, so all three do here.
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let bytes = text.as_bytes();
        let mut line_starts = vec![0];
        for (index, byte) in bytes.iter().enumerate() {
            let breaks_here = match byte {
                b'\n' => true,
                b'\r' => bytes.get(index + 1) != Some(&b'\n'),
                _ => false,
            };
            if breaks_here {
                line_starts.push(index as u32 + 1);
            }
        }
        LineIndex { line_starts }
    }

    /// The LSP position of byte `offset` in `text`, its character counted in
    /// UTF-16 code units -- the one encoding every LSP client must accept.
    /// An offset past the end answers the end, and one inside a character
    /// answers that character's start, so a span that broke `ns-syntax`'s
    /// codepoint alignment is reported slightly off rather than panicking.
    pub fn position(&self, text: &str, offset: u32) -> Position {
        let mut offset = (offset as usize).min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self
            .line_starts
            .partition_point(|start| *start as usize <= offset)
            - 1;
        let line_start = self.line_starts[line] as usize;
        let character = text[line_start..offset].encode_utf16().count();
        Position::new(line as u32, character as u32)
    }
}

/// The length of `span` in `text`, in UTF-16 code units.
pub fn utf16_length(text: &str, span: Span) -> u32 {
    let start = (span.start as usize).min(text.len());
    let end = (span.end as usize).clamp(start, text.len());
    text.get(start..end)
        .map_or(0, |slice| slice.encode_utf16().count() as u32)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ns-lsp`
Expected: PASS, 10 tests.

- [ ] **Step 6: Run the per-commit gate and commit**

Run: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (no warning), `cargo test -p ns-lsp`.

```bash
git add Cargo.toml Cargo.lock crates/ns-lsp
git commit -m "feat: ns-lsp crate with UTF-16 positions"
```

---

### Task 3: Analysis — findings, the panic guard, and the corpus tests

**Files:**
- Create: `crates/ns-lsp/src/analysis.rs`
- Create: `crates/ns-lsp/src/semantic_tokens.rs` (types only in this task: `Classified`, `TokenCategory`, and a `classify` that answers an empty list; Task 4 implements it)
- Modify: `crates/ns-lsp/src/lib.rs`
- Create: `crates/ns-lsp/tests/corpus.rs`

**Interfaces:**
- Consumes: `ns_runtime::resolve_for_editor`, `ns_runtime::EditorResolution` (Task 1).
- Produces:
  ```rust
  // analysis.rs
  pub struct Finding { pub span: ns_syntax::Span, pub message: String, pub severity: ns_syntax::Severity }
  pub struct Analysis { pub findings: Vec<Finding>, pub classified: Vec<crate::semantic_tokens::Classified> }
  pub fn analyze(text: &str) -> Analysis;
  pub(crate) fn guarded(text: &str, analyzer: impl FnOnce(&str) -> Analysis) -> Analysis;
  // semantic_tokens.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum TokenCategory { Parameter, Variable, Property, Class, Method }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct Classified { pub span: ns_syntax::Span, pub category: TokenCategory, pub declaration: bool, pub readonly: bool }
  pub fn classify(module: &ns_syntax::ast::Module, bindings: &std::collections::HashMap<ns_syntax::Span, ns_runtime::Binding>, tokens: &[ns_syntax::Token]) -> Vec<Classified>;
  ```

- [ ] **Step 1: Add the semantic-token types with an empty `classify`**

Create `crates/ns-lsp/src/semantic_tokens.rs`:

```rust
//! What the resolver knows about each name, as LSP semantic tokens.

use std::collections::HashMap;

use ns_runtime::Binding;
use ns_syntax::ast;
use ns_syntax::{Span, Token};

/// The token types of the legend, in legend order (`legend_types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenCategory {
    Parameter,
    Variable,
    Property,
    Class,
    Method,
}

/// One classified name: where it is, what it is, and whether this is its
/// declaration and whether it can never be assigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    pub span: Span,
    pub category: TokenCategory,
    pub declaration: bool,
    pub readonly: bool,
}

/// Every name in `module` the resolver can place. Task 4 implements it.
pub fn classify(
    _module: &ast::Module,
    _bindings: &HashMap<Span, Binding>,
    _tokens: &[Token],
) -> Vec<Classified> {
    Vec::new()
}
```

Add `pub mod analysis;` and `pub mod semantic_tokens;` to `lib.rs`.

- [ ] **Step 2: Write the failing unit tests**

Create `crates/ns-lsp/src/analysis.rs` with only its tests:

```rust
//! One document's analysis: the errors `ns-next` would report before running
//! it, and what the resolver knows about each name.

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(text: &str) -> Vec<String> {
        analyze(text)
            .findings
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    #[test]
    fn a_clean_module_has_no_findings() {
        let text = indoc::indoc! {"
            Newspeak3
            'Test'
            class Module = Object new () () : ()
        "};
        assert!(messages(text).is_empty(), "{:?}", messages(text));
    }

    #[test]
    fn a_lexer_error_is_reported_where_it_is() {
        let text = indoc::indoc! {"
            Newspeak3
            'Test'
            class Module = Object new () ( public broken = ( ^'unterminated ) ) : ()
        "};
        let analysis = analyze(text);
        assert_eq!(analysis.findings.len(), 1);
        assert_eq!(analysis.findings[0].message, "unterminated string literal");
        assert_eq!(
            analysis.findings[0].span.start as usize,
            text.find("'unterminated").expect("the literal")
        );
    }

    #[test]
    fn a_parser_error_is_reported() {
        let text = indoc::indoc! {"
            Newspeak3
            class Module = Object new () () : ()
        "};
        let found = messages(text);
        // Recovery may add further errors after the first; the first one is
        // the parser's own message for the missing category.
        assert_eq!(found.first().map(String::as_str), Some("expected a category string"), "{found:?}");
    }

    #[test]
    fn a_resolver_error_is_reported() {
        let text = indoc::indoc! {"
            Newspeak3
            'Test'
            class Module = Object new () ( public broken = ( ^outer Missing foo ) ) : ()
        "};
        let found = messages(text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("outer Missing"), "{}", found[0]);
    }

    #[test]
    fn a_panic_becomes_one_internal_error_finding() {
        let analysis = guarded("anything", |_| panic!("the analyzer broke"));
        assert_eq!(analysis.findings.len(), 1);
        assert!(
            analysis.findings[0].message.contains("internal error"),
            "{}",
            analysis.findings[0].message
        );
        assert!(
            analysis.findings[0].message.contains("the analyzer broke"),
            "{}",
            analysis.findings[0].message
        );
        assert_eq!(analysis.findings[0].span, ns_syntax::Span::new(0, 0));
    }
}
```

Check the parser's exact message for a missing category before relying on it: run `cargo run -p ns-cli -- parse` on a file without a category, or grep `crates/ns-syntax/src/parser` for `category string`. If the text differs, use the text the parser produces — it is the parser's message, not this crate's.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p ns-lsp analysis`
Expected: FAIL to compile — `cannot find function analyze`.

- [ ] **Step 4: Implement `analyze` and `guarded`**

Above the test module in `analysis.rs`:

```rust
use std::panic::{catch_unwind, AssertUnwindSafe};

use ns_syntax::{Diagnostic, Severity, Span};

use crate::semantic_tokens::{self, Classified};

/// One error to show, with the message `ns-next` itself produces.
pub struct Finding {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
}

impl From<Diagnostic> for Finding {
    fn from(diagnostic: Diagnostic) -> Finding {
        Finding {
            span: diagnostic.span,
            message: diagnostic.message,
            severity: diagnostic.severity,
        }
    }
}

pub struct Analysis {
    pub findings: Vec<Finding>,
    pub classified: Vec<Classified>,
}

/// The errors `ns-next run` would refuse `text` for before running it -- the
/// lexer's, else the parser's, else the resolver's -- and, when it parses,
/// every name the resolver can place. Never panics: see `guarded`.
pub fn analyze(text: &str) -> Analysis {
    guarded(text, analyze_unguarded)
}

/// Runs `analyzer`, turning a panic into one finding at the start of the
/// file. An editor sends invalid text all the time and `ns-syntax` promises
/// diagnostics rather than panics for it; a panic here is therefore an
/// ns-next defect, and it must be visible without taking the server, and the
/// editor's errors for every other file, down with it.
pub(crate) fn guarded(text: &str, analyzer: impl FnOnce(&str) -> Analysis) -> Analysis {
    match catch_unwind(AssertUnwindSafe(|| analyzer(text))) {
        Ok(analysis) => analysis,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no message".to_string());
            Analysis {
                findings: vec![Finding {
                    span: Span::new(0, 0),
                    message: format!(
                        "internal error in ns-next-lsp: {detail}; this is an ns-next defect, please report it"
                    ),
                    severity: Severity::Error,
                }],
                classified: Vec::new(),
            }
        }
    }
}

fn analyze_unguarded(text: &str) -> Analysis {
    // The lexer runs on its own first because `ns_syntax::parse` refuses a
    // token stream with lexer errors before parsing, and because the
    // classification needs the tokens themselves.
    let (tokens, lexer_diagnostics) = ns_syntax::lexer::Lexer::new(text).run();
    if lexer_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Analysis {
            findings: lexer_diagnostics.into_iter().map(Finding::from).collect(),
            classified: Vec::new(),
        };
    }
    let module = match ns_syntax::parse(text) {
        Ok(module) => module,
        Err(diagnostics) => {
            return Analysis {
                findings: diagnostics.into_iter().map(Finding::from).collect(),
                classified: Vec::new(),
            }
        }
    };
    let resolution = ns_runtime::resolve_for_editor(module);
    let classified =
        semantic_tokens::classify(&resolution.module, &resolution.bindings, &tokens);
    Analysis {
        findings: resolution
            .errors
            .into_iter()
            .map(|error| Finding {
                span: error.span,
                message: error.message,
                severity: Severity::Error,
            })
            .collect(),
        classified,
    }
}
```

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: `cargo test -p ns-lsp analysis`
Expected: PASS, 5 tests. The panic test prints the panic message on stderr; that is the default hook and expected.

- [ ] **Step 6: Write the corpus tests**

Create `crates/ns-lsp/tests/corpus.rs`:

```rust
//! The server's analysis over every Newspeak module the repository holds:
//! the resolver entry must agree with `resolve`, and nothing -- not even a
//! module cut off mid-way, which is what a file looks like while it is typed
//! -- may produce an internal error.

use std::path::{Path, PathBuf};

fn modules() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    collect(&root.join("corpus/modules"), &mut files);
    collect(&root.join("goldens"), &mut files);
    files.push(root.join("crates/ns-runtime/core-library/Kernel.ns"));
    files.sort();
    assert!(files.len() > 200, "found only {} modules", files.len());
    files
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("reading {}: {error}", directory.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "ns") {
            files.push(path);
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_editor_entry_agrees_with_resolve() {
    for path in modules() {
        let text = read(&path);
        let Ok(module) = ns_syntax::parse(&text) else {
            continue;
        };
        let resolution = ns_runtime::resolve_for_editor(module.clone());
        let origin = ns_runtime::SourceOrigin::File(path.clone());
        match ns_runtime::resolve(module, text.as_str().into(), origin) {
            Ok(resolved) => {
                assert!(
                    resolution.errors.is_empty(),
                    "{}: resolve found no error, the editor entry did",
                    path.display()
                );
                assert!(
                    *resolved.bindings == resolution.bindings,
                    "{}: the bindings differ",
                    path.display()
                );
            }
            Err(errors) => {
                let expected: Vec<_> = errors
                    .iter()
                    .map(|error| (error.span, error.message.clone()))
                    .collect();
                let actual: Vec<_> = resolution
                    .errors
                    .iter()
                    .map(|error| (error.span, error.message.clone()))
                    .collect();
                assert_eq!(expected, actual, "{}", path.display());
            }
        }
    }
}

fn assert_no_internal_error(text: &str, context: &str) {
    for finding in ns_lsp::analysis::analyze(text).findings {
        assert!(
            !finding.message.contains("internal error"),
            "{context}: {}",
            finding.message
        );
    }
}

#[test]
fn no_module_produces_an_internal_error() {
    for path in modules() {
        assert_no_internal_error(&read(&path), &path.display().to_string());
    }
}

/// Every prefix of a module ending at a token boundary, for the goldens and
/// the kernel; for the corpus modules, which are far larger, 64 prefixes
/// spread evenly over the file. Every prefix of every corpus module is
/// quadratic in its size and would make this test take minutes in a debug
/// build; 64 cut points per module still cut inside every kind of
/// construct the corpus holds.
#[test]
fn no_truncated_module_produces_an_internal_error() {
    for path in modules() {
        let text = read(&path);
        let (tokens, _) = ns_syntax::lexer::Lexer::new(&text).run();
        let boundaries: Vec<usize> = tokens.iter().map(|token| token.span.end as usize).collect();
        let in_corpus = path.components().any(|component| component.as_os_str() == "corpus");
        let stride = if in_corpus {
            (boundaries.len() / 64).max(1)
        } else {
            1
        };
        for end in boundaries.iter().step_by(stride) {
            let context = format!("{} cut at byte {end}", path.display());
            assert_no_internal_error(&text[..*end], &context);
        }
    }
}
```

- [ ] **Step 7: Run the corpus tests**

Run: `cargo test -p ns-lsp --test corpus`
Expected: PASS, 3 tests. If `the_editor_entry_agrees_with_resolve` fails, Task 1's split changed behaviour — fix the split, never the test. If a truncation produces an internal error, that is an `ns-syntax` or resolver panic on malformed input: report it to the controller with the file and byte offset, do not work around it in this crate.

- [ ] **Step 8: Run the per-commit gate and commit**

Run: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (no warning), `cargo test -p ns-lsp`.

```bash
git add crates/ns-lsp
git commit -m "feat: ns-lsp analysis with lexer, parser and resolver findings"
```

---

### Task 4: Classification and the semantic-token encoding

**Files:**
- Modify: `crates/ns-lsp/src/semantic_tokens.rs`

**Interfaces:**
- Consumes: `Classified`, `TokenCategory` (Task 3); `LineIndex`, `utf16_length` (Task 2); `analyze` (Task 3) in tests.
- Produces:
  ```rust
  pub fn classify(module: &ast::Module, bindings: &HashMap<Span, Binding>, tokens: &[Token]) -> Vec<Classified>;
  pub fn legend() -> lsp_types::SemanticTokensLegend;
  pub fn encode(classified: &[Classified], text: &str, line_index: &crate::documents::LineIndex) -> Vec<lsp_types::SemanticToken>;
  ```
  Legend token types, in this order: `parameter`, `variable`, `property`, `class`, `method`. Modifiers, in this order: `declaration` (bit 0), `readonly` (bit 1). Part 2's plugin maps exactly these strings.

How the resolver's output maps to categories (from `docs/design/editor-support.md`, "Classification", and `crates/ns-runtime/src/resolver/mod.rs`):

- `Binding::Local` / `Binding::OuterLocal` — a parameter or a local, decided by the innermost declaration of that name in the scopes the walk has entered (method or factory parameters and temporaries, block parameters and temporaries). An immutable temporary (`| step = 1. |`) is `readonly`.
- `Binding::LexicalSend { depth }` — a send to the class `depth` levels out in the lexical chain (0 is the innermost class). The name is looked up among what that class *declares*, both sides together: a slot getter is `property` (`readonly` when declared with `=`), a mutable slot's setter `name:` is `property`, a nested class is `class`, a method or the factory selector is `method`. A name that class does not declare — it only inherits it, or it is a predefined class like `Object` — is `method`: it is a send.
- A superclass or `<:` clause's receiver of a *nested* class is resolved against the chain *enclosing* that class, not including it (`resolve_superclass_clause`); a module root's clauses bind nothing.
- `Binding::SuperSend` — the selector is `method`.
- An explicit-receiver send with no binding is not classified: the lexer's colours stand.
- `self`, `super`, `outer`, `true`, `false`, `nil` are never classified: they are reserved words (`newspeak-spec.pdf` §4.1), and the plugin's lexer colours them.
- The selector of a classified send is found among the lexer's tokens: tokens of kind `Identifier`, `Keyword`, `SetterKeyword` or `BinarySelector` inside the message's span, outside the span of the send's receiver and of every argument. `MessageSend.span` of a keyword send starts at the receiver, which is why the receiver's span is excluded.

- [ ] **Step 1: Write the failing tests**

Append to `semantic_tokens.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const MODULE: &str = indoc::indoc! {"
        Newspeak3
        'Test'
        class Shapes usingPlatform: platform = Object new (
          | private count ::= 0. private limit = 10. |
        ) (
          class Inner = Object new () ()
          public grow: amount = (
            | step = 1. total |
            total:: amount + step.
            count:: count + total.
            ^limit
          )
          public make = ( ^Inner new )
          public again = ( ^self grow: 3 )
          public nested = ( ^outer Shapes grow: 4 )
          public each = ( ^[:element | element] )
          public inherited = ( ^printString )
        ) : ()
    "};

    /// What the `occurrence`-th (0-based) appearance of `text` in `MODULE`
    /// was classified as, or `None` when nothing was.
    fn classified_at(text: &str, occurrence: usize) -> Option<(TokenCategory, bool, bool)> {
        let start = MODULE
            .match_indices(text)
            .nth(occurrence)
            .unwrap_or_else(|| panic!("`{text}` #{occurrence} is not in the module"))
            .0 as u32;
        let analysis = crate::analysis::analyze(MODULE);
        assert!(analysis.findings.is_empty(), "the module is clean");
        analysis
            .classified
            .iter()
            .find(|classified| classified.span.start == start)
            .map(|classified| (classified.category, classified.declaration, classified.readonly))
    }

    use TokenCategory::{Class, Method, Parameter, Property, Variable};

    #[test]
    fn declarations() {
        assert_eq!(classified_at("Shapes", 0), Some((Class, true, false)));
        assert_eq!(classified_at("platform", 0), Some((Parameter, true, false)));
        assert_eq!(classified_at("count", 0), Some((Property, true, false)));
        assert_eq!(classified_at("limit", 0), Some((Property, true, true)));
        assert_eq!(classified_at("Inner", 0), Some((Class, true, false)));
        assert_eq!(classified_at("grow:", 0), Some((Method, true, false)));
        assert_eq!(classified_at("amount", 0), Some((Parameter, true, false)));
        assert_eq!(classified_at("step", 0), Some((Variable, true, true)));
        assert_eq!(classified_at("total", 0), Some((Variable, true, false)));
        assert_eq!(classified_at("element", 0), Some((Parameter, true, false)));
    }

    #[test]
    fn locals_and_parameters_in_use() {
        assert_eq!(classified_at("total", 1), Some((Variable, false, false)));
        assert_eq!(classified_at("amount", 1), Some((Parameter, false, false)));
        assert_eq!(classified_at("step", 1), Some((Variable, false, true)));
        assert_eq!(classified_at("element", 1), Some((Parameter, false, false)));
    }

    #[test]
    fn slots_in_use_are_properties() {
        // `count::` is the setter send to the mutable slot, `count` its getter.
        assert_eq!(classified_at("count", 1), Some((Property, false, false)));
        assert_eq!(classified_at("count", 2), Some((Property, false, false)));
        assert_eq!(classified_at("limit", 1), Some((Property, false, true)));
    }

    #[test]
    fn a_nested_class_in_use_is_a_class() {
        assert_eq!(classified_at("Inner", 1), Some((Class, false, false)));
    }

    #[test]
    fn self_and_outer_sends_colour_the_selector_not_the_receiver() {
        assert_eq!(classified_at("grow:", 1), Some((Method, false, false)));
        assert_eq!(classified_at("self", 0), None);
        assert_eq!(classified_at("grow:", 2), Some((Method, false, false)));
        assert_eq!(classified_at("outer", 0), None);
        assert_eq!(classified_at("Shapes", 1), None);
    }

    #[test]
    fn inherited_and_predefined_names_are_sends() {
        assert_eq!(classified_at("printString", 0), Some((Method, false, false)));
        // `Object` in the nested class's clause resolves against `Shapes`,
        // which does not declare it.
        assert_eq!(classified_at("Object", 1), Some((Method, false, false)));
    }

    #[test]
    fn a_module_roots_clause_is_not_classified() {
        assert_eq!(classified_at("Object", 0), None);
    }

    #[test]
    fn an_explicit_receiver_send_is_not_classified() {
        // `new` in `Inner new` has an explicit receiver and no binding.
        let start = MODULE.find("Inner new").expect("the send") + "Inner ".len();
        let analysis = crate::analysis::analyze(MODULE);
        assert!(!analysis
            .classified
            .iter()
            .any(|classified| classified.span.start as usize == start));
    }

    #[test]
    fn encoding_is_relative_and_in_utf16() {
        let text = "'ä' ab\n  cd";
        let line_index = crate::documents::LineIndex::new(text);
        let classified = [
            Classified {
                span: Span::new(5, 7),
                category: TokenCategory::Variable,
                declaration: true,
                readonly: false,
            },
            Classified {
                span: Span::new(10, 12),
                category: TokenCategory::Method,
                declaration: false,
                readonly: true,
            },
        ];
        let encoded = encode(&classified, text, &line_index);
        assert_eq!(encoded.len(), 2);
        // `ab` is at line 0, UTF-16 column 4 (the umlaut is one unit).
        assert_eq!(
            (encoded[0].delta_line, encoded[0].delta_start, encoded[0].length),
            (0, 4, 2)
        );
        assert_eq!(encoded[0].token_type, 1);
        assert_eq!(encoded[0].token_modifiers_bitset, 0b01);
        // `cd` is on the next line, column 2: the start is absolute again.
        assert_eq!(
            (encoded[1].delta_line, encoded[1].delta_start, encoded[1].length),
            (1, 2, 2)
        );
        assert_eq!(encoded[1].token_type, 4);
        assert_eq!(encoded[1].token_modifiers_bitset, 0b10);
    }

    #[test]
    fn encoding_sorts_and_drops_overlaps() {
        let text = "abcdef";
        let line_index = crate::documents::LineIndex::new(text);
        let variable = |start, end| Classified {
            span: Span::new(start, end),
            category: TokenCategory::Variable,
            declaration: false,
            readonly: false,
        };
        let encoded = encode(&[variable(3, 5), variable(0, 2), variable(1, 4)], text, &line_index);
        let starts: Vec<u32> = encoded.iter().map(|token| token.delta_start).collect();
        assert_eq!(starts, vec![0, 3]);
    }

    #[test]
    fn the_legend_lists_the_categories_in_order() {
        let legend = legend();
        let types: Vec<&str> = legend.token_types.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(types, vec!["parameter", "variable", "property", "class", "method"]);
        let modifiers: Vec<&str> = legend
            .token_modifiers
            .iter()
            .map(|modifier| modifier.as_str())
            .collect();
        assert_eq!(modifiers, vec!["declaration", "readonly"]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-lsp semantic_tokens`
Expected: FAIL to compile — `cannot find function encode` / `legend`.

- [ ] **Step 3: Implement classification, the legend and the encoding**

Replace the placeholder `classify` in `semantic_tokens.rs` and add the rest, keeping the types from Task 3. Update the `use` lines to:

```rust
use std::collections::HashMap;

use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
use ns_runtime::Binding;
use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::{Span, Token, TokenKind};

use crate::documents::{utf16_length, LineIndex};
```

Then:

```rust
impl TokenCategory {
    fn legend_index(self) -> u32 {
        match self {
            TokenCategory::Parameter => 0,
            TokenCategory::Variable => 1,
            TokenCategory::Property => 2,
            TokenCategory::Class => 3,
            TokenCategory::Method => 4,
        }
    }
}

const DECLARATION_BIT: u32 = 1;
const READONLY_BIT: u32 = 2;

/// The legend the server announces; `TokenCategory::legend_index` and the
/// two bits above index into it. The JetBrains plugin maps these strings.
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::PARAMETER,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::CLASS,
            SemanticTokenType::METHOD,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::READONLY,
        ],
    }
}

/// LSP's relative encoding of `classified`: sorted by position, each token's
/// line relative to the previous one's, its start relative to the previous
/// one's on the same line, both in UTF-16. A token that overlaps the one
/// before it is dropped, since the protocol has no way to express overlap.
pub fn encode(classified: &[Classified], text: &str, line_index: &LineIndex) -> Vec<SemanticToken> {
    let mut sorted = classified.to_vec();
    sorted.sort_by_key(|entry| (entry.span.start, entry.span.end));
    let mut encoded = Vec::with_capacity(sorted.len());
    let mut previous_line = 0;
    let mut previous_start = 0;
    let mut previous_end = 0;
    for entry in sorted {
        if !encoded.is_empty() && entry.span.start < previous_end {
            continue;
        }
        let position = line_index.position(text, entry.span.start);
        let delta_line = position.line - previous_line;
        let delta_start = if delta_line == 0 {
            position.character - previous_start
        } else {
            position.character
        };
        let mut modifiers = 0;
        if entry.declaration {
            modifiers |= DECLARATION_BIT;
        }
        if entry.readonly {
            modifiers |= READONLY_BIT;
        }
        encoded.push(SemanticToken {
            delta_line,
            delta_start,
            length: utf16_length(text, entry.span),
            token_type: entry.category.legend_index(),
            token_modifiers_bitset: modifiers,
        });
        previous_line = position.line;
        previous_start = position.character;
        previous_end = entry.span.end;
    }
    encoded
}

/// What one class declares, both sides together, under the full selector --
/// the same set `crates/ns-runtime/src/resolver/classes.rs`'s
/// `own_member_names` builds, but remembering what kind of member each is.
/// A name declared twice is a resolver error; the first declaration wins
/// here, so the colour is stable while the error is shown.
struct ClassMembers {
    members: HashMap<String, MemberKind>,
}

#[derive(Clone, Copy)]
enum MemberKind {
    Slot { readonly: bool },
    SlotSetter,
    NestedClass,
    Method,
}

impl ClassMembers {
    fn of(declaration: &ast::ClassDeclaration) -> ClassMembers {
        let mut members = HashMap::new();
        for body in std::iter::once(&declaration.instance_side).chain(&declaration.class_side) {
            for slot in &body.slots {
                let readonly = slot.mutability == ast::SlotMutability::Immutable;
                members
                    .entry(slot.name.name.clone())
                    .or_insert(MemberKind::Slot { readonly });
                if !readonly {
                    members
                        .entry(format!("{}:", slot.name.name))
                        .or_insert(MemberKind::SlotSetter);
                }
            }
            for method in &body.methods {
                members
                    .entry(selector_of(&method.pattern))
                    .or_insert(MemberKind::Method);
            }
            for nested in &body.nested_classes {
                members
                    .entry(nested.name.name.clone())
                    .or_insert(MemberKind::NestedClass);
            }
        }
        // §6: a declaration without a factory signature has the factory `new`.
        let factory = declaration
            .factory
            .as_ref()
            .map_or_else(|| "new".to_string(), selector_of);
        members.entry(factory).or_insert(MemberKind::Method);
        ClassMembers { members }
    }
}

/// A message pattern's full selector: `at:put:` for a keyword pattern, whose
/// parts the parser stores without their colon.
fn selector_of(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector } | ast::MessagePattern::Binary { selector, .. } => {
            selector.name.clone()
        }
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, _)| format!("{}:", keyword.name))
            .collect(),
    }
}

fn selector_name(selector: &ast::Selector) -> &str {
    match selector {
        ast::Selector::Unary(name) | ast::Selector::Binary(name) | ast::Selector::Keyword(name) => {
            name
        }
    }
}

#[derive(Clone, Copy)]
enum LocalKind {
    Parameter,
    Temporary { readonly: bool },
}

struct Classifier<'a> {
    bindings: &'a HashMap<Span, Binding>,
    tokens: &'a [Token],
    /// The local scopes entered, innermost last, each a list of names in
    /// declaration order.
    scopes: Vec<Vec<(String, LocalKind)>>,
    output: Vec<Classified>,
}

/// Every name in `module` the resolver can place; see this task's mapping.
pub fn classify(
    module: &ast::Module,
    bindings: &HashMap<Span, Binding>,
    tokens: &[Token],
) -> Vec<Classified> {
    let mut classifier = Classifier {
        bindings,
        tokens,
        scopes: Vec::new(),
        output: Vec::new(),
    };
    classifier.class(&module.root_class, &[]);
    classifier.output
}

impl Classifier<'_> {
    fn emit(&mut self, span: Span, category: TokenCategory, declaration: bool, readonly: bool) {
        self.output.push(Classified {
            span,
            category,
            declaration,
            readonly,
        });
    }

    /// `enclosing` is the chain around this declaration, innermost first --
    /// the resolver's `class_chain` without this class's own level.
    fn class(&mut self, declaration: &ast::ClassDeclaration, enclosing: &[&ClassMembers]) {
        let own = ClassMembers::of(declaration);
        let mut chain: Vec<&ClassMembers> = vec![&own];
        chain.extend_from_slice(enclosing);

        self.emit(declaration.name.span, TokenCategory::Class, true, false);
        // A module root's clauses bind nothing; a nested class's receivers
        // resolve against the chain enclosing it.
        if !enclosing.is_empty() {
            for clause in declaration
                .superclass
                .iter()
                .chain(&declaration.mixin_applications)
            {
                if clause.prefix.is_none() {
                    self.member_use(clause.receiver.span, &clause.receiver.name, enclosing);
                }
            }
        }

        // Slot initializers, initializer expressions and the clauses'
        // message arguments run in the instance initializer, where the
        // factory's parameters are locals.
        let factory_parameters: Vec<&ast::Identifier> = declaration
            .factory
            .as_ref()
            .map(pattern_parameters)
            .unwrap_or_default();
        self.enter_scope(&factory_parameters, &[]);
        for slot in &declaration.instance_side.slots {
            let readonly = slot.mutability == ast::SlotMutability::Immutable;
            self.emit(slot.name.span, TokenCategory::Property, true, readonly);
            if let Some(initializer) = &slot.initializer {
                self.expression(initializer, &chain);
            }
        }
        for statement in &declaration.instance_side.initializer_expressions {
            self.statement(statement, &chain);
        }
        for clause in declaration
            .superclass
            .iter()
            .chain(&declaration.mixin_applications)
        {
            for message in &clause.messages {
                for argument in &message.arguments {
                    self.expression(argument, &chain);
                }
            }
        }
        self.scopes.pop();

        if let Some(class_side) = &declaration.class_side {
            for slot in &class_side.slots {
                let readonly = slot.mutability == ast::SlotMutability::Immutable;
                self.emit(slot.name.span, TokenCategory::Property, true, readonly);
            }
        }
        for body in std::iter::once(&declaration.instance_side).chain(&declaration.class_side) {
            for method in &body.methods {
                self.method(method, &chain);
            }
        }
        for body in std::iter::once(&declaration.instance_side).chain(&declaration.class_side) {
            for nested in &body.nested_classes {
                self.class(nested, &chain);
            }
        }
    }

    fn method(&mut self, method: &ast::MethodDeclaration, chain: &[&ClassMembers]) {
        match &method.pattern {
            ast::MessagePattern::Unary { selector } | ast::MessagePattern::Binary { selector, .. } => {
                self.emit(selector.span, TokenCategory::Method, true, false);
            }
            ast::MessagePattern::Keyword { parts } => {
                for (keyword, _) in parts {
                    self.emit(keyword.span, TokenCategory::Method, true, false);
                }
            }
        }
        let parameters = pattern_parameters(&method.pattern);
        self.body(&parameters, &method.body, chain);
    }

    fn body(&mut self, parameters: &[&ast::Identifier], body: &ast::CodeBody, chain: &[&ClassMembers]) {
        self.enter_scope(parameters, &body.temporaries);
        for temporary in &body.temporaries {
            if let Some(initializer) = &temporary.initializer {
                self.expression(initializer, chain);
            }
        }
        for statement in &body.statements {
            self.statement(statement, chain);
        }
        self.scopes.pop();
    }

    /// Pushes a scope and emits each parameter's and temporary's declaration.
    fn enter_scope(&mut self, parameters: &[&ast::Identifier], temporaries: &[ast::SlotDeclaration]) {
        let mut scope = Vec::new();
        for parameter in parameters {
            self.emit(parameter.span, TokenCategory::Parameter, true, false);
            scope.push((parameter.name.clone(), LocalKind::Parameter));
        }
        for temporary in temporaries {
            let readonly = temporary.mutability == ast::SlotMutability::Immutable;
            self.emit(temporary.name.span, TokenCategory::Variable, true, readonly);
            scope.push((temporary.name.name.clone(), LocalKind::Temporary { readonly }));
        }
        self.scopes.push(scope);
    }

    fn statement(&mut self, statement: &Statement, chain: &[&ClassMembers]) {
        match statement {
            Statement::Expression(expression) | Statement::Return(expression) => {
                self.expression(expression, chain)
            }
        }
    }

    fn expression(&mut self, expression: &Expression, chain: &[&ClassMembers]) {
        match expression {
            Expression::Identifier(identifier) => self.identifier(identifier, chain),
            Expression::Literal(_) => {}
            Expression::Send(send) => self.send(send, chain),
            Expression::Cascade(cascade) => {
                self.expression(&cascade.receiver, chain);
                for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                    for argument in &message.arguments {
                        self.expression(argument, chain);
                    }
                }
            }
            Expression::Assign { target, value, .. } => {
                self.setter_target(target, chain);
                self.expression(value, chain);
            }
            Expression::Block(block) => {
                let parameters: Vec<&ast::Identifier> = block.parameters.iter().collect();
                self.body(&parameters, &block.body, chain);
            }
            Expression::DynamicArray { elements, .. } => {
                for element in elements {
                    self.expression(element, chain);
                }
            }
        }
    }

    fn identifier(&mut self, identifier: &ast::Identifier, chain: &[&ClassMembers]) {
        match self.bindings.get(&identifier.span) {
            Some(Binding::Local(_) | Binding::OuterLocal { .. }) => {
                let (category, readonly) = self.local(&identifier.name);
                self.emit(identifier.span, category, false, readonly);
            }
            Some(Binding::LexicalSend { .. }) => {
                self.member_use(identifier.span, &identifier.name, chain);
            }
            _ => {}
        }
    }

    fn setter_target(&mut self, target: &ast::Identifier, chain: &[&ClassMembers]) {
        match self.bindings.get(&target.span) {
            Some(Binding::Local(_) | Binding::OuterLocal { .. }) => {
                let (category, readonly) = self.local(&target.name);
                self.emit(target.span, category, false, readonly);
            }
            Some(Binding::LexicalSend { .. }) => {
                let setter = format!("{}:", target.name);
                self.member_use(target.span, &setter, chain);
            }
            _ => {}
        }
    }

    fn send(&mut self, send: &ast::Send, chain: &[&ClassMembers]) {
        match self.bindings.get(&send.span).copied() {
            Some(Binding::LexicalSend { depth }) => {
                let (category, readonly) =
                    member_category(chain, depth, selector_name(&send.message.selector));
                self.selector(send, category, readonly);
            }
            Some(Binding::SuperSend) => self.selector(send, TokenCategory::Method, false),
            _ => {}
        }
        // Visiting a `self`, `super` or `outer N` receiver emits nothing: the
        // resolver records no binding inside one.
        self.expression(&send.receiver, chain);
        for argument in &send.message.arguments {
            self.expression(argument, chain);
        }
    }

    /// Emits the selector tokens of `send`: inside the message's span, but
    /// outside the receiver's and every argument's. A keyword message's span
    /// starts at its receiver, so the receiver has to be excluded explicitly.
    fn selector(&mut self, send: &ast::Send, category: TokenCategory, readonly: bool) {
        let excluded: Vec<Span> = std::iter::once(expression_span(&send.receiver))
            .chain(send.message.arguments.iter().map(expression_span))
            .collect();
        let message = send.message.span;
        let first = self
            .tokens
            .partition_point(|token| token.span.start < message.start);
        let selector_spans: Vec<Span> = self.tokens[first..]
            .iter()
            .take_while(|token| token.span.end <= message.end)
            .filter(|token| {
                matches!(
                    token.kind,
                    TokenKind::Identifier
                        | TokenKind::Keyword
                        | TokenKind::SetterKeyword
                        | TokenKind::BinarySelector
                )
            })
            .filter(|token| !excluded.iter().any(|span| contains(*span, token.span)))
            .map(|token| token.span)
            .collect();
        for span in selector_spans {
            self.emit(span, category, false, readonly);
        }
    }

    fn member_use(&mut self, span: Span, name: &str, chain: &[&ClassMembers]) {
        let depth = match self.bindings.get(&span) {
            Some(Binding::LexicalSend { depth }) => *depth,
            _ => return,
        };
        let (category, readonly) = member_category(chain, depth, name);
        self.emit(span, category, false, readonly);
    }

    /// The innermost local of that name in the scopes entered. The resolver
    /// has already decided the name is a local; this only asks which kind.
    fn local(&self, name: &str) -> (TokenCategory, bool) {
        for scope in self.scopes.iter().rev() {
            if let Some((_, kind)) = scope.iter().rev().find(|(local, _)| local == name) {
                return match kind {
                    LocalKind::Parameter => (TokenCategory::Parameter, false),
                    LocalKind::Temporary { readonly } => (TokenCategory::Variable, *readonly),
                };
            }
        }
        (TokenCategory::Variable, false)
    }
}

/// What `name` is in the class `depth` levels out, or a send when that class
/// does not declare it.
fn member_category(chain: &[&ClassMembers], depth: u16, name: &str) -> (TokenCategory, bool) {
    match chain
        .get(depth as usize)
        .and_then(|members| members.members.get(name))
    {
        Some(MemberKind::Slot { readonly }) => (TokenCategory::Property, *readonly),
        Some(MemberKind::SlotSetter) => (TokenCategory::Property, false),
        Some(MemberKind::NestedClass) => (TokenCategory::Class, false),
        Some(MemberKind::Method) | None => (TokenCategory::Method, false),
    }
}

fn pattern_parameters(pattern: &ast::MessagePattern) -> Vec<&ast::Identifier> {
    match pattern {
        ast::MessagePattern::Unary { .. } => Vec::new(),
        ast::MessagePattern::Binary { parameter, .. } => vec![parameter],
        ast::MessagePattern::Keyword { parts } => parts.iter().map(|(_, parameter)| parameter).collect(),
    }
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end && outer.start < outer.end
}

fn expression_span(expression: &Expression) -> Span {
    match expression {
        Expression::Identifier(identifier) => identifier.span,
        Expression::Literal(literal) => literal_span(literal),
        Expression::Send(send) => send.span,
        Expression::Cascade(cascade) => cascade.span,
        Expression::Assign { span, .. } | Expression::DynamicArray { span, .. } => *span,
        Expression::Block(block) => block.span,
    }
}

fn literal_span(literal: &ast::Literal) -> Span {
    match literal {
        ast::Literal::Integer { span, .. }
        | ast::Literal::Float { span, .. }
        | ast::Literal::String { span, .. }
        | ast::Literal::Symbol { span, .. }
        | ast::Literal::Boolean { span, .. }
        | ast::Literal::Nil { span }
        | ast::Literal::Array { span, .. }
        | ast::Literal::Pattern { span, .. } => *span,
    }
}
```

Notes for the implementer:
- `contains` refuses a zero-width outer span: the parser synthesizes a zero-width `self` receiver for an implicit keyword send (`docs/design/dispatch.md`, "Shapes the resolver must recognize"), and a zero-width span must exclude nothing.
- If `ast::Expression` or `ast::Literal` has gained a variant since this plan was written, the matches above stop compiling: add the variant with its span.
- If a test in Step 1 fails, first check the expectation against the mapping at the top of this task and the resolver's code; a wrong expectation is fixed only when the resolver demonstrably records something else for a reason the spec gives. Report either to the controller.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ns-lsp`
Expected: PASS — the 11 tests of this task and everything before.

- [ ] **Step 5: Run the per-commit gate and commit**

Run: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (no warning), `cargo test -p ns-lsp`.

```bash
git add crates/ns-lsp
git commit -m "feat: semantic tokens from the resolver's bindings"
```

---

### Task 5: `dump-tokens`, for the plugin's lexer comparison

**Files:**
- Create: `crates/ns-lsp/src/token_dump.rs`
- Modify: `crates/ns-lsp/src/lib.rs`
- Modify: `crates/ns-lsp/src/main.rs`

**Interfaces:**
- Produces: `pub fn dump(text: &str) -> serde_json::Value` and the command line `ns-next-lsp dump-tokens <file>…`, which prints one JSON object per file, one per line:
  ```json
  {"file":"<path as given>","tokens":[{"kind":"Identifier","start":0,"end":3}],"diagnostics":0}
  ```
  `kind` is the `ns_syntax::TokenKind` variant name; `start` and `end` are **UTF-16** offsets from the start of the file, so the plugin, whose documents are UTF-16, compares them directly; `EndOfInput` is omitted; `diagnostics` counts the lexer's diagnostics. Exit status 0 when every file was read, 1 otherwise (after printing an `error:` line on stderr for each file that could not be read). Part 2's comparison test consumes exactly this format.

- [ ] **Step 1: Write the failing test**

Create `crates/ns-lsp/src/token_dump.rs`:

```rust
//! The lexer's tokens as JSON, for the JetBrains plugin's comparison of its
//! own lexer against this one (`docs/design/editor-support.md`, "Testing").

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_dumped_with_utf16_offsets() {
        let dumped = dump("'ä' foo:");
        assert_eq!(
            dumped,
            serde_json::json!({
                "tokens": [
                    {"kind": "String", "start": 0, "end": 3},
                    {"kind": "Keyword", "start": 4, "end": 8},
                ],
                "diagnostics": 0,
            })
        );
    }

    #[test]
    fn lexer_diagnostics_are_counted() {
        assert_eq!(dump("'open")["diagnostics"], 1);
    }
}
```

Add `pub mod token_dump;` to `lib.rs`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-lsp token_dump`
Expected: FAIL to compile — `cannot find function dump`.

- [ ] **Step 3: Implement `dump`**

Above the tests:

```rust
use ns_syntax::TokenKind;

/// `text`'s tokens with UTF-16 offsets, `EndOfInput` left out, and the
/// number of lexer diagnostics. The caller adds the file name.
pub fn dump(text: &str) -> serde_json::Value {
    let (tokens, diagnostics) = ns_syntax::lexer::Lexer::new(text).run();
    // UTF-16 offsets of every byte offset a token starts or ends at, by one
    // pass over the text rather than one per token.
    let mut utf16_at = vec![0u32; text.len() + 1];
    let mut units = 0u32;
    for (byte_offset, character) in text.char_indices() {
        for offset in byte_offset..byte_offset + character.len_utf8() {
            utf16_at[offset] = units;
        }
        units += character.len_utf16() as u32;
    }
    utf16_at[text.len()] = units;
    let entries: Vec<serde_json::Value> = tokens
        .iter()
        .filter(|token| token.kind != TokenKind::EndOfInput)
        .map(|token| {
            serde_json::json!({
                "kind": format!("{:?}", token.kind),
                "start": utf16_at[token.span.start as usize],
                "end": utf16_at[token.span.end as usize],
            })
        })
        .collect();
    serde_json::json!({
        "tokens": entries,
        "diagnostics": diagnostics.len(),
    })
}
```

`{:?}` on `TokenKind` is its variant name; the rule against `{:?}` concerns `Value` and `EvaluationResult`, not this.

- [ ] **Step 4: Wire the command line**

Replace `crates/ns-lsp/src/main.rs` with:

```rust
use std::process::ExitCode;

/// The stack the server runs on. It parses and resolves, which recurse; the
/// parser's nesting ceiling bounds the depth, and `ns-cli` runs the same
/// front end on 8 MiB for the same reason (`crates/ns-cli/src/main.rs`,
/// `INTERPRETER_STACK_BYTES`). The thread is needed regardless: the symbol
/// interner is thread-local, so everything that interns must run on one.
const SERVER_STACK_BYTES: usize = 8 * 1024 * 1024;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let worker = std::thread::Builder::new()
        .name("ns-next-lsp".to_string())
        .stack_size(SERVER_STACK_BYTES)
        .spawn(move || execute(&arguments))
        .expect("spawn the server thread");
    worker.join().unwrap_or_else(|_| ExitCode::from(101))
}

fn execute(arguments: &[String]) -> ExitCode {
    match arguments.split_first() {
        Some((command, files)) if command == "dump-tokens" => dump_tokens(files),
        _ => {
            eprintln!("usage: ns-next-lsp                        (speak LSP on stdin and stdout)");
            eprintln!("       ns-next-lsp dump-tokens <file>...  (print each file's tokens as JSON)");
            ExitCode::from(2)
        }
    }
}

fn dump_tokens(files: &[String]) -> ExitCode {
    let mut status = ExitCode::SUCCESS;
    for file in files {
        match std::fs::read_to_string(file) {
            Ok(text) => {
                let mut dumped = ns_lsp::token_dump::dump(&text);
                dumped["file"] = serde_json::Value::String(file.clone());
                println!("{dumped}");
            }
            Err(error) => {
                eprintln!("error: reading {file}: {error}");
                status = ExitCode::FAILURE;
            }
        }
    }
    status
}
```

Task 6 adds the no-argument case that runs the server.

- [ ] **Step 5: Run the tests and try the command**

Run: `cargo test -p ns-lsp token_dump`
Expected: PASS, 2 tests.

Run: `cargo run -p ns-lsp -- dump-tokens goldens/language/$(ls goldens/language | grep '\.ns$' | head -1)`
Expected: one line of JSON starting `{"diagnostics":0,"file":` (serde_json sorts keys) with a `tokens` array.

- [ ] **Step 6: Run the per-commit gate and commit**

Run: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (no warning), `cargo test -p ns-lsp`.

```bash
git add crates/ns-lsp
git commit -m "feat: ns-next-lsp dump-tokens for the plugin's lexer comparison"
```

---

### Task 6: The server loop

**Files:**
- Create: `crates/ns-lsp/src/server.rs`
- Modify: `crates/ns-lsp/src/lib.rs`
- Modify: `crates/ns-lsp/src/main.rs`
- Create: `crates/ns-lsp/tests/protocol.rs`

**Interfaces:**
- Consumes: `analyze` (Task 3), `legend`, `encode` (Task 4), `LineIndex` (Task 2).
- Produces: `pub fn run(connection: lsp_server::Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>>` — initializes, serves until `shutdown` and `exit`, and returns.

Protocol surface (spec, "Protocol surface"): `initialize` answering capabilities `textDocumentSync: Full` and a semantic-tokens provider (`full: true`, no range, the legend from Task 4); `textDocument/didOpen`, `didChange` (the last content change is the full text), `didClose` (clears that document's diagnostics); `textDocument/publishDiagnostics` after every open and change; `textDocument/semanticTokens/full`; `shutdown` and `exit`. Any other request is answered with `MethodNotFound`; any other notification is ignored.

- [ ] **Step 1: Write the failing end-to-end test**

Create `crates/ns-lsp/tests/protocol.rs`:

```rust
//! The server end to end over an in-memory connection, as an editor drives it.

use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(10);

fn receive(client: &Connection) -> Message {
    client
        .receiver
        .recv_timeout(TIMEOUT)
        .expect("the server answers within the timeout")
}

fn request(client: &Connection, id: i32, method: &str, params: Value) -> Value {
    client
        .sender
        .send(Message::Request(Request::new(RequestId::from(id), method.to_string(), params)))
        .expect("send a request");
    loop {
        if let Message::Response(response) = receive(client) {
            assert_eq!(response.id, RequestId::from(id));
            assert!(response.error.is_none(), "{method}: {:?}", response.error);
            return response.result.unwrap_or(Value::Null);
        }
    }
}

fn notify(client: &Connection, method: &str, params: Value) {
    client
        .sender
        .send(Message::Notification(Notification::new(method.to_string(), params)))
        .expect("send a notification");
}

fn next_diagnostics(client: &Connection) -> Value {
    loop {
        if let Message::Notification(notification) = receive(client) {
            if notification.method == "textDocument/publishDiagnostics" {
                return notification.params;
            }
        }
    }
}

const URI: &str = "file:///C:/work/Module.ns";

#[test]
fn an_editor_session() {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || ns_lsp::server::run(server).map_err(|error| error.to_string()))
        .expect("spawn the server");

    let capabilities = request(&client, 1, "initialize", json!({ "capabilities": {} }));
    assert_eq!(capabilities["capabilities"]["textDocumentSync"], 1, "full sync");
    assert_eq!(
        capabilities["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"],
        json!(["parameter", "variable", "property", "class", "method"])
    );
    notify(&client, "initialized", json!({}));

    // Non-ASCII text before the error on its own line, so a position counted
    // in bytes or in chars rather than UTF-16 units would be off.
    let broken = "Newspeak3\n'Test'\nclass Module = Object new () ( (* Grüße 😀 *) public broken = ( ^outer Missing foo ) ) : ()\n";
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "newspeak", "version": 1, "text": broken } }),
    );
    let published = next_diagnostics(&client);
    assert_eq!(published["uri"], URI);
    let diagnostics = published["diagnostics"].as_array().expect("an array");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["severity"], 1, "an error");
    assert_eq!(diagnostics[0]["source"], "ns-next");
    let line_three = broken.lines().nth(2).expect("line 3");
    let byte_column = line_three.find("outer Missing").expect("the send");
    let column = line_three[..byte_column].encode_utf16().count() as u64;
    assert_eq!(diagnostics[0]["range"]["start"], json!({ "line": 2, "character": column }));

    let fixed = "Newspeak3\n'Test'\nclass Module = Object new () ( public echo: argument = ( ^argument ) ) : ()\n";
    notify(
        &client,
        "textDocument/didChange",
        json!({ "textDocument": { "uri": URI, "version": 2 }, "contentChanges": [{ "text": fixed }] }),
    );
    let published = next_diagnostics(&client);
    assert_eq!(published["diagnostics"], json!([]));

    let tokens = request(
        &client,
        2,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": URI } }),
    );
    let data = tokens["data"].as_array().expect("token data");
    assert!(!data.is_empty(), "the fixed module has classified names");
    assert_eq!(data.len() % 5, 0, "five integers per token");

    let unknown = request(
        &client,
        3,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": "file:///never/opened.ns" } }),
    );
    assert_eq!(unknown["data"], json!([]), "an unknown document has no tokens");

    notify(&client, "textDocument/didClose", json!({ "textDocument": { "uri": URI } }));
    let published = next_diagnostics(&client);
    assert_eq!(published["diagnostics"], json!([]), "closing clears the errors");

    request(&client, 4, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    let outcome = server_thread.join().expect("the server thread does not panic");
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn an_unknown_request_is_method_not_found() {
    let (server, client) = Connection::memory();
    let server_thread = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || ns_lsp::server::run(server).map_err(|error| error.to_string()))
        .expect("spawn the server");
    request(&client, 1, "initialize", json!({ "capabilities": {} }));
    notify(&client, "initialized", json!({}));
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(2),
            "textDocument/hover".to_string(),
            json!({}),
        )))
        .expect("send");
    let Message::Response(response) = receive(&client) else {
        panic!("expected a response");
    };
    assert_eq!(
        response.error.map(|error| error.code),
        Some(lsp_server::ErrorCode::MethodNotFound as i32)
    );
    request(&client, 3, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    assert!(server_thread.join().expect("no panic").is_ok());
}
```

Add to `crates/ns-lsp/Cargo.toml` under `[dev-dependencies]`: nothing — `lsp-server` and `serde_json` are normal dependencies and visible to integration tests. Add `pub mod server;` to `lib.rs`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-lsp --test protocol`
Expected: FAIL to compile — `could not find server in ns_lsp`.

- [ ] **Step 3: Implement the server**

Create `crates/ns-lsp/src/server.rs`:

```rust
//! The LSP message loop: one thread, synchronous. Lexing, parsing and
//! resolving one file takes milliseconds, so neither a debounce nor a worker
//! pool would buy anything, and one thread is what the thread-local symbol
//! interner needs anyway.

use std::collections::HashMap;
use std::error::Error;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as NotificationKind, PublishDiagnostics,
};
use lsp_types::request::{Request as RequestKind, SemanticTokensFullRequest};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, PublishDiagnosticsParams, Range, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensServerCapabilities, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri,
};
use ns_syntax::Severity;

use crate::analysis::{analyze, Analysis};
use crate::documents::LineIndex;
use crate::semantic_tokens;

pub type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// One open document and its analysis, computed once per version.
struct Document {
    text: String,
    line_index: LineIndex,
    analysis: Analysis,
}

impl Document {
    fn new(text: String) -> Document {
        let line_index = LineIndex::new(&text);
        let analysis = analyze(&text);
        Document {
            text,
            line_index,
            analysis,
        }
    }
}

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: semantic_tokens::legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                ..SemanticTokensOptions::default()
            },
        )),
        ..ServerCapabilities::default()
    }
}

/// Initializes over `connection`, serves until the client's `shutdown` and
/// `exit`, and returns.
pub fn run(connection: Connection) -> ServerResult<()> {
    connection.initialize(serde_json::to_value(capabilities())?)?;
    let mut documents: HashMap<Uri, Document> = HashMap::new();
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                let response = answer(&documents, request);
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(notification) => {
                if let Some(uri) = apply(&mut documents, notification)? {
                    let params = diagnostics_of(&uri, documents.get(&uri));
                    connection.sender.send(Message::Notification(Notification::new(
                        PublishDiagnostics::METHOD.to_string(),
                        params,
                    )))?;
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn answer(documents: &HashMap<Uri, Document>, request: Request) -> Response {
    if request.method != SemanticTokensFullRequest::METHOD {
        return Response::new_err(
            request.id,
            ErrorCode::MethodNotFound as i32,
            format!("ns-next-lsp does not implement {}", request.method),
        );
    }
    let params: SemanticTokensParams = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(request.id, ErrorCode::InvalidParams as i32, error.to_string())
        }
    };
    // A document the client never opened -- or already closed -- has no
    // tokens; answering an empty set keeps the client's colours as they are.
    let data = documents
        .get(&params.text_document.uri)
        .map(|document| {
            semantic_tokens::encode(&document.analysis.classified, &document.text, &document.line_index)
        })
        .unwrap_or_default();
    Response::new_ok(
        request.id,
        SemanticTokens {
            result_id: None,
            data,
        },
    )
}

/// Applies a document notification; answers the URI whose diagnostics must be
/// published, or `None` for a notification that changes nothing.
fn apply(documents: &mut HashMap<Uri, Document>, notification: Notification) -> ServerResult<Option<Uri>> {
    match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = serde_json::from_value(notification.params)?;
            let uri = params.text_document.uri;
            documents.insert(uri.clone(), Document::new(params.text_document.text));
            Ok(Some(uri))
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(notification.params)?;
            let uri = params.text_document.uri;
            // Full synchronisation: the last change carries the whole text.
            let Some(change) = params.content_changes.into_iter().last() else {
                return Ok(None);
            };
            documents.insert(uri.clone(), Document::new(change.text));
            Ok(Some(uri))
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = serde_json::from_value(notification.params)?;
            documents.remove(&params.text_document.uri);
            Ok(Some(params.text_document.uri))
        }
        _ => Ok(None),
    }
}

/// The diagnostics to publish for `uri`: its findings, or none once closed.
fn diagnostics_of(uri: &Uri, document: Option<&Document>) -> PublishDiagnosticsParams {
    let diagnostics = document.map_or_else(Vec::new, |document| {
        document
            .analysis
            .findings
            .iter()
            .map(|finding| Diagnostic {
                range: Range::new(
                    document.line_index.position(&document.text, finding.span.start),
                    document.line_index.position(&document.text, finding.span.end),
                ),
                severity: Some(match finding.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                }),
                source: Some("ns-next".to_string()),
                message: finding.message.clone(),
                ..Diagnostic::default()
            })
            .collect()
    });
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    }
}
```

If `lsp-types` 0.97 spells a name differently (for example `SemanticTokensOptions` has no `Default`, or `Uri` is not `Hash`), follow the crate's API — `cargo doc -p lsp-types --open` or the source under `~/.cargo/registry/src/*/lsp-types-0.97.0/src` — and keep the behaviour described above. Do not add a dependency to work around it.

- [ ] **Step 4: Run the server from `main`**

In `crates/ns-lsp/src/main.rs`, extend `execute`'s match with the no-argument case, first:

```rust
    match arguments.split_first() {
        None => serve(),
        Some((command, files)) if command == "dump-tokens" => dump_tokens(files),
        _ => { /* the usage message, unchanged */ }
    }
```

and add:

```rust
fn serve() -> ExitCode {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    let outcome = ns_lsp::server::run(connection);
    // Joining the I/O threads flushes the last response before exiting.
    let joined = io_threads.join();
    match (outcome, joined) {
        (Ok(()), Ok(())) => ExitCode::SUCCESS,
        (Err(error), _) => {
            eprintln!("ns-next-lsp: {error}");
            ExitCode::FAILURE
        }
        (_, Err(error)) => {
            eprintln!("ns-next-lsp: {error}");
            ExitCode::FAILURE
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ns-lsp`
Expected: PASS, including both tests in `tests/protocol.rs`.

- [ ] **Step 6: Run the per-commit gate and commit**

Run: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (no warning), `cargo test -p ns-lsp`.

```bash
git add crates/ns-lsp
git commit -m "feat: ns-next-lsp serves diagnostics and semantic tokens"
```

---

### Task 7: Documentation, the gates, and the end-of-part run

**Files:**
- Modify: `CLAUDE.md` (the crate list at the top; "Working in this repository", the gates)
- Modify: `docs/design/README.md` (the map row, "Architecture", "Gates")
- Modify: `docs/design/editor-support.md` (status; the server's sections from `Planned:` to present tense)

- [ ] **Step 1: Update `CLAUDE.md`**

- In the opening paragraph change "three crates" to "four crates" and add, after the `ns-cli` bullet:

  ```markdown
  - `crates/ns-lsp` — the `ns-next-lsp` binary: a language server for `.ns` files
    (lexer, parser and resolver errors, and semantic tokens from the resolver's
    bindings) and `dump-tokens`, which the JetBrains plugin's lexer is checked
    against. It never starts the VM. Its dependencies (`lsp-server`, `lsp-types`,
    `serde_json`) stay in this crate. See `docs/design/editor-support.md`.
  ```

- In the per-commit gate block, change `cargo test --workspace` to:

  ```bash
  # the full suite, including every golden under both lowerings; the editor's
  # language server has its own group below
  cargo test --workspace --exclude ns-lsp
  ```

  and add, after the per-commit block's `parse-corpus` line:

  ```bash
  # --- before every commit that touches crates/ns-lsp or crates/ns-syntax ---
  cargo test -p ns-lsp
  ```

- In the end-of-batch block, add `--exclude ns-lsp` to each of the three `NS_NEXT_STRESS_*=1 cargo test --workspace` lines (the server never starts the VM), and add `cargo test -p ns-lsp` to that block.

- Below the blocks, add one paragraph:

  ```markdown
  `cargo clippy --workspace --all-targets` still compiles `ns-lsp` on every commit,
  so a change to the resolver or to `ns-syntax` that breaks the API the server uses
  fails at the commit even when its tests only run at the end of the batch.
  ```

- [ ] **Step 2: Update `docs/design/README.md`**

- The map row for editor support: status `**Partial:** the language server (\`crates/ns-lsp\`) is built; the JetBrains plugin is planned`.
- "Architecture": "Three crates" becomes "Four crates", and the tree gains `ns-lsp/       the \`ns-next-lsp\` binary: the editor's language server`.
- "Gates": mirror the `CLAUDE.md` change — `cargo test --workspace --exclude ns-lsp`, the stress lines with `--exclude ns-lsp`, and a `cargo test -p ns-lsp` line.

- [ ] **Step 3: Update `docs/design/editor-support.md`**

- Status block: `**Partial:** the language server is built (\`crates/ns-lsp/\`, and \`resolve_for_editor\` in \`crates/ns-runtime/src/resolver/mod.rs\`); the JetBrains plugin (\`editors/jetbrains/\`) is planned.`
- Retitle "Planned: the language server, `crates/ns-lsp`" to "The language server, `crates/ns-lsp`" and put it in the present tense, describing what exists. Record what implementation settled:
  - the entry is `ns_runtime::resolve_for_editor`, answering `EditorResolution`;
  - `Kernel.ns` resolves without errors through it (pinned by `the_editor_entry_resolves_the_kernel_without_the_mirror_mark`);
  - the legend has no `keyword` type — the reserved words (§4.1: `self`, `super`, `outer`, `true`, `false`, `nil`) are coloured by the plugin's lexer, so the server adds nothing for them;
  - the classification tests assert each expected category directly rather than through `insta` snapshots, so each expectation is stated and reviewed rather than accepted;
  - the robustness test cuts every golden and the kernel at every token boundary, and each corpus module at 64 evenly spaced token boundaries, because every prefix of every corpus module is quadratic in its size;
  - `dump-tokens` prints UTF-16 offsets, the unit the plugin's documents use.
- In "Planned: testing and gates", move the Rust half into a present-tense "Testing" paragraph under the server's section; the Kotlin half stays planned.

- [ ] **Step 4: Run the end-of-part gates**

This is the end of a batch (`CLAUDE.md`, "The gates are staggered"). Run each in the foreground, in Git Bash, and record the output of each:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace --exclude ns-lsp
cargo test -p ns-lsp
cargo run -p ns-cli -- parse-corpus corpus/modules      # parsed 210, failed 0, expected-fail 1, stale 0
NS_NEXT_STRESS_MOVES=1 cargo test --workspace --exclude ns-lsp
NS_NEXT_STRESS_CACHES=1 cargo test --workspace --exclude ns-lsp
NS_NEXT_STRESS_COLLECT=1 cargo test --workspace --exclude ns-lsp
cargo run -p ns-cli -- run-corpus corpus/modules        # ran 6, failed 0, skipped 205
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
cargo build --workspace
cargo build -p ns-lsp --release
```

Expected: every test passes, clippy is silent, both corpus reports match the numbers above exactly, and `target/release/ns-next-lsp.exe` exists. If a report differs, stop and report it; do not update an expected number.

- [ ] **Step 5: Commit**

This commit changes only Markdown, so the suite need not run again for it.

```bash
git add CLAUDE.md docs/design/README.md docs/design/editor-support.md
git commit -m "docs: the language server in the design documents and the gates"
```
