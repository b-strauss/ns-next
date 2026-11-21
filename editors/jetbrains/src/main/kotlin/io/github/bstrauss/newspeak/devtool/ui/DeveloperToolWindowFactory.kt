package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory
import com.intellij.openapi.wm.ex.ToolWindowEx
import io.github.bstrauss.newspeak.NewspeakBundle

/** The tool window's id, as `plugin.xml` registers it. */
const val TOOL_WINDOW_ID = "Newspeak JIT"

/**
 * The "Newspeak JIT" tool window (`docs/design/developer-tool.md`, decision
 * 26): a tab per traced run, which [io.github.bstrauss.newspeak.devtool.TracedRuns]
 * adds and removes, so that a run started before the window was first
 * opened still gets its tab.
 */
class DeveloperToolWindowFactory : ToolWindowFactory, DumbAware {
    override fun init(toolWindow: ToolWindow) {
        toolWindow.stripeTitle = NewspeakBundle.message("devtool.tool.window.title")
    }

    override fun createToolWindowContent(project: Project, toolWindow: ToolWindow) {
        (toolWindow as? ToolWindowEx)?.emptyText?.text = NewspeakBundle.message("devtool.tool.window.empty")
    }
}
