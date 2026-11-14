package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.google.gson.JsonParser
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.debugger.dap.DapConnectionClosedException
import io.github.bstrauss.newspeak.debugger.dap.DapRequestFailedException
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * The framing and the client's bookkeeping, over piped streams with a
 * scripted peer in place of the agent: no `ns-next` runs here.
 */
class DapConnectionTest {
    private val opened = mutableListOf<AutoCloseable>()

    @After
    fun closeEverything() {
        for (resource in opened.asReversed()) resource.close()
    }

    /** Two connections joined by pipes: what one sends, the other reads. */
    private fun joinedPair(): Pair<DapConnection, DapConnection> {
        val towardsSecond = PipedOutputStream()
        val towardsFirst = PipedOutputStream()
        val first = DapConnection(PipedInputStream(towardsFirst, PIPE_SIZE), towardsSecond)
        val second = DapConnection(PipedInputStream(towardsSecond, PIPE_SIZE), towardsFirst)
        opened.add(first)
        opened.add(second)
        return first to second
    }

    @Test
    fun framesRoundTrip() {
        val message = JsonParser.parseString(
            """{"seq":2,"type":"event","event":"output","body":{"output":"Grüße, 𝄞\r\n"}}""",
        ).asJsonObject

        val bytes = ByteArrayOutputStream()
        DapConnection(ByteArrayInputStream(ByteArray(0)), bytes).send(message)
        val written = bytes.toByteArray()
        val separator = "\r\n\r\n".toByteArray(Charsets.US_ASCII)
        val headerEnd = indexOf(written, separator)
        assertTrue("a header ends the frame's start", headerEnd > 0)
        val header = String(written, 0, headerEnd, Charsets.US_ASCII)
        val body = written.copyOfRange(headerEnd + separator.size, written.size)
        val text = String(body, Charsets.UTF_8)
        assertTrue("the body holds more bytes than characters", body.size > text.length)
        assertEquals("Content-Length: ${body.size}", header)
        assertEquals(message, JsonParser.parseString(text))

        val (sender, receiver) = joinedPair()
        val received = CompletableFuture<JsonObject>()
        receiver.start({ arrived -> received.complete(arrived) }, { _ -> })
        sender.send(message)
        assertEquals(message, received.get(WAIT_SECONDS, TimeUnit.SECONDS))
    }

    /** A peer that answers every request `respond` says, as the agent would. */
    private fun clientAgainst(respond: (JsonObject) -> JsonObject?): DapClient {
        val (clientSide, agentSide) = joinedPair()
        val responses = AtomicInteger(1)
        agentSide.start(
            { request ->
                val answer = respond(request) ?: return@start
                answer.addProperty("seq", responses.getAndIncrement())
                answer.addProperty("type", "response")
                answer.addProperty("request_seq", request.get("seq").asInt)
                answer.addProperty("command", request.get("command").asString)
                agentSide.send(answer)
            },
            { _ -> },
        )
        return DapClient(clientSide).also { client -> opened.add(AutoCloseable(client::close)) }
    }

    @Test
    fun aFailedResponseFailsItsFuture() {
        val client = clientAgainst { request ->
            JsonObject().apply {
                if (request.get("command").asString == "continue") {
                    addProperty("success", false)
                    addProperty("message", "the program is not stopped")
                } else {
                    addProperty("success", true)
                    add("body", JsonParser.parseString("""{"threads":[{"id":1,"name":"main"}]}"""))
                }
            }
        }

        val threads = client.request("threads").get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertEquals(1, threads.getAsJsonArray("threads").size())

        val failure = failureOf(client.request("continue", JsonObject().apply { addProperty("threadId", 1) }))
        if (failure !is DapRequestFailedException) throw AssertionError("not a failed request", failure)
        assertEquals("continue", failure.command)
        assertEquals("the program is not stopped", failure.message)
    }

    @Test
    fun anEventReachesItsListenersWithItsBody() {
        val (clientSide, agentSide) = joinedPair()
        val client = DapClient(clientSide).also { client -> opened.add(AutoCloseable(client::close)) }
        val stopped = CompletableFuture<JsonObject>()
        val exited = CompletableFuture<JsonObject>()
        client.onEvent("stopped") { body -> stopped.complete(body) }
        client.onEvent("exited") { body -> exited.complete(body) }
        agentSide.send(
            JsonParser.parseString(
                """{"seq":1,"type":"event","event":"stopped","body":{"reason":"breakpoint","threadId":1}}""",
            ).asJsonObject,
        )
        assertEquals("breakpoint", stopped.get(WAIT_SECONDS, TimeUnit.SECONDS).get("reason").asString)
        assertTrue("an event other than its own reaches no listener", !exited.isDone)
    }

    @Test
    fun closeFailsEveryPendingFutureAndTellsItsListeners() {
        val client = clientAgainst { _ -> null }
        val closings = AtomicInteger()
        client.onClose { closings.incrementAndGet() }
        val pending = client.request("threads")
        client.close()
        client.close()
        assertTrue(failureOf(pending) is DapConnectionClosedException)
        assertEquals(1, closings.get())
        assertTrue(failureOf(client.request("threads")) is DapConnectionClosedException)
        val late = AtomicInteger()
        client.onClose { late.incrementAndGet() }
        assertEquals("a listener added after the close is told at once", 1, late.get())
    }

    @Test
    fun theEndOfTheStreamClosesTheClient() {
        val (clientSide, agentSide) = joinedPair()
        val client = DapClient(clientSide).also { client -> opened.add(AutoCloseable(client::close)) }
        val closed = CompletableFuture<Unit>()
        client.onClose { closed.complete(Unit) }
        val pending = client.request("threads")
        agentSide.close()
        closed.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(failureOf(pending) is DapConnectionClosedException)
    }

    private fun failureOf(future: CompletableFuture<JsonObject>): Throwable {
        try {
            future.get(WAIT_SECONDS, TimeUnit.SECONDS)
        } catch (exception: ExecutionException) {
            return exception.cause ?: exception
        }
        fail("the future completed normally")
        throw IllegalStateException()
    }

    private fun indexOf(bytes: ByteArray, wanted: ByteArray): Int =
        (0..bytes.size - wanted.size).firstOrNull { start ->
            wanted.indices.all { offset -> bytes[start + offset] == wanted[offset] }
        } ?: -1

    private companion object {
        const val WAIT_SECONDS = 10L
        const val PIPE_SIZE = 64 * 1024
    }
}
