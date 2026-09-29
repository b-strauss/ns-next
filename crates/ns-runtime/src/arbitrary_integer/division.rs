// Adapted from num-bigint (https://github.com/rust-num/num-bigint), commit
// 41f0b035d12971c5c7f7501917e31f0f6439c26c, src/biguint/division.rs.
// Copyright (c) 2014 The Rust Project Developers. MIT licence; see
// THIRD-PARTY-NOTICES.md at the repository root. Changed for ns-next: names
// written out, `BigUint` replaced by digit slices, no `unsafe`.

//! Division of magnitudes: by one digit, by Knuth's algorithm D, and for
//! the largest operands by Burnikel and Ziegler's recursive division.
//! num-bigint's x86 `div` through `asm!` is not taken; the portable `u128`
//! division stands in for it.

use std::cmp::Ordering;

use super::addition::{
    add_in_place_with_carry, add_magnitudes, compare_magnitudes, subtract_in_place,
    subtract_magnitudes,
};
use super::multiplication::multiply_magnitudes;
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

    if dividend.len() > 2 * BURNIKEL_ZIEGLER_THRESHOLD && divisor.len() > BURNIKEL_ZIEGLER_THRESHOLD
    {
        return divide_remainder_recursively(dividend, divisor);
    }

    divide_remainder_knuth(dividend, divisor)
}

/// `dividend / divisor` by algorithm D, for a divisor of at least two
/// digits and a larger dividend, both trimmed.
fn divide_remainder_knuth(dividend: &[u64], divisor: &[u64]) -> (Vec<u64>, Vec<u64>) {
    // Algorithm D guesses each quotient digit from the divisor's top digit,
    // which is best when that digit's high bit is set: shift both operands
    // until it is, and the remainder back afterwards.
    let shift = divisor[divisor.len() - 1].leading_zeros();
    let (quotient, mut remainder) =
        divide_normalized(shift_left(dividend, shift), &shift_left(divisor, shift));
    shift_right_in_place(&mut remainder, shift);
    (quotient, remainder)
}

/// Above this many digits in the divisor, and twice as many in the
/// dividend, Burnikel and Ziegler's division wins; it also ends its
/// recursion, whose blocks of at most this many digits go to algorithm D.
const BURNIKEL_ZIEGLER_THRESHOLD: usize = 64;

/// `dividend / divisor` by Burnikel and Ziegler's recursive division
/// ("Fast Recursive Division", 1998), for a divisor of more than
/// `BURNIKEL_ZIEGLER_THRESHOLD` digits and a larger dividend, both trimmed.
///
/// The dividend is cut into blocks of `level` digits, `level` the least
/// power of two the divisor fits, and divided block by block like long
/// division, each step a division of two blocks by one. That division splits
/// every operand in halves and recurses, so it costs a few multiplications
/// of its size and inherits their speed.
fn divide_remainder_recursively(dividend: &[u64], divisor: &[u64]) -> (Vec<u64>, Vec<u64>) {
    debug_assert!(divisor.last() != Some(&0), "the divisor is not trimmed");
    let level = divisor.len().next_power_of_two();
    // Like algorithm D, each step needs the divisor's top bit set, here the
    // top bit of a whole block: shift both operands until it is, and the
    // remainder back afterwards.
    let shift = normalizing_shift(divisor, level);
    let (quotient, mut remainder) = divide_blocks(
        &shift_left_by_bits(dividend, shift),
        &shift_left_by_bits(divisor, shift),
        level,
    );
    shift_right_by_bits_in_place(&mut remainder, shift);
    (quotient, remainder)
}

/// How far the trimmed `digits` must shift left for its top bit to be the
/// top bit of a block of `level` digits.
fn normalizing_shift(digits: &[u64], level: usize) -> u64 {
    let bit_length = match digits.last() {
        Some(top) => digits.len() as u64 * 64 - u64::from(top.leading_zeros()),
        None => 0,
    };
    level as u64 * 64 - bit_length
}

/// `digits` split at digit `level`, as a trimmed high part and a trimmed low
/// part.
fn split_at_level(mut digits: Vec<u64>, level: usize) -> (Vec<u64>, Vec<u64>) {
    if digits.len() <= level {
        return (Vec::new(), digits);
    }
    let mut high = digits.split_off(level);
    trim_zero_top_digits(&mut high);
    trim_zero_top_digits(&mut digits);
    (high, digits)
}

/// `high * base^level + low`, trimmed, for a `low` below `base^level`.
fn concatenate(high: &[u64], mut low: Vec<u64>, level: usize) -> Vec<u64> {
    debug_assert!(significant_length(&low) <= level);
    low.resize(level, 0);
    low.extend_from_slice(high);
    trim_zero_top_digits(&mut low);
    low
}

