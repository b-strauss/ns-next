//! `Integer` (`i64`) and `Float` (`f64`) native methods.

use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch_integer(
    _interp: &mut Interpreter,
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
    let overflow = |op: &str| NewspeakError {
        selector: Symbol::intern(op),
        receiver: Value::SmallInt(n),
        message: format!("Integer overflow computing {n} {op} {:?}", args.first()),
    };
    match selector.as_str() {
        "+" => Some(
            arg_int()
                .and_then(|m| n.checked_add(m))
                .map(Value::SmallInt)
                .ok_or_else(|| overflow("+")),
        ),
        "-" => Some(
            arg_int()
                .and_then(|m| n.checked_sub(m))
                .map(Value::SmallInt)
                .ok_or_else(|| overflow("-")),
        ),
        "*" => Some(
            arg_int()
                .and_then(|m| n.checked_mul(m))
                .map(Value::SmallInt)
                .ok_or_else(|| overflow("*")),
        ),
        "//" => Some(match arg_int() {
            Some(0) => Err(NewspeakError {
                selector: Symbol::intern("//"),
                receiver: Value::SmallInt(n),
                message: "division by zero".to_string(),
            }),
            Some(m) => n
                .checked_div_euclid(m)
                .map(Value::SmallInt)
                .ok_or_else(|| overflow("//")),
            None => Err(overflow("//")),
        }),
        "<" => Some(Ok(Value::Bool(arg_int().is_some_and(|m| n < m)))),
        ">" => Some(Ok(Value::Bool(arg_int().is_some_and(|m| n > m)))),
        "<=" => Some(Ok(Value::Bool(arg_int().is_some_and(|m| n <= m)))),
        ">=" => Some(Ok(Value::Bool(arg_int().is_some_and(|m| n >= m)))),
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
        "printString" | "asString" => Some(Ok(Value::Obj(_interp.new_string(n.to_string())))),
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
    let arg_float = || match args.first() {
        Some(Value::Float(y)) => Some(*y),
        Some(Value::SmallInt(y)) => Some(*y as f64),
        _ => None,
    };
    match selector.as_str() {
        "+" => Some(Ok(Value::Float(x + arg_float().unwrap_or(f64::NAN)))),
        "-" => Some(Ok(Value::Float(x - arg_float().unwrap_or(f64::NAN)))),
        "*" => Some(Ok(Value::Float(x * arg_float().unwrap_or(f64::NAN)))),
        "/" => Some(Ok(Value::Float(x / arg_float().unwrap_or(f64::NAN)))),
        "<" => Some(Ok(Value::Bool(arg_float().is_some_and(|y| x < y)))),
        ">" => Some(Ok(Value::Bool(arg_float().is_some_and(|y| x > y)))),
        "=" => Some(Ok(Value::Bool(arg_float() == Some(x)))),
        "printString" | "asString" => Some(Ok(Value::Obj(interp.new_string(x.to_string())))),
        _ => None,
    }
}
