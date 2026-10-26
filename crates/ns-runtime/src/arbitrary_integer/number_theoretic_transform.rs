// Adapted from num-bigint (https://github.com/rust-num/num-bigint), commit
// 41f0b035d12971c5c7f7501917e31f0f6439c26c, src/biguint/ntt.rs.
// Copyright (c) 2014 The Rust Project Developers. MIT licence; see
// THIRD-PARTY-NOTICES.md at the repository root. Changed for ns-next: names
// written out, `BigUint` replaced by digit slices, no `unsafe`.

//! Multiplication by the number-theoretic transform, for operands above 512
//! digits. The product's digits are the convolution of the operands'
//! digits; the transform computes that convolution modulo a prime in
//! quasi-linear time, and since a convolution's terms can exceed any one
//! prime, it is computed modulo two or three primes and the residues
//! combined by the Chinese remainder theorem.
//!
//! Every modular value is kept in Montgomery form, `value * 2^64 mod PRIME`,
//! so that a modular product needs no division.

use std::cmp::max;
use std::iter::zip;

/// `(gcd, left_coefficient, right_coefficient)` with
/// `left * left_coefficient + right * right_coefficient == gcd`: the
/// extended Euclidean algorithm.
const fn extended_euclid(mut left: i128, mut right: i128) -> (i128, i128, i128) {
    assert!(left > 0 && right > 0);
    // The coefficients, as a row-major 2x2 matrix.
    let mut matrix = if left > right {
        (left, right) = (right, left);
        [0, 1, 1, 0]
    } else {
        [1, 0, 0, 1]
    };
    loop {
        if left == 0 {
            break (right, matrix[1], matrix[3]);
        }
        let (quotient, remainder) = (right / left, right % left);
        (left, right) = (remainder, left);
        matrix = [
            matrix[1] - quotient * matrix[0],
            matrix[0],
            matrix[3] - quotient * matrix[2],
            matrix[2],
        ];
    }
}

/// `value^-1 mod modulus`, where a zero modulus stands for 2^64.
const fn inverse_modulo_any(value: u64, modulus: u64) -> u64 {
    let wide_modulus = if modulus == 0 {
        1_i128 << 64
    } else {
        modulus as i128
    };
    let (divisor, mut coefficient, _) = extended_euclid(value as i128, wide_modulus);
    assert!(divisor == 1);
    if coefficient < 0 {
        coefficient += wide_modulus;
    }
    assert!(coefficient > 0 && coefficient < 1_i128 << 64);
    coefficient as u64
}

/// `value^-1 mod modulus`, for a modulus that is not zero.
const fn inverse_modulo(value: u64, modulus: u64) -> u64 {
    assert!(modulus != 0);
    inverse_modulo_any(value, modulus)
}

/// `value^-1 mod 2^64`.
const fn inverse_modulo_two_to_the_64(value: u64) -> u64 {
    inverse_modulo_any(value, 0)
}

/// Arithmetic modulo the prime `PRIME` in Montgomery form, with the radix
/// 2^64.
///
/// num-bigint keeps this apart from its general Montgomery code on purpose:
/// with the modulus a constant, the subtractive reduction stays `const`,
/// handles primes close to 2^64 without a 129-bit addition, and allows the
/// fused and delayed reductions the transform's kernels need.
struct Montgomery<const PRIME: u64>;

impl<const PRIME: u64> Montgomery<PRIME> {
    /// 2^64 mod PRIME, which is one in Montgomery form.
    const RADIX: u64 = ((1_u128 << 64) % PRIME as u128) as u64;
    /// 2^128 mod PRIME: a Montgomery product with it brings a plain value
    /// into Montgomery form.
    const RADIX_SQUARED: u64 = (Self::RADIX as u128 * Self::RADIX as u128 % PRIME as u128) as u64;
    /// PRIME^-1 mod 2^64.
    const PRIME_INVERSE: u64 = inverse_modulo_two_to_the_64(PRIME);

    /// The longest transform this prime supports: every factor of two, three
    /// and five of `PRIME - 1`.
    const MAX_TRANSFORM_LENGTH: u64 = 2_u64.pow(Self::FACTORS_OF_TWO)
        * 3_u64.pow(Self::FACTORS_OF_THREE)
        * 5_u64.pow(Self::FACTORS_OF_FIVE);

    /// A root of unity of exact order `MAX_TRANSFORM_LENGTH`, in Montgomery
    /// form.
    ///
    /// Some element's order is a multiple of `MAX_TRANSFORM_LENGTH`; raised
    /// to `(PRIME - 1) / MAX_TRANSFORM_LENGTH` it has exactly that order. It
    /// is enough to test `candidate^((PRIME - 1) / factor) != 1` for the
    /// factors 2, 3 and 5 alone: the quotient `(PRIME - 1) /
    /// MAX_TRANSFORM_LENGTH` holds none of them, so the other prime factors
    /// of `PRIME - 1` are absorbed by the final power and do not affect the
    /// root's order. `PRIME` must be prime for this to be a field, which is
    /// not checked here.
    const ROOT: u64 = {
        assert!(Self::MAX_TRANSFORM_LENGTH % 4050 == 0);
        let mut candidate = Self::RADIX;
        loop {
            if Self::power(candidate, PRIME / 2) != Self::RADIX
                && Self::power(candidate, PRIME / 3) != Self::RADIX
                && Self::power(candidate, PRIME / 5) != Self::RADIX
            {
                break Self::power(candidate, PRIME / Self::MAX_TRANSFORM_LENGTH);
            }
            candidate = Self::add(candidate, Self::RADIX);
        }
    };

    const FACTORS_OF_TWO: u32 = Self::factor_count(2);
    const FACTORS_OF_THREE: u32 = Self::factor_count(3);
    const FACTORS_OF_FIVE: u32 = Self::factor_count(5);

    /// How often `divisor` divides `PRIME - 1`.
    const fn factor_count(divisor: u64) -> u32 {
        let (mut remaining, mut count) = (PRIME - 1, 0);
        while remaining % divisor == 0 {
            remaining /= divisor;
            count += 1;
        }
        count
    }

    /// Montgomery reduction, `value * 2^-64 mod PRIME`.
    const fn reduce(value: u128) -> u64 {
        let factor = (value as u64).wrapping_mul(Self::PRIME_INVERSE);
        let correction = ((factor as u128 * PRIME as u128) >> 64) as u64;
        let (reduced, overflow) = ((value >> 64) as u64).overflowing_sub(correction);
        if overflow {
            reduced.wrapping_add(PRIME)
        } else {
            reduced
        }
    }

    /// `left * right * 2^-64 mod PRIME`.
    const fn multiply(left: u64, right: u64) -> u64 {
        Self::reduce(left as u128 * right as u128)
    }

    /// `left * right * 2^-64 - subtrahend mod PRIME`, fused.
    const fn multiply_subtract(left: u64, right: u64, subtrahend: u64) -> u64 {
        let product = left as u128 * right as u128;
        let low = product as u64;
        let high = Self::subtract((product >> 64) as u64, subtrahend);
        Self::reduce(low as u128 | ((high as u128) << 64))
    }

    /// `base^exponent`, the base and the result in Montgomery form.
    const fn power(mut base: u64, mut exponent: u64) -> u64 {
        let mut result = Self::RADIX;
        while exponent > 0 {
            if !exponent.is_multiple_of(2) {
                result = Self::multiply(result, base);
            }
            exponent /= 2;
            base = Self::multiply(base, base);
        }
        result
    }

    /// `factor * reduce(value)`, left unreduced as a `u128`, given
    /// `factor_low == reduce_low(factor)`, which the caller computes once
    /// and is responsible for.
    const fn multiply_unreduced(value: u128, factor: u64, factor_low: u64) -> u128 {
        let high_product = factor as u128 * (value >> 64);
        let low_product = factor_low as u128 * (value as u64 as u128);
        let (difference, overflow) = high_product.overflowing_sub(low_product);
        if overflow {
            difference.wrapping_add((PRIME as u128) << 64)
        } else {
            difference
        }
    }

