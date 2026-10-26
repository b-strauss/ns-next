// Adapted from num-bigint (https://github.com/rust-num/num-bigint), commit
// 41f0b035d12971c5c7f7501917e31f0f6439c26c, src/biguint/multiplication.rs.
// Copyright (c) 2014 The Rust Project Developers. MIT licence; see
// THIRD-PARTY-NOTICES.md at the repository root. Changed for ns-next: names
// written out, `BigUint` replaced by digit slices, no `unsafe`.

//! Multiplication of magnitudes: long multiplication, half-Karatsuba,
//! Karatsuba, Toom-3 and the number-theoretic transform
//! (`number_theoretic_transform`), chosen by operand size as num-bigint
//! chooses.

use std::cmp::Ordering;

use super::addition::{
    add_in_place, add_in_place_with_carry, compare_magnitudes, subtract_in_place,
    subtract_magnitudes,
};
use super::number_theoretic_transform::multiply_accumulate_by_transform;
use super::{low_digit, trim_zero_top_digits, ArbitraryInteger};

/// Below this many digits in the shorter operand, long multiplication wins.
const LONG_MULTIPLICATION_LIMIT: usize = 32;
/// Up to this many digits in the shorter operand, Karatsuba; above, Toom-3.
const KARATSUBA_LIMIT: usize = 256;
/// Up to this many digits in the shorter operand, Toom-3; above, the
/// number-theoretic transform.
const TOOM_3_LIMIT: usize = 512;

/// `addend + left * right + carry`, answering the low digit and leaving the
/// high one in `carry`.
#[inline]
pub(super) fn multiply_accumulate_with_carry(
    addend: u64,
    left: u64,
    right: u64,
    carry: &mut u128,
) -> u64 {
    *carry += u128::from(addend);
    *carry += u128::from(left) * u128::from(right);
    let low = low_digit(*carry);
    *carry >>= u64::BITS;
    low
}

/// `left * right + carry`, answering the low digit and leaving the high one
/// in `carry`.
#[inline]
pub(super) fn multiply_with_carry(left: u64, right: u64, carry: &mut u128) -> u64 {
    *carry += u128::from(left) * u128::from(right);
    let low = low_digit(*carry);
    *carry >>= u64::BITS;
    low
}

/// `accumulator += multiplicand * multiplier`, where the accumulator is
/// longer than the multiplicand.
fn multiply_accumulate_digit(accumulator: &mut [u64], multiplicand: &[u64], multiplier: u64) {
    if multiplier == 0 {
        return;
    }

    let mut carry = 0;
    let (low, high) = accumulator.split_at_mut(multiplicand.len());

    for (digit, &multiplicand_digit) in low.iter_mut().zip(multiplicand) {
        *digit = multiply_accumulate_with_carry(*digit, multiplicand_digit, multiplier, &mut carry);
    }

    // The carry out of a digit's multiply-accumulate fits one digit.
    debug_assert!(carry >> u64::BITS == 0);
    let final_carry = add_in_place_with_carry(high, &[low_digit(carry)]);
    assert_eq!(final_carry, 0, "carry overflow during multiplication");
}

/// `accumulator += left * right` by long multiplication, one pass per digit
/// of `left`, so `left` should be the shorter.
fn multiply_long(accumulator: &mut [u64], left: &[u64], right: &[u64]) {
    for (index, &digit) in left.iter().enumerate() {
        multiply_accumulate_digit(&mut accumulator[index..], right, digit);
    }
}

/// `accumulator += left * right`. The accumulator must be long enough to hold
/// the sum without a carry out, which `left.len() + right.len() + 1` digits
/// always are for an accumulator that starts at zero.
fn multiply_accumulate(mut accumulator: &mut [u64], mut left: &[u64], mut right: &[u64]) {
    // Least significant zero digits contribute nothing but a shift.
    if let Some(&0) = left.first() {
        let Some(first_non_zero) = left.iter().position(|&digit| digit != 0) else {
            return;
        };
        left = &left[first_non_zero..];
        accumulator = &mut accumulator[first_non_zero..];
    }
    if let Some(&0) = right.first() {
        let Some(first_non_zero) = right.iter().position(|&digit| digit != 0) else {
            return;
        };
        right = &right[first_non_zero..];
        accumulator = &mut accumulator[first_non_zero..];
    }

    let (shorter, longer) = if left.len() < right.len() {
        (left, right)
    } else {
        (right, left)
    };

    // num-bigint's thresholds, chosen there by benchmark.
    if shorter.len() <= LONG_MULTIPLICATION_LIMIT {
        multiply_long(accumulator, shorter, longer);
    } else if shorter.len() * 2 <= longer.len() {
        multiply_half_karatsuba(accumulator, shorter, longer);
    } else if shorter.len() <= KARATSUBA_LIMIT {
        multiply_karatsuba(accumulator, shorter, longer);
    } else if shorter.len() <= TOOM_3_LIMIT {
        multiply_toom_3(accumulator, shorter, longer);
    } else {
        multiply_accumulate_by_transform(accumulator, shorter, longer);
    }
}

