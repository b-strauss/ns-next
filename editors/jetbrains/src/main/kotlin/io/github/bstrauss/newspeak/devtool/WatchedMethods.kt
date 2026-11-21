package io.github.bstrauss.newspeak.devtool

import com.google.gson.JsonObject
import com.google.gson.JsonParser
import com.intellij.execution.RunManager
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.StoragePathMacros
import com.intellij.openapi.components.service
import com.intellij.openapi.editor.Document
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.TextRange
import com.intellij.openapi.util.io.FileUtil
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VfsUtilCore
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClient
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.util.xmlb.annotations.Attribute
import com.intellij.util.xmlb.annotations.Tag
import com.intellij.util.xmlb.annotations.XCollection
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import io.github.bstrauss.newspeak.lsp.MethodNamesParameters
import io.github.bstrauss.newspeak.lsp.ModuleIdentityParameters
import io.github.bstrauss.newspeak.lsp.NewspeakLanguageServer
import io.github.bstrauss.newspeak.lsp.NewspeakLspIntegrationProvider
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import io.github.bstrauss.newspeak.run.PROJECT_FILE_NAME
import org.eclipse.lsp4j.Position
import java.util.concurrent.ConcurrentHashMap

/**
 * The project's one list of watched methods (`docs/design/developer-tool.md`,
 * decision 23): each entry a method or a pattern in the tools' syntax, a
 * trailing `...` a prefix, with its switches for events and listings, from
 * which every traced run's `--trace-filter` and `--print-filter` are built.
 * Kept with the project's workspace, as its breakpoints are; made in the
 * editor ([WatchMethodAction], [WatchMethodIntention]) and managed in
 * [WatchedMethodsDialog]. The methods of a file are named by the language
 * server's `newspeak/methodNames`, since the plugin parses no Newspeak.
 */
@Service(Service.Level.PROJECT)
@State(name = "NewspeakWatchedMethods", storages = [Storage(StoragePathMacros.WORKSPACE_FILE)])
class WatchedMethods(private val project: Project) : PersistentStateComponent<WatchedMethods.Stored> {
    /** One entry: its pattern, and whether its events and its listings are asked for. */
    @Tag("watched")
    data class Entry(
        @get:Attribute var pattern: String = "",
        @get:Attribute var events: Boolean = true,
        @get:Attribute var listings: Boolean = true,
    )

    /** The entries as stored. */
    class Stored {
        @get:XCollection(style = XCollection.Style.v2)
        var entries: MutableList<Entry> = ArrayList()
    }

    /** The entries in the order they were made, replaced whole, so that a reader on another thread sees one list. */
    @Volatile
    var entries: List<Entry> = emptyList()
        private set

    /** Asks the server for a file's methods; null with no client yet or no answer in time. Tests replace it. */
    @Volatile
    internal var fetcher: (VirtualFile) -> List<MethodName>? = ::askServer

    /** Each file's last answer and the document stamp it is for, by URL. */
    private val answers = ConcurrentHashMap<String, Pair<List<MethodName>, Long>>()

    /** Adds [pattern] with both switches on; a pattern already listed stays as it is. */
    fun watch(pattern: String) {
        if (entries.none { entry -> entry.pattern == pattern }) entries = entries + Entry(pattern)
    }

    /** Removes the entry of [pattern], if any. */
    fun unwatch(pattern: String) {
        entries = entries.filterNot { entry -> entry.pattern == pattern }
    }

    /** Whether [pattern] is an entry of its own, which [unwatch] removes. */
    fun hasEntry(pattern: String): Boolean = entries.any { entry -> entry.pattern == pattern }

    /** Unwatches [name] where it is an entry, and watches it otherwise: the editor's one action. */
    fun toggle(name: String) {
        if (hasEntry(name)) unwatch(name) else watch(name)
    }

    /** Replaces every entry, as the dialog's OK does; an entry without a pattern is left out. */
    fun replaceEntries(replacement: List<Entry>) {
        entries = replacement.filter { entry -> entry.pattern.isNotBlank() }.map { entry -> entry.copy(pattern = entry.pattern.trim()) }
    }

    /** Whether an entry covers the method [name], in the tools' syntax. */
    fun isWatched(name: String): Boolean = entries.any { entry -> covers(entry.pattern, name) }

