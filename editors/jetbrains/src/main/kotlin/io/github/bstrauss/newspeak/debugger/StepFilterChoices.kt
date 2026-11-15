package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VfsUtil
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.search.FileTypeIndex
import com.intellij.psi.search.GlobalSearchScope
import com.intellij.util.concurrency.AppExecutorUtil
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakFileType
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.SymbolKind
import java.util.concurrent.Callable
import java.util.concurrent.Future
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import javax.swing.tree.DefaultMutableTreeNode
import javax.swing.tree.DefaultTreeModel

/**
 * One choice of the step filter's chooser: a module's file, which stands
 * for every class of the module, or a class in it, which stands for itself
 * and every class nested in it; [entry] what choosing it stores, [label]
 * what the tree shows, its [children] the classes nested in it, as the
 * source nests them. A file's choice carries its [file], whose classes are
 * read when its node is expanded, and a [note] once they could not be.
 */
class StepFilterChoice(
    val entry: SkippedClass,
    val label: String,
    val children: List<StepFilterChoice> = emptyList(),
    val file: VirtualFile? = null,
) {
    /** Said after the label; set on the event dispatch thread. */
    var note: String? = null

    override fun toString(): String = note?.let { said -> "$label  ($said)" } ?: label
}

/** The chooser's two groups of files: the project's modules and the core library's. */
class StepFilterFiles(val project: List<VirtualFile>, val library: List<VirtualFile>)

/**
 * The chooser's files for [project]: its `.ns` files and those of the core
 * library it shows. Read in a read action that waits for the indexes and
 * yields to a write action; asks nothing of the language server, so it is
 * quick, and the caller's progress cancels it.
 */
fun gatherFiles(project: Project): StepFilterFiles =
    ReadAction.nonBlocking(
        Callable {
            val projectFiles = FileTypeIndex.getFiles(NewspeakFileType, GlobalSearchScope.projectScope(project)).sortedBy(VirtualFile::getPath)
            val libraryFiles = NewspeakCoreLibrary.getInstance(project).root()
                ?.let { root -> VfsUtil.collectChildrenRecursively(root).filter { file -> !file.isDirectory && file.name.endsWith(".ns") } }
                .orEmpty()
            StepFilterFiles(projectFiles, libraryFiles)
        },
    ).inSmartMode(project).executeSynchronously()

/**
 * The choices of the classes [symbols] nest below their root class, in
 * [file], recursively, each with the entry [entryOf] gives it.
 */
fun classChoicesOf(
    file: VirtualFile,
    symbols: List<DocumentSymbol>,
    entryOf: (VirtualFile, List<String>) -> SkippedClass,
): List<StepFilterChoice> {
    val root = symbols.firstOrNull { symbol -> symbol.kind == SymbolKind.Class } ?: return emptyList()
    fun nested(symbol: DocumentSymbol, path: List<String>): List<StepFilterChoice> =
        symbol.children.orEmpty().filter { child -> child.kind == SymbolKind.Class }.map { child ->
            val classPath = path + child.name
            StepFilterChoice(entryOf(file, classPath), child.name, nested(child, classPath))
        }
    return nested(root, emptyList())
}

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

/** The child a file's node holds until its classes are read. */
internal object StepFilterPending {
    override fun toString(): String = NewspeakBundle.message("debugger.step.filters.reading")
}

/**
 * The chooser's tree over [files]: a group of the project's modules and
 * one of the core library's, each file a node for its whole module. A
 * file's classes are asked of the language server only when its node is
 * expanded ([read]), on a pooled thread, never on the event dispatch
 * thread, and for at most [timeoutMillis]: a file the server has not
 * answered by then offers its whole module only. Disposing it -- the
 * dialog closed -- cancels every question still asked.
 */
