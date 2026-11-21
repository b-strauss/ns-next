package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.LogicalPosition
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.TextEditor
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.TextRange
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.devtool.TracedRuns
import io.github.bstrauss.newspeak.devtool.WatchedMethods
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import io.github.bstrauss.newspeak.devtool.editor.DecorationsService
import io.github.bstrauss.newspeak.devtool.editor.DeveloperToolDecorationsSettings
import io.github.bstrauss.newspeak.devtool.editor.LINKED_STATEMENT_COLORS
import io.github.bstrauss.newspeak.devtool.editor.linkedColor
import io.github.bstrauss.newspeak.devtool.editor.sourceChecksum
import io.github.bstrauss.newspeak.devtool.model.TraceRecord
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.nio.file.Files
import java.nio.file.Path
import java.time.Instant

class CodeTabTest : BasePlatformTestCase() {
    private lateinit var source: Path
    private lateinit var tracedRuns: TracedRuns
    private lateinit var decorations: DecorationsService

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

    private fun tracedRun(): TraceRun = TraceRun("Demo", Instant.EPOCH).also { traced -> traced.add(tracedRecords()) }

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
                """{"body":1,"instructions":[""" +
                """{"index":0,"statement":0,"text":"LoadLiteral r3, 1"},{"index":1,"statement":0,"text":"SendPublic r4 <- r0 #+ (r3) @0"},""" +
                """{"index":2,"statement":0,"text":"StoreLocal r2 <- r4"},{"index":3,"statement":1,"text":"MakeClosure r5, [] -> copying r2"},""" +
                """{"index":4,"statement":1,"text":"SendPublic r6 <- r5 #value () @1"},{"index":5,"statement":1,"text":"Return r6"}]},""" +
                """{"body":2,"instructions":[{"index":0,"statement":0,"text":"SendPublic r1 <- r0 #* (r2) @0"},{"index":1,"statement":0,"text":"Return r1"}]}]}""",
            """{"kind":"code","body":1,"tier":"baseline","key":null,"entered":null,"architecture":"x64","bytes":40,"instructions":[""" +
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
        return lines.map { line -> checkNotNull(TraceRecord.parse(line)) }
    }

    private fun tab(run: TraceRun): CodeTab {
        tracedRuns.selected = run
        return CodeTab(project, run, testRootDisposable, tracedRuns, decorations)
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
        val run = TraceRun("Demo", Instant.EPOCH)
        val tab = tab(run)
        assertNull(tab.shownBody)
        run.add(tracedRecords())
        assertEquals(1, tab.shownBody)
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
               0   stmt 0  LoadLiteral r3, 1
               1   stmt 0  SendPublic r4 <- r0 #+ (r3) @0
               2   stmt 0  StoreLocal r2 <- r4
               3   stmt 1  MakeClosure r5, [] -> copying r2
               4   stmt 1  SendPublic r6 <- r5 #value () @1
               5   stmt 1  Return r6
            block {[] in Demo>>run:}
               0   stmt 0  SendPublic r1 <- r0 #* (r2) @0
               1   stmt 0  Return r1
            --- end

            """.trimIndent(),
            tab.viewer.document.text,
        )
        choose(tab, "baseline")
        assertEquals(
            """
            --- code {Demo>>run:} baseline x64 bytes=40
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
            ).map { line -> checkNotNull(TraceRecord.parse(line)) },
        )
        val tab = tab(run)
        tab.show(3)
        choose(tab, "twin")
        assertEquals(
            """
            --- code {Demo>>other} twin x64 bytes=8
              out of line
                +0x0000  jmp rbx
              @0  stmt 0  LoadLiteral r1, 7
                +0x0002  mov rax,7
            --- end

            """.trimIndent(),
            tab.viewer.document.text,
        )
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
        viewer.caretModel.moveToLogicalPosition(LogicalPosition(1, 0))
        val statement = editor.markupModel.allHighlighters.single { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS }
        assertEquals(FIRST, editor.document.getText(statement.textRange))
        assertEquals(linkedColor(0), statement.textAttributesKey)
        assertEquals(3, highlightedLines(viewer).size)
        // A block's line in its method's listing names the block's statement.
        viewer.caretModel.moveToLogicalPosition(LogicalPosition(8, 0))
        val blockStatement = editor.markupModel.allHighlighters.single { highlighter -> highlighter.textAttributesKey in LINKED_STATEMENT_COLORS }
        assertEquals(BLOCK_STATEMENT, editor.document.getText(blockStatement.textRange))
        // A line that stands for no instruction highlights nothing.
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

    fun testACopyInstalledAgainKeepsItsChoice() {
        val baselineAgain = """{"kind":"code","body":3,"tier":"baseline","key":null,"entered":null,"architecture":"x64","bytes":8,"instructions":[{"offset":0,"text":"ret","scope":0,"at":1,"annotation":null}],"data":[],"scopes":[{"scope":0,"body":3,"parent":null}],"mark":null}"""
        val firstBaseline = baselineAgain.replace("\"bytes\":8", "\"bytes\":4")
        val run = tracedRun().also { traced -> traced.add(listOf(checkNotNull(TraceRecord.parse(firstBaseline)), checkNotNull(TraceRecord.parse(baselineAgain)))) }
        val tab = tab(run)
        tab.show(3)
        assertEquals(listOf("bytecode", "baseline", "baseline (2)"), (0 until tab.chooser.itemCount).map { index -> tab.chooser.getItemAt(index).label })
        choose(tab, "baseline (2)")
        assertTrue(tab.viewer.document.text, "bytes=8" in tab.viewer.document.text)
        // A batch offers the copies again: the choice is the same install, not the first of its label.
        run.add(listOf(checkNotNull(TraceRecord.parse(baselineAgain.replace("\"bytes\":8", "\"bytes\":12")))))
        assertEquals("baseline (2)", (tab.chooser.selectedItem as CodeCopy).label)
        assertTrue(tab.viewer.document.text, "bytes=8" in tab.viewer.document.text)
    }

    private companion object {
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
