package io.github.bstrauss.newspeak.lsp

import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import io.github.bstrauss.newspeak.settings.NewspeakSettingsConfigurable
import java.util.concurrent.atomic.AtomicBoolean

/** One balloon per IDE session: every `.ns` file opened after it would say the same. */
object MissingServerNotification {
    private val shown = AtomicBoolean(false)

    fun showOnce(project: Project, location: ServerLocation) {
        if (!shown.compareAndSet(false, true)) return
        val content = when (location) {
            is ServerLocation.ConfiguredPathMissing ->
                "The configured language server does not exist: ${location.path}"
            is ServerLocation.NotBundledForPlatform ->
                "No ns-next-lsp is bundled for ${location.platform ?: "this platform"}. Build it with " +
                    "<code>cargo build -p ns-lsp --release</code> and set its path."
            is ServerLocation.Found -> return
        }
        NotificationGroupManager.getInstance()
            .getNotificationGroup("Newspeak")
            .createNotification("Newspeak: errors and semantic colours are off", content, NotificationType.WARNING)
            .addAction(NotificationAction.createSimpleExpiring("Configure…") {
                ShowSettingsUtil.getInstance().showSettingsDialog(project, NewspeakSettingsConfigurable::class.java)
            })
            .notify(project)
    }

    /** After the setting changes, a new problem deserves a new balloon. */
    fun reset() = shown.set(false)
}
