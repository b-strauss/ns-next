//! A `Float`'s texts: `printString`'s shortest text, the fixed, exponential
//! and precision texts `asStringFixed:`, `asStringExponential:` and
//! `asStringPrecision:` write, and the parser `Float parse:` reads them
//! back with (`docs/design/core-library.md`, "A Float's text").

/// The decimal digits of a magnitude and where its point goes: the value
/// is `0.d1 d2 d3 … × 10^point`. `digits` has no leading zero but for zero
/// itself.
struct Decimal {
    digits: Vec<u8>,
    point: i32,
}

/// The exponent beyond which `shortest` writes exponent form: the first
/// digit's power of ten must lie in `-6..21` for the decimal form.
const DECIMAL_EXPONENTS: std::ops::Range<i32> = -6..21;

/// The most leading zeros `precision` writes after the point before it
/// turns to exponent form.
const PRECISION_LEADING_ZEROS: i32 = 6;

/// `NaN`, `Infinity` or `-Infinity` for a value that has no digits.
fn named(value: f64) -> Option<String> {
    if value.is_nan() {
        Some("NaN".to_string())
    } else if value.is_infinite() {
        Some(if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string())
    } else {
        None
    }
}

/// The text's sign: a `-` for every negative value, `-0.0` included.
fn signed(value: f64) -> String {
    if value.is_sign_negative() {
        "-".to_string()
    } else {
        String::new()
    }
}

/// Splits Rust's exponent form, `d.ddde±x`, into digits and a point.
fn decimal_of_exponent_form(text: &str) -> Decimal {
    let (mantissa, exponent) = text
        .split_once('e')
        .expect("an exponent format writes an e");
    let exponent: i32 = exponent.parse().expect("an exponent is an integer");
    Decimal {
        digits: mantissa.bytes().filter(|byte| *byte != b'.').collect(),
        point: exponent + 1,
    }
}

/// The shortest digits that read back as `magnitude`, the closest to it
/// among them, and of two equally close the one whose last digit is even.
/// Rust's `{:e}` writes the shortest and closest digits, but breaks an
/// exact tie between two of them upward (`even_at_a_tie`).
fn shortest_decimal(magnitude: f64) -> Decimal {
    even_at_a_tie(
        magnitude,
        decimal_of_exponent_form(&format!("{magnitude:e}")),
    )
}

/// `shortest`, the shortest closest digits of `magnitude`, or, where
/// `magnitude` lies exactly halfway between them and the other digits of
/// the same length on its other side, whichever of the two ends in an
/// even digit, if it reads back as `magnitude` too: as the printers that
/// pick the shortest text of a double round a halfway remainder, so that
/// 900719925474099.25 prints as `900719925474099.2`, not `…099.3`.
fn even_at_a_tie(magnitude: f64, shortest: Decimal) -> Decimal {
    // A tie lies halfway past at most 17 digits, so the value has at most
    // 18 significant digits, and its 26 written digits end in 8 zeros,
    // exactly: most values are known not to tie without their whole
    // expansion.
    let leading = decimal_of_exponent_form(&format!("{magnitude:.25e}"));
    if leading.digits[18..].iter().any(|digit| *digit != b'0') {
        return shortest;
    }
    // The exact expansion: a finite double has at most 767 significant
    // digits, so 800 after the first are the value itself.
    let exact = decimal_of_exponent_form(&format!("{magnitude:.800e}"));
    // The digits `exact` keeps down to `shortest`'s last digit's place.
    let Some(kept) =
        usize::try_from(digit_count(shortest.digits.len()) + exact.point - shortest.point)
            .ok()
            .filter(|kept| (1..exact.digits.len()).contains(kept))
    else {
        return shortest;
    };
    let (lower, rest) = exact.digits.split_at(kept);
    let halfway = rest[0] == b'5' && rest[1..].iter().all(|digit| *digit == b'0');
    let last_is_even = |digits: &[u8]| digits.last().is_some_and(|digit| digit % 2 == 0);
    if !halfway || last_is_even(&shortest.digits) {
        return shortest;
    }
    let mut digits = lower.to_vec();
    let mut point = exact.point;
    if !last_is_even(&digits) {
        // The upper neighbour: the last digit raised, a 9 carried.
        let mut position = digits.len();
        loop {
            if position == 0 {
                digits.insert(0, b'1');
                point += 1;
                break;
            }
            position -= 1;
            if digits[position] == b'9' {
                digits[position] = b'0';
            } else {
                digits[position] += 1;
                break;
            }
        }
    }
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    let text = format!(
        "0.{}e{point}",
        std::str::from_utf8(&digits).expect("ASCII digits")
    );
    if text.parse::<f64>().ok().map(f64::to_bits) == Some(magnitude.to_bits()) {
        Decimal { digits, point }
    } else {
        shortest
    }
}

