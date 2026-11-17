package io.github.bstrauss.newspeak.debugger

import com.intellij.xdebugger.frame.XExecutionStack
import com.intellij.xdebugger.frame.XSuspendContext

/** A stop of the program: its one thread's stack. */
class NewspeakSuspendContext(internal val stack: NewspeakExecutionStack) : XSuspendContext() {
    override fun getActiveExecutionStack(): XExecutionStack = stack

    override fun getExecutionStacks(): Array<XExecutionStack> = arrayOf(stack)
}
