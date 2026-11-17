package io.github.bstrauss.newspeak.debugger

import com.intellij.icons.AllIcons
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.editor.Document
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.project.ProjectLocator
import com.intellij.openapi.util.TextRange
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.vfs.VirtualFileManager
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.breakpoints.XLineBreakpoint
import com.intellij.xdebugger.breakpoints.XLineBreakpointType
import com.intellij.xdebugger.breakpoints.ui.XBreakpointCustomPropertiesPanel
import com.intellij.xdebugger.evaluation.XDebuggerEditorsProvider
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakFileType
import org.eclipse.lsp4j.Position
import javax.swing.Icon

/**
 * A line breakpoint in a `.ns` file, which the agent sets at the first
 * statement starting on its line or after it, or, for a block variant, at
 * the block's first statement. Where it may stand is the language server's
 * answer ([BreakableLinesCache]); while there is none for the document's
 * current text every line is allowed, and the agent moves a breakpoint
 * that stands on none to the statement it sets it at.
 */
class NewspeakLineBreakpointType :
    XLineBreakpointType<NewspeakLineBreakpointProperties>(ID, NewspeakBundle.message("debugger.breakpoint.line.title")) {
    /** Never waits: a gutter click is answered from the cache, every line while it has no answer. */
    override fun canPutAt(file: VirtualFile, line: Int, project: Project): Boolean {
        if (file.fileType != NewspeakFileType) return false
        val lines = BreakableLinesCache.getInstance(project).currentOrAsk(file) ?: return true
        return lines.allows(line)
    }

    override fun createBreakpointProperties(file: VirtualFile, line: Int): NewspeakLineBreakpointProperties =
        NewspeakLineBreakpointProperties()

    /**
     * The line itself and each block whose first statement starts on it,
     * but a block whose first statement is the line's first: there the two
     * stop at the same statement. None for a line with no such block, which
     * the platform takes as one plain line breakpoint.
     */
    override fun computeVariants(project: Project, position: XSourcePosition): List<XLineBreakpointVariant> {
        val lines = BreakableLinesCache.getInstance(project).currentOrAsk(position.file) ?: return emptyList()
        val blocks = ReadAction.computeBlocking<List<BlockVariant>, RuntimeException> {
            val document = FileDocumentManager.getInstance().getDocument(position.file) ?: return@computeBlocking emptyList()
            if (position.line >= document.lineCount) return@computeBlocking emptyList()
            val lineStart = document.getLineStartOffset(position.line)
            val lineEnd = document.getLineEndOffset(position.line)
            val firstColumn = document.charsSequence.subSequence(lineStart, lineEnd).indexOfFirst { character -> !character.isWhitespace() }
            lines.blocksOn(position.line)
                .filter { block -> block.position.line == position.line && block.position.character != firstColumn }
                .mapNotNull { block ->
                    val range = textRangeOf(document, block.range.start, block.range.end) ?: return@mapNotNull null
                    BlockVariant(block.position.character, range, excerpt(document.charsSequence.subSequence(range.startOffset, range.endOffset)))
                }
        }
        if (blocks.isEmpty()) return emptyList()
        return listOf(LineVariant()) + blocks
    }

    override fun variantAndBreakpointMatch(
        breakpoint: XLineBreakpoint<NewspeakLineBreakpointProperties>,
        variant: XLineBreakpointVariant,
    ): Boolean = (variant as? NewspeakVariant)?.column == breakpoint.properties?.column

    /** A block variant's block, as the server last placed it; the whole line otherwise. */
    override fun getHighlightRange(breakpoint: XLineBreakpoint<NewspeakLineBreakpointProperties>): TextRange? {
        val column = breakpoint.properties?.column ?: return null
        val file = VirtualFileManager.getInstance().findFileByUrl(breakpoint.fileUrl) ?: return null
        val project = ProjectLocator.getInstance().guessProjectForFile(file) ?: return null
        val block = BreakableLinesCache.getInstance(project).current(file)
            ?.blocksOn(breakpoint.line)
            ?.firstOrNull { block -> block.position.character == column }
            ?: return null
        return ReadAction.computeBlocking<TextRange?, RuntimeException> {
            FileDocumentManager.getInstance().getDocument(file)?.let { document -> textRangeOf(document, block.range.start, block.range.end) }
        }
    }

    override fun getDisplayText(breakpoint: XLineBreakpoint<NewspeakLineBreakpointProperties>): String {
        val line = super.getDisplayText(breakpoint)
        val column = breakpoint.properties?.column ?: return line
        return NewspeakBundle.message("debugger.breakpoint.line.block", line, (column + 1).toString())
    }

    override fun createCustomPropertiesPanel(project: Project): XBreakpointCustomPropertiesPanel<XLineBreakpoint<NewspeakLineBreakpointProperties>> =
        PassCountPanel()

    /**
     * Newspeak's editor for the condition and the log expression, which
     * the agent decides and evaluates at each hit ([conditionOf],
     * [isLogpoint]).
     */
    override fun getEditorsProvider(
        breakpoint: XLineBreakpoint<NewspeakLineBreakpointProperties>,
        project: Project,
    ): XDebuggerEditorsProvider = NewspeakDebuggerEditorsProvider

    /** A variant of this type, and the column its breakpoint is sent with. */
    private interface NewspeakVariant {
        val column: Int?
    }

    private inner class LineVariant : XLineBreakpointVariant(), NewspeakVariant {
        override val column: Int? = null

        override fun getText(): String = NewspeakBundle.message("debugger.breakpoint.variant.line")

        override fun getIcon(): Icon = enabledIcon

        override fun getHighlightRange(): TextRange? = null

        override fun createProperties(): NewspeakLineBreakpointProperties = NewspeakLineBreakpointProperties()
    }

    private inner class BlockVariant(
        override val column: Int,
        private val range: TextRange,
        private val excerpt: String,
    ) : XLineBreakpointVariant(), NewspeakVariant {
        override fun getText(): String = NewspeakBundle.message("debugger.breakpoint.variant.block", excerpt)

        override fun getIcon(): Icon = AllIcons.Nodes.Lambda

        override fun getHighlightRange(): TextRange = range

        override fun createProperties(): NewspeakLineBreakpointProperties = NewspeakLineBreakpointProperties().apply { column = this@BlockVariant.column }
    }

    companion object {
        const val ID = "newspeak-line"

        /** The longest excerpt of a block a variant's name shows. */
        private const val EXCERPT_LENGTH = 40

        private fun excerpt(text: CharSequence): String {
            val oneLine = text.toString().replace(Regex("""\s+"""), " ")
            return if (oneLine.length <= EXCERPT_LENGTH) oneLine else oneLine.take(EXCERPT_LENGTH - 1) + "…"
        }

        /** The offsets of an LSP range in [document], or null for one outside it. */
        private fun textRangeOf(document: Document, start: Position, end: Position): TextRange? {
            val startOffset = offsetOf(document, start) ?: return null
            val endOffset = offsetOf(document, end) ?: return null
            return if (startOffset <= endOffset) TextRange(startOffset, endOffset) else null
        }

        /** An LSP position's offset: its line's start plus its UTF-16 code units, which are the document's characters. */
        private fun offsetOf(document: Document, position: Position): Int? {
            if (position.line < 0 || position.line >= document.lineCount || position.character < 0) return null
            val offset = document.getLineStartOffset(position.line) + position.character
            return offset.takeIf { offset <= document.getLineEndOffset(position.line) }
        }
    }
}
