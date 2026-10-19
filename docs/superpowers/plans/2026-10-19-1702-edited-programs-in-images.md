# Edited programs in images (road step 5b2b) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 5b2b: every declaration holds its own text, its nested declarations cut out; a program a MirrorBuilder edited is written to an image and loads back edited, with the code an older version still runs; an image carries its program's source by default; and a program loaded from an image can be edited, so that every golden, the MirrorBuilder's included, goes through `build` then `load` again.

**Architecture:** A `Mixin` holds a `DeclarationText` instead of its module's whole text: the declaration's own text with every nested declaration cut out as a hole, and the offset it began at, so that the spans the compiler recorded still slice it. A module's text is assembled from the current version of each declaration, so an edit of a nested class leaves its enclosing declaration's text untouched and valid, no version keeps another's text alive, and an unedited module assembles to its file byte for byte. The image carries those texts in a table of their own, each row naming its own. The encoder plans its rows through the mixin ids -- each declaration's current version -- and names every mixin by its `MixinId`. The graph is walked before the code table is written, so the code table also takes every body a closure or a frame of the graph runs that no row reaches, and a frame record names its body by code id. `Mutator::prepare_edit` no longer refuses a mixin decoded from an image, whose text the image now carries. `ns-next build` writes the source unless `--strip-source` is given.

**Tech Stack:** Rust 1.98.0, the Newspeak core library, `cargo-nextest`.

**Spec:** `docs/design/liveness-and-reflection.md`, "Planned: road step 5b2b, edited programs in images, and reclaiming ids" (its first two bullets), "A MirrorBuilder and an atomic install" ("Compiling: the module's text is what an edit edits"), "An edit replaces the mixin under its id"; `docs/design/image-format.md`, "Planned: edited mixins in the image", "Source", "Verification", "How frames actually get into an image"; `docs/design/object-model.md`, "Source retention and provenance". Read them before Task 1. Prior art, for behaviour only: Primordial Soup keeps a text per method and per class header and assembles a declaration's source from them (`prior-art/primordialsoup/newspeak/Mirrors.ns:633-650`, `:717-736`), and serializes methods as graph objects, so a closure over an old method's code is written like any object (`installAll`, `:308-327`); copy no code.

**Decided with the user on 2026-10-19** (not reopened):

1. Road step 5b2b is split in two. This plan: declaration texts, source in images by default, edited programs in images, the code an older version runs, editing a loaded program, every golden through images again, and the decompiler dropped. Road step 5b2c, a plan of its own after this one: the weak application cache and the ids' reuse rules, `Mixin::nested` through ids, and the per-creation marker of the nested-class cache slot.
2. **An image carries its program's source by default.** `ns-next build` writes it; `--strip-source` leaves it out (it replaces `--debug-source`). A program loaded from an image without source cannot be edited (`<name> has no source text to edit`, 5b2a's diagnostic).
3. **Exporting an image back to `.ns` rests on the carried source; there is no decompiler.** A decompiler could only invent the names and comments compiled code no longer has; it is dropped from the open work, and the decision is recorded in `image-format.md`.
4. **The code an older version runs goes into the code table**, and a frame record names its body by code id. A body that belongs to no version of a mixin the image carries is still refused, as a closure over another module's code is today.
5. **Every declaration holds its own text, its nested declarations cut out**, rather than its module's whole text: no version keeps another's text alive, in the heap or in an image; every span slices the text of its own version; and a module's text is assembled from its declarations' current versions. Chosen over carrying every module text a version still points into (which duplicates whole modules, in the heap and the image) and over rebasing spans onto the current text when writing (which leaves the heap's duplicates and drops an old body's source).

**Refined while planning, stated here for review:**

