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
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.GREATER_THAN
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.IDENTIFIER
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.KEYWORD
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.LEFT_PARENTHESIS
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.LESS_THAN
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.MIXIN_OPERATOR
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.NUMBER
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.POUND
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

    @Test fun negativeNumberVersusSubtraction() {
        assertEquals(listOf(NUMBER), kinds("-42"))
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, NUMBER), kinds("a - 42"))
        assertEquals(listOf(IDENTIFIER, NUMBER), kinds("a -42"))
        assertEquals(listOf(IDENTIFIER, BINARY_SELECTOR, NUMBER), kinds("a-42"))
    }

    @Test fun aCommentIsNotWhitespaceForTheNegativeLiteralRule() {
        assertEquals(listOf(IDENTIFIER, COMMENT, BINARY_SELECTOR, NUMBER), kinds("x (*c*)-42"))
        assertEquals(listOf(IDENTIFIER, COMMENT, NUMBER), kinds("x (*c*) -42"))
    }

    @Test fun stringsWithEscapedQuote() = assertEquals(listOf(STRING to "'it''s'"), lexAll("'it''s'"))

    @Test fun symbols() = assertEquals(List(4) { SYMBOL }, kinds("#foo #at:put: #+ #'hi there'"))

    @Test fun hashParenthesisIsPoundThenLeftParenthesis() =
        assertEquals(listOf(POUND, LEFT_PARENTHESIS, NUMBER, RIGHT_PARENTHESIS), kinds("#(1)"))

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
