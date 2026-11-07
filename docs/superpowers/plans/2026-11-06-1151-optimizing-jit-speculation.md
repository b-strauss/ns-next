# Speculation in the optimizing JIT (road step 9, stage 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optimized code that speculates: method bodies compiled per (body, receiver application, entered application), guards that trap at arithmetic, conditionals and sites never executed, the rules that keep a trap from looping, and the invalidation of the copies an edit, a bulk forward or a collection makes stale.

**Architecture:** A method body's budget, still one per body, requests a copy for the key of the frame whose spend ran it out; the copy is installed in the body's slot beside its baseline code and any other copies, a receiver-guarded cell names its copy's verified entry, a `super` or private lexical cell names a shared dispatching stub over the body's table of copies, and a push or the call stub looks the key up in Rust. In a copy the receiver's layout is fixed, so its own-slot reads are fixed offsets; in every optimized body a site the feedback says never ran, a `SmallInteger` operation whose fallback never ran and a conditional whose non-Boolean path never ran become guards whose failure writes the frame back and exits to run the instruction again. The exit counts the trap against the copy; a second trap at a site or the trap limit makes the copy not entrant and raises its version, and past the last version it is compiled without traps. Since every event that invalidates happens after an exit, and after an exit every frame goes on in baseline code, no frame is ever converted in this stage.

**Tech Stack:** Rust 1.98.0, `crates/ns-runtime/src/jit/` and `jit/optimizing/`, `interpreter/inline_cache.rs`, `ns-assembler` (x64, AArch64), `cargo-nextest`, `cargo xtask gates`.

**Spec:** `docs/design/execution.md`, "Planned: the optimizing JIT" -- "Customization", "Speculation and its failure", "Feedback", "Invalidation", "Verification", "Stages" item 2 -- and "The optimizing tier's foundation", which this stage builds on. The four decisions of 2026-11-06 are in those sections. Read them and "Planned: the deoptimization contract" before any task.

## Global Constraints

- Branch: `feature/optimizing-jit-speculation` (exists, from `master`); every commit on it.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`); every commit message ends with `Co-Authored-By: <the committing model's own name> <noreply@anthropic.com>` and `Claude-Session: https://claude.ai/code/session_01J94EWdTAqndqD5KeQGQLia`.
- Every command runs in the foreground; long commands pass `timeout: 600000`.
- Gates per commit: `cargo xtask gates commit`; plus `caches` when a commit touches `interpreter/inline_cache.rs`; `codegen` when it touches `vm.rs`, `interpreter/mod.rs`, `interpreter/sends.rs`, `heap.rs`, `heap/frames.rs` or `bytecode/compact.rs`; `release-checks` when it touches a type a size test pins. The end-of-batch gates run once, in Task 10: every commit before it is work in progress.
- Names are written out; clippy's `min_ident_chars` holds it.
- No file under `jit/` but `macro_assembler.rs` names an architecture's register or encoder outside a test module; a new register need is a named role.
- `ns-runtime` forbids `unsafe`. No new external dependency.
- Comments explain the decision, not the mechanics.
- A new golden's `.toml` is recorded with `NS_NEXT_NO_JIT=1`, from the interpreter, and its output established as right by reading the program.
- A leaf keeps its rule: none compiles or installs optimized code; the compile and install stay at the run loop's safepoint (`jit::optimize_requested`).
- Optimized code stays a lowering of the interpreter's instructions: every instruction it does not lower as a simple node it emits through the baseline's template function, after the write-back.
- A cell's entry address stays a function of its entry -- guard and target -- and the JIT's state (`jit::cell_entry`); a cell is written only from its entry.
- A trap counts against no exit budget; a forced exit of the deopt stress mode counts against neither.

## Decisions this plan makes inside the design

Each follows from the design and the code; each is stated here once.

