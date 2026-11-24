# The kernel's numbers — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The fourth of the kernel completion's six sub-projects:
Primordial Soup's whole public numbers API -- `Float`'s mathematics and
texts, `Integer`'s and `Number`'s protocol, a `Fraction` -- on a symmetric
numeric tower: exact comparison across `Integer`, `LargeInteger`,
`Fraction` and `Float`, a hash that agrees with it, and exact arithmetic
wherever no `Float` takes part.

**Architecture:** Two pure modules carry the mathematics with no heap and
no `Value`: `numeric/` (the `Rational` in normal form, the numeric hash
modulo 2^61 - 1, a `Float`'s texts and its parser) beside the existing
`arbitrary_integer/` (which gains gcd, truncating division, a right shift,
base-256 digits and radix printing). `builtins/numbers/` is split into one
file per representation plus `tower.rs`, which classifies any number once
(`Operand`) and holds the tower's comparison, equality, hash and arithmetic
for every pair; the existing leaves keep their fast paths and hand every
other case to it, and a new `number:` family of mirror rows serves
`Number`'s own methods, which `Fraction` inherits. `Fraction` is a builtin
(tag 39) whose instances are made only by Rust, in normal form: a
fixed-pointer object of two fields, numerator and denominator, that no
Newspeak slot names; an image carries one inline as a value (tag 10).

**Tech Stack:** Rust 1.98.0 (`crates/ns-runtime`), Newspeak
(`crates/ns-runtime/core-library/Kernel.ns`, `PlatformFactory.ns`),
goldens (`goldens/**/*.ns` + `.toml`, run by
`crates/ns-cli/tests/run_goldens.rs` under both lowerings).

**Spec:** `docs/superpowers/specs/2026-10-11-kernel-numbers-design.md`.
Where this plan departs from it, it says so and why:

- **A fraction's object is `Format::FixedPointers` with two fields**, not
  an object format of its own. The collector already traces both words
  (`pointer_ranges`' fixed-pointer arm, every `Trace` of a heap object), so
  neither needs a new arm. What makes the object a fraction is its class's
  builtin, `Fraction`, together with exactly two fields: `Fraction new`
  (the class declares no slot, so zero fields) and an instance of a
  program's subclass (whose class's builtin is no `Fraction`) are no
  number. `Value`'s declared-slot walk sees no slot in it, since the class
  declares none.
- **`quo:`, `rem:` and `//` on a `Float`** are built in Task 8 with the
  rest of the tower's arithmetic, not in phase 3 as the spec lists them:
  one function answers all four over every pair of numbers, so building
  an integer-only copy first would be thrown away.
- **`Number`'s arithmetic and comparison are `number:` rows**, a family
  the spec does not name: `Fraction` inherits them from `Number`, and
  `Integer`, `LargeInteger` and `Float` keep their own rows for their fast
  paths and send everything else to the same `tower.rs` functions, so the
  error texts and the answers cannot diverge.
- **A value's derived equality compares two numbers by the tower before
  it compares their classes.** `builtins/value.rs` checks the classes of
  every pair first (a rule of ns-next's, not of §3.1.1, which says nothing
  on how `Value>>=` compares); for two numbers the tower decides instead,
  as decision 2 of the spec asks.
- **The goldens are written per task**, each beside the code it pins,
  under the directories the spec names.

## Global Constraints

- Communicate with the user in German; all code, comments, identifiers,
  commit messages and documentation in English.
- Names are written out (CLAUDE.md, "Conventions"): no identifier of one or
  two characters, no truncated word, in Rust and in Newspeak; clippy's
  `min_ident_chars` enforces the short ones.
- **Never `{:?}` a `Value`, never `.unwrap()`/`.expect()`/`{:?}` an
  `EvaluationResult`**; tests use the file's local `expect_ok` /
  `expect_error` helpers -- copy a neighbour's.
- **`vm.rs` and `compiler/` do not change.** The new leaves live in
  `builtins/`; no dispatch or error text goes anywhere else.
- **Every store into an existing object goes through `heap.rs`'s store
  accessors.** A fraction's two words are written only by its allocation,
  an initializing store into an object just allocated.
- **A wire tag is assigned once** (`image/opcodes.rs`,
  `image/encode.rs`'s `builtin_class_tag`, `image/decode.rs`'s builtin
  mapping, `tests/opcode_tags.rs`): `Fraction` takes builtin class tag
  **39**, `VALUE_FRACTION` value tag **10**. Nothing is renumbered.
- **An image is untrusted input**: the decoder refuses a fraction that is
  not in normal form and never panics on one; its allocation stays inside
  the bound `decode_value`'s doc comment derives (Task 6 updates the
  derivation).
- **The VM mirror is a capability**: the new rows are leaves on the
  existing mirror; no row hands out the mirror, its class or `Top`.
- **Program outcomes are pinned**: a golden whose output changes changes
  because the decided semantics changed -- a `Float`'s text (Task 2), `=`
  across classes (Task 7), `/` and the `//` family on every number
  (Task 8); say so in its comment and in the commit message. Every golden
  runs under both lowerings.
- **The documentation rule**: every member of `Kernel.ns`,
  `PlatformFactory.ns` and `CollectionsFactory.ns` -- public, protected and
  private, class and slot included -- has a `(* … *)` comment directly
  above it, to the standard of Javadoc: what it does and answers, each
  argument and what it must be, every Error it signals and when, and what
  it guarantees, as long as that takes; a section that would say nothing
  is left out. A comment explaining a decision follows the doc comment as
  a second `(* … *)`. No comment in a shipped module names another project
  (no "Primordial Soup", "Python", "num-rational").
  `crates/ns-runtime/tests/core_library_documentation.rs` holds it.
- **The bodies are written here**: the API is Primordial Soup's, the code
  is not. Do not open the bodies of `Number`, `Integer`, `Float` or
  `Fraction` in `prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`;
  its tests (`KernelTests.ns`) and this plan are the description. No code
  is copied from `num-rational` (its licence is not taken on; decided with
  the user on 2026-10-11); `num-integer`'s, whose licence is taken on
  (`THIRD-PARTY-NOTICES.md`), may be adapted with the header the files of
  `arbitrary_integer/` carry.
- **Comments explain the decision**, not the mechanics; a Newspeak module
  inline in a Rust test is an `indoc! {"…"}`, indented with spaces, the
  closing `"}` on its own line. `.ns` files outside `corpus/` are indented
  with four spaces (`crates/ns-cli/tests/source_formatting.rs`).
- **Documentation describes the current state**: when a task changes
  behaviour, the design-document sentences and the doc comments
  describing the old behaviour are rewritten in the same commit (no
  "previously", no "historical"). `builtins/numbers/mod.rs`'s module note
  is rewritten by the task that changes what it says.
- Conventional commits; every commit message ends with
  ```
  Co-Authored-By: <the implementing model's name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01Jkf2ZQ7tKiV22DYxa5NbfA
  ```
  where the first line names the implementer's own model. Never override
  the author or committer identity.
- Work on the checked-out branch `feature/kernel-numbers` in
  `D:\Workspace\ns-next`; no worktree.
- Run every command in the foreground, never in the background; pass a
  600000 ms timeout to builds and test suites.
- **The per-commit gates** (CLAUDE.md, "Working in this repository"), run
  from the repository root before every commit, all green:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace
  cargo test --workspace --doc
  cargo run -p ns-analyzer -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  ```
  The end-of-batch gates (the five stress modes, `run-corpus` plain and
  stressed, the benchmarks profile, the release size tests,
  `verifyPlugin`) are **not** run per task; the controller runs them once
  after Task 10 (the `finishing-a-batch` skill). Every commit before that
  is work in progress. No task touches `ns-syntax`, `ns-analyzer`'s
  sources, `schemas/` or the plugin, so the Gradle gate does not apply.
- Run one golden or a family with
  `cargo nextest run -p ns-cli --test run_goldens <part of its path>`.
- A golden's `.toml` holds `stdout = '''…'''` (and `status`/`stderr` for a
  failing run); copy a neighbour's. Establish each expected output by
  reasoning from this plan before running it; if the run disagrees, find
  out which is wrong -- never copy the run's output into the `.toml`
  because the code produced it.
- `corpus/runnable.toml` and `goldens/tool/` hold outputs too: when a
  float's text or `=` changes one, the same rule applies.

## Review Focus

- **Doubles at the edges of the integers** -- `2^53`, `2^53 + 1`, `2^60`,
  `2^61 - 1`, `-2^60`, a `LargeInteger` beyond `f64::MAX`, `-0.0`, the
  smallest subnormal: `=` must stay transitive, `<` must agree with `=`,
  and equal numbers must hash alike (Task 5's and Task 7's property tests,
  Task 7's golden).
- **A fraction too large or too small for a double** -- `asFloat` of
  `(10 raisedTo: 400) / 3` answers `Infinity`, of `1 / (10 raisedTo: 400)`
  `0.0`, of numerator and denominator both beyond `f64::MAX` a finite
  double (where dividing two `asFloat`s would give NaN); ties round to
  even (Task 5's tests, Task 8's golden).
- **Numbers as keys and as slots** -- `1` and `1.0` and `2/2` are one key
  of a `Map` and one element of a `Set`; a value with slot `3` equals and
  hashes like one with slot `3.0`; a NaN key is found again (Task 7's
  golden).
- **A Float's text read back** -- `Float parse: x printString` answers `x`
  for every finite `x`, `-0.0`, `Infinity` and `NaN` included; `nan`,
  `inf`, an empty string and trailing text are refused (Task 2's golden
  and tests).
- **An untrusted fraction in an image** -- a denominator of 0, 1 or below,
  a common factor, a part that is no integer: refused with a decode error,
  never a panic, never a fraction that breaks `=` (Task 6's tests).

---

### Task 1: `builtins/numbers.rs` split, one file per representation

**Files:**
- Move: `crates/ns-runtime/src/builtins/numbers.rs` →
  `crates/ns-runtime/src/builtins/numbers/mod.rs` (`git mv`, so history
  follows)
- Create: `crates/ns-runtime/src/builtins/numbers/integer.rs`
- Create: `crates/ns-runtime/src/builtins/numbers/large_integer.rs`
- Create: `crates/ns-runtime/src/builtins/numbers/float.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: the same paths every caller uses today, re-exported from
  `builtins::numbers` -- `execute_integer`, `execute_large_integer`,
  `execute_float`, `integer_equals`, `large_integer_equals`,
  `large_integer_hash`, `float_equals`, `float_hash`, `fixed_text`,
  `checked_floor_divide`, `floor_modulo` -- and, in `numbers/mod.rs`, the
  helpers the three files share, `pub(super)`: `Accepted`,
  `type_mismatch`, `Answer`, `promote`, `division_by_zero`,
  `integer_operand`, `float_operand`, `with_small_integer_parts`,
  `integer_ordering`, `shift_left`.

No behaviour changes, so no new test: the suites are the test.

- [ ] **Step 1: Move the file**

```bash
mkdir crates/ns-runtime/src/builtins/numbers
git mv crates/ns-runtime/src/builtins/numbers.rs crates/ns-runtime/src/builtins/numbers/mod.rs
```

- [ ] **Step 2: Split it**

Into `integer.rs`: `integer_equals` and `execute_integer`. Into
`large_integer.rs`: `large_integer_hash`, `large_integer_equals` and
`execute_large_integer`. Into `float.rs`: `float_equals`, `float_hash`,
`fixed_text` and `execute_float`. Each unit test goes with the function it
tests (`every_nan_equals_every_other_and_hashes_alike` and
`fixed_text_rounds_a_tie_up_on_the_exact_binary_value` to `float.rs`; the
floor tests stay in `mod.rs` with `checked_floor_divide` and
`floor_modulo`). The module note and the shared helpers stay in `mod.rs`,
the helpers made `pub(super)`. `mod.rs` declares and re-exports:

```rust
mod float;
mod integer;
mod large_integer;

pub(crate) use float::{execute_float, fixed_text, float_equals, float_hash};
pub(crate) use integer::{execute_integer, integer_equals};
pub(crate) use large_integer::{execute_large_integer, large_integer_equals, large_integer_hash};
```

Each new file starts with a one-paragraph `//!` note naming what it holds
(e.g. "`Float`'s leaves: IEEE 754 doubles, immediate or boxed. The rules
across the numeric classes are the module note's.").

- [ ] **Step 3: Run the gates**

All five per-commit gates. Expected: green, no golden changed, no test
added or removed (compare `cargo nextest run --workspace` totals with the
run before Step 1).

- [ ] **Step 4: Commit**

```bash
git add crates/ns-runtime/src/builtins/numbers
git commit -m "refactor: builtins/numbers split into one file per representation"
```
(with the two trailer lines).

---

### Task 2: A `Float`'s texts: shortest, fixed, exponential, precision, and `Float parse:`

**Files:**
- Create: `crates/ns-runtime/src/numeric/mod.rs`
- Create: `crates/ns-runtime/src/numeric/float_text.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (`pub(crate) mod numeric;`)
- Modify: `crates/ns-runtime/src/builtins/numbers/float.rs` (`fixed_text`
  moves to `numeric::float_text::fixed`; `PrintString`,
  `AsStringFixed`, the `asInteger` error text; three new leaves)
- Modify: `crates/ns-runtime/src/builtins/numbers/mod.rs` (re-export list)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs`
  (`FloatPrimitive::AsStringExponential`, `AsStringPrecision`, `Parse`;
  three rows; `answers_false_to_a_stranger`)
- Modify: `crates/ns-runtime/src/heap.rs` (`Heap::describe` writes a float
  as `numeric::float_text::shortest`)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Float`:
  `printString`'s comment, `asStringFixed:`'s comment,
  `asStringExponential:`, `asStringPrecision:`, a class side with
  `parse:`)
- Create: `goldens/library/number/float/a_float_prints_as_its_shortest_text.{ns,toml}`
- Create: `goldens/library/number/float/a_float_writes_fixed_exponential_and_precision_texts.{ns,toml}`
- Create: `goldens/library/number/float/a_float_is_parsed_from_its_text.{ns,toml}`
- Modify: every golden, `corpus/runnable.toml` entry and `goldens/tool/`
  output whose float text changes (Step 7)

**Interfaces:**
- Consumes: nothing new.
- Produces: `crate::numeric::float_text::{shortest, fixed, exponential,
  precision, parse}` -- `shortest(value: f64) -> String`,
  `fixed(value: f64, fraction_digits: usize) -> String`,
  `exponential(value: f64, fraction_digits: usize) -> String`,
  `precision(value: f64, significant_digits: usize) -> String`,
  `parse(text: &str) -> Option<f64>`; on `Float`: `asStringExponential:`,
  `asStringPrecision:`, class-side `parse:`; mirror rows
  `float:asStringExponential:ifFail:`, `float:asStringPrecision:ifFail:`,
  `floatParse:ifFail:`.

The texts are Primordial Soup's (`KernelTests.ns`'s `testFloatAsString`,
`testFloatAsStringFixed`, `testFloatAsStringExponential`,
`testFloatAsStringPrecision`, `testFloatParse`): the shortest digits that
read back as the same double, laid out in decimal when the exponent `e`
of the first digit (the value is `d.ddd × 10^e`) satisfies `-6 <= e < 21`
and in exponent form otherwise, with `.0` after an integral decimal text;
`Infinity`, `-Infinity`, `NaN`; a `-` for every negative value, `-0.0`
included. The fixed, exponential and precision texts round half up on the
value's exact binary expansion, as `fixed_text` already does.

- [ ] **Step 1: Write the failing goldens**

`a_float_prints_as_its_shortest_text.ns`:

```newspeak
(* A Float prints as the shortest text that reads back as the same
   double: in decimal while the exponent of its first digit lies from -6
   to 20, with `.0` when the text would look integral, and in exponent
   form beyond; the infinities and NaN by name. `printString` and
   `asString` answer the same, and so does the description an error
   message gives. *)
class AFloatPrintsAsItsShortestText = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. |
        transcript print: 3.0; newLine.
        transcript print: -0.0; newLine.
        transcript print: 1.5; newLine.
        transcript print: 0.1; newLine.
        transcript print: 1.0 / 3.0; newLine.
        transcript print: 100000000000000000000 asFloat; newLine.
        transcript print: 1000000000000000000000 asFloat; newLine.
        transcript print: 123456789012345678901234567890 asFloat; newLine.
        transcript print: 0.000001; newLine.
        transcript print: 0.0000001; newLine.
        transcript print: 1.0 / 0.0; newLine.
        transcript print: -1.0 / 0.0; newLine.
        transcript print: 0.0 / 0.0; newLine.
        transcript show: 2.5 asString; newLine.
        transcript print: 4 * 3.5; newLine.
        transcript show: ([(-1.0 / 0.0) asInteger] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([3 bitAnd: 2.0] on: Error do: [:error | error messageText]); newLine
    )
)
```

`a_float_prints_as_its_shortest_text.toml`:

```toml
stdout = '''
3.0
-0.0
1.5
0.1
0.3333333333333333
100000000000000000000.0
1e+21
1.2345678901234568e+29
0.000001
1e-7
Infinity
-Infinity
NaN
2.5
14.0
asInteger requires a finite Float, got -Infinity
bitAnd: requires an integer argument, got 2.0
'''
```

`a_float_writes_fixed_exponential_and_precision_texts.ns`:

```newspeak
(* `asStringExponential:` writes one digit before the point and the
   given number after it; `asStringPrecision:` the given number of
   significant digits, in exponent form once the exponent is below -6 or
   the digits do not reach the point. Both round half up on the exact
   binary value; the non-finite values are written by name, as
   `asStringFixed:` writes them and every value at or beyond 1e21. *)
