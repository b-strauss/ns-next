//! Single-pass lexer for Newspeak3 source.

use crate::comment::{
    Comment, CommentKind, continues_line_block, first_line_break, first_of_either,
};
use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::token::{Token, TokenKind};

pub struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    position: usize,
    tokens: Vec<Token>,
    comments: Vec<Comment>,
    diagnostics: Vec<Diagnostic>,
}

/// A source's tokens with its comments kept, for an editor: the parser and
/// the runtime never see a comment, so `run` drops them.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub comments: Vec<Comment>,
    pub diagnostics: Vec<Diagnostic>,
}

/// §4.2's `specialCharacter`. `-` is not one: `binSel → (specialCharacter |
/// "-") specialCharacter*` lets it start a binary selector and nothing
/// more, so `+-` is `+` followed by `-`, and `x +-1` adds a negative one.
const SPECIAL: &[u8] = b"+*/\\~<>=&|@%,?!";

/// The words lexed as tokens of their own, so that a declaration can be
/// recognized by its first token; each is a name wherever the grammar takes
/// an identifier (`docs/spec/`, the rule `lex.unreserved-words`, whose
/// table the lexer's tests hold this one to).
const WORDS_WITH_TOKENS: [(&str, TokenKind); 4] = [
    ("class", TokenKind::Class),
    ("private", TokenKind::Private),
    ("public", TokenKind::Public),
    ("protected", TokenKind::Protected),
];

