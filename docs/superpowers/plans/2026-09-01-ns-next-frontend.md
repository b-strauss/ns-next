# ns-next Frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Status: Complete.** Verified against the codebase and git history on
> 2026-09-04 — all 14 tasks (75 steps) implemented and tested:
> `cargo test --workspace`, `cargo fmt --all -- --check`, and `cargo clippy
> --workspace --all-targets` are all clean, and every file/module this plan
> specifies exists in `crates/ns-syntax` and `crates/ns-cli`. Kept for
> historical reference; do not use as a checklist for new work.

**Goal:** Build `ns-syntax` (lexer + parser + AST for the official `Newspeak3` textual syntax) and a `ns-next` CLI that can parse single files and the whole official Newspeak repo as a corpus.

**Architecture:** A Cargo workspace. `ns-syntax` is a dependency-light library: a hand-written single-pass lexer produces `Token`s with byte-offset `Span`s, and a hand-written recursive-descent + Pratt parser produces a `Module` AST. Both collect `Diagnostic`s instead of panicking on bad input. `ns-cli` is a thin binary wrapping the library. The parser is validated against a vendored snapshot of every `.ns` file in `newspeaklanguage/newspeak`.

**Tech Stack:** Rust 1.98 (edition 2021), `clap` (CLI arg parsing), `insta` (AST snapshot tests), `toml` (reading the corpus blacklist). No parser generator.

**Spec:** `docs/superpowers/specs/2026-09-01-ns-next-runtime-design.md` — read it before starting. This plan implements only the "ns-syntax" and the `parse` / `parse-corpus` parts of the "ns-cli" sections. The resolver, object model, interpreter, core library, and `ns-next run` are a separate plan.

## Global Constraints

- **Rust toolchain:** 1.98.0, pinned via `rust-toolchain.toml`. `cargo` lives at `C:\Users\Benjamin\.cargo\bin\cargo.exe` and is **not on PATH** in every shell — either add `%USERPROFILE%\.cargo\bin` to PATH for the session or invoke cargo by full path.
- **Edition:** 2021 for every crate.
- **All code, identifiers, comments, and doc comments in English.**
- **Lints:** `#![warn(clippy::all)]` at each crate root; `cargo clippy --all-targets` must be warning-free before every commit.
- **Formatting:** `cargo fmt --all` before every commit; CI-style check is `cargo fmt --all -- --check`.
- **No `unwrap()` / `expect()` / `panic!` on parser input paths.** Malformed source produces `Diagnostic`s. `unwrap`/`expect` are allowed only for genuine internal invariants and in tests.
- **Terminology:** use classic terms — `blacklist` (not "allowlist"/"denylist"), `whitelist`, `sane`. The corpus exceptions file is `corpus/blacklist.toml`.
- **Spans are byte offsets** (`u32`) into the original source string, never char indices.
- **Grammar source of truth:** `corpus/reference/NewspeakGrammar.ns` (vendored in Task 1). When this plan and the grammar disagree, the grammar wins — note the deviation in a code comment.
- **Commits:** conventional-commit style (`feat:`, `test:`, `chore:`, `docs:`). Commit at the end of every task; small intermediate commits within a task are fine. Do not add `Co-Authored-By` unless the repo already uses it (it does — keep the trailer `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`). Never override git author/committer identity with `-c`.

---

## File Structure

```
ns-next/
  Cargo.toml                        # [workspace] members
  rust-toolchain.toml               # pins 1.98.0
  crates/
    ns-syntax/
      Cargo.toml
      src/
        lib.rs                      # public API: lex(), parse(), re-exports
        span.rs                     # Span
        diagnostic.rs               # Diagnostic, Severity
        token.rs                    # Token, TokenKind
        lexer.rs                    # Lexer
        ast.rs                      # AST node types
        parser/
          mod.rs                    # Parser struct, cursor, entry points
          expr.rs                   # expression / message / block / literal parsing
          decl.rs                   # class / method / slot parsing
      tests/
        corpus.rs                   # parses every corpus .ns file
        snapshots/                  # insta snapshots (committed)
    ns-cli/
      Cargo.toml
      src/
        main.rs                     # clap CLI: `parse`, `parse-corpus`
  corpus/
    PROVENANCE.md                   # upstream URL + commit hash + refresh procedure
    blacklist.toml                  # known-unparseable files, each with a reason
    reference/
      NewspeakGrammar.ns            # vendored grammar (not counted as a corpus test input)
    modules/
      *.ns                          # every .ns file from the upstream repo
  docs/superpowers/{specs,plans}/
```

Responsibilities:

- **`span.rs`** — a 2-field `Span` value type plus merge/contains helpers. Nothing else depends on lexer internals.
- **`diagnostic.rs`** — `Diagnostic { span, message, severity }`, `Severity`. Rendered by the CLI; produced by lexer and parser.
- **`token.rs`** — the token vocabulary. Pure data.
- **`lexer.rs`** — source `&str` → `Vec<Token>` + diagnostics. One responsibility: lexical analysis.
- **`ast.rs`** — the tree the parser builds and the resolver later consumes. Pure data; every node has a `Span`.
- **`parser/`** — tokens → `Module`. Split by grammar area (expressions vs. declarations) because the file would otherwise be large and the two areas change independently.
- **`ns-cli/src/main.rs`** — argument parsing and human-readable diagnostic rendering only; no parsing logic of its own.

---

