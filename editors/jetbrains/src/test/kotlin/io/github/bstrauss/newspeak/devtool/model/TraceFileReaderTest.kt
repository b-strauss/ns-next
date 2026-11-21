package io.github.bstrauss.newspeak.devtool.model

import com.intellij.openapi.util.Disposer
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.NewspeakBundle
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption

class TraceFileReaderTest : BasePlatformTestCase() {
    /** What a reader published, in order, on the EDT. */
    private class Received {
        val batches = mutableListOf<List<TraceRecord>>()
        val errors = mutableListOf<String>()
        val records get() = batches.flatten()
    }

    private fun traceFile(): Path = Files.createTempFile("trace", ".jsonl").also { file -> file.toFile().deleteOnExit() }

    private fun reader(file: Path, received: Received): TraceFileReader {
        val reader = TraceFileReader(file, { batch -> received.batches += batch }, { error -> received.errors += error })
        Disposer.register(testRootDisposable, reader)
        return reader
    }

    private fun waitFor(what: String, condition: () -> Boolean) {
        PlatformTestUtil.waitWithEventsDispatching(what, condition, WAIT_SECONDS)
    }

    private fun fixtureLines(): List<String> =
        checkNotNull(javaClass.getResource("/devtool/trace.jsonl")).readText().lines().filter { line -> line.isNotEmpty() }

    fun testRecordsParseIntoTheirKinds() {
        val file = traceFile()
        val lines = fixtureLines()
        Files.writeString(file, lines.joinToString("\n", postfix = "\n"))
        val received = Received()
        reader(file, received).finish()
        waitFor("every record arrives") { received.records.size == lines.size }
        assertEquals(emptyList<String>(), received.errors)
        val records = received.records

        assertEquals(Header(version = 1, architecture = "x64"), records.first())

        val block = records.filterIsInstance<Body>().single { body -> body.number == 5 }
        assertEquals("[] in Demo>>main:args:", block.name)
        assertEquals("Demo", block.moduleId)
        assertNull(block.moduleVersion)
        assertEquals("/work/Demo.ns", block.file)
        assertNull(block.library)
        assertEquals(SourceRange(28, 21, 28, 117), block.range)
        assertEquals("cc7d86cd57ba8347", block.checksum)
        assertEquals(Send(at = 2, site = 8, call = SourceRange(28, 57, 28, 69), selector = listOf(SourceRange(28, 63, 28, 67))), block.sends.first())
        // A statement's instructions run from the first up to, not including, the second.
        assertEquals(listOf(Statement(instructions = 0 until 19, range = SourceRange(28, 31, 28, 116))), block.statements)
        val library = records.filterIsInstance<Body>().single { body -> body.number == 17 }
        assertEquals("Kernel.ns", library.library)
        assertNull(library.file)

        val inlined = records.filterIsInstance<Event>().first { event -> event.event == "inlined" }
        assertEquals("inlining", inlined.category)
        assertEquals(5, inlined.body)
        assertEquals(5, inlined.integer("in-body"))
        assertEquals(3, inlined.integer("at"))
        assertEquals(3, inlined.integer("callee"))
        assertNull(inlined.text("key"))
        val folded = records.filterIsInstance<Event>().first { event -> event.event == "declined-no-body" }
        assertEquals("not-expressed", folded.fields.getAsJsonArray("reasons")[0].asJsonObject["reason"].asString)
        val transition = records.filterIsInstance<Event>().first { event -> event.event == "transition" }
        assertEquals("caches", transition.category)
        assertEquals("vacant", transition.text("before"))
        assertEquals("monomorphic", transition.text("after"))

        val bytecode = records.filterIsInstance<Bytecode>().single { listing -> listing.body == 4 }
        assertEquals(listOf(4, 5), bytecode.bodies.map { listed -> listed.body })
        assertEquals(BytecodeInstruction(index = 0, statement = null, text = "MakeVector r17, size 1"), bytecode.bodies.first().instructions.first())

        val code = records.filterIsInstance<Code>().first()
        assertEquals(5, code.body)
        assertEquals("baseline", code.tier)
        assertNull(code.key)
        assertNull(code.entered)
        assertEquals("x64", code.architecture)
        assertEquals(5312, code.bytes)
        assertEquals(CodeInstruction(offset = 0, text = "mov r11,[r14+58h]", scope = null, at = null, annotation = null), code.instructions.first())
        assertEquals(listOf(DataWord(offset = 0, word = "budget word")), code.data)
        assertEquals(listOf(Scope(scope = 0, body = 5, parent = null)), code.scopes)
        assertNull(code.mark)

        val inlining = records.filterIsInstance<Inlining>().single()
        assertEquals(5, inlining.body)
        assertEquals(Scope(scope = 1, body = 3, parent = ScopeParent(scope = 0, at = 3)), inlining.scopes[1])
        assertEquals(Arm(scope = 0, at = 3, receiver = "Demo", inlinedScope = 1, always = null), inlining.arms[0])
        assertEquals(Arm(scope = 0, at = 13, receiver = "newspeak.core.Kernel/Array", inlinedScope = null, always = "indexed"), inlining.arms[1])
        assertEquals(Decline(scope = 0, at = 2, callee = null, reason = "not-expressed"), inlining.declined.first())
        assertEquals(3601, inlining.installed)
        assertNull(inlining.refused)
    }