class AFloatWritesFixedExponentialAndPrecisionTexts = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. |
        transcript show: (3.0 asStringExponential: 0); newLine.
        transcript show: (3.0 asStringExponential: 1); newLine.
        transcript show: (1.5 asStringExponential: 0); newLine.
        transcript show: ((200.0 / 3.0) asStringExponential: 2); newLine.
        transcript show: (0.0 asStringExponential: 1); newLine.
        transcript show: (-0.0 asStringExponential: 0); newLine.
        transcript show: ((1.0 / 3.0) asStringExponential: 1); newLine.
        transcript show: ((1.0 / 0.0) asStringExponential: 2); newLine.
        transcript show: (3.0 asStringPrecision: 1); newLine.
        transcript show: (3.0 asStringPrecision: 2); newLine.
        transcript show: ((200.0 / 3.0) asStringPrecision: 1); newLine.
        transcript show: ((200.0 / 3.0) asStringPrecision: 2); newLine.
        transcript show: ((200.0 / 3.0) asStringPrecision: 3); newLine.
        transcript show: ((1.0 / 3.0) asStringPrecision: 1); newLine.
        transcript show: (-0.0 asStringPrecision: 2); newLine.
        transcript show: (0.000001 asStringPrecision: 2); newLine.
        transcript show: (0.0000001 asStringPrecision: 2); newLine.
        transcript show: ((0.0 / 0.0) asStringPrecision: 3); newLine.
        transcript show: (1000000000000000000000 asFloat asStringFixed: 2); newLine.
        transcript show: ((-1.0 / 0.0) asStringFixed: 1); newLine.
        transcript show: ([3.0 asStringExponential: 21] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([3.0 asStringPrecision: 0] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([3.0 asStringPrecision: 22] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([3.0 asStringPrecision: nil] on: Error do: [:error | error messageText]); newLine
    )
)
```

`a_float_writes_fixed_exponential_and_precision_texts.toml`:

```toml
stdout = '''
3e+0
3.0e+0
2e+0
6.67e+1
0.0e+0
-0e+0
3.3e-1
Infinity
3
3.0
7e+1
67
66.7
0.3
-0.0
0.0000010
1.0e-7
NaN
1e+21
-Infinity
asStringExponential: takes 0 to 20 fraction digits, got 21
asStringPrecision: takes 1 to 21 digits, got 0
asStringPrecision: takes 1 to 21 digits, got 22
asStringPrecision: requires an integer argument, got nil
'''
```

`a_float_is_parsed_from_its_text.ns`:

```newspeak
(* `Float parse:` reads a decimal text -- an optional sign, digits with
   an optional point, an optional exponent -- and the names `Infinity`
   and `NaN`, which is every text `printString` writes, so a Float's text
   reads back as the same Float. Any other text is an ArgumentError
   carrying it, the lower-case names and stray characters included. *)
class AFloatIsParsedFromItsText = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. third = 1.0 / 3.0. |
        transcript print: (Float parse: '3'); newLine.
        transcript print: (Float parse: '3.'); newLine.
        transcript print: (Float parse: '-3'); newLine.
        transcript print: (Float parse: '0003.0000'); newLine.
        transcript print: (Float parse: '1.5'); newLine.
        transcript print: (Float parse: '2.5e-3'); newLine.
        transcript print: (Float parse: '1e+21'); newLine.
        transcript print: (Float parse: 'NaN'); newLine.
        transcript print: (Float parse: 'Infinity'); newLine.
        transcript print: (Float parse: '-Infinity'); newLine.
        transcript print: (Float parse: third printString) = third; newLine.
        transcript print: (Float parse: -0.0 printString); newLine.
        transcript show: ([Float parse: ''] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Float parse: 'nan'] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Float parse: 'inf'] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Float parse: '--9'] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Float parse: '3 '] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Float parse: 3] on: Error do: [:error | error messageText]); newLine
    )
)
```

`a_float_is_parsed_from_its_text.toml`:

```toml
stdout = '''
3.0
3.0
-3.0
3.0
1.5
0.0025
1e+21
NaN
Infinity
-Infinity
true
-0.0
parse: requires the text of a Float, got the String ""
parse: requires the text of a Float, got the String "nan"
parse: requires the text of a Float, got the String "inf"
parse: requires the text of a Float, got the String "--9"
parse: requires the text of a Float, got the String "3 "
parse: requires a String, got 3
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens a_float_`
Expected: FAIL -- `3.0` prints `3`, `asStringExponential:` is not
understood, `Float parse:` is not understood.

- [ ] **Step 3: `numeric::float_text`**

`crates/ns-runtime/src/numeric/mod.rs`:

```rust
//! Numbers with no heap and no `Value`: what the numeric leaves compute
//! and write, kept apart so that it is tested as plain Rust and so that
//! `Heap::describe` can write a float as `Float>>printString` does.
//! `arbitrary_integer` beside it is the integers' arithmetic.

pub(crate) mod float_text;
```

`crates/ns-runtime/src/numeric/float_text.rs`:

```rust
//! A `Float`'s texts: `printString`'s shortest text, the fixed, exponential
//! and precision texts `asStringFixed:`, `asStringExponential:` and
//! `asStringPrecision:` write, and the parser `Float parse:` reads them
//! back with (`docs/design/core-library.md`, "A Float's text").

/// The decimal digits of a magnitude and where its point goes: the value
/// is `0.d1 d2 d3 … × 10^point`. `digits` has no leading zero but for zero
/// itself.
struct Decimal {
    digits: Vec<u8>,
    point: i32,
}

/// The exponent beyond which `shortest` writes exponent form: the first
/// digit's power of ten must lie in `-6..21` for the decimal form.
const DECIMAL_EXPONENTS: std::ops::Range<i32> = -6..21;

/// The most leading zeros `precision` writes after the point before it
/// turns to exponent form.
const PRECISION_LEADING_ZEROS: i32 = 6;

/// `NaN`, `Infinity` or `-Infinity` for a value that has no digits.
fn named(value: f64) -> Option<String> {
    if value.is_nan() {
        Some("NaN".to_string())
    } else if value.is_infinite() {
        Some(if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string())
    } else {
        None
    }
}

/// The text's sign: a `-` for every negative value, `-0.0` included.
fn signed(value: f64) -> String {
    if value.is_sign_negative() {
        "-".to_string()
    } else {
        String::new()
    }
}

/// Splits Rust's exponent form, `d.ddde±x`, into digits and a point.
fn decimal_of_exponent_form(text: &str) -> Decimal {
    let (mantissa, exponent) = text
        .split_once('e')
        .expect("an exponent format writes an e");
    let exponent: i32 = exponent.parse().expect("an exponent is an integer");
    Decimal {
        digits: mantissa.bytes().filter(|byte| *byte != b'.').collect(),
        point: exponent + 1,
    }
}

/// The shortest digits that read back as `magnitude`, which Rust's `{:e}`
/// writes.
fn shortest_decimal(magnitude: f64) -> Decimal {
    decimal_of_exponent_form(&format!("{magnitude:e}"))
}

/// `significant` digits of `magnitude`, rounded half up on its exact binary
/// value. A finite double's exact expansion has at most 767 significant
/// digits, so 800 after the first are the value itself, not a rounding.
fn rounded_decimal(magnitude: f64, significant: usize) -> Decimal {
    let exact = decimal_of_exponent_form(&format!("{magnitude:.800e}"));
    let mut digits = exact.digits[..significant].to_vec();
    let mut point = exact.point;
    if exact.digits[significant] >= b'5' {
        let mut position = digits.len();
        loop {
            if position == 0 {
                // Every digit was 9: 9.99 rounds to 10.0, one more digit
                // before the point, and the last digit drops.
                digits.insert(0, b'1');
                digits.pop();
                point += 1;
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
    Decimal { digits, point }
}

fn push_digits(text: &mut String, digits: &[u8]) {
    text.push_str(std::str::from_utf8(digits).expect("ASCII digits"));
}

fn push_zeros(text: &mut String, count: i32) {
    for _ in 0..count.max(0) {
        text.push('0');
    }
}

/// The decimal form: `digits` with the point after `point` of them,
/// padded with zeros to `digits_after_point` digits after it, and `.0`
/// when there are none and `trailing_zero` asks for it.
fn decimal_form(
    text: &mut String,
    digits: &[u8],
    point: i32,
    digits_after_point: i32,
    trailing_zero: bool,
) {
    let length = digits.len() as i32;
    if point <= 0 {
        text.push('0');
        if digits_after_point > 0 {
            text.push('.');
            push_zeros(text, -point);
            push_digits(text, digits);
            push_zeros(text, digits_after_point - (-point + length));
        }
    } else if point >= length {
        push_digits(text, digits);
        push_zeros(text, point - length);
        if digits_after_point > 0 {
            text.push('.');
            push_zeros(text, digits_after_point);
        }
    } else {
        let split = point as usize;
        push_digits(text, &digits[..split]);
        text.push('.');
        push_digits(text, &digits[split..]);
        push_zeros(text, digits_after_point - (length - point));
    }
    if digits_after_point <= 0 && trailing_zero {
        text.push_str(".0");
    }
}

/// The exponent form: the first digit, the rest after a point, `e`, and
/// the exponent with its sign, `+` included.
fn exponential_form(text: &mut String, digits: &[u8], exponent: i32) {
    push_digits(text, &digits[..1]);
    if digits.len() > 1 {
        text.push('.');
        push_digits(text, &digits[1..]);
    }
    text.push('e');
    text.push(if exponent < 0 { '-' } else { '+' });
    text.push_str(&exponent.unsigned_abs().to_string());
}

/// `Float>>printString`: the shortest text that reads back as `value`.
pub(crate) fn shortest(value: f64) -> String {
    if let Some(name) = named(value) {
        return name;
    }
    let decimal = shortest_decimal(value.abs());
    let mut text = signed(value);
    let exponent = decimal.point - 1;
    if DECIMAL_EXPONENTS.contains(&exponent) {
        let after_point = decimal.digits.len() as i32 - decimal.point;
        decimal_form(&mut text, &decimal.digits, decimal.point, after_point, true);
    } else {
        exponential_form(&mut text, &decimal.digits, exponent);
    }
    text
}

/// `Float>>asStringFixed:`: `fraction_digits` digits after the point, a tie
/// rounded up on the exact binary value (Rust's formatter would round it
/// to even), and the value's own sign even when the digits round to zero.
/// Outside ±1e21, and for a NaN or an infinity, `shortest`'s text.
pub(crate) fn fixed(value: f64, fraction_digits: usize) -> String {
    // The body of today's `fixed_text`, moved unchanged, except that its
    // first branch answers `shortest(value)` instead of `value.to_string()`.
}

/// `Float>>asStringExponential:`: one digit before the point and
/// `fraction_digits` after it, rounded half up, then the exponent.
pub(crate) fn exponential(value: f64, fraction_digits: usize) -> String {
    if let Some(name) = named(value) {
        return name;
    }
    let decimal = rounded_decimal(value.abs(), fraction_digits + 1);
    let mut text = signed(value);
    exponential_form(&mut text, &decimal.digits, decimal.point - 1);
    text
}

/// `Float>>asStringPrecision:`: `significant_digits` significant digits,
/// rounded half up; in exponent form when the value needs more than
/// `PRECISION_LEADING_ZEROS` zeros after the point or its digits end
/// before the point.
pub(crate) fn precision(value: f64, significant_digits: usize) -> String {
    if let Some(name) = named(value) {
        return name;
    }
    let decimal = rounded_decimal(value.abs(), significant_digits);
    let mut text = signed(value);
    let wanted = significant_digits as i32;
    if 1 - decimal.point > PRECISION_LEADING_ZEROS || decimal.point - wanted > 0 {
        exponential_form(&mut text, &decimal.digits, decimal.point - 1);
    } else {
        let after_point = (wanted - decimal.point).max(0);
        decimal_form(&mut text, &decimal.digits, decimal.point, after_point, false);
    }
    text
}

/// `Float class>>parse:`: an optional `-` or `+`, then `Infinity`, `NaN`,
/// or digits with an optional point and an optional exponent (`e` or `E`,
/// an optional sign, digits), at least one digit before the exponent.
/// `None` for anything else, a lower-case name and a space included.
pub(crate) fn parse(text: &str) -> Option<f64> {
    let (negative, unsigned) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let magnitude = match unsigned {
        "Infinity" => f64::INFINITY,
        "NaN" => f64::NAN,
        _ if is_decimal_text(unsigned) => unsigned.parse::<f64>().ok()?,
        _ => return None,
    };
    Some(if negative { -magnitude } else { magnitude })
}

/// Whether `text` is digits, an optional point and digits -- one digit at
/// least -- and an optional exponent, and nothing else. Rust's own parser
/// takes more (`inf`, `nan`, `infinity` in any case), so the grammar is
/// checked first.
fn is_decimal_text(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits_from = |start: usize| {
        bytes[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let mut index = digits_from(0);
    let mut mantissa_digits = index;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction = digits_from(index);
        mantissa_digits += fraction;
        index += fraction;
    }
    if mantissa_digits == 0 {
        return false;
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent = digits_from(index);
        if exponent == 0 {
            return false;
        }
        index += exponent;
    }
    index == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::{exponential, fixed, parse, precision, shortest};

    #[test]
    fn shortest_writes_decimal_from_the_sixth_place_to_the_twenty_first_digit() {
        assert_eq!(shortest(3.0), "3.0");
        assert_eq!(shortest(-0.0), "-0.0");
        assert_eq!(shortest(0.1), "0.1");
        assert_eq!(shortest(1e20), "100000000000000000000.0");
        assert_eq!(shortest(1e21), "1e+21");
        assert_eq!(shortest(1e-6), "0.000001");
        assert_eq!(shortest(1e-7), "1e-7");
        assert_eq!(shortest(f64::MAX), "1.7976931348623157e+308");
        assert_eq!(shortest(-f64::MAX), "-1.7976931348623157e+308");
        assert_eq!(shortest(f64::from_bits(1)), "5e-324");
        assert_eq!(shortest(f64::INFINITY), "Infinity");
        assert_eq!(shortest(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(shortest(f64::NAN), "NaN");
    }

    #[test]
    fn exponential_and_precision_round_half_up() {
        assert_eq!(exponential(3.0, 2), "3.00e+0");
        assert_eq!(exponential(9.99, 1), "1.0e+1");
        assert_eq!(exponential(0.125, 1), "1.3e-1");
        assert_eq!(precision(0.125, 2), "0.13");
        assert_eq!(precision(99.95, 3), "100");
        assert_eq!(precision(123456.0, 3), "1.23e+5");
        assert_eq!(precision(f64::NEG_INFINITY, 3), "-Infinity");
        assert_eq!(fixed(f64::NAN, 2), "NaN");
        assert_eq!(fixed(1e21, 2), "1e+21");
    }

    #[test]
    fn every_finite_shortest_text_parses_back_to_its_value() {
        let values = [
            0.0, -0.0, 1.0, 0.1, 1.0 / 3.0, 1e21, 1e-7, 123.456e-300,
            f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1), -2.5e-3,
        ];
        for value in values {
            let text = shortest(value);
            let parsed = parse(&text).unwrap_or_else(|| panic!("{text} parses"));
            assert_eq!(parsed.to_bits(), value.to_bits(), "{text}");
        }
        assert!(parse("NaN").is_some_and(f64::is_nan));
        assert_eq!(parse("-Infinity"), Some(f64::NEG_INFINITY));
    }

    #[test]
    fn parse_refuses_what_print_string_never_writes() {
        for text in ["", "nan", "inf", "infinity", "NAN", "--9", "3 ", " 3", "1e", "e5", ".", "+", "0x10"] {
            assert_eq!(parse(text).map(f64::to_bits), None, "{text:?}");
        }
        assert_eq!(parse("3."), Some(3.0));
        assert_eq!(parse(".5"), Some(0.5));
        assert_eq!(parse("+2E3"), Some(2000.0));
    }
}
```

(`fixed`'s body is `fixed_text`'s, moved from `numbers/float.rs` with its
test, `fixed_text_rounds_a_tie_up_on_the_exact_binary_value`, whose last
three assertions become `fixed(1e21, 2) == "1e+21"`, `fixed(NaN, 2) ==
"NaN"`, `fixed(-inf, 2) == "-Infinity"`. Remove `fixed_text` from
`numbers/float.rs` and from the re-export list.)

- [ ] **Step 4: The leaves and their rows**

`primitive.rs`: add `AsStringExponential`, `AsStringPrecision` and
`Parse` to `FloatPrimitive`, all three `false` in
`answers_false_to_a_stranger`, and rows after
`float:asStringFixed:ifFail:`:

```rust
    mirror(
        "float:asStringExponential:ifFail:",
        "asStringExponential:",
        Primitive::Float(FloatPrimitive::AsStringExponential),
    ),
    mirror(
        "float:asStringPrecision:ifFail:",
        "asStringPrecision:",
        Primitive::Float(FloatPrimitive::AsStringPrecision),
    ),
    mirror(
        "floatParse:ifFail:",
        "parse:",
        Primitive::Float(FloatPrimitive::Parse),
    ),
```

`numbers/float.rs`: `Parse` is sent to the class, so `execute_float`
answers it before it reads the receiver as a float:

```rust
    if let FloatPrimitive::Parse = primitive {
        return Some(parse(mutator, receiver, arguments));
    }
    let receiver_float = mutator.heap.float_value(receiver)?;
```

```rust
/// `Float class>>parse:`: the Float a text names (`numeric::float_text::parse`
/// says which texts), or an `ArgumentError` carrying the argument.
fn parse<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = Symbol::intern("parse:");
    let argument = first_argument(arguments);
    let text = argument
        .as_object()
        .filter(|object| !mutator.heap.is_symbol(*object))
        .and_then(|object| mutator.heap.str_value(object));
    let Some(text) = text else {
        return Err(NewspeakError::argument_error(
            selector,
            receiver,
            argument,
            format!("parse: requires a String, got {}", mutator.heap.describe(argument)),
        ));
    };
    match crate::numeric::float_text::parse(&text) {
        Some(value) => Ok(mutator.new_float(value)),
        None => Err(NewspeakError::argument_error(
            selector,
            receiver,
            argument,
            format!(
                "parse: requires the text of a Float, got {}",
                mutator.heap.describe(argument)
            ),
        )),
    }
}
```

(Check how `builtins/text.rs` reads a String argument -- if it has a
helper that answers a String's text and refuses a Symbol, use it instead
of the three lines above.)

`PrintString` answers `numeric::float_text::shortest(receiver_float)`;
`AsStringFixed` calls `numeric::float_text::fixed`; the `AsInteger`
refusal's text is `format!("asInteger requires a finite Float, got {}",
float_text::shortest(receiver_float))`. The two new digit-count leaves
share one helper with `AsStringFixed`'s range check:

```rust
/// A digit count for `asStringFixed:`, `asStringExponential:` or
/// `asStringPrecision:` within `range`, or the `ArgumentError` its
/// selector signals: `out_of_range` names the range in words.
fn digit_count<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: &str,
    range: std::ops::RangeInclusive<i64>,
    out_of_range: &str,
    accepted: Accepted,
    arguments: &[Value<'gc>],
) -> Result<usize, NewspeakError<'gc>>
```

It answers the count for an `IntegerArgument::Small` within `range`;
an `ArgumentError` carrying the argument with
`"{selector} {out_of_range}, got {count}"` for one outside it or a
`LargeInteger` (described by `describe_argument`); and
`type_mismatch(…, accepted, …)` for anything else. `AsStringFixed` passes
`0..=20`, `"takes 0 to 20 fraction digits"`, `Accepted::Number` (its
pinned text says "numeric"); `AsStringExponential` the same range and
words with `Accepted::Integer`; `AsStringPrecision` `1..=21`,
`"takes 1 to 21 digits"`, `Accepted::Integer`.

`heap.rs`'s `Heap::describe`: the `SmallFloat` arm and the boxed-float
arm write `crate::numeric::float_text::shortest(float)` instead of
`float.to_string()`.

- [ ] **Step 5: `Float` in `Kernel.ns`**

Replace `printString`'s doc comment, extend `asStringFixed:`'s, add the
two instance methods after it, and give `Float` a class side (the class
currently ends `)` -- it becomes `) : ( … )`):

```newspeak
        (* Answers a new String of the receiver with `fractionDigits`
           digits after the point, rounded half away from zero on its exact
           binary value, keeping its sign even when every digit is zero;
           `printString`'s text when it is not within ±1e21, a NaN or an
           infinity included.

           fractionDigits: an Integer from 0 to 20.

           Signals an ArgumentError carrying `fractionDigits`:
           'asStringFixed: takes 0 to 20 fraction digits, got …' for an
           Integer outside 0 to 20, a LargeInteger included, and
           'asStringFixed: requires a numeric argument, got …' for one that
           is no Integer. *)
        public asStringFixed: fractionDigits = (
            ^vmMirror float: self asStringFixed: fractionDigits
                ifFail: [^super asStringFixed: fractionDigits]
        )
        (* Answers a new String of the receiver in exponent form: one digit,
           a point and `fractionDigits` digits when there are any, rounded
           half away from zero on its exact binary value, then 'e', the
           exponent's sign and the exponent -- '3.0e+0', '6.67e+1',
           '-0e+0'; 'NaN', 'Infinity' or '-Infinity' for those values.

           fractionDigits: an Integer from 0 to 20.

           Signals an ArgumentError carrying `fractionDigits`:
           'asStringExponential: takes 0 to 20 fraction digits, got …' for
           an Integer outside 0 to 20, a LargeInteger included, and
           'asStringExponential: requires an integer argument, got …' for
           one that is no Integer. *)
        public asStringExponential: fractionDigits = (
            ^vmMirror float: self asStringExponential: fractionDigits
                ifFail: [^super asStringExponential: fractionDigits]
        )
        (* Answers a new String of the receiver with `digits` significant
           digits, rounded half away from zero on its exact binary value:
           in decimal ('66.7', '0.0000010'), or in `asStringExponential:`'s
           form ('7e+1', '1.0e-7') when the value needs more than six zeros
           after the point or its digits end before the point; 'NaN',
           'Infinity' or '-Infinity' for those values.

           digits: an Integer from 1 to 21.

           Signals an ArgumentError carrying `digits`: 'asStringPrecision:
           takes 1 to 21 digits, got …' for an Integer outside 1 to 21, a
           LargeInteger included, and 'asStringPrecision: requires an
           integer argument, got …' for one that is no Integer. *)
        public asStringPrecision: digits = (
            ^vmMirror float: self asStringPrecision: digits
                ifFail: [^super asStringPrecision: digits]
        )
        (* Answers a new String of the shortest decimal text that reads
           back as the receiver: in decimal when its first digit's power of
           ten lies from -6 to 20, with '.0' after an integral value
           ('3.0', '0.000001', '100000000000000000000.0'), and in exponent
           form beyond ('1e+21', '1e-7'); 'NaN', 'Infinity' and
           '-Infinity' for those values. A negative value, -0.0 included,
           begins with '-'. *)
        public printString = ( ^vmMirror floatPrintString: self ifFail: [^super printString] )
        (* Answers what `printString` answers. *)
        public asString = ( ^vmMirror floatAsString: self ifFail: [^super asString] )
    ) : (
        (* Answers the Float `text` names: an optional '-' or '+', then
           'Infinity', 'NaN', or decimal digits with an optional point and
           an optional exponent ('e' or 'E', an optional sign, digits),
           with at least one digit before the exponent -- every text
           `printString` writes. The nearest Float to the decimal value;
           an infinity beyond the largest.

           text: a String.

           Signals an ArgumentError carrying `text`: 'parse: requires the
           text of a Float, got …' for a String that is no such text, a
           lower-case 'nan' or 'inf', a space or an empty String included,
           and 'parse: requires a String, got …' for anything else. *)
        public parse: text = ( ^vmMirror floatParse: text ifFail: [^super parse: text] )
    )
```

- [ ] **Step 6: Run the new goldens and the unit tests**

Run: `cargo nextest run -p ns-runtime float_text` and
`cargo nextest run -p ns-cli --test run_goldens a_float_`
Expected: PASS.

- [ ] **Step 7: Every changed float text**

Run: `cargo nextest run --workspace` and
`cargo run -p ns-cli --release -- run-corpus corpus/runnable.toml` (check
the command `CLAUDE.md` and the `finishing-a-batch` skill give for
`run-corpus`; use theirs). Every failure must be a float written in the
new text -- `3` that is now `3.0`, `-0` now `-0.0`, `inf` now `Infinity`,
a long integral float now in exponent form -- and nothing else. For each:
check by the rule above that the new text is right, update the expected
output, and if the golden's comment states the old text, rewrite that
sentence. `goldens/library/number/float/float_sine_cosine_and_truncation.toml`
is one (`0 -0 1` becomes `0.0 -0.0 1.0`, `got -inf` becomes `got
-Infinity`), `an_integer_meets_a_float.toml` another. List every changed
file in the commit message with one clause on why.

- [ ] **Step 8: The design document's sentences**

`docs/design/core-library.md`: wherever it states that a float prints
with Rust's `to_string`, as `inf`, or with no point for an integral
value, rewrite the sentence to the new text. Add a short section "A
Float's text" after "A NaN equals a NaN", saying what Steps 3-5 built and
why the layout is Primordial Soup's (the shortest digits, the decimal
range, `.0`, the names, half-up rounding), that `Float parse:` reads
every text `printString` writes, and that `Heap::describe` writes the
same text. Keep it to what holds now.

- [ ] **Step 9: Run the gates, then commit**

```bash
git add -A crates/ns-runtime goldens corpus docs/design/core-library.md
git commit -m "feat: a Float prints as its shortest text, writes exponential and precision texts, and is parsed"
```
(the body lists the goldens whose text changed; the two trailer lines).

---

### Task 3: `Float`'s mathematics, rounding and tests, and `Number`'s share of them

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (eighteen
  `FloatPrimitive` variants and rows)
- Modify: `crates/ns-runtime/src/builtins/numbers/float.rs`
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Number`, `Float`)
- Create: `goldens/library/number/float/float_mathematics_and_rounding.{ns,toml}`

**Interfaces:**
- Consumes: `numeric::float_text::shortest` (Task 2) through `printString`.
- Produces: on `Float`: `tan`, `asin`, `acos`, `atan`, `atan:`, `exp`,
  `ln`, `log`, `raisedTo:`, `floor`, `ceiling`, `rounded`, `truncated`,
  `negated`, `abs`, `asFloat`, `isNaN`, `isFinite`, `isInfinite`; on
  `Number`: `sqrt sin cos tan asin acos atan exp ln log` and `atan:` as
  `^self asFloat …`, `isNaN` (`false`), `isFinite` (`true`), `isInfinite`
  (`false`), `between:and:`, `min:`. Mirror rows `floatTangent:ifFail:`,
  `floatArcSine:ifFail:`, `floatArcCosine:ifFail:`,
  `floatArcTangent:ifFail:`, `float:arcTangent:ifFail:`,
  `floatExponential:ifFail:`, `floatNaturalLogarithm:ifFail:`,
  `floatLogarithm:ifFail:`, `float:raisedTo:ifFail:`,
  `floatFloor:ifFail:`, `floatCeiling:ifFail:`, `floatRounded:ifFail:`,
  `floatTruncated:ifFail:`, `floatNegated:ifFail:`,
  `floatAbsolute:ifFail:`, `floatIsNaN:ifFail:`, `floatIsFinite:ifFail:`,
  `floatIsInfinite:ifFail:`.

`floor`, `ceiling`, `rounded` and `truncated` answer a `Float`, as
Primordial Soup's do (spec, "The protocol"): `Infinity floor` is
`Infinity`, and `asInteger` asks for an integer. `rounded` rounds half
away from zero (`f64::round`). `atan:` is the angle of the point
(argument, receiver): `y atan: x` is `atan2(y, x)`. `raisedTo:` on a
`Float` is `powf` with the exponent converted to the nearest double.
Mathematical functions are the platform's: a golden prints only values
exact on every platform and bounds the others, as
`float_sine_cosine_and_truncation.ns` does.

- [ ] **Step 1: Write the failing golden**

`float_mathematics_and_rounding.ns`:

```newspeak
(* A Float's mathematical functions are the IEEE double's; only values
   exact on every platform are printed, the others are bounded. Rounding
   answers a Float -- `rounded` half away from zero -- and keeps an
   infinity or a NaN as it is. An integer answers the same functions
   through `asFloat`, and the questions about NaN and infinity as a
   finite number does. `atan:` is the angle of the point (argument,
   receiver). *)
class FloatMathematicsAndRounding = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. quarter natural common |
        transcript print: 0.0 tan; show: ' '; print: 0.0 asin; show: ' '; print: 1.0 acos; show: ' '; print: 0.0 atan; newLine.
        transcript print: (0.0 atan: 1.0); show: ' '; print: 0.0 exp; show: ' '; print: 1.0 ln; show: ' '; print: 1.0 log; newLine.
        quarter:: 1.0 atan: 1.0.
        natural:: 1.0 exp.
        common:: 100.0 log.
        transcript print: (quarter > 0.7853 and: [quarter < 0.7854]); show: ' '; print: (natural > 2.718 and: [natural < 2.719]); show: ' '; print: (common > 1.9999999 and: [common < 2.0000001]); newLine.
        transcript print: (2.0 raisedTo: 10); show: ' '; print: (4.0 raisedTo: 0.5); show: ' '; print: (2.0 raisedTo: -1); show: ' '; print: (2.0 raisedTo: 3); newLine.
        transcript print: 3.7 floor; show: ' '; print: -3.7 floor; show: ' '; print: 3.2 ceiling; show: ' '; print: -3.2 ceiling; newLine.
        transcript print: 2.5 rounded; show: ' '; print: -2.5 rounded; show: ' '; print: 2.4 rounded; show: ' '; print: 3.7 truncated; show: ' '; print: -3.7 truncated; newLine.
        transcript print: (1.0 / 0.0) floor; show: ' '; print: (0.0 / 0.0) rounded; show: ' '; print: -0.4 rounded; newLine.
        transcript print: 1.5 negated; show: ' '; print: 0.0 negated; show: ' '; print: -2.5 abs; show: ' '; print: -0.0 abs; show: ' '; print: 1.5 asFloat; newLine.
        transcript print: (0.0 / 0.0) isNaN; show: ' '; print: 1.5 isNaN; show: ' '; print: (1.0 / 0.0) isInfinite; show: ' '; print: (1.0 / 0.0) isFinite; show: ' '; print: 1.5 isFinite; newLine.
        transcript print: 3 isNaN; show: ' '; print: 3 isFinite; show: ' '; print: 3 isInfinite; show: ' '; print: 100000000000000000000 isFinite; newLine.
        transcript print: 4 sqrt; show: ' '; print: 0 sin; show: ' '; print: 1 ln; show: ' '; print: 0 exp; show: ' '; print: 100000000000000000000 sqrt; newLine.
        transcript print: (3 between: 1 and: 5); show: ' '; print: (3 between: 4 and: 5); show: ' '; print: (3 between: 3 and: 3); newLine.
        transcript print: (3 min: 5); show: ' '; print: (5 min: 3); show: ' '; print: (2.5 min: 3); show: ' '; print: (3 max: 2.5); newLine.
        transcript show: ([1.5 atan: nil] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([2.0 raisedTo: 'x'] on: Error do: [:error | error messageText]); newLine
    )
)
```

`float_mathematics_and_rounding.toml`:

```toml
stdout = '''
0.0 0.0 0.0 0.0
0.0 1.0 0.0 0.0
true true true
1024.0 2.0 0.5 8.0
3.0 -4.0 4.0 -3.0
3.0 -3.0 2.0 3.0 -3.0
Infinity NaN -0.0
-1.5 -0.0 2.5 0.0 1.5
true false true false true
false true false true
2.0 0.0 0.0 1.0 10000000000.0
true false true
3 3 2.5 3
atan: requires a numeric argument, got nil
raisedTo: requires a numeric argument, got the String "x"
'''
```

(`-0.4 rounded` is `-0.0`: `f64::round` keeps the sign of a value that
rounds to zero, as C's `round` does.)

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-cli --test run_goldens float_mathematics_and_rounding`
Expected: FAIL -- `tan` is not understood.

- [ ] **Step 3: The leaves and their rows**

`primitive.rs`: add to `FloatPrimitive`, each `false` in
`answers_false_to_a_stranger`: `Tangent`, `ArcSine`, `ArcCosine`,
`ArcTangent`, `ArcTangentOf`, `Exponential`, `NaturalLogarithm`,
`Logarithm`, `RaisedTo`, `Floor`, `Ceiling`, `Rounded`, `Truncated`,
`Negated`, `Absolute`, `IsNaN`, `IsFinite`, `IsInfinite`. Rows after
`floatCosine:ifFail:`, one per variant, in this shape:

```rust
    mirror(
        "floatTangent:ifFail:",
        "tan",
        Primitive::Float(FloatPrimitive::Tangent),
    ),
    mirror(
        "float:arcTangent:ifFail:",
        "atan:",
        Primitive::Float(FloatPrimitive::ArcTangentOf),
    ),
```

The selectors and reported names: `floatTangent:ifFail:` `tan`,
`floatArcSine:ifFail:` `asin`, `floatArcCosine:ifFail:` `acos`,
`floatArcTangent:ifFail:` `atan`, `float:arcTangent:ifFail:` `atan:`,
`floatExponential:ifFail:` `exp`, `floatNaturalLogarithm:ifFail:` `ln`,
`floatLogarithm:ifFail:` `log`, `float:raisedTo:ifFail:` `raisedTo:`,
`floatFloor:ifFail:` `floor`, `floatCeiling:ifFail:` `ceiling`,
`floatRounded:ifFail:` `rounded`, `floatTruncated:ifFail:` `truncated`,
`floatNegated:ifFail:` `negated`, `floatAbsolute:ifFail:` `abs`,
`floatIsNaN:ifFail:` `isNaN`, `floatIsFinite:ifFail:` `isFinite`,
`floatIsInfinite:ifFail:` `isInfinite`.

`numbers/float.rs`, in `execute_float`'s match:

```rust
        FloatPrimitive::Tangent => Some(Ok(mutator.new_float(receiver_float.tan()))),
        FloatPrimitive::ArcSine => Some(Ok(mutator.new_float(receiver_float.asin()))),
        FloatPrimitive::ArcCosine => Some(Ok(mutator.new_float(receiver_float.acos()))),
        FloatPrimitive::ArcTangent => Some(Ok(mutator.new_float(receiver_float.atan()))),
        // `y atan: x`, the angle of the point (x, y).
        FloatPrimitive::ArcTangentOf => {
            Some(arithmetic("atan:", f64::atan2).map(|float| mutator.new_float(float)))
        }
        FloatPrimitive::Exponential => Some(Ok(mutator.new_float(receiver_float.exp()))),
        FloatPrimitive::NaturalLogarithm => Some(Ok(mutator.new_float(receiver_float.ln()))),
        FloatPrimitive::Logarithm => Some(Ok(mutator.new_float(receiver_float.log10()))),
        FloatPrimitive::RaisedTo => {
            Some(arithmetic("raisedTo:", f64::powf).map(|float| mutator.new_float(float)))
        }
        FloatPrimitive::Floor => Some(Ok(mutator.new_float(receiver_float.floor()))),
        FloatPrimitive::Ceiling => Some(Ok(mutator.new_float(receiver_float.ceil()))),
        // Half away from zero, as C's `round`.
        FloatPrimitive::Rounded => Some(Ok(mutator.new_float(receiver_float.round()))),
        FloatPrimitive::Truncated => Some(Ok(mutator.new_float(receiver_float.trunc()))),
        // The sign bit flipped, so `0.0 negated` is `-0.0`, which `0 - self`
        // would not answer.
        FloatPrimitive::Negated => Some(Ok(mutator.new_float(-receiver_float))),
        FloatPrimitive::Absolute => Some(Ok(mutator.new_float(receiver_float.abs()))),
        FloatPrimitive::IsNaN => Some(Ok(Value::boolean(receiver_float.is_nan()))),
        FloatPrimitive::IsFinite => Some(Ok(Value::boolean(receiver_float.is_finite()))),
        FloatPrimitive::IsInfinite => Some(Ok(Value::boolean(receiver_float.is_infinite()))),
```

(`arithmetic` is `execute_float`'s existing closure over `argument_float`,
which takes an `Integer`, a `LargeInteger` or a `Float` and is the type
mismatch for anything else; its texts name the selector passed.)

- [ ] **Step 4: `Float` and `Number` in `Kernel.ns`**

In `Float`, after `cos`:

```newspeak
        (* Answers the tangent of the receiver, in radians, a Float. *)
        public tan = ( ^vmMirror floatTangent: self ifFail: [^super tan] )
        (* Answers the arc sine of the receiver, in radians from -π/2 to
           π/2, a Float; a NaN outside -1 to 1. *)
        public asin = ( ^vmMirror floatArcSine: self ifFail: [^super asin] )
        (* Answers the arc cosine of the receiver, in radians from 0 to π, a
           Float; a NaN outside -1 to 1. *)
        public acos = ( ^vmMirror floatArcCosine: self ifFail: [^super acos] )
        (* Answers the arc tangent of the receiver, in radians from -π/2 to
           π/2, a Float. *)
        public atan = ( ^vmMirror floatArcTangent: self ifFail: [^super atan] )
        (* Answers the angle, in radians from -π to π, of the point whose
           abscissa is `x` and whose ordinate is the receiver, a Float.

           x: a number, converted to the nearest Float first.

           Signals an ArgumentError carrying `x`, 'atan: requires a numeric
           argument, got …', for any other argument. *)
        public atan: x = ( ^vmMirror float: self arcTangent: x ifFail: [^super atan: x] )
        (* Answers e raised to the receiver, a Float. *)
        public exp = ( ^vmMirror floatExponential: self ifFail: [^super exp] )
        (* Answers the natural logarithm of the receiver, a Float: minus
           infinity for zero, a NaN below zero. *)
        public ln = ( ^vmMirror floatNaturalLogarithm: self ifFail: [^super ln] )
        (* Answers the logarithm of the receiver to base 10, a Float: minus
           infinity for zero, a NaN below zero. *)
        public log = ( ^vmMirror floatLogarithm: self ifFail: [^super log] )
        (* Answers the receiver raised to `exponent` as IEEE 754's `pow`
           computes it, a Float.

           exponent: a number, converted to the nearest Float first.

           Signals an ArgumentError carrying `exponent`, 'raisedTo:
           requires a numeric argument, got …', for any other argument. *)
        public raisedTo: exponent = (
            ^vmMirror float: self raisedTo: exponent ifFail: [^super raisedTo: exponent]
        )
        (* Answers the greatest integral value not above the receiver, a
           Float; an infinity or a NaN is answered as it is. *)
        public floor = ( ^vmMirror floatFloor: self ifFail: [^super floor] )
        (* Answers the least integral value not below the receiver, a
           Float; an infinity or a NaN is answered as it is. *)
        public ceiling = ( ^vmMirror floatCeiling: self ifFail: [^super ceiling] )
        (* Answers the integral value nearest the receiver, a half rounded
           away from zero, a Float that keeps the receiver's sign; an
           infinity or a NaN is answered as it is. *)
        public rounded = ( ^vmMirror floatRounded: self ifFail: [^super rounded] )
        (* Answers the receiver without its fraction, toward zero, a Float
           that keeps the receiver's sign; an infinity or a NaN is answered
           as it is. *)
        public truncated = ( ^vmMirror floatTruncated: self ifFail: [^super truncated] )
        (* Answers the receiver with its sign changed, a Float: `0.0
           negated` is -0.0. *)
        public negated = ( ^vmMirror floatNegated: self ifFail: [^super negated] )
        (* Answers the receiver without its sign, a Float: -0.0 answers
           0.0. *)
        public abs = ( ^vmMirror floatAbsolute: self ifFail: [^super abs] )
        (* Answers the receiver itself. *)
        public asFloat = ( ^self )
        (* Answers whether the receiver is a NaN. *)
        public isNaN = ( ^vmMirror floatIsNaN: self ifFail: [^super isNaN] )
        (* Answers whether the receiver is neither an infinity nor a NaN. *)
        public isFinite = ( ^vmMirror floatIsFinite: self ifFail: [^super isFinite] )
        (* Answers whether the receiver is an infinity, of either sign. *)
        public isInfinite = ( ^vmMirror floatIsInfinite: self ifFail: [^super isInfinite] )
```

In `Number`, after `max:` (`Number`'s class comment becomes "The
superclass of `Integer` and `Float`: counting loops, comparing, the
mathematical functions through `asFloat`, and the questions about NaN and
infinity."):

```newspeak
        (* Answers the lesser of the receiver and `other`: the receiver
           when it is `<` `other`, `other` otherwise.

           other: a number.

           Signals the ArgumentError the receiver's `<` signals for an
           `other` that is no number. *)
        public min: other = ( ^self < other ifTrue: [self] ifFalse: [other] )
        (* Answers whether the receiver is at least `min` and at most
           `max`, a Boolean.

           min: a number.
           max: a number.

           Signals the ArgumentError the receiver's `>=` or `<=` signals for
           a bound that is no number. *)
        public between: min and: max = ( ^self >= min and: [self <= max] )
        (* Answers false: only a Float can be a NaN. *)
        public isNaN = ( ^false )
        (* Answers true: only a Float can be infinite or a NaN. *)
        public isFinite = ( ^true )
        (* Answers false: only a Float can be infinite. *)
        public isInfinite = ( ^false )
        (* Answers the square root of the receiver as a Float, computed on
           `asFloat`. *)
        public sqrt = ( ^self asFloat sqrt )
        (* Answers the sine of the receiver in radians as a Float, computed
           on `asFloat`. *)
        public sin = ( ^self asFloat sin )
        (* Answers the cosine of the receiver in radians as a Float,
           computed on `asFloat`. *)
        public cos = ( ^self asFloat cos )
        (* Answers the tangent of the receiver in radians as a Float,
           computed on `asFloat`. *)
        public tan = ( ^self asFloat tan )
        (* Answers the arc sine of the receiver as a Float, computed on
           `asFloat`. *)
        public asin = ( ^self asFloat asin )
        (* Answers the arc cosine of the receiver as a Float, computed on
           `asFloat`. *)
        public acos = ( ^self asFloat acos )
        (* Answers the arc tangent of the receiver as a Float, computed on
           `asFloat`. *)
        public atan = ( ^self asFloat atan )
        (* Answers `Float>>atan:` of the receiver's `asFloat`.

           x: a number.

           Signals `Float>>atan:`'s ArgumentError for any other argument. *)
        public atan: x = ( ^self asFloat atan: x )
        (* Answers e raised to the receiver as a Float, computed on
           `asFloat`. *)
        public exp = ( ^self asFloat exp )
        (* Answers the natural logarithm of the receiver as a Float,
           computed on `asFloat`. *)
        public ln = ( ^self asFloat ln )
        (* Answers the logarithm to base 10 of the receiver as a Float,
           computed on `asFloat`. *)
        public log = ( ^self asFloat log )
```

(`Number>>asFloat` comes with the tower in Task 8; until then the
integers' own `asFloat` answers, and those are the only numbers there
are.)

- [ ] **Step 5: Run the golden, then the gates**

Run: `cargo nextest run -p ns-cli --test run_goldens float_mathematics_and_rounding`
Expected: PASS. Then all five gates.

- [ ] **Step 6: Commit**

```bash
git add crates/ns-runtime goldens/library/number/float
git commit -m "feat: Float's mathematics, rounding and tests, and Number's share of them"
```
(with the two trailer lines).

---

### Task 4: `Integer`'s protocol: gcd, shifts, digits, radix texts and parsing, modular arithmetic

**Files:**
- Modify: `crates/ns-runtime/src/arbitrary_integer/mod.rs` (`one`,
  `from_u64`, `to_u64`, `is_one`, `bit_length`, `gcd`,
  `truncated_divide`, `shift_right`, `base_256_digit`,
  `base_256_length`, `to_radix_string`)
- Modify: `crates/ns-runtime/src/arbitrary_integer/conversion.rs` (the
  radix digits beside `to_decimal`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (six
  `IntegerPrimitive` variants and rows)
- Modify: `crates/ns-runtime/src/builtins/numbers/integer.rs`
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Integer`, its new
  class side, `LargeInteger`'s class comment)
- Create: `goldens/library/number/integer/integer_protocol.{ns,toml}`
- Create: `goldens/library/number/integer/integer_protocol_errors.{ns,toml}`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces, on `ArbitraryInteger` (all `pub`):
  `one() -> ArbitraryInteger`, `from_u64(u64) -> ArbitraryInteger`,
  `to_u64(&self) -> Option<u64>` (`None` for a negative value or one past
  `u64`), `is_one(&self) -> bool`, `bit_length(&self) -> u64` (of the
  magnitude; 0 for zero), `gcd(&self, &ArbitraryInteger) ->
  ArbitraryInteger` (non-negative; `0 gcd 0` is 0),
  `truncated_divide(&self, &ArbitraryInteger) -> Option<(ArbitraryInteger,
  ArbitraryInteger)>` (quotient toward zero and the remainder with the
  dividend's sign; `None` for a zero divisor), `shift_right(&self, u64) ->
  ArbitraryInteger` (arithmetic: toward negative infinity),
  `base_256_length(&self) -> usize`, `base_256_digit(&self, usize) ->
  Option<u8>` (the magnitude's digit at a zero-based index),
  `to_radix_string(&self, u32) -> String` (2 to 36, upper case, a `-`
  for a negative value). On `Integer`: `gcd:`, `>>`, `&`, `|`,
  `digitAt:`, `digitLength`, `asStringRadix:`, `floor`, `ceiling`,
  `rounded`, `truncated`, `asInteger`, `numerator`, `denominator`,
  `isKindOfFraction`, `raisedTo:modulo:`, `reciprocalModulo:`; class
  side `parse:` and `parse:radix:`. Mirror rows `integer:gcd:ifFail:`,
  `integer:shiftRight:ifFail:`, `integer:digitAt:ifFail:`,
  `integerDigitLength:ifFail:`, `integer:asStringRadix:ifFail:`,
  `integerParse:radix:ifFail:`.

The new leaves take an integer receiver of either representation, so
`LargeInteger` inherits `Integer`'s methods for them rather than
declaring its own. `digitAt:` and `digitLength` read the magnitude's
base-256 digits, least significant first, from 1: `-1 digitAt: 1` is 1,
and zero has no digit (`0 digitLength` is 0). `quo:` and `rem:` come in
Task 8, over every number.

- [ ] **Step 1: Write the failing goldens**

`integer_protocol.ns`:

```newspeak
(* An integer's own protocol: the greatest common divisor, never
   negative; an arithmetic right shift, toward negative infinity; `&` and
   `|` as `bitAnd:` and `bitOr:`; the base-256 digits of its magnitude,
   least significant first; its text in any radix from 2 to 36, upper
   case, and `Integer parse:radix:` back; the rounding selectors and
   `numerator`/`denominator` of a fraction whose denominator is 1; and
   modular exponentiation and inverse. A LargeInteger answers the same
   way. *)
class IntegerProtocol = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. big = 1180591620717411303424. |
        transcript print: (12 gcd: 18); show: ' '; print: (-12 gcd: 18); show: ' '; print: (12 gcd: -18); show: ' '; print: (0 gcd: 5); show: ' '; print: (0 gcd: 0); newLine.
        transcript print: (big gcd: 110680464442257309696); show: ' '; print: (big gcd: 3); newLine.
        transcript print: -8 >> 1; show: ' '; print: 5 >> 1; show: ' '; print: -1 >> 100; show: ' '; print: big >> 68; show: ' '; print: 5 >> 0; show: ' '; print: big negated >> 200; newLine.
        transcript print: (12 & 10); show: ' '; print: (12 | 3); show: ' '; print: (big | 1); newLine.
        transcript print: (255 digitAt: 1); show: ' '; print: (256 digitAt: 2); show: ' '; print: (256 digitAt: 1); show: ' '; print: (-1 digitAt: 1); newLine.
        transcript print: 256 digitLength; show: ' '; print: 0 digitLength; show: ' '; print: 18446744073709551616 digitLength; show: ' '; print: (18446744073709551616 digitAt: 9); newLine.
        transcript show: (255 asStringRadix: 16); show: ' '; show: (-255 asStringRadix: 2); show: ' '; show: (18446744073709551616 asStringRadix: 16); show: ' '; show: (35 asStringRadix: 36); show: ' '; show: (0 asStringRadix: 2); newLine.
        transcript print: (Integer parse: '42'); show: ' '; print: (Integer parse: '-0123'); show: ' '; print: (Integer parse: 'CAFE' radix: 16); show: ' '; print: (Integer parse: 'cafe' radix: 16); show: ' '; print: (Integer parse: 'z' radix: 36); newLine.
        transcript print: (Integer parse: '18446744073709551616'); show: ' '; print: (Integer parse: '18446744073709551616') class; newLine.
        transcript print: 7 floor; show: ' '; print: 7 ceiling; show: ' '; print: 7 rounded; show: ' '; print: 7 truncated; show: ' '; print: 7 asInteger; show: ' '; print: big floor = big; newLine.
        transcript print: 7 numerator; show: ' '; print: 7 denominator; show: ' '; print: 7 isKindOfFraction; show: ' '; print: big isKindOfFraction; newLine.
        transcript print: (4 raisedTo: 13 modulo: 497); show: ' '; print: (3 reciprocalModulo: 11); show: ' '; print: (3 raisedTo: -1 modulo: 11); show: ' '; print: (2 raisedTo: 0 modulo: 7); show: ' '; print: (big raisedTo: 2 modulo: 1000); newLine
    )
)
```

`integer_protocol.toml` (`big` is 2^70; `110680464442257309696` is
3 × 2^65; `big negated >> 200` is -1; `big | 1` is 2^70 + 1; 2^70 mod
1000 is 424, so its square mod 1000 is 776):

```toml
stdout = '''
6 6 6 5 0
36893488147419103232 1
-4 2 -1 4 5 -1
8 15 1180591620717411303425
255 1 0 1
2 0 9 1
FF -11111111 10000000000000000 Z 0
42 -123 51966 51966 35
18446744073709551616 LargeInteger
7 7 7 7 7 true
7 1 true true
445 4 4 1 776
'''
```

`integer_protocol_errors.ns`:

```newspeak
(* What an integer's own protocol refuses, each an ArgumentError carrying
   the argument: an index past the digits, a radix outside 2 to 36, a
   text that is no integer in its radix, a negative shift, an argument
   that is no integer, and an inverse that does not exist. *)
class IntegerProtocolErrors = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. Error = platform kernel Error. ArgumentError = platform kernel ArgumentError. |
        transcript show: ([256 digitAt: 3] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([256 digitAt: 0] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([255 asStringRadix: 37] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Integer parse: '12a'] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Integer parse: ''] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Integer parse: '-'] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Integer parse: '10' radix: 1] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Integer parse: 10] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([5 >> -1] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([5 gcd: 1.5] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([5 & nil] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([2 reciprocalModulo: 4] on: Error do: [:error | error messageText]); newLine.
        transcript print: ([2 reciprocalModulo: 4] on: ArgumentError do: [:error | error argument]); newLine
    )
)
```

`integer_protocol_errors.toml`:

```toml
stdout = '''
digitAt: index 3 out of range: valid indices are 1..2 (digitLength 2)
digitAt: index 0 out of range: valid indices are 1..2 (digitLength 2)
asStringRadix: takes a radix from 2 to 36, got 37
parse:radix: requires the digits of an Integer in radix 10, got the String "12a"
parse:radix: requires the digits of an Integer in radix 10, got the String ""
parse:radix: requires the digits of an Integer in radix 10, got the String "-"
parse:radix: takes a radix from 2 to 36, got 1
parse:radix: requires a String, got 10
>> takes a non-negative shift count, got -1
gcd: requires an integer argument, got 1.5
bitAnd: requires an integer argument, got nil
reciprocalModulo: 2 has no inverse modulo 4
4
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens integer_protocol`
Expected: FAIL -- `gcd:` is not understood.

- [ ] **Step 3: `ArbitraryInteger`'s new operations, test first**

Add to `arbitrary_integer`'s unit tests (where its existing tests live):

```rust
    #[test]
    fn gcd_is_non_negative_and_euclids() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        assert_eq!(integer(12).gcd(&integer(18)), integer(6));
        assert_eq!(integer(-12).gcd(&integer(18)), integer(6));
        assert_eq!(integer(0).gcd(&integer(-5)), integer(5));
        assert_eq!(integer(0).gcd(&integer(0)), integer(0));
        let power = ArbitraryInteger::one().shift_left(130).expect("small");
        let multiple = power.multiply(&integer(3));
        assert_eq!(power.gcd(&multiple), power);
    }

    #[test]
    fn truncated_division_rounds_toward_zero() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        let divide = |dividend: i128, divisor: i128| {
            integer(dividend)
                .truncated_divide(&integer(divisor))
                .expect("non-zero divisor")
        };
        assert_eq!(divide(7, 2), (integer(3), integer(1)));
        assert_eq!(divide(-7, 2), (integer(-3), integer(-1)));
        assert_eq!(divide(7, -2), (integer(-3), integer(1)));
        assert_eq!(divide(-7, -2), (integer(3), integer(-1)));
        assert!(integer(1).truncated_divide(&integer(0)).is_none());
    }

    #[test]
    fn shift_right_rounds_toward_negative_infinity() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        assert_eq!(integer(-8).shift_right(1), integer(-4));
        assert_eq!(integer(-7).shift_right(1), integer(-4));
        assert_eq!(integer(7).shift_right(1), integer(3));
        assert_eq!(integer(-1).shift_right(1000), integer(-1));
        assert_eq!(integer(1 << 100).shift_right(98), integer(4));
        assert_eq!(integer(-(1 << 100)).shift_right(200), integer(-1));
        assert_eq!(integer(5).shift_right(u64::MAX), integer(0));
    }

    #[test]
    fn digits_bit_length_and_radix_text() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        assert_eq!(integer(0).bit_length(), 0);
        assert_eq!(integer(-255).bit_length(), 8);
        assert_eq!(integer(1 << 64).bit_length(), 65);
        assert_eq!(integer(256).base_256_length(), 2);
        assert_eq!(integer(256).base_256_digit(1), Some(1));
        assert_eq!(integer(256).base_256_digit(2), None);
        assert_eq!(integer(0).base_256_length(), 0);
        assert_eq!(integer(-255).to_radix_string(16), "-FF");
        assert_eq!(integer(0).to_radix_string(2), "0");
        assert_eq!(integer(1 << 64).to_radix_string(16), "10000000000000000");
        assert_eq!(
            integer(-(1 << 100)).to_radix_string(10),
            integer(-(1 << 100)).to_decimal_string()
        );
        assert_eq!(integer(35).to_radix_string(36), "Z");
        assert_eq!(integer(5).to_u64(), Some(5));
        assert_eq!(integer(-5).to_u64(), None);
        assert!(ArbitraryInteger::one().is_one());
    }
