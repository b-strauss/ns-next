//! The errors a deferred literal raises when the VM executes it, and the
//! integer-literal parser both the compiler and the VM use.

use crate::types::*;
use crate::Symbol;

use super::Signal;

/// The program-level error a bare identifier raises when it has no binding
/// and names no builtin class -- reachable only for a bare `super`, which
/// `resolver::resolve_identifier` deliberately leaves unbound and which the
/// lexer deliberately does not reserve, and for a `super`-prefixed
/// superclass-clause receiver that names no builtin class (the known gap in
/// `docs/design/dispatch.md`, "Binding variants").
///
/// Raised by `vm::make_value` for `bytecode::Literal::UnresolvedIdentifier`.
pub(crate) fn unresolved_identifier_error(name: &str, receiver: Value) -> Signal {
    Signal::Error(NewspeakError {
        selector: Symbol::intern(name),
        receiver,
        message: format!("internal: unresolved identifier `{name}`"),
    })
}

/// The error a lexer-accepted integer literal that `parse_integer_literal`
/// rejects raises when it is *evaluated*. Raised by `vm::make_value` for
/// `bytecode::Literal::MalformedInteger`.
pub(crate) fn malformed_integer_error(message: String) -> Signal {
    Signal::Error(NewspeakError {
        selector: Symbol::PARSE,
        receiver: Value::Nil,
        message,
    })
}

/// Evaluates an `ast::Literal::Integer`'s source text, which the lexer hands
/// over verbatim.
///
/// Two shapes reach here (`lexer.rs`'s `lex_number`, and `parser/expr.rs`'s
/// `parse_literal`, which classifies anything containing `r` as an `Integer`):
/// a plain decimal literal (`42`, `-42`), and the Smalltalk/Newspeak radix
/// literal `<radix>r<digits>` (`16rFF`, `2r1010`, `16r-1F`, `-16rFF`) --
/// the spec's Source Format section accepts both. `text.parse::<i64>()`
/// alone would reject every radix literal with a misleading "out of i64
/// range".
///
/// A gap kept explicit: the lexer also accepts a *fractional*
/// radix literal (`16r1.8`), which the parser still classifies as an `Integer`.
/// That needs radix-aware float construction; it is rejected here with a
/// message that says so rather than silently truncating.
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
    if digits.contains('.') {
        return Err(format!(
            "fractional radix literals are not supported in Milestone 1: `{text}`"
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

#[cfg(test)]
mod tests {
    use super::parse_integer_literal;

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

    #[test]
    fn a_fractional_radix_literal_says_it_is_unsupported() {
        let error = parse_integer_literal("16r1.8").expect_err("fractional radix");
        assert!(
            error.contains("fractional radix literals are not supported"),
            "got: {error}"
        );
    }
}
