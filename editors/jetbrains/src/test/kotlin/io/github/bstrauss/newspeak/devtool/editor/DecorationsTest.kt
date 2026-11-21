package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.openapi.actionSystem.DefaultActionGroup
import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.TextRange
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.devtool.MethodName
import io.github.bstrauss.newspeak.devtool.TraceFilters
import io.github.bstrauss.newspeak.devtool.TracedRuns
import io.github.bstrauss.newspeak.devtool.WatchedMethods
import io.github.bstrauss.newspeak.devtool.model.MethodState
import io.github.bstrauss.newspeak.devtool.model.TraceRecord
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.nio.file.Files
import java.nio.file.Path
import java.time.Instant

class DecorationsTest : BasePlatformTestCase() {
    private lateinit var tracedRuns: TracedRuns
    private lateinit var watchedMethods: WatchedMethods
    private lateinit var settings: DeveloperToolDecorationsSettings.Settings
    private lateinit var directory: Path
    private var libraryDirectory: Path? = null

    override fun setUp() {
        super.setUp()
        directory = Files.createTempDirectory("newspeak-decorations")
        // Runs and watches of the test's own, never the project's services, which outlive a test.
        tracedRuns = TracedRuns(project) { null }
        Disposer.register(testRootDisposable, tracedRuns)
        watchedMethods = WatchedMethods(project)
        watchedMethods.fetcher = { _ -> null }
        settings = DeveloperToolDecorationsSettings.Settings()
    }

    /** The service under test, over the test's runs, watches, library directory and settings. */
    private fun decorations(): DecorationsService =
        DecorationsService(project, tracedRuns, watchedMethods, { libraryDirectory }, { settings }).also { service ->
            Disposer.register(testRootDisposable, service)
        }

