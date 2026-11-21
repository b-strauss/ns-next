package io.github.bstrauss.newspeak.devtool

/**
 * What a watched pattern names a module by, read with the grammar
 * `ns-next run`'s trace filter reads it (`trace_events/filter.rs`,
 * `TraceFilter::parse`; `tool_names.rs`'s class, side and method forms),
 * so that a run is never handed a pattern it refuses: `ns-next run` exits
 * with status 2 for a pattern not of the form, and for one whose module
 * no module of the program or the binary has.
 */
sealed interface PatternModule {
    /** A module named whole, with a version or none: a method, a class's selectors, a class path. */
    data class Named(val id: String, val version: String?) : PatternModule

    /** The versions of one id that begin with a prefix: `com.acme.Util@1....`. */
    data class Versions(val id: String) : PatternModule

    /** The ids that begin with a prefix: `Demo...`, `com.acme....`. */
    data class IdPrefix(val prefix: String) : PatternModule
}

/** What [pattern] names its module by; null for a pattern the runtime refuses as not of the form. */
fun patternModule(pattern: String): PatternModule? {
    val text = pattern.trim()
    if (text.isEmpty()) return null
    if (!text.endsWith(WatchedMethods.PREFIX_MARK)) {
        if (WatchedMethods.PREFIX_MARK in text) return null
        return methodForm(text)
    }
    val stem = text.removeSuffix(WatchedMethods.PREFIX_MARK)
    if (WatchedMethods.PREFIX_MARK in stem) return null
    if (">>" in stem) {
        val (side, prefix) = stem.split(">>", limit = 2)
        // A selector holds no `.`, so a prefix with one selects nothing.
        if (prefix.any(Char::isWhitespace) || '.' in prefix) return null
        return sideForm(side)
    }
    val trimmed = stem.trimEnd()
    if (trimmed.endsWith(" class")) return sideForm(trimmed)
    if (stem.isEmpty() || stem.any(Char::isWhitespace)) return null
    if ('/' in stem) {
        val module = classForm(stem.substringBefore('/')) ?: return null
        val path = stem.substringAfter('/').split('.')
        if (!path.dropLast(1).all(::isIdentifier) || !isIdentifierPrefix(path.last())) return null
        return module
    }
    if ('@' in stem) {
        val id = stem.substringBefore('@')
        val prefix = stem.substringAfter('@')
        if (!id.split('.').all(::isIdentifier) || !prefix.all { character -> character.isAsciiLetterOrDigit() || character == '.' || character == '-' }) return null
        return PatternModule.Versions(id)
    }
    val segments = stem.split('.')
    if (!segments.dropLast(1).all(::isIdentifier) || !isIdentifierPrefix(segments.last())) return null
    return PatternModule.IdPrefix(stem)
}

/**
 * Whether a run whose program and binary have [modules] -- each id with
 * its versions -- takes [pattern]: of the form, and naming one of them as
 * the runtime's check does, an id exactly and a version the id has. An
 * id prefix, which the runtime never refuses, counts only where it begins
 * one of them, since it would trace nothing else.
 */
fun runTakes(pattern: String, modules: Map<String, Set<String>>): Boolean = when (val module = patternModule(pattern)) {
    null -> false
    is PatternModule.Named -> modules[module.id]?.let { versions -> module.version == null || module.version in versions } ?: false
    is PatternModule.Versions -> module.id in modules
    is PatternModule.IdPrefix -> modules.keys.any { id -> id.startsWith(module.prefix) }
}

/**
 * The ids of the modules ns-next ships, each declared by the `:module:`
 * of its source under `crates/ns-runtime/core-library/`, none with a
 * version; `WatchPatternsTest` holds the list to those sources.
 */
val SHIPPED_MODULE_IDS: List<String> = listOf(
    "newspeak.core.Kernel",
    "newspeak.core.Manifest",
    "newspeak.core.PlatformFactory",
    "newspeak.core.CollectionsFactory",
    "newspeak.core.MirrorsFactory",
    "newspeak.core.TimeFactory",
    "newspeak.core.TranscriptFactory",
    "newspeak.core.TypedArraysFactory",
)

/** `<class>>><selector>` or `<class> class>><selector>` (`tool_names::method_form`). */
private fun methodForm(text: String): PatternModule? {
    if (">>" !in text) return null
    val (side, selector) = text.split(">>", limit = 2)
    val trimmed = selector.trim()
    if (trimmed.isEmpty() || trimmed.any(Char::isWhitespace)) return null
    return sideForm(side)
}

/** A class followed by ` class` for the class side, or not (`tool_names::side_form`). */
private fun sideForm(text: String): PatternModule? {
    val trimmed = text.trim()
    val rest = trimmed.removeSuffix("class")
    val classPart = if (rest.length < trimmed.length && rest.isNotEmpty() && rest.last().isWhitespace()) rest.trimEnd() else trimmed
    return classForm(classPart)
}

/** `<id>[@<version>][/<Class>(.<Class>)*]` (`tool_names::class_form`). */
private fun classForm(text: String): PatternModule.Named? {
    val trimmed = text.trim()
    val module = trimmed.substringBefore('/')
    if ('/' in trimmed && !trimmed.substringAfter('/').split('.').all(::isIdentifier)) return null
    val id = module.substringBefore('@')
    if (!id.split('.').all(::isIdentifier)) return null
    val version = if ('@' in module) module.substringAfter('@') else null
    if (version != null && !isVersion(version)) return null
    return PatternModule.Named(id, version)
}

/** An identifier of a module id or a class path (`modules::is_identifier`). */
private fun isIdentifier(segment: String): Boolean = segment.isNotEmpty() &&
    (segment[0].isAsciiLetter() || segment[0] == '_') &&
    segment.all { character -> character.isAsciiLetterOrDigit() || character == '_' }

private fun isIdentifierPrefix(text: String): Boolean = text.isEmpty() || isIdentifier(text)

/** A semver version without build metadata (`modules::Version::parse`). */
private fun isVersion(text: String): Boolean {
    if ('+' in text) return false
    val core = text.substringBefore('-')
    val numbers = core.split('.')
    if (numbers.size != 3 || !numbers.all(::isVersionNumber)) return false
    if ('-' !in text) return true
    return text.substringAfter('-').split('.').all { part ->
        part.isNotEmpty() &&
            part.all { character -> character.isAsciiLetterOrDigit() || character == '-' } &&
            (!part.all(Char::isAsciiDigit) || isVersionNumber(part))
    }
}

/** Digits without a leading zero that fit in 64 unsigned bits. */
private fun isVersionNumber(text: String): Boolean =
    text.isNotEmpty() && text.all(Char::isAsciiDigit) && !(text.length > 1 && text[0] == '0') && text.toULongOrNull() != null

private fun Char.isAsciiDigit(): Boolean = this in '0'..'9'

private fun Char.isAsciiLetter(): Boolean = this in 'a'..'z' || this in 'A'..'Z'

private fun Char.isAsciiLetterOrDigit(): Boolean = isAsciiLetter() || isAsciiDigit()
