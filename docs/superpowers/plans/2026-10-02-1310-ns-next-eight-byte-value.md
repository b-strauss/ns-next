# The Eight-Byte Value Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `Value` one tagged 64-bit word -- Spur's tags, a 61-bit `SmallInteger`, the constants `nil`, `true` and `false`, a `SmallFloat64`, and a handle -- with every float outside the `SmallFloat64` range boxed, every integer outside 61 bits a `LargeInteger`, and every value stored in a heap object a bare eight-byte `Word`. This is road step 2's second plan.

**Architecture:** Seven stages, each landing green on its own, so that every behaviour change lands on today's enum and the switch to the word moves no golden. Stage 1 makes `Value` opaque behind constructors, accessors and a `ValueKind` view, and moves it to `crates/ns-runtime/src/heap/value.rs`, where it can reach a handle's private fields. Stage 2 moves the `SmallInteger`/`LargeInteger` boundary to 61 bits, stage 3 boxes the floats outside Spur's range, stage 4 replaces the private representation with the word, stage 5 stores bare `Word`s in the heap with a checked store, stage 6 does `SmallInteger` arithmetic on the tagged words, and stage 7 measures against the tree the branch started from.

**Tech Stack:** Rust 1.98.0, edition 2021, `#![forbid(unsafe_code)]` in `ns-runtime`. No new dependency. Tests under `cargo-nextest`; goldens under `goldens/`.

**Spec:** `docs/design/object-model.md`, "Planned: an eight-byte value", including its "How it is built: seven stages, each green" (decided with the user on 2026-10-02). Background: `docs/design/execution.md`, "Where the 3% goes" (the store-forwarding stall) and "The Are We Fast Yet benchmarks" (the baseline and its protocol); `docs/design/memory.md`, "The brand" and "The move stress mode"; `docs/design/image-format.md`, "Planned: the image under the new representation"; `docs/design/core-library.md`, "Large integers". Spur's encoding: `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c` -- constants at `:2779-2784`, `integerObjectOf` at `:41157`, `isSmallFloatValueBits` at `:41231`, `floatObjectOf` at `:40785`, `smallFloatValueOf` at `:41488`.

## Global Constraints

- Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - `vm.rs` holds no dispatch logic and raises no error; `IntegerBinary` declines to its fallback send for every case it does not answer.
  - Compiling a resolved module never raises a `NewspeakError`.
  - An image is untrusted input: no byte sequence may panic, and every pre-allocation is bounded by `bounded_count`.
  - A wire tag is assigned once and never renumbered or reused. **This plan adds no wire tag**: a boxed float is `VALUE_FLOAT`/`LITERAL_FLOAT` and a `LargeInteger` that fits `i64` is `VALUE_SMALL_INTEGER`/`LITERAL_SMALL_INTEGER`.
  - A handle means nothing against a `Heap` that did not mint it; the brand needs no cast, and only `heap` (now `heap.rs` and its child modules) changes a value's brand.
  - Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvaluationResult`. Tests use a local `expect_ok` helper.
- **Every integer has exactly one representation**: an integer in `SMALL_INTEGER_MIN ..= SMALL_INTEGER_MAX` (−2^60 ..= 2^60 − 1) is a `SmallInteger`, every other one a `LargeInteger`; `Mutator::new_integer` is the only way an `ArbitraryInteger` becomes a `Value`.
- **Every float has exactly one representation**: a float whose bits pass Spur's `isSmallFloatValueBits` is a `SmallFloat64` immediate, every other one an `ObjectData::Float` object of the builtin `Float`; `Mutator::new_float` is the only way an `f64` becomes a `Value`.
- A boxed `Float`, like a `LargeInteger`, is a value object (§3.1.1): its `=`, `==` and `hash` come from its state; a NaN equals a NaN.
- A golden that changes is a behaviour change: each change is named in its task with the reason it is right. Only Task 6 changes golden output.
- Names are written out (`CLAUDE.md`, "Conventions"); clippy's `min_ident_chars` refuses one- and two-character names. Comments explain the decision, not the mechanics.
- A Newspeak module inline in a test is an `indoc! {"…"}`, indented with the Rust around it.
- Conventional commits. Every commit ends with
  `Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01AnZi5xMPKXmXfe1e8TdEAq`.
  A subagent's commit names the subagent's own model.
- Work happens on the branch `eight-byte-value` in the main working directory (no worktree).
- Per-commit gate (`CLAUDE.md`, "Working in this repository"), every task, however small:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace --exclude ns-lsp`
  - `cargo test --workspace --exclude ns-lsp --doc`
  - `cargo run -p ns-cli -- parse-corpus corpus/modules` → "parsed 211, failed 0"

  Tasks 4, 8 and 9 also run `cargo nextest run --workspace --release` (the size tests run only there). The stress modes, `run-corpus`, the benchmarks profile and the `ns-lsp` suite run once, in Task 13 (the `finishing-a-batch` skill). A commit that never saw them is work in progress; a subagent implementing a task is told so. Nothing under `crates/ns-syntax` or `editors/jetbrains` changes.
- Run every command in the foreground, except Task 11's measurement (see there).

## Review Focus

