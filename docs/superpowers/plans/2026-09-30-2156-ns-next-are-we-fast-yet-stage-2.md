# Are We Fast Yet, Stage 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mandelbrot, NBody and Richards join the seven Are We Fast Yet benchmarks of stage 1 in `benchmarks/are-we-fast-yet/`, and Primordial Soup's own Richards (the Ton80 version) joins them as `RichardsTon80`. Each runs under the existing harness at its Are We Fast Yet size, and the kernel widens only as far as they need, in Primordial Soup's protocol.

**Architecture:**
- **The benchmarks.**
  - Mandelbrot, NBody and Richards are ported from SOMns (`prior-art/somns/core-lib/Benchmarks`) into Primordial Soup's module shape, as stage 1's seven were: a module `class Mandelbrot usingPlatform: platform` whose one public method `bench` runs the workload once, checks the result and raises `Error signal:` when it differs.
  - `RichardsTon80` is Primordial Soup's `newspeak/Richards.ns`, already in that shape, renamed so that both Richards versions sit in one harness.
- **The kernel.** It gains:
  - mixed arithmetic: `Integer`'s and `LargeInteger`'s `+ - * < > <= >=` accept a `Float` argument, as `Float`'s already accept an integer one;
  - `Integer`'s and `LargeInteger`'s `bitOr:`, `bitXor:` and `<<` as VM-mirror leaves, and `Integer>>bitInvert` in Newspeak;
  - `Float>>sqrt` as a VM-mirror leaf.
- **The platform.** It does not change: no benchmark of this stage needs `platform system`.

**Tech Stack:** Rust 1.98.0, edition 2021; Newspeak in `crates/ns-runtime/core-library/` and `benchmarks/`; `cargo-nextest`; goldens under `goldens/`. No new dependency.

**Spec:** the design settled with the user on 2026-09-30, recorded in this plan. The background is in:
- `docs/OPEN-WORK.md`, "The road to a high-performance VM", stage 2 of "Before step 2's second plan, the Are We Fast Yet benchmarks";
- `docs/design/execution.md`, "The Are We Fast Yet benchmarks";
- `docs/design/core-library.md`, "What exists", "Large integers" and "Gap: an integer has no `/`";
- stage 1's plan, `docs/superpowers/plans/2026-09-30-1353-ns-next-are-we-fast-yet-stage-1.md`, whose module shape, header and harness this plan reuses.

The decisions, each the user's (2026-09-30):
1. **Protocol.** Stage 2 follows Primordial Soup's protocol, as stage 1 does. SOMns's number protocol differs in the division selectors, and the ports translate them:
   - SOMns's `//` is float division (`DoubleDivPrim`); in Primordial Soup and in the canonical Newspeak `/` is that division and `//` floors. Every SOMns `//` in Mandelbrot and NBody becomes `/`. Keeping `//` would floor every quotient silently.
   - SOMns's `/` on two integers truncates (`DividePrim`). Richards's `data control / 2` becomes `// 2`, which is the same for the non-negative control word.
   - `function value: message with: handle` becomes `value:value:`; `Array new:withAll:` becomes an allocation and a loop; `and:` with a Boolean argument becomes `&`; `self error:` becomes `Error signal:`.
   - SOMns's Richards traces through `system printString:` and `printNewline` only when `tracing` is true, which it never is. The tracing is left out, so no `platform system` protocol is needed.
2. **Both Richards versions.** The Are We Fast Yet Richards (SOMns's; 23246 queued packets, 9297 holds) and Primordial Soup's Ton80 Richards (1000 idle counts; 2322 and 928) both come in, as `Richards` and `RichardsTon80`.
3. **Workload sizes.** Mandelbrot and NBody run at the sizes Are We Fast Yet pins: Mandelbrot 750, which answers 50, and NBody 250000 steps, which answers an energy of `-0.1690859889909308`. The user chose these knowing the cost: one run takes about 4 s (Mandelbrot) and about 3 s (NBody) in a release build, and several times that in the debug build the goldens run in.
4. **Mandelbrot's licence.** SOMns's `Mandelbrot.ns` carries the Computer Language Benchmarks Game's BSD-3 text rather than SOMns's MIT. The user's instruction: copy it as it is. Its header keeps that text, as every adapted file keeps its upstream header; `THIRD-PARTY-NOTICES.md` gets no entry for it.

**A proposal the user has not confirmed.** Mixed arithmetic is the set in "The kernel" above, symmetric to what `Float` already does: an integer's `+ - * < > <= >=` convert it and a `Float` argument to doubles, as Primordial Soup's primitives do (`vm/primitives.cc`, `IS_FLOAT_OP`, `FLOAT_VALUE`). `=` stays class-tight (`1 = 1.0` is `false`) until the numeric tower, and `//` and `\\` stay integer-only because `Float` has neither. Task 7 records both as open.

## Global Constraints

- **The invariants.** Every change obeys `CLAUDE.md`, "Invariants that are easy to break". In particular:
  - The VM mirror is a capability. The new leaves are reached only from `Kernel.ns`, which is resolved with `resolve_kernel`. None of its six locks changes.
  - `vm.rs` gets no dispatch logic. Nothing in this plan touches `vm.rs` or the compiler. An inlined `IntegerOperation` with a `Float` operand already falls back to the send, and the send reaches the leaf this plan widens.
  - No wire tag is added, renumbered or retired. A mirror row has no wire tag, and none of `bitOr:`, `bitXor:` or `<<` joins `IntegerOperation`.
- **Primordial Soup's protocol.** Every method the kernel gains exists in Primordial Soup's `KernelForPrimordialSoup.ns`, with the same selector and meaning: `<<` (line 1599), `bitInvert` (1635), `bitOr:` (1639), `bitXor:` (1644) and `Float>>sqrt` (1145). Where our body differs, a comment says why.
- **No carve-outs.** Every integer leaf handles both representations, and a `SmallInteger` meeting a `LargeInteger` in either position. Mixed arithmetic works for a `LargeInteger` receiver as for an `Integer` one.
- **Leaf errors.**
  - A leaf declines (`None`) only for a receiver of the wrong shape, as every number leaf does today.
  - A wrong argument raises the type mismatch, which names what the leaf takes: "a numeric argument" for arithmetic and ordering, "an integer argument" for `//`, `\\` and the bit operations.
  - A negative shift count and a shift whose result cannot be allocated raise messages of their own.
  - A size the program chooses is never a Rust allocation abort: `<<` reserves with `try_reserve_exact`, as `Array new:` does (`builtins/collections.rs`, `allocate`).
- **Names are written out, in Newspeak too** (`CLAUDE.md`, "Conventions"). The ports rename SOMns's and Primordial Soup's `x`, `y`, `z`, `vx`, `dx`, `dt`, `e`, `b`, `i`, `j`, `t`, `v`, `s`, `l`, `k`, `p`, `q`, `tcb`, `ctrl`, `cnt`, `RBObject` and the capitalised Ton80 constants.
- **Comments.**
  - A comment explains the decision, not the mechanics.
  - A golden's rationale is a `(* … *)` comment under its category string. The `.toml` holds only the recorded answer.
- **Writing `.ns` files.** Write them with the editor's Write/Edit tool, never through a shell heredoc or `sed`. The shell here swallows one backslash of `\\`, which silently turns `\\` into `\`.
- **Goldens.** A golden that changes is a behaviour change. This plan changes exactly one on purpose: `goldens/errors/a_large_integer_meets_a_float` becomes `a_large_integer_meets_nil` in Task 1, because a `LargeInteger` receiver now answers a `Float` argument. If any other golden's output changes, stop and report it.
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

  The end-of-batch gates (three stress modes, `run-corpus` plain and under stress, the `ns-lsp` suite) run once, in Task 8. That staggering is deliberate: a commit before Task 8 is work in progress. The JetBrains plugin and `ns-syntax` are untouched.
- **The suite gets slower, on purpose.** From Task 5 on, `goldens/awfy/all_benchmarks_verify` runs Mandelbrot and NBody at full size, under both lowerings and through `build`/`load` (`golden_round_trips.rs`), and `a_timed_run_prints_a_score_for_every_benchmark` runs each of them at least twice. Expect roughly a minute more per suite run in the debug build, and nextest marking these tests SLOW. That is not a failure.

## Review Focus

- **NaN on the integer side.** `1 < (0.0 / 0.0)` and `1 > (0.0 / 0.0)` answer `false`, as `Float`'s ordering does with a NaN. An ordering that compares with `partial_cmp(…).unwrap()` panics, and one that compares `Ordering` values treats NaN as equal. Pinned in Task 1.
- **Precision beyond 2^53.** `9007199254740993 + 0.0` answers the double `9007199254740992`: the integer is converted first, ties to even, as `Float`'s leaves convert an integer argument. Pinned in Task 1.
- **`<<` at the `i64` boundary.** `-9223372036854775808 << 1` is `-18446744073709551616`, and `-1 << 63` is exactly `i64::MIN`, still an `Integer`. A shift computed in `i64`, or checked with `checked_shl` (which checks only the count), gets the first wrong. Pinned in Task 3.
- **`<<` with a count the program cannot afford.** `1 << 9223372036854775807` raises "cannot allocate", with no abort; `0 << 18446744073709551616` answers `0` without allocating; `1 << -1` raises "non-negative". Pinned in Task 3.
- **`bitOr:` and `bitXor:` with a negative `LargeInteger`.** They are defined over two's complement, as `bitAnd:` is: `(0 - 18446744073709551617) bitXor: (0 - 18446744073709551617)` is `0`, and `-1 bitXor: 18446744073709551616` is `-18446744073709551617`. A sign-and-magnitude operation gets both wrong. Pinned in Task 2 against `i128`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/ns-runtime/src/builtins/numbers.rs` | mixed arithmetic in the integer leaves; `Accepted` and `type_mismatch`; the `bitOr:`, `bitXor:`, `<<` and `sqrt` leaves; `shift_left` |
| `crates/ns-runtime/src/builtins/primitive.rs` | the new enum variants and `MIRROR` rows, classified in `answers_false_to_a_stranger` |
| `crates/ns-runtime/src/arbitrary_integer/mod.rs` | `bitwise`, `bit_or`, `bit_xor`, `shift_left` and their unit tests |
| `crates/ns-runtime/core-library/Kernel.ns` | the forwarders, `Integer>>bitInvert`, `Float>>sqrt`, `Number`'s comment |
| `crates/ns-runtime/tests/builtins_boolean_numbers.rs`, `tests/vm.rs`, `src/interpreter/mirror.rs` (tests) | the type-mismatch expectations mixed arithmetic changes |
| `goldens/arithmetic/`, `goldens/kernel/`, `goldens/errors/` | a golden for each new kernel method and error |
| `benchmarks/are-we-fast-yet/Mandelbrot.ns`, `NBody.ns`, `Richards.ns`, `RichardsTon80.ns` (new) | the four benchmarks |
| `benchmarks/are-we-fast-yet/AreWeFastYet.ns`, `are-we-fast-yet.json`, `goldens/awfy/*.json`, `all_benchmarks_verify.toml` | the harness's tuple and the projects name them |
| `crates/ns-cli/tests/run.rs` | `--timed`'s expected names |
| `THIRD-PARTY-NOTICES.md` | the introduction names Primordial Soup's Richards |
| `docs/…`, `CLAUDE.md` | Task 7 |

---

### Task 1: an integer meets a `Float`

An integer's `+ - * < > <= >=` convert the receiver and a `Float` argument to doubles, exactly as `Float`'s leaves convert an integer argument (`execute_float`'s `argument_float`), so `4 * 3.5` and `3.5 * 4` are the same double operation. NBody sends `4 * self piValue`, and `Number>>to:do:` with an `Integer` receiver and a `Float` bound sends `Integer>><=` with a `Float` argument. `=` stays class-tight, and `//` and `\\` stay integer-only.

The type mismatch today derives its wording from the receiver's class: "an integer argument" for every integer leaf. After this task an integer's arithmetic takes any number, so the wording comes from the operation instead (`Accepted`).

**Files:**
- Modify: `crates/ns-runtime/src/builtins/numbers.rs` (the module note, `type_mismatch`, `execute_integer`, `execute_large_integer`, `execute_float`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`class Number`'s comment only)
- Modify: `crates/ns-runtime/tests/builtins_boolean_numbers.rs`, `crates/ns-runtime/tests/vm.rs`, `crates/ns-runtime/src/interpreter/mirror.rs` (tests only)
- Create: `goldens/arithmetic/an_integer_meets_a_float.ns`, `.toml`
- Create: `goldens/kernel/an_integer_counts_to_a_float_bound.ns`, `.toml`
- Rename: `goldens/errors/a_large_integer_meets_a_float.{ns,toml}` → `goldens/errors/a_large_integer_meets_nil.{ns,toml}`

**Interfaces:**
- Produces:
  - `enum Accepted { Integer, Number }` in `numbers.rs`, private;
  - `type_mismatch(heap, receiver, selector, accepted: Accepted, got)`, which Tasks 2 and 3 call with `Accepted::Integer`.

- [ ] **Step 1: Write the goldens.**

`goldens/arithmetic/an_integer_meets_a_float.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* An `Integer` or `LargeInteger` receiver meets a `Float` argument as a
   `Float` receiver meets an integer one: both become doubles, as Primordial
   Soup's primitives convert them, so the integer is rounded first. `=`
   stays class-tight until the numeric tower, and a NaN is ordered with
   nothing. *)
class IntegerMeetsFloat = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          nan = 0.0 / 0.0.
          big = 18446744073709551616. |
        transcript print: 3 + 1.5; newLine.
        transcript print: 3 - 0.5; newLine.
        transcript print: 4 * 3.5; newLine.
        transcript print: 3 < 3.5; newLine.
        transcript print: 3 > 3.5; newLine.
        transcript print: 3 <= 3.0; newLine.
        transcript print: 3 >= 3.5; newLine.
        transcript print: 3 = 3.0; newLine.
        transcript print: 1 < nan; newLine.
        transcript print: 1 > nan; newLine.
        transcript print: 9007199254740993 + 0.0; newLine.
        transcript print: big * 0.5; newLine.
        transcript print: big < 1.5; newLine.
        transcript print: big negated < 1.5; newLine.
        transcript print: big - 0.5 = big asFloat; newLine
    )
)
```
`goldens/arithmetic/an_integer_meets_a_float.toml`:
```toml
stdout = '''
4.5
2.5
14
true
false
true
false
false
false
false
9007199254740992
9223372036854776000
false
true
true
'''
```
`Float>>printString` is Rust's `f64` `Display`: `14.0` prints `14`, and 2^63 prints `9223372036854776000` (a probe on 2026-09-30 printed `9223372036854775808 asFloat` so). If a line differs, check it against that rule before touching the program.

`goldens/kernel/an_integer_counts_to_a_float_bound.ns`:
```newspeak
Newspeak3
'Kernel'
(* `to:do:` and `to:by:do:` with an `Integer` receiver and a `Float` bound
   or step: `Number`'s loop compares and adds across the two classes, and
   the counter becomes a `Float` once a `Float` step is added. *)
class IntegerCountsToFloat = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. |
        1 to: 2.5 do: [:index | transcript print: index; newLine].
        1 to: 2 by: 0.5 do: [:value | transcript print: value; newLine]
    )
)
```
`goldens/kernel/an_integer_counts_to_a_float_bound.toml`:
```toml
stdout = '''
1
2
1
1.5
2
'''
```

Rename the error golden with `git mv`, then rewrite it. `goldens/errors/a_large_integer_meets_nil.ns`:
```newspeak
Newspeak3
'T'
(* A `LargeInteger` receiver meets an argument that is no number with its
   leaf's type mismatch, which names what arithmetic takes: neither an
   overflow nor a fall through to `doesNotUnderstand:`. *)
class LargeIntegerMeetsNil = () (
    public main: platform args: arguments = (
        platform transcript show: (9223372036854775807 + 1 + nil) printString; newLine
    )
)
```
`goldens/errors/a_large_integer_meets_nil.toml`:
```toml
status = 1
stderr = '''
Uncaught Newspeak error: + requires a numeric argument, got nil
'''
```

- [ ] **Step 2: Run them and see them fail.**

Run: `cargo nextest run -p ns-cli --test run_goldens an_integer_meets_a_float an_integer_counts_to a_large_integer_meets_nil`
Expected: FAIL. The first two end in `requires an integer argument, got 1.5` (or `got 2.5`), and the third says `requires an integer argument, got nil`.

- [ ] **Step 3: Name what a leaf accepts.**

In `numbers.rs`, replace `type_mismatch` and its doc comment:
```rust
/// What a numeric leaf takes as its argument, which its type mismatch names.
#[derive(Clone, Copy)]
enum Accepted {
    /// `//`, `\\` and the bit operations: an `Integer` or a `LargeInteger`.
    Integer,
    /// Arithmetic and ordering: any of the three number classes.
    Number,
}

/// A numeric operation on an argument it does not accept (this module's
/// note says which those are). The message names what the operation takes,
/// not what the receiver is: an integer's `+` takes any number, its `//`
/// only an integer.
fn type_mismatch<'gc>(
    heap: &Heap<'gc>,
    receiver: &Value<'gc>,
    selector: Symbol,
    accepted: Accepted,
    got: Option<&Value<'gc>>,
) -> NewspeakError<'gc> {
    let required = match accepted {
        Accepted::Integer => "an integer",
        Accepted::Number => "a numeric",
    };
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!(
            "{selector} requires {required} argument, got {}",
            heap.describe_argument(got)
        ),
    }
}
```
Add, below `integer_operand`:
```rust
/// A `Float` argument, which an integer's arithmetic and ordering take by
/// converting the receiver to a double (this module's note).
fn float_operand<'gc>(value: Option<&Value<'gc>>) -> Option<f64> {
    match value {
        Some(Value::Float(float)) => Some(*float),
        _ => None,
    }
}
```

- [ ] **Step 4: Widen `execute_integer`.**

Change its `mismatch` closure to take the accepted kind:
```rust
    let mismatch = |operator: &str, accepted: Accepted| {
        type_mismatch(
            &mutator.heap,
            &receiver,
            Symbol::intern(operator),
            accepted,
            arguments.first(),
        )
    };
