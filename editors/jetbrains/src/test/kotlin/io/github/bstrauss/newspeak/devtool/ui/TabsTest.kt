package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.util.Disposer
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.google.gson.JsonObject
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.model.Event
import io.github.bstrauss.newspeak.devtool.model.KEPT_EVENTS
import io.github.bstrauss.newspeak.devtool.model.MethodState
import io.github.bstrauss.newspeak.devtool.model.IndexedTrace
import io.github.bstrauss.newspeak.devtool.model.TraceRecord
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.nio.file.Files
import java.time.Instant
import javax.swing.RowSorter
import javax.swing.SortOrder
import javax.swing.tree.DefaultMutableTreeNode
import javax.swing.tree.TreePath

class TabsTest : BasePlatformTestCase() {
    /** The file the records are written to, as the reader hands a run its listings and plans: as entries naming their places. */
    private val trace = IndexedTrace()

    private fun records(lines: List<String>): List<TraceRecord> = trace.records(lines)

    private fun fixtureRun(vararg extra: String): TraceRun {
        val lines = checkNotNull(javaClass.getResource("/devtool/trace.jsonl")).readText().lines().filter { line -> line.isNotEmpty() }
        return trace.run().also { run -> run.add(records(lines + extra)) }
    }

    fun testMethodsListsEachRootWithItsState() {
        val tab = MethodsTab(project, fixtureRun(), testRootDisposable)
        val rows = tab.model.items.associateBy { row -> row.name }
        assertEquals(
            setOf(
                "Demo>>measure:", "Demo>>square:", "Demo>>main:args:", "Demo/Circle>>area", "Demo/Square>>area",
                "newspeak.core.Kernel/Number>>to:do:",
            ),
            rows.keys,
        )
        assertEquals(MethodState.BASELINE, rows.getValue("Demo>>square:").state)
        assertEquals(MethodState.INTERPRETED, rows.getValue("Demo>>main:args:").state)
        val optimized = rows.getValue("newspeak.core.Kernel/Number>>to:do:")
        assertEquals(MethodState.OPTIMIZED, optimized.state)
        assertEquals(1, optimized.copies)
        assertEquals(1269, optimized.optimizedBytes)
        assertEquals(2816, optimized.baselineBytes)
        assertEquals(0, optimized.twinBytes)
    }

    fun testMethodsShowTheBytesOfEveryTier() {
        val run = fixtureRun(
            eventLine(3, "compile", "compiled-twin", ""","bytes":1800"""),
            """{"kind":"code","body":3,"tier":"twin","key":null,"entered":null,"architecture":"x64","base":4096,"bytes":1900,"instructions":[],"data":[],"scopes":[],"mark":null}""",
        )
        val tab = MethodsTab(project, run, testRootDisposable)
        val square = tab.model.items.single { row -> row.name == "Demo>>square:" }
        assertEquals(1216, square.baselineBytes)
        // The largest of the twin's installs, whether an event or a listing tells of it.
        assertEquals(1900, square.twinBytes)
        assertEquals(0, square.optimizedBytes)
        val columns = (0 until tab.model.columnCount).map { index -> tab.model.getColumnName(index) }
        assertEquals(listOf("Baseline bytes", "Twin bytes", "Optimized bytes"), columns.subList(5, 8))
    }

    fun testMethodsListABodyTheRunDidNotDescribe() {
        val run = fixtureRun(eventLine(40, "compile", "compiled", ""","trigger":"call","bytes":512"""))
        val tab = MethodsTab(project, run, testRootDisposable)
        val undescribed = tab.model.items.single { row -> row.body == 40 }
        // Named as the events name it.
        assertEquals("#40", undescribed.name)
        assertEquals(MethodState.BASELINE, undescribed.state)
        assertEquals(512, undescribed.baselineBytes)
        // A body only a plan names has a row too.
        val planned = fixtureRun("""{"kind":"inlining","body":41,"key":null,"entered":null,"scopes":[{"scope":0,"body":41,"parent":null}],"arms":[],"declined":[],"outcome":{"refused":"zone-full"}}""")
        assertEquals("#41", MethodsTab(project, planned, testRootDisposable).model.items.single { row -> row.body == 41 }.name)
    }

