package io.github.bstrauss.newspeak.library

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.process.CapturingProcessHandler
import com.intellij.openapi.diagnostic.Logger
import java.io.IOException
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.FileTime
import java.time.Duration
import java.time.Instant
import kotlin.io.path.isDirectory
import kotlin.io.path.isRegularFile
import kotlin.io.path.name

/**
 * The shipped sources of every `ns-next` the IDE ran, each under [root]
 * in a directory named by its fingerprint, as `ns-next shipped-sources`
 * writes them: `core-library/`'s layout, and a `fingerprint` file. A
 * directory is written once, through a staging directory moved into place,
 * so that no reader ever sees one half written -- a write the IDE's exit
 * or a crash cut short leaves only a staging directory, which the pruning
 * deletes; its files are made read-only, as the binary's own text is
 * nothing to edit. Within the IDE every write runs on one thread, for all
 * its projects; a directory that appears in place while one is written
 * could only come from another process using the same system directory,
 * and is taken when it holds the fingerprint.
 *
 * Every use of a directory sets the time of its [MARKER] ([touch]); a
 * directory unused for [UNUSED_LIMIT] is deleted by [prune] unless an open
 * project uses it, so that projects run with different binaries keep
 * theirs and a binary the IDE no longer runs does not keep its sources.
 */
class CoreLibraryStore(val root: Path) {
    /** What [ensure] found: the directory, and whether it was written now. */
    data class Ensured(val directory: Path, val extracted: Boolean)

    /**
     * The directory of [runtime]'s shipped sources, written first when
     * none holds them. Blocks on `ns-next`, so it is never called on the
     * event dispatch thread.
     */
    fun ensure(runtime: Path): Ensured {
        val fingerprint = fingerprintOf(runtime)
        val directory = root.resolve(fingerprint)
        if (holds(directory, fingerprint)) return Ensured(directory, extracted = false)
        // Without its fingerprint file a directory was never completed: an older plugin's, or one damaged by hand.
        if (Files.exists(directory)) deleteRecursively(directory)
        Files.createDirectories(root)
        val staging = root.resolve("$STAGING_PREFIX$fingerprint-${ProcessHandle.current().pid()}-${System.nanoTime()}")
        try {
            run(runtime, listOf("shipped-sources", staging.toString()))
            if (!holds(staging, fingerprint)) {
                throw IOException("$runtime wrote shipped sources whose fingerprint is not the $fingerprint it answered")
            }
            makeReadOnly(staging)
            try {
                Files.move(staging, directory, StandardCopyOption.ATOMIC_MOVE)
            } catch (exception: IOException) {
                // Defensive: only another process using this system directory could have put it there meanwhile.
                if (!holds(directory, fingerprint)) throw exception
                return Ensured(directory, extracted = false)
            }
        } finally {
            if (Files.exists(staging)) deleteRecursively(staging)
        }
        return Ensured(directory, extracted = true)
    }

    /** The fingerprint [runtime] answers, checked to be one before it names a directory. */
    fun fingerprintOf(runtime: Path): String {
        val fingerprint = run(runtime, listOf("shipped-sources", "--fingerprint")).trim()
        if (!isFingerprint(fingerprint)) {
            throw IOException("$runtime answered no fingerprint of its shipped sources but `${fingerprint.take(EXCERPT_LENGTH)}`")
        }
        return fingerprint
    }

    /** Records that [directory] is used now. */
    fun touch(directory: Path, now: Instant = Instant.now()) {
        val marker = directory.resolve(MARKER)
        if (!Files.exists(marker)) Files.createFile(marker)
        Files.setLastModifiedTime(marker, FileTime.from(now))
    }