```
Give `arithmetic` a `float` operation and a `Float` branch after the `LargeInteger` one. Its comment gains one sentence: "A `Float` argument makes it a double operation on the converted receiver, which allocates nothing either."
```rust
    let arithmetic =
        |operator: &str,
         checked: fn(i64, i64) -> Option<i64>,
         wide: fn(i128, i128) -> i128,
         large: fn(&ArbitraryInteger, &ArbitraryInteger) -> ArbitraryInteger,
         float: fn(f64, f64) -> f64| {
            match argument_integer() {
                Some(argument) => Ok(checked(receiver_integer, argument)
                    .map(Value::SmallInteger)
                    .ok_or_else(|| {
                        ArbitraryInteger::from_i128(wide(
                            i128::from(receiver_integer),
                            i128::from(argument),
                        ))
                    })),
                None => match large_argument() {
                    Some(argument) => Ok(Err(large(
                        &ArbitraryInteger::from_i64(receiver_integer),
                        &argument,
                    ))),
                    None => match float_operand(arguments.first()) {
                        Some(argument) => {
                            Ok(Ok(Value::Float(float(receiver_integer as f64, argument))))
                        }
                        None => Err(mismatch(operator, Accepted::Number)),
                    },
                },
            }
        };
```
Give `compare` the same `Float` branch. `partial_cmp` answers `None` for a NaN, and `is_some_and` turns that into `false`, as `Float`'s own ordering answers:
```rust
    let compare = |operator: &str, operation: fn(Ordering) -> bool| match argument_integer() {
        Some(argument) => Ok(Value::Boolean(operation(receiver_integer.cmp(&argument)))),
        None => match large_argument() {
            Some(argument) => Ok(Value::Boolean(operation(
                ArbitraryInteger::from_i64(receiver_integer).cmp(&argument),
            ))),
            None => match float_operand(arguments.first()) {
                Some(argument) => Ok(Value::Boolean(
                    (receiver_integer as f64)
                        .partial_cmp(&argument)
                        .is_some_and(operation),
                )),
                None => Err(mismatch(operator, Accepted::Number)),
            },
        },
    };
```
Pass the double operation to each `arithmetic` call: `|left, right| left + right` for `Add`, `left - right` for `Subtract`, `left * right` for `Multiply`, as the last argument. The `//`, `\\` and `bitAnd:` arms call `mismatch("//", Accepted::Integer)`, `mismatch("\\\\", Accepted::Integer)` and `mismatch("bitAnd:", Accepted::Integer)`.

- [ ] **Step 5: Widen `execute_large_integer` the same way.**

Its `mismatch` takes `accepted` as above. Its `arithmetic` and `compare` gain the `Float` branch, with `receiver_integer.to_f64()` as the converted receiver:
```rust
    let arithmetic =
        |operator: &str,
         operation: fn(&ArbitraryInteger, &ArbitraryInteger) -> ArbitraryInteger,
         float: fn(f64, f64) -> f64| {
            match argument() {
                Some(argument) => Ok(Ok(operation(&receiver_integer, &argument))),
                None => match float_operand(arguments.first()) {
                    Some(argument) => Ok(Err(Value::Float(float(receiver_integer.to_f64(), argument)))),
                    None => Err(mismatch(operator, Accepted::Number)),
                },
            }
        };
    let compare = |operator: &str, operation: fn(Ordering) -> bool| match argument() {
        Some(argument) => Ok(Value::Boolean(operation(receiver_integer.cmp(&argument)))),
        None => match float_operand(arguments.first()) {
            Some(argument) => Ok(Value::Boolean(
                receiver_integer
                    .to_f64()
                    .partial_cmp(&argument)
                    .is_some_and(operation),
            )),
            None => Err(mismatch(operator, Accepted::Number)),
        },
    };
```
`arithmetic` now answers `Ok(Ok(integer))` for an integer result, to be made a `Value`, and `Ok(Err(value))` for a double that already is one. Rewrite the three arms to match:
```rust
        LargeIntegerPrimitive::Add => Some(
            arithmetic("+", ArbitraryInteger::add, |left, right| left + right).map(|answer| {
                answer.map_or_else(|value| value, |integer| mutator.new_integer(integer))
            }),
        ),
```
and the same for `Subtract` (`-`) and `Multiply` (`*`). If the borrow checker refuses the `mutator` use inside `map`, bind the result first (`let answer = arithmetic(…);`) and match on it. The `//`, `\\` and `bitAnd:` arms pass `Accepted::Integer`.

- [ ] **Step 6: `execute_float` names what it accepts.**

Its closure keeps one argument and always passes `Accepted::Number`, so `Float`'s messages do not change:
```rust
    let mismatch = |operator: &str| {
        type_mismatch(
            &mutator.heap,
            &receiver,
            Symbol::intern(operator),
            Accepted::Number,
            arguments.first(),
        )
    };
```

- [ ] **Step 7: Rewrite the module note.**

In `numbers.rs`'s module note, replace the third bullet ("An integer meeting a `Float` argument is a type mismatch: …") with:
```rust
//! * An integer's arithmetic and ordering meet a `Float` argument the same
//!   way, converting the receiver (`4 * 3.5` is `14.0`), as Primordial
//!   Soup's primitives do. `//`, `\\` and the bit operations take integers
//!   only; `Float` has none of them.
```
and replace the closing paragraph ("`Fraction`, a `Number` protocol and the coercion between integers and floats are still to come …") with:
```rust
//! `Fraction`, `Integer>>/` and cross-class `=` are still to come, with the
//! numeric tower (`docs/design/core-library.md`, "Gap: an integer has no
//! `/`").
```

- [ ] **Step 8: `Number`'s comment.**

In `Kernel.ns`, the comment above `public class Number` ends with a reason that is no longer true. Replace its last sentence, from "The increment is `current + step`" to "must still count.", with nothing: the comment ends after "ours sends them." The code does not change.

- [ ] **Step 9: Update the tests that pinned the old mismatch.**

In `crates/ns-runtime/tests/builtins_boolean_numbers.rs`:
- Replace `mixed_integer_float_arithmetic_reports_a_type_mismatch_not_an_overflow` and its doc comment with:
  ```rust
  /// An integer meets a `Float` argument as a `Float` meets an integer one:
  /// `1 + 1.5` is the double `2.5` (`builtins/numbers.rs`'s module note).
  #[test]
  fn integer_arithmetic_widens_a_float_argument() {
      let mut interpreter = Interpreter::bootstrap();

      let one = interpreter.stash(Value::SmallInteger(1));
      let result = interpreter.send(
          &one,
          Symbol::intern("+"),
          &[interpreter.stash(Value::Float(1.5))],
      );

      let answer = expect_ok(result, "1 + 1.5");
      interpreter.enter(|mutator| match mutator.fetch(&answer) {
          Value::Float(sum) => assert_eq!(sum, 2.5),
          other => panic!(
              "expected the double 2.5, got {}",
              mutator.heap().describe(&other)
          ),
      });
  }
  ```
- In `integer_arithmetic_with_an_object_argument_is_an_error_not_a_stack_overflow`, the expected prefix becomes `"+ requires a numeric argument, got "`.
- In `an_ordering_comparison_against_a_mismatched_type_is_an_error_not_false`, send `nil` instead of `1.5` (`interpreter.stash(Value::Nil)`), and expect `"< requires a numeric argument, got nil"`. Its doc comment says "a type-mismatched argument"; keep it.
- In `a_float_receiver_asks_for_a_numeric_argument`, the doc comment becomes: "A `Float` receiver takes any number, so its mismatch asks for a number, as an integer's arithmetic does too; only `//`, `\\` and the bit operations ask for an integer."

