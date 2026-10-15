# Shipped modules on first demand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The boot builds only `Kernel` and `PlatformFactory`; `TranscriptFactory`, `TimeFactory`, `CollectionsFactory` and `TypedArraysFactory` are built when a program first asks for them, and each class of such a module only when its accessor builds it.

**Architecture:** `PlatformFactory.ns`'s lazy module slots build a module with ordinary Newspeak sends between two new mirror leaves that run no Newspeak: `shippedModuleClassNamed:` (root mixin → root class, kept pending) and `recordShippedModule:` (checks the mirror slot, records the instance and its names). A lazy module's classes are named in `nested::after_clause` as their accessors build them. `image::load` builds, from the host and before the tenure, what its shipped-names section names. The world's order becomes the order of demand.

**Tech Stack:** Rust 1.98.0, edition 2021; Newspeak (`PlatformFactory.ns`); `cargo-nextest`; no new dependency.

**Spec:** `docs/superpowers/specs/2026-10-14-precompiled-shipped-code-design.md`, section "The second stage: loading a module on first demand" -- read it before any task. The earlier sections describe the first stage, built by `docs/superpowers/plans/2026-10-14-2207-precompiled-shipped-code.md`.

## Global Constraints

- Eager: `Kernel`, `PlatformFactory`. Lazy: `TranscriptFactory`, `TimeFactory`, `CollectionsFactory`, `TypedArraysFactory`. On both paths (decoded artifact and source) and under both lowerings.
- The run loop gains no `Step` variant, no `Continuation` variant and no field on either.
- The two new leaves are mirror rows (`builtins/primitive.rs`'s `MIRROR`), reachable only by shipped code; they run no Newspeak.
- `moduleInstanceNamed:` answers only a recorded shipped module's instance and declines everything else, as today.
- Nothing about a lazy module is recorded in the shipped world before `recordShippedModule:` accepts its instance; a class is named only once it exists.
- No identity hash is drawn by building a module or a class of it.
- `ShippedWorld::add_module_instance` still refuses a second instance; a shipped name recorded twice is still a defect (`record_entries`' assertion).
- `image::load` refuses an unknown shipped name before building anything; it builds only modules and classes of the binary's own shipped world, never anything the image describes; a started image builds nothing.
- Every member of `PlatformFactory.ns` keeps a `///` comment to the standard `CLAUDE.md` demands (`crates/ns-runtime/tests/core_library_documentation.rs`); after editing it, run `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/PlatformFactory.ns`.
- Names written out; comments explain decisions; no golden's expected output changes except a new golden this plan adds.
- Commit messages are conventional and end with the trailer lines the controller gives.

**Gates, staggered:** every commit runs `cargo xtask gates commit` (it regenerates the embedded shipped code after formatting). `images`, `caches`, `benchmarks`, `stress` run once, in Task 6.

## Review Focus

1. **A demand interrupted by the depth limit** must leave nothing recorded and a pending class the next demand reuses. Pinned by the golden of Task 3.
2. **`recordShippedModule:` handed an object that is not an instance of a pending root class** (or a second instance) must decline, never record. Pinned in Task 2.
3. **An image naming a class of a lazy module that this run never built** must load: the loader builds it. Pinned in Task 4.
4. **A class of a lazy module built before its module is recorded** cannot happen (the accessor needs the instance); a class of an eager module built by `complete_shipped_world` after the hook named it must not be named twice. Pinned in Task 1.
5. **Determinism of image bytes**: two runs of the same program write the same image. Pinned by the existing byte-equality tests in `tests/image.rs`, which Task 4 keeps green.

---

### Task 1: The shipped world records one module and one class at a time

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs`
- Test: same file's `tests`

**Interfaces:**
- Produces, on `Mutator<'gc>` (or as `pub(crate)` functions taking `&mut Mutator<'gc>`):
  - `record_shipped_module(&mut self, module: Symbol, instance: ObjectReference<'gc>)` -- `add_module_instance`, then the names of the instance, the root class, its metaclass and its levels (today's first two pushes in `shipped_entries`).
  - `record_shipped_class(&mut self, module: Symbol, nested: &Rc<Mixin>, class: ClassReference<'gc>)` -- the names of one class the root declares, its metaclass and its levels, with the positions `shipped_entries` computes today (public / `KERNEL_CLASSES_FOR_SHIPPED_MODULES` / `accessed_classes` / builtin tests); a no-op when the class is already named.
  - `ShippedWorld::module_of_instance(&self, instance: ObjectReference<'gc>) -> Option<Symbol>`.
- `complete_shipped_world` becomes: for each recorded module, build each class its root declares (`build_nested_class`) and `record_shipped_class` it. `install_shipped_modules` calls `record_shipped_module` where it called `add_module_instance`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_class_is_named_once_however_often_it_is_recorded() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let before = mutator.shipped_world.entries().len();
        let collections = Symbol::intern("CollectionsFactory");
        let instance = mutator
            .shipped_world
            .module_instance(collections)
            .expect("built at boot until Task 3");
        let application = root_application(mutator.heap(), instance);
        let list = application
            .mixin
            .nested
            .iter()
            .find(|nested| nested.name.as_str() == "List")
            .expect("CollectionsFactory declares List")
            .clone();
        let class = application
            .nested_slot
            .get(&mixin_key(&list))
            .and_then(|index| mutator.heap().instance_field(instance, *index))
            .and_then(Value::as_object)
            .expect("List was built at boot");
        mutator.record_shipped_class(collections, &list, class);
        assert_eq!(mutator.shipped_world.entries().len(), before);
    });
}
```

- [ ] **Step 2: Run it to see it fail** -- `cargo nextest run -p ns-runtime -E 'test(a_class_is_named_once_however_often_it_is_recorded)'`: compile error, no `record_shipped_class`.

- [ ] **Step 3: Implement** by splitting `shipped_entries` into the two functions above, keeping its positions logic in one place (a helper computing one class's `Positions` from the module, the root mixin and the nested mixin). `record_entries` stays the only writer of `entries`/`by_name`. Update `ShippedWorld::entries`' field comment: entries are recorded per module, then per class as each is built, in the order of recording.

- [ ] **Step 4: Run the crate's tests** -- `cargo nextest run -p ns-runtime`: all pass; the world's entries after a boot are the same set as before (the existing `shipped_world.rs` tests pin names and positions).

- [ ] **Step 5: Gates and commit** -- `cargo xtask gates commit`; `refactor: record the shipped world a module and a class at a time`.

---

### Task 2: The two leaves, the naming hook and one host-side build

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`ModulePrimitive`, two `MIRROR` rows)
- Modify: `crates/ns-runtime/src/builtins/module.rs`
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs` (pending classes, traced)
- Modify: `crates/ns-runtime/src/interpreter/nested.rs` (`after_clause`'s hook)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (host-side build; `install_shipped_modules` uses it)
- Modify: `crates/ns-runtime/src/shipped_code/mod.rs` (`generate` compiles without building)
- Test: `builtins/module.rs` and `core_library.rs` tests

**Interfaces:**
- `ModulePrimitive::{InstanceNamed, ClassNamed, Record}`; rows `"shippedModuleClassNamed:ifFail:"` (unary, the name the subject) and `"recordShippedModule:ifFail:"` (unary, the instance the subject), beside `moduleInstanceNamed:ifFail:`.
- `ShippedWorld` gains `pending: Vec<(Symbol, ClassReference<'gc>)>`, traced in its `Trace` impl.
- `pub(crate) fn shipped_module_class<'gc>(mutator: &mut Mutator<'gc>, name: Symbol) -> Option<ClassReference<'gc>>` -- `None` unless `name` is a lazy shipped module not yet recorded; answers the pending class if one exists, else builds it (`shipped_root_mixin`, `root_class_of_mixin`) and keeps it pending.
- `pub(crate) fn record_pending_module<'gc>(mutator: &mut Mutator<'gc>, instance: Value<'gc>) -> Option<ObjectReference<'gc>>` -- `None` unless `instance` is an object whose class is a pending root class; runs `check_factory_holds_the_mirror` for a module whose factory takes the mirror; removes the pending entry; `record_shipped_module`; answers the instance.
- `pub(crate) const LAZY_SHIPPED_MODULES: &[&str] = &["TranscriptFactory", "TimeFactory", "CollectionsFactory", "TypedArraysFactory"];` in `core_library.rs`.
- `impl Interpreter { pub(crate) fn build_shipped_module(&mut self, name: &str) }` -- from the host: `shipped_module_class`, `instantiate` with the `ShippedFactory` arguments, `record_pending_module`; a failure panics through `booted`.
- The hook in `after_clause`, after the class is cached: if `nested.shipped` and `mutator.shipped_world.module_of_instance(enclosing)` is `Some(module)` and `nested` is one of that root's `nested` mixins (pointer-equal), `record_shipped_class(module, &nested, class)`.

- [ ] **Step 1: Write the failing tests** in `builtins/module.rs`'s tests (copy the bootstrap and `expect_ok` style of `core_library.rs`'s `module_instance_named_answers_the_boots_instance_and_declines_anything_else`):
  - `the_class_leaf_declines_a_name_that_is_no_lazy_module`: `Kernel`, `PlatformFactory`, `Nothing`, and a non-symbol receiver all answer `None`.
  - `the_class_leaf_declines_a_recorded_module`: while all modules are still built at boot (until Task 3), `CollectionsFactory` answers `None`.
  - `the_record_leaf_declines_what_is_no_instance_of_a_pending_class`: `nil`, a fresh `Object`, and the kernel instance answer `None`, and the world's entry count is unchanged.