1. **"Never executed" is a bit per site, not an epoch per family.** `InlineCaches` keeps an executed bit per site, set by every write of a non-vacant entry (`advance`, `fill`, the polymorphic and megamorphic promotions) and cleared only when `forget_families` drops the family. A site `invalidate` or `forget_freed` vacated keeps its bit, so it reads as executed -- a real send -- exactly as the design's epoch would make it, at no cost on any path but the slow one. `execution.md`'s "Feedback" is corrected in Task 10.
2. **A trap names a site.** A send site's trap is its own site; an `IntegerBinary`'s trap is its fallback send's site (the `target` the snapshot already reads, `fallback_taken`); a conditional's trap is the first send at its not-Boolean target (`Snapshot::fallback_site`). The site's executed bit decides the speculation and a trapped bit beside it, set by the exit that counts the trap, says it trapped before. The overflow's sticky bit is the fallback site's executed bit: an overflow with `SmallInteger` operands runs the fallback send.
3. **A trap is an exit that runs the instruction again** (`ExitKind::Trap`, beside `ExitKind::Reexecute`), its write-back on its cold path alone, as a poll's is, and the exit names itself as a trap so that the run loop counts it (`resumption_after_exit`), as `stubs::REFUSED_AT_THE_BOUND` names a refusal.
4. **Only method bodies are customized.** A block body has one optimized copy (decided with the user); an initializer's, a factory's and a superclass clause's body has one too, since its activation is entered by the initializer leaf or the run loop at a level the body's kind fixes, never through a receiver-guarded cell. The one copy lives where stage 1's does (`JitSlot::optimized`); a method body's copies in `JitSlot::copies`.
5. **What customization reads in this stage:** the own slots. `LoadOwnSlot` and `ReturnSlotIfFilled` become simple nodes in a copy, at the absolute index the entered application's `own_slot_base` gives, with no guard. `InitializeSlot` and `InitializeSlotProvisionally` keep their templates (an initializer runs once per instance), and the outer walk keeps its template, whose fixed structure pays with inlining.
6. **No table from dispatch id back to application yet**: the request carries the entered application it was made for, all the snapshot needs. The table is built with stage 3, whose snapshot reads callees' receivers; Task 10 records it in `OPEN-WORK.md`.
7. **Invalidation frees at once.** An install and a bulk forward run after an exit, and a full collection at a safepoint, so no return address into the zone exists: the copies are freed there, as `abandon_optimized` frees between entries, their cells written afresh and their dispatching rows cleared. No safepoint is requested for it.
8. **An invalidation is not a speculation's failure**: it raises no version and counts no trap; the body's budget is armed afresh at its version's wait, so that the next copy waits for new feedback.
9. **The limits start at Strongtalk's**: `TRAP_LIMIT` 5 traps per copy, `LAST_VERSION` 3, the wait `OPTIMIZATION_BUDGET × 4^version`; Task 9 measures them.

## Review Focus

1. **A mixin applied twice in one chain** runs one method body on one receiver at two levels: each level must run its own copy, with its own slot base, or a slot is read from the other level at exit status 0. Test: Task 2's `a_mixin_applied_twice_has_a_copy_per_level` and Task 8's `mixin_applied_twice_customized.ns`.
2. **A dispatch id reclaimed and minted again** for another application must never meet a copy made for the old one. Test: Task 7's `a_copy_keyed_on_a_reclaimed_id_is_freed`.
3. **A program whose types change for ever** -- a loop alternating `SmallInteger` and `Float` at one site, a conditional fed a non-Boolean every thousandth pass -- must settle: not entrant, recompiled with a slow path, at most `LAST_VERSION` recompiles, never a loop. Test: Task 6's `alternating_types_settle_within_the_last_version` and Task 8's `types_changing_in_a_loop.ns`.
4. **An edit installed while a copy runs below on the stack**: the frames go on in baseline code and see the new layout. Test: Task 8's `an_edit_while_an_optimized_frame_is_below.ns` under `NS_NEXT_STRESS_OPTIMIZE`.
5. **The frame at a trap is the interpreter's word for word**, the debug poison and the leftover analysis included, on both architectures. Test: Task 5's `a_trap_writes_back_every_dirty_binding`, the suite under `NS_NEXT_STRESS_DEOPT`, and the `aarch64` gate in Task 10.

---

