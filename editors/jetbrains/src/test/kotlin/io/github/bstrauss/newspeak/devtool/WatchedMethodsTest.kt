package io.github.bstrauss.newspeak.devtool

import com.intellij.execution.RunManager
import com.intellij.openapi.util.TextRange
import com.intellij.openapi.util.io.FileUtil
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.util.xmlb.XmlSerializer
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.run.NewspeakConfigurationType
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import io.github.bstrauss.newspeak.run.nothingTracedNotice
import java.nio.file.Files

class WatchedMethodsTest : BasePlatformTestCase() {
    /** A list of its own, never the project's service, which outlives a test. */
    private fun watchedMethods() = WatchedMethods(project)

    fun testWatchAddsAnEntryWithBothSwitches() {
        val watched = watchedMethods()
        watched.watch("Demo>>run")
        watched.watch("Demo>>run")
        assertEquals(listOf(WatchedMethods.Entry("Demo>>run", events = true, listings = true)), watched.entries)
        assertTrue(watched.isWatched("Demo>>run"))
        watched.unwatch("Demo>>run")
        assertEquals(emptyList<WatchedMethods.Entry>(), watched.entries)
        assertFalse(watched.isWatched("Demo>>run"))
    }

    fun testFiltersOfAnEmptyListAreEverything() {
        assertEquals(TraceFilters.EVERYTHING, watchedMethods().filters())
    }

    fun testFiltersFollowTheSwitches() {
        val watched = watchedMethods()
        watched.watch("Demo>>run")
        watched.watch("Demo/Account>>deposit:")
        watched.replaceEntries(
            listOf(
                WatchedMethods.Entry("Demo>>run", events = true, listings = false),
                WatchedMethods.Entry("Demo/Account>>deposit:", events = true, listings = true),
            ),
        )
        assertEquals(
            TraceFilters(listOf("Demo>>run", "Demo/Account>>deposit:"), listOf("Demo/Account>>deposit:"), noEvents = false, noListings = false),
            watched.filters(),
        )
        // No entry wants a kind: none of it is asked for, since no pattern would mean every body.
        watched.replaceEntries(listOf(WatchedMethods.Entry("Demo>>run", events = false, listings = true)))
        assertEquals(TraceFilters(emptyList(), listOf("Demo>>run"), noEvents = true, noListings = false), watched.filters())
        watched.replaceEntries(listOf(WatchedMethods.Entry("Demo>>run", events = true, listings = false)))
        assertEquals(TraceFilters(listOf("Demo>>run"), emptyList(), noEvents = false, noListings = true), watched.filters())
    }

    fun testTheListIsKeptInTheWorkspace() {
        val watched = watchedMethods()
        watched.watch("Demo>>run")
        watched.replaceEntries(watched.entries + WatchedMethods.Entry("com.acme.Demo/Account class>>...", events = false, listings = true))
        val element = XmlSerializer.serialize(watched.state)
        val restored = watchedMethods()
        restored.loadState(XmlSerializer.deserialize(element, WatchedMethods.Stored::class.java))
        assertEquals(watched.entries, restored.entries)
    }

    fun testAPrefixPatternWatchesItsMethods() {
        val watched = watchedMethods()
        watched.watch("Demo>>sq...")
        assertTrue(watched.isWatched("Demo>>square:"))
        assertTrue(watched.isWatched("Demo@1.0.0>>square:"))
        assertFalse(watched.isWatched("Demo>>run"))
        assertFalse(watched.isWatched("Demo/Inner>>square:"))
    }

    fun testAPatternWithoutAVersionCoversEveryVersion() {
        val watched = watchedMethods()
        watched.watch("Demo>>run")
        assertTrue(watched.isWatched("Demo@2.0.0>>run"))
        // A pattern with a version covers that version only, exact or a prefix, as the runtime's filter matches.
        watched.replaceEntries(listOf(WatchedMethods.Entry("Demo@1.0.0>>run"), WatchedMethods.Entry("Other@1....")))
        assertFalse(watched.isWatched("Demo>>run"))
        assertTrue(watched.isWatched("Demo@1.0.0>>run"))
        assertFalse(watched.isWatched("Demo@2.0.0>>run"))
        assertFalse(watched.isWatched("Other>>run"))
        assertTrue(watched.isWatched("Other@1.2.0>>run"))
        assertFalse(watched.isWatched("Other@2.0.0>>run"))
    }

