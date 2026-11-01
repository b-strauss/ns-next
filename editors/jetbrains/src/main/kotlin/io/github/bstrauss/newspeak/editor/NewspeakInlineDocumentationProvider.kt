package io.github.bstrauss.newspeak.editor

import com.intellij.markdown.utils.doc.DocMarkdownToHtmlConverter
import com.intellij.openapi.editor.Document
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.TextRange
import com.intellij.platform.backend.documentation.DocumentationTarget
import com.intellij.platform.backend.documentation.InlineDocumentation
import com.intellij.platform.backend.documentation.InlineDocumentationProvider
import com.intellij.platform.lsp.util.getOffsetInDocument
import com.intellij.psi.PsiDocumentManager
import com.intellij.psi.PsiFile
import io.github.bstrauss.newspeak.NewspeakLanguage
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import org.eclipse.lsp4j.DocumentSymbol

/**
 * Renders every doc comment (`NewspeakDocComments`) in place, its text read
 * as Markdown, as Java renders its doc comments: the platform draws it, puts
 * the toggle between rendered and raw text in the gutter, and follows
 * *Settings | Editor | Appearance | Render documentation comments* and the
 * reader mode.
 *
 * What a doc comment documents is the server's to say: the document symbol
 * -- a class, a method, a slot -- that starts on the line after it, the
 * first of them when several do, so the outermost. Without the server's
 * symbols for the current text the comment has no owner yet and renders
 * all the same; the daemon runs again once they arrive.
 */
class NewspeakInlineDocumentationProvider : InlineDocumentationProvider {
    override fun inlineDocumentationItems(file: PsiFile?): Collection<InlineDocumentation> {
        // The extension point is every language's.
        if (file == null || file.language != NewspeakLanguage) return emptyList()
        val document = PsiDocumentManager.getInstance(file.project).getDocument(file) ?: return emptyList()
        val blocks = NewspeakDocComments.blocks(file)
        if (blocks.isEmpty()) return emptyList()
        val symbols = file.virtualFile?.let { DocumentSymbolCache.getInstance(file.project).currentForDaemon(it) }
        return blocks.map { lines ->
            val range = TextRange(lines.first().textRange.startOffset, lines.last().textRange.endOffset)
            NewspeakDocComment(file.project, NewspeakDocComments.markdown(lines), range, symbols?.let { owner(it, range, document) })
        }
    }

    override fun findInlineDocumentation(file: PsiFile, textRange: TextRange): InlineDocumentation? =
        inlineDocumentationItems(file).firstOrNull { it.documentationRange == textRange }

    /** The range of the first symbol, in pre-order, that starts on the line after `comment`. */
    private fun owner(symbols: List<DocumentSymbol>, comment: TextRange, document: Document): TextRange? {
        val line = document.getLineNumber(comment.endOffset) + 1
        fun find(level: List<DocumentSymbol>): DocumentSymbol? {
            for (symbol in level) {
                if (symbol.range.start.line == line) return symbol
                find(symbol.children.orEmpty())?.let { return it }
            }
            return null
        }
        val symbol = find(symbols) ?: return null
        val start = getOffsetInDocument(document, symbol.range.start) ?: return null
        val end = getOffsetInDocument(document, symbol.range.end) ?: return null
        return TextRange(start, end).takeIf { start <= end }
    }
}

private class NewspeakDocComment(
    private val project: Project,
    private val markdown: String,
    private val range: TextRange,
    private val owner: TextRange?,
) : InlineDocumentation {
    override fun getDocumentationRange(): TextRange = range

    override fun getDocumentationOwnerRange(): TextRange? = owner

    /**
     * The description, then the known tags as the platform's section table,
     * as Java renders its tags; each part's Markdown through the platform's
     * converter, so a tag's code reads as the description's does.
     */
    override fun renderText(): String {
        val comment = NewspeakDocTags.parse(markdown)
        val render = { text: String -> DocMarkdownToHtmlConverter.convert(project, text, NewspeakLanguage) }
        return render(comment.description) + NewspeakDocTags.sectionsHtml(comment, render)
    }

    override fun getOwnerTarget(): DocumentationTarget? = null
}
