// Adapted from num-bigint (https://github.com/rust-num/num-bigint), commit
// 41f0b035d12971c5c7f7501917e31f0f6439c26c, src/biguint/division.rs.
// Copyright (c) 2014 The Rust Project Developers. MIT licence; see
// THIRD-PARTY-NOTICES.md at the repository root. Changed for ns-next: names
// written out, `BigUint` replaced by digit slices, no `unsafe`.

//! Division of magnitudes: by one digit, and otherwise by Knuth's algorithm
//! D. num-bigint's x86 `div` through `asm!` is not taken; the portable `u128`
//! division stands in for it.

use std::cmp::Ordering;

use super::addition::{add_in_place_with_carry, compare_magnitudes};
use super::{significant_length, trim_zero_top_digits};

/// Half a digit: a divisor at most this large goes through
/// `divide_half_digits`.
const HALF_DIGIT: u64 = u32::MAX as u64;

/// `[high, low] / divisor` as a quotient and a remainder, where `high <
/// divisor`, so both fit one digit.
#[inline]
fn divide_wide(high: u64, low: u64, divisor: u64) -> (u64, u64) {
    debug_assert!(high < divisor);

    let dividend = (u128::from(high) << u64::BITS) | u128::from(low);
    let divisor = u128::from(divisor);
    ((dividend / divisor) as u64, (dividend % divisor) as u64)
}

/// `[remainder, digit] / divisor` for a divisor of at most half a digit, in
/// two half-digit steps of plain `u64` division, like long division by hand.
/// Without the `div` instruction num-bigint reaches through `asm!`, a
/// `u128` division is a library call; this is the path num-bigint takes on
/// every target that lacks it.
#[inline]
fn divide_half_digits(remainder: u64, digit: u64, divisor: u64) -> (u64, u64) {
    const HALF_BITS: u32 = u64::BITS / 2;

    debug_assert!(remainder < divisor && divisor <= HALF_DIGIT);
    let high_dividend = (remainder << HALF_BITS) | (digit >> HALF_BITS);
    let (high, remainder) = (high_dividend / divisor, high_dividend % divisor);
    let low_dividend = (remainder << HALF_BITS) | (digit & HALF_DIGIT);
    let (low, remainder) = (low_dividend / divisor, low_dividend % divisor);
    ((high << HALF_BITS) | low, remainder)
}

/// `dividend / divisor` for a one-digit divisor, as a trimmed quotient and a
/// remainder.
///
/// # Panics
///
/// For a zero divisor, which every caller has ruled out.
pub(super) fn divide_by_digit(dividend: &[u64], divisor: u64) -> (Vec<u64>, u64) {
    assert!(divisor != 0, "a magnitude was divided by zero");

    let mut quotient = dividend.to_vec();
    let mut remainder = 0;

    if divisor <= HALF_DIGIT {
        for digit in quotient.iter_mut().rev() {
            (*digit, remainder) = divide_half_digits(remainder, *digit, divisor);
        }
    } else {
        for digit in quotient.iter_mut().rev() {
            (*digit, remainder) = divide_wide(remainder, *digit, divisor);
        }
    }

    trim_zero_top_digits(&mut quotient);
    (quotient, remainder)
}

/// `minuend -= subtrahend * multiplier` over two slices of one length,
/// answering the borrow out of the top digit.
fn subtract_multiple_same_length(minuend: &mut [u64], subtrahend: &[u64], multiplier: u64) -> u64 {
    debug_assert!(minuend.len() == subtrahend.len());

    // The carry lies between `-u64::MAX` and 0; it is kept offset by
    // `u64::MAX` so that it is never negative.
    let mut offset_carry = u64::MAX;

    for (digit, &subtrahend_digit) in minuend.iter_mut().zip(subtrahend) {
        // `digit - subtrahend_digit * multiplier + carry`, offset by
        // `u64::MAX << 64` into `u128` range.
        let offset_sum = ((u128::from(u64::MAX) << u64::BITS) | u128::from(*digit))
            - u128::from(u64::MAX)
            + u128::from(offset_carry)
            - u128::from(subtrahend_digit) * u128::from(multiplier);

        offset_carry = (offset_sum >> u64::BITS) as u64;
        *digit = offset_sum as u64;
    }

    u64::MAX - offset_carry
}

