package io.github.bstrauss.newspeak.devtool

import com.intellij.execution.process.ProcessEvent
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessListener
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
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
 * its tab drops the run, its reader and its file, and closing the project
 * drops them all. Read and changed on the EDT only.
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

    /** One run: its file, what releases its reader and file, and its tab, if the tool window has one. */
    private class Traced(val run: TraceRun, val file: Path, val disposable: Disposable) {
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
     * Starts a run of [configuration] narrowed by [filters] through
     * [startProcess], handing it the trace file to pass as `--trace-file` --
     * null for a run that traces and lists nothing, which starts no reader
     * and opens no tab -- and answers its handler.
     */
    fun start(
        configuration: NewspeakRunConfiguration,
        filters: TraceFilters,
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
            register(configuration, file, handler, filters)
        } else {
            // Never expired: a project closed meanwhile still lets its file go.
            application.invokeLater { register(configuration, file, handler, filters) }
        }
        return handler
    }

    /** [started], or, for a project closed since the process started, the file let go: no tab will ever own it. */
    private fun register(configuration: NewspeakRunConfiguration, file: Path, handler: ProcessHandler, filters: TraceFilters) {
        if (disposed || project.isDisposed) traceFiles.release(file) else started(configuration, file, handler, filters)
    }

    /**
     * A traced run of [configuration] has started [handler], writing into
     * [file] narrowed by [filters]: its [TraceRun], named by the
     * configuration and the time, a reader following the file into it to its
     * end once the process terminates, and its tab, selected.
     */
    fun started(
        configuration: NewspeakRunConfiguration,
        file: Path,
        handler: ProcessHandler,
        filters: TraceFilters = TraceFilters.EVERYTHING,
    ): TraceRun {
        val startedAt = Instant.now()
        val run = TraceRun("${configuration.name} ${TIME_OF_DAY.format(startedAt.atZone(ZoneId.systemDefault()))}", startedAt, filters)
        val disposable = Disposer.newDisposable(this, "traced run ${run.name}")
        val entry = Traced(run, file, disposable)
        val reader = TraceFileReader(file, run::add) { message ->
            entry.error = message
            entry.panel?.showError(message)
            changed()
        }
        // Registered before the reader, so disposed after it: the file outlives what reads it.
        Disposer.register(disposable) { traceFiles.release(file) }
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
        traced.add(entry)
        // The window is not shown: it shares the bottom with Run and Debug, whose console it would hide.
        contentManager()?.let { manager -> addTab(manager, entry) }
        selected = run
        return run
    }

    /** Drops [run]: its tab, its reader and its file. */
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

    private fun addTab(manager: ContentManager, entry: Traced) {
        listenTo(manager)
        val panel = RunPanel(project, entry.run).also { created -> Disposer.register(entry.disposable, created) }
        entry.error?.let(panel::showError)
        val content = ContentFactory.getInstance().createContent(panel, entry.run.name, false)
        content.isCloseable = true
        entry.panel = panel
        entry.content = content
        manager.addContent(content)
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
    }
}
