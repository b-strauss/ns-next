package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.Disposable
import com.intellij.openapi.project.Project
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBScrollPane
import com.intellij.ui.dualView.TreeTableView
import com.intellij.ui.table.TableView
import com.intellij.ui.treeStructure.treetable.ListTreeTableModelOnColumns
import com.intellij.ui.treeStructure.treetable.TreeColumnInfo
import com.intellij.util.ui.ColumnInfo
import com.intellij.util.ui.ListTableModel
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.model.MethodState
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.awt.BorderLayout
import java.awt.CardLayout
import java.awt.FlowLayout
import java.awt.event.MouseAdapter
import java.awt.event.MouseEvent
import javax.swing.JPanel
import javax.swing.tree.DefaultMutableTreeNode
import javax.swing.tree.TreePath

/**
 * A method of the Methods tab, or one of its bodies where the tab groups
 * by method: a body and what its events say of it -- a method's row
 * counting its blocks' --, its [state] null where the run knows nothing
 * of how it ran, and the bytes of its largest install of each tier --
 * [baselineBytes], [twinBytes], [optimizedBytes] -- 0 for a tier never
 * installed.
 */
data class MethodRow(
    val body: Int,
    val name: String,
    val state: MethodState?,
    val copies: Int,
    val deopts: Int,
    val abandons: Int,
    val baselineBytes: Int,
    val twinBytes: Int,
    val optimizedBytes: Int,
    val declined: Int,
)

/**
 * A node of the Methods tab's tree: a class, which has no [row], a
 * method, or one of a method's bodies, shown by its [label].
 */
class MethodNode(val label: String, val row: MethodRow?) : DefaultMutableTreeNode() {
    override fun toString(): String = label
}

/** A column of the Methods tab: its title's [key], the order it sorts by, and the value it shows of a row. */
private class MethodColumn(val key: String, val comparator: Comparator<MethodRow>, val value: (MethodRow) -> Any)

/**
 * The Methods tab (`docs/design/developer-tool.md`, decision 29): every
 * method the run names -- described, or named by number alone, as the
 * events name it, `#<number>` -- a block counted as its method's, its
 * name with `[] in` left out, with its state, blank where the run knows
 * nothing of how it ran, how many optimized copies it has, how often it
 * deoptimized and was abandoned, the code of its largest install of each
 * tier, a column a tier, and the callees the optimizer declined in it;
 * sortable by every column, a double click opening the method. A part of
 * the name, in any case, filters the rows; grouped by method, a method
 * with blocks holds a row of each of its bodies, and grouped by class,
 * each class holds the rows of its methods, its factory and initializers
 * and its class side's among them.
 */
class MethodsTab(private val project: Project, private val run: TraceRun, parent: Disposable) : JPanel(BorderLayout()) {
    private val columns = listOf(
        MethodColumn("devtool.methods.column.name", compareBy(MethodRow::name)) { row -> row.name },
        MethodColumn("devtool.methods.column.state", compareBy(nullsFirst<MethodState>(), MethodRow::state)) { row -> row.state?.name?.lowercase().orEmpty() },
        MethodColumn("devtool.methods.column.copies", compareBy(MethodRow::copies)) { row -> row.copies },
        MethodColumn("devtool.methods.column.deopts", compareBy(MethodRow::deopts)) { row -> row.deopts },
        MethodColumn("devtool.methods.column.abandons", compareBy(MethodRow::abandons)) { row -> row.abandons },
        MethodColumn("devtool.methods.column.baseline", compareBy(MethodRow::baselineBytes)) { row -> row.baselineBytes },
        MethodColumn("devtool.methods.column.twin", compareBy(MethodRow::twinBytes)) { row -> row.twinBytes },
        MethodColumn("devtool.methods.column.optimized", compareBy(MethodRow::optimizedBytes)) { row -> row.optimizedBytes },
        MethodColumn("devtool.methods.column.declined", compareBy(MethodRow::declined)) { row -> row.declined },
    )

    /** The part a method's name must hold, in any case, for its row to show, in the table and the tree alike. */
    val nameFilter = nameFilterField(NewspeakBundle.message("devtool.methods.filter.name")) { refresh() }

    /** Whether the methods show in a tree, each with its bodies, or under their classes. */
    val grouping = groupingChooser { showView() }

    /** The rows that pass the filter, which every batch of the run replaces. */
    val model = ListTableModel<MethodRow>(
        columns.map { column ->
            object : ColumnInfo<MethodRow, Any>(NewspeakBundle.message(column.key)) {
                override fun valueOf(item: MethodRow): Any = column.value(item)

                override fun getComparator(): Comparator<MethodRow> = column.comparator
            }
        }.toTypedArray<ColumnInfo<MethodRow, *>>(),
        filteredRows(),
    )

