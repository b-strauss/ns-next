// Adapted from num-integer (https://github.com/rust-num/num-integer), commit
// 765fd9b3eb9397471d41b9f613e530f6d9d6383f, src/lib.rs.
// Copyright (c) 2014 The Rust Project Developers. MIT licence; see
// THIRD-PARTY-NOTICES.md at the repository root. Changed for ns-next: names
// written out, the generic `div_floor` specialized to `ArbitraryInteger`.

//! Integers of any size, as a sign and a magnitude: the arithmetic behind
//! `LargeInteger`, with no heap and no `Value`. The digit algorithms are
//! adapted from num-bigint, one file each (`addition`, `multiplication`,
//! `number_theoretic_transform`, `division`, `conversion`), and floor
//! division from num-integer, here.

use std::cmp::Ordering;

mod addition;
mod conversion;
mod division;
mod multiplication;
mod number_theoretic_transform;

use addition::{add_magnitudes, compare_magnitudes, subtract_magnitudes};
use division::{divide_by_digit, divide_remainder, shift_left, shift_right_in_place};
use multiplication::multiply_magnitudes;

/// The most digits `to_bounded_text` renders in decimal. 64 digits are
/// 4,096 bits, about 1,233 decimal digits, which the quadratic conversion
/// finishes in microseconds; an integer longer than that is past anything a
/// reader of a listing would read digit by digit anyway.
pub const DECIMAL_RENDERING_LIMIT: usize = 64;

/// How many digits at each end `to_bounded_text` shows of an integer past
/// `DECIMAL_RENDERING_LIMIT`.
const HEXADECIMAL_END_DIGITS: usize = 2;

/// An integer of any size: a sign and a magnitude of 64-bit digits, least
/// significant first. Canonical: no zero top digit, and zero is the empty
/// magnitude with `negative == false`, so derived equality is value equality.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArbitraryInteger {
    negative: bool,
    magnitude: Box<[u64]>,
}

impl ArbitraryInteger {
    pub fn zero() -> ArbitraryInteger {
        ArbitraryInteger {
            negative: false,
            magnitude: Box::new([]),
        }
    }

    pub fn from_i64(value: i64) -> ArbitraryInteger {
        // `unsigned_abs`, not `abs`: `i64::MIN` has no positive `i64`.
        ArbitraryInteger::from_parts(value < 0, vec![value.unsigned_abs()])
    }

    pub fn from_i128(value: i128) -> ArbitraryInteger {
        let magnitude = value.unsigned_abs();
        ArbitraryInteger::from_parts(
            value < 0,
            vec![magnitude as u64, (magnitude >> u64::BITS) as u64],
        )
    }

    /// Trims zero top digits and clears the sign of a zero.
    pub fn from_parts(negative: bool, mut magnitude: Vec<u64>) -> ArbitraryInteger {
        trim_zero_top_digits(&mut magnitude);
        ArbitraryInteger {
            negative: negative && !magnitude.is_empty(),
            magnitude: magnitude.into_boxed_slice(),
        }
    }

    /// The integer of sign `negative` and the magnitude `digits`, least
    /// significant first, as `from_parts` canonicalises it.
    pub fn from_sign_and_digits(negative: bool, digits: &[u64]) -> ArbitraryInteger {
        ArbitraryInteger::from_parts(negative, digits.to_vec())
    }

    /// The integer `value` truncates to, toward zero, exactly; `None` for a
    /// NaN or an infinity. Primordial Soup's `Float>>asInteger`
    /// (`LargeInteger::FromDouble`).
    pub fn from_f64_truncated(value: f64) -> Option<ArbitraryInteger> {
        if !value.is_finite() {
            return None;
        }
        let truncated = value.trunc();
        // Below 2^63 in magnitude, `as` converts a whole `f64` exactly.
        if truncated.abs() < 9_223_372_036_854_775_808.0 {
            return Some(ArbitraryInteger::from_i64(truncated as i64));
        }
        // Beyond, the value is its 53-bit significand times a power of two
        // of at least 2^11, which `shift_left` multiplies out.
        let bits = truncated.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) - 1075;
        let significand = (bits & ((1 << 52) - 1)) | (1 << 52);
        let magnitude =
            ArbitraryInteger::from_parts(false, vec![significand]).shift_left(exponent)?;
        Some(if truncated < 0.0 {
            magnitude.negated()
        } else {
            magnitude
        })
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    pub fn is_zero(&self) -> bool {
        self.magnitude.is_empty()
    }

    pub fn magnitude(&self) -> &[u64] {
        &self.magnitude
    }

