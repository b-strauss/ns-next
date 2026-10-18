# `become:` and recorded guards (road step 5b1) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 5b1, the VM's half of live editing: a mixin's identity is its `MixinId`, so a fresh mixin can be installed under an old one's id; `become:` exists as an eager bulk forward behind one VM-mirror leaf; and every slot access whose index was decided without looking at the receiver checks a recorded guard, so a receiver the forward replaced is never written in the old layout.

**Architecture:** Every version of a mixin answers the one id it was first registered under, and the heap keeps per id the current version, its enclosing mixin's id and its module's text; `Mutator::replace_mixin` swaps the current version and evicts what was cached for the id. The bulk forward is a fourth `Relocation` beside the compaction's and the move stress mode's: it validates its pairs, copies each old object's identity hash into its new one, turns each old object into a forwarder, rewrites every handle the heap and the step roots hold through `forward_everything`, remembers each old holder now naming a young object, and vacates every inline cache -- all inside the one leaf `elementsOf:forwardIdentityToElementsOf:ifFail:`, which refuses anything but an ordinary instance of a class whose mixin is not shipped. An activation whose code addresses its own slots records, when it is pushed, its receiver's guard and the full-width index of the level it was entered at; a private or super slot accessor's cache entry records the same pair. One compare is the hot path; a mismatch takes a cold check that the receiver's application chain still contains that level, and otherwise raises an ordinary Newspeak error.

**Tech Stack:** Rust 1.98.0, the Newspeak core library, `cargo-nextest`.

**Spec:** `docs/design/object-model.md`, "Planned: `become:` as an eager bulk forward" and "Planned with road step 5: what a slot access checks"; `docs/design/liveness-and-reflection.md`, "Planned: road step 5b, MirrorBuilder and atomic installation" and "Planned with road step 5: an edit replaces the mixin under its id" ("What the id has to carry"); `docs/design/memory.md`, "Generations". Read them before Task 1. Prior art: `prior-art/primordialsoup/vm/heap.cc:1072-1229` (`BecomeForward`: validation `:1073-1113`, hash copy and corpse `:1118-1138`, the remembered set rebuilt `:1214-1230`), `prior-art/primordialsoup/vm/primitives.cc:2271-2281` (the one primitive), `prior-art/primordialsoup/newspeak/Mirrors.ns:308-327` (`installAll`, its one caller). Copy no code.

**Decided with the user on 2026-10-18** (this plan's own decisions; the spec's are settled and not reopened):

1. Road step 5b is built as two plans: this one (5b1), then the MirrorBuilder, the atomic install, the weak application cache, the image's edited mixins and run-time compilation (5b2).
2. The bulk forward runs inside the leaf, in the middle of a step, not at the next safepoint: no instruction runs between the forward and the leaf's answer, so it is atomic; nothing moves but the forwarded objects become garbage, so no move epoch changes. `Heap::class_set_application`'s heap walk already runs inside a step.
3. The leaf comes with this plan, its Newspeak caller with 5b2's install. Tests reach it by handing the VM mirror to a test program from the host, which no program can do.
4. The leaf is a lock of its own: both sides must be ordinary instances (format `FixedPointers`) of a class whose application's mixin is not shipped, besides Primordial Soup's checks (equal sizes, no immediate, no split, no chain).

**Refined while planning, stated here for review:** the spec's cold check asks whether "the receiver's application inherits the accessing level". The accessing level is the application the activation was *entered* at, by its full-width `ApplicationIndex`, recorded beside the guard: the class object's *current* application is the wrong one, since 5b2's install rewrites a class object's application in place while an activation entered before it still runs code compiled against the old mixin's slot list. So a stack entry records two `u32`s, and only for code that addresses its own slots (`CompiledCode::addresses_own_slots`, derived as `outer_reach` is), so that a send to an ordinary method pays one flag test at push.

## Global Constraints

