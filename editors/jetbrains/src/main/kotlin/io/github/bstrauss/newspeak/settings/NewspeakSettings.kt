package io.github.bstrauss.newspeak.settings

import com.intellij.openapi.components.BaseState
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.SimplePersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/** Application-level: the server path is specific to a machine and does not belong in `.idea/`. */
@Service(Service.Level.APP)
@State(name = "NewspeakSettings", storages = [Storage("newspeak.xml")])
class NewspeakSettings : SimplePersistentStateComponent<NewspeakSettings.Settings>(Settings()) {
    class Settings : BaseState() {
        /** A language server to start instead of the bundled one; null for the bundled one. */
        var serverPath by string()
    }

    companion object {
        fun getInstance(): NewspeakSettings = service()
    }
}
