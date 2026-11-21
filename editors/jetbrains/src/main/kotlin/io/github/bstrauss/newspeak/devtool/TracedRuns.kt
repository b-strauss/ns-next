package io.github.bstrauss.newspeak.devtool

import com.intellij.execution.executors.DefaultRunExecutor
import com.intellij.execution.process.ProcessEvent
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessListener
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.options.advanced.AdvancedSettings
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.wm.ToolWindowManager
import com.intellij.ui.content.Content
import com.intellij.ui.content.ContentFactory
import com.intellij.ui.content.ContentManager
import com.intellij.ui.content.ContentManagerEvent
import com.intellij.ui.content.ContentManagerListener
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import io.github.bstrauss.newspeak.devtool.model.TraceFileReader
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import io.github.bstrauss.newspeak.devtool.ui.RunPanel
import io.github.bstrauss.newspeak.devtool.ui.TOOL_WINDOW_ID
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import io.github.bstrauss.newspeak.run.writesTrace
import java.nio.file.Path
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

private val TIME_OF_DAY: DateTimeFormatter = DateTimeFormatter.ofPattern("HH:mm:ss")

/**
 * The project's traced runs (`docs/design/developer-tool.md`, decisions 24
 * and 26): every Run or Debug that traces or lists anything writes its trace
 * into a temporary file of its own, which a [TraceFileReader] follows into
 * the run's [TraceRun], shown as a tab of the "Newspeak JIT" tool window.
 * The newest run is [selected], whose decorations the editor shows; closing
 * its tab drops the run and its reader, and its file once its process has
 * ended, and closing the project drops them all. A new run of a
 * configuration replaces the tab of its last run that has ended and is
 * not pinned, as the Run tool window reuses its tabs (decision 35). Read
 * and changed on the EDT only.
 *
 * [contentManager] answers the tool window's contents, null where the
 * project has none, and [traceFiles] makes and lets go the runs' files;
 * tests hand in their own.
 */
