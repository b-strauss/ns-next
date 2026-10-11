//! The numeric tower's rules, once for every pair of representations:
//! `Integer` (`Small`), `LargeInteger` (`Large`), `Fraction` and `Float`.
//! An argument is classified once (`operand`); comparison and equality go
//! by exact value, a finite double being the dyadic rational it is; the
//! hash is `numeric_hash`'s, which agrees with that equality. The
//! representations' own leaves keep their fast paths and send every other
//! case here, as `Number`'s rows -- which `Fraction` inherits -- send all
//! of theirs (`docs/design/core-library.md`, "The numeric tower").

use std::cmp::Ordering;

use crate::builtins::primitive::{NumberPrimitive, Primitive};
use crate::numeric::{numeric_hash, Rational};
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

use super::{type_mismatch, Accepted};

/// A number, read out of its representation once.
#[derive(Debug, Clone)]
pub(crate) enum Operand {
    Small(i64),
    Large(ArbitraryInteger),
    Fraction(Rational),
    Float(f64),
}

/// The largest integer every smaller one of which a double holds exactly.
const EXACT_DOUBLE_INTEGER: u64 = 1 << 53;

impl Operand {
    /// The exact value; `None` for a NaN or an infinity.
    pub(crate) fn exact(&self) -> Option<Rational> {
        match self {
            Operand::Small(integer) => {
                Some(Rational::from_integer(ArbitraryInteger::from_i64(*integer)))
            }
            Operand::Large(integer) => Some(Rational::from_integer(integer.clone())),
            Operand::Fraction(rational) => Some(rational.clone()),
            Operand::Float(float) => Rational::from_f64(*float),
        }
    }

