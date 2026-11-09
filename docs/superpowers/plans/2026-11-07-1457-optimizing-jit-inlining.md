# Inlining in the optimizing JIT (road step 9, stage 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optimized code that inlines: sends whose receivers the caches recorded compiled into their caller as type-cases of inlined callees, blocks compiled with their home and never created where nothing lets them escape, the recompilee found by walking up the stack, and a frame that left its optimized code for a safepoint gone back into it.

**Architecture:** An inlined scope lies in its physical frame exactly where the interpreter would push its activation -- on its sender's argument slots, its thirteen header words between its arguments and its other registers -- so its registers are physical registers at a constant offset, and at every interrupt point every scope's words are the interpreter's. The code moves the frame register to the scope's register 0 around its frame nodes, so every baseline template runs unchanged; receiver and current class are header bindings written back like any dirty register; the exit's walk writes the other header words of each scope the interrupt point's metadata names, and allocates the blocks the compile eliminated, one object per block per exit, so that after any exit every scope is an interpreted activation going on in its own baseline code. A poll that finds a safepoint request leaves lightly -- headers written, blocks not allocated -- and the run loop re-enters the same code at that poll after the safepoint unless the safepoint made that impossible. The inlining plan is made on a tree of snapshots before the graph is built, ordered by estimated frequency and bounded by limits per callee and per compile.

**Tech Stack:** Rust 1.98.0, `crates/ns-runtime/src/jit/optimizing/`, `jit/materialize.rs`, `jit/mod.rs`, `vm.rs`, `heap/`, `ns-assembler` (x64, AArch64), `cargo-nextest`, `cargo xtask gates`.

**Spec:** `docs/design/execution.md`, "Planned: the optimizing JIT" -- "Inlining", "When a body is optimized" (the walk), "Feedback", "Customization", "Speculation and its failure" (the type guard), "Closures, frames and non-local return in inlined scopes", "Invalidation", "Verification", "Stages" item 3 -- and "Planned: the deoptimization contract", "The optimizing tier's foundation" and "Speculation", which this stage builds on. Read them before any task. The four decisions of 2026-11-07 are below; Task 11 writes them into those sections.

## Decisions of 2026-11-07 (with the user)

1. **An inlined scope lies at the interpreter's position.** Its register `r` is physical register `offset + r`, `offset` its sender scope's offset plus the send's `argument_base`; the header gap is part of the numbering, as it is of every frame. At every interrupt point every scope's words are the interpreter's; the exit writes each scope's header and nothing is rebuilt, copied or deoptimized lazily. The contract's "the conversion writes the k activations as suspended frames" and "an optimized frame below the top is deoptimized lazily" are replaced by this.
2. **A block that nothing lets escape is eliminated** -- created in the compile, every use an inlined `value`-family send or a move -- and materialized at an exit that finds it live: one closure per block per exit, its home reified and married in place, every register that holds it given the same object. Its copied values' source registers must stay unwritten while it lives, or it is not eliminated. A block that escapes is created by its `MakeClosure` as today, which leaves optimized code (the twin transfer in the outermost scope, an exit in an inlined one) until stage 4.
3. **Type-cases** take up to `POLYMORPHIC_WIDTH` (4) arms, one per cached type whose callee is inlined, in the cache's order; a type not inlined has no arm. The fall-through traps only where the site never trapped and every cached type has an arm, and is the site's own send through its cell otherwise; a megamorphic site gets no type-case. Each arm's callee is customized to its receiver's application.
4. **A frame that left optimized code at a poll goes back into it** after the safepoint, at that poll, unless the copy was freed, a frame among its scopes was reified, or the stack was rebuilt; then its eliminated blocks are materialized and every scope goes on in baseline code, as after any exit.

## Global Constraints

