package io.github.bstrauss.newspeak.lexer

import com.google.gson.JsonParser
import com.intellij.psi.TokenType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.TimeUnit
import kotlin.io.path.extension
import kotlin.io.path.isRegularFile

private data class ComparedToken(val kind: String, val start: Int, val end: Int)

/**
 * The Kotlin lexer against `ns-syntax`'s, over every module the repository
 * holds: every significant token must agree in kind and span, and the tokens
 * only the Kotlin lexer has -- whitespace and comments -- must fill exactly
 * the gaps between them. This is what keeps the second lexer honest
 * (`docs/design/editor-support.md`, "Why a Kotlin lexer").
 */
class LexerComparisonTest {
    private val repositoryRoot: Path = Path.of(System.getProperty("nsNext.repositoryRoot"))
    private val languageServer: Path = Path.of(System.getProperty("nsNext.languageServer"))

    private fun modules(): List<Path> {
        val files = mutableListOf<Path>()
        for (directory in listOf("corpus/modules", "goldens")) {
            Files.walk(repositoryRoot.resolve(directory)).use { paths ->
                paths.filter { path -> path.isRegularFile() && path.extension == "ns" }.forEach { path -> files.add(path) }
            }
        }
        files.add(repositoryRoot.resolve("crates/ns-runtime/core-library/Kernel.ns"))
        return files.sorted()
    }

    /**
     * `dump-tokens` for one batch of files, in a single process. Splitting into
     * batches is what keeps the whole run under Windows' command-line length
     * limit -- the repository holds close to 400 `.ns` files, well past the
     * couple of hundred a single command line can safely carry.
     */
    private fun rustTokensBatch(files: List<Path>): Map<String, Pair<List<ComparedToken>, Int>> {
        val output = Files.createTempFile("dump-tokens", ".jsonl")
        try {
            val process = ProcessBuilder(listOf(languageServer.toString(), "dump-tokens") + files.map { file -> file.toString() })
                .redirectOutput(output.toFile())
                .redirectError(ProcessBuilder.Redirect.INHERIT)
                .start()
            assertTrue("dump-tokens finishes", process.waitFor(5, TimeUnit.MINUTES))
            assertEquals("dump-tokens exit status", 0, process.exitValue())
            return Files.readAllLines(output).associate { line ->
                val entry = JsonParser.parseString(line).asJsonObject
                val tokens = entry.getAsJsonArray("tokens").map { element ->
                    val token = element.asJsonObject
                    ComparedToken(token.get("kind").asString, token.get("start").asInt, token.get("end").asInt)
                }
                entry.get("file").asString to (tokens to entry.get("diagnostics").asInt)
            }
        } finally {
            Files.deleteIfExists(output)
        }
    }

    private fun rustTokens(files: List<Path>): Map<String, Pair<List<ComparedToken>, Int>> {
        val merged = mutableMapOf<String, Pair<List<ComparedToken>, Int>>()
        for (batch in files.chunked(BATCH_SIZE)) {
            merged.putAll(rustTokensBatch(batch))
        }
        return merged
    }

    @Test
    fun theKotlinLexerAgreesWithNsSyntax() {
        val files = modules()
        assertTrue("found only ${files.size} modules", files.size > 200)
        val expected = rustTokens(files)
        for (file in files) {
            // Read exactly as the Rust lexer reads it: no line-ending conversion.
            val text = Files.readString(file)
            val (rustTokens, diagnostics) = expected.getValue(file.toString())
            val lexer = NewspeakLexer()
            lexer.start(text, 0, text.length, 0)
            val significant = mutableListOf<ComparedToken>()
            var badCharacters = 0
            var covered = 0
            while (true) {
                val type = lexer.tokenType ?: break
                assertEquals("$file: tokens are contiguous", covered, lexer.tokenStart)
                covered = lexer.tokenEnd
                val tokenText = text.substring(lexer.tokenStart, lexer.tokenEnd)
                when {
                    type == TokenType.WHITE_SPACE ->
                        assertTrue("$file: whitespace at ${lexer.tokenStart}", tokenText.all { character -> character in " \t\n\u000C\r" })
                    type == NewspeakTokenTypes.COMMENT ->
                        assertTrue("$file: comment at ${lexer.tokenStart}", tokenText.startsWith("(*"))
                    type == TokenType.BAD_CHARACTER -> badCharacters += 1
                    else -> significant.add(ComparedToken((type as NewspeakTokenType).rustKind!!, lexer.tokenStart, lexer.tokenEnd))
                }
                lexer.advance()
            }
            assertEquals("$file: the whole text is covered", text.length, covered)
            assertEquals("$file", rustTokens, significant)
            if (diagnostics == 0) {
                assertEquals("$file: no bad character where ns-syntax reports nothing", 0, badCharacters)
            }
        }
    }

    companion object {
        /** Files per `dump-tokens` invocation; see [rustTokens]. */
        private const val BATCH_SIZE = 100
    }
}
