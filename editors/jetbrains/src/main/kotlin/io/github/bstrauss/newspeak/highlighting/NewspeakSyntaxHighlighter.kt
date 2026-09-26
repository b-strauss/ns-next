package io.github.bstrauss.newspeak.highlighting

import com.intellij.lexer.Lexer
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.openapi.fileTypes.SyntaxHighlighterFactory
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakLexer
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

class NewspeakSyntaxHighlighter : SyntaxHighlighterBase() {
    override fun getHighlightingLexer(): Lexer = NewspeakLexer()

    override fun getTokenHighlights(tokenType: IElementType?): Array<TextAttributesKey> =
        pack(ATTRIBUTES[tokenType])

    private companion object {
        val ATTRIBUTES: Map<IElementType, TextAttributesKey> = mapOf(
            Types.CLASS to NewspeakColors.KEYWORD,
            Types.PUBLIC to NewspeakColors.KEYWORD,
            Types.PRIVATE to NewspeakColors.KEYWORD,
            Types.PROTECTED to NewspeakColors.KEYWORD,
            Types.RESERVED_WORD to NewspeakColors.RESERVED_WORD,
            Types.CARET to NewspeakColors.RETURN,
            Types.IDENTIFIER to NewspeakColors.IDENTIFIER,
            Types.KEYWORD to NewspeakColors.KEYWORD_SELECTOR,
            Types.SETTER_KEYWORD to NewspeakColors.SETTER_KEYWORD,
            Types.BINARY_SELECTOR to NewspeakColors.BINARY_SELECTOR,
            Types.LESS_THAN to NewspeakColors.ANGLE_BRACKET,
            Types.GREATER_THAN to NewspeakColors.ANGLE_BRACKET,
            Types.EQUALS to NewspeakColors.ASSIGNMENT,
            Types.COLON_COLON_EQUALS to NewspeakColors.ASSIGNMENT,
            Types.MIXIN_OPERATOR to NewspeakColors.ASSIGNMENT,
            Types.NUMBER to NewspeakColors.NUMBER,
            Types.STRING to NewspeakColors.STRING,
            Types.SYMBOL to NewspeakColors.SYMBOL,
            Types.POUND to NewspeakColors.SYMBOL,
            Types.COMMENT to NewspeakColors.COMMENT,
            Types.LEFT_PARENTHESIS to NewspeakColors.PARENTHESES,
            Types.RIGHT_PARENTHESIS to NewspeakColors.PARENTHESES,
            Types.LEFT_BRACKET to NewspeakColors.BRACKETS,
            Types.RIGHT_BRACKET to NewspeakColors.BRACKETS,
            Types.LEFT_BRACE to NewspeakColors.BRACES,
            Types.RIGHT_BRACE to NewspeakColors.BRACES,
            Types.BAR to NewspeakColors.SLOT_BARS,
            Types.DOUBLE_BAR to NewspeakColors.SLOT_BARS,
            Types.DOT to NewspeakColors.DOT,
            Types.SEMICOLON to NewspeakColors.SEMICOLON,
            TokenType.BAD_CHARACTER to NewspeakColors.BAD_CHARACTER,
        )
    }
}

class NewspeakSyntaxHighlighterFactory : SyntaxHighlighterFactory() {
    override fun getSyntaxHighlighter(project: Project?, virtualFile: VirtualFile?): SyntaxHighlighter =
        NewspeakSyntaxHighlighter()
}
