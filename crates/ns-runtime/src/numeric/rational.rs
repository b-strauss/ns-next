//! Exact rational numbers in the normal form a `Fraction` holds and the
//! tower computes in: reduced by their gcd, the denominator positive, the
//! sign on the numerator. An integer is one whose denominator is 1; a
//! `Fraction` never is (`docs/design/core-library.md`, "The numeric
//! tower").

use std::cmp::Ordering;

use crate::ArbitraryInteger;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rational {
    numerator: ArbitraryInteger,
    denominator: ArbitraryInteger,
}

/// The exponent of the largest double's leading bit, and of the smallest
/// normal double's.
const MAXIMUM_EXPONENT: i64 = 1023;
const MINIMUM_NORMAL_EXPONENT: i64 = -1022;
/// A double's significant bits, the leading one included.
const SIGNIFICANT_BITS: i64 = 53;
/// The exponent of the smallest subnormal double, 2^-1074.
const SMALLEST_EXPONENT: i64 = -1074;

impl Rational {
    pub(crate) fn from_integer(integer: ArbitraryInteger) -> Rational {
        Rational {
            numerator: integer,
            denominator: ArbitraryInteger::one(),
        }
    }

    /// `numerator / denominator` in normal form; `None` for a zero
    /// denominator.
    pub(crate) fn new(
        numerator: ArbitraryInteger,
        denominator: ArbitraryInteger,
    ) -> Option<Rational> {
        if denominator.is_zero() {
            return None;
        }
        // Not zero, since the denominator is not.
        let divisor = numerator.gcd(&denominator);
        let (mut numerator, _) = numerator.truncated_divide(&divisor)?;
        let (mut denominator, _) = denominator.truncated_divide(&divisor)?;
        if denominator.is_negative() {
            numerator = numerator.negated();
            denominator = denominator.negated();
        }
        Some(Rational {
            numerator,
            denominator,
        })
    }

    /// Whether `numerator / denominator` is already in normal form: a
    /// positive denominator and no common factor.
    pub(crate) fn is_normal(numerator: &ArbitraryInteger, denominator: &ArbitraryInteger) -> bool {
        !denominator.is_negative() && !denominator.is_zero() && numerator.gcd(denominator).is_one()
    }

    /// A rational from parts known to be in normal form: a fraction read
    /// back from the heap, which only `new` made.
    pub(crate) fn from_normal_parts(
        numerator: ArbitraryInteger,
        denominator: ArbitraryInteger,
    ) -> Rational {
        debug_assert!(Rational::is_normal(&numerator, &denominator));
        Rational {
            numerator,
            denominator,
        }
    }

    pub(crate) fn numerator(&self) -> &ArbitraryInteger {
        &self.numerator
    }

    pub(crate) fn denominator(&self) -> &ArbitraryInteger {
        &self.denominator
    }

    /// The numerator and the denominator, given up rather than copied, for
    /// a caller that stores them (`Mutator::new_fraction`).
    pub(crate) fn into_parts(self) -> (ArbitraryInteger, ArbitraryInteger) {
        (self.numerator, self.denominator)
    }

    pub(crate) fn is_integer(&self) -> bool {
        self.denominator.is_one()
    }

    pub(crate) fn add(&self, other: &Rational) -> Rational {
        self.combine(other, ArbitraryInteger::add)
    }

    pub(crate) fn subtract(&self, other: &Rational) -> Rational {
        self.combine(other, ArbitraryInteger::subtract)
    }

    /// `a/b ± c/d` as `(a·d ± c·b) / (b·d)`, reduced.
    fn combine(
        &self,
        other: &Rational,
        operation: fn(&ArbitraryInteger, &ArbitraryInteger) -> ArbitraryInteger,
    ) -> Rational {
        let numerator = operation(
            &self.numerator.multiply(&other.denominator),
            &other.numerator.multiply(&self.denominator),
        );
        Rational::new(numerator, self.denominator.multiply(&other.denominator))
            .expect("a product of positive denominators is not zero")
    }

    pub(crate) fn multiply(&self, other: &Rational) -> Rational {
        Rational::new(
            self.numerator.multiply(&other.numerator),
            self.denominator.multiply(&other.denominator),
        )
        .expect("a product of positive denominators is not zero")
    }

