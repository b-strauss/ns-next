# Overlapping Register Windows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Road step 7a′: a send's arguments are no longer copied into the
callee's window -- the caller's contiguous argument slots *are* the
callee's first registers, in Cog's layout (arguments, header,
everything else) -- with no change to any program's outcome and each of
its two stages measured no slower in release than the tree before it.

**Architecture:** Stage 1 (T1-T3) makes a send's arguments contiguous
slots at the top of the caller's live registers, lowered straight into
them by both lowerings, and replaces the step state's argument buffer by
an argument window the decisions read in place; the push still copies,
once, from the slots. Stage 2 (T4-T8) makes the header 13 words in every
build, leaves its words out of the register numbering, pushes the
callee's frame on the caller's slots, and adds the decoder's checks that
keep a crafted image from writing a live header. Each stage ends in a
measurement that decides whether it is kept.

**Tech Stack:** Rust 1.98.0, cargo-nextest, `cargo xtask gates`.

**Spec:** `docs/design/execution.md`, "Planned: overlapping register
windows" (decided with the user on 2026-10-24), read with "Activations"
("Representation", "Suspending and resuming", "The depth limit"), "No
re-entry from Rust", "Closure conversion"; `docs/design/compiler.md`,
"Registers", "The value-register invariant", "The reference lowering";
`docs/design/image-format.md`, "Verification".

## Global Constraints

- `CLAUDE.md`'s invariants hold throughout. The ones this plan touches
  most: an expression's value register still holds its value when its
  consumer executes; `vm.rs` holds no dispatch logic; compiling a
  resolved module never raises a `NewspeakError`; an image is untrusted
  input (no byte sequence panics or aborts); a wire tag is assigned once
  and never renumbered or reused; every instruction reading another
  frame's registers answers its depth in `outer_reach_of` and
  `narrow_outer_reach`; a handle never outlives its interpreter; never
  `{:?}` a `Value` or `.unwrap()` an `EvaluationResult`.
- Every golden, `run-corpus` and benchmark outcome stays exactly as
  recorded, under both lowerings. A changed outcome is a defect of the
  change, never a reason to update a golden.
- `ns-runtime` keeps `#![forbid(unsafe_code)]`; no new external
  dependency.
- The header is 13 words in every build once T4 lands
  (`frame_header::COUNT == 13`); the flags word packs the kind tag in
  bits 0-7, the argument count in bits 8-23, `counted` at 24,
  `retains_registers` at 25 and, in a debug build, the 31-bit generation
  in bits 26-56.
- The register numbering skips the header from T5 on: registers
  `0 .. argument_count` are the arguments, `argument_count ..
  argument_count + 13` are never named by an instruction, and copied
  values, temporaries, the vector register and scratch start at
  `argument_count + 13`. One constant names the 13 for the compiler,
  `bytecode::HEADER_REGISTERS: u16`, held equal to
  `frame_header::COUNT` by a compile-time assertion.
- New wire tags take the next free numbers in `image/opcodes.rs` (46 is
  the next today); the tags they replace go into `RETIRED_OPCODES`, and
  `tests/opcode_tags.rs` pins both.
- Names are written out (`CLAUDE.md`, "Conventions"): `argument_base`,
  `argument_sources`, `arguments`, never `args` or `base` alone as a
  field.
- Gates: every commit runs `cargo xtask gates commit`; a commit touching
  `crates/ns-runtime/src/image/` or `bytecode.rs`/`bytecode/` also runs
  `cargo xtask gates images`; one touching
  `crates/ns-runtime/src/interpreter/inline_cache.rs` also runs
  `cargo xtask gates caches`; one touching `crates/ns-analyzer` also
  runs `cargo xtask gates analyzer`; one changing a type a size test pins
  (`RegisterBase`, `ActivationReference`, `Step`, `Signal`) also runs
  `cargo xtask gates release-checks`. `end-of-batch` runs at the end of
  each stage, in T3 and T8; every commit before them is work in
  progress. Every command runs in the foreground.
