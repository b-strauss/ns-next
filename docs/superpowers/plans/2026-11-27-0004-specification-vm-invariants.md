# The specification's fifth batch: the VM invariants -- Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Write the book's last part, `docs/spec/vm-invariants.md`, every
internal rule whose breach silently falsifies a rule of parts 1 to 4, each
saying what it protects and what finds its breach; and make the design
documents and `CLAUDE.md` point at the book's rules by a checked
`` `r[…]` ``.

**Architecture:** `cargo xtask spec` (`xtask/src/spec.rs`) gains three
checks -- the pointers in `CLAUDE.md` and `docs/design/*.md`, the `Protects:`
line of every `vm.*` rule, and the table of detectors with the obligation
that every `vm.*` rule has a test, a detector or an `Undetected:` line --
and renders a rule's detectors beside its tests. The chapter is written
from an audit of every design document, section by section; the design
documents and `CLAUDE.md` then shrink their own statements of the rules to
pointers.

**Tech Stack:** Rust (xtask, edition 2024), mdBook 0.5.4, Markdown.

**Spec:** `docs/design/specification.md`, "The fifth batch's decisions"
(seven decisions, 2026-11-26), and its decisions 1 to 7 above them; the
book's conventions in `docs/spec/introduction.md`.

## Global Constraints

- The chapter names no Rust type, function, file or crate (decision 2); its terms are the ones its first section defines.
- Every `vm.*` rule is declared in `docs/spec/vm-invariants.md` and nowhere else.
- Every `vm.*` rule has a `Protects:` line whose links, followed through `vm.*` rules, reach a rule of `lex`, `syntax`, `semantics`, `runtime`, `bytecode` or `image` (decision 3).
- Every `vm.*` rule has a `// spec:` test, a detector row, or an `Undetected:` line with its reason -- never `Undetected:` beside a test or a detector (decision 4).
- A pointer is `` `r[<identifier>]` `` in backticks, outside a fenced block, in `CLAUDE.md` or `docs/design/*.md` (decision 7).
- Names are written out (`CLAUDE.md`, "Conventions"); no identifier of one or two characters.
- A web link is never wrapped or shortened.
- Conventional commits; every commit ends with the session's `Co-Authored-By` and `Claude-Session` lines.
- A commit that changes more than Markdown runs `cargo xtask gates commit` first and passes it; a Markdown-only commit runs `cargo nextest run -p xtask` and `cargo xtask spec`.
- The batch changes `crates/ns-runtime` (tests), so it ends with `cargo xtask gates end-of-batch`; a commit before that is work in progress -- a subagent is told so.
- Subagents run every command in the foreground.

## Review Focus

1. A `` `r[…]` `` inside a fenced code block or a sample of the book's conventions -- must be ignored, not refused (Task 1, `a_pointer_in_a_fenced_block_is_a_sample`).
2. A bare file name in backticks such as `` `vm.rs` `` -- must never be read as a pointer (Task 1, `a_bare_identifier_is_no_pointer`).
3. A stress mode added to `STRESS_MODES` without a row in the table, or a row naming a mode the gate does not run -- refused naming the mode (Task 2, `a_stress_mode_without_a_row_is_refused`, `a_row_naming_no_stress_mode_is_refused`).
4. `vm.*` rules whose `Protects:` lines point only at each other in a cycle -- refused, since nothing outside the part is protected (Task 2, `a_protection_cycle_among_vm_rules_is_refused`).
5. A rule marked `Undetected:` that also has a test or a detector -- refused as contradictory (Task 2, `an_undetected_rule_with_a_detector_is_refused`).

---

### Task 1: The pointers' check

**Files:**
- Modify: `xtask/src/spec.rs` (new `Pointer`, `pointers_in`, `check_pointers`; `render` calls both; tests in its `mod tests`)
- Modify: `docs/design/specification.md:101` (the sample `r[runtime.lookup.public]` → `r[runtime.lookup]`)
- Modify: every `docs/design/*.md` and `CLAUDE.md` bare reference to a rule (37 today: `` `image.bound.sizing` `` → `` `r[image.bound.sizing]` ``)