    fun testTheMethodAtAnOffsetIsTheOneWhoseRangeHoldsIt() {
        val names = listOf(
            MethodName("Demo>>run", TextRange(10, 20), TextRange(12, 15)),
            MethodName("Demo>>stop", TextRange(30, 40), TextRange(32, 36)),
        )
        assertEquals("Demo>>run", methodAt(names, 20)?.name)
        assertEquals("Demo>>stop", methodAt(names, 30)?.name)
        assertNull(methodAt(names, 25))
    }

    fun testAProjectFileSAsGivesTheModuleItsId() {
        myFixture.addFileToProject(
            "app/newspeak.json",
            """{ "main": "App.ns", "modules": [{ "path": "lib/Demo.ns", "as": "legacy.Demo" }, "lib/Other.ns"] }""",
        )
        val demo = myFixture.addFileToProject("app/lib/Demo.ns", "class Demo = () ()\n").virtualFile
        val other = myFixture.addFileToProject("app/lib/Other.ns", "class Other = () ()\n").virtualFile
        assertEquals("legacy.Demo", projectAliasOf(demo))
        assertNull(projectAliasOf(other))
        assertEquals("legacy.Demo/Account class>>create", withId("Demo/Account class>>create", "legacy.Demo"))
        assertEquals("legacy.Demo>>/", withId("com.acme.Demo>>/", "legacy.Demo"))
    }

    fun testTheServerSNamesTakeTheProjectFileSId() {
        myFixture.addFileToProject("app/newspeak.json", """{ "main": "App.ns", "modules": [{ "path": "lib/Demo.ns", "as": "legacy.Demo" }] }""")
        val demo = myFixture.addFileToProject("app/lib/Demo.ns", "class Demo = () (\n  public run = (^1)\n) : ()\n").virtualFile
        val watched = watchedMethods()
        var asked = 0
        watched.fetcher = { _ ->
            asked++
            listOf(MethodName("com.acme.Demo>>run", TextRange(20, 37), TextRange(27, 30)))
        }
        assertEquals(listOf(MethodName("legacy.Demo>>run", TextRange(20, 37), TextRange(27, 30))), watched.methodNames(demo))
        assertEquals(watched.methodNames(demo), watched.currentMethodNames(demo))
        assertEquals("kept for the document's stamp", 1, asked)
    }

    fun testARunIsGivenOnlyThePatternsOfItsModules() {
        val watched = watchedMethods()
        watched.replaceEntries(
            listOf(
                WatchedMethods.Entry("Demo>>run"),
                WatchedMethods.Entry("Elsewhere>>run"),
                WatchedMethods.Entry("Elsewhere...", events = false),
                WatchedMethods.Entry("com.acme.Util@2.0.0/Inner>>...", events = false),
                WatchedMethods.Entry("newspeak.core.Kernel/Array>>at:", listings = false),
            ),
        )
        val modules = mapOf("Demo" to emptySet(), "com.acme.Util" to setOf("1.0.0")) + SHIPPED_MODULE_IDS.associateWith { emptySet<String>() }
        assertEquals(
            TraceFilters(listOf("Demo>>run", "newspeak.core.Kernel/Array>>at:"), listOf("Demo>>run"), noEvents = false, noListings = false),
            watched.filters(modules),
        )
        // Every pattern of a kind another program's: that kind is not asked for, never every body.
        watched.replaceEntries(listOf(WatchedMethods.Entry("Elsewhere>>run"), WatchedMethods.Entry("Demo>>run", events = false)))
        assertEquals(TraceFilters(emptyList(), listOf("Demo>>run"), noEvents = true, noListings = false), watched.filters(modules))
    }

