package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.project.DumbAwareAction
import com.intellij.openapi.project.ProjectManager
import com.intellij.platform.lsp.api.LspClientManager

/** Restarts the server in every open project, after the path setting changed or by hand. */
fun restartNewspeakServers() {
    MissingServerNotification.reset()
    for (project in ProjectManager.getInstance().openProjects) {
        LspClientManager.getInstance(project).stopAndRestartClientsIfNeeded(NewspeakLspIntegrationProvider::class.java)
    }
}

class RestartServerAction : DumbAwareAction() {
    override fun actionPerformed(event: AnActionEvent) = restartNewspeakServers()
}
