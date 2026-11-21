package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.openapi.Disposable
import com.intellij.openapi.editor.Document
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.markup.HighlighterLayer
import com.intellij.openapi.editor.markup.HighlighterTargetArea
import com.intellij.openapi.editor.markup.RangeHighlighter
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.Disposer
import io.github.bstrauss.newspeak.devtool.model.Body
import io.github.bstrauss.newspeak.devtool.model.SourceRange
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.nio.file.InvalidPathException
import java.nio.file.Path

/**
 * The colours of the linked statements, one per statement by its index,
 * cycling: neighbouring statements never share one. Keys of their own, not
 * derived from an `EditorColors` key, so that no other highlighting's colour
 * makes two statements alike; their defaults are the plugin's schemes'.
 */
val LINKED_STATEMENT_COLORS: List<TextAttributesKey> =
    (1..LINKED_COLOR_COUNT).map { number -> TextAttributesKey.createTextAttributesKey("NEWSPEAK_DEVTOOL_LINKED_$number") }

private const val LINKED_COLOR_COUNT = 6

/** The colour of the statement [statement] of its body. */
fun linkedColor(statement: Int): TextAttributesKey = LINKED_STATEMENT_COLORS[statement % LINKED_STATEMENT_COLORS.size]

/**
 * Source, bytecode and machine code highlighted together
 * (`docs/design/developer-tool.md`, decision 29): the statement [select]ed --
 * a body's statement by its index -- in every editor of the body's source,
 * and in every listing of [run] the lines of its instructions -- the
 * bytecode lines whose index lies in the statement's instructions, the
 * machine code lines whose position does, in a scope of the same body. The
 * positions are the run's records' (decision 18), never the plugin's
 * reading of the source. Read and changed on the EDT only.
 *
 * [sourcePath] answers the file a body's source lies in.
 */
class LinkedHighlighting(
    private val project: Project,
    private val run: TraceRun,
    private val sourcePath: (Body) -> Path?,
    parent: Disposable,
) {
    /** The statement highlighted: its body and its index there; null while none is. */
    var selected: Pair<Int, Int>? = null
        private set

    private val highlighters = ArrayList<RangeHighlighter>()

    init {
        Disposer.register(parent) { clear() }
    }

    /** Highlights [statement] of [body] everywhere, or nothing for a null statement. */
    fun select(body: Int, statement: Int?) {
        selected = statement?.let { index -> body to index }
        refresh()
    }

    /** Highlights the statement the instruction at [position] belongs to; nothing for an instruction outside every statement. */
    fun selectInstruction(position: ListingPosition?) {
        val statement = position?.let { at ->
            run.bodies[at.body]?.statements?.indexOfFirst { statement -> at.index in statement.instructions }?.takeIf { index -> index >= 0 }
        }
        select(position?.body ?: 0, statement)
    }

    /** Draws the selection again, as a listing's text or an editor's file changed. */
    fun refresh() {
        clear()
        val (bodyNumber, statementIndex) = selected ?: return
        val body = run.bodies[bodyNumber] ?: return
        val statement = body.statements.getOrNull(statementIndex) ?: return
        val color = linkedColor(statementIndex)
        val path = sourcePath(body)?.let(::normalized)
        for (editor in EditorFactory.getInstance().allEditors) {
            if (editor.isDisposed || (editor.project != null && editor.project !== project)) continue
            val listing = listingOf(editor.document)
            if (listing != null) {
                if (listing.run !== run) continue
                for ((line, position) in listing.positions.withIndex()) {
                    if (position == null || position.body != bodyNumber || position.index !in statement.instructions) continue
                    if (line >= editor.document.lineCount) continue
                    val document = editor.document
                    highlighters += editor.markupModel.addRangeHighlighter(
                        color, document.getLineStartOffset(line), document.getLineEndOffset(line), LAYER, HighlighterTargetArea.LINES_IN_RANGE,
                    )
                }
            } else if (path != null && editorPath(editor) == path) {
                val range = statement.range?.let { range -> offsets(editor.document, range) } ?: continue
                highlighters += editor.markupModel.addRangeHighlighter(color, range.first, range.last, LAYER, HighlighterTargetArea.EXACT_RANGE)
            }
        }
    }

    /** Takes every highlight away. */
    fun clear() {
        for (highlighter in highlighters) highlighter.dispose()
        highlighters.clear()
    }

    private fun editorPath(editor: Editor): Path? {
        val file = FileDocumentManager.getInstance().getFile(editor.document) ?: return null
        return file.fileSystem.getNioPath(file)?.let(::normalized)
    }

    companion object {
        /** Below the selection and the caret row, above the syntax colours. */
        private const val LAYER = HighlighterLayer.SELECTION - 1

        /** [path] as two names of one file compare equal. */
        fun normalized(path: Path): Path = path.toAbsolutePath().normalize()

        /** The file [body]'s source lies in: a program's by its path, a library module's under [libraryDirectory]; null without one. */
        fun sourcePathOf(body: Body, libraryDirectory: Path?): Path? = try {
            body.file?.let { file -> Path.of(file) } ?: body.library?.let { library -> libraryDirectory?.resolve(library) }
        } catch (_: InvalidPathException) {
            null
        }

        /** [range]'s start and end offsets in [document]; null where it lies outside the text. */
        fun offsets(document: Document, range: SourceRange): IntRange? {
            fun offset(line: Int, column: Int): Int? {
                if (line < 0 || line >= document.lineCount || column < 0) return null
                // The runtime counts columns in UTF-16 units, as a document's offsets are.
                val offset = document.getLineStartOffset(line) + column
                return offset.takeIf { offset <= document.getLineEndOffset(line) }
            }
            val start = offset(range.startLine, range.startColumn) ?: return null
            val end = offset(range.endLine, range.endColumn) ?: return null
            return if (start <= end) start..end else null
        }
    }
}
