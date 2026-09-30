//! `Integer` (`i64`), `LargeInteger` (`ArbitraryInteger`) and `Float`
//! (`f64`) native methods.
//!
//! # Integers are unbounded; the numeric tower is not there yet
//!
//! §5.1.1's integers are not limited in size. Each integer has exactly one
//! representation: a `SmallInteger` (class `Integer`) when it fits `i64`,
//! otherwise a `LargeInteger` object, and every leaf answers an integer
//! through `Mutator::new_integer`, which picks the one that fits. Across
//! the numeric classes:
//!
//! * `Integer`'s and `LargeInteger`'s arithmetic and ordering accept an
//!   argument of either integer class (`integer_operand`): `5 + big` and
//!   `big + 5` both answer.
//! * A `Float`'s arithmetic and ordering convert an `Integer` or a
//!   `LargeInteger` argument to the nearest `double` (`1.5 + 1` is `2.5`).
//! * An integer meeting a `Float` argument is a type mismatch: `1 + 1.5`
//!   raises a `NewspeakError` that names the argument. With no
//!   `generality`/`coerce:` pair there is no rule for which way such a
//!   mixed operation widens, so it fails loudly rather than guessing.
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
//! `Fraction`, a `Number` protocol and the coercion between integers and
//! floats are still to come (`docs/design/core-library.md`, "Gap: an integer
//! has no `/`").

use std::cmp::Ordering;

use crate::builtins::primitive::{
    FloatPrimitive, IntegerPrimitive, LargeIntegerPrimitive, Primitive,
};
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

/// A numeric operation on an argument it does not accept (this module's
/// note says which those are). The message names what the receiver takes:
/// an `Integer` or `LargeInteger` receiver takes either integer class, a
/// `Float` receiver any of the three number classes.
fn type_mismatch<'gc>(
    heap: &Heap<'gc>,
    receiver: &Value<'gc>,
    selector: Symbol,
    got: Option<&Value<'gc>>,
) -> NewspeakError<'gc> {
    let required = match receiver {
        Value::Float(_) => "a numeric",
        _ => "an integer",
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

/// `Integer>>=`'s rule: an argument of any other type is plainly unequal,
/// never an error (this module's note says why). Named so that
/// `builtins::value`'s derived equality decides an integer exactly as
/// `Integer>>=` does rather than growing a second copy of the rule.
pub(crate) fn integer_equals<'gc>(receiver: i64, argument: Option<&Value<'gc>>) -> bool {
    matches!(argument, Some(Value::SmallInteger(other)) if *other == receiver)
}

/// `Float>>=`'s rule, the same way, and class-tight: only a `Float` can be
/// equal to a `Float`.
///
/// It widened an `Integer` argument until 2026-09-23. That broke the one
/// rule a `hash` has -- `1.0 = 1` answered `true` while `1.0 hash` and
/// `1 hash` differ -- and it made `builtins::value`'s derived equality,
/// which asks the classes first, answer `false` for the very pair this
/// answered `true` for. One rule now, in both places. Cross-type numeric
/// equality is what the numeric tower under `Number` brings back
/// (`docs/design/core-library.md`, "Gap: an integer has no `/`"); the
/// conversion of an integer in `argument_float` below stays, because arithmetic
/// and ordering ask a magnitude question rather than an identity one.
pub(crate) fn float_equals<'gc>(receiver: f64, argument: Option<&Value<'gc>>) -> bool {
    matches!(argument, Some(Value::Float(other)) if *other == receiver)
}

/// A float's content hash: its bits, mixed and folded to 32 bits. `-0.0` is
/// normalized to `0.0` first -- the two have different bits but compare
/// equal, and a `hash` that told them apart would break the one rule a
/// `hash` has ("if `x = y` ... then `x hash = y hash`"). A `NaN` needs no
/// such care: it equals nothing, itself included, so nothing constrains its
/// hash.
///
/// Shared with `builtins::value`, whose derived hash must answer the same
/// number for a float slot as `Float>>hash` does for the float itself.
pub(crate) fn float_hash(value: f64) -> i64 {
    let normalized = if value == 0.0 { 0.0 } else { value };
    crate::hash::content_hash(normalized.to_bits())
}

/// `//` on two `i64`s, rounded toward negative infinity (Primordial Soup's
/// `Integer>>//`; `num-integer`'s `div_floor`). `None` for a zero divisor
/// and for `i64::MIN // -1`, whose quotient does not fit -- the leaf
/// answers `i64::MIN // -1` with a `LargeInteger` and raises for a zero
/// divisor, and `IntegerBinary` falls back to the send for both, so this is
/// the only copy of the rule.
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