    /// `subtract(0, reduce(value))`, fast.
    const fn reduce_low(value: u64) -> u64 {
        let factor = value.wrapping_mul(Self::PRIME_INVERSE);
        ((factor as u128 * PRIME as u128) >> 64) as u64
    }

    /// `left + right mod PRIME`, in `[0, PRIME)`.
    const fn add(left: u64, right: u64) -> u64 {
        Self::subtract(left, PRIME.wrapping_sub(right))
    }

    /// `left + right mod PRIME`, in `[0, 2^64)`: not fully reduced, which a
    /// following Montgomery product tolerates.
    const fn add_unreduced(left: u64, right: u64) -> u64 {
        let (sum, overflow) = left.overflowing_add(right);
        if overflow {
            sum.wrapping_sub(PRIME)
        } else {
            sum
        }
    }

    /// `add_unreduced` where an inverse kernel multiplies the sum by a
    /// twiddle factor next, `add` otherwise.
    const fn add_for<const INVERSE: bool, const TWIDDLE: bool>(left: u64, right: u64) -> u64 {
        if INVERSE && TWIDDLE {
            Self::add_unreduced(left, right)
        } else {
            Self::add(left, right)
        }
    }

    /// `left - right mod PRIME`, in `[0, PRIME)`.
    const fn subtract(left: u64, right: u64) -> u64 {
        let (difference, overflow) = left.overflowing_sub(right);
        if overflow {
            difference.wrapping_add(PRIME)
        } else {
            difference
        }
    }
}

/// How one transform is computed: `length == base_length * block_count`,
/// with `block_count` a divisor of the prime's `MAX_TRANSFORM_LENGTH`. The
/// transform runs as the radix stages in `stages`, each a `(stage_size,
/// radix)`, down to blocks of `base_length` values, which are multiplied by
/// a direct convolution.
struct TransformPlan {
    length: usize,
    base_length: usize,
    block_count: usize,
    last_radix: usize,
    stages: Vec<(usize, usize)>,
}

impl TransformPlan {
    /// The cheapest plan of at least `minimum_length` values.
    fn build<const PRIME: u64>(minimum_length: usize) -> TransformPlan {
        assert!(minimum_length as u64 <= Montgomery::<PRIME>::MAX_TRANSFORM_LENGTH);
        let (mut best_length, mut best_cost, mut best_base_length) = (usize::MAX, usize::MAX, 1);
        for fives in 0..=Montgomery::<PRIME>::FACTORS_OF_FIVE {
            for threes in 0..=Montgomery::<PRIME>::FACTORS_OF_THREE {
                let odd_length = 5_u64.pow(fives) * 3_u64.pow(threes);
                if odd_length >= 2 * minimum_length as u64 {
                    break;
                }

                let (mut length, mut twos) = (odd_length as usize, 0);
                while length < minimum_length && twos < Montgomery::<PRIME>::FACTORS_OF_TWO {
                    length *= 2;
                    twos += 1;
                }

                if length >= minimum_length && length < best_cost {
                    let (mut remaining, mut cost) = (length, 0);
                    let mut base_length = 1;

                    // num-bigint's cost model. A radix-R stage processes all
                    // `length` values in groups of R, at about `length *
                    // weight / 100`, the weights calibrated by measurement:
                    // radix 2 at 85 (small) or 100 (large), radix 3 at 100,
                    // radix 4 at 90 or 100, radix 5 at 156 to 186 by length,
                    // radix 6 at 110 or 115. Small is up to 2^20 values,
                    // about what the second-level cache holds; radix 5 is
                    // dearest for its five modular products per butterfly,
                    // against one for radix 3 and 4.
                    let small_transform = length <= 1 << 20;
                    let radix_six_weight = if small_transform { 110 } else { 115 };
                    let radix_five_weight =
                        156 + 30 * length.saturating_sub(5 << 14).min(1 << 16) / (1 << 16);
                    let radix_four_weight = if small_transform { 90 } else { 100 };
                    let radix_three_weight = 100;
                    let radix_two_weight = if small_transform { 85 } else { 100 };

                    if length % 5 == 0 {
                        (base_length, remaining, cost) =
                            (5, remaining / 5, cost + length * 89 / 100);
                    } else if threes >= twos + 2 {
                        (base_length, remaining, cost) =
                            (9, remaining / 9, cost + length * 180 / 100);
                    } else if twos >= threes + 3 && (twos - threes) % 2 == 1 {
                        (base_length, remaining, cost) =
                            (8, remaining / 8, cost + length * 130 / 100);
                    } else if twos >= threes + 2 && threes == 0 {
                        (base_length, remaining, cost) =
                            (4, remaining / 4, cost + length * 87 / 100);
                    } else if twos == 0 && threes >= 1 {
                        (base_length, remaining, cost) =
                            (3, remaining / 3, cost + length * 86 / 100);
                    } else if threes == 0 && twos >= 1 {
                        (base_length, remaining, cost) =
                            (2, remaining / 2, cost + length * 86 / 100);
                    } else if length % 6 == 0 {
                        (base_length, remaining, cost) =
                            (6, remaining / 6, cost + length * 91 / 100);
                    }

                    let (mut has_radix_six, mut has_radix_two) = (false, false);
                    while remaining % 6 == 0 {
                        (remaining, cost) = (remaining / 6, cost + length * radix_six_weight / 100);
                        has_radix_six = true;
                    }
                    while remaining % 5 == 0 {
                        (remaining, cost) =
                            (remaining / 5, cost + length * radix_five_weight / 100);
                    }
                    while remaining % 4 == 0 {
                        (remaining, cost) =
                            (remaining / 4, cost + length * radix_four_weight / 100);
                    }
                    while remaining % 3 == 0 {
                        (remaining, cost) =
                            (remaining / 3, cost + length * radix_three_weight / 100);
                    }
                    while remaining % 2 == 0 {
                        (remaining, cost) = (remaining / 2, cost + length * radix_two_weight / 100);
                        has_radix_two = true;
                    }

                    if has_radix_six && has_radix_two {
                        // One radix-6 stage and the remaining radix-2 stage
                        // run as a radix-4 and a radix-3 stage.
                        cost -= length * radix_six_weight / 100;
                        cost -= length * radix_two_weight / 100;
                        cost += length * radix_four_weight / 100;
                        cost += length * radix_three_weight / 100;
                    }

                    // Work that grows with the length whatever the stages.
                    cost += length * 4 / 100;
                    if cost < best_cost {
                        (best_length, best_cost, best_base_length) = (length, cost, base_length);
                    }
                }
            }
        }

        let (mut sixes, mut fives, mut fours, mut threes, mut twos) = (0, 0, 0, 0, 0);
        let mut remaining = best_length / best_base_length;
        while remaining % 6 == 0 {
            remaining /= 6;
            sixes += 1;
        }
        while remaining % 5 == 0 {
            remaining /= 5;
            fives += 1;
        }
        while remaining % 4 == 0 {
            remaining /= 4;
            fours += 1;
        }
        while remaining % 3 == 0 {
            remaining /= 3;
            threes += 1;
        }
        while remaining % 2 == 0 {
            remaining /= 2;
            twos += 1;
        }
        while sixes > 0 && twos > 0 {
            sixes -= 1;
            twos -= 1;
            fours += 1;
            threes += 1;
        }

        let mut stages = vec![];
        let mut stage_size = best_length;
        for (count, radix) in [(twos, 2), (threes, 3), (fours, 4), (fives, 5), (sixes, 6)] {
            for _ in 0..count {
                stages.push((stage_size, radix));
                stage_size /= radix;
            }
        }

        TransformPlan {
            length: best_length,
            base_length: best_base_length,
            block_count: best_length / best_base_length,
            last_radix: stages.last().unwrap_or(&(1, 1)).1,
            stages,
        }
    }
}

