# Are We Fast Yet, Stage 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Seven Are We Fast Yet benchmarks come into the tree as runnable Newspeak programs: Sieve, Permute, Queens, Towers, Bounce, Storage and List. They run under a harness that verifies them deterministically for the goldens and `ns-next bench`, and measures itself with `--timed`. The platform widens only as far as they need, in Primordial Soup's protocol.

**Architecture:**
- **The benchmarks.** Each one is ported from SOMns (`prior-art/somns/core-lib/Benchmarks`, MIT) into Primordial Soup's module shape: a module `class Sieve usingPlatform: platform = (…)` whose one public method `bench` runs the workload once, checks the result against AWFY's expected value and raises `Error signal:` when it differs. They send only protocol that Primordial Soup's `KernelForPrimordialSoup.ns` has.
- **The harness.** `AreWeFastYet.ns` is adapted from Primordial Soup's `BenchmarkRunner.ns` (Apache 2.0). It is a project root, `packageUsing: manifest`, holding the benchmarks as `manifest Sieve`.
  - Without `--timed` it runs each benchmark once and prints `Sieve: ok`.
  - With `--timed` it prints a score in runs per second, measured with `platform time Stopwatch` as Primordial Soup measures.
- **The kernel.** It gains:
  - `Number`'s `to:do:` and `to:by:do:` as Newspeak methods;
  - `Integer`'s and `LargeInteger`'s `\\`, `bitAnd:` and `asFloat` as VM-mirror leaves;
  - `Float`'s `<=`, `>=` and `asStringFixed:` as VM-mirror leaves.
- **The platform.** It gains `platform time`, a facet answering `Stopwatch`, over one new leaf that reads a monotonic clock.

**Tech Stack:** Rust 1.98.0, edition 2021; Newspeak in `crates/ns-runtime/core-library/` and `benchmarks/`; `cargo-nextest`; goldens under `goldens/`. No new dependency.

**Spec:** the design settled with the user on 2026-09-30, recorded in this plan. The background is in:
- `docs/OPEN-WORK.md`, "Before step 2's second plan, the Are We Fast Yet benchmarks" (stage 1);
- `docs/design/image-format.md`, "Packaging";
- `docs/design/core-library.md`, "The platform" and "Large integers".

The decisions, each the user's:
1. **Protocol.** The benchmarks follow Primordial Soup's protocol, not SOMns's. No `Object>>value`, no `&&`, no `%`, no `<>`, no `downTo:do:`, no value-evaluating `new:withAll:`, no `error:`. The ports are rewritten wherever SOMns sends those.
2. **Module shape.** Every benchmark file is a Primordial Soup-style module, `usingPlatform:` with `bench`. There is no `andHarness:` and no `Benchmark` superclass. The harness follows `BenchmarkRunner.ns`.
3. **Measuring.** Both ways:
   - Without `--timed` the harness is deterministic. The goldens pin it and `ns-next bench` times it.
   - With `--timed` it measures itself with `Stopwatch` as Primordial Soup does.
4. **No `Object>>out`.** It would hand stdout to every object without the platform. The harness writes through `platform transcript`.
5. **Where the programs live.** `benchmarks/are-we-fast-yet/` holds them. The goldens under `goldens/awfy/` point at them.
6. **What stays out.**
   - `Manifest at:ifAbsent:` is not needed: the harness selects by `name`.
   - `platform kernel Array` is not needed: `Array` is `Object`'s accessor.
   - A program's exit status is a separate item in `OPEN-WORK.md`, not part of this plan. The recommendation is an explicit `platform system exit:`; that is not yet decided.

## Global Constraints

- **The invariants.** Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - The VM mirror is a capability. The new leaves are reached only from `Kernel.ns` and `PlatformFactory.ns`, which are resolved with `resolve_kernel`. None of its six locks changes.
  - `vm.rs` gets no dispatch logic. Nothing in this plan touches `vm.rs` or the compiler.
  - No wire tag is added, renumbered or retired. A mirror row has no wire tag, and `\\` does **not** join `IntegerOperation`.
- **Primordial Soup's protocol.** Every method the kernel or platform gains exists in Primordial Soup's `KernelForPrimordialSoup.ns` or `Time.ns`, with the same selector and meaning. Where our body differs, a comment says why.
- **No carve-outs.**
  - Every integer leaf handles both representations, and a `SmallInteger` meeting a `LargeInteger` in either position.
  - `to:do:` works with a `Float` receiver and across the `SmallInteger`/`LargeInteger` boundary.
- **Leaf errors.**
  - A leaf declines (`None`) only for a receiver of the wrong shape, as every number leaf does today.
  - A wrong argument raises `type_mismatch`, a zero divisor raises `division by zero`, and an out-of-range count raises a message of its own.
- **Names are written out, in Newspeak too** (`CLAUDE.md`, "Conventions"). The ports rename SOMns's `i`, `k`, `n`, `v`, `r`, `c`, `x`, `y`, `xVel`, `tmp`, `val` and Primordial Soup's `p`.
- **Comments.**
  - A comment explains the decision, not the mechanics.
  - A golden's rationale is a `(* … *)` comment under its category string. The `.toml` holds only the recorded answer.
- **Writing `.ns` files.** Write them with the editor's Write/Edit tool, never through a shell heredoc or `sed`. The shell here swallows one backslash of `\\`, which silently turns `\\` into `\`.
- **License headers.**
  - Every file under `benchmarks/are-we-fast-yet/` carries a header naming its upstream file and commit, what was changed, and its licence text, as specified in Task 6.
  - `THIRD-PARTY-NOTICES.md` gains a SOMns entry (MIT) and a Primordial Soup entry (Apache 2.0).
- **Commits.** Conventional commits. Every commit ends with these lines, and a subagent's commit names the subagent's own model:
  ```
  Co-Authored-By: <the model that actually wrote the code> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01XkuJBQierVYxAMXpiaz4ty
  ```
- **Per-commit gate** (`CLAUDE.md`, "Working in this repository"):
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets` (warning-free)
  - `cargo nextest run --workspace --exclude ns-lsp`
  - `cargo test --workspace --exclude ns-lsp --doc`
  - `cargo run -p ns-cli -- parse-corpus corpus/modules`, which must report "parsed 211, failed 0"

  The end-of-batch gates (three stress modes, `run-corpus` plain and under stress, the `ns-lsp` suite) run once, in Task 10. That staggering is deliberate: a commit before Task 10 is work in progress. The JetBrains plugin and `ns-syntax` are untouched.
- **Goldens.** A golden that changes is a behaviour change. No existing golden's output should change in this plan; if one does, stop and report it.

## Review Focus

- **The tie in `asStringFixed:`.** Expected results, following Primordial Soup's double-conversion, which rounds a tie up on the exact binary value:
  - `0.125 asStringFixed: 2` is `'0.13'`;
  - `2.5 asStringFixed: 0` is `'3'`;
  - `-0.125 asStringFixed: 2` is `'-0.13'`;
  - `1.005 asStringFixed: 2` is `'1.00'` (1.005 is below the tie in binary).

  Rust's `format!("{:.2}", 0.125)` answers `0.12` (ties to even), so a leaf that calls the formatter directly is wrong. Pinned in Task 4.
- **Negative zero, and a negative that rounds to zero.** `-0.0 asStringFixed: 1` and `-0.04 asStringFixed: 1` both answer `'-0.0'`, as double-conversion does: the sign is the value's, not the rounded digits'. Pinned in Task 4.
- **`i64::MIN` in the new integer leaves.**
  - `-9223372036854775808 \\ -1` is `0`. Rust's `%` panics on it.
  - `-9223372036854775808 bitAnd: -1` is `-9223372036854775808`.
  - `-9223372036854775808 asFloat` is exact.

  Pinned in Tasks 2 and 3.
- **`bitAnd:` with a negative `LargeInteger`.** Two's complement extends a negative number's sign bits without end:
  - `(0 - 18446744073709551616) bitAnd: 18446744073709551615` is `0`;
  - `-1 bitAnd: 18446744073709551616` is `18446744073709551616`.

  A sign-and-magnitude `and` gets both wrong. Pinned in Task 3 against `i128`.
- **The harness's arguments.**
  - `-- Nope` is an uncaught `Unknown benchmark: Nope`.
  - `-- --timed Sieve extra` is an uncaught usage error.
  - `-- --timed` with no name times all seven.

  The two errors exit 1 with their message on stderr, never with a `doesNotUnderstand:` from inside the harness. The usage golden passes `--timed`, so both branches of the argument parsing run. Pinned in Tasks 6 and 8.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ns-runtime/core-library/Kernel.ns` | `Number`'s `to:do:`, `to:by:do:`; `Integer`'s and `LargeInteger`'s `\\`, `bitAnd:`, `asFloat` forwarders; `Float`'s `<=`, `>=`, `asStringFixed:` forwarders |
| `crates/ns-runtime/src/builtins/primitive.rs` | the new `MIRROR` rows and enum variants, classified in `answers_false_to_a_stranger` |
| `crates/ns-runtime/src/builtins/numbers.rs` | the new integer and float leaves; `floor_modulo`, `fixed_text`; `division_by_zero` takes its selector |
| `crates/ns-runtime/src/arbitrary_integer/mod.rs` | `ArbitraryInteger::floor_modulo`, `ArbitraryInteger::bit_and` |
| `crates/ns-runtime/core-library/PlatformFactory.ns` | `lazy public time`, `Time`, `Stopwatch`, their class accessors |
| `crates/ns-runtime/src/platform.rs`, `builtins/primitive.rs` | `PlatformPrimitive::MonotonicNanoseconds` and its row |
| `crates/ns-runtime/tests/platform.rs`, `tests/image.rs`, `src/interpreter/kernel.rs` (tests) | the platform's shape, paths and fast paths, widened by `time`, `Time`, `Stopwatch` |
| `benchmarks/are-we-fast-yet/*.ns` (new) | the harness and the seven benchmarks |
| `benchmarks/are-we-fast-yet/are-we-fast-yet.json` (new) | the project that runs, builds and benches them |
| `goldens/awfy/*.json`, `*.toml` (new) | the harness verified under both lowerings and through `build`/`load` |
| `goldens/kernel/`, `goldens/arithmetic/`, `goldens/errors/` | goldens for each new kernel method |
| `crates/ns-cli/tests/run.rs` | `--timed`'s output shape |
| `THIRD-PARTY-NOTICES.md` | SOMns (MIT) and Primordial Soup (Apache 2.0) |
| `docs/…`, `CLAUDE.md` | Task 9 |

---

### Task 1: `Number` iterates, and `Float` compares with `<=` and `>=`