**Interfaces:**
- Produces:
  - `pub struct Pointer { pub identifier: String, pub document: PathBuf, pub line: usize }`
  - `pub fn pointers_in(root: &Path) -> Result<Vec<Pointer>, String>` -- reads `root/CLAUDE.md` and every `root/docs/design/*.md` (not below), takes every `` `r[` ``…`` ]` `` span outside a fenced block (`walk_markdown`'s `outside`), refuses `"{document}:{line}: `r[{text}]` is no rule identifier"` for a malformed one.
  - `pub fn check_pointers(rules: &[Rule], pointers: &[Pointer]) -> Result<(), String>` -- refusal text `"{document}:{line}: `r[{identifier}]` names no rule of the specification"`, one line each, joined by `\n`.

- [ ] **Step 1: Write the failing tests** in `xtask/src/spec.rs`'s `mod tests`, each over a `Scratch` with a one-rule book (`docs/spec/SUMMARY.md`, a chapter with `r[vm.sample]`):
  - `a_pointer_to_a_rule_of_the_book_is_accepted`: `docs/design/a.md` holding `` see `r[vm.sample]` `` → `check_pointers` is `Ok`, `pointers_in` answers one pointer at line 1.
  - `a_pointer_to_no_rule_is_refused`: line 3 holds `` `r[vm.none]` `` → error equals ``docs/design/a.md:3: `r[vm.none]` names no rule of the specification``.
  - `a_pointer_in_a_fenced_block_is_a_sample`: the same pointer inside a ```` ``` ```` fence → `pointers_in` answers none.
  - `a_bare_identifier_is_no_pointer`: `` `vm.rs` `` and `` `image.bound.sizing` `` → none.
  - `a_malformed_pointer_is_refused`: `` `r[Vm.X]` `` in `CLAUDE.md` → error ``CLAUDE.md:1: `r[Vm.X]` is no rule identifier``.
  - `every_pointer_names_a_rule_of_the_book` over `repository_root()` (beside `every_reference_names_a_rule_of_the_book`).
- [ ] **Step 2: Run** `cargo nextest run -p xtask pointer` -- expected: compile failure (`pointers_in` undefined).
- [ ] **Step 3: Implement** `Pointer`, `pointers_in`, `check_pointers`; call them in `render` after `check_references`.
- [ ] **Step 4: Run** `cargo nextest run -p xtask` -- expected: the repository test fails on `specification.md:101`; fix the sample, then rewrite the bare references. Find them with `grep -rnoE '`(lex|syntax|semantics|runtime|bytecode|image|vm)\.[a-z0-9-]+(\.[a-z0-9-]+)*`' CLAUDE.md docs/design/*.md`, leaving out file names (`.rs`, `.md`, `.ns`, `.toml`, `.json`, `.bin`) and anything that names no rule of the book. Rerun: PASS.
- [ ] **Step 5: Gate and commit** -- `cargo xtask gates commit`, then `git commit -m "feat: the design documents' and CLAUDE.md's pointers at the book's rules are checked"`.

### Task 2: The `vm.*` rules' checks, the detectors and their rendering

**Files:**
- Modify: `xtask/src/spec.rs` (new items below; `listed_tests` becomes `listed_checks`; `untested_rules` and `render` take the detectors)
- Modify: `xtask/src/main.rs` only if `STRESS_MODES` must be reached from `spec.rs` (it is visible to `crate::spec` as it stands).

**Interfaces:**
- Consumes: `Rule`, `Reference`, `book_rules`, `references_in`, `walk_markdown` (existing).
- Produces:
  - `pub enum DetectorKind { Stress, Debug, Compiler }`, spelled `stress`, `debug`, `compiler` in the table.
  - `pub struct Detector { pub name: String, pub kind: DetectorKind, pub rules: Vec<String>, pub line: usize }`
  - `pub fn detectors_in(markdown: &str, chapter: &Path) -> Result<Vec<Detector>, String>` -- the one fenced block whose info string is `spec-table vm-detectors`, a Markdown table with the header `| Detector | Kind | What it checks | Rules |`; `Rules` is a comma-separated list of backticked identifiers. No block answers an empty list.
  - `pub fn check_detectors(rules: &[Rule], detectors: &[Detector], stress_modes: &[&str]) -> Result<(), String>` -- each row's rules exist and begin with `vm.`; each of `stress_modes` has exactly one `stress` row of its name; no `stress` row names another.
  - `pub fn check_invariants(book: &Path, rules: &[Rule], references: &[Reference], detectors: &[Detector]) -> Result<(), String>` -- over `book/vm-invariants.md`: a rule's span runs from its line to the next rule line or heading. For each `vm.*` rule: a line beginning `Protects:` in its span with at least one `#r-<identifier>` link; every `vm.*` rule reaches a non-`vm` rule through those links; at least one of a reference, a detector row naming it, a line beginning `Undetected:`; not `Undetected:` together with either. Also refuses a `vm.*` rule declared in any other chapter.
  - `render` passes `crate::STRESS_MODES` to `check_detectors` and calls `check_invariants`.