    /// The nearest double, a tie to even.
    // `to_f64`, `is_float` and `integer` serve the tower's arithmetic,
    // from Task 8 of the numbers plan on.
    #[allow(dead_code)]
    pub(crate) fn to_f64(&self) -> f64 {
        match self {
            // `as` rounds to the nearest double, ties to even.
            Operand::Small(integer) => *integer as f64,
            Operand::Large(integer) => integer.to_f64(),
            Operand::Fraction(rational) => rational.to_f64(),
            Operand::Float(float) => *float,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_float(&self) -> bool {
        matches!(self, Operand::Float(_))
    }

    /// The integer an `Integer` or a `LargeInteger` is; `None` for a
    /// `Fraction` or a `Float`.
    #[allow(dead_code)]
    pub(crate) fn integer(&self) -> Option<ArbitraryInteger> {
        match self {
            Operand::Small(integer) => Some(ArbitraryInteger::from_i64(*integer)),
            Operand::Large(integer) => Some(integer.clone()),
            _ => None,
        }
    }
}

/// `value` as a number; `None` for anything that is no number, `Fraction
/// new` and a `LargeInteger new` among them.
pub(crate) fn operand<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<Operand> {
    match value.kind() {
        ValueKind::SmallInteger(integer) => Some(Operand::Small(integer)),
        ValueKind::SmallFloat(float) => Some(Operand::Float(float)),
        ValueKind::Object(object) => {
            if let Some(float) = heap.float_value(value) {
                return Some(Operand::Float(float));
            }
            if let Some(integer) = heap.large_integer(object) {
                return Some(Operand::Large(integer));
            }
            heap.fraction(object).map(Operand::Fraction)
        }
        _ => None,
    }
}

/// The order of two numbers by their exact values; `None` when either is
/// a NaN, which is ordered with nothing.
pub(crate) fn order(left: &Operand, right: &Operand) -> Option<Ordering> {
    match (left, right) {
        (Operand::Small(first), Operand::Small(second)) => Some(first.cmp(second)),
        (Operand::Float(first), Operand::Float(second)) => first.partial_cmp(second),
        (Operand::Float(float), other) => float_order(*float, other),
        (other, Operand::Float(float)) => float_order(*float, other).map(Ordering::reverse),
        _ => Some(left.exact()?.cmp(&right.exact()?)),
    }
}

/// A double against a number that is no double.
fn float_order(float: f64, other: &Operand) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    if float.is_infinite() {
        return Some(if float > 0.0 {
            Ordering::Greater
        } else {
            Ordering::Less
        });
    }
    // Within 2^53 an integer converts exactly, so the doubles decide.
    if let Operand::Small(integer) = other {
        if integer.unsigned_abs() <= EXACT_DOUBLE_INTEGER {
            return float.partial_cmp(&(*integer as f64));
        }
    }
    Some(Rational::from_f64(float)?.cmp(&other.exact()?))
}

/// `=` across the tower: the same exact value, and a NaN equals a NaN
/// (`docs/design/core-library.md`, "A NaN equals a NaN").
pub(crate) fn equal(left: &Operand, right: &Operand) -> bool {
    match (left, right) {
        (Operand::Float(first), Operand::Float(second)) if first.is_nan() && second.is_nan() => {
            true
        }
        _ => order(left, right) == Some(Ordering::Equal),
    }
}

/// The hash `=` agrees with.
pub(crate) fn hash(operand: &Operand) -> i64 {
    match operand {
        Operand::Small(integer) => numeric_hash::small_integer_hash(*integer),
        Operand::Large(integer) => numeric_hash::integer_hash(integer),
        Operand::Fraction(rational) => numeric_hash::rational_hash(rational),
        Operand::Float(float) => numeric_hash::float_hash(*float),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Comparison {
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
}

impl Comparison {
    fn selector(self) -> &'static str {
        match self {
            Comparison::Less => "<",
            Comparison::Greater => ">",
            Comparison::LessOrEqual => "<=",
            Comparison::GreaterOrEqual => ">=",
        }
    }

    fn holds(self, order: Ordering) -> bool {
        match self {
            Comparison::Less => order.is_lt(),
            Comparison::Greater => order.is_gt(),
            Comparison::LessOrEqual => order.is_le(),
            Comparison::GreaterOrEqual => order.is_ge(),
        }
    }
}

/// An ordering leaf: a Boolean, `false` when either side is a NaN; the
/// type mismatch for an argument that is no number. `None` declines a
/// receiver that is no number.
pub(crate) fn compare<'gc>(
    heap: &Heap<'gc>,
    comparison: Comparison,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let left = operand(heap, receiver)?;
    let argument = arguments.first().copied();
    let Some(right) = argument.and_then(|value| operand(heap, value)) else {
        return Some(Err(type_mismatch(
            heap,
            receiver,
            Symbol::intern(comparison.selector()),
            Accepted::Number,
            argument,
        )));
    };
    Some(Ok(Value::boolean(
        order(&left, &right).is_some_and(|order| comparison.holds(order)),
    )))
}

/// `=` between two values: `false` unless both are numbers of one value.
pub(crate) fn values_equal<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    argument: Option<Value<'gc>>,
) -> bool {
    let Some(left) = operand(heap, receiver) else {
        return false;
    };
    argument
        .and_then(|value| operand(heap, value))
        .is_some_and(|right| equal(&left, &right))
}

/// A number's hash; `None` for anything that is no number.
pub(crate) fn value_hash<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<i64> {
    operand(heap, value).map(|number| hash(&number))
}