    val table = TableView(model)

    /** The grouped rows: a node a method or a class, made afresh at every batch with its expansion and selection kept. */
    val treeRoot = MethodNode("", null)

    val treeModel = ListTreeTableModelOnColumns(
        treeRoot,
        (
            listOf<ColumnInfo<*, *>>(TreeColumnInfo(NewspeakBundle.message(columns.first().key))) +
                columns.drop(1).map { column ->
                    object : ColumnInfo<Any, Any>(NewspeakBundle.message(column.key)) {
                        override fun valueOf(item: Any): Any = (item as MethodNode).row?.let(column.value) ?: ""
                    }
                }
            ).toTypedArray(),
    )

    val treeTable = TreeTableView(treeModel).apply { setRootVisible(false) }

    /** The column the tree's rows are sorted by, and whether descending; the name's, ascending, until a header is clicked. */
    private var treeSort = 0 to false

    private val views = JPanel(CardLayout()).apply {
        add(JBScrollPane(table), TABLE)
        add(JBScrollPane(treeTable), TREE)
    }

    init {
        table.addMouseListener(
            object : MouseAdapter() {
                override fun mouseClicked(event: MouseEvent) {
                    if (event.clickCount == 2) table.selectedObject?.let(::open)
                }
            },
        )
        treeTable.addMouseListener(
            object : MouseAdapter() {
                override fun mouseClicked(event: MouseEvent) {
                    if (event.clickCount == 2) selectedNode()?.row?.let(::open)
                }
            },
        )
        treeTable.tableHeader.addMouseListener(
            object : MouseAdapter() {
                override fun mouseClicked(event: MouseEvent) {
                    val column = treeTable.columnAtPoint(event.point)
                    if (column < 0) return
                    val sorted = treeTable.convertColumnIndexToModel(column)
                    // A second click on the same column turns its order round.
                    treeSort = sorted to (treeSort.first == sorted && !treeSort.second)
                    rebuildTree()
                }
            },
        )
        val filters = JPanel(FlowLayout(FlowLayout.LEFT))
        filters.add(nameFilter)
        filters.add(JBLabel(NewspeakBundle.message("devtool.group")))
        filters.add(grouping)
        add(filters, BorderLayout.NORTH)
        add(views, BorderLayout.CENTER)
        run.onChanged(parent) { refresh() }
    }

    /** Selects the row of the method [body], if it has one, and scrolls to it, in the view shown. */
    fun select(body: Int) {
        if (grouping.selectedItem == Grouping.NONE) {
            table.selection = model.items.filter { row -> row.body == body }
            val index = table.selectedRow
            if (index >= 0) table.scrollRectToVisible(table.getCellRect(index, 0, true))
            return
        }
        val node = nodes().firstOrNull { node -> node.row?.body == body } ?: return
        selectNode(node)
    }

    /** Opens [row]'s method. */
    fun open(row: MethodRow) = navigate(project, run, row.body, null)

    private fun showView() {
        val grouped = grouping.selectedItem != Grouping.NONE
        if (grouped) rebuildTree()
        (views.layout as CardLayout).show(views, if (grouped) TREE else TABLE)
    }

    private fun refresh() {
        val selected = table.selectedObject?.body
        model.items = filteredRows()
        table.selection = model.items.filter { row -> row.body == selected }
        if (grouping.selectedItem != Grouping.NONE) rebuildTree()
    }

    private fun needle(): String = nameFilter.text.trim()

    private fun filteredRows(): List<MethodRow> {
        val needle = needle()
        return methodRows().filter { row -> nameMatches(row.name, needle) }
    }