    pub fn to_i64(&self) -> Option<i64> {
        match *self.magnitude {
            [] => Some(0),
            // `2^63` fits only as `i64::MIN`, which `checked_sub_unsigned`
            // reaches from zero and `checked_neg` of an `i64` cannot.
            [digit] if self.negative => 0_i64.checked_sub_unsigned(digit),
            [digit] => i64::try_from(digit).ok(),
            _ => None,
        }
    }

    pub fn one() -> ArbitraryInteger {
        ArbitraryInteger::from_u64(1)
    }

    pub fn from_u64(value: u64) -> ArbitraryInteger {
        ArbitraryInteger::from_parts(false, vec![value])
    }

    /// The value as a `u64`; `None` for a negative value or one past it.
    pub fn to_u64(&self) -> Option<u64> {
        match (self.negative, &*self.magnitude) {
            (_, []) => Some(0),
            (false, [digit]) => Some(*digit),
            _ => None,
        }
    }

    pub fn is_one(&self) -> bool {
        !self.negative && *self.magnitude == [1]
    }

    /// The magnitude's bit count: 0 for zero, n for 2^(n-1) up to 2^n - 1.
    pub fn bit_length(&self) -> u64 {
        match self.magnitude.last() {
            None => 0,
            Some(top) => {
                (self.magnitude.len() as u64 - 1) * u64::from(u64::BITS)
                    + u64::from(u64::BITS - top.leading_zeros())
            }
        }
    }

    /// The greatest common divisor, never negative; `0 gcd 0` is 0.
    /// Euclid's algorithm on the magnitudes, a one-digit pair in `u64`.
    pub fn gcd(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        let mut larger = self.magnitude.to_vec();
        let mut smaller = other.magnitude.to_vec();
        if compare_magnitudes(&larger, &smaller) == Ordering::Less {
            std::mem::swap(&mut larger, &mut smaller);
        }
        while !smaller.is_empty() {
            if let ([first], [second]) = (larger.as_slice(), smaller.as_slice()) {
                let (mut first, mut second) = (*first, *second);
                while second != 0 {
                    (first, second) = (second, first % second);
                }
                return ArbitraryInteger::from_u64(first);
            }
            let (_, remainder) = divide_remainder(&larger, &smaller);
            larger = std::mem::replace(&mut smaller, remainder);
        }
        ArbitraryInteger::from_parts(false, larger)
    }

    /// The quotient rounded toward zero and the remainder, which takes the
    /// dividend's sign; `None` for a zero divisor.
    pub fn truncated_divide(
        &self,
        divisor: &ArbitraryInteger,
    ) -> Option<(ArbitraryInteger, ArbitraryInteger)> {
        if divisor.is_zero() {
            return None;
        }
        let (quotient, remainder) = divide_remainder(&self.magnitude, &divisor.magnitude);
        Some((
            ArbitraryInteger::from_parts(self.negative != divisor.negative, quotient),
            ArbitraryInteger::from_parts(self.negative, remainder),
        ))
    }

    /// The value divided by two to the `count`, rounded toward negative
    /// infinity: an arithmetic shift of the two's complement, so a
    /// negative value never shifts past -1.
    pub fn shift_right(&self, count: u64) -> ArbitraryInteger {
        let shifted_magnitude = |magnitude: &[u64]| {
            let digit_shift = usize::try_from(count / u64::from(u64::BITS)).unwrap_or(usize::MAX);
            if digit_shift >= magnitude.len() {
                return Vec::new();
            }
            let mut digits = magnitude[digit_shift..].to_vec();
            shift_right_in_place(&mut digits, (count % u64::from(u64::BITS)) as u32);
            digits
        };
        if !self.negative {
            return ArbitraryInteger::from_parts(false, shifted_magnitude(&self.magnitude));
        }
        // -(((|n| - 1) >> count) + 1), the floor of n / 2^count.
        let less_one = subtract_magnitudes(&self.magnitude, &[1]);
        let shifted = shifted_magnitude(&less_one);
        ArbitraryInteger::from_parts(true, add_magnitudes(&shifted, &[1]))
    }

    /// How many base-256 digits the magnitude has; 0 for zero.
    pub fn base_256_length(&self) -> usize {
        self.bit_length().div_ceil(8) as usize
    }

    /// The magnitude's base-256 digit at zero-based `index`, least
    /// significant first; `None` past the last.
    pub fn base_256_digit(&self, index: usize) -> Option<u8> {
        if index >= self.base_256_length() {
            return None;
        }
        let digit = self.magnitude[index / 8];
        Some((digit >> ((index % 8) * 8)) as u8)
    }