/// An `i64` operation's result that does not fit `i64`, as the
/// `LargeInteger` §5.1.1 asks for: "integers are not limited in size".
fn promote<'gc>(mutator: &mut Mutator<'gc>, wide: i128) -> Value<'gc> {
    mutator.new_integer(ArbitraryInteger::from_i128(wide))
}

/// Division by zero, raised in the same words by `Integer`'s and
/// `LargeInteger`'s `//` and `\\`, naming the selector sent.
fn division_by_zero<'gc>(receiver: Value<'gc>, selector: &str) -> NewspeakError<'gc> {
    NewspeakError {
        selector: Symbol::intern(selector),
        receiver,
        message: "division by zero".to_string(),
    }
}

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

pub(crate) fn execute_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: IntegerPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::SmallInteger(receiver_integer) = receiver else {
        return None;
    };
    let argument_integer = || match arguments.first() {
        Some(Value::SmallInteger(argument)) => Some(*argument),
        _ => None,
    };
    let mismatch = |operator: &str| {
        type_mismatch(
            &mutator.heap,
            &receiver,
            Symbol::intern(operator),
            arguments.first(),
        )
    };
    // A `LargeInteger` argument, read only once the argument is known to be
    // no `SmallInteger`, so the common case never touches the heap.
    let large_argument = || integer_operand(&mutator.heap, arguments.first());
    // `+`/`-`/`*`: an argument of neither integer class is a type mismatch.
    // A result that fits is answered without allocating. One that does not
    // is recomputed in `i128`, where two `i64`s' sum, difference or product
    // always fits, and one with a `LargeInteger` argument as an
    // `ArbitraryInteger`; either is handed back as the inner `Err` for the
    // caller to make a `Value` of -- the closure holds `mutator` shared, so
    // it cannot allocate the `LargeInteger` itself.
    let arithmetic =
        |operator: &str,
         checked: fn(i64, i64) -> Option<i64>,
         wide: fn(i128, i128) -> i128,
         large: fn(&ArbitraryInteger, &ArbitraryInteger) -> ArbitraryInteger| {
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
                    None => Err(mismatch(operator)),
                },
            }
        };
    // Ordering comparisons: an argument of the wrong type is an error, not a
    // silent `false`.
    let compare = |operator: &str, operation: fn(Ordering) -> bool| match argument_integer() {
        Some(argument) => Ok(Value::Boolean(operation(receiver_integer.cmp(&argument)))),
        None => match large_argument() {
            Some(argument) => Ok(Value::Boolean(operation(
                ArbitraryInteger::from_i64(receiver_integer).cmp(&argument),
            ))),
            None => Err(mismatch(operator)),
        },
    };
    match primitive {
        IntegerPrimitive::Add => Some(
            arithmetic(
                "+",
                i64::checked_add,
                |left, right| left + right,
                ArbitraryInteger::add,
            )
            .map(|answer| answer.unwrap_or_else(|wide| mutator.new_integer(wide))),
        ),
        IntegerPrimitive::Subtract => Some(
            arithmetic(
                "-",
                i64::checked_sub,
                |left, right| left - right,
                ArbitraryInteger::subtract,
            )
            .map(|answer| answer.unwrap_or_else(|wide| mutator.new_integer(wide))),
        ),
        IntegerPrimitive::Multiply => Some(
            arithmetic(
                "*",
                i64::checked_mul,
                |left, right| left * right,
                ArbitraryInteger::multiply,
            )
            .map(|answer| answer.unwrap_or_else(|wide| mutator.new_integer(wide))),
        ),
        IntegerPrimitive::FloorDivide => Some(match argument_integer() {
            Some(0) => Err(division_by_zero(receiver, "//")),
            // With the zero divisor refused above, the one quotient that
            // does not fit is `i64::MIN // -1`'s, which is `-i64::MIN`.
            Some(divisor) => Ok(match checked_floor_divide(receiver_integer, divisor) {
                Some(quotient) => Value::SmallInteger(quotient),
                None => promote(mutator, -i128::from(receiver_integer)),
            }),
            // A `LargeInteger` divisor is never zero, since zero fits
            // `i64`; `floor_divide`'s `None` is still answered as the zero
            // divisor it would mean rather than assumed away.
            None => match large_argument() {
                Some(divisor) => {
                    match ArbitraryInteger::from_i64(receiver_integer).floor_divide(&divisor) {
                        Some(quotient) => Ok(mutator.new_integer(quotient)),
                        None => Err(division_by_zero(receiver, "//")),
                    }
                }
                None => Err(mismatch("//")),
            },
        }),
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
        IntegerPrimitive::Less => Some(compare("<", Ordering::is_lt)),
        IntegerPrimitive::Greater => Some(compare(">", Ordering::is_gt)),
        IntegerPrimitive::LessOrEqual => Some(compare("<=", Ordering::is_le)),
        IntegerPrimitive::GreaterOrEqual => Some(compare(">=", Ordering::is_ge)),
        // Deliberately *not* an error on a type mismatch -- see the
        // module-level note.
        IntegerPrimitive::Equal => crate::interpreter::future::declines_for_a_future(
            mutator,
            Primitive::Integer(primitive),
            integer_equals(receiver_integer, arguments.first()),
            receiver,
            arguments,
        )
        .map(|answer| Ok(Value::Boolean(answer))),
        // `i64::MIN` is the one receiver whose negation and absolute value
        // do not fit.
        IntegerPrimitive::Negated => Some(Ok(match receiver_integer.checked_neg() {
            Some(negated) => Value::SmallInteger(negated),
            None => mutator.new_integer(ArbitraryInteger::from_i64(receiver_integer).negated()),
        })),
        IntegerPrimitive::Absolute => Some(Ok(match receiver_integer.checked_abs() {
            Some(absolute) => Value::SmallInteger(absolute),
            None => mutator.new_integer(ArbitraryInteger::from_i64(receiver_integer).absolute()),
        })),
        IntegerPrimitive::PrintString => Some(Ok(Value::Object(
            mutator.new_string(receiver_integer.to_string()),
        ))),
    }
}

