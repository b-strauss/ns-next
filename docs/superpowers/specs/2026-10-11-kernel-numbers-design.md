# The kernel's numbers — design

The fourth of the kernel completion's six sub-projects
(`docs/design/core-library.md`, "Planned: the kernel completed to
Primordial Soup's core API"): Primordial Soup's whole public numbers API --
`Float`'s mathematics, `Integer`'s and `Number`'s protocol, `Fraction` --
and a symmetric numeric tower. Decided with the user on 2026-10-11.

This file is temporary: its content moves into the living documents
(`core-library.md` above all) in the batch's documentation task, and the
file is deleted there.

## Where it starts

An inventory taken on 2026-10-11 of Primordial Soup's public selectors on
`Number`, `Integer`, `Float` and `Fraction`
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`, lines
1033-2126) counted 126: 26 present here, 15 in part, 85 missing, every one
of `Fraction`'s among them.

What Primordial Soup does across types, and where this design departs:

- Its primitives compute `Integer` with `Float` through `double`, and on a
  primitive's failure it double-dispatches (`addFromNumber:`,
  `addFromFraction:`, ...); its comparisons test `isKindOfFraction` and
  `isKindOfFloat`.
- Its tower is not symmetric: `(1/2) = 0.5` is `false`; `Float>>hash` is
  `asInteger`, so `1.5 hash = 1 hash` and a NaN's `hash` signals;
  `Fraction>>hash` is `numerator bitXor: denominator`, so `1/2` and `0.5`
  hash apart; a large integer compares with a `Float` through a lossy
  conversion. The user's decision of 2026-10-06 asks for the symmetric
  tower instead.
- `(1/2) / 0` answers a fraction with denominator 0 there; here it signals.

## Decisions

1. **Mixed comparison is exact.** `=` and `< <= > >=` compare any two of
   `Integer`, `LargeInteger`, `Fraction` and `Float` by their exact value:
   a finite `double` is an exact rational, compared as one. Through
   `double` (Primordial Soup's way) `=` is no equivalence --
   `2^53 = 2^53 asFloat` and `(2^53 + 1) = 2^53 asFloat` would both be
   true -- and no `hash` could serve it. ±Infinity lies beyond every
   rational. Every comparison with a NaN answers `false` except NaN `=`
   NaN ("A NaN equals a NaN" stands). `=` with a non-number answers
   `false`; an ordering with one signals `ArgumentError`.

2. **The hash is Python's numeric hash.** Every number hashes as its exact
   rational value `n/d` modulo the Mersenne prime P = 2^61 - 1, as
   `n * d^-1 mod P`, signed as `n` is; the residue is folded into the
   `SmallInteger` range by a fixed function that is the identity on it.
   Every `SmallInteger` lies inside (-P, P), so its residue is itself and
   the fold leaves it alone: `Integer>>hash` stays `^self`. `LargeInteger`, `Float` and `Fraction`
   get new hashes; +Infinity, -Infinity and NaN one constant each; `0.0`
   and `-0.0` hash as `0`. Equal numbers hash alike whatever their
   representation, because reduction modulo P respects sums, products and
   cancellation. The derived hash of a value object hashes a number slot
   by the same function, and its derived equality compares number slots by
   decision 1, so a value with slot `3` equals and hashes like one with
   slot `3.0`.

3. **`Fraction` is a builtin with a representation of its own**, as
   `LargeInteger` is, not a Newspeak class with slots: the tower's rules --
   exact comparison, the hash, promotion, normalization -- then live in one
   Rust module for all four representations, every arithmetic and
   comparison leaf accepts every number and declines only a non-number, a
   fraction is normal by construction, and a mixed operation costs no
   Newspeak fallback sends. It was chosen on its merits, not its cost.
   - A subclass of `Number`, builtin class tag **39**, reached as
     `platform kernel Fraction`.
   - Its object holds two words, numerator and denominator, each an
     integer value (an immediate `Integer` or a `LargeInteger` handle);
     they are no Newspeak slots, nothing writes them after creation, and
     both are pointer words to the collector (`pointer_ranges`, `Trace`).
   - **The normal form**: reduced by their gcd, denominator > 1, the sign
     on the numerator. One Rust constructor (`Mutator::new_fraction` or
     the like) makes every fraction: it reduces, normalizes the sign,
     signals `ArgumentError` for a zero denominator, and answers an
     `Integer` when the denominator becomes 1.
   - `Fraction numerator:denominator:` is the public factory over it.
     `Fraction reducedNumerator:denominator:` keeps Primordial Soup's
     contract (the caller supplies the normal form) but checks it, and
     signals `ArgumentError` for an argument pair not in normal form rather
     than building a fraction that breaks `=` and `hash`.
   - `Fraction new` and an instance of a subclass hold no representation
     and are no number: every leaf declines them on the missing payload,
     as for `LargeInteger`.
   - `Heap::describe` describes a fraction as `(3/4)`.
   - **The image** carries a fraction inline as a value, as a
     `LargeInteger`: value tag **10** (`VALUE_FRACTION`), then the
     numerator and the denominator as integer values. The decoder refuses
     anything not in normal form -- either part not an integer, a
     denominator below 2, a common factor -- since an image is untrusted
     input. `tests/opcode_tags.rs` pins both tags.
   - `num-rational`'s `Ratio` is the model for the arithmetic, the
     rounding and the comparison, but no code is copied from it: its
     licence is not taken on (decided with the user on 2026-10-11).

4. **`Integer` answers `isKindOfFraction` with `true`**, as in Primordial
   Soup, with `numerator` `^self` and `denominator` `^1`: an integer is a
   fraction with denominator 1.

5. **Arithmetic's result type.** For `+ - * /`:
   - integer with integer: exact; `/` answers an `Integer` when the
     division is exact, a `Fraction` otherwise;
   - an integer or a fraction with a fraction: exact, normalized (an
     `Integer` when the denominator becomes 1);
   - anything with a `Float`: a `Float`, the rational side rounded to the
     nearest `double` in one correctly rounded step. Primordial Soup's
     `numerator asFloat / denominator asFloat` rounds twice and answers NaN
     once both parts pass `f64::MAX`; that is not followed.

6. **Division by zero.** An exact division by zero -- an integer or
   fraction receiver, an integer or fraction divisor -- signals
   `ArgumentError`. A `Float` division stays IEEE's: `1.0 / 0` and
   `1 / 0.0` answer `Infinity`.

7. **`//`, `\\`, `quo:` and `rem:` take every pair of numbers**, as
   Smalltalk-80's do, where Primordial Soup's primitives take integers
   (and, for `//`, floats) only: `a // b` is `(a / b) floor`, `a \\ b` is
   `a - (a // b * b)`, `quo:` and `rem:` their truncating counterparts.
   Integer operands answer integers, fractions exact results, and a
   `Float` among them a `Float`, as Primordial Soup's `Float` `//` does. A
   zero divisor signals `ArgumentError`, a `Float` one included, as
   Primordial Soup's `//` does.

8. **`raisedTo:`**: an integer exponent computes exactly, a negative one
   answering the reciprocal (a `Fraction` for an integer base);
   `0 raisedTo: -1` signals `ArgumentError`. Any other exponent computes
   through `Float`.

9. **No double-dispatch selectors.** Primordial Soup's sixteen
   `addFromNumber:`, `subtractFromNumber:`, `multiplyFromNumber:`,
   `divideFromNumber:` and `...FromFraction:` selectors are its mechanism,
   public only for being that; here the leaves compute every combination,
   and the selectors are left out. A non-number argument to arithmetic and
   to an ordering signals `ArgumentError` alike, as today, where Primordial
   Soup's arithmetic ends in `doesNotUnderstand: #addFromNumber:`.

10. **`Float` prints as Primordial Soup's does** (`testFloatAsString`,
    `KernelTests.ns:1815`): the shortest text that reads back as the same
    `double`, with `.0` when it would look integral (`3.0`, `-0.0`); the
    exponent form outside Primordial Soup's decimal range (`1e+21`,
    `1.7976931348623157e+308`); `Infinity`, `-Infinity` and `NaN`. Rust
    finds the shortest digits; only the layout around them is new.
    `asStringFixed:` outside ±1e21 and for the non-finite values,
    `asStringExponential:` (0..20 digits) and `asStringPrecision:` (1..21)
    follow the same texts, and `Float parse:` reads exactly what
    `printString` writes, `NaN` and `Infinity` included, refusing `nan`,
    `inf`, an empty string and `--9` with `ArgumentError`. The goldens
    whose float output changes are updated, each with the reason the new
    text is right.