    /// The text in `radix` (2 to 36), upper-case digits after a `-` for a
    /// negative value.
    pub fn to_radix_string(&self, radix: u32) -> String {
        assert!((2..=36).contains(&radix), "a radix from 2 to 36");
        let digits = conversion::to_radix(&self.magnitude, radix);
        if self.negative {
            format!("-{digits}")
        } else {
            digits
        }
    }

    pub fn add(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        ArbitraryInteger::sum(
            self.negative,
            &self.magnitude,
            other.negative,
            &other.magnitude,
        )
    }

    pub fn subtract(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        ArbitraryInteger::sum(
            self.negative,
            &self.magnitude,
            !other.negative,
            &other.magnitude,
        )
    }

    /// The sum of two signed magnitudes: equal signs add the magnitudes,
    /// different ones subtract the smaller from the larger, which lends the
    /// sum its sign.
    fn sum(
        left_negative: bool,
        left: &[u64],
        right_negative: bool,
        right: &[u64],
    ) -> ArbitraryInteger {
        if left_negative == right_negative {
            return ArbitraryInteger::from_parts(left_negative, add_magnitudes(left, right));
        }
        match compare_magnitudes(left, right) {
            Ordering::Greater => {
                ArbitraryInteger::from_parts(left_negative, subtract_magnitudes(left, right))
            }
            Ordering::Less => {
                ArbitraryInteger::from_parts(right_negative, subtract_magnitudes(right, left))
            }
            Ordering::Equal => ArbitraryInteger::zero(),
        }
    }

    pub fn multiply(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        ArbitraryInteger::from_parts(
            self.negative != other.negative,
            multiply_magnitudes(&self.magnitude, &other.magnitude),
        )
    }

    /// Rounded toward negative infinity; `None` for a zero divisor.
    ///
    /// num-integer's `div_floor`: the truncated quotient, one further from
    /// zero when the signs differ and the division leaves a remainder.
    pub fn floor_divide(&self, divisor: &ArbitraryInteger) -> Option<ArbitraryInteger> {
        if divisor.is_zero() {
            return None;
        }
        let (mut quotient, remainder) = divide_remainder(&self.magnitude, &divisor.magnitude);
        let signs_differ = self.negative != divisor.negative;
        if signs_differ && !remainder.is_empty() {
            quotient = add_magnitudes(&quotient, &[1]);
        }
        Some(ArbitraryInteger::from_parts(signs_differ, quotient))
    }

    /// The remainder of `floor_divide`, which takes the divisor's sign;
    /// `None` for a zero divisor. Derived from the quotient rather than
    /// adapted from `num-integer`'s `mod_floor`, so the two can never
    /// disagree.
    pub fn floor_modulo(&self, divisor: &ArbitraryInteger) -> Option<ArbitraryInteger> {
        let quotient = self.floor_divide(divisor)?;
        Some(self.subtract(&quotient.multiply(divisor)))
    }

    /// Bitwise and over two's complement, where a negative number's sign
    /// bits extend without end (Primordial Soup's and Squeak's `bitAnd:`).
    pub fn bit_and(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        self.bitwise(other, |left, right| left & right)
    }

    /// Bitwise inclusive or, over two's complement as `bit_and`.
    pub fn bit_or(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        self.bitwise(other, |left, right| left | right)
    }

    /// Bitwise exclusive or, over two's complement as `bit_and`.
    pub fn bit_xor(&self, other: &ArbitraryInteger) -> ArbitraryInteger {
        self.bitwise(other, |left, right| left ^ right)
    }

    /// The receiver times two to the `count`: the magnitude shifted, the
    /// sign kept, which is exact for a negative receiver too. `None` when
    /// the result's digits cannot be reserved, so a count the program
    /// chose is never a Rust allocation abort.
    pub fn shift_left(&self, count: u64) -> Option<ArbitraryInteger> {
        if self.is_zero() {
            return Some(ArbitraryInteger::zero());
        }
        let digit_shift = usize::try_from(count / u64::from(u64::BITS)).ok()?;
        let bit_shift = (count % u64::from(u64::BITS)) as u32;
        let length = digit_shift
            .checked_add(self.magnitude.len())?
            .checked_add(1)?;
        let mut magnitude: Vec<u64> = Vec::new();
        magnitude.try_reserve_exact(length).ok()?;
        magnitude.resize(digit_shift, 0);
        let mut carry = 0;
        for digit in self.magnitude.iter() {
            if bit_shift == 0 {
                magnitude.push(*digit);
            } else {
                magnitude.push((digit << bit_shift) | carry);
                carry = digit >> (u64::BITS - bit_shift);
            }
        }
        magnitude.push(carry);
        Some(ArbitraryInteger::from_parts(self.negative, magnitude))
    }

