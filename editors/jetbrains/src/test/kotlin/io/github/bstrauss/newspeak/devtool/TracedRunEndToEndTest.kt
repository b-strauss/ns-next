package io.github.bstrauss.newspeak.devtool

import com.intellij.execution.ExecutionListener
import com.intellij.execution.ExecutionManager
import com.intellij.execution.ProgramRunnerUtil
import com.intellij.execution.RunManager
import com.intellij.execution.executors.DefaultDebugExecutor
import com.intellij.execution.executors.DefaultRunExecutor
import com.intellij.execution.process.ProcessEvent
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessListener
import com.intellij.execution.process.ProcessOutputTypes
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.execution.ui.RunContentManager
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.LogicalPosition
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.Key
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.testFramework.runInEdtAndGet
import com.intellij.testFramework.runInEdtAndWait
import com.intellij.xdebugger.XDebugProcess
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.XDebuggerManagerListener
import io.github.bstrauss.newspeak.devtool.editor.DecorationsService
import io.github.bstrauss.newspeak.devtool.editor.StateGutterRenderer
import io.github.bstrauss.newspeak.devtool.model.MethodState
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import io.github.bstrauss.newspeak.devtool.ui.CodeTab
import io.github.bstrauss.newspeak.devtool.ui.InliningTab
import io.github.bstrauss.newspeak.library.CoreLibraryStore
import io.github.bstrauss.newspeak.run.NewspeakConfigurationType
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit

/**
 * A traced run end to end, against the real runtime the build names in
 * `nsNext.runtime`: Run on a Newspeak run configuration whose Developer
 * Tool tab asks for the `compile` and `code-zone` categories -- a baseline
 * tier is shown only where its end can be heard -- and machine code, with one
 * method watched, through the platform's runner as the user starts it; the
 * trace file the runtime writes is read into the selected run, and the
 * editor of the program shows the method's state.
 *
 * The test runs off the event dispatch thread, so that it may wait for the
 * process and the reader while the platform works there. Every wait is
 * bounded.
 */
class TracedRunEndToEndTest : BasePlatformTestCase() {
    // Read once a test runs: JUnit 3 makes an instance of every test even where a filter leaves the class out.
    private val runtime: Path by lazy { Path.of(System.getProperty("nsNext.runtime")) }
    private lateinit var directory: Path
    private val handlers = ArrayList<ProcessHandler>()

    override fun runInDispatchThread(): Boolean = false

    override fun setUp() {
        super.setUp()
        directory = Files.createTempDirectory("ns-next-traced-run")
        NewspeakSettings.getInstance().state.runtimePath = runtime.toString()
    }

    override fun tearDown() {
        try {
            for (handler in handlers) {
                if (!handler.isProcessTerminated) handler.destroyProcess()
                handler.waitFor(WAIT_MILLISECONDS)
            }
            runInEdtAndWait {
                val editors = FileEditorManager.getInstance(project)
                editors.openFiles.forEach(editors::closeFile)
                val tracedRuns = TracedRuns.getInstance(project)
                tracedRuns.runs.forEach(tracedRuns::close)
                RunContentManager.getInstance(project).allDescriptors.forEach(Disposer::dispose)
                val watched = WatchedMethods.getInstance(project)
                watched.replaceEntries(emptyList())
                watched.identifier = { null }
            }
            NewspeakSettings.getInstance().state.runtimePath = null
            CoreLibraryStore.deleteRecursively(directory)
        } finally {
            super.tearDown()
        }
    }

