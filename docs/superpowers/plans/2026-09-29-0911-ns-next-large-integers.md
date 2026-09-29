# Large Integers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ns-next's integers unbounded, as §5.1.1 requires: a `SmallInteger` result that overflows `i64` becomes an instance of a new kernel class `LargeInteger`, a result that fits again becomes a `SmallInteger`, every integer literal is representable, and `//` rounds toward negative infinity. This is the first of road step 2's two plans. The eight-byte value is the second, and it only moves the boundary from 64 bits to 61.

**Architecture:** A pure-Rust arbitrary-precision integer, `ArbitraryInteger` (sign and 64-bit digits, least significant first), lives in a new module `crates/ns-runtime/src/arbitrary_integer/`. Its digit arithmetic is adapted from `num-bigint` and `num-integer` (MIT): schoolbook, Karatsuba, Toom-3 and number-theoretic-transform multiplication, and Knuth's algorithm D and Burnikel-Ziegler division, each above `num-bigint`'s own threshold. It knows nothing about the heap. On today's arena a `LargeInteger` is an ordinary object whose payload is a new `ObjectData::LargeInteger(ArbitraryInteger)`, of a new builtin class `LargeInteger`, a subclass of `Integer`. Its methods are `Kernel.ns` forwarders to new VM-mirror leaves (`Primitive::LargeInteger`). `Integer`'s leaves promote on overflow and accept a `LargeInteger` argument, `Float`'s convert one to the nearest `double`, and `Value`'s derived equality and hash read one by state. One normalizing constructor, `Mutator::new_integer`, is the only way an `ArbitraryInteger` becomes a `Value`, so every integer has exactly one representation. An integer literal beyond `i64` becomes `Literal::LargeInteger`, which retires `Literal::MalformedInteger` and its wire tag 8. An image carries a `LargeInteger` as a value (`VALUE_LARGE_INTEGER`), never as an object record, because a value has no identity.