    /// One digit beyond the longer magnitude holds the sign, so the top bit
    /// of the result's top digit says whether it is negative: every one of
    /// `&`, `|` and `^` applied to two sign-extended digits answers the
    /// result's own sign extension.
    fn bitwise(
        &self,
        other: &ArbitraryInteger,
        operation: fn(u64, u64) -> u64,
    ) -> ArbitraryInteger {
        let length = self.magnitude.len().max(other.magnitude.len()) + 1;
        let digits: Vec<u64> = self
            .twos_complement_digits(length)
            .iter()
            .zip(other.twos_complement_digits(length))
            .map(|(left_digit, right_digit)| operation(*left_digit, right_digit))
            .collect();
        if digits[length - 1] >> (u64::BITS - 1) == 1 {
            // A negative result's magnitude is its complement plus one.
            let complement = digits.iter().map(|digit| !digit).collect();
            ArbitraryInteger::from_parts(false, complement)
                .add(&ArbitraryInteger::from_i64(1))
                .negated()
        } else {
            ArbitraryInteger::from_parts(false, digits)
        }
    }

    /// `length` digits of the two's-complement representation, least
    /// significant first: a negative `-m` is the complement of `m - 1`.
    fn twos_complement_digits(&self, length: usize) -> Vec<u64> {
        if self.negative {
            let decremented = self.absolute().subtract(&ArbitraryInteger::from_i64(1));
            let mut digits = decremented.magnitude().to_vec();
            digits.resize(length, 0);
            digits.iter().map(|digit| !digit).collect()
        } else {
            let mut digits = self.magnitude.to_vec();
            digits.resize(length, 0);
            digits
        }
    }

    pub fn negated(&self) -> ArbitraryInteger {
        ArbitraryInteger {
            negative: !self.negative && !self.is_zero(),
            magnitude: self.magnitude.clone(),
        }
    }

    pub fn absolute(&self) -> ArbitraryInteger {
        ArbitraryInteger {
            negative: false,
            magnitude: self.magnitude.clone(),
        }
    }

    /// The nearest `f64`, ties to even; beyond `f64::MAX` it is infinite.
    pub fn to_f64(&self) -> f64 {
        // The magnitude is rounded, then the sign applied: rounding to
        // nearest is symmetric, so negating afterwards is exact.
        let magnitude = conversion::to_f64(&self.magnitude);
        if self.negative {
            -magnitude
        } else {
            magnitude
        }
    }

    pub fn to_decimal_string(&self) -> String {
        let digits = conversion::to_decimal(&self.magnitude);
        if self.negative {
            format!("-{digits}")
        } else {
            digits
        }
    }

    /// The value as a tool shows it: the decimal text up to
    /// `DECIMAL_RENDERING_LIMIT` digits, and above it the digit count with
    /// the hexadecimal of the first and last `HEXADECIMAL_END_DIGITS` digits.
    ///
    /// `inspect` and `disasm` render integers an artifact names, and an
    /// artifact is untrusted: decimal conversion is quadratic in the digit
    /// count, so one crafted integer of millions of digits would keep either
    /// tool busy for hours. The elided form costs the same at any length, and
    /// the limit leaves every integer a program plausibly writes in decimal.
    pub fn to_bounded_text(&self) -> String {
        let length = self.magnitude.len();
        if length <= DECIMAL_RENDERING_LIMIT {
            return self.to_decimal_string();
        }
        let sign = if self.negative { "-" } else { "" };
        let hexadecimal = |digits: &[u64]| -> String {
            digits
                .iter()
                .rev()
                .map(|digit| format!("{digit:016x}"))
                .collect()
        };
        // The top digit is non-zero (canonical), so printing it unpadded
        // shows no leading zeros.
        let top = self.magnitude[length - 1];
        let high = hexadecimal(&self.magnitude[length - HEXADECIMAL_END_DIGITS..length - 1]);
        let low = hexadecimal(&self.magnitude[..HEXADECIMAL_END_DIGITS]);
        format!("<{length} words> {sign}0x{top:x}{high}...{low}")
    }

    /// `text` is an optional `-` and digits of `radix` (2..=36, either case).
    pub fn parse(text: &str, radix: u32) -> Option<ArbitraryInteger> {
        // `char::to_digit` panics beyond radix 36, so the range is checked
        // before any digit is.
        if !(2..=36).contains(&radix) {
            return None;
        }
        let (negative, digits) = match text.strip_prefix('-') {
            Some(digits) => (true, digits),
            None => (false, text),
        };
        if digits.is_empty() {
            return None;
        }
        let values = digits
            .chars()
            .map(|character| character.to_digit(radix).map(|value| value as u8))
            .collect::<Option<Vec<u8>>>()?;
        Some(ArbitraryInteger::from_parts(
            negative,
            conversion::from_radix_digits(&values, radix),
        ))
    }

