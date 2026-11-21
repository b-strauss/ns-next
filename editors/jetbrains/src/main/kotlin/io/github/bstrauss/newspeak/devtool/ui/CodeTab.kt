package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.event.CaretEvent
import com.intellij.openapi.editor.event.CaretListener
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.FileEditorManagerEvent
import com.intellij.openapi.fileEditor.FileEditorManagerListener
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.ComboBox
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.TextRange
import com.intellij.ui.components.JBLabel
import com.intellij.util.ui.JBUI
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.TracedRuns
import io.github.bstrauss.newspeak.devtool.editor.CodeListing
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import io.github.bstrauss.newspeak.devtool.editor.DecorationsService
import io.github.bstrauss.newspeak.devtool.editor.LinkedHighlighting
import io.github.bstrauss.newspeak.devtool.editor.PlacedBody
import io.github.bstrauss.newspeak.devtool.editor.bytecodeListing
import io.github.bstrauss.newspeak.devtool.editor.codeListing
import io.github.bstrauss.newspeak.devtool.editor.listingOf
import io.github.bstrauss.newspeak.devtool.editor.showListing
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.awt.BorderLayout
import java.awt.Dimension
import java.awt.FlowLayout
import javax.swing.AbstractButton
import javax.swing.DefaultComboBoxModel
import javax.swing.JButton
import javax.swing.JList
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
 * Only while its run is [tracedRuns]' selected one does it follow the
 * caret, so that two runs' tabs never both draw; it takes the caret as it
 * stands, without waiting for it to move, when records arrive before it
 * showed a body, when its run is selected again and when another editor
 * is selected. It places the caret and
 * the source's statements by the bodies [decorations] keeps markers of,
 * which follow every edit, and links nothing in a method changed since the
 * run (decision 32).
 */
