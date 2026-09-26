package io.github.bstrauss.newspeak.highlighting

import com.intellij.openapi.editor.DefaultLanguageHighlighterColors as Default
import com.intellij.openapi.editor.HighlighterColors
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.colors.TextAttributesKey.createTextAttributesKey

/**
 * Every colour the plugin sets, each derived from a platform default so every
 * colour scheme works without configuration. The first group comes from the
 * lexer; the second from the language server's semantic tokens. A send derives
 * from the instance-method colour rather than the function-call one: every send
 * is a message to an object, and the platform schemes leave function calls in
 * the plain text colour while giving instance methods their own, as TypeScript's
 * method calls have.
 */
object NewspeakColors {
    @JvmField val KEYWORD = createTextAttributesKey("NEWSPEAK_KEYWORD", Default.KEYWORD)
    @JvmField val RESERVED_WORD = createTextAttributesKey("NEWSPEAK_RESERVED_WORD", Default.KEYWORD)
    @JvmField val RETURN = createTextAttributesKey("NEWSPEAK_RETURN", Default.KEYWORD)
    @JvmField val IDENTIFIER = createTextAttributesKey("NEWSPEAK_IDENTIFIER", Default.IDENTIFIER)
    @JvmField val KEYWORD_SELECTOR = createTextAttributesKey("NEWSPEAK_KEYWORD_SELECTOR", Default.INSTANCE_METHOD)
    /**
     * `name::` -- a write to a slot or a local, or a setter send; only the
     * server can tell which, so the lexer leaves it in the plain identifier
     * colour for the semantic one to replace rather than flashing it as a send.
     */
    @JvmField val SETTER_KEYWORD = createTextAttributesKey("NEWSPEAK_SETTER_KEYWORD", Default.IDENTIFIER)
    /** `+`, `-`, `,` ...: always a send or a method's own selector, so a method from the first keystroke. */
    @JvmField val BINARY_SELECTOR = createTextAttributesKey("NEWSPEAK_BINARY_SELECTOR", Default.INSTANCE_METHOD)
    /**
     * `<` and `>` open and close type annotations and pattern literals as well
     * as being binary selectors; the lexer leaves them neutral and the server
     * colours the ones that are sends.
     */
    @JvmField val ANGLE_BRACKET = createTextAttributesKey("NEWSPEAK_ANGLE_BRACKET", Default.OPERATION_SIGN)
    @JvmField val ASSIGNMENT = createTextAttributesKey("NEWSPEAK_ASSIGNMENT", Default.OPERATION_SIGN)
    @JvmField val NUMBER = createTextAttributesKey("NEWSPEAK_NUMBER", Default.NUMBER)
    @JvmField val STRING = createTextAttributesKey("NEWSPEAK_STRING", Default.STRING)
    @JvmField val SYMBOL = createTextAttributesKey("NEWSPEAK_SYMBOL", Default.CONSTANT)
    @JvmField val COMMENT = createTextAttributesKey("NEWSPEAK_COMMENT", Default.BLOCK_COMMENT)
    @JvmField val PARENTHESES = createTextAttributesKey("NEWSPEAK_PARENTHESES", Default.PARENTHESES)
    @JvmField val BRACKETS = createTextAttributesKey("NEWSPEAK_BRACKETS", Default.BRACKETS)
    @JvmField val BRACES = createTextAttributesKey("NEWSPEAK_BRACES", Default.BRACES)
    @JvmField val SLOT_BARS = createTextAttributesKey("NEWSPEAK_SLOT_BARS", Default.BRACKETS)
    @JvmField val DOT = createTextAttributesKey("NEWSPEAK_DOT", Default.DOT)
    @JvmField val SEMICOLON = createTextAttributesKey("NEWSPEAK_SEMICOLON", Default.SEMICOLON)
    @JvmField val BAD_CHARACTER = createTextAttributesKey("NEWSPEAK_BAD_CHARACTER", HighlighterColors.BAD_CHARACTER)

    @JvmField val PARAMETER = createTextAttributesKey("NEWSPEAK_PARAMETER", Default.PARAMETER)
    @JvmField val LOCAL_VARIABLE = createTextAttributesKey("NEWSPEAK_LOCAL_VARIABLE", Default.LOCAL_VARIABLE)
    @JvmField val SLOT = createTextAttributesKey("NEWSPEAK_SLOT", Default.INSTANCE_FIELD)
    @JvmField val IMMUTABLE_SLOT = createTextAttributesKey("NEWSPEAK_IMMUTABLE_SLOT", Default.INSTANCE_FIELD)
    @JvmField val CLASS_NAME = createTextAttributesKey("NEWSPEAK_CLASS_NAME", Default.CLASS_NAME)
    @JvmField val METHOD_DECLARATION = createTextAttributesKey("NEWSPEAK_METHOD_DECLARATION", Default.FUNCTION_DECLARATION)
    @JvmField val SEND = createTextAttributesKey("NEWSPEAK_SEND", Default.INSTANCE_METHOD)

    val ALL: List<TextAttributesKey> = listOf(
        KEYWORD, RESERVED_WORD, RETURN, IDENTIFIER, KEYWORD_SELECTOR, SETTER_KEYWORD, BINARY_SELECTOR, ANGLE_BRACKET, ASSIGNMENT,
        NUMBER, STRING, SYMBOL, COMMENT, PARENTHESES, BRACKETS, BRACES, SLOT_BARS, DOT, SEMICOLON,
        BAD_CHARACTER, PARAMETER, LOCAL_VARIABLE, SLOT, IMMUTABLE_SLOT, CLASS_NAME, METHOD_DECLARATION, SEND,
    )
}
