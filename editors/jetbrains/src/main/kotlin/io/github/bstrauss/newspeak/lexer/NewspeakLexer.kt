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
    private var stateAtTokenStart = INITIAL
    /** Whether the previous significant token ends a value: the Rust lexer's `previous_ends_value`. */
    private var previousEndsValue = false
    private var position = 0

    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        bufferEnd = endOffset
        tokenEnd = startOffset
        previousEndsValue = initialState == PREVIOUS_ENDS_VALUE
        advance()
    }

    override fun getState(): Int = stateAtTokenStart
    override fun getTokenType(): IElementType? = currentType
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = bufferEnd

    override fun advance() {
        tokenStart = tokenEnd
        stateAtTokenStart = if (previousEndsValue) PREVIOUS_ENDS_VALUE else INITIAL
        if (tokenStart >= bufferEnd) {
            currentType = null
            return
        }
        position = tokenStart
        val type = lexOne()
        currentType = type
        tokenEnd = position
        if (type != TokenType.WHITE_SPACE && type != Types.COMMENT && type != TokenType.BAD_CHARACTER) {
            previousEndsValue = type in ENDS_VALUE
        }
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
            character == '#' && peek(1) == '(' -> single(Types.POUND)
            character == '#' -> lexSymbol()
            character == '|' -> when {
                peek(1) == '|' -> {
                    position += 2
                    Types.DOUBLE_BAR
                }
                peek(1)?.let { next -> next in SPECIAL } == true -> lexOperator()
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
            character == '-' &&
                peek(1)?.let { next -> isAsciiDigit(next) } == true &&
                (!previousEndsValue || precededByWhitespace()) -> lexNumber()
            character in SPECIAL -> lexOperator()
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

    private fun wordKind(start: Int, end: Int): IElementType =
        when (val word = buffer.subSequence(start, end).toString()) {
            "class" -> Types.CLASS
            "private" -> Types.PRIVATE
            "public" -> Types.PUBLIC
            "protected" -> Types.PROTECTED
            in Types.RESERVED_WORDS -> Types.RESERVED_WORD
            else -> if (word.isEmpty()) TokenType.BAD_CHARACTER else Types.IDENTIFIER
        }

    private fun lexOperator(): IElementType {
        val character = buffer[position]
        val next = peek(1)
        if (character == '<' && next == ':') {
            position += 2
            return Types.MIXIN_OPERATOR
        }
        val runOfTwoPlus = next != null && next in SPECIAL
        if (!runOfTwoPlus) {
            when (character) {
                '=' -> return single(Types.EQUALS)
                '<' -> return single(Types.LESS_THAN)
                '>' -> return single(Types.GREATER_THAN)
            }
        }
        while (peek()?.let { following -> following in SPECIAL } == true) position += 1
        return Types.BINARY_SELECTOR
    }

    private fun lexNumber(): IElementType {
        if (peek() == '-') position += 1
        eatDigits()
        if (peek() == 'r') {
            position += 1
            if (peek() == '-') position += 1
            eatExtendedDigits()
            if (peek() == '.' && peek(1)?.let { next -> isAsciiLetterOrDigit(next) } == true) {
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
        while (peek()?.let { next -> isAsciiDigit(next) || next in 'A'..'Z' } == true) position += 1
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
                while (peek()?.let { following -> isWordCharacter(following) } == true) position += 1
                while (peek() == ':') {
                    position += 1
                    while (peek()?.let { following -> isWordCharacter(following) } == true) position += 1
                }
            }
            next != null && next in SPECIAL -> {
                while (peek()?.let { following -> following in SPECIAL } == true) position += 1
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

    /** The Rust lexer's `preceded_by_whitespace`: a comment does not count. */
    private fun precededByWhitespace(): Boolean = position == 0 || isAsciiWhitespace(buffer[position - 1])

    companion object {
        const val INITIAL = 0
        const val PREVIOUS_ENDS_VALUE = 1

        /** The Rust lexer's `SPECIAL`. */
        private const val SPECIAL = "+-*/\\~<>=&|@%,?!"

        private val ENDS_VALUE: Set<IElementType> = setOf(
            Types.IDENTIFIER, Types.RESERVED_WORD, Types.NUMBER, Types.STRING, Types.SYMBOL,
            Types.RIGHT_PARENTHESIS, Types.RIGHT_BRACKET, Types.RIGHT_BRACE,
        )

        /** Rust's `u8::is_ascii_whitespace`: no vertical tab, and nothing beyond ASCII. */
        private fun isAsciiWhitespace(character: Char) =
            character == ' ' || character == '\t' || character == '\n' || character == '\u000C' || character == '\r'

        private fun isAsciiLetter(character: Char) = character in 'a'..'z' || character in 'A'..'Z'
        private fun isAsciiDigit(character: Char) = character in '0'..'9'
        private fun isAsciiLetterOrDigit(character: Char) = isAsciiLetter(character) || isAsciiDigit(character)
        private fun isWordCharacter(character: Char) = character == '_' || isAsciiLetterOrDigit(character)
    }
}
