package io.github.bstrauss.newspeak.devtool.ui

import com.google.gson.JsonParser
import io.github.bstrauss.newspeak.devtool.editor.ListingNames
import io.github.bstrauss.newspeak.devtool.editor.ListingPosition
import io.github.bstrauss.newspeak.devtool.editor.bytecodeListing
import io.github.bstrauss.newspeak.devtool.editor.codeListing
import io.github.bstrauss.newspeak.devtool.model.Bytecode
import io.github.bstrauss.newspeak.devtool.model.Code
import io.github.bstrauss.newspeak.devtool.model.TraceRecord
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/**
 * The plugin's listings held to the CLI's: each listing of the fixture
 * `crates/ns-cli/src/mcp/trace_listings.rs`'s `the_listings_fixture_is_current`
 * keeps current -- its record as the trace file writes it and the text
 * `--print-*` writes of it -- written from the record as the Code tab and a
 * listing tab write it, a baseline copy's positions `passed` at one
 * instruction among them.
 */
class ListingsFixtureTest {
    @Test fun testEveryListingIsWrittenAsTheCliWritesIt() {
        val entries = checkNotNull(javaClass.getResource("/devtool/listings.jsonl")).readText().lines().filter { line -> line.isNotBlank() }
        var passed = 0
        for (entry in entries) {
            val fields = JsonParser.parseString(entry).asJsonObject
            val run = TraceRun("Demo", Instant.EPOCH)
            val bodies = fields.getAsJsonArray("bodies").map { element ->
                val body = element.asJsonObject
                """{"kind":"body","body":${body.get("body").asInt},"name":${body.get("name")},"module":{"id":"Demo","version":null},"range":null,"checksum":"0000000000000000","statements":[],"sends":[]}"""
            }
            run.add(bodies.map { line -> checkNotNull(TraceRecord.parse(line)) })
            run.add(fields.getAsJsonArray("bytecode").map { record -> checkNotNull(TraceRecord.parse(record.toString())) })
            val record = fields.getAsJsonObject("record")
            val listing = when (val parsed = TraceRecord.parse(record.toString())) {
                is Bytecode -> bytecodeListing(run, parsed)
                is Code -> {
                    passed += parsed.instructions.sumOf { instruction -> instruction.passed.size }
                    codeListing(ListingNames.of(run), parsed)
                }
                else -> error("no listing: $record")
            }
            assertEquals(entry, fields.get("text").asString, listing.text)
            if (record.get("kind").asString == "code" && record.get("tier").asString == "baseline") {
                // Each position line, the passed ones too, links its bytecode instruction.
                val linked = listing.text.lines().zip(listing.positions)
                    .filter { (line, _) -> line.startsWith("  @") }
                    .map { (_, position) -> position }
                assertEquals(listOf(ListingPosition(4, 0), ListingPosition(4, 1), ListingPosition(4, 2)), linked)
            }
        }
        assertTrue("no position passed at an instruction", passed > 0)
    }
}