- Branch: `feature/optimizing-jit-inlining` (exists, from `master`); every commit on it.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`); every commit message ends with `Co-Authored-By: <the committing model's own name> <noreply@anthropic.com>` and `Claude-Session: https://claude.ai/code/session_01J94EWaXAruBrUzLnXPbNz8`.
- Every command runs in the foreground; long commands pass `timeout: 600000`.
- Gates per commit: `cargo xtask gates commit`; plus `caches` when a commit touches `interpreter/inline_cache.rs`; `codegen` when it touches `vm.rs`, `interpreter/mod.rs`, `interpreter/sends.rs`, `heap.rs`, `heap/frames.rs` or `bytecode/compact.rs`; `release-checks` when it touches a type a size test pins. The end-of-batch gates run once, in Task 11: every commit before it is work in progress.
- Names are written out; clippy's `min_ident_chars` holds it.
- No file under `jit/` but `macro_assembler.rs` names an architecture's register or encoder outside a test module; a new register need is a named role.
- `ns-runtime` forbids `unsafe`. No new external dependency.
- Comments explain the decision, not the mechanics.
- A new golden's `.toml` is recorded with `NS_NEXT_NO_JIT=1`, from the interpreter, and its output established as right by reading the program.
- Optimized code stays a lowering of the interpreter's instructions: every instruction it does not lower as a simple node it emits through the baseline's template function, after the write-back, with the frame register at its scope's register 0.
- No store is left out: before every interrupt point every dirty binding of every live scope is written back -- registers, and the receiver and current class of a scope with a frame node -- on the cold path for traps and polls, as today.
- A leaf keeps its rule: none compiles or installs optimized code; the compile and install stay at the run loop's safepoint. The exit's walk may allocate (an allocation never collects; it only requests a safepoint) and runs no Newspeak code.
- What is observed is never inlined: `on:do:`, `ensure:`, `ifCurtailed:`, a body with an unwind marker, and every kernel method whose activation a leaf reads or marks.
- A cell's entry address stays a function of its entry and the JIT's state; a cell is written only from its entry.

## Decisions this plan makes inside the design

