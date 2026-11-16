//! The hash every number answers: its exact value `n/d` modulo the
//! Mersenne prime 2^61 - 1, as `n · d⁻¹`, signed as `n` is, folded into
//! the `SmallInteger` range. Reduction modulo a prime respects sums,
//! products and cancellation, so equal numbers hash alike whatever their
//! representation -- `3`, `3.0` and `6/2`; `0.5` and `1/2` -- and a
//! `SmallInteger`, which lies within the modulus, hashes as itself
//! (`docs/design/core-library.md`, "The numeric tower").

use crate::ArbitraryInteger;
use crate::numeric::Rational;

const MODULUS: u64 = (1 << 61) - 1;

/// The hash of an infinity, and of a fraction whose denominator the
/// modulus divides, which has no inverse: negated for a negative one.
const INFINITY_HASH: i64 = 314_159;

/// Every NaN's hash: they are all equal (`docs/design/core-library.md`,
/// "A NaN equals a NaN").
const NOT_A_NUMBER_HASH: i64 = 271_828;

/// Half the `SmallInteger` range: hashes lie from `-HALF` to `HALF - 1`.
const HALF: i64 = 1 << 60;

/// A magnitude modulo the modulus, from its most significant digit down.
fn residue(magnitude: &[u64]) -> u64 {
    magnitude.iter().rev().fold(0, |accumulator, digit| {
        reduce((u128::from(accumulator) << 64) | u128::from(*digit))
    })
}

fn multiply(left: u64, right: u64) -> u64 {
    reduce(u128::from(left) * u128::from(right))
}

/// `value` modulo the modulus.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a remainder modulo MODULUS lies below 2^61"
)]
fn reduce(value: u128) -> u64 {
    (value % u128::from(MODULUS)) as u64
}

/// `value⁻¹` modulo the prime modulus, by Fermat: `value^(P - 2)`.
/// `value` is not zero.
fn inverse(value: u64) -> u64 {
    let mut result = 1;
    let mut base = value;
    let mut exponent = MODULUS - 2;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = multiply(result, base);
        }
        base = multiply(base, base);
        exponent >>= 1;
    }
    result
}

/// A residue with a sign, folded into the `SmallInteger` range: left alone
/// inside it, moved by `HALF` toward zero outside it.
fn folded(residue: u64, negative: bool) -> i64 {
    let value = residue as i64;
    let signed = if negative { -value } else { value };
    if (-HALF..HALF).contains(&signed) {
        signed
    } else if signed > 0 {
        signed - HALF
    } else {
        signed + HALF
    }
}

pub(crate) fn integer_hash(integer: &ArbitraryInteger) -> i64 {
    folded(residue(integer.magnitude()), integer.is_negative())
}

/// An `i64`'s hash, which is the value itself within the `SmallInteger`
/// range.
pub(crate) fn small_integer_hash(value: i64) -> i64 {
    folded(value.unsigned_abs() % MODULUS, value < 0)
}

pub(crate) fn rational_hash(rational: &Rational) -> i64 {
    let negative = rational.numerator().is_negative();
    let denominator = residue(rational.denominator().magnitude());
    if denominator == 0 {
        return if negative {
            -INFINITY_HASH
        } else {
            INFINITY_HASH
        };
    }
    let numerator = residue(rational.numerator().magnitude());
    // An integer (an integral double included) has the residue 1 as its
    // denominator: skipping the inverse spares its ~61 modular squarings.
    if denominator == 1 {
        return folded(numerator, negative);
    }
    folded(multiply(numerator, inverse(denominator)), negative)
}

/// A double's hash: its exact value's, the two infinities' and NaN's
/// constants apart.
pub(crate) fn float_hash(value: f64) -> i64 {
    if value.is_nan() {
        NOT_A_NUMBER_HASH
    } else if value.is_infinite() {
        if value > 0.0 {
            INFINITY_HASH
        } else {
            -INFINITY_HASH
        }
    } else {
        rational_hash(&Rational::from_f64(value).expect("a finite double"))
    }
}

#[cfg(test)]
mod tests {
    use super::{float_hash, integer_hash, rational_hash, small_integer_hash};
    use crate::numeric::Rational;
    use crate::{ArbitraryInteger, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};

    #[test]
    fn a_small_integer_hashes_as_itself() {
        for value in [0, 1, -1, 42, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN] {
            assert_eq!(small_integer_hash(value), value);
            assert_eq!(integer_hash(&ArbitraryInteger::from_i64(value)), value);
        }
    }

    #[test]
    fn equal_numbers_hash_alike_across_representations() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        let rational = |numerator: i128, denominator: i128| {
            Rational::new(integer(numerator), integer(denominator)).expect("non-zero")
        };
        assert_eq!(float_hash(3.0), small_integer_hash(3));
        assert_eq!(float_hash(-3.0), small_integer_hash(-3));
        assert_eq!(float_hash(0.0), 0);
        assert_eq!(float_hash(-0.0), 0);
        assert_eq!(float_hash(0.5), rational_hash(&rational(1, 2)));
        assert_eq!(float_hash(-0.375), rational_hash(&rational(-3, 8)));
        let power = integer(1 << 100);
        assert_eq!(float_hash(2f64.powi(100)), integer_hash(&power));
        assert_eq!(rational_hash(&rational(6, 3)), small_integer_hash(2));
        assert_eq!(
            float_hash(f64::MAX),
            rational_hash(&Rational::from_f64(f64::MAX).expect("finite"))
        );
        assert_eq!(float_hash(f64::NAN), float_hash(-f64::NAN));
        assert_ne!(float_hash(f64::INFINITY), float_hash(f64::NEG_INFINITY));
    }

    #[test]
    fn every_hash_is_a_small_integer() {
        let modulus = (1i128 << 61) - 1;
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        for value in [
            modulus - 1,
            modulus,
            modulus + 1,
            -(modulus - 1),
            1 << 120,
            -(1 << 120),
        ] {
            let hash = integer_hash(&integer(value));
            assert!(
                (SMALL_INTEGER_MIN..=SMALL_INTEGER_MAX).contains(&hash),
                "{value}"
            );
        }
        let multiple_of_modulus = Rational::new(integer(1), integer(modulus)).expect("non-zero");
        let hash = rational_hash(&multiple_of_modulus);
        assert!((SMALL_INTEGER_MIN..=SMALL_INTEGER_MAX).contains(&hash));
    }
}
