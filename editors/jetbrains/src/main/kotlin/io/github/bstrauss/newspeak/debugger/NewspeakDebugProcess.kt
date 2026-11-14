package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.ui.ConsoleViewContentType
import com.intellij.execution.ui.ExecutionConsole
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.editor.Document
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.fileTypes.FileType
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.MessageType
import com.intellij.util.concurrency.AppExecutorUtil
import com.intellij.xdebugger.XDebugProcess
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebuggerUtil
import com.intellij.xdebugger.XExpression
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.breakpoints.XBreakpointHandler
import com.intellij.xdebugger.evaluation.EvaluationMode
import com.intellij.xdebugger.evaluation.XDebuggerEditorsProvider
import com.intellij.xdebugger.frame.XExecutionStack
import com.intellij.xdebugger.frame.XStackFrame
import com.intellij.xdebugger.frame.XSuspendContext
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakFileType
import io.github.bstrauss.newspeak.debugger.dap.DapBreakpointEvent
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.debugger.dap.DapConnectionClosedException
import io.github.bstrauss.newspeak.debugger.dap.DapOutputEvent
import io.github.bstrauss.newspeak.debugger.dap.DapProcessEvent
import io.github.bstrauss.newspeak.debugger.dap.DapStackFrame
import io.github.bstrauss.newspeak.debugger.dap.DapStoppedEvent
import java.io.IOException
import java.net.ServerSocket
import java.net.SocketTimeoutException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit

/**
 * A debug session of one `ns-next run --debug-connect`, over the plugin's
 * DAP client: the runtime connects to [serverSocket], the session checks
 * that the connection is the process it started, sets the breakpoints and
 * lets the program start; then it shows each stop and carries the user's
 * steps to the agent.
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
) : XDebugProcess(session) {
    private val work = AppExecutorUtil.createBoundedApplicationPoolExecutor("Newspeak debugger", 1)
    private val lineBreakpoints = NewspeakLineBreakpointHandler(session)

    @Volatile
    private var client: DapClient? = null

    @Volatile
    private var sources: NewspeakSources? = null

    /** The session ends: the user stopped it, the program ended, or it failed. Nothing more is asked of the agent. */
    @Volatile
    private var ended = false

    /** The stop shown last, shown again when the agent refuses to leave it. */
    @Volatile
    private var lastStop: NewspeakSuspendContext? = null

    /** Completes when the agent answered the first `terminate`; for the tests of the Stop. */
    internal val terminateAnswered = CompletableFuture<Unit>()

    override fun getEditorsProvider(): XDebuggerEditorsProvider = NewspeakDebuggerEditorsProvider

    override fun getBreakpointHandlers(): Array<XBreakpointHandler<*>> = arrayOf(lineBreakpoints)

    override fun doGetProcessHandler(): ProcessHandler = handler

    /** The process's own console: the program's standard streams stay its own, never relayed by the agent. */
    override fun createConsole(): ExecutionConsole = console ?: super.createConsole()

    override fun sessionInitialized() {
        work.execute(::connect)
    }

    // ---- The start ----

    private fun connect() {
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
        val client = DapClient(connection)
        this.client = client
        sources = NewspeakSources(client)
        if (ended) {
            client.close()
            return
        }
        client.onEvent("process") { body -> work.execute { checkProcess(DapProcessEvent.from(body)) } }
        client.onEvent("initialized") { _ -> work.execute { configure(client) } }
        client.onEvent("stopped") { body -> work.execute { stopped(client, DapStoppedEvent.from(body)) } }
        client.onEvent("breakpoint") { body -> work.execute { lineBreakpoints.changed(DapBreakpointEvent.from(body)) } }
        client.onEvent("output") { body -> show(DapOutputEvent.from(body)) }
        client.onEvent("terminated") { _ -> work.execute(::terminated) }
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
            .thenCompose { _ -> client.request("attach", JsonObject().apply { addProperty("stopOnEntry", false) }) }
            .whenComplete { _, failure ->
                if (failure != null && causeOf(failure) !is DapConnectionClosedException) {
                    work.execute { fail(NewspeakBundle.message("debugger.error.start", messageOf(failure))) }
                }
            }
    }

    /**
     * Whether the connection is the process this session started: another
     * process on the machine could have connected to the port first.
     */
    private fun checkProcess(event: DapProcessEvent) {
        if (ended) return
        val started = handler.process.pid()
        if (event.systemProcessId != started) {
            fail(NewspeakBundle.message("debugger.error.wrong.process", event.systemProcessId?.toString() ?: "?", started.toString()))
            return
        }
        handler.terminateThroughDebugger = ::requestTerminate
    }

    /** The agent takes the configuration: the breakpoints, then the start. */
    private fun configure(client: DapClient) {
        if (ended) return
        val answers = lineBreakpoints.sendAll(client)
        // The start waits for every answer, since DAP's order has the client configure before it.
        CompletableFuture.allOf(*answers.toTypedArray())
            .handle { _, _ -> client.request("configurationDone") }
            .thenCompose { done -> done }
            .whenComplete { _, failure -> if (failure != null) refused(failure) }
    }

    // ---- Stops ----

    private fun stopped(client: DapClient, event: DapStoppedEvent) {
        if (ended) return
        val (frames, total) = framesFrom(client, 0)
        val context = NewspeakSuspendContext(NewspeakExecutionStack(this, frames, total))
        lastStop = context
        val breakpoint = event.hitBreakpointIds.firstNotNullOfOrNull(lineBreakpoints::breakpointOf)
        if (breakpoint == null) {
            session.positionReached(context)
        } else if (!session.breakpointReached(breakpoint, null, context)) {
            // The platform's own decision not to suspend there, as a logging breakpoint's.
            resume(context)
        }
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
        } catch (exception: Exception) {
            LOG.warn("the stack of a stop could not be read", exception)
            return emptyList<NewspeakStackFrame>() to start
        }
        val frames = body.get("stackFrames")
            ?.takeIf { element -> element.isJsonArray }
            ?.asJsonArray
            ?.filter { element -> element.isJsonObject }
            ?.map { element -> DapStackFrame.from(element.asJsonObject) }
            .orEmpty()
            .map { frame -> NewspeakStackFrame(client, frame, positionOf(frame)) }
        val total = body.get("totalFrames")?.takeIf { element -> element.isJsonPrimitive }?.asInt ?: (start + frames.size)
        return frames to maxOf(total, start + frames.size)
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
        val sources = sources ?: return
        goOn("nsNext/runToPosition") {
            add("source", sources.sourceOf(position.file))
            addProperty("line", position.line + 1)
        }
    }

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
        val arguments = JsonObject().apply {
            addProperty("threadId", THREAD_ID)
            if (line) addProperty("granularity", "line")
            more()
        }
        client.request(command, arguments).whenComplete { _, failure ->
            if (failure == null || refused(failure)) return@whenComplete
            lastStop?.let { stop -> work.execute { if (!ended) session.positionReached(stop) } }
        }
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
     * its next safepoint. False when the connection is gone, so the
     * process is killed instead.
     */
    private fun requestTerminate(): Boolean {
        val client = client ?: return false
        if (client.isClosed) return false
        client.request("terminate").whenComplete { _, failure ->
            if (failure == null) {
                terminateAnswered.complete(Unit)
            } else if (!handler.isProcessTerminated) {
                // Nobody is left to ask: as the second Stop would.
                handler.killProcess()
            }
        }
        return true
    }

    override fun stop() {
        if (!handler.isProcessTerminated) requestTerminate()
        ended = true
        handler.terminateThroughDebugger = null
        serverSocket.close()
        client?.close()
    }

    /** The agent ended the session: the program has ended, and its process ends the platform's session as it exits. */
    private fun terminated() {
        ended = true
        handler.terminateThroughDebugger = null
        client?.close()
    }

    private fun connectionClosed() {
        handler.terminateThroughDebugger = null
        if (ended || handler.isProcessTerminating || handler.isProcessTerminated) return
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

        /** The frames a stop reads before it is shown, and a page of the rest. */
        const val FRAMES_PAGE = 50
    }
}

/** The editor of an expression; evaluation is not supported yet, so it is plain Newspeak text. */
object NewspeakDebuggerEditorsProvider : XDebuggerEditorsProvider() {
    override fun getFileType(): FileType = NewspeakFileType

    override fun createDocument(
        project: Project,
        expression: XExpression,
        sourcePosition: XSourcePosition?,
        mode: EvaluationMode,
    ): Document = EditorFactory.getInstance().createDocument(expression.expression)
}
