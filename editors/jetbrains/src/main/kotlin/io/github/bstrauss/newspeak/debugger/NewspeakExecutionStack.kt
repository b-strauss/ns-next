package io.github.bstrauss.newspeak.debugger

import com.intellij.xdebugger.frame.XExecutionStack
import com.intellij.xdebugger.frame.XStackFrame
import io.github.bstrauss.newspeak.NewspeakBundle

/**
 * The one thread a stop shows -- the VM runs on one (`debugger.md`,
 * "Threads") -- with the frames of its first page, read before the stop
 * is shown so that its top frame is at hand, and the rest of its
 * [totalFrames] read a page at a time once the frames view asks for them.
 */
class NewspeakExecutionStack(
    private val process: NewspeakDebugProcess,
    internal val firstFrames: List<NewspeakStackFrame>,
    private val totalFrames: Int,
) : XExecutionStack(NewspeakBundle.message("debugger.thread.name")) {
    override fun getTopFrame(): XStackFrame? = firstFrames.firstOrNull()

    override fun computeStackFrames(firstFrameIndex: Int, container: XStackFrameContainer) {
        if (firstFrameIndex < firstFrames.size) {
            container.addStackFrames(firstFrames.subList(firstFrameIndex, firstFrames.size), firstFrames.size >= totalFrames)
        } else if (firstFrames.size >= totalFrames) {
            container.addStackFrames(emptyList<XStackFrame>(), true)
        }
        val next = maxOf(firstFrameIndex, firstFrames.size)
        if (next < totalFrames) process.addFramesFrom(next, totalFrames, container)
    }
}