    private fun open(path: Path, text: String): Editor {
        Files.createDirectories(path.parent)
        Files.writeString(path, text)
        val file = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(path))
        myFixture.configureFromExistingVirtualFile(file)
        return myFixture.editor
    }

    private fun openDemo(): Editor = open(directory.resolve("Demo.ns"), DEMO)

    /** `[startLine, startColumn, endLine, endColumn]` of [part] in [text], searched from the start of [within]. */
    private fun range(text: String, part: String, within: String = part): String {
        val base = text.indexOf(within)
        check(base >= 0) { within }
        val start = text.indexOf(part, base)
        check(start >= 0) { part }
        fun position(offset: Int): String {
            val line = text.substring(0, offset).count { character -> character == '\n' }
            val column = offset - (text.lastIndexOf('\n', offset - 1) + 1)
            return "$line,$column"
        }
        return "[${position(start)},${position(start + part.length)}]"
    }

    private fun sendJson(text: String, within: String, at: Int, site: Int?, call: String, selector: String): String =
        """{"at":$at,"site":${site ?: "null"},"call":${range(text, call, within)},"selector":[${range(text, selector, call)}]}"""

    private fun bodyJson(
        number: Int,
        name: String,
        source: String,
        text: String,
        within: String,
        family: Int? = null,
        checksum: String = sourceChecksum(within),
        sends: List<String> = emptyList(),
    ): String =
        """{"kind":"body","body":$number,"name":"$name","family":${family ?: "null"},"module":{"id":"Demo","version":null},"source":$source,"range":${range(text, within)},"checksum":"$checksum","statements":[],"sends":[${sends.joinToString(",")}]}"""

    private fun fileSource(path: Path): String = """{"file":"${path.toString().replace("\\", "\\\\")}"}"""

    private fun event(body: Int?, category: String, event: String, fields: String = ""): String =
        """{"kind":"event","category":"$category","body":${body ?: "null"},"event":"$event"$fields}"""

    private fun records(lines: List<String>): List<TraceRecord> = lines.map { line -> checkNotNull(TraceRecord.parse(line)) }

    /** The run of [DEMO]: `square:` optimized, `main:args:` optimized with its two blocks' sends decided and a second copy retired, `smile` interpreted. */
    private fun demoRun(path: Path = directory.resolve("Demo.ns"), name: String = "Demo"): TraceRun {
        val source = fileSource(path)
        val lines = listOf(
            HEADER,
            bodyJson(1, "Demo>>square:", source, DEMO, SQUARE, sends = listOf(sendJson(DEMO, SQUARE, 2, 0, "x * x", "*"))),
            bodyJson(
                2, "Demo>>main:args:", source, DEMO, MAIN,
                sends = listOf(
                    sendJson(DEMO, MAIN, 3, 0, "args do: [:each | each printString]", "do:"),
                    sendJson(DEMO, MAIN, 7, 1, "args collect: [:each | self square: each]", "collect:"),
                ),
            ),
            bodyJson(3, "[] in Demo>>main:args:", source, DEMO, FIRST_BLOCK, family = 2, sends = listOf(sendJson(DEMO, FIRST_BLOCK, 1, 2, "each printString", "printString"))),
            bodyJson(4, "[] in Demo>>main:args:", source, DEMO, SECOND_BLOCK, family = 2, sends = listOf(sendJson(DEMO, SECOND_BLOCK, 2, 3, "self square: each", "square:"))),
            bodyJson(5, "Demo>>smile", source, DEMO, SMILE, checksum = SMILE_CHECKSUM, sends = listOf(sendJson(DEMO, SMILE, 1, 0, "'😀' size", "size"))),
            event(1, "compile", "compiled"),
            event(1, "optimize", "optimized", ""","key":null,"entered":null,"bytes":100"""),
            event(2, "compile", "compiled"),
            event(2, "inlining", "declined", ""","in-body":3,"at":1,"callee":null,"reason":"over-weight","key":null,"entered":null"""),
            event(2, "inlining", "inlined", ""","in-body":4,"at":2,"callee":1,"key":null,"entered":null"""),
            event(2, "optimize", "optimized", ""","key":null,"entered":null,"bytes":200"""),
            event(2, "optimize", "optimized", ""","key":"Demo/Other","entered":null,"bytes":150"""),
            event(2, "deoptimize", "retired", ""","key":"Demo/Other","entered":null,"reason":"exits","version":"none""""),
            event(2, "caches", "transition", ""","before":"vacant","after":"monomorphic","selector":"square:","site":3,"receiver":"Demo""""),
            event(2, "caches", "transition", ""","before":"monomorphic","after":"polymorphic","selector":"square:","site":3,"receiver":"Demo/Other""""),
            event(2, "caches", "transition", ""","before":"vacant","after":"monomorphic","selector":"printString","site":2,"receiver":"Integer""""),
        )
        return TraceRun(name, Instant.EPOCH).also { run -> run.add(records(lines)) }
    }

    private fun icons(editor: Editor): List<Pair<Int, StateGutterRenderer>> = editor.markupModel.allHighlighters
        .mapNotNull { highlighter -> (highlighter.gutterIconRenderer as? StateGutterRenderer)?.let { renderer -> editor.document.getLineNumber(highlighter.startOffset) to renderer } }
        .sortedBy { (line, _) -> line }

    private fun hints(editor: Editor): List<Pair<Int, String>> = editor.inlayModel
        .getInlineElementsInRange(0, editor.document.textLength, SendInlayRenderer::class.java)
        .map { inlay -> inlay.offset to inlay.renderer.text }
        .sortedBy { (offset, _) -> offset }

    private fun lineOf(part: String): Int = DEMO.substring(0, DEMO.indexOf(part)).count { character -> character == '\n' }

    private fun endOf(part: String, within: String): Int = DEMO.indexOf(part, DEMO.indexOf(within)) + part.length

    fun testAnOptimizedMethodShowsItsIcon() {
        val editor = openDemo()
        decorations()
        tracedRuns.selected = demoRun()
        val icons = icons(editor)
        assertEquals(listOf(lineOf(SQUARE), lineOf(MAIN), lineOf(SMILE)), icons.map { (line, _) -> line })
        val square = icons.first().second
        assertEquals(MethodState.OPTIMIZED, square.state)
        assertFalse(square.stale)
        assertEquals("optimized · 1 copies · 0 deoptimizations", square.tooltipText)
        val main = icons[1].second
        assertEquals(MethodState.OPTIMIZED, main.state)
        assertTrue(main.tooltipText, main.tooltipText.startsWith("optimized · 1 copies · 1 deoptimizations"))
        assertTrue(main.tooltipText, "exits" in main.tooltipText)
    }

    fun testEachBlockIsDecoratedAtItsOwnRange() {
        val editor = openDemo()
        decorations()
        tracedRuns.selected = demoRun()
        val hints = hints(editor)
        assertTrue(hints.toString(), endOf("printString", FIRST_BLOCK) to "declined: over-weight · mono" in hints)
        assertTrue(hints.toString(), endOf("square:", SECOND_BLOCK) to "inlined · poly 2" in hints)
        // The blocks carry no icon of their own: the icon is their method's.
        assertEquals(3, icons(editor).size)
    }

    fun testADeclinedSendShowsItsReason() {
        val editor = openDemo()
        decorations()
        tracedRuns.selected = demoRun()
        val hint = editor.inlayModel.getInlineElementsInRange(0, editor.document.textLength, SendInlayRenderer::class.java)
            .single { inlay -> inlay.offset == endOf("printString", FIRST_BLOCK) }
        assertTrue(hint.renderer.text, hint.renderer.text.startsWith("declined: over-weight"))
        assertTrue(hint.renderer.tooltip, "reason=over-weight" in hint.renderer.tooltip)
    }

    fun testACacheStateFollowsTheInliningHint() {
        val editor = openDemo()
        decorations()
        val run = demoRun()
        tracedRuns.selected = run
        assertTrue(hints(editor).toString(), endOf("square:", SECOND_BLOCK) to "inlined · poly 2" in hints(editor))
        // A batch of the selected run redecorates.
        run.add(records(listOf(event(2, "caches", "transition", ""","before":"polymorphic","after":"megamorphic","selector":"square:","site":3,"receiver":"Demo/Third""""))))
        assertTrue(hints(editor).toString(), endOf("square:", SECOND_BLOCK) to "inlined · mega" in hints(editor))
        // A send with only a cache state shows it alone.
        run.add(records(listOf(event(2, "caches", "transition", ""","before":"vacant","after":"monomorphic","selector":"do:","site":0,"receiver":"Array""""))))
        assertTrue(hints(editor).toString(), endOf("do:", MAIN) to "mono" in hints(editor))
    }

    fun testTheChecksumMatchesADocumentWithAnEmoji() {
        assertEquals(SMILE_CHECKSUM, sourceChecksum(SMILE))
        val editor = openDemo()
        decorations()
        tracedRuns.selected = demoRun()
        val smile = icons(editor).single { (line, _) -> line == lineOf(SMILE) }.second
        assertFalse("the document's text is the run's", smile.stale)
        assertTrue(hints(editor).toString(), hints(editor).none { (offset, _) -> offset == endOf("size", SMILE) })
    }

    fun testAnEditedMethodBecomesStale() {
        val editor = openDemo()
        decorations()
        tracedRuns.selected = demoRun()
        val mainHints = hints(editor).size
        val offset = DEMO.indexOf("x * x")
        WriteCommandAction.runWriteCommandAction(project) { editor.document.insertString(offset, "1 + ") }
        val icons = icons(editor)
        val square = icons.single { (line, _) -> line == lineOf(SQUARE) }.second
        assertTrue(square.stale)
        assertEquals("Changed since the run", square.tooltipText)
        val main = icons.single { (line, _) -> line == lineOf(MAIN) }.second
        assertFalse(main.stale)
        assertEquals(MethodState.OPTIMIZED, main.state)
        // square: had no hint, so main:args:' hints are all that is left, at their places moved by the edit.
        assertEquals(mainHints, hints(editor).size)
        assertTrue(hints(editor).toString(), endOf("square:", SECOND_BLOCK) + 4 to "inlined · poly 2" in hints(editor))
        // An edit inside a block makes its method stale, and every hint of the method goes.
        val blockOffset = editor.document.text.indexOf("printString")
        WriteCommandAction.runWriteCommandAction(project) { editor.document.insertString(blockOffset, "yourself ") }
        assertTrue(icons(editor).single { (line, _) -> line == lineOf(MAIN) }.second.stale)
        assertEquals(emptyList<Pair<Int, String>>(), hints(editor))
        // Undone, the text is the run's again.
        WriteCommandAction.runWriteCommandAction(project) { editor.document.deleteString(blockOffset, blockOffset + "yourself ".length) }
        assertFalse(icons(editor).single { (line, _) -> line == lineOf(MAIN) }.second.stale)
        assertEquals(mainHints, hints(editor).size)
    }

    fun testSwitchingRunsRedecorates() {
        val editor = openDemo()
        decorations()
        tracedRuns.selected = demoRun()
        val path = directory.resolve("Demo.ns")
        val other = TraceRun("Other", Instant.EPOCH).also { run ->
            run.add(records(listOf(HEADER, bodyJson(1, "Demo>>square:", fileSource(path), DEMO, SQUARE), event(1, "compile", "abandoned", ""","exits":3"""))))
        }
        tracedRuns.selected = other
        val icons = icons(editor)
        assertEquals(listOf(lineOf(SQUARE)), icons.map { (line, _) -> line })
        assertEquals(MethodState.ABANDONED, icons.single().second.state)
        assertEquals(emptyList<Pair<Int, String>>(), hints(editor))
        tracedRuns.selected = null
        assertEquals(emptyList<Pair<Int, StateGutterRenderer>>(), icons(editor))
    }

    fun testSettingsSwitchOffTheHints() {
        val editor = openDemo()
        val service = decorations()
        tracedRuns.selected = demoRun()
        assertFalse(hints(editor).isEmpty())
        settings.showSendHints = false
        service.settingsChanged()
        assertEquals(emptyList<Pair<Int, String>>(), hints(editor))
        assertEquals(3, icons(editor).size)
        settings.showStateIcons = false
        service.settingsChanged()
        assertEquals(emptyList<Pair<Int, StateGutterRenderer>>(), icons(editor))
        settings.showSendHints = true
        settings.showStateIcons = true
        service.settingsChanged()
        assertFalse(hints(editor).isEmpty())
        assertEquals(3, icons(editor).size)
    }

    fun testALibraryFileIsDecorated() {
        val library = directory.resolve("library")
        libraryDirectory = library
        val editor = open(library.resolve("platform/TimeFactory.ns"), DEMO)
        decorations()
        val run = TraceRun("Demo", Instant.EPOCH).also { traced ->
            traced.add(records(listOf(HEADER, bodyJson(1, "Demo>>square:", """{"library":"platform/TimeFactory.ns"}""", DEMO, SQUARE), event(1, "compile", "compiled"))))
        }
        tracedRuns.selected = run
        assertEquals(MethodState.BASELINE, icons(editor).single().second.state)
    }

    fun testAWatchedMethodShowsAnEyeUntilItRuns() {
        val editor = openDemo()
        val document = editor.document
        watchedMethods.fetcher = { _ ->
            listOf(SQUARE to "Demo>>square:", SMILE to "Demo>>smile").map { (text, name) ->
                val start = DEMO.indexOf(text)
                MethodName(name, TextRange(start, start + text.length), TextRange(start, DEMO.indexOf(" =", start)))
            }
        }
        decorations()
        watchedMethods.watch("Demo>>smile")
        // The names are asked for off the event dispatch thread; their answer redecorates.
        val deadline = System.currentTimeMillis() + 10_000
        while (icons(editor).isEmpty()) {
            assertTrue("timed out", System.currentTimeMillis() < deadline)
            PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
            Thread.sleep(20)
        }
        val eye = icons(editor).single()
        assertEquals(document.getLineNumber(DEMO.indexOf(SMILE)), eye.first)
        assertNull(eye.second.state)
        assertTrue(eye.second.watched)
        // Once a run has its body, the state icon takes the eye's place, badged.
        tracedRuns.selected = demoRun()
        val smile = icons(editor).filter { (line, _) -> line == lineOf(SMILE) }.map { (_, renderer) -> renderer }.single()
        assertEquals(MethodState.INTERPRETED, smile.state)
        assertTrue(smile.watched)
        assertFalse(icons(editor).single { (line, _) -> line == lineOf(SQUARE) }.second.watched)
        watchedMethods.unwatch("Demo>>smile")
        assertFalse(icons(editor).single { (line, _) -> line == lineOf(SMILE) }.second.watched)
    }

    fun testAFileWithCrlfLineEndsStaysDecorated() {
        val path = directory.resolve("Demo.ns")
        // The document holds the text with `\n`; the runtime hashed the file's bytes, `\r` left out.
        val editor = open(path, DEMO.replace("\n", "\r\n"))
        decorations()
        val source = fileSource(path)
        val run = TraceRun("Demo", Instant.EPOCH).also { traced ->
            traced.add(
                records(
                    listOf(
                        HEADER,
                        bodyJson(2, "Demo>>main:args:", source, DEMO, MAIN, checksum = sourceChecksum(MAIN.replace("\n", "\r\n"))),
                        event(2, "compile", "compiled"),
                    ),
                ),
            )
        }
        tracedRuns.selected = run
        val main = icons(editor).single().second
        assertEquals(MethodState.BASELINE, main.state)
        assertFalse("a multi-line method of a CRLF file is the run's", main.stale)
    }

    fun testTheMarkersOutliveAClosedEditor() {
        val editor = openDemo()
        decorations()
        tracedRuns.selected = demoRun()
        val offset = DEMO.indexOf("x * x")
        WriteCommandAction.runWriteCommandAction(project) { editor.document.insertString(offset, "1 +\n    ") }
        // A line added: records read afresh would place every method below it a line too high.
        // Saved, as a file closed after an edit usually is; the document is the one reopened.
        FileDocumentManager.getInstance().saveDocument(editor.document)
        val before = hints(editor)
        val file = checkNotNull(editor.virtualFile)
        FileEditorManager.getInstance(project).closeFile(file)
        myFixture.configureFromExistingVirtualFile(file)
        val reopened = myFixture.editor
        assertNotSame(editor, reopened)
        // The methods below the edit are placed by the markers that followed it, not by the records afresh.
        assertEquals(before, hints(reopened))
        val icons = icons(reopened)
        val document = reopened.document
        fun lineIn(part: String): Int = document.getLineNumber(document.text.indexOf(part))
        assertEquals(listOf(lineIn("square: x"), lineIn(MAIN), lineIn(SMILE)), icons.map { (line, _) -> line })
        assertEquals(listOf(true, false, false), icons.map { (_, renderer) -> renderer.stale })
    }

    fun testTwoOwnersOfOneDocumentKeepTheirOwnMarkers() {
        // Two projects' services decorating one shared document, as a core library file is: one service each.
        val editor = openDemo()
        decorations()
        tracedRuns.selected = demoRun()
        val otherRuns = TracedRuns(project) { null }.also { runs -> Disposer.register(testRootDisposable, runs) }
        DecorationsService(project, otherRuns, watchedMethods, { libraryDirectory }, { settings }).also { other ->
            Disposer.register(testRootDisposable, other)
        }
        otherRuns.selected = demoRun(name = "Other")
        assertEquals(6, icons(editor).size)
        // The other's clear takes its own decorations and markers only.
        otherRuns.selected = null
        val icons = icons(editor)
        assertEquals(listOf(lineOf(SQUARE), lineOf(MAIN), lineOf(SMILE)), icons.map { (line, _) -> line })
        assertEquals(2, hints(editor).size)
        // And the first still follows the document's edits.
        val offset = DEMO.indexOf("x * x")
        WriteCommandAction.runWriteCommandAction(project) { editor.document.insertString(offset, "1 + ") }
        assertEquals(listOf(true, false, false), icons(editor).map { (_, renderer) -> renderer.stale })
    }

    fun testACachesOnlyRunDrawsNoStateIcon() {
        val editor = openDemo()
        decorations()
        val source = fileSource(directory.resolve("Demo.ns"))
        val run = TraceRun("Demo", Instant.EPOCH).also { traced ->
            traced.add(
                records(
                    listOf(
                        """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["caches"],"listings":[]}""",
                        bodyJson(2, "Demo>>main:args:", source, DEMO, MAIN, sends = listOf(sendJson(DEMO, MAIN, 3, 0, "args do: [:each | each printString]", "do:"))),
                        event(2, "caches", "transition", ""","before":"vacant","after":"monomorphic","selector":"do:","site":0,"receiver":"Array""""),
                    ),
                ),
            )
        }
        tracedRuns.selected = run
        // Nothing traced says how main:args: ran: no icon, but its send's cache.
        assertEquals(emptyList<Pair<Int, StateGutterRenderer>>(), icons(editor))
        assertEquals(listOf(endOf("do:", MAIN) to "mono"), hints(editor))
    }

    fun testAFilteredOutCalleeDrawsNoStateIcon() {
        val editor = openDemo()
        decorations()
        val source = fileSource(directory.resolve("Demo.ns"))
        val filters = TraceFilters(listOf("Demo>>main:args:"), emptyList(), noEvents = false, noListings = true)
        val run = TraceRun("Demo", Instant.EPOCH, filters).also { traced ->
            traced.add(
                records(
                    listOf(
                        HEADER,
                        // square: is described as the callee of main:args:'s plan; its own events were filtered out.
                        bodyJson(1, "Demo>>square:", source, DEMO, SQUARE),
                        bodyJson(2, "Demo>>main:args:", source, DEMO, MAIN),
                        event(2, "compile", "compiled"),
                        event(2, "inlining", "inlined", ""","in-body":2,"at":7,"callee":1,"key":null,"entered":null"""),
                    ),
                ),
            )
        }
        tracedRuns.selected = run
        val icons = icons(editor)
        assertEquals(listOf(lineOf(MAIN)), icons.map { (line, _) -> line })
        assertEquals(MethodState.BASELINE, icons.single().second.state)
    }

    fun testARangeOutsideTheTextIsNotDecorated() {
        val editor = openDemo()
        decorations()
        val path = directory.resolve("Demo.ns")
        val outside = """{"kind":"body","body":9,"name":"Demo>>gone","family":null,"module":{"id":"Demo","version":null},"source":${fileSource(path)},"range":[100,0,101,4],"checksum":"0000000000000000","statements":[],"sends":[{"at":0,"site":0,"call":[100,0,100,4],"selector":[[100,0,100,4]]}]}"""
        val pastTheLine = """{"kind":"body","body":10,"name":"Demo>>wide","family":null,"module":{"id":"Demo","version":null},"source":${fileSource(path)},"range":[1,0,1,400],"checksum":"0000000000000000","statements":[],"sends":[]}"""
        val run = TraceRun("Demo", Instant.EPOCH).also { traced ->
            traced.add(records(listOf(HEADER, outside, pastTheLine, bodyJson(1, "Demo>>square:", fileSource(path), DEMO, SQUARE), event(9, "compile", "compiled"))))
        }
        tracedRuns.selected = run
        assertEquals(listOf(lineOf(SQUARE)), icons(editor).map { (line, _) -> line })
    }

    fun testADeoptimizedMethodShowsItsIcon() {
        val editor = openDemo()
        decorations()
        val source = fileSource(directory.resolve("Demo.ns"))
        val run = TraceRun("Demo", Instant.EPOCH).also { traced ->
            traced.add(
                records(
                    listOf(
                        HEADER,
                        bodyJson(1, "Demo>>square:", source, DEMO, SQUARE),
                        event(1, "compile", "compiled"),
                        event(1, "optimize", "optimized", ""","key":null,"entered":null,"bytes":100"""),
                        event(1, "deoptimize", "invalidated", ""","key":null,"entered":null,"cause":"bulk-forward""""),
                    ),
                ),
            )
        }
        tracedRuns.selected = run
        val square = icons(editor).single().second
        assertEquals(MethodState.DEOPTIMIZED, square.state)
        assertFalse(square.stale)
        assertEquals("deoptimized · 0 copies · 1 deoptimizations · last: invalidated, bulk-forward", square.tooltipText)
        assertSame(StateGutterRenderer(MethodState.DEOPTIMIZED, stale = false, watched = false, "", square.popupMenuActions).icon, square.icon)
        assertNotSame(StateGutterRenderer(MethodState.OPTIMIZED, stale = false, watched = false, "", square.popupMenuActions).icon, square.icon)
    }

    /** The texts of [renderer]'s menu, in its order. */
    private fun menu(renderer: StateGutterRenderer): List<String?> =
        (renderer.popupMenuActions as DefaultActionGroup).childActionsOrStubs.map { action -> action.templateText }

    fun testAListedMethodWithoutAStateShowsTheNeutralIconAndItsMenu() {
        val editor = openDemo()
        decorations()
        val source = fileSource(directory.resolve("Demo.ns"))
        val run = TraceRun("Demo", Instant.EPOCH).also { traced ->
            traced.add(
                records(
                    listOf(
                        """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":[],"listings":["bytecode"]}""",
                        bodyJson(1, "Demo>>square:", source, DEMO, SQUARE),
                        bodyJson(5, "Demo>>smile", source, DEMO, SMILE, checksum = SMILE_CHECKSUM),
                        """{"kind":"bytecode","body":1,"bodies":[{"body":1,"instructions":[{"index":0,"statement":0,"text":"Return r0"}]}]}""",
                    ),
                ),
            )
        }
        tracedRuns.selected = run
        // Only the method the run lists: smile is described, never listed.
        val square = icons(editor).single()
        assertEquals(lineOf(SQUARE), square.first)
        val renderer = square.second
        assertNull(renderer.state)
        assertTrue(renderer.listed)
        assertEquals("The run listed this method's code but traced nothing of how it ran", renderer.tooltipText)
        assertNotSame(StateGutterRenderer(null, stale = false, watched = true, "", renderer.popupMenuActions).icon, renderer.icon)
        assertEquals(listOf("Watch Method", "Show Code", "Show Inlining", "Open in Tool Window"), menu(renderer))
    }

    fun testAnOptimizedMethodWhoseCopiesCannotBeHeardSaysWhichCategoriesItNeeds() {
        val editor = openDemo()
        decorations()
        val source = fileSource(directory.resolve("Demo.ns"))
        val run = TraceRun("Demo", Instant.EPOCH).also { traced ->
            traced.add(
                records(
                    listOf(
                        """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["compile","optimize","code-zone"],"listings":[]}""",
                        bodyJson(1, "Demo>>square:", source, DEMO, SQUARE),
                        event(1, "compile", "compiled"),
                        event(1, "optimize", "optimized", ""","key":null,"entered":null,"bytes":100"""),
                    ),
                ),
            )
        }
        tracedRuns.selected = run
        val square = icons(editor).single().second
        assertEquals(MethodState.BASELINE, square.state)
        assertEquals("baseline · 0 copies · 0 deoptimizations · shown as its baseline tier: trace deoptimize too to see its optimized copies", square.tooltipText)
    }

    private companion object {
        val DEMO = """
            |class Demo usingPlatform: platform = (
            |) (
            |  square: x = (
            |    ^x * x
            |  )
            |  main: args = (
            |    args do: [:each | each printString].
            |    args collect: [:each | self square: each].
            |  )
            |  smile = (
            |    ^'${"😀"}' size
            |  )
            |)
            |
        """.trimMargin()
        const val SQUARE = "square: x = (\n    ^x * x\n  )"
        const val MAIN = "main: args = (\n    args do: [:each | each printString].\n    args collect: [:each | self square: each].\n  )"
        const val FIRST_BLOCK = "[:each | each printString]"
        const val SECOND_BLOCK = "[:each | self square: each]"
        const val SMILE = "smile = (\n    ^'😀' size\n  )"

        /** FNV-1a 64 of [SMILE]'s UTF-8 bytes, computed outside the plugin. */
        const val SMILE_CHECKSUM = "365a1aecc2f3c0f7"

        const val HEADER = """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["compile","optimize","inlining","deoptimize","code-zone","caches"],"listings":[]}"""
    }
}
