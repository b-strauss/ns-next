//! Single-pass lexer for Newspeak3 source.

use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::token::{Token, TokenKind};

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    diags: Vec<Diagnostic>,
}

const SPECIAL: &[u8] = b"+-*/\\~<>=&|@%,?!";

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Lexer<'a> {
        Lexer {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            diags: Vec::new(),
        }
    }

    pub fn run(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        while self.pos < self.bytes.len() {
            self.skip_trivia(); // full impl in Task 5; for now: skip ASCII whitespace
            if self.pos >= self.bytes.len() {
                break;
            }
            self.lex_one();
        }
        self.push(TokenKind::Eoi, self.pos, self.pos);
        (self.tokens, self.diags)
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b) if b.is_ascii_whitespace() => self.pos += 1,
                Some(b'(') if self.peek_at(1) == Some(b'*') => self.skip_block_comment(),
                _ => break,
            }
        }
    }

    fn skip_block_comment(&mut self) {
        let start = self.pos;
        self.pos += 2;
        let mut depth = 1u32;
        while depth > 0 {
            match (self.peek(), self.peek_at(1)) {
                (Some(b'('), Some(b'*')) => {
                    self.pos += 2;
                    depth += 1;
                }
                (Some(b'*'), Some(b')')) => {
                    self.pos += 2;
                    depth -= 1;
                }
                (Some(_), _) => self.pos += 1,
                (None, _) => {
                    self.diags.push(Diagnostic::error(
                        Span::new(start as u32, self.pos as u32),
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
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn lex_one(&mut self) {
        let start = self.pos;
        let b = self.bytes[self.pos];
        match b {
            b'(' => self.single(TokenKind::LParen),
            b')' => self.single(TokenKind::RParen),
            b'[' => self.single(TokenKind::LBracket),
            b']' => self.single(TokenKind::RBracket),
            b'{' => self.single(TokenKind::LBrace),
            b'}' => self.single(TokenKind::RBrace),
            b'^' => self.single(TokenKind::Caret),
            b'.' => self.single(TokenKind::Dot),
            b';' => self.single(TokenKind::Semicolon),
            b'#' if self.peek_at(1) == Some(b'(') => self.single(TokenKind::Pound),
            b'#' => self.lex_symbol(),
            b'|' => {
                if self.peek_at(1) == Some(b'|') {
                    self.pos += 2;
                    self.push(TokenKind::DoubleBar, start, self.pos);
                } else if self.peek_at(1).is_some_and(|n| SPECIAL.contains(&n)) {
                    // `|` starting a multi-char run (`|>`, `|=`, ...) is a
                    // binary selector, not a slot/temp delimiter.
                    self.lex_operator();
                } else {
                    self.single(TokenKind::Bar);
                }
            }
            b':' => {
                if self.peek_at(1) == Some(b'=') {
                    self.pos += 2;
                    self.push(TokenKind::Assign, start, self.pos);
                } else if self.peek_at(1) == Some(b':') && self.peek_at(2) == Some(b'=') {
                    self.pos += 3;
                    self.push(TokenKind::ColonColonEquals, start, self.pos);
                } else {
                    self.single(TokenKind::Colon);
                }
            }
            _ if b == b'_' || b.is_ascii_alphabetic() => self.lex_word(),
            _ if b.is_ascii_digit() => self.lex_number(), // Task 4
            b'\'' => self.lex_string(),                   // Task 4
            b'"' => self.lex_char_constant(),             // Task 4
            _ if b == b'-'
                && self.peek_at(1).is_some_and(|n| n.is_ascii_digit())
                && (!self.prev_ends_value() || self.preceded_by_whitespace()) =>
            {
                self.lex_number()
            }
            _ if SPECIAL.contains(&b) => self.lex_operator(),
            _ => {
                self.pos += 1;
                self.diags.push(Diagnostic::error(
                    Span::new(start as u32, self.pos as u32),
                    format!("unexpected character {:?}", b as char),
                ));
            }
        }
    }

    fn single(&mut self, kind: TokenKind) {
        let start = self.pos;
        self.pos += 1;
        self.push(kind, start, self.pos);
    }

    fn lex_word(&mut self) {
        let start = self.pos;
        while matches!(self.peek(), Some(b) if b == b'_' || b.is_ascii_alphanumeric()) {
            self.pos += 1;
        }
        // keyword / setter-keyword?
        if self.peek() == Some(b':') {
            if self.peek_at(1) == Some(b'=')
                || (self.peek_at(1) == Some(b':') && self.peek_at(2) == Some(b'='))
            {
                // `id:=` -> Identifier then Assign; `id::=` -> Identifier then
                // ColonColonEquals. Emit the bare word and let `lex_one`'s `:` arm
                // form the operator.
                let kind = self.word_kind(start, self.pos);
                self.push(kind, start, self.pos);
                return;
            }
            if self.peek_at(1) == Some(b':') {
                self.pos += 2;
                self.push(TokenKind::SetterKeyword, start, self.pos);
                return;
            }
            self.pos += 1;
            self.push(TokenKind::Keyword, start, self.pos);
            return;
        }
        let kind = self.word_kind(start, self.pos);
        self.push(kind, start, self.pos);
    }

    fn word_kind(&self, start: usize, end: usize) -> TokenKind {
        match &self.src[start..end] {
            "class" => TokenKind::Class,
            "private" => TokenKind::Private,
            "public" => TokenKind::Public,
            "protected" => TokenKind::Protected,
            _ => TokenKind::Identifier,
        }
    }

    fn lex_operator(&mut self) {
        let start = self.pos;
        // Fixed single-char punctuation that also happens to be `specialCharacter`.
        let b = self.bytes[self.pos];
        let next = self.peek_at(1);
        let is_run_of_two_plus = next.is_some_and(|n| SPECIAL.contains(&n));
        if !is_run_of_two_plus {
            match b {
                b'=' => return self.single(TokenKind::Equals),
                b'<' => return self.single(TokenKind::LessThan),
                b'>' => return self.single(TokenKind::GreaterThan),
                _ => {}
            }
        }
        // `::=` / `:=` are handled in `lex_one`'s `:` arm before we get here; a run
        // starting with a special char is a binary selector.
        while matches!(self.peek(), Some(n) if SPECIAL.contains(&n)) {
            self.pos += 1;
        }
        self.push(TokenKind::BinarySelector, start, self.pos);
    }

    fn lex_number(&mut self) {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        self.eat_digits();
        // radix?
        if self.peek() == Some(b'r') {
            self.pos += 1;
            if self.peek() == Some(b'-') {
                self.pos += 1;
            }
            self.eat_extended_digits();
            if self.peek() == Some(b'.')
                && self.peek_at(1).is_some_and(|b| b.is_ascii_alphanumeric())
            {
                self.pos += 1;
                self.eat_extended_digits();
            }
        } else if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
            self.eat_digits();
        }
        if self.peek() == Some(b'e') && self.exponent_follows() {
            self.pos += 1;
            if self.peek() == Some(b'-') {
                self.pos += 1;
            }
            self.eat_digits();
        }
        self.push(TokenKind::Number, start, self.pos);
    }

    fn exponent_follows(&self) -> bool {
        let mut i = 1;
        if self.peek_at(i) == Some(b'-') {
            i += 1;
        }
        self.peek_at(i).is_some_and(|b| b.is_ascii_digit())
    }

    fn eat_digits(&mut self) {
        while matches!(self.peek(), Some(b) if b.is_ascii_digit()) {
            self.pos += 1;
        }
    }

    fn eat_extended_digits(&mut self) {
        while matches!(self.peek(), Some(b) if b.is_ascii_digit() || b.is_ascii_uppercase()) {
            self.pos += 1;
        }
    }

    fn lex_string(&mut self) {
        let start = self.pos;
        self.pos += 1; // opening quote
        loop {
            match self.peek() {
                None => {
                    self.diags.push(Diagnostic::error(
                        Span::new(start as u32, self.pos as u32),
                        "unterminated string literal",
                    ));
                    break;
                }
                Some(b'\'') => {
                    if self.peek_at(1) == Some(b'\'') {
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                        break;
                    }
                }
                Some(_) => self.pos += 1,
            }
        }
        self.push(TokenKind::String, start, self.pos);
    }

    fn lex_symbol(&mut self) {
        let start = self.pos;
        self.pos += 1; // '#'
        match self.peek() {
            Some(b'\'') => {
                self.lex_string();
                self.tokens.pop(); // replace String token with Symbol span
            }
            Some(b) if b == b'_' || b.is_ascii_alphabetic() => {
                // identifier, possibly a run of keywords `at:put:`
                while matches!(self.peek(), Some(c) if c == b'_' || c.is_ascii_alphanumeric()) {
                    self.pos += 1;
                }
                while self.peek() == Some(b':') {
                    self.pos += 1;
                    while matches!(self.peek(), Some(c) if c == b'_' || c.is_ascii_alphanumeric()) {
                        self.pos += 1;
                    }
                }
            }
            Some(b) if SPECIAL.contains(&b) => {
                while matches!(self.peek(), Some(c) if SPECIAL.contains(&c)) {
                    self.pos += 1;
                }
            }
            _ => {
                self.diags.push(Diagnostic::error(
                    Span::new(start as u32, self.pos as u32),
                    "expected symbol after `#`",
                ));
            }
        }
        self.push(TokenKind::Symbol, start, self.pos);
    }

    fn lex_char_constant(&mut self) {
        let start = self.pos;
        self.pos += 1; // opening `"`
        match self.peek() {
            Some(_) => self.pos += 1,
            None => {
                self.diags.push(Diagnostic::error(
                    Span::new(start as u32, self.pos as u32),
                    "unterminated character constant",
                ));
                self.push(TokenKind::CharConstant, start, self.pos);
                return;
            }
        }
        if self.peek() == Some(b'"') {
            self.pos += 1;
        } else {
            self.diags.push(Diagnostic::error(
                Span::new(start as u32, self.pos as u32),
                "character constant must contain exactly one character",
            ));
        }
        self.push(TokenKind::CharConstant, start, self.pos);
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
        self.pos == 0 || self.bytes[self.pos - 1].is_ascii_whitespace()
    }

    fn prev_ends_value(&self) -> bool {
        matches!(
            self.tokens.last().map(|t| t.kind),
            Some(
                TokenKind::Identifier
                    | TokenKind::Number
                    | TokenKind::String
                    | TokenKind::Symbol
                    | TokenKind::CharConstant
                    | TokenKind::RParen
                    | TokenKind::RBracket
                    | TokenKind::RBrace
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenKind::*;

    fn kinds(src: &str) -> Vec<crate::token::TokenKind> {
        let (tokens, diags) = Lexer::new(src).run();
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn identifiers_and_keywords() {
        assert_eq!(
            kinds("foo bar: baz"),
            vec![Identifier, Keyword, Identifier, Eoi]
        );
    }

    #[test]
    fn leading_underscore_identifier() {
        assert_eq!(kinds("_x"), vec![Identifier, Eoi]);
    }

    #[test]
    fn reserved_words() {
        assert_eq!(
            kinds("class public private"),
            vec![Class, Public, Private, Eoi]
        );
    }

    #[test]
    fn punctuation_vs_binary_selector() {
        assert_eq!(
            kinds("| || = := ::= ^ ."),
            vec![
                Bar,
                DoubleBar,
                Equals,
                Assign,
                ColonColonEquals,
                Caret,
                Dot,
                Eoi,
            ]
        );
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
                Eoi,
            ]
        );
    }

    #[test]
    fn setter_keyword() {
        assert_eq!(kinds("x:: 3"), vec![SetterKeyword, Number, Eoi]);
    }

    #[test]
    fn backslash_is_a_binary_selector() {
        assert_eq!(
            kinds("a \\\\ b"),
            vec![Identifier, BinarySelector, Identifier, Eoi]
        );
    }

    #[test]
    fn setter_keyword_init_without_spaces() {
        assert_eq!(
            kinds("foo::=bar"),
            vec![Identifier, ColonColonEquals, Identifier, Eoi]
        );
    }

    #[test]
    fn decimal_numbers() {
        assert_eq!(
            kinds("0 42 3.14 1.5e-3"),
            vec![Number, Number, Number, Number, Eoi,]
        );
    }

    #[test]
    fn radix_numbers() {
        assert_eq!(kinds("16rFF 2r1010"), vec![Number, Number, Eoi]);
    }

    #[test]
    fn negative_number_vs_subtraction() {
        assert_eq!(kinds("-42"), vec![Number, Eoi]);
        assert_eq!(
            kinds("a - 42"),
            vec![Identifier, BinarySelector, Number, Eoi]
        );
        assert_eq!(kinds("a -42"), vec![Identifier, Number, Eoi]);
    }

    #[test]
    fn strings_with_escaped_quote() {
        let (tokens, diags) = Lexer::new("'it''s'").run();
        assert!(diags.is_empty());
        assert_eq!(tokens[0].kind, String);
        assert_eq!(tokens[0].span.text("'it''s'"), "'it''s'");
    }

    #[test]
    fn symbols() {
        assert_eq!(
            kinds("#foo #at:put: #+ #'hi there'"),
            vec![Symbol, Symbol, Symbol, Symbol, Eoi,]
        );
    }

    #[test]
    fn hash_paren_is_pound_then_lparen() {
        assert_eq!(kinds("#(1)"), vec![Pound, LParen, Number, RParen, Eoi]);
    }

    #[test]
    fn character_constants() {
        assert_eq!(
            kinds(r#""a" "'" " ""#),
            vec![CharConstant, CharConstant, CharConstant, Eoi]
        );
    }

    #[test]
    fn unterminated_string_is_diagnostic() {
        let (_tokens, diags) = Lexer::new("'oops").run();
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn line_and_block_comments_are_trivia() {
        assert_eq!(
            kinds("a (* comment *) b"),
            vec![Identifier, Identifier, Eoi]
        );
    }

    #[test]
    fn comments_nest() {
        assert_eq!(
            kinds("a (* x (* y *) z *) b"),
            vec![Identifier, Identifier, Eoi]
        );
    }

    #[test]
    fn unterminated_comment_is_diagnostic() {
        let (_t, diags) = Lexer::new("a (* oops").run();
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn lex_api_returns_err_on_diagnostics() {
        assert!(crate::lex("'unterminated").is_err());
        assert!(crate::lex("class Foo = ()").is_ok());
    }

    #[test]
    fn token_stream_ends_with_eoi() {
        let tokens = crate::lex("foo").unwrap();
        assert_eq!(tokens.last().unwrap().kind, Eoi);
    }

    #[test]
    fn lexes_vendored_grammar_without_error() {
        let src = std::fs::read_to_string("../../corpus/reference/NewspeakGrammar.ns").unwrap();
        let (_tokens, diags) = Lexer::new(&src).run();
        assert!(diags.is_empty(), "{diags:#?}");
    }
}
