# Removing `:=` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop accepting `:=`, Smalltalk's assignment, which §5.4.3 does not
give Newspeak, and write every golden, test and document example that used it
as the specification's setter send, `name:: expression`.

**Architecture:** Both spellings already parse to the same
`Expression::Assign`, so the conversion is done first, while `:=` is still
accepted: every recorded outcome must hold unchanged across it, which is the
proof that the two spellings meant the same thing everywhere they were used.
Only then does the lexer stop producing `TokenKind::Assign`. It names the
mistake instead — a `:=` is one lexer error saying what to write — rather than
letting it fall apart into `:` and `=` and surface as a confusing parse error
further on. The parser loses the `:=` arm of `parse_expression` and the two
`peek2() != TokenKind::Assign` guards that kept a unary-send loop from eating
an assignment's target.

**Tech Stack:** Rust 1.98.0, edition 2021, the three workspace crates; no new
dependency. `perl` (Git Bash) for the mechanical conversion.

**Spec:** `docs/design/frontend.md`, "Deviation: `:=` is accepted as a synonym
for a setter send" (the decision, taken with the user on 2026-09-19, is its
last paragraph), and `docs/PROGRESS.md`, "Where the next session starts", the
`:=` item of "Known gaps the step left". The rule itself is §5.4.3 of
`newspeak-spec.pdf` ("Setter Sends").

## Global Constraints

- Work on the branch `feature/remove-assign`, created from `master` at
  `24e414d`, on the checked-out tree — no worktree.
- Nothing under `goldens/**/*.toml` changes. A golden's recorded outcome
  moving is a behaviour change, and this step changes none.
- The corpus gates stay exactly as they are: `parse-corpus` reports
  `parsed 210, failed 0, expected-fail 1, stale 0` and `run-corpus` reports
  `ran 6, failed 0, skipped 205`. The corpus contains no `:=` outside
  comments (checked while planning), so neither gate may move.
- Plans under `docs/superpowers/plans/` are execution records and are not
  edited, even where they spell `:=`. Chronology entries in
  `docs/PROGRESS.md` below "Where the next session starts" are not rewritten
  either; only the forward-looking parts are.
- Names are written out (`CLAUDE.md`, "Conventions"); clippy's
  `min_ident_chars` enforces it. Conventional commits, each ending with the
  attribution lines the session gives.
- `cargo fmt --all` before every commit; `cargo clippy --workspace
  --all-targets` warning-free.
