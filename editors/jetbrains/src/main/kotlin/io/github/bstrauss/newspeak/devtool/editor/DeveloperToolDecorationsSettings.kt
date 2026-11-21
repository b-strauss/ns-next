package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.openapi.components.BaseState
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.SimplePersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/**
 * Whether the editor shows a traced run's state icons and its hints at the
 * sends (`docs/design/developer-tool.md`, decision 27: both can be switched
 * off, as other inlay hints can). The user's taste, so application-level,
 * as the plugin's other settings are.
 */
@Service(Service.Level.APP)
@State(name = "NewspeakDeveloperToolDecorations", storages = [Storage("newspeak.xml")])
class DeveloperToolDecorationsSettings : SimplePersistentStateComponent<DeveloperToolDecorationsSettings.Settings>(Settings()) {
    class Settings : BaseState() {
        /** Whether a traced method shows its state in the gutter. */
        var showStateIcons by property(true)

        /** Whether a traced send shows what became of it after its selector. */
        var showSendHints by property(true)
    }

    companion object {
        fun getInstance(): DeveloperToolDecorationsSettings = service()
    }
}
