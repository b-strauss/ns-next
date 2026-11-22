package io.github.bstrauss.newspeak.devtool.ui

import com.google.gson.JsonArray
import com.google.gson.JsonElement
import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.popup.JBPopupFactory
import com.intellij.ui.SearchTextField
import com.intellij.ui.components.JBCheckBox
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBScrollPane
import com.intellij.ui.components.JBTextArea
import com.intellij.ui.table.TableView
import com.intellij.ui.treeStructure.Tree
import com.intellij.util.ui.ColumnInfo
import com.intellij.util.ui.JBUI
import com.intellij.util.ui.ListTableModel
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.TRACE_CATEGORIES
import io.github.bstrauss.newspeak.devtool.model.Event
import io.github.bstrauss.newspeak.devtool.model.KEPT_EVENTS
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.awt.BorderLayout
import java.awt.CardLayout
import java.awt.FlowLayout
import java.awt.GridLayout
import java.awt.event.MouseAdapter
import java.awt.event.MouseEvent
import javax.swing.JButton
import javax.swing.JPanel
import javax.swing.SwingConstants
import javax.swing.tree.DefaultMutableTreeNode
import javax.swing.tree.DefaultTreeModel

/** The fields whose value is a body's number, which the text trace writes as its name. */
private val BODY_FIELDS = setOf("in-body", "callee", "spent-by")

/** The fields whose value is a class or application name, which the text trace writes in braces. */
private val NAME_FIELDS = setOf("key", "entered", "receiver")

/**
 * The order of the text trace's fields (`crates/ns-cli/src/trace.rs`, `line`).
 * The file's objects have no order, and no two events of the text trace
 * order two fields differently but for `at`, which an `osr` event writes
 * before its reason and the others after.
 */
private val FIELD_ORDER = listOf(
    "key", "entered", "trigger", "spent-by", "walk-end", "in-body", "callee", "count", "reasons",
    "bytes", "inlined-scopes", "inlined-sites", "type-case-arms", "eliminated-blocks", "tier", "cause",
    "reason", "version", "freeing", "bodies-freed", "exits", "by", "selector", "site", "receiver", "at",
)

/** An event's word as the text trace writes it: a cache transition's is the transition itself, `<before>-><after>`. */
fun eventWord(event: Event): String =
    if (event.event == "transition") "${event.text("before")}->${event.text("after")}" else event.event

/** [event]'s fields as the text trace writes them: ` name=value` each, naming a body by its name in braces. */
fun eventFields(run: TraceRun, event: Event): String {
    val fields = event.fields
    val names = fields.keySet().filterNot { name -> event.event == "transition" && (name == "before" || name == "after") }
    val known = if (event.category == "osr") listOf("at") + FIELD_ORDER.filter { name -> name != "at" } else FIELD_ORDER
    val ordered = known.filter { name -> name in names } + names.filter { name -> name !in known }.sorted()
    val text = StringBuilder()
    for (name in ordered) {
        val value = fields.get(name)
        if (value == null || value.isJsonNull) {
            // A retirement says it has no version; a key or entered application that is absent is left out.
            if (name == "version") text.append(" version=none")
            continue
        }
        text.append(' ').append(name).append('=').append(fieldValue(run, name, value, event))
    }
    return text.toString()
}

private fun fieldValue(run: TraceRun, name: String, value: JsonElement, event: Event): String = when {
    name in BODY_FIELDS -> "{${bodyName(run, value.asInt)}}"
    name in NAME_FIELDS -> "{${value.asString}}"
    name == "selector" && event.event == "transition" -> "#${value.asString}"
    name == "reasons" && value.isJsonArray -> reasonsText(value.asJsonArray)
    else -> value.asString
}

/** Each reason with its count, `<reason>:<count>`, joined by commas: one value, which no space splits. */
private fun reasonsText(reasons: JsonArray): String =
    reasons.joinToString(",") { element -> "${element.asJsonObject.get("reason").asString}:${element.asJsonObject.get("count").asInt}" }

