package io.github.bstrauss.newspeak.highlighting

import com.intellij.openapi.util.TextRange
import com.intellij.psi.PsiFileFactory
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.NewspeakLanguage
import io.github.bstrauss.newspeak.editor.NewspeakDocComments
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes
import java.nio.file.Files
import java.nio.file.Path

/**
 * The block rule is stated twice: over the PSI (`NewspeakDocComments`, what
 * renders) and over the buffer (`NewspeakHighlightingLexer`, what shows
 * green). Over every source of the repository the two find the same doc
 * comments, so a change to either alone -- what counts as whitespace, what
 * starts a line -- fails here instead of colouring a comment that does not
 * render, or the reverse.
 */
class NewspeakDocCommentAgreementTest : BasePlatformTestCase() {
    private fun psiBlocks(text: String): List<TextRange> {
        val file = PsiFileFactory.getInstance(project).createFileFromText("Module.ns", NewspeakLanguage, text)
        return NewspeakDocComments.blocks(file).map { lines -> TextRange(lines.first().textRange.startOffset, lines.last().textRange.endOffset) }
    }

    private fun highlighterBlocks(text: String): List<TextRange> {
        val lexer = NewspeakSyntaxHighlighter().highlightingLexer
        lexer.start(text, 0, text.length, 0)
        val ranges = mutableListOf<TextRange>()
        while (true) {
            val type = lexer.tokenType ?: break
            if (type == NewspeakTokenTypes.DOC_COMMENT) ranges += TextRange(lexer.tokenStart, lexer.tokenEnd)
            lexer.advance()
        }
        return ranges
    }

    /** The edges of the rule the sources may not show: every whitespace character, remarks, blank lines, the text's ends. */
    fun testTheyAgreeAtTheEdgesOfTheRule() {
        val texts = listOf(
            "/// a\n/// b",
            "\u000C/// a\n \t\u000C/// b\nx",
            "x /// a\n/// b\n/// c",
            "x /// a\n/// b\ny",
            "/// a\n\n/// b\n/// c\n \n/// d",
            "(* c *) /// a\n/// b",
            "/// a\r/// b\n/// c",
            "'s\n/// a' /// b\n/// c",
        )
        for (text in texts) assertEquals(text, psiBlocks(text), highlighterBlocks(text))
    }

    fun testThePsiRuleAndTheHighlighterFindTheSameDocComments() {
        val root = Path.of(System.getProperty("nsNext.repositoryRoot"))
        var blocks = 0
        for (directory in listOf("goldens", "crates/ns-runtime/core-library", "benchmarks")) {
            Files.walk(root.resolve(directory)).use { paths ->
                paths.filter { path -> path.toString().endsWith(".ns") }.forEach { path ->
                    // A document's text, as the editor and the PSI hold it.
                    val text = Files.readString(path).replace("\r\n", "\n")
                    val expected = psiBlocks(text)
                    assertEquals("$path", expected, highlighterBlocks(text))
                    blocks += expected.size
                }
            }
        }
        assertTrue("the sources hold doc comments", blocks > 100)
    }
}
