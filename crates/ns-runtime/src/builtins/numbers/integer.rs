//! `Integer`'s leaves: the `i64` that fits a `SmallInteger`. The rules across
//! the numeric classes are the module note's.

use super::{
    checked_floor_divide, division_by_zero, float_operand, floor_modulo, integer_operand,
    integer_ordering, promote, shift_left, type_mismatch, with_small_integer_parts, Accepted,
    Answer,
};
use crate::builtins::first_argument;
use crate::builtins::primitive::{AnyIntegerPrimitive, IntegerPrimitive, Primitive};
use crate::types::*;
use crate::{ArbitraryInteger, Mutator, Symbol};
use std::cmp::Ordering;

/// `Integer>>=`'s rule: an argument of any other type is plainly unequal,
/// never an error (this module's note says why). Named so that
/// `builtins::value`'s derived equality decides an integer exactly as
/// `Integer>>=` does rather than growing a second copy of the rule.
pub(crate) fn integer_equals<'gc>(receiver: i64, argument: Option<Value<'gc>>) -> bool {
    argument.and_then(|value| value.as_small_integer()) == Some(receiver)
}

pub(crate) fn execute_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: IntegerPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let receiver_integer = receiver.as_small_integer()?;
    let argument_integer = || arguments.first().and_then(|value| value.as_small_integer());
    let mismatch = |operator: &str, accepted: Accepted| {
        type_mismatch(
            &mutator.heap,
            receiver,
            Symbol::intern(operator),
            accepted,
            arguments.first().copied(),
        )
    };
    // A `LargeInteger` argument, read only once the argument is known to be
    // no `SmallInteger`, so the common case never touches the heap.
    let large_argument = || integer_operand(&mutator.heap, arguments.first().copied());
    // `+`/`-`/`*`: an argument of neither integer class nor `Float` is a type
    // mismatch.
    // A result that fits a `SmallInteger` is answered without allocating.
    // One that does not is recomputed in `i128`, where two `i64`s' sum,
    // difference or product always fits, and one with a `LargeInteger`
    // argument as an `ArbitraryInteger`; either is handed back as an
    // `Answer::Integer` for the caller to make a `Value` of -- the closure
    // holds `mutator` shared, so it cannot allocate the `LargeInteger`
    // itself. A `Float` argument makes it a double operation on the
    // converted receiver, handed back as an `Answer::Float` for the same
    // reason.
    let arithmetic = |operator: &str,
                      tagged: fn(Value<'gc>, Value<'gc>) -> Option<Value<'gc>>,
                      wide: fn(i128, i128) -> i128,
                      large: fn(&ArbitraryInteger, &ArbitraryInteger) -> ArbitraryInteger,
                      float: fn(f64, f64) -> f64| {
        match argument_integer() {
            Some(argument) => Ok(match tagged(receiver, arguments[0]) {
                Some(result) => Answer::Value(result),
                None => Answer::Integer(ArbitraryInteger::from_i128(wide(
                    i128::from(receiver_integer),
                    i128::from(argument),
                ))),
            }),
            None => match large_argument() {
                Some(argument) => Ok(Answer::Integer(large(
                    &ArbitraryInteger::from_i64(receiver_integer),
                    &argument,
                ))),
                None => match float_operand(&mutator.heap, arguments.first().copied()) {
                    Some(argument) => Ok(Answer::Float(float(receiver_integer as f64, argument))),
                    None => Err(mismatch(operator, Accepted::Number)),
                },
            },
        }
    };
    // Ordering comparisons: an argument of the wrong type is an error, not a
    // silent `false`. `partial_cmp` answers `None` for a NaN, which is
    // ordered with nothing, as `Float`'s own ordering answers.
    let compare = |operator: &str, operation: fn(Ordering) -> bool| match arguments
        .first()
        .and_then(|argument| receiver.small_integer_order(*argument))
    {
        Some(order) => Ok(Value::boolean(operation(order))),
        None => match with_small_integer_parts(receiver_integer, |receiver_parts| {
            integer_ordering(&mutator.heap, receiver_parts, arguments.first().copied())
        }) {
            Some(order) => Ok(Value::boolean(operation(order))),
            None => match float_operand(&mutator.heap, arguments.first().copied()) {
                Some(argument) => Ok(Value::boolean(
                    (receiver_integer as f64)
                        .partial_cmp(&argument)
                        .is_some_and(operation),
                )),
                None => Err(mismatch(operator, Accepted::Number)),
            },
        },
    };
    match primitive {
        IntegerPrimitive::Add => Some(
            arithmetic(
                "+",
                Value::small_integer_sum,
                |left, right| left + right,
                ArbitraryInteger::add,
                |left, right| left + right,
            )
            .map(|answer| answer.into_value(mutator)),
        ),
        IntegerPrimitive::Subtract => Some(
            arithmetic(
                "-",
                Value::small_integer_difference,
                |left, right| left - right,
                ArbitraryInteger::subtract,
                |left, right| left - right,
            )
            .map(|answer| answer.into_value(mutator)),
        ),
        IntegerPrimitive::Multiply => Some(
            arithmetic(
                "*",
                Value::small_integer_product,
                |left, right| left * right,
                ArbitraryInteger::multiply,
                |left, right| left * right,
            )
            .map(|answer| answer.into_value(mutator)),
        ),
        IntegerPrimitive::FloorDivide => Some(match argument_integer() {
            Some(0) => Err(division_by_zero(receiver, "//")),
            // With the zero divisor refused above, the one quotient that
            // does not fit is `SMALL_INTEGER_MIN // -1`'s, which is
            // `-SMALL_INTEGER_MIN`.
            Some(divisor) => Ok(
                match checked_floor_divide(receiver_integer, divisor)
                    .filter(|number| fits_small_integer(*number))
                {
                    Some(quotient) => Value::small_integer(quotient),
                    None => promote(mutator, -i128::from(receiver_integer)),
                },
            ),
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
                None => Err(mismatch("//", Accepted::Integer)),
            },
        }),
        IntegerPrimitive::Modulo => Some(match argument_integer() {
            Some(0) => Err(division_by_zero(receiver, "\\\\")),
            Some(divisor) => Ok(Value::small_integer(floor_modulo(
                receiver_integer,
                divisor,
            ))),
            None => match large_argument() {
                Some(divisor) => {
                    match ArbitraryInteger::from_i64(receiver_integer).floor_modulo(&divisor) {
                        Some(remainder) => Ok(mutator.new_integer(remainder)),
                        None => Err(division_by_zero(receiver, "\\\\")),
                    }
                }
                None => Err(mismatch("\\\\", Accepted::Integer)),
            },
        }),
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
        IntegerPrimitive::Less => Some(compare("<", Ordering::is_lt)),
        IntegerPrimitive::Greater => Some(compare(">", Ordering::is_gt)),
        IntegerPrimitive::LessOrEqual => Some(compare("<=", Ordering::is_le)),
        IntegerPrimitive::GreaterOrEqual => Some(compare(">=", Ordering::is_ge)),
        // Deliberately *not* an error on a type mismatch -- see the
        // module-level note.
        IntegerPrimitive::Equal => crate::interpreter::future::declines_for_a_future(
            mutator,
            Primitive::Integer(primitive),
            integer_equals(receiver_integer, arguments.first().copied()),
            receiver,
            arguments,
        )
        .map(|answer| Ok(Value::boolean(answer))),
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