- Every commit is green: the per-commit gates in `CLAUDE.md` ("The gates", group `commit`; plus `images` where a task names it, and `caches` for every task touching `interpreter/inline_cache.rs`) run before each commit through `cargo xtask gates commit` (`cargo xtask gates commit images`, `cargo xtask gates commit caches` where named). The end-of-batch gates (`cargo xtask gates end-of-batch`) run once, after Task 8, by the controller (`finishing-a-batch` skill); an implementer does not run them, and a task's commit is work in progress until they have run.
- Commits are conventional (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`, `perf:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01RRpMXsUbvJ9BUg9se8QKwP`. Never change the git identity.
- Work on the branch `feat/become-and-recorded-guards` in `D:\Workspace\ns-next`, checked out in the main working directory, no worktree. It exists already.
- Run every command in the foreground.
- Names are written out (`CLAUDE.md`, "Names are written out"): `replacement`, `entered_level`, `recorded_guard`, `forwarded`, never `repl`, `lvl`, `fwd`.
- **Program outcomes are pinned, not re-derived** (`CLAUDE.md`). No existing golden's `.toml` changes in this plan; if one would, stop and report.
- The `MIRROR` table gains one row, appended after `reflectionSymbolText:ifFail:` (`builtins/primitive.rs:2331`), never inserted. It changes the shipped world's fingerprint, so `crates/ns-runtime/tests/fixtures/golden-image.bin` is regenerated in the same commit: `cargo nextest run -p ns-runtime --test image --run-ignored only regenerate_the_golden_fixture`, then confirm with `cmp -l` against the previous fixture (`git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $TEMP/old-fixture.bin`) that only the 8 fingerprint bytes after the flag byte differ.
- **The size tests run in release** after every task that touches `Target`, `Resolved`, `Primitive` or `Heap`'s fields: `cargo nextest run -p ns-runtime --release -E 'test(stays_the_size) | test(one_word) | test(fits_in_the_niche)'`. `Resolved` stays 24 bytes, `Target` 16, `CacheEntry` 24, `GlobalEntry` 32, `Lookup` 24. If one grows, stop and report.
- **Every store into an existing object goes through `heap.rs`'s store accessors** (`CLAUDE.md`). The forward's rewrite is a collector-style rewrite, like a compaction's: it writes words without the barrier and keeps the remembered set itself (Task 3).
- **A handle means nothing across a move** (`CLAUDE.md`). The forward moves nothing, so no epoch changes, but after it a handle to a forwarded object names a forwarder: a debug build's `Heap::block` already refuses one (`heap.rs:2120-2131`). No Rust local may hold a handle to a forwarded object across the leaf; the run loop holds none (`vm.rs`'s `run_stretch` reads the receiver from the heap at each instruction), and the leaf's own arguments are two `Array`s and a block, none of which can be forwarded.
- **The VM mirror is a capability** (`CLAUDE.md`, the six locks). Nothing here gives a program the mirror. A test that hands it to a test program does so from the host, through `Interpreter::send`, as `builtins/reflection.rs`'s `leaf` helper does (`:413-427`).
- **`vm.rs` holds no dispatch logic** (`CLAUDE.md`). The slot-access check is a function in `interpreter/` (`interpreter/slots.rs` for the accessor, `interpreter/locals.rs` for the own-slot instructions) that `vm.rs` calls; the error text lives there once.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult`; tests use the local `expect_ok` helper of their file. A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- Comments explain the decision, not the mechanics; a comment that gives lazy forwarding as its reason is rewritten (Task 7).

## Review Focus

1. **A handle to a forwarded object read after the forward.** A host `Root` on an old object, a register of a running activation, a reified frame's receiver, a closure's copied value, a `WeakArray` slot and an `Ephemeron`'s key naming an old object: after the forward each names the new object, and a debug build's `Heap::block` never meets a forwarder. Pinned by Task 3's `the_forward_rewrites_every_holder_of_an_old_object` and Task 4's golden-style test run under every stress mode (`NS_NEXT_STRESS_MOVES`, `_COLLECT`, `_SCAVENGE`, `_CACHES`, `_WIDE`) by the end-of-batch `stress` gate.
2. **An old object that holds a young new one.** An old holder whose field the forward rewrites to a young object is remembered, or the next scavenge reclaims the young object under it: silent wrong output at exit status 0. Pinned by Task 3's `an_old_holder_rewritten_to_a_young_object_is_remembered`, which scavenges and reads through the holder, and by `NS_NEXT_STRESS_SCAVENGE`'s verification of the remembered set.
3. **A refused forward that changed something.** A split, a chain, an immediate, unequal sizes, a shipped instance (a `List`, the platform), a class object, an `Array`, a `String`, a closure, a reified `Mixin`: the leaf declines and every object, hash and cache entry is as before. Pinned by Task 4's `a_refused_forward_changes_nothing`.
4. **A slot site that sees several classes.** A private slot read in a superclass's method with receivers of three subclasses, and a `super` slot send from two subclasses: every answer is the slot's value, the cold check passing for each (the cache records the first receiver's guard). Pinned by Task 6's `a_private_slot_site_with_subclass_receivers_answers_each_slot`, and by every golden under both lowerings.
5. **A forwarded receiver in the middle of an initializer, a lazy slot's getter and a simultaneous clause.** `InitializeSlot`, `ReturnSlotIfFilled`, `LoadOwnSlot` and `InitializeSlotProvisionally` on a receiver forwarded to an instance of another class each raise the error of "The error texts"; forwarded to an instance of the same application they proceed. Pinned by Task 5's tests.

## The error texts

| Where | Text |
|---|---|
| An own-slot instruction whose receiver's chain no longer contains the level the activation was entered at (`interpreter/locals.rs`) | `<receiver> no longer has the slots of <class>, which the running code was compiled for` |
| A private or super slot accessor whose receiver's chain does not contain the level the slot was found at (`interpreter/slots.rs`) | `<receiver> no longer has the slots of <class>, which the slot accessor <selector> addresses` |

`<receiver>` is `Heap::describe` of the receiver; `<class>` is the name of the level's mixin (`Mixin::reflected_name`); `<selector>` the accessor's. Both are an ordinary `NewspeakError::error`, as `no_field_for_slot_accessor` (`interpreter/slots.rs:63-78`) builds one, so a program's `on: Error do:` catches them.

---

### Task 1: Every version of a mixin answers one id

**Files:**
- Modify: `crates/ns-runtime/src/heap/tables.rs` (`Tables`, `:57-64`, and the module doc `:1-6`)
- Modify: `crates/ns-runtime/src/heap.rs` (`mixin_id`, `:3224-3234`; `mixin`, `:3236`; a new `known_mixin_id`, `enclosing_mixin_id`, `mixin_module_text`, `replace_mixin`)
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinApplication::nested_slot`, `:945-958`, rekeyed; the doc comments of `Mixin::enclosing` and `Mixin::module_text`, `:680-695`, which now say the heap reads them once, at registration)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`apply_mixin`, `:1893-2039`: the key and the `nested_slot` insert at `:1922`; a new `Mutator::replace_mixin`)
- Modify: `crates/ns-runtime/src/interpreter/nested.rs` (`cache_slot`, `:461-480`), `interpreter/kernel.rs` (`:851-862`, `:2740-2743`), `interpreter/shipped_world.rs` (`index_by_mixin` `:202-212`, `declared_by_mixin` `:224-238`, `:424-428`, the tests at `:1175`, `:1206`, `:1245`)
- Modify: `crates/ns-runtime/src/builtins/reflection.rs` (the readers of `enclosing_mixin` and `module_text`, `:65`, `:73`, `:104`, `:151`, `:384-386`)
- Test: `crates/ns-runtime/src/interpreter/mod.rs`'s `mod tests` (or the neighbouring test module that already loads a module through `Interpreter::load_module`; copy its `expect_ok`)

**Interfaces:**
- Produces, on `Heap` (all `pub(crate)`):
  - `fn mixin_id(&mut self, mixin: &Rc<Mixin>) -> MixinId` -- unchanged signature; on first sight it now also registers the mixin's enclosing mixin first (`mixin.enclosing_mixin()`, recursively) and records, per id, the enclosing mixin's id and the mixin's `module_text`.
  - `fn known_mixin_id(&self, mixin: &Rc<Mixin>) -> Option<MixinId>` -- the same lookup without minting, for readers that hold `&Heap`.
  - `fn mixin(&self, id: MixinId) -> Option<Rc<Mixin>>` -- now answers the id's *current* version.
  - `fn enclosing_mixin_id(&self, id: MixinId) -> Option<MixinId>` and `fn mixin_module_text(&self, id: MixinId) -> Option<ModuleText>`.
  - `fn replace_mixin(&mut self, id: MixinId, replacement: Rc<Mixin>, module_text: Option<ModuleText>)` -- makes `replacement` the id's current version: `replacement`'s address answers `id` from then on, the previous version's address keeps answering `id` too (the previous `Rc` is kept in `Tables::superseded`, so that its address stays unique while the map names it), the enclosing id stays, the module text is the one given. Panics (a compiler-invariant violation, `CLAUDE.md`) when `replacement` already has an id, when the current version is `shipped`, or when one side is a class side and the other not.
- Produces, on `Mutator`: `pub(crate) fn replace_mixin(&mut self, id: MixinId, replacement: Rc<Mixin>, module_text: Option<ModuleText>)` -- `Heap::replace_mixin`, then evicts every `applications` entry whose key's mixin is `id` and the `metaclass_mixins` entry for `id`, so the next application of the id applies the replacement. Applications already built, and the class objects naming them, keep the version they were built from; 5b2's install rewrites those.
- Produces: `MixinApplication::nested_slot: IdentityMap<MixinId, usize>`.
- Produces: `apply_mixin` applies the id's current version (`heap.mixin(heap.mixin_id(mixin))`) whatever version it is handed, so a nested class created after a replacement is the replacement's, though the enclosing mixin's `nested` still holds the version it was built with.

- [ ] **Step 1: Write the failing tests:**
  - `every_version_of_a_mixin_answers_one_id`: load a module `Outer` declaring `public class Inner = () ( public answer = (^1) )`; reach `Inner`'s mixin (`heap.class_application(class)?.mixin` of an `Inner` class object, sent from the host); build a replacement with `Mixin`'s fields copied (`tests/image.rs`'s `mixin_of`, `:724`, is the model for a shallow copy; write a local helper) whose `answer` is another compiled method (compile one with `MixinMethod::from_declaration` over a parsed and resolved one-method module, as `tests/image.rs`'s `with_method_code`, `:750`, does); `mutator.replace_mixin(id, replacement, text)`; then `known_mixin_id` of the old and of the new `Rc` is `Some(id)`, `heap.mixin(id)` is the replacement (`Rc::ptr_eq`), `enclosing_mixin_id(id)` is `Outer`'s root's id, and the reified mixin object for `id` (`Mutator::reified_mixin`) is the same object before and after (`==` through the heap's identity key).
  - `a_nested_class_created_after_a_replacement_is_the_replacement`: same module with `public class Outer usingPlatform: platform = (...) ( public class Inner = () ( public answer = (^1) ) public fresh = (^Inner new answer) )` as a module whose root is instantiated twice: replace `Inner`'s mixin with one whose `answer` answers 2, then send `fresh` to a *second* `Outer` instance created after the replacement: it answers 2. The first instance, whose `Inner` class object was created and cached before, answers 1: its class object keeps the application it was built with (5b2's install updates it).
  - `a_replaced_mixin_reads_its_enclosing_mixin_and_text_through_the_id`: the mirror leaves `reflectionMixinEnclosing:ifFail:` and `reflectionMixinSource:ifFail:` on the reified mixin after the replacement answer `Outer`'s reified mixin and the replacement's text (call them as `builtins/reflection.rs`'s tests do).
  - `replacing_a_shipped_mixin_panics`: `#[should_panic(expected = "shipped")]`, replacing `List`'s mixin.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(answers_one_id) | test(after_a_replacement) | test(through_the_id) | test(replacing_a_shipped_mixin)'` -- compile errors (`replace_mixin`, `known_mixin_id` missing).
- [ ] **Step 3: The table.** `Tables` gains `superseded: Vec<Rc<Mixin>>` and, per id, `enclosing: Vec<Option<MixinId>>` and `module_texts: Vec<Option<ModuleText>>`, pushed beside `mixins` in `mixin_id`. Doc: the address map is sound because every version it names is held, the current one in `mixins` and the earlier ones in `superseded`, until 5b2's weak application cache decides what may be dropped. Implement `known_mixin_id`, `enclosing_mixin_id`, `mixin_module_text` and `replace_mixin` as in Interfaces; `mixin_id` registers the enclosing mixin before minting the child's id, so that the child's row can name it.
- [ ] **Step 4: Rekey `nested_slot`** by `MixinId`. `apply_mixin` (`:1922`) inserts `self.heap.mixin_id(nested)`; every reader asks `known_mixin_id(nested)` and treats `None` as the absent entry it treats today (`cache_slot` answers `None`, the kernel's and the shipped world's lookups their existing "absent" branches). `index_by_mixin` and `declared_by_mixin` are keyed by `MixinId` the same way, their tests rewritten to look up `known_mixin_id`. Update `nested_slot`'s doc: keyed by the declaration's id, which every version of it answers, and still not by its name.
- [ ] **Step 5: Apply the current version.** At the top of `apply_mixin`, after computing the id: `let mixin = &self.heap.mixin(id).expect(...)` -- the current version, which is `mixin` itself until a replacement. Add `Mutator::replace_mixin`.
- [ ] **Step 6: The leaves read through the id.** In `builtins/reflection.rs`, the enclosing mixin is `heap.enclosing_mixin_id(id)` then `heap.mixin`, and the text `heap.mixin_module_text(id)` (a class side's: its instance side's id's), where `id` is the reified mixin's own (`Heap::mixin_object_id`). `Mixin::enclosing` and `Mixin::module_text` stay as what a builder records; their doc comments now say the heap copies them into the id's row when it first registers the mixin, and that readers ask the heap.
- [ ] **Step 7: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 8: Commit** after `cargo xtask gates commit`: `feat: give every version of a mixin the one id it was registered under`.

### Task 2: A body says whether it addresses its own slots

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`CompiledCode`, `:397-441`; `from_builder`, `:506-525`; `CompiledCode::new`; a new `addresses_own_slots_of` beside `outer_reach_of`, `:843`)
- Modify: `crates/ns-runtime/src/bytecode/compact.rs` (`CodeBuilder`: the derivation beside its `outer_reach`)
- Modify: `crates/ns-runtime/src/shipped_code/encode.rs` (the destructuring at `:378-381`: `addresses_own_slots: _`, with the comment that the builder derives it)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`:1921-1927`, the comment naming what `from_builder` derives)

**Interfaces:**
- Produces: `pub addresses_own_slots: bool` on `CompiledCode` -- whether the body itself contains `InitializeSlot`, `InitializeSlotProvisionally`, `LoadOwnSlot` or `ReturnSlotIfFilled`. Derived by `from_builder` and `new`, never on the wire. `pub(crate) fn addresses_own_slots_of(instructions: &[Instruction]) -> bool`, an exhaustive `match` with no `_` and no `..` over the instruction kinds, as `outer_reach_of` is, so a new instruction must decide.

- [ ] **Step 1: Write the failing tests** in `bytecode.rs`'s `mod tests`: `a_body_with_an_own_slot_instruction_addresses_its_own_slots` (one `CompiledCode::new` per instruction of the four, each `true`) and `a_body_without_one_does_not` (`LoadSelf` and `Return` only: `false`); and in `compiler/lower.rs`'s tests: `an_initializer_and_a_lazy_getter_address_their_own_slots_and_a_method_does_not`: compile a class `| a = 1. lazy b = 2. | ( m = (^a) )` and check the initializer's code and `b`'s getter answer `true`, `m`'s `false` (the helpers beside `lower_kernel_method`, `:3799`).
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(own_slots)'`.
- [ ] **Step 3: Derive it** in `CodeBuilder` as it derives `outer_reach` (set when it emits one of the four), and in `addresses_own_slots_of` for the decoded view; `new` and `from_builder` fill the field; a test asserts the two agree for every body of the shipped world's compiled mixins (beside the round-trip tests' `assert_derived_as_decoded`, which already compares derived fields: extend it).
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 5: Commit** after `cargo xtask gates commit images` (the image decoder builds bodies through `from_builder`): `feat: derive whether a body addresses its own slots`.

