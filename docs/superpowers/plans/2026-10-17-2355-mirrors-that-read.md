# Mirrors that read — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 5a: a program reads the structure of classes, mixins, methods and objects through `platform mirrors`, and nothing it reaches that way leads to a shipped module's root or the VM mirror.

**Architecture:** A seventh shipped module, `platform/MirrorsFactory.ns`, built on first demand over the VM mirror and the kernel, holds the mirror classes; its facet `platform mirrors` hands out `ObjectMirror` and `ClassMirror`. The mirrors read through a new leaf family, `ReflectionPrimitive`, which answers names, flags and arrays, and values only through levels whose mixin is not shipped. A mixin is reified as a canonical, kernel-private `Mixin` object (one SmallInteger field, its `MixinId`), kept in a strong table on the heap and written by a new image record; every `Mixin` gets its enclosing mixin (`Weak`) and its module's source text. `MirrorPrimitive` widens to `u16` first, since the table passes 256 rows.

**Tech Stack:** Rust 1.98.0, the Newspeak core library (`crates/ns-runtime/core-library/`), goldens under `goldens/`, `cargo-nextest`.

**Spec:** `docs/design/liveness-and-reflection.md`, "Planned: road step 5a, mirrors that read", and `docs/design/image-format.md`, "Planned: the reified mixin record" (designed with the user on 2026-10-17). Read both before Task 1. Their decisions are settled; this plan does not reopen them. Prior art for the protocol: `prior-art/primordialsoup/newspeak/Mirrors.ns` (`ClassMirror` `:898-1009`, `MixinMirror` `:1347-1430`, `MethodMirror` `:1244-1283`, `ObjectMirror` `:1575-1700`, `SlotDeclarationMirror` `:2225`), `prior-art/newspeak/MirrorsForSqueak.ns` (`ClassMirror` `:522-560`, the `computeMirrorGroup:` order), `prior-art/newspeak/MirrorTesting.ns` and `MirrorTestingModel.ns` (the behaviours the goldens pin); `prior-art/newspeak/MirrorGroups.ns:60-91`. Read them; copy no code (`CLAUDE.md`, "Copying code out of `prior-art/` is a licensing decision"), and name no other implementation in a shipped module's comments.

## Global Constraints

- Every commit is green: the per-commit gates in `CLAUDE.md` ("The gates", group `commit`, plus `images` where a task says so) run before each commit, through `cargo xtask gates commit` (`cargo xtask gates commit images` where named). The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 10, by the controller (`finishing-a-batch` skill); an implementer does not run them, and a task's commit is work in progress until they have run.
- Commits are conventional (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- Work on the branch `feat/read-only-mirrors` in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree. It exists already.
- Run every command in the foreground.
- Names are written out (`CLAUDE.md`, "Names are written out"): `reified_mixin`, `enclosing`, `module_text`, `selector`, never `rm`, `enc`, `src`, `sel`.
- **Program outcomes are pinned, not re-derived** (`CLAUDE.md`). An existing golden's `.toml` does not change in this plan; if one would, stop and report. A new golden's `.toml` is written from the spec before the code runs it, never copied from a run.
- After editing a `.ns` file: `cargo run -p ns-analyzer -- fmt <the files edited>`; never at the repository root.
- Every member of a shipped module, public, protected or private, has a `///` comment to the standard in `CLAUDE.md` ("Every member of a shipped core-library module is documented"). No other implementation is named. `crates/ns-runtime/tests/core_library_documentation.rs` enforces the presence.
- Any change to `Kernel.ns`, `PlatformFactory.ns`, a file under `core-library/platform/`, a `BuiltinClass` or the `MIRROR` table changes the shipped world's fingerprint, so `crates/ns-runtime/tests/fixtures/golden-image.bin` is regenerated in the same commit: `cargo nextest run -p ns-runtime --test image --run-ignored only regenerate_the_golden_fixture`, then confirm with `cmp -l` against the previous fixture (`git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $TEMP/old-fixture.bin`) that only the 8 fingerprint bytes after the flag byte differ.
- **The size tests run in release** after every task that touches `Primitive`, a `BuiltinClass` or `Heap`'s fields: `cargo nextest run -p ns-runtime --release -E 'test(stays_the_size) | test(one_word) | test(fits_in_the_niche)'`.
- **New `MIRROR` rows are appended at the end, never inserted.** After Task 1 the ceiling is 65,536.
- **Every store into an existing object goes through `heap.rs`'s store accessors** (`CLAUDE.md`). A reified mixin is allocated young by the mutator and filled by its initializing store; the decoder fills its placeholders as every record's.
- **A handle held on the Rust side is traced.** The heap's table of reified mixins holds `ObjectReference`s: it is traced wherever `activation_class` is (the scavenge's roots, `forward_everything`, `begin_marking`, `traced_size`) (`CLAUDE.md`, "A handle means nothing against a `Heap` that did not mint it").
- **The VM mirror is a capability** (`CLAUDE.md`, the six locks). No step here gives a program the mirror, its class, `Top`'s class object, a shipped root's instance, or a `MirrorSend` binding. `MirrorsFactory`'s root holds the mirror in `private vmMirror = mirror.`, immutable and eager, so `factory_holds_the_mirror` accepts it.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult`; tests use the local `expect_ok` helper of their file. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- Goldens are `main: platform args:` modules printing through `platform transcript`, formatted, commented with `///`; each new file follows its neighbours under `goldens/library/`. The new directory is `goldens/library/mirror/`. Every golden runs under both lowerings automatically.
- An image's spans are untrusted: every slice of a module's text goes through one function, `mixin::source_text(text, span) -> Option<&str>`, which answers `None` unless `span.start <= span.end <= text.len()` and both ends are character boundaries (`str::get`).

## Review Focus

1. **A path from a mirror to the VM mirror or a shipped root.** `ObjectMirror reflecting: platform transcript` then `getSlot:` of every name `getClass slots` lists, `slots`, `getClass enclosingObject`, `getClass reflectee`; the same over every class `platform collections`, `platform typedArrays`, `platform time` and `platform kernel` hand out, their superclass chains and their mixins' `enclosingMixin` chains: each answers structure or signals the `Error` of "The error texts", never a value that is the mirror, a root instance or a shipped class the program did not give. Pinned by Task 8's golden `a_mirror_hands_out_nothing_a_shipped_level_holds` and Task 5's leaf tests `a_withheld_value_is_declined_whatever_its_level`.
2. **A program class over a shipped one.** A subclass of `platform collections List` and a class applying a shipped class's mixin through `<:`: `ClassMirror slots` lists `List`'s slot declarations and the program's, `ObjectMirror slots` and `getSlot:` read only the program's, and `enclosingObject` of the `<:` level signals the `Error`. Pinned by Task 8's golden `a_program_class_over_a_shipped_one_reads_only_its_own_slots`.
3. **A class side that is synthesized** (a class declaring no class side): its mixin mirror answers `isMeta` true, the name `'Foo class'`, `instanceMixin` the instance side, the same reified object on every request, and survives a full collection and a move. Pinned by Task 5's `a_synthesized_class_side_is_one_reified_mixin` and Task 7's golden.
4. **A snapshot holding mirrors.** A program whose module slot holds a `MixinMirror` on its own class, a `ClassMirror` on `List` and a `MethodMirror`: after build and load, `==` of the reified mixin with a freshly reached one holds, and the mirrors answer what they did. A crafted image with two records for one mixin, a row past the table, a side byte of 2 or a header on another class is refused before it allocates, never a panic. Pinned by Task 4's image tests and Task 9's round trip.
5. **Source from an image without source, or with a span past the text.** `source` answers `nil`, never a panic and never a slice across a character boundary. Pinned by Task 2's `a_span_outside_the_text_answers_no_source` and Task 7's golden over an image with no source (the `images` profile runs every golden through `build` then `load`).

