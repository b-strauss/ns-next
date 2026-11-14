package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.ui.LabeledComponent
import com.intellij.ui.components.JBTextField
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointProperties
import com.intellij.xdebugger.breakpoints.ui.XBreakpointCustomPropertiesPanel
import io.github.bstrauss.newspeak.NewspeakBundle
import java.awt.BorderLayout
import javax.swing.JComponent
import javax.swing.JPanel

/** A breakpoint whose stops the VM counts, as IntelliJ's pass count and DAP's `hitCondition` mean it. */
interface PassCounted {
    /**
     * The hit condition in the agent's forms -- `n`, `=n`, `==n`, `>n`,
     * `>=n`, `%n` -- or null or blank for a stop at every hit; a malformed
     * one the agent answers unverified, with its reason
     * (`docs/design/debugger.md`, "Hit counts and temporary breakpoints").
     */
    var passCount: String?
}

/** The hit condition to send, or null for none. */
val PassCounted.hitCondition: String? get() = passCount?.trim()?.takeIf(String::isNotEmpty)

/**
 * A line breakpoint's settings: its pass count, and for a block variant
 * the column its block's first statement starts at, counted from 0 in
 * UTF-16 code units -- null for the line itself.
 */
class NewspeakLineBreakpointProperties : XBreakpointProperties<NewspeakLineBreakpointProperties>(), PassCounted {
    override var passCount: String? = null
    var column: Int? = null

    override fun getState(): NewspeakLineBreakpointProperties = this

    override fun loadState(state: NewspeakLineBreakpointProperties) {
        passCount = state.passCount
        column = state.column
    }
}

/** The field that edits a breakpoint's pass count, in its properties and in the popup at its icon. */
class PassCountPanel<P, B : XBreakpoint<P>> : XBreakpointCustomPropertiesPanel<B>()
    where P : XBreakpointProperties<*>, P : PassCounted {
    private val field = JBTextField().apply {
        emptyText.text = NewspeakBundle.message("debugger.breakpoint.pass.count.empty")
        toolTipText = NewspeakBundle.message("debugger.breakpoint.pass.count.tooltip")
    }
    private val panel = JPanel(BorderLayout()).apply {
        add(LabeledComponent.create(field, NewspeakBundle.message("debugger.breakpoint.pass.count"), BorderLayout.WEST), BorderLayout.CENTER)
    }

    override fun getComponent(): JComponent = panel

    override fun loadFrom(breakpoint: B) {
        field.text = breakpoint.properties.passCount.orEmpty()
    }

    override fun saveTo(breakpoint: B) {
        val properties = breakpoint.properties
        val passCount = field.text.trim().takeIf(String::isNotEmpty)
        if (properties.passCount == passCount) return
        properties.passCount = passCount
        reportChanged(breakpoint)
    }
}

/**
 * Tells a running session that [breakpoint]'s properties changed, so that
 * its handler sends its set again. The platform reports a change only
 * through its own setters, and the one way to report a plugin's property
 * -- `XBreakpointBase.fireBreakpointChanged` -- is internal API; turning
 * an enabled breakpoint off and on again goes through the public setter,
 * whose change events unregister and register it with the session.
 */
fun reportChanged(breakpoint: XBreakpoint<*>) {
    if (!breakpoint.isEnabled) return
    breakpoint.isEnabled = false
    breakpoint.isEnabled = true
}
