//! The lexer's tokens as JSON, for the JetBrains plugin's comparison of its
//! own lexer against this one (`docs/design/editor-support.md`, "Testing").

use ns_syntax::TokenKind;

/// `text`'s tokens with UTF-16 offsets, `EndOfInput` left out, and the
/// number of lexer diagnostics. The caller adds the file name.
pub fn dump(text: &str) -> serde_json::Value {
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
        units += character.len_utf16() as u32;
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
    serde_json::json!({
        "tokens": entries,
        "diagnostics": diagnostics.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_dumped_with_utf16_offsets() {
        let dumped = dump("'ä' foo:");
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
        assert_eq!(dump("'open")["diagnostics"], 1);
    }
}
