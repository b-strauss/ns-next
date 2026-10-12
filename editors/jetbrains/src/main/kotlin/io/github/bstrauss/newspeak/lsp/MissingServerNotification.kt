package io.github.bstrauss.newspeak.lsp

import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.settings.NewspeakSettingsConfigurable
import java.util.concurrent.atomic.AtomicBoolean

/** One balloon per IDE session: every `.ns` file opened after it would say the same. */
object MissingServerNotification {
    private val shown = AtomicBoolean(false)

    fun showOnce(project: Project, location: ServerLocation) {
        if (!shown.compareAndSet(false, true)) return
        val content = when (location) {
            is ServerLocation.ConfiguredPathMissing ->
                NewspeakBundle.message("notification.server.missing.configured.path", location.path)
            is ServerLocation.NotBundledForPlatform ->
                NewspeakBundle.message(
                    "notification.server.missing.not.bundled",
                    location.platform ?: NewspeakBundle.message("notification.server.missing.not.bundled.this.platform"),
                )
            is ServerLocation.PluginDirectoryUnknown ->
                NewspeakBundle.message("notification.server.missing.plugin.directory")
            is ServerLocation.Found -> return
        }
        NotificationGroupManager.getInstance()
            .getNotificationGroup("Newspeak")
            .createNotification(NewspeakBundle.message("notification.server.missing.title"), content, NotificationType.WARNING)
            .addAction(NotificationAction.createSimpleExpiring(NewspeakBundle.message("notification.server.configure")) {
                ShowSettingsUtil.getInstance().showSettingsDialog(project, NewspeakSettingsConfigurable::class.java)
            })
            .notify(project)
    }

    /** After the setting changes, a new problem deserves a new balloon. */
    fun reset() = shown.set(false)
}
