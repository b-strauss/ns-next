//! `Float`'s leaves: IEEE 754 doubles, immediate or boxed. The rules across
//! the numeric classes are the module note's.

use super::tower::{self, Arithmetic, Comparison};
use super::{Accepted, type_mismatch};
use crate::builtins::collections::{self, IntegerArgument};
use crate::builtins::first_argument;
use crate::builtins::primitive::{FloatPrimitive, Primitive};
use crate::interpreter::Arguments;
use crate::numeric::{float_text, numeric_hash};
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

/// `=` between two doubles: IEEE 754's equality, except that every NaN
/// equals every other. A Float is a value, so its `=` must be an
/// equivalence and its `==` answers the same (§3.1.1); IEEE's is not
/// reflexive. `0.0` and `-0.0` stay equal; the ordering stays IEEE's
/// (`docs/design/core-library.md`, "A NaN equals a NaN"). The same rule as
/// `tower::equal`'s for two `Float`s, kept here as the fast path.
fn doubles_equal(left: f64, right: f64) -> bool {
    left == right || (left.is_nan() && right.is_nan())
}

pub(crate) fn execute_float<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FloatPrimitive,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    if let FloatPrimitive::Parse = primitive {
        return parse(mutator, receiver, arguments);
    }
    let receiver_float = mutator.heap.float_value(receiver)?;
    // `atan:` and `raisedTo:` take any number, converted to the nearest
    // double in one rounding; anything else is a type mismatch, never a
    // silent `NaN`. A double is handed back for the caller to make a
    // `Value` of: the closure holds `mutator` shared, and
    // `Mutator::new_float` needs it.
    let float_function = |operator: &str, operation: fn(f64, f64) -> f64| {
        let argument = arguments.first(&mutator.heap);
        match argument.and_then(|value| tower::operand(&mutator.heap, value)) {
            Some(number) => Ok(operation(receiver_float, number.to_f64())),
            None => Err(type_mismatch(
                &mutator.heap,
                receiver,
                Symbol::intern(operator),
                Accepted::Number,
                argument,
            )),
        }
    };
    match primitive {
        FloatPrimitive::Add => arithmetic(
            mutator,
            Arithmetic::Add,
            receiver,
            receiver_float,
            arguments,
            |left, right| left + right,
        ),
        FloatPrimitive::Subtract => arithmetic(
            mutator,
            Arithmetic::Subtract,
            receiver,
            receiver_float,
            arguments,
            |left, right| left - right,
        ),
        FloatPrimitive::Multiply => arithmetic(
            mutator,
            Arithmetic::Multiply,
            receiver,
            receiver_float,
            arguments,
            |left, right| left * right,
        ),
        FloatPrimitive::Divide => arithmetic(
            mutator,
            Arithmetic::Divide,
            receiver,
            receiver_float,
            arguments,
            |left, right| left / right,
        ),
        FloatPrimitive::Less => Some(compare(
            &mutator.heap,
            Comparison::Less,
            receiver,
            receiver_float,
            arguments,
            |left, right| left < right,
        )),
        FloatPrimitive::Greater => Some(compare(
            &mutator.heap,
            Comparison::Greater,
            receiver,
            receiver_float,
            arguments,
            |left, right| left > right,
        )),
        FloatPrimitive::LessOrEqual => Some(compare(
            &mutator.heap,
            Comparison::LessOrEqual,
            receiver,
            receiver_float,
            arguments,
            |left, right| left <= right,
        )),
        FloatPrimitive::GreaterOrEqual => Some(compare(
            &mutator.heap,
            Comparison::GreaterOrEqual,
            receiver,
            receiver_float,
            arguments,
            |left, right| left >= right,
        )),
        // Never an error: anything that is no number is unequal. A `Float`
        // or a `SmallInteger` within 2^53 compares here as a double, every
        // other argument the tower's way.
        FloatPrimitive::Equal => {
            let equal = match arguments
                .first(&mutator.heap)
                .and_then(|argument| tower::ordering_double(&mutator.heap, argument))
            {
                Some(argument) => doubles_equal(receiver_float, argument),
                None => {
                    tower::values_equal(&mutator.heap, receiver, arguments.first(&mutator.heap))
                }
            };
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Float(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        FloatPrimitive::Hash => Some(Ok(Value::small_integer(numeric_hash::float_hash(
            receiver_float,
        )))),
        FloatPrimitive::SquareRoot => Some(Ok(mutator.new_float(receiver_float.sqrt()))),
        // Primordial Soup's are the C library's `sin` and `cos`; Rust's
        // call the same functions. No benchmark result depends on their
        // last bit (CD was checked against correctly rounded values).
        FloatPrimitive::Sine => Some(Ok(mutator.new_float(receiver_float.sin()))),
        FloatPrimitive::Cosine => Some(Ok(mutator.new_float(receiver_float.cos()))),
        FloatPrimitive::Tangent => Some(Ok(mutator.new_float(receiver_float.tan()))),
        FloatPrimitive::ArcSine => Some(Ok(mutator.new_float(receiver_float.asin()))),
        FloatPrimitive::ArcCosine => Some(Ok(mutator.new_float(receiver_float.acos()))),
        FloatPrimitive::ArcTangent => Some(Ok(mutator.new_float(receiver_float.atan()))),
        // `y atan: x`, the angle of the point (x, y).
        FloatPrimitive::ArcTangentOf => {
            Some(float_function("atan:", f64::atan2).map(|float| mutator.new_float(float)))
        }
        FloatPrimitive::Exponential => Some(Ok(mutator.new_float(receiver_float.exp()))),
        FloatPrimitive::NaturalLogarithm => Some(Ok(mutator.new_float(receiver_float.ln()))),
        FloatPrimitive::Logarithm => Some(Ok(mutator.new_float(receiver_float.log10()))),
        FloatPrimitive::RaisedTo => {
            Some(float_function("raisedTo:", f64::powf).map(|float| mutator.new_float(float)))
        }
        FloatPrimitive::Floor => Some(Ok(mutator.new_float(receiver_float.floor()))),
        FloatPrimitive::Ceiling => Some(Ok(mutator.new_float(receiver_float.ceil()))),
        // Half away from zero, as C's `round`.
        FloatPrimitive::Rounded => Some(Ok(mutator.new_float(receiver_float.round()))),
        FloatPrimitive::Truncated => Some(Ok(mutator.new_float(receiver_float.trunc()))),
        // The sign bit flipped, so `0.0 negated` is `-0.0`, which `0 - self`
        // would not answer.
        FloatPrimitive::Negated => Some(Ok(mutator.new_float(-receiver_float))),
        FloatPrimitive::Absolute => Some(Ok(mutator.new_float(receiver_float.abs()))),
        FloatPrimitive::IsNaN => Some(Ok(Value::boolean(receiver_float.is_nan()))),
        FloatPrimitive::IsFinite => Some(Ok(Value::boolean(receiver_float.is_finite()))),
        FloatPrimitive::IsInfinite => Some(Ok(Value::boolean(receiver_float.is_infinite()))),
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

/// `+ - * /`: a `Float` or a `SmallInteger` argument is the fast path,
/// computed as the tower computes it (`tower::arithmetic_double`); every
/// other argument -- a number of another class, or the type mismatch of
/// one that is none -- is the tower's. The operation is a closure, not a
/// function pointer, so that it is inlined: called through a pointer, a
/// Float's every `+` paid an indirect call.
#[inline(always)]
fn arithmetic<'gc>(
    mutator: &mut Mutator<'gc>,
    operation: Arithmetic,
    receiver: Value<'gc>,
    receiver_float: f64,
    arguments: Arguments<'_, 'gc>,
    fast: impl FnOnce(f64, f64) -> f64,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match arguments
        .first(&mutator.heap)
        .and_then(|argument| tower::arithmetic_double(&mutator.heap, argument))
    {
        Some(argument) => Some(Ok(mutator.new_float(fast(receiver_float, argument)))),
        None => tower::arithmetic(mutator, operation, receiver, arguments),
    }
}

/// The four orderings: a `Float` or a `SmallInteger` within 2^53 compares
/// here as IEEE 754 doubles (`tower::ordering_double`); every other
/// argument -- a number of another class, or the type mismatch of one that
/// is none -- is the tower's. A closure for the reason `arithmetic` takes
/// one.
#[inline(always)]
fn compare<'gc>(
    heap: &Heap<'gc>,
    comparison: Comparison,
    receiver: Value<'gc>,
    receiver_float: f64,
    arguments: Arguments<'_, 'gc>,
    fast: impl FnOnce(f64, f64) -> bool,
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    match arguments
        .first(heap)
        .and_then(|argument| tower::ordering_double(heap, argument))
    {
        Some(argument) => Ok(Value::boolean(fast(receiver_float, argument))),
        None => tower::compare(heap, comparison, receiver, arguments)
            .expect("a Float receiver is a number"),
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
    arguments: Arguments<'_, 'gc>,
) -> Result<usize, NewspeakError<'gc>> {
    match collections::integer_argument(heap, arguments.first(heap)) {
        Some(IntegerArgument::Small(count)) => match usize::try_from(count) {
            Ok(digits) if range.contains(&count) => Ok(digits),
            _ => Err(NewspeakError::argument_error(
                Symbol::intern(selector),
                receiver,
                Value::small_integer(count),
                format!("{selector} {out_of_range}, got {count}"),
            )),
        },
        Some(IntegerArgument::Large { .. }) => Err(NewspeakError::argument_error(
            Symbol::intern(selector),
            receiver,
            first_argument(heap, arguments),
            format!(
                "{selector} {out_of_range}, got {}",
                heap.describe_argument(arguments.first(heap))
            ),
        )),
        None => Err(type_mismatch(
            heap,
            receiver,
            Symbol::intern(selector),
            accepted,
            arguments.first(heap),
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
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::Float) {
        return None;
    }
    Some(parse_text(
        mutator,
        receiver,
        first_argument(&mutator.heap, arguments),
    ))
}

/// The work of `parse`: reads `argument` as a String that is not a Symbol and
/// answers the Float its text names, or the `ArgumentError` `parse:` signals.
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
    use super::tower::{Operand, equal, hash};

    #[test]
    fn every_nan_equals_every_other_and_hashes_alike() {
        let not_a_numbers = [
            f64::NAN,
            -f64::NAN,
            f64::from_bits(0x7ff8_0000_0000_0001),
            f64::from_bits(0xfff0_0000_0000_0001),
        ];
        for first in not_a_numbers {
            assert!(first.is_nan());
            for second in not_a_numbers {
                assert!(equal(&Operand::Float(first), &Operand::Float(second)));
                assert_eq!(hash(&Operand::Float(first)), hash(&Operand::Float(second)));
            }
        }
        assert!(equal(&Operand::Float(0.0), &Operand::Float(-0.0)));
        assert_eq!(hash(&Operand::Float(0.0)), hash(&Operand::Float(-0.0)));
        assert!(!equal(&Operand::Float(f64::NAN), &Operand::Float(1.0)));
    }
}
