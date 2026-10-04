package io.github.bstrauss.newspeak.structure

import com.intellij.ide.projectView.PresentationData
import com.intellij.ide.structureView.StructureViewBuilder
import com.intellij.ide.structureView.StructureViewModel
import com.intellij.ide.structureView.StructureViewTreeElement
import com.intellij.ide.structureView.TextEditorBasedStructureViewModel
import com.intellij.ide.structureView.TreeBasedStructureViewBuilder
import com.intellij.lang.PsiStructureViewFactory
import com.intellij.navigation.ItemPresentation
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.editor.Document
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.util.Key
import com.intellij.openapi.util.Pair
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClient
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.platform.lsp.util.getOffsetInDocument
import com.intellij.psi.PsiFile
import com.intellij.util.concurrency.AppExecutorUtil
import io.github.bstrauss.newspeak.lsp.NewspeakLspIntegrationProvider
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.DocumentSymbolParams
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * The Structure tool window and the File Structure popup for a Newspeak file,
 * built from the language server's `textDocument/documentSymbol`. The
 * platform has a structure view for any language server, but it draws one
 * icon per symbol kind and cannot show a member's access; this one draws the
 * access icon beside the kind's, as Java's does (`NewspeakStructureElement`).
 */
class NewspeakStructureViewFactory : PsiStructureViewFactory {
    override fun getStructureViewBuilder(psiFile: PsiFile): StructureViewBuilder? {
        val file = psiFile.virtualFile ?: return null
        return object : TreeBasedStructureViewBuilder() {
            override fun createStructureViewModel(editor: Editor?): StructureViewModel =
                NewspeakStructureViewModel(psiFile, file, editor)

            // A module's one root class is the top of the tree; a row for the file above it adds nothing.
            override fun isRootNodeShown(): Boolean = false
        }
    }
}

/**
 * Asks the server lazily, when the tree asks for the file's children, and
 * again only once the document has changed. The answer is kept on the file
 * with the document stamp it was given for, so every model of the file --
 * the tool window rebuilds its model each time it shows the file -- answers
 * an unchanged document at once, without asking. The tree computes its
 * children off the EDT; asked on the EDT, the model answers what it already
 * has rather than block the UI on the server.
 *
 * With no client yet, no answer within the timeout, or an empty answer, the
 * tree stays as it was (empty the first time) and the model tells the view to
 * ask again a little later. With no client it starts one, since the platform
 * starts the server only for a file opened in an editor. The server answers
 * at least the root class for every document it has opened
 * (`crates/ns-analyzer/src/document_symbols.rs`), and for a file not open in an
 * editor it reads the file from disk, so an empty answer means a file the
 * server cannot read, or one whose `didOpen` it has not had yet -- the race of
 * a file opened with the Structure tool window already showing, or of editors
 * restored at IDE start. The platform refreshes its own structure
 * view when a client starts through a listener that is internal API, which
 * the plugin may not use; asking again reaches the same tree once the server
 * is up. It asks every second for about a minute, then every ten seconds for
 * as long as the view lives, so a server that starts much later (its path
 * set afterwards, or restarted by hand) still fills the tree without an edit;
 * each new model (another file, the view reopened) starts at the fast pace
 * again.
 */