## The leaves, in one table

Family `Primitive::Reflection(ReflectionPrimitive)`, `builtins/reflection.rs`, appended in this order. "Declines" means the leaf answers `None` and the failure block runs; no leaf here raises.

| Row | Reported | Variant | Subject | Answers | Declines when |
|---|---|---|---|---|---|
| `reflectionMixinOf:ifFail:` | `mixin` | `MixinOf` | a class or metaclass | the reified mixin its application applies | the subject is no class object |
| `reflectionIsMetaclass:ifFail:` | `isMeta` | `IsMetaclass` | a class or metaclass | `true` for a metaclass | the subject is no class object |
| `reflectionEnclosingObjectOf:ifFail:` | `enclosingObject` | `EnclosingObjectOf` | a class or metaclass | its enclosing object (`nil` for a module's class) | no class object; its mixin is shipped; the value is withheld |
| `reflectionSlotNamesOf:ifFail:` | `slots` | `SlotNamesOf` | any object | an `Array` of Symbols: the named, non-lazy slots of non-shipped levels, superclass level first, declaration order; empty for anything with no named slots | never |
| `reflectionSlotOf:named:ifFail:` | `getSlot:` | `SlotNamed` | any object | the slot's value, the nearest level declaring the name winning | the name is no Symbol; no non-lazy slot of that name on a non-shipped level; the value is withheld |
| `reflectionMixinName:ifFail:` | `name` | `MixinName` | a reified mixin | a String: the mixin's name, a class side's `'<instance side's name> class'` | the subject is no reified mixin |
| `reflectionMixinIsMeta:ifFail:` | `isMeta` | `MixinIsMeta` | a reified mixin | `true` for a class side | no reified mixin |
| `reflectionMixinEnclosing:ifFail:` | `enclosingMixin` | `MixinEnclosing` | a reified mixin | the reified enclosing mixin, a class side's that of its instance side; `nil` at a module's root | no reified mixin |
| `reflectionMixinInstanceSide:ifFail:` | `instanceMixin` | `MixinInstanceSide` | a reified mixin | the reified instance side (itself for an instance side) | no reified mixin |
| `reflectionMixinClassSide:ifFail:` | `classMixin` | `MixinClassSide` | a reified mixin | the reified class side, synthesized or declared (itself for a class side) | no reified mixin |
| `reflectionMixinSlots:ifFail:` | `slots` | `MixinSlots` | a reified mixin | an `Array` of four-element `Array`s, `{name. isMutable. accessModifier. isLazy}`, declaration order | no reified mixin |
| `reflectionMixinMethods:ifFail:` | `methods` | `MixinMethods` | a reified mixin | an `Array` of two-element `Array`s, `{selector. accessModifier}`, the declared methods only, by `code.source.start` then selector | no reified mixin |
| `reflectionMixinNested:ifFail:` | `nestedClasses` | `MixinNested` | a reified mixin | an `Array` of the reified nested mixins, declaration order; empty for a class side | no reified mixin |
| `reflectionMixinAccess:ifFail:` | `accessModifier` | `MixinAccess` | a reified mixin | the Symbol of its accessor's access (`Mixin::access`), a class side's that of its instance side | no reified mixin |
| `reflectionMixin:methodSource:ifFail:` | `source` | `MethodSource` | a reified mixin | a String, the method's text, or `nil` | no reified mixin; the argument is no Symbol naming a declared method |
| `reflectionMixinSource:ifFail:` | `source` | `MixinSource` | a reified mixin | a String, the class declaration's text (a class side's: its instance side's), or `nil` | no reified mixin |
| `reflectionMixinIsShipped:ifFail:` | `isShipped` | `MixinIsShipped` | a reified mixin | `Mixin::shipped` (a class side's: its instance side's) | no reified mixin |

An access modifier is the Symbol `#public`, `#protected` or `#private`. A value is **withheld** when it is the VM mirror (`is_vm_mirror`) or an instance the shipped world records (`ShippedWorld::module_instances`). A **declared method** is a `MethodEntry::Compiled` whose selector is not a lazy slot's name of the same mixin; slot accessors, the factory and primitives are not. Every variant is `false` in `answers_false_to_a_stranger`, as `BehaviorPrimitive`'s are. The existing rows `objectClass:ifFail:`, `object:identicalTo:ifFail:`, `objectHash:ifFail:` and `behaviorSuperclass:ifFail:` are reused, not duplicated.

## The error texts

Signalled by `MirrorsFactory.ns` (an `Error` through `Error signal:`, or the kernel's `ArgumentError` as `TypedArraysFactory.ns` signals one), never by a leaf:

| Where | Class | Text |
|---|---|---|
| `ClassMirror reflecting:` of anything that is no class or metaclass | `ArgumentError` (argument set) | `reflecting: requires a class or a metaclass` |
| `ObjectMirror>>getSlot:` declined | `Error` | `getSlot: no slot named <name> that a mirror may read` |
| `ClassMirror>>enclosingObject` declined | `Error` | `enclosingObject: the enclosing object of a core-library class is not reflected` |
| `ClassMirror>>reflectee` withheld | `Error` | `reflectee: a core-library class reached by navigation is not handed out` |

`<name>` is the Symbol's text (`asString`), the only operand interpolated, since a Symbol's `asString` runs no user code.

---

### Task 1: The mirror table past 256 rows

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`MirrorPrimitive(u16)` at `:792`, its doc at `:787-790`, `from_index` at `:801`, the casts at `:2160` and `:2174`, the `const _` assert at `:2147`)
- Modify: `crates/ns-runtime/src/bytecode.rs` (`Instruction::SendMirror`, `:187`), `crates/ns-runtime/src/bytecode/compact.rs` (`CompactInstruction::SendMirror`, `:144`; the `const _` at `:283`)
- Modify: `crates/ns-runtime/src/shipped_code/encode.rs` (`mirror_primitive`, `:358`), `shipped_code/decode.rs` (`:223`), `shipped_code/mod.rs` (`FORMAT_VERSION` 1 → 2, `:38`)
- Modify: `docs/OPEN-WORK.md` (remove the item at `:235-250` that prepared this), `docs/design/core-library.md` (`:2172`, "`Primitive` stays two bytes"; it is four now)

**Interfaces:**
- Produces: `MirrorPrimitive(u16)`, `MirrorPrimitive::from_index(u16)`, `MirrorPrimitive::index() -> usize`. Every later task relies on rows past 255 being representable.

- [ ] **Step 1: Write the failing test** in `builtins/primitive.rs`'s `mod tests`: `the_mirror_table_may_pass_256_rows`, a `const` check that `MirrorPrimitive::from_index(300)`'s type accepts 300 -- written as `let _: MirrorPrimitive = MirrorPrimitive(300);` inside the test (the field is private to the module, the test is inside it). It does not compile before the change.
- [ ] **Step 2: Run it to see it fail**: `cargo nextest run -p ns-runtime -E 'test(the_mirror_table_may_pass_256_rows)'` -- a compile error (`300` does not fit `u8`).
- [ ] **Step 3: Widen.** `u8` → `u16` at every place in Files; the assert becomes `MIRROR.len() <= u16::MAX as usize + 1`; the shipped code's codec writes and reads two bytes (`self.u16(...)` / `self.reader.u16()`, adding the reader and writer method if the codec has none, modelled on its `u32`). `FORMAT_VERSION` 2, because an artifact of version 1 encodes a row in one byte. The compact `SendMirror` stays eight bytes; the `const _` at `compact.rs:283` proves it.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS; then the size tests in release (Global Constraints) -- PASS with every holder at its recorded size (`Resolved` 24, `Target` 16, `CacheEntry` 24, `GlobalEntry` 32, `Lookup` 24). If one grows, stop and report.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `refactor: index the mirror table with sixteen bits`.

### Task 2: A mixin's enclosing mixin and its module's text

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`Mixin::enclosing`, `Mixin::module_text`, `ModuleText`, `Mixin::empty`, `source_text`, `Mixin::set_enclosing_of_children`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`compile_root_mixin` `:307`, `build_root_mixin` `:318`, `mixin_of` `:332`, `build_mixin` `:295`, `build_class_side` `:441`, the literal at `:388`)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (`:375`, `:384`, `shipped_root_mixin` `:359`), `interpreter/kernel.rs` (`:1012`, the test caller)
- Modify: `crates/ns-runtime/src/shipped_code/mod.rs` (`decode_module` `:138`), `shipped_code/decode.rs` (`decode_section` `:28`, `Decoder::mixin` `:79`)
- Modify: `crates/ns-runtime/src/image/mixins.rs` (`decode` `:461` split into `read` and `build`; the literal at `:874`), `image/mod.rs` (`decode_described`, `:388-403`)
- Modify: `crates/ns-runtime/src/interpreter/metaclass.rs` (`metaclass_mixin`, `:106-123`)