## The protocol

Primordial Soup's selectors, with its meaning and the rules above.

- **`Number`**: `/`, `//`, `\\`, `quo:`, `rem:`, `raisedTo:`; `abs` and
  `negated` for every number; `between:and:`, `min:`; the mathematics
  `sqrt sin cos tan asin acos atan atan: exp ln log` as `^self asFloat
  ...`, so every number answers them; `asInteger`, `asFloat` and
  `printString` for every number.
- **`Integer`**: `gcd:` (a leaf), `raisedTo:modulo:`, `reciprocalModulo:`;
  `>>` (arithmetic shift), `&`, `|`, `digitAt:`, `digitLength`; `floor`,
  `ceiling`, `rounded`, `truncated` and `asInteger`, each `^self`;
  `numerator`, `denominator`, `isKindOfFraction`; `asStringRadix:` (2 to
  36, upper-case digits); class-side `Integer parse:` and
  `Integer parse:radix:`.
- **`Float`**: a leaf for each mathematical function, `atan:` and
  `raisedTo:`; `floor`, `ceiling`, `rounded` (half away from zero) and
  `truncated`, each answering a **`Float`** as Primordial Soup's do, so
  `Infinity floor` and `1e300 floor` answer without an error or a huge
  `LargeInteger`, and `asInteger` asks for an integer; `asFloat`;
  `asStringExponential:`, `asStringPrecision:`; `isFinite`, `isInfinite`,
  `isNaN`; class-side `Float parse:`.
