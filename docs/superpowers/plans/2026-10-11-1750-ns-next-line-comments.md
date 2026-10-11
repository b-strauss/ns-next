# Line comments — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A second comment form, `///` to the end of the line, beside the
specification's `(* … *)`; consecutive `///` lines form one comment whose
continuous text the syntax crate answers; and every Newspeak source of the
repository outside `corpus/` written with it.

**Architecture:** `ns-syntax` gains a `Comment` (`kind`, `span`, `text`) that
replaces the bare `Span` in `Lexed.comments` and `EditorParse.comments`; the
lexer skips `///` as trivia, merges consecutive line comments into one
`Comment`, and ends a binary selector before `///`. The JetBrains lexer
mirrors it token by token, with a token type of its own for a line comment.
The folding, the class comment and the core library's documentation test
read the new `Comment`. A one-off script outside the repository rewrites
every `(* … *)` outside `corpus/` as `///` lines, verified by comparing the
token stream before and after; a test then refuses a block comment in any
such source but the one golden that pins it.

**Tech Stack:** Rust 1.98.0 (`crates/ns-syntax`, `crates/ns-analyzer`,
`crates/ns-runtime` tests, `crates/ns-cli` tests), Kotlin
(`editors/jetbrains`), Newspeak (`crates/ns-runtime/core-library/*.ns`,
`goldens/**/*.ns`, `benchmarks/are-we-fast-yet/*.ns`), Python 3 for the
one-off conversion (kept in `.temp/`, never committed).

**Spec:** none on file. The design was agreed in conversation with the user
on 2026-10-11 and is recorded here in full ("The design" below); the living
documents receive it in Tasks 2 and 5.

## The design

Decided with the user, 2026-10-11:

1. **`///` starts a line comment wherever it stands outside a string or a
   comment**, and the comment runs up to, not including, the next `\n` or
   `\r` (or the end of the source). `////…` is a comment too.
2. **A binary selector ends before `///`.** `a +/// x` is `a +` followed by
   a comment; `#+/// x` is the symbol `#+`; `#/// x` is a `#` with nothing
   after it (the existing "expected symbol after `#`" diagnostic). `//` is
   still a binary selector. This is a deviation from §4.2 (`binSel →
   (specialCharacter | "-") specialCharacter*`), which admits `///`,
   `+///`, `<///>` and every other run containing `///`; the user accepts
   it, and it is recorded in `docs/design/README.md`'s deviation table.
3. **The two forms do not see each other.** Inside `(* … *)`, `///` is
   text; inside a `///` comment, `(*` opens nothing. Inside `'…'` and
   `"…"` both are characters.
4. **One `Comment` per block.** `Lexed.comments` and `EditorParse.comments`
   become `Vec<Comment>`: `kind` (`CommentKind::Block` for `(* … *)`,
   `CommentKind::Line` for `///`), `span` (first to last comment
   character) and `text(source) -> String`. Two `///` comments are one
   block when each is the first thing on its line and only whitespace with
   exactly one line break lies between them; a blank line, code, or a
   `///` that follows code on its line starts a new block. A `(* … *)` is
   always a block of its own.
5. **`Comment::text`** answers the content without comment markers, for a
   later hover to render. `Line`: per line, the indentation, `///` and
   one following space are removed, trailing whitespace is trimmed, the
   lines are joined with `\n`. `Block`: the inside of `(*` and `*)` (an
   unterminated one: everything after `(*`), its first line trimmed, the
   following lines stripped of their common indentation and of trailing
   whitespace, leading and trailing blank lines dropped, joined with `\n`.
   It renders nothing (no Markdown); that is the hover's business.
6. **The house style is `///`** for every Newspeak source of the
   repository outside `corpus/`: the core library, the goldens and the
   benchmarks. Existing `(* … *)` comments there are rewritten. The core
   library's documentation test requires a `Line` comment above every
   member; `crates/ns-cli/tests/source_formatting.rs` refuses a `Block`
   comment in any `.ns` outside `corpus/` except
   `goldens/language/syntax/line_comment.ns`, the golden that pins both
   forms side by side. Newspeak modules inline in Rust tests are rewritten
   where the comment is incidental; tests about `(* … *)` itself keep it.
7. **Editor:** folding of a multi-line `///` block falls out of its single
   span (preview `/// first line ...`); the JetBrains `Commenter` gains the
   line-comment prefix `///`, so the comment-line action writes one; TODO
   markers are found in both forms. The class comment (`ClassDeclaration::class_comment`)
   is also taken from a `///` block standing first in a class header.
8. **A hover showing `Comment::text`** is not built here; it goes into
   `docs/OPEN-WORK.md`.

## Global Constraints