### Task 1: The feedback's groundwork

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs` (the two bit sets beside `sites`, set in `advance`, `fill` and the promotions, cleared in `forget_families`), `crates/ns-runtime/src/jit/optimizing/snapshot.rs` (`site_executed`, `fallback_site`, the per-instruction speculation facts)
- Test: `crates/ns-runtime/src/interpreter/inline_cache.rs` tests, `crates/ns-runtime/src/jit/optimizing/snapshot.rs` tests

**Interfaces:**
- Produces:
  - `impl InlineCaches { pub(crate) fn site_executed(&self, feedback_base: FeedbackBase, site: u32) -> bool; pub(crate) fn site_trapped(&self, feedback_base: FeedbackBase, site: u32) -> bool; pub(crate) fn note_trap(&mut self, feedback_base: FeedbackBase, site: u32) -> bool /* whether it had trapped before */ }`
  - `pub(crate) struct SiteFacts { pub(crate) site: u32, pub(crate) executed: bool, pub(crate) trapped: bool }` and on `Snapshot`: `pub(crate) sites: Vec<Option<SiteFacts>>`, one per instruction: a send's own site, an `IntegerBinary`'s fallback site, a `JumpIfNotBoolean`'s fallback site, `None` for every other instruction. `fallback_taken` stays.
  - `fn Snapshot::fallback_site(instructions: &[Instruction], target: u32) -> Option<u32>`: the call site of the first send at or after `target` before any jump.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_site_filled_once_stays_executed_through_an_invalidation() {
    // fill a site, caches.invalidate(), then:
    assert!(caches.site_executed(base, site));
}

#[test]
fn a_forgotten_family_forgets_that_its_sites_ran() {
    // fill, then forget_families without the family's base:
    assert!(!caches.site_executed(base, site));
}

#[test]
fn a_second_trap_at_a_site_says_it_trapped_before() {
    assert!(!caches.note_trap(base, site));
    assert!(caches.note_trap(base, site));
}

#[test]
fn a_snapshot_names_the_site_each_speculation_rests_on() {
    // a body with SendPublic, IntegerBinary and an inlined ifTrue:ifFalse:
    assert_eq!(snapshot.sites[send_index].map(|facts| facts.site), Some(send_site));
    assert_eq!(snapshot.sites[integer_index].map(|facts| facts.site), Some(fallback_send_site));
    assert_eq!(snapshot.sites[jump_if_not_boolean_index].map(|facts| facts.site), Some(conditional_fallback_site));
    assert!(snapshot.sites[load_self_index].is_none());
}
```

- [ ] **Step 2: Run them to see them fail** -- `cargo nextest run -p ns-runtime -E 'test(/executed|trapped before|forgets that its sites|speculation rests/)'`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** -- two `Vec<u64>` bit sets indexed as `sites` is; grown with it.
- [ ] **Step 4: Run them to see them pass**, and `cargo nextest run -p ns-runtime --test inline_caches`. Expected: PASS.
- [ ] **Step 5: Commit** -- `cargo xtask gates commit caches`, then `git commit -m "feat: the caches record which sites ran and which trapped"`.

---

### Task 2: Customization: the key, the copies and their entries

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`JitSlot::copies`, `CustomizationKey`, `InstalledBody::key`, the request's key in `budget_spent`, `optimize`, `install_optimized`, `optimized_push_entry`, `entry_of_target`, `jit::called`'s lookup, `free_entered_code`, `free_optimized`), `crates/ns-runtime/src/interpreter/inline_cache.rs` and `interpreter/inline_cache/cells.rs` (every `jit::cell_entry(jit, &entry.target)` becomes `jit::cell_entry(jit, &entry)`; `refresh_cells_entering` unchanged in meaning), `crates/ns-runtime/src/heap.rs` (an accessor for the running machine frame's receiver dispatch id and current class's application, read by the budget leaf), `crates/ns-runtime/src/interpreter/mod.rs` (the push asks with the key)
- Test: `crates/ns-runtime/tests/jit.rs`, `jit/mod.rs` tests

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, PartialEq, Eq, Hash)] pub(crate) struct CustomizationKey { pub(crate) receiver: DispatchId, pub(crate) entered: DispatchId }` -- `entered` the instances id of the current class's application.
  - `pub(crate) struct CustomizedCopy { pub(crate) key: CustomizationKey, pub(crate) body: u32 /* index in bodies, NO_BODY once freed */, pub(crate) version: u8, pub(crate) traps: u8, pub(crate) refused: bool }`; `JitSlot { copies: RefCell<Vec<CustomizedCopy>>, .. }`. A copy's row stays when its code is freed, so that its version and refusal outlive it.
  - `JitState::copy_of(&self, code: &CompiledCode, key: CustomizationKey) -> Option<&InstalledBody>`.
  - `pub(crate) fn cell_entry(state: &JitState, entry: &CacheEntry) -> u64` and `expected_cell_entry` likewise: a method target behind a receiver guard names the copy for `(entry.guard, the entered application its level names)` when there is one, its baseline code otherwise; Task 3 adds the dispatching stub for the other guards.
  - The request: `JitState::optimization_requests: Vec<OptimizationRequest>`, `pub(crate) struct OptimizationRequest { code: Weak<CompiledCode>, key: Option<CustomizationKey>, entered: Weak<MixinApplication> }` (`None` for a body that is not customized).
- Consumes: nothing earlier.

- [ ] **Step 1: Write the failing tests** (`tests/jit.rs`, with `load_with`/`run` and `set_optimize_stress(true)`)

```rust
ns_runtime::for_each_lowering! {
    fn a_method_run_on_two_applications_has_two_copies(lowering) {
        // class A and class B <: A, a method of A summing a slot in a loop, run on an A and a B
        assert_eq!(run(&mut interpreter, &class), expected);
        assert_eq!(interpreter.jit_statistics().customized_copies, 2);
    }
}

