package io.github.bstrauss.newspeak.devtool

import com.intellij.execution.process.NopProcessHandler
import com.intellij.execution.process.ProcessHandler
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.PathManager
import com.intellij.openapi.util.Disposer
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.ui.content.ContentFactory
import com.intellij.ui.content.ContentManager
import io.github.bstrauss.newspeak.devtool.editor.CodeListing
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
import io.github.bstrauss.newspeak.devtool.editor.ListingPosition
import io.github.bstrauss.newspeak.devtool.ui.RunPanel
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
    private fun start(configuration: NewspeakRunConfiguration, handler: ProcessHandler = NopProcessHandler()): Path? {
        var handed: Path? = null
        tracedRuns.start(configuration, TraceFilters.EVERYTHING) { traceFile ->
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
        val file = checkNotNull(start(configuration("Hello")))
        Files.writeString(file, """{"kind":"header","format":"ns-next-trace","version":1,"architecture":null}""" + "\n")
        var told = 0
        tracedRuns.addListener({ told++ }, testRootDisposable)
        contents.removeContent(contents.contents.single(), true)
        assertEmpty(tracedRuns.runs)
        assertNull(tracedRuns.selected)
        assertTrue(told > 0)
        waitUntil { !Files.exists(file) }
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
        // Started off the EDT, the run registers later on it -- by when its project has closed.
        ApplicationManager.getApplication().executeOnPooledThread {
            tracedRuns.start(configuration, TraceFilters.EVERYTHING) { traceFile ->
                // The runtime creates the file as it starts.
                Files.writeString(checkNotNull(traceFile), "")
                handed.set(traceFile)
                NopProcessHandler()
            }
        }.get(10, TimeUnit.SECONDS)
        val file = checkNotNull(handed.get())
        assertTrue(Files.exists(file))
        Disposer.dispose(tracedRuns)
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

    fun testARunWithoutTracingStartsNothing() {
        assertNull(start(configuration("Hello", traced = false)))
        // Watched methods that want no event of a run that lists nothing leave nothing to trace either.
        var handed: Path? = Path.of("unset")
        tracedRuns.start(configuration("Hello"), TraceFilters(emptyList(), emptyList(), noEvents = true, noListings = false)) { traceFile ->
            handed = traceFile
            NopProcessHandler()
        }
        assertNull(handed)
        assertEmpty(tracedRuns.runs)
        assertNull(tracedRuns.selected)
        assertEquals(0, contents.contentCount)
    }
}
