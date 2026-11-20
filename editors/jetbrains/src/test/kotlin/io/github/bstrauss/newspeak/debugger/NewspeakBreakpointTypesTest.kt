package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonArray
import com.google.gson.JsonObject
import com.google.gson.JsonParser
import com.intellij.execution.KillableProcess
import com.intellij.execution.executors.DefaultDebugExecutor
import com.intellij.execution.process.ProcessEvent
import com.intellij.execution.process.ProcessListener
import com.intellij.execution.runners.ExecutionEnvironmentBuilder
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.command.CommandProcessor
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.Key
import com.intellij.openapi.util.io.NioFiles
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.vfs.newvfs.impl.VfsRootAccess
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.testFramework.runInEdtAndGet
import com.intellij.testFramework.runInEdtAndWait
import com.intellij.xdebugger.XDebugProcess
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebugSessionListener
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.XDebuggerManagerListener
import com.intellij.xdebugger.XDebuggerUtil
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.breakpoints.SuspendPolicy
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XLineBreakpoint
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.library.CoreLibraryStore
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import io.github.bstrauss.newspeak.run.NewspeakConfigurationType
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.lang.reflect.Proxy
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicInteger

/**
 * The three breakpoint types: where a line breakpoint may stand, what each
 * kind sends the agent -- through the handlers, over a scripted peer in
 * place of `ns-next`'s agent -- and, against the real runtime the build
 * names in `nsNext.runtime`, a temporary breakpoint's one stop.
 *
 * The test runs off the event dispatch thread, so that the real session
 * may be waited for while the platform updates it there.
 */
class NewspeakBreakpointTypesTest : BasePlatformTestCase() {
    private lateinit var directory: Path
    private lateinit var serverFetcher: (VirtualFile) -> BreakableLines?
    private val opened = mutableListOf<AutoCloseable>()
    private val debugged = CopyOnWriteArrayList<Debugging>()

    private val cache: BreakableLinesCache get() = BreakableLinesCache.getInstance(project)
    private val breakpoints get() = XDebuggerManager.getInstance(project).breakpointManager

    override fun runInDispatchThread(): Boolean = false

    override fun setUp() {
        super.setUp()
        directory = Files.createTempDirectory("ns-next-breakpoint-types")
        serverFetcher = cache.fetcher
    }

    // The light project, its services and its breakpoints outlive a test.
    override fun tearDown() {
        try {
            for (debugging in debugged) {
                val handler = debugging.process.processHandler
                if (!handler.isProcessTerminated) (handler as KillableProcess).killProcess()
                handler.waitFor(WAIT_MILLISECONDS)
                debugging.awaitEnd("the session's end after the test")
            }
            for (resource in opened.asReversed()) resource.close()
            cache.fetcher = serverFetcher
            runInEdtAndWait {
                for (debugging in debugged) {
                    debugging.session.consoleView?.let(Disposer::dispose)
                    Disposer.dispose(debugging.session.runContentDescriptor)
                }
                val editors = FileEditorManager.getInstance(project)
                editors.openFiles.forEach(editors::closeFile)
                WriteAction.run<Throwable> {
                    breakpoints.allBreakpoints.filterNot(breakpoints::isDefaultBreakpoint).forEach(breakpoints::removeBreakpoint)
                }
            }
            // The default "Any exception" breakpoint is the project's for every test.
            for (any in breakpoints.getDefaultBreakpoints(exceptionType())) any.properties.loadState(NewspeakExceptionBreakpointProperties())
            NewspeakSettings.getInstance().state.runtimePath = null
            NioFiles.deleteRecursively(directory)
        } finally {
            super.tearDown()
        }
    }

    // ---- Where a line breakpoint may stand ----

    fun testCanPutAtFollowsTheServersBreakableLines() {
        val file = program("Lines", LINES)
        val type = lineType()
        cache.fetcher = { BreakableLines.of(listOf(BREAKABLE_LINE_3, BREAKABLE_LINE_4)) }
        assertNotNull(cache.fetch(file))

        assertTrue("a statement's line", runInEdtAndGet { type.canPutAt(file, 3, project) })
        assertFalse("a comment's line", runInEdtAndGet { type.canPutAt(file, 2, project) })

        // A document that does not parse: every line, the comment's too.
        cache.fetcher = { BreakableLines.EVERY_LINE }
        assertNotNull(cache.fetch(file))
        assertTrue(runInEdtAndGet { type.canPutAt(file, 2, project) })
    }

