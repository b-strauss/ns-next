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
 * lives, in a run whose header leaves neither `deoptimize` nor
 * `code-zone` out, so that the copy's end would be heard, and
 * [cache] its inline cache's
 * state -- `mono`, `poly <n>` with the receivers it has seen since it was
 * last vacant, `mega` -- each null where the trace says nothing of it.
 */
data class SendOutcome(val inlining: String?, val cache: String?)

/**
 * The most events a run keeps as such, the oldest dropped first: a run that
 * traces for long writes events without end, and only the Events tab
 * shows them one by one. What they say of a body's tiers, its sends, its
 * caches and its counts is kept beside them for the run's length, so that
 * no state, hint or count changes when an event is dropped.
 */
const val KEPT_EVENTS = 200_000

/** What a body's events counted: the Methods tab's columns and the gutter icon's tooltip. */
class EventTally {
    /** Its copies retired or invalidated, every `deoptimize` event. */
    var deoptimizations = 0
        internal set

    /** Its code abandoned for its exits. */
    var abandonments = 0
        internal set

    /** Callees an optimizing compile of it declined to inline. */
    var declines = 0
        internal set

    /** The bytes of its largest optimized copy an `optimized` event announced; 0 for none. */
    var largestOptimized = 0
        internal set

    /** Its last `deoptimize` event and its place among the run's events; null before any. */
    var lastDeoptimization: Event? = null
        internal set

