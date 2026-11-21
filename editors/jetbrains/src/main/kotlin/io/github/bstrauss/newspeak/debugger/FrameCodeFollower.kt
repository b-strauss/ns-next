package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.FileEditorManagerListener
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebugSessionListener
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import java.util.concurrent.atomic.AtomicInteger

/**
 * The frame-code tab of a session: a read-only listing that replaces its
 * text with the code of the frame the session selects, at every stop and
 * at every selection of another frame, until the tab closes or the
 * session ends. A listing is asked for on the connection's thread and
 * written on the event dispatch thread, so that no wait blocks it; only
 * the answer to the latest request is written, since the answers of
 * several selections made in a row arrive in no fixed order.
 */
class FrameCodeFollower private constructor(
    private val session: XDebugSession,
    private val process: NewspeakDebugProcess,
    val file: CodeListingFile,
) {
    private val lifetime = Disposer.newDisposable(process.sessionLifetime, "Newspeak frame code of ${file.name}")
    private val requested = AtomicInteger()

    @Volatile
    private var ended = false

    init {
        session.addSessionListener(
            object : XDebugSessionListener {
                override fun sessionPaused() = refresh()

                override fun stackFrameChanged() = refresh()

                override fun sessionStopped() = end()
            },
            lifetime,
        )
        session.project.messageBus.connect(lifetime).subscribe(
            FileEditorManagerListener.FILE_EDITOR_MANAGER,
            object : FileEditorManagerListener {
                override fun fileClosed(source: FileEditorManager, closed: VirtualFile) {
                    if (closed == file) end()
                }
            },
        )
    }

    /** Shows the selected frame's code, if the session is stopped at one. */
    fun refresh() {
        if (ended) return
        val frame = session.currentStackFrame as? NewspeakStackFrame ?: return
        val sequence = requested.incrementAndGet()
        process.frameCode(frame).whenComplete { code, failure ->
            // A frame the stop no longer has is left as the tab shows it.
            if (failure != null) return@whenComplete
            ApplicationManager.getApplication().invokeLater(
                { if (!ended && requested.get() == sequence) write(code) },
                ModalityState.nonModal(),
            )
        }
    }

    private fun write(code: FrameCode) {
        val document = FileDocumentManager.getInstance().getDocument(file) ?: return
        val text = text(code)
        if (document.text == text) return
        WriteAction.run<RuntimeException> {
            // The file is read-only to everything but this, its one writer.
            file.isWritable = true
            document.setReadOnly(false)
            try {
                document.setText(text)
            } finally {
                document.setReadOnly(true)
                file.isWritable = false
            }
        }
    }

    private fun end() {
        ended = true
        if (process.frameCodeFollower === this) process.frameCodeFollower = null
        Disposer.dispose(lifetime)
    }

    companion object {
        /** The tab's name, one for every frame it follows, so that it never names another body than its text shows. */
        const val TITLE = "Frame Code"

        /** The tab's text: the bytecode, then the machine code or the line that says there is none. */
        fun text(code: FrameCode): String =
            code.bytecode.trimEnd('\n') + "\n" + (code.machineCode?.trimEnd('\n') ?: NewspeakBundle.message("devtool.frame.interpreted")) + "\n"

        /** Opens the tab of [code], which follows [session]'s selected frame from now on; a tab already open is shown and refreshed instead. */
        fun open(session: XDebugSession, process: NewspeakDebugProcess, code: FrameCode) {
            val manager = FileEditorManager.getInstance(session.project)
            val existing = process.frameCodeFollower
            if (existing != null && !existing.ended) {
                manager.openFile(existing.file, true)
                existing.refresh()
                return
            }
            val file = CodeListingFile(TITLE, text(code))
            val follower = FrameCodeFollower(session, process, file)
            // A session that stopped while the answer was in flight never says so again.
            if (session.isStopped) {
                follower.end()
                return
            }
            process.frameCodeFollower = follower
            manager.openFile(file, true)
            // A step between the answer and the tab's opening is shown too.
            follower.refresh()
        }
    }
}
