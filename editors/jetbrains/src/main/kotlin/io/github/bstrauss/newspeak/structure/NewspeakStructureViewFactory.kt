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
 * again only once the document has changed. The tree computes its children
 * off the EDT; asked on the EDT, the model answers what it already has rather
 * than block the UI on the server.
 *
 * With no client yet, or no answer within the timeout, the tree stays as it
 * was (empty the first time) and the model tells the view to ask again a
 * little later. The platform refreshes its own structure view when a client
 * starts through a listener that is internal API, which the plugin may not
 * use; asking again, a bounded number of times, reaches the same tree once
 * the server is up.
 */
class NewspeakStructureViewModel(
    psiFile: PsiFile,
    private val file: VirtualFile,
    editor: Editor?,
) : TextEditorBasedStructureViewModel(editor, psiFile), StructureViewModel.ElementInfoProvider {
    private class Answer(val symbols: List<DocumentSymbol>, val documentStamp: Long)

    @Volatile private var answer: Answer? = null
    @Volatile private var disposed = false
    private val retryPending = AtomicBoolean(false)
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
        symbols.map { symbol -> NewspeakStructureElement(psiFile.project, file, symbol) }.toTypedArray()

    internal fun symbols(): List<DocumentSymbol> {
        val stamp = documentStamp()
        val known = answer
        if (known != null && known.documentStamp == stamp) return known.symbols
        if (ApplicationManager.getApplication().isDispatchThread) {
            askAgainLater()
            return known?.symbols.orEmpty()
        }
        val fetched = fetch()
        if (fetched == null) {
            askAgainLater()
            return known?.symbols.orEmpty()
        }
        retries.set(0)
        answer = Answer(fetched, stamp)
        return fetched
    }

    private fun documentStamp(): Long =
        FileDocumentManager.getInstance().getDocument(file)?.modificationStamp ?: file.modificationStamp

    /** Null when there is no client yet or no answer in time; the server answers an empty list for a file it has not opened. */
    private fun fetch(): List<DocumentSymbol>? {
        val client = LspClientManager.getInstance(psiFile.project)
            .getClients(NewspeakLspIntegrationProvider::class.java)
            .firstOrNull() ?: return null
        val parameters = DocumentSymbolParams(client.getDocumentIdentifier(file))
        val response = client.sendRequestSync(LspClient.DEFAULT_REQUEST_TIMEOUT_MS) { server ->
            server.textDocumentService.documentSymbol(parameters)
        } ?: return null
        // The server answers the nested form only; a flat `SymbolInformation` never occurs.
        return response.mapNotNull { either -> if (either.isRight) either.right else null }
    }

    private fun askAgainLater() {
        if (disposed || retries.get() >= MAXIMUM_RETRIES || !retryPending.compareAndSet(false, true)) return
        retries.incrementAndGet()
        AppExecutorUtil.getAppScheduledExecutorService().schedule(
            {
                retryPending.set(false)
                ApplicationManager.getApplication().invokeLater({ fireModelUpdate() }, { disposed })
            },
            RETRY_DELAY_MILLISECONDS,
            TimeUnit.MILLISECONDS,
        )
    }

    private companion object {
        /** About a minute: long enough for a server to start, short enough not to poll a missing one forever. */
        const val MAXIMUM_RETRIES = 60
        const val RETRY_DELAY_MILLISECONDS = 1000L
    }
}

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
