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

/**
 * A method of the Methods tab: a root body and what its events say of it,
 * its [state] null where the run knows nothing of how it ran, and the
 * bytes of its largest install of each tier -- [baselineBytes],
 * [twinBytes], [optimizedBytes] -- 0 for a tier never installed.
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
 * The Methods tab (`docs/design/developer-tool.md`, decision 29): every
 * method the run names -- described, or named by number alone, as the
 * events name it, `#<number>` -- a block counted as its method's, its
 * name with `[] in` left out, with its state, blank where the run knows
 * nothing of how it ran, how many optimized copies it has, how often it
 * deoptimized and was abandoned, the code of its largest install of each
 * tier, a column a tier, and the callees the optimizer declined in it;
 * sortable by every column, a double click opening the method.
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
            column("devtool.methods.column.state", compareBy(nullsFirst<MethodState>(), MethodRow::state)) { row -> row.state?.name?.lowercase().orEmpty() },
            column("devtool.methods.column.copies", compareBy(MethodRow::copies)) { row -> row.copies },
            column("devtool.methods.column.deopts", compareBy(MethodRow::deopts)) { row -> row.deopts },
            column("devtool.methods.column.abandons", compareBy(MethodRow::abandons)) { row -> row.abandons },
            column("devtool.methods.column.baseline", compareBy(MethodRow::baselineBytes)) { row -> row.baselineBytes },
            column("devtool.methods.column.twin", compareBy(MethodRow::twinBytes)) { row -> row.twinBytes },
            column("devtool.methods.column.optimized", compareBy(MethodRow::optimizedBytes)) { row -> row.optimizedBytes },
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

    /** Selects the row of the method [body], if it has one, and scrolls to it. */
    fun select(body: Int) {
        table.selection = model.items.filter { row -> row.body == body }
        val index = table.selectedRow
        if (index >= 0) table.scrollRectToVisible(table.getCellRect(index, 0, true))
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
        val largest = HashMap<Pair<Int, String>, Int>()
        fun install(root: Int, tier: String, bytes: Int) {
            largest.merge(root to tier, bytes, ::maxOf)
        }
        // A body the run names only by its number has a row of its own, as the events name it.
        val methods = run.bodies.keys.filterTo(HashSet()) { number -> rootOf(run, number) == number }
        for ((number, tally) in run.tallies) {
            val body = rootOf(run, number)
            methods += body
            deopts.merge(body, tally.deoptimizations, Int::plus)
            abandons.merge(body, tally.abandonments, Int::plus)
            declined.merge(body, tally.declines, Int::plus)
            install(body, BASELINE, tally.largestBaseline)
            install(body, TWIN, tally.largestTwin)
            install(body, OPTIMIZED, tally.largestOptimized)
        }
        for ((number, codes) in run.code) {
            val root = rootOf(run, number)
            methods += root
            for (code in codes) install(root, code.tier, code.bytes)
        }
        for (number in run.bytecode.keys) methods += rootOf(run, number)
        for (number in run.inlining.keys) methods += rootOf(run, number)
        return methods.map { body ->
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
    }
}
