package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.execution.process.ProcessEvent
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessListener
import com.intellij.execution.ui.ConsoleViewContentType
import com.intellij.execution.ui.ExecutionConsole
import com.intellij.execution.ui.RunnerLayoutUi
import com.intellij.execution.ui.layout.PlaceInGrid
import com.intellij.icons.AllIcons
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.editor.Document
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.fileTypes.FileType
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.MessageType
import com.intellij.openapi.util.Disposer
import com.intellij.util.concurrency.AppExecutorUtil
import com.intellij.xdebugger.XDebugProcess
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebuggerUtil
import com.intellij.xdebugger.XExpression
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointHandler
import com.intellij.xdebugger.evaluation.EvaluationMode
import com.intellij.xdebugger.evaluation.XDebuggerEditorsProvider
import com.intellij.xdebugger.frame.XDropFrameHandler
import com.intellij.xdebugger.frame.XExecutionStack
import com.intellij.xdebugger.frame.XStackFrame
import com.intellij.xdebugger.frame.XSuspendContext
import com.intellij.xdebugger.stepping.XSmartStepIntoHandler
import com.intellij.xdebugger.ui.XDebugTabLayouter
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakFileType
import io.github.bstrauss.newspeak.debugger.dap.DapBreakpointEvent
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.debugger.dap.DapConnectionClosedException
import io.github.bstrauss.newspeak.debugger.dap.DapOutputEvent
import io.github.bstrauss.newspeak.debugger.dap.DapProcessEvent
import io.github.bstrauss.newspeak.debugger.dap.DapStackFrame
import io.github.bstrauss.newspeak.debugger.dap.DapStepInTarget
import io.github.bstrauss.newspeak.debugger.dap.DapStoppedEvent
import io.github.bstrauss.newspeak.debugger.dap.stringOrNull
import io.github.bstrauss.newspeak.debugger.memory.NewspeakMemoryView
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import java.io.IOException
import java.net.ServerSocket
import java.net.SocketTimeoutException
import java.nio.file.Path
import java.security.MessageDigest
import java.util.concurrent.CompletableFuture
import java.util.concurrent.Future
import java.util.concurrent.TimeUnit

/**
 * A debug session of one `ns-next run --debug-connect`, over the plugin's
 * DAP client: the runtime connects to [serverSocket], the session checks
 * that the connection is the process it started -- its process id, and
 * [secret], which the runner handed that process alone -- sets the
 * breakpoints and lets the program start; then it shows each stop and
 * carries the user's steps to the agent.
 *
 * Threads: the client hands every event and answer to its reader thread,
 * which must never wait for another answer. Every event is therefore
 * handed on at once to [work], a thread of the session's own that takes
 * them one at a time in the agent's order and may wait, bounded, for the
 * answers a stop needs before the platform is told of it (the stack's
 * first page, a module's text); the platform's session is told from
 * there, which it allows from any thread. The value tree's and the frames
 * view's requests complete their nodes from the reader thread, which the
 * nodes allow; the event dispatch thread never waits for an answer.
 */
