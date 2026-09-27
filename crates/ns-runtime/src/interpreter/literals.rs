//! The errors a deferred literal raises when the VM executes it, and the
//! integer-literal parser both the compiler and the VM use.

use crate::types::*;
use crate::Symbol;

use super::Signal;

/// The program-level error a bare identifier raises when it has no binding.
/// Only a module root's clause receivers are unbound, and the root rule
/// decides them and they never run; a bare `super`, the other unbound name
/// once, is a syntax error (§5.10, `docs/design/frontend.md`).
///
/// Raised by `vm::make_value` for `bytecode::Literal::UnresolvedIdentifier`.
pub(crate) fn unresolved_identifier_error<'gc>(name: &str, receiver: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern(name),
        receiver,
        message: format!("internal: unresolved identifier `{name}`"),
    })
}

/// The error a lexer-accepted integer literal that `parse_integer_literal`
/// rejects raises when it is *evaluated*. Raised by `vm::make_value` for
/// `bytecode::Literal::MalformedInteger`.
pub(crate) fn malformed_integer_error<'gc>(message: String) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::PARSE,
        receiver: Value::Nil,
        message,
    })
}

/// Evaluates an `ast::Literal::Integer`'s source text, which the lexer hands
/// over verbatim.
///
/// Two shapes reach here (`lexer.rs`'s `lex_number`, and `parser/expression.rs`'s
/// `parse_literal`, which classifies anything containing `r` as an `Integer`):
/// a plain decimal literal (`42`, `-42`), and the Smalltalk/Newspeak radix
/// literal `<radix>r<digits>` (`16rFF`, `2r1010`, `16r-1F`, `-16rFF`) --
/// the spec's Source Format section accepts both. `text.parse::<i64>()`
/// alone would reject every radix literal with a misleading "out of i64
/// range".
///
/// The parser classifies a number with a fraction or an exponent as a
/// float (`parse_float_literal`) and refuses a radix or digit that denotes
/// nothing, so for a parsed program only an integer beyond `i64` fails here;
/// the other checks stand for an image's literal text, which no parser saw.
///
/// Returns the `NewspeakError` message on failure (never panics -- this is a
/// Newspeak-program-input path).
pub(crate) fn parse_integer_literal(text: &str) -> Result<i64, String> {
    let Some((radix_part, digits)) = text.split_once('r') else {
        return text
            .parse::<i64>()
            .map_err(|_| format!("integer literal out of i64 range: {text}"));
    };
    // A leading `-` on the radix itself (`-16rFF`) negates the whole value;
    // a leading `-` on the digits (`16r-1F`) is handled by `from_str_radix`.
    let (negated, radix_digits) = match radix_part.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, radix_part),
    };
    let radix: u32 = radix_digits
        .parse()
        .map_err(|_| format!("malformed radix in integer literal `{text}`"))?;
    if !(2..=36).contains(&radix) {
        return Err(format!(
            "radix {radix} out of range in integer literal `{text}`: must be 2..36"
        ));
    }
    let value = i64::from_str_radix(digits, radix).map_err(|_| {
        format!("malformed or out-of-i64-range radix integer literal `{text}` (radix {radix})")
    })?;
    if negated {
        value
            .checked_neg()
            .ok_or_else(|| format!("integer literal out of i64 range: {text}"))
    } else {
        Ok(value)
    }
}

/// A non-integral number literal's value (§5.1.1), as the one `f64` this
/// runtime represents it with (a recorded deviation: the section names an
/// exact `FixedPointNumber`). A decimal one is Rust's own float syntax. A
/// radix one, `16r1.8` or `2r1e3`, is its integer and fraction digits in
/// its radix, times ten to its exponent -- the canonical parser's reading
/// (`NewspeakParsing.ns`, `radixNum`), since the section gives an exponent
/// no base. The parser has checked every digit against the radix, so this
/// never fails on a parsed program; an unparseable text is NaN.
pub(crate) fn parse_float_literal(text: &str) -> f64 {
    let Some((radix_text, rest)) = text.split_once('r') else {
        return text.parse().unwrap_or(f64::NAN);
    };
    let Ok(radix) = radix_text.parse::<u32>() else {
        return f64::NAN;
    };
    let (mantissa, exponent) = match rest.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().unwrap_or(0)),
        None => (rest, 0),
    };
    let (negative, unsigned) = match mantissa.strip_prefix('-') {
        Some(unsigned) => (true, unsigned),
        None => (false, mantissa),
    };
    let (integer_digits, fraction_digits) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let base = f64::from(radix);
    let mut value = 0.0;
    for digit in integer_digits.chars() {
        let Some(digit) = digit.to_digit(radix) else {
            return f64::NAN;
        };
        value = value * base + f64::from(digit);
    }
    let mut scale = 1.0 / base;
    for digit in fraction_digits.chars() {
        let Some(digit) = digit.to_digit(radix) else {
            return f64::NAN;
        };
        value += f64::from(digit) * scale;
        scale /= base;
    }
    let value = value * 10f64.powi(exponent);
    if negative {
        -value
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_float_literal, parse_integer_literal};

    #[test]
    fn a_radix_float_is_its_digits_in_its_radix_times_ten_to_its_exponent() {
        assert_eq!(parse_float_literal("16r1.8"), 1.5);
        assert_eq!(parse_float_literal("2r1e3"), 1000.0);
        assert_eq!(parse_float_literal("16r1Fe2"), 3100.0);
        assert_eq!(parse_float_literal("16r-1.8"), -1.5);
        assert_eq!(parse_float_literal("2r0.01"), 0.25);
        assert_eq!(parse_float_literal("1.5e-3"), 0.0015);
    }

    #[test]
    fn plain_decimal_literals_still_parse() {
        assert_eq!(parse_integer_literal("42"), Ok(42));
        assert_eq!(parse_integer_literal("-42"), Ok(-42));
        assert_eq!(parse_integer_literal("0"), Ok(0));
    }

    #[test]
    fn radix_literals_parse_in_every_accepted_shape() {
        assert_eq!(parse_integer_literal("16rFF"), Ok(255));
        assert_eq!(parse_integer_literal("16r1F"), Ok(31));
        assert_eq!(parse_integer_literal("2r1010"), Ok(10));
        assert_eq!(parse_integer_literal("36rZ"), Ok(35));
        // `-` on the digits, and `-` on the radix, both mean "negative".
        assert_eq!(parse_integer_literal("16r-1F"), Ok(-31));
        assert_eq!(parse_integer_literal("-16rFF"), Ok(-255));
    }

    #[test]
    fn a_decimal_literal_out_of_range_says_so() {
        let error = parse_integer_literal("99999999999999999999").expect_err("out of range");
        assert!(error.contains("out of i64 range"), "got: {error}");
    }

    #[test]
    fn an_out_of_range_radix_is_rejected_honestly() {
        let error = parse_integer_literal("1rZ").expect_err("radix 1");
        assert!(error.contains("must be 2..36"), "got: {error}");
        let error = parse_integer_literal("40rZ").expect_err("radix 40");
        assert!(error.contains("must be 2..36"), "got: {error}");
    }

    #[test]
    fn a_digit_outside_the_radix_is_rejected() {
        let error = parse_integer_literal("2r1012").expect_err("digit 2 is not base-2");
        assert!(error.contains("malformed"), "got: {error}");
    }
}