```

Run: `cargo nextest run -p ns-runtime arbitrary_integer` -- FAIL (no such
methods). Then implement, in `impl ArbitraryInteger`:

```rust
    pub fn one() -> ArbitraryInteger {
        ArbitraryInteger::from_u64(1)
    }

    pub fn from_u64(value: u64) -> ArbitraryInteger {
        ArbitraryInteger::from_parts(false, vec![value])
    }

    /// The value as a `u64`; `None` for a negative value or one past it.
    pub fn to_u64(&self) -> Option<u64> {
        match (self.negative, &*self.magnitude) {
            (_, []) => Some(0),
            (false, [digit]) => Some(*digit),
            _ => None,
        }
    }

    pub fn is_one(&self) -> bool {
        !self.negative && *self.magnitude == [1]
    }

    /// The magnitude's bit count: 0 for zero, n for 2^(n-1) up to 2^n - 1.
    pub fn bit_length(&self) -> u64 {
        match self.magnitude.last() {
            None => 0,
            Some(top) => {
                (self.magnitude.len() as u64 - 1) * u64::from(u64::BITS)
                    + u64::from(u64::BITS - top.leading_zeros())
            }
        }
    }

    /// The greatest common divisor, never negative; `0 gcd 0` is 0.
    /// Euclid's algorithm on the magnitudes, a one-digit pair in `u64`.
    pub fn gcd(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        let mut larger = self.magnitude.to_vec();
        let mut smaller = other.magnitude.to_vec();
        if compare_magnitudes(&larger, &smaller) == Ordering::Less {
            std::mem::swap(&mut larger, &mut smaller);
        }
        while !smaller.is_empty() {
            if let ([first], [second]) = (larger.as_slice(), smaller.as_slice()) {
                let (mut first, mut second) = (*first, *second);
                while second != 0 {
                    (first, second) = (second, first % second);
                }
                return ArbitraryInteger::from_u64(first);
            }
            let (_, remainder) = divide_remainder(&larger, &smaller);
            larger = std::mem::replace(&mut smaller, remainder);
        }
        ArbitraryInteger::from_parts(false, larger)
    }

    /// The quotient rounded toward zero and the remainder, which takes the
    /// dividend's sign; `None` for a zero divisor.
    pub fn truncated_divide(
        &self,
        divisor: &ArbitraryInteger,
    ) -> Option<(ArbitraryInteger, ArbitraryInteger)> {
        if divisor.is_zero() {
            return None;
        }
        let (quotient, remainder) = divide_remainder(&self.magnitude, &divisor.magnitude);
        Some((
            ArbitraryInteger::from_parts(self.negative != divisor.negative, quotient),
            ArbitraryInteger::from_parts(self.negative, remainder),
        ))
    }

    /// The value divided by two to the `count`, rounded toward negative
    /// infinity: an arithmetic shift of the two's complement, so a
    /// negative value never shifts past -1.
    pub fn shift_right(&self, count: u64) -> ArbitraryInteger {
        let shifted_magnitude = |magnitude: &[u64]| {
            let digit_shift =
                usize::try_from(count / u64::from(u64::BITS)).unwrap_or(usize::MAX);
            if digit_shift >= magnitude.len() {
                return Vec::new();
            }
            let mut digits = magnitude[digit_shift..].to_vec();
            shift_right_in_place(&mut digits, (count % u64::from(u64::BITS)) as u32);
            digits
        };
        if !self.negative {
            return ArbitraryInteger::from_parts(false, shifted_magnitude(&self.magnitude));
        }
        // -(((|n| - 1) >> count) + 1), the floor of n / 2^count.
        let less_one = subtract_magnitudes(&self.magnitude, &[1]);
        let shifted = shifted_magnitude(&less_one);
        ArbitraryInteger::from_parts(true, add_magnitudes(&shifted, &[1]))
    }

    /// How many base-256 digits the magnitude has; 0 for zero.
    pub fn base_256_length(&self) -> usize {
        self.bit_length().div_ceil(8) as usize
    }

    /// The magnitude's base-256 digit at zero-based `index`, least
    /// significant first; `None` past the last.
    pub fn base_256_digit(&self, index: usize) -> Option<u8> {
        if index >= self.base_256_length() {
            return None;
        }
        let digit = self.magnitude[index / 8];
        Some((digit >> ((index % 8) * 8)) as u8)
    }

    /// The text in `radix` (2 to 36), upper-case digits after a `-` for a
    /// negative value.
    pub fn to_radix_string(&self, radix: u32) -> String {
        assert!((2..=36).contains(&radix), "a radix from 2 to 36");
        let digits = if radix == 10 {
            conversion::to_decimal(&self.magnitude)
        } else {
            conversion::to_radix(&self.magnitude, radix)
        };
        if self.negative {
            format!("-{digits}")
        } else {
            digits
        }
    }
