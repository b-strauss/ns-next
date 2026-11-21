package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.openapi.Disposable
import com.intellij.openapi.actionSystem.ActionGroup
import com.intellij.openapi.actionSystem.DefaultActionGroup
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.EDT
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.editor.Document
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.Inlay
import com.intellij.openapi.editor.RangeMarker
import com.intellij.openapi.editor.event.DocumentEvent
import com.intellij.openapi.editor.event.DocumentListener
import com.intellij.openapi.editor.event.EditorFactoryEvent
import com.intellij.openapi.editor.event.EditorFactoryListener
import com.intellij.openapi.editor.event.EditorMouseEvent
import com.intellij.openapi.editor.event.EditorMouseMotionListener
import com.intellij.openapi.editor.markup.HighlighterLayer
import com.intellij.openapi.editor.markup.HighlighterTargetArea
import com.intellij.openapi.editor.markup.RangeHighlighter
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.DumbAwareAction
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.openapi.roots.AdditionalLibraryRootsListener
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.Key
import com.intellij.openapi.util.text.StringUtil
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.wm.ToolWindowManager
import com.intellij.util.containers.ContainerUtil
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.TracedRuns
import io.github.bstrauss.newspeak.devtool.WatchedMethods
import io.github.bstrauss.newspeak.devtool.model.Body
import io.github.bstrauss.newspeak.devtool.model.Event
import io.github.bstrauss.newspeak.devtool.model.Send
import io.github.bstrauss.newspeak.devtool.model.SourceRange
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import io.github.bstrauss.newspeak.devtool.ui.RunPanel
import io.github.bstrauss.newspeak.devtool.ui.TOOL_WINDOW_ID
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.nio.file.Path

/**
 * The editor's decorations of the selected traced run
 * (`docs/design/developer-tool.md`, decisions 27, 28, 32 and 33): in every
 * open editor of a body's file -- a program's or the core library's -- a
 * state icon at each method's header and a hint after each send the run
 * says anything of, the blocks' at their own ranges; an eye at each watched
 * method the run has no body of. The data are the trace's, not the PSI's,
 * so the service keeps them itself: a range marker for each body and send,
 * made from the body's record once per document and run, from which every
 * highlighter and inlay is placed. A method whose text in its markers no
 * longer has its records' checksums is stale: its hints go, its icon greys.
 * Read and changed on the EDT only.
 *
 * [libraryDirectory] answers where the core library's files lie, and
 * [settings] whether icons and hints are shown; tests hand in their own,
 * and their own [tracedRuns] and [watchedMethods].
 */