ns_runtime::for_each_lowering! {
    fn a_mixin_applied_twice_has_a_copy_per_level(lowering) {
        // M applied twice in C's chain, M's method reading M's slot, sent at both levels (public and super)
        assert_eq!(run(&mut interpreter, &class), expected_both_levels);
        assert_eq!(interpreter.jit_statistics().customized_copies, 2);
    }
}

ns_runtime::for_each_lowering! {
    fn a_block_has_one_copy_whatever_its_receivers(lowering) {
        // one block body run by closures of an A and of a B
        assert_eq!(interpreter.jit_statistics().customized_copies, 0);
        assert!(interpreter.jit_statistics().optimized_bodies >= 1);
    }
}
```

`jit/mod.rs` tests: `a_receiver_guarded_cell_names_its_copy`, asserting that `cell_entry` of an entry guarded by A's id answers the start of A's copy and that of an entry guarded by B's id B's baseline code before B's copy exists.

- [ ] **Step 2: Run them to see them fail** -- `cargo nextest run -p ns-runtime -E 'test(/two_copies|copy_per_level|one_copy_whatever|names_its_copy/)'`. Expected: FAIL to compile (`customized_copies`).
- [ ] **Step 3: Implement.** The budget leaf reads its own frame's receiver and current class, which the call wrote, never another activation. A run-out whose key already has a copy or a refused row requests nothing; one for a key without a copy requests it, and the budget is armed again after the install, so that the next run-out by another key compiles that one. `--jit-statistics` prints `customized_copies`.
- [ ] **Step 4: Run them to see them pass**, then the suite under `NS_NEXT_STRESS_OPTIMIZE=1 cargo nextest run --workspace` (timeout 600000). Expected: PASS, every golden unchanged.
- [ ] **Step 5: Commit** -- `cargo xtask gates commit caches codegen`, then `git commit -m "feat: method bodies optimized per receiver and entered application"`.

---

### Task 3: The dispatching entry

**Files:**
- Create: `crates/ns-runtime/src/heap/optimized_copies.rs` (the table of copies, a reservation of its own, as `heap/machine_entries.rs`)
- Modify: `crates/ns-runtime/src/jit/targets.rs` (`assemble_dispatching_stub`), `crates/ns-runtime/src/jit/macro_assembler.rs` (the idioms it lacks: load the current class's application word from the callee's frame), `crates/ns-runtime/src/jit/mod.rs` (`entry_of_target` names the stub for a method target behind a `super` or private lexical guard once the body has a copy; the rows written at every install and cleared at every free, abandonment, compaction move and zone replacement), `crates/ns-runtime/src/jit/zone.rs` (the compaction remaps the rows' addresses)
- Test: `crates/ns-runtime/tests/jit.rs`, `jit/targets.rs` tests (cross-assembled on both architectures)

**Interfaces:**
- Produces:
  - A body's rows: up to `DISPATCHING_ROWS` (8) of `(receiver dispatch id, entered application word, entry address)` per code id, the table indexed by code id as the machine entry table is; a row is written only from the copy its key names and cleared wherever that stops naming machine code (CLAUDE.md's rule for the machine entry table, applied to this one). A copy past the rows is reached by receiver-guarded cells, the push and the call stub only.
  - `JitState::dispatching_stub_address(&self) -> u64`.
  - The stub: entered as a callee is, the receiver in `callee_receiver`, the current class written by the call; compares each row; on a match jumps into the copy with `jump_into_called`; on none into the body's baseline entry, read from the row block's last word, written afresh wherever the baseline body moves.
- Consumes: Task 2's `CustomizationKey`, `copies`.

- [ ] **Step 1: Write the failing tests**

```rust
ns_runtime::for_each_lowering! {
    fn a_private_method_reaches_each_receiver_s_copy(lowering) {
        // a private method of A sent lexically from a public one, run on an A and a B <: A
        assert_eq!(run(&mut interpreter, &class), expected);
        assert!(interpreter.jit_statistics().dispatched_calls > 0);
    }
}