**Interfaces:**
- Produces, in `mixin.rs`:
  - `pub struct ModuleText(pub Rc<str>)` with a `Debug` that prints `ModuleText(<len> bytes)`, so that printing a mixin does not print its module;
  - `Mixin::enclosing: std::cell::OnceCell<std::rc::Weak<Mixin>>` -- set once, after the enclosing mixin's `Rc` exists; a class side's names its instance side's mixin; empty for a module's root and a builtin's mixin;
  - `Mixin::module_text: Option<ModuleText>` -- `None` for a builtin's mixin, the VM mirror's, a synthesized class side's (it has no text of its own) and a mixin of an image without source;
  - `pub fn enclosing_mixin(&self) -> Option<Rc<Mixin>>` (upgrades the `Weak`);
  - `pub fn set_enclosing_of_children(parent: &Rc<Mixin>)` -- sets every `nested` mixin's and the `class_side`'s `enclosing` to `parent`; a cell already set is left (an `Rc` shared by two trees cannot happen: the builders build trees and `check_preorder` proves it for an image);
  - `pub fn source_text(text: &str, span: Span) -> Option<&str>` (Global Constraints).
- Produces, in `metaclass.rs`: a synthesized class side's `enclosing` names the instance mixin it was synthesized for, and its `module_text` is `None`.