- [ ] **Step 1: Write the failing tests** (each over a small `Scratch` book whose `vm-invariants.md` holds the rules in question and a `semantics.md` with `r[semantics.sample]`), with the refusal each must contain:
  - `a_vm_rule_without_protects_is_refused` -- `vm-invariants.md:<line>: `vm.a` says nothing it protects`.
  - `a_protection_cycle_among_vm_rules_is_refused` -- `vm.a` protects `vm.b`, `vm.b` protects `vm.a` → `` `vm.a` protects no rule outside the VM invariants `` (and the same for `vm.b`).
  - `a_vm_rule_protecting_through_another_is_accepted` -- `vm.a` → `vm.b` → `semantics.sample`, both with a detector → `Ok`.
  - `a_vm_rule_with_nothing_that_checks_it_is_refused` -- `` `vm.a` has no test, no detector and no `Undetected:` line ``.
  - `an_undetected_rule_with_a_detector_is_refused` -- `` `vm.a` is marked undetected but has a detector or a test ``.
  - `an_undetected_rule_alone_is_accepted`.
  - `a_stress_mode_without_a_row_is_refused` -- `stress_modes = ["MOVES", "JIT"]`, only `MOVES` has a row → `the stress mode JIT has no row in the table of detectors`.
  - `a_row_naming_no_stress_mode_is_refused` -- a `stress` row `ZONES` → `vm-invariants.md:<line>: ZONES is no stress mode the stress gate runs`.
  - `a_detector_naming_no_vm_rule_is_refused` -- row rules `` `semantics.sample` `` → `` `semantics.sample` is no rule of the VM invariants ``.
  - `a_vm_rule_outside_its_chapter_is_refused` -- `r[vm.stray]` in `semantics.md` → `` `vm.stray` is declared outside the VM invariants ``.
  - `a_rules_detectors_are_listed_beside_its_tests` -- `processed_chapter` of a rule with one Rust test and the detector `SCAVENGE` (stress) yields `<summary>1 test, 1 detector</summary>` and `<li><code>SCAVENGE</code><span class="test-file">stress mode</span></li>`; a `debug` row shows `debug build`, a `compiler` row `Rust compiler`.
  - `an_undetected_rule_shows_so` -- the summary is `tests: none, undetected` (a `<span class="rule-tests">`, as `none yet` is today).
  - `a_rule_with_a_detector_is_not_untested` -- `untested_rules` leaves it out; an `Undetected:` rule stays in, its line ending ` (undetected)`.
  - `every_vm_rule_protects_and_is_checked` over `repository_root()`.