    fun testAMalformedPatternIsNeverPassed() {
        val watched = watchedMethods()
        val malformed = listOf("Demo...>>run", "Demo>>", "Demo/>>run", "Demo@1.0>>run", "Demo@1.0.0+build>>run", "Demo>>a b", "1Demo>>run", "Demo>>x.y...")
        for (pattern in malformed) assertNull(pattern, patternModule(pattern))
        val wellFormed = listOf("Demo>>run", "Demo/Account class>>create", "Demo@1.0.0-beta.1>>run", "Demo/Acc...", "Demo/...", "Demo@1....", "com.acme....", "Demo class...", "Demo>>,")
        for (pattern in wellFormed) assertNotNull(pattern, patternModule(pattern))
        watched.replaceEntries(listOf(WatchedMethods.Entry("Demo...>>run"), WatchedMethods.Entry("Demo>>run")))
        assertEquals(TraceFilters(listOf("Demo>>run"), listOf("Demo>>run"), noEvents = false, noListings = false), watched.filters())
        assertEquals("Demo...>>run", firstMalformedPattern(watched.entries))
        assertNull(firstMalformedPattern(listOf(WatchedMethods.Entry("Demo>>run"), WatchedMethods.Entry("  "))))
    }

    fun testTheProgramSModulesAreItsProjectFileSWithTheirIds() {
        val projectFile = myFixture.addFileToProject(
            "run/newspeak.json",
            """{ "main": "App.ns", "modules": [{ "path": "lib/Demo.ns", "as": "legacy.Demo" }, "lib/Util.ns", "lib/Missing.ns"] }""",
        ).virtualFile
        myFixture.addFileToProject("run/App.ns", "class App = () ()\n")
        myFixture.addFileToProject("run/lib/Demo.ns", "class Demo = () ()\n")
        myFixture.addFileToProject("run/lib/Util.ns", "class Util = () ()\n")
        val identities = mapOf(
            "App.ns" to ModuleIdentity("App", null),
            "Demo.ns" to ModuleIdentity("com.acme.Demo", "1.0.0"),
            "Util.ns" to ModuleIdentity("com.acme.Util", "2.1.0"),
        )
        val modules = programModules(projectFile) { file -> identities[file.name] }
        assertEquals(
            mapOf("App" to emptySet(), "legacy.Demo" to setOf("1.0.0"), "com.acme.Util" to setOf("2.1.0")),
            modules.filterKeys { id -> id !in SHIPPED_MODULE_IDS },
        )
        assertTrue(modules.keys.containsAll(SHIPPED_MODULE_IDS))
        val module = myFixture.addFileToProject("single/Single.ns", "class Single = () ()\n").virtualFile
        assertEquals(setOf("Single") + SHIPPED_MODULE_IDS, programModules(module) { ModuleIdentity("Single", null) }.keys)
    }

    fun testAnAbsoluteModulePathIsFound() {
        val program = Files.createTempDirectory("newspeak-program")
        val elsewhere = Files.createTempDirectory("newspeak-modules")
        val demoPath = elsewhere.resolve("Demo.ns")
        Files.writeString(demoPath, "class Demo = () ()\n")
        val written = FileUtil.toSystemIndependentName(demoPath.toString())
        val projectPath = program.resolve("newspeak.json")
        Files.writeString(projectPath, """{ "main": "App.ns", "modules": [{ "path": "$written", "as": "legacy.Demo" }] }""")
        Files.writeString(program.resolve("App.ns"), "class App = () ()\n")
        val fileSystem = LocalFileSystem.getInstance()
        val projectFile = checkNotNull(fileSystem.refreshAndFindFileByNioFile(projectPath))
        val demo = checkNotNull(fileSystem.refreshAndFindFileByNioFile(demoPath))
        val modules = programModules(projectFile) { file -> if (file == demo) ModuleIdentity("com.acme.Demo", "1.0.0") else ModuleIdentity("App", null) }
        assertEquals(setOf("1.0.0"), modules["legacy.Demo"])
        // The project file found through a run configuration's program, the module's `as` through its absolute path.
        val runManager = RunManager.getInstance(project)
        val settings = runManager.createConfiguration("App", NewspeakConfigurationType.getInstance().factory)
        (settings.configuration as NewspeakRunConfiguration).program = projectPath.toString()
        runManager.addConfiguration(settings)
        try {
            assertEquals("legacy.Demo", projectAliasOf(demo, project))
        } finally {
            runManager.removeConfiguration(settings)
        }
    }

