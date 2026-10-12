package io.github.bstrauss.newspeak.editor

import com.intellij.lang.ASTNode
import com.intellij.lang.folding.FoldingBuilderEx
import com.intellij.lang.folding.FoldingDescriptor
import com.intellij.openapi.editor.Document
import com.intellij.openapi.editor.FoldingGroup
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.util.TextRange
import com.intellij.psi.PsiElement
import com.intellij.psi.TokenType
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * Shows a method whose body is one line on the line of its header, as
 * IntelliJ's "One-line methods" does for Java: the formatter writes every
 * body on lines of its own (`docs/design/formatting.md`, rule 10), and
 *
 * ```
 *   public size = (
 *     ^size_.
 *   )
 * ```
 *
 * reads as `public size = ( ^size_. )`. Two folds, the line break before the
 * body and the one after it, each shown as a space and opened together;
 * collapsed by default unless *Settings | Editor | General | Code Folding |
 * Newspeak | One-line methods* is off. The file is never changed.
 *
 * The method is found in the tokens, since the plugin has no Newspeak parser
 * (`NewspeakParserDefinition`): `= (` ending a line, one line of body, and a
 * `)` alone on the next. A class header has the same shape, but its `)` is
 * followed by the instance side's `(`, and a parenthesised slot initializer
 * by its `.`, so a `)` that does not end its line is no method's.
 */
class NewspeakOneLineMethodFolding : FoldingBuilderEx(), DumbAware {
    override fun buildFoldRegions(root: PsiElement, document: Document, quick: Boolean): Array<FoldingDescriptor> {
        val tokens = root.node.getChildren(null).filter { it.elementType != TokenType.WHITE_SPACE }
        val text = document.charsSequence
        val collapsed = NewspeakSettings.getInstance().state.collapseOneLineMethods
        val descriptors = mutableListOf<FoldingDescriptor>()
        for (index in 0 until tokens.size - 1) {
            if (tokens[index].elementType != Types.EQUALS || tokens[index + 1].elementType != Types.LEFT_PARENTHESIS) continue
            val method = oneLineMethod(tokens, index + 1, text) ?: continue
            val group = FoldingGroup.newGroup("one-line method")
            for (range in listOf(method.beforeBody, method.afterBody)) {
                descriptors += FoldingDescriptor(method.opening, range, group, " ", collapsed, emptySet())
            }
        }
        return descriptors.toTypedArray()
    }

    override fun getPlaceholderText(node: ASTNode): String = " "

    override fun isCollapsedByDefault(node: ASTNode): Boolean = NewspeakSettings.getInstance().state.collapseOneLineMethods

    private class OneLineMethod(val opening: ASTNode, val beforeBody: TextRange, val afterBody: TextRange)

    /** The method whose body opens at `tokens[openingIndex]`, when that body is one line and fits folded. */
    private fun oneLineMethod(tokens: List<ASTNode>, openingIndex: Int, text: CharSequence): OneLineMethod? {
        val opening = tokens[openingIndex]
        val first = tokens.getOrNull(openingIndex + 1) ?: return null
        if (lineBreaks(text, opening.textRange.endOffset, first.startOffset) != 1) return null
        // The body: every token that starts on the first one's line.
        var closingIndex = openingIndex + 1
        while (closingIndex < tokens.size && lineBreaks(text, first.startOffset, tokens[closingIndex].startOffset) == 0) {
            closingIndex++
        }
        val closing = tokens.getOrNull(closingIndex) ?: return null
        val last = tokens[closingIndex - 1]
        val body = tokens.subList(openingIndex + 1, closingIndex)
        val qualifies = closing.elementType == Types.RIGHT_PARENTHESIS &&
            lineBreaks(text, last.textRange.endOffset, closing.startOffset) == 1 &&
            lineBreaks(text, first.startOffset, last.textRange.endOffset) == 0 &&
            // A `///` would swallow the `)` shown after it; a comment alone is no statement.
            body.none { it.elementType == Types.LINE_COMMENT } &&
            body.any { it.elementType != Types.COMMENT } &&
            balanced(body) &&
            endsItsLine(tokens.getOrNull(closingIndex + 1), closing, text)
        if (!qualifies) return null
        val lineStart = lineStartOf(text, opening.startOffset)
        val header = text.subSequence(lineStart, opening.textRange.endOffset)
        val statement = text.subSequence(first.startOffset, last.textRange.endOffset)
        val width = columns(header) + 1 + columns(statement) + 2
        if (width > MAXIMUM_WIDTH) return null
        return OneLineMethod(
            opening,
            TextRange(opening.textRange.endOffset, first.startOffset),
            TextRange(last.textRange.endOffset, closing.startOffset),
        )
    }

    /** Whether every bracket the body opens it also closes, so the `)` after it is the method's. */
    private fun balanced(body: List<ASTNode>): Boolean {
        var depth = 0
        for (token in body) {
            when (token.elementType) {
                Types.LEFT_PARENTHESIS, Types.LEFT_BRACKET, Types.LEFT_BRACE -> depth++
                Types.RIGHT_PARENTHESIS, Types.RIGHT_BRACKET, Types.RIGHT_BRACE -> if (--depth < 0) return false
            }
        }
        return depth == 0
    }

    private fun endsItsLine(next: ASTNode?, closing: ASTNode, text: CharSequence): Boolean =
        next == null || lineBreaks(text, closing.textRange.endOffset, next.startOffset) > 0

    /** A document's line breaks are `\n`, whatever the file's are. */
    private fun lineBreaks(text: CharSequence, start: Int, end: Int): Int = (start until end).count { text[it] == '\n' }

    private fun lineStartOf(text: CharSequence, offset: Int): Int {
        var start = offset
        while (start > 0 && text[start - 1] != '\n') start--
        return start
    }

    /** Columns as the formatter counts them, one per code point. */
    private fun columns(text: CharSequence): Int = Character.codePointCount(text, 0, text.length)

    companion object {
        /** The formatter's width (`docs/design/formatting.md`, rule 1): a folded line never shows longer than it. */
        const val MAXIMUM_WIDTH = 100
    }
}
