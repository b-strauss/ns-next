package io.github.bstrauss.newspeak.devtool.model

import com.google.gson.JsonObject
import com.intellij.openapi.util.Disposer
import io.github.bstrauss.newspeak.devtool.TraceFilters
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class TraceRunTest {
    private fun run() = trace.run()

    /** The file the records are written to, as the reader hands a run its listings and plans: as entries naming their places. */
    private val trace = IndexedTrace()

    private fun records(vararg lines: String): List<TraceRecord> = trace.records(*lines)

    private fun event(body: Int, category: String, event: String, fields: String = ""): String =
        """{"kind":"event","category":"$category","body":$body,"event":"$event"$fields}"""

    /** The fixture's records: a hot block whose sends were inlined, one of them polymorphic. */
    private fun fixtureRun(): TraceRun {
        val lines = checkNotNull(javaClass.getResource("/devtool/trace.jsonl")).readText().lines().filter { line -> line.isNotEmpty() }
        return run().also { traceRun -> traceRun.add(records(*lines.toTypedArray())) }
    }

    @Test fun testStateFollowsTheEvents() {
        val traceRun = run()
        assertEquals(MethodState.INTERPRETED, traceRun.state(3))
        traceRun.add(records(event(3, "compile", "compiled", ""","trigger":"call","bytes":1216""")))
        assertEquals(MethodState.BASELINE, traceRun.state(3))
        traceRun.add(records(event(3, "compile", "compiled-twin", ""","bytes":1300""")))
        assertEquals(MethodState.TWIN, traceRun.state(3))
        traceRun.add(records(event(3, "optimize", "optimized", ""","key":"Demo","entered":null,"bytes":900""")))
        assertEquals(MethodState.OPTIMIZED, traceRun.state(3))
        assertEquals(1, traceRun.copies(3))
        traceRun.add(records(event(3, "optimize", "optimized", ""","key":"Demo/Account","entered":"Demo","bytes":900""")))
        traceRun.add(records(event(3, "deoptimize", "invalidated", ""","key":"Demo/Account","entered":"Demo","cause":"edit"""")))
        assertEquals("a copy is left", MethodState.OPTIMIZED, traceRun.state(3))
        // Baseline code freed says nothing of the optimized copies.
        traceRun.add(records(event(3, "code-zone", "freed", ""","key":null,"entered":null,"tier":"baseline","cause":"compaction"""")))
        assertEquals(MethodState.OPTIMIZED, traceRun.state(3))
        traceRun.add(records(event(3, "deoptimize", "retired", ""","key":"Demo","entered":null,"reason":"exits","version":null""")))
        assertEquals(MethodState.DEOPTIMIZED, traceRun.state(3))
        assertEquals(0, traceRun.copies(3))
        traceRun.add(records(event(3, "compile", "abandoned", ""","exits":1000""")))
        assertEquals(MethodState.ABANDONED, traceRun.state(3))
        // Revived, it counts its entries again: interpreted until it is compiled.
        traceRun.add(records(event(3, "compile", "revived", ""","by":"decay"""")))
        assertEquals(MethodState.INTERPRETED, traceRun.state(3))
        traceRun.add(records(event(3, "compile", "compiled", ""","trigger":"entry","bytes":1216""")))
        assertEquals(MethodState.BASELINE, traceRun.state(3))
        // Its baseline code freed, it runs interpreted again.
        traceRun.add(records(event(3, "code-zone", "freed", ""","key":null,"entered":null,"tier":"baseline","cause":"compaction"""")))
        assertEquals(MethodState.INTERPRETED, traceRun.state(3))

        traceRun.add(records(event(4, "optimize", "optimized", ""","key":null,"entered":null,"bytes":900""")))
        traceRun.add(records(event(4, "code-zone", "freed", ""","key":null,"entered":null,"tier":"optimized","cause":"breakpoint"""")))
        assertEquals(MethodState.DEOPTIMIZED, traceRun.state(4))

        // The zone replaced drops every body's machine code, copies included.
        traceRun.add(records(event(5, "compile", "compiled", ""","trigger":"call","bytes":64""")))
        traceRun.add(records(event(5, "optimize", "optimized", ""","key":null,"entered":null,"bytes":900""")))
        traceRun.add(records(event(6, "compile", "compiled", ""","trigger":"call","bytes":64""")))
        traceRun.add(records("""{"kind":"event","category":"code-zone","body":null,"event":"replaced","bodies-freed":2}"""))
        assertEquals(MethodState.INTERPRETED, traceRun.state(5))
        assertEquals(0, traceRun.copies(5))
        assertEquals(MethodState.INTERPRETED, traceRun.state(6))

        val fixture = fixtureRun()
        assertEquals(MethodState.OPTIMIZED, fixture.state(5))
        assertEquals(MethodState.OPTIMIZED, fixture.state(17))
        assertEquals(MethodState.BASELINE, fixture.state(3))
        assertEquals(MethodState.INTERPRETED, fixture.state(4))
    }

    @Test fun testASendCollectsItsInliningAndItsCache() {
        val traceRun = fixtureRun()
        // The block's sites are its method's family's: its transitions are the method's events.
        assertEquals(SendOutcome(inlining = "inlined", cache = "mono"), traceRun.sends(5)[3])
        assertEquals(SendOutcome(inlining = null, cache = "mono"), traceRun.sends(5)[2])
        assertEquals(SendOutcome(inlining = "inlined", cache = "poly 2"), traceRun.sends(2)[0])
        assertEquals(SendOutcome(inlining = null, cache = "mono"), traceRun.sends(4)[18])
        assertNull("a send nothing became of", traceRun.sends(5)[6])

        traceRun.add(
            records(
                event(5, "inlining", "declined", ""","key":null,"entered":null,"in-body":5,"callee":2,"reason":"over-weight","at":14"""),
                event(5, "optimize", "optimized", ""","key":null,"entered":null,"bytes":3601"""),
                event(2, "caches", "transition", ""","before":"polymorphic","after":"megamorphic","selector":"area","site":0,"receiver":"Demo/Triangle""""),
            ),
        )
        assertEquals(SendOutcome(inlining = "declined: over-weight", cache = "mono"), traceRun.sends(5)[14])
        assertEquals(SendOutcome(inlining = "inlined", cache = "mega"), traceRun.sends(2)[0])

        // Vacated, no site keeps its state, and each counts its receivers afresh.
        traceRun.add(records("""{"kind":"event","category":"caches","body":null,"event":"vacated","cause":"edit"}"""))
        assertNull(traceRun.sends(2)[0]?.cache)
        assertNull(traceRun.sends(5)[3]?.cache)
        traceRun.add(records(event(2, "caches", "transition", ""","before":"vacant","after":"monomorphic","selector":"area","site":0,"receiver":"Demo/Circle"""")))
        assertEquals("mono", traceRun.sends(2)[0]?.cache)
        traceRun.add(records(event(2, "caches", "transition", ""","before":"monomorphic","after":"polymorphic","selector":"area","site":0,"receiver":"Demo/Triangle"""")))
        assertEquals("poly 2", traceRun.sends(2)[0]?.cache)
    }

    @Test fun testABlocksSendsFindTheirCachesThroughItsFamily() {
        val traceRun = run()
        traceRun.add(
            records(
                """{"kind":"body","body":4,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[{"at":1,"site":0,"call":null,"selector":[]}]}""",
                """{"kind":"body","body":5,"name":"[] in Demo>>run","family":4,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000002","statements":[],"sends":[{"at":2,"site":1,"call":null,"selector":[]}]}""",
                event(4, "caches", "transition", ""","before":"vacant","after":"monomorphic","selector":"size","site":1,"receiver":"Demo/Box""""),
            ),
        )
        assertTrue("no bytecode listing names the family", traceRun.bytecode.isEmpty())
        assertEquals(mapOf(2 to SendOutcome(inlining = null, cache = "mono")), traceRun.sends(5))
        assertEquals("the method's own site 0 saw nothing", emptyMap<Int, SendOutcome>(), traceRun.sends(4))
    }

    @Test fun testOnlyAnInstalledCopysPlanCounts() {
        val traceRun = run()
        traceRun.add(
            records(
                """{"kind":"body","body":5,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[{"at":3,"site":null,"call":null,"selector":[]},{"at":7,"site":null,"call":null,"selector":[]}]}""",
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"at":3"""),
            ),
        )
        assertEquals("its outcome has not arrived", emptyMap<Int, SendOutcome>(), traceRun.sends(5))
        traceRun.add(records(event(5, "optimize", "refused", ""","key":"Demo","entered":null,"reason":"too-large"""")))
        assertEquals("a refused copy's plan says nothing", emptyMap<Int, SendOutcome>(), traceRun.sends(5))
        traceRun.add(
            records(
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"at":3"""),
                event(5, "optimize", "deferred", ""","key":"Demo","entered":null,"reason":"zone-full""""),
            ),
        )
        assertEquals("a deferred copy's plan says nothing", emptyMap<Int, SendOutcome>(), traceRun.sends(5))
        traceRun.add(
            records(
                event(5, "inlining", "declined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"reason":"over-weight","at":3"""),
                event(5, "optimize", "optimized", ""","key":"Demo","entered":null,"bytes":900"""),
                // Another copy's plan, which its own outcome decides.
                event(5, "inlining", "inlined", ""","key":"Demo/Box","entered":"Demo","in-body":5,"callee":3,"at":7"""),
            ),
        )
        assertEquals(mapOf(3 to SendOutcome(inlining = "declined: over-weight", cache = null)), traceRun.sends(5))
        // Without the `optimize` category, the plan's listing says how the compile ended.
        traceRun.add(records("""{"kind":"inlining","body":5,"key":"Demo/Box","entered":"Demo","scopes":[],"arms":[],"declined":[],"outcome":{"installed":800}}"""))
        assertEquals(SendOutcome(inlining = "inlined", cache = null), traceRun.sends(5)[7])
    }

    @Test fun testASendsEventIsTheInstalledCopysDecision() {
        val traceRun = run()
        traceRun.add(
            records(
                """{"kind":"body","body":5,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[{"at":3,"site":null,"call":null,"selector":[]},{"at":7,"site":null,"call":null,"selector":[]}]}""",
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"at":3"""),
                event(5, "optimize", "optimized", ""","key":"Demo","entered":null,"bytes":900"""),
                // A later compile of the same copy declines the send, and is put off: its decision is no copy's.
                event(5, "inlining", "declined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"reason":"over-weight","at":3"""),
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":4,"at":7"""),
                event(5, "optimize", "deferred", ""","key":"Demo","entered":null,"reason":"zone-full""""),
            ),
        )
        assertEquals("inlined", traceRun.sends(5)[3]?.inlining)
        assertEquals("inlined", traceRun.inliningEvent(5, 3)?.event)
        assertNull("a deferred compile's decision", traceRun.inliningEvent(5, 7))
    }

    @Test fun testACompileWhoseOutcomeIsWithheldIsDroppedAtTheNextRecord() {
        val traceRun = run()
        traceRun.add(
            records(
                """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["inlining","deoptimize","code-zone"],"listings":["inlining"]}""",
                """{"kind":"body","body":5,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[{"at":3,"site":null,"call":null,"selector":[]},{"at":7,"site":null,"call":null,"selector":[]}]}""",
                // A compile whose plan the listings filter withheld: nothing says whether its copy was installed.
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"at":3"""),
                // A body described between two inlining events belongs to no compile.
                """{"kind":"body","body":6,"name":"Demo>>helper","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000002","statements":[],"sends":[]}""",
                event(5, "inlining", "declined", ""","key":"Demo","entered":null,"in-body":5,"callee":6,"reason":"over-weight","at":7"""),
            ),
        )
        assertEquals("a compile's stretch waits for its outcome", 2, traceRun.waitingDecisions)
        traceRun.add(records(event(6, "compile", "compiled", ""","trigger":"call","bytes":64""")))
        assertEquals("the stretch ended without an outcome", 0, traceRun.waitingDecisions)
        // A later compile of the same copy, whose plan is heard, says nothing of the send at 3.
        traceRun.add(
            records(
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":6,"at":7"""),
                """{"kind":"inlining","body":5,"key":"Demo","entered":null,"scopes":[],"arms":[],"declined":[],"outcome":{"installed":800}}""",
            ),
        )
        assertEquals(mapOf(7 to SendOutcome(inlining = "inlined", cache = null)), traceRun.sends(5))
        assertEquals(0, traceRun.waitingDecisions)
        // The run's end drops a compile still waiting.
        traceRun.add(records(event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":6,"at":3""")))
        assertEquals(1, traceRun.waitingDecisions)
        traceRun.ended()
        assertEquals(0, traceRun.waitingDecisions)
    }

    /** A run whose header names [categories] and [listings], with a body whose send at 3 a plan inlines. */
    private fun planned(categories: String, listings: String): TraceRun {
        val traceRun = run()
        traceRun.add(
            records(
                """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":$categories,"listings":$listings}""",
                """{"kind":"body","body":5,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[{"at":3,"site":null,"call":null,"selector":[]}]}""",
                event(5, "inlining", "inlined", ""","key":null,"entered":null,"in-body":5,"callee":3,"at":3"""),
            ),
        )
        return traceRun
    }

    @Test fun testAPlanNoOutcomeCanConcludeIsNeverShown() {
        val inlined = mapOf(3 to SendOutcome(inlining = "inlined", cache = null))
        // Whether the copy was installed is never heard: a refused or deferred compile's decision would stand for good.
        val unconcluded = planned("""["inlining","deoptimize","code-zone"]""", """["code"]""")
        // The same copy compiled again and again, each compile declining a callee, as a deferral retried while the zone stays full.
        repeat(3) {
            unconcluded.add(records(event(5, "inlining", "declined", ""","key":null,"entered":null,"in-body":5,"callee":4,"reason":"over-weight","at":3""")))
        }
        assertEquals("neither `optimize` nor the plans traced", emptyMap<Int, SendOutcome>(), unconcluded.sends(5))
        assertNull(unconcluded.inliningEvent(5, 3))
        assertEquals("nothing waits", 0, unconcluded.waitingDecisions)
        assertEquals("no compile is heard installed, so none of its declines counts", 0, unconcluded.tallies[5]?.declines ?: 0)
        assertEquals("`optimize` traced", emptyMap<Int, SendOutcome>(), planned("""["inlining","optimize","deoptimize","code-zone"]""", "[]").sends(5))
        assertEquals("the plans listed", emptyMap<Int, SendOutcome>(), planned("""["inlining","deoptimize","code-zone"]""", """["inlining"]""").sends(5))
        // A header that does not say is taken to trace an outcome.
        val unsaid = run()
        unsaid.add(
            records(
                """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64"}""",
                """{"kind":"body","body":5,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[{"at":3,"site":null,"call":null,"selector":[]}]}""",
                event(5, "inlining", "inlined", ""","key":null,"entered":null,"in-body":5,"callee":3,"at":3"""),
            ),
        )
        assertEquals(emptyMap<Int, SendOutcome>(), unsaid.sends(5))
        unsaid.add(records(event(5, "optimize", "optimized", ""","key":null,"entered":null,"bytes":900""")))
        assertEquals(inlined, unsaid.sends(5))
    }

    @Test fun testDeclinesCountOnlyForAnInstalledCopy() {
        val traceRun = run()
        val declined = event(5, "inlining", "declined", ""","key":null,"entered":null,"in-body":5,"callee":4,"reason":"over-weight","at":3""")
        // Deferred twice, then installed: only the installed compile's decline counts.
        repeat(2) { traceRun.add(records(declined, event(5, "optimize", "deferred", ""","key":null,"entered":null,"reason":"zone-full""""))) }
        assertEquals(0, traceRun.tallies[5]?.declines ?: 0)
        traceRun.add(records(declined, event(5, "optimize", "refused", ""","key":null,"entered":null,"reason":"too-large"""")))
        assertEquals(0, traceRun.tallies[5]?.declines ?: 0)
        traceRun.add(records(declined, event(5, "optimize", "optimized", ""","key":null,"entered":null,"bytes":900""")))
        assertEquals(1, checkNotNull(traceRun.tallies[5]).declines)
    }

    @Test fun testASendsInliningGoesWithItsCopy() {
        val traceRun = run()
        traceRun.add(
            records(
                """{"kind":"body","body":5,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[{"at":3,"site":null,"call":null,"selector":[]},{"at":7,"site":null,"call":null,"selector":[]}]}""",
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"at":3"""),
                event(5, "optimize", "optimized", ""","key":"Demo","entered":null,"bytes":900"""),
                event(5, "inlining", "declined", ""","key":"Demo/Box","entered":"Demo","in-body":5,"callee":3,"reason":"over-weight","at":3"""),
                event(5, "inlining", "inlined", ""","key":"Demo/Box","entered":"Demo","in-body":5,"callee":4,"at":7"""),
                event(5, "optimize", "optimized", ""","key":"Demo/Box","entered":"Demo","bytes":800"""),
            ),
        )
        assertEquals("the copy installed last decides", SendOutcome(inlining = "declined: over-weight", cache = null), traceRun.sends(5)[3])
        // Retired, the later copy's decisions go: the send at 3 is the first copy's again, the one at 7 nothing's.
        traceRun.add(records(event(5, "deoptimize", "retired", ""","key":"Demo/Box","entered":"Demo","reason":"exits","version":null""")))
        assertEquals(mapOf(3 to SendOutcome(inlining = "inlined", cache = null)), traceRun.sends(5))
        // Baseline code freed takes no copy's decisions.
        traceRun.add(records(event(5, "code-zone", "freed", ""","key":null,"entered":null,"tier":"baseline","cause":"compaction"""")))
        assertEquals(mapOf(3 to SendOutcome(inlining = "inlined", cache = null)), traceRun.sends(5))
        traceRun.add(records(event(5, "code-zone", "freed", ""","key":"Demo","entered":null,"tier":"optimized","cause":"breakpoint"""")))
        assertEquals("freed", emptyMap<Int, SendOutcome>(), traceRun.sends(5))

        // Invalidated, and the zone replaced, likewise.
        traceRun.add(
            records(
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"at":3"""),
                event(5, "optimize", "optimized", ""","key":"Demo","entered":null,"bytes":900"""),
            ),
        )
        assertEquals("inlined", traceRun.sends(5)[3]?.inlining)
        traceRun.add(records(event(5, "deoptimize", "invalidated", ""","key":"Demo","entered":null,"cause":"edit"""")))
        assertEquals("invalidated", emptyMap<Int, SendOutcome>(), traceRun.sends(5))
        traceRun.add(
            records(
                event(5, "inlining", "inlined", ""","key":"Demo","entered":null,"in-body":5,"callee":3,"at":3"""),
                event(5, "optimize", "optimized", ""","key":"Demo","entered":null,"bytes":900"""),
                """{"kind":"event","category":"code-zone","body":null,"event":"replaced","bodies-freed":1}""",
            ),
        )
        assertEquals("the zone replaced", emptyMap<Int, SendOutcome>(), traceRun.sends(5))
    }

    /** A body described before any event: a run with [header] and [filters], `Demo>>run` its body 5 and `Demo>>helper` its body 6. */
    private fun described(header: String, filters: TraceFilters = TraceFilters.EVERYTHING): TraceRun {
        val traceRun = TraceRun("Demo", Instant.EPOCH, filters)
        traceRun.add(
            records(
                header,
                """{"kind":"body","body":5,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[]}""",
                """{"kind":"body","body":6,"name":"Demo>>helper","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000002","statements":[],"sends":[]}""",
            ),
        )
        return traceRun
    }

    @Test fun testAStateIsAnsweredOnlyWhereTheTraceKnowsIt() {
        val cachesOnly = described("""{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["caches"],"listings":["bytecode"]}""")
        assertNull("no compile traced", cachesOnly.state(5))
        // A listing of its machine code says its tier all the same.
        cachesOnly.add(records("""{"kind":"code","body":6,"tier":"baseline","key":null,"entered":null,"architecture":"x64","bytes":64,"instructions":[],"data":[],"scopes":[],"mark":null}"""))
        assertNull("a listing whose end no event would tell", cachesOnly.state(6))

        val compiles = """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["compile","inlining"],"listings":[]}"""
        assertEquals("compiles traced for every body", MethodState.INTERPRETED, described(compiles).state(6))
        val filtered = described(compiles, TraceFilters(listOf("Demo>>run"), emptyList(), noEvents = false, noListings = true))
        assertEquals(MethodState.INTERPRETED, filtered.state(5))
        // helper is described as a callee of run's plan; its own events were filtered out.
        filtered.add(records(event(5, "inlining", "inlined", ""","key":null,"entered":null,"in-body":5,"callee":6,"at":3""")))
        assertNull("a callee filtered out", filtered.state(6))
        assertEquals(MethodState.INTERPRETED, described(compiles, TraceFilters(listOf("Demo>>..."), emptyList(), noEvents = false, noListings = true)).state(6))
    }

    private fun optimizedListing(key: String) =
        """{"kind":"code","body":5,"tier":"optimized","key":"$key","entered":null,"architecture":"x64","bytes":64,"instructions":[],"data":[],"scopes":[],"mark":null}"""

    @Test fun testAListingCountsOnlyWhereItsEndCanBeHeard() {
        val listingsOnly = described("""{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":[],"listings":["code"]}""")
        listingsOnly.add(records(optimizedListing("Demo")))
        assertNull("no retirement could ever take the copy back", listingsOnly.state(5))
        assertEquals(0, listingsOnly.copies(5))

        val deoptimizations = described("""{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["deoptimize","code-zone"],"listings":["code"]}""")
        deoptimizations.add(records(optimizedListing("Demo"), optimizedListing("Demo/Box")))
        assertEquals(MethodState.OPTIMIZED, deoptimizations.state(5))
        assertEquals(2, deoptimizations.copies(5))
        deoptimizations.add(records(event(5, "deoptimize", "retired", ""","key":"Demo","entered":null,"reason":"exits","version":null""")))
        assertEquals(1, deoptimizations.copies(5))
        deoptimizations.add(records(event(5, "deoptimize", "invalidated", ""","key":"Demo/Box","entered":null,"cause":"edit"""")))
        assertEquals(MethodState.DEOPTIMIZED, deoptimizations.state(5))
    }

    @Test fun testNoInliningIsShownWhereNoRetirementCanArrive() {
        val traceRun = run()
        traceRun.add(
            records(
                """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["optimize","inlining"],"listings":[]}""",
                """{"kind":"body","body":5,"name":"Demo>>run","family":null,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[{"at":3,"site":null,"call":null,"selector":[]}]}""",
                event(5, "inlining", "inlined", ""","key":null,"entered":null,"in-body":5,"callee":3,"at":3"""),
                event(5, "optimize", "optimized", ""","key":null,"entered":null,"bytes":900"""),
            ),
        )
        assertEquals(emptyMap<Int, SendOutcome>(), traceRun.sends(5))
        assertNull("no tier whose end can be heard", traceRun.state(5))
    }

    /** A run whose header traces [categories], its body 5 compiled and optimized. */
    private fun tiered(categories: String): TraceRun {
        val traceRun = described("""{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":$categories,"listings":[]}""")
        traceRun.add(
            records(
                event(5, "compile", "compiled", ""","trigger":"call","bytes":64"""),
                event(5, "optimize", "optimized", ""","key":null,"entered":null,"bytes":900"""),
                event(6, "compile", "compiled", ""","trigger":"call","bytes":64"""),
            ),
        )
        return traceRun
    }

    @Test fun testATierIsShownOnlyWhereItsEndCanBeHeard() {
        val withoutDeoptimize = tiered("""["compile","optimize","code-zone"]""")
        assertEquals("the baseline tier beneath the copy, whose end can be heard", MethodState.BASELINE, withoutDeoptimize.state(5))
        assertEquals(0, withoutDeoptimize.copies(5))
        val withoutCodeZone = tiered("""["compile","optimize","deoptimize"]""")
        assertNull("neither a copy nor baseline code whose free could be heard", withoutCodeZone.state(5))
        assertNull(withoutCodeZone.state(6))
        val all = tiered("""["compile","optimize","deoptimize","code-zone"]""")
        assertEquals(MethodState.OPTIMIZED, all.state(5))
        assertEquals(1, all.copies(5))
        assertEquals(MethodState.BASELINE, all.state(6))
        all.add(records(event(5, "deoptimize", "retired", ""","key":null,"entered":null,"reason":"exits","version":null""")))
        assertEquals(MethodState.DEOPTIMIZED, all.state(5))
    }

    @Test fun testABodyPlacedInTheCurrentTextHasNoPlaceInTheFile() {
        val ranged = { placed: String -> """{"kind":"body","body":4,"name":"Demo>>helper","family":null,"module":{"id":"Demo","version":null},"source":{"file":"/work/Demo.ns"},"range":[12,2,14,3],"placed":"$placed","checksum":"0000000000000001","statements":[{"instructions":[0,1],"range":[13,4,13,9]}],"sends":[{"at":0,"site":0,"call":[13,4,13,9],"selector":[[13,6,13,9]]}]}""" }
        val current = TraceRecord.parse(ranged("current")) as Body
        assertTrue(current.placedInCurrentText)
        assertNull(current.range)
        assertNull(current.statements.single().range)
        assertNull(current.sends.single().call)
        assertEquals(listOf<SourceRange?>(null), current.sends.single().selector)
        val inFile = TraceRecord.parse(ranged("source")) as Body
        assertFalse(inFile.placedInCurrentText)
        assertEquals(SourceRange(12, 2, 14, 3), inFile.range)
        assertEquals(SourceRange(13, 6, 13, 9), inFile.sends.single().selector.single())
    }

    @Test fun testABodyWithoutADescriptionIsAccepted() {
        val traceRun = run()
        traceRun.add(records(event(99, "compile", "compiled", ""","trigger":"entry","bytes":64""")))
        assertNull(traceRun.bodies[99])
        assertEquals(MethodState.BASELINE, traceRun.state(99))
        assertEquals(emptyMap<Int, SendOutcome>(), traceRun.sends(99))
        assertEquals(1, traceRun.events.size)
    }

    @Test fun testARunKeepsItsLastEventsAndCountsEveryOne() {
        val traceRun = run()
        traceRun.add(
            records(
                event(3, "inlining", "declined", ""","key":null,"entered":null,"in-body":3,"callee":4,"reason":"over-weight","at":2"""),
                event(3, "optimize", "optimized", ""","key":null,"entered":null,"bytes":900"""),
                event(3, "deoptimize", "retired", ""","key":null,"entered":null,"reason":"exits","version":null"""),
            ),
        )
        val osr = Event("osr", 3, "osr-entered", JsonObject())
        traceRun.add(List(KEPT_EVENTS) { osr })
        assertEquals(KEPT_EVENTS, traceRun.events.size)
        assertEquals(3L, traceRun.droppedEvents)
        assertEquals(KEPT_EVENTS + 3L, traceRun.eventCount)
        assertTrue("the oldest are dropped", traceRun.events.all { kept -> kept === osr })
        // What the dropped events said stays.
        val tally = checkNotNull(traceRun.tallies[3])
        assertEquals(1, tally.deoptimizations)
        assertEquals(1, tally.declines)
        assertEquals(900, tally.largestOptimized)
        assertEquals("retired", tally.lastDeoptimization?.event)
        assertEquals(MethodState.DEOPTIMIZED, traceRun.state(3))
    }

    @Test fun testListenersHearEachBatch() {
        val traceRun = run()
        val parent = Disposer.newDisposable()
        var heard = 0
        traceRun.onChanged(parent) { heard++ }
        traceRun.add(records(event(3, "compile", "compiled", ""","trigger":"call","bytes":1216""")))
        traceRun.add(emptyList())
        assertEquals(1, heard)
        Disposer.dispose(parent)
        traceRun.add(records(event(3, "compile", "compiled-twin", ""","bytes":1300""")))
        assertEquals(1, heard)
    }
}
