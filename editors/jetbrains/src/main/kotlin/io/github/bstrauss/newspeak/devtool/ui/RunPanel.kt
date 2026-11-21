package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.icons.AllIcons
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBTabbedPane
import com.intellij.util.ui.JBUI
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.awt.BorderLayout
import javax.swing.JPanel

/**
 * One traced run's tab of the tool window: its four views -- Methods,
 * Events, Code and Inlining (`docs/design/developer-tool.md`, decision 29)
 * -- over [run], and above them the error that ended the reading of its
 * file, if one did.
 */
class RunPanel(val run: TraceRun) : JPanel(BorderLayout()) {
    private val errorLabel = JBLabel(AllIcons.General.Error).apply {
        border = JBUI.Borders.empty(4, 8)
        isVisible = false
    }

    /** The views, in decision 29's order. */
    val views = JBTabbedPane().apply {
        for (key in listOf("devtool.view.methods", "devtool.view.events", "devtool.view.code", "devtool.view.inlining")) {
            addTab(NewspeakBundle.message(key), JPanel())
        }
    }

    init {
        add(errorLabel, BorderLayout.NORTH)
        add(views, BorderLayout.CENTER)
    }

    /** Shows [message], the reason the run's file is read no further. */
    fun showError(message: String) {
        errorLabel.text = message
        errorLabel.isVisible = true
    }

    /** The error shown; null while none is. */
    val shownError: String? get() = errorLabel.text.takeIf { errorLabel.isVisible }
}