/// Karatsuba for a longer operand at least twice the shorter: split only
/// the longer one in half, `shorter * (low + high * base^half)`, which is the
/// Karatsuba formula with the shorter operand's high half zero.
fn multiply_half_karatsuba(accumulator: &mut [u64], shorter: &[u64], longer: &[u64]) {
    let half = longer.len() / 2;
    let (low, high) = longer.split_at(half);

    multiply_accumulate(accumulator, shorter, low);
    multiply_accumulate(&mut accumulator[half..], shorter, high);
}

/// Karatsuba: with `x = x0 + x1 * b` and `y = y0 + y1 * b`,
/// `x * y = p2 * b^2 + (p0 + p2 - p1) * b + p0` for the three products
/// `p0 = x0 * y0`, `p2 = x1 * y1` and `p1 = (x1 - x0) * (y1 - y0)`. The shifts
/// are offsets into the accumulator, and one scratch buffer serves every
/// product. `p1` goes last because it may be negative and the accumulator
/// never may.
fn multiply_karatsuba(accumulator: &mut [u64], shorter: &[u64], longer: &[u64]) {
    // Splitting at half the shorter operand, not the longer, is markedly
    // faster when the two differ in length.
    let half = shorter.len() / 2;
    let (shorter_low, shorter_high) = shorter.split_at(half);
    let (longer_low, longer_high) = longer.split_at(half);

    // Each high half is at least as long as its low half.
    let length = shorter_high.len() + longer_high.len() + 1;
    let mut product = vec![0; length];

    // p2 = x1 * y1
    multiply_accumulate(&mut product, shorter_high, longer_high);
    trim_zero_top_digits(&mut product);

    add_in_place(&mut accumulator[half..], &product);
    add_in_place(&mut accumulator[half * 2..], &product);

    // p0 = x0 * y0
    product.clear();
    product.resize(length, 0);
    multiply_accumulate(&mut product, shorter_low, longer_low);
    trim_zero_top_digits(&mut product);

    add_in_place(accumulator, &product);
    add_in_place(&mut accumulator[half..], &product);

    // p1 = (x1 - x0) * (y1 - y0)
    let (shorter_sign, shorter_difference) = subtract_with_sign(shorter_high, shorter_low);
    let (longer_sign, longer_difference) = subtract_with_sign(longer_high, longer_low);

    match (shorter_sign, longer_sign) {
        (Ordering::Equal, _) | (_, Ordering::Equal) => {}
        (Ordering::Greater, Ordering::Greater) | (Ordering::Less, Ordering::Less) => {
            product.clear();
            product.resize(length, 0);

            multiply_accumulate(&mut product, &shorter_difference, &longer_difference);
            trim_zero_top_digits(&mut product);

            subtract_in_place(&mut accumulator[half..], &product);
        }
        (Ordering::Greater, Ordering::Less) | (Ordering::Less, Ordering::Greater) => {
            multiply_accumulate(
                &mut accumulator[half..],
                &shorter_difference,
                &longer_difference,
            );
        }
    }
}