    fun testATracedRunDecoratesItsMethod() {
        val program = program("Demo", DEMO)
        val watched = WatchedMethods.getInstance(project)
        // The module's identity is the language server's answer, and a light test starts no server.
        watched.identifier = { file -> if (file == program) ModuleIdentity("Demo", null) else null }
        watched.watch("Demo>>run")
        // As the project's opening would; the light test project runs no startup activity of its own.
        runInEdtAndWait { DecorationsService.getInstance(project) }

        val configuration = NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration
        configuration.name = "Demo"
        configuration.program = program.path
        configuration.options.traceCategories = mutableListOf("compile", "code-zone")
        configuration.options.printCode = true
        val output = StringBuffer()
        val standardOutput = StringBuffer()
        val started = recordNextProcess { event, outputType ->
            output.append(event.text)
            if (outputType == ProcessOutputTypes.STDOUT) standardOutput.append(event.text)
        }
        runInEdtAndWait {
            val runManager = RunManager.getInstance(project)
            val settings = runManager.createConfiguration(configuration, NewspeakConfigurationType.getInstance().factory)
            ProgramRunnerUtil.executeConfiguration(settings, DefaultRunExecutor.getRunExecutorInstance())
        }
        val handler = started.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(output.toString(), handler.waitFor(WAIT_MILLISECONDS))
        assertEquals(output.toString(), 0, handler.exitCode)
        assertTrue(output.toString(), standardOutput.contains("4200"))
        // The trace goes into the file, never into the console.
        assertFalse(output.toString(), output.contains("[compile]"))

        val run = waitFor("the run's method and its code") {
            runInEdtAndGet { TracedRuns.getInstance(project).selected?.takeIf { run -> run.method("Demo>>run")?.let(run.code::containsKey) == true } }
        }
        val method = checkNotNull(runInEdtAndGet { run.method("Demo>>run") })
        val state = runInEdtAndGet { run.state(method) }
        assertTrue("$state", state == MethodState.BASELINE || state == MethodState.OPTIMIZED)
        assertEquals(DefaultRunExecutor.EXECUTOR_ID, runInEdtAndGet { TracedRuns.getInstance(project).executorOf(run) })
        // The listing is read back from the file the runtime wrote, at the place the reader counted, off the EDT.
        val entry = runInEdtAndGet { run.code.getValue(method).first() }
        val code = run.readCode(entry)
        assertEquals(entry.body, code.body)
        assertEquals(entry.tier, code.tier)
        assertEquals(entry.bytes, code.bytes)
        assertTrue(code.instructions.isNotEmpty())

        val editor = runInEdtAndGet { checkNotNull(FileEditorManager.getInstance(project).openTextEditor(OpenFileDescriptor(project, program), false)) }
        val runLine = DEMO.lines().indexOfFirst { line -> line.trim().startsWith("run = (") }
        val icon = waitFor("an icon at run's line") { runInEdtAndGet { icons(editor)[runLine] } }
        assertEquals(state, icon.state)
        assertFalse(icon.stale)
        assertTrue(icon.watched)
    }

    /**
     * The Inlining tab shows the plans of a run started as the user starts
     * one -- the categories `compile`, `optimize`, `deoptimize` and
     * `code-zone`, machine code and inlining plans listed, no method
     * watched --: a row for each plan the runtime wrote, the plugin's
     * command line, reader and tab between them.
     */
    fun testTheInliningTabShowsTheRunsPlans() {
        val program = program("Hot", HOT)
        val configuration = NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration
        configuration.name = "Hot"
        configuration.program = program.path
        configuration.options.traceCategories = mutableListOf("compile", "optimize", "deoptimize", "code-zone")
        configuration.options.printCode = true
        configuration.options.printInlining = true
        val output = StringBuffer()
        val started = recordNextProcess { event, _ -> output.append(event.text) }
        runInEdtAndWait {
            val runManager = RunManager.getInstance(project)
            val settings = runManager.createConfiguration(configuration, NewspeakConfigurationType.getInstance().factory)
            ProgramRunnerUtil.executeConfiguration(settings, DefaultRunExecutor.getRunExecutorInstance())
        }
        val handler = started.get(WAIT_SECONDS, TimeUnit.SECONDS)
        // Made as the run's tab makes it, before the records arrive.
        val tab = waitFor("the traced run") {
            runInEdtAndGet { TracedRuns.getInstance(project).selected?.let { run -> InliningTab(project, run, testRootDisposable) } }
        }
        assertTrue(output.toString(), handler.waitFor(WAIT_MILLISECONDS))
        assertEquals(output.toString(), 0, handler.exitCode)
        val run = runInEdtAndGet { checkNotNull(TracedRuns.getInstance(project).selected) }
        val plans = waitFor("the run's plans") { runInEdtAndGet { run.plans.size.takeIf { count -> count > 0 } } }
        assertEquals("a row for each plan", plans, runInEdtAndGet { tab.tree.rowCount })
    }