In `crates/ns-runtime/src/interpreter/mirror.rs`, `an_error_a_leaf_raises_names_the_reported_selector`: the prefix becomes `"+ requires a numeric argument"`.

In `crates/ns-runtime/tests/vm.rs`, `an_inlined_integer_operation_with_a_non_integer_operand_falls_back_to_the_send`: the expected substring becomes `"requires a numeric argument"`.

Then search for any other expectation of the old text: `rg "requires an integer argument" crates goldens corpus`. Every remaining hit must be a `//`, `\\` or `bitAnd:` case, which keeps "an integer". Change any other hit the same way and name it in the commit message.

- [ ] **Step 10: Run the goldens and the per-commit gate.**

Run: `cargo nextest run -p ns-cli --test run_goldens an_integer_meets_a_float an_integer_counts_to a_large_integer_meets_nil`, then the full per-commit gate.
Expected: PASS.

- [ ] **Step 11: Commit.**

```bash
git add -A crates/ns-runtime goldens/arithmetic/an_integer_meets_a_float.* goldens/kernel/an_integer_counts_to_a_float_bound.* goldens/errors/
git commit -m "feat: an integer's arithmetic and ordering meet a Float argument"
```
The commit body says that `a_large_integer_meets_a_float` became `a_large_integer_meets_nil` because that program now answers, and that the mismatch text of an integer's arithmetic changed from "an integer" to "a numeric".

---

### Task 2: `bitOr:`, `bitXor:` and `bitInvert`

Richards and Mandelbrot send `bitXor:`, and RichardsTon80 sends `bitOr:` and `bitInvert`. All three are Primordial Soup's. `bitOr:` and `bitXor:` are leaves over two's complement, like `bitAnd:` (`ArbitraryInteger::bit_and`); `bitInvert` is Primordial Soup's Newspeak body, `^-1 - self`, on `Integer`, which `LargeInteger` inherits.

**Files:**
- Modify: `crates/ns-runtime/src/arbitrary_integer/mod.rs` (`bitwise`, `bit_or`, `bit_xor`, a unit test)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs`, `crates/ns-runtime/src/builtins/primitive.rs`, `crates/ns-runtime/core-library/Kernel.ns`
- Create: `goldens/arithmetic/bit_or_and_bit_xor_are_twos_complement.ns`, `.toml`

**Interfaces:**
- Consumes: `Accepted::Integer` (Task 1).
- Produces:
  - `ArbitraryInteger::bit_or(&self, other: &ArbitraryInteger) -> ArbitraryInteger`;
  - `ArbitraryInteger::bit_xor(&self, other: &ArbitraryInteger) -> ArbitraryInteger`;
  - `IntegerPrimitive::{BitOr, BitXor}`, `LargeIntegerPrimitive::{BitOr, BitXor}`;
  - `Integer>>bitOr:`, `Integer>>bitXor:`, `Integer>>bitInvert`, `LargeInteger>>bitOr:`, `LargeInteger>>bitXor:`.

- [ ] **Step 1: Write the failing unit test**, in `arbitrary_integer/mod.rs`'s tests, after `bit_and_is_twos_complement_and_agrees_with_i128`. `i128`'s `|` and `^` are the oracle:
```rust
    #[test]
    fn bit_or_and_bit_xor_are_twos_complement_and_agree_with_i128() {
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
                let left_integer = ArbitraryInteger::from_i128(left);
                let right_integer = ArbitraryInteger::from_i128(right);
                assert_eq!(
                    left_integer.bit_or(&right_integer).to_decimal_string(),
                    (left | right).to_string(),
                    "{left} bitOr: {right}"
                );
                assert_eq!(
                    left_integer.bit_xor(&right_integer).to_decimal_string(),
                    (left ^ right).to_string(),
                    "{left} bitXor: {right}"
                );
            }
        }
    }
```

- [ ] **Step 2: Run it and see it fail to compile.**

Run: `cargo nextest run -p ns-runtime bit_or`
Expected: FAIL, because `bit_or` is not found.

- [ ] **Step 3: Generalise `bit_and`.**

Replace `bit_and` and its doc comment with:
```rust
    /// Bitwise and over two's complement, where a negative number's sign
    /// bits extend without end (Primordial Soup's and Squeak's `bitAnd:`).
    pub fn bit_and(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        self.bitwise(other, |left, right| left & right)
    }

    /// Bitwise inclusive or, over two's complement as `bit_and`.
    pub fn bit_or(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        self.bitwise(other, |left, right| left | right)
    }

    /// Bitwise exclusive or, over two's complement as `bit_and`.
    pub fn bit_xor(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        self.bitwise(other, |left, right| left ^ right)
    }

    /// One digit beyond the longer magnitude holds the sign, so the top bit
    /// of the result's top digit says whether it is negative: every one of
    /// `&`, `|` and `^` applied to two sign-extended digits answers the
    /// result's own sign extension.
    fn bitwise(&self, other: &ArbitraryInteger, operation: fn(u64, u64) -> u64) -> ArbitraryInteger {
        let length = self.magnitude.len().max(other.magnitude.len()) + 1;
        let digits: Vec<u64> = self
            .twos_complement_digits(length)
            .iter()
            .zip(other.twos_complement_digits(length))
            .map(|(left_digit, right_digit)| operation(*left_digit, right_digit))
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
```

- [ ] **Step 4: Run the unit tests.**

Run: `cargo nextest run -p ns-runtime bit_`
Expected: PASS, `bit_and`'s test included.

- [ ] **Step 5: Write the golden.**

`goldens/arithmetic/bit_or_and_bit_xor_are_twos_complement.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* `bitOr:` and `bitXor:` are Primordial Soup's: operations over two's
   complement, where a negative number's sign bits extend without end, for
   either representation in either position. `bitInvert` is Primordial
   Soup's `-1 - self`. Each line is a pair's `bitOr:`, then its `bitXor:`;
   the last six are `bitInvert`s. Richards and Mandelbrot send them. *)
class BitOrAndXor = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          two64 = 18446744073709551616.
          pairs = {
            {12. 10}. {-1. 255}. {-256. 255}.
            {-9223372036854775808. -1}.
            {two64 negated. two64 - 1}. {-1. two64}. {two64 + 5. -2}.
            {two64 negated - 1. two64 negated - 1} }. |
        pairs do: [:pair |
            transcript
                print: ((pair at: 1) bitOr: (pair at: 2)); show: ' ';
                print: ((pair at: 1) bitXor: (pair at: 2)); newLine].
        {0. 5. -6. 9223372036854775807. -9223372036854775808. two64} do: [:value |
            transcript print: value bitInvert; newLine]
    )
)
```
`goldens/arithmetic/bit_or_and_bit_xor_are_twos_complement.toml`. Every line was computed with Python's `|`, `^` and `-1 - value`, whose integers are two's complement with the sign extended without end. Recompute them the same way before recording, and record Python's answer wherever it differs from this plan's:
```toml
stdout = '''
14 6
-1 -256
-1 -1
-1 9223372036854775807
-1 -1
-1 -18446744073709551617
-1 -18446744073709551621
-18446744073709551617 0
-1
-6
5
-9223372036854775808
9223372036854775807
-18446744073709551617
'''
```

- [ ] **Step 6: Add the leaves and forwarders.**

In `primitive.rs`:
- Add `BitOr` and `BitXor` after `BitAnd` to `IntegerPrimitive` and `LargeIntegerPrimitive`, and put each in its family's `=> false` group of `answers_false_to_a_stranger`.
- Append these rows at the end of `MIRROR`:
```rust
    mirror(
        "integer:bitOr:ifFail:",
        "bitOr:",
        Primitive::Integer(IntegerPrimitive::BitOr),
    ),
    mirror(
        "largeInteger:bitOr:ifFail:",
        "bitOr:",
        Primitive::LargeInteger(LargeIntegerPrimitive::BitOr),
    ),
    mirror(
        "integer:bitXor:ifFail:",
        "bitXor:",
        Primitive::Integer(IntegerPrimitive::BitXor),
    ),
    mirror(
        "largeInteger:bitXor:ifFail:",
        "bitXor:",
        Primitive::LargeInteger(LargeIntegerPrimitive::BitXor),
    ),
```
In `numbers.rs`, `execute_integer`, after the `BitAnd` arm. Two `i64`s' `|` and `^` always fit, since `i64` is two's complement with its sign extended:
```rust
        IntegerPrimitive::BitOr => Some(match argument_integer() {
            Some(argument) => Ok(Value::SmallInteger(receiver_integer | argument)),
            None => match large_argument() {
                Some(argument) => Ok(mutator
                    .new_integer(ArbitraryInteger::from_i64(receiver_integer).bit_or(&argument))),
                None => Err(mismatch("bitOr:", Accepted::Integer)),
            },
        }),
        IntegerPrimitive::BitXor => Some(match argument_integer() {
            Some(argument) => Ok(Value::SmallInteger(receiver_integer ^ argument)),
            None => match large_argument() {
                Some(argument) => Ok(mutator
                    .new_integer(ArbitraryInteger::from_i64(receiver_integer).bit_xor(&argument))),
                None => Err(mismatch("bitXor:", Accepted::Integer)),
            },
        }),
```
In `execute_large_integer`, after its `BitAnd` arm:
```rust
        LargeIntegerPrimitive::BitOr => Some(match argument() {
            Some(argument) => Ok(mutator.new_integer(receiver_integer.bit_or(&argument))),
            None => Err(mismatch("bitOr:", Accepted::Integer)),
        }),
        LargeIntegerPrimitive::BitXor => Some(match argument() {
            Some(argument) => Ok(mutator.new_integer(receiver_integer.bit_xor(&argument))),
            None => Err(mismatch("bitXor:", Accepted::Integer)),
        }),
```
In `Kernel.ns`, `class Integer`, after `bitAnd:`:
```newspeak
        public bitOr: other = ( ^vmMirror integer: self bitOr: other ifFail: [^super bitOr: other] )
        public bitXor: other = ( ^vmMirror integer: self bitXor: other ifFail: [^super bitXor: other] )
        public bitInvert = ( ^-1 - self )
```
and in `class LargeInteger`, after `bitAnd:`:
```newspeak
        public bitOr: other = ( ^vmMirror largeInteger: self bitOr: other ifFail: [^super bitOr: other] )
        public bitXor: other = ( ^vmMirror largeInteger: self bitXor: other ifFail: [^super bitXor: other] )
```

- [ ] **Step 7: Run the golden and the per-commit gate.**

Run: `cargo nextest run -p ns-cli --test run_goldens bit_or_and_bit_xor`, then the gate.
Expected: PASS. A kernel test that lists `Integer`'s members or fast paths may need the new selectors; extend such a list with them and nothing else.

- [ ] **Step 8: Commit.**

```bash
git commit -m "feat: Integer and LargeInteger answer bitOr:, bitXor: and bitInvert over two's complement"
```

---

### Task 3: `<<`

Mandelbrot sends `<<`. It is Primordial Soup's primitive 23 (`vm/primitives.cc`, `Integer_bitShiftLeft`): the receiver times two to the count, widening into a large integer, and failing, so that the method raises, for a negative count. Primordial Soup answers a count too large for memory with an out-of-memory error; ours raises "cannot allocate", found with `try_reserve_exact` before anything is written, as `Array new:` does.

**Files:**
- Modify: `crates/ns-runtime/src/arbitrary_integer/mod.rs` (`shift_left`, two unit tests)
- Modify: `crates/ns-runtime/src/builtins/numbers.rs` (`shift_left`, two arms), `primitive.rs`, `core-library/Kernel.ns`
- Create: `goldens/arithmetic/shift_left_widens_into_a_large_integer.ns`, `.toml`
- Create: `goldens/errors/shift_left_by_a_negative_count.ns`, `.toml`
- Create: `goldens/errors/shift_left_beyond_what_can_be_allocated.ns`, `.toml`

**Interfaces:**
- Consumes: `type_mismatch`, `Accepted::Integer` (Task 1).
- Produces:
  - `ArbitraryInteger::shift_left(&self, count: u64) -> Option<ArbitraryInteger>`, `None` when the result cannot be allocated;
  - `IntegerPrimitive::ShiftLeft`, `LargeIntegerPrimitive::ShiftLeft`;
  - `Integer>><<`, `LargeInteger>><<`.

- [ ] **Step 1: Write the failing unit tests**, in `arbitrary_integer/mod.rs`'s tests:
```rust
    #[test]
    fn shift_left_multiplies_by_a_power_of_two_and_agrees_with_i128() {
        let samples: [i128; 7] = [
            0,
            1,
            -1,
            3,
            -12345,
            i128::from(i64::MAX),
            i128::from(i64::MIN),
        ];
        for value in samples {
            for count in [0_u64, 1, 13, 63, 64] {
                assert_eq!(
                    ArbitraryInteger::from_i128(value)
                        .shift_left(count)
                        .expect("a small shift allocates")
                        .to_decimal_string(),
                    (value << count).to_string(),
                    "{value} << {count}"
                );
            }
        }
    }

    #[test]
    fn shift_left_refuses_a_result_that_cannot_be_allocated() {
        assert!(ArbitraryInteger::from_i64(1).shift_left(u64::MAX).is_none());
        assert!(ArbitraryInteger::zero().shift_left(u64::MAX).is_some());
    }