`to:do:` and `to:by:do:` are Newspeak methods on `Number`, after Primordial Soup's (`KernelForPrimordialSoup.ns:2106-2124`). Their loop condition sends `<=` and `>=`. A `Float` today has only `<`, `>` and `=`, so `1.5 to: 3 do:` would fail on its first test. `Float` therefore gains the two comparisons as leaves (Primordial Soup has them on `Float`). The compiler does not inline `to:do:` in this plan, which OPEN-WORK records in Task 9. A `SmallInteger` receiver with a `Float` bound stays the mixed-arithmetic gap that stage 2 closes (`OPEN-WORK.md`, stage 2).

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`class Number`, `class Float`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`FloatPrimitive`, `answers_false_to_a_stranger`'s Float arm, two `MIRROR` rows appended at the end of the table)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs` (`execute_float`)
- Create: `goldens/kernel/number_iteration.ns`, `.toml`
- Create: `goldens/arithmetic/float_orders_with_less_or_equal.ns`, `.toml`

**Interfaces:**
- Produces:
  - `Number>>to:do:`, `Number>>to:by:do:`, both answering `nil`;
  - `Float>><=`, `Float>>>=`;
  - `FloatPrimitive::LessOrEqual`, `FloatPrimitive::GreaterOrEqual`;
  - mirror selectors `float:lessOrEqual:ifFail:` and `float:greaterOrEqual:ifFail:`.

- [ ] **Step 1: Write the two goldens.**

`goldens/kernel/number_iteration.ns`:
```newspeak
Newspeak3
'Kernel'
(* `to:do:` and `to:by:do:` are `Number`'s, after Primordial Soup's: the
   bound is inclusive, a negative step counts down, an empty range runs
   nothing, and both answer nil. A `Float` receiver counts in `Float`s, and
   a range that crosses into `LargeInteger` keeps counting. *)
class NumberIteration = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. |
        1 to: 3 do: [:index | transcript print: index; newLine].
        5 to: 1 by: -2 do: [:index | transcript print: index; newLine].
        3 to: 1 do: [:index | transcript show: 'never'; newLine].
        transcript print: (1 to: 0 do: [:index | index]); newLine.
        transcript print: (1 to: 0 by: 1 do: [:index | index]); newLine.
        1.5 to: 3 do: [:value | transcript print: value; newLine].
        9223372036854775806 to: 9223372036854775808 do: [:index | transcript print: index; newLine]
    )
)
```
`goldens/kernel/number_iteration.toml`:
```toml
stdout = '''
1
2
3
5
3
1
nil
nil
1.5
2.5
9223372036854775806
9223372036854775807
9223372036854775808
'''
```
`goldens/arithmetic/float_orders_with_less_or_equal.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* `Float`'s `<=` and `>=`, which Primordial Soup's `Float` has and
   `Number`'s `to:do:` sends: an `Integer` or `LargeInteger` argument is
   widened to the nearest double as for `<`, and a NaN is ordered with
   nothing. *)
class FloatOrders = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. nan = 0.0 / 0.0. |
        transcript print: 1.5 <= 2; newLine.
        transcript print: 2.0 <= 2; newLine.
        transcript print: 2.5 <= 2; newLine.
        transcript print: 2.0 >= 2; newLine.
        transcript print: 1.5 >= 9223372036854775808; newLine.
        transcript print: nan <= 1.0; newLine.
        transcript print: nan >= 1.0; newLine
    )
)
```
`goldens/arithmetic/float_orders_with_less_or_equal.toml`:
```toml
stdout = '''
true
true
false
true
false
false
false
'''
```

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-cli --test run_goldens number_iteration float_orders`
Expected: FAIL. The status is 1 with `does not understand #to:do:`, then `#<=`.

- [ ] **Step 3: Add the leaves.**

In `primitive.rs`, add `LessOrEqual` and `GreaterOrEqual` to `FloatPrimitive` after `Greater`. Add both to the `=> false` group of the Float arm in `answers_false_to_a_stranger`. Append two rows at the end of `MIRROR`, after the last existing row:
```rust
    mirror(
        "float:lessOrEqual:ifFail:",
        "<=",
        Primitive::Float(FloatPrimitive::LessOrEqual),
    ),
    mirror(
        "float:greaterOrEqual:ifFail:",
        ">=",
        Primitive::Float(FloatPrimitive::GreaterOrEqual),
    ),
```
In `numbers.rs`, `execute_float`, after `FloatPrimitive::Greater`:
```rust
        FloatPrimitive::LessOrEqual => Some(compare("<=", |left, right| left <= right)),
        FloatPrimitive::GreaterOrEqual => Some(compare(">=", |left, right| left >= right)),
```

- [ ] **Step 4: Add the Newspeak methods.**

In `Kernel.ns`, `class Float`, after `public > other`:
```newspeak
        public <= other = ( ^vmMirror float: self lessOrEqual: other ifFail: [^super <= other] )
        public >= other = ( ^vmMirror float: self greaterOrEqual: other ifFail: [^super >= other] )
```
Replace `class Number`'s body and its comment. The existing comment says it is empty "until there is a body to move". Keep its first sentence and replace the rest:
```newspeak
    (* §3.1.1 names `Number` among the classes whose instances are values,
       and §5.1.1's numeric literals are its instances. Iteration is here,
       as in Primordial Soup's `Number`, so an `Integer`, a `LargeInteger`
       and a `Float` receiver share one body. Primordial Soup's compiler
       inlines both; ours sends them. The increment is `current + step`
       rather than Primordial Soup's `step + current`, because an `Integer`
       receiver meeting a `Float` argument is not yet arithmetic here and a
       `Float` receiver must still count. *)
    public class Number = Value new () (
        public to: stop do: action = (
            | current |
            current:: self.
            [current <= stop] whileTrue: [
                action value: current.
                current:: current + 1].
            ^nil
        )
        public to: stop by: step do: action = (
            | current |
            current:: self.
            step < 0
                ifTrue: [[current >= stop] whileTrue: [
                    action value: current.
                    current:: current + step]]
                ifFalse: [[current <= stop] whileTrue: [
                    action value: current.
                    current:: current + step]].
            ^nil
        )
    )
```

- [ ] **Step 5: Run the goldens and the suite.**

Run: `cargo nextest run -p ns-cli --test run_goldens number_iteration float_orders`, then the full per-commit gate.
Expected: PASS.

The kernel tests that pin fast paths or member lists may now name the new `Float` members, for example `interpreter/kernel.rs`'s fast-path tests. If one fails, extend its list with the new selectors, and nothing else.

- [ ] **Step 6: Commit.**

```bash
git add crates/ns-runtime/core-library/Kernel.ns crates/ns-runtime/src/builtins/primitive.rs crates/ns-runtime/src/builtins/numbers.rs goldens/kernel/number_iteration.* goldens/arithmetic/float_orders_with_less_or_equal.*
git commit -m "feat: Number iterates with to:do: and to:by:do:, Float orders with <= and >="
```

---

### Task 2: `\\`, the remainder of floor division

`\\` answers the remainder of `//`, so it takes the divisor's sign: `(a // b) * b + (a \\ b) = a` (Primordial Soup's `Integer>>\\`, "Remainder of integer division rounded toward negative infinity"). A zero divisor raises `division by zero`, as `//` does, naming `\\`.

**Files:**
- Modify: `crates/ns-runtime/src/arbitrary_integer/mod.rs` (`floor_modulo`, and a unit test)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs`:
  - `floor_modulo`;
  - `division_by_zero` takes its selector;
  - the two new arms;
  - a unit test.
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`IntegerPrimitive::Modulo`, `LargeIntegerPrimitive::Modulo`, two rows)
- Modify: `crates/ns-runtime/core-library/Kernel.ns`
- Create: `goldens/arithmetic/floor_modulo_takes_the_divisors_sign.ns`, `.toml`
- Create: `goldens/errors/floor_modulo_by_zero_is_uncaught.ns`, `.toml`

**Interfaces:**
- Produces:
  - `ArbitraryInteger::floor_modulo(&self, divisor: &ArbitraryInteger) -> Option<ArbitraryInteger>`, which is `None` for a zero divisor;
  - `numbers::floor_modulo(dividend: i64, divisor: i64) -> i64`, whose `divisor` must not be zero;
  - `division_by_zero(receiver: Value<'gc>, selector: &str)`;
  - `Integer>>\\` and `LargeInteger>>\\`.

- [ ] **Step 1: Write the failing unit tests.**

In `arbitrary_integer/mod.rs`'s test module:
```rust
    #[test]
    fn floor_modulo_takes_the_divisors_sign_and_agrees_with_i128() {
        let samples: [i128; 12] = [
            0,
            1,
            -1,
            7,
            -7,
            i128::from(i64::MAX),
            i128::from(i64::MIN),
            i128::from(i64::MIN) - 1,
            1 << 64,
            -(1 << 64),
            (1 << 100) + 12345,
            -(1 << 100) - 12345,
        ];
        for dividend in samples {
            for divisor in samples {
                let answer = ArbitraryInteger::from_i128(dividend)
                    .floor_modulo(&ArbitraryInteger::from_i128(divisor));
                if divisor == 0 {
                    assert!(answer.is_none());
                    continue;
                }
                let remainder = dividend % divisor;
                let expected = if remainder != 0 && ((remainder < 0) != (divisor < 0)) {
                    remainder + divisor
                } else {
                    remainder
                };
                assert_eq!(
                    answer,
                    Some(ArbitraryInteger::from_i128(expected)),
                    "{dividend} \\\\ {divisor}"
                );
            }
        }
    }
```
If `ArbitraryInteger` has no `PartialEq`/`Debug` derive, compare `to_decimal_string()` instead. Check the struct's derives first.

In `numbers.rs`'s test module:
```rust
    #[test]
    fn floor_modulo_takes_the_divisors_sign() {
        assert_eq!(super::floor_modulo(7, 2), 1);
        assert_eq!(super::floor_modulo(-7, 2), 1);
        assert_eq!(super::floor_modulo(7, -2), -1);
        assert_eq!(super::floor_modulo(-7, -2), -1);
        assert_eq!(super::floor_modulo(6, 3), 0);
        assert_eq!(super::floor_modulo(i64::MIN, -1), 0);
        assert_eq!(super::floor_modulo(i64::MIN, i64::MAX), i64::MAX - 1);
    }
```

- [ ] **Step 2: Run them and see them fail to compile.**

Run: `cargo nextest run -p ns-runtime floor_modulo`
Expected: FAIL, because `floor_modulo` is not found.

- [ ] **Step 3: Implement both.**

In `arbitrary_integer/mod.rs`, after `floor_divide`:
```rust
    /// The remainder of `floor_divide`, which takes the divisor's sign;
    /// `None` for a zero divisor. Derived from the quotient rather than
    /// adapted from `num-integer`'s `mod_floor`, so the two can never
    /// disagree.
    pub fn floor_modulo(&self, divisor: &ArbitraryInteger) -> Option<ArbitraryInteger> {
        let quotient = self.floor_divide(divisor)?;
        Some(self.subtract(&quotient.multiply(divisor)))
    }
```
In `numbers.rs`, after `checked_floor_divide`:
```rust
/// `\\` on two `i64`s: the remainder of `checked_floor_divide`, which takes
/// the divisor's sign. `divisor` is not zero. `wrapping_rem` answers `0` for
/// `i64::MIN \\ -1`, the true remainder, where `%` would overflow; the
/// correction cannot overflow, because it adds operands of opposite signs.
pub(crate) fn floor_modulo(dividend: i64, divisor: i64) -> i64 {
    let remainder = dividend.wrapping_rem(divisor);
    if remainder != 0 && ((remainder < 0) != (divisor < 0)) {
        remainder + divisor
    } else {
        remainder
    }
}
```
Change `division_by_zero` to take the selector the leaf answers for, and pass `"//"` at its existing call sites:
```rust
/// Division by zero, raised in the same words by `Integer`'s and
/// `LargeInteger`'s `//` and `\\`, naming the selector sent.
fn division_by_zero<'gc>(receiver: Value<'gc>, selector: &str) -> NewspeakError<'gc> {
    NewspeakError {
        selector: Symbol::intern(selector),
        receiver,
        message: "division by zero".to_string(),
    }
}
```

- [ ] **Step 4: Run the unit tests.**

Run: `cargo nextest run -p ns-runtime floor_modulo`
Expected: PASS.

- [ ] **Step 5: Write the goldens.**

`goldens/arithmetic/floor_modulo_takes_the_divisors_sign.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* `\\` is the remainder of `//`, so it takes the divisor's sign and
   `(a // b) * b + (a \\ b) = a` holds for every pair, as Primordial Soup's
   `Integer>>\\` says ("Remainder of integer division rounded toward
   negative infinity"). The `LargeInteger` rows cross the boundary in both
   positions. *)
