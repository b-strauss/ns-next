package io.github.bstrauss.newspeak.editor

import com.intellij.lang.documentation.DocumentationMarkup
import io.github.bstrauss.newspeak.NewspeakBundle

/**
 * One `@name …` tag of a doc comment: `name` without its `@`, `argument`
 * the first word after it for `param` (the parameter's name) and `signals`
 * (the error's class), and `text` the rest, its lines joined with `\n`.
 */
data class DocTag(val name: String, val argument: String?, val text: String)

/**
 * A doc comment read as documentation: the description (the lines before the
 * first tag, an inline `{@summary …}` replaced by its text, then each
 * unknown tag verbatim as a paragraph of its own, so a misspelt tag stays
 * visible rather than vanishing), its summary, and its tags in source order,
 * unknown ones among them.
 */
data class DocComment(val description: String, val summary: String?, val tags: List<DocTag>)

/**
 * The grammar of a doc comment's tags, the one `ns-syntax` reads
 * (`crates/ns-syntax/src/doc_comment.rs`), so that the rendered comment in the
 * editor and the server's hover show the same sections; a rule changed there
 * changes here.
 */
object NewspeakDocTags {
    private const val SUMMARY_OPENING = "{@summary "
    private const val FENCE = "```"
    private val ARGUMENT_TAGS = setOf("param", "signals")

    /** The sections in their one order, whatever the comment's, each with its label's key. */
    private val SECTIONS = listOf(
        "factory" to "doc.section.factory",
        "param" to "doc.section.params",
        "answers" to "doc.section.answers",
        "signals" to "doc.section.signals",
        "see" to "doc.section.see",
    )
    private val KNOWN_TAGS = SECTIONS.map { (name, _) -> name }.toSet()

    /** Reads `markdown`, a doc comment's lines without their `///` (`NewspeakDocComments.markdown`). */
    fun parse(markdown: String): DocComment {
        val descriptionLines = mutableListOf<String>()
        val tagLines = mutableListOf<Pair<String, MutableList<String>>>()
        var inFence = false
        for (line in markdown.lines().map(String::trimEnd)) {
            val trimmed = line.trimStart()
            val name = if (inFence) null else tagName(trimmed)
            if (trimmed.startsWith(FENCE)) inFence = !inFence
            when {
                name != null -> tagLines += name to mutableListOf(line)
                tagLines.isNotEmpty() -> tagLines.last().second += line
                else -> descriptionLines += line
            }
        }
        val ownDescription = withoutTrailingBlankLines(descriptionLines).joinToString("\n")
        val inline = inlineSummary(ownDescription)
        var description = inline?.first ?: ownDescription
        val summary = inline?.second ?: firstSentence(description)
        val tags = tagLines.map { (name, lines) ->
            val kept = withoutTrailingBlankLines(lines)
            if (name !in KNOWN_TAGS) {
                if (description.isNotEmpty()) description += "\n\n"
                description += kept.joinToString("\n").trimStart()
            }
            tagOf(name, kept)
        }
        return DocComment(description, summary, tags)
    }

    /**
     * The known tags of `comment` as the platform's section table, as Java's
     * rendered doc comments lay theirs out, each item's Markdown turned into
     * HTML by `render`; `""` for a comment without a known tag. An unknown tag
     * is no section: the description shows it.
     */
    fun sectionsHtml(comment: DocComment, render: (String) -> String): String {
        val rows = SECTIONS.mapNotNull { (name, labelKey) ->
            val items = comment.tags.filter { tag -> tag.name == name }.map(::sectionItem)
            if (items.isEmpty()) null else labelKey to items
        }
        if (rows.isEmpty()) return ""
        return buildString {
            append(DocumentationMarkup.SECTIONS_START)
            for ((labelKey, items) in rows) {
                append(DocumentationMarkup.SECTION_HEADER_START)
                append(NewspeakBundle.message(labelKey))
                append(DocumentationMarkup.SECTION_SEPARATOR)
                items.forEach { item -> append(render(item)) }
                append(DocumentationMarkup.SECTION_END)
            }
            append(DocumentationMarkup.SECTIONS_END)
        }
    }

    /** A tag as an item of its section, as the server's hover writes it: a name in code, a dash, the text. */
    private fun sectionItem(tag: DocTag): String = when {
        tag.argument == null -> tag.text
        tag.text.isEmpty() -> "`${tag.argument}`"
        else -> "`${tag.argument}` – ${tag.text}"
    }