- [ ] **Step 2: Run them to see them fail** -- compile errors (no `ClassNamed`, `Record`).

- [ ] **Step 3: Implement** the leaves through `shipped_module_class` and `record_pending_module`; the hook; `build_shipped_module`; route `install_shipped_modules` through `build_shipped_module` for every module in `SHIPPED_MODULES` (still all of them -- Task 3 makes four lazy), then `complete_shipped_world`. `generate()`: boot on the source path, then compile each of `module_names()` with `shipped_root_mixin_from_source` inside one `enter` -- no instance needed; `Interpreter::shipped_root_mixins` goes if nothing else uses it.

- [ ] **Step 4: Run the crate's tests** -- all pass; the `shipped_code::` tests confirm `generate()` still round-trips.

- [ ] **Step 5: Gates and commit** -- `feat: build a shipped module through two mirror leaves and name its classes as they are built`.

---

### Task 3: `image::load` builds what its names name

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs` (`load`)
- Modify: `crates/ns-runtime/src/image/shipped_names.rs` (`resolve_in`'s doc; a builder of named modules and classes)
- Modify: `crates/ns-runtime/tests/image.rs` (fixtures and `resolve_in` callers)
- Test: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- `impl Interpreter { pub fn build_shipped_names(&mut self, names: &[ShippedName]) -> Result<(), DecodeError> }` (`#[doc(hidden)]` if only tests and `load` call it): first refuses any name whose module is no shipped module or whose declaration that module's root does not declare (`UnknownShippedName`), and in a started image any name but the kernel's (`ShippedNameInStartedImage`) -- before building anything; then, for each lazy module named and not recorded, in `SHIPPED_MODULES` order, `build_shipped_module`; then, for each named declaration of a lazy module not yet named, `build_nested_class` on the module's instance.
- `load`: phase 1 decodes up to and including the shipped-names section inside its `enter` and does not resolve; then, outside it, `build_shipped_names(&names)?`; then `resolve` (inside a short `enter`), then `tenure_now`, then phase 2 as today.

