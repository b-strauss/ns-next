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

    fun testTheTagsRenderAsSections() {
        val file = myFixture.configureByText(
            "Module.ns",
            """
            class M = () (
              /// Grows.
              /// @param amount how much.
              /// @param step how often.
              grow: amount by: step = ( ^amount )
            )
            """.trimIndent(),
        )
        val html = items(file).single().renderText()!!
        assertTrue(html, html.contains("class='sections'"))
        assertTrue(html, html.contains("Params:"))
        assertTrue(html, Regex("<code>(<span[^>]*>)?amount(</span>)?</code>").containsMatchIn(html))
        assertFalse(html, html.contains("@param"))
        // Each item is a paragraph of its own, not run together with the next.
        assertTrue(html, Regex("how much\\.(</p>)?\\s*<p>").containsMatchIn(html))
    }

    fun testTheOwnerIsTheServersSymbolOnTheNextLine() {
        val file = myFixture.configureByText("Module.ns", source)
        val two = symbol("two", 8, 8)
        val module = symbol("M", 3, 9, listOf(symbol("run", 5, 5), two))
        cache.fetch(file.virtualFile) { listOf(module) }
        val owners = items(file).map { it.documentationOwnerRange?.substring(source) }
        assertEquals(listOf(source.substring(source.indexOf("class M")), "public two = ( ^2 )"), owners)
    }

    /**
     * A module's `:module:` line stands between its class's doc comment and
     * the class, as in every shipped module: the owner is the symbol after
     * the metadata comments, each on the line right above the next.
     */
    fun testTheOwnerStepsOverMetadata() {
        val text = """
            /// The module.
            /// Holds nothing.
            (* :module: com.acme.Module@1.0.0 *)
            (* :since: 2 *)
            class Module = () ()
        """.trimIndent()
        val file = myFixture.configureByText("Module.ns", text)
        val range = Range(Position(4, 0), Position(4, text.lines()[4].length))
        cache.fetch(file.virtualFile) { listOf(DocumentSymbol("Module", SymbolKind.Class, range, range, null, emptyList())) }
        assertEquals(listOf("class Module = () ()"), items(file).map { it.documentationOwnerRange?.substring(text) })
    }

    /** A symbol over `text` from the first non-blank character of `line` to that line's end. */
    private fun symbolOn(text: String, name: String, kind: SymbolKind, line: Int, children: List<DocumentSymbol> = emptyList()): DocumentSymbol {
        val lines = text.lines()
        val range = Range(Position(line, lines[line].indexOfFirst { !it.isWhitespace() }), Position(line, lines[line].length))
        return DocumentSymbol(name, kind, range, range, null, children)
    }

    /** The owners of `text`'s doc comments, in order, as `symbols` place them. */
    private fun ownersOf(text: String, symbols: List<DocumentSymbol>): List<String?> {
        val file = myFixture.configureByText("Module.ns", text)
        cache.fetch(file.virtualFile) { symbols }
        return items(file).map { it.documentationOwnerRange?.substring(text) }
    }

    /**
     * A blank line between a doc comment and the metadata after it ends the
     * comment's reach, as it does between the comment and a declaration:
     * the comment documents nothing, though a symbol starts after the
     * metadata.
     */
    fun testABlankLineBeforeTheMetadataDetachesTheComment() {
        val text = """
            /// The module.
            /// Holds nothing.

            (* :module: com.acme.Module@1.0.0 *)
            class Module = () ()
        """.trimIndent()
        assertEquals(listOf(null), ownersOf(text, listOf(symbolOn(text, "Module", SymbolKind.Class, 4))))
    }

    /** A metadata comment above a slot or a method is stepped over as one above a class is. */
    fun testTheOwnerStepsOverMetadataAboveASlotAndAMethod() {
        val text = """
            class Module = (
              |
                /// The count.
                /// Starts at 0.
                (* :since: 2 *)
                public count ::= 0.
              |
            ) (
              /// Answers two.
              /// Always.
              (* :deprecated: use three *)
              public two = ( ^2 )
            )
        """.trimIndent()
        val count = symbolOn(text, "count", SymbolKind.Property, 5)
        val two = symbolOn(text, "two", SymbolKind.Method, 11)
        val module = Range(Position(0, 0), Position(12, 1))
        val root = DocumentSymbol("Module", SymbolKind.Class, module, module, null, listOf(count, two))
        assertEquals(listOf("public count ::= 0.", "public two = ( ^2 )"), ownersOf(text, listOf(root)))
    }

    /**
     * With the server's symbols at hand, a comment whose documented line
     * starts no symbol has no owner: past its metadata stands a plain
     * remark, or what follows starts no declaration the server names.
     */
    fun testACommentWithoutADocumentedSymbolHasNoOwner() {
        val text = """
            class Module = () (
              /// Answers two.
              /// Always.
              (* :since: 2 *)
              (* a remark *)
              public two = ( ^2 )
              /// Answers three.
              /// Always.
              (* :since: 3 *)
              public three = ( ^3 )
            )
        """.trimIndent()
        val two = symbolOn(text, "two", SymbolKind.Method, 5)
        val module = Range(Position(0, 0), Position(10, 1))
        val root = DocumentSymbol("Module", SymbolKind.Class, module, module, null, listOf(two))
        assertEquals(listOf(null, null), ownersOf(text, listOf(root)))
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
