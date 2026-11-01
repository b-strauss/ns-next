package io.github.bstrauss.newspeak.editor

import com.intellij.openapi.util.TextRange
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.backend.documentation.InlineDocumentation
import com.intellij.psi.PsiFile
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind

class NewspeakInlineDocumentationProviderTest : BasePlatformTestCase() {
    private val cache: DocumentSymbolCache get() = DocumentSymbolCache.getInstance(project)
    private lateinit var serverFetcher: (VirtualFile) -> List<DocumentSymbol>?

    override fun setUp() {
        super.setUp()
        serverFetcher = cache.fetcher
        // No server in a test: a file without a kept answer stays without one.
        cache.fetcher = { null }
    }

    // The light project, and its services, outlive a test.
    override fun tearDown() {
        try {
            cache.fetcher = serverFetcher
        } finally {
            super.tearDown()
        }
    }

    private val source = """
        /// The module.
        ///
        /// Holds `run`, *one* method.
        class M = () (
          /// A remark.
          run = ( ^1 )
          /// Answers two,
          /// always.
          public two = ( ^2 )
        )
    """.trimIndent()

    private fun items(file: PsiFile): List<InlineDocumentation> =
        NewspeakInlineDocumentationProvider().inlineDocumentationItems(file).toList()

    /** A symbol over `source` from the first non-blank character of `startLine` to the end of `endLine`. */
    private fun symbol(name: String, startLine: Int, endLine: Int, children: List<DocumentSymbol> = emptyList()): DocumentSymbol {
        val lines = source.lines()
        val start = lines[startLine].indexOfFirst { !it.isWhitespace() }
        val range = Range(Position(startLine, start), Position(endLine, lines[endLine].length))
        return DocumentSymbol(name, SymbolKind.Method, range, range, null, children)
    }

    fun testEveryDocCommentAndNoSingleLineIsAnItem() {
        val file = myFixture.configureByText("Module.ns", source)
        val texts = items(file).map { it.documentationRange.substring(source) }
        assertEquals(listOf("/// The module.\n///\n/// Holds `run`, *one* method.", "/// Answers two,\n  /// always."), texts)
    }

    fun testTheTextRendersAsMarkdown() {
        val file = myFixture.configureByText("Module.ns", source)
        val html = items(file).first().renderText()!!
        assertTrue(html, Regex("<code>(<span[^>]*>)?run(</span>)?</code>").containsMatchIn(html))
        assertTrue(html, html.contains("<i>one</i>"))
        assertFalse(html, html.contains("///"))
    }

    fun testTheOwnerIsTheServersSymbolOnTheNextLine() {
        val file = myFixture.configureByText("Module.ns", source)
        val two = symbol("two", 8, 8)
        val module = symbol("M", 3, 9, listOf(symbol("run", 5, 5), two))
        cache.fetch(file.virtualFile) { listOf(module) }
        val owners = items(file).map { it.documentationOwnerRange?.substring(source) }
        assertEquals(listOf(source.substring(source.indexOf("class M")), "public two = ( ^2 )"), owners)
    }

    fun testWithoutTheServersSymbolsThereIsNoOwner() {
        val file = myFixture.configureByText("Module.ns", source)
        assertEquals(listOf(null, null), items(file).map { it.documentationOwnerRange })
    }

    fun testAnItemIsFoundByItsRange() {
        val file = myFixture.configureByText("Module.ns", source)
        val range = items(file).last().documentationRange
        assertEquals(range, NewspeakInlineDocumentationProvider().findInlineDocumentation(file, range)?.documentationRange)
        assertNull(NewspeakInlineDocumentationProvider().findInlineDocumentation(file, TextRange(0, 1)))
    }

    fun testAnotherLanguagesFileHasNone() {
        val file = myFixture.configureByText("notes.txt", "/// one\n/// two\n")
        assertEmpty(items(file))
    }
}