/// `LargeInteger>>hash`, and the derived hash of a value slot holding one:
/// the sign and magnitude's content hash, folded to 32 bits as
/// `content_hash` folds every content hash, which `builtins::value`'s sum
/// over a value's slots relies on to stay inside an `i64`.
pub(crate) fn large_integer_hash(integer: &ArbitraryInteger) -> i64 {
    crate::hash::content_hash((integer.is_negative(), integer.magnitude()))
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
        Some(Value::Object(object)) => heap
            .large_integer(*object)
            .is_some_and(|other| other == *receiver),
        _ => false,
    }
}

pub(crate) fn execute_large_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: LargeIntegerPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    // The payload, never the class: a user subclass's instance, or a
    // `LargeInteger new`, is no number and declines to `Object`'s methods.
    let Value::Object(object) = receiver else {
        return None;
    };
    let receiver_integer = mutator.heap.large_integer(object)?;
    // Either integer class as the argument; anything else is the leaf's type
    // mismatch. As in `execute_integer`, these closures hold `mutator`
    // shared, so each answers an `ArbitraryInteger` for the caller to make a
    // `Value` of.
    let argument = || integer_operand(&mutator.heap, arguments.first());
    let mismatch = |operator: &str| {
        type_mismatch(
            &mutator.heap,
            &receiver,
            Symbol::intern(operator),
            arguments.first(),
        )
    };
    let arithmetic =
        |operator: &str,
         operation: fn(&ArbitraryInteger, &ArbitraryInteger) -> ArbitraryInteger| {
            match argument() {
                Some(argument) => Ok(operation(&receiver_integer, &argument)),
                None => Err(mismatch(operator)),
            }
        };
    let compare = |operator: &str, operation: fn(Ordering) -> bool| match argument() {
        Some(argument) => Ok(Value::Boolean(operation(receiver_integer.cmp(&argument)))),
        None => Err(mismatch(operator)),
    };
    match primitive {
        LargeIntegerPrimitive::Add => {
            Some(arithmetic("+", ArbitraryInteger::add).map(|answer| mutator.new_integer(answer)))
        }
        LargeIntegerPrimitive::Subtract => Some(
            arithmetic("-", ArbitraryInteger::subtract).map(|answer| mutator.new_integer(answer)),
        ),
        LargeIntegerPrimitive::Multiply => Some(
            arithmetic("*", ArbitraryInteger::multiply).map(|answer| mutator.new_integer(answer)),
        ),
        LargeIntegerPrimitive::FloorDivide => Some(match argument() {
            Some(divisor) => match receiver_integer.floor_divide(&divisor) {
                Some(quotient) => Ok(mutator.new_integer(quotient)),
                None => Err(division_by_zero(receiver, "//")),
            },
            None => Err(mismatch("//")),
        }),
        LargeIntegerPrimitive::Modulo => Some(match argument() {
            Some(divisor) => match receiver_integer.floor_modulo(&divisor) {
                Some(remainder) => Ok(mutator.new_integer(remainder)),
                None => Err(division_by_zero(receiver, "\\\\")),
            },
            None => Err(mismatch("\\\\")),
        }),
        LargeIntegerPrimitive::Less => Some(compare("<", Ordering::is_lt)),
        LargeIntegerPrimitive::Greater => Some(compare(">", Ordering::is_gt)),
        LargeIntegerPrimitive::LessOrEqual => Some(compare("<=", Ordering::is_le)),
        LargeIntegerPrimitive::GreaterOrEqual => Some(compare(">=", Ordering::is_ge)),
        LargeIntegerPrimitive::Equal => crate::interpreter::future::declines_for_a_future(
            mutator,
            Primitive::LargeInteger(primitive),
            large_integer_equals(&mutator.heap, &receiver_integer, arguments.first()),
            receiver,
            arguments,
        )
        .map(|answer| Ok(Value::Boolean(answer))),
        LargeIntegerPrimitive::Hash => Some(Ok(Value::SmallInteger(large_integer_hash(
            &receiver_integer,
        )))),
        LargeIntegerPrimitive::PrintString => Some(Ok(Value::Object(
            mutator.new_string(receiver_integer.to_decimal_string()),
        ))),
        LargeIntegerPrimitive::Negated => Some(Ok(mutator.new_integer(receiver_integer.negated()))),
        LargeIntegerPrimitive::Absolute => {
            Some(Ok(mutator.new_integer(receiver_integer.absolute())))
        }
    }
}