class NewspeakDebugProcess(
    session: XDebugSession,
    private val serverSocket: ServerSocket,
    private val handler: NewspeakDebuggeeProcessHandler,
    private val console: ExecutionConsole?,
    private val secret: String,
) : XDebugProcess(session) {
    private val work = AppExecutorUtil.createBoundedApplicationPoolExecutor("Newspeak debugger", 1)
    private val lineBreakpoints = NewspeakLineBreakpointHandler(session)
    private val methodBreakpoints = NewspeakMethodBreakpointHandler(session)

    /** The temporary line breakpoints the last stop hit, which the next command removes before it goes on. */
    @Volatile
    private var hitTemporaries: List<NewspeakLineBreakpoint> = emptyList()
    private val exceptionBreakpoints = NewspeakExceptionBreakpointHandler(session)

    @Volatile
    private var client: DapClient? = null

    @Volatile
    private var sources: NewspeakSources? = null

    /** The session ends: the user stopped it, the program ended, or it failed. Nothing more is asked of the agent. */
    @Volatile
    private var ended = false

    /**
     * Whether a `process` event named the process this session started.
     * Nothing the connection says is acted on before: a peer that never
     * identifies itself is no more trusted than one that names another
     * process.
     */
    @Volatile
    private var verified = false

    /** The stop shown last, shown again when the agent refuses to leave it. */
    @Volatile
    private var lastStop: NewspeakSuspendContext? = null

    /**
     * Whether the configuration sent the step filters: from then on every
     * change of the settings sends them again, while before it the
     * configuration sends them as they are by then.
     */
    @Volatile
    private var stepFiltersSent = false

    /**
     * The settings' changes, heard while the session lasts: disposed at
     * its `stop()`, and with the project's step filters at the latest.
     */
    private val stepFiltersSubscription = Disposer.newDisposable("Newspeak step filters of a debug session").also { subscription ->
        Disposer.register(NewspeakSkippedClasses.getInstance(session.project), subscription)
    }

    /**
     * Held while the filters are read and their request queued, so that
     * two sends -- the configuration's and a change's -- reach the agent
     * in the order they read the settings, the later read always last.
     */
    private val stepFiltersLock = Any()

    /** Completes when the agent answered the first `terminate`; for the tests of the Stop. */
    internal val terminateAnswered = CompletableFuture<Unit>()

    /** The objects this session marked, which every later evaluation names by their labels. */
    val markers = NewspeakValueMarkers(session) { client }

    private val smartStepInto = NewspeakSmartStepIntoHandler(this)
    private val dropFrame = NewspeakDropFrameHandler(this)

    override fun getEditorsProvider(): XDebuggerEditorsProvider = NewspeakDebuggerEditorsProvider

    override fun getSmartStepIntoHandler(): XSmartStepIntoHandler<*> = smartStepInto

    override fun getDropFrameHandler(): XDropFrameHandler = dropFrame

    /** The session's memory view, a panel of its tab, made once the tab is. */
    internal val memoryView = NewspeakMemoryView(session) { client }

    /**
     * The frames view's Show Library Frames applies to this session: the
     * core library's frames are library frames, folded while it is off
     * ([NewspeakExecutionStack]).
     */
    override fun isLibraryFrameFilterSupported(): Boolean = true

    /** The session's tab gains the memory view, minimized beside the variables, as Java's Memory tab is. */
    override fun createTabLayouter(): XDebugTabLayouter = object : XDebugTabLayouter() {
        override fun registerAdditionalContent(ui: RunnerLayoutUi) {
            val content = ui.createContent(
                MEMORY_CONTENT,
                memoryView.component,
                NewspeakBundle.message("debugger.memory.title"),
                AllIcons.Debugger.Db_db_object,
                null,
            )
            content.isCloseable = false
            ui.addContent(content, 0, PlaceInGrid.right, true)
        }
    }

    override fun getBreakpointHandlers(): Array<XBreakpointHandler<*>> = arrayOf(lineBreakpoints, methodBreakpoints, exceptionBreakpoints)

    override fun doGetProcessHandler(): ProcessHandler = handler

    /** The process's own console: the program's standard streams stay its own, never relayed by the agent. */
    override fun createConsole(): ExecutionConsole = console ?: super.createConsole()

    override fun sessionInitialized() {
        ApplicationManager.getApplication().messageBus.connect(stepFiltersSubscription)
            .subscribe(NewspeakDebuggerSettings.TOPIC, StepFiltersListener { client?.let(::changeStepFilters) })
        // A use of the library, and a look at whether the binary's sources changed since the project opened, while the runtime starts.
        val library = NewspeakCoreLibrary.getInstance(session.project).refreshForDebugging()
        work.execute { connect(library) }
    }

    // ---- The start ----

    private fun connect(library: Future<Path?>) {
        val connection = try {
            DapConnection.accept(serverSocket, ACCEPT_MILLISECONDS)
        } catch (_: SocketTimeoutException) {
            fail(NewspeakBundle.message("debugger.error.no.connection", (ACCEPT_MILLISECONDS / 1000).toString()))
            return
        } catch (exception: IOException) {
            // A socket closed by stop() while it waited is no failure.
            if (!ended) fail(NewspeakBundle.message("debugger.error.connection", exception.message ?: exception.toString()))
            return
        } finally {
            serverSocket.close()
        }
        awaitLibrary(library)
        val client = DapClient(connection)
        this.client = client
        sources = NewspeakSources(client)
        if (ended) {
            client.close()
            return
        }
        client.onEvent("process") { body -> work.execute { checkProcess(DapProcessEvent.from(body)) } }
        client.onEvent("initialized") { _ -> work.execute { if (identified()) configure(client) } }
        client.onEvent("stopped") { body -> work.execute { if (identified()) stopped(client, DapStoppedEvent.from(body)) } }
        client.onEvent("breakpoint") { body -> work.execute { if (identified()) breakpointChanged(DapBreakpointEvent.from(body)) } }
        client.onEvent("output") { body -> work.execute { if (identified()) show(DapOutputEvent.from(body)) } }
        client.onEvent("terminated") { _ -> work.execute { if (identified()) terminated() } }
        client.onClose { work.execute(::connectionClosed) }
        val initialize = JsonObject().apply {
            addProperty("clientID", "jetbrains")
            addProperty("clientName", "Newspeak (ns-next) plugin")
            addProperty("adapterID", "ns-next")
            addProperty("linesStartAt1", true)
            addProperty("columnsStartAt1", true)
            addProperty("pathFormat", "path")
        }
        client.request("initialize", initialize)
            .thenCompose { _ -> client.request("attach", attachArguments()) }
            .whenComplete { _, failure ->
                if (failure != null && causeOf(failure) !is DapConnectionClosedException) {
                    work.execute { fail(NewspeakBundle.message("debugger.error.start", messageOf(failure))) }
                }
            }
    }

    /**
     * `attach`'s arguments: the core library the project shows, its
     * directory absolute, by which the agent names and shows the shipped
     * modules (`coreLibrary`); none while the project shows none.
     */
    internal fun attachArguments(): JsonObject = JsonObject().apply {
        addProperty("stopOnEntry", false)
        NewspeakCoreLibrary.getInstance(session.project).directory?.let { directory ->
            addProperty("coreLibrary", directory.toAbsolutePath().toString())
        }
    }

    /**
     * Waits, bounded, for the session's look at the core library: when the
     * binary was rebuilt with other shipped sources, it moves the line
     * breakpoints to the new library's files, which the session must send
     * rather than the old ones. Before the client exists, so that nothing
     * is sent meanwhile; a look that fails or takes too long leaves the
     * breakpoints where they are.
     */
    private fun awaitLibrary(library: Future<Path?>) {
        try {
            library.get(LIBRARY_SECONDS, TimeUnit.SECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        } catch (exception: Exception) {
            LOG.warn("the debug session starts without its look at the core library", exception)
        }
    }

    /**
     * Whether the connection is the process this session started: another
     * process on the machine could have connected to the port first. A
     * process id is no secret, so the event must return [secret] too,
     * which only the started process was given; the runtime returns none
     * when it was given none, which this session's process always is.
     */
    private fun checkProcess(event: DapProcessEvent) {
        if (ended) return
        val started = handler.process.pid()
        if (event.systemProcessId != started) {
            fail(NewspeakBundle.message("debugger.error.wrong.process", event.systemProcessId?.toString() ?: "?", started.toString()))
            return
        }
        val returned = event.secret
        // Compared in constant time, so that how long a refusal takes says nothing of a guess.
        if (returned == null || !MessageDigest.isEqual(returned.toByteArray(Charsets.UTF_8), secret.toByteArray(Charsets.UTF_8))) {
            val what = NewspeakBundle.message(if (returned == null) "debugger.error.secret.missing" else "debugger.error.secret.wrong")
            fail(NewspeakBundle.message("debugger.error.wrong.secret", started.toString(), what))
            return
        }
        verified = true
        handler.terminateThroughDebugger = { client?.let(::requestTerminate) != null }
    }

    /**
     * Whether the session may act on what the connection says: false, and
     * the session failed, while no `process` event named its process. The
     * agent sends that event before every other, so only a peer that is
     * not `ns-next`'s agent meets this.
     */
    private fun identified(): Boolean {
        if (ended) return false
        if (verified) return true
        fail(NewspeakBundle.message("debugger.error.unidentified"))
        return false
    }

    /** The agent takes the configuration: the breakpoints of every kind and the step filters, then the start. */
    private fun configure(client: DapClient) {
        if (ended) return
        stepFiltersSent = true
        val answers = lineBreakpoints.sendAll(client) + methodBreakpoints.sendAll(client) + exceptionBreakpoints.sendAll(client) +
            sendStepFilters(client)
        // The start waits for every answer, since DAP's order has the client configure before it.
        CompletableFuture.allOf(*answers.toTypedArray())
            .handle { _, _ -> client.request("configurationDone") }
            .thenCompose { done -> done }
            .whenComplete { _, failure -> if (failure != null) refused(failure) }
    }

    /**
     * Sends the step filters (`nsNext/setStepFilters`): the settings'
     * switch and the project's classes, by their files' paths. An entry
     * the agent cannot take it leaves out and says in an `output` event,
     * which the console shows; a refusal of the whole request is reported.
     */
    private fun sendStepFilters(client: DapClient): CompletableFuture<*> =
        synchronized(stepFiltersLock) {
            client.request("nsNext/setStepFilters", NewspeakDebuggerSettings.stepFilterArguments(session.project))
        }.whenComplete { _, failure -> if (failure != null) refused(failure) }

    /**
     * The settings changed: a session configured sends the filters again
     * at once, on the thread that changed them, which never waits for the
     * answer -- so that a step the user starts after a change is sent
     * after it, and runs under the filters it changed to.
     */
    private fun changeStepFilters(client: DapClient) {
        if (ended || !stepFiltersSent) return
        sendStepFilters(client)
    }

    // ---- Stops ----

    private fun stopped(client: DapClient, event: DapStoppedEvent) {
        if (ended) return
        val (frames, total) = framesFrom(client, 0)
        val context = NewspeakSuspendContext(NewspeakExecutionStack(this, frames, total))
        lastStop = context
        // The platform's `breakpointReached` takes one breakpoint: a stop that several hit shows the first of them.
        val breakpoint = event.hitBreakpointIds.firstNotNullOfOrNull { id ->
            lineBreakpoints.breakpointOf(id) ?: methodBreakpoints.breakpointOf(id)
        }
        hitTemporaries = event.hitBreakpointIds.mapNotNull(lineBreakpoints::breakpointOf).filter { hit -> hit.isTemporary }
        val failed = failedConditionMessage(event)
        if (failed != null) {
            // The runtime stops where a condition or log message failed, whatever the breakpoint's suspend
            // policy, so that the error is seen: never asking `breakpointReached`, which would resume a
            // breakpoint that suspends nothing and drop the error unseen.
            session.positionReached(context)
            session.reportMessage(failed, MessageType.WARNING)
            return
        }
        if (breakpoint == null) {
            session.positionReached(context)
        } else if (!session.breakpointReached(breakpoint, loggedAt(client, breakpoint, event, frames), context)) {
            // The platform's own decision not to suspend there, as a logging breakpoint's.
            resume(context)
            return
        }
        exceptionMessage(event)?.let { message -> session.reportMessage(message, MessageType.WARNING) }
    }

    /**
     * The text a breakpoint that stops logs: its log expression's value,
     * evaluated once, here, in the stop's top frame -- a logpoint's the
     * agent logs itself without stopping, and one whose condition or log
     * message failed stops with the error instead, which is not evaluated
     * again. The error of an evaluation that fails is the text logged, as
     * the platform's own debuggers log it.
     */
    private fun loggedAt(client: DapClient, breakpoint: XBreakpoint<*>, event: DapStoppedEvent, frames: List<NewspeakStackFrame>): String? {
        val expression = logExpressionOf(breakpoint) ?: return null
        if (isLogpoint(breakpoint) || event.text != null) return null
        val arguments = JsonObject().apply {
            addProperty("expression", expression)
            frames.firstOrNull()?.let { frame -> addProperty("frameId", frame.id) }
        }
        val text = try {
            client.request("evaluate", arguments).get(ANSWER_SECONDS, TimeUnit.SECONDS).stringOrNull("result").orEmpty()
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            return null
        } catch (exception: Exception) {
            messageOf(exception)
        }
        return text + "\n"
    }

    /** What a breakpoint's stop with an error tells the user: its condition or log message failed, and why. */
    private fun failedConditionMessage(event: DapStoppedEvent): String? {
        if (event.reason != "breakpoint") return null
        return event.text?.let { text -> NewspeakBundle.message("debugger.stopped.condition.failed", text) }
    }

    /**
     * What an exception's stop tells the user: the exception's class and
     * text, or that a `halt` stopped it; null for any other stop. Shown as
     * the debugger's balloon, which the platform shows whichever of the
     * session's tabs is in front, where the console holds the program's own
     * streams and would mix it into them.
     */
    private fun exceptionMessage(event: DapStoppedEvent): String? {
        if (event.reason != "exception") return null
        val description = event.description
        val text = event.text.orEmpty()
        return when {
            description != null -> NewspeakBundle.message("debugger.stopped.exception", description, text)
            text == "halt" -> NewspeakBundle.message("debugger.stopped.halt")
            else -> NewspeakBundle.message("debugger.stopped.exception.unnamed", text)
        }
    }

    /** A `breakpoint` event: the agent resolved, moved or set one of them unasked. */
    private fun breakpointChanged(event: DapBreakpointEvent) {
        if (!lineBreakpoints.changed(event)) methodBreakpoints.changed(event)
    }

    /**
     * A page of frames from [start], each with its place resolved, and the
     * stack's depth; none when the agent cannot answer, so that the stop is
     * still shown and can be left.
     */
    private fun framesFrom(client: DapClient, start: Int): Pair<List<NewspeakStackFrame>, Int> {
        val arguments = JsonObject().apply {
            addProperty("threadId", THREAD_ID)
            addProperty("startFrame", start)
            addProperty("levels", FRAMES_PAGE)
        }
        val body = try {
            client.request("stackTrace", arguments).get(ANSWER_SECONDS, TimeUnit.SECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            return emptyList<NewspeakStackFrame>() to start
        } catch (exception: Exception) {
            LOG.warn("the stack of a stop could not be read", exception)
            return emptyList<NewspeakStackFrame>() to start
        }
        val read = body.get("stackFrames")
            ?.takeIf { element -> element.isJsonArray }
            ?.asJsonArray
            ?.filter { element -> element.isJsonObject }
            ?.map { element -> DapStackFrame.from(element.asJsonObject) }
            .orEmpty()
        val answered = body.get("totalFrames")?.takeIf { element -> element.isJsonPrimitive }?.asInt ?: (start + read.size)
        val total = maxOf(answered, start + read.size)
        val frames = read.mapIndexed { index, frame -> NewspeakStackFrame(client, frame, positionOf(frame), total - 1 - (start + index)) }
        return frames to total
    }

    private fun positionOf(frame: DapStackFrame): XSourcePosition? {
        if (frame.line <= 0) return null
        val file = frame.source?.let { source -> sources?.fileFor(source) } ?: return null
        return XDebuggerUtil.getInstance().createPosition(file, frame.line - 1, maxOf(frame.column - 1, 0))
    }

    /** The frames from [start] up to [total], a page at a time, as the frames view asks for them. */
    internal fun addFramesFrom(start: Int, total: Int, container: XExecutionStack.XStackFrameContainer) {
        val client = client ?: return
        work.execute {
            var next = start
            while (next < total && !container.isObsolete && !ended) {
                val (frames, _) = framesFrom(client, next)
                if (frames.isEmpty()) {
                    container.addStackFrames(emptyList<XStackFrame>(), true)
                    return@execute
                }
                next += frames.size
                container.addStackFrames(frames, next >= total)
            }
        }
    }

    // ---- Going on ----

    override fun resume(context: XSuspendContext?) = goOn("continue")

    override fun startStepOver(context: XSuspendContext?) = goOn("next", line = true)

    override fun startStepInto(context: XSuspendContext?) = goOn("stepIn", line = true)

    override fun startStepOut(context: XSuspendContext?) = goOn("stepOut", line = true)

    override fun runToPosition(position: XSourcePosition, context: XSuspendContext?) {
        val sources = sources
        if (sources == null) {
            session.reportError(NewspeakBundle.message("debugger.error.not.started"))
            return
        }
        val source = sources.sourceOf(position.file)
        if (source == null) {
            session.reportError(NewspeakBundle.message("debugger.error.no.source", position.file.name))
            showLastStopAgain()
            return
        }
        goOn("nsNext/runToPosition") {
            add("source", source)
            addProperty("line", position.line + 1)
        }
    }

    /**
     * The targets of the stop's line (`stepInTargets`), asked of the frame
     * a step steps from: the top frame, or, where the agent refuses it --
     * an exception's stop, whose top frames are its machinery, hidden --
     * the first frame not hidden. None when the session has no stop.
     */
    internal fun stepInTargets(): CompletableFuture<List<DapStepInTarget>> {
        val client = client ?: return CompletableFuture.completedFuture(emptyList())
        val frames = lastStop?.stack?.firstFrames.orEmpty()
        val candidates = listOfNotNull(frames.firstOrNull(), frames.firstOrNull { frame -> !frame.hidden }).distinct()
        fun ask(index: Int): CompletableFuture<List<DapStepInTarget>> {
            val frame = candidates.getOrNull(index) ?: return CompletableFuture.completedFuture(emptyList())
            return client.request("stepInTargets", JsonObject().apply { addProperty("frameId", frame.id) })
                .thenApply { body ->
                    body.get("targets")
                        ?.takeIf { element -> element.isJsonArray }
                        ?.asJsonArray
                        ?.filter { element -> element.isJsonObject }
                        ?.map { element -> DapStepInTarget.from(element.asJsonObject) }
                        .orEmpty()
                }
                .exceptionallyCompose { failure ->
                    if (messageOf(failure) == TOP_FRAME_ONLY && index + 1 < candidates.size) ask(index + 1) else CompletableFuture.failedFuture(failure)
                }
        }
        return ask(0)
    }

    /** Steps into the send the agent's target [id] names, Smart Step Into's choice. */
    internal fun stepIntoTarget(id: Int) = goOn("stepIn") { addProperty("targetId", id) }

    /**
     * Restarts [frame] (`restartFrame`): the agent stops at its start, a
     * stop of the reason `restart`, which is shown as any other; a refusal
     * is reported, and the program stays at its stop.
     */
    internal fun restartFrame(frame: NewspeakStackFrame) {
        val client = client ?: return
        client.request("restartFrame", JsonObject().apply { addProperty("frameId", frame.id) })
            .whenComplete { _, failure -> if (failure != null) refused(failure) }
    }

    /**
     * Makes [frame] return the value of [expression], evaluated in it, or
     * `nil` for a blank one (`nsNext/forceReturn`); the stop that follows
     * is where its sender goes on. A failed evaluation, a withheld value
     * and a frame of the core library are refused, the program still at
     * its stop.
     */
    internal fun forceReturn(frame: NewspeakStackFrame, expression: String): CompletableFuture<JsonObject>? {
        val client = client ?: return null
        val arguments = JsonObject().apply {
            addProperty("frameId", frame.id)
            if (expression.isNotBlank()) addProperty("expression", expression)
        }
        return client.request("nsNext/forceReturn", arguments).whenComplete { _, failure -> if (failure != null) refused(failure) }
    }

    /**
     * The code [frame] stands in, as the agent lists it (`nsNext/frameCode`):
     * its bytecode marked where the frame stands and, where it goes on in
     * machine code, that code marked where its call returns. The future
     * completes on the connection's thread; a refusal -- a frame the stop
     * no longer has -- fails it.
     */
    internal fun frameCode(frame: NewspeakStackFrame): CompletableFuture<FrameCode> {
        val client = client ?: return CompletableFuture.failedFuture(IllegalStateException("not connected"))
        return client.request("nsNext/frameCode", JsonObject().apply { addProperty("frameId", frame.id) }).thenApply { response ->
            FrameCode(
                response.get("body").asString,
                response.get("bytecode").asString,
                response.get("machineCode")?.takeUnless { element -> element.isJsonNull }?.asString,
            )
        }
    }

    /** The frame-code tab that follows the session's selected frame, none while there is none. */
    internal var frameCodeFollower: FrameCodeFollower? = null

    /**
     * What lasts while the session does -- the frame-code tab's follower --:
     * disposed at its `stop()`, and with the project at the latest, so that
     * a project closed without a stop leaks nothing.
     */
    internal val sessionLifetime = Disposer.newDisposable(NewspeakSkippedClasses.getInstance(session.project), "Newspeak debug session")

    override fun startPausing() {
        val client = client ?: return
        client.request("pause", JsonObject().apply { addProperty("threadId", THREAD_ID) })
            .whenComplete { _, failure -> if (failure != null) refused(failure) }
    }

    /**
     * Asks the agent to go on. The platform has left the stop already; an
     * agent that refuses stays at it, so it is shown again.
     */
    private fun goOn(command: String, line: Boolean = false, more: JsonObject.() -> Unit = {}) {
        val client = client ?: return
        // The platform removes a temporary breakpoint hit at the stop once the session resumes, after
        // this command went out: a loop would reach it again first. The agent takes requests in order,
        // so the removal sent here, before the command, stands before the run goes on.
        hitTemporaries.forEach { hit -> lineBreakpoints.unregisterBreakpoint(hit, true) }
        hitTemporaries = emptyList()
        val arguments = JsonObject().apply {
            addProperty("threadId", THREAD_ID)
            if (line) addProperty("granularity", "line")
            more()
        }
        client.request(command, arguments).whenComplete { _, failure ->
            if (failure == null || refused(failure)) return@whenComplete
            showLastStopAgain()
        }
    }

    /** The platform left the stop when it asked to go on; the agent, which did not, is shown at it again. */
    private fun showLastStopAgain() {
        lastStop?.let { stop -> work.execute { if (!ended) session.positionReached(stop) } }
    }

    /** Reports what the agent refused; true when there is nothing to report, the session having ended. */
    private fun refused(failure: Throwable): Boolean {
        if (ended || causeOf(failure) is DapConnectionClosedException) return true
        session.reportError(NewspeakBundle.message("debugger.error.request", messageOf(failure)))
        return false
    }

    // ---- The end ----

    /**
     * The first Stop: the program ends as its own `terminate…` would, at
     * its next safepoint. The answer's future; null when the connection is
     * gone, so the process is killed instead. A connection that closes
     * before the answer kills it too, as the second Stop would, since
     * nobody is left to ask.
     */
    private fun requestTerminate(client: DapClient): CompletableFuture<JsonObject>? {
        if (client.isClosed) return null
        return client.request("terminate").whenComplete { _, failure ->
            if (failure == null) {
                terminateAnswered.complete(Unit)
            } else if (!handler.isProcessTerminated) {
                handler.killProcess()
            }
        }
    }

    /**
     * The session ends. A program still running is asked to terminate, and
     * the connection is kept until the agent answered or the process ended
     * -- closed at once, the request might never arrive -- but no longer
     * than [STOP_GRACE_SECONDS], after which the process is killed.
     */
    override fun stop() {
        ended = true
        Disposer.dispose(stepFiltersSubscription)
        Disposer.dispose(sessionLifetime)
        Disposer.dispose(memoryView)
        handler.terminateThroughDebugger = null
        serverSocket.close()
        val client = client ?: return
        val terminate = if (handler.isProcessTerminated) null else requestTerminate(client)
        if (terminate == null) {
            client.close()
            return
        }
        val processEnded = CompletableFuture<Unit>()
        handler.addProcessListener(
            object : ProcessListener {
                override fun processTerminated(event: ProcessEvent) {
                    processEnded.complete(Unit)
                }
            },
        )
        if (handler.isProcessTerminated) processEnded.complete(Unit)
        CompletableFuture.anyOf(terminate, processEnded)
            .orTimeout(STOP_GRACE_SECONDS, TimeUnit.SECONDS)
            .whenComplete { _, _ -> client.close() }
    }

    /** The agent ended the session: the program has ended, and its process ends the platform's session as it exits. */
    private fun terminated() {
        ended = true
        handler.terminateThroughDebugger = null
        client?.close()
    }

    /**
     * The connection closed. One that never named this session's process
     * is failed as any other word of it would be, so that the process
     * started is not left running undebugged.
     */
    private fun connectionClosed() {
        handler.terminateThroughDebugger = null
        if (ended || handler.isProcessTerminating || handler.isProcessTerminated) return
        if (!identified()) return
        ended = true
        session.reportMessage(NewspeakBundle.message("debugger.connection.closed"), MessageType.WARNING)
    }

    /**
     * Ends a session that cannot go on: reported, and the process killed --
     * not asked to end, which a process that is not this session's own
     * might ignore, and which the runtime waiting for its configuration
     * would only take once someone lets it start.
     */
    private fun fail(message: String) {
        if (ended) return
        ended = true
        session.reportError(message)
        handler.terminateThroughDebugger = null
        client?.close()
        if (!handler.isProcessTerminated) handler.killProcess()
    }

    private fun show(event: DapOutputEvent) {
        val type = if (event.category == "stderr") ConsoleViewContentType.ERROR_OUTPUT else ConsoleViewContentType.SYSTEM_OUTPUT
        session.consoleView?.print(event.output, type)
    }

    private companion object {
        val LOG = Logger.getInstance(NewspeakDebugProcess::class.java)

        /** The one thread the agent shows. */
        const val THREAD_ID = 1

        /** How long the runtime has to connect once started. */
        const val ACCEPT_MILLISECONDS = 30_000

        /** Bounds each answer a stop waits for, so that a stalled connection cannot hold the session's thread. */
        const val ANSWER_SECONDS = 30L

        /** Bounds the wait for the core library's look, which writes it when the binary's sources are new. */
        const val LIBRARY_SECONDS = 60L

        /** How long a stopped session keeps the connection for its `terminate` to be answered. */
        const val STOP_GRACE_SECONDS = 10L

        /** The frames a stop reads before it is shown, and a page of the rest. */
        const val FRAMES_PAGE = 50

        /** The memory view's content in the session's tab. */
        const val MEMORY_CONTENT = "NewspeakMemoryView"

        /** The agent's refusal of `stepInTargets` for a frame other than the one a step steps from. */
        const val TOP_FRAME_ONLY = "smart step into takes the top frame"
    }
}

/**
 * The editor of an expression -- the Evaluate dialog's, a watch's, a
 * breakpoint's condition and log expression, a value set -- as Newspeak
 * text, which the agent compiles in the stop's scope; it resolves nothing
 * itself, since the names it may use are the frame's.
 */
object NewspeakDebuggerEditorsProvider : XDebuggerEditorsProvider() {
    override fun getFileType(): FileType = NewspeakFileType

    override fun createDocument(
        project: Project,
        expression: XExpression,
        sourcePosition: XSourcePosition?,
        mode: EvaluationMode,
    ): Document = EditorFactory.getInstance().createDocument(expression.expression)
}

/** A frame's code as the agent lists it: the [body]'s name, its [bytecode] and its [machineCode], null for a frame that goes on in the interpreter. */
data class FrameCode(val body: String, val bytecode: String, val machineCode: String?)