    fun testMethodsSortByCopies() {
        val tab = MethodsTab(project, fixtureRun(), testRootDisposable)
        val copies = 2
        checkNotNull(tab.table.rowSorter).sortKeys = listOf(RowSorter.SortKey(copies, SortOrder.DESCENDING))
        assertEquals("newspeak.core.Kernel/Number>>to:do:", tab.table.getRow(0).name)
        checkNotNull(tab.table.rowSorter).sortKeys = listOf(RowSorter.SortKey(copies, SortOrder.ASCENDING))
        assertEquals(1, tab.table.getRow(tab.table.rowCount - 1).copies)
    }

    fun testEventsFilterByCategory() {
        val tab = EventsTab(project, fixtureRun(), testRootDisposable)
        val all = tab.visibleRows.size
        assertTrue(tab.visibleRows.any { row -> row.event.category == "caches" })
        checkNotNull(tab.categoryBoxes["caches"]).isSelected = false
        assertTrue(tab.visibleRows.none { row -> row.event.category == "caches" })
        assertTrue(tab.visibleRows.size < all)
        checkNotNull(tab.categoryBoxes["caches"]).isSelected = true
        tab.bodyFilter.text = "Circle"
        assertEquals(setOf("Demo/Circle>>area"), tab.visibleRows.map { row -> row.body }.toSet())
    }

    fun testEventsTextViewMatchesTheTextTrace() {
        val bodies = listOf("run" to 100, "helper" to 101).map { (name, number) -> """{"kind":"body","body":$number,"name":"Demo>>$name","module":{"id":"Demo","version":null},"range":null,"checksum":"0000000000000000","statements":[],"sends":[]}""" }
        val declined = """{"kind":"event","category":"inlining","body":100,"event":"declined","in-body":100,"callee":101,"reason":"over-weight","at":7,"key":null,"entered":null}"""
        val run = fixtureRun(*bodies.toTypedArray(), declined)
        val tab = EventsTab(project, run, testRootDisposable)
        tab.textView.isSelected = true
        val lines = tab.text.text.lines()
        assertEquals("[inlining] {Demo>>run} declined in-body={Demo>>run} callee={Demo>>helper} reason=over-weight at=7", lines.last())
        assertTrue(
            lines.toString(),
            "[caches] {Demo>>main:args:} vacant->monomorphic selector=#to:do: site=16 receiver={newspeak.core.Kernel/Integer}" in lines,
        )
        assertTrue(
            lines.toString(),
            "[optimize] {[] in Demo>>main:args:} requested spent-by={[] in Demo>>main:args:} walk-end=not-recompilable" in lines,
        )
        assertTrue(
            lines.toString(),
            "[inlining] {[] in Demo>>main:args:} declined-no-body count=2 reasons=not-expressed:2" in lines,
        )
    }

    /** A run's views are made before its records arrive: the plans and the grouped events that arrive later are shown, a row each. */
    fun testTheTreesShowWhatArrivesAfterThem() {
        val run = trace.run()
        val inlining = InliningTab(project, run, testRootDisposable, immediateReads())
        val events = EventsTab(project, run, testRootDisposable)
        events.groupByMethod.isSelected = true
        val lines = checkNotNull(javaClass.getResource("/devtool/trace.jsonl")).readText().lines().filter { line -> line.isNotEmpty() }
        run.add(records(lines))
        assertTrue(run.plans.isNotEmpty())
        assertEquals("a row for each plan", run.plans.size, inlining.tree.rowCount)
        assertEquals("a row for each method", events.treeModel.getChildCount(events.treeModel.root), events.tree.rowCount)
        assertTrue(events.tree.rowCount > 0)
    }

    fun testInliningTreeNestsScopesUnderTheirParents() {
        val tab = InliningTab(project, fixtureRun(), testRootDisposable, immediateReads())
        val roots = (tab.treeModel.root as DefaultMutableTreeNode).children().toList().map { node -> node as DefaultMutableTreeNode }
        val plan = roots.first { node -> node.toString().startsWith("[] in Demo>>main:args:") }
        assertTrue(plan.toString(), "installed, 3601 bytes" in plan.toString())
        // Read from the run's file as the plan is expanded.
        tab.tree.expandPath(TreePath(plan.path))
        val scopes = plan.children().toList().map { node -> node as DefaultMutableTreeNode }
        // The root scope hangs from the plan; the scopes it inlined hang from it, and theirs from them.
        val rootScope = scopes.single { node -> (node.userObject as InliningNode).body == 5 }
        val inlined = rootScope.children().toList().map { node -> (node as DefaultMutableTreeNode).toString() }
        assertTrue(inlined.toString(), "Demo>>square:" in inlined)
        val measure = rootScope.children().toList().map { node -> node as DefaultMutableTreeNode }.first { node -> node.toString() == "Demo>>measure:" }
        val nested = measure.children().toList().map { node -> node.toString() }
        assertTrue(nested.toString(), "Demo/Square>>area" in nested && "Demo/Circle>>area" in nested)
        // The declines of the root scope are leaves.
        assertTrue(rootScope.children().toList().any { node -> node.toString().contains("declined") })
    }

