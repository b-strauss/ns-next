package io.github.bstrauss.newspeak.debugger

import com.intellij.icons.AllIcons
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.project.DumbAware
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebuggerManager

/**
 * Show Frame Code on the frame the frames view selects: asks the agent for
 * the frame's bytecode and machine code (`nsNext/frameCode`) and opens
 * them in a read-only editor tab that goes on to follow the selected
 * frame and every step (`FrameCodeFollower`). Shown for a Newspeak
 * session, enabled at a stop on one of its frames.
 */
class ShowFrameCodeAction : AnAction(AllIcons.Debugger.Frame), DumbAware {
    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    override fun update(event: AnActionEvent) {
        val session = sessionOf(event)
        event.presentation.isVisible = session?.debugProcess is NewspeakDebugProcess
        event.presentation.isEnabled = session != null && session.isSuspended && session.currentStackFrame is NewspeakStackFrame
    }

    override fun actionPerformed(event: AnActionEvent) {
        val session = sessionOf(event) ?: return
        val process = session.debugProcess as? NewspeakDebugProcess ?: return
        val frame = session.currentStackFrame as? NewspeakStackFrame ?: return
        process.frameCode(frame).whenComplete { code, failure ->
            if (failure != null) return@whenComplete
            ApplicationManager.getApplication().invokeLater({ if (!session.isStopped) FrameCodeFollower.open(session, process, code) }, ModalityState.nonModal())
        }
    }

    /** The session the tool window shows, or, where its context names none, the project's current one. */
    private fun sessionOf(event: AnActionEvent): XDebugSession? =
        event.getData(XDebugSession.DATA_KEY) ?: event.project?.let { project -> XDebuggerManager.getInstance(project).currentSession }
}