```

(Check that `shift_right_in_place` accepts a shift of 0, that
`subtract_magnitudes`/`add_magnitudes` take canonical slices and answer a
`Vec<u64>` as their uses above assume, and that `from_parts` trims a zero
top digit; adapt to their real signatures. `conversion::to_decimal` is
`pub(super)`, reachable from `mod.rs`.)

`conversion.rs`, beside `to_decimal`:

```rust
/// The magnitude's digits in `radix` (2 to 36, not 10), upper case, most
/// significant first; "0" for zero. Divides by the largest power of the
/// radix a digit holds, then writes each remainder's digits.
pub(super) fn to_radix(magnitude: &[u64], radix: u32) -> String {
    if magnitude.is_empty() {
        return "0".to_string();
    }
    let base = u64::from(radix);
    let mut chunk_digits = 0;
    let mut chunk = 1u64;
    while let Some(next) = chunk.checked_mul(base) {
        chunk = next;
        chunk_digits += 1;
    }
    let mut remaining = magnitude.to_vec();
    let mut chunks = Vec::new();
    while !remaining.is_empty() {
        let (quotient, remainder) = super::division::divide_by_digit(&remaining, chunk);
        chunks.push(remainder);
        remaining = quotient;
        while remaining.last() == Some(&0) {
            remaining.pop();
        }
    }
    let digit = |value: u64| {
        char::from_digit(value as u32, radix)
            .expect("a digit below the radix")
            .to_ascii_uppercase()
    };
    let mut text = String::new();
    for (position, mut value) in chunks.into_iter().rev().enumerate() {
        let mut written = Vec::with_capacity(chunk_digits);
        while value > 0 {
            written.push(digit(value % base));
            value /= base;
        }
        if position > 0 {
            written.resize(chunk_digits, '0');
        }
        text.extend(written.iter().rev());
    }
    text
}
```

(If `to_decimal` is the same chunked loop, factor the shared part
instead of keeping two copies.)

Run: `cargo nextest run -p ns-runtime arbitrary_integer` -- PASS.

- [ ] **Step 4: The leaves and their rows**

`primitive.rs`: add to `IntegerPrimitive`, each `false` in
`answers_false_to_a_stranger`: `Gcd`, `ShiftRight`, `DigitAt`,
`DigitLength`, `AsStringRadix`, `Parse`. Rows after
`integer:shiftLeft:ifFail:`:

| mirror selector | reported |
|---|---|
| `integer:gcd:ifFail:` | `gcd:` |
| `integer:shiftRight:ifFail:` | `>>` |
| `integer:digitAt:ifFail:` | `digitAt:` |
| `integerDigitLength:ifFail:` | `digitLength` |
| `integer:asStringRadix:ifFail:` | `asStringRadix:` |
| `integerParse:radix:ifFail:` | `parse:radix:` |

`numbers/integer.rs`: `execute_integer` answers the six before it reads
the receiver as a `SmallInteger`:

```rust
    if let Some(answer) = integer_protocol(mutator, primitive, receiver, arguments) {
        return answer;
    }
    let receiver_integer = receiver.as_small_integer()?;
```

```rust
/// The leaves that take an integer receiver of either representation, so
/// that `LargeInteger` inherits `Integer`'s methods for them. `None` for
/// every other primitive, which `execute_integer` goes on to; `Some(None)`
/// declines a receiver that is no integer.
fn integer_protocol<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: IntegerPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Option<Result<Value<'gc>, NewspeakError<'gc>>>> {
    let answer = match primitive {
        IntegerPrimitive::Parse => parse(mutator, receiver, arguments),
        IntegerPrimitive::Gcd
        | IntegerPrimitive::ShiftRight
        | IntegerPrimitive::DigitAt
        | IntegerPrimitive::DigitLength
        | IntegerPrimitive::AsStringRadix => {
            let Some(integer) = integer_operand(&mutator.heap, Some(receiver)) else {
                return Some(None);
            };
            match primitive {
                IntegerPrimitive::Gcd => gcd(mutator, receiver, &integer, arguments),
                IntegerPrimitive::ShiftRight => shift_right(mutator, receiver, &integer, arguments),
                IntegerPrimitive::DigitAt => digit_at(mutator, receiver, &integer, arguments),
                IntegerPrimitive::DigitLength => {
                    Ok(Value::small_integer(integer.base_256_length() as i64))
                }
                _ => radix_text(mutator, receiver, &integer, arguments),
            }
        }
        _ => return None,
    };
    Some(Some(answer))
}
```

Each helper takes the argument through `integer_operand`; an argument
that is no integer is `type_mismatch(…, Accepted::Integer, …)` naming the
reported selector. They answer:

- `gcd`: `mutator.new_integer(integer.gcd(&argument))`.
- `shift_right`: a negative count is an `ArgumentError` carrying it,
  `">> takes a non-negative shift count, got {count}"` (the count by
  `to_bounded_text`); a count past `u64` is `shift_right(u64::MAX)`;
  otherwise `new_integer(integer.shift_right(count))`.
- `digit_at`: an index from 1 to `base_256_length` answers
  `Value::small_integer` of `base_256_digit(index - 1)`; any other
  Integer, a `LargeInteger` included, is an `ArgumentError` carrying it,
  `"digitAt: index {index} out of range: valid indices are 1..{length}
  (digitLength {length})"`.
- `radix_text`: a radix from 2 to 36 answers
  `new_string(integer.to_radix_string(radix))`; any other Integer is an
  `ArgumentError` carrying it, `"asStringRadix: takes a radix from 2 to
  36, got {radix}"`.
- `parse` (sent to the class; the arguments are the text and the radix):
  the radix checked as `radix_text`'s, its text naming `parse:radix:`;
  a text that is no String (a Symbol refused too) an `ArgumentError`
  carrying it, `"parse:radix: requires a String, got {describe}"`; then
  `ArbitraryInteger::parse(text, radix)`, whose `None` is an
  `ArgumentError` carrying the text, `"parse:radix: requires the digits
  of an Integer in radix {radix}, got {describe}"`; the answer through
  `new_integer`.

- [ ] **Step 5: `Integer` in `Kernel.ns`**

After `<<` in `Integer`:

```newspeak
        (* Answers the receiver divided by 2 to the power `count`, rounded
           toward negative infinity: the two's-complement shift, so a
           negative receiver shifts to -1 at the least.

           count: an Integer or a LargeInteger, at least 0.

           Signals an ArgumentError carrying `count`: '>> requires an
           integer argument, got …' for an argument that is no integer and
           '>> takes a non-negative shift count, got …' for a negative
           one. *)
        public >> count = ( ^vmMirror integer: self shiftRight: count ifFail: [^super >> count] )
        (* As `bitAnd:`, whose ArgumentError's text names 'bitAnd:'. *)
        public & other = ( ^self bitAnd: other )
        (* As `bitOr:`, whose ArgumentError's text names 'bitOr:'. *)
        public | other = ( ^self bitOr: other )
        (* Answers the greatest common divisor of the receiver and `other`,
           an integer never negative; 0 when both are 0.

           other: an Integer or a LargeInteger.

           Signals an ArgumentError carrying `other`, 'gcd: requires an
           integer argument, got …', for any other argument. *)
        public gcd: other = ( ^vmMirror integer: self gcd: other ifFail: [^super gcd: other] )
        (* Answers the base-256 digit at `index` of the receiver's
           magnitude, least significant first, an Integer from 0 to 255:
           the sign is not a digit, so `-1 digitAt: 1` is 1.

           index: an Integer from 1 to `digitLength`.

           Signals an ArgumentError carrying `index`: 'digitAt: index …
           out of range: valid indices are 1..… (digitLength …)' for an
           Integer outside them, a LargeInteger included, and 'digitAt:
           requires an integer argument, got …' for one that is no
           integer. *)
        public digitAt: index = (
            ^vmMirror integer: self digitAt: index ifFail: [^super digitAt: index]
        )
        (* Answers how many base-256 digits the receiver's magnitude has,
           an Integer: 0 for 0. *)
        public digitLength = ( ^vmMirror integerDigitLength: self ifFail: [^super digitLength] )
        (* Answers a new String of the receiver in `radix`: its digits from
           '0' to '9' and 'A' to 'Z', most significant first, after a '-'
           for a negative receiver.

           radix: an Integer from 2 to 36.

           Signals an ArgumentError carrying `radix`: 'asStringRadix: takes
           a radix from 2 to 36, got …' for an Integer outside them, a
           LargeInteger included, and 'asStringRadix: requires an integer
           argument, got …' for one that is no integer. *)
        public asStringRadix: radix = (
            ^vmMirror integer: self asStringRadix: radix ifFail: [^super asStringRadix: radix]
        )
        (* Answers the receiver itself: an integer is its own floor. *)
        public floor = ( ^self )
        (* Answers the receiver itself: an integer is its own ceiling. *)
        public ceiling = ( ^self )
        (* Answers the receiver itself: an integer is already rounded. *)
        public rounded = ( ^self )
        (* Answers the receiver itself: an integer has no fraction. *)
        public truncated = ( ^self )
        (* Answers the receiver itself. *)
        public asInteger = ( ^self )
        (* Answers the receiver itself: an integer is the numerator of the
           fraction it is, over 1. *)
        public numerator = ( ^self )
        (* Answers 1: an integer is a fraction whose denominator is 1. *)
        public denominator = ( ^1 )
        (* Answers true: an integer is a fraction whose denominator is 1. *)
        public isKindOfFraction = ( ^true )
        (* Answers the receiver raised to `exponent` modulo `modulus`, an
           integer from 0 to `modulus` - 1 for a positive `modulus`, by
           squaring and multiplying modulo `modulus` at each step; a
           negative `exponent` raises the receiver's inverse modulo
           `modulus` to `exponent negated`.

           exponent: an integer.
           modulus: an integer other than 0.

           Signals the ArgumentError `reciprocalModulo:` signals for a
           negative `exponent` when the receiver has no inverse, and the
           ArgumentErrors of `\\` and `bitAnd:` for arguments that are no
           integers or a zero `modulus`. *)
        public raisedTo: exponent modulo: modulus = (
            | result base remaining |
            exponent < 0 ifTrue: [
                ^(self reciprocalModulo: modulus) raisedTo: exponent negated modulo: modulus].
            result:: 1 \\ modulus.
            base:: self \\ modulus.
            remaining:: exponent.
            [remaining > 0] whileTrue: [
                (remaining bitAnd: 1) = 1 ifTrue: [result:: result * base \\ modulus].
                base:: base * base \\ modulus.
                remaining:: remaining >> 1].
            ^result
        )
        (* Answers the inverse of the receiver modulo `modulus`: the integer
           from 0 to `modulus` - 1 whose product with the receiver is 1
           modulo `modulus`, found by the extended Euclidean algorithm.

           modulus: a positive integer.

           Signals an ArgumentError carrying `modulus`, 'reciprocalModulo:
           … has no inverse modulo …', when the receiver and `modulus` have
           a common divisor other than 1, and the ArgumentErrors of `\\`
           and `//` for a `modulus` that is no integer or is 0. *)
        public reciprocalModulo: modulus = (
            | remainder nextRemainder coefficient nextCoefficient |
            remainder:: modulus.
            nextRemainder:: self \\ modulus.
            coefficient:: 0.
            nextCoefficient:: 1.
            [nextRemainder = 0] whileFalse: [
                | quotient previousRemainder previousCoefficient |
                quotient:: remainder // nextRemainder.
                previousRemainder:: remainder.
                remainder:: nextRemainder.
                nextRemainder:: previousRemainder - (quotient * nextRemainder).
                previousCoefficient:: coefficient.
                coefficient:: nextCoefficient.
                nextCoefficient:: previousCoefficient - (quotient * nextCoefficient)].
            remainder = 1 ifFalse: [
                ^(outer Kernel ArgumentError value: modulus)
                    signal: 'reciprocalModulo: ', self printString, ' has no inverse modulo ', modulus printString].
            ^coefficient \\ modulus
        )
```

and give `Integer` a class side (`) : ( … )`):

```newspeak
    ) : (
        (* Answers the Integer `text` writes in decimal: `parse:radix:`
           with radix 10, and its ArgumentErrors. *)
        public parse: text = ( ^self parse: text radix: 10 )
        (* Answers the Integer `text` writes in `radix`: an optional '-'
           and at least one digit, '0' to '9' then 'A' to 'Z' in either
           case, each below `radix`; a LargeInteger beyond -2^60 to
           2^60 - 1.

           text: a String.
           radix: an Integer from 2 to 36.

           Signals an ArgumentError: 'parse:radix: takes a radix from 2 to
           36, got …' carrying `radix` for one outside them, 'parse:radix:
           requires a String, got …' carrying `text` for one that is no
           String, and 'parse:radix: requires the digits of an Integer in
           radix …, got …' carrying `text` for a String that writes no
           integer, an empty one or a lone '-' included. *)
        public parse: text radix: radix = (
            ^vmMirror integerParse: text radix: radix ifFail: [^super parse: text radix: radix]
        )
    )
```

(Check how `Kernel.ns` names `ArgumentError` from inside a nested class
and how it concatenates Strings -- `Object>>subclassResponsibility` near
the top of `Kernel.ns` signals with a concatenated text -- and follow
it. `LargeInteger`'s class comment gains: "It inherits `Integer`'s
`gcd:`, `>>`, `digitAt:`, `digitLength`, `asStringRadix:` and the
methods written in Newspeak, which take either representation.")

- [ ] **Step 6: Run the goldens, then the gates**

Run: `cargo nextest run -p ns-cli --test run_goldens integer_protocol`
Expected: PASS. Then all five gates.

- [ ] **Step 7: Commit**

```bash
git add crates/ns-runtime goldens/library/number/integer
git commit -m "feat: Integer's gcd, shifts, digits, radix texts and parsing, and modular arithmetic"
```
(with the two trailer lines).

---

### Task 5: `Rational` and the numeric hash, as plain Rust

**Files:**
- Create: `crates/ns-runtime/src/numeric/rational.rs`
- Create: `crates/ns-runtime/src/numeric/numeric_hash.rs`
- Modify: `crates/ns-runtime/src/numeric/mod.rs` (declare both, re-export
  `Rational`)

**Interfaces:**
- Consumes: `ArbitraryInteger::{one, from_u64, to_u64, is_one,
  bit_length, gcd, truncated_divide}` (Task 4), and its existing
  `from_i64`, `from_i128`, `add`, `subtract`, `multiply`, `floor_divide`,
  `shift_left`, `negated`, `absolute`, `is_zero`, `is_negative`,
  `magnitude`, `Ord`.
- Produces: `crate::numeric::Rational` (`Clone`, `Debug`, `PartialEq`,
  `Eq`, `Ord`) with `pub(crate)` `from_integer(ArbitraryInteger)`,
  `new(numerator, denominator) -> Option<Rational>` (reduced, the sign on
  the numerator; `None` for a zero denominator), `is_normal(&numerator,
  &denominator) -> bool`, `from_normal_parts(numerator, denominator)`,
  `numerator() -> &ArbitraryInteger`, `denominator() ->
  &ArbitraryInteger`, `is_integer()`, `is_zero()`, `negated()`, `add`, `subtract`,
  `multiply` (`&self, &Rational) -> Rational`, `divide(&self, &Rational)
  -> Option<Rational>`, `floor() -> ArbitraryInteger`, `truncated() ->
  ArbitraryInteger`, `from_f64(f64) -> Option<Rational>` (exact; `None`
  for a NaN or an infinity), `to_f64(&self) -> f64` (correctly rounded,
  ties to even). `crate::numeric::numeric_hash::{integer_hash,
  small_integer_hash, rational_hash, float_hash}` -- `(&ArbitraryInteger)
  -> i64`, `(i64) -> i64`, `(&Rational) -> i64`, `(f64) -> i64` -- each
  within -2^60 to 2^60 - 1, and `small_integer_hash(value) == value` for
  every value in that range.

Nothing reaches these yet but their tests; Tasks 6 to 8 build on them.
The hash is the spec's decision 2: a number's exact value `n/d` taken
modulo P = 2^61 - 1 as `n × d⁻¹`, signed as `n` is, folded into the
`SmallInteger` range by a function that leaves that range alone. Since
reduction modulo P respects sums, products and cancellation, two equal
numbers hash alike whatever their representations; a denominator that P
divides hashes as the infinity of its sign, as no inverse exists.

- [ ] **Step 1: Write the failing tests**

At the end of `numeric/rational.rs` (create it with only `use` lines and
the test module first, so the tests fail to compile against the missing
items):

```rust
#[cfg(test)]
mod tests {
    use super::Rational;
    use crate::ArbitraryInteger;

    fn rational(numerator: i128, denominator: i128) -> Rational {
        Rational::new(
            ArbitraryInteger::from_i128(numerator),
            ArbitraryInteger::from_i128(denominator),
        )
        .expect("a non-zero denominator")
    }

    #[test]
    fn new_reduces_and_puts_the_sign_on_the_numerator() {
        let half = rational(-3, -6);
        assert_eq!(half.numerator(), &ArbitraryInteger::from_i64(1));
        assert_eq!(half.denominator(), &ArbitraryInteger::from_i64(2));
        assert_eq!(rational(3, -6), rational(-1, 2));
        assert!(rational(6, 3).is_integer());
        assert!(Rational::new(ArbitraryInteger::one(), ArbitraryInteger::zero()).is_none());
        assert!(Rational::is_normal(
            &ArbitraryInteger::from_i64(-3),
            &ArbitraryInteger::from_i64(4)
        ));
        assert!(!Rational::is_normal(
            &ArbitraryInteger::from_i64(2),
            &ArbitraryInteger::from_i64(4)
        ));
        assert!(!Rational::is_normal(
            &ArbitraryInteger::from_i64(3),
            &ArbitraryInteger::from_i64(-4)
        ));
        assert!(!Rational::is_normal(
            &ArbitraryInteger::from_i64(3),
            &ArbitraryInteger::zero()
        ));
    }

    #[test]
    fn arithmetic_is_exact() {
        assert_eq!(rational(1, 2).add(&rational(1, 3)), rational(5, 6));
        assert_eq!(rational(1, 2).subtract(&rational(1, 2)), rational(0, 1));
        assert_eq!(rational(2, 3).multiply(&rational(3, 4)), rational(1, 2));
        assert_eq!(rational(1, 2).divide(&rational(1, 4)), Some(rational(2, 1)));
        assert_eq!(rational(1, 2).divide(&rational(0, 1)), None);
        assert_eq!(rational(-7, 2).floor(), ArbitraryInteger::from_i64(-4));
        assert_eq!(rational(-7, 2).truncated(), ArbitraryInteger::from_i64(-3));
        assert!(rational(1, 3) < rational(1, 2));
        assert!(rational(-1, 2) < rational(-1, 3));
    }

    #[test]
    fn a_double_is_an_exact_rational() {
        assert_eq!(Rational::from_f64(0.5), Some(rational(1, 2)));
        assert_eq!(Rational::from_f64(-3.0), Some(rational(-3, 1)));
        assert_eq!(Rational::from_f64(-0.0), Some(rational(0, 1)));
        assert_eq!(
            Rational::from_f64(0.1),
            Some(rational(3_602_879_701_896_397, 36_028_797_018_963_968))
        );
        assert!(Rational::from_f64(f64::NAN).is_none());
        assert!(Rational::from_f64(f64::INFINITY).is_none());
        let smallest = Rational::from_f64(f64::from_bits(1)).expect("finite");
        assert_eq!(smallest.denominator().bit_length(), 1075);
        assert_eq!(
            Rational::from_f64(f64::MAX).expect("finite").numerator().bit_length(),
            1024
        );
    }

    #[test]
    fn to_f64_rounds_correctly_and_ties_to_even() {
        for value in [
            0.5, -0.1, 1.0 / 3.0, f64::MAX, -f64::MAX, f64::MIN_POSITIVE,
            f64::from_bits(1), f64::from_bits(0x000f_ffff_ffff_ffff), 1e300, 1e-300,
        ] {
            let exact = Rational::from_f64(value).expect("finite");
            assert_eq!(exact.to_f64().to_bits(), value.to_bits(), "{value}");
        }
        assert_eq!(rational(1, 3).to_f64(), 1.0 / 3.0);
        assert_eq!(rational(2, 3).to_f64(), 2.0 / 3.0);
        // 2^53 + 1 lies halfway between two doubles and rounds to the even
        // 2^53; 2^53 + 3 rounds up to 2^53 + 4.
        assert_eq!(rational((1 << 53) + 1, 1).to_f64(), 9_007_199_254_740_992.0);
        assert_eq!(rational((1 << 53) + 3, 1).to_f64(), 9_007_199_254_740_996.0);
        // Numerator and denominator both beyond the largest double.
        let huge = ArbitraryInteger::one().shift_left(2000).expect("small");
        let three_huge = huge.multiply(&ArbitraryInteger::from_i64(3));
        let third = Rational::new(huge.clone(), three_huge).expect("non-zero");
        assert_eq!(third.to_f64(), 1.0 / 3.0);
        let enormous = Rational::from_integer(huge.clone());
        assert_eq!(enormous.to_f64(), f64::INFINITY);
        assert_eq!(enormous.negated().to_f64(), f64::NEG_INFINITY);
        let tiny = Rational::new(ArbitraryInteger::one(), huge).expect("non-zero");
        assert_eq!(tiny.to_f64(), 0.0);
        // Just above half the smallest subnormal rounds up to it; exactly
        // half is a tie and rounds to the even zero.
        let half_smallest = Rational::new(
            ArbitraryInteger::one(),
            ArbitraryInteger::one().shift_left(1075).expect("small"),
        )
        .expect("non-zero");
        assert_eq!(half_smallest.to_f64(), 0.0);
        let above_half = half_smallest.add(&Rational::new(
            ArbitraryInteger::one(),
            ArbitraryInteger::one().shift_left(1100).expect("small"),
        ).expect("non-zero"));
        assert_eq!(above_half.to_f64(), f64::from_bits(1));
    }
}
```

(`Rational::negated` is in the interface list below too.)

At the end of `numeric/numeric_hash.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{float_hash, integer_hash, rational_hash, small_integer_hash};
    use crate::numeric::Rational;
    use crate::{ArbitraryInteger, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};

    #[test]
    fn a_small_integer_hashes_as_itself() {
        for value in [0, 1, -1, 42, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN] {
            assert_eq!(small_integer_hash(value), value);
            assert_eq!(integer_hash(&ArbitraryInteger::from_i64(value)), value);
        }
    }

    #[test]
    fn equal_numbers_hash_alike_across_representations() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        let rational = |numerator: i128, denominator: i128| {
            Rational::new(integer(numerator), integer(denominator)).expect("non-zero")
        };
        assert_eq!(float_hash(3.0), small_integer_hash(3));
        assert_eq!(float_hash(-3.0), small_integer_hash(-3));
        assert_eq!(float_hash(0.0), 0);
        assert_eq!(float_hash(-0.0), 0);
        assert_eq!(float_hash(0.5), rational_hash(&rational(1, 2)));
        assert_eq!(float_hash(-0.375), rational_hash(&rational(-3, 8)));
        let power = integer(1 << 100);
        assert_eq!(float_hash(2f64.powi(100)), integer_hash(&power));
        assert_eq!(rational_hash(&rational(6, 3)), small_integer_hash(2));
        assert_eq!(
            float_hash(f64::MAX),
            rational_hash(&Rational::from_f64(f64::MAX).expect("finite"))
        );
        assert_eq!(float_hash(f64::NAN), float_hash(-f64::NAN));
        assert_ne!(float_hash(f64::INFINITY), float_hash(f64::NEG_INFINITY));
    }

    #[test]
    fn every_hash_is_a_small_integer() {
        let modulus = (1i128 << 61) - 1;
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        for value in [modulus - 1, modulus, modulus + 1, -(modulus - 1), 1 << 120, -(1 << 120)] {
            let hash = integer_hash(&integer(value));
            assert!((SMALL_INTEGER_MIN..=SMALL_INTEGER_MAX).contains(&hash), "{value}");
        }
        let multiple_of_modulus = Rational::new(integer(1), integer(modulus)).expect("non-zero");
        let hash = rational_hash(&multiple_of_modulus);
        assert!((SMALL_INTEGER_MIN..=SMALL_INTEGER_MAX).contains(&hash));
    }
}
```

Run: `cargo nextest run -p ns-runtime numeric::` -- FAIL (does not
compile).

- [ ] **Step 2: `Rational`**

`numeric/rational.rs`, above its tests:

```rust
//! Exact rational numbers in the normal form a `Fraction` holds and the
//! tower computes in: reduced by their gcd, the denominator positive, the
//! sign on the numerator. An integer is one whose denominator is 1; a
//! `Fraction` never is (`docs/design/core-library.md`, "The numeric
//! tower").

use std::cmp::Ordering;

use crate::ArbitraryInteger;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rational {
    numerator: ArbitraryInteger,
    denominator: ArbitraryInteger,
}

/// The exponent of the largest double's leading bit, and of the smallest
/// normal double's.
const MAXIMUM_EXPONENT: i64 = 1023;
const MINIMUM_NORMAL_EXPONENT: i64 = -1022;
/// A double's significant bits, the leading one included.
const SIGNIFICANT_BITS: i64 = 53;
/// The exponent of the smallest subnormal double, 2^-1074.
const SMALLEST_EXPONENT: i64 = -1074;

