package io.github.bstrauss.newspeak.devtool.editor

import org.junit.After
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files
import java.nio.file.Path

/**
 * [sameFile] over the paths a trace and an editor name one file by: a
 * trace keeps the path its run was given, an editor the one the platform
 * found, and the two differ in spelling wherever a link or a Windows short
 * name stands between them.
 */
class SourcePlacesTest {
    private val directory: Path = Files.createTempDirectory("newspeak-source-places")

    @After
    fun removeDirectory() {
        directory.toFile().deleteRecursively()
    }

    @Test
    fun aPathAndItsNormalFormNameOneFile() {
        val file = Files.writeString(directory.resolve("Demo.ns"), "")
        assertTrue(sameFile(file, directory.resolve("other").resolve("..").resolve("Demo.ns")))
    }

    @Test
    fun twoPathsToOneFileNameOneFile() {
        // A hard link stands in for a symbolic link or a short name: two
        // spellings the text alone cannot tell are one file, made without
        // the privilege a symbolic link needs on Windows.
        val file = Files.writeString(directory.resolve("Demo.ns"), "")
        val link = Files.createLink(directory.resolve("Linked.ns"), file)
        assertTrue(sameFile(file, link))
    }

    @Test
    fun twoFilesAreNotOne() {
        val first = Files.writeString(directory.resolve("First.ns"), "")
        val second = Files.writeString(directory.resolve("Second.ns"), "")
        assertFalse(sameFile(first, second))
    }

    @Test
    fun aMissingFileIsNoOtherFile() {
        val file = Files.writeString(directory.resolve("Demo.ns"), "")
        assertFalse(sameFile(file, directory.resolve("Missing.ns")))
    }
}
