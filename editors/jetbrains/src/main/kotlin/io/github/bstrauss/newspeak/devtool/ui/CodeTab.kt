package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.icons.AllIcons
import com.intellij.ide.IdeEventQueue
import com.intellij.ide.util.PropertiesComponent
import com.intellij.openapi.Disposable
import com.intellij.openapi.actionSystem.ActionManager
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.DefaultActionGroup
import com.intellij.openapi.actionSystem.ToggleAction
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.components.service
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.event.CaretEvent
import com.intellij.openapi.editor.event.CaretListener
import com.intellij.openapi.editor.event.EditorFactoryEvent
import com.intellij.openapi.editor.event.EditorFactoryListener
import com.intellij.openapi.editor.ex.EditorEx
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.FileEditorManagerEvent
import com.intellij.openapi.fileEditor.FileEditorManagerListener
import com.intellij.openapi.fileEditor.TextEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.ComboBox
import com.intellij.openapi.util.Condition
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.TextRange
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.ui.components.JBLabel
import com.intellij.util.ui.JBUI
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.TracedRuns
import io.github.bstrauss.newspeak.devtool.editor.CodeListing
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import io.github.bstrauss.newspeak.devtool.editor.DecorationsService
import io.github.bstrauss.newspeak.devtool.editor.LinkedHighlighting
import io.github.bstrauss.newspeak.devtool.editor.ListingNames
import io.github.bstrauss.newspeak.devtool.editor.PlacedBody
import io.github.bstrauss.newspeak.devtool.editor.bodySourcePath
import io.github.bstrauss.newspeak.devtool.editor.bytecodeListing
import io.github.bstrauss.newspeak.devtool.editor.codeListing
import io.github.bstrauss.newspeak.devtool.editor.listingOf
import io.github.bstrauss.newspeak.devtool.editor.showListing
import io.github.bstrauss.newspeak.devtool.model.Bytecode
import io.github.bstrauss.newspeak.devtool.model.CodeEntry
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import java.awt.BorderLayout
import java.awt.Dimension
import java.awt.FlowLayout
import java.awt.Font
import java.awt.event.InputEvent
import java.nio.file.Path
import javax.swing.ComboBoxModel
import javax.swing.DefaultComboBoxModel
import javax.swing.JButton
import javax.swing.JComponent
import javax.swing.JList
import javax.swing.JPanel
import javax.swing.JViewport
import javax.swing.ListCellRenderer
import javax.swing.UIManager
import javax.swing.event.ChangeListener

/** Whether [editor]'s viewport is shown with an extent: the platform's own test before it scrolls a restored editor. */
private fun viewportShowing(editor: Editor): Boolean {
    val viewport = (editor as? EditorEx)?.scrollPane?.viewport ?: return true
    return viewport.isShowing && viewport.extentSize.width > 0 && viewport.extentSize.height > 0
}

/** Whether [editor]'s viewport has an extent, shown or not: the viewer is scrolled by its own size alone. */
private fun hasExtent(editor: Editor): Boolean {
    val viewport = (editor as? EditorEx)?.scrollPane?.viewport ?: return true
    return viewport.extentSize.width > 0 && viewport.extentSize.height > 0
}

/**
 * A copy of a body the chooser offers: its [label], and either the body's
 * [bytecode], which the run keeps, or an [install] of its machine code,
 * whose listing is read from the run's file when it is shown.
 */
class CodeCopy private constructor(val label: String, val bytecode: Bytecode?, val install: CodeEntry?) {
    constructor(label: String, bytecode: Bytecode) : this(label, bytecode, null)

