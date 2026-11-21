package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.openapi.Disposable
import com.intellij.openapi.editor.Document
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.markup.HighlighterLayer
import com.intellij.openapi.editor.markup.HighlighterTargetArea
import com.intellij.openapi.editor.markup.RangeHighlighter
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.Disposer
import io.github.bstrauss.newspeak.devtool.model.TraceRun

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
 * reading of the source, placed in a source by its body's marker; a body
 * whose method changed since the run is painted nowhere (decision 32).
 * Read and changed on the EDT only.
 *
 * [placedBodies] answers a document's bodies of [run] where they stand now.
 */
class LinkedHighlighting(
    private val project: Project,
    private val run: TraceRun,
    private val placedBodies: (Document) -> List<PlacedBody>,
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

    /** Takes the selection and every highlight away. */
    fun deselect() = select(0, null)

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
        val editors = EditorFactory.getInstance().allEditors.filter { editor ->
            !editor.isDisposed && (editor.project == null || editor.project === project)
        }
        val listings = editors.filter { editor -> listingOf(editor.document)?.run === run }
        val sources = editors.filter { editor -> listingOf(editor.document) == null }.mapNotNull { editor ->
            placedBodies(editor.document).firstOrNull { placed -> placed.body.number == bodyNumber }?.let { placed -> editor to placed }
        }
        // A method changed since the run has no statement the records describe: nothing is linked to it.
        if (sources.any { (_, placed) -> placed.stale }) return
        val color = linkedColor(statementIndex)
        for (editor in listings) {
            val document = editor.document
            for ((line, position) in checkNotNull(listingOf(document)).positions.withIndex()) {
                if (position == null || position.body != bodyNumber || position.index !in statement.instructions) continue
                if (line >= document.lineCount) continue
                highlighters += editor.markupModel.addRangeHighlighter(
                    color, document.getLineStartOffset(line), document.getLineEndOffset(line), LAYER, HighlighterTargetArea.LINES_IN_RANGE,
                )
            }
        }
        for ((editor, placed) in sources) {
            val range = statement.range?.let(placed::place) ?: continue
            highlighters += editor.markupModel.addRangeHighlighter(color, range.startOffset, range.endOffset, LAYER, HighlighterTargetArea.EXACT_RANGE)
        }
    }

    /** Takes every highlight away. */
    fun clear() {
        for (highlighter in highlighters) highlighter.dispose()
        highlighters.clear()
    }

    private companion object {
        /** Below the selection and the caret row, above the syntax colours. */
        const val LAYER = HighlighterLayer.SELECTION - 1
    }
}
