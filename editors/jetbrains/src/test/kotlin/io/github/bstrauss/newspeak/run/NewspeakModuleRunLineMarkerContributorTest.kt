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
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.PsiElement
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind

class NewspeakModuleRunLineMarkerContributorTest : BasePlatformTestCase() {
    private val cache: DocumentSymbolCache get() = DocumentSymbolCache.getInstance(project)
    private lateinit var serverFetcher: (VirtualFile) -> List<DocumentSymbol>?

    override fun setUp() {
        super.setUp()
        serverFetcher = cache.fetcher
    }

    override fun tearDown() {
        try {
            cache.fetcher = serverFetcher
            val runManager = RunManager.getInstance(project)
            runManager.getConfigurationSettingsList(NewspeakConfigurationType.getInstance())
                .forEach(runManager::removeConfiguration)
        } finally {
            super.tearDown()
        }
    }

    /** A symbol at the first occurrence of `marker` on `line` of `text`, its range and selection that one point. */
    private fun symbolAt(text: String, line: Int, marker: String, name: String, kind: SymbolKind, detail: String?): DocumentSymbol {
        // Kotlin strings are UTF-16, the unit the server's columns count in.
        val character = text.lines()[line].indexOf(marker)
        val position = Position(line, character)
        return DocumentSymbol(name, kind, Range(position, position), Range(position, position), detail)
    }

    private fun module(text: String, factory: String, mainDetail: String?): List<DocumentSymbol> {
        val root = symbolAt(text, 0, "Hello", "Hello", SymbolKind.Class, "public")
        val mainLine = text.lines().indexOfFirst { line -> line.contains("main:") }
        root.children = listOfNotNull(
            symbolAt(text, 0, "Hello", factory, SymbolKind.Constructor, "public"),
            mainDetail?.let { detail ->
                symbolAt(text, mainLine, "main:", "main: platform args: arguments", SymbolKind.Method, detail)
            },
        )
        return listOf(root)
    }

    private val runnable = """
        class Hello = (
        ) (
        public main: platform args: arguments = (
            ^ 0
        )
        )
    """.trimIndent()

    /** Per 0-based line, the Newspeak menu entries of the play buttons there, after the cache holds `symbols`. */
    private fun buttons(text: String, symbols: List<DocumentSymbol>): Map<Int, List<String>> {
        cache.fetcher = { symbols }
        myFixture.configureByText("Hello.ns", text)
        assertNotNull(cache.fetch(myFixture.file.virtualFile))
        val document = myFixture.editor.document
        return myFixture.findAllGutters()
            .filterIsInstance<LineMarkerInfo.LineMarkerGutterIconRenderer<*>>()
            .filter { renderer -> renderer.icon == AllIcons.RunConfigurations.TestState.Run }
            .associate { renderer ->
                val entries = menuActions(renderer.popupMenuActions)
                    .filterIsInstance<NewspeakRunAction>()
                    .map { action -> "${action.script}:${action.command}" }
                document.getLineNumber(renderer.lineMarkerInfo.startOffset) to entries
            }
    }

    private fun menuActions(action: AnAction?): List<AnAction> = when (action) {
        null -> emptyList()
        is ActionWithDelegate<*> -> listOfNotNull(action.delegate as? AnAction)
        is ActionGroup -> action.getChildren(null).flatMap(::menuActions)
        else -> listOf(action)
    }

    fun testARunnableModuleHasRunAndBenchmarkBesideMain() {
        assertEquals(mapOf(2 to listOf("null:RUN", "null:BENCH")), buttons(runnable, module(runnable, "new", "public")))
    }

    fun testAProtectedMainOrAnotherFactoryHasNone() {
        assertEquals(emptyMap<Int, List<String>>(), buttons(runnable, module(runnable, "new", "protected")))
        assertEquals(emptyMap<Int, List<String>>(), buttons(runnable, module(runnable, "usingPlatform: platform", "public")))
    }