/// The cyclic convolution of two blocks of `base_length` values into
/// `output`, where a term that wraps around is multiplied by `twiddle`: the
/// direct multiplication at the bottom of the transform.
fn convolve_base<const PRIME: u64>(output: &mut [u64], left: &[u64], right: &[u64], twiddle: u64) {
    assert_eq!(output.len(), left.len());
    assert_eq!(left.len(), right.len());

    let length = output.len();
    let twiddle_low = Montgomery::<PRIME>::reduce_low(twiddle);
    for (index, output_value) in output.iter_mut().enumerate() {
        // The sums are kept negated, so that the subtraction's overflow
        // stands in for a modular reduction.
        let mut sum: u128 = 0;
        for term in index + 1..length {
            let (difference, overflow) =
                sum.overflowing_sub(left[term] as u128 * right[index + length - term] as u128);
            sum = if overflow {
                difference.wrapping_add((PRIME as u128) << 64)
            } else {
                difference
            };
        }

        sum = Montgomery::<PRIME>::multiply_unreduced(sum, twiddle, twiddle_low);
        for term in 0..=index {
            let (difference, overflow) =
                sum.overflowing_sub(left[term] as u128 * right[index - term] as u128);
            sum = if overflow {
                difference.wrapping_add((PRIME as u128) << 64)
            } else {
                difference
            };
        }

        *output_value = Montgomery::<PRIME>::reduce(sum);
    }
}

/// The roots of unity the butterflies use, for the forward transform or,
/// with `INVERSE`, the inverse one.
struct KernelConstants<const PRIME: u64, const INVERSE: bool>;

impl<const PRIME: u64, const INVERSE: bool> KernelConstants<PRIME, INVERSE> {
    const ROOT: u64 = Montgomery::<PRIME>::power(
        Montgomery::<PRIME>::ROOT,
        if INVERSE {
            Montgomery::<PRIME>::MAX_TRANSFORM_LENGTH - 1
        } else {
            1
        },
    );

    const THIRD_ROOT: u64 =
        Montgomery::<PRIME>::power(Self::ROOT, Montgomery::<PRIME>::MAX_TRANSFORM_LENGTH / 3);
    const FOURTH_ROOT: u64 =
        Montgomery::<PRIME>::power(Self::ROOT, Montgomery::<PRIME>::MAX_TRANSFORM_LENGTH / 4);
    const FIFTH_ROOT: u64 =
        Montgomery::<PRIME>::power(Self::ROOT, Montgomery::<PRIME>::MAX_TRANSFORM_LENGTH / 5);
    const SIXTH_ROOT: u64 =
        Montgomery::<PRIME>::power(Self::ROOT, Montgomery::<PRIME>::MAX_TRANSFORM_LENGTH / 6);

    /// The radix-5 butterfly's five constants, from the fifth root `root`:
    /// `1 + 4^-1`, `4^-1 * (2 root + 2 root^4 + 1)`, `2^-1 * (root -
    /// root^4)`, `2^-1 * (2 root + 2 root^2 + 1)` and `2^-1 * (2 root^2 + 2
    /// root^4 + 1)`, all modulo PRIME.
    const RADIX_FIVE: [u64; 5] = {
        let root = Self::FIFTH_ROOT;
        let root_squared = Montgomery::<PRIME>::power(root, 2);
        let root_fourth = Montgomery::<PRIME>::power(root, 4);
        let half = Montgomery::<PRIME>::multiply(
            Montgomery::<PRIME>::RADIX_SQUARED,
            inverse_modulo(2, PRIME),
        );
        let quarter = Montgomery::<PRIME>::multiply(
            Montgomery::<PRIME>::RADIX_SQUARED,
            inverse_modulo(4, PRIME),
        );
        [
            Montgomery::<PRIME>::add(Montgomery::<PRIME>::RADIX, quarter),
            Montgomery::<PRIME>::add(
                Montgomery::<PRIME>::multiply(half, Montgomery::<PRIME>::add(root, root_fourth)),
                quarter,
            ),
            Montgomery::<PRIME>::multiply(half, Montgomery::<PRIME>::subtract(root, root_fourth)),
            Montgomery::<PRIME>::add(Montgomery::<PRIME>::add(root, root_squared), half),
            Montgomery::<PRIME>::add(Montgomery::<PRIME>::add(root_squared, root_fourth), half),
        ]
    };
}

// The butterflies of radix 2 to 6. The forward transform (decimation in
// frequency) multiplies the inputs by the twiddle factors first; the inverse
// one (decimation in time) multiplies the outputs last. A kernel without
// `TWIDDLE` serves the first block of a stage, whose twiddle factors are all
// one.

const fn radix_two_kernel<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    first: u64,
    mut second: u64,
    twiddle: u64,
) -> [u64; 2] {
    if !INVERSE && TWIDDLE {
        second = Montgomery::<PRIME>::multiply(twiddle, second);
    }
    let output_zero = Montgomery::<PRIME>::add(first, second);
    let mut output_one = Montgomery::<PRIME>::subtract(first, second);
    if INVERSE && TWIDDLE {
        output_one = Montgomery::<PRIME>::multiply(twiddle, output_one);
    }
    [output_zero, output_one]
}

fn radix_two_block<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    block: &mut [u64],
    twiddle: u64,
) {
    let twiddle = if TWIDDLE { twiddle } else { 0 };
    let part = block.len() / 2;
    let (firsts, seconds) = block.split_at_mut(part);
    zip(firsts, seconds).for_each(|(first, second)| {
        [*first, *second] = radix_two_kernel::<PRIME, INVERSE, TWIDDLE>(*first, *second, twiddle);
    });
}

const fn radix_three_kernel<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    first: u64,
    mut second: u64,
    mut third: u64,
    [twiddle, twiddle_squared]: [u64; 2],
) -> [u64; 3] {
    if !INVERSE && TWIDDLE {
        second = Montgomery::<PRIME>::multiply(twiddle, second);
        third = Montgomery::<PRIME>::multiply(twiddle_squared, third);
    }
    let rotated_difference = Montgomery::<PRIME>::multiply(
        KernelConstants::<PRIME, INVERSE>::THIRD_ROOT,
        Montgomery::<PRIME>::subtract(second, third),
    );
    let output_zero = Montgomery::<PRIME>::add(first, Montgomery::<PRIME>::add(second, third));
    let mut output_one = Montgomery::<PRIME>::subtract(
        first,
        Montgomery::<PRIME>::subtract(third, rotated_difference),
    );
    let mut output_two = Montgomery::<PRIME>::subtract(
        Montgomery::<PRIME>::subtract(first, second),
        rotated_difference,
    );
    if INVERSE && TWIDDLE {
        output_one = Montgomery::<PRIME>::multiply(twiddle, output_one);
        output_two = Montgomery::<PRIME>::multiply(twiddle_squared, output_two);
    }
    [output_zero, output_one, output_two]
}

fn radix_three_block<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    block: &mut [u64],
    twiddle: u64,
) {
    let twiddles = if TWIDDLE {
        let twiddle_squared = Montgomery::<PRIME>::multiply(twiddle, twiddle);
        [twiddle, twiddle_squared]
    } else {
        [0; 2]
    };
    let part = block.len() / 3;
    let (firsts, rest) = block.split_at_mut(part);
    let (seconds, thirds) = rest.split_at_mut(part);
    zip(zip(firsts, seconds), thirds).for_each(|((first, second), third)| {
        [*first, *second, *third] =
            radix_three_kernel::<PRIME, INVERSE, TWIDDLE>(*first, *second, *third, twiddles);
    });
}