@Service(Service.Level.PROJECT)
class TracedRuns internal constructor(
    private val project: Project,
    private val traceFiles: TraceFiles = TraceFiles.getInstance(),
    private val contentManager: () -> ContentManager?,
) : Disposable {
    constructor(project: Project) : this(
        project,
        contentManager = { ToolWindowManager.getInstance(project).getToolWindow(TOOL_WINDOW_ID)?.contentManager },
    )

    private var disposed = false

    /**
     * One run: the name of the configuration it ran and the executor it
     * ran under, its process, its file, what releases its reader and file,
     * and its tab, if the tool window has one.
     */
    private class Traced(
        val configurationName: String,
        val executorId: String,
        val handler: ProcessHandler,
        val run: TraceRun,
        val file: Path,
        val disposable: Disposable,
    ) {
        var content: Content? = null
        var panel: RunPanel? = null
        var error: String? = null
    }

    private val traced = ArrayList<Traced>()
    private val listeners = ArrayList<() -> Unit>()

    /** The content manager this service listens to, once it added a tab to it. */
    private var listenedManager: ContentManager? = null

    /** Every run whose tab is open, oldest first. */
    val runs: List<TraceRun> get() = traced.map(Traced::run)

    /** The run the editor shows, the newest by default; null without any. Selecting one selects its tab. */
    var selected: TraceRun? = null
        set(run) {
            if (field === run) return
            field = run
            val content = traced.firstOrNull { entry -> entry.run === run }?.content
            content?.manager?.let { manager -> if (manager.selectedContent !== content) manager.setSelectedContent(content) }
            changed()
        }

    /** [run]'s tab, null for a run without one: closed, or started while the project had no tool window. */
    fun panel(run: TraceRun): RunPanel? = traced.firstOrNull { entry -> entry.run === run }?.panel

    /** The reader's error for [run], which ended its reading; null while it reads. */
    fun error(run: TraceRun): String? = traced.firstOrNull { entry -> entry.run === run }?.error

    /**
     * Starts a run of [configuration] under the executor [executorId]
     * narrowed by [filters] through [startProcess], handing it the trace
     * file to pass as `--trace-file` -- null for a run that traces and lists
     * nothing, which starts no reader and opens no tab -- and answers its
     * handler.
     */
    fun start(
        configuration: NewspeakRunConfiguration,
        filters: TraceFilters,
        executorId: String = DefaultRunExecutor.EXECUTOR_ID,
        startProcess: (traceFile: Path?) -> ProcessHandler,
    ): ProcessHandler {
        if (!writesTrace(configuration, filters)) return startProcess(null)
        val file = traceFiles.newFile()
        val handler = try {
            startProcess(file)
        } catch (exception: Exception) {
            traceFiles.release(file)
            throw exception
        }
        val application = ApplicationManager.getApplication()
        if (application.isDispatchThread) {
            register(configuration, executorId, file, handler, filters)
        } else {
            // Never expired: a project closed meanwhile still lets its file go.
            application.invokeLater { register(configuration, executorId, file, handler, filters) }
        }
        return handler
    }

    /** [started], or, for a project closed since the process started, the file let go once the process has ended: no tab will ever own it. */
    private fun register(configuration: NewspeakRunConfiguration, executorId: String, file: Path, handler: ProcessHandler, filters: TraceFilters) {
        if (disposed || project.isDisposed) releaseOnceEnded(file, handler) else started(configuration, file, handler, filters, executorId)
    }

    /**
     * A traced run of [configuration] under the executor [executorId] has
     * started [handler], writing into [file] narrowed by [filters]: its
     * [TraceRun], named by the
     * configuration and the time, a reader following the file into it to its
     * end once the process terminates, and its tab, selected, which takes
     * the place of the tab it replaces ([replaceable]).
     */
    fun started(
        configuration: NewspeakRunConfiguration,
        file: Path,
        handler: ProcessHandler,
        filters: TraceFilters = TraceFilters.EVERYTHING,
        executorId: String = DefaultRunExecutor.EXECUTOR_ID,
    ): TraceRun {
        val startedAt = Instant.now()
        val run = TraceRun("${configuration.name} ${TIME_OF_DAY.format(startedAt.atZone(ZoneId.systemDefault()))}", startedAt, filters, file)
        val disposable = Disposer.newDisposable(this, "traced run ${run.name}")
        val entry = Traced(configuration.name, executorId, handler, run, file, disposable)
        val reader = TraceFileReader(
            file,
            run::add,
            { message ->
                entry.error = message
                entry.panel?.showError(message)
                changed()
            },
            run::ended,
        )
        // Registered before the reader, so disposed after it: the file outlives what reads it, and what writes it.
        Disposer.register(disposable) { releaseOnceEnded(file, handler) }
        Disposer.register(disposable, reader)
        handler.addProcessListener(
            object : ProcessListener {
                override fun processTerminated(event: ProcessEvent) {
                    reader.finish()
                }
            },
            disposable,
        )
        // A process that ended before the run was registered has written everything it will.
        if (handler.isProcessTerminated) reader.finish()
        val manager = contentManager()
        val replaced = replaceable(configuration.name, executorId, manager)
        val place = replaced?.content?.let { content -> manager?.getIndexOfContent(content) }?.takeIf { index -> index >= 0 }
        traced.add(entry)
        // The window is not shown: it shares the bottom with Run and Debug, whose console it would hide.
        manager?.let { contents -> addTab(contents, entry, place) }
        selected = run
        // Closed only once the new run is selected, so that the selection never passes through another run.
        replaced?.let { previous -> close(previous.run) }
        return run
    }

    /**
     * The run a new run of the configuration named [configurationName]
     * under the executor [executorId] replaces, as the Run and Debug tool
     * windows choose the tab they reuse (`RunContentManagerImpl`'s
     * `getContentFromManager`): a run of the same configuration, by its
     * name, under the same executor -- the platform's windows are one per
     * executor --, whose process has ended and whose tab is not pinned,
     * the selected tab first, then the first in the tabs' order, a run
     * without a tab last; null where none is. A run whose process goes on
     * is never replaced, nor another configuration's.
     */
    private fun replaceable(configurationName: String, executorId: String, manager: ContentManager?): Traced? {
        val candidates = traced.filter { entry ->
            entry.configurationName == configurationName && entry.executorId == executorId &&
                entry.handler.isProcessTerminated && entry.content?.isPinned != true
        }
        val tabs = manager?.contents?.toList().orEmpty()
        val inTabOrder = candidates.sortedBy { entry -> tabs.indexOf(entry.content).takeIf { index -> index >= 0 } ?: Int.MAX_VALUE }
        val selectedContent = manager?.selectedContent
        return inTabOrder.firstOrNull { entry -> entry.content != null && entry.content === selectedContent } ?: inTabOrder.firstOrNull()
    }

    /**
     * Lets [file] go once [handler]'s process has ended: a tab closed while
     * its run goes on drops its reader at once, and the file, which the
     * process still writes, when the process ends, so that no file is left
     * behind that a deleted name no longer reaches.
     */
    private fun releaseOnceEnded(file: Path, handler: ProcessHandler) {
        if (handler.isProcessTerminated) {
            traceFiles.release(file)
            return
        }
        handler.addProcessListener(
            object : ProcessListener {
                override fun processTerminated(event: ProcessEvent) {
                    traceFiles.release(file)
                }
            },
        )
        // A process that ended while the listener was added is told of by neither.
        if (handler.isProcessTerminated) traceFiles.release(file)
    }

    /** Drops [run]: its tab and its reader at once, and its file once its process has ended. */
    fun close(run: TraceRun) {
        val entry = traced.firstOrNull { candidate -> candidate.run === run } ?: return
        traced.remove(entry)
        Disposer.dispose(entry.disposable)
        CodeListingFile.unlinkListingsOf(run)
        entry.content?.let { content -> content.manager?.removeContent(content, true) }
        if (selected === run) selected = traced.lastOrNull()?.run
        changed()
    }

    /** Calls [listener] whenever a run is added, closed or selected, or its reading fails, until [parent] is disposed. */
    fun addListener(listener: () -> Unit, parent: Disposable) {
        listeners.add(listener)
        Disposer.register(parent) { listeners.remove(listener) }
    }

    override fun dispose() {
        disposed = true
        for (entry in traced) CodeListingFile.unlinkListingsOf(entry.run)
        traced.clear()
        listeners.clear()
    }

    /** Adds [entry]'s tab to [manager], at [place] where it replaces one, last otherwise. */
    private fun addTab(manager: ContentManager, entry: Traced, place: Int?) {
        listenTo(manager)
        val panel = RunPanel(project, entry.run).also { created -> Disposer.register(entry.disposable, created) }
        entry.error?.let(panel::showError)
        val content = ContentFactory.getInstance().createContent(panel, entry.run.name, false)
        content.isCloseable = true
        // The tab's menu offers Pin Tab, by which a run is kept from being replaced, as a Run tab's is,
        // and the setting that starts the Run window's tabs pinned starts these pinned too.
        content.isPinnable = true
        if (AdvancedSettings.getBoolean(START_PINNED)) content.isPinned = true
        entry.panel = panel
        entry.content = content
        if (place != null && place <= manager.contentCount) manager.addContent(content, place) else manager.addContent(content)
    }

    private fun listenTo(manager: ContentManager) {
        if (listenedManager === manager) return
        listenedManager = manager
        manager.addContentManagerListener(
            object : ContentManagerListener {
                override fun contentRemoved(event: ContentManagerEvent) {
                    traced.firstOrNull { entry -> entry.content === event.content }?.let { entry -> close(entry.run) }
                }

                override fun selectionChanged(event: ContentManagerEvent) {
                    if (event.operation != ContentManagerEvent.ContentOperation.add) return
                    traced.firstOrNull { entry -> entry.content === event.content }?.let { entry -> selected = entry.run }
                }
            },
        )
    }

    private fun changed() {
        for (listener in listeners.toList()) listener()
    }

    companion object {
        fun getInstance(project: Project): TracedRuns = project.service()

        /** The advanced setting by which the platform starts every run's tab pinned (`RunContentManagerImpl`'s `createNewContent`). */
        internal const val START_PINNED = "start.run.configurations.pinned"
    }
}