    /** The name of the tag `trimmed` opens: `@`, one or more ASCII letters, then whitespace or the line's end. */
    private fun tagName(trimmed: String): String? {
        if (!trimmed.startsWith('@')) return null
        val name = trimmed.drop(1).takeWhile { character -> character in 'a'..'z' || character in 'A'..'Z' }
        if (name.isEmpty()) return null
        val rest = trimmed.substring(1 + name.length)
        return name.takeIf { rest.isEmpty() || rest.first().isWhitespace() }
    }

    /** The tag `name` of `lines`, its own line first. */
    private fun tagOf(name: String, lines: List<String>): DocTag {
        var firstLine = lines.first().trimStart().substring(1 + name.length).trimStart()
        var argument: String? = null
        if (name in ARGUMENT_TAGS && firstLine.isNotEmpty()) {
            val word = firstLine.takeWhile { character -> !character.isWhitespace() }
            argument = word
            firstLine = firstLine.substring(word.length).trimStart()
        }
        val text = (listOf(firstLine) + lines.drop(1)).joinToString("\n")
        return DocTag(name, argument, text.trimStart())
    }

    private fun withoutTrailingBlankLines(lines: List<String>): List<String> =
        lines.dropLastWhile { line -> line.isBlank() }

    /**
     * Tracks whether a walk over text stands inside a backtick code span: a
     * run of backticks opens one, and only a run of the same length closes
     * it, as Markdown reads them.
     */
    private class CodeSpans {
        private var openingRun: Int? = null

        /** Steps over the backtick run at `offset` of `text`, if any, and answers its length. */
        fun step(text: String, offset: Int): Int {
            var run = 0
            while (offset + run < text.length && text[offset + run] == '`') run++
            if (run > 0) {
                openingRun = when (openingRun) {
                    null -> run
                    run -> null
                    else -> openingRun
                }
            }
            return run
        }

        val inside: Boolean get() = openingRun != null
    }

    /**
     * The first `{@summary …}` of `description` outside code, if any: the
     * description with the construct replaced by its text, and the text with
     * its whitespace collapsed. It ends at the `}` matching its `{`, braces in
     * code not counted, so a summary may show code that holds a brace.
     */
    private fun inlineSummary(description: String): Pair<String, String>? {
        var code = CodeSpans()
        var offset = 0
        var opening = -1
        while (offset < description.length) {
            val run = code.step(description, offset)
            if (run > 0) {
                offset += run
                continue
            }
            if (!code.inside && description.startsWith(SUMMARY_OPENING, offset)) {
                opening = offset
                break
            }
            offset++
        }
        if (opening < 0) return null
        val textStart = opening + SUMMARY_OPENING.length
        code = CodeSpans()
        var depth = 1
        offset = textStart
        var closing = -1
        while (offset < description.length) {
            val run = code.step(description, offset)
            if (run > 0) {
                offset += run
                continue
            }
            if (!code.inside) {
                when (description[offset]) {
                    '{' -> depth++
                    '}' -> if (--depth == 0) {
                        closing = offset
                        break
                    }
                }
            }
            offset++
        }
        if (closing < 0) return null
        val text = description.substring(textStart, closing)
        val rendered = description.substring(0, opening) + text + description.substring(closing + 1)
        return rendered to collapsed(text)
    }

    /**
     * The first sentence of `description`'s first paragraph: up to and
     * including the first `.` outside code that whitespace follows or that
     * ends the paragraph -- so `a.b` does not end it -- else the whole
     * paragraph; whitespace collapsed. `null` for a description with no text.
     */
    private fun firstSentence(description: String): String? {
        val paragraph = description.lines()
            .dropWhile { line -> line.isBlank() }
            .takeWhile { line -> line.isNotBlank() }
            .joinToString("\n")
        if (paragraph.isEmpty()) return null
        val code = CodeSpans()
        var offset = 0
        while (offset < paragraph.length) {
            val run = code.step(paragraph, offset)
            if (run > 0) {
                offset += run
                continue
            }
            val character = paragraph[offset]
            offset++
            if (character == '.' && !code.inside && (offset == paragraph.length || paragraph[offset].isWhitespace())) {
                return collapsed(paragraph.substring(0, offset))
            }
        }
        return collapsed(paragraph)
    }

    /** `text` with each run of whitespace -- Unicode's, as Rust's `split_whitespace` reads it -- one space, none at either end. */
    private fun collapsed(text: String): String =
        String(CharArray(text.length) { index -> if (text[index].isWhitespace()) ' ' else text[index] })
            .split(' ')
            .filter(String::isNotEmpty)
            .joinToString(" ")
}