@Service(Service.Level.PROJECT)
class DecorationsService internal constructor(
    private val project: Project,
    private val tracedRuns: TracedRuns,
    private val watchedMethods: WatchedMethods,
    private val libraryDirectory: () -> Path?,
    private val settings: () -> DeveloperToolDecorationsSettings.Settings,
) : Disposable {
    constructor(project: Project) : this(
        project,
        TracedRuns.getInstance(project),
        WatchedMethods.getInstance(project),
        { project.service<NewspeakCoreLibrary>().directory },
        { DeveloperToolDecorationsSettings.getInstance().state },
    )

    /** A send of a body: the marker over its selector's last part, its hint going after the marker's end. */
    private class SendMarker(val send: Send, val marker: RangeMarker)

    /** A body and the marker over its range, as the run's record placed it. */
    private class BodyMarker(val body: Body, val marker: RangeMarker, val sends: List<SendMarker>)

    /** A method of a document: the bodies of its family, its root's among them where the run described it. */
    private class MethodMarkers {
        val bodies = ArrayList<BodyMarker>()

        /** Whether the text of one of [bodies] no longer has its record's checksum. */
        var stale = false

        val root: BodyMarker? get() = bodies.firstOrNull { marker -> marker.body.family == null }
    }

    /**
     * A document's markers for the decorated [run]: each method by its
     * root's number, and every body number placed or judged another file's.
     * Kept in the document's user data for the whole run, so that an editor
     * closed and opened again finds the markers that followed every edit.
     */
    private class DocumentMarkers(val run: TraceRun) {
        val methods = LinkedHashMap<Int, MethodMarkers>()
        val seen = HashSet<Int>()

        /** Forgets the bodies judged another file's, whose file a new library directory may make this one. */
        fun forgetOtherFiles() {
            seen.retainAll(methods.values.flatMap { method -> method.bodies.map { body -> body.body.number } }.toSet())
        }

        fun dispose() {
            for (method in methods.values) for (body in method.bodies) {
                body.marker.dispose()
                for (send in body.sends) send.marker.dispose()
            }
            methods.clear()
        }
    }

    /** What the service put into an editor, which it takes out again before it decorates the editor afresh. */
    private class EditorDecorations {
        val highlighters = ArrayList<RangeHighlighter>()
        val inlays = ArrayList<Inlay<*>>()
    }

    /** What the tooltips read off the run's events, made once per batch. */
    private class Summary(run: TraceRun) {
        val deoptimizations = HashMap<Int, Int>()
        val lastDeoptimization = HashMap<Int, Event>()
        val inliningAt = HashMap<Pair<Int, Int>, Event>()
        val transitionAt = HashMap<Pair<Int, Int>, Event>()

        init {
            for (event in run.events) {
                val body = event.body
                when {
                    event.event == "inlined" || event.event == "declined" -> {
                        val inBody = event.integer("in-body")
                        val at = event.integer("at")
                        if (inBody != null && at != null) inliningAt[inBody to at] = event
                    }
                    event.event == "transition" -> {
                        val site = event.integer("site")
                        if (body != null && site != null) transitionAt[body to site] = event
                    }
                    event.category == "deoptimize" && body != null -> {
                        val root = run.bodies[body]?.family ?: body
                        deoptimizations.merge(root, 1, Int::plus)
                        lastDeoptimization[root] = event
                    }
                }
            }
        }
    }

    private var run: TraceRun? = null
    private var runDisposable: Disposable? = null
    private var summary: Summary? = null
    /** The documents holding markers of the decorated run, held weakly: the markers themselves are in each document's user data. */
    private val documents: MutableSet<Document> = ContainerUtil.createWeakSet()
    private val editors = HashMap<Editor, EditorDecorations>()

    /** Whether the content component's tooltip is a hint's, which leaving the hint takes away. */
    private var hintTooltipShown = false

    /**
     * This service's markers in a document's user data. A key of its own,
     * since a core library file is one document shared by every open
     * project, each decorating it with its own run: one project's markers
     * must never replace, nor its clear dispose, another's.
     */
    private val markersKey = Key.create<DocumentMarkers>("newspeak.devtool.decorations.${System.identityHashCode(this)}")

    init {
        val factory = EditorFactory.getInstance()
        factory.addEditorFactoryListener(
            object : EditorFactoryListener {
                override fun editorCreated(event: EditorFactoryEvent) {
                    if (event.editor.project === project) decorate(event.editor)
                }

                override fun editorReleased(event: EditorFactoryEvent) {
                    // The document keeps its markers: they follow its edits until the run is no longer decorated.
                    if (event.editor.project === project) editors.remove(event.editor)
                }
            },
            this,
        )
        factory.eventMulticaster.addDocumentListener(
            object : DocumentListener {
                override fun documentChanged(event: DocumentEvent) {
                    changed(event)
                }
            },
            this,
        )
        factory.eventMulticaster.addEditorMouseMotionListener(
            object : EditorMouseMotionListener {
                override fun mouseMoved(event: EditorMouseEvent) {
                    hover(event)
                }
            },
            this,
        )
        // A library directory that changed may hold the files of the library's bodies.
        project.messageBus.connect(this).subscribe(
            AdditionalLibraryRootsListener.TOPIC,
            AdditionalLibraryRootsListener { _, _, _, libraryName ->
                if (libraryName == NewspeakCoreLibrary.NAME) {
                    ApplicationManager.getApplication().invokeLater(
                        {
                            for (document in documents) document.getUserData(markersKey)?.forgetOtherFiles()
                            decorateAll()
                        },
                        project.disposed,
                    )
                }
            },
        )
        tracedRuns.addListener({ follow() }, this)
        watchedMethods.addListener({ decorateAll() }, this)
        follow()
    }

    /** Shows the settings' icons and hints, or takes them away: the settings page's apply. */
    fun settingsChanged() = decorateAll()

    override fun dispose() {
        if (ApplicationManager.getApplication().isDispatchThread) clear()
    }

    /** Decorates with the selected run, if it is another than the one decorated. */
    private fun follow() {
        val selected = tracedRuns.selected
        if (selected === run) return
        clear()
        run = selected
        if (selected != null) {
            val disposable = Disposer.newDisposable(this, "decorations of ${selected.name}")
            runDisposable = disposable
            selected.onChanged(disposable) {
                summary = null
                decorateAll()
            }
        }
        decorateAll()
    }

    /** Takes every decoration and marker away, and stops following the decorated run. */
    private fun clear() {
        for ((editor, decorations) in editors) if (!editor.isDisposed) remove(decorations)
        editors.clear()
        for (document in documents) {
            document.getUserData(markersKey)?.dispose()
            document.putUserData(markersKey, null)
        }
        documents.clear()
        runDisposable?.let(Disposer::dispose)
        runDisposable = null
        summary = null
    }

    private fun decorateAll() {
        for (editor in EditorFactory.getInstance().allEditors) if (editor.project === project) decorate(editor)
    }

    private fun remove(decorations: EditorDecorations) {
        decorations.highlighters.forEach(RangeHighlighter::dispose)
        decorations.highlighters.clear()
        decorations.inlays.forEach(Disposer::dispose)
        decorations.inlays.clear()
    }

    /** Puts [editor]'s decorations afresh: its file's methods' icons and hints, and the eyes. */
    private fun decorate(editor: Editor) {
        if (editor.isDisposed) return
        val document = editor.document
        val file = FileDocumentManager.getInstance().getFile(document) ?: return
        if (file.extension != NEWSPEAK_EXTENSION) return
        val decorations = editors.getOrPut(editor) { EditorDecorations() }
        remove(decorations)
        val current = run
        val methods = current?.let { traced -> markersOf(document, file, traced).methods.values }.orEmpty()
        val showIcons = settings().showStateIcons
        val showHints = settings().showSendHints
        val iconOffsets = ArrayList<Int>()
        if (current != null) {
            for (method in methods) {
                val root = method.root
                if (showIcons && root != null && root.marker.isValid) {
                    decorations.highlighters += icon(editor, root.marker.startOffset, stateIcon(current, method, root.body))
                    iconOffsets += root.marker.startOffset
                }
                if (showHints && !method.stale) for (body in method.bodies) hints(editor, current, body, decorations)
            }
        }
        eyes(editor, file, iconOffsets, decorations)
    }

    /** The markers of [document]'s bodies in [traced], made for each body the first time the document is decorated with it. */
    private fun markersOf(document: Document, file: VirtualFile, traced: TraceRun): DocumentMarkers {
        val markers = document.getUserData(markersKey)?.takeIf { kept -> kept.run === traced } ?: DocumentMarkers(traced).also { made ->
            document.putUserData(markersKey, made)
            documents.add(document)
        }
        val path = file.fileSystem.getNioPath(file) ?: return markers
        val touched = HashSet<MethodMarkers>()
        for (body in traced.bodies.values) {
            if (body.number in markers.seen) continue
            // A library body is looked at again once the library has a directory.
            val source = sourcePath(body) ?: continue
            if (!sameFile(source, path)) {
                markers.seen.add(body.number)
                continue
            }
            // A range outside the document's text is never placed: nothing guesses where the body went.
            val marker = body.range?.let { range -> marker(document, range) } ?: continue
            markers.seen.add(body.number)
            val sends = body.sends.mapNotNull { send ->
                val selector = send.selector.lastOrNull { token -> token != null } ?: send.call
                selector?.let { range -> marker(document, range) }?.let { sendMarker -> SendMarker(send, sendMarker) }
            }
            val method = markers.methods.getOrPut(body.family ?: body.number) { MethodMarkers() }
            method.bodies += BodyMarker(body, marker, sends)
            touched += method
        }
        // A body seen for the first time is checked against the text it is seen in.
        for (method in touched) method.stale = isStale(document, method)
        return markers
    }

    private fun sourcePath(body: Body): Path? = bodySourcePath(body, libraryDirectory())

    /** A marker over [range] of [document], null where the range lies outside its text. */
    private fun marker(document: Document, range: SourceRange): RangeMarker? =
        documentRange(document, range)?.let { offsets -> document.createRangeMarker(offsets) }

    private fun isStale(document: Document, method: MethodMarkers): Boolean = method.bodies.any { body ->
        val marker = body.marker
        !marker.isValid || sourceChecksum(document.charsSequence.subSequence(marker.startOffset, marker.endOffset)) != body.body.checksum
    }

    /** Re-checks the methods [event] touched; a method that became stale, or the run's again, redecorates its document's editors. */
    private fun changed(event: DocumentEvent) {
        val markers = event.document.getUserData(markersKey)?.takeIf { kept -> kept.run === run } ?: return
        val changeEnd = event.offset + event.newLength
        var flipped = false
        for (method in markers.methods.values) {
            val touched = method.bodies.any { body ->
                !body.marker.isValid || (body.marker.startOffset <= changeEnd && event.offset <= body.marker.endOffset)
            }
            if (!touched) continue
            val stale = isStale(event.document, method)
            if (stale != method.stale) {
                method.stale = stale
                flipped = true
            }
        }
        if (flipped) for (editor in EditorFactory.getInstance().getEditors(event.document, project)) decorate(editor)
    }

    private fun icon(editor: Editor, offset: Int, renderer: StateGutterRenderer): RangeHighlighter =
        editor.markupModel.addRangeHighlighter(null, offset, offset, HighlighterLayer.LAST, HighlighterTargetArea.LINES_IN_RANGE).apply {
            gutterIconRenderer = renderer
        }

    private fun stateIcon(traced: TraceRun, method: MethodMarkers, root: Body): StateGutterRenderer {
        val watched = watchedMethods.isWatched(root.name)
        val actions = methodActions(traced, root)
        if (method.stale) return StateGutterRenderer(traced.state(root.number), stale = true, watched = watched, NewspeakBundle.message("devtool.stale"), actions)
        val state = traced.state(root.number)
        val summary = summary()
        var tooltip = NewspeakBundle.message(
            "devtool.gutter.tooltip",
            state.name.lowercase(),
            traced.copies(root.number),
            summary.deoptimizations[root.number] ?: 0,
        )
        summary.lastDeoptimization[root.number]?.let { last ->
            val reason = last.text("reason") ?: last.text("cause")
            if (reason != null) tooltip += NewspeakBundle.message("devtool.gutter.last", last.event, reason)
        }
        return StateGutterRenderer(state, stale = false, watched = watched, tooltip, actions)
    }

    private fun summary(): Summary = summary ?: Summary(checkNotNull(run)).also { made -> summary = made }

    private fun hints(editor: Editor, traced: TraceRun, body: BodyMarker, decorations: EditorDecorations) {
        val outcomes = traced.sends(body.body.number)
        if (outcomes.isEmpty()) return
        val root = body.body.family ?: body.body.number
        for (send in body.sends) {
            val outcome = outcomes[send.send.at] ?: continue
            if (!send.marker.isValid) continue
            val text = listOfNotNull(outcome.inlining, outcome.cache).joinToString(" · ")
            val events = listOfNotNull(
                summary().inliningAt[body.body.number to send.send.at],
                send.send.site?.let { site -> summary().transitionAt[root to site] },
            )
            val tooltip = "<html>" + events.joinToString("<br>") { event -> StringUtil.escapeXmlEntities(eventLine(event)) } + "</html>"
            editor.inlayModel.addInlineElement(send.marker.endOffset, true, SendInlayRenderer(text, tooltip))?.let(decorations.inlays::add)
        }
    }

    /** [event] as `[category] event name=value…`, its fields as the file has them. */
    private fun eventLine(event: Event): String = buildString {
        append("[").append(event.category).append("] ").append(event.event)
        for ((name, value) in event.fields.entrySet()) {
            append(' ').append(name).append('=').append(if (value.isJsonPrimitive) value.asString else value.toString())
        }
    }

    /**
     * The eyes: at each watched method of [file] without an icon at its
     * declaration, at its pattern's start. The names are the server's, asked
     * for off the event dispatch thread when the document has none kept;
     * their answer decorates again.
     */
    private fun eyes(editor: Editor, file: VirtualFile, iconOffsets: List<Int>, decorations: EditorDecorations) {
        if (watchedMethods.entries.isEmpty()) return
        val names = watchedMethods.currentMethodNames(file)
        if (names == null) {
            watchedMethods.request(file)
            return
        }
        val length = editor.document.textLength
        for (method in names) {
            if (!watchedMethods.isWatched(method.name)) continue
            if (iconOffsets.any { offset -> method.range.containsOffset(offset) }) continue
            val offset = method.selection.startOffset.coerceIn(0, length)
            val actions = DefaultActionGroup(toggleAction(method.name))
            decorations.highlighters += icon(editor, offset, StateGutterRenderer(null, stale = false, watched = true, NewspeakBundle.message("devtool.gutter.watched"), actions))
        }
    }

    /** The menu of decision 28: watch or unwatch, show the code, show the inlining, open the tool window at the method. */
    private fun methodActions(traced: TraceRun, root: Body): ActionGroup = DefaultActionGroup(
        toggleAction(root.name),
        DumbAwareAction.create(NewspeakBundle.message("devtool.gutter.show.code")) { show(traced, root.number, RunPanel.View.CODE) },
        DumbAwareAction.create(NewspeakBundle.message("devtool.gutter.show.inlining")) { show(traced, root.number, RunPanel.View.INLINING) },
        DumbAwareAction.create(NewspeakBundle.message("devtool.gutter.open")) { show(traced, root.number, RunPanel.View.METHODS) },
    )

    private fun toggleAction(name: String) = DumbAwareAction.create(
        NewspeakBundle.message(if (watchedMethods.hasEntry(name)) "devtool.watch.method.action.unwatch" else "devtool.watch.method.action.watch"),
    ) { watchedMethods.toggle(name) }

    private fun show(traced: TraceRun, body: Int, view: RunPanel.View) {
        // A run whose tab was closed meanwhile has nothing to show.
        if (tracedRuns.runs.none { candidate -> candidate === traced }) return
        tracedRuns.selected = traced
        val toolWindow = ToolWindowManager.getInstance(project).getToolWindow(TOOL_WINDOW_ID) ?: return
        toolWindow.activate { tracedRuns.panel(traced)?.show(view, body) }
    }

    /** Shows a hint's fields while the mouse is over it. */
    private fun hover(event: EditorMouseEvent) {
        val editor = event.editor
        if (editor.project !== project) return
        val renderer = event.inlay?.renderer as? SendInlayRenderer
        if (renderer != null) {
            editor.contentComponent.toolTipText = renderer.tooltip
            hintTooltipShown = true
        } else if (hintTooltipShown) {
            editor.contentComponent.toolTipText = null
            hintTooltipShown = false
        }
    }

    /** The run decorated: the selected one, whose bodies [placedBodies] places; null while none is. */
    val decoratedRun: TraceRun? get() = run

    /**
     * The bodies of [traced] in [document] where it is the decorated run,
     * each at its marker's offsets, which followed every edit since the run,
     * and stale where its method's text no longer has the records'
     * checksums; none for another run and a document no body is from. The
     * Code tab's caret and linked highlighting place themselves by these, so
     * that nothing reads the run's ranges against a text they no longer
     * describe.
     */
    fun placedBodies(document: Document, traced: TraceRun): List<PlacedBody> {
        if (traced !== run) return emptyList()
        val file = FileDocumentManager.getInstance().getFile(document) ?: return emptyList()
        if (file.extension != NEWSPEAK_EXTENSION) return emptyList()
        return markersOf(document, file, traced).methods.values.flatMap { method ->
            method.bodies.filter { body -> body.marker.isValid }.map { body ->
                PlacedBody(body.body, document, body.marker.textRange, method.stale)
            }
        }
    }

    companion object {
        fun getInstance(project: Project): DecorationsService = project.service()

        private const val NEWSPEAK_EXTENSION = "ns"
    }
}