internal class StepFilterTree(
    private val project: Project,
    files: StepFilterFiles,
    private val timeoutMillis: Long = CLASSES_TIMEOUT_MILLISECONDS,
) : Disposable {
    private val skipped = NewspeakSkippedClasses.getInstance(project)

    /** The tree's model, its root hidden. */
    val model = DefaultTreeModel(
        DefaultMutableTreeNode().apply {
            for ((name, group) in listOf(
                NewspeakBundle.message("debugger.step.filters.group.project") to files.project,
                NewspeakBundle.message("debugger.step.filters.group.library") to files.library,
            )) {
                if (group.isEmpty()) continue
                add(DefaultMutableTreeNode(name).apply { group.forEach { file -> add(fileNode(file)) } })
            }
        },
    )

    /** The questions asked, by node, each with its flag of being given up; read and written on the event dispatch thread. */
    private val asked = HashMap<DefaultMutableTreeNode, Pair<Future<*>, AtomicBoolean>>()

    @Volatile
    private var disposed = false

    private fun fileNode(file: VirtualFile): DefaultMutableTreeNode {
        val whole = skipped.entryFor(file, emptyList())
        return DefaultMutableTreeNode(StepFilterChoice(whole, whole.path, file = file)).apply { add(DefaultMutableTreeNode(StepFilterPending)) }
    }

    /**
     * Asks the classes of the file [node] stands for, unless they were
     * read or are being asked; answers the question, for the tests. Called
     * on the event dispatch thread as the node expands.
     */
    fun read(node: DefaultMutableTreeNode): Future<*>? {
        val file = (node.userObject as? StepFilterChoice)?.file ?: return null
        if (!isPending(node) || disposed) return null
        asked[node]?.let { (question, _) -> return question }
        val symbols = DocumentSymbolCache.getInstance(project)
        // Read by the question between its attempts: one given up asks the server nothing more, even where its fetch swallowed the interrupt.
        val givenUp = AtomicBoolean(false)
        val question = AppExecutorUtil.getAppExecutorService().submit {
            val answer = try {
                symbolsWaiting(symbols, file, givenUp::get)
            } catch (_: InterruptedException) {
                null
            }
            show(node, answer)
        }
        asked[node] = question to givenUp
        // The bound: a server that never answers -- its request blocked -- is given up on, whatever its thread does.
        AppExecutorUtil.getAppScheduledExecutorService().schedule(
            {
                if (!question.isDone) {
                    givenUp.set(true)
                    question.cancel(true)
                    show(node, null)
                }
            },
            timeoutMillis,
            TimeUnit.MILLISECONDS,
        )
        return question
    }

    /** Puts [answer]'s classes under [node], or marks the file whole-module only without one; once, the first to arrive. */
    private fun show(node: DefaultMutableTreeNode, answer: List<DocumentSymbol>?) {
        ApplicationManager.getApplication().invokeLater(
            {
                if (disposed || !isPending(node)) return@invokeLater
                val choice = node.userObject as StepFilterChoice
                val file = checkNotNull(choice.file)
                node.removeAllChildren()
                val classes = answer?.let { symbols -> classChoicesOf(file, symbols) { owner, classPath -> skipped.entryFor(owner, classPath) } }
                if (classes == null) choice.note = NewspeakBundle.message("debugger.step.filters.whole.module")
                classes.orEmpty().forEach { child -> node.add(nodeOf(child)) }
                model.nodeStructureChanged(node)
            },
            ModalityState.any(),
        )
    }

    private fun isPending(node: DefaultMutableTreeNode): Boolean =
        node.childCount == 1 && (node.firstChild as DefaultMutableTreeNode).userObject === StepFilterPending

    override fun dispose() {
        disposed = true
        asked.values.forEach { (question, givenUp) ->
            givenUp.set(true)
            question.cancel(true)
        }
        asked.clear()
    }

    private companion object {
        /** Long enough for a server that is starting to answer a file. */
        const val CLASSES_TIMEOUT_MILLISECONDS = 15_000L

        fun nodeOf(choice: StepFilterChoice): DefaultMutableTreeNode =
            DefaultMutableTreeNode(choice).apply { choice.children.forEach { child -> add(nodeOf(child)) } }
    }
}

/**
 * [file]'s symbols, asked of the language server again for a few seconds
 * while it has none: it may be starting. Null when it never answered.
 * An interrupt, or [givenUp] once it answers true -- the question
 * cancelled -- ends it before the next attempt.
 */
internal fun symbolsWaiting(cache: DocumentSymbolCache, file: VirtualFile, givenUp: () -> Boolean = { false }): List<DocumentSymbol>? {
    repeat(SYMBOL_ATTEMPTS) { attempt ->
        if (givenUp() || Thread.currentThread().isInterrupted) return null
        (cache.current(file) ?: cache.fetch(file))?.let { answer -> return answer }
        if (attempt + 1 < SYMBOL_ATTEMPTS) Thread.sleep(SYMBOL_RETRY_MILLISECONDS)
    }
    return null
}

private const val SYMBOL_ATTEMPTS = 20
private const val SYMBOL_RETRY_MILLISECONDS = 500L