```
Every sample shifted by at most 64 fits `i128`: `i64::MAX << 64` is `2^127 - 2^64`, and `i64::MIN << 64` is `i128::MIN`.

- [ ] **Step 2: Run them and see them fail to compile.**

Run: `cargo nextest run -p ns-runtime shift_left`
Expected: FAIL, because `shift_left` is not found.

- [ ] **Step 3: Implement it**, after `bit_xor`:
```rust
    /// The receiver times two to the `count`: the magnitude shifted, the
    /// sign kept, which is exact for a negative receiver too. `None` when
    /// the result's digits cannot be reserved, so a count the program
    /// chose is never a Rust allocation abort.
    pub fn shift_left(&self, count: u64) -> Option<ArbitraryInteger> {
        if self.is_zero() {
            return Some(ArbitraryInteger::zero());
        }
        let digit_shift = usize::try_from(count / u64::from(u64::BITS)).ok()?;
        let bit_shift = (count % u64::from(u64::BITS)) as u32;
        let length = digit_shift.checked_add(self.magnitude.len())?.checked_add(1)?;
        let mut magnitude: Vec<u64> = Vec::new();
        magnitude.try_reserve_exact(length).ok()?;
        magnitude.resize(digit_shift, 0);
        let mut carry = 0;
        for digit in self.magnitude.iter() {
            if bit_shift == 0 {
                magnitude.push(*digit);
            } else {
                magnitude.push((digit << bit_shift) | carry);
                carry = digit >> (u64::BITS - bit_shift);
            }
        }
        magnitude.push(carry);
        Some(ArbitraryInteger::from_parts(self.negative, magnitude))
    }
```

- [ ] **Step 4: Run the unit tests.**

Run: `cargo nextest run -p ns-runtime shift_left`
Expected: PASS.

- [ ] **Step 5: Write the goldens.**

`goldens/arithmetic/shift_left_widens_into_a_large_integer.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* `<<` is Primordial Soup's: the receiver times two to the count, for
   either representation in either position, widening into a
   `LargeInteger` when the result does not fit and back when it does. A
   zero receiver answers zero for any count. Mandelbrot shifts with it. *)
class ShiftLeft = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          two64 = 18446744073709551616.
          pairs = {
            {1. 3}. {5. 0}. {-3. 2}. {1. 62}. {1. 63}. {-1. 63}. {-1. 64}.
            {3. 64}. {-9223372036854775808. 1}. {two64. 1}. {two64 negated. 65}.
            {0. 100}. {0. two64} }. |
        pairs do: [:pair |
            transcript print: ((pair at: 1) << (pair at: 2)); newLine]
    )
)
```
`goldens/arithmetic/shift_left_widens_into_a_large_integer.toml`, computed with Python's `<<`; recompute before recording:
```toml
stdout = '''
8
5
-12
4611686018427387904
9223372036854775808
-9223372036854775808
-18446744073709551616
55340232221128654848
-18446744073709551616
36893488147419103232
-680564733841876926926749214863536422912
0
0
'''
```
`goldens/errors/shift_left_by_a_negative_count.ns`:
```newspeak
Newspeak3
'T'
(* A negative count is refused, as Primordial Soup's primitive fails for it
   and its method raises. *)
class NegativeShift = () (
    public main: platform args: arguments = (
        platform transcript print: 1 << -1; newLine
    )
)
```
`goldens/errors/shift_left_by_a_negative_count.toml`:
```toml
status = 1
stderr = '''
Uncaught Newspeak error: << takes a non-negative shift count, got -1
'''
```
`goldens/errors/shift_left_beyond_what_can_be_allocated.ns`:
```newspeak
Newspeak3
'T'
(* A count whose result cannot be allocated is an error the program sees,
   never a Rust allocation abort, as `Array new:` refuses such a size. *)
class HugeShift = () (
    public main: platform args: arguments = (
        platform transcript print: 1 << 9223372036854775807; newLine
    )
)
```
`goldens/errors/shift_left_beyond_what_can_be_allocated.toml`:
```toml
status = 1
stderr = '''
Uncaught Newspeak error: << cannot allocate the result of shifting by 9223372036854775807 bits
'''
```

- [ ] **Step 6: Add the leaves and forwarders.**

In `primitive.rs`, add `ShiftLeft` after `BitXor` to both families and to their `=> false` groups, and append:
```rust
    mirror(
        "integer:shiftLeft:ifFail:",
        "<<",
        Primitive::Integer(IntegerPrimitive::ShiftLeft),
    ),
    mirror(
        "largeInteger:shiftLeft:ifFail:",
        "<<",
        Primitive::LargeInteger(LargeIntegerPrimitive::ShiftLeft),
    ),
```
In `numbers.rs`, after `division_by_zero`:
```rust
/// `<<` for either integer class, once the fast path of a `SmallInteger`
/// shifted by less than 64 is ruled out: Primordial Soup's primitive 23,
/// which fails for a negative count so that its method raises. A result
/// that cannot be allocated is an error, as `Array new:`'s is, never an
/// abort.
fn shift_left<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    receiver_integer: &ArbitraryInteger,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = Symbol::intern("<<");
    let refusal = |message: String| NewspeakError {
        selector,
        receiver,
        message,
    };
    let Some(count) = integer_operand(&mutator.heap, arguments.first()) else {
        return Err(type_mismatch(
            &mutator.heap,
            &receiver,
            selector,
            Accepted::Integer,
            arguments.first(),
        ));
    };
    if count.is_negative() {
        return Err(refusal(format!(
            "<< takes a non-negative shift count, got {}",
            count.to_bounded_text()
        )));
    }
    let shifted = count
        .to_i64()
        .and_then(|count| u64::try_from(count).ok())
        .and_then(|count| receiver_integer.shift_left(count));
    match shifted {
        Some(result) => Ok(mutator.new_integer(result)),
        // A count beyond `u64` shifts a zero receiver to zero like any
        // other count.
        None if receiver_integer.is_zero() => Ok(Value::SmallInteger(0)),
        None => Err(refusal(format!(
            "<< cannot allocate the result of shifting by {} bits",
            count.to_bounded_text()
        ))),
    }
}
```
In `execute_integer`, after the `BitXor` arm. A `SmallInteger` shifted by less than 64 always fits `i128`, so the common case allocates nothing:
```rust
        IntegerPrimitive::ShiftLeft => Some(match argument_integer() {
            Some(count) if (0..64).contains(&count) => {
                let wide = i128::from(receiver_integer) << count;
                Ok(match i64::try_from(wide) {
                    Ok(small) => Value::SmallInteger(small),
                    Err(_) => promote(mutator, wide),
                })
            }
            _ => shift_left(
                mutator,
                receiver,
                &ArbitraryInteger::from_i64(receiver_integer),
                arguments,
            ),
        }),
```
In `execute_large_integer`, after its `BitXor` arm:
```rust
        LargeIntegerPrimitive::ShiftLeft => {
            Some(shift_left(mutator, receiver, &receiver_integer, arguments))
        }
```
In `Kernel.ns`, `class Integer` and `class LargeInteger`, after `bitXor:`:
```newspeak
        public << other = ( ^vmMirror integer: self shiftLeft: other ifFail: [^super << other] )
```
```newspeak
        public << other = ( ^vmMirror largeInteger: self shiftLeft: other ifFail: [^super << other] )
```

- [ ] **Step 7: Run the goldens and the per-commit gate.**

Run: `cargo nextest run -p ns-cli --test run_goldens shift_left`, then the gate.
Expected: PASS.

- [ ] **Step 8: Commit.**

```bash
git commit -m "feat: Integer and LargeInteger shift left with <<, refusing a result that cannot be allocated"
```

---

### Task 4: `Float>>sqrt`

NBody sends `sqrt` to a `Float`. It is Primordial Soup's `Float>>sqrt` (`KernelForPrimordialSoup.ns:1145`, `FLOAT_FUNCTION_1(sqrt)` in `vm/primitives.cc`): the IEEE square root, NaN for a negative receiver. Rust's `f64::sqrt` is that operation, correctly rounded, which is what makes NBody's energy match Are We Fast Yet's to the last bit. `Number>>sqrt` (Primordial Soup's `^self asFloat sqrt`, for an integer receiver) is not added: no benchmark sends it.

**Files:**
- Modify: `crates/ns-runtime/src/builtins/numbers.rs`, `primitive.rs`, `core-library/Kernel.ns`
- Create: `goldens/arithmetic/float_square_root.ns`, `.toml`

**Interfaces:**
- Produces: `FloatPrimitive::SquareRoot`, mirror selector `floatSquareRoot:ifFail:`, `Float>>sqrt`.

- [ ] **Step 1: Write the golden.**

`goldens/arithmetic/float_square_root.ns`:
```newspeak
Newspeak3
'Arithmetic'
(* `sqrt` is Primordial Soup's `Float>>sqrt`: the IEEE square root,
   correctly rounded, NaN for a negative receiver and `-0.0` for `-0.0`.
   NBody sends it. *)
class FloatSquareRoot = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. |
        transcript print: 0.25 sqrt; newLine.
        transcript print: 2.0 sqrt; newLine.
        transcript print: 4.0 sqrt = 2.0; newLine.
        transcript print: -1.0 sqrt; newLine.
        transcript print: -0.0 sqrt; newLine
    )
)
```
`goldens/arithmetic/float_square_root.toml`:
```toml
stdout = '''
0.5
1.4142135623730951
true
NaN
-0
'''
```

- [ ] **Step 2: Run it and see it fail.**

Run: `cargo nextest run -p ns-cli --test run_goldens float_square_root`
Expected: FAIL with `does not understand #sqrt`.

- [ ] **Step 3: Add the leaf and forwarder.**

In `primitive.rs`, add `SquareRoot` after `AsStringFixed` to `FloatPrimitive` and to the Float arm's `=> false` group, and append:
```rust
    mirror(
        "floatSquareRoot:ifFail:",
        "sqrt",
        Primitive::Float(FloatPrimitive::SquareRoot),
    ),
```
In `numbers.rs`, `execute_float`, after `Hash`:
```rust
        FloatPrimitive::SquareRoot => Some(Ok(Value::Float(receiver_float.sqrt()))),
```
In `Kernel.ns`, `class Float`, after `hash`:
```newspeak
        public sqrt = ( ^vmMirror floatSquareRoot: self ifFail: [^super sqrt] )
```