ns_runtime::for_each_lowering! {
    fn a_super_send_reaches_its_copy(lowering) {
        assert_eq!(run(&mut interpreter, &class), expected);
    }
}
```

`jit/targets.rs`: `the_dispatching_stub_assembles_on_both_architectures` and `a_compaction_remaps_the_dispatching_rows` (`tests/jit.rs`, under `set_zone_stress`).

- [ ] **Step 2: Run them to see them fail.** Expected: FAIL (`dispatched_calls`).
- [ ] **Step 3: Implement.** `dispatched_calls` is counted on the stub's cold path only (no match), as the other statistics are counted off the hot path; the test asserts the hit through the copy's own statistics if that is the cheaper count.
- [ ] **Step 4: Run them to see them pass**, then `NS_NEXT_STRESS_ZONE=1` and `NS_NEXT_STRESS_OPTIMIZE=1` over `cargo nextest run --workspace` (each timeout 600000). Expected: PASS.
- [ ] **Step 5: Commit** -- `cargo xtask gates commit codegen`, then `git commit -m "feat: super and private lexical sends reach their copy through a dispatching entry"`.

---

### Task 4: Own slots at fixed offsets in a copy

**Files:**
- Modify: `crates/ns-runtime/src/jit/optimizing/snapshot.rs` (`Snapshot::customization: Option<Customization>`), `jit/optimizing/graph.rs` (`Node::OwnSlotLoad`, `End::ReturnUnlessNil`), `jit/optimizing/build.rs`, `liveness.rs`, `allocate.rs`, `verify.rs`, `metadata.rs`, `emit.rs` (their arms), `jit/macro_assembler.rs` (a field load at a fixed index off a value, if the idioms lack it)
- Test: `jit/optimizing/build.rs` and `emit.rs` tests, `tests/jit.rs`

**Interfaces:**
- Produces:
  - `pub(crate) struct Customization { pub(crate) key: CustomizationKey, pub(crate) own_slot_base: u32 }`, taken from the request's entered application.
  - `Node::OwnSlotLoad { value: ValueId, receiver: ValueId, absolute_index: u32 }` -- a simple node, no leaf call; `End::ReturnUnlessNil { value: ValueId, next: BlockId }` for `ReturnSlotIfFilled`.
  - Without a `Customization`, both instructions stay frame nodes, as in stage 1.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_copy_reads_an_own_slot_as_a_simple_node() {
    // build a body LoadOwnSlot 0 into r1; Return r1, with Customization { own_slot_base: 3, .. }
    assert!(graph_nodes.iter().any(|node| matches!(node, Node::OwnSlotLoad { absolute_index: 3, .. })));
    assert!(!graph_nodes.iter().any(|node| matches!(node, Node::Frame { .. })));
}

#[test]
fn a_shared_body_keeps_its_own_slot_template() {
    // the same body without a customization
    assert!(graph_nodes.iter().any(|node| matches!(node, Node::Frame { .. })));
}
```

`tests/jit.rs`: `a_lazy_slot_read_by_a_copy_answers_its_value_once_filled`.

- [ ] **Step 2: Run them to see them fail.** Expected: FAIL to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run them to see them pass**, then the suite under `NS_NEXT_STRESS_OPTIMIZE=1` and `NS_NEXT_STRESS_MOVE=1` together (timeout 600000). Expected: PASS.
- [ ] **Step 5: Commit** -- `cargo xtask gates commit`, then `git commit -m "feat: a copy reads its own slots at fixed offsets"`.

---

### Task 5: Guards and traps

**Files:**
- Modify: `crates/ns-runtime/src/jit/macro_assembler.rs` (`ExitKind::Trap`, `MacroAssembler::trap(program_counter: u32, site: u32) -> Label`), `jit/stubs.rs` (the exit names a trap and its site: `BlockWord::ExitTrapSite`), `jit/optimizing/graph.rs` (`ValueKind { Unknown, SmallInteger, Boolean }` per value; `Node::IntegerBinary`'s `fallback: Option<BlockId>`, `None` meaning a trap; `End::Branch`'s `not_boolean: Option<BlockId>`; `Node::Trap { program_counter: u32, site: u32 }` ending a block for a never-executed send), `build.rs` (the speculation from `Snapshot::sites`, the kinds propagated: an `IntegerBinary` arithmetic result a `SmallInteger` after its guard, a comparison's a `Boolean`; a known `SmallInteger` operand tests no tag, a known `Boolean` condition no third way), `liveness.rs`, `allocate.rs` (a trap is an interrupt point whose write-back is on its cold path, as a poll's), `metadata.rs` (`Resumes::Reexecute` at every trap; the allocation's verifier replays the cold write-back), `verify.rs` (a value of a known kind is defined by a node that guarantees it), `emit.rs` (the traps' cold paths beside `emit_polls`)
- Test: `jit/optimizing/build.rs`, `allocate.rs`, `emit.rs` tests, `tests/jit.rs`

**Interfaces:**
- Produces:
  - `pub(crate) enum Speculation { Traps, None }` on the compile (`optimizing::compile(&snapshot, macro_assembler, role, futures_exit, speculation)`): `None` past the last version, where every guard keeps stage 1's way (fallback send, third way, real send).
  - A site speculates only when its `SiteFacts` say `executed == false`; an `IntegerBinary` whose fallback site ran keeps stage 1's fallback, `fallback_taken` included.