- [ ] **Step 1: Write the failing tests:**
  - in `interpreter/instantiate.rs`'s `mod tests` (create it beside the file's other tests if it has none, modelled on `builtins/value.rs`'s `mod tests` and its `expect_ok`): `a_module_built_from_source_links_every_mixin_to_its_enclosing_one`: a module `Outer` declaring `Inner`, which declares `Innermost` and a class side; load it with `Interpreter::load_module`; through `interpreter.enter`, reach `Outer`'s root mixin (`heap.class_application(class)?.mixin`): `Inner`'s `enclosing_mixin()` is `Outer`'s (`Rc::ptr_eq`), `Innermost`'s is `Inner`'s, `Inner`'s class side's is `Inner`, `Outer`'s is `None`; every one's `module_text` is the module's text (`Rc::ptr_eq` with the `ResolvedModule::source`).
  - `a_synthesized_class_side_names_its_instance_side`: `metaclass_mixin` of a mixin declaring no class side: `enclosing_mixin()` is that mixin.
  - in `interpreter/core_library.rs`'s tests: `a_shipped_module_decoded_from_the_embedded_code_carries_its_text_and_links`: the `CollectionsFactory` root mixin from `shipped_root_mixin` -- its `module_text` equals `COLLECTIONS_FACTORY_SOURCE` byte for byte, `List`'s `enclosing_mixin()` is the root; the same under a boot that compiles the source (the test helper the file's other tests use to force compiling the shipped source).
  - in `tests/image.rs`: `a_mixin_decoded_from_an_image_with_source_carries_its_text_and_links` and `a_mixin_decoded_from_an_image_without_source_carries_no_text`: build an image of a two-class module with and without source (the helpers beside `a_second_typed_arrays_facet_is_a_record_of_its_shipped_class`), load, and check as above.
  - in `mixin.rs`'s tests: `a_span_outside_the_text_answers_no_source`: over the text `"abcé"` (five bytes, `é` two of them): `Span::new(0, 3)` → `Some("abc")`; `Span::new(0, 5)` → `Some("abcé")`; `Span::new(0, 4)` → `None` (inside `é`); `Span::new(2, 9)` → `None` (past the end); `Span::new(3, 1)` → `None` (reversed).
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run --workspace -E 'test(enclosing) | test(carries_its_text) | test(carries_no_text) | test(no_source)'`.
- [ ] **Step 3: The fields.** Add `enclosing` and `module_text` to `Mixin` with doc comments saying what each is for (the mirrors' `enclosingMixin`, `instanceMixin` and `source`) and that road step 5b moves the link to the id; default both in `Mixin::empty`; add them explicitly to the two full literals. `ModuleText` and `source_text` as in Interfaces.
- [ ] **Step 4: From source.** Thread `ResolvedModule::source` (an `Rc<str>`) from `compile_root_mixin` through `build_root_mixin`, `mixin_of`, `build_mixin` and `build_class_side` into each mixin's `module_text`, sharing the one `Rc`; after each `Rc::new` of a mixin, call `Mixin::set_enclosing_of_children`. The kernel's text is `resolved_kernel().source`.
- [ ] **Step 5: From the embedded shipped code.** `decode_module` takes the module's text as an `Rc<str>`: `shipped_root_mixin` answers it from a thread-local cache of one `Rc<str>` per shipped module, filled on first use from `KERNEL_SOURCE` or `shipped_module(name).source` (a separate branch for the kernel, since `shipped_module` panics on it) -- one copy per module per thread, never one per decode. `Decoder::mixin` sets `module_text` and, before returning, `set_enclosing_of_children`.
- [ ] **Step 6: From an image.** Split `mixins::decode` into `read` (rows, `check_ids`, `check_preorder`, the checks) answering the rows and `module_rows`, and `build`, which takes the modules' texts. `decode_described` calls `read`, then `decode_modules` (unchanged bytes: it needs only the module count), then `build` with `sources[module of row]` -- module `i` owns the rows `module_rows[i] .. module_rows[i + 1]` (to the end for the last) -- each module's text one `Rc<str>` made from its decoded `String`, `None` when the image carries no source. `build` calls `set_enclosing_of_children` after each parent's `Rc`. `LoadedModule::source` still answers the host its `String`.
- [ ] **Step 7: The synthesized class side** in `metaclass_mixin`: set its `enclosing` to `Rc::downgrade(mixin)` before caching it.
- [ ] **Step 8: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 9: Commit** after `cargo xtask gates commit images`: `feat: give every mixin its enclosing mixin and its module's text`.

### Task 3: The reified mixin

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass::Mixin` appended after `Ephemeron`, `ALL` to 38)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`private class Mixin = Object () ()` beside `Activation`, documented)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`kernel_class_name` after `:106`; `check_kernel_shape`'s lists if they need the new class; `PRIVATE_KERNEL_CLASSES` `:2484`; the absent list `:2436-2466`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`decide_factory` `:485`: a `Mixin` arm refusing `new`, beside `Symbol`'s at `:523`)
- Modify: `crates/ns-runtime/src/heap.rs` (`mixin_objects`, `allocate_mixin_object`, `mixin_object`, `record_mixin_object`, `mixin_object_id`; tracing in the scavenge's roots `:4881`, `forward_everything` `:5233`, `begin_marking` `:5437`, `traced_size` `:4992`; `describe` `:3020`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::reified_mixin`, `Mutator::mixin_of_reified`)
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs` (the private-class loop's list at `:660`)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)

**Interfaces:**
- Produces, in `heap.rs`:
  - `mixin_objects: Vec<Option<ObjectReference<'gc>>>`, indexed by `MixinId::index()`, strong;
  - `pub(crate) fn allocate_mixin_object(&mut self, class: ClassReference<'gc>, id: MixinId) -> ObjectReference<'gc>` -- `FixedPointers`, one field, the id as a `SmallInteger`, modelled on `allocate_fraction` (`:1197`);
  - `pub(crate) fn mixin_object(&self, id: MixinId) -> Option<ObjectReference<'gc>>`, `pub(crate) fn record_mixin_object(&mut self, id: MixinId, object: ObjectReference<'gc>)`;
  - `pub fn mixin_object_id(&self, object: ObjectReference<'gc>) -> Option<MixinId>` -- `Some` only for an object whose class's builtin is `Mixin` and whose one field is a `SmallInteger` naming a row of the table, as `fraction_parts` gates (`:2741`);
  - `describe` answers `the mixin <name>` for one, `the mixin <name> class` for a class side's.
- Produces, in `interpreter/mod.rs`:
  - `pub(crate) fn reified_mixin(&mut self, mixin: &Rc<Mixin>) -> ObjectReference<'gc>` -- the id from `heap.mixin_id(mixin)`, then the recorded object or a new one recorded;
  - `pub(crate) fn mixin_of_reified(&self, object: ObjectReference<'gc>) -> Option<Rc<Mixin>>`.

- [ ] **Step 1: Write the failing tests** in `heap.rs`'s `mod tests` (helpers there: `collect_for_test`, `scavenge_bare`, `move_bare_heap`) and `interpreter/mod.rs`'s tests:
  - `a_mixin_is_reified_once`: `reified_mixin` twice on one `Rc<Mixin>` answers the same object; on another mixin a different one; `mixin_of_reified` answers the mixin back (`Rc::ptr_eq`).
  - `a_reified_mixin_survives_every_collection_and_move`: reify, drop every other reference, `scavenge_bare`, `collect_for_test`, and (`#[cfg(debug_assertions)]`) `move_bare_heap`; `reified_mixin` still answers an object for which `mixin_object_id` is the same id and which is the table's entry.
  - `an_object_of_another_class_is_no_reified_mixin`: `mixin_object_id` of a `Fraction` and of an ordinary instance with one SmallInteger field answers `None`.
  - `mixin_new_is_refused`: `decide_factory` for the class `Mixin` refuses as for `Symbol` (assert the same refusal variant `Symbol` gets).
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(reified) | test(mixin_new_is_refused)'`.
- [ ] **Step 3: The builtin.** `BuiltinClass::Mixin` appended; `Kernel.ns`'s private class, its comment saying that an instance stands for one mixin and that only the mirrors read it; the kernel lists and the shipped world's private-class list; `decide_factory`'s arm.
- [ ] **Step 4: The table** on `Heap` and its tracing, the accessors, `describe`, and the two `Mutator` methods. `traced_size` counts the table's words.
- [ ] **Step 5: Regenerate the fixture**, run the size tests in release (Global Constraints), run `cargo nextest run --workspace` -- PASS.
- [ ] **Step 6: Commit** after `cargo xtask gates commit`: `feat: reify a mixin as a canonical kernel object`.

