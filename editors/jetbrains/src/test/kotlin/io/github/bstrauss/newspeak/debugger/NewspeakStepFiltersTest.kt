package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonPrimitive
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.vfs.newvfs.impl.VfsRootAccess
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.util.xmlb.XmlSerializer
import io.github.bstrauss.newspeak.library.CoreLibraryStore
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.TimeUnit
import javax.swing.tree.DefaultMutableTreeNode

/**
 * A project's classes not to step into: the chooser's choices of files and
 * classes, the entries stored per project and read back, a library file's
 * entry across fingerprints, the arguments a session sends, and the page
 * that lists them.
 */
class NewspeakStepFiltersTest : BasePlatformTestCase() {
    private lateinit var directory: Path

    override fun setUp() {
        super.setUp()
        directory = Files.createTempDirectory("ns-next-step-filters")
        VfsRootAccess.allowRootAccess(testRootDisposable, directory.toString())
    }

    override fun tearDown() {
        try {
            NewspeakSkippedClasses.getInstance(project).update(emptyList())
            CoreLibraryStore.deleteRecursively(directory)
        } finally {
            super.tearDown()
        }
    }

    /** The chooser offers each file, for its whole module, with the classes its root nests, as the source nests them. */
    fun testTheChooserOffersEachFileWithItsNestedClasses() {
        val walker = file("Walker.ns")
        val plain = file("Plain.ns")
        val symbols = mapOf(
            walker to listOf(
                symbol(
                    "Walker",
                    SymbolKind.Class,
                    symbol("main:args:", SymbolKind.Method),
                    symbol("Helper", SymbolKind.Class, symbol("Inner", SymbolKind.Class), symbol("run", SymbolKind.Method)),
                    symbol("Other", SymbolKind.Class),
                ),
            ),
        )
        val choices = choicesOf(listOf(walker, plain), { file, classPath -> SkippedClass(false, file.name, classPath) }, symbols::get)
        fun outline(choice: StepFilterChoice): Any = choice.entry.classPath to choice.children.map(::outline)
        assertEquals(listOf("Plain.ns", "Walker.ns"), choices.map(StepFilterChoice::label))
        assertEquals("a file the server did not answer offers its whole module only", emptyList<Any>(), choices[0].children)
        assertEquals(
            emptyList<String>() to listOf(
                listOf("Helper") to listOf(listOf("Helper", "Inner") to emptyList<Any>()),
                listOf("Other") to emptyList<Any>(),
            ),
            outline(choices[1]),
        )
        assertEquals(setOf(emptyList(), listOf("Helper"), listOf("Helper", "Inner"), listOf("Other")), classPathsOf(symbols.getValue(walker)))

        val root = rootOf(StepFilterChoices(project = choices, library = emptyList()))
        assertEquals("only the groups that have files", 1, root.childCount)
        assertEquals(2, (root.getChildAt(0) as DefaultMutableTreeNode).childCount)
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

    private fun symbol(name: String, kind: SymbolKind, vararg children: DocumentSymbol): DocumentSymbol {
        val range = Range(Position(0, 0), Position(0, 0))
        return DocumentSymbol(name, kind, range, range, null, children.toList())
    }

    private companion object {
        const val WAIT_SECONDS = 30L
    }
}
