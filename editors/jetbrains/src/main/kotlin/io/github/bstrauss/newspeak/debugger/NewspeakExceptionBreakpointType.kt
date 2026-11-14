package io.github.bstrauss.newspeak.debugger

import com.intellij.icons.AllIcons
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.InputValidator
import com.intellij.openapi.ui.Messages
import com.intellij.ui.components.JBCheckBox
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointProperties
import com.intellij.xdebugger.breakpoints.XBreakpointType
import com.intellij.xdebugger.breakpoints.ui.XBreakpointCustomPropertiesPanel
import io.github.bstrauss.newspeak.NewspeakBundle
import javax.swing.BoxLayout
import javax.swing.Icon
import javax.swing.JComponent
import javax.swing.JPanel

/**
 * An exception breakpoint: the default "Any exception", which stops at an
 * uncaught exception, at a caught one where its handler is about to run,
 * and at a `halt`, as each of its boxes says; and one per exception class,
 * `Module.Path.To.Class`, which stops at that class's exceptions and its
 * subclasses' (`docs/design/debugger.md`, "Exceptions").
 */
class NewspeakExceptionBreakpointType :
    XBreakpointType<XBreakpoint<NewspeakExceptionBreakpointProperties>, NewspeakExceptionBreakpointProperties>(
        ID,
        NewspeakBundle.message("debugger.breakpoint.exception.title"),
    ) {
    override fun getDisplayText(breakpoint: XBreakpoint<NewspeakExceptionBreakpointProperties>): String =
        breakpoint.properties.className ?: NewspeakBundle.message("debugger.breakpoint.exception.any")

    override fun getEnabledIcon(): Icon = AllIcons.Debugger.Db_exception_breakpoint

    override fun getDisabledIcon(): Icon = AllIcons.Debugger.Db_disabled_exception_breakpoint

    override fun createProperties(): NewspeakExceptionBreakpointProperties = NewspeakExceptionBreakpointProperties()

    /**
     * "Any exception", which every project has, on: an uncaught exception
     * and a `halt` stop by default, as the agent's own defaults have them
     * (`docs/design/debugger.md`, "Exceptions"). The platform makes a
     * default breakpoint off unless its type turns it on.
     */
    override fun createDefaultBreakpoint(
        creator: XBreakpointCreator<NewspeakExceptionBreakpointProperties>,
    ): XBreakpoint<NewspeakExceptionBreakpointProperties> =
        creator.createBreakpoint(NewspeakExceptionBreakpointProperties()).apply { isEnabled = true }

    override fun isAddBreakpointButtonVisible(): Boolean = true

    /** Asks for the class's name and adds a breakpoint for it. */
    override fun addBreakpoint(project: Project, parentComponent: JComponent?): XBreakpoint<NewspeakExceptionBreakpointProperties>? {
        val name = Messages.showInputDialog(
            project,
            NewspeakBundle.message("debugger.breakpoint.exception.dialog.message"),
            NewspeakBundle.message("debugger.breakpoint.exception.dialog.title"),
            null,
            "",
            ClassNameValidator,
        )?.trim() ?: return null
        val properties = NewspeakExceptionBreakpointProperties().apply { className = name }
        return WriteAction.compute<XBreakpoint<NewspeakExceptionBreakpointProperties>, RuntimeException> {
            XDebuggerManager.getInstance(project).breakpointManager.addBreakpoint(this, properties)
        }
    }

    override fun createCustomPropertiesPanel(
        project: Project,
    ): XBreakpointCustomPropertiesPanel<XBreakpoint<NewspeakExceptionBreakpointProperties>> = ExceptionFiltersPanel()

    /** A dotted name without blanks: the agent decides the rest. */
    private object ClassNameValidator : InputValidator {
        override fun checkInput(inputString: String?): Boolean {
            val name = inputString?.trim() ?: return false
            return name.isNotEmpty() && name.none(Char::isWhitespace) && name.split('.').none(String::isEmpty)
        }

        override fun canClose(inputString: String?): Boolean = checkInput(inputString)
    }

    /** The boxes of the filters a breakpoint turns on; `halt` only for "Any exception", since a `Halt` is no class a user names. */
    private class ExceptionFiltersPanel : XBreakpointCustomPropertiesPanel<XBreakpoint<NewspeakExceptionBreakpointProperties>>() {
        private val uncaught = JBCheckBox(NewspeakBundle.message("debugger.breakpoint.exception.uncaught"))
        private val caught = JBCheckBox(NewspeakBundle.message("debugger.breakpoint.exception.caught"))
        private val halt = JBCheckBox(NewspeakBundle.message("debugger.breakpoint.exception.halt"))
        private val panel = JPanel().apply {
            layout = BoxLayout(this, BoxLayout.Y_AXIS)
            add(uncaught)
            add(caught)
            add(halt)
        }

        override fun getComponent(): JComponent = panel

        override fun loadFrom(breakpoint: XBreakpoint<NewspeakExceptionBreakpointProperties>) {
            val properties = breakpoint.properties
            uncaught.isSelected = properties.uncaught
            caught.isSelected = properties.caught
            halt.isSelected = properties.halt
            halt.isVisible = properties.className == null
        }

        override fun saveTo(breakpoint: XBreakpoint<NewspeakExceptionBreakpointProperties>) {
            val properties = breakpoint.properties
            val changed = properties.uncaught != uncaught.isSelected ||
                properties.caught != caught.isSelected ||
                (properties.className == null && properties.halt != halt.isSelected)
            if (!changed) return
            properties.uncaught = uncaught.isSelected
            properties.caught = caught.isSelected
            if (properties.className == null) properties.halt = halt.isSelected
            reportChanged(breakpoint)
        }
    }

    companion object {
        const val ID = "newspeak-exception"
    }
}

/**
 * An exception breakpoint's class -- null for "Any exception" -- and the
 * filters it turns on: [uncaught] exceptions, [caught] ones, and, for
 * "Any exception" only, [halt].
 */
class NewspeakExceptionBreakpointProperties : XBreakpointProperties<NewspeakExceptionBreakpointProperties>() {
    var className: String? = null
    var uncaught: Boolean = true
    var caught: Boolean = false
    var halt: Boolean = true

    override fun getState(): NewspeakExceptionBreakpointProperties = this

    override fun loadState(state: NewspeakExceptionBreakpointProperties) {
        className = state.className
        uncaught = state.uncaught
        caught = state.caught
        halt = state.halt
    }
}