- [ ] **Step 4: Run the golden and the per-commit gate.**

Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git commit -m "feat: Float answers sqrt"
```

---

### Task 5: Mandelbrot and NBody

Both ports start from the SOMns files and follow stage 1's pattern (its Task 6 and 7a): the module shape, `bench` running the workload once and checking it, the protocol rewritten as listed, and every name written out. Their results are Are We Fast Yet's own, at its sizes: Mandelbrot 750 answers 50 (SOMns's `verify:inner:`), and NBody 250000 steps answer `-0.1690859889909308` (SOMns's `innerBenchmarkLoop:numThreads:`).

Both ports were checked when this plan was written, on a release build of `master` with the missing selectors replaced by Newspeak helpers: Mandelbrot answered 50. NBody, with a Newton square root that is not correctly rounded, answered `-0.1690859889909363`: close enough to show the port's structure is right, and not the same, which is why Task 4's `sqrt` matters.

**Files:**
- Create: `benchmarks/are-we-fast-yet/Mandelbrot.ns`, `NBody.ns`
- Modify: `benchmarks/are-we-fast-yet/AreWeFastYet.ns` (the tuple), `are-we-fast-yet.json`, the four `goldens/awfy/*.json`, `goldens/awfy/all_benchmarks_verify.toml`
- Modify: `crates/ns-cli/tests/run.rs` (`a_timed_run_prints_a_score_for_every_benchmark`'s names)

**Interfaces:**
- Consumes: mixed arithmetic (Task 1), `bitXor:` (Task 2), `<<` (Task 3), `sqrt` (Task 4); stage 1's module shape and `Class>>name`.
- Produces: the modules `Mandelbrot` and `NBody`, each `usingPlatform:` with a public `bench`.

- [ ] **Step 1: Write Mandelbrot.**

`benchmarks/are-we-fast-yet/Mandelbrot.ns` starts with this header. After its fourth paragraph it carries SOMns's own header of the file, lines 1 to 46 of `prior-art/somns/core-lib/Benchmarks/Mandelbrot.ns` without their opening `(*`, verbatim and indented by three spaces: the note on the Ruby transcription and the Computer Language Benchmarks Game's BSD-3 text. The file is copied with its licence as it stands (the user's decision; no `THIRD-PARTY-NOTICES.md` entry).
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from SOMns (https://github.com/smarr/SOMns, commit
   e56f4c2815cef4cae2045a9b05dc1fd190315ba6,
   core-lib/Benchmarks/Mandelbrot.ns), one of the Are We Fast Yet benchmarks.
   Changed for ns-next: a module in Primordial Soup's shape, whose bench
   runs once and checks its own result; only Primordial Soup's protocol is
   sent; names are written out.

   SOMns's own header of the file:

   This version is a transcription of the Ruby implementation mandelbrot.rb
   …(the rest of SOMns's lines 2 to 46)… *)
```
Then the class:
```newspeak
class Mandelbrot usingPlatform: platform = (
    | private Error = platform kernel Error. |
) (
    public bench = (
        | result |
        result:: mandelbrot: 750.
        result = 50 ifFalse: [Error signal: 'Mandelbrot: expected 50, got ', result printString].
        ^result
    )
    mandelbrot: size = (
        | sum byteAccumulator bitNumber row |
        sum:: 0.
        byteAccumulator:: 0.
        bitNumber:: 0.
        row:: 0.
        [row < size] whileTrue: [
            | imaginaryConstant column |
            imaginaryConstant:: (2.0 * row / size) - 1.0.
            column:: 0.
            [column < size] whileTrue: [
                | real realSquared imaginary imaginarySquared realConstant escape iteration notDone |
                realSquared:: real:: 0.0.
                imaginarySquared:: imaginary:: 0.0.
                realConstant:: (2.0 * column / size) - 1.5.
                iteration:: 0.
                notDone:: true.
                escape:: 0.
                [notDone and: [iteration < 50]] whileTrue: [
                    real:: realSquared - imaginarySquared + realConstant.
                    imaginary:: 2.0 * real * imaginary + imaginaryConstant.
                    (* preserve recalculation *)
                    realSquared:: real * real.
                    imaginarySquared:: imaginary * imaginary.
                    (realSquared + imaginarySquared > 4.0) ifTrue: [
                        notDone:: false.
                        escape:: 1].
                    iteration:: iteration + 1].
                byteAccumulator:: (byteAccumulator << 1) + escape.
                bitNumber:: bitNumber + 1.
                (* Code is very similar for these cases, but using separate blocks
                   ensures we skip the shifting when it is unnecessary,
                   which is in most cases. *)
                bitNumber = 8
                    ifTrue: [
                        sum:: sum bitXor: byteAccumulator.
                        byteAccumulator:: 0.
                        bitNumber:: 0]
                    ifFalse: [
                        (column = (size - 1)) ifTrue: [
                            byteAccumulator:: byteAccumulator << (8 - bitNumber).
                            sum:: sum bitXor: byteAccumulator.
                            byteAccumulator:: 0.
                            bitNumber:: 0]].
                column:: column + 1].
            row:: row + 1].
        ^sum
    )
)
```
Its rewrites:
- `(2.0 * y // size)` and `(2.0 * x // size)` become `/` (SOMns's `//` divides as floats).
- `y`, `x`, `z`, `zr`, `zi`, `zrzr`, `zizi`, `cr`, `ci`, `bitNum`, `byteAcc` become `row`, `column`, `iteration`, `real`, `imaginary`, `realSquared`, `imaginarySquared`, `realConstant`, `imaginaryConstant`, `bitNumber`, `byteAccumulator`.
- SOMns's nested `Mandelbrot` class with `firstResult` and `verify:inner:` becomes `bench`, which checks the one pinned size.

- [ ] **Step 2: Write NBody.**

`benchmarks/are-we-fast-yet/NBody.ns` carries the header of `benchmarks/are-we-fast-yet/Sieve.ns`, with `core-lib/Benchmarks/NBody.ns` as the path and the same MIT text. Before the MIT text it quotes SOMns's own attribution, lines 2 to 8 of the file, indented by three spaces, under "SOMns's own header of the file:". Then:
```newspeak
class NBody usingPlatform: platform = (
    | private Error = platform kernel Error. |
) (
    public bench = (
        | system energy |
        system:: NBodySystem new.
        1 to: 250000 do: [:step | system advance: 0.01].
        energy:: system energy.
        energy = -0.1690859889909308 ifFalse: [
            Error signal: 'NBody: expected -0.1690859889909308, got ', energy printString].
        ^energy
    )
    class NBodySystem = (
        | bodies = createBodies. |
    ) (
        public createBodies = (
            | momentumX momentumY momentumZ bodies |
            bodies:: {Body sun. Body jupiter. Body saturn. Body uranus. Body neptune}.
            momentumX:: 0.0.
            momentumY:: 0.0.
            momentumZ:: 0.0.
            bodies do: [:body |
                momentumX:: momentumX + (body velocityX * body mass).
                momentumY:: momentumY + (body velocityY * body mass).
                momentumZ:: momentumZ + (body velocityZ * body mass)].
            (bodies at: 1) offsetMomentumX: momentumX momentumY: momentumY momentumZ: momentumZ.
            ^bodies
        )
        public advance: timeStep = (
            1 to: bodies size do: [:first |
                | firstBody |
                firstBody:: bodies at: first.
                first + 1 to: bodies size do: [:second |
                    | deltaX deltaY deltaZ secondBody distanceSquared distance magnitude |
                    secondBody:: bodies at: second.
                    deltaX:: firstBody positionX - secondBody positionX.
                    deltaY:: firstBody positionY - secondBody positionY.
                    deltaZ:: firstBody positionZ - secondBody positionZ.
                    distanceSquared:: (deltaX * deltaX) + (deltaY * deltaY) + (deltaZ * deltaZ).
                    distance:: distanceSquared sqrt.
                    magnitude:: timeStep / (distanceSquared * distance).
                    firstBody velocityX: firstBody velocityX - (deltaX * secondBody mass * magnitude).
                    firstBody velocityY: firstBody velocityY - (deltaY * secondBody mass * magnitude).
                    firstBody velocityZ: firstBody velocityZ - (deltaZ * secondBody mass * magnitude).
                    secondBody velocityX: secondBody velocityX + (deltaX * firstBody mass * magnitude).
                    secondBody velocityY: secondBody velocityY + (deltaY * firstBody mass * magnitude).
                    secondBody velocityZ: secondBody velocityZ + (deltaZ * firstBody mass * magnitude)]].
            bodies do: [:body |
                body positionX: body positionX + (timeStep * body velocityX).
                body positionY: body positionY + (timeStep * body velocityY).
                body positionZ: body positionZ + (timeStep * body velocityZ)]
        )
        public energy = (
            | deltaX deltaY deltaZ distance total |
            total:: 0.0.
            1 to: bodies size do: [:first |
                | firstBody |
                firstBody:: bodies at: first.
                total:: total + (0.5 * firstBody mass *
                    ((firstBody velocityX * firstBody velocityX) +
                     (firstBody velocityY * firstBody velocityY) +
                     (firstBody velocityZ * firstBody velocityZ))).
                first + 1 to: bodies size do: [:second |
                    | secondBody |
                    secondBody:: bodies at: second.
                    deltaX:: firstBody positionX - secondBody positionX.
                    deltaY:: firstBody positionY - secondBody positionY.
                    deltaZ:: firstBody positionZ - secondBody positionZ.
                    distance:: ((deltaX * deltaX) + (deltaY * deltaY) + (deltaZ * deltaZ)) sqrt.
                    total:: total - ((firstBody mass * secondBody mass) / distance)]].
            ^total
        )
    )
    class Body = (
        | public positionX ::= 0.0.
          public positionY ::= 0.0.
          public positionZ ::= 0.0.
          public velocityX ::= 0.0.
          public velocityY ::= 0.0.
          public velocityZ ::= 0.0.
          public mass ::= 0.0. |
    ) (
        public offsetMomentumX: momentumX momentumY: momentumY momentumZ: momentumZ = (
            velocityX:: 0.0 - (momentumX / Body solarMass).
            velocityY:: 0.0 - (momentumY / Body solarMass).
            velocityZ:: 0.0 - (momentumZ / Body solarMass)
        )
    ) : (
        public piValue = ( ^3.141592653589793 )
        public solarMass = ( ^4 * self piValue * self piValue )
        public daysPerYear = ( ^365.24 )
        public jupiter = (
            | body |
            body:: self new.
            body positionX: 4.8414314424647209.
            body positionY: -1.16032004402742839.
            body positionZ: -0.103622044471123109.
            body velocityX: 0.00166007664274403694 * self daysPerYear.
            body velocityY: 0.00769901118419740425 * self daysPerYear.
            body velocityZ: -0.0000690460016972063023 * self daysPerYear.
            body mass: 0.000954791938424326609 * self solarMass.
            ^body
        )
        public saturn = (
            | body |
            body:: self new.
            body positionX: 8.34336671824457987.
            body positionY: 4.12479856412430479.
            body positionZ: -0.403523417114321381.
            body velocityX: -0.00276742510726862411 * self daysPerYear.
            body velocityY: 0.00499852801234917238 * self daysPerYear.
            body velocityZ: 0.0000230417297573763929 * self daysPerYear.
            body mass: 0.000285885980666130812 * self solarMass.
            ^body
        )
        public uranus = (
            | body |
            body:: self new.
            body positionX: 12.894369562139131.
            body positionY: -15.1111514016986312.
            body positionZ: -0.223307578892655734.
            body velocityX: 0.00296460137564761618 * self daysPerYear.
            body velocityY: 0.0023784717395948095 * self daysPerYear.
            body velocityZ: -0.0000296589568540237556 * self daysPerYear.
            body mass: 0.0000436624404335156298 * self solarMass.
            ^body
        )
        public neptune = (
            | body |
            body:: self new.
            body positionX: 15.3796971148509165.
            body positionY: -25.9193146099879641.
            body positionZ: 0.179258772950371181.
            body velocityX: 0.00268067772490389322 * self daysPerYear.
            body velocityY: 0.00162824170038242295 * self daysPerYear.
            body velocityZ: -0.000095159225451971587 * self daysPerYear.
            body mass: 0.0000515138902046611451 * self solarMass.
            ^body
        )
        public sun = (
            | body |
            body:: self new.
            body mass: self solarMass.
            ^body
        )
    )
)
```
Its rewrites:
- Every `//` becomes `/` (float division, as in Mandelbrot).
- `x`, `y`, `z`, `vx`, `vy`, `vz` become `positionX` … `velocityZ`; `Pi`, `SolarMass`, `DaysPerYear` become `piValue`, `solarMass`, `daysPerYear` (`Pi` has two characters); `offsetMomentumX:y:z:` becomes `offsetMomentumX:momentumY:momentumZ:`; `px`, `dt`, `dx`, `dSquared`, `mag`, `e`, `b`, `i`, `j`, `iBody`, `jBody` are written out.
- SOMns's dead `Body>>print` and its commented-out calls are left out.
- SOMns's nested `NBody` class with `expectedEnergy` becomes `bench`, which checks the one pinned size.

Every numeric literal is copied digit for digit: the energy is compared bit for bit, and a changed digit changes the double.

- [ ] **Step 3: Add them to the harness and the projects.**

The tuple in `AreWeFastYet.ns` stays alphabetical:
```newspeak
    | private benchmarks = {
        manifest Bounce. manifest List. manifest Mandelbrot. manifest NBody.
        manifest Permute. manifest Queens. manifest Sieve. manifest Storage.
        manifest Towers }. |
```
Add `"Mandelbrot.ns", "NBody.ns"` to `are-we-fast-yet.json`'s `modules`, in the same order, and `"../../benchmarks/are-we-fast-yet/Mandelbrot.ns"` and `…/NBody.ns` to each of the four `goldens/awfy/*.json`.

`goldens/awfy/all_benchmarks_verify.toml`:
```toml
stdout = '''
Bounce: ok
List: ok
Mandelbrot: ok
NBody: ok
Permute: ok
Queens: ok
Sieve: ok
Storage: ok
Towers: ok
'''
```
In `crates/ns-cli/tests/run.rs`, `a_timed_run_prints_a_score_for_every_benchmark`, the expected names become:
```rust
        [
            "Bounce",
            "List",
            "Mandelbrot",
            "NBody",
            "Permute",
            "Queens",
            "Sieve",
            "Storage",
            "Towers"
        ]
```
(`cargo fmt` lays it out.)

- [ ] **Step 4: Run each benchmark once in a release build.**

Run:
- `cargo build -p ns-cli --release`
- `./target/release/ns-next run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- Mandelbrot`
- `./target/release/ns-next run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- NBody`

Expected: `Mandelbrot: ok` after about 4 s, `NBody: ok` after about 3 s. If a benchmark raises its own `expected … got …`, the port differs from SOMns: compare it line by line with the SOMns file, literal digits included, and do not change the expected value.

- [ ] **Step 5: Run the goldens and the per-commit gate.**

Run: `cargo nextest run -p ns-cli awfy a_timed_run`, then the gate.
Expected: PASS. These tests take tens of seconds in the debug build now (Global Constraints, "The suite gets slower").

- [ ] **Step 6: Commit.**

```bash
git add benchmarks goldens/awfy crates/ns-cli/tests/run.rs
git commit -m "feat: Mandelbrot and NBody in Primordial Soup's protocol, at Are We Fast Yet's sizes"
```

---

### Task 6: Richards and RichardsTon80

`Richards` is SOMns's Are We Fast Yet version; `RichardsTon80` is Primordial Soup's, derived from the Dart Ton80 version, with its own task structure and counts. Both ports were checked when this plan was written, with `bitXor:`, `bitOr:` and `bitInvert` replaced by Newspeak helpers on `master`: Richards answered 23246 and 9297, and RichardsTon80 2322 and 928.

**Files:**
- Create: `benchmarks/are-we-fast-yet/Richards.ns`, `RichardsTon80.ns`
- Modify: the tuple, the project file, the four golden `.json` files, `all_benchmarks_verify.toml` and `run.rs`'s names, as in Task 5
- Modify: `THIRD-PARTY-NOTICES.md` (the introduction)

**Interfaces:**
- Consumes: `bitXor:`, `bitOr:`, `bitInvert` (Task 2); `bitAnd:`, `//` (stage 1 and before).
- Produces: the modules `Richards` and `RichardsTon80`.

- [ ] **Step 1: Write Richards.**

`benchmarks/are-we-fast-yet/Richards.ns` carries the header of `benchmarks/are-we-fast-yet/Sieve.ns`, with `core-lib/Benchmarks/Richards.ns` as the path. Its "Changed for ns-next" text gains one sentence: "the tracing, which SOMns never switches on, is left out." Then:
```newspeak
class Richards usingPlatform: platform = (
    | private Error = platform kernel Error.
      private NoTask = nil.
      private Idler = 1.
      private NoWork = nil.
      private Worker = 2.
      private WorkPacketKind = 2.
      private HandlerA = 3.
      private HandlerB = 4.
      private DeviceA = 5.
      private DeviceB = 6.
      private DevicePacketKind = 1. |
) (
    public bench = (
        | benchmarks |
        benchmarks:: RichardsBenchmarks new.
        benchmarks start ifFalse: [
            Error signal: 'Richards: expected 23246 queued packets and 9297 holds, got ',
                benchmarks queuePacketCount printString, ' and ', benchmarks holdCount printString].
        ^true
    )
    (* SOMns's `Array new: 4 withAll: 0`; Primordial Soup's `Array` has no
       `new:withAll:`. *)
    newArrayOfZeroes: size = (
        | array |
        array:: Array new: size.
        1 to: size do: [:index | array at: index put: 0].
        ^array
    )
    class RichardsBenchmarks = (
        | taskList currentTask currentTaskIdentity taskTable
          public queuePacketCount
          public holdCount |
    ) (
        createDevice: identity priority: priority work: work state: state = (
            | data |
            data:: DeviceTaskDataRecord create.
            self
                createTask: identity
                priority: priority
                work: work
                state: state
                function: [:packet :record |
                    | functionWork |
                    functionWork:: packet.
                    NoWork == functionWork
                        ifTrue: [
                            NoWork == (functionWork:: record pending)
                                ifTrue: [self wait]
                                ifFalse: [
                                    record pending: NoWork.
                                    self queuePacket: functionWork]]
                        ifFalse: [
                            record pending: functionWork.
                            self holdSelf]]
                data: data
        )
        createHandler: identity priority: priority work: work state: state = (
            | data |
            data:: HandlerTaskDataRecord create.
            self
                createTask: identity
                priority: priority
                work: work
                state: state
                function: [:packet :record |
                    | workPacket |
                    NoWork == packet ifFalse: [
                        WorkPacketKind = packet kind
                            ifTrue: [record workInAdd: packet]
                            ifFalse: [record deviceInAdd: packet]].
                    NoWork == (workPacket:: record workIn)
                        ifTrue: [self wait]
                        ifFalse: [
                            | count |
                            count:: workPacket datum.
                            count > 4
                                ifTrue: [
                                    record workIn: workPacket link.
                                    self queuePacket: workPacket]
                                ifFalse: [
                                    | devicePacket |
                                    NoWork == (devicePacket:: record deviceIn)
                                        ifTrue: [self wait]
                                        ifFalse: [
                                            record deviceIn: devicePacket link.
                                            devicePacket datum: (workPacket data at: count).
                                            workPacket datum: count + 1.
                                            self queuePacket: devicePacket]]]]
                data: data
        )
        createIdler: identity priority: priority work: work state: state = (
            | data |
            data:: IdleTaskDataRecord create.
            self
                createTask: identity
                priority: priority
                work: work
                state: state
                function: [:packet :record |
                    record count: record count - 1.
                    0 = record count
                        ifTrue: [self holdSelf]
                        ifFalse: [
                            0 = (record control bitAnd: 1)
                                ifTrue: [
                                    record control: record control // 2.
                                    self release: DeviceA]
                                ifFalse: [
                                    record control: (record control // 2 bitXor: 53256).
                                    self release: DeviceB]]]
                data: data
        )
        createPacket: link identity: identity kind: kind = (
            ^Packet create: link identity: identity kind: kind
        )
        createTask: identity priority: priority work: work state: state function: block data: data = (
            | task |
            task:: TaskControlBlock
                link: taskList
                create: identity
                priority: priority
                initialWorkQueue: work
                initialState: state
                function: block
                privateData: data.
            taskList:: task.
            taskTable at: identity put: task
        )
        createWorker: identity priority: priority work: work state: state = (
            | data |
            data:: WorkerTaskDataRecord create.
            self
                createTask: identity
                priority: priority
                work: work
                state: state
                function: [:packet :record |
                    NoWork == packet
                        ifTrue: [self wait]
                        ifFalse: [
                            record destination: (HandlerA = record destination
                                ifTrue: [HandlerB]
                                ifFalse: [HandlerA]).
                            packet identity: record destination.
                            packet datum: 1.
                            1 to: 4 do: [:index |
                                record count: record count + 1.
                                record count > 26 ifTrue: [record count: 1].
                                (* 65 is the character code of $A. *)
                                packet data at: index put: 65 + record count - 1].
                            self queuePacket: packet]]
                data: data
        )
        public start = (
            | workQueue |
            self initScheduler.
            self createIdler: Idler priority: 0 work: NoWork state: TaskState running.
            workQueue:: self createPacket: NoWork identity: Worker kind: WorkPacketKind.
            workQueue:: self createPacket: workQueue identity: Worker kind: WorkPacketKind.
            self createWorker: Worker priority: 1000 work: workQueue state: TaskState waitingWithPacket.
            workQueue:: self createPacket: NoWork identity: DeviceA kind: DevicePacketKind.
            workQueue:: self createPacket: workQueue identity: DeviceA kind: DevicePacketKind.
            workQueue:: self createPacket: workQueue identity: DeviceA kind: DevicePacketKind.
            self createHandler: HandlerA priority: 2000 work: workQueue state: TaskState waitingWithPacket.
            workQueue:: self createPacket: NoWork identity: DeviceB kind: DevicePacketKind.
            workQueue:: self createPacket: workQueue identity: DeviceB kind: DevicePacketKind.
            workQueue:: self createPacket: workQueue identity: DeviceB kind: DevicePacketKind.
            self createHandler: HandlerB priority: 3000 work: workQueue state: TaskState waitingWithPacket.
            self createDevice: DeviceA priority: 4000 work: NoWork state: TaskState waiting.
            self createDevice: DeviceB priority: 5000 work: NoWork state: TaskState waiting.
            self schedule.
            ^(queuePacketCount = 23246) & (holdCount = 9297)
        )
        findTask: identity = (
            | task |
            task:: taskTable at: identity.
            NoTask == task ifTrue: [Error signal: 'findTask failed'].
            ^task
        )
        holdSelf = (
            holdCount:: holdCount + 1.
            currentTask taskHolding: true.
            ^currentTask link
        )
        initScheduler = (
            queuePacketCount:: 0.
            holdCount:: 0.
            (* SOMns's `Array new: 6 withAll: NoTask`: `NoTask` is nil, which
               `Array new:` fills with. *)
            taskTable:: Array new: 6.
            taskList:: NoTask
        )
        queuePacket: packet = (
            | task |
            task:: self findTask: packet identity.
            NoTask == task ifTrue: [^NoTask].
            queuePacketCount:: queuePacketCount + 1.
            packet link: NoWork.
            packet identity: currentTaskIdentity.
            ^task addInput: packet checkPriority: currentTask
        )
        release: identity = (
            | task |
            task:: self findTask: identity.
            NoTask == task ifTrue: [^NoTask].
            task taskHolding: false.
            task priority > currentTask priority
                ifTrue: [^task]
                ifFalse: [^currentTask]
        )
        wait = (
            currentTask taskWaiting: true.
            ^currentTask
        )
        schedule = (
            currentTask:: taskList.
            [NoTask == currentTask] whileFalse: [
                currentTask isTaskHoldingOrWaiting
                    ifTrue: [currentTask:: currentTask link]
                    ifFalse: [
                        currentTaskIdentity:: currentTask identity.
                        currentTask:: currentTask runTask]]
        )
    )
    class RichardsObject = (
    ) (
        public append: packet head: queueHead = (
            | mouse link |
            packet link: NoWork.
            NoWork == queueHead ifTrue: [^packet].
            mouse:: queueHead.
            [NoWork == (link:: mouse link)] whileFalse: [mouse:: link].
            mouse link: packet.
            ^queueHead
        )
    )
    class DeviceTaskDataRecord create = (
        | public pending ::= NoWork. |
    ) ()
    class HandlerTaskDataRecord create = RichardsObject (
        | public workIn ::= NoWork.
          public deviceIn ::= NoWork. |
    ) (
        public deviceInAdd: packet = (
            deviceIn:: self append: packet head: deviceIn
        )
        public workInAdd: packet = (
            workIn:: self append: packet head: workIn
        )
    )
    class IdleTaskDataRecord create = (
        | public control ::= 1.
          public count ::= 10000. |
    ) ()
    class Packet create: initialLink identity: initialIdentity kind: initialKind = (
        | public link ::= initialLink.
          public kind ::= initialKind.
          public identity ::= initialIdentity.
          public datum ::= 1.
          public data ::= newArrayOfZeroes: 4. |
    ) ()
    class TaskState = RichardsObject (
        | packetPendingFlag taskWaitingFlag taskHoldingFlag |
    ) (
        public isPacketPending = ( ^packetPendingFlag )
        public isTaskHolding = ( ^taskHoldingFlag )
        public isTaskWaiting = ( ^taskWaitingFlag )
        public taskHolding: value = ( taskHoldingFlag:: value )
        public taskWaiting: value = ( taskWaitingFlag:: value )
        public packetPending: value = ( packetPendingFlag:: value )
        public packetPending = (
            packetPendingFlag:: true.
            taskWaitingFlag:: false.
            taskHoldingFlag:: false
        )
        public running = (
            packetPendingFlag:: taskWaitingFlag:: taskHoldingFlag:: false
        )
        public waiting = (
            packetPendingFlag:: taskHoldingFlag:: false.
            taskWaitingFlag:: true
        )
        public waitingWithPacket = (
            taskHoldingFlag:: false.
            taskWaitingFlag:: packetPendingFlag:: true
        )
        public isRunning = ( ^packetPendingFlag not and: [taskWaitingFlag not and: [taskHoldingFlag not]] )
        public isTaskHoldingOrWaiting = ( ^taskHoldingFlag or: [packetPendingFlag not and: [taskWaitingFlag]] )
        public isWaiting = ( ^packetPendingFlag not and: [taskWaitingFlag and: [taskHoldingFlag not]] )
        public isWaitingWithPacket = ( ^packetPendingFlag and: [taskWaitingFlag and: [taskHoldingFlag not]] )
    ) : (
        public packetPending = ( ^self new packetPending )
        public running = ( ^self new running )
        public waiting = ( ^self new waiting )
        public waitingWithPacket = ( ^self new waitingWithPacket )
    )
    class TaskControlBlock link: initialLink create: initialIdentity priority: initialPriority initialWorkQueue: initialWorkQueue initialState: initialState function: block privateData: privateData = TaskState (
        | public link = initialLink.
          public identity = initialIdentity.
          public function = block.
          public priority = initialPriority.
          protected input ::= initialWorkQueue.
          protected handle = privateData. |
        self packetPending: initialState isPacketPending.
        self taskWaiting: initialState isTaskWaiting.
        self taskHolding: initialState isTaskHolding.
    ) (
        public addInput: packet checkPriority: oldTask = (
            NoWork == input
                ifTrue: [
                    input:: packet.
                    self packetPending: true.
                    priority > oldTask priority ifTrue: [^self]]
                ifFalse: [
                    input:: self append: packet head: input].
            ^oldTask
        )
        public runTask = (
            | message |
            self isWaitingWithPacket
                ifTrue: [
                    message:: input.
                    input:: message link.
                    NoWork == input
                        ifTrue: [self running]
                        ifFalse: [self packetPending]]
                ifFalse: [
                    message:: NoWork].
            ^function value: message value: handle
        )
    )
    class WorkerTaskDataRecord create = (
        | public destination ::= HandlerA.
          public count ::= 0. |
    ) ()
)
```
Its rewrites:
- `data control / 2` becomes `record control // 2` (SOMns's `/` truncates two integers; the control word is never negative, so floor and truncation agree); `&` with an integer becomes `bitAnd:`, as in stage 1's Bounce.
- `function value: message with: handle` becomes `value:value:`.
- `Array new: 6 withAll: NoTask` becomes `Array new: 6`, and `Array new: 4 withAll: 0` becomes `newArrayOfZeroes: 4`.
- `queuePacketCount = 23246 and: holdCount = 9297` passes a Boolean to `and:`, which Primordial Soup's `True>>and:` would send `value`; it becomes `&`. `queuePacketCount` and `holdCount` become public so `bench` can report them.
- `self error:` becomes `Error signal:`.
- The tracing (`tracing`, `layout`, `initTrace`, `trace:`, the `Transcript` class, the `asString` methods that only it sends) is left out.
- `RBObject` becomes `RichardsObject`; the blocks' `:work :word` become `:packet :record`, which no longer shadow the method's `work` and `data`; `TaskState`'s `packetPending_` flags become `packetPendingFlag` and so on; `aBoolean` becomes `value`; `t` and `workQ` are written out; the factories' parameters no longer share their slots' names.

- [ ] **Step 2: Write RichardsTon80.**

`benchmarks/are-we-fast-yet/RichardsTon80.ns`:
```newspeak
Newspeak3
'AreWeFastYet'
(* Adapted for ns-next from Primordial Soup's Richards.ns
   (https://github.com/rmacnak/primordialsoup, commit
   338f0bb342a618f5608ab72ab1b1fae9b226e472, newspeak/Richards.ns), which
   says of itself: "An OS kernel simulation benchmark, originally written in
   BCPL by Martin Richards. This version derived from the Dart Ton80
   version." It is not the Are We Fast Yet Richards, which is Richards.ns
   beside it. Changed for ns-next: the module is named RichardsTon80 so
   that both sit in one harness; bench signals an Error rather than an
   Exception and answers the queue count; quo: 2 is // 2, the same for the
   never negative control word; next: queueHead is the assignment it
   means; names are written out.

   Copyright 2008, the Newspeak project authors.

   Licensed under the Apache License, Version 2.0 (the ''License''); you
   may not use this file except in compliance with the License. You may
   obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
   and in THIRD-PARTY-NOTICES.md at the repository root. *)
class RichardsTon80 usingPlatform: platform = (
    | private Error = platform kernel Error.
      dataSize = 4.
      idleCount = 1000.

      idleIdentity = 1.
      workerIdentity = 2.
      handlerAIdentity = 3.
      handlerBIdentity = 4.
      deviceAIdentity = 5.
      deviceBIdentity = 6.
      numberOfIdentities = 6.

      kindDevice = 0.
      kindWork = 1.

      (* The task is running and is currently scheduled. *)
      stateRunning = 0.

      (* The task has packets left to process. *)
      stateRunnable = 1.

      (* The task is not currently running. The task is not blocked as such and may be started by the scheduler. *)
      stateSuspended = 2.

      (* The task is blocked and cannot be run until it is explicitly released. *)
      stateHeld = 4.
      stateSuspendedRunnable = stateSuspended bitOr: stateRunnable.
      stateNotHeld = stateHeld bitInvert. |
) (
    (* A task that suspends itself after each time it has been run to simulate waiting for data from an external device. *)
    class DeviceTask on: taskScheduler = Task on: taskScheduler (
        | protected pending <Packet> |
    ) (
        public run: packet <Packet> ^<TaskControlBlock> = (
            packet isNil ifTrue: [
                | functionWork <Packet> |
                pending isNil ifTrue: [^scheduler suspendCurrent].
                functionWork:: pending.
                pending:: nil.
                ^scheduler queue: functionWork].
            pending:: packet.
            ^scheduler holdCurrent
        )
    )
    (* A task that manipulates work packets and then suspends itself. *)
    class HandlerTask on: taskScheduler = Task on: taskScheduler (
        | workIn <Packet>
          deviceIn <Packet> |
    ) (
        public run: packet <Packet> ^<TaskControlBlock> = (
            packet isNil ifFalse: [
                packet kind = kindWork
                    ifTrue: [workIn:: packet appendTo: workIn]
                    ifFalse: [deviceIn:: packet appendTo: deviceIn]].
            workIn isNil ifFalse: [
                | queued count = workIn datum. |
                count < dataSize
                    ifTrue: [
                        deviceIn isNil ifFalse: [
                            queued:: deviceIn.
                            deviceIn:: deviceIn link.
                            queued datum: (workIn data at: count + 1 (* 1-origin conversion, sigh *)).
                            workIn datum: (count + 1).
                            ^scheduler queue: queued]]
                    ifFalse: [
                        queued:: workIn.
                        workIn:: workIn link.
                        ^scheduler queue: queued]].
            ^scheduler suspendCurrent
        )
    )
    (* An idle task doesn't do any work itself but cycles control between the two device tasks. *)
    class IdleTask on: taskScheduler with: initialControl with: initialCount = Task on: taskScheduler (
        | protected control <Integer> ::= initialControl. (* A seed value that controls how the device tasks are scheduled. *)
          protected count <Integer> ::= initialCount. (* The number of times this task should be scheduled. *) |
    ) (
        public run: packet <Packet> ^<TaskControlBlock> = (
            count:: count - 1.
            count = 0 ifTrue: [^scheduler holdCurrent].
            (control bitAnd: 1) = 0 ifTrue: [
                control:: control // 2.
                ^scheduler release: deviceAIdentity].
            control:: (control // 2) bitXor: 16rD008.
            ^scheduler release: deviceBIdentity
        )
    )
    (* A simple package of data that is manipulated by the tasks.  The exact layout of the payload data carried by a packet is not importaint, and neither is the nature of the work performed on packets by the tasks. Besides carrying data, packets form linked lists and are hence used both as data and worklists. *)
    class Packet link: initialLink identity: initialIdentity kind: initialKind = (
        | public link <Packet> ::= initialLink. (* The tail of the linked list of packets. *)
          public identity <Integer> ::= initialIdentity. (* An ID for this packet. *)
          public kind <Integer> ::= initialKind. (* The type of this packet. *)
          public datum <Integer> ::= 0.
          public data <Array[Integer]> = Array new: dataSize. |
    ) (
        (* Add this packet to the end of a worklist, and return the worklist. *)
        public appendTo: queueHead <Packet> ^<Packet> = (
            | peek next |
            link: nil.
            queueHead isNil ifTrue: [^self].
            next:: queueHead.
            [(peek:: next link) isNil]
                whileFalse: [next:: peek].
            next link: self.
            ^queueHead
        )
    )
    (* A scheduler can be used to schedule a set of tasks based on their relative priorities.  Scheduling is done by maintaining a list of task control blocks which holds tasks and the data queue they are processing. *)
    class Scheduler = (
        | public queueCount <Integer> ::= 0.
          public holdCount <Integer> ::= 0.
          currentBlock <TaskControlBlock>
          currentIdentity
          list <TaskControlBlock>
          blocks <Array[TaskControlBlock]> = Array new: numberOfIdentities. |
    ) (
        (* Add a device task to this scheduler. *)
        public addDeviceTask: id <Integer> priority: priority <Integer> work: queue <Packet> = (
            addTask: id priority: priority work: queue task: (DeviceTask on: self).
        )
        (* Add a handler task to this scheduler. *)
        public addHandlerTask: id <Integer> priority: priority <Integer> work: queue <Packet> = (
            addTask: id priority: priority work: queue task: (HandlerTask on: self).
        )
        (* Add an idle task to this scheduler. *)
        public addIdleTask: id <Integer> priority: priority <Integer> work: queue <Packet> state: count <Integer> = (
            addRunningTask: id priority: priority work: queue task: (IdleTask on: self with: 1 with: count).
        )
        addRunningTask: id <Integer> priority: priority <Integer> work: queue <Packet> task: task <Task> = (
            addTask: id priority: priority work: queue task: task.
            currentBlock setRunning.
        )
        addTask: id <Integer> priority: priority <Integer> work: queue <Packet> task: task <Task> = (
            currentBlock:: TaskControlBlock link: list id: id priority: priority queue: queue task: task.
            list:: currentBlock.
            blocks at: id put: currentBlock.
        )
        (* Add a work task to this scheduler. *)
        public addWorkerTask: id <Integer> priority: priority <Integer> work: queue <Packet> = (
            addTask: id priority: priority work: queue task: (WorkerTask on: self with: handlerAIdentity with: 0).
        )
        (* Block the currently executing task and return the next task control block to run.  The blocked task will not be made runnable until it is explicitly released, even if new work is added to it. *)
        public holdCurrent ^<TaskControlBlock> = (
            holdCount:: holdCount + 1.
            currentBlock markAsHeld.
            ^currentBlock link
        )
        (* Add the specified packet to the end of the worklist used by the task associated with the packet and make the task runnable if it is currently suspended. *)
        public queue: packet <Packet> ^<TaskControlBlock> = (
            | block = blocks at: packet identity. |
            block isNil ifTrue: [^block].
            queueCount:: queueCount + 1.
            packet link: nil.
            packet identity: currentIdentity.
            ^block addInput: packet checkPriority: currentBlock
        )
        (* Release a task that is currently blocked and return the next block to run. *)
        public release: id <Integer> ^<TaskControlBlock> = (
            | block = blocks at: id. |
            block isNil ifTrue: [^block].
            block markAsNotHeld.
            block priority > currentBlock priority ifTrue: [^block].
            ^currentBlock
        )
        (* Execute the tasks managed by this scheduler. *)
        public schedule = (
            currentBlock:: list.
            [currentBlock isNil] whileFalse: [
                currentBlock isHeldOrSuspended
                    ifTrue: [currentBlock:: currentBlock link]
                    ifFalse: [
                        currentIdentity:: currentBlock identity.
                        currentBlock:: currentBlock run]].
        )
        (* Suspend the currently executing task and return the next task control block to run. If new work is added to the suspended task it will be made runnable. *)
        public suspendCurrent ^<TaskControlBlock> = (
            currentBlock markAsSuspended.
            ^currentBlock
        )
    )
    (* Abstract task that manipulates work packets. *)
    class Task on: taskScheduler = (
        | protected scheduler <Scheduler> = taskScheduler. (* The scheduler that manages this task. *) |
    ) (
        public run: packet <Packet> = (
            subclassResponsibility
        )
    )
    (* A task control block manages a task and the queue of work packages associated with it. *)
    class TaskControlBlock link: initialLink id: initialIdentity priority: initialPriority queue: initialQueue task: initialTask = (
        | public link <TaskControlBlock> = initialLink.
          public identity <Integer> = initialIdentity.
          public priority <Integer> = initialPriority.
          queue <Packet> ::= initialQueue.
          task <Task> = initialTask.
          state <Integer> ::= queue isNil ifTrue: [stateSuspended] ifFalse: [stateSuspendedRunnable]. |
    ) (
        (* Adds a packet to the worklist of this block's task, marks this as runnable if necessary, and returns the next runnable object to run (the one with the highest priority. *)
        public addInput: packet <Packet> checkPriority: oldTask <Task> ^<TaskControlBlock> = (
            queue isNil
                ifTrue: [
                    queue:: packet.
                    self markAsRunnable.
                    priority > oldTask priority ifTrue: [^self]]
                ifFalse: [
                    queue:: packet appendTo: queue].
            ^oldTask
        )
        public isHeldOrSuspended = (
            ^((state bitAnd: stateHeld) = 0) not
                or: [state = stateSuspended]
        )
        public markAsHeld = (
            state:: state bitOr: stateHeld.
        )
        public markAsNotHeld = (
            state:: state bitAnd: stateNotHeld.
        )
        public markAsRunnable = (
            state:: state bitOr: stateRunnable.
        )
        public markAsSuspended = (
            state:: state bitOr: stateSuspended.
        )
        (* Runs this task, if it is ready to be run, and returns the next task to run. *)
        public run ^<TaskControlBlock> = (
            | packet |
            state = stateSuspendedRunnable ifTrue: [
                packet:: queue.
                queue:: packet link.
                state:: queue isNil ifTrue: [stateRunning] ifFalse: [stateRunnable].
            ].
            ^task run: packet
        )
        public setRunning = (
            state:: stateRunning.
        )
    )
    (* A task that manipulates work packets. *)
    class WorkerTask on: taskScheduler with: initialDestination with: initialCount = Task on: taskScheduler (
        | protected destination <Integer> ::= initialDestination.
          protected count <Integer> ::= initialCount. |
    ) (
        public run: packet <Packet> ^<TaskControlBlock> = (
            packet isNil ifTrue: [^scheduler suspendCurrent].
            destination = handlerAIdentity
                ifTrue: [destination:: handlerBIdentity]
                ifFalse: [destination:: handlerAIdentity].
            packet identity: destination.
            packet datum: 0.
            1 to: dataSize do: [:index |
                count:: count + 1.
                count > 26 ifTrue: [count:: 1].
                packet data at: index put: count.
            ].
            ^scheduler queue: packet
        )
    )
    public bench = (
        | scheduler queue |
        scheduler:: Scheduler new.
        scheduler addIdleTask: idleIdentity priority: 0 work: nil state: idleCount.
        queue:: Packet link: nil identity: workerIdentity kind: kindWork.
        queue:: Packet link: queue identity: workerIdentity kind: kindWork.
        scheduler addWorkerTask: workerIdentity priority: 1000 work: queue.

        queue:: Packet link: nil identity: deviceAIdentity kind: kindDevice.
        queue:: Packet link: queue identity: deviceAIdentity kind: kindDevice.
        queue:: Packet link: queue identity: deviceAIdentity kind: kindDevice.
        scheduler addHandlerTask: handlerAIdentity priority: 2000 work: queue.

        queue:: Packet link: nil identity: deviceBIdentity kind: kindDevice.
        queue:: Packet link: queue identity: deviceBIdentity kind: kindDevice.
        queue:: Packet link: queue identity: deviceBIdentity kind: kindDevice.
        scheduler addHandlerTask: handlerBIdentity priority: 3000 work: queue.

        scheduler addDeviceTask: deviceAIdentity priority: 4000 work: nil.

        scheduler addDeviceTask: deviceBIdentity priority: 5000 work: nil.

        scheduler schedule.

        scheduler queueCount = 2322 ifFalse: [
            Error signal: 'RichardsTon80: expected queue count 2322, got ', scheduler queueCount printString].
        scheduler holdCount = 928 ifFalse: [
            Error signal: 'RichardsTon80: expected hold count 928, got ', scheduler holdCount printString].
        ^scheduler queueCount
    )
)
```
Its rewrites, beyond the header's list:
- The capitalised constants (`DATA_SIZE`, `COUNT`, `ID_IDLE` … `STATE_NOT_HELD`) become `dataSize`, `idleCount`, `idleIdentity` … `stateNotHeld`; `COUNT` becomes `idleCount` so that `IdleTask`'s slot `count` does not shadow it.
- The factory parameters `s`, `l`, `i`, `k`, `p`, `q`, `t`, `ctrl`, `cnt`, `v1`, `v2` get names of their own, and no longer share their slots' names; `v` becomes `queued`, `tcb` and `t` become `block`, `currentTcb` and `currentId` become `currentBlock` and `currentIdentity`, and `:i` becomes `:index`.
- The type annotations and the upstream comments stay as Primordial Soup wrote them, typo included. `Task>>run:`'s `subclassResponsibility` stays: no instance of `Task` itself is ever run, and our kernel has no such method, so it would end in `doesNotUnderstand:` if one were.
- The file is indented with spaces, as the other benchmarks are; Primordial Soup's uses tabs.

- [ ] **Step 3: Add them to the harness and the projects.**

The tuple becomes:
```newspeak
    | private benchmarks = {
        manifest Bounce. manifest List. manifest Mandelbrot. manifest NBody.
        manifest Permute. manifest Queens. manifest Richards.
        manifest RichardsTon80. manifest Sieve. manifest Storage.
        manifest Towers }. |
```
Add `"Richards.ns", "RichardsTon80.ns"` to `are-we-fast-yet.json` and the four `goldens/awfy/*.json` in the same order. `all_benchmarks_verify.toml` gains `Richards: ok` and `RichardsTon80: ok` after `Queens: ok`. `run.rs`'s names gain `"Richards"` and `"RichardsTon80"` after `"Queens"`.

- [ ] **Step 4: Widen the notices' introduction.**

In `THIRD-PARTY-NOTICES.md`, the bullet on `benchmarks/are-we-fast-yet/` becomes:
```markdown
- Newspeak code in `benchmarks/are-we-fast-yet/`: the benchmarks from SOMns,
  licensed MIT, and the harness and `RichardsTon80.ns` from Primordial Soup,
  licensed under the Apache License 2.0.
```

- [ ] **Step 5: Run each benchmark once, then the goldens and the per-commit gate.**

Run: `cargo run -q -p ns-cli -- run benchmarks/are-we-fast-yet/are-we-fast-yet.json -- Richards` and the same for `RichardsTon80`, then `cargo nextest run -p ns-cli awfy a_timed_run`, then the gate.
Expected: `Richards: ok`, `RichardsTon80: ok`, PASS. As in Task 5, an `expected … got …` from a benchmark means the port differs from its upstream; never change the expected value.

- [ ] **Step 6: Commit.**

```bash
git add benchmarks goldens/awfy crates/ns-cli/tests/run.rs THIRD-PARTY-NOTICES.md
git commit -m "feat: the Are We Fast Yet Richards and Primordial Soup's Ton80 Richards"
```

---

### Task 7: the documentation describes the built state

**Files:**
- Modify: `docs/design/core-library.md`, `docs/design/execution.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md`, `prior-art/PROVENANCE.md` (if it names what is adapted), `CLAUDE.md`

- [ ] **Step 1: `core-library.md`.**
  - "What exists", the per-class forwarder list (around lines 110-121): `Integer`'s gains `bitOr:`, `bitXor:` and `<<`; `LargeInteger`'s "the same"; `Float`'s gains `sqrt`. Add that `Integer>>bitInvert` is ordinary Newspeak, Primordial Soup's `^-1 - self`, and reaches no leaf.
  - "The leaves are the 98 rows" (around line 413) gives today's count of `mirror(` rows in `MIRROR`: count them (105 if nothing else changed).
  - "Large integers": its row counts; `bitOr:` and `bitXor:` are over two's complement like `bitAnd:`, through one `bitwise`; `<<` shifts the magnitude and refuses a result it cannot reserve.
  - "Gap: an integer has no `/`", last paragraph: "a `Float`'s arithmetic and ordering convert an integer argument … while an integer's raise a type mismatch" is no longer true. Rewrite it: both directions convert to doubles, as Primordial Soup's primitives do; `=` alone stays class-tight, and `//` and `\\` take integers only. Keep the rest of the section, which is still true: `Integer>>/`, `Fraction` and cross-class `=` wait for the tower.
  - Delete any other sentence that says an integer meeting a `Float` is a type mismatch (`rg -n "type mismatch" docs/design/core-library.md`).
- [ ] **Step 2: `execution.md`, "The Are We Fast Yet benchmarks".**
  - The benchmarks: "seven of the Are We Fast Yet benchmarks -- Bounce, List, Permute, Queens, Sieve, Storage and Towers --" becomes the ten of them (Bounce, List, Mandelbrot, NBody, Permute, Queens, Richards, Sieve, Storage, Towers), and Primordial Soup's own Richards, `RichardsTon80`, derived from the Dart Ton80 version, beside them.
  - Say that the harness is adapted from Primordial Soup's `BenchmarkRunner.ns` and `RichardsTon80.ns` from its `Richards.ns`, both Apache 2.0.
  - "with no name, all seven in turn" becomes "all eleven in turn".
  - Add a paragraph: Mandelbrot and NBody run at Are We Fast Yet's sizes (750; 250000 steps), each taking seconds per run in a release build, so `--timed` measures them with one run per phase: its score for them is one run's rate, not a throughput over many runs. Decided with the user on 2026-09-30, who chose the pinned sizes over smaller ones.
- [ ] **Step 3: `OPEN-WORK.md`.**
  - In "The road to a high-performance VM", the Are We Fast Yet item: stage 2 is built. Replace its numbered line 2 with a sentence in the paragraph: stage 2 (Mandelbrot, NBody, Richards, and Primordial Soup's Ton80 Richards beside them) is built and follows Primordial Soup's protocol, as stage 1 does (decided with the user on 2026-09-30). Stage 3 stays as its one numbered line. Whether stage 3 follows Primordial Soup's protocol is for its plan to settle.
  - Remove the sentence "Mixed arithmetic also closes `to:do:` with an `Integer` receiver … raises a type mismatch until it exists".
  - The baseline "over all fourteen" becomes "over all fifteen: the fourteen Are We Fast Yet benchmarks and Primordial Soup's Richards".
  - Add one item, with where it is explained: **`//` and `\\` with a `Float` operand.** Primordial Soup answers `7 // 2.5` and `7.5 // 2` as a floored `Float` (`vm/primitives.cc`, `Number_floorDivide`); ours raises a type mismatch for the first and `doesNotUnderstand:` for the second, since `Float` has neither selector. The on-demand rule adds them when a benchmark or runnable module sends them ([core-library.md](design/core-library.md), "Gap: an integer has no `/`").
- [ ] **Step 4: `PROGRESS.md`.**
  - "Where the next session starts" names stage 3, whose plan is still to be written, and keeps the eight-byte value after it.
  - A new milestone section, "Are We Fast Yet, stage 2", lists the commits of this plan with one line each, in the shape of stage 1's section, and the four decisions of this plan's header.
- [ ] **Step 5: `prior-art/PROVENANCE.md`.** If its primordialsoup entry names what is adapted from it, add `newspeak/Richards.ns`.
- [ ] **Step 6: `CLAUDE.md`.** "`benchmarks/are-we-fast-yet/` — seven Are We Fast Yet benchmarks and their harness" becomes "— ten Are We Fast Yet benchmarks, Primordial Soup's Ton80 Richards and their harness".
- [ ] **Step 7: Check the diff is Markdown only** (`git diff --stat`), then commit. A Markdown-only commit runs no gates.

```bash
git commit -m "docs: Are We Fast Yet stage 2, mixed arithmetic and the bit operations it added"
```

---

### Task 8: the end-of-batch gates

- [ ] **Step 1: Run the `finishing-a-batch` skill** and every gate it names:
  - the three stress modes, which now run Mandelbrot and NBody at full size under each: expect each stress suite to take several minutes longer than before, and give its command `timeout: 600000` or run it in the skill's parallel form;
  - `run-corpus` plain and under stress;
  - `cargo nextest run -p ns-lsp`;
  - `cargo nextest run --workspace --release` for the size tests;
  - its check that a fresh session can start from the documentation alone.

  `verifyPlugin` does not apply: nothing under `editors/jetbrains`, `ns-lsp` or `ns-syntax` changed. Confirm with `git diff --stat master`.
- [ ] **Step 2: Fix what a gate finds** in a commit of its own, naming the gate, and rerun that gate.
