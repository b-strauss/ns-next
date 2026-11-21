package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.util.Disposer
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile

class FrameCodeFollowerTest : BasePlatformTestCase() {
    private fun listing(mark: Int): String = buildString {
        append("method Demo>>run (args 0, registers 16)\n")
        for (index in 0 until LINES) {
            val gap = if (index == mark) "->" else "  "
            append(index.toString().padStart(4)).append(gap).append(" stmt $index  LoadLiteral r$index, $index\n")
        }
        append("This frame goes on in the interpreter.\n")
    }

    fun testAStepReplacesWhatChangedAndShowsTheMarkedLine() {
        val file = CodeListingFile(FrameCodeFollower.TITLE, listing(mark = 2))
        val editor = checkNotNull(FileEditorManager.getInstance(project).openTextEditor(OpenFileDescriptor(project, file), true))
        val document = checkNotNull(FileDocumentManager.getInstance().getDocument(file))
        // A marker over the head line, which every step leaves as it is.
        val head = document.createRangeMarker(0, document.getLineEndOffset(0))
        editor.caretModel.moveToOffset(document.getLineStartOffset(LINES - 1))
        FrameCodeFollower.write(project, file, listing(mark = 40))
        assertEquals(listing(mark = 40), document.text)
        assertTrue("the text before the change is not written again", head.isValid)
        assertEquals(0, head.startOffset)
        assertEquals(document.getLineEndOffset(0), head.endOffset)
        // The caret stands at the marked line, never at the start.
        assertEquals(41, editor.caretModel.logicalPosition.line)
        assertEquals(41, FrameCodeFollower.markedLine(document.text))
        // A text that did not change leaves the caret where the user put it.
        editor.caretModel.moveToOffset(document.getLineStartOffset(5))
        FrameCodeFollower.write(project, file, listing(mark = 40))
        assertEquals(5, editor.caretModel.logicalPosition.line)
        assertFalse(file.isWritable)
    }

    fun testTheMachineCodesMarkIsFoundWithoutABytecodeMark() {
        val text = "--- code {Demo>>run} baseline x64 base=0x1000 bytes=8\n  @0  stmt 0  Return r1\n    +0x0000  mov rax,1\n  ->+0x0004  ret\n--- end\n"
        assertEquals(3, FrameCodeFollower.markedLine(text))
        assertNull(FrameCodeFollower.markedLine("method Demo>>run (args 0, registers 16)\n   0   stmt 0  MakeClosure r5, [] ->\n"))
    }

    fun testOnlyTheLastEditorOfTheTabEndsTheFollower() {
        val file = CodeListingFile(FrameCodeFollower.TITLE, listing(mark = 0))
        val manager = FileEditorManager.getInstance(project)
        manager.openFile(file, true)
        // Another split's editor of the tab closed: this one still shows it.
        assertFalse(FrameCodeFollower.closesFollower(manager, file, file))
        manager.closeFile(file)
        assertTrue(FrameCodeFollower.closesFollower(manager, file, file))
        val other = CodeListingFile("other", "text\n")
        assertFalse(FrameCodeFollower.closesFollower(manager, other, file))
    }

    fun testADeferredStepAfterTheLifetimesDisposalDoesNothing() {
        val session = Disposer.newDisposable(testRootDisposable, "session")
        val lifetime = Disposer.newDisposable(session, "follower")
        val released = FrameCodeFollower.expiresWith(lifetime)
        val steps = ArrayList<String>()
        ApplicationManager.getApplication().invokeLater({ steps += "before" }, ModalityState.any(), released)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals("a step while the follower lives runs", listOf("before"), steps)
        ApplicationManager.getApplication().invokeLater({ steps += "after" }, ModalityState.any(), released)
        // The session ends, and with it the follower's lifetime, before the step reaches the event dispatch thread.
        Disposer.dispose(session)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertEquals(listOf("before"), steps)
        assertTrue(released.value(null))
    }

    private companion object {
        const val LINES = 60
    }
}
