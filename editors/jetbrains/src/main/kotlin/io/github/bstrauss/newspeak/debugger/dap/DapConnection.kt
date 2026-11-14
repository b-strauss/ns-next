package io.github.bstrauss.newspeak.debugger.dap

import com.google.gson.JsonObject
import com.google.gson.JsonParseException
import com.google.gson.JsonParser
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.ServerSocket
import java.util.concurrent.atomic.AtomicBoolean

/**
 * One Debug Adapter Protocol connection: DAP's framing -- each message a
 * JSON object after a `Content-Length` header, counted in UTF-8 bytes, and
 * a blank line -- over a pair of streams, the messages read by a thread of
 * the connection's own.
 *
 * [send] may be called from any thread; frames are never interleaved.
 * Nothing reads before [start], so whoever starts it decides the thread
 * that every message is handed to: the reader's.
 */
class DapConnection(
    input: InputStream,
    private val output: OutputStream,
    private val resource: Closeable? = null,
) : Closeable {
    private val input = BufferedInputStream(input)
    private val writeLock = Any()
    private val started = AtomicBoolean()
    private val closed = AtomicBoolean()

    /** Writes [message] as one frame and flushes it. */
    @Throws(IOException::class)
    fun send(message: JsonObject) {
        val body = message.toString().toByteArray(Charsets.UTF_8)
        val frame = ByteArrayOutputStream(body.size + HEADER_ROOM)
        frame.write("$CONTENT_LENGTH ${body.size}\r\n\r\n".toByteArray(Charsets.US_ASCII))
        frame.write(body)
        // One write, so that a frame is never split between two senders.
        synchronized(writeLock) {
            frame.writeTo(output)
            output.flush()
        }
    }

    /**
     * Starts the reader thread, once: it hands every message to
     * [onMessage], in order, and calls [onEnd] once when the stream ends --
     * with null at a clean end between frames, otherwise with what went
     * wrong, a malformed frame among it, after which nothing more is read.
     * Anything [onMessage] throws, an `Error` included, ends the connection
     * the same way.
     */
    fun start(onMessage: (JsonObject) -> Unit, onEnd: (Throwable?) -> Unit) {
        check(started.compareAndSet(false, true)) { "the connection is already read" }
        val reader = Thread({
            var failure: Throwable? = null
            try {
                while (true) {
                    val message = readMessage() ?: break
                    onMessage(message)
                }
            } catch (throwable: Throwable) {
                // An `Error` too -- a listener's failed assertion, say -- or
                // the client would never learn that nothing reads any more,
                // and its requests would wait for ever.
                failure = throwable
            } finally {
                onEnd(if (closed.get()) null else failure)
            }
        }, "Newspeak DAP reader")
        reader.isDaemon = true
        reader.start()
    }

    /**
     * Closes both streams and the resource they belong to, once. A reader
     * blocked on a socket ends at once; one blocked on another kind of
     * stream may end only when the peer closes its end.
     */
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        for (closeable in listOfNotNull(resource, input, output)) {
            try {
                closeable.close()
            } catch (exception: IOException) {
                // Closing is all that is left to do with it.
            }
        }
    }

    /**
     * The next message, or null at the end of the stream before a frame
     * begins. Every frame is bounded before it is read, since the peer
     * could otherwise make the IDE allocate whatever it names.
     */
    private fun readMessage(): JsonObject? {
        var length: Int? = null
        var lines = 0
        while (true) {
            val line = readHeaderLine() ?: if (lines == 0) return null else throw DapProtocolException("the stream ended inside a message")
            lines += 1
            if (lines > MAXIMUM_HEADER_LINES) throw DapProtocolException("a message's header is too long")
            if (line.isEmpty()) break
            // Header names are case-insensitive, as HTTP's are.
            if (line.regionMatches(0, CONTENT_LENGTH, 0, CONTENT_LENGTH.length, ignoreCase = true)) {
                length = line.substring(CONTENT_LENGTH.length).trim().toIntOrNull()
                    ?: throw DapProtocolException("a message's Content-Length is no number: $line")
            }
        }
        val bodyLength = length ?: throw DapProtocolException("a message's header has no Content-Length")
        if (bodyLength < 0 || bodyLength > MAXIMUM_LENGTH) {
            throw DapProtocolException("a message of $bodyLength bytes is longer than the $MAXIMUM_LENGTH the plugin reads")
        }
        val body = input.readNBytes(bodyLength)
        if (body.size < bodyLength) throw DapProtocolException("the stream ended inside a message")
        val parsed = try {
            JsonParser.parseString(String(body, Charsets.UTF_8))
        } catch (exception: JsonParseException) {
            throw DapProtocolException("a message is no JSON: ${exception.message}", exception)
        }
        if (!parsed.isJsonObject) throw DapProtocolException("a message is no JSON object: $parsed")
        return parsed.asJsonObject
    }

    /** One header line without its line ending, or null at the end of the stream before it begins. */
    private fun readHeaderLine(): String? {
        val line = ByteArrayOutputStream()
        while (true) {
            val byte = input.read()
            if (byte < 0) {
                if (line.size() == 0) return null
                throw DapProtocolException("the stream ended inside a message")
            }
            if (byte == '\n'.code) break
            if (line.size() >= MAXIMUM_HEADER_LINE) throw DapProtocolException("a message's header is too long")
            line.write(byte)
        }
        return line.toString(Charsets.US_ASCII).removeSuffix("\r")
    }

    companion object {
        private const val CONTENT_LENGTH = "Content-Length:"

        /** Room for the header in front of a body. */
        private const val HEADER_ROOM = 32

        /** The longest message read: the agent's own bound on what it reads. */
        private const val MAXIMUM_LENGTH = 64 * 1024 * 1024

        /** The longest header line read. */
        private const val MAXIMUM_HEADER_LINE = 1024

        /** The most header lines one message may have; DAP defines two. */
        private const val MAXIMUM_HEADER_LINES = 16

        /**
         * The connection of the first runtime that connects to
         * [serverSocket] within [timeoutMillis] milliseconds, the connecting
         * mode's (`ns-next run --debug-connect`); [serverSocket] keeps the
         * timeout. A [java.net.SocketTimeoutException] when none connects
         * in time.
         */
        @Throws(IOException::class)
        fun accept(serverSocket: ServerSocket, timeoutMillis: Int): DapConnection {
            serverSocket.soTimeout = timeoutMillis
            val socket = serverSocket.accept()
            try {
                // A request is one small frame awaited at once: Nagle's delay would only slow each step.
                socket.tcpNoDelay = true
                return DapConnection(socket.getInputStream(), socket.getOutputStream(), socket)
            } catch (exception: IOException) {
                socket.close()
                throw exception
            }
        }
    }
}

/** A frame or a message the peer sent that is no DAP message. */
class DapProtocolException(message: String, cause: Throwable? = null) : IOException(message, cause)