/// `Number`'s own rows, which `Fraction` inherits and any other number
/// reaches only through `super`.
pub(crate) fn execute_number<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: NumberPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let comparison = |comparison| compare(&mutator.heap, comparison, receiver, arguments);
    match primitive {
        NumberPrimitive::Less => comparison(Comparison::Less),
        NumberPrimitive::Greater => comparison(Comparison::Greater),
        NumberPrimitive::LessOrEqual => comparison(Comparison::LessOrEqual),
        NumberPrimitive::GreaterOrEqual => comparison(Comparison::GreaterOrEqual),
        NumberPrimitive::Equal => {
            operand(&mutator.heap, receiver)?;
            let equal = values_equal(&mutator.heap, receiver, arguments.first().copied());
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Number(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        NumberPrimitive::Hash => {
            value_hash(&mutator.heap, receiver).map(|hash| Ok(Value::small_integer(hash)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::{equal, hash, order, Operand};
    use crate::numeric::Rational;
    use crate::{ArbitraryInteger, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};

    /// Numbers around every edge the tower has: 2^53, the `SmallInteger`
    /// range, the modulus 2^61 - 1, the largest double and beyond, the
    /// smallest subnormal, the zeros, the infinities, NaN -- each in
    /// every representation that can hold it.
    fn samples() -> Vec<Operand> {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        let power = |exponent: u64| ArbitraryInteger::one().shift_left(exponent).expect("small");
        let fraction = |numerator: ArbitraryInteger, denominator: ArbitraryInteger| {
            Operand::Fraction(Rational::new(numerator, denominator).expect("non-zero"))
        };
        let two_to_the_53 = 1i64 << 53;
        let mut samples = vec![
            Operand::Small(0),
            Operand::Small(1),
            Operand::Small(-1),
            Operand::Small(3),
            Operand::Small(two_to_the_53),
            Operand::Small(two_to_the_53 + 1),
            Operand::Small(-two_to_the_53),
            Operand::Small(SMALL_INTEGER_MAX),
            Operand::Small(SMALL_INTEGER_MIN),
            Operand::Large(integer(i128::from(SMALL_INTEGER_MAX) + 1)),
            Operand::Large(integer((1 << 61) - 1)),
            Operand::Large(power(70)),
            Operand::Large(power(70).add(&integer(1))),
            Operand::Large(power(70).negated()),
            Operand::Large(power(1100)),
            fraction(integer(1), integer(2)),
            fraction(integer(-1), integer(2)),
            fraction(integer(1), integer(3)),
            fraction(integer(i128::from(two_to_the_53) + 1), integer(2)),
            fraction(integer(1), integer((1 << 61) - 1)),
            fraction(integer(1), power(1075)),
            fraction(
                power(1100).add(&integer(1)),
                power(1100).multiply(&integer(3)),
            ),
        ];
        for float in [
            0.0,
            -0.0,
            0.5,
            -0.5,
            3.0,
            1.0 / 3.0,
            2f64.powi(53),
            2f64.powi(60),
            2f64.powi(61),
            2f64.powi(70),
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ] {
            samples.push(Operand::Float(float));
        }
        samples
    }

    fn is_not_a_number(operand: &Operand) -> bool {
        matches!(operand, Operand::Float(float) if float.is_nan())
    }

    #[test]
    fn equality_is_an_equivalence_that_the_order_and_the_hash_agree_with() {
        let samples = samples();
        for left in &samples {
            assert!(equal(left, left), "{left:?} equals itself");
            for right in &samples {
                let same = equal(left, right);
                assert_eq!(
                    same,
                    equal(right, left),
                    "{left:?} = {right:?} is symmetric"
                );
                if same {
                    assert_eq!(hash(left), hash(right), "{left:?} and {right:?} hash alike");
                }
                match order(left, right) {
                    Some(Ordering::Equal) => assert!(same, "{left:?} = {right:?}"),
                    Some(found) => {
                        assert!(!same, "{left:?} and {right:?} are ordered apart");
                        assert_eq!(order(right, left), Some(found.reverse()));
                    }
                    None => assert!(is_not_a_number(left) || is_not_a_number(right)),
                }
                for third in &samples {
                    if same && equal(right, third) {
                        assert!(equal(left, third), "{left:?} = {right:?} = {third:?}");
                    }
                    if order(left, right) == Some(Ordering::Less)
                        && order(right, third) == Some(Ordering::Less)
                    {
                        assert_eq!(order(left, third), Some(Ordering::Less));
                    }
                }
            }
        }
    }

    #[test]
    fn a_double_and_an_integer_compare_exactly_past_two_to_the_53() {
        let two_to_the_53 = 1i64 << 53;
        let rounded = Operand::Float((two_to_the_53 + 1) as f64);
        assert_eq!(
            order(&Operand::Small(two_to_the_53 + 1), &rounded),
            Some(Ordering::Greater)
        );
        assert!(equal(&Operand::Small(two_to_the_53), &rounded));
        assert!(!equal(&Operand::Small(two_to_the_53 + 1), &rounded));
    }
}
