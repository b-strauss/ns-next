package io.github.bstrauss.newspeak.library

import com.intellij.openapi.util.io.FileUtil
import com.intellij.openapi.vfs.VfsUtilCore
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.FileTime
import java.time.Duration
import java.time.Instant

/** The store's pruning and its places, over directories the test writes: no `ns-next` is started. */
class CoreLibraryStoreTest {
    private lateinit var root: Path
    private lateinit var store: CoreLibraryStore
    private val now: Instant = Instant.parse("2026-11-15T12:00:00Z")

    @Before fun createRoot() {
        root = Files.createTempDirectory("ns-next-core-library")
        store = CoreLibraryStore(root)
    }

    @After fun deleteRoot() {
        CoreLibraryStore.deleteRecursively(root)
    }

    @Test fun aDirectoryUnusedForThirtyDaysIsDeletedAndARecentlyUsedOneKept() {
        val old = fingerprintDirectory("0000000000000001", lastUsed = now.minus(Duration.ofDays(31)))
        val recent = fingerprintDirectory("0000000000000002", lastUsed = now.minus(Duration.ofDays(29)))
        store.prune(now, inUse = emptySet())
        assertFalse("unused for 31 days", Files.exists(old))
        assertTrue("used 29 days ago", Files.exists(recent))
    }

    @Test fun aDirectoryInUseIsKeptHoweverLongAgoItWasUsed() {
        val current = fingerprintDirectory("0000000000000003", lastUsed = now.minus(Duration.ofDays(400)))
        val otherProjects = fingerprintDirectory("0000000000000004", lastUsed = now.minus(Duration.ofDays(400)))
        store.prune(now, inUse = setOf(current, otherProjects))
        assertTrue(Files.exists(current))
        assertTrue(Files.exists(otherProjects))
    }

    /** A directory never touched counts from its own time; anything not the store's own is left alone. */
    @Test fun onlyTheStoresOwnDirectoriesArePruned() {
        val untouched = root.resolve("0000000000000005")
        Files.createDirectories(untouched)
        Files.writeString(untouched.resolve("Kernel.ns"), "class Kernel = ()\n")
        Files.setLastModifiedTime(untouched, FileTime.from(now.minus(Duration.ofDays(60))))
        val foreign = root.resolve("notes")
        Files.createDirectories(foreign)
        Files.setLastModifiedTime(foreign, FileTime.from(now.minus(Duration.ofDays(60))))
        store.prune(now, inUse = emptySet())
        assertFalse(Files.exists(untouched))
        assertTrue(Files.exists(foreign))
    }

    @Test fun aTouchRecordsTheUse() {
        val directory = fingerprintDirectory("0000000000000006", lastUsed = now.minus(Duration.ofDays(90)))
        store.touch(directory, now)
        store.prune(now.plus(Duration.ofDays(10)), inUse = emptySet())
        assertTrue(Files.exists(directory))
    }

    @Test fun aFileIsPlacedByItsFingerprintAndItsPathBelowIt() {
        val place = store.placeOf(root.resolve("00000000000000ab").resolve("platform").resolve("TimeFactory.ns"))
        assertEquals("00000000000000ab", place?.fingerprint)
        assertEquals(Path.of("platform", "TimeFactory.ns"), place?.relative)
        assertNull("not a fingerprint", store.placeOf(root.resolve("notes").resolve("Kernel.ns")))
        assertNull("outside the store", store.placeOf(root.resolveSibling("Kernel.ns")))
        assertNull("the directory itself", store.placeOf(root.resolve("00000000000000ab")))
    }

    /**
     * Only a file of the directory the project shows is named to the agent,
     * by its module's name: one of any other fingerprint's -- an older
     * binary's, left behind by a look that failed -- is no module the
     * running binary has, and is not sent at all.
     */
    @Test fun onlyTheShownDirectorysFilesAreNamedToTheAgent() {
        val shown = root.resolve("00000000000000ab")
        val older = root.resolve("00000000000000cd")
        fun sourceOf(path: Path, shownDirectory: Path?) = librarySourceOf(urlOf(path), shownDirectory, store)

        assertEquals(LibrarySource.Module("Kernel.ns"), sourceOf(shown.resolve("Kernel.ns"), shown))
        assertEquals(LibrarySource.Module("TimeFactory.ns"), sourceOf(shown.resolve("platform").resolve("TimeFactory.ns"), shown))
        assertEquals(LibrarySource.Elsewhere, sourceOf(older.resolve("Kernel.ns"), shown))
        assertEquals("nothing shown", LibrarySource.Elsewhere, sourceOf(shown.resolve("Kernel.ns"), null))
        assertEquals("no module", LibrarySource.Elsewhere, sourceOf(shown.resolve(CoreLibraryStore.FINGERPRINT_FILE), shown))
        assertEquals(LibrarySource.Outside, sourceOf(root.resolveSibling("Program.ns"), shown))
        assertEquals(LibrarySource.Outside, librarySourceOf("temp:///Kernel.ns", shown, store))
    }

    @Test fun aFingerprintIsSixteenLowercaseHexDigits() {
        assertTrue(CoreLibraryStore.isFingerprint("0123456789abcdef"))
        assertFalse(CoreLibraryStore.isFingerprint("0123456789ABCDEF"))
        assertFalse(CoreLibraryStore.isFingerprint("0123456789abcde"))
        assertFalse(CoreLibraryStore.isFingerprint("../../etc/passwd"))
    }

    /** The file URL of [path], as a breakpoint holds it. */
    private fun urlOf(path: Path): String = VfsUtilCore.pathToUrl(FileUtil.toSystemIndependentName(path.toString()))

    /** A directory as the store writes one, its files read-only, last used at [lastUsed]. */
    private fun fingerprintDirectory(fingerprint: String, lastUsed: Instant): Path {
        val directory = root.resolve(fingerprint)
        Files.createDirectories(directory.resolve("platform"))
        for (file in listOf(directory.resolve("Kernel.ns"), directory.resolve("platform").resolve("TimeFactory.ns"))) {
            Files.writeString(file, "class ${file.fileName.toString().removeSuffix(".ns")} = ()\n")
            file.toFile().setWritable(false)
        }
        Files.writeString(directory.resolve(CoreLibraryStore.FINGERPRINT_FILE), "$fingerprint\n")
        store.touch(directory, lastUsed)
        return directory
    }
}