### Task 3: The bulk forward in the heap

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`Relocation`, `:723-725`; `forward_everything`, `:5312-5368`; a new `Heap::forward_identity` and `IdentityForward`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (`split`, `:64`, made `pub(in crate::heap)`; a new `pub(crate) fn forward_identity`)
- Test: `crates/ns-runtime/src/heap.rs`'s `mod tests` (or `crates/ns-runtime/tests/heap.rs`, where the store-accessor tests are; follow the one that already builds instances, frames and roots by hand)

**Interfaces:**
- Produces, in `heap.rs`:
  - `pub(crate) enum ForwardRefusal { UnequalSizes, Split, Chain }` -- what the heap itself refuses; the leaf's lock (Task 4) refuses the rest before calling.
  - `Relocation` gains `fn holder_done(&self) -> bool { false }`, a default the compaction's `ForwardingTable` and the stress mode's `Shift` keep: after each object's block, `forward_everything` asks whether that block's rewrite named a young object, and remembers the block's object when it is old. Monomorphized, so the two existing relocations pay nothing.
  - `pub(crate) struct IdentityForward` -- an `IdentityMap<u64, NonZeroU64>` from each old object's offset to its new object's, and a `Cell<bool>` set by `apply` when the answer is young; `apply` answers the offset unchanged when it is not a key.
  - `pub(crate) fn forward_identity(&mut self, old: &[ObjectReference<'gc>], new: &[ObjectReference<'gc>]) -> Result<Forwarder<IdentityForward>, ForwardRefusal>` -- validates (sizes; an object named twice among `old` is a split; an object among `old` that is also among `new` is a chain; several old objects to one new one is a merge, accepted) before it writes anything; then, pair by pair in order, sets the new object's identity hash to the old one's (`object_set_identity_hash`'s write; for a merge the last pair's hash stands, as Primordial Soup's does) and turns the old object into a forwarder (`Header::with_format(Format::Forwarded)`, the guard `DispatchId::FORWARDED`, the class word the new object's offset, the word count kept so the space stays walkable); then `forward_everything` with a `Forwarder` whose `from_epoch` and `to_epoch` are both the current epoch; then remembers every holder it recorded. A forwarded object left in the remembered set stays there: a forwarder's block has no pointer ranges (`heap/object.rs:141-149`), so the next scavenge reads nothing through it.
