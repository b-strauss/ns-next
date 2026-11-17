package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.xdebugger.frame.XCompositeNode
import com.intellij.xdebugger.frame.XNamedValue
import com.intellij.xdebugger.frame.XReferrersProvider
import com.intellij.xdebugger.frame.XValue
import com.intellij.xdebugger.frame.XValueChildrenList
import com.intellij.xdebugger.frame.XValueModifier
import com.intellij.xdebugger.frame.XValueNode
import com.intellij.xdebugger.frame.XValuePlace
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapVariable
import java.util.concurrent.CompletionException
import java.util.concurrent.ExecutionException
import javax.swing.Icon

/**
 * A variable of a stopped frame, or a child of one's value, or the value
 * of an evaluation, as the agent describes it: its value's text and its
 * class, and its children only once the tree opens it, a page at a time.
 * [icon] marks a value the tree tells apart from the variables, as the
 * value the last call returned. [locals] is the reference of the frame's
 * `Locals` scope for a variable of a frame, the only kind the agent sets
 * (`setVariable`), and null for any other value.
 */
class NewspeakValue(
    private val client: DapClient,
    val variable: DapVariable,
    private val icon: Icon? = null,
    private val locals: Int? = null,
) : XNamedValue(variable.name) {
    override fun computePresentation(node: XValueNode, place: XValuePlace) {
        node.setPresentation(icon, variable.type, variable.value, variable.variablesReference > 0)
    }

    override fun computeChildren(node: XCompositeNode) {
        if (variable.variablesReference <= 0) {
            node.addChildren(XValueChildrenList.EMPTY, true)
            return
        }
        addVariablePage(client, variable.variablesReference, 0, variable.indexedVariables, node)
    }

    /**
     * A frame's variable is set by the agent; a withheld one -- reached
     * through the core library's code, shown read-only -- and a referrer
     * on the stack, which names a place, are not.
     */
    override fun getModifier(): XValueModifier? {
        val reference = locals ?: return null
        if (variable.readOnly || variable.virtual) return null
        return NewspeakValueModifier(client, reference, variable.name)
    }

    /**
     * What refers to the value, for the platform's Show Referring Objects:
     * every value the agent hands out a value reference for, which a
     * withheld one has not.
     */
    override fun getReferrersProvider(): XReferrersProvider? {
        val reference = variable.valueReference ?: return null
        return object : XReferrersProvider() {
            override fun getReferringObjectsValue(): XValue = NewspeakReferrers(client, reference)
        }
    }
}

/**
 * The objects and frame variables that refer to the value [valueReference]
 * names, as its children, a page at a time (`nsNext/referrers`).
 */
class NewspeakReferrers(private val client: DapClient, private val valueReference: Int) : XValue() {
    override fun computePresentation(node: XValueNode, place: XValuePlace) {
        node.setPresentation(null, null, "", true)
    }

    override fun computeChildren(node: XCompositeNode) {
        addPage(client, "nsNext/referrers", 0, null, node) { addProperty("valueReference", valueReference) }
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
internal fun addVariablePage(client: DapClient, reference: Int, start: Int, total: Int?, node: XCompositeNode, locals: Boolean = false) {
    addPage(client, "variables", start, total, node, if (locals) reference else null) { addProperty("variablesReference", reference) }
}

/**
 * Adds to [node] a page from [start] of what [command] answers as
 * `variables` answers -- the variables of a reference, a class's
 * instances, a value's referrers -- with the arguments [naming] writes,
 * and offers the next page as [addVariablePage] does. [locals] is the
 * `Locals` reference the page's variables belong to, for a frame's.
 */
internal fun addPage(
    client: DapClient,
    command: String,
    start: Int,
    total: Int?,
    node: XCompositeNode,
    locals: Int? = null,
    naming: JsonObject.() -> Unit,
) {
    val arguments = JsonObject().apply {
        naming()
        addProperty("start", start)
        addProperty("count", VARIABLES_PAGE)
    }
    client.request(command, arguments).whenComplete { body, failure ->
        if (node.isObsolete) return@whenComplete
        if (failure != null) {
            node.setErrorMessage(messageOf(failure))
            return@whenComplete
        }
        val variables = variablesOf(body)
        val children = XValueChildrenList(variables.size)
        for (variable in variables) children.add(NewspeakValue(client, variable, locals = locals))
        val loaded = start + variables.size
        val more = variables.isNotEmpty() && if (total != null) loaded < total else variables.size == VARIABLES_PAGE
        node.addChildren(children, !more)
        if (more) {
            // -1: the platform's "more, how many unknown", for named children the agent does not count.
            node.tooManyChildren(total?.minus(loaded) ?: -1) { addPage(client, command, loaded, total, node, locals, naming) }
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

/** What failed a request, without the wrapper a future's continuation, or its `get`, sees. */
internal fun causeOf(failure: Throwable): Throwable =
    if (failure is CompletionException || failure is ExecutionException) failure.cause ?: failure else failure

/** The text of what failed a request. */
internal fun messageOf(failure: Throwable): String {
    val cause = causeOf(failure)
    return cause.message ?: cause.toString()
}
