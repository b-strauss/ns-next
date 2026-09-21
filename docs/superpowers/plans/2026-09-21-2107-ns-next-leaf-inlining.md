# Leaf Inlining Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give integer arithmetic and comparison an inlined fast path in compiled
code, guarded at run time and falling back to the ordinary send, so that a leaf
operation costs no send when both operands are integers.

**Architecture:** The compiler already inlines the control-flow special selectors
with a runtime guard and a fallback to a real send
(`compiler/control_flow.rs` recognizes, `compiler/lower.rs` emits). This adds the
same shape for the integer leaves: one new instruction that performs the
operation when both operands are `Value::SmallInteger` and a `checked_*`
operation yields a result, and jumps to a fallback otherwise. The fallback is
the ordinary send that exists today, so overflow, a zero divisor, non-integer
receivers, user subclasses and every error text stay in `builtins/` and are
never duplicated into `vm.rs` or `compiler/`. The reference lowering is untouched and keeps running every send
uncached, which is what makes it the oracle for this change.

**Tech Stack:** Rust 1.98, edition 2021, no new dependencies.

**Spec:** `docs/design/core-library.md`, "Planned: the core library as Newspeak
source over a VM mirror" — specifically the paragraph beginning "The cost,
stated plainly", which sequences this work first, and the older paragraph
naming Cog's `bytecodePrimAdd` as the model. Background on the existing guarded
inlining: `docs/design/compiler.md`, "Control-flow inlining".

## Global Constraints

- **Rust 1.98.0, edition 2021.** No new external dependency without a reason
  that survives review; this plan adds none.
- **All code, identifiers, comments and commit messages in English.**
- **Names are written out.** No identifier of one or two characters and no
  truncated word: `instruction`, not `instr`; `destination`, not `dest`;
  `left_register`, not `l`. Clippy's `min_ident_chars` (threshold 2, inclusive)
  enforces it.
- **`vm.rs` holds no dispatch logic and no error text.** The new instruction
  computes or jumps; it never raises a Newspeak error and never decides a
  lookup. Every failure path goes to the fallback send.
- **A wire tag is assigned once and never renumbered or reused.** The highest
  instruction opcode today is `OPCODE_STORE_VECTOR = 32`
  (`crates/ns-runtime/src/image/opcodes.rs`), so the new instruction takes
  **33**. `crates/ns-runtime/tests/opcode_tags.rs` pins every value.
- **The reference lowering is not changed.** `compiler/reference.rs` keeps
  emitting an ordinary send for these selectors. It is the oracle.
- **Program outcomes are pinned.** Every golden under `goldens/<category>/*.ns`
  must keep its recorded `status`, `stdout` and `stderr` byte for byte, under
  both lowerings. A change to a golden's recorded output is a behaviour change
  and is not part of this plan.
- **Gates that must stay green:** `cargo test --workspace`, the same suite under
  `NS_NEXT_STRESS_MOVES=1`, `NS_NEXT_STRESS_CACHES=1` and
  `NS_NEXT_STRESS_COLLECT=1`; `cargo clippy --workspace --all-targets`
  warning-free; `cargo fmt --all` before every commit;
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reporting
  "parsed 210, failed 0, expected-fail 1, stale 0";
  `cargo run -p ns-cli -- run-corpus corpus/modules` reporting
  "ran 6, failed 0, skipped 205", also under all three stress modes.
- **A Newspeak module written inline in a test is an `indoc! {"…"}`**, indented
  with the Rust around it, closing `"}` on a line of its own.
- **Commit messages use conventional commits** (`feat:`, `fix:`, `perf:`,
  `test:`, `docs:`, `chore:`) and end with:

  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01TZ3x6EPM5Z3bMHr8Yw36rc
  ```

## Which operations are inlined

**Every binary integer primitive there is.** `IntegerPrimitive`
(`crates/ns-runtime/src/builtins/primitive.rs:52`) declares twelve operations,
of which nine are binary; all nine are inlined. The other three — `Negated`,
`Absolute`, `PrintString` — are unary and outside this plan's shape.

| Selector | Operation | Result |
|---|---|---|
| `+` | `checked_add` | `Value::SmallInteger` |
| `-` | `checked_sub` | `Value::SmallInteger` |
| `*` | `checked_mul` | `Value::SmallInteger` |
| `//` | `checked_div_euclid` | `Value::SmallInteger` |
| `<` | `<` | `Value::Boolean` |
| `>` | `>` | `Value::Boolean` |
| `<=` | `<=` | `Value::Boolean` |
| `>=` | `>=` | `Value::Boolean` |
| `=` | `==` | `Value::Boolean` |

