package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonArray
import com.google.gson.JsonObject
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.StoragePathMacros
import com.intellij.openapi.components.service
import com.intellij.openapi.project.Project
import com.intellij.openapi.project.guessProjectDir
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VfsUtilCore
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.util.xmlb.annotations.Attribute
import com.intellij.util.xmlb.annotations.Tag
import com.intellij.util.xmlb.annotations.XCollection
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import java.nio.file.Path

/**
 * The classes a project's steps do not go into, as Java's "Do not step
 * into the classes": each a module's file and a class in it, chosen from
 * the project's files and the core library's ([NewspeakStepFiltersConfigurable]).
 * Kept with the project's workspace, as its breakpoints are; every change
 * is told on [NewspeakDebuggerSettings.TOPIC], which each running session
 * hears. A disposable parent of every session's subscription to the
 * filters, so that none outlives the project.
 */
@Service(Service.Level.PROJECT)
@State(name = "NewspeakSkippedClasses", storages = [Storage(StoragePathMacros.WORKSPACE_FILE)])
class NewspeakSkippedClasses(private val project: Project) : PersistentStateComponent<NewspeakSkippedClasses.Stored>, Disposable {
    /** The entries as stored. */
    class Stored {
        @get:XCollection(style = XCollection.Style.v2)
        var entries: MutableList<StoredEntry> = ArrayList()
    }

    /** One entry as stored: [SkippedClass]'s parts. */
    @Tag("entry")
    class StoredEntry {
        @get:Attribute
        var library: Boolean = false

        @get:Attribute
        var path: String = ""

        @get:XCollection(style = XCollection.Style.v2)
        var classPath: MutableList<String> = ArrayList()
    }

    /** The entries as they stand, replaced whole, so that a session's thread reads one consistent list. */
    @Volatile
    var entries: List<SkippedClass> = emptyList()
        private set

    override fun getState(): Stored = Stored().apply {
        entries = this@NewspeakSkippedClasses.entries.map { entry ->
            StoredEntry().apply {
                library = entry.inLibrary
                path = entry.path
                classPath = entry.classPath.toMutableList()
            }
        }.toMutableList()
    }

    override fun loadState(state: Stored) {
        entries = state.entries.filter { stored -> stored.path.isNotEmpty() }
            .map { stored -> SkippedClass(stored.library, stored.path, stored.classPath.toList()) }
    }

    /** Changes the entries and tells every running session; a change to nothing tells nobody. */
    fun update(changed: List<SkippedClass>) {
        if (changed == entries) return
        entries = changed
        ApplicationManager.getApplication().messageBus.syncPublisher(NewspeakDebuggerSettings.TOPIC).stepFiltersChanged()
    }

    /** The directory a project file's entry is relative to: the project's own. */
    fun projectDirectory(): VirtualFile? = project.guessProjectDir()

    /** The directory a library file's entry is relative to: the library the project shows now. */
    fun libraryDirectory(): Path? = NewspeakCoreLibrary.getInstance(project).directory

    /**
     * The file [entry] names now, against [projectDirectory] and
     * [libraryDirectory] -- the project's and the library's the project
     * shows, by default; null when it is gone, or no library is shown for
     * a library file's.
     */
    fun fileOf(
        entry: SkippedClass,
        projectDirectory: Path? = projectDirectory()?.let { directory -> directory.fileSystem.getNioPath(directory) },
        libraryDirectory: Path? = libraryDirectory(),
    ): VirtualFile? {
        val path = entry.resolvedAgainst(if (entry.inLibrary) libraryDirectory else projectDirectory) ?: return null
        return LocalFileSystem.getInstance().findFileByNioFile(path)?.takeIf(VirtualFile::isValid)
    }

    /**
     * The entry for [classPath] in [file]: a file of any directory of
     * shipped sources by its path below it, so that it names the same
     * module in the next fingerprint's, and any other by its path below
     * [projectDirectory], or by its own path outside it.
     */
    fun entryFor(file: VirtualFile, classPath: List<String>, projectDirectory: VirtualFile? = projectDirectory()): SkippedClass {
        val own = file.fileSystem.getNioPath(file)
        own?.let(NewspeakCoreLibrary.STORE::placeOf)?.let { place ->
            return SkippedClass(inLibrary = true, path = slashed(place.relative), classPath = classPath)
        }
        val relative = projectDirectory?.let { directory -> VfsUtilCore.getRelativePath(file, directory) }
        return SkippedClass(inLibrary = false, path = relative ?: own?.toString() ?: file.path, classPath = classPath)
    }

    /**
     * `nsNext/setStepFilters`'s `entries`: each entry whose file is there,
     * by its absolute path -- a library file's under the library the
     * project shows now, which the agent takes for the shipped module --
     * and its class path. An entry whose file is gone names nothing the
     * agent could take, and is left out.
     */
    fun arguments(): JsonArray = JsonArray().apply {
        for (entry in entries) {
            val file = fileOf(entry) ?: continue
            add(
                JsonObject().apply {
                    add("source", JsonObject().apply { addProperty("path", file.toNioPath().toString()) })
                    add("classPath", JsonArray().apply { entry.classPath.forEach(::add) })
                },
            )
        }
    }

    override fun dispose() {}

    companion object {
        fun getInstance(project: Project): NewspeakSkippedClasses = project.service()

        private fun slashed(path: Path): String = (0 until path.nameCount).joinToString("/") { index -> path.getName(index).toString() }
    }
}

/**
 * A class steps do not go into: the module of the file at [path] -- below
 * the core library's directory when [inLibrary], so that it names the same
 * module after `ns-next` changes its fingerprint, and below the project's
 * directory otherwise, or absolute for a file outside it -- `/`-separated,
 * and in it the class [classPath] names from the module's root down, the
 * root's own name left out; empty for the whole module.
 */
data class SkippedClass(val inLibrary: Boolean, val path: String, val classPath: List<String>) {
    /** The file's path against [base], the directory the entry is relative to; null without one for a relative entry. */
    fun resolvedAgainst(base: Path?): Path? {
        val own = Path.of(path)
        if (own.isAbsolute && !inLibrary) return own
        return base?.let { directory -> path.split('/').fold(directory) { resolved, part -> resolved.resolve(part) } }
    }

    /** How the list shows the entry: the file, and the class after it. */
    fun presentableText(): String = if (classPath.isEmpty()) path else "$path  ${classPath.joinToString(".")}"
}