- Consumes: Task 1's `SiteFacts`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn an_integer_operation_whose_fallback_never_ran_has_no_fallback_block() {
    assert!(matches!(integer_node, Node::IntegerBinary { fallback: None, .. }));
}

#[test]
fn a_result_known_to_be_a_small_integer_is_not_tested_again() {
    // r3 := r1 + r2; r4 := r3 < r5 -- the second operation tests r5's tag only
    assert_eq!(emitted_tag_tests(&code), 3);
}

#[test]
fn a_comparison_s_branch_has_no_third_way() {
    assert!(matches!(branch_end, End::Branch { not_boolean: None, .. }));
}

#[test]
fn a_trap_writes_back_every_dirty_binding() {
    // metadata::verify_allocation over every body of goldens/runtime/jit/ with Speculation::Traps
    // passes, and over a hand-written body with two dirty bindings before a trap it records both stores on the cold path
}
```

`tests/jit.rs`:

```rust
ns_runtime::for_each_lowering! {
    fn a_float_after_warm_up_traps_once_and_answers_right(lowering) {
        // a loop over SmallIntegers, optimized, then one pass with a Float
        assert_eq!(run(&mut interpreter, &class), expected_with_float);
        assert_eq!(interpreter.jit_statistics().traps, 1);
    }
}
```

and its neighbours for an overflow after warm-up and a non-Boolean after warm-up.

- [ ] **Step 2: Run them to see them fail.** Expected: FAIL to compile.
- [ ] **Step 3: Implement.** The trap's cold path writes back, then exits `Trap` at the program counter of the instruction it stands for; the run loop resumes the frame there in the interpreter, as a `Reexecute` exit's is resumed. Counting comes in Task 6: here a trap counts against nothing but `traps`.
- [ ] **Step 4: Run them to see them pass**, then the suite under `NS_NEXT_STRESS_OPTIMIZE=1` and under `NS_NEXT_STRESS_DEOPT=1` (each timeout 600000). Expected: PASS, every golden unchanged.
- [ ] **Step 5: Commit** -- `cargo xtask gates commit`, then `git commit -m "feat: optimized code guards arithmetic, conditionals and unrun sites and traps"`.

---

### Task 6: The rules against deoptimization loops

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`TRAP_LIMIT`, `LAST_VERSION`, `optimization_budget_after(budget, version)` replacing the doubling, `count_trap`, `make_not_entrant`, `abandon_optimized` raising the version, the refusal for good past the last version, `JitSlot::decay_exits` halving each copy's traps, `resumption_after_exit` dispatching a trap), `jit/optimizing/mod.rs` (`Speculation` chosen from the version)
- Test: `jit/mod.rs` tests, `tests/jit.rs`

**Interfaces:**
- Produces:
  - `const TRAP_LIMIT: u8 = 5; const LAST_VERSION: u8 = 3;`
  - `fn optimization_budget_after(budget: i64, version: u8) -> i64` -- `budget × 4^version`, saturating.
  - The rule: a trap sets its site's trapped bit (`InlineCaches::note_trap`); when the site had trapped before, or the copy's `traps` reach `TRAP_LIMIT`, the copy is made not entrant -- freed, its cells and dispatching rows written afresh -- its `version` raised and the body's budget armed at `optimization_budget_after(budget, version)`. Its recompile is compiled with `Speculation::Traps` up to `LAST_VERSION`, with `Speculation::None` past it. An abandonment by `EXIT_BUDGET` raises the version the same way; one past the last version sets the row's `refused`, and the key is never requested again. The uncustomized copy of a block or initializer keeps the same fields on its `JitSlot`.
- Consumes: Tasks 2 and 5.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_wait_grows_as_four_to_the_version() {
    assert_eq!(optimization_budget_after(240_000, 0), 240_000);
    assert_eq!(optimization_budget_after(240_000, 2), 3_840_000);
}

#[test]
fn a_decay_halves_a_copy_s_traps() { /* traps 4 -> 2 */ }
```

`tests/jit.rs`:

```rust
ns_runtime::for_each_lowering! {
    fn alternating_types_settle_within_the_last_version(lowering) {
        // one site fed SmallInteger and Float in turn for many budgets
        assert_eq!(run(&mut interpreter, &class), expected);
        let statistics = interpreter.jit_statistics();
        assert!(statistics.made_not_entrant <= u64::from(LAST_VERSION) + 1);
    }
}

ns_runtime::for_each_lowering! {
    fn a_copy_abandoned_past_its_last_version_stays_in_baseline_code(lowering) {
        // a body whose optimized code exits at a template every pass, with set_optimization_budget small
        assert_eq!(interpreter.jit_statistics().refused_for_good, 1);
    }
}
```

