package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.testFramework.LightVirtualFile
import io.github.bstrauss.newspeak.NewspeakFileType
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapSource
import io.github.bstrauss.newspeak.debugger.dap.stringOrNull
import io.github.bstrauss.newspeak.library.LibrarySource
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import io.github.bstrauss.newspeak.library.librarySourceOf
import java.io.IOException
import java.nio.file.Files
import java.nio.file.InvalidPathException
import java.nio.file.Path
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

/**
 * The files a session's frames are shown in. The agent names a program's
 * module by its file's path while its text is the file's, and every other
 * module -- a shipped one, an image's, one an install changed -- by a
 * reference whose `source` request answers its text, stable per module for
 * the session; such a module is shown read-only, as `<Module>.ns`, in one
 * file per reference for the session, so that every frame in it opens the
 * same editor. That file is the project's core library's file of the name
 * ([libraryFile]) when its text is the agent's -- a shipped module of the
 * very binary that runs, whose breakpoints stand there -- and a
 * `LightVirtualFile` of the agent's text otherwise.
 */
class NewspeakSources(
    private val client: DapClient,
    private val libraryFile: (String) -> VirtualFile? = { null },
    private val librarySource: (String) -> LibrarySource = { fileUrl -> librarySourceOf(fileUrl, null, NewspeakCoreLibrary.STORE) },
) {
    private val byReference = ConcurrentHashMap<Int, VirtualFile>()

    /**
     * The file [source] names, or null for none the IDE can show. Blocks
     * on the `source` request the first time a reference is shown, so it is
     * called on a thread of the debugger's own, never on the event
     * dispatch thread nor on the client's reader thread.
     */
    fun fileFor(source: DapSource): VirtualFile? {
        source.sourceReference?.let { reference -> return fileForReference(reference, source.name) }
        val path = source.path ?: return null
        val nioPath = try {
            Path.of(path)
        } catch (_: InvalidPathException) {
            return null
        }
        val files = LocalFileSystem.getInstance()
        return files.findFileByNioFile(nioPath) ?: files.refreshAndFindFileByNioFile(nioPath)
    }

    /**
     * How the agent names [file]: by its reference when this session showed
     * it so, by its name when it is a file of the core library the project
     * shows ([librarySource]), which is how the agent knows a shipped
     * module, by its path when it is any other file on disk but one of
     * another directory of shipped sources; null for any other -- a file of
     * an older binary's shipped sources, a module's text an earlier session
     * showed -- which the agent cannot name.
     */
    fun sourceOf(file: VirtualFile): JsonObject? {
        val reference = byReference.entries.firstOrNull { (_, shown) -> shown == file }?.key
        if (reference != null) {
            return JsonObject().apply {
                addProperty("name", file.name)
                addProperty("sourceReference", reference)
            }
        }
        if (!file.isInLocalFileSystem) return null
        return when (val shipped = librarySource(file.url)) {
            is LibrarySource.Module -> JsonObject().apply { addProperty("name", shipped.name) }
            LibrarySource.Elsewhere -> null
            LibrarySource.Outside -> JsonObject().apply { addProperty("path", file.toNioPath().toString()) }
        }
    }

    private fun fileForReference(reference: Int, name: String?): VirtualFile? {
        byReference[reference]?.let { file -> return file }
        val arguments = JsonObject().apply {
            addProperty("sourceReference", reference)
            add("source", JsonObject().apply { addProperty("sourceReference", reference) })
        }
        val text = try {
            client.request("source", arguments).get(ANSWER_SECONDS, TimeUnit.SECONDS).stringOrNull("content")
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            null
        } catch (_: Exception) {
            null
        } ?: return null
        val file = name?.let(libraryFile)?.takeIf { library -> textOf(library) == text }
            ?: LightVirtualFile(name ?: "$reference.ns", NewspeakFileType, text).apply { isWritable = false }
        return byReference.putIfAbsent(reference, file) ?: file
    }

    /** A library file's text as written, byte for byte the binary's: no document's, whose line ends may differ. */
    private fun textOf(file: VirtualFile): String? = try {
        Files.readString(file.toNioPath())
    } catch (_: IOException) {
        null
    } catch (_: UnsupportedOperationException) {
        null
    }

    private companion object {
        /** The agent answers `source` from text it holds; this bounds a connection that stalls. */
        const val ANSWER_SECONDS = 30L
    }
}
