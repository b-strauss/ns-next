# Edited programs in images (road step 5b2b) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 5b2b: a program a MirrorBuilder edited is written to an image and loads back edited, with the code an older version still runs; an image carries its program's source by default; and a program loaded from an image can be edited, so that every golden, the MirrorBuilder's included, goes through `build` then `load` again.

**Architecture:** The encoder plans its rows through the mixin ids -- each declaration's *current* version -- and names every mixin by its `MixinId`, not by an address. The module section carries, per module, its current text and every earlier text a row's spans still point into, and each row names its text, so that a loaded world holds exactly the texts the live one did. The graph is walked before the code table is written, so the code table also takes every body a closure or a frame of the graph runs that no row reaches -- the code of a version an install superseded -- and a frame record names its body by code id rather than leaving the decoder to look it up in the current declaration. `Mutator::prepare_edit` no longer refuses a mixin decoded from an image: it already parses the module's current text at every edit, and an image now carries that text. `ns-next build` writes the source unless `--strip-source` is given.

**Tech Stack:** Rust 1.98.0, the Newspeak core library, `cargo-nextest`.

**Spec:** `docs/design/liveness-and-reflection.md`, "Planned: road step 5b2b, edited programs in images, and reclaiming ids" (its first two bullets), "A MirrorBuilder and an atomic install", "An edit replaces the mixin under its id"; `docs/design/image-format.md`, "Planned: edited mixins in the image", "Source", "Verification", "How frames actually get into an image". Read them before Task 1. Prior art, for behaviour only: Primordial Soup serializes methods as graph objects, so a closure over an old method's code is written like any object (`prior-art/primordialsoup/newspeak/Mirrors.ns`, `installAll` `:308-327`); copy no code.

**Decided with the user on 2026-10-19** (not reopened):

