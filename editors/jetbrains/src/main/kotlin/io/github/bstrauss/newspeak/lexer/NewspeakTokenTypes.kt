package io.github.bstrauss.newspeak.lexer

import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.NewspeakLanguage

/**
 * A token of the Newspeak lexer. [rustKind] is the `ns_syntax::TokenKind`
 * variant this token corresponds to -- what the lexer comparison checks --
 * or null for a token the Rust lexer does not produce.
 */
class NewspeakTokenType(debugName: String, val rustKind: String?) : IElementType(debugName, NewspeakLanguage)

object NewspeakTokenTypes {
    private fun mirrored(kind: String) = NewspeakTokenType(kind, kind)

    @JvmField val IDENTIFIER = mirrored("Identifier")
    /** An identifier the specification reserves (§4.1); the Rust lexer calls it an `Identifier`. */
    @JvmField val RESERVED_WORD = NewspeakTokenType("ReservedWord", "Identifier")
    @JvmField val KEYWORD = mirrored("Keyword")
    @JvmField val SETTER_KEYWORD = mirrored("SetterKeyword")
    @JvmField val BINARY_SELECTOR = mirrored("BinarySelector")
    @JvmField val CLASS = mirrored("Class")
    @JvmField val PRIVATE = mirrored("Private")
    @JvmField val PUBLIC = mirrored("Public")
    @JvmField val PROTECTED = mirrored("Protected")
    @JvmField val LEFT_PARENTHESIS = mirrored("LeftParenthesis")
    @JvmField val RIGHT_PARENTHESIS = mirrored("RightParenthesis")
    @JvmField val LEFT_BRACKET = mirrored("LeftBracket")
    @JvmField val RIGHT_BRACKET = mirrored("RightBracket")
    @JvmField val LEFT_BRACE = mirrored("LeftBrace")
    @JvmField val RIGHT_BRACE = mirrored("RightBrace")
    @JvmField val BAR = mirrored("Bar")
    @JvmField val DOUBLE_BAR = mirrored("DoubleBar")
    @JvmField val CARET = mirrored("Caret")
    @JvmField val DOT = mirrored("Dot")
    @JvmField val SEMICOLON = mirrored("Semicolon")
    @JvmField val COLON = mirrored("Colon")
    @JvmField val EQUALS = mirrored("Equals")
    @JvmField val COLON_COLON_EQUALS = mirrored("ColonColonEquals")
    @JvmField val POUND = mirrored("Pound")
    @JvmField val LESS_THAN = mirrored("LessThan")
    @JvmField val GREATER_THAN = mirrored("GreaterThan")
    @JvmField val MIXIN_OPERATOR = mirrored("MixinOperator")
    @JvmField val NUMBER = mirrored("Number")
    @JvmField val STRING = mirrored("String")
    @JvmField val SYMBOL = mirrored("Symbol")
    /** A `(* ... *)` comment, nested ones included. The Rust lexer skips comments. */
    @JvmField val COMMENT = NewspeakTokenType("Comment", null)

    /** `newspeak-spec.pdf` §4.1, as `crates/ns-syntax/src/parser/mod.rs`'s `RESERVED_WORDS`. */
    val RESERVED_WORDS: Set<String> = setOf("self", "super", "outer", "true", "false", "nil")
}
