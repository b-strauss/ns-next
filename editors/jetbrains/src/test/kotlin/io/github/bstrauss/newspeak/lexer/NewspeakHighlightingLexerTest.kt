package io.github.bstrauss.newspeak.lexer

import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.DOC_COMMENT
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes.LINE_COMMENT
import org.junit.Assert.assertEquals
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

/** Every token the highlighting lexer answers for [text] as (type, text), whitespace included. */
private fun highlightingTokens(text: String): List<Pair<IElementType, String>> {
    val lexer = NewspeakHighlightingLexer()
    lexer.start(text, 0, text.length, 0)
    val tokens = mutableListOf<Pair<IElementType, String>>()
    while (true) {
        val type = lexer.tokenType ?: break
        tokens.add(type to text.substring(lexer.tokenStart, lexer.tokenEnd))
        lexer.advance()
    }
    return tokens
}

/** The comment tokens of [text], as (type, text). */
private fun comments(text: String): List<Pair<IElementType, String>> =
    highlightingTokens(text).filter { (type, _) -> type == DOC_COMMENT || type == LINE_COMMENT }

class NewspeakHighlightingLexerTest {
    @Test fun aBlockOfTwoLinesIsOneDocComment() {
        assertEquals(
            listOf(DOC_COMMENT to "/// one\n  /// two"),
            comments("class M = () (\n  /// one\n  /// two\n  run = ( ^1 )\n)"),
        )
    }

    @Test fun aBlockOfThreeLinesIsOneDocComment() {
        assertEquals(listOf(DOC_COMMENT to "/// one\n/// two\n///   three"), comments("/// one\n/// two\n///   three\nx"))
    }

    @Test fun aSingleLineStaysALineComment() {
        assertEquals(listOf(LINE_COMMENT to "/// one"), comments("class M = () (\n  /// one\n  run = ( ^1 )\n)"))
    }

    @Test fun aBlankLineOrCodeSplitsABlock() {
        assertEquals(
            listOf(LINE_COMMENT to "/// one", LINE_COMMENT to "/// two"),
            comments("class M = () (\n  /// one\n\n  /// two\n  run = ( ^1 )\n)"),
        )
        assertEquals(
            listOf(LINE_COMMENT to "/// one", LINE_COMMENT to "/// two"),
            comments("class M = () (\n  /// one\n  run = ( ^1 )\n  /// two\n)"),
        )
    }

    @Test fun aTrailingCommentJoinsNoBlock() {
        assertEquals(
            listOf(LINE_COMMENT to "/// remark", DOC_COMMENT to "/// one\n  /// two"),
            comments("class M = () (\n  run = ( ^1 ) /// remark\n  /// one\n  /// two\n  stop = ( ^2 )\n)"),
        )
        assertEquals(
            listOf(LINE_COMMENT to "/// remark", LINE_COMMENT to "/// one"),
            comments("class M = () (\n  run = ( ^1 ) /// remark\n  /// one\n  stop = ( ^2 )\n)"),
        )
    }

    @Test fun aBlockAtTheStartOfTheFileIsOne() {
        assertEquals(listOf(DOC_COMMENT to "/// one\n/// two"), comments("/// one\n/// two\nclass M = () ()"))
    }

    @Test fun aBlockCommentBesideALineIsNone() {
        assertEquals(listOf(LINE_COMMENT to "/// two"), comments("class M = () (\n  (* one *)\n  /// two\n  run = ( ^1 )\n)"))
    }

    @Test fun aBlockAtTheEndOfTheTextEndsWithIt() {
        assertEquals(listOf(DOC_COMMENT to "/// one\r\n/// two"), comments("x\r\n/// one\r\n/// two"))
    }

    /** Where the editor may relex from: only a token with nothing but whitespace before it on its line. */
    @Test fun theStateSaysWhetherATokenStartsItsLine() {
        val text = "  x /// a\n  /// b\ny"
        val lexer = NewspeakHighlightingLexer()
        lexer.start(text, 0, text.length, 0)
        val states = mutableListOf<Pair<String, Int>>()
        while (lexer.tokenType != null) {
            states += text.substring(lexer.tokenStart, lexer.tokenEnd) to lexer.state
            lexer.advance()
        }
        val initial = NewspeakHighlightingLexer.INITIAL
        val midLine = NewspeakHighlightingLexer.MID_LINE
        assertEquals(
            listOf("  " to initial, "x" to initial, " " to midLine, "/// a" to midLine, "\n  " to midLine, "/// b" to initial, "\n" to midLine, "y" to initial),
            states,
        )
    }

    @Test fun restartsAroundBlocks() {
        val text = "/// a\n/// b\nx /// c\n/// d\n\n/// e\n  /// f\n  /// g\ny"
        assertRestartsCorrectly(text, "fixture", ::NewspeakHighlightingLexer)
    }

    /**
     * Over every source of the repository: the tokens restart correctly, and
     * a doc comment, lexed again by the parsing lexer, is its lines and the
     * whitespace between them -- every other token is the parsing lexer's.
     */
    @Test fun agreesWithTheParsingLexerOverEverySource() {
        val root = Path.of(System.getProperty("nsNext.repositoryRoot"))
        var blocks = 0
        for (directory in listOf("goldens", "crates/ns-runtime/core-library", "benchmarks")) {
            Files.walk(root.resolve(directory)).use { paths ->
                paths.filter { path -> path.toString().endsWith(".ns") }.forEach { path ->
                    val text = Files.readString(path)
                    val expanded = highlightingTokens(text).flatMap { (type, tokenText) ->
                        if (type != DOC_COMMENT) return@flatMap listOf(type to tokenText)
                        blocks += 1
                        val lines = lexAll(tokenText)
                        assertEquals(
                            "$path: a doc comment is `///` lines and one line break between",
                            lines.indices.map { index -> if (index % 2 == 0) LINE_COMMENT else TokenType.WHITE_SPACE },
                            lines.map { (lineType, _) -> lineType },
                        )
                        for ((lineType, lineText) in lines) {
                            if (lineType == TokenType.WHITE_SPACE) assertEquals("$path", 1, lineText.count { it == '\n' })
                        }
                        lines
                    }
                    assertEquals("$path", lexAll(text), expanded)
                }
            }
        }
        assertEquals("the sources hold doc comments", true, blocks > 100)
    }

    /** A restart from every token is quadratic in a file's tokens, so only the goldens, as `LexerRestartTest`. */
    @Test fun restartsInsideEveryGolden() {
        val goldens = Path.of(System.getProperty("nsNext.repositoryRoot")).resolve("goldens")
        Files.walk(goldens).use { paths ->
            paths.filter { path -> path.toString().endsWith(".ns") }.forEach { path ->
                assertRestartsCorrectly(Files.readString(path), path.toString(), ::NewspeakHighlightingLexer)
            }
        }
    }
}
