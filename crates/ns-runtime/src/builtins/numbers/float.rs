//! `Float`'s leaves: IEEE 754 doubles, immediate or boxed. The rules across
//! the numeric classes are the module note's.

use super::{float_operand, type_mismatch, Accepted};
use crate::builtins::collections::{self, IntegerArgument};
use crate::builtins::first_argument;
use crate::builtins::primitive::{FloatPrimitive, Primitive};
use crate::numeric::float_text;
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

/// `Float>>=`: IEEE 754's equality, except that every NaN equals every
/// other. A Float is a value, so its `=` must be an equivalence and its
/// `==` answers the same (§3.1.1); IEEE's is not reflexive. This is
/// JavaScript's SameValueZero, the equality of its `Map` and `Set`: `0.0`
/// and `-0.0` stay equal. The ordering stays IEEE's
/// (`docs/design/core-library.md`, "A NaN equals a NaN").
pub(crate) fn float_equals<'gc>(
    heap: &Heap<'gc>,
    receiver: f64,
    argument: Option<Value<'gc>>,
) -> bool {
    float_operand(heap, argument)
        .is_some_and(|other| other == receiver || (other.is_nan() && receiver.is_nan()))
}

/// A float's content hash: its bits, mixed and folded to 32 bits. `-0.0` is
/// normalized to `0.0` and every NaN to one NaN first, because `=` makes
/// each of them equal to the other, and a `hash` that told them apart would
/// break the one rule a `hash` has ("if `x = y` ... then `x hash = y
/// hash`").
///
/// Shared with `builtins::value`, whose derived hash must answer the same
/// number for a float slot as `Float>>hash` does for the float itself.
pub(crate) fn float_hash(value: f64) -> i64 {
    let normalized = if value == 0.0 {
        0.0
    } else if value.is_nan() {
        f64::NAN
    } else {
        value
    };
    crate::hash::content_hash(normalized.to_bits())
}

pub(crate) fn execute_float<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FloatPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    if let FloatPrimitive::Parse = primitive {
        return parse(mutator, receiver, arguments);
    }
    let receiver_float = mutator.heap.float_value(receiver)?;
    // An `Integer` or `LargeInteger` argument becomes the nearest `double`;
    // anything else is a type mismatch, never a silent `NaN` result for
    // arithmetic or a silent `false` for comparisons.
    let argument_float = || {
        let argument = *arguments.first()?;
        if let Some(float) = mutator.heap.float_value(argument) {
            return Some(float);
        }
        match argument.kind() {
            ValueKind::SmallInteger(integer) => Some(integer as f64),
            ValueKind::Object(object) => mutator
                .heap
                .large_integer(object)
                .map(|integer| integer.to_f64()),
            _ => None,
        }
    };
    let mismatch = |operator: &str| {
        type_mismatch(
            &mutator.heap,
            receiver,
            Symbol::intern(operator),
            Accepted::Number,
            arguments.first().copied(),
        )
    };
    // A double, for the caller to make a `Value` of: the closure holds
    // `mutator` shared, and `Mutator::new_float` needs it.
    let arithmetic = |operator: &str, operation: fn(f64, f64) -> f64| match argument_float() {
        None => Err(mismatch(operator)),
        Some(argument) => Ok(operation(receiver_float, argument)),
    };
    let compare = |operator: &str, operation: fn(f64, f64) -> bool| match argument_float() {
        None => Err(mismatch(operator)),
        Some(argument) => Ok(Value::boolean(operation(receiver_float, argument))),
    };
    match primitive {
        FloatPrimitive::Add => {
            Some(arithmetic("+", |left, right| left + right).map(|float| mutator.new_float(float)))
        }
        FloatPrimitive::Subtract => {
            Some(arithmetic("-", |left, right| left - right).map(|float| mutator.new_float(float)))
        }
        FloatPrimitive::Multiply => {
            Some(arithmetic("*", |left, right| left * right).map(|float| mutator.new_float(float)))
        }
        FloatPrimitive::Divide => {
            Some(arithmetic("/", |left, right| left / right).map(|float| mutator.new_float(float)))
        }
        FloatPrimitive::Less => Some(compare("<", |left, right| left < right)),
        FloatPrimitive::Greater => Some(compare(">", |left, right| left > right)),
        FloatPrimitive::LessOrEqual => Some(compare("<=", |left, right| left <= right)),
        FloatPrimitive::GreaterOrEqual => Some(compare(">=", |left, right| left >= right)),
        // Same exception as `Integer`'s `=`: never an error.
        FloatPrimitive::Equal => crate::interpreter::future::declines_for_a_future(
            mutator,
            Primitive::Float(primitive),
            float_equals(&mutator.heap, receiver_float, arguments.first().copied()),
            receiver,
            arguments,
        )
        .map(|answer| Ok(Value::boolean(answer))),
        FloatPrimitive::Hash => Some(Ok(Value::small_integer(float_hash(receiver_float)))),
        FloatPrimitive::SquareRoot => Some(Ok(mutator.new_float(receiver_float.sqrt()))),
        // Primordial Soup's are the C library's `sin` and `cos`; Rust's
        // call the same functions. No benchmark result depends on their
        // last bit (CD was checked against correctly rounded values).
        FloatPrimitive::Sine => Some(Ok(mutator.new_float(receiver_float.sin()))),
        FloatPrimitive::Cosine => Some(Ok(mutator.new_float(receiver_float.cos()))),
        FloatPrimitive::AsInteger => Some(
            match crate::ArbitraryInteger::from_f64_truncated(receiver_float) {
                Some(integer) => Ok(mutator.new_integer(integer)),
                None => Err(NewspeakError::argument_error(
                    Symbol::intern("asInteger"),
                    receiver,
                    receiver,
                    format!(
                        "asInteger requires a finite Float, got {}",
                        float_text::shortest(receiver_float)
                    ),
                )),
            },
        ),
        FloatPrimitive::PrintString => Some(Ok(Value::object(
            mutator.new_string(float_text::shortest(receiver_float)),
        ))),
        FloatPrimitive::AsStringFixed => Some(
            digit_count(
                &mutator.heap,
                receiver,
                "asStringFixed:",
                0..=20,
                "takes 0 to 20 fraction digits",
                Accepted::Number,
                arguments,
            )
            .map(|count| {
                Value::object(mutator.new_string(float_text::fixed(receiver_float, count)))
            }),
        ),
        FloatPrimitive::AsStringExponential => Some(
            digit_count(
                &mutator.heap,
                receiver,
                "asStringExponential:",
                0..=20,
                "takes 0 to 20 fraction digits",
                Accepted::Integer,
                arguments,
            )
            .map(|count| {
                Value::object(mutator.new_string(float_text::exponential(receiver_float, count)))
            }),
        ),
        FloatPrimitive::AsStringPrecision => Some(
            digit_count(
                &mutator.heap,
                receiver,
                "asStringPrecision:",
                1..=21,
                "takes 1 to 21 digits",
                Accepted::Integer,
                arguments,
            )
            .map(|count| {
                Value::object(mutator.new_string(float_text::precision(receiver_float, count)))
            }),
        ),
        FloatPrimitive::Parse => unreachable!("execute_float answers Parse before this match"),
    }
}

