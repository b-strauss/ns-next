package io.github.bstrauss.newspeak.lsp

import com.intellij.codeInsight.daemon.DaemonCodeAnalyzer
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.Key
import com.intellij.openapi.util.Pair
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClient
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.psi.PsiManager
import com.intellij.util.concurrency.AppExecutorUtil
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.DocumentSymbolParams
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

/**
 * The language server's document symbols of each file, kept with the
 * document stamp they were answered for and shared by everything that reads
 * them: the structure view, the play button beside `main:args:` and "Run" in
 * a module's context menu. An answer counts only for the stamp it was given
 * for, so a reader never places anything by the positions of older text.
 */
@Service(Service.Level.PROJECT)
class DocumentSymbolCache(private val project: Project) {
    /** Asks the server for a file's symbols; null with no client yet or no answer in time. Tests replace it. */
    @Volatile
    internal var fetcher: (VirtualFile) -> List<DocumentSymbol>? = ::askServer

    /** The stamp each file's `request` is asking for, by URL: one question in flight per file and stamp. */
    private val requested = ConcurrentHashMap<String, Long>()

    /** The kept answer when it is the current document's; never asks. */
    fun current(file: VirtualFile): List<DocumentSymbol>? {
        val kept = file.getUserData(LAST_ANSWER) ?: return null
        return kept.first.takeIf { kept.second == documentStamp(file) }
    }

    /**
     * Asks now and waits, off the EDT; keeps and answers a usable answer.
     * For callers that ask once per gesture or per tree build -- never a
     * daemon pass, which holds a read action every write action waits for.
     */
    fun fetch(file: VirtualFile, fetchSymbols: (VirtualFile) -> List<DocumentSymbol>? = fetcher): List<DocumentSymbol>? {
        // Read before asking: an answer to older text must not be kept under a newer stamp.
        val stamp = documentStamp(file)
        val answer = usableAnswer(fetchSymbols(file)) ?: return null
        file.putUserData(LAST_ANSWER, Pair.create(answer, stamp))
        return answer
    }

    /**
     * Asks on a pooled thread and calls `onAnswer` there once an answer is
     * kept. While the server has none -- not started yet, or the file not
     * opened with it yet -- it asks again every second, about a minute
     * long; a later edit asks anew. A second request for a file and stamp
     * already in flight is not called back, as every caller passes the same
     * callback, the daemon's restart. Once an answer is kept a new request
     * asks again, so a caller reads `current` first.
     */
    fun request(file: VirtualFile, onAnswer: () -> Unit) {
        val stamp = documentStamp(file)
        if (requested.put(file.url, stamp) == stamp) return
        ask(file, stamp, onAnswer, attempt = 1)
    }

    /**
     * The current answer for a daemon pass, which never waits: without one it
     * asks and restarts the file's daemon once an answer is kept, so the pass
     * runs again with it.
     */
    fun currentForDaemon(file: VirtualFile): List<DocumentSymbol>? =
        current(file) ?: null.also { request(file) { restartDaemon(file) } }

    private fun restartDaemon(file: VirtualFile) {
        ApplicationManager.getApplication().invokeLater({
            // The answer can arrive after the file is deleted; `findFile` logs a warning for an invalid file.
            if (!file.isValid) return@invokeLater
            val psiFile = PsiManager.getInstance(project).findFile(file) ?: return@invokeLater
            DaemonCodeAnalyzer.getInstance(project).restart(psiFile, "Newspeak document symbols arrived")
        }, project.disposed)
    }

    private fun ask(file: VirtualFile, stamp: Long, onAnswer: () -> Unit, attempt: Int) {
        AppExecutorUtil.getAppExecutorService().execute {
            // A later request for newer text has taken over.
            if (requested[file.url] != stamp || project.isDisposed) return@execute
            when {
                fetch(file) != null -> {
                    requested.remove(file.url, stamp)
                    onAnswer()
                }
                attempt < REQUEST_ATTEMPTS -> AppExecutorUtil.getAppScheduledExecutorService().schedule(
                    { ask(file, stamp, onAnswer, attempt + 1) },
                    REQUEST_RETRY_DELAY_MILLISECONDS,
                    TimeUnit.MILLISECONDS,
                )
                else -> requested.remove(file.url, stamp)
            }
        }
    }

    /**
     * Null when there is no client yet or no answer in time. With no client,
     * it starts one: the platform starts the server only for a file opened in
     * an editor, and this file may be merely selected in the project view.
     */
    private fun askServer(file: VirtualFile): List<DocumentSymbol>? {
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

    companion object {
        fun getInstance(project: Project): DocumentSymbolCache = project.service()

        /** The open document's stamp, else the file's: what an answer is kept against. */
        fun documentStamp(file: VirtualFile): Long = ReadAction.computeBlocking<Long, RuntimeException> {
            FileDocumentManager.getInstance().getDocument(file)?.modificationStamp ?: file.modificationStamp
        }

        /** About a minute: long enough for a server to start. */
        private const val REQUEST_ATTEMPTS = 60
        private const val REQUEST_RETRY_DELAY_MILLISECONDS = 1000L
    }
}

/**
 * The last answer for a file and its document stamp. It lives on the
 * `VirtualFile` for the session, so it holds platform types only (the
 * platform's `Pair` and lsp4j's symbols, both loaded by the platform): a
 * plugin class there would keep the plugin's class loader reachable and stop
 * the plugin from unloading without a restart.
 */
private val LAST_ANSWER = Key.create<Pair<List<DocumentSymbol>, Long>>("io.github.bstrauss.newspeak.lsp.documentSymbols")

/**
 * The symbols to keep, or null to ask again: an empty answer is a file the
 * server has not opened yet or could not read, never a file without symbols,
 * since every document it opens or reads answers at least its root class.
 */
fun usableAnswer(symbols: List<DocumentSymbol>?): List<DocumentSymbol>? = symbols?.takeIf { answer -> answer.isNotEmpty() }