    /**
     * The filters a traced run is given: everything for an empty list, and
     * otherwise each kind narrowed to the entries that want it and that the
     * run takes -- of the form, and, given the [modules] of the run's
     * program and the binary, each id with its versions, naming one of
     * them ([runTakes]) -- since `ns-next run` refuses any other with exit
     * status 2. A kind left without a pattern is not asked for at all,
     * since a run without a pattern would trace every body. Without
     * [modules] only the form is checked.
     */
    fun filters(modules: Map<String, Set<String>>? = null): TraceFilters {
        val current = entries
        if (current.isEmpty()) return TraceFilters.EVERYTHING
        val taken = current.filter { entry ->
            if (modules == null) patternModule(entry.pattern) != null else runTakes(entry.pattern, modules)
        }
        val events = taken.filter(Entry::events).map(Entry::pattern)
        val listings = taken.filter(Entry::listings).map(Entry::pattern)
        return TraceFilters(events, listings, noEvents = events.isEmpty(), noListings = listings.isEmpty())
    }

    /** The kept names of [file]'s methods when they are the current document's; never asks. */
    fun currentMethodNames(file: VirtualFile): List<MethodName>? {
        val kept = answers[file.url] ?: return null
        return kept.first.takeIf { kept.second == DocumentSymbolCache.documentStamp(file) }
    }

    /** The document stamp each file's names are being asked for, by URL: one question in flight per file and stamp. */
    private val requested = ConcurrentHashMap<String, Long>()

    /**
     * Asks for [file]'s names on a pooled thread, once per document stamp,
     * so that a caller that never waits -- an action's update, an
     * intention's availability -- finds them kept the next time.
     */
    fun request(file: VirtualFile) {
        val stamp = DocumentSymbolCache.documentStamp(file)
        if (requested.put(file.url, stamp) == stamp) return
        ApplicationManager.getApplication().executeOnPooledThread {
            try {
                if (!project.isDisposed) methodNames(file)
            } finally {
                requested.remove(file.url, stamp)
            }
        }
    }

    /**
     * The names of [file]'s methods in source order, each with its id as the
     * project file gives it ([projectAliasOf]); asks the server and waits,
     * so never on the event dispatch thread. Empty when the server has no
     * answer.
     */
    fun methodNames(file: VirtualFile): List<MethodName> {
        currentMethodNames(file)?.let { kept -> return kept }
        // Read before asking: an answer to older text must not be kept under a newer stamp.
        val stamp = DocumentSymbolCache.documentStamp(file)
        val answer = fetcher(file) ?: return emptyList()
        val alias = projectAliasOf(file, project)
        val named = if (alias == null) answer else answer.map { method -> method.copy(name = withId(method.name, alias)) }
        answers[file.url] = named to stamp
        return named
    }

    override fun getState(): Stored = Stored().apply { entries = this@WatchedMethods.entries.mapTo(ArrayList()) { entry -> entry.copy() } }

    override fun loadState(state: Stored) {
        replaceEntries(state.entries)
    }

    /**
     * The id and version the loader gives the module of [file], as the
     * server answers them; null with no client yet, no answer in time, or
     * a `:module:` the loader refuses. Waits, so never on the event
     * dispatch thread.
     */
    fun moduleIdentity(file: VirtualFile): ModuleIdentity? {
        val client = client() ?: return null
        val parameters = ModuleIdentityParameters(client.getDocumentIdentifier(file))
        val answer = client.sendRequestSync(LspClient.DEFAULT_REQUEST_TIMEOUT_MS) { server ->
            (server as NewspeakLanguageServer).moduleIdentity(parameters)
        } ?: return null
        val id = answer.id ?: return null
        return ModuleIdentity(id, answer.version)
    }

    /** The server's client; with none it starts one, as `DocumentSymbolCache` does, and answers null. */
    private fun client(): LspClient? {
        val client = LspClientManager.getInstance(project)
            .getClients(NewspeakLspIntegrationProvider::class.java)
            .firstOrNull()
        if (client == null) NewspeakLspIntegrationProvider.registered()?.startClient(project)
        return client
    }

    /** Null when there is no client yet or no answer in time; with no client it starts one, as `DocumentSymbolCache` does. */
    private fun askServer(file: VirtualFile): List<MethodName>? {
        val client = client() ?: return null
        val document = ReadAction.computeBlocking<Document?, RuntimeException> {
            FileDocumentManager.getInstance().getDocument(file)
        } ?: return null
        val parameters = MethodNamesParameters(client.getDocumentIdentifier(file))
        val answer = client.sendRequestSync(LspClient.DEFAULT_REQUEST_TIMEOUT_MS) { server ->
            (server as NewspeakLanguageServer).methodNames(parameters)
        } ?: return null
        return ReadAction.computeBlocking<List<MethodName>, RuntimeException> {
            answer.mapNotNull { entry ->
                val name = entry.name ?: return@mapNotNull null
                val range = entry.range?.let { range -> textRange(document, range.start, range.end) } ?: return@mapNotNull null
                val selection = entry.selectionRange?.let { range -> textRange(document, range.start, range.end) } ?: return@mapNotNull null
                MethodName(name, range, selection)
            }
        }
    }

