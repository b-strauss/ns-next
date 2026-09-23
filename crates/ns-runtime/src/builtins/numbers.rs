//! `Integer` (`i64`) and `Float` (`f64`) native methods.
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
//!   alike, because each `=` is class-tight (`Character>>=` and `String>>=`
//!   are for the same reason). `Float>>=` widened an `Integer` argument
//!   until 2026-09-23, which made `1.0 = 1` true while `1.0 hash` and
//!   `1 hash` differ -- the one rule a `hash` has -- and was asymmetric
//!   besides. A real numeric tower would answer `true` both ways *and*
//!   hash alike; until there is one, `false` both ways is the answer that
//!   keeps the `=`/`hash` contract.
//!
//! Implementing the tower means widening the whole `Value` numeric domain
//! (`LargeInteger`/`Fraction` belong in the same change) and is out of scope
//! here. Until then these operations fail loudly rather than silently, and
//! with a type-mismatch message of their own: reporting a mixed-type
//! argument as an "Integer overflow" would be actively misleading.

use crate::builtins::primitive::{FloatPrimitive, IntegerPrimitive};
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

/// A mixed-type numeric operation (see this module's known-gap note). Kept
/// distinct from the overflow error below so the message never claims an
/// overflow that did not happen.
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
    let overflow = |operator: &str| NewspeakError {
        selector: Symbol::intern(operator),
        receiver: Value::SmallInteger(receiver_integer),
        message: format!(
            "Integer overflow computing {receiver_integer} {operator} {}",
            mutator.heap.describe_argument(arguments.first())
        ),
    };
    // `+`/`-`/`*`: a non-Integer argument is a type mismatch; only a genuine
    // `checked_*` failure is an overflow.
    let arithmetic =
        |operator: &str, operation: fn(i64, i64) -> Option<i64>| match argument_integer() {
            None => Err(mismatch(operator)),
            Some(argument) => operation(receiver_integer, argument)
                .map(Value::SmallInteger)
                .ok_or_else(|| overflow(operator)),
        };
    // Ordering comparisons: an argument of the wrong type is an error, not a
    // silent `false`.
    let compare = |operator: &str, operation: fn(i64, i64) -> bool| match argument_integer() {
        None => Err(mismatch(operator)),
        Some(argument) => Ok(Value::Boolean(operation(receiver_integer, argument))),
    };
    match primitive {
        IntegerPrimitive::Add => Some(arithmetic("+", i64::checked_add)),
        IntegerPrimitive::Subtract => Some(arithmetic("-", i64::checked_sub)),
        IntegerPrimitive::Multiply => Some(arithmetic("*", i64::checked_mul)),
        IntegerPrimitive::FloorDivide => Some(match argument_integer() {
            None => Err(mismatch("//")),
            Some(0) => Err(NewspeakError {
                selector: Symbol::intern("//"),
                receiver: Value::SmallInteger(receiver_integer),
                message: "division by zero".to_string(),
            }),
            Some(divisor) => receiver_integer
                .checked_div_euclid(divisor)
                .map(Value::SmallInteger)
                .ok_or_else(|| overflow("//")),
        }),
        IntegerPrimitive::Less => Some(compare("<", |left, right| left < right)),
        IntegerPrimitive::Greater => Some(compare(">", |left, right| left > right)),
        IntegerPrimitive::LessOrEqual => Some(compare("<=", |left, right| left <= right)),
        IntegerPrimitive::GreaterOrEqual => Some(compare(">=", |left, right| left >= right)),
        // Deliberately *not* an error on a type mismatch -- see the
        // module-level note.
        IntegerPrimitive::Equal => Some(Ok(Value::Boolean(integer_equals(
            receiver_integer,
            arguments.first(),
        )))),
        IntegerPrimitive::Negated => Some(
            receiver_integer
                .checked_neg()
                .map(Value::SmallInteger)
                .ok_or_else(|| overflow("negated")),
        ),
        IntegerPrimitive::Absolute => Some(
            receiver_integer
                .checked_abs()
                .map(Value::SmallInteger)
                .ok_or_else(|| overflow("abs")),
        ),
        IntegerPrimitive::PrintString => Some(Ok(Value::Object(
            mutator.new_string(receiver_integer.to_string()),
        ))),
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
        FloatPrimitive::Equal => Some(Ok(Value::Boolean(float_equals(
            receiver_float,
            arguments.first(),
        )))),
        FloatPrimitive::Hash => Some(Ok(Value::SmallInteger(float_hash(receiver_float)))),
        FloatPrimitive::PrintString => Some(Ok(Value::Object(
            mutator.new_string(receiver_float.to_string()),
        ))),
    }
}
