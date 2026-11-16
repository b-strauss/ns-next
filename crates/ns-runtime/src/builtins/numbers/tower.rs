//! The numeric tower's rules, once for every pair of representations:
//! `Integer` (`Small`), `LargeInteger` (`Large`), `Fraction` and `Float`.
//! An argument is classified once (`operand`); comparison and equality go
//! by exact value, a finite double being the dyadic rational it is; the
//! hash is `numeric_hash`'s, which agrees with that equality. The
//! representations' own leaves keep their fast paths and send every other
//! case here, as `Number`'s rows -- which `Fraction` inherits -- send all
//! of theirs (`docs/design/core-library.md`, "The numeric tower").

use crate::interpreter::Arguments;
use std::cmp::Ordering;

use crate::builtins::primitive::{NumberPrimitive, Primitive};
use crate::numeric::{Rational, numeric_hash};
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

use super::{Accepted, type_mismatch};

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
    pub(crate) fn to_f64(&self) -> f64 {
        match self {
            // `as` rounds to the nearest double, ties to even.
            Operand::Small(integer) => *integer as f64,
            Operand::Large(integer) => integer.to_f64(),
            Operand::Fraction(rational) => rational.to_f64(),
            Operand::Float(float) => *float,
        }
    }

    pub(crate) fn is_float(&self) -> bool {
        matches!(self, Operand::Float(_))
    }

    /// The integer an `Integer` or a `LargeInteger` is; `None` for a
    /// `Fraction` or a `Float`.
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
///
/// Inlined, so that a value that is no object -- `nil`, a Boolean, a
/// Character -- is refused without a call: value equality asks this of
/// both sides of every `nil == anObject`.
#[inline(always)]
pub(crate) fn operand<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<Operand> {
    match value.kind() {
        ValueKind::SmallInteger(integer) => Some(Operand::Small(integer)),
        ValueKind::SmallFloat(float) => Some(Operand::Float(float)),
        ValueKind::Object(object) => object_operand(heap, object),
        _ => None,
    }
}

/// `operand` for an object: a boxed `Float`, a `LargeInteger` or a
/// `Fraction`.
fn object_operand<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>) -> Option<Operand> {
    if let Some(float) = heap.float_value(Value::object(object)) {
        return Some(Operand::Float(float));
    }
    if let Some(integer) = heap.large_integer(object) {
        return Some(Operand::Large(integer));
    }
    heap.fraction(object).map(Operand::Fraction)
}

/// The double a leaf's fast path computes with against a `Float`: the
/// `Float` itself, or a `SmallInteger` rounded to the nearest double, which
/// is what `compute` does with a `SmallInteger` beside a `Float`. `None`
/// for anything else, which the leaf leaves to `arithmetic`.
#[inline(always)]
pub(crate) fn arithmetic_double<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<f64> {
    match value.kind() {
        // `as` rounds to the nearest double, ties to even, as `to_f64` does.
        ValueKind::SmallInteger(integer) => Some(integer as f64),
        _ => heap.float_value(value),
    }
}

/// The double a leaf's fast path orders against a `Float` by IEEE 754's
/// comparison and `=`, agreeing with `order` and `equal`: the `Float`
/// itself, or a `SmallInteger` within 2^53, which converts exactly (as
/// `float_order` decides). `None` for anything else -- a `SmallInteger`
/// beyond 2^53 among them, which only the exact comparison orders right --
/// which the leaf leaves to `compare` or `values_equal`.
#[inline(always)]
pub(crate) fn ordering_double<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Option<f64> {
    match value.kind() {
        ValueKind::SmallInteger(integer) => exact_double(integer),
        _ => heap.float_value(value),
    }
}

/// `integer` as a double when the conversion is exact for every integer of
/// its magnitude, so that IEEE 754's comparison of it with any double is
/// the exact comparison; `None` beyond 2^53.
#[inline(always)]
pub(crate) fn exact_double(integer: i64) -> Option<f64> {
    (integer.unsigned_abs() <= EXACT_DOUBLE_INTEGER).then_some(integer as f64)
}

