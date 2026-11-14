package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonArray
import com.google.gson.JsonObject
import com.intellij.openapi.vfs.VfsUtilCore
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointHandler
import com.intellij.xdebugger.breakpoints.XBreakpointProperties
import com.intellij.xdebugger.breakpoints.XLineBreakpoint
import io.github.bstrauss.newspeak.debugger.dap.DapBreakpoint
import io.github.bstrauss.newspeak.debugger.dap.DapBreakpointEvent
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap

private typealias NewspeakLineBreakpoint = XLineBreakpoint<XBreakpointProperties<*>>

/**
 * The line breakpoints of a session, as the agent knows them: DAP sets a
 * file's breakpoints as a whole, so every change resends its file's set.
 * The platform registers each breakpoint as the session starts, before the
 * agent can take one; they are held until the agent's `initialized` event
 * ([sendAll]) and sent as they change from then on.
 */
class NewspeakLineBreakpointHandler(private val session: XDebugSession) :
    XBreakpointHandler<NewspeakLineBreakpoint>(NewspeakLineBreakpointType::class.java) {
    private val lock = Any()
    private val byFile = LinkedHashMap<String, MutableList<NewspeakLineBreakpoint>>()
    private var client: DapClient? = null

    /** Each breakpoint the agent holds, by the id it gave it, for a stop's `hitBreakpointIds`. */
    private val byId = ConcurrentHashMap<Int, NewspeakLineBreakpoint>()
    private val idsByFile = ConcurrentHashMap<String, List<Int>>()

    override fun registerBreakpoint(breakpoint: NewspeakLineBreakpoint) {
        synchronized(lock) {
            byFile.getOrPut(breakpoint.fileUrl) { mutableListOf() }.add(breakpoint)
            send(breakpoint.fileUrl)
        }
    }

    override fun unregisterBreakpoint(breakpoint: NewspeakLineBreakpoint, temporary: Boolean) {
        synchronized(lock) {
            byFile[breakpoint.fileUrl]?.remove(breakpoint)
            send(breakpoint.fileUrl)
        }
    }

    /** Sends every file's breakpoints to [client], and every later change: the answers' futures. */
    fun sendAll(client: DapClient): List<CompletableFuture<*>> = synchronized(lock) {
        this.client = client
        byFile.keys.toList().mapNotNull(::send)
    }

    /** The breakpoint the agent's [id] names, if it is still one of this session's. */
    fun breakpointOf(id: Int): XBreakpoint<*>? = byId[id]

    /** A `breakpoint` event: the agent resolved or moved one of them unasked. */
    fun changed(event: DapBreakpointEvent) {
        val id = event.breakpoint.id ?: return
        val breakpoint = byId[id] ?: return
        present(breakpoint, event.breakpoint)
    }

    private fun send(fileUrl: String): CompletableFuture<*>? {
        val client = client ?: return null
        // Only a file on disk is a program's module by its path (NewspeakLineBreakpointType.canPutAt).
        if (!fileUrl.startsWith("file:")) return null
        val breakpoints = byFile[fileUrl].orEmpty().filter(XBreakpoint<*>::isEnabled)
        val arguments = JsonObject().apply {
            add("source", JsonObject().apply { addProperty("path", Path.of(VfsUtilCore.urlToPath(fileUrl)).toString()) })
            add(
                "breakpoints",
                JsonArray().apply {
                    for (breakpoint in breakpoints) add(JsonObject().apply { addProperty("line", breakpoint.line + 1) })
                },
            )
        }
        // Answers come in the order of the requests, so the last answer for a file is its latest set.
        return client.request("setBreakpoints", arguments).thenAccept { body ->
            val answers = body.get("breakpoints")
                ?.takeIf { element -> element.isJsonArray }
                ?.asJsonArray
                ?.filter { element -> element.isJsonObject }
                ?.map { element -> DapBreakpoint.from(element.asJsonObject) }
                .orEmpty()
            idsByFile.remove(fileUrl)?.forEach(byId::remove)
            val ids = mutableListOf<Int>()
            for ((index, breakpoint) in breakpoints.withIndex()) {
                val answer = answers.getOrNull(index) ?: continue
                answer.id?.let { id ->
                    byId[id] = breakpoint
                    ids.add(id)
                }
                present(breakpoint, answer)
            }
            idsByFile[fileUrl] = ids
        }
    }

    private fun present(breakpoint: NewspeakLineBreakpoint, answer: DapBreakpoint) {
        if (answer.verified) {
            session.setBreakpointVerified(breakpoint)
        } else {
            session.setBreakpointInvalid(breakpoint, answer.message)
        }
    }
}