const fn radix_four_kernel<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    first: u64,
    mut second: u64,
    mut third: u64,
    mut fourth: u64,
    [twiddle, twiddle_squared, twiddle_cubed]: [u64; 3],
) -> [u64; 4] {
    if !INVERSE && TWIDDLE {
        second = Montgomery::<PRIME>::multiply(twiddle, second);
        third = Montgomery::<PRIME>::multiply(twiddle_squared, third);
        fourth = Montgomery::<PRIME>::multiply(twiddle_cubed, fourth);
    }
    let first_plus_third = Montgomery::<PRIME>::add(first, third);
    let first_minus_third = Montgomery::<PRIME>::subtract(first, third);
    let second_plus_fourth = Montgomery::<PRIME>::add(second, fourth);
    let second_minus_fourth = Montgomery::<PRIME>::subtract(second, fourth);
    let rotated_second_minus_fourth = Montgomery::<PRIME>::multiply(
        KernelConstants::<PRIME, INVERSE>::FOURTH_ROOT,
        second_minus_fourth,
    );
    let output_zero = Montgomery::<PRIME>::add(first_plus_third, second_plus_fourth);
    let mut output_one = Montgomery::<PRIME>::add_for::<INVERSE, TWIDDLE>(
        first_minus_third,
        rotated_second_minus_fourth,
    );
    let mut output_two = Montgomery::<PRIME>::subtract(first_plus_third, second_plus_fourth);
    let mut output_three =
        Montgomery::<PRIME>::subtract(first_minus_third, rotated_second_minus_fourth);
    if INVERSE && TWIDDLE {
        output_one = Montgomery::<PRIME>::multiply(twiddle, output_one);
        output_two = Montgomery::<PRIME>::multiply(twiddle_squared, output_two);
        output_three = Montgomery::<PRIME>::multiply(twiddle_cubed, output_three);
    }
    [output_zero, output_one, output_two, output_three]
}

fn radix_four_block<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    block: &mut [u64],
    twiddle: u64,
) {
    let twiddles = if TWIDDLE {
        let twiddle_squared = Montgomery::<PRIME>::multiply(twiddle, twiddle);
        let twiddle_cubed = Montgomery::<PRIME>::multiply(twiddle, twiddle_squared);
        [twiddle, twiddle_squared, twiddle_cubed]
    } else {
        [0; 3]
    };
    let part = block.len() / 4;
    let (firsts, rest) = block.split_at_mut(part);
    let (seconds, rest) = rest.split_at_mut(part);
    let (thirds, fourths) = rest.split_at_mut(part);
    zip(zip(zip(firsts, seconds), thirds), fourths).for_each(
        |(((first, second), third), fourth)| {
            [*first, *second, *third, *fourth] = radix_four_kernel::<PRIME, INVERSE, TWIDDLE>(
                *first, *second, *third, *fourth, twiddles,
            );
        },
    );
}

/// The radix-5 butterfly, with five modular products; its products are
/// numbered as num-bigint numbers them.
const fn radix_five_kernel<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    first: u64,
    mut second: u64,
    mut third: u64,
    mut fourth: u64,
    mut fifth: u64,
    [twiddle, twiddle_squared, twiddle_cubed, twiddle_fourth]: [u64; 4],
) -> [u64; 5] {
    if !INVERSE && TWIDDLE {
        second = Montgomery::<PRIME>::multiply(twiddle, second);
        third = Montgomery::<PRIME>::multiply(twiddle_squared, third);
        fourth = Montgomery::<PRIME>::multiply(twiddle_cubed, fourth);
        fifth = Montgomery::<PRIME>::multiply(twiddle_fourth, fifth);
    }
    let constants = KernelConstants::<PRIME, INVERSE>::RADIX_FIVE;
    let second_plus_fifth = Montgomery::<PRIME>::add(second, fifth);
    let third_plus_fourth = Montgomery::<PRIME>::add(third, fourth);
    let second_minus_fifth = Montgomery::<PRIME>::subtract(second, fifth);
    let fourth_minus_third = Montgomery::<PRIME>::subtract(fourth, third);
    let sum_of_sums = Montgomery::<PRIME>::add(second_plus_fifth, third_plus_fourth);
    let difference_of_sums = Montgomery::<PRIME>::subtract(second_plus_fifth, third_plus_fourth);
    let sum_of_differences =
        Montgomery::<PRIME>::add_unreduced(second_minus_fifth, fourth_minus_third);
    let product_one = Montgomery::<PRIME>::add(first, sum_of_sums);
    let product_two =
        Montgomery::<PRIME>::multiply_subtract(constants[0], sum_of_sums, product_one);
    let product_three = Montgomery::<PRIME>::multiply(constants[1], difference_of_sums);
    let product_four = Montgomery::<PRIME>::multiply(constants[2], sum_of_differences);
    let product_five =
        Montgomery::<PRIME>::multiply_subtract(constants[3], fourth_minus_third, product_four);
    let product_six = Montgomery::<PRIME>::multiply_subtract(
        PRIME.wrapping_sub(constants[4]),
        second_minus_fifth,
        product_four,
    );
    let partial_one = Montgomery::<PRIME>::subtract(product_three, product_two);
    let partial_two = Montgomery::<PRIME>::add(product_two, product_three);
    let output_zero = product_one;
    let mut output_one = Montgomery::<PRIME>::subtract(partial_one, product_five);
    let mut output_two =
        Montgomery::<PRIME>::subtract(Montgomery::<PRIME>::subtract(0, partial_two), product_six);
    let mut output_three = Montgomery::<PRIME>::subtract(product_six, partial_two);
    let mut output_four =
        Montgomery::<PRIME>::add_for::<INVERSE, TWIDDLE>(partial_one, product_five);
    if INVERSE && TWIDDLE {
        output_one = Montgomery::<PRIME>::multiply(twiddle, output_one);
        output_two = Montgomery::<PRIME>::multiply(twiddle_squared, output_two);
        output_three = Montgomery::<PRIME>::multiply(twiddle_cubed, output_three);
        output_four = Montgomery::<PRIME>::multiply(twiddle_fourth, output_four);
    }
    [
        output_zero,
        output_one,
        output_two,
        output_three,
        output_four,
    ]
}

fn radix_five_block<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    block: &mut [u64],
    twiddle: u64,
) {
    let twiddles = if TWIDDLE {
        let twiddle_squared = Montgomery::<PRIME>::multiply(twiddle, twiddle);
        let twiddle_cubed = Montgomery::<PRIME>::multiply(twiddle, twiddle_squared);
        let twiddle_fourth = Montgomery::<PRIME>::multiply(twiddle_squared, twiddle_squared);
        [twiddle, twiddle_squared, twiddle_cubed, twiddle_fourth]
    } else {
        [0; 4]
    };
    let part = block.len() / 5;
    let (firsts, rest) = block.split_at_mut(part);
    let (seconds, rest) = rest.split_at_mut(part);
    let (thirds, rest) = rest.split_at_mut(part);
    let (fourths, fifths) = rest.split_at_mut(part);
    zip(zip(zip(zip(firsts, seconds), thirds), fourths), fifths).for_each(
        |((((first, second), third), fourth), fifth)| {
            [*first, *second, *third, *fourth, *fifth] =
                radix_five_kernel::<PRIME, INVERSE, TWIDDLE>(
                    *first, *second, *third, *fourth, *fifth, twiddles,
                );
        },
    );
}