/// A digit count for `asStringFixed:`, `asStringExponential:` or
/// `asStringPrecision:` within `range`, or the `ArgumentError` its
/// selector signals: `out_of_range` names the range in words. The bound is
/// the algorithm's own: a `LargeInteger` is past it as surely as a
/// `SmallInteger` is.
fn digit_count<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: &str,
    range: std::ops::RangeInclusive<i64>,
    out_of_range: &str,
    accepted: Accepted,
    arguments: &[Value<'gc>],
) -> Result<usize, NewspeakError<'gc>> {
    match collections::integer_argument(heap, arguments.first().copied()) {
        Some(IntegerArgument::Small(count)) if range.contains(&count) => Ok(count as usize),
        Some(IntegerArgument::Small(count)) => Err(NewspeakError::argument_error(
            Symbol::intern(selector),
            receiver,
            Value::small_integer(count),
            format!("{selector} {out_of_range}, got {count}"),
        )),
        Some(IntegerArgument::Large { .. }) => Err(NewspeakError::argument_error(
            Symbol::intern(selector),
            receiver,
            first_argument(arguments),
            format!(
                "{selector} {out_of_range}, got {}",
                heap.describe_argument(arguments.first().copied())
            ),
        )),
        None => Err(type_mismatch(
            heap,
            receiver,
            Symbol::intern(selector),
            accepted,
            arguments.first().copied(),
        )),
    }
}

/// `Float class>>parse:`: the Float a text names (`numeric::float_text::parse`
/// says which texts), or an `ArgumentError` carrying the argument. Declines
/// for a subject that is not `Float` itself, which a flat class side cannot
/// be sent from.
fn parse<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::Float) {
        return None;
    }
    Some(parse_text(mutator, receiver, first_argument(arguments)))
}

fn parse_text<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    argument: Value<'gc>,
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = Symbol::intern("parse:");
    let text = argument
        .as_object()
        .filter(|object| !mutator.heap.is_symbol(*object))
        .and_then(|object| mutator.heap.str_value(object));
    let Some(text) = text else {
        return Err(NewspeakError::argument_error(
            selector,
            receiver,
            argument,
            format!(
                "parse: requires a String, got {}",
                mutator.heap.describe(argument)
            ),
        ));
    };
    match float_text::parse(&text) {
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

#[cfg(test)]
mod tests {
    use super::{float_equals, float_hash};

    #[test]
    fn every_nan_equals_every_other_and_hashes_alike() {
        let not_a_numbers = [
            f64::NAN,
            -f64::NAN,
            f64::from_bits(0x7ff8_0000_0000_0001),
            f64::from_bits(0xfff0_0000_0000_0001),
        ];
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            for first in not_a_numbers {
                assert!(first.is_nan());
                for second in not_a_numbers {
                    let second_value = mutator.new_float(second);
                    assert!(float_equals(&mutator.heap, first, Some(second_value)));
                    assert_eq!(float_hash(first), float_hash(second));
                }
            }
            let negative_zero = mutator.new_float(-0.0);
            assert!(float_equals(&mutator.heap, 0.0, Some(negative_zero)));
            assert_eq!(float_hash(0.0), float_hash(-0.0));
            let one = mutator.new_float(1.0);
            assert!(!float_equals(&mutator.heap, f64::NAN, Some(one)));
        });
    }
}