/// `significant` digits of `magnitude`, rounded half up on its exact binary
/// value. A finite double's exact expansion has at most 767 significant
/// digits, so 800 after the first are the value itself, not a rounding.
fn rounded_decimal(magnitude: f64, significant: usize) -> Decimal {
    let exact = decimal_of_exponent_form(&format!("{magnitude:.800e}"));
    let mut digits = exact.digits[..significant].to_vec();
    let mut point = exact.point;
    if exact.digits[significant] >= b'5' {
        let mut position = digits.len();
        loop {
            if position == 0 {
                // Every digit was 9: 9.99 rounds to 10.0, one more digit
                // before the point, and the last digit drops.
                digits.insert(0, b'1');
                digits.pop();
                point += 1;
                break;
            }
            position -= 1;
            if digits[position] == b'9' {
                digits[position] = b'0';
            } else {
                digits[position] += 1;
                break;
            }
        }
    }
    Decimal { digits, point }
}

/// A count of a decimal's digits as the `i32` its point is measured in.
/// `shortest` writes at most 17 digits and `precision` at most 21, the
/// most its leaf admits.
fn digit_count(count: usize) -> i32 {
    i32::try_from(count).expect("internal: a decimal written in decimal form has at most 21 digits")
}

fn push_digits(text: &mut String, digits: &[u8]) {
    text.push_str(std::str::from_utf8(digits).expect("ASCII digits"));
}

fn push_zeros(text: &mut String, count: i32) {
    for _ in 0..count.max(0) {
        text.push('0');
    }
}

/// The decimal form: `digits` with the point after `point` of them,
/// padded with zeros to `digits_after_point` digits after it, and `.0`
/// when there are none and `trailing_zero` asks for it.
fn decimal_form(
    text: &mut String,
    digits: &[u8],
    point: i32,
    digits_after_point: i32,
    trailing_zero: bool,
) {
    let length = digit_count(digits.len());
    if point <= 0 {
        text.push('0');
        if digits_after_point > 0 {
            text.push('.');
            push_zeros(text, -point);
            push_digits(text, digits);
            push_zeros(text, digits_after_point - (-point + length));
        }
    } else if point >= length {
        push_digits(text, digits);
        push_zeros(text, point - length);
        if digits_after_point > 0 {
            text.push('.');
            push_zeros(text, digits_after_point);
        }
    } else {
        let split = usize::try_from(point)
            .expect("internal: the point lies between 0 and the digits' length here");
        push_digits(text, &digits[..split]);
        text.push('.');
        push_digits(text, &digits[split..]);
        push_zeros(text, digits_after_point - (length - point));
    }
    if digits_after_point <= 0 && trailing_zero {
        text.push_str(".0");
    }
}

/// The exponent form: the first digit, the rest after a point, `e`, and
/// the exponent with its sign, `+` included.
fn exponential_form(text: &mut String, digits: &[u8], exponent: i32) {
    push_digits(text, &digits[..1]);
    if digits.len() > 1 {
        text.push('.');
        push_digits(text, &digits[1..]);
    }
    text.push('e');
    text.push(if exponent < 0 { '-' } else { '+' });
    text.push_str(&exponent.unsigned_abs().to_string());
}

/// `Float>>printString`: the shortest text that reads back as `value`.
pub(crate) fn shortest(value: f64) -> String {
    if let Some(name) = named(value) {
        return name;
    }
    let decimal = shortest_decimal(value.abs());
    let mut text = signed(value);
    let exponent = decimal.point - 1;
    if DECIMAL_EXPONENTS.contains(&exponent) {
        let after_point = digit_count(decimal.digits.len()) - decimal.point;
        decimal_form(&mut text, &decimal.digits, decimal.point, after_point, true);
    } else {
        exponential_form(&mut text, &decimal.digits, exponent);
    }
    text
}