/// The radix-6 butterfly: three radix-2 butterflies, then two radix-3 ones.
const fn radix_six_kernel<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    mut first: u64,
    mut second: u64,
    mut third: u64,
    mut fourth: u64,
    mut fifth: u64,
    mut sixth: u64,
    [twiddle, twiddle_squared, twiddle_cubed, twiddle_fourth, twiddle_fifth]: [u64; 5],
) -> [u64; 6] {
    if !INVERSE && TWIDDLE {
        second = Montgomery::<PRIME>::multiply(twiddle, second);
        third = Montgomery::<PRIME>::multiply(twiddle_squared, third);
        fourth = Montgomery::<PRIME>::multiply(twiddle_cubed, fourth);
        fifth = Montgomery::<PRIME>::multiply(twiddle_fourth, fifth);
        sixth = Montgomery::<PRIME>::multiply(twiddle_fifth, sixth);
    }
    (first, fourth) = (
        Montgomery::<PRIME>::add(first, fourth),
        Montgomery::<PRIME>::subtract(first, fourth),
    );
    (second, fifth) = (
        Montgomery::<PRIME>::add(second, fifth),
        Montgomery::<PRIME>::subtract(second, fifth),
    );
    (third, sixth) = (
        Montgomery::<PRIME>::add(third, sixth),
        Montgomery::<PRIME>::subtract(third, sixth),
    );
    let rotated_second_minus_third = Montgomery::<PRIME>::multiply(
        KernelConstants::<PRIME, INVERSE>::SIXTH_ROOT,
        Montgomery::<PRIME>::subtract(second, third),
    );
    let output_zero = Montgomery::<PRIME>::add(first, Montgomery::<PRIME>::add(second, third));
    let mut output_two = Montgomery::<PRIME>::subtract(
        first,
        Montgomery::<PRIME>::subtract(second, rotated_second_minus_third),
    );
    let mut output_four = Montgomery::<PRIME>::subtract(
        Montgomery::<PRIME>::subtract(first, third),
        rotated_second_minus_third,
    );
    let rotated_fifth_plus_sixth = Montgomery::<PRIME>::multiply(
        KernelConstants::<PRIME, INVERSE>::SIXTH_ROOT,
        Montgomery::<PRIME>::add_unreduced(fifth, sixth),
    );
    let mut output_one = Montgomery::<PRIME>::subtract(
        fourth,
        Montgomery::<PRIME>::subtract(sixth, rotated_fifth_plus_sixth),
    );
    let mut output_three =
        Montgomery::<PRIME>::subtract(fourth, Montgomery::<PRIME>::subtract(fifth, sixth));
    let mut output_five = Montgomery::<PRIME>::subtract(
        fourth,
        Montgomery::<PRIME>::subtract(rotated_fifth_plus_sixth, fifth),
    );
    if INVERSE && TWIDDLE {
        output_one = Montgomery::<PRIME>::multiply(twiddle, output_one);
        output_two = Montgomery::<PRIME>::multiply(twiddle_squared, output_two);
        output_three = Montgomery::<PRIME>::multiply(twiddle_cubed, output_three);
        output_four = Montgomery::<PRIME>::multiply(twiddle_fourth, output_four);
        output_five = Montgomery::<PRIME>::multiply(twiddle_fifth, output_five);
    }
    [
        output_zero,
        output_one,
        output_two,
        output_three,
        output_four,
        output_five,
    ]
}

fn radix_six_block<const PRIME: u64, const INVERSE: bool, const TWIDDLE: bool>(
    block: &mut [u64],
    twiddle: u64,
) {
    let twiddles = if TWIDDLE {
        let twiddle_squared = Montgomery::<PRIME>::multiply(twiddle, twiddle);
        let twiddle_cubed = Montgomery::<PRIME>::multiply(twiddle, twiddle_squared);
        let twiddle_fourth = Montgomery::<PRIME>::multiply(twiddle_squared, twiddle_squared);
        let twiddle_fifth = Montgomery::<PRIME>::multiply(twiddle_squared, twiddle_cubed);
        [
            twiddle,
            twiddle_squared,
            twiddle_cubed,
            twiddle_fourth,
            twiddle_fifth,
        ]
    } else {
        [0; 5]
    };
    let part = block.len() / 6;
    let (firsts, rest) = block.split_at_mut(part);
    let (seconds, rest) = rest.split_at_mut(part);
    let (thirds, rest) = rest.split_at_mut(part);
    let (fourths, rest) = rest.split_at_mut(part);
    let (fifths, sixths) = rest.split_at_mut(part);
    zip(
        zip(zip(zip(zip(firsts, seconds), thirds), fourths), fifths),
        sixths,
    )
    .for_each(|(((((first, second), third), fourth), fifth), sixth)| {
        [*first, *second, *third, *fourth, *fifth, *sixth] =
            radix_six_kernel::<PRIME, INVERSE, TWIDDLE>(
                *first, *second, *third, *fourth, *fifth, *sixth, twiddles,
            );
    });
}

/// A kernel run over one block of a stage, given the block's twiddle factor.
type BlockKernel = fn(&mut [u64], u64);

/// Runs one stage over the blocks of `values`: the first block without
/// twiddle factors, each later one with its own, taken from `twiddles` at
/// `twiddle_index`, which advances by one per block.
fn run_stage(
    values: &mut [u64],
    stage_size: usize,
    twiddles: &[u64],
    twiddle_index: &mut usize,
    without_twiddle: BlockKernel,
    with_twiddle: BlockKernel,
) {
    let mut blocks = values.chunks_exact_mut(stage_size);
    without_twiddle(blocks.next().expect("a stage has at least one block"), 0);
    *twiddle_index += 1;
    for block in blocks {
        with_twiddle(block, twiddles[*twiddle_index]);
        *twiddle_index += 1;
    }
}

/// The forward transform, decimation in frequency, stage by stage; with
/// `INVERSE`, the inverse one, decimation in time, the stages in reverse.
fn transform<const PRIME: u64, const INVERSE: bool>(
    plan: &TransformPlan,
    values: &mut [u64],
    twiddles: &[u64],
) {
    let values = &mut values[..plan.length];
    let mut twiddle_index = 0;
    for step in 0..plan.stages.len() {
        let stage_index = if INVERSE {
            plan.stages.len() - 1 - step
        } else {
            step
        };
        let (stage_size, radix) = plan.stages[stage_index];
        debug_assert_eq!(plan.length % stage_size, 0);
        debug_assert_eq!(stage_size % radix, 0);

        let (without_twiddle, with_twiddle): (BlockKernel, BlockKernel) = match radix {
            2 => (
                radix_two_block::<PRIME, INVERSE, false>,
                radix_two_block::<PRIME, INVERSE, true>,
            ),
            3 => (
                radix_three_block::<PRIME, INVERSE, false>,
                radix_three_block::<PRIME, INVERSE, true>,
            ),
            4 => (
                radix_four_block::<PRIME, INVERSE, false>,
                radix_four_block::<PRIME, INVERSE, true>,
            ),
            5 => (
                radix_five_block::<PRIME, INVERSE, false>,
                radix_five_block::<PRIME, INVERSE, true>,
            ),
            6 => (
                radix_six_block::<PRIME, INVERSE, false>,
                radix_six_block::<PRIME, INVERSE, true>,
            ),
            _ => unreachable!("a plan has stages of radix 2 to 6 only"),
        };
        run_stage(
            values,
            stage_size,
            twiddles,
            &mut twiddle_index,
            without_twiddle,
            with_twiddle,
        );
    }
}

/// Writes the twiddle factors of the last of `stages` into `output`,
/// answering how many there are.
fn compute_twiddle_factors<const PRIME: u64, const INVERSE: bool>(
    stages: &[(usize, usize)],
    output: &mut [u64],
) -> usize {
    let mut count = 1;
    output[0] = Montgomery::<PRIME>::RADIX;
    let last_radix = stages.last().map_or(1, |&(_, radix)| radix);
    for stage in (1..stages.len()).rev() {
        let radix = stages[stage - 1].1;
        let step = Montgomery::<PRIME>::power(
            KernelConstants::<PRIME, INVERSE>::ROOT,
            Montgomery::<PRIME>::MAX_TRANSFORM_LENGTH / (count * radix * last_radix) as u64,
        );
        for index in count..radix * count {
            output[index] = Montgomery::<PRIME>::multiply(step, output[index - count]);
        }
        count *= radix;
    }
    count
}

