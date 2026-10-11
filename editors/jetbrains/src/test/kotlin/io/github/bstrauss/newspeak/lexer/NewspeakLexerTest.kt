package io.github.bstrauss.newspeak.lexer

import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.BAR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.BINARY_SELECTOR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.CARET
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.CLASS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.COLON_COLON_EQUALS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.COMMENT
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.DOT
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.DOUBLE_BAR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.EQUALS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.EVENTUAL_SEND_OPERATOR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.GREATER_THAN
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.IDENTIFIER
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.KEYWORD
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.LEFT_PARENTHESIS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.LESS_THAN
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.LINE_COMMENT
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.MIXIN_OPERATOR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.NUMBER
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.PRIVATE
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.PUBLIC
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.RESERVED_WORD
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.RIGHT_PARENTHESIS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.SETTER_KEYWORD
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.STRING
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.SYMBOL
import org.junit.Assert.assertEquals
import org.junit.Test

/** Every token of [text] as (type, text), whitespace included. */
fun lexAll(text: String): List<Pair<IElementType, String>> {
    val lexer = NewspeakLexer()
    lexer.start(text, 0, text.length, 0)
    val tokens = mutableListOf<Pair<IElementType, String>>()
    while (true) {
        val type = lexer.tokenType ?: break
        tokens.add(type to text.substring(lexer.tokenStart, lexer.tokenEnd))
        lexer.advance()
    }
    return tokens
}

/** The significant token types of [text]: no whitespace. */
fun kinds(text: String): List<IElementType> =
    lexAll(text).map { (type, _) -> type }.filter { type -> type != TokenType.WHITE_SPACE }

/** The tokens of [text] the Rust lexer also emits, as (type, text). */
fun significant(text: String): List<Pair<IElementType, String>> =
    lexAll(text).filter { (type, _) -> type != TokenType.WHITE_SPACE && type != COMMENT && type != LINE_COMMENT }

class NewspeakLexerTest {
    @Test fun identifiersAndKeywords() =
        assertEquals(listOf(IDENTIFIER, KEYWORD, IDENTIFIER), kinds("foo bar: baz"))

    @Test fun leadingUnderscoreIdentifier() = assertEquals(listOf(IDENTIFIER), kinds("_x"))

    @Test fun reservedWordsOfTheLexer() =
        assertEquals(listOf(CLASS, PUBLIC, PRIVATE), kinds("class public private"))

    @Test fun reservedWordsOfTheSpecification() =
        assertEquals(List(6) { RESERVED_WORD }, kinds("self super outer true false nil"))

    @Test fun punctuationVersusBinarySelector() =
        assertEquals(listOf(BAR, DOUBLE_BAR, EQUALS, COLON_COLON_EQUALS, CARET, DOT), kinds("| || = ::= ^ ."))

    @Test fun smalltalkAssignmentIsABadCharacterPair() {
        assertEquals(listOf(IDENTIFIER, TokenType.BAD_CHARACTER, NUMBER), kinds("x := 1"))
        assertEquals(TokenType.BAD_CHARACTER to ":=", lexAll("x:=1")[1])
    }