/// `Float>>asStringFixed:`: `fraction_digits` digits after the point, a tie
/// rounded up on the exact binary value (Rust's formatter would round it
/// to even), and the value's own sign even when the digits round to zero.
/// Outside ±1e21, and for a NaN or an infinity, `shortest`'s text.
pub(crate) fn fixed(value: f64, fraction_digits: usize) -> String {
    if !(value > -1e21 && value < 1e21) {
        return shortest(value);
    }
    // Every finite double has an exact decimal expansion of at most 1074
    // fraction digits, so this text is the value itself, not a rounding.
    let exact = format!("{:.1074}", value.abs());
    let (integer_text, fraction_text) = exact
        .split_once('.')
        .expect("a fixed-precision format has a point");
    let mut digits: Vec<u8> = integer_text
        .bytes()
        .chain(fraction_text.bytes().take(fraction_digits))
        .collect();
    if fraction_text.as_bytes()[fraction_digits] >= b'5' {
        let mut position = digits.len();
        loop {
            if position == 0 {
                digits.insert(0, b'1');
                break;
            }
            position -= 1;
            if digits[position] == b'9' {
                digits[position] = b'0';
            } else {
                digits[position] += 1;
                break;
            }
        }
    }
    let integer_length = digits.len() - fraction_digits;
    let mut text = String::with_capacity(digits.len() + 2);
    if value.is_sign_negative() {
        text.push('-');
    }
    text.push_str(std::str::from_utf8(&digits[..integer_length]).expect("ASCII digits"));
    if fraction_digits > 0 {
        text.push('.');
        text.push_str(std::str::from_utf8(&digits[integer_length..]).expect("ASCII digits"));
    }
    text
}

/// `Float>>asStringExponential:`: one digit before the point and
/// `fraction_digits` after it, rounded half up, then the exponent.
/// `fraction_digits` is at most 20, which the leaf guarantees: the exact
/// expansion `rounded_decimal` reads has more digits than that.
pub(crate) fn exponential(value: f64, fraction_digits: usize) -> String {
    if let Some(name) = named(value) {
        return name;
    }
    let decimal = rounded_decimal(value.abs(), fraction_digits + 1);
    let mut text = signed(value);
    exponential_form(&mut text, &decimal.digits, decimal.point - 1);
    text
}

/// `Float>>asStringPrecision:`: `significant_digits` significant digits,
/// rounded half up; in exponent form when the value needs more than
/// `PRECISION_LEADING_ZEROS` zeros after the point or its digits end
/// before the point.
/// `significant_digits` is 1 to 21, which the leaf guarantees.
pub(crate) fn precision(value: f64, significant_digits: usize) -> String {
    if let Some(name) = named(value) {
        return name;
    }
    let decimal = rounded_decimal(value.abs(), significant_digits);
    let mut text = signed(value);
    let wanted = digit_count(significant_digits);
    if 1 - decimal.point > PRECISION_LEADING_ZEROS || decimal.point - wanted > 0 {
        exponential_form(&mut text, &decimal.digits, decimal.point - 1);
    } else {
        let after_point = (wanted - decimal.point).max(0);
        decimal_form(
            &mut text,
            &decimal.digits,
            decimal.point,
            after_point,
            false,
        );
    }
    text
}

/// `Float class>>parse:`: an optional `-` or `+`, then `Infinity`, `NaN`,
/// or digits with an optional point and an optional exponent (`e` or `E`,
/// an optional sign, digits), at least one digit before the exponent.
/// `None` for anything else, a lower-case name and a space included.
pub(crate) fn parse(text: &str) -> Option<f64> {
    let (negative, unsigned) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let magnitude = match unsigned {
        "Infinity" => f64::INFINITY,
        "NaN" => f64::NAN,
        _ if is_decimal_text(unsigned) => unsigned.parse::<f64>().ok()?,
        _ => return None,
    };
    Some(if negative { -magnitude } else { magnitude })
}

