# Doc comment tags Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A doc comment (a block of two or more `///` lines) carries Java-style block tags -- `@param`, `@answers`, `@signals`, `@see`, and `@factory` for a class's factory -- and an inline `{@summary ...}`; the hover and the editor's rendered view show them as Java's sections, the server warns about an unknown tag and a `@param` naming no parameter, the structure view's tooltip shows the summary, and the whole core library is documented with the tags, which its documentation test enforces.

**Architecture:** One grammar, parsed in two places: `ns_syntax::doc_comment` (Rust: the server's hover and warnings, and the core library's documentation test) and `NewspeakDocTags` in the plugin (Kotlin: the rendered view and the structure view's tooltip), both held to the same cases. The server's hover appends the sections as the HTML table the platform styles for Java (`DocumentationMarkup`'s `sections` classes); the plugin builds the same table for the rendered view. The core library is migrated module by module, each module joining the test's list of tagged modules in the commit that migrates it.

**Tech Stack:** Rust (`ns-syntax`, `ns-analyzer`, `ns-runtime`'s tests), Kotlin (the JetBrains plugin, IntelliJ Platform 2026.1.4+), Newspeak (`crates/ns-runtime/core-library/`).

**Spec:** the decisions taken with the user on 2026-11-01, recorded here (there is no separate spec):

| # | Decision |
|---|---|
| 1 | Parameters: `@param name text` (Java's form), replacing the paragraph of `name: text` lines. |
| 2 | What a method answers: `@answers text`, section "Answers:". |
| 3 | Errors: `@signals ErrorClass text`, one tag per class, section "Signals:". |
| 4 | `@see text`, section "See Also:". |
| 5 | No `@deprecated` for now; recorded in `docs/OPEN-WORK.md`. |
| 6 | No `@since`, `@author`, `@version`; recorded in `docs/OPEN-WORK.md`. |
| 7 | No `[Name]` links; references stay in backticks until go to definition exists; recorded in `docs/OPEN-WORK.md`. |
| 8 | The core library's documentation, `Manifest.ns` included, is migrated in this batch. |
| 9 | `core_library_documentation.rs` also checks: every member's comment is a doc comment (two or more `///` lines); every method and every factory has exactly one `@param` per parameter, in order, naming it; every method has exactly one `@answers`. No check that a tag is known (the server warns instead). |
| 10 | The hover shows the sections as the platform's HTML table (Java's look in IntelliJ; other clients see a plain table). |
| 11 | An unknown tag, and a `@param` naming no parameter of its declaration, are server **warnings**. |
| 12 | Summary: the text of an inline `{@summary ...}` in the description, else the description's first sentence. Shown in the structure view's tooltip (built by the plugin, never in a document symbol's `detail`, whose words the plugin reads); completion, which does not exist yet, is recorded in `docs/OPEN-WORK.md`. |
| 13 | `@factory text` in a class's doc comment describes the class's factory (KDoc's `@constructor`); the class's `@param` tags are the factory's parameters. Required by the documentation test only for a class whose header names its factory; allowed for the default `new`. |
| 14 | The hover over the factory -- its keywords in the header, and a send the resolver binds to it -- shows the factory's signature (`usingPlatform: platform`, or `new`), the `@factory` text as its description, "Params:", and an automatic "Answers: a new instance of `Shapes`."; container `Shapes`'s class side. |
| 15 | The hover over the class's name (and the rendered view) shows the description, then the sections "Factory:" and "Params:" among the others. |

## Global Constraints

- The doc comment rule is unchanged: a block of two or more `///` lines, attached before the declaration and ending on the line right above it (`hover::doc_comment` and `directly_above`; the plugin's `NewspeakDocComments`).
- Names are written out (CLAUDE.md, "Conventions"); clippy warning-free; `.ns` files formatted by `cargo run -p ns-analyzer -- fmt <paths>`, never at the repository root.
- A core-library comment names no other implementation (CLAUDE.md); `Manifest.ns`'s current comment names two and is rewritten.
- Only comments change in `core-library/`; no code, no behaviour. Every golden's outcome stays as recorded.
- Every commit runs `cargo xtask gates commit`; a commit touching `crates/ns-analyzer` also `cargo xtask gates analyzer`. The batch changes `crates/ns-runtime` (the core library) and so ends with `cargo xtask gates end-of-batch` and `plugin-verification` (CLAUDE.md, "The gates"); a subagent implementing a task runs only the per-commit gates and is told the end-of-batch gates come after the last task.
- Commit messages are conventional (`feat:`, `docs:`, `test:`) and end with the session's attribution lines.

## Review Focus

1. A `@` line inside a fenced code block of the description is code, not a tag -- a parser that splits there loses the example. (Task 1 test `a_tag_line_inside_a_code_fence_is_text`.)
2. A `{@summary ...}` whose text holds backticks or a `}` inside backticks, and a first sentence whose `.` sits inside backticks (`` `a.b` ``) -- the summary must not end there. (Task 1 tests `a_summary_ends_at_its_closing_brace_outside_code`, `a_first_sentence_does_not_end_inside_code`.)
3. CRLF sources: the warnings' spans and the hover must hold on `\r\n` lines as on `\n`. (Task 1 test `spans_hold_on_crlf_lines`; Task 3 test `a_warning_is_placed_on_crlf_lines`.)
4. A doc comment above something that is not a method or a class (a slot, a statement) with a `@param`: no parameter list to check against -- warn that a slot has no parameters rather than silently accept or crash. (Task 3 test `a_param_above_a_slot_names_no_parameter`.)
5. The Markdown the hover sends must come through the platform's converter with the table's classes and the cells' Markdown both rendered; if the converter drops either, the fallback in Task 2 applies. (Task 2 Step 1, the plugin test `NewspeakHoverSectionsTest`.)

---

## The grammar (both parsers)

Input: a doc comment's lines without their `///` and the one space after it (`Comment::text`'s lines; the plugin's `NewspeakDocComments.markdown`).

- **Description:** every line before the first tag line.
- **Tag line:** outside a fenced code block (a line whose trimmed text starts with three backticks opens or closes one), a line whose trimmed text starts with `@` followed by one or more ASCII letters and then whitespace or the line's end. The letters are the tag's name.
- **A tag's text:** the rest of its line after the name, then every following line up to the next tag line, joined with `\n`; trailing blank lines dropped, leading whitespace of the first line dropped.
- **Known tags:** `param` and `signals` take their text's first word as an argument (a parameter name; an error class) and the rest as text; `answers`, `see` and `factory` take none. Any other name is an unknown tag: kept, with its text, in order.
- **Inline summary:** in the description, `{@summary ` up to the matching `}`, counting `{`/`}` outside backtick code spans. The rendered description replaces the whole construct by its text.
- **Summary:** the inline summary's text if there is one, else the description's first sentence: up to and including the first `.` outside backtick code spans that is followed by whitespace or ends the description; whitespace runs collapsed to one space; none for an empty description.
- **Rendering order:** description, then the sections Factory, Params (each `` `name` – text ``), Answers, Signals (each `` `Class` – text ``), See Also (each text), each section only when it has a tag. An unknown tag's line is rendered as a paragraph of the description, verbatim, after the description's own text.

## The section table

The platform's markup (`com.intellij.lang.documentation.DocumentationMarkup`): `<table class='sections'>`, each section `<tr><td valign='top' class='section'><p>Params:</td><td valign='top'>` ... `</td>`, then `</table>`. Labels: `Factory:`, `Params:`, `Answers:`, `Signals:`, `See Also:`.

---

### Task 1: The doc comment parser in `ns-syntax`

**Files:**
- Create: `crates/ns-syntax/src/doc_comment.rs`
- Modify: `crates/ns-syntax/src/lib.rs` (`pub mod doc_comment;`, re-export `DocComment`)
- Modify: `crates/ns-syntax/src/comment.rs` (make `lines` usable by the new module; it is `pub(crate)` already)

**Interfaces:**
- Produces:
  - `pub struct DocComment { pub description: String, pub summary: Option<String>, pub tags: Vec<DocTag> }` -- `description` already with the inline summary replaced by its text and unknown tag lines appended as paragraphs.
  - `pub struct DocTag { pub kind: DocTagKind, pub name_span: Span, pub argument: Option<(String, Span)>, pub text: String }` -- `name_span` covers `@name` in the source; `argument` the first word and its span for `Param` and `Signals`, `None` when the line has no word after the name.
  - `pub enum DocTagKind { Param, Answers, Signals, See, Factory, Unknown(String) }`
  - `impl DocComment { pub fn parse(comment: Comment, source: &str) -> DocComment }` -- reads the comment's lines from `source` itself (not `Comment::text`) so that spans are source offsets; a `Block` comment answers an empty `DocComment`.

- [ ] **Step 1: Write the failing tests** in `doc_comment.rs`'s test module, each on an `indoc!` source holding one `///` block (parse the first comment of `lexer::Lexer::new(source).run_keeping_comments()`):
  - `a_comment_without_tags_is_all_description` -- description equals `Comment::text`, no tags.
  - `a_factory_tag_has_no_argument` -- `@factory makes one.` gives `Factory` with text `"makes one."`.
  - `tags_split_the_description_off` -- `/// Evaluates.\n///\n/// @param stop a number.\n/// @param action a block,\n/// sent value:.\n/// @answers nil.\n/// @signals ArgumentError when stop is no number.\n/// @see `to:by:do:`` → description `"Evaluates."`; params `stop`/`"a number."` and `action`/`"a block,\nsent value:."`; answers `"nil."`; signals `ArgumentError`/`"when stop is no number."`; see `` "`to:by:do:`" ``.
  - `an_unknown_tag_is_kept_in_order` -- `@retrun nil.` gives `Unknown("retrun")` with text `"nil."`, and the description ends with the paragraph `"@retrun nil."`.
  - `a_param_without_a_name_has_no_argument`.
  - `an_at_sign_inside_a_line_is_text` -- `/// mail me @home` stays description.
  - `a_tag_line_inside_a_code_fence_is_text`.
  - `the_summary_is_the_first_sentence` -- `"Answers the size. Never nil."` → `"Answers the size."`; a description without a `.` → the whole first paragraph collapsed.
  - `a_first_sentence_does_not_end_inside_code` -- `` "Sends `a.b` first. Then more." `` → `` "Sends `a.b` first." ``.
  - `an_inline_summary_wins_and_reads_as_its_text` -- `"{@summary The size.} Answers the size of the receiver."` → summary `"The size."`, description `"The size. Answers the size of the receiver."`.
  - `a_summary_ends_at_its_closing_brace_outside_code` -- `` "{@summary Uses `{x}` here.} More." `` → `` "Uses `{x}` here." ``.
  - `spans_hold_on_crlf_lines` -- the same source with `\r\n`: `name_span` and the argument's span slice to `@param` and `stop`.
- [ ] **Step 2: Run** `cargo nextest run -p ns-syntax doc_comment` -- Expected: FAIL, the module does not exist.
- [ ] **Step 3: Implement** `DocComment::parse` by the grammar above, walking the source lines of `comment.span` with `comment::lines`, stripping each line's indentation, `///` and one space, and tracking each line's start offset for the spans.
- [ ] **Step 4: Run** `cargo nextest run -p ns-syntax doc_comment` -- Expected: PASS; then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat(syntax): a doc comment's tags and summary`.

### Task 2: The hover shows the description, the sections and the summary's text, and the factory has its own

**Files:**
- Modify: `crates/ns-analyzer/src/hover.rs` (`markdown`; `DeclaredKind::Factory`), `crates/ns-analyzer/src/semantic_tokens.rs` (the factory's declaration and references)
- Create: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/editor/NewspeakHoverSectionsTest.kt`

**Interfaces:**
- Consumes: `ns_syntax::doc_comment::{DocComment, DocTag, DocTagKind}` (Task 1).
- Produces: the hover Markdown: signature fence, `\n\n`, the description, `\n\n`, the section table (when any tag), `\n\n*In* ...` (when a container). The table's form is fixed by Step 1.
- Produces: `DeclaredKind::Factory`, declared per class under the key `factory_span(class)`: the span from the factory pattern's first keyword (or selector) to its last parameter's end, or, for the default `new`, the empty span at the class name's start. `Declared` gains `class: Option<Span>` -- the class's declaration span -- set for a factory, from which `markdown` reads the class's doc comment and name. The class side's factory member (`ClassMembers::of`) declares this key instead of the class's span; the header's factory keywords are emitted as references to it (category `Method`, declaration `true`).

- [ ] **Step 1: Settle the table's form against the platform's converter.** Write `NewspeakHoverSectionsTest` (a `BasePlatformTestCase`): it converts, with `DocMarkdownToHtmlConverter.convert(project, markdown, NewspeakLanguage)`, the Markdown `"Evaluates.\n\n<table class='sections'>\n<tr><td valign='top' class='section'><p>Params:</td><td valign='top'>\n\n`stop` – a number.\n\n</td></tr>\n</table>"` and asserts the output contains `class='sections'` (or `class="sections"`) and matches `<code>(<span[^>]*>)?stop(</span>)?</code>`. Run `.\gradlew.bat --no-daemon test --tests "*NewspeakHoverSectionsTest"` from `editors/jetbrains`. If it passes, the server writes the cells' text as Markdown between blank lines, exactly in that form. If it fails, the server writes each cell as HTML itself: `&`, `<`, `>` escaped, backtick spans as `<code>...</code>`, `*x*` as `<i>x</i>`, `**x**` as `<b>x</b>`, paragraphs as `<p>`, no blank lines inside the table; change the test's sample to that form and keep it as the pin.
- [ ] **Step 2: Write the failing hover tests** in `hover.rs`'s tests, on a module whose `grow: amount by: step` carries `@param amount ...`, `@param step ...`, `@answers ...`, `@signals ArgumentError ...`, `@see ...` and a `{@summary ...}`:
  - `the_tags_become_sections_after_the_description` -- the exact expected Markdown in Step 1's form, labels `Params:`, `Answers:`, `Signals:`, `See Also:`, each param `` `amount` – … ``, the error `` `ArgumentError` – … ``.
  - `a_comment_without_tags_has_no_table` -- existing expectations unchanged.
  - `the_inline_summary_reads_as_its_text`.
  - `the_factory_shows_its_signature_its_text_its_params_and_answers` -- over `usingPlatform:` in the header and over a class-side `^usingPlatform: nil`: fence `usingPlatform: platform`, the `@factory` text, a table with `Params:` (the class's `@param`) and `Answers:` `` a new instance of `Shapes`. ``, then `` *In* `Shapes`'s class side ``.
  - `the_default_factory_is_new` -- a class without a factory in its header, a class-side `^new`: fence `new`, its `@factory` text when the class has one.
  - `the_class_shows_its_factory_section` -- over the class name: the table holds `Factory:` before `Params:`.
  - Update the existing `hover.rs` tests only where their doc comments now carry tags.
- [ ] **Step 3: Run** `cargo nextest run -p ns-analyzer hover` -- Expected: FAIL.
- [ ] **Step 4: Implement** in `markdown`: parse the documentation with `DocComment::parse`, write its description, then the table in Step 1's form; the container line stays last. For a factory, read the class's doc comment: its `@factory` text as the description, its `@param` tags and the fixed `Answers:` row as the table.
- [ ] **Step 5: Run** `cargo nextest run -p ns-analyzer hover` and the plugin test -- Expected: PASS; then `cargo xtask gates commit analyzer`.
- [ ] **Step 6: Commit** -- `feat(analyzer): a hover shows a doc comment's tags as sections`.

### Task 3: Warnings for an unknown tag and a `@param` naming no parameter

**Files:**
- Create: `crates/ns-analyzer/src/doc_tags.rs`
- Modify: `crates/ns-analyzer/src/lib.rs` (`pub mod doc_tags;`), `crates/ns-analyzer/src/analysis.rs` (extend `findings`)

**Interfaces:**
- Consumes: `DocComment::parse` (Task 1); `hover::doc_comment` (existing) for which declaration a doc comment documents.
- Produces: `pub fn findings(module: &ast::Module, comments: &[Comment], text: &str) -> Vec<Finding>` with `severity: Severity::Warning`.

Messages, exactly:
- unknown tag, at its `name_span`: ``unknown doc comment tag `@retrun`; the tags are @param, @answers, @signals, @see and @factory``
- `@factory` in a comment that is no class's doc comment, at its `name_span`: ```@factory` describes a class's factory: the comment documents a method`` (`a slot`, or `nothing` for a comment no declaration owns)
- `@param` without a name, at its `name_span`: ```@param` needs the name of a parameter``
- `@signals` without a class, at its `name_span`: ``@signals` needs the class of the Error signalled``
- `@param` naming none of a method's or factory's parameters, at the name's span: ```amout` is no parameter of `grow:by:` ``; above a slot or anything without parameters: ```amout` is no parameter: the comment documents a slot`` (`a class without a factory` for a class without one).

Every doc comment of the file (`Line` comments of two or more lines in `comments`) is checked for the first three; the `@param` names only where the comment is a declaration's doc comment by the hover's rule (attached `Before`, ending on the line right above). Walk classes (both sides, nested) and their members iteratively or by recursion over nesting only (no expression walk is needed).

- [ ] **Step 1: Write the failing tests** in `doc_tags.rs`: `an_unknown_tag_is_a_warning` (span and message), `a_factory_tag_outside_a_class_is_a_warning`, `a_tag_without_its_argument_is_a_warning` (both forms), `a_param_naming_no_parameter_is_a_warning` (method; and a class's factory), `a_param_above_a_slot_names_no_parameter`, `a_single_line_comment_is_not_checked`, `a_warning_is_placed_on_crlf_lines`, `a_correct_comment_has_none`.
- [ ] **Step 2: Run** `cargo nextest run -p ns-analyzer doc_tags` -- Expected: FAIL.
- [ ] **Step 3: Implement** `findings` and append them in `analyze_unguarded` after the resolver's.
- [ ] **Step 4: Run** -- Expected: PASS; `crates/ns-analyzer/src/analysis.rs`'s test `a_clean_module_has_no_findings` still passes; then `cargo xtask gates commit analyzer`.
- [ ] **Step 5: Commit** -- `feat(analyzer): warnings for an unknown doc tag and a param naming nothing`.

### Task 4: The plugin renders the tags and shows the summary

**Files:**
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/editor/NewspeakDocTags.kt`
- Modify: `editor/NewspeakInlineDocumentationProvider.kt` (`renderText`), `editor/NewspeakDocComments.kt` (`summaryAbove`), `structure/NewspeakStructureElement.kt` and its factory (the tooltip)
- Test: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/editor/NewspeakDocTagsTest.kt`, extend `NewspeakInlineDocumentationProviderTest.kt` and `structure/NewspeakStructureElementTest.kt`

**Interfaces:**
- Produces:
  - `data class DocTag(val name: String, val argument: String?, val text: String)` (`name` without `@`; `factory` among the known names) and `data class DocComment(val description: String, val summary: String?, val tags: List<DocTag>)`, `object NewspeakDocTags { fun parse(markdown: String): DocComment; fun sectionsHtml(comment: DocComment, render: (String) -> String): String }` -- `render` turns a tag's Markdown into HTML (the converter), `sectionsHtml` answers `""` without known tags and otherwise the table built from `DocumentationMarkup`'s `SECTIONS_START`, `SECTION_HEADER_START`, `SECTION_SEPARATOR`, `SECTION_END`, `SECTIONS_END`.
  - `NewspeakDocComments.summaryAbove(file: PsiFile, line: Int): String?` -- the summary of the doc comment whose last line is `line - 1`.

- [ ] **Step 1: Write the failing tests:** `NewspeakDocTagsTest` (plain JUnit) with the same cases as Task 1's tests but the spans (each case named alike); in `NewspeakInlineDocumentationProviderTest`, `testTheTagsRenderAsSections` (output contains `class='sections'`, `Params:`, the param's `<code>` and no `@param`); in the structure tests, `testATooltipShowsTheSummary` (a method with a two-line doc comment: tooltip is the existing text, `<br>`, the summary, wrapped in `<html>`) and that a member without a doc comment keeps its tooltip unchanged.
- [ ] **Step 2: Run** `.\gradlew.bat --no-daemon test --tests "*DocTags*" --tests "*InlineDocumentationProvider*" --tests "*StructureElement*"` -- Expected: FAIL.
- [ ] **Step 3: Implement** the parser (the grammar above, line by line, without spans), `renderText` as `convert(description)` followed by `sectionsHtml(...)`, `summaryAbove`, and the tooltip: where the structure element is built from a `DocumentSymbol`, look up `summaryAbove(file, symbol.range.start.line)` and, when present, make the tooltip `"<html>" + escaped(tooltip) + "<br>" + escaped(summary) + "</html>"` (`StringUtil.escapeXmlEntities`).
- [ ] **Step 4: Run** the tests -- Expected: PASS; then `cargo xtask gates commit`.
- [ ] **Step 5: Commit** -- `feat(jetbrains): doc tags rendered as sections, the summary in the structure view`.

### Task 5: The documentation test checks the tags of the tagged modules

**Files:**
- Modify: `crates/ns-runtime/tests/core_library_documentation.rs`

**Interfaces:**
- Consumes: `ns_syntax::doc_comment::DocComment` (Task 1); `ns-runtime` already dev-depends on `ns-syntax` (it uses `ns_syntax::parse` there).
- Produces: `const TAGGED_MODULES: &[&str] = &[];` -- each migration task adds its module; Task 11 removes the list and checks every module of `DOCUMENTED_MODULES`.

Checks, for every member of a tagged module (`member_starts` gains the member's kind and parameter names: a method's pattern parameters, a class's factory parameters, none for a slot):
- the comment directly above it has two or more `///` lines;
- the `@param` tags name exactly the parameters, once each, in pattern order;
- a method has exactly one `@answers`;
- a class whose header names its factory has exactly one `@factory`; no member but a class has one.
The failure lists `module: member: what is wrong`, one line each, as the existing test does.

- [ ] **Step 1: Write the test** `every_member_of_a_tagged_module_has_its_tags` and, on an inline `indoc!` module, `the_tag_check_finds_each_fault` (a one-line comment, a missing, an extra, a misordered `@param`, a missing and a doubled `@answers`, a class with a factory but no `@factory`, a `@factory` above a method), so the checker is tested before any module is listed.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime --test core_library_documentation` -- Expected: PASS (no module tagged yet; the fault test passes once the checker exists).
- [ ] **Step 3: Commit** -- `test: the core library's doc tags are checked module by module`.

### Migration rules (Tasks 6 to 11)

Only comments change. For every class, slot and method of the module:
1. A paragraph of `name: text` lines becomes `@param name text` tags, in the pattern's order; continuation lines stay plain lines under their tag. A class's factory parameters (`mirror: ...` in `Kernel`'s class comment) likewise.
2. The sentence(s) saying what a method answers become `@answers text`, lower case after the tag (`@answers nil.`, `@answers whether …, a Boolean.`). The description keeps what the method does; where it was only the "Answers …" sentence, write a description sentence of its own (it is the summary) rather than repeat `@answers` verbatim. Every method has one, `@answers the receiver.` included.
3. Each "Signals X when/for …" becomes one `@signals X …` per error class; a sentence about what is not signalled stays in the description.
4. A description that begins "As `other`, …" keeps that sentence and gains `@see `other``; elsewhere `@see` only where the text already points the reader to another member.
5. A class whose header names its factory gets `@factory text` saying what the factory makes and does with its arguments, taken from the class comment where it said so; the class's `@param` tags describe the factory's parameters. Allowed, not required, for a class with the default `new`.
6. Tags after the description and one blank `///` line, in the order `@factory`, `@param`, `@answers`, `@signals`, `@see`; lines at most 100 columns with indentation.
7. A slot's or class's one-line comment gains a second line of real content (what it holds, what it guarantees), never filler.
8. A comment names no other implementation.
9. After editing: `cargo run -p ns-analyzer -- fmt <file>`; add the module to `TAGGED_MODULES` (and, for `Manifest.ns`, to `DOCUMENTED_MODULES`); `cargo nextest run -p ns-runtime --test core_library_documentation`; `cargo xtask gates commit`; commit `docs(core): <module> documented with doc tags`.

### Task 6: `Manifest.ns`, `TranscriptFactory.ns`, `TimeFactory.ns`

`Manifest.ns` is documented in full first (its slots and `doesNotUnderstand:` have no comment, and its class comment names two other implementations), then all three by the rules. One commit per module.

### Task 7: `PlatformFactory.ns`

### Task 8: `CollectionsFactory.ns`

### Task 9: `MirrorsFactory.ns` and `TypedArraysFactory.ns`

One commit per module.

### Task 10: `Kernel.ns`

About 3,300 doc lines: migrate by nested class in source order, committing in parts (each part a commit that passes the gates without `Kernel.ns` in `TAGGED_MODULES`), and add it to the list in the last part.

### Task 11: Every documented module tagged; the documentation

**Files:**
- Modify: `crates/ns-runtime/tests/core_library_documentation.rs` (drop `TAGGED_MODULES`; the tag checks run over `DOCUMENTED_MODULES`, `Manifest.ns` among them)
- Modify: `CLAUDE.md` ("Every member of a shipped core-library module is documented": the comment is a doc comment of two or more `///` lines with `@param` per argument in order, `@answers`, `@signals` per Error class and `@see`, and a class whose header names its factory `@factory`; `Manifest.ns` in the list; the test holds the tags), `docs/design/editor-support.md` (the grammar, the hover's sections and their form, the factory's hover, the rendered view's sections, the warnings, the summary and the structure view's tooltip; the testing list), `docs/design/core-library.md` (wherever it states the documentation convention), `docs/OPEN-WORK.md` (decisions 5, 6, 7, completion showing the summary, and `{@inheritDoc}`, which was not decided)

- [ ] **Step 1:** Edit the test and run `cargo nextest run -p ns-runtime --test core_library_documentation` -- Expected: PASS.
- [ ] **Step 2:** Edit the documents; `cargo xtask gates commit analyzer`.
- [ ] **Step 3: Commit** -- `docs: doc comment tags`.
- [ ] **Step 4:** The end of the batch: `cargo xtask gates end-of-batch` and `cargo xtask gates plugin-verification`, then the `finishing-a-batch` skill.
