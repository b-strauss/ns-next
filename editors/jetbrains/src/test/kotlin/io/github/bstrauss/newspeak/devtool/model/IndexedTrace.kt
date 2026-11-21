package io.github.bstrauss.newspeak.devtool.model

import io.github.bstrauss.newspeak.devtool.TraceFilters
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption
import java.time.Instant

/**
 * A trace file a test writes line by line, as the runtime appends to one,
 * and the records its lines hold as the reader hands them to a run --
 * each listing and plan as its entry, placed where its line stands in the
 * file ([TraceRecord.indexed]) --, so that a run built from them reads its
 * listings back from [file].
 */
class IndexedTrace(val file: Path = Files.createTempFile("trace-", ".jsonl").also { created -> created.toFile().deleteOnExit() }) {
    /** Appends [lines] to the file and answers their records. */
    fun records(lines: List<String>): List<TraceRecord> {
        var offset = Files.size(file)
        val records = lines.map { line ->
            val bytes = line.toByteArray(StandardCharsets.UTF_8)
            val record = checkNotNull(TraceRecord.indexed(line, RecordPlace(offset, bytes.size))) { line }
            offset += bytes.size + 1
            record
        }
        Files.writeString(file, lines.joinToString("") { line -> line + "\n" }, StandardOpenOption.APPEND)
        return records
    }

    /** Appends [lines] to the file and answers their records. */
    fun records(vararg lines: String): List<TraceRecord> = records(lines.toList())

    /** A run of the file, its records still to be added. */
    fun run(name: String = "Demo", filters: TraceFilters = TraceFilters.EVERYTHING): TraceRun = TraceRun(name, Instant.EPOCH, filters, file)
}