/// The cyclic convolution of `left` and `right` modulo PRIME, times
/// `factor`, left in `left`. Both have one length, hold their operands from
/// offset `base_length` on, `left_length` and `right_length` values long,
/// and are both overwritten.
fn convolve<const PRIME: u64>(
    plan: &TransformPlan,
    left: &mut [u64],
    left_length: usize,
    right: &mut [u64],
    right_length: usize,
    mut factor: u64,
) {
    assert!(!left.is_empty() && left.len() == right.len());
    let base_length = plan.base_length;
    let last_radix = plan.last_radix as u64;

    // The factor, the inverse transform's division by the block count
    // (negated, `(PRIME - 1) / block_count`, which the base convolution's
    // negated sums undo) and the conversions into and out of Montgomery
    // form are applied up front, to the shorter operand alone.
    factor = Montgomery::<PRIME>::multiply(
        Montgomery::<PRIME>::power(Montgomery::<PRIME>::RADIX_SQUARED, 3),
        Montgomery::<PRIME>::multiply(factor, (PRIME - 1) / plan.block_count as u64),
    );
    let shorter = if left_length < right_length {
        &mut left[base_length..base_length + left_length]
    } else {
        &mut right[base_length..base_length + right_length]
    };
    for value in shorter {
        *value = Montgomery::<PRIME>::multiply(*value, factor);
    }

    // Room for every stage's twiddle factors, and two more.
    let (mut radix_prefix_product, mut twiddle_count) = (1, 2);
    for &(_, radix) in &plan.stages {
        twiddle_count += radix_prefix_product;
        radix_prefix_product *= radix;
    }

    let mut twiddles = vec![0_u64; twiddle_count];
    let mut last_stage_twiddle_start = 0;
    for stage in 0..plan.stages.len() {
        let stage_twiddle_count = compute_twiddle_factors::<PRIME, false>(
            &plan.stages[0..=stage],
            &mut twiddles[last_stage_twiddle_start..],
        );
        if stage + 1 < plan.stages.len() {
            last_stage_twiddle_start += stage_twiddle_count;
        }
    }

    transform::<PRIME, false>(plan, &mut left[base_length..], &twiddles);
    transform::<PRIME, false>(plan, &mut right[base_length..], &twiddles);

    // The blocks' direct convolutions.
    let (mut position, mut last_stage_twiddle_index, mut position_in_last_radix) =
        (0, last_stage_twiddle_start, 0);
    let mut current_twiddle = Montgomery::<PRIME>::RADIX;
    let twiddle_step = Montgomery::<PRIME>::power(
        KernelConstants::<PRIME, false>::ROOT,
        Montgomery::<PRIME>::MAX_TRANSFORM_LENGTH / last_radix,
    );

    while position < plan.length {
        // Each block's result goes just before it in `left`, where the cache
        // still holds it.
        let (output_block, left_block) =
            left[position..position + 2 * base_length].split_at_mut(base_length);
        let right_block = &right[position + base_length..position + 2 * base_length];
        convolve_base::<PRIME>(output_block, left_block, right_block, current_twiddle);
        position += base_length;
        position_in_last_radix += 1;
        if position_in_last_radix == last_radix {
            last_stage_twiddle_index += 1;
            position_in_last_radix = 0;
            current_twiddle = twiddles[last_stage_twiddle_index];
        } else {
            current_twiddle = Montgomery::<PRIME>::multiply(current_twiddle, twiddle_step);
        }
    }

    let mut twiddle_offset = 0;
    for stage in (0..plan.stages.len()).rev() {
        twiddle_offset += compute_twiddle_factors::<PRIME, true>(
            &plan.stages[0..=stage],
            &mut twiddles[twiddle_offset..],
        );
    }
    transform::<PRIME, true>(plan, left, &twiddles);
}

// The three primes: each below 2^64, with `PRIME - 1` rich enough in
// factors of two, three and five to allow transforms of about 10^18 values.

/// Transforms up to `2^22 * 3^21 * 5^2 = 1_096_847_532_018_892_800` values.
const FIRST_PRIME: u64 = 14_259_017_916_245_606_401;
/// Transforms up to `2^19 * 3^17 * 5^6 = 1_057_916_215_296_000_000` values.
const SECOND_PRIME: u64 = 17_984_575_660_032_000_001;
/// Transforms up to `2^17 * 3^22 * 5^4 = 2_570_736_403_169_280_000` values.
const THIRD_PRIME: u64 = 17_995_154_822_184_960_001;

/// `FIRST_PRIME^-1 mod SECOND_PRIME`, in Montgomery form.
const FIRST_PRIME_INVERSE_MOD_SECOND: u64 = Montgomery::<SECOND_PRIME>::multiply(
    Montgomery::<SECOND_PRIME>::RADIX_SQUARED,
    inverse_modulo(FIRST_PRIME, SECOND_PRIME),
);

/// `(FIRST_PRIME * SECOND_PRIME)^-1 mod THIRD_PRIME`, in Montgomery form.
const FIRST_TIMES_SECOND_INVERSE_MOD_THIRD: u64 = Montgomery::<THIRD_PRIME>::multiply(
    Montgomery::<THIRD_PRIME>::RADIX_SQUARED,
    inverse_modulo(
        (FIRST_PRIME as u128 * SECOND_PRIME as u128 % THIRD_PRIME as u128) as u64,
        THIRD_PRIME,
    ),
);

/// `FIRST_PRIME mod THIRD_PRIME`, in Montgomery form.
const FIRST_PRIME_MOD_THIRD: u64 =
    Montgomery::<THIRD_PRIME>::multiply(Montgomery::<THIRD_PRIME>::RADIX_SQUARED, FIRST_PRIME);

/// `FIRST_PRIME * SECOND_PRIME`, as two digits.
const FIRST_TIMES_SECOND_LOW: u64 = (FIRST_PRIME as u128 * SECOND_PRIME as u128) as u64;
const FIRST_TIMES_SECOND_HIGH: u64 = ((FIRST_PRIME as u128 * SECOND_PRIME as u128) >> 64) as u64;

/// Adds `carry` into `accumulator` from its lowest digit up, answering the
/// carry out of its top digit.
fn propagate_carry(accumulator: &mut [u64], mut carry: u64) -> u64 {
    for digit in accumulator {
        let (sum, overflow) = digit.overflowing_add(carry);
        (*digit, carry) = (sum, u64::from(overflow));
        if !overflow {
            break;
        }
    }
    carry
}

/// Splits `source` into values of `bits` bits each, least significant first,
/// and writes each to both destinations.
fn pack(source: &[u64], first_destination: &mut [u64], second_destination: &mut [u64], bits: u64) {
    // `pending` holds the `filled` low bits of the value being assembled;
    // `consumed` counts the bits of the current digit already taken.
    let mut filled = 0_u64;
    let mut destinations = zip(first_destination, second_destination);
    let mut pending = 0_u64;
    let mask = (1_u64 << bits) - 1;
    for digit in source {
        let mut consumed = 0;
        while consumed < 64 {
            pending |= (digit >> consumed) << filled;
            let available = 64 - consumed;
            if filled + available >= bits {
                let packed = pending & mask;
                let (first, second) = destinations
                    .next()
                    .expect("the destinations hold every packed value");
                *first = packed;
                *second = packed;
                pending = 0;
                consumed = consumed + bits - filled;
                filled = 0;
            } else {
                filled += available;
                break;
            }
        }
    }
    if filled > 0 {
        let packed = pending & mask;
        let (first, second) = destinations
            .next()
            .expect("the destinations hold every packed value");
        *first = packed;
        *second = packed;
    }
}

