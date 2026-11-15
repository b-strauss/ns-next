package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonPrimitive
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.vfs.newvfs.impl.VfsRootAccess
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.util.xmlb.XmlSerializer
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.library.CoreLibraryStore
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import javax.swing.tree.DefaultMutableTreeNode

/**
 * A project's classes not to step into: the chooser's choices of files and
 * classes, the entries stored per project and read back, a library file's
 * entry across fingerprints, the arguments a session sends, and the page
 * that lists them.
 */
class NewspeakStepFiltersTest : BasePlatformTestCase() {
    private lateinit var directory: Path

    /** Releases a fetcher that never answers, so that its thread ends with the test. */
    private val released = CountDownLatch(1)

    private lateinit var originalFetcher: (VirtualFile) -> List<DocumentSymbol>?

    override fun setUp() {
        super.setUp()
        originalFetcher = DocumentSymbolCache.getInstance(project).fetcher
        directory = Files.createTempDirectory("ns-next-step-filters")
        VfsRootAccess.allowRootAccess(testRootDisposable, directory.toString())
    }

    override fun tearDown() {
        try {
            released.countDown()
            DocumentSymbolCache.getInstance(project).fetcher = originalFetcher
            NewspeakSkippedClasses.getInstance(project).update(emptyList())
            CoreLibraryStore.deleteRecursively(directory)
        } finally {
            super.tearDown()
        }
    }

    /** A file's choices are the classes its root nests, as the source nests them; the paths they name are what an entry is checked against. */
    fun testAFileOffersItsNestedClasses() {
        val walker = file("Walker.ns")
        val symbols = walkerSymbols()
        val choices = classChoicesOf(walker, symbols) { file, classPath -> SkippedClass(false, file.name, classPath) }
        fun outline(choice: StepFilterChoice): Any = choice.entry.classPath to choice.children.map(::outline)
        assertEquals(
            listOf(
                listOf("Helper") to listOf(listOf("Helper", "Inner") to emptyList<Any>()),
                listOf("Other") to emptyList<Any>(),
            ),
            choices.map(::outline),
        )
        assertEquals(setOf(emptyList(), listOf("Helper"), listOf("Helper", "Inner"), listOf("Other")), classPathsOf(symbols))
    }

    /**
     * The chooser lists the files at once, each for its whole module, and
     * asks a file's classes only as its node expands: the language server's
     * answer puts them under it, nothing asked before.
     */
    fun testTheChooserReadsAFilesClassesAsItExpands() {
        val walker = file("Walker.ns")
        val plain = file("Plain.ns")
        val asked = mutableListOf<VirtualFile>()
        DocumentSymbolCache.getInstance(project).fetcher = { file ->
            synchronized(asked) { asked.add(file) }
            if (file == walker) walkerSymbols() else null
        }
        val chooser = StepFilterTree(project, StepFilterFiles(project = listOf(plain, walker), library = emptyList()))
        Disposer.register(testRootDisposable, chooser)
        val root = chooser.model.root as DefaultMutableTreeNode
        assertEquals("only the groups that have files", 1, root.childCount)
        val group = root.getChildAt(0) as DefaultMutableTreeNode
        assertEquals(2, group.childCount)
        assertEquals("nothing is asked before a node expands", emptyList<VirtualFile>(), synchronized(asked) { asked.toList() })
        val node = group.children().toList().map { child -> child as DefaultMutableTreeNode }
            .first { child -> (child.userObject as StepFilterChoice).file == walker }
        assertEquals(StepFilterPending, (node.firstChild as DefaultMutableTreeNode).userObject)
        checkNotNull(chooser.read(node))
        PlatformTestUtil.waitWithEventsDispatching(
            "the classes are shown",
            { (node.firstChild as DefaultMutableTreeNode).userObject !== StepFilterPending },
            WAIT_SECONDS.toInt(),
        )
        val classes = node.children().toList().map { child -> ((child as DefaultMutableTreeNode).userObject as StepFilterChoice).entry.classPath }
        assertEquals(listOf(listOf("Helper"), listOf("Other")), classes)
        assertEquals(listOf(walker), synchronized(asked) { asked.toList() })
        assertNull("a node read once is not asked again", chooser.read(node))
    }