### Task 4: The reified mixin in the image

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OBJECT_MIXIN = 14`, "Next free number: 15"; `MIXIN_SIDE_INSTANCE = 0`, `MIXIN_SIDE_CLASS = 1` as a new family with its "Next free number: 2")
- Modify: `crates/ns-runtime/tests/opcode_tags.rs` (the three lists at `:215-227`, `:252-262`, `:348-360`, and the new family pinned)
- Modify: `crates/ns-runtime/src/heap/records.rs` (`ObjectShape::Mixin`), `heap.rs` (`object_shape` answers it before the `FixedPointers` arm)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`GraphTable::visit_value` `:994`, `encode_object` `:1573`: the record; the refusal of the VM mirror's class's mixin)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`Sizing::record` `:3514`, the second pass `:3812-4124`, the refusals as `DecodeError` variants with their `Display`)
- Modify: `crates/ns-runtime/src/image/shipped_names.rs` (`NamePosition::Mixin`, allowed for every `ShippedKind::Class` entry whatever its accessibility), `interpreter/shipped_world.rs` (`Positions`, `class_positions` `:461`)
- Modify: `crates/ns-runtime/src/image/inspect.rs` (`walk_graph` `:466-493`)
- Modify: `crates/ns-runtime/tests/image.rs`, `tests/image_truncations.rs`

**Interfaces:**
- Consumes: `Mutator::reified_mixin`, `Heap::mixin_object_id`, `record_mixin_object` (Task 3); `Mixin::enclosing` (Task 2), through which the decoder finds a class side's instance mixin.
- Produces: `OBJECT_MIXIN` written as `tag, header, reference (MIXIN_ROW u32 | MIXIN_SHIPPED u32), side u8` -- the reference exactly as `encode_class_record` (`:1466`) writes a class's mixin, the side 1 when the mixin is a class side (declared: its instance mixin's `class_side` is it; synthesized: it is `metaclass_mixin` of its `enclosing`); decoded, side 1 resolves through `Mutator::metaclass_mixin` of the named instance mixin, which answers the declared class side or the cached synthesized one.

- [ ] **Step 1: Write the failing tests** in `tests/image.rs`, with the fixtures beside `a_crafted_weak_array_and_ephemeron_decode_to_their_values` (`:4190`) and the round trips beside `:2954-3089`:
  - `a_reified_mixin_round_trips_onto_the_same_mixin`: a program module whose slot holds, put there by the test through `interpreter.enter` and `reified_mixin`, the reified mixins of its own class, of that class's class side (synthesized) and of a nested class's declared class side; after encode and load, each decodes to an object whose `mixin_of_reified` is the loaded module's corresponding mixin, and `reified_mixin` of that mixin answers the decoded object itself (canonical).
  - `a_reified_shipped_mixin_round_trips_by_name`: the reified mixin of `List`'s mixin and of `Object`'s class side; after load, `mixin_of_reified` is the loading binary's own mixin (`Rc::ptr_eq` with `class_application(List)?.mixin`).
  - refusals, each `refused_before_allocating` (`:4176`): `two_records_for_one_mixin_are_refused`, `a_mixin_record_past_the_table_is_refused`, `a_mixin_record_with_side_two_is_refused`, `a_mixin_record_on_another_class_is_refused`, `a_mixin_record_naming_a_shipped_module_instance_is_refused` (a shipped name that is not a class's);
  - `the_mirror_class_mixin_is_never_written`: reify the VM mirror's class's mixin into a module slot; encoding fails with the encoder's refusal of the mirror's class, never panics.
  - `inspect_shows_a_reified_mixin` beside `inspect_shows_a_weak_array_and_an_ephemeron` (`:3089`).
  - in `tests/image_truncations.rs`: a builder `reified_mixin()` beside `weak()` (`:162`) in `IMAGES`; every cut refused.
  - in `tests/opcode_tags.rs`: `OBJECT_MIXIN` is 14 in every list; the side family pinned.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(mixin_record) | test(reified) | test(mirror_class_mixin) | test(truncat) | test(opcode)'`.
- [ ] **Step 3: Encode.** `visit_value` admits an `ObjectShape::Mixin` like any object; its class header is `Mixin`'s shipped name; the body names the mixin by row (`context.mixin_ids`) or by shipped name (`context.shipped_by_mixin`) in `NamePosition::Mixin`; a mixin found in neither is refused with the encoder's error for an unnamed shipped object, and the VM mirror's class's mixin with lock (5)'s refusal.
- [ ] **Step 4: Decode.** First pass: read the reference and the side, refuse each case of Step 1 and a duplicate within the image (a set of `(reference, side)` seen) and one the heap already holds (`mixin_object` of the resolved id is `Some`), before any allocation, as `OBJECT_SYMBOL` does (`:3601`). Second pass: `shape_placeholder` as `PlaceholderFormat::Instance` with one field, the resolved `MixinId` as a `SmallInteger` through `fill_placeholder_field` (`:1505`), then `record_mixin_object`. The resolution of side 1 needs the mutator's `metaclass_mixin`, which may synthesize and so allocates nothing in the object space (it is a Rust-side `Rc`); call it in the first pass, so the second pass only looks up.
- [ ] **Step 5: Inspect** prints `mixin row <id>` or `mixin <shipped name>`, and `class side` when side is 1.
- [ ] **Step 6: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 7: Commit** after `cargo xtask gates commit images`: `feat: write a reified mixin into the image`.

### Task 5: The reflection leaves

