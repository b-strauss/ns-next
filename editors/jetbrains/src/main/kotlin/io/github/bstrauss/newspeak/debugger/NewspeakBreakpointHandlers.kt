package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonArray
import com.google.gson.JsonObject
import com.intellij.icons.AllIcons
import com.intellij.openapi.ui.MessageType
import com.intellij.openapi.vfs.VfsUtilCore
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.breakpoints.SuspendPolicy
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointHandler
import com.intellij.xdebugger.breakpoints.XBreakpointType
import com.intellij.xdebugger.breakpoints.XLineBreakpoint
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.dap.DapBreakpoint
import io.github.bstrauss.newspeak.debugger.dap.DapBreakpointEvent
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapConnectionClosedException
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap

/*
 * The breakpoints of a session, as the agent knows them. DAP sets each
 * kind as a whole -- line breakpoints a file's set at a time -- so every
 * change resends its set. The platform registers each breakpoint as the
 * session starts, before the agent can take one; they are held until the
 * agent's `initialized` event (`sendAll`) and sent as they change from
 * then on. Answers come in the order of the requests, so the last answer
 * for a set is its latest.
 */

/*
 * A breakpoint's condition is decided by the runtime, as the agent's
 * `condition`, never by the session: the platform leaves a condition to
 * the debug process, and this one sends it, so that a condition with a
 * side effect runs once at each hit. A log expression is the agent's
 * `logMessage` where nothing else needs the stop (`isLogpoint`): the
 * agent logs it and goes on, as a DAP logpoint. Where the breakpoint
 * stops anyway -- it suspends, or the platform logs its hit or the stack
 * there, or it is a method breakpoint, which DAP gives no log message --
 * the session evaluates the expression at the stop, in its top frame,
 * once, and hands the platform its text to log.
 */

/** The breakpoint's condition, null for none or a blank one. */
internal fun conditionOf(breakpoint: XBreakpoint<*>): String? = breakpoint.conditionExpression?.expression?.trim()?.takeIf(String::isNotEmpty)

/** The breakpoint's log expression, null for none or a blank one. */
internal fun logExpressionOf(breakpoint: XBreakpoint<*>): String? = breakpoint.logExpressionObject?.expression?.trim()?.takeIf(String::isNotEmpty)

/** Whether a line breakpoint is the agent's logpoint: it logs an expression and nothing needs it to stop. */
internal fun isLogpoint(breakpoint: XBreakpoint<*>): Boolean =
    breakpoint is XLineBreakpoint<*> &&
        logExpressionOf(breakpoint) != null &&
        breakpoint.suspendPolicy == SuspendPolicy.NONE &&
        !breakpoint.isLogMessage &&
        !breakpoint.isLogStack

/**
 * The agent's log message that logs [expression]'s value: the expression
 * in braces, after a blank where it starts with a brace of its own, since
 * `{{` is the message's escape of a brace.
 */
internal fun logMessageOf(expression: String): String = "{" + (if (expression.startsWith("{")) " " else "") + expression + "}"

internal typealias NewspeakLineBreakpoint = XLineBreakpoint<NewspeakLineBreakpointProperties>
private typealias NewspeakMethodBreakpoint = XBreakpoint<NewspeakMethodBreakpointProperties>
private typealias NewspeakExceptionBreakpoint = XBreakpoint<NewspeakExceptionBreakpointProperties>

/**
 * The handling every kind shares: the breakpoints registered, by the set
 * each belongs to, the client once there is one, and the breakpoint each
 * of the agent's ids names, for a stop's `hitBreakpointIds` and a
 * `breakpoint` event.
 */