impl Rational {
    pub(crate) fn from_integer(integer: ArbitraryInteger) -> Rational {
        Rational {
            numerator: integer,
            denominator: ArbitraryInteger::one(),
        }
    }

    /// `numerator / denominator` in normal form; `None` for a zero
    /// denominator.
    pub(crate) fn new(
        numerator: ArbitraryInteger,
        denominator: ArbitraryInteger,
    ) -> Option<Rational> {
        if denominator.is_zero() {
            return None;
        }
        // Not zero, since the denominator is not.
        let divisor = numerator.gcd(&denominator);
        let (mut numerator, _) = numerator.truncated_divide(&divisor)?;
        let (mut denominator, _) = denominator.truncated_divide(&divisor)?;
        if denominator.is_negative() {
            numerator = numerator.negated();
            denominator = denominator.negated();
        }
        Some(Rational {
            numerator,
            denominator,
        })
    }

    /// Whether `numerator / denominator` is already in normal form: a
    /// positive denominator and no common factor.
    pub(crate) fn is_normal(numerator: &ArbitraryInteger, denominator: &ArbitraryInteger) -> bool {
        !denominator.is_negative() && !denominator.is_zero() && numerator.gcd(denominator).is_one()
    }

    /// A rational from parts known to be in normal form: a fraction read
    /// back from the heap, which only `new` made.
    pub(crate) fn from_normal_parts(
        numerator: ArbitraryInteger,
        denominator: ArbitraryInteger,
    ) -> Rational {
        debug_assert!(Rational::is_normal(&numerator, &denominator));
        Rational {
            numerator,
            denominator,
        }
    }

    pub(crate) fn numerator(&self) -> &ArbitraryInteger {
        &self.numerator
    }

    pub(crate) fn denominator(&self) -> &ArbitraryInteger {
        &self.denominator
    }

    pub(crate) fn is_integer(&self) -> bool {
        self.denominator.is_one()
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.numerator.is_zero()
    }

    pub(crate) fn negated(&self) -> Rational {
        Rational {
            numerator: self.numerator.negated(),
            denominator: self.denominator.clone(),
        }
    }

    pub(crate) fn add(&self, other: &Rational) -> Rational {
        self.combine(other, ArbitraryInteger::add)
    }

    pub(crate) fn subtract(&self, other: &Rational) -> Rational {
        self.combine(other, ArbitraryInteger::subtract)
    }

    /// `a/b ± c/d` as `(a·d ± c·b) / (b·d)`, reduced.
    fn combine(
        &self,
        other: &Rational,
        operation: fn(&ArbitraryInteger, &ArbitraryInteger) -> ArbitraryInteger,
    ) -> Rational {
        let numerator = operation(
            &self.numerator.multiply(&other.denominator),
            &other.numerator.multiply(&self.denominator),
        );
        Rational::new(numerator, self.denominator.multiply(&other.denominator))
            .expect("a product of positive denominators is not zero")
    }

    pub(crate) fn multiply(&self, other: &Rational) -> Rational {
        Rational::new(
            self.numerator.multiply(&other.numerator),
            self.denominator.multiply(&other.denominator),
        )
        .expect("a product of positive denominators is not zero")
    }

    /// The quotient; `None` for a zero divisor.
    pub(crate) fn divide(&self, other: &Rational) -> Option<Rational> {
        Rational::new(
            self.numerator.multiply(&other.denominator),
            self.denominator.multiply(&other.numerator),
        )
    }

    /// The greatest integer not above the value.
    pub(crate) fn floor(&self) -> ArbitraryInteger {
        self.numerator
            .floor_divide(&self.denominator)
            .expect("a positive denominator")
    }

    /// The value without its fraction, toward zero.
    pub(crate) fn truncated(&self) -> ArbitraryInteger {
        self.numerator
            .truncated_divide(&self.denominator)
            .expect("a positive denominator")
            .0
    }

    /// The exact value of a finite double, which is a dyadic rational:
    /// its significand over a power of two, or an integer. `None` for a
    /// NaN or an infinity.
    pub(crate) fn from_f64(value: f64) -> Option<Rational> {
        if !value.is_finite() {
            return None;
        }
        let bits = value.to_bits();
        let negative = bits >> 63 == 1;
        let biased_exponent = ((bits >> 52) & 0x7ff) as i64;
        let fraction = bits & ((1 << 52) - 1);
        let (significand, exponent) = if biased_exponent == 0 {
            (fraction, SMALLEST_EXPONENT)
        } else {
            (fraction | (1 << 52), biased_exponent - 1075)
        };
        if significand == 0 {
            return Some(Rational::from_integer(ArbitraryInteger::zero()));
        }
        // An odd significand over a power of two is already reduced.
        let trailing = significand.trailing_zeros();
        let magnitude = ArbitraryInteger::from_u64(significand >> trailing);
        let numerator = if negative { magnitude.negated() } else { magnitude };
        let exponent = exponent + i64::from(trailing);
        Some(if exponent >= 0 {
            Rational::from_integer(
                numerator
                    .shift_left(exponent as u64)
                    .expect("a double's integer has at most 1024 bits"),
            )
        } else {
            Rational {
                numerator,
                denominator: ArbitraryInteger::one()
                    .shift_left(exponent.unsigned_abs())
                    .expect("a double's denominator has at most 1075 bits"),
            }
        })
    }

    /// The nearest double, a tie to the one whose last bit is even;
    /// beyond the largest double an infinity, below half the smallest
    /// subnormal a zero, each with the value's sign. One rounding of the
    /// exact quotient, where dividing two converted parts would round
    /// three times and give a NaN once both pass the largest double.
    pub(crate) fn to_f64(&self) -> f64 {
        if self.numerator.is_zero() {
            return 0.0;
        }
        let magnitude = self.numerator.absolute();
        // The value lies in [2^exponent, 2^(exponent + 1)), and the bit
        // lengths put the exponent at their difference or one below it.
        let mut exponent = magnitude.bit_length() as i64 - self.denominator.bit_length() as i64;
        let rounded = if exponent > MAXIMUM_EXPONENT + 1 {
            f64::INFINITY
        } else if exponent < SMALLEST_EXPONENT - 2 {
            0.0
        } else {
            if compare_with_power(&magnitude, &self.denominator, exponent) == Ordering::Less {
                exponent -= 1;
            }
            round_to_double(&magnitude, &self.denominator, exponent)
        };
        if self.numerator.is_negative() {
            -rounded
        } else {
            rounded
        }
    }
}

/// `numerator · 2^-power` against `denominator`, that is the value
/// `numerator / denominator` against `2^power`, in integers.
fn compare_with_power(
    numerator: &ArbitraryInteger,
    denominator: &ArbitraryInteger,
    power: i64,
) -> Ordering {
    let (left, right) = scaled(numerator, denominator, -power);
    left.cmp(&right)
}

/// `numerator · 2^shift` and `denominator` for a non-negative `shift`,
/// `numerator` and `denominator · 2^-shift` for a negative one: the pair
/// whose quotient is `numerator / denominator · 2^shift`.
fn scaled(
    numerator: &ArbitraryInteger,
    denominator: &ArbitraryInteger,
    shift: i64,
) -> (ArbitraryInteger, ArbitraryInteger) {
    let shifted = |integer: &ArbitraryInteger, count: u64| {
        integer
            .shift_left(count)
            .expect("a shift bounded by the double's exponent range")
    };
    if shift >= 0 {
        (shifted(numerator, shift as u64), denominator.clone())
    } else {
        (numerator.clone(), shifted(denominator, shift.unsigned_abs()))
    }
}

/// The double nearest the positive `numerator / denominator`, which lies
/// in `[2^exponent, 2^(exponent + 1))`.
fn round_to_double(
    numerator: &ArbitraryInteger,
    denominator: &ArbitraryInteger,
    exponent: i64,
) -> f64 {
    if exponent > MAXIMUM_EXPONENT {
        return f64::INFINITY;
    }
    // The significant bits the result keeps: 53, fewer below the normal
    // range, where the last one is 2^-1074's.
    let precision = if exponent >= MINIMUM_NORMAL_EXPONENT {
        SIGNIFICANT_BITS
    } else {
        exponent - SMALLEST_EXPONENT + 1
    };
    if precision <= 0 {
        // Below 2^-1074: from [2^-1075, 2^-1074) it rounds to 2^-1074 when
        // above half of it, and a tie at exactly 2^-1075 to the even zero.
        let above_half = precision == 0
            && compare_with_power(numerator, denominator, SMALLEST_EXPONENT - 1)
                == Ordering::Greater;
        return if above_half { f64::from_bits(1) } else { 0.0 };
    }
    // The quotient has exactly `precision` bits; the remainder decides the
    // rounding.
    let (scaled_numerator, scaled_denominator) =
        scaled(numerator, denominator, precision - 1 - exponent);
    let (quotient, remainder) = scaled_numerator
        .truncated_divide(&scaled_denominator)
        .expect("a positive denominator");
    let mut significand = quotient.to_u64().expect("at most 53 bits");
    let twice_remainder = remainder.shift_left(1).expect("one bit more");
    match twice_remainder.cmp(&scaled_denominator) {
        Ordering::Greater => significand += 1,
        Ordering::Equal if significand & 1 == 1 => significand += 1,
        _ => {}
    }
    // `significand · 2^power` is a double (or overflows to the infinity a
    // round-up past the largest double is); `power` lies from -1074 to
    // 971, so one step through 2^-1000 keeps every product exact.
    let power = exponent + 1 - precision;
    let value = significand as f64;
    if power < -1000 {
        value * 2f64.powi(-1000) * 2f64.powi((power + 1000) as i32)
    } else {
        value * 2f64.powi(power as i32)
    }
}

impl Ord for Rational {
    /// By value: `a/b` against `c/d` as `a·d` against `c·b`, the
    /// denominators being positive.
    fn cmp(&self, other: &Rational) -> Ordering {
        self.numerator
            .multiply(&other.denominator)
            .cmp(&other.numerator.multiply(&self.denominator))
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Rational) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
```

(The early exits of `to_f64`: with `exponent` from the bit lengths the
value lies in `[2^(exponent - 1), 2^(exponent + 1))`, so past `1024` it
overflows for certain and below `-1076` it is under `2^-1075`, half the
smallest subnormal, for certain; both bounds keep every later shift
within a few thousand bits whatever the parts' sizes.)

- [ ] **Step 3: The numeric hash**

`numeric/numeric_hash.rs`, above its tests:

```rust
//! The hash every number answers: its exact value `n/d` modulo the
//! Mersenne prime 2^61 - 1, as `n · d⁻¹`, signed as `n` is, folded into
//! the `SmallInteger` range. Reduction modulo a prime respects sums,
//! products and cancellation, so equal numbers hash alike whatever their
//! representation -- `3`, `3.0` and `6/2`; `0.5` and `1/2` -- and a
//! `SmallInteger`, which lies within the modulus, hashes as itself
//! (`docs/design/core-library.md`, "The numeric tower").

use crate::numeric::Rational;
use crate::ArbitraryInteger;

const MODULUS: u64 = (1 << 61) - 1;

/// The hash of an infinity, and of a fraction whose denominator the
/// modulus divides, which has no inverse: negated for a negative one.
const INFINITY_HASH: i64 = 314_159;

/// Every NaN's hash: they are all equal (`docs/design/core-library.md`,
/// "A NaN equals a NaN").
const NOT_A_NUMBER_HASH: i64 = 271_828;

/// Half the `SmallInteger` range: hashes lie from `-HALF` to `HALF - 1`.
const HALF: i64 = 1 << 60;

/// A magnitude modulo the modulus, from its most significant digit down.
fn residue(magnitude: &[u64]) -> u64 {
    magnitude.iter().rev().fold(0, |accumulator, digit| {
        (((u128::from(accumulator) << 64) | u128::from(*digit)) % u128::from(MODULUS)) as u64
    })
}

fn multiply(left: u64, right: u64) -> u64 {
    (u128::from(left) * u128::from(right) % u128::from(MODULUS)) as u64
}

/// `value⁻¹` modulo the prime modulus, by Fermat: `value^(P - 2)`.
/// `value` is not zero.
fn inverse(value: u64) -> u64 {
    let mut result = 1;
    let mut base = value;
    let mut exponent = MODULUS - 2;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = multiply(result, base);
        }
        base = multiply(base, base);
        exponent >>= 1;
    }
    result
}

/// A residue with a sign, folded into the `SmallInteger` range: left alone
/// inside it, moved by `HALF` toward zero outside it.
fn folded(residue: u64, negative: bool) -> i64 {
    let value = residue as i64;
    let signed = if negative { -value } else { value };
    if (-HALF..HALF).contains(&signed) {
        signed
    } else if signed > 0 {
        signed - HALF
    } else {
        signed + HALF
    }
}

pub(crate) fn integer_hash(integer: &ArbitraryInteger) -> i64 {
    folded(residue(integer.magnitude()), integer.is_negative())
}

/// An `i64`'s hash, which is the value itself within the `SmallInteger`
/// range.
pub(crate) fn small_integer_hash(value: i64) -> i64 {
    folded(value.unsigned_abs() % MODULUS, value < 0)
}

pub(crate) fn rational_hash(rational: &Rational) -> i64 {
    let negative = rational.numerator().is_negative();
    let denominator = residue(rational.denominator().magnitude());
    if denominator == 0 {
        return if negative { -INFINITY_HASH } else { INFINITY_HASH };
    }
    let numerator = residue(rational.numerator().magnitude());
    folded(multiply(numerator, inverse(denominator)), negative)
}

/// A double's hash: its exact value's, the two infinities' and NaN's
/// constants apart.
pub(crate) fn float_hash(value: f64) -> i64 {
    if value.is_nan() {
        NOT_A_NUMBER_HASH
    } else if value.is_infinite() {
        if value > 0.0 {
            INFINITY_HASH
        } else {
            -INFINITY_HASH
        }
    } else {
        rational_hash(&Rational::from_f64(value).expect("a finite double"))
    }
}
```

`numeric/mod.rs` declares `pub(crate) mod numeric_hash;` and
`mod rational;` with `pub(crate) use rational::Rational;`, and its note
gains "the exact rationals the tower computes in, and the hash every
number answers".

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p ns-runtime numeric::`
Expected: PASS. Clippy will flag the module's items as unused until Task
6 uses them: add `#[allow(dead_code)]` on `mod rational;` and
`mod numeric_hash;` with a comment "used from Task 6 of the numbers plan
on", and remove both in Task 7, which uses every item.

- [ ] **Step 5: Run the gates, then commit**

```bash
git add crates/ns-runtime/src/numeric
git commit -m "feat: exact rationals and the numeric hash, as plain Rust"
```
(with the two trailer lines).

---

### Task 6: `Fraction`'s representation: the class, its making, its text and its place in the image

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`BuiltinClass::Fraction` and
  `ALL`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs`
  (`kernel_class_name`; the tests that list builtins)
- Modify: `crates/ns-runtime/src/heap.rs` (`allocate_fraction`,
  `fraction_parts`, `fraction`, `Heap::describe`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`
  (`Mutator::new_fraction`, `Mutator::new_rational`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (a
  `FractionPrimitive` family, its four rows, `Primitive::Fraction`)
- Create: `crates/ns-runtime/src/builtins/numbers/fraction.rs`
- Modify: `crates/ns-runtime/src/builtins/numbers/mod.rs`,
  `crates/ns-runtime/src/builtins/mod.rs` (dispatch)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`class Fraction`)
- Modify: `crates/ns-runtime/core-library/PlatformFactory.ns`
  (`KernelNamespace>>Fraction`)
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`VALUE_FRACTION = 10`,
  the builtin tags' "next free number" to 40, the value tags' to 11)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`builtin_class_tag`,
  `encode_value`, the graph walk)
- Modify: `crates/ns-runtime/src/image/decode.rs` (the builtin mapping,
  `decode_value`, the first pass's `value`, a `DecodeError` variant,
  `decode_value`'s doc comment)
- Modify: `crates/ns-runtime/src/image/inspect.rs`
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`,
  `crates/ns-runtime/tests/image.rs`
- Modify: `docs/design/image-format.md` (the value tags and the builtin
  class tags)
- Create: `goldens/library/number/fraction/a_fraction_is_made_in_normal_form.{ns,toml}`

**Interfaces:**
- Consumes: `numeric::Rational` (Task 5).
- Produces: `BuiltinClass::Fraction` (tag 39); in `heap.rs`
  `pub(crate) fn allocate_fraction(&mut self, class, numerator: Value,
  denominator: Value) -> ObjectReference`, `pub(crate) fn
  fraction_parts(&self, object) -> Option<(Value<'gc>, Value<'gc>)>` and
  `pub(crate) fn fraction(&self, object) -> Option<Rational>`; on
  `Mutator`: `pub(crate) fn new_fraction(&mut self, rational: Rational)
  -> Value<'gc>` (a rational that is no integer) and `pub(crate) fn
  new_rational(&mut self, rational: Rational) -> Value<'gc>` (an integer
  through `new_integer` when the denominator is 1, a fraction otherwise);
  `Primitive::Fraction(FractionPrimitive)` with `Numerator`,
  `Denominator`, `Make`, `MakeReduced`; mirror rows
  `fractionNumerator:ifFail:`, `fractionDenominator:ifFail:`,
  `fractionNumerator:denominator:ifFail:`,
  `fractionReducedNumerator:denominator:ifFail:`; `class Fraction` in
  `Kernel.ns` with `isKindOfFraction`, `numerator`, `denominator`,
  `printString`, `asString`, class-side `numerator:denominator:` and
  `reducedNumerator:denominator:`; `platform kernel Fraction`;
  `VALUE_FRACTION`.

`495e6c61` ("an abstract Collection in the kernel") added a builtin class
the same way: `git show 495e6c61 -- crates/ns-runtime/src/types.rs
crates/ns-runtime/src/interpreter/kernel.rs
crates/ns-runtime/src/image crates/ns-runtime/tests/opcode_tags.rs
crates/ns-runtime/core-library/PlatformFactory.ns` lists every place a
builtin is named; `f1206a21` ("images carry LargeInteger values") added
the inline value tag the same way. Follow both.

A fraction's object: `Format::FixedPointers`, two fields, numerator then
denominator, each a `SmallInteger` or a `LargeInteger`, in normal form;
its class's builtin is `Fraction`. `fraction_parts` answers the two
fields only when both hold; anything else -- `Fraction new` (no field),
an instance of a program's subclass of `Fraction` (its class's builtin
is none) -- is no number, and every leaf declines it. Until Task 7, `=`
between two fractions is `Value`'s derived equality over no declared
slot, which answers `true`; no golden of this task compares two
fractions.

- [ ] **Step 1: Write the failing golden**

`goldens/library/number/fraction/a_fraction_is_made_in_normal_form.ns`:

```newspeak
(* `Fraction numerator:denominator:` makes a fraction in normal form --
   reduced, its sign on the numerator -- and answers an Integer when the
   denominator divides the numerator. `reducedNumerator:denominator:`
   takes a pair already in that form and refuses any other. A fraction
   prints as `(numerator/denominator)`; `Fraction new` holds no value and
   is no number. *)
class AFractionIsMadeInNormalForm = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          kernel = platform kernel.
          Fraction = kernel Fraction.
          Error = kernel Error.
          threeQuarters = Fraction numerator: 6 denominator: 8. |
        transcript print: threeQuarters; show: ' '; print: threeQuarters numerator; show: ' '; print: threeQuarters denominator; show: ' '; print: threeQuarters class; newLine.
        transcript print: (Fraction numerator: 1 denominator: -2); show: ' '; print: (Fraction numerator: -3 denominator: -6); newLine.
        transcript print: (Fraction numerator: 6 denominator: 3); show: ' '; print: (Fraction numerator: 6 denominator: 3) class; show: ' '; print: (Fraction numerator: 0 denominator: 5); newLine.
        transcript print: (Fraction numerator: 1180591620717411303424 denominator: 3); show: ' '; print: (Fraction numerator: 1 denominator: 1180591620717411303424) denominator class; newLine.
        transcript print: (Fraction reducedNumerator: 3 denominator: 4); show: ' '; show: (Fraction reducedNumerator: -3 denominator: 4) asString; newLine.
        transcript print: threeQuarters isKindOfFraction; show: ' '; print: threeQuarters isKindOfNumber; show: ' '; print: threeQuarters isKindOfInteger; show: ' '; print: threeQuarters isKindOfFloat; newLine.
        transcript show: ([Fraction numerator: 1 denominator: 0] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Fraction numerator: 1.5 denominator: 2] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Fraction numerator: 1 denominator: nil] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Fraction reducedNumerator: 2 denominator: 4] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Fraction reducedNumerator: 3 denominator: 1] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Fraction reducedNumerator: 3 denominator: -4] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([Fraction new numerator] on: kernel MessageNotUnderstood do: [:error | 'no number']); newLine
    )
)
```

`a_fraction_is_made_in_normal_form.toml`:

```toml
stdout = '''
(3/4) 3 4 Fraction
(-1/2) (1/2)
2 Integer 0
(1180591620717411303424/3) LargeInteger
(3/4) (-3/4)
true true false false
numerator:denominator: requires a non-zero denominator
numerator:denominator: requires integers, got 1.5
numerator:denominator: requires integers, got nil
reducedNumerator:denominator: requires a fraction in normal form, got 2/4
reducedNumerator:denominator: requires a fraction in normal form, got 3/1
reducedNumerator:denominator: requires a fraction in normal form, got 3/-4
no number
'''
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo nextest run -p ns-cli --test run_goldens a_fraction_is_made_in_normal_form`
Expected: FAIL -- `kernel Fraction` is not understood.

- [ ] **Step 3: The builtin**

`types.rs`: `BuiltinClass::Fraction` after `Collection`, with the doc
comment "An exact quotient of two integers in normal form, made only by
the runtime: a `Number` whose two hidden fields no slot names
(`numeric::Rational`)."; add it to `BuiltinClass::ALL`.
`kernel.rs`: `BuiltinClass::Fraction => "Fraction"`; the hierarchy test
gains `(BuiltinClass::Fraction, Some(BuiltinClass::Number))`; the value
test `(BuiltinClass::Fraction, true)`; the absent-accessor list
`"Fraction"`. `encode.rs`'s `builtin_class_tag`: `BuiltinClass::Fraction
=> 39`; `decode.rs`'s mapping `39 => BuiltinClass::Fraction`;
`opcodes.rs`: the builtin tags' "Next free number: 40.";
`tests/opcode_tags.rs` pins 39 (follow how it pins 38).

- [ ] **Step 4: The heap and the mutator**

`heap.rs`, beside `allocate_large_integer`:

```rust
    /// A `Fraction` object of `class` holding `numerator` and
    /// `denominator`, two integers in normal form.
    /// `Mutator::new_fraction` is the one caller, and has made them so.
    pub(crate) fn allocate_fraction(
        &mut self,
        class: ClassReference<'gc>,
        numerator: Value<'gc>,
        denominator: Value<'gc>,
    ) -> ObjectReference<'gc> {
        // `store`'s check, as `allocate_array` makes it.
        #[cfg(debug_assertions)]
        for part in [numerator, denominator] {
            self.store(part);
        }
        let (object, fields) = self.allocate_object(class, Format::FixedPointers, 0, false, 2);
        write_words(
            &mut self.space.words_mut()[fields],
            [numerator.word(), denominator.word()].into_iter(),
        );
        object
    }

    /// A `Fraction`'s numerator and denominator, each a `SmallInteger` or
    /// a `LargeInteger`; `None` for any other object -- `Fraction new`,
    /// which has no field, and an instance of a program's subclass, whose
    /// class is no builtin.
    pub(crate) fn fraction_parts(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<(Value<'gc>, Value<'gc>)> {
        if self.class_builtin(self.object_class(object)) != Some(BuiltinClass::Fraction)
            || self.instance_field_count(object) != Some(2)
        {
            return None;
        }
        Some((self.instance_field(object, 0)?, self.instance_field(object, 1)?))
    }

    /// A `Fraction`'s value; `None` for any other object. Copies out.
    pub(crate) fn fraction(&self, object: ObjectReference<'gc>) -> Option<crate::numeric::Rational> {
        let (numerator, denominator) = self.fraction_parts(object)?;
        let integer = |value: Value<'gc>| match value.kind() {
            ValueKind::SmallInteger(integer) => Some(crate::ArbitraryInteger::from_i64(integer)),
            ValueKind::Object(part) => self.large_integer(part),
            _ => None,
        };
        Some(crate::numeric::Rational::from_normal_parts(
            integer(numerator)?,
            integer(denominator)?,
        ))
    }
```

(Use the real names of `instance_field_count`/`instance_field` and of
`allocate_array`'s word writer; check that `Format::FixedPointers` with a
field count of 2 is what `allocate_instance` makes for a two-slot
instance.)

`Heap::describe`, in the `Format` match before the fall-through arm:
`_ if self.fraction_parts(object).is_some() => { let (numerator,
denominator) = …; format!("({}/{})", self.describe(numerator),
self.describe(denominator)) }` -- the text `printString` answers.

`interpreter/mod.rs`, beside `new_integer`:

```rust
    /// A `Fraction` holding `rational`, which must not be an integer:
    /// each part through `new_integer`, so a part that fits is a
    /// `SmallInteger`. The one way a fraction is made.
    pub(crate) fn new_fraction(&mut self, rational: crate::numeric::Rational) -> Value<'gc> {
        debug_assert!(!rational.is_integer());
        let numerator = self.new_integer(rational.numerator().clone());
        let denominator = self.new_integer(rational.denominator().clone());
        let class = self.builtin_class(BuiltinClass::Fraction);
        Value::object(self.heap.allocate_fraction(class, numerator, denominator))
    }

    /// `rational` as the number it is: an integer when its denominator is
    /// 1, a `Fraction` otherwise.
    pub(crate) fn new_rational(&mut self, rational: crate::numeric::Rational) -> Value<'gc> {
        if rational.is_integer() {
            self.new_integer(rational.numerator().clone())
        } else {
            self.new_fraction(rational)
        }
    }
