package io.github.bstrauss.newspeak.lexer

import org.junit.Assert.assertEquals
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

private data class LexedToken(val type: String, val start: Int, val end: Int, val state: Int)

private fun lexFrom(text: String, start: Int, state: Int): List<LexedToken> {
    val lexer = NewspeakLexer()
    lexer.start(text, start, text.length, state)
    val tokens = mutableListOf<LexedToken>()
    while (true) {
        val type = lexer.tokenType ?: break
        tokens.add(LexedToken(type.toString(), lexer.tokenStart, lexer.tokenEnd, lexer.state))
        lexer.advance()
    }
    return tokens
}

/**
 * The editor re-lexes from any token boundary with the state it stored there;
 * doing so must answer exactly the tokens a full lex answers from that point.
 */
fun assertRestartsCorrectly(text: String, context: String) {
    val full = lexFrom(text, 0, 0)
    for ((index, token) in full.withIndex()) {
        assertEquals("$context, restarted at ${token.start}", full.subList(index, full.size), lexFrom(text, token.start, token.state))
    }
}

class LexerRestartTest {
    @Test fun restartsAroundTheNegativeLiteralRule() {
        assertRestartsCorrectly("a -1. a-1. (a) -1. 3 - -1. x (*c*)-42", "fixture")
    }

    @Test fun restartsInsideEveryGolden() {
        val goldens = Path.of(System.getProperty("nsNext.repositoryRoot")).resolve("goldens")
        Files.walk(goldens).use { paths ->
            paths.filter { path -> path.toString().endsWith(".ns") }.forEach { path ->
                assertRestartsCorrectly(Files.readString(path), path.toString())
            }
        }
    }
}
