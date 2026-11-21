package io.github.bstrauss.newspeak.devtool.ui

import com.google.gson.JsonParser
import io.github.bstrauss.newspeak.devtool.model.Event
import io.github.bstrauss.newspeak.devtool.model.TraceRecord
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/**
 * The Events tab's "Text" view held to the text trace: every event of
 * `crates/ns-cli/src/trace.rs`'s `every_event`, its record as the trace
 * file writes it and the line `ns-next run --trace` writes, from the
 * fixture `trace_file.rs`'s `the_events_fixture_is_current` keeps current.
 */
class EventsFixtureTest {
    @Test fun testEveryEventIsWrittenAsTheTextTraceWritesIt() {
        val entries = checkNotNull(javaClass.getResource("/devtool/events.jsonl")).readText().lines().filter { line -> line.isNotBlank() }
        // `every_event` has an event of each kind, some kinds twice; a kind is its category and its event.
        assertTrue(entries.size.toString(), entries.size >= EVENT_KINDS)
        val kinds = HashSet<String>()
        for (entry in entries) {
            val fields = JsonParser.parseString(entry).asJsonObject
            val bodies = fields.getAsJsonArray("bodies").map { element ->
                val body = element.asJsonObject
                """{"kind":"body","body":${body.get("body").asInt},"name":${body.get("name")},"module":{"id":"Demo","version":null},"range":null,"checksum":"0000000000000000","statements":[],"sends":[]}"""
            }
            val run = TraceRun("Demo", Instant.EPOCH)
            run.add(bodies.map { line -> checkNotNull(TraceRecord.parse(line)) })
            val record = fields.getAsJsonObject("record")
            val event = TraceRecord.parse(record.toString()) as Event
            kinds += "${event.category} ${event.event}"
            assertEquals(entry, fields.get("line").asString, traceLine(run, event))
        }
        assertEquals(kinds.toString(), EVENT_KINDS, kinds.size)
    }

    private companion object {
        /** `trace.rs`'s `EVENT_KINDS`. */
        const val EVENT_KINDS = 28
    }
}