    /**
     * A language server that never answers -- its request blocked, deaf to
     * the interrupt -- never holds the chooser: after the bound, the file
     * offers its whole module only, and the question given up asks nothing
     * more once the request returns.
     */
    fun testAFileTheServerNeverAnswersOffersItsWholeModuleOnly() {
        val walker = file("Walker.ns")
        val fetches = AtomicInteger()
        DocumentSymbolCache.getInstance(project).fetcher = { _ ->
            fetches.incrementAndGet()
            while (released.count > 0) {
                try {
                    released.await()
                } catch (_: InterruptedException) {
                    // Deaf to it, as a request blocked in the platform may be.
                }
            }
            null
        }
        val files = StepFilterFiles(project = listOf(walker), library = emptyList())
        val chooser = StepFilterTree(project, files, timeoutMillis = SHORT_BOUND_MILLISECONDS)
        Disposer.register(testRootDisposable, chooser)
        val node = (chooser.model.root as DefaultMutableTreeNode).getChildAt(0).getChildAt(0) as DefaultMutableTreeNode
        val choice = node.userObject as StepFilterChoice
        checkNotNull(chooser.read(node))
        PlatformTestUtil.waitWithEventsDispatching("the file is given up on", { node.childCount == 0 }, WAIT_SECONDS.toInt())
        assertEquals(NewspeakBundle.message("debugger.step.filters.whole.module"), choice.note)
        assertEquals(emptyList<String>(), choice.entry.classPath)
        released.countDown()
        // Longer than a retry's pause: a question that went on would have asked again by then.
        Thread.sleep(RETRY_OUTLASTED_MILLISECONDS)
        assertEquals("asked once, never after it was given up", 1, fetches.get())
    }

    /** The entries are stored with the project and read back as they were. */
    fun testTheEntriesAreStoredAndReadBack() {
        val skipped = NewspeakSkippedClasses.getInstance(project)
        val entries = listOf(
            SkippedClass(inLibrary = false, path = "source/Walker.ns", classPath = listOf("Helper", "Inner")),
            SkippedClass(inLibrary = true, path = "platform/CollectionsFactory.ns", classPath = emptyList()),
        )
        skipped.update(entries)
        val element = XmlSerializer.serialize(skipped.state)
        skipped.update(emptyList())
        skipped.loadState(XmlSerializer.deserialize(element, NewspeakSkippedClasses.Stored::class.java))
        assertEquals(entries, skipped.entries)
    }