- Never judge performance from a debug build; T3 and T8 follow the
  `measuring-performance` skill and the protocol of execution.md's "What
  frames as words moved" (three release builds a side with the pinned
  `RUSTFLAGS`, each in its own `--target-dir`, varying the block layout
  inside `vm::run` as that section says).

## Review Focus

1. **A named local passed as an argument and assigned by a later
   argument** (`self foo: x bar: (x:: 3)`): the slot must hold the value
   `x` had when the first argument was evaluated, not 3. The send copies
   named locals at send time, so `lower_operand`'s protective `Move` --
   now into the slot -- is what keeps it. Pinned in T1.
2. **A setter assignment's value read after the send** (`^x:: 3`, `a::
   b:: 4`): the value is read from the slot after the callee returned,
   so a callee must never write its argument registers. Pinned by
   goldens already; T5 adds a debug poison that makes any read of a dead
   register above the slots visible, and T7 the decoder's refusal of a
   write to an argument register.
3. **A send whose argument is an inlined conditional or loop**
   (`self foo: (c ifTrue: [1] ifFalse: [2])`): its value is in the
   branch's result register, not lowered into the slot, so the send must
   copy it. Pinned in T2.
4. **A future forced before a send is re-run, and every other push that
   is not the send's own callee** (`doesNotUnderstand:`'s handler,
   `Message>>sendTo:`, `cull:`, unwind blocks, the VM-error `signal`, the
   factory's initializer): each lands above the caller's whole window,
   so the slots the re-run reads are untouched. Pinned in T5.
5. **A suspended sender resumed after its callee returned**
   (`NS_NEXT_STRESS_SUSPEND`): the sender's frame object holds its
   registers below the callee's header only, and its slots' values --
   which the callee shared -- come back intact. Pinned in T6 and by the
   stress gate in T8.

---

## Stage 1: contiguous slots, and the buffer goes

### Task 1: A send names its argument slots

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (the send and initializer
  instructions; `outer_reach_of`; the disassembly text)
- Modify: `crates/ns-runtime/src/bytecode/compact.rs` (narrow and wide
  send forms, `SendSite`, `WideSend`, the initializer register tables)
- Modify: `crates/ns-runtime/src/compiler/lower.rs`,
  `crates/ns-runtime/src/compiler/reference.rs`
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `image/encode.rs`,
  `image/decode.rs`; `crates/ns-runtime/tests/opcode_tags.rs`
- Modify: `crates/ns-runtime/src/vm.rs` (`send`, `send_mirror`, the
  initializer arms)
- Test: `crates/ns-runtime/src/compiler/` unit tests,
  `crates/ns-runtime/src/bytecode/compact.rs` round-trip tests

**Interfaces:**
- Produces: in `Instruction::SendLexical`, `SendPublic`,
  `SendInlinable`, `SendSuper`, `SendMirror`, `InitializeSuperclass` and
  `InitializeAppliedLevel`, the field `argument_registers: Vec<u16>` is
  replaced by `argument_base: u16` and `argument_sources: Vec<u16>`. The
  send's arguments are the registers `argument_base ..
  argument_base + argument_sources.len()`; before deciding, the send
  copies `argument_sources[index]` into `argument_base + index` for
  every `index` where the two differ. `InitializeInstance` keeps naming
  `0 .. argument_count` (its base is 0, its sources its own).
- Produces: the eight-byte forms carry `argument_base: u8` and one `u8`
  source per argument (`SendPublic2` is `destination, receiver,
  argument_base, sources: [u8; 2], site` -- eight bytes with the tag);
  `WideSend` gains `argument_base: u16` beside its `arguments` (the
  sources).
- Produces: new wire tags for the seven instructions, taking 46 onward;
  25-28, 44 and 45 join `RETIRED_OPCODES` (`SendMirror` has none).

- [ ] **Step 1: Write the failing tests.** In the lowering tests, for
  each of `self foo: 1 bar: 2`, `a foo: x` (a parameter `x`),
  `super foo: (b bar)` and a lexical send `foo: 3`, assert on the
  lowered instruction that `argument_sources.len()` is the arity and that
  `argument_base .. argument_base + arity` are registers at or above the
  body's first scratch register, allocated after the receiver's. Under
  the reference lowering assert additionally that every source equals
  its slot (`argument_sources[index] == argument_base + index`). For
  `self foo: x bar: (x:: 3)` with a writable local `x`, assert that the
  first source is not `x`'s register (the protective copy went into the
  slot).
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime compiler`; the new
  tests fail to compile.