    companion object {
        fun getInstance(project: Project): WatchedMethods = project.service()

        /** What ends a pattern that selects by a prefix, as the runtime's filter reads it. */
        const val PREFIX_MARK = "..."
    }
}

/** A method of a file: its name in the tools' syntax, its declaration's range and its pattern's, in document offsets. */
data class MethodName(val name: String, val range: TextRange, val selection: TextRange)

/** The method whose declaration holds [offset], its ends included, so that a caret just past the body still names it. */
fun methodAt(names: List<MethodName>, offset: Int): MethodName? = names.firstOrNull { method -> method.range.containsOffset(offset) }

/** The method whose declaration spans [line] of [document]: the gutter's line. */
fun methodOnLine(names: List<MethodName>, document: Document, line: Int): MethodName? = names.firstOrNull { method ->
    line in document.getLineNumber(method.range.startOffset)..document.getLineNumber(method.range.endOffset)
}

/** An LSP range of [document], null where it lies outside the document's text. */
private fun textRange(document: Document, start: Position, end: Position): TextRange? {
    fun offset(position: Position): Int? {
        if (position.line < 0 || position.line >= document.lineCount) return null
        // The analyzer counts columns in UTF-16 units, as a document's offsets are.
        val offset = document.getLineStartOffset(position.line) + position.character
        return offset.takeIf { offset <= document.getLineEndOffset(position.line) }
    }
    val startOffset = offset(start) ?: return null
    val endOffset = offset(end) ?: return null
    return if (startOffset <= endOffset) TextRange(startOffset, endOffset) else null
}

/**
 * Whether [pattern] covers the method [name]. One rule for both kinds of
 * pattern: a pattern without a version covers every version of its id, a
 * name's version left out before it is compared; a pattern with one
 * covers that version only, never a name without a version -- as the
 * runtime's filter matches a body (`ModuleName::covers`). A pattern is
 * then the same name, or for a trailing `...` a beginning of it. Only
 * the editor asks this, of names it shows; a run's filter is the
 * runtime's own.
 */
internal fun covers(pattern: String, name: String): Boolean {
    val prefix = pattern.endsWith(WatchedMethods.PREFIX_MARK)
    val stem = pattern.removeSuffix(WatchedMethods.PREFIX_MARK)
    val versioned = '@' in stem.substring(0, idEnd(stem))
    val compared = if (versioned) name else withoutVersion(name)
    return if (prefix) compared.startsWith(stem) else compared == stem
}

/**
 * Where a name's module part ends: at the path's `/`, the side's ` class`
 * or the selector's `>>`, whichever comes first; an id and a version hold
 * none of them, so the first is the end, a selector's own `/` or `>`
 * coming after it.
 */
private fun idEnd(name: String): Int = listOf(name.indexOf('/'), name.indexOf(' '), name.indexOf(">>"))
    .filter { index -> index >= 0 }
    .minOrNull() ?: name.length

/** A name's id, its version or null, and the rest after them. */
private fun parts(name: String): Triple<String, String?, String> {
    val end = idEnd(name)
    val module = name.substring(0, end)
    val at = module.indexOf('@')
    return if (at < 0) Triple(module, null, name.substring(end)) else Triple(module.substring(0, at), module.substring(at + 1), name.substring(end))
}

private fun withoutVersion(name: String): String {
    val (id, _, rest) = parts(name)
    return id + rest
}

/** [name] with its id replaced by [id], its version and the rest as they are. */
internal fun withId(name: String, id: String): String {
    val (_, version, rest) = parts(name)
    return id + (version?.let { "@$it" } ?: "") + rest
}

/**
 * The id a project file gives [file] with an `as`, or null where none does:
 * the project files of the file's directories, nearest first, and then the
 * programs of the project's Newspeak run configurations, the first that
 * lists the file deciding. Reads files, so never on the event dispatch
 * thread.
 */
