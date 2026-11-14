package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.execution.KillableProcess
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.executors.DefaultDebugExecutor
import com.intellij.execution.process.ProcessEvent
import com.intellij.execution.process.ProcessListener
import com.intellij.execution.runners.ExecutionEnvironmentBuilder
import com.intellij.notification.Notification
import com.intellij.notification.Notifications
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.Key
import com.intellij.openapi.util.io.NioFiles
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.testFramework.runInEdtAndWait
import com.intellij.xdebugger.XDebugProcess
import com.intellij.xdebugger.XDebugProcessStarter
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebugSessionListener
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.XDebuggerManagerListener
import com.intellij.xdebugger.XDebuggerUtil
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointListener
import com.intellij.xdebugger.frame.XCompositeNode
import com.intellij.xdebugger.frame.XDebuggerTreeNodeHyperlink
import com.intellij.xdebugger.frame.XFullValueEvaluator
import com.intellij.xdebugger.frame.XValue
import com.intellij.xdebugger.frame.XValueChildrenList
import com.intellij.xdebugger.frame.XValueContainer
import com.intellij.xdebugger.frame.XValueNode
import com.intellij.xdebugger.frame.XValuePlace
import com.intellij.xdebugger.frame.presentation.XValuePresentation
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.run.NewspeakConfigurationType
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import javax.swing.Icon

/**
 * The debug process in the platform's debugger, against the real runtime
 * the build names in `nsNext.runtime`: Debug on a Newspeak run
 * configuration, through the plugin's runner, as the user starts it.
 *
 * The test runs off the event dispatch thread, so that it may wait for
 * the session while the platform updates it there. Every wait is bounded,
 * and every listener is registered as the debug process is created --
 * before it connects -- so that no stop can come before it.
 */
class NewspeakDebugProcessTest : BasePlatformTestCase() {
    // Read once a test runs: JUnit 3 makes an instance of every test even where a filter leaves the class out.
    private val runtime: Path by lazy { Path.of(System.getProperty("nsNext.runtime")) }
    private lateinit var directory: Path
    private val debugged = CopyOnWriteArrayList<Debugging>()

    override fun runInDispatchThread(): Boolean = false

    override fun setUp() {
        super.setUp()
        directory = Files.createTempDirectory("ns-next-debug-process")
        NewspeakSettings.getInstance().state.runtimePath = runtime.toString()
    }

    override fun tearDown() {
        try {
            for (debugging in debugged) {
                val handler = debugging.process.processHandler
                if (!handler.isProcessTerminated) (handler as KillableProcess).killProcess()
                handler.waitFor(WAIT_MILLISECONDS)
                debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
            }
            runInEdtAndWait {
                // The session's console and the editors its stops opened.
                for (debugging in debugged) {
                    debugging.session.consoleView?.let(Disposer::dispose)
                    Disposer.dispose(debugging.session.runContentDescriptor)
                }
                val editors = FileEditorManager.getInstance(project)
                editors.openFiles.forEach(editors::closeFile)
                val breakpoints = XDebuggerManager.getInstance(project).breakpointManager
                WriteAction.run<Throwable> { breakpoints.allBreakpoints.forEach(breakpoints::removeBreakpoint) }
            }
            NewspeakSettings.getInstance().state.runtimePath = null
            NioFiles.deleteRecursively(directory)
        } finally {
            super.tearDown()
        }
    }

    fun testDebuggingStopsAtALineBreakpointAndShowsTheFrame() {
        val program = program("Stops", STOPS)
        toggleBreakpoint(program, line = 6)
        val debugging = debug(program)

        val position = debugging.nextPause()
        assertEquals(program, position.file)
        assertEquals("line 6, counted from 0 by the platform", 5, position.line)

        val frame = checkNotNull(debugging.session.currentStackFrame) { "the stop shows a frame" }
        val locals = childrenOf(frame)
        assertTrue(locals.keys.toString(), locals.keys.containsAll(listOf("count", "elements")))
        assertEquals("3", presentationOf(locals.getValue("count")).value)
        val elements = presentationOf(locals.getValue("elements"))
        assertTrue("an Array has children", elements.hasChildren)
        val elementValues = childrenOf(locals.getValue("elements")).map { (name, value) -> name to presentationOf(value).value }
        assertEquals(listOf("[1]" to "10", "[2]" to "20", "[3]" to "30"), elementValues)

        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertEquals(0, debugging.process.processHandler.exitCode)
        assertTrue(debugging.output(), debugging.output().contains("3"))
    }