class FloorModulo = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          large = 9223372036854775808.
          pairs = {
            {7. 2}. {-7. 2}. {7. -2}. {-7. -2}. {6. 3}.
            {-9223372036854775808. -1}.
            {large. 10}. {large negated - 1. 10}.
            {7. large}. {-7. large}. {large * large + 5. large negated} }. |
        pairs do: [:pair |
            | dividend divisor |
            dividend:: pair at: 1.
            divisor:: pair at: 2.
            transcript
                print: dividend \\ divisor;
                show: ' ';
                print: (dividend // divisor) * divisor + (dividend \\ divisor) = dividend;
                newLine]
    )
)
```
`goldens/arithmetic/floor_modulo_takes_the_divisors_sign.toml`:
```toml
stdout = '''
1 true
1 true
-1 true
-1 true
0 true
0 true
8 true
1 true
7 true
9223372036854775801 true
-9223372036854775803 true
'''
```
Check the last row before recording it:
- `large * large + 5` is 2^126 + 5, and its divisor is −2^63.
- 2^126 is an exact multiple of 2^63, so the floor quotient is −2^63 − 1.
- The remainder is then 5 − 2^63 = −9223372036854775803.

Compute it with Python (`(2**126 + 5) % -(2**63)`, since Python's `%` is floor modulo) and record what Python answers.

`goldens/errors/floor_modulo_by_zero_is_uncaught.ns`:
```newspeak
Newspeak3
'Errors'
(* `\\` refuses a zero divisor with the "division by zero" `//` raises,
   for either representation of the receiver; the first raise ends the
   program. *)
class FloorModuloByZero = () (
    public main: platform args: arguments = (
        platform transcript print: 7 \\ 0; newLine
    )
)
```
`goldens/errors/floor_modulo_by_zero_is_uncaught.toml`:
```toml
status = 1
stderr = '''
Uncaught Newspeak error: division by zero
'''
```

- [ ] **Step 6: Add the leaves and forwarders.**

In `primitive.rs`:
- Add `Modulo` after `FloorDivide` to both `IntegerPrimitive` and `LargeIntegerPrimitive`, and put each in the `=> false` group of its arm.
- Append these rows:
```rust
    mirror(
        "integer:modulo:ifFail:",
        "\\\\",
        Primitive::Integer(IntegerPrimitive::Modulo),
    ),
    mirror(
        "largeInteger:modulo:ifFail:",
        "\\\\",
        Primitive::LargeInteger(LargeIntegerPrimitive::Modulo),
    ),
```
In `numbers.rs`, `execute_integer`, after `FloorDivide`:
```rust
        IntegerPrimitive::Modulo => Some(match argument_integer() {
            Some(0) => Err(division_by_zero(receiver, "\\\\")),
            Some(divisor) => Ok(Value::SmallInteger(floor_modulo(receiver_integer, divisor))),
            None => match large_argument() {
                Some(divisor) => {
                    match ArbitraryInteger::from_i64(receiver_integer).floor_modulo(&divisor) {
                        Some(remainder) => Ok(mutator.new_integer(remainder)),
                        None => Err(division_by_zero(receiver, "\\\\")),
                    }
                }
                None => Err(mismatch("\\\\")),
            },
        }),
```
In `execute_large_integer`, after `FloorDivide`:
```rust
        LargeIntegerPrimitive::Modulo => Some(match argument() {
            Some(divisor) => match receiver_integer.floor_modulo(&divisor) {
                Some(remainder) => Ok(mutator.new_integer(remainder)),
                None => Err(division_by_zero(receiver, "\\\\")),
            },
            None => Err(mismatch("\\\\")),
        }),
```
In `Kernel.ns`, write the forwarder after `//` in `Integer`:
```newspeak
        public \\ other = ( ^vmMirror integer: self modulo: other ifFail: [^super \\ other] )
```
and after `//` in `LargeInteger`:
```newspeak
        public \\ other = ( ^vmMirror largeInteger: self modulo: other ifFail: [^super \\ other] )
```

- [ ] **Step 7: Run the goldens and the per-commit gate.**

Run: `cargo nextest run -p ns-cli --test run_goldens floor_modulo`, then the full gate.
Expected: PASS.

- [ ] **Step 8: Commit.**

```bash
git commit -m "feat: Integer and LargeInteger answer \\\\, the remainder of floor division"
```
Stage exactly the files listed under **Files** above.

---

### Task 3: `bitAnd:` over two's complement

`bitAnd:` is Primordial Soup's `Integer>>bitAnd:`. It is defined over the two's-complement representation, in which a negative number has infinitely many leading one bits. Two `SmallInteger`s are exact with `i64`'s `&`, because an `i64` is two's complement with its sign extended. A `LargeInteger` in either position goes through `ArbitraryInteger::bit_and`.

**Files:**
- Modify: `crates/ns-runtime/src/arbitrary_integer/mod.rs` (`bit_and`, `twos_complement_digits`, a unit test)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs`, `primitive.rs`, `core-library/Kernel.ns`
- Create: `goldens/arithmetic/bit_and_is_twos_complement.ns`, `.toml`

**Interfaces:**
- Produces:
  - `ArbitraryInteger::bit_and(&self, other: &ArbitraryInteger) -> ArbitraryInteger`;
  - `IntegerPrimitive::BitAnd`, `LargeIntegerPrimitive::BitAnd`;
  - `Integer>>bitAnd:`, `LargeInteger>>bitAnd:`.

- [ ] **Step 1: Write the failing unit test**, in `arbitrary_integer/mod.rs`'s tests. `i128`'s `&` is the oracle, since `i128` is two's complement:
```rust
    #[test]
    fn bit_and_is_twos_complement_and_agrees_with_i128() {
        let samples: [i128; 12] = [
            0,
            1,
            -1,
            65535,
            -65536,
            i128::from(i64::MAX),
            i128::from(i64::MIN),
            (1 << 64) - 1,
            1 << 64,
            -(1 << 64),
            -(1 << 64) - 1,
            -(1 << 100) + 12345,
        ];
        for left in samples {
            for right in samples {
                assert_eq!(
                    ArbitraryInteger::from_i128(left)
                        .bit_and(&ArbitraryInteger::from_i128(right))
                        .to_decimal_string(),
                    (left & right).to_string(),
                    "{left} bitAnd: {right}"
                );
            }
        }
    }
```

- [ ] **Step 2: Run it and see it fail to compile.**

Run: `cargo nextest run -p ns-runtime bit_and`
Expected: FAIL, because `bit_and` is not found.

- [ ] **Step 3: Implement it**, after `floor_modulo`:
```rust
    /// Bitwise and over two's complement, where a negative number's sign
    /// bits extend without end (Primordial Soup's and Squeak's `bitAnd:`).
    /// One digit beyond the longer magnitude holds the sign, so the top
    /// bit of the result's top digit says whether it is negative.
    pub fn bit_and(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        let length = self.magnitude.len().max(other.magnitude.len()) + 1;
        let digits: Vec<u64> = self
            .twos_complement_digits(length)
            .iter()
            .zip(other.twos_complement_digits(length))
            .map(|(left_digit, right_digit)| left_digit & right_digit)
            .collect();
        if digits[length - 1] >> (u64::BITS - 1) == 1 {
            // A negative result's magnitude is its complement plus one.
            let complement = digits.iter().map(|digit| !digit).collect();
            ArbitraryInteger::from_parts(false, complement)
                .add(&ArbitraryInteger::from_i64(1))
                .negated()
        } else {
            ArbitraryInteger::from_parts(false, digits)
        }
    }

    /// `length` digits of the two's-complement representation, least
    /// significant first: a negative `-m` is the complement of `m - 1`.
    fn twos_complement_digits(&self, length: usize) -> Vec<u64> {
        if self.negative {
            let decremented = self.absolute().subtract(&ArbitraryInteger::from_i64(1));
            let mut digits = decremented.magnitude().to_vec();
            digits.resize(length, 0);
            digits.iter().map(|digit| !digit).collect()
        } else {
            let mut digits = self.magnitude.to_vec();
            digits.resize(length, 0);
            digits
        }
    }
```

- [ ] **Step 4: Run the unit test.**

Run: `cargo nextest run -p ns-runtime bit_and`
Expected: PASS.

- [ ] **Step 5: Write the golden.**

`goldens/arithmetic/bit_and_is_twos_complement.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* `bitAnd:` is Primordial Soup's: an and over two's complement, where a
   negative number's sign bits extend without end, for either
   representation in either position. Bounce's and Storage's `Random`
   masks with it. *)