pub(crate) fn execute_float<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FloatPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Float(receiver_float) = receiver else {
        return None;
    };
    // An `Integer` or `LargeInteger` argument becomes the nearest `double`;
    // anything else is a type mismatch, never a silent `NaN` result for
    // arithmetic or a silent `false` for comparisons.
    let argument_float = || match arguments.first() {
        Some(Value::Float(float)) => Some(*float),
        Some(Value::SmallInteger(integer)) => Some(*integer as f64),
        Some(Value::Object(object)) => mutator
            .heap
            .large_integer(*object)
            .map(|integer| integer.to_f64()),
        _ => None,
    };
    let mismatch = |operator: &str| {
        type_mismatch(
            &mutator.heap,
            &receiver,
            Symbol::intern(operator),
            arguments.first(),
        )
    };
    let arithmetic = |operator: &str, operation: fn(f64, f64) -> f64| match argument_float() {
        None => Err(mismatch(operator)),
        Some(argument) => Ok(Value::Float(operation(receiver_float, argument))),
    };
    let compare = |operator: &str, operation: fn(f64, f64) -> bool| match argument_float() {
        None => Err(mismatch(operator)),
        Some(argument) => Ok(Value::Boolean(operation(receiver_float, argument))),
    };
    match primitive {
        FloatPrimitive::Add => Some(arithmetic("+", |left, right| left + right)),
        FloatPrimitive::Subtract => Some(arithmetic("-", |left, right| left - right)),
        FloatPrimitive::Multiply => Some(arithmetic("*", |left, right| left * right)),
        FloatPrimitive::Divide => Some(arithmetic("/", |left, right| left / right)),
        FloatPrimitive::Less => Some(compare("<", |left, right| left < right)),
        FloatPrimitive::Greater => Some(compare(">", |left, right| left > right)),
        FloatPrimitive::LessOrEqual => Some(compare("<=", |left, right| left <= right)),
        FloatPrimitive::GreaterOrEqual => Some(compare(">=", |left, right| left >= right)),
        // Same exception as `Integer`'s `=`: never an error.
        FloatPrimitive::Equal => crate::interpreter::future::declines_for_a_future(
            mutator,
            Primitive::Float(primitive),
            float_equals(receiver_float, arguments.first()),
            receiver,
            arguments,
        )
        .map(|answer| Ok(Value::Boolean(answer))),
        FloatPrimitive::Hash => Some(Ok(Value::SmallInteger(float_hash(receiver_float)))),
        FloatPrimitive::PrintString => Some(Ok(Value::Object(
            mutator.new_string(receiver_float.to_string()),
        ))),
    }
}

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
