package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.ui.Messages
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebuggerManager
import io.github.bstrauss.newspeak.NewspeakBundle

/**
 * Force Return on the frame the frames view selects, as Java's debugger
 * offers it there: asks for the expression whose value the frame returns
 * at once -- none, or a blank one, returns `nil` -- and the agent
 * evaluates it in that frame and forces its value (`nsNext/forceReturn`);
 * the program goes on to where the frame's sender goes on, the next stop.
 * Shown for a Newspeak session's stop, on a frame the agent may end: not
 * one of the core library's.
 */
class NewspeakForceReturnAction : AnAction(), DumbAware {
    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    override fun update(event: AnActionEvent) {
        val session = sessionOf(event)
        event.presentation.isVisible = session?.debugProcess is NewspeakDebugProcess
        event.presentation.isEnabled = session != null && session.isSuspended && (session.currentStackFrame as? NewspeakStackFrame)?.canRestart == true
    }

    override fun actionPerformed(event: AnActionEvent) {
        val session = sessionOf(event) ?: return
        val process = session.debugProcess as? NewspeakDebugProcess ?: return
        val frame = session.currentStackFrame as? NewspeakStackFrame ?: return
        val expression = Messages.showInputDialog(
            session.project,
            NewspeakBundle.message("debugger.force.return.message"),
            NewspeakBundle.message("debugger.force.return.title"),
            null,
        ) ?: return
        process.forceReturn(frame, expression)
    }

    /** The session the tool window shows, or, where its context names none, the project's current one. */
    private fun sessionOf(event: AnActionEvent): XDebugSession? =
        event.getData(XDebugSession.DATA_KEY) ?: event.project?.let { project -> XDebuggerManager.getInstance(project).currentSession }
}
