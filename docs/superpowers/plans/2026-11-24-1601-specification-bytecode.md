# The specification's bytecode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The third batch of ns-next's specification: the chapter
`docs/spec/bytecode.md` -- the instructions, each on a page of its own,
their register effects, their two encodings (the image's wire encoding
and the eight-byte form the run loop executes) and the rules the image
decoder checks on a body -- with machine-readable tables that Rust tests
hold to the code, and every rule named by the tests that check it.

**Architecture:** Mostly prose and tests. Three fenced `spec-table`
blocks in the chapter -- the register effects, the wire tags and
layouts, the eight-byte forms -- are read by a test-only reader in
`ns-runtime` (as `ns-syntax`'s `spec_tables.rs` reads the lexical
structure's) and compared with `bytecode.rs`'s exhaustive matches,
`image/opcodes.rs` and `image/encode.rs`, and `bytecode/compact.rs`. One
runtime change: the decoder refuses a statement map whose ranges leave
the body, which it reads unchecked today.

**Tech Stack:** Rust (`ns-runtime`'s `bytecode`, `image`), Markdown under
`docs/spec/`, mdBook 0.5.4 through `cargo xtask spec`.

**Spec:** `docs/design/specification.md`: decisions 1 to 7 (decision 4:
"each instruction has a page in the frame of the JVM specification's
chapter 6 -- operands, encoding, the registers read and written, outer
reach, jump targets, effect, errors"; decision 3: "For the formats the
current state is the contract"), "The order of building it", item 3,
and the user's decision of 2026-11-24 that **both encodings are
normative**: the wire encoding and the eight-byte form, each with rules
and tests.

## Global Constraints

- The book describes the bytecode as it is at the batch's last commit.
  Where a design document and the code disagree, the code decides and the
  disagreement is a ruling in the ledger.
- Both encodings are normative (decided with the user on 2026-11-24): the
  wire encoding (`image/encode.rs`, `image/decode.rs`, `image/opcodes.rs`)
  and the eight-byte form (`bytecode/compact.rs`). The JVM specification's
  frame per instruction: operands, encoding (wire and eight-byte), reads,
  writes, outer reach, jump targets, effect, errors.
- Rule identifiers start with `bytecode.`; segments `[a-z][a-z0-9-]*`;
  an instruction's page is `bytecode.instruction.<kebab-case name>`
  (`bytecode.instruction.send-public`). The identifiers each task declares
  are listed in it; a task may add identifiers beside them, never rename
  one another task links to.
- An instruction's effect is written in numbered steps, as `runtime.md`
  writes an operation, and calls the abstract operations of `runtime.md`
  by link (`[Send](runtime.md#r-runtime.send)`) rather than restating
  them. Every error text quoted is the code's, copied, never paraphrased.
- The three tables are fenced blocks with the info string `spec-table
  <name>`, one row per line, columns separated by whitespace, `-` for an
  empty cell; their names are `register-effects`, `wire-tags`,
  `wire-layouts` and `compact-forms`.
- Every tag number, operand width and field order the book states is held
  to the code by a test; a statement no test holds is prose without a rule
  identifier, or a rule that the book's "Untested rules" appendix lists.
- No change to an instruction, a tag, an operand layout or the eight-byte
  form; the one behaviour change is Task 6's refusal.
- No change to `crates/ns-syntax`, the lexer or the plugin.
- Names written out (`CLAUDE.md`, "Conventions"), in code and in the
  book's examples.
- Gates: the commit gate at every commit that changes more than Markdown;
  Task 6 touches `image/decode.rs`, so its commit also runs `cargo xtask
  gates images`; the batch changed the runtime, so it ends with `cargo
  xtask gates end-of-batch`.

## Review Focus

- `LoadLiteral` of `Literal::BodyTooLarge`, which writes no register and
  does not fall through: the register-effects table must state the
  exception and its test must exercise it, not only the plain
  `LoadLiteral`.
- `MakeClosure`'s outer reach, which is its block's reach less one, not a
  field: the table states it as `block-1` and the test builds a block whose
  reach is 2 and expects 1.
- A retired wire tag (`RETIRED_OPCODES`, `RETIRED_LITERALS`,
  `RETIRED_CODE_KIND_TAGS`): the book lists each, and the table test
  fails when a constant takes a retired number or a retired number leaves
  the list.
- A `CompactInstruction` variant added later: the eight-byte table's test
  must fail to compile or fail, never pass with the variant missing from
  the book.
- An image whose statement map names an instruction past the body, or a
  range whose start is past its end: refused with a decode error, never a
  panic or a wrong line in the debugger.

---

## File Structure

- `docs/spec/bytecode.md` -- the chapter (written from scratch over the
  stub).
- `crates/ns-runtime/src/spec_tables.rs` -- new, `#[cfg(test)]`: reads the
  chapter's `spec-table` blocks as rows (`pub(crate) fn
  bytecode_table(name: &str) -> Vec<Vec<String>>`).
- `crates/ns-runtime/src/lib.rs` -- `#[cfg(test)] mod spec_tables;`.
- `crates/ns-runtime/src/bytecode.rs` -- the register-effects test
  (`#[cfg(test)]` module).
- `crates/ns-runtime/src/image/encode.rs` -- the wire tests (its
  `#[cfg(test)]` module, created if absent).
- `crates/ns-runtime/src/bytecode/compact.rs` -- the eight-byte test, and
  `CompactInstruction::form_name`.
- `crates/ns-runtime/src/image/decode.rs` -- the statement-map refusal.
- `crates/ns-runtime/tests/image.rs` -- the refusal's test and two tests
  that pin existing refusals.
- Rust tests under `crates/ns-runtime/` and goldens under `goldens/` --
  `// spec:` and `rules` lines.
- `docs/design/specification.md`, `docs/design/image-format.md`,
  `docs/design/execution.md`, `docs/design/compiler.md`,
  `docs/OPEN-WORK.md`, `docs/PROGRESS.md`, `CLAUDE.md` -- the status.

---

### Task 1: Bodies, registers and the register-effects table

**Files:**
- Create: `crates/ns-runtime/src/spec_tables.rs`
- Modify: `crates/ns-runtime/src/lib.rs`, `crates/ns-runtime/src/bytecode.rs`, `docs/spec/bytecode.md`, `docs/design/specification.md`

**Interfaces:**
- Produces: `crate::spec_tables::bytecode_table(name: &str) -> Vec<Vec<String>>`
  (each non-empty line of the block, split at whitespace; panics naming
  the table when the chapter has none); the `register-effects` table; the
  rule identifiers below, which Tasks 2-6 link to.

- [ ] **Step 1: Write the failing test** in `bytecode.rs`'s test module:
  `every_instruction_has_the_register_effects_the_book_gives`. It builds
  one sample of every `Instruction` variant, each register field a
  distinct number (lists of two or three), a `LoadLiteral` of `Nil` and
  one of `BodyTooLarge(BodyLimit::Registers)`, a `MakeClosure` over a block
  whose `outer_reach` is 2; for each it finds the table row by the
  variant's name (`format!("{instruction:?}")` up to the first ` ` or
  `{`; the `BodyTooLarge` load's row is `LoadLiteral/BodyTooLarge`) and
  asserts, against the field values parsed from the sample's `Debug`:
  - column `reads`: the comma-separated field names whose values, in
    order, are exactly `registers_read`'s;
  - column `writes`: the field name whose value `register_written`
    answers, or `-`;
  - column `target`: the field `jump_target` answers, or `-`;
  - column `falls-through`: `yes`/`no`, `falls_through`'s answer;
  - column `outer-reach`: `0`, `depth`, or `block-1`, matching
    `outer_reach_of(&[sample])`;
  - column `own-slots`: `yes`/`no`, `addresses_own_slots_of(&[sample])`;
  and that the table has exactly one row per sample.
  Put `// spec: bytecode.register-effects` directly above `#[test]`.

- [ ] **Step 2: Run it to see it fail**
  Run: `cargo nextest run -p ns-runtime every_instruction_has_the_register_effects`
  Expected: FAIL, "the bytecode chapter has no table `register-effects`".

- [ ] **Step 3: Write the chapter's first part** in `docs/spec/bytecode.md`:
  an introduction (the bytecode is a register machine; one body per
  method, block, initializer and superclass clause; what the chapter
  covers; both encodings normative); then sections with these rules:
  - "Bodies": `bytecode.body` (the fields of a `CompiledCode` that are the
    contract: kind, `argument_count`, `register_count`, `copied_count`,
    `local_count`, source span, statement map, instructions; `outer_reach`,
    `addresses_own_slots`, `family_size` derived, never stored),
    `bytecode.body.kind` (the four `CodeKind`s and what frame each runs in),
    `bytecode.body.statement-map`.
  - "Registers": `bytecode.register.layout` (arguments, the
    `HEADER_REGISTERS` = 13 header numbers no instruction names, copied
    values, temporaries, the vector register, scratch from
    `local_count`), `bytecode.register.arguments` (no instruction writes
    an argument register), `bytecode.register.argument-window` (a send's
    contiguous slots `argument_base ..`, the sources copied in before it
    decides, the slots at or above `local_count`, a source never another
    slot of the same window), `bytecode.register.leftovers` (a register
    a frame does not write before it reads it is dead; the rule
    `compiler.md`, "What a callee leaves", states).
  - "Limits": `bytecode.limit.instructions` (`INSTRUCTION_LIMIT` = 2^20),
    `bytecode.limit.arguments` (`ARGUMENT_LIMIT` = 255),
    `bytecode.limit.registers`, and how a body past one compiles to
    `Literal::BodyTooLarge` (link `compiler.md`'s "Limits" text into the
    rule, not the design document).
  - "Call sites": `bytecode.call-site` (a send's number in its code
    family, dense from 0; `UNCACHED` for the reference lowering; never on
    the wire).
  - "Register effects": `bytecode.register-effects`, the prose saying
    what each column means, then the `spec-table register-effects` block,
    header row first (`instruction reads writes target falls-through
    outer-reach own-slots`), one row per `Instruction` variant plus
    `LoadLiteral/BodyTooLarge`, read off `bytecode.rs`'s matches.
  Add the user's 2026-11-24 decision to `docs/design/specification.md` as
  "## The third batch's decision": both encodings normative; the
  alternatives (the wire alone, as the JVM specification leaves
  HotSpot's rewritten bytecodes out; the eight-byte form as informative
  prose) and why they were not taken.

- [ ] **Step 4: Write `spec_tables.rs`** as `ns-syntax`'s reader does
  (`include_str!` of `docs/spec/bytecode.md`, the block after the line
  ```` ```spec-table <name> ````, up to the closing fence), answering rows;
  run the test.
  Run: `cargo nextest run -p ns-runtime every_instruction_has_the_register_effects`
  Expected: PASS. A failure is a wrong row: correct the book, never the
  matches.

- [ ] **Step 5: Tag the tests that check these rules.** `// spec:` lines
  on the existing tests of the limits (`compiler.md`, "Limits", names
  them), the argument window and the leftovers (`bytecode/leftovers.rs`'s
  tests, `tests/image.rs`'s `a_send_whose_source_is_another_of_its_slots_is_refused`,
  `a_write_to_an_argument_register_is_refused`,
  `an_image_reading_unwritten_scratch_is_refused`).
  Run: `cargo xtask spec`
  Expected: the book builds, no unknown rule.

- [ ] **Step 6: Commit** after the commit gate (`cargo xtask gates commit`).
  `docs: the specification's bodies, registers and register effects`

### Task 2: The instruction pages -- values, variables, slots and objects

**Files:**
- Modify: `docs/spec/bytecode.md`, Rust tests and goldens (tags only)

**Interfaces:**
- Consumes: Task 1's register rules and table.
- Produces: `bytecode.instruction.<name>` for `LoadLiteral`, `LoadSelf`,
  `LoadEnclosingObject`, `Move`, `StoreLocal`, `LoadOuterLocal`,
  `StoreOuterLocal`, `MakeArray`, `MakeVector`, `LoadVector`,
  `StoreVector`, `InitializeSlot`, `InitializeSlotProvisionally`,
  `LoadOwnSlot`, `ReturnSlotIfFilled`, `MakeFuture`, `ResolveFuture`,
  `MakeClosure`.

- [ ] **Step 1: Write one page per instruction** under "Instructions",
  each a `###` heading with the instruction's name, its rule line, then,
  in this order: **Operands** (each field, its meaning and type),
  **Encoding** (its wire tag and its eight-byte forms, linking the rows of
  Tasks 4 and 5's tables by name), **Reads / Writes / Outer reach /
  Jumps** (one line, agreeing with the `register-effects` row), **Effect**
  (numbered steps, calling `runtime.md`'s operations by link:
  `runtime.slot.read`, `runtime.slot.write`, `runtime.slot.guard`,
  `runtime.outer`, `runtime.instantiate`), **Errors** (each signal, its
  text from the code -- `interpreter/literals.rs` for the deferred
  literals, `interpreter/locals.rs` for the vectors and own slots --, or
  "none"). `LoadLiteral`'s page lists every literal kind, the deferred
  three (`UnresolvedIdentifier`, `BodyTooLarge`, `Unbuilt`) with their
  error texts, and that a string, symbol, array or large integer is a
  fresh object each time it runs.

- [ ] **Step 2: Tag the tests.** For each page, the goldens and Rust tests
  that exercise the instruction get its identifier (`rules` in a
  golden's `.toml`, `// spec:` above a Rust `#[test]`); an instruction no
  test names stays in "Untested rules" and is listed in the ledger.
  Run: `cargo xtask spec`
  Expected: builds; every new identifier resolves.

- [ ] **Step 3: Commit** after the commit gate.
  `docs: the specification's instructions for values, variables and slots`

### Task 3: The instruction pages -- sends, control flow and initialization

**Files:**
- Modify: `docs/spec/bytecode.md`, Rust tests and goldens (tags only)

**Interfaces:**
- Consumes: Task 2's page frame.
- Produces: `bytecode.instruction.<name>` for `SendPublic`, `SendLexical`,
  `SendSuper`, `SendInlinable`, `SendMirror`, `Return`, `ImplicitReturn`,
  `NonLocalReturn`, `SignalNonBoolean`, `Jump`, `JumpIfNotBoolean`,
  `BranchIfFalse`, `BranchIfTrue`, `IntegerBinary`, `AllocateInstance`,
  `InitializeInstance`, `CheckAppliedClause`, `InitializeSuperclass`,
  `InitializeAppliedLevel`.

- [ ] **Step 1: Write the pages** in Task 2's frame. Effects call
  `runtime.send`, `runtime.implicit-receiver`, `runtime.outer-send`,
  `runtime.invoke`, `runtime.does-not-understand`, `runtime.unwind`,
  `runtime.instantiate`, `runtime.apply-mixin` by link. Pinned specifics:
  `SendInlinable` is `SendPublic` but for the depth count
  (`compiler.md`, "How the limit counts"); `SendMirror` has no wire tag,
  is emitted only into the shipped kernel and its row is decided at
  compile time; `IntegerBinary`'s nine operations, each computed only when
  both operands are `SmallInteger`s and the result fits, otherwise a jump
  to `target`, where the ordinary send stands; `SignalNonBoolean`'s
  `NonBooleanReceiver`; `InitializeInstance` names its body's own
  arguments by no operand.

- [ ] **Step 2: Tag the tests**, as Task 2, Step 2.
  Run: `cargo xtask spec`
  Expected: builds.

- [ ] **Step 3: Commit** after the commit gate.
  `docs: the specification's instructions for sends, control flow and initialization`

### Task 4: The wire encoding

**Files:**
- Modify: `docs/spec/bytecode.md`, `crates/ns-runtime/src/image/encode.rs`

**Interfaces:**
- Consumes: `crate::spec_tables::bytecode_table`.
- Produces: the tables `wire-tags` and `wire-layouts`; rules
  `bytecode.wire.tags`, `bytecode.wire.tag-assigned-once`,
  `bytecode.wire.body`, `bytecode.wire.instruction`,
  `bytecode.wire.literal`, `bytecode.wire.call-sites`.

- [ ] **Step 1: Write the failing tests** in `image/encode.rs`'s test
  module:
  - `the_books_wire_tags_are_the_encoders`: every row of `wire-tags` is
    `<space> <name> <number>` with space one of `instruction`, `literal`,
    `integer-operation`, `code-kind`, or `<space> retired <number>`; the
    named rows equal, as a set, the `OPCODE_*`, `LITERAL_*` and
    code-kind constants of `opcodes.rs` and the integer-operation bytes
    `integer_operation_tag` answers (names as the constants' suffixes in
    kebab case, `send-public`, `unbuilt-pattern-literal`); the retired
    rows equal `RETIRED_OPCODES`, `RETIRED_LITERALS` and
    `RETIRED_CODE_KIND_TAGS`.
  - `each_instruction_is_written_as_the_book_lays_it_out`: for every
    `Instruction` sample of Task 1 but `SendMirror`, encodes it with
    `encode_instruction` and asserts the bytes are the row of
    `wire-layouts` for its name -- `<name> <field>:<type> ...`, types
    `u8`, `u16`, `u32`, `symbol` (a `u32` symbol id), `code` (a `u32` code
    id), `literal`, `registers` (a `u16` count then `u16`s), `symbols`
    (likewise of symbol ids), `operation` (a byte) -- filled from the
    sample's field values, little-endian.
  Put `// spec: bytecode.wire.tags` and `// spec: bytecode.wire.instruction`
  above them.

- [ ] **Step 2: Run them to see them fail**
  Run: `cargo nextest run -p ns-runtime the_books_wire_tags each_instruction_is_written`
  Expected: FAIL, the tables are missing.

- [ ] **Step 3: Write "The wire encoding"** in the chapter: the rules above
  -- a body's record in the order `encode_code` writes it (kind tag,
  arity, `register_count`, `copied_count`, `local_count`, the span, the
  statement map, the uncached flag, the instruction count, the
  instructions), every number little-endian, the literal's own tag and
  payload, call sites never written and numbered by the decoder in a
  family's emission order, a tag assigned once and a retired number
  refused -- and the two tables, read off `opcodes.rs` and
  `encode_instruction`. Each instruction page of Tasks 2 and 3 links its
  rows.

- [ ] **Step 4: Run the tests**
  Run: `cargo nextest run -p ns-runtime the_books_wire_tags each_instruction_is_written`
  Expected: PASS. A mismatch is a wrong row, never a changed encoder.

- [ ] **Step 5: Tag** `tests/opcode_tags.rs`'s tests with
  `bytecode.wire.tag-assigned-once`, the retired-tag refusals
  (`a_retired_send_tag_is_refused`,
  `an_invalid_or_retired_literal_tag_is_a_decode_error_not_a_panic`) and
  the call-site numbering tests (`number_call_site_families_*`) with
  theirs. Run `cargo xtask spec`; expected: builds.

- [ ] **Step 6: Commit** after the commit gate.
  `docs: the specification's wire encoding of the bytecode`

### Task 5: The eight-byte form

**Files:**
- Modify: `docs/spec/bytecode.md`, `crates/ns-runtime/src/bytecode/compact.rs`

**Interfaces:**
- Consumes: `crate::spec_tables::bytecode_table`.
- Produces: `CompactInstruction::form_name(&self) -> &'static str`
  (`#[cfg(test)]`, an exhaustive `match` with no `_` arm); the table
  `compact-forms`; rules `bytecode.compact.slot`, `bytecode.compact.forms`,
  `bytecode.compact.tables`, `bytecode.compact.choice`,
  `bytecode.compact.wide`, `bytecode.compact.breakpoint`,
  `bytecode.compact.round-trip`.

- [ ] **Step 1: Write the failing test** in `compact.rs`'s test module:
  `every_eight_byte_form_is_the_books`. A function `every_form() ->
  Vec<CompactInstruction>` builds one value of every variant, each field
  at its type's maximum (`u8::MAX`, `u16::MAX`, `u32::MAX`, `i32::MIN`
  for `LoadSmallInteger`; a `Symbol` field, typed `symbol` in the table,
  and `SendMirror`'s `MirrorPrimitive`, typed `primitive`, any value),
  with a comment at `form_name` saying that a new
  variant goes into both. The test asserts that the names `form_name`
  answers over `every_form()` are distinct and equal the table's first
  column, and that each row's remaining cells, `<field>:<type>`, are the
  sample's fields in `Debug` order with the type whose maximum the field
  prints. Put `// spec: bytecode.compact.forms` above it.

- [ ] **Step 2: Run it to see it fail**
  Run: `cargo nextest run -p ns-runtime every_eight_byte_form_is_the_books`
  Expected: FAIL, no table `compact-forms`.

- [ ] **Step 3: Write "The eight-byte form"**: one instruction one slot of
  eight bytes, a register a `u8`, a jump target an absolute `u32`
  instruction index; the side tables (`constants`, `literals`,
  `send_sites`, `blocks`, `mirror_sites`, `superclass_sites`,
  `register_lists`, `wide`) and what each holds; which form the encoder
  chooses (`LoadNil`/`LoadTrue`/`LoadFalse`, `LoadSmallInteger` for an
  integer in `i32`, `LoadConstant` for another `SmallInteger` or a
  `SmallFloat64`, `LoadLiteral` otherwise; a send's form by its argument
  count, `Wide` from three arguments or a register above 255; every other
  instruction `Wide` when a register exceeds 255 or a table index 16
  bits); `Breakpoint`, never emitted or decoded, the original kept beside
  the slots; the decoded view an exact inverse, a body built narrow equal
  to the same body built wide; then the `spec-table compact-forms` block
  read off `CompactInstruction`. Each instruction page links its forms.

- [ ] **Step 4: Run the test**
  Run: `cargo nextest run -p ns-runtime every_eight_byte_form_is_the_books`
  Expected: PASS.

- [ ] **Step 5: Tag** `tests/compact_encoding.rs`'s tests, the
  round-trip tests' `assert_derived_as_decoded` users and the goldens
  under `goldens/runtime/encoding/` with `bytecode.compact.*` rules. Run
  `cargo xtask spec`; expected: builds.

- [ ] **Step 6: Commit** after the commit gate.
  `docs: the specification's eight-byte form`

### Task 6: The rules the decoder checks

**Files:**
- Modify: `docs/spec/bytecode.md`, `crates/ns-runtime/src/image/decode.rs`, `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: Task 4's wire rules.
- Produces: the rules `bytecode.verify.<name>`, numbered in the chapter;
  a decode error for a statement map that leaves its body.

- [ ] **Step 1: Write the failing test** in `tests/image.rs`:
  `a_statement_map_leaving_its_body_is_refused`: an image whose body's
  statement range ends past its instruction count, and one whose range
  starts past its end, each refused by `load` with an error, never a
  panic. `// spec: bytecode.verify.statement-map` above it.

- [ ] **Step 2: Run it to see it fail**
  Run: `cargo nextest run -p ns-runtime a_statement_map_leaving_its_body_is_refused`
  Expected: FAIL (the image loads).

- [ ] **Step 3: Refuse it** in `decode_code` where the map is read: every
  range `start <= end <= instruction_count`, else a new
  `DecodeError::StatementMapOutOfRange { statement, start, end, length }`
  (or the error enum's existing shape for such a refusal), checked once
  the instruction count is known.

- [ ] **Step 4: Run it**
  Run: `cargo nextest run -p ns-runtime a_statement_map_leaving_its_body_is_refused`
  Expected: PASS.

- [ ] **Step 5: Pin two refusals no test pins**, each watched passing
  only with its check in place (comment the check out, see it fail,
  restore): `an_outer_local_naming_its_outer_bodys_header_is_refused`
  (`verify_outer_locals`, `HeaderRegister` at depth 1) and
  `a_large_integer_literal_with_an_invalid_sign_is_refused`
  (`InvalidTag { what: "large integer sign" }`).

- [ ] **Step 6: Write "The rules the decoder checks"**: the image decoder
  refuses a body unless each of these holds, numbered, each its own rule
  with its tests' names in the book through their `// spec:` lines -- the
  tags (instruction, code kind, literal, integer operation; symbol ids in
  range); the body's fields (arity at most `ARGUMENT_LIMIT` and at most
  `register_count`, `local_count` at most `register_count`, the copied
  values within the registers, a span not backwards, at most
  `INSTRUCTION_LIMIT` instructions, the statement map within the body);
  the registers (every operand below `register_count`, none a header
  register, no argument written); the argument windows (fit the
  registers, start at or above the header's end and `local_count`, no
  source another slot, at most 255 arguments); the control flow (every
  target inside the body, a non-empty body ending in `Return`,
  `ImplicitReturn`, `NonLocalReturn`, `SignalNonBoolean` or `Jump`); the
  blocks (`MakeClosure` names an earlier block body in the table, created
  by no other, its parameter and copied counts its body's, nesting at
  most 128); the outer locals (index below the outer body's
  `local_count`, not its header, no argument stored; a depth past the
  nesting bounded by the outermost body); the leftovers
  (`bytecode.register.leftovers`, and the bound past which the analysis
  refuses); a large integer canonical; and that a body's kind and arity
  agree with what it is attached to. State as rules, too, what is not
  checked at a body and why: `own_index`, which the instances' field
  counts hold (the image chapter's rule, linked once it exists, named in
  prose until then); a vector's index and size, checked at run time; and
  that the embedded shipped code is not verified, the binary's own.
  Tag every test the survey of `decode.rs` names (`tests/image.rs`'s
  refusal tests, `tests/image_truncations.rs`).

- [ ] **Step 7: Commit** after `cargo xtask gates commit images`.
  `fix: the decoder refuses a statement map leaving its body`

### Task 7: The status

**Files:**
- Modify: `docs/design/specification.md`, `docs/design/image-format.md`,
  `docs/design/execution.md`, `docs/design/compiler.md`,
  `docs/design/README.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`,
  `CLAUDE.md`

- [ ] **Step 1: Write the status.** `specification.md`: the third batch
  built, its status paragraph naming the chapter. `image-format.md`'s
  "The instruction encoding" and "Verification", `execution.md`'s "The
  compact instruction encoding" and `compiler.md`'s "The instruction
  set" and "Registers": one sentence each that the rules are the book's
  (`docs/spec/bytecode.md`), the mechanism and its reasoning staying
  here, and the statement map's new refusal where "Verification" lists
  the checks. `CLAUDE.md`: the `docs/spec/` paragraph (the bytecode
  written; the image format and the VM invariants not yet) and the rule
  "for every part the book already writes -- today ..." gaining the
  bytecode. `OPEN-WORK.md`'s "Next": the fourth batch, the image format.
  `PROGRESS.md`: the entry and the milestone row.

- [ ] **Step 2: Run the end-of-batch gates**
  Run: `cargo xtask gates end-of-batch`
  Expected: every group passes; each stress summary reports every test
  passed and `run-corpus` "ran 6, failed 0, skipped 205".

- [ ] **Step 3: Commit**
  `docs: the specification's third batch in the status`