## Task 1: Workspace scaffold and vendored corpus

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`
- Create: `crates/ns-syntax/Cargo.toml`, `crates/ns-syntax/src/lib.rs`
- Create: `crates/ns-cli/Cargo.toml`, `crates/ns-cli/src/main.rs`
- Create: `corpus/PROVENANCE.md`, `corpus/blacklist.toml`
- Create: `corpus/reference/NewspeakGrammar.ns`, `corpus/modules/*.ns`

**Interfaces:**
- Consumes: nothing.
- Produces: a building workspace with two crates; `ns_syntax` exposes an empty `lib.rs` (only `#![warn(clippy::all)]` for now); `corpus/modules/` populated.

- [x] **Step 1: Create the workspace manifest**

`Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/ns-syntax", "crates/ns-cli"]

[workspace.package]
edition = "2021"
rust-version = "1.98"
license = "MIT"

[workspace.dependencies]
clap = { version = "4", features = ["derive"] }
insta = "1"
toml = "0.8"
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.98.0"
components = ["rustfmt", "clippy"]
```

- [x] **Step 2: Create the `ns-syntax` crate skeleton**

`crates/ns-syntax/Cargo.toml`:

```toml
[package]
name = "ns-syntax"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dev-dependencies]
insta.workspace = true
toml.workspace = true
```

`crates/ns-syntax/src/lib.rs`:

```rust
#![warn(clippy::all)]

//! Lexer, AST, and parser for the Newspeak3 textual syntax.
```

- [x] **Step 3: Create the `ns-cli` crate skeleton**

`crates/ns-cli/Cargo.toml`:

```toml
[package]
name = "ns-cli"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[[bin]]
name = "ns-next"
path = "src/main.rs"

[dependencies]
ns-syntax = { path = "../ns-syntax" }
clap.workspace = true
```

`crates/ns-cli/src/main.rs`:

```rust
#![warn(clippy::all)]

fn main() {
    println!("ns-next");
}
```

- [x] **Step 4: Vendor the corpus**

Clone the upstream repo to a temp location, record the exact commit, and copy files:

```bash
git clone --depth 1 https://github.com/newspeaklanguage/newspeak.git /tmp/ns-upstream
cd /tmp/ns-upstream && git rev-parse HEAD   # record this hash
```

- Copy every `*.ns` file found anywhere in the upstream tree into `corpus/modules/`, flattening the directory structure. If two files share a base name, prefix the second with its parent directory name and note it in `PROVENANCE.md`.
- Copy `NewspeakGrammar.ns` additionally to `corpus/reference/NewspeakGrammar.ns`.
- Copy the upstream `LICENSE` to `corpus/UPSTREAM-LICENSE`.

`corpus/PROVENANCE.md`:

```markdown
# Corpus Provenance

Files under `corpus/modules/` and `corpus/reference/` are copied verbatim from:

- Repository: https://github.com/newspeaklanguage/newspeak
- Commit: <FULL_HASH_RECORDED_IN_STEP_4>
- Retrieved: 2026-09-01

`corpus/modules/` is a flattened copy of every `*.ns` file in that tree, used
only as a parser test corpus. `corpus/reference/NewspeakGrammar.ns` is the
grammar this parser follows.

## Refresh procedure

1. Clone upstream at the new commit.
2. Replace `corpus/modules/*.ns` and `corpus/reference/NewspeakGrammar.ns`.
3. Update the commit hash and date above.
4. Run `cargo test -p ns-syntax --test corpus` and reconcile `corpus/blacklist.toml`.

## Renamed files

<list any base-name collisions and how they were renamed, or "none">
```

`corpus/blacklist.toml`:

```toml
# Corpus files that are expected to fail parsing, with a reason.
# Format:
#   [[entry]]
#   file = "SomeModule.ns"
#   reason = "why this file cannot yet be parsed"
#
# The corpus test fails if a file here parses successfully (stale entry)
# or if a file NOT here fails to parse (regression).

entry = []
```

- [x] **Step 5: Verify the workspace builds**

Run: `cargo build --workspace`
Expected: compiles, produces `ns-next` binary.

Run: `cargo test --workspace`
Expected: passes, 0 tests.

Run: `cargo fmt --all -- --check` and `cargo clippy --all-targets`
Expected: both clean.

- [x] **Step 6: Commit**

```bash
git add -A
git commit -m "chore: scaffold workspace and vendor Newspeak corpus

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 2: Span and Diagnostic

**Files:**
- Create: `crates/ns-syntax/src/span.rs`
- Create: `crates/ns-syntax/src/diagnostic.rs`
- Modify: `crates/ns-syntax/src/lib.rs`

**Interfaces:**
- Produces:
  - `Span { start: u32, end: u32 }`, `Copy`, with `Span::new(start: u32, end: u32) -> Span`, `Span::merge(self, other: Span) -> Span`, `Span::contains(self, offset: u32) -> bool`, `Span::text(self, src: &str) -> &str`.
  - `Severity` enum: `Error`, `Warning`.
  - `Diagnostic { span: Span, message: String, severity: Severity }` with `Diagnostic::error(span: Span, message: impl Into<String>) -> Diagnostic`.

- [x] **Step 1: Write the failing test**

`crates/ns-syntax/src/span.rs`:

```rust
//! Byte-offset source spans.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_covers_both() {
        let a = Span::new(2, 5);
        let b = Span::new(10, 12);
        assert_eq!(a.merge(b), Span::new(2, 12));
    }

    #[test]
    fn text_slices_source() {
        let src = "hello world";
        assert_eq!(Span::new(6, 11).text(src), "world");
    }

    #[test]
    fn contains_is_half_open() {
        let s = Span::new(3, 6);
        assert!(s.contains(3));
        assert!(s.contains(5));
        assert!(!s.contains(6));
    }
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p ns-syntax span::`
Expected: FAIL — `Span` not found.

- [x] **Step 3: Write minimal implementation**

Prepend to `crates/ns-syntax/src/span.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Span {
        debug_assert!(start <= end, "span start after end");
        Span { start, end }
    }

    pub fn merge(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }

    pub fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    pub fn text(self, src: &str) -> &str {
        &src[self.start as usize..self.end as usize]
    }
}
```

`crates/ns-syntax/src/diagnostic.rs`:

```rust
//! Non-fatal lexer/parser errors.

use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic { span, message: message.into(), severity: Severity::Error }
    }
}
```

Update `crates/ns-syntax/src/lib.rs`:

```rust
#![warn(clippy::all)]

//! Lexer, AST, and parser for the Newspeak3 textual syntax.

pub mod diagnostic;
pub mod span;

pub use diagnostic::{Diagnostic, Severity};
pub use span::Span;
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test -p ns-syntax span::`
Expected: PASS (3 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): add Span and Diagnostic types

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 3: Lexer — identifiers, keywords, binary selectors, reserved words, punctuation

**Files:**
- Create: `crates/ns-syntax/src/token.rs`
- Create: `crates/ns-syntax/src/lexer.rs`
- Modify: `crates/ns-syntax/src/lib.rs`

**Interfaces:**
- Consumes: `Span`, `Diagnostic`.
- Produces:
  - `TokenKind` enum (variants grow over Tasks 3–5). This task adds:
    `Identifier`, `Keyword` (`foo:`), `SetterKeyword` (`foo::`), `BinarySelector`, `Bang` (`!`) — *note:* `!` is a `specialCharacter`, so it is only ever part of a `BinarySelector`; do not add a separate token — , `Class` (the `class` reserved word), `Private`, `Public`, `Protected`, `LParen`, `RParen`, `LBracket`, `RBracket`, `LBrace`, `RBrace`, `Bar` (`|`), `DoubleBar` (`||`), `Caret` (`^`), `Dot`, `Semicolon`, `Colon`, `Assign` (`:=`), `Equals` (`=`), `ColonColonEquals` (`::=`), `Pound` (`#`), `LessThan` (`<`), `GreaterThan` (`>`).
  - `Token { kind: TokenKind, span: Span }`.
  - `Lexer::new(src: &str) -> Lexer<'_>` and `Lexer::run(self) -> (Vec<Token>, Vec<Diagnostic>)` (full behavior completed in Task 5; this task may expose a crate-private helper `next_token`).

**Grammar notes (from `corpus/reference/NewspeakGrammar.ns`):**
- `id = (letter | '_') , (letter | digit | '_')*` — ASCII letters, digits, underscore. A leading `_` is legal.
- `kw = id ':'` ; `setterKw = id '::'` (an identifier immediately followed by `::`). `::=` is a distinct token (mutable-slot init) and must be matched before `setterKeyword` / `Assign`.
- `binSel = specialCharacter | ('-' , (specialCharacter | '-')*)` where `specialCharacter` is one of `+ - * / ~ < > = & | @ % , ? !` (and `-`). Binary selectors are maximal runs of these. **But** `|` alone is `Bar`, `||` is `DoubleBar`, `=` alone is `Equals`, `<` / `>` alone are `LessThan` / `GreaterThan`, and a leading `::=` / `:=` are matched first. Resolve by: try the fixed punctuation tokens first at each position, then fall back to a binary-selector run.
- Reserved words are matched from an `Identifier` lexeme: `class`, `private`, `public`, `protected`.

- [x] **Step 1: Write the failing test**

`crates/ns-syntax/src/lexer.rs`:

```rust
//! Single-pass lexer for Newspeak3 source.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenKind::*;

    fn kinds(src: &str) -> Vec<crate::token::TokenKind> {
        let (tokens, diags) = Lexer::new(src).run();
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn identifiers_and_keywords() {
        assert_eq!(
            kinds("foo bar: baz"),
            vec![Identifier, Keyword, Identifier, Eoi]
        );
    }

    #[test]
    fn leading_underscore_identifier() {
        assert_eq!(kinds("_x"), vec![Identifier, Eoi]);
    }

    #[test]
    fn reserved_words() {
        assert_eq!(kinds("class public private"), vec![Class, Public, Private, Eoi]);
    }

    #[test]
    fn punctuation_vs_binary_selector() {
        assert_eq!(kinds("| || = := ::= ^ ."), vec![
            Bar, DoubleBar, Equals, Assign, ColonColonEquals, Caret, Dot, Eoi,
        ]);
    }

    #[test]
    fn binary_selectors() {
        assert_eq!(kinds("a + b <= c"), vec![
            Identifier, BinarySelector, Identifier, BinarySelector, Identifier, Eoi,
        ]);
    }

    #[test]
    fn setter_keyword() {
        assert_eq!(kinds("x:: 3"), vec![SetterKeyword, Number, Eoi]);
    }
}
```

*(This test references `Number` and `Eoi` — add those `TokenKind` variants now even though `Number` is lexed in Task 4 and `Eoi` emission is finalized in Task 5.)*

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p ns-syntax lexer::`
Expected: FAIL — `Lexer` / `token` module not found.

- [x] **Step 3: Write minimal implementation**

`crates/ns-syntax/src/token.rs`:

```rust
use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // Task 3
    Identifier,
    Keyword,
    SetterKeyword,
    BinarySelector,
    Class,
    Private,
    Public,
    Protected,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Bar,
    DoubleBar,
    Caret,
    Dot,
    Semicolon,
    Colon,
    Assign,
    Equals,
    ColonColonEquals,
    Pound,
    LessThan,
    GreaterThan,
    // Task 4
    Number,
    String,
    Symbol,
    CharConstant,
    // Task 5
    Eoi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
```

`crates/ns-syntax/src/lexer.rs` (prepend):

```rust
use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::token::{Token, TokenKind};

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    diags: Vec<Diagnostic>,
}

const SPECIAL: &[u8] = b"+-*/~<>=&|@%,?!";

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Lexer<'a> {
        Lexer { src, bytes: src.as_bytes(), pos: 0, tokens: Vec::new(), diags: Vec::new() }
    }

    pub fn run(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        while self.pos < self.bytes.len() {
            self.skip_trivia(); // full impl in Task 5; for now: skip ASCII whitespace
            if self.pos >= self.bytes.len() {
                break;
            }
            self.lex_one();
        }
        self.push(TokenKind::Eoi, self.pos, self.pos);
        (self.tokens, self.diags)
    }

    fn skip_trivia(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token { kind, span: Span::new(start as u32, end as u32) });
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn lex_one(&mut self) {
        let start = self.pos;
        let b = self.bytes[self.pos];
        match b {
            b'(' => self.single(TokenKind::LParen),
            b')' => self.single(TokenKind::RParen),
            b'[' => self.single(TokenKind::LBracket),
            b']' => self.single(TokenKind::RBracket),
            b'{' => self.single(TokenKind::LBrace),
            b'}' => self.single(TokenKind::RBrace),
            b'^' => self.single(TokenKind::Caret),
            b'.' => self.single(TokenKind::Dot),
            b';' => self.single(TokenKind::Semicolon),
            b'#' => self.single(TokenKind::Pound), // Symbol body lexed in Task 4
            b'|' => {
                if self.peek_at(1) == Some(b'|') {
                    self.pos += 2;
                    self.push(TokenKind::DoubleBar, start, self.pos);
                } else {
                    self.single(TokenKind::Bar);
                }
            }
            b':' => {
                if self.peek_at(1) == Some(b'=') {
                    self.pos += 2;
                    self.push(TokenKind::Assign, start, self.pos);
                } else {
                    self.single(TokenKind::Colon);
                }
            }
            _ if b == b'_' || b.is_ascii_alphabetic() => self.lex_word(),
            _ if b.is_ascii_digit() => self.lex_number(), // Task 4
            b'\'' => self.lex_string(),                   // Task 4
            b'"' => self.lex_char_constant(),             // Task 4
            _ if SPECIAL.contains(&b) => self.lex_operator(),
            _ => {
                self.pos += 1;
                self.diags.push(Diagnostic::error(
                    Span::new(start as u32, self.pos as u32),
                    format!("unexpected character {:?}", b as char),
                ));
            }
        }
    }

    fn single(&mut self, kind: TokenKind) {
        let start = self.pos;
        self.pos += 1;
        self.push(kind, start, self.pos);
    }

    fn lex_word(&mut self) {
        let start = self.pos;
        while matches!(self.peek(), Some(b) if b == b'_' || b.is_ascii_alphanumeric()) {
            self.pos += 1;
        }
        // keyword / setter-keyword?
        if self.peek() == Some(b':') {
            if self.peek_at(1) == Some(b'=') {
                // `id:=` -> Identifier then Assign; stop the word here.
                let kind = self.word_kind(start, self.pos);
                self.push(kind, start, self.pos);
                return;
            }
            if self.peek_at(1) == Some(b':') && self.peek_at(2) != Some(b'=') {
                self.pos += 2;
                self.push(TokenKind::SetterKeyword, start, self.pos);
                return;
            }
            self.pos += 1;
            self.push(TokenKind::Keyword, start, self.pos);
            return;
        }
        let kind = self.word_kind(start, self.pos);
        self.push(kind, start, self.pos);
    }

    fn word_kind(&self, start: usize, end: usize) -> TokenKind {
        match &self.src[start..end] {
            "class" => TokenKind::Class,
            "private" => TokenKind::Private,
            "public" => TokenKind::Public,
            "protected" => TokenKind::Protected,
            _ => TokenKind::Identifier,
        }
    }

    fn lex_operator(&mut self) {
        let start = self.pos;
        // Fixed single-char punctuation that also happens to be `specialCharacter`.
        let b = self.bytes[self.pos];
        let next = self.peek_at(1);
        let is_run_of_two_plus = next.is_some_and(|n| SPECIAL.contains(&n));
        if !is_run_of_two_plus {
            match b {
                b'=' => return self.single(TokenKind::Equals),
                b'<' => return self.single(TokenKind::LessThan),
                b'>' => return self.single(TokenKind::GreaterThan),
                _ => {}
            }
        }
        // `::=` is handled in `lex_one`'s `:` arm before we get here; a run
        // starting with a special char is a binary selector.
        while matches!(self.peek(), Some(n) if SPECIAL.contains(&n)) {
            self.pos += 1;
        }
        self.push(TokenKind::BinarySelector, start, self.pos);
    }

    // Task 4 stubs — implemented next task.
    fn lex_number(&mut self) { self.single(TokenKind::Number); }
    fn lex_string(&mut self) { self.single(TokenKind::String); }
    fn lex_char_constant(&mut self) { self.single(TokenKind::CharConstant); }
}
```

Handle `::=` in the `:` arm — replace the `b':'` arm body with:

```rust
b':' => {
    if self.peek_at(1) == Some(b'=') {
        self.pos += 2;
        self.push(TokenKind::Assign, start, self.pos);
    } else if self.peek_at(1) == Some(b':') && self.peek_at(2) == Some(b'=') {
        self.pos += 3;
        self.push(TokenKind::ColonColonEquals, start, self.pos);
    } else {
        self.single(TokenKind::Colon);
    }
}
```

Update `lib.rs`:

```rust
pub mod lexer;
pub mod token;
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test -p ns-syntax lexer::`
Expected: PASS (6 tests). The `setter_keyword` test needs `x::` → `SetterKeyword`; verify `x:: 3` does not consume the space.

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): lex identifiers, keywords, operators, punctuation

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 4: Lexer — numbers, strings, symbols, character constants, patterns

**Files:**
- Modify: `crates/ns-syntax/src/lexer.rs` (replace the Task 3 stubs)
- Modify: `crates/ns-syntax/src/token.rs` (no new variants; `Number`, `String`, `Symbol`, `CharConstant` already declared)

**Interfaces:**
- Produces: real lexing for
  - `Number` — `decimalNum = '-'? digits ('.' digits)? ('e' '-'? digits)?` and `radixNum = digits 'r' '-'? extendedDigits ('.' extendedDigits)? ('e' '-'? digits)?` (e.g. `16rFF`, `2r1010`, `1.5e-3`, `-42`). The leading `-` is part of the literal **only** when not preceded by a value token — see note.
  - `String` — `'...'` with `''` as an escaped single quote.
  - `Symbol` — `#` followed by: a string (`#'...'`), a keyword run (`#at:put:`), a binary selector (`#+`), or an identifier (`#foo`). The `#` was already emitted as `Pound` in Task 3 — **change** Task 3 so `#` calls `lex_symbol()` instead of emitting `Pound`. Keep `Pound` only if a bare `#` appears (diagnostic).
  - `CharConstant` — `"` + exactly one character (which may be `'`, `"`, or space) + `"`.

**Note on leading `-`:** the grammar puts `-` inside `decimalNum`, but `a - b` must lex as three tokens. Rule: the lexer emits a negative `Number` only when the `-` is immediately followed by a digit **and** the previous emitted token is not one that can end an expression (`Identifier`, `Number`, `String`, `Symbol`, `CharConstant`, `RParen`, `RBracket`, `RBrace`). Otherwise `-` starts a `BinarySelector`. Put this decision in `lex_operator` / a shared helper.

- [x] **Step 1: Write the failing tests**

Add to `lexer.rs` tests:

```rust
#[test]
fn decimal_numbers() {
    assert_eq!(kinds("0 42 3.14 1.5e-3"), vec![
        Number, Number, Number, Number, Eoi,
    ]);
}

#[test]
fn radix_numbers() {
    assert_eq!(kinds("16rFF 2r1010"), vec![Number, Number, Eoi]);
}

#[test]
fn negative_number_vs_subtraction() {
    assert_eq!(kinds("-42"), vec![Number, Eoi]);
    assert_eq!(kinds("a - 42"), vec![Identifier, BinarySelector, Number, Eoi]);
    assert_eq!(kinds("a -42"), vec![Identifier, Number, Eoi]);
}

#[test]
fn strings_with_escaped_quote() {
    let (tokens, diags) = Lexer::new("'it''s'").run();
    assert!(diags.is_empty());
    assert_eq!(tokens[0].kind, String);
    assert_eq!(tokens[0].span.text("'it''s'"), "'it''s'");
}

#[test]
fn symbols() {
    assert_eq!(kinds("#foo #at:put: #+ #'hi there'"), vec![
        Symbol, Symbol, Symbol, Symbol, Eoi,
    ]);
}

#[test]
fn character_constants() {
    assert_eq!(kinds(r#""a" "'" " ""#), vec![CharConstant, CharConstant, CharConstant, Eoi]);
}

#[test]
fn unterminated_string_is_diagnostic() {
    let (_tokens, diags) = Lexer::new("'oops").run();
    assert_eq!(diags.len(), 1);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-syntax lexer::`
Expected: FAIL — stubs emit wrong token counts.

- [x] **Step 3: Write the implementation**

Replace the Task 3 stubs in `lexer.rs`:

```rust
fn lex_number(&mut self) {
    let start = self.pos;
    if self.peek() == Some(b'-') {
        self.pos += 1;
    }
    self.eat_digits();
    // radix?
    if self.peek() == Some(b'r') {
        self.pos += 1;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        self.eat_extended_digits();
        if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|b| b.is_ascii_alphanumeric()) {
            self.pos += 1;
            self.eat_extended_digits();
        }
    } else if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|b| b.is_ascii_digit()) {
        self.pos += 1;
        self.eat_digits();
    }
    if self.peek() == Some(b'e') && self.exponent_follows() {
        self.pos += 1;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        self.eat_digits();
    }
    self.push(TokenKind::Number, start, self.pos);
}

fn exponent_follows(&self) -> bool {
    let mut i = 1;
    if self.peek_at(i) == Some(b'-') {
        i += 1;
    }
    self.peek_at(i).is_some_and(|b| b.is_ascii_digit())
}

fn eat_digits(&mut self) {
    while matches!(self.peek(), Some(b) if b.is_ascii_digit()) {
        self.pos += 1;
    }
}

fn eat_extended_digits(&mut self) {
    while matches!(self.peek(), Some(b) if b.is_ascii_digit() || b.is_ascii_uppercase()) {
        self.pos += 1;
    }
}

fn lex_string(&mut self) {
    let start = self.pos;
    self.pos += 1; // opening quote
    loop {
        match self.peek() {
            None => {
                self.diags.push(Diagnostic::error(
                    Span::new(start as u32, self.pos as u32),
                    "unterminated string literal",
                ));
                break;
            }
            Some(b'\'') => {
                if self.peek_at(1) == Some(b'\'') {
                    self.pos += 2;
                } else {
                    self.pos += 1;
                    break;
                }
            }
            Some(_) => self.pos += 1,
        }
    }
    self.push(TokenKind::String, start, self.pos);
}

fn lex_symbol(&mut self) {
    let start = self.pos;
    self.pos += 1; // '#'
    match self.peek() {
        Some(b'\'') => {
            self.lex_string();
            self.tokens.pop(); // replace String token with Symbol span
        }
        Some(b) if b == b'_' || b.is_ascii_alphabetic() => {
            // identifier, possibly a run of keywords `at:put:`
            while matches!(self.peek(), Some(c) if c == b'_' || c.is_ascii_alphanumeric()) {
                self.pos += 1;
            }
            while self.peek() == Some(b':') {
                self.pos += 1;
                while matches!(self.peek(), Some(c) if c == b'_' || c.is_ascii_alphanumeric()) {
                    self.pos += 1;
                }
            }
        }
        Some(b) if SPECIAL.contains(&b) => {
            while matches!(self.peek(), Some(c) if SPECIAL.contains(&c)) {
                self.pos += 1;
            }
        }
        _ => {
            self.diags.push(Diagnostic::error(
                Span::new(start as u32, self.pos as u32),
                "expected symbol after `#`",
            ));
        }
    }
    self.push(TokenKind::Symbol, start, self.pos);
}

fn lex_char_constant(&mut self) {
    let start = self.pos;
    self.pos += 1; // opening `"`
    match self.peek() {
        Some(_) => self.pos += 1,
        None => {
            self.diags.push(Diagnostic::error(
                Span::new(start as u32, self.pos as u32),
                "unterminated character constant",
            ));
            self.push(TokenKind::CharConstant, start, self.pos);
            return;
        }
    }
    if self.peek() == Some(b'"') {
        self.pos += 1;
    } else {
        self.diags.push(Diagnostic::error(
            Span::new(start as u32, self.pos as u32),
            "character constant must contain exactly one character",
        ));
    }
    self.push(TokenKind::CharConstant, start, self.pos);
}
```

In `lex_one`, change the `b'#'` arm to `b'#' => self.lex_symbol(),` and the `b'"'` arm to `b'"' => self.lex_char_constant(),`. Delete the `Pound` single from the `#` arm (keep the `Pound` variant declared for potential future use).

Add negative-number detection: in `lex_one`, before the `SPECIAL.contains(&b)` arm, add:

```rust
_ if b == b'-'
    && self.peek_at(1).is_some_and(|n| n.is_ascii_digit())
    && !self.prev_ends_value() =>
{
    self.lex_number()
}
```

and the helper:

```rust
fn prev_ends_value(&self) -> bool {
    matches!(
        self.tokens.last().map(|t| t.kind),
        Some(
            TokenKind::Identifier
                | TokenKind::Number
                | TokenKind::String
                | TokenKind::Symbol
                | TokenKind::CharConstant
                | TokenKind::RParen
                | TokenKind::RBracket
                | TokenKind::RBrace
        )
    )
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-syntax lexer::`
Expected: PASS (all lexer tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): lex numbers, strings, symbols, character constants

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 5: Lexer — comments, trivia, and the public `lex()` API

**Files:**
- Modify: `crates/ns-syntax/src/lexer.rs`
- Modify: `crates/ns-syntax/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn lex(src: &str) -> Result<Vec<Token>, Vec<Diagnostic>>` — `Ok` iff no `Severity::Error` diagnostics; the `Vec<Token>` always ends with `Eoi`.
  - Comments `(* ... *)` are skipped as trivia. They **nest** (`(* a (* b *) c *)`). An unterminated comment is one `Diagnostic`.
  - `(* :literalmessage: ... *)` is still skipped at lex time (the parser does not see it in Milestone 1); just treat it as an ordinary nesting comment.
  - Whitespace (space, tab, CR, LF, form feed) is trivia.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn line_and_block_comments_are_trivia() {
    assert_eq!(kinds("a (* comment *) b"), vec![Identifier, Identifier, Eoi]);
}

#[test]
fn comments_nest() {
    assert_eq!(kinds("a (* x (* y *) z *) b"), vec![Identifier, Identifier, Eoi]);
}

#[test]
fn unterminated_comment_is_diagnostic() {
    let (_t, diags) = Lexer::new("a (* oops").run();
    assert_eq!(diags.len(), 1);
}

#[test]
fn lex_api_returns_err_on_diagnostics() {
    assert!(crate::lex("'unterminated").is_err());
    assert!(crate::lex("class Foo = ()").is_ok());
}

#[test]
fn token_stream_ends_with_eoi() {
    let tokens = crate::lex("foo").unwrap();
    assert_eq!(tokens.last().unwrap().kind, Eoi);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-syntax`
Expected: FAIL — `crate::lex` missing; comments currently lex as `LParen`/`BinarySelector`/`RParen`.

- [x] **Step 3: Write the implementation**

Replace `skip_trivia` in `lexer.rs`:

```rust
fn skip_trivia(&mut self) {
    loop {
        match self.peek() {
            Some(b) if b.is_ascii_whitespace() => self.pos += 1,
            Some(b'(') if self.peek_at(1) == Some(b'*') => self.skip_block_comment(),
            _ => break,
        }
    }
}

fn skip_block_comment(&mut self) {
    let start = self.pos;
    self.pos += 2;
    let mut depth = 1u32;
    while depth > 0 {
        match (self.peek(), self.peek_at(1)) {
            (Some(b'('), Some(b'*')) => {
                self.pos += 2;
                depth += 1;
            }
            (Some(b'*'), Some(b')')) => {
                self.pos += 2;
                depth -= 1;
            }
            (Some(_), _) => self.pos += 1,
            (None, _) => {
                self.diags.push(Diagnostic::error(
                    Span::new(start as u32, self.pos as u32),
                    "unterminated comment",
                ));
                return;
            }
        }
    }
}
```

Add to `lib.rs`:

```rust
use crate::token::Token;

/// Lex `src` into tokens. Returns `Err` if any error-severity diagnostic was produced.
pub fn lex(src: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let (tokens, diags) = lexer::Lexer::new(src).run();
    if diags.iter().any(|d| d.severity == Severity::Error) {
        Err(diags)
    } else {
        Ok(tokens)
    }
}
```

and `pub use token::{Token, TokenKind};`.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-syntax`
Expected: PASS (all lexer + api tests).

- [x] **Step 5: Sanity-check against the real grammar file**

Run a throwaway check that the lexer does not choke on the vendored grammar:

```rust
#[test]
fn lexes_vendored_grammar_without_error() {
    let src = std::fs::read_to_string("../../corpus/reference/NewspeakGrammar.ns").unwrap();
    let (_tokens, diags) = Lexer::new(&src).run();
    assert!(diags.is_empty(), "{diags:#?}");
}
```

Run: `cargo test -p ns-syntax lexes_vendored_grammar`
Expected: PASS. If it fails, fix the lexer (do not edit the grammar file); the failure is a real lexical case (likely radix numbers, `<...>` patterns, or a `specialCharacter` not in `SPECIAL`).

- [x] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): skip nested comments and add public lex() API

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 6: AST types

**Files:**
- Create: `crates/ns-syntax/src/ast.rs`
- Modify: `crates/ns-syntax/src/lib.rs`

**Interfaces:**
- Produces the full AST vocabulary the parser builds and the (future) resolver consumes. Every node struct has a `pub span: Span`.

```rust
//! Abstract syntax tree for a Newspeak3 compilation unit.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub language_id: String,     // e.g. "Newspeak3"
    pub category: String,        // the quoted category string, contents only
    pub root_class: ClassDecl,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessModifier {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    pub access: Option<AccessModifier>,
    pub name: Ident,
    pub factory: Option<MessagePattern>,     // absent form: `class C = Super (...)`
    pub superclass: Option<SuperclassClause>,
    pub class_comment: Option<String>,
    pub instance_side: ClassBody,
    pub class_side: Option<ClassBody>,        // the `: ( ... )` block
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassBody {
    pub slots: Vec<SlotDecl>,
    pub init_exprs: Vec<Stmt>,                // instance side only; empty on class side
    pub methods: Vec<MethodDecl>,
    pub nested_classes: Vec<ClassDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuperclassClause {
    pub prefix: Option<SuperclassPrefix>,
    pub receiver: Ident,                      // the unaryMsg receiver identifier
    pub messages: Vec<MessageSend>,           // trailing `message?` (0 or more sends)
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SuperclassPrefix {
    SelfPrefix,
    SuperPrefix,
    Outer(Ident),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotDecl {
    pub access: Option<AccessModifier>,
    pub name: Ident,
    pub mutability: SlotMutability,
    pub init: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotMutability {
    /// `name = expr.`  immutable, initialized
    Immutable,
    /// `name ::= expr.`  mutable, initialized
    MutableInit,
    /// `name.`  mutable, uninitialized
    MutableBare,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub access: Option<AccessModifier>,
    pub pattern: MessagePattern,
    pub body: CodeBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessagePattern {
    Unary { selector: Ident },
    Binary { selector: Ident, param: Ident },
    Keyword { parts: Vec<(Ident, Ident)> }, // (keyword-without-colon, param)
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeBody {
    pub temps: Vec<Ident>,
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    Return(Expr), // `^ expr`
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Ident(Ident),
    Literal(Literal),
    /// `receiver selector: arg ...`, `receiver + arg`, `receiver unary`
    Send(Box<Send>),
    /// `receiver msg1; msg2; msg3`
    Cascade(Box<Cascade>),
    /// `lhs := rhs`  (lhs is always an Ident in Newspeak)
    Assign { target: Ident, value: Box<Expr>, span: Span },
    Block(Box<BlockExpr>),
    /// `{ a. b. c }`
    DynamicArray { elements: Vec<Expr>, span: Span },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Send {
    pub receiver: Expr,
    pub message: MessageSend,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageSend {
    pub selector: Selector,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cascade {
    pub receiver: Expr,          // the receiver of the first message
    pub first: MessageSend,
    pub rest: Vec<MessageSend>,  // each `; msg`
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Unary(String),
    Binary(String),
    Keyword(String), // full selector with colons, e.g. "at:put:"
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockExpr {
    pub params: Vec<Ident>,
    pub body: CodeBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int { text: String, span: Span },     // keep raw text; runtime parses/validates
    Float { text: String, span: Span },
    String { value: String, span: Span }, // unescaped contents
    Symbol { value: String, span: Span }, // without the leading '#'
    Char { value: char, span: Span },
    Bool { value: bool, span: Span },
    Nil { span: Span },
    /// `#( ... )` and `{ ... }` literal-array forms that contain only literals
    Array { elements: Vec<Literal>, span: Span },
    /// `<...>` annotation/pattern literal — kept raw in Milestone 1
    Pattern { text: String, span: Span },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}