**Files:**
- Create: `crates/ns-runtime/src/builtins/reflection.rs` (`ReflectionPrimitive` and the seventeen leaves of "The leaves, in one table"; its `mod tests`)
- Modify: `crates/ns-runtime/src/builtins/mod.rs` (`execute`'s arm, passing `selector` as `Weak`'s does), `builtins/primitive.rs` (`Primitive::Reflection`, `answers_false_to_a_stranger`, the seventeen rows appended)
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs` (`pub(crate) fn is_withheld(&self, value: Value<'gc>) -> bool`, answering the VM mirror and every recorded module instance)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated: the table changed)

**Interfaces:**
- Consumes: `Mutator::reified_mixin`, `Mutator::mixin_of_reified` (Task 3); `Mixin::enclosing_mixin`, `Mixin::module_text`, `source_text` (Task 2); `Mutator::metaclass_mixin`; `heap.class_application`, `class_this_class`, `class_enclosing`, `class_superclass`, `with_class_declared_slots`, `instance_field`, `is_class` (`heap.rs:3306-3494`); `Mutator::new_string`, `new_symbol`, `new_array` (`interpreter/mod.rs:1580-1658`).
- Produces: the seventeen rows, exactly as in the table; the walk by level that `SlotNamesOf` and `SlotNamed` share -- from the object's class up the superclass chain, each level's application's mixin, its own non-lazy slots, the field index `own_slot_base + position` among the non-lazy slots, as `MixinApplication::declared_slots` counts them -- skipping a level whose mixin is shipped.

- [ ] **Step 1: Write the failing tests** in `reflection.rs`'s `mod tests`, with the `leaf` helper and `expect_ok` copied from `builtins/value.rs`'s tests (`:623-680`), over a module loaded from an `indoc!` that declares `Base` (slot `inherited ::= 0`), `Derived = Base` (slots `own ::= 1`, `private secret = 2`, a lazy `later = 3`, methods `public b = (^1)`, `a = (^2)`, `private c = (^3)` in the source order `b`, `a`, `c`, a nested class `Inner`, a class side declaring `make`), a class `Plain` with no class side, and a class `Failed = Error ()()` declaring no slot of its own over the kernel's `Error`, which declares slots (if `Object`'s class accessors do not answer `Error`, the module takes it from `platform kernel Error` in its factory):
  - `a_class_answers_its_mixin_and_a_metaclass_its_class_side`: `reflectionMixinOf:` of `Derived` and of its metaclass; `reflectionMixinIsMeta:` false and true; `reflectionMixinName:` `'Derived'` and `'Derived class'`; `reflectionIsMetaclass:` of `Derived` false, of its metaclass true, of `3` declined.
  - `the_members_of_a_mixin_are_answered_in_their_order`: `reflectionMixinSlots:` of `Derived` is `{{#own. true. #protected. false}. {#secret. false. #private. false}. {#later. true. #protected. true}}`; `reflectionMixinMethods:` is `{{#b. #public}. {#a. #protected}. {#c. #private}}` -- the lazy slot's getter, the accessors and the factory absent; `reflectionMixinNested:` is the reified `Inner` alone; of the class side, `reflectionMixinMethods:` is `{{#make. #protected}}` and `reflectionMixinNested:` empty.
  - `a_synthesized_class_side_is_one_reified_mixin`: `reflectionMixinClassSide:` of `Plain`'s mixin twice answers one object, its `reflectionMixinInstanceSide:` the instance side, its name `'Plain class'`; the same after a full collection.
  - `a_mixin_answers_its_enclosing_mixin`: `Inner`'s `reflectionMixinEnclosing:` is `Derived`'s reified mixin; `Derived`'s is the module root's; the root's is `nil`; `Derived`'s class side's is the module root's.
  - `a_method_and_a_class_answer_their_source`: `reflectionMixin:methodSource:` of `#b` is `'public b = (^1)'` -- the text from the access modifier to the closing parenthesis; of `#own` (an accessor) declined; `reflectionMixinSource:` of `Inner` its declaration's text.
  - `the_slots_of_an_object_skip_lazy_and_shipped_levels`: `reflectionSlotNamesOf:` of a `Derived` is `{#inherited. #own. #secret}`; of a `Failed` `{}`, whatever `Error` declares; of `3`, a String and a class object `{}`.
  - `a_slot_is_read_by_name_from_the_nearest_level`: `reflectionSlotOf:named:` `#own` → 1, `#secret` → 2, `#later` declined (lazy), `#missing` declined, `'own'` (a String) declined.
  - `a_withheld_value_is_declined_whatever_its_level`: demand `platform transcript`, take the `TranscriptFactory` instance from the shipped world, and assert `reflectionSlotOf: it named: #vmMirror` declines; build in the test a program instance whose slot holds that root instance (through `interpreter.enter`, `set_instance_field`) and assert reading that slot declines too; `reflectionEnclosingObjectOf:` of the transcript's class declines (shipped level), of `Derived` answers the module instance, of a module's class `nil`.
  - `a_mixin_reports_whether_it_is_shipped`: `List`'s mixin true, `Derived`'s false, `Derived`'s class side's false, `List`'s class side's true.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(/^builtins::reflection::/)'`.
- [ ] **Step 3: Implement** the family as in the table. A leaf checks every operand before it allocates (`builtins/mod.rs:49-54`); it builds its symbols and strings first and its array last. The order of `MixinMethods` sorts by `code.source.start`, then by the selector's text, so it is deterministic whatever the table's hashing. A class side's own data comes from the class-side `Mixin`; its name, access, shipped flag and source from its instance side (`enclosing_mixin`, or the mixin whose `class_side` it is).
- [ ] **Step 4: Regenerate the fixture**, run the size tests in release, run `cargo nextest run --workspace` -- PASS (`every_mirror_selector_is_unique_ends_in_if_fail_and_takes_two_more_arguments` included).
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: the reflection leaves`.

### Task 6: `MirrorsFactory.ns`, `ObjectMirror`, `ClassMirror` and `platform mirrors`

**Files:**
- Create: `crates/ns-runtime/core-library/platform/MirrorsFactory.ns`
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns` (`lazy private mirrorsModule` after `typedArraysModule` `:161-175`; `lazy public mirrors = mirrorsModule build.` after `typedArrays` `:209-217`, its comment carrying the on-demand exemption as `typedArrays`' does; the module comment's lists `:1-4`, `:15-18`, `:28-33`; the `Platform` comment `:177-188`; `kernelModule`'s comment `:61-66`)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (`MIRRORS_FACTORY_SOURCE` after `:29-30`; the `ShippedFactory` comment `:46-56`; a `SHIPPED_MODULES` entry after `:83-87` with `ShippedFactory::MirrorAndKernel`; `"MirrorsFactory"` appended to `LAZY_SHIPPED_MODULES` `:95-100`; `build_shipped_module`'s comment `:229-236`)
- Modify: `crates/ns-runtime/tests/core_library_documentation.rs` (`[&str; 7]`, `"platform/MirrorsFactory.ns"`)
- Modify: `crates/ns-runtime/tests/platform.rs` (`the_mirrors_module_is_built_once_on_first_demand` after `:258-261`; `the_platform_hands_out_none_of_its_modules_classes` `:784-836` with `(&mirrors, "MirrorsFactory", "Mirrors")`; `the_mirrors_facet_is_a_value` modelled on `:627-660`)
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs` (`building_a_module_on_demand_draws_no_identity_hash` `:936`: `("mirrors", "ClassMirror")`; the lists at `:643-651`, `:749-757`, `:762-771`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (the absent list: `"MirrorsFactory"`, `"Mirrors"`, `"ObjectMirror"`, `"ClassMirror"`, `"MirrorGroup"`, `"SlotMirror"`)
- Modify: `crates/ns-analyzer/tests/hint_gate.rs:15` (`>= 8`)
- Create: goldens under `goldens/library/mirror/` (below)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin`

**Interfaces:**
- Consumes: the leaves (Task 5), `objectClass:ifFail:`, `object:identicalTo:ifFail:`, `objectHash:ifFail:`, `behaviorSuperclass:ifFail:`.
- Produces, in `MirrorsFactory.ns`:
  - `class MirrorsFactory usingVmMirror: mirror kernel: kernel = Value`, slots `private vmMirror = mirror.`, `private Error = kernel Error.`, `private ArgumentError = kernel ArgumentError.`; `public build = (^Mirrors new)`.
  - `private class Mirrors = Value () (public ObjectMirror = (^outer MirrorsFactory ObjectMirror) public ClassMirror = (^outer MirrorsFactory ClassMirror))`.
  - `public class ObjectMirror reflecting: anObject = Object` -- `reflectee`, `getClass` (a `NavigatedClassMirror`), `getSlot:`, `slots`, `=`, `hash`, `isKindOfObjectMirror`.
  - `public class ClassMirror reflecting: aClass = Object` -- `reflectee`, `name`, `simpleName`, `mixin`, `superclass`, `enclosingObject`, `isMeta`, `slots`, `methods`, `nestedClasses`, `=`, `hash`, `isKindOfClassMirror`; the initializer signals the `ArgumentError` of "The error texts" when `reflectionIsMetaclass:` declines.
  - `private class NavigatedClassMirror reflecting: aClass = ClassMirror reflecting: aClass` -- overrides `reflectee` to signal the withheld `Error` when the class's mixin is shipped; every navigation (`getClass`, `superclass`) answers one of these, never a `ClassMirror`.
  - `private class SlotMirror name: aSymbol value: anObject = Object` -- `name`, `value` (an `ObjectMirror`).
  - `private class MirrorGroup over: anArray = Object` -- `size`, `isEmpty`, `do:`, `collect:` (an `Array`), `detect:ifNone:`, `findMirrorNamed:`, `includesMirrorNamed:`, `asArray` (a copy).
  - `ClassMirror>>mixin`, `slots`, `methods` and `nestedClasses` need Task 7's `MixinMirror`, `SlotDeclarationMirror`, `MethodMirror` and `ClassDeclarationMirror`. Task 6 declares them with the members `ClassMirror` uses (`MixinMirror reflecting:`, `name`, `slots`, `methods`, `nestedClasses`, `isMeta`; the three others' `reflecting:...` factories and `name`), and Task 7 completes them.
