//! `Integer`'s leaves: the `i64` that fits a `SmallInteger`. The rules across
//! the numeric classes are the module note's.

use super::tower::{self, Arithmetic, Comparison};
use super::{
    Accepted, checked_floor_divide, division_by_zero, floor_modulo, integer_operand, promote,
    shift_left, type_mismatch,
};
use crate::builtins::first_argument;
use crate::builtins::primitive::{AnyIntegerPrimitive, IntegerPrimitive, Primitive};
use crate::interpreter::Arguments;
use crate::types::*;
use crate::{ArbitraryInteger, Mutator, Symbol};
use std::cmp::Ordering;

pub(crate) fn execute_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: IntegerPrimitive,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let receiver_integer = receiver.as_small_integer()?;
    let argument_integer = || {
        arguments
            .first(&mutator.heap)
            .and_then(|value| value.as_small_integer())
    };
    let mismatch = |operator: &str, accepted: Accepted| {
        type_mismatch(
            &mutator.heap,
            receiver,
            Symbol::intern(operator),
            accepted,
            arguments.first(&mutator.heap),
        )
    };
    // A `LargeInteger` argument, read only once the argument is known to be
    // no `SmallInteger`, so the common case never touches the heap.
    let large_argument = || integer_operand(&mutator.heap, arguments.first(&mutator.heap));
    // Ordering comparisons: two `SmallInteger`s compare here; every other
    // argument -- a number of another class, or the type mismatch of one
    // that is none -- is the tower's.
    let compare = |comparison: Comparison, operation: fn(Ordering) -> bool| match arguments
        .first(&mutator.heap)
        .and_then(|argument| receiver.small_integer_order(argument))
    {
        Some(order) => Ok(Value::boolean(operation(order))),
        None => tower::compare(&mutator.heap, comparison, receiver, arguments)
            .expect("an Integer receiver is a number"),
    };
    match primitive {
        IntegerPrimitive::Add => small_arithmetic(
            mutator,
            Arithmetic::Add,
            receiver,
            arguments,
            Value::small_integer_sum,
            |left, right| left + right,
        ),
        IntegerPrimitive::Subtract => small_arithmetic(
            mutator,
            Arithmetic::Subtract,
            receiver,
            arguments,
            Value::small_integer_difference,
            |left, right| left - right,
        ),
        IntegerPrimitive::Multiply => small_arithmetic(
            mutator,
            Arithmetic::Multiply,
            receiver,
            arguments,
            Value::small_integer_product,
            |left, right| left * right,
        ),
        // Two `SmallInteger`s divide here; every other divisor -- a number
        // of another class, or the type mismatch of one that is none -- is
        // the tower's.
        IntegerPrimitive::FloorDivide => match argument_integer() {
            Some(0) => Some(Err(division_by_zero(
                receiver,
                "//",
                arguments.get(&mutator.heap, 0),
            ))),
            // With the zero divisor refused above, the one quotient that
            // does not fit is `SMALL_INTEGER_MIN // -1`'s, which is
            // `-SMALL_INTEGER_MIN`.
            Some(divisor) => Some(Ok(
                match checked_floor_divide(receiver_integer, divisor)
                    .filter(|number| fits_small_integer(*number))
                {
                    Some(quotient) => Value::small_integer(quotient),
                    None => promote(mutator, -i128::from(receiver_integer)),
                },
            )),
            None => tower::arithmetic(mutator, Arithmetic::FloorDivide, receiver, arguments),
        },
        IntegerPrimitive::Modulo => match argument_integer() {
            Some(0) => Some(Err(division_by_zero(
                receiver,
                "\\\\",
                arguments.get(&mutator.heap, 0),
            ))),
            Some(divisor) => Some(Ok(Value::small_integer(floor_modulo(
                receiver_integer,
                divisor,
            )))),
            None => tower::arithmetic(mutator, Arithmetic::Modulo, receiver, arguments),
        },
        IntegerPrimitive::BitAnd => Some(match argument_integer() {
            Some(argument) => Ok(Value::small_integer(receiver_integer & argument)),
            None => match large_argument() {
                Some(argument) => Ok(mutator
                    .new_integer(ArbitraryInteger::from_i64(receiver_integer).bit_and(&argument))),
                None => Err(mismatch("bitAnd:", Accepted::Integer)),
            },
        }),
        IntegerPrimitive::BitOr => Some(match argument_integer() {
            Some(argument) => Ok(Value::small_integer(receiver_integer | argument)),
            None => match large_argument() {
                Some(argument) => Ok(mutator
                    .new_integer(ArbitraryInteger::from_i64(receiver_integer).bit_or(&argument))),
                None => Err(mismatch("bitOr:", Accepted::Integer)),
            },
        }),
        IntegerPrimitive::BitXor => Some(match argument_integer() {
            Some(argument) => Ok(Value::small_integer(receiver_integer ^ argument)),
            None => match large_argument() {
                Some(argument) => Ok(mutator
                    .new_integer(ArbitraryInteger::from_i64(receiver_integer).bit_xor(&argument))),
                None => Err(mismatch("bitXor:", Accepted::Integer)),
            },
        }),
        IntegerPrimitive::ShiftLeft => Some(match argument_integer() {
            Some(count) if (0..64).contains(&count) => {
                let wide = i128::from(receiver_integer) << count;
                Ok(
                    match i64::try_from(wide)
                        .ok()
                        .filter(|number| fits_small_integer(*number))
                    {
                        Some(small) => Value::small_integer(small),
                        None => promote(mutator, wide),
                    },
                )
            }
            _ => shift_left(
                mutator,
                receiver,
                &ArbitraryInteger::from_i64(receiver_integer),
                arguments,
            ),
        }),
        IntegerPrimitive::Less => Some(compare(Comparison::Less, Ordering::is_lt)),
        IntegerPrimitive::Greater => Some(compare(Comparison::Greater, Ordering::is_gt)),
        IntegerPrimitive::LessOrEqual => Some(compare(Comparison::LessOrEqual, Ordering::is_le)),
        IntegerPrimitive::GreaterOrEqual => {
            Some(compare(Comparison::GreaterOrEqual, Ordering::is_ge))
        }
        // Never an error: anything that is no number is unequal. A
        // `SmallInteger` argument compares here, every other the tower's way.
        IntegerPrimitive::Equal => {
            let equal = match argument_integer() {
                Some(argument) => argument == receiver_integer,
                None => {
                    tower::values_equal(&mutator.heap, receiver, arguments.first(&mutator.heap))
                }
            };
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Integer(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        // `SMALL_INTEGER_MIN` is the one receiver whose negation and
        // absolute value do not fit.
        IntegerPrimitive::Negated => Some(Ok(
            match receiver_integer
                .checked_neg()
                .filter(|number| fits_small_integer(*number))
            {
                Some(negated) => Value::small_integer(negated),
                None => mutator.new_integer(ArbitraryInteger::from_i64(receiver_integer).negated()),
            },
        )),
        IntegerPrimitive::Absolute => Some(Ok(
            match receiver_integer
                .checked_abs()
                .filter(|number| fits_small_integer(*number))
            {
                Some(absolute) => Value::small_integer(absolute),
                None => {
                    mutator.new_integer(ArbitraryInteger::from_i64(receiver_integer).absolute())
                }
            },
        )),
        IntegerPrimitive::PrintString => Some(Ok(Value::object(
            mutator.new_string(receiver_integer.to_string()),
        ))),
        // `as` rounds to the nearest double, ties to even.
        IntegerPrimitive::AsFloat => Some(Ok(mutator.new_float(receiver_integer as f64))),
    }
}

/// `+`, `-` or `*` on a `SmallInteger` receiver. A `SmallInteger` argument
/// is the fast path: a result that fits is answered without allocating,
/// and one that does not is recomputed in `i128`, where two `i64`s' sum,
/// difference or product always fits. Every other argument -- a number of
/// another class, or the type mismatch of one that is none -- is the
/// tower's.
fn small_arithmetic<'gc>(
    mutator: &mut Mutator<'gc>,
    operation: Arithmetic,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
    tagged: fn(Value<'gc>, Value<'gc>) -> Option<Value<'gc>>,
    wide: fn(i128, i128) -> i128,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let receiver_integer = receiver.as_small_integer()?;
    let small_argument = arguments
        .first(&mutator.heap)
        .and_then(|argument| Some((argument, argument.as_small_integer()?)));
    let Some((argument, argument_integer)) = small_argument else {
        return tower::arithmetic(mutator, operation, receiver, arguments);
    };
    Some(Ok(match tagged(receiver, argument) {
        Some(result) => result,
        None => promote(
            mutator,
            wide(i128::from(receiver_integer), i128::from(argument_integer)),
        ),
    }))
}