/// Long division by blocks of `level` digits: each block of the dividend,
/// from the top, is divided with the running remainder above it.
fn divide_blocks(dividend: &[u64], divisor: &[u64], level: usize) -> (Vec<u64>, Vec<u64>) {
    let mut dividend_blocks = dividend.chunks(level);

    // A top block already below the divisor is the first remainder.
    let mut remainder = Vec::new();
    if let Some(top_block) = dividend_blocks.clone().next_back() {
        if compare_magnitudes(top_block, divisor) == Ordering::Less {
            dividend_blocks.next_back();
            remainder = top_block.to_vec();
            trim_zero_top_digits(&mut remainder);
        }
    }

    let mut quotient = vec![0; dividend_blocks.len() * level];
    let quotient_blocks = quotient.chunks_mut(level);

    for (quotient_block, dividend_block) in quotient_blocks.zip(dividend_blocks).rev() {
        let mut dividend_block = dividend_block.to_vec();
        trim_zero_top_digits(&mut dividend_block);
        let (block_quotient, block_remainder) =
            divide_two_blocks_by_one(remainder, dividend_block, divisor.to_vec(), level);
        quotient_block[..block_quotient.len()].copy_from_slice(&block_quotient);
        remainder = block_remainder;
    }

    trim_zero_top_digits(&mut quotient);
    (quotient, remainder)
}

/// `[high_block, low_block] / divisor`, blocks of `level` digits, for a
/// divisor of `level` digits with its top bit set and a `high_block` below
/// it, so that the quotient fits one block: Burnikel and Ziegler's
/// algorithm 1.
fn divide_two_blocks_by_one(
    high_block: Vec<u64>,
    low_block: Vec<u64>,
    divisor: Vec<u64>,
    level: usize,
) -> (Vec<u64>, Vec<u64>) {
    debug_assert_eq!(
        compare_magnitudes(&high_block, &divisor),
        Ordering::Less,
        "the quotient does not fit one block"
    );
    debug_assert_eq!(
        normalizing_shift(&divisor, level),
        0,
        "the divisor's top bit is not set"
    );
    if level <= BURNIKEL_ZIEGLER_THRESHOLD {
        return divide_remainder(&concatenate(&high_block, low_block, level), &divisor);
    }
    let half = level / 2;
    let (first_block, second_block) = split_at_level(high_block, half);
    let (third_block, fourth_block) = split_at_level(low_block, half);
    let (divisor_high, divisor_low) = split_at_level(divisor, half);
    let (high_quotient, remainder) = divide_three_halves_by_two(
        first_block,
        second_block,
        third_block,
        divisor_high.clone(),
        divisor_low.clone(),
        half,
    );
    let (remainder_high, remainder_low) = split_at_level(remainder, half);
    let (low_quotient, final_remainder) = divide_three_halves_by_two(
        remainder_high,
        remainder_low,
        fourth_block,
        divisor_high,
        divisor_low,
        half,
    );
    (
        concatenate(&high_quotient, low_quotient, half),
        final_remainder,
    )
}

/// `[first_block, second_block, third_block] / [divisor_high, divisor_low]`,
/// blocks of `level` digits, for a dividend below `divisor * base^level`, so
/// that the quotient fits one block: Burnikel and Ziegler's algorithm 2.
fn divide_three_halves_by_two(
    first_block: Vec<u64>,
    second_block: Vec<u64>,
    third_block: Vec<u64>,
    divisor_high: Vec<u64>,
    divisor_low: Vec<u64>,
    level: usize,
) -> (Vec<u64>, Vec<u64>) {
    // The quotient is first estimated from the top blocks alone. Step 3 of
    // algorithm 2 tells `first_block < divisor_high` from the rest, which
    // keeps `divide_two_blocks_by_one`'s quotient in one block.
    let (mut quotient, partial_remainder, subtrahend);
    if compare_magnitudes(&first_block, &divisor_high) == Ordering::Less {
        (quotient, partial_remainder) =
            divide_two_blocks_by_one(first_block, second_block, divisor_high.clone(), level);
        subtrahend = multiply_magnitudes(&quotient, &divisor_low);
    } else {
        // The estimate is `base^level - 1`, and the partial remainder
        // `[first_block, second_block] - estimate * divisor_high`, which is
        // `[first_block - divisor_high, second_block] + divisor_high`.
        quotient = vec![u64::MAX; level];
        partial_remainder = add_magnitudes(
            &concatenate(
                &subtract_magnitudes(&first_block, &divisor_high),
                second_block,
                level,
            ),
            &divisor_high,
        );
        // `estimate * divisor_low`, likewise, is `divisor_low * base^level -
        // divisor_low`.
        subtrahend =
            subtract_magnitudes(&concatenate(&divisor_low, Vec::new(), level), &divisor_low);
    }

    // The estimate is at most two too large, so at most two corrections
    // lift the remainder to the subtrahend, and the final subtraction
    // cannot go below zero.
    let mut remainder = concatenate(&partial_remainder, third_block, level);
    if compare_magnitudes(&remainder, &subtrahend) == Ordering::Less {
        let divisor = concatenate(&divisor_high, divisor_low, level);
        decrement(&mut quotient);
        remainder = add_magnitudes(&remainder, &divisor);
        if compare_magnitudes(&remainder, &subtrahend) == Ordering::Less {
            decrement(&mut quotient);
            remainder = add_magnitudes(&remainder, &divisor);
        }
    }
    (quotient, subtract_magnitudes(&remainder, &subtrahend))
}

