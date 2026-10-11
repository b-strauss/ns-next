//! `Integer` (`i64`), `LargeInteger` (`ArbitraryInteger`) and `Float`
//! (`f64`) native methods.
//!
//! # Integers are unbounded; the numeric tower is not there yet
//!
//! §5.1.1's integers are not limited in size. Each integer has exactly one
//! representation: a `SmallInteger` (class `Integer`) when it fits 61 bits
//! (`SMALL_INTEGER_MIN` to `SMALL_INTEGER_MAX`), otherwise a `LargeInteger`
//! object, and every leaf answers an integer through `Mutator::new_integer`,
//! which picks the one that fits. Across the numeric classes:
//!
//! * `Integer`'s and `LargeInteger`'s arithmetic and ordering accept an
//!   argument of either integer class (`integer_operand`): `5 + big` and
//!   `big + 5` both answer.
//! * A `Float`'s arithmetic and ordering convert an `Integer` or a
//!   `LargeInteger` argument to the nearest `double` (`1.5 + 1` is `2.5`).
//! * An integer's arithmetic and ordering meet a `Float` argument the same
//!   way, converting the receiver (`4 * 3.5` is `14.0`), as Primordial
//!   Soup's primitives do. `//`, `\\` and the bit operations take integers
//!   only; `Float` has none of them.
//! * `=` is the deliberate exception: an argument of another class makes it
//!   plain `false`, never an error -- matching `Object`'s own `=` and the
//!   usual Smalltalk convention -- and it is class-tight across `Integer`
//!   and `Float`: `1 = 1.0` and `1.0 = 1` both answer `false`, as
//!   `String>>=` and `Symbol>>=` are class-tight. `Float>>=` widened an
//!   `Integer` argument until 2026-09-23, which made `1.0 = 1` true while
//!   `1.0 hash` and `1 hash` differ -- the one rule a `hash` has. A real
//!   numeric tower would answer `true` both ways *and* hash alike; until
//!   there is one, `false` both ways keeps the `=`/`hash` contract. Between
//!   `Integer` and `LargeInteger` class-tightness loses nothing: with one
//!   representation per integer, no two of them are ever the same number.
//!
//! `Fraction`, `Integer>>/` and cross-class `=` are still to come, with the
//! numeric tower (`docs/design/core-library.md`, "Gap: an integer has no
//! `/`").

mod float;
mod integer;
mod large_integer;

pub(crate) use float::{execute_float, float_equals, float_hash};
pub(crate) use integer::{execute_integer, integer_equals};
pub(crate) use large_integer::{execute_large_integer, large_integer_equals, large_integer_hash};

use std::cmp::Ordering;

use crate::builtins::first_argument;
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

/// What a numeric leaf takes as its argument, which its type mismatch names.
#[derive(Clone, Copy)]
pub(super) enum Accepted {
    /// `//`, `\\` and the bit operations: an `Integer` or a `LargeInteger`.
    Integer,
    /// Arithmetic and ordering: any of the three number classes.
    Number,
}

/// A numeric operation on an argument it does not accept (this module's
/// note says which those are). The message names what the operation takes,
/// not what the receiver is: an integer's `+` takes any number, its `//`
/// only an integer.
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

/// Division by zero, raised in the same words by `Integer`'s and
/// `LargeInteger`'s `//` and `\\`, naming the selector sent.
pub(super) fn division_by_zero<'gc>(receiver: Value<'gc>, selector: &str) -> NewspeakError<'gc> {
    // The refused argument is the divisor, which is always the integer zero.
    NewspeakError::argument_error(
        Symbol::intern(selector),
        receiver,
        Value::small_integer(0),
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

/// A `SmallInteger` as a sign and a canonical magnitude, lent to `read`
/// from the stack, so that it compares with a `LargeInteger`'s digits read
/// in place.
pub(super) fn with_small_integer_parts<T>(
    integer: i64,
    read: impl FnOnce((bool, &[u64])) -> T,
) -> T {
    let digit = [integer.unsigned_abs()];
    // Zero's canonical magnitude is empty.
    let digits: &[u64] = if integer == 0 { &[] } else { &digit };
    read((integer < 0, digits))
}

/// The order of `receiver` -- a sign and a canonical magnitude -- against
/// an integer argument of either representation, reading a `LargeInteger`
/// argument in place; `None` for anything else, which a comparison then
/// tries as a `Float`.
pub(super) fn integer_ordering<'gc>(
    heap: &Heap<'gc>,
    receiver: (bool, &[u64]),
    value: Option<Value<'gc>>,
) -> Option<Ordering> {
    match value?.kind() {
        ValueKind::SmallInteger(integer) => Some(with_small_integer_parts(integer, |argument| {
            crate::arbitrary_integer::compare_signed(receiver, argument)
        })),
        ValueKind::Object(object) => heap.with_large_integer(object, |negative, digits| {
            crate::arbitrary_integer::compare_signed(receiver, (negative, digits))
        }),
        _ => None,
    }
}

/// A `Float` argument, which an integer's arithmetic and ordering take by
/// converting the receiver to a double (this module's note).
pub(super) fn float_operand<'gc>(heap: &Heap<'gc>, value: Option<Value<'gc>>) -> Option<f64> {
    heap.float_value(value?)
}

/// What an arithmetic closure answers. The closures hold `mutator` shared,
/// so a result that needs it -- a `LargeInteger`, or a float, which
/// `Mutator::new_float` makes -- is handed back for the caller to make a
/// `Value` of.
pub(super) enum Answer<'gc> {
    Value(Value<'gc>),
    Integer(ArbitraryInteger),
    Float(f64),
}

impl<'gc> Answer<'gc> {
    fn into_value(self, mutator: &mut Mutator<'gc>) -> Value<'gc> {
        match self {
            Answer::Value(value) => value,
            Answer::Integer(integer) => mutator.new_integer(integer),
            Answer::Float(float) => mutator.new_float(float),
        }
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
