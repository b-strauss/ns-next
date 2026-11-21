package io.github.bstrauss.newspeak.devtool.model

import io.github.bstrauss.newspeak.NewspeakBundle
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.charset.StandardCharsets
import java.nio.file.NoSuchFileException
import java.nio.file.Path
import java.nio.file.StandardOpenOption

/** A record a view wanted that its trace file no longer gives, [message] saying why, in the user's words. */
class UnreadableRecordException(message: String) : Exception(message)

/**
 * The record at [place] in the trace file [file], parsed by the reader's
 * own parsing ([TraceRecord.parse]): what a view reads when it shows a
 * listing or a plan the run keeps only the entry of (decision 34). Reads
 * the file and nothing else, so it runs on any thread; a view calls it off
 * the EDT, since one listing's line can be a few MiB.
 *
 * @throws UnreadableRecordException where the file is gone or cannot be
 *   read, ends before the record does, or holds no record there.
 */
fun readRecord(file: Path, place: RecordPlace): TraceRecord {
    val bytes = ByteBuffer.allocate(place.length)
    try {
        FileChannel.open(file, StandardOpenOption.READ).use { channel ->
            while (bytes.hasRemaining()) {
                // A file truncated or replaced since the record was read ends before it.
                if (channel.read(bytes, place.offset + bytes.position()) < 0) {
                    throw UnreadableRecordException(NewspeakBundle.message("devtool.record.unreadable", file.toString(), NewspeakBundle.message("devtool.record.short")))
                }
            }
        }
    } catch (exception: NoSuchFileException) {
        // Its message is the path alone, which the sentence names already.
        throw UnreadableRecordException(NewspeakBundle.message("devtool.record.unreadable", file.toString(), NewspeakBundle.message("devtool.record.gone")))
    } catch (exception: IOException) {
        throw UnreadableRecordException(
            NewspeakBundle.message("devtool.record.unreadable", file.toString(), exception.message ?: exception.javaClass.simpleName),
        )
    }
    val line = String(bytes.array(), StandardCharsets.UTF_8)
    return try {
        TraceRecord.parse(line)
    } catch (exception: MalformedRecordException) {
        throw UnreadableRecordException(NewspeakBundle.message("devtool.record.unreadable", file.toString(), exception.message ?: ""))
    } ?: throw UnreadableRecordException(NewspeakBundle.message("devtool.record.unreadable", file.toString(), NewspeakBundle.message("devtool.record.kind")))
}
