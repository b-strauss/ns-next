package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClient
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.util.concurrency.AppExecutorUtil
import io.github.bstrauss.newspeak.lsp.BreakableLinesAnswer
import io.github.bstrauss.newspeak.lsp.BreakableLinesParameters
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import io.github.bstrauss.newspeak.lsp.NewspeakLanguageServer
import io.github.bstrauss.newspeak.lsp.NewspeakLspIntegrationProvider
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

/**
 * The lines of each file a line breakpoint can stand on, as the language
 * server's `newspeak/breakableLines` answers them, kept with the document
 * stamp they were answered for: the compiler alone knows where a statement
 * compiled to an instruction, and the platform asks before any session
 * runs (`docs/design/debugger.md`, "A breakpoint names a source position").
 * An answer counts only for its stamp, so a gutter click never decides by
 * the lines of older text.
 */
@Service(Service.Level.PROJECT)
class BreakableLinesCache(private val project: Project) {
    /** Asks the server for a file's lines; null with no client yet or no answer in time. Tests replace it. */
    @Volatile
    internal var fetcher: (VirtualFile) -> BreakableLines? = ::askServer

    /** Each file's last answer and the stamp it is for, by URL. */
    private val answers = ConcurrentHashMap<String, Pair<BreakableLines, Long>>()

    /** The stamp each file's question is asking for, by URL: one question in flight per file and stamp. */
    private val requested = ConcurrentHashMap<String, Long>()

    /** The kept answer when it is the current document's; never asks. */
    fun current(file: VirtualFile): BreakableLines? {
        val (answer, stamp) = answers[file.url] ?: return null
        return answer.takeIf { stamp == DocumentSymbolCache.documentStamp(file) }
    }

    /**
     * The current answer, never waiting: without one it asks in the
     * background, and the caller allows every line meanwhile -- a gutter
     * click is never refused for want of an answer.
     */
    fun currentOrAsk(file: VirtualFile): BreakableLines? = current(file) ?: null.also { request(file) }

    /** Asks now and waits, off the event dispatch thread; keeps and answers an answer. */
    fun fetch(file: VirtualFile): BreakableLines? {
        // Read before asking: an answer to older text must not be kept under a newer stamp.
        val stamp = DocumentSymbolCache.documentStamp(file)
        val answer = fetcher(file) ?: return null
        answers[file.url] = answer to stamp
        return answer
    }

    /**
     * Asks on a pooled thread, again every second while the server has no
     * answer -- not started yet, or the file not opened with it yet --
     * about a minute long; a later edit asks anew.
     */
    fun request(file: VirtualFile) {
        val stamp = DocumentSymbolCache.documentStamp(file)
        if (requested.put(file.url, stamp) == stamp) return
        ask(file, stamp, attempt = 1)
    }

    private fun ask(file: VirtualFile, stamp: Long, attempt: Int) {
        AppExecutorUtil.getAppExecutorService().execute {
            // A later request for newer text has taken over.
            if (requested[file.url] != stamp || project.isDisposed) return@execute
            when {
                fetch(file) != null -> requested.remove(file.url, stamp)
                attempt < REQUEST_ATTEMPTS -> AppExecutorUtil.getAppScheduledExecutorService().schedule(
                    { ask(file, stamp, attempt + 1) },
                    REQUEST_RETRY_DELAY_MILLISECONDS,
                    TimeUnit.MILLISECONDS,
                )
                else -> requested.remove(file.url, stamp)
            }
        }
    }

    /**
     * Null when there is no client yet, no answer in time, or an answer for
     * another version of the document than the one the client holds -- the
     * server not yet told of an edit. With no client, it starts one, as
     * `DocumentSymbolCache` does.
     */
    private fun askServer(file: VirtualFile): BreakableLines? {
        // Only a file on disk is a document the server opens; any other is allowed every line.
        if (!file.isInLocalFileSystem) return BreakableLines.EVERY_LINE
        val client = LspClientManager.getInstance(project)
            .getClients(NewspeakLspIntegrationProvider::class.java)
            .firstOrNull()
        if (client == null) {
            NewspeakLspIntegrationProvider.registered()?.startClient(project)
            return null
        }
        val document = ReadAction.computeBlocking<_, RuntimeException> { FileDocumentManager.getInstance().getDocument(file) }
        val expectedVersion = document?.let(client::getDocumentVersion)
        val parameters = BreakableLinesParameters(client.getDocumentIdentifier(file))
        val answer = client.sendRequestSync(LspClient.DEFAULT_REQUEST_TIMEOUT_MS) { server ->
            (server as NewspeakLanguageServer).breakableLines(parameters)
        } ?: return null
        if (answer.version != null && expectedVersion != null && answer.version != expectedVersion) return null
        return BreakableLines.of(answer)
    }

    companion object {
        fun getInstance(project: Project): BreakableLinesCache = project.service()

        /** About a minute: long enough for a server to start. */
        private const val REQUEST_ATTEMPTS = 60
        private const val REQUEST_RETRY_DELAY_MILLISECONDS = 1000L
    }
}

/** Where line breakpoints may stand in one version of a document: [lines] by number, from 0, or null for every line. */
class BreakableLines private constructor(private val lines: Map<Int, List<BlockPlace>>?) {
    /** Whether a line breakpoint may stand on [line], counted from 0. */
    fun allows(line: Int): Boolean = lines == null || lines.containsKey(line)

    /** The blocks whose first statement starts on [line], in order. */
    fun blocksOn(line: Int): List<BlockPlace> = lines?.get(line).orEmpty()

    companion object {
        /** A document that does not parse or resolve: nothing is known, so nothing is refused. */
        val EVERY_LINE = BreakableLines(null)

        fun of(lines: List<BreakableLine>): BreakableLines = BreakableLines(lines.associate { line -> line.line to line.blocks })

        /** The server's answer, its malformed entries left out. */
        fun of(answer: BreakableLinesAnswer): BreakableLines {
            val lines = answer.lines ?: return EVERY_LINE
            return of(
                lines.mapNotNull { entry ->
                    val line = entry.line ?: return@mapNotNull null
                    val blocks = entry.variants.orEmpty().mapNotNull { variant ->
                        val range = variant.range ?: return@mapNotNull null
                        val position = variant.position ?: return@mapNotNull null
                        BlockPlace(range, position)
                    }
                    BreakableLine(line, blocks)
                },
            )
        }
    }
}

/** A line a breakable statement starts on, from 0, and the blocks whose first statement starts on it. */
data class BreakableLine(val line: Int, val blocks: List<BlockPlace>)

/** A block literal's [range] and where its first statement starts, [position]: a line breakpoint's variant. */
data class BlockPlace(val range: Range, val position: Position)