- Rust 1.98.0, edition 2021; no new external dependency.
- `crates/ns-syntax/src/lexer.rs` and `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexer.kt` change in the **same commit** (CLAUDE.md, "Working in this repository").
- Names are written out — no identifier of one or two characters and no truncated word, in Rust, Kotlin **and the Python script** (CLAUDE.md, "Conventions"; clippy's `min_ident_chars`).
- Comments explain the decision, not the mechanics; English only.
- `corpus/` is never touched; every corpus module still parses (`parse-corpus`: "parsed 211, failed 0").
- No golden's `.toml` changes: no golden prints a source position, so rewriting comments cannot change an outcome. A golden that fails after a rewrite is a defect of the rewrite.
- `.ns` sources outside `corpus/` are indented with four spaces, never tabs.
- `crates/ns-runtime/core-library/**` is `-text` in `.gitattributes`: keep it byte-for-byte LF, no CRLF.
- Before every commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (warning-free), `cargo nextest run --workspace`, `cargo test --workspace --doc`, `cargo run -p ns-analyzer -- parse-corpus corpus/modules`; and, because `crates/ns-syntax` and `editors/jetbrains` change, `./editors/jetbrains/gradlew -p editors/jetbrains check` in every commit that touches either.
- The end-of-batch gates (the five stress modes, `run-corpus` plain and under stress, the benchmarks profile, `verifyPlugin`) run once, in Task 6, through the `finishing-a-batch` skill. Commits before that are work in progress.
- Subagents run every command in the foreground.
- Commit messages end with the attribution lines the session's system reminder gives.

## Review Focus

1. **A selector against `///` in the token-level arms that decide by the next byte** — `|///`, `=///`, `<///`, `>///` must answer `Bar`, `Equals`, `LessThan`, `GreaterThan` and then a comment, not a one-character `BinarySelector`: `lex_operator`'s "run of two or more" test and the `|` arm look one byte ahead. Pinned in Task 1, Step 1 (`a_line_comment_ends_every_selector_before_it`) and mirrored in Task 2.
2. **A trailing `///` after code followed by a `///` line** must stay two comments, or a method's documentation absorbs the previous line's remark. Pinned in Task 1 (`a_trailing_line_comment_starts_no_block_with_the_next_line`).
3. **CRLF sources** — a `\r\n` between two `///` lines is one line break, and `text` contains no `\r`. Pinned in Task 1 (`a_crlf_line_comment_block_is_one_comment` and the `text` test).
4. **The rewrite moving code** — a comment with code after it on its line is lifted above the line; if that line starts inside a multi-line string literal, the inserted lines would land inside the string. The token comparison of Tasks 3 and 4 (kind and length of every token, before and after) catches it; such a site is fixed by hand.
5. **The rewrite leaving `(*` that the next pass finds again** — a nested comment becomes text inside `///` lines; the scanner skips `///` lines, so it is not re-converted. Pinned by the script's own fixture run in Task 3, Step 2.
6. **A slot section's documentation after its bar** — `| (* doc *)` with the slot on the next line must become the bar, then the `///` lines directly above the slot, or the documentation test finds the bar between comment and slot. Pinned by the fixture (Task 3, Step 2) and by the documentation test after Task 3.

---

### Task 1: `Comment`, the `///` lexer rule, and the Rust readers of comments

**Files:**
- Create: `crates/ns-syntax/src/comment.rs`
- Modify: `crates/ns-syntax/src/lib.rs` (module, re-export, `EditorParse.comments`)
- Modify: `crates/ns-syntax/src/lexer.rs:7-23` (`Lexer.comments`, `Lexed.comments`), `:67-107` (`skip_trivia`, `skip_block_comment`), `:149-163` (`|` arm), `:267-308` (`lex_operator`), `:426-431` (symbol arm), tests from `:486`
- Modify: `crates/ns-syntax/src/parser/declaration.rs:935-953` (`take_class_comment`) and its tests near `:2142`
- Modify: `crates/ns-syntax/tests/recovery.rs` (new test beside `a_clean_source_parses_as_parse_parses_it`)
- Modify: `crates/ns-analyzer/src/folding.rs:20-75` (`folds`, `comment_preview`) and its tests
- Modify: `crates/ns-runtime/tests/core_library_documentation.rs:39-46` (`comment_spans`)

**Interfaces:**
- Produces (Task 2 onwards rely on these names):
  - `ns_syntax::comment::{Comment, CommentKind}`, re-exported as `ns_syntax::{Comment, CommentKind}`.
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum CommentKind { Block, Line }`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Comment { pub kind: CommentKind, pub span: Span }`
  - `impl Comment { pub fn text(self, source: &str) -> String }`
  - `ns_syntax::lexer::Lexed.comments: Vec<Comment>`; `ns_syntax::EditorParse.comments: Vec<Comment>`
  - `ns_analyzer::folding::folds(module: &ast::Module, comments: &[Comment], text: &str) -> Vec<Fold>`

**Commit:** none. Task 2 commits Tasks 1 and 2 together, because a change to `lexer.rs` and its Kotlin mirror land in one commit.

- [ ] **Step 1: Write the failing lexer tests**

In `crates/ns-syntax/src/lexer.rs`'s `mod tests`, replace `line_and_block_comments_are_trivia` (its name promised a line comment it never had) and add the tests below. `kinds` is the module's existing helper; add `comments_of` beside it.

```rust
    /// Every comment of `source` as its kind and the text its span covers.
    fn comments_of(source: &str) -> Vec<(CommentKind, &str)> {
        Lexer::new(source)
            .run_keeping_comments()
            .comments
            .iter()
            .map(|comment| (comment.kind, comment.span.text(source)))
            .collect()
    }

    #[test]
    fn line_and_block_comments_are_trivia() {
        assert_eq!(
            kinds("a (* comment *) b"),
            vec![Identifier, Identifier, EndOfInput]
        );
        assert_eq!(
            kinds("a /// comment\nb"),
            vec![Identifier, Identifier, EndOfInput]
        );
        assert_eq!(kinds("a /// comment"), vec![Identifier, EndOfInput]);
        assert_eq!(kinds("x///y"), vec![Identifier, EndOfInput]);
        assert_eq!(kinds("a //// four\nb"), vec![Identifier, Identifier, EndOfInput]);
    }

    /// §4.2's `binSel` would take `+///` whole; ns-next ends every selector
    /// before `///`, the deviation `docs/design/README.md` records.
    #[test]
    fn a_line_comment_ends_every_selector_before_it() {
        let texts = |source: &'static str| -> Vec<(crate::token::TokenKind, &'static str)> {
            let (tokens, diagnostics) = Lexer::new(source).run();
            assert!(diagnostics.is_empty(), "{source}: {diagnostics:?}");
            tokens
                .into_iter()
                .map(|token| (token.kind, token.span.text(source)))
                .collect()
        };
        assert_eq!(
            texts("a +/// x\nb"),
            vec![(Identifier, "a"), (BinarySelector, "+"), (Identifier, "b"), (EndOfInput, "")]
        );
        assert_eq!(
            texts("a // b"),
            vec![(Identifier, "a"), (BinarySelector, "//"), (Identifier, "b"), (EndOfInput, "")]
        );
        assert_eq!(texts("#+/// x"), vec![(Symbol, "#+"), (EndOfInput, "")]);
        assert_eq!(
            texts("| a |/// x"),
            vec![(Bar, "|"), (Identifier, "a"), (Bar, "|"), (EndOfInput, "")]
        );
        assert_eq!(texts("a =/// x"), vec![(Identifier, "a"), (Equals, "="), (EndOfInput, "")]);
        assert_eq!(texts("a </// x"), vec![(Identifier, "a"), (LessThan, "<"), (EndOfInput, "")]);
        assert_eq!(texts("a >/// x"), vec![(Identifier, "a"), (GreaterThan, ">"), (EndOfInput, "")]);
        assert_eq!(texts("a -/// x"), vec![(Identifier, "a"), (BinarySelector, "-"), (EndOfInput, "")]);
    }

    #[test]
    fn a_hash_before_a_line_comment_names_no_symbol() {
        let (_, diagnostics) = Lexer::new("#/// x").run();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].message, "expected symbol after `#`");
    }

    /// Neither form sees the other, and a string sees neither.
    #[test]
    fn the_two_comment_forms_do_not_see_each_other() {
        assert_eq!(
            comments_of("(* a /// b *) c /// d (* e\nf"),
            [(CommentKind::Block, "(* a /// b *)"), (CommentKind::Line, "/// d (* e")]
        );
        assert_eq!(kinds("'///' \"///\" '(*'"), vec![String, String, String, EndOfInput]);
    }

    #[test]
    fn consecutive_line_comments_are_one_block() {
        let source = "    /// one\n    /// two\n    run";
        assert_eq!(
            comments_of(source),
            [(CommentKind::Line, "/// one\n    /// two")]
        );
    }

    #[test]
    fn a_blank_line_or_code_starts_a_new_block() {
        assert_eq!(
            comments_of("/// one\n\n/// two"),
            [(CommentKind::Line, "/// one"), (CommentKind::Line, "/// two")]
        );
        assert_eq!(
            comments_of("/// one\nx\n/// two"),
            [(CommentKind::Line, "/// one"), (CommentKind::Line, "/// two")]
        );
        assert_eq!(
            comments_of("(* one *)\n/// two"),
            [(CommentKind::Block, "(* one *)"), (CommentKind::Line, "/// two")]
        );
    }

    #[test]
    fn a_trailing_line_comment_starts_no_block_with_the_next_line() {
        assert_eq!(
            comments_of("x foo. /// remark\n/// documentation\nbar"),
            [(CommentKind::Line, "/// remark"), (CommentKind::Line, "/// documentation")]
        );
    }

    #[test]
    fn a_crlf_line_comment_block_is_one_comment() {
        assert_eq!(
            comments_of("/// one\r\n/// two\r\nx"),
            [(CommentKind::Line, "/// one\r\n/// two")]
        );
    }
```

Also update `comment_spans_are_kept_on_request` to the new element type (its assertions stay):

```rust
        let texts: Vec<&str> = lexed
            .comments
            .iter()
            .map(|comment| comment.span.text(source))
            .collect();
```

and add `use crate::comment::CommentKind;` to the test module's imports.

- [ ] **Step 2: Write the failing `Comment::text` tests**

Create `crates/ns-syntax/src/comment.rs` with only the tests and the type stubs needed to compile them, so the tests fail on behaviour, not on a missing name:

```rust
//! A comment as an editor reads it: its form, its span, and its text
//! without comment markers (`docs/design/frontend.md`, "Comments").

use crate::span::Span;

/// Which of the two comment forms a comment is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// §4.2's `(* … *)`, nested ones inside it.
    Block,
    /// ns-next's `///` lines, consecutive ones merged into one comment.
    Line,
}