```

- [x] **Step 1: Write the file** exactly as above.

- [x] **Step 2: Wire into `lib.rs`**

```rust
pub mod ast;
```

- [x] **Step 3: Add a construction smoke test**

`crates/ns-syntax/src/ast.rs` (append):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_compare_by_value() {
        let a = Literal::Nil { span: Span::new(0, 3) };
        let b = Literal::Nil { span: Span::new(0, 3) };
        assert_eq!(a, b);
    }
}
```

- [x] **Step 4: Build**

Run: `cargo test -p ns-syntax ast::`
Expected: PASS (1 test); no clippy warnings.

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): define the Newspeak AST

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 7: Parser infrastructure and primary expressions

**Files:**
- Create: `crates/ns-syntax/src/parser/mod.rs`
- Create: `crates/ns-syntax/src/parser/expr.rs`
- Modify: `crates/ns-syntax/src/lib.rs`

**Interfaces:**
- Consumes: `lex()`, `Token`, `TokenKind`, `ast::*`, `Diagnostic`, `Span`.
- Produces (crate-internal for now):
  - `struct Parser<'a> { src: &'a str, tokens: Vec<Token>, pos: usize, diags: Vec<Diagnostic> }`
  - `Parser::new(src: &'a str, tokens: Vec<Token>) -> Parser<'a>`
  - cursor helpers: `peek(&self) -> TokenKind`, `peek2(&self) -> TokenKind`, `at(&self, k: TokenKind) -> bool`, `bump(&mut self) -> Token`, `eat(&mut self, k: TokenKind) -> bool`, `expect(&mut self, k: TokenKind) -> Option<Token>` (pushes a diagnostic and returns `None` on mismatch), `ident_text(&self, tok: Token) -> String`, `span_from(&self, start: Token) -> Span`.
  - `Parser::parse_primary(&mut self) -> Expr` — identifier, literal, `( expr )`, block, dynamic array `{ ... }`. Literal parsing lives here: int/float (classified by presence of `.`/`e` in the lexeme, radix always Int), string, symbol, char, and the keyword identifiers `nil`/`true`/`false` (recognized here, not in the lexer).
- Recovery: on an unexpected token, push one diagnostic, synthesize a `Literal::Nil` node spanning the bad token, and do **not** advance past a structural token (`RParen`, `RBracket`, `Dot`, `Eoi`).

- [x] **Step 1: Write the failing tests**

`crates/ns-syntax/src/parser/mod.rs`:

```rust
//! Recursive-descent + Pratt parser for Newspeak3.

mod expr;

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::token::{Token, TokenKind};

pub struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_expr(src: &str) -> Expr {
        let tokens = crate::lex(src).expect("lex");
        let mut p = Parser::new(src, tokens);
        let e = p.parse_expr_for_test();
        assert!(p.diags.is_empty(), "{:?}", p.diags);
        e
    }

    #[test]
    fn bare_identifier() {
        match parse_expr("foo") {
            Expr::Ident(id) => assert_eq!(id.name, "foo"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn integer_vs_float_literal() {
        assert!(matches!(parse_expr("42"), Expr::Literal(Literal::Int { .. })));
        assert!(matches!(parse_expr("3.5"), Expr::Literal(Literal::Float { .. })));
        assert!(matches!(parse_expr("16rFF"), Expr::Literal(Literal::Int { .. })));
    }

    #[test]
    fn nil_true_false_are_literals() {
        assert!(matches!(parse_expr("nil"), Expr::Literal(Literal::Nil { .. })));
        assert!(matches!(parse_expr("true"), Expr::Literal(Literal::Bool { value: true, .. })));
    }

    #[test]
    fn parenthesized() {
        assert!(matches!(parse_expr("(foo)"), Expr::Ident(_)));
    }
}
```

Add a test-only entry point in `expr.rs` or `mod.rs`:

```rust
#[cfg(test)]
impl<'a> Parser<'a> {
    pub(crate) fn parse_expr_for_test(&mut self) -> Expr {
        self.parse_expression()
    }
}
```