/** [event] as the line `ns-next run --trace` writes: `[<category>] {<body>} <event> name=value…`. */
fun traceLine(run: TraceRun, event: Event): String =
    "[${event.category}] {${event.body?.let { body -> bodyName(run, body) }.orEmpty()}} ${eventWord(event)}${eventFields(run, event)}"

/** An event of the table: its place in the file's order, which is the time's, and the cells it shows. */
data class EventRow(val order: Long, val event: Event, val body: String, val fields: String) {
    /** What the grouped view shows: the event and its fields, the method being the group. */
    override fun toString(): String = "[${event.category}] ${eventWord(event)}$fields"
}

/**
 * The Events tab (`docs/design/developer-tool.md`, decision 29): the run's
 * events in time order, filtered by category and by a part of the body's
 * name in any case, as a table, grouped under their methods or under their
 * classes and then their methods, or as the text trace's own lines. A double click opens the source an event with a position names.
 * It shows the events the run keeps, the last [KEPT_EVENTS], and says how
 * many earlier ones were dropped.
 */
class EventsTab(private val project: Project, private val run: TraceRun, parent: Disposable) : JPanel(BorderLayout()) {
    /**
     * One box per category, by its name, all selected: an event of a
     * category unchecked is hidden. The boxes are shown in the popup
     * [categoriesButton] opens.
     */
    val categoryBoxes: Map<String, JBCheckBox> = TRACE_CATEGORIES.associateWith { name ->
        JBCheckBox(NewspeakBundle.message("devtool.category.$name"), true).apply {
            toolTipText = NewspeakBundle.message("devtool.category.$name.tooltip")
            addItemListener {
                nameCategories()
                refilter()
            }
        }
    }

    /** Opens the category boxes in a popup below it, which stays open while boxes are clicked; it says how many are shown. */
    val categoriesButton = JButton(AllIcons.General.ArrowDown).apply {
        horizontalTextPosition = SwingConstants.LEADING
        addActionListener { showCategories() }
    }

    /** The part a body's name must hold, in any case, for its events to show, in the table, the tree and the text alike. */
    val bodyFilter: SearchTextField = nameFilterField(NewspeakBundle.message("devtool.events.filter.body")) { refilter() }

    /** Whether the events show in a tree, under their methods, or under their classes and then their methods. */
    val grouping = groupingChooser { regroup() }

    /** Whether the events show as the text trace's lines. */
    val textView = JBCheckBox(NewspeakBundle.message("devtool.events.text")).apply { addItemListener { showView() } }

    private fun column(key: String, value: (EventRow) -> Any) =
        object : ColumnInfo<EventRow, Any>(NewspeakBundle.message(key)) {
            override fun valueOf(item: EventRow): Any = value(item)
        }

    /** The model's rows, which it holds as they are, so that the oldest can be taken off in place. */
    private var rowItems = ArrayList<EventRow>()

    /** The events that pass the filters, a row each, in time order. */
    val model = ListTableModel<EventRow>(
        arrayOf<ColumnInfo<EventRow, *>>(
            column("devtool.events.column.time") { row -> row.order },
            column("devtool.events.column.category") { row -> row.event.category },
            column("devtool.events.column.body") { row -> row.body },
            column("devtool.events.column.event") { row -> eventWord(row.event) },
            column("devtool.events.column.fields") { row -> row.fields.trim() },
        ),
        rowItems,
    )

    val table = TableView(model)

    /**
     * The events under their methods, a node a method, or under their
     * classes, a node a class holding a node a method, each node in the
     * order of its first event.
     */
    private val treeRoot = DefaultMutableTreeNode()
    val treeModel = DefaultTreeModel(treeRoot)
    val tree = Tree(treeModel).apply { isRootVisible = false }

    /** Says how many of the run's earliest events it no longer keeps; hidden while it keeps every one. */
    val droppedLabel = JBLabel().apply {
        border = JBUI.Borders.empty(0, 8, 4, 8)
        isVisible = false
    }

    /** The run's events up to which the rows were made, counted from the run's first. */
    private var taken = 0L
    private var textFed = 0
    private var treeFed = 0
    /** The tree's node of each method, by its name, under the root or under its class's node. */
    private val groups = HashMap<String, DefaultMutableTreeNode>()