1. Road step 5b2b is split in two. This plan: source in images by default, edited programs in images, the code an older version runs, editing a loaded program, every golden through images again, and the decompiler dropped. Road step 5b2c, a plan of its own after this one: the weak application cache and the ids' reuse rules, `Mixin::nested` through ids, and the per-creation marker of the nested-class cache slot.
2. **An image carries its program's source by default.** `ns-next build` writes every module's text; `--strip-source` leaves it out (it replaces `--debug-source`). A program loaded from an image without source cannot be edited (`<name> has no source text to edit`, 5b2a's diagnostic).
3. **Exporting an image back to `.ns` rests on the carried source; there is no decompiler.** A decompiler could only invent the names and comments the compiled code no longer has, so it is dropped from the open work, and the decision is recorded in `image-format.md`.
4. **The code an older version runs goes into the code table.** Every body the graph's closures and frames run that no row reaches enters the code table, and a frame record names its body by code id. A body that belongs to no version of a mixin the image carries is still refused, as a closure over another module's code is today.

**Refined while planning, stated here for review:**

- *An edited module has several texts.* An install makes the edited subtree's mixins read in the new text, while every mixin outside the subtree keeps the text it was read in, whose spans still describe its methods there (`liveness-and-reflection.md`, "Compiling", step 5). Writing only the current text would slice an enclosing class's method source from the wrong text after load. So the module section carries the current text first and then every other text a row of the module is read in, and a row names its text by index; the loader gives each row its text and makes the first the module's current text. No text is parsed on load.
- *Two retired numbers.* The new layouts take new numbers, so that a stale artifact is refused rather than misread (`opcodes.rs`'s rule, here applied to the header flag too): `OBJECT_FRAME` 10 is retired and the frame record becomes 15; `FLAG_HAS_SOURCE` 1 is retired and the source variant becomes bit 8.
- *The host no longer hands the text.* `image::ModuleSource::source: Option<&str>` becomes `with_source: bool`; the encoder reads the texts from the heap, since after an edit the host's file is no longer the program's text.
- *A frame of an old method.* A method frame record carries its selector, its method's access and its body's code id; the decoder reuses the current declaration's method when its body is that very code, and otherwise builds a `MixinMethod` of the old body, which no dispatch ever finds (a decoded frame is never re-entered).

Task 6 writes all of this into the design documents.

## Global Constraints

- Every commit is green: the per-commit gates in `CLAUDE.md` ("The gates") run before each commit through `cargo xtask gates <groups>`, the groups each task names. The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 6, by the controller (`finishing-a-batch` skill); an implementer does not run them, and a task's commit is work in progress until they have run.
- Commits are conventional (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- Work on the branch `road-step-5b2b` in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree. It exists already.
- Run every command in the foreground.
- Names are written out (`CLAUDE.md`, "Names are written out"): `text_index`, `module_texts`, `superseded`, `code_id`, never `idx`, `txt`, `sup`, `cid`.
- **Nothing under `crates/ns-syntax` changes.**
- **An image is untrusted input** (`CLAUDE.md`). Every new count, index and id the decoder reads -- a module's text count, a row's text index, a frame's code id -- is validated before it indexes or allocates, counts through `bounded_count`; a crafted value is a `DecodeError`, never a panic. The loader still never parses: a text is parsed only by an edit, after load.
- **A wire tag is assigned once and never renumbered or reused** (`CLAUDE.md`). `OBJECT_FRAME` takes 15 and 10 joins `RETIRED_OBJECT_BODIES`; `FLAG_HAS_SOURCE` takes 8 and 1 joins `RETIRED_HEADER_FLAGS`; `tests/opcode_tags.rs` pins both in the same commit.
- **`crates/ns-runtime/tests/fixtures/golden-image.bin` does not change in Tasks 1-3**: the fixture is source-free and holds no frame, and a source-free row is written exactly as today. If `the_wire_format_matches_the_golden_fixture` fails there, stop and report. Task 4 edits a comment of `MirrorsFactory.ns`, which changes the shipped world's fingerprint, so it regenerates the fixture: `cargo nextest run -p ns-runtime --test image --run-ignored only regenerate_the_golden_fixture`, then `cmp -l` against the previous fixture (`git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $TEMP/old-fixture.bin`) shows that only the 8 fingerprint bytes after the flag byte differ.
- **Program outcomes are pinned, not re-derived** (`CLAUDE.md`). No golden's `status`, `stdout` or `stderr` changes in this plan; Task 5 deletes only the `skipped_in_images` lines. If an outcome would change, stop and report.
- **The VM mirror is a capability** (`CLAUDE.md`, the six locks). Nothing here adds a leaf or a way to reach the mirror; an edit still resolves with `resolve`, never `resolve_kernel`, and still refuses a shipped mixin.
- **A handle means nothing across a move.** The encoder runs between sends and allocates nothing; the graph table it builds first holds handles only until the graph is written, within one entry.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult`; tests use the local `expect_ok` helper of their file. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- After editing a `.ns` file run `cargo run -p ns-analyzer -- fmt <the files>`. Every member of `MirrorsFactory.ns` keeps its Javadoc-standard `///` comment.
- Comments explain the decision, not the mechanics. Documentation describes the current state: a comment that says what "road step 5b2b" will do is rewritten to say what is, or names road step 5b2c where it is about reclamation.

## Review Focus

1. **An enclosing class's method source after a nested edit, through an image.** `Inner` edited, the program written with source and loaded: `Outer`'s method `source` answers its text from the text `Outer` was read in, `Inner`'s from the edited text, and `LoadedModule::source` the current text. Pinned by Task 2's `an_enclosing_method_keeps_its_source_after_a_nested_edit`.
2. **Two generations.** A loaded program edited, written again and loaded again: the second image's current text holds both edits, and every row's text is one the second world held. Pinned by Task 4's `a_loaded_program_edited_and_written_again_loads_with_both_edits`.
3. **A closure over code no version has any more.** A method removed by an edit after a closure of it was stored: the closure is written, loads, and runs its old block, whose home frame's body is the removed method's. Pinned by Task 3's `a_closure_of_a_removed_method_runs_after_a_load`.
4. **Crafted bytes in the new fields.** A row's text index past its module's texts, a module's text count past the bytes left, a frame's code id past the table, a method frame whose code is a block, a closure frame whose code is not its closure's: each a `DecodeError`, never a panic. Pinned by Task 1's `a_row_text_index_past_its_module_texts_is_refused` and Task 3's `a_frame_whose_code_disagrees_with_its_kind_is_refused`.
5. **A loaded program whose text is not its code.** An image whose text was swapped for a valid module of other classes, and one with no source: the edit answers a diagnostic (`<name>'s source text does not hold its declaration`, `<name> has no source text to edit`) and changes nothing; never a panic. Pinned by Task 4's `a_loaded_program_whose_text_lacks_the_declaration_is_refused` and `a_loaded_program_without_source_is_refused`.

---

### Task 1: The module section carries every text a row is read in

**Files:**
- Modify: `crates/ns-runtime/src/image/modules.rs` (`ModuleSource`, `encode_modules`, `decode_modules`, `DecodedModule`)
- Modify: `crates/ns-runtime/src/image/mixins.rs` (`plan`'s `Row` gains `text_index`, `encode`, `read`, `read_row`, `RawRow`, `build`)
- Modify: `crates/ns-runtime/src/image/mod.rs` (`encode_describing`, `write_describing`, `load`, `decode_described`, `LoadedModule::source`)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`FLAG_HAS_SOURCE`, `KNOWN_FLAGS`, `RETIRED_HEADER_FLAGS`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (new `DecodeError` variants)
- Modify: `crates/ns-runtime/src/image/inspect.rs` (the module section and the row's text index)
- Modify: every `image::ModuleSource { .. }` construction: `crates/ns-runtime/src/interpreter/mirror.rs:647`, `crates/ns-runtime/src/interpreter/install.rs` (test), `crates/ns-runtime/tests/image.rs` (`snapshot_one`, `module_only_image` and their ~60 callers), `crates/ns-runtime/tests/image_truncations.rs`, `crates/ns-runtime/tests/trampoline.rs:417`, `crates/ns-analyzer/tests/common/mod.rs:133`, `crates/ns-analyzer/tests/disasm_snapshots.rs:284`, `crates/ns-cli/src/program.rs:256` (`module_sources` keeps its `with_source` parameter)
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`

**Interfaces:**
- Produces: `pub struct ModuleSource<'a, 'gc> { pub class: ClassReference<'gc>, pub origin: &'a SourceOrigin, pub with_source: bool }`; `pub struct DecodedModule { pub origin: SourceOrigin, pub texts: Vec<Rc<str>>, pub root: u32 }` (empty `texts` without source; `texts[0]` the current text); `LoadedModule::source: Option<Rc<str>>` stays and answers the current text; `mixins::Row::text_index: Option<u32>`; `mixins::read(reader, symbols, codes, has_source: bool)`; `mixins::build(table, codes, module_texts: &[Vec<Rc<str>>]) -> Result<Decoded, DecodeError>`; `encode::FLAG_HAS_SOURCE == 8`; `DecodeError::TextIndexOutOfRange { row: u32, index: u32, texts: usize }`.

**The wire, with source** (`FLAG_HAS_SOURCE` set). A mixin row ends with one more field, after its superclass message code id: a `u32` text index into its module's texts. A module record is its origin, then a `u32` text count (at least 1) and that many texts (`write_str`), the module's current text first, then its root's object id. Without source both are absent, so a source-free image is byte for byte what it is today.

**Which texts a module carries.** The encoder reads every text from the heap, never from the host: the current text is `Heap::current_module_text(id of the module's root mixin)`; each row's text is its mixin's `module_text`. The module's list is the current text, then every distinct other text (by `Rc::ptr_eq`) in row order. A module asked for source that has no current text, or one of whose rows has no text, is refused: ``image encoding cannot carry the source of module `<root name>`, which was built without it``.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/image.rs`, change `snapshot_one` and `module_only_image` to take `with_source: bool` in place of `source: Option<&str>`, and every caller: `Some(<anything>)` becomes `true`, `None` becomes `false`. Before replacing a `Some(text)`, check that `text` is the very text the module was compiled from (the `load` of the same test); if a test passes another text on purpose, stop and report it -- the encoder no longer takes a text from the host. Then add, beside `the_wire_format_matches_the_golden_fixture`:

```rust
/// A row's text index is checked against its module's texts before it
/// indexes them: a crafted index is refused, not a panic.
#[test]
fn a_row_text_index_past_its_module_texts_is_refused() {
    let (mut interpreter, class) = load(TRIVIAL_MODULE);
    let bytes = interpreter.enter(|mutator| {
        let class = class_reference(mutator, &class);
        module_only_image(mutator, true, &Origin::File(PathBuf::from("t.ns")), class)
            .expect("snapshot")
    });
    // The module's one row names text 0, the module's one text; the
    // index is the last `u32` of the row, just before the module section.
    let mut crafted = bytes.clone();
    let offset = row_text_index_offset(&crafted, 0);
    crafted[offset..offset + 4].copy_from_slice(&1_u32.to_le_bytes());
    match image::load(&crafted) {
        Err(message) => assert!(
            message.contains("names text 1 of its module, which carries 1"),
            "{message}"
        ),
        Ok(_) => panic!("a text index past the module's texts loaded"),
    }
}
```

That a source-free image is written exactly as before is what `the_wire_format_matches_the_golden_fixture` already pins.

`row_text_index_offset(bytes, row)` is a test helper you write with `inspect`'s reader (`ns_runtime::image::inspect`), which already walks the sections and knows where the mixin table's rows end; if `inspect` answers no offsets, make it record each row's end offset in its row description (`inspect::Row::end`, a `usize`) rather than duplicating the walk in the test. `TRIVIAL_MODULE` exists in `image.rs`.

In `crates/ns-runtime/tests/opcode_tags.rs`'s `header_flags_are_fixed`:

```rust
    assert_eq!(encode::FLAG_HAS_SOURCE, 8);
    assert_eq!(encode::FLAG_START, 4);
    // Bit 2 was `FLAG_STATELESS`, bit 1 the source variant with one text
    // per module; both are retired and never reused.
    assert_eq!(encode::RETIRED_HEADER_FLAGS, 2 | 1);
```

`a_header_flag_this_build_does_not_assign_is_rejected` in `image.rs` sets bit 3 (8), which is now assigned: move it to bit 4 (16), the next unassigned one, and add a sibling test that bit 0 (1), now retired, is refused the same way.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --test image --test opcode_tags -E 'test(text_index) | test(header_flag)'`
Expected: compile errors (`with_source`, `row_text_index_offset`), then, once it compiles, FAIL.

- [ ] **Step 3: Implement**

`encode.rs`:

```rust
/// The image carries its program's source: per module, its current text
/// and every earlier text a mixin row is read in, and per row the index of
/// its text (`docs/design/image-format.md`, "Source").
pub const FLAG_HAS_SOURCE: u8 = 8;
pub const FLAG_START: u8 = 4;
pub const KNOWN_FLAGS: u8 = FLAG_HAS_SOURCE | FLAG_START;
/// Flags that once meant something and are never reused: 2 was
/// `FLAG_STATELESS`, the stateless binary, retired 2026-09-29; 1 was the
/// source variant with one text per module, retired 2026-10-19 when an
/// edited module came to carry several, so that an image of the old layout
/// is refused rather than misread.
pub const RETIRED_HEADER_FLAGS: u8 = 2 | 1;
```

Keep the existing doc comments' substance; move the reasons into these.

`mixins.rs`: `plan` takes the texts into account only for the index. Give `Row` a `text_index: Option<u32>` and compute it in a new function the encoder calls, so that `plan` keeps its one job:

```rust
/// Per module, the texts its rows are read in: the module's current text
/// first, then every other text a row holds, in row order, each once.
/// Answers the texts and each row's index into its module's list.
pub fn module_texts(
    rows: &[Row],
    module_rows: &[usize],
    current_texts: &[Rc<str>],
) -> Result<(Vec<Vec<Rc<str>>>, Vec<u32>), String>
```

`module_rows` are the indexes of each module's first row in `rows` (record them in `plan` as it walks the roots), `current_texts` the heap's current text per module. A row whose mixin has no `module_text` is the refusal above. `encode` writes `row.text_index` (a `u32`) after the superclass message's optional id when `Some`. `read` takes `has_source` and reads the index into `RawRow::text_index: Option<u32>`; `build` takes `module_texts: &[Vec<Rc<str>>]`, one list per module (empty without source), answers `Result<Decoded, DecodeError>`, and gives each row the text its index names, refusing an index past its module's list with `TextIndexOutOfRange`, whose message is `mixin-table row {row} names text {index} of its module, which carries {texts}`. `decode` (the `inspect` path) passes empty lists and so builds rows without text, as today.

`modules.rs`: `ModuleSource::with_source`; `encode_modules(writer, modules, root_ids, texts: Option<&[Vec<Rc<str>>]>)` writes per module the text count and texts when `Some`; `decode_modules` reads the count through `decode::bounded_count(count, 4, reader)` (each text at least its `u32` length) and refuses a count of 0 with a new `DecodeError::ModuleWithoutText { module: u32 }` (`module {module} carries source but no text`).

`mod.rs`: `encode_describing` computes `with_source` (every module's flag; the existing "every module or none" refusal moves here, worded as today), asks the heap for each module's current text (`heap.known_mixin_id(root)` then `heap.current_module_text(id)`; a root never registered has its own `module_text`), and calls `mixins::module_texts`. `load` hands `decoded_modules[i].texts` to `mixins::build`, then, inside the graph phase's entry once the module's root class record exists, makes `texts[0]` the module's current text when it is not the root row's own (`heap.mixin_id(&root_mixin)`, then `heap.set_current_module_text`; both are `pub(crate)` and `load` is in the crate). `LoadedModule::source` is `texts.first().cloned()`.

`inspect.rs`: the module listing says how many texts a module carries (`source, 2 texts` / `no source`), and a row shows its text index when present.

Every other `ModuleSource` construction: `source: None` → `with_source: false`; `source: Some(text)` → `with_source: true`; `crates/ns-analyzer/tests/common/mod.rs`'s `with_source.then_some(text.as_str())` → `with_source`.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime --test image --test opcode_tags --test image_truncations --test trampoline` then `cargo nextest run -p ns-analyzer`
Expected: PASS, the fixture test included, unchanged. An `inspect` snapshot of a with-source image under `crates/ns-analyzer/tests/` may change by the `2 texts` wording: check the diff says only that, then accept it.

- [ ] **Step 5: Commit**

Run: `cargo xtask gates commit images analyzer`
```bash
git add -A crates/ns-runtime crates/ns-analyzer crates/ns-cli
git commit -m "feat: carry every text an image's rows are read in, with the module's current text first"
```

---

### Task 2: An edited program is written to an image

**Files:**
- Modify: `crates/ns-runtime/src/image/mixins.rs` (`plan` walks the current versions; `MixinIds` keyed by `MixinId`)
- Modify: `crates/ns-runtime/src/image/mod.rs` (`encode_describing`: the refusal goes; the roots are the current versions)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`encode_class_record`, `encode_mixin_record`, `EncodeContext::mixin_ids`, `encode_graph_section`'s parameter)
- Modify: `crates/ns-runtime/src/image/modules.rs` (`root_record_defect` compares mixins with `Heap::same_mixin`)
- Modify: `crates/ns-runtime/src/heap.rs`, `crates/ns-runtime/src/heap/tables.rs` (`Tables::edited` and `Heap::mixin_is_edited` go if nothing else reads them)
- Test: `crates/ns-runtime/src/interpreter/install.rs` (its `tests` module)

**Interfaces:**
- Consumes: Task 1's `ModuleSource::with_source`, `mixins::module_texts`.
- Produces: `pub(crate) type MixinIds = HashMap<MixinId, u32>`; `mixins::plan(heap: &Heap<'_>, module_roots: &[Rc<Mixin>]) -> Result<Vec<Row>, String>`; `encode::encode_code_table(writer, rows: &[mixins::Row], extra: &[Rc<CompiledCode>], symbols)` (Task 3 fills `extra`; pass `&[]` here).

**What changes.** A class object an install re-applied names an application of its mixin's current version, but a mixin's `nested` and `class_side` still hold the versions it was built with, a level's `AppliedClause::declaration` the version that applied it, and a module root's `mixin_applications` clause the root's old version. So the encoder never keys a mixin by its address: `plan` replaces every mixin it reaches by `heap.mixin(id)` when `heap.known_mixin_id(mixin)` answers an id (a mixin no class object applied has none and is taken as it is), and every lookup of a row -- a class record's mixin, an applied clause's declaration, a reified mixin's instance side -- goes through `heap.known_mixin_id`. `root_record_defect` compares with `heap.same_mixin(candidate, root_mixin, heap.known_mixin_id(root_mixin))` rather than `Rc::ptr_eq`, which the decoder's freshly built mixins still satisfy by reference.

- [ ] **Step 1: Write the failing tests**

Replace `an_edited_program_is_refused_with_an_error` in `install.rs`'s tests by a section "Images of an edited program" with these tests and helpers:

```rust
    /// `class`'s module written with its source, `roots` the graph's roots.
    fn image_of(interpreter: &mut Interpreter, class: &Root, roots: &[Root]) -> Vec<u8> {
        interpreter.enter(|mutator| {
            let class = mutator
                .fetch(class)
                .as_object()
                .expect("the module is a class object");
            let origin = SourceOrigin::File(PathBuf::from("t.ns"));
            let modules = [crate::image::ModuleSource {
                class,
                origin: &origin,
                with_source: true,
            }];
            let roots: Vec<Value<'_>> = roots.iter().map(|root| mutator.fetch(root)).collect();
            crate::image::snapshot(mutator, &modules, &roots).expect("snapshot")
        })
    }

    fn loaded(bytes: &[u8]) -> crate::image::LoadedImage {
        match crate::image::load(bytes) {
            Ok(image) => image,
            Err(message) => panic!("load: {message}"),
        }
    }

    /// The source a loaded world's mixin answers for `selector`, sliced as
    /// the method mirror's leaf slices it.
    fn method_source_in(
        interpreter: &mut Interpreter,
        class: &Root,
        nested: Option<&str>,
        selector: &str,
    ) -> Option<String> {
        interpreter.enter(|mutator| {
            let mut mixin = mixin_of(mutator, class);
            if let Some(name) = nested {
                let id = mutator.heap.known_mixin_id(
                    mixin.nested.iter().find(|each| each.name.as_str() == name)?,
                )?;
                mixin = mutator.heap.mixin(id)?;
            }
            let id = mutator.heap.known_mixin_id(&mixin)?;
            let text = mutator.heap.mixin_module_text(id)?;
            let method = mixin.compiled_method(Symbol::intern(selector))?;
            crate::mixin::source_text(&text.0, method.code.source).map(str::to_string)
        })
    }

    /// `OUTER` loaded and instantiated: the module class, an `Outer`, an
    /// `Inner` made before any edit, and `Inner`'s id.
    fn outer_inner_and_module(source: &str) -> (Interpreter, Root, Root, Root, MixinId) {
        let (mut interpreter, outer) = load(source);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        (interpreter, outer, inner_class, inner, inner_id)
    }

    fn is_nil(interpreter: &mut Interpreter, root: &Root) -> bool {
        interpreter.enter(|mutator| mutator.fetch(root).is_nil())
    }

    /// An edited nested class is written and loads edited: its added
    /// method answers on an instance made before the edit.
    #[test]
    fn an_edited_program_loads_edited() {
        let (mut interpreter, outer, _, inner, inner_id) = outer_inner_and_module(OUTER);
        install(&mut interpreter, inner_id, &add_method("public b = ( ^x + 41 )"));
        let bytes = image_of(&mut interpreter, &outer, &[inner]);
        let mut image = loaded(&bytes);
        let inner = image.roots[0].clone();
        let answer = send(&mut image.interpreter, &inner, "b");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(42));
    }

    /// A slot added through the header is in the image's layout: an old
    /// instance migrated to `nil`, a new one initialized.
    #[test]
    fn a_header_edit_loads_with_its_layout() {
        let (mut interpreter, outer, inner_class, old, inner_id) = outer_inner_and_module(OUTER);
        install(
            &mut interpreter,
            inner_id,
            &header_edit("public class Inner = (| public x ::= 1. public y ::= 2. |)"),
        );
        let new = send(&mut interpreter, &inner_class, "new");
        let bytes = image_of(&mut interpreter, &outer, &[old, new]);
        let mut image = loaded(&bytes);
        let (old, new) = (image.roots[0].clone(), image.roots[1].clone());
        let old_y = send(&mut image.interpreter, &old, "y");
        assert!(is_nil(&mut image.interpreter, &old_y));
        let new_y = send(&mut image.interpreter, &new, "y");
        assert_eq!(integer(&mut image.interpreter, &new_y), Some(2));
        let old_x = send(&mut image.interpreter, &old, "x");
        assert_eq!(integer(&mut image.interpreter, &old_x), Some(1));
    }

    /// After an edit of `Inner`, `Outer`'s methods still lie in the text
    /// `Outer` was read in, and the loaded module's current text holds the
    /// edit.
    #[test]
    fn an_enclosing_method_keeps_its_source_after_a_nested_edit() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Inner = (| public x ::= 1. |) (
                public a = ( ^1 )
              )
              public after = ( ^2 )
            )
        "};
        let (mut interpreter, outer) = load(source);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        install(
            &mut interpreter,
            inner_id,
            &add_method("public added = ( ^'a longer method that moves the text after it' )"),
        );
        let bytes = image_of(&mut interpreter, &outer, &[instance, inner_class]);
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        assert_eq!(
            method_source_in(&mut image.interpreter, &outer, None, "after").as_deref(),
            Some("public after = ( ^2 )")
        );
        assert_eq!(
            method_source_in(&mut image.interpreter, &outer, Some("Inner"), "added").as_deref(),
            Some("public added = ( ^'a longer method that moves the text after it' )")
        );
        let current = image.modules[0].source.clone().expect("the image carries source");
        assert!(current.contains("public added"));
        assert!(current.contains("public after = ( ^2 )"));
    }

    /// An edit of a module's root class whose `<:` clause applies `Value`
    /// is written: the clause's level names the root's old version, and
    /// the root rule holds by id.
    #[test]
    fn an_edited_root_with_a_clause_is_written() {
        let (mut interpreter, root) = load(ROOT_WITH_A_VALUE_CLAUSE);
        let instance = expect_ok(interpreter.instantiate(&root, &[]), "a Root");
        let root_id = interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, &root);
            mutator.heap.known_mixin_id(&mixin).expect("the root is applied")
        });
        install(&mut interpreter, root_id, &add_method("public b = ( ^2 )"));
        let bytes = image_of(&mut interpreter, &root, &[instance]);
        let mut image = loaded(&bytes);
        let instance = image.roots[0].clone();
        let answer = send(&mut image.interpreter, &instance, "b");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(2));
    }
```

`ROOT_WITH_A_VALUE_CLAUSE` is a module whose root class applies `Value` through its one `<:` clause and declares `public a = ( ^1 )`: take the clause's exact spelling from an existing golden or test that builds such a root (`git grep -n "<: Value" goldens crates/ns-runtime/tests` finds them), as an `indoc!` constant beside `OUTER`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --lib -E 'test(edited_program) | test(header_edit_loads) | test(keeps_its_source_after) | test(edited_root)'`
Expected: FAIL with `image encoding cannot yet serialize mixin ...`.

- [ ] **Step 3: Implement**

`mixins::plan(heap, module_roots)`: the preorder walk, over the current versions:

```rust
/// `mixin`'s current version: an install replaced it under its id when
/// one names it, and a mixin no class object applied has none.
fn current<'gc>(heap: &Heap<'gc>, mixin: &Rc<Mixin>) -> Rc<Mixin> {
    heap.known_mixin_id(mixin)
        .and_then(|id| heap.mixin(id))
        .unwrap_or_else(|| Rc::clone(mixin))
}
```

Walk iteratively or with `mixin::preorder`'s order, visiting `current(nested)` and `current(class_side)`; row ids are keyed by `heap.known_mixin_id` where there is one. A mixin without an id cannot be named by any record (no class object applies it and nothing reified it), so it needs no key; give the row map the type `MixinIds = HashMap<MixinId, u32>` and skip those. `encode_code_table` takes the planned rows instead of re-planning them (`encode_describing` plans once and passes the rows to both). The three lookups in `encode.rs` (`:1546`, `:1594`, `:1843`) become `context.heap.known_mixin_id(mixin).and_then(|id| context.mixin_ids.get(&id))`, their error texts unchanged. `encode_describing` loses the refusal; if `Heap::mixin_is_edited` and `Tables::edited` then have no reader but tests, delete them and the asserts that read them (`install.rs:835-836`, `:1050`), replacing each with an assert on what the test means (`heap.mixin(id)` is the fresh version, or is unchanged).

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime`
Expected: PASS.

- [ ] **Step 5: Commit**

Run: `cargo xtask gates commit images`
```bash
git add -A crates/ns-runtime
git commit -m "feat: write an edited program to an image, naming each mixin by its id"
```

---

### Task 3: The code an older version runs, and frames that name their body

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs` (`write_image`: walk the graph first)
- Modify: `crates/ns-runtime/src/image/encode.rs` (split `encode_graph_section` into `walk_graph` and `encode_graph_section`; `encode_code_table`'s extra bodies; `encode_frame`)
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OBJECT_FRAME = 15`, `RETIRED_OBJECT_BODIES` gains 10, the record's doc)
- Modify: `crates/ns-runtime/src/image/decode.rs` (the frame record's two passes, `method_declaration`, `verify_graph`'s kind check, new `DecodeError` variants)
- Modify: `crates/ns-runtime/src/heap.rs` (`fill_frame_fields` takes the body; `Heap::superseded_mixins`)
- Modify: `crates/ns-runtime/src/image/inspect.rs` (the frame record)
- Test: `crates/ns-runtime/src/interpreter/install.rs` (tests), `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`

**Interfaces:**
- Consumes: Task 2's `mixins::plan(heap, ..)`, `encode_code_table(writer, rows, extra, symbols)`.
- Produces: `encode::walk_graph<'gc>(mutator: &Mutator<'gc>, roots: &[Value<'gc>], module_roots: &[ClassReference<'gc>]) -> Result<WalkedGraph<'gc>, String>` (the graph table and the shipped indexes, every refusal `encode_graph_section` makes before writing); `WalkedGraph::bodies(&self, heap) -> Vec<Rc<CompiledCode>>` (each closure's code and each frame's body, in object order); `encode_graph_section(mutator, writer, walked: &WalkedGraph<'gc>, roots, code_ids, mixin_ids, symbols)`; `Heap::fill_frame_fields(&mut self, frame, record: FrameRecord<'gc>, code: Option<Rc<CompiledCode>>)`; `pub(crate) fn Heap::superseded_mixins(&self) -> &[Rc<Mixin>]`; `opcodes::OBJECT_FRAME == 15`; `DecodeError::FrameCodeMismatch` (`a frame's body is not one its kind runs`).

**The order of `write_image`.** Walk the graph (`walk_graph`), which refuses the mirror, an unnamed shipped class and a shipped module root as today; plan the rows; collect the extra bodies; write the code table and the mixin table; write the graph with the walked table; then the names and the header as today.

**Which extra bodies are admitted.** Every version of every mixin the rows describe: each row's mixin and each superseded version whose `heap.known_mixin_id` is the id of a row's mixin (`Heap::superseded_mixins`, filtered). The admitted bodies are those versions' methods' code, initializer, superclass clause and message, and `<:` message codes, with their nested blocks (`collect_code`'s walk over `MakeClosure`). A body of `WalkedGraph::bodies` that is a row's is already in the table; one that is admitted joins it after the rows' bodies, in graph order, with its blocks before it; any other is refused with the existing ``image encoding cannot serialize a closure over code that no module the image carries compiled`` (for a frame's body: ``... a frame running code that ...``).

**The frame record (15).** `OBJECT_FRAME`'s body as today, except that the kind's payload is followed by the body: a presence byte and a `u32` code id, absent exactly for a top-level frame; and a method frame's payload is its selector and its method's access tag (`mixins::access_tag`). Decoding: the first pass sizes the record over the new field; the second reads the code id against the code table (`BadCodeId`), and for a method frame takes the current declaration's method of that selector (`method_declaration`) when its `code` is this body (`Rc::ptr_eq`), and otherwise builds one:

```rust
Rc::new(MixinMethod {
    selector,
    access,
    argument_count: usize::from(code.argument_count),
    // A version an install superseded: no declaration, and no dispatch
    // finds it, since a decoded frame is never entered again.
    declaration: None,
    code: Rc::clone(&code),
    fast_path: None,
    class_accessor: None,
})
```

`fill_frame_fields` records the body it is handed instead of the one `kind_code` would find in the class's current mixin, which after an edit is another. `verify_graph` checks that the body is one the frame's kind runs, before the program-counter and register checks: a method frame a `CodeKind::Method` body whose argument count is its method's, a closure frame its closure's own code (`Rc::ptr_eq`), an initializer frame `CodeKind::Initializer`, a superclass clause `CodeKind::SuperclassClause`, a superclass message and a `<:` clause's message `CodeKind::SuperclassMessage` (check `compiler/` for the kind a `<:` message body is compiled with and use that); otherwise `FrameCodeMismatch`.

- [ ] **Step 1: Write the failing tests**

In `install.rs`'s "Images of an edited program":

```rust
    /// A closure made by a method an edit removed keeps its block across
    /// an image: the block and its home frame's body enter the code table.
    #[test]
    fn a_closure_of_a_removed_method_runs_after_a_load() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        let block = send(&mut interpreter, &inner, "block");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        install(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod(Symbol::intern("block"))]),
        );
        let bytes = image_of(&mut interpreter, &outer, &[block]);
        let mut image = loaded(&bytes);
        let block = image.roots[0].clone();
        let answer = send(&mut image.interpreter, &block, "value");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(1));
    }

    /// A replaced method's closure keeps the old block's answer after a
    /// load, while the method answers the new one.
    #[test]
    fn a_closure_of_a_replaced_method_keeps_its_old_code_after_a_load() {
        let (mut interpreter, outer, _, inner, inner_id) = outer_inner_and_module(OUTER);
        let old_block = send(&mut interpreter, &inner, "block");
        install(&mut interpreter, inner_id, &add_method("public block = ( ^[ 2 ] )"));
        let bytes = image_of(&mut interpreter, &outer, &[old_block, inner]);
        let mut image = loaded(&bytes);
        let (old_block, inner) = (image.roots[0].clone(), image.roots[1].clone());
        let old_answer = send(&mut image.interpreter, &old_block, "value");
        assert_eq!(integer(&mut image.interpreter, &old_answer), Some(1));
        let new_block = send(&mut image.interpreter, &inner, "block");
        let new_answer = send(&mut image.interpreter, &new_block, "value");
        assert_eq!(integer(&mut image.interpreter, &new_answer), Some(2));
    }
```

The captured frame of a closure made by `block` is the method's frame, so `a_closure_of_a_removed_method_runs_after_a_load` also writes a method frame whose body no row holds.

In `tests/image.rs`, beside the existing crafted-frame tests -- search for `CapturedFrameMismatch` and `ProgramCounterOutOfRange`, and locate the frame record the way they do (they find a record's offset with `inspect`'s reader; extend `inspect`'s frame description with the offset of its code id, `inspect::Frame::code_offset: Option<usize>`, if no existing offset reaches it) -- two tests over the image those tests build from a method that answers a block (a closure over a method-local block, whose captured frame is a method frame):

- `a_frame_whose_code_disagrees_with_its_kind_is_refused`: the captured method frame's code id rewritten to the block's own code id (the closure record's code id, read from the same image); `image::load` answers an `Err` whose message contains `a frame's body is not one its kind runs`.
- `a_frame_code_id_past_the_table_is_refused`: the same code id rewritten to the code table's length; `image::load` answers an `Err` naming that id, as `BadCodeId`'s message does.

Each with a doc comment saying what the crafted bytes would otherwise do: decode a frame against a body it never ran.

In `opcode_tags.rs`, pin `OBJECT_FRAME == 15` and that 10 is in `RETIRED_OBJECT_BODIES`, and the next free number comment (16).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime -E 'test(closure_of_a_removed) | test(closure_of_a_replaced) | test(frame_whose_code) | test(frame_code_id) | test(tags)'`
Expected: FAIL: the encoder refuses the closure over code no module compiled; the tags differ.

- [ ] **Step 3: Implement** as described above. `Heap::superseded_mixins` answers `&self.tables.superseded`. `inspect` prints a frame's body as `code <id>` after its kind, and a method frame's access.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime` then `cargo nextest run -p ns-cli --profile images`
Expected: PASS; the golden fixture unchanged (it holds no frame).

- [ ] **Step 5: Commit**

Run: `cargo xtask gates commit images`
```bash
git add -A crates/ns-runtime
git commit -m "feat: carry the code an older version runs, and name a frame's body by code id"
```

---

### Task 4: A program loaded from an image can be edited

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/edit.rs` (`prepare_edit`: the refusal of a mixin with no declaration goes)
- Modify: `crates/ns-runtime/core-library/platform/MirrorsFactory.ns` (`asBuilder`'s comment drops the loaded-program diagnostic)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated: the fingerprint)
- Test: `crates/ns-runtime/src/interpreter/install.rs` (tests)

**Interfaces:**
- Consumes: Tasks 1-3: an image carries its text, its edited rows and old code.
- Produces: nothing new; `prepare_edit` answers for a decoded mixin what it answers for a compiled one.

A decoded mixin has no declaration (`Mixin::declaration` is `None`), but `prepare_edit` never reads it: it parses the module's current text and finds the declaration by its path of names, and pairs the fresh mixins with the old ones by name and id. The loader makes the image's first text the module's current one (Task 1), so the first edit parses it -- after load, never during. A crafted image whose text disagrees with its code is edited into what its text says, which is a valid program, or refused with the diagnostics that already exist.

- [ ] **Step 1: Write the failing tests**

```rust
    /// A program loaded from an image with source is edited like one
    /// compiled from source, and its instances migrate.
    #[test]
    fn a_loaded_program_is_edited() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        let bytes = image_of(&mut interpreter, &outer, &[inner]);
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let inner = image.roots[0].clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        install(
            &mut image.interpreter,
            inner_id,
            &header_edit("public class Inner = (| public x ::= 1. public y ::= 5. |)"),
        );
        let x = send(&mut image.interpreter, &inner, "x");
        assert_eq!(integer(&mut image.interpreter, &x), Some(1));
        let y = send(&mut image.interpreter, &inner, "y");
        assert!(image.interpreter.enter(|mutator| mutator.fetch(&y).is_nil()));
    }

    /// Edited before an image and again after its load, written again and
    /// loaded again: the second image holds both edits.
    #[test]
    fn a_loaded_program_edited_and_written_again_loads_with_both_edits() {
        let (mut interpreter, outer, _, inner, inner_id) = outer_inner_and_module(OUTER);
        install(&mut interpreter, inner_id, &add_method("public b = ( ^2 )"));
        let first = image_of(&mut interpreter, &outer, &[inner]);
        let mut first = loaded(&first);
        let outer = first.modules[0].class.clone();
        let inner = first.roots[0].clone();
        let inner_id = nested_id_of(&mut first.interpreter, &outer, "Inner");
        install(&mut first.interpreter, inner_id, &add_method("public c = ( ^3 )"));
        let second = image_of(&mut first.interpreter, &outer, &[inner]);
        let mut second = loaded(&second);
        let inner = second.roots[0].clone();
        let b = send(&mut second.interpreter, &inner, "b");
        assert_eq!(integer(&mut second.interpreter, &b), Some(2));
        let c = send(&mut second.interpreter, &inner, "c");
        assert_eq!(integer(&mut second.interpreter, &c), Some(3));
        let text = second.modules[0].source.clone().expect("the image carries source");
        assert!(text.contains("public b = ( ^2 )") && text.contains("public c = ( ^3 )"));
    }

    /// An image written without source: the edit answers 5b2a's
    /// diagnostic and changes nothing.
    #[test]
    fn a_loaded_program_without_source_is_refused() {
        let (mut interpreter, outer, _, inner, _) = outer_inner_and_module(OUTER);
        let bytes = interpreter.enter(|mutator| {
            let class = mutator.fetch(&outer).as_object().expect("a class object");
            let origin = SourceOrigin::File(PathBuf::from("t.ns"));
            let modules = [crate::image::ModuleSource {
                class,
                origin: &origin,
                with_source: false,
            }];
            let roots = [mutator.fetch(&inner)];
            crate::image::snapshot(mutator, &modules, &roots).expect("snapshot")
        });
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        let refused = image
            .interpreter
            .enter(|mutator| mutator.install_edit(inner_id, &add_method("public b = ( ^2 )")));
        match refused {
            Err(message) => assert_eq!(message, "Inner has no source text to edit"),
            Ok(_) => panic!("a program without source was edited"),
        }
    }

    /// A crafted image whose text holds other classes than its code: the
    /// edit is refused with the existing diagnostic, never a panic.
    #[test]
    fn a_loaded_program_whose_text_lacks_the_declaration_is_refused() {
        let (mut interpreter, outer, _, inner, _) = outer_inner_and_module(OUTER);
        let mut bytes = image_of(&mut interpreter, &outer, &[inner]);
        // A valid module of the same length that declares no `Inner`: the
        // text is length-prefixed, so the swap keeps every other byte.
        let mut swapped = String::from("class Outer = Object new () ()\n");
        while swapped.len() < OUTER.len() {
            swapped.push(' ');
        }
        let start = bytes
            .windows(OUTER.len())
            .position(|window| window == OUTER.as_bytes())
            .expect("the image carries the module's text");
        bytes[start..start + OUTER.len()].copy_from_slice(swapped.as_bytes());
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        let refused = image
            .interpreter
            .enter(|mutator| mutator.install_edit(inner_id, &add_method("public b = ( ^2 )")));
        match refused {
            Err(message) => {
                assert_eq!(message, "Inner's source text does not hold its declaration");
            }
            Ok(_) => panic!("an edit was installed from a text without its declaration"),
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --lib -E 'test(loaded_program)'`
Expected: `a_loaded_program_is_edited` and `..._written_again_...` FAIL with `Inner was loaded from an image, and a loaded program cannot be edited yet`; the two refusals may already pass.

- [ ] **Step 3: Implement**

Delete the refusal (`edit.rs:186-190`) and its row in any diagnostics list in the file's docs. In `MirrorsFactory.ns`'s `asBuilder` comment, drop the clause `'<name> was loaded from an image, and a loaded program cannot be edited yet' for a class an image decoded;` and keep the other two; say instead that a class an image decoded is edited from the text the image carries. Run `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/platform/MirrorsFactory.ns`. Regenerate the golden fixture as the Global Constraints say, and confirm only the fingerprint moved.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime`
Expected: PASS.

- [ ] **Step 5: Commit**

Run: `cargo xtask gates commit images`
```bash
git add -A crates/ns-runtime
git commit -m "feat: edit a program loaded from an image from the text the image carries"
```

---

### Task 5: `build` writes the source, and every golden goes through images

**Files:**
- Modify: `crates/ns-cli/src/main.rs` (`Build::debug_source` → `strip_source`; `Load::print_source`'s help)
- Modify: `crates/ns-cli/src/commands/build.rs`, `crates/ns-cli/src/commands/load.rs:31`
- Modify: `crates/ns-cli/tests/build_load.rs:212-290` (the source test inverted)
- Modify: `crates/ns-cli/tests/common/mod.rs` (`Expectation::skipped_in_images` and its doc go; `:88`'s field)
- Modify: `crates/ns-cli/tests/golden_round_trips.rs` (the skip on it goes; the module doc's "An artifact carries no source")
- Modify: `crates/ns-cli/tests/run_goldens.rs:101-147` (the guard's second half goes)
- Modify: the 33 `goldens/library/mirror-builder/**/*.toml` (the `skipped_in_images` line goes)
- Modify: `README.md:165`

**Interfaces:**
- Consumes: Tasks 1-4.
- Produces: `ns-next build <program> <out> [--strip-source] [--lowering ..]`.

- [ ] **Step 1: Write the failing test**

Rewrite `a_built_image_is_source_free_unless_debug_source_is_asked_for` as `a_built_image_carries_its_source_unless_stripped`: the default `build` contains every source line of 12 characters or more and `load --print-source` prints the text verbatim; `build --strip-source` contains none of them, still loads and prints `42`, and `load --print-source` fails with `carries no source (it was written with --strip-source)`; the stripped image is the smaller. Keep the existing comments' reasons (the 12-character floor, what the source-free property does not claim), reworded for the new default.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p ns-cli --test build_load -E 'test(carries_its_source)'`
Expected: FAIL (`--strip-source` is unknown).

- [ ] **Step 3: Implement**

```rust
        /// Leave every module's source text out of the image. An image
        /// without it runs as well, but a program loaded from it cannot be
        /// edited and `load --print-source` has nothing to print.
        #[arg(long)]
        strip_source: bool,
```

`build::execute` takes `with_source: bool` and passes `!strip_source`. `load.rs`'s message names `--strip-source`. Delete the `skipped_in_images` line from every golden `.toml` under `goldens/library/mirror-builder/` (`git grep -l skipped_in_images goldens` lists them; the key appears nowhere else), then the field, its accessor and its doc from `common/mod.rs`, the `|| expectation.skipped_in_images().is_some()` from `golden_round_trips.rs` (its comment says only what `skipped` means), and the `blank_in_images` half of `the_guards_on_skipped_refuse_what_they_promise_to` with its sentence in the doc comment. `deny_unknown_fields` now refuses a stale `skipped_in_images` key, which is the guard that stays. In `README.md`, describe `--strip-source` where `--debug-source` was.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-cli` then `cargo nextest run -p ns-cli --profile images -E 'test(mirror_builder) | test(mirror-builder)'` (the trial names carry the golden's path; list them with `--list` if the filter finds none)
Expected: PASS; every mirror-builder golden runs through `build` then `load` under both lowerings with its recorded outcome.

- [ ] **Step 5: Commit**

Run: `cargo xtask gates commit images`
```bash
git add -A crates/ns-cli goldens README.md
git commit -m "feat: write an image's source unless --strip-source, and run every golden through images"
```

---

### Task 6: The documentation describes the built step

**Files:**
- Modify: `docs/design/image-format.md` ("Source", "Layout", the frame record in "How frames actually get into an image" and "Verification", "Planned: edited mixins in the image" becomes a section of what is, "Packaging"'s `build` synopsis, "Open questions" loses the decompiler, "Decisions" gains decisions 2-4 above)
- Modify: `docs/design/liveness-and-reflection.md` ("Planned: road step 5b2b ..." splits: what this plan built moves into "A MirrorBuilder and an atomic install" or a section of its own, and the rest becomes "Planned: road step 5b2c, reclaiming ids"; the diagnostics table loses the loaded-program row; "What pins it" loses `skipped_in_images`; "What the id has to carry" no longer lists the encoder's address keys)
- Modify: `docs/design/object-model.md`, `docs/design/memory.md`, `docs/design/README.md` (every "road step 5b2b" about the reclamation names road step 5b2c)
- Modify: `docs/OPEN-WORK.md` ("Next" is road step 5b2c, with its four items; the decompiler item goes)
- Modify: `docs/PROGRESS.md` (a row and a section for road step 5b2b, linking this plan)
- Modify: the code comments that still say what road step 5b2b will do: `crates/ns-runtime/src/heap/tables.rs` (three), `crates/ns-runtime/src/interpreter/install.rs` (`refuse_an_unmigratable_instance`'s "the formats road step 5b2b's decoded programs bring"), and any `git grep -n "5b2b" crates` finds
- Check: `CLAUDE.md` ("An image is untrusted input" -- does it still hold as written with several texts and the frame's code id? It names no field that changed; leave it unless a sentence became false) and the `image` description in its `ns-runtime` paragraph

- [ ] **Step 1:** Write the documents. Keep their voice: the current state, the reason beside the rule, the decision with its date (2026-10-19) and "decided with the user". `git grep -n "5b2b\|debug-source\|debug_source\|skipped_in_images\|cannot be edited yet\|decompiler" -- docs crates README.md CLAUDE.md editors` lists what to look at; the plans under `docs/superpowers/plans/` stay as written.
- [ ] **Step 2:** Run: `git diff --stat` -- Markdown and comments only. A Rust comment change runs the `commit` gates (`cargo xtask gates commit`); a Markdown-only commit runs none.
- [ ] **Step 3: Commit**

```bash
git add -A docs crates README.md CLAUDE.md
git commit -m "docs: describe edited programs in images and road step 5b2c"
```