    fun testWithoutACurrentAnswerThereIsNoButtonYet() {
        cache.fetcher = { null }
        myFixture.configureByText("Hello.ns", runnable)
        assertEquals(0, myFixture.findAllGutters().count { renderer -> renderer.icon == AllIcons.RunConfigurations.TestState.Run })
        // The pass asked; answer it before the test ends, so the request never reaches the real server once tearDown
        // restores it. Its daemon restart may still run after tearDown, and is skipped for a file no longer valid.
        val file = myFixture.file.virtualFile
        cache.fetcher = { module(runnable, "new", "public") }
        val deadline = System.currentTimeMillis() + 10_000
        while (cache.current(file) == null && System.currentTimeMillis() < deadline) {
            PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
            Thread.sleep(20)
        }
        assertNotNull("the pending request was answered", cache.current(file))
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
    }

    // A character outside the BMP takes two UTF-16 units; the button must still sit on `main:`.
    fun testColumnsPastAsciiStillFindMain() {
        val wide = runnable.replace("public main:", "(* \uD83D\uDE00 *) public main:")
        assertEquals(mapOf(2 to listOf("null:RUN", "null:BENCH")), buttons(wide, module(wide, "new", "public")))
    }

    fun testRunnableMainReadsTheRootsFactoryAndItsPublicInstanceSideMain() {
        assertNotNull(runnableMain(module(runnable, "new", "public")))
        assertNull(runnableMain(module(runnable, "new", "public class")))
        assertNull(runnableMain(module(runnable, "new", "private")))
        assertNull(runnableMain(module(runnable, "new", null)))
        assertNull(runnableMain(module(runnable, "packageUsing: manifest", "public")))
        assertNull(runnableMain(emptyList()))
        // The selector decides, whatever the parameters are called.
        val renamed = module(runnable, "new", "public")
        renamed.single().children[1].name = "main: platformObject args: words"
        assertNotNull(runnableMain(renamed))
        renamed.single().children[1].name = "main: platform"
        assertNull(runnableMain(renamed))
    }

    private val producer: NewspeakModuleRunConfigurationProducer
        get() = RunConfigurationProducer.getInstance(NewspeakModuleRunConfigurationProducer::class.java)

    private fun contextAt(element: PsiElement) =
        ConfigurationContext.createEmptyContextForLocation(PsiLocation(element))

    fun testTheContextMenuRunsARunnableModule() {
        cache.fetcher = { module(runnable, "new", "public") }
        myFixture.configureByText("Hello.ns", runnable)
        assertNotNull(cache.fetch(myFixture.file.virtualFile))
        val onBody = checkNotNull(myFixture.file.findElementAt(runnable.indexOf("^ 0")))
        for (context in listOf(contextAt(myFixture.file), contextAt(onBody))) {
            val configuration = checkNotNull(producer.createConfigurationFromContext(context)).configuration as NewspeakRunConfiguration
            assertEquals(myFixture.file.virtualFile.path, configuration.program)
            assertNull(configuration.script)
            assertEquals(NewspeakCommand.RUN, configuration.command)
            assertEquals("Hello.ns", configuration.name)
        }
        val gutter = createConfigurationFromGutter(project, myFixture.file.virtualFile, null, NewspeakCommand.RUN)
        assertTrue(producer.isConfigurationFromContext(gutter.configuration as NewspeakRunConfiguration, contextAt(myFixture.file)))
    }

    fun testTheContextMenuOfAModuleThatCannotRunOffersNothing() {
        cache.fetcher = { module(runnable, "new", "protected") }
        myFixture.configureByText("Hello.ns", runnable)
        assertNotNull(cache.fetch(myFixture.file.virtualFile))
        assertNull(producer.createConfigurationFromContext(contextAt(myFixture.file)))
    }

    // Off the EDT, with nothing kept yet, the producer asks the server itself: a Project-view
    // file never opened in an editor has no answer until someone asks.
    fun testOffTheEdtTheContextMenuAsksForTheSymbols() {
        cache.fetcher = { module(runnable, "new", "public") }
        val file = myFixture.addFileToProject("Hello.ns", runnable)
        val created = ApplicationManager.getApplication().executeOnPooledThread<Any?> {
            ReadAction.computeBlocking<Any?, RuntimeException> {
                producer.createConfigurationFromContext(contextAt(file))
            }
        }.get()
        assertNotNull(created)
    }
}
