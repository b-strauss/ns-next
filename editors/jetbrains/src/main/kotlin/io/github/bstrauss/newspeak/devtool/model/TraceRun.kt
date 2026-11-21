package io.github.bstrauss.newspeak.devtool.model

import com.intellij.openapi.Disposable
import com.intellij.openapi.util.Disposer
import java.time.Instant

/** How a method ran, as its gutter icon says (`docs/design/developer-tool.md`, decision 27). */
enum class MethodState { INTERPRETED, BASELINE, TWIN, OPTIMIZED, DEOPTIMIZED, ABANDONED }

/**
 * What became of a send: [inlining] `inlined` or `declined: <reason>` by the
 * last optimizing compile that planned it, and [cache] its inline cache's
 * state -- `mono`, `poly <n>` with the receivers it has seen since it was
 * last vacant, `mega` -- each null where the trace says nothing of it.
 */
data class SendOutcome(val inlining: String?, val cache: String?)

/**
 * One traced run, [name]d by its configuration and [started] at a time
 * (decision 26): the records [TraceFileReader] read, and what the editor and
 * the tool window draw from them. Read and changed on the EDT only.
 *
 * A body number may come without a [Body]: a body no mixin holds, or one
 * whose range lies outside its module's text. Such a body has a state and no
 * sends, since only a description lists them.
 */
class TraceRun(val name: String, val started: Instant) {
    private val describedBodies = HashMap<Int, Body>()
    private val recordedEvents = ArrayList<Event>()
    private val bytecodeListings = HashMap<Int, Bytecode>()
    private val codeListings = HashMap<Int, MutableList<Code>>()
    private val inliningPlans = HashMap<Int, MutableList<Inlining>>()

    /** The root of each block's family, from the bytecode listings, which list a method's blocks after it. */
    private val familyRoots = HashMap<Int, Int>()
    private val histories = HashMap<Int, BodyHistory>()
    private val inliningAtSends = HashMap<SendKey, String>()
    private val caches = HashMap<SendKey, CacheHistory>()
    private val listeners = ArrayList<() -> Unit>()

    /** The architecture the run's machine code is instructions of, from the header; null without a JIT or before it arrived. */
    var architecture: String? = null
        private set

    /** Every body described, by its number. */
    val bodies: Map<Int, Body> get() = describedBodies

    /** Every event, in the file's order. */
    val events: List<Event> get() = recordedEvents

    /** Each body's bytecode, by its root's number. */
    val bytecode: Map<Int, Bytecode> get() = bytecodeListings

    /** Each body's installs of machine code, in their order. */
    val code: Map<Int, List<Code>> get() = codeListings

    /** Each body's optimizing compiles' plans, in their order. */
    val inlining: Map<Int, List<Inlining>> get() = inliningPlans

    /** [body]'s state after its events in their order; [MethodState.INTERPRETED] for one without any. */
    fun state(body: Int): MethodState = histories[body]?.state ?: MethodState.INTERPRETED

    /** How many optimized copies of [body] are installed. */
    fun copies(body: Int): Int = histories[body]?.copies ?: 0

    /** What became of each send of [body] the trace says anything of, by the send's bytecode index. */
    fun sends(body: Int): Map<Int, SendOutcome> {
        val described = describedBodies[body] ?: return emptyMap()
        // A block's sites are numbered in its method's family, whose root the transitions name.
        val root = familyRoots[body] ?: body
        val outcomes = LinkedHashMap<Int, SendOutcome>()
        for (send in described.sends) {
            val inlining = inliningAtSends[SendKey(body, send.at)]
            val cache = send.site?.let { site -> caches[SendKey(root, site)]?.shortForm() }
            if (inlining != null || cache != null) outcomes[send.at] = SendOutcome(inlining, cache)
        }
        return outcomes
    }

    /** Takes [batch], in the file's order, and tells the listeners once. */
    fun add(batch: List<TraceRecord>) {
        if (batch.isEmpty()) return
        for (record in batch) {
            when (record) {
                is Header -> architecture = record.architecture
                is Body -> describedBodies[record.number] = record
                is Event -> take(record)
                is Bytecode -> {
                    bytecodeListings[record.body] = record
                    for (listed in record.bodies) if (listed.body != record.body) familyRoots[listed.body] = record.body
                }
                is Code -> codeListings.getOrPut(record.body) { ArrayList() }.add(record)
                is Inlining -> inliningPlans.getOrPut(record.body) { ArrayList() }.add(record)
            }
        }
        for (listener in listeners.toList()) listener()
    }

    /** Calls [listener] after every batch taken, until [parent] is disposed. */
    fun onChanged(parent: Disposable, listener: () -> Unit) {
        listeners.add(listener)
        Disposer.register(parent) { listeners.remove(listener) }
    }

    private fun take(event: Event) {
        recordedEvents.add(event)
        val body = event.body
        when (event.event) {
            "inlined" -> inliningAt(event)?.let { key -> inliningAtSends[key] = "inlined" }
            "declined" -> inliningAt(event)?.let { key -> inliningAtSends[key] = "declined: ${event.text("reason")}" }
            "transition" -> {
                val site = event.integer("site")
                if (body != null && site != null) caches.getOrPut(SendKey(body, site)) { CacheHistory() }.take(event)
            }
            else -> if (body != null) histories.getOrPut(body) { BodyHistory() }.take(event)
        }
    }

    private fun inliningAt(event: Event): SendKey? {
        val inBody = event.integer("in-body") ?: return null
        val at = event.integer("at") ?: return null
        return SendKey(inBody, at)
    }

    /** A send by its body and index, or a cache site by its family's root and number. */
    private data class SendKey(val body: Int, val position: Int)

    /** A body's tiers as its events tell them. */
    private class BodyHistory {
        private var tier = MethodState.INTERPRETED
        private var abandoned = false

        var copies = 0
            private set

        val state get() = if (abandoned) MethodState.ABANDONED else tier

        fun take(event: Event) {
            when (event.event) {
                "compiled" -> if (copies == 0) tier = MethodState.BASELINE
                "compiled-twin" -> if (copies == 0) tier = MethodState.TWIN
                "optimized" -> {
                    copies++
                    tier = MethodState.OPTIMIZED
                }
                "retired", "invalidated" -> uncount()
                "freed" -> if (event.text("tier") == "optimized") uncount()
                "abandoned" -> abandoned = true
                "revived" -> {
                    abandoned = false
                    if (copies == 0) tier = MethodState.BASELINE
                }
            }
        }

        /** A copy gone; the last one leaves the body deoptimized. */
        private fun uncount() {
            // A copy the trace never saw installed -- its event filtered out -- uncounts nothing.
            if (copies == 0) return
            copies--
            if (copies == 0) tier = MethodState.DEOPTIMIZED
        }
    }

    /** A cache site's last state and the receivers it has seen since it was last vacant. */
    private class CacheHistory {
        private var after: String? = null
        private val receivers = LinkedHashSet<String>()

        fun take(event: Event) {
            if (event.text("before") == "vacant") receivers.clear()
            event.text("receiver")?.let { receiver -> receivers.add(receiver) }
            after = event.text("after")
        }

        fun shortForm(): String? = when (after) {
            "monomorphic" -> "mono"
            "polymorphic" -> "poly ${receivers.size}"
            "megamorphic" -> "mega"
            else -> after
        }
    }
}
