package io.github.bstrauss.newspeak.settings

import com.intellij.openapi.fileChooser.FileChooserDescriptorFactory
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
            row("ns-next-analyzer path:") {
                textFieldWithBrowseButton(
                    FileChooserDescriptorFactory.singleFile().withTitle("Choose the ns-next-analyzer Executable"),
                )
                    .bindText({ settings.serverPath.orEmpty() }, { text -> settings.serverPath = text.trim().ifEmpty { null } })
                    .align(AlignX.FILL)
                    .comment(
                        "Leave empty to use the ns-next-analyzer bundled with the plugin. Point it at a locally built " +
                            "target/debug/ns-next-analyzer to see changes to ns-next's parser or resolver at once."
                    )
            }
            row("ns-next path:") {
                textFieldWithBrowseButton(
                    FileChooserDescriptorFactory.singleFile().withTitle("Choose the ns-next Executable"),
                )
                    .bindText({ settings.runtimePath.orEmpty() }, { text -> settings.runtimePath = text.trim().ifEmpty { null } })
                    .align(AlignX.FILL)
                    .comment(
                        "The ns-next runtime that Newspeak run configurations start; build it with " +
                            "cargo build -p ns-cli --release. It is not bundled with the plugin."
                    )
            }
        }
    }

    /** Restarts the language servers for a changed analyzer path; a changed ns-next path needs nothing, since every run reads it afresh. */
    override fun apply() {
        super.apply()
        restartNewspeakServers()
    }
}
