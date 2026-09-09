//! `Integer` (`i64`) and `Float` (`f64`) native methods.
//!
//! # Milestone 1 known gap: no numeric tower
//!
//! Milestone 1 implements no coercion between `Integer` and `Float`. There
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
//! here. Until then these operations fail loudly rather than silently: an
//! earlier version reported a mixed-type argument as an "Integer overflow",
//! which was actively misleading.

use crate::types::*;
use crate::{Heap, Interpreter, Symbol};

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
            heap.describe_arg(got)
        ),
    }
}

pub fn dispatch_integer(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::SmallInt(n) = receiver else {
        return None;
    };
    let arg_int = || match args.first() {
        Some(Value::SmallInt(m)) => Some(*m),
        _ => None,
    };
    let mismatch =
        |op: &str| type_mismatch(&interp.heap, &receiver, Symbol::intern(op), args.first());
    let overflow = |op: &str| NewspeakError {
        selector: Symbol::intern(op),
        receiver: Value::SmallInt(n),
        message: format!(
            "Integer overflow computing {n} {op} {}",
            interp.heap.describe_arg(args.first())
        ),
    };
    // `+`/`-`/`*`: a non-Integer argument is a type mismatch; only a genuine
    // `checked_*` failure is an overflow.
    let arith = |op: &str, f: fn(i64, i64) -> Option<i64>| match arg_int() {
        None => Err(mismatch(op)),
        Some(m) => f(n, m).map(Value::SmallInt).ok_or_else(|| overflow(op)),
    };
    // Ordering comparisons: an argument of the wrong type is an error, not a
    // silent `false` (which is what an earlier version answered).
    let compare = |op: &str, f: fn(i64, i64) -> bool| match arg_int() {
        None => Err(mismatch(op)),
        Some(m) => Ok(Value::Bool(f(n, m))),
    };
    match selector.as_str() {
        "+" => Some(arith("+", i64::checked_add)),
        "-" => Some(arith("-", i64::checked_sub)),
        "*" => Some(arith("*", i64::checked_mul)),
        "//" => Some(match arg_int() {
            None => Err(mismatch("//")),
            Some(0) => Err(NewspeakError {
                selector: Symbol::intern("//"),
                receiver: Value::SmallInt(n),
                message: "division by zero".to_string(),
            }),
            Some(m) => n
                .checked_div_euclid(m)
                .map(Value::SmallInt)
                .ok_or_else(|| overflow("//")),
        }),
        "<" => Some(compare("<", |a, b| a < b)),
        ">" => Some(compare(">", |a, b| a > b)),
        "<=" => Some(compare("<=", |a, b| a <= b)),
        ">=" => Some(compare(">=", |a, b| a >= b)),
        // Deliberately *not* an error on a type mismatch -- see the
        // module-level note.
        "=" => Some(Ok(Value::Bool(arg_int() == Some(n)))),
        "negated" => Some(
            n.checked_neg()
                .map(Value::SmallInt)
                .ok_or_else(|| overflow("negated")),
        ),
        "abs" => Some(
            n.checked_abs()
                .map(Value::SmallInt)
                .ok_or_else(|| overflow("abs")),
        ),
        "printString" | "asString" => Some(Ok(Value::Obj(interp.new_string(n.to_string())))),
        _ => None,
    }
}

pub fn dispatch_float(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Float(x) = receiver else {
        return None;
    };
    // An `Integer` argument widens losslessly; anything else is a type
    // mismatch (previously: a silent `NaN` result for arithmetic, a silent
    // `false` for comparisons).
    let arg_float = || match args.first() {
        Some(Value::Float(y)) => Some(*y),
        Some(Value::SmallInt(y)) => Some(*y as f64),
        _ => None,
    };
    let mismatch =
        |op: &str| type_mismatch(&interp.heap, &receiver, Symbol::intern(op), args.first());
    let arith = |op: &str, f: fn(f64, f64) -> f64| match arg_float() {
        None => Err(mismatch(op)),
        Some(y) => Ok(Value::Float(f(x, y))),
    };
    let compare = |op: &str, f: fn(f64, f64) -> bool| match arg_float() {
        None => Err(mismatch(op)),
        Some(y) => Ok(Value::Bool(f(x, y))),
    };
    match selector.as_str() {
        "+" => Some(arith("+", |a, b| a + b)),
        "-" => Some(arith("-", |a, b| a - b)),
        "*" => Some(arith("*", |a, b| a * b)),
        "/" => Some(arith("/", |a, b| a / b)),
        "<" => Some(compare("<", |a, b| a < b)),
        ">" => Some(compare(">", |a, b| a > b)),
        // Same exception as `Integer`'s `=`: never an error.
        "=" => Some(Ok(Value::Bool(arg_float() == Some(x)))),
        "printString" | "asString" => Some(Ok(Value::Obj(interp.new_string(x.to_string())))),
        _ => None,
    }
}