    /**
     * A library file's entry is its path below the fingerprint's directory,
     * so that the next fingerprint's library -- an `ns-next` rebuilt --
     * names its file of the same path.
     */
    fun testALibraryEntrySurvivesAFingerprintChange() {
        val older = NewspeakCoreLibrary.STORE.root.resolve("00000000000000aa")
        val newer = NewspeakCoreLibrary.STORE.root.resolve("00000000000000bb")
        try {
            for (library in listOf(older, newer)) {
                Files.createDirectories(library.resolve("platform"))
                Files.writeString(library.resolve("platform/CollectionsFactory.ns"), "class CollectionsFactory = ()\n")
                VfsRootAccess.allowRootAccess(testRootDisposable, library.toString())
            }
            val skipped = NewspeakSkippedClasses.getInstance(project)
            val olderFile = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(older.resolve("platform/CollectionsFactory.ns")))
            val entry = skipped.entryFor(olderFile, listOf("List"))
            assertEquals(SkippedClass(inLibrary = true, path = "platform/CollectionsFactory.ns", classPath = listOf("List")), entry)
            val newerFile = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(newer.resolve("platform/CollectionsFactory.ns")))
            assertEquals(newerFile, skipped.fileOf(entry, projectDirectory = null, libraryDirectory = newer))
            assertNull("no library shown, no file", skipped.fileOf(entry, projectDirectory = null, libraryDirectory = null))
        } finally {
            CoreLibraryStore.deleteRecursively(older)
            CoreLibraryStore.deleteRecursively(newer)
        }
    }

    /** A project file's entry is relative to the project's directory, one outside it absolute; a session sends each by its file's path. */
    fun testAProjectEntryIsRelativeAndSentByItsPath() {
        val skipped = NewspeakSkippedClasses.getInstance(project)
        val source = directory.resolve("source")
        Files.createDirectories(source)
        Files.writeString(source.resolve("Walker.ns"), "class Walker = ()\n")
        val walker = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(source.resolve("Walker.ns")))
        val base = checkNotNull(LocalFileSystem.getInstance().findFileByNioFile(directory))
        val relative = skipped.entryFor(walker, listOf("Helper"), projectDirectory = base)
        assertEquals(SkippedClass(inLibrary = false, path = "source/Walker.ns", classPath = listOf("Helper")), relative)
        assertEquals(walker, skipped.fileOf(relative, projectDirectory = directory))

        val outside = skipped.entryFor(walker, emptyList(), projectDirectory = null)
        assertEquals(walker.toNioPath().toString(), outside.path)
        val gone = SkippedClass(inLibrary = false, path = directory.resolve("Gone.ns").toString(), classPath = emptyList())
        skipped.update(listOf(outside, gone))
        assertEquals(
            """[{"source":{"path":${JsonPrimitive(walker.toNioPath().toString())}},"classPath":[]}]""",
            skipped.arguments().toString(),
        )
    }

    /** An entry whose path no file system can hold -- a damaged workspace file -- counts as gone and never stops the arguments. */
    fun testAnEntryOfAnImpossiblePathCountsAsGone() {
        val skipped = NewspeakSkippedClasses.getInstance(project)
        val damaged = listOf(
            SkippedClass(inLibrary = false, path = "bad\u0000name.ns", classPath = emptyList()),
            SkippedClass(inLibrary = true, path = "platform/bad\u0000name.ns", classPath = emptyList()),
        )
        for (entry in damaged) {
            assertNull(entry.resolvedAgainst(directory))
            assertNull(skipped.fileOf(entry, projectDirectory = directory, libraryDirectory = directory))
        }
        skipped.update(damaged)
        assertEquals("[]", skipped.arguments().toString())
    }

    /** The page lists the project's entries, applies and resets them, and marks one whose file is gone. */
    fun testThePageAppliesResetsAndMarksAGoneEntry() {
        val skipped = NewspeakSkippedClasses.getInstance(project)
        val gone = SkippedClass(inLibrary = false, path = directory.resolve("Gone.ns").toString(), classPath = emptyList())
        skipped.update(listOf(gone))
        val page = NewspeakStepFiltersConfigurable(project)
        page.createComponent()
        assertEquals(listOf(gone), page.entries.items)
        assertFalse(page.isModified)
        page.checkEntries().get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertEquals(setOf(gone), page.invalid)

        page.entries.remove(0)
        assertTrue(page.isModified)
        page.apply()
        assertEquals(emptyList<SkippedClass>(), skipped.entries)
        skipped.update(listOf(gone))
        page.reset()
        assertEquals(listOf(gone), page.entries.items)
        page.disposeUIResources()
    }

    private fun file(name: String): VirtualFile {
        val path = directory.resolve(name)
        Files.writeString(path, "class ${name.removeSuffix(".ns")} = ()\n")
        return checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(path))
    }

    private fun walkerSymbols(): List<DocumentSymbol> = listOf(
        symbol(
            "Walker",
            SymbolKind.Class,
            symbol("main:args:", SymbolKind.Method),
            symbol("Helper", SymbolKind.Class, symbol("Inner", SymbolKind.Class), symbol("run", SymbolKind.Method)),
            symbol("Other", SymbolKind.Class),
        ),
    )

    private fun symbol(name: String, kind: SymbolKind, vararg children: DocumentSymbol): DocumentSymbol {
        val range = Range(Position(0, 0), Position(0, 0))
        return DocumentSymbol(name, kind, range, range, null, children.toList())
    }

    private companion object {
        const val WAIT_SECONDS = 30L
        const val SHORT_BOUND_MILLISECONDS = 300L
        const val RETRY_OUTLASTED_MILLISECONDS = 1_500L
    }
}
