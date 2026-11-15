package io.github.bstrauss.newspeak.library

import com.intellij.navigation.ItemPresentation
import com.intellij.openapi.project.Project
import com.intellij.openapi.roots.AdditionalLibraryRootsProvider
import com.intellij.openapi.roots.SyntheticLibrary
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.openapi.vfs.VirtualFile
import io.github.bstrauss.newspeak.NewspeakIcons
import javax.swing.Icon

/** The project's "Newspeak Core Library" under External Libraries, while there is one ([NewspeakCoreLibrary]). */
class NewspeakCoreLibraryRootsProvider : AdditionalLibraryRootsProvider() {
    override fun getAdditionalProjectLibraries(project: Project): Collection<SyntheticLibrary> =
        NewspeakCoreLibrary.getInstance(project).root()?.let { root -> listOf(NewspeakCoreLibraryRoots(root)) }.orEmpty()

    override fun getRootsToWatch(project: Project): Collection<VirtualFile> =
        NewspeakCoreLibrary.getInstance(project).root()?.let(::listOf).orEmpty()
}

/**
 * The library over one directory of shipped sources, in `core-library/`'s
 * layout: its roots are the directory's modules and its folders --
 * `Kernel.ns`, `Manifest.ns`, `PlatformFactory.ns` and `platform/` -- so
 * that External Libraries shows them directly under the library, with
 * neither the fingerprint's directory nor the store's own files, its
 * `fingerprint` and its use marker, which lie under no root.
 */
class NewspeakCoreLibraryRoots(private val root: VirtualFile) :
    SyntheticLibrary(
        COMPARISON_ID,
        ExcludeFileCondition { isDirectory, name, _, _, _ -> !isDirectory && !name.endsWith(".ns") },
    ),
    ItemPresentation {
    override fun getSourceRoots(): Collection<VirtualFile> = coreLibrarySourceRoots(root)

    override fun getPresentableText(): String = NewspeakCoreLibrary.NAME

    override fun getIcon(unused: Boolean): Icon = NewspeakIcons.FILE

    override fun equals(other: Any?): Boolean = other is NewspeakCoreLibraryRoots && other.root == root

    override fun hashCode(): Int = root.hashCode()

    private companion object {
        /** Names the library across its roots' changes, as the platform compares libraries by it. */
        const val COMPARISON_ID = "io.github.bstrauss.newspeak.coreLibrary"
    }
}

/**
 * The roots of the library over [directory]: every folder and every module
 * directly in it, the store's files left out. A directory is written once
 * and never changes ([CoreLibraryStore]), so its entries are its roots for
 * as long as it is shown.
 */
fun coreLibrarySourceRoots(directory: VirtualFile): List<VirtualFile> =
    directory.children.filter { entry -> entry.isDirectory || entry.name.endsWith(".ns") }.sortedBy(VirtualFile::getName)

/** Looks at the settings' `ns-next` as a project opens, which shows its library and prunes the store. */
class NewspeakCoreLibraryStartup : ProjectActivity {
    override suspend fun execute(project: Project) {
        NewspeakCoreLibrary.getInstance(project).decideAndRefresh()
    }
}
