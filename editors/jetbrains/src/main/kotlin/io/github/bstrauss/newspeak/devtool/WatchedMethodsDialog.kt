package io.github.bstrauss.newspeak.devtool

import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.DialogWrapper
import com.intellij.openapi.ui.ValidationInfo
import com.intellij.ui.TableUtil
import com.intellij.ui.ToolbarDecorator
import com.intellij.ui.table.TableView
import com.intellij.util.ui.ColumnInfo
import com.intellij.util.ui.ListTableModel
import io.github.bstrauss.newspeak.NewspeakBundle
import javax.swing.JComponent
import javax.swing.JTable

/**
 * The project's watched methods as a table: each entry's pattern, in the
 * tools' syntax, and its two switches, with entries added and removed.
 * Edits a copy, which OK puts in place whole.
 */
class WatchedMethodsDialog(private val project: Project) : DialogWrapper(project) {
    /** The rows, copies of the entries; the table edits them in place. */
    private val model = ListTableModel<WatchedMethods.Entry>(
        arrayOf<ColumnInfo<WatchedMethods.Entry, *>>(
            PatternColumn(),
            SwitchColumn("devtool.watched.methods.column.events", WatchedMethods.Entry::events) { entry, on -> entry.events = on },
            SwitchColumn("devtool.watched.methods.column.listings", WatchedMethods.Entry::listings) { entry, on -> entry.listings = on },
        ),
        WatchedMethods.getInstance(project).entries.map { entry -> entry.copy() },
    )

    private val table = TableView(model)

    init {
        title = NewspeakBundle.message("devtool.watched.methods.title")
        init()
    }

    override fun createCenterPanel(): JComponent = ToolbarDecorator.createDecorator(table)
        .setAddAction {
            model.addRow(WatchedMethods.Entry())
            val row = model.rowCount - 1
            table.selection = listOf(model.getItem(row))
            table.editCellAt(row, 0)
        }
        .setRemoveAction { TableUtil.removeSelectedItems(table) }
        .createPanel()

    override fun getPreferredFocusedComponent(): JComponent = table

    /** A pattern `ns-next run` would refuse as not of the form, which would end every traced run with status 2. */
    override fun doValidate(): ValidationInfo? {
        // A pattern still being typed counts.
        TableUtil.stopEditing(table)
        val malformed = firstMalformedPattern(model.items) ?: return null
        return ValidationInfo(NewspeakBundle.message("devtool.watched.methods.malformed", malformed), table)
    }

    override fun doOKAction() {
        TableUtil.stopEditing(table)
        WatchedMethods.getInstance(project).replaceEntries(model.items)
        super.doOKAction()
    }

    private class PatternColumn : ColumnInfo<WatchedMethods.Entry, String>(NewspeakBundle.message("devtool.watched.methods.column.pattern")) {
        override fun valueOf(item: WatchedMethods.Entry): String = item.pattern

        override fun isCellEditable(item: WatchedMethods.Entry): Boolean = true

        override fun setValue(item: WatchedMethods.Entry, value: String) {
            item.pattern = value
        }
    }

    private class SwitchColumn(
        key: String,
        private val read: (WatchedMethods.Entry) -> Boolean,
        private val write: (WatchedMethods.Entry, Boolean) -> Unit,
    ) : ColumnInfo<WatchedMethods.Entry, Boolean>(NewspeakBundle.message(key)) {
        override fun valueOf(item: WatchedMethods.Entry): Boolean = read(item)

        // The table's own check box renders and edits a Boolean column.
        override fun getColumnClass(): Class<*> = Boolean::class.javaObjectType

        override fun isCellEditable(item: WatchedMethods.Entry): Boolean = true

        override fun setValue(item: WatchedMethods.Entry, value: Boolean) {
            write(item, value)
        }

        override fun getWidth(table: JTable): Int = table.getFontMetrics(table.font).stringWidth(name) + SWITCH_COLUMN_PADDING
    }

    private companion object {
        /** Room around a switch column's title, so the title is never cut. */
        const val SWITCH_COLUMN_PADDING = 24
    }
}

/** The first pattern of [entries] not of the trace filter's form, an empty one left out as OK leaves it out; null when there is none. */
fun firstMalformedPattern(entries: List<WatchedMethods.Entry>): String? =
    entries.map { entry -> entry.pattern.trim() }.firstOrNull { pattern -> pattern.isNotEmpty() && patternModule(pattern) == null }
