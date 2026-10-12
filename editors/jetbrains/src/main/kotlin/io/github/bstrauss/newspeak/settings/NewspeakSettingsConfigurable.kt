package io.github.bstrauss.newspeak.settings

import com.intellij.openapi.fileChooser.FileChooserDescriptorFactory
import com.intellij.openapi.options.BoundConfigurable
import com.intellij.openapi.ui.DialogPanel
import com.intellij.ui.dsl.builder.AlignX
import com.intellij.ui.dsl.builder.bindText
import com.intellij.ui.dsl.builder.panel
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.lsp.restartNewspeakServers

class NewspeakSettingsConfigurable : BoundConfigurable(NewspeakBundle.message("settings.display.name")) {
    override fun createPanel(): DialogPanel {
        val settings = NewspeakSettings.getInstance().state
        return panel {
            row(NewspeakBundle.message("settings.server.path.label")) {
                textFieldWithBrowseButton(
                    FileChooserDescriptorFactory.singleFile().withTitle(NewspeakBundle.message("settings.server.path.chooser.title")),
                )
                    .bindText({ settings.serverPath.orEmpty() }, { text -> settings.serverPath = text.trim().ifEmpty { null } })
                    .align(AlignX.FILL)
                    .comment(NewspeakBundle.message("settings.server.path.comment"))
            }
            row(NewspeakBundle.message("settings.runtime.path.label")) {
                textFieldWithBrowseButton(
                    FileChooserDescriptorFactory.singleFile().withTitle(NewspeakBundle.message("settings.runtime.path.chooser.title")),
                )
                    .bindText({ settings.runtimePath.orEmpty() }, { text -> settings.runtimePath = text.trim().ifEmpty { null } })
                    .align(AlignX.FILL)
                    .comment(NewspeakBundle.message("settings.runtime.path.comment"))
            }
        }
    }

    /** Restarts the language servers for a changed analyzer path; a changed ns-next path needs nothing, since every run reads it afresh. */
    override fun apply() {
        super.apply()
        restartNewspeakServers()
    }
}