internal fun projectAliasOf(file: VirtualFile, project: Project? = null): String? {
    val candidates = LinkedHashSet<VirtualFile>()
    generateSequence(file.parent) { directory -> directory.parent }
        .mapNotNull { directory -> directory.findChild(PROJECT_FILE_NAME) }
        .forEach(candidates::add)
    project?.let(RunManager::getInstance)?.allConfigurationsList
        ?.filterIsInstance<NewspeakRunConfiguration>()
        ?.mapNotNull { configuration -> configuration.program?.trim()?.takeIf(String::isNotEmpty) }
        ?.filter { program -> program.endsWith(PROJECT_FILE_NAME) }
        ?.mapNotNull { program -> LocalFileSystem.getInstance().findFileByPath(program.replace('\\', '/')) }
        ?.forEach(candidates::add)
    for (projectFile in candidates) {
        val listed = listedAlias(projectFile, file) ?: continue
        return listed.alias
    }
    return null
}

/** How a project file lists a module: [alias] its `as`, or null for none. */
private class Listed(val alias: String?)

/** How [projectFile] lists [module]; null where it does not, or cannot be read as a project file. */
private fun listedAlias(projectFile: VirtualFile, module: VirtualFile): Listed? {
    val directory = projectFile.parent ?: return null
    val entry = projectModuleEntries(projectFile)?.firstOrNull { entry -> directory.findFileByRelativePath(entry.path) == module } ?: return null
    return Listed(entry.alias)
}

/** A module a project file names: its [path], `/`-separated and relative to the file's directory, and its `as`. */
internal data class ProjectModuleEntry(val path: String, val alias: String?)

/**
 * The modules [projectFile] names -- `main`'s first, then each of
 * `modules` -- as written; null where it cannot be read as a project
 * file, as one being typed cannot, ns-next refusing it at the start.
 */
internal fun projectModuleEntries(projectFile: VirtualFile): List<ProjectModuleEntry>? {
    val root = try {
        JsonParser.parseString(VfsUtilCore.loadText(projectFile)) as? JsonObject
    } catch (_: Exception) {
        null
    } ?: return null
    val entries = mutableListOf<ProjectModuleEntry>()
    root.get("main")?.takeIf { element -> element.isJsonPrimitive }?.asString?.let { main -> entries += ProjectModuleEntry(FileUtil.toSystemIndependentName(main), null) }
    val modules = root.get("modules")?.takeIf { element -> element.isJsonArray }?.asJsonArray ?: return entries
    for (entry in modules) {
        when {
            entry.isJsonPrimitive && entry.asJsonPrimitive.isString -> entries += ProjectModuleEntry(FileUtil.toSystemIndependentName(entry.asString), null)
            entry.isJsonObject -> {
                val fields = entry.asJsonObject
                val path = fields.get("path")?.takeIf { element -> element.isJsonPrimitive }?.asString ?: continue
                entries += ProjectModuleEntry(FileUtil.toSystemIndependentName(path),fields.get("as")?.takeIf { element -> element.isJsonPrimitive }?.asString)
            }
        }
    }
    return entries
}

/** A module's id and version as the loader gives them, the analyzer's `newspeak/moduleIdentity`. */
data class ModuleIdentity(val id: String, val version: String?)

/**
 * The modules a run of [programFile] -- a project's `newspeak.json` or one
 * module's source -- and the binary have, each id with its versions: each
 * module the project file names by the id its `as` gives, else the one
 * [identityOf] answers, and every shipped module. A module whose identity
 * is not known is left out, so a pattern naming it is not passed: a run
 * refused is worse than a watch left out.
 */
fun programModules(programFile: VirtualFile?, identityOf: (VirtualFile) -> ModuleIdentity?): Map<String, Set<String>> {
    val modules = SHIPPED_MODULE_IDS.associateWithTo(LinkedHashMap<String, MutableSet<String>>()) { mutableSetOf() }
    fun add(id: String, version: String?) {
        val versions = modules.getOrPut(id) { mutableSetOf() }
        version?.let(versions::add)
    }
    if (programFile == null) return modules
    if (programFile.name == PROJECT_FILE_NAME) {
        val directory = programFile.parent ?: return modules
        for (entry in projectModuleEntries(programFile).orEmpty()) {
            val file = directory.findFileByRelativePath(entry.path)
            val identity = file?.let(identityOf)
            val id = entry.alias ?: identity?.id ?: continue
            add(id, identity?.version)
        }
    } else {
        identityOf(programFile)?.let { identity -> add(identity.id, identity.version) }
    }
    return modules
}
