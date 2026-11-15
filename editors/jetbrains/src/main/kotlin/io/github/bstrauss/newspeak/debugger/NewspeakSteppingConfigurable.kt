package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.options.Configurable
import com.intellij.ui.components.JBCheckBox
import io.github.bstrauss.newspeak.NewspeakBundle
import java.awt.BorderLayout
import javax.swing.JComponent
import javax.swing.JPanel

/**
 * Newspeak's section of Settings | Build, Execution, Deployment | Debugger
 * | Stepping: whether a step goes into the kernel's and the platform's
 * code, the default of every session and the same switch as the debug
 * tool window's toggle ([NewspeakDebuggerSettings]). The classes not to
 * step into are a project's, on a page of their own
 * ([NewspeakStepFiltersConfigurable]), since this section has no project.
 */
class NewspeakSteppingConfigurable : Configurable {
    /** The switch; for the tests too. */
    internal val stepIntoShipped = JBCheckBox(NewspeakBundle.message("debugger.settings.step.into.shipped"))

    override fun getDisplayName(): String = NewspeakBundle.message("debugger.settings.display.name")

    override fun createComponent(): JComponent {
        val created = JPanel(BorderLayout()).apply { add(stepIntoShipped, BorderLayout.NORTH) }
        reset()
        return created
    }

    override fun isModified(): Boolean = stepIntoShipped.isSelected != NewspeakDebuggerSettings.getInstance().stepIntoShipped

    override fun apply() {
        NewspeakDebuggerSettings.getInstance().update(stepIntoShipped.isSelected)
    }

    override fun reset() {
        stepIntoShipped.isSelected = NewspeakDebuggerSettings.getInstance().stepIntoShipped
    }
}
