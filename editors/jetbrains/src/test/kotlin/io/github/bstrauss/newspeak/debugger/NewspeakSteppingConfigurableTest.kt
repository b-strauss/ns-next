package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.application.ApplicationManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.util.xmlb.XmlSerializer
import com.intellij.xdebugger.settings.DebuggerSettingsCategory

/** The shipped switch's settings: the Stepping section's configurable, what is stored, and the change every session hears. */
class NewspeakSteppingConfigurableTest : BasePlatformTestCase() {
    private var saved = false

    override fun setUp() {
        super.setUp()
        saved = NewspeakDebuggerSettings.getInstance().stepIntoShipped
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

    fun testTheConfigurableShowsAppliesAndResetsTheSwitch() {
        val settings = NewspeakDebuggerSettings.getInstance()
        settings.update(false)
        val configurable = NewspeakSteppingConfigurable()
        configurable.createComponent()
        assertFalse(configurable.stepIntoShipped.isSelected)
        assertFalse(configurable.isModified)

        configurable.stepIntoShipped.isSelected = true
        assertTrue(configurable.isModified)
        configurable.apply()
        assertTrue(settings.stepIntoShipped)
        assertFalse(configurable.isModified)

        configurable.stepIntoShipped.isSelected = false
        configurable.reset()
        assertTrue(configurable.stepIntoShipped.isSelected)
        configurable.disposeUIResources()
    }

    /** What the platform stores of the settings is read back as the same switch. */
    fun testTheStoredStateIsReadBack() {
        val settings = NewspeakDebuggerSettings.getInstance()
        for (switch in listOf(true, false)) {
            settings.update(switch)
            val element = XmlSerializer.serialize(settings.state)
            settings.update(!switch)
            settings.loadState(XmlSerializer.deserialize(element, NewspeakDebuggerSettings.State::class.java))
            assertEquals(switch, settings.stepIntoShipped)
        }
    }

    /** A change is told to every session; a change to the same switch to none. */
    fun testAChangeIsToldOnceAndNoChangeNotAtAll() {
        val settings = NewspeakDebuggerSettings.getInstance()
        settings.update(false)
        var told = 0
        ApplicationManager.getApplication().messageBus.connect(testRootDisposable)
            .subscribe(NewspeakDebuggerSettings.TOPIC, StepFiltersListener { told++ })
        settings.update(true)
        settings.update(true)
        assertEquals(1, told)
        assertEquals("""{"shipped":false,"entries":[]}""", NewspeakDebuggerSettings.stepFilterArguments(project).toString())
        settings.update(false)
        assertEquals(2, told)
        assertEquals("""{"shipped":true,"entries":[]}""", NewspeakDebuggerSettings.stepFilterArguments(project).toString())
    }
}
