//! Single-pass lexer for Newspeak3 source.

use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::token::{Token, TokenKind};

pub struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    position: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

const SPECIAL: &[u8] = b"+-*/\\~<>=&|@%,?!";

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Lexer<'a> {
        Lexer {
            source,
            bytes: source.as_bytes(),
            position: 0,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn run(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        while self.position < self.bytes.len() {
            self.skip_trivia();
            if self.position >= self.bytes.len() {
                break;
            }
            self.lex_one();
        }
        self.push(TokenKind::EndOfInput, self.position, self.position);
        (self.tokens, self.diagnostics)
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(byte) if byte.is_ascii_whitespace() => self.position += 1,
                Some(b'(') if self.peek_at(1) == Some(b'*') => self.skip_block_comment(),
                _ => break,
            }
        }
    }

    fn skip_block_comment(&mut self) {
        let start = self.position;
        self.position += 2;
        let mut depth = 1u32;
        while depth > 0 {
            match (self.peek(), self.peek_at(1)) {
                (Some(b'('), Some(b'*')) => {
                    self.position += 2;
                    depth += 1;
                }
                (Some(b'*'), Some(b')')) => {
                    self.position += 2;
                    depth -= 1;
                }
                (Some(_), _) => self.position += 1,
                (None, _) => {
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start as u32, self.position as u32),
                        "unterminated comment",
                    ));
                    return;
                }
            }
        }
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start as u32, end as u32),
        });
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.position + offset).copied()
    }

    fn lex_one(&mut self) {
        let start = self.position;
        let byte = self.bytes[self.position];
        match byte {
            b'(' => self.single(TokenKind::LeftParenthesis),
            b')' => self.single(TokenKind::RightParenthesis),
            b'[' => self.single(TokenKind::LeftBracket),
            b']' => self.single(TokenKind::RightBracket),
            b'{' => self.single(TokenKind::LeftBrace),
            b'}' => self.single(TokenKind::RightBrace),
            b'^' => self.single(TokenKind::Caret),
            b'.' => self.single(TokenKind::Dot),
            b';' => self.single(TokenKind::Semicolon),
            b'#' if self.peek_at(1) == Some(b'(') => self.single(TokenKind::Pound),
            b'#' => self.lex_symbol(),
            b'|' => {
                if self.peek_at(1) == Some(b'|') {
                    self.position += 2;
                    self.push(TokenKind::DoubleBar, start, self.position);
                } else if self
                    .peek_at(1)
                    .is_some_and(|next_byte| SPECIAL.contains(&next_byte))
                {
                    // `|` starting a multi-character run (`|>`, `|=`, ...) is a
                    // binary selector, not a slot/temporary delimiter.
                    self.lex_operator();
                } else {
                    self.single(TokenKind::Bar);
                }
            }
            b':' => {
                if self.peek_at(1) == Some(b':') && self.peek_at(2) == Some(b'=') {
                    self.position += 3;
                    self.push(TokenKind::ColonColonEquals, start, self.position);
                } else if self.peek_at(1) == Some(b'=') {
                    // Smalltalk's assignment. §5.4.3 gives Newspeak one
                    // assignment form, the setter send, and nothing in the
                    // specification's grammar, the canonical grammar, Primordial
                    // Soup or SOMns accepts `:=`. Named here, because split into
                    // `:` and `=` it would surface as an unrelated parse error
                    // further on.
                    self.position += 2;
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start as u32, self.position as u32),
                        "`:=` is not Newspeak syntax; a setter send is written `name:: expression`",
                    ));
                } else {
                    self.single(TokenKind::Colon);
                }
            }
            _ if byte == b'_' || byte.is_ascii_alphabetic() => self.lex_word(),
            _ if byte.is_ascii_digit() => self.lex_number(),
            b'\'' => self.lex_string(),
            b'"' => self.lex_character_constant(),
            _ if byte == b'-'
                && self
                    .peek_at(1)
                    .is_some_and(|next_byte| next_byte.is_ascii_digit())
                && (!self.previous_ends_value() || self.preceded_by_whitespace()) =>
            {
                self.lex_number()
            }
            _ if SPECIAL.contains(&byte) => self.lex_operator(),
            _ => {
                // Decode the real UTF-8 character at `self.position` rather than
                // Latin-1-decoding a single byte: `byte as char` mangles a
                // multi-byte codepoint (e.g. `é` -> `Ã©`, reported twice) and
                // leaves the span ending mid-codepoint, which panics later in
                // `Span::text`. Advance by the whole character; one diagnostic per
                // stray character, span aligned to codepoint boundaries.
                let character = self.source[self.position..]
                    .chars()
                    .next()
                    .unwrap_or('\u{FFFD}');
                self.position += character.len_utf8();
                self.diagnostics.push(Diagnostic::error(
                    Span::new(start as u32, self.position as u32),
                    format!("unexpected character {character:?}"),
                ));
            }
        }
    }

    fn single(&mut self, kind: TokenKind) {
        let start = self.position;
        self.position += 1;
        self.push(kind, start, self.position);
    }

    fn lex_word(&mut self) {
        let start = self.position;
        while matches!(self.peek(), Some(byte) if byte == b'_' || byte.is_ascii_alphanumeric()) {
            self.position += 1;
        }
        // keyword / setter-keyword?
        if self.peek() == Some(b':') {
            if self.peek_at(1) == Some(b'=')
                || (self.peek_at(1) == Some(b':') && self.peek_at(2) == Some(b'='))
            {
                // `id::=` -> Identifier then ColonColonEquals, and `id:=` ->
                // Identifier then the `:` arm's refusal of `:=`. Emit the bare
                // word and let `lex_one`'s `:` arm take the rest.
                let kind = self.word_kind(start, self.position);
                self.push(kind, start, self.position);
                return;
            }
            if self.peek_at(1) == Some(b':') {
                self.position += 2;
                self.push(TokenKind::SetterKeyword, start, self.position);
                return;
            }
            self.position += 1;
            self.push(TokenKind::Keyword, start, self.position);
            return;
        }
        let kind = self.word_kind(start, self.position);
        self.push(kind, start, self.position);
    }

    fn word_kind(&self, start: usize, end: usize) -> TokenKind {
        match &self.source[start..end] {
            "class" => TokenKind::Class,
            "private" => TokenKind::Private,
            "public" => TokenKind::Public,
            "protected" => TokenKind::Protected,
            _ => TokenKind::Identifier,
        }
    }

    fn lex_operator(&mut self) {
        let start = self.position;
        // Fixed single-character punctuation that also happens to be `specialCharacter`.
        let byte = self.bytes[self.position];
        let next = self.peek_at(1);
        let is_run_of_two_plus = next.is_some_and(|next_byte| SPECIAL.contains(&next_byte));
        if !is_run_of_two_plus {
            match byte {
                b'=' => return self.single(TokenKind::Equals),
                b'<' => return self.single(TokenKind::LessThan),
                b'>' => return self.single(TokenKind::GreaterThan),
                _ => {}
            }
        }
        // `::=` / `:=` are handled in `lex_one`'s `:` arm before we get here; a run
        // starting with a special character is a binary selector.
        while matches!(self.peek(), Some(next_byte) if SPECIAL.contains(&next_byte)) {
            self.position += 1;
        }
        self.push(TokenKind::BinarySelector, start, self.position);
    }

    fn lex_number(&mut self) {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        self.eat_digits();
        // radix?
        if self.peek() == Some(b'r') {
            self.position += 1;
            if self.peek() == Some(b'-') {
                self.position += 1;
            }
            self.eat_extended_digits();
            if self.peek() == Some(b'.')
                && self
                    .peek_at(1)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
            {
                self.position += 1;
                self.eat_extended_digits();
            }
        } else if self.peek() == Some(b'.')
            && self.peek_at(1).is_some_and(|byte| byte.is_ascii_digit())
        {
            self.position += 1;
            self.eat_digits();
        }
        if self.peek() == Some(b'e') && self.exponent_follows() {
            self.position += 1;
            if self.peek() == Some(b'-') {
                self.position += 1;
            }
            self.eat_digits();
        }
        self.push(TokenKind::Number, start, self.position);
    }

    fn exponent_follows(&self) -> bool {
        let mut offset = 1;
        if self.peek_at(offset) == Some(b'-') {
            offset += 1;
        }
        self.peek_at(offset)
            .is_some_and(|byte| byte.is_ascii_digit())
    }

    fn eat_digits(&mut self) {
        while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
            self.position += 1;
        }
    }

    fn eat_extended_digits(&mut self) {
        while matches!(self.peek(), Some(byte) if byte.is_ascii_digit() || byte.is_ascii_uppercase())
        {
            self.position += 1;
        }
    }

    fn lex_string(&mut self) {
        let start = self.position;
        self.position += 1; // opening quote
        loop {
            match self.peek() {
                None => {
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start as u32, self.position as u32),
                        "unterminated string literal",
                    ));
                    break;
                }
                Some(b'\'') => {
                    if self.peek_at(1) == Some(b'\'') {
                        self.position += 2;
                    } else {
                        self.position += 1;
                        break;
                    }
                }
                Some(_) => self.position += 1,
            }
        }
        self.push(TokenKind::String, start, self.position);
    }

    fn lex_symbol(&mut self) {
        let start = self.position;
        self.position += 1; // '#'
        match self.peek() {
            Some(b'\'') => {
                self.lex_string();
                self.tokens.pop(); // replace String token with Symbol span
            }
            Some(byte) if byte == b'_' || byte.is_ascii_alphabetic() => {
                // identifier, possibly a run of keywords `at:put:`
                while matches!(self.peek(), Some(next_byte) if next_byte == b'_' || next_byte.is_ascii_alphanumeric())
                {
                    self.position += 1;
                }
                while self.peek() == Some(b':') {
                    self.position += 1;
                    while matches!(self.peek(), Some(next_byte) if next_byte == b'_' || next_byte.is_ascii_alphanumeric())
                    {
                        self.position += 1;
                    }
                }
            }
            Some(byte) if SPECIAL.contains(&byte) => {
                while matches!(self.peek(), Some(next_byte) if SPECIAL.contains(&next_byte)) {
                    self.position += 1;
                }
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    Span::new(start as u32, self.position as u32),
                    "expected symbol after `#`",
                ));
            }
        }
        self.push(TokenKind::Symbol, start, self.position);
    }

    fn lex_character_constant(&mut self) {
        let start = self.position;
        self.position += 1; // opening `"` (ASCII, one byte)
                            // The character between the quotes may be any
                            // single codepoint, including a multi-byte one
                            // (`"é"`). Advance by its full UTF-8 width so the
                            // token span never ends mid-codepoint.
        match self.source[self.position..].chars().next() {
            Some(character) => self.position += character.len_utf8(),
            None => {
                self.diagnostics.push(Diagnostic::error(
                    Span::new(start as u32, self.position as u32),
                    "unterminated character constant",
                ));
                self.push(TokenKind::CharacterConstant, start, self.position);
                return;
            }
        }
        if self.peek() == Some(b'"') {
            self.position += 1;
        } else {
            self.diagnostics.push(Diagnostic::error(
                Span::new(start as u32, self.position as u32),
                "character constant must contain exactly one character",
            ));
        }
        self.push(TokenKind::CharacterConstant, start, self.position);
    }

    /// True when the byte immediately before the current position is ASCII
    /// whitespace, or we are at the start of input. Used to let `-42` lex as a
    /// negative literal even after a value token when a space separates them
    /// (`a -42`), while `a-42` and `a - 42` stay subtraction.
    ///
    /// NOTE: Comments are not considered whitespace here. This is deliberate:
    /// a comment is trivia but does NOT count as preceding whitespace for the
    /// purposes of distinguishing negative literals from subtraction. So `x (*c*)-42`
    /// stays subtraction; `x (*c*) -42` is a negative literal (the real space after
    /// the comment counts).
    fn preceded_by_whitespace(&self) -> bool {
        self.position == 0 || self.bytes[self.position - 1].is_ascii_whitespace()
    }

    fn previous_ends_value(&self) -> bool {
        matches!(
            self.tokens.last().map(|token| token.kind),
            Some(
                TokenKind::Identifier
                    | TokenKind::Number
                    | TokenKind::String
                    | TokenKind::Symbol
                    | TokenKind::CharacterConstant
                    | TokenKind::RightParenthesis
                    | TokenKind::RightBracket
                    | TokenKind::RightBrace
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenKind::*;

    fn kinds(source: &str) -> Vec<crate::token::TokenKind> {
        let (tokens, diagnostics) = Lexer::new(source).run();
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
        tokens.into_iter().map(|token| token.kind).collect()
    }

    #[test]
    fn identifiers_and_keywords() {
        assert_eq!(
            kinds("foo bar: baz"),
            vec![Identifier, Keyword, Identifier, EndOfInput]
        );
    }

    #[test]
    fn leading_underscore_identifier() {
        assert_eq!(kinds("_x"), vec![Identifier, EndOfInput]);
    }

    #[test]
    fn reserved_words() {
        assert_eq!(
            kinds("class public private"),
            vec![Class, Public, Private, EndOfInput]
        );
    }

    #[test]
    fn punctuation_versus_binary_selector() {
        assert_eq!(
            kinds("| || = ::= ^ ."),
            vec![
                Bar,
                DoubleBar,
                Equals,
                ColonColonEquals,
                Caret,
                Dot,
                EndOfInput,
            ]
        );
    }

    #[test]
    fn smalltalk_assignment_is_refused_by_name() {
        // §5.4.3 gives Newspeak one assignment form, the setter send. Both the
        // spaced and the glued spelling are named, not split into `:` and `=`.
        for (source, start, end) in [("x := 1", 2, 4), ("x:=1", 1, 3)] {
            let (_, diagnostics) = Lexer::new(source).run();
            assert_eq!(diagnostics.len(), 1, "{source}: {diagnostics:?}");
            let diagnostic = &diagnostics[0];
            assert_eq!(
                diagnostic.severity,
                crate::diagnostic::Severity::Error,
                "{source}"
            );
            assert_eq!(diagnostic.span, Span::new(start, end), "{source}");
            assert!(
                diagnostic
                    .message
                    .starts_with("`:=` is not Newspeak syntax"),
                "{source}: {}",
                diagnostic.message
            );
        }
    }

    #[test]
    fn binary_selectors() {
        assert_eq!(
            kinds("a + b <= c"),
            vec![
                Identifier,
                BinarySelector,
                Identifier,
                BinarySelector,
                Identifier,
                EndOfInput,
            ]
        );
    }

    #[test]
    fn setter_keyword() {
        assert_eq!(kinds("x:: 3"), vec![SetterKeyword, Number, EndOfInput]);
    }

    #[test]
    fn backslash_is_a_binary_selector() {
        assert_eq!(
            kinds("a \\\\ b"),
            vec![Identifier, BinarySelector, Identifier, EndOfInput]
        );
    }

    #[test]
    fn setter_keyword_initializer_without_spaces() {
        assert_eq!(
            kinds("foo::=bar"),
            vec![Identifier, ColonColonEquals, Identifier, EndOfInput]
        );
    }

    #[test]
    fn decimal_numbers() {
        assert_eq!(
            kinds("0 42 3.14 1.5e-3"),
            vec![Number, Number, Number, Number, EndOfInput,]
        );
    }

    #[test]
    fn radix_numbers() {
        assert_eq!(kinds("16rFF 2r1010"), vec![Number, Number, EndOfInput]);
    }

    #[test]
    fn negative_number_versus_subtraction() {
        assert_eq!(kinds("-42"), vec![Number, EndOfInput]);
        assert_eq!(
            kinds("a - 42"),
            vec![Identifier, BinarySelector, Number, EndOfInput]
        );
        assert_eq!(kinds("a -42"), vec![Identifier, Number, EndOfInput]);
    }

    #[test]
    fn strings_with_escaped_quote() {
        let (tokens, diagnostics) = Lexer::new("'it''s'").run();
        assert!(diagnostics.is_empty());
        assert_eq!(tokens[0].kind, String);
        assert_eq!(tokens[0].span.text("'it''s'"), "'it''s'");
    }

    #[test]
    fn symbols() {
        assert_eq!(
            kinds("#foo #at:put: #+ #'hi there'"),
            vec![Symbol, Symbol, Symbol, Symbol, EndOfInput,]
        );
    }

    #[test]
    fn hash_parenthesis_is_pound_then_left_parenthesis() {
        assert_eq!(
            kinds("#(1)"),
            vec![Pound, LeftParenthesis, Number, RightParenthesis, EndOfInput]
        );
    }

    #[test]
    fn character_constants() {
        assert_eq!(
            kinds(r#""a" "'" " ""#),
            vec![
                CharacterConstant,
                CharacterConstant,
                CharacterConstant,
                EndOfInput
            ]
        );
    }

    #[test]
    fn unterminated_string_is_diagnostic() {
        let (_tokens, diagnostics) = Lexer::new("'oops").run();
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn line_and_block_comments_are_trivia() {
        assert_eq!(
            kinds("a (* comment *) b"),
            vec![Identifier, Identifier, EndOfInput]
        );
    }

    #[test]
    fn comments_nest() {
        assert_eq!(
            kinds("a (* x (* y *) z *) b"),
            vec![Identifier, Identifier, EndOfInput]
        );
    }

    #[test]
    fn unterminated_comment_is_diagnostic() {
        let (_t, diagnostics) = Lexer::new("a (* oops").run();
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn lex_api_returns_error_on_diagnostics() {
        assert!(crate::lex("'unterminated").is_err());
        assert!(crate::lex("class Foo = ()").is_ok());
    }

    #[test]
    fn token_stream_ends_with_end_of_input() {
        let tokens = crate::lex("foo").unwrap();
        assert_eq!(tokens.last().unwrap().kind, EndOfInput);
    }

    #[test]
    fn stray_non_ascii_character_is_one_codepoint_aligned_diagnostic() {
        let source = "é";
        let (_tokens, diagnostics) = Lexer::new(source).run();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        // Span is codepoint-aligned: `text` must not panic and covers the character.
        assert_eq!(diagnostics[0].span.text(source), "é");
    }

    #[test]
    fn character_constant_may_hold_a_multibyte_character() {
        let source = "\"é\"";
        let (tokens, diagnostics) = Lexer::new(source).run();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let character_constants: Vec<_> = tokens
            .iter()
            .filter(|token| token.kind == CharacterConstant)
            .collect();
        assert_eq!(character_constants.len(), 1);
        assert_eq!(character_constants[0].span.text(source), "\"é\"");
    }

    #[test]
    fn literalmessage_pragma_comment_is_plain_trivia() {
        // `(* :literalmessage: ... *)` is an ordinary block comment: no tokens,
        // no diagnostics.
        let (tokens, diagnostics) = Lexer::new("(* :literalmessage: foo bar *)").run();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            tokens
                .into_iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>(),
            vec![EndOfInput]
        );
    }

    #[test]
    fn lexes_vendored_grammar_without_error() {
        let source = std::fs::read_to_string("../../corpus/reference/NewspeakGrammar.ns").unwrap();
        let (_tokens, diagnostics) = Lexer::new(&source).run();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }
}
