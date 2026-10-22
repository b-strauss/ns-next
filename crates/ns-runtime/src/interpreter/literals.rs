//! The error a deferred literal raises when the VM executes it, and the
//! number-literal parsers the compiler uses.

use crate::types::*;
use crate::{ArbitraryInteger, Symbol};

use super::Signal;

/// The program-level error a bare identifier raises when it has no binding.
/// Only a module root's clause receivers are unbound, and the root rule
/// decides them and they never run; a bare `super`, the other unbound name
/// once, is a syntax error (§5.10, `docs/design/frontend.md`).
///
/// Raised by `vm::make_value` for `bytecode::Literal::UnresolvedIdentifier`.
pub(crate) fn unresolved_identifier_error<'gc>(name: &str, receiver: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError::error(
        Symbol::intern(name),
        receiver,
        format!("internal: unresolved identifier `{name}`"),
    ))
}

/// The program-level error a body past the register ceiling raises when it
/// runs: the compiler replaces such a body with one that loads
/// `bytecode::Literal::BodyTooLarge` first (`docs/design/compiler.md`,
/// "Compilation cannot fail"), so only a body that runs fails. Named after
/// the kind of body the activation runs, with a method's own selector and a
/// block's `value`, as `recursion_too_deep_error` names them.
///
/// Raised by `vm::make_value` for `bytecode::Literal::BodyTooLarge`.
pub(crate) fn body_too_large_error<'gc>(
    kind: &FrameKind<'gc>,
    receiver: Value<'gc>,
) -> Signal<'gc> {
    let (selector, body) = match kind {
        FrameKind::Method(method) => (method.selector, "method"),
        FrameKind::Closure(_) => (Symbol::VALUE, "block"),
        FrameKind::Initializer(..) | FrameKind::TopLevel => (Symbol::NEW, "initializer"),
        FrameKind::Factory(_) => (Symbol::NEW, "factory"),
        FrameKind::SuperclassClause(_) => (Symbol::NEW, "superclass clause"),
    };
    Signal::Error(NewspeakError::error(
        selector,
        receiver,
        format!(
            "{body} too large: it needs more than {} registers",
            u16::MAX
        ),
    ))
}

/// An integer literal's value: `Small` whenever it fits a `SmallInteger`'s
/// 61 bits, so an integer keeps its one representation from the literal on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IntegerLiteral {
    Small(i64),
    Large(ArbitraryInteger),
}

/// Evaluates an `ast::Literal::Integer`'s source text, which the lexer hands
/// over verbatim.
///
/// Two shapes reach here (`lexer.rs`'s `lex_number`, and `parser/expression.rs`'s
/// `parse_literal`, which classifies anything containing `r` as an `Integer`):
/// a plain decimal literal (`42`, `-42`), and the Smalltalk/Newspeak radix
/// literal `<radix>r<digits>` (`16rFF`, `2r1010`, `16r-1F`, `-16rFF`) --
/// the spec's Source Format section accepts both. §5.1.1 bounds neither:
/// "integers are not limited in size", so a literal beyond a
/// `SmallInteger`'s 61 bits is a `Large` one, never a failure.
///
/// The parser classifies a number with a fraction or an exponent as a
/// float (`parse_float_literal`) and refuses a radix or digit that denotes
/// nothing, so a parsed program never fails here. The checks stand for text
/// no parser saw, which a test can still hand over; an image cannot, since
/// it carries a literal's digits rather than its text.
///
/// Returns a message on failure (never panics).
pub(crate) fn parse_integer_literal(text: &str) -> Result<IntegerLiteral, String> {
    let (negated, radix, digits) = match text.split_once('r') {
        None => (false, 10, text),
        Some((radix_part, digits)) => {
            // A leading `-` on the radix itself (`-16rFF`) negates the whole
            // value; a leading `-` on the digits (`16r-1F`) is the digits'
            // own sign.
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
            (negated, radix, digits)
        }
    };
    // Nearly every literal fits a `SmallInteger`; the arbitrary parse is the
    // fallback, not the rule.
    let small = i64::from_str_radix(digits, radix).ok().and_then(|value| {
        if negated {
            value.checked_neg()
        } else {
            Some(value)
        }
    });
    if let Some(value) = small.filter(|number| fits_small_integer(*number)) {
        return Ok(IntegerLiteral::Small(value));
    }
    let Some(value) = ArbitraryInteger::parse(digits, radix) else {
        return Err(format!(
            "malformed integer literal `{text}` (radix {radix})"
        ));
    };
    let value = if negated { value.negated() } else { value };
    // A literal in `i64` beyond 61 bits reaches here as well as one beyond
    // `i64`, so the range is tested again rather than assumed.
    Ok(
        match value.to_i64().filter(|number| fits_small_integer(*number)) {
            Some(small) => IntegerLiteral::Small(small),
            None => IntegerLiteral::Large(value),
        },
    )
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
    use super::{parse_float_literal, parse_integer_literal, IntegerLiteral};

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
        assert_eq!(parse_integer_literal("42"), Ok(IntegerLiteral::Small(42)));
        assert_eq!(parse_integer_literal("-42"), Ok(IntegerLiteral::Small(-42)));
        assert_eq!(parse_integer_literal("0"), Ok(IntegerLiteral::Small(0)));
    }

    #[test]
    fn radix_literals_parse_in_every_accepted_shape() {
        assert_eq!(
            parse_integer_literal("16rFF"),
            Ok(IntegerLiteral::Small(255))
        );
        assert_eq!(
            parse_integer_literal("16r1F"),
            Ok(IntegerLiteral::Small(31))
        );
        assert_eq!(
            parse_integer_literal("2r1010"),
            Ok(IntegerLiteral::Small(10))
        );
        assert_eq!(parse_integer_literal("36rZ"), Ok(IntegerLiteral::Small(35)));
        // `-` on the digits, and `-` on the radix, both mean "negative".
        assert_eq!(
            parse_integer_literal("16r-1F"),
            Ok(IntegerLiteral::Small(-31))
        );
        assert_eq!(
            parse_integer_literal("-16rFF"),
            Ok(IntegerLiteral::Small(-255))
        );
    }

    #[test]
    fn a_decimal_literal_beyond_i64_is_large() {
        let Ok(IntegerLiteral::Large(integer)) = parse_integer_literal("99999999999999999999")
        else {
            panic!("a literal beyond i64 should parse to a large integer")
        };
        assert_eq!(integer.to_decimal_string(), "99999999999999999999");
    }

    /// A literal that fits a `SmallInteger` is `Small`, and one step past
    /// either 61-bit edge is `Large`, as are the `i64` edges, which lie
    /// beyond them.
    #[test]
    fn a_literal_at_a_small_integer_edge_is_small_and_one_past_it_is_large() {
        assert_eq!(
            parse_integer_literal("1152921504606846975"),
            Ok(IntegerLiteral::Small(1_152_921_504_606_846_975))
        );
        assert_eq!(
            parse_integer_literal("-1152921504606846976"),
            Ok(IntegerLiteral::Small(-1_152_921_504_606_846_976))
        );
        assert_eq!(
            parse_integer_literal("-16r1000000000000000"),
            Ok(IntegerLiteral::Small(-1_152_921_504_606_846_976))
        );
        for text in [
            "1152921504606846976",
            "-1152921504606846977",
            "16r1000000000000000",
            "9223372036854775807",
            "-9223372036854775808",
            "-16r8000000000000000",
            "9223372036854775808",
            "-9223372036854775809",
        ] {
            assert!(
                matches!(parse_integer_literal(text), Ok(IntegerLiteral::Large(_))),
                "{text}"
            );
        }
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