    /// The quotient; `None` for a zero divisor.
    pub(crate) fn divide(&self, other: &Rational) -> Option<Rational> {
        Rational::new(
            self.numerator.multiply(&other.denominator),
            self.denominator.multiply(&other.numerator),
        )
    }

    /// The greatest integer not above the value.
    pub(crate) fn floor(&self) -> ArbitraryInteger {
        self.numerator
            .floor_divide(&self.denominator)
            .expect("a positive denominator")
    }

    /// The value without its fraction, toward zero.
    pub(crate) fn truncated(&self) -> ArbitraryInteger {
        self.numerator
            .truncated_divide(&self.denominator)
            .expect("a positive denominator")
            .0
    }

    /// The exact value of a finite double, which is a dyadic rational:
    /// its significand over a power of two, or an integer. `None` for a
    /// NaN or an infinity.
    pub(crate) fn from_f64(value: f64) -> Option<Rational> {
        if !value.is_finite() {
            return None;
        }
        let bits = value.to_bits();
        let negative = bits >> 63 == 1;
        let biased_exponent = ((bits >> 52) & 0x7ff) as i64;
        let fraction = bits & ((1 << 52) - 1);
        let (significand, exponent) = if biased_exponent == 0 {
            (fraction, SMALLEST_EXPONENT)
        } else {
            (fraction | (1 << 52), biased_exponent - 1075)
        };
        if significand == 0 {
            return Some(Rational::from_integer(ArbitraryInteger::zero()));
        }
        // An odd significand over a power of two is already reduced.
        let trailing = significand.trailing_zeros();
        let magnitude = ArbitraryInteger::from_u64(significand >> trailing);
        let numerator = if negative {
            magnitude.negated()
        } else {
            magnitude
        };
        let exponent = exponent + i64::from(trailing);
        Some(if exponent >= 0 {
            Rational::from_integer(
                numerator
                    .shift_left(exponent as u64)
                    .expect("a double's integer has at most 1024 bits"),
            )
        } else {
            Rational {
                numerator,
                denominator: ArbitraryInteger::one()
                    .shift_left(exponent.unsigned_abs())
                    .expect("a double's denominator has at most 1075 bits"),
            }
        })
    }

    /// The nearest double, a tie to the one whose last bit is even;
    /// beyond the largest double an infinity, below half the smallest
    /// subnormal a zero, each with the value's sign. One rounding of the
    /// exact quotient, where dividing two converted parts would round
    /// three times and give a NaN once both pass the largest double.
    pub(crate) fn to_f64(&self) -> f64 {
        if self.numerator.is_zero() {
            return 0.0;
        }
        let magnitude = self.numerator.absolute();
        // The value lies in [2^exponent, 2^(exponent + 1)), and the bit
        // lengths put the exponent at their difference or one below it.
        let mut exponent = magnitude.bit_length() as i64 - self.denominator.bit_length() as i64;
        let rounded = if exponent > MAXIMUM_EXPONENT + 1 {
            f64::INFINITY
        } else if exponent < SMALLEST_EXPONENT - 2 {
            0.0
        } else {
            if compare_with_power(&magnitude, &self.denominator, exponent) == Ordering::Less {
                exponent -= 1;
            }
            round_to_double(&magnitude, &self.denominator, exponent)
        };
        if self.numerator.is_negative() {
            -rounded
        } else {
            rounded
        }
    }
}

/// The value `numerator / denominator` against `2^power`, in integers.
fn compare_with_power(
    numerator: &ArbitraryInteger,
    denominator: &ArbitraryInteger,
    power: i64,
) -> Ordering {
    let (left, right) = scaled(numerator, denominator, -power);
    left.cmp(&right)
}

/// `numerator · 2^shift` and `denominator` for a non-negative `shift`,
/// `numerator` and `denominator · 2^-shift` for a negative one: the pair
/// whose quotient is `numerator / denominator · 2^shift`.
fn scaled(
    numerator: &ArbitraryInteger,
    denominator: &ArbitraryInteger,
    shift: i64,
) -> (ArbitraryInteger, ArbitraryInteger) {
    let shifted = |integer: &ArbitraryInteger, count: u64| {
        integer
            .shift_left(count)
            .expect("a shift bounded by the double's exponent range")
    };
    if shift >= 0 {
        (shifted(numerator, shift as u64), denominator.clone())
    } else {
        (
            numerator.clone(),
            shifted(denominator, shift.unsigned_abs()),
        )
    }
}