    fun testStepOverMovesToTheNextLine() {
        val program = program("Stops", STOPS)
        toggleBreakpoint(program, line = 4)
        val debugging = debug(program)
        assertEquals(3, debugging.nextPause().line)

        onEdt { debugging.session.stepOver(false) }
        val stepped = debugging.nextPause()
        assertEquals(program, stepped.file)
        assertEquals("line 5, counted from 0 by the platform", 4, stepped.line)

        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    fun testStopTerminatesAndASecondStopKills() {
        val program = program("LongLeaf", LONG_LEAF)
        val debugging = debug(program)
        debugging.printed("computing").get(WAIT_SECONDS, TimeUnit.SECONDS)
        val handler = debugging.process.processHandler

        // The first Stop: what the platform's Stop does to a running process.
        handler.destroyProcess()
        (debugging.process as NewspeakDebugProcess).terminateAnswered.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertFalse("a program inside one leaf outlives the terminate", handler.waitFor(LEAF_OUTLIVES_MILLISECONDS))
        assertTrue(handler.isProcessTerminating)

        // The second: the platform's Stop on a process that is terminating kills it.
        (handler as KillableProcess).killProcess()
        assertTrue(handler.waitFor(WAIT_MILLISECONDS))
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertFalse(debugging.output(), debugging.output().contains("done"))
    }

    fun testAWrongProcessIdEndsTheSession() {
        val program = program("LongLeaf", LONG_LEAF)
        // A process of ns-next's own, which the fake agent below does not speak for.
        val handler = NewspeakDebuggeeProcessHandler(GeneralCommandLine(runtime.toString(), "run", program.path))
        val serverSocket = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        val reported = CompletableFuture<String>()
        project.messageBus.connect(testRootDisposable).subscribe(
            Notifications.TOPIC,
            object : Notifications {
                override fun notify(notification: Notification) {
                    reported.complete(notification.content)
                }
            },
        )
        val recorder = recordNextProcess()
        onEdt {
            XDebuggerManager.getInstance(project)
                .newSessionBuilder(
                    object : XDebugProcessStarter() {
                        override fun start(session: XDebugSession): XDebugProcess =
                            NewspeakDebugProcess(session, serverSocket, handler, null)
                    },
                )
                .sessionName("wrong process")
                .showTab(true)
                .startSession()
        }
        val debugging = recorder.get(WAIT_SECONDS, TimeUnit.SECONDS)

        val otherProcess = ProcessHandle.current().pid()
        val received = CopyOnWriteArrayList<String>()
        val socket = Socket(InetAddress.getLoopbackAddress(), serverSocket.localPort)
        val agent = DapConnection(socket.getInputStream(), socket.getOutputStream(), socket)
        agent.start(
            { request ->
                val command = request.get("command").asString
                received.add(command)
                agent.send(response(request))
                if (command == "attach") {
                    agent.send(event("process", JsonObject().apply { addProperty("systemProcessId", otherProcess) }))
                    agent.send(event("initialized", JsonObject()))
                }
            },
            { _ -> },
        )
        try {
            assertEquals(
                NewspeakBundle.message("debugger.error.wrong.process", otherProcess.toString(), handler.process.pid().toString()),
                reported.get(WAIT_SECONDS, TimeUnit.SECONDS),
            )
            assertTrue("the process started is ended", handler.waitFor(WAIT_MILLISECONDS))
            debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
            assertEquals(listOf("initialize", "attach"), received.toList())
        } finally {
            agent.close()
        }
    }

    // ---- Starting a session ----

    /** One session the test started, and what the test waits for in it. */
    inner class Debugging(val process: XDebugProcess) {
        val session: XDebugSession get() = process.session
        val pauses = LinkedBlockingQueue<XSourcePosition>()
        val stopped = CompletableFuture<Unit>()
        private val text = StringBuffer()
        private val awaitedTexts = CopyOnWriteArrayList<Pair<String, CompletableFuture<Unit>>>()

        init {
            process.session.addSessionListener(
                object : XDebugSessionListener {
                    override fun sessionPaused() {
                        process.session.currentPosition?.let(pauses::add)
                    }

                    override fun sessionStopped() {
                        stopped.complete(Unit)
                    }
                },
            )
            process.processHandler.addProcessListener(
                object : ProcessListener {
                    override fun onTextAvailable(event: ProcessEvent, outputType: Key<*>) {
                        text.append(event.text)
                        for ((awaited, future) in awaitedTexts) if (text.contains(awaited)) future.complete(Unit)
                    }
                },
            )
        }

        fun nextPause(): XSourcePosition =
            pauses.poll(WAIT_SECONDS, TimeUnit.SECONDS) ?: throw AssertionError("no stop within $WAIT_SECONDS s: ${output()}")

        fun output(): String = text.toString()

        /** Completes once the process printed `awaited`. */
        fun printed(awaited: String): CompletableFuture<Unit> {
            val future = CompletableFuture<Unit>()
            awaitedTexts.add(awaited to future)
            if (text.contains(awaited)) future.complete(Unit)
            return future
        }
    }

    /**
     * The next debug process the platform starts in the project, as it is
     * started: the platform tells its listeners before it initializes the
     * session, so the test's listeners see every stop.
     */
    private fun recordNextProcess(): CompletableFuture<Debugging> {
        val started = CompletableFuture<Debugging>()
        project.messageBus.connect(testRootDisposable).subscribe(
            XDebuggerManager.TOPIC,
            object : XDebuggerManagerListener {
                override fun processStarted(debugProcess: XDebugProcess) {
                    if (started.isDone) return
                    val debugging = Debugging(debugProcess)
                    debugged.add(debugging)
                    started.complete(debugging)
                }
            },
        )
        return started
    }

    /** Debug on a Newspeak run configuration of `program`, as the user starts it. */
    private fun debug(program: VirtualFile): Debugging {
        val configuration = NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration
        configuration.program = program.path
        val environment = ExecutionEnvironmentBuilder.create(DefaultDebugExecutor.getDebugExecutorInstance(), configuration).build()
        assertTrue(environment.runner is NewspeakDebugRunner)
        val recorder = recordNextProcess()
        onEdt { environment.runner.execute(environment) }
        return recorder.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    private fun program(name: String, source: String): VirtualFile {
        val path = directory.resolve("$name.ns")
        Files.writeString(path, source)
        return checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(path))
    }

    /**
     * A line breakpoint on `line`, counted from 1, once it exists: the
     * platform adds a toggled breakpoint later, and a session started
     * before it would let the program run past the line.
     */
    private fun toggleBreakpoint(file: VirtualFile, line: Int) {
        val added = CompletableFuture<Unit>()
        val connection = project.messageBus.connect(testRootDisposable)
        connection.subscribe(
            XBreakpointListener.TOPIC,
            object : XBreakpointListener<XBreakpoint<*>> {
                override fun breakpointAdded(breakpoint: XBreakpoint<*>) {
                    added.complete(Unit)
                }
            },
        )
        onEdt { XDebuggerUtil.getInstance().toggleLineBreakpoint(project, file, line - 1) }
        added.get(WAIT_SECONDS, TimeUnit.SECONDS)
        connection.disconnect()
    }

    private fun onEdt(action: () -> Unit) = runInEdtAndWait { action() }

    // ---- Reading frames and values ----

    /** The children `container` computes, by name, in order. */
    private fun childrenOf(container: XValueContainer): Map<String, XValue> {
        val node = CollectingNode()
        container.computeChildren(node)
        return node.collected.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    private class CollectingNode : XCompositeNode {
        val collected = CompletableFuture<Map<String, XValue>>()
        private val children = LinkedHashMap<String, XValue>()

        override fun addChildren(children: XValueChildrenList, last: Boolean) {
            for (value in children.topValues) this.children[value.name] = value
            for (index in 0 until children.size()) this.children[children.getName(index)] = children.getValue(index)
            if (last) collected.complete(this.children)
        }

        @Deprecated("the platform's own deprecation; the tree calls it still")
        override fun tooManyChildren(remaining: Int) {
            collected.complete(children)
        }

        override fun tooManyChildren(remaining: Int, addNextChildren: Runnable) {
            collected.complete(children)
        }

        override fun setAlreadySorted(alreadySorted: Boolean) {}

        override fun setErrorMessage(errorMessage: String) {
            collected.completeExceptionally(AssertionError(errorMessage))
        }

        override fun setErrorMessage(errorMessage: String, link: XDebuggerTreeNodeHyperlink?) {
            collected.completeExceptionally(AssertionError(errorMessage))
        }

        override fun setMessage(message: String, icon: Icon?, attributes: com.intellij.ui.SimpleTextAttributes, link: XDebuggerTreeNodeHyperlink?) {}
    }

    private data class Presentation(val type: String?, val value: String, val hasChildren: Boolean)

    private fun presentationOf(value: XValue): Presentation {
        val presented = CompletableFuture<Presentation>()
        value.computePresentation(
            object : XValueNode {
                override fun setPresentation(icon: Icon?, type: String?, value: String, hasChildren: Boolean) {
                    presented.complete(Presentation(type, value, hasChildren))
                }

                override fun setPresentation(icon: Icon?, presentation: XValuePresentation, hasChildren: Boolean) {
                    val text = StringBuilder()
                    presentation.renderValue(
                        object : XValuePresentation.XValueTextRenderer {
                            override fun renderValue(value: String) { text.append(value) }
                            override fun renderValue(value: String, key: com.intellij.openapi.editor.colors.TextAttributesKey) { text.append(value) }
                            override fun renderStringValue(value: String) { text.append(value) }
                            override fun renderStringValue(value: String, additionalSpecialCharsToHighlight: String?, maxLength: Int) { text.append(value) }
                            override fun renderNumericValue(value: String) { text.append(value) }
                            override fun renderKeywordValue(value: String) { text.append(value) }
                            override fun renderComment(comment: String) {}
                            override fun renderSpecialSymbol(symbol: String) { text.append(symbol) }
                            override fun renderError(error: String) { text.append(error) }
                        },
                    )
                    presented.complete(Presentation(presentation.type, text.toString(), hasChildren))
                }

                override fun setFullValueEvaluator(fullValueEvaluator: XFullValueEvaluator) {}

                override fun isObsolete(): Boolean = false
            },
            XValuePlace.TREE,
        )
        return presented.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    private fun response(request: JsonObject) = JsonObject().apply {
        addProperty("seq", 0)
        addProperty("type", "response")
        addProperty("request_seq", request.get("seq").asInt)
        addProperty("command", request.get("command").asString)
        addProperty("success", true)
    }

    private fun event(name: String, body: JsonObject) = JsonObject().apply {
        addProperty("seq", 0)
        addProperty("type", "event")
        addProperty("event", name)
        add("body", body)
    }

    private companion object {
        const val WAIT_SECONDS = 30L
        const val WAIT_MILLISECONDS = 30_000L

        /** How long a program inside one leaf must outlive the terminate: it would end at once otherwise. */
        const val LEAF_OUTLIVES_MILLISECONDS = 1_000L

        /** A root with two locals; line 6 prints the count. */
        val STOPS = """
            class Stops = Object new () (
              public main: platform args: arguments = (
                | count elements |
                count:: 3.
                elements:: {10. 20. 30}.
                platform transcript show: count printString; newLine.
              )
            ) : ()
        """.trimIndent() + "\n"

        /**
         * A root that says it computes, then spends long in one leaf: the
         * decimal digits of a number of two million digits, half a minute
         * in a debug build, written in one leaf. The number is made before
         * the line is printed, since making it reaches safepoints.
         */
        val LONG_LEAF = """
            class LongLeaf = Object new () (
              public main: platform args: arguments = (
                | big = 10 raisedTo: 2000000. |
                platform transcript show: 'computing'; newLine.
                big printString.
                platform transcript show: 'done'; newLine.
              )
            ) : ()
        """.trimIndent() + "\n"
    }
}
