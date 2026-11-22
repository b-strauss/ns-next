package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.ide.ui.laf.darcula.ui.DarculaComboBoxUI
import com.intellij.ide.util.PropertiesComponent
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.LogicalPosition
import com.intellij.openapi.editor.event.DocumentEvent
import com.intellij.openapi.editor.event.DocumentListener
import com.intellij.openapi.editor.ex.EditorEx
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.fileEditor.TextEditor
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.TextRange
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.TestActionEvent
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.util.concurrency.AppExecutorUtil
import com.intellij.util.ui.JBUI
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.TracedRuns
import io.github.bstrauss.newspeak.devtool.WatchedMethods
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import io.github.bstrauss.newspeak.devtool.editor.DecorationsService
import io.github.bstrauss.newspeak.devtool.editor.DeveloperToolDecorationsSettings
import io.github.bstrauss.newspeak.devtool.editor.LINKED_STATEMENT_COLORS
import io.github.bstrauss.newspeak.devtool.editor.linkedColor
import io.github.bstrauss.newspeak.devtool.editor.listingOf
import io.github.bstrauss.newspeak.devtool.editor.bytecodeListing
import io.github.bstrauss.newspeak.devtool.editor.sourceChecksum
import io.github.bstrauss.newspeak.devtool.model.Bytecode
import io.github.bstrauss.newspeak.devtool.model.IndexedTrace
import io.github.bstrauss.newspeak.devtool.model.TraceRecord
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.nio.file.Files
import java.nio.file.Path
import java.awt.Container
import java.awt.Rectangle
import java.time.Instant
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import javax.swing.JComponent
import javax.swing.JList
import javax.swing.ListCellRenderer
import javax.swing.UIManager
import javax.swing.event.ChangeEvent
import javax.swing.event.ChangeListener

class CodeTabTest : BasePlatformTestCase() {
    private lateinit var source: Path
    private lateinit var tracedRuns: TracedRuns
    private lateinit var decorations: DecorationsService

    /** The file the records are written to, from which the tab reads its listings back. */
    private val trace = IndexedTrace()

    override fun setUp() {
        super.setUp()
        source = Files.createTempDirectory("newspeak-code-tab").resolve("Demo.ns")
        Files.writeString(source, DEMO)
        // Runs and decorations of the test's own, never the project's services, which outlive a test.
        tracedRuns = TracedRuns(project) { null }
        Disposer.register(testRootDisposable, tracedRuns)
        val watchedMethods = WatchedMethods(project)
        watchedMethods.fetcher = { _ -> null }
        val settings = DeveloperToolDecorationsSettings.Settings()
        decorations = DecorationsService(project, tracedRuns, watchedMethods, { null }, { settings })
        Disposer.register(testRootDisposable, decorations)
    }

    // The light project, and its properties, outlive a test.
    override fun tearDown() {
        try {
            PropertiesComponent.getInstance(project).unsetValue(SYNC_PROPERTY)
        } finally {
            super.tearDown()
        }
    }

    /** `[startLine, startColumn, endLine, endColumn]` of [part] in [DEMO], searched from the start of [within]. */
    private fun range(part: String, within: String = part): String {
        val start = DEMO.indexOf(part, DEMO.indexOf(within))
        check(start >= 0) { part }
        fun position(offset: Int): String {
            val line = DEMO.substring(0, offset).count { character -> character == '\n' }
            return "$line,${offset - (DEMO.lastIndexOf('\n', offset - 1) + 1)}"
        }
        return "[${position(start)},${position(start + part.length)}]"
    }

    private fun body(number: Int, name: String, within: String, family: Int?, statements: String): String {
        val file = source.toString().replace("\\", "\\\\")
        return """{"kind":"body","body":$number,"name":"$name","family":${family ?: "null"},"module":{"id":"Demo","version":null},"source":{"file":"$file"},"range":${range(within)},"checksum":"${sourceChecksum(within)}","statements":[$statements],"sends":[]}"""
    }

    private fun tracedRun(): TraceRun = trace.run().also { traced -> traced.add(tracedRecords()) }

    private fun tracedRecords(): List<TraceRecord> {
        val lines = listOf(
            """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64"}""",
            body(
                1, "Demo>>run:", RUN, null,
                """{"instructions":[0,3],"range":${range(FIRST)}},{"instructions":[3,6],"range":${range(SECOND)}}""",
            ),
            body(3, "Demo>>other", OTHER, null, """{"instructions":[0,2],"range":${range(OTHER_STATEMENT)}}"""),
            """{"kind":"bytecode","body":3,"bodies":[{"body":3,"instructions":[{"index":0,"statement":0,"text":"LoadLiteral r1, 7"},{"index":1,"statement":0,"text":"Return r1"}]}]}""",
            body(2, "[] in Demo>>run:", BLOCK, 1, """{"instructions":[0,2],"range":${range(BLOCK_STATEMENT, BLOCK)}}"""),
            """{"kind":"bytecode","body":1,"bodies":[""" +
                """{"body":1,"kind":"method","arguments":1,"registers":20,"instructions":[""" +
                """{"index":0,"statement":0,"text":"LoadLiteral r3, 1"},{"index":1,"statement":0,"text":"SendPublic r4 <- r0 #+ (r3) @0"},""" +
                """{"index":2,"statement":0,"text":"StoreLocal r2 <- r4"},{"index":3,"statement":1,"text":"MakeClosure r5, [] -> copying r2"},""" +
                """{"index":4,"statement":1,"text":"SendPublic r6 <- r5 #value () @1"},{"index":5,"statement":1,"text":"Return r6"}]},""" +
                """{"body":2,"kind":"block","in":1,"at":3,"arguments":0,"registers":16,"instructions":[{"index":0,"statement":0,"text":"SendPublic r1 <- r0 #* (r2) @0"},{"index":1,"statement":0,"text":"Return r1"}]}]}""",
            """{"kind":"code","body":1,"tier":"baseline","key":null,"entered":null,"architecture":"x64","base":12288,"bytes":40,"instructions":[""" +
                """{"offset":0,"text":"push rbp","scope":null,"at":null,"annotation":null},""" +
                """{"offset":1,"text":"mov rax,1","scope":0,"at":0,"annotation":null},""" +
                """{"offset":5,"text":"call rcx","scope":0,"at":1,"annotation":"cell site 0"},""" +
                """{"offset":7,"text":"call rdx","scope":0,"at":4,"annotation":"cell site 1"},""" +
                """{"offset":9,"text":"jmp rbx","scope":null,"at":null,"annotation":"exit send @4"}],""" +
                """"data":[{"offset":0,"word":"budget word"}],"scopes":[{"scope":0,"body":1,"parent":null}],"mark":null}""",
            """{"kind":"code","body":1,"tier":"optimized","key":"Demo","entered":null,"architecture":"x64","bytes":24,"instructions":[""" +
                """{"offset":0,"text":"add rax,2","scope":0,"at":1,"annotation":null},""" +
                """{"offset":4,"text":"imul rax,rcx","scope":1,"at":0,"annotation":null},""" +
                """{"offset":8,"text":"ret","scope":0,"at":5,"annotation":null}],""" +
                """"data":[],"scopes":[{"scope":0,"body":1,"parent":null},{"scope":1,"body":2,"parent":[0,4]}],"mark":null}""",
        )
        return trace.records(lines)
    }

    /** Whether the event being dispatched counts as the user's input, as a click or a key would. */
    private var userInput = false

    /** Whether an editor's loading ends only when the test says so, as the platform's asynchronous loading may. */
    private var loadingDeferred = false

    /** What waits for an editor's loading to end while [loadingDeferred]. */
    private val whenLoadedActions = ArrayList<Runnable>()

    private fun endLoading() {
        val actions = whenLoadedActions.toList()
        whenLoadedActions.clear()
        actions.forEach(Runnable::run)
    }

    /** Whether a tab takes the production's test of a source viewport's readiness, which a headless editor never passes. */
    private var productionReadiness = false

    private fun tab(run: TraceRun, parent: Disposable = testRootDisposable, reads: RecordReads = immediateReads()): CodeTab {
        tracedRuns.selected = run
        return CodeTab(
            project, run, parent, tracedRuns, decorations, { null }, { userInput },
            { _, action -> if (loadingDeferred) whenLoadedActions += action else action.run() },
            { _, action -> if (openedDeferred) whenOpenedActions += action else action.run() },
            reads = reads,
        ) { editor -> !productionReadiness && editor.scrollingModel.visibleArea.height > 0 }
    }

