package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.options.Configurable
import com.intellij.openapi.ui.Messages
import com.intellij.ui.CollectionListModel
import com.intellij.ui.ToolbarDecorator
import com.intellij.ui.components.JBCheckBox
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBList
import com.intellij.util.ui.JBUI
import io.github.bstrauss.newspeak.NewspeakBundle
import java.awt.BorderLayout
import javax.swing.JComponent
import javax.swing.JPanel
import javax.swing.ListSelectionModel

/**
 * Newspeak's section of Settings | Build, Execution, Deployment | Debugger
 * | Stepping: whether a step goes into the kernel's and the platform's
 * code, the default of every session and the same switch as the debug
 * tool window's toggle, and the classes not to step into, as Java's
 * section has them ([NewspeakDebuggerSettings]).
 */
class NewspeakSteppingConfigurable : Configurable {
    /** The switch, and the list of classes; for the tests too. */
    internal val stepIntoShipped = JBCheckBox(NewspeakBundle.message("debugger.settings.step.into.shipped"))
    internal val skippedClasses = CollectionListModel<String>()

    override fun getDisplayName(): String = NewspeakBundle.message("debugger.settings.display.name")

    override fun createComponent(): JComponent {
        val list = JBList(skippedClasses).apply { selectionMode = ListSelectionModel.SINGLE_SELECTION }
        val decorated = ToolbarDecorator.createDecorator(list)
            .setAddAction { _ -> askForClass(list, "")?.let(skippedClasses::add) }
            .setEditAction { _ ->
                val index = list.selectedIndex
                if (index >= 0) askForClass(list, skippedClasses.getElementAt(index))?.let { name -> skippedClasses.setElementAt(name, index) }
            }
            .setRemoveAction { _ -> list.selectedIndex.takeIf { index -> index >= 0 }?.let(skippedClasses::remove) }
            .disableUpDownActions()
            .createPanel()
        val classes = JPanel(BorderLayout(0, JBUI.scale(GAP))).apply {
            add(JBLabel(NewspeakBundle.message("debugger.settings.skipped.classes")), BorderLayout.NORTH)
            add(decorated, BorderLayout.CENTER)
        }
        val created = JPanel(BorderLayout(0, JBUI.scale(GAP))).apply {
            add(stepIntoShipped, BorderLayout.NORTH)
            add(classes, BorderLayout.CENTER)
        }
        reset()
        return created
    }

    /** Asks for a class's name in the agent's form, checked as an exception breakpoint's is; null when cancelled. */
    private fun askForClass(parent: JComponent, initial: String): String? = Messages.showInputDialog(
        parent,
        NewspeakBundle.message("debugger.settings.skipped.class.dialog.message"),
        NewspeakBundle.message("debugger.settings.skipped.class.dialog.title"),
        null,
        initial,
        NewspeakExceptionBreakpointType.ClassNameValidator,
    )?.trim()

    /** The filters the controls show. */
    private fun edited(): StepFilters = StepFilters(stepIntoShipped.isSelected, skippedClasses.items.toList())

    override fun isModified(): Boolean = edited() != NewspeakDebuggerSettings.getInstance().filters

    override fun apply() {
        NewspeakDebuggerSettings.getInstance().update(edited())
    }

    override fun reset() {
        val filters = NewspeakDebuggerSettings.getInstance().filters
        stepIntoShipped.isSelected = filters.stepIntoShipped
        skippedClasses.replaceAll(filters.skippedClasses)
    }

    private companion object {
        const val GAP = 4
    }
}
