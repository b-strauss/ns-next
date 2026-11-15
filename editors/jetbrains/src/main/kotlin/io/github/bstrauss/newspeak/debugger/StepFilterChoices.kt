package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.progress.ProgressManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VfsUtil
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.search.FileTypeIndex
import com.intellij.psi.search.GlobalSearchScope
import io.github.bstrauss.newspeak.NewspeakFileType
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.SymbolKind

/**
 * One choice of the step filter's chooser: a module's file, which stands
 * for every class of the module, or a class in it, which stands for itself
 * and every class nested in it; [entry] what choosing it stores, [label]
 * what the tree shows, its [children] the classes nested in it, as the
 * source nests them.
 */
class StepFilterChoice(val entry: SkippedClass, val label: String, val children: List<StepFilterChoice>) {
    override fun toString(): String = label
}

/** The chooser's two groups: the project's modules and the core library's. */
class StepFilterChoices(val project: List<StepFilterChoice>, val library: List<StepFilterChoice>)

/**
 * The choices of [files], each with the entry [entryOf] gives it: a file
 * whose classes the language server answered ([symbolsOf]) offers the
 * classes its root class nests, recursively; a file it did not answer, the
 * whole module only.
 */
fun choicesOf(
    files: List<VirtualFile>,
    entryOf: (VirtualFile, List<String>) -> SkippedClass,
    symbolsOf: (VirtualFile) -> List<DocumentSymbol>?,
): List<StepFilterChoice> = files.map { file ->
    val whole = entryOf(file, emptyList())
    val root = symbolsOf(file)?.firstOrNull { symbol -> symbol.kind == SymbolKind.Class }
    fun nested(symbol: DocumentSymbol, path: List<String>): List<StepFilterChoice> =
        symbol.children.orEmpty().filter { child -> child.kind == SymbolKind.Class }.map { child ->
            val classPath = path + child.name
            StepFilterChoice(entryOf(file, classPath), child.name, nested(child, classPath))
        }
    StepFilterChoice(whole, whole.path, root?.let { symbol -> nested(symbol, emptyList()) }.orEmpty())
}.sortedBy(StepFilterChoice::label)

/**
 * Every class path [symbols] declare below their root class, the root's
 * own -- the empty path -- among them: what an entry's class path is
 * checked against.
 */
fun classPathsOf(symbols: List<DocumentSymbol>): Set<List<String>> {
    val root = symbols.firstOrNull { symbol -> symbol.kind == SymbolKind.Class } ?: return emptySet()
    val found = mutableSetOf<List<String>>(emptyList())
    fun walk(symbol: DocumentSymbol, path: List<String>) {
        for (child in symbol.children.orEmpty().filter { child -> child.kind == SymbolKind.Class }) {
            val classPath = path + child.name
            found.add(classPath)
            walk(child, classPath)
        }
    }
    walk(root, emptyList())
    return found
}

/**
 * The chooser's choices for [project]: its `.ns` files and the core
 * library's, each with its classes from the language server. Waits for
 * the server, so it runs under a progress indicator off the event
 * dispatch thread.
 */
fun gatherChoices(project: Project): StepFilterChoices {
    val skipped = NewspeakSkippedClasses.getInstance(project)
    val projectFiles = ReadAction.computeBlocking<List<VirtualFile>, RuntimeException> {
        FileTypeIndex.getFiles(NewspeakFileType, GlobalSearchScope.projectScope(project)).toList()
    }
    val libraryFiles = NewspeakCoreLibrary.getInstance(project).root()
        ?.let { root -> VfsUtil.collectChildrenRecursively(root).filter { file -> !file.isDirectory && file.name.endsWith(".ns") } }
        .orEmpty()
    val symbols = DocumentSymbolCache.getInstance(project)
    // The server is waited for once: when it answered a file, or never answered the first, the others are asked once each.
    var waited = false
    val symbolsOf = { file: VirtualFile ->
        if (waited) {
            symbols.current(file) ?: symbols.fetch(file)
        } else {
            waited = true
            symbolsWaiting(symbols, file)
        }
    }
    return StepFilterChoices(
        project = choicesOf(projectFiles, { file, classPath -> skipped.entryFor(file, classPath) }, symbolsOf),
        library = choicesOf(libraryFiles, { file, classPath -> skipped.entryFor(file, classPath) }, symbolsOf),
    )
}

/**
 * [file]'s symbols, asked of the language server again for a few seconds
 * while it has none: it may be starting. Null when it never answered.
 */
internal fun symbolsWaiting(cache: DocumentSymbolCache, file: VirtualFile): List<DocumentSymbol>? {
    repeat(SYMBOL_ATTEMPTS) { attempt ->
        ProgressManager.checkCanceled()
        (cache.current(file) ?: cache.fetch(file))?.let { answer -> return answer }
        if (attempt + 1 < SYMBOL_ATTEMPTS) Thread.sleep(SYMBOL_RETRY_MILLISECONDS)
    }
    return null
}

private const val SYMBOL_ATTEMPTS = 20
private const val SYMBOL_RETRY_MILLISECONDS = 500L