/** Starts the project's decorations as it opens, so that the eyes show before any run. */
class DecorationsStartup : ProjectActivity {
    override suspend fun execute(project: Project) {
        withContext(Dispatchers.EDT) {
            if (!project.isDisposed) DecorationsService.getInstance(project)
        }
    }
}

/**
 * The checksum of a body's source as the runtime computes it: FNV-1a 64
 * over the UTF-8 bytes of [text] but every `\r`, 16 lower-case hex digits
 * (decision 18) -- so that a file with CRLF line ends, which a document
 * holds with `\n`, matches the record.
 */
fun sourceChecksum(text: CharSequence): String {
    var hash = FNV_OFFSET_BASIS
    for (byte in text.toString().toByteArray(Charsets.UTF_8)) {
        if (byte == CARRIAGE_RETURN) continue
        hash = (hash xor (byte.toULong() and BYTE_MASK)) * FNV_PRIME
    }
    return java.lang.Long.toHexString(hash.toLong()).padStart(CHECKSUM_DIGITS, '0')
}

private const val FNV_OFFSET_BASIS: ULong = 0xcbf29ce484222325uL
private const val FNV_PRIME: ULong = 0x100000001b3uL
private const val BYTE_MASK: ULong = 0xffuL
private const val CHECKSUM_DIGITS = 16
private const val CARRIAGE_RETURN = '\r'.code.toByte()
