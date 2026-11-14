package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.xdebugger.frame.XCompositeNode
import com.intellij.xdebugger.frame.XNamedValue
import com.intellij.xdebugger.frame.XValueChildrenList
import com.intellij.xdebugger.frame.XValueNode
import com.intellij.xdebugger.frame.XValuePlace
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapVariable
import java.util.concurrent.CompletionException

/**
 * A variable of a stopped frame, or a child of one's value, as the agent
 * describes it: its value's text and its class, and its children only once
 * the tree opens it, a page at a time.
 */
class NewspeakValue(private val client: DapClient, private val variable: DapVariable) : XNamedValue(variable.name) {
    override fun computePresentation(node: XValueNode, place: XValuePlace) {
        node.setPresentation(null, variable.type, variable.value, variable.variablesReference > 0)
    }

    override fun computeChildren(node: XCompositeNode) {
        if (variable.variablesReference <= 0) {
            node.addChildren(XValueChildrenList.EMPTY, true)
            return
        }
        addVariablePage(client, variable.variablesReference, 0, variable.indexedVariables, node)
    }
}

/** How many children a node shows before it offers the next ones: an `Array` of a million elements is one request a page. */
internal const val VARIABLES_PAGE = 100

/**
 * Adds to [node] the variables of [reference] from [start], a page of
 * them, and offers the next page while there may be more: up to [total]
 * when the agent said how many there are (an indexed value's), otherwise
 * while a page comes back full.
 */
internal fun addVariablePage(client: DapClient, reference: Int, start: Int, total: Int?, node: XCompositeNode) {
    val arguments = JsonObject().apply {
        addProperty("variablesReference", reference)
        addProperty("start", start)
        addProperty("count", VARIABLES_PAGE)
    }
    client.request("variables", arguments).whenComplete { body, failure ->
        if (node.isObsolete) return@whenComplete
        if (failure != null) {
            node.setErrorMessage(messageOf(failure))
            return@whenComplete
        }
        val variables = variablesOf(body)
        val children = XValueChildrenList(variables.size)
        for (variable in variables) children.add(NewspeakValue(client, variable))
        val loaded = start + variables.size
        val more = variables.isNotEmpty() && if (total != null) loaded < total else variables.size == VARIABLES_PAGE
        node.addChildren(children, !more)
        if (more) {
            // -1: the platform's "more, how many unknown", for named children the agent does not count.
            node.tooManyChildren(total?.minus(loaded) ?: -1) { addVariablePage(client, reference, loaded, total, node) }
        }
    }
}

internal fun variablesOf(body: JsonObject): List<DapVariable> =
    body.get("variables")
        ?.takeIf { element -> element.isJsonArray }
        ?.asJsonArray
        ?.filter { element -> element.isJsonObject }
        ?.map { element -> DapVariable.from(element.asJsonObject) }
        .orEmpty()

/** What failed a request, without the wrapper a future's continuation sees. */
internal fun causeOf(failure: Throwable): Throwable = if (failure is CompletionException) failure.cause ?: failure else failure

/** The text of what failed a request. */
internal fun messageOf(failure: Throwable): String {
    val cause = causeOf(failure)
    return cause.message ?: cause.toString()
}
