package io.github.bstrauss.newspeak.devtool

import com.intellij.openapi.util.TextRange
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.util.xmlb.XmlSerializer

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
        watched.replaceEntries(listOf(WatchedMethods.Entry("Demo@1.0.0>>run")))
        assertTrue(watched.isWatched("Demo>>run"))
        assertTrue(watched.isWatched("Demo@1.0.0>>run"))
        assertFalse(watched.isWatched("Demo@2.0.0>>run"))
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
        myFixture.addFileToProject("app/newspeak.json", """{ "main": "lib/Demo.ns", "modules": [{ "path": "lib/Demo.ns", "as": "legacy.Demo" }] }""")
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
}