/// `digits -= 1`, trimmed; `digits` is not zero.
fn decrement(digits: &mut Vec<u64>) {
    subtract_in_place(digits, &[1]);
    trim_zero_top_digits(digits);
}

/// `digits << bits` for a shift of any size.
fn shift_left_by_bits(digits: &[u64], bits: u64) -> Vec<u64> {
    let whole_digits = (bits / 64) as usize;
    let mut shifted = vec![0; whole_digits];
    shifted.extend(shift_left(digits, (bits % 64) as u32));
    shifted
}

/// `digits >>= bits` for a shift of any size, trimmed; the bits shifted out
/// are dropped.
fn shift_right_by_bits_in_place(digits: &mut Vec<u64>, bits: u64) {
    let whole_digits = (bits / 64) as usize;
    digits.drain(..whole_digits.min(digits.len()));
    shift_right_in_place(digits, (bits % 64) as u32);
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

    fn digits(count: usize, seed: u64) -> Vec<u64> {
        let mut state = seed;
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                state
            })
            .collect()
    }

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

    /// Shapes past both of Burnikel and Ziegler's thresholds, a dividend
    /// above 128 digits and a divisor above 64: ordinary ones, a divisor just
    /// past the threshold, the smallest dividend that takes the recursion, a
    /// divisor of all ones, which normalization shifts by whole digits
    /// alone, and an exact multiple, whose remainder is empty.
    #[test]
    fn burnikel_ziegler_shapes_hold_the_identity() {
        for (dividend_count, divisor_count) in [(200, 100), (1000, 300), (3000, 65), (129, 65)] {
            check(&digits(dividend_count, 14), &digits(divisor_count, 15));
        }

        check(&digits(200, 16), &[u64::MAX; 90]);

        let divisor = digits(100, 17);
        let quotient = digits(150, 18);
        let dividend = multiply_schoolbook(&divisor, &quotient);
        let (found_quotient, remainder) = divide_remainder(&dividend, &divisor);
        assert_eq!(found_quotient, quotient);
        assert!(remainder.is_empty());
    }

    /// A block of 64 digits: `lowest`, then `middle` up to the `top` digit.
    fn block(lowest: u64, middle: u64, top: u64) -> Vec<u64> {
        let mut digits = vec![middle; 64];
        digits[0] = lowest;
        digits[63] = top;
        digits
    }

    /// Algorithm 2's rare steps, at the top level of a 128-digit divisor
    /// `[divisor_high, divisor_low]` whose top bit is set, so that nothing
    /// is shifted: a dividend `[first, second, third, 0]` whose top two
    /// blocks are the first remainder. Found by modelling algorithm 2 on
    /// blocks of 8, 16 and 64 bits, where the same shapes, in units of half
    /// the block, reach the same steps: the estimate from `first ==
    /// divisor_high` of all ones, right and one too large, and an estimate
    /// divided from the top blocks two too large.
    #[test]
    fn burnikel_ziegler_rare_steps_hold_the_identity() {
        let zero = block(0, 0, 0);
        let one = block(1, 0, 0);
        let half = block(0, 0, 1 << 63);
        let half_less_one = block(u64::MAX, u64::MAX, (1 << 63) - 1);
        let half_plus_one = block(1, 0, 1 << 63);
        let all_ones_less_one = block(u64::MAX - 1, u64::MAX, u64::MAX);

        // (first, second, third, divisor_high, divisor_low)
        let shapes = [
            (&half, &zero, &zero, &half, &one),
            (&half, &zero, &zero, &half, &half_plus_one),
            (&half_less_one, &zero, &zero, &half, &all_ones_less_one),
        ];
        for (first, second, third, divisor_high, divisor_low) in shapes {
            let dividend = [&zero[..], third, second, first].concat();
            let divisor = [&divisor_low[..], divisor_high].concat();
            check(&dividend, &divisor);
            assert_eq!(
                super::divide_remainder_recursively(&dividend, &divisor),
                super::divide_remainder_knuth(&dividend, &divisor),
            );
        }
    }

    #[test]
    fn burnikel_ziegler_agrees_with_algorithm_d() {
        for (dividend_count, divisor_count) in
            [(200, 100), (1000, 300), (3000, 65), (129, 65), (700, 699)]
        {
            let dividend = digits(dividend_count, 10);
            let divisor = digits(divisor_count, 11);
            assert_eq!(
                super::divide_remainder_recursively(&dividend, &divisor),
                super::divide_remainder_knuth(&dividend, &divisor),
                "{dividend_count} / {divisor_count} digits",
            );
        }
    }
}
