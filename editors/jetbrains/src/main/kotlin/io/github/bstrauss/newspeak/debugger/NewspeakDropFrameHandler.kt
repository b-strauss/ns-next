package io.github.bstrauss.newspeak.debugger

import com.intellij.util.ThreeState
import com.intellij.xdebugger.frame.XDropFrameHandler
import com.intellij.xdebugger.frame.XStackFrame

/**
 * The platform's Reset Frame over the agent's `restartFrame`: the frame
 * runs again from its start, every `ensure:` block above it run first,
 * and the stop it makes there is shown as the program's next. A frame of
 * the core library, which the agent says it cannot restart
 * (`canRestart`), is not offered.
 */
class NewspeakDropFrameHandler(private val process: NewspeakDebugProcess) : XDropFrameHandler {
    override fun canDropFrame(frame: XStackFrame): ThreeState = ThreeState.fromBoolean(frame is NewspeakStackFrame && frame.canRestart)

    override fun drop(frame: XStackFrame) {
        if (frame is NewspeakStackFrame) process.restartFrame(frame)
    }
}
