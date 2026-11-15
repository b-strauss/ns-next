package io.github.bstrauss.newspeak.library

import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.application.PathManager
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.roots.AdditionalLibraryRootsListener
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VfsUtil
import com.intellij.openapi.vfs.VfsUtilCore
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.util.concurrency.AppExecutorUtil
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.run.RuntimeLocation
import io.github.bstrauss.newspeak.run.locateRuntime
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import java.io.IOException
import java.nio.file.Files
import java.nio.file.InvalidPathException
import java.nio.file.Path
import java.time.Instant
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Future
import kotlin.io.path.isRegularFile
import kotlin.io.path.name

/**
 * A project's "Newspeak Core Library": the shipped sources of the `ns-next`
 * the settings name, which every run of the project starts, taken from that
 * binary (`ns-next shipped-sources`) into the IDE's system directory, a
 * directory per fingerprint ([CoreLibraryStore]), and shown under External
 * Libraries ([NewspeakCoreLibraryRootsProvider]). It is looked at afresh as
 * the project opens, as the settings change and as a debug session starts,
 * which is a use of the directory too; when the fingerprint changed, the
 * line breakpoints in an older directory's files move to the new one's
 * ([moveLineBreakpoints]). Every look runs on one thread of the
 * application's own, so that two projects never write or prune at once
 * and the event dispatch thread never waits for `ns-next`. A failure is
 * logged and leaves the library out.
 */
@Service(Service.Level.PROJECT)
class NewspeakCoreLibrary(private val project: Project) : Disposable {
    /** The directory shown, the one the settings' `ns-next` wrote; null while there is none. */
    @Volatile
    var directory: Path? = null
        private set

    /** Looks at the binary again on the library's thread; the future answers the directory, or null for none. */
    fun refresh(): Future<Path?> = EXECUTOR.submit<Path?> { refreshNow() }

    private fun refreshNow(): Path? {
        if (project.isDisposed) return null
        val runtime = when (val location = locateRuntime(NewspeakSettings.getInstance().state.runtimePath)) {
            is RuntimeLocation.Found -> location.path
            is RuntimeLocation.ConfiguredPathMissing -> {
                LOG.info("the Newspeak core library is left out: no ns-next at ${location.path}")
                return show(null)
            }
            RuntimeLocation.NotConfigured -> return show(null)
        }
        val ensured = try {
            STORE.ensure(runtime)
        } catch (exception: IOException) {
            LOG.warn("the Newspeak core library is left out: the shipped sources of $runtime could not be written", exception)
            return show(null)
        }
        try {
            STORE.touch(ensured.directory)
        } catch (exception: IOException) {
            LOG.warn("the use of ${ensured.directory} could not be recorded", exception)
        }
        show(ensured.directory)
        STORE.prune(Instant.now(), IN_USE.values.toSet())
        return ensured.directory
    }

    /** Shows [shown] as the library, and moves the line breakpoints into it when it is another directory. */
    private fun show(shown: Path?): Path? {
        val previous = directory
        if (shown == previous) return shown
        val oldRoots = previous?.let(LocalFileSystem.getInstance()::findFileByNioFile)?.let(::listOf).orEmpty()
        val newRoot = shown?.let(LocalFileSystem.getInstance()::refreshAndFindFileByNioFile)
        // Written outside the IDE, the files are seen only once refreshed.
        newRoot?.let { root -> VfsUtil.markDirtyAndRefresh(false, true, true, root) }
        if (shown == null) IN_USE.remove(project) else IN_USE[project] = shown
        directory = shown
        ApplicationManager.getApplication().invokeAndWait(
            {
                if (project.isDisposed) return@invokeAndWait
                WriteAction.run<RuntimeException> {
                    AdditionalLibraryRootsListener.fireAdditionalLibraryChanged(
                        project,
                        NAME,
                        oldRoots,
                        listOfNotNull(newRoot),
                        NAME,
                    )
                }
                if (shown != null) moveLineBreakpoints(project, STORE, shown)
            },
            ModalityState.nonModal(),
        )
        return shown
    }

    /** The root of the library shown, as a file; null while there is none or it is gone. */
    fun root(): VirtualFile? = directory?.let(LocalFileSystem.getInstance()::findFileByNioFile)

    /**
     * The library's file named [name] -- `Kernel.ns`, `TimeFactory.ns` --
     * wherever it lies in the layout; null for none.
     */
    fun fileNamed(name: String): VirtualFile? {
        val shown = directory ?: return null
        val path = try {
            Files.walk(shown).use { stream -> stream.filter { path -> path.isRegularFile() && path.name == name }.findFirst().orElse(null) }
        } catch (_: IOException) {
            null
        } ?: return null
        return LocalFileSystem.getInstance().findFileByNioFile(path) ?: LocalFileSystem.getInstance().refreshAndFindFileByNioFile(path)
    }

    override fun dispose() {
        IN_USE.remove(project)
    }

    companion object {
        private val LOG = Logger.getInstance(NewspeakCoreLibrary::class.java)

        /** The name External Libraries shows. */
        val NAME: String get() = NewspeakBundle.message("library.core.name")

        /** Every directory of shipped sources the IDE wrote: `<system directory>/newspeak/core-library/`. */
        val STORE: CoreLibraryStore by lazy { CoreLibraryStore(PathManager.getSystemDir().resolve("newspeak").resolve("core-library")) }

        /** The directory each open project shows, which no pruning deletes. */
        private val IN_USE = ConcurrentHashMap<Project, Path>()

        /** One thread for every project's look at its binary: writing and pruning never run at once. */
        private val EXECUTOR = AppExecutorUtil.createBoundedApplicationPoolExecutor("Newspeak core library", 1)

        fun getInstance(project: Project): NewspeakCoreLibrary = project.service()

        /**
         * The name the agent knows the module in the library file at
         * [fileUrl] by, `<Module>.ns`; null for a file outside every
         * directory of shipped sources.
         */
        fun moduleSourceNameOf(fileUrl: String): String? {
            if (!fileUrl.startsWith("file:")) return null
            val path = try {
                Path.of(VfsUtilCore.urlToPath(fileUrl))
            } catch (_: InvalidPathException) {
                return null
            }
            val place = STORE.placeOf(path) ?: return null
            return place.relative.fileName.toString().takeIf { name -> name.endsWith(".ns") }
        }
    }
}
