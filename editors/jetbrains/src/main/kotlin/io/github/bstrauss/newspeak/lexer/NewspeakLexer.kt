package io.github.bstrauss.newspeak.lexer

import com.intellij.lexer.LexerBase
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * The Newspeak lexer, mirroring `crates/ns-syntax/src/lexer.rs` function by
 * function; read the two side by side. The differences, and why, are in
 * `docs/design/editor-support.md` and the plan that built this. The lexer
 * comparison test holds the two to the same tokens over every module in the
 * repository.
 */
class NewspeakLexer : LexerBase() {
    private var buffer: CharSequence = ""
    private var bufferEnd = 0
    private var tokenStart = 0
    private var tokenEnd = 0
    private var currentType: IElementType? = null
    private var position = 0

    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        bufferEnd = endOffset
        tokenEnd = startOffset
        advance()
    }

    /** Every token boundary is a restart point: the lexer carries no state across tokens. */
    override fun getState(): Int = INITIAL
    override fun getTokenType(): IElementType? = currentType
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = bufferEnd

    override fun advance() {
        tokenStart = tokenEnd
        if (tokenStart >= bufferEnd) {
            currentType = null
            return
        }
        position = tokenStart
        val type = lexOne()
        currentType = type
        tokenEnd = position
    }

    private fun peek(offset: Int = 0): Char? {
        val index = position + offset
        return if (index < bufferEnd) buffer[index] else null
    }

    /** One code point's length in chars at [index], never reaching past the end. */
    private fun codePointLength(index: Int): Int =
        minOf(Character.charCount(Character.codePointAt(buffer, index)), bufferEnd - index)

    private fun single(type: IElementType): IElementType {
        position += 1
        return type
    }

    private fun lexOne(): IElementType {
        val character = buffer[position]
        // `skip_trivia`: whitespace and comments, which the Rust lexer drops.
        if (isAsciiWhitespace(character)) {
            while (peek()?.let { next -> isAsciiWhitespace(next) } == true) position += 1
            return TokenType.WHITE_SPACE
        }
        if (character == '(' && peek(1) == '*') {
            lexBlockComment()
            return Types.COMMENT
        }
        if (character == '/' && startsLineComment(0)) {
            lexLineComment()
            return Types.LINE_COMMENT
        }
        // `lex_one`, arm by arm.
        return when {
            character == '(' -> single(Types.LEFT_PARENTHESIS)
            character == ')' -> single(Types.RIGHT_PARENTHESIS)
            character == '[' -> single(Types.LEFT_BRACKET)
            character == ']' -> single(Types.RIGHT_BRACKET)
            character == '{' -> single(Types.LEFT_BRACE)
            character == '}' -> single(Types.RIGHT_BRACE)
            character == '^' -> single(Types.CARET)
            character == '.' -> single(Types.DOT)
            character == ';' -> single(Types.SEMICOLON)
            // `#(`: the Rust lexer reports Smalltalk's literal array and steps
            // over the `#`, emitting no token.
            character == '#' && peek(1) == '(' -> single(TokenType.BAD_CHARACTER)
            character == '#' -> lexSymbol()
            character == '|' -> when {
                peek(1) == '|' -> {
                    position += 2
                    Types.DOUBLE_BAR
                }
                continuesBinarySelector(1) -> lexOperator()
                else -> single(Types.BAR)
            }
            character == ':' -> when {
                peek(1) == ':' && peek(2) == '=' -> {
                    position += 3
                    Types.COLON_COLON_EQUALS
                }
                peek(1) == '=' -> {
                    // `:=`: the Rust lexer reports it and emits no token.
                    position += 2
                    TokenType.BAD_CHARACTER
                }
                else -> single(Types.COLON)
            }
            character == '_' || isAsciiLetter(character) -> lexWord()
            isAsciiDigit(character) -> lexNumber()
            character == '\'' -> lexString()
            character == '"' -> lexDoubleQuotedString()
            character == '-' && peek(1)?.let { next -> isAsciiDigit(next) } == true -> lexNumber()
            startsBinarySelector(character) -> lexOperator()
            else -> {
                // A character that starts no token: the Rust lexer reports it
                // and steps over one code point, emitting no token.
                position += codePointLength(position)
                TokenType.BAD_CHARACTER
            }
        }
    }

    private fun lexBlockComment() {
        position += 2
        var depth = 1
        while (depth > 0) {
            val current = peek() ?: return
            when {
                current == '(' && peek(1) == '*' -> {
                    position += 2
                    depth += 1
                }
                current == '*' && peek(1) == ')' -> {
                    position += 2
                    depth -= 1
                }
                else -> position += 1
            }
        }
    }

    /** `starts_line_comment`: ns-next's `///`, a deviation from §4.2's `binSel`. */
    private fun startsLineComment(offset: Int): Boolean =
        peek(offset) == '/' && peek(offset + 1) == '/' && peek(offset + 2) == '/'

    /** `continues_binary_selector`: a special character that starts no line comment. */
    private fun continuesBinarySelector(offset: Int): Boolean =
        peek(offset)?.let { next -> next in SPECIAL } == true && !startsLineComment(offset)

    /** `skip_line_comment`, one line per token: the plugin merges nothing. */
    private fun lexLineComment() {
        while (peek()?.let { next -> next != '\n' && next != '\r' } == true) position += 1
    }

    private fun lexWord(): IElementType {
        val start = position
        while (peek()?.let { next -> isWordCharacter(next) } == true) position += 1
        if (peek() == ':') {
            if (peek(1) == '=' || (peek(1) == ':' && peek(2) == '=')) {
                return wordKind(start, position)
            }
            if (peek(1) == ':') {
                position += 2
                return Types.SETTER_KEYWORD
            }
            position += 1
            return Types.KEYWORD
        }
        return wordKind(start, position)
    }

    // `lexWord` is only entered on a word character (see its one call site's
    // guard), so `start < end` always: there is no empty-word case to guard
    // against here.
    private fun wordKind(start: Int, end: Int): IElementType =
        when (buffer.subSequence(start, end).toString()) {
            "class" -> Types.CLASS
            "private" -> Types.PRIVATE
            "public" -> Types.PUBLIC
            "protected" -> Types.PROTECTED
            in Types.RESERVED_WORDS -> Types.RESERVED_WORD
            else -> Types.IDENTIFIER
        }

    private fun lexOperator(): IElementType {
        val character = buffer[position]
        val next = peek(1)
        if (character == '<' && next == ':') {
            position += 2
            return Types.MIXIN_OPERATOR
        }
        if (character == '<' && next == '-' && peek(2) == ':') {
            position += 3
            return Types.EVENTUAL_SEND_OPERATOR
        }
        val runOfTwoPlus = continuesBinarySelector(1)
        if (!runOfTwoPlus) {
            when (character) {
                '=' -> return single(Types.EQUALS)
                '<' -> return single(Types.LESS_THAN)
                '>' -> return single(Types.GREATER_THAN)
            }
        }
        position += 1
        while (continuesBinarySelector(0)) position += 1
        return Types.BINARY_SELECTOR
    }

    private fun lexNumber(): IElementType {
        val negative = peek() == '-'
        if (negative) position += 1
        eatDigits()
        val radix = !negative && peek() == 'r' &&
            (isExtendedDigit(peek(1)) || (peek(1) == '-' && isExtendedDigit(peek(2))))
        if (radix) {
            position += 1
            if (peek() == '-') position += 1
            eatExtendedDigits()
            if (peek() == '.' && isExtendedDigit(peek(1))) {
                position += 1
                eatExtendedDigits()
            }
        } else if (peek() == '.' && peek(1)?.let { next -> isAsciiDigit(next) } == true) {
            position += 1
            eatDigits()
        }
        if (peek() == 'e' && exponentFollows()) {
            position += 1
            if (peek() == '-') position += 1
            eatDigits()
        }
        return Types.NUMBER
    }

    private fun exponentFollows(): Boolean {
        var offset = 1
        if (peek(offset) == '-') offset += 1
        return peek(offset)?.let { next -> isAsciiDigit(next) } == true
    }

    private fun eatDigits() {
        while (peek()?.let { next -> isAsciiDigit(next) } == true) position += 1
    }

    private fun eatExtendedDigits() {
        while (isExtendedDigit(peek())) position += 1
    }

    /** A single-quoted string, `''` inside it a quote; unterminated, it runs to the end. */
    private fun lexString(): IElementType {
        position += 1
        while (true) {
            when (peek()) {
                null -> break
                '\'' -> if (peek(1) == '\'') {
                    position += 2
                } else {
                    position += 1
                    break
                }
                else -> position += 1
            }
        }
        return Types.STRING
    }

    private fun lexSymbol(): IElementType {
        position += 1
        val next = peek()
        when {
            next == '\'' -> lexString()
            next != null && (next == '_' || isAsciiLetter(next)) -> {
                eatWord()
                while (peek() == ':') {
                    position += 1
                    val afterColon = position
                    eatWord()
                    if (position == afterColon) break
                    if (peek() != ':') {
                        position = afterColon
                        break
                    }
                }
            }
            next != null && startsBinarySelector(next) && !startsLineComment(0) -> {
                position += 1
                while (continuesBinarySelector(0)) position += 1
            }
            // A `#` followed by nothing a symbol can be: the Rust lexer reports
            // it and still emits the `#` as a symbol.
        }
        return Types.SYMBOL
    }

    /**
     * A double-quoted string (§5.1.5), which has no escape: any one character
     * between two quotes, `"` included, else everything up to the next `"`.
     */
    private fun lexDoubleQuotedString(): IElementType {
        position += 1
        if (position < bufferEnd) {
            val after = position + codePointLength(position)
            if (after < bufferEnd && buffer[after] == '"') {
                position = after + 1
                return Types.STRING
            }
        }
        while (position < bufferEnd && buffer[position] != '"') position += 1
        if (position < bufferEnd) position += 1
        return Types.STRING
    }

    private fun eatWord() {
        while (peek()?.let { next -> isWordCharacter(next) } == true) position += 1
    }

    companion object {
        const val INITIAL = 0

        /** The Rust lexer's `SPECIAL`, §4.2's `specialCharacter`: `-` only starts a binary selector. */
        private const val SPECIAL = "+*/\\~<>=&|@%,?!"

        private fun startsBinarySelector(character: Char) = character == '-' || character in SPECIAL

        /** §4.2's `extendedDigits`: a digit or an uppercase letter. */
        private fun isExtendedDigit(character: Char?) =
            character != null && (isAsciiDigit(character) || character in 'A'..'Z')

        /** Rust's `u8::is_ascii_whitespace`: no vertical tab, and nothing beyond ASCII. */
        private fun isAsciiWhitespace(character: Char) =
            character == ' ' || character == '\t' || character == '\n' || character == '\u000C' || character == '\r'

        private fun isAsciiLetter(character: Char) = character in 'a'..'z' || character in 'A'..'Z'
        private fun isAsciiDigit(character: Char) = character in '0'..'9'
        private fun isAsciiLetterOrDigit(character: Char) = isAsciiLetter(character) || isAsciiDigit(character)
        private fun isWordCharacter(character: Char) = character == '_' || isAsciiLetterOrDigit(character)
    }
}