    private fun mainPlan(tab: InliningTab): DefaultMutableTreeNode =
        (tab.treeModel.root as DefaultMutableTreeNode).children().toList().map { node -> node as DefaultMutableTreeNode }
            .first { node -> node.toString().startsWith("[] in Demo>>main:args:") }

    /**
     * A plan's nodes are read from the file as it is first expanded, and
     * kept across a collapse; whether the scopes inside stay open is the
     * IDE's setting "collapse recursively", as in every tree.
     */
    fun testAPlanIsReadWhenExpandedAndKeptWhenCollapsed() {
        var readCount = 0
        val counting = RecordReads({ task -> readCount++; task.run() }, { task, _, _ -> task.run() }, { _, _, _ -> })
        val tab = InliningTab(project, fixtureRun(), testRootDisposable, counting)
        val plan = mainPlan(tab)
        val path = TreePath(plan.path)
        assertEquals("nothing is said before a read has taken long", listOf(""), plan.children().toList().map { node -> node.toString() })
        tab.tree.expandPath(path)
        assertTrue(tab.tree.isExpanded(path))
        assertEquals(listOf("[] in Demo>>main:args:"), plan.children().toList().map { node -> node.toString() })
        val scope = plan.firstChild
        tab.tree.expandPath(TreePath((scope as DefaultMutableTreeNode).path))
        tab.tree.collapsePath(path)
        assertSame("the nodes are kept", scope, plan.firstChild)
        tab.tree.expandPath(path)
        assertSame(scope, plan.firstChild)
        assertEquals("read once", 1, readCount)
    }

    /** On a pooled thread, the plan arrives after the expansion, which stays. */
    fun testAPlanIsReadOffTheEventDispatchThread() {
        val tab = InliningTab(project, fixtureRun(), testRootDisposable)
        val plan = mainPlan(tab)
        val path = TreePath(plan.path)
        tab.tree.expandPath(path)
        PlatformTestUtil.waitWithEventsDispatching(
            "the plan arrives",
            { plan.childCount == 1 && plan.firstChild.toString() == "[] in Demo>>main:args:" },
            WAIT_SECONDS,
        )
        assertTrue(tab.tree.isExpanded(path))
    }

    /** A plan whose read failed is read again at its next expansion; only a plan that was read keeps its nodes. */
    fun testAPlanSaysItIsReadOnlyOnceTheReadTakesLong() {
        val reads = ArrayList<Runnable>()
        val notices = ArrayList<Runnable>()
        val tab = InliningTab(project, fixtureRun(), testRootDisposable, RecordReads({ task -> reads += task }, { task, _, _ -> task.run() }, { task, _, _ -> notices += task }))
        val plan = mainPlan(tab)
        tab.tree.expandPath(TreePath(plan.path))
        assertEquals(listOf(""), plan.children().toList().map { node -> node.toString() })
        notices.single().run()
        assertEquals(listOf(NewspeakBundle.message("devtool.record.reading")), plan.children().toList().map { node -> node.toString() })
        reads.single().run()
        assertEquals(listOf("[] in Demo>>main:args:"), plan.children().toList().map { node -> node.toString() })
    }