class BitAnd = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          two64 = 18446744073709551616.
          pairs = {
            {12. 10}. {-1. 255}. {-256. 255}. {74755 * 1309 + 13849. 65535}.
            {-9223372036854775808. -1}.
            {two64 negated. two64 - 1}. {-1. two64}. {two64 + 5. -2}.
            {two64 negated - 1. two64 negated - 1} }. |
        pairs do: [:pair |
            transcript print: ((pair at: 1) bitAnd: (pair at: 2)); newLine]
    )
)
```
`goldens/arithmetic/bit_and_is_twos_complement.toml`: compute each line with Python `a & b` (Python's `&` on ints is two's complement with infinite sign extension) before recording it:
```toml
stdout = '''
8
255
0
22896
-9223372036854775808
0
18446744073709551616
18446744073709551620
-18446744073709551617
'''
```
Check the fourth line with Python: `(74755*1309+13849) & 65535`. Record Python's answer, not this plan's, wherever they differ.

- [ ] **Step 6: Add the leaves and forwarders.**

In `primitive.rs`:
- Add `BitAnd` after `Modulo` to `IntegerPrimitive` and `LargeIntegerPrimitive`, and put each in its `=> false` group.
- Append these rows:
```rust
    mirror(
        "integer:bitAnd:ifFail:",
        "bitAnd:",
        Primitive::Integer(IntegerPrimitive::BitAnd),
    ),
    mirror(
        "largeInteger:bitAnd:ifFail:",
        "bitAnd:",
        Primitive::LargeInteger(LargeIntegerPrimitive::BitAnd),
    ),
```
In `numbers.rs`, `execute_integer`:
```rust
        IntegerPrimitive::BitAnd => Some(match argument_integer() {
            Some(argument) => Ok(Value::SmallInteger(receiver_integer & argument)),
            None => match large_argument() {
                Some(argument) => Ok(mutator
                    .new_integer(ArbitraryInteger::from_i64(receiver_integer).bit_and(&argument))),
                None => Err(mismatch("bitAnd:")),
            },
        }),
```
In `execute_large_integer`:
```rust
        LargeIntegerPrimitive::BitAnd => Some(match argument() {
            Some(argument) => Ok(mutator.new_integer(receiver_integer.bit_and(&argument))),
            None => Err(mismatch("bitAnd:")),
        }),
```
In `Kernel.ns`, write the forwarder in `Integer`:
```newspeak
        public bitAnd: other = ( ^vmMirror integer: self bitAnd: other ifFail: [^super bitAnd: other] )
```
and in `LargeInteger`:
```newspeak
        public bitAnd: other = ( ^vmMirror largeInteger: self bitAnd: other ifFail: [^super bitAnd: other] )
```

- [ ] **Step 7: Run the golden and the per-commit gate.**

Expected: PASS.

- [ ] **Step 8: Commit.**

```bash
git commit -m "feat: Integer and LargeInteger answer bitAnd: over two's complement"
```

---

### Task 4: `asFloat` and `asStringFixed:`

These two are what the harness's score needs. The score is `(runs * 1000000000) asFloat / elapsed asFloat`, printed with `asStringFixed: 1`.
- **`asFloat`** answers the nearest double, ties to even (`as f64` for an `i64`, `ArbitraryInteger::to_f64` for a `LargeInteger`, infinite beyond `f64::MAX`).
- **`asStringFixed:`** follows Primordial Soup's `Float>>asStringFixed:` (primitive 55, `vm/double_conversion.cc:52`):
  - It prints 0 to 20 fraction digits, rounding a tie **up** on the exact binary value, as double-conversion's `FastFixedDtoa` does (`double-conversion/fixed-dtoa.cc:262-265`).
  - The sign is the value's, so `-0.04` prints `-0.0`.
  - Outside ±1e21, and for a NaN or an infinity, Primordial Soup prints the shortest representation; ours prints what `printString` prints.
  - A count outside 0..20 raises.

**Files:**
- Modify: `crates/ns-runtime/src/builtins/numbers.rs`:
  - `fixed_text` and its unit test;
  - `IntegerPrimitive::AsFloat`, `LargeIntegerPrimitive::AsFloat`, `FloatPrimitive::AsStringFixed` arms.
- Modify: `primitive.rs`, `Kernel.ns`
- Create: `goldens/arithmetic/as_float_and_as_string_fixed.ns`, `.toml`
- Create: `goldens/errors/as_string_fixed_refuses_more_than_twenty_digits.ns`, `.toml`

**Interfaces:**
- Produces:
  - `numbers::fixed_text(value: f64, fraction_digits: usize) -> String`, where `fraction_digits` is at most 20;
  - `Integer>>asFloat`, `LargeInteger>>asFloat`, `Float>>asStringFixed:`.

- [ ] **Step 1: Write the failing unit test**, in `numbers.rs`'s tests:
```rust
    #[test]
    fn fixed_text_rounds_a_tie_up_on_the_exact_binary_value() {
        assert_eq!(super::fixed_text(0.125, 2), "0.13");
        assert_eq!(super::fixed_text(-0.125, 2), "-0.13");
        assert_eq!(super::fixed_text(2.5, 0), "3");
        assert_eq!(super::fixed_text(0.5, 0), "1");
        assert_eq!(super::fixed_text(1.005, 2), "1.00");
        assert_eq!(super::fixed_text(9.995, 2), "9.99");
        assert_eq!(super::fixed_text(99.95, 1), "100.0");
        assert_eq!(super::fixed_text(123.456, 1), "123.5");
        assert_eq!(super::fixed_text(1.0, 3), "1.000");
        assert_eq!(super::fixed_text(-0.0, 1), "-0.0");
        assert_eq!(super::fixed_text(-0.04, 1), "-0.0");
        assert_eq!(super::fixed_text(0.1, 20), "0.10000000000000000555");
        assert_eq!(super::fixed_text(1e21, 2), (1e21f64).to_string());
        assert_eq!(super::fixed_text(f64::NAN, 2), "NaN");
        assert_eq!(super::fixed_text(f64::NEG_INFINITY, 2), f64::NEG_INFINITY.to_string());
    }
```
Check the `9.995` and `99.95` expectations before relying on them. `9.995` is 9.99499999999999992894572642399… in binary, so it must round down. `99.95` is 99.9500000000000028421709430404…, so it must round up. Print `format!("{:.40}", value)` to see the exact expansions, and correct the expectation if the expansion says otherwise.

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo nextest run -p ns-runtime fixed_text`
Expected: FAIL, because `fixed_text` is not found.

- [ ] **Step 3: Implement `fixed_text`**, after `float_hash`:
```rust
/// `Float>>asStringFixed:`'s text, as Primordial Soup's double-conversion
/// writes it: `fraction_digits` digits after the point, a tie rounded up on
/// the exact binary value (Rust's formatter would round it to even), and
/// the value's own sign even when the digits round to zero. Outside ±1e21,
/// and for a NaN or an infinity, the text `printString` answers.
pub(crate) fn fixed_text(value: f64, fraction_digits: usize) -> String {
    if !(value > -1e21 && value < 1e21) {
        return value.to_string();
    }
    // Every finite double has an exact decimal expansion of at most 1074
    // fraction digits, so this text is the value itself, not a rounding.
    let exact = format!("{:.1074}", value.abs());
    let (integer_text, fraction_text) = exact
        .split_once('.')
        .expect("a fixed-precision format has a point");
    let mut digits: Vec<u8> = integer_text
        .bytes()
        .chain(fraction_text.bytes().take(fraction_digits))
        .collect();
    if fraction_text.as_bytes()[fraction_digits] >= b'5' {
        let mut position = digits.len();
        loop {
            if position == 0 {
                digits.insert(0, b'1');
                break;
            }
            position -= 1;
            if digits[position] == b'9' {
                digits[position] = b'0';
            } else {
                digits[position] += 1;
                break;
            }
        }
    }
    let integer_length = digits.len() - fraction_digits;
    let mut text = String::with_capacity(digits.len() + 2);
    if value.is_sign_negative() {
        text.push('-');
    }
    text.push_str(std::str::from_utf8(&digits[..integer_length]).expect("ASCII digits"));
    if fraction_digits > 0 {
        text.push('.');
        text.push_str(std::str::from_utf8(&digits[integer_length..]).expect("ASCII digits"));
    }
    text
}
```

- [ ] **Step 4: Run the unit test.**

Run: `cargo nextest run -p ns-runtime fixed_text`
Expected: PASS.

- [ ] **Step 5: Add the leaves and forwarders.**

In `primitive.rs`, add these variants, each in its `=> false` group:
- `AsFloat` to `IntegerPrimitive` and `LargeIntegerPrimitive`;
- `AsStringFixed` to `FloatPrimitive`.

Append these rows:
```rust
    mirror(
        "integerAsFloat:ifFail:",
        "asFloat",
        Primitive::Integer(IntegerPrimitive::AsFloat),
    ),
    mirror(
        "largeIntegerAsFloat:ifFail:",
        "asFloat",
        Primitive::LargeInteger(LargeIntegerPrimitive::AsFloat),
    ),
    mirror(
        "float:asStringFixed:ifFail:",
        "asStringFixed:",
        Primitive::Float(FloatPrimitive::AsStringFixed),
    ),
```
In `numbers.rs`:
```rust
        // `as` rounds to the nearest double, ties to even.
        IntegerPrimitive::AsFloat => Some(Ok(Value::Float(receiver_integer as f64))),
```
```rust
        LargeIntegerPrimitive::AsFloat => Some(Ok(Value::Float(receiver_integer.to_f64()))),
```
```rust
        FloatPrimitive::AsStringFixed => Some(match arguments.first() {
            Some(Value::SmallInteger(count)) if (0..=20).contains(count) => Ok(Value::Object(
                mutator.new_string(fixed_text(receiver_float, *count as usize)),
            )),
            // Primordial Soup's primitive fails outside 0 to 20 and its
            // method raises; double-conversion's buffer is sized for 20.
            Some(Value::SmallInteger(count)) => Err(NewspeakError {
                selector: Symbol::intern("asStringFixed:"),
                receiver,
                message: format!("asStringFixed: takes 0 to 20 fraction digits, got {count}"),
            }),
            _ => Err(mismatch("asStringFixed:")),
        }),
```
If clippy refuses `*count as usize`, use `usize::try_from(*count)`; the guard makes it infallible.

In `Kernel.ns`:
- `Integer`: `public asFloat = ( ^vmMirror integerAsFloat: self ifFail: [^super asFloat] )`
- `LargeInteger`: `public asFloat = ( ^vmMirror largeIntegerAsFloat: self ifFail: [^super asFloat] )`
- `Float`: `public asStringFixed: fractionDigits = ( ^vmMirror float: self asStringFixed: fractionDigits ifFail: [^super asStringFixed: fractionDigits] )`

- [ ] **Step 6: Write the goldens.**

