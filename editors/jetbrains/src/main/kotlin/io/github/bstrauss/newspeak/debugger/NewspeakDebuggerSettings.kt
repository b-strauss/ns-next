package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonArray
import com.google.gson.JsonObject
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.options.Configurable
import com.intellij.util.messages.Topic
import com.intellij.util.xmlb.annotations.XCollection
import com.intellij.xdebugger.XDebuggerUtil
import com.intellij.xdebugger.settings.DebuggerSettingsCategory
import com.intellij.xdebugger.settings.XDebuggerSettings

/**
 * The step filters every Newspeak debug session starts with and is held to
 * (`docs/design/debugger.md`, decision 16): whether a step goes into the
 * shipped modules' code, the kernel's and the platform's, which by default
 * it passes, and the classes, each `Module.Path.To.Class`, whose code it
 * passes too, as Java's "Do not step into the classes". Kept with the
 * platform's debugger settings, edited under Settings | Build, Execution,
 * Deployment | Debugger | Stepping ([NewspeakSteppingConfigurable]) and,
 * for the shipped modules, by the debug tool window's toggle
 * ([NewspeakStepIntoShippedAction]); every change is told on [TOPIC], which
 * each running session hears and sends to its agent.
 */
class NewspeakDebuggerSettings : XDebuggerSettings<NewspeakDebuggerSettings.State>(ID) {
    /** The settings as stored. */
    class State {
        var stepIntoShipped: Boolean = false

        @get:XCollection(style = XCollection.Style.v2)
        var skippedClasses: MutableList<String> = ArrayList()
    }

    /** The filters as they stand, replaced whole, so that a session's thread reads one consistent pair. */
    @Volatile
    var filters: StepFilters = StepFilters(stepIntoShipped = false, skippedClasses = emptyList())
        private set

    override fun getState(): State = State().apply {
        stepIntoShipped = filters.stepIntoShipped
        skippedClasses = filters.skippedClasses.toMutableList()
    }

    override fun loadState(state: State) {
        filters = StepFilters(state.stepIntoShipped, state.skippedClasses.toList())
    }

    override fun createConfigurables(category: DebuggerSettingsCategory): Collection<Configurable> =
        if (category == DebuggerSettingsCategory.STEPPING) listOf(NewspeakSteppingConfigurable()) else emptyList()

    /** Changes the filters and tells every running session; a change to nothing tells nobody. */
    fun update(changed: StepFilters) {
        if (changed == filters) return
        filters = changed
        ApplicationManager.getApplication().messageBus.syncPublisher(TOPIC).stepFiltersChanged()
    }

    companion object {
        const val ID = "newspeak"

        /** Told whenever the filters change. */
        @Topic.AppLevel
        val TOPIC: Topic<StepFiltersListener> = Topic.create("Newspeak step filters", StepFiltersListener::class.java)

        fun getInstance(): NewspeakDebuggerSettings = XDebuggerUtil.getInstance().getDebuggerSettings(NewspeakDebuggerSettings::class.java)
    }
}

/** Whether a step goes into the shipped modules' code, and the classes whose code it passes. */
data class StepFilters(val stepIntoShipped: Boolean, val skippedClasses: List<String>) {
    /**
     * `nsNext/setStepFilters`'s arguments: `shipped` whether the shipped
     * modules are filtered, `classes` the names the agent parses, each
     * `Module.Path.To.Class`.
     */
    fun arguments(): JsonObject = JsonObject().apply {
        addProperty("shipped", !stepIntoShipped)
        add("classes", JsonArray().apply { skippedClasses.forEach(::add) })
    }
}

/** Hears a change of the step filters. */
fun interface StepFiltersListener {
    fun stepFiltersChanged()
}
