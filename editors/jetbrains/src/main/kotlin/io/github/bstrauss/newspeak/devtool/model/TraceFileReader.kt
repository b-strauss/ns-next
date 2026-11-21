package io.github.bstrauss.newspeak.devtool.model

import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.util.concurrency.AppExecutorUtil
import io.github.bstrauss.newspeak.NewspeakBundle
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption

/** The longest a record read waits before it is published, so that the tool window follows a run live. */
internal const val PUBLICATION_MILLISECONDS = 200L

/** The most records one batch holds, so that the EDT is never held long by one. */
internal const val BATCH_RECORDS = 5_000

/** How long the reader waits for the file to grow before it looks again. */
private const val POLL_MILLISECONDS = 50L

private const val READ_BYTES = 64 * 1024

/** The format's version this reader reads. */
private const val VERSION = 1

/**
 * Follows the trace file at [path] while `ns-next run --trace-file` appends
 * to it (`docs/design/developer-tool.md`, decision 25), on a pooled thread:
 * each complete line parsed into its [TraceRecord], a line not yet ended
 * kept until its end arrives, and the records handed to [onBatch] on the
 * EDT in the file's order, a batch at most every
 * [PUBLICATION_MILLISECONDS] or as soon as it holds [BATCH_RECORDS]. A file
 * that is no trace, a version this reader does not know and a line that is
 * no record are reported once to [onError], on the EDT, after the records
 * read before them, and end the reading.
 * The file need not exist yet: the reader waits for the runtime to create it.
 *
 * Reads until it is disposed, or, once [finish] is called, to the file's
 * end.
 */
class TraceFileReader(
    private val path: Path,
    private val onBatch: (List<TraceRecord>) -> Unit,
    private val onError: (String) -> Unit,
) : Disposable {
    @Volatile private var disposed = false

    @Volatile private var finished = false

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
                var lastPublication = System.currentTimeMillis()
                while (!disposed) {
                    // Read before looking at the end, so that what was written before `finish` is read.
                    val ending = finished
                    buffer.clear()
                    val read = channel.read(buffer)
                    if (read > 0) {
                        buffer.flip()
                        if (!lines.take(buffer)) return
                    } else if (ending) {
                        break
                    } else {
                        Thread.sleep(POLL_MILLISECONDS)
                    }
                    val now = System.currentTimeMillis()
                    if (lines.pending.isNotEmpty() && now - lastPublication >= PUBLICATION_MILLISECONDS) {
                        publish(lines.drain())
                        lastPublication = now
                    }
                }
                if (lines.pending.isNotEmpty()) publish(lines.drain())
            }
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        } catch (exception: IOException) {
            report(NewspeakBundle.message("devtool.file.unreadable", path.toString(), exception.message ?: exception.javaClass.simpleName))
        }
    }

    /** The file, once the runtime has created it; null if the reader was disposed or finished before. */
    private fun open(): FileChannel? {
        while (!disposed) {
            // Looked at before trying, so that a file created just before `finish` is still read.
            val ending = finished
            if (Files.exists(path)) return FileChannel.open(path, StandardOpenOption.READ)
            if (ending) return null
            Thread.sleep(POLL_MILLISECONDS)
        }
        return null
    }

    private fun publish(batch: List<TraceRecord>) {
        ApplicationManager.getApplication().invokeLater { if (!disposed) onBatch(batch) }
    }

    private fun report(error: String) {
        ApplicationManager.getApplication().invokeLater { if (!disposed) onError(error) }
    }

    /** Splits the bytes read at `\n` into records, and publishes a batch whenever one is full. */
    private inner class LineReader {
        /** The bytes of a line whose `\n` has not arrived yet. */
        private val tail = ByteArrayOutputStream()

        private var lineNumber = 0

        var pending = ArrayList<TraceRecord>()
            private set

        fun drain(): List<TraceRecord> = pending.also { pending = ArrayList() }

        /** Takes [bytes]; false once the file proved no trace this reader reads, the error reported. */
        fun take(bytes: ByteBuffer): Boolean {
            val array = bytes.array()
            var start = bytes.arrayOffset() + bytes.position()
            val end = bytes.arrayOffset() + bytes.limit()
            for (index in start until end) {
                if (array[index] != NEWLINE) continue
                // A newline byte is never part of a multi-byte character, so a line decodes whole.
                val line = if (tail.size() == 0) {
                    String(array, start, index - start, StandardCharsets.UTF_8)
                } else {
                    tail.write(array, start, index - start)
                    tail.toString(StandardCharsets.UTF_8).also { tail.reset() }
                }
                start = index + 1
                if (!line(line)) return false
                if (pending.size >= BATCH_RECORDS) publish(drain())
            }
            tail.write(array, start, end - start)
            return true
        }

        private fun line(line: String): Boolean {
            lineNumber++
            val record = try {
                TraceRecord.parse(line)
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
