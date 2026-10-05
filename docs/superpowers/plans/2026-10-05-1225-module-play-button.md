# Running a module from its `main:args:` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Run/Benchmark play button beside a public instance-side
`main:args:` of a `.ns` module whose root class is instantiated with `new`,
"Run" in such a file's context menu, and a "Newspeak" run configuration that
takes a `.ns` file as well as a `newspeak.json`.

**Architecture:** The plugin decides "runnable" from the analyzer's document
symbols, which already carry everything (the root symbol, its `Constructor`
named `new`, a `Method` whose pattern's selector is `main:args:` with `detail` `public`); the analyzer
only gains a test pinning that contract. The structure view's per-file
symbol cache becomes a project service, `DocumentSymbolCache`, read without
blocking by the line marker (which asks asynchronously and restarts the
daemon on the answer) and by the context-menu producer (which may ask
synchronously off the EDT). The run configuration's `projectFile` becomes
`program`, a `newspeak.json` (ns-next is given its directory) or a `.ns`
file (ns-next is given the file).

**Tech Stack:** Kotlin, IntelliJ Platform 2026.1.4+/2026.2.3 (native LSP
API, lsp4j types), Gradle wrapper in `editors/jetbrains/`; Rust for the one
analyzer test.

**Spec:** `docs/superpowers/specs/2026-10-05-module-play-button-design.md`

## Global Constraints

- Communicate with the user in German; all code, comments, identifiers,
  commit messages and documentation in English.
- Names are written out: no identifier of one or two characters, no
  truncated word (`index`, `arguments`, `document`, not `i`, `args`, `doc`);
  lambda parameters are named (`{ symbol -> … }`), never `it`, as the
  surrounding code does.
- Comments explain the decision, what a check prevents, not the mechanics;
  match the comment density of the neighbouring code.
- A plugin class never references a class of the JSON plugin unless it is
  registered in `newspeak-json.xml` (the JSON plugin is an optional
  dependency). In particular, do not reference
  `NewspeakJsonSchemaProviderFactory.PROJECT_FILE_NAME` from the `run`
  package's always-loaded classes; Task 3 adds the run package's own
  constant.
- Never put a plugin class into `VirtualFile` user data; only platform types
  (`com.intellij.openapi.util.Pair`, lsp4j `DocumentSymbol`, `Long`).
- Conventional commits; every commit message ends with
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DhtAcwQi6qo4racmc1iK4i
  ```
  Never override the author or committer identity.
- Work on the checked-out branch `feature/module-play-button` in
  `D:\Workspace\ns-next`; no worktree.
- Run every command in the foreground, never in the background.
- **The commit gates** (CLAUDE.md, "Working in this repository"), run from
  the repository root before every commit of this plan, all green:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo nextest run --workspace
  cargo test --workspace --doc
  cargo run -p ns-analyzer -- parse-corpus corpus/modules   # "parsed 211, failed 0"
  ./editors/jetbrains/gradlew -p editors/jetbrains check
  ```
  A commit that changes only Markdown runs none of them. These are the
  per-commit gates only; the end-of-batch gates (stress modes, `run-corpus`,
  the benchmarks profile, `verifyPlugin`) belong to the `finishing-a-batch`
  skill after the last task, so a task's commit is work in progress until
  then.
- The plugin tests run in `./editors/jetbrains/gradlew -p editors/jetbrains
  test`; one class at a time with `--tests
  'io.github.bstrauss.newspeak.run.NewspeakRunConfigurationTest'`.

## Review Focus

1. **The button after an edit.** After typing in a runnable module, the
   button must come back without a further edit: the line marker's
   asynchronous request must restart the daemon. Task 2 tests that a
   request calls back once the answer is stored, and retries while the
   server has no answer; Task 7 checks the restart by hand.
2. **A stale answer never places a button.** An answer stored for an older
   document stamp must not be `current`, or the button sits on the wrong
   line. Task 2 tests `current` after an edit.
3. **Columns past ASCII.** The server's positions are UTF-16; a line with a
   character outside the BMP before `main:` must still put the button on
   `main:`. Task 4 tests it.
4. **A `.ns` program with a script**, from a hand-edited workspace file, is
   refused by the check rather than started; the form drops a script typed
   for a `.ns` program. Task 3 tests both.
5. **A program that is neither** a `newspeak.json` nor a `.ns` file (a
   `.txt`, a directory, the root) is refused with the message naming both
   kinds, never handed to ns-next. Task 3 tests it.

---

### Task 1: Pin the symbols contract the plugin reads (analyzer test)

The plugin will treat a module as runnable when the root symbol has a
`Constructor` child named `new` and a `Method` child whose selector is
`main:args:` and whose `detail` is exactly `public`. The server names a
method by its pattern as written (`pattern_text`: `main: platform args:
arguments`), so the plugin derives the selector from the name. Pin that in the analyzer so a change to
the symbols cannot silently remove the button.

**Files:**
- Modify: `crates/ns-analyzer/src/document_symbols.rs` (the `tests` module,
  after `a_member_shows_its_effective_access`)

**Interfaces:**
- Consumes: `encode`, `symbols_of` (test helper), `LineIndex`, as
  `a_member_shows_its_effective_access` uses them.
- Produces: nothing for later tasks; the contract it pins is consumed by
  Task 4's `runnableMain`.

- [ ] **Step 1: Write the test**

Add to the `tests` module of `crates/ns-analyzer/src/document_symbols.rs`:

```rust
    /// The JetBrains plugin puts a play button beside `main:args:` when the
    /// root's factory is `new` -- written or omitted -- and `main:args:` is a
    /// public instance-side method (`editors/jetbrains/.../run/NewspeakModuleRun.kt`,
    /// `runnableMain`); these are the names, kinds and details it reads -- a
    /// method named by its pattern, whose selector the plugin takes apart.
    #[test]
    fn a_runnable_root_shows_its_factory_new_and_a_public_instance_side_main() {
        let members = |text: &str| -> Vec<(String, SymbolKind, Option<String>)> {
            let parsed = ns_syntax::parse_for_editor(text);
            let line_index = LineIndex::new(text);
            let encoded = encode(&symbols_of(&parsed.module), text, &line_index);
            encoded[0]
                .children
                .as_ref()
                .expect("children")
                .iter()
                .map(|member| (member.name.clone(), member.kind, member.detail.clone()))
                .collect()
        };
        let omitted = indoc! {"
            class Hello = () (
                public main: platform args: arguments = ( ^0 )
            ) : (
                public main: platform args: arguments = ( ^0 )
            )
        "};
        let expected = vec![
            (
                "new".to_string(),
                SymbolKind::CONSTRUCTOR,
                Some("public".to_string()),
            ),
            (
                "main: platform args: arguments".to_string(),
                SymbolKind::METHOD,
                Some("public".to_string()),
            ),
            (
                "main: platform args: arguments".to_string(),
                SymbolKind::METHOD,
                Some("public class".to_string()),
            ),
        ];
        assert_eq!(members(omitted), expected);
        let written = omitted.replacen("class Hello =", "class Hello new =", 1);
        assert_eq!(members(&written), expected);
        let protected = indoc! {"
            class Hello = () (
                main: platform args: arguments = ( ^0 )
            )
        "};
        assert_eq!(
            members(protected)[1],
            (
                "main: platform args: arguments".to_string(),
                SymbolKind::METHOD,
                Some("protected".to_string()),
            )
        );
    }
```

`SymbolKind` here is `lsp_types::SymbolKind`; import it in the test module
if `use super::*` does not bring it.

- [ ] **Step 2: Run it**

Run: `cargo nextest run -p ns-analyzer a_runnable_root`
Expected: PASS (it pins behaviour that exists; a failure means the
contract above is not what the server answers -- stop and report it).

- [ ] **Step 3: Gates and commit**

Run the commit gates (Global Constraints). Then:

```bash
git add crates/ns-analyzer/src/document_symbols.rs
git commit -m "test: the document symbols a module's play button is read from"
```

---

### Task 2: `DocumentSymbolCache`, shared by the structure view and the run markers

**Files:**
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lsp/DocumentSymbolCache.kt`
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/structure/NewspeakStructureViewFactory.kt`
  (the model's `symbols()`, `documentStamp()`, `fetch()`; `LAST_ANSWER`
  and `usableAnswer` move to the cache)
- Create: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/lsp/DocumentSymbolCacheTest.kt`
- Test (unchanged, must stay green): `structure/NewspeakStructureViewRefreshTest.kt`, `structure/NewspeakStructureViewFactoryTest.kt`

**Interfaces:**
- Produces (package `io.github.bstrauss.newspeak.lsp`):
  - `@Service(Service.Level.PROJECT) class DocumentSymbolCache(project: Project)`
  - `companion fun getInstance(project: Project): DocumentSymbolCache`
  - `companion fun documentStamp(file: VirtualFile): Long`
  - `fun current(file: VirtualFile): List<DocumentSymbol>?`
  - `fun fetch(file: VirtualFile, fetchSymbols: (VirtualFile) -> List<DocumentSymbol>? = fetcher): List<DocumentSymbol>?`
  - `fun request(file: VirtualFile, onAnswer: () -> Unit)`
  - `@Volatile internal var fetcher: (VirtualFile) -> List<DocumentSymbol>?` (tests replace it)
  - top-level `fun usableAnswer(symbols: List<DocumentSymbol>?): List<DocumentSymbol>?` (moved from the structure package; update its import in the structure package and its tests)

- [ ] **Step 1: Write the failing test**

`editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/lsp/DocumentSymbolCacheTest.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

class DocumentSymbolCacheTest : BasePlatformTestCase() {
    private val range = Range(Position(0, 0), Position(0, 20))
    private fun moduleSymbols() = listOf(DocumentSymbol("Module", SymbolKind.Class, range, range, "public"))

    private val cache: DocumentSymbolCache get() = DocumentSymbolCache.getInstance(project)
    private lateinit var serverFetcher: (com.intellij.openapi.vfs.VirtualFile) -> List<DocumentSymbol>?

    override fun setUp() {
        super.setUp()
        serverFetcher = cache.fetcher
    }

    // The light project, and its services, outlive a test.
    override fun tearDown() {
        try {
            cache.fetcher = serverFetcher
        } finally {
            super.tearDown()
        }
    }

    private val file by lazy { myFixture.addFileToProject("Module.ns", "class Module = () ()\n").virtualFile }

    private fun edit() {
        val document = FileDocumentManager.getInstance().getDocument(file)!!
        WriteCommandAction.runWriteCommandAction(project) { document.insertString(0, " ") }
    }

    fun testAFetchedAnswerIsCurrentUntilTheDocumentChanges() {
        cache.fetcher = { moduleSymbols() }
        assertNull(cache.current(file))
        assertEquals(moduleSymbols(), cache.fetch(file))
        assertEquals(moduleSymbols(), cache.current(file))
        edit()
        assertNull("an answer for older text places nothing", cache.current(file))
    }

    fun testAnEmptyAnswerIsNotKept() {
        cache.fetcher = { emptyList() }
        assertNull(cache.fetch(file))
        assertNull(cache.current(file))
    }

    fun testARequestCallsBackOnceTheAnswerIsKept() {
        val fetches = AtomicInteger()
        cache.fetcher = { fetches.incrementAndGet(); moduleSymbols() }
        val answered = CountDownLatch(1)
        cache.request(file) { answered.countDown() }
        assertTrue(answered.await(5, TimeUnit.SECONDS))
        assertEquals(moduleSymbols(), cache.current(file))
        assertEquals(1, fetches.get())
    }

    fun testARequestAsksAgainWhileTheServerHasNoAnswer() {
        val fetches = AtomicInteger()
        // No client yet on the first question, an answer on the second.
        cache.fetcher = { if (fetches.incrementAndGet() == 1) null else moduleSymbols() }
        val answered = CountDownLatch(1)
        cache.request(file) { answered.countDown() }
        assertTrue(answered.await(10, TimeUnit.SECONDS))
        assertEquals(2, fetches.get())
    }

    fun testTwoRequestsForOneStampAskOnce() {
        val fetches = AtomicInteger()
        val release = CountDownLatch(1)
        cache.fetcher = { fetches.incrementAndGet(); release.await(5, TimeUnit.SECONDS); moduleSymbols() }
        val answered = CountDownLatch(1)
        cache.request(file) { answered.countDown() }
        cache.request(file) { answered.countDown() }
        release.countDown()
        assertTrue(answered.await(5, TimeUnit.SECONDS))
        assertEquals(1, fetches.get())
    }
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests 'io.github.bstrauss.newspeak.lsp.DocumentSymbolCacheTest'`
Expected: compilation failure, `DocumentSymbolCache` unresolved.

- [ ] **Step 3: Write the cache**

`editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/lsp/DocumentSymbolCache.kt`:

```kotlin
package io.github.bstrauss.newspeak.lsp

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
     * long; a later edit asks anew.
     */
    fun request(file: VirtualFile, onAnswer: () -> Unit) {
        val stamp = documentStamp(file)
        if (requested.put(file.url, stamp) == stamp) return
        ask(file, stamp, onAnswer, attempt = 1)
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
        fun documentStamp(file: VirtualFile): Long = ReadAction.compute<Long, RuntimeException> {
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
```

- [ ] **Step 4: Make the structure model read through the cache**

In `NewspeakStructureViewFactory.kt`:
- delete `LAST_ANSWER`, `usableAnswer`, the model's `documentStamp()` and
  `fetch()`, and the imports only they used (`Key`, `Pair`,
  `FileDocumentManager`, `LspClient`, `LspClientManager`,
  `DocumentSymbolParams`, `NewspeakLspIntegrationProvider`);
- import `io.github.bstrauss.newspeak.lsp.DocumentSymbolCache`;
- replace `symbols()` with:

```kotlin
    internal fun symbols(): List<DocumentSymbol> {
        val cache = DocumentSymbolCache.getInstance(psiFile.project)
        val stamp = DocumentSymbolCache.documentStamp(file)
        val known = answer
        if (known != null && known.documentStamp == stamp) return known.symbols
        // The Structure tool window builds a new model each time it shows the
        // file (selected anew in the Project view, the window reopened); a
        // model that starts empty would show an empty tree until the server
        // has answered, then the tree, a flicker on every rebuild. The cache
        // keeps the last answer on the file, so a new model of an unchanged
        // document shows it at once.
        cache.current(file)?.let { shared ->
            answer = Answer(shared, stamp)
            return shared
        }
        if (ApplicationManager.getApplication().isDispatchThread) {
            askAgainLater()
            return known?.symbols.orEmpty()
        }
        val fetched = cache.fetch(file, fetcher ?: cache.fetcher)
        if (fetched == null) {
            askAgainLater()
            return known?.symbols.orEmpty()
        }
        retries.set(0)
        answer = Answer(fetched, stamp)
        return fetched
    }
```

`psiFile` is the model's constructor parameter; make it `private val psiFile: PsiFile` if the class does not keep it yet (it passes it to
`TextEditorBasedStructureViewModel`, which exposes `getPsiFile()` -- use that
if a field would shadow it). Rewrite the class's doc comment sentence
"The answer is kept on the file with the document stamp it was given for"
to say it is kept in `DocumentSymbolCache`. Update any test import of
`usableAnswer` from `io.github.bstrauss.newspeak.structure` to
`io.github.bstrauss.newspeak.lsp`.

- [ ] **Step 5: Run the cache and structure tests**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests 'io.github.bstrauss.newspeak.lsp.DocumentSymbolCacheTest' --tests 'io.github.bstrauss.newspeak.structure.*'`
Expected: PASS, the structure tests unchanged.

- [ ] **Step 6: Gates and commit**

Run the commit gates. Then:

```bash
git add editors/jetbrains/src
git commit -m "refactor: the document symbols are kept in a project cache the run markers can share"
```

---

### Task 3: The run configuration takes a `.ns` module as its program

**Files:**
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunConfiguration.kt`
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunConfigurationEditor.kt`
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunAction.kt`
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunConfigurationProducer.kt`
- Modify: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakConfigurationType.kt` (description)
- Test: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/run/NewspeakRunConfigurationTest.kt`, `.../run/NewspeakRunLineMarkerContributorTest.kt`

**Interfaces:**
- Produces (package `io.github.bstrauss.newspeak.run`):
  - `const val PROJECT_FILE_NAME = "newspeak.json"` (in `NewspeakRunConfiguration.kt`)
  - `enum class ProgramKind { PROJECT, MODULE }`
  - `fun programKind(path: Path): ProgramKind?`
  - `NewspeakRunConfiguration.program: String?` (replaces `projectFile`)
  - `NewspeakRunConfiguration.namesProgram(file: VirtualFile): Boolean` (replaces `namesProjectFile`)
  - `fun programArgument(configuration: NewspeakRunConfiguration): Path?` (replaces `projectDirectory`): a project's directory, or the module file
  - `fun workingDirectory(configuration: NewspeakRunConfiguration): Path?`: the program file's directory
  - `NewspeakRunAction(programFile: VirtualFile, script: String?, command: NewspeakCommand)` (parameter renamed)
  - `fun gutterConfigurationName(programFile: VirtualFile, script: String?, command: NewspeakCommand): String`
  - `fun createConfigurationFromGutter(project: Project, programFile: VirtualFile, script: String?, command: NewspeakCommand): RunnerAndConfigurationSettings`

- [ ] **Step 1: Rename in the existing tests and add the new ones**

In `NewspeakRunConfigurationTest.kt` and `NewspeakRunLineMarkerContributorTest.kt`
rename every `projectFile` to `program`, `namesProjectFile` to
`namesProgram` and `projectDirectory(` to `programArgument(`; in
`testAnInvalidProjectPathIsRefusedLikeAMissingOne` expect the new message
`"Choose the program: a project's newspeak.json or a .ns module"`.
`testWithARuntimeTheProjectFileAndIterationsAreChecked` asserts the message
contains `newspeak.json`, which still holds. Then add to
`NewspeakRunConfigurationTest`:

```kotlin
    private val moduleFile: String = Path.of("/work/app/Hello.ns").toAbsolutePath().toString()

    fun testRunAModuleGivesItsFileAndRunsInItsDirectory() {
        val run = configuration().apply {
            program = "/work/app/Hello.ns"
            programArguments = "one"
        }
        assertEquals(listOf(runtime.toString(), "run", moduleFile, "--", "one"), newspeakCommandLine(runtime, run))
        assertEquals(projectDirectory, newspeakProcessCommandLine(runtime, run).workDirectory?.toString())
    }

    fun testBenchAModuleWithItsIterations() {
        val bench = configuration().apply {
            program = "/work/app/Hello.ns"
            command = NewspeakCommand.BENCH
            iterations = 3
        }
        assertEquals(listOf(runtime.toString(), "bench", "--iterations", "3", moduleFile), newspeakCommandLine(runtime, bench))
    }

    fun testAModuleWithAScriptIsRefused() {
        val directory = temporaryDirectory()
        NewspeakSettings.getInstance().state.runtimePath = Files.createFile(directory.resolve("ns-next")).toString()
        val module = Files.writeString(directory.resolve("Hello.ns"), "class Hello = () ()").toString()
        val scripted = configuration().apply {
            program = module
            script = "timed"
        }
        val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { scripted.checkConfiguration() }
        assertTrue(error.localizedMessage, error.localizedMessage.contains("script"))
        configuration().apply { program = module }.checkConfiguration()
    }

    fun testAProgramThatIsNeitherIsRefused() {
        val directory = temporaryDirectory()
        NewspeakSettings.getInstance().state.runtimePath = Files.createFile(directory.resolve("ns-next")).toString()
        val notes = Files.writeString(directory.resolve("notes.txt"), "").toString()
        for (program in listOf(notes, directory.toString())) {
            val other = configuration().apply { this.program = program }
            val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { other.checkConfiguration() }
            assertEquals("Choose the program: a project's newspeak.json or a .ns module", error.localizedMessage)
        }
        assertNull(programArgument(configuration().apply { program = "/work/app/notes.txt" }))
    }

    fun testTheEditorDropsTheScriptOfAModule() {
        val shown = configuration().apply {
            program = "/work/app/Hello.ns"
            script = "timed"
        }
        val editor = NewspeakRunConfigurationEditor()
        Disposer.register(testRootDisposable, editor)
        editor.resetFrom(shown)
        val applied = configuration()
        editor.applyTo(applied)
        assertEquals("/work/app/Hello.ns", applied.program)
        assertNull("ns-next refuses --script with a .ns file", applied.script)
    }
```

and to `NewspeakRunLineMarkerContributorTest`:

```kotlin
    fun testAModulesConfigurationIsNamedByItsFile() {
        val module = myFixture.addFileToProject("app/Hello.ns", "class Hello = () ()").virtualFile
        val run = createConfigurationFromGutter(project, module, null, NewspeakCommand.RUN)
        val bench = createConfigurationFromGutter(project, module, null, NewspeakCommand.BENCH)
        assertEquals("Hello.ns", run.name)
        assertEquals("Hello.ns (benchmark)", bench.name)
        assertEquals(module.path, (run.configuration as NewspeakRunConfiguration).program)
        assertSame(run, createConfigurationFromGutter(project, module, null, NewspeakCommand.RUN))
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests 'io.github.bstrauss.newspeak.run.*'`
Expected: compilation failure (`program`, `namesProgram`, `programArgument` unresolved).

- [ ] **Step 3: Implement in `NewspeakRunConfiguration.kt`**

- `NewspeakRunConfigurationOptions`: replace `projectFile` with
  ```kotlin
      /** The program: a project's `newspeak.json`, whose directory ns-next is given, or a `.ns` module. */
      var program by string()
  ```
  (The stored attribute changes name; configurations saved before lose
  their path -- the plugin is unpublished, decided with the user.)
- The configuration's `projectFile` property becomes `program` over
  `options.program`; `namesProjectFile` becomes `namesProgram`, its body
  unchanged but reading `program`, its comment saying "the program".
- Add, at file level:
  ```kotlin
  /** `crates/ns-cli/src/project.rs`'s `PROJECT_FILE_NAME`: the name ns-next recognises a project file by. */
  const val PROJECT_FILE_NAME = "newspeak.json"

  /** What a configuration runs: a project from its `newspeak.json`, or one module from its source. */
  enum class ProgramKind { PROJECT, MODULE }

  /**
   * By the file's name, as ns-next tells the two apart; null for anything
   * else, which the check refuses rather than hand ns-next a file it would
   * read as a source.
   */
  fun programKind(path: Path): ProgramKind? {
      val name = path.fileName?.toString() ?: return null
      return when {
          name == PROJECT_FILE_NAME -> ProgramKind.PROJECT
          name.endsWith(".ns") -> ProgramKind.MODULE
          else -> null
      }
  }
  ```
- Rename `projectFilePath` to `programFilePath` (body unchanged, reading
  `program`), `NO_PROJECT_FILE` to `NO_PROGRAM` with the text
  `"Choose the program: a project's newspeak.json or a .ns module"`, and add
  ```kotlin
  private const val SCRIPT_OF_A_MODULE =
      "A script belongs to a project's newspeak.json: ns-next refuses --script with a .ns module"
  ```
- Replace `projectDirectory` with:
  ```kotlin
  /**
   * What ns-next is given: a project's directory, or the module's file. Null
   * without a program, for one naming a root, for text that is no path and
   * for a file that is neither kind; `getState` refuses them all before
   * anything is started.
   */
  fun programArgument(configuration: NewspeakRunConfiguration): Path? {
      val path = programFilePath(configuration) ?: return null
      return when (programKind(path)) {
          ProgramKind.PROJECT -> path.parent
          ProgramKind.MODULE -> path
          null -> null
      }
  }

  /** Where ns-next runs: the program file's directory, for a project and a module alike. */
  fun workingDirectory(configuration: NewspeakRunConfiguration): Path? =
      programArgument(configuration)?.let { programFilePath(configuration)?.parent }
  ```
- `checkConfiguration`: after the runtime check,
  ```kotlin
          val programPath = programFilePath(this)
          val kind = programPath?.let(::programKind)
          if (programPath == null || kind == null || !Files.isRegularFile(programPath)) {
              throw RuntimeConfigurationError(NO_PROGRAM)
          }
          if (kind == ProgramKind.MODULE && script != null) {
              throw RuntimeConfigurationError(SCRIPT_OF_A_MODULE)
          }
  ```
  then the iterations check as it is.
- `getState`: `if (programArgument(this) == null) throw ExecutionException(NO_PROGRAM)`.
- `newspeakProcessCommandLine`: `.withWorkDirectory(workingDirectory(configuration)?.toFile())`.
- `newspeakCommandLine`: replace the project-directory line with
  ```kotlin
      // A project is given as its directory, a module as its file; the configuration names the file either way.
      add(checkNotNull(programArgument(configuration)) { NO_PROGRAM }.toString())
  ```

- [ ] **Step 4: The form**

In `NewspeakRunConfigurationEditor.kt`: `projectFileField` becomes
`programField`, the row `"Program:"`, the chooser
```kotlin
                FileChooserDescriptorFactory.singleFile()
                    .withTitle("Choose a Project's newspeak.json or a .ns Module")
                    .withFileFilter { file -> file.name == PROJECT_FILE_NAME || file.extension == "ns" },
```
The script row is disabled while the program names a module:
```kotlin
        row("Script:") {
            scriptField = textField()
                .align(AlignX.FILL)
                .comment("Empty for none; a project's only")
                .component
        }.enabledIf(programIsAProject)
```
with, as a member built before `form`:
```kotlin
    /** ns-next refuses `--script` with a `.ns` module, so the field is off for one. */
    private val programIsAProject = object : ComponentPredicate() {
        override fun invoke(): Boolean = !namesAModule(programField.text)

        override fun addListener(listener: (Boolean) -> Unit) {
            programField.textField.document.addDocumentListener(object : DocumentAdapter() {
                override fun textChanged(event: DocumentEvent) = listener(invoke())
            })
        }
    }
```
(`com.intellij.ui.layout.ComponentPredicate`, `com.intellij.ui.DocumentAdapter`,
`javax.swing.event.DocumentEvent`). Because `form` is built at construction
and reads `programIsAProject`, declare `programIsAProject` above `form`; its
`invoke` runs only after the program row has set `programField`. Add at file
level in `NewspeakRunConfiguration.kt`:
```kotlin
/** Whether text typed as a program names a `.ns` module; text that is no path names none. */
fun namesAModule(text: String): Boolean = try {
    programKind(Path.of(text.trim())) == ProgramKind.MODULE
} catch (_: InvalidPathException) {
    false
}
```
`resetEditorFrom` and `applyEditorTo` read and write `program`;
`applyEditorTo` stores
`configuration.script = if (namesAModule(programField.text)) null else scriptField.text.trim().ifEmpty { null }`.

- [ ] **Step 5: Action, gutter name, JSON producer, type description**

- `NewspeakRunAction`: rename the parameter `projectFile` to `programFile`;
  its doc comment: "runs or benchmarks a project, one of its scripts, or a
  module".
- `gutterConfigurationName`:
  ```kotlin
  /** What the gutter names a configuration: a project by its directory, a module by its file; the script; whether it benchmarks. */
  fun gutterConfigurationName(programFile: VirtualFile, script: String?, command: NewspeakCommand): String {
      val programName = if (programFile.name == PROJECT_FILE_NAME) programFile.parent?.name ?: programFile.name else programFile.name
      val scriptPart = script?.let { name -> " [$name]" }.orEmpty()
      val benchmarkPart = if (command == NewspeakCommand.BENCH) " (benchmark)" else ""
      return programName + scriptPart + benchmarkPart
  }
  ```
- `createConfigurationFromGutter`: parameter `programFile`; match with
  `configuration.namesProgram(programFile)`; set `this.program = programFile.path`;
  its doc comment says "the same program file, script and command".
- `NewspeakRunConfigurationProducer`: `configuration.program = target.file.path`,
  `configuration.namesProgram(target.file)`, and `PROJECT_FILE_NAME` for the
  `"newspeak.json"` literal.
- `NewspeakConfigurationType`'s description: `"Runs or benchmarks a Newspeak project or module with ns-next"`.

- [ ] **Step 6: Run the run tests**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests 'io.github.bstrauss.newspeak.run.*'`
Expected: PASS.

- [ ] **Step 7: Gates and commit**

Run the commit gates. Then:

```bash
git add editors/jetbrains/src
git commit -m "feat: a Newspeak run configuration runs a .ns module as well as a project"
```

---

### Task 4: The play button beside a runnable module's `main:args:`

**Files:**
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakModuleRun.kt` (`runnableMain`)
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakModuleRunLineMarkerContributor.kt`
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml` (register it)
- Create: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/run/NewspeakModuleRunLineMarkerContributorTest.kt`

**Interfaces:**
- Consumes: `DocumentSymbolCache` (Task 2), `NewspeakRunAction`,
  `NewspeakCommand` (Task 3), `NewspeakTokenTypes.KEYWORD`,
  `com.intellij.platform.lsp.util.getOffsetInDocument`.
- Produces: `fun runnableMain(symbols: List<DocumentSymbol>): DocumentSymbol?`
  (package `io.github.bstrauss.newspeak.run`), consumed by Task 5.

- [ ] **Step 1: Write the failing tests**

`NewspeakModuleRunLineMarkerContributorTest.kt`:

```kotlin
package io.github.bstrauss.newspeak.run

import com.intellij.codeInsight.daemon.LineMarkerInfo
import com.intellij.execution.RunManager
import com.intellij.icons.AllIcons
import com.intellij.openapi.actionSystem.ActionGroup
import com.intellij.openapi.actionSystem.ActionWithDelegate
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind

class NewspeakModuleRunLineMarkerContributorTest : BasePlatformTestCase() {
    private val cache: DocumentSymbolCache get() = DocumentSymbolCache.getInstance(project)
    private lateinit var serverFetcher: (VirtualFile) -> List<DocumentSymbol>?

    override fun setUp() {
        super.setUp()
        serverFetcher = cache.fetcher
    }

    override fun tearDown() {
        try {
            cache.fetcher = serverFetcher
            val runManager = RunManager.getInstance(project)
            runManager.getConfigurationSettingsList(NewspeakConfigurationType.getInstance())
                .forEach(runManager::removeConfiguration)
        } finally {
            super.tearDown()
        }
    }

    /** A symbol at the first occurrence of `marker` on `line` of `text`, its range and selection that one point. */
    private fun symbolAt(text: String, line: Int, marker: String, name: String, kind: SymbolKind, detail: String?): DocumentSymbol {
        // Kotlin strings are UTF-16, the unit the server's columns count in.
        val character = text.lines()[line].indexOf(marker)
        val position = Position(line, character)
        return DocumentSymbol(name, kind, Range(position, position), Range(position, position), detail)
    }

    private fun module(text: String, factory: String, mainDetail: String?): List<DocumentSymbol> {
        val root = symbolAt(text, 0, "Hello", "Hello", SymbolKind.Class, "public")
        val mainLine = text.lines().indexOfFirst { line -> line.contains("main:") }
        root.children = listOfNotNull(
            symbolAt(text, 0, "Hello", factory, SymbolKind.Constructor, "public"),
            mainDetail?.let { detail ->
                symbolAt(text, mainLine, "main:", "main: platform args: arguments", SymbolKind.Method, detail)
            },
        )
        return listOf(root)
    }

    private val runnable = """
        class Hello = (
        ) (
        public main: platform args: arguments = (
            ^ 0
        )
        )
    """.trimIndent()

    /** Per 0-based line, the Newspeak menu entries of the play buttons there, after the cache holds `symbols`. */
    private fun buttons(text: String, symbols: List<DocumentSymbol>): Map<Int, List<String>> {
        cache.fetcher = { symbols }
        myFixture.configureByText("Hello.ns", text)
        assertNotNull(cache.fetch(myFixture.file.virtualFile))
        val document = myFixture.editor.document
        return myFixture.findAllGutters()
            .filterIsInstance<LineMarkerInfo.LineMarkerGutterIconRenderer<*>>()
            .filter { renderer -> renderer.icon == AllIcons.RunConfigurations.TestState.Run }
            .associate { renderer ->
                val entries = menuActions(renderer.popupMenuActions)
                    .filterIsInstance<NewspeakRunAction>()
                    .map { action -> "${action.script}:${action.command}" }
                document.getLineNumber(renderer.lineMarkerInfo.startOffset) to entries
            }
    }

    private fun menuActions(action: AnAction?): List<AnAction> = when (action) {
        null -> emptyList()
        is ActionWithDelegate<*> -> listOfNotNull(action.delegate as? AnAction)
        is ActionGroup -> action.getChildren(null).flatMap(::menuActions)
        else -> listOf(action)
    }

    fun testARunnableModuleHasRunAndBenchmarkBesideMain() {
        assertEquals(mapOf(2 to listOf("null:RUN", "null:BENCH")), buttons(runnable, module(runnable, "new", "public")))
    }

    fun testAProtectedMainOrAnotherFactoryHasNone() {
        assertEquals(emptyMap<Int, List<String>>(), buttons(runnable, module(runnable, "new", "protected")))
        assertEquals(emptyMap<Int, List<String>>(), buttons(runnable, module(runnable, "usingPlatform: platform", "public")))
    }

    fun testWithoutACurrentAnswerThereIsNoButtonYet() {
        cache.fetcher = { null }
        myFixture.configureByText("Hello.ns", runnable)
        assertEquals(0, myFixture.findAllGutters().count { renderer -> renderer.icon == AllIcons.RunConfigurations.TestState.Run })
    }

    // A character outside the BMP takes two UTF-16 units; the button must still sit on `main:`.
    fun testColumnsPastAsciiStillFindMain() {
        val wide = runnable.replace("public main:", "(* \uD83D\uDE00 *) public main:")
        assertEquals(mapOf(2 to listOf("null:RUN", "null:BENCH")), buttons(wide, module(wide, "new", "public")))
    }

    fun testRunnableMainReadsTheRootsFactoryAndItsPublicInstanceSideMain() {
        assertNotNull(runnableMain(module(runnable, "new", "public")))
        assertNull(runnableMain(module(runnable, "new", "public class")))
        assertNull(runnableMain(module(runnable, "new", "private")))
        assertNull(runnableMain(module(runnable, "new", null)))
        assertNull(runnableMain(module(runnable, "packageUsing: manifest", "public")))
        assertNull(runnableMain(emptyList()))
        // The selector decides, whatever the parameters are called.
        val renamed = module(runnable, "new", "public")
        renamed.single().children[1].name = "main: platformObject args: words"
        assertNotNull(runnableMain(renamed))
        renamed.single().children[1].name = "main: platform"
        assertNull(runnableMain(renamed))
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests 'io.github.bstrauss.newspeak.run.NewspeakModuleRunLineMarkerContributorTest'`
Expected: compilation failure, `runnableMain` unresolved.

- [ ] **Step 3: `runnableMain`**

`NewspeakModuleRun.kt`:

```kotlin
package io.github.bstrauss.newspeak.run

import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.SymbolKind

/**
 * `main:args:` of a module ns-next can run from its source, or null: the
 * root class -- the answer's one top-level symbol -- is instantiated with
 * `new`, its factory written or omitted, and `main:args:` must be a public
 * instance-side method, since ns-next sends it to the instance as a public
 * send (`crates/ns-cli/src/program.rs`, `instantiate_application`). A
 * project's main module, built with `packageUsing:`, is run from its
 * `newspeak.json` instead.
 */
fun runnableMain(symbols: List<DocumentSymbol>): DocumentSymbol? {
    val members = symbols.firstOrNull()?.children.orEmpty()
    val builtWithNew = members.any { member -> member.kind == SymbolKind.Constructor && member.name == "new" }
    if (!builtWithNew) return null
    // A `detail` of `public` alone: `public class` is a class-side method, which ns-next never sends.
    return members.firstOrNull { member ->
        member.kind == SymbolKind.Method && selectorOf(member.name) == MAIN_SELECTOR && member.detail == "public"
    }
}

private const val MAIN_SELECTOR = "main:args:"

/**
 * A method's selector from its symbol's name, which the server writes as the
 * pattern (`main: platform args: arguments` → `main:args:`): its words ending
 * in `:`, joined. A unary or binary pattern has none, and answers "".
 */
private fun selectorOf(pattern: String): String =
    pattern.split(' ').filter { word -> word.endsWith(":") }.joinToString("")
```

- [ ] **Step 4: The contributor**

`NewspeakModuleRunLineMarkerContributor.kt`:

```kotlin
package io.github.bstrauss.newspeak.run

import com.intellij.codeInsight.daemon.DaemonCodeAnalyzer
import com.intellij.execution.lineMarker.RunLineMarkerContributor
import com.intellij.icons.AllIcons
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.util.getOffsetInDocument
import com.intellij.psi.PsiDocumentManager
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiManager
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache

/**
 * The play button beside `main:args:` of a module ns-next can run from its
 * source (`runnableMain`), opening Run and Benchmark. It reads the language
 * server's symbols and never waits for them: a daemon pass holds a read
 * action every write action, typing included, would wait behind. Without a
 * current answer it asks for one and restarts the file's daemon once it is
 * kept, so after an edit the button returns as soon as the server answers.
 */
class NewspeakModuleRunLineMarkerContributor : RunLineMarkerContributor(), DumbAware {
    override fun getInfo(element: PsiElement): Info? {
        // Only the keyword `main:` can start the pattern; every other leaf is turned away before the cache is read.
        if (element.firstChild != null || element.node.elementType != NewspeakTokenTypes.KEYWORD || element.text != "main:") {
            return null
        }
        val psiFile = element.containingFile ?: return null
        val file = psiFile.virtualFile ?: return null
        val project = element.project
        val cache = DocumentSymbolCache.getInstance(project)
        val symbols = cache.current(file)
        if (symbols == null) {
            cache.request(file) { restartDaemon(project, file) }
            return null
        }
        val main = runnableMain(symbols) ?: return null
        val document = PsiDocumentManager.getInstance(project).getDocument(psiFile) ?: return null
        if (getOffsetInDocument(document, main.selectionRange.start) != element.textRange.startOffset) return null
        val className = symbols.first().name
        return Info(
            AllIcons.RunConfigurations.TestState.Run,
            arrayOf(NewspeakRunAction(file, null, NewspeakCommand.RUN), NewspeakRunAction(file, null, NewspeakCommand.BENCH)),
        ) { _ -> "Run module '$className'" }
    }

    private fun restartDaemon(project: Project, file: VirtualFile) {
        ApplicationManager.getApplication().invokeLater({
            val psiFile = PsiManager.getInstance(project).findFile(file) ?: return@invokeLater
            DaemonCodeAnalyzer.getInstance(project).restart(psiFile)
        }, project.disposed)
    }
}
```

If the compiler or `verifyPlugin` reports `DaemonCodeAnalyzer.restart(PsiFile)`
deprecated on the targeted platform, use the overload it names as the
replacement (one taking a reason object), passing the string
`"Newspeak document symbols arrived"`.

- [ ] **Step 5: Register it**

In `plugin.xml`, after the `configurationType` line:

```xml
        <!-- Reads the language server's symbols, no JSON PSI, so it is here and not in newspeak-json.xml. -->
        <runLineMarkerContributor language="Newspeak" implementationClass="io.github.bstrauss.newspeak.run.NewspeakModuleRunLineMarkerContributor"/>
```

- [ ] **Step 6: Run the tests**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests 'io.github.bstrauss.newspeak.run.*'`
Expected: PASS.

- [ ] **Step 7: Gates and commit**

Run the commit gates. Then:

```bash
git add editors/jetbrains/src
git commit -m "feat: a play button beside main:args: of a module ns-next can run"
```

---

### Task 5: "Run" in the context menu of a runnable module

**Files:**
- Create: `editors/jetbrains/src/main/kotlin/io/github/bstrauss/newspeak/run/NewspeakModuleRunConfigurationProducer.kt`
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml`
- Test: `editors/jetbrains/src/test/kotlin/io/github/bstrauss/newspeak/run/NewspeakModuleRunLineMarkerContributorTest.kt` (append)

**Interfaces:**
- Consumes: `runnableMain` (Task 4), `DocumentSymbolCache` (Task 2),
  `gutterConfigurationName`, `namesProgram`, `program` (Task 3),
  `io.github.bstrauss.newspeak.NewspeakFile`.
- Produces: `class NewspeakModuleRunConfigurationProducer : LazyRunConfigurationProducer<NewspeakRunConfiguration>`.

- [ ] **Step 1: Write the failing tests**

Append to `NewspeakModuleRunLineMarkerContributorTest`:

```kotlin
    private val producer: NewspeakModuleRunConfigurationProducer
        get() = com.intellij.execution.actions.RunConfigurationProducer.getInstance(NewspeakModuleRunConfigurationProducer::class.java)

    private fun contextAt(element: com.intellij.psi.PsiElement) =
        com.intellij.execution.actions.ConfigurationContext.createEmptyContextForLocation(com.intellij.execution.PsiLocation(element))

    fun testTheContextMenuRunsARunnableModule() {
        cache.fetcher = { module(runnable, "new", "public") }
        myFixture.configureByText("Hello.ns", runnable)
        assertNotNull(cache.fetch(myFixture.file.virtualFile))
        val onBody = checkNotNull(myFixture.file.findElementAt(runnable.indexOf("^ 0")))
        for (context in listOf(contextAt(myFixture.file), contextAt(onBody))) {
            val configuration = checkNotNull(producer.createConfigurationFromContext(context)).configuration as NewspeakRunConfiguration
            assertEquals(myFixture.file.virtualFile.path, configuration.program)
            assertNull(configuration.script)
            assertEquals(NewspeakCommand.RUN, configuration.command)
            assertEquals("Hello.ns", configuration.name)
        }
        val gutter = createConfigurationFromGutter(project, myFixture.file.virtualFile, null, NewspeakCommand.RUN)
        assertTrue(producer.isConfigurationFromContext(gutter.configuration as NewspeakRunConfiguration, contextAt(myFixture.file)))
    }

    fun testTheContextMenuOfAModuleThatCannotRunOffersNothing() {
        cache.fetcher = { module(runnable, "new", "protected") }
        myFixture.configureByText("Hello.ns", runnable)
        assertNotNull(cache.fetch(myFixture.file.virtualFile))
        assertNull(producer.createConfigurationFromContext(contextAt(myFixture.file)))
    }

    // Off the EDT, with nothing kept yet, the producer asks the server itself: a Project-view
    // file never opened in an editor has no answer until someone asks.
    fun testOffTheEdtTheContextMenuAsksForTheSymbols() {
        cache.fetcher = { module(runnable, "new", "public") }
        val file = myFixture.addFileToProject("Hello.ns", runnable)
        val created = com.intellij.openapi.application.ApplicationManager.getApplication().executeOnPooledThread<Any?> {
            com.intellij.openapi.application.ReadAction.compute<Any?, RuntimeException> {
                producer.createConfigurationFromContext(contextAt(file))
            }
        }.get()
        assertNotNull(created)
    }
```

(Move the fully qualified names into imports when writing the file.)

- [ ] **Step 2: Run them to see them fail**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests 'io.github.bstrauss.newspeak.run.NewspeakModuleRunLineMarkerContributorTest'`
Expected: compilation failure, `NewspeakModuleRunConfigurationProducer` unresolved.

- [ ] **Step 3: The producer**

```kotlin
package io.github.bstrauss.newspeak.run

import com.intellij.execution.actions.ConfigurationContext
import com.intellij.execution.actions.LazyRunConfigurationProducer
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.util.Ref
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.PsiElement
import io.github.bstrauss.newspeak.NewspeakFile
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache

/**
 * "Run" in the context menu of a module ns-next can run from its source, in
 * the project view and anywhere in its editor, configured as the play button
 * beside `main:args:` configures it.
 */
class NewspeakModuleRunConfigurationProducer : LazyRunConfigurationProducer<NewspeakRunConfiguration>() {
    override fun getConfigurationFactory(): ConfigurationFactory = NewspeakConfigurationType.getInstance().factory

    override fun setupConfigurationFromContext(
        configuration: NewspeakRunConfiguration,
        context: ConfigurationContext,
        sourceElement: Ref<PsiElement>,
    ): Boolean {
        val file = runnableModule(context) ?: return false
        configuration.program = file.path
        configuration.script = null
        configuration.command = NewspeakCommand.RUN
        configuration.name = gutterConfigurationName(file, null, NewspeakCommand.RUN)
        return true
    }

    // Recognising a configuration needs no symbols: it is this module's if it names the file.
    override fun isConfigurationFromContext(configuration: NewspeakRunConfiguration, context: ConfigurationContext): Boolean {
        val file = moduleFile(context) ?: return false
        return configuration.namesProgram(file) && configuration.script == null && configuration.command == NewspeakCommand.RUN
    }

    private fun moduleFile(context: ConfigurationContext): VirtualFile? =
        (context.psiLocation?.containingFile as? NewspeakFile)?.virtualFile

    private fun runnableModule(context: ConfigurationContext): VirtualFile? {
        val file = moduleFile(context) ?: return null
        val cache = DocumentSymbolCache.getInstance(context.project)
        // Asked once per gesture, so waiting for the server off the EDT is affordable, as the
        // structure view's question is; on the EDT only a kept answer counts.
        val symbols = cache.current(file)
            ?: if (ApplicationManager.getApplication().isDispatchThread) null else cache.fetch(file)
        return file.takeIf { symbols != null && runnableMain(symbols) != null }
    }
}
```

- [ ] **Step 4: Register it**

In `plugin.xml`, after the line-marker registration of Task 4:

```xml
        <runConfigurationProducer implementation="io.github.bstrauss.newspeak.run.NewspeakModuleRunConfigurationProducer"/>
```

- [ ] **Step 5: Run the run tests**

Run: `./editors/jetbrains/gradlew -p editors/jetbrains test --tests 'io.github.bstrauss.newspeak.run.*'`
Expected: PASS, the `newspeak.json` producer's tests unchanged.

- [ ] **Step 6: Gates and commit**

Run the commit gates. Then:

```bash
git add editors/jetbrains/src
git commit -m "feat: Run in the context menu of a module ns-next can run"
```

---

### Task 6: Documentation

**Files:**
- Modify: `docs/design/editor-support.md` ("Structure", "The plugin" components list, "The project file and running projects")
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml` (description, change notes)
- Modify: `editors/jetbrains/README.md`, `docs/design/README.md`, `docs/PROGRESS.md` where they describe the play buttons (find them with `grep -n "play button" docs/design/README.md docs/PROGRESS.md editors/jetbrains/README.md`)
- Modify: `docs/OPEN-WORK.md` (remove the item "Running a module from its `main:args:`")

Documentation describes the current state; nothing is labelled historical.

- [ ] **Step 1: `editor-support.md`**

- "Structure", "When it asks" and "One answer per document stamp": the last
  usable answer is kept by `DocumentSymbolCache` (`lsp` package, a project
  service) on the `VirtualFile` with its stamp, shared with the run markers;
  the structure model still asks off the EDT and retries as described.
- "The project file and running projects":
  - the run configuration's field is the **program**: a `newspeak.json`
    (ns-next is given its directory) or a `.ns` module (ns-next is given the
    file); both run in the file's directory; the command line
    `ns-next <run|bench> [--script=<name>] [--iterations <n>] <project directory | module file> [-- <arguments>]`;
    the script field is disabled for a module and a module with a script is
    refused by the check, as the CLI refuses `--script` with a source file;
    a program of neither kind is refused ("Choose the program: …"); the
    stored option is `program`.
  - a new paragraph **"The module's play button."**: beside `main:args:` of a
    runnable module -- the root class built with `new`, written or omitted,
    and a public instance-side `main:args:`, since ns-next sends it as a
    public send (a protected one answers `doesNotUnderstand:`) -- read from
    the document symbols (`runnableMain`), not from a parser of the plugin's
    own; registered in `plugin.xml`, not `newspeak-json.xml`; the line
    marker never waits for the server, asks asynchronously and restarts the
    file's daemon on the answer, so after an edit the button returns once
    the server answers; a stale answer places nothing. A project's main
    module, built with `packageUsing:`, has no button: it runs from its
    `newspeak.json`. The configuration is named `<file>.ns`, with
    ` (benchmark)` for `bench`. "Run" in the context menu of a runnable
    module (`NewspeakModuleRunConfigurationProducer`) does the same; off the
    EDT it asks the server itself.
  - add to the "Decided with the user on 2026-10-05" paragraph: the module
    play button and the context menu's Run, the shared cache, the program
    field.
- "The plugin", component "The project file and run configurations": name
  the module's play button and producer.

- [ ] **Step 2: `plugin.xml` description and change notes**

Add to the description's list, after the `newspeak.json` entry:

```html
      <li>Running and benchmarking a single module: a play button beside <code>main:args:</code>
          of a module whose root class is created with <code>new</code>, and Run in its context menu.</li>
```

and to the change notes: `Running and benchmarking a single module from its <code>main:args:</code>.`

- [ ] **Step 3: The other documents**

Update each place the `grep` above finds so it states the current
behaviour, and remove the OPEN-WORK item (keep "Planned later"'s other
items untouched). If `PROGRESS.md` holds a milestone entry per merged
feature, add one for this one in its style.

- [ ] **Step 4: Commit**

`plugin.xml` is not Markdown, so run the commit gates. Then:

```bash
git add docs editors/jetbrains/README.md editors/jetbrains/src/main/resources/META-INF/plugin.xml
git commit -m "docs: a module runs from the play button beside its main:args:"
```

---

### Task 7: Check by hand under `runIde`, with the user

Not delegable: it needs the IDE on screen.

- [ ] **Step 1: Build and start**

Run: `cargo build -p ns-cli --release` and `./editors/jetbrains/gradlew -p editors/jetbrains runIde`.
In the sandbox IDE set the ns-next path to `target/release/ns-next.exe`.

- [ ] **Step 2: The user checks, with a module such as**

```
class Hello = (
) (
public main: platform args: arguments = (
    platform Transcript printString: 'hello'
)
)
```

- the button sits beside `main:` and its menu holds Run and Benchmark; Run
  prints and exits 0; Benchmark runs ten iterations;
- after typing a line above `main:args:`, the button returns on the moved
  line without a further edit;
- `main:args:` made protected (no modifier), or a factory
  `usingPlatform: platform`: no button;
- the Project view's context menu of the file, and the editor's, offer Run;
- the configuration's form with the `.ns` program disables Script;
- a project's `newspeak.json` still runs as before.

- [ ] **Step 3: Record**

Add a dated "Checked under `runIde` (2026-10-…)" sentence to
`editor-support.md`'s running section listing what was seen, as the
"Structure" section does; commit as `docs:` (Markdown only, no gates).

After this task the batch goes to the `finishing-a-batch` skill (end-of-batch
gates, the spec's content confirmed in `editor-support.md`, the spec file
deleted, the whole-branch review).
