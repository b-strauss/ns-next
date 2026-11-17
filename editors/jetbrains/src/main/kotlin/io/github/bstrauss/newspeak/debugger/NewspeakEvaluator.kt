package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.openapi.editor.Document
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.TextRange
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.evaluation.ExpressionInfo
import com.intellij.xdebugger.evaluation.XDebuggerEvaluator
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapVariable
import org.jetbrains.concurrency.AsyncPromise
import org.jetbrains.concurrency.Promise
import org.jetbrains.concurrency.resolvedPromise

/**
 * Evaluates an expression in [frame] through the agent's `evaluate`, as a
 * block in the stop's scope (`debugger.md`, decision 8): the Evaluate
 * dialog, every watch at every stop, and the editor's hover. The answer is
 * a value as the variables view shows one, with its children; an
 * evaluation the agent refuses or that ends with an exception is the
 * error the platform shows in the value's place.
 */
class NewspeakEvaluator(private val client: DapClient, private val frame: NewspeakStackFrame) : XDebuggerEvaluator() {
    override fun evaluate(expression: String, callback: XEvaluationCallback, expressionPosition: XSourcePosition?) {
        val arguments = JsonObject().apply {
            addProperty("expression", expression)
            addProperty("frameId", frame.id)
        }
        client.request("evaluate", arguments).whenComplete { body, failure ->
            if (failure != null) {
                callback.errorOccurred(messageOf(failure))
            } else {
                callback.evaluated(NewspeakValue(client, DapVariable.evaluated(expression, body)))
            }
        }
    }

    /**
     * The expression at [offset]: the identifier around it. A hover, which
     * must have no side effect, takes it only when it names one of the
     * frame's variables, which reading runs no code; any other name may be
     * a send of the receiver's, which an evaluation would run.
     */
    override fun getExpressionInfoAtOffsetAsync(
        project: Project,
        document: Document,
        offset: Int,
        sideEffectsAllowed: Boolean,
    ): Promise<ExpressionInfo?> {
        val range = identifierAt(document.charsSequence, offset) ?: return resolvedPromise(null)
        if (sideEffectsAllowed) return resolvedPromise(ExpressionInfo(range))
        val name = range.subSequence(document.charsSequence).toString()
        val answer = AsyncPromise<ExpressionInfo?>()
        frame.variableNames.whenComplete { names, failure ->
            answer.setResult(if (failure == null && name in names) ExpressionInfo(range) else null)
        }
        return answer
    }

    internal companion object {
        /** The range of the identifier [offset] stands in or just after, a letter or `_` and then letters, digits and `_`. */
        fun identifierAt(text: CharSequence, offset: Int): TextRange? {
            fun part(index: Int) = index in text.indices && (text[index].isLetterOrDigit() || text[index] == '_')
            var start = minOf(offset, text.length)
            while (part(start - 1)) start--
            var end = start
            while (part(end)) end++
            if (end <= start || end < offset) return null
            val first = text[start]
            return if (first.isLetter() || first == '_') TextRange(start, end) else null
        }
    }
}