- [ ] **Step 2: Run them to see them fail.** Expected: FAIL to compile.
- [ ] **Step 3: Implement.** `--jit-statistics` prints `traps`, `made_not_entrant`, `refused_for_good`. Delete `MAXIMUM_REVIVAL_DOUBLINGS`'s use for optimized code (it stays for the baseline's revival).
- [ ] **Step 4: Run them to see them pass**, then the suite under `NS_NEXT_STRESS_OPTIMIZE=1` and `NS_NEXT_STRESS_DEOPT=1` (each timeout 600000). Expected: PASS.
- [ ] **Step 5: Commit** -- `cargo xtask gates commit`, then `git commit -m "feat: a trap or an abandonment raises a copy's version, and the last compiles without traps"`.

---

### Task 7: Invalidation and its stress mode

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`invalidate_applications`, `invalidate_all_optimized`, `forget_freed_copies`, `stress_invalidate_from_env`), `crates/ns-runtime/src/interpreter/install.rs` (the old applications' dispatch ids handed over after the commit), `crates/ns-runtime/src/builtins/reflection.rs` and `heap/world.rs` (the bulk forward invalidates all), `crates/ns-runtime/src/interpreter/reclaim.rs` (the freed guards), `crates/ns-runtime/src/interpreter/safepoint.rs` (the stress mode at every safepoint), `crates/ns-runtime/src/interpreter/mod.rs` (`set_invalidate_stress`, hidden), `xtask/src/main.rs` (`STRESS_MODES` gains `INVALIDATE`, twelve; its test renamed `the_stress_group_runs_twelve_modes`), `crates/ns-runtime/tests/jit.rs` (`a_stress_mode_polls_out`'s list), `crates/ns-cli/tests/cli.rs` (the `env_remove` lists)
- Test: `tests/jit.rs`, `xtask` tests

**Interfaces:**
- Produces:
  - `pub(crate) fn invalidate_applications(mutator: &mut Mutator<'_>, rebuilt: &[DispatchId])` -- frees every copy whose `receiver` or `entered` is among them; `pub(crate) fn invalidate_all_optimized(mutator: &mut Mutator<'_>)` -- every copy and every uncustomized optimized body; `pub(crate) fn forget_freed_copies(mutator: &mut Mutator<'_>, freed: &FreedIds)` -- the rows keyed on a freed guard are dropped with their code. None raises a version (decision 8).
  - `NS_NEXT_STRESS_INVALIDATE` (debug builds, implies `NS_NEXT_STRESS_OPTIMIZE`): `invalidate_all_optimized` at every safepoint, after `optimize_requested`'s installs of the previous one have run at least once.

- [ ] **Step 1: Write the failing tests**

```rust
ns_runtime::for_each_lowering! {
    fn an_edit_frees_the_copies_of_the_applications_it_rebuilds(lowering) {
        // optimize a method of A and one of an unrelated D, then install an edit adding a slot to A
        assert_eq!(statistics_after.invalidated_copies, 1);
        assert_eq!(run(&mut interpreter, &class), expected_after_edit);
    }
}

ns_runtime::for_each_lowering! {
    fn a_copy_keyed_on_a_reclaimed_id_is_freed(lowering) {
        // a class optimized, then dropped, collect_now, a new class minting the id again, run
        assert_eq!(run(&mut interpreter, &class), expected_of_the_new_class);
    }
}
```

`xtask`: `assert_eq!(STRESS_MODES.len(), 12)`.

- [ ] **Step 2: Run them to see them fail.** Expected: FAIL.
- [ ] **Step 3: Implement.** `--jit-statistics` prints `invalidated_copies`.
- [ ] **Step 4: Run them to see them pass**, then `NS_NEXT_STRESS_INVALIDATE=1 cargo nextest run --workspace` (timeout 600000). Expected: PASS, every golden unchanged.
- [ ] **Step 5: Commit** -- `cargo xtask gates commit codegen`, then `git commit -m "feat: an edit, a bulk forward and a reclaim invalidate the copies they make stale"`.

---

### Task 8: The speculation's goldens