Every module is still built at boot when this task lands (Task 4 makes four lazy), so the builder's building half is exercised only from Task 4 on; its refusing half is tested here.

- [ ] **Step 1: Write the failing tests** in `tests/image.rs` (style of its neighbours): `build_shipped_names` refuses a name of an unknown module, a name of a declaration its module's root does not declare, and in a started image a `CollectionsFactory` name, each with the `DecodeError` above, and builds nothing (the world's entry count is unchanged).

- [ ] **Step 2: Run them** -- compile error, no `build_shipped_names`.

- [ ] **Step 3: Implement** `build_shipped_names` and the new order in `load`. Update `load`'s doc comment: the loader builds the binary's modules and classes the section names, before the tenure, by the binary's own code.

- [ ] **Step 4: Run** `cargo nextest run --workspace` -- green. Then `cargo nextest run --workspace --profile images` -- green.

- [ ] **Step 5: Gates and commit** -- `feat: load an image by building the shipped modules and classes it names`.

---

### Task 4: Four modules on first demand

**Files:**
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns` (the four lazy module slots and their comments)
- Modify: `crates/ns-runtime/src/interpreter/core_library.rs` (`install_shipped_modules` builds `PlatformFactory` only; doc comments)
- Modify: `crates/ns-runtime/src/builtins/module.rs` (module doc)
- Create: `goldens/runtime/depth-limit/a_module_first_demanded_at_the_depth_limit_is_built_later.ns` and `.toml`
- Modify: every test that assumes all modules after `bootstrap()` (list below), `crates/ns-runtime/tests/inline_caches.rs` (boot counts), `crates/ns-runtime/tests/image.rs`
- Test: `core_library.rs` tests, `crates/ns-runtime/tests/platform.rs`

**Interfaces:**
- Consumes Task 2's leaves and `build_shipped_module`, and Task 3's `Interpreter::build_shipped_names`.

- [ ] **Step 1: Write the failing tests**
  - In `core_library.rs`: `the_boot_builds_only_the_kernel_and_the_platform_module` -- after `Interpreter::bootstrap()`, `module_instance` is `Some` for `Kernel` and `PlatformFactory`, `None` for each of `LAZY_SHIPPED_MODULES`.
  - In `tests/platform.rs` (style of its neighbours): a program sending `platform collections` twice answers identical namespaces; after it, the shipped world (through a `#[doc(hidden)]` query if the test is an integration test -- prefer a unit test in `core_library.rs` that runs a small module through `load_module`/`instantiate`/`send`) names `CollectionsFactory List` once `platform collections List` was sent and does not name `CollectionsFactory Map`.
  - Identity hashes: `building_a_module_on_demand_draws_no_identity_hash`, modelled on `a_boot_draws_no_identity_hash`: send `platform collections`, `platform time`, `platform transcript`, `platform typedArrays` and one class of each, then assert the heap's identity-hash counter is where the boot left it.
  - In `tests/image.rs` (style of its round trips): a program that keeps a `List` (sent `platform collections List new` and stored it) is written as its neighbours write images, then loaded into a fresh interpreter: the load succeeds, and the loaded object's class is the `List` the loading world's `CollectionsFactory` instance answers.
  - The golden, modelled on `goldens/runtime/depth-limit/an_error_just_below_the_depth_limit_is_caught.ns`: probe the depth, then at the probed depth send `platform collections` inside `on: Error do:` and show the message text, then from a shallow stack show `(platform collections List new add: 1; yourself) size`. Its `.toml` expects `stack overflow: recursion too deep` then `1` -- establish the first line by running it, and check it is the depth error and not another; never write an expectation the program does not produce for a reason you understand.

