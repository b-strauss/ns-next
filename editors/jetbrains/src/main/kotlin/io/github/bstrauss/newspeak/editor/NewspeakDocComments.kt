package io.github.bstrauss.newspeak.editor

import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.PsiWhiteSpace
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * A doc comment: a block of two or more `///` lines, which documents what
 * follows it as a Java doc comment does. The colour
 * (`NewspeakDocCommentAnnotator`) and the rendered view
 * (`NewspeakInlineDocumentationProvider`) both read this one rule, so what
 * shows green is what renders.
 *
 * A block is the server's (`ns-syntax`'s `continues_line_block`): `///`
 * lines that each start their line, with only whitespace and one line break
 * between two of them. A `///` after code is a remark on its line and
 * belongs to no block.
 */
object NewspeakDocComments {
    /** Whether `element` is a `///` line of a doc comment. */
    fun isDocCommentLine(element: PsiElement): Boolean =
        element.node.elementType == Types.LINE_COMMENT &&
            startsItsLine(element) &&
            (blockNeighbour(element, PsiElement::getPrevSibling) != null || blockNeighbour(element, PsiElement::getNextSibling) != null)

    /** Every doc comment of `file`, each as its `///` lines in order. */
    fun blocks(file: PsiFile): List<List<PsiElement>> {
        val blocks = mutableListOf<List<PsiElement>>()
        var element = file.firstChild
        while (element != null) {
            if (isDocCommentLine(element)) {
                val lines = mutableListOf(element)
                while (true) lines += blockNeighbour(lines.last(), PsiElement::getNextSibling) ?: break
                blocks += lines
                element = lines.last()
            }
            element = element.nextSibling
        }
        return blocks
    }

    /** A doc comment's text as Markdown: each line without its `///` and the one space after it. */
    fun markdown(lines: List<PsiElement>): String =
        lines.joinToString("\n") { line -> line.text.removePrefix("///").removePrefix(" ").trimEnd() }

    /** The `///` line just before or after `comment` in its block, if there is one. */
    private fun blockNeighbour(comment: PsiElement, step: (PsiElement) -> PsiElement?): PsiElement? {
        val gap = step(comment) as? PsiWhiteSpace ?: return null
        val neighbour = step(gap) ?: return null
        val oneLineBreak = gap.text.count { it == '\n' } == 1
        return neighbour.takeIf { oneLineBreak && it.node.elementType == Types.LINE_COMMENT && startsItsLine(it) }
    }

    /** Whether only whitespace stands before `comment` on its line. */
    private fun startsItsLine(comment: PsiElement): Boolean {
        val before = comment.prevSibling ?: return true
        return before is PsiWhiteSpace && ('\n' in before.text || before.prevSibling == null)
    }
}
