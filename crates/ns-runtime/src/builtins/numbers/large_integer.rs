//! `LargeInteger`'s leaves: an integer beyond a `SmallInteger`, held as an
//! `ArbitraryInteger`. The rules across the numeric classes are the module
//! note's.

use super::tower::{self, Comparison};
use super::{
    division_by_zero, float_operand, integer_operand, shift_left, type_mismatch, Accepted, Answer,
};
use crate::builtins::primitive::{LargeIntegerPrimitive, Primitive};
use crate::types::*;
use crate::{ArbitraryInteger, Mutator, Symbol};

pub(crate) fn execute_large_integer<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: LargeIntegerPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    // The payload, never the class: a user subclass's instance, or a
    // `LargeInteger new`, is no number and declines to `Object`'s methods
    // -- `tower::operand` answers `None` for either.
    let comparison = |comparison| tower::compare(&mutator.heap, comparison, receiver, arguments);
    match primitive {
        LargeIntegerPrimitive::Less => return comparison(Comparison::Less),
        LargeIntegerPrimitive::Greater => return comparison(Comparison::Greater),
        LargeIntegerPrimitive::LessOrEqual => return comparison(Comparison::LessOrEqual),
        LargeIntegerPrimitive::GreaterOrEqual => return comparison(Comparison::GreaterOrEqual),
        LargeIntegerPrimitive::Hash => {
            return tower::value_hash(&mutator.heap, receiver)
                .map(|hash| Ok(Value::small_integer(hash)));
        }
        // Never an error: anything that is no number is unequal.
        LargeIntegerPrimitive::Equal => {
            tower::operand(&mutator.heap, receiver)?;
            let equal = tower::values_equal(&mutator.heap, receiver, arguments.first().copied());
            return crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::LargeInteger(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)));
        }
        _ => {}
    }
    let object = receiver.as_object()?;
    let receiver_integer = mutator.heap.large_integer(object)?;
    // Either integer class as the argument, or a `Float` for arithmetic;
    // anything else is the leaf's type mismatch. As in
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
        // Answered above, before the copy: a fall-through here would
        // decline the leaf silently to `Object`'s methods.
        LargeIntegerPrimitive::Less
        | LargeIntegerPrimitive::Greater
        | LargeIntegerPrimitive::LessOrEqual
        | LargeIntegerPrimitive::GreaterOrEqual
        | LargeIntegerPrimitive::Equal
        | LargeIntegerPrimitive::Hash => unreachable!(
            "the comparisons, `=` and `hash` on a LargeInteger are answered by the \
             early returns before its digits are copied"
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