/// `accumulator += left * right` through two primes, `SECOND_PRIME` and
/// `THIRD_PRIME`, with the operands repacked into values of `bits` bits:
/// few enough that no term of the convolution reaches the primes' product
/// (`bits_per_digit_for`).
pub(super) fn multiply_accumulate_two_primes(
    accumulator: &mut [u64],
    left: &[u64],
    right: &[u64],
    bits: u64,
) {
    assert!(bits < 63);
    let left_length = (64 * left.len() as u64).div_ceil(bits) as usize;
    let right_length = (64 * right.len() as u64).div_ceil(bits) as usize;
    let product_length = left_length + right_length;
    let second_plan = TransformPlan::build::<SECOND_PRIME>(product_length);
    let third_plan = TransformPlan::build::<THIRD_PRIME>(product_length);

    let mut left_second = vec![0_u64; second_plan.base_length + second_plan.length];
    let mut left_third = vec![0_u64; third_plan.base_length + third_plan.length];
    let mut right_second = vec![0_u64; second_plan.base_length + second_plan.length];
    let mut right_third = vec![0_u64; third_plan.base_length + third_plan.length];

    pack(
        left,
        &mut left_second[second_plan.base_length..],
        &mut left_third[third_plan.base_length..],
        bits,
    );
    pack(
        right,
        &mut right_second[second_plan.base_length..],
        &mut right_third[third_plan.base_length..],
        bits,
    );

    // The factors prepare the Chinese remainder step below: the residue
    // modulo SECOND_PRIME comes out times THIRD_PRIME^-1, the one modulo
    // THIRD_PRIME times -SECOND_PRIME^-1.
    convolve::<SECOND_PRIME>(
        &second_plan,
        &mut left_second,
        left_length,
        &mut right_second,
        right_length,
        inverse_modulo(THIRD_PRIME, SECOND_PRIME),
    );
    convolve::<THIRD_PRIME>(
        &third_plan,
        &mut left_third,
        left_length,
        &mut right_third,
        right_length,
        Montgomery::<THIRD_PRIME>::subtract(0, inverse_modulo(SECOND_PRIME, THIRD_PRIME)),
    );

    // Combines the two residues of each term and adds the terms into the
    // accumulator `bits` apart, carrying as it goes.
    let mask = (1_u64 << bits) - 1;
    let mut carry: u128 = 0;
    let (mut output_index, mut buffered_bits) = (0_usize, 0_u64);
    let mut bit_buffer: u64 = 0;
    let mut addition_carry: u64 = 0;
    for index in 0..product_length {
        let (residue_second, residue_third) = (left_second[index], left_third[index]);
        let (mut value, overflow) = (residue_second as u128 * THIRD_PRIME as u128 + carry)
            .overflowing_sub(residue_third as u128 * SECOND_PRIME as u128);
        if overflow {
            value = value.wrapping_add(SECOND_PRIME as u128 * THIRD_PRIME as u128);
        }
        carry = value >> bits;

        let packed = (value as u64) & mask;
        bit_buffer |= packed << buffered_bits;
        buffered_bits += bits;
        if buffered_bits >= 64 {
            let (sum, first_overflow) = bit_buffer.overflowing_add(addition_carry);
            let (sum, second_overflow) = accumulator[output_index].overflowing_add(sum);
            accumulator[output_index] = sum;
            addition_carry = u64::from(first_overflow || second_overflow);

            (output_index, buffered_bits) = (output_index + 1, buffered_bits - 64);
            bit_buffer = packed >> (bits - buffered_bits);
        }
    }

    // `addition_carry + bit_buffer` cannot overflow: the buffer is not full
    // and the carry is 0 or 1.
    propagate_carry(
        &mut accumulator[output_index..],
        addition_carry + bit_buffer,
    );
}

/// Reduces each digit of `source` modulo `PRIME` into `destination`; one
/// subtraction is enough, since every prime lies above 2^63.
fn reduce_digits<const PRIME: u64>(source: &[u64], destination: &mut [u64]) {
    for (reduced, &digit) in zip(destination, source) {
        *reduced = if digit >= PRIME { digit - PRIME } else { digit };
    }
}

/// `accumulator += left * right` through all three primes, on whole digits.
pub(super) fn multiply_accumulate_three_primes(
    accumulator: &mut [u64],
    left: &[u64],
    right: &[u64],
) {
    let product_length = left.len() + right.len();
    let first_plan = TransformPlan::build::<FIRST_PRIME>(product_length);
    let second_plan = TransformPlan::build::<SECOND_PRIME>(product_length);
    let third_plan = TransformPlan::build::<THIRD_PRIME>(product_length);
    let mut left_first = vec![0_u64; first_plan.base_length + first_plan.length];
    let mut left_second = vec![0_u64; second_plan.base_length + second_plan.length];
    let mut left_third = vec![0_u64; third_plan.base_length + third_plan.length];
    let scratch_length = max(left_first.len(), max(left_second.len(), left_third.len()));
    let mut right_scratch = vec![0_u64; scratch_length];

    reduce_digits::<FIRST_PRIME>(left, &mut left_first[first_plan.base_length..]);
    reduce_digits::<FIRST_PRIME>(right, &mut right_scratch[first_plan.base_length..]);
    convolve::<FIRST_PRIME>(
        &first_plan,
        &mut left_first,
        left.len(),
        &mut right_scratch[..first_plan.base_length + first_plan.length],
        right.len(),
        1,
    );

    // The scratch still holds the last convolution's values past `right`,
    // which are cleared; nothing below `base_length` is read.
    reduce_digits::<SECOND_PRIME>(left, &mut left_second[second_plan.base_length..]);
    reduce_digits::<SECOND_PRIME>(right, &mut right_scratch[second_plan.base_length..]);
    right_scratch[second_plan.base_length..][right.len()..second_plan.length].fill(0);
    convolve::<SECOND_PRIME>(
        &second_plan,
        &mut left_second,
        left.len(),
        &mut right_scratch[..second_plan.base_length + second_plan.length],
        right.len(),
        1,
    );

    reduce_digits::<THIRD_PRIME>(left, &mut left_third[third_plan.base_length..]);
    reduce_digits::<THIRD_PRIME>(right, &mut right_scratch[third_plan.base_length..]);
    right_scratch[third_plan.base_length..][right.len()..third_plan.length].fill(0);
    convolve::<THIRD_PRIME>(
        &third_plan,
        &mut left_third,
        left.len(),
        &mut right_scratch[..third_plan.base_length + third_plan.length],
        right.len(),
        1,
    );

    // Combines the three residues of each term and adds the terms into the
    // accumulator a digit apart, carrying as it goes.
    let mut carry: u128 = 0;
    for index in 0..product_length {
        let (residue_first, residue_second, residue_third) =
            (left_first[index], left_second[index], left_third[index]);
        // The term `value` solves
        //     value = residue_first  mod FIRST_PRIME,
        //     value = residue_second mod SECOND_PRIME,
        //     value = residue_third  mod THIRD_PRIME.
        // The first two amount to
        //     value = residue_first + FIRST_PRIME * (inverse * (residue_second
        //             - residue_first) mod SECOND_PRIME)
        //         mod FIRST_PRIME * SECOND_PRIME,
        // where `FIRST_PRIME * inverse = 1 mod SECOND_PRIME`.
        let difference = Montgomery::<SECOND_PRIME>::subtract(residue_second, residue_first);
        let first_step =
            Montgomery::<SECOND_PRIME>::multiply(difference, FIRST_PRIME_INVERSE_MOD_SECOND);
        let partial = residue_first as u128 + FIRST_PRIME as u128 * first_step as u128;
        let partial_mod_third = Montgomery::<THIRD_PRIME>::add(
            residue_first,
            Montgomery::<THIRD_PRIME>::multiply(FIRST_PRIME_MOD_THIRD, first_step),
        );

        // That leaves
        //     value = partial       mod FIRST_PRIME * SECOND_PRIME,
        //     value = residue_third mod THIRD_PRIME,
        // whose solution is
        //     value = partial + FIRST_PRIME * SECOND_PRIME * (inverse *
        //             (residue_third - partial) mod THIRD_PRIME),
        // where `FIRST_PRIME * SECOND_PRIME * inverse = 1 mod THIRD_PRIME`.
        let third_difference =
            Montgomery::<THIRD_PRIME>::subtract(residue_third, partial_mod_third);
        let second_step = Montgomery::<THIRD_PRIME>::multiply(
            third_difference,
            FIRST_TIMES_SECOND_INVERSE_MOD_THIRD,
        );
        let (low_sum, overflow) =
            carry.overflowing_add(partial + FIRST_TIMES_SECOND_LOW as u128 * second_step as u128);
        let low_digit = low_sum as u64;
        let high_part = FIRST_TIMES_SECOND_HIGH as u128 * second_step as u128
            + (low_sum >> 64)
            + if overflow { 1_u128 << 64 } else { 0 };

        let (sum, overflow) = accumulator[index].overflowing_add(low_digit);
        accumulator[index] = sum;
        carry = high_part + u128::from(overflow);
    }
    propagate_carry(&mut accumulator[product_length..], carry as u64);
}

