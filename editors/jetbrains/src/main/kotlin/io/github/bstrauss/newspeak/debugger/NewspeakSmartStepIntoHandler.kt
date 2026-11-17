package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.editor.Document
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.util.TextRange
import com.intellij.util.concurrency.AppExecutorUtil
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.frame.XSuspendContext
import com.intellij.xdebugger.stepping.XSmartStepIntoHandler
import com.intellij.xdebugger.stepping.XSmartStepIntoVariant
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.dap.DapStepInTarget
import org.jetbrains.concurrency.AsyncPromise
import org.jetbrains.concurrency.Promise

/**
 * Smart Step Into over the agent's `stepInTargets`: the sends of the
 * stop's line the VM can step into, in the order the line runs them, each
 * highlighted at its span; the chosen one is stepped into by `stepIn`
 * with its `targetId`. A line with no target is stepped into as a plain
 * step into, the platform's own fallback.
 */
class NewspeakSmartStepIntoHandler(private val process: NewspeakDebugProcess) :
    XSmartStepIntoHandler<NewspeakSmartStepIntoHandler.Target>() {
    /** One send of the line, by the agent's label -- its selector -- and the span the editor highlights. */
    class Target(val target: DapStepInTarget, private val range: TextRange?) : XSmartStepIntoVariant() {
        override fun getText(): String = target.label

        override fun getHighlightRange(): TextRange? = range
    }

    /** Never asked: the platform asks [computeSmartStepVariantsAsync], which waits for the agent off the event dispatch thread. */
    override fun computeSmartStepVariants(position: XSourcePosition): List<Target> = emptyList()

    override fun computeSmartStepVariantsAsync(position: XSourcePosition): Promise<List<Target>> {
        val answer = AsyncPromise<List<Target>>()
        // The answer completes on the client's reader thread, which must never wait for a read action.
        process.stepInTargets().whenCompleteAsync({ targets, failure ->
            if (failure != null) {
                answer.setError(messageOf(failure))
                return@whenCompleteAsync
            }
            val document = ReadAction.computeBlocking<Document?, RuntimeException> { FileDocumentManager.getInstance().getDocument(position.file) }
            answer.setResult(targets.map { target -> Target(target, document?.let { rangeOf(it, target) }) })
        }, AppExecutorUtil.getAppExecutorService())
        return answer
    }

    override fun startStepInto(variant: Target, context: XSuspendContext?) {
        process.stepIntoTarget(variant.target.id)
    }

    override fun getPopupTitle(): String = NewspeakBundle.message("debugger.smart.step.into.title")

    private companion object {
        /** The target's span in [document], from its 1-based lines and columns; null for one it does not fit. */
        fun rangeOf(document: Document, target: DapStepInTarget): TextRange? = ReadAction.computeBlocking<TextRange?, RuntimeException> {
            val start = offsetOf(document, target.line, target.column) ?: return@computeBlocking null
            val end = offsetOf(document, target.endLine ?: target.line, target.endColumn) ?: return@computeBlocking null
            if (start <= end) TextRange(start, end) else null
        }

        fun offsetOf(document: Document, line: Int?, column: Int?): Int? {
            if (line == null || column == null || line < 1 || line > document.lineCount || column < 1) return null
            val offset = document.getLineStartOffset(line - 1) + column - 1
            return offset.takeIf { offset <= document.getLineEndOffset(line - 1) }
        }
    }
}
