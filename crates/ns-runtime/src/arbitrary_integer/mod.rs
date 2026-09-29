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
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => compare_magnitudes(&self.magnitude, &other.magnitude),
            // Between two negatives, the larger magnitude is the smaller.
            (true, true) => compare_magnitudes(&other.magnitude, &self.magnitude),
        }
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