`goldens/arithmetic/as_float_and_as_string_fixed.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* `asFloat` answers the nearest double, and `asStringFixed:` prints a
   fixed number of fraction digits as Primordial Soup's double-conversion
   does: a tie rounds up on the exact binary value, and the sign is the
   value's. The harness's score is printed with it. *)
class AsFloatAndFixed = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. |
        transcript print: 3 asFloat; newLine.
        transcript print: -9223372036854775808 asFloat; newLine.
        transcript print: 9223372036854775808 asFloat; newLine.
        transcript show: ((1000000000 * 7) asFloat / 3 asFloat asStringFixed: 1); newLine.
        transcript show: (0.125 asStringFixed: 2); newLine.
        transcript show: (-0.125 asStringFixed: 2); newLine.
        transcript show: (2.5 asStringFixed: 0); newLine.
        transcript show: (-0.04 asStringFixed: 1); newLine.
        transcript show: (1.005 asStringFixed: 2); newLine
    )
)
```
Mind the precedence. `(1000000000 * 7) asFloat / 3 asFloat asStringFixed: 1` parses as `((7000000000.0) / (3.0)) asStringFixed: 1`, because the keyword applies to the whole binary expression. The result is `'2333333333.3'`.

`goldens/arithmetic/as_float_and_as_string_fixed.toml`, where each `print:` line is Rust's `Display` of the double:
```toml
stdout = '''
3
-9223372036854775808
9223372036854775808
2333333333.3
0.13
-0.13
3
-0.0
1.00
'''
```

`goldens/errors/as_string_fixed_refuses_more_than_twenty_digits.ns`:
```newspeak
Newspeak3
'Errors'
(* Primordial Soup's `asStringFixed:` takes 0 to 20 fraction digits and
   raises for any other count. *)
class AsStringFixedRefuses = () (
    public main: platform args: arguments = (
        platform transcript show: (1.5 asStringFixed: 21); newLine
    )
)
```
`goldens/errors/as_string_fixed_refuses_more_than_twenty_digits.toml`:
```toml
status = 1
stderr = '''
Uncaught Newspeak error: asStringFixed: takes 0 to 20 fraction digits, got 21
'''
```

- [ ] **Step 7: Run the goldens and the per-commit gate.**

Expected: PASS. If `3 asFloat` prints `3` rather than `3.0`, that is today's `Float printString` (Rust's `Display`), not a defect of this task: record what it prints.

- [ ] **Step 8: Commit.**

```bash
git commit -m "feat: integers answer asFloat, Float answers asStringFixed: as Primordial Soup rounds"
```

---

### Task 5: `platform time Stopwatch`

`Stopwatch` is Primordial Soup's (`prior-art/primordialsoup/newspeak/Time.ns:179-204`), narrowed to what the harness sends: `new`, `start` and `elapsedNanoseconds`. `stop` comes later, when a program needs it.
- **The clock.** It is one new platform leaf, `clockMonotonicNanoseconds:ifFail:`, which answers the nanoseconds since a process-wide origin: a `std::time::Instant` taken the first time anything reads the clock.
- **The facet.** `platform time` is a facet like `kernel`: a `Value` whose one member is the `Stopwatch` class accessor.
- **`Stopwatch` is not a value.** It keeps mutable state, so it is `Object new`, as `CollectionsFactory.ns`'s `List` is. The module comment's "Everything here is a value" is narrowed to say so.

**Files:**
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns`
- Modify: `crates/ns-runtime/src/platform.rs` (the leaf, the origin, the module comment)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`PlatformPrimitive::MonotonicNanoseconds`, a row, `answers_false_to_a_stranger`)
- Modify tests that pin the platform's shape. Run them first to see which fail. These are expected:
  - `crates/ns-runtime/tests/platform.rs`: `the_platform_builds_its_members_on_first_use` and the members list near line 473;
  - `crates/ns-runtime/tests/image.rs`: `a_class_the_platform_hands_out_is_named_by_its_accessor`, `a_platform_member_is_named_by_its_path`;
  - `crates/ns-runtime/src/interpreter/kernel.rs`: `the_platforms_class_accessors_are_recognized_without_a_fast_path` (the list near line 1330) and `objects_class_accessors_are_exactly_the_nameable_builtins` (the absent names near line 1555).
- Create: `goldens/kernel/a_stopwatch_measures_elapsed_time.ns`, `.toml`

**Interfaces:**
- Produces:
  - `platform time`, answering the `Time` facet;
  - `platform Time`, `platform Stopwatch`, `platform time Stopwatch`;
  - `Stopwatch new start`, answering the stopwatch;
  - `Stopwatch>>elapsedNanoseconds`, answering an `Integer`;
  - `PlatformPrimitive::MonotonicNanoseconds`.

- [ ] **Step 1: Write the golden.**

The golden cannot print a time, so it prints what must hold of any run: an unstarted stopwatch reads 0, and a started one reads a non-negative integer that does not decrease.

`goldens/kernel/a_stopwatch_measures_elapsed_time.ns`:
```newspeak
Newspeak3
'Kernel'
(* `platform time Stopwatch` is Primordial Soup's: it reads a monotonic
   clock in nanoseconds, so an unstarted stopwatch reads 0, and a started
   one reads an integer that never decreases. Nothing here prints a time,
   which would differ between runs. *)
class StopwatchReads = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          Stopwatch = platform time Stopwatch.
          idle = Stopwatch new.
          running = Stopwatch new start.
          first second |
        transcript print: idle elapsedNanoseconds; newLine.
        first:: running elapsedNanoseconds.
        1 to: 1000 do: [:index | index].
        second:: running elapsedNanoseconds.
        transcript print: first >= 0; newLine.
        transcript print: second >= first; newLine.
        transcript print: (running start) == running; newLine
    )
)
```
`goldens/kernel/a_stopwatch_measures_elapsed_time.toml`:
```toml
stdout = '''
0
true
true
true
'''
```

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo nextest run -p ns-cli --test run_goldens a_stopwatch`
Expected: FAIL, with `does not understand #time`.

- [ ] **Step 3: Add the leaf.**

In `primitive.rs`:
- Add `MonotonicNanoseconds` to `PlatformPrimitive`.
- Extend the arm `Primitive::Platform(PlatformPrimitive::Show | PlatformPrimitive::NewLine) => false` with it.
- Append this row:
```rust
    mirror(
        "clockMonotonicNanoseconds:ifFail:",
        "currentMonotonicNanoseconds",
        Primitive::Platform(PlatformPrimitive::MonotonicNanoseconds),
    ),
```
In `platform.rs`, add a new arm to `stream`, and the origin above it:
```rust
/// The instant every monotonic reading is measured from, taken on the
/// first reading. One per process, so two interpreters' stopwatches agree.
static CLOCK_ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
```
```rust
        // Nanoseconds since the process's origin: an `i64` holds 292 years
        // of them, so the conversion cannot fail in practice and saturates
        // rather than wraps if it ever did.
        PlatformPrimitive::MonotonicNanoseconds => {
            let elapsed = CLOCK_ORIGIN.get_or_init(std::time::Instant::now).elapsed();
            Ok(Value::SmallInteger(
                i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX),
            ))
        }
```
Rewrite `platform.rs`'s module comment so its first sentence stays true. It currently says the transcript is "the one part of the platform that reaches Rust". It now becomes "the transcript's output and the monotonic clock, the two parts of the platform that reach Rust", with one sentence on the clock.

- [ ] **Step 4: Add the Newspeak.**

In `PlatformFactory.ns`, add to `Platform`:
```newspeak
        lazy public time = Time new.
```
next to `lazy public kernel = KernelNamespace new.`, and with the other class accessors:
```newspeak
        public Time = ( ^outer PlatformFactory Time )
        public Stopwatch = ( ^outer PlatformFactory Stopwatch )
```
Then add, after `KernelNamespace`:
```newspeak
    (* `platform time`: Primordial Soup's `Time` facet, holding only what
       `benchmarks/are-we-fast-yet/AreWeFastYet.ns` measures with. *)
    private class Time = Value new () (
        public Stopwatch = ( ^outer PlatformFactory Stopwatch )
    )
    (* Primordial Soup's `Stopwatch` (`Time.ns`), narrowed to what
       `AreWeFastYet.ns` sends. It keeps its reading in slots, so it is
       an object, not a value. A stopwatch carried across a snapshot reads
       a start taken against another process's clock, as Primordial Soup's
       does. *)
    private class Stopwatch = Object new (
        | private cumulativeNanoseconds ::= 0.
          private startNanoseconds ::= nil. |
    ) (
        public elapsedNanoseconds = (
            startNanoseconds isNil ifTrue: [^cumulativeNanoseconds].
            ^cumulativeNanoseconds + (currentMonotonicNanoseconds - startNanoseconds)
        )
        public start = (
            startNanoseconds isNil ifFalse: [^self].
            startNanoseconds:: currentMonotonicNanoseconds
        )
        private currentMonotonicNanoseconds = ( ^vmMirror clockMonotonicNanoseconds: self ifFail: [nil] )
    )
```
Narrow the module comment's "Everything here is a value (§3.1.1)" paragraph to "Everything here but `Stopwatch` is a value…", with the reason from `Stopwatch`'s comment.

If the bootstrap or a kernel test requires every nested class of this module to be a value, stop and report it. Do not work around it.

- [ ] **Step 5: Run the golden, then fix the shape tests.**

Run: `cargo nextest run -p ns-cli --test run_goldens a_stopwatch`
Expected: PASS.

Run: `cargo nextest run --workspace --exclude ns-lsp`
Expected: the shape tests named under **Files** fail and name the new members. Extend each list with exactly `time`, `Time` and `Stopwatch`, and each expected path list with the paths the failure prints for them: `time`, `Time`, `Stopwatch`, `time Stopwatch`. A test that pins something else is a real failure; stop and report it.

Also add a test to `tests/image.rs`, beside `a_list_is_a_record_of_its_path_named_class`: a program-made `Stopwatch` is an ordinary record of its path-named class and round-trips. Model it on that test, with `platform time Stopwatch new start` in place of the list.

- [ ] **Step 6: Run the per-commit gate.**

Expected: PASS.

- [ ] **Step 7: Commit.**

```bash
git commit -m "feat: platform time answers Primordial Soup's Stopwatch over a monotonic clock"
```

---

### Task 6: the harness and Sieve, verified by goldens

This task brings in the harness, the first benchmark, the project file and the licence notices, and pins them. Tasks 7a and 7b add the other six benchmarks to the same harness.

**Files:**
- Create: `benchmarks/are-we-fast-yet/AreWeFastYet.ns`
- Create: `benchmarks/are-we-fast-yet/Sieve.ns`
- Create: `benchmarks/are-we-fast-yet/are-we-fast-yet.json`
- Create: `goldens/awfy/all_benchmarks_verify.json`, `.toml`
- Create: `goldens/awfy/a_named_benchmark_runs_alone.json`, `.toml`
- Create: `goldens/awfy/an_unknown_benchmark_is_refused.json`, `.toml`
- Create: `goldens/awfy/extra_arguments_are_a_usage_error.json`, `.toml`
- Modify: `THIRD-PARTY-NOTICES.md`

