package io.github.bstrauss.newspeak.devtool.model

import com.intellij.openapi.util.Disposer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class TraceRunTest {
    private fun run() = TraceRun("Demo", Instant.EPOCH)

    private fun records(vararg lines: String): List<TraceRecord> = lines.map { line -> checkNotNull(TraceRecord.parse(line)) }

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
                // Another copy's plan, which its own outcome decides.
                event(5, "inlining", "inlined", ""","key":"Demo/Box","entered":"Demo","in-body":5,"callee":3,"at":7"""),
                event(5, "optimize", "optimized", ""","key":"Demo","entered":null,"bytes":900"""),
            ),
        )
        assertEquals(mapOf(3 to SendOutcome(inlining = "declined: over-weight", cache = null)), traceRun.sends(5))
        // Without the `optimize` category, the plan's listing says how the compile ended.
        traceRun.add(records("""{"kind":"inlining","body":5,"key":"Demo/Box","entered":"Demo","scopes":[],"arms":[],"declined":[],"outcome":{"installed":800}}"""))
        assertEquals(SendOutcome(inlining = "inlined", cache = null), traceRun.sends(5)[7])
    }

    @Test fun testABodyWithoutADescriptionIsAccepted() {
        val traceRun = run()
        traceRun.add(records(event(99, "compile", "compiled", ""","trigger":"entry","bytes":64""")))
        assertNull(traceRun.bodies[99])
        assertEquals(MethodState.BASELINE, traceRun.state(99))
        assertEquals(emptyMap<Int, SendOutcome>(), traceRun.sends(99))
        assertEquals(1, traceRun.events.size)
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
