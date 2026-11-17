package io.github.bstrauss.newspeak.debugger

import com.intellij.ui.ColoredTextContainer
import com.intellij.ui.SimpleTextAttributes
import com.intellij.util.ui.EmptyIcon
import com.intellij.xdebugger.frame.XExecutionStack
import com.intellij.xdebugger.frame.XStackFrame
import com.intellij.xdebugger.settings.XDebuggerSettingsManager
import io.github.bstrauss.newspeak.NewspeakBundle

/**
 * The one thread a stop shows -- the VM runs on one (`debugger.md`,
 * "Threads") -- with the frames of its first page, read before the stop
 * is shown so that its top frame is at hand, and the rest of its
 * [totalFrames] read a page at a time once the frames view asks for them.
 *
 * The frames of the core library are library frames: while the frames
 * view hides library frames, its Show Library Frames off -- the
 * platform's setting, which the debug process says it respects
 * (`isLibraryFrameFilterSupported`) -- each run of them below the top
 * frame is folded into one line that says how many it holds, as Java's
 * frames are; the top frame, where the program stands, is always shown.
 */
class NewspeakExecutionStack(
    private val process: NewspeakDebugProcess,
    internal val firstFrames: List<NewspeakStackFrame>,
    private val totalFrames: Int,
) : XExecutionStack(NewspeakBundle.message("debugger.thread.name")) {
    override fun getTopFrame(): XStackFrame? = firstFrames.firstOrNull()

    override fun computeStackFrames(firstFrameIndex: Int, container: XStackFrameContainer) {
        if (XDebuggerSettingsManager.getInstance().dataViewSettings.isShowLibraryStackFrames) {
            addFrames(firstFrameIndex, container)
        } else {
            // The view counts the folds among its frames, so the index is one of the folded list.
            addFrames(0, LibraryFramesFolder(container, firstFrames.firstOrNull(), firstFrameIndex))
        }
    }

    private fun addFrames(firstFrameIndex: Int, container: XStackFrameContainer) {
        if (firstFrameIndex < firstFrames.size) {
            container.addStackFrames(firstFrames.subList(firstFrameIndex, firstFrames.size), firstFrames.size >= totalFrames)
        } else if (firstFrames.size >= totalFrames) {
            container.addStackFrames(emptyList<XStackFrame>(), true)
        }
        val next = maxOf(firstFrameIndex, firstFrames.size)
        if (next < totalFrames) process.addFramesFrom(next, totalFrames, container)
    }

    /**
     * Hands [container] the frames with each run of library frames below
     * [top] folded into one [LibraryFrames], a run that goes on past a
     * page held until it ends, and leaves out the first [skip] of what it
     * would hand on, which the view holds already.
     */
    private class LibraryFramesFolder(
        private val container: XStackFrameContainer,
        private val top: NewspeakStackFrame?,
        private var skip: Int,
    ) : XStackFrameContainer by container {
        private val run = mutableListOf<NewspeakStackFrame>()

        override fun addStackFrames(stackFrames: MutableList<out XStackFrame>, last: Boolean) {
            val folded = mutableListOf<XStackFrame>()
            for (frame in stackFrames) {
                if (frame is NewspeakStackFrame && frame.shipped && frame !== top) {
                    run.add(frame)
                } else {
                    flushInto(folded)
                    folded.add(frame)
                }
            }
            if (last) flushInto(folded)
            val shown = folded.drop(skip)
            skip -= folded.size - shown.size
            if (shown.isNotEmpty() || last) container.addStackFrames(shown.toMutableList(), last)
        }

        private fun flushInto(folded: MutableList<XStackFrame>) {
            if (run.isEmpty()) return
            folded.add(LibraryFrames(run.toList()))
            run.clear()
        }
    }
}

/** A run of library frames folded into one line of the frames view: how many it holds, greyed. */
class LibraryFrames(val frames: List<NewspeakStackFrame>) : XStackFrame() {
    override fun customizePresentation(component: ColoredTextContainer) {
        component.append(NewspeakBundle.message("debugger.frames.library", frames.size), SimpleTextAttributes.GRAYED_ATTRIBUTES)
        component.setIcon(EmptyIcon.ICON_16)
    }
}
