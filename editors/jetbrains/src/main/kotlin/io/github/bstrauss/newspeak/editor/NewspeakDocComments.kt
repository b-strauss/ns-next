package io.github.bstrauss.newspeak.editor

import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.PsiWhiteSpace
import com.intellij.psi.util.CachedValueProvider
import com.intellij.psi.util.CachedValuesManager
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * A doc comment: a block of two or more `///` lines, which documents what
 * follows it as a Java doc comment does. The rendered view
 * (`NewspeakInlineDocumentationProvider`) reads this rule, and the colour
 * (`NewspeakHighlightingLexer`, which merges a block into one token) states
 * the same one over the text, so what shows green is what renders.
 *
 * A block is the server's (`ns-syntax`'s `continues_line_block`): `///`
 * lines that each start their line, with only whitespace and one line break
 * between two of them. A `///` after code is a remark on its line and
 * belongs to no block.
 *
 * What a block documents is what starts after it, past the metadata
 * comments between -- a module's `(* :module: ... *)` stands between its
 * class's doc comment and the class -- each on the line right above the
 * next, as the server's hover reads it (`hover::doc_comment_above`): a
 * blank line or any other comment between, or code after a metadata
 * comment on its line, leaves the block documenting nothing.
 */
object NewspeakDocComments {
    /** Whether `element` is a `///` line of a doc comment. */
    fun isDocCommentLine(element: PsiElement): Boolean =
        element.node.elementType == Types.LINE_COMMENT &&
            startsItsLine(element) &&
            (blockNeighbour(element, PsiElement::getPrevSibling) != null || blockNeighbour(element, PsiElement::getNextSibling) != null)

    /** Every doc comment of `file`, each as its `///` lines in order. */
    fun blocks(file: PsiFile): List<List<PsiElement>> {
        val blocks = mutableListOf<List<PsiElement>>()
        var element = file.firstChild
        while (element != null) {
            if (isDocCommentLine(element)) {
                val lines = mutableListOf(element)
                while (true) lines += blockNeighbour(lines.last(), PsiElement::getNextSibling) ?: break
                blocks += lines
                element = lines.last()
            }
            element = element.nextSibling
        }
        return blocks
    }

    /** A doc comment's text as Markdown: each line without its `///` and the one space after it. */
    fun markdown(lines: List<PsiElement>): String =
        lines.joinToString("\n") { line -> line.text.removePrefix("///").removePrefix(" ").trimEnd() }

    /**
     * The summary (`NewspeakDocTags`) of the doc comment of `file` that
     * documents what starts on the 0-based `line` ([documentedStart]);
     * `null` without such a comment or without a summary.
     */
    fun summaryAbove(file: PsiFile, line: Int): String? = summariesByDocumentedLine(file)[line]

    /**
     * The offset at which what the doc comment [lines] documents starts: the
     * element on the line right after it, or after the metadata comments
     * that follow it, each on the line right above the next; `null` where
     * a blank line, another comment, or nothing stands there.
     */
    fun documentedStart(lines: List<PsiElement>): Int? {
        var last = lines.last()
        while (true) {
            val gap = last.nextSibling as? PsiWhiteSpace ?: return null
            if (gap.text.count { it == '\n' } != 1) return null
            val next = gap.nextSibling ?: return null
            when {
                isMetadata(next) -> last = next
                next.node.elementType == Types.COMMENT || next.node.elementType == Types.LINE_COMMENT -> return null
                else -> return next.textRange.startOffset
            }
        }
    }

    /**
     * Whether `element` is a metadata comment, `(* :tag: payload *)`, as
     * `ns-syntax`'s `Metadata::read` reads one: the text inside opens, past
     * whitespace, with `:`, then a tag of no whitespace that is never
     * empty, then a second `:`.
     */
    private fun isMetadata(element: PsiElement): Boolean {
        if (element.node.elementType != Types.COMMENT) return false
        val text = element.text
        if (text.length < 4 || !text.startsWith("(*") || !text.endsWith("*)")) return false
        val inside = text.substring(2, text.length - 2).trimStart()
        if (!inside.startsWith(':')) return false
        val tagEnd = inside.indexOf(':', startIndex = 1).takeIf { index -> index >= 0 } ?: return false
        val tag = inside.substring(1, tagEnd)
        return tag.isNotEmpty() && tag.none(Char::isWhitespace)
    }

    /**
     * Every summary of `file` by the 0-based line what its doc comment
     * documents starts on, computed once per change of the file, its
     * comments included: the structure view asks for every row, and a walk
     * of the file per row would make a large module's tree quadratic. Lines
     * are counted in the PSI's own text, which its offsets index, so an edit
     * not yet committed never misplaces a summary.
     */
    private fun summariesByDocumentedLine(file: PsiFile): Map<Int, String> =
        CachedValuesManager.getCachedValue(file) {
            val text = file.text
            val summaries = HashMap<Int, String>()
            var line = 0
            var counted = 0
            for (block in blocks(file)) {
                // The blocks come in order, and what one documents starts before the next block.
                val start = documentedStart(block) ?: continue
                while (counted < start) {
                    if (text[counted] == '\n') line++
                    counted++
                }
                NewspeakDocTags.parse(markdown(block)).summary?.let { summary -> summaries[line] = summary }
            }
            CachedValueProvider.Result.create<Map<Int, String>>(summaries, file)
        }

    /** The `///` line just before or after `comment` in its block, if there is one. */
    private fun blockNeighbour(comment: PsiElement, step: (PsiElement) -> PsiElement?): PsiElement? {
        val gap = step(comment) as? PsiWhiteSpace ?: return null
        val neighbour = step(gap) ?: return null
        val oneLineBreak = gap.text.count { it == '\n' } == 1
        return neighbour.takeIf { oneLineBreak && it.node.elementType == Types.LINE_COMMENT && startsItsLine(it) }
    }

    /** Whether only whitespace stands before `comment` on its line. */
    private fun startsItsLine(comment: PsiElement): Boolean {
        val before = comment.prevSibling ?: return true
        return before is PsiWhiteSpace && ('\n' in before.text || before.prevSibling == null)
    }
}