/// One comment of a source, in source order among its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment {
    pub kind: CommentKind,
    /// From the first comment character to the last: `(*` to `*)`, or the
    /// first line's `///` to the end of the last line's text.
    pub span: Span,
}

impl Comment {
    /// The comment's text without its markers, its lines joined with `\n`.
    pub fn text(self, source: &str) -> String {
        let _ = source;
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(kind: CommentKind, source: &str) -> String {
        Comment {
            kind,
            span: Span::new(0, source.len() as u32),
        }
        .text(source)
    }

    #[test]
    fn a_line_block_loses_its_markers_and_one_space() {
        let source = "/// First line.\n    ///   indented\n    ///\n    /// last  ";
        assert_eq!(whole(CommentKind::Line, source), "First line.\n  indented\n\nlast");
    }

    #[test]
    fn a_fourth_slash_is_text() {
        assert_eq!(whole(CommentKind::Line, "////x"), "/x");
        assert_eq!(whole(CommentKind::Line, "///x"), "x");
    }

    #[test]
    fn a_crlf_line_block_has_no_carriage_return() {
        assert_eq!(whole(CommentKind::Line, "/// one\r\n/// two"), "one\ntwo");
    }

    #[test]
    fn a_block_loses_its_markers_and_common_indentation() {
        assert_eq!(
            whole(CommentKind::Block, "(* A module\n   with a header. *)"),
            "A module\nwith a header."
        );
        assert_eq!(
            whole(CommentKind::Block, "(*\n    one\n      two\n*)"),
            "one\n  two"
        );
        assert_eq!(whole(CommentKind::Block, "(* a (* b *) c *)"), "a (* b *) c");
        assert_eq!(whole(CommentKind::Block, "(**)"), "");
    }

    #[test]
    fn an_unterminated_block_keeps_everything_after_its_opening() {
        assert_eq!(whole(CommentKind::Block, "(* open\nstill"), "open\nstill");
    }
}
```

In `crates/ns-syntax/src/lib.rs` add `pub mod comment;` beside the other modules and `pub use comment::{Comment, CommentKind};` beside the other re-exports.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo nextest run -p ns-syntax`
Expected: compile errors in `lexer.rs`'s tests (`comment.kind` on a `Span`) — the lexer still answers spans. That is the failure Step 4 fixes; the `comment.rs` tests compile and fail (`text` answers `""`).

- [ ] **Step 4: Implement `Comment::text`**

Replace the stub body in `comment.rs`:

```rust
impl Comment {
    /// The comment's text without its markers, its lines joined with `\n`,
    /// for a reader that shows a comment rather than the source: a `Line`
    /// block without each line's indentation, `///` and one space after
    /// it; a `Block` without `(*` and `*)` and without the indentation its
    /// lines share. Rendering it is the reader's business.
    pub fn text(self, source: &str) -> String {
        let raw = self.span.text(source);
        match self.kind {
            CommentKind::Line => line_text(raw),
            CommentKind::Block => block_text(raw),
        }
    }
}

/// `text` with each of `\r\n`, `\r` and `\n` read as one line break, so a
/// source's line endings never reach the text.
fn lines_of(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect()
}

fn line_text(raw: &str) -> String {
    lines_of(raw)
        .iter()
        .map(|line| {
            let line = line.trim_start();
            let line = line.strip_prefix("///").unwrap_or(line);
            line.strip_prefix(' ').unwrap_or(line).trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn block_text(raw: &str) -> String {
    let inner = raw.strip_prefix("(*").unwrap_or(raw);
    let inner = inner.strip_suffix("*)").unwrap_or(inner);
    let lines = lines_of(inner);
    let Some((first, rest)) = lines.split_first() else {
        return String::new();
    };
    // The continuation lines are usually indented to sit under the first
    // line's text; what they share is layout, not content.
    let shared_indentation = rest
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    let mut text: Vec<String> = Vec::with_capacity(lines.len());
    text.push(first.trim().to_string());
    for line in rest {
        if line.trim().is_empty() {
            text.push(String::new());
        } else {
            text.push(line[shared_indentation..].trim_end().to_string());
        }
    }
    while text.first().is_some_and(String::is_empty) {
        text.remove(0);
    }
    while text.last().is_some_and(String::is_empty) {
        text.pop();
    }
    text.join("\n")
}

/// Whether a `///` comment starting at `start` continues the line-comment
/// block `previous`: both start their lines and only whitespace with one
/// line break lies between them. A `///` after code is a remark on that
/// line, so it neither joins the block above nor takes the next line in.
pub(crate) fn continues_line_block(source: &str, previous: Span, start: usize) -> bool {
    let gap = &source[previous.end as usize..start];
    let line_breaks =
        gap.matches('\n').count() + gap.matches('\r').count() - gap.matches("\r\n").count();
    line_breaks == 1
        && gap.bytes().all(|byte| byte.is_ascii_whitespace())
        && starts_its_line(source, previous.start as usize)
}

/// Whether only whitespace stands between the start of `offset`'s line and
/// `offset`.
fn starts_its_line(source: &str, offset: usize) -> bool {
    source[..offset]
        .bytes()
        .rev()
        .take_while(|&byte| byte != b'\n' && byte != b'\r')
        .all(|byte| byte.is_ascii_whitespace())
}
```

- [ ] **Step 5: Implement the lexer rule**

In `crates/ns-syntax/src/lexer.rs`:

1. Imports: `use crate::comment::{continues_line_block, Comment, CommentKind};`. `Lexer.comments` and `Lexed.comments` become `Vec<Comment>`; update `Lexed`'s doc comment to "A source's tokens with its comments kept, for an editor".
2. `skip_trivia`:

```rust
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(byte) if byte.is_ascii_whitespace() => self.position += 1,
                Some(b'(') if self.peek_at(1) == Some(b'*') => self.skip_block_comment(),
                Some(b'/') if self.starts_line_comment(0) => self.skip_line_comment(),
                _ => break,
            }
        }
    }
```

3. In `skip_block_comment`, both pushes become `self.comments.push(Comment { kind: CommentKind::Block, span: Span::new(start as u32, self.position as u32) });`.
4. New methods after `skip_block_comment`:

```rust
    /// Whether ns-next's `///` line comment starts `offset` bytes ahead.
    /// It is a deviation from §4.2, whose `binSel` admits `///` and every
    /// run of special characters containing it; a comment wherever it
    /// stands is what a reader expects (`docs/design/README.md`,
    /// "Deviations from the language specification").
    fn starts_line_comment(&self, offset: usize) -> bool {
        let start = self.position + offset;
        self.bytes.get(start..start + 3) == Some(b"///".as_slice())
    }

    /// Whether the byte `offset` ahead continues a binary selector: a
    /// special character that does not start a line comment.
    fn continues_binary_selector(&self, offset: usize) -> bool {
        self.peek_at(offset)
            .is_some_and(|byte| SPECIAL.contains(&byte))
            && !self.starts_line_comment(offset)
    }

    /// A `///` comment, to the end of its line. It joins the comment before
    /// it when the two form one block (`continues_line_block`), so a reader
    /// gets one comment, with one text, per block.
    fn skip_line_comment(&mut self) {
        let start = self.position;
        while matches!(self.peek(), Some(byte) if byte != b'\n' && byte != b'\r') {
            self.position += 1;
        }
        let span = Span::new(start as u32, self.position as u32);
        if let Some(previous) = self.comments.last_mut() {
            if previous.kind == CommentKind::Line
                && continues_line_block(self.source, previous.span, start)
            {
                previous.span = previous.span.merge(span);
                return;
            }
        }
        self.comments.push(Comment {
            kind: CommentKind::Line,
            span,
        });
    }