    /**
     * Makes the tree afresh from the run's rows as they are, sorted by the
     * column clicked, keeping the groups that were expanded and the row
     * that was selected; a filter being typed expands every group, so
     * that what it finds shows.
     */
    private fun rebuildTree() {
        val expanded = treeRoot.children().toList().map { node -> node as MethodNode }
            .filter { node -> treeTable.tree.isExpanded(TreePath(node.path)) }
            .mapTo(HashSet()) { node -> node.label }
        val selected = selectedNode()
        val selectedBody = selected?.row?.body
        val selectedGroup = (selected?.parent as? MethodNode)?.takeIf { node -> node !== treeRoot }?.label
        treeRoot.removeAllChildren()
        val needle = needle()
        val order = columns[treeSort.first].comparator.let { comparator -> if (treeSort.second) comparator.reversed() else comparator }
        when (grouping.selectedItem) {
            Grouping.CLASS -> {
                val byClass = methodRows().filter { row -> nameMatches(row.name, needle) }.groupBy { row -> classOf(row.name) }
                val classOrder = if (treeSort.first == 0 && treeSort.second) byClass.keys.sortedDescending() else byClass.keys.sorted()
                for (className in classOrder) {
                    val group = MethodNode(className, null)
                    for (row in byClass.getValue(className).sortedWith(order)) group.add(MethodNode(row.name, row))
                    treeRoot.add(group)
                }
            }
            Grouping.METHOD -> {
                val bodies = bodyRows().groupBy { row -> rootOf(run, row.body) }
                for (method in methodRows().sortedWith(order)) {
                    val own = bodies[method.body].orEmpty()
                    val methodMatches = nameMatches(method.name, needle)
                    val shown = if (methodMatches) own else own.filter { row -> nameMatches(row.name, needle) }
                    if (!methodMatches && shown.isEmpty()) continue
                    val group = MethodNode(method.name, method)
                    // A method of one body is its body; only one with blocks has rows under it.
                    if (own.any { row -> row.body != method.body }) {
                        for (row in shown.sortedWith(order)) group.add(MethodNode(row.name, row))
                    }
                    treeRoot.add(group)
                }
            }
            else -> {}
        }
        treeModel.reload()
        showRootsChildren(treeTable.tree, treeRoot)
        for (node in treeRoot.children().toList().map { node -> node as MethodNode }) {
            if (needle.isNotEmpty() || node.label in expanded) treeTable.tree.expandPath(TreePath(node.path))
        }
        if (selectedBody != null) {
            val again = nodes().firstOrNull { node ->
                node.row?.body == selectedBody && ((node.parent as? MethodNode)?.takeIf { parent -> parent !== treeRoot }?.label == selectedGroup)
            }
            again?.let(::selectNode)
        }
    }

    /** Every node of the tree, each before its children, in the tree's order. */
    private fun nodes(): List<MethodNode> =
        treeRoot.preorderEnumeration().toList().map { node -> node as MethodNode }.filter { node -> node !== treeRoot }

    private fun selectedNode(): MethodNode? = treeTable.tree.lastSelectedPathComponent as? MethodNode

    private fun selectNode(node: MethodNode) {
        val path = TreePath(node.path)
        path.parentPath?.let { parentPath -> treeTable.tree.expandPath(parentPath) }
        treeTable.tree.selectionPath = path
        val index = treeTable.tree.getRowForPath(path)
        if (index >= 0) treeTable.scrollRectToVisible(treeTable.getCellRect(index, 0, true))
    }

    /** A row a method, a block counted as its method's. */
    private fun methodRows(): List<MethodRow> = rows { number -> rootOf(run, number) }

    /** A row a body, a block's apart from its method's. */
    private fun bodyRows(): List<MethodRow> = rows { number -> number }

    /** A row for each body [keyOf] maps the run's bodies to, counting every body it maps there. */
    private fun rows(keyOf: (Int) -> Int): List<MethodRow> {
        val deopts = HashMap<Int, Int>()
        val abandons = HashMap<Int, Int>()
        val declined = HashMap<Int, Int>()
        val largest = HashMap<Pair<Int, String>, Int>()
        fun install(key: Int, tier: String, bytes: Int) {
            largest.merge(key to tier, bytes, ::maxOf)
        }
        // A body the run names only by its number has a row of its own, as the events name it.
        val keys = run.bodies.keys.mapTo(HashSet(), keyOf)
        for ((number, tally) in run.tallies) {
            val key = keyOf(number)
            keys += key
            deopts.merge(key, tally.deoptimizations, Int::plus)
            abandons.merge(key, tally.abandonments, Int::plus)
            declined.merge(key, tally.declines, Int::plus)
            install(key, BASELINE, tally.largestBaseline)
            install(key, TWIN, tally.largestTwin)
            install(key, OPTIMIZED, tally.largestOptimized)
        }
        for ((number, codes) in run.code) {
            val key = keyOf(number)
            keys += key
            for (code in codes) install(key, code.tier, code.bytes)
        }
        for (number in run.bytecode.keys) keys += keyOf(number)
        for (number in run.inlining.keys) keys += keyOf(number)
        return keys.map { body ->
            MethodRow(
                body,
                bodyName(run, body),
                run.state(body),
                run.copies(body),
                deopts[body] ?: 0,
                abandons[body] ?: 0,
                largest[body to BASELINE] ?: 0,
                largest[body to TWIN] ?: 0,
                largest[body to OPTIMIZED] ?: 0,
                declined[body] ?: 0,
            )
        }.sortedBy { row -> row.body }
    }

    private companion object {
        const val BASELINE = "baseline"
        const val TWIN = "twin"
        const val OPTIMIZED = "optimized"
        const val TABLE = "table"
        const val TREE = "tree"
    }
}
