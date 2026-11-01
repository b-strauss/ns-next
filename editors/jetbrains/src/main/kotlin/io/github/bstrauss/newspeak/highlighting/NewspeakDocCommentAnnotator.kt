package io.github.bstrauss.newspeak.highlighting

import com.intellij.lang.annotation.AnnotationHolder
import com.intellij.lang.annotation.Annotator
import com.intellij.lang.annotation.HighlightSeverity
import com.intellij.openapi.project.DumbAware
import com.intellij.psi.PsiElement
import io.github.bstrauss.newspeak.editor.NewspeakDocComments

/**
 * Colours every line of a doc comment (`NewspeakDocComments`) as Java colours
 * its doc comments (`NewspeakColors.DOC_COMMENT`); a single `///` line keeps
 * the comment colour. The lexer sees one line at a time and the highlighter
 * relexes from an edited line on, so a line's colour, which depends on the
 * lines around it, is decided here instead.
 */
class NewspeakDocCommentAnnotator : Annotator, DumbAware {
    override fun annotate(element: PsiElement, holder: AnnotationHolder) {
        if (!NewspeakDocComments.isDocCommentLine(element)) return
        holder.newSilentAnnotation(HighlightSeverity.TEXT_ATTRIBUTES)
            .textAttributes(NewspeakColors.DOC_COMMENT)
            .create()
    }
}