**Interfaces:**
- Consumes (Tasks 1-5):
  - `to:do:`, `bitAnd:`, `\\`, `asFloat`, `asStringFixed:`;
  - `platform time Stopwatch`, `platform kernel Error`, `platform transcript`;
  - `Class>>name`, answering the module's declared name as a `String`.
- Produces:
  - **Each benchmark module**, `class <Name> usingPlatform: platform`, whose instance answers `bench`. `bench` runs the workload once and raises `Error signal: '<Name>: expected <e>, got <a>'` when the result is wrong.
  - **The harness's command line:** `-- [--timed] [<Name>]`.
  - **Its output:**
    - untimed, one line `<Name>: ok` per benchmark, in the order of its `benchmarks` tuple;
    - timed, `<Name>: <runs per second, one fraction digit>`.

- [ ] **Step 1: Write the harness.**

`benchmarks/are-we-fast-yet/AreWeFastYet.ns`:
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from Primordial Soup's BenchmarkRunner.ns
   (https://github.com/rmacnak/primordialsoup, commit
   338f0bb342a618f5608ab72ab1b1fae9b226e472, newspeak/BenchmarkRunner.ns).
   Changed for ns-next: the benchmarks are the Are We Fast Yet programs;
   a run without --timed verifies each benchmark once instead of
   measuring it, so its output can be pinned; the command line selects a
   benchmark by name; output goes through the platform's transcript
   rather than Object>>out; names are written out.

   Copyright 2012 Google Inc.
   Copyright 2013 Ryan Macnak

   Licensed under the Apache License, Version 2.0 (the ''License''); you
   may not use this file except in compliance with the License. You may
   obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
   and in THIRD-PARTY-NOTICES.md at the repository root.

   ns-next -- run are-we-fast-yet.json -- [--timed] [Benchmark]: without
   --timed, runs each benchmark (or the one named) once and prints
   '<Name>: ok'; with it, measures each for at least 20 ms after 3 ms of
   warm-up and prints its runs per second, as Primordial Soup does. *)
class AreWeFastYet packageUsing: manifest = (
    | private benchmarks = {
        manifest Sieve }. |
) (
    class Benchmarking usingPlatform: platform = (
        | private Stopwatch = platform time Stopwatch.
          private Error = platform kernel Error.
          private transcript = platform transcript.
          private benchmarkPlatform = platform. |
    ) (
        (* Primordial Soup's measure: runs the block until the time is
           spent and answers runs per second. *)
        measure: block forAtLeast: nanoseconds = (
            | runs stopwatch elapsed |
            runs:: 0.
            stopwatch:: Stopwatch new start.
            [block value.
             runs:: runs + 1.
             elapsed:: stopwatch elapsedNanoseconds.
             elapsed < nanoseconds] whileTrue: [].
            ^(runs * 1000000000) asFloat / elapsed asFloat
        )
        (* The line is written only once the benchmark has run, so a
           benchmark that raises leaves no half-written line on stdout. *)
        report: benchmark timed: timed = (
            | instance result |
            instance:: benchmark usingPlatform: benchmarkPlatform.
            timed
                ifTrue: [
                    measure: [instance bench] forAtLeast: 3000000.
                    result:: (measure: [instance bench] forAtLeast: 20000000) asStringFixed: 1]
                ifFalse: [
                    instance bench.
                    result:: 'ok'].
            transcript show: benchmark name, ': ', result; newLine
        )
        public run: name timed: timed = (
            | found |
            found:: false.
            benchmarks do: [:benchmark |
                (name isNil or: [benchmark name = name]) ifTrue: [
                    found:: true.
                    report: benchmark timed: timed]].
            found ifFalse: [Error signal: 'Unknown benchmark: ', name]
        )
        public usage = (
            Error signal: 'usage: ns-next run are-we-fast-yet.json -- [--timed] [Benchmark]'
        )
    )
    public main: platform args: arguments = (
        | benchmarking = Benchmarking usingPlatform: platform.
          timed first |
        timed:: arguments size >= 1 and: [(arguments at: 1) = '--timed'].
        first:: timed ifTrue: [2] ifFalse: [1].
        arguments size > first ifTrue: [^benchmarking usage].
        benchmarking
            run: (arguments size = first ifTrue: [arguments at: first] ifFalse: [nil])
            timed: timed
    )
)
```
Two constructs here are assumed rather than verified: a method temporary with an initializer (`| benchmarking = … |`, as `goldens/manifest/…/App.ns` uses) and `whileTrue: []` with an empty block. If either is refused, write the plain form: a temporary assigned on the next line, or `whileTrue: [nil]`.

- [ ] **Step 2: Write Sieve.**

The port comes from `prior-art/somns/core-lib/Benchmarks/Sieve.ns`:
- `flags putAll: true` becomes a `to:do:` that stores `true`.
- `harness Benchmark`'s `verifyResult:` becomes the check in `bench`.
- `i` and `k` become `candidate` and `multiple`.

`benchmarks/are-we-fast-yet/Sieve.ns`:
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from SOMns (https://github.com/smarr/SOMns, commit
   e56f4c2815cef4cae2045a9b05dc1fd190315ba6,
   core-lib/Benchmarks/Sieve.ns), one of the Are We Fast Yet benchmarks.
   Changed for ns-next: a module in Primordial Soup's shape, whose bench
   runs once and checks its own result; only Primordial Soup's protocol is
   sent; names are written out.

   Copyright (c) 2001-2015 see AUTHORS file

   Permission is hereby granted, free of charge, to any person obtaining a copy
   of this software and associated documentation files (the 'Software'), to deal
   in the Software without restriction, including without limitation the rights
   to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
   copies of the Software, and to permit persons to whom the Software is
   furnished to do so, subject to the following conditions:

   The above copyright notice and this permission notice shall be included in
   all copies or substantial portions of the Software.

   THE SOFTWARE IS PROVIDED 'AS IS', WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
   IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
   FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
   AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
   LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
   OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
   THE SOFTWARE. *)
class Sieve usingPlatform: platform = (
    | private Error = platform kernel Error. |
) (
    public bench = (
        | result |
        result:: sieve: (Array new: 5000) size: 5000.
        result = 669 ifFalse: [Error signal: 'Sieve: expected 669, got ', result printString].
        ^result
    )
    sieve: flags size: size = (
        | primeCount |
        primeCount:: 0.
        1 to: size do: [:index | flags at: index put: true].
        2 to: size do: [:candidate |
            (flags at: candidate - 1) ifTrue: [
                | multiple |
                primeCount:: primeCount + 1.
                multiple:: candidate + candidate.
                [multiple <= size] whileTrue: [
                    flags at: multiple - 1 put: false.
                    multiple:: multiple + candidate]]].
        ^primeCount
    )
)
```
Every later benchmark carries the same header, with its own file name in the path, and the same three-line "Changed for ns-next" text.

- [ ] **Step 3: Write the project file.**

`benchmarks/are-we-fast-yet/are-we-fast-yet.json`:
```json
{
  "root": "AreWeFastYet.ns",
  "modules": ["Sieve.ns"]
}
```

- [ ] **Step 4: Run it by hand.**

Run:
- `cargo run -q -p ns-cli -- run benchmarks/are-we-fast-yet/are-we-fast-yet.json`
- `cargo run -q -p ns-cli -- run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- --timed Sieve`

Expected: `Sieve: ok`, then `Sieve: <number with one fraction digit>`.

- [ ] **Step 5: Write the goldens.**

Every golden `.json` names the same modules as the project file, with paths relative to `goldens/awfy/`, because the harness names every benchmark when it is instantiated:
```json
{
  "root": "../../benchmarks/are-we-fast-yet/AreWeFastYet.ns",
  "modules": ["../../benchmarks/are-we-fast-yet/Sieve.ns"]
}
```
Write that same content to all four `.json` files. The `.toml`s:
- `all_benchmarks_verify.toml`:
  ```toml
  stdout = '''
  Sieve: ok
  '''
  ```
- `a_named_benchmark_runs_alone.toml`: `arguments = ["Sieve"]` and the same stdout.
- `an_unknown_benchmark_is_refused.toml`:
  ```toml
  arguments = ["Nope"]
  status = 1
  stderr = '''
  Uncaught Newspeak error: Unknown benchmark: Nope
  '''
  ```
- `extra_arguments_are_a_usage_error.toml`:
  ```toml
  arguments = ["--timed", "Sieve", "extra"]
  status = 1
  stderr = '''
  Uncaught Newspeak error: usage: ns-next run are-we-fast-yet.json -- [--timed] [Benchmark]
  '''
  ```

A golden `.json` carries no rationale comment. The golden category row in `docs/design/README.md` (Task 9) carries it instead.

- [ ] **Step 6: Run the goldens.**

Run: `cargo nextest run -p ns-cli awfy`
Expected: PASS for `golden::optimized`, `golden::reference` and `build_load` of all four.

- [ ] **Step 7: Add the notices.**

In `THIRD-PARTY-NOTICES.md`:
- Widen the introduction: `ns-next` also adapts Newspeak code from two projects under `benchmarks/are-we-fast-yet/`, each file naming its upstream in a header.
- Append two sections in the existing shape:
  - **`## SOMns`**: upstream `https://github.com/smarr/SOMns`, commit `e56f4c2815cef4cae2045a9b05dc1fd190315ba6`; MIT; the full text of `prior-art/somns/LICENSE` in a fence.
  - **`## Primordial Soup`**: upstream `https://github.com/rmacnak/primordialsoup`, commit `338f0bb342a618f5608ab72ab1b1fae9b226e472`; Apache License 2.0, which covers its Newspeak sources (`prior-art/primordialsoup/LICENSE`, quoted in a fence). It is followed by the full Apache License 2.0 text, copied from `prior-art/num-bigint/LICENSE-APACHE` from `Apache License` through `END OF TERMS AND CONDITIONS`. §4(a) of that licence requires a copy to accompany the work.

- [ ] **Step 8: Run the per-commit gate.**

Expected: PASS.

- [ ] **Step 9: Commit.**

```bash
git add benchmarks goldens/awfy THIRD-PARTY-NOTICES.md
git commit -m "feat: the Are We Fast Yet harness and Sieve, verified by goldens"
```

---

### Task 7a: Permute, Queens and Towers

Each port starts from the SOMns file in `prior-art/somns/core-lib/Benchmarks/` and carries Task 6's header with its own file name. The SOMns-only protocol each one sends is rewritten as listed; everything else stays as SOMns wrote it, apart from written-out names. Where SOMns evaluates an argument eagerly (`&&` with a Boolean, `and:` with a Boolean), the port uses `&`, which evaluates both sides too, so the workload does not change.