    fun testAPlanWhoseReadFailedIsReadAgain() {
        var readCount = 0
        val counting = RecordReads({ task -> readCount++; task.run() }, { task, _, _ -> task.run() }, { _, _, _ -> })
        val tab = InliningTab(project, fixtureRun(), testRootDisposable, counting)
        val bytes = Files.readAllBytes(trace.file)
        Files.delete(trace.file)
        val plan = mainPlan(tab)
        val path = TreePath(plan.path)
        tab.tree.expandPath(path)
        assertEquals(NewspeakBundle.message("devtool.record.unreadable", trace.file.toString(), NewspeakBundle.message("devtool.record.gone")), plan.firstChild.toString())
        tab.tree.collapsePath(path)
        Files.write(trace.file, bytes)
        tab.tree.expandPath(path)
        assertEquals(2, readCount)
        assertEquals(listOf("[] in Demo>>main:args:"), plan.children().toList().map { node -> node.toString() })
        tab.tree.collapsePath(path)
        tab.tree.expandPath(path)
        assertEquals("a plan read is kept", 2, readCount)
    }

    /** A plan whose file is gone says so in its place, and throws nothing. */
    fun testAPlanWhoseFileIsGoneSaysSo() {
        val tab = InliningTab(project, fixtureRun(), testRootDisposable, immediateReads())
        Files.delete(trace.file)
        val plan = mainPlan(tab)
        tab.tree.expandPath(TreePath(plan.path))
        val message = plan.children().toList().single().toString()
        assertEquals(NewspeakBundle.message("devtool.record.unreadable", trace.file.toString(), NewspeakBundle.message("devtool.record.gone")), message)
    }

    fun testNavigationOpensALibraryFileUnderTheLibraryDirectory() {
        val directory = Files.createTempDirectory("newspeak-library")
        Files.createDirectories(directory.resolve("platform"))
        val library = directory.resolve("Kernel.ns")
        Files.writeString(library, "line zero\nline one\n  to: x do: y\n")
        val body = """{"kind":"body","body":1,"name":"K>>to:do:","module":{"id":"K","version":null},"source":{"library":"Kernel.ns"},"range":[2,2,2,14],"checksum":"0000000000000000","statements":[],"sends":[]}"""
        val run = TraceRun("Demo", Instant.EPOCH).also { traced -> traced.add(records(listOf(body))) }
        val manager = FileEditorManager.getInstance(project)
        navigate(project, run, 1, null, libraryDirectory = directory)
        // A file the virtual file system does not hold yet is refreshed off the event dispatch thread, then opened.
        PlatformTestUtil.waitWithEventsDispatching("the file opens", { manager.selectedTextEditor != null }, WAIT_SECONDS)
        val editor = checkNotNull(manager.selectedTextEditor)
        assertEquals("Kernel.ns", checkNotNull(editor.virtualFile).name)
        assertEquals(2, editor.caretModel.logicalPosition.line)
        assertEquals(2, editor.caretModel.logicalPosition.column)
        // Known by now, it opens at once.
        manager.closeFile(checkNotNull(editor.virtualFile))
        navigate(project, run, 1, null, libraryDirectory = directory)
        assertEquals("Kernel.ns", checkNotNull(manager.selectedTextEditor?.virtualFile).name)
        // A body without a description opens nothing.
        manager.closeFile(checkNotNull(manager.selectedTextEditor?.virtualFile))
        navigate(project, run, 99, null, libraryDirectory = directory)
        assertNull(FileEditorManager.getInstance(project).selectedTextEditor)
    }

    private fun eventLine(body: Int, category: String, event: String, fields: String = ""): String =
        """{"kind":"event","category":"$category","body":$body,"event":"$event"$fields}"""

    fun testMethodsCountABlockUnderItsMethod() {
        val run = fixtureRun(
            eventLine(5, "inlining", "declined", ""","key":null,"entered":null,"in-body":5,"callee":3,"reason":"over-weight","at":7"""),
            eventLine(5, "optimize", "optimized", ""","key":null,"entered":null,"bytes":100"""),
        )
        val tab = MethodsTab(project, run, testRootDisposable)
        assertTrue("a block has no row of its own", tab.model.items.none { row -> row.body == 5 })
        val main = tab.model.items.single { row -> row.name == "Demo>>main:args:" }
        // The state and the copies are the root's own; the block's bytes and declines are added to it.
        assertEquals(MethodState.INTERPRETED, main.state)
        assertEquals(0, main.copies)
        assertEquals(3601, main.optimizedBytes)
        assertEquals(1, main.declined)
    }