```

(Allocation never collects -- a collection runs only at a safepoint --
so `numerator` and `denominator` stay valid across the third
allocation, as a leaf's values do across `new_array`.)

- [ ] **Step 5: The leaves and their rows**

`primitive.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FractionPrimitive {
    /// `numerator`: a `Fraction`'s numerator.
    Numerator,
    /// `denominator`: a `Fraction`'s denominator.
    Denominator,
    /// `Fraction class>>numerator:denominator:`: any two integers, the
    /// denominator not zero, made normal.
    Make,
    /// `Fraction class>>reducedNumerator:denominator:`: a pair already in
    /// normal form, checked.
    MakeReduced,
}
```

`Primitive::Fraction(FractionPrimitive)` with a doc line; all four
`false` in `answers_false_to_a_stranger`; rows after the float rows:

| mirror selector | reported |
|---|---|
| `fractionNumerator:ifFail:` | `numerator` |
| `fractionDenominator:ifFail:` | `denominator` |
| `fractionNumerator:denominator:ifFail:` | `numerator:denominator:` |
| `fractionReducedNumerator:denominator:ifFail:` | `reducedNumerator:denominator:` |

`builtins/mod.rs`: `Primitive::Fraction(primitive) =>
numbers::execute_fraction(mutator, primitive, receiver,
arguments).map(wrap_error)`; `numbers/mod.rs`: `mod fraction;` and
`pub(crate) use fraction::execute_fraction;`.

`numbers/fraction.rs`:

```rust
//! `Fraction`'s leaves: reading a fraction's parts and making one. A
//! fraction is made only here and by the tower's arithmetic, both through
//! `Mutator::new_rational`, so every one is in normal form
//! (`docs/design/core-library.md`, "The numeric tower").

use crate::builtins::primitive::FractionPrimitive;
use crate::numeric::Rational;
use crate::types::*;
use crate::{ArbitraryInteger, Mutator, Symbol};

use super::integer_operand;

pub(crate) fn execute_fraction<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FractionPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        FractionPrimitive::Numerator => {
            let (numerator, _) = mutator.heap.fraction_parts(receiver.as_object()?)?;
            Some(Ok(numerator))
        }
        FractionPrimitive::Denominator => {
            let (_, denominator) = mutator.heap.fraction_parts(receiver.as_object()?)?;
            Some(Ok(denominator))
        }
        FractionPrimitive::Make => Some(make(mutator, receiver, arguments)),
        FractionPrimitive::MakeReduced => Some(make_reduced(mutator, receiver, arguments)),
    }
}

/// The two integer arguments, or the `ArgumentError` carrying the first
/// that is none: "<selector> requires integers, got …".
fn integer_pair<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: &str,
    arguments: &[Value<'gc>],
) -> Result<(ArbitraryInteger, ArbitraryInteger), NewspeakError<'gc>> {
    let mut parts = Vec::with_capacity(2);
    for index in 0..2 {
        let argument = arguments.get(index).copied();
        match integer_operand(&mutator.heap, argument) {
            Some(integer) => parts.push(integer),
            None => {
                return Err(NewspeakError::argument_error(
                    Symbol::intern(selector),
                    receiver,
                    argument.unwrap_or(Value::nil()),
                    format!(
                        "{selector} requires integers, got {}",
                        mutator.heap.describe_argument(argument)
                    ),
                ))
            }
        }
    }
    let denominator = parts.pop().expect("two parts");
    let numerator = parts.pop().expect("two parts");
    Ok((numerator, denominator))
}

/// `Fraction numerator:denominator:`: the pair made normal, an integer
/// when the denominator divides the numerator.
fn make<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = "numerator:denominator:";
    let (numerator, denominator) = integer_pair(mutator, receiver, selector, arguments)?;
    match Rational::new(numerator, denominator) {
        Some(rational) => Ok(mutator.new_rational(rational)),
        None => Err(NewspeakError::argument_error(
            Symbol::intern(selector),
            receiver,
            arguments.get(1).copied().unwrap_or(Value::nil()),
            "numerator:denominator: requires a non-zero denominator",
        )),
    }
}

/// `Fraction reducedNumerator:denominator:`: the pair as given, which must
/// already be a fraction in normal form -- reduced, its denominator above
/// 1 -- since a fraction outside it would break `=` and `hash`.
fn make_reduced<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = "reducedNumerator:denominator:";
    let (numerator, denominator) = integer_pair(mutator, receiver, selector, arguments)?;
    if denominator.is_one() || !Rational::is_normal(&numerator, &denominator) {
        return Err(NewspeakError::argument_error(
            Symbol::intern(selector),
            receiver,
            arguments.get(1).copied().unwrap_or(Value::nil()),
            format!(
                "{selector} requires a fraction in normal form, got {}/{}",
                numerator.to_bounded_text(),
                denominator.to_bounded_text()
            ),
        ));
    }
    Ok(mutator.new_fraction(Rational::from_normal_parts(numerator, denominator)))
}
```

(`integer_operand` is `numbers/mod.rs`'s, `pub(super)` since Task 1;
check that `NewspeakError::argument_error` takes a `&str` and a `String`
alike, as its other calls suggest.)

- [ ] **Step 6: `Fraction` in `Kernel.ns` and the kernel facet**

After `Float` in `Kernel.ns`:

```newspeak
    (* The exact quotient of two integers, in normal form: its
       denominator above 1, no factor common to both parts, its sign on
       the numerator. `Fraction numerator:denominator:` makes one, and so
       does dividing integers that do not divide; a fraction is never
       equal to an integer, which a quotient that divides is instead. *)
    (* Made only by the runtime, so a fraction is always in normal form
       and `=` and `hash` can compare its parts. Its parts are two hidden
       fields, not slots: no program writes them. *)
    public class Fraction = Number new () (
        (* Answers true: the receiver is a Fraction. *)
        public isKindOfFraction = ( ^true )
        (* Answers the numerator, an integer of either sign. *)
        public numerator = ( ^vmMirror fractionNumerator: self ifFail: [^super numerator] )
        (* Answers the denominator, an integer above 1. *)
        public denominator = ( ^vmMirror fractionDenominator: self ifFail: [^super denominator] )
        (* Answers a new String of the numerator and the denominator
           between parentheses and separated by '/': '(3/4)', '(-1/2)'. *)
        public printString = (
            ^'(', self numerator printString, '/', self denominator printString, ')'
        )
        (* Answers what `printString` answers. *)
        public asString = ( ^self printString )
    ) : (
        (* Answers `numerator` divided by `denominator`, exactly: an
           Integer when `denominator` divides `numerator`, otherwise a
           Fraction in normal form, its sign on the numerator.

           numerator: an integer.
           denominator: an integer other than 0.

           Signals an ArgumentError: 'numerator:denominator: requires a
           non-zero denominator' carrying `denominator` for 0, and
           'numerator:denominator: requires integers, got …' carrying the
           first argument that is no integer. *)
        public numerator: numerator denominator: denominator = (
            ^vmMirror fractionNumerator: numerator denominator: denominator
                ifFail: [^super numerator: numerator denominator: denominator]
        )
        (* Answers the Fraction `numerator`/`denominator`, a pair the
           caller has put in normal form already: no factor common to both,
           `denominator` above 1. Nothing is reduced.

           numerator: an integer.
           denominator: an integer above 1 with no factor common to
           `numerator`.

           Signals an ArgumentError carrying `denominator`,
           'reducedNumerator:denominator: requires a fraction in normal
           form, got …/…', for a pair not in normal form, and one carrying
           the argument, 'reducedNumerator:denominator: requires integers,
           got …', for an argument that is no integer. *)
        public reducedNumerator: numerator denominator: denominator = (
            ^vmMirror fractionReducedNumerator: numerator denominator: denominator
                ifFail: [^super reducedNumerator: numerator denominator: denominator]
        )
    )
```

`PlatformFactory.ns`, `KernelNamespace`, after `Collection`:

```newspeak
        (* Answers the kernel's class `Fraction`, whose class side makes an
           exact quotient of two integers. *)
        public Fraction = ( ^kernelModule Fraction )
```

`Number`'s class comment: "The superclass of `Integer`, `Float` and
`Fraction`: …".

- [ ] **Step 7: The image**

`opcodes.rs`:

```rust
/// A `Fraction`, carried inline as a value rather than as an object
/// record, since a value has no identity (§3.1.1): the numerator and then
/// the denominator, each a `VALUE_SMALL_INTEGER` or a
/// `VALUE_LARGE_INTEGER` in its canonical form. Only a fraction in normal
/// form is accepted -- a denominator above 1, no common factor -- since
/// anything else would break `=` and `hash`.
pub const VALUE_FRACTION: u8 = 10;
```

and the value tags' "Next free number: 11." `tests/opcode_tags.rs` pins
it.

`encode.rs`'s `encode_value`, after the `LargeInteger` branch:

```rust
    // A `Fraction` likewise, by value: its two parts in the integers' own
    // encoding, which `write_integer_value` shares with the branch above.
    if let Some((numerator, denominator)) = context.heap.fraction_parts(object) {
        writer.write_u8(VALUE_FRACTION);
        write_integer_value(writer, context, numerator)?;
        write_integer_value(writer, context, denominator)?;
        return Ok(());
    }
```

where `write_integer_value` is the `LargeInteger` branch's body made a
function over an integer `Value` (a `SmallInteger` written as
`VALUE_SMALL_INTEGER`, a `LargeInteger` as the branch writes it), which
the branch then calls too. Wherever the graph walk keeps a
`LargeInteger` from becoming a record (`GraphTable::visit_value` and the
`ObjectShape::LargeInteger` arms -- `grep -n "large_integer\|LargeInteger"
crates/ns-runtime/src/image/encode.rs`), a fraction is kept out the same
way: it is a value, and its parts are no objects of the graph.

`decode.rs`: a `DecodeError::NonNormalFraction` variant (follow the
neighbours' doc comment and `Display`); in `decode_value`:

```rust
        // Its two parts allocate as `VALUE_SMALL_INTEGER` and
        // `VALUE_LARGE_INTEGER` do, and the fraction four words more; its
        // ratio to its bytes stays below the bound above.
        VALUE_FRACTION => {
            let numerator = read_integer_part(reader)?;
            let denominator = read_integer_part(reader)?;
            if denominator.is_one() || !Rational::is_normal(&numerator, &denominator) {
                return Err(DecodeError::NonNormalFraction);
            }
            Ok(context
                .mutator
                .new_fraction(Rational::from_normal_parts(numerator, denominator)))
        }
```

with

```rust
/// One part of a `VALUE_FRACTION`: a `VALUE_SMALL_INTEGER` or a
/// `VALUE_LARGE_INTEGER`, nothing else -- a fraction never nests.
fn read_integer_part(reader: &mut Reader) -> Result<ArbitraryInteger, DecodeError> {
    match reader.read_u8()? {
        VALUE_SMALL_INTEGER => Ok(ArbitraryInteger::from_i64(reader.read_i64()?)),
        VALUE_LARGE_INTEGER => read_large_integer(reader),
        tag => Err(DecodeError::InvalidTag { what: "fraction part", tag }),
    }
}
```

and the first pass's `value` reads it the same way, checking the normal
form there too so a rejected image allocates nothing:
`VALUE_FRACTION => { let numerator = read_integer_part(reader)?; let
denominator = read_integer_part(reader)?; if …not normal… {
Err(NonNormalFraction) } else { Ok(()) } }`. `decode_value`'s doc
comment: a new bullet under "Per allocating value" -- a
`VALUE_FRACTION` of two one-word parts beyond 61 bits is 19 bytes (the
tag and two `VALUE_SMALL_INTEGER`s) for the word that holds it, a
fraction of 4 words and two `LargeInteger`s of 3 words each, 8 + 32 + 48
= 88 bytes, 4.6x, with the two transient digits 104 bytes, 5.5x; parts
written as `VALUE_LARGE_INTEGER`s cost more bytes for the same objects;
so the bound of 10.7x and 16x stands. (Check the `LargeInteger` object's
word count the bullet above it uses -- 24 bytes for one digit -- and
redo the sums with it.)

`inspect.rs`: `opcodes::VALUE_FRACTION => format!("fraction {}/{}",
…)`, the two parts read with the readers `inspect.rs` uses for the two
integer tags.

`tests/image.rs`: next to the test that round-trips a `LargeInteger`
value (`grep -n "LargeInteger\|large" crates/ns-runtime/tests/image.rs`),
add one that round-trips a value holding fractions -- `(1/3)`, `(-7/2)`,
`(1/(2^70))`, `((2^70)/3)` in an `Array` -- and checks each loads with
equal parts; and one per refusal: a `VALUE_FRACTION` whose denominator is
0, 1, -4, or shares a factor with the numerator (`2/4`), and one whose
part is a `VALUE_FLOAT`, each a `DecodeError` and no panic. Build the
bytes the way the neighbouring refusal tests do (they patch an encoded
image or write the graph section by hand -- follow them).

`docs/design/image-format.md`: the value-tag list gains 10, the
builtin-tag list 39, each with one sentence, as `LargeInteger`'s and
`Collection`'s entries read.

- [ ] **Step 8: Run the golden, the image tests, then the gates**

Run: `cargo nextest run -p ns-cli --test run_goldens a_fraction_is_made_in_normal_form`,
`cargo nextest run -p ns-runtime --test image`, then all five gates.
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add -A crates/ns-runtime goldens/library/number/fraction docs/design/image-format.md
git commit -m "feat: Fraction, made in normal form by the runtime and carried inline in an image"
```
(with the two trailer lines).

---

### Task 7: The tower's comparison, equality and hash

**Files:**
- Create: `crates/ns-runtime/src/builtins/numbers/tower.rs`
- Modify: `crates/ns-runtime/src/builtins/numbers/mod.rs` (`mod tower;`,
  the module note)
- Modify: `crates/ns-runtime/src/builtins/numbers/integer.rs`,
  `large_integer.rs`, `float.rs` (comparison, `=` and `hash` hand every
  case but their fast path to `tower`)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (a
  `NumberPrimitive` family with its six rows, `Primitive::Number`)
- Modify: `crates/ns-runtime/src/builtins/mod.rs` (dispatch)
- Modify: `crates/ns-runtime/src/builtins/value.rs` (`compare_node`,
  `hash_node`, `derived_equality`)
- Modify: `crates/ns-runtime/src/numeric/mod.rs` (drop the
  `#[allow(dead_code)]`s of Task 5)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Number`'s
  comparisons, `=` and `hash`; the doc comments of `Integer`'s,
  `LargeInteger`'s and `Float`'s)
- Create: `goldens/library/number/tower/numbers_compare_by_exact_value.{ns,toml}`
- Create: `goldens/library/number/tower/numbers_hash_as_they_compare.{ns,toml}`
- Modify: every golden whose output `=` or an ordering across classes
  changes (Step 8)

**Interfaces:**
- Consumes: `numeric::Rational`, `numeric::numeric_hash` (Task 5);
  `Heap::fraction` (Task 6).
- Produces, in `builtins/numbers/tower.rs` (all `pub(crate)`):
  `enum Operand { Small(i64), Large(ArbitraryInteger), Fraction(Rational),
  Float(f64) }` with `fn exact(&self) -> Option<Rational>`, `fn
  to_f64(&self) -> f64`, `fn is_float(&self) -> bool`, `fn
  integer(&self) -> Option<ArbitraryInteger>`; `fn operand(heap, Value)
  -> Option<Operand>`; `fn order(&Operand, &Operand) -> Option<Ordering>`
  (`None` with a NaN); `fn equal(&Operand, &Operand) -> bool` (a NaN
  equals a NaN); `fn hash(&Operand) -> i64`; `enum Comparison { Less,
  Greater, LessOrEqual, GreaterOrEqual }`; the leaf glue `fn
  compare(heap, Comparison, receiver, arguments) -> Option<Result<Value,
  NewspeakError>>` (`None` declines a receiver that is no number), `fn
  values_equal(heap, Value, Option<Value>) -> bool` (`false` unless both
  are numbers), `fn value_hash(heap, Value) -> Option<i64>`, and
  `execute_number` for `Primitive::Number`. `NumberPrimitive::{Less,
  Greater, LessOrEqual, GreaterOrEqual, Equal, Hash}` and their rows
  `number:lessThan:ifFail:`, `number:greaterThan:ifFail:`,
  `number:lessOrEqual:ifFail:`, `number:greaterOrEqual:ifFail:`,
  `number:equals:ifFail:`, `numberHash:ifFail:`. On `Number`: `<`, `>`,
  `<=`, `>=`, `=`, `hash`, which `Fraction` inherits.

The spec's decisions 1 and 2: `=` and the orderings compare any two
numbers by exact value; every comparison with a NaN answers `false` but
NaN `=` NaN; `=` with a non-number is `false` and an ordering with one
the type mismatch; equal numbers hash alike. The fast paths stay where
they are -- two `SmallInteger`s, two `Float`s -- and a `SmallInteger`
within 2^53 meets a `Float` as a double, which is exact there; every
other pair goes through `Rational`. Once this task is in,
`integer_equals`, `large_integer_equals`, `large_integer_hash`,
`float_equals` and `float_hash` are gone: `value.rs` asks the tower too.

- [ ] **Step 1: Write the failing goldens**

`goldens/library/number/tower/numbers_compare_by_exact_value.ns`:

```newspeak
(* `=` and the orderings compare an Integer, a LargeInteger, a Fraction
   and a Float by their exact values, in both directions: 2^53 + 1 is
   not the double 2^53 it rounds to, and is greater than it. A NaN is
   ordered with nothing and equals only a NaN; an infinity lies beyond
   every other number. `=` with something that is no number is false; an
   ordering with it is an ArgumentError. *)
class NumbersCompareByExactValue = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          kernel = platform kernel.
          Fraction = kernel Fraction.
          Error = kernel Error.
          half = Fraction numerator: 1 denominator: 2.
          third = Fraction numerator: 1 denominator: 3.
          nan = 0.0 / 0.0.
          infinity = 1.0 / 0.0.
          twoToThe53 = 9007199254740992.
          twoToThe70 = 1180591620717411303424. |
        transcript print: 1 = 1.0; show: ' '; print: 1.0 = 1; show: ' '; print: -0.0 = 0; show: ' '; print: 3 = 3; newLine.
        transcript print: twoToThe53 = twoToThe53 asFloat; show: ' '; print: twoToThe53 + 1 = (twoToThe53 + 1) asFloat; show: ' '; print: (twoToThe53 + 1) asFloat = twoToThe53; newLine.
        transcript print: twoToThe70 = twoToThe70 asFloat; show: ' '; print: twoToThe70 asFloat = twoToThe70; show: ' '; print: twoToThe70 + 1 = twoToThe70 asFloat; newLine.
        transcript print: half = 0.5; show: ' '; print: 0.5 = half; show: ' '; print: third = 0.3333333333333333; show: ' '; print: half = (Fraction numerator: 2 denominator: 4); show: ' '; print: half = third; show: ' '; print: half = nil; newLine.
        transcript print: half < 0.6; show: ' '; print: 0.6 > half; show: ' '; print: third < half; show: ' '; print: 3 < (Fraction numerator: 7 denominator: 2); show: ' '; print: (Fraction numerator: 7 denominator: 2) <= 3; newLine.
        transcript print: twoToThe53 + 1 > twoToThe53 asFloat; show: ' '; print: twoToThe53 asFloat < (twoToThe53 + 1); show: ' '; print: twoToThe70 + 1 > twoToThe70 asFloat; newLine.
        transcript print: nan < 1; show: ' '; print: 1 < nan; show: ' '; print: nan = nan; show: ' '; print: 1 = nan; show: ' '; print: half < nan; newLine.
        transcript print: infinity > twoToThe70; show: ' '; print: infinity negated < (Fraction numerator: -1 denominator: 3); show: ' '; print: twoToThe70 < infinity; newLine.
        transcript show: ([half < nil] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([3 < 'x'] on: Error do: [:error | error messageText]); newLine
    )
)
```

`numbers_compare_by_exact_value.toml`:

```toml
stdout = '''
true true true true
true false true
true true false
true true false true false false
true true true true false
true true true
false false true false false
true true true
< requires a numeric argument, got nil
< requires a numeric argument, got the String "x"
'''
```

`goldens/library/number/tower/numbers_hash_as_they_compare.ns`:

```newspeak
(* Equal numbers hash alike whatever their classes, so `1`, `1.0` and
   `2/2` are one key of a Map and one element of a Set, a NaN key is
   found again, and a value whose slot holds `3` equals and hashes like
   one whose slot holds `3.0`. An integer's hash is the integer, and an
   integral Float's too.

   The module is a value (`Object <: Value`) because §3.1.1 asks a value
   class's enclosing object to be one; the transcript lives in `Script`,
   which is no value. *)