    @Test fun binarySelectors() =
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, IDENTIFIER, BINARY_SELECTOR, IDENTIFIER), kinds("a + b <= c"))

    @Test fun mixinOperatorIsOneTokenAndLeavesLessThanAlone() {
        assertEquals(listOf(IDENTIFIER, MIXIN_OPERATOR, IDENTIFIER), kinds("x <: y"))
        assertEquals(listOf(IDENTIFIER, MIXIN_OPERATOR, IDENTIFIER), kinds("Object<:Value"))
        assertEquals(listOf(IDENTIFIER, LESS_THAN, IDENTIFIER), kinds("x < y"))
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, IDENTIFIER), kinds("x <= y"))
        assertEquals(listOf(IDENTIFIER, LESS_THAN, IDENTIFIER, GREATER_THAN), kinds("foo <Bar>"))
    }

    @Test fun setterKeyword() = assertEquals(listOf(SETTER_KEYWORD, NUMBER), kinds("x:: 3"))

    @Test fun backslashIsABinarySelector() =
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, IDENTIFIER), kinds("a \\\\ b"))

    @Test fun setterKeywordInitializerWithoutSpaces() =
        assertEquals(listOf(IDENTIFIER, COLON_COLON_EQUALS, IDENTIFIER), kinds("foo::=bar"))

    @Test fun decimalNumbers() = assertEquals(List(4) { NUMBER }, kinds("0 42 3.14 1.5e-3"))

    @Test fun radixNumbers() = assertEquals(listOf(NUMBER, NUMBER), kinds("16rFF 2r1010"))

    /**
     * A `-` against a digit is a negative number wherever it stands; the
     * parser splits it into a selector where the grammar has a binary message
     * (`a -42` subtracts), as the Rust lexer leaves it to.
     */
    @Test fun aMinusAgainstADigitIsANegativeNumber() {
        assertEquals(listOf(NUMBER), kinds("-42"))
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, NUMBER), kinds("a - 42"))
        assertEquals(listOf(IDENTIFIER, NUMBER), kinds("a -42"))
        assertEquals(listOf(IDENTIFIER, NUMBER), kinds("a-42"))
        assertEquals(listOf(IDENTIFIER, COMMENT, NUMBER), kinds("x (*c*)-42"))
    }

    /** §4.2's `binSel`: `-` starts a binary selector and continues none. */
    @Test fun aMinusOnlyStartsABinarySelector() {
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, NUMBER), kinds("a +-1"))
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, IDENTIFIER), kinds("a -> b"))
    }

    @Test fun anEventualSendIsOneToken() =
        assertEquals(listOf(IDENTIFIER, EVENTUAL_SEND_OPERATOR, IDENTIFIER), kinds("a <-: b"))

    /** A radix is taken only with a digit after the `r`, and never after a `-`. */
    @Test fun aRadixNeedsItsDigits() {
        assertEquals(listOf(NUMBER, IDENTIFIER), kinds("16r"))
        assertEquals(listOf(NUMBER, IDENTIFIER), kinds("-16rFF"))
        assertEquals(listOf(NUMBER), kinds("16r-1F"))
    }

    /** §4.2's `kws`: every keyword of a symbol ends in its colon. */
    @Test fun aSymbolsKeywordsEachEndInAColon() =
        assertEquals(listOf(SYMBOL to "#at:", IDENTIFIER to "put"), lexAll("#at:put"))

    @Test fun stringsWithEscapedQuote() = assertEquals(listOf(STRING to "'it''s'"), lexAll("'it''s'"))

    @Test fun symbols() = assertEquals(List(4) { SYMBOL }, kinds("#foo #at:put: #+ #'hi there'"))

    /** Smalltalk's literal array is no Newspeak syntax: the Rust lexer reports its `#`. */
    @Test fun aLiteralArraysHashIsABadCharacter() =
        assertEquals(listOf(TokenType.BAD_CHARACTER, LEFT_PARENTHESIS, NUMBER, RIGHT_PARENTHESIS), kinds("#(1)"))

    @Test fun aLoneHashIsASymbol() = assertEquals(listOf(SYMBOL to "#"), lexAll("#"))

    @Test fun doubleQuotedStrings() = assertEquals(List(5) { STRING }, kinds("\"a\" \"'\" \" \" \"abc\" \"\""))

    @Test fun aOneCharacterDoubleQuotedStringMayBeADoubleQuote() =
        assertEquals(STRING to "\"\"\"", lexAll("(char: \"\"\") x").first { (type, _) -> type == STRING })

    @Test fun aOneCharacterDoubleQuotedStringMayBeASurrogatePair() =
        assertEquals(listOf(STRING to "\"😀\""), lexAll("\"😀\""))

    @Test fun unterminatedStringsRunToTheEnd() {
        assertEquals(listOf(STRING to "'oops"), lexAll("'oops"))
        assertEquals(listOf(STRING to "\"oops"), lexAll("\"oops"))
    }

    @Test fun commentsAreTokensAndNest() {
        assertEquals(listOf(IDENTIFIER, COMMENT, IDENTIFIER), kinds("a (* comment *) b"))
        assertEquals(COMMENT to "(* x (* y *) z *)", lexAll("a (* x (* y *) z *) b")[2])
    }

    @Test fun anUnterminatedCommentRunsToTheEnd() =
        assertEquals(COMMENT to "(* oops", lexAll("a (* oops")[2])

    @Test fun aLineCommentRunsToTheEndOfItsLine() {
        assertEquals(listOf(IDENTIFIER, LINE_COMMENT, IDENTIFIER), kinds("a /// comment (* opens nothing\nb"))
        assertEquals(LINE_COMMENT to "/// comment", lexAll("a /// comment\nb")[2])
        assertEquals(listOf(IDENTIFIER, LINE_COMMENT), kinds("x///y"))
        assertEquals(listOf(IDENTIFIER, LINE_COMMENT, LINE_COMMENT, IDENTIFIER), kinds("a /// one\n/// two\nb"))
        assertEquals(listOf(IDENTIFIER, COMMENT, IDENTIFIER), kinds("a (* x /// y *) b"))
        assertEquals(listOf(STRING, STRING), kinds("'///' \"///\""))
    }

    /** `a_line_comment_ends_every_selector_before_it` in `lexer.rs`. */
    @Test fun aLineCommentEndsEverySelectorBeforeIt() {
        assertEquals(listOf(IDENTIFIER to "a", BINARY_SELECTOR to "+"), significant("a +/// x"))
        assertEquals(listOf(IDENTIFIER to "a", BINARY_SELECTOR to "-"), significant("a -/// x"))
        assertEquals(listOf(IDENTIFIER to "a", BINARY_SELECTOR to "//", IDENTIFIER to "b"), significant("a // b"))
        assertEquals(listOf(SYMBOL to "#+"), significant("#+/// x"))
        assertEquals(listOf(SYMBOL to "#"), significant("#/// x"))
        assertEquals(listOf(BAR to "|", IDENTIFIER to "a", BAR to "|"), significant("| a |/// x"))
        assertEquals(listOf(IDENTIFIER to "a", EQUALS to "="), significant("a =/// x"))
        assertEquals(listOf(IDENTIFIER to "a", LESS_THAN to "<"), significant("a </// x"))
        assertEquals(listOf(IDENTIFIER to "a", GREATER_THAN to ">"), significant("a >/// x"))
    }

    @Test fun aStrayNonAsciiCharacterIsOneBadCharacter() {
        assertEquals(listOf(TokenType.BAD_CHARACTER to "é"), lexAll("é"))
        assertEquals(listOf(TokenType.BAD_CHARACTER to "😀"), lexAll("😀"))
    }

    @Test fun nonAsciiWhitespaceIsABadCharacter() =
        assertEquals(listOf(IDENTIFIER, TokenType.BAD_CHARACTER, IDENTIFIER), kinds("a b"))

    @Test fun everyCharacterBelongsToAToken() {
        val text = "class M = Object new ( | x ::= 'é'. | ) ( public f = ( ^x := 😀 (* open"
        assertEquals(text, lexAll(text).joinToString("") { (_, tokenText) -> tokenText })
    }
}