/// The order of two numbers by their exact values; `None` when either is
/// a NaN, which is ordered with nothing.
pub(crate) fn order(left: &Operand, right: &Operand) -> Option<Ordering> {
    match (left, right) {
        (Operand::Small(first), Operand::Small(second)) => Some(first.cmp(second)),
        // Two integers compare as integers: no `Rational` is built.
        (Operand::Large(first), Operand::Large(second)) => Some(first.cmp(second)),
        (Operand::Small(first), Operand::Large(second)) => {
            Some(ArbitraryInteger::from_i64(*first).cmp(second))
        }
        (Operand::Large(first), Operand::Small(second)) => {
            Some(first.cmp(&ArbitraryInteger::from_i64(*second)))
        }
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
    if let Some(double) = match other {
        Operand::Small(integer) => exact_double(*integer),
        _ => None,
    } {
        return float.partial_cmp(&double);
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

/// The tower's arithmetic operations.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Arithmetic {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Quotient,
    Remainder,
}

impl Arithmetic {
    fn selector(self) -> &'static str {
        match self {
            Arithmetic::Add => "+",
            Arithmetic::Subtract => "-",
            Arithmetic::Multiply => "*",
            Arithmetic::Divide => "/",
            Arithmetic::FloorDivide => "//",
            Arithmetic::Modulo => "\\\\",
            Arithmetic::Quotient => "quo:",
            Arithmetic::Remainder => "rem:",
        }
    }
}

/// An arithmetic answer before it is a `Value`.
#[derive(Debug)]
enum Outcome {
    Integer(ArbitraryInteger),
    Rational(Rational),
    Float(f64),
}

/// The one refusal arithmetic has.
#[derive(Debug)]
struct DivisionByZero;

/// `operation` over two numbers: over doubles when either is a `Float`,
/// exactly otherwise.
fn compute(
    operation: Arithmetic,
    left: &Operand,
    right: &Operand,
) -> Result<Outcome, DivisionByZero> {
    if left.is_float() || right.is_float() {
        return float_compute(operation, left.to_f64(), right.to_f64());
    }
    if let (Some(dividend), Some(divisor)) = (left.integer(), right.integer()) {
        return integer_compute(operation, &dividend, &divisor);
    }
    let first = left.exact().expect("no Float, so finite");
    let second = right.exact().expect("no Float, so finite");
    let floor = |quotient: Rational| quotient.floor();
    let truncate = |quotient: Rational| quotient.truncated();
    let rounded_quotient = |round: fn(Rational) -> ArbitraryInteger| {
        first.divide(&second).map(round).ok_or(DivisionByZero)
    };
    let remainder_after = |quotient: ArbitraryInteger| {
        first.subtract(&Rational::from_integer(quotient).multiply(&second))
    };
    Ok(match operation {
        Arithmetic::Add => Outcome::Rational(first.add(&second)),
        Arithmetic::Subtract => Outcome::Rational(first.subtract(&second)),
        Arithmetic::Multiply => Outcome::Rational(first.multiply(&second)),
        Arithmetic::Divide => Outcome::Rational(first.divide(&second).ok_or(DivisionByZero)?),
        Arithmetic::FloorDivide => Outcome::Integer(rounded_quotient(floor)?),
        Arithmetic::Quotient => Outcome::Integer(rounded_quotient(truncate)?),
        Arithmetic::Modulo => Outcome::Rational(remainder_after(rounded_quotient(floor)?)),
        Arithmetic::Remainder => Outcome::Rational(remainder_after(rounded_quotient(truncate)?)),
    })
}

/// Two integers: `ArbitraryInteger`'s own operations, a quotient that
/// does not divide made a `Rational`.
fn integer_compute(
    operation: Arithmetic,
    dividend: &ArbitraryInteger,
    divisor: &ArbitraryInteger,
) -> Result<Outcome, DivisionByZero> {
    Ok(Outcome::Integer(match operation {
        Arithmetic::Add => dividend.add(divisor),
        Arithmetic::Subtract => dividend.subtract(divisor),
        Arithmetic::Multiply => dividend.multiply(divisor),
        Arithmetic::Divide => {
            return Rational::new(dividend.clone(), divisor.clone())
                .map(Outcome::Rational)
                .ok_or(DivisionByZero);
        }
        Arithmetic::FloorDivide => dividend.floor_divide(divisor).ok_or(DivisionByZero)?,
        Arithmetic::Modulo => dividend.floor_modulo(divisor).ok_or(DivisionByZero)?,
        Arithmetic::Quotient => dividend.truncated_divide(divisor).ok_or(DivisionByZero)?.0,
        Arithmetic::Remainder => dividend.truncated_divide(divisor).ok_or(DivisionByZero)?.1,
    }))
}

/// Two doubles: IEEE 754's arithmetic; `//` and its kin refuse a zero
/// divisor where `/` answers an infinity or a NaN.
fn float_compute(operation: Arithmetic, left: f64, right: f64) -> Result<Outcome, DivisionByZero> {
    let rounding = matches!(
        operation,
        Arithmetic::FloorDivide | Arithmetic::Modulo | Arithmetic::Quotient | Arithmetic::Remainder
    );
    if rounding && right == 0.0 {
        return Err(DivisionByZero);
    }
    Ok(Outcome::Float(match operation {
        Arithmetic::Add => left + right,
        Arithmetic::Subtract => left - right,
        Arithmetic::Multiply => left * right,
        Arithmetic::Divide => left / right,
        Arithmetic::FloorDivide => (left / right).floor(),
        Arithmetic::Quotient => (left / right).trunc(),
        // `%` is C's `fmod`: exact, with the dividend's sign.
        Arithmetic::Remainder => left % right,
        Arithmetic::Modulo => {
            let remainder = left % right;
            if remainder != 0.0 && (remainder < 0.0) != (right < 0.0) {
                remainder + right
            } else {
                remainder
            }
        }
    }))
}

fn answer<'gc>(mutator: &mut Mutator<'gc>, outcome: Outcome) -> Value<'gc> {
    match outcome {
        Outcome::Integer(integer) => mutator.new_integer(integer),
        Outcome::Rational(rational) => mutator.new_rational(rational),
        Outcome::Float(float) => mutator.new_float(float),
    }
}