- Produces, in `heap/world.rs`: `pub(crate) fn forward_identity<'gc>(mutator: &mut Mutator<'gc>, old: &[ObjectReference<'gc>], new: &[ObjectReference<'gc>]) -> Result<(), ForwardRefusal>` -- `Heap::forward_identity`, then the step roots through the answer (`split` and `StepRoots::trace`, as `forward_step_state` does), then `mutator.caches.invalidate()`. Callable inside a step.

- [ ] **Step 1: Write the failing tests:**
  - `the_forward_rewrites_every_holder_of_an_old_object`: allocate an old object A and a new object B of a fixed-pointers class; make A reachable from: a field of another instance, an `Array` element, a `WeakArray` slot, an `Ephemeron`'s key and a host `Root`; forward `[A]` to `[B]`; every holder now names B (`==` by `object_identity_key`), and `Root`'s value is B. Registers, a running activation's receiver and a closure's copied values are reached by a running program, so Task 4's and Task 5's tests pin them.
  - `the_new_object_takes_the_old_ones_identity_hash`: A hashed (`object_ensure_identity_hash`), B not; after the forward B's hash is A's. A merge `[A, C] -> [B, B]` leaves B with C's hash.
  - `a_split_a_chain_or_unequal_sizes_changes_nothing`: `[A, A] -> [B, D]` is `Split`, `[A, B] -> [B, D]` is `Chain`, `[A] -> [B, D]` is `UnequalSizes`; after each, A's format is still `FixedPointers`, every holder still names A, B's hash is unchanged.
  - `an_old_holder_rewritten_to_a_young_object_is_remembered`: an old holder H (allocate it, then `tenure` it the way the scavenger tests do), a young new object B with a field holding a distinct young object; forward H's field target A to B; run a scavenge (`scavenge_now`); H's field still reads B and B's field its object (a debug build's poison would otherwise panic in `block`).
  - `a_forwarder_is_garbage_after_the_forward`: after forwarding a young A and an old A2, a scavenge and a full collection both complete, `heap_statistics().live_objects` no longer counts them, and the move stress mode's `move_now` walks the space without panic.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(the_forward_rewrites) | test(identity_hash) | test(changes_nothing) | test(is_remembered) | test(garbage_after_the_forward)'`.
- [ ] **Step 3: Implement** as in Interfaces. In `forward_everything`'s object loop, after `trace_block`, `if forwarder.objects.holder_done() && !space::is_young(offset) { holders.push(offset) }`, and remember the holders after the loop, since `remember` needs the words the loop has borrowed. Write `forward_identity`'s doc comment: why eager (`object-model.md`, "Why eager"), why no epoch changes (nothing live moves), and that its one caller is the leaf.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS; then the size tests in release (Global Constraints).
- [ ] **Step 5: Commit** after `cargo xtask gates commit`: `feat: forward an array of objects to another in one walk`.

### Task 4: The `become:` leaf

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`ReflectionPrimitive`, `:826`, a variant `ForwardIdentity`; the exhaustive match at `:313-333`; the `MIRROR` row appended after `:2331-2334`)
- Modify: `crates/ns-runtime/src/builtins/reflection.rs` (`execute`, `:34`; a new `forward_identity` leaf function)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated, Global Constraints)
- Test: `crates/ns-runtime/src/builtins/reflection.rs`'s `mod tests`

**Interfaces:**
- Consumes: Task 3's `heap::world::forward_identity` and `ForwardRefusal`.
- Produces: the row `mirror("elementsOf:forwardIdentityToElementsOf:ifFail:", "forwardIdentity", Primitive::Reflection(ReflectionPrimitive::ForwardIdentity))`. It answers `nil` after the forward; it declines (the failure block runs, nothing changed) when either argument is not an `Array`, when the sizes differ, when an element of either is not an object whose format is `FixedPointers`, or whose class's application's mixin is `shipped` (an instance of a program's subclass of a shipped class is accepted: its class's own mixin is the program's), and on every `ForwardRefusal`. A reified `Mixin` is an instance of the kernel's `Mixin`, so it is refused as shipped. Its `answers_false_to_a_stranger` arm is `false`, as every reflection variant's is.

- [ ] **Step 1: Write the failing tests**, over a fixture module (the file's `a_fixture`, `:542`, is the model) declaring `public class Point = ( | public x ::= 1. public y ::= 2. | ) ( public sum = (^x + y) )`, `public class Wide = ( | public x ::= 10. public y ::= 20. public z ::= 30. | ) ( public sum = (^x + y + z) )` and a method `public swap: mirror = ( | point wide holder sums | point:: Point new. wide:: Wide new. holder:: {point}. sums:: Array new: 2. 1 to: 2 do: [:index | sums at: index put: point sum. index = 1 ifTrue: [mirror elementsOf: {point} forwardIdentityToElementsOf: {wide} ifFail: [^#declined]]]. ^{sums at: 1. sums at: 2. (holder at: 1) sum. point == wide} )` -- the mirror handed in from the host with `interpreter.send` as `leaf` does:
  - `the_leaf_forwards_a_program_instance`: `swap:` answers `{3. 60. 60. true}`: the method's register, the block's copy of it and the array element all name the `Wide` now, and the one site `point sum`, whose inline cache the first round filled for `Point`, runs `Wide`'s method in the second, because the caches were vacated. Under both lowerings.
  - `a_refused_forward_changes_nothing`: one host-side call each, every one answering `declined`, then checking the first argument's elements are unchanged by identity and `sum` still answers as before: unequal sizes; `{3}`; `{'text'}`; `{#(1 2)}`; a class object (`Point`); a closure (`[1]`); `platform collections List new`; the platform itself; a reified `Mixin` (`reflectionMixinOf:` of `Point`); a split `{p. p} -> {w. w2}`; a chain `{p. w} -> {w. w2}`; and the same with the refused object on the new side.
  - `a_program_subclass_of_a_shipped_class_is_forwarded`: a fixture `public class Failure = Error ( | public detail ::= 1. | ) ()` instance forwarded to another `Failure`: the leaf answers `nil`.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(the_leaf_forwards) | test(refused_forward) | test(subclass_of_a_shipped_class_is_forwarded)'`.
- [ ] **Step 3: Implement** the variant, the row, the leaf. The lock reads the class's application through `heap.class_application(heap.object_class(object))` and its `mixin.shipped`; a comment says why the leaf refuses what its one caller never asks for (`object-model.md`, "What the forward refuses": a defect in the install must not reach the kernel or the mirror).
- [ ] **Step 4: Regenerate the golden fixture** and compare with `cmp -l` (Global Constraints).
- [ ] **Step 5: Run** `cargo nextest run --workspace` -- PASS; the size tests in release.
- [ ] **Step 6: Commit** after `cargo xtask gates commit images`: `feat: reach the bulk forward through one mirror leaf`.

### Task 5: Own-slot instructions check a recorded guard

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`StackEntry`, `:404-430`, and its `Trace` impl, `:810`, which names every field; `push`, `:4280-4294`; new accessors `activation_recorded_guard`, `activation_entered_level`, `activation_record_guard`)
- Modify: `crates/ns-runtime/src/interpreter/locals.rs` (`filled_own_slot`, `:146`; `load_own_slot`, `:164`; a new `check_own_slots`)
- Modify: `crates/ns-runtime/src/vm.rs` (`initialize_slot`, `:1597`; `load_own_slot_into`, `:2035`; `initialize_slot_provisionally`, `:2047`; the compact arms at `:750`, `:786`, `:802-807` and the reference arms at `:1271`, `:1286`, `:1308`, `:1312`, whichever call the functions above)
- Test: `crates/ns-runtime/src/builtins/reflection.rs`'s `mod tests` (the mirror is handed in there already), or a new `mod tests` in `interpreter/locals.rs` copying that file's helpers

**Interfaces:**
- Consumes: Task 2's `CompiledCode::addresses_own_slots`; Task 4's leaf.
- Produces, on `StackEntry`: `recorded_guard: DispatchId` and `entered_level: ApplicationIndex`. `push` sets them only when `header.code.addresses_own_slots`: the receiver's `Heap::dispatch_id` and its `current_class`'s application index (`class_application_index`, or the accessor `class_application_entry` reads, `heap.rs:3340`); otherwise `DispatchId::NONE` and `ApplicationIndex::UNREGISTERED`. The `Trace` impl names both fields, as holding no handle, since it destructures every field.
- Produces, in `interpreter/locals.rs`: `pub(crate) fn check_own_slots<'gc>(mutator: &mut Mutator<'gc>, activation: ActivationReference) -> Result<(), Signal<'gc>>` -- `Ok` at once when the receiver's `dispatch_id` equals the recorded guard and the guard is not `NONE` (the hot path: one header load, one compare); otherwise the cold check, `#[inline(never)]`: walk the receiver's class's application and its `superclass` links for an application whose `index` is `entered_level`; found, it records the receiver's current guard (`activation_record_guard`) and answers `Ok`; not found, it answers the first error of "The error texts". An immediate receiver has no own slots and passes as today's code treats it (the instructions skip it).
- Every one of the four instructions calls `check_own_slots` before it reads or writes; `ReturnSlotIfFilled` and `LoadOwnSlot` propagate the error as `InitializeSlot` already does (`Result<_, Signal>`), changing their functions' return types where they answer plain values today. The reference lowering runs the same functions, so its activations take the same check; it caches nothing, so its hot path is the same compare.

- [ ] **Step 1: Write the failing tests**, the mirror handed in from the host, over a fixture declaring `public class Wide = ( | public x ::= 10. public y ::= 20. public z ::= 30. | ) ()` and:
  - `public class Shifting usingMirror: mirror into: target = ( | public a = mirror isNil ifFalse: [mirror elementsOf: {self} forwardIdentityToElementsOf: {target} ifFail: [nil]]. public b = 2. | ) ()` -- `an_initializer_whose_receiver_was_forwarded_raises`: `Shifting usingMirror: mirror into: Wide new` raises the own-slot error naming `Shifting`, caught by the host as an error result whose text is `a Wide no longer has the slots of Shifting, which the running code was compiled for` (`Heap::describe` of a `Wide` instance; check what it prints for an instance of a program class and use exactly that).
  - `public class Lazy = ( | public mirror ::= nil. lazy public late = (mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil]. 3). | ) ()` -- `a_lazy_getter_whose_receiver_was_forwarded_raises`: set `mirror`, send `late`: the getter's `InitializeSlot` raises the same error.
  - `a_simultaneous_clause_whose_receiver_was_forwarded_raises`: a class whose slots form a simultaneous clause (`goldens/language/slot/` has the shape; copy one) with the forward in the first initializer: the clause's `LoadOwnSlot` or `InitializeSlotProvisionally` raises.
  - `an_initializer_whose_receiver_was_forwarded_to_its_own_application_proceeds`: `Shifting usingMirror: mirror into: twin`, where `twin` is `Shifting usingMirror: nil into: nil` made beforehand (the `isNil` guard skips the forward): the instance's own guard and the twin's are one application's, so the hot path passes, the initializer completes, and the answer is `twin` itself (`==`), the forward having replaced the new instance everywhere, the initializer's own receiver register included.
  - All four under both lowerings: run each test's interpreter with `Lowering::Reference` too (the file's helper takes a lowering, or add the parameter).
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(whose_receiver_was_forwarded)'` -- the first three answer wrong values or succeed instead of raising.
- [ ] **Step 3: Implement** as in Interfaces; the error is built in `locals.rs` beside `check_own_slots`, the class name read from `heap.application(entered_level)`'s mixin.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS; the size tests in release.
- [ ] **Step 5: Measure** (the `measuring-performance` skill): `bench_send`, `bench_loop`, `bench_block` and the Are We Fast Yet `DeltaBlue` and `Richards` in release, against `master`, as the skill says to compare two trees. Record the numbers in the commit message. A slowdown past the skill's noise threshold on any of them is reported to the controller before committing, not committed silently.
- [ ] **Step 6: Commit** after `cargo xtask gates commit`: `feat: check a recorded guard before an own-slot access`.

### Task 6: Private and super slot accessors check a recorded guard

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs` (`Resolved::Slot`, `:41-45`; `act`'s `Resolved::Slot` arm, `:457-466`; the `resolve_*` that build `Resolved::Slot`, via `dispatch::found_entry`, `interpreter/dispatch.rs:350-367`)
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (`Target::Slot`, `:66-70`; the fill at `:639`; `materialize`, `:693`)
- Modify: `crates/ns-runtime/src/interpreter/slots.rs` (`decide_slot_accessor`, `:28-53`; a new `check_slot_level`)
- Test: `crates/ns-runtime/src/builtins/reflection.rs`'s `mod tests` (as Task 5)

**Interfaces:**
- Consumes: Task 4's leaf.
- Produces: `Resolved::Slot { kind, field, chain, level: ApplicationIndex, recorded_guard: DispatchId }` -- `level` the application of the class whose own slot the accessor is (the level `found_entry` found the entry at), `recorded_guard` the receiver's `dispatch_id` when it was resolved. `Target::Slot` carries the same two fields, `field` staying `u32`; the release size test proves `Target` stays 16 bytes and `Resolved` 24.
- Produces, in `slots.rs`: `fn check_slot_level(&mut self, receiver: Value<'gc>, level: ApplicationIndex, recorded_guard: DispatchId, selector: Symbol) -> Result<(), Signal<'gc>>` -- for `Chain::Private` and `Chain::Super` only, called by `act` before `decide_slot_accessor`: `Ok` when the receiver's `dispatch_id` equals `recorded_guard` (not `NONE`); otherwise the cold walk of Task 5 for `level`, answering the second error of "The error texts" when it is absent. A cache entry is not rewritten on a cold pass (an entry is shared by every receiver at the site, and rewriting it would make two receivers alternate). `Chain::Public` and `Chain::Protected` need no check: their sites guard on the receiver's own `dispatch_id` (`send_public`, and the non-private branch of `send_lexical`, `inline_cache.rs:758-850`), so a hit is already a match; a `Lexical` private site guards on N's application and a super site on `current_class`'s (`:842`, `:1002`), which is why those two need it.

- [ ] **Step 1: Write the failing tests**, the mirror handed in from the host:
  - `a_private_slot_read_after_its_receiver_was_forwarded_raises`: `public class Keeper = ( | private secret ::= 7. | ) ( public peek: mirror = ( secret. mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil]. ^secret ) )` -- the second `secret` raises the accessor error naming `Keeper` and `secret`; under both lowerings.
  - `a_super_slot_read_after_its_receiver_was_forwarded_raises`: `public class Base = ( | public value ::= 7. | ) ()` and `public class Derived = Base () ( public value = (^0) public peek: mirror = ( super value. mirror elementsOf: {self} forwardIdentityToElementsOf: {Wide new} ifFail: [nil]. ^super value ) )` -- raises; under both lowerings.
  - `a_private_slot_site_with_subclass_receivers_answers_each_slot`: `public class Holder = ( | private secret ::= 1. | ) ( public read = (^secret) public secret: value = (secret:: value) )` with three subclasses `One`, `Two`, `Three` (no slots of their own); one `read` site sent to an instance of each, three times round: every answer is that instance's `secret`; under both lowerings.
- [ ] **Step 2: Run them to see them fail**: `cargo nextest run -p ns-runtime -E 'test(after_its_receiver_was_forwarded) | test(subclass_receivers)'` -- the first two answer a field of the `Wide` instead of raising (or `no field`), the third passes already and must keep passing.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run** `cargo nextest run --workspace` -- PASS; the size tests in release.
- [ ] **Step 5: Measure** as in Task 5, Step 5, and report the same way.
- [ ] **Step 6: Commit** after `cargo xtask gates commit caches`: `feat: check a recorded guard before a private or super slot access`.

### Task 7: Comments that gave lazy forwarding as their reason

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`DispatchId::FORWARDED`, `:58-60`, and the `const _` comment, `:87-89`)
- Modify: `crates/ns-runtime/src/heap/scavenge.rs` (`evacuate`'s and `follow`'s doc comments, `:118-144`)

- [ ] **Step 1: Rewrite** `FORWARDED`'s doc: the guard the bulk forward stamps on each forwarder, never minted, so that a cache could never hit on one even if a reader met it -- and no reader does, since the forward is eager and leaves no handle naming a forwarder; the assertion's comment says it keeps the reserved value below every minted id. `evacuate`/`follow`: a chain of forwarders is the scavenger's own (a young object copied twice in one scavenge cannot happen, but `follow` costs nothing for one link), never a bulk forward's, which leaves none behind. No comment says "lazy".
- [ ] **Step 2: Check** `grep -rn -i "lazy forward\|road step 5" crates/ns-runtime/src` and rewrite each comment that still promises what this plan built or gives lazy forwarding as a reason.
- [ ] **Step 3: Run** `cargo nextest run --workspace` -- PASS.
- [ ] **Step 4: Commit** after `cargo xtask gates commit`: `docs: give the forwarder's guard and the scavenger's chains their present reasons`.

### Task 8: The design documents follow

**Files:**
- Modify: `docs/design/object-model.md` ("Planned: `become:` as an eager bulk forward" → "`become:` is an eager bulk forward", present tense, what is built: the leaf, its lock, its validation, the walk, the hash, the remembered holders, the vacated caches, no epoch change; what is still 5b2's: its caller. "Planned with road step 5: what a slot access checks" → "What a slot access checks", with the refinement: the entered level by its full-width index, recorded only for code that addresses its own slots, the cache entries' pair, the cold check's walk, the two error texts; "An application has two ids": recorded guards exist; the Decisions list: the four decisions of 2026-10-18 and the refinement)
- Modify: `docs/design/liveness-and-reflection.md` (status block; "Planned: road step 5b" split into what 5b1 built -- the id carries every version, `replace_mixin`, the forward -- and what "Planned: road step 5b2" still holds; "What the id has to carry": `Heap::mixin_id`, `nested_slot`, `ShippedWorld`'s two maps and the enclosing link and text are on the id; the encoder's row ids and `ShippedWorld::declared_by_mixin`'s image use remain for 5b2 if they remain)
- Modify: `docs/design/memory.md` ("Generations": the forward as the third writer of words without the barrier, which keeps the remembered set itself), `docs/design/dispatch.md` (the slot accessor's check on private and super sites), `docs/design/execution.md` (a stack entry's two new fields)
- Modify: `docs/OPEN-WORK.md` ("Next" becomes road step 5b2, with everything the step list's 5b entry says is still open, plus: whether an edit that changes only methods keeps the applications, so the cold check passes for activations entered before it, or builds new ones and widens the cold check to a layout comparison; the measured cost of Tasks 5 and 6), `docs/PROGRESS.md` (an entry for 5b1), `CLAUDE.md` if an invariant changed (the forward is a third word-writer beside the collector's rewrites: add it to "Every store into an existing object goes through `heap.rs`'s store accessors")
- [ ] **Step 1: Write them** in the present tense (`CLAUDE.md`, "Documentation other than a plan describes the current state").
- [ ] **Step 2: Commit**: `docs: record road step 5b1`.

---

## Left to 5b2

The MirrorBuilder and its protocol (both prior arts build from source: `asBuilder`, `methods addFromSource:`, slots through the header's source), compiling at run time (the resolver binds an implicit-receiver send's depth at compile time, `Binding::LexicalSend { depth }`, so adding or removing a slot or a method can change the bindings of unchanged methods and nested classes, which then need their source or AST -- absent for a mixin decoded from an image without source), the atomic install and the instance migration, the weak application cache and the ids' reuse rules, the image's edited mixins (the encoder's row ids still keyed by address, the old code a closure or frame still runs, the source of an edited mixin), and the forward leaf's Newspeak caller.
