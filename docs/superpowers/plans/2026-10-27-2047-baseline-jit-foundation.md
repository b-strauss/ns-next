# Baseline JIT foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The foundation of road step 8's baseline JIT on x64 -- a doubly
mapped code zone, an assembler, the macro assembler of the VM's idioms, the
entry and the exit, the compile trigger and the one exhaustive `match` that
every instruction is lowered through -- so that every body runs as machine
code from its second entry until its first instruction without a template,
and instructions are added later without touching any of it.

**Architecture:** Stages 1 and 2 of `docs/design/execution.md`, "Planned:
the baseline JIT", "Stages". `ns-memory` gets the code zone and the one
`unsafe` call into machine code; a new crate `ns-assembler` encodes x64; a
new module `crates/ns-runtime/src/jit/` holds the macro assembler, the
stubs, the baseline compiler and the per-interpreter JIT state; `vm.rs`
enters machine code at a push and takes its exit. Machine code runs only
frames the interpreter pushed: no compiled send, no compiled return, no
on-stack replacement yet (later plans, stages 3-7). A machine-code frame
leaves by an exit that names the program counter the interpreter resumes
the frame at, so the frame needs no compiled bit in this plan.

**Tech Stack:** Rust 1.98.0, edition 2021; `windows-sys` and `libc`
(already `ns-memory`'s); `iced-x86` as a dev-dependency of `ns-assembler`
only (approved by the user on 2026-10-27).

**Spec:** `docs/design/execution.md`, "Planned: the baseline JIT", and
"Planned: the deoptimization contract" above it.

## Global Constraints

- `ns-runtime`, `ns-assembler`, `ns-tooling` and `ns-analyzer` forbid
  `unsafe`; all of it is in `ns-memory`.
- Names are written out (CLAUDE.md, "Conventions"): `program_counter`,
  `register_base`, `machine_offset`, never `pc`, `rb`, `off`.
- Machine code holds no handle: an object is reached only through a
  register of the frame or a table read through a leaf; an immediate word
  (`nil`, `true`, `false`, a `SmallInteger`, an immediate `Float`) may be
  embedded.
- No code is written into the zone except through `CodeWriter`, and the
  zone is never mapped writable and executable in one view.
- The JIT is compiled in only for `target_arch = "x86_64"`; on every other
  target `jit::available()` answers `false` and every body runs
  interpreted.
- `NS_NEXT_NO_JIT=1` turns the JIT off for a run (for measuring and for
  bisecting); every golden passes with it on, with it off, and under each
  new stress mode.
- `vm.rs` holds no dispatch logic (CLAUDE.md invariant): it calls
  `jit::` functions, as it calls `interpreter::` ones.
- Commits: conventional (`feat:`, `test:`, `docs:`, ...), each ending with
  the `Co-Authored-By:` line naming the committing model and the
  `Claude-Session:` line given in the session.
- Gates: `cargo xtask gates commit` before every commit; `memory` on
  tasks touching `crates/ns-memory`; `codegen` on tasks touching `vm.rs`
  or `interpreter/mod.rs`; `end-of-batch` after Task 8. A commit that
  never saw the end-of-batch gates is work in progress.

## Review Focus

1. **An exit at an instruction a template half executed** -- the frame's
   registers must read exactly as the interpreter's would before that
   instruction, or the re-execution applies it twice; Task 7's
   `a_body_exiting_mid_way_matches_the_interpreter` pins it.
2. **A collection requested while machine code runs** -- only the back
   edge's poll may notice it, and the frame must resume interpreted at the
   jump's target with the safepoint taken; Task 6's exit stress mode and
   Task 7's `a_back_edge_poll_takes_the_safepoint` pin it.
3. **The debug poison read by a template** -- a dead register read must
   trap in a debug build as the interpreter's read does; Task 4's
   `a_register_load_traps_on_poison_in_debug` pins it.
4. **Two interpreters on one thread sharing nothing** -- a body's machine
   entry from one interpreter's zone must never run under another's; Task
   5's `a_second_interpreter_does_not_run_the_first_ones_code` pins it.
5. **A zone that fills up** -- compiling past the zone's end must leave the
   body interpreted, never abort; Task 5's `a_full_zone_leaves_bodies_interpreted`
   pins it.

---

## File Structure

- `crates/ns-memory/src/code_zone.rs` (create): `CodeZone`, `CodeWriter`,
  `CodeOffset`, the entry and the leaf shim. Platform calls go in
  `crates/ns-memory/src/platform/{windows,posix}.rs` beside the existing
  ones.
- `crates/ns-assembler/` (create): `Cargo.toml`, `src/lib.rs` (the
  operations, `Label`, `Assembler`, `Encoder` trait), `src/x64.rs` (the
  encoder), `tests/x64_encodings.rs` (against `iced-x86`).
- `crates/ns-runtime/src/jit/mod.rs` (create): `JitState`, `available()`,
  `JitSlot`, the compile trigger, the entry from `vm.rs`.
- `crates/ns-runtime/src/jit/macro_assembler.rs` (create): the VM's idioms
  and the register roles per ABI.
- `crates/ns-runtime/src/jit/stubs.rs` (create): the entry stub and the
  exit stub.
- `crates/ns-runtime/src/jit/baseline.rs` (create): the baseline compiler,
  its exhaustive `match`, the program-counter map.
- `crates/ns-runtime/src/jit/leaves.rs` (create): the leaf table and the
  generic straight-line step.
- `crates/ns-runtime/src/bytecode.rs` (modify): `CompiledCode` gains its
  `JitSlot`.
- `crates/ns-runtime/src/vm.rs` (modify): enter machine code at a push,
  take its exit.
- `crates/ns-runtime/src/interpreter/mod.rs` (modify): `JitState` on the
  interpreter, the two stress switches.
- `xtask/src/main.rs`, `crates/ns-cli/tests/cli.rs` (modify): the two new
  stress modes.
- `CLAUDE.md`, `docs/design/execution.md`, `docs/design/README.md`,
  `docs/PROGRESS.md`, `docs/OPEN-WORK.md` (modify, Task 8).

---

### Task 1: The code zone in `ns-memory`

**Files:**
- Create: `crates/ns-memory/src/code_zone.rs`
- Modify: `crates/ns-memory/src/lib.rs` (module and re-exports),
  `crates/ns-memory/src/platform/windows.rs`,
  `crates/ns-memory/src/platform/posix.rs`, `crates/ns-memory/Cargo.toml`
  (the `windows-sys` features `Win32_System_Diagnostics_Debug` and
  `Win32_System_Threading` if the calls need them)
- Test: `crates/ns-memory/src/code_zone.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub struct CodeOffset(pub u32)` -- a byte offset into a zone, `Copy`,
    `Eq`, `Ord`, `Hash`, `Debug`.
  - `pub struct CodeZone` -- `CodeZone::reserve(bytes: usize) -> Result<CodeZone, MemoryError>`,
    `bytes` at most `CodeZone::MAXIMUM_BYTES` (`128 << 20`);
    `fn size(&self) -> usize`; `fn identity(&self) -> u64` (unique per
    zone in the process, from a global `AtomicU64` counter, never 0);
    `fn executable_address(&self, offset: CodeOffset) -> usize`;
    `fn bytes(&self, range: Range<u32>) -> Option<&[u8]>` (read through
    the executable view, for tests and the disassembly of a body);
    `fn writer(&mut self) -> CodeWriter<'_>`.
  - `pub struct CodeWriter<'zone>` -- `fn write(&mut self, offset: CodeOffset, bytes: &[u8]) -> Result<(), MemoryError>`
    (commits pages as needed; refuses a range past the zone);
    dropping it flushes the instruction cache over every range written
    (a no-op on x64) and, on Apple AArch64, switches the thread back to
    execute.
  - `#[cfg(test)] pub fn executable_view_is_writable(&self) -> bool` --
    the protection the executable view was mapped with, as the zone
    recorded it.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_written_byte_reads_back_through_the_executable_view() {
    let mut zone = CodeZone::reserve(1 << 20).expect("a zone");
    zone.writer().write(CodeOffset(64), &[0xC3, 0x90]).expect("written");
    assert_eq!(zone.bytes(64..66), Some(&[0xC3, 0x90][..]));
}

#[test]
fn a_write_past_the_zone_is_refused() {
    let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
    let end = u32::try_from(zone.size()).expect("small");
    assert!(zone.writer().write(CodeOffset(end - 1), &[0, 0]).is_err());
}

#[test]
fn a_zone_larger_than_the_branch_reach_is_refused() {
    assert!(CodeZone::reserve(CodeZone::MAXIMUM_BYTES + 1).is_err());
}

#[test]
fn two_zones_have_two_identities() {
    let first = CodeZone::reserve(1 << 16).expect("a zone");
    let second = CodeZone::reserve(1 << 16).expect("a zone");
    assert_ne!(first.identity(), second.identity());
    assert_ne!(first.identity(), 0);
}

#[test]
fn the_executable_view_is_never_writable() {
    // The platform's own query (VirtualQuery / /proc/self/maps or mincore
    // plus a write probe is not portable): answer the protection the
    // executable view was mapped with, through a test-only accessor.
    let zone = CodeZone::reserve(1 << 16).expect("a zone");
    assert!(!zone.executable_view_is_writable());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-memory code_zone`
Expected: FAIL to compile, `CodeZone` not found.

- [ ] **Step 3: Implement the zone**

Windows: `CreateFileMappingW(INVALID_HANDLE_VALUE, null, PAGE_EXECUTE_READWRITE | SEC_RESERVE, …)`,
then two views with `MapViewOfFile3` (or `MapViewOfFileEx`): one
`FILE_MAP_READ | FILE_MAP_EXECUTE`, one `FILE_MAP_WRITE`; commit with
`VirtualAlloc(MEM_COMMIT)` on the writable view's range and
`VirtualProtect` the executable view's range to `PAGE_EXECUTE_READ`.
Linux: `memfd_create`, `ftruncate`, two `mmap`s (`PROT_READ | PROT_EXEC`
and `PROT_READ | PROT_WRITE`, `MAP_SHARED`). Other Unix but Apple
AArch64: `shm_open` with a unique name, unlinked at once, then as Linux.
Apple AArch64: one `mmap` with `MAP_JIT`, the writer switching with
`pthread_jit_write_protect_np(0)` on creation and `(1)` on drop; its
executable and writable addresses coincide. The instruction-cache flush:
`FlushInstructionCache` on Windows AArch64, `sys_icache_invalidate` on
Apple, `__builtin___clear_cache` through `libc`'s binding on Linux
AArch64; nothing on x64. Comments state what each choice prevents
(`docs/design/execution.md`, "Executable memory").

- [ ] **Step 4: Run the tests and the memory gate**

Run: `cargo nextest run -p ns-memory` then `cargo xtask gates memory`
Expected: PASS; clippy clean for `x86_64-unknown-linux-gnu` and
`aarch64-apple-darwin`.

- [ ] **Step 5: Commit** (`feat: a doubly mapped code zone in ns-memory`),
  after `cargo xtask gates commit`.

---

### Task 2: The entry into machine code and the leaf shim

**Files:**
- Modify: `crates/ns-memory/src/code_zone.rs`
- Test: same file

**Interfaces:**
- Consumes: Task 1's `CodeZone`, `CodeOffset`.
- Produces:
  - `pub type Leaf<C> = fn(&mut C, [u64; 3]) -> u64;`
  - `pub const ENTRY_BLOCK_WORDS: usize = 16;` and the block layout the
    generated entry stub reads: word 0 the context pointer, word 1 the
    shim's address, word 2 the leaf table's address, word 3 the leaf
    count; words 4..16 belong to the caller (`jit::stubs` defines them).
  - `impl CodeZone { pub fn enter<C>(&self, stub: CodeOffset, block: &mut [u64; ENTRY_BLOCK_WORDS], context: &mut C, leaves: &[Leaf<C>]) -> u64 }`
    -- writes words 0-3, calls the code at `stub` with the platform's C
    ABI as `extern "C" fn(*mut u64) -> u64`, the block's address its one
    argument, and answers its result.
  - The shim, `extern "C" fn leaf_shim<C>(block: *mut u64, leaf: u64, first: u64, second: u64, third: u64) -> u64`,
    reads the context and the table from the block and calls
    `leaves[leaf]`; a leaf index past the count aborts the process. Its
    doc comment states the safety argument: machine code entered through
    `enter` is a function holding `context` mutably; it hands the block
    back only while it waits for the leaf, so the `&mut C` the shim makes
    is the only live reference. A panic in a leaf aborts (Rust aborts
    when a panic reaches an `extern "C"` boundary); the doc says so.
  - `enter` is safe; its doc comment states the trust it rests on: the
    code at `stub` was written by `ns-runtime`'s JIT, which is the
    contract, as `unsafe` behind a safe abstraction is accepted here
    (`docs/design/README.md`, "Principles").

- [ ] **Step 1: Write the failing tests** (x64 only, hand-encoded bytes;
  the first argument register is `rcx` on Windows, `rdi` elsewhere)

```rust
#[cfg(target_arch = "x86_64")]
#[test]
fn entered_code_answers_a_word_of_its_block() {
    let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
    // mov rax, [argument + 32]; ret
    #[cfg(windows)]
    let code = [0x48, 0x8B, 0x41, 0x20, 0xC3];
    #[cfg(not(windows))]
    let code = [0x48, 0x8B, 0x47, 0x20, 0xC3];
    zone.writer().write(CodeOffset(0), &code).expect("written");
    let mut block = [0u64; ENTRY_BLOCK_WORDS];
    block[4] = 42;
    let mut context = ();
    assert_eq!(zone.enter(CodeOffset(0), &mut block, &mut context, &[]), 42);
}

#[cfg(target_arch = "x86_64")]
#[test]
fn the_entry_writes_the_context_and_the_shim_into_the_block() {
    // The same one-load body as above, reading word 0, then word 1:
    // the first answers the context's address, the second the shim's
    // (`leaf_shim::<u64> as usize`).
}
```

The shim is exercised through generated code in Task 4
(`a_leaf_call_from_a_body_reaches_the_context`), where the assembler
writes the call sequence; hand-encoding it here would test the hand
encoding.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-memory entered_code`
Expected: FAIL to compile, `enter` not found.

- [ ] **Step 3: Implement `enter` and `leaf_shim`**

- [ ] **Step 4: Run** `cargo nextest run -p ns-memory` and
  `cargo xtask gates memory`. Expected: PASS.

- [ ] **Step 5: Commit** (`feat: entering machine code and calling a Rust leaf from it`),
  after `cargo xtask gates commit`.

---

### Task 3: `ns-assembler`, the operations and the x64 encoder

**Files:**
- Create: `crates/ns-assembler/Cargo.toml` (workspace lints,
  `#![forbid(unsafe_code)]` in `lib.rs`; dev-dependency `iced-x86` with
  default features off and `decoder`, `intel`, `std`), `src/lib.rs`,
  `src/x64.rs`, `tests/x64_encodings.rs`
- Modify: root `Cargo.toml` (workspace member)

**Interfaces:**
- Produces (`ns_assembler::`):
  - `pub struct Register(pub u8)` -- a machine register number of the
    target; `x64::RAX … x64::R15` constants.
  - `pub enum Width { Byte, Half, Word32, Word64 }`
  - `pub struct Memory { pub base: Register, pub displacement: i32 }`
  - `pub enum Condition { Equal, NotEqual, Less, LessOrEqual, Greater, GreaterOrEqual, Below, BelowOrEqual, Above, AboveOrEqual, Overflow, NoOverflow }`
  - `pub struct Label(u32)`
  - `pub struct Assembler<E: Encoder>` with: `new()`, `label() -> Label`,
    `bind(Label)`, `move_register(Register, Register)`,
    `move_immediate(Register, u64)`, `load(Width, Register, Memory)`,
    `store(Width, Memory, Register)`, `store_immediate32(Memory, i32)`,
    `add(Register, Register)`, `subtract(Register, Register)`,
    `multiply_checked(Register, Register)`, `and_immediate(Register, i32)`,
    `or_immediate(Register, i32)`, `shift_left(Register, u8)`,
    `shift_right_arithmetic(Register, u8)`, `compare(Register, Register)`,
    `compare_immediate(Register, i32)`, `compare_memory(Register, Memory)`,
    `test_immediate(Register, i32)`, `jump(Label)`,
    `jump_if(Condition, Label)`, `call_register(Register)`,
    `push(Register)`, `pop(Register)`, `add_stack(i32)`, `return_()`,
    `trap()`; `finish(self) -> Result<Assembled, AssemblyError>`.
  - `pub struct Assembled { pub bytes: Vec<u8>, pub label_offsets: Vec<Option<u32>> }`
    -- `label_offsets[label]` is where the label was bound.
  - `pub trait Encoder` -- one per architecture; the encoder of an
    operation answers its bytes; a branch answers its short and long
    forms, chosen in `finish`'s one pass: a forward branch at its longest,
    a backward one exact (`docs/design/execution.md`, "The foundation and
    the one extension point").
  - `pub struct X64;` implementing `Encoder`.
  - `AssemblyError::UnboundLabel(Label)`.

- [ ] **Step 1: Write the failing tests** in `tests/x64_encodings.rs`:
  for every operation, assemble it over a table of operands -- every
  register 0..16 on each side, displacements 0, 1, -1, 127, 128, -129,
  `i32::MAX` -- decode the bytes with `iced_x86::Decoder` (64-bit), format
  with Intel syntax and compare to the expected text, e.g.
  `assert_eq!(disassemble(&bytes), "mov rax,[r13+80h]")`. Branches:
  `a_backward_branch_within_reach_is_short` (a `jump` to a label bound
  two operations earlier is two bytes), `a_forward_branch_is_long`,
  `an_unbound_label_is_an_error`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-assembler`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the assembler and the x64 encoder**

- [ ] **Step 4: Run them to verify they pass.**

- [ ] **Step 5: Commit** (`feat: ns-assembler, the operations and their x64 encoding`),
  after `cargo xtask gates commit`.

---

### Task 4: The macro assembler and the stubs

**Files:**
- Create: `crates/ns-runtime/src/jit/mod.rs` (module skeleton,
  `pub(crate) fn available() -> bool`), `jit/macro_assembler.rs`,
  `jit/stubs.rs`
- Modify: `crates/ns-runtime/Cargo.toml` (`ns-assembler`),
  `crates/ns-runtime/src/lib.rs` (`mod jit;`)
- Test: `jit/stubs.rs` and `jit/macro_assembler.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: Task 2's `CodeZone::enter`, `Leaf`, `ENTRY_BLOCK_WORDS`;
  Task 3's `Assembler<X64>`.
- Produces:
  - `pub(crate) enum Abi { Windows64, SystemV }` and
    `Abi::host() -> Abi`.
  - Block words 4..16 (`jit::stubs::BlockWord`, an enum with explicit
    discriminants): `ObjectSpaceBase = 4`, `StackBase = 5`,
    `RegisterBase = 6` (the frame's register base, in words from the
    stack's base), `MachineEntry = 7` (the body's address to call),
    `ExitProgramCounter = 8`, `ExitKind = 9`, `SavedStackPointer = 10`,
    `PollWord = 11` (the address of the heap's safepoint flag).
  - `pub(crate) enum ExitKind { Reexecute = 0, Safepoint = 1 }`.
  - Register roles (`macro_assembler::Roles`, per `Abi`), pinned in
    callee-saved registers while a body runs: `frame` (absolute address
    of the frame's register 0), `context` (the block), `object_space`
    (the object space's base); scratch registers for templates.
  - `pub(crate) struct MacroAssembler { assembler: Assembler<X64>, roles: Roles, abi: Abi }`
    with idioms: `load_frame_register(target: Register, register: u16)`,
    `store_frame_register(register: u16, source: Register)`,
    `load_immediate_word(target: Register, word: u64)`,
    `call_leaf(leaf: LeafIndex, arguments: [Operand; 3])` (saves nothing
    across: the register cache is flushed by the caller),
    `exit(kind: ExitKind, program_counter: u32)`,
    `poll(program_counter: u32)` (compare the poll word, take
    `exit(Safepoint, program_counter)` when set).
  - `pub(crate) fn assemble_entry_stub(abi: Abi) -> Assembled` -- saves
    the ABI's callee-saved registers, loads the roles from the block,
    stores the stack pointer into `SavedStackPointer`, calls
    `MachineEntry`; the exit stub (`assemble_exit_stub`) writes the
    exit's program counter and kind into the block, resets the stack
    pointer from `SavedStackPointer`, restores the callee-saved
    registers and returns `ExitKind` to `enter`'s caller.
  - In a debug build `load_frame_register` compares the word with the
    heap's poison word and calls the leaf `Leaf::Poisoned`, which panics
    with the interpreter's message, so a template's read of a dead
    register fails as the interpreter's read does (`compiler.md`, "What
    a callee leaves").

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_entry_stub_runs_a_body_that_exits_at_once() {
    // A body that is only `exit(Reexecute, 7)`, entered through the
    // stub: `enter` answers ExitKind::Reexecute and the block's
    // ExitProgramCounter is 7; the callee-saved registers are intact
    // afterwards (the test calls it twice in a loop of 1000 and keeps a
    // local live across the calls).
}

#[test]
fn a_body_moves_one_frame_register_into_another() {
    // A fake frame of eight words in a Vec<u64>; StackBase is its
    // address, RegisterBase 0; the body loads register 2 and stores it
    // into register 5, then exits. Afterwards frame[5] == frame[2].
}

#[test]
fn a_leaf_call_from_a_body_reaches_the_context() {
    // A body that calls leaf 0 with (5, 6, 7) through `call_leaf` and
    // exits; the leaf adds its arguments to the context, a `u64` counter
    // starting at 100. Afterwards the counter is 118.
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "dead register")]
fn a_register_load_traps_on_poison_in_debug() { /* frame[3] = poison */ }
```

The tests exercise the stubs on a fake frame in a `Vec<u64>` and do not
need an interpreter.

- [ ] **Step 2: Run them to verify they fail.**

Run: `cargo nextest run -p ns-runtime jit::`

- [ ] **Step 3: Implement the macro assembler and the stubs.**

- [ ] **Step 4: Run them to verify they pass**, and
  `cargo xtask gates commit`.

- [ ] **Step 5: Commit** (`feat: the macro assembler of the VM's idioms and the entry and exit stubs`).

---

### Task 5: The JIT state, the compile trigger and the extension point

**Files:**
- Modify: `crates/ns-runtime/src/jit/mod.rs`, `crates/ns-runtime/src/bytecode.rs`
  (`CompiledCode::jit: JitSlot`), `crates/ns-runtime/src/interpreter/mod.rs`
  (`Interpreter`'s `jit: JitState`)
- Create: `crates/ns-runtime/src/jit/baseline.rs`, `jit/leaves.rs`
- Test: `crates/ns-runtime/src/jit/baseline.rs`, `crates/ns-runtime/tests/jit.rs`

**Interfaces:**
- Consumes: Tasks 2-4.
- Produces:
  - `pub(crate) struct JitSlot` on `CompiledCode`, interior-mutable
    (`Cell`s), not encoded by any image or shipped-code encoder: the
    owning zone's identity (`u64`, 0 for none), the body's machine offset
    and its program-counter map index, and the entry count. States:
    *counting* (entries so far), *compiled* (zone identity and offset),
    *refused* (never tried again).
  - `pub(crate) struct JitState` on the interpreter: `zone: Rc<CodeZone>`
    (default size 16 MiB, a constant `DEFAULT_ZONE_BYTES` whose comment
    says the size is measured later -- `OPEN-WORK.md`'s step 8 item),
    `next_offset`, the entry stub's offset, the program-counter maps
    (`Vec<ProgramCounterMap>`), `enabled: bool` (false when
    `!available()`, `NS_NEXT_NO_JIT=1`, or the zone could not be
    reserved).
  - `pub(crate) fn machine_entry(state: &JitState, code: &CompiledCode) -> Option<CodeOffset>`
    -- `Some` only when the slot's zone identity is `state.zone.identity()`.
  - `pub(crate) fn note_entry(state: &mut JitState, code: &CompiledCode)`
    -- counts; at the second entry (the first under the compile stress
    mode, Task 6) compiles the body; a failure (zone full, a body past
    `u16::MAX` instructions) marks the slot refused.
  - `jit::baseline::compile(code: &CompiledCode, macro_assembler: &mut MacroAssembler) -> Result<BaselineBody, Refusal>`
    with **the one exhaustive `match` over `Instruction`**, each arm
    choosing `Lowering::Template`, `Lowering::Leaf` or
    `Lowering::Exit`. In this task every arm is `Lowering::Exit`, with
    a comment block above the `match` stating the three lowerings and
    their conditions (`execution.md`, "The foundation and the one
    extension point"); no wildcard arm.
  - `pub struct JitStatistics { pub compiled_bodies: u64, pub refused_bodies: u64, pub machine_back_edges: u64, pub safepoint_exits: u64 }`
    and `Interpreter::jit_statistics(&self) -> JitStatistics`, counted in
    every build (a back edge counts through the poll's cold path only
    under a test switch, so the hot loop pays nothing).
  - `ProgramCounterMap`: the machine offset of every instruction's start,
    and the reverse lookup `program_counter_at(machine_offset) -> Option<u32>`.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/ns-runtime/tests/jit.rs, through the public Interpreter API and
// a local `expect_ok` (copy a neighbour's).
#[test]
fn a_body_is_compiled_at_its_second_entry() {
    // A program calling `answer` three times; JitStatistics (a
    // `pub fn jit_statistics(&self) -> JitStatistics` on Interpreter,
    // test-facing like `cache_statistics`) reports `compiled_bodies`
    // >= 1 after it ran, and 0 under NS_NEXT_NO_JIT (set through a
    // `set_jit_enabled(false)` test switch, not the environment).
}

#[test]
fn a_second_interpreter_does_not_run_the_first_ones_code() {
    // Two interpreters on one thread built from the same source; run the
    // first's program twice (compiled), then the second's: its statistics
    // count its own compilation, and the output is right.
}

#[test]
fn a_full_zone_leaves_bodies_interpreted() {
    // An interpreter with a zone of one page (a test switch
    // `with_zone_bytes`): a program with many bodies still prints its
    // output, and statistics report refused bodies.
}
```

- [ ] **Step 2: Run them to verify they fail.**

- [ ] **Step 3: Implement**, and wire `vm.rs`: at every push (the
  `Transfer::Switched` to a new activation at program counter 0),
  `jit::note_entry`, then, when `machine_entry` answers, run the body
  through `jit::run_machine_code(mutator, activation, register_base) -> MachineExit`
  (in `jit/mod.rs`: fills the block, `zone.enter`, decodes the exit).
  `MachineExit::Reexecute { program_counter }` continues the stretch at
  that program counter, interpreted; `MachineExit::Safepoint { program_counter }`
  leaves the stretch as a backward jump that found a request does,
  storing that program counter. The check on a push is the only
  addition to the hot path: one load of the slot's state and one
  compare. Run `cargo xtask gates codegen` and keep its budgets; if a
  budget breaks, move the cold part (`note_entry`'s counting past the
  first compare, `run_machine_code`) out of line with `#[cold]` before
  touching the budget.

- [ ] **Step 4: Run** `cargo nextest run --workspace` (every golden under
  both lowerings, every body exiting at its first instruction) and
  `cargo xtask gates commit codegen`. Expected: PASS.

- [ ] **Step 5: Commit** (`feat: bodies compiled at their second entry through one exhaustive match`).

---

### Task 6: The two stress modes

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`,
  `crates/ns-runtime/src/jit/mod.rs`, `xtask/src/main.rs` (the stress
  group's mode list and its test), `crates/ns-cli/tests/cli.rs`
  (`env_remove` of both), `docs/design/README.md` ("Gates"),
  `CLAUDE.md` (the stress row: "eight stress modes")

**Interfaces:**
- Produces: `NS_NEXT_STRESS_JIT=1` -- every body compiled at its first
  entry; `NS_NEXT_STRESS_EXIT=1` -- every poll a body passes takes the
  exit as if a safepoint were requested (in this plan the back edges;
  later plans add the entry's). Both read in a debug build only, as the
  other stress switches are, and both force `enabled` on where
  `available()`.

- [ ] **Step 1: Write the failing test** in `xtask/src/main.rs`:
  `the_stress_group_runs_eight_modes` asserting the mode list contains
  `NS_NEXT_STRESS_JIT` and `NS_NEXT_STRESS_EXIT` beside the six.

- [ ] **Step 2: Run it to verify it fails.**

- [ ] **Step 3: Implement both switches and add them to the list.**

- [ ] **Step 4: Run** `NS_NEXT_STRESS_JIT=1 cargo nextest run --workspace`
  and `NS_NEXT_STRESS_EXIT=1 cargo nextest run --workspace` (timeout
  600000 each). Expected: PASS.

- [ ] **Step 5: Commit** (`test: two stress modes, every body compiled and every poll an exit`),
  after `cargo xtask gates commit`.

---

### Task 7: The first templates and the generic leaf

**Files:**
- Modify: `crates/ns-runtime/src/jit/baseline.rs`,
  `jit/macro_assembler.rs`, `jit/leaves.rs`, `crates/ns-runtime/src/vm.rs`
  (only to make the step functions named below `pub(crate)`)
- Test: `crates/ns-runtime/src/jit/baseline.rs`,
  `crates/ns-runtime/tests/jit.rs`, two goldens under
  `goldens/runtime/jit/` (shaped as their neighbours under
  `goldens/runtime/`, formatted with `ns-next-analyzer fmt`, `///`
  comments)

**Interfaces:**
- Consumes: Task 5's `match`, Task 4's idioms.
- Produces:
  - Templates, each the exact effect of its `run_stretch` arm in
    `vm.rs`: `Move`, `StoreLocal`, `LoadSelf`, `LoadLiteral` whose literal
    is an immediate word (`Nil`, `Boolean`, a `SmallInteger` in range, an
    immediate `Float`; the word computed at compile time by the function
    `load_literal_into` already uses), `Jump` (a backward one polls
    first), `JumpIfNotBoolean`, `BranchIfFalse`, `BranchIfTrue`, and
    `IntegerBinary` -- the `SmallInteger` fast path of `integer_into` for
    all nine operations, a decline (non-integer operand, overflow, zero
    divisor, `SMALL_INTEGER_MIN // -1`) jumping to `target` as
    `integer_into`'s `false` does.
  - `Leaf::Step` in `jit/leaves.rs`: `fn step(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64`
    -- arguments the activation, the program counter, unused; runs the
    straight-line instruction there through the interpreter's own step
    function (`load_literal_into`, `make_array_into`,
    `make_vector_into`, `load_vector_into`, and `StoreVector`'s), and
    answers 0 when done or 1 to ask for an exit that re-executes -- only
    before it changed anything (a refused vector access), so the
    interpreter raises the error itself. `LoadLiteral` with a
    non-immediate literal, `MakeArray`, `MakeVector`, `LoadVector` and
    `StoreVector` take `Lowering::Leaf`; every other instruction stays
    `Lowering::Exit`, with a one-line comment per group saying which
    later stage gives it a template (sends: stage 3; closures,
    non-local return, allocation: stage 4).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_counting_loop_runs_in_machine_code() {
    // goldens/runtime/jit/counting_loop.ns: `| sum ::= 0. index ::= 1. |
    // [index <= 1000000] whileTrue: [sum:: sum + index. index:: index + 1].
    // sum out` -- prints 500000500000; under NS_NEXT_STRESS_JIT the
    // statistics report machine-code back edges > 0.
}

#[test]
fn integer_overflow_in_machine_code_answers_a_large_integer() {
    // goldens/runtime/jit/overflow.ns: the largest SmallInteger plus 1 inside
    // a loop body run compiled; prints the LargeInteger.
}

#[test]
fn a_body_exiting_mid_way_matches_the_interpreter() {
    // A body with templated moves, then a send, then more moves: run
    // compiled and with set_jit_enabled(false); the outputs are equal
    // and so is every register the program can observe afterwards.
}

#[test]
fn a_back_edge_poll_takes_the_safepoint() {
    // A loop of 100000 iterations allocating an Array literal through
    // the leaf, run compiled: it completes with the right output and
    // `jit_statistics().safepoint_exits` is at least 1, since the
    // allocations request scavenges only the back edge can notice.
}
```

Record each golden's `.toml` from the interpreter with the JIT off
(`NS_NEXT_NO_JIT=1`), never from the JIT's output.

- [ ] **Step 2: Run them to verify they fail** (the loop's statistics
  report no compiled back edge while every instruction exits).

- [ ] **Step 3: Implement the templates and the leaf.**

- [ ] **Step 4: Run** `cargo nextest run --workspace`, then under
  `NS_NEXT_STRESS_JIT=1` and `NS_NEXT_STRESS_EXIT=1`, and
  `cargo xtask gates commit codegen`. Expected: PASS.

- [ ] **Step 5: Commit** (`feat: templates for moves, literals, jumps and integer arithmetic`).

---

### Task 8: Measure, document, close the batch

**Files:**
- Modify: `docs/design/execution.md` (move "Stages" 1 and 2 to built,
  in present tense, and add "What the baseline JIT's foundation moved"),
  `docs/design/README.md` (the map: `ns-assembler`, `jit/`),
  `CLAUDE.md` (seven crates: `ns-assembler`'s line; `ns-memory`'s line
  with the code zone; `jit/` in `ns-runtime`'s description),
  `docs/PROGRESS.md`, `docs/OPEN-WORK.md` ("Next": stage 3, sends and the
  caches in machine code)

- [ ] **Step 1: Measure** with the `measuring-performance` skill: every
  program of `benchmarks/` and `ns-next bench`, this branch against
  `master`, three builds a side, release; and the same branch with
  `NS_NEXT_NO_JIT=1` against `master` to isolate the push's added
  check. A program slower than `master` beyond its builds' spread is
  diagnosed and fixed in this batch, not recorded.

- [ ] **Step 2: Write the documents** -- the measurements, and that a
  body runs compiled only until its first instruction without a
  template, so the foundation is not expected to be faster yet.

- [ ] **Step 3: Run** `cargo xtask gates end-of-batch` (timeout 600000
  per call; the stress part runs the eight modes). Expected: PASS.

- [ ] **Step 4: Commit** (`docs: the baseline JIT's foundation`), then
  the `finishing-a-batch` skill.