class CodeTab(
    private val project: Project,
    private val run: TraceRun,
    parent: Disposable,
    private val tracedRuns: TracedRuns = TracedRuns.getInstance(project),
    private val decorations: DecorationsService = DecorationsService.getInstance(project),
) : JPanel(BorderLayout()) {
    private val document = EditorFactory.getInstance().createDocument("")

    /** The read-only editor the listing is shown in. */
    val viewer: Editor = EditorFactory.getInstance().createViewer(document, project)

    /**
     * The copies of the body shown, as wide as its longest label -- a key
     * and an entered application make an optimized copy's long --, never
     * cut but where the tab is narrower than the bar would be.
     */
    val chooser: ComboBox<CodeCopy> = object : ComboBox<CodeCopy>() {
        override fun getPreferredSize(): Dimension {
            val size = super.getPreferredSize()
            val list = JList<CodeCopy>()
            val widest = (0 until itemCount).maxOfOrNull { index ->
                renderer.getListCellRendererComponent(list, getItemAt(index), -1, false, false).preferredSize.width
            } ?: 0
            // The arrow and the border around the label.
            val chrome = insets.left + insets.right + components.filterIsInstance<AbstractButton>().sumOf { button -> button.preferredSize.width }
            val wanted = maxOf(size.width, widest + chrome)
            val room = roomForChooser()
            return Dimension(if (room > 0) minOf(wanted, room) else wanted, size.height)
        }
    }

    private val title = JBLabel()

    private val openButton = JButton(NewspeakBundle.message("devtool.code.open"), AllIcons.Actions.MoveToWindow).apply {
        addActionListener { listing?.let(::openInEditor) }
    }

    private val bar = JPanel(FlowLayout(FlowLayout.LEFT)).apply {
        border = JBUI.Borders.empty(2, 4)
    }

    val highlighting = LinkedHighlighting(project, run, { document -> decorations.placedBodies(document, run) }, parent)

    /** The body whose copies are offered; null before the caret stood in one. */
    var shownBody: Int? = null
        private set

    /** The listing shown; null while none is. */
    var listing: CodeListing? = null
        private set

    /** Set while the tab writes its viewer, whose caret then moves without the user. */
    private var updating = false

    /** Whether its run was the selected one at the last change of [tracedRuns]. */
    private var wasActive = tracedRuns.selected === run

    init {
        Disposer.register(parent) { EditorFactory.getInstance().releaseEditor(viewer) }
        bar.add(title)
        bar.add(chooser)
        bar.add(openButton)
        add(bar, BorderLayout.NORTH)
        add(viewer.component, BorderLayout.CENTER)
        chooser.addActionListener { showChosen() }
        EditorFactory.getInstance().eventMulticaster.addCaretListener(
            object : CaretListener {
                override fun caretPositionChanged(event: CaretEvent) = caretMoved(event.editor)
            },
            parent,
        )
        project.messageBus.connect(parent).subscribe(
            FileEditorManagerListener.FILE_EDITOR_MANAGER,
            object : FileEditorManagerListener {
                override fun selectionChanged(event: FileEditorManagerEvent) = followSelectedEditor()
            },
        )
        // The caret stood in a method before the run described it: the records name its body now.
        run.onChanged(parent) { shownBody?.let { body -> offer(body, keepChoice = true) } ?: followSelectedEditor() }
        tracedRuns.addListener(
            {
                val nowActive = active()
                // A run no longer selected leaves nothing drawn for the selected one's tab to draw over.
                if (!nowActive) highlighting.deselect()
                // The caret moved while another run was selected.
                if (nowActive && !wasActive) followSelectedEditor()
                wasActive = nowActive
            },
            parent,
        )
        // A method an edit made stale is linked no longer, without waiting for the caret to move.
        decorations.addStalenessListener({ if (active()) highlighting.refresh() }, parent)
        followSelectedEditor()
    }

    /** Shows the copies of [body], its bytecode first: the gutter menu's "Show Code" and the caret's body. */
    fun show(body: Int) = offer(body, keepChoice = false)

    /**
     * Opens [listing] in an editor tab of its own, read-only, named `<body
     * name> <tier>`; the tab already open for the same copy of the same run
     * is brought forward instead of a second one.
     */
    fun openInEditor(listing: CodeListing): CodeListingFile {
        val manager = FileEditorManager.getInstance(project)
        val file = manager.openFiles.filterIsInstance<CodeListingFile>().firstOrNull { open ->
            open.listing.run === listing.run && open.listing.title == listing.title && open.listing.text == listing.text
        } ?: CodeListingFile(listing)
        manager.openFile(file, true)
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
        // Another body's labels: the chooser takes their width, and the bar is laid out again.
        bar.revalidate()
        bar.repaint()
        val index = copies.indexOfFirst { copy -> copy.label == chosen }.takeIf { found -> found >= 0 } ?: 0
        if (copies.isNotEmpty()) chooser.selectedIndex = index
        showChosen()
    }

    /** The bytecode listing holding [body] -- a block's is its method's -- and every install of [body]'s machine code. */
    private fun copies(body: Int): List<CodeCopy> {
        val copies = ArrayList<CodeCopy>()
        val bytecode = run.bytecode[body] ?: run.bytecode.values.firstOrNull { listed -> listed.bodies.any { part -> part.body == body } }
        if (bytecode != null) copies += CodeCopy(BYTECODE) { bytecodeListing(run, bytecode) }
        // A copy installed again under the same tier and key is numbered, so that a choice kept by its label is the same install.
        val installs = HashMap<String, Int>()
        for (code in run.code[body].orEmpty()) {
            val label = StringBuilder(code.tier)
            code.key?.let { key -> label.append(" key=").append(key) }
            code.entered?.let { entered -> label.append(" entered=").append(entered) }
            val count = installs.merge(label.toString(), 1, Int::plus) ?: 1
            if (count > 1) label.append(" (").append(count).append(')')
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

    /** The width the bar leaves the chooser beside the title and the button: the tab's, 0 before it was laid out. */
    private fun roomForChooser(): Int {
        if (width <= 0) return 0
        val layout = bar.layout as FlowLayout
        val gaps = layout.hgap * BAR_GAPS + bar.insets.left + bar.insets.right
        return maxOf(width - title.preferredSize.width - openButton.preferredSize.width - gaps, 1)
    }

    private fun active(): Boolean = tracedRuns.selected === run

    /** Takes the caret of the selected editor as it stands, as if it had just moved there. */
    private fun followSelectedEditor() {
        FileEditorManager.getInstance(project).selectedTextEditor?.let(::caretMoved)
    }

    private fun caretMoved(editor: Editor) {
        if (updating || editor.isDisposed) return
        if (editor.project != null && editor.project !== project) return
        if (!active()) {
            if (highlighting.selected != null) highlighting.deselect()
            return
        }
        val document = editor.document
        val shownListing = listingOf(document)
        if (shownListing != null) {
            if (shownListing.run !== run) return
            highlighting.selectInstruction(shownListing.positions.getOrNull(editor.caretModel.logicalPosition.line))
            return
        }
        // Only a Newspeak file's caret names a statement: another editor's -- a console scrolling with the program's output -- leaves the link as it is.
        if (FileDocumentManager.getInstance().getFile(document)?.extension != NEWSPEAK_EXTENSION) return
        val caret = editor.caretModel.offset
        // Bodies nest, so the innermost holding the caret starts last, and of those ends first.
        val placed = decorations.placedBodies(document, run)
            .filter { candidate -> candidate.range.startOffset <= caret && caret <= candidate.range.endOffset }
            .maxWithOrNull(compareBy<PlacedBody> { candidate -> candidate.range.startOffset }.thenByDescending { candidate -> candidate.range.endOffset })
        // Outside every body, and in a method changed since the run, the caret names no statement.
        if (placed == null || placed.stale) {
            highlighting.deselect()
            return
        }
        val body = placed.body
        if (body.number != shownBody) show(body.number)
        val statement = body.statements.withIndex()
            .mapNotNull { (index, statement) -> statement.range?.let(placed::place)?.let { range -> index to range } }
            .filter { (_, range) -> range.startOffset <= caret && caret <= range.endOffset }
            .maxWithOrNull(compareBy<Pair<Int, TextRange>> { (_, range) -> range.startOffset }.thenByDescending { (_, range) -> range.endOffset })
        highlighting.select(body.number, statement?.first)
    }

    private companion object {
        const val BYTECODE = "bytecode"

        /** A flow layout's gaps around and between the bar's three components. */
        const val BAR_GAPS = 4
        const val NEWSPEAK_EXTENSION = "ns"
    }
}