/// The leaves that take an integer receiver of either representation, so
/// that `LargeInteger` inherits `Integer`'s methods for them, and `parse:`,
/// whose subject is the `Integer` class. `None` declines a receiver that is
/// neither.
pub(crate) fn execute_any_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: AnyIntegerPrimitive,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    if let AnyIntegerPrimitive::Parse = primitive {
        return parse(mutator, receiver, arguments);
    }
    let integer = integer_operand(&mutator.heap, Some(receiver))?;
    Some(match primitive {
        AnyIntegerPrimitive::Gcd => gcd(mutator, receiver, &integer, arguments),
        AnyIntegerPrimitive::ShiftRight => shift_right(mutator, receiver, &integer, arguments),
        AnyIntegerPrimitive::DigitAt => digit_at(mutator, receiver, &integer, arguments),
        AnyIntegerPrimitive::DigitLength => {
            Ok(Value::small_integer(integer.base_256_length() as i64))
        }
        AnyIntegerPrimitive::AsStringRadix => radix_text(mutator, receiver, &integer, arguments),
        AnyIntegerPrimitive::Parse => unreachable!("answered above"),
    })
}

/// The argument of `selector` as an integer of either representation, or
/// the type mismatch naming the selector.
fn integer_argument<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    argument: Option<Value<'gc>>,
) -> Result<ArbitraryInteger, NewspeakError<'gc>> {
    integer_operand(&mutator.heap, argument).ok_or_else(|| {
        type_mismatch(
            &mutator.heap,
            receiver,
            selector,
            Accepted::Integer,
            argument,
        )
    })
}

