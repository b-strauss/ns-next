package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.Disposable
import com.intellij.openapi.project.Project
import com.intellij.ui.components.JBScrollPane
import com.intellij.ui.table.TableView
import com.intellij.util.ui.ColumnInfo
import com.intellij.util.ui.ListTableModel
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.model.MethodState
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.awt.BorderLayout
import java.awt.event.MouseAdapter
import java.awt.event.MouseEvent
import javax.swing.JPanel

/** A method of the Methods tab: a root body and what its events say of it. */
data class MethodRow(
    val body: Int,
    val name: String,
    val state: MethodState,
    val copies: Int,
    val deopts: Int,
    val abandons: Int,
    val codeBytes: Int,
    val declined: Int,
)

/**
 * The Methods tab (`docs/design/developer-tool.md`, decision 29): every
 * method the run described -- a block is its method's, so its events count under its root and its name with
 * `[] in` is left out -- with its state, how many optimized copies it has,
 * how often it deoptimized and was abandoned, its largest copy's code and the
 * callees the optimizer declined in it; sortable by every column, a double
 * click opening the method.
 */
class MethodsTab(private val project: Project, private val run: TraceRun, parent: Disposable) : JPanel(BorderLayout()) {
    private fun column(key: String, comparator: Comparator<MethodRow>, value: (MethodRow) -> Any) =
        object : ColumnInfo<MethodRow, Any>(NewspeakBundle.message(key)) {
            override fun valueOf(item: MethodRow): Any = value(item)

            override fun getComparator(): Comparator<MethodRow> = comparator
        }

    /** The rows, which every batch of the run replaces. */
    val model = ListTableModel<MethodRow>(
        arrayOf<ColumnInfo<MethodRow, *>>(
            column("devtool.methods.column.name", compareBy(MethodRow::name)) { row -> row.name },
            column("devtool.methods.column.state", compareBy(MethodRow::state)) { row -> row.state.name.lowercase() },
            column("devtool.methods.column.copies", compareBy(MethodRow::copies)) { row -> row.copies },
            column("devtool.methods.column.deopts", compareBy(MethodRow::deopts)) { row -> row.deopts },
            column("devtool.methods.column.abandons", compareBy(MethodRow::abandons)) { row -> row.abandons },
            column("devtool.methods.column.bytes", compareBy(MethodRow::codeBytes)) { row -> row.codeBytes },
            column("devtool.methods.column.declined", compareBy(MethodRow::declined)) { row -> row.declined },
        ),
        rows(),
    )

    val table = TableView(model)

    init {
        table.addMouseListener(
            object : MouseAdapter() {
                override fun mouseClicked(event: MouseEvent) {
                    if (event.clickCount == 2) table.selectedObject?.let(::open)
                }
            },
        )
        add(JBScrollPane(table), BorderLayout.CENTER)
        run.onChanged(parent) { refresh() }
    }

    /** Opens [row]'s method. */
    fun open(row: MethodRow) = navigate(project, run, row.body, null)

    private fun refresh() {
        val selected = table.selectedObject?.body
        model.items = rows()
        table.selection = model.items.filter { row -> row.body == selected }
    }

    private fun rows(): List<MethodRow> {
        val deopts = HashMap<Int, Int>()
        val abandons = HashMap<Int, Int>()
        val declined = HashMap<Int, Int>()
        val installed = HashMap<Int, Int>()
        for (event in run.events) {
            val body = event.body?.let { number -> rootOf(run, number) } ?: continue
            if (event.category == "deoptimize") deopts.merge(body, 1, Int::plus)
            if (event.event == "abandoned") abandons.merge(body, 1, Int::plus)
            if (event.event == "declined") declined.merge(body, 1, Int::plus)
            if (event.event == "optimized") event.integer("bytes")?.let { bytes -> installed.merge(body, bytes, ::maxOf) }
        }
        for ((number, codes) in run.code) {
            val root = rootOf(run, number)
            installed.merge(root, codes.filter { code -> code.tier == "optimized" }.maxOfOrNull { code -> code.bytes } ?: 0, ::maxOf)
        }
        return run.bodies.values.filter { body -> rootOf(run, body.number) == body.number }.map { body ->
            MethodRow(
                body.number,
                body.name,
                run.state(body.number),
                run.copies(body.number),
                deopts[body.number] ?: 0,
                abandons[body.number] ?: 0,
                installed[body.number] ?: 0,
                declined[body.number] ?: 0,
            )
        }.sortedBy { row -> row.body }
    }
}