- *Two forms of one text.* A shipped module's declarations share its one text per thread (`interpreter/core_library.rs`'s `SHIPPED_MODULE_TEXTS`, which exists so that no boot copies the core library), since a shipped mixin is never edited and so never outlives the text of another version; a program's declarations each own a copy of their own text with the holes cut out. Both answer the same `slice` and `assemble`.
- *A module's root declaration's text is the whole module*, prologue and trailing text included, its nested declarations cut out; its declaration source is that text sliced at the root's span.
- *An old body's text across an image* waits for its reader. In the live heap a superseded version keeps its text as long as it is held; but nothing yet asks a body for its text -- a method mirror reads the current version's methods -- so the image carries the texts of the rows it writes, and the link from a body to its text comes with the activation mirror that will read it (`OPEN-WORK.md`, added by Task 7).
- *Two retired numbers.* The new layouts take new numbers so that a stale artifact is refused rather than misread (`opcodes.rs`'s rule, applied to the header flag too): `OBJECT_FRAME` 10 is retired and the frame record becomes 15; `FLAG_HAS_SOURCE` 1 is retired and the source variant becomes bit 8.
- *The host no longer hands the text.* `image::ModuleSource::source: Option<&str>` becomes `with_source: bool`; the encoder reads the texts from the heap, since after an edit the host's file is no longer the program's text.
- *A frame of an old method* carries its selector, its method's access and its body's code id; the decoder reuses the current declaration's method when its body is that very code, and otherwise builds a `MixinMethod` of the old body, which no dispatch finds (a decoded frame is never re-entered).

Task 7 writes all of this into the design documents.

## Global Constraints

- Every commit is green: the per-commit gates in `CLAUDE.md` ("The gates") run before each commit through `cargo xtask gates <groups>`, the groups each task names. The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 7, by the controller (`finishing-a-batch` skill); an implementer does not run them, and a task's commit is work in progress until they have run.
- Commits are conventional (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- Work on the branch `road-step-5b2b` in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree. It exists already.
- Run every command in the foreground.
- Names are written out (`CLAUDE.md`, "Names are written out"): `declaration_text`, `holes`, `text_index`, `superseded`, `code_id`, never `decl_text`, `idx`, `txt`, `cid`.
- **Nothing under `crates/ns-syntax` changes.**
- **An image is untrusted input** (`CLAUDE.md`). Every new count, index, offset and id the decoder reads -- the text table's count, a text's holes, a row's text index, a frame's code id -- is validated before it indexes or allocates; counts go through `bounded_count`; a crafted value is a `DecodeError`, never a panic. Slicing and assembling a text answer `None` for any span or hole that does not fit, never panic. The loader still never parses: a text is parsed only by an edit, after load.
- **A wire tag is assigned once and never renumbered or reused** (`CLAUDE.md`). `OBJECT_FRAME` takes 15 and 10 joins `RETIRED_OBJECT_BODIES`; `FLAG_HAS_SOURCE` takes 8 and 1 joins `RETIRED_HEADER_FLAGS`; `tests/opcode_tags.rs` pins both in the same commit.
- **`crates/ns-runtime/tests/fixtures/golden-image.bin` does not change in Tasks 1-4**: the fixture is source-free and holds no frame, and a source-free image is written exactly as today. If `the_wire_format_matches_the_golden_fixture` fails there, stop and report. Task 5 edits a comment of `MirrorsFactory.ns`, which changes the shipped world's fingerprint, so it regenerates the fixture: `cargo nextest run -p ns-runtime --test image --run-ignored only regenerate_the_golden_fixture`, then `cmp -l` against the previous fixture (`git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $TEMP/old-fixture.bin`) shows that only the 8 fingerprint bytes after the flag byte differ.
- **Program outcomes are pinned, not re-derived** (`CLAUDE.md`). No golden's `status`, `stdout` or `stderr` changes in this plan -- a mirror's `source` answers the same text as before for every golden; Task 6 deletes only the `skipped_in_images` lines. If an outcome would change, stop and report.
- **The VM mirror is a capability** (`CLAUDE.md`, the six locks). Nothing here adds a leaf or a way to reach the mirror; an edit still resolves with `resolve`, never `resolve_kernel`, and still refuses a shipped mixin.
- **A handle means nothing across a move.** The encoder runs between sends and allocates nothing; the graph table it builds first holds handles only until the graph is written, within one entry.
- **Never judge performance from a debug build.** Task 1 changes what every boot builds for the shipped modules; it must copy no shipped text (the shared form). If you want to confirm boot time did not move, ask the controller rather than timing a debug build.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult`; tests use the local `expect_ok` helper of their file. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- After editing a `.ns` file run `cargo run -p ns-analyzer -- fmt <the files>`. Every member of `MirrorsFactory.ns` keeps its Javadoc-standard `///` comment.
- Comments explain the decision, not the mechanics. Documentation describes the current state: a comment that says what "road step 5b2b" will do is rewritten to say what is, or names road step 5b2c where it is about reclamation.

## Review Focus

1. **An enclosing class's method source after a nested edit, live and through an image.** `Inner` edited, so that its text grew: `Outer`'s method `source` answers its own text, `Inner`'s answers the edited one, and the module's text holds both -- in the live heap and after the program was written with source and loaded. Pinned by Task 1's `an_enclosing_declaration_keeps_its_text_across_a_nested_edit` and Task 3's `an_enclosing_method_keeps_its_source_through_an_image`.
2. **No version keeps another's text.** After `Inner` was edited, no text `Outer` or any other mixin of the module holds contains `Inner`'s old members, and an image of the edited program does not contain them either. Pinned by Task 1's `an_edit_leaves_no_old_text_in_the_enclosing_declaration` and Task 3's `an_image_of_an_edited_program_holds_no_old_text`.
3. **A module assembles to its file.** A module with a prologue, comments between members and around nested classes, a class side, non-ASCII text and nested classes three deep assembles to its text byte for byte, and so does every shipped module. Pinned by Task 1's `an_unedited_module_assembles_to_its_text` and `every_shipped_module_assembles_to_its_text`.
4. **Crafted bytes in the new fields.** A text whose holes overlap, run past the text or are out of order; a row's text index past the table; a row whose text has another number of holes than the row has nested declarations; a frame's code id past the code table; a method frame whose code is a block; a closure frame whose code is not its closure's: each a `DecodeError`, never a panic. Pinned by Task 2's `a_crafted_declaration_text_is_refused` and Task 4's `a_frame_whose_code_disagrees_with_its_kind_is_refused`.
5. **A loaded program whose text is not its code.** An image whose text was swapped for a valid module that lacks the edited declaration, and one written without source: the edit answers a diagnostic (`<name>'s source text does not hold its declaration`, `<name> has no source text to edit`) and changes nothing; never a panic. Pinned by Task 5's `a_loaded_program_whose_text_lacks_the_declaration_is_refused` and `a_loaded_program_without_source_is_refused`.

---

### Task 1: Every declaration holds its own text

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`DeclarationText`; `Mixin::module_text` and `ModuleText` replaced by `Mixin::text`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`build_mixin`, `build_root_mixin`, `mixin_of`, `build_class_side`: cut or share each declaration's text)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs`, `crates/ns-runtime/src/shipped_code/{mod,decode,encode}.rs` (shipped mixins share their module's text)
- Modify: `crates/ns-runtime/src/heap.rs`, `crates/ns-runtime/src/heap/tables.rs` (`Tables::module_texts` and `current_module_texts` go; `replace_mixin` loses its text parameter; `Heap::assembled_text`, `declaration_source`, `module_source`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::replace_mixin`), `crates/ns-runtime/src/interpreter/edit.rs` (the original text is the assembled module; `current_declaration_source` goes), `crates/ns-runtime/src/interpreter/install.rs` (no current text to set)
- Modify: `crates/ns-runtime/src/builtins/reflection.rs` (`method_source`, `MixinSource`)
- Modify: `crates/ns-runtime/src/image/mod.rs`, `crates/ns-runtime/src/image/modules.rs`, `crates/ns-runtime/src/image/mixins.rs` (interim: the encoder writes the assembled module text, the decoder cuts each row's text from it; `ModuleSource::with_source`)
- Modify: every `image::ModuleSource { .. }` construction: `crates/ns-runtime/src/interpreter/mirror.rs:647`, `crates/ns-runtime/src/interpreter/install.rs` (test), `crates/ns-runtime/tests/image.rs` (`snapshot_one`, `module_only_image` and their callers), `crates/ns-runtime/tests/image_truncations.rs`, `crates/ns-runtime/tests/trampoline.rs:417`, `crates/ns-analyzer/tests/common/mod.rs:133`, `crates/ns-analyzer/tests/disasm_snapshots.rs:284`, `crates/ns-cli/src/program.rs:256` (`module_sources` keeps its `with_source` parameter)
- Test: `crates/ns-runtime/src/mixin.rs` (`source_text_tests` becomes `declaration_text_tests`), `crates/ns-runtime/src/interpreter/install.rs` (tests), `crates/ns-runtime/src/interpreter/core_library.rs` or `shipped_code/mod.rs` (tests)

**Interfaces:**
- Produces, in `crate::mixin`:

```rust
/// The text of one declaration as it was read: from its start to its end
/// -- for a module's root declaration, the whole module -- with every
/// declaration nested in it cut out, as a hole. A span the compiler
/// recorded counts from the start of the module text the declaration was
/// read in, and still slices it.
pub struct DeclarationText { /* see Step 3 */ }

impl DeclarationText {
    /// `range` of `module_text` with each of `holes` cut out, copied.
    /// `holes` are the spans of the declarations nested directly in it, in
    /// source order. `None` when a hole is not within `range`, the holes
    /// overlap or are out of order, or a boundary is not a character
    /// boundary of `module_text`.
    pub fn cut(module_text: &str, range: Span, holes: &[Span]) -> Option<DeclarationText>;
    /// The same, sharing `module_text` instead of copying it: for a module
    /// whose declarations are never replaced.
    pub fn shared(module_text: Rc<str>, range: Span, holes: &[Span]) -> Option<DeclarationText>;
    /// The text `span` covers, which must lie within the declaration and in
    /// no hole; `None` otherwise.
    pub fn slice(&self, span: Span) -> Option<&str>;
    /// The declaration's whole text with `nested[i]` in hole `i`; `None`
    /// unless there is one text per hole.
    pub fn assemble(&self, nested: &[String]) -> Option<String>;
    /// Where the declaration began, and the spans of its holes, in the
    /// module text it was read in: what an image writes.
    pub fn range(&self) -> Span;
    pub fn holes(&self) -> &[Span];
    /// The text with every hole cut out: what an image writes.
    pub fn cut_text(&self) -> std::borrow::Cow<'_, str>;
}
```

- Produces, on `Mixin`: `pub text: Option<Rc<DeclarationText>>` (a class side, declared or synthesized from a declaration, shares its instance side's `Rc`; `None` where no text exists).
- Produces, on `Heap` (`pub(crate)`): `fn assembled_text(&self, mixin: &Rc<Mixin>) -> Option<String>` (the mixin's text with every hole filled by the assembled text of the *current* version of the nested declaration it stands for, found through `known_mixin_id` and `mixin`); `fn declaration_source(&self, id: MixinId) -> Option<String>` (the declaration's text from its access modifier or `class` to its closing parenthesis, as the module's current text holds it -- `assembled_text` sliced by the declaration's span, which for a root cuts the prologue and the trailing text); `fn module_source(&self, module_root: MixinId) -> Option<String>` (`assembled_text` of the root). `replace_mixin(&mut self, id, replacement)`.
- Produces, in `crate::image`: `pub struct ModuleSource<'a, 'gc> { pub class: ClassReference<'gc>, pub origin: &'a SourceOrigin, pub with_source: bool }`.

- [ ] **Step 1: Write the failing tests**

In `mixin.rs`, replace `source_text_tests` with `declaration_text_tests` (keep its two existing cases, rewritten against `DeclarationText::slice` on a text with no hole) and add:

```rust
    const MODULE: &str = "Newspeak3\n'Test'\nclass Outer = (\n  (* \u{e4} *) class Inner = (\n  ) ( public a = ( ^1 ) )\n  public b = ( ^2 )\n)\n";

    fn span_of(text: &str, needle: &str) -> Span {
        let start = text.find(needle).expect("the needle is in the text");
        Span::new(start as u32, (start + needle.len()) as u32)
    }

    /// The root's text is the whole module with its nested declaration cut
    /// out; filling the hole back gives the module byte for byte.
    #[test]
    fn a_cut_text_assembles_to_the_module() {
        let inner = span_of(MODULE, "class Inner = (\n  ) ( public a = ( ^1 ) )");
        let whole = Span::new(0, MODULE.len() as u32);
        let root = DeclarationText::cut(MODULE, whole, &[inner]).expect("cut");
        let nested = DeclarationText::cut(MODULE, inner, &[]).expect("cut");
        let nested_text = nested.assemble(&[]).expect("no holes");
        assert_eq!(root.assemble(&[nested_text]).as_deref(), Some(MODULE));
        assert!(!root.cut_text().contains("public a"));
    }

    /// A span after a hole still slices the text it was recorded against,
    /// and one inside a hole or across one slices nothing.
    #[test]
    fn a_span_slices_around_the_holes() {
        let inner = span_of(MODULE, "class Inner = (\n  ) ( public a = ( ^1 ) )");
        let root = DeclarationText::cut(MODULE, Span::new(0, MODULE.len() as u32), &[inner])
            .expect("cut");
        assert_eq!(root.slice(span_of(MODULE, "public b = ( ^2 )")), Some("public b = ( ^2 )"));
        assert_eq!(root.slice(span_of(MODULE, "(* \u{e4} *)")), Some("(* \u{e4} *)"));
        assert_eq!(root.slice(span_of(MODULE, "public a = ( ^1 )")), None);
        assert_eq!(root.slice(Span::new(inner.start - 1, inner.end + 1)), None);
    }

    /// Holes out of order, overlapping, outside the range or off a
    /// character boundary cut nothing.
    #[test]
    fn a_hole_that_does_not_fit_cuts_nothing() {
        let whole = Span::new(0, MODULE.len() as u32);
        let first = span_of(MODULE, "class Inner");
        let second = span_of(MODULE, "public b");
        assert!(DeclarationText::cut(MODULE, whole, &[second, first]).is_none());
        assert!(DeclarationText::cut(MODULE, whole, &[first, first]).is_none());
        assert!(DeclarationText::cut(MODULE, first, &[second]).is_none());
        let umlaut = MODULE.find('\u{e4}').expect("an umlaut") as u32;
        assert!(DeclarationText::cut(MODULE, whole, &[Span::new(umlaut + 1, umlaut + 2)]).is_none());
    }

    /// The shared form answers exactly what the cut form answers.
    #[test]
    fn the_shared_form_answers_as_the_cut_one() {
        let inner = span_of(MODULE, "class Inner = (\n  ) ( public a = ( ^1 ) )");
        let whole = Span::new(0, MODULE.len() as u32);
        let cut = DeclarationText::cut(MODULE, whole, &[inner]).expect("cut");
        let shared = DeclarationText::shared(Rc::from(MODULE), whole, &[inner]).expect("shared");
        let probe = span_of(MODULE, "public b = ( ^2 )");
        assert_eq!(cut.slice(probe), shared.slice(probe));
        assert_eq!(cut.cut_text(), shared.cut_text());
        assert_eq!(cut.assemble(&["X".into()]), shared.assemble(&["X".into()]));
    }
```

In `install.rs`'s tests:

```rust
    /// `Outer`'s and `Inner`'s method source, sliced from each version's
    /// own text.
    fn method_source_of(mutator: &Mutator<'_>, id: MixinId, selector: &str) -> Option<String> {
        let mixin = mutator.heap.mixin(id)?;
        let method = mixin.compiled_method(Symbol::intern(selector))?;
        mixin.text.as_ref()?.slice(method.code.source).map(str::to_string)
    }

    const NESTED_AND_AFTER: &str = indoc! {"
        Newspeak3
        'Test'
        class Outer = Object new () (
          (* Inner's comment *)
          public class Inner = (| public x ::= 1. |) (
            public a = ( ^1 )
          )
          public after = ( ^2 )
        )
    "};

    /// An edit that grows `Inner` leaves `Outer`'s text as it was read, so
    /// `Outer`'s method still slices; the module's text holds the edit.
    #[test]
    fn an_enclosing_declaration_keeps_its_text_across_a_nested_edit() {
        let (mut interpreter, outer) = load(NESTED_AND_AFTER);
        let _instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        install(
            &mut interpreter,
            inner_id,
            &add_method("public added = ( ^'a method that moves the text after it' )"),
        );
        interpreter.enter(|mutator| {
            let outer_id = mutator
                .heap
                .known_mixin_id(&mixin_of(mutator, &outer))
                .expect("Outer is applied");
            assert_eq!(
                method_source_of(mutator, outer_id, "after").as_deref(),
                Some("public after = ( ^2 )")
            );
            assert_eq!(
                method_source_of(mutator, inner_id, "added").as_deref(),
                Some("public added = ( ^'a method that moves the text after it' )")
            );
            let module = mutator.heap.module_source(outer_id).expect("the module has text");
            assert!(module.starts_with("Newspeak3\n'Test'\n"));
            assert!(module.contains("public added") && module.contains("(* Inner's comment *)"));
            assert_eq!(
                mutator.heap.declaration_source(outer_id),
                module.find("class Outer").map(|start| module[start..].trim_end().to_string())
            );
        });
    }

    /// After the edit, no text the module's mixins hold contains `Inner`'s
    /// old members: the old version is held by the table of superseded
    /// versions alone, never by its enclosing declaration's text.
    #[test]
    fn an_edit_leaves_no_old_text_in_the_enclosing_declaration() {
        let (mut interpreter, outer) = load(NESTED_AND_AFTER);
        let _instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        install(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod(Symbol::intern("a"))]),
        );
        interpreter.enter(|mutator| {
            let outer_mixin = mixin_of(mutator, &outer);
            let outer_text = outer_mixin.text.as_ref().expect("Outer has text");
            assert!(!outer_text.cut_text().contains("public a = ( ^1 )"));
            let inner = mutator.heap.mixin(inner_id).expect("Inner is registered");
            let inner_text = inner.text.as_ref().expect("Inner has text");
            assert!(!inner_text.cut_text().contains("public a = ( ^1 )"));
        });
    }

    /// An unedited module assembles to the text it was read in, byte for
    /// byte: prologue, comments around and between members, a class side,
    /// non-ASCII text and declarations three deep.
    #[test]
    fn an_unedited_module_assembles_to_its_text() {
        let source = indoc! {"
            Newspeak3
            'Test'
            (* the module's comment *)
            class Outer = Object new () (
              (* \u{e4}\u{1F600} *)
              public class Middle = () (
                public class Innermost = () ( public c = ( ^'\u{e9}' ) )
                public b = ( ^2 )
              ) : ( public made = ( ^self new ) )
              public a = ( ^1 )
            ) : (
              public marker = ( ^7 )
            )
            (* trailing *)
        "};
        let (mut interpreter, outer) = load(source);
        let _instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        interpreter.enter(|mutator| {
            let root = mixin_of(mutator, &outer);
            assert_eq!(mutator.heap.assembled_text(&root).as_deref(), Some(source));
        });
    }
```

In `core_library.rs`'s tests (or `shipped_code/mod.rs`'s, wherever a test already boots the shipped world and reads its mixins):

```rust
    /// Every shipped module's root mixin assembles to the module's text,
    /// sharing it rather than copying it.
    #[test]
    fn every_shipped_module_assembles_to_its_text() { /* boot an
       interpreter, build every shipped module (`build_shipped_names` with
       every name, or the helper an existing test uses to reach each
       module's root mixin), and for each assert
       `heap.assembled_text(&root) == Some(shipped_module_text(name).to_string())`
       and that `root.text` is the shared form (`DeclarationText::is_shared`,
       a `#[cfg(test)]` accessor you add) */ }
```

Write that last test out with the helpers the file already has; find them with `git grep -n "shipped_module_text\|fn boot\|build_shipped" crates/ns-runtime/src`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --lib -E 'test(declaration_text) | test(keeps_its_text) | test(no_old_text) | test(assembles_to)'`
Expected: compile errors (`DeclarationText`, `Mixin::text`, `Heap::assembled_text`).

- [ ] **Step 3: Implement**

`DeclarationText`:

```rust
pub struct DeclarationText {
    /// Where the declaration's text is kept.
    storage: TextStorage,
    /// The declaration's extent in the module text it was read in.
    range: Span,
    /// The declarations nested directly in it, in source order, each by
    /// its span in that module text.
    holes: Vec<Span>,
}

enum TextStorage {
    /// The module's text itself, holes and all: a shipped module's, which
    /// every boot of the thread shares and no edit replaces.
    Shared(Rc<str>),
    /// `range` with the holes cut out, owned: a program's declaration,
    /// whose versions an edit replaces one by one, so that no version
    /// keeps another's text alive.
    Cut(Box<str>),
}
```

`slice(span)`: refuse a span outside `range`, inside or across a hole; for `Shared` slice the module text at the span (`str::get`, so a non-boundary answers `None`); for `Cut` subtract `range.start` and the lengths of the holes that end at or before `span.start`, then `get`. `assemble(nested)`: `None` unless `nested.len() == holes.len()`; concatenate the segments between the holes (for `Shared`, slices of the module text; for `Cut`, consecutive pieces of the owned text, whose boundaries are the holes' positions mapped as above) with `nested[i]` after segment `i`. `cut_text()`: `Cow::Borrowed` for `Cut`, the joined segments for `Shared`. `Debug` prints the range, the hole count and the length, not the text (as `ModuleText`'s did). Validate in `cut`/`shared` exactly what the doc comment says, so that every later `slice` and `assemble` stays within the storage.

`instantiate.rs`: `mixin_of` takes the module text as today and a flag for which form to make -- shared for `ModuleSource::Kernel` and `ModuleSource::CoreLibrary`, cut for `ModuleSource::User` -- and the range: the whole module text (`0..len`) when `declaration` is the module's root (`std::ptr::eq(declaration, root)`, and `build_root_mixin` is the root's only builder), its `span` otherwise; the holes are `declaration.instance_side.nested_classes()`' spans. A class side gets its instance side's `Rc<DeclarationText>`, so build the text before the class side and pass it down. `None` only if `cut` refuses, which a parsed module never makes it do (`debug_assert!` that).

`core_library.rs` and `shipped_code`: a shipped mixin decoded from the embedded artifact gets `DeclarationText::shared` over `shipped_module_text(name)`, from its span and its nested mixins' spans, built where `decode.rs:147` sets `module_text` today; nothing copies a shipped text.

`heap.rs`/`tables.rs`: delete `Tables::module_texts`, `Tables::current_module_texts`, `Heap::mixin_module_text`, `current_module_text`, `set_current_module_text`; `register_mixin` stops copying a text; `replace_mixin` loses its text parameter (the fresh mixin holds its own). Add `assembled_text`, `declaration_source`, `module_source` as specified; `assembled_text` recurses along the declarations' nesting, which the parser (and an image's `MAX_MIXIN_DEPTH`) bounds.

`edit.rs`: `prepare_edit`'s original text is `self.heap.module_source(module_root)` (the diagnostic for `None` stays `<name> has no source text to edit`); `PreparedEdit::module_text` stays the new module's whole text (an `Rc<str>`), which `build_mixin` cuts the fresh subtree's texts from; `current_declaration_source` goes -- its callers use `Heap::declaration_source`. `install.rs`'s `replace_versions` no longer sets a current text.

`reflection.rs`: `method_source` slices `mixin.text` (a class side's is its instance side's, so the instance-side fallback goes); `MixinSource` answers `heap.declaration_source(instance)`.

`image` (interim, until Task 2 replaces it): the encoder takes each module's text from `heap.module_source(id of the root)` -- never from the host -- and `ModuleSource::source` becomes `with_source: bool` (every construction: `None` → `false`, `Some(..)` → `true`; before replacing a `Some(text)` in `tests/image.rs`, check that `text` is the very text the module was compiled from, and stop and report if a test passes another one on purpose). `mixins::build` cuts each row's text from its module's text: the module's root row over the whole text, every other row over its span, the holes its nested rows' spans; a cut that fails (a crafted span) leaves the row without text, which its `source` answers as `nil`. A class-side row shares its instance side's text.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime` then `cargo nextest run -p ns-analyzer -p ns-cli`
Expected: PASS, the golden fixture unchanged, every golden's outcome unchanged.

- [ ] **Step 5: Commit**

Run: `cargo xtask gates commit images analyzer`
```bash
git add -A crates
git commit -m "feat: hold each declaration's own text, its nested declarations cut out"
```

---

### Task 2: The image carries the declaration texts

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs` (`encode_describing`, `write_describing`, `load`, `decode_described`; the text table between the code table and the mixin table)
- Create: `crates/ns-runtime/src/image/texts.rs` (`encode`, `decode`: the text table)
- Modify: `crates/ns-runtime/src/image/mixins.rs` (a row names its text; `read` takes `has_source`; `build` takes the decoded texts and refuses a mismatch)
- Modify: `crates/ns-runtime/src/image/modules.rs` (the module record carries no text; `DecodedModule::source` goes)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`FLAG_HAS_SOURCE`, `KNOWN_FLAGS`, `RETIRED_HEADER_FLAGS`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (new `DecodeError` variants)
- Modify: `crates/ns-runtime/src/image/inspect.rs`
- Test: `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`

**Interfaces:**
- Consumes: Task 1's `DeclarationText` (`cut`, `range`, `holes`, `cut_text`), `Mixin::text`.
- Produces: `encode::FLAG_HAS_SOURCE == 8`; `LoadedModule::source: Option<Rc<str>>` answers the assembled module text (`Rc<str>` of `Heap::module_source`, computed once at load); `DecodeError::TextIndexOutOfRange { row: u32, index: u32, texts: usize }`, `DecodeError::MalformedText { text: u32 }`, `DecodeError::TextHolesMismatch { row: u32, holes: usize, nested: usize }`.

**The wire, with source** (`FLAG_HAS_SOURCE` set). After the code table, a text table: a `u32` count, then per text a `u32` range start, a `u32` range end, a `u32` hole count and that many `(u32 start, u32 end)` holes, then the text with the holes cut out (`write_str`). A mixin row ends with one more field, after its superclass message code id: a `u32` index into the text table. The module record carries no text. Without source neither the table nor the index is written, so a source-free image is byte for byte what it is today.

**Encoding.** The table holds every distinct `Rc<DeclarationText>` (by `Rc::ptr_eq`) the rows' mixins hold, in row order; a class side's row names its instance side's. A module asked for source one of whose rows has no text is refused: ``image encoding cannot carry the source of module `<root name>`, which was built without it``. The "every module or none" refusal stays, worded as today.

**Decoding.** Each text is read with its counts bounded (`bounded_count(count, 8, reader)` for the holes) and rebuilt through `DeclarationText` from its parts -- add `DeclarationText::from_parts(range: Span, holes: Vec<Span>, cut_text: Box<str>) -> Option<DeclarationText>`, which checks that the holes are in order, disjoint and within `range` and that `cut_text`'s length is `range` minus the holes' lengths, and is what `MalformedText` reports (`text {text} of the image does not describe a declaration`). `mixins::build` gives each row the text its index names (`TextIndexOutOfRange`: `mixin-table row {row} names text {index}, and the image carries {texts}`) and refuses a row whose text has another number of holes than the row has nested rows (`TextHolesMismatch`: `mixin-table row {row}'s text has {holes} nested declarations, and the row {nested}`). `load` answers each module's `source` as the root's assembled text.

- [ ] **Step 1: Write the failing tests**

In `tests/image.rs`, beside `the_wire_format_matches_the_golden_fixture`:

```rust
/// A text the decoder cannot hold to its declaration is refused before any
/// mixin holds it: holes out of order, a hole past the range, a length that
/// does not add up, a text index past the table, another number of holes
/// than nested rows.
#[test]
fn a_crafted_declaration_text_is_refused() { /* snapshot NESTED_MODULE
   (it has a nested class) with source; locate the text table with
   `inspect` (extend `inspect`'s description of a text with its byte
   offsets, `inspect::Text { offset, holes_offset, ... }`, if it records
   none) and craft, one image each: the root text's first hole's end set
   past its range end; its two hole fields swapped so start > end; the
   cut text's length prefix one larger; the root row's text index set to
   the table's length; the root row's text index set to the nested row's
   text (no holes, one nested row). Assert each `image::load` is an `Err`
   whose message names the case: "does not describe a declaration" for the
   first three, "names text" and "nested declarations" for the last two */ }

/// Written with source and loaded, a module answers its text whole, and
/// each mixin slices its methods from its own text.
#[test]
fn a_loaded_module_answers_its_text_and_every_method_source() { /* load
   NESTED_MODULE, snapshot with source, `image::load`; assert
   `modules[0].source` equals NESTED_MODULE, and for the root and the
   nested mixin of `modules[0].declaration` that `text.slice(method
   .code.source)` answers each method's source text as written */ }
```

Write both out in full with the file's helpers (`snapshot_one`, `class_reference`, `image::inspect`); each crafted case is its own `assert!` with the case named in its message.

In `opcode_tags.rs`'s `header_flags_are_fixed`:

```rust
    assert_eq!(encode::FLAG_HAS_SOURCE, 8);
    assert_eq!(encode::FLAG_START, 4);
    // Bit 2 was `FLAG_STATELESS`, bit 1 the source variant that carried one
    // text per module; both are retired and never reused.
    assert_eq!(encode::RETIRED_HEADER_FLAGS, 2 | 1);
```

`a_header_flag_this_build_does_not_assign_is_rejected` in `image.rs` sets bit 3 (8), which is now assigned: move it to bit 4 (16), and add a sibling test that bit 0 (1), now retired, is refused the same way.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --test image --test opcode_tags -E 'test(declaration_text) | test(answers_its_text) | test(header_flag)'`
Expected: FAIL.

- [ ] **Step 3: Implement** as described. `encode.rs`'s flags:

```rust
/// The image carries its program's source: a table of every declaration's
/// own text, and per mixin row the index of its text
/// (`docs/design/image-format.md`, "Source").
pub const FLAG_HAS_SOURCE: u8 = 8;
pub const FLAG_START: u8 = 4;
pub const KNOWN_FLAGS: u8 = FLAG_HAS_SOURCE | FLAG_START;
/// Flags that once meant something and are never reused: 2 was
/// `FLAG_STATELESS`, the stateless binary, retired 2026-09-29; 1 was the
/// source variant that carried one text per module, retired 2026-10-19
/// when every declaration came to hold its own, so that an image of the
/// old layout is refused rather than misread.
pub const RETIRED_HEADER_FLAGS: u8 = 2 | 1;
```

`inspect` lists the text table (`text 0: 120..480, 1 hole, 300 bytes`), shows a row's text index, and no longer a module's source flag per module (the header's flag says it for all).

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime --test image --test opcode_tags --test image_truncations` then `cargo nextest run -p ns-analyzer -p ns-cli`
Expected: PASS, the fixture unchanged. An `inspect` snapshot of a with-source image under `crates/ns-analyzer/tests/` changes by the text table: check the diff shows only that, then accept it.

- [ ] **Step 5: Commit**

Run: `cargo xtask gates commit images analyzer`
```bash
git add -A crates
git commit -m "feat: carry each declaration's own text in an image's text table"
```

---

### Task 3: An edited program is written to an image

**Files:**
- Modify: `crates/ns-runtime/src/image/mixins.rs` (`plan` walks the current versions; `MixinIds` keyed by `MixinId`)
- Modify: `crates/ns-runtime/src/image/mod.rs` (`encode_describing`: the refusal goes; the rows are planned once)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`encode_code_table` takes the rows; `encode_class_record`, `encode_mixin_record`, `EncodeContext::mixin_ids`)
- Modify: `crates/ns-runtime/src/image/modules.rs` (`root_record_defect` compares with `Heap::same_mixin`)
- Modify: `crates/ns-runtime/src/heap.rs`, `crates/ns-runtime/src/heap/tables.rs` (`Tables::edited` and `Heap::mixin_is_edited` go if nothing but tests read them)
- Test: `crates/ns-runtime/src/interpreter/install.rs` (tests)

**Interfaces:**
- Consumes: Tasks 1-2.
- Produces: `pub(crate) type MixinIds = HashMap<MixinId, u32>`; `mixins::plan(heap: &Heap<'_>, module_roots: &[Rc<Mixin>]) -> Result<Vec<Row>, String>`; `encode::encode_code_table(writer, rows: &[mixins::Row], extra: &[Rc<CompiledCode>], symbols)` (Task 4 fills `extra`; pass `&[]` here).

**What changes.** A class object an install re-applied names an application of its mixin's current version, but a mixin's `nested` and `class_side` still hold the versions it was built with, a level's `AppliedClause::declaration` the version that applied it, and a module root's clause level the root's old version. So the encoder never keys a mixin by its address: `plan` takes every mixin it reaches as `heap.mixin(id)` when `heap.known_mixin_id(mixin)` answers an id (a mixin no class object applied has none and is taken as it is), and every lookup of a row -- a class record's mixin, an applied clause's declaration, a reified mixin's instance side -- goes through `heap.known_mixin_id`. `root_record_defect` compares with `heap.same_mixin(candidate, root_mixin, heap.known_mixin_id(root_mixin))` rather than `Rc::ptr_eq`, which the decoder's freshly built mixins still satisfy by reference. The rows' texts are the current versions' own, so an edited module's image holds each declaration's text once.

- [ ] **Step 1: Write the failing tests**

Replace `an_edited_program_is_refused_with_an_error` in `install.rs`'s tests by a section "Images of an edited program":

```rust
    /// `class`'s module written with its source, `roots` the graph's roots.
    fn image_of(interpreter: &mut Interpreter, class: &Root, roots: &[Root]) -> Vec<u8> {
        image_with(interpreter, class, roots, true)
    }

    fn image_with(
        interpreter: &mut Interpreter,
        class: &Root,
        roots: &[Root],
        with_source: bool,
    ) -> Vec<u8> {
        interpreter.enter(|mutator| {
            let class = mutator.fetch(class).as_object().expect("a class object");
            let origin = SourceOrigin::File(PathBuf::from("t.ns"));
            let modules = [crate::image::ModuleSource {
                class,
                origin: &origin,
                with_source,
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

    /// `source` loaded and instantiated: the module class, its `Inner`
    /// class object, an `Inner` made before any edit, and `Inner`'s id.
    fn module_and_inner(source: &str) -> (Interpreter, Root, Root, Root, MixinId) {
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

    fn contains(bytes: &[u8], needle: &str) -> bool {
        bytes.windows(needle.len()).any(|window| window == needle.as_bytes())
    }

    /// An edited nested class is written and loads edited: its added
    /// method answers on an instance made before the edit.
    #[test]
    fn an_edited_program_loads_edited() {
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
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
        let (mut interpreter, outer, inner_class, old, inner_id) = module_and_inner(OUTER);
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

    /// After a growing edit of `Inner`, the loaded `Outer` still slices
    /// its method from its own text, and the loaded module's text holds
    /// the edit.
    #[test]
    fn an_enclosing_method_keeps_its_source_through_an_image() {
        let (mut interpreter, outer, inner_class, _, inner_id) = module_and_inner(NESTED_AND_AFTER);
        install(
            &mut interpreter,
            inner_id,
            &add_method("public added = ( ^'a method that moves the text after it' )"),
        );
        let bytes = image_of(&mut interpreter, &outer, &[inner_class]);
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        image.interpreter.enter(|mutator| {
            let outer_id = mutator
                .heap
                .known_mixin_id(&mixin_of(mutator, &outer))
                .expect("Outer is applied");
            assert_eq!(
                method_source_of(mutator, outer_id, "after").as_deref(),
                Some("public after = ( ^2 )")
            );
            assert_eq!(
                method_source_of(mutator, inner_id, "added").as_deref(),
                Some("public added = ( ^'a method that moves the text after it' )")
            );
        });
        let module = image.modules[0].source.clone().expect("the image carries source");
        assert!(module.contains("public added") && module.contains("public after = ( ^2 )"));
    }

    /// An image of an edited program holds each declaration's text once:
    /// the removed method's text is nowhere in it.
    #[test]
    fn an_image_of_an_edited_program_holds_no_old_text() {
        let (mut interpreter, outer, inner_class, _, inner_id) = module_and_inner(NESTED_AND_AFTER);
        install(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod(Symbol::intern("a"))]),
        );
        let bytes = image_of(&mut interpreter, &outer, &[inner_class]);
        assert!(!contains(&bytes, "public a = ( ^1 )"));
        assert!(contains(&bytes, "public after = ( ^2 )"));
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

`ROOT_WITH_A_VALUE_CLAUSE` is a module whose root class applies `Value` through its one `<:` clause and declares `public a = ( ^1 )`: take the clause's exact spelling from an existing golden or test that builds such a root (`git grep -n "<: Value" goldens crates/ns-runtime/tests`), as an `indoc!` constant beside `OUTER`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime --lib -E 'test(edited_program) | test(header_edit_loads) | test(through_an_image) | test(no_old_text) | test(edited_root)'`
Expected: FAIL with `image encoding cannot yet serialize mixin ...`.

- [ ] **Step 3: Implement**

`mixins::plan(heap, module_roots)` walks in `mixin::preorder`'s order over the current versions:

```rust
/// `mixin`'s current version: an install replaced it under its id when
/// one names it, and a mixin no class object applied has none.
fn current<'gc>(heap: &Heap<'gc>, mixin: &Rc<Mixin>) -> Rc<Mixin> {
    heap.known_mixin_id(mixin)
        .and_then(|id| heap.mixin(id))
        .unwrap_or_else(|| Rc::clone(mixin))
}
```

A row's nested and class-side ids follow the current versions; row ids are keyed by `heap.known_mixin_id`, and a mixin without one needs no key, since no record can name it. `encode_describing` plans once and hands the rows to `encode_code_table`, `mixins::encode` and the text table. The three lookups in `encode.rs` (`:1546`, `:1594`, `:1843`) become `context.heap.known_mixin_id(mixin).and_then(|id| context.mixin_ids.get(&id))`, their error texts unchanged. `encode_describing` loses the refusal; if `Heap::mixin_is_edited` and `Tables::edited` then have no reader but tests, delete them and replace each assert on them (`install.rs:835-836`, `:1050`) by one on what the test means (`heap.mixin(id)` is the fresh version, or is unchanged).

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

### Task 4: The code an older version runs, and frames that name their body

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs` (`write_image`: walk the graph first)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`walk_graph` split out of `encode_graph_section`; the extra bodies; `encode_frame`)
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OBJECT_FRAME = 15`, `RETIRED_OBJECT_BODIES` gains 10, the record's doc, the next free number)
- Modify: `crates/ns-runtime/src/image/decode.rs` (the frame record's two passes, `method_declaration`, `verify_graph`'s kind check, new `DecodeError` variants)
- Modify: `crates/ns-runtime/src/heap.rs` (`fill_frame_fields` takes the body; `Heap::superseded_mixins`)
- Modify: `crates/ns-runtime/src/image/inspect.rs` (the frame record)
- Test: `crates/ns-runtime/src/interpreter/install.rs` (tests), `crates/ns-runtime/tests/image.rs`, `crates/ns-runtime/tests/opcode_tags.rs`

**Interfaces:**
- Consumes: Task 3's `mixins::plan(heap, ..)`, `encode_code_table(writer, rows, extra, symbols)`.
- Produces: `encode::walk_graph<'gc>(mutator: &Mutator<'gc>, roots: &[Value<'gc>], module_roots: &[ClassReference<'gc>]) -> Result<WalkedGraph<'gc>, String>` (the graph table and the shipped indexes, with every refusal `encode_graph_section` makes before writing); `WalkedGraph::bodies(&self, heap: &Heap<'gc>) -> Vec<Rc<CompiledCode>>` (each closure's code and each frame's body, in object order); `encode_graph_section(mutator, writer, walked: &WalkedGraph<'gc>, roots, code_ids, mixin_ids, symbols)`; `Heap::fill_frame_fields(&mut self, frame, record: FrameRecord<'gc>, code: Option<Rc<CompiledCode>>)`; `pub(crate) fn Heap::superseded_mixins(&self) -> &[Rc<Mixin>]`; `opcodes::OBJECT_FRAME == 15`; `DecodeError::FrameCodeMismatch` (`a frame's body is not one its kind runs`).

**The order of `write_image`.** Walk the graph (`walk_graph`), which refuses the mirror, an unnamed shipped class and a shipped module root as today; plan the rows; collect the extra bodies; write the code table, the text table and the mixin table; write the graph with the walked table; then the names and the header as today.

**Which extra bodies are admitted.** Every version of every mixin the rows describe: each row's mixin, and each superseded version whose `heap.known_mixin_id` is the id of a row's mixin (`Heap::superseded_mixins`, filtered). The admitted bodies are those versions' method codes, initializer, superclass clause and message, and `<:` message codes, with their nested blocks (`collect_code`'s walk over `MakeClosure`). A body of `WalkedGraph::bodies` that is a row's is already in the table; one that is admitted joins it after the rows' bodies, in graph order, its blocks before it; any other is refused with the existing ``image encoding cannot serialize a closure over code that no module the image carries compiled`` (for a frame's body, ``image encoding cannot serialize a frame running code that no module the image carries compiled``).

**The frame record (15).** `OBJECT_FRAME`'s body as today, except that the kind's payload is followed by the body -- a presence byte and a `u32` code id, absent exactly for a top-level frame -- and a method frame's payload is its selector and its method's access tag (`mixins::access_tag`). Decoding: the first pass sizes the record over the new fields; the second reads the code id against the code table (`BadCodeId`), and for a method frame takes the current declaration's method of that selector (`method_declaration`) when its `code` is this body (`Rc::ptr_eq`), and otherwise builds one:

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

`fill_frame_fields` records the body it is handed instead of the one `kind_code` would find in the class's current mixin, which after an edit is another (`heap.rs:8262`'s test caller passes `None` and keeps today's behaviour). `verify_graph` checks, before the program-counter and register checks, that the body is one the frame's kind runs: a method frame a `CodeKind::Method` body whose argument count is its method's; a closure frame its closure's own code (`Rc::ptr_eq`); an initializer frame `CodeKind::Initializer`; a superclass clause `CodeKind::SuperclassClause`; a superclass message and a `<:` clause's message `CodeKind::SuperclassMessage` (confirm in `compiler/` the kind a `<:` message body is compiled with, and use that); otherwise `FrameCodeMismatch`.

- [ ] **Step 1: Write the failing tests**

In `install.rs`'s "Images of an edited program":

```rust
    /// A closure made by a method an edit removed keeps its block across
    /// an image: the block and its home frame's body, the removed method's,
    /// enter the code table.
    #[test]
    fn a_closure_of_a_removed_method_runs_after_a_load() {
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
        let block = send(&mut interpreter, &inner, "block");
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
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
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

In `tests/image.rs`, beside the existing crafted-frame tests -- search for `CapturedFrameMismatch` and `ProgramCounterOutOfRange`, and locate the frame record as they do (extend `inspect`'s frame description with the offset of its code id, `inspect::Frame::code_offset: Option<usize>`, if nothing existing reaches it) -- two tests over the image those tests build from a method that answers a block (a closure over a method-local block, whose captured frame is a method frame):

- `a_frame_whose_code_disagrees_with_its_kind_is_refused`: the captured method frame's code id rewritten to the block's own code id (the closure record's, read from the same image); `image::load` answers an `Err` whose message contains `a frame's body is not one its kind runs`.
- `a_frame_code_id_past_the_table_is_refused`: the same code id rewritten to the code table's length; `image::load` answers an `Err` naming that id, as `BadCodeId`'s message does.

Each with a doc comment saying what the crafted bytes would otherwise do: decode a frame against a body it never ran. In `opcode_tags.rs`, pin `OBJECT_FRAME == 15`, that 10 is in `RETIRED_OBJECT_BODIES`, and the next free number, 16.

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

### Task 5: A program loaded from an image can be edited

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/edit.rs` (`prepare_edit`: the refusal of a mixin with no declaration goes)
- Modify: `crates/ns-runtime/core-library/platform/MirrorsFactory.ns` (`asBuilder`'s comment)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated: the fingerprint)
- Test: `crates/ns-runtime/src/interpreter/install.rs` (tests)

**Interfaces:**
- Consumes: Tasks 1-4: a loaded mixin holds its own text, and the module's text assembles from them.
- Produces: nothing new; `prepare_edit` answers for a decoded mixin what it answers for a compiled one.

A decoded mixin has no declaration (`Mixin::declaration` is `None`), but `prepare_edit` never reads it: it parses the module's text, assembled from the loaded declarations' texts, finds the declaration by its path of names, and pairs the fresh mixins with the old ones by name and id. The text is parsed at the first edit, after load, never during it. A crafted image whose text disagrees with its code is edited into what its text says, which is a valid program, or refused with the diagnostics that exist.

- [ ] **Step 1: Write the failing tests**

```rust
    /// A program loaded from an image with source is edited like one
    /// compiled from source, and its instances migrate.
    #[test]
    fn a_loaded_program_is_edited() {
        let (mut interpreter, outer, _, inner, _) = module_and_inner(OUTER);
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
        assert!(is_nil(&mut image.interpreter, &y));
    }

    /// Edited before an image and again after its load, written again and
    /// loaded again: the second image holds both edits.
    #[test]
    fn a_loaded_program_edited_and_written_again_loads_with_both_edits() {
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
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
        let (mut interpreter, outer, _, inner, _) = module_and_inner(OUTER);
        let bytes = image_with(&mut interpreter, &outer, &[inner], false);
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
        let (mut interpreter, outer, _, inner, _) = module_and_inner(OUTER);
        let mut bytes = image_of(&mut interpreter, &outer, &[inner]);
        // `Inner`'s own text, swapped for text of the same length that
        // declares a class of another name: the text table is
        // length-prefixed, so the swap keeps every other byte, and the
        // module assembles into a valid module without `Inner`.
        let inner_text = "class Inner = (| public x ::= 1. |)";
        let swapped = "class Other = (| public x ::= 1. |)";
        let start = bytes
            .windows(inner_text.len())
            .position(|window| window == inner_text.as_bytes())
            .expect("the image carries Inner's text");
        bytes[start..start + inner_text.len()].copy_from_slice(swapped.as_bytes());
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

Delete the refusal (`edit.rs:186-190`) and its mention in the file's docs. In `MirrorsFactory.ns`'s `asBuilder` comment, drop the clause `'<name> was loaded from an image, and a loaded program cannot be edited yet' for a class an image decoded;`, keep the other two, and say that a class an image decoded is edited from the text the image carries. Run `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/platform/MirrorsFactory.ns`. Regenerate the golden fixture as the Global Constraints say, and confirm only the fingerprint moved.

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

### Task 6: `build` writes the source, and every golden goes through images

**Files:**
- Modify: `crates/ns-cli/src/main.rs` (`Build::debug_source` → `strip_source`; `Load::print_source`'s help)
- Modify: `crates/ns-cli/src/commands/build.rs`, `crates/ns-cli/src/commands/load.rs:31`
- Modify: `crates/ns-cli/tests/build_load.rs:212-290` (the source test inverted)
- Modify: `crates/ns-cli/tests/common/mod.rs` (`Expectation::skipped_in_images`, its accessor and doc, `:88`'s field)
- Modify: `crates/ns-cli/tests/golden_round_trips.rs` (the skip on it; the doc's "An artifact carries no source")
- Modify: `crates/ns-cli/tests/run_goldens.rs:101-147` (the guard's second half)
- Modify: the 33 `goldens/library/mirror-builder/**/*.toml` (the `skipped_in_images` line goes)
- Modify: `README.md:165`

**Interfaces:**
- Consumes: Tasks 1-5.
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

`build::execute` takes `with_source: bool`, passed `!strip_source`. `load.rs`'s message names `--strip-source`. Delete the `skipped_in_images` line from every golden `.toml` (`git grep -l skipped_in_images goldens` lists them; the key appears nowhere else), then the field, its accessor and its doc from `common/mod.rs`, the `|| expectation.skipped_in_images().is_some()` from `golden_round_trips.rs` (its comment then says only what `skipped` means), and the `blank_in_images` half of `the_guards_on_skipped_refuse_what_they_promise_to` with its sentence in the doc comment. `deny_unknown_fields` now refuses a stale `skipped_in_images` key. In `README.md`, describe `--strip-source` where `--debug-source` was.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-cli` then `cargo nextest run -p ns-cli --profile images -E 'test(mirror-builder) | test(mirror_builder)'` (the trial names carry the golden's path; list them with `--list` if the filter finds none)
Expected: PASS; every mirror-builder golden runs through `build` then `load` under both lowerings with its recorded outcome.

- [ ] **Step 5: Commit**

Run: `cargo xtask gates commit images`
```bash
git add -A crates/ns-cli goldens README.md
git commit -m "feat: write an image's source unless --strip-source, and run every golden through images"
```

---

### Task 7: The documentation describes the built step

**Files:**
- Modify: `docs/design/image-format.md` ("Source" -- declaration texts and the text table; "Layout"; the frame record in "How frames actually get into an image" and "Verification"; "Planned: edited mixins in the image" becomes a section of what is; "Packaging"'s `build` synopsis; "Open questions" loses the decompiler; "Decisions" gains decisions 2-5 above)
- Modify: `docs/design/object-model.md` ("Source retention and provenance": a declaration's own text, the two forms, assembly)
- Modify: `docs/design/liveness-and-reflection.md` ("Planned: road step 5b2b ..." splits: what this plan built is described where it belongs -- "Compiling: the module's text is what an edit edits" now assembles the module from declaration texts, with no current-text table and no re-parse for a declaration's source -- and the rest becomes "Planned: road step 5b2c, reclaiming ids"; the diagnostics table loses the loaded-program row; "What pins it" loses `skipped_in_images`; "What the id has to carry" no longer lists the encoder's address keys or `Tables::current_module_texts`)
- Modify: `docs/design/memory.md`, `docs/design/README.md` (every "road step 5b2b" about the reclamation names road step 5b2c)
- Modify: `docs/OPEN-WORK.md` ("Next" is road step 5b2c with its four items; the decompiler item goes; a new item: a body's link to its declaration text, so that an image carries the text of code an older version runs, with the activation mirror that reads it)
- Modify: `docs/PROGRESS.md` (a row and a section for road step 5b2b, linking this plan)
- Modify: `CLAUDE.md` (the `ns-runtime` paragraph's mention of the module's current text, if it names `current_module_text`; "An image is untrusted input" if a sentence became false)
- Modify: the code comments that still say what road step 5b2b will do: `crates/ns-runtime/src/heap/tables.rs`, `crates/ns-runtime/src/interpreter/install.rs` (`refuse_an_unmigratable_instance`), and whatever `git grep -n "5b2b" crates` finds

- [ ] **Step 1:** Write the documents. Keep their voice: the current state, the reason beside the rule, each decision with its date (2026-10-19) and "decided with the user". `git grep -n "5b2b\|debug-source\|debug_source\|skipped_in_images\|cannot be edited yet\|decompiler\|module_text\|current_module_text\|ModuleText" -- docs crates README.md CLAUDE.md editors` lists what to look at; the plans under `docs/superpowers/plans/` stay as written.
- [ ] **Step 2:** Run: `git diff --stat`. A Rust comment change runs the `commit` gates (`cargo xtask gates commit`); a Markdown-only commit runs none.
- [ ] **Step 3: Commit**

```bash
git add -A docs crates README.md CLAUDE.md
git commit -m "docs: describe declaration texts, edited programs in images and road step 5b2c"
```
