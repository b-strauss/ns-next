# Half-built values Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An instance of a value class is no value object until its factory has returned: until then `=`, `hash`, `==`, `identityHash` and `objectIsValue:` signal, and an image carries the mark.

**Architecture:** A header bit, "under construction", is part of the instance header every application of a class that inherits `Value` precomputes, so the interpreter's and machine code's allocation set it without a new test. A new instruction, `CompleteInstance`, between `InitializeInstance` and `Return` of the synthesized factory body, clears it. A construction cut short by an unwind or an uncaught signal leaves it set for good. The value leaves and the weak `IsValue` leaf refuse a marked object; the image writes one under a new object-body tag, 17.

**Tech Stack:** Rust 1.100 (edition 2024), `cargo-nextest`, Newspeak goldens, mdBook spec.

**Spec:** the decisions below, made with the user on 2026-11-28; Task 2 writes them into `docs/design/object-model.md`, "A value object's four conditions", which is the living spec from then on. Background: `.temp/comparison-newspeak-docs.md`, section 12, item 1.

## Decisions (made with the user, 2026-11-28)

1. **D, the under-construction mark.** §3.1.1 makes an object a value only when its slots "have been initialized"; Bracha's formal semantics defines value status as a greatest fixed point over initialized slots (`prior-art/newspeak/formal-semantics/newspeak-semantics.tex`, around line 2385) and the actor transfer "only for a fully initialized value" (`actor-semantics.tex:41`). An instance of a value class is marked at its allocation and unmarked when its factory returns normally; while marked it is no value. Storing it into a value's slot and using it as an enclosing object stay allowed (the module pattern `| origin = Point x: 0 |` and cycles between values, which §3.1.1's coinductive phrasing admits). `=`, `hash` and through them `==` and `identityHash` signal on a marked object.
2. **`objectIsValue:` signals** on a marked object, as `=` and `hash` do, so that `Ephemeron>>key:` and `WeakMap>>at:put:` need no check of their own.
3. **The image writes a marked instance as object-body tag 17, `instance-under-construction`**, laid out as `instance` (tag 0); the decoder refuses it for a class that does not inherit `Value`.

Proposed in this plan, not decided by the user (say so when reporting): the instruction `CompleteInstance` and its wire tag 52; the decoder accepting `CompleteInstance` in a decoded body as it accepts `AllocateInstance`; the error text; what `Value>>=` checks (receiver, argument, every object its walk reaches).

The probes the goldens start from (`identity.ns`, `escape.ns`, `cycle.ns`, `module.ns`, `nested.ns`) are in `C:\Users\Benjamin\AppData\Local\Temp\claude\a470a51e0d4364df\fb3eaa88-6c13-40d3-8e0d-84f21541b3a7\scratchpad\values\`; each golden is written afresh in the repository's layout and `///` comments.

## Global Constraints

- Nothing is copied from `prior-art/`: SOMns, Eurasia, Eastasia and Primordial Soup were read for ideas only (licensing; CLAUDE.md, "Copying code out of `prior-art/` is a licensing decision").
- Branch `feature/half-built-values`, created before the first commit.
- Names are written out (CLAUDE.md, Conventions); conventional commits; every commit ends with the session's `Co-Authored-By` and `Claude-Session` lines.
- A new golden's `.toml` is recorded with `NS_NEXT_NO_JIT=1`, and names its rules (`rules = [...]`); a Rust test names its rules in a `// spec:` line.
- A commit that changes behaviour, a format or an invariant changes its rule in `docs/spec/` in the same commit.
- A wire tag is assigned once (`image/opcodes.rs`, `tests/opcode_tags.rs`).
- An edited comment in `Kernel.ns` changes the fingerprint: `regenerate_the_golden_fixture` (`tests/image.rs`) rewrites `tests/fixtures/golden-image.bin` in the same commit. `Kernel.ns` is LF: edit it with Edit, never Python text mode.
- After editing a `.ns`: `cargo run -p ns-analyzer -- fmt <files>`.
- Every commit runs `cargo xtask gates commit`. Tasks 1 and 3 also run `images` (bytecode and image format), Task 1 `codegen` (it touches `bytecode/compact.rs`, `heap.rs`). The batch's end runs `end-of-batch` and `plugin-verification` is not needed (no plugin change). A commit before the end-of-batch gates is work in progress.
- The error text, for every refusal (proposed), naming the class only, never the half-built state: `an instance of Point is still under construction: an instance of a value class is a value only once its factory has returned`, signalled as an `Error` under the selector that was sent (`=`, `hash`, `isValue` …).

## Review Focus

1. **Machine code's allocation and completion.** A value class instantiated in a hot loop runs `AllocateInstance` and `CompleteInstance` as baseline templates and inside optimized code; a completed value must compare and hash as before. Test: golden `values_made_in_a_hot_loop_compare_as_values.ns` (Task 1).
2. **A factory nested in a factory.** A value's initializer instantiates its own class (a list of values built recursively); each `CompleteInstance` clears its own instance only. Test: golden `a_value_built_inside_another_ones_initializer_completes_on_its_own.ns` (Task 2).
3. **A signal resumed inside the initializer.** `resume:` lets the construction finish, and the instance is then a value. Test: golden `a_construction_resumed_after_a_signal_completes.ns` (Task 2).
4. **The hot-swap migration** rebuilds instances in a new layout (`Mutator::migrate`, `interpreter/install.rs:913`): a marked instance stays marked, an unmarked one stays unmarked. Test: `a_migrated_instance_keeps_its_construction_mark` (Task 4).
5. **A debugger evaluation at a stop inside the initializer** (`self = other`, `self hash`) signals in the evaluation and leaves the stopped frame as it was. Test: `an_evaluation_comparing_a_value_under_construction_signals` (Task 4).

---

### Task 1: The mark, set by the allocation and cleared by `CompleteInstance`

No observable change yet: every factory clears the bit it set, and nothing reads it but the tests.

**Files:**
- Modify: `crates/ns-runtime/src/heap/header.rs` (bit 32's doc and accessor pair), `crates/ns-runtime/src/heap.rs` (`InstanceHeader::for_slots`/`register_application` :5961, `allocate_instance` :2933 fallback, a `Heap::object_under_construction` / `Heap::complete_instance` pair), `crates/ns-runtime/src/heap/application_ids.rs` (only if the row's header word is built there).
- Modify: `crates/ns-runtime/src/bytecode.rs` (new `Instruction::CompleteInstance { instance: u16 }` and its arms in `sent_message`, `argument_slots`, `outer_reach_of`, `registers_read`, `register_written`, `jump_target`, `addresses_own_slots_of`, `format_instruction`), `bytecode/compact.rs` (its eight-byte form and every arm: `fits_narrow`, `encode_narrow`, `emitted_entry`, `narrow_outer_reach`, `narrow_addresses_own_slots`, `decode`), `bytecode/register_effects_tests.rs`, `bytecode/compact/forms_tests.rs`, `spec_tables.rs`, `interpreter/inline_cache/verify.rs`.
- Modify: `crates/ns-runtime/src/compiler/mod.rs` (`factory_body`: four instructions, statement map `(0, 0..4)`), `interpreter/instantiate.rs` (`factory_registers` :1385 recognizes four; `FACTORY_INITIALIZE` stays 1; the fast path's resume at `FACTORY_INITIALIZE + 1` now lands on `CompleteInstance`), `vm.rs` (compact and wide arms beside `AllocateInstance`'s :1321/:2124), `debugger/stepping.rs` (check the `FACTORY_INITIALIZE` use :2065 still holds).
- Modify: `crates/ns-runtime/src/jit/baseline.rs` (`lower`'s arm, a template clearing the bit: load the header word, clear bit 32, store it; no barrier, the word holds no pointer), `jit/macro_assembler.rs` (the idiom if missing, the bit's constant through `machine_layout`), `jit/optimizing/snapshot.rs`/`build.rs` (the instruction as a simple frame node).
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OPCODE_COMPLETE_INSTANCE = 52`, next free 53), `image/encode.rs` `encode_instruction`, `image/decode.rs` (the instruction and `own_registers`), `shipped_code/encode.rs`, `shipped_code/decode.rs`, `image/encode/wire_tests.rs`, `crates/ns-runtime/tests/opcode_tags.rs`.
- Modify: `docs/spec/bytecode.md` (page `### CompleteInstance` after `### InitializeInstance`, rule `r[bytecode.instruction.complete-instance]`, rows in the register-effects table :206 and the wire table :928, operand grammar :1009), `docs/spec/runtime.md` `r[runtime.instantiate]` (the mark at the allocation step, its clearing as the last step), `docs/spec/vm-invariants.md` (`r[vm.heap.precomputed-header-matches]`: the header carries the mark exactly for an application that inherits `Value`; a new `r[vm.heap.under-construction-only-on-value-instances]`: the mark is set only on a `FixedPointers` instance of such a class, never on another format, where bit 32 is a `LargeInteger`'s sign), `docs/design/object-model.md` header table :2236, `docs/design/memory.md` "The headers precomputed" :3627, `docs/design/compiler.md` (the factory body's four instructions), `docs/design/execution.md` (the template).
- Create: `goldens/library/value/values_made_in_a_hot_loop_compare_as_values.ns` / `.toml`.

**Interfaces:**
- Produces: `Header::under_construction(self) -> bool`, `Header::with_under_construction(self, bool) -> Header`; `Heap::object_under_construction(&self, ObjectReference<'gc>) -> bool` (false for every format but `FixedPointers`); `Heap::complete_instance(&mut self, ObjectReference<'gc>)`; `Instruction::CompleteInstance { instance: u16 }`; `OPCODE_COMPLETE_INSTANCE = 52`.

- [ ] **Step 1: Create the branch** `git switch -c feature/half-built-values`.
- [ ] **Step 2: Write the failing tests.**
  - In `heap/header.rs`'s tests, extend `each_field_reads_back_what_was_written_and_disturbs_no_other` with the under-construction field.
  - In `crates/ns-runtime/tests/value_conditions.rs` (helpers `load`, `expect_ok`), `// spec: runtime.instantiate`, `a_value_is_marked_only_until_its_factory_returns`: a program answers an `Array` holding a completed value and an instance whose construction escaped `self` into a box and was then cut short by a caught signal (the `escape.ns` shape, Task 2); through the answer's `Root`, `object_under_construction` is false for the first and true for the second, and false for an `Array` and a `LargeInteger` whose sign bit is set.
  - `opcode_tags.rs`: `the_instantiation_instructions_keep_their_numbers` adds `OPCODE_COMPLETE_INSTANCE == 52`; `the_next_free_instruction_tag_is_fifty_two` becomes `..._fifty_three`.
  - `factory_body(2)` is four instructions, the third `CompleteInstance { instance }`, in the compiler's unit tests.
- [ ] **Step 3: Run them to verify they fail:** `cargo nextest run -p ns-runtime value_conditions opcode_tags header` — FAIL (no such method / tag).
- [ ] **Step 4: Implement** the files above. The precomputed `InstanceHeader` of an application with `inherits_value` (the row's `value_slots`) carries the bit, so `MacroAssembler::load_instance_header` and `Heap::allocate_instance_with` need no change; `Heap::allocate_instance`'s own path (no precomputed header) sets it from `class_inherits_value`. `CompleteInstance` on an object not marked does nothing, so a crafted body cannot do worse than complete early.
- [ ] **Step 5: Write the golden** `values_made_in_a_hot_loop_compare_as_values.ns`: a value class `Point x: y:` made 2000 times in a loop of a module method and compared with `=` and `hash` against a fresh one each time, printing the count of equal ones (2000) and `true` for `(Point x: 1 y: 2) hash = (Point x: 1 y: 2) hash`; record with `NS_NEXT_NO_JIT=1`, `rules = ["runtime.instantiate", "semantics.value.identity"]`.
- [ ] **Step 6: Run** `cargo xtask gates commit images codegen` — all pass.
- [ ] **Step 7: Commit** `feat: an instance of a value class is marked under construction until its factory returns`.

### Task 2: A value under construction answers no value question

**Files:**
- Modify: `crates/ns-runtime/src/builtins/value.rs` (`execute`'s `SameAs` :90 and `Hash` :102: refuse a marked receiver or argument before anything else, and a marked object the walk reaches — `compare_node`, `hash_node`; the refusal is `Some(Err(...))`, never a decline, so `valueEquals:` and `valueHashOfSlots` are never reached with one), `builtins/weak.rs` (`IsValue` :55, after `declines_for_a_future`), `interpreter/instantiate.rs` (one text builder, `under_construction_error(mutator, selector, receiver, object) -> NewspeakError`, beside `non_value_slot_error` :1974, used by both leaves).
- Modify: `crates/ns-runtime/core-library/Kernel.ns`: the doc comments of `Value>>=` :2144, `==` :2168, `hash` :2191, `identityHash` :2210, `Ephemeron>>key:` :6682 and `WeakMap>>at:put:` :6831 gain an `@signals Error … while the receiver (or argument, or a value it reaches) is under construction` line; the class comment of `Value` :2080 says when an instance becomes a value. Then `regenerate_the_golden_fixture`.
- Modify: `docs/spec/language/semantics.md` (new `r[semantics.value.under-construction]` after `r[semantics.value.conditions]`: an instance of a value class is a value object only once its factory has returned; until then — forever, if the factory never returns — `=`, `hash`, `==`, `identityHash` and the question whether it is a value signal an error, while storing it and using it as an enclosing object are allowed; `r[semantics.value.conditions]`'s lead-in points at it), `docs/spec/runtime.md` (the two leaves' rules), `docs/design/object-model.md` ("A value object's four conditions": a part on the mark — the decisions above with their reasons, the prior art as read, and what is allowed while marked), `docs/design/core-library.md` if it describes `Value`'s leaves.
- Create goldens under `goldens/library/value/`, each `.toml` recorded with `NS_NEXT_NO_JIT=1` and `rules = ["semantics.value.under-construction", …]`:
  - `a_value_under_construction_cannot_be_hashed.ns` — the scratchpad's `identity.ns` shape: the initializer hashes `self`; status 1, the refusal under `hash`.
  - `a_value_under_construction_cannot_be_compared.ns` — `self = other` and `other = self` in the initializer, each caught and its `messageText` printed.
  - `a_value_under_construction_has_no_identity_yet.ns` — `self == self` in the initializer signals.
  - `an_aborted_construction_never_becomes_a_value.ns` — the scratchpad's `escape.ns`: the instance escapes into an `Array`, a later slot initializer signals and the caller catches it; afterwards its `hash` signals, and its slot reads `nil`.
  - `a_value_under_construction_is_no_weak_key.ns` — `Ephemeron new key: self` in the initializer signals the under-construction error, not `key: requires an object that is not a value`.
  - `a_value_module_builds_its_nested_values.ns` — `class M = Value (| origin = Point x: 0. same = origin = (Point x: 0). |)`: prints `0` and `true` (two instances of one class object are compared without reaching the module).
  - `values_in_a_cycle_compare_once_complete.ns` — the scratchpad's `cycle.ns`: prints `built` and `true`.
  - `a_value_built_inside_another_ones_initializer_completes_on_its_own.ns` — `Link value: n next: (n > 0 ifTrue: [Link value: n - 1 next: nil] …)` built recursively three deep; every link hashes and compares after the outer factory returns. (Review Focus 2.)
  - `a_construction_resumed_after_a_signal_completes.ns` — a slot initializer signals a resumable `Warning`, the caller resumes it with a value, the factory returns, and the instance hashes and compares. (Review Focus 3.)
- Modify: unit tests in `builtins/value.rs` (from :665, helper `leaf`): `the_equality_leaf_refuses_an_operand_under_construction`, `the_hash_leaf_refuses_a_receiver_under_construction`, `the_walk_refuses_a_value_under_construction_it_reaches`.

**Interfaces:**
- Consumes: `Heap::object_under_construction` (Task 1).
- Produces: `instantiate::under_construction_error(mutator: &Mutator<'gc>, selector: Symbol, receiver: Value<'gc>, object: ObjectReference<'gc>) -> NewspeakError`, text per Global Constraints.

- [ ] **Step 1: Write the goldens and the unit tests above** (`.toml` with the expected output written by hand from the decisions, the error text per Global Constraints).
- [ ] **Step 2: Run them to verify they fail:** `cargo nextest run -p ns-cli goldens -E 'test(/value/)'` and `cargo nextest run -p ns-runtime value::` — the refusal goldens FAIL (they print a hash); the module, cycle, recursion and resume goldens PASS already.
- [ ] **Step 3: Implement** the leaves, the text builder, the `Kernel.ns` comments (then `fmt`, then `regenerate_the_golden_fixture`), the spec and design text.
- [ ] **Step 4: Verify the recorded outputs** with `NS_NEXT_NO_JIT=1 target/debug/ns-next run <golden>` for each new golden, against the hand-written `.toml`; a difference is investigated, never copied over.
- [ ] **Step 5: Run** `cargo xtask gates commit` — passes.
- [ ] **Step 6: Commit** `fix: a value under construction answers no value question`.

### Task 3: The image carries the mark

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OBJECT_INSTANCE_UNDER_CONSTRUCTION = 17`, next free 18), `image/encode.rs` :1828 (the tag for a marked instance), `image/decode.rs` (`Sizing::record` :4762 and `decode_graph_section` :5232 take it as `instance`; the placeholder's header built by `rewrite_placeholder_header` gets the mark; `check_program_instances` :4346 or a check beside it refuses the tag for a class that does not inherit `Value`), `image/inspect.rs` :565, `crates/ns-runtime/tests/opcode_tags.rs` (live object-body list :272, `next_free == 18`).
- Modify: `docs/spec/image-format.md` (tag table :114, record grammar :408, a verification rule `r[image.verify.under-construction]` beside `r[image.verify.program-instances]` :726), `docs/design/image-format.md` (Layout and Verification).
- Test: `crates/ns-runtime/tests/image.rs`.

**Interfaces:**
- Consumes: `Heap::object_under_construction`, `Header::with_under_construction` (Task 1).
- Produces: `OBJECT_INSTANCE_UNDER_CONSTRUCTION = 17`.

- [ ] **Step 1: Write the failing tests** in `tests/image.rs`:
  - `// spec: image.verify.under-construction` `an_aborted_construction_stays_under_construction_across_an_image`: build the `an_aborted_construction_never_becomes_a_value` program's world up to after the caught construction (the application's initializer does it, so `build` writes it), load it, and `hash` of the escaped instance signals the under-construction error; a completed value in the same image hashes as before.
  - `an_image_marking_an_instance_of_a_class_that_is_no_value_class_is_refused`: a hand-made image with tag 17 on an instance of a plain class fails `load` with a verification error, no panic.
  - `opcode_tags.rs`: the object-body tag 17 is live, next free 18.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime --test image under_construction` — FAIL.
- [ ] **Step 3: Implement** the encoder, both decoder passes, the check, the inspector, the spec rules and the design text.
- [ ] **Step 4: Run** `cargo xtask gates commit images` — passes.
- [ ] **Step 5: Commit** `feat: an image carries a value's construction mark`.

### Task 4: Hot swap and the debugger keep the mark

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/install.rs` `Mutator::migrate` :913 (the rebuilt instance takes the old one's mark), `docs/design/liveness-and-reflection.md` ("The complete install", the migration's paragraph), `docs/spec/runtime.md` `r[runtime.install]` if it lists what a migration carries.
- Test: the install tests (`crates/ns-runtime/tests/` file holding `migrate` tests; find with `grep -l migrate crates/ns-runtime/tests`), the debugger evaluation tests (`crates/ns-runtime/tests/` file holding `debugger::evaluation` tests).

- [ ] **Step 1: Write the failing tests.**
  - `// spec: runtime.install` `a_migrated_instance_keeps_its_construction_mark`: an aborted construction leaves a marked instance; an edit adds an immutable slot to its class and installs; the migrated instance's `hash` still signals the under-construction error, and a completed instance of the same class still hashes. (Review Focus 4.)
  - `// spec: semantics.value.under-construction` `an_evaluation_comparing_a_value_under_construction_signals`: a line breakpoint in a value class's slot initializer; at the stop evaluate `self hash`; the answer is the under-construction error, and continuing finishes the run with the instance complete. (Review Focus 5.)
- [ ] **Step 2: Run them** — the migration test FAILS (the mark is lost); the evaluation test passes or fails, either is recorded.
- [ ] **Step 3: Implement** the migration's copy of the mark and the document text.
- [ ] **Step 4: Run** `cargo xtask gates commit` — passes.
- [ ] **Step 5: Commit** `fix: a migrated instance keeps its construction mark`.

### Task 5: The batch's end

**Files:**
- Modify: `docs/OPEN-WORK.md` (remove nothing unless an item named this defect; add the actor crossing's rule — a marked value crosses as a far reference — to the actors item), `docs/PROGRESS.md` (one entry).

- [ ] **Step 1:** Write the two documents.
- [ ] **Step 2: Run** `cargo xtask gates end-of-batch` — passes (stress modes, images, caches, benchmarks, release-checks, codegen).
- [ ] **Step 3: Commit** `docs: a value under construction is written into the progress and the open work`.
- [ ] **Step 4:** The `finishing-a-batch` skill; then report to the user, naming the proposals of this plan the user did not decide.