    var lastDeoptimizationOrder = -1L
        internal set
}

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
    private val recordedEvents = ArrayDeque<Event>()
    private val eventTallies = HashMap<Int, EventTally>()
    private val transitionEvents = HashMap<Pair<Int, Int>, Event>()
    private val bytecodeListings = HashMap<Int, Bytecode>()
    private val codeListings = HashMap<Int, MutableList<Code>>()
    private val inliningPlans = HashMap<Int, MutableList<Inlining>>()
    private val plansInOrder = ArrayList<Inlining>()
    private val histories = HashMap<Int, BodyHistory>()
    private val inliningAtSends = HashMap<SendKey, Decision>()

    /**
     * The plan's decisions of each installed optimized copy that still
     * lives, the copy installed last at the end: a send's inlining is that
     * of the last of them that planned it, so that a copy retired,
     * invalidated, freed or lost with the zone takes its decisions with it.
     */
    private val liveCopies = LinkedHashMap<CompileKey, List<Decision>>()
    private val caches = HashMap<SendKey, CacheHistory>()

    /**
     * The optimizing compile whose outcome has not arrived yet, and the
     * plan's decisions its `inlined` and `declined` events made: only an
     * installed copy's plan says what became of a send. A compile runs
     * whole at one safepoint, its inlining events one stretch of the file
     * followed by its outcome -- its `optimized`, `refused` or `deferred`
     * event, then its plan's record -- so any other record but a body's
     * description or a bytecode listing, which the file places wherever a
     * body is first named, ends the stretch: a compile whose outcome the
     * events filter, the listings filter or a plan the runtime could not
     * name (`jit/mod.rs`, `record_plan`) withheld is dropped there, its
     * decisions never shown, since whether its copy was installed is
     * never heard. So at most one compile's decisions wait.
     */
    private var openCompile: CompileKey? = null
    private var openDecisions = ArrayList<Decision>()

    /** How many decisions wait for their compile's outcome. */
    internal val waitingDecisions: Int get() = openDecisions.size

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

    /**
     * The last [KEPT_EVENTS] events, in the file's order: the Events tab's
     * rows. What the run says of a body, a send or a cache is kept apart
     * from them, for the run's length.
     */
    val events: List<Event> get() = recordedEvents

    /** How many events were taken in all; the first of [events] is the one taken at `eventCount - events.size`. */
    var eventCount = 0L
        private set

    /** How many of the earliest events [events] no longer holds. */
    val droppedEvents: Long get() = eventCount - recordedEvents.size

    /** What the events said of each body, by the number of the body they are about. */
    val tallies: Map<Int, EventTally> get() = eventTallies

    /** The last cache transition at each site, by its family's root and the site's number. */
    val lastTransition: Map<Pair<Int, Int>, Event> get() = transitionEvents

    /** Each body's bytecode, by its root's number. */
    val bytecode: Map<Int, Bytecode> get() = bytecodeListings

    /** Each body's installs of machine code, in their order. */
    val code: Map<Int, List<Code>> get() = codeListings

    /** Each body's optimizing compiles' plans, in their order. */
    val inlining: Map<Int, List<Inlining>> get() = inliningPlans

    /** Every optimizing compile's plan, in the file's order. */
    val plans: List<Inlining> get() = plansInOrder

    /**
     * [body]'s state after its events in their order, its listed machine
     * code counted among them where no event announced it -- and a tier
     * only where its end can be heard, never one that may be over:
     * optimized where `deoptimize` (a retirement, an invalidation) and
     * `code-zone` (a copy freed, the zone replaced) are, baseline and twin
     * where `compile` (an abandonment) and `code-zone` (the code freed, the
     * zone replaced) are, interpreted where `compile` is, each for the
     * body's own events passing the run's filter; deoptimized and
     * abandoned as they are heard. An optimized body whose copies' end
     * cannot be heard answers its baseline tier, if that one's can. Null
     * where the trace cannot say how it runs: a body named only as a
     * callee, a scope, a cache's family or by a bytecode listing, and one
     * whose every tier may be over.
     */
    fun state(body: Int): MethodState? {
        val history = histories[body] ?: return MethodState.INTERPRETED.takeIf { heardOf(body, "compile") }
        return when (val state = history.state) {
            MethodState.ABANDONED, MethodState.DEOPTIMIZED -> state
            MethodState.OPTIMIZED -> if (copiesEndHeard(body)) state else baseState(body, history.base)
            else -> baseState(body, state)
        }
    }

    /** How many optimized copies of [body] are installed, as its events and listings say; 0 where their end cannot be heard. */
    fun copies(body: Int): Int = histories[body]?.copies?.takeIf { copiesEndHeard(body) } ?: 0

    /** Whether the end of [body]'s optimized copies would be heard: a retirement or an invalidation, a free or the zone's replacement. */
    private fun copiesEndHeard(body: Int): Boolean = heardOf(body, "deoptimize") && heardOf(body, "code-zone")

    /** [tier], interpreted, baseline or twin, where its end would be heard -- a compile, or an abandonment and a free -- and null otherwise. */
    private fun baseState(body: Int, tier: MethodState): MethodState? = when (tier) {
        MethodState.INTERPRETED -> tier.takeIf { heardOf(body, "compile") }
        else -> tier.takeIf { heardOf(body, "compile") && heardOf(body, "code-zone") }
    }

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
     * announced it -- `compile` or `optimize` not heard of for the body --;
     * [state] shows the tier only where its end can be heard.
     */
    private fun countListed(code: Code) {
        val announcing = if (code.tier == "optimized") "optimize" else "compile"
        if (heardOf(code.body, announcing)) return
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
        for (send in described.sends) {
            val inlining = inliningAtSends[SendKey(body, send.at)]?.outcome?.takeIf { inliningEnds() }
            val cache = send.site?.let { site -> caches[SendKey(root, site)]?.shortForm() }
            if (inlining != null || cache != null) outcomes[send.at] = SendOutcome(inlining, cache)
        }
        return outcomes
    }

    /**
     * The `inlined` or `declined` event behind the inlining [sends] shows
     * at the send [at] of [body]: a live installed copy's decision, never a
     * refused, deferred or dropped compile's; null where [sends] shows none.
     */
    fun inliningEvent(body: Int, at: Int): Event? = inliningAtSends[SendKey(body, at)]?.event?.takeIf { inliningEnds() }

    /** A decision whose copy's end -- a retirement, an invalidation, a free, the zone replaced -- could never arrive would stand for good: none is shown. */
    private fun inliningEnds(): Boolean = categories?.let { traced -> "deoptimize" in traced && "code-zone" in traced } != false

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
                    leaveOpenCompile()
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

    /** The file was read to its end: a compile still waiting for its outcome never hears it, and its decisions are dropped. */
    fun ended() = leaveOpenCompile()

    /** Calls [listener] after every batch taken, until [parent] is disposed. */
    fun onChanged(parent: Disposable, listener: () -> Unit) {
        listeners.add(listener)
        Disposer.register(parent) { listeners.remove(listener) }
    }

    private fun take(event: Event) {
        recordedEvents.addLast(event)
        if (recordedEvents.size > KEPT_EVENTS) recordedEvents.removeFirst()
        val order = eventCount++
        tally(event, order)
        val body = event.body
        if (event.category == INLINING) {
            // A compile's inlining events, whatever their kind, belong to its stretch.
            if (body != null) continueCompile(compileKey(body, event))
            when (event.event) {
                "inlined" -> plan(event, "inlined")
                "declined" -> plan(event, "declined: ${event.text("reason")}")
            }
            return
        }
        val concluded = body != null && event.category == "optimize" && event.event in OUTCOMES
        if (!concluded) leaveOpenCompile()
        when (event.event) {
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
                if (concluded) concludePlan(compileKey(body, event), installed = event.event == "optimized")
                when (event.event) {
                    "retired", "invalidated" -> copyGone(compileKey(body, event))
                    "freed" -> if (event.text("tier") == "optimized") copyGone(compileKey(body, event))
                }
            }
        }
    }

    /** Counts [event], the [order]th taken, into what the tabs and the tooltips read, which [events] may drop. */
    private fun tally(event: Event, order: Long) {
        val body = event.body
        if (event.event == "transition") {
            val site = event.integer("site")
            if (body != null && site != null) transitionEvents[body to site] = event
        }
        if (body == null) return
        val tally = eventTallies.getOrPut(body) { EventTally() }
        if (event.category == "deoptimize") {
            tally.deoptimizations++
            tally.lastDeoptimization = event
            tally.lastDeoptimizationOrder = order
        }
        when (event.event) {
            "abandoned" -> tally.abandonments++
            "declined" -> tally.declines++
            "optimized" -> event.integer("bytes")?.let { bytes -> tally.largestOptimized = maxOf(tally.largestOptimized, bytes) }
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
            val decision = Decision(send, outcome, event)
            liveCopies[key] = liveCopies[key].orEmpty() + decision
            inliningAtSends[send] = decision
            return
        }
        openDecisions.add(Decision(send, outcome, event))
    }

    /** An inlining event of the compile [key]: the open compile's, or the first of a new one, which ends the open one unheard. */
    private fun continueCompile(key: CompileKey) {
        if (openCompile == key) return
        leaveOpenCompile()
        openCompile = key
    }

    /** A record of no compile's stretch arrived: the open compile's outcome was withheld, and its decisions are dropped. */
    private fun leaveOpenCompile() {
        openCompile = null
        if (openDecisions.isNotEmpty()) openDecisions = ArrayList()
    }

    /** The compile [key] ended: its decisions apply to the sends if its copy was [installed], and are dropped otherwise. */
    private fun concludePlan(key: CompileKey, installed: Boolean) {
        val open = openCompile
        val decisions = openDecisions
        leaveOpenCompile()
        // Another compile's outcome: the open one's was withheld.
        if (open != key || decisions.isEmpty()) return
        if (!installed) return
        // A key's new copy replaces its old one and is the one installed last.
        liveCopies.remove(key)
        liveCopies[key] = decisions
        for (decision in decisions) inliningAtSends[decision.send] = decision
    }

    /** The copy [key] is gone: each send it planned keeps the decision of the last live copy that planned it too, or none. */
    private fun copyGone(key: CompileKey) {
        liveCopies.remove(key) ?: return
        inliningAtSends.clear()
        for (decisions in liveCopies.values) for (decision in decisions) inliningAtSends[decision.send] = decision
    }

    private fun inliningAt(event: Event): SendKey? {
        val inBody = event.integer("in-body") ?: return null
        val at = event.integer("at") ?: return null
        return SendKey(inBody, at)
    }

    private companion object {
        const val INLINING = "inlining"

        /** The `optimize` events that end a compile: its copy installed, refused or put off. */
        val OUTCOMES = setOf("optimized", "refused", "deferred")
    }

    /** A plan's decision at [send]: its [outcome] as the hint writes it, and the [event] that made it. */
    private class Decision(val send: SendKey, val outcome: String, val event: Event)

    /** A send by its body and index, or a cache site by its family's root and number. */
    private data class SendKey(val body: Int, val position: Int)

    /** An optimizing compile: its body and the copy's key and entered application. */
    private data class CompileKey(val body: Int, val key: String?, val entered: String?)

    /** A body's tiers as its events tell them. */
    private class BodyHistory {
        private var tier = MethodState.INTERPRETED
        private var abandoned = false

        /** The tier of the body's baseline code beside its copies: interpreted, baseline or twin. */
        var base = MethodState.INTERPRETED
            private set

        var copies = 0
            private set

        val state get() = if (abandoned) MethodState.ABANDONED else tier

        fun take(event: Event) {
            when (event.event) {
                "compiled" -> {
                    base = MethodState.BASELINE
                    if (copies == 0) tier = MethodState.BASELINE
                }
                "compiled-twin" -> {
                    base = MethodState.TWIN
                    if (copies == 0) tier = MethodState.TWIN
                }
                "optimized" -> {
                    copies++
                    tier = MethodState.OPTIMIZED
                }
                "retired", "invalidated" -> uncount()
                "freed" -> when (event.text("tier")) {
                    "optimized" -> uncount()
                    // Its baseline code gone, the body runs interpreted until it is compiled again.
                    else -> {
                        base = MethodState.INTERPRETED
                        if (copies == 0) tier = MethodState.INTERPRETED
                    }
                }
                "abandoned" -> abandoned = true
                // Revived, it counts its entries again, interpreted until compiled.
                "revived" -> {
                    abandoned = false
                    base = MethodState.INTERPRETED
                    if (copies == 0) tier = MethodState.INTERPRETED
                }
            }
        }

        /** Every machine code of the body dropped at once. */
        fun lose() {
            copies = 0
            base = MethodState.INTERPRETED
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