- **The 61-bit edges under both lowerings.** `1152921504606846975 + 1`, `-1152921504606846976 - 1`, `-1152921504606846976 // -1`, `-1152921504606846976 negated` and `abs`, `1 << 60` answer `LargeInteger`s whose `printString` is the exact decimal; the same results come from the inlined `IntegerBinary` (optimized lowering) and from the send (reference lowering). Pinned in Task 6 (golden) and Task 10 (unit tests of the tagged operations).
- **The float edges.** `0.0` and `-0.0` are both immediate and distinct as words, `0.0 = -0.0` is `true`; the smallest and largest immediate exponents, an exponent of 896 with a zero mantissa (2^-127, boxed), a denormal, `1.0e300`, NaN and the infinities each round-trip through `new_float`/`float_value` with identical bits. Pinned in Task 7 (unit tests, golden) and Task 8 (word round-trip tests).
- **A hostile image's integers and floats.** An `i64` value or literal outside 61 bits decodes to a `LargeInteger`; every `f64` bit pattern under `VALUE_FLOAT`/`LITERAL_FLOAT` decodes (immediate or boxed) without a panic; `read_large_integer` still refuses a payload that fits `i64`. Pinned in Tasks 6 and 7 (`tests/image.rs`).
- **A stale handle stored into the heap.** In a debug build the store itself panics (`Heap::store`), not the next move; a host value carried across a run is refused at `fetch`. Pinned in Task 9.
- **A `LargeInteger` used as an index, size or count.** `at:`, `at:put:`, `Array new:`, `copyFrom:to:`, `asStringFixed:` refuse it in the words they refuse an out-of-range `SmallInteger`, never "requires an Integer". Pinned in Task 5 (golden).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ns-runtime/src/heap/value.rs` (new, Task 4) | `Value`, `ValueKind`, `Word`, `SMALL_INTEGER_MIN`/`MAX`, `fits_small_integer`, `is_small_float_bits`, the brand conversions `at_rest`/`branded`, the tagged arithmetic (Task 10) |
| `crates/ns-runtime/src/heap.rs` | `mod value;`, re-exports, `Heap::float_value`, `Heap::load`/`Heap::store` (Task 9), `describe`, `dispatch_id`, storage as `Word` |
| `crates/ns-runtime/src/types.rs` | re-exports `Value`, `ValueKind`; `ObjectData::Float` (Task 7); `Instance`/`Array`/`ClosureData.copied`/`Frame` hold `Word` (Task 9) |
| `crates/ns-runtime/src/trace.rs`, `heap/mark.rs` | `Tracer::value` through the accessors; `Tracer::word` (Task 9) |
| `crates/ns-runtime/src/interpreter/mod.rs` | `Mutator::new_float`, `Mutator::new_integer`'s 61-bit test, size tests |
| `crates/ns-runtime/src/builtins/*.rs` | leaves through the accessors; index/size leaves take a `LargeInteger` (Task 5); 61-bit fast paths (Task 6); boxed floats (Task 7); tagged arithmetic (Task 10) |
| `crates/ns-runtime/src/vm.rs` | `IntegerBinary` (Tasks 6, 10), conditional jumps and literals through the accessors |
| `crates/ns-runtime/src/interpreter/literals.rs`, `compiler/lower.rs`, `bytecode.rs` | an integer literal is small only within 61 bits |
| `crates/ns-runtime/src/image/encode.rs`, `decode.rs` | a `LargeInteger` that fits `i64` written as an `i64`; an `i64` outside 61 bits decoded to a `LargeInteger`; boxed floats as `VALUE_FLOAT` |
| `crates/ns-runtime/src/platform.rs` | `MonotonicNanoseconds` through `new_integer` |
| `crates/ns-cli/src/*.rs`, `crates/*/tests/*.rs` | through the accessors |
| `goldens/**` | the goldens named in Tasks 5, 6 and 7 |
| `docs/design/*.md`, `CLAUDE.md`, `docs/PROGRESS.md`, `docs/OPEN-WORK.md` | moved from planned to built (Task 12) |

**The interface every task after Task 1 relies on** (exact signatures; defined on the enum in Task 1, moved to `heap/value.rs` in Task 4, unchanged by Tasks 8 and 9):

```rust
#[derive(Clone, Copy)]
pub enum ValueKind<'gc> {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    /// The immediate float only; a boxed float is an `Object`. Read any
    /// float through `Heap::float_value`.
    SmallFloat(f64),
    Object(ObjectReference<'gc>),
}

impl<'gc> Value<'gc> {
    pub const fn nil() -> Value<'gc>;
    pub const fn boolean(value: bool) -> Value<'gc>;
    /// `debug_assert!`s `fits_small_integer(value)` from Task 6 on.
    pub fn small_integer(value: i64) -> Value<'gc>;
    pub const fn object(object: ObjectReference<'gc>) -> Value<'gc>;
    pub fn kind(self) -> ValueKind<'gc>;
    pub fn is_nil(self) -> bool;
    pub fn as_boolean(self) -> Option<bool>;
    pub fn as_small_integer(self) -> Option<i64>;
    pub fn as_object(self) -> Option<ObjectReference<'gc>>;
}

impl<'gc> Mutator<'gc> {
    /// The one way an `f64` becomes a `Value`.
    pub fn new_float(&mut self, value: f64) -> Value<'gc>;
}

impl<'gc> Heap<'gc> {
    /// Any float, immediate or boxed; `None` for anything else.
    pub fn float_value(&self, value: Value<'gc>) -> Option<f64>;
}
```

From Task 6: `pub const SMALL_INTEGER_MIN: i64 = -(1 << 60);`, `pub const SMALL_INTEGER_MAX: i64 = (1 << 60) - 1;`, `pub const fn fits_small_integer(value: i64) -> bool`, exported from `ns_runtime` (`lib.rs`'s `pub use types::*`). From Task 7: `pub(crate) const fn is_small_float_bits(bits: u64) -> bool`. From Task 9: `pub struct Word` (opaque), `Heap::load(&self, word: Word) -> Value<'gc>`, `Heap::store(&self, value: Value<'gc>) -> Word`. From Task 10: the tagged operations listed there.

**How a site converts** (Tasks 2 and 3; every site in the workspace follows one of these rows):

| Today | After |
|---|---|
| `Value::Nil` (a value) | `Value::nil()` |
| `Value::Boolean(flag)` / `Value::SmallInteger(number)` / `Value::Object(object)` (a value) | `Value::boolean(flag)` / `Value::small_integer(number)` / `Value::object(object)` |
| `Value::Float(number)` (a value) | `mutator.new_float(number)` -- the only constructor |
| `.map(Value::SmallInteger)`, `.map_or(Value::Nil, Value::Object)` | `.map(Value::small_integer)`, `.map_or(Value::nil(), Value::object)` |
| `let Value::Object(object) = value else { … }` | `let Some(object) = value.as_object() else { … }` |
| `if let Some(Value::SmallInteger(index)) = arguments.first()` | `if let Some(index) = arguments.first().and_then(|value| value.as_small_integer())` |
| `matches!(value, Value::Nil)` | `value.is_nil()` |
| `matches!(value, Value::Boolean(false))` | `value.as_boolean() == Some(false)` |
| `assert!(matches!(value, Value::SmallInteger(5)))` | `assert_eq!(value.as_small_integer(), Some(5))` |
| `match value { Value::Nil => …, Value::Boolean(flag) => …, … }` | `match value.kind() { ValueKind::Nil => …, ValueKind::Boolean(flag) => …, … }` |
| a pattern reading `Value::Float(number)` | `heap.float_value(value)` (or `ValueKind::SmallFloat` only where the site is about the immediate itself: `immediate_index`, the encoder's scalar arm) |
| `(Value::SmallInteger(left), Value::SmallInteger(right))` tuple patterns | `(left.as_small_integer(), right.as_small_integer())` matched as `(Some(left), Some(right))`, or `kind()` pairs |
| a panic text naming `Value::Object` | name what was expected in words: `"expected an object, got {}"` with `heap.describe` |

---

### Task 1: The accessor API on today's enum

**Files:**
- Modify: `crates/ns-runtime/src/types.rs:23-30`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (beside `new_integer`, ~1467)
- Modify: `crates/ns-runtime/src/heap.rs` (beside `large_integer`, ~1351)

**Interfaces:**
- Produces: everything in "The interface every task after Task 1 relies on", except the Task 6/7/9/10 items.

- [ ] **Step 1: Write the failing tests** at the end of `types.rs`'s test module (create `#[cfg(test)] mod value_tests` if none fits):

```rust
#[cfg(test)]
mod value_tests {
    use super::{Value, ValueKind};

    #[test]
    fn each_constructor_answers_through_its_accessor_and_no_other() {
        let nil: Value<'static> = Value::nil();
        assert!(nil.is_nil());
        assert_eq!(nil.as_boolean(), None);
        let truth: Value<'static> = Value::boolean(true);
        assert_eq!(truth.as_boolean(), Some(true));
        assert!(!truth.is_nil());
        let three: Value<'static> = Value::small_integer(3);
        assert_eq!(three.as_small_integer(), Some(3));
        assert_eq!(three.as_boolean(), None);
        assert!(three.as_object().is_none());
        assert!(matches!(three.kind(), ValueKind::SmallInteger(3)));
        assert!(matches!(Value::<'static>::boolean(false).kind(), ValueKind::Boolean(false)));
        assert!(matches!(Value::<'static>::nil().kind(), ValueKind::Nil));
    }
}
```

And in `crates/ns-runtime/tests/heap.rs` (copy the file's existing interpreter setup and `expect_ok` helper):

```rust
#[test]
fn a_float_is_made_by_the_mutator_and_read_by_the_heap() {
    let mut interpreter = Interpreter::new();
    interpreter.enter(|mutator| {
        let value = mutator.new_float(2.5);
        assert_eq!(mutator.heap().float_value(value), Some(2.5));
        assert_eq!(mutator.heap().float_value(Value::small_integer(2)), None);
        assert!(matches!(value.kind(), ValueKind::SmallFloat(number) if number == 2.5));
    });
}
```

(Use the constructor the file already uses for an `Interpreter`; `ValueKind` is exported through `ns_runtime::*` once Step 3 adds it to `types.rs`.)

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime value_tests a_float_is_made_by_the_mutator`
Expected: compile errors — `no function or associated item named 'nil'`, `cannot find type 'ValueKind'`, `no method named 'new_float'`.

- [ ] **Step 3: Implement**, in `types.rs` directly below the enum:

```rust
/// What a `Value` is, for an exhaustive `match`: the view every site
/// matches on, so that no code outside the representation names it
/// (`docs/design/object-model.md`, "How it is built"). `SmallFloat` is the
/// immediate float only; code that reads a float asks `Heap::float_value`,
/// which also answers a boxed one.
#[derive(Clone, Copy)]
pub enum ValueKind<'gc> {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    SmallFloat(f64),
    Object(ObjectReference<'gc>),
}

impl<'gc> Value<'gc> {
    pub const fn nil() -> Value<'gc> {
        Value::Nil
    }

    pub const fn boolean(value: bool) -> Value<'gc> {
        Value::Boolean(value)
    }

    pub fn small_integer(value: i64) -> Value<'gc> {
        Value::SmallInteger(value)
    }

    pub const fn object(object: ObjectReference<'gc>) -> Value<'gc> {
        Value::Object(object)
    }

    pub fn kind(self) -> ValueKind<'gc> {
        match self {
            Value::Nil => ValueKind::Nil,
            Value::Boolean(flag) => ValueKind::Boolean(flag),
            Value::SmallInteger(number) => ValueKind::SmallInteger(number),
            Value::Float(number) => ValueKind::SmallFloat(number),
            Value::Object(object) => ValueKind::Object(object),
        }
    }

    pub fn is_nil(self) -> bool {
        matches!(self, Value::Nil)
    }

    pub fn as_boolean(self) -> Option<bool> {
        match self {
            Value::Boolean(flag) => Some(flag),
            _ => None,
        }
    }

    pub fn as_small_integer(self) -> Option<i64> {
        match self {
            Value::SmallInteger(number) => Some(number),
            _ => None,
        }
    }

    pub fn as_object(self) -> Option<ObjectReference<'gc>> {
        match self {
            Value::Object(object) => Some(object),
            _ => None,
        }
    }
}
```

In `interpreter/mod.rs`, beside `new_integer`:

```rust
    /// The one way an `f64` becomes a `Value`, so that every float has one
    /// representation once floats outside the `SmallFloat64` range are boxed
    /// (`docs/design/object-model.md`, "Planned: an eight-byte value").
    pub fn new_float(&mut self, value: f64) -> Value<'gc> {
        Value::Float(value)
    }
```

In `heap.rs`, beside `large_integer`:

```rust
    /// Any float, immediate or boxed; `None` for anything else.
    pub fn float_value(&self, value: Value<'gc>) -> Option<f64> {
        match value {
            Value::Float(number) => Some(number),
            _ => None,
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p ns-runtime value_tests a_float_is_made_by_the_mutator`
Expected: PASS.

- [ ] **Step 5: Run the per-commit gate, then commit**

```bash
git add crates/ns-runtime
git commit -m "feat: Value answers through constructors, accessors and a ValueKind view"
```
(with the two trailer lines of the Global Constraints)

---

### Task 2: The sources go through the API

**Files:**
- Modify: every file under `crates/ns-runtime/src` that names a `Value::` variant (632 occurrences; the counts per file are in the table below), including their `#[cfg(test)]` modules and the eight doctest lines at `interpreter/mod.rs:474-535`
- Modify: `crates/ns-cli/src/entry.rs:83,86,109`, `crates/ns-cli/src/program.rs:176-177`

**Interfaces:**
- Consumes: Task 1's API.
- Produces: no code outside `types.rs` names a `Value::` variant except the `at_rest`/`branded` pair and `float_value` in `heap.rs`, `kind()` and the accessors in `types.rs`, `new_float`, and `trace.rs` (Task 4 rewrites those).

Counts per file (src): `heap.rs` 120, `builtins/value.rs` 79, `builtins/numbers.rs` 51, `interpreter/mod.rs` 43, `vm.rs` 25, `interpreter/mirror.rs` 24, `interpreter/future.rs` 21, `image/encode.rs` 20, `image/decode.rs` 20, `builtins/text.rs` 20, `interpreter/kernel.rs` 19, `interpreter/instantiate.rs` 19, `interpreter/unwind.rs` 18, `interpreter/resolve.rs` 15, `interpreter/nested.rs` 15, `builtins/object_class.rs` 15, `builtins/collections.rs` 15, `builtins/activation.rs` 13, `builtins/behavior.rs` 9, `interpreter/locals.rs` 8, `builtins/block.rs` 8, `image/platform_paths.rs` 6, `image/mod.rs` 6, `types.rs` 5, `trace.rs` 5, `dispatch.rs` 4, `builtins/message.rs` 4, `mixin.rs` 3, `interpreter/core_library.rs` 3, `bytecode.rs` 3, `platform.rs` 2, `interpreter/value_class.rs` 2, `interpreter/metaclass.rs` 2, `builtins/module.rs` 2, `interpreter/slots.rs` 1, `interpreter/inline_cache.rs` 1, `heap/world.rs` 1.

This is a behaviour-preserving rewrite: no golden, no test expectation changes.

- [ ] **Step 1: Convert, file by file, by the conversion table** above "Task 1". Specific sites that need care:
  - `vm.rs:613-642` (`JumpIfNotBoolean`, `BranchIfFalse`, `BranchIfTrue`): `register(...).as_boolean().is_some()`, `== Some(false)`, `== Some(true)`.
  - `vm.rs:651-704` (`IntegerBinary`): `let (Some(left_integer), Some(right_integer)) = (left_value.as_small_integer(), right_value.as_small_integer()) else { jump_to!(*target) };` and `.map(Value::small_integer)`, `Some(Value::boolean(left_integer < right_integer))`.
  - `vm.rs:1182-1200` (`make_value`): `Literal::Float(number) => mutator.new_float(*number)`.
  - Every float constructor in `builtins/numbers.rs` (lines 337, 495, 557, 640, 680, 705, 709, 710): the closures at `316-343`, `551-564` and `678-681` hold `mutator` shared and cannot call `new_float`; make each return the raw `f64` (or, where the closure already returns `Result<Value, ArbitraryInteger>`, add a variant, e.g. an enum `Answer { Value(Value<'gc>), Integer(ArbitraryInteger), Float(f64) }` local to `numbers.rs`) and box after the closure with `mutator.new_float`.
  - `builtins/numbers.rs` `float_operand` (274-279), `argument_float` (660-668), `execute_float`'s receiver (654), `float_equals` (93-96): read with `heap.float_value(...)`; `float_operand` and `float_equals` gain a `heap: &Heap<'gc>` parameter.
  - `builtins/value.rs:288,337`, `builtins/object_class.rs:44`: match on `kind()`'s `SmallFloat`.
  - `heap.rs:38-48` (`immediate_index`), `heap.rs:1632` (`describe`), `heap.rs:1688,1702` (`stash`/`fetch`), `heap.rs:1974` (`dispatch_id`), `heap.rs:3101-3170` (`replace_resolved_futures`; the `&mut Value` at 3134 only reads `as_object()` and then assigns the whole value).
  - `image/encode.rs:146-161` (`encode_scalar_value`) and `image/decode.rs:870-879,2387-2425`: `kind()` and constructors; `decode_value`'s `VALUE_FLOAT` becomes `context.mutator.new_float(reader.read_f64()?)`. `decode_scalar_value` (no mutator) keeps building the immediate: add a `pub(crate)` constructor `Value::small_float(value: f64)` in `types.rs` used only there and by `new_float`; Task 7 gives `decode_scalar_value` a mutator and removes the decoder's use.
  - The doctests at `interpreter/mod.rs:474-535`: convert the ordinary ones; for the three `compile_fail` blocks (starting at 487, 508, 532) convert them, then confirm by hand that each still fails for the reason its prose states (the brand), by temporarily changing ` ```compile_fail` to ` ```` ` ` ```rust` and reading the error, then restoring it.
  - Panic and error texts naming `Value::Object` (`builtins/value.rs` 3, `interpreter/mod.rs` 1, `interpreter/unwind.rs` 4, `types.rs` 1): rewrite in words.
- [ ] **Step 2: Check no variant is named outside the allowed places**

Run: `rg -n "Value::(Nil|Boolean|SmallInteger|Float|Object)\b" crates/ns-runtime/src crates/ns-cli/src`
Expected: only `types.rs` (the enum, its `impl`, `small_float`), `heap.rs` (`at_rest`, `branded`, `float_value`), `trace.rs:23-28`, and `interpreter/mod.rs`'s `new_float`.

- [ ] **Step 3: Run the per-commit gate.** Expected: everything passes unchanged; the tests under `crates/*/tests` still compile because the enum is still public.
- [ ] **Step 4: Commit**

```bash
git add crates
git commit -m "refactor: the runtime's and the CLI's sources read and make values through the API"
```

---

### Task 3: The tests go through the API

**Files:**
- Modify: every file under `crates/ns-runtime/tests` and `crates/ns-cli/tests` that names a `Value::` variant (540 occurrences). Largest: `image.rs` 137, `vm.rs` 46, `future.rs` 46, `heap.rs` 38, `builtins_boolean_numbers.rs` 36, `trampoline.rs` 27, `instantiate.rs` 25, `platform.rs` 23, `nested_classes.rs` 21; ns-cli: `tests/build_load.rs:467`, `tests/disasm_snapshots.rs:276`.

**Interfaces:**
- Consumes: Task 1's API.

- [ ] **Step 1: Convert by the conversion table.** A test that stashes a float (`interpreter.stash(Value::Float(1.5))`, e.g. `builtins/value.rs:887,1683` in the src test modules already done in Task 2, and `tests/builtins_boolean_numbers.rs:225,230,310,340,359,368,373`, `tests/heap.rs:303`, `tests/image.rs:94,107,147-162`) makes it inside an entry:

```rust
let one_and_a_half = interpreter.enter(|mutator| {
    let value = mutator.new_float(1.5);
    mutator.stash(value)
});
```

  `tests/image.rs:141-169` (`scalar_value_boundary_cases_round_trip`) keeps its NaN and infinity cases for now: `Value::small_float` is `pub(crate)` and not reachable from an integration test, so build the scalar values inside `interpreter.enter` with `mutator.new_float`, and keep calling `encode_scalar_value`/`decode_scalar_value` (Task 7 reworks this test). Panic texts naming a variant are rewritten in words.
- [ ] **Step 2: Check**

Run: `rg -n "Value::(Nil|Boolean|SmallInteger|Float|Object)\b" crates --glob '!crates/ns-runtime/src/types.rs' --glob '!crates/ns-runtime/src/heap.rs' --glob '!crates/ns-runtime/src/trace.rs' --glob '!crates/ns-runtime/src/interpreter/mod.rs'`
Expected: no output. Then `rg -n "Value::(Nil|Boolean|SmallInteger|Float|Object)\b" crates/ns-runtime/src/interpreter/mod.rs` shows only `new_float`.
- [ ] **Step 3: Run the per-commit gate.** Expected: everything passes; no expectation changed.
- [ ] **Step 4: Commit**

```bash
git add crates
git commit -m "test: the tests read and make values through the API"
```

---

### Task 4: `Value` becomes opaque, in `heap/value.rs`

**Files:**
- Create: `crates/ns-runtime/src/heap/value.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (`mod value;`, re-export, remove `at_rest`/`branded`/`float_value`'s variant matches), `crates/ns-runtime/src/types.rs` (remove the enum and `ValueKind`, re-export from `heap`), `crates/ns-runtime/src/trace.rs:20-29`, `crates/ns-runtime/src/interpreter/mod.rs` (`new_float`)

**Interfaces:**
- Consumes: Task 1's API.
- Produces: the same API, now the only way to reach a value; `pub(super) fn at_rest` / `branded` in `heap::value`; `pub(crate) fn small_float`.

`Value` moves under `heap` because the word (Task 8) has to build and take apart an `ObjectReference`, whose fields are private to `heap` and visible to its child modules.

- [ ] **Step 1: Write the failing test** in `heap/value.rs`'s own test module (create the file with only the test first, and `mod value;` in `heap.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::Value;

    #[test]
    fn a_value_is_copied_whole_and_unchanged() {
        let original: Value<'static> = Value::small_integer(-7);
        let copy = original;
        assert_eq!(original.as_small_integer(), Some(-7));
        assert_eq!(copy.as_small_integer(), Some(-7));
    }
}
```

Run: `cargo nextest run -p ns-runtime a_value_is_copied_whole`
Expected: FAIL to compile (`no Value in heap::value`).

- [ ] **Step 2: Implement.** `heap/value.rs`:

```rust
//! A Newspeak value as the runtime holds it: opaque, made by constructors
//! and read through accessors and `ValueKind`, so that its representation
//! can change without touching a single site
//! (`docs/design/object-model.md`, "How it is built"). It lives under
//! `heap` because a handle's fields are private to `heap`.