    /** Whether the platform's word that it opened a file, after restoring its state, comes only when the test says so. */
    private var openedDeferred = false

    /** What waits for that word while [openedDeferred]. */
    private val whenOpenedActions = ArrayList<Runnable>()

    private fun tellOpened() {
        val actions = whenOpenedActions.toList()
        whenOpenedActions.clear()
        actions.forEach(Runnable::run)
    }

    private fun openSource(): Editor {
        myFixture.configureFromExistingVirtualFile(checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(source)))
        return myFixture.editor
    }

    private fun Editor.caretAt(part: String, within: String = part) {
        caretModel.moveToOffset(DEMO.indexOf(part, DEMO.indexOf(within)) + 1)
    }

    /** The lines of [editor] the linked highlighting colours, each with its colour. */
    private fun highlightedLines(editor: Editor): List<Pair<String, Int>> =
        editor.markupModel.allHighlighters
            .filter { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS }
            .sortedBy { highlighter -> highlighter.startOffset }
            .map { highlighter ->
                val line = editor.document.getLineNumber(highlighter.startOffset)
                val text = editor.document.getText(TextRange(editor.document.getLineStartOffset(line), editor.document.getLineEndOffset(line)))
                text to LINKED_STATEMENT_COLORS.indexOf(highlighter.textAttributesKey)
            }

    private fun choose(tab: CodeTab, label: String) {
        tab.chooser.selectedIndex = (0 until tab.chooser.itemCount).first { index -> tab.chooser.getItemAt(index).label == label }
    }

    fun testTheTabFollowsTheCaret() {
        val tab = tab(tracedRun())
        val editor = openSource()
        editor.caretAt(FIRST)
        assertEquals(1, tab.shownBody)
        assertTrue(tab.viewer.document.text, tab.viewer.document.text.startsWith("--- bytecode {Demo>>run:}\n"))
        editor.caretAt(BLOCK_STATEMENT, BLOCK)
        assertEquals("the innermost body is the block's", 2, tab.shownBody)
        // A block's bytecode is its method's listing; it has no machine code of its own.
        assertEquals(listOf("bytecode"), (0 until tab.chooser.itemCount).map { index -> tab.chooser.getItemAt(index).label })
        editor.caretAt("class Demo")
        assertEquals("outside every body the tab stays", 2, tab.shownBody)
    }

    fun testACaretPlacedBeforeTheTabShowsItsBodyAtOnce() {
        val editor = openSource()
        editor.caretAt(FIRST)
        val tab = tab(tracedRun())
        assertEquals("no caret move is needed", 1, tab.shownBody)
        assertTrue(tab.viewer.document.text, tab.viewer.document.text.startsWith("--- bytecode {Demo>>run:}\n"))
    }

    fun testACaretPlacedBeforeTheRecordsShowsItsBodyWhenTheyArrive() {
        val editor = openSource()
        editor.caretAt(FIRST)
        val run = trace.run()
        val tab = tab(run)
        assertNull(tab.shownBody)
        run.add(tracedRecords())
        assertEquals(1, tab.shownBody)
    }

    fun testAChosenListingIsBuiltOnce() {
        val tab = tab(tracedRun())
        val before = tab.listingsShown
        tab.show(1)
        assertEquals("the body offered", before + 1, tab.listingsShown)
        choose(tab, "baseline")
        assertEquals("a copy chosen", before + 2, tab.listingsShown)
    }

    fun testTheChooserListsEveryCopy() {
        val tab = tab(tracedRun())
        tab.show(1)
        assertEquals(
            listOf("bytecode", "baseline", "optimized key=Demo"),
            (0 until tab.chooser.itemCount).map { index -> tab.chooser.getItemAt(index).label },
        )
        assertEquals(
            """
            --- bytecode {Demo>>run:}
            method Demo>>run: (args 1, registers 20)
               0   stmt 0  LoadLiteral r3, 1
               1   stmt 0  SendPublic r4 <- r0 #+ (r3) @0
               2   stmt 0  StoreLocal r2 <- r4
               3   stmt 1  MakeClosure r5, [] -> copying r2
                    block block (args 0, registers 16)
                       0   stmt 0  SendPublic r1 <- r0 #* (r2) @0
                       1   stmt 0  Return r1
               4   stmt 1  SendPublic r6 <- r5 #value () @1
               5   stmt 1  Return r6
            --- end

            """.trimIndent(),
            tab.viewer.document.text,
        )
        choose(tab, "baseline")
        assertEquals(
            """
            --- code {Demo>>run:} baseline x64 base=0x3000 bytes=40
                +0x0000  push rbp
              @0  stmt 0  LoadLiteral r3, 1
                +0x0001  mov rax,1
              @1  stmt 0  SendPublic r4 <- r0 #+ (r3) @0
                +0x0005  call rcx  ; cell site 0
              @4  stmt 1  SendPublic r6 <- r5 #value () @1
                +0x0007  call rdx  ; cell site 1
              out of line
                +0x0009  jmp rbx  ; exit send @4
            data:
                data+0x0000  budget word
            --- end

            """.trimIndent(),
            tab.viewer.document.text,
        )
        choose(tab, "optimized key=Demo")
        assertEquals(
            """
            --- code {Demo>>run:} optimized x64 key={Demo} bytes=24
            scopes:
              s0 {Demo>>run:}
                s1 {[] in Demo>>run:} at s0@4
              s0 {Demo>>run:} @1  stmt 0  SendPublic r4 <- r0 #+ (r3) @0
                +0x0000  add rax,2
              s1 {[] in Demo>>run:} @0  stmt 0  SendPublic r1 <- r0 #* (r2) @0
                +0x0004  imul rax,rcx
              s0 {Demo>>run:} @5  stmt 1  Return r6
                +0x0008  ret
            --- end

            """.trimIndent(),
            tab.viewer.document.text,
        )
    }

    /** Code that begins out of line, the record saying so, as a twin's transfer stub may. */
    fun testAFirstStretchOutOfLineIsNamed() {
        val run = tracedRun()
        run.add(
            listOf(
                """{"kind":"code","body":3,"tier":"twin","key":null,"entered":null,"architecture":"x64","base":4096,"bytes":8,"instructions":[""" +
                    """{"offset":0,"text":"jmp rbx","scope":null,"at":null,"out-of-line":true,"annotation":null},""" +
                    """{"offset":2,"text":"mov rax,7","scope":0,"at":0,"out-of-line":false,"annotation":null}],""" +
                    """"data":[],"scopes":[{"scope":0,"body":3,"parent":null}],"mark":null}""",
            ).let(trace::records),
        )
        val tab = tab(run)
        tab.show(3)
        choose(tab, "twin")
        assertEquals(
            """
            --- code {Demo>>other} twin x64 base=0x1000 bytes=8
              out of line
                +0x0000  jmp rbx
              @0  stmt 0  LoadLiteral r1, 7
                +0x0002  mov rax,7
            --- end

            """.trimIndent(),
            tab.viewer.document.text,
        )
    }

    /** A body the run listed no bytecode of -- a host's own factory -- is named by the lines its scope carries. */
    fun testAScopeWithoutBytecodeIsNamedByItsOwnLines() {
        val run = tracedRun()
        run.add(
            listOf(
                """{"kind":"code","body":3,"tier":"twin","key":null,"entered":null,"architecture":"x64","base":4096,"bytes":4,"instructions":[""" +
                    """{"offset":0,"text":"mov rax,7","scope":0,"at":1,"out-of-line":false,"annotation":null}],""" +
                    """"data":[],"scopes":[{"scope":0,"body":3,"parent":null,""" +
                    """"lines":["   0   stmt 0  AllocateInstance r13","   1   stmt 0  InitializeInstance r13"]}],"mark":null}""",
            ).let(trace::records),
        )
        val tab = tab(run)
        tab.show(3)
        choose(tab, "twin")
        assertTrue(tab.viewer.document.text, tab.viewer.document.text.contains("\n  @1  stmt 0  InitializeInstance r13\n"))
    }

    fun testAStatementHighlightsItsBytecodeAndItsMachineCode() {
        val tab = tab(tracedRun())
        val editor = openSource()
        // In the method's statement, outside the block it makes.
        editor.caretAt("value", SECOND)
        assertEquals(1, tab.shownBody)
        assertEquals(
            listOf(
                "   3   stmt 1  MakeClosure r5, [] -> copying r2" to 1,
                "   4   stmt 1  SendPublic r6 <- r5 #value () @1" to 1,
                "   5   stmt 1  Return r6" to 1,
            ),
            highlightedLines(tab.viewer),
        )
        // The source's statement takes the same colour.
        val statement = editor.markupModel.allHighlighters.single { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS }
        assertEquals(linkedColor(1), statement.textAttributesKey)
        assertEquals(SECOND, editor.document.getText(statement.textRange))
        choose(tab, "baseline")
        assertEquals(
            listOf("  @4  stmt 1  SendPublic r6 <- r5 #value () @1" to 1, "    +0x0007  call rdx  ; cell site 1" to 1),
            highlightedLines(tab.viewer),
        )
        // Within the block, the optimized copy's lines of the inlined block's scope.
        editor.caretAt(BLOCK_STATEMENT, BLOCK)
        tab.show(1)
        choose(tab, "optimized key=Demo")
        assertEquals(
            listOf("  s1 {[] in Demo>>run:} @0  stmt 0  SendPublic r1 <- r0 #* (r2) @0" to 0, "    +0x0004  imul rax,rcx" to 0),
            highlightedLines(tab.viewer),
        )
    }

    fun testACaretInTheListingHighlightsItsStatement() {
        val tab = tab(tracedRun())
        val editor = openSource()
        tab.show(1)
        val viewer = tab.viewer
        // Line 1 is the method's line; line 2 its first instruction.
        viewer.caretModel.moveToLogicalPosition(LogicalPosition(2, 0))
        val statement = editor.markupModel.allHighlighters.single { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS }
        assertEquals(FIRST, editor.document.getText(statement.textRange))
        assertEquals(linkedColor(0), statement.textAttributesKey)
        assertEquals(3, highlightedLines(viewer).size)
        // A block's line in its method's listing names the block's statement.
        viewer.caretModel.moveToLogicalPosition(LogicalPosition(8, 0))
        val blockStatement = editor.markupModel.allHighlighters.single { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS }
        assertEquals(BLOCK_STATEMENT, editor.document.getText(blockStatement.textRange))
        // A line that stands for no instruction -- the head, a body's line -- highlights nothing.
        viewer.caretModel.moveToLogicalPosition(LogicalPosition(6, 0))
        assertTrue(editor.markupModel.allHighlighters.none { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS })
        viewer.caretModel.moveToLogicalPosition(LogicalPosition(0, 0))
        assertTrue(editor.markupModel.allHighlighters.none { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS })
    }

    fun testOpenInEditorOpensAFixedReadOnlyTab() {
        val tab = tab(tracedRun())
        tab.show(1)
        choose(tab, "optimized key=Demo")
        val shown = checkNotNull(tab.listing)
        val file = tab.openInEditor(shown)
        assertEquals("Demo>>run: optimized key=Demo", file.name)
        val manager = FileEditorManager.getInstance(project)
        assertTrue(manager.isFileOpen(file))
        assertFalse(file.isWritable)
        val document = checkNotNull(FileDocumentManager.getInstance().getDocument(file))
        assertFalse(document.isWritable)
        assertEquals(shown.text, document.text)
        // The same copy opened again brings its tab forward.
        assertSame(file, tab.openInEditor(checkNotNull(tab.listing)))
        assertEquals(1, manager.openFiles.count { open -> open is CodeListingFile })
        // The tab stays while the tool window's view moves on.
        choose(tab, "bytecode")
        assertTrue(manager.isFileOpen(file))
        assertEquals(shown.text, document.text)
        // Its lines are linked as the tool window's are.
        val listingEditor = checkNotNull(manager.getSelectedEditor(file) as? TextEditor).editor
        val sourceEditor = openSource()
        sourceEditor.caretAt(FIRST)
        assertEquals(listOf("  s0 {Demo>>run:} @1  stmt 0  SendPublic r4 <- r0 #+ (r3) @0" to 0, "    +0x0000  add rax,2" to 0), highlightedLines(listingEditor))
        // A listing of text alone, the debugger's frame code, links nothing.
        val text = CodeListingFile("frame", "--- code {Demo>>run:} baseline x64 bytes=1\n--- end\n")
        assertEquals("frame", text.name)
        assertFalse(text.isWritable)
    }

    fun testAMethodEditedSinceTheRunLinksNothing() {
        val tab = tab(tracedRun())
        val editor = openSource()
        val document = editor.document
        // A line more inside run:, so that the method below moves by a line.
        WriteCommandAction.runWriteCommandAction(project) { document.insertString(document.text.indexOf("+ 1") + 1, "\n     ") }
        editor.caretModel.moveToOffset(document.text.indexOf("y::") + 1)
        assertTrue(editor.markupModel.allHighlighters.none { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS })
        assertEquals(emptyList<Pair<String, Int>>(), highlightedLines(tab.viewer))
        // The method below the edit is unchanged and still linked, at the place its text moved to.
        editor.caretModel.moveToOffset(document.text.indexOf(OTHER_STATEMENT) + 1)
        assertEquals(3, tab.shownBody)
        val statement = editor.markupModel.allHighlighters.single { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS }
        assertEquals(OTHER_STATEMENT, document.getText(statement.textRange))
        assertEquals(listOf("   0   stmt 0  LoadLiteral r1, 7" to 0, "   1   stmt 0  Return r1" to 0), highlightedLines(tab.viewer))
        // Leaving every body takes the highlights away.
        editor.caretModel.moveToOffset(0)
        assertTrue(editor.markupModel.allHighlighters.none { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS })
        assertEquals(emptyList<Pair<String, Int>>(), highlightedLines(tab.viewer))
    }

    fun testAnotherRunSelectedTakesTheHighlightsAway() {
        val tab = tab(tracedRun())
        val editor = openSource()
        editor.caretAt(FIRST)
        assertEquals(3, highlightedLines(tab.viewer).size)
        tracedRuns.selected = TraceRun("Other", Instant.EPOCH)
        assertEquals(emptyList<Pair<String, Int>>(), highlightedLines(tab.viewer))
        assertTrue(editor.markupModel.allHighlighters.none { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS })
    }

    fun testACaretInAnotherEditorKeepsTheLink() {
        val tab = tab(tracedRun())
        val editor = openSource()
        editor.caretAt(FIRST)
        assertEquals(3, highlightedLines(tab.viewer).size)
        // An editor of no Newspeak file and no listing, as the Run console is, moving its caret as output arrives.
        val console = EditorFactory.getInstance().createEditor(EditorFactory.getInstance().createDocument("output\nmore output\n"), project)
        try {
            console.caretModel.moveToOffset(console.document.textLength)
            assertEquals(3, highlightedLines(tab.viewer).size)
            assertEquals(1, editor.markupModel.allHighlighters.count { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS })
        } finally {
            EditorFactory.getInstance().releaseEditor(console)
        }
    }

    fun testAnEditInsideALinkedMethodTakesItsHighlightsAwayAtOnce() {
        val tab = tab(tracedRun())
        val editor = openSource()
        editor.caretAt(FIRST)
        assertEquals(3, highlightedLines(tab.viewer).size)
        val document = editor.document
        // Typed inside run:, after the caret, which stays where it is: the method is stale, and its links go without a caret move.
        WriteCommandAction.runWriteCommandAction(project) { document.insertString(document.text.indexOf("+ 1") + 1, " 0 +") }
        assertEquals(emptyList<Pair<String, Int>>(), highlightedLines(tab.viewer))
        assertTrue(editor.markupModel.allHighlighters.none { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS })
    }

    /** The listing is read on a pooled thread, the text shown before staying until it arrives. */
    fun testAListingIsReadOffTheEventDispatchThread() {
        val onTheEdt = AtomicReference<Boolean>()
        val watching = RecordReads(
            { task ->
                AppExecutorUtil.getAppExecutorService().execute {
                    onTheEdt.set(ApplicationManager.getApplication().isDispatchThread)
                    task.run()
                }
            },
            { task, modality, expired -> ApplicationManager.getApplication().invokeLater(task, modality, expired) },
            // Never slow, so that a loaded machine cannot put the notice between the two texts.
            { _, _, _ -> },
        )
        val tab = tab(tracedRun(), reads = watching)
        tab.show(1)
        val bytecode = tab.viewer.document.text
        choose(tab, "baseline")
        assertEquals("the text shown stays while the read lasts", bytecode, tab.viewer.document.text)
        assertNull(tab.listingMessage)
        PlatformTestUtil.waitWithEventsDispatching("the listing arrives", { tab.listing?.title == "Demo>>run: baseline" }, WAIT_SECONDS.toInt())
        assertEquals("read off the EDT", false, onTheEdt.get())
        assertNull(tab.listingMessage)
        assertTrue(tab.viewer.document.text, tab.viewer.document.text.startsWith("--- code {Demo>>run:} baseline x64 base=0x3000 bytes=40\n"))
    }

    /**
     * The listing's text is built off the EDT, with the read: what the
     * delivery on the EDT receives is finished text, named as the run named
     * its bodies when the copy was chosen. A body described anew between the
     * read and the delivery -- which a listing built at the delivery would
     * name -- leaves the delivered text as it was built.
     */
    fun testAListingsTextIsBuiltOffTheEventDispatchThread() {
        val deliveries = ArrayList<Runnable>()
        val builtOffTheEdt = AtomicReference<Boolean>()
        val reads = RecordReads(
            { task ->
                AppExecutorUtil.getAppExecutorService().submit {
                    builtOffTheEdt.set(!ApplicationManager.getApplication().isDispatchThread)
                    task.run()
                }.get(WAIT_SECONDS, TimeUnit.SECONDS)
            },
            { task, _, _ -> deliveries += task },
            { _, _, _ -> },
        )
        val run = tracedRun()
        val tab = tab(run, reads = reads)
        tab.show(1)
        choose(tab, "optimized key=Demo")
        assertEquals(true, builtOffTheEdt.get())
        // Renamed after the read, before the delivery.
        run.add(trace.records(body(2, "[] in Demo>>renamed", BLOCK, 1, """{"instructions":[0,2],"range":${range(BLOCK_STATEMENT, BLOCK)}}""")))
        deliveries.single().run()
        val text = checkNotNull(tab.listing).text
        assertTrue(text, "{[] in Demo>>run:}" in text)
        assertFalse(text, "renamed" in text)
    }

    /** A read that takes long says so in the viewer once it has taken long, the text before it shown until then. */
    fun testASlowReadSaysSoOnceItTakesLong() {
        val reads = ArrayList<Runnable>()
        val notices = ArrayList<Runnable>()
        val tab = tab(tracedRun(), reads = RecordReads({ task -> reads += task }, { task, _, _ -> task.run() }, { task, _, _ -> notices += task }))
        tab.show(1)
        val bytecode = tab.viewer.document.text
        choose(tab, "baseline")
        assertEquals(bytecode, tab.viewer.document.text)
        notices.single().run()
        assertEquals(NewspeakBundle.message("devtool.record.reading"), tab.viewer.document.text)
        assertEquals(NewspeakBundle.message("devtool.record.reading"), tab.listingMessage)
        assertNull(tab.listing)
        reads.single().run()
        assertEquals("Demo>>run: baseline", checkNotNull(tab.listing).title)
        // A read delivered before its notice is due shows no notice.
        choose(tab, "optimized key=Demo")
        reads.last().run()
        notices.last().run()
        assertEquals("Demo>>run: optimized key=Demo", checkNotNull(tab.listing).title)
        assertNull(tab.listingMessage)
    }

    /**
     * A batch that brings another install of the shown body, the chosen one
     * left as it is, changes nothing in the viewer -- its text, its caret,
     * its scroll -- and reads nothing again.
     */
    fun testABatchThatKeepsTheChosenInstallLeavesTheViewerAlone() {
        var readCount = 0
        val counting = RecordReads({ task -> readCount++; task.run() }, { task, _, _ -> task.run() }, { _, _, _ -> })
        val run = tracedRun()
        val tab = tab(run, reads = counting)
        tab.show(1)
        choose(tab, "optimized key=Demo")
        assertEquals(1, readCount)
        val text = tab.viewer.document.text
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(3, 2))
        val caret = tab.viewer.caretModel.offset
        val scroll = tab.viewer.scrollingModel.verticalScrollOffset
        val shown = tab.listingsShown
        var changes = 0
        tab.viewer.document.addDocumentListener(
            object : DocumentListener {
                override fun documentChanged(event: DocumentEvent) {
                    changes++
                }
            },
            testRootDisposable,
        )
        run.add(
            trace.records(
                """{"kind":"code","body":1,"tier":"baseline","key":null,"entered":null,"architecture":"x64","bytes":8,"instructions":[{"offset":0,"text":"ret","scope":0,"at":5,"annotation":null}],"data":[],"scopes":[{"scope":0,"body":1,"parent":null}],"mark":null}""",
            ),
        )
        assertEquals("the new install is offered", listOf("bytecode", "baseline", "optimized key=Demo", "baseline (2)"), (0 until tab.chooser.itemCount).map { index -> tab.chooser.getItemAt(index).label })
        assertEquals("optimized key=Demo", (tab.chooser.selectedItem as CodeCopy).label)
        assertEquals("nothing is read again", 1, readCount)
        assertEquals(0, changes)
        assertEquals(text, tab.viewer.document.text)
        assertEquals(caret, tab.viewer.caretModel.offset)
        assertEquals(scroll, tab.viewer.scrollingModel.verticalScrollOffset)
        assertEquals(shown, tab.listingsShown)
    }

    /** A read that arrives after another copy was chosen shows nothing: the last copy chosen is shown. */
    fun testAReadForACopyNoLongerChosenIsDropped() {
        val queued = ArrayList<Runnable>()
        val tab = tab(tracedRun(), reads = RecordReads({ task -> queued += task }, { task, _, _ -> task.run() }, { _, _, _ -> }))
        tab.show(1)
        choose(tab, "baseline")
        choose(tab, "optimized key=Demo")
        assertEquals(2, queued.size)
        queued.toList().asReversed().forEach(Runnable::run)
        assertTrue(tab.viewer.document.text, tab.viewer.document.text.startsWith("--- code {Demo>>run:} optimized x64 key={Demo}"))
        assertEquals("Demo>>run: optimized key=Demo", checkNotNull(tab.listing).title)
    }

    /**
     * While a chosen copy is read, the text shown before stays as text
     * alone: no line of it is linked, it is no listing a caret or another
     * view takes for the chosen copy, and "Open in Editor" is off until the
     * chosen copy has arrived.
     */
    fun testAPendingReadLinksNothingAndOpensNothing() {
        val reads = ArrayList<Runnable>()
        val tab = tab(tracedRun(), reads = RecordReads({ task -> reads += task }, { task, _, _ -> task.run() }, { _, _, _ -> }))
        val editor = openSource()
        editor.caretAt(FIRST)
        assertEquals(3, highlightedLines(tab.viewer).size)
        assertTrue(tab.openButton.isEnabled)
        val bytecode = tab.viewer.document.text
        choose(tab, "baseline")
        assertEquals(bytecode, tab.viewer.document.text)
        assertEquals(emptyList<Pair<String, Int>>(), highlightedLines(tab.viewer))
        assertNull(listingOf(tab.viewer.document))
        assertNull(tab.listing)
        assertFalse(tab.openButton.isEnabled)
        reads.single().run()
        assertEquals("Demo>>run: baseline", checkNotNull(tab.listing).title)
        assertTrue(tab.openButton.isEnabled)
        assertTrue(highlightedLines(tab.viewer).isNotEmpty())
    }

    /**
     * A listing whose read failed is read again when the user chooses it
     * again, never by a batch, which would read it on every batch.
     */
    fun testAFailedListingIsReadAgainWhenChosenAgain() {
        var readCount = 0
        val counting = RecordReads({ task -> readCount++; task.run() }, { task, _, _ -> task.run() }, { _, _, _ -> })
        val run = tracedRun()
        val tab = tab(run, reads = counting)
        tab.show(1)
        val bytes = Files.readAllBytes(trace.file)
        Files.delete(trace.file)
        choose(tab, "baseline")
        assertEquals(1, readCount)
        assertEquals(NewspeakBundle.message("devtool.record.unreadable", trace.file.toString(), NewspeakBundle.message("devtool.record.gone")), tab.listingMessage)
        Files.write(trace.file, bytes)
        // A batch with another install of the body offers the copies again, the failed one still chosen.
        run.add(
            trace.records(
                """{"kind":"code","body":1,"tier":"twin","key":null,"entered":null,"architecture":"x64","bytes":8,"instructions":[],"data":[],"scopes":[],"mark":null}""",
            ),
        )
        assertEquals("a batch reads nothing again", 1, readCount)
        assertNull(tab.listing)
        choose(tab, "baseline")
        assertEquals(2, readCount)
        assertEquals("Demo>>run: baseline", checkNotNull(tab.listing).title)
        assertNull(tab.listingMessage)
    }

    /** A read that fails otherwise than its file not giving the record is said so too, never left waiting. */
    fun testAReadThatThrowsIsShownAsAFailure() {
        val failure = IllegalStateException("broken")
        val throwing = RecordReads({ task -> task.run() }, { task, _, _ -> task.run() }, { _, _, _ -> })
        var delivered: Result<String>? = null
        throwing.read({ true }, { throw failure }) { outcome -> delivered = outcome }
        assertEquals(NewspeakBundle.message("devtool.record.failed", failure.toString()), checkNotNull(delivered).exceptionOrNull()?.message)
    }

    /**
     * A placed body may list a statement the runtime has no place for --
     * a full initializer's statement of a clause, its range `null` -- beside
     * placed ones: the caret on a placed statement links its own lines
     * alone, and a listing line of the unplaced one selects it and colours
     * no source, throwing nothing.
     */
    fun testAStatementWithoutARangeLinksNoSource() {
        val run = trace.run()
        run.add(
            trace.records(
                """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64"}""",
                body(3, "Demo>>other", OTHER, null, """{"instructions":[0,1],"range":null},{"instructions":[1,2],"range":${range(OTHER_STATEMENT)}}"""),
                """{"kind":"bytecode","body":3,"bodies":[{"body":3,"instructions":[{"index":0,"statement":0,"text":"InitializeSuperclass r1"},{"index":1,"statement":1,"text":"Return r1"}]}]}""",
            ),
        )
        val tab = tab(run)
        val editor = openSource()
        editor.caretAt(OTHER_STATEMENT)
        assertEquals(3 to 1, tab.highlighting.selected)
        assertEquals(listOf("Return r1"), highlightedLines(tab.viewer).map { (line, _) -> line.substringAfter("stmt 1  ") })
        val unplaced = tab.viewer.document.text.lines().indexOfFirst { line -> "InitializeSuperclass" in line }
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(unplaced, 0))
        assertEquals(3 to 0, tab.highlighting.selected)
        assertEquals(emptyList<Pair<String, Int>>(), highlightedLines(editor))
    }

    /** A listing whose file is gone says so in the viewer, and throws nothing. */
    fun testAListingWhoseFileIsGoneSaysSo() {
        val tab = tab(tracedRun())
        tab.show(1)
        Files.delete(trace.file)
        choose(tab, "baseline")
        assertNull(tab.listing)
        val message = checkNotNull(tab.listingMessage)
        assertEquals(NewspeakBundle.message("devtool.record.unreadable", trace.file.toString(), NewspeakBundle.message("devtool.record.gone")), message)
        assertEquals(message, tab.viewer.document.text)
        // The bytecode, which the run keeps, is still shown.
        choose(tab, "bytecode")
        assertTrue(tab.viewer.document.text.startsWith("--- bytecode {Demo>>run:}\n"))
        assertNull(tab.listingMessage)
    }

    fun testACopyInstalledAgainKeepsItsChoice() {
        val baselineAgain = """{"kind":"code","body":3,"tier":"baseline","key":null,"entered":null,"architecture":"x64","bytes":8,"instructions":[{"offset":0,"text":"ret","scope":0,"at":1,"annotation":null}],"data":[],"scopes":[{"scope":0,"body":3,"parent":null}],"mark":null}"""
        val firstBaseline = baselineAgain.replace("\"bytes\":8", "\"bytes\":4")
        val run = tracedRun().also { traced -> traced.add(trace.records(firstBaseline, baselineAgain)) }
        val tab = tab(run)
        tab.show(3)
        assertEquals(listOf("bytecode", "baseline", "baseline (2)"), (0 until tab.chooser.itemCount).map { index -> tab.chooser.getItemAt(index).label })
        choose(tab, "baseline (2)")
        assertTrue(tab.viewer.document.text, "bytes=8" in tab.viewer.document.text)
        // A batch offers the copies again: the choice is the same install, not the first of its label.
        run.add(trace.records(baselineAgain.replace("\"bytes\":8", "\"bytes\":12")))
        assertEquals("baseline (2)", (tab.chooser.selectedItem as CodeCopy).label)
        assertTrue(tab.viewer.document.text, "bytes=8" in tab.viewer.document.text)
    }

    fun testABlockIsNestedAtTheInstructionTheRecordNames() {
        val listing = """{"kind":"bytecode","body":1,"bodies":[""" +
            """{"body":1,"kind":"method","arguments":0,"registers":15,"instructions":[{"index":0,"statement":0,"text":"MakeClosure r1, [] ->"},{"index":1,"statement":0,"text":"Closure r2"},{"index":2,"statement":0,"text":"Return r2"}]},""" +
            """{"body":2,"kind":"block","in":1,"at":1,"arguments":0,"registers":14,"instructions":[{"index":0,"statement":0,"text":"Return r0"}]}]}"""
        val bytecode = TraceRecord.parse(listing) as Bytecode
        val run = TraceRun("Demo", Instant.EPOCH)
        // Nested where the record says, whatever the instruction's text.
        assertEquals(
            """
            --- bytecode {#1}
            method #1 (args 0, registers 15)
               0   stmt 0  MakeClosure r1, [] ->
               1   stmt 0  Closure r2
                    block block (args 0, registers 14)
                       0   stmt 0  Return r0
               2   stmt 0  Return r2
            --- end

            """.trimIndent(),
            bytecodeListing(run, bytecode).text,
        )
    }

    /** The IDE's combo box UI, its current value's rectangle and padding opened to the test. */
    private class ExposedComboBoxUI : DarculaComboBoxUI() {
        fun currentValue(): Rectangle = rectangleForCurrentValue()

        fun rightPadding(): Int = padding?.right ?: 0
    }

    /** A run whose method has a copy with the longest label a key and an entered application make. */
    private fun runWithALongLabel(): TraceRun {
        val long = """{"kind":"code","body":1,"tier":"optimized","key":"newspeak.core.Kernel/Integer","entered":"newspeak.core.Kernel/Number","architecture":"x64","bytes":4,"instructions":[],"data":[],"scopes":[{"scope":0,"body":1,"parent":null}],"mark":null}"""
        return tracedRun().also { traced -> traced.add(trace.records(long)) }
    }

    fun testTheChooserPaintsEveryLabelWhole() {
        val tab = tab(runWithALongLabel())
        val ui = ExposedComboBoxUI()
        // A theme gives the padding, which a headless test's defaults leave out. The UI's preferred width counts
        // the arrow as a square button as high as the value while the layout makes it the arrow area wide, and
        // measures the label with its renderer's border, which the paint takes away: with the New UI's
        // renderer and a narrow padding, the platform's own width leaves the longest label cut.
        val padding = UIManager.get(PADDING)
        UIManager.put(PADDING, JBUI.insets(1, 1))
        try {
            tab.chooser.setUI(ui)
            tab.chooser.renderer = newUiRenderer()
            checkLabelsPaintedWhole(tab, ui)
        } finally {
            UIManager.put(PADDING, padding)
        }
    }

    /** The renderer the New UI's combo box UI makes (`DarculaComboBoxUI.createRenderer`), internal to the platform. */
    private fun newUiRenderer(): ListCellRenderer<in CodeCopy> {
        val constructor = Class.forName("com.intellij.ide.ui.laf.darcula.ui.DarculaComboBoxRenderer").getDeclaredConstructor()
        constructor.isAccessible = true
        @Suppress("UNCHECKED_CAST")
        return constructor.newInstance() as ListCellRenderer<in CodeCopy>
    }

    private fun checkLabelsPaintedWhole(tab: CodeTab, ui: ExposedComboBoxUI) {
        // Measured first with the copies of a body whose labels are short.
        tab.show(3)
        tab.chooser.preferredSize
        tab.show(1)
        val list = JList<CodeCopy>()
        val labels = (0 until tab.chooser.itemCount).map { index -> tab.chooser.getItemAt(index) }
        assertTrue(labels.toString(), labels.any { copy -> copy.label == "optimized key=newspeak.core.Kernel/Integer entered=newspeak.core.Kernel/Number" })
        tab.chooser.size = tab.chooser.preferredSize
        tab.chooser.doLayout()
        // The closed value is painted in this rectangle, less its right padding, and cut where the label is wider.
        val available = ui.currentValue().width - ui.rightPadding()
        for (copy in labels) {
            // Measured as the UI paints it: in the combo's font, the renderer's border taken away.
            val label = tab.chooser.renderer.getListCellRendererComponent(list, copy, -1, false, false) as JComponent
            label.font = tab.chooser.font
            label.border = JBUI.Borders.empty()
            val width = label.preferredSize.width
            assertTrue("${copy.label} needs $width, the chooser paints $available", width <= available)
        }
    }

    fun testATabNarrowerThanTheBarBoundsTheChooser() {
        val tab = tab(runWithALongLabel())
        tab.show(1)
        val wide = tab.chooser.preferredSize.width
        tab.setSize(300, 400)
        val bounded = tab.chooser.preferredSize.width
        assertTrue("$bounded of $wide", bounded < 300 && bounded < wide)
        // A tab too narrow even for the title keeps a chooser that shows a short label.
        tab.setSize(10, 400)
        assertTrue(tab.chooser.preferredSize.width.toString(), tab.chooser.preferredSize.width > 40)
    }

    // ---- Scroll with the editor ----

    /**
     * Gives [editor] a viewport of [lines] lines, scrolled to its start, as
     * a laid-out window would, so that its visible area is what the tab
     * scrolls by.
     */
    private fun showLines(editor: Editor, lines: Int) {
        editor.scrollingModel.disableAnimation()
        val height = editor.lineHeight * lines
        editor.component.setBounds(0, 0, VIEW_WIDTH, height)
        layOut(editor.component)
        // What the component takes beside its lines -- borders, a scroll bar -- added once it is measured.
        editor.component.setBounds(0, 0, VIEW_WIDTH, height + editor.component.height - editor.scrollingModel.visibleArea.height)
        layOut(editor.component)
        editor.scrollingModel.scrollVertically(0)
        check(editor.scrollingModel.visibleArea.height == height) { "a visible area of ${editor.scrollingModel.visibleArea}" }
    }

    private fun layOut(component: Container) {
        component.doLayout()
        for (child in component.components) if (child is Container) layOut(child)
    }

    /** Whether the lines from [first] to [last] of [editor] are all in its visible area. */
    private fun shows(editor: Editor, first: Int, last: Int): Boolean {
        val visible = editor.scrollingModel.visibleArea
        val top = editor.logicalPositionToXY(LogicalPosition(first, 0)).y
        val bottom = editor.logicalPositionToXY(LogicalPosition(last, 0)).y + editor.lineHeight
        return top >= visible.y && bottom <= visible.y + visible.height
    }

    /** The lines of the viewer the linked highlighting colours. */
    private fun linkedViewerLines(tab: CodeTab): List<Int> =
        tab.viewer.markupModel.allHighlighters
            .filter { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS }
            .map { highlighter -> tab.viewer.document.getLineNumber(highlighter.startOffset) }
            .sorted()

    /** The viewer's first line of the block's statement in its method's bytecode. */
    private fun blockLine(tab: CodeTab): Int = tab.viewer.document.text.lines().indexOfFirst { line -> "#* (r2)" in line }

    fun testScrollWithTheEditorIsOffUntilTurnedOnAndRememberedPerProject() {
        val tab = tab(tracedRun())
        assertFalse(tab.syncWithEditor)
        assertFalse(tab.syncAction.isSelected(TestActionEvent.createTestEvent()))
        tab.syncAction.setSelected(TestActionEvent.createTestEvent(), true)
        assertTrue(tab.syncWithEditor)
        // Another run's tab of the project, as the next traced run opens, takes the choice.
        assertTrue(CodeTab(project, tracedRun(), testRootDisposable, tracedRuns, decorations).syncWithEditor)
        assertEquals("Scroll with the editor", tab.syncAction.templatePresentation.text)
    }

    fun testASourceCaretScrollsTheViewerToItsStatementsLines() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        val editor = openSource()
        editor.caretAt(FIRST)
        showLines(tab.viewer, 2)
        editor.caretAt("value", SECOND)
        val linked = linkedViewerLines(tab)
        assertTrue(linked.toString(), linked.size > 2)
        // More lines than the viewer shows: the first at the top.
        assertEquals(tab.viewer.logicalPositionToXY(LogicalPosition(linked.first(), 0)).y, tab.viewer.scrollingModel.verticalScrollOffset)
        // The statement's lines shown already stay where they are, the line above them too.
        showLines(tab.viewer, linked.last() - linked.first() + 2)
        val above = tab.viewer.logicalPositionToXY(LogicalPosition(linked.first() - 1, 0)).y
        tab.viewer.scrollingModel.scrollVertically(above)
        assertEquals(above, tab.viewer.scrollingModel.verticalScrollOffset)
        editor.caretModel.moveToOffset(DEMO.indexOf("value", DEMO.indexOf(SECOND)) + 3)
        assertEquals(linked, linkedViewerLines(tab))
        assertEquals(above, tab.viewer.scrollingModel.verticalScrollOffset)
        assertEquals("the viewer's caret stays", 0, tab.viewer.caretModel.offset)
    }

    fun testAViewerCaretScrollsTheSourceToItsStatementWithoutMovingItsCaret() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        val editor = openSource()
        editor.caretAt(FIRST)
        val caret = editor.caretModel.offset
        showLines(editor, 1)
        // The block's statement, in the method's bytecode: the source's line 3.
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        val statementLine = editor.document.getLineNumber(DEMO.indexOf(BLOCK_STATEMENT, DEMO.indexOf(BLOCK)))
        assertTrue(shows(editor, statementLine, statementLine))
        assertFalse("the first line scrolled out", shows(editor, 0, 0))
        assertEquals("the source's caret stays", caret, editor.caretModel.offset)
        // The scroll moves no caret, so the source's caret sets nothing off: the tab shows the method still, and the block's statement is linked.
        assertEquals(1, tab.shownBody)
        assertEquals(2 to 0, tab.highlighting.selected)
        assertEquals(blockLine(tab), tab.viewer.caretModel.logicalPosition.line)
    }

    /** The opened editor's statement line in the source, and whether [editor] shows it without its first line. */
    private fun showsTheBlockStatementOnly(editor: Editor): Boolean {
        val statementLine = editor.document.getLineNumber(DEMO.indexOf(BLOCK_STATEMENT, DEMO.indexOf(BLOCK)))
        return shows(editor, statementLine, statementLine) && !shows(editor, 0, 0)
    }

    fun testAViewerCaretOpensTheSourceThatNoEditorShowsInTheBackground() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        loadingDeferred = true
        val manager = FileEditorManager.getInstance(project)
        val file = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(source))
        tab.show(1)
        val shown = tab.listingsShown
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertTrue(manager.isFileOpen(file))
        val opened = checkNotNull(manager.getSelectedEditor(file) as? TextEditor).editor
        assertEquals("the opened editor's caret is not moved", 0, opened.caretModel.offset)
        // The opening is followed by nothing, its selection included: the listing, its body and the link are the viewer's.
        assertEquals(shown, tab.listingsShown)
        assertEquals(1, tab.shownBody)
        assertEquals(2 to 0, tab.highlighting.selected)
        // While the editor still loads, a caret the platform moves is the opening's and is not followed.
        opened.caretAt("value", SECOND)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals(2 to 0, tab.highlighting.selected)
        // Once loaded, the editor is scrolled to the statement, its caret where it is.
        showLines(opened, 1)
        val caret = opened.caretModel.offset
        endLoading()
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertTrue("scrolled to the block's statement", showsTheBlockStatementOnly(opened))
        assertEquals(caret, opened.caretModel.offset)
        // And followed as any editor: a caret the debugger moves at a step, no input of the user's.
        opened.caretAt(OTHER_STATEMENT)
        assertEquals(3, tab.shownBody)
    }

    fun testAUsersCaretInAnEditorStillLoadingIsFollowed() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        loadingDeferred = true
        val file = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(source))
        tab.show(1)
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        val opened = checkNotNull(FileEditorManager.getInstance(project).getSelectedEditor(file) as? TextEditor).editor
        userInput = true
        opened.caretAt(OTHER_STATEMENT)
        assertEquals(3, tab.shownBody)
    }

    fun testTurningTheToggleOffFollowsEveryCaretAgain() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        loadingDeferred = true
        val file = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(source))
        tab.show(1)
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        val opened = checkNotNull(FileEditorManager.getInstance(project).getSelectedEditor(file) as? TextEditor).editor
        opened.caretAt("value", SECOND)
        assertEquals(2 to 0, tab.highlighting.selected)
        // The editor is still loading, and the toggle goes off: today's behaviour, nothing filtered.
        tab.syncWithEditor = false
        opened.caretAt(OTHER_STATEMENT)
        assertEquals(3, tab.shownBody)
        // Its loading ending later scrolls nothing either.
        showLines(opened, 1)
        endLoading()
        assertEquals(0, opened.scrollingModel.verticalScrollOffset)
    }

    /** Opens Demo.ns through the viewer's caret on the block's statement, and answers its editor. */
    private fun openThroughTheViewer(tab: CodeTab): Editor {
        tab.syncWithEditor = true
        val file = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(source))
        tab.show(1)
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        return checkNotNull(FileEditorManager.getInstance(project).getSelectedEditor(file) as? TextEditor).editor
    }

    fun testTheOpeningWaitsForThePlatformToTellItOpenedTheFile() {
        openedDeferred = true
        val tab = tab(tracedRun())
        val opened = openThroughTheViewer(tab)
        showLines(opened, 1)
        // Loaded, but the platform has not told yet that it opened the file, after restoring its state.
        opened.caretAt("value", SECOND)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals("still the opening's", 2 to 0, tab.highlighting.selected)
        assertEquals("not scrolled yet", 0, opened.scrollingModel.verticalScrollOffset)
        tellOpened()
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertTrue("scrolled to the block's statement", showsTheBlockStatementOnly(opened))
        opened.caretAt(OTHER_STATEMENT)
        assertEquals(3, tab.shownBody)
    }

    fun testThePlatformsScrollToTheRestoredCaretComesFirst() {
        loadingDeferred = true
        val tab = tab(tracedRun())
        val opened = openThroughTheViewer(tab)
        val viewport = (opened as EditorEx).scrollPane.viewport
        // As the platform does for an editor it restores while loading: once the viewport shows, it scrolls to the restored caret.
        val lastLine = opened.document.lineCount - 1
        var restoredScroll = false
        viewport.addChangeListener(
            object : ChangeListener {
                override fun stateChanged(event: ChangeEvent) {
                    if (restoredScroll || opened.scrollingModel.visibleArea.height == 0) return
                    restoredScroll = true
                    opened.scrollingModel.scrollVertically(opened.logicalPositionToXY(LogicalPosition(lastLine, 0)).y)
                }
            },
        )
        endLoading()
        // The viewport is not shown yet: nothing is scrolled while it is not.
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        showLines(opened, 1)
        assertTrue(restoredScroll)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertTrue("the tab's scroll comes after the platform's", showsTheBlockStatementOnly(opened))
    }

    fun testAScrollWaitingForItsViewportEndsWithTheToggle() {
        loadingDeferred = true
        val tab = tab(tracedRun())
        val opened = openThroughTheViewer(tab)
        val viewport = (opened as EditorEx).scrollPane.viewport
        val listeners = viewport.changeListeners.size
        endLoading()
        assertEquals("the tab waits for the viewport", listeners + 1, viewport.changeListeners.size)
        tab.syncWithEditor = false
        assertEquals("the wait ends with the toggle", listeners, viewport.changeListeners.size)
        showLines(opened, 1)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertFalse(showsTheBlockStatementOnly(opened))
    }

    fun testAScrollWaitingForItsViewportEndsWithTheTab() {
        loadingDeferred = true
        val lifetime = Disposer.newDisposable(testRootDisposable, "the Code tab")
        val tab = tab(tracedRun(), lifetime)
        val opened = openThroughTheViewer(tab)
        val viewport = (opened as EditorEx).scrollPane.viewport
        val listeners = viewport.changeListeners.size
        endLoading()
        Disposer.dispose(lifetime)
        assertEquals("the wait ends with the tab", listeners, viewport.changeListeners.size)
        showLines(opened, 1)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertFalse(showsTheBlockStatementOnly(opened))
    }

    fun testAQueuedScrollExpiresWithTheToggle() {
        loadingDeferred = true
        val tab = tab(tracedRun())
        val opened = openThroughTheViewer(tab)
        showLines(opened, 1)
        // The viewport shows: the scroll is queued at the opening's end, and the toggle goes off before it runs.
        endLoading()
        tab.syncWithEditor = false
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertFalse(showsTheBlockStatementOnly(opened))
    }

    fun testAFileClosedBeforeItsOpeningIsOverIsFollowedWhenOpenedAgain() {
        loadingDeferred = true
        val tab = tab(tracedRun())
        val opened = openThroughTheViewer(tab)
        val file = checkNotNull(opened.virtualFile)
        val manager = FileEditorManager.getInstance(project)
        manager.closeFile(file)
        loadingDeferred = false
        val again = checkNotNull(manager.openTextEditor(OpenFileDescriptor(project, file), false))
        again.caretAt(OTHER_STATEMENT)
        assertEquals(3, tab.shownBody)
        // The loading the closed editor waited for, ending late, scrolls nothing and filters nothing.
        endLoading()
        again.caretAt(FIRST)
        assertEquals(1, tab.shownBody)
    }

    fun testEveryEditorOfTheFileIsTheOpeningsWhileItLasts() {
        loadingDeferred = true
        val tab = tab(tracedRun())
        val opened = openThroughTheViewer(tab)
        // An editor of the file that the opening's call did not answer, as one made later would be.
        val other = EditorFactory.getInstance().createEditor(opened.document, project)
        try {
            other.caretAt("value", SECOND)
            assertEquals(2 to 0, tab.highlighting.selected)
            endLoading()
            other.caretAt(OTHER_STATEMENT)
            assertEquals("followed once the opening is over", 3, tab.shownBody)
        } finally {
            EditorFactory.getInstance().releaseEditor(other)
        }
    }

    /** Opens a second Newspeak file in front of Demo.ns, which stays open in a background tab. */
    private fun openUnrelatedInFront(): VirtualFile {
        val otherPath = source.resolveSibling("Unrelated.ns")
        Files.writeString(otherPath, "class Unrelated = () ()\n")
        val other = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(otherPath))
        FileEditorManager.getInstance(project).openFile(other, true)
        return other
    }

    /**
     * The user's path as production takes it: every caret move made by a
     * click or a key. The selection the opening makes, which the fixture
     * delivers within the user's click, is the opening's still; the
     * source's caret is followed as ever.
     */
    fun testAUsersClickInTheViewerBringsABackgroundTabForwardAndTheSourceStillLeads() {
        userInput = true
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        val editor = openSource()
        editor.caretAt(FIRST)
        val file = checkNotNull(editor.virtualFile)
        openUnrelatedInFront()
        tab.show(1)
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        val manager = FileEditorManager.getInstance(project)
        assertTrue(file in manager.selectedFiles)
        assertEquals("the opening's selection is not followed", 2 to 0, tab.highlighting.selected)
        assertEquals(1, tab.shownBody)
        editor.caretAt(OTHER_STATEMENT)
        assertEquals("the user's caret in the source is followed", 3, tab.shownBody)
    }

    fun testAUsersClickInTheViewerOpensTheSourceWithoutJitter() {
        userInput = true
        val tab = tab(tracedRun())
        val opened = openThroughTheViewer(tab)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals(0, opened.caretModel.offset)
        assertEquals(2 to 0, tab.highlighting.selected)
        assertEquals(1, tab.shownBody)
    }

    fun testAFinishedOpeningScrollsToTheViewersStatementOfThen() {
        loadingDeferred = true
        val tab = tab(tracedRun())
        val opened = openThroughTheViewer(tab)
        showLines(opened, 1)
        // The viewer's caret moves on to the method's first statement while the file still loads.
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(2, 0))
        assertEquals(1 to 0, tab.highlighting.selected)
        endLoading()
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        val firstLine = opened.document.getLineNumber(DEMO.indexOf(FIRST))
        assertTrue("the source shows the viewer's statement", shows(opened, firstLine, firstLine))
        assertFalse(showsTheBlockStatementOnly(opened))
    }

    fun testAFileFoundAfterTheToggleWentOffIsNotOpened() {
        // A file made after the file system listed its directory, which it then does not hold until a refresh.
        val directory = Files.createTempDirectory("newspeak-code-tab-unseen")
        checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(directory)).children
        source = directory.resolve("Demo.ns")
        Files.writeString(source, DEMO)
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        tab.show(1)
        // The file system does not hold the file yet: it is looked up on a pooled thread.
        assertNull(LocalFileSystem.getInstance().findFileByNioFile(source))
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        tab.syncWithEditor = false
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(WAIT_SECONDS)
        while (LocalFileSystem.getInstance().findFileByNioFile(source) == null) {
            check(System.nanoTime() < deadline) { "the file was never found" }
            // The refresh applies what it found on the event dispatch thread.
            PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
            Thread.sleep(POLL_MILLISECONDS)
        }
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals(0, FileEditorManager.getInstance(project).openFiles.size)
    }

    fun testABroughtForwardFileThatIsSelectedAlreadyLeavesNoSelectionSwallowed() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        val editor = openSource()
        editor.caretAt(FIRST)
        val document = editor.document
        // The method other is deleted: its body is placed nowhere, so the selected file is "brought forward".
        WriteCommandAction.runWriteCommandAction(project) {
            val start = document.text.indexOf(OTHER)
            document.deleteString(start, start + OTHER.length)
        }
        tab.show(3)
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(2, 0))
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals(3, tab.shownBody)
        // The user goes to another file and back: coming back to Demo.ns is followed.
        openUnrelatedInFront()
        FileEditorManager.getInstance(project).openFile(checkNotNull(editor.virtualFile), true)
        assertEquals(1, tab.shownBody)
    }

    fun testAViewerNotShownIsScrolledOnceItShows() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        val editor = openSource()
        editor.caretAt(FIRST)
        tab.viewer.scrollingModel.disableAnimation()
        // The tool window is hidden: the viewer has no extent when the source's caret moves.
        editor.caretAt("value", SECOND)
        val linked = linkedViewerLines(tab)
        assertEquals(0, tab.viewer.scrollingModel.verticalScrollOffset)
        tab.viewer.component.setBounds(0, 0, VIEW_WIDTH, tab.viewer.lineHeight * 2)
        layOut(tab.viewer.component)
        val height = tab.viewer.lineHeight * 2
        tab.viewer.component.setBounds(0, 0, VIEW_WIDTH, height + tab.viewer.component.height - tab.viewer.scrollingModel.visibleArea.height)
        layOut(tab.viewer.component)
        assertEquals(tab.viewer.logicalPositionToXY(LogicalPosition(linked.first(), 0)).y, tab.viewer.scrollingModel.verticalScrollOffset)
    }

    /**
     * A viewer laid out once and hidden now -- another inner tab in front --
     * keeps its extent, is scrolled at once and shows the lines when shown
     * again at its size, which no viewport change announces. A headless
     * viewer is never showing, as a hidden one is.
     */
    fun testAHiddenViewerWithItsExtentIsScrolledAtOnce() {
        productionReadiness = true
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        val editor = openSource()
        editor.caretAt(FIRST)
        showLines(tab.viewer, 2)
        assertFalse(tab.viewer.component.isShowing)
        editor.caretAt("value", SECOND)
        val linked = linkedViewerLines(tab)
        assertEquals(tab.viewer.logicalPositionToXY(LogicalPosition(linked.first(), 0)).y, tab.viewer.scrollingModel.verticalScrollOffset)
    }

    fun testTurningTheToggleOnBringsTheViewerToTheSourceAtOnce() {
        val tab = tab(tracedRun())
        val editor = openSource()
        editor.caretAt("value", SECOND)
        showLines(tab.viewer, 2)
        val linked = linkedViewerLines(tab)
        assertEquals(0, tab.viewer.scrollingModel.verticalScrollOffset)
        tab.syncWithEditor = true
        assertEquals(tab.viewer.logicalPositionToXY(LogicalPosition(linked.first(), 0)).y, tab.viewer.scrollingModel.verticalScrollOffset)
    }

    fun testAViewerCaretBringsTheSourceInABackgroundTabForward() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        val editor = openSource()
        editor.caretAt(FIRST)
        val caret = editor.caretModel.offset
        showLines(editor, 1)
        val otherPath = source.resolveSibling("Unrelated.ns")
        Files.writeString(otherPath, "class Unrelated = () ()\n")
        val other = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(otherPath))
        val manager = FileEditorManager.getInstance(project)
        manager.openFile(other, true)
        val file = checkNotNull(editor.virtualFile)
        assertFalse(file in manager.selectedFiles)
        tab.show(1)
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertTrue("the tab is brought forward", file in manager.selectedFiles)
        assertEquals("its caret is not moved", caret, checkNotNull(manager.getSelectedEditor(file) as? TextEditor).editor.caretModel.offset)
        assertTrue("scrolled to the block's statement", showsTheBlockStatementOnly(editor))
        assertEquals(1, tab.shownBody)
        assertEquals(2 to 0, tab.highlighting.selected)
    }

    fun testAViewerLineOfNoStatementLeavesTheSourceAsItIs() {
        val tab = tab(tracedRun())
        tab.syncWithEditor = true
        val editor = openSource()
        editor.caretAt(FIRST)
        showLines(editor, 1)
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        assertTrue(editor.scrollingModel.verticalScrollOffset > 0)
        editor.scrollingModel.scrollVertically(0)
        // The listing's head names no statement: the source stays where it is.
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(0, 0))
        assertNull(tab.highlighting.selected)
        assertEquals(0, editor.scrollingModel.verticalScrollOffset)
    }

    fun testABodyPlacedInNoFileOpensNothing() {
        val run = tracedRun()
        // A body whose ranges count in the runtime's text, not the file's: the reader keeps no range of it.
        val current = body(4, "Demo>>elsewhere", OTHER, null, """{"instructions":[0,2],"range":${range(OTHER_STATEMENT)}}""")
            .replace("\"kind\":\"body\",", "\"kind\":\"body\",\"placed\":\"current\",")
        run.add(
            listOf(
                checkNotNull(TraceRecord.parse(current)),
                checkNotNull(
                    TraceRecord.parse(
                        """{"kind":"bytecode","body":4,"bodies":[{"body":4,"instructions":[{"index":0,"statement":0,"text":"LoadLiteral r1, 9"},{"index":1,"statement":0,"text":"Return r1"}]}]}""",
                    ),
                ),
            ),
        )
        val tab = tab(run)
        tab.syncWithEditor = true
        tab.show(4)
        val line = tab.viewer.document.text.lines().indexOfFirst { text -> "LoadLiteral r1, 9" in text }
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(line, 0))
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals(0, FileEditorManager.getInstance(project).openFiles.size)
    }

    fun testWhileOffNothingScrolls() {
        val tab = tab(tracedRun())
        val editor = openSource()
        editor.caretAt(FIRST)
        showLines(tab.viewer, 2)
        showLines(editor, 1)
        editor.caretAt("value", SECOND)
        assertEquals(0, tab.viewer.scrollingModel.verticalScrollOffset)
        assertTrue("the link stands as before", linkedViewerLines(tab).isNotEmpty())
        editor.caretModel.moveToOffset(0)
        tab.show(1)
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab), 0))
        assertEquals(0, editor.scrollingModel.verticalScrollOffset)
        // Nothing is opened either.
        FileEditorManager.getInstance(project).closeFile(checkNotNull(editor.virtualFile))
        tab.viewer.caretModel.moveToLogicalPosition(LogicalPosition(blockLine(tab) - 1, 0))
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals(0, FileEditorManager.getInstance(project).openFiles.size)
    }

    private companion object {
        const val SYNC_PROPERTY = "newspeak.devtool.code.scrollWithEditor"
        const val WAIT_SECONDS = 30L
        const val POLL_MILLISECONDS = 20L
        const val VIEW_WIDTH = 800

        const val PADDING = "ComboBox.padding"
        const val FIRST = "y:: x + 1"
        const val SECOND = "^[y * 2] value"
        const val BLOCK = "[y * 2]"
        const val BLOCK_STATEMENT = "y * 2"
        const val RUN = "run: x = (\n    | y |\n    y:: x + 1.\n    ^[y * 2] value\n  )"
        const val OTHER_STATEMENT = "^3 + 4"
        const val OTHER = "other = (\n    $OTHER_STATEMENT\n  )"
        val DEMO = "class Demo = () (\n  $RUN\n  $OTHER\n)\n"
    }
}
