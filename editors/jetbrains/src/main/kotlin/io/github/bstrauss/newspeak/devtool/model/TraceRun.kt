package io.github.bstrauss.newspeak.devtool.model

import com.intellij.openapi.Disposable
import com.intellij.openapi.util.Disposer
import com.google.gson.JsonObject
import io.github.bstrauss.newspeak.devtool.TraceFilters
import io.github.bstrauss.newspeak.devtool.covers
import java.time.Instant

/** How a method ran, as its gutter icon says (`docs/design/developer-tool.md`, decision 27). */
enum class MethodState { INTERPRETED, BASELINE, TWIN, OPTIMIZED, DEOPTIMIZED, ABANDONED }

/**
 * What became of a send: [inlining] `inlined` or `declined: <reason>` by the
 * last optimizing compile that planned it and whose installed copy still
 * lives, in a run whose header does not leave `deoptimize` out, and
 * [cache] its inline cache's
 * state -- `mono`, `poly <n>` with the receivers it has seen since it was
 * last vacant, `mega` -- each null where the trace says nothing of it.
 */
data class SendOutcome(val inlining: String?, val cache: String?)

/**
 * One traced run, [name]d by its configuration and [started] at a time
 * (decision 26), narrowed by the [filters] it was started with: the records
 * [TraceFileReader] read, and what the editor and the tool window draw from
 * them. Read and changed on the EDT only.
 *
 * A body number may come without a [Body]: a body no mixin holds, or one
 * whose range lies outside its module's text. Such a body may have a state
 * and has no sends, since only a description lists them.
 */
class TraceRun(val name: String, val started: Instant, private val filters: TraceFilters = TraceFilters.EVERYTHING) {
    private val describedBodies = HashMap<Int, Body>()
    private val recordedEvents = ArrayList<Event>()
    private val bytecodeListings = HashMap<Int, Bytecode>()
    private val codeListings = HashMap<Int, MutableList<Code>>()
    private val inliningPlans = HashMap<Int, MutableList<Inlining>>()
    private val plansInOrder = ArrayList<Inlining>()
    private val histories = HashMap<Int, BodyHistory>()
    private val inliningAtSends = HashMap<SendKey, String>()

    /**
     * The plan's decisions of each installed optimized copy that still
     * lives, the copy installed last at the end: a send's inlining is that
     * of the last of them that planned it, so that a copy retired,
     * invalidated, freed or lost with the zone takes its decisions with it.
     */
    private val liveCopies = LinkedHashMap<CompileKey, List<Pair<SendKey, String>>>()
    private val caches = HashMap<SendKey, CacheHistory>()

    /**
     * The plan's decisions of each optimizing compile whose outcome has not
     * arrived yet: a compile's `inlined` and `declined` events precede its
     * `optimized`, `refused` or `deferred` event, and only an installed
     * copy's plan says what became of a send.
     */
    private val plannedInlining = HashMap<CompileKey, MutableList<Pair<SendKey, String>>>()

    /**
     * Whether a compile's outcome can arrive: an `optimize` event or an
     * inlining listing. Where the header says neither is traced, a plan's
     * decisions are taken as they come, for nothing would ever conclude
     * them; a header that does not say is taken to trace them.
     */
    private var outcomeCanArrive = true

    /** The categories the header says are traced; null before it arrived and for a header that does not say. */
    private var categories: List<String>? = null
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

    /** Every optimizing compile's plan, in the file's order. */
    val plans: List<Inlining> get() = plansInOrder

    /**
     * [body]'s state after its events in their order -- its listed machine
     * code counted among them where no event announced it but a
     * deoptimization would be heard of --, and otherwise
     * [MethodState.INTERPRETED] only where a compile would have been heard
     * of -- `compile` traced and the body's own events passing the run's
     * filter. Null where the trace knows nothing of how it ran: a body named
     * only as a callee, a scope, a cache's family or by a bytecode listing,
     * and one whose code was listed in a run that would never hear its copy
     * retired.
     */
    fun state(body: Int): MethodState? = histories[body]?.state ?: MethodState.INTERPRETED.takeIf { heardOf(body, "compile") }

    /** How many optimized copies of [body] are installed, as its events -- and its listings, where they count -- say. */
    fun copies(body: Int): Int = histories[body]?.copies ?: 0

    /**
     * Whether an event of [category] about [body] would have reached the
     * run: the category traced -- a header that does not say is taken to
     * trace every one -- and the body's name passing the events filter.
     */
    private fun heardOf(body: Int, category: String): Boolean {
        if (filters.noEvents) return false
        if (categories?.contains(category) == false) return false
        if (filters.events.isEmpty()) return true
        val name = describedBodies[body]?.name ?: return false
        return filters.events.any { pattern -> covers(pattern, name) }
    }

    /**
     * Counts [code]'s install in its body's history where no event
     * announced it -- `compile` or `optimize` not heard of for the body --
     * and only where its end can be heard, a retirement or invalidation in
     * `deoptimize`: an install nothing would ever take back would stand
     * for good.
     */
    private fun countListed(code: Code) {
        val announcing = if (code.tier == "optimized") "optimize" else "compile"
        if (heardOf(code.body, announcing) || !heardOf(code.body, "deoptimize")) return
        val event = when (code.tier) {
            "optimized" -> "optimized"
            "twin" -> "compiled-twin"
            else -> "compiled"
        }
        histories.getOrPut(code.body) { BodyHistory() }.take(Event("compile", code.body, event, JsonObject()))
    }

