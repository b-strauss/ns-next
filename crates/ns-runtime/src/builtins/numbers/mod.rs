//! `Integer` (`i64`), `LargeInteger` (`ArbitraryInteger`), `Float`
//! (`f64`) and `Fraction` (`numeric::Rational`) native methods.
//!
//! # One representation per number; the tower computes across them
//!
//! §5.1.1's integers are not limited in size. Each integer has exactly one
//! representation: a `SmallInteger` (class `Integer`) when it fits 61 bits
//! (`SMALL_INTEGER_MIN` to `SMALL_INTEGER_MAX`), otherwise a `LargeInteger`
//! object, and every leaf answers an integer through `Mutator::new_integer`,
//! which picks the one that fits. A `Fraction` is made only in normal form
//! (`fraction`), so it is never an integer and equal fractions have equal
//! parts; an exact answer is made through `Mutator::new_rational`, which
//! answers an integer when the denominator is 1. Across the numeric
//! classes:
//!
//! * The tower (`tower`) computes every pair's arithmetic: with no `Float`
//!   among the operands exactly (`7 / 2` is `(7/2)`, `(1/2) + (1/2)` is
//!   `1`), with one the other rounded once to the nearest double and IEEE
//!   754 computing (`1.5 + 1` is `2.5`, `4 * 3.5` is `14.0`). `//` and
//!   `\\` round toward negative infinity, `quo:` and `rem:` toward zero,
//!   over every pair; the four, and an exact `/`, refuse a zero divisor --
//!   `0.0` included for the four -- with "division by zero", where a
//!   `Float`'s `/` answers an infinity or a NaN.
//! * The tower compares any two numbers by their exact values, a finite
//!   double being the dyadic rational it is, and hashes them so that equal
//!   numbers hash alike: `1 = 1.0`, `(1/2) = 0.5`, and 2^53 + 1 is greater
//!   than the double 2^53. A NaN is ordered with nothing and equals a NaN.
//!   `=` with anything that is no number is plain `false`, never an error;
//!   an ordering or an arithmetic operation with it is the type mismatch.
//! * `Integer`'s, `LargeInteger`'s and `Float`'s own leaves keep only their
//!   fast paths -- two `SmallInteger`s; a `Float` with a `Float`, or with
//!   a `SmallInteger` computed and compared as the tower does it
//!   (`tower::arithmetic_double`, `tower::ordering_double`) -- and send
//!   every other case to the tower, as `Number`'s rows, which `Fraction`
//!   inherits, send all of theirs. The bit operations, `gcd:` and `>>`
//!   take integers only (`integer_operand`).

mod float;
mod fraction;
mod integer;
mod large_integer;
pub(crate) mod tower;

pub(crate) use float::execute_float;
pub(crate) use fraction::execute_fraction;
pub(crate) use integer::{execute_any_integer, execute_integer};
pub(crate) use large_integer::execute_large_integer;
pub(crate) use tower::execute_number;

use crate::builtins::first_argument;
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

/// What a numeric leaf takes as its argument, which its type mismatch names.
#[derive(Clone, Copy)]
pub(super) enum Accepted {
    /// The bit operations, `gcd:` and `>>`: an `Integer` or a `LargeInteger`.
    Integer,
    /// Arithmetic and ordering: any number.
    Number,
}

/// A numeric operation on an argument it does not accept (this module's
/// note says which those are). The message names what the operation takes,
/// not what the receiver is: an integer's `+` takes any number, its
/// `bitAnd:` only an integer.
pub(super) fn type_mismatch<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    accepted: Accepted,
    got: Option<Value<'gc>>,
) -> NewspeakError<'gc> {
    let required = match accepted {
        Accepted::Integer => "an integer",
        Accepted::Number => "a numeric",
    };
    NewspeakError::argument_error(
        selector,
        receiver,
        got.unwrap_or(Value::nil()),
        format!(
            "{selector} requires {required} argument, got {}",
            heap.describe_argument(got)
        ),
    )
}