    /**
     * A slot's initializer links its source and its code in the Code tab
     * both ways, from the records the runtime wrote: the caret on the slot's
     * expression selects its statement and colours its listing's lines, and
     * the viewer's caret on such a line selects the statement again.
     */
    fun testASlotInitializerLinksItsSourceAndItsCode() {
        val program = program("Balls", BALLS)
        val configuration = NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration
        configuration.name = "Balls"
        configuration.program = program.path
        configuration.options.traceCategories = mutableListOf("compile")
        configuration.options.printBytecode = true
        configuration.options.printCode = true
        val output = StringBuffer()
        val started = recordNextProcess { event, _ -> output.append(event.text) }
        runInEdtAndWait { DecorationsService.getInstance(project) }
        runInEdtAndWait {
            val runManager = RunManager.getInstance(project)
            val settings = runManager.createConfiguration(configuration, NewspeakConfigurationType.getInstance().factory)
            ProgramRunnerUtil.executeConfiguration(settings, DefaultRunExecutor.getRunExecutorInstance())
        }
        val handler = started.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(output.toString(), handler.waitFor(WAIT_MILLISECONDS))
        assertEquals(output.toString(), 0, handler.exitCode)
        val run = waitFor("the initializer's code") {
            runInEdtAndGet {
                TracedRuns.getInstance(project).selected?.takeIf { run ->
                    run.bodies.values.any { body -> body.name == "Balls/Ball initializer" && run.code.containsKey(body.number) }
                }
            }
        }
        val compiled = runInEdtAndGet { run.bodies.values.single { body -> body.name == "Balls/Ball initializer" && run.code.containsKey(body.number) } }
        assertTrue("the initializer that runs is described with its statements", compiled.statements.isNotEmpty())
        val editor = runInEdtAndGet { checkNotNull(FileEditorManager.getInstance(project).openTextEditor(OpenFileDescriptor(project, program), true)) }
        val tab = runInEdtAndGet { CodeTab(project, run, testRootDisposable) }
        runInEdtAndWait { editor.caretModel.moveToOffset(BALLS.indexOf("seed + 500") + 1) }
        val selected = runInEdtAndGet { tab.highlighting.selected }
        assertNotNull("the slot's statement is selected", selected?.second)
        val linked = runInEdtAndGet { tab.highlighting.linkedLines(tab.viewer.document) }
        assertTrue("the listing's lines of the statement", linked.isNotEmpty())
        runInEdtAndWait {
            // Nothing selected, as before any caret named a statement.
            tab.highlighting.deselect()
            tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(linked.first(), 0))
        }
        assertEquals("the listing's line selects the slot's statement", selected, runInEdtAndGet { tab.highlighting.selected })
    }

    /** A traced Debug is told to the runs as Debug's, so that it replaces no traced Run's tab. */
    fun testATracedDebugRunsUnderItsExecutor() {
        val program = program("Demo", DEMO)
        val configuration = NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration
        configuration.name = "Demo"
        configuration.program = program.path
        configuration.options.traceCategories = mutableListOf("compile")
        val output = StringBuffer()
        val started = recordNextProcess { event, _ -> output.append(event.text) }
        val session = CompletableFuture<XDebugSession>()
        project.messageBus.connect(testRootDisposable).subscribe(
            XDebuggerManager.TOPIC,
            object : XDebuggerManagerListener {
                override fun processStarted(debugProcess: XDebugProcess) {
                    session.complete(debugProcess.session)
                }
            },
        )
        runInEdtAndWait {
            val runManager = RunManager.getInstance(project)
            val settings = runManager.createConfiguration(configuration, NewspeakConfigurationType.getInstance().factory)
            ProgramRunnerUtil.executeConfiguration(settings, DefaultDebugExecutor.getDebugExecutorInstance())
        }
        val handler = started.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(output.toString(), handler.waitFor(WAIT_MILLISECONDS))
        val run = waitFor("the traced run") { runInEdtAndGet { TracedRuns.getInstance(project).selected } }
        assertEquals(DefaultDebugExecutor.EXECUTOR_ID, runInEdtAndGet { TracedRuns.getInstance(project).executorOf(run) })
        // The session's console, which the light test's end finds unreleased otherwise.
        val debugged = session.get(WAIT_SECONDS, TimeUnit.SECONDS)
        waitFor("the session's end") { debugged.isStopped.takeIf { stopped -> stopped } }
        runInEdtAndWait {
            debugged.consoleView?.let(Disposer::dispose)
            Disposer.dispose(debugged.runContentDescriptor)
        }
    }

    /** The number of the body named [name], once described; asked on the EDT, where the run is changed. */
    private fun TraceRun.method(name: String): Int? = bodies.values.firstOrNull { body -> body.name == name }?.number

    /** Each state icon of [editor] by its line. */
    private fun icons(editor: Editor): Map<Int, StateGutterRenderer> = editor.markupModel.allHighlighters
        .mapNotNull { highlighter ->
            (highlighter.gutterIconRenderer as? StateGutterRenderer)?.let { renderer -> editor.document.getLineNumber(highlighter.startOffset) to renderer }
        }
        .toMap()

    /**
     * The handler of the next process the platform starts in the project,
     * with [onText] told its output: the platform tells its listeners
     * before the process is started, so the listener sees every line.
     */
    private fun recordNextProcess(onText: (ProcessEvent, Key<*>) -> Unit): CompletableFuture<ProcessHandler> {
        val started = CompletableFuture<ProcessHandler>()
        project.messageBus.connect(testRootDisposable).subscribe(
            ExecutionManager.EXECUTION_TOPIC,
            object : ExecutionListener {
                override fun processStarting(executorId: String, environment: ExecutionEnvironment, handler: ProcessHandler) {
                    if (started.isDone) return
                    handlers.add(handler)
                    handler.addProcessListener(
                        object : ProcessListener {
                            override fun onTextAvailable(event: ProcessEvent, outputType: Key<*>) = onText(event, outputType)
                        },
                    )
                    started.complete(handler)
                }
            },
        )
        return started
    }

    /** [condition]'s first answer that is not null, asked until [WAIT_MILLISECONDS] pass. */
    private fun <T : Any> waitFor(what: String, condition: () -> T?): T {
        val deadline = System.currentTimeMillis() + WAIT_MILLISECONDS
        while (true) {
            condition()?.let { answer -> return answer }
            assertTrue("no $what within $WAIT_SECONDS s", System.currentTimeMillis() < deadline)
            Thread.sleep(POLL_MILLISECONDS)
        }
    }

    private fun program(name: String, source: String): VirtualFile {
        val path = directory.resolve("$name.ns")
        Files.writeString(path, source)
        return checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(path))
    }

    private companion object {
        const val WAIT_SECONDS = 30L
        const val WAIT_MILLISECONDS = 30_000L
        const val POLL_MILLISECONDS = 20L

        /** A class whose slots run sends as they are initialized, instantiated often enough that its initializer is compiled. */
        val BALLS = """
            class Balls = Object new () (
              class Ball new: seed = (
                |
                  horizontal ::= seed + 500.
                  vertical ::= seed max: 300.
                |
              ) (
                public sum = (
                  ^horizontal + vertical
                )
              )

              public main: platform args: arguments = (
                | total ::= 0. |
                1 to: 1000 do: [:index | total:: total + (Ball new: index) sum].
                platform transcript show: total printString; newLine.
              )
            )

        """.trimIndent()

        /** `run` is sent three hundred times, each sending `square:` a thousand times, so that the optimizing JIT compiles it. */
        val HOT = """
            class Hot = Object new () (
              square: x = (
                ^x * x
              )

              run = (
                | total ::= 0. |
                1 to: 1000 do: [:index | total:: total + (self square: index)].
                ^total
              )

              public main: platform args: arguments = (
                | total ::= 0. |
                1 to: 300 do: [:index | total:: total + run].
                platform transcript show: total printString; newLine.
              )
            )

        """.trimIndent()

        /** `run` is sent a hundred times, so that the baseline JIT compiles it at its second entry. */
        val DEMO = """
            class Demo = Object new () (
              run = (
                ^21 * 2.
              )

              public main: platform args: arguments = (
                |
                  total ::= 0.
                |
                1 to: 100 do: [:index | total:: total + run].
                platform transcript show: total printString; newLine.
              )
            )

        """.trimIndent()
    }
}
