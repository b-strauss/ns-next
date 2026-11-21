package io.github.bstrauss.newspeak.devtool.model

import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.util.concurrency.AppExecutorUtil
import io.github.bstrauss.newspeak.NewspeakBundle
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.channels.ReadableByteChannel
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit

/** The longest a record read waits before it is published, so that the tool window follows a run live. */
internal const val PUBLICATION_MILLISECONDS = 200L

/** The most records one batch holds, so that the EDT is never held long by one. */
internal const val BATCH_RECORDS = 5_000

/** How long the reader waits for the file to grow before it looks again, right after it last grew. */
internal const val POLL_MILLISECONDS = 50L

/**
 * The longest the reader waits before it looks again: while nothing is
 * appended each wait doubles up to it, so that a run that has gone quiet
 * costs a look twice a second, and the first bytes appended bring it back
 * to [POLL_MILLISECONDS].
 */
internal const val LONGEST_POLL_MILLISECONDS = 500L

private const val READ_BYTES = 64 * 1024

/** The format's version this reader reads. */
private const val VERSION = 1

/**
 * Follows the trace file at [path] while `ns-next run --trace-file` appends
 * to it (`docs/design/developer-tool.md`, decision 25), on a pooled thread:
 * each complete line parsed into its [TraceRecord] -- a listing or a plan
 * into its entry, which names the line's place in the file
 * ([TraceRecord.indexed]) --, a line not yet ended
 * kept until its end arrives, and the records handed to [onBatch] on the
 * EDT in the file's order, a batch at most every
 * [PUBLICATION_MILLISECONDS] or as soon as it holds [BATCH_RECORDS]. The
 * next batch is handed over only once the last one was applied, so that a
 * backlog -- a file written faster than the EDT takes it -- waits in the
 * file, never in the EDT's queue. A file that is no trace, a version this
 * reader does not know, a line that is no record and a file that can no
 * longer be read are reported once to
 * [onError], on the EDT, after the records read before them, and end the
 * reading. The file need not exist yet: the reader waits for the runtime
 * to create it, looking less often the longer nothing comes, from
 * [POLL_MILLISECONDS] up to [LONGEST_POLL_MILLISECONDS].
 *
 * Reads until it is disposed, or, once [finish] is called, to the file's
 * end, and then tells [onEnd], on the EDT, after the last batch.
 *
 * [dispatch] runs a task on the EDT, [pause] waits, [clock] answers the
 * time in milliseconds and [openChannel] opens the file; tests hand in
 * their own.
 */
