//! `Integer` (`i64`) and `Float` (`f64`) native methods.
//!
//! # Known gap: no numeric tower
//!
//! There is no coercion between `Integer` and `Float`. There
//! is no `Number` protocol, no `generality`/`coerce:` pair, and no promotion
//! of a mixed operation to the wider type:
//!
//! * `Integer` arithmetic (`dispatch_integer`) requires an `Integer`
//!   argument. `1 + 1.5` does **not** produce `2.5`; it raises a
//!   type-mismatch `NewspeakError`.
//! * `Float` arithmetic (`dispatch_float`) is one-directional: it accepts an
//!   `Integer` argument and widens it (`1.5 + 1` is `2.5`), because that
//!   direction is lossless and free. Any other argument type raises a
//!   type-mismatch `NewspeakError`.
//! * Ordering comparisons (`<`, `>`, `<=`, `>=`) follow the same rule as
//!   arithmetic: a type-mismatched argument is an error, not an answer.
//! * `=` is the deliberate exception: a type-mismatched argument makes it
//!   plain `false` (two values of different types are not equal), never an
//!   error -- matching `Object`'s own `=` and the usual Smalltalk
//!   convention. It is asymmetric across the two receivers, for the same
//!   missing-coercion reason: `1 = 1.0` is `false` (`Integer`'s `=` only
//!   accepts an `Integer`), while `1.0 = 1` is `true` (`Float`'s `=` widens
//!   its argument like the rest of `dispatch_float`). A real numeric tower
//!   would answer `true` both ways.
//!
//! Implementing the tower means widening the whole `Value` numeric domain
//! (`LargeInteger`/`Fraction` belong in the same change) and is out of scope
//! here. Until then these operations fail loudly rather than silently, and
//! with a type-mismatch message of their own: reporting a mixed-type
//! argument as an "Integer overflow" would be actively misleading.

use crate::types::*;
use crate::{Heap, Mutator, Symbol};

/// A mixed-type numeric operation (see this module's known-gap note). Kept
/// distinct from the overflow error below so the message never claims an
/// overflow that did not happen.
fn type_mismatch(
    heap: &Heap,
    receiver: &Value,
    selector: Symbol,
    got: Option<&Value>,
) -> NewspeakError {
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

pub(crate) fn dispatch_integer(
    mutator: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Value, NewspeakError>> {
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
    match selector.as_str() {
        "+" => Some(arithmetic("+", i64::checked_add)),
        "-" => Some(arithmetic("-", i64::checked_sub)),
        "*" => Some(arithmetic("*", i64::checked_mul)),
        "//" => Some(match argument_integer() {
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
        "<" => Some(compare("<", |left, right| left < right)),
        ">" => Some(compare(">", |left, right| left > right)),
        "<=" => Some(compare("<=", |left, right| left <= right)),
        ">=" => Some(compare(">=", |left, right| left >= right)),
        // Deliberately *not* an error on a type mismatch -- see the
        // module-level note.
        "=" => Some(Ok(Value::Boolean(
            argument_integer() == Some(receiver_integer),
        ))),
        "negated" => Some(
            receiver_integer
                .checked_neg()
                .map(Value::SmallInteger)
                .ok_or_else(|| overflow("negated")),
        ),
        "abs" => Some(
            receiver_integer
                .checked_abs()
                .map(Value::SmallInteger)
                .ok_or_else(|| overflow("abs")),
        ),
        "printString" | "asString" => Some(Ok(Value::Object(
            mutator.new_string(receiver_integer.to_string()),
        ))),
        _ => None,
    }
}

pub(crate) fn dispatch_float(
    mutator: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Value, NewspeakError>> {
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
    match selector.as_str() {
        "+" => Some(arithmetic("+", |left, right| left + right)),
        "-" => Some(arithmetic("-", |left, right| left - right)),
        "*" => Some(arithmetic("*", |left, right| left * right)),
        "/" => Some(arithmetic("/", |left, right| left / right)),
        "<" => Some(compare("<", |left, right| left < right)),
        ">" => Some(compare(">", |left, right| left > right)),
        // Same exception as `Integer`'s `=`: never an error.
        "=" => Some(Ok(Value::Boolean(argument_float() == Some(receiver_float)))),
        "printString" | "asString" => Some(Ok(Value::Object(
            mutator.new_string(receiver_float.to_string()),
        ))),
        _ => None,
    }
}