1. **The frame register moves only around frame nodes** of an inlined scope: a simple node addresses a physical register as today, so a scope with no frame node costs nothing; the move is one `lea`-like add of the scope's offset in words, through a new idiom `MacroAssembler::shift_frame(words: i32)`, emitted before the first and after the last of a run of frame nodes of one scope.
2. **The receiver and current class of an inlined scope are two header bindings** (`HeaderBinding::Receiver`, `HeaderBinding::CurrentClass` of a scope), dirty from the scope's entry, written back before the scope's first frame node and, on the cold path, before a trap or poll inside it. The current class is computed at the arm as a compiled call computes it (`sends.rs`, from the chain's start and the target's level), only where a frame node of the scope needs it.
3. **Every other header word is the walk's**: code, program counter, caller, flags (counted, the kind's tag, argument count), feedback base, continuation (`Deliver`), guard and entered level as the push records them, register count and reified (nil, or the frame object a materialized block's home needed). The metadata of each interrupt point names its chain of scopes, innermost first; the install keeps it, per copy, in `OptimizedMetadata`, indexed by the interrupt point's return address or exit site.
4. **Depth and room**: the prologue compares the depth against the limit minus the copy's deepest inlined chain `D` and the stack's end against the frame's deepest extent; either refusal exits to baseline code, which pushes real frames and overflows exactly where the interpreter does. A real send `d` scopes deep adds `d` to the push's comparison and increment, as the contract says.
5. **Always inlined** in stage 3: slot accessors (a load, or a store through the barrier), the `MethodFastPath` kinds `Constant`, `Receiver`, `Parameter`, `BuiltinClass`, `ValueOfParameter` and `cull:`'s, a lazy slot's getter as a test and a load, `==`, `class`, and the `SmallInteger` mirror rows the graph already expresses (`IntegerBinary`'s operations), `Array`'s and `ByteArray`'s `at:` and `at:put:` with their bounds and kind checks as traps. `Float` arithmetic stays the leaf call it is: unboxed numbers are stage 5.
6. **The cost model starts at Strongtalk's** (`compiler/costModel.cpp:31-46`): an instruction 1, a send 5, a closure creation or non-local return 3, an accessor or fast-path send 2. The limits start at Strongtalk's (`runtime/debug.hpp:275-297`): a method 40, a block 70, plus 35 per block argument created in the compile, a compile 12,000 weighted instructions, recursion one level (`MaxRecursionUnroll` there is 2; the design says once), and the physical frame's registers within `u16::MAX`. Task 10 measures them.
7. **Frequency** is estimated from the site's loop depth (each enclosing loop ×8) times the callee's spent budget share (its row's `OPTIMIZATION_BUDGET - remaining`, clamped at 0, divided by `OPTIMIZATION_BUDGET`, at least 1/64); sites never executed are excluded; candidates are taken greedily in descending frequency while each is under its limit and the compile under its own.
8. **The table from dispatch id back to application** (`Heap::application_of_dispatch_id`) is a row per minted dispatch id in `heap/tables.rs`, written where an id is minted and cleared where one is reclaimed; it serves the snapshot of callees' receivers.
9. **A copy's dependencies** are recorded at install (`CopyDependencies`): every application whose layout, lookup or outer structure an arm or a customized read relied on, every inlined body's code id and feedback base. `invalidate_applications`, `forget_freed_copies` and every slide of a feedback base or freeing of a code id free the copies naming one, at once, as today.
10. **The recompilee walk runs at the exit the budget's cold path now takes** (`ExitKind::BudgetSpent`, counted against no budget), Strongtalk's `findTopInlinableFrame` (`recompiler/recompiler.cpp:56-121`): up at most 10 frames, from a block to its home and from a method with block arguments to their home (`senderOrParentOf`, `:199-237`), while the inlining plan would inline the frame's body into the caller's site, stopping at a caller running optimized code installed within the last `OPTIMIZATION_BUDGET` of its spend ("young") and at a caller whose own budget share is under 1/64.
11. **Exits inside inlined scopes** count against the physical copy's exit budget; traps count against the copy, at the inlined callee's site, and set that site's trapped bit, which every compile inlining the callee then reads.

## Review Focus

1. **The caller's registers an inlined callee overlays.** An inlined scope's words begin at the send's `argument_base`; the caller's bindings of those physical registers must be dead at the send (the leftover analysis taints them after it), and a value the caller holds there in a machine register must never be read as the callee's. Test: Task 3's `an_inlined_callee_overlays_the_caller_s_scratch_registers_only` and the suite under `NS_NEXT_STRESS_DEOPT`.
2. **Recursion near the depth limit**: an overflow inside inlined recursion is raised at exactly the interpreter's activation and text. Test: Task 5's `inlined_recursion_overflows_where_the_interpreter_does.ns`.
3. **An eliminated block materialized in two scopes** -- the home's register and the callee's parameter -- is one object, `==` to itself, its home married to the in-place activation. Test: Task 6's `an_eliminated_block_is_one_object_after_an_exit`.
4. **A poll re-entered after a compaction moved the copy, or after an invalidation freed it**: the first goes on in the moved code at the remapped poll, the second in baseline code with its blocks materialized. Test: Task 8's `a_poll_re_enters_moved_code` and `a_freed_copy_s_poll_deoptimizes`.
5. **An edit that changes an inlined callee's receiver layout while an optimized frame inlining it is below on the stack**: the copy is freed by its dependency and the frames go on in baseline code. Test: Task 9's `an_edit_of_an_inlined_callee_frees_the_copy` and Task 10's golden `an_edit_below_an_inlined_frame.ns`.

---

### Task 1: The application table and the snapshot tree

**Files:**
- Modify: `crates/ns-runtime/src/heap/tables.rs`, `crates/ns-runtime/src/heap.rs` (the accessor), `crates/ns-runtime/src/jit/optimizing/snapshot.rs`
- Create: `crates/ns-runtime/src/jit/optimizing/tree.rs`
- Test: `crates/ns-runtime/src/jit/optimizing/tree.rs` tests, `crates/ns-runtime/tests/jit.rs`

**Interfaces:**
- Produces:
  - `impl Heap { pub(crate) fn application_of_dispatch_id(&self, id: DispatchId) -> Option<ApplicationIndex> }`
  - `pub(crate) struct ReceiverFacts { pub(crate) dispatch_id: DispatchId, pub(crate) application: ApplicationIndex, pub(crate) own_slot_bases: Vec<u32> /* per level of the chain */ }`
  - `pub(crate) enum CalleeFacts { Method { snapshot: Box<Snapshot>, level: u16 }, Slot { field: u32, kind: AccessorKind, level: ApplicationIndex }, FastPath(MethodFastPath), Primitive(Primitive), Other }`
  - `pub(crate) struct SiteCandidates { pub(crate) site: u32, pub(crate) megamorphic: bool, pub(crate) arms: Vec<(ReceiverFacts, CalleeFacts)> /* cache order */ }`
  - `pub(crate) struct SnapshotTree { pub(crate) root: Snapshot, pub(crate) sites: HashMap<(CodeId, u32), SiteCandidates>, pub(crate) blocks: HashMap<CodeId, Snapshot> }`
  - `impl SnapshotTree { pub(crate) fn take(code: &Rc<CompiledCode>, heap: &mut Heap, caches: &InlineCaches, feedback_base: FeedbackBase, customization: Option<Customization>, weight_limit: u32) -> SnapshotTree }`: eagerly down to what `weight_limit` could take (callees whose own weight passes it are recorded as `Other`), each callee snapshotted against the feedback base its entry names, each block literal of a snapshotted body snapshotted once.

- [ ] **Step 1: Write the failing tests**: `a_minted_dispatch_id_answers_its_application`, `a_reclaimed_dispatch_id_answers_none` (in `crates/ns-runtime/tests/jit.rs`, through a full collection); in `tree.rs`: `a_monomorphic_site_s_callee_is_snapshotted_with_its_receiver` (`assert_eq!(candidates.arms.len(), 1)`, the callee's `code_id` the method's), `a_polymorphic_site_keeps_its_cache_order`, `a_megamorphic_site_has_no_arms`, `a_block_literal_is_snapshotted_once`, `a_callee_past_the_limit_is_other`.
- [ ] **Step 2: Run them**: `cargo nextest run -p ns-runtime tree:: a_minted_dispatch_id a_reclaimed_dispatch_id`; expected FAIL (not defined).
- [ ] **Step 3: Implement** the table (a `Vec<Option<ApplicationIndex>>` grown under the tables' growth limit, its committed size counted as theirs) and `SnapshotTree::take`, reading the cache entries' `Target`s (`interpreter/inline_cache.rs`) through a new reader `InlineCaches::entries_of(feedback_base, family_size, site) -> SiteEntries` that answers `Monomorphic(&CacheEntry)`, `Polymorphic(&[CacheEntry])`, `Megamorphic` or `Vacant`.
- [ ] **Step 4: Run them**: same command; expected PASS. Then `cargo xtask gates commit caches`.
- [ ] **Step 5: Commit**: `feat: the snapshot tree of a compile's callees and the table from dispatch id to application`.

### Task 2: The inlining plan

**Files:**
- Create: `crates/ns-runtime/src/jit/optimizing/plan.rs`
- Test: its tests

**Interfaces:**
- Consumes: Task 1's `SnapshotTree`.
- Produces:
  - `pub(crate) const COST_INSTRUCTION: u32 = 1; COST_SEND: u32 = 5; COST_CLOSURE: u32 = 3; COST_CHEAP_SEND: u32 = 2; METHOD_LIMIT: u32 = 40; BLOCK_LIMIT: u32 = 70; BLOCK_ARGUMENT_BONUS: u32 = 35; COMPILE_LIMIT: u32 = 12_000; RECURSION_LIMIT: u32 = 1; LOOP_WEIGHT: u32 = 8;`
  - `pub(crate) fn weight(snapshot: &Snapshot) -> u32`
  - `pub(crate) enum ArmPlan { Inline { callee: ScopeId }, Always(AlwaysInlined) }`, `pub(crate) enum AlwaysInlined { SlotLoad { field: u32 }, SlotStore { field: u32 }, Constant(QuickConstant), Receiver, Parameter(u8), BuiltinClass(BuiltinClass), LazyGetter { field: u32 }, Primitive(Primitive) }`
  - `pub(crate) struct SitePlan { pub(crate) arms: Vec<(DispatchId, ApplicationIndex, ArmPlan)>, pub(crate) fall_through: FallThrough }`, `pub(crate) enum FallThrough { Trap, Send }`
  - `pub(crate) struct ScopePlan { pub(crate) code_id: CodeId, pub(crate) parent: Option<(ScopeId, u32 /* call pc */)>, pub(crate) offset: u16, pub(crate) receiver: Option<ApplicationIndex>, pub(crate) block_of: Option<ScopeId> /* a block scope's home */ }`
  - `pub(crate) struct InliningPlan { pub(crate) scopes: Vec<ScopePlan> /* ScopeId 0 the root */, pub(crate) sites: HashMap<(ScopeId, u32), SitePlan>, pub(crate) blocks: HashMap<(ScopeId, u32 /* MakeClosure pc */), BlockPlan>, pub(crate) deepest_chain: u16, pub(crate) deepest_extent: u32 }`, `pub(crate) struct EliminatedId(pub(crate) u32)` (one per eliminated block of a compile; `BlockId` is the graph's basic block), `pub(crate) enum BlockPlan { Eliminated(EliminatedId), Created }`
  - `pub(crate) fn plan(tree: &SnapshotTree) -> InliningPlan`

- [ ] **Step 1: Write the failing tests**: `a_small_monomorphic_callee_is_inlined`, `a_callee_over_its_limit_is_sent`, `a_block_argument_raises_the_callee_s_limit`, `recursion_is_unrolled_once`, `an_observed_method_is_never_inlined` (an `ensure:` callee, a body with an unwind marker), `the_compile_limit_takes_the_most_frequent_first` (two sites, the inner loop's taken when only one fits), `a_site_that_trapped_falls_through_to_its_send`, `a_type_not_inlined_has_no_arm_and_its_site_falls_through_to_its_send`, `a_block_passed_to_an_inlined_callee_that_only_values_it_is_eliminated`, `a_block_stored_or_passed_on_is_created`, `a_block_whose_copied_source_is_written_while_it_lives_is_created`, `an_inlined_scope_lies_on_its_sender_s_argument_slots` (`assert_eq!(plan.scopes[1].offset, root_send_argument_base)`).
- [ ] **Step 2: Run them**: `cargo nextest run -p ns-runtime plan::`; expected FAIL.
- [ ] **Step 3: Implement** `weight` with the costs above and `plan` greedily by Decision 7's frequency; escape of a block is decided over every scope the plan inlines: a block is eliminated only if each use of its register, through moves and into inlined parameters, is a `value`-family send the plan inlines as that block's scope.
- [ ] **Step 4: Run them**: same command; expected PASS. `cargo xtask gates commit`.
- [ ] **Step 5: Commit**: `feat: the inlining plan over the snapshot tree`.

### Task 3: The builder's scopes: inlined methods, their returns and type-cases

**Files:**
- Modify: `crates/ns-runtime/src/jit/optimizing/build.rs`, `graph.rs`, `verify.rs`, `liveness.rs`, `allocate.rs`, `metadata.rs`
- Test: their tests

**Interfaces:**
- Consumes: Task 2's `InliningPlan`.
- Produces:
  - `pub(crate) fn build_inlined(tree: &SnapshotTree, plan: &InliningPlan, speculation: Speculation) -> Graph` (`build` stays, the plan with no inlined scope)
  - In `graph.rs`: `Node::Frame { scope: ScopeId, .. }` (each frame node names its scope); a scope's receiver and current class are bindings of the allocation like a register's, keyed `BindingPlace::Register(u16)` or `BindingPlace::Header(ScopeId, HeaderBinding)`, with `pub(crate) enum HeaderBinding { Receiver, CurrentClass }`; `End::TypeCase { program_counter: u32, scope: ScopeId, receiver: ValueId, arms: Vec<(DispatchId, BlockId)>, fall_through: Option<BlockId> /* None: trap */, site: u32 }`
  - In `metadata.rs`: `pub(crate) struct ScopeAtPoint { pub(crate) scope: ScopeId, pub(crate) code_id: CodeId, pub(crate) program_counter: u32, pub(crate) resumes: Resumes, pub(crate) offset: u16, pub(crate) locations: Vec<(u16, Location)> }` and `InterruptPoint { scopes: Vec<ScopeAtPoint> /* innermost first */ }`; `allocate::Location` gains `EliminatedBlock(EliminatedId)`.

- [ ] **Step 1: Write the failing tests**: `an_inlined_method_s_return_binds_the_send_s_destination`, `two_returns_of_an_inlined_method_meet_in_a_phi`, `an_inlined_callee_overlays_the_caller_s_scratch_registers_only` (the caller's binding of a register at or above the send's `argument_base` is unbound after the inlined call; a caller register below it keeps its value across), `a_type_case_tests_its_arms_in_the_cache_s_order`, `a_type_case_whose_site_never_trapped_traps_past_its_arms`, `a_type_case_after_a_trap_falls_through_to_the_send`, `an_inlined_scope_s_frame_node_names_its_scope`, `every_interrupt_point_names_its_chain_of_scopes`, `an_inlined_arm_reads_its_receiver_s_own_slot_at_a_fixed_offset`, and the sweeps `every_inlined_graph_verifies` and `every_inlined_allocation_verifies` over `corpus_of_bodies()` with plans built from synthetic monomorphic trees.
- [ ] **Step 2: Run them**: `cargo nextest run -p ns-runtime optimizing::`; expected the new ones FAIL.
- [ ] **Step 3: Implement**: the builder walks the root's instructions and, at a send the plan inlines, the callee's, with registers renumbered by the scope's offset (`physical = offset + register`), its parameters bound to the caller's argument values, its receiver to the send's receiver value, its `Return` a jump to a merge block after the send binding the send's destination; `verify_allocation` replays header bindings as it replays registers; the verifier checks every frame node's reads against its own scope's registers.
- [ ] **Step 4: Run them**: same command; expected PASS. `cargo xtask gates commit`.
- [ ] **Step 5: Commit**: `feat: inlined method scopes, their returns and type-cases in the optimizer's graph`.

### Task 4: Always-inlined callees

**Files:**
- Modify: `crates/ns-runtime/src/jit/optimizing/build.rs`, `graph.rs`, `emit.rs`
- Test: their tests, `crates/ns-runtime/tests/jit.rs`

**Interfaces:**
- Consumes: Task 2's `AlwaysInlined`, Task 3's type-case.
- Produces: simple nodes `Node::FieldLoad { value, object, field }`, `Node::FieldStore { object, field, source }` (emitted through `MacroAssembler::store_with_barrier`), `Node::IdentityCompare { value, left, right }`, `Node::ClassOf { value, object }` (a frame node where the class needs the heap's tables), `Node::IndexedLoad`/`IndexedStore { kind: IndexedKind::{Array, ByteArray}, .. }` whose bounds, kind and immutability checks trap at the send's site.

- [ ] **Step 1: Write the failing tests**: `an_accessor_arm_is_a_field_load`, `a_setter_arm_stores_through_the_barrier` (a young value into an old receiver is remembered), `a_quick_method_arm_answers_its_constant`, `a_lazy_getter_arm_tests_and_loads`, `an_array_at_put_out_of_bounds_traps_and_raises_the_interpreter_s_error`, `identity_compare_answers_a_boolean_of_known_kind`.
- [ ] **Step 2: Run them**; expected FAIL.
- [ ] **Step 3: Implement** the nodes in `build.rs` and their emission in `emit.rs`, each check a trap whose metadata re-executes the send.
- [ ] **Step 4: Run them**; expected PASS. `cargo xtask gates commit`.
- [ ] **Step 5: Commit**: `feat: accessors, quick methods and indexed access inlined as nodes`.

### Task 5: The emitter's scopes and the exit that materializes them

**Files:**
- Modify: `crates/ns-runtime/src/jit/optimizing/emit.rs`, `crates/ns-runtime/src/jit/macro_assembler.rs` (`shift_frame`), `crates/ns-runtime/src/jit/materialize.rs`, `crates/ns-runtime/src/jit/mod.rs` (`install_optimized` keeps the metadata), `crates/ns-runtime/src/jit/sends.rs` (the push's depth term)
- Create: `goldens/runtime/optimizer/inlined_recursion_overflows_where_the_interpreter_does.ns` and `.toml`
- Test: `crates/ns-runtime/tests/jit.rs`, the golden

**Interfaces:**
- Consumes: Task 3's `InterruptPoint::scopes`.
- Produces:
  - `pub(crate) fn shift_frame(&mut self, words: i32)` on `MacroAssembler`
  - `pub(crate) struct OptimizedMetadata { points: Vec<InterruptPoint>, by_address: HashMap<u32 /* offset in the copy */, usize> }` on each installed copy, moved with it by a compaction
  - `materialize_frame` and `materialize_innermost` write each scope's header from its `ScopeAtPoint`; `leaf_frame` answers the innermost scope's frame for a leaf called from an inlined scope.

- [ ] **Step 1: Write the failing tests**: `an_exit_inside_an_inlined_callee_leaves_two_interpreted_frames` (after a send to an uncompiled callee inside the inlined scope, `Heap::activation_depth` counts both scopes and each reads its own code, program counter and caller), `each_scope_goes_on_in_its_own_baseline_code_after_an_exit`, `a_leaf_called_from_an_inlined_scope_reads_that_scope`, `the_prologue_refuses_within_the_deepest_chain_of_the_limit`; the golden recurses through an inlined method to the depth limit and pins the interpreter's error text.
- [ ] **Step 2: Run them**: `cargo nextest run -p ns-runtime --test jit inlined` and `cargo nextest run golden::optimizer::inlined_recursion`; expected FAIL.
- [ ] **Step 3: Implement**: `optimizing::compile` builds with `build_inlined`; the emitter shifts the frame register around each scope's runs of frame nodes and writes back header bindings with registers; the install keeps `OptimizedMetadata`; the walk writes the headers; the prologue's comparisons take `deepest_chain` and `deepest_extent` (Decision 4).
- [ ] **Step 4: Run them**, then the whole suite under `NS_NEXT_STRESS_DEOPT=1` (`cargo nextest run --workspace`, `timeout: 600000`); expected PASS. `cargo xtask gates commit codegen`.
- [ ] **Step 5: Commit**: `feat: inlined scopes in optimized code, materialized by the exit in place`.

### Task 6: Blocks with their home, eliminated blocks and the non-local return

**Files:**
- Modify: `crates/ns-runtime/src/jit/optimizing/build.rs`, `graph.rs`, `metadata.rs`, `emit.rs`, `crates/ns-runtime/src/jit/materialize.rs`, `crates/ns-runtime/src/heap.rs` (an accessor that reifies a frame at an offset and builds a closure over it, if `Heap::reify` and the closure's allocation do not serve)
- Create: goldens `goldens/runtime/optimizer/inlined_to_do_writes_its_home_s_local.ns`, `inlined_block_returns_from_its_home.ns`, `eliminated_block_seen_after_an_exit.ns` with their `.toml`
- Test: `crates/ns-runtime/tests/jit.rs`, the goldens

**Interfaces:**
- Consumes: Task 2's `BlockPlan`, Task 5's walk.
- Produces: `Node::EliminatedBlock { value: ValueId, block: EliminatedId }` (a value no machine code reads; its frame word holds `nil` until an exit materializes it); `LoadOuterLocal`/`StoreOuterLocal` of a block scope whose home is in the compile become reads and writes of the home's physical register; a `NonLocalReturn` whose home is in the compile becomes a jump to the home's return; `materialize::materialize_blocks(mutator, points) -> ()` allocating one closure per `EliminatedId` live at the point, the home reified (`Heap::reify`) and every `Location::EliminatedBlock(id)` register written with the same handle.

- [ ] **Step 1: Write the failing tests**: `a_to_do_loop_inlined_with_its_block_creates_no_closure` (no closure allocated over 1,000 passes, counted by `JitStatistics::materialized_blocks` staying 0 and `--jit-statistics`' allocations), `an_eliminated_block_is_one_object_after_an_exit` (the home's register and the callee's parameter answer the same handle), `the_home_of_a_materialized_block_goes_on_in_its_twin`, `a_non_local_return_from_an_inlined_block_jumps_to_its_home_s_return`, `a_block_that_escapes_still_leaves_optimized_code`; the goldens pin the outcomes under both lowerings and the optimize stress mode.
- [ ] **Step 2: Run them**; expected FAIL.
- [ ] **Step 3: Implement** in the builder, the metadata and the walk.
- [ ] **Step 4: Run them**, then the suite under `NS_NEXT_STRESS_DEOPT=1`; expected PASS. `cargo xtask gates commit`.
- [ ] **Step 5: Commit**: `feat: blocks compiled with their home and eliminated where nothing lets them escape`.

### Task 7: The recompilee walk

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`budget_spent`, `request_copy`, `resumption_after_exit`), `crates/ns-runtime/src/jit/stubs.rs` (`ExitKind::BudgetSpent`), `crates/ns-runtime/src/jit/leaves.rs`
- Test: `crates/ns-runtime/tests/jit.rs`

**Interfaces:**
- Consumes: Task 2's `plan` (asked whether a frame's body would be inlined at its caller's site).
- Produces: `pub(crate) const RECOMPILEE_SEARCH_LENGTH: usize = 10;` `fn find_recompilee(mutator: &mut Mutator<'_>, triggering: ActivationReference) -> ActivationReference`, called by `resumption_after_exit` for `ExitKind::BudgetSpent`, whose request is made for the frame it answers; the exit counts against no budget.

- [ ] **Step 1: Write the failing tests**: `a_small_hot_callee_requests_its_caller_s_optimization`, `a_block_s_run_out_requests_its_home`, `a_method_taking_a_block_requests_the_block_s_home`, `the_walk_stops_at_young_optimized_code`, `the_walk_stops_after_ten_frames`, `a_budget_exit_counts_against_no_budget`.
- [ ] **Step 2: Run them**; expected FAIL.
- [ ] **Step 3: Implement** Decision 10.
- [ ] **Step 4: Run them**; expected PASS. `cargo xtask gates commit`.
- [ ] **Step 5: Commit**: `feat: the recompilee found by walking up the stack`.

### Task 8: Back into optimized code at a poll

**Files:**
- Modify: `crates/ns-runtime/src/jit/optimizing/emit.rs` (an entry per poll), `crates/ns-runtime/src/jit/materialize.rs` (the light exit), `crates/ns-runtime/src/vm.rs` (`after_safepoint`), `crates/ns-runtime/src/jit/mod.rs` (`resumed_at_poll`), `crates/ns-runtime/src/jit/zone.rs` (the poll entries remapped)
- Test: `crates/ns-runtime/tests/jit.rs`

**Interfaces:**
- Consumes: Task 5's metadata, Task 6's `materialize_blocks`.
- Produces: `pub(crate) struct StoppedAtPoll { physical: ActivationReference, code_id: CodeId, key: Option<CustomizationKey>, point: usize, scopes: u16 }` (the copy named by its body and key, never by an address, so that a compaction or a freeing between is found), recorded by the poll's light exit; `pub(crate) fn resumed_at_poll(mutator: &mut Mutator<'_>, stopped: StoppedAtPoll) -> Option<MachineEntry>`, which takes the k-1 inner scopes out of the stack's bookkeeping (innermost, live count, depth count) and answers the poll's entry, or materializes the blocks and answers `None`; `JitStatistics::{poll_reentries, poll_deoptimizations}`.

- [ ] **Step 1: Write the failing tests**: `a_scavenge_in_an_inlined_loop_goes_back_into_its_optimized_code` (`poll_reentries` > 0 and the loop's activation never runs baseline code after it), `a_poll_re_enters_moved_code` (under the zone stress mode's sparing compaction), `a_freed_copy_s_poll_deoptimizes` (under `NS_NEXT_STRESS_INVALIDATE`), `a_reified_scope_s_poll_deoptimizes`, `the_suspend_stress_mode_deoptimizes_at_a_poll`.
- [ ] **Step 2: Run them**; expected FAIL.
- [ ] **Step 3: Implement**: the poll's entry reloads every live value from its frame word and sets the frame register to the poll's scope; `vm::after_safepoint` asks `resumed_at_poll` before `back_edge_ready`.
- [ ] **Step 4: Run them**, then the suite under `NS_NEXT_STRESS_SCAVENGE=1` and `NS_NEXT_STRESS_ZONE=1`; expected PASS. `cargo xtask gates commit codegen`.
- [ ] **Step 5: Commit**: `feat: a frame goes back into its optimized code at the poll it left`.

### Task 9: The copies' dependencies

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`CopyDependencies`, `invalidate_applications`, `forget_freed_copies`, the slide and the freeing of a code id)
- Test: `crates/ns-runtime/tests/jit.rs`

**Interfaces:**
- Consumes: Task 2's plan (its applications, code ids, feedback bases).
- Produces: `pub(crate) struct CopyDependencies { applications: Vec<DispatchId>, code_ids: Vec<CodeId>, feedback_bases: Vec<FeedbackBase> }` on each copy.

- [ ] **Step 1: Write the failing tests**: `an_edit_of_an_inlined_callee_frees_the_copy`, `a_reclaimed_inlined_code_id_frees_the_copy`, `a_slide_of_an_inlined_callee_s_feedback_base_frees_the_copy`, `an_edit_of_an_unrelated_class_keeps_the_copy`.
- [ ] **Step 2: Run them**; expected FAIL.
- [ ] **Step 3: Implement** Decision 9.
- [ ] **Step 4: Run them**; expected PASS. `cargo xtask gates commit`.
- [ ] **Step 5: Commit**: `feat: a copy freed when anything it inlined changes`.

### Task 10: Goldens, statistics and measurement

**Files:**
- Create: under `goldens/runtime/optimizer/`: `type_case_widens_after_warm_up.ns`, `inlined_callee_exits_to_an_uncompiled_callee.ns`, `mixin_applied_twice_inlined.ns`, `an_edit_below_an_inlined_frame.ns`, `allocating_inlined_loop.ns`, each with its `.toml`
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`JitStatistics`: inlined sites, type-case arms, eliminated and materialized blocks, budget exits, poll re-entries and deoptimizations), `crates/ns-cli` (`--jit-statistics` prints them)
- Test: the goldens, `crates/ns-cli` tests of the printout

- [ ] **Step 1: Write the goldens and record their `.toml`s with `NS_NEXT_NO_JIT=1`**, each output checked by reading the program.
- [ ] **Step 2: Run them**: `cargo nextest run golden::optimizer`; expected PASS under both lowerings and the optimize stress mode.
- [ ] **Step 3: Measure** with the `measuring-performance` skill: the twenty-seven sources against `master`, pinned builds, the performance cores; then the limits of Decision 6 (each halved and doubled on the geomean), and the shared stubs' mispredictions behind optimized callers (`OPEN-WORK.md`, "Optimized code's sends mispredict in the shared stubs"). A regression found is diagnosed and fixed before Task 11.
- [ ] **Step 4: Record** the results in `docs/design/execution.md`, a new "What inlining moved" under "Measurements".
- [ ] **Step 5: Commit**: `test: inlining's goldens and statistics` and `docs: what inlining moved`.

### Task 11: Documents and the end of the batch

**Files:**
- Modify: `docs/design/execution.md` (a built section "Inlining" after "Speculation"; "Planned: the optimizing JIT" and "Planned: the deoptimization contract" rewritten to say only what is not built, Decisions 1 to 4 of 2026-11-07 stated there), `docs/OPEN-WORK.md` (Next: stage 4; what this stage left), `docs/PROGRESS.md`, `CLAUDE.md` (the crate map's optimizing JIT: its third stage), `docs/design/README.md` if its map changes

- [ ] **Step 1: Update the documents** to the current state, removing the items this stage closed (the dispatch-id table, the mispredictions if measured away).
- [ ] **Step 2: Run** `cargo xtask gates end-of-batch` (`timeout: 600000`, repeated per group if it outlives it); expected every group PASS. A failure is fixed and the group rerun.
- [ ] **Step 3: Commit**: `docs: inlining, stage 3 of the optimizing JIT`.
- [ ] **Step 4: Finish** with the `finishing-a-batch` skill.