abstract class NewspeakBreakpointHandler<B : XBreakpoint<*>>(
    typeClass: Class<out XBreakpointType<B, *>>,
    protected val session: XDebugSession,
) : XBreakpointHandler<B>(typeClass) {
    private val lock = Any()
    private val bySet = LinkedHashMap<String, MutableList<B>>()
    private var client: DapClient? = null
    private val byId = ConcurrentHashMap<Int, B>()
    private val idsBySet = ConcurrentHashMap<String, List<Int>>()

    /** The set [breakpoint] is sent in. */
    protected abstract fun setOf(breakpoint: B): String

    /** Sends [set]'s [breakpoints], all enabled, and takes the agent's answer; null for a set the agent cannot name. */
    protected abstract fun send(client: DapClient, set: String, breakpoints: List<B>): CompletableFuture<*>?

    override fun registerBreakpoint(breakpoint: B) {
        synchronized(lock) {
            val breakpoints = bySet.getOrPut(setOf(breakpoint)) { mutableListOf() }
            // Registered twice, it would be sent twice and answered under two ids.
            if (breakpoint in breakpoints) return
            breakpoints.add(breakpoint)
            sendSet(setOf(breakpoint))
        }
    }

    override fun unregisterBreakpoint(breakpoint: B, temporary: Boolean) {
        synchronized(lock) {
            bySet[setOf(breakpoint)]?.remove(breakpoint)
            sendSet(setOf(breakpoint))
        }
    }

    /** Sends every set to [client], and every later change: the answers' futures. */
    fun sendAll(client: DapClient): List<CompletableFuture<*>> = synchronized(lock) {
        this.client = client
        sets().mapNotNull(::sendSet)
    }

    /** The sets sent at the start: each one a breakpoint was registered in. */
    protected open fun sets(): List<String> = bySet.keys.toList()

    /** The breakpoint the agent's [id] names, if it is still one of this session's. */
    fun breakpointOf(id: Int): B? = byId[id]

    /** Whether the agent's [id] names one of these breakpoints, which a `breakpoint` event then updates. */
    fun changed(event: DapBreakpointEvent): Boolean {
        val id = event.breakpoint.id ?: return false
        val breakpoint = byId[id] ?: return false
        present(breakpoint, event.breakpoint)
        return true
    }

    /** Shows what the agent says of [breakpoint]. */
    protected abstract fun present(breakpoint: B, answer: DapBreakpoint)

    private fun sendSet(set: String): CompletableFuture<*>? {
        val client = client ?: return null
        val breakpoints = bySet[set].orEmpty().filter(XBreakpoint<*>::isEnabled)
        return send(client, set, breakpoints)?.whenComplete { _, failure -> if (failure != null) reportFailure(failure) }
    }

    /** Takes the agent's answer to [set]'s [breakpoints]: each one's id and what it says of it, in order. */
    protected fun answered(set: String, breakpoints: List<B>, body: JsonObject) {
        val answers = breakpointsOf(body)
        idsBySet.remove(set)?.forEach(byId::remove)
        val ids = mutableListOf<Int>()
        for ((index, breakpoint) in breakpoints.withIndex()) {
            // An answer short of the set leaves none of them shown as an older answer said.
            val answer = answers.getOrNull(index)
                ?: DapBreakpoint(id = null, verified = false, message = NewspeakBundle.message("debugger.breakpoint.unanswered"), line = null, column = null)
            answer.id?.let { id ->
                byId[id] = breakpoint
                ids.add(id)
            }
            present(breakpoint, answer)
        }
        idsBySet[set] = ids
    }

    private fun reportFailure(failure: Throwable) {
        if (causeOf(failure) is DapConnectionClosedException) return
        session.reportError(NewspeakBundle.message("debugger.error.request", messageOf(failure)))
    }

    private fun breakpointsOf(body: JsonObject): List<DapBreakpoint> =
        body.get("breakpoints")
            ?.takeIf { element -> element.isJsonArray }
            ?.asJsonArray
            ?.filter { element -> element.isJsonObject }
            ?.map { element -> DapBreakpoint.from(element.asJsonObject) }
            .orEmpty()
}

/**
 * Line breakpoints, a file's set at a time. A file on disk is named by its
 * path -- a program's module's file, and a file of the core library, which
 * the agent takes for the shipped module only in the library the session
 * handed it, so that no program module of a shipped module's name ever
 * takes its breakpoints -- and a module shown by reference by its name.
 */
class NewspeakLineBreakpointHandler(session: XDebugSession) :
    NewspeakBreakpointHandler<NewspeakLineBreakpoint>(NewspeakLineBreakpointType::class.java, session) {
    override fun setOf(breakpoint: NewspeakLineBreakpoint): String = breakpoint.fileUrl

    override fun send(client: DapClient, set: String, breakpoints: List<NewspeakLineBreakpoint>): CompletableFuture<*>? {
        val source = sourceOf(set) ?: return null
        val arguments = JsonObject().apply {
            add("source", source)
            add(
                "breakpoints",
                JsonArray().apply {
                    for (breakpoint in breakpoints) {
                        add(
                            JsonObject().apply {
                                addProperty("line", breakpoint.line + 1)
                                breakpoint.properties?.column?.let { column -> addProperty("column", column + 1) }
                                breakpoint.properties?.hitCondition?.let { condition -> addProperty("hitCondition", condition) }
                                conditionOf(breakpoint)?.let { condition -> addProperty("condition", condition) }
                                if (isLogpoint(breakpoint)) addProperty("logMessage", logMessageOf(checkNotNull(logExpressionOf(breakpoint))))
                            },
                        )
                    }
                },
            )
        }
        return client.request("setBreakpoints", arguments).thenAccept { body -> answered(set, breakpoints, body) }
    }

    /**
     * Verified where it was put, verified and moved where the agent sets it
     * at another line -- the breakpoint stays where the user put it, and
     * says where it stops -- or invalid with the agent's reason.
     */
    override fun present(breakpoint: NewspeakLineBreakpoint, answer: DapBreakpoint) {
        val line = answer.line
        when {
            !answer.verified -> session.setBreakpointInvalid(breakpoint, answer.message)
            line != null && line != breakpoint.line + 1 -> session.updateBreakpointPresentation(
                breakpoint,
                AllIcons.Debugger.Db_verified_breakpoint,
                NewspeakBundle.message("debugger.breakpoint.moved", line.toString()),
            )
            else -> session.setBreakpointVerified(breakpoint)
        }
    }

    /** How the agent names the file at [fileUrl]: by path for a file on disk, by name for a module's text shown by reference. */
    private fun sourceOf(fileUrl: String): JsonObject? {
        if (fileUrl.startsWith("file:")) {
            return JsonObject().apply { addProperty("path", Path.of(VfsUtilCore.urlToPath(fileUrl)).toString()) }
        }
        val name = fileUrl.substringAfterLast('/').takeIf { name -> name.endsWith(".ns") } ?: return null
        return JsonObject().apply { addProperty("name", name) }
    }
}

