// Adapted from num-bigint (https://github.com/rust-num/num-bigint), commit
// 41f0b035d12971c5c7f7501917e31f0f6439c26c, src/biguint/convert.rs.
// Copyright (c) 2014 The Rust Project Developers. MIT licence; see
// THIRD-PARTY-NOTICES.md at the repository root. Changed for ns-next: names
// written out, `BigUint` replaced by digit slices, no `unsafe`.

//! Conversions of a magnitude to and from radix text and to `f64`.

use super::addition::add_in_place;
use super::division::divide_by_digit;
use super::multiplication::multiply_with_carry;
use super::trim_zero_top_digits;

/// For every radix from 2 to 36, the largest power of it that fits one digit
/// and its exponent, so that conversions work a digit's worth of radix
/// digits at a time. Index by radix; entries 0 and 1 are unused.
const RADIX_BASES: [(u64, usize); 37] = radix_bases();

const fn radix_bases() -> [(u64, usize); 37] {
    let mut bases = [(0, 0); 37];
    let mut index: usize = 2;
    while index <= 36 {
        let radix = index as u64;
        let mut power = 1;
        let mut base = radix;
        while let Some(next) = base.checked_mul(radix) {
            base = next;
            power += 1;
        }
        bases[index] = (base, power);
        index += 1;
    }
    bases
}

/// The magnitude as decimal digits, most significant first, with no sign;
/// `"0"` for zero.
pub(super) fn to_decimal(magnitude: &[u64]) -> String {
    to_radix(magnitude, 10)
}

/// The lowest digit of `value` in `radix`, 2 to 36, as a number.
fn radix_digit(value: u64, radix: u64) -> u8 {
    u8::try_from(value % radix).expect("internal: a remainder modulo a radix of at most 36")
}

/// The magnitude's digits in `radix` (2 to 36), upper case, most significant
/// first, with no sign; `"0"` for zero.
///
/// Divides by the radix's largest power that fits a digit (num-bigint's small
/// path) and writes out each remainder's digits; its divide-and-conquer path
/// for very long numbers is not taken.
pub(super) fn to_radix(magnitude: &[u64], radix: u32) -> String {
    debug_assert!((2..=36).contains(&radix));
    let (base, power) = RADIX_BASES[radix as usize];
    let radix = u64::from(radix);

    if magnitude.is_empty() {
        return String::from("0");
    }

    // Least significant first, reversed at the end.
    let mut digits: Vec<u8> = Vec::new();
    let mut rest = magnitude.to_vec();
    while rest.len() > 1 {
        let (quotient, mut remainder) = divide_by_digit(&rest, base);
        for _ in 0..power {
            digits.push(radix_digit(remainder, radix));
            remainder /= radix;
        }
        rest = quotient;
    }

    let mut remainder = rest[0];
    while remainder != 0 {
        digits.push(radix_digit(remainder, radix));
        remainder /= radix;
    }

    // A non-empty magnitude has a non-zero top digit, and its last division
    // leaves a non-zero single digit, so no leading zeros need trimming.
    digits
        .iter()
        .rev()
        .map(|&digit| {
            char::from(if digit < 10 {
                b'0' + digit
            } else {
                b'A' + digit - 10
            })
        })
        .collect()
}

/// The magnitude written by `digits`, most significant first, each already a
/// value below `radix`, trimmed. The digits must not be empty.
///
/// num-bigint's `from_radix_digits_be`: the digits are taken a digit's worth
/// at a time, and the magnitude so far is multiplied by the radix's largest
/// power that fits a digit and the chunk added in.
pub(super) fn from_radix_digits(digits: &[u8], radix: u32) -> Vec<u64> {
    debug_assert!(!digits.is_empty());
    debug_assert!((2..=36).contains(&radix));
    debug_assert!(digits.iter().all(|&digit| u32::from(digit) < radix));

    let (base, power) = RADIX_BASES[radix as usize];
    let radix = u64::from(radix);

    // Each radix digit carries at most `log2(radix)` bits.
    let bits_per_digit = u64::BITS - radix.leading_zeros();
    let capacity = digits.len() * bits_per_digit as usize / u64::BITS as usize + 1;
    let mut magnitude = Vec::with_capacity(capacity);

    let chunk_value = |chunk: &[u8]| {
        chunk.iter().fold(0, |accumulator, &digit| {
            accumulator * radix + u64::from(digit)
        })
    };

    // The first chunk takes the digits left over from whole chunks, so
    // every later one is a full `power` digits.
    let leading = match digits.len() % power {
        0 => power,
        partial => partial,
    };
    let (head, tail) = digits.split_at(leading);
    magnitude.push(chunk_value(head));

    for chunk in tail.chunks(power) {
        if magnitude.last() != Some(&0) {
            magnitude.push(0);
        }

        let mut carry = 0;
        for digit in magnitude.iter_mut() {
            *digit = multiply_with_carry(*digit, base, &mut carry);
        }
        debug_assert!(carry == 0);

        add_in_place(&mut magnitude, &[chunk_value(chunk)]);
    }

    trim_zero_top_digits(&mut magnitude);
    magnitude
}

/// The magnitude's number of significant bits.
fn bit_length(magnitude: &[u64]) -> u64 {
    match magnitude.last() {
        Some(&top) => {
            magnitude.len() as u64 * u64::from(u64::BITS) - u64::from(top.leading_zeros())
        }
        None => 0,
    }
}

/// The magnitude's top 64 significant bits, rounded to odd: when any bit
/// below them is set, the lowest bit is set too. A second rounding of the
/// answer to nearest-ties-to-even then rounds exactly as rounding the whole
/// magnitude once would, since the odd bit breaks every false tie.
fn high_bits_to_u64(magnitude: &[u64]) -> u64 {
    match magnitude {
        [] => 0,
        [digit] => *digit,
        _ => {
            let mut bits = bit_length(magnitude);
            let mut high_bits = 0_u64;
            let mut high_bits_length = 0;

            for &digit in magnitude.iter().rev() {
                let digit_bits = (bits - 1) % u64::from(u64::BITS) + 1;
                let bits_wanted = Ord::min(64 - high_bits_length, digit_bits);

                if bits_wanted != 0 {
                    if bits_wanted != 64 {
                        high_bits <<= bits_wanted;
                    }
                    high_bits |= digit >> (digit_bits - bits_wanted);
                }

                if digit_bits - bits_wanted != 0 {
                    let dropped_bits = u32::try_from(digit_bits - bits_wanted)
                        .expect("internal: at most a digit's 64 bits are dropped");
                    let dropped = digit << (64 - dropped_bits);
                    high_bits |= u64::from(dropped != 0);
                }

                high_bits_length += bits_wanted;
                bits -= bits_wanted;
            }

            high_bits
        }
    }
}

/// The `f64` nearest the magnitude, ties to even; infinite beyond `f64::MAX`.
pub(super) fn to_f64(magnitude: &[u64]) -> f64 {
    let mantissa = high_bits_to_u64(magnitude);
    let exponent = bit_length(magnitude) - u64::from(u64::BITS - mantissa.leading_zeros());

    // A non-zero exponent means the mantissa holds a full 64 bits, so the
    // value is at least `2^(exponent + 63)`: from exponent 1024 on, beyond
    // every finite double.
    if exponent >= 1024 {
        return f64::INFINITY;
    }
    // `mantissa as f64` is the one rounding; scaling by a power of two is
    // exact, or overflows to infinity, which is the right answer then.
    let scale = f64::from_bits((1023 + exponent) << 52);
    mantissa as f64 * scale
}
