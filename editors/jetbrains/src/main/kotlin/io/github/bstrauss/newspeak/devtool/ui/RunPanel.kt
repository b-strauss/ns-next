package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.project.Project
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
class RunPanel(project: Project, val run: TraceRun) : JPanel(BorderLayout()), Disposable {
    private val errorLabel = JBLabel(AllIcons.General.Error).apply {
        border = JBUI.Borders.empty(4, 8)
        isVisible = false
    }

    /** The views, in decision 29's order. */
    val views = JBTabbedPane().apply {
        addTab(NewspeakBundle.message("devtool.view.methods"), MethodsTab(project, run, this@RunPanel))
        addTab(NewspeakBundle.message("devtool.view.events"), EventsTab(project, run, this@RunPanel))
        addTab(NewspeakBundle.message("devtool.view.code"), JPanel())
        addTab(NewspeakBundle.message("devtool.view.inlining"), InliningTab(project, run, this@RunPanel))
    }

    init {
        add(errorLabel, BorderLayout.NORTH)
        add(views, BorderLayout.CENTER)
    }

    /** Releases nothing itself; its views' listeners are children of it, which the disposal of the run's tab ends. */
    override fun dispose() = Unit

    /** Shows [message], the reason the run's file is read no further. */
    fun showError(message: String) {
        errorLabel.text = message
        errorLabel.isVisible = true
    }

    /** The error shown; null while none is. */
    val shownError: String? get() = errorLabel.text.takeIf { errorLabel.isVisible }
}