- Produces, in `PlatformFactory.ns`: `platform mirrors`, the `Mirrors` facet.

- [ ] **Step 1: Write the goldens** (the `.toml` from the spec, before the code runs them), each a module with its own small model of classes:
  - `an_object_mirror_reads_an_object.ns`: `reflectee ==` the object; `getClass reflectee ==` its class; `getSlot:` of an inherited and of an own slot, a private one included; `slots` names and values in order, superclass first; `getSlot: #missing` caught `on: Error do:` and its `messageText` printed; two mirrors on one object `=` and of equal `hash`, mirrors on two equal-but-distinct objects not `=`.
  - `an_object_mirror_sends_its_reflectee_nothing.ns`: a class whose `=`, `hash` and `class` signal; `ObjectMirror reflecting:` it, `=`, `hash`, `getClass` and `slots` print without signalling.
  - `a_class_mirror_walks_the_superclass_chain.ns`: `name`, `isMeta` of a class and of its metaclass (`(ClassMirror reflecting: Foo) getClass`-style through `ObjectMirror reflecting: Foo`), `superclass name` to the chain's end (`nil`), `slots` names inherited first, `methods` names as a sorted list (`MirrorGroup collect:` then sort), `nestedClasses` names, `enclosingObject reflectee` of a nested class `==` its enclosing instance, of the module's class `nil`; `ClassMirror reflecting: 3` caught as `ArgumentError`, text printed.
  - `a_class_mirror_reached_by_navigation_holds_back_a_shipped_class.ns`: `ClassMirror reflecting: platform collections List` gives `reflectee` back; `(ObjectMirror reflecting: platform collections List new) getClass reflectee` signals the withheld `Error`; its `name` is `'List'`.
  - `platform_mirrors_is_one_facet.ns`: `platform mirrors == platform mirrors`; `platform mirrors ClassMirror == platform mirrors ClassMirror`; `platform mirrors MirrorsFactory` caught as `MessageNotUnderstood`.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-cli -E 'test(mirror)'`.
- [ ] **Step 3: Write `MirrorsFactory.ns`** as in Interfaces, the module comment in the factory pattern's words (`TypedArraysFactory.ns:1-41` is the model), every member documented to the standard; the shipped-level rule written once, on the module, and each refusal's member pointing to it. Equality never sends to a reflectee: `=` asks `object:identicalTo:ifFail:` and `hash` `objectHash:ifFail:`. `slots` of a `ClassMirror` walks `superclass` to the end, reverses, and concatenates each level's `mixin slots` (non-lazy), so a `<:` level contributes where it stands.
- [ ] **Step 4: Register the module** in every list of Files and wire `platform mirrors`.
- [ ] **Step 5: Format** (`cargo run -p ns-analyzer -- fmt` over the edited `.ns` files and the new goldens), **regenerate the fixture**, run `cargo nextest run --workspace` -- PASS.
- [ ] **Step 6: Commit** after `cargo xtask gates commit`: `feat: platform mirrors, ObjectMirror and ClassMirror`.

### Task 7: Mixin, declaration, method and slot mirrors

**Files:**
- Modify: `crates/ns-runtime/core-library/platform/MirrorsFactory.ns`
- Create: goldens under `goldens/library/mirror/`
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin`

**Interfaces:**
- Consumes: Task 6's classes; the mixin leaves (Task 5).
- Produces, all private classes of `MirrorsFactory`:
  - `MixinMirror reflecting: aReifiedMixin` -- `name`, `simpleName`, `isMeta`, `enclosingMixin`, `classMixin`, `instanceMixin`, `declaration`, `slots` (non-lazy), `lazySlots`, `methods`, `nestedClasses`, `=` (identity of the reified mixin), `hash`, `isKindOfMixinMirror`;
  - `ClassDeclarationMirror reflecting: aReifiedMixin` (its instance side) -- `name`, `instanceSide`, `classSide`, `enclosingClass`, `accessModifier`, `source`, `=`, `hash`, `isKindOfClassDeclarationMirror`;
  - `MethodMirror in: aReifiedMixin selector: aSymbol access: anAccessSymbol` -- `name`, `accessModifier`, `definingMixin`, `source`, `=` (same mixin and selector), `hash`, `isKindOfMethodMirror`;
  - `SlotDeclarationMirror in: aReifiedMixin name: aSymbol mutable: aBoolean access: anAccessSymbol lazy: aBoolean` -- `name`, `isMutable`, `isLazy`, `accessModifier`, `definingMixin`, `=`, `hash`, `isKindOfSlotDeclarationMirror`.
  - `ClassMirror>>nestedClasses` answers `ClassDeclarationMirror`s, inherited levels first.