/// The double nearest the positive `numerator / denominator`, which lies
/// in `[2^exponent, 2^(exponent + 1))`.
fn round_to_double(
    numerator: &ArbitraryInteger,
    denominator: &ArbitraryInteger,
    exponent: i64,
) -> f64 {
    if exponent > MAXIMUM_EXPONENT {
        return f64::INFINITY;
    }
    // The significant bits the result keeps: 53, fewer below the normal
    // range, where the last one is 2^-1074's.
    let precision = if exponent >= MINIMUM_NORMAL_EXPONENT {
        SIGNIFICANT_BITS
    } else {
        exponent - SMALLEST_EXPONENT + 1
    };
    if precision <= 0 {
        // Below 2^-1074: from [2^-1075, 2^-1074) it rounds to 2^-1074 when
        // above half of it, and a tie at exactly 2^-1075 to the even zero.
        let above_half = precision == 0
            && compare_with_power(numerator, denominator, SMALLEST_EXPONENT - 1)
                == Ordering::Greater;
        return if above_half { f64::from_bits(1) } else { 0.0 };
    }
    // The quotient has exactly `precision` bits; the remainder decides the
    // rounding.
    let (scaled_numerator, scaled_denominator) =
        scaled(numerator, denominator, precision - 1 - exponent);
    let (quotient, remainder) = scaled_numerator
        .truncated_divide(&scaled_denominator)
        .expect("a positive denominator");
    let mut significand = quotient.to_u64().expect("at most 53 bits");
    let twice_remainder = remainder.shift_left(1).expect("one bit more");
    match twice_remainder.cmp(&scaled_denominator) {
        Ordering::Greater => significand += 1,
        Ordering::Equal if significand & 1 == 1 => significand += 1,
        _ => {}
    }
    // `significand · 2^power` is a double (or overflows to the infinity a
    // round-up past the largest double is); `power` lies from -1074 to
    // 971, so one step through 2^-1000 keeps every product exact.
    let power = exponent + 1 - precision;
    let value = significand as f64;
    if power < -1000 {
        value * 2f64.powi(-1000) * 2f64.powi((power + 1000) as i32)
    } else {
        value * 2f64.powi(power as i32)
    }
}

impl Ord for Rational {
    /// By value: `a/b` against `c/d` as `a·d` against `c·b`, the
    /// denominators being positive.
    fn cmp(&self, other: &Rational) -> Ordering {
        self.numerator
            .multiply(&other.denominator)
            .cmp(&other.numerator.multiply(&self.denominator))
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Rational) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::Rational;
    use crate::ArbitraryInteger;

    fn rational(numerator: i128, denominator: i128) -> Rational {
        Rational::new(
            ArbitraryInteger::from_i128(numerator),
            ArbitraryInteger::from_i128(denominator),
        )
        .expect("a non-zero denominator")
    }

    #[test]
    fn new_reduces_and_puts_the_sign_on_the_numerator() {
        let half = rational(-3, -6);
        assert_eq!(half.numerator(), &ArbitraryInteger::from_i64(1));
        assert_eq!(half.denominator(), &ArbitraryInteger::from_i64(2));
        assert_eq!(rational(3, -6), rational(-1, 2));
        assert!(rational(6, 3).is_integer());
        assert!(Rational::new(ArbitraryInteger::one(), ArbitraryInteger::zero()).is_none());
        assert!(Rational::is_normal(
            &ArbitraryInteger::from_i64(-3),
            &ArbitraryInteger::from_i64(4)
        ));
        assert!(!Rational::is_normal(
            &ArbitraryInteger::from_i64(2),
            &ArbitraryInteger::from_i64(4)
        ));
        assert!(!Rational::is_normal(
            &ArbitraryInteger::from_i64(3),
            &ArbitraryInteger::from_i64(-4)
        ));
        assert!(!Rational::is_normal(
            &ArbitraryInteger::from_i64(3),
            &ArbitraryInteger::zero()
        ));
    }