- **`Fraction`**: `numerator`, `denominator`, the arithmetic and the
  comparisons; `asFloat` (correctly rounded); `asInteger` and `truncated`
  (towards zero), `floor`, `ceiling`, `rounded` (half away from zero), each
  answering an `Integer`; `printString` and `asString` as `(3/4)`,
  `(-3/4)`; `isKindOfFraction`.

Two additions beyond Primordial Soup's API:

- `truncated` on `Integer` (`^self`) and `Float` (a `Float`), where
  Primordial Soup has it on `Fraction` only while all three answer
  `floor`, `ceiling` and `rounded`.
- `Number>>isNaN`, `isFinite` and `isInfinite` answer `false`, `true` and
  `false`, so every number answers the questions `Float` overrides; the
  `Number>>isNaN` part was planned in "A NaN equals a NaN" already.

## Rust structure

- **`arbitrary_integer/`** gains what the protocol needs, written here:
  `gcd`, truncating division and remainder, a public arithmetic right
  shift, base-256 digits, printing in radix 2 to 36, and the residue
  modulo 2^61 - 1.
- **`builtins/numbers.rs` is split** into `builtins/numbers/`:
  `integer.rs`, `large_integer.rs`, `float.rs`, `fraction.rs` and
  `tower.rs`, one file per representation and one for the rules they
  share.
- **`tower.rs`** holds the tower's rules once: a number operand that
  classifies an argument a single time (small integer, large integer,
  fraction, float); the exact comparison of any two (a finite `double`
  split exactly into mantissa times a power of two and compared in
  integers); the numeric hash; the correctly rounded conversion of a
  fraction to `double`; and the fraction constructor.
- **The leaves**: every arithmetic and comparison leaf accepts every
  number and declines only a non-number. The fast paths stay as they are:
  `SmallInteger` with `SmallInteger`, `Float` with `Float`, and the
  inlined `IntegerBinary`; the new cases sit where the type mismatch sits
  today. `Fraction` gets `fraction:` rows in `MIRROR`.
