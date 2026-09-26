package io.github.bstrauss.newspeak.settings

import com.intellij.openapi.options.BoundConfigurable
import com.intellij.openapi.ui.DialogPanel
import com.intellij.ui.dsl.builder.AlignX
import com.intellij.ui.dsl.builder.bindText
import com.intellij.ui.dsl.builder.panel
import io.github.bstrauss.newspeak.lsp.restartNewspeakServers

class NewspeakSettingsConfigurable : BoundConfigurable("Newspeak") {
    override fun createPanel(): DialogPanel {
        val settings = NewspeakSettings.getInstance().state
        return panel {
            row("Language server:") {
                textField()
                    .bindText({ settings.serverPath.orEmpty() }, { text -> settings.serverPath = text.trim().ifEmpty { null } })
                    .align(AlignX.FILL)
                    .comment(
                        "Leave empty to use the ns-next-lsp bundled with the plugin. Point it at a locally built " +
                            "target/debug/ns-next-lsp to see changes to ns-next's parser or resolver at once."
                    )
            }
        }
    }

    override fun apply() {
        super.apply()
        restartNewspeakServers()
    }
}