    /** What became of each send of [body] the trace says anything of, by the send's bytecode index. */
    fun sends(body: Int): Map<Int, SendOutcome> {
        val described = describedBodies[body] ?: return emptyMap()
        // A block's sites are numbered in its method's family, whose root the transitions name.
        val root = described.family ?: body
        val outcomes = LinkedHashMap<Int, SendOutcome>()
        // A decision whose copy's retirement could never arrive would stand for good: none is shown.
        val inliningEnds = categories?.contains("deoptimize") != false
        for (send in described.sends) {
            val inlining = inliningAtSends[SendKey(body, send.at)]?.takeIf { inliningEnds }
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
                is Header -> {
                    architecture = record.architecture
                    val traced = record.categories
                    val listings = record.listings
                    categories = traced
                    outcomeCanArrive = traced == null || listings == null || "optimize" in traced || "inlining" in listings
                }
                is Body -> describedBodies[record.number] = record
                is Event -> take(record)
                is Bytecode -> bytecodeListings[record.body] = record
                is Code -> {
                    codeListings.getOrPut(record.body) { ArrayList() }.add(record)
                    countListed(record)
                }
                is Inlining -> {
                    inliningPlans.getOrPut(record.body) { ArrayList() }.add(record)
                    plansInOrder.add(record)
                    // The plan's own outcome, for a trace without the `optimize` category.
                    concludePlan(CompileKey(record.body, record.key, record.entered), installed = record.installed != null)
                }
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
            "inlined" -> plan(event, "inlined")
            "declined" -> plan(event, "declined: ${event.text("reason")}")
            "transition" -> {
                val site = event.integer("site")
                if (body != null && site != null) caches.getOrPut(SendKey(body, site)) { CacheHistory() }.take(event)
            }
            // Every site vacated at once: none keeps the state or the receivers it had.
            "vacated" -> caches.clear()
            // The zone dropped every body's machine code, copies included.
            "replaced" -> {
                for (history in histories.values) history.lose()
                liveCopies.clear()
                inliningAtSends.clear()
            }
            else -> if (body != null) {
                histories.getOrPut(body) { BodyHistory() }.take(event)
                when (event.event) {
                    "optimized" -> concludePlan(compileKey(body, event), installed = true)
                    "refused", "deferred" -> if (event.category == "optimize") concludePlan(compileKey(body, event), installed = false)
                    "retired", "invalidated" -> copyGone(compileKey(body, event))
                    "freed" -> if (event.text("tier") == "optimized") copyGone(compileKey(body, event))
                }
            }
        }
    }

    private fun compileKey(body: Int, event: Event) = CompileKey(body, event.text("key"), event.text("entered"))

    /** Keeps [event]'s decision at its send until its compile's outcome says whether the copy was installed. */
    private fun plan(event: Event, outcome: String) {
        val body = event.body ?: return
        val send = inliningAt(event) ?: return
        val key = compileKey(body, event)
        if (!outcomeCanArrive) {
            // Nothing would conclude the compile: its decisions are its copy's at once.
            liveCopies[key] = liveCopies[key].orEmpty() + (send to outcome)
            inliningAtSends[send] = outcome
            return
        }
        plannedInlining.getOrPut(key) { ArrayList() }.add(send to outcome)
    }

    /** The compile [key] ended: its decisions apply to the sends if its copy was [installed], and are dropped otherwise. */
    private fun concludePlan(key: CompileKey, installed: Boolean) {
        val decisions = plannedInlining.remove(key) ?: return
        if (!installed) return
        // A key's new copy replaces its old one and is the one installed last.
        liveCopies.remove(key)
        liveCopies[key] = decisions
        for ((send, outcome) in decisions) inliningAtSends[send] = outcome
    }

    /** The copy [key] is gone: each send it planned keeps the decision of the last live copy that planned it too, or none. */
    private fun copyGone(key: CompileKey) {
        liveCopies.remove(key) ?: return
        inliningAtSends.clear()
        for (decisions in liveCopies.values) for ((send, outcome) in decisions) inliningAtSends[send] = outcome
    }

    private fun inliningAt(event: Event): SendKey? {
        val inBody = event.integer("in-body") ?: return null
        val at = event.integer("at") ?: return null
        return SendKey(inBody, at)
    }

    /** A send by its body and index, or a cache site by its family's root and number. */
    private data class SendKey(val body: Int, val position: Int)

    /** An optimizing compile: its body and the copy's key and entered application. */
    private data class CompileKey(val body: Int, val key: String?, val entered: String?)

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
                "freed" -> when (event.text("tier")) {
                    "optimized" -> uncount()
                    // Its baseline code gone, the body runs interpreted until it is compiled again.
                    else -> if (copies == 0) tier = MethodState.INTERPRETED
                }
                "abandoned" -> abandoned = true
                // Revived, it counts its entries again, interpreted until compiled.
                "revived" -> {
                    abandoned = false
                    if (copies == 0) tier = MethodState.INTERPRETED
                }
            }
        }

        /** Every machine code of the body dropped at once. */
        fun lose() {
            copies = 0
            tier = MethodState.INTERPRETED
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
