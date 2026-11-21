package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.service
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.event.CaretEvent
import com.intellij.openapi.editor.event.CaretListener
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.ComboBox
import com.intellij.openapi.util.Disposer
import com.intellij.ui.components.JBLabel
import com.intellij.util.ui.JBUI
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.TracedRuns
import io.github.bstrauss.newspeak.devtool.editor.CodeListing
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import io.github.bstrauss.newspeak.devtool.editor.LinkedHighlighting
import io.github.bstrauss.newspeak.devtool.editor.bytecodeListing
import io.github.bstrauss.newspeak.devtool.editor.codeListing
import io.github.bstrauss.newspeak.devtool.editor.listingOf
import io.github.bstrauss.newspeak.devtool.editor.showListing
import io.github.bstrauss.newspeak.devtool.model.SourceRange
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import java.awt.BorderLayout
import java.awt.FlowLayout
import java.nio.file.Path
import javax.swing.DefaultComboBoxModel
import javax.swing.JButton
import javax.swing.JPanel

/** A copy of a body the chooser offers: its [label] and how its listing is made. */
class CodeCopy(val label: String, val listing: () -> CodeListing) {
    override fun toString(): String = label
}

/**
 * The Code tab (`docs/design/developer-tool.md`, decisions 29 and 30): the
 * bytecode and machine code of the body at the caret of a source editor --
 * the innermost body of [run] whose range holds it --, a chooser of its
 * copies -- its bytecode, then each install of machine code in its order,
 * baseline, twin and optimized per key -- over a read-only viewer of the
 * listing, and "Open in Editor", which puts the copy shown into an editor
 * tab that stays. Source, bytecode and machine code are highlighted
 * together ([LinkedHighlighting]).
 *
 * Only while [active] -- its run the selected one -- does it follow the
 * caret, so that two runs' tabs never both draw; [libraryDirectory] answers
 * where the core library's files lie.
 */
