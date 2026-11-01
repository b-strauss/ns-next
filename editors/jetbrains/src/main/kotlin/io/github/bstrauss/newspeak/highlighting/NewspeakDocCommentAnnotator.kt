package io.github.bstrauss.newspeak.highlighting

import com.intellij.lang.annotation.AnnotationHolder
import com.intellij.lang.annotation.Annotator
import com.intellij.lang.annotation.HighlightSeverity
import com.intellij.openapi.project.DumbAware
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiWhiteSpace
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * Colours a block of two or more `///` lines as a doc comment
 * (`NewspeakColors.DOC_COMMENT`), as Java colours its doc comments; a single
 * `///` line keeps the comment colour.
 *
 * A block is the server's (`ns-syntax`'s `continues_line_block`): `///` lines
 * that each start their line, with only whitespace and one line break between
 * two of them. A `///` after code is a remark on its line and belongs to no
 * block. The lexer sees one line at a time and the highlighter relexes from
 * an edited line on, so a line's colour, which depends on the lines around
 * it, is decided here instead.
 */
class NewspeakDocCommentAnnotator : Annotator, DumbAware {
    override fun annotate(element: PsiElement, holder: AnnotationHolder) {
        if (element.node.elementType != Types.LINE_COMMENT || !startsItsLine(element)) return
        if (blockNeighbour(element, PsiElement::getPrevSibling) == null &&
            blockNeighbour(element, PsiElement::getNextSibling) == null
        ) {
            return
        }
        holder.newSilentAnnotation(HighlightSeverity.TEXT_ATTRIBUTES)
            .textAttributes(NewspeakColors.DOC_COMMENT)
            .create()
    }

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
