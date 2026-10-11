//! `Fraction`'s leaves: reading a fraction's parts and making one. A
//! fraction is made only here and by the tower's arithmetic, both through
//! `Mutator::new_rational` or `Mutator::new_fraction`, so every one is in
//! normal form (`docs/design/core-library.md`, "The numeric tower").

use crate::builtins::primitive::FractionPrimitive;
use crate::numeric::Rational;
use crate::types::*;
use crate::{ArbitraryInteger, Mutator, Symbol};

use super::integer_operand;

pub(crate) fn execute_fraction<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FractionPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        FractionPrimitive::Numerator => {
            let (numerator, _) = mutator.heap.fraction_parts(receiver.as_object()?)?;
            Some(Ok(numerator))
        }
        FractionPrimitive::Denominator => {
            let (_, denominator) = mutator.heap.fraction_parts(receiver.as_object()?)?;
            Some(Ok(denominator))
        }
        FractionPrimitive::Make => {
            declines_for_another_class(mutator, receiver)?;
            Some(make(mutator, receiver, arguments))
        }
        FractionPrimitive::MakeReduced => {
            declines_for_another_class(mutator, receiver)?;
            Some(make_reduced(mutator, receiver, arguments))
        }
    }
}

/// `Some` when the class-side rows' subject is `Fraction` itself, whose
/// flat class side is the only one these methods are sent from; `None`,
/// declining, for any other subject.
fn declines_for_another_class<'gc>(mutator: &Mutator<'gc>, receiver: Value<'gc>) -> Option<()> {
    let class = receiver.as_object()?;
    (mutator.heap.class_builtin(class) == Some(BuiltinClass::Fraction)).then_some(())
}

/// The two integer arguments, or the `ArgumentError` carrying the first
/// that is none: "<selector> requires integers, got …".
fn integer_pair<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: &str,
    arguments: &[Value<'gc>],
) -> Result<(ArbitraryInteger, ArbitraryInteger), NewspeakError<'gc>> {
    let part = |index: usize| {
        let argument = arguments.get(index).copied();
        integer_operand(&mutator.heap, argument).ok_or_else(|| {
            NewspeakError::argument_error(
                Symbol::intern(selector),
                receiver,
                argument.unwrap_or(Value::nil()),
                format!(
                    "{selector} requires integers, got {}",
                    mutator.heap.describe_argument(argument)
                ),
            )
        })
    };
    Ok((part(0)?, part(1)?))
}

/// `Fraction numerator:denominator:`: the pair made normal, an integer
/// when the denominator divides the numerator.
fn make<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = "numerator:denominator:";
    let (numerator, denominator) = integer_pair(mutator, receiver, selector, arguments)?;
    match Rational::new(numerator, denominator) {
        Some(rational) => Ok(mutator.new_rational(rational)),
        None => Err(NewspeakError::argument_error(
            Symbol::intern(selector),
            receiver,
            arguments.get(1).copied().unwrap_or(Value::nil()),
            "numerator:denominator: requires a non-zero denominator",
        )),
    }
}

/// `Fraction reducedNumerator:denominator:`: the pair as given, which must
/// already be a fraction in normal form -- reduced, its denominator above
/// 1 -- since a fraction outside it would break `=` and `hash`.
fn make_reduced<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let selector = "reducedNumerator:denominator:";
    let (numerator, denominator) = integer_pair(mutator, receiver, selector, arguments)?;
    if denominator.is_one() || !Rational::is_normal(&numerator, &denominator) {
        return Err(NewspeakError::argument_error(
            Symbol::intern(selector),
            receiver,
            arguments.get(1).copied().unwrap_or(Value::nil()),
            format!(
                "{selector} requires a fraction in normal form, got {}/{}",
                numerator.to_bounded_text(),
                denominator.to_bounded_text()
            ),
        ));
    }
    Ok(mutator.new_fraction(Rational::from_normal_parts(numerator, denominator)))
}
