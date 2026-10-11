//! `LargeInteger`'s leaves: an integer beyond a `SmallInteger`, held as an
//! `ArbitraryInteger`. The rules across the numeric classes are the module
//! note's.

use std::cmp::Ordering;

use super::{
    division_by_zero, float_operand, integer_operand, integer_ordering, shift_left, type_mismatch,
    Accepted, Answer,
};
use crate::builtins::primitive::{LargeIntegerPrimitive, Primitive};
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

/// `LargeInteger>>hash`, and the derived hash of a value slot holding one:
/// the sign and magnitude's content hash, folded to 32 bits as
/// `content_hash` folds every content hash, which `builtins::value`'s sum
/// over a value's slots relies on to stay inside an `i64`.
pub(crate) fn large_integer_hash(negative: bool, digits: &[u64]) -> i64 {
    crate::hash::content_hash((negative, digits))
}

/// `LargeInteger>>=`'s rule: only a `LargeInteger` of the same value is
/// equal. A `SmallInteger` never is, because `new_integer` never makes a
/// `LargeInteger` of a value that fits.
///
/// Both read in place: each integer's magnitude is canonical, so equal
/// signs and digits are equal values.
pub(crate) fn large_integer_equals<'gc>(
    heap: &Heap<'gc>,
    receiver: (bool, &[u64]),
    argument: Option<Value<'gc>>,
) -> bool {
    argument
        .and_then(|value| value.as_object())
        .and_then(|object| {
            heap.with_large_integer(object, |negative, digits| (negative, digits) == receiver)
        })
        .unwrap_or(false)
}