```

5. The `|` arm in `lex_one`: replace the condition `self.peek_at(1).is_some_and(|next_byte| SPECIAL.contains(&next_byte))` with `self.continues_binary_selector(1)`.
6. `lex_operator`: `let is_run_of_two_plus = self.continues_binary_selector(1);` and the loop becomes `while self.continues_binary_selector(0) { self.position += 1; }`.
7. The symbol arm (`Some(byte) if starts_binary_selector(byte) =>`, around line 426): guard `Some(byte) if starts_binary_selector(byte) && !self.starts_line_comment(0) =>`, loop `while self.continues_binary_selector(0) { self.position += 1; }`.

- [ ] **Step 6: Run the syntax crate's tests**

Run: `cargo nextest run -p ns-syntax`
Expected: the new lexer and `comment.rs` tests pass. Compile errors remain in `lib.rs` (`EditorParse.comments`) — fix them in Step 7 first if the crate does not build.

- [ ] **Step 7: `EditorParse`, the class comment, and their tests**

`crates/ns-syntax/src/lib.rs`: `EditorParse.comments: Vec<Comment>`, doc comment "Every comment, in source order, consecutive `///` lines as one." Nothing else changes there (`lexed.comments` moves across as before).

In `crates/ns-syntax/tests/recovery.rs`, after `a_clean_source_parses_as_parse_parses_it`:

```rust
/// A block of `///` lines reaches the editor as one comment with its text.
#[test]
fn a_line_comment_block_is_one_comment_with_its_text() {
    let source = indoc! {"
        /// A module,
        /// documented.
        class Module = () (
            run = ( ^1 )
        ) : ()
    "};
    let parsed = ns_syntax::parse_for_editor(source);
    assert!(parsed.diagnostics.is_empty());
    let [comment] = parsed.comments.as_slice() else {
        panic!("one comment, not {}", parsed.comments.len());
    };
    assert_eq!(comment.kind, ns_syntax::CommentKind::Line);
    assert_eq!(comment.text(source), "A module,\ndocumented.");
}
```

In `crates/ns-syntax/src/parser/declaration.rs`'s tests, after `block_comment_in_header_is_still_the_class_comment`:

```rust
    // A `///` block standing first in the header is the class comment too,
    // as its text: the markers are syntax, not content.
    #[test]
    fn line_comment_block_in_header_is_the_class_comment() {
        let module = module(indoc! {"
            class C = (
                /// a comment
                /// on two lines
            ) ()
        "});
        assert_eq!(
            module.root_class.class_comment.as_deref(),
            Some("a comment\non two lines")
        );
    }
```

and `take_class_comment` becomes (doc comment extended by one paragraph):

```rust
    /// … (existing paragraphs) …
    ///
    /// A block of ns-next's `///` lines in the same place is the class
    /// comment too, as `Comment::text` answers it.
    fn take_class_comment(&mut self) -> Option<String> {
        let previous_end = self.tokens.get(self.position.checked_sub(1)?)?.span.end as usize;
        let current_start = self.current().span.start as usize;
        let gap = self.source.get(previous_end..current_start)?;
        let trimmed = gap.trim();
        if let Some(inner) = trimmed
            .strip_prefix("(*")
            .and_then(|rest| rest.strip_suffix("*)"))
        {
            return Some(inner.to_string());
        }
        let lexed = crate::lexer::Lexer::new(gap).run_keeping_comments();
        match lexed.comments.as_slice() {
            // The gap holds no token but the end of input: the block is all
            // that stands there.
            [comment] if comment.kind == crate::comment::CommentKind::Line && lexed.tokens.len() == 1 => {
                Some(comment.text(gap))
            }
            _ => None,
        }
    }
```

- [ ] **Step 8: Folding**

`crates/ns-analyzer/src/folding.rs`: import `ns_syntax::{Comment, CommentKind, Span}`; `folds` takes `comments: &[Comment]` and pushes `collector.push(comment.span, comment_preview(*comment, text), true);`. Replace `comment_preview`:

```rust
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
        .map(|line| line.chars().take(COMMENT_PREVIEW_CHARACTERS).collect::<String>());
    match (comment.kind, first) {
        (CommentKind::Block, Some(line)) => format!("(* {line} ...*)"),
        (CommentKind::Block, None) => "(*...*)".to_string(),
        (CommentKind::Line, Some(line)) => format!("/// {line} ..."),
        (CommentKind::Line, None) => "///...".to_string(),
    }
}
```

Add the test beside `a_comment_folds_to_its_first_line`:

```rust
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
```

`crates/ns-analyzer/src/analysis.rs` needs no change (it passes `&comments` through); fix whatever else the compiler names.

- [ ] **Step 9: The documentation test reads `Comment`**

`crates/ns-runtime/tests/core_library_documentation.rs`'s `comment_spans` becomes, unchanged in what it accepts (both kinds; Task 3 narrows it):

```rust
/// Every comment of `source`, as a byte range.
fn comment_spans(source: &str) -> Vec<(usize, usize)> {
    ns_syntax::lexer::Lexer::new(source)
        .run_keeping_comments()
        .comments
        .iter()
        .map(|comment| (comment.span.start as usize, comment.span.end as usize))
        .collect()
}
```

- [ ] **Step 10: Run the workspace**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets && cargo nextest run --workspace && cargo test --workspace --doc && cargo run -p ns-analyzer -- parse-corpus corpus/modules`
Expected: warning-free, all green, "parsed 211, failed 0". Do not commit (Task 2 does).

---

### Task 2: The JetBrains lexer, the commenter, the golden and the language's documents — commit

**Files:**
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexer.kt` (`lexOne`, `lexOperator`, `lexSymbol`, new helpers)
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakTokenTypes.kt:47-48` (new `LINE_COMMENT`)
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/parser/NewspeakParserDefinition.kt:44` (`COMMENTS`)
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/highlighting/NewspeakSyntaxHighlighter.kt:42`
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/editor/NewspeakCommenter.kt`, `NewspeakIndexPatternBuilder.kt`
- Modify: every other `main` file that handles `NewspeakTokenTypes.COMMENT` (find them: `grep -rn "COMMENT" editors/jetbrains/src/main --include=*.kt`)
- Test: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/lexer/NewspeakLexerTest.kt`, `LexerComparisonTest.kt:96-97`, `NewspeakEditorTest.kt`
- Create: `goldens/language/syntax/line_comment.ns`, `goldens/language/syntax/line_comment.toml`
- Modify: `docs/design/frontend.md` ("Grammar" list at `:184`, "Lexer" at `:189` and `:257-263`, "Recovery" at `:482`), `docs/design/README.md` (map row at `:80`, deviation table at `:402-414`), `docs/design/editor-support.md` (`:72`, the folding table at `:319`, `:746`, `:771`)

**Interfaces:**
- Consumes: Task 1's rule (design points 1–4); the Kotlin lexer mirrors `starts_line_comment`, `continues_binary_selector`, `skip_line_comment` by name in its comments.
- Produces: `NewspeakTokenTypes.LINE_COMMENT` (a `NewspeakTokenType("LineComment", null)`), part of `NewspeakParserDefinition.COMMENTS`.

- [ ] **Step 1: Write the failing Kotlin tests**

`NewspeakLexerTest.kt` — import `NewspeakTokenTypes.LINE_COMMENT`, `GREATER_THAN` is already imported; add after `anUnterminatedCommentRunsToTheEnd`:

```kotlin
    @Test fun aLineCommentRunsToTheEndOfItsLine() {
        assertEquals(listOf(IDENTIFIER, LINE_COMMENT, IDENTIFIER), kinds("a /// comment (* opens nothing\nb"))
        assertEquals(LINE_COMMENT to "/// comment", lexAll("a /// comment\nb")[2])
        assertEquals(listOf(IDENTIFIER, LINE_COMMENT), kinds("x///y"))
        assertEquals(listOf(IDENTIFIER, LINE_COMMENT, LINE_COMMENT, IDENTIFIER), kinds("a /// one\n/// two\nb"))
        assertEquals(listOf(IDENTIFIER, COMMENT, IDENTIFIER), kinds("a (* x /// y *) b"))
        assertEquals(listOf(STRING, STRING), kinds("'///' \"///\""))
    }

    /** `a_line_comment_ends_every_selector_before_it` in `lexer.rs`. */
    @Test fun aLineCommentEndsEverySelectorBeforeIt() {
        assertEquals(listOf(IDENTIFIER to "a", BINARY_SELECTOR to "+"), significant("a +/// x"))
        assertEquals(listOf(IDENTIFIER to "a", BINARY_SELECTOR to "//", IDENTIFIER to "b"), significant("a // b"))
        assertEquals(listOf(SYMBOL to "#+"), significant("#+/// x"))
        assertEquals(listOf(SYMBOL to "#"), significant("#/// x"))
        assertEquals(listOf(BAR to "|", IDENTIFIER to "a", BAR to "|"), significant("| a |/// x"))
        assertEquals(listOf(IDENTIFIER to "a", EQUALS to "="), significant("a =/// x"))
        assertEquals(listOf(IDENTIFIER to "a", LESS_THAN to "<"), significant("a </// x"))
        assertEquals(listOf(IDENTIFIER to "a", GREATER_THAN to ">"), significant("a >/// x"))
    }
```

with a helper beside `lexAll` (whitespace and both comment types dropped):

```kotlin
/** The tokens of [text] the Rust lexer also emits, as (type, text). */
fun significant(text: String): List<Pair<IElementType, String>> =
    lexAll(text).filter { (type, _) -> type != TokenType.WHITE_SPACE && type != COMMENT && type != LINE_COMMENT }
```

Read how the file's existing `kinds` filters before adding `significant`; if `kinds` already drops whitespace only, keep that.

`NewspeakEditorTest.kt`, after `testBlockCommentWrapsTheSelection`:

```kotlin
    /** The comment-line action writes ns-next's `///`. */
    fun testLineCommentPrefixesTheLine() {
        myFixture.configureByText("Module.ns", "<caret>x\ny")
        myFixture.performEditorAction(IdeActions.ACTION_COMMENT_LINE)
        myFixture.checkResult("///x\ny")
    }

    fun testTodoInALineCommentIsFound() {
        myFixture.configureByText("Module.ns", "/// TODO: finish\nx")
        assertEquals(1, PsiTodoSearchHelper.getInstance(project).findTodoItems(myFixture.file).size)
    }
```

If the platform's default for a custom language inserts a space after the prefix, the expected text is `"/// x\ny"`; pin what the platform does (`Comment::text` reads both the same).

- [ ] **Step 2: Run them to see them fail**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests "*NewspeakLexerTest*" --tests "*NewspeakEditorTest*"`
Expected: compile failure on `LINE_COMMENT`.

- [ ] **Step 3: Implement the Kotlin side**

`NewspeakTokenTypes.kt`, beside `COMMENT` (whose doc comment stays):

```kotlin
    /** A `///` comment, one line of it; consecutive ones are separate tokens here. The Rust lexer skips comments. */
    @JvmField val LINE_COMMENT = NewspeakTokenType("LineComment", null)
```

`NewspeakLexer.kt`:

```kotlin
        if (character == '/' && startsLineComment(0)) {
            lexLineComment()
            return Types.LINE_COMMENT
        }
```

placed right after the `(*` check in `lexOne`; then

```kotlin
    /** `starts_line_comment`: ns-next's `///`, a deviation from §4.2's `binSel`. */
    private fun startsLineComment(offset: Int): Boolean =
        peek(offset) == '/' && peek(offset + 1) == '/' && peek(offset + 2) == '/'

    /** `continues_binary_selector`: a special character that starts no line comment. */
    private fun continuesBinarySelector(offset: Int): Boolean =
        peek(offset)?.let { next -> next in SPECIAL } == true && !startsLineComment(offset)

    /** `skip_line_comment`, one line per token: the plugin merges nothing. */
    private fun lexLineComment() {
        while (peek()?.let { next -> next != '\n' && next != '\r' } == true) position += 1
    }
```

- `|` arm: `continuesBinarySelector(1) -> lexOperator()`.
- `lexOperator`: `val runOfTwoPlus = continuesBinarySelector(1)`; loop `while (continuesBinarySelector(0)) position += 1`.
- `lexSymbol`: guard `next != null && startsBinarySelector(next) && !startsLineComment(0) ->`; loop `while (continuesBinarySelector(0)) position += 1`.

`NewspeakParserDefinition.COMMENTS = TokenSet.create(NewspeakTokenTypes.COMMENT, NewspeakTokenTypes.LINE_COMMENT)`. `NewspeakSyntaxHighlighter`: `Types.LINE_COMMENT to NewspeakColors.COMMENT` beside the `COMMENT` entry. `NewspeakCommenter`:

```kotlin
/** Both of ns-next's comment forms: `///` lines and `(* … *)` blocks. */
class NewspeakCommenter : Commenter {
    override fun getLineCommentPrefix(): String = "///"
    // … the block members unchanged …
}
```

`NewspeakIndexPatternBuilder`, so a TODO is searched after `///` and before the line's end:

```kotlin
/** Where TODO markers are searched: inside a comment, not counting its markers. */
// …
    override fun getCommentStartDelta(tokenType: IElementType?): Int =
        if (tokenType == NewspeakTokenTypes.LINE_COMMENT) 3 else 2
    override fun getCommentEndDelta(tokenType: IElementType?): Int =
        if (tokenType == NewspeakTokenTypes.LINE_COMMENT) 0 else 2
```

Every other `main` site that tests `== NewspeakTokenTypes.COMMENT` (spell checking among them, if it does) treats `LINE_COMMENT` the same.

`LexerComparisonTest.kt:96-97` becomes:

```kotlin
                    type == NewspeakTokenTypes.COMMENT ->
                        assertTrue("$file: comment at ${lexer.tokenStart}", tokenText.startsWith("(*"))
                    type == NewspeakTokenTypes.LINE_COMMENT ->
                        assertTrue("$file: line comment at ${lexer.tokenStart}", tokenText.startsWith("///") && '\n' !in tokenText && '\r' !in tokenText)
```

- [ ] **Step 4: Run the plugin's suite**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: BUILD SUCCESSFUL, the comparison test among the passing ones.

- [ ] **Step 5: The golden**

`goldens/language/syntax/line_comment.ns` (the only `.ns` outside `corpus/` that keeps a `(* … *)` after Task 5):

```newspeak
/// A line comment runs from /// to the end of its line wherever it stands
/// outside a string or a comment, so a binary selector ends before it: the
/// first line of main sends + with the next line's 4. // is still the
/// integer division newspeak-spec.pdf 4.2's binSel names. Inside a string
/// /// is text, inside (* ... *) it is text too, and (* opens nothing
/// inside a line comment. The line comment is ns-next's deviation from
/// 4.2, whose binSel admits ///; docs/design/README.md records it.
class LineComment = Object new () (
    public main: platform args: arguments = (
        | t = platform transcript. |
        t show: (3 +/// the addend is on the next line
            4) printString; newLine.
        t show: (17 // 5) printString; newLine. /// still integer division
        t show: 'a /// b'; newLine.
        t show: "a /// b"; newLine.
        (* a block comment, /// inside it is text *)
        t show: 'after the block'; newLine.
        //// four slashes are a comment too
        /// (* opens nothing here
        t show: 'after the line'; newLine
    )
) : ()
```

`goldens/language/syntax/line_comment.toml`:

```toml
stdout = '''
7
3
a /// b
a /// b
after the block
after the line
'''
```

Run: `cargo nextest run -p ns-cli -E 'test(/line_comment/)'`
Expected: PASS under both lowerings. If the filter matches nothing, run `cargo nextest run -p ns-cli --test run_goldens` and look for `line_comment`.

- [ ] **Step 6: The language's documents**

- `docs/design/frontend.md`
  - "Grammar", the list at `:184`: add after the `(* ... *)` bullet: "- `///` starts a line comment to the end of its line wherever it stands outside a string or a comment, ending a binary selector before it — ns-next's deviation from §4.2's `binSel` (see "Comments")."
  - "Lexer", `:189`: "It handles nested `(* *)` comments and `///` line comments, literal-message …".
  - Replace the paragraph "**Comments are trivia.**" (`:257-263`) with a subsection `### Comments` holding: both forms are trivia, no token stands for one, the parser never sees one; the `///` rule (design points 1–3) and its reason (a comment wherever it stands is what a reader expects; a selector containing `///` is the price, accepted); `run_keeping_comments` answers one `Comment` per block — the block rule (design point 4) and why a trailing `///` does not join; `Comment::text` (design point 5); the class comment taken from a `///` block; `run`, `lex` and `parse` drop the list, the token stream the same either way. Then adjust the reference at `:482` ("`comments`, every comment in source order, consecutive `///` lines as one").
- `docs/design/README.md`
  - Map row (`:80`): "one deviation is recorded (a modifier needs no whitespace after it)" → "two deviations are recorded (a modifier needs no whitespace after it; `///` starts a line comment wherever it stands)"; "the comments' spans" → "its comments, each with its text".
  - Deviation table, a row after the `§5.1.4` one (table rows are in section order; §4.2 comes first, so it is the table's first row):
    `| `///` starts a line comment wherever it stands outside a string or a comment, so a binary selector ends before it: §4.2's `binSel` admits any run of special characters, `///`, `+///` and `<///>` among them, and no such selector can be declared or sent | §4.2 | [frontend.md](frontend.md), "Comments" |`
- `docs/design/editor-support.md`
  - `:72`: "with its comments' spans" → "with its comments".
  - The folding table's Comment row: "`(*` to `*)`, an unterminated one to the end of the file; a block of `///` lines, first to last" | "`(* `, the first non-blank line cut to 60 characters, ` ...*)`, or `/// `, that line, ` ...`; `(*...*)` or `///...` for a blank one" | `comment`.
  - `:746` (every comment a single token): say a `///` comment is one token per line, of its own type `LINE_COMMENT`, and why (the plugin's lexer is restartable at every token and merges nothing).
  - `:771` (the `Commenter`): "a `Commenter` for `///` lines and `(* *)` blocks".

- [ ] **Step 7: Gates and commit**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets && cargo nextest run --workspace && cargo test --workspace --doc && cargo run -p ns-analyzer -- parse-corpus corpus/modules && ./editors/jetbrains/gradlew -p editors/jetbrains check`
Expected: all green, "parsed 211, failed 0".

```bash
git add crates/ns-syntax crates/ns-analyzer crates/ns-runtime/tests/core_library_documentation.rs editors/jetbrains goldens/language/syntax/line_comment.ns goldens/language/syntax/line_comment.toml docs/design
git commit -m "feat: /// line comments beside (* *), one comment per block with its text"
```

(with the attribution lines).

---

### Task 3: The core library in `///`, and its documentation test requiring it — commit

**Files:**
- Create (not committed): `.temp/line-comments/convert_comments.py`, `.temp/line-comments/compare_tokens.py`, `.temp/line-comments/fixture.ns`
- Modify: `crates/ns-runtime/core-library/Kernel.ns`, `PlatformFactory.ns`, `CollectionsFactory.ns`, `Manifest.ns`
- Modify: `crates/ns-runtime/tests/core_library_documentation.rs`

**Interfaces:**
- Consumes: `ns_syntax::CommentKind::Line` (Task 1); `ns-next-analyzer dump-tokens` (existing; one JSON document per file per line: `{"file", "tokens": [{"kind", "start", "end"}], "diagnostics"}`).
- Produces: the two scripts, reused unchanged by Task 4.

- [ ] **Step 1: Write the scripts**

`.temp/line-comments/convert_comments.py` (`.temp/` is ignored by git; the script is a one-off and stays out of the repository):

```python
"""Rewrites every (* ... *) comment of the .ns files named on the command
line as /// lines, in place. One-off for the line-comments branch."""

import sys


def skip_string(text, index):
    """The index after the single-quoted string opening at `index`."""
    index += 1
    while index < len(text):
        if text[index] == "'":
            if text.startswith("''", index):
                index += 2
                continue
            return index + 1
        index += 1
    return index


def skip_double_quoted_string(text, index):
    """The index after the double-quoted string opening at `index`: one
    character and a quote, else up to the next quote (ns-syntax's rule)."""
    if index + 2 < len(text) and text[index + 2] == '"':
        return index + 3
    close = text.find('"', index + 1)
    return len(text) if close < 0 else close + 1


def block_comments(text):
    """(start, end) of every outermost (* ... *) outside strings and ///
    lines, as ns-syntax's lexer finds them."""
    spans = []
    index = 0
    while index < len(text):
        if text[index] == "'":
            index = skip_string(text, index)
        elif text[index] == '"':
            index = skip_double_quoted_string(text, index)
        elif text.startswith("///", index):
            newline = text.find("\n", index)
            index = len(text) if newline < 0 else newline
        elif text.startswith("(*", index):
            start = index
            index += 2
            depth = 1
            while depth > 0:
                if index >= len(text):
                    sys.exit(f"unterminated comment at offset {start}")
                if text.startswith("(*", index):
                    depth += 1
                    index += 2
                elif text.startswith("*)", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            spans.append((start, index))
        else:
            index += 1
    return spans


def body_lines(inner):
    """The comment's lines: the first trimmed, the rest without their
    shared indentation, blank ones at either end dropped."""
    lines = inner.split("\n")
    first, rest = lines[0].strip(), lines[1:]
    indentations = [len(line) - len(line.lstrip(" ")) for line in rest if line.strip()]
    shared = min(indentations, default=0)
    body = [first] + [line[shared:].rstrip() if line.strip() else "" for line in rest]
    while body and not body[0]:
        body.pop(0)
    while body and not body[-1]:
        body.pop()
    return body


def convert_one(text, start, end):
    """`text` with the comment at start..end rewritten as /// lines."""
    line_start = text.rfind("\n", 0, start) + 1
    line_end = text.find("\n", end)
    if line_end < 0:
        line_end = len(text)
    line = text[line_start:]
    indentation = line[: len(line) - len(line.lstrip(" "))]
    before = text[line_start:start]
    after = text[end:line_end]
    body = body_lines(text[start + 2 : end - 2])
    rendered = [indentation + ("/// " + content if content else "///") for content in body]
    if not rendered:
        rendered = [indentation + "///"]
    if before.strip() in ("|", "||") and not after.strip():
        # A slot section's bar, then the first slot's documentation: the
        # comment goes below the bar, indented as the slot is, so nothing
        # stands between it and the slot it documents.
        following = text[line_end + 1 :].split("\n", 1)[0]
        slot_indentation = following[: len(following) - len(following.lstrip(" "))]
        lines = [before.rstrip()] + [
            slot_indentation + ("/// " + content if content else "///") for content in body
        ]
    elif not before.strip() and not after.strip():
        lines = rendered
    elif before.strip() and not after.strip() and len(body) <= 1:
        # A remark after code stays where it was.
        lines = [before.rstrip() + " ///" + (" " + body[0] if body else "")]
    else:
        # Code after the comment, or a long remark after code: the comment
        # goes above the line, the code stays on it.
        if before.strip() and after.strip():
            code = before.rstrip() + " " + after.lstrip()
        elif before.strip():
            code = before.rstrip()
        else:
            code = indentation + after.lstrip()
        lines = rendered + [code]
    return text[:line_start] + "\n".join(lines) + text[line_end:]


def main():
    for path in sys.argv[1:]:
        with open(path, encoding="utf-8", newline="") as source:
            text = source.read()
        # A Windows checkout may hold CRLF (core.autocrlf); the file is
        # written back with the line endings it had.
        crlf = "\r\n" in text
        text = text.replace("\r\n", "\n")
        if "\r" in text:
            sys.exit(f"{path}: a lone carriage return; convert by hand")
        count = 0
        while True:
            spans = block_comments(text)
            if not spans:
                break
            start, end = spans[0]
            text = convert_one(text, start, end)
            count += 1
        if crlf:
            text = text.replace("\n", "\r\n")
        with open(path, "w", encoding="utf-8", newline="") as target:
            target.write(text)
        print(f"{path}: {count}")


main()
```

`.temp/line-comments/compare_tokens.py`:

```python
"""Compares two dump-tokens outputs file by file: the same kinds and
lengths in the same order, so a comment rewrite moved no code."""

import json
import sys


def load(path):
    documents = {}
    with open(path, encoding="utf-8") as dump:
        for line in dump:
            document = json.loads(line)
            documents[document["file"]] = (
                [(token["kind"], token["end"] - token["start"]) for token in document["tokens"]],
                document["diagnostics"],
            )
    return documents


before, after = load(sys.argv[1]), load(sys.argv[2])
failures = [name for name in before if before[name] != after.get(name)]
for name in failures:
    print(f"changed: {name}")
print(f"compared {len(before)} files, {len(failures)} changed")
sys.exit(1 if failures else 0)
```

`.temp/line-comments/fixture.ns`, every shape the rewrite meets:

```newspeak
(* Header line one
   header line two. *)
class Fixture = Object new (
    | (* the slot's documentation *)
      slot = 1. |
) (
    (* own line *)
    public first = ( ^1 ) (* trailing *)
    public second = ( (* leading *) ^2 )
    (* outer (* nested *) still outer *)
    public third = ( ^'(* not a comment *)' )
    public fourth = (
        ^3 (* a long remark
              on two lines *)
    )
    (*
        indented block
          deeper
    *)
    public fifth = ( (* one *) ^4 (* two *) )
) : ()
```

- [ ] **Step 2: Try the script on the fixture**

Run (from the repository root):
```bash
cp .temp/line-comments/fixture.ns .temp/line-comments/fixture-converted.ns
cargo build -p ns-analyzer
./target/debug/ns-next-analyzer dump-tokens .temp/line-comments/fixture-converted.ns > .temp/line-comments/fixture-before.jsonl
python .temp/line-comments/convert_comments.py .temp/line-comments/fixture-converted.ns
./target/debug/ns-next-analyzer dump-tokens .temp/line-comments/fixture-converted.ns > .temp/line-comments/fixture-after.jsonl
python .temp/line-comments/compare_tokens.py .temp/line-comments/fixture-before.jsonl .temp/line-comments/fixture-after.jsonl
cat .temp/line-comments/fixture-converted.ns
```
Expected: `compared 1 files, 0 changed`, and the file reads:

```newspeak
/// Header line one
/// header line two.
class Fixture = Object new (
    |
      /// the slot's documentation
      slot = 1. |
) (
    /// own line
    public first = ( ^1 ) /// trailing
    /// leading
    public second = ( ^2 )
    /// outer (* nested *) still outer
    public third = ( ^'(* not a comment *)' )
    public fourth = (
        /// a long remark
        /// on two lines
        ^3
    )
    /// indented block
    ///   deeper
    /// one
    /// two
    public fifth = ( ^4 )
) : ()
```

(This output was produced by running the script while the plan was written.) The comments are taken first to last, so two lifted from one line keep their order. A lifted comment directly under another comment merges into its block (`indented block` … `two` above): right when both describe what follows, wrong when the upper one documents something else — review every lifted line by hand (Step 3).

- [ ] **Step 3: Convert the core library**

```bash
files="crates/ns-runtime/core-library/Kernel.ns crates/ns-runtime/core-library/PlatformFactory.ns crates/ns-runtime/core-library/CollectionsFactory.ns crates/ns-runtime/core-library/Manifest.ns"
./target/debug/ns-next-analyzer dump-tokens $files > .temp/line-comments/core-before.jsonl
python .temp/line-comments/convert_comments.py $files
./target/debug/ns-next-analyzer dump-tokens $files > .temp/line-comments/core-after.jsonl
python .temp/line-comments/compare_tokens.py .temp/line-comments/core-before.jsonl .temp/line-comments/core-after.jsonl
```
Expected: `compared 4 files, 0 changed`.

Then read the diff (`git diff --stat` and `git diff crates/ns-runtime/core-library | grep '^+' | grep -v '^+ *///'`) for every line the script changed that is code rather than a comment line: each is a lifted comment's code line; check it reads naturally and that no two blocks merged that should not (a lifted comment directly under a member's documentation). Fix by hand. `grep -n '(\*' crates/ns-runtime/core-library/*.ns` now finds only text inside `///` lines and strings.

- [ ] **Step 4: Require `///` above every member**

In `crates/ns-runtime/tests/core_library_documentation.rs`:

```rust
//! Every member of a shipped core-library module is documented
//! (CLAUDE.md, "Conventions"): a `///` comment directly above every class,
//! slot and method, whatever its access.
```

```rust
/// Every `///` block of `source`, as a byte range. A `(* … *)` is no
/// member's documentation: the house style is the line comment, whose
/// text a reader such as a hover takes.
fn comment_spans(source: &str) -> Vec<(usize, usize)> {
    ns_syntax::lexer::Lexer::new(source)
        .run_keeping_comments()
        .comments
        .iter()
        .filter(|comment| comment.kind == ns_syntax::CommentKind::Line)
        .map(|comment| (comment.span.start as usize, comment.span.end as usize))
        .collect()
}
```

The assertion message: "members without a `///` comment directly above them".

- [ ] **Step 5: Gates and commit**

Run the five pre-commit commands (Global Constraints). `crates/ns-analyzer/tests/disasm_snapshots.rs` may hold snapshots with source positions of the core library: if one fails, confirm the diff is positions only, then update it as that test's own instructions say; anything else is a defect of the rewrite.

```bash
git add crates/ns-runtime/core-library crates/ns-runtime/tests/core_library_documentation.rs
git commit -m "refactor: the core library documents every member with ///"
```

(plus any snapshot the step updated; with the attribution lines).

---

### Task 4: Goldens, benchmarks and inline modules in `///`; block comments refused — commit

**Files:**
- Modify: every `goldens/**/*.ns` and `benchmarks/**/*.ns` holding `(*`, except `goldens/language/syntax/line_comment.ns`
- Modify: the Newspeak modules inline in Rust tests whose comment is incidental (list in Step 4)
- Modify: `crates/ns-cli/tests/source_formatting.rs`

**Interfaces:**
- Consumes: the scripts of Task 3; `ns_syntax::lexer::Lexer`, `ns_syntax::CommentKind::Block` (Task 1).

- [ ] **Step 1: Write the failing check**

Append to `crates/ns-cli/tests/source_formatting.rs` (update its module doc to "…indented with four spaces, never tabs, and commented with `///`"):

```rust
/// The one source that keeps a `(* … *)`: the golden that pins both forms.
const BLOCK_COMMENT_GOLDEN: &str = "goldens/language/syntax/line_comment.ns";

/// The house style is the `///` line comment, whose text an editor reads;
/// `(* … *)` stays valid Newspeak and is pinned by one golden. The check
/// lexes rather than searches, so a `(*` in a string is no offence.
#[test]
fn newspeak_sources_outside_the_corpus_comment_with_line_comments() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("git")
        .current_dir(&root)
        .args(["ls-files", "*.ns"])
        .output()
        .expect("git ls-files runs");
    assert!(output.status.success(), "git ls-files failed");
    let listing = String::from_utf8(output.stdout).expect("file names are UTF-8");
    let mut offences = Vec::new();
    for name in listing.lines() {
        if name.starts_with("corpus/") || name == BLOCK_COMMENT_GOLDEN {
            continue;
        }
        let text = std::fs::read_to_string(root.join(name)).expect("a tracked file reads");
        for comment in ns_syntax::lexer::Lexer::new(&text).run_keeping_comments().comments {
            if comment.kind == ns_syntax::CommentKind::Block {
                let line = text[..comment.span.start as usize].matches('\n').count() + 1;
                offences.push(format!("{name}:{line}"));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "(* … *) comments in Newspeak sources (write /// lines):\n{}",
        offences.join("\n")
    );
}
```

Factor the `git ls-files` listing both tests share into one helper `tracked_sources_outside_the_corpus(root: &Path) -> Vec<String>` while you are there.

Run: `cargo nextest run -p ns-cli -E 'test(/comment_with_line_comments/)'`
Expected: FAIL, listing the goldens and benchmarks.

- [ ] **Step 2: Convert the goldens and the benchmarks**

```bash
git ls-files 'goldens/*.ns' 'benchmarks/*.ns' | grep -v '^goldens/language/syntax/line_comment.ns$' | xargs grep -l '(\*' > .temp/line-comments/rest-files.txt
# Through xargs: some 340 paths are past Windows' command-line limit.
xargs ./target/debug/ns-next-analyzer dump-tokens < .temp/line-comments/rest-files.txt > .temp/line-comments/rest-before.jsonl
xargs python .temp/line-comments/convert_comments.py < .temp/line-comments/rest-files.txt
xargs ./target/debug/ns-next-analyzer dump-tokens < .temp/line-comments/rest-files.txt > .temp/line-comments/rest-after.jsonl
python .temp/line-comments/compare_tokens.py .temp/line-comments/rest-before.jsonl .temp/line-comments/rest-after.jsonl
```
Expected: `compared N files, 0 changed` (N ≈ 340). A changed file is reverted (`git checkout -- <file>`) and converted by hand. A Windows checkout holds these files with CRLF (`core.autocrlf`); the script keeps each file's line endings, and git normalizes them on commit.

A dry run over copies of every source while this plan was written converted 1,394 comments in 382 files (the core library included) and left no `(*` outside `///` lines and strings; 109 code lines were rewritten (trailing remarks and lifted comments), the ones Step 3 reviews.

- [ ] **Step 3: Review the lifted lines**

`git diff goldens benchmarks | grep '^+' | grep -v '^+ *///' | grep -v '^+++'` lists every code line the script rewrote. Check each as in Task 3, Step 3. The benchmarks' licence headers become `///` lines with their text unchanged.

- [ ] **Step 4: The Newspeak modules inline in Rust tests**

Candidates: `grep -rln 'indoc!\|formatdoc!' crates | xargs grep -n '(\*'`, then keep only hits inside a Newspeak string (most `(*` in Rust are dereferences). Rewrite by hand those whose comment is incidental to the test. Leave unchanged, because they test the block form itself: `crates/ns-syntax/src/lexer.rs`, `crates/ns-syntax/src/parser/declaration.rs` (class-comment tests), `crates/ns-syntax/tests/recovery.rs` (`a_clean_source_parses_as_parse_parses_it`, `an_unterminated…` and any other comment test), `crates/ns-analyzer/src/folding.rs` (the `(* … *)` fold tests), `crates/ns-analyzer/src/analysis.rs` and `tests/protocol.rs` wherever a test's assertion names the comment or its span. A rewritten module whose test asserts a position, a span or a snapshot is checked to change only in that position, then its expectation updated.

- [ ] **Step 5: Gates and commit**

Run the five pre-commit commands; `cargo nextest run -p ns-cli -E 'test(/comment_with_line_comments/)'` passes now. Every golden passes unchanged — no `.toml` is in the diff (`git diff --stat -- '*.toml'` is empty).

```bash
git add goldens benchmarks crates
git commit -m "refactor: the goldens and benchmarks comment with ///, and (* *) is refused outside corpus/"
```

(with the attribution lines).

---

### Task 5: The conventions and the remaining documents — commit

**Files:**
- Modify: `CLAUDE.md` (Conventions: the four-space bullet, the "Every member … documented" bullet at `:378-387`)
- Modify: `docs/design/README.md:329-331` ("The goldens": a golden's rationale)
- Modify: `docs/design/frontend.md:16-36` ("What a source file looks like": its sample)
- Modify: `docs/design/core-library.md` (where it states the documentation form; `grep -n 'comment' docs/design/core-library.md`), `docs/design/object-model.md:272-273` and every other Newspeak sample in `docs/design/` with a `(* … *)` (`grep -n '(\*' docs/design/*.md`), except where the text is about the block form
- Modify: `docs/PROGRESS.md` (a section for this milestone, dated 2026-10-11, at the end)
- Modify: `docs/OPEN-WORK.md` (a hover item)

- [ ] **Step 1: CLAUDE.md**

- The bullet "Newspeak sources (`.ns`) outside `corpus/` are indented with four spaces, never tabs; `crates/ns-cli/tests/source_formatting.rs` holds it, and `corpus/` stays upstream's verbatim copy." becomes "… are indented with four spaces, never tabs, and commented with `///` lines, never `(* … *)` (but in `goldens/language/syntax/line_comment.ns`, which pins both forms); `crates/ns-cli/tests/source_formatting.rs` holds both, and `corpus/` stays upstream's verbatim copy."
- "has a `(* … *)` comment directly above it" → "has a `///` comment directly above it".

- [ ] **Step 2: The design documents**

- `docs/design/README.md`, "The goldens": "is a `(* ... *)` comment at the top of the `.ns`" → "is a `///` comment at the top of the `.ns`".
- `frontend.md`'s sample in "What a source file looks like": the class comment line becomes `/// optional class comment`, and the sentence around it says both forms are comments (point to "Comments").
- `core-library.md`: where it describes the documentation comment, `///`.
- Newspeak samples in `docs/design/*.md`: their `(* … *)` remarks become `///` (a trailing remark stays trailing). Leave text that discusses the block form.

- [ ] **Step 3: PROGRESS.md and OPEN-WORK.md**

`docs/PROGRESS.md`, a section `## Line comments (2026-10-11)` in the file's style: `///` beside `(* … *)`, the deviation from §4.2 and where it is recorded, `Comment` with its text, the plugin's `LINE_COMMENT` and commenter, the repository's sources rewritten, the two checks that hold it (`core_library_documentation.rs`, `source_formatting.rs`), the golden.

`docs/OPEN-WORK.md`, an item where editor work is listed (read the file to place it; not "Next"): "**A hover with the comment above a declaration.** `textDocument/hover` on a class, slot or method shows the `Comment::text` of the `///` block (or `(* … *)`) directly above its declaration, rendered as Markdown. `ns_syntax::Comment` answers the text; nothing yet maps a declaration to its comment (`docs/design/frontend.md`, "Comments"; `editor-support.md`)."

- [ ] **Step 4: Commit**

`git diff --stat` shows Markdown only, so no gates run.

```bash
git add CLAUDE.md docs
git commit -m "docs: /// is the house comment style; a hover over its text is open"
```

(with the attribution lines).

---

### Task 6: Finish the batch

- [ ] **Step 1:** Invoke the `finishing-a-batch` skill and follow it: the five stress modes, `run-corpus` plain and under stress, the benchmarks profile, `verifyPlugin`, the documentation check that a fresh session can start from the documents alone.
- [ ] **Step 2:** Delete `.temp/` (it holds this branch's scripts and dumps only).
- [ ] **Step 3:** The whole-branch review, then hand back to the user for the merge decision.