    constructor(label: String, install: CodeEntry) : this(label, null, install)

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
 * together ([LinkedHighlighting]). A copy installed again under the same
 * tier and key -- an optimized copy compiled anew after its retirement --
 * is offered once more, its label numbered: `optimized key=Demo (2)`.
 * "Scroll with the editor" ([syncWithEditor]) scrolls the viewer to the
 * listing lines of a source caret's statement and the source to the
 * statement of a viewer caret's line.
 *
 * Only while its run is [tracedRuns]' selected one does it follow the
 * caret, so that two runs' tabs never both draw; it takes the caret as it
 * stands, without waiting for it to move, when records arrive before it
 * showed a body, when its run is selected again and when another editor
 * is selected. It places the caret and
 * the source's statements by the bodies [decorations] keeps markers of,
 * which follow every edit, and links nothing in a method changed since the
 * run (decision 32).
 *
 * An install's listing is read from the run's file ([reads], decision 34)
 * when it is chosen, the viewer keeping the text it shows until it
 * arrives -- saying that it reads only where the read takes long -- and
 * saying why where the file no longer gives it; a read for a copy no
 * longer chosen when it arrives is dropped, and a batch that leaves the
 * chosen install as it is changes nothing in the viewer.
 */
class CodeTab(
    private val project: Project,
    private val run: TraceRun,
    parent: Disposable,
    private val tracedRuns: TracedRuns = TracedRuns.getInstance(project),
    private val decorations: DecorationsService = DecorationsService.getInstance(project),
    private val libraryDirectory: () -> Path? = { project.service<NewspeakCoreLibrary>().directory },
    private val byUser: () -> Boolean = { IdeEventQueue.getInstance().trueCurrentEvent is InputEvent },
    private val whenLoaded: (Editor, Runnable) -> Unit = { editor, action -> FileEditorManager.getInstance(project).runWhenLoaded(editor, action) },
    private val deliverOpened: (VirtualFile, Runnable) -> Unit = { _, action -> action.run() },
    private val reads: RecordReads = RecordReads.pooled(),
    private val viewportReady: (Editor) -> Boolean = ::viewportShowing,
) : JPanel(BorderLayout()) {
    private val document = EditorFactory.getInstance().createDocument("")

    /** The read-only editor the listing is shown in. */
    val viewer: Editor = EditorFactory.getInstance().createViewer(document, project)

    /** The copies of the body shown, every label painted whole but where the tab is narrower ([CopyChooser]). */
    val chooser = CopyChooser { roomForChooser() }

    private val title = JBLabel()

    /** Opens the listing shown in an editor tab; off while none is, a chosen copy's read pending among them. */
    val openButton = JButton(NewspeakBundle.message("devtool.code.open"), AllIcons.Actions.MoveToWindow).apply {
        addActionListener { listing?.let(::openInEditor) }
    }

    /** The toggle of [syncWithEditor]. */
    val syncAction: ToggleAction = object : ToggleAction(
        NewspeakBundle.message("devtool.code.sync"),
        NewspeakBundle.message("devtool.code.sync"),
        AllIcons.General.AutoscrollToSource,
    ) {
        override fun isSelected(event: AnActionEvent): Boolean = syncWithEditor

        override fun setSelected(event: AnActionEvent, state: Boolean) {
            syncWithEditor = state
        }

        override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.EDT
    }

    private val toolbar = ActionManager.getInstance().createActionToolbar(TOOLBAR_PLACE, DefaultActionGroup(syncAction), true).apply {
        targetComponent = this@CodeTab
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

    /** Set while [offer] fills the chooser, whose selection then changes without the user and is shown once, by [offer]. */
    private var offering = false

    /**
     * Whether the viewer and the source editors scroll with each other's
     * caret: the project's choice, off until the user turns it on. It
     * scrolls and never moves a caret, so that neither direction sets off
     * the other.
     */
    var syncWithEditor: Boolean
        get() = PropertiesComponent.getInstance(project).getBoolean(SYNC_PROPERTY, false)
        set(value) {
            PropertiesComponent.getInstance(project).setValue(SYNC_PROPERTY, value, false)
            // Off is today's behaviour: every caret and selection is followed again at once.
            if (!value) endSyncOpenings()
            // On, the two sides are brought together at once, as the platform's own autoscroll does.
            if (value) followSelectedEditor()
        }

    /** Set while [openForSync] opens a file, whose selection and caret then change without the user. */
    private var openingForSync = false

    /**
     * A file the tab brought forward or opened for a viewer's caret, until
     * what the opening sets off is over: the platform's restore of a newly
     * opened editor's state -- its caret and its selection, applied before
     * the platform tells that it opened the file ([told]) -- and the
     * loading of the file's editors ([editors], [loading]), after which the
     * tab scrolls them to the viewer's statement. While it lasts, a caret
     * move in any editor of the file is the opening's, whichever editors
     * `openFile` answered.
     */
    private inner class SyncOpening(val file: VirtualFile) {
        /** The file's editors, waited for once nothing is left to restore; null before. */
        var editors: MutableList<Editor>? = null

        /** Those of [editors] whose loading has not ended. */
        val loading = HashSet<Editor>()

        /** Whether the platform told that it opened the file, after restoring its editors' state. */
        var told = false

        /** Whether the opening made editors whose state the platform restores, so that it waits for [told]. */
        var waitsForTelling = false
    }

    /** The openings not over yet ([SyncOpening]). */
    private val syncOpenings = ArrayList<SyncOpening>()

    /** The file whose one selection change the opening makes is not followed: used up by that one event. */
    private var selectionToken: VirtualFile? = null

    /** How many times the tab built a listing and wrote it into its viewer; tests read it. */
    internal var listingsShown = 0
        private set

    /** Whether its run was the selected one at the last change of [tracedRuns]. */
    private var wasActive = tracedRuns.selected === run

    /** The scrolls waiting for a viewport; the tab's end, the toggle going off and the editor's release end them. */
    private val pendingScrolls = ArrayList<PendingScroll>()

    /** The viewer's wait for its viewport to show; null while none is. */
    private var viewerWait: PendingScroll? = null

    /** Set once the tab is disposed; read by a listing's read off the EDT too. */
    @Volatile private var disposed = false

    /** How many listings the tab asked for: a read delivers only while it is the last asked for. Read off the EDT too. */
    @Volatile private var listingRequests = 0L

    /** The install whose listing the viewer shows or is reading; null while it shows bytecode or nothing. */
    private var shownInstall: CodeEntry? = null

    /** The install whose read failed, which the viewer says why of; null while none is. */
    private var failedInstall: CodeEntry? = null

    /** The message the viewer shows in place of a listing -- while a read that takes long lasts, or why it cannot be -- null while none is. */
    var listingMessage: String? = null
        private set

    /** A queued scroll runs no more once the tab is gone or the toggle is off. */
    private val scrollExpired = Condition<Any?> { disposed || project.isDisposed || !syncWithEditor }

    init {
        Disposer.register(parent) {
            disposed = true
            pendingScrolls.toList().forEach(PendingScroll::cancel)
            EditorFactory.getInstance().releaseEditor(viewer)
        }
        bar.add(title)
        bar.add(chooser)
        bar.add(openButton)
        bar.add(toolbar.component)
        add(bar, BorderLayout.NORTH)
        add(viewer.component, BorderLayout.CENTER)
        chooser.addActionListener { if (!offering) showChosen(retry = true) }
        EditorFactory.getInstance().eventMulticaster.addCaretListener(
            object : CaretListener {
                override fun caretPositionChanged(event: CaretEvent) {
                    if (fromSyncOpening(event.editor)) return
                    caretMoved(event.editor)
                }
            },
            parent,
        )
        project.messageBus.connect(parent).subscribe(
            FileEditorManagerListener.FILE_EDITOR_MANAGER,
            object : FileEditorManagerListener {
                // The platform publishes a selection from a coroutine, never within the input that
                // caused it, so the opening's own one is told by the token and, delivered within
                // the opening's call, by the call itself.
                override fun selectionChanged(event: FileEditorManagerEvent) {
                    val ownSelection = event.newFile != null && event.newFile == selectionToken
                    if (ownSelection) selectionToken = null
                    if (ownSelection || openingForSync) return
                    followSelectedEditor()
                }

                override fun fileOpened(source: FileEditorManager, file: VirtualFile) {
                    deliverOpened(file) {
                        for (opening in syncOpenings.filter { opening -> opening.file == file }) {
                            opening.told = true
                            if (opening.waitsForTelling) startLoading(opening)
                        }
                    }
                }

                // A file closed before its opening was over is scrolled no more, nor filtered.
                override fun fileClosed(source: FileEditorManager, file: VirtualFile) {
                    syncOpenings.removeAll { opening -> opening.file == file }
                }
            },
        )
        EditorFactory.getInstance().addEditorFactoryListener(
            object : EditorFactoryListener {
                // An editor released before its loading ended is waited for no more; an opening left with none is dropped.
                override fun editorReleased(event: EditorFactoryEvent) {
                    pendingScrolls.filter { pending -> pending.editor === event.editor }.forEach(PendingScroll::cancel)
                    for (opening in syncOpenings.toList()) {
                        val editors = opening.editors ?: continue
                        if (!editors.remove(event.editor)) continue
                        opening.loading.remove(event.editor)
                        if (editors.isEmpty()) syncOpenings -= opening else finishIfOver(opening)
                    }
                }
            },
            parent,
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
        offering = true
        try {
            chooser.model = DefaultComboBoxModel(copies.toTypedArray())
            val index = copies.indexOfFirst { copy -> copy.label == chosen }.takeIf { found -> found >= 0 } ?: 0
            if (copies.isNotEmpty()) chooser.selectedIndex = index
        } finally {
            offering = false
        }
        // A body shown afresh -- the gutter's Show Code, the caret's body -- reads a failed copy again; a batch never.
        showChosen(retry = !keepChoice)
    }

    /** The bytecode listing holding [body] -- a block's is its method's -- and every install of [body]'s machine code. */
    private fun copies(body: Int): List<CodeCopy> {
        val copies = ArrayList<CodeCopy>()
        val bytecode = run.bytecode[body] ?: run.bytecode.values.firstOrNull { listed -> listed.bodies.any { part -> part.body == body } }
        if (bytecode != null) copies += CodeCopy(BYTECODE, bytecode)
        // A copy installed again under the same tier and key is numbered, so that a choice kept by its label is the same install.
        val installs = HashMap<String, Int>()
        for (code in run.code[body].orEmpty()) {
            val label = StringBuilder(code.tier)
            code.key?.let { key -> label.append(" key=").append(key) }
            code.entered?.let { entered -> label.append(" entered=").append(entered) }
            val count = installs.merge(label.toString(), 1, Int::plus) ?: 1
            if (count > 1) label.append(" (").append(count).append(')')
            copies += CodeCopy(label.toString(), code)
        }
        return copies
    }

    /**
     * Shows the chosen copy: [retry] where the user chose it or the body was
     * shown afresh, which reads again a copy whose read failed.
     */
    private fun showChosen(retry: Boolean) {
        val copy = chooser.selectedItem as? CodeCopy
        val install = copy?.install
        // The install shown, or being read, is left as it is: a batch that offers the body's copies again
        // keeps the viewer's text, caret and scroll, and reads nothing again.
        if (install != null && install == shownInstall && !(retry && install == failedInstall)) return
        failedInstall = null
        listingsShown++
        val request = ++listingRequests
        shownInstall = install
        if (install == null) {
            display(copy?.bytecode?.let { bytecode -> bytecodeListing(run, bytecode) }, null)
            return
        }
        // The text shown stays until the new one is read, but as text alone: no line of it is linked and
        // "Open in Editor" is off until then, so that nothing takes the old copy for the chosen one.
        // Only a read that takes long says so in its place.
        suspendListing()
        // Built off the EDT too, from a copy of what it names: only the viewer's text is written on it.
        val names = ListingNames.of(run)
        reads.read(
            { !disposed && listingRequests == request },
            { codeListing(names, run.readCode(install)) },
            { display(null, NewspeakBundle.message("devtool.record.reading")) },
        ) { outcome ->
            outcome.fold(
                { shown -> display(shown, null) },
                { error ->
                    // A copy whose read failed is read again when it is chosen again, never by a batch.
                    failedInstall = install
                    display(null, error.message.orEmpty())
                },
            )
        }
    }

    /** Leaves the viewer's text as it is, but no longer as a listing: it links nothing and opens in no editor tab. */
    private fun suspendListing() {
        listing = null
        showListing(document, null)
        openButton.isEnabled = false
        highlighting.refresh()
    }

    /** Writes [shown] into the viewer, or [message] in its place where no listing is shown. */
    private fun display(shown: CodeListing?, message: String?) {
        listing = shown
        openButton.isEnabled = shown != null
        listingMessage = message
        updating = true
        try {
            ApplicationManager.getApplication().runWriteAction { document.setText(shown?.text ?: message.orEmpty()) }
            showListing(document, shown)
        } finally {
            updating = false
        }
        highlighting.refresh()
    }

    /** The width the bar leaves the chooser beside the title and the button, which may be none; null before the tab was laid out. */
    private fun roomForChooser(): Int? {
        if (width <= 0) return null
        val layout = bar.layout as FlowLayout
        val gaps = layout.hgap * BAR_GAPS + bar.insets.left + bar.insets.right
        return width - title.preferredSize.width - openButton.preferredSize.width - toolbar.component.preferredSize.width - gaps
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
            if (editor === viewer && syncWithEditor) highlighting.selected?.let { (body, statement) -> scrollSource(body, statement) }
            return
        }
        // Only a Newspeak file's caret names a statement: another editor's -- a console scrolling with the program's output -- leaves the link as it is.
        val file = FileDocumentManager.getInstance().getFile(document)
        if (file?.extension != NEWSPEAK_EXTENSION) return
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
        if (statement != null && syncWithEditor) scrollViewer()
    }

    /**
     * Scrolls the viewer to the listing lines the selected statement links,
     * once it has an extent -- a tool window never shown lays it out with
     * none -- and then to the statement selected by then; one wait at a
     * time. A viewer laid out once and hidden keeps its extent, and is
     * scrolled at once: shown again at its size, it changes nothing a
     * viewport listener would hear.
     */
    private fun scrollViewer() {
        viewerWait?.cancel()
        viewerWait = null
        val scroll = {
            val lines = highlighting.linkedLines(viewer.document)
            if (lines.isNotEmpty()) {
                scrollIntoView(viewer, viewer.document.getLineStartOffset(lines.first()), viewer.document.getLineEndOffset(lines.last()))
            }
        }
        viewerWait = whenViewportShows(viewer, ::hasExtent) {
            viewerWait = null
            scroll()
        }
    }

    /**
     * Whether a caret move in [editor] comes from the tab's own opening of
     * its file, and so names nothing the user chose: within the opening
     * call, and in an editor of a file whose opening is not over
     * ([SyncOpening]), every move but one made by the user's click or key.
     * Once the opening is over, every move is followed -- a debugger's step
     * among them.
     */
    private fun fromSyncOpening(editor: Editor): Boolean {
        if (openingForSync) return true
        if (syncOpenings.isEmpty()) return false
        val file = FileDocumentManager.getInstance().getFile(editor.document) ?: return false
        if (syncOpenings.none { opening -> opening.file == file }) return false
        return !byUser()
    }

    /**
     * Scrolls every source editor that shows [body] -- the selected one of
     * each editor window, a background tab showing nothing -- to its
     * [statement], their carets where they are; brings the body's file
     * forward, or opens it, without taking the focus from the viewer and
     * without moving its caret, where no editor shows it, and a body the
     * run's records place in no file -- one without a range, or placed in
     * a text that is not the file's ([navigate]'s rule) -- nowhere. A
     * method changed since the run is scrolled to nowhere, as it is
     * linked nowhere.
     */
    private fun scrollSource(body: Int, statement: Int) {
        val described = run.bodies[body] ?: return
        val range = described.statements.getOrNull(statement)?.range ?: return
        val shown = FileEditorManager.getInstance(project).selectedEditors.filterIsInstance<TextEditor>().map(TextEditor::getEditor)
            .filter { editor -> !editor.isDisposed && listingOf(editor.document) == null }
            .mapNotNull { editor ->
                decorations.placedBodies(editor.document, run).firstOrNull { placed -> placed.body.number == body }?.let { placed -> editor to placed }
            }
        if (shown.isNotEmpty()) {
            for ((editor, placed) in shown) {
                placed.place(range)?.let { placedRange -> scrollIntoView(editor, placedRange.startOffset, placedRange.endOffset) }
            }
            return
        }
        if (described.range == null) return
        val path = bodySourcePath(described, libraryDirectory()) ?: return
        withSourceFile(project, path, ::openForSync)
    }

    /**
     * Opens [file] behind the viewer, or brings its tab forward, and
     * scrolls it to the viewer's statement once the opening is over
     * ([SyncOpening]); what the opening sets off in the editor is not
     * followed ([fromSyncOpening], [selectionToken]). Where the file was
     * open before and the call made no editor -- its tab was brought
     * forward -- the platform restores nothing, and the opening is over once
     * the editors are loaded; otherwise the platform tells that it opened
     * the file once the new composite's state is restored. A file found only
     * after the toggle went off or the tab was closed is not opened.
     */
    private fun openForSync(file: VirtualFile) {
        if (disposed || !syncWithEditor) return
        val manager = FileEditorManager.getInstance(project)
        val opening = SyncOpening(file)
        val openBefore = manager.isFileOpen(file)
        val before = manager.getAllEditors(file).toSet()
        val selectedBefore = manager.selectedEditors.toSet()
        // Before the call, which may restore the state and tell of the opening within it.
        syncOpenings += opening
        selectionToken = file
        openingForSync = true
        try {
            manager.openFile(file, false)
        } finally {
            openingForSync = false
        }
        // Nothing opened -- the file gone, or no editor for it -- tells of nothing: the opening ends.
        if (!manager.isFileOpen(file)) {
            syncOpenings -= opening
            selectionToken = null
            return
        }
        // No selection changed, so none will come for the token to take.
        if (manager.selectedEditors.toSet() == selectedBefore) selectionToken = null
        // A file not open before has its state restored whenever its composite gets its editors, and so
        // has one the call made editors for in some window: both wait for the platform's word.
        val made = manager.getAllEditors(file).filterNot(before::contains)
        val restores = !openBefore || made.isNotEmpty()
        if (!restores || opening.told) startLoading(opening) else opening.waitsForTelling = true
    }

    /** Waits for the loading of [opening]'s file's editors, now that nothing is left to restore. */
    private fun startLoading(opening: SyncOpening) {
        if (opening !in syncOpenings || opening.editors != null) return
        val editors = FileEditorManager.getInstance(project).getAllEditors(opening.file).filterIsInstance<TextEditor>().map(TextEditor::getEditor)
        if (editors.isEmpty()) {
            syncOpenings -= opening
            return
        }
        opening.editors = editors.toMutableList()
        opening.loading += editors
        for (editor in editors) {
            whenLoaded(editor) {
                opening.loading -= editor
                finishIfOver(opening)
            }
        }
    }

    private fun finishIfOver(opening: SyncOpening) {
        if (opening in syncOpenings && opening.editors != null && opening.loading.isEmpty()) finish(opening)
    }

    /**
     * Ends [opening], so that its file's editors are followed as any again,
     * and scrolls them, once the platform's own scroll to the restored caret
     * is done ([afterPlatformScroll]), to the statement the viewer's caret
     * names then -- which may have moved on while the file opened.
     */
    private fun finish(opening: SyncOpening) {
        syncOpenings -= opening
        for (editor in opening.editors.orEmpty()) {
            if (editor.isDisposed) continue
            afterPlatformScroll(editor) {
                val (body, statement) = highlighting.selected ?: return@afterPlatformScroll
                val range = run.bodies[body]?.statements?.getOrNull(statement)?.range ?: return@afterPlatformScroll
                val placed = decorations.placedBodies(editor.document, run).firstOrNull { candidate -> candidate.body.number == body }
                placed?.place(range)?.let { placedRange -> scrollIntoView(editor, placedRange.startOffset, placedRange.endOffset) }
            }
        }
    }

    /**
     * Runs [scroll] after the platform's scroll of a newly opened editor to
     * its restored caret. The platform queues that scroll on the event
     * dispatch thread when it restores the editor's state, before it tells
     * that the file was opened, and makes it there at once if the editor's
     * viewport is shown with an extent, else from a viewport listener when
     * it is. So the tab waits for the same readiness and then queues
     * [scroll]: queued after the platform's action, it runs after it, and
     * queued from its own listener, it runs after every listener of that
     * change -- whichever of the two listeners was added first, which
     * matters since a viewport calls the last added first. Both are queued
     * through the same queue (`Dispatchers.EDT` and `invokeLater` both reach
     * `LaterInvocator.invokeLater`), and the platform's scroll does not
     * suspend between its dispatch and the scroll. The wait ends with the
     * tab, the toggle going off and the editor's release, and a queued
     * scroll expires with the first two.
     */
    private fun afterPlatformScroll(editor: Editor, scroll: () -> Unit) {
        // The editor's own modality is never less strict than the platform's dispatch for it, so the
        // queue, which runs what it may in the order queued, holds the tab's back while it holds the platform's.
        val queue = {
            ApplicationManager.getApplication().invokeLater(
                { if (!editor.isDisposed) scroll() },
                ModalityState.stateForComponent(editor.component),
                scrollExpired,
            )
        }
        whenViewportShows(editor, action = queue)
    }

    /**
     * Runs [action] once [editor]'s viewport is [ready] -- by default shown
     * with an extent: at once where it is, else from a viewport listener,
     * which the answered wait holds; null where it ran at once.
     */
    private fun whenViewportShows(editor: Editor, ready: (Editor) -> Boolean = viewportReady, action: () -> Unit): PendingScroll? {
        val viewport = (editor as? EditorEx)?.scrollPane?.viewport
        if (viewport == null || ready(editor)) {
            action()
            return null
        }
        val pending = PendingScroll(editor, viewport)
        pending.listener = ChangeListener {
            if (ready(editor)) {
                pending.cancel()
                action()
            }
        }
        pendingScrolls += pending
        viewport.addChangeListener(pending.listener)
        return pending
    }

    /** A scroll waiting for its editor's viewport to show: its listener, removed when it shows or the wait ends. */
    private inner class PendingScroll(val editor: Editor, val viewport: JViewport) {
        var listener: ChangeListener? = null

        fun cancel() {
            listener?.let(viewport::removeChangeListener)
            pendingScrolls -= this
        }
    }

    /** Ends every opening without scrolling: nothing the openings set off is filtered any more. */
    private fun endSyncOpenings() {
        pendingScrolls.toList().forEach(PendingScroll::cancel)
        viewerWait = null
        syncOpenings.clear()
        selectionToken = null
    }

    /**
     * Scrolls [editor] vertically so that the lines from [startOffset] to
     * [endOffset] are shown, where they are not already: by the least
     * distance, and with the first line at the top where they do not fit.
     */
    private fun scrollIntoView(editor: Editor, startOffset: Int, endOffset: Int) {
        val visible = editor.scrollingModel.visibleArea
        val top = editor.offsetToXY(startOffset).y
        val bottom = editor.offsetToXY(endOffset).y + editor.lineHeight
        if (top >= visible.y && bottom <= visible.y + visible.height) return
        val target = if (top < visible.y || bottom - top > visible.height) top else bottom - visible.height
        editor.scrollingModel.scrollVertically(target)
    }

    private companion object {
        const val BYTECODE = "bytecode"

        /** The project's property that remembers [syncWithEditor]. */
        const val SYNC_PROPERTY = "newspeak.devtool.code.scrollWithEditor"

        /** The place of the bar's toolbar, for the action system. */
        const val TOOLBAR_PLACE = "NewspeakCodeTab"

        /** A flow layout's gaps around and between the bar's four components. */
        const val BAR_GAPS = 5
        const val NEWSPEAK_EXTENSION = "ns"
    }
}

/**
 * The Code tab's chooser of copies. The closed value is painted in the
 * combo's width less what the UI takes beside it -- the right inset, the
 * arrow button as wide as the layout makes it, the padding on both sides
 * (`DarculaComboBoxUI.rectangleForCurrentValue` and `paintCurrentValue`)
 * -- and a label wider than that is cut, while the UI's preferred width
 * counts the arrow as square and so may leave less. So the chooser wants
 * at least its widest label, measured in its own font without the
 * renderer's border, as the UI paints it, plus what the UI takes; bounded
 * by the [room] the tab leaves, null before it was laid out, but never
 * below what its shortest label needs. The labels are measured once per
 * model, font and renderer.
 */
class CopyChooser(private val room: () -> Int?) : ComboBox<CodeCopy>() {
    /** The widths the shortest and the widest label need; null until measured. */
    private var measured: Pair<Int, Int>? = null

    override fun getPreferredSize(): Dimension {
        val size = super.getPreferredSize()
        val (shortest, widest) = measured ?: measure().also { widths -> measured = widths }
        val wanted = maxOf(size.width, widest)
        val bound = room()?.let { width -> maxOf(width, minOf(shortest, wanted)) } ?: wanted
        return Dimension(minOf(wanted, bound), size.height)
    }

    override fun setModel(model: ComboBoxModel<CodeCopy>) {
        measured = null
        super.setModel(model)
    }

    override fun setFont(font: Font?) {
        measured = null
        super.setFont(font)
    }

    override fun setRenderer(renderer: ListCellRenderer<in CodeCopy>?) {
        measured = null
        super.setRenderer(renderer)
    }

    override fun updateUI() {
        measured = null
        super.updateUI()
    }

    /** The combo's width that paints the shortest and the widest label whole. */
    private fun measure(): Pair<Int, Int> {
        val list = JList<CodeCopy>()
        val labels = (0 until itemCount).map { index ->
            val component = renderer.getListCellRendererComponent(list, getItemAt(index), -1, false, false)
            // The UI paints the value in the combo's font, its renderer's border taken away.
            component.font = font
            val insets = (component as? JComponent)?.insets
            component.preferredSize.width - (insets?.let { taken -> taken.left + taken.right } ?: 0)
        }
        if (labels.isEmpty()) return 0 to 0
        val padding = UIManager.getInsets(PADDING)
        val arrow = components.filterIsInstance<JButton>().firstOrNull()?.preferredSize?.width ?: 0
        val taken = insets.right + arrow + (padding?.let { inset -> inset.left + inset.right } ?: 0)
        return labels.min() + taken to labels.max() + taken
    }

    private companion object {
        /** The key the UI's padding is installed from (`BasicComboBoxUI.installDefaults`). */
        const val PADDING = "ComboBox.padding"
    }
}
