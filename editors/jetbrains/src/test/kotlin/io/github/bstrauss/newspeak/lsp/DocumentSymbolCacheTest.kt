package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

class DocumentSymbolCacheTest : BasePlatformTestCase() {
    private val range = Range(Position(0, 0), Position(0, 20))
    private fun moduleSymbols() = listOf(DocumentSymbol("Module", SymbolKind.Class, range, range, "public"))

    private val cache: DocumentSymbolCache get() = DocumentSymbolCache.getInstance(project)
    private lateinit var serverFetcher: (com.intellij.openapi.vfs.VirtualFile) -> List<DocumentSymbol>?

    override fun setUp() {
        super.setUp()
        serverFetcher = cache.fetcher
    }

    // The light project, and its services, outlive a test.
    override fun tearDown() {
        try {
            cache.fetcher = serverFetcher
        } finally {
            super.tearDown()
        }
    }

    private val file by lazy { myFixture.addFileToProject("Module.ns", "class Module = () ()\n").virtualFile }

    private fun edit() {
        val document = FileDocumentManager.getInstance().getDocument(file)!!
        WriteCommandAction.runWriteCommandAction(project) { document.insertString(0, " ") }
    }

    fun testAFetchedAnswerIsCurrentUntilTheDocumentChanges() {
        cache.fetcher = { moduleSymbols() }
        assertNull(cache.current(file))
        assertEquals(moduleSymbols(), cache.fetch(file))
        assertEquals(moduleSymbols(), cache.current(file))
        edit()
        assertNull("an answer for older text places nothing", cache.current(file))
    }

    fun testAnEmptyAnswerIsNotKept() {
        cache.fetcher = { emptyList() }
        assertNull(cache.fetch(file))
        assertNull(cache.current(file))
    }

    fun testARequestCallsBackOnceTheAnswerIsKept() {
        val fetches = AtomicInteger()
        cache.fetcher = { fetches.incrementAndGet(); moduleSymbols() }
        val answered = CountDownLatch(1)
        cache.request(file) { answered.countDown() }
        assertTrue(answered.await(5, TimeUnit.SECONDS))
        assertEquals(moduleSymbols(), cache.current(file))
        assertEquals(1, fetches.get())
    }

    fun testARequestAsksAgainWhileTheServerHasNoAnswer() {
        val fetches = AtomicInteger()
        // No client yet on the first question, an answer on the second.
        cache.fetcher = { if (fetches.incrementAndGet() == 1) null else moduleSymbols() }
        val answered = CountDownLatch(1)
        cache.request(file) { answered.countDown() }
        assertTrue(answered.await(10, TimeUnit.SECONDS))
        assertEquals(2, fetches.get())
    }

    fun testTwoRequestsForOneStampAskOnce() {
        val fetches = AtomicInteger()
        val release = CountDownLatch(1)
        cache.fetcher = { fetches.incrementAndGet(); release.await(5, TimeUnit.SECONDS); moduleSymbols() }
        val answered = CountDownLatch(1)
        cache.request(file) { answered.countDown() }
        cache.request(file) { answered.countDown() }
        release.countDown()
        assertTrue(answered.await(5, TimeUnit.SECONDS))
        assertEquals(1, fetches.get())
    }
}