*(`parse_expression` is implemented in Task 8; for Task 7 make it temporarily delegate to `parse_primary`.)*

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-syntax parser::`
Expected: FAIL — `Parser` methods missing.

- [x] **Step 3: Write the implementation**

`parser/mod.rs` cursor helpers:

```rust
impl<'a> Parser<'a> {
    pub fn new(src: &'a str, tokens: Vec<Token>) -> Parser<'a> {
        Parser { src, tokens, pos: 0, diags: Vec::new() }
    }

    fn cur(&self) -> Token {
        self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek(&self) -> TokenKind {
        self.cur().kind
    }

    fn peek2(&self) -> TokenKind {
        self.tokens.get(self.pos + 1).map_or(TokenKind::Eoi, |t| t.kind)
    }

    fn at(&self, k: TokenKind) -> bool {
        self.peek() == k
    }

    fn bump(&mut self) -> Token {
        let t = self.cur();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, k: TokenKind) -> bool {
        if self.at(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, k: TokenKind) -> Option<Token> {
        if self.at(k) {
            Some(self.bump())
        } else {
            let t = self.cur();
            self.diags.push(Diagnostic::error(
                t.span,
                format!("expected {k:?}, found {:?}", t.kind),
            ));
            None
        }
    }

    fn text(&self, tok: Token) -> &str {
        tok.span.text(self.src)
    }

    fn ident(&mut self) -> Ident {
        let t = self.bump();
        Ident { name: self.text(t).to_string(), span: t.span }
    }
}
```

`parser/expr.rs`:

```rust
use super::Parser;
use crate::ast::*;
use crate::token::TokenKind as T;

impl<'a> Parser<'a> {
    pub(super) fn parse_primary(&mut self) -> Expr {
        match self.peek() {
            T::LParen => {
                self.bump();
                let e = self.parse_expression();
                self.expect(T::RParen);
                e
            }
            T::LBracket => self.parse_block(),
            T::LBrace => self.parse_dynamic_array(),
            T::Number | T::String | T::Symbol | T::CharConstant => {
                Expr::Literal(self.parse_literal())
            }
            T::Identifier => {
                let t = self.cur();
                match self.text(t) {
                    "nil" => { self.bump(); Expr::Literal(Literal::Nil { span: t.span }) }
                    "true" => { self.bump(); Expr::Literal(Literal::Bool { value: true, span: t.span }) }
                    "false" => { self.bump(); Expr::Literal(Literal::Bool { value: false, span: t.span }) }
                    _ => Expr::Ident(self.ident()),
                }
            }
            T::LessThan => Expr::Literal(self.parse_pattern_literal()),
            _ => {
                let t = self.cur();
                self.diags.push(crate::diagnostic::Diagnostic::error(
                    t.span,
                    format!("expected an expression, found {:?}", t.kind),
                ));
                Expr::Literal(Literal::Nil { span: t.span })
            }
        }
    }

    fn parse_literal(&mut self) -> Literal {
        let t = self.bump();
        let raw = self.text(t).to_string();
        match t.kind {
            T::Number => {
                let is_radix = raw.contains('r');
                let looks_float = !is_radix && (raw.contains('.') || raw.contains('e'));
                if looks_float {
                    Literal::Float { text: raw, span: t.span }
                } else {
                    Literal::Int { text: raw, span: t.span }
                }
            }
            T::String => Literal::String { value: unescape_string(&raw), span: t.span },
            T::Symbol => Literal::Symbol { value: symbol_body(&raw), span: t.span },
            T::CharConstant => {
                let inner = &raw[1..raw.len() - 1];
                Literal::Char { value: inner.chars().next().unwrap_or('\u{0}'), span: t.span }
            }
            _ => unreachable!("parse_literal called on {:?}", t.kind),
        }
    }

    // `<...>` — capture raw text including the angle brackets.
    fn parse_pattern_literal(&mut self) -> Literal {
        let start = self.bump(); // `<`
        let mut end = start;
        while !self.at(T::GreaterThan) && !self.at(T::Eoi) {
            end = self.bump();
        }
        if let Some(gt) = self.expect(T::GreaterThan) {
            end = gt;
        }
        let span = start.span.merge(end.span);
        Literal::Pattern { text: span.text(self.src).to_string(), span }
    }

    // Placeholder until Task 9.
    fn parse_block(&mut self) -> Expr {
        let lb = self.bump();
        // minimal: skip to matching ]
        while !self.at(T::RBracket) && !self.at(T::Eoi) {
            self.bump();
        }
        let rb = self.bump();
        Expr::Block(Box::new(BlockExpr {
            params: Vec::new(),
            body: CodeBody { temps: Vec::new(), stmts: Vec::new(), span: lb.span.merge(rb.span) },
            span: lb.span.merge(rb.span),
        }))
    }

    fn parse_dynamic_array(&mut self) -> Expr {
        let lb = self.bump();
        while !self.at(T::RBrace) && !self.at(T::Eoi) {
            self.bump();
        }
        let rb = self.bump();
        Expr::DynamicArray { elements: Vec::new(), span: lb.span.merge(rb.span) }
    }
}

fn unescape_string(raw: &str) -> String {
    let inner = &raw[1..raw.len() - 1];
    inner.replace("''", "'")
}

fn symbol_body(raw: &str) -> String {
    let body = &raw[1..]; // drop '#'
    if body.starts_with('\'') {
        body[1..body.len() - 1].replace("''", "'")
    } else {
        body.to_string()
    }
}
```

Add stub in `mod.rs`:

```rust
impl<'a> Parser<'a> {
    pub(crate) fn parse_expression(&mut self) -> Expr {
        // Task 8 replaces this with full message-send parsing.
        self.parse_primary()
    }
}
```

Update `lib.rs`: `mod parser;` (private for now).

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-syntax parser::`
Expected: PASS (4 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): parser scaffold and primary expressions

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 8: Parser — message sends, cascades, assignment

**Files:**
- Modify: `crates/ns-syntax/src/parser/expr.rs`
- Modify: `crates/ns-syntax/src/parser/mod.rs`

**Interfaces:**
- Produces:
  - `Parser::parse_expression(&mut self) -> Expr` — full precedence: `assignment` → `keyword message` → `binary message` → `unary message` → `primary`.
  - Assignment: `Ident := expr` (right-associative). Only a bare identifier is a legal target; anything else is a diagnostic but still parsed as `Assign` for recovery.
  - `setterKeyword` form: `recv foo:: expr` parses as a keyword send with selector `foo:` and one argument. (Grammar's `setterKeyword star , sendExpression`; Milestone 1 treats a single leading `id::` as sugar for `id: <sendExpression>` sent to the enclosing receiver — implement the common `recv x:: v` case; a bare leading `x:: v` with no receiver is `self x: v` and is left to the resolver, so parse it as a `Send` with receiver `Expr::Ident("self")`... **no** — instead represent it as `Assign { target, value }` because `x:: v` is semantically a slot write. Document this choice in a comment.)
  - Cascades: `recv m1; m2; m3` where `m1..m3` are unary/binary/keyword messages sent to `recv`. Binds looser than a single message, tighter than assignment.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn unary_chain() {
    // foo bar baz  ==  ((foo bar) baz)
    let e = parse_expr("foo bar baz");
    let Expr::Send(s) = e else { panic!() };
    assert_eq!(s.message.selector, Selector::Unary("baz".into()));
    assert!(matches!(s.receiver, Expr::Send(_)));
}

#[test]
fn binary_left_assoc_and_below_unary() {
    // a + b foo  ==  a + (b foo)
    let e = parse_expr("a + b foo");
    let Expr::Send(s) = e else { panic!() };
    assert_eq!(s.message.selector, Selector::Binary("+".into()));
    assert!(matches!(s.message.args[0], Expr::Send(_)));
}

#[test]
fn keyword_below_binary() {
    // d at: i + 1 put: x  ==  d at:(i+1) put:x  (one keyword send, selector at:put:)
    let e = parse_expr("d at: i + 1 put: x");
    let Expr::Send(s) = e else { panic!() };
    assert_eq!(s.message.selector, Selector::Keyword("at:put:".into()));
    assert_eq!(s.message.args.len(), 2);
}

#[test]
fn assignment_right_assoc() {
    let e = parse_expr("x := y := 1");
    let Expr::Assign { target, value, .. } = e else { panic!() };
    assert_eq!(target.name, "x");
    assert!(matches!(*value, Expr::Assign { .. }));
}

#[test]
fn cascade() {
    let e = parse_expr("s add: 1; add: 2; yourself");
    let Expr::Cascade(c) = e else { panic!() };
    assert_eq!(c.rest.len(), 2);
    assert_eq!(c.first.selector, Selector::Keyword("add:".into()));
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-syntax parser::`
Expected: FAIL.

- [x] **Step 3: Write the implementation**

Replace `parse_expression` and add the precedence ladder in `expr.rs`:

```rust
impl<'a> Parser<'a> {
    pub(crate) fn parse_expression(&mut self) -> Expr {
        // assignment: Ident ':=' expression
        if self.at(T::Identifier) && self.peek2() == T::Assign {
            let target = self.ident();
            self.bump(); // :=
            let value = self.parse_expression();
            let span = target.span.merge(expr_span(&value));
            return Expr::Assign { target, value: Box::new(value), span };
        }
        self.parse_cascade()
    }

    fn parse_cascade(&mut self) -> Expr {
        let first_full = self.parse_keyword_message();
        if !self.at(T::Semicolon) {
            return first_full;
        }
        // `first_full` must be a Send; its receiver is the cascade receiver.
        let Expr::Send(boxed) = first_full else {
            let t = self.cur();
            self.diags.push(crate::diagnostic::Diagnostic::error(
                t.span, "cascade must follow a message send",
            ));
            return first_full;
        };
        let Send { receiver, message, span: first_span } = *boxed;
        let mut rest = Vec::new();
        while self.eat(T::Semicolon) {
            rest.push(self.parse_one_message_send());
        }
        let last = rest.last().map(|m| m.span).unwrap_or(first_span);
        Expr::Cascade(Box::new(Cascade {
            span: expr_span(&receiver).merge(last),
            receiver,
            first: message,
            rest,
        }))
    }

    fn parse_keyword_message(&mut self) -> Expr {
        let mut recv = self.parse_binary_message();
        if self.at(T::Keyword) || self.at(T::SetterKeyword) {
            let start = expr_span(&recv);
            let mut selector = String::new();
            let mut args = Vec::new();
            let mut end = start;
            while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                let kw = self.bump();
                let kw_text = self.text(kw);
                // SetterKeyword lexeme is `foo::`; normalize to `foo:`
                let part = kw_text.trim_end_matches(':');
                selector.push_str(part);
                selector.push(':');
                let arg = self.parse_binary_message();
                end = expr_span(&arg);
                args.push(arg);
            }
            recv = Expr::Send(Box::new(Send {
                span: start.merge(end),
                receiver: recv,
                message: MessageSend { selector: Selector::Keyword(selector), args, span: start.merge(end) },
            }));
        }
        recv
    }

    fn parse_binary_message(&mut self) -> Expr {
        let mut recv = self.parse_unary_message();
        while self.at(T::BinarySelector) {
            let op = self.bump();
            let arg = self.parse_unary_message();
            let span = expr_span(&recv).merge(expr_span(&arg));
            recv = Expr::Send(Box::new(Send {
                span,
                receiver: recv,
                message: MessageSend {
                    selector: Selector::Binary(self.text(op).to_string()),
                    args: vec![arg],
                    span,
                },
            }));
        }
        recv
    }

    fn parse_unary_message(&mut self) -> Expr {
        let mut recv = self.parse_primary();
        while self.at(T::Identifier) && !self.is_reserved_ident(self.cur()) && self.peek2() != T::Assign {
            let sel = self.bump();
            let span = expr_span(&recv).merge(sel.span);
            recv = Expr::Send(Box::new(Send {
                span,
                receiver: recv,
                message: MessageSend {
                    selector: Selector::Unary(self.text(sel).to_string()),
                    args: Vec::new(),
                    span,
                },
            }));
        }
        recv
    }

    // A single `; msg` element: unary | binary | keyword, no receiver.
    fn parse_one_message_send(&mut self) -> MessageSend {
        let start = self.cur().span;
        match self.peek() {
            T::Keyword | T::SetterKeyword => {
                let mut selector = String::new();
                let mut args = Vec::new();
                let mut end = start;
                while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                    let kw = self.bump();
                    selector.push_str(self.text(kw).trim_end_matches(':'));
                    selector.push(':');
                    let arg = self.parse_binary_message();
                    end = expr_span(&arg);
                    args.push(arg);
                }
                MessageSend { selector: Selector::Keyword(selector), args, span: start.merge(end) }
            }
            T::BinarySelector => {
                let op = self.bump();
                let arg = self.parse_unary_message();
                MessageSend {
                    selector: Selector::Binary(self.text(op).to_string()),
                    span: start.merge(expr_span(&arg)),
                    args: vec![arg],
                }
            }
            _ => {
                let sel = self.expect(T::Identifier).unwrap_or(self.cur());
                MessageSend {
                    selector: Selector::Unary(self.text(sel).to_string()),
                    args: Vec::new(),
                    span: sel.span,
                }
            }
        }
    }

    fn is_reserved_ident(&self, tok: crate::token::Token) -> bool {
        matches!(self.text(tok), "nil" | "true" | "false")
    }
}

pub(super) fn expr_span(e: &Expr) -> crate::span::Span {
    match e {
        Expr::Ident(i) => i.span,
        Expr::Literal(l) => literal_span(l),
        Expr::Send(s) => s.span,
        Expr::Cascade(c) => c.span,
        Expr::Assign { span, .. } => *span,
        Expr::Block(b) => b.span,
        Expr::DynamicArray { span, .. } => *span,
    }
}

pub(super) fn literal_span(l: &Literal) -> crate::span::Span {
    match l {
        Literal::Int { span, .. }
        | Literal::Float { span, .. }
        | Literal::String { span, .. }
        | Literal::Symbol { span, .. }
        | Literal::Char { span, .. }
        | Literal::Bool { span, .. }
        | Literal::Nil { span }
        | Literal::Array { span, .. }
        | Literal::Pattern { span, .. } => *span,
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-syntax parser::`
Expected: PASS (Task 7 + Task 8 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): parse message sends, cascades, and assignment

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 9: Parser — blocks, code bodies, and literal arrays

**Files:**
- Modify: `crates/ns-syntax/src/parser/expr.rs`
- Create: `crates/ns-syntax/src/parser/decl.rs` (only `parse_code_body` for now; class/method parsing is Task 10–12)
- Modify: `crates/ns-syntax/src/parser/mod.rs`

**Interfaces:**
- Produces:
  - `Parser::parse_code_body(&mut self) -> CodeBody` — optional `| temps |` (using `Bar ... Bar`; also accept `DoubleBar ... DoubleBar` empty-temp form defensively) then statements separated by `Dot`, where the last statement may be `^ expr`. A trailing `Dot` is allowed. Stops at `RBracket` / `RParen` / `Eoi`.
  - `Parser::parse_block(&mut self) -> Expr` (replace the Task 7 stub) — `[ (':' param)* '|' ]?` then a `CodeBody` then `]`.
  - Real `parse_dynamic_array` — `{ expr (. expr)* .? }`.
  - Literal arrays `#( ... )`: a `Symbol` token whose lexeme is exactly `#` is impossible (lexer requires a body), so `#(` arrives as `Symbol("#")`? No — fix: in the lexer, `#` followed by `(` must emit `Pound` then `LParen`. Update Task 3/4 note: **`#` immediately followed by `(` emits `Pound`**, and the parser turns `Pound LParen ... RParen` into a `Literal::Array`. Add that lexer case and a test in this task.
  - `parse_literal_array(&mut self) -> Literal` — inside `#( )`, elements are literals; bare identifiers `foo` become `Literal::Symbol("foo")` (Smalltalk rule), nested `( )` are nested arrays, `#` is optional before symbols.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn block_with_params_and_temps() {
    let e = parse_expr("[:x :y | | t | t := x. t + y ]");
    let Expr::Block(b) = e else { panic!() };
    assert_eq!(b.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["x", "y"]);
    assert_eq!(b.body.temps.len(), 1);
    assert_eq!(b.body.stmts.len(), 2);
}

#[test]
fn empty_block() {
    let e = parse_expr("[]");
    let Expr::Block(b) = e else { panic!() };
    assert!(b.params.is_empty() && b.body.stmts.is_empty());
}

#[test]
fn dynamic_array() {
    let e = parse_expr("{ 1. 2 + 3. foo }");
    let Expr::DynamicArray { elements, .. } = e else { panic!() };
    assert_eq!(elements.len(), 3);
}

#[test]
fn literal_array() {
    let e = parse_expr("#(1 2 foo #bar (3 4))");
    let Expr::Literal(Literal::Array { elements, .. }) = e else { panic!() };
    assert_eq!(elements.len(), 5);
    assert!(matches!(elements[2], Literal::Symbol { .. })); // bare `foo`
    assert!(matches!(elements[4], Literal::Array { .. }));
}

#[test]
fn return_statement_in_body() {
    // exercised via block body
    let e = parse_expr("[ ^ 42 ]");
    let Expr::Block(b) = e else { panic!() };
    assert!(matches!(b.body.stmts[0], Stmt::Return(_)));
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-syntax parser::`
Expected: FAIL.

- [x] **Step 3: Write the implementation**

Lexer change (`lexer.rs`, in `lex_symbol` or `lex_one`): when `#` is followed by `(`, emit `Pound` and return without consuming `(`:

```rust
// in lex_one, replace `b'#' => self.lex_symbol(),`
b'#' if self.peek_at(1) == Some(b'(') => self.single(TokenKind::Pound),
b'#' => self.lex_symbol(),
```

Add lexer test:

```rust
#[test]
fn hash_paren_is_pound_then_lparen() {
    assert_eq!(kinds("#(1)"), vec![Pound, LParen, Number, RParen, Eoi]);
}
```

`parser/decl.rs`:

```rust
use super::Parser;
use crate::ast::*;
use crate::token::TokenKind as T;

impl<'a> Parser<'a> {
    pub(crate) fn parse_code_body(&mut self) -> CodeBody {
        let start = self.cur().span;
        let temps = self.parse_temps();
        let mut stmts = Vec::new();
        loop {
            match self.peek() {
                T::RBracket | T::RParen | T::Eoi => break,
                T::Caret => {
                    let caret = self.bump();
                    let e = self.parse_expression();
                    let sp = caret.span.merge(super::expr::expr_span(&e));
                    stmts.push(Stmt::Return(e));
                    let _ = sp;
                    self.eat(T::Dot);
                    break; // nothing after a return
                }
                T::Dot => {
                    self.bump(); // stray dot
                }
                _ => {
                    let e = self.parse_expression();
                    stmts.push(Stmt::Expr(e));
                    if !self.eat(T::Dot) {
                        break;
                    }
                }
            }
        }
        let end = self.cur().span;
        CodeBody { temps, stmts, span: start.merge(end) }
    }

    fn parse_temps(&mut self) -> Vec<Ident> {
        if self.at(T::DoubleBar) {
            self.bump(); // `||` == empty temp list
            return Vec::new();
        }
        if !self.at(T::Bar) {
            return Vec::new();
        }
        // Distinguish `| t |` (temps) from a leading binary `|` expression:
        // a temp list is `Bar Identifier* Bar`. Look ahead.
        let mut i = self.pos + 1;
        loop {
            match self.tokens.get(i).map(|t| t.kind) {
                Some(T::Identifier) => i += 1,
                Some(T::Bar) => break,
                _ => return Vec::new(), // not a temp list
            }
        }
        self.bump(); // opening Bar
        let mut temps = Vec::new();
        while self.at(T::Identifier) {
            temps.push(self.ident());
        }
        self.expect(T::Bar);
        temps
    }
}
```

Replace `parse_block` and `parse_dynamic_array` in `expr.rs`:

```rust
fn parse_block(&mut self) -> Expr {
    let lb = self.expect(T::LBracket).unwrap_or(self.cur());
    let mut params = Vec::new();
    while self.at(T::Colon) {
        self.bump();
        if self.at(T::Identifier) {
            params.push(self.ident());
        }
    }
    if !params.is_empty() {
        self.expect(T::Bar);
    }
    let body = self.parse_code_body();
    let rb = self.expect(T::RBracket).unwrap_or(self.cur());
    let span = lb.span.merge(rb.span);
    Expr::Block(Box::new(BlockExpr { params, body, span }))
}

fn parse_dynamic_array(&mut self) -> Expr {
    let lb = self.bump(); // {
    let mut elements = Vec::new();
    while !self.at(T::RBrace) && !self.at(T::Eoi) {
        elements.push(self.parse_expression());
        if !self.eat(T::Dot) {
            break;
        }
    }
    let rb = self.expect(T::RBrace).unwrap_or(self.cur());
    Expr::DynamicArray { elements, span: lb.span.merge(rb.span) }
}
```

Add literal-array handling in `parse_primary` (the `T::Pound` arm) and a helper:

```rust
T::Pound if self.peek2() == T::LParen => Expr::Literal(self.parse_literal_array()),
```

```rust
fn parse_literal_array(&mut self) -> Literal {
    let pound = self.bump(); // #
    self.expect(T::LParen);
    let mut elements = Vec::new();
    while !self.at(T::RParen) && !self.at(T::Eoi) {
        elements.push(self.parse_array_element());
    }
    let rp = self.expect(T::RParen).unwrap_or(self.cur());
    Literal::Array { elements, span: pound.span.merge(rp.span) }
}

fn parse_array_element(&mut self) -> Literal {
    match self.peek() {
        T::LParen => {
            let lp = self.bump();
            let mut inner = Vec::new();
            while !self.at(T::RParen) && !self.at(T::Eoi) {
                inner.push(self.parse_array_element());
            }
            let rp = self.expect(T::RParen).unwrap_or(self.cur());
            Literal::Array { elements: inner, span: lp.span.merge(rp.span) }
        }
        T::Pound => {
            self.bump();
            self.parse_array_element()
        }
        T::Identifier => {
            let id = self.bump();
            // bare word inside a literal array is a symbol
            Literal::Symbol { value: self.text(id).to_string(), span: id.span }
        }
        T::Number | T::String | T::Symbol | T::CharConstant => self.parse_literal(),
        _ => {
            let t = self.bump();
            self.diags.push(crate::diagnostic::Diagnostic::error(
                t.span, "invalid literal-array element",
            ));
            Literal::Nil { span: t.span }
        }
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-syntax`
Expected: PASS (all lexer + parser tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): parse blocks, code bodies, and literal arrays

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 10: Parser — method declarations

**Files:**
- Modify: `crates/ns-syntax/src/parser/decl.rs`

**Interfaces:**
- Produces:
  - `Parser::parse_message_pattern(&mut self) -> MessagePattern` — `unarySelector` | `binarySelector slotDecl` | `(keyword slotDecl)+`.
  - `Parser::parse_method_decl(&mut self) -> MethodDecl` — `accessModifier? messagePattern '=' '(' codeBody ')'`.
  - `Parser::parse_access_modifier(&mut self) -> Option<AccessModifier>`.

- [x] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod method_tests {
    use super::*;
    use crate::parser::Parser;

    fn method(src: &str) -> MethodDecl {
        let tokens = crate::lex(src).unwrap();
        let mut p = Parser::new(src, tokens);
        let m = p.parse_method_decl_for_test();
        assert!(p.diags_is_empty(), "{:?}", p.debug_diags());
        m
    }

    #[test]
    fn unary_method() {
        let m = method("greeting = ( ^ 'hi' )");
        assert!(matches!(m.pattern, MessagePattern::Unary { .. }));
        assert_eq!(m.body.stmts.len(), 1);
    }

    #[test]
    fn keyword_method_with_access() {
        let m = method("public at: k put: v = ( slot at: k put: v. ^ v )");
        let MessagePattern::Keyword { parts } = &m.pattern else { panic!() };
        assert_eq!(parts.len(), 2);
        assert_eq!(m.access, Some(AccessModifier::Public));
    }

    #[test]
    fn binary_method() {
        let m = method("+ other = ( ^ self addTo: other )");
        assert!(matches!(m.pattern, MessagePattern::Binary { .. }));
    }
}
```

Add test helpers to `Parser` (behind `#[cfg(test)]`): `parse_method_decl_for_test`, `diags_is_empty`, `debug_diags`.

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-syntax method_tests`
Expected: FAIL.

- [x] **Step 3: Write the implementation**

```rust
impl<'a> Parser<'a> {
    pub(crate) fn parse_access_modifier(&mut self) -> Option<AccessModifier> {
        match self.peek() {
            T::Public => { self.bump(); Some(AccessModifier::Public) }
            T::Protected => { self.bump(); Some(AccessModifier::Protected) }
            T::Private => { self.bump(); Some(AccessModifier::Private) }
            _ => None,
        }
    }

    pub(crate) fn parse_message_pattern(&mut self) -> MessagePattern {
        match self.peek() {
            T::BinarySelector => {
                let sel = self.ident_from_bump();
                let param = self.ident();
                MessagePattern::Binary { selector: sel, param }
            }
            T::Keyword | T::SetterKeyword => {
                let mut parts = Vec::new();
                while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                    let kw = self.bump();
                    let key = Ident {
                        name: self.text(kw).trim_end_matches(':').to_string(),
                        span: kw.span,
                    };
                    let param = self.ident();
                    parts.push((key, param));
                }
                MessagePattern::Keyword { parts }
            }
            _ => {
                let sel = self.ident();
                MessagePattern::Unary { selector: sel }
            }
        }
    }

    pub(crate) fn parse_method_decl(&mut self) -> MethodDecl {
        let start = self.cur().span;
        let access = self.parse_access_modifier();
        let pattern = self.parse_message_pattern();
        self.expect(T::Equals);
        self.expect(T::LParen);
        let body = self.parse_code_body();
        let end = self.cur().span;
        self.expect(T::RParen);
        MethodDecl { access, pattern, body, span: start.merge(end) }
    }

    fn ident_from_bump(&mut self) -> Ident {
        let t = self.bump();
        Ident { name: self.text(t).to_string(), span: t.span }
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-syntax method_tests`
Expected: PASS (3 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): parse method declarations

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 11: Parser — slot declarations

**Files:**
- Modify: `crates/ns-syntax/src/parser/decl.rs`

**Interfaces:**
- Produces:
  - `Parser::parse_slot_decls(&mut self) -> Vec<SlotDecl>` — parses `| slotDef* |` (sequential) or `|| slotDef* ||` (simultaneous). Milestone 1 records both the same way; the distinction is not semantically modeled yet (note in a comment). Each `slotDef` is `accessModifier? identifier (('=' | '::=') expression)? '.'`. A bare `identifier '.'` is `MutableBare`.
  - This shares the opening `Bar` / `DoubleBar` token with method-body temps and with binary-`|` expressions; here we are in *class-body position* so a leading `Bar`/`DoubleBar` is always a slot section.

- [x] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod slot_tests {
    use super::*;
    use crate::parser::Parser;

    fn slots(src: &str) -> Vec<SlotDecl> {
        let tokens = crate::lex(src).unwrap();
        let mut p = Parser::new(src, tokens);
        let s = p.parse_slot_decls_for_test();
        assert!(p.diags_is_empty(), "{:?}", p.debug_diags());
        s
    }

    #[test]
    fn immutable_and_mutable_slots() {
        let s = slots("| private List = platform collections List. count ::= 0. name. |");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].access, Some(AccessModifier::Private));
        assert_eq!(s[0].mutability, SlotMutability::Immutable);
        assert_eq!(s[1].mutability, SlotMutability::MutableInit);
        assert_eq!(s[2].mutability, SlotMutability::MutableBare);
    }

    #[test]
    fn empty_section() {
        assert!(slots("| |").is_empty());
    }

    #[test]
    fn simultaneous_section() {
        let s = slots("|| a = 1. b = 2. ||");
        assert_eq!(s.len(), 2);
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-syntax slot_tests`
Expected: FAIL.

- [x] **Step 3: Write the implementation**

```rust
impl<'a> Parser<'a> {
    pub(crate) fn parse_slot_decls(&mut self) -> Vec<SlotDecl> {
        let simultaneous = self.at(T::DoubleBar);
        let close = if simultaneous { T::DoubleBar } else { T::Bar };
        if !self.eat(close) {
            return Vec::new();
        }
        let mut slots = Vec::new();
        while !self.at(close) && !self.at(T::Eoi) {
            let start = self.cur().span;
            let access = self.parse_access_modifier();
            if !self.at(T::Identifier) {
                // recovery: skip to next '.' or close
                let t = self.bump();
                self.push_error(t.span, "expected a slot name");
                self.recover_to_dot(close);
                continue;
            }
            let name = self.ident();
            let (mutability, init) = match self.peek() {
                T::Equals => {
                    self.bump();
                    (SlotMutability::Immutable, Some(self.parse_expression()))
                }
                T::ColonColonEquals => {
                    self.bump();
                    (SlotMutability::MutableInit, Some(self.parse_expression()))
                }
                _ => (SlotMutability::MutableBare, None),
            };
            let end = self.cur().span;
            self.eat(T::Dot);
            slots.push(SlotDecl { access, name, mutability, init, span: start.merge(end) });
        }
        self.expect(close);
        slots
    }

    fn recover_to_dot(&mut self, close: T) {
        while !self.at(T::Dot) && !self.at(close) && !self.at(T::Eoi) {
            self.bump();
        }
        self.eat(T::Dot);
    }
}
```

Add `push_error(&mut self, span: Span, msg: &str)` helper to `mod.rs` if not present.

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-syntax slot_tests`
Expected: PASS (3 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): parse slot declarations

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 12: Parser — class declarations and the public `parse()` API

**Files:**
- Modify: `crates/ns-syntax/src/parser/decl.rs`
- Modify: `crates/ns-syntax/src/parser/mod.rs`
- Modify: `crates/ns-syntax/src/lib.rs`

**Interfaces:**
- Produces:
  - `Parser::parse_class_decl(&mut self) -> ClassDecl` — handles both header forms:
    - `accessModifier? 'class' Ident messagePattern '=' superclassClause? '(' classComment? slotDecls? initExprs methods nestedClasses ')' (':' '(' classSideBody ')')?`
    - the empty-`messagePattern` form `'class' Ident '=' ...`
  - `Parser::parse_superclass_clause(&mut self) -> SuperclassClause` — `superclassPrefix? unaryMsg message?`. `unaryMsg` here starts with an identifier receiver; the trailing `message?` is zero or more unary/binary/keyword sends applied to it (so `Object new`, `superclass subclassMixin: m`, `platform collections List` all parse).
  - `Parser::parse_class_body(&mut self, instance_side: bool) -> ClassBody` — the interleaving inside `( ... )`: optional class comment (a leading `String` literal token that stands alone), optional slot section, then a sequence where each item is a nested `class ...`, a method decl, or (instance side only, before any method/class) an init expression statement terminated by `.`.
  - `pub fn parse(src: &str) -> Result<ast::Module, Vec<Diagnostic>>` — lexes, then parses `languageId category classDecl`, `Eoi`. `languageId` is the first `Identifier` token (`Newspeak3`); `category` is the first `String` literal.
- The rule for telling an init expression from a method: an init expression is any statement that is **not** `accessModifier? messagePattern '='` and not `class`. Implement by lookahead: if `peek() == Class` → nested class; else scan a `messagePattern` speculatively and check for a following `=` `(` — if present it's a method, otherwise rewind and parse a statement. Provide `Parser::checkpoint()` / `Parser::restore(cp)` (just save/restore `pos` and `diags.len()`).

- [x] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod class_tests {
    use super::*;

    fn module(src: &str) -> Module {
        match crate::parse(src) {
            Ok(m) => m,
            Err(d) => panic!("parse errors: {d:#?}"),
        }
    }

    #[test]
    fn minimal_module() {
        let m = module("Newspeak3\n'App'\nclass Empty usingPlatform: p = ()\n");
        assert_eq!(m.language_id, "Newspeak3");
        assert_eq!(m.category, "App");
        assert_eq!(m.root_class.name.name, "Empty");
        let MessagePattern::Keyword { parts } = &m.root_class.factory.as_ref().unwrap() else { panic!() };
        assert_eq!(parts[0].0.name, "usingPlatform");
    }

    #[test]
    fn superclass_clause_with_send() {
        let m = module("Newspeak3\n'X'\nclass C = Object new ()\n");
        let sc = m.root_class.superclass.as_ref().unwrap();
        assert_eq!(sc.receiver.name, "Object");
        assert_eq!(sc.messages.len(), 1);
    }

    #[test]
    fn slots_methods_and_nested_class() {
        let src = r#"Newspeak3
'X'
class Outer usingPlatform: p = (
    (* a comment *)
    | x ::= 0. |
    x := 1.
    public bump = ( x := x + 1 )
    class Inner = ( public ping = ( ^ 42 ) )
)"#;
        let m = module(src);
        let body = &m.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.init_exprs.len(), 1);
        assert_eq!(body.methods.len(), 1);
        assert_eq!(body.nested_classes.len(), 1);
        assert_eq!(m.root_class.class_comment.as_deref(), Some(" a comment "));
    }

    #[test]
    fn class_side_block() {
        let src = "Newspeak3\n'X'\nclass C = ( ) : ( public default = ( ^ C new ) )\n";
        let m = module(src);
        assert_eq!(m.root_class.class_side.as_ref().unwrap().methods.len(), 1);
    }

    #[test]
    fn trailing_garbage_is_an_error() {
        assert!(crate::parse("Newspeak3\n'X'\nclass C = ()\nextra").is_err());
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-syntax class_tests`
Expected: FAIL.

- [x] **Step 3: Write the implementation**

```rust
impl<'a> Parser<'a> {
    pub(crate) fn parse_module(&mut self) -> Module {
        let start = self.cur().span;
        let language_id = if self.at(T::Identifier) {
            self.text(self.bump()).to_string()
        } else {
            self.push_error(self.cur().span, "expected a language id (e.g. `Newspeak3`)");
            String::new()
        };
        let category = if self.at(T::String) {
            let t = self.bump();
            let raw = self.text(t);
            raw[1..raw.len() - 1].replace("''", "'")
        } else {
            self.push_error(self.cur().span, "expected a category string");
            String::new()
        };
        let root_class = self.parse_class_decl();
        let end = self.cur().span;
        if !self.at(T::Eoi) {
            self.push_error(self.cur().span, "unexpected tokens after the top-level class");
        }
        Module { language_id, category, root_class, span: start.merge(end) }
    }

    pub(crate) fn parse_class_decl(&mut self) -> ClassDecl {
        let start = self.cur().span;
        let access = self.parse_access_modifier();
        self.expect(T::Class);
        let name = self.ident();
        let factory = if self.at(T::Equals) {
            None
        } else {
            Some(self.parse_message_pattern())
        };
        self.expect(T::Equals);
        let superclass = if self.at(T::LParen) { None } else { Some(self.parse_superclass_clause()) };
        self.expect(T::LParen);
        let class_comment = if self.at(T::String) {
            let t = self.bump();
            let raw = self.text(t);
            Some(raw[1..raw.len() - 1].replace("''", "'"))
        } else {
            None
        };
        let instance_side = self.parse_class_body(true);
        let inst_end = self.cur().span;
        self.expect(T::RParen);
        let class_side = if self.eat(T::Colon) {
            self.expect(T::LParen);
            // class-side comment allowed too
            if self.at(T::String) { self.bump(); }
            let body = self.parse_class_body(false);
            self.expect(T::RParen);
            Some(body)
        } else {
            None
        };
        let end = class_side.as_ref().map(|b| b.span).unwrap_or(inst_end);
        ClassDecl {
            access, name, factory, superclass, class_comment,
            instance_side, class_side, span: start.merge(end),
        }
    }

    pub(crate) fn parse_superclass_clause(&mut self) -> SuperclassClause {
        let start = self.cur().span;
        let prefix = match self.peek() {
            T::Identifier => match self.text(self.cur()) {
                "self" => { self.bump(); Some(SuperclassPrefix::SelfPrefix) }
                "super" => { self.bump(); Some(SuperclassPrefix::SuperPrefix) }
                "outer" => {
                    self.bump();
                    Some(SuperclassPrefix::Outer(self.ident()))
                }
                _ => None,
            },
            _ => None,
        };
        let receiver = self.ident();
        // trailing sends: parse a full expression starting from `receiver` and
        // decompose it into a chain of MessageSend. Simpler: re-run the message
        // parsers with `receiver` as the seed.
        let mut messages = Vec::new();
        self.parse_trailing_sends(&mut messages);
        let end = messages.last().map(|m| m.span).unwrap_or(receiver.span);
        SuperclassClause { prefix, receiver, messages, span: start.merge(end) }
    }

    fn parse_trailing_sends(&mut self, out: &mut Vec<MessageSend>) {
        // unary*, then optional binary*, then optional keyword — matches `message`
        while self.at(T::Identifier) && !self.is_reserved_ident(self.cur())
            && self.peek2() != T::Assign && self.peek() != T::Equals
        {
            let sel = self.bump();
            out.push(MessageSend {
                selector: Selector::Unary(self.text(sel).to_string()),
                args: Vec::new(),
                span: sel.span,
            });
        }
        while self.at(T::BinarySelector) {
            let op = self.bump();
            let arg = self.parse_unary_message();
            out.push(MessageSend {
                selector: Selector::Binary(self.text(op).to_string()),
                span: op.span.merge(super::expr::expr_span(&arg)),
                args: vec![arg],
            });
        }
        if self.at(T::Keyword) {
            let mut selector = String::new();
            let mut args = Vec::new();
            let start = self.cur().span;
            let mut end = start;
            while self.at(T::Keyword) {
                let kw = self.bump();
                selector.push_str(self.text(kw).trim_end_matches(':'));
                selector.push(':');
                let arg = self.parse_binary_message();
                end = super::expr::expr_span(&arg);
                args.push(arg);
            }
            out.push(MessageSend { selector: Selector::Keyword(selector), args, span: start.merge(end) });
        }
    }

    pub(crate) fn parse_class_body(&mut self, instance_side: bool) -> ClassBody {
        let start = self.cur().span;
        let slots = if self.at(T::Bar) || self.at(T::DoubleBar) {
            self.parse_slot_decls()
        } else {
            Vec::new()
        };
        let mut init_exprs = Vec::new();
        let mut methods = Vec::new();
        let mut nested_classes = Vec::new();
        let mut seen_decl = false;
        loop {
            match self.peek() {
                T::RParen | T::Eoi => break,
                T::Class | T::Public | T::Protected | T::Private => {
                    if self.peek() == T::Class || self.class_after_modifier() {
                        nested_classes.push(self.parse_class_decl());
                    } else {
                        methods.push(self.parse_method_decl());
                    }
                    seen_decl = true;
                }
                T::Dot => { self.bump(); }
                _ => {
                    if !seen_decl && self.looks_like_method() {
                        methods.push(self.parse_method_decl());
                        seen_decl = true;
                    } else if !seen_decl && instance_side {
                        let e = self.parse_expression();
                        init_exprs.push(Stmt::Expr(e));
                        self.eat(T::Dot);
                    } else if self.looks_like_method() {
                        methods.push(self.parse_method_decl());
                        seen_decl = true;
                    } else {
                        let t = self.bump();
                        self.push_error(t.span, "expected a method, nested class, or (before them) an init expression");
                    }
                }
            }
        }
        let end = self.cur().span;
        ClassBody { slots, init_exprs, methods, nested_classes, span: start.merge(end) }
    }

    fn class_after_modifier(&self) -> bool {
        self.peek2() == T::Class
    }

    fn looks_like_method(&mut self) -> bool {
        let cp = self.checkpoint();
        self.parse_access_modifier();
        let _ = self.try_parse_message_pattern();
        let is_method = self.at(T::Equals) && self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(T::LParen);
        self.restore(cp);
        is_method
    }

    pub(crate) fn checkpoint(&self) -> (usize, usize) {
        (self.pos, self.diags.len())
    }

    pub(crate) fn restore(&mut self, cp: (usize, usize)) {
        self.pos = cp.0;
        self.diags.truncate(cp.1);
    }

    fn try_parse_message_pattern(&mut self) -> Option<()> {
        match self.peek() {
            T::BinarySelector => { self.bump(); if self.at(T::Identifier) { self.bump(); } Some(()) }
            T::Keyword | T::SetterKeyword => {
                while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                    self.bump();
                    if self.at(T::Identifier) { self.bump(); } else { return None; }
                }
                Some(())
            }
            T::Identifier => { self.bump(); Some(()) }
            _ => None,
        }
    }
}
```

`lib.rs`:

```rust
pub use ast::Module;

/// Parse `src` into a `Module`. Returns `Err` on any error-severity diagnostic.
pub fn parse(src: &str) -> Result<ast::Module, Vec<Diagnostic>> {
    let tokens = match lex(src) {
        Ok(t) => t,
        Err(d) => return Err(d),
    };
    let mut p = parser::Parser::new(src, tokens);
    let module = p.parse_module();
    let diags = p.into_diagnostics();
    if diags.iter().any(|d| d.severity == Severity::Error) {
        Err(diags)
    } else {
        Ok(module)
    }
}
```

Add `Parser::into_diagnostics(self) -> Vec<Diagnostic>` and make `parse_module` / `Parser` reachable (`pub(crate)` is fine; `lib.rs` calls via `parser::Parser`).

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-syntax`
Expected: PASS (all suites).

- [x] **Step 5: Snapshot test a realistic module**

Add `crates/ns-syntax/tests/parse_snapshot.rs`:

```rust
#[test]
fn hello_world_ast() {
    let src = r#"Newspeak3
'HelloApp'
class Hello usingPlatform: platform = (
    | private Transcript = platform Transcript. |
    public main: args = ( Transcript show: 'Hello, World!' )
)"#;
    let module = ns_syntax::parse(src).expect("parse");
    insta::assert_debug_snapshot!(module);
}
```

Run: `cargo test -p ns-syntax --test parse_snapshot`, then `cargo insta accept` (or review `.snap.new` and rename). Commit the snapshot.

- [x] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(syntax): parse class declarations and add parse() API

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 13: CLI — `parse` and `parse-corpus`

**Files:**
- Modify: `crates/ns-cli/src/main.rs`

**Interfaces:**
- Consumes: `ns_syntax::{parse, Diagnostic, Severity}`.
- Produces the `ns-next` binary with:
  - `ns-next parse <FILE> [--ast]` — prints `OK <file>` on success, or each diagnostic as `<file>:<line>:<col>: error: <message>` and exits `1`. With `--ast`, pretty-prints the `Module` debug on success.
  - `ns-next parse-corpus <DIR>` — walks `<DIR>` recursively for `*.ns`, parses each, honors a `blacklist.toml` next to `<DIR>` (i.e. `<DIR>/../blacklist.toml`, or `--blacklist <PATH>`). Prints a summary `parsed N, failed M, expected-fail K` and exits `1` if any file not on the blacklist failed, or any blacklisted file parsed.
- Line/column: compute from the byte offset by counting `\n` up to `span.start`.

- [x] **Step 1: Write the failing test**

`crates/ns-cli/tests/cli.rs`:

```rust
use std::process::Command;

fn ns_next() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ns-next"))
}

#[test]
fn parse_ok_file() {
    let dir = tempdir();
    let f = dir.join("Ok.ns");
    std::fs::write(&f, "Newspeak3\n'X'\nclass C = ()\n").unwrap();
    let out = ns_next().arg("parse").arg(&f).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn parse_bad_file_exits_1() {
    let dir = tempdir();
    let f = dir.join("Bad.ns");
    std::fs::write(&f, "Newspeak3\n'X'\nclass C = (").unwrap();
    let out = ns_next().arg("parse").arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));
}

// minimal tempdir helper to avoid a dev-dependency
fn tempdir() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ns-cli-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&p);
    p
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p ns-cli`
Expected: FAIL — subcommands not implemented.

- [x] **Step 3: Write the implementation**

```rust
#![warn(clippy::all)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ns_syntax::{Diagnostic, Severity};

#[derive(Parser)]
#[command(name = "ns-next", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Parse a single .ns file.
    Parse {
        file: PathBuf,
        #[arg(long)]
        ast: bool,
    },
    /// Parse every .ns file under a directory.
    ParseCorpus {
        dir: PathBuf,
        #[arg(long)]
        blacklist: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Cmd::Parse { file, ast } => cmd_parse(&file, ast),
        Cmd::ParseCorpus { dir, blacklist } => cmd_parse_corpus(&dir, blacklist.as_deref()),
    }
}

fn cmd_parse(file: &Path, ast: bool) -> ExitCode {
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: error: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    match ns_syntax::parse(&src) {
        Ok(module) => {
            if ast {
                println!("{module:#?}");
            }
            println!("OK {}", file.display());
            ExitCode::SUCCESS
        }
        Err(diags) => {
            print_diagnostics(file, &src, &diags);
            ExitCode::FAILURE
        }
    }
}

fn cmd_parse_corpus(dir: &Path, blacklist: Option<&Path>) -> ExitCode {
    let bl_path = blacklist
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.parent().unwrap_or(dir).join("blacklist.toml"));
    let blacklist = load_blacklist(&bl_path);

    let mut files: Vec<PathBuf> = Vec::new();
    collect_ns_files(dir, &mut files);
    files.sort();

    let (mut parsed, mut unexpected_fail, mut expected_fail, mut stale) = (0, 0, 0, 0);
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(f).unwrap_or_default();
        let ok = ns_syntax::parse(&src).is_ok();
        let listed = blacklist.contains(&name);
        match (ok, listed) {
            (true, false) => parsed += 1,
            (true, true) => {
                stale += 1;
                eprintln!("STALE blacklist entry (now parses): {name}");
            }
            (false, true) => expected_fail += 1,
            (false, false) => {
                unexpected_fail += 1;
                eprintln!("FAIL {name}");
            }
        }
    }
    println!(
        "parsed {parsed}, failed {unexpected_fail}, expected-fail {expected_fail}, stale {stale}"
    );
    if unexpected_fail == 0 && stale == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn collect_ns_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ns_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ns") {
            out.push(path);
        }
    }
}