    /** The tree's node of each class, by its name, while the events are grouped by class. */
    private val classGroups = HashMap<String, DefaultMutableTreeNode>()

    /** The text view's lines. */
    val text = JBTextArea().apply { isEditable = false }

    private val views = JPanel(CardLayout()).apply {
        add(JBScrollPane(table), TABLE)
        add(JBScrollPane(tree), TREE)
        add(JBScrollPane(text), TEXT)
    }

    init {
        val filters = JPanel(FlowLayout(FlowLayout.LEFT))
        filters.add(bodyFilter)
        filters.add(JBLabel(NewspeakBundle.message("devtool.group")))
        filters.add(grouping)
        filters.add(textView)
        filters.add(categoriesButton)
        nameCategories()
        val head = JPanel(BorderLayout())
        head.add(filters, BorderLayout.NORTH)
        head.add(droppedLabel, BorderLayout.SOUTH)
        add(head, BorderLayout.NORTH)
        add(views, BorderLayout.CENTER)
        table.addMouseListener(
            object : MouseAdapter() {
                override fun mouseClicked(event: MouseEvent) {
                    if (event.clickCount == 2) table.selectedObject?.let { row -> open(row.event) }
                }
            },
        )
        tree.addMouseListener(
            object : MouseAdapter() {
                override fun mouseClicked(event: MouseEvent) {
                    if (event.clickCount != 2) return
                    val row = (tree.lastSelectedPathComponent as? DefaultMutableTreeNode)?.userObject as? EventRow
                    row?.let { chosen -> open(chosen.event) }
                }
            },
        )
        takeNew()
        run.onChanged(parent) { takeNew() }
    }

    /** Names on [categoriesButton] how many categories are shown: all, or so many of all. */
    private fun nameCategories() {
        val shown = categoryBoxes.values.count { box -> box.isSelected }
        categoriesButton.text = if (shown == categoryBoxes.size) {
            NewspeakBundle.message("devtool.events.categories.all")
        } else {
            NewspeakBundle.message("devtool.events.categories.some", shown, categoryBoxes.size)
        }
    }

    /** Shows the category boxes below [categoriesButton], in a popup that closes as the focus leaves it. */
    private fun showCategories() {
        val boxes = JPanel(GridLayout(0, 1)).apply { border = JBUI.Borders.empty(4, 8) }
        // A box has one parent: each popup takes them from the last.
        for (box in categoryBoxes.values) boxes.add(box)
        JBPopupFactory.getInstance()
            .createComponentPopupBuilder(boxes, categoryBoxes.values.first())
            .setRequestFocus(true)
            .createPopup()
            .showUnderneathOf(categoriesButton)
    }

    /** The rows that pass the filters. */
    val visibleRows: List<EventRow> get() = model.items

    /** Opens the source [event] names: its method at its `at`, in the scope an inlining names. */
    fun open(event: Event) {
        val at = event.integer("at") ?: return
        val body = event.integer("in-body") ?: event.body ?: return
        navigate(project, run, body, at)
    }

    private fun showView() {
        feedViews()
        (views.layout as CardLayout).show(
            views,
            when {
                textView.isSelected -> TEXT
                grouping.selectedItem != Grouping.NONE -> TREE
                else -> TABLE
            },
        )
    }

    /** Every filter changed: the rows are made afresh, and the views that are not shown are made when they are. */
    private fun refilter() {
        taken = 0L
        textFed = 0
        treeFed = 0
        rowItems = ArrayList()
        model.items = rowItems
        text.text = ""
        clearTree()
        takeNew()
    }

    /** Another grouping was chosen: the tree is made afresh from the rows, which stay. */
    private fun regroup() {
        treeFed = 0
        clearTree()
        showView()
    }

    private fun clearTree() {
        treeRoot.removeAllChildren()
        groups.clear()
        classGroups.clear()
        treeModel.reload()
    }

