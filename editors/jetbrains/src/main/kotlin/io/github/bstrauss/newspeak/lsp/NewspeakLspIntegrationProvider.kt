package io.github.bstrauss.newspeak.lsp

import com.intellij.ide.plugins.PluginManagerCore
import com.intellij.openapi.extensions.PluginId
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspIntegrationProvider
import com.intellij.platform.lsp.api.LspIntegrationProvider.LspClientStarter
import io.github.bstrauss.newspeak.NewspeakFileType
import io.github.bstrauss.newspeak.PLUGIN_ID
import io.github.bstrauss.newspeak.settings.NewspeakSettings

class NewspeakLspIntegrationProvider : LspIntegrationProvider {
    override fun fileOpened(project: Project, file: VirtualFile, clientStarter: LspClientStarter) {
        if (file.fileType != NewspeakFileType) return
        when (val location = locateServer()) {
            is ServerLocation.Found -> clientStarter.ensureClientStarted(NewspeakLspClientDescriptor(project, location.path))
            else -> MissingServerNotification.showOnce(project, location)
        }
    }
}

fun locateServer(): ServerLocation = ServerBinaryLocator(
    configuredPath = NewspeakSettings.getInstance().state.serverPath,
    pluginDirectory = PluginManagerCore.getPlugin(PluginId.getId(PLUGIN_ID))?.pluginPath,
    osName = System.getProperty("os.name"),
    osArch = System.getProperty("os.arch"),
).locate()