- [ ] **Step 1: Write the goldens:**
  - `a_mixin_mirror_answers_only_its_own_members.ns`: a model `Super` / `Klass = Super` with slots in the five access and mutability variants, two lazy slots, three methods of three accesses, two nested classes, a class side with `classMethod`: `mixin slots` own only, in order, with `isMutable`, `isLazy` and `accessModifier` (an unmarked one `#protected`); `lazySlots`; `methods` names sorted, `definingMixin = mixin` for each; the class side's `name` `'Klass class'`, `methods` `{classMethod}`, `slots` and `nestedClasses` empty; `ClassMirror slots` against `mixin slots` (inherited first).
  - `a_mixin_mirror_navigates_its_declaration.ns`: `enclosingMixin` of a nested class's mixin is its enclosing class's, of the module's `nil`; `classMixin instanceMixin = mixin`; `declaration instanceSide = mixin`, `declaration classSide = mixin classMixin`; `declaration enclosingClass name`; a declaration is not `=` to its instance side; `nestedClasses` of a mixin answer declarations whose `name`s are in order.
  - `a_synthesized_class_side_is_one_mixin.ns`: `classMixin` of a class without a class side twice `=` and `==`-backed (`isMeta`, `name`, `instanceMixin = mixin`).
  - `method_mirrors_of_two_applications_are_equal.ns`: one mixin applied twice (two enclosing instances): the two `ClassMirror`s are not `=`, their `mixin`s are, and the method mirrors of one selector from each are `=` with equal `hash`.
  - `a_method_and_a_class_answer_their_source.ns`: `(mixin methods findMirrorNamed: #compute) source` prints the method's text; a class's `declaration source` prints the declaration; a shipped method's source (`List`'s `add:`) is a non-empty String (`isEmpty not` printed, never the text, which would pin the library's layout).
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-cli -E 'test(mirror)'`.
- [ ] **Step 3: Complete the classes**, every member documented; each `MirrorGroup` built from the leaf's array in the leaf's order.
- [ ] **Step 4: Format, regenerate the fixture, run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: mixin, declaration, method and slot mirrors`.

### Task 8: The shipped boundary

**Files:**
- Create: goldens under `goldens/library/mirror/`
- Modify: `crates/ns-runtime/tests/platform.rs` (`no_mirror_answers_a_withheld_value`)

**Interfaces:**
- Consumes: Tasks 5 to 7.

- [ ] **Step 1: Write the goldens and the test:**
  - `a_mirror_hands_out_nothing_a_shipped_level_holds.ns`: for `platform transcript`, `platform time`, `platform collections List new`, `platform typedArrays Int8Array new: 2`, a `platform time Stopwatch new` and the facets themselves: `ObjectMirror slots size` prints 0; `getSlot:` of every name in `getClass slots` (each `SlotDeclarationMirror name`) caught `on: Error do:`, and the count of refusals printed equal to the count of names; `getClass enclosingObject` refused; `getClass reflectee` refused; for each class the facets hand out (`List`, `Map`, `Int8Array`, `Stopwatch`, `Collection`): `(ClassMirror reflecting: it) enclosingObject` refused, `reflectee ==` it, and the `superclass` chain's `reflectee`s each refused or `==` a class the program can name -- printed as a count of refusals per chain.
  - `a_program_class_over_a_shipped_one_reads_only_its_own_slots.ns`: `Listed = List` with slots `extra ::= 7`: `ClassMirror slots` names `List`'s declared slots then `extra`; `ObjectMirror slots` only `extra`; `getSlot: #extra` 7; `getSlot:` of a `List` slot name refused. A class `Viewed` that applies, with a `<:` clause, the mixin of a shipped class the program names (`platform kernel Collection`; the syntax as in `goldens/language/class/mixin/`): the `ClassMirror` of the level the clause adds (found by walking `superclass`) answers `enclosingObject` with the refusal.
  - `no_mirror_answers_a_withheld_value` in `tests/platform.rs`: a Rust test that runs a module walking, through mirrors, from every facet object and every class they hand out: every `slots` value, every `getSlot:` answer, every `enclosingObject reflectee` and every `superclass` `reflectee` that answers without signalling is collected into an `Array` the module answers; the test asserts that no element `is_withheld` and no element is a class object whose mixin is shipped and not among the classes the facets hand out.
- [ ] **Step 2: Run them.** They pass if Tasks 5 to 7 hold the rule; a failure is a defect in those tasks, fixed in their code within this task's commit and named in its message.
- [ ] **Step 3: Format, run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 4: Commit** after `cargo xtask gates commit`: `test: hold the mirrors to the shipped boundary`.

### Task 9: A snapshot holding mirrors

**Files:**
- Modify: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: Task 4's record; Tasks 6 and 7.

- [ ] **Step 1: Write the failing test** `a_program_holding_mirrors_loads_back_holding_them`: a module whose instance, built by `main:`-less instantiation in the test with the platform, keeps in slots a `MixinMirror` on its own class (`(ClassMirror reflecting: Foo) mixin`), a `ClassMirror` on `platform collections List`, a `MethodMirror` (`mixin methods findMirrorNamed: #compute`) and an `ObjectMirror` on a program object; build an image of that instance, load it, and send through the loaded instance: the mixin mirror's `name`, `=` against a freshly reached `(ClassMirror reflecting: Foo) mixin` (true), the class mirror's `reflectee == platform collections List` (true), the method mirror's `name`, the object mirror's `getSlot:`. Encode the image both with and without source; without, the method mirror's `source` is `nil`.
- [ ] **Step 2: Run it**: `cargo nextest run -p ns-runtime --test image -E 'test(holding_mirrors)'`. If it fails for a reason that is Task 4's (a record or a name the encoder refuses), fix it in Task 4's code within this commit and say so in its message.
- [ ] **Step 3: Commit** after `cargo xtask gates commit images`: `test: a program holding mirrors survives a snapshot`.

### Task 10: Documentation

**Files:**
- Modify: `docs/design/liveness-and-reflection.md` ("Planned: road step 5a, mirrors that read" becomes the present-tense section "Mirrors that read"; the status block says what is built; the "(proposed, for the user to confirm with the plan)" mark goes once the user confirmed it)
- Modify: `docs/design/image-format.md` ("Planned: the reified mixin record" becomes present tense, placed in "Sections" or beside the class records; the tag table)
- Modify: `docs/design/core-library.md` (the module table and its count, "the table has five entries" `:3899-3905`, every list of shipped modules -- `:9`, `:24`, `:31`, `:67`, `:81`, `:721-751`, `:798`, `:858`, `:2649`, `:2675`, `:2682`, `:3232`, `:3954` -- lock (2)'s list, the on-demand rule's exemptions `:1082-1097`, the row count `:631`, `Kernel.ns`'s private class `Mixin`)
- Modify: `docs/design/object-model.md` (the reified mixin; `Mixin::enclosing` and `Mixin::module_text`; "Source retention and provenance" `:1552-1568`: the text now reaches the heap through the mixins), `docs/design/memory.md` (the table of reified mixins among the traced Rust-side handles), `docs/design/README.md` (`:88`, the golden directory `library/mirror/`)
- Modify: `docs/OPEN-WORK.md` ("Next" becomes road step 5b; the road's step 5 entry split into 5a built and 5b; new items: `getSlot:` of a lazy slot, the indexed elements in `ObjectMirror slots`, `evaluate:` and `perform:`, metadata through mirrors (§4.3), `instances` and `referringObjects`, the activation mirrors -- each with where the spec or prior art asks for it)
- Modify: `docs/PROGRESS.md` (the milestone)
- Modify: `CLAUDE.md` (the `ns-runtime` paragraph: `MirrorsFactory.ns` among the shipped modules, "the other five built on first demand"; lock (2)'s list; the on-demand rule's exemptions; the documentation rule's module list)

- [ ] **Step 1: Write the documents** in the present tense; read each section touched end to end and delete what is no longer true (`CLAUDE.md`, "Documentation other than a plan describes the current state").
- [ ] **Step 2: Commit** (Markdown only, no gates): `docs: describe the mirrors that read`.
