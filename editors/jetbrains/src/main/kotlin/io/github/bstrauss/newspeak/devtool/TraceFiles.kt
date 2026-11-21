package io.github.bstrauss.newspeak.devtool

import com.intellij.openapi.application.PathManager
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.diagnostic.thisLogger
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.util.concurrency.AppExecutorUtil
import java.io.IOException
import java.nio.file.Files
import java.nio.file.Path
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/** The directory under the IDE's temporary directory that every traced run's file lies in. */
internal const val TRACE_DIRECTORY = "newspeak-trace"

/**
 * The traced runs' files of every project of the IDE (`docs/design/developer-tool.md`,
 * "The plugin's half"): each made in [directory] and named live until its
 * run lets it go, and the directory swept once per IDE start of every file
 * no live run names -- what an IDE that ended before it could delete a
 * file, or a project closed before its run was registered, left behind.
 * Application-wide, so that one project's sweep never deletes another
 * open project's running trace.
 */
@Service(Service.Level.APP)
class TraceFiles internal constructor(private val directory: Path) {
    constructor() : this(Path.of(PathManager.getTempPath(), TRACE_DIRECTORY))

    /** The files a run of this IDE writes or reads, named before the file exists so that a sweep never takes one. */
    private val live: MutableSet<Path> = ConcurrentHashMap.newKeySet()

    private val swept = AtomicBoolean()

    /**
     * A new file's path, named live: its directory made and nothing left at
     * the path, since a file there would be read before the runtime
     * truncates it.
     */
    fun newFile(): Path {
        val file = directory.resolve("trace-${UUID.randomUUID()}.jsonl")
        live.add(file)
        try {
            Files.createDirectories(directory)
            Files.deleteIfExists(file)
        } catch (exception: IOException) {
            // The runtime refuses a path it cannot create, which the run's console then says.
            thisLogger().warn("cannot prepare the trace file $file", exception)
        }
        return file
    }

    /** [file]'s run is over: the file is deleted, off the EDT, as every file operation of the tool is. */
    fun release(file: Path) {
        AppExecutorUtil.getAppExecutorService().execute {
            try {
                Files.deleteIfExists(file)
            } catch (exception: IOException) {
                thisLogger().info("cannot delete the trace file $file", exception)
            }
            live.remove(file)
        }
    }

    /** Whether [file] is named live. */
    internal fun isLive(file: Path): Boolean = file in live

    /** Deletes, the first time it is called in the IDE's run, every file of the directory no live run names. Never on the EDT. */
    fun sweepOnce() {
        if (!swept.compareAndSet(false, true)) return
        val files = try {
            Files.newDirectoryStream(directory).use { stream -> stream.toList() }
        } catch (_: IOException) {
            // No directory, no file left behind.
            return
        }
        for (file in files) {
            if (file in live) continue
            try {
                Files.deleteIfExists(file)
            } catch (exception: IOException) {
                thisLogger().info("cannot delete the trace file $file", exception)
            }
        }
    }

    companion object {
        fun getInstance(): TraceFiles = service()
    }
}

/** Sweeps the trace files' directory as the first project of the IDE's run opens. */
class TraceFilesSweep : ProjectActivity {
    override suspend fun execute(project: Project) {
        TraceFiles.getInstance().sweepOnce()
    }
}