    fun testARunStartWaitsForTheServerToRun() {
        // A client that comes up initializing after it was started, and runs two looks later.
        var started = false
        var looks = 0
        val running = runningClient(
            lookup = { if (started) "client" else null },
            start = {
                started = true
                true
            },
            isRunning = { _ -> ++looks >= 3 },
            isOver = { false },
            isCancelled = { false },
            pause = { _ -> },
        )
        assertEquals("client", running)
        assertTrue(started)
        // A server that cannot start is not waited for, nor is one that never runs waited for past the bound.
        var now = 0L
        assertNull(runningClient<String>({ null }, { false }, { true }, { false }, { false }, pause = { _ -> now += 1 }, clock = { now }))
        assertEquals(0L, now)
        assertNull(runningClient({ "client" }, { true }, { false }, { false }, { false }, timeoutMilliseconds = 500, pause = { milliseconds -> now += milliseconds }, clock = { now }))
        assertTrue(now >= 500)
        // A server that failed or stopped ends the wait at once, found so or failing while it is waited for.
        now = 0L
        assertNull(runningClient({ "client" }, { true }, { false }, { true }, { false }, pause = { milliseconds -> now += milliseconds }, clock = { now }))
        assertEquals(0L, now)
        var failed = false
        assertNull(runningClient({ "client" }, { true }, { false }, { _ -> failed.also { failed = true } }, { false }, pause = { milliseconds -> now += milliseconds }, clock = { now }))
        assertEquals(WatchedMethods.CLIENT_POLL_MILLISECONDS, now)
        // A running client is answered at once, never started again.
        assertEquals("client", runningClient({ "client" }, { throw AssertionError("started again") }, { true }, { false }, { false }))
    }

    fun testARunStartWaitsForTheServerOnceForEveryModule() {
        val watched = watchedMethods()
        var waits = 0
        watched.serverWaiter = {
            waits++
            null
        }
        val identities = watched.moduleIdentities()
        val first = myFixture.addFileToProject("app/A.ns", "class A = () ()\n").virtualFile
        val second = myFixture.addFileToProject("app/B.ns", "class B = () ()\n").virtualFile
        assertNull(identities(first))
        assertNull(identities(second))
        assertEquals(1, waits)
    }

    fun testARunThatTracesNothingSaysWhichEntriesWereDropped() {
        val watched = watchedMethods()
        watched.replaceEntries(listOf(WatchedMethods.Entry("Elsewhere>>run"), WatchedMethods.Entry("Gone>><=")))
        val modules = SHIPPED_MODULE_IDS.associateWith { emptySet<String>() }
        val filters = watched.filters(modules)
        val dropped = watched.droppedPatterns(modules)
        assertEquals(listOf("Elsewhere>>run", "Gone>><="), dropped)
        val configuration = (NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration).apply {
            program = "/work/app/Hello.ns"
            options.traceCategories = mutableListOf("compile")
        }
        assertEquals(
            NewspeakBundle.message("devtool.watched.nothing.traced.content", "Elsewhere&gt;&gt;run, Gone&gt;&gt;&lt;="),
            nothingTracedNotice(configuration, filters, dropped),
        )
        // A run that still traces something, and one that traces nothing by its own settings, say nothing.
        watched.replaceEntries(listOf(WatchedMethods.Entry("Elsewhere>>run"), WatchedMethods.Entry("newspeak.core.Kernel/Array>>at:")))
        assertNull(nothingTracedNotice(configuration, watched.filters(modules), watched.droppedPatterns(modules)))
        configuration.options.traceCategories = mutableListOf()
        assertNull(nothingTracedNotice(configuration, filters, dropped))
    }

    fun testTheShippedIdsAreTheCoreLibrarySDeclarations() {
        val root = java.nio.file.Path.of(System.getProperty("nsNext.repositoryRoot")).resolve("crates/ns-runtime/core-library")
        val declaration = Regex("""\(\* :module: ([A-Za-z0-9_.]+) \*\)""")
        val declared = java.nio.file.Files.walk(root).use { paths ->
            paths.filter { path -> path.toString().endsWith(".ns") }.toList()
        }.mapNotNull { path -> declaration.find(java.nio.file.Files.readString(path))?.groupValues?.get(1) }
        assertEquals(declared.sorted(), SHIPPED_MODULE_IDS.sorted())
    }
}
