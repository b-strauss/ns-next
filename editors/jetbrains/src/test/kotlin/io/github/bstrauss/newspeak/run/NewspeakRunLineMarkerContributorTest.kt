package io.github.bstrauss.newspeak.run

import com.intellij.codeInsight.daemon.LineMarkerInfo
import com.intellij.execution.PsiLocation
import com.intellij.execution.RunManager
import com.intellij.execution.actions.ConfigurationContext
import com.intellij.execution.actions.RunConfigurationProducer
import com.intellij.icons.AllIcons
import com.intellij.openapi.actionSystem.ActionGroup
import com.intellij.openapi.actionSystem.ActionWithDelegate
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.application.PathManager
import com.intellij.openapi.vfs.newvfs.impl.VfsRootAccess
import com.intellij.psi.PsiElement
import com.intellij.testFramework.fixtures.BasePlatformTestCase

class NewspeakRunLineMarkerContributorTest : BasePlatformTestCase() {
    override fun setUp() {
        super.setUp()
        // The schema provider of the same descriptor reads the schema from the
        // plugin's jar in the test sandbox once a newspeak.json is open, a root
        // the test framework's VFS guard does not allow by default.
        val pluginJar = checkNotNull(PathManager.getJarPathForClass(NewspeakRunAction::class.java)) {
            "no jar or directory holds NewspeakRunAction"
        }
        VfsRootAccess.allowRootAccess(testRootDisposable, pluginJar)
    }

    private val projectJson = """
        {
          "main": "App.ns",
          "modules": ["Greeter.ns"],
          "scripts": { "timed": ["--timed"], "sieve": ["Sieve"] },
          "other": { "main": "Nested.ns" }
        }
    """.trimIndent()

    /** The play buttons' renderers; the platform merges the markers of one line into one renderer. */
    private fun playButtons(): List<LineMarkerInfo.LineMarkerGutterIconRenderer<*>> = myFixture.findAllGutters()
        .filterIsInstance<LineMarkerInfo.LineMarkerGutterIconRenderer<*>>()
        .filter { renderer -> renderer.icon == AllIcons.RunConfigurations.TestState.Run }

    /** A menu's actions, unwrapped from the platform's line-marker wrappers and groups. */
    private fun menuActions(action: AnAction?): List<AnAction> = when (action) {
        null -> emptyList()
        is ActionWithDelegate<*> -> listOfNotNull(action.delegate as? AnAction)
        is ActionGroup -> action.getChildren(null).flatMap(::menuActions)
        else -> listOf(action)
    }

    /**
     * Per 0-based line, what this plugin's play buttons there run, one
     * `script:command` per menu entry. Another plugin's buttons (npm's, in a
     * package.json) carry the same icon and are left out.
     */
    private fun newspeakEntriesByLine(fileName: String, text: String): Map<Int, List<String>> {
        myFixture.configureByText(fileName, text)
        val document = myFixture.editor.document
        return playButtons()
            .associate { renderer ->
                val entries = menuActions(renderer.popupMenuActions)
                    .filterIsInstance<NewspeakRunAction>()
                    .map { action -> "${action.script}:${action.command}" }
                document.getLineNumber(renderer.lineMarkerInfo.startOffset) to entries
            }
            .filterValues(List<String>::isNotEmpty)
    }

    fun testMainAndEachScriptCarryAPlayButtonAndNothingElse() {
        assertEquals(
            mapOf(
                1 to listOf("null:RUN", "null:BENCH"),
                3 to listOf("timed:RUN", "timed:BENCH", "sieve:RUN", "sieve:BENCH"),
            ),
            newspeakEntriesByLine("newspeak.json", projectJson),
        )
    }

    fun testAnotherJsonFileCarriesNone() {
        assertEquals(emptyMap<Int, List<String>>(), newspeakEntriesByLine("package.json", projectJson))
    }

    fun testTheMenuHoldsRunAndBenchmark() {
        myFixture.configureByText("newspeak.json", projectJson)
        val document = myFixture.editor.document
        val mainButton = playButtons().single { renderer -> document.getLineNumber(renderer.lineMarkerInfo.startOffset) == 1 }
        val texts = menuActions(mainButton.popupMenuActions).map { action -> action.templatePresentation.text }
        assertEquals(listOf("Run", "Benchmark"), texts)
    }

    fun testRunOnAScriptCreatesItsConfigurationWithoutStartingIt() {
        myFixture.configureByText("newspeak.json", projectJson)
        val settings = createConfigurationFromGutter(project, myFixture.file.virtualFile, "timed", NewspeakCommand.BENCH)
        val configuration = settings.configuration as NewspeakRunConfiguration
        assertEquals("timed", configuration.script)
        assertEquals(NewspeakCommand.BENCH, configuration.command)
        assertEquals(myFixture.file.virtualFile.path, configuration.projectFile)
        assertEquals("${myFixture.file.virtualFile.parent.name} [timed] (benchmark)", settings.name)
        assertTrue(settings.isTemporary)
        assertSame(settings, RunManager.getInstance(project).selectedConfiguration)
    }

    fun testASecondClickReusesTheConfiguration() {
        myFixture.configureByText("newspeak.json", projectJson)
        val file = myFixture.file.virtualFile
        val first = createConfigurationFromGutter(project, file, null, NewspeakCommand.RUN)
        val benchmark = createConfigurationFromGutter(project, file, null, NewspeakCommand.BENCH)
        assertNotSame(first, benchmark)
        assertSame(first, createConfigurationFromGutter(project, file, null, NewspeakCommand.RUN))
        assertEquals(file.parent.name, first.name)
    }

    private fun contextAt(element: PsiElement): ConfigurationContext =
        ConfigurationContext.createEmptyContextForLocation(PsiLocation(element))

    private val producer: NewspeakRunConfigurationProducer
        get() = RunConfigurationProducer.getInstance(NewspeakRunConfigurationProducer::class.java)

    fun testTheContextMenuRunsTheScriptUnderTheCaretOrTheProject() {
        myFixture.configureByText("newspeak.json", projectJson)
        val file = myFixture.file
        val onSieve = checkNotNull(file.findElementAt(myFixture.editor.document.text.indexOf("\"sieve\"")))
        val fromKey = checkNotNull(producer.createConfigurationFromContext(contextAt(onSieve))).configuration as NewspeakRunConfiguration
        assertEquals("sieve", fromKey.script)
        assertEquals(NewspeakCommand.RUN, fromKey.command)
        assertEquals(file.virtualFile.path, fromKey.projectFile)
        assertEquals("${file.virtualFile.parent.name} [sieve]", fromKey.name)

        val fromFile = checkNotNull(producer.createConfigurationFromContext(contextAt(file))).configuration as NewspeakRunConfiguration
        assertNull(fromFile.script)

        val gutter = createConfigurationFromGutter(project, file.virtualFile, "sieve", NewspeakCommand.RUN)
        assertTrue(producer.isConfigurationFromContext(gutter.configuration as NewspeakRunConfiguration, contextAt(onSieve)))
        assertFalse(producer.isConfigurationFromContext(gutter.configuration as NewspeakRunConfiguration, contextAt(file)))
    }

    fun testTheContextMenuOfAnotherJsonFileOffersNothing() {
        myFixture.configureByText("package.json", projectJson)
        assertNull(producer.createConfigurationFromContext(contextAt(myFixture.file)))
    }
}