/// Whether `byte` can start a binary selector.
fn starts_binary_selector(byte: u8) -> bool {
    byte == b'-' || SPECIAL.contains(&byte)
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Lexer<'a> {
        Lexer {
            source,
            bytes: source.as_bytes(),
            position: 0,
            tokens: Vec::new(),
            comments: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn run(self) -> (Vec<Token>, Vec<Diagnostic>) {
        let lexed = self.run_keeping_comments();
        (lexed.tokens, lexed.diagnostics)
    }

    pub fn run_keeping_comments(mut self) -> Lexed {
        while self.position < self.bytes.len() {
            self.skip_trivia();
            if self.position >= self.bytes.len() {
                break;
            }
            self.lex_one();
        }
        self.push(TokenKind::EndOfInput, self.position, self.position);
        Lexed {
            tokens: self.tokens,
            comments: self.comments,
            diagnostics: self.diagnostics,
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                // The run counted in a local, not `self.position` stored at
                // every byte: indentation is most of a source's whitespace.
                Some(byte) if byte.is_ascii_whitespace() => {
                    self.position += self.bytes[self.position..]
                        .iter()
                        .take_while(|byte| byte.is_ascii_whitespace())
                        .count();
                }
                Some(b'(') if self.peek_at(1) == Some(b'*') => self.skip_block_comment(),
                Some(b'/') if self.starts_line_comment(0) => self.skip_line_comment(),
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
                    if depth == 0 {
                        self.comments.push(Comment {
                            kind: CommentKind::Block,
                            span: Span::from_offsets(start, self.position),
                        });
                    }
                }
                // Straight to the next byte that can open or close a
                // comment, rather than one byte at a time through the text.
                (Some(_), _) => {
                    let rest = &self.bytes[self.position + 1..];
                    self.position += 1 + first_of_either(rest, b'(', b'*').unwrap_or(rest.len());
                }
                (None, _) => {
                    self.diagnostics.push(Diagnostic::error(
                        Span::from_offsets(start, self.position),
                        "unterminated comment",
                    ));
                    self.comments.push(Comment {
                        kind: CommentKind::Block,
                        span: Span::from_offsets(start, self.position),
                    });
                    return;
                }
            }
        }
    }

    /// Whether ns-next's `///` line comment starts `offset` bytes ahead.
    /// It is a deviation from §4.2, whose `binSel` admits `///` and every
    /// run of special characters containing it; a comment wherever it
    /// stands is what a reader expects (`docs/spec/`, the rule
    /// `lex.comment.line`, and its appendix's "Differences from Bracha's
    /// Newspeak").
    fn starts_line_comment(&self, offset: usize) -> bool {
        let start = self.position + offset;
        self.bytes.get(start..start + 3) == Some(b"///".as_slice())
    }

    /// Whether the byte `offset` ahead continues a binary selector: a
    /// special character that does not start a line comment.
    fn continues_binary_selector(&self, offset: usize) -> bool {
        self.peek_at(offset)
            .is_some_and(|byte| SPECIAL.contains(&byte))
            && !self.starts_line_comment(offset)
    }

    /// A `///` comment, to the end of its line. It joins the comment before
    /// it when the two form one block (`continues_line_block`), so a reader
    /// gets one comment, with one text, per block.
    fn skip_line_comment(&mut self) {
        let start = self.position;
        self.position = first_line_break(&self.bytes[start..])
            .map_or(self.bytes.len(), |length| start + length);
        let span = Span::from_offsets(start, self.position);
        if let Some(previous) = self.comments.last_mut()
            && previous.kind == CommentKind::Line
            && continues_line_block(self.source, previous.span, start)
        {
            previous.span = previous.span.merge(span);
            return;
        }
        self.comments.push(Comment {
            kind: CommentKind::Line,
            span,
        });
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::from_offsets(start, end),
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
            // Smalltalk's literal array. Newspeak has none -- neither the
            // specification's `literal` (§5.1) nor the canonical grammar --
            // and writes a tuple, `{ 1. 2 }`, instead (§5.1.7). Named here,
            // because a bare `#` would surface as an unrelated error.
            b'#' if self.peek_at(1) == Some(b'(') => {
                self.position += 1;
                self.diagnostics.push(Diagnostic::error(
                    Span::from_offsets(start, self.position),
                    "`#( ... )` is not Newspeak syntax; a tuple is written `{ element. element }`",
                ));
            }
            b'#' => self.lex_symbol(),
            b'|' => {
                if self.peek_at(1) == Some(b'|') {
                    self.position += 2;
                    self.push(TokenKind::DoubleBar, start, self.position);
                } else if self.continues_binary_selector(1) {
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
                        Span::from_offsets(start, self.position),
                        "`:=` is not Newspeak syntax; a setter send is written `name:: expression`",
                    ));
                } else {
                    self.single(TokenKind::Colon);
                }
            }
            _ if byte == b'_' || byte.is_ascii_alphabetic() => self.lex_word(),
            _ if byte.is_ascii_digit() => self.lex_number(),
            b'\'' => self.lex_string(),
            b'"' => self.lex_double_quoted_string(),
            // §4.2's `decimalNum → ["-"] digits ...`: a `-` against a digit is
            // a negative literal's sign wherever a literal can stand. Where a
            // binary message can, the PEG takes it as the selector instead
            // (`3 -4` subtracts), which only the parser knows, so the parser
            // splits it there (`Parser::split_negative_number`).
            _ if byte == b'-'
                && self
                    .peek_at(1)
                    .is_some_and(|next_byte| next_byte.is_ascii_digit()) =>
            {
                self.lex_number()
            }
            _ if starts_binary_selector(byte) => self.lex_operator(),
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
                    Span::from_offsets(start, self.position),
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
        let word = &self.source[start..end];
        WORDS_WITH_TOKENS
            .iter()
            .find(|(spelling, _)| *spelling == word)
            .map_or(TokenKind::Identifier, |(_, kind)| *kind)
    }

    fn lex_operator(&mut self) {
        let start = self.position;
        // Fixed single-character punctuation that also happens to be `specialCharacter`.
        let byte = self.bytes[self.position];
        let next = self.peek_at(1);
        // `<:` is §6.2's `mixinOperator`, taken as one token even though `<`
        // alone is a `specialCharacter` that a binary selector may start with.
        // The longest match is safe because `:` is *not* a `specialCharacter`
        // (the specification's lexical grammar, `specialCharacter`), so no
        // binary selector spelled `<:` exists that a program could have meant
        // instead. `<` and `<=` are unaffected: neither is followed by `:`.
        if byte == b'<' && next == Some(b':') {
            self.position += 2;
            self.push(TokenKind::MixinOperator, start, self.position);
            return;
        }
        // §5.6's asynchronous send token. No binary selector is spelled `<-`,
        // since `-` cannot continue one, so the three characters are
        // unambiguous.
        if byte == b'<' && next == Some(b'-') && self.peek_at(2) == Some(b':') {
            self.position += 3;
            self.push(TokenKind::EventualSendOperator, start, self.position);
            return;
        }
        let is_run_of_two_plus = self.continues_binary_selector(1);
        if !is_run_of_two_plus {
            match byte {
                b'=' => return self.single(TokenKind::Equals),
                b'<' => return self.single(TokenKind::LessThan),
                b'>' => return self.single(TokenKind::GreaterThan),
                _ => {}
            }
        }
        // `::=` / `:=` are handled in `lex_one`'s `:` arm before we get here; a
        // binary selector is its first character and the special characters
        // after it.
        self.position += 1;
        while self.continues_binary_selector(0) {
            self.position += 1;
        }
        self.push(TokenKind::BinarySelector, start, self.position);
    }

    /// §4.2's `num → radixNum | decimalNum`. Each optional part is taken only
    /// when it is complete -- a radix `r` with an extended digit after it, a
    /// fraction's `.` with a digit -- so `16r` is `16` followed by `r`, and
    /// `1.foo` a `1` and a statement's end. Only `decimalNum` has a leading
    /// `-`, so `-16rFF` is `-16` followed by the name `rFF`.
    fn lex_number(&mut self) {
        let start = self.position;
        let negative = self.peek() == Some(b'-');
        if negative {
            self.position += 1;
        }
        self.eat_digits();
        let extended_digit_at = |offset: usize| self.peek_at(offset).is_some_and(is_extended_digit);
        let radix = !negative
            && self.peek() == Some(b'r')
            && (extended_digit_at(1) || (self.peek_at(1) == Some(b'-') && extended_digit_at(2)));
        if radix {
            self.position += 1; // `r`
            if self.peek() == Some(b'-') {
                self.position += 1;
            }
            self.eat_extended_digits();
            if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(is_extended_digit) {
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
        while self.peek().is_some_and(is_extended_digit) {
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
                        Span::from_offsets(start, self.position),
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
            // §4.2's `sym → str | kws | binSel | id`: an identifier, or a run
            // of keywords each ending in its colon (`#at:put:`). A word after
            // the last colon belongs to the symbol only when its own colon
            // follows, so `#at:put` is `#at:` followed by `put`.
            Some(byte) if byte == b'_' || byte.is_ascii_alphabetic() => {
                self.eat_word();
                while self.peek() == Some(b':') {
                    self.position += 1;
                    let after_colon = self.position;
                    self.eat_word();
                    if self.position == after_colon {
                        break;
                    }
                    if self.peek() != Some(b':') {
                        self.position = after_colon;
                        break;
                    }
                }
            }
            Some(byte) if starts_binary_selector(byte) && !self.starts_line_comment(0) => {
                self.position += 1;
                while self.continues_binary_selector(0) {
                    self.position += 1;
                }
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    Span::from_offsets(start, self.position),
                    "expected symbol after `#`",
                ));
            }
        }
        self.push(TokenKind::Symbol, start, self.position);
    }

    /// A double-quoted string literal (§5.1.5), which has no escape: it ends
    /// at the next `"`. The one exception is the one-character form -- any
    /// single character between two quotes, `"` included -- so `"""` is the
    /// string `"`, which is how the Newspeak corpus writes it; a `"` anywhere
    /// else is written in a single-quoted string instead. §5.1.4's character
    /// literals are gone, as it announces: `"c"` is a one-character string.
    fn lex_double_quoted_string(&mut self) {
        let start = self.position;
        self.position += 1; // opening `"` (ASCII, one byte)
        // Advance by the whole codepoint, so the span never ends mid-character.
        if let Some(character) = self.source[self.position..].chars().next() {
            let after = self.position + character.len_utf8();
            if self.source.as_bytes().get(after) == Some(&b'"') {
                self.position = after + 1;
                self.push(TokenKind::String, start, self.position);
                return;
            }
        }
        match self.source[self.position..].find('"') {
            Some(offset) => self.position += offset + 1,
            None => {
                self.position = self.source.len();
                self.diagnostics.push(Diagnostic::error(
                    Span::from_offsets(start, self.position),
                    "unterminated string literal",
                ));
            }
        }
        self.push(TokenKind::String, start, self.position);
    }

    fn eat_word(&mut self) {
        while matches!(self.peek(), Some(byte) if byte == b'_' || byte.is_ascii_alphanumeric()) {
            self.position += 1;
        }
    }
}

