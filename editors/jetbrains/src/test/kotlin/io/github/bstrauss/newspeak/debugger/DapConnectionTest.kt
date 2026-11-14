package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.google.gson.JsonParser
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.debugger.dap.DapConnectionClosedException
import io.github.bstrauss.newspeak.debugger.dap.DapProtocolException
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
import java.util.concurrent.CopyOnWriteArrayList
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

    /** A connection reading what the test writes, byte by byte as it chooses, onto `writer`. */
    private class RawPeer(val connection: DapConnection, val writer: PipedOutputStream) {
        fun write(text: String) = write(text.toByteArray(Charsets.UTF_8))

        fun write(bytes: ByteArray) {
            writer.write(bytes)
            writer.flush()
        }
    }

    private fun rawPeer(): RawPeer {
        val writer = PipedOutputStream()
        val connection = DapConnection(PipedInputStream(writer, PIPE_SIZE), ByteArrayOutputStream())
        opened.add(connection)
        opened.add(writer)
        return RawPeer(connection, writer)
    }

    /** What the connection read: its messages, and how it ended once it did. */
    private class Reading {
        val messages = CopyOnWriteArrayList<JsonObject>()
        val ended = CompletableFuture<Throwable?>()
    }

    private fun reading(connection: DapConnection): Reading {
        val reading = Reading()
        connection.start({ message -> reading.messages.add(message) }, { cause -> reading.ended.complete(cause) })
        return reading
    }

    @Test
    fun headerNamesAreReadInAnyCaseAndOtherHeadersIgnored() {
        val peer = rawPeer()
        val reading = reading(peer.connection)
        val body = """{"seq":1,"type":"event","event":"initialized"}"""
        peer.write("content-LENGTH: ${body.length}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n$body")
        peer.write("CONTENT-LENGTH:${body.length}\r\n\r\n$body")
        peer.writer.close()
        assertEquals(null, reading.ended.get(WAIT_SECONDS, TimeUnit.SECONDS))
        assertEquals(2, reading.messages.size)
        assertEquals("initialized", reading.messages[1].get("event").asString)
    }

    @Test
    fun aBodyDeliveredInPiecesIsOneMessage() {
        val peer = rawPeer()
        val reading = reading(peer.connection)
        val body = """{"seq":1,"type":"event","event":"output","body":{"output":"Grüße"}}""".toByteArray(Charsets.UTF_8)
        peer.write("Content-Length: ${body.size}\r")
        peer.write("\n\r\n")
        // Split inside the two bytes of `ü`, so that no piece decodes alone.
        val split = body.indexOfFirst { byte -> byte.toInt() and 0x80 != 0 } + 1
        peer.write(body.copyOfRange(0, split))
        peer.write(body.copyOfRange(split, body.size))
        peer.writer.close()
        assertEquals(null, reading.ended.get(WAIT_SECONDS, TimeUnit.SECONDS))
        assertEquals(1, reading.messages.size)
        assertEquals("Grüße", reading.messages[0].getAsJsonObject("body").get("output").asString)
    }

    @Test
    fun aPeerClosingInsideABodyEndsTheConnectionWithItsCause() {
        val peer = rawPeer()
        val reading = reading(peer.connection)
        peer.write("Content-Length: 100\r\n\r\n{\"seq\":1,")
        peer.writer.close()
        assertTrue(reading.ended.get(WAIT_SECONDS, TimeUnit.SECONDS) is DapProtocolException)
        assertTrue(reading.messages.isEmpty())
    }

    @Test
    fun anOversizedLengthIsRefusedBeforeItIsRead() {
        val peer = rawPeer()
        val reading = reading(peer.connection)
        // Nothing follows the header: a reader that tried to read the body would wait for ever.
        peer.write("Content-Length: ${64 * 1024 * 1024 + 1}\r\n\r\n")
        val cause = reading.ended.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(cause.toString(), cause is DapProtocolException && cause.message!!.contains("longer than"))
    }

    @Test
    fun malformedJsonEndsTheConnection() {
        val peer = rawPeer()
        val reading = reading(peer.connection)
        val body = """{"seq":1,"type":"""
        peer.write("Content-Length: ${body.length}\r\n\r\n$body")
        val cause = reading.ended.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(cause.toString(), cause is DapProtocolException && cause.message!!.contains("no JSON"))
    }

    @Test
    fun aMalformedFrameClosesTheClientAndFailsWhatIsPending() {
        val peer = rawPeer()
        val client = DapClient(peer.connection).also { client -> opened.add(AutoCloseable(client::close)) }
        val closed = CompletableFuture<Unit>()
        client.onClose { closed.complete(Unit) }
        val pending = client.request("threads")
        peer.write("Content-Length: 3\r\n\r\n[1]")
        closed.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(failureOf(pending) is DapConnectionClosedException)
    }

    @Test
    fun anErrorAListenerThrowsStillClosesTheClient() {
        val (clientSide, agentSide) = joinedPair()
        val client = DapClient(clientSide).also { client -> opened.add(AutoCloseable(client::close)) }
        val closed = CompletableFuture<Unit>()
        client.onClose { closed.complete(Unit) }
        client.onEvent("stopped") { _ -> throw AssertionError("a listener's own failure") }
        val pending = client.request("threads")
        agentSide.send(JsonParser.parseString("""{"seq":1,"type":"event","event":"stopped","body":{}}""").asJsonObject)
        closed.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(failureOf(pending) is DapConnectionClosedException)
    }

    @Test
    fun aCloseListenerThatThrowsLeavesTheOthersCalled() {
        val client = clientAgainst { _ -> null }
        val later = AtomicInteger()
        client.onClose { throw IllegalStateException("a close listener's own failure") }
        client.onClose { later.incrementAndGet() }
        client.close()
        assertEquals(1, later.get())
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