    fun testAPartialLastLineWaitsForItsEnd() {
        val file = traceFile()
        val event = """{"kind":"event","category":"compile","body":3,"event":"compiled","trigger":"call","bytes":1216}"""
        Files.writeString(file, HEADER + "\n" + event.substring(0, 40))
        val received = Received()
        val reader = reader(file, received)
        waitFor("the header arrives") { received.records.isNotEmpty() }
        // Longer than a publication's interval: a half record would have arrived by then.
        val until = System.currentTimeMillis() + 3 * PUBLICATION_MILLISECONDS
        waitFor("time passes") { System.currentTimeMillis() >= until }
        assertEquals(1, received.records.size)
        assertEquals(emptyList<String>(), received.errors)
        Files.writeString(file, event.substring(40) + "\n", StandardOpenOption.APPEND)
        reader.finish()
        waitFor("the event arrives") { received.records.size == 2 }
        assertEquals(emptyList<String>(), received.errors)
        val compiled = received.records[1] as Event
        assertEquals("compiled", compiled.event)
        assertEquals(1216, compiled.integer("bytes"))
    }

    fun testAnUnknownVersionIsRefused() {
        val file = traceFile()
        Files.writeString(
            file,
            """{"kind":"header","format":"ns-next-trace","version":2,"architecture":null}""" + "\n" +
                """{"kind":"event","category":"compile","body":3,"event":"compiled","trigger":"call","bytes":1216}""" + "\n",
        )
        val received = Received()
        reader(file, received).finish()
        waitFor("the refusal arrives") { received.errors.isNotEmpty() }
        assertEquals(listOf(NewspeakBundle.message("devtool.file.version", 2)), received.errors)
        assertEquals(emptyList<TraceRecord>(), received.records)
    }

    fun testALargeFileIsReadInBatches() {
        val file = traceFile()
        val count = 200_000
        Files.newBufferedWriter(file).use { writer ->
            writer.write(HEADER)
            writer.write("\n")
            for (index in 0 until count) {
                writer.write("""{"kind":"event","category":"osr","body":7,"event":"osr-entered","at":$index}""")
                writer.write("\n")
            }
        }
        val received = Received()
        reader(file, received).finish()
        waitFor("every record arrives") { received.records.size == count + 1 }
        assertEquals(emptyList<String>(), received.errors)
        assertTrue(received.batches.all { batch -> batch.size <= BATCH_RECORDS })
        val positions = received.records.drop(1).map { record -> (record as Event).integer("at") }
        assertEquals((0 until count).toList(), positions)
    }

    private companion object {
        const val WAIT_SECONDS = 60
        const val HEADER = """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64"}"""
    }
}
