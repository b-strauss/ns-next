package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.extensions.PluginAware
import com.intellij.openapi.extensions.PluginDescriptor
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspIntegrationProvider
import com.intellij.platform.lsp.api.LspIntegrationProvider.LspClientStarter
import io.github.bstrauss.newspeak.NewspeakFileType
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import java.nio.file.Path

/**
 * `PluginAware` is the platform's own way to hand an extension its own plugin
 * descriptor: `XmlExtensionAdapter` calls `setPluginDescriptor` right after
 * constructing an instance that implements it, with no lookup on the
 * extension's own part. `PluginManagerCore.getPlugin(PluginId)` would answer
 * the same `pluginPath`, but every overload of that lookup -- on
 * `PluginManagerCore`, on `PluginManager` and on
 * `PluginAwareClassLoader.getPluginDescriptor` -- is `@ApiStatus.Internal` in
 * the platform this plugin targets, so `verifyPlugin` refuses it.
 */
class NewspeakLspIntegrationProvider : LspIntegrationProvider, PluginAware {
    private var pluginDescriptor: PluginDescriptor? = null

    override fun setPluginDescriptor(pluginDescriptor: PluginDescriptor) {
        this.pluginDescriptor = pluginDescriptor
    }

    override fun fileOpened(project: Project, file: VirtualFile, clientStarter: LspClientStarter) {
        if (file.fileType != NewspeakFileType) return
        when (val location = locateServer(pluginDescriptor?.pluginPath)) {
            is ServerLocation.Found -> clientStarter.ensureClientStarted(NewspeakLspClientDescriptor(project, location.path))
            else -> MissingServerNotification.showOnce(project, location)
        }
    }
}

fun locateServer(pluginDirectory: Path?): ServerLocation = ServerBinaryLocator(
    configuredPath = NewspeakSettings.getInstance().state.serverPath,
    pluginDirectory = pluginDirectory,
    osName = System.getProperty("os.name"),
    osArch = System.getProperty("os.arch"),
).locate()
