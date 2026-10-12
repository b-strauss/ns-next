package io.github.bstrauss.newspeak.settings

import com.intellij.openapi.components.BaseState
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.SimplePersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/**
 * Application-level: the server and runtime paths are specific to a machine
 * and do not belong in `.idea/`, and how code is folded is the user's taste,
 * not the project's.
 */
@Service(Service.Level.APP)
@State(name = "NewspeakSettings", storages = [Storage("newspeak.xml")])
class NewspeakSettings : SimplePersistentStateComponent<NewspeakSettings.Settings>(Settings()) {
    class Settings : BaseState() {
        /** A language server to start instead of the bundled one; null for the bundled one. */
        var serverPath by string()

        /** The ns-next runtime a Newspeak run configuration starts; null until the user chooses one. */
        var runtimePath by string()

        /** Whether a one-line method opens folded onto its header's line (`NewspeakOneLineMethodFolding`). */
        var collapseOneLineMethods by property(true)
    }

    companion object {
        fun getInstance(): NewspeakSettings = service()
    }
}
