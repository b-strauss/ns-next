package io.github.bstrauss.newspeak.debugger

import com.intellij.icons.AllIcons
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.InputValidator
import com.intellij.openapi.ui.Messages
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointProperties
import com.intellij.xdebugger.breakpoints.XBreakpointType
import com.intellij.xdebugger.breakpoints.ui.XBreakpointCustomPropertiesPanel
import io.github.bstrauss.newspeak.NewspeakBundle
import javax.swing.Icon
import javax.swing.JComponent

/**
 * A method breakpoint: every activation of the method stops at its entry
 * and at each of its exits (`docs/design/debugger.md`, "Method
 * breakpoints"). It names the method as the agent reads it,
 * `Module.Outer.Inner>>selector` or `Module.Inner class>>selector` for the
 * class side's; the agent refuses a name of another form with its reason,
 * and holds one whose module is not built yet until it is.
 */
class NewspeakMethodBreakpointType :
    XBreakpointType<XBreakpoint<NewspeakMethodBreakpointProperties>, NewspeakMethodBreakpointProperties>(
        ID,
        NewspeakBundle.message("debugger.breakpoint.method.title"),
    ) {
    override fun getDisplayText(breakpoint: XBreakpoint<NewspeakMethodBreakpointProperties>): String =
        breakpoint.properties.methodName.orEmpty()

    override fun getEnabledIcon(): Icon = AllIcons.Debugger.Db_method_breakpoint

    override fun getDisabledIcon(): Icon = AllIcons.Debugger.Db_disabled_method_breakpoint

    override fun createProperties(): NewspeakMethodBreakpointProperties = NewspeakMethodBreakpointProperties()

    override fun isAddBreakpointButtonVisible(): Boolean = true

    /** Asks for the method's name and adds a breakpoint for it. */
    override fun addBreakpoint(project: Project, parentComponent: JComponent?): XBreakpoint<NewspeakMethodBreakpointProperties>? {
        val name = Messages.showInputDialog(
            project,
            NewspeakBundle.message("debugger.breakpoint.method.dialog.message"),
            NewspeakBundle.message("debugger.breakpoint.method.dialog.title"),
            null,
            "",
            MethodNameValidator,
        )?.trim() ?: return null
        val properties = NewspeakMethodBreakpointProperties().apply { methodName = name }
        return WriteAction.compute<XBreakpoint<NewspeakMethodBreakpointProperties>, RuntimeException> {
            XDebuggerManager.getInstance(project).breakpointManager.addBreakpoint(this, properties)
        }
    }

    override fun createCustomPropertiesPanel(project: Project): XBreakpointCustomPropertiesPanel<XBreakpoint<NewspeakMethodBreakpointProperties>> =
        PassCountPanel()

    /** The dialog's own check: a class and a selector around `>>`. The agent decides the rest. */
    private object MethodNameValidator : InputValidator {
        override fun checkInput(inputString: String?): Boolean {
            val (className, selector) = inputString?.split(">>")?.takeIf { parts -> parts.size == 2 } ?: return false
            return className.isNotBlank() && selector.isNotBlank() && selector.trim().none(Char::isWhitespace)
        }

        override fun canClose(inputString: String?): Boolean = checkInput(inputString)
    }

    companion object {
        const val ID = "newspeak-method"
    }
}

/** A method breakpoint's method, in the agent's form, and its pass count. */
class NewspeakMethodBreakpointProperties : XBreakpointProperties<NewspeakMethodBreakpointProperties>(), PassCounted {
    var methodName: String? = null
    override var passCount: String? = null

    override fun getState(): NewspeakMethodBreakpointProperties = this

    override fun loadState(state: NewspeakMethodBreakpointProperties) {
        methodName = state.methodName
        passCount = state.passCount
    }
}