    /** A batch arrived: the events after the last taken are appended, which leaves the table's selection and scroll and the tree's expansion where they are. */
    private fun takeNew() {
        val shown = categoryBoxes.filterValues { box -> box.isSelected }.keys
        val needle = bodyFilter.text.trim()
        val events = run.events
        val first = run.droppedEvents
        dropRowsBefore(first)
        val added = ArrayList<EventRow>()
        for (order in maxOf(taken, first) until run.eventCount) {
            val event = events[(order - first).toInt()]
            if (event.category !in shown) continue
            if (needle.isNotEmpty() && event.body?.let { body -> nameMatches(bodyName(run, body), needle) } != true) continue
            added.add(EventRow(order, event, event.body?.let { body -> bodyName(run, body) }.orEmpty(), eventFields(run, event)))
        }
        taken = run.eventCount
        if (added.isNotEmpty()) model.addRows(added)
        if (first > 0) {
            droppedLabel.text = NewspeakBundle.message("devtool.events.dropped", first, KEPT_EVENTS)
            droppedLabel.isVisible = true
        }
        feedViews()
    }

    /**
     * Takes away the rows of the events the run dropped, which are the
     * oldest, from the table, the text and the tree alike, leaving the
     * table's selection and scroll on the rows that stay.
     */
    private fun dropRowsBefore(first: Long) {
        val rows = rowItems
        val dropped = rows.indexOfFirst { row -> row.order >= first }.let { index -> if (index < 0) rows.size else index }
        if (dropped == 0) return
        val gone = ArrayList(rows.subList(0, dropped))
        rows.subList(0, dropped).clear()
        model.fireTableRowsDeleted(0, dropped - 1)
        val textDropped = minOf(dropped, textFed)
        if (textDropped > 0) {
            // A line a row, with no newline after the last.
            val end = if (textDropped < textFed) text.getLineStartOffset(textDropped) else text.document.length
            text.replaceRange("", 0, end)
            textFed -= textDropped
        }
        val treeDropped = minOf(dropped, treeFed)
        for (row in gone.subList(0, treeDropped)) {
            // A group's children are its rows in order, so a dropped row is its group's first.
            val name = groupName(row)
            val group = groups[name] ?: continue
            if (group.childCount > 0) treeModel.removeNodeFromParent(group.getChildAt(0) as DefaultMutableTreeNode)
            if (group.childCount == 0) {
                val parent = group.parent as DefaultMutableTreeNode
                treeModel.removeNodeFromParent(group)
                groups.remove(name)
                // A class's node goes with its last method's.
                if (parent !== treeRoot && parent.childCount == 0) {
                    treeModel.removeNodeFromParent(parent)
                    classGroups.remove(parent.userObject as String)
                }
            }
        }
        treeFed -= treeDropped
    }

    private fun groupName(row: EventRow): String = row.event.body?.let { body -> bodyName(run, rootOf(run, body)) }.orEmpty()

    /** Brings the shown view of the text or the tree up to the rows, which the other is brought to when it is shown. */
    private fun feedViews() {
        val rows = model.items
        if (textView.isSelected) {
            for (index in textFed until rows.size) {
                if (index > 0) text.append("\n")
                text.append(traceLine(run, rows[index].event))
            }
            textFed = rows.size
        } else if (grouping.selectedItem != Grouping.NONE) {
            val byClass = grouping.selectedItem == Grouping.CLASS
            for (index in treeFed until rows.size) {
                val row = rows[index]
                val name = groupName(row)
                val group = groups.getOrPut(name) {
                    val parent = if (byClass) {
                        val className = classOf(name)
                        classGroups.getOrPut(className) {
                            DefaultMutableTreeNode(className).also { node -> treeModel.insertNodeInto(node, treeRoot, treeRoot.childCount) }
                        }
                    } else {
                        treeRoot
                    }
                    DefaultMutableTreeNode(name).also { node -> treeModel.insertNodeInto(node, parent, parent.childCount) }
                }
                treeModel.insertNodeInto(DefaultMutableTreeNode(row), group, group.childCount)
            }
            treeFed = rows.size
            showRootsChildren(tree, treeRoot)
        }
    }


    private companion object {
        const val TABLE = "table"
        const val TREE = "tree"
        const val TEXT = "text"
    }
}
