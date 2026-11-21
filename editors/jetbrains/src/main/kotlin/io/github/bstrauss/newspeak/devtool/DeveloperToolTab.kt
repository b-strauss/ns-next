package io.github.bstrauss.newspeak.devtool

import com.intellij.openapi.options.SettingsEditor
import com.intellij.ui.components.JBCheckBox
import com.intellij.ui.dsl.builder.panel
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.run.NewspeakCommand
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import javax.swing.JComponent

/** The listing kinds `ns-next run` prints, by the name its option has after `--print-`. */
val LISTING_KINDS: List<String> = listOf("bytecode", "code", "inlining")

/**
 * The "Developer Tool" tab of a Newspeak run configuration: which trace
 * categories and which listings a run asks ns-next for. `bench` rejects the
 * options, so the tab is greyed while the configuration's command is it.
 */
class DeveloperToolTab : SettingsEditor<NewspeakRunConfiguration>() {
    /** One box per trace category, by its name; each has its sentence as tooltip. */
    val categoryBoxes: Map<String, JBCheckBox> = TRACE_CATEGORIES.associateWith { name ->
        JBCheckBox(NewspeakBundle.message("devtool.category.$name")).apply {
            toolTipText = NewspeakBundle.message("devtool.category.$name.tooltip")
        }
    }

    /** One box per listing kind, by its name. */
    val listingBoxes: Map<String, JBCheckBox> = LISTING_KINDS.associateWith { kind ->
        JBCheckBox(NewspeakBundle.message("devtool.listing.$kind")).apply {
            toolTipText = NewspeakBundle.message("devtool.listing.$kind.tooltip")
        }
    }

    /** Checks or clears every category at once, and shows whether all are checked. */
    val allBox: JBCheckBox = JBCheckBox(NewspeakBundle.message("devtool.category.all")).apply {
        toolTipText = NewspeakBundle.message("devtool.category.all.tooltip")
    }

    private var updatingAll = false

    init {
        allBox.addActionListener {
            updatingAll = true
            categoryBoxes.values.forEach { box -> box.isSelected = allBox.isSelected }
            updatingAll = false
        }
        categoryBoxes.values.forEach { box ->
            box.addActionListener { if (!updatingAll) allBox.isSelected = categoryBoxes.values.all(JBCheckBox::isSelected) }
        }
    }

    private val form = panel {
        group(NewspeakBundle.message("devtool.group.events")) {
            row { cell(allBox) }
            categoryBoxes.values.forEach { box -> row { cell(box) } }
        }
        group(NewspeakBundle.message("devtool.group.listings")) {
            listingBoxes.values.forEach { box -> row { cell(box) } }
        }
        row {
            // Task 5's dialog lives behind this; until then there is nothing to open.
            link(NewspeakBundle.message("devtool.watched.methods.link")) {}.enabled(false)
        }
    }

    private fun allBoxes(): List<JBCheckBox> = listOf(allBox) + categoryBoxes.values + listingBoxes.values

    override fun createEditor(): JComponent = form

    override fun resetEditorFrom(configuration: NewspeakRunConfiguration) {
        val options = configuration.options
        categoryBoxes.forEach { (name, box) -> box.isSelected = name in options.traceCategories }
        allBox.isSelected = categoryBoxes.values.all(JBCheckBox::isSelected)
        listingBoxes.getValue("bytecode").isSelected = options.printBytecode
        listingBoxes.getValue("code").isSelected = options.printCode
        listingBoxes.getValue("inlining").isSelected = options.printInlining
        val enabled = configuration.command != NewspeakCommand.BENCH
        allBoxes().forEach { box -> box.isEnabled = enabled }
    }

    override fun applyEditorTo(configuration: NewspeakRunConfiguration) {
        val options = configuration.options
        options.traceCategories = categoryBoxes.filterValues(JBCheckBox::isSelected).keys.toMutableList()
        options.printBytecode = listingBoxes.getValue("bytecode").isSelected
        options.printCode = listingBoxes.getValue("code").isSelected
        options.printInlining = listingBoxes.getValue("inlining").isSelected
    }
}