**Tech Stack:** Rust 1.98.0, edition 2021, `#![forbid(unsafe_code)]` in `ns-runtime` (no `unsafe`, so none of `num-bigint`'s intrinsics or inline assembly carry over). No new dependency. Tests under `cargo-nextest`; goldens under `goldens/`.

**Spec:** `docs/design/core-library.md`, "Planned: large integers" and "Deviation: `i64` integers"; `docs/design/object-model.md`, "Planned: an eight-byte value" ("Integers become unbounded", "Literals"); `docs/design/image-format.md`, "Planned: the image under the new representation" ("Integers"); `docs/design/compiler.md` on `Literal::MalformedInteger`. The road: `docs/OPEN-WORK.md`, "The road to a high-performance VM", step 2. §5.1.1 of `docs/references/newspeak-spec.md` ("Note that integers are not limited in size").

## Global Constraints

- Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - `vm.rs` holds no dispatch logic and raises no error.
  - Compiling a resolved module never raises a `NewspeakError` (a compiler-invariant violation is `panic!`).
  - An image is untrusted input: validate every count before it allocates, and no byte sequence may panic.
  - A wire tag is assigned once and never renumbered or reused.
- `ns-runtime` keeps `#![forbid(unsafe_code)]`. Nothing adapted from `num-bigint` may use `unsafe`, `core::arch` intrinsics or `asm!`; take the portable `u128` variants instead.
- No new external dependency, not even a dev-dependency. Correctness is checked against `i128`, against the algorithms' own cross-checks, and against values computed in Python and pasted into the tests.
- Adapted code carries this header at the top of each file that holds any, with the file path filled in:
  ```rust
  // Adapted from num-bigint (https://github.com/rust-num/num-bigint), commit
  // 41f0b035d12971c5c7f7501917e31f0f6439c26c, src/biguint/<file>.rs.
  // Copyright (c) 2014 The Rust Project Developers. MIT licence; see
  // THIRD-PARTY-NOTICES.md at the repository root. Changed for ns-next: names
  // written out, `BigUint` replaced by digit slices, no `unsafe`.
  ```
  For `num-integer` the same header names `https://github.com/rust-num/num-integer`, commit `765fd9b3eb9397471d41b9f613e530f6d9d6383f`, `src/lib.rs`.
- Every integer has exactly one representation: an `ArbitraryInteger` that fits `i64` is always a `Value::SmallInteger`, never a `LargeInteger` object. The only way to make a `Value` from an `ArbitraryInteger` is `Mutator::new_integer`.
- `3 class` stays `Integer`; `LargeInteger` is a subclass of `Integer` (core-library.md, "Planned: large integers").
- A `LargeInteger` is a value object (§3.1.1): its `=`, `==` and `hash` come from its state, never from an identity hash.
- New wire tags take the next free number in their family: builtin class tag **31**, value tag **9** (`VALUE_LARGE_INTEGER`), literal tag **10** (`LITERAL_LARGE_INTEGER`). Literal tag **8** (`LITERAL_MALFORMED_INTEGER`) is retired into `RETIRED_LITERALS`.
- Names are written out (`CLAUDE.md`, "Conventions"). Clippy's `min_ident_chars` refuses one- and two-character names, and `num-bigint`'s `a`, `b`, `q`, `r`, `x0`, `y1`, `p2`, `hi`, `lo` must be renamed when ported (`dividend`, `divisor`, `quotient`, `remainder`, `low_left`, `high_right`, …). No truncated words: `digits`, not `data`; `accumulator`, not `acc`.
- Comments explain the decision, not the mechanics.
- A golden that changes is a behaviour change. Establish the new output against the specification (and, for `//`, against Primordial Soup) before updating it; each such change is named in its task.
- Conventional commits. Every commit ends with the lines
  `Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_0124tQcaVnY7xCNbadqvatoZ`.
  A subagent's commit names the subagent's own model.
- Per-commit gate (`CLAUDE.md`, "Working in this repository"):
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace --exclude ns-lsp`
  - `cargo test --workspace --exclude ns-lsp --doc`
  - `cargo run -p ns-cli -- parse-corpus corpus/modules`, which must report "parsed 211, failed 0"

  The stress modes, `run-corpus` and the `ns-lsp` suite run once, in Task 10. The JetBrains plugin is untouched, so its gradle gates do not run.

## Review Focus

- **`i64::MIN` at every edge.** Expected results:
  - `-9223372036854775808 negated`, `abs` and `// -1` each answer `9223372036854775808`, a `LargeInteger`.
  - `9223372036854775808 negated` answers the `SmallInteger` `-9223372036854775808`.

  A digit-sign conversion that goes through `i64::abs` panics or wraps here. Pinned in Task 2 (unit tests), Task 4 (golden) and Task 5 (golden).
- **A result that fits again is a `SmallInteger`.** Expected: `(9223372036854775807 + 1 - 1) = 9223372036854775807` is `true`, and its `class printString` is `Integer`. A `LargeInteger` that equals a `SmallInteger`'s value would break `=` and `hash` silently. Pinned in Task 5 (golden) and Task 4 (`new_integer`).
- **A hostile image's `LargeInteger`.** Each of these must be a `DecodeError`, never a panic or an allocation beyond the input's size:
  - a digit count far beyond the bytes left;
  - a zero top digit;
  - a value that fits `i64`;
  - a negative zero.

  Pinned in Task 7.
- **Floor division's sign cases.** Expected: `7 // -2` = `-4`, `-7 // 2` = `-4`, `-7 // -2` = `3`, and the same four sign combinations across the `SmallInteger`/`LargeInteger` boundary. They must agree under both lowerings, the inlined `IntegerBinary` and the send. Pinned in Tasks 1 and 5.
- **A user subclass of `Integer` or `LargeInteger`, and `LargeInteger new`.** Expected: an ordinary instance that no integer leaf takes for a number. The leaves decline, the send falls to `Object`'s methods, and nothing panics or answers a number. Leaves must test the payload (`ObjectData::LargeInteger`), never the class. Pinned in Task 5.

---

## File Structure

| File | Responsibility |
|---|---|
| `prior-art/num-bigint`, `prior-art/num-integer`, `prior-art/num-rational` (submodules), `.gitmodules`, `prior-art/PROVENANCE.md` | the sources the arithmetic is adapted from, pinned, read and never built |
| `THIRD-PARTY-NOTICES.md` (new, repository root) | the MIT licence text and copyright line of `num-bigint` and `num-integer` |
| `crates/ns-runtime/src/arbitrary_integer/mod.rs` (new) | `ArbitraryInteger`: sign and magnitude, constructors, comparison, signed arithmetic, floor division (after `num-integer`) |
| `crates/ns-runtime/src/arbitrary_integer/addition.rs` (new) | magnitude addition, subtraction and comparison over digit slices (after `num-bigint`'s `addition.rs`, `subtraction.rs`) |
| `crates/ns-runtime/src/arbitrary_integer/multiplication.rs` (new) | schoolbook, half-Karatsuba, Karatsuba and Toom-3 (after `num-bigint`'s `multiplication.rs`) |
| `crates/ns-runtime/src/arbitrary_integer/division.rs` (new) | division by one digit and Knuth's algorithm D (after `num-bigint`'s `division.rs`) |
| `crates/ns-runtime/src/arbitrary_integer/conversion.rs` (new) | decimal text, radix parsing, nearest `f64` (after `num-bigint`'s `convert.rs`) |
| `crates/ns-runtime/src/lib.rs` | `mod arbitrary_integer;` and `pub use arbitrary_integer::ArbitraryInteger;` |
| `crates/ns-runtime/src/types.rs` | `BuiltinClass::LargeInteger`, `ObjectData::LargeInteger` |
| `crates/ns-runtime/core-library/Kernel.ns` | `class LargeInteger = Integer new ()` and its forwarders |
| `crates/ns-runtime/src/interpreter/kernel.rs`, `interpreter/mod.rs`, `heap.rs`, `trace.rs` | bootstrap, `Mutator::new_integer`, `Heap::large_integer`, `describe`, tracing |
| `crates/ns-runtime/src/builtins/primitive.rs`, `builtins/numbers.rs`, `builtins/mod.rs`, `builtins/value.rs` | the `LargeInteger` leaves, promotion in `Integer`'s, conversion in `Float`'s, derived equality and hash |
| `crates/ns-runtime/src/vm.rs` | `IntegerBinary`'s `//` calls the one floor-division helper; `make_value` materializes `Literal::LargeInteger` |
| `crates/ns-runtime/src/bytecode.rs`, `compiler/lower.rs`, `interpreter/literals.rs` | `Literal::LargeInteger`; `MalformedInteger` removed |
| `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`, `inspect.rs` | tags 31, 9 and 10; tag 8 retired |
| `crates/ns-runtime/tests/*.rs`, `goldens/**` | the tests named in each task |
| `CLAUDE.md`, `docs/design/*.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md` | moved from planned to built (Task 9) |

**The interface every task after Task 2 relies on** (exact signatures; `crates/ns-runtime/src/arbitrary_integer/mod.rs`):

```rust
/// An integer of any size: a sign and a magnitude of 64-bit digits, least
/// significant first. Canonical: no zero top digit, and zero is the empty
/// magnitude with `negative == false`, so derived equality is value equality.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArbitraryInteger {
    negative: bool,
    magnitude: Box<[u64]>,
}

impl ArbitraryInteger {
    pub fn zero() -> ArbitraryInteger;
    pub fn from_i64(value: i64) -> ArbitraryInteger;
    pub fn from_i128(value: i128) -> ArbitraryInteger;
    /// Trims zero top digits and clears the sign of a zero.
    pub fn from_parts(negative: bool, magnitude: Vec<u64>) -> ArbitraryInteger;
    pub fn is_negative(&self) -> bool;
    pub fn is_zero(&self) -> bool;
    pub fn magnitude(&self) -> &[u64];
    pub fn to_i64(&self) -> Option<i64>;
    pub fn add(&self, other: &ArbitraryInteger) -> ArbitraryInteger;
    pub fn subtract(&self, other: &ArbitraryInteger) -> ArbitraryInteger;
    pub fn multiply(&self, other: &ArbitraryInteger) -> ArbitraryInteger;
    /// Rounded toward negative infinity; `None` for a zero divisor.
    pub fn floor_divide(&self, divisor: &ArbitraryInteger) -> Option<ArbitraryInteger>;
    pub fn negated(&self) -> ArbitraryInteger;
    pub fn absolute(&self) -> ArbitraryInteger;
    /// The nearest `f64`, ties to even; beyond `f64::MAX` it is infinite.
    pub fn to_f64(&self) -> f64;
    pub fn to_decimal_string(&self) -> String;
    /// `text` is an optional `-` and digits of `radix` (2..=36, either case).
    pub fn parse(text: &str, radix: u32) -> Option<ArbitraryInteger>;
}

impl Ord for ArbitraryInteger { /* numeric order */ }
impl PartialOrd for ArbitraryInteger { /* Some(self.cmp(other)) */ }
```

Two heap-facing additions (Task 4) that Tasks 5 to 8 rely on:

```rust
// crates/ns-runtime/src/interpreter/mod.rs, impl<'gc> Mutator<'gc>
/// The one way an `ArbitraryInteger` becomes a `Value`: a `SmallInteger`
/// when it fits `i64`, otherwise a fresh `LargeInteger` object.
pub fn new_integer(&mut self, integer: ArbitraryInteger) -> Value<'gc>;

// crates/ns-runtime/src/heap.rs, impl<'gc> Heap<'gc>
/// The integer a `LargeInteger` object holds; `None` for any other object.
/// Copies out, as every accessor does (`docs/design/memory.md`, "The seam").
pub fn large_integer(&self, object: ObjectReference<'gc>) -> Option<ArbitraryInteger>;
```

And one helper that `builtins/numbers.rs` owns and `vm.rs` calls (Task 1):

```rust
// crates/ns-runtime/src/builtins/numbers.rs
/// `//` on two `i64`s, rounded toward negative infinity; `None` for a zero
/// divisor and for `i64::MIN // -1`, whose quotient does not fit.
#[inline]
pub(crate) fn checked_floor_divide(dividend: i64, divisor: i64) -> Option<i64>;
```

---

### Task 1: `//` rounds toward negative infinity

The specification does not define `//`. Searching `docs/references/newspeak-spec.md` for `//`, "floor" and "truncat" finds nothing. Primordial Soup does define it: `prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns:1958`, "Integer division, rounded toward negative infinity", and `KernelTests.ns:3887-3892` asserts it for negative divisors. The core-library design takes `num-integer`'s floor division for `//`. Today both the leaf (`builtins/numbers.rs:154`) and the inlined arm (`vm.rs:681`) use `checked_div_euclid`, which rounds a negative divisor the other way: `7 // -2` answers `-3` where it should answer `-4`. No golden divides by a negative number, so no golden changes. This task only adds one.

**Files:**
- Modify: `crates/ns-runtime/src/builtins/numbers.rs` (add `checked_floor_divide`, use it in `IntegerPrimitive::FloorDivide`)
- Modify: `crates/ns-runtime/src/vm.rs:676-683` (the `FloorDivide` arm calls the helper)
- Create: `goldens/arithmetic/floor_division_rounds_toward_negative_infinity.ns` and `.toml`

**Interfaces:**
- Produces: `pub(crate) fn checked_floor_divide(dividend: i64, divisor: i64) -> Option<i64>` in `builtins/numbers.rs`.

- [ ] **Step 1: Write the failing golden.** `goldens/arithmetic/floor_division_rounds_toward_negative_infinity.ns`:

```
Newspeak3
'Arithmetic'
(* `//` rounds toward negative infinity, as Primordial Soup's `Integer>>//`
   does (`KernelForPrimordialSoup.ns`, "Integer division, rounded toward
   negative infinity"); the specification does not define it. The first
   four have literal operands, which the inlined `IntegerBinary` computes
   itself; the last four go through a local, so the fast path and the send
   are both pinned under both lowerings. *)
class FloorDivision usingPlatform: p = Object new (
) (
    public main: p args: a = (
        | seven two |
        p transcript show: (7 // 2) printString; newLine.
        p transcript show: (7 // -2) printString; newLine.
        p transcript show: (-7 // 2) printString; newLine.
        p transcript show: (-7 // -2) printString; newLine.
        seven:: 7.
        two:: 2.
        p transcript show: (seven // two) printString; newLine.
        p transcript show: (seven // two negated) printString; newLine.
        p transcript show: (seven negated // two) printString; newLine.
        p transcript show: (seven negated // two negated) printString; newLine
    )
) : ()
```

`goldens/arithmetic/floor_division_rounds_toward_negative_infinity.toml` (values from Python's `//`, which also rounds toward negative infinity):

```toml
stdout = '''
3
-4
-4
3
3
-4
-4
3
'''
```

- [ ] **Step 2: Run it to verify it fails.**
Run: `cargo nextest run -p ns-cli --test run_goldens floor_division`
Expected: FAIL on lines 2, 4, 6 and 8. `div_euclid` answers `-3` for `7 // -2` and `4` for `-7 // -2`; the other two cases agree.

- [ ] **Step 3: Add the helper and use it.** In `builtins/numbers.rs`, after `float_hash`:

```rust
/// `//` on two `i64`s, rounded toward negative infinity (Primordial Soup's
/// `Integer>>//`; `num-integer`'s `div_floor`). `None` for a zero divisor
/// and for `i64::MIN // -1`, whose quotient does not fit -- the leaf
/// answers the one and raises the other, and `IntegerBinary` falls back to
/// the send for both, so this is the only copy of the rule.
#[inline]
pub(crate) fn checked_floor_divide(dividend: i64, divisor: i64) -> Option<i64> {
    let quotient = dividend.checked_div(divisor)?;
    let remainder = dividend % divisor;
    if remainder != 0 && ((remainder < 0) != (divisor < 0)) {
        Some(quotient - 1)
    } else {
        Some(quotient)
    }
}
```

`dividend % divisor` cannot overflow once `checked_div` has succeeded, because the one overflowing pair, `i64::MIN % -1`, is refused first. In `execute_integer`'s `FloorDivide` arm, replace `receiver_integer.checked_div_euclid(divisor)` with `checked_floor_divide(receiver_integer, divisor)`. In `vm.rs`, replace the arm and its comment:

```rust
                    // `checked_floor_divide` answers `None` for a zero divisor
                    // as well as for `i64::MIN // -1`, so both take the
                    // fallback and the primitive decides what they answer. It
                    // is `builtins::numbers`' own function, so the rounding
                    // rule has one copy.
                    IntegerOperation::FloorDivide => {
                        crate::builtins::numbers::checked_floor_divide(left_integer, right_integer)
                            .map(Value::SmallInteger)
                    }
```

If `builtins::numbers` is not reachable from `vm.rs`, widen it to `pub(crate)` (`CLAUDE.md`: "If you need something private there, widen it to `pub(crate)`").

- [ ] **Step 4: Add a unit test** at the end of `builtins/numbers.rs`, in a new `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::checked_floor_divide;

    #[test]
    fn floor_division_rounds_toward_negative_infinity() {
        assert_eq!(checked_floor_divide(7, 2), Some(3));
        assert_eq!(checked_floor_divide(7, -2), Some(-4));
        assert_eq!(checked_floor_divide(-7, 2), Some(-4));
        assert_eq!(checked_floor_divide(-7, -2), Some(3));
        assert_eq!(checked_floor_divide(6, -2), Some(-3));
        assert_eq!(checked_floor_divide(i64::MIN, 1), Some(i64::MIN));
        assert_eq!(checked_floor_divide(i64::MIN, -1), None);
        assert_eq!(checked_floor_divide(1, 0), None);
    }
}
```

- [ ] **Step 5: Run the golden, the unit test and the per-commit gate.** All pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/ns-runtime/src/builtins/numbers.rs crates/ns-runtime/src/vm.rs goldens/arithmetic/floor_division_rounds_toward_negative_infinity.*
git commit -m "fix: round // toward negative infinity, as Primordial Soup does"
```

---

### Task 2: `ArbitraryInteger`, the digit arithmetic

A pure-Rust module with no heap, no `Value` and no `Mutator`. It holds everything `LargeInteger` computes. The prior-art submodules and the notices file come with it, because its headers cite them.

**Files:**
- Create: submodules `prior-art/num-bigint`, `prior-art/num-integer`, `prior-art/num-rational`; modify `.gitmodules`, `prior-art/PROVENANCE.md`
- Create: `THIRD-PARTY-NOTICES.md`
- Create: `crates/ns-runtime/src/arbitrary_integer/{mod,addition,multiplication,division,conversion}.rs`
- Modify: `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Produces: `ArbitraryInteger` exactly as in "The interface every task after Task 2 relies on" above, exported as `ns_runtime::ArbitraryInteger`.

- [ ] **Step 1: Add the three submodules at the pinned commits.**

```bash
git submodule add https://github.com/rust-num/num-bigint.git prior-art/num-bigint
git -C prior-art/num-bigint checkout 41f0b035d12971c5c7f7501917e31f0f6439c26c
git submodule add https://github.com/rust-num/num-integer.git prior-art/num-integer
git -C prior-art/num-integer checkout 765fd9b3eb9397471d41b9f613e530f6d9d6383f
git submodule add https://github.com/rust-num/num-rational.git prior-art/num-rational
git -C prior-art/num-rational checkout cf95d6719c585d4721645c7ace5dfe1dd5f063e2
```

In `prior-art/PROVENANCE.md`:
- Add three rows to "What each one is for":
  - `num-bigint/`: the arithmetic `crates/ns-runtime/src/arbitrary_integer/` adapts (MIT OR Apache-2.0; taken under MIT, notices in `THIRD-PARTY-NOTICES.md`).
  - `num-integer/`: the floor division it adapts.
  - `num-rational/`: the model for a future `Fraction` and for exact float-to-rational conversion (`docs/design/core-library.md`, "Gap: an integer has no `/`"). Nothing is taken from it yet.
- Add three rows to "Pinned versions": `41f0b035…` 2026-08-21, `765fd9b3…` 2026-08-11 and `cf95d671…` 2026-07-07, each added 2026-09-29.
- Change "seven reference implementations" wherever this file and `CLAUDE.md` count them. `CLAUDE.md` is updated in Task 9.

- [ ] **Step 2: Write `THIRD-PARTY-NOTICES.md`.** It gets a short introduction naming the directory that carries adapted code (`crates/ns-runtime/src/arbitrary_integer/`). Then one section each for `num-bigint` and `num-integer`, holding the upstream URL, the pinned commit, and the complete text of that project's `LICENSE-MIT`, copied verbatim from the submodule with its `Copyright (c) 2014 The Rust Project Developers` line.

- [ ] **Step 3: Write the failing tests first**, in `arbitrary_integer/mod.rs`'s `#[cfg(test)] mod tests`. Operations are checked against `i128`, which holds every sum, difference and product of two `i64`s, over a deterministic spread of operands. There is no `rand` dependency; a fixed list plus a linear congruential sequence serves.

```rust
#[cfg(test)]
mod tests {
    use super::ArbitraryInteger;

    /// Operands at every boundary the representation has: zero, one digit,
    /// the `i64` edges and the digit edge, plus a deterministic spread.
    fn operands() -> Vec<i64> {
        let mut values = vec![
            0, 1, -1, 2, -2, 7, -7, 255, -256,
            i64::MAX, i64::MIN, i64::MAX - 1, i64::MIN + 1,
            u32::MAX as i64, -(u32::MAX as i64), 1 << 32, -(1 << 32),
        ];
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..200 {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            values.push(state as i64 >> (state % 63));
        }
        values
    }

    fn wide(value: i128) -> ArbitraryInteger {
        ArbitraryInteger::from_i128(value)
    }

    #[test]
    fn arithmetic_agrees_with_i128() {
        for &left in &operands() {
            for &right in &operands() {
                let (left_wide, right_wide) = (i128::from(left), i128::from(right));
                let (left_integer, right_integer) = (wide(left_wide), wide(right_wide));
                assert_eq!(left_integer.add(&right_integer), wide(left_wide + right_wide));
                assert_eq!(left_integer.subtract(&right_integer), wide(left_wide - right_wide));
                assert_eq!(left_integer.multiply(&right_integer), wide(left_wide * right_wide));
                assert_eq!(left_integer.cmp(&right_integer), left_wide.cmp(&right_wide));
                if right != 0 {
                    assert_eq!(
                        left_integer.floor_divide(&right_integer),
                        Some(wide(left_wide.div_euclid(right_wide)
                            - i128::from(right_wide < 0 && left_wide.rem_euclid(right_wide) != 0))),
                    );
                }
            }
        }
    }

    #[test]
    fn zero_has_one_representation() {
        assert_eq!(ArbitraryInteger::from_parts(true, vec![0, 0]), ArbitraryInteger::zero());
        assert!(!ArbitraryInteger::from_i64(5).subtract(&ArbitraryInteger::from_i64(5)).is_negative());
        assert_eq!(ArbitraryInteger::zero().negated(), ArbitraryInteger::zero());
    }

    #[test]
    fn the_i64_edges_convert_both_ways() {
        let minimum = ArbitraryInteger::from_i64(i64::MIN);
        assert_eq!(minimum.to_i64(), Some(i64::MIN));
        assert_eq!(minimum.negated().to_i64(), None);
        assert_eq!(minimum.negated(), wide(1_i128 << 63));
        assert_eq!(minimum.absolute(), wide(1_i128 << 63));
        assert_eq!(wide(1_i128 << 63).negated().to_i64(), Some(i64::MIN));
        assert_eq!(wide(i128::from(i64::MAX) + 1).to_i64(), None);
        assert_eq!(minimum.floor_divide(&ArbitraryInteger::from_i64(-1)), Some(wide(1_i128 << 63)));
        assert_eq!(minimum.floor_divide(&ArbitraryInteger::zero()), None);
    }

    #[test]
    fn decimal_text_matches_python() {
        // Values computed with Python 3.14; `str(2**200)` and so on.
        let two_to_the_200 = ArbitraryInteger::parse(
            "1606938044258990275541962092341162602522202993782792835301376", 10).unwrap();
        let mut power = ArbitraryInteger::from_i64(1);
        for _ in 0..200 {
            power = power.multiply(&ArbitraryInteger::from_i64(2));
        }
        assert_eq!(power, two_to_the_200);
        assert_eq!(power.to_decimal_string(),
            "1606938044258990275541962092341162602522202993782792835301376");
        assert_eq!(power.negated().to_decimal_string(),
            "-1606938044258990275541962092341162602522202993782792835301376");
        assert_eq!(ArbitraryInteger::zero().to_decimal_string(), "0");
        assert_eq!(ArbitraryInteger::parse("FFFFFFFFFFFFFFFFFFFF", 16).unwrap().to_decimal_string(),
            "1208925819614629174706175");
        assert_eq!(ArbitraryInteger::parse("-99999999999999999999", 10).unwrap().to_decimal_string(),
            "-99999999999999999999");
        assert_eq!(ArbitraryInteger::parse("1", 2), Some(ArbitraryInteger::from_i64(1)));
        assert_eq!(ArbitraryInteger::parse("", 10), None);
        assert_eq!(ArbitraryInteger::parse("12", 2), None);
        assert_eq!(ArbitraryInteger::parse("10", 37), None);
    }

    #[test]
    fn nearest_double() {
        assert_eq!(wide(1_i128 << 64).to_f64(), 18_446_744_073_709_551_616.0);
        // 2^64 + 1 rounds to 2^64; 2^53 + 1 rounds to even, 2^53.
        assert_eq!(wide((1_i128 << 64) + 1).to_f64(), 18_446_744_073_709_551_616.0);
        assert_eq!(wide((1_i128 << 53) + 1).to_f64(), 9_007_199_254_740_992.0);
        assert_eq!(wide(-(1_i128 << 70)).to_f64(), -1_180_591_620_717_411_303_424.0);
        let mut huge = ArbitraryInteger::from_i64(1);
        for _ in 0..1100 {
            huge = huge.multiply(&ArbitraryInteger::from_i64(2));
        }
        assert_eq!(huge.to_f64(), f64::INFINITY);
        assert_eq!(huge.negated().to_f64(), f64::NEG_INFINITY);
    }
}
```

Replace `.unwrap()` with `.expect("…")` naming the value if clippy's configuration asks for it. `unwrap` on an `Option` in a test is permitted here, since the repository's ban is on `Value`/`EvaluationResult`.

In `multiplication.rs`'s own `#[cfg(test)] mod tests`, cross-check the four multiplication algorithms against schoolbook on operands large enough to reach each branch (33, 70, 300 and 700 digits), with the digits taken from the same linear congruential sequence:

```rust
#[cfg(test)]
mod tests {
    use super::{multiply_magnitudes, multiply_schoolbook};

    fn digits(count: usize, seed: u64) -> Vec<u64> {
        let mut state = seed;
        (0..count)
            .map(|_| {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                state
            })
            .collect()
    }

    #[test]
    fn every_algorithm_agrees_with_schoolbook() {
        // (shorter, longer): long multiplication, half-Karatsuba (the longer
        // at least twice the shorter), Karatsuba (at most 256), Toom-3 (above).
        for (shorter, longer) in [(3, 5), (33, 40), (40, 90), (100, 120), (300, 310), (700, 700)] {
            let left = digits(shorter, 1);
            let right = digits(longer, 2);
            assert_eq!(multiply_magnitudes(&left, &right), multiply_schoolbook(&left, &right),
                "{shorter} x {longer} digits");
            assert_eq!(multiply_magnitudes(&right, &left), multiply_schoolbook(&left, &right));
        }
    }

    #[test]
    fn all_ones_digits_carry_through_every_algorithm() {
        for count in [1, 33, 257, 600] {
            let ones = vec![u64::MAX; count];
            assert_eq!(multiply_magnitudes(&ones, &ones), multiply_schoolbook(&ones, &ones));
        }
    }
}
```

In `division.rs`'s tests, check algorithm D by its defining identity: for dividend and divisor magnitudes of many shapes (one digit, two digits, a divisor whose top digit already has its high bit set, a divisor of all `u64::MAX` digits, a dividend smaller than the divisor, equal ones), `quotient * divisor + remainder == dividend` and `remainder < divisor`, computed through `multiply_schoolbook` and `addition`'s helpers:

```rust
#[cfg(test)]
mod tests {
    use super::divide_remainder;
    use crate::arbitrary_integer::addition::{add_magnitudes, compare_magnitudes};
    use crate::arbitrary_integer::multiplication::multiply_schoolbook;
    use std::cmp::Ordering;

    fn trimmed(mut digits: Vec<u64>) -> Vec<u64> {
        while digits.last() == Some(&0) {
            digits.pop();
        }
        digits
    }

    fn check(dividend: &[u64], divisor: &[u64]) {
        let (quotient, remainder) = divide_remainder(dividend, divisor);
        let product = multiply_schoolbook(&quotient, divisor);
        assert_eq!(trimmed(add_magnitudes(&product, &remainder)), trimmed(dividend.to_vec()));
        assert_eq!(compare_magnitudes(&remainder, divisor), Ordering::Less);
    }

    #[test]
    fn quotient_times_divisor_plus_remainder_is_the_dividend() {
        let shapes: [(&[u64], &[u64]); 7] = [
            (&[10, 20, 30], &[7]),
            (&[u64::MAX, u64::MAX, u64::MAX, 5], &[3, 1]),
            (&[1, 2, 3, 4, 5, 6], &[0, 0, 1 << 63]),
            (&[u64::MAX; 9], &[u64::MAX; 4]),
            (&[5], &[7, 1]),
            (&[4, 9, 1], &[4, 9, 1]),
            (&[0, 0, 0, 0, 1], &[1, 0, 1]),
        ];
        for (dividend, divisor) in shapes {
            check(dividend, divisor);
        }
    }
}
```

If the helper names differ once written, keep the tests and rename to match. What must hold is the identity and the bound.

- [ ] **Step 4: Run them to verify they fail.**
Run: `cargo nextest run -p ns-runtime arbitrary_integer`
Expected: FAIL to compile. The module does not exist yet.

- [ ] **Step 5: Write `addition.rs`** from `prior-art/num-bigint/src/biguint/addition.rs:43-93` (`adc`'s portable form, `__add2`, `add2`) and `subtraction.rs:43-112` (`sbb`'s portable form, `sub2`, `__sub2rev`, `sub2rev`). Add `compare_magnitudes(left: &[u64], right: &[u64]) -> Ordering` (num-bigint's `cmp_slice`: compare lengths of the trimmed slices, then digits from the top) and an owning `add_magnitudes(left: &[u64], right: &[u64]) -> Vec<u64>`. Take only the non-intrinsic `adc`/`sbb` bodies: the `#[cfg(target_arch = "x86_64")]` ones call `core::arch`, which is `unsafe`. The adapted-code header goes at the top.

- [ ] **Step 6: Write `multiplication.rs`** from `prior-art/num-bigint/src/biguint/multiplication.rs`:
  - `mac_with_carry`, `mul_with_carry` and `mac_digit` (`:17-58`);
  - `mac3` (`:70-422`) with its four branches, as `multiply_accumulate(accumulator: &mut [u64], left: &[u64], right: &[u64])`:
    - long multiplication at `shorter.len() <= 32`;
    - half-Karatsuba when `shorter.len() * 2 <= longer.len()`;
    - Karatsuba at `<= 256`;
    - Toom-3 at `<= 512`;
    - above that, the number-theoretic transform, which Task 3 adds.
  - `sub_sign` (`:455-479`).

  In this task the branch that calls `ntt::mac3` (`:413-421`) falls back to Toom-3, so every size is multiplied correctly before Task 3 exists. Task 3 replaces that fallback with the call.

  Toom-3's intermediates are signed (`BigInt` upstream, `:300-411`). Write them with `ArbitraryInteger` and its `add`, `subtract` and `multiply`, plus three `pub(super)` helpers on `ArbitraryInteger` that exist only for Toom-3:
  - `doubled(&self)`, for `* 2` and `<< 1`;
  - `halved_exactly(&self)`, for `>> 1` on a value known to be even;
  - `divided_exactly_by_three(&self)`, for `/ 3u32` on a value known to be a multiple, through `division.rs`'s one-digit division.

  The final accumulation (`:400-411`) adds or subtracts each coefficient's magnitude into the accumulator at its offset, by its sign. Expose:
  - `pub(super) fn multiply_magnitudes(left: &[u64], right: &[u64]) -> Vec<u64>` (num-bigint's `mul3`, `:424-433`, answering a trimmed `Vec`);
  - `pub(super) fn multiply_schoolbook(left: &[u64], right: &[u64]) -> Vec<u64>`, the long-multiplication loop alone, which the tests and `division.rs`'s tests use as the oracle.

  The adapted-code header goes at the top.

- [ ] **Step 7: Write `division.rs`** from `prior-art/num-bigint/src/biguint/division.rs`:
  - `div_wide`, the portable `u128` form only (`:23-31`). The `asm!` form (`:33-75`) is not taken.
  - `div_half` (`:77-86`), `div_rem_digit` (`:87-112`) and `sub_mul_digit_same_len` (`:138-163`).
  - `div_rem_cow`'s dispatch (`:172-232`). Its Burnikel-Ziegler branch (`:196-202`) is Task 3's; in this task every size goes to algorithm D.
  - `div_rem_core` (`:372-472`), Knuth's algorithm D.

  Expose:
  - `pub(super) fn divide_remainder(dividend: &[u64], divisor: &[u64]) -> (Vec<u64>, Vec<u64>)`, where the divisor is non-empty and trimmed, and both results are trimmed;
  - `pub(super) fn divide_by_digit(dividend: &[u64], divisor: u64) -> (Vec<u64>, u64)`.

  The normalizing shift (`biguint_shl`, `>> shift`) becomes two small digit-slice shifts in this file. The adapted-code header goes at the top.

- [ ] **Step 8: Write `conversion.rs`** from `prior-art/num-bigint/src/biguint/convert.rs`:
  - `to_decimal`: the repeated division by the largest power of ten a digit holds, `10^19`, after `to_radix_digits_le_small` (`:745-767`) and `get_radix_base` (`:839-847`). Only the small path is taken, not the divide-and-conquer one (`:768-795`); the design names this one.
  - `parse_radix`: `from_radix_digits_be` (`:91-143`), which multiplies in chunks of the radix's largest power that fits a digit. Power-of-two radixes may go through it too; the bitwise fast path (`:33-89`) is not needed.
  - `to_f64`: `high_bits_to_u64` (`:261-310`, round-to-odd) and `to_f64` (`:371-380`), with the sign applied last.

  The adapted-code header goes at the top.

- [ ] **Step 9: Write `mod.rs`.** The `ArbitraryInteger` struct and every method of the interface go here, plus `Ord`/`PartialOrd`: sign first, then `compare_magnitudes`, reversed for two negatives. The signed operations dispatch on the signs:
  - equal signs add magnitudes;
  - different signs subtract the smaller magnitude from the larger and take the larger's sign.

  `floor_divide` divides magnitudes with `divide_remainder`. When the signs differ it negates the quotient, and when they differ and the remainder is non-zero it also adds one to the quotient's magnitude first: that is `num-integer`'s `div_floor` (`prior-art/num-integer/src/lib.rs`, the `impl Integer for` signed types' `div_floor`), which carries the `num-integer` header. `from_i64` and `from_i128` take the magnitude through `unsigned_abs`, never `abs`. `to_i64` accepts `2^63` only when negative. `parse` strips one leading `-`, refuses an empty digit string and any character that is not a digit of the radix, and refuses a radix outside `2..=36`.

- [ ] **Step 10: Register the module** in `crates/ns-runtime/src/lib.rs`: `mod arbitrary_integer;` and `pub use arbitrary_integer::ArbitraryInteger;` next to the other re-exports.

- [ ] **Step 11: Run the tests.**
Run: `cargo nextest run -p ns-runtime arbitrary_integer`
Expected: PASS. Then run the per-commit gate.

- [ ] **Step 12: Commit.**

```bash
git add .gitmodules prior-art/num-bigint prior-art/num-integer prior-art/num-rational prior-art/PROVENANCE.md THIRD-PARTY-NOTICES.md crates/ns-runtime/src/arbitrary_integer crates/ns-runtime/src/lib.rs
git commit -m "feat: add ArbitraryInteger, adapted from num-bigint and num-integer"
```

---

### Task 3: number-theoretic-transform multiplication and Burnikel-Ziegler division

Decided with the user on 2026-09-29: the arithmetic also takes `num-bigint`'s two algorithms for the largest operands. Multiplication above 512 digits uses the number-theoretic transform. Division of a dividend above 128 digits by a divisor above 64 digits uses Burnikel and Ziegler's recursive division. Below those thresholds nothing changes, so Task 2's tests keep passing and gain companions at the new sizes.

**Files:**
- Create: `crates/ns-runtime/src/arbitrary_integer/number_theoretic_transform.rs`
- Modify:
  - `crates/ns-runtime/src/arbitrary_integer/multiplication.rs` (the branch above 512 digits);
  - `arbitrary_integer/division.rs` (the Burnikel-Ziegler branch and function);
  - `arbitrary_integer/mod.rs` (`mod number_theoretic_transform;`).

**Interfaces:**
- Consumes: Task 2's `multiply_accumulate`, `multiply_schoolbook`, `divide_remainder` and the addition helpers.
- Produces:
  - `pub(super) fn multiply_accumulate_by_transform(accumulator: &mut [u64], left: &[u64], right: &[u64])`, the port of `ntt::mac3`;
  - `pub(super) fn multiply_accumulate_two_primes(accumulator: &mut [u64], left: &[u64], right: &[u64], bits: u64)` and `pub(super) fn multiply_accumulate_three_primes(accumulator: &mut [u64], left: &[u64], right: &[u64])`, the two paths, which the tests call directly;
  - `pub(super) const fn bits_per_digit_for(length: u64) -> u64`;
  - `divide_remainder_recursively` (Burnikel-Ziegler) and `divide_remainder_knuth` (algorithm D, Task 2's body renamed). `divide_remainder` keeps its signature and dispatches between them.

  No public signature changes.

- [ ] **Step 1: Write the failing tests.** In `number_theoretic_transform.rs`'s `#[cfg(test)] mod tests`, check both prime paths against `multiply_schoolbook`.
  - The dispatch chooses three primes only for operands of astronomical length (`compute_bits` drops below 43 bits only near 2^42 digits), so the tests call each path directly.
  - Lengths: 1, 2, 513, 700 and 1500, plus unequal ones (600 by 1400).
  - Digits: from the linear congruential sequence, and all `u64::MAX`, which drives every carry.
  - The accumulator already holds a value, since `mac3` *adds* the product into it.

```rust
#[cfg(test)]
mod tests {
    use super::{
        bits_per_digit_for, multiply_accumulate_by_transform, multiply_accumulate_three_primes,
        multiply_accumulate_two_primes,
    };
    use crate::arbitrary_integer::addition::add_magnitudes;
    use crate::arbitrary_integer::multiplication::multiply_schoolbook;

    fn digits(count: usize, seed: u64) -> Vec<u64> {
        let mut state = seed;
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                state
            })
            .collect()
    }

    fn trimmed(mut digits: Vec<u64>) -> Vec<u64> {
        while digits.last() == Some(&0) {
            digits.pop();
        }
        digits
    }

    /// The two-prime path at the bit width the dispatch would choose.
    fn two_primes_at_best_width(accumulator: &mut [u64], left: &[u64], right: &[u64]) {
        let bits = bits_per_digit_for(left.len().max(right.len()) as u64);
        multiply_accumulate_two_primes(accumulator, left, right, bits);
    }

    /// Checks `accumulator + left * right`, the contract every `mac3` has.
    fn check(multiply: fn(&mut [u64], &[u64], &[u64]), left: &[u64], right: &[u64], seed: u64) {
        let start = digits(left.len() / 2, seed);
        let mut accumulator = start.clone();
        accumulator.resize(left.len() + right.len() + 1, 0);
        multiply(&mut accumulator, left, right);
        let expected = trimmed(add_magnitudes(&start, &multiply_schoolbook(left, right)));
        assert_eq!(trimmed(accumulator), expected, "{} x {} digits", left.len(), right.len());
    }

    #[test]
    fn both_prime_paths_agree_with_schoolbook() {
        for (left_count, right_count) in
            [(1, 1), (2, 3), (513, 513), (700, 700), (600, 1400), (1500, 1500)]
        {
            let left = digits(left_count, 3);
            let right = digits(right_count, 4);
            check(two_primes_at_best_width, &left, &right, 5);
            check(multiply_accumulate_three_primes, &left, &right, 6);
            check(multiply_accumulate_by_transform, &left, &right, 7);
        }
    }

    #[test]
    fn all_ones_digits_carry_through_both_paths() {
        for count in [1, 513, 1200] {
            let ones = vec![u64::MAX; count];
            check(two_primes_at_best_width, &ones, &ones, 8);
            check(multiply_accumulate_three_primes, &ones, &ones, 9);
        }
    }
}
```

In `multiplication.rs`'s tests, extend two lists so they reach the transform through the dispatch:
- add `(600, 600)` and `(900, 2000)` to `every_algorithm_agrees_with_schoolbook`;
- add `1100` to `all_ones_digits_carry_through_every_algorithm`.

In `division.rs`'s tests, add Burnikel-Ziegler shapes to the identity check:
- a 200-digit dividend by a 100-digit divisor;
- 1000 by 300;
- 3000 by 65, just past the threshold;
- 129 by 65;
- 200 by 90 with a divisor of all `u64::MAX` digits;
- an exact multiple, built as `multiply_schoolbook(divisor, quotient)`, whose remainder must be empty.

Then add one test that the two algorithms agree. It gets its own copy of the `digits` helper:

```rust
    #[test]
    fn burnikel_ziegler_agrees_with_algorithm_d() {
        for (dividend_count, divisor_count) in [(200, 100), (1000, 300), (3000, 65), (129, 65), (700, 699)] {
            let dividend = digits(dividend_count, 10);
            let divisor = digits(divisor_count, 11);
            assert_eq!(
                super::divide_remainder_recursively(&dividend, &divisor),
                super::divide_remainder_knuth(&dividend, &divisor),
                "{dividend_count} / {divisor_count} digits",
            );
        }
    }
```

- [ ] **Step 2: Run them to verify they fail.**
Run: `cargo nextest run -p ns-runtime arbitrary_integer`
Expected: FAIL to compile. The functions do not exist yet.

- [ ] **Step 3: Port the transform** into `number_theoretic_transform.rs` from `prior-art/num-bigint/src/biguint/ntt.rs`. Upstream is already `#![forbid(unsafe_code)]`. Take its whole 64-bit path:
  - the `arith` module (`:7-63`) and `Arith<const P: u64>` (`:64-213`), Montgomery arithmetic modulo a prime;
  - `NttPlan` (`:214-385`);
  - `conv_base` (`:386-416`);
  - the radix-2 to radix-6 kernels and single blocks (`:417-671`);
  - `ntt_dif_dit` (`:672-732`), `calc_twiddle_factors` (`:733-756`) and `conv` (`:757-840`);
  - the three primes and their derived constants (`:841-858`);
  - `propagate_carry` (`:859-869`), `mac3_two_primes` (`:870-973`), `mac3_three_primes` (`:974-1072`) and `mac3_u64` with `compute_bits` (`:1073-1116`).

  Do not take the `cfg_digit!` wrapper (`:1118-1151`). Its 32-bit arm converts `u32` digits, and its 64-bit arm is just `mac3_u64`, which becomes `multiply_accumulate_by_transform`.

  Adjustments:
  - Lift `compute_bits` out of `mac3_u64` as `bits_per_digit_for`.
  - Rename `mac3_two_primes` and `mac3_three_primes` as in "Produces".
  - Write every name out:
    - `P1`, `P2`, `P3` become `FIRST_PRIME`, `SECOND_PRIME`, `THIRD_PRIME`, and the derived constants follow (`FIRST_PRIME_INVERSE_MOD_SECOND`, …).
    - The const generic `P` stays: it is a type parameter, which keeps Rust's convention (`CLAUDE.md`, "Names are written out").
    - `x`, `y`, `px`, `w1`, `c`, `l`, `lo`, `hi`, `mid` and every other short local becomes the word it stands for; the upstream comments say which.
    - Clippy's `min_ident_chars` refuses any short name that slips through.
  - The adapted-code header names `src/biguint/ntt.rs`.
  - Keep upstream's comments that explain a choice (the prime selection, the bit-width bound), rewritten in this repository's style.

- [ ] **Step 4: Dispatch to it** in `multiplication.rs`. Task 2 left the branch above 512 digits as a Toom-3 fallback; it becomes `number_theoretic_transform::multiply_accumulate_by_transform(accumulator, left, right)`, as upstream's `:413-421` does.

- [ ] **Step 5: Port Burnikel-Ziegler** into `division.rs` from `prior-art/num-bigint/src/biguint/division.rs:223-370`. That range holds `BURNIKEL_ZIEGLER_THRESHOLD` (64) and `div_rem_burnikel_ziegler` with its nested helpers:
  - `divide_biguint`, `normalizing_shift_amount` and `concat_biguint`;
  - `div_two_digit_by_one` and `div_three_halves_by_two`;
  - `div_blocks`.

  Their `BigUint` values become owned `Vec<u64>` magnitudes, worked on with Task 2's addition, subtraction, comparison, shift and multiplication helpers. The subtractions upstream writes on `BigUint` never go negative only because the algorithm's own correction loop keeps them positive, so port that loop exactly.

  Name the entry `divide_remainder_recursively(dividend: &[u64], divisor: &[u64]) -> (Vec<u64>, Vec<u64>)`, and rename Task 2's algorithm-D body to `divide_remainder_knuth`. `divide_remainder` becomes the dispatch of upstream's `:172-221`, in this order:
  - the one-digit case;
  - the ordering shortcuts;
  - Burnikel-Ziegler when `dividend.len() > 128 && divisor.len() > 64`;
  - algorithm D otherwise.

  Write names out as in Step 3: `first_block`, `quotient_block`, `level`, never `a1`, `b2`, `q`, `r`. The adapted-code header already on the file covers the new code.

- [ ] **Step 6: Run the tests.**
Run: `cargo nextest run -p ns-runtime arbitrary_integer`
Expected: PASS. Then run the per-commit gate.

- [ ] **Step 7: Commit.**

```bash
git add crates/ns-runtime/src/arbitrary_integer
git commit -m "feat: number-theoretic-transform multiplication and Burnikel-Ziegler division"
```

---

### Task 4: the `LargeInteger` class and object, and promotion

After this task, an `Integer` operation that overflows answers a `LargeInteger`, which prints itself and compares and hashes as a value. Arithmetic *on* a `LargeInteger` is Task 5.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs`:
  - `BuiltinClass::LargeInteger`, appended after `Future`, and in `ALL`, whose length becomes 26;
  - `ObjectData::LargeInteger(ArbitraryInteger)`.
- Modify: `crates/ns-runtime/core-library/Kernel.ns`: `public class LargeInteger = Integer new () ( … )` after `Integer`.
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs`:
  - `kernel_class_name`;
  - the tests `the_hierarchy_follows_the_specification` (`(LargeInteger, Some(Integer))`), `every_value_classs_application_inherits_value`, `QUICK_METHODS`/`NOT_FORWARDERS` as they apply, and the absent-accessor list in `objects_class_accessors_are_exactly_the_nameable_builtins`.
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Mutator::new_integer`)
- Modify: `crates/ns-runtime/src/heap.rs`:
  - `Heap::large_integer`;
  - `describe`, which gives the decimal text as for a `SmallInteger`;
  - the future-replacement pass (`:3066`), where `LargeInteger` holds no handle.
- Modify: `crates/ns-runtime/src/trace.rs:66` (`ObjectData::LargeInteger(_)` traces nothing)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`builtin_class_tag`: `LargeInteger => 31`), `image/decode.rs` (`builtin_class_from_tag`: `31 => LargeInteger`), `image/opcodes.rs:274` ("Next free number: 32"), `tests/opcode_tags.rs::builtin_class_tags_are_fixed`
- Modify: `crates/ns-runtime/src/builtins/primitive.rs`:
  - a new family `Primitive::LargeInteger(LargeIntegerPrimitive)`;
  - its `MIRROR` rows;
  - `answers_false_to_a_stranger`.
- Modify: `crates/ns-runtime/src/builtins/numbers.rs` (promotion; `execute_large_integer` for `Equal`, `Hash`, `PrintString`), `builtins/mod.rs` (dispatch the new family)
- Modify: `crates/ns-runtime/src/builtins/value.rs` (`compare_node` and `hash_node` read a `LargeInteger` slot)
- Test: `crates/ns-runtime/tests/builtins_boolean_numbers.rs`; `goldens/errors/inlined_integer_addition_overflow_is_uncaught.*` (removed) → `goldens/arithmetic/an_overflowing_integer_operation_answers_a_large_integer.*` (new); `goldens/kernel/a_value_with_a_large_integer_slot_compares_by_state.*` (new)

**Interfaces:**
- Consumes: `ArbitraryInteger` (Task 2); `checked_floor_divide` (Task 1).
- Produces:
  - `Mutator::new_integer`, `Heap::large_integer` (signatures above);
  - `pub(crate) fn large_integer_hash(integer: &ArbitraryInteger) -> i64` in `builtins/numbers.rs`, which is `crate::hash::content_hash((integer.is_negative(), integer.magnitude()))`, so it stays below 2^32 as `value.rs`'s sum requires;
  - `LargeIntegerPrimitive` with the variants `Add, Subtract, Multiply, FloorDivide, Less, Greater, LessOrEqual, GreaterOrEqual, Equal, Hash, Negated, Absolute, PrintString`. Task 5 fills in the arithmetic.

- [ ] **Step 1: Write the failing tests.** In `tests/builtins_boolean_numbers.rs`, **replace** the two tests that pin an overflow error:
  - `integer_addition_overflow_is_a_newspeak_error_not_a_panic` (`:65`) becomes `integer_addition_overflow_answers_a_large_integer`, which sends `+ 1` to `i64::MAX` and asserts that the answer's `printString` is `9223372036854775808`.
  - `a_real_integer_overflow_is_still_reported_as_an_overflow` (`:381`) becomes `integer_multiplication_overflow_answers_a_large_integer`, which asserts that `i64::MAX * 2` prints `18446744073709551614`.

  This changes pinned behaviour: §5.1.1, "Note that integers are not limited in size", is the authority, and core-library.md's "Deviation: `i64` integers" records the old behaviour as the deviation it was. Use the file's `expect_ok` and `as_string` helpers, and send `printString` to the result:

```rust
#[test]
fn integer_addition_overflow_answers_a_large_integer() {
    let mut interpreter = Interpreter::bootstrap();
    let max = interpreter.stash(Value::SmallInteger(i64::MAX));
    let sum = expect_ok(
        interpreter.send(&max, Symbol::intern("+"), &[interpreter.stash(Value::SmallInteger(1))]),
        "i64::MAX + 1",
    );
    let printed = expect_ok(interpreter.send(&sum, Symbol::intern("printString"), &[]), "printString");
    interpreter.enter(|mutator| {
        assert_eq!(as_string(mutator.heap(), &mutator.fetch(&printed)), "9223372036854775808");
    });
}
```

`mixed_integer_float_arithmetic_reports_a_type_mismatch_not_an_overflow` (`:230`) stays as it is.

Delete `goldens/errors/inlined_integer_addition_overflow_is_uncaught.ns` and `.toml`. It pinned the overflow error the specification forbids. The inlining concern its comment names (the fast path must leave the overflow to the primitive) carries over to the new golden, whose first line takes that path. Create `goldens/arithmetic/an_overflowing_integer_operation_answers_a_large_integer.ns`:

```
Newspeak3
'Arithmetic'
(* §5.1.1: "integers are not limited in size". An `Integer` operation whose
   result does not fit becomes a `LargeInteger`, and one whose result fits
   again is an `Integer`, so each integer has one representation. The first
   line's operands are literal small integers, so the inlined `IntegerBinary`
   runs `checked_add`, which declines, and the ordinary `+` send promotes:
   inlining must not take the promotion away from the primitive. *)
class Overflow usingPlatform: p = Object new (
) (
    public main: p args: a = (
        | max min big |
        p transcript show: (9223372036854775807 + 1) printString; newLine.
        max:: 9223372036854775807.
        min:: max negated - 1.
        p transcript show: (min - 1) printString; newLine.
        p transcript show: (max * max) printString; newLine.
        p transcript show: min negated printString; newLine.
        p transcript show: min abs printString; newLine.
        p transcript show: (min // -1) printString; newLine.
        big:: max + 1.
        p transcript show: big class printString; newLine.
        p transcript show: max class printString; newLine.
        p transcript show: (big = (max + 1)) printString; newLine.
        p transcript show: (big hash = (max + 1) hash) printString; newLine.
        p transcript show: (big == (max + 1)) printString; newLine.
        p transcript show: (big = max) printString; newLine.
        p transcript show: (max = big) printString; newLine
    )
) : ()
```

`.toml` (values from Python):

```toml
stdout = '''
9223372036854775808
-9223372036854775809
85070591730234615847396907784232501249
9223372036854775808
9223372036854775808
9223372036854775808
LargeInteger
Integer
true
true
true
false
false
'''
```

`big - 1` arithmetic *on* a `LargeInteger` belongs to Task 5, which adds the golden line that shows demotion.

Create `goldens/kernel/a_value_with_a_large_integer_slot_compares_by_state.ns`. It pins `builtins/value.rs`'s derived equality and hash over a `LargeInteger` slot, and that two distinct `LargeInteger` objects of one value make equal values:

```
Newspeak3
'Kernel'
(* §3.1.1: a value's equality and hash come from its state. A slot holding a
   `LargeInteger` is compared by the integer's value, not by the object that
   holds it: the two `Box`es below hold two distinct `LargeInteger` objects of
   one value.

   The module is a value (`Object <: Value`) because §3.1.1 asks a value
   class's enclosing object to be one; the transcript lives in `Script`,
   which is no value -- the arrangement `a_value_has_no_identity.ns` uses. *)
class ValueSlots usingPlatform: platformAtCreation = Object <: Value () (
    class Box of: initialContents = Value new (
        | public contents = initialContents. |
    ) ()
    class Script on: aTranscript = Object new (
        | transcript = aTranscript. |
    ) (
        public run = (
            | first second third |
            first:: Box of: 9223372036854775807 + 1.
            second:: Box of: 9223372036854775807 + 1.
            third:: Box of: 9223372036854775807 * 2.
            transcript show: (first = second) printString; newLine.
            transcript show: (first hash = second hash) printString; newLine.
            transcript show: (first == second) printString; newLine.
            transcript show: (first = third) printString; newLine
        )
    )
    public main: platform args: arguments = (
        (Script on: platform transcript) run
    )
) : ()
```

`.toml`:

```toml
stdout = '''
true
true
true
false
'''
```

- [ ] **Step 2: Run them to verify they fail.**
Run: `cargo nextest run -p ns-runtime --test builtins_boolean_numbers` and `cargo nextest run -p ns-cli --test run_goldens large_integer overflowing`
Expected: FAIL. The overflow error is still raised.

- [ ] **Step 3: Add the class and the object.**
  - `types.rs`: append `LargeInteger` to `BuiltinClass` after `Future`, with a doc comment: "§5.1.1's unbounded integers: an `Integer` that does not fit `i64`. A subclass of `Integer`, whose instances are the immediates, so `3 class` stays `Integer`." Then append it to `ALL`.
  - Add `ObjectData::LargeInteger(crate::ArbitraryInteger)`, with a doc comment saying it never holds a value that fits `i64` (`Mutator::new_integer` decides) and holds no handle.
  - The release-build size test `an_object_is_the_size_the_amplification_figure_assumes` (`heap.rs:4108`) must still pass. `ArbitraryInteger` is 24 bytes (`Box<[u64]>` plus a `bool`), below `ClosureData`'s 32. Run `cargo nextest run -p ns-runtime --release an_object_is_the_size` once to confirm.
  - `kernel.rs::kernel_class_name`: `BuiltinClass::LargeInteger => "LargeInteger"`.
  - `encode.rs::builtin_class_tag`: `LargeInteger => 31`. `decode.rs::builtin_class_from_tag`: `31 => LargeInteger`.
  - `opcodes.rs:274`: "Next free number: 32".
  - `tests/opcode_tags.rs::builtin_class_tags_are_fixed`: a line for 31.

  `LargeInteger` is a class a program may hold (`big class`), so it is not added to `named_builtin`, `unnameable_class` or `referenced_builtin_class`'s refusals. `Object` gains no accessor for it: nothing on `corpus/runnable.toml` names it, and §3.2's accessors are for the predefined classes. Add `"LargeInteger"` to the absent list of `objects_class_accessors_are_exactly_the_nameable_builtins` (`kernel.rs:1552-1570`).

  - `trace.rs:66` and `heap.rs:3066`: add `ObjectData::LargeInteger(_)` beside `ObjectData::String(_)`.
  - `heap.rs::describe`: `ObjectData::LargeInteger(integer) => integer.to_decimal_string()`, since a `SmallInteger` describes as its bare digits.
  - `heap.rs`, beside `str_value`:

```rust
    pub fn large_integer(&self, object: ObjectReference<'gc>) -> Option<crate::ArbitraryInteger> {
        self.check_object_origin(object);
        match &self.objects[object.index as usize].data {
            ObjectData::LargeInteger(integer) => Some(integer.clone()),
            _ => None,
        }
    }
```

  - `interpreter/mod.rs`, beside `new_string`:

```rust
    /// The one way an `ArbitraryInteger` becomes a `Value`, so that every
    /// integer has one representation: one that fits `i64` is a
    /// `SmallInteger`, never a `LargeInteger` object, which `Integer>>=`,
    /// `hash` and the image's canonical-encoding check all rely on.
    pub fn new_integer(&mut self, integer: crate::ArbitraryInteger) -> Value<'gc> {
        match integer.to_i64() {
            Some(small) => Value::SmallInteger(small),
            None => {
                let class = self.builtin_class(BuiltinClass::LargeInteger);
                Value::Object(self.new_instance_object(class, ObjectData::LargeInteger(integer)))
            }
        }
    }
```

- [ ] **Step 4: Write `Kernel.ns`'s class.** Place it after `Integer`:

```
    (* §5.1.1: "integers are not limited in size". An `Integer` whose value
       does not fit a `SmallInteger`; every leaf answers an `Integer` when
       the result fits again, so each integer has one representation.
       `3 class` stays `Integer` -- the section asks only for its protocol. *)
    public class LargeInteger = Integer new () (
        public + other = ( ^vmMirror largeInteger: self add: other ifFail: [^super + other] )
        public - other = ( ^vmMirror largeInteger: self subtract: other ifFail: [^super - other] )
        public * other = ( ^vmMirror largeInteger: self multiply: other ifFail: [^super * other] )
        public // other = ( ^vmMirror largeInteger: self floorDivide: other ifFail: [^super // other] )
        public < other = ( ^vmMirror largeInteger: self lessThan: other ifFail: [^super < other] )
        public > other = ( ^vmMirror largeInteger: self greaterThan: other ifFail: [^super > other] )
        public <= other = ( ^vmMirror largeInteger: self lessOrEqual: other ifFail: [^super <= other] )
        public >= other = ( ^vmMirror largeInteger: self greaterOrEqual: other ifFail: [^super >= other] )
        public = other = ( ^vmMirror largeInteger: self equals: other ifFail: [^super = other] )
        public hash = ( ^vmMirror largeIntegerHash: self ifFail: [^super hash] )
        public negated = ( ^vmMirror largeIntegerNegated: self ifFail: [^super negated] )
        public abs = ( ^vmMirror largeIntegerAbsolute: self ifFail: [^super abs] )
        public printString = ( ^vmMirror largeIntegerPrintString: self ifFail: [^super printString] )
        public asString = ( ^vmMirror largeIntegerAsString: self ifFail: [^super asString] )
    )
```

`==` is inherited from `Value` (`^self = other`). A `LargeInteger` that is not really one (a `LargeInteger new`, a user subclass's instance) declines every leaf and falls through `super` to `Integer`'s methods, whose leaves decline a non-`SmallInteger` receiver, and on to `Object`'s. That is exactly what `Integer new` does today.

- [ ] **Step 5: Add the primitive family and its rows.** In `builtins/primitive.rs`, add `LargeInteger(LargeIntegerPrimitive)` to `Primitive` after `Integer`, and the enum:

```rust
/// `LargeInteger`'s leaves: `Integer`'s protocol for a receiver whose
/// payload is `ObjectData::LargeInteger`. A family of its own rather than
/// `Integer`'s leaves widened, so `IntegerPrimitive` keeps meaning "the
/// receiver is a `SmallInteger`", which is what a JIT tier emits inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeIntegerPrimitive {
    Add, Subtract, Multiply, FloorDivide,
    Less, Greater, LessOrEqual, GreaterOrEqual,
    Equal, Hash, Negated, Absolute, PrintString,
}
```

`answers_false_to_a_stranger`: `Primitive::LargeInteger(LargeIntegerPrimitive::Equal) => true`, and every other variant `=> false`, listed by name as the `Integer` arm is. Add the `MIRROR` rows after `Integer`'s, following their exact form. The `reported` column is the Newspeak selector, as in `Integer`'s rows (`primitive.rs:423-487`):

| selector | reported | primitive |
|---|---|---|
| `largeInteger:add:ifFail:` | `+` | `Add` |
| `largeInteger:subtract:ifFail:` | `-` | `Subtract` |
| `largeInteger:multiply:ifFail:` | `*` | `Multiply` |
| `largeInteger:floorDivide:ifFail:` | `//` | `FloorDivide` |
| `largeInteger:lessThan:ifFail:` | `<` | `Less` |
| `largeInteger:greaterThan:ifFail:` | `>` | `Greater` |
| `largeInteger:lessOrEqual:ifFail:` | `<=` | `LessOrEqual` |
| `largeInteger:greaterOrEqual:ifFail:` | `>=` | `GreaterOrEqual` |
| `largeInteger:equals:ifFail:` | `=` | `Equal` |
| `largeIntegerHash:ifFail:` | `hash` | `Hash` |
| `largeIntegerNegated:ifFail:` | `negated` | `Negated` |
| `largeIntegerAbsolute:ifFail:` | `abs` | `Absolute` |
| `largeIntegerPrintString:ifFail:` | `printString` | `PrintString` |
| `largeIntegerAsString:ifFail:` | `asString` | `PrintString` |

Dispatch the family in `builtins/mod.rs` next to `Primitive::Integer`, to `numbers::execute_large_integer`. The kernel test `every_kernel_method_has_the_fast_path_it_should` should then see each `LargeInteger` forwarder as `MethodFastPath::Mirror`. If it lists methods by hand, add them.

- [ ] **Step 6: Promote in `execute_integer`.**
  - `+`, `-` and `*`: on `checked_*` failure, compute in `i128` (which cannot overflow for two `i64`s) and answer `mutator.new_integer(ArbitraryInteger::from_i128(result))`.
  - `//`: the one `None` besides a zero divisor is `i64::MIN // -1`; answer `new_integer(ArbitraryInteger::from_i128(-(i128::from(receiver_integer))))` for it, keeping the zero-divisor error.
  - `negated` and `abs`: on `None`, answer `new_integer(ArbitraryInteger::from_i64(receiver_integer).negated())` (or `.absolute()`).

  The `overflow` closure and its "Integer overflow computing" text go away entirely. No integer operation can overflow any more. The closures borrow `mutator` immutably for `mismatch`, so allocate after the match yields the `i128`. For example:

```rust
    let arithmetic = |operator: &str, operation: fn(i128, i128) -> i128| match argument_integer() {
        None => Err(mismatch(operator)),
        Some(argument) => Ok(operation(i128::from(receiver_integer), i128::from(argument))),
    };
    match primitive {
        IntegerPrimitive::Add => Some(arithmetic("+", |left, right| left + right)
            .map(|sum| mutator.new_integer(ArbitraryInteger::from_i128(sum)))),
        // …
    }
```

Keep the `SmallInteger` fast answer where `checked_*` succeeds, so the common case allocates nothing. `new_integer` on an `i128` that fits answers a `SmallInteger` either way, so this is a matter of speed, not of correctness. Restructure the closures as the borrow checker needs.

- [ ] **Step 7: Write `execute_large_integer`**, answering `Equal`, `Hash` and `PrintString` in this task and declining (`None`) for the rest until Task 5:

```rust
pub(crate) fn execute_large_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: LargeIntegerPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    // The payload, never the class: a user subclass's instance, or a
    // `LargeInteger new`, is no number and declines to `Object`'s methods.
    let Value::Object(object) = receiver else { return None };
    let receiver_integer = mutator.heap.large_integer(object)?;
    match primitive {
        LargeIntegerPrimitive::Equal => crate::interpreter::future::declines_for_a_future(
            mutator,
            Primitive::LargeInteger(primitive),
            large_integer_equals(&mutator.heap, &receiver_integer, arguments.first()),
            receiver,
            arguments,
        )
        .map(|answer| Ok(Value::Boolean(answer))),
        LargeIntegerPrimitive::Hash => Some(Ok(Value::SmallInteger(large_integer_hash(&receiver_integer)))),
        LargeIntegerPrimitive::PrintString => Some(Ok(Value::Object(
            mutator.new_string(receiver_integer.to_decimal_string()),
        ))),
        _ => None,
    }
}

/// `LargeInteger>>=`'s rule: only a `LargeInteger` of the same value is
/// equal. A `SmallInteger` never is, because `new_integer` never makes a
/// `LargeInteger` of a value that fits.
pub(crate) fn large_integer_equals<'gc>(
    heap: &Heap<'gc>,
    receiver: &ArbitraryInteger,
    argument: Option<&Value<'gc>>,
) -> bool {
    match argument {
        Some(Value::Object(object)) => heap.large_integer(*object).is_some_and(|other| other == *receiver),
        _ => false,
    }
}
```

The `_ => None` arm is temporary: Task 5 replaces it with the named variants, so the match is exhaustive again. `declines_for_a_future` debug-checks that its caller is marked in `answers_false_to_a_stranger`, which Step 5 did.

- [ ] **Step 8: Derived equality and hash** in `builtins/value.rs`. In `compare_node`'s `(Value::Object(left), Value::Object(right))` arm, before the `str_value` test:

```rust
            if let Some(integer) = mutator.heap().large_integer(left) {
                return Some(Pair::Decided(numbers::large_integer_equals(
                    mutator.heap(),
                    &integer,
                    Some(&second),
                )));
            }
```

In `hash_node`'s `Value::Object` arm, beside `str_value`:

```rust
            if let Some(integer) = mutator.heap().large_integer(object) {
                return Some(Node::Leaf(numbers::large_integer_hash(&integer)));
            }
```

`(Value::SmallInteger(left), _)` already answers `false` against a `LargeInteger` through `integer_equals`, which is right.

- [ ] **Step 9: Run the tests and the goldens.** The new golden, the value-slot golden and the replaced Rust tests pass. Run the kernel tests (`cargo nextest run -p ns-runtime kernel`) and fix every hand-kept list they name. Then run the per-commit gate.

- [ ] **Step 10: Commit.**

```bash
git add -A crates/ns-runtime goldens
git commit -m "feat: promote an overflowing Integer to a LargeInteger (§5.1.1)"
```

---

### Task 5: `LargeInteger` arithmetic, and mixed operands

**Files:**
- Modify: `crates/ns-runtime/src/builtins/numbers.rs`:
  - `execute_large_integer` complete;
  - `execute_integer` accepts a `LargeInteger` argument;
  - `execute_float` converts one;
  - the module doc's "Known gap" rewritten for what now holds.
- Test: `goldens/arithmetic/large_integer_arithmetic.*`, `goldens/arithmetic/large_integer_algorithms_agree_at_every_size.*`, `goldens/arithmetic/a_float_meets_a_large_integer.*`, `goldens/errors/large_integer_floor_division_by_zero_is_uncaught.*`, `goldens/errors/a_large_integer_meets_a_float.*`, `goldens/kernel/a_subclass_of_large_integer_is_no_number.*` (all new); one line added to `goldens/arithmetic/an_overflowing_integer_operation_answers_a_large_integer.*`

**Interfaces:**
- Consumes: `ArbitraryInteger`; `Mutator::new_integer`; `Heap::large_integer`; `large_integer_equals`, `large_integer_hash`; `LargeIntegerPrimitive`.
- Produces: nothing new for later tasks.

- [ ] **Step 1: Write the failing goldens.** All values were computed with Python 3.14, whose `//` rounds toward negative infinity. `goldens/arithmetic/large_integer_arithmetic.ns`:

```
Newspeak3
'Arithmetic'
(* `LargeInteger`'s arithmetic and ordering, with a `LargeInteger` on either
   side and an `Integer` on the other, and a result that fits again answered
   as an `Integer`. The operands are built by arithmetic, not literals, so
   this does not depend on large literals (their own golden). *)
class LargeArithmetic usingPlatform: p = Object new (
) (
    public main: p args: a = (
        | two big other small back count |
        two:: 2.
        small:: 9223372036854775807.
        count:: 0.
        big:: 1.
        [count < 200] whileTrue: [big:: big * two. count:: count + 1].
        p transcript show: big printString; newLine.
        other:: 0 - (big // (small * small)).
        p transcript show: other printString; newLine.
        p transcript show: (big + other) printString; newLine.
        p transcript show: (big - other) printString; newLine.
        p transcript show: (big * other) printString; newLine.
        p transcript show: (big // -7) printString; newLine.
        p transcript show: (other // 7) printString; newLine.
        p transcript show: (big // other) printString; newLine.
        p transcript show: (other // big) printString; newLine.
        p transcript show: (big < other) printString; newLine.
        p transcript show: (big > other) printString; newLine.
        p transcript show: (big <= big) printString; newLine.
        p transcript show: (other >= big) printString; newLine.
        p transcript show: (5 < big) printString; newLine.
        p transcript show: (5 > other) printString; newLine.
        p transcript show: (5 + big) printString; newLine.
        p transcript show: (5 - big) printString; newLine.
        p transcript show: (5 // big) printString; newLine.
        p transcript show: (-5 // big) printString; newLine.
        back:: (small + 1) - 1.
        p transcript show: (back = small) printString; newLine.
        p transcript show: back class printString; newLine.
        p transcript show: (big // big) class printString; newLine.
        p transcript show: other negated printString; newLine.
        p transcript show: other abs printString; newLine.
        p transcript show: (small + 1) negated class printString; newLine
    )
) : ()
```

No operand is a large literal, since those come only in Task 6. `.toml`, computed with Python for exactly this program (`big = 2**200`, `small = 2**63 - 1`, `other = -(big // (small * small))`). The last three lines are worked out as follows:
- `(big // big) class` is `Integer`, because 1 fits.
- `other` is still a `LargeInteger` (about 2^74).
- `(small + 1) negated` is `-2^63`, which fits `i64`, so its class is `Integer`.

```toml
stdout = '''
1606938044258990275541962092341162602522202993782792835301376
-18889465931478580858880
1606938044258990275541962092341162602503313527851314254442496
1606938044258990275541962092341162602541092459714271416160256
-30354201441027016739698610523402307084907483590419082039490512364704698909725818880
-229562577751284325077423156048737514646028999111827547900197
-2698495133068368694126
-85070591730234615847396907784232501252
-1
false
true
true
false
true
true
1606938044258990275541962092341162602522202993782792835301381
-1606938044258990275541962092341162602522202993782792835301371
0
-1
true
Integer
Integer
18889465931478580858880
18889465931478580858880
Integer
'''
```

`goldens/arithmetic/large_integer_algorithms_agree_at_every_size.ns` squares 3 repeatedly, so every threshold of both dispatches is crossed at least once. The operand sizes, in 64-bit digits:

| Operation | Operand sizes | Algorithm |
|---|---|---|
| the squares up to `3^2048` | at most 26 digits | schoolbook |
| `3^4096 = (3^2048)^2` | 51 digits | Karatsuba |
| `3^32768 = (3^16384)^2` | 406 digits | Toom-3 |
| `3^65536 = (3^32768)^2` | 812 digits | the number-theoretic transform |
| `huge * medium` | 1624 by 102 digits | half-Karatsuba |
| `(huge - 1) // large` and `large // medium` | dividends above 128 digits, divisors above 64 | Burnikel-Ziegler |
| `(medium - 1) // small` | a 51-digit divisor | algorithm D |
| `lastSixOf:` | a one-digit divisor | one-digit division |

```
Newspeak3
'Arithmetic'
(* Repeated squaring of 3 crosses every threshold of both dispatches:
   schoolbook, Karatsuba, Toom-3 and the number-theoretic transform for the
   squares, half-Karatsuba for the lopsided product, Burnikel-Ziegler, Knuth's
   algorithm D and one-digit division for the quotients. Each line is a
   decimal digit count and the last six digits, which Python computed. *)
class Squares usingPlatform: platform = Object new (
) (
    lastSixOf: integer = ( ^integer - (integer // 1000000 * 1000000) )
    report: integer on: transcript = (
        transcript show: integer printString size printString; show: ' '; show: (lastSixOf: integer) printString; newLine
    )
    public main: platform args: arguments = (
        | transcript power count small medium large huge |
        transcript:: platform transcript.
        power:: 3.
        count:: 0.
        [count < 11] whileTrue: [power:: power * power. count:: count + 1].
        small:: power.
        medium:: small * small.
        count:: 12.
        power:: medium.
        [count < 15] whileTrue: [power:: power * power. count:: count + 1].
        large:: power.
        huge:: large * large.
        report: small on: transcript.
        report: medium on: transcript.
        report: large on: transcript.
        report: huge on: transcript.
        transcript show: (huge // large = large) printString; newLine.
        report: (huge - 1) // large on: transcript.
        report: large // medium on: transcript.
        report: (medium - 1) // small on: transcript.
        report: huge * medium on: transcript
    )
) : ()
```

The `.toml` was computed with Python 3.14 (`sys.set_int_max_str_digits(0)`) for `small = 3**2048`, `medium = 3**4096`, `large = 3**32768` and `huge = 3**65536`. Each line is the digit count, then `value % 1000000`.

```toml
stdout = '''
978 703361
1955 696321
15635 365761
31269 109121
true
15635 365760
13681 944641
978 703360
33223 243841
'''
```

`goldens/arithmetic/a_float_meets_a_large_integer.ns`. A `Float`'s leaves convert a `LargeInteger` argument to the nearest `double`, as they convert an `Integer` one (core-library.md, "Planned: large integers"):

```
Newspeak3
'Arithmetic'
class FloatMeetsLarge usingPlatform: p = Object new (
) (
    public main: p args: a = (
        | big |
        big:: 9223372036854775807 + 1 * 2.
        p transcript show: (1.5 + big) printString; newLine.
        p transcript show: (1.5 < big) printString; newLine.
        p transcript show: (1.5 = big) printString; newLine
    )
) : ()
```

`big` is `2^64` (binary operators associate left to right), and `1.5 + 2^64` rounds to `2^64` as an `f64`. A `Float` prints through Rust's `f64` `Display`, which writes the shortest round-trip digits without an exponent: `18446744073709552000`. `=` stays class-tight, so it answers `false`.

```toml
stdout = '''
18446744073709552000
true
false
'''
```

`goldens/errors/a_large_integer_meets_a_float.ns` checks that a `LargeInteger`'s leaves meet a `Float` argument as an `Integer`'s do, with the type mismatch. Its shape is that of `inlined_integer_floor_division_by_zero_is_uncaught.ns`, and its body is `p transcript show: (9223372036854775807 + 1 + 1.5) printString; newLine`. Expected: `status = 1`, and a `stderr` that is `numbers.rs`'s `type_mismatch` text for selector `+` with the `Float` described as its argument, prefixed `Uncaught Newspeak error: `. Run it once, check the text is exactly that function's wording (neither "overflow" nor `doesNotUnderstand:`), and record it.

`goldens/errors/large_integer_floor_division_by_zero_is_uncaught.ns` has the body `p transcript show: ((9223372036854775807 + 1) // 0) printString; newLine`. Its `.toml` has `status = 1` and a stderr of `Uncaught Newspeak error: division by zero`, matching `goldens/errors/inlined_integer_floor_division_by_zero_is_uncaught.toml`'s form.

`goldens/kernel/a_subclass_of_large_integer_is_no_number.ns` is modelled on `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods.ns`, and its output follows from that golden's (`true an Object` for `MyInteger`). No program can name `LargeInteger` bare, since `Object` has no accessor for it. A superclass clause's receiver is resolved in the enclosing scope, so a module method answers the class:

```
Newspeak3
'Kernel'
(* A user subclass of `LargeInteger`, and `LargeInteger new`, inherit its
   methods but no integer payload, so every `LargeInteger` leaf declines,
   then every `Integer` leaf, and the send reaches `Object`'s: nothing takes
   such an instance for a number. `LargeInteger` is reached through an
   instance's class, since `Object` declares no accessor for it. The module
   is a value because `MyLarge`, a subclass of `Value`, asks its enclosing
   object to be one (§3.1.1). *)
class LargeFallThrough usingPlatform: platformAtCreation = Object <: Value () (
    class MyLarge = largeIntegerClass new () ()
    class Script on: aTranscript = Object new (
        | transcript = aTranscript. |
    ) (
        report: instance = (
            transcript show: (instance = instance) printString; show: ' '; show: instance printString; newLine
        )
        public run = (
            report: MyLarge new.
            report: largeIntegerClass new.
            transcript show: (MyLarge new + 1) printString; newLine
        )
    )
    largeIntegerClass = ( ^(9223372036854775807 + 1) class )
    public main: platform args: arguments = (
        (Script on: platform transcript) run
    )
) : ()
```

`Script` reaches `largeIntegerClass` as an implicit-receiver send, which finds the enclosing module's method lexically. `.toml`:

```toml
status = 1
stdout = '''
true an Object
true an Object
'''
stderr = '''
Uncaught Newspeak error: an object does not understand #+
'''
```

If `largeIntegerClass new` in a superclass clause is refused (a clause must name a class the resolver can see), write the clause as the resolver allows and keep the three reports. Nothing may answer a number.

Add one line to `goldens/arithmetic/an_overflowing_integer_operation_answers_a_large_integer.ns`, after the last one: `p transcript show: ((big - 1) = max) printString; newLine`. Add `true` to its `.toml`.

- [ ] **Step 2: Run them to verify they fail.**
Run: `cargo nextest run -p ns-cli --test run_goldens large_integer float_meets overflowing subclass_of_large`
Expected: FAIL. The arithmetic leaves decline, so the sends fall through to `doesNotUnderstand:`.

- [ ] **Step 3: One operand reader** in `numbers.rs`, used by both families:

```rust
/// An integer operand of either representation, for the slow path: a
/// `SmallInteger` widened, or a `LargeInteger`'s payload. `None` for
/// anything else, which each leaf turns into its own type mismatch.
fn integer_operand<'gc>(heap: &Heap<'gc>, value: Option<&Value<'gc>>) -> Option<ArbitraryInteger> {
    match value {
        Some(Value::SmallInteger(integer)) => Some(ArbitraryInteger::from_i64(*integer)),
        Some(Value::Object(object)) => heap.large_integer(*object),
        _ => None,
    }
}
```

- [ ] **Step 4: Complete `execute_large_integer`.** Replace `_ => None` with every remaining variant, named:
  - `Add`, `Subtract` and `Multiply` read `integer_operand`, raise `type_mismatch` on `None`, and answer `new_integer(receiver_integer.add(&argument))` and so on.
  - `FloorDivide` answers the `"division by zero"` error (worded exactly as `execute_integer`'s) for a zero divisor, and otherwise `new_integer(quotient)`.
  - The four orderings compare with `Ord` after `integer_operand`, and raise `type_mismatch` on `None`.
  - `Negated` and `Absolute` are `new_integer(receiver_integer.negated())` and `.absolute()`.

- [ ] **Step 5: Mixed operands in `execute_integer`.** Where `argument_integer()` is `None`, try `integer_operand` before raising the mismatch. Past that point the argument is a `LargeInteger`, so compute through `ArbitraryInteger::from_i64(receiver_integer)` and `new_integer`. For the orderings compare the two `ArbitraryInteger`s. `Equal` stays `integer_equals` (a `SmallInteger` never equals a `LargeInteger`). `//` by a `LargeInteger` is never a zero divisor.

- [ ] **Step 6: `Float` meets `LargeInteger`.** In `execute_float`'s `argument_float`, add `Some(Value::Object(object)) => mutator.heap.large_integer(*object).map(|integer| integer.to_f64())`. Since `argument_float` is a closure over `mutator`, read `arguments.first()` through `mutator.heap` as the other closures already do. `float_equals` stays class-tight and is unchanged.

- [ ] **Step 7: Rewrite the module doc's "Known gap: no numeric tower"** in `numbers.rs` to state what holds now:
  - integers are unbounded (`Integer` and `LargeInteger`, one representation each);
  - `Integer`'s and `LargeInteger`'s leaves accept either as the argument;
  - a `Float` converts either to the nearest `double`;
  - an integer meeting a `Float` argument is a type mismatch;
  - `=` is class-tight across `Integer` and `Float`.

  The sentence "`LargeInteger`/`Fraction` belong in the same change" goes: `Fraction` and the tower remain, with `docs/design/core-library.md`, "Gap: an integer has no `/`", as their pointer.

- [ ] **Step 8: Run the goldens and the per-commit gate.** All pass under both lowerings (`run_goldens` runs each golden under both).

- [ ] **Step 9: Commit.**

```bash
git add -A crates/ns-runtime/src/builtins goldens
git commit -m "feat: LargeInteger arithmetic, ordering and mixed operands"
```

---

### Task 6: every integer literal is representable; `MalformedInteger` retires

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs`:
  - `Literal::LargeInteger(ArbitraryInteger)` added;
  - `Literal::MalformedInteger` removed;
  - its doc comment (`:15-35`);
  - the display at `:802`, which gives `large-int <decimal>`;
  - the fixture at `:1086`, which becomes a `LargeInteger` literal.
- Modify: `crates/ns-runtime/src/interpreter/literals.rs`:
  - `parse_integer_literal` answers `Result<IntegerLiteral, String>`;
  - `malformed_integer_error` removed;
  - the tests at `:165`, `:179`.
- Modify: `crates/ns-runtime/src/compiler/lower.rs`:
  - `lower_literal` (`:1952-1961`);
  - the comment at `:1266`;
  - the test at `:2520`.
- Modify: `crates/ns-runtime/src/vm.rs` (`make_value`, `:1178-1200`)
- Test: `crates/ns-runtime/tests/vm.rs:399` (replaced); `goldens/arithmetic/large_integer_literals.*` (new)

**Interfaces:**
- Consumes: `ArbitraryInteger::parse`, `Mutator::new_integer`.
- Produces:
  - `pub(crate) enum IntegerLiteral { Small(i64), Large(ArbitraryInteger) }` in `interpreter/literals.rs`;
  - `bytecode::Literal::LargeInteger(ArbitraryInteger)`, which never holds a value that fits `i64`.

  Task 7 encodes the latter.

- [ ] **Step 1: Write the failing tests.** Replace `tests/vm.rs:399`'s `a_malformed_integer_literal_raises_an_out_of_range_error_when_evaluated` with `an_integer_literal_beyond_i64_evaluates_to_a_large_integer`: `^99999999999999999999 printString` answers `'99999999999999999999'`. Keep the file's existing helpers. In `interpreter/literals.rs`, replace `a_decimal_literal_out_of_range_says_so` (`:165`) with `a_decimal_literal_beyond_i64_is_large`, asserting `parse_integer_literal("99999999999999999999")` is `Ok(IntegerLiteral::Large(_))` whose `to_decimal_string()` round-trips. The bad-digit test at `:179` stays: an image, or a test, can still hand it text no parser admitted. In `compiler/lower.rs`, replace `an_out_of_range_integer_literal_lowers_to_malformed_integer_not_a_panic` (`:2520`) with `an_integer_literal_beyond_i64_lowers_to_a_large_integer_literal`, expecting `BytecodeLiteral::LargeInteger(ArbitraryInteger::parse("99999999999999999999", 10).unwrap())`.

`goldens/arithmetic/large_integer_literals.ns`:

```
Newspeak3
'Arithmetic'
(* §5.1.1 bounds no literal: "integers are not limited in size". A decimal
   and a radix literal beyond `i64`, a negative one, and the two `i64` edges,
   which stay `Integer`s. *)
class Literals usingPlatform: p = Object new (
) (
    public main: p args: a = (
        p transcript show: 99999999999999999999 printString; newLine.
        p transcript show: -99999999999999999999 printString; newLine.
        p transcript show: 16rFFFFFFFFFFFFFFFFFFFF printString; newLine.
        p transcript show: 2r10000000000000000000000000000000000000000000000000000000000000000000000 printString; newLine.
        p transcript show: 9223372036854775808 class printString; newLine.
        p transcript show: 9223372036854775807 class printString; newLine.
        p transcript show: -9223372036854775808 class printString; newLine.
        p transcript show: -9223372036854775809 class printString; newLine.
        p transcript show: (99999999999999999999 = 99999999999999999999) printString; newLine
    )
) : ()
```

`.toml` (Python: `0xFFFFFFFFFFFFFFFFFFFF`, `2**70`):

```toml
stdout = '''
99999999999999999999
-99999999999999999999
1208925819614629174706175
1180591620717411303424
LargeInteger
Integer
Integer
LargeInteger
true
'''
```

Count the zeros of the `2r1…` literal: it must be a `1` followed by 70 zeros.

- [ ] **Step 2: Run them to verify they fail.** The tests do not compile against the new names. The golden prints the malformed-literal error.

- [ ] **Step 3: `parse_integer_literal`** answers `IntegerLiteral`. It tries `i64` first, as today, and falls back to `ArbitraryInteger::parse(digits, radix)`. The radix's leading `-` negates the whole value, as it does now. It answers `IntegerLiteral::Large` only when `to_i64()` is `None`, so a literal that fits is always `Small`. Its doc comment drops "out of `i64` range" as a failure: the only failures left are texts no parser produces (a bad radix, a bad digit), which the image decoder does not supply either, since Task 7 carries digits, not text. Remove `malformed_integer_error`.

- [ ] **Step 4: `lower_literal`**:

```rust
        // Parsed once, here. The parser admits only integer literals that
        // denote a value (§5.1.1 bounds none; `check_radix_number` refuses a
        // radix or digit that denotes nothing), so a refusal is a compiler
        // invariant, not a program error.
        ast::Literal::Integer { text, .. } => match parse_integer_literal(text) {
            Ok(IntegerLiteral::Small(integer)) => BytecodeLiteral::SmallInteger(integer),
            Ok(IntegerLiteral::Large(integer)) => BytecodeLiteral::LargeInteger(integer),
            Err(message) => panic!("the parser admitted an integer literal that denotes nothing: {message}"),
        },
```

Update the comment at `lower.rs:1266` that says "exactly like `MalformedInteger`" to name `UnresolvedIdentifier` alone.

- [ ] **Step 5: `make_value`**: `Literal::LargeInteger(integer) => mutator.new_integer(integer.clone())`, a fresh object each time it runs. A value needs no identity (object-model.md, "Literals"). Remove the `MalformedInteger` arm, and change the doc comment to name only `UnresolvedIdentifier` as the deferred error.

- [ ] **Step 6: `bytecode.rs`.** Remove the variant and add `LargeInteger(crate::ArbitraryInteger)`. Rewrite the doc comment so it has one deferred-error literal, `UnresolvedIdentifier`, and says that a `LargeInteger` literal's digits live in the code, which holds no handle, and that `LoadLiteral` allocates the object each time. The disassembly display is `Literal::LargeInteger(integer) => format!("large-int {}", integer.to_decimal_string())`. The fixture at `:1086` becomes `Literal::LargeInteger(ArbitraryInteger::parse("99999999999999999999", 10).unwrap())`, and its expected display follows.

- [ ] **Step 7: The image code will not compile yet**, because `encode.rs:788` and `decode.rs:2035` name the removed variant. Do Task 7's Step 3 now (the literal tag) in this same commit, or comment nothing out: the per-commit gate must pass. **Merge Tasks 6 and 7 into one commit if that is simpler**; they are separate tasks for review, not for history.

- [ ] **Step 8: Run the tests, the golden and the per-commit gate.** All pass.

- [ ] **Step 9: Commit** (with Task 7, see Step 7).

---

### Task 7: the image carries a `LargeInteger`

The design (image-format.md, "Integers") is:
- a `LargeInteger` travels as a **value**, `VALUE_LARGE_INTEGER` (9), and as a **literal**, `LITERAL_LARGE_INTEGER` (10), each carrying a sign byte, a `u32` digit count and that many `u64` digits, least significant first;
- it is never an object record, since a value has no identity;
- the decoder refuses a non-canonical encoding;
- literal tag 8 retires.

**Files:**
- Modify: `crates/ns-runtime/src/image/opcodes.rs`:
  - `VALUE_LARGE_INTEGER = 9` and "Next free number: 10";
  - `LITERAL_LARGE_INTEGER = 10` and "Next free number: 11";
  - `LITERAL_MALFORMED_INTEGER` removed and `RETIRED_LITERALS` = `[6, 7, 8]`, with a dated doc line for 8: "`MalformedInteger`, an integer literal beyond `i64`, until every integer literal became representable (2026-09-29)".
- Modify: `crates/ns-runtime/src/image/encode.rs`:
  - `encode_literal` (`:788`);
  - `encode_value` (`:1380`ff);
  - the graph walk's `visit_value` (`:1029`ff), which must not give a `LargeInteger` object a record.
- Modify: `crates/ns-runtime/src/image/decode.rs`:
  - the literal reader (`:2035`);
  - `decode_value` (`:2294`), which allocates;
  - a new `DecodeError::NonCanonicalInteger`.
- Modify: `crates/ns-runtime/src/image/inspect.rs` (`read_value`, the literal walk)
- Test: `crates/ns-runtime/tests/opcode_tags.rs` (`:78`, `:137`, the retired list); `crates/ns-runtime/tests/image.rs` (`:578` replaced, new round-trip and refusal tests)

**Interfaces:**
- Consumes: `bytecode::Literal::LargeInteger`; `Heap::large_integer`; `Mutator::new_integer`.
- Produces: `DecodeError::NonCanonicalInteger`.

- [ ] **Step 1: Write the failing tests** in `tests/image.rs`, modelled on the neighbouring round-trip tests (`scalar_value_boundary_cases_round_trip`, `:141`, and the literal round trip at `:578`):
  - `a_large_integer_value_round_trips`: a module whose slot holds `9223372036854775807 + 1` and one holding its negation, snapshotted and loaded. The loaded slots print `9223372036854775808` and `-9223372036854775808`, and the second is an `Integer`, not a `LargeInteger`, because `-2^63` fits.
  - `a_large_integer_literal_round_trips`, replacing the `MalformedInteger("16r1.8")` round trip at `:578`: `Literal::LargeInteger(ArbitraryInteger::parse("99999999999999999999", 10).unwrap())` survives encode and decode unchanged.
  - `a_non_canonical_large_integer_is_refused`: hand-assembled bytes for each of the following. The existing tests in the file that build a byte image by hand show how to splice a value into a record.
    - (a) a value tag 9 whose digits fit `i64` (count 1, digit 5);
    - (b) a zero top digit (count 2, digits `[1, 0]`);
    - (c) a negative zero (sign 1, count 0);
    - (d) a count of `u32::MAX` with ten bytes left.

    (a) to (c) are `DecodeError::NonCanonicalInteger`. (d) is the truncation error the reader already answers (`bounded_count` refuses before allocating). The same four for the literal tag 10.
  - `a_large_integer_is_a_value_not_a_record`: after a snapshot of a module holding two slots with the same `LargeInteger` object, `inspect`'s output contains no object record whose class is `LargeInteger`, and both slots show `large-int 9223372036854775808`.

In `tests/opcode_tags.rs`:
- assert `VALUE_LARGE_INTEGER == 9` and `LITERAL_LARGE_INTEGER == 10`;
- remove the `LITERAL_MALFORMED_INTEGER` lines (`:78`, `:137`), and add 8 to the asserted retired list;
- the test that feeds each retired literal tag to the decoder (`tests/image.rs:1106`, `an_invalid_or_retired_literal_tag_is_a_decode_error_not_a_panic`) must now cover 8. Its "unknown tag" example, `LITERAL_UNRESOLVED_IDENTIFIER + 1`, is 10, which is now live, so change it to 11 (or `LITERAL_LARGE_INTEGER + 1`).

- [ ] **Step 2: Run them to verify they fail.**

- [ ] **Step 3: Encode.** One shared writer:

```rust
/// A `LargeInteger`'s wire form, shared by the value and the literal tag:
/// a sign byte, the digit count, the digits least significant first.
fn write_large_integer(writer: &mut Writer, integer: &ArbitraryInteger) {
    writer.write_u8(u8::from(integer.is_negative()));
    writer.write_u32(wire_count(integer.magnitude().len()));
    for digit in integer.magnitude() {
        writer.write_u64(*digit);
    }
}
```

Use the file's existing `usize`-to-`u32` narrowing helper (the one documented at `:146`, "A `usize` count or index narrowed to the wire's `u32` field") under its real name, and add `write_u64` to `Writer` if it is missing, beside `write_i64`. `encode_literal`: `Literal::LargeInteger(integer) => { writer.write_u8(LITERAL_LARGE_INTEGER); write_large_integer(writer, integer); }`. `encode_value`, before the platform-path test: `if let Some(integer) = context.heap.large_integer(*object) { writer.write_u8(VALUE_LARGE_INTEGER); write_large_integer(writer, &integer); return; }`. `visit_value`, after `seen_through`: `if heap.large_integer(*object).is_some() { return; }`, so no record and no object id is made for it.

- [ ] **Step 4: Decode.** One shared reader, which validates before it allocates:

```rust
/// The inverse of `encode::write_large_integer`. Refuses every encoding but
/// the canonical one, so an integer has one representation after a load as
/// before it: one that fits `i64` must travel as `VALUE_SMALL_INTEGER`
/// (or `LITERAL_SMALL_INTEGER`), and neither a zero top digit nor a negative
/// zero is written by the encoder.
fn read_large_integer(reader: &mut Reader) -> Result<ArbitraryInteger, DecodeError> {
    let negative = match reader.read_u8()? {
        0 => false,
        1 => true,
        tag => return Err(DecodeError::InvalidTag { what: "large integer sign", tag }),
    };
    let count = bounded_count(reader.read_u32()?, 8, reader)?;
    let mut magnitude = Vec::with_capacity(count);
    for _ in 0..count {
        magnitude.push(reader.read_u64()?);
    }
    if magnitude.last() == Some(&0) || (magnitude.is_empty() && negative) {
        return Err(DecodeError::NonCanonicalInteger);
    }
    let integer = ArbitraryInteger::from_parts(negative, magnitude);
    if integer.to_i64().is_some() {
        return Err(DecodeError::NonCanonicalInteger);
    }
    Ok(integer)
}
```

Use the decoder's real bounding helper, whatever its name and signature (`CLAUDE.md` calls it `bounded_count`). It must refuse a count whose bytes the remaining input cannot hold *before* `Vec::with_capacity`. Add `read_u64` to `Reader` if it is missing. The empty magnitude with `negative == false` is zero, which fits `i64`, so the last check refuses it too.

- The literal reader: `LITERAL_LARGE_INTEGER => Literal::LargeInteger(read_large_integer(reader)?)`. Tag 8 is now unknown and falls to the existing `InvalidTag` arm.
- `decode_value`: `VALUE_LARGE_INTEGER => { let integer = read_large_integer(reader)?; Ok(context.mutator.new_integer(integer)) }`.

  `decode_value` takes `context: &DecodeContext`, and `DecodeContext` holds `mutator: &mut Mutator`. Change `decode_value`, `decode_values` and their callers (`:2321`, `:2387`, `:2432-2434`, `:3135`, `:3164`, `:3274`, `:3396`) to take `&mut DecodeContext`. Where a caller holds another borrow of `context` across the call, read what it needs into a local first.

  Allocating here is sound: the decoder never polls a safepoint, so nothing moves the placeholders `tables` holds. The allocation is proportional to the input, 56 bytes plus the digits for every 14 or more bytes read, well inside the amplification figure `CLAUDE.md` gives.
- `DecodeError::NonCanonicalInteger`, with a `Display` of "an integer is not in its one canonical encoding", in the enum's existing style.

- [ ] **Step 5: `inspect.rs`.** `read_value`: `opcodes::VALUE_LARGE_INTEGER => format!("large-int {}", read_large_integer_text(reader)?)`, and the literal walk the same way. `read_large_integer_text` reads sign, count and digits with the same bound, and renders through `ArbitraryInteger::from_parts(…).to_decimal_string()`. `inspect` is a structural walk and does not refuse a non-canonical integer; the decoder does.

- [ ] **Step 6: Run the tests and the per-commit gate.** All pass, including `crates/ns-cli/tests/golden_round_trips.rs`, which round-trips every golden through a built binary. Task 5's and Task 6's goldens are the round trip's `LargeInteger` coverage.

- [ ] **Step 7: Commit** (Tasks 6 and 7 together, or Task 7 alone if Task 6 committed separately):

```bash
git add -A crates/ns-runtime goldens
git commit -m "feat: every integer literal is representable; images carry LargeInteger values"
```

---

### Task 8: the corpus and the CLI still agree

`crates/ns-cli` prints and disassembles literals and values. Its commands must know the new literal and must not name the removed one.

**Files:**
- Modify: whatever `cargo build --workspace` and `grep -rn "MalformedInteger\|malformed-int" crates editors` turn up in `crates/ns-cli` and `crates/ns-lsp`.
- Test: `crates/ns-cli/tests/*` as they name the literal.

- [ ] **Step 1:** Run `grep -rn "MalformedInteger\|malformed-int\|out of i64 range\|Integer overflow" crates editors goldens corpus/runnable.toml`. Every hit outside `docs/` must be gone or updated. A test expecting the `malformed-int` disassembly expects `large-int <decimal>` instead.
- [ ] **Step 2:** Run the per-commit gate, then `cargo run -p ns-cli -- disasm` on `goldens/arithmetic/large_integer_literals.ns` (see the `disasm` command's usage). The literals print as `large-int …`.
- [ ] **Step 3: Commit** if anything changed: `fix: the CLI names LargeInteger literals`.

---

### Task 9: the documentation describes the built state

Documentation describes the current state, not how it got there. Every sentence that says integers are `i64`, that overflow raises, or that large integers are "planned" is rewritten or removed.

**Files:**
- Modify: `docs/design/README.md`:
  - remove the deviation row at `:256`;
  - the "Builtins and the platform" row at `:44`: its "Still to do" column loses "Large integers with the eight-byte value", and its status names `LargeInteger`.
- Modify: `docs/design/core-library.md`:
  - "Deviation: `i64` integers" (`:653-664`) is removed;
  - "Planned: large integers" (`:666-729`) becomes a built section, "Large integers". It states:
    - the class;
    - the leaves;
    - the payload on today's arena (and that the object space moves it inline);
    - the algorithms taken, with their thresholds (schoolbook, half-Karatsuba, Karatsuba, Toom-3, the number-theoretic transform with two or three primes; algorithm D, Burnikel-Ziegler), and the one not taken (divide-and-conquer radix conversion);
    - the licensing arrangement as built (`THIRD-PARTY-NOTICES.md`, the headers).
  - the tower paragraph (`:786-806`) and `:2408-2413` in the present tense;
  - `//` is documented as rounding toward negative infinity, citing Primordial Soup.
- Modify: `docs/design/compiler.md`:
  - `:38` (the `Literal` list);
  - `:470-482`, where the fallback carries a promotion and a zero divisor, not "the overflow the specification requires";
  - `:520-527`, the `goldens/errors` mention of the `i64` overflow golden;
  - `:681-690`, which keeps `UnresolvedIdentifier` as the one deferred literal, and says `LargeInteger` is carried as digits.
- Modify: `docs/design/image-format.md`:
  - `:20`;
  - the literal-tag list (`:294-295`: 8 retired, 10 `LargeInteger`);
  - the value-tag list (9);
  - the builtin-class tag list (31);
  - "Planned: the image under the new representation", "Integers" (`:1158-1169`): what is now built (tags 9 and 10, the refusals) moves out of "Planned" into the format's description. What stays planned is the 61-bit decision in the decoder, which is the eight-byte plan's.
- Modify: `docs/design/object-model.md`:
  - `:18`;
  - "Planned: an eight-byte value", where "Integers become unbounded" and "Literals" now describe what exists and what the word changes (the boundary moves to 61 bits);
  - `:1472`, where the tags exist;
  - `:1896`, the decision list.
- Modify: `docs/design/memory.md` `:83`, `:1614`; `docs/design/execution.md` `:5716` (each names `MalformedInteger` retiring as future).
- Modify: `CLAUDE.md`:
  - "Compiling an already-resolved module can never raise a `NewspeakError`" cites `Literal::MalformedInteger`; cite `Literal::UnresolvedIdentifier` alone;
  - "seven reference implementations" becomes ten, with a sentence on the three `rust-num` crates: why they are there, and that `num-bigint` and `num-integer` are the source of adapted MIT code under `THIRD-PARTY-NOTICES.md`;
  - the `ns-runtime` crate description mentions `LargeInteger` among the kernel classes and `arbitrary_integer/`.
- Modify: `docs/OPEN-WORK.md`:
  - road step 2 splits into "large integers -- built" and the eight-byte value, still open;
  - add items for divide-and-conquer decimal conversion, and for `\\` (floor modulo), which `Kernel.ns` does not declare yet. Each gets where it is explained.
- Modify: `docs/PROGRESS.md`:
  - a milestone entry for large integers, with commit hashes;
  - "Where the next session starts" names the eight-byte value (road step 2's second plan).

- [ ] **Step 1:** `grep -rn "i64\|MalformedInteger\|overflow\|large integer\|LargeInteger\|Planned: large" CLAUDE.md docs/design docs/OPEN-WORK.md docs/PROGRESS.md README.md` and go through every hit.
- [ ] **Step 2:** Edit each document as listed. A fresh session must be able to start from `docs/design/README.md` and `PROGRESS.md` alone.
- [ ] **Step 3:** No test run: this commit changes only Markdown.
- [ ] **Step 4: Commit.** `docs: describe large integers as built`

---

### Task 10: the end-of-batch gates

- [ ] **Step 1:** Run the three stress modes together (`CLAUDE.md`, "once at the end of a batch of work"):

```bash
mkdir -p .temp && for mode in MOVES CACHES COLLECT; do
  (env NS_NEXT_STRESS_$mode=1 cargo nextest run --workspace --exclude ns-lsp      > .temp/stress-$mode.log 2>&1 || echo "stress $mode FAILED: .temp/stress-$mode.log") &
done; wait; grep -h Summary .temp/stress-*.log
```

Expected: three summaries with no failure. A `LargeInteger` allocated mid-leaf and then moved is exactly what `STRESS_MOVES` exercises.
- [ ] **Step 2:** `cargo nextest run -p ns-lsp`, which must pass (`ns-lsp` compiles against the resolver, unchanged here).
- [ ] **Step 3:** `cargo run -p ns-cli -- run-corpus corpus/modules` must report "ran 6, failed 0, skipped 205". Then the same under `NS_NEXT_STRESS_MOVES=1`, `NS_NEXT_STRESS_CACHES=1` and `NS_NEXT_STRESS_COLLECT=1`.
- [ ] **Step 4:** Run `cargo nextest run --workspace --release` once, for the release-only size tests (`Object` is still 56 bytes).
- [ ] **Step 5:** A release build of `bench_send.ns` and `bench_loop.ns` against the parent commit of Task 1, with the pinned `RUSTFLAGS` (`CLAUDE.md`, "Never judge performance from a debug build"). Several builds of each tree, pooled. The promotion lives off the fast path, so no change beyond the noise is expected. Record the numbers in the `PROGRESS.md` entry. If `IntegerBinary`'s `//` arm slowed (it now calls `checked_floor_divide`), check with `dumpbin /disasm` that the helper was inlined.
- [ ] **Step 6:** Fix anything a gate found in a commit of its own, and re-run that gate.