class TraceFileReader internal constructor(
    private val path: Path,
    private val onBatch: (List<TraceRecord>) -> Unit,
    private val onError: (String) -> Unit,
    private val onEnd: () -> Unit,
    private val dispatch: (Runnable) -> Unit,
    private val pause: (Long) -> Unit,
    private val clock: () -> Long,
    private val openChannel: (Path) -> ReadableByteChannel = { file -> FileChannel.open(file, StandardOpenOption.READ) },
) : Disposable {
    constructor(path: Path, onBatch: (List<TraceRecord>) -> Unit, onError: (String) -> Unit, onEnd: () -> Unit = {}) : this(
        path,
        onBatch,
        onError,
        onEnd,
        { task -> ApplicationManager.getApplication().invokeLater(task) },
        { milliseconds -> Thread.sleep(milliseconds) },
        System::currentTimeMillis,
    )

    @Volatile private var disposed = false

    @Volatile private var finished = false

    /** Held from a batch's hand-over until the EDT applied it: what makes the reader wait for the EDT. */
    private val applying = Semaphore(1)

    /** How long the next look waits: [POLL_MILLISECONDS] right after bytes arrived, doubling up to [LONGEST_POLL_MILLISECONDS]. */
    private var wait = POLL_MILLISECONDS

    init {
        AppExecutorUtil.getAppExecutorService().execute { read() }
    }

    /** The process has ended: read what the file holds to its end once more, then stop. */
    fun finish() {
        finished = true
    }

    override fun dispose() {
        disposed = true
    }

    private fun read() {
        val lines = LineReader()
        try {
            val channel = open() ?: return
            channel.use {
                val buffer = ByteBuffer.allocate(READ_BYTES)
                var lastPublication = clock()
                while (!disposed) {
                    // Read before looking at the end, so that what was written before `finish` is read.
                    val ending = finished
                    buffer.clear()
                    val read = channel.read(buffer)
                    if (read > 0) {
                        wait = POLL_MILLISECONDS
                        buffer.flip()
                        if (!lines.take(buffer)) return
                    } else if (ending) {
                        break
                    } else {
                        // Records read wait no longer than their publication is due, however long the file stays quiet.
                        val due = (PUBLICATION_MILLISECONDS - (clock() - lastPublication)).coerceAtLeast(1)
                        waitForBytes(if (lines.pending.isEmpty()) wait else wait.coerceAtMost(due))
                    }
                    val now = clock()
                    if (lines.pending.isNotEmpty() && now - lastPublication >= PUBLICATION_MILLISECONDS) {
                        publish(lines.drain())
                        lastPublication = now
                    }
                }
                if (lines.pending.isNotEmpty()) publish(lines.drain())
                handOver(onEnd)
            }
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        } catch (exception: IOException) {
            // The records read before the error are the run's as much as a malformed line's are.
            if (lines.pending.isNotEmpty()) publish(lines.drain())
            report(NewspeakBundle.message("devtool.file.unreadable", path.toString(), exception.message ?: exception.javaClass.simpleName))
        }
    }

    /** The file, once the runtime has created it; null if the reader was disposed or finished before. */
    private fun open(): ReadableByteChannel? {
        while (!disposed) {
            // Looked at before trying, so that a file created just before `finish` is still read.
            val ending = finished
            if (Files.exists(path)) return openChannel(path)
            if (ending) return null
            waitForBytes(wait)
        }
        return null
    }

    /** Waits [milliseconds] for the file to change, and makes the next wait twice as long, up to [LONGEST_POLL_MILLISECONDS]. */
    private fun waitForBytes(milliseconds: Long) {
        pause(milliseconds)
        wait = (wait * 2).coerceAtMost(LONGEST_POLL_MILLISECONDS)
    }

    /** Hands [batch] to the EDT once the last batch was applied; nothing once the reader is disposed. */
    private fun publish(batch: List<TraceRecord>) = handOver { onBatch(batch) }

    /** Runs [task] on the EDT once what was handed over before it was applied; nothing once the reader is disposed. */
    private fun handOver(task: () -> Unit) {
        while (!applying.tryAcquire(POLL_MILLISECONDS, TimeUnit.MILLISECONDS)) {
            if (disposed) return
        }
        dispatch {
            try {
                if (!disposed) task()
            } finally {
                applying.release()
            }
        }
    }

    /** Reports [error] on the EDT, after every batch handed over before it. */
    private fun report(error: String) {
        dispatch { if (!disposed) onError(error) }
    }

    /** Splits the bytes read at `\n` into records, and publishes a batch whenever one is full. */
    private inner class LineReader {
        /** The bytes of a line whose `\n` has not arrived yet. */
        private val tail = ByteArrayOutputStream()

        /** The file's offset of the first byte [take] is given next: every byte taken before, counted. */
        private var taken = 0L

        /** The file's offset of the first byte of the line not ended yet, by which its record is read again ([RecordPlace]). */
        private var lineStart = 0L

        private var lineNumber = 0

        var pending = ArrayList<TraceRecord>()
            private set

        fun drain(): List<TraceRecord> = pending.also { pending = ArrayList() }

        /** Takes [bytes]; false once the file proved no trace this reader reads, the error reported. */
        fun take(bytes: ByteBuffer): Boolean {
            val array = bytes.array()
            var start = bytes.arrayOffset() + bytes.position()
            val end = bytes.arrayOffset() + bytes.limit()
            val first = start
            val takenBefore = taken
            taken += end - start
            for (index in start until end) {
                if (array[index] != NEWLINE) continue
                // Bytes, not characters: the place is what a later read seeks to.
                val newlineAt = takenBefore + (index - first)
                val place = RecordPlace(lineStart, Math.toIntExact(newlineAt - lineStart))
                lineStart = newlineAt + 1
                // A newline byte is never part of a multi-byte character, so a line decodes whole.
                val line = if (tail.size() == 0) {
                    String(array, start, index - start, StandardCharsets.UTF_8)
                } else {
                    tail.write(array, start, index - start)
                    tail.toString(StandardCharsets.UTF_8).also { tail.reset() }
                }
                start = index + 1
                if (!line(line, place)) return false
                if (pending.size >= BATCH_RECORDS) publish(drain())
            }
            tail.write(array, start, end - start)
            return true
        }

        private fun line(line: String, place: RecordPlace): Boolean {
            lineNumber++
            val record = try {
                TraceRecord.indexed(line, place)
            } catch (exception: MalformedRecordException) {
                fail(
                    if (lineNumber == 1) NewspeakBundle.message("devtool.file.format")
                    else NewspeakBundle.message("devtool.file.malformed", lineNumber.toString(), exception.message ?: ""),
                )
                return false
            }
            if (lineNumber == 1) {
                if (record !is Header) {
                    fail(NewspeakBundle.message("devtool.file.format"))
                    return false
                }
                if (record.version != VERSION) {
                    fail(NewspeakBundle.message("devtool.file.version", record.version))
                    return false
                }
            }
            if (record != null) pending.add(record)
            return true
        }

        /** Ends the reading with [error]: the records read before it are published first, so that the run keeps everything the file held up to it. */
        private fun fail(error: String) {
            if (pending.isNotEmpty()) publish(drain())
            report(error)
        }
    }

    private companion object {
        const val NEWLINE = '\n'.code.toByte()
    }
}