/// `dividend / divisor` as a trimmed quotient and a trimmed remainder. The
/// divisor must be non-empty and trimmed; the dividend need not be trimmed.
///
/// # Panics
///
/// For an empty (zero) divisor, which every caller has ruled out.
pub(super) fn divide_remainder(dividend: &[u64], divisor: &[u64]) -> (Vec<u64>, Vec<u64>) {
    assert!(!divisor.is_empty(), "a magnitude was divided by zero");
    debug_assert!(divisor.last() != Some(&0), "the divisor is not trimmed");
    let dividend = &dividend[..significant_length(dividend)];

    if dividend.is_empty() {
        return (Vec::new(), Vec::new());
    }

    if let [digit] = *divisor {
        if digit == 1 {
            return (dividend.to_vec(), Vec::new());
        }
        let (quotient, remainder) = divide_by_digit(dividend, digit);
        let remainder = if remainder == 0 {
            Vec::new()
        } else {
            vec![remainder]
        };
        return (quotient, remainder);
    }

    // Algorithm D needs a dividend at least as long as the divisor.
    match compare_magnitudes(dividend, divisor) {
        Ordering::Less => return (Vec::new(), dividend.to_vec()),
        Ordering::Equal => return (vec![1], Vec::new()),
        Ordering::Greater => {}
    }

    // num-bigint sends a dividend of more than 128 digits by a divisor of
    // more than 64 to Burnikel and Ziegler's recursive division; until that
    // exists here, algorithm D carries every size, which is correct and only
    // slower.

    // Algorithm D guesses each quotient digit from the divisor's top digit,
    // which is best when that digit's high bit is set: shift both operands
    // until it is, and the remainder back afterwards.
    let shift = divisor[divisor.len() - 1].leading_zeros();
    let (quotient, mut remainder) =
        divide_normalized(shift_left(dividend, shift), &shift_left(divisor, shift));
    shift_right_in_place(&mut remainder, shift);
    (quotient, remainder)
}

/// Knuth, TAOCP volume 2, section 4.3.1, algorithm D, with the improvement
/// of exercises 19 to 21: the divisor has at least two digits and its top
/// digit's high bit set, and the dividend is at least as long.
///
/// Each step guesses the next quotient digit from the dividend's top three
/// digits and the divisor's top two, subtracts that multiple of the divisor
/// at the digit's position, and adds one divisor back in the rare case
/// (probability at most `2^-63`, exercise 21) that the guess was one too
/// large.
fn divide_normalized(mut dividend: Vec<u64>, divisor: &[u64]) -> (Vec<u64>, Vec<u64>) {
    debug_assert!(dividend.len() >= divisor.len() && divisor.len() > 1);
    debug_assert!(divisor[divisor.len() - 1].leading_zeros() == 0);

    // One more significant digit of the dividend, above what `dividend`
    // still holds.
    let mut extra_top = 0;

    // The divisor's two most significant digits, which every guess uses.
    let divisor_top = divisor[divisor.len() - 1];
    let divisor_second = divisor[divisor.len() - 2];

    let quotient_length = dividend.len() - divisor.len() + 1;
    let mut quotient = vec![0; quotient_length];

    for position in (0..quotient_length).rev() {
        debug_assert!(dividend.len() == divisor.len() + position);

        let dividend_top = dividend[dividend.len() - 1];
        let dividend_second = dividend[dividend.len() - 2];

        // The first guess, `[extra_top, dividend_top] / divisor_top`, is
        // never too small and at most two too large.
        let (mut guess, mut remainder) = if extra_top < divisor_top {
            let (guess, remainder) = divide_wide(extra_top, dividend_top, divisor_top);
            (guess, u128::from(remainder))
        } else {
            debug_assert!(extra_top == divisor_top);
            // The quotient digit fits one digit, so the guess saturates:
            // `[extra_top, dividend_top] = divisor_top * u64::MAX +
            // (extra_top + dividend_top)`.
            (u64::MAX, u128::from(extra_top) + u128::from(dividend_top))
        };

        // Refine the guess against the top three dividend digits and the
        // top two divisor digits: it is too large while
        // `(remainder << 64) + dividend_second < guess * divisor_second`.
        while remainder <= u128::from(u64::MAX)
            && ((remainder << u64::BITS) | u128::from(dividend_second))
                < u128::from(guess) * u128::from(divisor_second)
        {
            guess -= 1;
            remainder += u128::from(divisor_top);
        }

        // The guess is now right or, rarely, one too large; subtracting
        // shows which.
        let mut borrow = subtract_multiple_same_length(&mut dividend[position..], divisor, guess);
        if borrow > extra_top {
            guess -= 1;
            borrow -= add_in_place_with_carry(&mut dividend[position..], divisor);
        }
        // The extra top digit is now cleared.
        debug_assert!(borrow == extra_top);

        quotient[position] = guess;

        extra_top = dividend
            .pop()
            .expect("the dividend is longer than the divisor at every step");
    }

    if extra_top != 0 {
        dividend.push(extra_top);
    }
    trim_zero_top_digits(&mut dividend);

    debug_assert_eq!(compare_magnitudes(&dividend, divisor), Ordering::Less);

    trim_zero_top_digits(&mut quotient);
    (quotient, dividend)
}