/// `//` on two `i64`s, rounded toward negative infinity (Primordial Soup's
/// `Integer>>//`; `num-integer`'s `div_floor`). `None` for a zero divisor
/// and for `i64::MIN // -1`, whose quotient does not fit `i64`. A quotient
/// it answers can still lie beyond a `SmallInteger` (`SMALL_INTEGER_MIN //
/// -1`), so its callers test the range: the leaf answers such a quotient
/// with a `LargeInteger` and raises for a zero divisor, and `IntegerBinary`
/// falls back to the send for both, so this is the only copy of the rule.
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

/// An integer operation's result that does not fit a `SmallInteger`, as the
/// `LargeInteger` §5.1.1 asks for: "integers are not limited in size".
pub(super) fn promote<'gc>(mutator: &mut Mutator<'gc>, wide: i128) -> Value<'gc> {
    mutator.new_integer(ArbitraryInteger::from_i128(wide))
}

/// Division by zero, raised in the same words by every number's `/`, `//`,
/// `\\`, `quo:` and `rem:` that refuses its zero divisor, naming the
/// selector sent and carrying the divisor -- `0`, a `LargeInteger` never,
/// or `0.0` -- as the refused argument.
pub(super) fn division_by_zero<'gc>(
    receiver: Value<'gc>,
    selector: &str,
    divisor: Value<'gc>,
) -> NewspeakError<'gc> {
    NewspeakError::argument_error(
        Symbol::intern(selector),
        receiver,
        divisor,
        "division by zero",
    )
}

/// `<<` for either integer class, once the fast path of a `SmallInteger`
/// shifted by less than 64 is ruled out: Primordial Soup's primitive 23,
/// which fails for a negative count so that its method raises. A result
/// that cannot be allocated is an error, as `Array new:`'s is, never an
/// abort.
pub(super) fn shift_left<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    receiver_integer: &ArbitraryInteger,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = Symbol::intern("<<");
    let refusal = |message: String| {
        NewspeakError::argument_error(selector, receiver, first_argument(arguments), message)
    };
    let Some(count) = integer_operand(&mutator.heap, arguments.first().copied()) else {
        return Err(type_mismatch(
            &mutator.heap,
            receiver,
            selector,
            Accepted::Integer,
            arguments.first().copied(),
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
        // A count beyond `i64` (`to_i64` fails) or a result that cannot be
        // allocated: a zero receiver still shifts to zero like any other
        // count. The `u64` conversion above cannot fail, since the count is
        // known non-negative.
        None if receiver_integer.is_zero() => Ok(Value::small_integer(0)),
        None => Err(refusal(format!(
            "<< cannot allocate the result of shifting by {} bits",
            count.to_bounded_text()
        ))),
    }
}

/// An integer operand of either representation, for the slow path: a
/// `SmallInteger` widened, or a `LargeInteger`'s payload. `None` for
/// anything else, which each leaf turns into its own type mismatch.
pub(super) fn integer_operand<'gc>(
    heap: &Heap<'gc>,
    value: Option<Value<'gc>>,
) -> Option<ArbitraryInteger> {
    match value?.kind() {
        ValueKind::SmallInteger(integer) => Some(ArbitraryInteger::from_i64(integer)),
        ValueKind::Object(object) => heap.large_integer(object),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::checked_floor_divide;
    use crate::SMALL_INTEGER_MIN;

    #[test]
    fn floor_division_rounds_toward_negative_infinity() {
        assert_eq!(checked_floor_divide(7, 2), Some(3));
        assert_eq!(checked_floor_divide(7, -2), Some(-4));
        assert_eq!(checked_floor_divide(-7, 2), Some(-4));
        assert_eq!(checked_floor_divide(-7, -2), Some(3));
        assert_eq!(checked_floor_divide(6, -2), Some(-3));
        assert_eq!(checked_floor_divide(i64::MIN, 1), Some(i64::MIN));
        assert_eq!(checked_floor_divide(i64::MIN, -1), None);
        // An `i64` quotient beyond a `SmallInteger`: the helper answers it,
        // and the leaf's and `IntegerBinary`'s range test declines it.
        assert_eq!(
            checked_floor_divide(SMALL_INTEGER_MIN, -1),
            Some(1_152_921_504_606_846_976)
        );
        assert_eq!(checked_floor_divide(1, 0), None);
    }

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
}