    /// Twice the value; Toom-3's `* 2`.
    fn doubled(&self) -> ArbitraryInteger {
        ArbitraryInteger::from_parts(self.negative, shift_left(&self.magnitude, 1))
    }

    /// Half the value, which must be even; Toom-3's `>> 1`.
    fn halved_exactly(&self) -> ArbitraryInteger {
        debug_assert!(self.magnitude.first().is_none_or(|&low| low & 1 == 0));
        let mut magnitude = self.magnitude.to_vec();
        shift_right_in_place(&mut magnitude, 1);
        ArbitraryInteger::from_parts(self.negative, magnitude)
    }

    /// A third of the value, which must be a multiple of three; Toom-3's
    /// `/ 3`.
    fn divided_exactly_by_three(&self) -> ArbitraryInteger {
        let (quotient, remainder) = divide_by_digit(&self.magnitude, 3);
        debug_assert!(remainder == 0);
        ArbitraryInteger::from_parts(self.negative, quotient)
    }
}

impl Ord for ArbitraryInteger {
    fn cmp(&self, other: &ArbitraryInteger) -> Ordering {
        compare_signed(
            (self.negative, &self.magnitude),
            (other.negative, &other.magnitude),
        )
    }
}

/// The order of two integers each given as a sign and a canonical
/// magnitude, least significant digit first -- `Ord`'s rule, for a caller
/// that holds the digits without an `ArbitraryInteger` around them (a
/// `LargeInteger` the heap lends in place).
pub(crate) fn compare_signed(first: (bool, &[u64]), second: (bool, &[u64])) -> Ordering {
    match (first.0, second.0) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => compare_magnitudes(first.1, second.1),
        // Between two negatives, the larger magnitude is the smaller.
        (true, true) => compare_magnitudes(second.1, first.1),
    }
}

impl PartialOrd for ArbitraryInteger {
    fn partial_cmp(&self, other: &ArbitraryInteger) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Drops zero top digits, so that a magnitude is canonical.
fn trim_zero_top_digits(digits: &mut Vec<u64>) {
    digits.truncate(significant_length(digits));
}

/// The length of `digits` without its zero top digits.
fn significant_length(digits: &[u64]) -> usize {
    digits
        .iter()
        .rposition(|&digit| digit != 0)
        .map_or(0, |index| index + 1)
}

#[cfg(test)]
mod tests {
    use super::ArbitraryInteger;

    #[test]
    fn gcd_is_non_negative_and_euclids() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        assert_eq!(integer(12).gcd(&integer(18)), integer(6));
        assert_eq!(integer(-12).gcd(&integer(18)), integer(6));
        assert_eq!(integer(0).gcd(&integer(-5)), integer(5));
        assert_eq!(integer(0).gcd(&integer(0)), integer(0));
        let power = ArbitraryInteger::one().shift_left(130).expect("small");
        let multiple = power.multiply(&integer(3));
        assert_eq!(power.gcd(&multiple), power);
    }