/// `gcd:`.
fn gcd<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    integer: &ArbitraryInteger,
    arguments: Arguments<'_, 'gc>,
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let other = integer_argument(
        mutator,
        receiver,
        Symbol::intern("gcd:"),
        arguments.first(&mutator.heap),
    )?;
    Ok(mutator.new_integer(integer.gcd(&other)))
}

/// `>>`: a count past `u64` shifts every bit out, as `u64::MAX` does.
fn shift_right<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    integer: &ArbitraryInteger,
    arguments: Arguments<'_, 'gc>,
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = Symbol::intern(">>");
    let count = integer_argument(mutator, receiver, selector, arguments.first(&mutator.heap))?;
    if count.is_negative() {
        return Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(&mutator.heap, arguments),
            format!(
                ">> takes a non-negative shift count, got {}",
                count.to_bounded_text()
            ),
        ));
    }
    Ok(mutator.new_integer(integer.shift_right(count.to_u64().unwrap_or(u64::MAX))))
}

/// `digitAt:`: the index counts from 1.
fn digit_at<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    integer: &ArbitraryInteger,
    arguments: Arguments<'_, 'gc>,
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = Symbol::intern("digitAt:");
    let index = integer_argument(mutator, receiver, selector, arguments.first(&mutator.heap))?;
    let length = integer.base_256_length();
    let digit = index
        .to_u64()
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| integer.base_256_digit(index));
    match digit {
        Some(digit) => Ok(Value::small_integer(i64::from(digit))),
        None => Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(&mutator.heap, arguments),
            format!(
                "digitAt: index {} out of range: valid indices are 1..{length} (digitLength {length})",
                index.to_bounded_text()
            ),
        )),
    }
}

/// A radix from 2 to 36, or the ArgumentError that names `selector`
/// (`asStringRadix:` or `parse:radix:`).
fn radix_argument<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    radix: Value<'gc>,
) -> Result<u32, NewspeakError<'gc>> {
    let integer = integer_argument(mutator, receiver, selector, Some(radix))?;
    match integer
        .to_u64()
        .and_then(|number| u32::try_from(number).ok())
    {
        Some(radix_number @ 2..=36) => Ok(radix_number),
        _ => Err(NewspeakError::argument_error(
            selector,
            receiver,
            radix,
            format!(
                "{selector} takes a radix from 2 to 36, got {}",
                integer.to_bounded_text()
            ),
        )),
    }
}

/// `asStringRadix:`.
fn radix_text<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    integer: &ArbitraryInteger,
    arguments: Arguments<'_, 'gc>,
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let radix = radix_argument(
        mutator,
        receiver,
        Symbol::intern("asStringRadix:"),
        first_argument(&mutator.heap, arguments),
    )?;
    Ok(Value::object(
        mutator.new_string(integer.to_radix_string(radix)),
    ))
}

/// `Integer class>>parse:radix:`: the Integer a String writes in a radix,
/// or an `ArgumentError` carrying the argument at fault. Declines for a
/// subject that is not `Integer` itself, which a flat class side cannot be
/// sent from.
fn parse<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::Integer) {
        return None;
    }
    let selector = Symbol::intern("parse:radix:");
    let text_value = first_argument(&mutator.heap, arguments);
    let radix_value = arguments.try_get(&mutator.heap, 1).unwrap_or(Value::nil());
    let radix = match radix_argument(mutator, receiver, selector, radix_value) {
        Ok(radix) => radix,
        Err(error) => return Some(Err(error)),
    };
    // A Symbol is refused: it is not the text of a number.
    let text = text_value
        .as_object()
        .filter(|object| !mutator.heap.is_symbol(*object))
        .and_then(|object| mutator.heap.str_value(object));
    let Some(text) = text else {
        return Some(Err(NewspeakError::argument_error(
            selector,
            receiver,
            text_value,
            format!(
                "parse:radix: requires a String, got {}",
                mutator.heap.describe(text_value)
            ),
        )));
    };
    Some(match ArbitraryInteger::parse(&text, radix) {
        Some(integer) => Ok(mutator.new_integer(integer)),
        None => Err(NewspeakError::argument_error(
            selector,
            receiver,
            text_value,
            format!(
                "parse:radix: requires the digits of an Integer in radix {radix}, got {}",
                mutator.heap.describe(text_value)
            ),
        )),
    })
}