    #[test]
    fn arithmetic_is_exact() {
        assert_eq!(rational(1, 2).add(&rational(1, 3)), rational(5, 6));
        assert_eq!(rational(1, 2).subtract(&rational(1, 2)), rational(0, 1));
        assert_eq!(rational(2, 3).multiply(&rational(3, 4)), rational(1, 2));
        assert_eq!(rational(1, 2).divide(&rational(1, 4)), Some(rational(2, 1)));
        assert_eq!(rational(1, 2).divide(&rational(0, 1)), None);
        assert_eq!(rational(-7, 2).floor(), ArbitraryInteger::from_i64(-4));
        assert_eq!(rational(-7, 2).truncated(), ArbitraryInteger::from_i64(-3));
        assert!(rational(1, 3) < rational(1, 2));
        assert!(rational(-1, 2) < rational(-1, 3));
    }

    #[test]
    fn a_double_is_an_exact_rational() {
        assert_eq!(Rational::from_f64(0.5), Some(rational(1, 2)));
        assert_eq!(Rational::from_f64(-3.0), Some(rational(-3, 1)));
        assert_eq!(Rational::from_f64(-0.0), Some(rational(0, 1)));
        assert_eq!(
            Rational::from_f64(0.1),
            Some(rational(3_602_879_701_896_397, 36_028_797_018_963_968))
        );
        assert!(Rational::from_f64(f64::NAN).is_none());
        assert!(Rational::from_f64(f64::INFINITY).is_none());
        let smallest = Rational::from_f64(f64::from_bits(1)).expect("finite");
        assert_eq!(smallest.denominator().bit_length(), 1075);
        assert_eq!(
            Rational::from_f64(f64::MAX)
                .expect("finite")
                .numerator()
                .bit_length(),
            1024
        );
    }

    #[test]
    fn to_f64_rounds_correctly_and_ties_to_even() {
        for value in [
            0.5,
            -0.1,
            1.0 / 3.0,
            f64::MAX,
            -f64::MAX,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::from_bits(0x000f_ffff_ffff_ffff),
            1e300,
            1e-300,
        ] {
            let exact = Rational::from_f64(value).expect("finite");
            assert_eq!(exact.to_f64().to_bits(), value.to_bits(), "{value}");
        }
        assert_eq!(rational(1, 3).to_f64(), 1.0 / 3.0);
        assert_eq!(rational(2, 3).to_f64(), 2.0 / 3.0);
        // 2^53 + 1 lies halfway between two doubles and rounds to the even
        // 2^53; 2^53 + 3 rounds up to 2^53 + 4.
        assert_eq!(rational((1 << 53) + 1, 1).to_f64(), 9_007_199_254_740_992.0);
        assert_eq!(rational((1 << 53) + 3, 1).to_f64(), 9_007_199_254_740_996.0);
        // Numerator and denominator both beyond the largest double.
        let huge = ArbitraryInteger::one().shift_left(2000).expect("small");
        let three_huge = huge.multiply(&ArbitraryInteger::from_i64(3));
        let third = Rational::new(huge.clone(), three_huge).expect("non-zero");
        assert_eq!(third.to_f64(), 1.0 / 3.0);
        let enormous = Rational::from_integer(huge.clone());
        assert_eq!(enormous.to_f64(), f64::INFINITY);
        assert_eq!(
            Rational::from_integer(huge.negated()).to_f64(),
            f64::NEG_INFINITY
        );
        let tiny = Rational::new(ArbitraryInteger::one(), huge).expect("non-zero");
        assert_eq!(tiny.to_f64(), 0.0);
        // Just above half the smallest subnormal rounds up to it; exactly
        // half is a tie and rounds to the even zero.
        let half_smallest = Rational::new(
            ArbitraryInteger::one(),
            ArbitraryInteger::one().shift_left(1075).expect("small"),
        )
        .expect("non-zero");
        assert_eq!(half_smallest.to_f64(), 0.0);
        let above_half = half_smallest.add(
            &Rational::new(
                ArbitraryInteger::one(),
                ArbitraryInteger::one().shift_left(1100).expect("small"),
            )
            .expect("non-zero"),
        );
        assert_eq!(above_half.to_f64(), f64::from_bits(1));
    }
}
