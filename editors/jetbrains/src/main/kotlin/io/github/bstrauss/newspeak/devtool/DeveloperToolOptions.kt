package io.github.bstrauss.newspeak.devtool

/** The names of `ns-next run --trace`'s seven categories, in the runtime's order (`trace_events.rs`). */
val TRACE_CATEGORIES: List<String> = listOf("compile", "osr", "optimize", "inlining", "deoptimize", "code-zone", "caches")

/**
 * What a run is given beyond the categories and listing kinds: the patterns
 * that narrow the events (`--trace-filter`) and the listings
 * (`--print-filter`), and whether either kind is switched off for this run.
 * An empty list is no filter: every event, every listing.
 */
data class TraceFilters(
    val events: List<String>,
    val listings: List<String>,
    val noEvents: Boolean,
    val noListings: Boolean,
) {
    companion object {
        val EVERYTHING = TraceFilters(emptyList(), emptyList(), noEvents = false, noListings = false)
    }
}

/** The options of ns-next's own that the Developer Tool tab owns; the free field of the first tab may not hold them. */
val DEVELOPER_TOOL_OPTION_PREFIXES: List<String> = listOf("--trace", "--print-")