/** Method breakpoints, one set, by the method's name in the agent's form. */
class NewspeakMethodBreakpointHandler(session: XDebugSession) :
    NewspeakBreakpointHandler<NewspeakMethodBreakpoint>(NewspeakMethodBreakpointType::class.java, session) {
    override fun setOf(breakpoint: NewspeakMethodBreakpoint): String = SET

    /** Sent at the start even when empty: the agent's set is then known to be the session's. */
    override fun sets(): List<String> = listOf(SET)

    override fun send(client: DapClient, set: String, breakpoints: List<NewspeakMethodBreakpoint>): CompletableFuture<*> {
        val arguments = JsonObject().apply {
            add(
                "breakpoints",
                JsonArray().apply {
                    for (breakpoint in breakpoints) {
                        add(
                            JsonObject().apply {
                                addProperty("name", breakpoint.properties.methodName.orEmpty().trim())
                                breakpoint.properties.hitCondition?.let { condition -> addProperty("hitCondition", condition) }
                                conditionOf(breakpoint)?.let { condition -> addProperty("condition", condition) }
                            },
                        )
                    }
                },
            )
        }
        return client.request("setFunctionBreakpoints", arguments).thenAccept { body -> answered(set, breakpoints, body) }
    }

    /**
     * Only a line breakpoint shows whether it is verified; a method one the
     * agent cannot set -- its name refused, or no built module declaring
     * the method yet, which it sets once one does -- is reported with the
     * agent's reason.
     */
    override fun present(breakpoint: NewspeakMethodBreakpoint, answer: DapBreakpoint) {
        if (answer.verified) return
        val reason = answer.message ?: return
        session.reportMessage(
            NewspeakBundle.message("debugger.breakpoint.method.unverified", breakpoint.properties.methodName.orEmpty(), reason),
            MessageType.WARNING,
        )
    }

    private companion object {
        const val SET = "methods"
    }
}

/**
 * Exception breakpoints, one set, as the agent's filters `uncaught`,
 * `caught` and `halt`, each class breakpoint's filters an option whose
 * condition names its class. The agent keeps one list of classes for both
 * filters, so "Any exception" taking uncaught or caught exceptions takes
 * every class for both: the class breakpoints' filters are then sent
 * without their classes.
 */
class NewspeakExceptionBreakpointHandler(session: XDebugSession) :
    NewspeakBreakpointHandler<NewspeakExceptionBreakpoint>(NewspeakExceptionBreakpointType::class.java, session) {
    override fun setOf(breakpoint: NewspeakExceptionBreakpoint): String = SET

    /** Sent at the start even when empty, or the agent's own defaults would stand. */
    override fun sets(): List<String> = listOf(SET)

    override fun send(client: DapClient, set: String, breakpoints: List<NewspeakExceptionBreakpoint>): CompletableFuture<*> {
        val anyClass = breakpoints.map(XBreakpoint<NewspeakExceptionBreakpointProperties>::getProperties).filter { properties -> properties.className == null }
        val classes = breakpoints.map(XBreakpoint<NewspeakExceptionBreakpointProperties>::getProperties).filter { properties -> properties.className != null }
        val everyClass = anyClass.any { properties -> properties.uncaught || properties.caught }
        val filters = linkedSetOf<String>()
        val options = JsonArray()
        for ((filter, takes) in listOf<Pair<String, (NewspeakExceptionBreakpointProperties) -> Boolean>>(
            UNCAUGHT to { properties -> properties.uncaught },
            CAUGHT to { properties -> properties.caught },
        )) {
            if (anyClass.any(takes)) filters.add(filter)
            for (properties in classes.filter(takes)) {
                if (everyClass) {
                    filters.add(filter)
                } else {
                    options.add(
                        JsonObject().apply {
                            addProperty("filterId", filter)
                            addProperty("condition", properties.className!!.trim())
                        },
                    )
                }
            }
        }
        if (anyClass.any { properties -> properties.halt }) filters.add(HALT)
        val arguments = JsonObject().apply {
            add("filters", JsonArray().apply { filters.forEach(::add) })
            add("filterOptions", options)
        }
        return client.request("setExceptionBreakpoints", arguments)
    }

    /** An exception breakpoint has no place to show it in. */
    override fun present(breakpoint: NewspeakExceptionBreakpoint, answer: DapBreakpoint) {}

    private companion object {
        const val SET = "exceptions"
        const val UNCAUGHT = "uncaught"
        const val CAUGHT = "caught"
        const val HALT = "halt"
    }
}