    #[test]
    fn truncated_division_rounds_toward_zero() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        let divide = |dividend: i128, divisor: i128| {
            integer(dividend)
                .truncated_divide(&integer(divisor))
                .expect("non-zero divisor")
        };
        assert_eq!(divide(7, 2), (integer(3), integer(1)));
        assert_eq!(divide(-7, 2), (integer(-3), integer(-1)));
        assert_eq!(divide(7, -2), (integer(-3), integer(1)));
        assert_eq!(divide(-7, -2), (integer(3), integer(-1)));
        assert!(integer(1).truncated_divide(&integer(0)).is_none());
    }

    #[test]
    fn shift_right_rounds_toward_negative_infinity() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        assert_eq!(integer(-8).shift_right(1), integer(-4));
        assert_eq!(integer(-7).shift_right(1), integer(-4));
        assert_eq!(integer(7).shift_right(1), integer(3));
        assert_eq!(integer(-1).shift_right(1000), integer(-1));
        assert_eq!(integer(1 << 100).shift_right(98), integer(4));
        assert_eq!(integer(-(1 << 100)).shift_right(200), integer(-1));
        assert_eq!(integer(5).shift_right(u64::MAX), integer(0));
    }

    #[test]
    fn digits_bit_length_and_radix_text() {
        let integer = |value: i128| ArbitraryInteger::from_i128(value);
        assert_eq!(integer(0).bit_length(), 0);
        assert_eq!(integer(-255).bit_length(), 8);
        assert_eq!(integer(1 << 64).bit_length(), 65);
        assert_eq!(integer(256).base_256_length(), 2);
        assert_eq!(integer(256).base_256_digit(1), Some(1));
        assert_eq!(integer(256).base_256_digit(2), None);
        assert_eq!(integer(0).base_256_length(), 0);
        assert_eq!(integer(-255).to_radix_string(16), "-FF");
        assert_eq!(integer(0).to_radix_string(2), "0");
        assert_eq!(integer(1 << 64).to_radix_string(16), "10000000000000000");
        assert_eq!(
            integer(-(1 << 100)).to_radix_string(10),
            integer(-(1 << 100)).to_decimal_string()
        );
        assert_eq!(integer(35).to_radix_string(36), "Z");
        assert_eq!(integer(5).to_u64(), Some(5));
        assert_eq!(integer(-5).to_u64(), None);
        assert!(ArbitraryInteger::one().is_one());
    }

    #[test]
    fn a_float_truncates_exactly_at_every_magnitude() {
        let two_to_63 = 2f64.powi(63);
        let values = [
            f64::MAX,
            -f64::MAX,
            2f64.powi(1023),
            two_to_63,
            -two_to_63,
            two_to_63 - 1024.0,
            0.5,
            -0.5,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
        ];
        for value in values {
            let integer = ArbitraryInteger::from_f64_truncated(value)
                .unwrap_or_else(|| panic!("{value} truncates"));
            assert_eq!(integer.to_f64(), value.trunc(), "{value}");
            let expected = if value == -two_to_63 {
                Some(i64::MIN)
            } else if value.abs() >= two_to_63 {
                None
            } else {
                Some(value.trunc() as i64)
            };
            assert_eq!(integer.to_i64(), expected, "{value}");
        }
        assert!(ArbitraryInteger::from_f64_truncated(f64::NAN).is_none());
        assert!(ArbitraryInteger::from_f64_truncated(f64::INFINITY).is_none());
        assert!(ArbitraryInteger::from_f64_truncated(f64::NEG_INFINITY).is_none());
    }

    /// Operands at every boundary the representation has: zero, one digit,
    /// the `i64` edges and the digit edge, plus a deterministic spread.
    fn operands() -> Vec<i64> {
        let mut values = vec![
            0,
            1,
            -1,
            2,
            -2,
            7,
            -7,
            255,
            -256,
            i64::MAX,
            i64::MIN,
            i64::MAX - 1,
            i64::MIN + 1,
            u32::MAX as i64,
            -(u32::MAX as i64),
            1 << 32,
            -(1 << 32),
        ];
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..200 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            values.push(state as i64 >> (state % 63));
        }
        values
    }

    fn wide(value: i128) -> ArbitraryInteger {
        ArbitraryInteger::from_i128(value)
    }

    #[test]
    fn arithmetic_agrees_with_i128() {
        for &left in &operands() {
            for &right in &operands() {
                let (left_wide, right_wide) = (i128::from(left), i128::from(right));
                let (left_integer, right_integer) = (wide(left_wide), wide(right_wide));
                assert_eq!(
                    left_integer.add(&right_integer),
                    wide(left_wide + right_wide)
                );
                assert_eq!(
                    left_integer.subtract(&right_integer),
                    wide(left_wide - right_wide)
                );
                assert_eq!(
                    left_integer.multiply(&right_integer),
                    wide(left_wide * right_wide)
                );
                assert_eq!(left_integer.cmp(&right_integer), left_wide.cmp(&right_wide));
                if right != 0 {
                    assert_eq!(
                        left_integer.floor_divide(&right_integer),
                        Some(wide(
                            left_wide.div_euclid(right_wide)
                                - i128::from(
                                    right_wide < 0 && left_wide.rem_euclid(right_wide) != 0
                                )
                        )),
                    );
                }
            }
        }
    }

    #[test]
    fn zero_has_one_representation() {
        assert_eq!(
            ArbitraryInteger::from_parts(true, vec![0, 0]),
            ArbitraryInteger::zero()
        );
        assert!(!ArbitraryInteger::from_i64(5)
            .subtract(&ArbitraryInteger::from_i64(5))
            .is_negative());
        assert_eq!(ArbitraryInteger::zero().negated(), ArbitraryInteger::zero());
    }

    #[test]
    fn the_i64_edges_convert_both_ways() {
        let minimum = ArbitraryInteger::from_i64(i64::MIN);
        assert_eq!(minimum.to_i64(), Some(i64::MIN));
        assert_eq!(minimum.negated().to_i64(), None);
        assert_eq!(minimum.negated(), wide(1_i128 << 63));
        assert_eq!(minimum.absolute(), wide(1_i128 << 63));
        assert_eq!(wide(1_i128 << 63).negated().to_i64(), Some(i64::MIN));
        assert_eq!(wide(i128::from(i64::MAX) + 1).to_i64(), None);
        assert_eq!(
            minimum.floor_divide(&ArbitraryInteger::from_i64(-1)),
            Some(wide(1_i128 << 63))
        );
        assert_eq!(minimum.floor_divide(&ArbitraryInteger::zero()), None);
    }

    #[test]
    fn floor_modulo_takes_the_divisors_sign_and_agrees_with_i128() {
        let samples: [i128; 12] = [
            0,
            1,
            -1,
            7,
            -7,
            i128::from(i64::MAX),
            i128::from(i64::MIN),
            i128::from(i64::MIN) - 1,
            1 << 64,
            -(1 << 64),
            (1 << 100) + 12345,
            -(1 << 100) - 12345,
        ];
        for dividend in samples {
            for divisor in samples {
                let answer = ArbitraryInteger::from_i128(dividend)
                    .floor_modulo(&ArbitraryInteger::from_i128(divisor));
                if divisor == 0 {
                    assert!(answer.is_none());
                    continue;
                }
                let remainder = dividend % divisor;
                let expected = if remainder != 0 && ((remainder < 0) != (divisor < 0)) {
                    remainder + divisor
                } else {
                    remainder
                };
                assert_eq!(
                    answer,
                    Some(ArbitraryInteger::from_i128(expected)),
                    "{dividend} \\\\ {divisor}"
                );
            }
        }
    }

    #[test]
    fn bit_and_is_twos_complement_and_agrees_with_i128() {
        let samples: [i128; 12] = [
            0,
            1,
            -1,
            65535,
            -65536,
            i128::from(i64::MAX),
            i128::from(i64::MIN),
            (1 << 64) - 1,
            1 << 64,
            -(1 << 64),
            -(1 << 64) - 1,
            -(1 << 100) + 12345,
        ];
        for left in samples {
            for right in samples {
                assert_eq!(
                    ArbitraryInteger::from_i128(left)
                        .bit_and(&ArbitraryInteger::from_i128(right))
                        .to_decimal_string(),
                    (left & right).to_string(),
                    "{left} bitAnd: {right}"
                );
            }
        }
    }

    #[test]
    fn bit_or_and_bit_xor_are_twos_complement_and_agree_with_i128() {
        let samples: [i128; 12] = [
            0,
            1,
            -1,
            65535,
            -65536,
            i128::from(i64::MAX),
            i128::from(i64::MIN),
            (1 << 64) - 1,
            1 << 64,
            -(1 << 64),
            -(1 << 64) - 1,
            -(1 << 100) + 12345,
        ];
        for left in samples {
            for right in samples {
                let left_integer = ArbitraryInteger::from_i128(left);
                let right_integer = ArbitraryInteger::from_i128(right);
                assert_eq!(
                    left_integer.bit_or(&right_integer).to_decimal_string(),
                    (left | right).to_string(),
                    "{left} bitOr: {right}"
                );
                assert_eq!(
                    left_integer.bit_xor(&right_integer).to_decimal_string(),
                    (left ^ right).to_string(),
                    "{left} bitXor: {right}"
                );
            }
        }
    }

    #[test]
    fn shift_left_multiplies_by_a_power_of_two_and_agrees_with_i128() {
        let samples: [i128; 7] = [
            0,
            1,
            -1,
            3,
            -12345,
            i128::from(i64::MAX),
            i128::from(i64::MIN),
        ];
        for value in samples {
            for count in [0_u64, 1, 13, 63, 64] {
                assert_eq!(
                    ArbitraryInteger::from_i128(value)
                        .shift_left(count)
                        .expect("a small shift allocates")
                        .to_decimal_string(),
                    (value << count).to_string(),
                    "{value} << {count}"
                );
            }
        }
    }

    #[test]
    fn shift_left_refuses_a_result_that_cannot_be_allocated() {
        assert!(ArbitraryInteger::from_i64(1).shift_left(u64::MAX).is_none());
        assert!(ArbitraryInteger::zero().shift_left(u64::MAX).is_some());
    }

    #[test]
    fn decimal_text_matches_python() {
        // Values computed with Python 3.14; `str(2**200)` and so on.
        let two_to_the_200 = ArbitraryInteger::parse(
            "1606938044258990275541962092341162602522202993782792835301376",
            10,
        )
        .unwrap();
        let mut power = ArbitraryInteger::from_i64(1);
        for _ in 0..200 {
            power = power.multiply(&ArbitraryInteger::from_i64(2));
        }
        assert_eq!(power, two_to_the_200);
        assert_eq!(
            power.to_decimal_string(),
            "1606938044258990275541962092341162602522202993782792835301376"
        );
        assert_eq!(
            power.negated().to_decimal_string(),
            "-1606938044258990275541962092341162602522202993782792835301376"
        );
        assert_eq!(ArbitraryInteger::zero().to_decimal_string(), "0");
        assert_eq!(
            ArbitraryInteger::parse("FFFFFFFFFFFFFFFFFFFF", 16)
                .unwrap()
                .to_decimal_string(),
            "1208925819614629174706175"
        );
        assert_eq!(
            ArbitraryInteger::parse("-99999999999999999999", 10)
                .unwrap()
                .to_decimal_string(),
            "-99999999999999999999"
        );
        assert_eq!(
            ArbitraryInteger::parse("1", 2),
            Some(ArbitraryInteger::from_i64(1))
        );
        assert_eq!(ArbitraryInteger::parse("", 10), None);
        assert_eq!(ArbitraryInteger::parse("12", 2), None);
        assert_eq!(ArbitraryInteger::parse("10", 37), None);
    }

    #[test]
    fn nearest_double() {
        assert_eq!(wide(1_i128 << 64).to_f64(), 18_446_744_073_709_551_616.0);
        // 2^64 + 1 rounds to 2^64; 2^53 + 1 rounds to even, 2^53.
        assert_eq!(
            wide((1_i128 << 64) + 1).to_f64(),
            18_446_744_073_709_551_616.0
        );
        assert_eq!(wide((1_i128 << 53) + 1).to_f64(), 9_007_199_254_740_992.0);
        assert_eq!(
            wide(-(1_i128 << 70)).to_f64(),
            -1_180_591_620_717_411_303_424.0
        );
        let mut huge = ArbitraryInteger::from_i64(1);
        for _ in 0..1100 {
            huge = huge.multiply(&ArbitraryInteger::from_i64(2));
        }
        assert_eq!(huge.to_f64(), f64::INFINITY);
        assert_eq!(huge.negated().to_f64(), f64::NEG_INFINITY);
    }

    /// `i128 as f64` rounds to nearest, ties to even, so every product of
    /// two operands checks the rounding across one- and two-digit values.
    #[test]
    fn nearest_double_agrees_with_i128() {
        for &left in &operands() {
            for &right in &operands() {
                let product = i128::from(left) * i128::from(right);
                assert_eq!(wide(product).to_f64(), product as f64, "{product}");
            }
        }
    }

    fn long_operand(digit_count: usize, seed: u64, negative: bool) -> ArbitraryInteger {
        let mut state = seed;
        let digits = (0..digit_count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                state
            })
            .collect();
        ArbitraryInteger::from_parts(negative, digits)
    }

    /// The floor quotient `q` of `a` by `d` leaves `a - q * d` with the
    /// divisor's sign (or zero) and a smaller magnitude than the divisor.
    #[test]
    fn floor_division_of_long_operands_rounds_down() {
        for (dividend_digits, divisor_digits) in [(3, 1), (9, 2), (30, 12), (80, 79)] {
            for (dividend_negative, divisor_negative) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                let dividend = long_operand(dividend_digits, 3, dividend_negative);
                let divisor = long_operand(divisor_digits, 4, divisor_negative);
                let quotient = dividend.floor_divide(&divisor).unwrap();
                let remainder = dividend.subtract(&quotient.multiply(&divisor));
                assert!(remainder.is_zero() || remainder.is_negative() == divisor_negative);
                assert!(remainder.absolute() < divisor.absolute());
            }
        }
    }

    #[test]
    fn bounded_text_is_decimal_up_to_the_limit_and_elided_above_it() {
        let at_limit = long_operand(super::DECIMAL_RENDERING_LIMIT, 6, true);
        assert_eq!(at_limit.to_bounded_text(), at_limit.to_decimal_string());
        let digits = (1..=u64::try_from(super::DECIMAL_RENDERING_LIMIT).unwrap() + 1).collect();
        let past_limit = ArbitraryInteger::from_parts(true, digits);
        assert_eq!(
            past_limit.to_bounded_text(),
            "<65 words> -0x410000000000000040...00000000000000020000000000000001"
        );
    }

    #[test]
    fn decimal_text_round_trips() {
        for digit_count in [1, 2, 5, 40] {
            for negative in [false, true] {
                let integer = long_operand(digit_count, 5, negative);
                let text = integer.to_decimal_string();
                assert_eq!(ArbitraryInteger::parse(&text, 10), Some(integer));
            }
        }
    }
}