class NumbersHashAsTheyCompare = Object <: Value () (
    class Box of: initialContents = Value new (
        | public contents = initialContents. |
    ) ()
    class Script on: aPlatform = Object new (
        | platform = aPlatform. transcript = aPlatform transcript. |
    ) (
        public run = (
            | Fraction = platform kernel Fraction.
              half = Fraction numerator: 1 denominator: 2.
              nan = 0.0 / 0.0.
              twoToThe70 = 1180591620717411303424.
              map set |
            transcript print: 3 hash = 3.0 hash; show: ' '; print: -3 hash = -3.0 hash; show: ' '; print: 0 hash = -0.0 hash; show: ' '; print: 3.0 hash; show: ' '; print: -3.0 hash; newLine.
            transcript print: half hash = 0.5 hash; show: ' '; print: twoToThe70 hash = twoToThe70 asFloat hash; show: ' '; print: nan hash = (0.0 / 0.0) hash; show: ' '; print: (Fraction numerator: 2 denominator: 4) hash = half hash; newLine.
            map:: platform collections Map new.
            map at: 1 put: #one.
            map at: half put: #half.
            transcript print: (map at: 1.0); show: ' '; print: (map at: 0.5); show: ' '; print: map size; newLine.
            map at: 2 put: #two.
            map at: 2.0 put: #twoAgain.
            transcript print: (map at: 2); show: ' '; print: map size; newLine.
            map at: nan put: #nan.
            transcript print: (map at: 0.0 / 0.0); newLine.
            set:: platform collections Set new.
            set add: 1; add: 1.0; add: (Fraction numerator: 2 denominator: 2); add: nan; add: 0.0 / 0.0.
            transcript print: set size; newLine.
            transcript print: (Box of: 3) = (Box of: 3.0); show: ' '; print: (Box of: 3) hash = (Box of: 3.0) hash; show: ' '; print: (Box of: half) = (Box of: 0.5); show: ' '; print: (Box of: 3) = (Box of: 4.0); newLine
        )
    )
    public main: platform args: arguments = (
        (Script on: platform) run
    )
) : ()
```

`numbers_hash_as_they_compare.toml`:

```toml
stdout = '''
true true true 3 -3
true true true true
#one #half 2
#twoAgain 3
#nan
2
true true true false
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens tower/`
Expected: FAIL -- `1 = 1.0` answers `false`.

- [ ] **Step 3: The tower's rules, with their property test**

`numbers/tower.rs`:

```rust
//! The numeric tower's rules, once for every pair of representations:
//! `Integer` (`Small`), `LargeInteger` (`Large`), `Fraction` and `Float`.
//! An argument is classified once (`operand`); comparison and equality go
//! by exact value, a finite double being the dyadic rational it is; the
//! hash is `numeric_hash`'s, which agrees with that equality. The
//! representations' own leaves keep their fast paths and send every other
//! case here, as `Number`'s rows -- which `Fraction` inherits -- send all
//! of theirs (`docs/design/core-library.md`, "The numeric tower").

use std::cmp::Ordering;

use crate::builtins::primitive::{NumberPrimitive, Primitive};
use crate::numeric::{numeric_hash, Rational};
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

use super::{type_mismatch, Accepted};

/// A number, read out of its representation once.
#[derive(Debug, Clone)]
pub(crate) enum Operand {
    Small(i64),
    Large(ArbitraryInteger),
    Fraction(Rational),
    Float(f64),
}

/// The largest integer every smaller one of which a double holds exactly.
const EXACT_DOUBLE_INTEGER: u64 = 1 << 53;

impl Operand {
    /// The exact value; `None` for a NaN or an infinity.
    pub(crate) fn exact(&self) -> Option<Rational> {
        match self {
            Operand::Small(integer) => Some(Rational::from_integer(ArbitraryInteger::from_i64(*integer))),
            Operand::Large(integer) => Some(Rational::from_integer(integer.clone())),
            Operand::Fraction(rational) => Some(rational.clone()),
            Operand::Float(float) => Rational::from_f64(*float),
        }
    }

    /// The nearest double, a tie to even.
    pub(crate) fn to_f64(&self) -> f64 {
        match self {
            // `as` rounds to the nearest double, ties to even.
            Operand::Small(integer) => *integer as f64,
            Operand::Large(integer) => integer.to_f64(),
            Operand::Fraction(rational) => rational.to_f64(),
            Operand::Float(float) => *float,
        }
    }

    pub(crate) fn is_float(&self) -> bool {
        matches!(self, Operand::Float(_))
    }

    /// The integer an `Integer` or a `LargeInteger` is; `None` for a
    /// `Fraction` or a `Float`.
    pub(crate) fn integer(&self) -> Option<ArbitraryInteger> {
        match self {
            Operand::Small(integer) => Some(ArbitraryInteger::from_i64(*integer)),
            Operand::Large(integer) => Some(integer.clone()),
            _ => None,
        }
    }
}

/// `value` as a number; `None` for anything that is no number, `Fraction
/// new` and a `LargeInteger new` among them.
pub(crate) fn operand<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<Operand> {
    match value.kind() {
        ValueKind::SmallInteger(integer) => Some(Operand::Small(integer)),
        ValueKind::SmallFloat(float) => Some(Operand::Float(float)),
        ValueKind::Object(object) => {
            if let Some(float) = heap.float_value(value) {
                return Some(Operand::Float(float));
            }
            if let Some(integer) = heap.large_integer(object) {
                return Some(Operand::Large(integer));
            }
            heap.fraction(object).map(Operand::Fraction)
        }
        _ => None,
    }
}

/// The order of two numbers by their exact values; `None` when either is
/// a NaN, which is ordered with nothing.
pub(crate) fn order(left: &Operand, right: &Operand) -> Option<Ordering> {
    match (left, right) {
        (Operand::Small(first), Operand::Small(second)) => Some(first.cmp(second)),
        (Operand::Float(first), Operand::Float(second)) => first.partial_cmp(second),
        (Operand::Float(float), other) => float_order(*float, other),
        (other, Operand::Float(float)) => float_order(*float, other).map(Ordering::reverse),
        _ => Some(left.exact()?.cmp(&right.exact()?)),
    }
}

/// A double against a number that is no double.
fn float_order(float: f64, other: &Operand) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    if float.is_infinite() {
        return Some(if float > 0.0 { Ordering::Greater } else { Ordering::Less });
    }
    // Within 2^53 an integer converts exactly, so the doubles decide.
    if let Operand::Small(integer) = other {
        if integer.unsigned_abs() <= EXACT_DOUBLE_INTEGER {
            return float.partial_cmp(&(*integer as f64));
        }
    }
    Some(Rational::from_f64(float)?.cmp(&other.exact()?))
}

/// `=` across the tower: the same exact value, and a NaN equals a NaN
/// (`docs/design/core-library.md`, "A NaN equals a NaN").
pub(crate) fn equal(left: &Operand, right: &Operand) -> bool {
    match (left, right) {
        (Operand::Float(first), Operand::Float(second)) if first.is_nan() && second.is_nan() => true,
        _ => order(left, right) == Some(Ordering::Equal),
    }
}

/// The hash `=` agrees with.
pub(crate) fn hash(operand: &Operand) -> i64 {
    match operand {
        Operand::Small(integer) => numeric_hash::small_integer_hash(*integer),
        Operand::Large(integer) => numeric_hash::integer_hash(integer),
        Operand::Fraction(rational) => numeric_hash::rational_hash(rational),
        Operand::Float(float) => numeric_hash::float_hash(*float),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Comparison {
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
}

impl Comparison {
    fn selector(self) -> &'static str {
        match self {
            Comparison::Less => "<",
            Comparison::Greater => ">",
            Comparison::LessOrEqual => "<=",
            Comparison::GreaterOrEqual => ">=",
        }
    }

    fn holds(self, order: Ordering) -> bool {
        match self {
            Comparison::Less => order.is_lt(),
            Comparison::Greater => order.is_gt(),
            Comparison::LessOrEqual => order.is_le(),
            Comparison::GreaterOrEqual => order.is_ge(),
        }
    }
}

/// An ordering leaf: a Boolean, `false` when either side is a NaN; the
/// type mismatch for an argument that is no number. `None` declines a
/// receiver that is no number.
pub(crate) fn compare<'gc>(
    heap: &Heap<'gc>,
    comparison: Comparison,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let left = operand(heap, receiver)?;
    let argument = arguments.first().copied();
    let Some(right) = argument.and_then(|value| operand(heap, value)) else {
        return Some(Err(type_mismatch(
            heap,
            receiver,
            Symbol::intern(comparison.selector()),
            Accepted::Number,
            argument,
        )));
    };
    Some(Ok(Value::boolean(
        order(&left, &right).is_some_and(|order| comparison.holds(order)),
    )))
}

/// `=` between two values: `false` unless both are numbers of one value.
pub(crate) fn values_equal<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    argument: Option<Value<'gc>>,
) -> bool {
    let Some(left) = operand(heap, receiver) else {
        return false;
    };
    argument
        .and_then(|value| operand(heap, value))
        .is_some_and(|right| equal(&left, &right))
}

/// A number's hash; `None` for anything that is no number.
pub(crate) fn value_hash<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<i64> {
    operand(heap, value).map(|number| hash(&number))
}

/// `Number`'s own rows, which `Fraction` inherits and any other number
/// reaches only through `super`.
pub(crate) fn execute_number<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: NumberPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let comparison = |comparison| compare(&mutator.heap, comparison, receiver, arguments);
    match primitive {
        NumberPrimitive::Less => comparison(Comparison::Less),
        NumberPrimitive::Greater => comparison(Comparison::Greater),
        NumberPrimitive::LessOrEqual => comparison(Comparison::LessOrEqual),
        NumberPrimitive::GreaterOrEqual => comparison(Comparison::GreaterOrEqual),
        NumberPrimitive::Equal => {
            operand(&mutator.heap, receiver)?;
            let equal = values_equal(&mutator.heap, receiver, arguments.first().copied());
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Number(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        NumberPrimitive::Hash => {
            value_hash(&mutator.heap, receiver).map(|hash| Ok(Value::small_integer(hash)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::{equal, hash, order, Operand};
    use crate::numeric::Rational;
    use crate::{ArbitraryInteger, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};

    /// Numbers around every edge the tower has: 2^53, the `SmallInteger`
    /// range, the modulus 2^61 - 1, the largest double and beyond, the
    /// smallest subnormal, the zeros, the infinities, NaN -- each in
    /// every representation that can hold it.
    fn samples() -> Vec<Operand> {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        let power = |exponent: u64| ArbitraryInteger::one().shift_left(exponent).expect("small");
        let fraction = |numerator: ArbitraryInteger, denominator: ArbitraryInteger| {
            Operand::Fraction(Rational::new(numerator, denominator).expect("non-zero"))
        };
        let two_to_the_53 = 1i64 << 53;
        let mut samples = vec![
            Operand::Small(0),
            Operand::Small(1),
            Operand::Small(-1),
            Operand::Small(3),
            Operand::Small(two_to_the_53),
            Operand::Small(two_to_the_53 + 1),
            Operand::Small(-two_to_the_53),
            Operand::Small(SMALL_INTEGER_MAX),
            Operand::Small(SMALL_INTEGER_MIN),
            Operand::Large(integer(i128::from(SMALL_INTEGER_MAX) + 1)),
            Operand::Large(integer((1 << 61) - 1)),
            Operand::Large(power(70)),
            Operand::Large(power(70).add(&integer(1))),
            Operand::Large(power(70).negated()),
            Operand::Large(power(1100)),
            fraction(integer(1), integer(2)),
            fraction(integer(-1), integer(2)),
            fraction(integer(1), integer(3)),
            fraction(integer(i128::from(two_to_the_53) + 1), integer(2)),
            fraction(integer(1), integer((1 << 61) - 1)),
            fraction(integer(1), power(1075)),
            fraction(power(1100).add(&integer(1)), power(1100).multiply(&integer(3))),
        ];
        for float in [
            0.0, -0.0, 0.5, -0.5, 3.0, 1.0 / 3.0, 2f64.powi(53), 2f64.powi(60), 2f64.powi(61),
            2f64.powi(70), f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1), f64::INFINITY,
            f64::NEG_INFINITY, f64::NAN,
        ] {
            samples.push(Operand::Float(float));
        }
        samples
    }

    fn is_not_a_number(operand: &Operand) -> bool {
        matches!(operand, Operand::Float(float) if float.is_nan())
    }

    #[test]
    fn equality_is_an_equivalence_that_the_order_and_the_hash_agree_with() {
        let samples = samples();
        for left in &samples {
            assert!(equal(left, left), "{left:?} equals itself");
            for right in &samples {
                let same = equal(left, right);
                assert_eq!(same, equal(right, left), "{left:?} = {right:?} is symmetric");
                if same {
                    assert_eq!(hash(left), hash(right), "{left:?} and {right:?} hash alike");
                }
                match order(left, right) {
                    Some(Ordering::Equal) => assert!(same, "{left:?} = {right:?}"),
                    Some(found) => {
                        assert!(!same, "{left:?} and {right:?} are ordered apart");
                        assert_eq!(order(right, left), Some(found.reverse()));
                    }
                    None => assert!(is_not_a_number(left) || is_not_a_number(right)),
                }
                for third in &samples {
                    if same && equal(right, third) {
                        assert!(equal(left, third), "{left:?} = {right:?} = {third:?}");
                    }
                    if order(left, right) == Some(Ordering::Less)
                        && order(right, third) == Some(Ordering::Less)
                    {
                        assert_eq!(order(left, third), Some(Ordering::Less));
                    }
                }
            }
        }
    }

    #[test]
    fn a_double_and_an_integer_compare_exactly_past_two_to_the_53() {
        let two_to_the_53 = 1i64 << 53;
        let rounded = Operand::Float((two_to_the_53 + 1) as f64);
        assert_eq!(order(&Operand::Small(two_to_the_53 + 1), &rounded), Some(Ordering::Greater));
        assert!(equal(&Operand::Small(two_to_the_53), &rounded));
        assert!(!equal(&Operand::Small(two_to_the_53 + 1), &rounded));
    }
}
```

(The `Debug` derive on `Operand` is for the assertion texts; it never
formats a `Value`.)

`numbers/mod.rs`: `mod tower;` and `pub(crate) use tower::execute_number;`.

Run: `cargo nextest run -p ns-runtime tower` -- FAIL until Step 4 adds
`NumberPrimitive`, then PASS.

- [ ] **Step 4: `Number`'s rows**

`primitive.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberPrimitive {
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
    Hash,
}
```

`Primitive::Number(NumberPrimitive)` with the doc line "`Number`'s own
leaves, over every representation: the tower (`builtins::numbers::tower`)."
In `answers_false_to_a_stranger`: `Primitive::Number(NumberPrimitive::Equal)
=> true`, the other five `false`. `builtins/mod.rs`:
`Primitive::Number(primitive) => numbers::execute_number(mutator,
primitive, receiver, arguments).map(wrap_error)`. Rows after the
`fraction…` rows:

| mirror selector | reported |
|---|---|
| `number:lessThan:ifFail:` | `<` |
| `number:greaterThan:ifFail:` | `>` |
| `number:lessOrEqual:ifFail:` | `<=` |
| `number:greaterOrEqual:ifFail:` | `>=` |
| `number:equals:ifFail:` | `=` |
| `numberHash:ifFail:` | `hash` |

(`interpreter::future::declines_for_a_future` checks in a debug build
that its caller is an equality leaf; if it lists them by variant, add
`NumberPrimitive::Equal` there.)

- [ ] **Step 5: The representations' leaves hand their other cases to the tower**

`integer.rs`: the `compare` closure keeps its `small_integer_order` fast
path and answers everything else with `tower::compare(&mutator.heap,
comparison, receiver, arguments).expect("an Integer receiver is a
number")`. `IntegerPrimitive::Equal`: a `SmallInteger` argument compares
directly; anything else is `tower::values_equal`. Remove
`integer_equals`, and with it `with_small_integer_parts` and
`integer_ordering` from `mod.rs` if nothing else uses them.

`large_integer.rs`: `Less`/`Greater`/`LessOrEqual`/`GreaterOrEqual` are
`tower::compare(…)`; `Equal` is `tower::values_equal` behind
`declines_for_a_future`; `Hash` is `tower::value_hash`. Both still
decline a receiver with no digits (`operand` answers `None` for one).
Remove `large_integer_equals` and `large_integer_hash`.

`float.rs`: `compare` keeps its fast path for a `Float` argument and
answers everything else with `tower::compare`; `Equal` keeps the
two-`Float` rule (NaN equals NaN, `0.0 = -0.0`) for a `Float` argument and
is `tower::values_equal` otherwise; `Hash` is
`numeric_hash::float_hash(receiver_float)`. Remove `float_equals` and
`float_hash`; `every_nan_equals_every_other_and_hashes_alike` becomes a
test of `tower::equal` and `tower::hash` over `Operand::Float`s, with the
same four NaNs and `0.0`/`-0.0`.

`numbers/mod.rs`'s re-export list loses the removed names.

- [ ] **Step 6: A value's derived equality and hash**

`builtins/value.rs`:

- `compare_node`: the `SmallInteger` and `SmallFloat` arms, and the
  `LargeInteger` and boxed-float checks in the `Object` arm, become one
  check at the top of the function: `if let Some(left) =
  tower::operand(mutator.heap(), first) { return
  Some(Pair::Decided(tower::operand(mutator.heap(), second).is_some_and(|right|
  tower::equal(&left, &right)))); }` -- a number equals only a number of
  its value.
- `derived_equality`: before it compares the two classes, a pair of two
  numbers is decided by the tower and the walk goes on:

```rust
        // Two numbers are equal by value across their classes -- `3` and
        // `3.0` -- so the tower decides them before the classes are
        // compared (`docs/design/core-library.md`, "The numeric tower").
        if let (Some(left), Some(right)) = (
            tower::operand(mutator.heap(), first),
            tower::operand(mutator.heap(), second),
        ) {
            if !tower::equal(&left, &right) {
                return Some(false);
            }
            continue;
        }
```

- `hash_node`: the `SmallInteger` and `SmallFloat` arms and the
  `LargeInteger` and boxed-float checks become one check at the top:
  `if let Some(hash) = tower::value_hash(mutator.heap(), value) { return
  Some(Node::Leaf(crate::hash::content_hash(hash))); }`. The comment above
  the old `SmallInteger` arm (why a slot's hash is folded below 2^32)
  moves with it, rewritten: every number's slot hash is its numeric hash
  folded by `content_hash`, so equal numbers in two slots hash alike and
  the sum stays inside an `i64`.

`tower` is `pub(super)` from `numbers`; make it `pub(crate)` (`pub(crate)
mod tower;`) so `value.rs` reaches it.

`the_equality_and_the_hash_decline_on_the_same_shapes` and the other
tests in `value.rs` keep passing; if one of them pins that `3` and `3.0`
in a slot are unequal, that is decision 2 changing it -- rewrite its
expectation and its comment.

- [ ] **Step 7: `Kernel.ns`**

`Number`, after `isKindOfFraction`:

```newspeak
        (* Answers whether the receiver is less than `other` by their exact
           values, a Boolean: false whenever either is a NaN.

           other: a number.

           Signals an ArgumentError carrying `other`, '< requires a numeric
           argument, got …', for any other argument. *)
        public < other = ( ^vmMirror number: self lessThan: other ifFail: [^super < other] )
        (* As `<`, whether the receiver is greater; its error text names
           '>'. *)
        public > other = ( ^vmMirror number: self greaterThan: other ifFail: [^super > other] )
        (* As `<`, whether the receiver is less or equal; its error text
           names '<='. *)
        public <= other = ( ^vmMirror number: self lessOrEqual: other ifFail: [^super <= other] )
        (* As `<`, whether the receiver is greater or equal; its error text
           names '>='. *)
        public >= other = (
            ^vmMirror number: self greaterOrEqual: other ifFail: [^super >= other]
        )
        (* Answers whether `other` is a number of the same exact value,
           whatever its class: `1 = 1.0`, `(1/2) = 0.5`; a NaN equals a
           NaN.

           other: any object; never signals. Anything that is no number is
           unequal. *)
        public = other = ( ^vmMirror number: self equals: other ifFail: [^super = other] )
        (* Answers a hash of the exact value, alike for every two numbers
           `=` makes equal, whatever their classes; an integer's is the
           integer itself. *)
        public hash = ( ^vmMirror numberHash: self ifFail: [^super hash] )
```

Rewrite the doc comments that state the old rule:
`Integer>>=` ("Answers whether `other` is a number of the same exact
value: `3 = 3.0` is true. other: any object; never signals. Anything that
is no number is unequal."), `Integer>><` ("… A Float or a Fraction
`other` is compared by exact value, so 2^53 + 1 is greater than the Float
2^53. other: a number."), `Integer>>hash` ("Answers the receiver itself,
the hash every number of its value answers."), `LargeInteger>>=`,
`LargeInteger>>hash` ("Answers a hash of the value, alike for every
number `=` makes equal."), `LargeInteger>><`, `Float>>=` ("… a number of
the same exact value, whatever its class: `0.0` equals `-0.0` and `0`,
every NaN every other NaN."), `Float>><` and `Float>>hash` likewise.

- [ ] **Step 8: Run the goldens; every changed `=`**

Run: `cargo nextest run -p ns-cli --test run_goldens tower/` -- PASS.
Then `cargo nextest run --workspace` and `run-corpus`: every other
failure must be `=` across classes now answering by value, or an
ordering between an integer past 2^53 and a Float now exact
(`goldens/library/number/float/an_integer_meets_a_float.toml`'s `3 =
3.0` is one). Check each by decision 1, update the expected output and
any comment stating the class-tight rule ("stays class-tight until the
numeric tower"), and list them in the commit message.

- [ ] **Step 9: The module note**

`builtins/numbers/mod.rs`'s note, "Integers are unbounded; the numeric
tower is not there yet", is rewritten to the current state: each integer
has one representation; a fraction is made only in normal form; the
tower (`tower.rs`) compares and hashes every pair by exact value, and the
representations' leaves keep only their fast paths; `//`, `\\` and the
arithmetic across a `Fraction` are still the leaves' own until Task 8
(that last clause is Task 8's to delete). Remove the bullet on the
class-tight `=`.

- [ ] **Step 10: Run the gates, then commit**

```bash
git add -A crates/ns-runtime goldens corpus
git commit -m "feat: numbers compare and hash by exact value across Integer, LargeInteger, Fraction and Float"
```
(the body lists the goldens whose output changed; the two trailer lines).

---

### Task 8: The tower's arithmetic, and `Fraction`'s whole protocol

**Files:**
- Modify: `crates/ns-runtime/src/builtins/numbers/tower.rs` (`Arithmetic`,
  `compute`, `arithmetic`, `answer`)
- Modify: `crates/ns-runtime/src/builtins/numbers/mod.rs`
  (`division_by_zero` carries the divisor; the module note)
- Modify: `crates/ns-runtime/src/builtins/numbers/integer.rs`,
  `large_integer.rs`, `float.rs` (arithmetic, `//`, `\\` and the float
  leaves' argument hand their other cases to the tower)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (nine
  `NumberPrimitive` variants and rows)
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Number`'s
  arithmetic, `asFloat`, `negated`, `abs`, `raisedTo:`; `Fraction`'s
  rounding; the doc comments of `Integer`'s, `LargeInteger`'s and
  `Float`'s arithmetic)
- Create: `goldens/library/number/tower/arithmetic_is_exact_without_a_float.{ns,toml}`
- Create: `goldens/library/number/fraction/a_fraction_rounds_and_converts.{ns,toml}`
- Modify: every golden whose `/`, `//` or `\\` output changes (Step 7)

**Interfaces:**
- Consumes: `tower::{Operand, operand}` (Task 7), `Rational` (Task 5),
  `Mutator::new_rational` (Task 6).
- Produces: in `tower.rs`, `pub(crate) enum Arithmetic { Add, Subtract,
  Multiply, Divide, FloorDivide, Modulo, Quotient, Remainder }`,
  `pub(crate) fn arithmetic(mutator, Arithmetic, receiver, arguments) ->
  Option<Result<Value, NewspeakError>>` (`None` declines a receiver that
  is no number) and `pub(crate) fn as_float(mutator, receiver) ->
  Option<Value>`; `NumberPrimitive::{Add, Subtract, Multiply, Divide,
  FloorDivide, Modulo, Quotient, Remainder, AsFloat}` with rows
  `number:add:ifFail:`, `number:subtract:ifFail:`,
  `number:multiply:ifFail:`, `number:divide:ifFail:`,
  `number:floorDivide:ifFail:`, `number:modulo:ifFail:`,
  `number:quotient:ifFail:`, `number:remainder:ifFail:`,
  `numberAsFloat:ifFail:`. On `Number`: `+ - * / // \\ quo: rem:`,
  `asFloat`, `negated`, `abs`, `raisedTo:`; on `Fraction`: `floor`,
  `ceiling`, `rounded`, `truncated`, `asInteger`.

The spec's decisions 5 to 8. With no `Float` among the operands the
answer is exact: an `Integer` when it is integral, a `Fraction`
otherwise. With a `Float` the other side is rounded once to the nearest
double and IEEE 754 computes. `a // b` is the floor of `a / b`, `a \\ b`
is `a - (a // b * b)`, `quo:` and `rem:` their truncating counterparts;
over doubles `//` is `(a / b) floor`, `\\` the floored `fmod`, `quo:` `(a
/ b) truncated`, `rem:` `fmod`. An exact division by zero, and `//`,
`\\`, `quo:` and `rem:` by any zero, `0.0` included, signal "division by
zero" carrying the divisor; `/` by a zero `Float`, or of a `Float` by
zero, is IEEE's infinity or NaN.

- [ ] **Step 1: Write the failing goldens**

`goldens/library/number/tower/arithmetic_is_exact_without_a_float.ns`:

```newspeak
(* With no Float among the operands, arithmetic is exact: dividing
   integers that do not divide answers a Fraction, and a Fraction whose
   denominator becomes 1 answers an Integer. A Float among them makes
   the answer a Float, the other operand rounded once to the nearest
   double. `//` and `\\` round toward negative infinity, `quo:` and
   `rem:` toward zero, over every pair of numbers; any of them by zero,
   and an exact `/` by zero, is "division by zero", while a Float's `/`
   by zero is an infinity. *)
class ArithmeticIsExactWithoutAFloat = () (
    public main: platform args: arguments = (
        | transcript = platform transcript.
          kernel = platform kernel.
          Error = kernel Error.
          ArgumentError = kernel ArgumentError.
          half = 1 / 2.
          third = 1 / 3.
          twoToThe70 = 1180591620717411303424. |
        transcript print: 7 / 2; show: ' '; print: 6 / 3; show: ' '; print: (6 / 3) class; show: ' '; print: -6 / 4; show: ' '; print: (7 / 2) class; newLine.
        transcript print: half + third; show: ' '; print: half - third; show: ' '; print: half * third; show: ' '; print: half / third; show: ' '; print: half + half; show: ' '; print: (half + half) class; newLine.
        transcript print: 3 + half; show: ' '; print: half * 4; show: ' '; print: twoToThe70 / 3; show: ' '; print: twoToThe70 / 4; newLine.
        transcript print: half + 0.25; show: ' '; print: 0.25 + half; show: ' '; print: third * 3.0; show: ' '; print: 1 / 2.0; newLine.
        transcript print: 7 // 2; show: ' '; print: -7 // 2; show: ' '; print: -7 \\ 2; show: ' '; print: (-7 quo: 2); show: ' '; print: (-7 rem: 2); newLine.
        transcript print: (7 / 2) // 1; show: ' '; print: (-7 / 2) \\ 1; show: ' '; print: ((-7 / 2) quo: 1); show: ' '; print: ((-7 / 2) rem: 1); show: ' '; print: 7 // 2.0; show: ' '; print: 7.5 \\ 2; show: ' '; print: -7.5 \\ 2; show: ' '; print: (-7.5 rem: 2); show: ' '; print: (7.5 quo: 2); newLine.
        transcript print: (2 raisedTo: 10); show: ' '; print: (2 raisedTo: -2); show: ' '; print: (half raisedTo: 3); show: ' '; print: (2 raisedTo: 0.5) > 1.414; show: ' '; print: (2 raisedTo: 100); newLine.
        transcript print: 1.0 / 0; show: ' '; print: 1 / 0.0; show: ' '; print: -1 / 0.0; newLine.
        transcript show: ([1 / 0] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([half / 0] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([1.5 // 0] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([7 \\ 0.0] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([0 raisedTo: -1] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([half + nil] on: Error do: [:error | error messageText]); newLine.
        transcript show: ([7 // nil] on: Error do: [:error | error messageText]); newLine.
        transcript print: ([1.5 // 0.0] on: ArgumentError do: [:error | error argument]); newLine
    )
)
```

`arithmetic_is_exact_without_a_float.toml` (`(-7/2) \\ 1` is `-7/2 - (-4)`;
`-7.5 \\ 2` is `fmod`'s `-1.5` moved to the divisor's sign):

```toml
stdout = '''
(7/2) 2 Integer (-3/2) Fraction
(5/6) (1/6) (1/6) (3/2) 1 Integer
(7/2) 2 (1180591620717411303424/3) 295147905179352825856
0.75 0.75 1.0 0.5
3 -4 1 -3 -1
3 (1/2) -3 (-1/2) 3.0 1.5 0.5 -1.5 3.0
1024 (1/4) (1/8) true 1267650600228229401496703205376
Infinity Infinity -Infinity
division by zero
division by zero
division by zero
division by zero
division by zero
+ requires a numeric argument, got nil
// requires a numeric argument, got nil
0.0
'''
```

`goldens/library/number/fraction/a_fraction_rounds_and_converts.ns`:

```newspeak
(* A Fraction rounds to an Integer -- `floor` and `ceiling` outward,
   `truncated` and `asInteger` toward zero, `rounded` to the nearest with
   a half away from zero -- and converts to the nearest Float in one
   rounding, so a fraction whose parts are both beyond the largest Float
   still converts, and one beyond it or below the smallest converts to an
   infinity or zero. It answers the mathematical functions through that
   Float, and `negated` and `abs` exactly. *)
class AFractionRoundsAndConverts = () (
    public main: platform args: arguments = (
        | transcript = platform transcript. half = 1 / 2. third = 1 / 3. huge = 10 raisedTo: 400. |
        transcript print: (7 / 2) floor; show: ' '; print: (7 / 2) ceiling; show: ' '; print: (7 / 2) rounded; show: ' '; print: (7 / 2) truncated; show: ' '; print: (7 / 2) asInteger; newLine.
        transcript print: (-7 / 2) floor; show: ' '; print: (-7 / 2) ceiling; show: ' '; print: (-7 / 2) rounded; show: ' '; print: (-7 / 2) truncated; show: ' '; print: (5 / 3) rounded; show: ' '; print: (-5 / 3) rounded; newLine.
        transcript print: half asFloat; show: ' '; print: third asFloat; show: ' '; print: (huge / 3) asFloat; show: ' '; print: (1 / huge) asFloat; show: ' '; print: (huge + 1 / (huge * 3)) asFloat; newLine.
        transcript print: half negated; show: ' '; print: (-1 / 2) abs; show: ' '; print: half abs; show: ' '; print: (1 / 4) sqrt; show: ' '; print: half isNaN; show: ' '; print: half isFinite; newLine
    )
)
```

`a_fraction_rounds_and_converts.toml`:

```toml
stdout = '''
3 4 4 3 3
-4 -3 -4 -3 2 -2
0.5 0.3333333333333333 Infinity 0.0 0.3333333333333333
(-1/2) (1/2) (1/2) 0.5 false true
'''
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo nextest run -p ns-cli --test run_goldens arithmetic_is_exact_without_a_float a_fraction_rounds_and_converts`
Expected: FAIL -- `7 / 2` is not understood.

- [ ] **Step 3: The tower's arithmetic**

`tower.rs`, after `hash`:

```rust
#[derive(Debug, Clone, Copy)]
pub(crate) enum Arithmetic {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Quotient,
    Remainder,
}

impl Arithmetic {
    fn selector(self) -> &'static str {
        match self {
            Arithmetic::Add => "+",
            Arithmetic::Subtract => "-",
            Arithmetic::Multiply => "*",
            Arithmetic::Divide => "/",
            Arithmetic::FloorDivide => "//",
            Arithmetic::Modulo => "\\\\",
            Arithmetic::Quotient => "quo:",
            Arithmetic::Remainder => "rem:",
        }
    }
}

/// An arithmetic answer before it is a `Value`.
enum Outcome {
    Integer(ArbitraryInteger),
    Rational(Rational),
    Float(f64),
}

/// The one refusal arithmetic has.
struct DivisionByZero;

/// `operation` over two numbers: over doubles when either is a `Float`,
/// exactly otherwise.
fn compute(
    operation: Arithmetic,
    left: &Operand,
    right: &Operand,
) -> Result<Outcome, DivisionByZero> {
    if left.is_float() || right.is_float() {
        return float_compute(operation, left.to_f64(), right.to_f64());
    }
    if let (Some(dividend), Some(divisor)) = (left.integer(), right.integer()) {
        return integer_compute(operation, &dividend, &divisor);
    }
    let first = left.exact().expect("no Float, so finite");
    let second = right.exact().expect("no Float, so finite");
    let floor = |quotient: Rational| quotient.floor();
    let truncate = |quotient: Rational| quotient.truncated();
    let rounded_quotient = |round: fn(Rational) -> ArbitraryInteger| {
        first.divide(&second).map(round).ok_or(DivisionByZero)
    };
    let remainder_after = |quotient: ArbitraryInteger| {
        first.subtract(&Rational::from_integer(quotient).multiply(&second))
    };
    Ok(match operation {
        Arithmetic::Add => Outcome::Rational(first.add(&second)),
        Arithmetic::Subtract => Outcome::Rational(first.subtract(&second)),
        Arithmetic::Multiply => Outcome::Rational(first.multiply(&second)),
        Arithmetic::Divide => Outcome::Rational(first.divide(&second).ok_or(DivisionByZero)?),
        Arithmetic::FloorDivide => Outcome::Integer(rounded_quotient(floor)?),
        Arithmetic::Quotient => Outcome::Integer(rounded_quotient(truncate)?),
        Arithmetic::Modulo => Outcome::Rational(remainder_after(rounded_quotient(floor)?)),
        Arithmetic::Remainder => Outcome::Rational(remainder_after(rounded_quotient(truncate)?)),
    })
}

/// Two integers: `ArbitraryInteger`'s own operations, a quotient that
/// does not divide made a `Rational`.
fn integer_compute(
    operation: Arithmetic,
    dividend: &ArbitraryInteger,
    divisor: &ArbitraryInteger,
) -> Result<Outcome, DivisionByZero> {
    Ok(Outcome::Integer(match operation {
        Arithmetic::Add => dividend.add(divisor),
        Arithmetic::Subtract => dividend.subtract(divisor),
        Arithmetic::Multiply => dividend.multiply(divisor),
        Arithmetic::Divide => {
            return Rational::new(dividend.clone(), divisor.clone())
                .map(Outcome::Rational)
                .ok_or(DivisionByZero)
        }
        Arithmetic::FloorDivide => dividend.floor_divide(divisor).ok_or(DivisionByZero)?,
        Arithmetic::Modulo => dividend.floor_modulo(divisor).ok_or(DivisionByZero)?,
        Arithmetic::Quotient => dividend.truncated_divide(divisor).ok_or(DivisionByZero)?.0,
        Arithmetic::Remainder => dividend.truncated_divide(divisor).ok_or(DivisionByZero)?.1,
    }))
}

/// Two doubles: IEEE 754's arithmetic; `//` and its kin refuse a zero
/// divisor where `/` answers an infinity or a NaN.
fn float_compute(operation: Arithmetic, left: f64, right: f64) -> Result<Outcome, DivisionByZero> {
    let rounding = matches!(
        operation,
        Arithmetic::FloorDivide | Arithmetic::Modulo | Arithmetic::Quotient | Arithmetic::Remainder
    );
    if rounding && right == 0.0 {
        return Err(DivisionByZero);
    }
    Ok(Outcome::Float(match operation {
        Arithmetic::Add => left + right,
        Arithmetic::Subtract => left - right,
        Arithmetic::Multiply => left * right,
        Arithmetic::Divide => left / right,
        Arithmetic::FloorDivide => (left / right).floor(),
        Arithmetic::Quotient => (left / right).trunc(),
        // `%` is C's `fmod`: exact, with the dividend's sign.
        Arithmetic::Remainder => left % right,
        Arithmetic::Modulo => {
            let remainder = left % right;
            if remainder != 0.0 && (remainder < 0.0) != (right < 0.0) {
                remainder + right
            } else {
                remainder
            }
        }
    }))
}

fn answer<'gc>(mutator: &mut Mutator<'gc>, outcome: Outcome) -> Value<'gc> {
    match outcome {
        Outcome::Integer(integer) => mutator.new_integer(integer),
        Outcome::Rational(rational) => mutator.new_rational(rational),
        Outcome::Float(float) => mutator.new_float(float),
    }
}

/// An arithmetic leaf: the answer, the type mismatch for an argument that
/// is no number, or "division by zero" carrying the divisor. `None`
/// declines a receiver that is no number.
pub(crate) fn arithmetic<'gc>(
    mutator: &mut Mutator<'gc>,
    operation: Arithmetic,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let left = operand(&mutator.heap, receiver)?;
    let argument = arguments.first().copied();
    let Some(right) = argument.and_then(|value| operand(&mutator.heap, value)) else {
        return Some(Err(type_mismatch(
            &mutator.heap,
            receiver,
            Symbol::intern(operation.selector()),
            Accepted::Number,
            argument,
        )));
    };
    Some(match compute(operation, &left, &right) {
        Ok(outcome) => Ok(answer(mutator, outcome)),
        Err(DivisionByZero) => Err(super::division_by_zero(
            receiver,
            operation.selector(),
            argument.unwrap_or(Value::nil()),
        )),
    })
}

/// `Number>>asFloat`: the nearest Float, in one rounding.
pub(crate) fn as_float<'gc>(mutator: &mut Mutator<'gc>, receiver: Value<'gc>) -> Option<Value<'gc>> {
    let number = operand(&mutator.heap, receiver)?;
    Some(mutator.new_float(number.to_f64()))
}
```

`execute_number` gains the nine new variants: the eight arithmetic ones
are `arithmetic(mutator, Arithmetic::…, receiver, arguments)`, `AsFloat`
is `as_float(mutator, receiver).map(Ok)`.

`mod.rs`'s `division_by_zero` takes the divisor as a third parameter and
carries it (the text stays "division by zero"); its callers in
`integer.rs` and `large_integer.rs` pass the argument. Add to `tower.rs`'s
tests:

```rust
    #[test]
    fn the_rounding_divisions_agree_with_their_definitions() {
        let integer = |value: i64| Operand::Small(value);
        let fraction = |numerator: i128, denominator: i128| {
            Operand::Fraction(
                Rational::new(
                    ArbitraryInteger::from_i128(numerator),
                    ArbitraryInteger::from_i128(denominator),
                )
                .expect("non-zero"),
            )
        };
        let exact = |outcome: Result<super::Outcome, super::DivisionByZero>| match outcome {
            Ok(super::Outcome::Integer(value)) => Rational::from_integer(value),
            Ok(super::Outcome::Rational(value)) => value,
            _ => panic!("an exact outcome"),
        };
        use super::{compute, Arithmetic};
        let minus_seven_halves = fraction(-7, 2);
        assert_eq!(exact(compute(Arithmetic::FloorDivide, &minus_seven_halves, &integer(1))),
            Rational::from_integer(ArbitraryInteger::from_i64(-4)));
        assert_eq!(exact(compute(Arithmetic::Modulo, &minus_seven_halves, &integer(1))),
            Rational::new(ArbitraryInteger::from_i64(1), ArbitraryInteger::from_i64(2)).expect("non-zero"));
        assert_eq!(exact(compute(Arithmetic::Quotient, &minus_seven_halves, &integer(1))),
            Rational::from_integer(ArbitraryInteger::from_i64(-3)));
        assert_eq!(exact(compute(Arithmetic::Remainder, &minus_seven_halves, &integer(1))),
            Rational::new(ArbitraryInteger::from_i64(-1), ArbitraryInteger::from_i64(2)).expect("non-zero"));
        assert!(compute(Arithmetic::Divide, &integer(1), &integer(0)).is_err());
        assert!(compute(Arithmetic::Modulo, &Operand::Float(1.5), &Operand::Float(0.0)).is_err());
        assert!(matches!(
            compute(Arithmetic::Divide, &Operand::Float(1.0), &integer(0)),
            Ok(super::Outcome::Float(value)) if value == f64::INFINITY
        ));
    }
```

(Give `Outcome` and `DivisionByZero` `#[derive(Debug)]` if the
assertions need it; neither holds a `Value`.)

- [ ] **Step 4: `Number`'s nine rows, and the leaves' other cases**

`primitive.rs`: add `Add`, `Subtract`, `Multiply`, `Divide`,
`FloorDivide`, `Modulo`, `Quotient`, `Remainder`, `AsFloat` to
`NumberPrimitive`, all `false` in `answers_false_to_a_stranger`; rows:

| mirror selector | reported |
|---|---|
| `number:add:ifFail:` | `+` |
| `number:subtract:ifFail:` | `-` |
| `number:multiply:ifFail:` | `*` |
| `number:divide:ifFail:` | `/` |
| `number:floorDivide:ifFail:` | `//` |
| `number:modulo:ifFail:` | `\\` |
| `number:quotient:ifFail:` | `quo:` |
| `number:remainder:ifFail:` | `rem:` |
| `numberAsFloat:ifFail:` | `asFloat` |

`integer.rs`: `arithmetic`'s closure keeps its `SmallInteger`-argument
fast path (the tagged sum and the `i128` recomputation) and answers every
other argument with `tower::arithmetic(mutator, Arithmetic::…, receiver,
arguments)`; so do `FloorDivide` and `Modulo` past their `SmallInteger`
cases (a zero `SmallInteger` divisor keeps its `division_by_zero`). The
`large_argument`/`float_operand` branches go: the tower answers them.
`large_integer.rs`: `+ - * // \\` are `tower::arithmetic`. `float.rs`:
`+ - * /` keep their `Float`-argument fast path and answer every other
argument with `tower::arithmetic`; `atan:` and `raisedTo:` convert their
argument with `tower::operand(…).map(|number| number.to_f64())`, so a
`Fraction` argument converts in one rounding too. `argument_float` and
`float_operand` go if nothing uses them.

- [ ] **Step 5: `Kernel.ns`**

`Number`, after `hash`:

```newspeak
        (* Answers the sum of the receiver and `other`: exact when neither
           is a Float -- an integer when it is integral, a Fraction
           otherwise -- and a Float computed by IEEE 754 when either is,
           the other rounded to the nearest Float first.

           other: a number.

           Signals an ArgumentError carrying `other`, '+ requires a numeric
           argument, got …', for any other argument. *)
        public + other = ( ^vmMirror number: self add: other ifFail: [^super + other] )
        (* As `+`, the difference; its error text names '-'. *)
        public - other = ( ^vmMirror number: self subtract: other ifFail: [^super - other] )
        (* As `+`, the product; its error text names '*'. *)
        public * other = ( ^vmMirror number: self multiply: other ifFail: [^super * other] )
        (* As `+`, the quotient: exact when neither is a Float, so `7 / 2`
           is the Fraction (7/2) and `6 / 3` the Integer 2; a Float when
           either is, an infinity or a NaN for a zero divisor.

           other: a number; not an exact zero when neither is a Float.

           Signals an ArgumentError: 'division by zero', carrying `other`,
           for an exact zero `other` when neither is a Float, and '/
           requires a numeric argument, got …', carrying `other`, for any
           other argument. *)
        public / other = ( ^vmMirror number: self divide: other ifFail: [^super / other] )
        (* Answers the quotient of the receiver by `other` rounded toward
           negative infinity: an integer when neither is a Float, a Float
           when either is.

           other: a number other than zero, 0.0 included.

           Signals an ArgumentError: 'division by zero', carrying `other`,
           for a zero `other`, and '// requires a numeric argument, got …',
           carrying `other`, for any other argument. *)
        public // other = ( ^vmMirror number: self floorDivide: other ifFail: [^super // other] )
        (* Answers the remainder of `//`, `self - (self // other * other)`,
           which takes the sign of `other`: exact when neither is a Float,
           a Float when either is.

           other: as `//`'s, with the same errors; the type error's text
           names '\\'. *)
        public \\ other = ( ^vmMirror number: self modulo: other ifFail: [^super \\ other] )
        (* Answers the quotient of the receiver by `other` rounded toward
           zero: an integer when neither is a Float, a Float when either
           is.

           other: as `//`'s, with the same errors; the type error's text
           names 'quo:'. *)
        public quo: other = ( ^vmMirror number: self quotient: other ifFail: [^super quo: other] )
        (* Answers the remainder of `quo:`, `self - ((self quo: other) *
           other)`, which takes the sign of the receiver: exact when
           neither is a Float, a Float when either is.

           other: as `//`'s, with the same errors; the type error's text
           names 'rem:'. *)
        public rem: other = ( ^vmMirror number: self remainder: other ifFail: [^super rem: other] )
        (* Answers the nearest Float to the receiver's exact value, a tie
           rounded to the even one: an infinity beyond the largest Float,
           a zero below half the smallest. *)
        public asFloat = ( ^vmMirror numberAsFloat: self ifFail: [^super asFloat] )
        (* Answers `0 - self`, exactly. *)
        public negated = ( ^0 - self )
        (* Answers the receiver without its sign: `negated` when it is
           below 0, the receiver otherwise. *)
        public abs = ( ^self < 0 ifTrue: [self negated] ifFalse: [self] )
        (* Answers the receiver raised to `exponent`: exactly by repeated
           squaring for an integer `exponent` -- a negative one answering
           the reciprocal, a Fraction for an integer receiver -- and
           through `asFloat` and `Float>>raisedTo:` for any other.

           exponent: a number.

           Signals the ArgumentError of `/`, 'division by zero', for a
           zero receiver and a negative integer `exponent`, and
           `Float>>raisedTo:`'s for an `exponent` that is no number. *)
        public raisedTo: exponent = (
            | result base remaining |
            exponent isKindOfInteger ifFalse: [^self asFloat raisedTo: exponent].
            exponent < 0 ifTrue: [^1 / (self raisedTo: exponent negated)].
            result:: 1.
            base:: self.
            remaining:: exponent.
            [remaining > 0] whileTrue: [
                (remaining bitAnd: 1) = 1 ifTrue: [result:: result * base].
                remaining:: remaining >> 1.
                remaining > 0 ifTrue: [base:: base * base]].
            ^result
        )
```

`Fraction`, after `denominator`:

```newspeak
        (* Answers the greatest integer not above the receiver. *)
        public floor = ( ^self numerator // self denominator )
        (* Answers the least integer not below the receiver. *)
        public ceiling = ( ^(self numerator negated // self denominator) negated )
        (* Answers the receiver without its fraction, an integer toward
           zero. *)
        public truncated = ( ^self numerator quo: self denominator )
        (* Answers `truncated`. *)
        public asInteger = ( ^self truncated )
        (* Answers the integer nearest the receiver, a half rounded away
           from zero. *)
        public rounded = (
            | nearest |
            nearest:: (self numerator abs * 2 + self denominator) // (self denominator * 2).
            ^self numerator < 0 ifTrue: [nearest negated] ifFalse: [nearest]
        )
```

The doc comments of `Integer>>+` (and `-`, `*`, `//`, `\\`),
`LargeInteger>>+` and its siblings, and `Float>>+` (and `-`, `*`, `/`)
name what they now take ("other: a number") and answer ("a Fraction for a
Fraction `other`"), and `Integer>>//`'s and `\\`'s type error says
"requires a numeric argument". `Integer`, `LargeInteger` and `Float`
keep their own rows for `+ - * //` (and `Float` for `/`) because the
fast paths are theirs; `/`, `quo:` and `rem:` on an integer and `//`,
`\\`, `quo:`, `rem:` on a `Float` are `Number`'s.

- [ ] **Step 6: Run the new goldens**

Run: `cargo nextest run -p ns-cli --test run_goldens arithmetic_is_exact_without_a_float a_fraction_rounds_and_converts`
Expected: PASS.

- [ ] **Step 7: Every changed `/`, `//` and `\\`**

`cargo nextest run --workspace` and `run-corpus`: every other failure
must be `/` on an integer now answering where it ended in
`doesNotUnderstand:`, `//` or `\\` now taking a `Float` or a `Fraction`
argument where it was a type error ("// requires an integer argument"
becomes "… a numeric argument"), or a division by zero now carrying the
divisor. Check each by decisions 5 to 7, update it and any comment
stating the old rule, and list them in the commit message. The Are We
Fast Yet goldens (`goldens/tool/awfy/`) must not change: a benchmark that
divided integers with `/` would have failed before.

- [ ] **Step 8: The module note**

`builtins/numbers/mod.rs`'s note: delete Task 7's clause that the
arithmetic across a `Fraction` is still to come; say that the tower
computes every pair's arithmetic and the leaves keep only their fast
paths.

- [ ] **Step 9: Run the gates, then commit**

```bash
git add -A crates/ns-runtime goldens corpus
git commit -m "feat: exact arithmetic across the tower, Integer>>/ answering a Fraction, and Fraction's protocol"
```
(the body lists the goldens whose output changed; the two trailer lines).

---

### Task 9: The measurement against `master`

**Files:**
- Create: `.temp/measurements/2026-10-11-numbers/` (raw output only;
  `.temp/` is not committed)
- Modify: `docs/design/core-library.md` (one sentence with the result, in
  the section "The numeric tower" Task 10 writes -- or, if Task 10 has not
  run, in "Large integers", from where Task 10 moves it)

**Interfaces:**
- Consumes: Tasks 1 to 8.
- Produces: the numbers the controller reports to the user.

- [ ] **Step 1: Follow the `measuring-performance` skill**

Invoke the skill and follow it exactly: release builds of `master` and of
this branch, the Are We Fast Yet benchmarks (the fourteen and Ton80's
Richards and DeltaBlue, `ns-next bench` as the skill says), the number of
runs and the comparison it prescribes. Never a debug build.

- [ ] **Step 2: Judge and report**

The criterion (spec, "Rust structure"): the integer and float paths are
not measurably slower. Report per benchmark the two medians and the
ratio, in the format the skill gives.

- If no benchmark is measurably slower: add one sentence to
  `docs/design/core-library.md` ("Measured on 2026-10-…: with the tower,
  the Are We Fast Yet benchmarks run as fast as before", with the largest
  ratio), commit `docs: the numeric tower measured against master`, and
  go on.
- If any is measurably slower: **stop**. Do not change code to recover it.
  Report the numbers to the controller, who takes them to the user, with
  the benchmarks that compute most with numbers (Mandelbrot and NBody with
  floats; Permute, Queens, Sieve, Towers and Bounce with small integers;
  CD with both) and the candidates: an integer leaf's slow path now
  classifies its argument through `tower::operand` where it read a
  `LargeInteger` or a `Float` directly; `Float>>=` with a non-`Float`
  argument goes through `Rational`; `Float>>hash` builds a `Rational`
  where it hashed the bits.

---

### Task 10: The living documents

**Files:**
- Modify: `docs/design/core-library.md`, `docs/design/object-model.md`
  (where it says a number's `=` is class-tight or lists the builtin
  classes), `docs/design/image-format.md` (Task 6 did the tags; check
  the rest), `docs/OPEN-WORK.md`, `docs/PROGRESS.md`,
  `docs/design/README.md` (if its map or its "how much exists" line names
  the numbers)
- Delete: `docs/superpowers/specs/2026-10-11-kernel-numbers-design.md`

**Interfaces:**
- Consumes: everything above.
- Produces: documentation from which a fresh session can start.

- [ ] **Step 1: `core-library.md`**

- **"Gap: an integer has no `/`"** is no gap any more: replace the
  section with **"The numeric tower"**, which says what holds now --
  `Number` and its four representations; `Fraction` a builtin made only
  by the runtime in normal form, two hidden fields, carried inline in an
  image; exact comparison and why (the transitivity argument at 2^53);
  the numeric hash modulo 2^61 - 1 and why an integer's stays itself;
  arithmetic's result types and the one rounding of a fraction to a
  double; division by zero; `//`, `\\`, `quo:`, `rem:` over every pair;
  `raisedTo:`; the double-dispatch selectors left out and why; a value's
  derived equality comparing two numbers by the tower; `tower.rs` as the
  one place of the rules and the `number:` rows; the measurement of
  Task 9. Keep the reasons the old section gave that still hold (that the
  specification fixes no arithmetic protocol, §3.1.1 and §5.1.1).
- **"Large integers"**: delete the sentences on the class-tight `=` ("With
  one representation per integer, the class-tight `=` loses nothing"),
  on a `Float`'s leaves converting through `double` for comparison, on
  `//` and `\\` taking integers only, and the pointer to "Gap: …"; say
  instead that `LargeInteger`'s hash is the numeric hash and that its
  comparisons go by exact value.
- **"A NaN equals a NaN"**: "Planned: `isNaN`" is built -- rewrite it to
  say `Float>>isNaN` is a leaf over `f64::is_nan` and `Number>>isNaN`
  answers `false`, keeping the reasons a NaN is no class of its own; its
  sentence on Primordial Soup's `isNaN` stays as the reason.
- **"Deviation: a non-integral literal is a binary float"** stays; add
  that `Fraction` now exists, so `(1/10) + (2/10) = (3/10)` is exact while
  `0.1 + 0.2 = 0.3` is not.
- **"Planned: the kernel completed to Primordial Soup's core API"**:
  sub-project 4 is built -- "4. **Numbers** -- built (2026-10-…, [its
  plan](../superpowers/plans/2026-10-11-1500-ns-next-kernel-numbers.md)):
  …" in the style of 1 to 3 -- and the decision bullet "A numeric tower,
  symmetric" says it is built and points at "The numeric tower".
- **"Decisions"**: one entry, dated 2026-10-11, for the spec's ten
  decisions and two additions, each with its reason in a sentence: exact
  comparison; Python's numeric hash; `Fraction` a builtin with a Rust
  representation (on its merits, not its cost); `Integer>>isKindOfFraction`;
  result types; division by zero; `//` and kin over every pair; exact
  `raisedTo:`; no double-dispatch selectors, `ArgumentError` for a
  non-number; Primordial Soup's float texts; `truncated` on every number;
  `isNaN`, `isFinite`, `isInfinite` on `Number`. Name that `num-rational`
  was the model and no code of it was taken.
- Read the whole numbers text once and remove every sentence that still
  describes the old state ("class-tight", "until the numeric tower",
  "`7 / 2` ends in `doesNotUnderstand:`", "inf").

- [ ] **Step 2: `OPEN-WORK.md` and `PROGRESS.md`**

- `OPEN-WORK.md`, "Next": sub-project 4 is built; the next piece of work
  is sub-project 5, `ByteArray` and `StringBuilder` with the UTF-8
  conversions and the typed arrays over a `ByteArray` (decided with the
  user on 2026-10-11), which has no design or plan yet. Remove any item
  this sub-project closed (search for "tower", "Fraction", "isNaN",
  "class-tight", "Integer>>/"). Add one item for what it left open, if
  anything did (for instance a review's deferred finding).
- `PROGRESS.md`: an entry for the sub-project in the style of its
  neighbours: what was built, the decisions, the measurement's result.

- [ ] **Step 3: Delete the spec**

`git rm docs/superpowers/specs/2026-10-11-kernel-numbers-design.md`;
`grep -rn "2026-10-11-kernel-numbers-design" docs CLAUDE.md` answers only
this plan.

- [ ] **Step 4: Commit**

Only Markdown changed (`git diff --stat`), so no gates run.
Commit: `docs: the kernel's numbers in the living documents`.

After this task the controller runs the end-of-batch gates (the
`finishing-a-batch` skill).
