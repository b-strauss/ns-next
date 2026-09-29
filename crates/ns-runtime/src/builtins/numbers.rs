//! `Integer` (`i64`), `LargeInteger` (`ArbitraryInteger`) and `Float`
//! (`f64`) native methods.
//!
//! # Known gap: no numeric tower
//!
//! There is no coercion between `Integer` and `Float`. There
//! is no `Number` protocol, no `generality`/`coerce:` pair, and no promotion
//! of a mixed operation to the wider type:
//!
//! * `Integer` arithmetic (`execute_integer`) requires an `Integer`
//!   argument. `1 + 1.5` does **not** produce `2.5`; it raises a
//!   type-mismatch `NewspeakError`.
//! * `Float` arithmetic (`execute_float`) is one-directional: it accepts an
//!   `Integer` argument and widens it (`1.5 + 1` is `2.5`), because that
//!   direction is lossless and free. Any other argument type raises a
//!   type-mismatch `NewspeakError`.
//! * Ordering comparisons (`<`, `>`, `<=`, `>=`) follow the same rule as
//!   arithmetic: a type-mismatched argument is an error, not an answer.
//! * `=` is the deliberate exception: a type-mismatched argument makes it
//!   plain `false` (two values of different types are not equal), never an
//!   error -- matching `Object`'s own `=` and the usual Smalltalk
//!   convention. Both directions answer `false`: `1 = 1.0` and `1.0 = 1`
//!   alike, because each `=` is class-tight (`String>>=` and `Symbol>>=`
//!   are for the same reason). `Float>>=` widened an `Integer` argument
//!   until 2026-09-23, which made `1.0 = 1` true while `1.0 hash` and
//!   `1 hash` differ -- the one rule a `hash` has -- and was asymmetric
//!   besides. A real numeric tower would answer `true` both ways *and*
//!   hash alike; until there is one, `false` both ways is the answer that
//!   keeps the `=`/`hash` contract.
//!
//! Implementing the tower means widening the whole `Value` numeric domain
//! (`Fraction` belongs in the same change) and is out of scope here. Until
//! then these operations fail loudly rather than silently, with a
//! type-mismatch message that names the argument.

use crate::builtins::primitive::{
    FloatPrimitive, IntegerPrimitive, LargeIntegerPrimitive, Primitive,
};
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

/// A mixed-type numeric operation (see this module's known-gap note).
fn type_mismatch<'gc>(
    heap: &Heap<'gc>,
    receiver: &Value<'gc>,
    selector: Symbol,
    got: Option<&Value<'gc>>,
) -> NewspeakError<'gc> {
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!(
            "{selector} requires a numeric argument of the receiver's own type \
             (Milestone 1 has no Integer/Float coercion), got {}",
            heap.describe_argument(got)
        ),
    }
}

/// `Integer>>=`'s rule: an argument of any other type is plainly unequal,
/// never an error (this module's known-gap note says why). Named so that
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
/// lossless widening in `argument_float` below stays, because arithmetic
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

/// An `i64` operation's result that does not fit `i64`, as the
/// `LargeInteger` §5.1.1 asks for: "integers are not limited in size".
fn promote<'gc>(mutator: &mut Mutator<'gc>, wide: i128) -> Value<'gc> {
    mutator.new_integer(ArbitraryInteger::from_i128(wide))
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
    // `+`/`-`/`*`: a non-Integer argument is a type mismatch. A result that
    // fits is answered without allocating; one that does not is recomputed
    // in `i128`, where two `i64`s' sum, difference or product always fits,
    // and handed back as the inner `Err` for the caller to promote -- the
    // closure holds `mutator` shared, so it cannot allocate the
    // `LargeInteger` itself.
    let arithmetic = |operator: &str,
                      checked: fn(i64, i64) -> Option<i64>,
                      wide: fn(i128, i128) -> i128| match argument_integer() {
        None => Err(mismatch(operator)),
        Some(argument) => Ok(checked(receiver_integer, argument)
            .map(Value::SmallInteger)
            .ok_or_else(|| wide(i128::from(receiver_integer), i128::from(argument)))),
    };
    // Ordering comparisons: an argument of the wrong type is an error, not a
    // silent `false`.
    let compare = |operator: &str, operation: fn(i64, i64) -> bool| match argument_integer() {
        None => Err(mismatch(operator)),
        Some(argument) => Ok(Value::Boolean(operation(receiver_integer, argument))),
    };
    match primitive {
        IntegerPrimitive::Add => Some(
            arithmetic("+", i64::checked_add, |left, right| left + right)
                .map(|answer| answer.unwrap_or_else(|wide| promote(mutator, wide))),
        ),
        IntegerPrimitive::Subtract => Some(
            arithmetic("-", i64::checked_sub, |left, right| left - right)
                .map(|answer| answer.unwrap_or_else(|wide| promote(mutator, wide))),
        ),
        IntegerPrimitive::Multiply => Some(
            arithmetic("*", i64::checked_mul, |left, right| left * right)
                .map(|answer| answer.unwrap_or_else(|wide| promote(mutator, wide))),
        ),
        IntegerPrimitive::FloorDivide => Some(match argument_integer() {
            None => Err(mismatch("//")),
            Some(0) => Err(NewspeakError {
                selector: Symbol::intern("//"),
                receiver: Value::SmallInteger(receiver_integer),
                message: "division by zero".to_string(),
            }),
            // With the zero divisor refused above, the one quotient that
            // does not fit is `i64::MIN // -1`'s, which is `-i64::MIN`.
            Some(divisor) => Ok(match checked_floor_divide(receiver_integer, divisor) {
                Some(quotient) => Value::SmallInteger(quotient),
                None => promote(mutator, -i128::from(receiver_integer)),
            }),
        }),
        IntegerPrimitive::Less => Some(compare("<", |left, right| left < right)),
        IntegerPrimitive::Greater => Some(compare(">", |left, right| left > right)),
        IntegerPrimitive::LessOrEqual => Some(compare("<=", |left, right| left <= right)),
        IntegerPrimitive::GreaterOrEqual => Some(compare(">=", |left, right| left >= right)),
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
    match primitive {
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
        // The arithmetic and the ordering on a `LargeInteger` receiver are
        // still to come; until then they decline to `Integer`'s methods
        // through `super`.
        _ => None,
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
    // An `Integer` argument widens losslessly; anything else is a type
    // mismatch, never a silent `NaN` result for arithmetic or a silent
    // `false` for comparisons.
    let argument_float = || match arguments.first() {
        Some(Value::Float(float)) => Some(*float),
        Some(Value::SmallInteger(integer)) => Some(*integer as f64),
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
}
