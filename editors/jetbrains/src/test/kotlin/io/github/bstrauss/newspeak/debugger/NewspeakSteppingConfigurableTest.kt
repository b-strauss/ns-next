package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.application.ApplicationManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.util.xmlb.XmlSerializer
import com.intellij.xdebugger.settings.DebuggerSettingsCategory

/** The step filters' settings: the Stepping section's configurable, what is stored, and the change every session hears. */
class NewspeakSteppingConfigurableTest : BasePlatformTestCase() {
    private lateinit var saved: StepFilters

    override fun setUp() {
        super.setUp()
        saved = NewspeakDebuggerSettings.getInstance().filters
    }

    override fun tearDown() {
        try {
            NewspeakDebuggerSettings.getInstance().update(saved)
        } finally {
            super.tearDown()
        }
    }

    fun testTheSteppingSectionHasNewspeaksConfigurable() {
        val settings = NewspeakDebuggerSettings.getInstance()
        assertEquals(listOf("Newspeak"), settings.createConfigurables(DebuggerSettingsCategory.STEPPING).map { configurable -> configurable.displayName })
        assertEquals(emptyList<Any>(), settings.createConfigurables(DebuggerSettingsCategory.GENERAL).toList())
    }

    fun testTheConfigurableShowsAppliesAndResetsTheFilters() {
        val settings = NewspeakDebuggerSettings.getInstance()
        settings.update(StepFilters(stepIntoShipped = false, skippedClasses = listOf("Walker.Inner")))
        val configurable = NewspeakSteppingConfigurable()
        configurable.createComponent()
        assertFalse(configurable.stepIntoShipped.isSelected)
        assertEquals(listOf("Walker.Inner"), configurable.skippedClasses.items)
        assertFalse(configurable.isModified)

        configurable.stepIntoShipped.isSelected = true
        configurable.skippedClasses.add("Collections.List")
        assertTrue(configurable.isModified)
        configurable.apply()
        assertEquals(StepFilters(stepIntoShipped = true, skippedClasses = listOf("Walker.Inner", "Collections.List")), settings.filters)
        assertFalse(configurable.isModified)

        configurable.stepIntoShipped.isSelected = false
        configurable.skippedClasses.remove(0)
        configurable.reset()
        assertTrue(configurable.stepIntoShipped.isSelected)
        assertEquals(listOf("Walker.Inner", "Collections.List"), configurable.skippedClasses.items)
        configurable.disposeUIResources()
    }

    /** What the platform stores of the settings is read back as the same filters, the default ones included. */
    fun testTheStoredStateIsReadBack() {
        val settings = NewspeakDebuggerSettings.getInstance()
        for (filters in listOf(StepFilters(true, listOf("Walker.Inner", "Collections")), StepFilters(false, emptyList()))) {
            settings.update(filters)
            val element = XmlSerializer.serialize(settings.state)
            settings.update(StepFilters(!filters.stepIntoShipped, listOf("Other")))
            settings.loadState(XmlSerializer.deserialize(element, NewspeakDebuggerSettings.State::class.java))
            assertEquals(filters, settings.filters)
        }
    }

    /** A change is told to every session; a change to the same filters to none. */
    fun testAChangeIsToldOnceAndNoChangeNotAtAll() {
        val settings = NewspeakDebuggerSettings.getInstance()
        settings.update(StepFilters(false, emptyList()))
        var told = 0
        ApplicationManager.getApplication().messageBus.connect(testRootDisposable)
            .subscribe(NewspeakDebuggerSettings.TOPIC, StepFiltersListener { told++ })
        settings.update(StepFilters(true, emptyList()))
        settings.update(StepFilters(true, emptyList()))
        assertEquals(1, told)
        assertEquals("""{"shipped":false,"classes":[]}""", settings.filters.arguments().toString())
        settings.update(StepFilters(false, listOf("Walker.Inner")))
        assertEquals(2, told)
        assertEquals("""{"shipped":true,"classes":["Walker.Inner"]}""", settings.filters.arguments().toString())
    }
}