**Files:**
- Create: `benchmarks/are-we-fast-yet/Permute.ns`, `Queens.ns`, `Towers.ns`
- Modify: `benchmarks/are-we-fast-yet/AreWeFastYet.ns` (the tuple), `are-we-fast-yet.json`, the four `goldens/awfy/*.json`, `all_benchmarks_verify.toml`

**Interfaces:**
- Consumes: Task 6's module shape and header; `to:by:do:` (Task 1).

- [ ] **Step 1: Write Permute.**
```newspeak
class Permute usingPlatform: platform = (
    | private Error = platform kernel Error.
      count
      values |
) (
    public bench = (
        count:: 0.
        values:: Array new: 7.
        permute: 6.
        count = 8660 ifFalse: [Error signal: 'Permute: expected 8660, got ', count printString].
        ^count
    )
    permute: size = (
        count:: count + 1.
        (size = 0) not ifTrue: [
            permute: size - 1.
            size to: 1 by: -1 do: [:index |
                swap: size with: index.
                permute: size - 1.
                swap: size with: index]]
    )
    swap: first with: second = (
        | temp |
        temp:: values at: first.
        values at: first put: (values at: second).
        values at: second put: temp
    )
)
```
Where SOMns keeps `count` and `v` as slots of its nested `Permute` class, this port keeps them as slots of the module. `bench` resets both, so a second `bench` on one instance starts afresh, as SOMns's `newInstance` does per run. Its rewrites: `<>` becomes `(… = …) not`, and `downTo:do:` becomes `to:by:do:` with `-1`.

- [ ] **Step 2: Write Queens.**
```newspeak
class Queens usingPlatform: platform = (
    | private Error = platform kernel Error.
      freeMaxs freeRows freeMins queenRows |
) (
    public bench = (
        | result |
        result:: true.
        1 to: 10 do: [:index | result:: result & queens].
        result ifFalse: [Error signal: 'Queens: expected true, got false'].
        ^result
    )
    queens = (
        freeRows:: newArray: 8 withAll: true.
        freeMaxs:: newArray: 16 withAll: true.
        freeMins:: newArray: 16 withAll: true.
        queenRows:: newArray: 8 withAll: -1.
        ^placeQueen: 1
    )
    placeQueen: column = (
        1 to: 8 do: [:row |
            (row: row column: column) ifTrue: [
                queenRows at: row put: column.
                row: row column: column put: false.
                column = 8 ifTrue: [^true].
                (placeQueen: column + 1) ifTrue: [^true].
                row: row column: column put: true]].
        ^false
    )
    row: row column: column = (
        ^(freeRows at: row) & (freeMaxs at: column + row) & (freeMins at: column - row + 8)
    )
    row: row column: column put: value = (
        freeRows at: row put: value.
        freeMaxs at: column + row put: value.
        freeMins at: column - row + 8 put: value
    )
    (* SOMns's `Array new:withAll:` sends its argument `value`; Primordial
       Soup's has no such message, and a Boolean or an Integer is stored as
       it is. *)
    newArray: size withAll: element = (
        | array |
        array:: Array new: size.
        1 to: size do: [:index | array at: index put: element].
        ^array
    )
)
```
Its rewrites:
- `result and: self queens` becomes `result & queens`: SOMns evaluates `self queens` before the send, and `&` also takes an evaluated Boolean.
- `&&` between Booleans becomes `&`.
- `new:withAll:` becomes `newArray:withAll:`.

- [ ] **Step 3: Write Towers.**
```newspeak
class Towers usingPlatform: platform = (
    | private Error = platform kernel Error.
      piles
      movesDone |
) (
    public bench = (
        piles:: Array new: 4.
        buildTowerAt: 1 disks: 13.
        movesDone:: 0.
        move: 13 disksFrom: 1 to: 2.
        movesDone = 8191 ifFalse: [Error signal: 'Towers: expected 8191, got ', movesDone printString].
        ^movesDone
    )
    pushDisk: disk onPile: pile = (
        | top |
        top:: piles at: pile.
        (top notNil and: [disk size >= top size])
            ifTrue: [Error signal: 'Cannot put a big disk on a smaller one'].
        disk next: top.
        piles at: pile put: disk
    )
    popDiskFrom: pile = (
        | top |
        top:: piles at: pile.
        top isNil ifTrue: [Error signal: 'Attempting to remove a disk from an empty pile'].
        piles at: pile put: top next.
        top next: nil.
        ^top
    )
    moveTopDiskFrom: fromPile to: toPile = (
        pushDisk: (popDiskFrom: fromPile) onPile: toPile.
        movesDone:: movesDone + 1
    )
    buildTowerAt: pile disks: disks = (
        disks to: 0 by: -1 do: [:size | pushDisk: (TowersDisk new: size) onPile: pile]
    )
    move: disks disksFrom: fromPile to: toPile = (
        disks = 1
            ifTrue: [moveTopDiskFrom: fromPile to: toPile]
            ifFalse: [
                | otherPile |
                otherPile:: 6 - fromPile - toPile.
                move: disks - 1 disksFrom: fromPile to: otherPile.
                moveTopDiskFrom: fromPile to: toPile.
                move: disks - 1 disksFrom: otherPile to: toPile]
    )
    class TowersDisk new: diskSize = (
        | public size = diskSize.
          public next |
    ) ()
)
```
Its rewrites:
- `&&` with a block becomes `and:`.
- `self error:` becomes `Error signal:`.
- `downTo:do:` becomes `to:by:do:`.
- The factory parameter is renamed so it does not collide with the slot `size`.

- [ ] **Step 4: Add them to the harness and the projects.**

Add them to the tuple in `AreWeFastYet.ns`, in alphabetical order: `manifest Permute. manifest Queens. manifest Sieve. manifest Towers`. Add their file names to `are-we-fast-yet.json`'s `modules` and to each `goldens/awfy/*.json`.

The expected output of `all_benchmarks_verify.toml` becomes:
```
Permute: ok
Queens: ok
Sieve: ok
Towers: ok
```

- [ ] **Step 5: Run the goldens and the per-commit gate.**

Expected: PASS. If a benchmark raises its own `expected … got …` error, the port differs from SOMns. Compare it line by line with the SOMns file; do not change the expected value.

- [ ] **Step 6: Commit.**

```bash
git commit -m "feat: Permute, Queens and Towers in Primordial Soup's protocol"
```

---

### Task 7b: Bounce, Storage and List

**Files:**
- Create: `benchmarks/are-we-fast-yet/Bounce.ns`, `Storage.ns`, `List.ns`
- Modify: the tuple, the project file, the four golden `.json` files, and `all_benchmarks_verify.toml`, as in Task 7a

**Interfaces:**
- Consumes: `\\`, `bitAnd:` (Tasks 2 and 3).

- [ ] **Step 1: Write Bounce.**

`Random` is the harness's in SOMns. Primordial Soup's module shape has no harness to take it from, so Bounce and Storage each declare their own.
```newspeak
class Bounce usingPlatform: platform = (
    | private Error = platform kernel Error. |
) (
    public bench = (
        | ballCount balls bounces random |
        random:: Random new: 74755.
        ballCount:: 100.
        bounces:: 0.
        balls:: Array new: ballCount.
        1 to: ballCount do: [:index | balls at: index put: (Ball new: random)].
        1 to: 50 do: [:index |
            balls do: [:ball |
                ball bounce ifTrue: [bounces:: bounces + 1]]].
        bounces = 1331 ifFalse: [Error signal: 'Bounce: expected 1331, got ', bounces printString].
        ^bounces
    )
    (* The Are We Fast Yet harness's generator (SOMns's Harness.ns). *)
    class Random new: initialSeed = (
        | seed ::= initialSeed. |
    ) (
        public next = (
            seed:: seed * 1309 + 13849 bitAnd: 65535.
            ^seed
        )
    )
    class Ball new: random = (
        | horizontal ::= random next \\ 500.
          vertical ::= random next \\ 500.
          horizontalVelocity ::= (random next \\ 300) - 150.
          verticalVelocity ::= (random next \\ 300) - 150. |
    ) (
        public bounce = (
            | horizontalLimit verticalLimit bounced |
            horizontalLimit:: 500.
            verticalLimit:: 500.
            bounced:: false.
            horizontal:: horizontal + horizontalVelocity.
            vertical:: vertical + verticalVelocity.
            horizontal > horizontalLimit ifTrue: [
                horizontal:: horizontalLimit.
                horizontalVelocity:: 0 - horizontalVelocity abs.
                bounced:: true].
            horizontal < 0 ifTrue: [
                horizontal:: 0.
                horizontalVelocity:: horizontalVelocity abs.
                bounced:: true].
            vertical > verticalLimit ifTrue: [
                vertical:: verticalLimit.
                verticalVelocity:: 0 - verticalVelocity abs.
                bounced:: true].
            vertical < 0 ifTrue: [
                vertical:: 0.
                verticalVelocity:: verticalVelocity abs.
                bounced:: true].
            ^bounced
        )
    )
)
```
- `seed * 1309 + 13849 bitAnd: 65535` parses as `((seed * 1309) + 13849) bitAnd: 65535`, which is SOMns's `& 65535`.
- `Array new: ballCount withAll: [Ball new: random]` became the loop. SOMns evaluates the block once per element, in index order, so the balls draw from `random` in the same order.
- `Random new` with its class-side default seed became `Random new: 74755`, which is SOMns's default.

- [ ] **Step 2: Write Storage.**
```newspeak
class Storage usingPlatform: platform = (
    | private Error = platform kernel Error.
      count |
) (
    public bench = (
        count:: 0.
        buildTreeDepth: 7 with: (Random new: 74755).
        count = 5461 ifFalse: [Error signal: 'Storage: expected 5461, got ', count printString].
        ^count
    )
    buildTreeDepth: depth with: random = (
        | array |
        count:: count + 1.
        depth = 1 ifTrue: [^Array new: random next \\ 10 + 1].
        array:: Array new: 4.
        1 to: 4 do: [:index | array at: index put: (buildTreeDepth: depth - 1 with: random)].
        ^array
    )
    (* The Are We Fast Yet harness's generator (SOMns's Harness.ns). *)
    class Random new: initialSeed = (
        | seed ::= initialSeed. |
    ) (
        public next = (
            seed:: seed * 1309 + 13849 bitAnd: 65535.
            ^seed
        )
    )
)
```