**One rule covers every failure, and it is why no operation needs excluding.**
Each `checked_*` answers `None` where the operation has no integer result —
`checked_add` on overflow, `checked_div_euclid` on a zero divisor *and* on
`i64::MIN // -1` — and `None` takes the fallback. So the error and its exact
text are still decided by `builtins/numbers.rs` and raised by the primitive:
"division by zero" for the zero divisor, the overflow message for the rest.
Nothing about rounding, division by zero or overflow is restated here, which is
what `vm.rs` must not do.

**`/` is not in the table because it does not exist on `Integer`.** It is
declared only as `FloatPrimitive::Divide`; an integer receiver has no `/` at
all, so there is nothing to inline and a guard for it would be a fast path to
a lookup failure — `7 / 2` ends in `doesNotUnderstand:` today. That is a known
gap, recorded in `docs/design/core-library.md`, "Gap: an integer has no `/`",
and it is **not** this plan's to close: adding `Integer>>/` means deciding what
an exact quotient answers without a `Fraction` class, which belongs to step 6.
Do not add it here, and do not treat its absence as an oversight to fix in
passing.

## File Structure

- `crates/ns-runtime/src/compiler/control_flow.rs` — gains the recognizer for
  the nine binary selectors, beside the existing control-flow one. Recognition
  only; no emission.
- `crates/ns-runtime/src/bytecode.rs` — gains `Instruction::IntegerBinary` and
  its `IntegerOperation` enumeration, plus the `disassemble` arm.
- `crates/ns-runtime/src/image/opcodes.rs` — gains `OPCODE_INTEGER_BINARY = 33`.
- `crates/ns-runtime/src/image/encode.rs`, `decode.rs` — encode and decode it.
- `crates/ns-runtime/src/vm.rs` — gains the execution arm: compute or jump.
- `crates/ns-runtime/src/compiler/lower.rs` — gains `lower_integer_binary`,
  called from `lower_send`'s inlining gate.
- `crates/ns-runtime/tests/opcode_tags.rs` — pins tag 33.
- `goldens/arithmetic/` — new goldens for the fallback paths.

---

### Task 1: Recognize the nine binary selectors

**Files:**
- Modify: `crates/ns-runtime/src/compiler/control_flow.rs`
- Test: same file, its `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `control_flow::is_ordinary_send`, which already gates inlining.
- Produces: `pub enum IntegerOperation`, declared in `bytecode.rs` and used
  here. It must be `pub`, not `pub(crate)`: Task 2 makes it a field of
  `Instruction`, which is `pub enum Instruction`, and a `pub(crate)` field type
  in a public enum does not compile. Also
  `pub(crate) fn recognize_integer_binary(send: &ast::Send) -> Option<IntegerOperation>`.

- [ ] **Step 1: Write the failing test**

Add to `control_flow.rs`'s test module. `parse_send` already exists there.

```rust
#[test]
fn every_binary_integer_primitive_is_recognized() {
    // All nine binary members of `IntegerPrimitive`. If a tenth is ever
    // added, this test is where its absence shows.
    let cases = [
        ("1 + 2", IntegerOperation::Add),
        ("1 - 2", IntegerOperation::Subtract),
        ("1 * 2", IntegerOperation::Multiply),
        ("1 // 2", IntegerOperation::FloorDivide),
        ("1 < 2", IntegerOperation::Less),
        ("1 > 2", IntegerOperation::Greater),
        ("1 <= 2", IntegerOperation::LessOrEqual),
        ("1 >= 2", IntegerOperation::GreaterOrEqual),
        ("1 = 2", IntegerOperation::Equal),
    ];
    for (source, expected) in cases {
        let send = parse_send(source);
        assert_eq!(
            recognize_integer_binary(&send),
            Some(expected),
            "expected `{source}` to be recognized"
        );
    }
}