/// Whether `text` is digits, an optional point and digits -- one digit at
/// least -- and an optional exponent, and nothing else. Rust's own parser
/// takes more (`inf`, `nan`, `infinity` in any case), so the grammar is
/// checked first.
fn is_decimal_text(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits_from = |start: usize| {
        bytes[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let mut index = digits_from(0);
    let mut mantissa_digits = index;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction = digits_from(index);
        mantissa_digits += fraction;
        index += fraction;
    }
    if mantissa_digits == 0 {
        return false;
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent = digits_from(index);
        if exponent == 0 {
            return false;
        }
        index += exponent;
    }
    index == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::{exponential, fixed, parse, precision, shortest};

    #[test]
    fn shortest_writes_decimal_from_the_sixth_place_to_the_twenty_first_digit() {
        assert_eq!(shortest(3.0), "3.0");
        assert_eq!(shortest(-0.0), "-0.0");
        assert_eq!(shortest(0.1), "0.1");
        assert_eq!(shortest(1e20), "100000000000000000000.0");
        assert_eq!(shortest(1e21), "1e+21");
        assert_eq!(shortest(1e-6), "0.000001");
        assert_eq!(shortest(1e-7), "1e-7");
        assert_eq!(shortest(f64::MAX), "1.7976931348623157e+308");
        assert_eq!(shortest(-f64::MAX), "-1.7976931348623157e+308");
        assert_eq!(shortest(f64::from_bits(1)), "5e-324");
        assert_eq!(shortest(f64::INFINITY), "Infinity");
        assert_eq!(shortest(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(shortest(f64::NAN), "NaN");
    }

    /// Of two shortest texts exactly as far from the value, the one
    /// ending in an even digit: 900719925474099.25 lies halfway between
    /// `…099.2` and `…099.3`, both of which read back as it.
    #[test]
    fn shortest_breaks_a_tie_to_the_even_digit() {
        assert_eq!(shortest(900_719_925_474_099.0 + 0.25), "900719925474099.2");
        assert_eq!(
            shortest(-(900_719_925_474_099.0 + 0.25)),
            "-900719925474099.2"
        );
        assert_eq!(shortest(900_719_925_474_098.0 + 0.25), "900719925474098.2");
        // A tie whose even neighbour lies above.
        assert_eq!(shortest(900_719_925_474_099.0 + 0.75), "900719925474099.8");
        // No tie: the closer text.
        assert_eq!(shortest(900_719_925_474_099.0 + 0.125), "900719925474099.1");
        assert_eq!(shortest(900_719_925_474_099.0 + 0.375), "900719925474099.4");
        assert_eq!(shortest(0.1 * 2f64.powi(53)), "900719925474099.2");
        assert_eq!(shortest(3.0 * f64::from_bits(1)), "1.5e-323");
    }

    #[test]
    fn exponential_and_precision_round_half_up() {
        assert_eq!(exponential(3.0, 2), "3.00e+0");
        assert_eq!(exponential(9.99, 1), "1.0e+1");
        assert_eq!(exponential(0.125, 1), "1.3e-1");
        assert_eq!(precision(0.125, 2), "0.13");
        assert_eq!(precision(99.95, 3), "100");
        assert_eq!(precision(123456.0, 3), "1.23e+5");
        assert_eq!(precision(f64::NEG_INFINITY, 3), "-Infinity");
        assert_eq!(fixed(f64::NAN, 2), "NaN");
        assert_eq!(fixed(1e21, 2), "1e+21");
    }

    #[test]
    fn fixed_rounds_a_tie_up_on_the_exact_binary_value() {
        assert_eq!(fixed(0.125, 2), "0.13");
        assert_eq!(fixed(-0.125, 2), "-0.13");
        assert_eq!(fixed(2.5, 0), "3");
        assert_eq!(fixed(0.5, 0), "1");
        assert_eq!(fixed(1.005, 2), "1.00");
        assert_eq!(fixed(9.995, 2), "9.99");
        assert_eq!(fixed(99.95, 1), "100.0");
        assert_eq!(fixed(123.456, 1), "123.5");
        assert_eq!(fixed(1.0, 3), "1.000");
        assert_eq!(fixed(-0.0, 1), "-0.0");
        assert_eq!(fixed(-0.04, 1), "-0.0");
        assert_eq!(fixed(0.1, 20), "0.10000000000000000555");
        assert_eq!(fixed(1e21, 2), "1e+21");
        assert_eq!(fixed(f64::NAN, 2), "NaN");
        assert_eq!(fixed(f64::NEG_INFINITY, 2), "-Infinity");
    }

    #[test]
    fn every_finite_shortest_text_parses_back_to_its_value() {
        let values = [
            0.0,
            -0.0,
            1.0,
            0.1,
            1.0 / 3.0,
            1e21,
            1e-7,
            123.456e-300,
            f64::MAX,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            -2.5e-3,
        ];
        for value in values {
            let text = shortest(value);
            let parsed = parse(&text).unwrap_or_else(|| panic!("{text} parses"));
            assert_eq!(parsed.to_bits(), value.to_bits(), "{text}");
        }
        assert!(parse("NaN").is_some_and(f64::is_nan));
        assert_eq!(parse("-Infinity"), Some(f64::NEG_INFINITY));
    }

    #[test]
    fn parse_refuses_what_print_string_never_writes() {
        for text in [
            "", "nan", "inf", "infinity", "NAN", "--9", "3 ", " 3", "1e", "e5", ".", "+", "0x10",
        ] {
            assert_eq!(parse(text).map(f64::to_bits), None, "{text:?}");
        }
        assert_eq!(parse("3."), Some(3.0));
        assert_eq!(parse(".5"), Some(0.5));
        assert_eq!(parse("+2E3"), Some(2000.0));
    }
}
