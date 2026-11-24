# The Compact Instruction Encoding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Store every compiled body as fixed eight-byte instructions with side tables, run `vm::run` on them, and keep today's `Instruction` only as the vocabulary the compiler emits and every tool reads back -- after an experiment has shown, on the Are We Fast Yet benchmarks, that the eight-byte slots are worth building.

**Architecture:** Task 1 is a gate: a throwaway copy runs the loop on eight-byte slots and is measured against `master`; the user decides on its numbers, and Task 2 (a 32-bit stream, same protocol) runs only if the user asks for it after Task 1. Once the gate is passed, the encoding lands in stages that are each green: the `Immediate` word (Task 3), the encoding with its builder and exact round trip (Task 4), `CompiledCode` storing only the encoding with every producer and reader and the loop switched over (Task 5), the wide forms' stress mode and goldens (Task 6), the measurement (Task 7), the documentation (Task 8), and the end-of-batch gates (Task 9).

**Tech Stack:** Rust 1.98.0, edition 2021, `#![forbid(unsafe_code)]` in `ns-runtime`. No new dependency. Tests under `cargo-nextest`; goldens under `goldens/`.

**Spec:** `docs/design/execution.md`, "Planned: the compact instruction encoding" (designed with the user on 2026-10-02), with its subsections "What it starts from", "The encoding" and "How it is built: measured first". Background: `execution.md`, "Where the 3% goes" (the niche, the spills), "A send's value delivered where it is decided: tried and halted" (the gate this plan's Task 1 copies), "What the eight-byte value moved" (the measurement protocol Tasks 1 and 7 repeat); `docs/design/compiler.md`, "The instruction set", "Safepoints and call-site identity"; `docs/design/image-format.md` (the wire format, unchanged by this plan).

## Global Constraints

- Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - `vm.rs` holds no dispatch logic and raises no error of its own; every send still goes through `interpreter/inline_cache.rs` (`send_public`, `send_lexical`, `send_super`) or `Mutator::decide_send`, and `SendMirror` through `interpreter::mirror::send_inlined`.
  - Compiling a resolved module never raises a `NewspeakError`, and encoding never fails: every `Instruction` has an encoding, the wide one at worst.
  - An image is untrusted input: no byte sequence may panic. **The wire format does not change and this plan adds no wire tag**: the decoder still reads today's `Instruction`s and emits them into the builder.
  - Every instruction that reads or writes another frame's registers by reference stays named in `bytecode::outer_reach_of`, which keeps reading `Instruction`s.
  - Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvaluationResult`. Tests use a local `expect_ok` helper.
- **The program counter stays an instruction index.** One `Instruction` is one slot; jump targets, `statement_program_counter`, a frame's `program_counter` and the image's frame records keep their meaning.
- **`CompactInstruction` is eight bytes**, pinned by a `const` assertion, and every field of every variant is an integer, a `[u8; 2]` or a `MirrorPrimitive` -- no heap field, so no niche.
- **The round trip is exact:** `CompiledCode::instructions()` answers exactly the `Instruction`s that were emitted, a `Float` literal bit for bit.
- Names are written out (`CLAUDE.md`, "Conventions"); clippy's `min_ident_chars` refuses one- and two-character names. Comments explain the decision, not the mechanics.
- A Newspeak module inline in a test is an `indoc! {"…"}` or `formatdoc!`, indented with the Rust around it.
- Conventional commits. Every commit ends with
  `Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01AnZi5xMPKXmXfe1e8TdEAq`.
  A subagent's commit names the subagent's own model.
- Work happens on the branch `feature/compact-instruction-encoding` in the main working directory (no worktree), created from `master` after the gate (Task 1) is passed. Tasks 1 and 2 commit nothing to the repository.
- Per-commit gate (`CLAUDE.md`, "Working in this repository"), every task that commits code, however small:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace --exclude ns-lsp`
  - `cargo test --workspace --exclude ns-lsp --doc`
  - `cargo run -p ns-cli -- parse-corpus corpus/modules` → "parsed 211, failed 0"

  The stress modes, `run-corpus`, the benchmarks profile and the `ns-lsp` suite run once, in Task 9 (the `finishing-a-batch` skill). A commit that never saw them is work in progress; a subagent implementing a task is told so. Nothing under `crates/ns-syntax` or `editors/jetbrains` changes.
- Run every command in the foreground, except the measurements of Tasks 1, 2 and 7 (see there), which the controller session runs itself.

## Review Focus

- **A body at the narrow forms' edges.** Register 255 is narrow and 256 wide; a send of two arguments is narrow and of three wide; `i32::MIN` and `i32::MAX` load through `LoadSmallInteger` and `i32::MAX + 1` through `LoadConstant`; the 65,536th constant, literal, send site, block, mirror site and register list of one body take a wide form. Each round-trips exactly and runs to the same output. Pinned in Task 4 (round-trip tests) and Task 6 (goldens and generated tests).
- **A float literal's bits.** `-0.0` and `0.0` are different constants and both round-trip; a float outside the `SmallFloat64` range (`1.0e300`, a denormal) is a `LoadLiteral` and allocates a boxed `Float` as today. Pinned in Task 3 (`Immediate`) and Task 4 (round trip by bits).
- **A decoded image's call sites.** `number_call_site_families` numbers sites held in `send_sites` and in `wide` in emission order, so a family that mixes narrow and wide sends (under `NS_NEXT_STRESS_WIDE`, every send) still numbers densely and `family_size` agrees with the compiler's. Pinned in Task 5 (the existing `tests/image.rs` numbering tests, unchanged in what they assert) and Task 6 (the whole suite under the stress mode).
- **A send whose caller resumes.** After a callee returns, the caller's send writes its value to the destination its slot or its wide `SendSite` names -- `send_destination` covers every send form, `SendMirror` and a wide `SendMirror` included. Pinned in Task 5 (the existing goldens) and Task 6 (the suite under the stress mode, where every send is wide).
- **An instruction that runs again after forcing a future.** `ResolveFuture` and `InitializeSlot` re-execute from the same `program_counter` after a forcing; in a wide form they must read their operands from the `wide` entry again. Pinned in Task 6 (the simultaneous-slot goldens under the stress mode).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ns-runtime/src/heap/value.rs` | `Immediate`, `ImmediateKind`, `Value::from_immediate` (Task 3) |
| `crates/ns-runtime/src/heap.rs` | re-exports `Immediate`, `ImmediateKind` (Task 3) |
| `crates/ns-runtime/src/bytecode/compact.rs` (new, Task 4) | `CompactInstruction`, the side-table types, `EncodedBody`, `CodeBuilder` (encode, patch), the decoded view |
| `crates/ns-runtime/src/bytecode.rs` | `mod compact;`, `Instruction::set_target`, `CompiledCode` storing `EncodedBody` with its constructors and readers (Task 5) |
| `crates/ns-runtime/src/compiler/lower.rs`, `compiler/reference.rs` | emit into a `CodeBuilder` (Task 5) |
| `crates/ns-runtime/src/image/decode.rs`, `image/encode.rs` | emit into a `CodeBuilder`; number call sites in the tables; read the view (Task 5) |
| `crates/ns-runtime/src/vm.rs` | the loop on `CompactInstruction`, one send path per kind, `run_wide` (Task 5) |
| `crates/ns-runtime/src/compiler/unwind_marker.rs`, `interpreter/inline_cache.rs` | read the view (Task 5) |
| `crates/ns-runtime/tests/*.rs`, in-crate test modules | `.instructions` → `.instructions()`, `CompiledCode { … }` → `CompiledCode::new(…)` (Task 5) |
| `crates/ns-runtime/src/interpreter/safepoint.rs` | `NS_NEXT_STRESS_WIDE` (Task 6) |
| `crates/ns-runtime/tests/compact_encoding.rs` (new, Task 6) | generated bodies past every 16-bit table index |
| `goldens/runtime/encoding/*` (new, Task 6) | a method of 300 registers, sends of three to eight arguments |
| `.claude/skills/finishing-a-batch/SKILL.md` | the fourth stress mode (Task 6) |
| `docs/design/*.md`, `CLAUDE.md`, `docs/PROGRESS.md`, `docs/OPEN-WORK.md` | planned → built (Tasks 7, 8) |

**The interface every task after Task 3 relies on:**

```rust
// heap/value.rs (Task 3), re-exported from heap.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Immediate(u64);
#[derive(Clone, Copy, Debug)]
pub enum ImmediateKind { Nil, Boolean(bool), SmallInteger(i64), SmallFloat(f64) }
impl Immediate {
    pub const NIL: Immediate;
    pub const fn boolean(value: bool) -> Immediate;
    pub fn small_integer(value: i64) -> Option<Immediate>; // None outside 61 bits
    pub fn small_float(value: f64) -> Option<Immediate>;   // None unless SmallFloat64
    pub fn kind(self) -> ImmediateKind;
}
impl<'gc> Value<'gc> { pub const fn from_immediate(immediate: Immediate) -> Value<'gc>; }

// bytecode/compact.rs (Task 4)
pub enum CompactInstruction { /* listed in Task 4 */ }
pub struct EncodedBody { /* slots and side tables, Task 4 */ }
pub struct CodeBuilder { /* Task 4 */ }
impl CodeBuilder {
    pub fn new() -> CodeBuilder;              // wide only where needed, or everywhere under
                                              // NS_NEXT_STRESS_WIDE (Task 6); `Default` is `new`
    pub fn never_wide_by_stress() -> CodeBuilder; // ignores the stress mode: tests of narrow forms
    pub fn always_wide() -> CodeBuilder;      // every instruction in a wide form
    pub fn emit(&mut self, instruction: Instruction) -> usize;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn patch_target(&mut self, index: usize, target: u32);
    pub fn instruction(&self, index: usize) -> Instruction;
    pub fn instructions(&self) -> Vec<Instruction>;
    pub fn finish(self) -> EncodedBody;
}
impl EncodedBody {
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn slot(&self, index: usize) -> CompactInstruction;
    pub fn instruction(&self, index: usize) -> Instruction;
    pub fn instructions(&self) -> Vec<Instruction>;
    pub fn for_each_emitted_mut<E>(
        &mut self,
        visit: impl FnMut(Emitted<'_>) -> Result<(), E>,
    ) -> Result<(), E>;
}
pub enum Emitted<'a> { CallSite(&'a mut CallSite), Block(&'a mut Rc<CompiledCode>) }

// bytecode.rs (Task 5)
impl CompiledCode {
    pub fn new(kind: CodeKind, instructions: Vec<Instruction>, argument_count: u16,
               register_count: u16, copied_count: u16,
               statement_program_counter: Vec<(usize, Range<usize>)>, source: Span) -> CompiledCode;
    pub(crate) fn from_builder(kind: CodeKind, builder: CodeBuilder, argument_count: u16,
               register_count: u16, copied_count: u16,
               statement_program_counter: Vec<(usize, Range<usize>)>, source: Span) -> CompiledCode;
    pub fn instruction(&self, index: usize) -> Instruction;
    pub fn instructions(&self) -> Vec<Instruction>;
    pub fn instruction_count(&self) -> usize;
    pub(crate) fn slot(&self, index: usize) -> CompactInstruction;
    pub(crate) fn body(&self) -> &EncodedBody;
    pub(crate) fn body_mut(&mut self) -> &mut EncodedBody;
}
impl Instruction { pub(crate) fn set_target(&mut self, target: u32); }
```

---

### Task 1: The gate -- eight-byte slots in a throwaway copy, measured against `master`

**Files:** none in the repository. Everything lives under `.temp/compact-gate/` (git-ignored).

**Interfaces:**
- Produces: the numbers the user decides on, and `.temp/compact-gate/vm.patch`, the throwaway loop, which Task 5 may read as a starting point (never apply it blindly: the throwaway encodes beside today's instructions, the real design does not).

The experiment answers one question: does the loop on eight-byte slots run the sixteen programs and the eight microbenchmarks faster than `master`? It is built only as far as that needs. **The controller session runs the measurement itself** (about half an hour: ten minutes of builds, twenty of runs); a subagent may build the copy (Steps 1-4) and must then stop.

- [ ] **Step 1: Make the copy.** From the repository root, in Git Bash:

```bash
mkdir -p .temp/compact-gate/tree && git archive master | tar -x -C .temp/compact-gate/tree
```

- [ ] **Step 2: Encode every body beside its instructions.** In the copy, add Task 3's `Immediate` (copy its code from Task 3, Step 4) and Task 4's `compact.rs` (copy its code from Task 4, Step 4; the decoded view and `for_each_emitted_mut` may be left out). Give `CompiledCode` one more field, `pub encoded: EncodedBody`, filled eagerly wherever a non-test `CompiledCode` is built: `compiler/lower.rs`'s `finish`, `compiler/reference.rs`'s `finish`, `compiler/mod.rs` and `image/decode.rs`'s `decode_code`, each by `let mut builder = CodeBuilder::new(); for instruction in &instructions { builder.emit(instruction.clone()); } builder.finish()`. Test-only constructions (`heap.rs`, `interpreter/unwind.rs`, `interpreter/inline_cache.rs`, `crates/ns-runtime/tests/`) may stay uncompiled; the experiment is judged by the goldens, which `ns-cli`'s tests run.

- [ ] **Step 3: Run `vm::run` on the slots.** Port `run_stretch` to `match code.encoded.slot(program_counter)`, exactly as Task 5, Step 6 describes (one arm per `CompactInstruction` variant, one send path per kind after the match, `run_wide` out of line for `Wide`). `send_destination`, `initialize_slot_refused` and `simultaneous_clause_instruction` take their operands from the slot (or its `wide` entry) instead of re-reading `code.instructions`. Save the diff of `vm.rs` as `.temp/compact-gate/vm.patch`.

- [ ] **Step 4: Check the copy is right.** In `.temp/compact-gate/tree`:

Run: `cargo nextest run -p ns-cli`
Expected: every golden passes under both lowerings, the same count as on `master`.

Build a release binary with debug information and check the loop's shape with `dumpbin /disasm` (Visual Studio Build Tools; not on `PATH`): one `ns_runtime::vm::run`, no `vm::run::closure`, no `run_stretch`, and `run_wide` a function of its own. Record `vm::run`'s instruction count and stack frame size beside `master`'s (6,978 instructions at `7f52e8b`; "What the eight-byte value moved").

- [ ] **Step 5: Measure.** The protocol is "What the eight-byte value moved"'s, with `master` as "before" and the copy as "after": three pinned release builds of each (`-C metadata=alpha`, `beta`, `gamma`), each checked to import no `VCRUNTIME140.dll` and to print the expected output for all sixteen programs and the eight microbenchmark sources (the four under `goldens/tool/bench/` and the four of `execution.md`, "What spec-conformant lookup moved", written out verbatim into `.temp/compact-gate/bench-lookup/`). Use the build commands of `docs/superpowers/plans/2026-10-02-1310-ns-next-eight-byte-value.md`, Task 11, Step 1, with `.temp\compact-gate\tree` in place of the `before` worktree and `master`'s tree as a `git worktree` at `.temp\compact-gate\master`; and its `measure.ps1` (Task 11, Step 2) with `$directory = ".temp\compact-gate"` and `target\bench-lookup` replaced by `.temp\compact-gate\bench-lookup`, with one change: the five slow programs run three times, every other ten (`measuring-performance`, "Three runs of the slow programs") -- in the program loop, `$iterations = if ($name -in "CD","Havlak","Mandelbrot","NBody","DeltaBlue") { 3 } else { 10 }` and `bench $project --iterations $iterations -- $name`; and without the `--timed` loop and `timed.csv`, since two ns-next trees are compared with `bench` alone (`measuring-performance`). Run it as one background command with a one-hour timeout; nothing else runs on the machine meanwhile.

- [ ] **Step 6: Report to the user and stop.** For each source: the pooled median of the nine values per tree, the range, the change in percent, and whether the ranges are disjoint; `vm::run`'s size and frame against `master`'s; what the measurement does not cover. Then **halt the plan**: the user decides whether the slots are built (go on with Task 3), whether the 32-bit stream is measured too (Task 2), or neither. Delete nothing under `.temp/compact-gate/` yet.

---

### Task 2 (only if the user asks for it after Task 1): the 32-bit stream, measured the same way

**Files:** none in the repository; `.temp/compact-gate/stream/`.

- [ ] **Step 1:** Copy `master` as in Task 1, Step 1, into `.temp/compact-gate/stream/tree`. Encode every body as a stream of `u32` units: the low byte the opcode, then up to three byte operands; a send `[opcode | destination | receiver | argument count]` followed by one unit with the site index and one unit per four further argument registers; a jump `[opcode | register | 0 | 0]` followed by the target unit; an operand above 255 through a `Wide` opcode whose operands follow as whole units. The program counter becomes a unit offset; a jump target is a unit offset, translated from the instruction index when the body is encoded.
- [ ] **Step 2:** Port the loop as in Task 1, Step 3, reading `units[program_counter]` and advancing by each form's length; check it as in Task 1, Step 4.
- [ ] **Step 3:** Measure the stream build against `master` *and* against Task 1's slot build, by the same protocol (nine binaries; the script's `$binaries` lists all three trees).
- [ ] **Step 4:** Report as in Task 1, Step 6, and stop. If the user chooses the stream, this plan is rewritten before anything is built.

---

### Task 3: `Immediate`, a value that holds no handle

**Files:**
- Modify: `crates/ns-runtime/src/heap/value.rs`
- Modify: `crates/ns-runtime/src/heap.rs:28` (re-export)

**Interfaces:**
- Produces: `Immediate`, `ImmediateKind`, `Value::from_immediate` as listed above.

- [ ] **Step 1: Create the branch**

```bash
git switch -c feature/compact-instruction-encoding master
```

- [ ] **Step 2: Write the failing tests** at the end of `value.rs`'s `mod tests`:

```rust
#[test]
fn an_immediate_answers_the_value_it_was_made_from() {
    let integer = Immediate::small_integer(SMALL_INTEGER_MAX).expect("61 bits fit");
    assert_eq!(
        Value::<'static>::from_immediate(integer).as_small_integer(),
        Some(SMALL_INTEGER_MAX)
    );
    assert!(Value::<'static>::from_immediate(Immediate::NIL).is_nil());
    assert_eq!(
        Value::<'static>::from_immediate(Immediate::boolean(true)).as_boolean(),
        Some(true)
    );
    let float = Immediate::small_float(1.5).expect("1.5 is a SmallFloat64");
    assert!(matches!(
        Value::<'static>::from_immediate(float).kind(),
        ValueKind::SmallFloat(value) if value == 1.5
    ));
}

#[test]
fn an_immediate_refuses_what_needs_an_object() {
    assert_eq!(Immediate::small_integer(SMALL_INTEGER_MAX + 1), None);
    assert_eq!(Immediate::small_integer(SMALL_INTEGER_MIN - 1), None);
    assert_eq!(Immediate::small_float(1.0e300), None);
    assert_eq!(Immediate::small_float(f64::NAN), None);
    assert_eq!(Immediate::small_float(f64::MIN_POSITIVE / 2.0), None);
}

#[test]
fn an_immediate_keeps_a_floats_bits() {
    let positive = Immediate::small_float(0.0).expect("zero is immediate");
    let negative = Immediate::small_float(-0.0).expect("negative zero is immediate");
    assert_ne!(positive, negative);
    let ImmediateKind::SmallFloat(back) = negative.kind() else {
        panic!("expected a SmallFloat");
    };
    assert_eq!(back.to_bits(), (-0.0f64).to_bits());
    assert!(matches!(
        Immediate::small_integer(-7).expect("fits").kind(),
        ImmediateKind::SmallInteger(-7)
    ));
    assert!(matches!(Immediate::NIL.kind(), ImmediateKind::Nil));
    assert!(matches!(Immediate::boolean(false).kind(), ImmediateKind::Boolean(false)));
}
```

- [ ] **Step 3: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime --lib an_immediate`
Expected: FAIL to compile, "cannot find type `Immediate`".

- [ ] **Step 4: Implement** in `value.rs`, after `impl std::fmt::Debug for Word`:

```rust
/// A value that holds no handle -- `nil`, a Boolean, a `SmallInteger`, a
/// `SmallFloat64` -- kept as the word it is, so that compiled code can
/// carry its constants beside its instructions
/// (`docs/design/execution.md`, "Planned: the compact instruction
/// encoding"). It means the same in every heap and across every move, so
/// code holding one needs no `Trace`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Immediate(u64);

/// What an `Immediate` is, for the decoded view of compiled code.
#[derive(Clone, Copy, Debug)]
pub enum ImmediateKind {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    SmallFloat(f64),
}

impl Immediate {
    pub const NIL: Immediate = Immediate(NIL_WORD);

    pub const fn boolean(value: bool) -> Immediate {
        Immediate(if value { TRUE_WORD } else { FALSE_WORD })
    }

    /// `None` outside 61 bits, where the integer is a `LargeInteger` object.
    pub fn small_integer(value: i64) -> Option<Immediate> {
        fits_small_integer(value).then(|| Immediate(Value::<'static>::small_integer(value).word))
    }

    /// `None` outside the `SmallFloat64` range, where the float is boxed.
    pub fn small_float(value: f64) -> Option<Immediate> {
        is_small_float_bits(value.to_bits())
            .then(|| Immediate(Value::<'static>::small_float(value).word))
    }

    pub fn kind(self) -> ImmediateKind {
        match Value::<'static>::from_immediate(self).kind() {
            ValueKind::Nil => ImmediateKind::Nil,
            ValueKind::Boolean(value) => ImmediateKind::Boolean(value),
            ValueKind::SmallInteger(value) => ImmediateKind::SmallInteger(value),
            ValueKind::SmallFloat(value) => ImmediateKind::SmallFloat(value),
            ValueKind::Object(_) => {
                unreachable!("internal: an Immediate is only ever made from an immediate word")
            }
        }
    }
}
```

and in `impl<'gc> Value<'gc>`, after `boolean`:

```rust
    /// The value `immediate` holds, in any heap: it carries no handle.
    pub const fn from_immediate(immediate: Immediate) -> Value<'gc> {
        Value::immediate(immediate.0)
    }
```

In `heap.rs:28`, add `Immediate, ImmediateKind` to the `pub use value::{…}` list.

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run -p ns-runtime --lib an_immediate`
Expected: 3 passed.

- [ ] **Step 6: Per-commit gate, then commit**

```bash
git add crates/ns-runtime/src/heap/value.rs crates/ns-runtime/src/heap.rs
git commit -m "feat: an Immediate is a value that holds no handle, for code to carry its constants"
```

---

### Task 4: The encoding, its builder and its exact round trip

**Files:**
- Create: `crates/ns-runtime/src/bytecode/compact.rs`
- Modify: `crates/ns-runtime/src/bytecode.rs` (`mod compact; pub use compact::{…};`, `Instruction::set_target`)

**Interfaces:**
- Consumes: `Immediate`, `ImmediateKind` (Task 3).
- Produces: `CompactInstruction`, `EncodedBody`, `CodeBuilder`, `Emitted`, `SendSite`, `WideSend`, `BlockSite`, `MirrorSite` as listed above and below. Nothing outside this file uses them yet.

- [ ] **Step 1: `Instruction::set_target`.** Move the match of `compiler/lower.rs`'s `patch_target` (`lower.rs:1538-1567`) into `bytecode.rs` as

```rust
impl Instruction {
    /// Sets the target of a jump, of an inlined integer operation's
    /// fallback, or of a mirror send's failure block -- the instructions a
    /// lowering emits before it knows where they go.
    pub(crate) fn set_target(&mut self, target: u32) {
        match self {
            Instruction::Jump { target: jump_target }
            | Instruction::JumpIfNotBoolean { target: jump_target, .. }
            | Instruction::BranchIfFalse { target: jump_target, .. }
            | Instruction::BranchIfTrue { target: jump_target, .. }
            | Instruction::IntegerBinary { target: jump_target, .. }
            | Instruction::SendMirror { failure: jump_target, .. } => *jump_target = target,
            other => unreachable!("internal: set_target on a non-jump instruction {other:?}"),
        }
    }
}
```

and make `Lowerer::patch_target` call `self.instructions[instruction_index].set_target(target)` (it changes again in Task 5).

- [ ] **Step 2: Write the failing round-trip tests** in `compact.rs`'s `mod tests`. Every assertion compares through `same`, which compares a `Float` literal by its bits (a NaN literal is equal to itself only that way):

```rust
use super::*;
use crate::bytecode::{CallSite, Instruction, IntegerOperation, Literal};
use crate::Symbol;

fn same(left: &[Instruction], right: &[Instruction]) -> bool {
    // `Debug` prints a float's shortest round-tripping text, `-0.0` and
    // `NaN` included, so equal text is equal bits for every literal here.
    format!("{left:?}") == format!("{right:?}")
}

fn round_trip(instructions: Vec<Instruction>) {
    for mut builder in [CodeBuilder::never_wide_by_stress(), CodeBuilder::always_wide()] {
        for instruction in &instructions {
            builder.emit(instruction.clone());
        }
        let body = builder.finish();
        assert!(same(&body.instructions(), &instructions), "{:?}", body.instructions());
        assert_eq!(body.len(), instructions.len());
    }
}

fn send(selector: &str, receiver: u16, arguments: &[u16], destination: u16) -> Instruction {
    Instruction::SendPublic {
        selector: Symbol::intern(selector),
        receiver_register: receiver,
        argument_registers: arguments.to_vec(),
        destination,
        call_site: CallSite { number: 3 },
    }
}

#[test]
fn a_compact_instruction_is_eight_bytes() {
    assert_eq!(std::mem::size_of::<CompactInstruction>(), 8);
}

#[test]
fn every_literal_round_trips() {
    let literals = [
        Literal::Nil,
        Literal::Boolean(true),
        Literal::Boolean(false),
        Literal::SmallInteger(0),
        Literal::SmallInteger(i64::from(i32::MIN)),
        Literal::SmallInteger(i64::from(i32::MAX)),
        Literal::SmallInteger(i64::from(i32::MAX) + 1),
        Literal::SmallInteger(crate::SMALL_INTEGER_MIN),
        Literal::Float(0.0),
        Literal::Float(-0.0),
        Literal::Float(1.5),
        Literal::Float(1.0e300),
        Literal::Float(f64::MIN_POSITIVE / 2.0),
        Literal::String("text".to_string()),
        Literal::Symbol(Symbol::intern("symbol")),
        Literal::LargeInteger(
            crate::ArbitraryInteger::parse("99999999999999999999", 10).expect("digits"),
        ),
        Literal::UnresolvedIdentifier("missing".to_string()),
    ];
    round_trip(
        literals
            .into_iter()
            .map(|literal| Instruction::LoadLiteral { destination: 1, literal })
            .collect(),
    );
}

#[test]
fn a_literal_takes_the_form_its_value_needs() {
    let mut builder = CodeBuilder::never_wide_by_stress();
    for literal in [
        Literal::Nil,
        Literal::Boolean(true),
        Literal::SmallInteger(7),
        Literal::SmallInteger(i64::from(i32::MAX) + 1),
        Literal::Float(1.5),
        Literal::Float(1.0e300),
    ] {
        builder.emit(Instruction::LoadLiteral { destination: 2, literal });
    }
    let body = builder.finish();
    assert!(matches!(body.slot(0), CompactInstruction::LoadNil { destination: 2 }));
    assert!(matches!(body.slot(1), CompactInstruction::LoadTrue { destination: 2 }));
    assert!(matches!(body.slot(2), CompactInstruction::LoadSmallInteger { destination: 2, value: 7 }));
    assert!(matches!(body.slot(3), CompactInstruction::LoadConstant { destination: 2, .. }));
    assert!(matches!(body.slot(4), CompactInstruction::LoadConstant { destination: 2, .. }));
    assert!(matches!(body.slot(5), CompactInstruction::LoadLiteral { destination: 2, .. }));
}

#[test]
fn sends_of_up_to_two_arguments_are_narrow_and_wider_ones_wide() {
    let mut builder = CodeBuilder::never_wide_by_stress();
    builder.emit(send("a", 1, &[], 2));
    builder.emit(send("b:", 1, &[3], 2));
    builder.emit(send("c:d:", 1, &[3, 4], 2));
    builder.emit(send("e:f:g:", 1, &[3, 4, 5], 2));
    builder.emit(send("h:", 256, &[3], 2));
    let body = builder.finish();
    assert!(matches!(body.slot(0), CompactInstruction::SendPublic0 { .. }));
    assert!(matches!(body.slot(1), CompactInstruction::SendPublic1 { .. }));
    assert!(matches!(body.slot(2), CompactInstruction::SendPublic2 { .. }));
    assert!(matches!(body.slot(3), CompactInstruction::SendPublicWide { .. }));
    assert!(matches!(body.slot(4), CompactInstruction::SendPublicWide { .. }));
}

#[test]
fn every_instruction_round_trips_narrow_and_wide() {
    let block = std::rc::Rc::new(crate::bytecode::CompiledCode::new(
        crate::bytecode::CodeKind::Block,
        vec![Instruction::ImplicitReturn { register: 0 }],
        0,
        1,
        0,
        vec![(0, 0..1)],
        ns_syntax::Span::new(0, 1),
    ));
    let mirror = crate::builtins::primitive::mirror_primitive_named("string:concatenate:ifFail:")
        .expect("the mirror has string:concatenate:ifFail:");
    let lexical = |arguments: &[u16]| Instruction::SendLexical {
        depth: 2,
        selector: Symbol::intern("lexical"),
        argument_registers: arguments.to_vec(),
        destination: 7,
        call_site: CallSite { number: 1 },
    };
    let superclass = |arguments: &[u16]| Instruction::SendSuper {
        selector: Symbol::intern("super"),
        argument_registers: arguments.to_vec(),
        destination: 7,
        call_site: CallSite::UNCACHED,
    };
    let inlinable = |arguments: &[u16]| Instruction::SendInlinable {
        selector: Symbol::intern("ifTrue:"),
        receiver_register: 4,
        argument_registers: arguments.to_vec(),
        destination: 7,
        call_site: CallSite::UNCACHED,
    };
    let mut instructions = vec![
        Instruction::LoadSelf { destination: 255 },
        Instruction::LoadEnclosingObject { destination: 3, depth: 4 },
        Instruction::Move { destination: 1, source: 2 },
        Instruction::StoreLocal { source: 1, slot: 2 },
        Instruction::LoadOuterLocal { destination: 1, depth: 2, index: 3 },
        Instruction::StoreOuterLocal { source: 1, depth: 2, index: 3 },
        Instruction::SendMirror {
            primitive: mirror,
            mirror_register: 1,
            subject_register: 2,
            argument_registers: vec![3, 4],
            destination: 5,
            failure: 9,
        },
        Instruction::MakeClosure {
            destination: 1,
            parameters: std::rc::Rc::new(vec![Symbol::intern("x")]),
            code: block,
            copied: vec![2, 300],
        },
        Instruction::MakeArray { destination: 1, element_registers: vec![2, 3, 400] },
        Instruction::MakeVector { destination: 1, size: 9 },
        Instruction::LoadVector { destination: 1, vector: 2, index: 3 },
        Instruction::StoreVector { vector: 1, index: 2, source: 3 },
        Instruction::InitializeSlot { source: 1, own_index: 70_000 },
        Instruction::ReturnSlotIfFilled { own_index: 2 },
        Instruction::MakeFuture { destination: 1, closure: 2 },
        Instruction::ResolveFuture { destination: 1, source: 2 },
        Instruction::LoadOwnSlot { destination: 1, own_index: 3 },
        Instruction::InitializeSlotProvisionally { source: 1, own_index: 3 },
        Instruction::Return { register: 1 },
        Instruction::ImplicitReturn { register: 1 },
        Instruction::NonLocalReturn { register: 1 },
        Instruction::Jump { target: 2 },
        Instruction::JumpIfNotBoolean { register: 1, target: 3 },
        Instruction::BranchIfFalse { register: 1, target: 4 },
        Instruction::BranchIfTrue { register: 1, target: 5 },
        Instruction::Move { destination: 256, source: 1 },
        Instruction::LoadOuterLocal { destination: 1, depth: 2, index: 256 },
    ];
    for operation in [
        IntegerOperation::Add,
        IntegerOperation::Subtract,
        IntegerOperation::Multiply,
        IntegerOperation::FloorDivide,
        IntegerOperation::Less,
        IntegerOperation::Greater,
        IntegerOperation::LessOrEqual,
        IntegerOperation::GreaterOrEqual,
        IntegerOperation::Equal,
    ] {
        instructions.push(Instruction::IntegerBinary {
            operation,
            destination: 1,
            left: 2,
            right: 3,
            target: 6,
        });
    }
    instructions.push(Instruction::IntegerBinary {
        operation: IntegerOperation::Add,
        destination: 1,
        left: 2,
        right: 999,
        target: 6,
    });
    for arguments in [&[][..], &[1][..], &[1, 2][..], &[1, 2, 3][..], &[1, 2, 3, 4, 5, 6, 7, 8][..]] {
        instructions.push(send("public", 4, arguments, 7));
        instructions.push(lexical(arguments));
        instructions.push(superclass(arguments));
        instructions.push(inlinable(arguments));
    }
    round_trip(instructions);
}

#[test]
fn a_patched_target_round_trips_narrow_and_wide() {
    for mut builder in [CodeBuilder::new(), CodeBuilder::always_wide()] {
        let jump = builder.emit(Instruction::Jump { target: 0 });
        let integer = builder.emit(Instruction::IntegerBinary {
            operation: IntegerOperation::Less,
            destination: 1,
            left: 2,
            right: 3,
            target: 0,
        });
        builder.patch_target(jump, 1);
        builder.patch_target(integer, 0);
        let body = builder.finish();
        assert!(matches!(body.instruction(0), Instruction::Jump { target: 1 }));
        assert!(matches!(body.instruction(1), Instruction::IntegerBinary { target: 0, .. }));
    }
}

#[test]
fn past_sixteen_bits_a_table_index_takes_the_wide_form() {
    let mut builder = CodeBuilder::never_wide_by_stress();
    let count = usize::from(u16::MAX) + 2;
    for index in 0..count {
        builder.emit(Instruction::LoadLiteral {
            destination: 1,
            literal: Literal::SmallInteger(i64::from(i32::MAX) + 1 + index as i64),
        });
    }
    let body = builder.finish();
    assert!(matches!(body.slot(0), CompactInstruction::LoadConstant { .. }));
    assert!(matches!(body.slot(count - 1), CompactInstruction::Wide { .. }));
    assert!(matches!(
        body.instruction(count - 1),
        Instruction::LoadLiteral { literal: Literal::SmallInteger(value), .. }
            if value == i64::from(i32::MAX) + count as i64
    ));
}

#[test]
fn call_sites_and_blocks_are_visited_in_emission_order() {
    let block = std::rc::Rc::new(crate::bytecode::CompiledCode::new(
        crate::bytecode::CodeKind::Block,
        vec![Instruction::ImplicitReturn { register: 0 }],
        0,
        1,
        0,
        vec![(0, 0..1)],
        ns_syntax::Span::new(0, 1),
    ));
    for mut builder in [CodeBuilder::new(), CodeBuilder::always_wide()] {
        builder.emit(send("a", 1, &[], 2));
        builder.emit(Instruction::MakeClosure {
            destination: 1,
            parameters: std::rc::Rc::new(vec![]),
            code: block.clone(),
            copied: vec![],
        });
        builder.emit(send("b:c:d:", 1, &[1, 2, 3], 2));
        let mut body = builder.finish();
        let mut seen = Vec::new();
        body.for_each_emitted_mut(|emitted| {
            seen.push(match emitted {
                Emitted::CallSite(site) => {
                    site.number = 40 + seen.len() as u32;
                    "site"
                }
                Emitted::Block(_) => "block",
            });
            Ok::<(), ()>(())
        })
        .expect("the visit never fails");
        assert_eq!(seen, ["site", "block", "site"]);
        assert_eq!(body.instruction(0).call_site(), Some(CallSite { number: 40 }));
        assert_eq!(body.instruction(2).call_site(), Some(CallSite { number: 42 }));
    }
}
```

The tests build blocks with `CompiledCode::new`, which Step 4 adds over today's `instructions` field (Task 5 changes its body, not its signature).

- [ ] **Step 3: Run them to see them fail**

Run: `cargo nextest run -p ns-runtime --lib compact`
Expected: FAIL to compile, "cannot find type `CodeBuilder`".

- [ ] **Step 4: Implement `compact.rs`.** The module doc, the enum, the tables, the builder and the view. Fields are declared narrowest first, because `#[repr(u8)]` lays each variant out as a `repr(C)` struct after its tag.

```rust
//! The encoding a compiled body is stored and run in: one eight-byte
//! `CompactInstruction` per `Instruction`, with everything that does not
//! fit eight bytes in side tables beside them
//! (`docs/design/execution.md`, "Planned: the compact instruction
//! encoding"). `Instruction` stays the vocabulary: a lowering or the image
//! decoder emits one into a `CodeBuilder`, which encodes it at once, and
//! every tool reads a body back through `EncodedBody::instruction`, an
//! exact inverse.

use std::rc::Rc;

use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{CallSite, CompiledCode, Instruction, IntegerOperation, Literal};
use crate::heap::{Immediate, ImmediateKind};
use crate::Symbol;

/// One instruction in eight bytes, matched on by the run loop. A register
/// is a `u8`, a jump target an absolute `u32` instruction index, and
/// anything else an index into one of the body's tables. An instruction
/// whose operands do not fit is a `Wide` (a send has its own wide forms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompactInstruction {
    LoadNil { destination: u8 },
    LoadTrue { destination: u8 },
    LoadFalse { destination: u8 },
    LoadSmallInteger { destination: u8, value: i32 },
    LoadConstant { destination: u8, constant: u16 },
    LoadLiteral { destination: u8, literal: u16 },
    LoadSelf { destination: u8 },
    LoadEnclosingObject { destination: u8, depth: u16 },
    Move { destination: u8, source: u8 },
    StoreLocal { source: u8, slot: u8 },
    LoadOuterLocal { destination: u8, index: u8, depth: u16 },
    StoreOuterLocal { source: u8, index: u8, depth: u16 },
    SendPublic0 { destination: u8, receiver: u8, site: u16 },
    SendPublic1 { destination: u8, receiver: u8, argument: u8, site: u16 },
    SendPublic2 { destination: u8, receiver: u8, arguments: [u8; 2], site: u16 },
    SendPublicWide { site: u32 },
    SendInlinable0 { destination: u8, receiver: u8, site: u16 },
    SendInlinable1 { destination: u8, receiver: u8, argument: u8, site: u16 },
    SendInlinable2 { destination: u8, receiver: u8, arguments: [u8; 2], site: u16 },
    SendInlinableWide { site: u32 },
    SendLexical0 { destination: u8, site: u16 },
    SendLexical1 { destination: u8, argument: u8, site: u16 },
    SendLexical2 { destination: u8, arguments: [u8; 2], site: u16 },
    SendLexicalWide { site: u32 },
    SendSuper0 { destination: u8, site: u16 },
    SendSuper1 { destination: u8, argument: u8, site: u16 },
    SendSuper2 { destination: u8, arguments: [u8; 2], site: u16 },
    SendSuperWide { site: u32 },
    SendMirror { destination: u8, mirror: u8, subject: u8, primitive: MirrorPrimitive, site: u16 },
    MakeClosure { destination: u8, block: u16 },
    MakeArray { destination: u8, elements: u16 },
    MakeVector { destination: u8, size: u16 },
    LoadVector { destination: u8, vector: u8, index: u16 },
    StoreVector { vector: u8, source: u8, index: u16 },
    InitializeSlot { source: u8, own_index: u32 },
    ReturnSlotIfFilled { own_index: u32 },
    MakeFuture { destination: u8, closure: u8 },
    ResolveFuture { destination: u8, source: u8 },
    LoadOwnSlot { destination: u8, own_index: u32 },
    InitializeSlotProvisionally { source: u8, own_index: u32 },
    Return { register: u8 },
    ImplicitReturn { register: u8 },
    NonLocalReturn { register: u8 },
    Jump { target: u32 },
    JumpIfNotBoolean { register: u8, target: u32 },
    BranchIfFalse { register: u8, target: u32 },
    BranchIfTrue { register: u8, target: u32 },
    IntegerAdd { destination: u8, left: u8, right: u8, target: u32 },
    IntegerSubtract { destination: u8, left: u8, right: u8, target: u32 },
    IntegerMultiply { destination: u8, left: u8, right: u8, target: u32 },
    IntegerFloorDivide { destination: u8, left: u8, right: u8, target: u32 },
    IntegerLess { destination: u8, left: u8, right: u8, target: u32 },
    IntegerGreater { destination: u8, left: u8, right: u8, target: u32 },
    IntegerLessOrEqual { destination: u8, left: u8, right: u8, target: u32 },
    IntegerGreaterOrEqual { destination: u8, left: u8, right: u8, target: u32 },
    IntegerEqual { destination: u8, left: u8, right: u8, target: u32 },
    Wide { index: u32 },
}

// The point of the encoding: a slot the loop copies into registers whole.
const _: () = assert!(std::mem::size_of::<CompactInstruction>() == 8);

/// A send's selector and call site, and for a `SendLexical` its depth;
/// for a wide form also its registers.
#[derive(Debug, Clone, PartialEq)]
pub struct SendSite {
    pub selector: Symbol,
    pub call_site: CallSite,
    /// `SendLexical`'s depth; 0 for every other kind.
    pub depth: u16,
    pub wide: Option<Box<WideSend>>,
}

/// The registers of a send that has no eight-byte form.
#[derive(Debug, Clone, PartialEq)]
pub struct WideSend {
    pub destination: u16,
    /// `SendPublic`'s and `SendInlinable`'s receiver; 0 for the others.
    pub receiver: u16,
    pub arguments: Box<[u16]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockSite {
    pub code: Rc<CompiledCode>,
    pub parameters: Rc<Vec<Symbol>>,
    pub copied: Box<[u16]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirrorSite {
    pub arguments: Box<[u16]>,
    pub failure: u32,
}

/// A compiled body as it is stored: the slots and their tables.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EncodedBody {
    slots: Box<[CompactInstruction]>,
    pub(crate) constants: Box<[Immediate]>,
    pub(crate) literals: Box<[Literal]>,
    pub(crate) send_sites: Box<[SendSite]>,
    pub(crate) blocks: Box<[BlockSite]>,
    pub(crate) mirror_sites: Box<[MirrorSite]>,
    pub(crate) register_lists: Box<[Box<[u16]>]>,
    pub(crate) wide: Box<[Instruction]>,
}

/// A send call site or a block, as `EncodedBody::for_each_emitted_mut`
/// visits them: the image decoder numbers a family's sends in emission
/// order, blocks included.
pub enum Emitted<'a> {
    CallSite(&'a mut CallSite),
    Block(&'a mut Rc<CompiledCode>),
}

/// Encodes `Instruction`s as they are emitted.
pub struct CodeBuilder {
    slots: Vec<CompactInstruction>,
    constants: Vec<Immediate>,
    literals: Vec<Literal>,
    send_sites: Vec<SendSite>,
    blocks: Vec<BlockSite>,
    mirror_sites: Vec<MirrorSite>,
    register_lists: Vec<Box<[u16]>>,
    wide: Vec<Instruction>,
    always_wide: bool,
}
```

The builder's behaviour, which the tests above pin:

- `CodeBuilder::new()` and `never_wide_by_stress()` build an empty builder with `always_wide` false (Task 6 makes `new()` read `NS_NEXT_STRESS_WIDE`); `always_wide()` sets it; `impl Default for CodeBuilder` calls `new()`.
- `emit(instruction)` pushes one slot and answers its index. When `always_wide` is set, or `fits_narrow(&instruction)` is false, it takes the wide path: a `SendPublic`, `SendInlinable`, `SendLexical` or `SendSuper` pushes a `SendSite` whose `wide` holds its registers and emits `Send…Wide { site }`; every other instruction is pushed onto `wide` whole and emits `Wide { index }`. Otherwise the narrow path:
  - `LoadLiteral`: `Nil` → `LoadNil`; `Boolean` → `LoadTrue`/`LoadFalse`; `SmallInteger(value)` → `LoadSmallInteger` when `i32::try_from(value)` succeeds, else `LoadConstant` with `Immediate::small_integer(value)` pushed onto `constants`, else (never for a well-formed literal, but total) `LoadLiteral`; `Float(value)` → `LoadConstant` when `Immediate::small_float(value)` is `Some`, else `LoadLiteral`; `String`, `Symbol`, `LargeInteger`, `UnresolvedIdentifier` → `LoadLiteral`, the literal pushed onto `literals`.
  - A send → its `0`, `1` or `2` form by the number of argument registers, a `SendSite { selector, call_site, depth, wide: None }` pushed.
  - `SendMirror` → `SendMirror`, a `MirrorSite { arguments, failure }` pushed. `MakeClosure` → a `BlockSite` pushed. `MakeArray` → its element registers pushed onto `register_lists`.
  - `IntegerBinary` → the opcode of its operation. Every other instruction → the variant of the same name, its `u16` registers narrowed with `as u8` (which `fits_narrow` has checked).
- `fits_narrow(&Instruction) -> bool` is one exhaustive `match` (no `_` arm, so a new `Instruction` variant must decide): every register field that has a `u8` slot field is at most 255 (`LoadOuterLocal`'s and `StoreOuterLocal`'s `index` included; the registers that go into a table -- `copied`, `element_registers`, a mirror send's `argument_registers` -- are not checked); a send has at most two argument registers; and the table the instruction would append to still has fewer than `u16::MAX as usize + 1` entries.
- `patch_target(index, target)`: on `Jump`, `JumpIfNotBoolean`, `BranchIfFalse`, `BranchIfTrue` or any `Integer…` slot, sets `target`; on `SendMirror`, sets its `MirrorSite`'s `failure`; on `Wide { index }`, calls `Instruction::set_target` on `wide[index]`; anything else is `unreachable!("internal: …")`.
- `instruction(index)` / `instructions()` decode exactly as the bullet list above encodes, in reverse: `LoadConstant` → `LoadLiteral` with `Literal::SmallInteger` or `Literal::Float` by `ImmediateKind` (`Nil` and `Boolean`, never emitted there, map to `Literal::Nil` and `Literal::Boolean` so the decoding stays total); `Send…Wide` reads its `SendSite.wide`; `Wide { index }` is `wide[index].clone()`. `EncodedBody` has the same two methods plus `slot(index)`, `len()`, `is_empty()`.
- `finish(self) -> EncodedBody` moves every `Vec` into its boxed slice.
- `EncodedBody::for_each_emitted_mut` walks the slots in order: a send slot (narrow or wide) visits `&mut send_sites[site].call_site`; `MakeClosure` visits `&mut blocks[block].code`; `Wide { index }` visits `wide[index]`'s call site (`Instruction::call_site_mut`) or its `MakeClosure`'s `code`; every other slot visits nothing. The first `Err` the visitor answers stops the walk and is answered.

In `bytecode.rs`, add `CompiledCode::new` with the signature listed at the top, over today's fields for now: it stores `instructions` and derives `family_size` and `outer_reach` with `family_size_of` and `outer_reach_of`, `unwind_marker` `None`. Add `mod compact;` and `pub use compact::{BlockSite, CodeBuilder, CompactInstruction, Emitted, EncodedBody, MirrorSite, SendSite, WideSend};`.

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run -p ns-runtime --lib compact`
Expected: 8 passed (the `past_sixteen_bits` test takes a few seconds in a debug build).

- [ ] **Step 6: Per-commit gate, then commit**

```bash
git add crates/ns-runtime/src/bytecode.rs crates/ns-runtime/src/bytecode/compact.rs crates/ns-runtime/src/compiler/lower.rs
git commit -m "feat: the compact encoding, eight bytes an instruction, and its exact round trip"
```

---

### Task 5: `CompiledCode` stores only the encoding, and the loop runs on it

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (`CompiledCode`, `disassemble`, the test module)
- Modify: `crates/ns-runtime/src/compiler/lower.rs`, `compiler/reference.rs`, `compiler/mod.rs`, `compiler/unwind_marker.rs`
- Modify: `crates/ns-runtime/src/image/decode.rs`, `image/encode.rs`
- Modify: `crates/ns-runtime/src/interpreter/inline_cache.rs:1169`, and the test constructions in `heap.rs`, `interpreter/unwind.rs`, `interpreter/inline_cache.rs`
- Modify: `crates/ns-runtime/src/vm.rs`
- Modify: `crates/ns-runtime/tests/{compiler,image,reference_lowering,trampoline,vm,heap}.rs`

**Interfaces:**
- Consumes: Task 4's `CodeBuilder`, `EncodedBody`, `Emitted`, `CompactInstruction`; Task 3's `Value::from_immediate`.
- Produces: the `CompiledCode` interface listed at the top.

This task is one commit: the representation and the loop change together, because no intermediate state runs at a reasonable speed (a loop that decodes the view per instruction allocates on every send).

- [ ] **Step 1: `CompiledCode` holds an `EncodedBody`.** Replace the field `pub instructions: Vec<Instruction>` by `body: EncodedBody` (private), keep every other field as it is, and add:

```rust
impl CompiledCode {
    /// A body from its instructions, encoded at once; `family_size` and
    /// `outer_reach` are derived from them, as the compiler derives them.
    pub fn new(
        kind: CodeKind,
        instructions: Vec<Instruction>,
        argument_count: u16,
        register_count: u16,
        copied_count: u16,
        statement_program_counter: Vec<(usize, Range<usize>)>,
        source: Span,
    ) -> CompiledCode {
        let mut builder = CodeBuilder::new();
        for instruction in instructions {
            builder.emit(instruction);
        }
        CompiledCode::from_builder(
            kind,
            builder,
            argument_count,
            register_count,
            copied_count,
            statement_program_counter,
            source,
        )
    }

    pub(crate) fn from_builder(
        kind: CodeKind,
        builder: CodeBuilder,
        argument_count: u16,
        register_count: u16,
        copied_count: u16,
        statement_program_counter: Vec<(usize, Range<usize>)>,
        source: Span,
    ) -> CompiledCode {
        let instructions = builder.instructions();
        CompiledCode {
            kind,
            family_size: family_size_of(&instructions),
            outer_reach: outer_reach_of(&instructions),
            body: builder.finish(),
            argument_count,
            register_count,
            copied_count,
            statement_program_counter,
            unwind_marker: None,
            source,
        }
    }

    /// The instruction at `index`, decoded: what was emitted there.
    pub fn instruction(&self, index: usize) -> Instruction {
        self.body.instruction(index)
    }

    /// Every instruction, decoded -- for tools and tests; the loop reads
    /// `slot`.
    pub fn instructions(&self) -> Vec<Instruction> {
        self.body.instructions()
    }

    pub fn instruction_count(&self) -> usize {
        self.body.len()
    }

    pub(crate) fn slot(&self, index: usize) -> CompactInstruction {
        self.body.slot(index)
    }

    pub(crate) fn body(&self) -> &EncodedBody {
        &self.body
    }

    pub(crate) fn body_mut(&mut self) -> &mut EncodedBody {
        &mut self.body
    }
}
```

`disassemble`/`write_code` iterate `code.instructions()`. The tests of `bytecode.rs` and Task 4's two test blocks build through `CompiledCode::new`.

- [ ] **Step 2: Both lowerings emit into a builder.** In `lower.rs`: the field `instructions: Vec<Instruction>` becomes `code: CodeBuilder`; `emit` is `self.code.emit(instruction)`; every `self.instructions.len()` is `self.code.len()`; `patch_target` is `self.code.patch_target(instruction_index, target)`; the `MakeVector` pushed in the constructor (`lower.rs:386-393`) is emitted into the new builder; `finish`'s `debug_assert!` over the targets reads `self.code.instructions()`, and `finish` answers `CompiledCode::from_builder(self.kind, self.code, self.argument_count, self.max_register, self.layout.copied.len() as u16, self.statement_program_counter, source)`. The same in `reference.rs` (`check_shape(&self.code.instructions(), self.first_scratch)` in debug builds; `from_builder` with `copied_count` 0 and `self.next_register`). `compiler/mod.rs`'s three constructions and `unwind_marker.rs`'s two reads go through `CompiledCode::new`/`from_builder` and `code.instructions()`.

- [ ] **Step 3: The image decoder emits into a builder; the encoder reads the view.** In `decode.rs`:
  - `decode_code` emits each `decode_instruction` into a `CodeBuilder` and answers `CompiledCode::from_builder(…)` (the `family_size` placeholder comment stays true: it is recomputed after numbering).
  - `block_nesting`, `lexical_parents`, `code_index` and every other walk over `code.instructions` iterate `code.instructions()`.
  - `number_call_sites` becomes a walk over `Rc::make_mut`'d bodies through `code.body_mut().for_each_emitted_mut(…)`: a `CallSite` that is not `UNCACHED` gets `*next` and `*next += 1`; a `Block` captures its table index from `index_of` *before* `Rc::make_mut`, recurses, and is pushed onto `writeback` -- exactly today's order and checks. After the walk, `code.family_size = family_size_of(&code.instructions())`.
  - `verify_body` reads `code.instructions()` once into a local and checks it as today; `DecodeError::ProgramCounterOutOfRange` compares against `instruction_count()`.

  In `encode.rs`, `collect_code`, `encode_code` and `compiled_uncached` iterate `code.instructions()`. The bytes written are unchanged: `tests/image.rs` pins them and must pass untouched in what it asserts.

- [ ] **Step 4: Every other reader.** `inline_cache.rs:1169` (`states_of`) iterates `body.instructions()`. Every test that read `code.instructions` reads `code.instructions()` (bind it to a local where a pattern borrows from it, e.g. `let instructions = code.instructions(); let Some(Instruction::MakeClosure { code: block, .. }) = instructions.first() else { … };`), `code.instructions[index]` becomes `code.instruction(index)`, and every `CompiledCode { … }` literal in a test becomes `CompiledCode::new(kind, instructions, argument_count, register_count, copied_count, statement_program_counter, source)` -- a literal that set `family_size` or `outer_reach` by hand had them derived the same way, which the test then still asserts.

- [ ] **Step 5: Run the compiler and image suites before touching the loop**

Run: `cargo nextest run -p ns-runtime --lib compiler bytecode image`
Expected: compile errors only in `vm.rs` (it still reads `code.instructions[…]`); fix nothing else to silence them.

- [ ] **Step 6: The loop on the slots.** In `vm.rs`, `run_stretch` matches `code.slot(program_counter)` (a `CompactInstruction`, copied out). The `debug_assert!` at the loop's head compares against `code.instruction_count()`. Then:
  - **Each instruction's effect is written once**, as an `#[inline(always)]` function over `u16` operands that today's arm body already almost is (for example `fn load_outer_local_into(mutator, activation, register_base, destination: u16, depth: u16, index: u16)`); the narrow arm widens its `u8`s with `u16::from` and calls it, and `run_wide` (below) calls the same function. No effect is written twice.
  - **Loads.** `LoadNil`/`LoadTrue`/`LoadFalse` write `Value::nil()`/`Value::boolean(…)`; `LoadSmallInteger` writes `Value::small_integer(i64::from(value))`; `LoadConstant` writes `Value::from_immediate(code.body().constants[usize::from(constant)])`; `LoadLiteral` calls `make_value` on `&code.body().literals[usize::from(literal)]`, as today.
  - **Sends.** Every send arm builds a `PendingSend` and leaves the match:

```rust
/// A send an arm gathered: what to send and where its value goes. The
/// arms of one kind differ only in where their argument registers are;
/// the kind's one path, after the match, runs it, so that each kind's
/// cache hit path is inlined into `vm::run` once.
struct PendingSend {
    kind: SendKind,
    destination: u16,
    /// `SendPublic`'s and `SendInlinable`'s receiver register.
    receiver: u16,
    site: u32,
    arguments: ArgumentRegisters,
}

#[derive(Clone, Copy)]
enum SendKind {
    Public,
    Inlinable,
    Lexical,
    Super,
}

#[derive(Clone, Copy)]
enum ArgumentRegisters {
    /// Up to two, from the slot.
    Inline { count: u8, registers: [u16; 2] },
    /// From the site's `WideSend`.
    Site,
}
```

    After the match, `match pending.kind` calls today's `send(…)` once per kind with today's `decide` closure (`send_public`, `mutator.decide_send` with `inlinable` true, `send_lexical` with the site's `depth`, `send_super`), the selector and `CallSite` read from `code.body().send_sites[pending.site as usize]`, and the argument registers as a `&[u16]`: `&registers[..usize::from(count)]` for `Inline`, `&site.wide…arguments` for `Site`. `send`'s signature does not change. The match's other arms keep answering a `Transfer` or `continue`; introduce `enum Arm { Transfer(Transfer), Send(PendingSend) }` as the match's value if that is the clearest way to keep both.
  - **`SendMirror`** keeps today's arm, its arguments and `failure` read from `code.body().mirror_sites[usize::from(site)]`.
  - **`MakeClosure`** reads its `BlockSite`; **`MakeArray`** its `register_lists` entry; the integer opcodes call today's `IntegerBinary` arm body with the operation fixed by the opcode (one `#[inline(always)]` function taking the operation as a parameter, called with a constant per arm, so each arm compiles to its own operation).
  - **`Wide { index }`** calls `#[inline(never)] fn run_wide(…) -> WideOutcome` on `&code.body().wide[index as usize]`, where `enum WideOutcome { Next, Jump(u32), Transfer(Transfer) }`; it matches the `Instruction` exhaustively and calls the same effect functions as the narrow arms. A send never reaches it (the builder gives sends their own wide forms): its four send arms are `unreachable!("internal: a send is never encoded through Wide")`. `WideOutcome::Jump` goes through `jump_to!`, so a backward wide jump still tests the safepoint.
  - **`send_destination`, `initialize_slot_refused`, `simultaneous_clause_instruction`** stop re-reading `code.instructions[program_counter]`: `send_destination(code: &CompiledCode, program_counter: usize) -> u16` matches the slot (every send form, the wide ones through their `SendSite`, `SendMirror`, and a `Wide` whose entry is a `SendMirror`); the other two take the operands their arm already has.

- [ ] **Step 7: Run the whole suite**

Run: `cargo nextest run --workspace --exclude ns-lsp`
Expected: every test passes, every golden under both lowerings, with no golden changed.

- [ ] **Step 8: Check the loop's shape.** Build a pinned release binary with debug information (`$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"; $env:CARGO_PROFILE_RELEASE_DEBUG = "limited"; cargo build --release -p ns-cli`) and run `dumpbin /disasm` on it: one `ns_runtime::vm::run`, no `vm::run::closure`, no `run_stretch`, `run_wide` a function of its own; record `vm::run`'s instruction count and stack frame size for Task 7. Clear both environment variables afterwards.

- [ ] **Step 9: Per-commit gate, then commit**

```bash
git add -A crates/ns-runtime
git commit -m "feat: a compiled body is stored and run as eight-byte instructions, read back through a decoded view"
```

---

### Task 6: The wide forms run, under a stress mode and in goldens

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs` (the switch), `crates/ns-runtime/src/bytecode/compact.rs` (`CodeBuilder::new` reads it)
- Create: `crates/ns-runtime/tests/compact_encoding.rs`
- Create: `goldens/runtime/encoding/three_hundred_registers.ns` and `.toml`, `goldens/runtime/encoding/sends_of_three_to_eight_arguments.ns` and `.toml`
- Modify: `.claude/skills/finishing-a-batch/SKILL.md`

**Interfaces:**
- Consumes: `CodeBuilder` (Task 4), the loop (Task 5).
- Produces: `NS_NEXT_STRESS_WIDE`.

- [ ] **Step 1: The switch.** In `safepoint.rs`, beside `stress_caches_from_env`:

```rust
/// Whether `NS_NEXT_STRESS_WIDE` asks for every instruction in a wide
/// form: set, and neither empty nor `0`. Debug builds only, like the other
/// stress modes; read once per process, since every body asks.
pub(crate) fn stress_wide_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        && *REQUESTED.get_or_init(|| {
            std::env::var_os("NS_NEXT_STRESS_WIDE")
                .is_some_and(|value| !value.is_empty() && value != "0")
        })
}
```

and `CodeBuilder::new()` sets `always_wide: crate::interpreter::safepoint::stress_wide_from_env()`.

- [ ] **Step 2: Run the suite under it**

Run (Git Bash): `NS_NEXT_STRESS_WIDE=1 cargo nextest run --workspace --exclude ns-lsp`
Expected: every test passes (the tests of narrow forms build with `never_wide_by_stress()`, so they assert the encoding and not the mode). A failure is a defect in a wide path: fix it, never the test.

- [ ] **Step 3: The goldens.** Generate `three_hundred_registers.ns` (Git Bash):

```bash
mkdir -p goldens/runtime/encoding
{
  echo "(* A method of more than 255 registers runs: every instruction that"
  echo "   names a register above 255 takes the wide form. *)"
  echo "class ThreeHundredRegisters = Object new ("
  echo ") ("
  echo "    public main: platform args: arguments = ("
  printf "        |"; for index in $(seq 1 300); do printf " t%d" "$index"; done; echo " |"
  for index in $(seq 1 300); do echo "        t$index:: $index."; done
  echo "        platform transcript show: (t1 + t150 + t300) printString; newLine."
  echo "        platform transcript show: [t300 + t299] value printString; newLine."
  echo "    )"
  echo ") : ()"
} > goldens/runtime/encoding/three_hundred_registers.ns
```

with `three_hundred_registers.toml`:

```toml
status = 0
stdout = '''
451
599
'''
stderr = ''
```

and `sends_of_three_to_eight_arguments.ns`, written by hand:

```newspeak
(* Sends of three to eight arguments, explicit, implicit-receiver and
   super, each the wide form of its kind. *)
class SendsOfThreeToEightArguments = Object new (
) (
    class Base = Object new (
    ) (
        public a: one b: two c: three = ( ^one + two + three )
    ) : ()
    class Derived = Base new (
    ) (
        public a: one b: two c: three = ( ^(super a: one b: two c: three) * 10 )
    ) : ()
    public sum: one with: two with: three with: four = ( ^one + two + three + four )
    public sum: one with: two with: three with: four with: five with: six with: seven with: eight = (
        ^one + two + three + four + five + six + seven + eight
    )
    public main: platform args: arguments = (
        | transcript = platform transcript. |
        transcript show: (Derived new a: 1 b: 2 c: 3) printString; newLine.
        transcript show: (sum: 1 with: 2 with: 3 with: 4) printString; newLine.
        transcript show: (self sum: 1 with: 2 with: 3 with: 4 with: 5 with: 6 with: 7 with: 8) printString; newLine.
    )
) : ()
```

with its `.toml`:

```toml
status = 0
stdout = '''
60
10
36
'''
stderr = ''
```

Run: `cargo nextest run -p ns-cli encoding`
Expected: both goldens pass under both lowerings. Before trusting the expected output, check each line by hand against the program (`CLAUDE.md`: an outcome is established, never copied from what the code printed).

- [ ] **Step 4: Bodies past sixteen bits.** `tests/compact_encoding.rs` builds three modules with `format!` (each a `main:args:` of 65,537 statements) and runs each under both lowerings as below:
  - 65,537 assignments of distinct integer literals beyond `i32` (`x:: 3000000000.`, `x:: 3000000001.`, …), then `^x`: the last constant is in a `Wide`, and the answer is `3000065536`;
  - 65,537 sends `x:: x + 1.` with `x` starting at `0` (under both lowerings the body has 65,537 send sites: the reference lowering's uncached `SendPublic`, the optimized one's fallback send behind each `IntegerAdd`), answering `65537`;
  - 65,537 string literals `s:: 'a'.`, then `^s size`, answering `1`.

  Each method is `public run = ( | x | … )` on a class built with `indoc!`-free `format!` (the source is generated, not written inline), loaded, instantiated and sent `run` with `tests/vm.rs`'s helpers (`load`, `new_instance`, `send`, `expect_ok`, `integer_of`, copied into the new file), under both lowerings as `tests/reference_lowering.rs` selects them. Each test asserts the answer, and that the method's `CompiledCode::instruction_count()` is what the source implies; each runs in a few seconds in a debug build.

Run: `cargo nextest run -p ns-runtime --test compact_encoding`
Expected: 3 passed.

- [ ] **Step 5: The skill runs the fourth stress mode.** In `.claude/skills/finishing-a-batch/SKILL.md`, Step 2: the loop becomes `for mode in MOVES CACHES COLLECT WIDE`, the prose names WIDE ("WIDE encodes every instruction in its wide form"), and `NS_NEXT_STRESS_WIDE=1 cargo run -p ns-cli -- run-corpus corpus/modules` joins the three `run-corpus` lines.

- [ ] **Step 6: Per-commit gate, then commit**

```bash
git add crates/ns-runtime goldens/runtime/encoding .claude/skills/finishing-a-batch/SKILL.md
git commit -m "test: every wide form runs, under NS_NEXT_STRESS_WIDE and in goldens past every narrow limit"
```

---

### Task 7: Measured against the tree the branch started from

**Files:**
- Modify: `docs/design/execution.md` (a new section "What the compact instruction encoding moved", after "What the eight-byte value moved")

**The controller session runs this task itself**, as Task 1's measurement: one background command with a one-hour timeout, nothing else on the machine.

- [ ] **Step 1: Build and check** three pinned binaries of each tree exactly as Task 1, Step 5, with `git merge-base master feature/compact-instruction-encoding` as "before" (in a `git worktree` under `.temp\measure\before`) and the branch head as "after"; the same checks.
- [ ] **Step 2: Measure** with Task 1's `measure.ps1` (three runs of the five slow programs), `$directory = ".temp\measure"`.
- [ ] **Step 3: Write the section**: date, the two commits, the machine, the protocol in one paragraph, a table per source (before, after, change, both ranges), `vm::run`'s instruction count and frame from Task 5, Step 8, against `master`'s, Task 1's gate numbers beside the result, and what the measurement does not cover. A source that got slower is named, with what the measurement shows about why or that it does not show it.
- [ ] **Step 4: Commit** (Markdown only: no gate)

```bash
git add docs/design/execution.md
git commit -m "docs: what the compact instruction encoding moved, on all sixteen programs and the microbenchmarks"
```

---

### Task 8: The documentation describes the built state

**Files:**
- Modify: `docs/design/execution.md` ("Planned: the compact instruction encoding" becomes a section on the encoding as built, its reasons kept; "Where the 3% goes"' last paragraph and "The VM" name the slot loop)
- Modify: `docs/design/compiler.md` ("The instruction set": `Instruction` is the vocabulary a lowering emits into a `CodeBuilder`, the stored form is the encoding; "Compiled code"'s struct; "Safepoints and call-site identity": the sites live in `send_sites` and `wide`)
- Modify: `docs/design/image-format.md` (the wire format still carries `Instruction`s and a frame's `program_counter` is still an instruction index, because one instruction is one slot)
- Modify: `docs/design/memory.md` (the decode amplification figures, if any of them counted `Instruction`'s 48 bytes: recompute from the new sizes and say how)
- Modify: `docs/design/README.md` (the map's row for activations and the VM loop)
- Modify: `CLAUDE.md` (the `crates/ns-runtime` paragraph names `bytecode/compact.rs`; the stress modes are four where the text counts them)
- Modify: `docs/PROGRESS.md` (a milestone "The compact instruction encoding (2026-10-…)" listing every commit; "Where the next session starts" names road step 3, the object space), `docs/OPEN-WORK.md` ("Next" and the road's item marked built; anything the reviews or Task 7 leave open added)

- [ ] **Step 1: Update each document** as listed, describing the current state only (`CLAUDE.md`, "Conventions").
- [ ] **Step 2: Check for stale statements**

Run: `rg -n "Vec<Instruction>|code\.instructions\b|48 bytes|instructions\[program_counter\]|not designed yet|compact instruction encoding" docs CLAUDE.md`
Expected: only sentences that are still true, each checked by hand; plans under `docs/superpowers/plans/` stay as written.

- [ ] **Step 3: Commit** (Markdown only: no gate)

```bash
git add docs CLAUDE.md
git commit -m "docs: the compact instruction encoding is built"
```

---

### Task 9: The end-of-batch gates

- [ ] **Step 1:** Run the `finishing-a-batch` skill in full -- now with four stress modes -- and report every summary line.
- [ ] **Step 2:** The final whole-branch review (on Fable), then hand the branch back to the user for the merge.