    fun testCanPutAtAllowsEveryLineUntilTheServerAnswers() {
        val file = program("Lines", LINES)
        val asked = CompletableFuture<Unit>()
        val release = CompletableFuture<Unit>()
        cache.fetcher = {
            asked.complete(Unit)
            release.get(WAIT_SECONDS, TimeUnit.SECONDS)
            BreakableLines.of(listOf(BREAKABLE_LINE_3))
        }
        assertTrue("no answer yet", runInEdtAndGet { lineType().canPutAt(file, 2, project) })
        asked.get(WAIT_SECONDS, TimeUnit.SECONDS)
        release.complete(Unit)
        waitUntil("the answer is kept") { cache.current(file) != null }
        assertFalse(runInEdtAndGet { lineType().canPutAt(file, 2, project) })
    }

    fun testVariantsListTheBlocksOfALine() {
        val file = program("Lines", LINES)
        cache.fetcher = { BreakableLines.of(listOf(BREAKABLE_LINE_3, BREAKABLE_LINE_4)) }
        assertNotNull(cache.fetch(file))
        val type = lineType()

        val variants = runInEdtAndGet { type.computeVariants(project, position(file, 3)) }
        assertEquals(listOf(null, BLOCK_COLUMN), variants.map { variant -> variant.createProperties()?.column })
        val block = variants[1]
        val text = String(file.contentsToByteArray(), Charsets.UTF_8)
        val range = checkNotNull(block.highlightRange)
        assertEquals("[:each | each printString]", text.substring(range.startOffset, range.endOffset))
        assertTrue(block.text, block.text.contains("[:each"))

        assertEquals("a line with no block has none", emptyList<Any>(), runInEdtAndGet { type.computeVariants(project, position(file, 4)) })
    }

    // ---- What each kind sends ----

    fun testAPassCountIsSentAsTheHitCondition() {
        val file = program("Lines", LINES)
        val counted = addLineBreakpoint(file, 4, NewspeakLineBreakpointProperties().apply { passCount = "3" })
        val block = addLineBreakpoint(file, 3, NewspeakLineBreakpointProperties().apply { column = BLOCK_COLUMN })
        val agent = RecordingAgent { _ ->
            JsonParser.parseString(
                """{"breakpoints":[{"id":1,"verified":true,"line":5},{"id":2,"verified":true,"line":4,"column":${BLOCK_COLUMN + 1}}]}""",
            ).asJsonObject
        }
        val session = RecordingSession()
        val handler = NewspeakLineBreakpointHandler(session.proxy)
        handler.registerBreakpoint(counted)
        handler.registerBreakpoint(block)
        handler.sendAll(agent.client).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }

