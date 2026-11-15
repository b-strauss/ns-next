package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonObject
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.project.Project
import com.intellij.util.messages.Topic
import com.intellij.xdebugger.XDebuggerUtil
import com.intellij.xdebugger.settings.DebuggerSettingsCategory
import com.intellij.xdebugger.settings.XDebuggerSettings

/**
 * Whether a step goes into the shipped modules' code, the kernel's and the
 * platform's, which by default it passes (`docs/design/debugger.md`,
 * decision 16): application-wide, as Java's stepping switches are, kept
 * with the platform's debugger settings, edited in Newspeak's section of
 * Settings | Build, Execution, Deployment | Debugger | Stepping
 * ([NewspeakSteppingConfigurable]) and by the debug tool window's toggle
 * ([NewspeakStepIntoShippedAction]). The classes not to step into are the
 * project's ([NewspeakSkippedClasses]). Every change of either is told on
 * [TOPIC], which each running session hears and sends to its agent.
 */
class NewspeakDebuggerSettings : XDebuggerSettings<NewspeakDebuggerSettings.State>(ID) {
    /** The settings as stored. */
    class State {
        var stepIntoShipped: Boolean = false
    }

    /** Whether a step goes into the shipped modules' code. */
    @Volatile
    var stepIntoShipped: Boolean = false
        private set

    override fun getState(): State = State().apply { stepIntoShipped = this@NewspeakDebuggerSettings.stepIntoShipped }

    override fun loadState(state: State) {
        stepIntoShipped = state.stepIntoShipped
    }

    override fun createConfigurables(category: DebuggerSettingsCategory): Collection<Configurable> =
        if (category == DebuggerSettingsCategory.STEPPING) listOf(NewspeakSteppingConfigurable()) else emptyList()

    /** Changes the switch and tells every running session; a change to nothing tells nobody. */
    fun update(changed: Boolean) {
        if (changed == stepIntoShipped) return
        stepIntoShipped = changed
        ApplicationManager.getApplication().messageBus.syncPublisher(TOPIC).stepFiltersChanged()
    }

    companion object {
        const val ID = "newspeak"

        /** Told whenever the switch or a project's classes change. */
        @Topic.AppLevel
        val TOPIC: Topic<StepFiltersListener> = Topic.create("Newspeak step filters", StepFiltersListener::class.java)

        fun getInstance(): NewspeakDebuggerSettings = XDebuggerUtil.getInstance().getDebuggerSettings(NewspeakDebuggerSettings::class.java)

        /**
         * `nsNext/setStepFilters`'s arguments for a session of [project]:
         * `shipped` whether the shipped modules are filtered, `entries`
         * the project's classes not to step into.
         */
        fun stepFilterArguments(project: Project): JsonObject = JsonObject().apply {
            addProperty("shipped", !getInstance().stepIntoShipped)
            add("entries", NewspeakSkippedClasses.getInstance(project).arguments())
        }
    }
}

/** Hears a change of the step filters. */
fun interface StepFiltersListener {
    fun stepFiltersChanged()
}