pub(crate) fn execute_large_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: LargeIntegerPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    // The payload, never the class: a user subclass's instance, or a
    // `LargeInteger new`, is no number and declines to `Object`'s methods.
    let object = receiver.as_object()?;
    // `=`, `hash` and an ordering against an integer only read the digits,
    // so they read them in place; the rest copy them into the
    // `ArbitraryInteger` the arithmetic takes.
    let argument_value = arguments.first().copied();
    match primitive {
        LargeIntegerPrimitive::Hash => {
            return mutator
                .heap
                .with_large_integer(object, large_integer_hash)
                .map(|hash| Ok(Value::small_integer(hash)));
        }
        LargeIntegerPrimitive::Equal => {
            let equal = mutator
                .heap
                .with_large_integer(object, |negative, digits| {
                    large_integer_equals(&mutator.heap, (negative, digits), argument_value)
                })?;
            return crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::LargeInteger(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)));
        }
        LargeIntegerPrimitive::Less
        | LargeIntegerPrimitive::Greater
        | LargeIntegerPrimitive::LessOrEqual
        | LargeIntegerPrimitive::GreaterOrEqual => {
            let order = mutator
                .heap
                .with_large_integer(object, |negative, digits| {
                    integer_ordering(&mutator.heap, (negative, digits), argument_value)
                })?;
            if let Some(order) = order {
                let operation: fn(Ordering) -> bool = match primitive {
                    LargeIntegerPrimitive::Less => Ordering::is_lt,
                    LargeIntegerPrimitive::Greater => Ordering::is_gt,
                    LargeIntegerPrimitive::LessOrEqual => Ordering::is_le,
                    _ => Ordering::is_ge,
                };
                return Some(Ok(Value::boolean(operation(order))));
            }
            // No integer argument: a `Float` one converts the receiver, and
            // anything else is the mismatch, both below.
        }
        _ => {}
    }
    let receiver_integer = mutator.heap.large_integer(object)?;
    // Either integer class as the argument, or a `Float` for arithmetic and
    // ordering; anything else is the leaf's type mismatch. As in
    // `execute_integer`, these closures hold `mutator` shared, so each answers
    // an `ArbitraryInteger` or a double for the caller to make a `Value` of.
    let argument = || integer_operand(&mutator.heap, arguments.first().copied());
    let mismatch = |operator: &str, accepted: Accepted| {
        type_mismatch(
            &mutator.heap,
            receiver,
            Symbol::intern(operator),
            accepted,
            arguments.first().copied(),
        )
    };
    let arithmetic = |operator: &str,
                      operation: fn(&ArbitraryInteger, &ArbitraryInteger) -> ArbitraryInteger,
                      float: fn(f64, f64) -> f64| {
        match argument() {
            Some(argument) => Ok(Answer::Integer(operation(&receiver_integer, &argument))),
            None => match float_operand(&mutator.heap, arguments.first().copied()) {
                Some(argument) => Ok(Answer::Float(float(receiver_integer.to_f64(), argument))),
                None => Err(mismatch(operator, Accepted::Number)),
            },
        }
    };
    // Reached only when the argument is no integer: an integer one was
    // ordered in place above.
    let compare = |operator: &str, operation: fn(Ordering) -> bool| match float_operand(
        &mutator.heap,
        arguments.first().copied(),
    ) {
        Some(argument) => Ok(Value::boolean(
            receiver_integer
                .to_f64()
                .partial_cmp(&argument)
                .is_some_and(operation),
        )),
        None => Err(mismatch(operator, Accepted::Number)),
    };
    match primitive {
        LargeIntegerPrimitive::Add => Some(
            arithmetic("+", ArbitraryInteger::add, |left, right| left + right)
                .map(|answer| answer.into_value(mutator)),
        ),
        LargeIntegerPrimitive::Subtract => Some(
            arithmetic("-", ArbitraryInteger::subtract, |left, right| left - right)
                .map(|answer| answer.into_value(mutator)),
        ),
        LargeIntegerPrimitive::Multiply => Some(
            arithmetic("*", ArbitraryInteger::multiply, |left, right| left * right)
                .map(|answer| answer.into_value(mutator)),
        ),
        LargeIntegerPrimitive::FloorDivide => Some(match argument() {
            Some(divisor) => match receiver_integer.floor_divide(&divisor) {
                Some(quotient) => Ok(mutator.new_integer(quotient)),
                None => Err(division_by_zero(receiver, "//")),
            },
            None => Err(mismatch("//", Accepted::Integer)),
        }),
        LargeIntegerPrimitive::Modulo => Some(match argument() {
            Some(divisor) => match receiver_integer.floor_modulo(&divisor) {
                Some(remainder) => Ok(mutator.new_integer(remainder)),
                None => Err(division_by_zero(receiver, "\\\\")),
            },
            None => Err(mismatch("\\\\", Accepted::Integer)),
        }),
        LargeIntegerPrimitive::BitAnd => Some(match argument() {
            Some(argument) => Ok(mutator.new_integer(receiver_integer.bit_and(&argument))),
            None => Err(mismatch("bitAnd:", Accepted::Integer)),
        }),
        LargeIntegerPrimitive::BitOr => Some(match argument() {
            Some(argument) => Ok(mutator.new_integer(receiver_integer.bit_or(&argument))),
            None => Err(mismatch("bitOr:", Accepted::Integer)),
        }),
        LargeIntegerPrimitive::BitXor => Some(match argument() {
            Some(argument) => Ok(mutator.new_integer(receiver_integer.bit_xor(&argument))),
            None => Err(mismatch("bitXor:", Accepted::Integer)),
        }),
        LargeIntegerPrimitive::ShiftLeft => {
            Some(shift_left(mutator, receiver, &receiver_integer, arguments))
        }
        LargeIntegerPrimitive::Less => Some(compare("<", Ordering::is_lt)),
        LargeIntegerPrimitive::Greater => Some(compare(">", Ordering::is_gt)),
        LargeIntegerPrimitive::LessOrEqual => Some(compare("<=", Ordering::is_le)),
        LargeIntegerPrimitive::GreaterOrEqual => Some(compare(">=", Ordering::is_ge)),
        // Answered in place above, before the copy: a fall-through here
        // would decline the leaf silently to `Object`'s `=` and `hash`.
        LargeIntegerPrimitive::Equal | LargeIntegerPrimitive::Hash => unreachable!(
            "`=` and `hash` on a LargeInteger are answered by the early return \
             before its digits are copied"
        ),
        LargeIntegerPrimitive::PrintString => Some(Ok(Value::object(
            mutator.new_string(receiver_integer.to_decimal_string()),
        ))),
        LargeIntegerPrimitive::AsFloat => Some(Ok(mutator.new_float(receiver_integer.to_f64()))),
        LargeIntegerPrimitive::Negated => Some(Ok(mutator.new_integer(receiver_integer.negated()))),
        LargeIntegerPrimitive::Absolute => {
            Some(Ok(mutator.new_integer(receiver_integer.absolute())))
        }
    }
}
