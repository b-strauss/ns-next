package io.github.bstrauss.newspeak.devtool

import com.intellij.execution.executors.DefaultDebugExecutor
import com.intellij.execution.executors.DefaultRunExecutor
import com.intellij.execution.process.NopProcessHandler
import com.intellij.execution.process.ProcessHandler
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.PathManager
import com.intellij.openapi.options.advanced.AdvancedSettings
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.wm.ToolWindowEP
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.ui.content.ContentFactory
import com.intellij.ui.content.ContentManager
import io.github.bstrauss.newspeak.devtool.editor.CodeListing
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import io.github.bstrauss.newspeak.devtool.editor.ListingPosition
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import io.github.bstrauss.newspeak.devtool.ui.RunPanel
import io.github.bstrauss.newspeak.devtool.ui.TOOL_WINDOW_ID
import io.github.bstrauss.newspeak.run.NewspeakConfigurationType
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class TracedRunsTest : BasePlatformTestCase() {
    private lateinit var contents: ContentManager

    /** Runs of their own, never the project's service, which outlives a test, over [contents] for the tool window's. */
    private lateinit var tracedRuns: TracedRuns

    override fun setUp() {
        super.setUp()
        contents = ContentFactory.getInstance().createContentManager(true, project)
        Disposer.register(testRootDisposable, contents)
        tracedRuns = TracedRuns(project) { contents }
        Disposer.register(testRootDisposable, tracedRuns)
    }

    private fun configuration(name: String, traced: Boolean = true): NewspeakRunConfiguration =
        (NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration).apply {
            setName(name)
            program = "/work/app/Hello.ns"
            if (traced) options.traceCategories = mutableListOf("compile")
        }

    /** Starts a run of [configuration] as `startProcess` does, with a process of the test's own; answers the file it was handed. */
    private fun start(
        configuration: NewspeakRunConfiguration,
        handler: ProcessHandler = NopProcessHandler(),
        executorId: String = DefaultRunExecutor.EXECUTOR_ID,
    ): Path? {
        var handed: Path? = null
        tracedRuns.start(configuration, TraceFilters.EVERYTHING, executorId) { traceFile ->
            handed = traceFile
            handler
        }
        return handed
    }

    private fun waitUntil(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 10_000
        while (!condition()) {
            assertTrue("timed out", System.currentTimeMillis() < deadline)
            PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
            Thread.sleep(20)
        }
    }

    fun testAStartedRunGetsItsTabAndIsSelected() {
        val file = checkNotNull(start(configuration("Hello")))
        assertTrue(file.startsWith(Path.of(PathManager.getTempPath(), "newspeak-trace")))
        val run = tracedRuns.runs.single()
        assertTrue(run.name, run.name.matches(Regex("""Hello \d\d:\d\d:\d\d""")))
        assertSame(run, tracedRuns.selected)
        val content = contents.contents.single()
        assertEquals(run.name, content.displayName)
        assertTrue(content.isCloseable)
        assertSame(run, (content.component as RunPanel).run)
        assertSame(content, contents.selectedContent)
    }

    fun testTheRunReadsItsFileToTheEndOnceTheProcessTerminates() {
        val handler = NopProcessHandler()
        val file = checkNotNull(start(configuration("Hello"), handler))
        val run = tracedRuns.selected!!
        handler.startNotify()
        Files.writeString(file, """{"kind":"header","format":"ns-next-trace","version":1,"architecture":"x64"}""" + "\n")
        handler.destroyProcess()
        waitUntil { run.architecture == "x64" }
    }

    fun testClosingTheTabDeletesTheFile() {
        val handler = NopProcessHandler()
        val file = checkNotNull(start(configuration("Hello"), handler))
        handler.startNotify()
        Files.writeString(file, """{"kind":"header","format":"ns-next-trace","version":1,"architecture":null}""" + "\n")
        handler.destroyProcess()
        waitUntil { handler.isProcessTerminated }
        var told = 0
        tracedRuns.addListener({ told++ }, testRootDisposable)
        contents.removeContent(contents.contents.single(), true)
        assertEmpty(tracedRuns.runs)
        assertNull(tracedRuns.selected)
        assertTrue(told > 0)
        waitUntil { !Files.exists(file) }
    }

    fun testATabClosedWhileItsRunGoesOnDeletesTheFileWhenTheProcessEnds() {
        val handler = NopProcessHandler()
        val file = checkNotNull(start(configuration("Hello"), handler))
        handler.startNotify()
        Files.writeString(file, """{"kind":"header","format":"ns-next-trace","version":1,"architecture":null}""" + "\n")
        contents.removeContent(contents.contents.single(), true)
        assertEmpty("the run and its reader are dropped at once", tracedRuns.runs)
        // Longer than a release takes: the process still writes the file.
        Thread.sleep(200)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertTrue("the file stays while its process runs", Files.exists(file))
        assertTrue(TraceFiles.getInstance().isLive(file))
        handler.destroyProcess()
        waitUntil { !Files.exists(file) }
    }

    fun testTheToolWindowLetsItsTabsBeClosed() {
        // The registered extension: without `canCloseContents` the platform draws no close button on a run's tab.
        val window = ToolWindowEP.EP_NAME.extensionList.single { extension -> extension.id == TOOL_WINDOW_ID }
        assertTrue(window.canCloseContents)
    }

    fun testAClosedRunsListingTabsKeepTheirTextAndLetTheRunGo() {
        start(configuration("Hello"))
        val run = tracedRuns.runs.single()
        val listing = CodeListingFile(CodeListing("Demo>>run baseline", "--- code {Demo>>run}\n--- end\n", listOf(ListingPosition(1, 0), null), run))
        val textOnly = CodeListingFile("frame code", "text")
        tracedRuns.close(run)
        assertNull("the run is let go", listing.listing.run)
        assertEquals("--- code {Demo>>run}\n--- end\n", listing.listing.text)
        assertEquals("Demo>>run baseline", listing.listing.title)
        assertEquals("frame code", textOnly.listing.title)
    }

    fun testARunWhoseProjectClosedBeforeItRegisteredLetsItsFileGo() {
        val handed = AtomicReference<Path>()
        val configuration = configuration("Hello")
        val handler = NopProcessHandler().also { started -> started.startNotify() }
        // Started off the EDT, the run registers later on it -- by when its project has closed.
        ApplicationManager.getApplication().executeOnPooledThread {
            tracedRuns.start(configuration, TraceFilters.EVERYTHING, DefaultRunExecutor.EXECUTOR_ID) { traceFile ->
                // The runtime creates the file as it starts.
                Files.writeString(checkNotNull(traceFile), "")
                handed.set(traceFile)
                handler
            }
        }.get(10, TimeUnit.SECONDS)
        val file = checkNotNull(handed.get())
        assertTrue(Files.exists(file))
        Disposer.dispose(tracedRuns)
        // The registration the pooled thread queued finds the project closed; the process still writes the file.
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        Thread.sleep(200)
        assertTrue("the file stays while its process runs", Files.exists(file))
        handler.destroyProcess()
        waitUntil { !Files.exists(file) }
        assertEmpty(tracedRuns.runs)
    }

    fun testADirectoryDeletedWhileTheIdeRunsIsMadeAgain() {
        val directory = Files.createTempDirectory("newspeak-trace").resolve("newspeak-trace")
        val files = TraceFiles(directory)
        files.sweepOnce()
        assertTrue(Files.isDirectory(directory))
        Files.delete(directory)
        val file = files.newFile()
        assertTrue("the run's file can be created", Files.isDirectory(file.parent))
        Files.writeString(file, "")
        files.release(file)
        waitUntil { !Files.exists(file) }
    }

    fun testTheSweepDeletesOnlyTheFilesNoLiveRunNames() {
        val directory = Files.createTempDirectory("newspeak-trace")
        val left = directory.resolve("trace-left.jsonl")
        Files.writeString(left, "")
        val files = TraceFiles(directory)
        val running = files.newFile()
        Files.writeString(running, "")
        files.sweepOnce()
        assertFalse("a file of an earlier run of the IDE", Files.exists(left))
        assertTrue("a running trace", Files.exists(running))
        // Once per IDE start.
        Files.writeString(left, "")
        files.sweepOnce()
        assertTrue(Files.exists(left))
        files.release(running)
        waitUntil { !Files.exists(running) && !files.isLive(running) }
    }

    fun testTheNewestRunIsSelected() {
        start(configuration("First"))
        start(configuration("Second"))
        val (first, second) = tracedRuns.runs
        assertSame(second, tracedRuns.selected)
        assertEquals(second.name, contents.selectedContent?.displayName)
        // Selecting a tab selects its run, and closing the selected one selects the newest left.
        contents.setSelectedContent(contents.contents.first())
        assertSame(first, tracedRuns.selected)
        start(configuration("Third"))
        tracedRuns.close(tracedRuns.selected!!)
        assertEquals(listOf(first, second), tracedRuns.runs)
        assertSame(second, tracedRuns.selected)
        assertEquals(2, contents.contentCount)
    }

    /** Starts a run of [configuration] whose process the test ends, as `startProcess` does; answers its process and file. */
    private fun startEnded(
        configuration: NewspeakRunConfiguration,
        ended: Boolean = true,
        executorId: String = DefaultRunExecutor.EXECUTOR_ID,
    ): Pair<NopProcessHandler, Path> {
        val handler = NopProcessHandler().also { started -> started.startNotify() }
        val file = checkNotNull(start(configuration, handler, executorId))
        Files.writeString(file, """{"kind":"header","format":"ns-next-trace","version":1,"architecture":null}""" + "\n")
        if (ended) {
            handler.destroyProcess()
            waitUntil { handler.isProcessTerminated }
        }
        return handler to file
    }

    fun testARunsTabCanBePinned() {
        start(configuration("Hello"))
        assertTrue(contents.contents.single().isPinnable)
    }

    fun testARerunReplacesTheEndedTabOfItsConfigurationAndDeletesItsFile() {
        val configuration = configuration("Hello")
        val (_, first) = startEnded(configuration)
        startEnded(configuration("Other"), ended = false)
        val replaced = tracedRuns.runs.first()
        val (_, second) = startEnded(configuration, ended = false)
        assertFalse("the ended run is dropped", replaced in tracedRuns.runs)
        assertEquals(2, contents.contentCount)
        // The new run takes the replaced tab's place, and is selected.
        assertSame(tracedRuns.selected, (contents.contents.first().component as RunPanel).run)
        assertEquals(second, tracedRuns.selected?.file)
        waitUntil { !Files.exists(first) }
        assertTrue(Files.exists(second))
    }

    fun testAPinnedTabIsNotReplaced() {
        val configuration = configuration("Hello")
        val (_, first) = startEnded(configuration)
        contents.contents.single().isPinned = true
        startEnded(configuration, ended = false)
        assertEquals(2, tracedRuns.runs.size)
        assertEquals(2, contents.contentCount)
        // Longer than a release takes.
        Thread.sleep(200)
        PlatformTestUtil.dispatchAllEventsInIdeEventQueue()
        assertTrue(Files.exists(first))
    }

    fun testARunStillGoingIsNotReplaced() {
        val configuration = configuration("Hello")
        val (handler, first) = startEnded(configuration, ended = false)
        startEnded(configuration, ended = false)
        assertEquals(2, tracedRuns.runs.size)
        assertEquals(2, contents.contentCount)
        assertTrue(Files.exists(first))
        // Ended later, it is replaced by the next run of its configuration, which the second still going is not.
        handler.destroyProcess()
        waitUntil { handler.isProcessTerminated }
        startEnded(configuration, ended = false)
        assertEquals(2, tracedRuns.runs.size)
        waitUntil { !Files.exists(first) }
    }

    /** Run and Debug have a tool window each on the platform: a traced Debug never replaces a traced Run's tab, nor the other way round. */
    fun testARunUnderAnotherExecutorIsNotReplaced() {
        val configuration = configuration("Hello")
        startEnded(configuration)
        startEnded(configuration, executorId = DefaultDebugExecutor.EXECUTOR_ID)
        assertEquals(2, tracedRuns.runs.size)
        startEnded(configuration, ended = false, executorId = DefaultDebugExecutor.EXECUTOR_ID)
        assertEquals("the ended Debug run is replaced, the Run one stays", 2, tracedRuns.runs.size)
        startEnded(configuration, ended = false)
        assertEquals(2, tracedRuns.runs.size)
    }

    /** The selected tab is replaced first, wherever it stands; the new one takes its place. */
    fun testTheSelectedEndedTabIsReplacedFirst() {
        val configuration = configuration("Hello")
        val (firstHandler, _) = startEnded(configuration, ended = false)
        val (secondHandler, _) = startEnded(configuration, ended = false)
        for (handler in listOf(firstHandler, secondHandler)) handler.destroyProcess()
        waitUntil { firstHandler.isProcessTerminated && secondHandler.isProcessTerminated }
        val (first, second) = tracedRuns.runs
        contents.setSelectedContent(contents.contents[1])
        startEnded(configuration, ended = false)
        assertEquals(listOf(first, tracedRuns.selected), tracedRuns.runs)
        assertFalse(second in tracedRuns.runs)
        assertSame("at the replaced tab's place", tracedRuns.selected, (contents.contents[1].component as RunPanel).run)
    }

    /** Without the selected tab among them, the first ended tab in the tabs' order is replaced, whatever order the runs started in. */
    fun testTheFirstEndedTabInTheTabsOrderIsReplaced() {
        val configuration = configuration("Hello")
        val (firstHandler, _) = startEnded(configuration, ended = false)
        val (secondHandler, _) = startEnded(configuration, ended = false)
        startEnded(configuration, ended = false)
        firstHandler.destroyProcess()
        waitUntil { firstHandler.isProcessTerminated }
        // The fourth run takes the first's tab, in front of the second's, which started before it.
        val (fourthHandler, _) = startEnded(configuration, ended = false)
        val (second, third, fourth) = tracedRuns.runs
        assertSame(fourth, (contents.contents[0].component as RunPanel).run)
        for (handler in listOf(secondHandler, fourthHandler)) handler.destroyProcess()
        waitUntil { secondHandler.isProcessTerminated && fourthHandler.isProcessTerminated }
        contents.setSelectedContent(contents.contents.single { content -> (content.component as RunPanel).run === third })
        startEnded(configuration, ended = false)
        assertTrue(second in tracedRuns.runs)
        assertFalse(fourth in tracedRuns.runs)
    }

    /** The selection goes from the replaced run to the new one, never through another, and the new tab is the selected one. */
    fun testAReplacementSelectsNoOtherRun() {
        val configuration = configuration("Hello")
        startEnded(configuration("Other"))
        startEnded(configuration)
        val seen = ArrayList<TraceRun?>()
        tracedRuns.addListener({ seen += tracedRuns.selected }, testRootDisposable)
        val replaced = tracedRuns.selected
        startEnded(configuration, ended = false)
        val newest = tracedRuns.selected
        assertNotSame(replaced, newest)
        assertEquals(setOf(newest), seen.toSet())
        assertSame(contents.selectedContent, contents.contents.single { content -> (content.component as RunPanel).run === newest })
    }

    /** The setting by which the platform starts its run tabs pinned starts these pinned, and so kept. */
    fun testTheSettingThatStartsTabsPinnedIsHonoured() {
        AdvancedSettings.setBoolean(TracedRuns.START_PINNED, true)
        try {
            val configuration = configuration("Hello")
            startEnded(configuration)
            assertTrue(contents.contents.single().isPinned)
            startEnded(configuration, ended = false)
            assertEquals(2, tracedRuns.runs.size)
        } finally {
            AdvancedSettings.setBoolean(TracedRuns.START_PINNED, false)
        }
    }

    fun testAnotherConfigurationsTabIsNotReplaced() {
        startEnded(configuration("First"))
        startEnded(configuration("Second"))
        startEnded(configuration("Third"), ended = false)
        assertEquals(3, tracedRuns.runs.size)
        assertEquals(3, contents.contentCount)
    }

    fun testARunWithoutTracingStartsNothing() {
        assertNull(start(configuration("Hello", traced = false)))
        // Watched methods that want no event of a run that lists nothing leave nothing to trace either.
        var handed: Path? = Path.of("unset")
        tracedRuns.start(configuration("Hello"), TraceFilters(emptyList(), emptyList(), noEvents = true, noListings = false), DefaultRunExecutor.EXECUTOR_ID) { traceFile ->
            handed = traceFile
            NopProcessHandler()
        }
        assertNull(handed)
        assertEmpty(tracedRuns.runs)
        assertNull(tracedRuns.selected)
        assertEquals(0, contents.contentCount)
    }
}