class CodeTab(
    private val project: Project,
    private val run: TraceRun,
    parent: Disposable,
    private val libraryDirectory: () -> Path? = { project.service<NewspeakCoreLibrary>().directory },
    private val active: () -> Boolean = { TracedRuns.getInstance(project).selected === run },
) : JPanel(BorderLayout()) {
    private val document = EditorFactory.getInstance().createDocument("")

    /** The read-only editor the listing is shown in. */
    val viewer: Editor = EditorFactory.getInstance().createViewer(document, project)

    /** The copies of the body shown. */
    val chooser = ComboBox<CodeCopy>()

    private val title = JBLabel()

    val highlighting = LinkedHighlighting(project, run, { body -> LinkedHighlighting.sourcePathOf(body, libraryDirectory()) }, parent)

    /** The body whose copies are offered; null before the caret stood in one. */
    var shownBody: Int? = null
        private set

    /** The listing shown; null while none is. */
    var listing: CodeListing? = null
        private set

    /** Set while the tab writes its viewer, whose caret then moves without the user. */
    private var updating = false

    init {
        Disposer.register(parent) { EditorFactory.getInstance().releaseEditor(viewer) }
        val open = JButton(NewspeakBundle.message("devtool.code.open"), AllIcons.Actions.MoveToWindow).apply {
            addActionListener { listing?.let(::openInEditor) }
        }
        val bar = JPanel(FlowLayout(FlowLayout.LEFT)).apply {
            border = JBUI.Borders.empty(2, 4)
            add(title)
            add(chooser)
            add(open)
        }
        add(bar, BorderLayout.NORTH)
        add(viewer.component, BorderLayout.CENTER)
        chooser.addActionListener { showChosen() }
        EditorFactory.getInstance().eventMulticaster.addCaretListener(
            object : CaretListener {
                override fun caretPositionChanged(event: CaretEvent) = caretMoved(event.editor)
            },
            parent,
        )
        run.onChanged(parent) { shownBody?.let { body -> offer(body, keepChoice = true) } }
    }

    /** Shows the copies of [body], its bytecode first: the gutter menu's "Show Code" and the caret's body. */
    fun show(body: Int) = offer(body, keepChoice = false)

    /** Opens [listing] in an editor tab of its own, read-only, named `<body name> <tier>`. */
    fun openInEditor(listing: CodeListing): CodeListingFile {
        val file = CodeListingFile(listing)
        FileEditorManager.getInstance(project).openFile(file, true)
        return file
    }

    private fun offer(body: Int, keepChoice: Boolean) {
        val copies = copies(body)
        val chosen = (chooser.selectedItem as? CodeCopy)?.label.takeIf { keepChoice && shownBody == body }
        // A batch that brought nothing new for the body leaves the viewer as it is.
        if (keepChoice && shownBody == body && copies.map(CodeCopy::label) == (0 until chooser.itemCount).map { index -> chooser.getItemAt(index).label }) return
        shownBody = body
        title.text = bodyName(run, body)
        chooser.model = DefaultComboBoxModel(copies.toTypedArray())
        val index = copies.indexOfFirst { copy -> copy.label == chosen }.takeIf { found -> found >= 0 } ?: 0
        if (copies.isNotEmpty()) chooser.selectedIndex = index
        showChosen()
    }

    /** The bytecode listing holding [body] -- a block's is its method's -- and every install of [body]'s machine code. */
    private fun copies(body: Int): List<CodeCopy> {
        val copies = ArrayList<CodeCopy>()
        val bytecode = run.bytecode[body] ?: run.bytecode.values.firstOrNull { listed -> listed.bodies.any { part -> part.body == body } }
        if (bytecode != null) copies += CodeCopy(BYTECODE) { bytecodeListing(run, bytecode) }
        for (code in run.code[body].orEmpty()) {
            val label = StringBuilder(code.tier)
            code.key?.let { key -> label.append(" key=").append(key) }
            code.entered?.let { entered -> label.append(" entered=").append(entered) }
            copies += CodeCopy(label.toString()) { codeListing(run, code) }
        }
        return copies
    }

    private fun showChosen() {
        val shown = (chooser.selectedItem as? CodeCopy)?.listing?.invoke()
        listing = shown
        updating = true
        try {
            ApplicationManager.getApplication().runWriteAction { document.setText(shown?.text.orEmpty()) }
            showListing(document, shown)
        } finally {
            updating = false
        }
        highlighting.refresh()
    }

    private fun caretMoved(editor: Editor) {
        if (updating || editor.isDisposed) return
        if (editor.project != null && editor.project !== project) return
        if (!active()) {
            // A run no longer selected leaves nothing drawn for the selected one's tab to draw over.
            if (highlighting.selected != null) highlighting.select(0, null)
            return
        }
        val document = editor.document
        val shownListing = listingOf(document)
        if (shownListing != null) {
            if (shownListing.run !== run) return
            highlighting.selectInstruction(shownListing.positions.getOrNull(editor.caretModel.logicalPosition.line))
            return
        }
        val file = FileDocumentManager.getInstance().getFile(document) ?: return
        val path = file.fileSystem.getNioPath(file)?.let(LinkedHighlighting::normalized) ?: return
        val offset = editor.caretModel.offset
        val line = document.getLineNumber(offset)
        val caret = Place(line, offset - document.getLineStartOffset(line))
        val library = libraryDirectory()
        val body = run.bodies.values
            .filter { candidate -> candidate.range?.holds(caret) == true }
            .filter { candidate -> LinkedHighlighting.sourcePathOf(candidate, library)?.let(LinkedHighlighting::normalized) == path }
            .innermost { candidate -> checkNotNull(candidate.range) } ?: return
        if (body.number != shownBody) show(body.number)
        val statement = body.statements.withIndex()
            .filter { (_, statement) -> statement.range?.holds(caret) == true }
            .innermost { (_, statement) -> checkNotNull(statement.range) }
        highlighting.select(body.number, statement?.index)
    }

    /** A place in a document: its line and its column in UTF-16 units, as the records' ranges count them. */
    private data class Place(val line: Int, val column: Int) : Comparable<Place> {
        override fun compareTo(other: Place): Int = compareValuesBy(this, other, Place::line, Place::column)
    }

    private val SourceRange.start get() = Place(startLine, startColumn)

    private val SourceRange.end get() = Place(endLine, endColumn)

    private fun SourceRange.holds(place: Place): Boolean = start <= place && place <= end

    /** The one of these whose range lies innermost: ranges nest, so the latest start, and of those the earliest end. */
    private fun <T> List<T>.innermost(range: (T) -> SourceRange): T? =
        maxWithOrNull(compareBy<T> { item -> range(item).start }.thenByDescending { item -> range(item).end })

    private companion object {
        const val BYTECODE = "bytecode"
    }
}