        val sent = agent.requests("setBreakpoints").last()
        assertEquals(file.toNioPath().toString(), sent.getAsJsonObject("source").get("path").asString)
        assertEquals(
            JsonParser.parseString("""[{"line":5,"hitCondition":"3"},{"line":4,"column":${BLOCK_COLUMN + 1}}]"""),
            sent.getAsJsonArray("breakpoints"),
        )
        assertEquals(listOf("setBreakpointVerified 5", "setBreakpointVerified 4"), session.calls)
        assertSame(counted, handler.breakpointOf(1))
    }

    /**
     * A breakpoint in a file of a directory of shipped sources is sent by
     * the file's path, never by the module's name: the agent takes the path
     * for the shipped module only in the library the session handed it, so
     * that a program module of the same name never takes it, and one of a
     * library the project does not show -- an older binary's -- for none.
     */
    fun testABreakpointInALibraryFileIsSentByItsPath() {
        val library = NewspeakCoreLibrary.STORE.root.resolve("00000000000000ef")
        try {
            Files.createDirectories(library)
            VfsRootAccess.allowRootAccess(testRootDisposable, library.toString())
            Files.writeString(library.resolve("Kernel.ns"), LINES)
            val kernel = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(library.resolve("Kernel.ns")))
            val breakpoint = addLineBreakpoint(kernel, 3, NewspeakLineBreakpointProperties())
            val agent = RecordingAgent { _ -> JsonParser.parseString("""{"breakpoints":[{"id":1,"verified":true}]}""").asJsonObject }
            val handler = NewspeakLineBreakpointHandler(RecordingSession().proxy)
            handler.registerBreakpoint(breakpoint)
            handler.sendAll(agent.client).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }
            val source = agent.requests("setBreakpoints").single().getAsJsonObject("source")
            assertEquals(kernel.toNioPath().toString(), source.get("path").asString)
            assertNull("no name", source.get("name"))
            runInEdtAndWait { WriteAction.run<Throwable> { breakpoints.removeBreakpoint(breakpoint) } }
        } finally {
            CoreLibraryStore.deleteRecursively(library)
        }
    }

    fun testAMovedBreakpointShowsTheLineItStopsAtAndARefusedOneItsMessage() {
        val file = program("Lines", LINES)
        val moved = addLineBreakpoint(file, 2, NewspeakLineBreakpointProperties())
        val refused = addLineBreakpoint(file, 3, NewspeakLineBreakpointProperties().apply { passCount = "=0" })
        val agent = RecordingAgent { _ ->
            JsonParser.parseString(
                """{"breakpoints":[{"id":1,"verified":true,"line":4},{"verified":false,"message":"`=0` is no hit condition"}]}""",
            ).asJsonObject
        }
        val session = RecordingSession()
        val handler = NewspeakLineBreakpointHandler(session.proxy)
        handler.registerBreakpoint(moved)
        handler.registerBreakpoint(refused)
        handler.sendAll(agent.client).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }

        assertEquals(
            listOf(
                "updateBreakpointPresentation 3 " + NewspeakBundle.message("debugger.breakpoint.moved", "4"),
                "setBreakpointInvalid 4 `=0` is no hit condition",
            ),
            session.calls,
        )
    }

    fun testAMethodBreakpointIsSentByItsName() {
        val method = runInEdtAndGet {
            WriteAction.compute<XBreakpoint<NewspeakMethodBreakpointProperties>, Throwable> {
                breakpoints.addBreakpoint(
                    methodType(),
                    NewspeakMethodBreakpointProperties().apply {
                        methodName = "Program/Inner class>>make:"
                        passCount = ">=2"
                    },
                )
            }
        }
        assertEquals("Program/Inner class>>make:", methodType().getDisplayText(method))
        val agent = RecordingAgent { _ -> JsonParser.parseString("""{"breakpoints":[{"id":7,"verified":true}]}""").asJsonObject }
        val handler = NewspeakMethodBreakpointHandler(RecordingSession().proxy)
        handler.registerBreakpoint(method)
        handler.sendAll(agent.client).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }

        assertEquals(
            JsonParser.parseString("""{"breakpoints":[{"name":"Program/Inner class>>make:","hitCondition":">=2"}]}"""),
            agent.requests("setFunctionBreakpoints").last(),
        )
        assertSame(method, handler.breakpointOf(7))
    }

    /**
     * A breakpoint kept from before modules had ids, in the dotted form, is
     * sent as it was written and shown with the agent's refusal, which
     * names the form to write: never rewritten behind the user's back.
     */
    fun testADottedMethodBreakpointIsSentUnchangedAndShownWithTheAgentsRefusal() {
        val dotted = addMethodBreakpoint("Demo.Account>>deposit:")
        val refusal = "no module `Demo.Account`; a class inside a module is written `Demo/Account`"
        val agent = RecordingAgent { _ -> JsonParser.parseString("""{"breakpoints":[{"verified":false,"message":"$refusal"}]}""").asJsonObject }
        val session = RecordingSession()
        val handler = NewspeakMethodBreakpointHandler(session.proxy)
        handler.registerBreakpoint(dotted)
        handler.sendAll(agent.client).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }

        assertEquals(
            JsonParser.parseString("""{"breakpoints":[{"name":"Demo.Account>>deposit:"}]}"""),
            agent.requests("setFunctionBreakpoints").last(),
        )
        assertEquals("Demo.Account>>deposit:", dotted.properties.methodName)
        assertEquals(
            listOf("reportMessage " + NewspeakBundle.message("debugger.breakpoint.method.unverified", "Demo.Account>>deposit:", refusal)),
            session.calls,
        )
    }

    /** A breakpoint's condition is the agent's `condition`, a line breakpoint's and a method breakpoint's alike, which the runtime decides. */
    fun testAConditionIsSentAsTheCondition() {
        val file = program("Lines", LINES)
        val line = addLineBreakpoint(file, 4, NewspeakLineBreakpointProperties())
        val method = addMethodBreakpoint("Lines>>main:args:")
        runInEdtAndWait {
            line.setCondition("arguments size > 1")
            method.setCondition("  ")
        }
        val agent = RecordingAgent { command ->
            JsonParser.parseString(if (command == "setBreakpoints") """{"breakpoints":[{"id":1,"verified":true}]}""" else """{"breakpoints":[{"id":2,"verified":true}]}""").asJsonObject
        }
        val lines = NewspeakLineBreakpointHandler(RecordingSession().proxy)
        val methods = NewspeakMethodBreakpointHandler(RecordingSession().proxy)
        lines.registerBreakpoint(line)
        methods.registerBreakpoint(method)
        (lines.sendAll(agent.client) + methods.sendAll(agent.client)).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }
        assertEquals(
            JsonParser.parseString("""[{"line":5,"condition":"arguments size > 1"}]"""),
            agent.requests("setBreakpoints").last().getAsJsonArray("breakpoints"),
        )
        // A blank condition is none.
        assertEquals(
            JsonParser.parseString("""{"breakpoints":[{"name":"Lines>>main:args:"}]}"""),
            agent.requests("setFunctionBreakpoints").last(),
        )

        runInEdtAndWait { method.setCondition("arguments isEmpty") }
        // The platform reports the change, which unregisters and registers the breakpoint again.
        methods.unregisterBreakpoint(method, false)
        methods.registerBreakpoint(method)
        waitUntil("the method set is sent again") { agent.requests("setFunctionBreakpoints").size == 3 }
        assertEquals(
            JsonParser.parseString("""{"breakpoints":[{"name":"Lines>>main:args:","condition":"arguments isEmpty"}]}"""),
            agent.requests("setFunctionBreakpoints").last(),
        )
    }

    /**
     * A log expression of a line breakpoint that needs no stop is the
     * agent's `logMessage`, the expression in braces -- after a blank
     * where it starts with a tuple's brace, since `{{` escapes a brace --
     * and the agent logs it without stopping; one that suspends, or logs
     * its hit, stops, and the session evaluates it there instead.
     */
    fun testALogExpressionIsSentAsTheLogMessage() {
        val file = program("Lines", LINES)
        val logged = addLineBreakpoint(file, 4, NewspeakLineBreakpointProperties())
        val tuple = addLineBreakpoint(file, 3, NewspeakLineBreakpointProperties())
        val suspending = addLineBreakpoint(file, 5, NewspeakLineBreakpointProperties())
        runInEdtAndWait {
            for (breakpoint in listOf(logged, tuple)) breakpoint.suspendPolicy = SuspendPolicy.NONE
            logged.setLogExpression("arguments size printString")
            tuple.setLogExpression("{1. 2} size")
            suspending.setLogExpression("arguments")
        }
        val agent = RecordingAgent { _ ->
            JsonParser.parseString("""{"breakpoints":[{"id":1,"verified":true},{"id":2,"verified":true},{"id":3,"verified":true}]}""").asJsonObject
        }
        val handler = NewspeakLineBreakpointHandler(RecordingSession().proxy)
        for (breakpoint in listOf(logged, tuple, suspending)) handler.registerBreakpoint(breakpoint)
        handler.sendAll(agent.client).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }
        assertEquals(
            JsonParser.parseString(
                """[{"line":5,"logMessage":"{arguments size printString}"},{"line":4,"logMessage":"{ {1. 2} size}"},{"line":6}]""",
            ),
            agent.requests("setBreakpoints").last().getAsJsonArray("breakpoints"),
        )
        assertFalse("a breakpoint that suspends stops, and is no logpoint", isLogpoint(suspending))
        runInEdtAndWait { logged.isLogMessage = true }
        assertFalse("one that logs its hit stops too", isLogpoint(logged))
    }

    /**
     * Review Focus 4: the runtime decides a condition, once at each hit,
     * and nothing in the IDE evaluates it again. The condition counts its
     * evaluations in a slot the program prints at its end: three hits,
     * each stopped at and resumed, count three.
     */
    fun testAConditionIsEvaluatedByTheRuntimeOnly() {
        NewspeakSettings.getInstance().state.runtimePath = runtime.toString()
        cache.fetcher = { null }
        val program = program("Counted", COUNTED)
        val breakpoint = addLineBreakpoint(program, COUNTED_BODY_LINE, NewspeakLineBreakpointProperties())
        runInEdtAndWait { breakpoint.setCondition("countHit") }
        val debugging = debug(program)

        repeat(3) { hit ->
            assertEquals("the stop at hit ${hit + 1}", COUNTED_BODY_LINE, debugging.nextPause().line)
            runInEdtAndWait { debugging.session.resume() }
        }
        debugging.awaitEnd("the session's end")
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertTrue("no fourth stop: ${debugging.pauses}", debugging.pauses.isEmpty())
        assertTrue(debugging.output(), debugging.output().contains("hits 3"))
    }

    fun testAnExceptionBreakpointSendsItsFiltersAndClasses() {
        val type = exceptionType()
        val any = breakpoints.getDefaultBreakpoints(type).single()
        assertEquals(NewspeakBundle.message("debugger.breakpoint.exception.any"), type.getDisplayText(any))
        any.properties.uncaught = false
        any.properties.caught = false
        any.properties.halt = true
        val failure = runInEdtAndGet {
            WriteAction.compute<XBreakpoint<NewspeakExceptionBreakpointProperties>, Throwable> {
                breakpoints.addBreakpoint(
                    type,
                    NewspeakExceptionBreakpointProperties().apply {
                        className = "Program/Failure"
                        uncaught = false
                        caught = true
                    },
                )
            }
        }
        val agent = RecordingAgent { _ -> JsonObject() }
        val handler = NewspeakExceptionBreakpointHandler(RecordingSession().proxy)
        handler.registerBreakpoint(any)
        handler.registerBreakpoint(failure)
        handler.sendAll(agent.client).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }
        assertEquals(
            JsonParser.parseString("""{"filters":["halt"],"filterOptions":[{"filterId":"caught","condition":"Program/Failure"}]}"""),
            agent.requests("setExceptionBreakpoints").last(),
        )

        // "Any exception" for uncaught ones takes every class: the agent keeps one list of classes for both filters.
        any.properties.uncaught = true
        handler.unregisterBreakpoint(any, false)
        handler.registerBreakpoint(any)
        waitUntil("the set is sent again") { agent.requests("setExceptionBreakpoints").size == 3 }
        assertEquals(
            JsonParser.parseString("""{"filters":["uncaught","caught","halt"],"filterOptions":[]}"""),
            agent.requests("setExceptionBreakpoints").last(),
        )
    }

    fun testTheMethodDialogSplitsAtTheFirstSelectorSeparatorAsTheAgentDoes() {
        val validator = NewspeakMethodBreakpointType.MethodNameValidator
        val accepted = listOf(
            "Module/Class>>>>",
            "com.acme.Module@1.2.0/Class class>>>>",
            "newspeak.core.Kernel/Array>>at:put:",
            "Module>>+",
        )
        for (name in accepted) {
            assertTrue(name, validator.checkInput(name))
        }
        for (refused in listOf("Module/Class", ">>run", "Module/Class>>", "Module/Class>>at: put:")) {
            assertFalse(refused, validator.checkInput(refused))
        }
    }

    fun testTheExceptionDialogRefusesACommaTheAgentWouldReadAsTwoClasses() {
        val validator = NewspeakExceptionBreakpointType.ClassNameValidator
        assertTrue(validator.checkInput("Program/Failure"))
        assertTrue(validator.checkInput("newspeak.core.Kernel/Error"))
        assertTrue(validator.checkInput("com.acme.Json@2.1.0-rc.1/Parser.Failure"))
        assertFalse(validator.checkInput("A/B,C/D"))
        assertFalse(validator.checkInput("Program..Failure"))
        assertFalse(validator.checkInput("Program//Failure"))
        assertFalse(validator.checkInput("Program/"))
        assertFalse(validator.checkInput("Program@/Failure"))
        assertFalse(validator.checkInput("Program/ Failure"))
    }

    fun testABreakpointTheAgentDidNotAnswerIsInvalidAndOneRegisteredTwiceIsSentOnce() {
        val file = program("Lines", LINES)
        val first = addLineBreakpoint(file, 3, NewspeakLineBreakpointProperties())
        val second = addLineBreakpoint(file, 4, NewspeakLineBreakpointProperties())
        val agent = RecordingAgent { _ -> JsonParser.parseString("""{"breakpoints":[{"id":1,"verified":true,"line":4}]}""").asJsonObject }
        val session = RecordingSession()
        val handler = NewspeakLineBreakpointHandler(session.proxy)
        handler.registerBreakpoint(first)
        handler.registerBreakpoint(first)
        handler.registerBreakpoint(second)
        handler.sendAll(agent.client).forEach { answer -> answer.get(WAIT_SECONDS, TimeUnit.SECONDS) }

        assertEquals(2, agent.requests("setBreakpoints").last().getAsJsonArray("breakpoints").size())
        assertEquals(
            listOf("setBreakpointVerified 4", "setBreakpointInvalid 5 " + NewspeakBundle.message("debugger.breakpoint.unanswered")),
            session.calls,
        )
    }

    fun testTheCacheKeepsOnlyTheCurrentAnswerOfAnOpenFile() {
        val file = program("Lines", LINES)
        cache.fetcher = { BreakableLines.of(listOf(BREAKABLE_LINE_3)) }
        runInEdtAndWait { FileEditorManager.getInstance(project).openFile(file, false) }
        assertNotNull(cache.fetch(file))
        assertTrue(cache.keeps(file))
        runInEdtAndWait { FileEditorManager.getInstance(project).closeFile(file) }
        assertFalse("a closed file's answer is dropped", cache.keeps(file))

        assertNotNull(cache.fetch(file))
        runInEdtAndWait {
            val document = checkNotNull(FileDocumentManager.getInstance().getDocument(file))
            WriteAction.run<Throwable> {
                CommandProcessor.getInstance().executeCommand(project, { document.insertString(0, " ") }, null, null)
            }
        }
        assertNull(cache.current(file))
        assertFalse("an older text's answer is dropped", cache.keeps(file))
    }

    // ---- Against the real runtime ----

    fun testATemporaryBreakpointIsRemovedAfterItsStop() {
        NewspeakSettings.getInstance().state.runtimePath = runtime.toString()
        cache.fetcher = { null }
        val program = program("Loop", LOOP)
        val temporary = addLineBreakpoint(program, LOOP_BODY_LINE, NewspeakLineBreakpointProperties(), temporary = true)
        val debugging = debug(program)

        assertEquals(LOOP_BODY_LINE, debugging.nextPause().line)
        assertTrue("still there while stopped", breakpoints.allBreakpoints.contains(temporary))
        runInEdtAndWait { debugging.session.resume() }

        debugging.awaitEnd("the session's end")
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertEquals(0, debugging.process.processHandler.exitCode)
        assertTrue("one stop, in the first iteration: ${debugging.pauses}", debugging.pauses.isEmpty())
        assertFalse("removed once hit", breakpoints.allBreakpoints.contains(temporary))
        assertTrue(debugging.output(), Regex("""1\s+2\s+3""").containsMatchIn(debugging.output()))
    }

    /**
     * A method breakpoint names a class inside a module after a `/`, as the
     * agent reads it: `Demo/Account>>deposit:` stops at the method's entry,
     * its first statement, and at its exit.
     */
    fun testAMethodBreakpointOnANestedClassStops() {
        NewspeakSettings.getInstance().state.runtimePath = runtime.toString()
        cache.fetcher = { null }
        val program = program("Demo", DEMO)
        addMethodBreakpoint("Demo/Account>>deposit:")
        val debugging = debug(program)

        val entry = debugging.nextPause()
        assertEquals(program, entry.file)
        assertEquals(DEMO_DEPOSIT_LINE, entry.line)
        runInEdtAndWait { debugging.session.resume() }
        assertEquals("the exit", program, debugging.nextPause().file)
        runInEdtAndWait { debugging.session.resume() }

        debugging.awaitEnd("the session's end")
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertEquals(0, debugging.process.processHandler.exitCode)
        assertTrue("no other stop: ${debugging.pauses}", debugging.pauses.isEmpty())
        assertTrue(debugging.output(), debugging.output().contains("balance 5"))
    }

    /**
     * An exception breakpoint names a shipped module's class by the
     * module's id: `newspeak.core.Kernel/Error` for caught exceptions stops
     * a caught `Error` at its handler's first statement. "Any exception" is
     * off, or it would take every class.
     */
    fun testAnExceptionBreakpointByIdStops() {
        NewspeakSettings.getInstance().state.runtimePath = runtime.toString()
        cache.fetcher = { null }
        for (any in breakpoints.getDefaultBreakpoints(exceptionType())) {
            any.properties.uncaught = false
            any.properties.halt = false
        }
        val program = program("Caught", CAUGHT)
        runInEdtAndWait {
            WriteAction.run<Throwable> {
                breakpoints.addBreakpoint(
                    exceptionType(),
                    NewspeakExceptionBreakpointProperties().apply {
                        className = "newspeak.core.Kernel/Error"
                        uncaught = false
                        caught = true
                    },
                )
            }
        }
        val debugging = debug(program)

        val stop = debugging.nextPause()
        assertEquals(program, stop.file)
        assertEquals(CAUGHT_HANDLER_LINE, stop.line)
        runInEdtAndWait { debugging.session.resume() }

        debugging.awaitEnd("the session's end")
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertEquals(0, debugging.process.processHandler.exitCode)
        assertTrue("one stop: ${debugging.pauses}", debugging.pauses.isEmpty())
        assertTrue(debugging.output(), debugging.output().contains("caught refused"))
    }

    // ---- Helpers ----

    private val runtime: Path by lazy { Path.of(System.getProperty("nsNext.runtime")) }

    private fun lineType() = XDebuggerUtil.getInstance().findBreakpointType(NewspeakLineBreakpointType::class.java)

    private fun methodType() = XDebuggerUtil.getInstance().findBreakpointType(NewspeakMethodBreakpointType::class.java)

    private fun exceptionType() = XDebuggerUtil.getInstance().findBreakpointType(NewspeakExceptionBreakpointType::class.java)

    private fun position(file: VirtualFile, line: Int): XSourcePosition = checkNotNull(XDebuggerUtil.getInstance().createPosition(file, line))

    private fun addLineBreakpoint(
        file: VirtualFile,
        line: Int,
        properties: NewspeakLineBreakpointProperties,
        temporary: Boolean = false,
    ): XLineBreakpoint<NewspeakLineBreakpointProperties> = runInEdtAndGet {
        WriteAction.compute<XLineBreakpoint<NewspeakLineBreakpointProperties>, Throwable> {
            breakpoints.addLineBreakpoint(lineType(), file.url, line, properties).apply { isTemporary = temporary }
        }
    }

    private fun addMethodBreakpoint(name: String): XBreakpoint<NewspeakMethodBreakpointProperties> = runInEdtAndGet {
        WriteAction.compute<XBreakpoint<NewspeakMethodBreakpointProperties>, Throwable> {
            breakpoints.addBreakpoint(methodType(), NewspeakMethodBreakpointProperties().apply { methodName = name })
        }
    }

    private fun program(name: String, source: String): VirtualFile {
        val path = directory.resolve("$name.ns")
        Files.writeString(path, source)
        return checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(path))
    }

    private fun waitUntil(what: String, condition: () -> Boolean) {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(WAIT_SECONDS)
        while (!condition()) {
            if (System.nanoTime() > deadline) throw AssertionError("not within $WAIT_SECONDS s: $what")
            Thread.sleep(POLL_MILLISECONDS)
        }
    }

    /**
     * A session that records what the handlers tell it of each breakpoint,
     * by its line counted from 1; it answers nothing else.
     */
    private class RecordingSession {
        val calls = CopyOnWriteArrayList<String>()
        val proxy: XDebugSession = Proxy.newProxyInstance(
            XDebugSession::class.java.classLoader,
            arrayOf(XDebugSession::class.java),
        ) { _, method, arguments ->
            when (method.name) {
                "setBreakpointVerified", "setBreakpointInvalid", "updateBreakpointPresentation" -> {
                    val breakpoint = arguments[0] as XLineBreakpoint<*>
                    val message = arguments.drop(1).filterIsInstance<String>().joinToString(" ")
                    calls.add(listOf(method.name, (breakpoint.line + 1).toString(), message).filter(String::isNotEmpty).joinToString(" "))
                    null
                }
                "reportError", "reportMessage" -> {
                    calls.add("${method.name} ${arguments[0]}")
                    null
                }
                "hashCode" -> System.identityHashCode(this)
                "equals" -> false
                "toString" -> "a recording session"
                else -> if (method.returnType == Boolean::class.javaPrimitiveType) false else null
            }
        } as XDebugSession
    }

    /** A scripted agent that answers every request with what [answer] gives and records the requests. */
    private inner class RecordingAgent(private val answer: (String) -> JsonObject) {
        private val received = CopyOnWriteArrayList<JsonObject>()
        val client: DapClient

        init {
            val towardsAgent = PipedOutputStream()
            val towardsClient = PipedOutputStream()
            val clientSide = DapConnection(PipedInputStream(towardsClient, PIPE_SIZE), towardsAgent)
            val agentSide = DapConnection(PipedInputStream(towardsAgent, PIPE_SIZE), towardsClient)
            opened.add(agentSide)
            val responses = AtomicInteger(1)
            agentSide.start(
                { request ->
                    received.add(request)
                    val command = request.get("command").asString
                    agentSide.send(
                        JsonObject().apply {
                            addProperty("seq", responses.getAndIncrement())
                            addProperty("type", "response")
                            addProperty("request_seq", request.get("seq").asInt)
                            addProperty("command", command)
                            addProperty("success", true)
                            add("body", answer(command))
                        },
                    )
                },
                { _ -> },
            )
            client = DapClient(clientSide)
            opened.add(AutoCloseable(client::close))
        }

        fun requests(command: String): List<JsonObject> =
            received.filter { request -> request.get("command").asString == command }
                .map { request -> request.getAsJsonObject("arguments") }
    }

    /** One session the test started, and what the test waits for in it. */
    private inner class Debugging(val process: XDebugProcess) {
        val session: XDebugSession get() = process.session
        val pauses = LinkedBlockingQueue<XSourcePosition>()
        val stopped = CompletableFuture<Unit>()
        private val text = StringBuffer()

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
                    }
                },
            )
        }

        fun nextPause(): XSourcePosition =
            pauses.poll(WAIT_SECONDS, TimeUnit.SECONDS) ?: throw AssertionError("no stop within $WAIT_SECONDS s: ${output()}")

        /**
         * Waits for the session to stop; a session that does not names
         * [what] it waited for, the stops it made and the program's output,
         * so that a timeout says where the run stood.
         */
        fun awaitEnd(what: String) {
            try {
                stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
            } catch (_: TimeoutException) {
                throw AssertionError("not within $WAIT_SECONDS s: $what; paused at ${pauses.map(::describe)}, suspended ${session.isSuspended}; output: ${output()}")
            }
        }

        fun output(): String = text.toString()
    }

    private fun describe(position: XSourcePosition): String = "${position.file.name}:${position.line}"

    /** Debug on a Newspeak run configuration of `program`, as the user starts it; its listeners see every stop. */
    private fun debug(program: VirtualFile): Debugging {
        val configuration = NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration
        configuration.program = program.path
        val environment = ExecutionEnvironmentBuilder.create(DefaultDebugExecutor.getDebugExecutorInstance(), configuration).build()
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
        runInEdtAndWait { environment.runner.execute(environment) }
        return try {
            started.get(WAIT_SECONDS, TimeUnit.SECONDS)
        } catch (_: TimeoutException) {
            throw AssertionError("no debug process started within $WAIT_SECONDS s")
        }
    }

    private companion object {
        const val WAIT_SECONDS = 30L
        const val WAIT_MILLISECONDS = 30_000L
        const val POLL_MILLISECONDS = 20L
        const val PIPE_SIZE = 64 * 1024

        /** A module whose line 3, counted from 0, holds a statement and a block, line 4 a statement, and line 2 a comment. */
        val LINES = """
            class Lines = Object new () (
              public main: platform args: arguments = (
                /// The elements, printed.
                {1. 2} collect: [:each | each printString].
                ^arguments
              )
            ) : ()
        """.trimIndent() + "\n"

        /** Where the block of line 3 starts its first statement, counted from 0 in UTF-16 code units. */
        const val BLOCK_COLUMN = 29

        val BREAKABLE_LINE_3 = BreakableLine(3, listOf(BlockPlace(Range(Position(3, 20), Position(3, 46)), Position(3, BLOCK_COLUMN))))
        val BREAKABLE_LINE_4 = BreakableLine(4, emptyList())

        /** A loop whose body, line 3 counted from 0, runs three times. */
        val LOOP = """
            class Loop = Object new () (
              public main: platform args: arguments = (
                1 to: 3 do: [:index |
                  platform transcript show: index printString; newLine].
              )
            ) : ()
        """.trimIndent() + "\n"
        const val LOOP_BODY_LINE = 3

        /**
         * A loop of three whose body, line 6 counted from 0, a condition
         * stands on that counts its evaluations, which the program prints.
         */
        val COUNTED = """
            class Counted = Object new (
              | hits ::= 0. |
            ) (
              public main: platform args: arguments = (
                1 to: 3 do: [:index |
                  index printString.
                ].
                platform transcript show: 'hits ' , hits printString; newLine.
              )
              countHit = (
                hits:: hits + 1.
                ^true
              )
            ) : ()
        """.trimIndent() + "\n"
        const val COUNTED_BODY_LINE = 5

        /** A module whose nested `Account`'s `deposit:` has its first statement on line 5, counted from 0. */
        val DEMO = """
            class Demo = Object new () (
              class Account = Object new (
                | public balance ::= 0. |
              ) (
                public deposit: amount = (
                  balance:: balance + amount.
                )
              ) : ()
              public main: platform args: arguments = (
                | account = Account new. |
                account deposit: 5.
                platform transcript show: 'balance ' , account balance printString; newLine.
              )
            ) : ()
        """.trimIndent() + "\n"
        const val DEMO_DEPOSIT_LINE = 5

        /** A module that catches an `Error`, its handler's first statement on line 4, counted from 0. */
        val CAUGHT = """
            class Caught = Object new () (
              public main: platform args: arguments = (
                | answer |
                answer:: [Error signal: 'refused'] on: Error do: [:error |
                  'caught ' , error messageText].
                platform transcript show: answer; newLine.
              )
            ) : ()
        """.trimIndent() + "\n"
        const val CAUGHT_HANDLER_LINE = 4
    }
}
