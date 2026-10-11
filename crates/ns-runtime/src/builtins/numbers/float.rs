//! `Float`'s leaves: IEEE 754 doubles, immediate or boxed. The rules across
//! the numeric classes are the module note's.

use super::{float_operand, type_mismatch, Accepted};
use crate::builtins::collections::{self, IntegerArgument};
use crate::builtins::first_argument;
use crate::builtins::primitive::{FloatPrimitive, Primitive};
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

/// `Float>>asStringFixed:`'s text, as Primordial Soup's double-conversion
/// writes it: `fraction_digits` digits after the point, a tie rounded up on
/// the exact binary value (Rust's formatter would round it to even), and
/// the value's own sign even when the digits round to zero. Outside ±1e21,
/// and for a NaN or an infinity, the text `printString` answers.
pub(crate) fn fixed_text(value: f64, fraction_digits: usize) -> String {
    if !(value > -1e21 && value < 1e21) {
        return value.to_string();
    }
    // Every finite double has an exact decimal expansion of at most 1074
    // fraction digits, so this text is the value itself, not a rounding.
    let exact = format!("{:.1074}", value.abs());
    let (integer_text, fraction_text) = exact
        .split_once('.')
        .expect("a fixed-precision format has a point");
    let mut digits: Vec<u8> = integer_text
        .bytes()
        .chain(fraction_text.bytes().take(fraction_digits))
        .collect();
    if fraction_text.as_bytes()[fraction_digits] >= b'5' {
        let mut position = digits.len();
        loop {
            if position == 0 {
                digits.insert(0, b'1');
                break;
            }
            position -= 1;
            if digits[position] == b'9' {
                digits[position] = b'0';
            } else {
                digits[position] += 1;
                break;
            }
        }
    }
    let integer_length = digits.len() - fraction_digits;
    let mut text = String::with_capacity(digits.len() + 2);
    if value.is_sign_negative() {
        text.push('-');
    }
    text.push_str(std::str::from_utf8(&digits[..integer_length]).expect("ASCII digits"));
    if fraction_digits > 0 {
        text.push('.');
        text.push_str(std::str::from_utf8(&digits[integer_length..]).expect("ASCII digits"));
    }
    text
}

pub(crate) fn execute_float<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FloatPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
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
                    format!("asInteger requires a finite Float, got {receiver_float}"),
                )),
            },
        ),
        FloatPrimitive::PrintString => Some(Ok(Value::object(
            mutator.new_string(receiver_float.to_string()),
        ))),
        FloatPrimitive::AsStringFixed => Some(
            match collections::integer_argument(&mutator.heap, arguments.first().copied()) {
                Some(IntegerArgument::Small(count)) if (0..=20).contains(&count) => Ok(
                    Value::object(mutator.new_string(fixed_text(receiver_float, count as usize))),
                ),
                // The 20 is Primordial Soup's own wrapper, not double-conversion's
                // (which allows 100): its `DoubleToCStringAsFixed` fixes 20 digits
                // either side of the point to size its buffer, so its primitive
                // fails outside 0 to 20 and its method raises.
                Some(IntegerArgument::Small(count)) => Err(NewspeakError::argument_error(
                    Symbol::intern("asStringFixed:"),
                    receiver,
                    Value::small_integer(count),
                    format!("asStringFixed: takes 0 to 20 fraction digits, got {count}"),
                )),
                // A `LargeInteger` is past 20 as surely as a `SmallInteger` is.
                Some(IntegerArgument::Large { .. }) => Err(NewspeakError::argument_error(
                    Symbol::intern("asStringFixed:"),
                    receiver,
                    first_argument(arguments),
                    format!(
                        "asStringFixed: takes 0 to 20 fraction digits, got {}",
                        mutator.heap.describe_argument(arguments.first().copied())
                    ),
                )),
                None => Err(mismatch("asStringFixed:")),
            },
        ),
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

    #[test]
    fn fixed_text_rounds_a_tie_up_on_the_exact_binary_value() {
        assert_eq!(super::fixed_text(0.125, 2), "0.13");
        assert_eq!(super::fixed_text(-0.125, 2), "-0.13");
        assert_eq!(super::fixed_text(2.5, 0), "3");
        assert_eq!(super::fixed_text(0.5, 0), "1");
        assert_eq!(super::fixed_text(1.005, 2), "1.00");
        assert_eq!(super::fixed_text(9.995, 2), "9.99");
        assert_eq!(super::fixed_text(99.95, 1), "100.0");
        assert_eq!(super::fixed_text(123.456, 1), "123.5");
        assert_eq!(super::fixed_text(1.0, 3), "1.000");
        assert_eq!(super::fixed_text(-0.0, 1), "-0.0");
        assert_eq!(super::fixed_text(-0.04, 1), "-0.0");
        assert_eq!(super::fixed_text(0.1, 20), "0.10000000000000000555");
        assert_eq!(super::fixed_text(1e21, 2), (1e21f64).to_string());
        assert_eq!(super::fixed_text(f64::NAN, 2), "NaN");
        assert_eq!(
            super::fixed_text(f64::NEG_INFINITY, 2),
            f64::NEG_INFINITY.to_string()
        );
    }
}