/// `digits << shift` for a shift below one digit, with a top digit added only
/// when bits are carried into it.
pub(super) fn shift_left(digits: &[u64], shift: u32) -> Vec<u64> {
    debug_assert!(shift < u64::BITS);
    if shift == 0 {
        return digits.to_vec();
    }
    let mut shifted = Vec::with_capacity(digits.len() + 1);
    let mut carry = 0;
    for &digit in digits {
        shifted.push((digit << shift) | carry);
        carry = digit >> (u64::BITS - shift);
    }
    if carry != 0 {
        shifted.push(carry);
    }
    shifted
}

/// `digits >>= shift` for a shift below one digit, trimmed; the bits shifted
/// out are dropped.
pub(super) fn shift_right_in_place(digits: &mut Vec<u64>, shift: u32) {
    debug_assert!(shift < u64::BITS);
    if shift == 0 {
        return;
    }
    for index in 0..digits.len() {
        let carried_in = digits
            .get(index + 1)
            .map_or(0, |&next| next << (u64::BITS - shift));
        digits[index] = (digits[index] >> shift) | carried_in;
    }
    trim_zero_top_digits(digits);
}

#[cfg(test)]
mod tests {
    use super::divide_remainder;
    use crate::arbitrary_integer::addition::{add_magnitudes, compare_magnitudes};
    use crate::arbitrary_integer::multiplication::multiply_schoolbook;
    use std::cmp::Ordering;

    fn trimmed(mut digits: Vec<u64>) -> Vec<u64> {
        while digits.last() == Some(&0) {
            digits.pop();
        }
        digits
    }

    fn check(dividend: &[u64], divisor: &[u64]) {
        let (quotient, remainder) = divide_remainder(dividend, divisor);
        let product = multiply_schoolbook(&quotient, divisor);
        assert_eq!(
            trimmed(add_magnitudes(&product, &remainder)),
            trimmed(dividend.to_vec())
        );
        assert_eq!(compare_magnitudes(&remainder, divisor), Ordering::Less);
    }

    #[test]
    fn quotient_times_divisor_plus_remainder_is_the_dividend() {
        let shapes: [(&[u64], &[u64]); 7] = [
            (&[10, 20, 30], &[7]),
            (&[u64::MAX, u64::MAX, u64::MAX, 5], &[3, 1]),
            (&[1, 2, 3, 4, 5, 6], &[0, 0, 1 << 63]),
            (&[u64::MAX; 9], &[u64::MAX; 4]),
            (&[5], &[7, 1]),
            (&[4, 9, 1], &[4, 9, 1]),
            (&[0, 0, 0, 0, 1], &[1, 0, 1]),
        ];
        for (dividend, divisor) in shapes {
            check(dividend, divisor);
        }
    }

    /// The step that adds one divisor back after a guess one too large runs
    /// for this shape (found by instrumenting that branch); random operands
    /// reach it with probability about `2^-63` per quotient digit.
    #[test]
    fn a_guess_one_too_large_is_corrected() {
        check(&[0, 0, 2], &[1, 0, 1]);
    }

    /// Every dividend of four and divisor of three digits drawn from the
    /// digits at the carry, borrow and normalization edges.
    #[test]
    fn edge_digits_hold_the_identity() {
        let edges = [0, 1, u64::MAX, 1 << 63, (1 << 63) - 1];
        let pick = |mut index: usize, count: usize| {
            (0..count)
                .map(|_| {
                    let digit = edges[index % edges.len()];
                    index /= edges.len();
                    digit
                })
                .collect::<Vec<u64>>()
        };
        for dividend_index in 0..edges.len().pow(4) {
            for divisor_index in 0..edges.len().pow(3) {
                let divisor = trimmed(pick(divisor_index, 3));
                if !divisor.is_empty() {
                    check(&pick(dividend_index, 4), &divisor);
                }
            }
        }
    }

    /// Operands of many lengths from a linear congruential sequence, so that
    /// long quotients exercise every step of the loop.
    #[test]
    fn long_operands_hold_the_identity() {
        let mut state: u64 = 7;
        let mut digits = |count: usize| {
            (0..count)
                .map(|_| {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    state >> (state % 64)
                })
                .collect::<Vec<u64>>()
        };
        for (dividend_length, divisor_length) in [(2, 2), (5, 2), (20, 3), (40, 17), (150, 70)] {
            let dividend = digits(dividend_length);
            let divisor = trimmed(digits(divisor_length));
            if !divisor.is_empty() {
                check(&dividend, &divisor);
            }
        }
    }
}