- [ ] **Step 3: Write List.**
```newspeak
class List usingPlatform: platform = (
    | private Error = platform kernel Error. |
) (
    public bench = (
        | result |
        result:: (talkWith: (makeList: 15) with: (makeList: 10) with: (makeList: 6)) length.
        result = 10 ifFalse: [Error signal: 'List: expected 10, got ', result printString].
        ^result
    )
    makeList: length = (
        | element |
        length = 0 ifTrue: [^nil].
        element:: ListElement new: length.
        element next: (makeList: length - 1).
        ^element
    )
    isShorter: first than: second = (
        | firstTail secondTail |
        firstTail:: first.
        secondTail:: second.
        [secondTail isNil] whileFalse: [
            firstTail isNil ifTrue: [^true].
            firstTail:: firstTail next.
            secondTail:: secondTail next].
        ^false
    )
    (* SOMns's talkWithX:withY:withZ:, Takeuchi's function over lists. *)
    talkWith: first with: second with: third = (
        (isShorter: second than: first) ifFalse: [^third].
        ^talkWith: (talkWith: first next with: second with: third)
            with: (talkWith: second next with: third with: first)
            with: (talkWith: third next with: first with: second)
    )
    class ListElement new: initialValue = (
        | public value ::= initialValue.
          public next ::= nil. |
    ) (
        public length = (
            next isNil ifTrue: [^1].
            ^1 + next length
        )
    )
)
```
- The module is named `List`, as in SOMns. It is a manifest module name, so check that the manifest does not refuse it for shadowing a public member of every object (`image-format.md`, "Packaging": "`Array`, `String` or any other predefined class `Object` has an accessor for").
  - `Object` has no `List` accessor today, so it is accepted.
  - If it is refused, rename the module to `ListBenchmark`, with its output line `ListBenchmark: ok`, and say so in its header.
- `whileFalse:` is a Block leaf today (`Kernel.ns:602`). Check it is present before relying on it.

- [ ] **Step 4: Add them to the harness and the projects.**

The tuple becomes, in alphabetical order:
```
manifest Bounce. manifest List. manifest Permute. manifest Queens.
manifest Sieve. manifest Storage. manifest Towers
```
`all_benchmarks_verify.toml`:
```toml
stdout = '''
Bounce: ok
List: ok
Permute: ok
Queens: ok
Sieve: ok
Storage: ok
Towers: ok
'''
```

- [ ] **Step 5: Run the goldens and the per-commit gate.**

Expected: PASS. Handle a benchmark's own error as in Task 7a.

- [ ] **Step 6: Commit.**

```bash
git commit -m "feat: Bounce, Storage and List complete stage 1 of Are We Fast Yet"
```

---

### Task 8: `--timed` and `bench`, checked by shape

A timed run's numbers differ between runs, so no golden pins them. A CLI test checks the shape instead. It also checks `--timed` with no name, which times all seven, and that `ns-next bench` accepts the project.

**Files:**
- Modify: `crates/ns-cli/tests/run.rs`

- [ ] **Step 1: Write the tests**, after the existing project tests, using the file's own `run`-style helper and the repository-root path helper that `cli.rs:351-354` uses for goldens:
```rust
/// `--timed` prints `<Name>: <runs per second>` with one fraction digit,
/// for every benchmark in the harness's order; the numbers differ between
/// runs, so only their shape is checked.
#[test]
fn a_timed_run_prints_a_score_for_every_benchmark() {
    let project = repository_root().join("benchmarks/are-we-fast-yet/are-we-fast-yet.json");
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(&project)
        .args(["--", "--timed"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let names: Vec<&str> = stdout
        .lines()
        .map(|line| {
            let (name, score) = line.split_once(": ").expect("a name and a score");
            let (whole, fraction) = score.split_once('.').expect("a fraction digit");
            assert!(!whole.is_empty() && whole.bytes().all(|byte| byte.is_ascii_digit()), "{line}");
            assert!(fraction.len() == 1 && fraction.bytes().all(|byte| byte.is_ascii_digit()), "{line}");
            name
        })
        .collect();
    assert_eq!(
        names,
        ["Bounce", "List", "Permute", "Queens", "Sieve", "Storage", "Towers"]
    );
}

/// `ns-next bench` times the untimed harness on one benchmark.
#[test]
fn bench_times_one_benchmark_of_the_project() {
    let project = repository_root().join("benchmarks/are-we-fast-yet/are-we-fast-yet.json");
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("bench")
        .arg(&project)
        .args(["--iterations", "1", "--", "Sieve"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Sieve: ok"), "{stdout}");
    assert!(stdout.contains("median"), "{stdout}");
}
```
Use the crate's existing helper for the repository root; find how `cli.rs` reaches `goldens/`. If none exists, derive the root from `env!("CARGO_MANIFEST_DIR")` joined with `../..`.

- [ ] **Step 2: Run them.**

Run: `cargo nextest run -p ns-cli a_timed_run bench_times_one`
Expected: PASS. A timed run spends at least 23 ms per benchmark, so about 0.2 s of measuring on top of the debug build's run time.

- [ ] **Step 3: Run the per-commit gate, then commit.**

```bash
git commit -m "test: a timed run's scores and bench over the project are checked by shape"
```

---

### Task 9: the documentation describes the built state

**Files:**
- Modify: `docs/design/core-library.md`, `docs/design/execution.md`, `docs/design/README.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`, `docs/references/PROVENANCE.md`, `prior-art/PROVENANCE.md`, `CLAUDE.md`

- [ ] **Step 1: `core-library.md`.**
  - The per-class forwarder lists (around lines 110-121) gain:
    - `Number`'s `to:do:`, `to:by:do:`;
    - `Integer`'s and `LargeInteger`'s `\\`, `bitAnd:`, `asFloat`;
    - `Float`'s `<=`, `>=`, `asStringFixed:`.
  - "The leaves are the 73 rows" (around line 371) gives today's count of `mirror(` rows. Count them.
  - "Large integers" gives its new row count, and says `\\` derives from `floor_divide` and `bitAnd:` is over two's complement.
  - "The platform" (around line 378):
    - its code block gains `lazy public time`, `Time` and `Stopwatch`;
    - its text says why `Stopwatch` is an object;
    - it records the clock's process-wide origin and the snapshot property.
  - Add one paragraph with its reasoning: where Primordial Soup's compiler inlines `to:do:`, ours sends it.
  - Remove any sentence the new protocol makes false. For example, check "Gap: an integer has no `/`" still reads true: `asFloat` does not close it.
- [ ] **Step 2: `execution.md`, "Measurements".** Add a subsection, "The Are We Fast Yet benchmarks". It covers:
  - what is in `benchmarks/are-we-fast-yet/` and where it comes from;
  - the two ways to measure: `ns-next bench are-we-fast-yet.json -- <Name>` and `ns-next run … -- --timed [<Name>]`, and what each reports;
  - that `bench` includes the one-off warm-up of each `main:args:` and excludes assembling the application;
  - that the protocol in `measuring-performance` applies: release builds, the same `RUSTFLAGS`.
- [ ] **Step 3: `README.md`.**
  - "The goldens": the category table gains an `awfy` row, "the Are We Fast Yet harness, verified; its sources live in `benchmarks/are-we-fast-yet/`".
  - The sentence "A project golden's modules live in the directory `goldens/<category>/<name>/`" allows a project golden that points at a shared tree, as `awfy`'s do.
  - "The map" names `benchmarks/`.
- [ ] **Step 4: `OPEN-WORK.md`.**
  - Remove stage 1 from "Next" and from the stage list.
  - Remove "Still to be decided: how they are measured" and point at `execution.md` instead.
  - Keep the stage 2 and stage 3 lines as they stand, and add one sentence. Stage 1 follows Primordial Soup's protocol and module shape (decided with the user on 2026-09-30, for stage 1). Whether stages 2 and 3 follow it too is for their plans to settle, and their selector lists were surveyed against SOMns's protocol.
  - "Next" names stage 2, whose plan is still to be written.
  - Add three items, each with where it is explained:
    - **A program's exit status.** A program cannot set its exit status. §8.4 is silent. Primordial Soup exits through an explicit `exit:` primitive (`Actors.ns:588`, `TestRunner.ns:50`) and ignores `main:args:`'s answer. SOMns takes the answer as the status and also has `system exit:`. The user decided on 2026-09-30 that this is an item of its own, outside the benchmarks. The recommendation given then was an explicit `platform system exit:` rather than the answer; that is not yet decided. Also open:
      - whether pending `ensure:` blocks run (Primordial Soup's primitive ends the process at once);
      - that an in-process host needs an unwind to the entry rather than `std::process::exit`;
      - the range of the status.
    - **Inlining `to:do:` and `to:by:do:`.** Primordial Soup's compiler inlines both (`:inlineBytecode:`, `KernelForPrimordialSoup.ns:2106`); ours sends them to `Number`. Whether to inline is decided by measuring the Are We Fast Yet benchmarks. It relates to "Between steps 7 and 8, real Newspeak bodies for `whileTrue:`…".
    - **A `SmallInteger` receiver with a `Float` bound in `to:do:`** raises until mixed arithmetic exists (stage 2). Name it under stage 2 rather than as an item of its own.
- [ ] **Step 5: `PROGRESS.md`.**
  - "Where the next session starts" names stage 2.
  - A new milestone section, "Are We Fast Yet, stage 1", lists the commits of this plan with one line each, in the shape of its neighbours.
- [ ] **Step 6: `PROVENANCE.md` files.**
  - `prior-art/PROVENANCE.md`'s somns and primordialsoup licence lines say code is now adapted from both, with notices in `THIRD-PARTY-NOTICES.md`.
  - `docs/references/PROVENANCE.md`'s "somns … not cited by any document" sentence is corrected if `execution.md` now cites it.
- [ ] **Step 7: `CLAUDE.md`.**
  - The overview names `benchmarks/are-we-fast-yet/` in one line, beside `editors/jetbrains/`.
  - "`PlatformFactory.ns` and `CollectionsFactory.ns` are widened only on demand: an accessor exists because a `runnable.toml` module needs it" is widened: "… a `runnable.toml` module or a program under `benchmarks/` needs it".
  - "Copying code out of `prior-art/` is a licensing decision, made so far only for `num-bigint` and `num-integer`" names SOMns and Primordial Soup too.
- [ ] **Step 8: Check the diff is Markdown only** (`git diff --stat`), then commit. A Markdown-only commit runs no gates.

```bash
git commit -m "docs: Are We Fast Yet stage 1, platform time, and the protocol it added"
```

---

### Task 10: the end-of-batch gates

- [ ] **Step 1: Run the `finishing-a-batch` skill** and every gate it names:
  - the three stress modes;
  - `run-corpus` plain and under stress;
  - `cargo nextest run -p ns-lsp`;
  - `cargo nextest run --workspace --release` for the size tests;
  - its check that a fresh session can start from the documentation alone.

  `verifyPlugin` does not apply: nothing under `editors/jetbrains`, `ns-lsp` or `ns-syntax` changed. Confirm with `git diff --stat master`.
- [ ] **Step 2: Fix what a gate finds** in a commit of its own, naming the gate, and rerun that gate.
- [ ] **Step 3: Report** each gate's command and result.
