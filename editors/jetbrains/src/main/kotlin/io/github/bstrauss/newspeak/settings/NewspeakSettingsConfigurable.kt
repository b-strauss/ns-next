package io.github.bstrauss.newspeak.settings

import com.intellij.openapi.fileChooser.FileChooserDescriptorFactory
import com.intellij.openapi.options.BoundConfigurable
import com.intellij.openapi.ui.DialogPanel
import com.intellij.ui.dsl.builder.AlignX
import com.intellij.ui.dsl.builder.bindSelected
import com.intellij.ui.dsl.builder.bindText
import com.intellij.openapi.components.serviceIfCreated
import io.github.bstrauss.newspeak.devtool.editor.DecorationsService
import io.github.bstrauss.newspeak.devtool.editor.DeveloperToolDecorationsSettings
import com.intellij.ui.dsl.builder.panel
import com.intellij.openapi.project.ProjectManager
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
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
            val decorations = DeveloperToolDecorationsSettings.getInstance().state
            group(NewspeakBundle.message("settings.devtool.group")) {
                row {
                    checkBox(NewspeakBundle.message("settings.devtool.state.icons"))
                        .bindSelected({ decorations.showStateIcons }, { selected -> decorations.showStateIcons = selected })
                }
                row {
                    checkBox(NewspeakBundle.message("settings.devtool.send.hints"))
                        .bindSelected({ decorations.showSendHints }, { selected -> decorations.showSendHints = selected })
                }
            }
        }
    }

    /**
     * Restarts the language servers for a changed analyzer path, and looks
     * at the ns-next the settings name for each open project's core
     * library; a run needs nothing, since every run reads the path afresh.
     * The editors' decorations follow the two switches at once.
     */
    override fun apply() {
        super.apply()
        restartNewspeakServers()
        ProjectManager.getInstance().openProjects.forEach { project ->
            NewspeakCoreLibrary.getInstance(project).decideAndRefresh()
            project.serviceIfCreated<DecorationsService>()?.settingsChanged()
        }
    }
}