use super::{Brand, ObjectReference};

#[derive(Clone, Copy)]
pub struct Value<'gc>(Representation<'gc>);

#[derive(Clone, Copy)]
enum Representation<'gc> {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    Float(f64),
    Object(ObjectReference<'gc>),
}
```

followed by `ValueKind` (moved verbatim from `types.rs`) and the `impl<'gc> Value<'gc>` of Task 1 rewritten over `self.0`, plus:

```rust
    /// An immediate float; only `Mutator::new_float` and the image's scalar
    /// decoder make one.
    pub(crate) fn small_float(value: f64) -> Value<'gc> {
        Value(Representation::Float(value))
    }

/// Never `{:?}` a `Value` in Newspeak-facing text (`Heap::describe`); this
/// exists so that the types holding one can derive `Debug` for Rust-side
/// diagnostics, and prints what an `ObjectReference` prints.
impl std::fmt::Debug for Value<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind() {
            ValueKind::Nil => formatter.write_str("nil"),
            ValueKind::Boolean(flag) => write!(formatter, "{flag}"),
            ValueKind::SmallInteger(number) => write!(formatter, "{number}"),
            ValueKind::SmallFloat(number) => write!(formatter, "{number}"),
            ValueKind::Object(object) => write!(formatter, "{object:?}"),
        }
    }
}

/// The two conversions of a brand (`docs/design/memory.md`, "The brand"),
/// moved here from `heap.rs` with the representation they take apart;
/// `pub(super)` keeps them inside `heap`, as before.
pub(super) fn at_rest(value: Value<'_>) -> Value<'static> {
    Value(match value.0 {
        Representation::Nil => Representation::Nil,
        Representation::Boolean(flag) => Representation::Boolean(flag),
        Representation::SmallInteger(number) => Representation::SmallInteger(number),
        Representation::Float(number) => Representation::Float(number),
        Representation::Object(object) => Representation::Object(ObjectReference {
            index: object.index,
            #[cfg(debug_assertions)]
            origin: object.origin,
            #[cfg(debug_assertions)]
            epoch: object.epoch,
            brand: Brand::default(),
        }),
    })
}
```

and `branded<'gc>(value: Value<'static>) -> Value<'gc>` the same way. Move the doc comment of `heap.rs`'s `at_rest` (heap.rs:315-325) onto it. In `heap.rs`: `mod value;`, `pub use value::{Value, ValueKind};`, `use value::{at_rest, branded};`, delete the old pair; `Brand` must be visible to the child (it is private in `heap`, which children see). `float_value` becomes `match value.kind() { ValueKind::SmallFloat(number) => Some(number), _ => None }`. In `types.rs`: delete the enum, `ValueKind` and the `impl`, and change line 21 to `pub use crate::heap::{ClassReference, FrameReference, ObjectReference, Value, ValueKind};`. `new_float` becomes `Value::small_float(value)`. `trace.rs:23-28`:

```rust
    fn value(&mut self, value: &mut Value<'_>) {
        if let Some(mut object) = value.as_object() {
            self.object(&mut object);
            *value = Value::object(object);
        }
    }
```

Delete the now-unused `#[derive(Debug)]` reliance: every type that derived `Debug` over a `Value` keeps compiling through the hand-written impl.

- [ ] **Step 3: Check that the representation is out of reach**

Run: `rg -n "Representation::" crates --glob '!crates/ns-runtime/src/heap/value.rs'`
Expected: no output.

- [ ] **Step 4: Run the test and the per-commit gate, plus `cargo nextest run --workspace --release`.** Expected: all pass; the release size tests are unchanged (`Signal` 48, `Object` 56).
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "refactor: Value is opaque and lives under heap, beside the handles it takes apart"
```

---

### Task 5: An index, size or count beyond `SmallInteger` is out of range, not "no Integer"

**Files:**
- Modify: `crates/ns-runtime/src/builtins/collections.rs:11-24` (`out_of_range`), `:55-108` (`element_at`, `element_at_put`), `:120-165` (`allocate`)
- Modify: `crates/ns-runtime/src/builtins/text.rs:130-170` (`copy_range`), `:195-215` (`ElementAt`)
- Modify: `crates/ns-runtime/src/builtins/value.rs:108-140` (`slotAt:`)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs:724-737` (`AsStringFixed`)
- Create: `goldens/library/collection/an_index_or_size_past_every_small_integer_is_out_of_range.ns` and `.toml`

**Interfaces:**
- Produces: `pub(crate) enum IntegerArgument { Small(i64), Large { negative: bool } }` and `pub(crate) fn integer_argument<'gc>(heap: &Heap<'gc>, argument: Option<&Value<'gc>>) -> Option<IntegerArgument>` in `collections.rs`; `out_of_range` takes `index: impl std::fmt::Display`.

Today `Array new: 18446744073709551616` answers "requires an Integer size", though a `LargeInteger` is an Integer; Task 6 would extend that defect to every integer between 2^60 and 2^63, which the goldens `array_new_refuses_what_is_no_size` and `a_string_index_error_is_caught` exercise. No index, size or count can reach a `LargeInteger`, so each leaf refuses one in the words it refuses an out-of-range `SmallInteger`. `String with:` already describes whatever it refuses and needs no change.

- [ ] **Step 1: Write the failing golden.** `an_index_or_size_past_every_small_integer_is_out_of_range.ns`:

```newspeak
(* An index, size or count that is a LargeInteger is refused in the words
   an out-of-range SmallInteger is: no index, size or count reaches one,
   and a LargeInteger is still an Integer. *)
class AnIndexOrSizePastEverySmallIntegerIsOutOfRange = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. huge = 18446744073709551616. |
        transcript show: ([(Array new: 3) at: huge] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([(Array new: 3) at: huge negated put: 1] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Array new: huge] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Array new: huge negated] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' at: huge] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' copyFrom: 1 to: huge] on: Error do: [:error | error messageText]); newLine.
        transcript show: (['abc' copyFrom: huge negated to: 2] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([1.5 asStringFixed: huge] on: Error do: [:error | error messageText]); newLine
    )
) : ()
```

`.toml`:

```toml
stdout = '''
index 18446744073709551616 out of range: valid indices are 1..3 (size 3)
index -18446744073709551616 out of range: valid indices are 1..3 (size 3)
new: cannot allocate an Array of 18446744073709551616 elements
new: requires a size of at least 0, got -18446744073709551616
index 18446744073709551616 out of range: valid indices are 1..3 (size 3)
copyFrom: 1 to: 18446744073709551616 out of range: valid ranges lie within 1..3, an empty range ends just before it starts (size 3)
copyFrom: -18446744073709551616 to: 2 out of range: valid ranges lie within 1..3, an empty range ends just before it starts (size 3)
asStringFixed: takes 0 to 20 fraction digits, got 18446744073709551616
'''
```

