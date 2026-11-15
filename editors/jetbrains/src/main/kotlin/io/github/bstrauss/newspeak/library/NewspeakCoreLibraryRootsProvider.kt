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
 * layout -- `platform/` a folder -- its files only the modules: the
 * directory's `fingerprint` and its use marker are the store's own.
 */
class NewspeakCoreLibraryRoots(private val root: VirtualFile) :
    SyntheticLibrary(
        COMPARISON_ID,
        ExcludeFileCondition { isDirectory, name, _, _, _ -> !isDirectory && !name.endsWith(".ns") },
    ),
    ItemPresentation {
    override fun getSourceRoots(): Collection<VirtualFile> = listOf(root)

    override fun getPresentableText(): String = NewspeakCoreLibrary.NAME

    override fun getIcon(unused: Boolean): Icon = NewspeakIcons.FILE

    override fun equals(other: Any?): Boolean = other is NewspeakCoreLibraryRoots && other.root == root

    override fun hashCode(): Int = root.hashCode()

    private companion object {
        /** Names the library across its roots' changes, as the platform compares libraries by it. */
        const val COMPARISON_ID = "io.github.bstrauss.newspeak.coreLibrary"
    }
}

/** Looks at the settings' `ns-next` as a project opens, which shows its library and prunes the store. */
class NewspeakCoreLibraryStartup : ProjectActivity {
    override suspend fun execute(project: Project) {
        NewspeakCoreLibrary.getInstance(project).decideAndRefresh()
    }
}
