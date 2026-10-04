package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.extensions.PluginAware
import com.intellij.openapi.extensions.PluginDescriptor
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClientManager
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
        descriptorFor(project)?.let(clientStarter::ensureClientStarted)
    }

    /**
     * Starts the client with no file open in an editor. The platform starts
     * it through `fileOpened` only, for a file opened in an editor, so a
     * Newspeak file merely selected in the project view would otherwise find
     * no server to ask for its structure. `LspClientManager.startClientsIfNeeded`
     * is no help there: it calls `fileOpened` for the files open in editors
     * only. Starting a client already running does nothing.
     */
    fun startClient(project: Project) {
        val descriptor = descriptorFor(project) ?: return
        LspClientManager.getInstance(project).ensureClientStarted(NewspeakLspIntegrationProvider::class.java, descriptor)
    }

    private fun descriptorFor(project: Project): NewspeakLspClientDescriptor? =
        when (val location = locateServer(pluginDescriptor?.pluginPath)) {
            is ServerLocation.Found -> NewspeakLspClientDescriptor(project, location.path)
            else -> {
                MissingServerNotification.showOnce(project, location)
                null
            }
        }

    companion object {
        /** The registered instance, which alone knows the plugin's directory (`setPluginDescriptor`). */
        fun registered(): NewspeakLspIntegrationProvider? =
            LspIntegrationProvider.EP_NAME.findExtension(NewspeakLspIntegrationProvider::class.java)
    }
}

fun locateServer(pluginDirectory: Path?): ServerLocation = ServerBinaryLocator(
    configuredPath = NewspeakSettings.getInstance().state.serverPath,
    pluginDirectory = pluginDirectory,
    operatingSystemName = System.getProperty("os.name"),
    operatingSystemArchitecture = System.getProperty("os.arch"),
).locate()