- [ ] **Step 2: Run** `cargo nextest run -p xtask` -- expected: compile failure on the new names.
- [ ] **Step 3: Implement** the items above; the repository test passes while the chapter has no rules, except `check_detectors`, which needs the table: add to `docs/spec/vm-invariants.md` the empty `spec-table vm-detectors` block with its header and one `stress` row per mode, each with `Rules` empty for now. `check_detectors` accepts an empty `Rules` until Task 4 (Task 4's step 3 makes it refuse one).
- [ ] **Step 4: Run** `cargo nextest run -p xtask` and `cargo xtask spec` -- PASS, the book renders.
- [ ] **Step 5: Gate and commit** -- `cargo xtask gates commit`; `feat: the VM invariants' rules say what they protect and what finds their breach`.

### Task 3: The audit

**Files:**
- Create: `.temp/vm-invariants/inventory.md` (not committed; `.temp/` is gitignored)

**Interfaces:**
- Produces: the inventory Task 4 writes the chapter from -- one entry per invariant, in the chapter's section order (model, heap and collector, handles and the brand, activations and frames, the compiler, dispatch and the caches, machine code):

  ```
  ## vm.<section>.<name>
  Statement: <the rule in the chapter's terms, one to three sentences>
  Protects: <rule identifiers of parts 1-4, or vm.* ones>
  Source: <design document, section heading> [; CLAUDE.md, "<entry>"]
  Tests: <file::test that checks it, or none>
  Detectors: <stress mode / debug check / compiler check, or none>
  Undetected: <reason, only when no test and no detector can exist>
  ```

- [ ] **Step 1: Read** every document under `docs/design/` section by section -- `memory.md`, `execution.md`, `compiler.md`, `dispatch.md`, `object-model.md`, `image-format.md` first, then the other nine -- and `CLAUDE.md`, "Invariants that are easy to break". An internal rule enters the inventory when its breach makes a rule of parts 1 to 4 false and no program is told why (silent wrong output, a stale handle, a kept or lost object, a wrong frame). A rule a program observes is not one: it is, or belongs in, parts 1 to 4 -- list it under a closing `## Already rules` with its identifier. A rule of the repository's work (the goldens pinned, no `{:?}` of a `Value`, a chain walked link by link, the architecture split of `jit/`) is not one either -- list it under `## Not invariants` with one line why.
- [ ] **Step 2: Seed** from `CLAUDE.md`'s internal entries, at least: the write barrier and its exemptions; the born-remembered old allocation; realigned aligned data; collections only at safepoints or between entries; only strong words traced, weak slots cleared, ephemerons mourned; every pointer word in its format's ranges; handles meaning nothing across heaps or moves, a host keeping a value only as a root; the brand; a stored activation is a frame object, every activation popped on every exit; outer reach answered for every instruction reaching another frame; an unwind target a frame object, its activation asked afresh; dead registers nilled before every trace; a value's register live until its consumer; compilation total; slot indices mixin-relative and a slot access decided without the receiver guarded; one lookup for every executor; a cache records the lookup and reads afresh what its key does not cover; the application cache keyed by the superclass's application; the six locks of the VM mirror; and machine code's rules -- template stores after the last exit, what a leaf may do, re-execution, lean bodies and their twins, the cut, a patched slot an exit, optimized code's frame word for word at each interrupt point and its entries, the entry block's lasting words, a cell written from its entry, machine entry rows and dispatching rows, the zone compacted only at a safepoint and remapped, no word between two leaf calls inside the mutator, `enter_called` at every called address, an optimized copy freed when what it inlined changes.
- [ ] **Step 3: Check completeness** by listing, for each of the six first documents, every section heading with the entries it gave or `none`; append that list to the inventory as `## Coverage`.
- [ ] **Step 4: Hand the inventory to the controller**, who reads it against three sections chosen at random from `## Coverage` before Task 4 starts. No commit.

### Task 4: The chapter

**Files:**
- Modify: `docs/spec/vm-invariants.md` (replace the stub)
- Modify: the Rust test files the inventory's `Tests:` lines name -- one `// spec: vm.…` line directly above each `#[test]`
- Modify: `xtask/src/spec.rs` (an empty `Rules` cell refused from now on: `the detector <name> names no rule`)

**Interfaces:**
- Consumes: Task 3's inventory; Task 2's table format and line forms `Protects:` / `Undetected:`.
- Produces: every `vm.*` identifier Tasks 5 to 7 point at.

- [ ] **Step 1: Write the chapter**: an opening paragraph (what the part is, that it names no code, that the design documents say where each rule is enforced); `## The model`, the terms as definitions, no `r[…]`; then one `##` section per inventory section, each rule as `r[vm.<section>.<name>]`, its statement, and its `Protects:` line of links (`[`semantics.slot.mutable`](language/semantics.md#r-semantics.slot.mutable)`, `[`vm.heap.barrier`](#r-vm.heap.barrier)`), and an `Undetected:` line where the inventory gives one; last, `## Detectors`, the `spec-table vm-detectors` filled: the fifteen stress rows, a `debug` row for each check of a debug build the inventory names (the poison word in dead registers and over a vacated nursery, the origin guard, the move epochs, the entry block's check at every entry, both lowerings' assertions, the leaf view's reification assertion …), a `compiler` row for each check the Rust compiler makes (the exhaustive matches over instructions, the brand's `compile_fail` doctests, the types without `Debug`).
- [ ] **Step 2: Add the `// spec:` lines** the inventory's `Tests:` name.
- [ ] **Step 3: Refuse an empty `Rules` cell** -- test `a_detector_naming_no_rule_is_refused` (`the detector MOVES names no rule`), then the check.
- [ ] **Step 4: Run** `cargo nextest run -p xtask` and `cargo xtask spec`, open `target/spec/html/vm-invariants.html` and check that every rule shows its tests and detectors -- PASS.
- [ ] **Step 5: Gate and commit** -- `cargo xtask gates commit`; `feat: the specification's VM invariants`.

### Task 5: Tests for the rules nothing checks yet

**Files:**
- Test: the crate test file of the subsystem each rule belongs to (`crates/ns-runtime/tests/<subsystem>.rs` or the module's `*_tests.rs`, following the neighbouring tests)
- Modify: `docs/spec/vm-invariants.md` (an `Undetected:` line removed where a test now names the rule)

- [ ] **Step 1: List** the rules Task 4 marked `Undetected:` whose reason is that no test exists yet rather than that none can; a rule whose breach no test can observe (undefined behaviour no tool detects, a property only of addresses) keeps its line.
- [ ] **Step 2: For each, write the test** that fails when the rule is broken, with its `// spec:` line, following `superpowers:test-driven-development`: show it red against a deliberately broken variant (not committed), then green.
- [ ] **Step 3: Remove** each such rule's `Undetected:` line.
- [ ] **Step 4: Gate and commit** -- `cargo xtask gates commit`; `test: every VM invariant a test can check is named by one`.

### Task 6: The design documents point at the rules

**Files:**
- Modify: every `docs/design/*.md` (one commit per document or per two small ones)

- [ ] **Step 1: For each document, section by section**, wherever it states a rule of the book -- any part -- put the pointer `` `r[…]` `` there and cut its own statement of the rule to one sentence beside it; keep whole the reasoning, the alternatives, the prior art, the measurements, the history of defects and the mechanism, which names the Rust code. Task 3's `Source:` lines say where each `vm.*` rule stands; the rules of parts 1 to 4 are found the same way.
- [ ] **Step 2: Run** `cargo nextest run -p xtask` and `cargo xtask spec` -- PASS.
- [ ] **Step 3: Commit** each -- `docs: <document> points at the book's rules`.

### Task 7: `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md` ("Invariants that are easy to break"; the crate list's "(not written yet)" for the VM invariants; the `docs/spec/` paragraph)

- [ ] **Step 1: Rewrite each internal entry**: its rule in one to three sentences, the `` `r[…]` `` of each rule it covers, the Rust names of the places that enforce it, the design document's section; drop what the book states in full. The entries that are rules of parts 1 to 4 keep a sentence and their pointers. The goldens, `{:?}` and the chain entries stay as they are.
- [ ] **Step 2: Update** the `docs/spec/` paragraph and the line in the crate list: the VM invariants are written, each saying what it protects and what finds its breach, and the design documents and `CLAUDE.md` point at the rules as `` `r[…]` ``, held by `cargo xtask spec`.
- [ ] **Step 3: Run** `cargo nextest run -p xtask` -- PASS; commit `docs: CLAUDE.md's invariants point at the book's rules`.

### Task 8: The documentation of the current state

**Files:**
- Modify: `docs/spec/introduction.md` ("Conventions": the `Protects:` and `Undetected:` lines, the detectors, the pointers), `docs/spec/runtime.md` (opening paragraph, if it still says the part is unwritten), `docs/design/specification.md` (opening paragraph: the fifth batch built; "The order of building it", item 5, "built"), `docs/design/README.md` ("The map", the specification's row: no longer "Partial"; the "Gates" text if it lists the spec checks), `docs/OPEN-WORK.md` (the item leaves "Next"; the complete install is next), `docs/PROGRESS.md` (the milestone)

- [ ] **Step 1: Update each** as listed, in the present tense.
- [ ] **Step 2: Run** `cargo xtask spec` -- PASS; commit `docs: the specification's fifth batch is built`.

### Task 9: The end of the batch

- [ ] **Step 1: Run** `cargo xtask gates end-of-batch` -- expected: every group passes.
- [ ] **Step 2: Follow** the `finishing-a-batch` skill (the documentation alone lets a fresh session start; `.temp/` deleted; the pull request offered, pushed only on the user's word).