- [ ] **Step 3: Change the instructions, their compact forms, the wire
  format and the VM.** The VM's `send` copies the sources that differ
  into their slots, then gathers the slots into the buffer as today; the
  decoder refuses a base or source past the body's register count, and
  a base whose last slot is.
- [ ] **Step 4: Change both lowerings.** The optimized lowering
  allocates the slots right after lowering the receiver (or at once,
  for a receiver-less send), lowers each argument through
  `lower_operand` as today, and records the register it answers as the
  argument's source; a protective copy `materialize` makes goes into
  the slot (`Move { destination: slot, source }`). The reference
  lowering lowers the receiver, then allocates the slots, then every
  argument into a fresh register `Move`d into its slot -- direct
  targeting comes in T2 -- and `check_shape` asserts sources equal
  slots.
- [ ] **Step 5: Run** the tests of Step 1, then `cargo xtask gates commit
  images`; all pass, every golden unchanged.
- [ ] **Step 6: Commit** `feat: a send names its contiguous argument slots`.

### Task 2: Arguments lowered straight into their slots

**Files:**
- Modify: `crates/ns-runtime/src/compiler/lower.rs`,
  `crates/ns-runtime/src/compiler/reference.rs`
- Test: the lowering unit tests

**Interfaces:**
- Consumes: T1's `argument_base`/`argument_sources`.
- Produces: `Lowerer::lower_expression_into(&mut self, expression:
  &Expression, rest: &[Expression], slot: u16) -> u16`, answering the
  argument's source: `slot` itself when the value was produced there.
  A non-inlined send writes its `destination` into the slot and a
  literal, `self`, an outer-local read or a closure is loaded into it;
  every other expression (a named local, an inlined form's result
  register, an assignment, a cascade) answers the register it is in, and
  the send copies it.

- [ ] **Step 1: Write the failing tests.** `self foo: (a bar) baz: 3`:
  both sources equal their slots and no `Move` is emitted. `self foo: (c
  ifTrue: [1] ifFalse: [2])`: the source differs from the slot (the
  branch's result register). Under the reference lowering, every source
  equals its slot and the only `Move`s are the reads of named locals
  (rule 3).
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime compiler`; the first
  and third fail.
- [ ] **Step 3: Implement `lower_expression_into`** and use it for every
  send's arguments in both lowerings; the reference lowering targets
  every value into its slot, so `check_shape`'s "every scratch register
  written by exactly one instruction" still holds.
- [ ] **Step 4: Run** the tests, then `cargo xtask gates commit images`;
  all pass.
- [ ] **Step 5: Commit** `perf: a send's arguments are lowered into their slots`.

### Task 3: The argument window replaces the buffer; stage 1 measured

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`StepArguments`,
  `decide_send`, `act`, `push`, `push_step`, `push_sent`, the buffer's
  removal), `interpreter/inline_cache.rs`, `interpreter/resolve.rs`,
  `interpreter/sends.rs`, `interpreter/instantiate.rs`,
  `interpreter/mirror.rs`, `interpreter/future.rs`,
  `interpreter/slots.rs`, every `builtins/` module that takes
  `arguments: &[Value]`, `platform.rs`, `vm.rs`, `heap.rs`
  (`push_activation_in` takes its arguments from the window)
