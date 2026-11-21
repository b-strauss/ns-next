package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.devtool.model.MethodState
import io.github.bstrauss.newspeak.devtool.model.TraceRecord
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.nio.file.Files
import java.time.Instant
import javax.swing.RowSorter
import javax.swing.SortOrder
import javax.swing.tree.DefaultMutableTreeNode

class TabsTest : BasePlatformTestCase() {
    private fun records(lines: List<String>): List<TraceRecord> = lines.map { line -> checkNotNull(TraceRecord.parse(line)) }

    private fun fixtureRun(vararg extra: String): TraceRun {
        val lines = checkNotNull(javaClass.getResource("/devtool/trace.jsonl")).readText().lines().filter { line -> line.isNotEmpty() }
        return TraceRun("Demo", Instant.EPOCH).also { run -> run.add(records(lines + extra)) }
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
        assertEquals(1269, optimized.codeBytes)
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

    fun testInliningTreeNestsScopesUnderTheirParents() {
        val tab = InliningTab(project, fixtureRun(), testRootDisposable)
        val roots = (tab.treeModel.root as DefaultMutableTreeNode).children().toList().map { node -> node as DefaultMutableTreeNode }
        val plan = roots.first { node -> node.toString().startsWith("[] in Demo>>main:args:") }
        assertTrue(plan.toString(), "installed, 3601 bytes" in plan.toString())
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

    fun testNavigationOpensALibraryFileUnderTheLibraryDirectory() {
        val directory = Files.createTempDirectory("newspeak-library")
        Files.createDirectories(directory.resolve("platform"))
        val library = directory.resolve("Kernel.ns")
        Files.writeString(library, "line zero\nline one\n  to: x do: y\n")
        val body = """{"kind":"body","body":1,"name":"K>>to:do:","module":{"id":"K","version":null},"source":{"library":"Kernel.ns"},"range":[2,2,2,14],"checksum":"0000000000000000","statements":[],"sends":[]}"""
        val run = TraceRun("Demo", Instant.EPOCH).also { traced -> traced.add(records(listOf(body))) }
        navigate(project, run, 1, null, libraryDirectory = directory)
        val editor = checkNotNull(FileEditorManager.getInstance(project).selectedTextEditor)
        assertEquals("Kernel.ns", checkNotNull(editor.virtualFile).name)
        assertEquals(2, editor.caretModel.logicalPosition.line)
        assertEquals(2, editor.caretModel.logicalPosition.column)
        // A body without a description opens nothing.
        FileEditorManager.getInstance(project).closeFile(checkNotNull(editor.virtualFile))
        navigate(project, run, 99, null, libraryDirectory = directory)
        assertNull(FileEditorManager.getInstance(project).selectedTextEditor)
    }

    private fun eventLine(body: Int, category: String, event: String, fields: String = ""): String =
        """{"kind":"event","category":"$category","body":$body,"event":"$event"$fields}"""

    fun testMethodsCountABlockUnderItsMethod() {
        val run = fixtureRun(eventLine(5, "inlining", "declined", ""","in-body":5,"callee":3,"reason":"over-weight","at":7"""))
        val tab = MethodsTab(project, run, testRootDisposable)
        assertTrue("a block has no row of its own", tab.model.items.none { row -> row.body == 5 })
        val main = tab.model.items.single { row -> row.name == "Demo>>main:args:" }
        // The state and the copies are the root's own; the block's bytes and declines are added to it.
        assertEquals(MethodState.INTERPRETED, main.state)
        assertEquals(0, main.copies)
        assertEquals(3601, main.codeBytes)
        assertEquals(1, main.declined)
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

    fun testAScopeOpensTheSendThatInlinedIt() {
        val tab = InliningTab(project, fixtureRun(), testRootDisposable)
        val plan = (tab.treeModel.root as DefaultMutableTreeNode).children().toList().map { node -> node as DefaultMutableTreeNode }
            .first { node -> node.toString().startsWith("[] in Demo>>main:args:") }
        val rootScope = plan.children().toList().map { node -> node as DefaultMutableTreeNode }.single()
        val square = rootScope.children().toList().map { node -> node as DefaultMutableTreeNode }.first { node -> node.toString() == "Demo>>square:" }
        val node = square.userObject as InliningNode
        assertEquals(5, node.body)
        assertEquals(3, node.at)
    }
}