/// An arithmetic leaf: the answer, the type mismatch for an argument that
/// is no number, or "division by zero" carrying the divisor. `None`
/// declines a receiver that is no number.
pub(crate) fn arithmetic<'gc>(
    mutator: &mut Mutator<'gc>,
    operation: Arithmetic,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let left = operand(&mutator.heap, receiver)?;
    let argument = arguments.first(&mutator.heap);
    let Some(right) = argument.and_then(|value| operand(&mutator.heap, value)) else {
        return Some(Err(type_mismatch(
            &mutator.heap,
            receiver,
            Symbol::intern(operation.selector()),
            Accepted::Number,
            argument,
        )));
    };
    Some(match compute(operation, &left, &right) {
        Ok(outcome) => Ok(answer(mutator, outcome)),
        Err(DivisionByZero) => Err(super::division_by_zero(
            receiver,
            operation.selector(),
            argument.unwrap_or(Value::nil()),
        )),
    })
}

/// `Number>>asFloat`: the nearest Float, in one rounding.
pub(crate) fn as_float<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
) -> Option<Value<'gc>> {
    let number = operand(&mutator.heap, receiver)?;
    Some(mutator.new_float(number.to_f64()))
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
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let left = operand(heap, receiver)?;
    let argument = arguments.first(heap);
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
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let comparison = |comparison| compare(&mutator.heap, comparison, receiver, arguments);
    match primitive {
        NumberPrimitive::Less => comparison(Comparison::Less),
        NumberPrimitive::Greater => comparison(Comparison::Greater),
        NumberPrimitive::LessOrEqual => comparison(Comparison::LessOrEqual),
        NumberPrimitive::GreaterOrEqual => comparison(Comparison::GreaterOrEqual),
        NumberPrimitive::Equal => {
            operand(&mutator.heap, receiver)?;
            let equal = values_equal(&mutator.heap, receiver, arguments.first(&mutator.heap));
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
        NumberPrimitive::Add => arithmetic(mutator, Arithmetic::Add, receiver, arguments),
        NumberPrimitive::Subtract => arithmetic(mutator, Arithmetic::Subtract, receiver, arguments),
        NumberPrimitive::Multiply => arithmetic(mutator, Arithmetic::Multiply, receiver, arguments),
        NumberPrimitive::Divide => arithmetic(mutator, Arithmetic::Divide, receiver, arguments),
        NumberPrimitive::FloorDivide => {
            arithmetic(mutator, Arithmetic::FloorDivide, receiver, arguments)
        }
        NumberPrimitive::Modulo => arithmetic(mutator, Arithmetic::Modulo, receiver, arguments),
        NumberPrimitive::Quotient => arithmetic(mutator, Arithmetic::Quotient, receiver, arguments),
        NumberPrimitive::Remainder => {
            arithmetic(mutator, Arithmetic::Remainder, receiver, arguments)
        }
        NumberPrimitive::AsFloat => as_float(mutator, receiver).map(Ok),
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::{Operand, equal, hash, order};
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

    /// The `Float` leaf's fast path computes and compares a `Float` with a
    /// `SmallInteger` as doubles (`arithmetic_double`, `ordering_double`):
    /// it must answer what the tower answers for every pair it takes.
    #[test]
    fn the_float_fast_path_agrees_with_the_tower() {
        use super::{Arithmetic, Outcome, compute, exact_double};
        let samples = samples();
        let floats = samples.iter().filter_map(|sample| match sample {
            Operand::Float(float) => Some(*float),
            _ => None,
        });
        let two_to_the_53 = 1i64 << 53;
        let mut integers: Vec<i64> = samples
            .iter()
            .filter_map(|sample| match sample {
                Operand::Small(integer) => Some(*integer),
                _ => None,
            })
            .collect();
        integers.extend([two_to_the_53 - 1, -two_to_the_53 - 1, 7, -7]);
        for float in floats {
            for &integer in &integers {
                let left = Operand::Float(float);
                let right = Operand::Small(integer);
                for (operation, fast) in [
                    (Arithmetic::Add, float + integer as f64),
                    (Arithmetic::Subtract, float - integer as f64),
                    (Arithmetic::Multiply, float * integer as f64),
                    (Arithmetic::Divide, float / integer as f64),
                ] {
                    match compute(operation, &left, &right) {
                        Ok(Outcome::Float(tower)) => assert_eq!(
                            tower.to_bits(),
                            fast.to_bits(),
                            "{float} {operation:?} {integer}"
                        ),
                        _ => panic!("{float} {operation:?} {integer} answers a Float"),
                    }
                }
                match exact_double(integer) {
                    Some(double) => {
                        assert_eq!(
                            float.partial_cmp(&double),
                            order(&left, &right),
                            "{float} against {integer}"
                        );
                        assert_eq!(
                            float == double || (float.is_nan() && double.is_nan()),
                            equal(&left, &right),
                            "{float} = {integer}"
                        );
                    }
                    None => assert!(integer.unsigned_abs() > two_to_the_53.unsigned_abs()),
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

    #[test]
    fn the_rounding_divisions_agree_with_their_definitions() {
        use super::{Arithmetic, compute};
        let integer = |value: i64| Operand::Small(value);
        let fraction = |numerator: i128, denominator: i128| {
            Operand::Fraction(
                Rational::new(
                    ArbitraryInteger::from_i128(numerator),
                    ArbitraryInteger::from_i128(denominator),
                )
                .expect("non-zero"),
            )
        };
        let exact = |outcome: Result<super::Outcome, super::DivisionByZero>| match outcome {
            Ok(super::Outcome::Integer(value)) => Rational::from_integer(value),
            Ok(super::Outcome::Rational(value)) => value,
            _ => panic!("an exact outcome"),
        };
        let minus_seven_halves = fraction(-7, 2);
        assert_eq!(
            exact(compute(
                Arithmetic::FloorDivide,
                &minus_seven_halves,
                &integer(1)
            )),
            Rational::from_integer(ArbitraryInteger::from_i64(-4))
        );
        assert_eq!(
            exact(compute(
                Arithmetic::Modulo,
                &minus_seven_halves,
                &integer(1)
            )),
            Rational::new(ArbitraryInteger::from_i64(1), ArbitraryInteger::from_i64(2))
                .expect("non-zero")
        );
        assert_eq!(
            exact(compute(
                Arithmetic::Quotient,
                &minus_seven_halves,
                &integer(1)
            )),
            Rational::from_integer(ArbitraryInteger::from_i64(-3))
        );
        assert_eq!(
            exact(compute(
                Arithmetic::Remainder,
                &minus_seven_halves,
                &integer(1)
            )),
            Rational::new(
                ArbitraryInteger::from_i64(-1),
                ArbitraryInteger::from_i64(2)
            )
            .expect("non-zero")
        );
        assert!(compute(Arithmetic::Divide, &integer(1), &integer(0)).is_err());
        assert!(
            compute(
                Arithmetic::Modulo,
                &Operand::Float(1.5),
                &Operand::Float(0.0)
            )
            .is_err()
        );
        assert!(matches!(
            compute(Arithmetic::Divide, &Operand::Float(1.0), &integer(0)),
            Ok(super::Outcome::Float(value)) if value == f64::INFINITY
        ));
    }
}