- Modify: `docs/design/execution.md` (a "What contiguous arguments
  moved" section; "Representation", "No re-entry from Rust")
- Test: `crates/ns-runtime/src/heap.rs` unit tests

**Interfaces:**
- Consumes: T1's slots.
- Produces, in `crates/ns-runtime/src/interpreter/mod.rs`:

```rust
/// The arguments a decision reads: a send's slots in its sender's
/// window, read where they are, or values a decision or the host built.
#[derive(Clone, Copy)]
pub(crate) enum Arguments<'values, 'gc> {
    Slots { register_base: RegisterBase, argument_base: u16, count: u16 },
    Values(&'values [Value<'gc>]),
}

impl<'values, 'gc> Arguments<'values, 'gc> {
    pub(crate) fn len(&self) -> usize;
    pub(crate) fn get(&self, heap: &Heap<'gc>, index: usize) -> Value<'gc>;
    pub(crate) fn to_vec(&self, heap: &Heap<'gc>) -> Vec<Value<'gc>>;
}
```

  Every decision, native and leaf that took `arguments: &[Value<'gc>]`
  takes `arguments: Arguments<'_, 'gc>`; `StepArguments::slice` becomes
  `StepArguments::window(&self, send: Arguments) -> Arguments`.
  `Heap::push_activation` and `push_reserved_activation` take
  `arguments: Arguments<'_, 'gc>` and copy the slots with one
  `copy_within` of the frame stack's words. `take_argument_buffer`,
  `return_argument_buffer` and the `argument_buffer` field are removed.

- [ ] **Step 1: Write the failing test** in `heap.rs`: a push whose
  `Arguments::Slots` names two slots of the frame below copies exactly
  their values into the new frame's registers 0 and 1, and a push from
  `Arguments::Values` copies the slice.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime heap`; fails to
  compile.
- [ ] **Step 3: Implement** the window through the decisions, natives and
  pushes, and remove the buffer. Nothing reads a slot after a heap
  allocation that could move it: no collection runs inside a decision,
  only at safepoints.
- [ ] **Step 4: Run** `cargo xtask gates commit images caches
  release-checks`; passes.
- [ ] **Step 5: Commit** `perf: decisions read a send's arguments where they are`.
- [ ] **Step 6: Run** `cargo xtask gates end-of-batch`; passes.
- [ ] **Step 7: Measure** the branch's head against its merge base with
  `master`, by "What frames as words moved"'s protocol in full: the
  eight micro sources (the four not in the tree written out from
  "What spec-conformant lookup moved"), the Are We Fast Yet programs, two
  sittings.
- [ ] **Step 8: Decide.** Kept if no program is slower with its builds
  not overlapping the merge base's. Otherwise **stop and report to the
  user**; the stage is not built on.
- [ ] **Step 9: Record** the table as "What contiguous arguments moved",
  after "What suspending and resuming moved", with what it covers, and
  bring "Representation" and "No re-entry from Rust" to the window;
  commit `docs: stage 1 of 7a′ measured`.

## Stage 2: the windows overlap

### Task 4: The header is 13 words in every build

**Files:**
- Modify: `crates/ns-runtime/src/heap/frames.rs` (`frame_header`,
  `FrameFlags` and its packing), `crates/ns-runtime/src/heap.rs`
  (`frame_generation`, the push's generation, `check_activation`)
- Modify: `docs/design/execution.md` ("Representation": the header's
  size and the flags word)
- Test: `crates/ns-runtime/src/heap/frames.rs` unit tests

**Interfaces:**
- Produces: `frame_header::GENERATION` is gone; `frame_header::COUNT` is
  13 unconditionally. `FrameFlags` gains `#[cfg(debug_assertions)]
  generation: u32` and keeps `argument_count: u16` (was `u32`); its
  layout is the Global Constraints'. `FrameFlags::new` takes an
  `argument_count: u16`; a push converts with `u16::try_from` and
  `expect`s it, naming the register ceiling.

- [ ] **Step 1: Write the failing tests:** `FrameFlags` round-trips every
  kind tag with argument counts 0 and `u16::MAX`, both booleans, and
  (debug) generations 0 and `0x7FFF_FFFF`; `frame_header::COUNT == 13`.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime frames`; fails.
- [ ] **Step 3: Implement**; the debug build's stale-handle check reads the
  generation from the flags word.
- [ ] **Step 4: Run** `cargo xtask gates commit release-checks`; passes.
- [ ] **Step 5: Commit** `refactor: the frame header is 13 words in every build`.

### Task 5: The callee's frame on the caller's slots

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`HEADER_REGISTERS`, the
  `CompiledCode::register_count` doc), `compiler/lower.rs` (`Layout`),
  `compiler/reference.rs`, `compiler/mod.rs` (the factory's `instance`
  and `answer` registers), `compiler/unwind_marker.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (`push_activation_in`, a new
  overlapped push, `pop_activation`, `activation_register_base`,
  `window_end`, `fill_copied_values`, `suspend_activations`,
  `register_slot`'s debug check), `heap/frames.rs`
  (`frame_header::REGISTER_COUNT`'s meaning, `FrameStack::truncate`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`push_step`,
  `push_sent`: a `StepArguments::Send` step pushes overlapped, every
  other one at the top), `vm.rs` (the sender's register base after a
  return)
- Test: `heap.rs` unit tests; every golden under both lowerings

**Interfaces:**
- Consumes: T3's `Arguments::Slots`, T4's 13-word header.
- Produces: `bytecode::HEADER_REGISTERS: u16 = 13`, with
  `const _: () = assert!(HEADER_REGISTERS as usize == frame_header::COUNT);`
  where both are visible. A frame's `ActivationReference` is its
  header's offset; its `RegisterBase` is that offset minus its argument
  count; `REGISTER_COUNT` holds the registers above the header (the
  window minus the arguments), so `window_end(offset) = offset + COUNT +
  REGISTER_COUNT`.
- Produces: `Heap::push_overlapped_activation(&mut self, header:
  ActivationHeader<'gc>, register_base: RegisterBase, argument_base:
  u16, argument_count: u16, temporary_count: usize) ->
  Result<ActivationReference, StackExhausted<'gc>>`, the sender's
  register base and the send's slots, placing the header at the slots'
  end; the
  pushes from `Arguments::Values` and from slots that are not the
  sender's top (the factory's initializer) keep copying at the top.
  `pop_activation` sets the top to the caller's `window_end`, and a debug
  build fills the caller's registers from its slots' end to its
  `window_end` with the poison `SmallInteger` `0x5EAD_5EAD`, so a read of
  a register the callee overwrote shows as wrong output.

- [ ] **Step 1: Write the failing tests** in `heap.rs`: an overlapped push
  over two slots answers a frame whose registers 0 and 1 are the slots'
  words (same stack index, no copy), whose header lies at the slots'
  end, and whose pop restores the top to the caller's window end; a
  push from `Arguments::Values` above it lands at the top.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime heap`; fails.
- [ ] **Step 3: Skip the header in the numbering** in both lowerings'
  layouts, the factory's registers and the unwind markers; every
  register index computed from `argument_count` moves by
  `HEADER_REGISTERS` above the arguments.
- [ ] **Step 4: Implement the overlapped push and the pop**, and suspend
  a sender's registers only below its callee's header, the rest nil.
- [ ] **Step 5: State the invariant in the lowerings:** every register
  above a send's slots is written before it is read after the send. The
  optimized lowering `debug_assert!`s, when it emits a send, that no
  register above the slots holds a value a later instruction of the
  statement reads (the scratch allocator's mark at the slots); the
  reference lowering's `check_shape` asserts that no register allocated
  above a send's slots before the send is read after it.
- [ ] **Step 6: Run** `cargo xtask gates commit images caches
  release-checks`; every golden unchanged under both lowerings, the
  debug poison never read.
- [ ] **Step 7: Commit** `perf: a callee's frame lies on its caller's argument slots`.

### Task 6: Frame objects without the header's numbers

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`reify`, `divorce`,
  `frame_local`, `frame_set_local`, `suspend_activations`,
  `resume_activation`, `frame_register_shape`),
  `crates/ns-runtime/src/image/encode.rs`, `image/decode.rs`
- Test: `heap.rs` unit tests, the image round-trip tests

**Interfaces:**
- Produces: a frame object's register fields are its registers without
  the header's numbers: register `r` is field `r` for `r <
  argument_count` and `r - HEADER_REGISTERS` above them; one function,
  `Heap::frame_field_of_register(argument_count: u16, register: u16) ->
  Option<usize>`, answers `None` for a register the header's numbers
  name. A frame record in an image holds the same fields; if a stale
  record could decode into a different valid frame, its tag is retired
  and reassigned.

- [ ] **Step 1: Write the failing tests:** a reified frame of a method with
  two arguments and three temporaries has five register fields, and
  `frame_local` of register `2 + HEADER_REGISTERS` answers the first
  temporary; suspending and resuming it round-trips every register.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime heap`; fails.
- [ ] **Step 3: Implement** the mapping in every reader and writer.
- [ ] **Step 4: Run** `cargo xtask gates commit images`; passes.
- [ ] **Step 5: Commit** `refactor: a frame object leaves the header's numbers out`.

### Task 7: The decoder keeps a crafted image off a live header

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`CompiledCode::local_count`),
  `compiler/lower.rs`, `compiler/reference.rs`, `image/encode.rs`,
  `image/decode.rs`, `image/opcodes.rs` if the code record's tag changes
- Modify: `docs/design/image-format.md` ("Verification")
- Test: `crates/ns-runtime/src/image/` decoder tests

**Interfaces:**
- Produces: `CompiledCode::local_count: u16`, the first scratch
  register (arguments, header numbers, copied values, temporaries and
  the vector register below it), written by both lowerings and carried
  in the code record.
- Produces: the decoder refuses, each with an error of its own, (a) an
  instruction that names a register in `argument_count ..
  argument_count + HEADER_REGISTERS`, (b) one that writes a register
  below `argument_count`, and (c) a `StoreOuterLocal` whose `index` is
  at or past the `local_count` of the body `depth` levels out, found
  through the `MakeClosure` nesting it decodes.

- [ ] **Step 1: Write the failing tests:** three crafted bodies, one per
  refusal, each refused with its error and none panicking.
- [ ] **Step 2: Run** `cargo nextest run -p ns-runtime image`; fails.
- [ ] **Step 3: Implement** the field and the three checks; the encoder
  writes `local_count`.
- [ ] **Step 4: Run** `cargo xtask gates commit images`; passes.
- [ ] **Step 5: Commit** `feat: the decoder refuses a write onto a live frame header`.

### Task 8: Stage 2 measured, the living text, the end of the batch

**Files:**
- Modify: `docs/design/execution.md` ("Representation", "Planned:
  overlapping register windows" rewritten as built, a "What overlapping
  windows moved" section, "Planned: JIT tiers"), `docs/design/compiler.md`
  ("Registers"), `docs/design/memory.md` if "What the collector traces"
  names the header's size, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`,
  `CLAUDE.md` (the invariant that every register above a send's slots
  is dead when it executes, beside the value-register invariant)

- [ ] **Step 1: Run** `cargo xtask gates end-of-batch`; passes.
- [ ] **Step 2: Measure** the head against T3's last commit, by the same
  protocol as T3.
- [ ] **Step 3: Decide.** Kept if no program is slower with its builds
  not overlapping T3's. Otherwise **stop and report to the user**.
- [ ] **Step 4: Record** "What overlapping windows moved", rewrite the
  planned section and every statement about copied arguments as built,
  and update `OPEN-WORK.md` ("Next" moves to 7c), `PROGRESS.md` (road
  step 7a′) and `CLAUDE.md`.
- [ ] **Step 5: Commit** `docs: road step 7a′ measured, and its progress entry`.