- No measurement: nothing on the run loop's path changes. The four
  measurement sources the documents carry change spelling only, to the one
  every compared binary accepts (`be3eb40` already parses `name::` at an
  expression's head, `crates/ns-syntax/src/parser/expr.rs` there), and they
  parse to the same AST as before.

---

## File Structure

- `goldens/**/*.ns` (49 files) — every `:=` becomes `::`. No `.toml` changes.
- `crates/ns-runtime/src/compiler/lower.rs`, `compiler/reference.rs` — the
  Newspeak in their unit tests and two comments.
- `crates/ns-runtime/tests/*.rs` (18 files) — the inline Newspeak modules.
- `crates/ns-syntax/src/lexer.rs` — the `:` arm refuses `:=` by name.
- `crates/ns-syntax/src/token.rs` — `TokenKind::Assign` goes.
- `crates/ns-syntax/src/parser/expression.rs`, `parser/declaration.rs` — the
  `:=` arm and the two guards go.
- `crates/ns-syntax/src/parser/mod.rs`, `parser/declaration.rs` (tests),
  `crates/ns-syntax/src/ast.rs` (doc comment) — `::` spelling.
- `docs/design/frontend.md`, `docs/design/README.md`, `docs/design/compiler.md`,
  `docs/design/execution.md`, `docs/PROGRESS.md` — the record.

---

### Task 1: Write every use as a setter send

The conversion comes first so that the whole suite, with `:=` still accepted,
proves the new spelling answers exactly what the old one did.

**Files:**
- Modify: the 49 `goldens/**/*.ns` files and the 20 `.rs` files under
  `crates/ns-runtime` that contain a `:=` not preceded by `:` (list them with
  the command in Step 1).

**Interfaces:**
- Consumes: nothing.
- Produces: a tree in which no `.ns` golden and no `ns-runtime` source spells
  `:=`. Task 2 relies on this: once `:=` is refused, anything missed here
  fails to parse.

- [ ] **Step 1: Create the branch and list what uses `:=`**

```bash
git switch -c feature/remove-assign
grep -rlP "(?<!:):=" goldens crates/ns-runtime
```

Expected: 50 paths — 49 under `goldens/`, and `lower.rs`, `reference.rs` and
18 files under `crates/ns-runtime/tests/`. `::=` (a mutable slot's
initializer) is not matched and must not be touched.

- [ ] **Step 2: Convert mechanically**

Every one of the 280 occurrences is an identifier, optional whitespace, then
`:=` (checked while planning: the pattern below matches all 280). Rewrite
each to the identifier glued to `::` and one space:

```bash
grep -rlP "(?<!:):=" goldens crates/ns-runtime \
  | xargs perl -pi -e 's/\b([A-Za-z_]\w*)\s*(?<!:):=(?!=)\s*/$1:: /g'
```

`x := 1` becomes `x:: 1`, `^n := t` becomes `^n:: t`, `(t := 9)` becomes
`(t:: 9)`, `x ::= 0` is left alone (the `\s*` cannot consume a `:`, and `::`
is not `:=`).

- [ ] **Step 3: Check that nothing is left and nothing else moved**

```bash
grep -rnP "(?<!:):=" goldens crates/ns-runtime
git diff --stat -- goldens
git diff -- 'goldens/**/*.toml'
```

Expected: the first command prints nothing; the diff touches only `.ns`
files; the `.toml` diff is empty. Read `git diff -- crates/ns-runtime/src` in
full: the two comments in `lower.rs` (the `^x:: y` example near line 418 and
`t:: t` near line 589) must still read as sentences.

- [ ] **Step 4: Run the suite and the corpus gates**

```bash
cargo fmt --all
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: every test passes, every golden under both lowerings;
`parsed 210, failed 0, expected-fail 1, stale 0`;
`ran 6, failed 0, skipped 205`. A golden that now fails means the two
spellings parsed differently somewhere — stop and find out why; do not edit
its `.toml`.

- [ ] **Step 5: Commit**

```bash
git add goldens crates/ns-runtime
git commit -m "test: write every assignment as a setter send

Every golden and runtime test spelled Smalltalk's := where §5.4.3 gives
Newspeak only the setter send, name:: expression. Both parse to the same
Expression::Assign, and no recorded outcome moved across the rewrite,
which is what lets the next commit refuse :=.
"
```

(Append the session's attribution lines to the message.)

---

### Task 2: Refuse `:=` in the lexer, by name

**Files:**
- Modify: `crates/ns-syntax/src/lexer.rs` (the `:` arm, `lex_word`'s comment,
  `lex_operator`'s comment, the `punctuation_versus_binary_selector` test, one
  new test)
- Modify: `crates/ns-syntax/src/token.rs:28` (`Assign` goes)
- Modify: `crates/ns-syntax/src/parser/expression.rs` (`parse_expression`'s
  `:=` arm and comment, the guard near line 464)
- Modify: `crates/ns-syntax/src/parser/declaration.rs` (the guard near line
  463; the tests near lines 997, 999 and 1127)
- Modify: `crates/ns-syntax/src/parser/mod.rs` (the tests near lines 413 and
  491, one new test)
- Modify: `crates/ns-syntax/src/ast.rs:119` (doc comment)

**Interfaces:**
- Consumes: Task 1's tree, in which nothing outside `ns-syntax` spells `:=`.
- Produces: `ns_syntax::lex` and `ns_syntax::parse` answer `Err` for any
  source containing `:=` outside a comment or string, with one diagnostic
  whose message starts
  `` `:=` is not Newspeak syntax``. `TokenKind::Assign` no longer exists.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-syntax/src/lexer.rs`'s test module, replace
`punctuation_versus_binary_selector` so it no longer lists `:=`:

```rust
    #[test]
    fn punctuation_versus_binary_selector() {
        assert_eq!(
            kinds("| || = ::= ^ ."),
            vec![
                Bar,
                DoubleBar,
                Equals,
                ColonColonEquals,
                Caret,
                Dot,
                EndOfInput,
            ]
        );
    }
```

and add, beside it:

```rust
    #[test]
    fn smalltalk_assignment_is_refused_by_name() {
        // §5.4.3 gives Newspeak one assignment form, the setter send. Both the
        // spaced and the glued spelling are named, not split into `:` and `=`.
        for (source, start, end) in [("x := 1", 2, 4), ("x:=1", 1, 3)] {
            let (_, diagnostics) = Lexer::new(source).run();
            assert_eq!(diagnostics.len(), 1, "{source}: {diagnostics:?}");
            let diagnostic = &diagnostics[0];
            assert_eq!(diagnostic.severity, Severity::Error, "{source}");
            assert_eq!(diagnostic.span, Span::new(start, end), "{source}");
            assert!(
                diagnostic
                    .message
                    .starts_with("`:=` is not Newspeak syntax"),
                "{source}: {}",
                diagnostic.message
            );
        }
    }
```

If `Severity` is not yet imported in that test module, import it the way the
module imports `Span` (`use crate::diagnostic::Severity;`).

In `crates/ns-syntax/src/parser/mod.rs`'s test module, delete
`assignment_right_associative` — `nested_setter_keyword_at_head_is_nested_assign`
already pins the same shape in the setter-send spelling — and add in its
place:

```rust
    #[test]
    fn a_module_spelling_colon_equals_does_not_parse() {
        let source = indoc! {"
            Newspeak3
            'X'
            class C = () (
                public m = ( | t | t := 1. ^t )
            ) : ()
        "};
        let Err(diagnostics) = crate::parse(source) else {
            panic!("`:=` parsed");
        };
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.starts_with("`:=` is not Newspeak syntax")),
            "{diagnostics:?}"
        );
    }
```

(`indoc` is already a dev-dependency of `ns-syntax`, used by
`parser/declaration.rs`'s tests; add `use indoc::indoc;` to this test module
if it is not imported there yet.)

- [ ] **Step 2: Run them to see them fail**

```bash
cargo test -p ns-syntax smalltalk_assignment_is_refused_by_name
cargo test -p ns-syntax a_module_spelling_colon_equals_does_not_parse
cargo test -p ns-syntax punctuation_versus_binary_selector
```

Expected: the first two FAIL (no diagnostic; the module parses), the third
passes already.

- [ ] **Step 3: Refuse `:=` in the lexer**

In `crates/ns-syntax/src/lexer.rs`, replace the `b':'` arm of `lex_one`:

```rust
            b':' => {
                if self.peek_at(1) == Some(b':') && self.peek_at(2) == Some(b'=') {
                    self.position += 3;
                    self.push(TokenKind::ColonColonEquals, start, self.position);
                } else if self.peek_at(1) == Some(b'=') {
                    // Smalltalk's assignment. §5.4.3 gives Newspeak one
                    // assignment form, the setter send, and nothing in the
                    // specification's grammar, the canonical grammar, Primordial
                    // Soup or SOMns accepts `:=`. Named here, because split into
                    // `:` and `=` it would surface as an unrelated parse error
                    // further on.
                    self.position += 2;
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start as u32, self.position as u32),
                        "`:=` is not Newspeak syntax; a setter send is written `name:: expression`",
                    ));
                } else {
                    self.single(TokenKind::Colon);
                }
            }
```

In `lex_word`, keep the branch that stops before `:=` and `::=` — it is what
routes a glued `x:=1` to the arm above instead of lexing `x:` as a keyword —
and rewrite its comment:

```rust
                // `id::=` -> Identifier then ColonColonEquals, and `id:=` ->
                // Identifier then the `:` arm's refusal of `:=`. Emit the bare
                // word and let `lex_one`'s `:` arm take the rest.
```

In `lex_operator`, the comment

```rust
        // `::=` / `:=` are handled in `lex_one`'s `:` arm before we get here; a run
        // starting with a special character is a binary selector.
```

stays true as written; leave it.

In `crates/ns-syntax/src/token.rs`, delete the `Assign,` variant (line 28)
and any doc comment directly above it.

- [ ] **Step 4: Remove the parser's `:=` arm and guards**

In `crates/ns-syntax/src/parser/expression.rs`, delete from
`parse_expression` the block that starts `// assignment: Identifier ':='
expression` and ends at its `return Expression::Assign { … };` and closing
brace, so the function opens with the setter-keyword case. Rewrite that
case's comment, which described it by reference to `:=`:

```rust
        // `x:: v` at expression head — the grammar's `setterKeyword*
        // sendExpression`, §5.4.3's setter send and Newspeak's only assignment
        // form. Semantically a slot write, so represent it as Assign.
        // Setter-keyword *after* a receiver (`receiver x:: v`) is still a
        // keyword send, handled in parse_keyword_message.
```

(keep the `NOTE:` paragraph below it unchanged).

In the unary-send loop near line 464 of the same file, the condition

```rust
            if !((self.at(TokenKind::Identifier) || self.at(TokenKind::Class))
                && !self.is_reserved_identifier(self.current())
                && self.peek2() != TokenKind::Assign)
```

becomes

```rust
            if !((self.at(TokenKind::Identifier) || self.at(TokenKind::Class))
                && !self.is_reserved_identifier(self.current()))
```

and in `crates/ns-syntax/src/parser/declaration.rs`'s `parse_trailing_sends`,
near line 463,

```rust
        while (self.at(TokenKind::Identifier) || self.at(TokenKind::Class))
            && !self.is_reserved_identifier(self.current())
            && self.peek2() != TokenKind::Assign
        {
```

becomes

```rust
        while (self.at(TokenKind::Identifier) || self.at(TokenKind::Class))
            && !self.is_reserved_identifier(self.current())
        {
```

Both guards only kept a unary loop from consuming the identifier in front of
a `:=`; a setter send is a single `SetterKeyword` token, which neither loop
consumes.

In `crates/ns-syntax/src/ast.rs`, the doc comment on `Expression::Assign`

```rust
    /// `lhs := rhs`  (lhs is always an Identifier in Newspeak)
```

becomes

```rust
    /// `name:: value`, a setter send at an expression's head (§5.4.3)
```

- [ ] **Step 5: Convert the syntax crate's own tests**

In `crates/ns-syntax/src/parser/declaration.rs`, `slots_methods_and_nested_class`:

```rust
                | x ::= 0. |
                x:: 1.
            ) (
                public bump = ( x:: x + 1 )
```

and `initializer_expressions_in_g1_only`:

```rust
            class C = ( x:: 1. y:: 2. ) ()
```

In `crates/ns-syntax/src/parser/mod.rs`, `block_with_parameters_and_temporaries`:

```rust
        let expression = parse_expression("[:x :y | | t | t:: x. t + y ]");
```

Then confirm nothing in the crate still spells it:

```bash
grep -rnP "(?<!:):=" crates/ns-syntax
```

Expected: only the new tests' sources, the lexer's refusal message and the
two lexer comments.

- [ ] **Step 6: Run everything**

```bash
cargo fmt --all
cargo build --workspace
cargo test --workspace
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: all green and warning-free; the two new tests pass;
`parsed 210, failed 0, expected-fail 1, stale 0`;
`ran 6, failed 0, skipped 205`. A compile error naming `TokenKind::Assign`
anywhere is a use this plan missed — remove it the same way. `parse-corpus`
moving means the corpus did spell `:=` in code after all: stop and report it
rather than blacklisting the file.

- [ ] **Step 7: Commit**

```bash
git add crates/ns-syntax
git commit -m "fix: refuse := instead of reading it as a setter send

§5.4.3 gives Newspeak one assignment form, name:: expression. The lexer
also read Smalltalk's := as a synonym for it. It now refuses := with a
diagnostic naming the setter send, and the parser loses the := arm and
the two guards that kept a unary loop off an assignment's target.
"
```

(Append the session's attribution lines to the message.)

---

### Task 3: Record it

**Files:**
- Modify: `docs/design/frontend.md` (the deviation section)
- Modify: `docs/design/README.md:213` (the deviations row)
- Modify: `docs/design/compiler.md:193`, `:624`, `:643`
- Modify: `docs/design/execution.md` (the three written-out sources near
  lines 1413–1485, the measured bodies near 525–536 and 641, and a written-out
  `bench_do.ns`)
- Modify: `docs/PROGRESS.md` ("Where the next session starts", the order of
  work, a new chronology entry)

**Interfaces:**
- Consumes: Tasks 1 and 2's commits (their hashes go into the chronology).
- Produces: documents in which a fresh session finds closure conversion as
  the next piece of work and no example spells `:=`.

- [ ] **Step 1: Retire the deviation in `frontend.md`**

Replace the whole section `## Deviation: \`:=\` is accepted as a synonym for a
setter send` with:

```markdown
## `:=` is refused

§5.4.3 gives Newspeak exactly one assignment form, the setter send `id:: e`
(desugaring to `id: e`, "Setter Sends"), which `parse_expression` reads at an
expression's head as an `Expression::Assign` ([dispatch.md](dispatch.md),
"Setter sends"). Nothing in the specification grammar, the canonical grammar
(`corpus/reference/NewspeakGrammar.ns`), Primordial Soup or SOMns accepts
`:=`, Smalltalk's assignment. The lexer refuses it by name — one diagnostic,
`` `:=` is not Newspeak syntax; a setter send is written `name:: expression` ``
— rather than splitting it into `:` and `=` and letting the parser report
something unrelated further on. Until 2026-09-19 it was accepted as a synonym
for the setter send, a deviation that only over-accepted; removing it
converted every golden and test that spelled it, with no recorded outcome
moving ([PROGRESS.md](../PROGRESS.md), "M4 — Removing `:=`").
```

- [ ] **Step 2: Drop the README row**

In `docs/design/README.md`, delete the deviations-table row that begins
``| `:=` assignment parses as a synonym for a setter send``. Leave the rest
of the table alone.

- [ ] **Step 3: Respell the examples in `compiler.md` and `execution.md`**

In `docs/design/compiler.md`, `` `^x := y` `` becomes `` `^x:: y` ``,
`` `b := [true ifTrue: b]` `` becomes `` `b:: [true ifTrue: b]` `` and
`` `b := [:n | n = 0 ifFalse: [b value: n - 1]]` `` becomes
`` `b:: [:n | n = 0 ifFalse: [b value: n - 1]]` ``.

In `docs/design/execution.md`, apply the same rewrite to every `:=` (the
measured bodies near lines 525–536 and 641, and the three sources under
"What spec-conformant lookup moved"):

```bash
perl -pi -e 's/\b([A-Za-z_]\w*)\s*(?<!:):=(?!=)\s*/$1:: /g' docs/design/execution.md
grep -nP "(?<!:):=" docs/design/execution.md docs/design/compiler.md
```

Expected: the grep prints nothing. Read the diff: a table cell such as
`` `sum:: sum + 1` `` must still be one cell.

- [ ] **Step 4: Write `bench_do.ns` out where the others are**

The 2c plan is an execution record and keeps its `:=`, so a measurement
following it verbatim would no longer run on the current tree. In
`docs/design/execution.md`, "What spec-conformant lookup moved", change the
paragraph that introduces the three sources from

```markdown
These are the three as every later measurement uses them, written under
`target\bench-lookup\` (ignored by git) beside the unchanged `bench_do.ns` of
the [2c plan](../superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md),
"Measurement protocol". Each prints `20000`:
```

to

```markdown
These are the three as every later measurement uses them, written under
`target\bench-lookup\` (ignored by git), and after them `bench_do.ns`, first
written out in the
[2c plan](../superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md),
"Measurement protocol". All four are spelled with the setter send, which every
compared binary back to `be3eb40` parses; the 2c plan's copy spells `:=`,
which the tree has refused since "M4 — Removing `:=`" ([PROGRESS.md](../PROGRESS.md)),
and is the same program. The three print `20000`:
```

and after the third source's closing fence add:

````markdown
`bench_do.ns` prints `275000`:

```
Newspeak3
'Bench'
class BenchDo usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	public main: p args: a = (
		| n |
		n:: 0.
		[n < 5000] whileTrue: [#(1 2 3 4 5 6 7 8 9 10) do: [:e | sum:: sum + e]. n:: n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```
````

It is the 2c plan's source with its three `:=` respelled and nothing else
changed. Then check it runs and prints the recorded answer (tabs, as in the others):

```bash
mkdir -p target/bench-lookup
# write the fenced source above to target/bench-lookup/bench_do.ns
cargo run -p ns-cli -- run target/bench-lookup/bench_do.ns
```

Expected: `275000`. Do the same for the three `bench_new*.ns` sources:
`20000` each.

- [ ] **Step 5: Update `PROGRESS.md`**

1. In "Where the next session starts", replace the known-gap item
   ``- **`:=` is accepted but is not Newspeak syntax** (§5.4.3): …`` (the
   whole item, to the end of its sentence about closure conversion) with
   nothing — delete it — and add, after the paragraph beginning
   **The specification gaps are done** (2026-09-19), a paragraph:

   ```markdown
   **Removing `:=` is done** (2026-09-19): the lexer refuses Smalltalk's
   assignment by name, and every golden, test and document example is
   spelled as §5.4.3's setter send, `name:: expression`, with no recorded
   outcome moving; see "M4 — Removing `:=`" below. **Closure conversion,
   step 4 of the order of work below, is next**
   ([execution.md](design/execution.md)).
   ```

   Also update the sentence that points at `bench_do.ns` in the 2c plan
   ("`bench_do.ns` is written out in the 2c plan …") to say that all four,
   `bench_do.ns` included, are written out in execution.md, "What
   spec-conformant lookup moved", spelled with the setter send.

2. In the paragraph "The list below is M4's order of work — …", change
   "the specification gaps done, removing `:=` next as its own small step
   ahead of 4, everything from 4 on still ahead" to "the specification gaps
   done, removing `:=` done as its own small step ahead of 4, everything from
   4 on still ahead".

3. In item 3 of the order of work, change "**Removing `:=`** is next, its own
   small step, before step 4 ("Where the next session starts" above)." to
   "**Removing `:=`** is done, its own small step before step 4; see "M4 —
   Removing `:=`" below."

4. Add a chronology entry after "## M4 — The specification gaps
   (2026-09-19)" and before "## Notes":

   ```markdown
   ## M4 — Removing `:=` (2026-09-19)

   Plan: [ns-next-remove-assign](superpowers/plans/2026-09-19-2125-ns-next-remove-assign.md).
   Branch `feature/remove-assign`, started from `24e414d`, on the checked-out
   tree. Decided with the user on 2026-09-19, while closing the specification
   gaps, as its own small step ahead of closure conversion.

   - `<Task 1 hash>` — every golden and runtime test spelled `:=` is written
     as a setter send, while `:=` was still accepted, so the whole suite
     under both lowerings proved the two spellings answered the same: no
     `.toml` moved.
   - `<Task 2 hash>` — the lexer refuses `:=` with a diagnostic naming the
     setter send, `TokenKind::Assign` is gone, and so are the parser's `:=`
     arm and the two guards that kept a unary loop off an assignment's
     target.
   - `<this commit>` — the record: frontend.md's deviation retired, the
     README row dropped, every document example respelled, and
     `bench_do.ns` written out beside the other three throwaway sources,
     since the 2c plan's copy, an execution record, keeps its `:=`.

   Not measured: nothing on the run loop's path changed, and the measurement
   sources changed spelling only. `parse-corpus` stayed at
   **210/0/1/0** and `run-corpus` at **6/0/205**, plain and under both stress
   modes.
   ```

   Fill in the two hashes from `git log --oneline -3`; the last line names
   this commit in words, since it cannot name its own hash.

- [ ] **Step 6: Check the documents let a fresh session start**

```bash
grep -rnP "(?<!:):=" docs --include=*.md | grep -v superpowers/plans
```

Expected: only `docs/references/newspeak-spec.md` (the transcribed
specification, not ours to edit), `frontend.md`'s new section, the
`execution.md` paragraph naming the 2c plan's `:=`, and `PROGRESS.md`'s
chronology (entries older than today's and the new entry). Read "Where the
next session starts" top to bottom: it must name closure conversion as next
and nothing about `:=` as pending.

- [ ] **Step 7: Run every gate and commit**

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
git add docs
git commit -m "docs: record the removal of :=

frontend.md's deviation becomes the rule, the README row goes, every
design-document example is spelled as a setter send, bench_do.ns is
written out beside the other three throwaway sources, and PROGRESS.md
names closure conversion as next.
"
```

(Append the session's attribution lines to the message.)
