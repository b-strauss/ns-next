# The specification's foundation and syntax Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The first batch of ns-next's own specification: the Markdown book
under `docs/spec/` with its tooling -- rule identifiers, the tests'
references to them, the HTML book `cargo xtask spec` renders, the checks
at every commit -- and its first content, the complete lexical structure
and grammar of the language.

**Architecture:** The book is plain Markdown that mdBook renders. `xtask`
reads every chapter, checks each `r[...]` rule identifier, collects the
references the tests make to them, writes a processed copy of the book
into `target/spec/` -- each identifier an anchor with the tests that name
it, and an appendix of the rules no test names -- and runs `mdbook build`
over that copy. Rust tests in the default suite hold the identifiers, the
references and the book's machine-readable tables to the code.

**Tech Stack:** Rust (`xtask`, `ns-syntax`'s and `ns-cli`'s tests), mdBook
0.5.4 as an installed tool, Markdown.

**Spec:** `docs/design/specification.md` (decisions 1 to 6, "The order of
building it", item 1). The syntax deviations this batch writes into the
book were decided with the user on 2026-11-23, listed under Global
Constraints.

## Global Constraints

- The specification is self-contained: it restates every rule of the
  language it covers; Bracha's document (`docs/references/newspeak-spec.md`,
  cited by its PDF's section) is its source, not its measure.
- A rule ns-next follows from Bracha is restated; a reading `frontend.md`
  already settled ("Where the specification contradicts itself or cannot
  be meant literally", "Words that are not reserved") is adopted; what
  Bracha marks *Unimplemented* and ns-next does not build -- object
  literals (§5.1.10), chains (`:|`), class expressions in methods -- is
  written as a marked gap; nothing undecided is invented.
- The five syntax deviations, each adopted as a normative rule:
  1. The language id and the class category (§8.1) are both optional,
     parsed, and without meaning.
  2. `///` starts a comment to the end of its line wherever it stands
     outside a string or a comment, and a binary selector ends before it;
     no selector contains `///` (§4.2's `binSel`).
  3. There are no character literals and no `Character` class (§5.1.4):
     `"a"` is a one-character string; `"..."` has no escape and ends at
     the next `"`, any single character between two quotes being a
     one-character string, so `"""` holds a quote; `'...'` doubles an
     embedded `'`.
  4. The grammar is a PEG over tokens and whitespace only separates
     tokens: `| public= 1. |` is refused as `| public = 1. |` is
     (§6.3.3, §6.3.4).
  5. `outer N` with no message after it is a valid expression, the *k*th
     enclosing object of `self` with respect to the current class -- a
     load, no send (§5.9).
- Notation: the grammar as a PEG over tokens, beneath it a lexical
  grammar over characters; static errors stated at their rule.
- Every normative rule is preceded by a line `r[<identifier>]` alone on
  its line, outside any fenced code block. An identifier is one or more
  segments joined by `.`, each matching `[a-z][a-z0-9-]*`; the first
  segment names the part: `lex` (lexical structure), `syntax` (grammar),
  and later `semantics`, `runtime`, `bytecode`, `image`, `vm`. Every
  identifier is unique in the book.
- A test names a rule it checks: a golden in its `.toml` as
  `rules = ["syntax.outer.alone"]`, a Rust test in a comment line
  `// spec: syntax.outer.alone` directly above its `#[test]` (one line per
  rule).
- `cargo xtask spec` writes only under `target/spec/`; the source book is
  never rewritten.
- mdBook is version 0.5.4, installed with
  `cargo install mdbook --locked --version 0.5.4`; it reaches no binary.
- Markdown under `docs/` is CRLF in the working tree and LF in the index,
  as every document is; the reader accepts both.
- Conventions of `CLAUDE.md` hold for every name in `xtask`: names written
  out, no identifier of one or two characters.
- No golden's `status`, `stdout` or `stderr` changes; a `.toml` only gains
  its `rules` line.

## Review Focus

- A `r[...]` line inside a fenced code block (a grammar sample, an
  example) is text, not a rule: the reader must skip fenced blocks.
- A reference to an identifier that does not exist -- a typo in a
  golden's `rules` or a `// spec:` comment -- must fail the default suite,
  naming the file and the identifier.
- A chapter file under `docs/spec/` missing from `SUMMARY.md` is silently
  left out of the book by mdBook: the check must refuse it.
- `cargo xtask spec` without mdBook installed, or with another version,
  must fail with the exact install command, not with "program not found".
- The three machine-readable tables -- the special characters, the
  reserved words and the words with tokens of their own -- must fail a
  test when `SPECIAL`, `RESERVED_WORDS` or `word_kind` and the table
  disagree in either direction.

---

## File Structure

- `docs/spec/book.toml` -- mdBook's configuration (title, `src = "."`).
- `docs/spec/SUMMARY.md` -- the table of contents.
- `docs/spec/introduction.md` -- scope, the five parts, the conventions:
  rule identifiers, the notation, what "a gap" means, how tests name rules.
- `docs/spec/language/lexical-structure.md` -- part 1, the tokens.
- `docs/spec/language/grammar.md` -- part 1, the PEG over tokens.
- `docs/spec/language/semantics.md`, `docs/spec/runtime.md`,
  `docs/spec/bytecode.md`, `docs/spec/image-format.md`,
  `docs/spec/vm-invariants.md` -- the parts later batches write, each
  stating what it will cover and that it is not written yet.
- `docs/spec/appendix/differences-from-bracha.md` -- the appendix.
- `xtask/src/spec.rs` -- reading the book, checking identifiers and
  references, rendering into `target/spec/`.
- `xtask/src/main.rs` -- the `spec` task and the commit gate's step.
- `crates/ns-cli/tests/common/mod.rs` -- `Expectation.rules`.
- `crates/ns-syntax/src/lexer.rs`, `crates/ns-syntax/src/parser/mod.rs`
  -- the three table tests, and `// spec:` lines on the lexer's tests.
- `crates/ns-syntax/src/parser/*.rs` -- `// spec:` lines on the parser's
  tests.
- `goldens/language/syntax/*.toml` and the other syntax goldens --
  `rules` lines.
- `CLAUDE.md`, `docs/design/README.md`, `docs/design/frontend.md`,
  `docs/design/specification.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`
  -- the rule, the prerequisite, the gate, the map, the moved deviations.

---

### Task 1: The book's skeleton and `cargo xtask spec`

**Files:**
- Create: `docs/spec/book.toml`, `docs/spec/SUMMARY.md`,
  `docs/spec/introduction.md`, the five part files and the appendix file
  listed above (each with its title and a paragraph on its scope; the
  unwritten parts say they are not written yet and which batch writes
  them, `docs/design/specification.md`, "The order of building it")
- Create: `xtask/src/spec.rs`
- Modify: `xtask/src/main.rs` (the `spec` task, its usage line, and in
  the `commit` group a step `spec book` after `doctests`)
- Test: `xtask/src/spec.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces, in `xtask/src/spec.rs`:
  - `pub struct Rule { pub identifier: String, pub chapter: PathBuf, pub line: usize }`
  - `pub fn rules_in(markdown: &str, chapter: &Path) -> Result<Vec<Rule>, String>`
    -- every `r[...]` line outside fenced blocks, a malformed identifier
    an error naming chapter and line.
  - `pub fn book_rules(book: &Path) -> Result<Vec<Rule>, String>` -- every
    chapter `SUMMARY.md` lists, read in its order; an error for a
    duplicate identifier (naming both places), for a chapter missing
    from `SUMMARY.md`, and for a `SUMMARY.md` entry with no file.
  - `pub struct Reference { pub identifier: String, pub test: PathBuf, pub line: usize }`
  - `pub fn references_in(root: &Path) -> Result<Vec<Reference>, String>`
    -- `rules = [...]` of every `.toml` under `goldens/` and every
    `// spec: <identifier>` line of every `.rs` under `crates/` and
    `xtask/`.
  - `pub fn render(root: &Path, output: &Path) -> Result<(), String>` --
    checks both, writes the processed copy and runs `mdbook build`.
  - `const MDBOOK_VERSION: &str = "0.5.4";`
- Consumes: nothing from other tasks.

- [ ] **Step 1: Write the failing tests**

In `xtask/src/spec.rs`, under `mod tests`:
- `a_rule_line_outside_a_fenced_block_is_a_rule`: `rules_in("intro\r\n\r\nr[lex.comment.line]\r\nA comment ...\r\n", ..)` answers one rule `lex.comment.line` at line 3.
- `a_rule_line_inside_a_fenced_block_is_text`: a ```` ``` ```` block holding `r[lex.x]` yields no rule.
- `a_malformed_identifier_is_refused`: `r[Lex.comment]`, `r[lex..comment]`, `r[lex.]` and `r[9lex]` each an error naming the line.
- `a_duplicate_identifier_is_refused`: a book of two chapters both declaring `syntax.outer.alone` fails naming both chapters.
- `a_chapter_missing_from_the_summary_is_refused`: a book with an extra `.md` not in `SUMMARY.md` fails naming it.
- `every_rule_of_the_book_is_well_formed_and_unique`: `book_rules(repository_root().join("docs/spec"))` is `Ok`.
- `every_reference_names_a_rule_of_the_book`: every `references_in(repository_root())` identifier is one of `book_rules`; a failure names test file, line and identifier.
- `a_reference_to_no_rule_is_reported`: on a temporary tree with one golden `.toml` naming `syntax.nothing`, the check of references against rules fails naming that file.

Use the temporary-directory helper the other `xtask` tests use, or
`std::env::temp_dir()` with a unique subdirectory removed at the end.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p xtask spec`
Expected: FAIL, the functions not defined.

- [ ] **Step 3: Implement the reader, the checks and the rendering**

`render` copies `docs/spec/` into `output/src/` and `book.toml` into
`output/`, replacing each rule line by an anchored block
`<div class="rule" id="r-<identifier>"><a href="#r-<identifier>">[<identifier>]</a> <span class="rule-tests">tests: <list or "none yet"></span></div>`
-- each test a path relative to the repository root -- appends to
`SUMMARY.md` a chapter `untested-rules.md` it generates, listing every
rule no reference names, grouped by chapter, then runs
`mdbook build <output>` with the HTML in `output/html`. Before running it,
`mdbook --version` must print `mdbook v0.5.4`; anything else, or no
program, fails with `the specification needs mdBook 0.5.4: cargo install mdbook --locked --version 0.5.4`.
The `spec` task calls `render(root, root.join("target/spec"))` and prints
the path of `target/spec/html/index.html`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p xtask spec`
Expected: PASS.

- [ ] **Step 5: Install mdBook and build the book**

Run: `cargo install mdbook --locked --version 0.5.4` (ask the user before
installing, as for any tool on their machine), then `cargo xtask spec`.
Expected: `target/spec/html/index.html` exists and lists the five parts,
the appendix and "Untested rules".

- [ ] **Step 6: The commit gate's step**

In `group("commit")`, after `doctests`, `Step::cargo("spec book", &["run", "--quiet", "-p", "xtask", "--", "spec"])`. Extend the existing gate test that
checks the commit group's labels with `spec book`.

Run: `cargo nextest run -p xtask`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add docs/spec xtask/src/spec.rs xtask/src/main.rs
git commit -m "feat: the specification's book and cargo xtask spec"
```

### Task 2: Tests name the rules they check

**Files:**
- Modify: `crates/ns-cli/tests/common/mod.rs` (`Expectation`)
- Modify: `docs/design/README.md` ("The goldens": the `rules` line)
- Test: `crates/ns-cli/tests/run_goldens.rs` or the module's own tests

**Interfaces:**
- Produces: `Expectation.rules: Vec<String>` (`#[serde(default)]`),
  documented as the identifiers of the specification's rules the golden
  checks, read by `xtask`'s `references_in` and by nothing at run time.
- Consumes: Task 1's identifier form.

- [ ] **Step 1: Write the failing test**

`a_golden_may_name_the_rules_it_checks`: an `Expectation` parsed from
`stdout = "1\n"\nrules = ["syntax.outer.alone"]` has `rules ==
["syntax.outer.alone"]`; one from `rule = [...]` is refused, as every
unknown key is (`deny_unknown_fields` stays).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p ns-cli a_golden_may_name_the_rules_it_checks`
Expected: FAIL, unknown field `rules`.

- [ ] **Step 3: Add the field**

- [ ] **Step 4: Run it, and the goldens**

Run: `cargo nextest run -p ns-cli`
Expected: PASS, every golden's outcome unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/ns-cli/tests/common/mod.rs docs/design/README.md
git commit -m "test: a golden names the specification's rules it checks"
```

### Task 3: The lexical structure

**Files:**
- Modify: `docs/spec/language/lexical-structure.md`
- Modify: `crates/ns-syntax/src/lexer.rs` and
  `crates/ns-syntax/src/parser/mod.rs` (three table tests; `// spec:`
  lines on the lexer's tests)
- Modify: the `.toml` of the lexical goldens under
  `goldens/language/syntax/` (comments, numbers and their signs, symbols,
  strings) with `rules`

**Interfaces:**
- Consumes: Task 1's rule form, Task 2's `rules` key.
- Produces: the `lex.*` identifiers the grammar chapter refers to:
  at least `lex.whitespace`, `lex.comment.block`, `lex.comment.line`,
  `lex.comment.metadata`, `lex.identifier`, `lex.reserved-words`,
  `lex.unreserved-words`, `lex.keyword`, `lex.setter-keyword`,
  `lex.special-characters`, `lex.binary-selector`, `lex.number`,
  `lex.number.radix`, `lex.number.sign`, `lex.string.single-quoted`,
  `lex.string.double-quoted`, `lex.string.one-character`,
  `lex.symbol`, `lex.punctuation`.

- [ ] **Step 1: Write the table tests**

In `lexer.rs`'s tests, reading
`include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/spec/language/lexical-structure.md"))`:
- `// spec: lex.special-characters` `the_special_characters_are_the_specifications`: the fenced block with the info string `spec-table special-characters` holds exactly the characters of `SPECIAL`, one per whitespace-separated entry, in any order.
- `// spec: lex.reserved-words` `the_reserved_words_are_the_specifications`, in `parser/mod.rs`'s tests (where `RESERVED_WORDS` lives): the block `spec-table reserved-words` holds exactly `RESERVED_WORDS` -- §4.1's `self`, `super`, `outer`, `true`, `false`, `nil`.
- `// spec: lex.unreserved-words` `the_words_with_tokens_of_their_own_are_the_specifications`, in `lexer.rs`'s tests: the block `spec-table words-with-tokens` holds exactly the words `word_kind` maps to a kind other than `Identifier` -- today `class`, `public`, `protected`, `private`, which are identifiers wherever the grammar takes one (`parser::is_name`; `frontend.md`, "Words that are not reserved"); the test lexes each listed word and asserts it is not `Identifier`, and lexes `lazy` and each reserved word and asserts each is.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-syntax the_special_characters the_reserved_words the_words_with_tokens`
Expected: FAIL, no such block.

- [ ] **Step 3: Write the chapter**

The lexical grammar over characters and the token kinds, one rule per
identifier above, each stating what the lexer produces and what it
refuses, from `frontend.md` ("Lexer", "Comments", "Words that are not
reserved", the string and symbol paragraphs), §4 and §5.1 of Bracha's
document and `lexer.rs`; deviations 2 and 3 of Global Constraints stated
as rules, each with a sentence saying how it differs from Bracha and a
link to the appendix. The tables as fenced blocks with the info strings
`spec-table special-characters`, `spec-table reserved-words` and
`spec-table words-with-tokens`.

- [ ] **Step 4: Name the rules in the tests**

A `// spec:` line above each lexer test for the rule it checks, and
`rules` in each lexical golden's `.toml`.

- [ ] **Step 5: Run the tests and build the book**

Run: `cargo nextest run -p ns-syntax -p xtask -p ns-cli` then `cargo xtask spec`
Expected: PASS; the book shows each `lex.*` rule with its tests.

- [ ] **Step 6: Commit**

```bash
git add docs/spec crates/ns-syntax/src/lexer.rs goldens
git commit -m "docs: the specification's lexical structure"
```

### Task 4: The grammar of modules, classes and members

**Files:**
- Modify: `docs/spec/language/grammar.md` (first half)
- Modify: `crates/ns-syntax/src/parser/declaration.rs`,
  `crates/ns-syntax/src/parser/mod.rs` (`// spec:` lines)
- Modify: the `.toml` of the declaration goldens
  (`goldens/language/syntax/spec_4_1_*`, `spec_6_3_3_*`,
  `goldens/language/module/` for the header) with `rules`

**Interfaces:**
- Consumes: Task 3's `lex.*` identifiers.
- Produces: `syntax.*` identifiers for the compilation unit
  (`syntax.compilation-unit`, `syntax.language-id`), the class
  declaration and its header (factory, superclass clause, `<:` clauses,
  slots sequential and simultaneous, initializer), access modifiers,
  lazy slots, methods, nested classes, the class side, metadata
  comments' places, and the PEG reading (`syntax.peg`).

- [ ] **Step 1: Write the chapter's first half**

The PEG over tokens (`syntax.peg`: an optional part taken whenever it
matches, the first alternative that matches in full winning), then each
production with its rule, from `frontend.md` ("The module header",
"Parser", "Words that are not reserved", the declaration paragraphs),
Bracha's §6 and §8.1 and `parser/declaration.rs`; deviations 1 and 4 as
rules. Static errors the parser reports stated at their production with
the diagnostic's wording where the parser fixes it.

- [ ] **Step 2: Name the rules in the tests**

`// spec:` lines on `parser::declaration`'s tests
(`unreserved_word_tests` among them) and `rules` in the declaration
goldens.

- [ ] **Step 3: Run the tests and build the book**

Run: `cargo nextest run -p ns-syntax -p xtask -p ns-cli` then `cargo xtask spec`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add docs/spec crates/ns-syntax/src/parser goldens
git commit -m "docs: the specification's grammar of modules, classes and members"
```

### Task 5: The grammar of expressions

**Files:**
- Modify: `docs/spec/language/grammar.md` (second half)
- Modify: `crates/ns-syntax/src/parser/*.rs` (`// spec:` lines)
- Modify: the `.toml` of the expression goldens
  (`goldens/language/syntax/` -- precedence, cascades, literals, type
  parameters --, `goldens/language/class/nesting/spec_5_9_outer_n_alone_is_the_enclosing_object.toml`)
  with `rules`

**Interfaces:**
- Consumes: Tasks 3 and 4's identifiers.
- Produces: `syntax.*` identifiers for statements and the return,
  expressions, the setter send (one setter keyword; `:=` refused with
  its diagnostic), unary, binary and keyword sends and their precedence,
  cascades, `self`, `super` (a message required), `outer` (`syntax.outer.send`,
  `syntax.outer.alone`), literals in expressions (a sign only where a
  literal stands, tuples, blocks and their parameters, patterns, symbols),
  type annotations and type parameters, and the marked gaps
  (`syntax.gap.object-literal`, `syntax.gap.chain`).

- [ ] **Step 1: Write the chapter's second half**

From `frontend.md` ("Parser", ":= is refused", "What is kept but
ignored, and what a reprint drops", "Not built"), Bracha's §5 and
`parser/expression.rs`; deviation 5 as a rule; the gaps as rules that
say what Bracha defines and that ns-next refuses or does not build it.

- [ ] **Step 2: Name the rules in the tests**

- [ ] **Step 3: Run the tests and build the book**

Run: `cargo nextest run -p ns-syntax -p xtask -p ns-cli` then `cargo xtask spec`
Expected: PASS; "Untested rules" lists only rules no existing test
checks, each of which is noted in Task 6's report.

- [ ] **Step 4: Commit**

```bash
git add docs/spec crates/ns-syntax/src/parser goldens
git commit -m "docs: the specification's grammar of expressions"
```

### Task 6: The appendix, the rule, and the documents

**Files:**
- Modify: `docs/spec/appendix/differences-from-bracha.md`
- Modify: `docs/design/README.md` (the deviations table: the rows of the
  `///` comment, the character literals, the modifiers' whitespace and
  `outer N` leave it for the appendix, the table's introduction saying
  so; "Gates"; the map's row of the specification)
- Modify: `docs/design/frontend.md` (where it states a rule the book now
  holds, it names the rule's identifier; the deviations it records point
  at the appendix)
- Modify: `CLAUDE.md` (the rule that a change keeps the specification
  with it; mdBook among the prerequisites of "Working in this
  repository"; `spec book` in the `commit` row of the gate table;
  `docs/spec/` and `xtask`'s `spec` task where `xtask/` is described)
- Modify: `docs/design/specification.md` (what is built), `docs/OPEN-WORK.md`
  ("Next" names the second batch), `docs/PROGRESS.md` (the batch)

**Interfaces:**
- Consumes: every identifier of Tasks 3 to 5.

- [ ] **Step 1: Write the appendix**

One section per difference -- the five deviations of Global Constraints
and the gaps of Task 5 -- each saying what Bracha's section says, what
this specification says, the rule's identifier, and why, from the design
document that decided it.

- [ ] **Step 2: The rule in `CLAUDE.md`**

Under "Invariants that are easy to break" or beside the lexer's mirror
rule: a commit that changes observable behaviour, a format or an
invariant changes the specification's rule for it in the same commit,
for every part `docs/spec/` already writes; a new test names the rule it
checks.

- [ ] **Step 3: The documents**

As listed under Files.

- [ ] **Step 4: Run the commit gate**

Run: `cargo xtask gates commit`
Expected: every step passes, `spec book` among them.

- [ ] **Step 5: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: the specification's appendix, and the rule that keeps it"
```

### Task 7: The end of the batch

- [ ] **Step 1: Run the end-of-batch gates**

The batch changed `ns-syntax` and `ns-cli` (their tests), so it is a
batch that changed the runtime: `cargo xtask gates end-of-batch`.
Expected: every group passes. A subagent implementing a task is told
that its commits are work in progress until this step.

- [ ] **Step 2: Finish the batch**

With the `finishing-a-batch` skill: a fresh session can start from the
documentation, `.temp/` deleted, merge into `master` with `--no-ff`,
the branch deleted.