fn load_blacklist(path: &Path) -> std::collections::HashSet<String> {
    #[derive(serde::Deserialize, Default)]
    struct Bl {
        #[serde(default)]
        entry: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        file: String,
        #[allow(dead_code)]
        reason: String,
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let bl: Bl = toml::from_str(&text).unwrap_or_default();
    bl.entry.into_iter().map(|e| e.file).collect()
}

fn print_diagnostics(file: &Path, src: &str, diags: &[Diagnostic]) {
    for d in diags {
        let (line, col) = line_col(src, d.span.start);
        let level = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!("{}:{line}:{col}: {level}: {}", file.display(), d.message);
    }
}

fn line_col(src: &str, offset: u32) -> (usize, usize) {
    let offset = offset as usize;
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in src.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}
```

Add to `crates/ns-cli/Cargo.toml`:

```toml
toml.workspace = true
serde = { version = "1", features = ["derive"] }
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-cli`
Expected: PASS (2 tests).

- [x] **Step 5: Try it against the corpus (informational)**

Run: `cargo run -p ns-cli -- parse-corpus corpus/modules`
Expected: prints a summary. Many failures are expected at this point — Task 14 triages them.

- [x] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(cli): add parse and parse-corpus subcommands

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 14: Corpus parse test and blacklist triage

**Files:**
- Create: `crates/ns-syntax/tests/corpus.rs`
- Modify: `corpus/blacklist.toml`
- Modify: `crates/ns-syntax/src/*` (parser fixes discovered here)
- Modify: `docs/superpowers/specs/2026-09-01-ns-next-runtime-design.md` (record final corpus pass rate)

**Interfaces:**
- Consumes: `ns_syntax::parse`, `corpus/modules/*.ns`, `corpus/blacklist.toml`.
- Produces: `cargo test -p ns-syntax --test corpus` — one test that parses every corpus file and asserts:
  - every non-blacklisted file parses, and
  - every blacklisted file still fails (no stale entries).

- [x] **Step 1: Write the test**

`crates/ns-syntax/tests/corpus.rs`:

```rust
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(serde::Deserialize, Default)]
struct Blacklist {
    #[serde(default)]
    entry: Vec<Entry>,
}

#[derive(serde::Deserialize)]
struct Entry {
    file: String,
    reason: String,
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

#[test]
fn every_corpus_file_parses_or_is_blacklisted() {
    let corpus = corpus_dir();
    let bl_text = std::fs::read_to_string(corpus.join("blacklist.toml")).unwrap();
    let bl: Blacklist = toml::from_str(&bl_text).unwrap();
    let blacklisted: HashSet<&str> = bl.entry.iter().map(|e| e.file.as_str()).collect();
    assert!(bl.entry.iter().all(|e| !e.reason.trim().is_empty()), "blacklist entry missing a reason");

    let mut modules: Vec<PathBuf> = std::fs::read_dir(corpus.join("modules"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ns"))
        .collect();
    modules.sort();
    assert!(modules.len() > 50, "corpus looks empty: {}", modules.len());

    let mut unexpected_failures = Vec::new();
    let mut stale = Vec::new();

    for path in &modules {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(path).unwrap();
        let ok = ns_syntax::parse(&src).is_ok();
        match (ok, blacklisted.contains(name.as_str())) {
            (true, true) => stale.push(name),
            (false, false) => {
                let err = ns_syntax::parse(&src).unwrap_err();
                unexpected_failures.push(format!("{name}: {:?}", err.first()));
            }
            _ => {}
        }
    }

    assert!(
        unexpected_failures.is_empty() && stale.is_empty(),
        "\nunexpected parse failures ({}):\n{}\n\nstale blacklist entries ({}):\n{}\n",
        unexpected_failures.len(),
        unexpected_failures.join("\n"),
        stale.len(),
        stale.join("\n"),
    );
}
```

Add `serde` to `ns-syntax` dev-dependencies:

```toml
serde = { version = "1", features = ["derive"] }
```

- [x] **Step 2: Run the test**

Run: `cargo test -p ns-syntax --test corpus`
Expected: FAIL with a list of unparseable files.

- [x] **Step 3: Triage — iterate**

For each failure, decide:
- **Parser bug / missing common construct** → fix the parser (add a focused unit test in the relevant `parser` submodule first, then fix). Re-run.
- **Genuinely out-of-scope syntax** (e.g. a rare construct not in `NewspeakGrammar.ns`, or a file that is not actually Newspeak3) → add to `corpus/blacklist.toml`:

```toml
[[entry]]
file = "SomeExotic.ns"
reason = "uses <primitive: ...> pragma bodies with nested keyword messages; deferred to Milestone 2"
```

Repeat until the corpus test passes. Keep a running note of each parser fix in its commit message.

**Budget guidance:** aim for ≥90% of corpus files parsing. If a single construct blocks many files and is small, fix it; if it is large (full pragma-expression grammar, type annotations), blacklist the affected files with one shared reason and note it for Milestone 2.

- [x] **Step 4: Record the outcome**

Append to the spec, under "Reality Check":

```markdown
### Corpus parse result (Milestone 1)

As of <commit>, `cargo test -p ns-syntax --test corpus` parses N of M corpus
files (P%). Blacklisted files and reasons are in `corpus/blacklist.toml`.
The dominant deferred constructs are: <list>.
```

- [x] **Step 5: Full verification**

Run:
```
cargo fmt --all -- --check
cargo clippy --all-targets
cargo test --workspace
```
Expected: all green.

- [x] **Step 6: Commit**

```bash
git add -A
git commit -m "test(syntax): parse the full Newspeak corpus with a triaged blacklist

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage (ns-syntax + parse CLI portions):**

| Spec item | Task |
|---|---|
| `Span` byte offsets | 2 |
| `Token` / `TokenKind` | 3 |
| Lexer: identifiers, keywords, binary selectors, reserved words | 3 |
| Lexer: nested `(* *)` comments, `:literalmessage:` | 5 |
| Lexer: char constants, string escapes, radix numbers | 4 |
| AST (`Module`, `ClassDecl`, `SlotDecl`, `MethodDecl`, `MessagePattern`, `CodeBody`, `Stmt`, `Expr`, `Literal`) | 6 |
| Parser: recursive descent + Pratt precedence (unary > binary > keyword) | 8 |
| Parser: cascades, blocks, assignment, arrays/tuples, parenthesized | 8, 9 |
| Parser: message patterns, `codeBody`, temps, `^` return | 9, 10 |
| Parser: slots `\|..\|` / `\|\|..\|\|`, `=` / `::=` | 11 |
| Parser: class decls, `superclassClause`, nested classes, class side, `compilationUnit` | 12 |
| Public `lex()` / `parse()` returning `Result<_, Vec<Diagnostic>>` | 5, 12 |
| Diagnostics collected, no panic on bad input | 2, 7, all parser tasks |
| CLI `parse` (+ `--ast`), `parse-corpus` honoring `blacklist.toml` | 13 |
| Vendored corpus with `PROVENANCE.md`, manual snapshot + commit hash | 1 |
| Parser corpus test, blacklist with reasons | 14 |
| Grammar transcribed from `NewspeakGrammar.ns` | 3, 4, 5, 12 (grammar notes in each) |
| Classic terminology (`blacklist`) | Global Constraints, 1, 13, 14 |

Deferred to the runtime plan (correctly out of scope here): resolver, `Binding`, object model, interpreter, core library, `platform`, `ns-next run`, execution fixtures.

**2. Placeholder scan:** Task 7's `parse_expression` and `parse_block` are explicitly labelled temporary and are replaced in Tasks 8 and 9 respectively — each replacement is shown in full. No `TODO`/`TBD` remain. The `looks_like_method` / `checkpoint` speculative-parse mechanism is fully specified in Task 12.

**3. Type consistency:**
- `Selector` is `Unary(String) | Binary(String) | Keyword(String)` everywhere (Tasks 6, 8, 12).
- `MessageSend { selector, args, span }` used identically in `Send`, `Cascade`, `SuperclassClause` (Tasks 6, 8, 12).
- `Ident { name: String, span: Span }` throughout.
- `CodeBody { temps: Vec<Ident>, stmts: Vec<Stmt>, span }` produced by `parse_code_body` (Task 9), consumed by `parse_block` (Task 9) and `parse_method_decl` (Task 10).
- `parse_slot_decls` (Task 11) returns `Vec<SlotDecl>`, consumed by `parse_class_body` (Task 12).
- Lexer `#(` handling: Task 3 emits `Pound`+`LParen`, Task 9 formalizes and tests it, Task 12 relies on it via `parse_primary`. Consistent — noted the change in Task 9's interface block.
- `lex()` / `parse()` signatures match between Task 5, Task 12, and the CLI in Task 13.

One inconsistency fixed inline: Task 7 lists `parse_literal` handling `nil/true/false` "in the parser, not the lexer" — Task 8's `is_reserved_ident` and Task 12's `parse_superclass_clause` both re-check the same three words by text. That is intentional (no reserved-word token), and each site references the same list.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-01-ns-next-frontend.md`. Two execution options:

**1. Subagent-Driven (recommended)** — a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