class NewspeakStructureViewModel(
    psiFile: PsiFile,
    private val file: VirtualFile,
    editor: Editor?,
    /** Asks for the file's symbols in place of the language server; for tests. */
    private val fetcher: ((VirtualFile) -> List<DocumentSymbol>?)? = null,
) : TextEditorBasedStructureViewModel(editor, psiFile), StructureViewModel.ElementInfoProvider {
    @Volatile private var answer: Answer? = null
    @Volatile private var disposed = false
    private val retryPending = AtomicBoolean(false)

    /** Whether a question is scheduled to be asked again; set before `symbols()` returns. */
    internal val asksAgain: Boolean get() = retryPending.get()
    private val retries = AtomicInteger(0)
    private val root = NewspeakStructureRoot(psiFile, this)

    override fun getRoot(): StructureViewTreeElement = root

    override fun isAlwaysShowsPlus(element: StructureViewTreeElement): Boolean = false

    override fun isAlwaysLeaf(element: StructureViewTreeElement): Boolean = false

    override fun dispose() {
        disposed = true
        super.dispose()
    }

    /** The innermost symbol whose range holds the caret, from what the server last answered. */
    override fun getCurrentEditorElement(): Any? {
        val editor = editor ?: return null
        val symbols = answer?.symbols ?: return null
        return innermostSymbolAt(editor.document, symbols, editor.caretModel.offset)
    }

    internal fun elementsOf(symbols: List<DocumentSymbol>): Array<StructureViewTreeElement> =
        // The server answers one top-level symbol, the module's class; only
        // that one is the root class.
        symbols.mapIndexed { index, symbol ->
            NewspeakStructureElement(psiFile.project, file, symbol, root = index == 0)
        }.toTypedArray()

    internal fun symbols(): List<DocumentSymbol> {
        val stamp = documentStamp()
        val known = answer
        if (known != null && known.documentStamp == stamp) return known.symbols
        // The Structure tool window builds a new model each time it shows the
        // file (selected anew in the Project view, the window reopened); a
        // model that starts empty would show an empty tree until the server
        // has answered, then the tree, a flicker on every rebuild. The last
        // answer is kept on the file, so a new model of an unchanged document
        // shows it at once.
        val shared = file.getUserData(LAST_ANSWER)
        if (shared != null && shared.second == stamp) {
            answer = Answer(shared.first, shared.second)
            return shared.first
        }
        if (ApplicationManager.getApplication().isDispatchThread) {
            askAgainLater()
            return known?.symbols.orEmpty()
        }
        val fetched = usableAnswer(fetch())
        if (fetched == null) {
            askAgainLater()
            return known?.symbols.orEmpty()
        }
        retries.set(0)
        answer = Answer(fetched, stamp)
        file.putUserData(LAST_ANSWER, Pair.create(fetched, stamp))
        return fetched
    }

    private fun documentStamp(): Long =
        FileDocumentManager.getInstance().getDocument(file)?.modificationStamp ?: file.modificationStamp

    /**
     * Null when there is no client yet or no answer in time. With no client,
     * it starts one: the platform starts the server only for a file opened in
     * an editor, and this file may be merely selected in the project view.
     */
    private fun fetch(): List<DocumentSymbol>? {
        fetcher?.let { fetchSymbols -> return fetchSymbols(file) }
        val project = psiFile.project
        val client = LspClientManager.getInstance(project)
            .getClients(NewspeakLspIntegrationProvider::class.java)
            .firstOrNull()
        if (client == null) {
            NewspeakLspIntegrationProvider.registered()?.startClient(project)
            return null
        }
        val parameters = DocumentSymbolParams(client.getDocumentIdentifier(file))
        val response = client.sendRequestSync(LspClient.DEFAULT_REQUEST_TIMEOUT_MS) { server ->
            server.textDocumentService.documentSymbol(parameters)
        } ?: return null
        // The server answers the nested form only; a flat `SymbolInformation` never occurs.
        return response.mapNotNull { either -> if (either.isRight) either.right else null }
    }

    private fun askAgainLater() {
        if (disposed || !retryPending.compareAndSet(false, true)) return
        val delay = if (retries.incrementAndGet() <= FAST_RETRIES) FAST_RETRY_DELAY_MILLISECONDS else SLOW_RETRY_DELAY_MILLISECONDS
        AppExecutorUtil.getAppScheduledExecutorService().schedule(
            {
                retryPending.set(false)
                ApplicationManager.getApplication().invokeLater({ fireModelUpdate() }, { disposed })
            },
            delay,
            TimeUnit.MILLISECONDS,
        )
    }

    private companion object {
        /** About a minute at the fast pace: long enough for a server to start. */
        const val FAST_RETRIES = 60
        const val FAST_RETRY_DELAY_MILLISECONDS = 1000L

        /** After that, a missing server costs one request every ten seconds while the view is open. */
        const val SLOW_RETRY_DELAY_MILLISECONDS = 10_000L
    }
}

/** What the server answered for a file, and the document stamp it answered for. */
private class Answer(val symbols: List<DocumentSymbol>, val documentStamp: Long)

/**
 * The last answer for a file and its document stamp, shared by every model
 * of it. It lives on the `VirtualFile` for the session, so it holds platform
 * types only (the platform's `Pair` and lsp4j's symbols, both loaded by the
 * platform): a plugin class there would keep the plugin's class loader
 * reachable and stop the plugin from unloading without a restart.
 */
private val LAST_ANSWER = Key.create<Pair<List<DocumentSymbol>, Long>>("io.github.bstrauss.newspeak.structure.answer")

/** The file, above the module's root class; not shown (`isRootNodeShown`). */
private class NewspeakStructureRoot(
    private val psiFile: PsiFile,
    private val model: NewspeakStructureViewModel,
) : StructureViewTreeElement {
    override fun getValue(): PsiFile = psiFile

    override fun getPresentation(): ItemPresentation =
        psiFile.presentation ?: PresentationData(psiFile.name, null, psiFile.getIcon(0), null)

    override fun getChildren(): Array<StructureViewTreeElement> = model.elementsOf(model.symbols())

    override fun navigate(requestFocus: Boolean) = psiFile.navigate(requestFocus)

    override fun canNavigate(): Boolean = psiFile.canNavigate()

    override fun canNavigateToSource(): Boolean = psiFile.canNavigateToSource()
}

/**
 * The symbols to show, or null to keep the tree and ask again: an empty answer
 * is a file the server has not opened yet or could not read, never a file
 * without symbols, since every document it opens or reads answers at least
 * its root class.
 */
fun usableAnswer(symbols: List<DocumentSymbol>?): List<DocumentSymbol>? = symbols?.takeIf { answer -> answer.isNotEmpty() }

/** The deepest symbol whose range holds `offset`, its end included, so a caret just past a `)` still selects it. */
fun innermostSymbolAt(document: Document, symbols: List<DocumentSymbol>, offset: Int): DocumentSymbol? {
    for (symbol in symbols) {
        val range = symbol.range ?: continue
        val start = getOffsetInDocument(document, range.start) ?: continue
        val end = getOffsetInDocument(document, range.end) ?: continue
        if (offset in start..end) {
            return innermostSymbolAt(document, symbol.children.orEmpty(), offset) ?: symbol
        }
    }
    return null
}
