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
        while let Some(&b) = self.bytes.get(self.pos) {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
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
            b'#' => self.single(TokenKind::Pound), // Symbol body lexed in Task 4
            b'|' => {
                if self.peek_at(1) == Some(b'|') {
                    self.pos += 2;
                    self.push(TokenKind::DoubleBar, start, self.pos);
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

    // Task 4 stubs — implemented next task.
    fn lex_number(&mut self) {
        self.single(TokenKind::Number);
    }
    fn lex_string(&mut self) {
        self.single(TokenKind::String);
    }
    fn lex_char_constant(&mut self) {
        self.single(TokenKind::CharConstant);
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
}
