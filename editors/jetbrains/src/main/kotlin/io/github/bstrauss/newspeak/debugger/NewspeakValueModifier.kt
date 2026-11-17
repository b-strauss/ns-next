package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.xdebugger.XExpression
import com.intellij.xdebugger.frame.XValueModifier
import io.github.bstrauss.newspeak.debugger.dap.DapClient

/**
 * Sets the variable [name] of the frame whose `Locals` scope [locals]
 * names to the value of an expression, evaluated in that frame
 * (`setVariable`). The agent refuses what the VM refuses -- a variable a
 * block captured before this point, one that is not assignable -- with
 * its reason, which the platform shows; once it is set, the platform
 * reads the frame's variables again.
 */
class NewspeakValueModifier(private val client: DapClient, private val locals: Int, private val name: String) : XValueModifier() {
    override fun setValue(expression: XExpression, callback: XModificationCallback) {
        val arguments = JsonObject().apply {
            addProperty("variablesReference", locals)
            addProperty("name", name)
            addProperty("value", expression.expression)
        }
        client.request("setVariable", arguments).whenComplete { _, failure ->
            if (failure != null) callback.errorOccurred(messageOf(failure)) else callback.valueModified()
        }
    }
}
