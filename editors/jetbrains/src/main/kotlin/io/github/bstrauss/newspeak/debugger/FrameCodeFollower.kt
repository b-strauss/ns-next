package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.command.CommandProcessor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.LogicalPosition
import com.intellij.openapi.editor.ScrollType
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.FileEditorManagerListener
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.Condition
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebugSessionListener
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * The frame-code tab of a session: a read-only listing that replaces its
 * text with the code of the frame the session selects, at every stop and
 * at every selection of another frame, until the last editor of the tab
 * closes or the session ends. A listing is asked for on the connection's
 * thread and written on the event dispatch thread, so that no wait blocks
 * it; only the answer to the latest request is written, since the answers
 * of several selections made in a row arrive in no fixed order. A text
 * written replaces only what changed, so that a step leaves the view
 * where the text is the same, and each editor of the tab then shows the
 * marked line, never the start.
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

    /** True once the follower's lifetime is over -- ended, or its session or project gone -- after which a deferred step does nothing. */
    private val released = expiresWith(lifetime)

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
                    if (closed != file) return
                    // Decided once the close is done, whatever the platform still counts open while it tells
                    // of it: a split's editor closed while another still shows the tab goes on following.
                    ApplicationManager.getApplication().invokeLater(
                        { if (!ended && closesFollower(source, closed, file)) end() },
                        ModalityState.any(),
                        released,
                    )
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
                released,
            )
        }
    }

    private fun write(code: FrameCode) = write(session.project, file, text(code))

    private fun end() {
        ended = true
        if (process.frameCodeFollower === this) process.frameCodeFollower = null
        Disposer.dispose(lifetime)
    }

    companion object {
        /**
         * The expiry of a step deferred to the event dispatch thread: true
         * from [lifetime]'s disposal on, so that a step queued before it --
         * a close decision, an answer's write -- runs nothing once the
         * follower, its session or its project is gone.
         */
        internal fun expiresWith(lifetime: Disposable): Condition<Any?> {
            val over = AtomicBoolean()
            Disposer.register(lifetime) { over.set(true) }
            return Condition { over.get() }
        }

        /** Whether [closed], which [source] closed in one of its windows, was the last editor of the follower's [file]. */
        internal fun closesFollower(source: FileEditorManager, closed: VirtualFile, file: VirtualFile): Boolean = closed == file && !source.isFileOpen(file)

        /**
         * Writes [text] into [file]'s document, replacing only the stretch
         * between the longest common start and end of the two texts, so
         * that every line before and after the change stays where it was
         * in each editor; then puts each editor's caret at the marked line
         * -- the frame's instruction in the bytecode, else the machine
         * code's -- and scrolls it into view only where it is not shown.
         * A text that is the document's already is not written.
         */
        internal fun write(project: Project, file: CodeListingFile, text: String) {
            val document = FileDocumentManager.getInstance().getDocument(file) ?: return
            val old = document.charsSequence
            if (old.contentEquals(text)) return
            val prefix = commonPrefixLength(old, text)
            val suffix = minOf(commonSuffixLength(old, text), old.length - prefix, text.length - prefix)
            // No undo step: the user never typed it.
            CommandProcessor.getInstance().runUndoTransparentAction {
                WriteAction.run<RuntimeException> {
                    // The file is read-only to everything but this, its one writer.
                    file.isWritable = true
                    document.setReadOnly(false)
                    try {
                        document.replaceString(prefix, old.length - suffix, text.substring(prefix, text.length - suffix))
                    } finally {
                        document.setReadOnly(true)
                        file.isWritable = false
                    }
                }
            }
            val marked = markedLine(text) ?: return
            for (editor in EditorFactory.getInstance().getEditors(document, project)) {
                editor.caretModel.moveToLogicalPosition(LogicalPosition(marked, 0))
                editor.scrollingModel.scrollToCaret(ScrollType.MAKE_VISIBLE)
            }
        }

        /** The line of [text] a listing marks: the bytecode's `   7->`, else the machine code's `  ->+0x…`; null for none. */
        internal fun markedLine(text: String): Int? {
            val lines = text.lines()
            val bytecode = lines.indexOfFirst { line -> BYTECODE_MARK.containsMatchIn(line) }
            if (bytecode >= 0) return bytecode
            return lines.indexOfFirst { line -> line.startsWith(CODE_MARK) }.takeIf { index -> index >= 0 }
        }

        private fun commonPrefixLength(first: CharSequence, second: CharSequence): Int {
            val limit = minOf(first.length, second.length)
            var length = 0
            while (length < limit && first[length] == second[length]) length++
            return length
        }

        private fun commonSuffixLength(first: CharSequence, second: CharSequence): Int {
            val limit = minOf(first.length, second.length)
            var length = 0
            while (length < limit && first[first.length - 1 - length] == second[second.length - 1 - length]) length++
            return length
        }

        /** A bytecode line marked in place of the two spaces after its four-column index (`bytecode::disassemble_marked`). */
        private val BYTECODE_MARK = Regex("""^\s*\d+->""")

        /** A machine-code line marked where its call returns (`listing.rs`). */
        private const val CODE_MARK = "  ->"

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
