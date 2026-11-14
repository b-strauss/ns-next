package io.github.bstrauss.newspeak.debugger.dap

import com.google.gson.JsonElement
import com.google.gson.JsonObject
import com.intellij.openapi.diagnostic.Logger
import java.io.IOException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * The plugin's side of a DAP session with `ns-next`'s agent: requests
 * numbered and matched to their responses, events handed to the listeners
 * of their name.
 *
 * Threads: it starts reading [connection] at once, and every response's
 * future completes, and every event listener runs, on the connection's
 * reader thread, one message at a time in the order the agent sent them.
 * A listener or a future's continuation must therefore not block on
 * another response -- it would wait for the thread that is to deliver it
 * -- and hops to whichever IntelliJ thread its work needs. A listener is
 * registered before the request that leads to its event, since an event
 * nobody listens for is dropped.
 */
class DapClient(private val connection: DapConnection) {
    private class PendingRequest(val command: String, val future: CompletableFuture<JsonObject>)

    private val nextSeq = AtomicInteger(1)
    private val pending = ConcurrentHashMap<Int, PendingRequest>()
    private val eventListeners = ConcurrentHashMap<String, CopyOnWriteArrayList<(JsonObject) -> Unit>>()
    private val closeListeners = CopyOnWriteArrayList<() -> Unit>()
    private val closed = AtomicBoolean()
    private val closeListenersCalled = AtomicBoolean()

    init {
        connection.start(::dispatch, ::ended)
    }

    /**
     * Sends the request [command] with [arguments]. The future completes
     * with the response's body (an empty object when it has none), fails
     * with a [DapRequestFailedException] when the agent answers that the
     * request failed, and with a [DapConnectionClosedException] when the
     * connection closes, or has closed, before any answer.
     */
    fun request(command: String, arguments: JsonObject = JsonObject()): CompletableFuture<JsonObject> {
        val future = CompletableFuture<JsonObject>()
        val seq = nextSeq.getAndIncrement()
        pending[seq] = PendingRequest(command, future)
        // A close between the check and the registration would otherwise leave the future pending forever.
        if (closed.get()) {
            pending.remove(seq)
            future.completeExceptionally(DapConnectionClosedException(command, null))
            return future
        }
        val message = JsonObject().apply {
            addProperty("seq", seq)
            addProperty("type", "request")
            addProperty("command", command)
            add("arguments", arguments)
        }
        try {
            connection.send(message)
        } catch (exception: IOException) {
            pending.remove(seq)
            future.completeExceptionally(DapConnectionClosedException(command, exception))
        }
        return future
    }

    /** Calls [listener] with the body of every event named [event] (an empty object when it has none). */
    fun onEvent(event: String, listener: (JsonObject) -> Unit) {
        eventListeners.computeIfAbsent(event) { CopyOnWriteArrayList() }.add(listener)
    }

    /**
     * Calls [listener] once when the connection closes, whichever side
     * closed it: on the reader thread when the agent ended it, on the
     * thread that called [close] otherwise, and at once when it has
     * already closed.
     */
    fun onClose(listener: () -> Unit) {
        closeListeners.add(listener)
        if (closeListenersCalled.get() && closeListeners.remove(listener)) listener()
    }

    /** Whether the connection has closed, by either side: a request now fails at once. */
    val isClosed: Boolean get() = closed.get()

    /** Closes the connection and fails every request still unanswered. */
    fun close() {
        connection.close()
        ended(null)
    }

    private fun dispatch(message: JsonObject) {
        when (message.stringOrNull("type")) {
            "response" -> answer(message)
            "event" -> deliver(message)
            "request" -> refuse(message)
            else -> LOG.warn("a DAP message of no known type: $message")
        }
    }

    private fun answer(response: JsonObject) {
        val requestSeq = response.get("request_seq")?.takeIf { element -> element.isJsonPrimitive }?.asInt
        val request = requestSeq?.let(pending::remove)
        if (request == null) {
            LOG.warn("a DAP response to no request awaited: $response")
            return
        }
        if (response.get("success")?.takeIf { element -> element.isJsonPrimitive }?.asBoolean == true) {
            request.future.complete(response.objectOrEmpty("body"))
        } else {
            request.future.completeExceptionally(DapRequestFailedException(request.command, failureMessage(request.command, response)))
        }
    }

    private fun deliver(event: JsonObject) {
        val name = event.stringOrNull("event") ?: return
        val body = event.objectOrEmpty("body")
        for (listener in eventListeners[name].orEmpty()) {
            // One faulty listener must not end the session for the others.
            try {
                listener(body)
            } catch (exception: RuntimeException) {
                LOG.warn("a listener of the DAP event `$name` failed", exception)
            }
        }
    }

    /** The agent sends no reverse request; one is answered as unsupported rather than left unanswered. */
    private fun refuse(request: JsonObject) {
        val command = request.stringOrNull("command") ?: return
        val response = JsonObject().apply {
            addProperty("seq", nextSeq.getAndIncrement())
            addProperty("type", "response")
            add("request_seq", request.get("seq"))
            addProperty("success", false)
            addProperty("command", command)
            addProperty("message", "the Newspeak plugin does not support the request `$command`")
        }
        try {
            connection.send(response)
        } catch (exception: IOException) {
            // The reader will see the connection end.
        }
    }

    private fun ended(cause: Throwable?) {
        if (cause != null) LOG.warn("the DAP connection to ns-next ended", cause)
        if (!closed.compareAndSet(false, true)) return
        connection.close()
        for (seq in pending.keys.toList()) {
            pending.remove(seq)?.let { request -> request.future.completeExceptionally(DapConnectionClosedException(request.command, cause)) }
        }
        closeListenersCalled.set(true)
        for (listener in closeListeners.toList()) {
            if (!closeListeners.remove(listener)) continue
            // As with the events: one faulty listener must not keep the others from learning of the close.
            try {
                listener()
            } catch (exception: RuntimeException) {
                LOG.warn("a listener of the DAP connection's close failed", exception)
            }
        }
    }

    private fun failureMessage(command: String, response: JsonObject): String =
        response.stringOrNull("message")
            ?: response.objectOrEmpty("body").get("error")?.takeIf { element -> element.isJsonObject }?.asJsonObject?.stringOrNull("format")
            ?: "the request `$command` failed"

    private companion object {
        val LOG = Logger.getInstance(DapClient::class.java)
    }
}

/** The agent answered the request [command] with a failure, which [message] gives. */
class DapRequestFailedException(val command: String, message: String) : RuntimeException(message)

/** The connection closed before the request [command] was answered. */
class DapConnectionClosedException(val command: String, cause: Throwable?) :
    RuntimeException("the debugger's connection closed before the request `$command` was answered", cause)

internal fun JsonObject.stringOrNull(name: String): String? =
    get(name)?.takeIf { element -> element.isJsonPrimitive }?.asString

internal fun JsonObject.objectOrEmpty(name: String): JsonObject =
    get(name)?.takeIf(JsonElement::isJsonObject)?.asJsonObject ?: JsonObject()