    /**
     * Deletes every directory of shipped sources -- and every staging
     * directory a writer left -- last used [UNUSED_LIMIT] before [now] or
     * earlier, except those in [inUse]. A directory never touched counts
     * from its own time. Anything else under [root] is left alone, and a
     * directory that cannot be deleted is logged and kept.
     */
    fun prune(now: Instant, inUse: Set<Path>) {
        val kept = inUse.map(::normal).toSet()
        val entries = try {
            Files.list(root).use { stream -> stream.toList() }
        } catch (_: IOException) {
            return
        }
        for (entry in entries) {
            if (!entry.isDirectory() || !(isFingerprint(entry.name) || entry.name.startsWith(STAGING_PREFIX))) continue
            if (normal(entry) in kept) continue
            try {
                val marker = entry.resolve(MARKER)
                val lastUsed = Files.getLastModifiedTime(if (marker.isRegularFile()) marker else entry).toInstant()
                if (Duration.between(lastUsed, now) < UNUSED_LIMIT) continue
                deleteRecursively(entry)
                LOG.info("deleted the shipped sources at $entry, unused since $lastUsed")
            } catch (exception: IOException) {
                LOG.warn("the shipped sources at $entry could not be pruned", exception)
            }
        }
    }

    /**
     * Where [path] lies in the store: the fingerprint of its directory and
     * its path below it, `platform/TimeFactory.ns` say; null for a path
     * outside every fingerprint directory.
     */
    fun placeOf(path: Path): Place? {
        val normalPath = normal(path)
        val normalRoot = normal(root)
        if (!normalPath.startsWith(normalRoot)) return null
        val relative = normalRoot.relativize(normalPath)
        if (relative.nameCount < 2 || !isFingerprint(relative.getName(0).toString())) return null
        return Place(relative.getName(0).toString(), relative.subpath(1, relative.nameCount))
    }

    /** A file's place in the store: [fingerprint]'s directory, and [relative] below it. */
    data class Place(val fingerprint: String, val relative: Path)

    private fun holds(directory: Path, fingerprint: String): Boolean {
        val file = directory.resolve(FINGERPRINT_FILE)
        if (!file.isRegularFile() || !directory.resolve(KERNEL_FILE).isRegularFile()) return false
        return try {
            Files.readString(file).trim() == fingerprint
        } catch (_: IOException) {
            false
        }
    }

    /** Runs [runtime] with [arguments] and answers its standard output; a failure, a time-out included, throws. */
    private fun run(runtime: Path, arguments: List<String>): String {
        val commandLine = GeneralCommandLine(listOf(runtime.toString()) + arguments).withCharset(Charsets.UTF_8)
        val output = try {
            CapturingProcessHandler(commandLine).runProcess(RUN_MILLISECONDS)
        } catch (exception: Exception) {
            throw IOException("$runtime could not be started: ${exception.message}", exception)
        }
        if (output.isTimeout) throw IOException("$runtime ${arguments.joinToString(" ")} did not end within ${RUN_MILLISECONDS / 1000} s")
        if (output.exitCode != 0) {
            throw IOException("$runtime ${arguments.joinToString(" ")} failed with exit code ${output.exitCode}: ${output.stderr.trim()}")
        }
        return output.stdout
    }

    companion object {
        private val LOG = Logger.getInstance(CoreLibraryStore::class.java)

        /** The file whose time is a directory's last use. */
        const val MARKER = "last-used"

        /** The file `ns-next shipped-sources` writes its fingerprint into. */
        const val FINGERPRINT_FILE = "fingerprint"

        /** How long a directory no project uses is kept. */
        val UNUSED_LIMIT: Duration = Duration.ofDays(30)

        private const val KERNEL_FILE = "Kernel.ns"
        private const val STAGING_PREFIX = ".staging-"
        private const val RUN_MILLISECONDS = 60_000
        private const val EXCERPT_LENGTH = 40
        private val FINGERPRINT = Regex("[0-9a-f]{16}")

        /** Whether [name] is a fingerprint as `ns-next shipped-sources --fingerprint` answers one: 16 lowercase hex digits. */
        fun isFingerprint(name: String): Boolean = FINGERPRINT.matches(name)

        private fun normal(path: Path): Path = path.toAbsolutePath().normalize()

        private fun makeReadOnly(directory: Path) {
            Files.walk(directory).use { stream ->
                stream.filter { path -> path.isRegularFile() }.forEach { path -> path.toFile().setWritable(false) }
            }
        }

        /** Deletes [directory] and everything under it, its read-only files included, which Windows refuses to delete otherwise. */
        fun deleteRecursively(directory: Path) {
            val paths = Files.walk(directory).use { stream -> stream.toList() }
            for (path in paths.asReversed()) {
                path.toFile().setWritable(true)
                Files.deleteIfExists(path)
            }
        }
    }
}