**Files:**
- Create: under `goldens/runtime/optimizer/`: `types_changing_in_a_loop.ns` (a site fed `SmallInteger`, then `Float`, then `LargeInteger`, then both in turn), `overflow_after_warm_up.ns`, `non_boolean_after_warm_up.ns` (the `NonBooleanReceiver` error's text at the right pass), `unrun_branch_taken_late.ns`, `mixin_applied_twice_customized.ns` (one body run at both levels on one receiver, each reading its own slot), `private_method_on_two_receivers.ns`, `super_send_customized.ns`, `an_edit_while_an_optimized_frame_is_below.ns` (a MirrorBuilder edit adding a slot, installed from a callee of a hot optimized method whose loop then reads the receiver's slots), each with its `.toml` from `NS_NEXT_NO_JIT=1`
- Test: `crates/ns-cli/tests/run_goldens.rs` (already runs `goldens/runtime/optimizer/` under `NS_NEXT_STRESS_OPTIMIZE` as `golden::optimizer::<name>`)

- [ ] **Step 1: Write the goldens and their `.toml`s** (`NS_NEXT_NO_JIT=1 cargo run -p ns-cli -- run <file>`), each output established right by reading the program; formatted with `cargo run -p ns-analyzer -- fmt goldens/runtime/optimizer`, commented with `///`.
- [ ] **Step 2: Run them** -- `cargo nextest run -p ns-cli --test run_goldens -E 'test(/optimizer/)'`, then with `NS_NEXT_STRESS_INVALIDATE=1` and `NS_NEXT_STRESS_DEOPT=1`. Expected: PASS under both lowerings and every mode. A failure is a defect of Tasks 2-7, fixed at its cause.
- [ ] **Step 3: Commit** -- `cargo xtask gates commit`, then `git commit -m "test: the speculation's goldens"`.

---

### Task 9: Measurement

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs` (`TRAP_LIMIT`, `LAST_VERSION` once measured), `docs/design/execution.md` (a subsection "What speculation moved" beside "What the optimizing tier's foundation moved")

- [ ] **Step 1: Follow the `measuring-performance` skill's final measurement**: three pinned builds a side (`master`, this branch), the Are We Fast Yet programs and the micro sources, three rotating rounds; and per program `--jit-statistics`' `customized_copies`, `traps`, `made_not_entrant`, `refused_for_good`, `dispatched_calls`.
- [ ] **Step 2: Measure the limits** on the interim protocol: `TRAP_LIMIT` 3, 5, 10 and `LAST_VERSION` 2, 3, 4 on the programs that trap at all; keep Strongtalk's where nothing differs beyond the spread.
- [ ] **Step 3: Write the subsection**: per program the pooled change and the spread; DeltaBlueTon80's shared-stub samples measured again (OPEN-WORK, "Optimized code's sends mispredict in the shared stubs"); `bench_recursion.ns` against stage 1. A measured slowdown is diagnosed and fixed in this task, not reported.
- [ ] **Step 4: Commit** -- `cargo xtask gates commit`, then `git commit -m "perf: speculation measured"`.

---

### Task 10: Documents and the end of the batch

**Files:**
- Modify: `docs/design/execution.md` (a section "Speculation" after "The optimizing tier's foundation", in the present tense, for what is built: the key and the copies, the dispatching entry, the own slots, the guards and traps, the versions and the refusal for good, the invalidation; "Feedback" corrected for decision 1; "Planned: the optimizing JIT" keeps only what is not built; "Stages": stage 2 built), `docs/design/README.md` (the map's row; "Gates": the twelve modes, "the thirteen suites at once"), `docs/design/memory.md` (the stress modes' count and list), `docs/design/dispatch.md` (a cell's entry is a function of its guard and target), `CLAUDE.md` (the `ns-runtime` paragraph: the copies, `heap/optimized_copies.rs`, the dispatching stub; the `stress` row's count; "Machine code is a lowering…": a dispatching row is written only from the copy its key names and cleared wherever that stops; "Every slot access whose index was decided without looking at the receiver checks a recorded guard first": a copy's own-slot reads, whose receiver its entry verified and whose frame any forward ends), `README.md` (the count), `.claude/skills/finishing-a-batch/SKILL.md` (the modes), `docs/OPEN-WORK.md` ("Next": stage 3; step 9's entry: stage 2 built, what it left -- the dispatch-id-to-application table for stage 3, a copy past `DISPATCHING_ROWS`; the item "Abandonments halved at every collection can hold the back-off at 2x for ever" removed; "Optimized code's sends mispredict in the shared stubs" rewritten from Task 9's numbers), `docs/PROGRESS.md` (the batch's entry with its commits)

- [ ] **Step 1: Write the documents.**
- [ ] **Step 2: Run the end-of-batch gates** -- `cargo xtask gates end-of-batch` (timeout 600000 per group; the Docker groups last). Expected: every group passes; `.temp/stress-INVALIDATE.log`, `.temp/stress-OPTIMIZE.log` and `.temp/stress-DEOPT.log` show their summaries passing.
- [ ] **Step 3: Commit** -- `git commit -m "docs: speculation in the optimizing tier"`.
