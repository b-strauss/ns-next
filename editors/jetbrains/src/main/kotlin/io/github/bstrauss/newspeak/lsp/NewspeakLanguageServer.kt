package io.github.bstrauss.newspeak.lsp

import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.TextDocumentIdentifier
import org.eclipse.lsp4j.jsonrpc.services.JsonRequest
import org.eclipse.lsp4j.services.LanguageServer
import java.util.concurrent.CompletableFuture

/**
 * `ns-next-analyzer` as the platform's LSP client sees it: the protocol's
 * requests and the server's own, which the client's remote proxy can send
 * only when they are declared on the class the descriptor names
 * (`NewspeakLspClientDescriptor.lsp4jServerClass`).
 */
interface NewspeakLanguageServer : LanguageServer {
    /**
     * The lines of an open document a line breakpoint can stand on
     * (`docs/design/editor-support.md`, "Breakable lines"); an unopened
     * document is an error.
     */
    @JsonRequest("newspeak/breakableLines")
    fun breakableLines(parameters: BreakableLinesParameters): CompletableFuture<BreakableLinesAnswer>

    /**
     * Every method of a document named in the tools' syntax, in source
     * order, with its declaration's range and its pattern's; a document
     * never opened is read from disk.
     */
    @JsonRequest("newspeak/methodNames")
    fun methodNames(parameters: MethodNamesParameters): CompletableFuture<List<MethodNamesEntry>>

    /**
     * A document's module id and version as the loader gives them; null
     * for a `:module:` the loader refuses. A document never opened is read
     * from disk.
     */
    @JsonRequest("newspeak/moduleIdentity")
    fun moduleIdentity(parameters: ModuleIdentityParameters): CompletableFuture<ModuleIdentityAnswer?>
}

/** The module identity request's parameters: the document asked about. */
class ModuleIdentityParameters(val textDocument: TextDocumentIdentifier)

/** The module's id and its version, null for a module without one; read by Gson. */
class ModuleIdentityAnswer(val id: String? = null, val version: String? = null)

/** The method names request's parameters: the document asked about. */
class MethodNamesParameters(val textDocument: TextDocumentIdentifier)

/** One method of the answer, read by Gson, which sets the fields whatever the server left out. */
class MethodNamesEntry(val name: String? = null, val range: Range? = null, val selectionRange: Range? = null)

/** The request's parameters: the document asked about. */
class BreakableLinesParameters(val textDocument: TextDocumentIdentifier)

/**
 * The server's answer: [version] the document's version it answered for,
 * absent for a document read from disk, and [lines] null for a document
 * that does not parse or resolve, every line of which is allowed. Read by
 * Gson, which sets the fields whatever the server left out.
 */
class BreakableLinesAnswer(val version: Int? = null, val lines: List<BreakableLinesEntry>? = null)

/** A line a breakable statement starts on, 0-based, and the blocks whose first statement starts on it. */
class BreakableLinesEntry(val line: Int? = null, val variants: List<BreakableLinesVariant>? = null)

/** A block literal's [range] and its first statement's [position]. */
class BreakableLinesVariant(val range: Range? = null, val position: Position? = null)