    fun testMethodsCountTheBlocksOfARootWhoseCodeWasGoneUnderIt() {
        val block = { number: Int -> """{"kind":"body","body":$number,"name":"[] in Demo>>gone","family":69,"module":{"id":"Demo","version":null},"source":null,"range":null,"checksum":"0000000000000001","statements":[],"sends":[]}""" }
        val run = fixtureRun(
            block(70),
            block(71),
            eventLine(70, "compile", "compiled", ""","trigger":"call","bytes":64"""),
            eventLine(71, "compile", "compiled", ""","trigger":"call","bytes":96"""),
            eventLine(69, "caches", "transition", ""","before":"vacant","after":"monomorphic","selector":"size","site":0,"receiver":"Demo""""),
        )
        val tab = MethodsTab(project, run, testRootDisposable)
        assertTrue("no block has a row of its own", tab.model.items.none { row -> row.body == 70 || row.body == 71 })
        val gone = tab.model.items.single { row -> row.body == 69 }
        assertEquals("Demo>>gone", gone.name)
        assertEquals(96, gone.baselineBytes)
    }

    fun testEventsGroupABlockUnderItsMethodAndGrowWithTheRun() {
        val run = fixtureRun()
        val tab = EventsTab(project, run, testRootDisposable)
        tab.groupByMethod.isSelected = true
        val root = tab.treeModel.root as DefaultMutableTreeNode
        val groups = root.children().toList().map { node -> node.toString() }
        assertTrue(groups.toString(), "Demo>>main:args:" in groups && "[] in Demo>>main:args:" !in groups)
        val before = tab.visibleRows.size
        tab.table.setRowSelectionInterval(3, 3)
        val selected = tab.table.selectedObject
        val treeCount = (0 until root.childCount).sumOf { index -> root.getChildAt(index).childCount }
        run.add(records(listOf(eventLine(2, "compile", "abandoned", ""","exits":3"""))))
        assertEquals(before + 1, tab.table.rowCount)
        assertSame("the selection survives a batch", selected, tab.table.selectedObject)
        assertEquals(treeCount + 1, (0 until root.childCount).sumOf { index -> root.getChildAt(index).childCount })
        // The text is made only when it is shown, and kept up to date while it is.
        assertEquals("", tab.text.text)
        tab.textView.isSelected = true
        assertEquals(before + 1, tab.text.text.lines().size)
        run.add(records(listOf(eventLine(2, "compile", "revived", ""","by":"decay""""))))
        assertEquals(before + 2, tab.text.text.lines().size)
    }

    fun testEventsShowTheEventsTheRunKeepsAndSayHowManyWereDropped() {
        val run = TraceRun("Demo", Instant.EPOCH)
        val body = """{"kind":"body","body":1,"name":"Demo>>run","module":{"id":"Demo","version":null},"range":null,"checksum":"0000000000000000","statements":[],"sends":[]}"""
        run.add(records(listOf(body)))
        fun events(from: Int, count: Int) = List(count) { index ->
            Event("osr", 1, "osr-entered", JsonObject().apply { addProperty("at", from + index) })
        }
        run.add(events(0, KEPT_EVENTS))
        val tab = EventsTab(project, run, testRootDisposable)
        assertFalse("nothing was dropped yet", tab.droppedLabel.isVisible)
        tab.groupByMethod.isSelected = true
        tab.textView.isSelected = true
        val root = tab.treeModel.root as DefaultMutableTreeNode
        tab.table.setRowSelectionInterval(10, 10)
        val selected = tab.table.selectedObject
        run.add(events(KEPT_EVENTS, 3))
        assertEquals(KEPT_EVENTS, tab.table.rowCount)
        assertEquals("the oldest rows go", 3L, tab.visibleRows.first().order)
        assertSame("the selection stays on its row", selected, tab.table.selectedObject)
        val lines = tab.text.text.lines()
        assertEquals(KEPT_EVENTS, lines.size)
        assertEquals("[osr] {Demo>>run} osr-entered at=3", lines.first())
        assertEquals("[osr] {Demo>>run} osr-entered at=${KEPT_EVENTS + 2}", lines.last())
        // The tree was fed before the text was shown; it lost its oldest rows too.
        assertEquals(KEPT_EVENTS - 3, (root.getChildAt(0) as DefaultMutableTreeNode).childCount)
        assertEquals("at=3", ((root.getChildAt(0).getChildAt(0) as DefaultMutableTreeNode).userObject as EventRow).fields.trim())
        assertTrue(tab.droppedLabel.isVisible)
        assertEquals(NewspeakBundle.message("devtool.events.dropped", 3L, KEPT_EVENTS), tab.droppedLabel.text)
    }

    fun testABatchKeepsTheInliningTreesExpansionAndSelection() {
        val run = fixtureRun()
        val tab = InliningTab(project, run, testRootDisposable, immediateReads())
        val root = tab.treeModel.root as DefaultMutableTreeNode
        val plan = root.children().toList().map { node -> node as DefaultMutableTreeNode }
            .first { node -> node.toString().startsWith("[] in Demo>>main:args:") }
        tab.tree.expandPath(TreePath(plan.path))
        val rootScope = plan.firstChild as DefaultMutableTreeNode
        val scopePath = TreePath(rootScope.path)
        tab.tree.expandPath(scopePath)
        tab.tree.selectionPath = scopePath
        val plans = root.childCount
        run.add(records(listOf("""{"kind":"inlining","body":3,"key":null,"entered":null,"scopes":[{"scope":0,"body":3,"parent":null}],"arms":[],"declined":[],"outcome":{"refused":"zone-full"}}""")))
        assertEquals("the new plan is appended", plans + 1, root.childCount)
        assertTrue("the plan stays expanded", tab.tree.isExpanded(scopePath))
        assertEquals("the selection stays", scopePath, tab.tree.selectionPath)
    }

    fun testMethodsLeaveTheStateOfAMethodTheRunKnowsNothingOfBlank() {
        val header = """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":["caches"],"listings":[]}"""
        val body = """{"kind":"body","body":1,"name":"Demo>>run","module":{"id":"Demo","version":null},"range":null,"checksum":"0000000000000000","statements":[],"sends":[]}"""
        val run = TraceRun("Demo", Instant.EPOCH).also { traced -> traced.add(records(listOf(header, body))) }
        val tab = MethodsTab(project, run, testRootDisposable)
        assertNull(tab.model.items.single().state)
        assertEquals("", tab.model.getValueAt(0, 1))
    }

    fun testAScopeOpensTheSendThatInlinedIt() {
        val tab = InliningTab(project, fixtureRun(), testRootDisposable, immediateReads())
        val plan = (tab.treeModel.root as DefaultMutableTreeNode).children().toList().map { node -> node as DefaultMutableTreeNode }
            .first { node -> node.toString().startsWith("[] in Demo>>main:args:") }
        tab.tree.expandPath(TreePath(plan.path))
        val rootScope = plan.children().toList().map { node -> node as DefaultMutableTreeNode }.single()
        val square = rootScope.children().toList().map { node -> node as DefaultMutableTreeNode }.first { node -> node.toString() == "Demo>>square:" }
        val node = square.userObject as InliningNode
        assertEquals(5, node.body)
        assertEquals(3, node.at)
    }

    fun testARunsTabNamesTheCategoriesTheTierIconsNeed() {
        fun header(categories: String) =
            """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64","categories":[$categories],"listings":["bytecode"]}"""
        val run = TraceRun("Demo", Instant.EPOCH).also { traced -> traced.add(records(listOf(header(""""compile","caches"""")))) }
        val panel = RunPanel(project, run).also { created -> Disposer.register(testRootDisposable, created) }
        assertEquals("No icons of baseline, twin or optimized code: trace deoptimize and code-zone too", panel.shownTierNotice)
        // A listing-only run shows no tier at all.
        assertEquals("No tier icons: trace compile, deoptimize and code-zone too", tierNotice(listOf("compile", "deoptimize", "code-zone")))
        assertEquals("No icons of optimized copies, which show as their baseline or twin tier: trace deoptimize too", tierNotice(listOf("deoptimize")))
        assertNull(tierNotice(emptyList()))
        // Every category a tier needs traced: no line.
        val full = TraceRun("Full", Instant.EPOCH).also { traced -> traced.add(records(listOf(header(""""compile","deoptimize","code-zone"""")))) }
        assertEquals(emptyList<String>(), full.missingTierCategories())
        // A header that does not say what it traces is taken to trace everything.
        val unsaid = TraceRun("Unsaid", Instant.EPOCH).also { traced ->
            traced.add(records(listOf("""{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64"}""")))
        }
        assertEquals(emptyList<String>(), unsaid.missingTierCategories())
    }

    private companion object {
        const val WAIT_SECONDS = 10
    }
}