/// The leaves that take an integer receiver of either representation, so
/// that `LargeInteger` inherits `Integer`'s methods for them, and `parse:`,
/// whose subject is the `Integer` class. `None` declines a receiver that is
/// neither.
pub(crate) fn execute_any_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: AnyIntegerPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
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
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let other = integer_argument(
        mutator,
        receiver,
        Symbol::intern("gcd:"),
        arguments.first().copied(),
    )?;
    Ok(mutator.new_integer(integer.gcd(&other)))
}

/// `>>`: a count past `u64` shifts every bit out, as `u64::MAX` does.
fn shift_right<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    integer: &ArbitraryInteger,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = Symbol::intern(">>");
    let count = integer_argument(mutator, receiver, selector, arguments.first().copied())?;
    if count.is_negative() {
        return Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(arguments),
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
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = Symbol::intern("digitAt:");
    let index = integer_argument(mutator, receiver, selector, arguments.first().copied())?;
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
            first_argument(arguments),
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
    match integer.to_u64() {
        Some(radix_number @ 2..=36) => Ok(radix_number as u32),
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
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let radix = radix_argument(
        mutator,
        receiver,
        Symbol::intern("asStringRadix:"),
        first_argument(arguments),
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
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::Integer) {
        return None;
    }
    let selector = Symbol::intern("parse:radix:");
    let text_value = first_argument(arguments);
    let radix_value = arguments.get(1).copied().unwrap_or(Value::nil());
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