/// §4.2's `extendedDigits`: a digit or an uppercase letter.
fn is_extended_digit(byte: u8) -> bool {
    byte.is_ascii_digit() || byte.is_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comment::CommentKind;
    use crate::token::TokenKind::*;

    fn kinds(source: &str) -> Vec<crate::token::TokenKind> {
        let (tokens, diagnostics) = Lexer::new(source).run();
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
        tokens.into_iter().map(|token| token.kind).collect()
    }

    /// A nested comment is one span, outer `(*` to outer `*)`; a `(*` in a
    /// string opens none; an unterminated one runs to the end.
    // spec: lex.comment.block
    #[test]
    fn comment_spans_are_kept_on_request() {
        let source = "a (* outer (* inner *) still *) 'not (* a comment' (* open";
        let lexed = Lexer::new(source).run_keeping_comments();
        let texts: Vec<&str> = lexed
            .comments
            .iter()
            .map(|comment| comment.span.text(source))
            .collect();
        assert_eq!(texts, ["(* outer (* inner *) still *)", "(* open"]);
        assert_eq!(lexed.diagnostics.len(), 1, "the unterminated comment");
    }

    // spec: lex.special-characters
    #[test]
    fn the_special_characters_are_the_specifications() {
        let mut table: Vec<u8> = crate::spec_tables::lexical_table("special-characters")
            .iter()
            .map(|entry| {
                assert_eq!(entry.len(), 1, "`{entry}` is one character");
                entry.as_bytes()[0]
            })
            .collect();
        let mut special = SPECIAL.to_vec();
        table.sort_unstable();
        special.sort_unstable();
        assert_eq!(table, special);
    }

    // spec: lex.unreserved-words
    #[test]
    fn the_words_with_tokens_of_their_own_are_the_specifications() {
        let mut table = crate::spec_tables::lexical_table("words-with-tokens");
        let mut words: Vec<std::string::String> = WORDS_WITH_TOKENS
            .iter()
            .map(|(word, _)| word.to_string())
            .collect();
        table.sort_unstable();
        words.sort_unstable();
        assert_eq!(table, words);
        for (word, kind) in WORDS_WITH_TOKENS {
            assert_eq!(kinds(word)[0], kind, "`{word}`");
        }
        assert_eq!(kinds("lazy")[0], Identifier);
    }

    // spec: lex.identifier
    // spec: lex.keyword
    #[test]
    fn identifiers_and_keywords() {
        assert_eq!(
            kinds("foo bar: baz"),
            vec![Identifier, Keyword, Identifier, EndOfInput]
        );
    }

    // spec: lex.identifier
    #[test]
    fn leading_underscore_identifier() {
        assert_eq!(kinds("_x"), vec![Identifier, EndOfInput]);
    }

    // spec: lex.unreserved-words
    #[test]
    fn reserved_words() {
        assert_eq!(
            kinds("class public private"),
            vec![Class, Public, Private, EndOfInput]
        );
    }

    // spec: lex.punctuation
    // spec: lex.binary-selector
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

    // spec: lex.assignment-refused
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

    // spec: lex.binary-selector
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

    // spec: lex.punctuation
    #[test]
    fn mixin_operator_is_one_token_and_leaves_less_than_alone() {
        // `<:` is §6.2's `mixinOperator`, one token. Everything else that
        // starts with `<` keeps its old lexing: the binary selectors `<` and
        // `<=`, and the `<` that opens a declaration's type annotation.
        assert_eq!(
            kinds("x <: y"),
            vec![Identifier, MixinOperator, Identifier, EndOfInput]
        );
        assert_eq!(
            kinds("Object<:Value"),
            vec![Identifier, MixinOperator, Identifier, EndOfInput]
        );
        assert_eq!(
            kinds("x < y"),
            vec![Identifier, LessThan, Identifier, EndOfInput]
        );
        assert_eq!(
            kinds("x <= y"),
            vec![Identifier, BinarySelector, Identifier, EndOfInput]
        );
        assert_eq!(
            kinds("foo <Bar>"),
            vec![Identifier, LessThan, Identifier, GreaterThan, EndOfInput]
        );
    }

    // spec: lex.setter-keyword
    #[test]
    fn setter_keyword() {
        assert_eq!(kinds("x:: 3"), vec![SetterKeyword, Number, EndOfInput]);
    }

    // spec: lex.special-characters
    #[test]
    fn backslash_is_a_binary_selector() {
        assert_eq!(
            kinds("a \\\\ b"),
            vec![Identifier, BinarySelector, Identifier, EndOfInput]
        );
    }

    // spec: lex.setter-keyword
    #[test]
    fn setter_keyword_initializer_without_spaces() {
        assert_eq!(
            kinds("foo::=bar"),
            vec![Identifier, ColonColonEquals, Identifier, EndOfInput]
        );
    }

    // spec: lex.number
    #[test]
    fn decimal_numbers() {
        assert_eq!(
            kinds("0 42 3.14 1.5e-3"),
            vec![Number, Number, Number, Number, EndOfInput,]
        );
    }

    // spec: lex.number.radix
    #[test]
    fn radix_numbers() {
        assert_eq!(kinds("16rFF 2r1010"), vec![Number, Number, EndOfInput]);
    }

    // spec: lex.number.sign
    #[test]
    fn negative_number_versus_subtraction() {
        assert_eq!(kinds("-42"), vec![Number, EndOfInput]);
        assert_eq!(
            kinds("a - 42"),
            vec![Identifier, BinarySelector, Number, EndOfInput]
        );
        assert_eq!(kinds("a -42"), vec![Identifier, Number, EndOfInput]);
    }

    /// A `-` against a digit is a negative number wherever it stands; the
    /// parser splits it where the grammar has a binary message.
    // spec: lex.number.sign
    #[test]
    fn a_minus_against_a_digit_is_a_negative_number() {
        assert_eq!(kinds("a-42"), vec![Identifier, Number, EndOfInput]);
        assert_eq!(kinds("x (*c*)-42"), vec![Identifier, Number, EndOfInput]);
    }

    /// §4.2's `binSel → (specialCharacter | "-") specialCharacter*`.
    // spec: lex.binary-selector
    #[test]
    fn a_minus_only_starts_a_binary_selector() {
        let source = "a +-1 -> b --c";
        let (tokens, diagnostics) = Lexer::new(source).run();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let texts: Vec<&str> = tokens.iter().map(|token| token.span.text(source)).collect();
        assert_eq!(texts, vec!["a", "+", "-1", "->", "b", "-", "-", "c", ""]);
    }

    /// §5.6's asynchronous send token.
    // spec: lex.punctuation
    #[test]
    fn an_eventual_send_is_one_token() {
        assert_eq!(
            kinds("a <-: b"),
            vec![Identifier, EventualSendOperator, Identifier, EndOfInput]
        );
    }

    /// §4.2's `num`: a radix needs an extended digit after its `r`, only an
    /// unsigned number has one, and a fraction needs a digit after its dot.
    // spec: lex.number
    // spec: lex.number.radix
    #[test]
    fn each_part_of_a_number_is_taken_only_when_complete() {
        let texts = |source: &'static str| -> Vec<&'static str> {
            let (tokens, diagnostics) = Lexer::new(source).run();
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            tokens.iter().map(|token| token.span.text(source)).collect()
        };
        assert_eq!(texts("16r"), vec!["16", "r", ""]);
        assert_eq!(texts("-16rFF"), vec!["-16", "rFF", ""]);
        assert_eq!(
            texts("16r-1F 2r1.1 16rff"),
            vec!["16r-1F", "2r1.1", "16", "rff", ""]
        );
        assert_eq!(texts("16r1.a"), vec!["16r1", ".", "a", ""]);
        assert_eq!(
            texts("1.5e-3 1e 2.x"),
            vec!["1.5e-3", "1", "e", "2", ".", "x", ""]
        );
    }

    /// §4.2's `sym → str | kws | binSel | id`, `kws → kw+`.
    // spec: lex.symbol
    #[test]
    fn a_symbols_keywords_each_end_in_a_colon() {
        let texts = |source: &'static str| -> Vec<&'static str> {
            let (tokens, _) = Lexer::new(source).run();
            tokens.iter().map(|token| token.span.text(source)).collect()
        };
        assert_eq!(texts("#at:put"), vec!["#at:", "put", ""]);
        assert_eq!(texts("#at:put:"), vec!["#at:put:", ""]);
        assert_eq!(texts("#-> #+-"), vec!["#->", "#+", "-", ""]);
    }

    // spec: lex.string.single-quoted
    #[test]
    fn strings_with_escaped_quote() {
        let (tokens, diagnostics) = Lexer::new("'it''s'").run();
        assert!(diagnostics.is_empty());
        assert_eq!(tokens[0].kind, String);
        assert_eq!(tokens[0].span.text("'it''s'"), "'it''s'");
    }

    // spec: lex.symbol
    #[test]
    fn symbols() {
        assert_eq!(
            kinds("#foo #at:put: #+ #'hi there'"),
            vec![Symbol, Symbol, Symbol, Symbol, EndOfInput,]
        );
    }

    // spec: lex.literal-array-refused
    #[test]
    fn a_literal_array_is_refused_by_name() {
        let (_, diagnostics) = Lexer::new("#(1)").run();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(
            diagnostics[0]
                .message
                .starts_with("`#( ... )` is not Newspeak syntax"),
            "{}",
            diagnostics[0].message
        );
    }

    // spec: lex.string.double-quoted
    #[test]
    fn double_quoted_strings() {
        assert_eq!(
            kinds(r#""a" "'" " " "abc" """#),
            vec![String, String, String, String, String, EndOfInput]
        );
    }

    /// The one-character form takes whatever single character stands between
    /// the quotes, `"` included, so the corpus's `"""` is the string `"`.
    // spec: lex.string.one-character
    #[test]
    fn a_one_character_double_quoted_string_may_be_a_double_quote() {
        let source = r#"(char: """) x"#;
        let (tokens, diagnostics) = Lexer::new(source).run();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let strings: Vec<_> = tokens
            .iter()
            .filter(|token| token.kind == String)
            .map(|token| token.span.text(source))
            .collect();
        assert_eq!(strings, vec![r#"""""#]);
        assert_eq!(
            kinds(r#"(char: """) x"#),
            vec![
                LeftParenthesis,
                Keyword,
                String,
                RightParenthesis,
                Identifier,
                EndOfInput
            ]
        );
    }

    // spec: lex.string.double-quoted
    #[test]
    fn unterminated_double_quoted_string_is_diagnostic() {
        let (_tokens, diagnostics) = Lexer::new("\"oops").run();
        assert_eq!(diagnostics.len(), 1);
    }

    // spec: lex.string.single-quoted
    #[test]
    fn unterminated_string_is_diagnostic() {
        let (_tokens, diagnostics) = Lexer::new("'oops").run();
        assert_eq!(diagnostics.len(), 1);
    }

    /// Every comment of `source` as its kind and the text its span covers.
    fn comments_of(source: &str) -> Vec<(CommentKind, &str)> {
        Lexer::new(source)
            .run_keeping_comments()
            .comments
            .iter()
            .map(|comment| (comment.kind, comment.span.text(source)))
            .collect()
    }

    // spec: lex.comment.line
    // spec: lex.comment.block
    // spec: lex.whitespace
    #[test]
    fn line_and_block_comments_are_trivia() {
        assert_eq!(
            kinds("a (* comment *) b"),
            vec![Identifier, Identifier, EndOfInput]
        );
        assert_eq!(
            kinds("a /// comment\nb"),
            vec![Identifier, Identifier, EndOfInput]
        );
        assert_eq!(kinds("a /// comment"), vec![Identifier, EndOfInput]);
        assert_eq!(kinds("x///y"), vec![Identifier, EndOfInput]);
        assert_eq!(
            kinds("a //// four\nb"),
            vec![Identifier, Identifier, EndOfInput]
        );
    }

    /// §4.2's `binSel` would take `+///` whole; ns-next ends every selector
    /// before `///`, the rule `lex.binary-selector` of the specification
    /// (`docs/spec/`), whose appendix records the difference.
    // spec: lex.comment.line
    // spec: lex.binary-selector
    #[test]
    fn a_line_comment_ends_every_selector_before_it() {
        let texts = |source: &'static str| -> Vec<(crate::token::TokenKind, &'static str)> {
            let (tokens, diagnostics) = Lexer::new(source).run();
            assert!(diagnostics.is_empty(), "{source}: {diagnostics:?}");
            tokens
                .into_iter()
                .map(|token| (token.kind, token.span.text(source)))
                .collect()
        };
        assert_eq!(
            texts("a +/// x\nb"),
            vec![
                (Identifier, "a"),
                (BinarySelector, "+"),
                (Identifier, "b"),
                (EndOfInput, "")
            ]
        );
        assert_eq!(
            texts("a // b"),
            vec![
                (Identifier, "a"),
                (BinarySelector, "//"),
                (Identifier, "b"),
                (EndOfInput, "")
            ]
        );
        assert_eq!(texts("#+/// x"), vec![(Symbol, "#+"), (EndOfInput, "")]);
        assert_eq!(
            texts("| a |/// x"),
            vec![(Bar, "|"), (Identifier, "a"), (Bar, "|"), (EndOfInput, "")]
        );
        assert_eq!(
            texts("a =/// x"),
            vec![(Identifier, "a"), (Equals, "="), (EndOfInput, "")]
        );
        assert_eq!(
            texts("a </// x"),
            vec![(Identifier, "a"), (LessThan, "<"), (EndOfInput, "")]
        );
        assert_eq!(
            texts("a >/// x"),
            vec![(Identifier, "a"), (GreaterThan, ">"), (EndOfInput, "")]
        );
        assert_eq!(
            texts("a -/// x"),
            vec![(Identifier, "a"), (BinarySelector, "-"), (EndOfInput, "")]
        );
    }

    // spec: lex.symbol
    #[test]
    fn a_hash_before_a_line_comment_names_no_symbol() {
        let (_, diagnostics) = Lexer::new("#/// x").run();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].message, "expected symbol after `#`");
    }

    /// Neither form sees the other, and a string sees neither.
    // spec: lex.comment.line
    // spec: lex.comment.block
    #[test]
    fn the_two_comment_forms_do_not_see_each_other() {
        assert_eq!(
            comments_of("(* a /// b *) c /// d (* e\nf"),
            [
                (CommentKind::Block, "(* a /// b *)"),
                (CommentKind::Line, "/// d (* e")
            ]
        );
        assert_eq!(
            kinds("'///' \"///\" '(*'"),
            vec![String, String, String, EndOfInput]
        );
    }

    #[test]
    fn consecutive_line_comments_are_one_block() {
        let source = "    /// one\n    /// two\n    run";
        assert_eq!(
            comments_of(source),
            [(CommentKind::Line, "/// one\n    /// two")]
        );
    }

    #[test]
    fn a_blank_line_or_code_starts_a_new_block() {
        assert_eq!(
            comments_of("/// one\n\n/// two"),
            [
                (CommentKind::Line, "/// one"),
                (CommentKind::Line, "/// two")
            ]
        );
        assert_eq!(
            comments_of("/// one\nx\n/// two"),
            [
                (CommentKind::Line, "/// one"),
                (CommentKind::Line, "/// two")
            ]
        );
        assert_eq!(
            comments_of("(* one *)\n/// two"),
            [
                (CommentKind::Block, "(* one *)"),
                (CommentKind::Line, "/// two")
            ]
        );
    }

    #[test]
    fn a_trailing_line_comment_starts_no_block_with_the_next_line() {
        assert_eq!(
            comments_of("x foo. /// remark\n/// documentation\nbar"),
            [
                (CommentKind::Line, "/// remark"),
                (CommentKind::Line, "/// documentation")
            ]
        );
    }

    #[test]
    fn a_crlf_line_comment_block_is_one_comment() {
        assert_eq!(
            comments_of("/// one\r\n/// two\r\nx"),
            [(CommentKind::Line, "/// one\r\n/// two")]
        );
    }

    // spec: lex.comment.block
    #[test]
    fn comments_nest() {
        assert_eq!(
            kinds("a (* x (* y *) z *) b"),
            vec![Identifier, Identifier, EndOfInput]
        );
    }

    // spec: lex.comment.block
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

    // spec: lex.longest-token
    #[test]
    fn token_stream_ends_with_end_of_input() {
        let tokens = crate::lex("foo").unwrap();
        assert_eq!(tokens.last().unwrap().kind, EndOfInput);
    }

    // spec: lex.source
    #[test]
    fn stray_non_ascii_character_is_one_codepoint_aligned_diagnostic() {
        let source = "é";
        let (_tokens, diagnostics) = Lexer::new(source).run();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        // Span is codepoint-aligned: `text` must not panic and covers the character.
        assert_eq!(diagnostics[0].span.text(source), "é");
    }

    // spec: lex.source
    // spec: lex.string.double-quoted
    #[test]
    fn a_double_quoted_string_may_hold_a_multibyte_character() {
        for source in ["\"é\"", "\"éé\""] {
            let (tokens, diagnostics) = Lexer::new(source).run();
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let strings: Vec<_> = tokens.iter().filter(|token| token.kind == String).collect();
            assert_eq!(strings.len(), 1);
            assert_eq!(strings[0].span.text(source), source);
        }
    }

    // spec: lex.comment.metadata
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