/// The most bits per packed value that keep every term of a convolution of
/// operands `length` digits long below `SECOND_PRIME * THIRD_PRIME`, so that
/// two primes recover it: the largest `bits` up to 62 with
/// `(2^bits - 1)^2 * packed_length < SECOND_PRIME * THIRD_PRIME`. It answers
/// 42 when nothing from 43 on qualifies, since below 43 bits the three-prime
/// path takes over and the width is not used.
pub(super) const fn bits_per_digit_for(length: u64) -> u64 {
    let total_bits = length * 64;
    let (mut low, mut high) = (42_u64, 62);
    while low < high {
        let middle = (low + high).div_ceil(2);
        let packed_maximum = (1_u64 << middle) - 1;
        let packed_length = total_bits.div_ceil(middle);
        let (bound, overflow) = (packed_maximum as u128)
            .pow(2)
            .overflowing_mul(packed_length as u128);
        if !overflow && bound < SECOND_PRIME as u128 * THIRD_PRIME as u128 {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

/// `accumulator += left * right` by the number-theoretic transform. The
/// accumulator must be long enough to hold the sum without a carry out, as
/// for every multiplication here.
///
/// Two primes take two transforms and three take three, but two carry fewer
/// bits per value: the longer the operands, the larger a convolution's terms
/// grow, and the fewer bits a value may hold before a term passes the two
/// primes' product. Three primes carry whole digits, 64 bits in three
/// transforms or 21.3 per transform; two primes do better while they can
/// pack at least 43 bits, which holds until operands of about 2^42 digits.
pub(super) fn multiply_accumulate_by_transform(
    accumulator: &mut [u64],
    left: &[u64],
    right: &[u64],
) {
    let bits = bits_per_digit_for(max(left.len(), right.len()) as u64);
    if bits >= 43 {
        multiply_accumulate_two_primes(accumulator, left, right, bits);
    } else {
        multiply_accumulate_three_primes(accumulator, left, right);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bits_per_digit_for, multiply_accumulate_by_transform, multiply_accumulate_three_primes,
        multiply_accumulate_two_primes, TransformPlan, FIRST_PRIME, SECOND_PRIME, THIRD_PRIME,
    };
    use crate::arbitrary_integer::addition::add_magnitudes;
    use crate::arbitrary_integer::multiplication::multiply_schoolbook;
    use std::collections::BTreeSet;

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

    /// The two-prime path at the bit width the dispatch would choose.
    fn two_primes_at_best_width(accumulator: &mut [u64], left: &[u64], right: &[u64]) {
        let bits = bits_per_digit_for(left.len().max(right.len()) as u64);
        multiply_accumulate_two_primes(accumulator, left, right, bits);
    }

    /// Checks `accumulator + left * right`, the contract every `mac3` has.
    fn check(multiply: fn(&mut [u64], &[u64], &[u64]), left: &[u64], right: &[u64], seed: u64) {
        let start = digits(left.len() / 2, seed);
        check_from(multiply, &start, left, right);
    }

    /// Checks `start + left * right` in an accumulator of the shortest
    /// length the contract allows, `left.len() + right.len() + 1` digits.
    fn check_from(
        multiply: fn(&mut [u64], &[u64], &[u64]),
        start: &[u64],
        left: &[u64],
        right: &[u64],
    ) {
        let mut accumulator = start.to_vec();
        accumulator.resize(left.len() + right.len() + 1, 0);
        multiply(&mut accumulator, left, right);
        let expected = trimmed(add_magnitudes(start, &multiply_schoolbook(left, right)));
        assert_eq!(
            trimmed(accumulator),
            expected,
            "{} x {} digits",
            left.len(),
            right.len()
        );
    }

    #[test]
    fn both_prime_paths_agree_with_schoolbook() {
        for (left_count, right_count) in [
            (1, 1),
            (2, 3),
            (513, 513),
            (700, 700),
            (600, 1400),
            (1500, 1500),
        ] {
            let left = digits(left_count, 3);
            let right = digits(right_count, 4);
            check(two_primes_at_best_width, &left, &right, 5);
            check(multiply_accumulate_three_primes, &left, &right, 6);
            check(multiply_accumulate_by_transform, &left, &right, 7);
        }
    }

    #[test]
    fn all_ones_digits_carry_through_both_paths() {
        for count in [1, 513, 1200] {
            let ones = vec![u64::MAX; count];
            check(two_primes_at_best_width, &ones, &ones, 8);
            check(multiply_accumulate_three_primes, &ones, &ones, 9);
        }
    }

    /// An accumulator of all ones below its top digit: every digit of the
    /// product's sum carries, up to the top digit the contract reserves.
    /// At 16 by 16 and 513 by 513 digits the two-prime path's last packed
    /// word lands on the digit below that top one, so only the carry
    /// propagated after its loop reaches the top.
    #[test]
    fn a_full_accumulator_carries_to_its_top_digit() {
        for (left_count, right_count) in [(1, 1), (16, 16), (513, 513), (513, 700)] {
            let left = digits(left_count, 12);
            let right = digits(right_count, 13);
            let start = vec![u64::MAX; left_count + right_count];
            check_from(two_primes_at_best_width, &start, &left, &right);
            check_from(multiply_accumulate_three_primes, &start, &left, &right);
            check_from(multiply_accumulate_by_transform, &start, &left, &right);
        }
    }

    /// Every length of product up to 400 digits, split evenly, through both
    /// paths. The plans those lengths choose use every radix, 2 to 6, and
    /// every base length a plan can reach, which the test checks, so that
    /// every kernel and every direct convolution runs. A base length of 3
    /// is reached by no length: it needs a plan of exactly 3 values, and for
    /// a minimum of 2 or 3 values the plans of 2 and 4 come first and cost
    /// less.
    #[test]
    fn every_plan_shape_agrees_with_schoolbook() {
        let mut radices = BTreeSet::new();
        let mut base_lengths = BTreeSet::new();
        for product_length in 2..=400 {
            for plan in [
                TransformPlan::build::<FIRST_PRIME>(product_length),
                TransformPlan::build::<SECOND_PRIME>(product_length),
                TransformPlan::build::<THIRD_PRIME>(product_length),
            ] {
                radices.extend(plan.stages.iter().map(|&(_, radix)| radix));
                base_lengths.insert(plan.base_length);
            }

            let left = digits(product_length / 2, product_length as u64);
            let right = digits(product_length - product_length / 2, 20);
            check(two_primes_at_best_width, &left, &right, 21);
            check(multiply_accumulate_three_primes, &left, &right, 22);
        }
        assert_eq!(radices, BTreeSet::from([2, 3, 4, 5, 6]));
        assert_eq!(base_lengths, BTreeSet::from([2, 4, 5, 6, 8, 9]));
    }

    /// Below 43 bits the two-prime path packs fewer effective bits per word
    /// than the three-prime one, which is why the dispatch switches there;
    /// until near 2^42 digits the widest safe packing stays above it.
    #[test]
    fn the_packing_width_shrinks_with_the_length() {
        // Computed with Python 3.14 by a linear scan rather than the binary
        // search under test, with exact integers so that no overflow stands
        // in for a failed bound:
        //     max(bits for bits in range(43, 63)
        //         if (2**bits - 1)**2 * -(-length * 64 // bits)
        //            < SECOND_PRIME * THIRD_PRIME)
        // for length = 1, 2**10, 2**20 and 2**30.
        assert_eq!(bits_per_digit_for(1), 62);
        assert_eq!(bits_per_digit_for(1 << 10), 58);
        assert_eq!(bits_per_digit_for(1 << 20), 53);
        assert_eq!(bits_per_digit_for(1 << 30), 48);
    }
}
