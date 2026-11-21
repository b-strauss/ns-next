package io.github.bstrauss.newspeak.devtool.model

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.util.Disposer
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.NewspeakBundle
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.ReadableByteChannel
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption
import java.util.Collections
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import java.time.Instant

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

        assertEquals(
            Header(
                version = 1,
                architecture = "x64",
                categories = listOf("compile", "osr", "optimize", "inlining", "deoptimize", "code-zone", "caches"),
                listings = listOf("bytecode", "code", "inlining"),
            ),
            records.first(),
        )

        val block = records.filterIsInstance<Body>().single { body -> body.number == 5 }
        assertEquals("[] in Demo>>main:args:", block.name)
        assertEquals(4, block.family)
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
        assertNull(library.family)
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

        // A listing and a plan arrive as their entries, and are read back whole at their places.
        val entry = records.filterIsInstance<CodeEntry>().first()
        assertEquals(5, entry.body)
        assertEquals("baseline", entry.tier)
        assertNull(entry.key)
        assertNull(entry.entered)
        assertEquals(2_147_549_184L, entry.base)
        assertEquals(5312, entry.bytes)
        val code = readRecord(file, entry.place) as Code
        assertEquals(5, code.body)
        assertEquals("x64", code.architecture)
        assertEquals(2_147_549_184L, code.base)
        assertEquals(5312, code.bytes)
        assertEquals(CodeInstruction(offset = 0, text = "mov r11,[r14+58h]", scope = null, at = null, annotation = null), code.instructions.first())
        assertEquals(listOf(DataWord(offset = 0, word = "budget word")), code.data)
        assertEquals(listOf(Scope(scope = 0, body = 5, parent = null)), code.scopes)
        assertNull(code.mark)

        val plan = records.filterIsInstance<PlanEntry>().single()
        assertEquals(5, plan.body)
        assertEquals(3601, plan.installed)
        assertNull(plan.refused)
        val inlining = readRecord(file, plan.place) as Inlining
        assertEquals(5, inlining.body)
        assertEquals(Scope(scope = 1, body = 3, parent = ScopeParent(scope = 0, at = 3)), inlining.scopes[1])
        assertEquals(Arm(scope = 0, at = 3, receiver = "Demo", inlinedScope = 1, always = null), inlining.arms[0])
        assertEquals(Arm(scope = 0, at = 13, receiver = "newspeak.core.Kernel/Array", inlinedScope = null, always = "indexed"), inlining.arms[1])
        assertEquals(Decline(scope = 0, at = 2, callee = null, reason = "not-expressed"), inlining.declined.first())
        assertEquals(3601, inlining.installed)
        assertNull(inlining.refused)
    }

    /**
     * The places are counted in bytes across reads: characters of two,
     * three and four bytes before and inside the listings, a line longer
     * than a read, and a listing whose end arrives only with a later write.
     */
    fun testAListingsPlaceIsItsLinesBytes() {
        val file = traceFile()
        val wide = "äé€𝄞"
        val first = codeLine(1, "mov $wide", 20_000)
        val second = codeLine(2, "add $wide", 3)
        val body = """{"kind":"body","body":1,"name":"Demo>>$wide","module":{"id":"Demo","version":null},"range":null,"checksum":"0000000000000000","statements":[],"sends":[]}"""
        val plan = """{"kind":"inlining","body":1,"key":"$wide","entered":null,"scopes":[],"arms":[],"declined":[],"outcome":{"refused":"zone-full"}}"""
        val written = listOf(HEADER, body, first, plan).joinToString("\n", postfix = "\n") + second.substring(0, 50)
        Files.writeString(file, written)
        val received = Received()
        val reader = reader(file, received)
        waitFor("the records before the partial line arrive") { received.records.size == 4 }
        Files.writeString(file, second.substring(50) + "\n", StandardOpenOption.APPEND)
        reader.finish()
        waitFor("the last listing arrives") { received.records.size == 5 }
        val entries = received.records.filterIsInstance<CodeEntry>()
        assertEquals(listOf(first, second), entries.map { entry -> readLine(file, entry.place) })
        assertEquals(20_000, (readRecord(file, entries[0].place) as Code).instructions.size)
        assertEquals("add $wide", (readRecord(file, entries[1].place) as Code).instructions.last().text)
        assertEquals(plan, readLine(file, received.records.filterIsInstance<PlanEntry>().single().place))
    }

    fun testAListingNoLongerInItsFileIsReportedAndNothingThrows() {
        val file = traceFile()
        Files.writeString(file, listOf(HEADER, codeLine(1, "ret", 1)).joinToString("\n", postfix = "\n"))
        val received = Received()
        reader(file, received).finish()
        waitFor("the listing arrives") { received.records.size == 2 }
        val place = received.records.filterIsInstance<CodeEntry>().single().place
        Files.writeString(file, HEADER + "\n")
        val shortened = unreadable(file, place)
        assertEquals(NewspeakBundle.message("devtool.record.unreadable", file.toString(), NewspeakBundle.message("devtool.record.short")), shortened.message)
        Files.delete(file)
        val gone = unreadable(file, place)
        assertTrue(checkNotNull(gone.message), checkNotNull(gone.message).startsWith(NewspeakBundle.message("devtool.record.unreadable", file.toString(), "")))
    }

    /**
     * A run read from a file of many listings and plans keeps none of their
     * text: what it keeps of each is its entry, whose every field is a
     * number, a short name or the record's place, and the listing is read
     * back from the file whole.
     */
    fun testARunKeepsNoListingText() {
        val file = traceFile()
        val marker = "listing-text-" + "x".repeat(200)
        val installs = 1_000
        val lines = ArrayList<String>()
        lines += HEADER
        for (index in 0 until installs) {
            lines += codeLine(1 + index % 10, "$marker $index", 20)
            lines += """{"kind":"inlining","body":${1 + index % 10},"key":"Demo","entered":null,"scopes":[{"scope":0,"body":1,"parent":null}],"arms":[{"scope":0,"at":3,"receiver":"$marker","inlined":{"always":"getter"}}],"declined":[{"scope":0,"at":4,"callee":null,"reason":"$marker"}],"outcome":{"installed":64}}"""
        }
        Files.writeString(file, lines.joinToString("\n", postfix = "\n"))
        val run = TraceRun("Demo", Instant.EPOCH, file = file)
        val reader = TraceFileReader(file, run::add, { error -> fail(error) }, run::ended)
        Disposer.register(testRootDisposable, reader)
        reader.finish()
        waitFor("every listing arrives") { run.plans.size == installs }
        val entries = run.code.values.flatten()
        assertEquals(installs, entries.size)
        for (entry in entries + run.plans) assertKeepsNoText(entry, marker)
        // Reading back gives the whole listing.
        val install = checkNotNull(run.code[1]).last()
        assertEquals("$marker 990", run.readCode(install).instructions.first().text)
        assertEquals(marker, run.readPlan(run.plans.last()).declined.single().reason)
    }

    /** Fails where a field of [entry] holds the text [marker], or any text longer than a name. */
    private fun assertKeepsNoText(entry: Any, marker: String) {
        for (field in entry.javaClass.declaredFields) {
            field.isAccessible = true
            when (val value = field.get(entry)) {
                null, is Number, is RecordPlace -> Unit
                is String -> assertTrue("${field.name} = $value", value.length < SHORT_TEXT && marker !in value)
                else -> fail("${entry.javaClass.simpleName}.${field.name} holds a ${value.javaClass.name}")
            }
        }
    }

    /** Why the record at [place] can no longer be read from [file]. */
    private fun unreadable(file: Path, place: RecordPlace): UnreadableRecordException {
        try {
            readRecord(file, place)
        } catch (exception: UnreadableRecordException) {
            return exception
        }
        throw AssertionError("the record was read")
    }

    private fun readLine(file: Path, place: RecordPlace): String {
        val bytes = Files.readAllBytes(file)
        return String(bytes, place.offset.toInt(), place.length, Charsets.UTF_8)
    }

    /** A baseline listing of [body] of [count] instructions, each [text]. */
    private fun codeLine(body: Int, text: String, count: Int): String {
        val instructions = (0 until count).joinToString(",") { index -> """{"offset":$index,"text":"$text","scope":null,"at":null,"annotation":null}""" }
        return """{"kind":"code","body":$body,"tier":"baseline","key":null,"entered":null,"architecture":"x64","bytes":$count,"instructions":[$instructions],"data":[],"scopes":[],"mark":null}"""
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

    fun testTheEndIsToldAfterTheLastBatch() {
        val file = traceFile()
        Files.writeString(file, HEADER + "\n")
        val received = Received()
        var recordsAtTheEnd = -1
        val reader = TraceFileReader(file, { batch -> received.batches += batch }, { error -> received.errors += error }) {
            recordsAtTheEnd = received.records.size
        }
        Disposer.register(testRootDisposable, reader)
        reader.finish()
        waitFor("the end is told") { recordsAtTheEnd >= 0 }
        assertEquals(1, recordsAtTheEnd)
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

    fun testAMalformedLineKeepsTheRecordsBeforeIt() {
        val file = traceFile()
        val compiled = """{"kind":"event","category":"compile","body":3,"event":"compiled","trigger":"call","bytes":1216}"""
        val withoutCategory = """{"kind":"event","body":3,"event":"compiled"}"""
        Files.writeString(file, HEADER + "\n" + compiled + "\n" + withoutCategory + "\n" + compiled + "\n")
        val received = Received()
        reader(file, received).finish()
        waitFor("the error arrives") { received.errors.isNotEmpty() }
        assertEquals(listOf(NewspeakBundle.message("devtool.file.malformed", "3", "no `category`")), received.errors)
        // Published before the error: the records up to the malformed line, none after it.
        assertEquals(2, received.records.size)
        assertEquals("compiled", (received.records[1] as Event).event)
    }

    fun testAnUnreadableFileKeepsTheRecordsReadBeforeIt() {
        val file = traceFile()
        val compiled = """{"kind":"event","category":"compile","body":3,"event":"compiled","trigger":"call","bytes":1216}"""
        // The file's bytes, then a read the operating system refuses.
        val bytes = ByteBuffer.wrap((HEADER + "\n" + compiled + "\n").toByteArray())
        val channel = object : ReadableByteChannel {
            override fun read(destination: ByteBuffer): Int {
                if (!bytes.hasRemaining()) throw IOException("the disk is gone")
                val count = minOf(destination.remaining(), bytes.remaining())
                repeat(count) { destination.put(bytes.get()) }
                return count
            }

            override fun isOpen() = true

            override fun close() {}
        }
        val received = Received()
        val reader = TraceFileReader(
            file,
            { batch -> received.batches += batch },
            { error -> received.errors += error },
            {},
            { task -> ApplicationManager.getApplication().invokeLater(task) },
            { milliseconds -> Thread.sleep(milliseconds) },
            System::currentTimeMillis,
            { _ -> channel },
        )
        Disposer.register(testRootDisposable, reader)
        waitFor("the error arrives") { received.errors.isNotEmpty() }
        assertEquals(listOf(NewspeakBundle.message("devtool.file.unreadable", file.toString(), "the disk is gone")), received.errors)
        // Published before the error: every record read.
        assertEquals(2, received.records.size)
        assertEquals("compiled", (received.records[1] as Event).event)
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

    fun testABacklogWaitsForTheLastBatchToBeApplied() {
        val file = traceFile()
        val count = 3 * BATCH_RECORDS
        Files.newBufferedWriter(file).use { writer ->
            writer.write(HEADER)
            writer.write("\n")
            for (index in 0 until count) {
                writer.write("""{"kind":"event","category":"osr","body":7,"event":"osr-entered","at":$index}""")
                writer.write("\n")
            }
        }
        // The EDT's queue as the reader fills it, run by hand.
        val queued = LinkedBlockingQueue<Runnable>()
        val received = Received()
        val reader = TraceFileReader(
            file,
            { batch -> received.batches += batch },
            { error -> received.errors += error },
            {},
            { task -> queued.add(task) },
            { milliseconds -> Thread.sleep(milliseconds) },
            System::currentTimeMillis,
        )
        Disposer.register(testRootDisposable, reader)
        reader.finish()
        var applied = 0
        while (received.records.size < count + 1) {
            val task = checkNotNull(queued.poll(WAIT_SECONDS.toLong(), TimeUnit.SECONDS)) { "a batch is handed over" }
            // Longer than the reader needs to read and parse a batch: a reader that does not wait would have queued the next.
            Thread.sleep(3 * PUBLICATION_MILLISECONDS)
            assertEquals("no batch is handed over before the last was applied", 0, queued.size)
            task.run()
            applied++
        }
        assertTrue("the backlog came in several batches", applied > 2)
        assertEquals(emptyList<String>(), received.errors)
    }

    fun testAQuietFileIsLookedAtLessOften() {
        val file = traceFile()
        Files.writeString(file, HEADER + "\n")
        // The time as the pauses pass it, so that the waits are the reader's own and not the machine's.
        val now = AtomicLong()
        val pauses = Collections.synchronizedList(ArrayList<Long>())
        val received = Received()
        val reader = TraceFileReader(
            file,
            { batch -> received.batches += batch },
            { error -> received.errors += error },
            {},
            { task -> ApplicationManager.getApplication().invokeLater(task) },
            { milliseconds ->
                pauses.add(milliseconds)
                now.addAndGet(milliseconds)
                Thread.sleep(1)
            },
            now::get,
        )
        Disposer.register(testRootDisposable, reader)
        waitFor("the header arrives and the file stays quiet") { received.records.isNotEmpty() && pauses.size >= 6 }
        // The header waits no longer than its publication is due; then each look waits twice as long, up to the longest.
        assertEquals(listOf(POLL_MILLISECONDS, 100L, 50L, 400L, LONGEST_POLL_MILLISECONDS, LONGEST_POLL_MILLISECONDS), pauses.take(6))
        val looks = pauses.size
        Files.writeString(file, """{"kind":"event","category":"osr","body":7,"event":"osr-entered","at":1}""" + "\n", StandardOpenOption.APPEND)
        fun afterTheBytes() = synchronized(pauses) { pauses.drop(looks).firstOrNull { pause -> pause != LONGEST_POLL_MILLISECONDS } }
        waitFor("the event arrives and the reader looks again") { received.records.size == 2 && afterTheBytes() != null }
        assertEquals("appended bytes bring the wait back to the shortest", POLL_MILLISECONDS, afterTheBytes())
        reader.finish()
    }

    private companion object {
        const val WAIT_SECONDS = 60
        /** Longer than any tier, key or reason a code or plan entry names. */
        const val SHORT_TEXT = 100

        const val HEADER = """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64"}"""
    }
}