/// Toom-3: both operands split into three parts, read as polynomials of
/// degree two in `t = base^part`, whose product is found by evaluating it at
/// 0, 1, -1, -2 and infinity and interpolating (Bodrato's sequence). The
/// intermediates are signed, so they are `ArbitraryInteger`s.
fn multiply_toom_3(accumulator: &mut [u64], shorter: &[u64], longer: &[u64]) {
    let part = longer.len() / 3 + 1;

    let shorter_low_length = Ord::min(shorter.len(), part);
    let shorter_middle_length = Ord::min(shorter.len() - shorter_low_length, part);

    let longer_low_length = part;
    let longer_middle_length = Ord::min(longer.len() - longer_low_length, part);

    let piece = |digits: &[u64]| ArbitraryInteger::from_parts(false, digits.to_vec());

    // shorter(t) = shorter_high * t^2 + shorter_middle * t + shorter_low
    let shorter_low = piece(&shorter[..shorter_low_length]);
    let shorter_middle =
        piece(&shorter[shorter_low_length..shorter_low_length + shorter_middle_length]);
    let shorter_high = piece(&shorter[shorter_low_length + shorter_middle_length..]);

    // longer(t) = longer_high * t^2 + longer_middle * t + longer_low
    let longer_low = piece(&longer[..longer_low_length]);
    let longer_middle = piece(&longer[longer_low_length..longer_low_length + longer_middle_length]);
    let longer_high = piece(&longer[longer_low_length + longer_middle_length..]);

    // The product w(t) = shorter(t) * longer(t) is evaluated at:
    //   w(0)        = shorter_low * longer_low
    //   w(1)        = (high + middle + low) * (high + middle + low)
    //   w(-1)       = (high - middle + low) * (high - middle + low)
    //   w(-2)       = (4 high - 2 middle + low) * (4 high - 2 middle + low)
    //   w(infinity) = shorter_high * longer_high
    let shorter_outer_sum = shorter_low.add(&shorter_high);
    let longer_outer_sum = longer_low.add(&longer_high);
    let shorter_at_minus_one = shorter_outer_sum.subtract(&shorter_middle);
    let longer_at_minus_one = longer_outer_sum.subtract(&longer_middle);

    let at_zero = shorter_low.multiply(&longer_low);
    let at_infinity = shorter_high.multiply(&longer_high);
    let at_one = shorter_outer_sum
        .add(&shorter_middle)
        .multiply(&longer_outer_sum.add(&longer_middle));
    let at_minus_one = shorter_at_minus_one.multiply(&longer_at_minus_one);
    let at_minus_two = shorter_at_minus_one
        .add(&shorter_high)
        .doubled()
        .subtract(&shorter_low)
        .multiply(
            &longer_at_minus_one
                .add(&longer_high)
                .doubled()
                .subtract(&longer_low),
        );

    // Interpolation, Bodrato's sequence. The divisions are exact.
    let mut third = at_minus_two.subtract(&at_one).divided_exactly_by_three();
    let mut first = at_one.subtract(&at_minus_one).halved_exactly();
    let mut second = at_minus_one.subtract(&at_zero);
    third = second
        .subtract(&third)
        .halved_exactly()
        .add(&at_infinity.doubled());
    second = second.add(&first.subtract(&at_infinity));
    first = first.subtract(&third);

    // Recomposition: each coefficient added or subtracted at its power of
    // `t`, highest first, so the accumulator never passes below zero.
    let coefficients = [&at_zero, &first, &second, &third, &at_infinity];
    for (power, coefficient) in coefficients.iter().enumerate().rev() {
        let target = &mut accumulator[part * power..];
        if coefficient.is_negative() {
            subtract_in_place(target, coefficient.magnitude());
        } else {
            add_in_place(target, coefficient.magnitude());
        }
    }
}

/// `left * right`, trimmed.
pub(super) fn multiply_magnitudes(left: &[u64], right: &[u64]) -> Vec<u64> {
    if left.is_empty() || right.is_empty() {
        return Vec::new();
    }
    let mut product = vec![0; left.len() + right.len() + 1];
    multiply_accumulate(&mut product, left, right);
    trim_zero_top_digits(&mut product);
    product
}

/// `left * right` by long multiplication alone: the oracle the faster
/// algorithms are checked against.
#[cfg(test)]
pub(super) fn multiply_schoolbook(left: &[u64], right: &[u64]) -> Vec<u64> {
    let mut product = vec![0; left.len() + right.len() + 1];
    multiply_long(&mut product, left, right);
    trim_zero_top_digits(&mut product);
    product
}

/// `left - right` as a sign and a magnitude: `Greater` for a positive
/// difference, `Less` for a negative one, `Equal` (and an empty magnitude)
/// for zero.
fn subtract_with_sign(left: &[u64], right: &[u64]) -> (Ordering, Vec<u64>) {
    match compare_magnitudes(left, right) {
        Ordering::Greater => (Ordering::Greater, subtract_magnitudes(left, right)),
        Ordering::Less => (Ordering::Less, subtract_magnitudes(right, left)),
        Ordering::Equal => (Ordering::Equal, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::{multiply_magnitudes, multiply_schoolbook};

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

    #[test]
    fn every_algorithm_agrees_with_schoolbook() {
        // (shorter, longer): long multiplication, half-Karatsuba (the longer
        // at least twice the shorter), Karatsuba (at most 256), Toom-3 (at
        // most 512), the number-theoretic transform (above), and half-Karatsuba
        // whose halves reach the transform.
        for (shorter, longer) in [
            (3, 5),
            (33, 40),
            (40, 90),
            (100, 120),
            (300, 310),
            (700, 700),
            (600, 600),
            (900, 2000),
        ] {
            let left = digits(shorter, 1);
            let right = digits(longer, 2);
            assert_eq!(
                multiply_magnitudes(&left, &right),
                multiply_schoolbook(&left, &right),
                "{shorter} x {longer} digits"
            );
            assert_eq!(
                multiply_magnitudes(&right, &left),
                multiply_schoolbook(&left, &right)
            );
        }
    }

    #[test]
    fn all_ones_digits_carry_through_every_algorithm() {
        for count in [1, 33, 257, 600, 1100] {
            let ones = vec![u64::MAX; count];
            assert_eq!(
                multiply_magnitudes(&ones, &ones),
                multiply_schoolbook(&ones, &ones)
            );
        }
    }
}
