package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.icons.AllIcons
import com.intellij.ui.ColoredTextContainer
import com.intellij.ui.SimpleTextAttributes
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.evaluation.XDebuggerEvaluator
import com.intellij.xdebugger.frame.XCompositeNode
import com.intellij.xdebugger.frame.XStackFrame
import com.intellij.xdebugger.frame.XValueChildrenList
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapScope
import io.github.bstrauss.newspeak.debugger.dap.DapStackFrame
import io.github.bstrauss.newspeak.debugger.dap.DapVariable
import java.util.concurrent.CompletableFuture

/**
 * One frame of a stop, at [position] -- null for a frame the agent places
 * nowhere, as one of a body compiled without its metadata -- with its
 * variables, and at a step's stop the value the step's last call returned.
 * [depth] counts the frames below it, the stack's first frame 0.
 */
class NewspeakStackFrame(
    private val client: DapClient,
    private val frame: DapStackFrame,
    private val position: XSourcePosition?,
    private val depth: Int,
) : XStackFrame() {
    /** A frame the agent hides by default -- the shipped modules' own -- stays listed, drawn greyed. */
    val hidden: Boolean get() = frame.presentationHint == "subtle"

    /** A frame of a shipped module's body: a library frame, which the frames view folds with its neighbours. */
    val shipped: Boolean get() = frame.shipped

    /** Whether the agent restarts the frame: never one of the core library's. */
    val canRestart: Boolean get() = frame.canRestart

    /** The agent's id of the frame at this stop. */
    val id: Int get() = frame.id

    override fun getSourcePosition(): XSourcePosition? = position

    /** Evaluates in this frame: the Evaluate dialog, watches and the editor's hovers. */
    override fun getEvaluator(): XDebuggerEvaluator = NewspeakEvaluator(client, this)

    /**
     * What the platform keeps the variables view's expanded nodes by and
     * restores them for at the next stop: the activation's place across
     * stops, which the DAP frame's `id`, numbered afresh at each stop, is
     * not. A frame is the same as one of an earlier stop when it runs the
     * same body -- the agent's name of it, `Class>>selector` or a block's
     * `[] in Class>>selector`, in the same module -- at the same depth, as
     * Java's frames are by their method: a step within one method, or a
     * loop's block activated again at the same depth, keeps what the user
     * opened, while a recursive activation, deeper, is another frame. A
     * data class, as the platform's front end compares the object by its
     * text.
     */
    override fun getEqualityObject(): Any = Identity(frame.name, frame.source?.path ?: frame.source?.name, depth)

    /** The parts of [getEqualityObject]: the body's name, its module's file or name, and the depth. */
    private data class Identity(val body: String, val module: String?, val depth: Int)

    override fun customizePresentation(component: ColoredTextContainer) {
        component.append(frame.name, if (hidden) SimpleTextAttributes.GRAYED_ATTRIBUTES else SimpleTextAttributes.REGULAR_ATTRIBUTES)
        if (position != null) {
            component.append(
                NewspeakBundle.message("debugger.frame.location", position.file.name, (position.line + 1).toString()),
                SimpleTextAttributes.GRAYED_ATTRIBUTES,
            )
        }
        component.setIcon(AllIcons.Debugger.Frame)
    }

    override fun computeChildren(node: XCompositeNode) {
        val arguments = JsonObject().apply { addProperty("frameId", frame.id) }
        client.request("scopes", arguments).whenComplete { body, failure ->
            if (node.isObsolete) return@whenComplete
            if (failure != null) {
                node.setErrorMessage(messageOf(failure))
                return@whenComplete
            }
            val scopes = body.get("scopes")
                ?.takeIf { element -> element.isJsonArray }
                ?.asJsonArray
                ?.filter { element -> element.isJsonObject }
                ?.map { element -> DapScope.from(element.asJsonObject) }
                .orEmpty()
            val locals = scopes.firstOrNull { scope -> scope.presentationHint == "locals" }
            val returned = scopes.firstOrNull { scope -> scope.presentationHint == "returnValue" }
            returnedValues(returned).whenComplete { values, _ ->
                if (node.isObsolete) return@whenComplete
                // The returned value first, as Java's is shown above the locals: named by the
                // call that returned it, the agent's name, and drawn with the platform's icon
                // of a returned value, which says what it is as Java's says it.
                if (!values.isNullOrEmpty()) {
                    val top = XValueChildrenList()
                    for (variable in values) {
                        top.addTopValue(NewspeakValue(client, variable, AllIcons.Debugger.WatchLastReturnValue))
                    }
                    node.addChildren(top, locals == null)
                } else if (locals == null) {
                    node.addChildren(XValueChildrenList.EMPTY, true)
                }
                if (locals != null) addVariablePage(client, locals.variablesReference, 0, null, node, locals = true)
            }
        }
    }

    /**
     * The names of the frame's variables, asked once for the stop: a hover
     * evaluates only these, since reading a variable runs no code, where
     * any other name may be a send.
     */
    internal val variableNames: CompletableFuture<Set<String>> by lazy {
        client.request("scopes", JsonObject().apply { addProperty("frameId", frame.id) })
            .thenCompose { body ->
                val locals = body.get("scopes")
                    ?.takeIf { element -> element.isJsonArray }
                    ?.asJsonArray
                    ?.filter { element -> element.isJsonObject }
                    ?.map { element -> DapScope.from(element.asJsonObject) }
                    ?.firstOrNull { scope -> scope.presentationHint == "locals" }
                    ?: return@thenCompose CompletableFuture.completedFuture(emptySet())
                val arguments = JsonObject().apply {
                    addProperty("variablesReference", locals.variablesReference)
                    addProperty("start", 0)
                    addProperty("count", VARIABLES_PAGE)
                }
                client.request("variables", arguments).thenApply { variables -> variablesOf(variables).map(DapVariable::name).toSet() }
            }
    }

    private fun returnedValues(scope: DapScope?): CompletableFuture<List<DapVariable>> {
        if (scope == null) return CompletableFuture.completedFuture(emptyList())
        val arguments = JsonObject().apply { addProperty("variablesReference", scope.variablesReference) }
        return client.request("variables", arguments).handle { body, failure -> if (failure != null) emptyList() else variablesOf(body) }
    }
}