- [ ] **Step 2: Run them to see them fail** -- the boot still builds everything.

- [ ] **Step 3: Switch.** In `PlatformFactory.ns`, each lazy module slot:

```newspeak
  lazy private collectionsModule = vmMirror
      moduleInstanceNamed: #CollectionsFactory
      ifFail: [
        vmMirror
            recordShippedModule: ((vmMirror shippedModuleClassNamed: #CollectionsFactory ifFail: [nil])
                usingKernel: kernel)
            ifFail: [nil]].
```

with `usingVmMirror: vmMirror` for `TranscriptFactory` and `TimeFactory` and `usingVmMirror: vmMirror kernel: kernel` for `TypedArraysFactory` (the `ShippedFactory` table in `core_library.rs` is the authority; keep them equal). Rewrite each slot's comment: built on first use, the one instance, and that a demand an unwind interrupted builds it again. Format with `ns-next-analyzer fmt`. In `install_shipped_modules`, build only `PlatformFactory` through `build_shipped_module`, then `complete_shipped_world` (kernel and `PlatformFactory`).

- [ ] **Step 4: Fix every test that assumed a complete world.** Known: `core_library.rs` (`the_boot_keeps_every_shipped_root_mixin_in_table_order` if it survived Task 2, the `moduleInstanceNamed:` test), `shipped_world.rs` (positions of `TypedArraysFactory`, `CollectionsFactory`, `TimeFactory` and every root), `image/shipped_names.rs` (a `CollectionsFactory` instance name), `tests/platform.rs` (`..._is_built_once_at_boot` and its neighbours: the instance is now built once on first demand -- rename and restate, never weaken), `tests/inline_caches.rs` (the boot's site and entry counts: establish the new numbers by running, and say in the test's comment why they fell). A test that needs a lazy module first demands it (`platform collections`, or `build_shipped_module` from the host). In `tests/image.rs`, every `resolve_in` against a fresh boot that names a lazy module (`FIXTURE_NAMES` and the callers near the lines Task 3's report lists) first calls `build_shipped_names`, as `load` does.

- [ ] **Step 5: Run** `cargo nextest run --workspace` -- green; `cargo nextest run --workspace --profile images` -- green.

- [ ] **Step 6: Gates and commit** -- `cargo xtask gates commit`; `feat: build four shipped modules on first demand`.

---

### Task 5: Measure, and the living documents

**Files:**
- Modify: `docs/design/execution.md` ("What the precompiled shipped code moved": rewrite the paragraph "The second stage does not pay at this size" -- it points to a missing `OPEN-WORK.md` item and states a verdict the user overturned)
- Modify: `docs/design/core-library.md` ("The embedded shipped code", "The module table and the boot", every place that says every module is built at boot; the VM mirror's leaves)
- Modify: `docs/design/image-format.md` ("Shipped names": the world is built on demand; "The image under generations": the loader builds named modules before the tenure, the restated argument)
- Modify: `docs/design/memory.md` (where it says nothing allocates between the bootstrap and the graph pass)
- Modify: `docs/OPEN-WORK.md`, `CLAUDE.md` (lock (2) of the VM mirror: the check now runs in `recordShippedModule:` as well as at boot; the image invariant's "the loader runs no code before the graph")
- Temporary, never committed: timing marks

- [ ] **Step 1: Measure** as the first stage's Task 7 did (`execution.md`'s section says how: release, pinned `RUSTFLAGS`, 30 marked runs and 40 whole-process runs, one build per side, `cargo xtask shipped-code` before each build): this branch's head against the head before Task 1, the hello program; and a program that sends `platform collections List new` once, for the first demand's cost. Remove the marks; `git status` clean under `crates/`.

- [ ] **Step 2: Write the documents** in the present tense, citing files; no pointer to the spec or the plans. `execution.md`'s rewritten paragraph gives the numbers before and after and why the stage was built (decided with the user on 2026-10-15: the artifact grows with the library).

- [ ] **Step 3: Gates and commit** -- `cargo xtask gates commit` if anything outside Markdown changed; `docs: describe the shipped modules built on first demand`.

---

### Task 6: The end of the batch's gates

- [ ] `cargo xtask gates end-of-batch`, then `cargo xtask shipped-code` and `cargo nextest run --workspace --release` -- green. On a failure: superpowers:systematic-debugging; a fix is its own commit and this task runs again.
