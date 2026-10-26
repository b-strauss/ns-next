//! The lexer's tokens as JSON, for the JetBrains plugin's comparison of its
//! own lexer against this one (`docs/design/editor-support.md`, "Testing").

use ns_syntax::TokenKind;

/// `text`'s tokens with UTF-16 offsets, `EndOfInput` left out, and the
/// number of lexer diagnostics. The caller adds the file name. A source
/// longer than a span can address is refused with the message of the
/// diagnostic `ns_syntax::lex` would answer, before it is lexed: its
/// offsets would wrap. The lexer is run directly rather than through
/// `lex`, which keeps no tokens of a file with lexer errors, and the
/// plugin's comparison needs them.
pub fn dump(text: &str) -> Result<serde_json::Value, String> {
    check_length(text.len())?;
    let (tokens, diagnostics) = ns_syntax::lexer::Lexer::new(text).run();
    // UTF-16 offsets of every byte offset a token starts or ends at, by one
    // pass over the text rather than one per token.
    let mut utf16_at = vec![0u32; text.len() + 1];
    let mut units = 0u32;
    for (byte_offset, character) in text.char_indices() {
        for element in utf16_at
            .iter_mut()
            .skip(byte_offset)
            .take(character.len_utf8())
        {
            *element = units;
        }
        units += u32::try_from(character.len_utf16())
            .expect("internal: a character is at most two UTF-16 units");
    }
    utf16_at[text.len()] = units;
    let entries: Vec<serde_json::Value> = tokens
        .iter()
        .filter(|token| token.kind != TokenKind::EndOfInput)
        .map(|token| {
            serde_json::json!({
                "kind": format!("{:?}", token.kind),
                "start": utf16_at[token.span.start as usize],
                "end": utf16_at[token.span.end as usize],
            })
        })
        .collect();
    Ok(serde_json::json!({
        "tokens": entries,
        "diagnostics": diagnostics.len(),
    }))
}

/// `dump`'s refusal of a source of `length` bytes, apart so that it can be
/// tested without a source of 4 GiB.
fn check_length(length: usize) -> Result<(), String> {
    match ns_syntax::source_too_long(length) {
        Some(diagnostic) => Err(diagnostic.message),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_dumped_with_utf16_offsets() {
        let dumped = dump("'ä' foo:").expect("a short source is dumped");
        assert_eq!(
            dumped,
            serde_json::json!({
                "tokens": [
                    {"kind": "String", "start": 0, "end": 3},
                    {"kind": "Keyword", "start": 4, "end": 8},
                ],
                "diagnostics": 0,
            })
        );
    }

    #[test]
    fn lexer_diagnostics_are_counted() {
        assert_eq!(
            dump("'open").expect("a short source is dumped")["diagnostics"],
            1
        );
    }

    /// The longest source a span addresses is dumped; one byte more is
    /// refused with `ns_syntax::lex`'s message, before any lexing.
    #[test]
    fn a_source_past_four_gibibytes_is_refused() {
        let longest = u32::MAX as usize;
        assert_eq!(check_length(longest), Ok(()));
        assert_eq!(
            check_length(longest + 1),
            Err(
                "source too long: 4294967296 bytes, more than the 4294967295 a source may have"
                    .to_string()
            )
        );
    }
}
