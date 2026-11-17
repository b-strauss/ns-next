package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.ui.InputValidator
import com.intellij.openapi.ui.Messages
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.impl.ui.tree.actions.XDebuggerTreeActionBase
import com.intellij.xdebugger.impl.ui.tree.nodes.XValueNodeImpl
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap

/**
 * The objects a session marked, by label (`nsNext/mark`, `nsNext/unmark`;
 * `debugger.md`, "Marking objects"). The agent holds each mark across
 * stops until it is taken off, names the object in every later
 * evaluation as `<label>_DebugLabel`, and ends its text with ` [label]`
 * wherever a stop shows it; the session keeps the labels it gave, so that
 * a value's text tells its label only where it is one of them.
 *
 * The platform's own Mark Object is not used: its value marker provider
 * is handed the value alone, never the label the user typed in the
 * platform's dialog, which the platform keeps in a map only its internal
 * API reaches -- and the agent needs that label to name the object by.
 */
class NewspeakValueMarkers(private val session: XDebugSession, private val client: () -> DapClient?) {
    private val labels = ConcurrentHashMap.newKeySet<String>()

    /** Marks [value] with [label], which moves from any object it marked before, and shows every value again with its mark. */
    fun mark(value: NewspeakValue, label: String): CompletableFuture<*> {
        val reference = value.variable.valueReference ?: return CompletableFuture.failedFuture<Unit>(IllegalArgumentException("the value is withheld"))
        val client = client() ?: return CompletableFuture.failedFuture<Unit>(IllegalStateException("no session"))
        val arguments = JsonObject().apply {
            addProperty("valueReference", reference)
            addProperty("label", label)
        }
        return client.request("nsNext/mark", arguments).thenRun {
            // An object marked before is renamed: its old label no longer names anything.
            labelOf(value)?.let(labels::remove)
            labels.add(label)
            session.rebuildViews()
        }
    }

    /** Takes [label] off the object it marks. */
    fun unmark(label: String): CompletableFuture<*> {
        val client = client() ?: return CompletableFuture.failedFuture<Unit>(IllegalStateException("no session"))
        return client.request("nsNext/unmark", JsonObject().apply { addProperty("label", label) }).thenRun {
            labels.remove(label)
            session.rebuildViews()
        }
    }

    /** The label [value] is marked with, as its text ends: one of this session's. */
    fun labelOf(value: NewspeakValue): String? {
        val text = value.variable.value
        if (!text.endsWith("]")) return null
        val open = text.lastIndexOf(" [")
        if (open < 0) return null
        return text.substring(open + 2, text.length - 1).takeIf(labels::contains)
    }

    companion object {
        /** A label the agent takes: an identifier of letters, digits and `_`, not starting with a digit, not ending in the suffix it names it by. */
        fun isLabel(text: String): Boolean =
            text.isNotEmpty() &&
                !text[0].isDigit() &&
                text.all { character -> character.isLetterOrDigit() || character == '_' } &&
                !text.endsWith(SUFFIX)

        /** What an evaluation names a marked object by, after its label. */
        const val SUFFIX = "_DebugLabel"
    }
}

/**
 * Mark Object… and Unmark Object on a value of a Newspeak session's tree:
 * asks for a label -- the value's name, by default, where it is one --
 * and marks the object with it, or takes the value's label off. Shown for
 * a Newspeak session, enabled on a value the agent hands out a value
 * reference for, which a withheld one has not.
 */
class NewspeakMarkObjectAction : XDebuggerTreeActionBase() {
    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    override fun update(e: AnActionEvent) {
        val markers = markersOf(e)
        e.presentation.isVisible = markers != null
        if (markers == null) return
        super.update(e)
        val value = getSelectedNode(e.dataContext)?.valueContainer as? NewspeakValue
        e.presentation.text = NewspeakBundle.message(
            if (value != null && markers.labelOf(value) != null) "debugger.mark.object.unmark" else "debugger.mark.object.mark",
        )
    }

    override fun isEnabled(node: XValueNodeImpl, e: AnActionEvent): Boolean =
        (node.valueContainer as? NewspeakValue)?.variable?.valueReference != null

    override fun perform(node: XValueNodeImpl, nodeName: String, e: AnActionEvent) {
        val markers = markersOf(e) ?: return
        val value = node.valueContainer as? NewspeakValue ?: return
        val session = sessionOf(e) ?: return
        val marked = markers.labelOf(value)
        val done = if (marked != null) {
            markers.unmark(marked)
        } else {
            val label = Messages.showInputDialog(
                session.project,
                NewspeakBundle.message("debugger.mark.object.message", NewspeakValueMarkers.SUFFIX),
                NewspeakBundle.message("debugger.mark.object.title"),
                null,
                nodeName.takeIf(NewspeakValueMarkers::isLabel).orEmpty(),
                LabelValidator,
            ) ?: return
            markers.mark(value, label)
        }
        done.whenComplete { _, failure ->
            if (failure != null) session.reportError(NewspeakBundle.message("debugger.error.request", messageOf(failure)))
        }
    }

    private fun sessionOf(event: AnActionEvent): XDebugSession? =
        event.getData(XDebugSession.DATA_KEY) ?: event.project?.let { project -> XDebuggerManager.getInstance(project).currentSession }

    private fun markersOf(event: AnActionEvent): NewspeakValueMarkers? = (sessionOf(event)?.debugProcess as? NewspeakDebugProcess)?.markers

    /** The dialog's check: a label the agent takes. */
    private object LabelValidator : InputValidator {
        override fun checkInput(inputString: String?): Boolean = inputString != null && NewspeakValueMarkers.isLabel(inputString)

        override fun canClose(inputString: String?): Boolean = checkInput(inputString)
    }
}