(Compare the header and key layout with a neighbouring golden's `.toml`; copy any other key it carries, such as `status`.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p ns-cli an_index_or_size_past_every_small_integer`
(If goldens are one generated test, run `cargo nextest run --workspace --exclude ns-lsp goldens` and find this file in the output.)
Expected: FAIL; lines read "requires an Integer index/size" and a type mismatch for `asStringFixed:`.

- [ ] **Step 3: Implement.** In `collections.rs`:

```rust
/// An index, size or count argument as a leaf reads it. A `LargeInteger`
/// is an Integer that no index, size or count can reach, so a leaf refuses
/// it in the words it refuses an out-of-range `SmallInteger`, never as
/// something that is no Integer.
pub(crate) enum IntegerArgument {
    Small(i64),
    Large { negative: bool },
}

pub(crate) fn integer_argument<'gc>(
    heap: &Heap<'gc>,
    argument: Option<&Value<'gc>>,
) -> Option<IntegerArgument> {
    let argument = *argument?;
    if let Some(number) = argument.as_small_integer() {
        return Some(IntegerArgument::Small(number));
    }
    let integer = heap.large_integer(argument.as_object()?)?;
    Some(IntegerArgument::Large {
        negative: integer.is_negative(),
    })
}
```

`out_of_range(receiver, selector, index: impl std::fmt::Display, length)`. Each leaf matches `integer_argument(&mutator.heap, arguments.first())`: `Small` keeps today's path; `Large { .. }` takes the out-of-range error with `mutator.heap.describe(argument)` as the index text; `None` keeps `not_an_integer`. `allocate`: `Large { negative: true }` → "requires a size of at least 0, got {described}", `Large { negative: false }` → "cannot allocate an Array of {described} elements". `copy_range`: each bound is `Small(number)` or `Large`; if either is `Large` the result is the out-of-range error, its message built from both bounds' descriptions (a `Small` bound prints as its number, as today). `slotAt:` (`value.rs:108-140`): a `Large` index takes the branch an out-of-range `SmallInteger` takes, with the described text. `AsStringFixed`: `Large` → "asStringFixed: takes 0 to 20 fraction digits, got {described}".

- [ ] **Step 4: Run the golden, then the per-commit gate.** Expected: PASS; no other golden changes.
- [ ] **Step 5: Commit**

```bash
git add crates goldens
git commit -m "fix: an index, size or count beyond every SmallInteger is refused as out of range"
```

---

### Task 6: `SmallInteger` has 61 bits

**Files:**
- Modify: `crates/ns-runtime/src/heap/value.rs` (constants, `fits_small_integer`, the assert in `small_integer`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:1467-1479` (`new_integer`)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs` — `322-323` (`arithmetic`'s fast path), `394-414` (FloorDivide), `452-466` (ShiftLeft), `481-490` (Negated/Absolute), `165-180` (`checked_floor_divide`'s doc), the module doc `7-9` and `507-509`
- Modify: `crates/ns-runtime/src/vm.rs:651-704` (`IntegerBinary`), and `bytecode.rs:58-75,327-343` (its docs)
- Modify: `crates/ns-runtime/src/interpreter/literals.rs:72-94` and its test `204-228`
- Modify: `crates/ns-runtime/src/platform.rs:79-86` (`MonotonicNanoseconds`)
- Modify: `crates/ns-runtime/src/image/encode.rs:797-816,1392-1407`, `image/decode.rs:870-879,2125-2140,2387-2425`
- Modify: the invariant comments at `types.rs:68-72`, `bytecode.rs:20-24`, `interpreter/literals.rs:23-24`, `opcodes.rs:150-153,197-202`, `decode.rs:425-430,2081-2085`
- Modify goldens: `goldens/library/number/large-integer/large_integer_literals.*`, `…/an_overflowing_integer_operation_answers_a_large_integer.*`, `…/large_integer_arithmetic.*`, `goldens/library/number/float/float_sine_cosine_and_truncation.*`, `goldens/language/control-flow/number_iteration.ns` (comment and range), `goldens/library/string/a_string_index_error_is_caught.ns` (comment only)
- Modify tests: `crates/ns-runtime/tests/vm.rs:1253-1297`, `tests/builtins_boolean_numbers.rs:67-89,381-401`, `tests/image.rs:141-169,1164-1222,1261-1312`, `src/builtins/numbers.rs:767-790`

**Interfaces:**
- Consumes: Task 4's `heap/value.rs`, Task 5's leaves.
- Produces: `SMALL_INTEGER_MIN`, `SMALL_INTEGER_MAX`, `fits_small_integer`; `decode_scalar_value<'gc>(reader: &mut Reader, mutator: &mut Mutator<'gc>) -> Result<Value<'gc>, DecodeError>`.

Why the goldens move: §5.1.1 makes integers unbounded and says nothing of where a `SmallInteger` ends; `core-library.md`, "Large integers", makes it the representation's choice, and the word makes it 61 bits (`object-model.md`, "Integers are already unbounded; the word moves the boundary"). An integer between 2^60 and 2^63 in magnitude is therefore a `LargeInteger`, and `class` says so. Each golden keeps its intent by gaining the 61-bit edges beside its i64 ones.

- [ ] **Step 1: Write the failing tests.**

In `heap/value.rs`'s tests:

```rust
    #[test]
    fn a_small_integer_has_sixty_one_bits() {
        use super::{fits_small_integer, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};
        assert_eq!(SMALL_INTEGER_MIN, -1_152_921_504_606_846_976);
        assert_eq!(SMALL_INTEGER_MAX, 1_152_921_504_606_846_975);
        assert!(fits_small_integer(SMALL_INTEGER_MIN));
        assert!(fits_small_integer(SMALL_INTEGER_MAX));
        assert!(!fits_small_integer(SMALL_INTEGER_MIN - 1));
        assert!(!fits_small_integer(SMALL_INTEGER_MAX + 1));
        assert!(!fits_small_integer(i64::MIN));
        assert!(!fits_small_integer(i64::MAX));
    }
```

In `interpreter/literals.rs`, replace `a_literal_at_an_i64_edge_is_small_and_one_past_it_is_large` with:

```rust
    #[test]
    fn a_literal_at_a_small_integer_edge_is_small_and_one_past_it_is_large() {
        assert!(matches!(parse_integer_literal("1152921504606846975"), IntegerLiteral::Small(1_152_921_504_606_846_975)));
        assert!(matches!(parse_integer_literal("-1152921504606846976"), IntegerLiteral::Small(-1_152_921_504_606_846_976)));
        assert!(matches!(parse_integer_literal("1152921504606846976"), IntegerLiteral::Large(_)));
        assert!(matches!(parse_integer_literal("-1152921504606846977"), IntegerLiteral::Large(_)));
        assert!(matches!(parse_integer_literal("16r1000000000000000"), IntegerLiteral::Large(_)));
        assert!(matches!(parse_integer_literal("9223372036854775807"), IntegerLiteral::Large(_)));
        assert!(matches!(parse_integer_literal("-9223372036854775808"), IntegerLiteral::Large(_)));
    }
```

(Match the function's real name and return type at `literals.rs:72`; keep the `16r` and negative-radix cases the old test had, moved to the new edges.)

In `tests/vm.rs`, `an_inlined_integer_operation_leaves_promotion_and_division_by_zero_to_the_primitive` (1253-1297): replace `i64::MAX` with `SMALL_INTEGER_MAX` and expect `"1152921504606846976"`; replace `i64::MIN // -1` with `SMALL_INTEGER_MIN // -1` and expect `"1152921504606846976"`. In `tests/builtins_boolean_numbers.rs`: `integer_addition_overflow_answers_a_large_integer` stashes `SMALL_INTEGER_MAX` and expects `"1152921504606846976"`; `integer_multiplication_overflow_answers_a_large_integer` expects `"2305843009213693950"` for `SMALL_INTEGER_MAX * 2`. In `src/builtins/numbers.rs:767-790`, add `assert_eq!(checked_floor_divide(SMALL_INTEGER_MIN, -1), Some(1_152_921_504_606_846_976));` beside the i64 case, documenting that the leaf's range test, not the helper, declines it.

In `tests/image.rs`:
- `scalar_value_boundary_cases_round_trip` (141-169): the integer cases become `SMALL_INTEGER_MIN` and `SMALL_INTEGER_MAX` (immediate both ways), and add: an `i64::MAX` written under `VALUE_SMALL_INTEGER` (build the bytes with the file's writer helpers) decodes, through `decode_scalar_value(reader, mutator)` inside `interpreter.enter`, to a `LargeInteger` whose `describe` is `"9223372036854775807"`.
- `a_large_integer_value_round_trips` (1164-1222): `("smallClass", "Integer")` becomes `("smallClass", "LargeInteger")`; add a slot holding `1152921504606846976` and expect it to round-trip as `LargeInteger` with the same digits; fix the doc at 1165-1167.
- `non_canonical_large_integer_payloads` (1261-1312): unchanged — a tag 9/10 payload that fits `i64` is still refused (`NonCanonicalInteger`), because the encoder writes such an integer as an `i64`. Add one case to `a_large_integer_is_a_value_not_a_record` (1349) or beside it: a snapshot holding `1152921504606846976` is written under `VALUE_SMALL_INTEGER` (check with `inspect`), and loads back as a `LargeInteger`.

Golden expectations (the reason for each is in this task's header):
- `large_integer_literals`: `9223372036854775807 class` and `-9223372036854775808 class` now print `LargeInteger`; add `1152921504606846975 class` → `Integer`, `1152921504606846976 class` → `LargeInteger`, `-1152921504606846976 class` → `Integer`, `-1152921504606846977 class` → `LargeInteger`; rewrite the header comment (lines 2-3) to name the 61-bit edges.
- `an_overflowing_integer_operation_answers_a_large_integer`: `max class` prints `LargeInteger`; add the 61-bit cases with their exact outputs: `1152921504606846975 + 1` → `1152921504606846976`, `-1152921504606846976 - 1` → `-1152921504606846977`, `-1152921504606846976 negated` and `abs` and `// -1` → `1152921504606846976`, `1152921504606846975 * 2` → `2305843009213693950`, `(1152921504606846975 + 1 - 1) class` → `Integer`; rewrite the comment at lines 4-6 so it says the 61-bit literals are the ones `IntegerBinary` declines on.
- `large_integer_arithmetic`: line 36's `back class` (2^63 − 1) and line 40's `(small + 1) negated class` (−2^63) print `LargeInteger`.
- `float_sine_cosine_and_truncation`: line 18's `(0 - 9223372036854775808) asFloat asInteger class` prints `LargeInteger`.
- `number_iteration`: move the range at line 14 to `1152921504606846974 to: 1152921504606846976 do:` so it still crosses into `LargeInteger`; its output lines become those three numbers; fix the comment at line 4.
- `a_string_index_error_is_caught`: output unchanged (Task 5); rewrite comment lines 3-4 ("largest and smallest SmallInteger") to say the i64 edges, which are `LargeInteger`s.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run --workspace --exclude ns-lsp`
Expected: the new and changed tests and goldens FAIL (unresolved `SMALL_INTEGER_MAX`, `Integer` printed where `LargeInteger` is expected).

- [ ] **Step 3: Implement.** In `heap/value.rs`:

```rust
/// The bounds of a `SmallInteger`: 61 bits, what the tagged word leaves
/// (`docs/design/object-model.md`, "Planned: an eight-byte value"). Every
/// integer outside them is a `LargeInteger`.
pub const SMALL_INTEGER_MIN: i64 = -(1 << 60);
pub const SMALL_INTEGER_MAX: i64 = (1 << 60) - 1;

pub const fn fits_small_integer(value: i64) -> bool {
    SMALL_INTEGER_MIN <= value && value <= SMALL_INTEGER_MAX
}
```

`small_integer` gains `debug_assert!(fits_small_integer(value), "a SmallInteger outside 61 bits");`. Re-export through `heap.rs` and `types.rs`.

- `new_integer`: `match integer.to_i64().filter(|number| fits_small_integer(*number))`; doc: "one in 61 bits is a `SmallInteger`".
- `numbers.rs`: the shared fast path `checked(receiver_integer, argument).filter(|number| fits_small_integer(*number)).map(Value::small_integer)` — on `None` the existing wide path through `new_integer` answers; FloorDivide `checked_floor_divide(..).filter(fits)`, else `promote(mutator, -i128::from(receiver_integer))` (only `SMALL_INTEGER_MIN // -1` reaches it, and `i64::MIN` no longer can); ShiftLeft `i64::try_from(wide).ok().filter(fits)`; Negated/Absolute `checked_neg()`/`checked_abs()` `.filter(fits)`. Update the comments that name `i64::MIN`.
- `vm.rs` `IntegerBinary`: `.filter(|number| fits_small_integer(*number))` before `.map(Value::small_integer)` for Add, Subtract, Multiply, FloorDivide; update the comment at 675-677 and the instruction's doc in `bytecode.rs` ("the result fits a `SmallInteger`").
- `literals.rs`: both paths answer `Small` only when `fits_small_integer`.
- `platform.rs:79-86`: `mutator.new_integer(ArbitraryInteger::from_i128(i128::from(nanoseconds)))` instead of saturating at `i64::MAX` (which is no longer a `SmallInteger`); check what type the elapsed nanoseconds have there and convert losslessly.
- Encoder: `encode_value` (1402-1407) writes a `LargeInteger` whose `to_i64()` is `Some` as `VALUE_SMALL_INTEGER` + `write_i64`, every other as today; `encode_literal` (813-816) writes a `Literal::LargeInteger` that fits `i64` as `LITERAL_SMALL_INTEGER` + `write_i64`. Comment: the wire keeps one canonical encoding per integer, independent of where a `SmallInteger` ends.
- Decoder: `VALUE_SMALL_INTEGER` (2396) → `Value::small_integer(number)` if `fits_small_integer(number)`, else `context.mutator.new_integer(ArbitraryInteger::from_i64(number))` (the common case allocates nothing); `LITERAL_SMALL_INTEGER` (2128) → `Literal::SmallInteger(number)` if `fits_small_integer(number)`, else `Literal::LargeInteger(ArbitraryInteger::from_i64(number))`; `decode_scalar_value` gains `mutator: &mut Mutator<'gc>` and does the same for tag 3. `read_large_integer`'s refusal of a payload that fits `i64` stays.
- The invariant comments listed under **Files** now say "fits 61 bits"/"fits a `SmallInteger`" where they say "fits `i64`", except `read_large_integer`'s, which stays "fits `i64`" and says why (the wire's canonical form).

- [ ] **Step 4: Run the per-commit gate.** Expected: all pass, including every golden under both lowerings.
- [ ] **Step 5: Commit**

```bash
git add crates goldens
git commit -m "feat: a SmallInteger has 61 bits, and every integer beyond them is a LargeInteger"
```

---

### Task 7: Floats outside the `SmallFloat64` range are boxed

**Files:**
- Modify: `crates/ns-runtime/src/heap/value.rs` (`is_small_float_bits`, the assert in `small_float`)
- Modify: `crates/ns-runtime/src/types.rs:52-73` (`ObjectData::Float(f64)`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`new_float`)
- Modify: `crates/ns-runtime/src/heap.rs` (`float_value`, `describe` 1640-1666, `replace_resolved_futures` 3157, an `is_boxed_float` beside `is_large_integer` 1362)
- Modify: `crates/ns-runtime/src/trace.rs:66`
- Modify: `crates/ns-runtime/src/builtins/numbers.rs` (`float_equals`, `float_operand`, `argument_float`, `execute_float`), `builtins/value.rs:289-316,343-366` (`compare_node`, `hash_node`)
- Modify: `crates/ns-runtime/src/image/encode.rs:1045-1062,1117-1148,1392-1407,1625-1668`, `image/decode.rs:870-879,2387-2425` (and the amplification comment 2400-2417)
- Modify tests: `tests/image.rs:141-169`, `src/builtins/numbers.rs:746-764`
- Create: `goldens/library/number/float/a_float_beyond_the_immediate_range_behaves_as_any_float.ns` and `.toml`

**Interfaces:**
- Consumes: Task 4's `small_float`, Task 6's `decode_scalar_value(reader, mutator)`.
- Produces: `pub(crate) const fn is_small_float_bits(bits: u64) -> bool`; `ObjectData::Float(f64)`; `Heap::is_boxed_float(&self, object: ObjectReference<'gc>) -> bool`.

- [ ] **Step 1: Write the failing tests.** In `heap/value.rs`'s tests:

```rust
    #[test]
    fn the_small_float_range_is_spurs() {
        use super::is_small_float_bits;
        let bits = |number: f64| number.to_bits();
        assert!(is_small_float_bits(bits(0.0)));
        assert!(is_small_float_bits(bits(-0.0)));
        assert!(is_small_float_bits(bits(1.0)));
        assert!(is_small_float_bits(bits(-2.5)));
        assert!(is_small_float_bits(bits(1.0e38)));
        // Exponent 896 with a non-zero mantissa is the smallest immediate.
        assert!(is_small_float_bits((896 << 52) | 1));
        // Exponent 896 with a zero mantissa, 2^-127, would encode as zero.
        assert!(!is_small_float_bits(896 << 52));
        // Exponent 1151 is the largest immediate, 1152 the first boxed.
        assert!(is_small_float_bits((1151 << 52) | ((1 << 52) - 1)));
        assert!(!is_small_float_bits(1152 << 52));
        assert!(!is_small_float_bits(bits(1.0e300)));
        assert!(!is_small_float_bits(bits(1.0e-300)));
        assert!(!is_small_float_bits(bits(f64::MIN_POSITIVE / 2.0)));
        assert!(!is_small_float_bits(bits(f64::NAN)));
        assert!(!is_small_float_bits(bits(f64::INFINITY)));
        assert!(!is_small_float_bits(bits(f64::NEG_INFINITY)));
    }
```

In `tests/heap.rs`:

```rust
#[test]
fn every_float_reads_back_with_its_own_bits_whether_immediate_or_boxed() {
    let mut interpreter = Interpreter::new();
    interpreter.enter(|mutator| {
        // (number, whether it is boxed)
        for (number, boxed) in [
            (0.0, false), (-0.0, false), (1.5, false), (1.0e38, false),
            (f64::from_bits((896 << 52) | 1), false), (f64::from_bits(896 << 52), true),
            (1.0e300, true), (-1.0e300, true), (1.0e-300, true), (5.0e-324, true),
            (f64::NAN, true), (f64::INFINITY, true), (f64::NEG_INFINITY, true),
            (f64::MAX, true), (f64::MIN_POSITIVE, true),
        ] {
            let value = mutator.new_float(number);
            let back = mutator.heap().float_value(value).map(f64::to_bits);
            assert_eq!(back, Some(number.to_bits()), "{number}");
            assert_eq!(value.as_object().is_some(), boxed, "{number}");
        }
    });
}
```

The golden `a_float_beyond_the_immediate_range_behaves_as_any_float.ns`:

```newspeak
(* A float outside the immediate range -- huge, denormal -- is a Float like
   any other: same class, equality and hash by value, arithmetic in and out
   of the range. *)
class AFloatBeyondTheImmediateRangeBehavesAsAnyFloat = Object new (
) (
    public main: platform args: arguments = (
        | transcript = platform transcript. big = 1.0e300. tiny = 1.0 / 1.0e300 / 1.0e10. |
        transcript show: big class name; newLine.
        transcript show: (big + big = (big * 2.0)) printString; newLine.
        transcript show: ((big + big) hash = (big * 2.0) hash) printString; newLine.
        transcript show: (big + big == (big * 2.0)) printString; newLine.
        transcript show: (big / big = 1.0) printString; newLine.
        transcript show: (big > 1.0e38) printString; newLine.
        transcript show: (tiny class = big class) printString; newLine.
        transcript show: (tiny > 0.0) printString; newLine.
        transcript show: (tiny + tiny = (tiny * 2.0)) printString; newLine.
        transcript show: (tiny < (1.0 / 1.0e300)) printString; newLine.
        transcript show: (big asInteger > 0) printString; newLine
    )
) : ()
```

`.toml`:

```toml
stdout = '''
Float
true
true
true
true
true
true
true
true
true
true
'''
```

In `tests/image.rs`, rework `scalar_value_boundary_cases_round_trip` (141-169): the NaN and infinity cases move to a snapshot test that stores `0.0 / 0.0`, `1.0 / 0.0` and `1.0e300` in slots of a program, round-trips it with `build`/`load` the way `a_large_integer_value_round_trips` does, and checks each slot's `printString` (`NaN`, `inf`, and the `describe` of `1.0e300`) and that no object record is written for them (`inspect` shows `VALUE_FLOAT`, as `a_large_integer_is_a_value_not_a_record` checks its digits). Keep `0.0` and `-0.0` in the scalar test (immediate). Add: a `VALUE_FLOAT` carrying each of NaN's bits, infinity and `2^-127` decodes through `decode_scalar_value(reader, mutator)` to a boxed float with the same bits.

In `src/builtins/numbers.rs:746-764`, the NaN unit tests run inside an interpreter entry with `mutator.new_float(f64::NAN)` and the new `float_equals(heap, …)` signature.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run --workspace --exclude ns-lsp`
Expected: FAIL — `is_small_float_bits` unresolved; `boxed` is `false` for 1.0e300.

- [ ] **Step 3: Implement.**

```rust
/// Spur's `isSmallFloatValueBits`: an exponent of 897 to 1151, or 896 with a
/// non-zero mantissa, or ±0.0. That is the eight-bit subset of the exponent
/// the tagged word keeps (`docs/design/object-model.md`, "Planned: an
/// eight-byte value"); every other float is a boxed `Float`.
pub(crate) const fn is_small_float_bits(bits: u64) -> bool {
    const OFFSET: u64 = 896;
    let exponent = (bits >> 52) & 0x7FF;
    let mantissa = bits & ((1 << 52) - 1);
    if exponent > OFFSET {
        exponent <= OFFSET + 0xFF
    } else if mantissa != 0 {
        exponent == OFFSET
    } else {
        exponent == 0
    }
}
```

`small_float` gains `debug_assert!(is_small_float_bits(value.to_bits()))`.

- `types.rs`: `ObjectData::Float(f64)` with doc: "An instance of `Float` whose value lies outside the immediate range: NaN, the infinities, denormals and magnitudes outside about 2^-127 to 2^129. `Mutator::new_float` is the one way to make one and decides that, so each float has one representation. Holds no handle."
- `new_float`: `if is_small_float_bits(value.to_bits()) { Value::small_float(value) } else { let class = self.builtin_class(BuiltinClass::Float); Value::object(self.new_instance_object(class, ObjectData::Float(value))) }`.
- `float_value`: `SmallFloat(number)` → `Some(number)`; `Object(object)` → the object's `ObjectData::Float(number)` → `Some(number)`; else `None`. Read the payload, never the class, so that `Float new` (an `Instance`) is no number, as `LargeInteger new` is not.
- The five exhaustive `ObjectData` matches gain the arm: `trace.rs:66` (no handle), `heap.rs` `describe` (`number.to_string()`, as the immediate arm), `heap.rs:3157` (no handle), `encode.rs:1147` (no record), `encode.rs:1666` (`unreachable!("a boxed Float is written as a value, never as an object record")`).
- Encoder: `visit_value` (1060) skips a boxed float as it skips a `LargeInteger`; `encode_value` writes it as `VALUE_FLOAT` + `write_f64`.
- Decoder: `VALUE_FLOAT` → `context.mutator.new_float(..)` (already, since Task 2); `decode_scalar_value`'s tag 4 → `mutator.new_float(..)`; remove the decoder's use of `Value::small_float`. Extend the amplification comment at 2400-2417: a boxed float is a 9-byte value that allocates one `Object` (56 bytes release, 72 debug), the same object a `LargeInteger` allocates from at least 14 bytes, so its term is 64/9 ≈ 7.1x release (80/9 ≈ 8.9x debug) where the `LargeInteger` term is 4.6x (5.7x); state the new worst case in the comment and record it for Task 12, which updates `memory.md` and `CLAUDE.md`.
- `numbers.rs`: `execute_float`'s receiver `let receiver_float = mutator.heap.float_value(receiver)?;`; `argument_float` and `float_operand` read `heap.float_value` first; `float_equals(heap, receiver, argument)` compares `heap.float_value(argument)` (an immediate never equals a boxed float, since the ranges are disjoint, but reading through `float_value` needs no case for it); results box through `new_float` (Task 2 already routes them there).
- `value.rs` `compare_node`: after the `LargeInteger` arm, `if let Some(number) = mutator.heap().float_value(Value::object(left)) { return Some(Pair::Decided(numbers::float_equals(mutator.heap(), number, Some(&second)))); }`; `hash_node`: likewise `Node::Leaf(numbers::float_hash(number))`. Delete the stale comment at `value.rs:385`.

- [ ] **Step 4: Run the per-commit gate.** Expected: all pass; the existing NaN goldens (`not_a_number_equals_itself`, `a_value_with_a_not_a_number_slot_equals_itself`, `an_integer_meets_a_float`, `float_orders_with_less_or_equal`, `float_sine_cosine_and_truncation`, `float_square_root`) pass unchanged — they now run on boxed NaNs.
- [ ] **Step 5: Commit**

```bash
git add crates goldens
git commit -m "feat: a float outside the SmallFloat64 range is a boxed Float"
```

---

### Task 8: The value is the tagged word

**Files:**
- Modify: `crates/ns-runtime/src/heap/value.rs` (the representation)
- Modify: `crates/ns-runtime/src/heap.rs:38-48` (`immediate_index`), `:1969-1987` (`dispatch_id`)
- Modify: the size tests `interpreter/mod.rs:2353-2369,2440-2446`, `heap.rs:4219-4223`

**Interfaces:**
- Consumes: the API (unchanged).
- Produces: `pub(super) fn immediate_index(self) -> usize` on `Value`; the word layout below, which Tasks 9 and 10 rely on: tag mask `0b111`; object `index << 3`; `SmallInteger` `(value << 3) | 0b001`; constants under `0b010` with payload 0 `nil`, 1 `false`, 2 `true`; `SmallFloat64` Spur's rotation under `0b100`.

- [ ] **Step 1: Write the failing tests** in `heap/value.rs`'s tests:

```rust
    #[test]
    fn every_kind_survives_the_word() {
        use super::{SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};
        for number in [0, 1, -1, 42, SMALL_INTEGER_MIN, SMALL_INTEGER_MAX] {
            assert_eq!(Value::<'static>::small_integer(number).as_small_integer(), Some(number));
        }
        for number in [0.0, -0.0, 1.0, -2.5, 1.0e38, f64::from_bits((896 << 52) | 1), f64::from_bits((1151 << 52) | ((1 << 52) - 1))] {
            let value = Value::<'static>::small_float(number);
            assert!(matches!(value.kind(), super::ValueKind::SmallFloat(back) if back.to_bits() == number.to_bits()), "{number}");
        }
        assert!(Value::<'static>::nil().is_nil());
        assert_eq!(Value::<'static>::boolean(false).as_boolean(), Some(false));
        assert_eq!(Value::<'static>::boolean(true).as_boolean(), Some(true));
        assert!(Value::<'static>::boolean(false).as_small_integer().is_none());
    }

    #[test]
    fn the_words_are_spurs() {
        assert_eq!(Value::<'static>::small_integer(5).word, (5 << 3) | 1);
        assert_eq!(Value::<'static>::small_integer(-1).word, u64::MAX & !0b110);
        assert_eq!(Value::<'static>::nil().word, 0b010);
        assert_eq!(Value::<'static>::boolean(false).word, (1 << 3) | 0b010);
        assert_eq!(Value::<'static>::boolean(true).word, (2 << 3) | 0b010);
        assert_eq!(Value::<'static>::small_float(0.0).word, 0b100);
        assert_eq!(Value::<'static>::small_float(-0.0).word, (1 << 3) | 0b100);
        assert_ne!(Value::<'static>::small_float(1.0).word & 0b111, 0);
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn a_release_value_is_one_word() {
        assert_eq!(std::mem::size_of::<Value<'static>>(), 8);
    }
```

And in `crates/ns-runtime/tests/heap.rs`, an object handle survives: allocate an array through the mutator, wrap it with `Value::object`, and check `as_object()` answers a handle `same` as the original and `heap.describe` of both agrees.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime every_kind_survives the_words_are_spurs`
Expected: FAIL to compile (`no field 'word'`).

- [ ] **Step 3: Implement.** Replace `Representation` with:

```rust
#[derive(Clone, Copy)]
pub struct Value<'gc> {
    word: u64,
    /// Debug builds only, as on the handle the word may carry: the heap
    /// and the move epoch it was minted in, so that the origin and epoch
    /// checks stay exact (`docs/design/memory.md`, "The brand"). Zero for an
    /// immediate.
    #[cfg(debug_assertions)]
    origin: u32,
    #[cfg(debug_assertions)]
    epoch: u32,
    brand: Brand<'gc>,
}

const TAG_MASK: u64 = 0b111;
const OBJECT_TAG: u64 = 0b000;
const SMALL_INTEGER_TAG: u64 = 0b001;
const CONSTANT_TAG: u64 = 0b010;
const SMALL_FLOAT_TAG: u64 = 0b100;
const TAG_BITS: u32 = 3;
const NIL_WORD: u64 = CONSTANT_TAG;
const FALSE_WORD: u64 = (1 << TAG_BITS) | CONSTANT_TAG;
const TRUE_WORD: u64 = (2 << TAG_BITS) | CONSTANT_TAG;
/// Spur's `smallFloatExponentOffset` shifted to where the rotated exponent
/// sits: the eight-bit exponent the word keeps starts at 897.
const SMALL_FLOAT_EXPONENT_OFFSET: u64 = 896 << 53;
```

with a private `const fn immediate(word: u64) -> Value<'gc>` setting `origin` and `epoch` to 0, and:

- `nil()`/`boolean()`: `immediate(NIL_WORD)`, `immediate(if value { TRUE_WORD } else { FALSE_WORD })`.
- `small_integer(value)`: `immediate(((value as u64) << TAG_BITS) | SMALL_INTEGER_TAG)` after the assert.
- `object(object)`: `Value { word: u64::from(object.index) << TAG_BITS, origin: object.origin, epoch: object.epoch, brand: object.brand }` (debug fields under `cfg`).
- `small_float(value)`: after the assert, Spur's `smallFloatObjectOfBits`:

```rust
        let mut rotated = value.to_bits().rotate_left(1);
        if rotated > 1 {
            rotated -= SMALL_FLOAT_EXPONENT_OFFSET;
        }
        Value::immediate((rotated << TAG_BITS) | SMALL_FLOAT_TAG)
```

- `kind()`: by `self.word & TAG_MASK`: `OBJECT_TAG` → `Object(self.reference())`; `SMALL_INTEGER_TAG` → `SmallInteger((self.word as i64) >> TAG_BITS)`; `CONSTANT_TAG` → `NIL_WORD`/`FALSE_WORD`/`TRUE_WORD`; `SMALL_FLOAT_TAG` → Spur's `smallFloatValueOf`:

```rust
        let mut rotated = self.word >> TAG_BITS;
        if rotated > 1 {
            rotated += SMALL_FLOAT_EXPONENT_OFFSET;
        }
        f64::from_bits(rotated.rotate_right(1))
```

  and any other tag `unreachable!("a value is made only by its constructors")`.
- `as_small_integer`: `(self.word & 1 == 1).then(|| (self.word as i64) >> TAG_BITS)` — bit 0 alone tells a `SmallInteger`, since tags 011, 101 and 111 are never produced. `is_nil`: `self.word == NIL_WORD`. `as_boolean`: `TRUE_WORD` → `Some(true)`, `FALSE_WORD` → `Some(false)`, else `None`. `as_object`: `(self.word & TAG_MASK == OBJECT_TAG).then(|| self.reference())`, where `reference()` rebuilds the `ObjectReference` from `(self.word >> TAG_BITS) as u32`, `origin`, `epoch`, `brand`.
- `immediate_index(self) -> usize`: `NIL_WORD` → 0, `FALSE_WORD` → 1, `TRUE_WORD` → 2, a `SmallInteger` → 3, a `SmallFloat` → 4; `heap.rs`'s free function becomes this method and `dispatch_id` calls `value.immediate_index()` when `as_object()` is `None`.
- `at_rest`/`branded`: copy every field and replace the brand.

Run the release size tests (`cargo nextest run --workspace --release`) and read the new sizes off the failures: `Signal`, `Result<Step, Signal>` (release and the debug assertion at `interpreter/mod.rs:2365-2369`), `EvaluationResult`, `Object`. Each must be at most its old value. Pin the new figures and say in each test's comment that the eight-byte value shrank them.

- [ ] **Step 4: Run the per-commit gate and `cargo nextest run --workspace --release`.** Expected: all pass; no golden changed.
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat: a value is one tagged word, Spur's tags, in a release build"
```

---

### Task 9: The heap stores bare words

**Files:**
- Modify: `crates/ns-runtime/src/heap/value.rs` (`Word`)
- Modify: `crates/ns-runtime/src/types.rs` — `ObjectData::Instance(Vec<Word>)`, `ObjectData::Array(Vec<Word>)`, `ClosureData.copied: Box<[Word]>` (types.rs:362), `Frame.receiver: Word`, `Frame.arguments: Vec<Word>`, `Frame.temporaries: Vec<Word>` (types.rs:684-688)
- Modify: `crates/ns-runtime/src/heap.rs` — `ActivationStack.values: Vec<Word>` (418), every accessor of the "Objects", "Closures", "Frames" and "Activation stack" tables below, `forward_everything` (3008-3041), `begin_marking` (3181-3198), `replace_resolved_futures` (3095-3169), `Forwarder`'s `Tracer` impl (687-721), the test `a_stale_handle_stored_in_the_heap_is_caught_by_the_next_move` (4174-4190)
- Modify: `crates/ns-runtime/src/heap/mark.rs:86-98`, `crates/ns-runtime/src/trace.rs` (`Tracer::word`, `trace_words`, the `ObjectData`, `ClosureData`, `Frame` impls)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`new_array`, `new_instance_object` callers that build `Instance`/`Array`), `interpreter/instantiate.rs:499,563`, `interpreter/future.rs:183`, `vm.rs:453-506` (`MakeClosure`, `MakeArray`, `MakeVector`), `image/decode.rs:2480-2510,2544,3228,3293,3297`, `image/encode.rs:1119,1625,1639` (read through `load`)
- Modify tests: `tests/image.rs:2158,7990-8008` (they build or match `ObjectData::Instance`, `Frame`, `ClosureData` directly)

**Interfaces:**
- Consumes: Task 8's word layout.
- Produces: `pub struct Word` (opaque: no public constructor or accessor), `pub(crate) const Word::NIL: Word`, `Heap::load(&self, word: Word) -> Value<'gc>` and `Heap::store(&self, value: Value<'gc>) -> Word`, both `#[inline(always)]`; `Tracer::word(&mut self, word: &mut Word)`.

Which storage becomes `Word`: everything inside a heap object or the activation stack's register file -- fields, elements, registers (live and reified), copied closure values, and a reified frame's receiver. What stays a branded `Value` or `ObjectReference` with its `Trace`: `StackEntry.receiver` and every `Continuation` (Rust-side records owned by one stack entry), `ClassData`'s handles, `Root` slots, `Mutator`'s roots, argument buffers, `Step`, `Signal`.

- [ ] **Step 1: Write the failing tests.** In `heap/value.rs`'s tests:

```rust
    #[test]
    fn a_word_is_eight_bytes_in_every_build() {
        assert_eq!(std::mem::size_of::<super::Word>(), 8);
    }
```

In `heap.rs`'s tests, replace `a_stale_handle_stored_in_the_heap_is_caught_by_the_next_move` with:

```rust
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "stale handle")]
    fn a_stale_handle_is_caught_where_it_is_stored() {
        // The body of the old test up to its `array_set`, unchanged: mint a
        // handle, move the heap so the handle's epoch is old, then store it.
        // The store itself panics now, since a stored word carries no epoch
        // for the next move to check.
    }
```

(Copy the old test's setup verbatim; only the expectation's location changes. The panic text comes from `check_object_origin`; match its actual wording.)

And in `crates/ns-runtime/tests/collector.rs` (copy a neighbour's setup): a program that builds an array of 1,000 boxes, each holding a `SmallInteger`, a boxed float, a `LargeInteger`, `nil` and another box, runs under `set_stress_moves(true)` and `set_stress_collect(true)` (the existing test hooks), and answers the sum and the descriptions it reads back afterwards; assert the output.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-runtime a_word_is_eight_bytes a_stale_handle_is_caught_where_it_is_stored`
Expected: FAIL to compile (`Word` unresolved), then the stale-handle test fails with no panic.

- [ ] **Step 3: Implement.**

```rust
/// A value as a heap object stores it: the bare word, eight bytes in every
/// build, with no origin, epoch or brand (`docs/design/object-model.md`,
/// "The debug build keeps its exact check"). Only `Heap::load` and
/// `Heap::store` convert, so a stored word is always one the heap checked.
#[derive(Clone, Copy)]
pub struct Word(u64);

impl Word {
    pub(crate) const NIL: Word = Word(NIL_WORD);

    /// The arena index of the object this word names, for the collector.
    pub(super) fn object_index(self) -> Option<u32> {
        (self.0 & TAG_MASK == OBJECT_TAG).then(|| (self.0 >> TAG_BITS) as u32)
    }

    pub(super) fn with_object_index(index: u32) -> Word {
        Word(u64::from(index) << TAG_BITS)
    }
}
```

`Value` gains `pub(super) fn word(self) -> Word` and `pub(super) fn from_word(word: Word, origin: u32, epoch: u32) -> Value<'gc>` (debug fields under `cfg`, origin and epoch applied only to an object word). In `heap.rs`:

```rust
    /// A stored word as a value: given this heap's id and current epoch,
    /// since every collection rewrites every stored word, so no stored word
    /// can be stale.
    #[inline(always)]
    pub fn load(&self, word: Word) -> Value<'gc> {
        Value::from_word(word, self.heap_id, self.epoch)
    }

    /// A value as a word to store. In a debug build the value must carry
    /// this heap's id and current epoch: a stored word has nothing left for
    /// the next move to check, so a stale handle is refused here or never.
    #[inline(always)]
    pub fn store(&self, value: Value<'gc>) -> Word {
        #[cfg(debug_assertions)]
        if let Some(object) = value.as_object() {
            self.check_object_origin(object);
        }
        value.word()
    }
```

(In a release build `from_word` ignores the two numbers; pass them under `cfg` so a release build reads no field it does not have.)

Every accessor converts at its boundary and its signature does not change: `array_at`, `instance_field`, `register`, `activation_local`, `frame_local`, `frame_receiver` answer `self.load(..)`; `array_set`, `set_instance_field`, `set_register`, `activation_set_local`, `frame_set_local`, `push_activation`, `fill_copied_values` take a `Value` and store `self.store(value)`; `array_elements` maps `load`; `resize(n, Value::nil())` becomes `resize(n, Word::NIL)`. `reify` stores the entry's receiver into the frame with `store`; `pop_activation`'s copies are already words. `Mutator::new_array(elements: Vec<Value<'gc>>)` and every builder of `ObjectData::Instance`/`Array`/`ClosureData`/`Frame` (instantiate.rs:499,563; future.rs:183; vm.rs `MakeClosure`/`MakeArray`/`MakeVector`; decode.rs:2480-2510,2544,3228,3293,3297) store each value through `heap.store`. The encoder reads fields and frames through `load` (encode.rs:1119,1625,1639, and `frame_snapshot`'s callers). `replace_resolved_futures` reads the words through `load` and writes back through `store`.

The collector:

```rust
pub(crate) trait Tracer {
    fn object(&mut self, object: &mut ObjectReference<'_>);
    fn frame(&mut self, frame: &mut FrameReference<'_>);
    /// A stored word: rewritten by its index, since it carries no epoch.
    fn word(&mut self, word: &mut Word);
    fn value(&mut self, value: &mut Value<'_>) { /* unchanged since Task 4 */ }
}

fn trace_words<T: Tracer>(words: &mut [Word], tracer: &mut T) {
    for word in words {
        tracer.word(word);
    }
}
```

`Forwarder::word`: `if let Some(index) = word.object_index() { *word = Word::with_object_index(self.relocation.apply(index)); }` (the same relocation `Forwarder::object` applies; check `ForwardingTable::apply`'s `DEAD` assertion still fires for a word). `Marker::word`: mark `object_index` as `Marker::object` marks an index. `trace.rs`'s `ObjectData` (`Instance`, `Array`), `ClosureData` (`copied`) and `Frame` (`receiver`, `arguments`, `temporaries`) impls call `tracer.word`/`trace_words`; `forward_everything` and `begin_marking` walk `stack.values` with `word`. `Word`'s `object_index`/`with_object_index` are `pub(super)` in `heap::value`, visible to `heap.rs` and `heap/mark.rs`; `trace.rs` only calls `tracer.word`.

Tests that build or match `ObjectData::Instance`, `Frame` or `ClosureData` directly (`tests/image.rs:2158,7990-8008`) go through the heap's accessors (`instance_field`, `frame_local`) or build through the mutator instead; `Word` has no public constructor on purpose.

- [ ] **Step 4: Run the per-commit gate, `cargo nextest run --workspace --release`, and the move stress mode over the whole suite once**

Run: `$env:NS_NEXT_STRESS_MOVES = "1"; cargo nextest run --workspace --exclude ns-lsp; Remove-Item Env:NS_NEXT_STRESS_MOVES` (PowerShell)
Expected: all pass.
- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat: a heap object stores bare words, and a store checks the value it stores"
```

---

### Task 10: `SmallInteger` arithmetic on the tagged words

**Files:**
- Modify: `crates/ns-runtime/src/heap/value.rs` (the operations)
- Modify: `crates/ns-runtime/src/vm.rs:651-704` (`IntegerBinary`)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs:316-362` (`arithmetic`'s and `compare`'s `SmallInteger` paths)

**Interfaces:**
- Consumes: Task 8's word layout.
- Produces, on `Value`, each `#[inline(always)] pub(crate)`:

```rust
pub(crate) fn small_integer_sum(self, other: Value<'gc>) -> Option<Value<'gc>>;
pub(crate) fn small_integer_difference(self, other: Value<'gc>) -> Option<Value<'gc>>;
pub(crate) fn small_integer_product(self, other: Value<'gc>) -> Option<Value<'gc>>;
pub(crate) fn small_integer_floor_quotient(self, other: Value<'gc>) -> Option<Value<'gc>>;
pub(crate) fn small_integer_order(self, other: Value<'gc>) -> Option<std::cmp::Ordering>;
```

Each answers `None` when either operand is not a `SmallInteger` or the result is not one (overflow, a zero divisor, `SMALL_INTEGER_MIN // -1`); the caller then falls back exactly as today.

- [ ] **Step 1: Write the failing tests** in `heap/value.rs`'s tests:

```rust
    #[test]
    fn tagged_arithmetic_answers_what_untagged_arithmetic_answers() {
        use super::{SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};
        let small = Value::<'static>::small_integer;
        let samples = [0, 1, -1, 7, -7, 3, 1 << 30, -(1 << 30), SMALL_INTEGER_MIN, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN + 1, SMALL_INTEGER_MAX - 1];
        for left in samples {
            for right in samples {
                let expect = |result: Option<i64>| result.filter(|number| super::fits_small_integer(*number));
                assert_eq!(small(left).small_integer_sum(small(right)).and_then(Value::as_small_integer), expect(left.checked_add(right)), "{left} + {right}");
                assert_eq!(small(left).small_integer_difference(small(right)).and_then(Value::as_small_integer), expect(left.checked_sub(right)), "{left} - {right}");
                assert_eq!(small(left).small_integer_product(small(right)).and_then(Value::as_small_integer), expect(left.checked_mul(right)), "{left} * {right}");
                assert_eq!(small(left).small_integer_floor_quotient(small(right)).and_then(Value::as_small_integer), expect(crate::builtins::numbers::checked_floor_divide(left, right)), "{left} // {right}");
                assert_eq!(small(left).small_integer_order(small(right)), Some(left.cmp(&right)), "{left} <=> {right}");
            }
        }
        assert!(small(1).small_integer_sum(Value::nil()).is_none());
        assert!(Value::<'static>::boolean(true).small_integer_order(small(1)).is_none());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p ns-runtime tagged_arithmetic_answers`
Expected: FAIL to compile (`no method named 'small_integer_sum'`).

- [ ] **Step 3: Implement.**

```rust
    /// Whether both are `SmallInteger`s: one test on the two words, since
    /// bit 0 alone tells a `SmallInteger`.
    #[inline(always)]
    fn both_small_integers(self, other: Value<'gc>) -> bool {
        self.word & other.word & SMALL_INTEGER_TAG == SMALL_INTEGER_TAG
    }

    /// `(a << 3 | 1) + (b << 3)` is `(a + b) << 3 | 1`, and it overflows
    /// `i64` exactly when `a + b` leaves 61 bits, so the one overflow test
    /// is the range test.
    #[inline(always)]
    pub(crate) fn small_integer_sum(self, other: Value<'gc>) -> Option<Value<'gc>> {
        if !self.both_small_integers(other) {
            return None;
        }
        let word = (self.word as i64).checked_add(other.word as i64 - 1)?;
        Some(Value::immediate(word as u64))
    }

    /// `(a << 3 | 1) - (b << 3)` is `(a - b) << 3 | 1`, with the same one test.
    #[inline(always)]
    pub(crate) fn small_integer_difference(self, other: Value<'gc>) -> Option<Value<'gc>> {
        if !self.both_small_integers(other) {
            return None;
        }
        let word = (self.word as i64).checked_sub(other.word as i64 - 1)?;
        Some(Value::immediate(word as u64))
    }

    /// One operand untagged, as Cog's and Soup's: `a * (b << 3)` is
    /// `(a * b) << 3`, overflowing `i64` exactly when `a * b` leaves 61 bits.
    #[inline(always)]
    pub(crate) fn small_integer_product(self, other: Value<'gc>) -> Option<Value<'gc>> {
        if !self.both_small_integers(other) {
            return None;
        }
        let shifted = ((self.word as i64) >> TAG_BITS).checked_mul(other.word as i64 - 1)?;
        Some(Value::immediate(shifted as u64 | SMALL_INTEGER_TAG))
    }

    /// Both untagged: the floor quotient of a tagged word is not the tagged
    /// floor quotient. `checked_floor_divide` is the leaves' own, so the
    /// rounding rule has one copy.
    #[inline(always)]
    pub(crate) fn small_integer_floor_quotient(self, other: Value<'gc>) -> Option<Value<'gc>> {
        if !self.both_small_integers(other) {
            return None;
        }
        let quotient = crate::builtins::numbers::checked_floor_divide(
            (self.word as i64) >> TAG_BITS,
            (other.word as i64) >> TAG_BITS,
        )?;
        fits_small_integer(quotient).then(|| Value::small_integer(quotient))
    }

    /// Tagging preserves order, so the words compare as the integers do.
    #[inline(always)]
    pub(crate) fn small_integer_order(self, other: Value<'gc>) -> Option<std::cmp::Ordering> {
        self.both_small_integers(other)
            .then(|| (self.word as i64).cmp(&(other.word as i64)))
    }
```

(`Value::immediate` is Task 8's private constructor; a result built from two immediates carries no origin or epoch.)

`vm.rs` `IntegerBinary`:

```rust
                let left_value = mutator.heap.register(register_base, *left);
                let right_value = mutator.heap.register(register_base, *right);
                let result = match operation {
                    IntegerOperation::Add => left_value.small_integer_sum(right_value),
                    IntegerOperation::Subtract => left_value.small_integer_difference(right_value),
                    IntegerOperation::Multiply => left_value.small_integer_product(right_value),
                    IntegerOperation::FloorDivide => left_value.small_integer_floor_quotient(right_value),
                    IntegerOperation::Less => left_value.small_integer_order(right_value).map(|order| Value::boolean(order.is_lt())),
                    IntegerOperation::Greater => left_value.small_integer_order(right_value).map(|order| Value::boolean(order.is_gt())),
                    IntegerOperation::LessOrEqual => left_value.small_integer_order(right_value).map(|order| Value::boolean(order.is_le())),
                    IntegerOperation::GreaterOrEqual => left_value.small_integer_order(right_value).map(|order| Value::boolean(order.is_ge())),
                    IntegerOperation::Equal => left_value.small_integer_order(right_value).map(|order| Value::boolean(order.is_eq())),
                };
```

followed by the existing `match result`; keep and update the comment above the arm (the arm declines every case it does not answer). `numbers.rs`: the `SmallInteger` paths of `arithmetic` for Add, Subtract, Multiply call `receiver.small_integer_sum(argument)` (and siblings) first, falling back to the existing wide path on `None` when the argument is a `SmallInteger`; `compare`'s `SmallInteger` path calls `small_integer_order`. The other leaves keep reading `as_small_integer`.

- [ ] **Step 4: Run the per-commit gate.** Expected: all pass, including `tests/vm.rs`'s `an_inlined_integer_operation_*` tests and the integer goldens under both lowerings.
- [ ] **Step 5: Check the inlining** (the `measuring-performance` skill, step 4). In PowerShell:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
$env:CARGO_PROFILE_RELEASE_DEBUG = "limited"
cargo build --release -p ns-cli
Remove-Item Env:RUSTFLAGS; Remove-Item Env:CARGO_PROFILE_RELEASE_DEBUG
& "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\dumpbin.exe" /disasm target\release\ns-next.exe > .temp\disasm.txt
Select-String -Path .temp\disasm.txt -Pattern "vm::run" | Select-Object -First 20
```

Expected: one `ns_runtime::vm::run` label, no `vm::run::closure`, no `run_stretch`, and none of the `small_integer_*` methods as a function of its own (`Select-String -Pattern "small_integer_"` finds no label). If one is outlined, force it with `#[inline(always)]` where it is missing and re-check. Record `vm::run`'s instruction count (lines between its label and the next) for Task 11.
- [ ] **Step 6: Commit**

```bash
git add crates
git commit -m "perf: SmallInteger arithmetic works on the tagged words, one test for both operands"
```

---

### Task 11: Measured against the tree the branch started from

**Files:**
- Modify: `docs/design/execution.md` (a new section "What the eight-byte value moved", after "The Are We Fast Yet benchmarks")

**Interfaces:**
- Consumes: the finished tree (Task 10's commit).

The protocol is the baseline's (`execution.md`, "The Are We Fast Yet benchmarks", "The baseline"), applied to two trees in one sitting. Nothing else may run on the machine while it measures. **The controller session runs this task itself, not a subagent**: the measurement takes over an hour, beyond the ten-minute limit of a foreground command, so it runs as one background command whose completion notification the controller waits for.

- [ ] **Step 1: Build three pinned binaries of each tree.** The before-tree is `git merge-base master eight-byte-value`. In PowerShell from the repository root:

```powershell
$before = git merge-base master eight-byte-value
New-Item -ItemType Directory -Force .temp\measure | Out-Null
git worktree add .temp\measure\before $before
foreach ($tree in @(@{ Name = "before"; Path = ".temp\measure\before" }, @{ Name = "after"; Path = "." })) {
    foreach ($metadata in "alpha", "beta", "gamma") {
        $env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6 -C metadata=$metadata"
        cargo build --release -p ns-cli --manifest-path "$($tree.Path)\Cargo.toml" --target-dir ".temp\measure\target-$($tree.Name)-$metadata"
        Copy-Item ".temp\measure\target-$($tree.Name)-$metadata\release\ns-next.exe" ".temp\measure\ns-next-$($tree.Name)-$metadata.exe"
    }
}
Remove-Item Env:RUSTFLAGS
git worktree remove .temp\measure\before
```

Check each of the six with `dumpbin /dependents` (no `VCRUNTIME140.dll`) and that `ns-next-<tree>-<metadata>.exe run benchmarks/are-we-fast-yet/are-we-fast-yet.json` prints `ok` for all sixteen. Write the four microbenchmark sources that are not in the tree (`bench_new.ns`, `bench_new2.ns`, `bench_newarg.ns`, `bench_do.ns`) verbatim from `execution.md:1698-1775` into `target\bench-lookup\`; check each binary prints the expected output for them and for `goldens/tool/bench/*.ns`.

- [ ] **Step 2: Measure.** Write `.temp\measure\measure.ps1`:

```powershell
$directory = ".temp\measure"
$project = "benchmarks/are-we-fast-yet/are-we-fast-yet.json"
$benchmarks = "Bounce","CD","DeltaBlue","DeltaBlueTon80","Havlak","Json","List","Mandelbrot","NBody","Permute","Queens","Richards","RichardsTon80","Sieve","Storage","Towers"
$sources = (Get-ChildItem goldens\tool\bench\*.ns).FullName + (Get-ChildItem target\bench-lookup\*.ns).FullName
$binaries = foreach ($tree in "before","after") { foreach ($metadata in "alpha","beta","gamma") { "$tree-$metadata" } }
function ToMilliseconds($number, $unit) { switch ($unit) { "s" { [double]$number * 1000 } "ms" { [double]$number } default { [double]$number / 1000 } } }
function Median($output) {
    $line = $output | Select-Object -Last 1
    if ($line -notmatch "median ([\d.]+)(s|ms|µs) ") { throw "unparsed: $line" }
    ToMilliseconds $Matches[1] $Matches[2]
}
"round,binary,source,median_ms" | Set-Content "$directory\bench.csv"
"round,binary,name,runs_per_second" | Set-Content "$directory\timed.csv"
foreach ($round in 1..3) {
    $order = 0..5 | ForEach-Object { $binaries[($_ + $round - 1) % 6] }
    foreach ($name in $benchmarks) { foreach ($binary in $order) {
        "$round,$binary,$name,$(Median (& "$directory\ns-next-$binary.exe" bench $project --iterations 10 -- $name))" | Add-Content "$directory\bench.csv"
    } }
    foreach ($source in $sources) { foreach ($binary in $order) {
        "$round,$binary,$(Split-Path $source -Leaf),$(Median (& "$directory\ns-next-$binary.exe" bench $source --iterations 30))" | Add-Content "$directory\bench.csv"
    } }
    foreach ($binary in $order) {
        foreach ($line in (& "$directory\ns-next-$binary.exe" run $project -- --timed)) {
            if ($line -match "^(\w+): ([\d.]+)$") { "$round,$binary,$($Matches[1]),$($Matches[2])" | Add-Content "$directory\timed.csv" }
        }
    }
}
```

Run it as a background command with a two-hour timeout (it takes about an hour and a half: six binaries, and the four slowest programs take seconds per run). `bench` prints a median above one second as seconds (`2.6379932s`), which `ToMilliseconds` handles; the script throws on any line it cannot parse rather than reusing a stale value.

- [ ] **Step 3: Pool and report.** For each source: each build's median of its three round medians, the pooled median of the nine values per tree, the range, and the change of the pooled median (after against before, in percent). For `--timed`, the same over the scores; the five programs whose score is one run's rate (CD, Havlak, Mandelbrot, NBody, DeltaBlue) are reported from `bench` only. Write the section "What the eight-byte value moved" in `execution.md`: date, the two commits, the machine (as the baseline names it), the protocol in one paragraph, a table per source (before, after, change, both ranges), the `vm::run` instruction count from Task 10 against the 7,143 last recorded, and what the measurement does not cover (one machine; data layout not varied; `bench` includes each run's instantiation). A source that got slower is named, with what the measurement shows about why or that it does not show it; the word stays (the spec, stage 7).
- [ ] **Step 4: Commit** (Markdown only: no gate)

```bash
git add docs/design/execution.md
git commit -m "docs: what the eight-byte value moved, measured on all sixteen programs and the microbenchmarks"
```

---

### Task 12: The documentation describes the built state

**Files:**
- Modify: `docs/design/object-model.md` ("Values and objects" describes the word, `ValueKind`, boxed floats and the 61-bit boundary; "Planned: an eight-byte value" is deleted, its still-true reasons moved into "Values and objects"; "Planned: the road" says step 2 is built)
- Modify: `docs/design/memory.md` (the store check, `Word`, `Tracer::word`; the decode amplification figures recomputed for 8-byte values and the boxed-float term from Task 7; "the transient peak" figures; `memory.md:1645`'s pointer)
- Modify: `docs/design/image-format.md` ("Planned: the image under the new representation" → the built rules: a `LargeInteger` that fits `i64` is written as an `i64`, an `i64` outside 61 bits decodes to a `LargeInteger`, a boxed float is `VALUE_FLOAT`)
- Modify: `docs/design/core-library.md` (`core-library.md:1045-1060` "a `Float` is an unboxed `Value::Float`"; "Large integers" names the 61-bit boundary; the index/size leaves' refusal of a `LargeInteger`)
- Modify: `docs/design/execution.md` ("Where the 3% goes" and "Planned: the compact instruction encoding" point to what the value now is)
- Modify: `docs/design/compiler.md` (its three `Value::` mentions; `Literal::SmallInteger`/`LargeInteger`'s 61-bit split)
- Modify: `docs/design/dispatch.md:891-894,1406` (sizes), `docs/design/README.md` (the map's status cells for values and builtins)
- Modify: `CLAUDE.md` (the image invariant's peak figures; "The brand" invariant's "`heap.rs`'s private `at_rest`/`branded`" → "`heap::value`'s `pub(super)` `at_rest`/`branded`"; the `crates/ns-runtime` paragraph mentions `heap/value.rs`)
- Modify: `docs/PROGRESS.md` (a milestone "The eight-byte value (2026-10-…)" listing every commit; "Where the next session starts" names road step 2's follow-on, the compact instruction encoding), `docs/OPEN-WORK.md` (step 2's second plan marked built; anything the reviews or Task 11 leave open added)

- [ ] **Step 1: Update each document** as listed, describing the current state only (`CLAUDE.md`, "Conventions"). Recompute every size-derived figure from the sizes Task 8 pinned and the derivation in `memory.md:964-1029`; state each new figure with its derivation.
- [ ] **Step 2: Check for stale statements**

Run: `rg -n "Value::(Nil|Boolean|SmallInteger|Float|Object)|16-byte .Value|fits .i64.|unboxed .Value::Float" docs/design CLAUDE.md docs/PROGRESS.md docs/OPEN-WORK.md`
Expected: only sentences that are still true (e.g. `read_large_integer`'s "fits `i64`"), each checked by hand.
- [ ] **Step 3: Commit** (Markdown only: no gate)

```bash
git add CLAUDE.md docs
git commit -m "docs: the documentation describes the eight-byte value as built"
```

---

### Task 13: The end-of-batch gates

- [ ] **Step 1:** Invoke the `finishing-a-batch` skill and run every gate it lists (the three stress modes, `run-corpus` plain and under stress, the benchmarks profile `cargo nextest run -p ns-cli --profile benchmarks`, `cargo nextest run -p ns-lsp`, the documentation check). Fix anything they find in its own commit, with the per-commit gate.
- [ ] **Step 2:** The final whole-branch review, on Fable, before the merge.