- **Value objects**: `builtins/value.rs`'s derived equality and hash take
  number slots through `tower`.
- **Inline caches** do not change: `Fraction` is a class of its own with
  methods of its own, and its leaves check the representation on every
  send, as `LargeInteger`'s do.
- **Performance**: the Are We Fast Yet benchmarks are measured in a
  release build before and after the tower (the `measuring-performance`
  skill); the integer and float paths must not get slower.

## Testing

- **Goldens** under `goldens/library/number/`, one per theme --
  `fraction/`, `tower/` (mixed arithmetic, exact comparison, hash),
  `float/` (mathematics, rounding, printing, parsing), `integer/` (`gcd:`,
  bits, radix, parsing) -- under both lowerings. Expected values come from
  Primordial Soup's `KernelTests.ns` (`IntegerTests`, `FloatTests`,
  `FractionTests`) where its behaviour is adopted, and from the decisions
  above where this design departs.
- **Changed goldens**: `1 = 1.0` now answers `true`, floats print
  differently, `7 / 2` answers a number. Each changed golden is updated
  with the reason its new output is right, never because the code now
  produces it.
- **Rust tests**: property tests over pairs of `Integer`, `LargeInteger`,
  `Fraction` and `Float` -- `a = b` implies `a hash = b hash`, `=` is
  transitive, `<` agrees with `=` -- with pairs chosen around 2^53, 2^60,
  2^61, ±Infinity, NaN and `-0.0`; the rounding of a fraction to `double`;
  an image round trip of a fraction, and the decoder's refusal of each
  non-normal form (denominator 0, 1 or negative, a common factor).
- **Stress modes**: fractions are objects with pointer words; the five
  stress modes at the batch's end cover the new format, the scavenges and
  the moves above all.
- **Documentation**: every new member of `Kernel.ns` documented to
  the standard `core_library_documentation.rs` holds.

## Order of work

One plan, six phases, each leaving the goldens green:

1. Split `builtins/numbers.rs` into `builtins/numbers/`, no behaviour
   changed.
2. `Float`: the mathematical leaves, `atan:`, `raisedTo:`, the rounding
   selectors, `abs`, `negated`, `asFloat`, `isNaN`, `isFinite`,
   `isInfinite` and their `Number` counterparts; the new printing,
   `asStringExponential:`, `asStringPrecision:`, `Float parse:`; the
   goldens whose float output changes.
3. `Integer`'s and `Number`'s protocol without fractions: `gcd:`, `quo:`,
   `rem:`, `>>`, `&`, `|`, `digitAt:`, `digitLength`, `asStringRadix:`,
   `Integer parse:` and `parse:radix:`, `raisedTo:modulo:`,
   `reciprocalModulo:`, the rounding selectors, `between:and:`, `min:`,
   the mathematics through `asFloat`, `//` on `Float`.
4. `Fraction`'s representation: the object format, `pointer_ranges`,
   `Trace`, the constructor and the normal form, `Heap::describe`, the
   builtin class tag, `platform kernel Fraction`, the image value tag and
   its checks.
5. The tower: `Integer>>/`, mixed arithmetic of every pair, exact
   comparison, the numeric hash (the new `LargeInteger` hash and the value
   walk with it), `//`, `\\`, `quo:`, `rem:` and `raisedTo:` over every
   number, `Fraction`'s whole protocol, the property tests, the goldens
   whose `=` changes.
6. The measurement and the documentation: Are We Fast Yet before and after
   in a release build; `core-library.md` ("Gap: an integer has no `/`",
   the class-tight `=`, "A NaN equals a NaN" with `isNaN` built, the
   decisions), the numbers module's note, `PROGRESS.md`, `OPEN-WORK.md`;
   this file deleted.

Kept as recorded deviations, unchanged: a non-integral literal stays a
binary `Float` (§5.1.1), and there is no fraction literal.
