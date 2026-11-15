package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.ToggleAction
import com.intellij.openapi.project.DumbAware
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebuggerManager

/**
 * "Step Into Kernel and Platform Code" in the debug tool window's Debugger
 * Settings, as Java's debugger offers its own switches there: whether a
 * step goes into the shipped modules' code, the same setting as the
 * Stepping section's switch ([NewspeakSteppingConfigurable]), applied at
 * once to every running session. Shown for a Newspeak session only.
 */
class NewspeakStepIntoShippedAction : ToggleAction(), DumbAware {
    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    override fun update(event: AnActionEvent) {
        super.update(event)
        event.presentation.isEnabledAndVisible = sessionOf(event)?.debugProcess is NewspeakDebugProcess
    }

    override fun isSelected(event: AnActionEvent): Boolean = NewspeakDebuggerSettings.getInstance().stepIntoShipped

    override fun setSelected(event: AnActionEvent, state: Boolean) {
        NewspeakDebuggerSettings.getInstance().update(state)
    }

    /** The session the tool window shows, or, where its context names none, the project's current one. */
    private fun sessionOf(event: AnActionEvent): XDebugSession? =
        event.getData(XDebugSession.DATA_KEY) ?: event.project?.let { project -> XDebuggerManager.getInstance(project).currentSession }
}