#[test]
fn a_selector_without_an_integer_primitive_is_not_recognized() {
    // `/` exists only as a float primitive, so an integer receiver has no `/`
    // to reach: inlining it would be a fast path to a lookup failure. The
    // others are not binary sends at all.
    for source in ["1 / 2", "1 max: 2", "1 printString"] {
        let send = parse_send(source);
        assert_eq!(
            recognize_integer_binary(&send),
            None,
            "expected `{source}` not to be recognized"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ns-runtime control_flow::tests::every_inlinable_binary_selector_is_recognized`

Expected: FAIL to compile, `cannot find function 'recognize_integer_binary'`.

- [ ] **Step 3: Declare the operation enumeration**

In `crates/ns-runtime/src/bytecode.rs`, beside the other bytecode types:

```rust
/// The integer leaves the compiler emits inline, guarded, with the ordinary
/// send as the fallback: every binary member of `IntegerPrimitive`. Each one
/// is computed with a `checked_*` operation, so the cases that have no
/// integer result -- overflow, a zero divisor -- answer `None` and take the
/// fallback, leaving every error and its text to `builtins/numbers.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerOperation {
    Add,
    Subtract,
    Multiply,
    FloorDivide,
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
}
```

- [ ] **Step 4: Write the recognizer**

In `crates/ns-runtime/src/compiler/control_flow.rs`, after `recognize`:

```rust
/// The arithmetic counterpart to `recognize`: which binary send has an
/// inlined fast path. Pure recognition, like `recognize` above -- the
/// emission is `lower.rs`'s `lower_integer_binary`.
///
/// The gate in front of this is the same `is_ordinary_send`: a `super` or
/// lexical send is never inlined, because its lookup does not start where an
/// explicit-receiver send's does.
pub(crate) fn recognize_integer_binary(send: &ast::Send) -> Option<IntegerOperation> {
    let ast::Selector::Binary(selector) = &send.message.selector else {
        return None;
    };
    match selector.as_str() {
        "+" => Some(IntegerOperation::Add),
        "-" => Some(IntegerOperation::Subtract),
        "*" => Some(IntegerOperation::Multiply),
        "//" => Some(IntegerOperation::FloorDivide),
        "<" => Some(IntegerOperation::Less),
        ">" => Some(IntegerOperation::Greater),
        "<=" => Some(IntegerOperation::LessOrEqual),
        ">=" => Some(IntegerOperation::GreaterOrEqual),
        "=" => Some(IntegerOperation::Equal),
        _ => None,
    }
}
```

Add `use crate::bytecode::IntegerOperation;` to the module's imports.
`ast::Selector` is `Unary(String) | Binary(String) | Keyword(String)`
(`crates/ns-syntax/src/ast.rs:156`), so the `let … else` above matches the
variant as written.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime control_flow::`

Expected: PASS, both new tests among them.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
git add crates/ns-runtime/src/bytecode.rs crates/ns-runtime/src/compiler/control_flow.rs
git commit -m "feat: recognize the integer leaves that get an inlined fast path"
```

---

### Task 2: The instruction, its wire tag, and its round trip

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs`
- Modify: `crates/ns-runtime/src/image/opcodes.rs`
- Modify: `crates/ns-runtime/src/image/encode.rs`
- Modify: `crates/ns-runtime/src/image/decode.rs`
- Test: `crates/ns-runtime/tests/opcode_tags.rs`

**Interfaces:**
- Consumes: `IntegerOperation` from Task 1.
- Produces: `Instruction::IntegerBinary { operation, destination, left, right, target }`,
  where `target` is the instruction index to jump to when the fast path does not
  apply, and `OPCODE_INTEGER_BINARY: u8 = 33`.

- [ ] **Step 1: Write the failing tag test**

In `crates/ns-runtime/tests/opcode_tags.rs`, beside the existing assertions:

```rust
#[test]
fn the_integer_binary_opcode_keeps_its_tag() {
    // A tag is assigned once and never renumbered: a stale artifact must hit
    // an unknown tag and be refused, never decode into a different, valid
    // program.
    assert_eq!(ns_runtime::image::opcodes::OPCODE_INTEGER_BINARY, 33);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --test opcode_tags the_integer_binary_opcode_keeps_its_tag`

Expected: FAIL to compile, `OPCODE_INTEGER_BINARY` not found.

- [ ] **Step 3: Add the instruction variant**

In `crates/ns-runtime/src/bytecode.rs`, in `enum Instruction`, appended after
the existing variants:

```rust
/// An integer leaf's inlined fast path. Performs `operation` on `left` and
/// `right` into `destination` when both hold `Value::SmallInteger` and the
/// result does not overflow; otherwise jumps to `target`, where the ordinary
/// send stands. Every error -- overflow, a non-integer operand, a user
/// subclass -- is raised by that send's primitive, never here, so there is
/// one source for each of them.
IntegerBinary {
    operation: IntegerOperation,
    destination: u16,
    left: u16,
    right: u16,
    target: u32,
},
```

- [ ] **Step 4: Add the disassembly arm**

In the same file's `disassemble`, matching the neighbouring arms' style:

```rust
Instruction::IntegerBinary {
    operation,
    destination,
    left,
    right,
    target,
} => format!("IntegerBinary {operation:?} r{destination} <- r{left}, r{right} else -> {target}"),
```

- [ ] **Step 5: Add the opcode tag**

In `crates/ns-runtime/src/image/opcodes.rs`, after `OPCODE_STORE_VECTOR`:

```rust
/// An integer leaf's inlined fast path (`Instruction::IntegerBinary`).
pub const OPCODE_INTEGER_BINARY: u8 = 33;
```

- [ ] **Step 6: Encode and decode it**

In `encode.rs`, following the shape of the arm that writes `Jump`'s `target`,
write the opcode, the operation as a `u8` with its own exhaustive `match` (so a
new operation fails to compile rather than silently encoding as another), then
`destination`, `left`, `right` as `u16` and `target` as `u32`.

In `decode.rs`, read them back in the same order, and **reject an unknown
operation byte** with the module's existing malformed-instruction error rather
than defaulting to one. Bound `target` exactly as the existing jump targets are
bounded — an image is untrusted input, so a target outside the code's
instruction range must be refused before it can be jumped to.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test opcode_tags && cargo test -p ns-runtime image::`

Expected: PASS.

- [ ] **Step 8: Check `outer_reach_of` needs nothing**

Open `bytecode::outer_reach_of` and confirm the new instruction is **not**
added: it reads and writes only registers of the activation executing it, never
another frame's by reference, so it is outside that function's subject. Record
that in a one-line comment on the new variant if it is not already obvious from
the text above.

- [ ] **Step 9: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
git add crates/ns-runtime/src/bytecode.rs crates/ns-runtime/src/image/ crates/ns-runtime/tests/opcode_tags.rs
git commit -m "feat: add the IntegerBinary instruction and its wire tag"
```

---

### Task 3: Execute it in the VM

**Files:**
- Modify: `crates/ns-runtime/src/vm.rs`
- Test: `crates/ns-runtime/src/vm.rs`'s test module, or the neighbouring
  `vm_*.rs` test file that matches how instruction tests are organized there

**Interfaces:**
- Consumes: `Instruction::IntegerBinary` from Task 2.
- Produces: nothing new; the arm returns the existing `Flow::Next` or
  `Flow::Jump(target)`.

- [ ] **Step 1: Write the failing test**

Test through compiled source rather than a hand-built code object, so the test
survives a change to the lowering. Use the crate's existing `expect_ok` helper
(copy the one in a neighbouring test file — `EvaluationResult` has no `Debug`,
so `unwrap` does not compile).

```rust
#[test]
fn an_integer_addition_takes_the_fast_path_and_an_overflow_takes_the_fallback() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Arithmetic usingPlatform: platform = Object new (
        ) (
          public main: p args: a = (
            p transcript show: (2 + 3) printString; newLine.
            p transcript show: (9223372036854775807 + 1) printString; newLine
          )
        ) : ()
    "};
    // The first line answers 5 through the inlined path; the second overflows,
    // takes the fallback, and raises the primitive's own error -- the one
    // `builtins/numbers.rs` raises, with its text unchanged.
    // Assert on the run's stdout and on its error, as the neighbouring tests do.
}
```

Fill the assertion in using whatever harness the neighbouring VM tests use to
run a module and capture `stdout`/`stderr`; `crates/ns-cli/tests/` has the
end-to-end shape if `ns-runtime`'s own tests do not.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime an_integer_addition_takes_the_fast_path`

Expected: FAIL — the lowering does not emit the instruction yet (Task 4), so the
test passes through the ordinary send and the assertion on the disassembly or
on the instruction count fails. If the test as written cannot fail before
Task 4, move it to Task 4 and keep only the VM-level assertions here.

- [ ] **Step 3: Write the execution arm**

In `vm.rs`, beside `Instruction::JumpIfNotBoolean`:

```rust
Instruction::IntegerBinary {
    operation,
    destination,
    left,
    right,
    target,
} => {
    let left_value = read_local(&mutator.heap, activation, *left);
    let right_value = read_local(&mutator.heap, activation, *right);
    let (Value::SmallInteger(left_integer), Value::SmallInteger(right_integer)) =
        (left_value, right_value)
    else {
        return Flow::Jump(*target as usize);
    };
    let result = match operation {
        IntegerOperation::Add => left_integer.checked_add(right_integer).map(Value::SmallInteger),
        IntegerOperation::Subtract => {
            left_integer.checked_sub(right_integer).map(Value::SmallInteger)
        }
        IntegerOperation::Multiply => {
            left_integer.checked_mul(right_integer).map(Value::SmallInteger)
        }
        // `checked_div_euclid` answers `None` for a zero divisor as well as
        // for `i64::MIN // -1`, so both take the fallback and the primitive
        // raises its own "division by zero" or overflow error. No zero check
        // is written here, because writing one would be the second copy.
        IntegerOperation::FloorDivide => left_integer
            .checked_div_euclid(right_integer)
            .map(Value::SmallInteger),
        IntegerOperation::Less => Some(Value::Boolean(left_integer < right_integer)),
        IntegerOperation::Greater => Some(Value::Boolean(left_integer > right_integer)),
        IntegerOperation::LessOrEqual => Some(Value::Boolean(left_integer <= right_integer)),
        IntegerOperation::GreaterOrEqual => {
            Some(Value::Boolean(left_integer >= right_integer))
        }
        IntegerOperation::Equal => Some(Value::Boolean(left_integer == right_integer)),
    };
    // Overflow is not an error here. It takes the fallback, so the error and
    // its text come from the primitive, which is the only place that decides
    // what an overflowing addition does.
    match result {
        Some(value) => {
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        None => Flow::Jump(*target as usize),
    }
}
```

The helper names are verified: `vm.rs` reads a register with
`read_local(&mutator.heap, activation, *register)` and writes one with
`write_local(&mut mutator.heap, activation, *destination, value)`, as the
`Instruction::Move` arm does. Use those exactly.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime`

Expected: PASS. Any test that asserts on a disassembly listing may now see the
new instruction; if one does, that is Task 4's doing, not this task's.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
git add crates/ns-runtime/src/vm.rs
git commit -m "feat: execute IntegerBinary, falling back on a non-integer or an overflow"
```

---

### Task 4: Emit it from the optimized lowering

**Files:**
- Modify: `crates/ns-runtime/src/compiler/lower.rs`
- Test: `crates/ns-runtime/src/compiler/lower.rs`'s test module

**Interfaces:**
- Consumes: `control_flow::recognize_integer_binary` (Task 1),
  `Instruction::IntegerBinary` (Task 2), and `lower.rs`'s existing
  `allocate_scratch`, `emit`, `here`, `patch_target`, `lower_operand`,
  `lower_expression` and `lower_fallback_send`.
- Produces: `fn lower_integer_binary(&mut self, send: &ast::Send, operation: IntegerOperation) -> u16`.

- [ ] **Step 1: Write the failing tests**

In `lower.rs`'s test module, using its existing `compile_foo` and `has` helpers:

```rust
#[test]
fn an_integer_addition_lowers_to_the_inlined_fast_path() {
    let code = compile_foo("1 + 2");
    assert!(has(&code, |instruction| matches!(
        instruction,
        Instruction::IntegerBinary {
            operation: IntegerOperation::Add,
            ..
        }
    )));
}

#[test]
fn the_inlined_fast_path_keeps_the_ordinary_send_as_its_fallback() {
    // The fallback is what raises every error the fast path declines to
    // handle, so it must still be emitted -- and it must be a real send, not
    // a second copy of the arithmetic.
    let code = compile_foo("1 + 2");
    assert!(has(&code, |instruction| matches!(
        instruction,
        Instruction::SendPublic { .. }
    )));
}

#[test]
fn a_super_send_of_an_inlinable_selector_is_not_inlined() {
    // `super + x` starts its lookup above the receiver's class, which the
    // guard cannot reproduce, so it stays an ordinary send.
    let code = compile_foo("super + 2");
    assert!(!has(&code, |instruction| matches!(
        instruction,
        Instruction::IntegerBinary { .. }
    )));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::tests::an_integer_addition_lowers`

Expected: FAIL — no `IntegerBinary` is emitted.

- [ ] **Step 3: Write the lowering**

In `lower.rs`, beside `lower_if`:

```rust
/// An integer leaf's inlined fast path, shaped like `emit_guarded_branch`
/// above but with two ways instead of three: the instruction itself is the
/// guard, and its `target` is the fallback.
///
/// The receiver is lowered with `lower_operand` against the argument, because
/// whatever the argument lowers to may write the receiver's register --
/// the same question `lower_arguments` asks for every send.
fn lower_integer_binary(&mut self, send: &ast::Send, operation: IntegerOperation) -> u16 {
    let argument = &send.message.arguments[0];
    let left_register = self.lower_operand(&send.receiver, std::slice::from_ref(argument));
    let right_register = self.lower_expression(argument);
    let destination = self.allocate_scratch();
    let mark = self.next_scratch;
    let fallback_jump = self.emit(Instruction::IntegerBinary {
        operation,
        destination,
        left: left_register,
        right: right_register,
        target: 0,
    });
    let after_fast_jump = self.emit(Instruction::Jump { target: 0 });
    let fallback_label = self.here();
    self.patch_target(fallback_jump, fallback_label);
    self.next_scratch = mark;
    let fallback_result = self.lower_fallback_send(left_register, &send.message);
    self.emit(Instruction::Move {
        destination,
        source: fallback_result,
    });
    let end_label = self.here();
    self.patch_target(after_fast_jump, end_label);
    destination
}
```

- [ ] **Step 4: Teach `patch_target` the new instruction**

`patch_target` matches on the instruction it patches. Add the `IntegerBinary`
arm so its `target` is patched like a jump's; without it the fallback target
stays `0` and every non-integer operand jumps to instruction zero — silent
wrong output, not a crash.

- [ ] **Step 5: Call it from the inlining gate**

In `lower_send`, after the existing `control_flow::recognize` branch and under
the same `is_ordinary_send` gate:

```rust
if let Some(operation) = control_flow::recognize_integer_binary(send) {
    return self.lower_integer_binary(send, operation);
}
```

- [ ] **Step 6: Teach `send_writes_register` about it**

`send_writes_register` answers what a send can clobber before it. An inlined
integer binary still runs its fallback send, which runs user code, so it must
answer exactly what the ordinary send answers — do not add a case that makes it
look harmless. Read the function and confirm the new form falls through to the
ordinary-send answer; if `recognize_integer_binary` short-circuits it anywhere,
that is a defect, and the test in Step 7 is what shows it.

- [ ] **Step 7: Write the aliasing test**

`compile_foo` wraps its argument as `foo: x = ( ^<body> )`, which has a
parameter but no temporaries, so it cannot express this test. Use
`lower_whole_method`, which the module's other tests use for a full source:

```rust
#[test]
fn a_local_read_across_an_inlined_arithmetic_fallback_is_copied() {
    // The fallback send runs user code, which can assign to a local through
    // its setter, so a local's register read afterwards must be a copy.
    // Getting this wrong is silent wrong output at exit status 0.
    let code = lower_whole_method(
        indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              bump: value = ( ^value + 1 )
              foo = ( | total | total:: 1. ^(total + (self bump: total)) + total )
            ) : ()
        "},
        1,
    );
    assert!(has(&code, |instruction| matches!(
        instruction,
        Instruction::Move { .. }
    )));
}
```

The second argument of `lower_whole_method` is the method index within the
class, so `1` selects `foo`, not `bump:`.
`an_assignment_two_inlined_levels_deep_in_a_method_copies_the_operand` in the
same module shows the same helper in use.

- [ ] **Step 8: Run the whole suite**

Run: `cargo test --workspace`

Expected: PASS, every golden included. **If a golden's output changed, stop.**
That is a behaviour change, and this plan's premise is that there is none — find
out which path changed before touching any `.toml`.

- [ ] **Step 9: Run the stress modes and the corpus gates**

```bash
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
NS_NEXT_STRESS_COLLECT=1 cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: "parsed 210, failed 0, expected-fail 1, stale 0" and
"ran 6, failed 0, skipped 205" from every run.

- [ ] **Step 10: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
git add crates/ns-runtime/src/compiler/lower.rs
git commit -m "perf: emit an inlined fast path for the integer leaves"
```

---

### Task 5: Goldens for the paths the fast path declines

**Files:**
- Create: `goldens/arithmetic/integer_fallbacks.ns` and its `.toml`
- Test: the existing golden harness picks them up

**Interfaces:**
- Consumes: everything above.
- Produces: recorded outcomes that pin the fallback's behaviour under both
  lowerings.

- [ ] **Step 1: Write the golden module**

```
Newspeak3
'Arithmetic'
class IntegerFallbacks usingPlatform: platform = Object new (
) (
  public main: p args: a = (
    (* The fast path. *)
    p transcript show: (2 + 3) printString; newLine.
    p transcript show: (7 < 9) printString; newLine.
    (* A float operand: not two small integers, so the fallback runs and the
       primitive decides what a mixed addition answers. *)
    p transcript show: (2 + 3.5) printString; newLine.
    (* A string operand: the fallback runs, the primitive fails, and the
       lookup ends where it ends today. *)
    p transcript show: (2 = 'two') printString; newLine.
    (* Floor division on the fast path, and then its two fallbacks: a zero
       divisor and `i64::MIN // -1`, both of which `checked_div_euclid`
       declines so that the primitive raises its own error. *)
    p transcript show: (7 // 2) printString; newLine
  )
) : ()
```

- [ ] **Step 2: Run it and read the output**

Run: `cargo run -p ns-cli -- run goldens/arithmetic/integer_fallbacks.ns --send main:args:`

Read the output. **Establish that each line is right before recording it** —
compare each against what the same expression answers through the reference
lowering, which never inlines:

Run: `cargo run -p ns-cli -- run goldens/arithmetic/integer_fallbacks.ns --send main:args: --lowering reference`

`--lowering` takes `optimized` (the default) or `reference`. The two runs must
agree. If they do not, the inlining changed behaviour and the fix is in Task 4,
not here.

- [ ] **Step 3: Record the golden**

Write the `.toml` beside it with the `status`, `stdout` and `stderr` the two
lowerings agreed on, in the format the other goldens' `.toml` files use.

- [ ] **Step 4: Write the two raising goldens**

Each error path needs its own module, because the first uncaught error ends the
run — one module cannot pin two of them.

`goldens/arithmetic/integer_overflow.ns`, which pins the overflow message:

```
Newspeak3
'Arithmetic'
class IntegerOverflow usingPlatform: platform = Object new (
) (
  public main: p args: a = (
    p transcript show: (9223372036854775807 + 1) printString; newLine
  )
) : ()
```

`goldens/arithmetic/integer_division_by_zero.ns`, which pins the
"division by zero" message reached through the inlined path's fallback:

```
Newspeak3
'Arithmetic'
class IntegerDivisionByZero usingPlatform: platform = Object new (
) (
  public main: p args: a = (
    p transcript show: (7 // 0) printString; newLine
  )
) : ()
```

Record each one's `status`, `stdout` and `stderr` the same way, after checking
both lowerings agree. These two are what show that inlining did not take the
error away from `builtins/numbers.rs`: the recorded `stderr` must be the same
text the primitive produced before this change.

- [ ] **Step 5: Run the suite**

Run: `cargo test --workspace`

Expected: PASS, the two new goldens among them, under both lowerings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add goldens/arithmetic/
git commit -m "test: pin the integer leaves' fallback and overflow paths"
```

---

### Task 6: Measure, and record the result

**Files:**
- Modify: `docs/design/execution.md` — a new "What leaf inlining moved" section
- Modify: `docs/PROGRESS.md` — the step's entry

**Interfaces:**
- Consumes: the finished implementation.
- Produces: the measured figures the next step argues from.

- [ ] **Step 1: Record the expectation before measuring**

Write down, before running anything, what each of the eight sources should do.
The reasoning: the arithmetic-heavy sources should improve, because a send per
operation becomes an instruction; sources that do no integer arithmetic in
their hot loop should not move. Name which is which in advance, so the result
is not read to fit afterwards.

- [ ] **Step 2: Build both binaries with the protocol's flags**

In PowerShell, for the baseline commit and for the head:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

`RUSTFLAGS` replaces the configuration's flags, which is why `+crt-static` is
repeated. Never measure a debug build.

- [ ] **Step 3: Check `vm::run` is still one inlined function**

Run `dumpbin /disasm` (from the Visual Studio Build Tools; not on `PATH`) over a
release build with debug information and confirm no `vm::run::closure` became a
function of its own. The step is near LLVM's inlining threshold, and this
instruction adds to it. **Do this before any timing**, because a split step
invalidates the comparison.

- [ ] **Step 4: Measure all eight sources, two sittings**

The seven sources of `docs/design/execution.md`'s "What removing the re-entries
moved", plus `goldens/bench/bench_megamorphic.ns`. Four are goldens; the other
four are **not in the tree** and must be written out from the text in
`execution.md`, "What spec-conformant lookup moved" — never reconstructed from
their prose descriptions, which would measure something else.

- [ ] **Step 5: Write the measurement up**

Add "What leaf inlining moved" to `docs/design/execution.md`, following the
shape of the sections around it: the expectation recorded in advance, the
table, which sources moved and which did not, and what is left unexplained
stated as unexplained rather than guessed at.

- [ ] **Step 6: Update the progress tracker**

In `docs/PROGRESS.md`, record the step as done in the order of work, with the
measured range, and say that step 6's move to the VM mirror now has its
cushion.

- [ ] **Step 7: Commit**

```bash
git add docs/design/execution.md docs/PROGRESS.md
git commit -m "docs: record what leaf inlining moved"
```

---

## Self-Review

**Spec coverage.** The design's sequencing requirement — "The inlined fast path
for leaves comes before the library moves" — is what this whole plan implements.
Its stated mechanism, "the compiler emits the arithmetic directly under a type
guard and falls back to the ordinary send, the way `ifTrue:` and `whileTrue:`
are lowered today", is Tasks 1 through 4. Cog's `bytecodePrimAdd`, named in
`core-library.md` as the model, is the same shape. The specification's own
requirement that an overflowing integer raise an error rather than wrap
(`core-library.md`, "Deviation: `i64` integers") is preserved by routing
overflow to the fallback, and pinned by Task 5's overflow golden; the zero
divisor is preserved the same way and pinned by its own golden beside it.

**Coverage of the operations themselves.** All nine binary members of
`IntegerPrimitive` are inlined, including `//`. An earlier draft of this plan
excluded division on the grounds that its error and rounding rules belong to
`builtins/numbers.rs` — which is true, and is equally true of `+`'s overflow,
so it was not a reason. `checked_div_euclid` declines a zero divisor and
`i64::MIN // -1` exactly as `checked_add` declines an overflow, and the same
fallback handles both. The only selector left out is `/`, which has no integer
primitive at all.

**Not covered here, by design:** the VM mirror, `Kernel.ns`, the bootstrap
change and the `Class`/`Behavior`/`Metaclass` work. Those are separate plans,
for the reason the brainstorming recorded — this one produces working, measured
software on its own, and its measurement informs the next.

**Placeholders.** Two steps deliberately say "match the spelling the code
actually uses" (Task 1 Step 4, Task 3 Step 3) rather than inventing a helper
name. That is not a placeholder: the surrounding code is named in each case and
the implementer is told to copy it, because a guessed name would not compile.
Task 3 Step 1's test body is left to the file's own harness for the same reason,
with the fallback named if `ns-runtime`'s tests do not have one.

**Type consistency.** `IntegerOperation` is declared once, in `bytecode.rs`
(Task 1 Step 3), used by `recognize_integer_binary` (Task 1 Step 4),
`Instruction::IntegerBinary` (Task 2 Step 3), the VM arm (Task 3 Step 3) and
`lower_integer_binary` (Task 4 Step 3), with the same nine variant names
throughout. `Instruction::IntegerBinary`'s field names — `operation`,
`destination`, `left`, `right`, `target` — are identical in all four places.
`OPCODE_INTEGER_BINARY = 33` appears in Task 2 Steps 1 and 5 with the same
value.
