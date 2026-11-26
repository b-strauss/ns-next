# The specification's image format Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The fourth batch of ns-next's specification: the chapter
`docs/spec/image-format.md` -- the structure of an image, its sections,
records and tags, the rules the decoder checks, the bounds of a rejected
load, and the two abstract operations *LoadImage* and *WriteImage* --
with machine-readable tables that Rust tests hold to the code, and every
rule named by the tests that check it.

**Architecture:** Mostly prose and tests. Two fenced `spec-table` blocks
in the chapter: `image-tags`, every tag space of the image outside a
body's encoding, held to `image/opcodes.rs`, `image/encode.rs` and
`image/shipped_names.rs`; and `image-structures`, a small grammar of the
whole file, which a test-only walker reads and runs over real images,
consuming every byte and reaching every row. A body inside the code table
is walked by the bytecode chapter's tables (`wire-body`, `wire-tags`,
`wire-layouts`), so the two chapters describe one file. The validation
rules, the load and the write are numbered prose, each rule tagged on the
tests that check it. No change to the format, the encoder or the decoder.

**Tech Stack:** Rust (`ns-runtime`'s `image`, `spec_tables`), Markdown
under `docs/spec/`, mdBook 0.5.4 through `cargo xtask spec`.

**Spec:** `docs/design/specification.md`: decisions 1 to 7 (decision 3:
"For the formats the current state is the contract"; decision 4: "the
image is structure tables with numbered validation rules"), "The order of
building it", item 4; and the user's two decisions of 2026-11-26, which
Task 1 writes into that document:

1. **The chapter makes the format, the load and the write normative**
   (option C): the structure, the tags and the validation, and besides
   them the abstract operations *LoadImage* and *WriteImage*, each in
   numbered steps, so that everything a program observes after `build`
   and `load` is a rule. Weighed: the format alone (A), and the format
   with the load (B), each leaving program-observable behaviour -- what
   a weak slot holds after a load, the packaging point -- without a rule.
2. **The bound of a rejected load is normative as a property, its
   factors informative** (option B): the decoder refuses before it
   allocates any object of the graph when the sized objects would take
   more than eight words a byte of the input left, and what a rejected
   load allocates is bounded by a constant multiple of the image's length
   plus a constant. The factors (10.7x and the rest) stay in
   `docs/design/memory.md`, "The amplification bound of a rejected load",
   as the implementation's derivation. Weighed: the factors as the rule
   (A), which depend on the build and which no test holds, and no memory
   bound at all (C).

## Global Constraints

- The book describes the image format as it is at the batch's last
  commit. Where `docs/design/image-format.md` and the code disagree, the
  code decides, and the disagreement is written into the ledger (below)
  and corrected in the design document in Task 7.
- No change to a tag, a section, a record's layout, the encoder's output
  or a decoder refusal. The only source changes outside tests are
  visibility: `shipped_names.rs`'s `KIND_*` constants become
  `pub(crate)`.
- Rule identifiers start with `image.`; segments `[a-z][a-z0-9-]*`. The
  identifiers each task declares are listed in it; a task may add
  identifiers beside them, never rename one another task links to.
- A rule says what is refused, never which `DecodeError` variant reports
  it, as `docs/spec/bytecode.md`'s decoder rules do. An error text a rule
  quotes (the encoder's refusals, a run-time signal) is the code's,
  copied, never paraphrased.
- The rules a body's encoding and its decoding already have are the
  bytecode chapter's (`bytecode.wire.*`, `bytecode.verify.*`); this
  chapter links them (`[bytecode.verify.blocks](bytecode.md#r-bytecode.verify.blocks)`)
  and never restates them.
- *LoadImage* and *WriteImage* are written as `runtime.md` writes an
  operation -- numbered steps, calling its operations by link -- and
  state only what a program or a host observes; the mechanism (the
  tenure, the two passes' bookkeeping, `Heap::shape_placeholder`) stays
  in the design documents.
- The fingerprint's computation is implementation-defined: the book says
  what it identifies and what a load does with it, not how it is hashed.
- Every tag number, field width and field order the book states is held
  to the code by a test; a statement no test holds is prose without a
  rule identifier, or a rule the book's "Untested rules" appendix lists.
- Names written out (`CLAUDE.md`, "Conventions"), in code and in the
  book's examples.
- No change to `crates/ns-syntax`, the lexer or the plugin.
- Gates: the commit gate (`cargo xtask gates commit`) at every commit
  that changes more than Markdown; Task 1 touches `src/image/`, so its
  commit also runs `cargo xtask gates images`; the batch changed the
  runtime crate, so it ends with `cargo xtask gates end-of-batch`.

## The ledger

A section at the end of this plan, `## Ledger`, which each task appends
to: every disagreement found between the design documents and the code
(what the document says, what the code does, which the book follows),
every rule no test names, and every claim the task verified or refuted.
Task 7 carries each entry into the design documents or
`docs/OPEN-WORK.md`.

## Review Focus

- **Two fields of one width written in the other order** (a module
  record's path and id, a class record's superclass and enclosing
  object): the walker reads them without failing, so Task 2's test also
  asserts the values named fields read on known samples, and types each
  id by the table it indexes, which the walker bounds.
- **A tag of an image space retired in `opcodes.rs` but named in the
  book, or a constant taking a retired number**: Task 1's test compares
  names and retired numbers per space, both ways.
- **An image with source and one without, a started image and a
  snapshot**: the walker's samples include each, so the rows conditioned
  on `has-source` and the graph shapes only a snapshot holds (frames,
  module instances in registers, several roots) are walked.
- **The eight-words-a-byte refusal**: an image whose object table claims
  more objects than its bytes can size is refused before anything is
  allocated; Task 4 finds or writes the test that pins it.
- **A send whose argument count disagrees with its selector**:
  `image-format.md`, "Principles", says such an image can abort while
  running because leaves index arguments unchecked; the mirror leaves now
  check their arity (`interpreter/mirror.rs`, `arity_error`). Task 4
  pins what happens with a test and the book states it.

---

## File Structure

- `docs/spec/image-format.md` -- the chapter, written from scratch over
  the stub.
- `crates/ns-runtime/src/spec_tables.rs` -- gains `image_table(name)`
  beside `bytecode_table(name)`, both through one private reader, and
  `#[cfg(test)] mod image_format;`.
- `crates/ns-runtime/src/spec_tables/image_format.rs` -- new,
  `#[cfg(test)]`: the tag-table test, the structure walker and its test,
  and the sample images it walks. It lives under `spec_tables/` rather
  than `image/`, so that only Task 1's visibility change touches the
  image format's directory.
- `crates/ns-runtime/src/image/shipped_names.rs` -- `KIND_*` become
  `pub(crate)`.
- `crates/ns-runtime/tests/image.rs`, `tests/image_truncations.rs`,
  `tests/opcode_tags.rs` -- `// spec:` lines; the new tests Tasks 4 and 6
  name.
- `docs/design/specification.md`, `docs/design/image-format.md`,
  `docs/design/memory.md`, `docs/design/README.md`, `docs/OPEN-WORK.md`,
  `docs/PROGRESS.md`, `CLAUDE.md` -- the decisions and the status.

---

### Task 1: The chapter's frame, the header and the tag table

**Files:**
- Modify: `docs/spec/image-format.md`, `docs/design/specification.md`,
  `crates/ns-runtime/src/spec_tables.rs`,
  `crates/ns-runtime/src/image/shipped_names.rs`
- Create: `crates/ns-runtime/src/spec_tables/image_format.rs`

**Interfaces:**
- Produces: `crate::spec_tables::image_table(name: &str) -> Vec<Vec<String>>`
  (as `bytecode_table`, over `docs/spec/image-format.md`; the panic names
  the image chapter); the table `image-tags`; the rules
  `image.encoding`, `image.untrusted`, `image.header`,
  `image.header.flags`, `image.header.fingerprint`, `image.sections`,
  `image.trailing-bytes`, `image.tags`, which Tasks 2 to 6 link to.

- [ ] **Step 1: Write the failing test** in `image_format.rs`:
  `the_books_image_tags_are_the_encoders`. Each row of `image-tags` is
  `<space> <name> <number>` or `<space> retired <number>`. The spaces,
  and the constants each is compared with (a name is the constant's
  suffix after its prefix, in kebab case: `OBJECT_BODY_LESS_CLASS` is
  `body-less-class`):
  - `header-flag`: `FLAG_HAS_SOURCE`, `FLAG_START`; retired the bits of
    `RETIRED_HEADER_FLAGS` (1 and 2);
  - `value`: `VALUE_*`, retired `RETIRED_VALUE_TAGS`;
  - `object-body`: `OBJECT_*`, retired `RETIRED_OBJECT_BODIES`;
  - `mixin-reference`: `MIXIN_ROW`, `MIXIN_SHIPPED`, retired
    `RETIRED_MIXIN_TAGS`;
  - `mixin-side`: `MIXIN_SIDE_*`;
  - `class-reference`: `CLASS_RECORD`, `CLASS_SHIPPED`, retired
    `RETIRED_CLASS_TAGS`;
  - `frame-kind`: `FRAME_METHOD`, `FRAME_CLOSURE`, `FRAME_TOP_LEVEL`,
    `FRAME_INITIALIZER`, `FRAME_SUPERCLASS_CLAUSE`,
    `FRAME_OWN_INITIALIZER`, `FRAME_STANDALONE_INITIALIZER`,
    `FRAME_FACTORY`, retired `RETIRED_FRAME_KINDS`;
  - `frame-body`: `FRAME_BODY_*`;
  - `hashed`: `HASHED_SHIPPED`, retired `RETIRED_HASHED_TAGS`;
  - `slot-kind`: `SLOT_*`;
  - `origin`: `ORIGIN_PROJECT`, `ORIGIN_CORE`, retired
    `RETIRED_ORIGIN_TAGS`;
  - `module-version`: `MODULE_UNVERSIONED`, `MODULE_VERSIONED`;
  - `shipped-kind`: `shipped_names::KIND_*` (`module-instance`, `class`,
    `metaclass`, `level`, `level-metaclass`).
  The test asserts, per space, that the named rows equal the constants as
  a set of (name, number), that the retired rows equal the retired list,
  that no named number is retired, and that the table names no space but
  these thirteen. Put `// spec: image.tags` directly above `#[test]`.
- [ ] **Step 2: Run it to see it fail**
  Run: `cargo nextest run -p ns-runtime the_books_image_tags_are_the_encoders`
  Expected: FAIL, "the image chapter has no table `image-tags`" (after
  Step 3's reader exists; before it, a compile error naming
  `image_table`).
- [ ] **Step 3: Write `image_table`** in `spec_tables.rs`: one private
  `fn table(chapter: &str, chapter_name: &str, name: &str)` that both
  public readers call, the image chapter by `include_str!` as the
  bytecode chapter is. Make `shipped_names.rs`'s `KIND_*` `pub(crate)`.
- [ ] **Step 4: Write the chapter's first part** in
  `docs/spec/image-format.md`:
  - An introduction: what an image is (an object graph with the code
    and declarations it runs, one format for a started image and a
    snapshot), what the chapter covers (structure, validation, bounds,
    *LoadImage*, *WriteImage*), and that a body's encoding is the
    bytecode chapter's.
  - "Encodings": `image.encoding` -- little-endian integers, IEEE-754
    `binary64`, UTF-8, an integer that fits `i64` as an `i64` and every
    other as sign and 64-bit digits, least significant first; no native
    code, pointers, tagging or host endianness.
  - "Untrusted input": `image.untrusted` -- no byte sequence makes a
    load panic or abort; every id, tag, count and length is checked
    before it indexes, allocates or slices; every recursion whose depth
    the bytes choose is bounded. The guarantee covers decoding; what a
    decoded image's code does when it runs is "Running a decoded image",
    which Task 4 writes.
  - "The header": `image.header` (magic `NSNX`, `FORMAT_VERSION` 1 as a
    `u32`, the flag byte, the fingerprint as a `u64`), `image.header.flags`
    (the flags `image-tags` gives; a flag the table does not give,
    retired ones included, refuses the image), `image.header.fingerprint`
    (the fingerprint identifies the shipped world of the binary that
    wrote the image, computed as that binary defines; a load whose binary
    has another refuses the image before it reads past the header).
  - "Sections": `image.sections` -- the order symbols, code, texts (only
    with `has-source`), mixins, modules, shipped names, graph (object
    table, roots), hashes; `image.trailing-bytes` -- a byte after the
    last section refuses the image.
  - "Tags": `image.tags` -- the tag spaces, `retired` as in
    `bytecode.wire.tags`, the once-assigned rule linked to
    `bytecode.wire.tag-assigned-once`; then the `spec-table image-tags`
    block, read off `opcodes.rs`, `encode.rs` and `shipped_names.rs`.
  Add the user's 2026-11-26 decisions to `docs/design/specification.md`
  as "## The fourth batch's decisions", both, with the alternatives
  weighed, as this plan's "Spec" states them.
- [ ] **Step 5: Run the test**
  Run: `cargo nextest run -p ns-runtime the_books_image_tags_are_the_encoders`
  Expected: PASS. A mismatch is a wrong row: correct the book, never
  `opcodes.rs`.
- [ ] **Step 6: Tag the tests that check these rules.** `// spec:` lines
  on `tests/opcode_tags.rs`'s tests (`image.tags`),
  `load_refuses_a_version_mismatch` and the bad-magic test
  (`image.header`), `a_retired_stateless_flag_is_refused_as_unknown` and
  `a_header_flag_of_the_retired_module_text_layout_is_rejected`
  (`image.header.flags`), the shipped-world mismatch test
  (`image.header.fingerprint`), the trailing-bytes test
  (`image.trailing-bytes`), `tests/image_truncations.rs`'s tests
  (`image.untrusted`). A rule of this task no test checks gets one in
  `tests/image.rs` (the trailing byte, the fingerprint mismatch), named
  after its refusal.
  Run: `cargo xtask spec`
  Expected: the book builds, no unknown rule.
- [ ] **Step 7: Commit** after `cargo xtask gates commit images`.
  `docs: the specification's image header and tags`

### Task 2: The structure and its walker

**Files:**
- Modify: `docs/spec/image-format.md`,
  `crates/ns-runtime/src/spec_tables/image_format.rs`

**Interfaces:**
- Consumes: `image_table`, `bytecode_table`, the `image-tags` table, the
  bytecode chapter's `wire-body`, `wire-tags` and `wire-layouts`.
- Produces: the table `image-structures`; the rules `image.structure`,
  `image.symbols`, `image.code-table`, `image.texts`, `image.mixins`,
  `image.modules`, `image.shipped-names`, `image.graph`,
  `image.graph.record`, `image.graph.value`, `image.hashes`, which Tasks
  3 to 6 link to.

The notation of `image-structures`, which the chapter explains in prose
before the block and the walker implements:

- A row `<structure> <item> …` defines a structure as its items in
  order. The first row is `image`, the whole file.
- A row `<structure>/<space>` defines a tagged structure: a `u8` tag of
  that space of `image-tags`; rows `<structure>/<space>/<tag-name> <item> …`
  give the fields after each tag. A retired or unknown tag fails the
  walk.
- An item is `<field>:<type>`. Types: `u8`, `u16`, `u32`, `u64`, `i64`,
  `f64` (little-endian); `magic` (the four bytes `NSNX`); `bytes` (a
  `u32` length and that many bytes); `text` (`bytes` that are UTF-8);
  the ids, each a `u32` the walker bounds by the count it indexes, read
  earlier in the file or, for forward references, checked once the
  count is known: `symbol`, `code-id`, `row-id`, `object-id`,
  `shipped-index`, `text-index`; `body`, a body as the bytecode
  chapter's tables give it; or a structure's name.
- `<type>*<field>` is that many of `<type>`, `<field>` an earlier field
  of the same row; `<type>?<field>` is present when that earlier field
  is not 0; `<type>?flag:<name>` is present when the header has that
  flag.

- [ ] **Step 1: Write the failing test** in `image_format.rs`:
  `every_image_is_laid_out_as_the_book_gives`. A walker
  (`struct Walker`, test-only) reads an image by `image-structures`,
  recording each field it reads under its path
  (`modules.module[0].id`). Over the samples below the test asserts:
  each walk ends exactly at the image's last byte; every row and every
  tagged variant of the table is reached by some sample (the assertion
  names those that are not); and on the samples' known values: the
  header's version is 1, the flags are `start` alone for the packaged
  application and `has-source` with or without it as built, the module
  record of the project sample reads its path, id and version as
  written, and the first symbol of every image is the one the encoder
  writes first. Samples, built as `tests/image.rs`'s helpers build them
  (`packaged_application`, `snapshot_of`, `snapshot_module`,
  `snapshot_of_without_source`, `reified_holder_snapshot`):
  - a packaged application of two modules, one versioned, with source;
  - the same without source;
  - snapshots holding: a closure and its frame; a frame of each kind
    (method, closure, top level, the three initializer kinds, superclass
    clause, factory) and each frame-body spelling; a `ByteArray`, a
    `WeakArray`, an `Ephemeron`, a reified mixin of each side; a
    `LargeInteger`, a `Fraction`, a boxed and an immediate `Float`; a
    body-less declaration's class object and a level over a shipped
    class; a module instance in a frame's register; several roots.
  A variant no sample can reach is a ledger entry and a question for the
  controller, never an exemption written silently. Put
  `// spec: image.structure` directly above `#[test]`.
- [ ] **Step 2: Run it to see it fail**
  Run: `cargo nextest run -p ns-runtime every_image_is_laid_out_as_the_book_gives`
  Expected: FAIL, "the image chapter has no table `image-structures`".
- [ ] **Step 3: Write "Structure"** in the chapter: the notation as
  prose, then one subsection per section with its rule and the rows it
  owns, read off `encode.rs`, `mixins.rs`, `modules.rs`,
  `shipped_names.rs`, `texts.rs` and `symbols.rs`:
  - `image.symbols`: every selector and name once, referenced by index.
  - `image.code-table`: every body reachable from the modules and the
    graph, a block before the body that creates it, then the bodies only
    a closure or frame of the graph runs (an older version's); each body
    as `bytecode.wire.body`.
  - `image.texts`: per declaration its range, holes and cut text.
  - `image.mixins`: the row's fields in order; the table a forest, one
    tree per module in preorder, the trees in module order.
  - `image.modules`: per module its origin and path, id, version and the
    object id of its root class's record; module order.
  - `image.shipped-names`: per name the module's and the declaration's
    symbols, the kind and, for a level or its metaclass, the clause
    index; each listed once, in the order the graph first uses it.
  - `image.graph`: the object table's count, each record a header (class
    reference, `u32` identity hash) and a body; the roots.
    `image.graph.record`: each body tag's fields. `image.graph.value`:
    each value tag's payload.
  - `image.hashes`: the closing hash table's entries.
  Then the `spec-table image-structures` block.
- [ ] **Step 4: Run the test**
  Run: `cargo nextest run -p ns-runtime every_image_is_laid_out_as_the_book_gives`
  Expected: PASS. A failure is a wrong row or a missing sample, never a
  changed encoder.
- [ ] **Step 5: Tag the tests.** The round-trip and determinism tests of
  `tests/image.rs` that pin a section's shape (the module-record tests,
  `the_building_machine_s_paths_are_not_written`, the frame-record,
  byte-array, weak-array, ephemeron and mixin-record round trips,
  `a_boxed_float_value_round_trips_without_a_record`,
  `a_large_integer_in_i64_travels_as_an_i64_and_loads_as_a_large_integer`)
  get the rule of their section.
  Run: `cargo xtask spec`
  Expected: builds, no unknown rule.
- [ ] **Step 6: Commit** after the commit gate.
  `docs: the specification's image structure`

### Task 3: The rules the decoder checks -- sections, limits and names

**Files:**
- Modify: `docs/spec/image-format.md`; Rust tests (tags, and the tests
  this task finds missing)

**Interfaces:**
- Consumes: Task 2's structure rules.
- Produces: the rules `image.verify.counts`, `image.verify.ids`,
  `image.limit.shipped-names` (65,536), `image.limit.clause-index`
  (1,024), `image.limit.clauses` (1,024 `<:` clauses a row),
  `image.verify.texts`, `image.verify.mixin-rows`,
  `image.verify.mixin-tree`, `image.verify.initializers`,
  `image.verify.modules`, `image.verify.module-identity`,
  `image.verify.root-rule`, `image.verify.shipped-names`,
  `image.verify.started-image-names`, `image.shipped.positions`.

- [ ] **Step 1: Write "The rules the decoder checks"**, its first half,
  as a numbered list continuing the form of
  `bytecode.md`'s decoder rules, each rule on its line, the limits linked
  to `bytecode.limit.*` and `bytecode.verify.blocks` where a body's
  limit applies:
  - counts bounded by the bytes left before anything is reserved for
    them; every id within the table it indexes;
  - the three count limits above;
  - each text holding to itself and to its row;
  - a mixin row: no two members sharing a name, a lazy slot with its
    getter, a body-less row with clauses and no members, the forest's
    trees one per module record, nested declarations within their own
    tree;
  - a mixin's bodies against its declaration (kind, arity, the three
    initializer bodies present as the declaration needs them, a
    body-less declaration's code named once);
  - the module records held to the project's rules (path, id, the
    namespace `newspeak.` at `core:` only and for the manifest alone,
    version, no control character, no two modules of one id and
    version), each module's root record and the root rule;
  - the shipped names: every name one of the loading binary's
    declarations, no duplicate, a level within its clauses; in a started
    image the kernel's alone and no module instance;
  - `image.shipped.positions`: what may stand where, as a table in prose
    (not a `spec-table`) of the six positions of
    `shipped_names::NamePosition` and what each allows, as
    `docs/design/image-format.md`, "What may be named where", gives it,
    `Top`, `Activation`, `FutureImplementation`, the module roots and the
    VM mirror's class among the refusals.
- [ ] **Step 2: Tag the tests** that check each rule (`tests/image.rs`:
  the limit tests `an_image_applying_1025_mixins_is_refused` and its
  neighbours, the module-record tests, the root-rule tests, the
  shipped-name tests, `indexed_collection_as_a_value_or_a_superclass_is_refused`,
  `a_started_image_naming_another_module_is_refused`,
  `a_started_image_names_no_module_instance`,
  `building_the_shipped_names_refuses_a_name_before_it_builds_anything`).
  A rule no test checks gets one, in `tests/image.rs` beside the tests
  of its section, crafting the image as its neighbours do; name it after
  the refusal (`a_shipped_name_listed_twice_is_refused`).
  Run: `cargo nextest run -p ns-runtime --test image` and `cargo xtask spec`
  Expected: PASS; builds, no unknown rule.
- [ ] **Step 3: Commit** after the commit gate.
  `docs: the specification's image rules for sections and shipped names`

### Task 4: The rules the decoder checks -- the graph, the bounds and the run

**Files:**
- Modify: `docs/spec/image-format.md`; Rust tests (tags, and the tests
  this task finds missing)

**Interfaces:**
- Consumes: Task 3's numbered list.
- Produces: the rules `image.verify.class-records`,
  `image.verify.metaclass-records`, `image.verify.frames`,
  `image.verify.byte-arrays`, `image.verify.weak-records`,
  `image.verify.mixin-records`, `image.verify.symbols`,
  `image.verify.program-instances`, `image.verify.value-classes`,
  `image.verify.numbers`, `image.verify.started-image-roots`,
  `image.verify.mirror`, `image.verify.not-checked`,
  `image.bound.sizing`, `image.bound.linear`, `image.run`.

- [ ] **Step 1: Write the second half of the list:** class records (the
  row a record applies, no class side's row, no body-less row's mixin,
  the applied clause and named class exactly on a level, the body-less
  record exactly for a body-less declaration's last clause, no cycle of
  named classes, the enclosing object an object, every class reference a
  class object); metaclass records paired with their class; frames (a
  reference to a frame names a frame record, a closure frame names a
  closure, the body spelling its kind allows, `FRAME_BODY_DECLARED` only
  for a shipped declaration, the register file matching its body, a
  factory frame always declared); byte arrays, weak arrays, ephemerons
  and reified mixins under their class alone and with its counts, one
  record per mixin; one record per symbol text, under `Symbol` alone, and
  no string under `Symbol`; every program class's instance an instance
  record of its layout's slot count; a value class's conditions decided
  again; numbers in their canonical form (a `LargeInteger` as
  `bytecode.verify.large-integer`, a fraction in normal form); a started
  image of exactly one root; the VM mirror, its class and `Top` nowhere.
  Then `image.verify.not-checked`, as `bytecode.verify.not-checked` is
  written: a ready-made instance's slots against §3.1.1's value
  conditions, a second class record applying a module's root row, a
  send's argument count against its selector -- each with what keeps it
  safe, linking `docs/OPEN-WORK.md`'s items.
- [ ] **Step 2: Write "Bounds"** with the two rules of the user's
  decision 2: `image.bound.sizing` -- before it allocates any object of
  the graph, the decoder sizes every record from its own bytes and
  refuses the image when the objects would take more than eight words a
  byte of the input left; `image.bound.linear` -- what a rejected load
  allocates is bounded by a constant multiple of the image's length plus
  a constant, and nothing of it outlives the load. One informative
  sentence links `docs/design/memory.md`, "The amplification bound of a
  rejected load", for the factors.
- [ ] **Step 3: Pin the sizing refusal.** Find the test that crafts an
  object table larger than its bytes can size (`decode.rs` near
  `eight words a byte`, `tests/image.rs` near line 4898, "The premise of
  the load's amplification bound"); if none crafts one past the
  eight-word limit, write
  `an_object_table_past_eight_words_a_byte_is_refused_before_allocating`
  in `tests/image.rs`: an image whose object table claims frame records
  of 65,535 registers each in a few bytes, refused with the space's
  allocated words unchanged (`Heap` statistics as the neighbouring
  test reads them). `image.bound.linear` stays without a test and so in
  "Untested rules"; write that in the ledger.
- [ ] **Step 4: Settle the argument-count claim.** Write
  `a_send_whose_argument_count_disagrees_with_its_selector_runs_to_an_error`
  in `tests/image.rs`: a packaged application whose code table is
  crafted, as `an_image_with_a_send_of_256_arguments_is_refused` crafts
  its body, so that a `SendPublic` of `==` (a leaf of `Object`) passes no
  argument and one of `printString` passes one; the load succeeds, and
  running it ends in a Newspeak error, not a panic or an abort. If it
  aborts or panics, stop and report to the controller with the output:
  that is a defect for the user to decide, not for this batch to write
  down as a rule. Then write "Running a decoded image", `image.run`:
  `load` runs the application's code with the exposure of running its
  source, its recursion bounded by the depth limit
  ([`runtime.depth-limit`](runtime.md#r-runtime.depth-limit)), and what the
  test established about a send's argument count. Write the outcome in
  the ledger, with the sentence of `image-format.md`, "Principles", that
  Task 7 corrects.
- [ ] **Step 5: Tag the tests** that check each rule of Steps 1 and 2,
  writing a test for a rule none checks, as Task 3, Step 2.
  Run: `cargo nextest run -p ns-runtime --test image` and `cargo xtask spec`
  Expected: PASS; builds, no unknown rule.
- [ ] **Step 6: Commit** after the commit gate.
  `docs: the specification's image rules for the graph and the bounds`

### Task 5: LoadImage

**Files:**
- Modify: `docs/spec/image-format.md`; Rust tests (tags, and the tests
  this task finds missing)

**Interfaces:**
- Consumes: Tasks 1 to 4's rules.
- Produces: the rules `image.load`, `image.load.shipped-world`,
  `image.load.identity`, `image.load.hashes`, `image.load.symbols`,
  `image.load.applications`, `image.load.caches`, `image.load.weak`,
  `image.load.modules`, `image.load.start`, `image.load.snapshot`.

- [ ] **Step 1: Write "Loading an image"**: the abstract operation
  *LoadImage*(*bytes*) in numbered steps, as `image::load` runs them:
  check the header and the fingerprint; read the symbols, the code table,
  the texts, the mixin table and the module section, each refused by its
  rules; resolve the shipped names against the loading binary's world,
  building by the binary's own code the modules and classes the image
  names that are not built yet, after refusing every unknown name and
  before building anything; decode the graph; check what Task 4's rules
  check once every record exists; hand the host each module's root class
  and the image's roots; for an image with `start`, send its one root
  `main: platform args:` with the loading process's platform. Then the
  properties a program observes, each a rule:
  - `image.load.identity`: two references to one record load as one
    object, a record's object is new, and a value written by value
    (an integer, a float, a fraction) loads as an equal value;
  - `image.load.hashes`: a record's identity hash is the one it was
    written with;
  - `image.load.symbols`: a symbol record loads as the loading world's
    canonical symbol of its text;
  - `image.load.applications`: a class's behaviour is rebuilt from its
    mixin and its superclass, so two instances that shared one before
    share one after; nothing about an application is read from the
    bytes;
  - `image.load.caches`: every cache starts empty;
  - `image.load.weak`: a weak array's slots and an ephemeron's fields
    hold what was written, nothing cleared at load;
  - `image.load.modules`: each module keeps the identity its record
    gives, which the tools name it by;
  - `image.load.start`: the platform the application receives is the
    first it ever sees;
  - `image.load.snapshot`: an image without `start` is decoded under the
    same rules, but its frames may hold the kernel, a module instance or
    an `Activation` in their registers, so a host loads one only when it
    decides to trust it ([OPEN-WORK.md](../OPEN-WORK.md), "A running
    world's image is trusted").
- [ ] **Step 2: Tag the tests**: the round trips of identity, hashes
  (the identity-hash tests), symbols
  (`a_decoded_symbol_round_trips_to_the_loaded_worlds_canonical_object`),
  shared applications, weak records, module identities, the loader's
  building (`building_the_shipped_names_of_a_built_world_builds_nothing`)
  and the start (`ns-cli`'s `load` tests). Goldens run through `build`
  and `load` by the `images` profile get no rule line; the profile is
  the check of the round trip as a whole, which the rule's prose names.
  A property no test checks gets one in `tests/image.rs`.
  Run: `cargo nextest run -p ns-runtime --test image` and `cargo xtask spec`
  Expected: PASS; builds, no unknown rule.
- [ ] **Step 3: Commit** after the commit gate.
  `docs: the specification's LoadImage`

### Task 6: WriteImage

**Files:**
- Modify: `docs/spec/image-format.md`; Rust tests (tags, and the tests
  this task finds missing)

**Interfaces:**
- Consumes: Tasks 1 to 5's rules.
- Produces: the rules `image.write`, `image.write.started`,
  `image.write.not-written`, `image.write.weak`, `image.write.lazy-slots`,
  `image.write.source`, `image.write.edited`, `image.write.deterministic`,
  `image.write.refusals`, `image.write.limits`.

- [ ] **Step 1: Write "Writing an image"**: the abstract operation
  *WriteImage*(*modules*, *roots*, *start*) in numbered steps, as
  `image::write_image` runs them, the module order (the application
  module, the project file's modules as written, the embedded manifest),
  each module's root class walked after the roots. Then the rules:
  - `image.write.started`: `build` writes the application assembled by
    its factory and every initializer, just before `main:args:` is sent,
    as one root with `start`; it holds no platform, and an application
    that reaches one is refused;
  - `image.write.not-written`: the shipped world, written as shipped
    names; `nil`, `true` and `false` as value tags; no application, cache
    entry, dispatch id or binding;
  - `image.write.weak`: a weak slot names its object only if the graph
    reaches that object strongly otherwise, else `nil`; an ephemeron
    whose key the graph never reaches is written with every field `nil`;
    an immediate, a shipped name or a value written by value is written
    as it is;
  - `image.write.lazy-slots`: a lazy slot is written with its value, as
    any slot is -- as it is today, Bracha's §6.3.3 intending otherwise,
    the question open in `docs/OPEN-WORK.md` ("Whether an image carries a
    lazy slot's value"), linked from the rule;
  - `image.write.source`: with source every declaration's text, for
    every module or for none;
  - `image.write.edited`: a program a MirrorBuilder edited is written as
    it now is, with the bodies an older version still runs;
  - `image.write.deterministic`: one world writes one byte sequence;
  - `image.write.refusals`: the writer refuses what the loader would
    refuse -- a module root the root rule forbids, the VM mirror, its
    class, `Top` and their metaclasses anywhere in the graph, a shipped
    object where `image.shipped.positions` does not allow it, a module
    identity the project's rules refuse, source for some modules only --
    each refusal's text quoted from `encode.rs`;
  - `image.write.limits`: every count and id is a `u32`, so an image
    holds fewer than 2^32 objects; the writer aborts past it today
    ([OPEN-WORK.md](../OPEN-WORK.md), "Writing an image of more than
    about four billion objects aborts instead of refusing").
- [ ] **Step 2: Tag the tests**: `package_refuses_an_application_that_reaches_the_platform`,
  `the_graph_section_encoding_is_deterministic`,
  `the_code_table_encoding_is_deterministic`, the weak-array and
  ephemeron encoding tests, the root-rule and mirror refusals of the
  encoder, the edited-program round trips, `snapshot_error`'s users. A
  rule no test checks gets one in `tests/image.rs`; for
  `image.write.deterministic` over a whole image, if no test writes one
  world twice and compares every byte, write
  `writing_one_world_twice_gives_one_byte_sequence`.
  Run: `cargo nextest run -p ns-runtime --test image` and `cargo xtask spec`
  Expected: PASS; builds, no unknown rule.
- [ ] **Step 3: Commit** after the commit gate.
  `docs: the specification's WriteImage`

### Task 7: The status

**Files:**
- Modify: `docs/design/specification.md`, `docs/design/image-format.md`,
  `docs/design/memory.md`, `docs/design/README.md`, `docs/OPEN-WORK.md`,
  `docs/PROGRESS.md`, `CLAUDE.md`

- [ ] **Step 1: Carry the ledger.** Every entry goes into the design
  document it corrects or into `docs/OPEN-WORK.md`; the sentence of
  `image-format.md`, "Principles", about an abort while running says what
  Task 4 established.
- [ ] **Step 2: Write the status.** `specification.md`: the fourth batch
  built, its status paragraph naming the chapter, the image format no
  longer among what is not written. `image-format.md`: its status block,
  and in "Layout", "Sections", "Verification", "Shipped names" and
  "Packaging" one sentence each that the rules are the book's
  (`docs/spec/image-format.md`), the mechanism and its reasoning staying
  here. `memory.md`'s "The amplification bound of a rejected load": the
  rules are the book's, the factors this section's derivation.
  `README.md`'s map, where it names the specification. `CLAUDE.md`: the
  `docs/spec/` paragraph (the image format written; the VM invariants
  not yet) and the rule "for every part the book already writes --
  today …" gaining the image format. `OPEN-WORK.md`'s "Next": the fifth
  batch, the VM invariants and the documents pointing at the rules.
  `PROGRESS.md`: the entry and the milestone row.
- [ ] **Step 3: Run the end-of-batch gates**
  Run: `cargo xtask gates end-of-batch`
  Expected: every group passes; each stress summary reports every test
  passed and `run-corpus` "ran 6, failed 0, skipped 205".
- [ ] **Step 4: Commit**
  `docs: the specification's fourth batch in the status`

## Ledger

(Each task appends its entries here.)
