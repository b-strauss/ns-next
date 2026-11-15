package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonArray
import com.google.gson.JsonObject
import com.google.gson.JsonParser
import com.intellij.execution.KillableProcess
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.executors.DefaultDebugExecutor
import com.intellij.execution.process.ProcessEvent
import com.intellij.execution.process.ProcessListener
import com.intellij.execution.runners.ExecutionEnvironmentBuilder
import com.intellij.icons.AllIcons
import com.intellij.ide.projectView.ViewSettings
import com.intellij.ide.projectView.impl.nodes.ExternalLibrariesNode
import com.intellij.ide.projectView.impl.nodes.PsiDirectoryNode
import com.intellij.ide.projectView.impl.nodes.SyntheticLibraryElementNode
import com.intellij.ide.util.treeView.AbstractTreeNode
import com.intellij.psi.PsiFileSystemItem
import com.intellij.notification.Notification
import com.intellij.notification.Notifications
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.roots.AdditionalLibraryRootsProvider
import com.intellij.openapi.roots.ProjectFileIndex
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.Key
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.vfs.newvfs.impl.VfsRootAccess
import com.intellij.testFramework.LightVirtualFile
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.testFramework.runInEdtAndWait
import com.intellij.xdebugger.XDebugProcess
import com.intellij.xdebugger.XDebugProcessStarter
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebugSessionListener
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.XDebuggerManagerListener
import com.intellij.xdebugger.XDebuggerUtil
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointListener
import com.intellij.xdebugger.breakpoints.XLineBreakpoint
import com.intellij.ui.ColoredTextContainer
import com.intellij.ui.SimpleTextAttributes
import com.intellij.xdebugger.frame.XCompositeNode
import com.intellij.xdebugger.frame.XExecutionStack
import com.intellij.xdebugger.frame.XStackFrame
import com.intellij.xdebugger.frame.XDebuggerTreeNodeHyperlink
import com.intellij.xdebugger.frame.XFullValueEvaluator
import com.intellij.xdebugger.frame.XValue
import com.intellij.xdebugger.frame.XValueChildrenList
import com.intellij.xdebugger.frame.XValueContainer
import com.intellij.xdebugger.frame.XValueNode
import com.intellij.xdebugger.frame.XValuePlace
import com.intellij.xdebugger.frame.presentation.XValuePresentation
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.library.CoreLibraryStore
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import io.github.bstrauss.newspeak.library.NewspeakCoreLibraryRoots
import io.github.bstrauss.newspeak.library.NewspeakCoreLibraryRootsProvider
import io.github.bstrauss.newspeak.run.NewspeakConfigurationType
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.nio.file.Files
import java.nio.file.Path
import java.time.Duration
import java.time.Instant
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import javax.swing.Icon

/**
 * The debug process in the platform's debugger, against the real runtime
 * the build names in `nsNext.runtime`: Debug on a Newspeak run
 * configuration, through the plugin's runner, as the user starts it.
 *
 * The test runs off the event dispatch thread, so that it may wait for
 * the session while the platform updates it there. Every wait is bounded,
 * and every listener is registered as the debug process is created --
 * before it connects -- so that no stop can come before it.
 */
class NewspeakDebugProcessTest : BasePlatformTestCase() {
    // Read once a test runs: JUnit 3 makes an instance of every test even where a filter leaves the class out.
    private val runtime: Path by lazy { Path.of(System.getProperty("nsNext.runtime")) }
    private lateinit var directory: Path
    private val debugged = CopyOnWriteArrayList<Debugging>()

    override fun runInDispatchThread(): Boolean = false

    override fun setUp() {
        super.setUp()
        directory = Files.createTempDirectory("ns-next-debug-process")
        NewspeakSettings.getInstance().state.runtimePath = runtime.toString()
    }

    override fun tearDown() {
        try {
            for (debugging in debugged) {
                val handler = debugging.process.processHandler
                if (!handler.isProcessTerminated) (handler as KillableProcess).killProcess()
                handler.waitFor(WAIT_MILLISECONDS)
                debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
            }
            runInEdtAndWait {
                // The session's console and the editors its stops opened.
                for (debugging in debugged) {
                    debugging.session.consoleView?.let(Disposer::dispose)
                    Disposer.dispose(debugging.session.runContentDescriptor)
                }
                val editors = FileEditorManager.getInstance(project)
                editors.openFiles.forEach(editors::closeFile)
                val breakpoints = XDebuggerManager.getInstance(project).breakpointManager
                WriteAction.run<Throwable> { breakpoints.allBreakpoints.forEach(breakpoints::removeBreakpoint) }
            }
            NewspeakSettings.getInstance().state.runtimePath = null
            // The store's files are read-only, which Windows refuses to delete otherwise.
            CoreLibraryStore.deleteRecursively(directory)
        } finally {
            super.tearDown()
        }
    }

    fun testDebuggingStopsAtALineBreakpointAndShowsTheFrame() {
        val program = program("Stops", STOPS)
        toggleBreakpoint(program, line = 6)
        val debugging = debug(program)

        val position = debugging.nextPause()
        assertEquals(program, position.file)
        assertEquals("line 6, counted from 0 by the platform", 5, position.line)

        val frame = checkNotNull(debugging.session.currentStackFrame) { "the stop shows a frame" }
        val locals = childrenOf(frame)
        assertTrue(locals.keys.toString(), locals.keys.containsAll(listOf("count", "elements")))
        assertEquals("3", presentationOf(locals.getValue("count")).value)
        val elements = presentationOf(locals.getValue("elements"))
        assertTrue("an Array has children", elements.hasChildren)
        val elementValues = childrenOf(locals.getValue("elements")).map { (name, value) -> name to presentationOf(value).value }
        assertEquals(listOf("[1]" to "10", "[2]" to "20", "[3]" to "30"), elementValues)

        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertEquals(0, debugging.process.processHandler.exitCode)
        assertTrue(debugging.output(), debugging.output().contains("3"))
    }

    fun testStepOverMovesToTheNextLine() {
        val program = program("Stops", STOPS)
        toggleBreakpoint(program, line = 4)
        val debugging = debug(program)
        assertEquals(3, debugging.nextPause().line)

        onEdt { debugging.session.stepOver(false) }
        val stepped = debugging.nextPause()
        assertEquals(program, stepped.file)
        assertEquals("line 5, counted from 0 by the platform", 4, stepped.line)

        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    fun testStopTerminatesAndASecondStopKills() {
        val program = program("LongLeaf", LONG_LEAF)
        val debugging = debug(program)
        debugging.printed("computing").get(WAIT_SECONDS, TimeUnit.SECONDS)
        val handler = debugging.process.processHandler

        // The first Stop: what the platform's Stop does to a running process.
        handler.destroyProcess()
        (debugging.process as NewspeakDebugProcess).terminateAnswered.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertFalse("a program inside one leaf outlives the terminate", handler.waitFor(LEAF_OUTLIVES_MILLISECONDS))
        assertTrue(handler.isProcessTerminating)

        // The second: the platform's Stop on a process that is terminating kills it.
        (handler as KillableProcess).killProcess()
        assertTrue(handler.waitFor(WAIT_MILLISECONDS))
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertFalse(debugging.output(), debugging.output().contains("done"))
    }

    fun testAWrongProcessIdEndsTheSession() {
        val reported = notifications()
        val faked = fakeSession()
        val otherProcess = ProcessHandle.current().pid()
        val agent = FakeAgent(faked.port, onAttach = {
            event("process", processBody(otherProcess, SECRET))
            event("initialized")
        })
        assertEquals(
            NewspeakBundle.message("debugger.error.wrong.process", otherProcess.toString(), faked.handler.process.pid().toString()),
            reported.get(WAIT_SECONDS, TimeUnit.SECONDS),
        )
        assertTrue("the process started is ended", faked.handler.waitFor(WAIT_MILLISECONDS))
        faked.debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertEquals(listOf("initialize", "attach"), agent.commands())
    }

    /**
     * A peer that names the started process's id, which is no secret, but
     * returns no secret or another is no more trusted than one that names
     * another process: the session ends, saying what was compared.
     */
    fun testAPeerWithoutTheSecretEndsTheSession() {
        for ((returned, what) in listOf(null to "debugger.error.secret.missing", "0".repeat(32) to "debugger.error.secret.wrong")) {
            val reported = notifications()
            val faked = fakeSession()
            val agent = FakeAgent(faked.port, onAttach = {
                event("process", processBody(faked.handler.process.pid(), returned))
                event("initialized")
            })
            assertEquals(
                NewspeakBundle.message("debugger.error.wrong.secret", faked.handler.process.pid().toString(), NewspeakBundle.message(what)),
                reported.get(WAIT_SECONDS, TimeUnit.SECONDS),
            )
            assertTrue("the process started is ended", faked.handler.waitFor(WAIT_MILLISECONDS))
            faked.debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
            assertEquals(listOf("initialize", "attach"), agent.commands())
        }
    }

    /** An exception's stop tells the user which exception stopped it, and a halt's that a halt did. */
    fun testAnExceptionStopSaysWhichException() {
        val stops = listOf(
            JsonObject().apply {
                addProperty("reason", "exception")
                addProperty("description", "ZeroDivide")
                addProperty("text", "division by zero")
            } to NewspeakBundle.message("debugger.stopped.exception", "ZeroDivide", "division by zero"),
            JsonObject().apply {
                addProperty("reason", "exception")
                addProperty("text", "halt")
            } to NewspeakBundle.message("debugger.stopped.halt"),
        )
        for ((body, message) in stops) {
            val reported = notifications()
            val faked = fakeSession()
            val agent = identifiedAgent(faked) { command, _ ->
                when (command) {
                    "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
                    "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                    else -> JsonObject()
                }
            }
            agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
            agent.event("stopped", body)
            faked.debugging.nextPause()
            assertEquals(message, reported.get(WAIT_SECONDS, TimeUnit.SECONDS))
        }
    }

    fun testAPeerThatNeverIdentifiesItsProcessEndsTheSession() {
        val reported = notifications()
        val faked = fakeSession()
        val agent = FakeAgent(faked.port, onAttach = {
            event("initialized")
            event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        })
        assertEquals(NewspeakBundle.message("debugger.error.unidentified"), reported.get(WAIT_SECONDS, TimeUnit.SECONDS))
        assertTrue("the process started is ended", faked.handler.waitFor(WAIT_MILLISECONDS))
        faked.debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertEquals(listOf("initialize", "attach"), agent.commands())
        assertTrue("no stop is shown", faked.debugging.pauses.isEmpty())
    }

    fun testATerminatedEventFromAPeerThatNeverIdentifiedItsProcessEndsTheSession() {
        val reported = notifications()
        val faked = fakeSession()
        FakeAgent(faked.port, onAttach = { event("terminated") })
        assertEquals(NewspeakBundle.message("debugger.error.unidentified"), reported.get(WAIT_SECONDS, TimeUnit.SECONDS))
        assertTrue("the process started is ended", faked.handler.waitFor(WAIT_MILLISECONDS))
        faked.debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    fun testAPeerThatClosesBeforeIdentifyingItsProcessEndsTheSession() {
        val reported = notifications()
        val faked = fakeSession()
        FakeAgent(faked.port, onAttach = { close() })
        assertEquals(NewspeakBundle.message("debugger.error.unidentified"), reported.get(WAIT_SECONDS, TimeUnit.SECONDS))
        assertTrue("the process started is ended", faked.handler.waitFor(WAIT_MILLISECONDS))
        faked.debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    fun testRunToCursorBeforeTheConnectionSaysTheSessionHasNotStarted() {
        val reported = notifications()
        val faked = fakeSession()
        val program = program("Stops", STOPS)
        val position = checkNotNull(XDebuggerUtil.getInstance().createPosition(program, 3))
        onEdt { faked.debugging.process.runToPosition(position, null) }
        assertEquals(NewspeakBundle.message("debugger.error.not.started"), reported.get(WAIT_SECONDS, TimeUnit.SECONDS))
    }

    fun testStopKeepsTheConnectionUntilTheTerminateIsAnswered() {
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { _, _ -> JsonObject() }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        val process = faked.debugging.process as NewspeakDebugProcess
        process.stop()
        process.terminateAnswered.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(agent.commands().toString(), agent.commands().contains("terminate"))
    }

    fun testTheShippedSourcesAreWrittenOnceAndReused() {
        val store = CoreLibraryStore(directory.resolve("store"))
        val first = store.ensure(runtime)
        assertTrue("written the first time", first.extracted)
        assertEquals(store.fingerprintOf(runtime), first.directory.fileName.toString())
        for (file in SHIPPED_FILES) {
            val path = first.directory.resolve(file)
            assertTrue(file, Files.isRegularFile(path))
            assertFalse("$file is read-only", Files.isWritable(path))
        }
        val written = Files.getLastModifiedTime(first.directory.resolve("Kernel.ns"))

        val second = store.ensure(runtime)
        assertFalse("reused the second time", second.extracted)
        assertEquals(first.directory, second.directory)
        assertEquals(written, Files.getLastModifiedTime(second.directory.resolve("Kernel.ns")))
        assertEquals("no staging directory is left", listOf(first.directory), Files.list(store.root).use { stream -> stream.toList() })
    }

    /**
     * The library under External Libraries is the settings' binary's
     * shipped sources, in `core-library/`'s layout: the modules and
     * `platform/` directly under the library's node, neither the
     * fingerprint's directory nor the store's own files.
     */
    fun testTheLibraryShowsTheShippedSourcesOfTheSettingsBinary() {
        val library = libraryDirectory()
        assertEquals(NewspeakCoreLibrary.STORE.root.resolve(CoreLibraryStore(directory).fingerprintOf(runtime)), library)
        val provider = AdditionalLibraryRootsProvider.EP_NAME.findExtensionOrFail(NewspeakCoreLibraryRootsProvider::class.java)
        val shown = provider.getAdditionalProjectLibraries(project).single() as NewspeakCoreLibraryRoots
        assertEquals("Newspeak Core Library", shown.presentableText)
        assertEquals(
            listOf("Kernel.ns", "Manifest.ns", "PlatformFactory.ns", "platform"),
            shown.sourceRoots.map { root -> root.name },
        )
        assertTrue(shown.sourceRoots.all { root -> root.parent.toNioPath() == library })
        val platform = shown.sourceRoots.single { root -> root.name == "platform" }
        assertTrue("platform/ is a folder", platform.isDirectory)
        val time = checkNotNull(platform.findChild("TimeFactory.ns"))
        val kernel = shown.sourceRoots.single { root -> root.name == "Kernel.ns" }
        val folder = checkNotNull(LocalFileSystem.getInstance().findFileByNioFile(library))
        val fingerprint = checkNotNull(folder.findChild(CoreLibraryStore.FINGERPRINT_FILE))
        val marker = checkNotNull(folder.findChild(CoreLibraryStore.MARKER))
        val index = ProjectFileIndex.getInstance(project)
        val inIndex = ReadAction.computeBlocking<List<Boolean>, RuntimeException> {
            listOf(
                index.isInLibrarySource(time),
                index.isInLibrarySource(kernel),
                index.isInLibrary(fingerprint),
                index.isInLibrary(marker),
                index.isInLibrary(folder),
            )
        }
        assertEquals("the modules are the library's source, the store's files and the fingerprint's directory not", listOf(true, true, false, false, false), inIndex)

        // What the project view shows under External Libraries.
        val tree = ReadAction.computeBlocking<Map<String, List<String>>, RuntimeException> {
            val node = ExternalLibrariesNode(project, ViewSettings.DEFAULT).children
                .single { child -> child is SyntheticLibraryElementNode && child.value == shown }
            fun nameOf(node: AbstractTreeNode<*>): String = (node.value as PsiFileSystemItem).name
            node.children.associate { child ->
                nameOf(child) to if (child is PsiDirectoryNode) child.children.map(::nameOf).sorted() else emptyList()
            }
        }
        assertEquals(
            mapOf(
                "Kernel.ns" to emptyList(),
                "Manifest.ns" to emptyList(),
                "PlatformFactory.ns" to emptyList(),
                "platform" to listOf(
                    "CollectionsFactory.ns",
                    "MirrorsFactory.ns",
                    "TimeFactory.ns",
                    "TranscriptFactory.ns",
                    "TypedArraysFactory.ns",
                ),
            ),
            tree,
        )
    }

    /**
     * A project that is no Newspeak project shows no library, though the
     * settings name a binary; its first debug session makes it one, and
     * shows it.
     */
    fun testAProjectThatIsNoNewspeakProjectShowsNoLibrary() {
        val library = NewspeakCoreLibrary.getInstance(project)
        library.forgetTheProjectIsNewspeak()
        assertNull(library.refresh().get(WAIT_SECONDS, TimeUnit.SECONDS))
        assertNull(library.directory)
        val provider = AdditionalLibraryRootsProvider.EP_NAME.findExtensionOrFail(NewspeakCoreLibraryRootsProvider::class.java)
        assertEquals(emptyList<Any>(), provider.getAdditionalProjectLibraries(project).toList())

        assertNotNull(libraryDirectory())
        assertEquals(1, provider.getAdditionalProjectLibraries(project).size)
    }

    /**
     * The pruning keeps the directory an open project shows however long
     * ago its use was recorded, and deletes one no project shows that has
     * been unused as long.
     */
    fun testThePruningKeepsTheDirectoryAnOpenProjectShows() {
        val library = libraryDirectory()
        val unused = NewspeakCoreLibrary.STORE.root.resolve(OLDER_FINGERPRINT)
        try {
            Files.createDirectories(unused)
            Files.writeString(unused.resolve(CoreLibraryStore.FINGERPRINT_FILE), "$OLDER_FINGERPRINT\n")
            val longAgo = Instant.now().minus(Duration.ofDays(400))
            NewspeakCoreLibrary.STORE.touch(unused, longAgo)
            NewspeakCoreLibrary.STORE.touch(library, longAgo)
            NewspeakCoreLibrary.pruneUnused(Instant.now())
            assertTrue("the open project's directory is kept", Files.isDirectory(library))
            assertFalse("the unused one is deleted", Files.exists(unused))
        } finally {
            NewspeakCoreLibrary.STORE.touch(library)
            if (Files.exists(unused)) CoreLibraryStore.deleteRecursively(unused)
        }
    }

    /**
     * The session hands the agent the library the project shows, absolute,
     * at `attach`; a shipped frame the agent then shows by the path of the
     * library's file opens that file, where its breakpoints stand, with no
     * `source` request.
     */
    fun testTheAttachHandsTheAgentTheLibraryWhoseFilesShippedFramesOpen() {
        val library = libraryDirectory()
        val kernel = library.resolve("Kernel.ns")
        val stack = JsonParser.parseString(SHIPPED_STACK).asJsonObject
        stack.getAsJsonArray("stackFrames")[0].asJsonObject.add(
            "source",
            JsonObject().apply {
                addProperty("name", "Kernel.ns")
                addProperty("path", kernel.toString())
            },
        )
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, _ ->
            when (command) {
                "stackTrace" -> stack
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertEquals(library.toAbsolutePath().toString(), agent.requests("attach").single().get("coreLibrary").asString)
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        val position = faked.debugging.nextPause()
        assertEquals(LocalFileSystem.getInstance().findFileByNioFile(kernel), position.file)
        assertEquals(1, position.line)
        assertFalse("no source is asked for", agent.commands().contains("source"))
    }

    /** `Array>>do:`'s loop body, which `WALKER`'s `do:` reaches, stops the program at a breakpoint in the library's file. */
    fun testABreakpointInTheLibrarysKernelStopsTheProgram() {
        val library = libraryDirectory()
        val kernel = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(library.resolve("Kernel.ns")))
        val body = arrayDoBody(library)
        val program = program("Walker", WALKER)
        toggleBreakpoint(kernel, line = body + 1)
        val debugging = debug(program)

        for (element in 1..2) {
            val position = debugging.nextPause()
            assertEquals("the stop at element $element", kernel, position.file)
            assertEquals(body, position.line)
            onEdt { debugging.session.resume() }
        }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertTrue(debugging.output(), debugging.output().contains("12"))
    }

    /**
     * A breakpoint in an older fingerprint's `Kernel.ns` -- the binary
     * rebuilt with other shipped sources since the library was shown -- is
     * moved by the session's look at the library before the session sends
     * it, and stops the program in the new library's file.
     */
    fun testASessionSendsTheBreakpointsMovedToTheBinarysLibrary() {
        val library = libraryDirectory()
        val older = NewspeakCoreLibrary.STORE.root.resolve(OLDER_FINGERPRINT)
        try {
            Files.createDirectories(older)
            // Only the library shown is a root of the project; the older directory is the test's own.
            VfsRootAccess.allowRootAccess(testRootDisposable, older.toString())
            Files.copy(library.resolve("Kernel.ns"), older.resolve("Kernel.ns"))
            Files.writeString(older.resolve(CoreLibraryStore.FINGERPRINT_FILE), "$OLDER_FINGERPRINT\n")
            val olderKernel = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(older.resolve("Kernel.ns")))
            val kernel = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(library.resolve("Kernel.ns")))
            val body = arrayDoBody(library)
            toggleBreakpoint(olderKernel, line = body + 1)
            onEdt { NewspeakCoreLibrary.getInstance(project).forgetTheDirectoryShown() }

            val debugging = debug(program("Walker", WALKER))
            val position = debugging.nextPause()
            assertEquals(kernel, position.file)
            assertEquals(body, position.line)
            val breakpoints = XDebuggerManager.getInstance(project).breakpointManager.allBreakpoints
                .filterIsInstance<XLineBreakpoint<*>>()
            assertEquals(listOf(kernel.url), breakpoints.map { breakpoint -> breakpoint.fileUrl })
            runInEdtAndWait {
                val manager = XDebuggerManager.getInstance(project).breakpointManager
                WriteAction.run<Throwable> { manager.allBreakpoints.forEach(manager::removeBreakpoint) }
            }
            onEdt { debugging.session.resume() }
            debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        } finally {
            CoreLibraryStore.deleteRecursively(older)
        }
    }

    /** The line, counted from 0, of `Array>>do:`'s loop body in [library]'s `Kernel.ns`. */
    private fun arrayDoBody(library: Path): Int {
        val lines = Files.readString(library.resolve("Kernel.ns")).lines()
        val size = lines.indexOfFirst { line -> line.contains("count = vmMirror collectionSize: self ifFail:") }
        assertTrue("Array>>do:", size >= 0)
        return size + lines.drop(size).indexOfFirst { line -> line.trim() == "action value: (self at: index)." }
    }

    /**
     * The library's `Manifest.ns` is no shipped module to the VM but the
     * program's own embedded one; a breakpoint there, sent by its name
     * too, stops the project's manifest as it answers a module.
     */
    fun testABreakpointInTheLibrarysManifestStopsAProject() {
        val library = libraryDirectory()
        val manifest = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(library.resolve("Manifest.ns")))
        val line = Files.readString(library.resolve("Manifest.ns")).lines()
            .indexOfFirst { text -> text.contains("((names at: index) = message selector) ifTrue:") }
        assertTrue("Manifest>>doesNotUnderstand:", line >= 0)
        program("App", SHARING)
        program("Shared", SHARED)
        Files.writeString(directory.resolve("newspeak.json"), """{"main": "App.ns", "modules": ["Shared.ns"]}""")
        val projectFile = checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(directory.resolve("newspeak.json")))
        toggleBreakpoint(manifest, line = line + 1)
        val debugging = debug(projectFile)

        val position = debugging.nextPause()
        assertEquals(manifest, position.file)
        assertEquals(line, position.line)
        val breakpoints = XDebuggerManager.getInstance(project).breakpointManager
        runInEdtAndWait { WriteAction.run<Throwable> { breakpoints.allBreakpoints.forEach(breakpoints::removeBreakpoint) } }
        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(debugging.output(), debugging.output().contains("hello"))
    }

    /** The project's library, written from the settings' binary if it is not yet. */
    private fun libraryDirectory(): Path =
        checkNotNull(NewspeakCoreLibrary.getInstance(project).refreshForDebugging().get(WAIT_SECONDS, TimeUnit.SECONDS)) { "no library" }

    /** A module text other than any shipped one is shown by reference, whether or not a library is shown. */
    fun testAModuleShownByReferenceIsOneReadOnlyFileAcrossStops() {
        libraryDirectory()
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, _ ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
                "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)

        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        val first = faked.debugging.nextPause()
        val file = first.file
        assertTrue(file.toString(), file is LightVirtualFile)
        assertEquals("Kernel.ns", file.name)
        assertFalse("a module's text is read-only", file.isWritable)
        assertEquals(KERNEL_TEXT, String(file.contentsToByteArray(), Charsets.UTF_8))
        assertEquals(1, first.line)

        // The shipped frame is listed, greyed; the program's own is not.
        val frames = framesOf(faked.debugging.session)
        assertEquals(listOf("Kernel>>value", "Program>>main:args:"), frames.map { frame -> textOf(frame).first })
        assertEquals(SimpleTextAttributes.GRAYED_ATTRIBUTES, textOf(frames[0]).second)
        assertEquals(SimpleTextAttributes.REGULAR_ATTRIBUTES, textOf(frames[1]).second)

        onEdt { faked.debugging.session.resume() }
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        val second = faked.debugging.nextPause()
        assertSame("one file per module for the session", file, second.file)
        assertEquals(1, agent.commands().count { command -> command == "source" })
    }

    fun testVariablesArePagedByAHundred() {
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, arguments ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
                "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                "scopes" -> JsonParser.parseString(
                    """{"scopes":[{"name":"Locals","presentationHint":"locals","variablesReference":$LOCALS_REFERENCE,"expensive":false}]}""",
                ).asJsonObject
                "variables" -> variablesAnswer(arguments)
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        faked.debugging.nextPause()

        val elements = childrenOf(checkNotNull(faked.debugging.session.currentStackFrame)).getValue("elements")
        val node = PagingNode()
        elements.computeChildren(node)
        val firstPage = node.pages.poll(WAIT_SECONDS, TimeUnit.SECONDS) ?: throw AssertionError("no first page")
        assertEquals((1..100).map { index -> "[$index]" }, firstPage)
        assertEquals("the elements not yet shown", 50, node.remaining.get(WAIT_SECONDS, TimeUnit.SECONDS))
        checkNotNull(node.next).run()
        val secondPage = node.pages.poll(WAIT_SECONDS, TimeUnit.SECONDS) ?: throw AssertionError("no second page")
        assertEquals((101..150).map { index -> "[$index]" }, secondPage)
        assertTrue("the second page is the last", node.last)
        val pageRequests = agent.requests("variables")
            .filter { arguments -> arguments.get("variablesReference").asInt == ELEMENTS_REFERENCE }
            .map { arguments -> arguments.get("start").asInt to arguments.get("count").asInt }
        assertEquals(listOf(0 to 100, 100 to 100), pageRequests)
    }

    fun testTheReturnValueIsShownAboveTheLocalsNamedByItsCall() {
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, arguments ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
                "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                "scopes" -> JsonParser.parseString(
                    """{"scopes":[
                        {"name":"Locals","presentationHint":"locals","variablesReference":$LOCALS_REFERENCE,"expensive":false},
                        {"name":"Return value","presentationHint":"returnValue","variablesReference":$RETURNED_REFERENCE,"expensive":false}
                    ]}""",
                ).asJsonObject
                "variables" -> if (arguments.get("variablesReference").asInt == RETURNED_REFERENCE) {
                    JsonParser.parseString(
                        """{"variables":[{"name":"(kinds at: which) value","value":"1","type":"Integer","variablesReference":0}]}""",
                    ).asJsonObject
                } else {
                    variablesAnswer(arguments)
                }
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", JsonObject().apply { addProperty("reason", "step") })
        faked.debugging.nextPause()

        // As Java's, the value the last call returned comes first, named by the call, drawn with the platform's icon of a return value.
        val children = childrenOf(checkNotNull(faked.debugging.session.currentStackFrame))
        assertEquals(listOf("(kinds at: which) value", "elements"), children.keys.toList())
        val returned = presentationOf(children.getValue("(kinds at: which) value"))
        assertEquals(Presentation(AllIcons.Debugger.WatchLastReturnValue, "Integer", "1", false), returned)
        assertNull("a local has no icon of its own", presentationOf(children.getValue("elements")).icon)
    }

    /** The agent's `variables` answer: one local, an `Array` of 150 elements, and its elements a page at a time. */
    private fun variablesAnswer(arguments: JsonObject): JsonObject {
        val variables = JsonArray()
        when (arguments.get("variablesReference").asInt) {
            LOCALS_REFERENCE -> variables.add(
                JsonObject().apply {
                    addProperty("name", "elements")
                    addProperty("value", "an Array")
                    addProperty("type", "Array")
                    addProperty("variablesReference", ELEMENTS_REFERENCE)
                    addProperty("indexedVariables", ELEMENT_COUNT)
                },
            )
            ELEMENTS_REFERENCE -> {
                val start = arguments.get("start").asInt
                val end = minOf(ELEMENT_COUNT, start + arguments.get("count").asInt)
                for (index in start until end) {
                    variables.add(
                        JsonObject().apply {
                            addProperty("name", "[${index + 1}]")
                            addProperty("value", (index * 10).toString())
                            addProperty("variablesReference", 0)
                        },
                    )
                }
            }
        }
        return JsonObject().apply { add("variables", variables) }
    }

    // ---- A fake agent ----

    /** A session over a process of the test's own, `ns-next` running a program undebugged, whose port no runtime connects to. */
    private class FakedSession(val debugging: Debugging, val handler: NewspeakDebuggeeProcessHandler, val port: Int)

    private fun fakeSession(): FakedSession {
        val program = program("LongLeaf", LONG_LEAF)
        val handler = NewspeakDebuggeeProcessHandler(GeneralCommandLine(runtime.toString(), "run", program.path))
        val serverSocket = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        val recorder = recordNextProcess()
        onEdt {
            XDebuggerManager.getInstance(project)
                .newSessionBuilder(
                    object : XDebugProcessStarter() {
                        override fun start(session: XDebugSession): XDebugProcess =
                            NewspeakDebugProcess(session, serverSocket, handler, null, SECRET)
                    },
                )
                .sessionName("fake agent")
                .showTab(true)
                .startSession()
        }
        return FakedSession(recorder.get(WAIT_SECONDS, TimeUnit.SECONDS), handler, serverSocket.localPort)
    }

    /** A fake agent that names the session's own process and returns its secret, as `ns-next`'s does. */
    private fun identifiedAgent(faked: FakedSession, answer: (String, JsonObject) -> JsonObject): FakeAgent =
        FakeAgent(
            faked.port,
            onAttach = {
                event("process", processBody(faked.handler.process.pid(), SECRET))
                event("initialized")
            },
            answer = answer,
        )

    /** A `process` event's body naming [processId] and returning [secret], if any. */
    private fun processBody(processId: Long, secret: String?) = JsonObject().apply {
        addProperty("systemProcessId", processId)
        secret?.let { returned -> addProperty("nsNextSecret", returned) }
    }

    /**
     * A scripted agent in place of `ns-next`'s: it answers every request
     * with what [answer] gives, a success, and after `attach` sends what
     * [onAttach] sends. Its connection writes in order, so its events
     * follow its answers as the real agent's do.
     */
    private inner class FakeAgent(
        port: Int,
        private val onAttach: FakeAgent.() -> Unit,
        private val answer: (String, JsonObject) -> JsonObject = { _, _ -> JsonObject() },
    ) {
        private val received = CopyOnWriteArrayList<JsonObject>()
        private val connection: DapConnection

        /** Completes when the session sent `configurationDone`. */
        val configured = CompletableFuture<Unit>()

        init {
            val socket = Socket(InetAddress.getLoopbackAddress(), port)
            connection = DapConnection(socket.getInputStream(), socket.getOutputStream(), socket)
            Disposer.register(testRootDisposable) { connection.close() }
            connection.start(
                { request ->
                    received.add(request)
                    val command = request.get("command").asString
                    val arguments = request.getAsJsonObject("arguments") ?: JsonObject()
                    connection.send(response(request).apply { add("body", answer(command, arguments)) })
                    if (command == "attach") onAttach()
                    if (command == "configurationDone") configured.complete(Unit)
                },
                { _ -> },
            )
        }

        fun event(name: String, body: JsonObject = JsonObject()) {
            connection.send(eventMessage(name, body))
        }

        /** Closes the connection, as a peer that goes away does. */
        fun close() {
            connection.close()
        }

        fun commands(): List<String> = received.map { request -> request.get("command").asString }

        fun requests(command: String): List<JsonObject> =
            received.filter { request -> request.get("command").asString == command }
                .map { request -> request.getAsJsonObject("arguments") }
    }

    /** The text of the next notification the project shows. */
    private fun notifications(): CompletableFuture<String> {
        val reported = CompletableFuture<String>()
        project.messageBus.connect(testRootDisposable).subscribe(
            Notifications.TOPIC,
            object : Notifications {
                override fun notify(notification: Notification) {
                    reported.complete(notification.content)
                }
            },
        )
        return reported
    }

    /** The frames of the current stop, all of them. */
    private fun framesOf(session: XDebugSession): List<XStackFrame> {
        val collected = CompletableFuture<List<XStackFrame>>()
        val frames = CopyOnWriteArrayList<XStackFrame>()
        checkNotNull(session.suspendContext).activeExecutionStack!!.computeStackFrames(
            0,
            object : XExecutionStack.XStackFrameContainer {
                override fun addStackFrames(stackFrames: MutableList<out XStackFrame>, last: Boolean) {
                    frames.addAll(stackFrames)
                    if (last) collected.complete(frames.toList())
                }

                override fun errorOccurred(errorMessage: String) {
                    collected.completeExceptionally(AssertionError(errorMessage))
                }

                override fun isObsolete(): Boolean = false
            },
        )
        return collected.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    /** A frame's name as the frames view draws it, and the attributes it draws it with. */
    private fun textOf(frame: XStackFrame): Pair<String, SimpleTextAttributes> {
        val fragments = mutableListOf<Pair<String, SimpleTextAttributes>>()
        frame.customizePresentation(
            object : ColoredTextContainer {
                override fun append(fragment: String, attributes: SimpleTextAttributes) {
                    fragments.add(fragment to attributes)
                }
            },
        )
        return fragments.first()
    }

    /** A node that records each page of children and the offer of the next. */
    private class PagingNode : XCompositeNode {
        val pages = LinkedBlockingQueue<List<String>>()
        val remaining = CompletableFuture<Int>()

        @Volatile
        var next: Runnable? = null

        @Volatile
        var last = false

        override fun addChildren(children: XValueChildrenList, last: Boolean) {
            this.last = last
            pages.add((0 until children.size()).map(children::getName))
        }

        @Deprecated("the platform's own deprecation; the tree calls it still")
        override fun tooManyChildren(remaining: Int) {
            this.remaining.complete(remaining)
        }

        override fun tooManyChildren(remaining: Int, addNextChildren: Runnable) {
            next = addNextChildren
            this.remaining.complete(remaining)
        }

        override fun setAlreadySorted(alreadySorted: Boolean) {}

        override fun setErrorMessage(errorMessage: String) {
            pages.add(listOf("error: $errorMessage"))
        }

        override fun setErrorMessage(errorMessage: String, link: XDebuggerTreeNodeHyperlink?) {
            pages.add(listOf("error: $errorMessage"))
        }

        override fun setMessage(message: String, icon: Icon?, attributes: SimpleTextAttributes, link: XDebuggerTreeNodeHyperlink?) {}
    }

    // ---- Starting a session ----

    /** One session the test started, and what the test waits for in it. */
    inner class Debugging(val process: XDebugProcess) {
        val session: XDebugSession get() = process.session
        val pauses = LinkedBlockingQueue<XSourcePosition>()
        val stopped = CompletableFuture<Unit>()
        private val text = StringBuffer()
        private val awaitedTexts = CopyOnWriteArrayList<Pair<String, CompletableFuture<Unit>>>()

        init {
            process.session.addSessionListener(
                object : XDebugSessionListener {
                    override fun sessionPaused() {
                        process.session.currentPosition?.let(pauses::add)
                    }

                    override fun sessionStopped() {
                        stopped.complete(Unit)
                    }
                },
            )
            process.processHandler.addProcessListener(
                object : ProcessListener {
                    override fun onTextAvailable(event: ProcessEvent, outputType: Key<*>) {
                        text.append(event.text)
                        for ((awaited, future) in awaitedTexts) if (text.contains(awaited)) future.complete(Unit)
                    }
                },
            )
        }

        fun nextPause(): XSourcePosition =
            pauses.poll(WAIT_SECONDS, TimeUnit.SECONDS) ?: throw AssertionError("no stop within $WAIT_SECONDS s: ${output()}")

        fun output(): String = text.toString()

        /** Completes once the process printed `awaited`. */
        fun printed(awaited: String): CompletableFuture<Unit> {
            val future = CompletableFuture<Unit>()
            awaitedTexts.add(awaited to future)
            if (text.contains(awaited)) future.complete(Unit)
            return future
        }
    }

    /**
     * The next debug process the platform starts in the project, as it is
     * started: the platform tells its listeners before it initializes the
     * session, so the test's listeners see every stop.
     */
    private fun recordNextProcess(): CompletableFuture<Debugging> {
        val started = CompletableFuture<Debugging>()
        project.messageBus.connect(testRootDisposable).subscribe(
            XDebuggerManager.TOPIC,
            object : XDebuggerManagerListener {
                override fun processStarted(debugProcess: XDebugProcess) {
                    if (started.isDone) return
                    val debugging = Debugging(debugProcess)
                    debugged.add(debugging)
                    started.complete(debugging)
                }
            },
        )
        return started
    }

    /** Debug on a Newspeak run configuration of `program`, as the user starts it. */
    private fun debug(program: VirtualFile): Debugging {
        val configuration = NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration
        configuration.program = program.path
        val environment = ExecutionEnvironmentBuilder.create(DefaultDebugExecutor.getDebugExecutorInstance(), configuration).build()
        assertTrue(environment.runner is NewspeakDebugRunner)
        val recorder = recordNextProcess()
        onEdt { environment.runner.execute(environment) }
        return recorder.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    private fun program(name: String, source: String): VirtualFile {
        val path = directory.resolve("$name.ns")
        Files.writeString(path, source)
        return checkNotNull(LocalFileSystem.getInstance().refreshAndFindFileByNioFile(path))
    }

    /**
     * A line breakpoint on `line`, counted from 1, once it exists: the
     * platform adds a toggled breakpoint later, and a session started
     * before it would let the program run past the line.
     */
    private fun toggleBreakpoint(file: VirtualFile, line: Int) {
        val added = CompletableFuture<Unit>()
        val connection = project.messageBus.connect(testRootDisposable)
        connection.subscribe(
            XBreakpointListener.TOPIC,
            object : XBreakpointListener<XBreakpoint<*>> {
                override fun breakpointAdded(breakpoint: XBreakpoint<*>) {
                    added.complete(Unit)
                }
            },
        )
        onEdt { XDebuggerUtil.getInstance().toggleLineBreakpoint(project, file, line - 1) }
        added.get(WAIT_SECONDS, TimeUnit.SECONDS)
        connection.disconnect()
    }

    private fun onEdt(action: () -> Unit) = runInEdtAndWait { action() }

    // ---- Reading frames and values ----

    /** The children `container` computes, by name, in order. */
    private fun childrenOf(container: XValueContainer): Map<String, XValue> {
        val node = CollectingNode()
        container.computeChildren(node)
        return node.collected.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    private class CollectingNode : XCompositeNode {
        val collected = CompletableFuture<Map<String, XValue>>()
        private val children = LinkedHashMap<String, XValue>()

        override fun addChildren(children: XValueChildrenList, last: Boolean) {
            for (value in children.topValues) this.children[value.name] = value
            for (index in 0 until children.size()) this.children[children.getName(index)] = children.getValue(index)
            if (last) collected.complete(this.children)
        }

        @Deprecated("the platform's own deprecation; the tree calls it still")
        override fun tooManyChildren(remaining: Int) {
            collected.complete(children)
        }

        override fun tooManyChildren(remaining: Int, addNextChildren: Runnable) {
            collected.complete(children)
        }

        override fun setAlreadySorted(alreadySorted: Boolean) {}

        override fun setErrorMessage(errorMessage: String) {
            collected.completeExceptionally(AssertionError(errorMessage))
        }

        override fun setErrorMessage(errorMessage: String, link: XDebuggerTreeNodeHyperlink?) {
            collected.completeExceptionally(AssertionError(errorMessage))
        }

        override fun setMessage(message: String, icon: Icon?, attributes: SimpleTextAttributes, link: XDebuggerTreeNodeHyperlink?) {}
    }

    private data class Presentation(val icon: Icon?, val type: String?, val value: String, val hasChildren: Boolean)

    private fun presentationOf(value: XValue): Presentation {
        val presented = CompletableFuture<Presentation>()
        value.computePresentation(
            object : XValueNode {
                override fun setPresentation(icon: Icon?, type: String?, value: String, hasChildren: Boolean) {
                    presented.complete(Presentation(icon, type, value, hasChildren))
                }

                override fun setPresentation(icon: Icon?, presentation: XValuePresentation, hasChildren: Boolean) {
                    val text = StringBuilder()
                    presentation.renderValue(
                        object : XValuePresentation.XValueTextRenderer {
                            override fun renderValue(value: String) { text.append(value) }
                            override fun renderValue(value: String, key: com.intellij.openapi.editor.colors.TextAttributesKey) { text.append(value) }
                            override fun renderStringValue(value: String) { text.append(value) }
                            override fun renderStringValue(value: String, additionalSpecialCharsToHighlight: String?, maxLength: Int) { text.append(value) }
                            override fun renderNumericValue(value: String) { text.append(value) }
                            override fun renderKeywordValue(value: String) { text.append(value) }
                            override fun renderComment(comment: String) {}
                            override fun renderSpecialSymbol(symbol: String) { text.append(symbol) }
                            override fun renderError(error: String) { text.append(error) }
                        },
                    )
                    presented.complete(Presentation(icon, presentation.type, text.toString(), hasChildren))
                }

                override fun setFullValueEvaluator(fullValueEvaluator: XFullValueEvaluator) {}

                override fun isObsolete(): Boolean = false
            },
            XValuePlace.TREE,
        )
        return presented.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    private fun response(request: JsonObject) = JsonObject().apply {
        addProperty("seq", 0)
        addProperty("type", "response")
        addProperty("request_seq", request.get("seq").asInt)
        addProperty("command", request.get("command").asString)
        addProperty("success", true)
    }

    private fun eventMessage(name: String, body: JsonObject) = JsonObject().apply {
        addProperty("seq", 0)
        addProperty("type", "event")
        addProperty("event", name)
        add("body", body)
    }

    private companion object {
        const val WAIT_SECONDS = 30L

        /** The secret the fake sessions' processes were given, which an identified fake agent returns. */
        const val SECRET = "00112233445566778899aabbccddeeff"
        const val WAIT_MILLISECONDS = 30_000L

        /** How long a program inside one leaf must outlive the terminate: it would end at once otherwise. */
        const val LEAF_OUTLIVES_MILLISECONDS = 1_000L

        /** The fake agent's references: a frame's locals, and an `Array` among them with [ELEMENT_COUNT] elements. */
        const val LOCALS_REFERENCE = 100
        const val ELEMENTS_REFERENCE = 200
        const val ELEMENT_COUNT = 150

        /** The fake agent's reference of a step's stop's "Return value" scope. */
        const val RETURNED_REFERENCE = 300

        /** A shipped module's text, which the fake agent names by a reference only. */
        val KERNEL_TEXT = """
            class Kernel = (
              value = (
                ^self
              )
            )
        """.trimIndent() + "\n"

        /** A stop in it: its shipped frame hidden by default, above the program's own, placed nowhere. */
        val SHIPPED_STACK = """
            {"stackFrames":[
              {"id":1,"name":"Kernel>>value","source":{"name":"Kernel.ns","sourceReference":7},"line":2,"column":3,"presentationHint":"subtle"},
              {"id":2,"name":"Program>>main:args:","line":0,"column":0}
            ],"totalFrames":2}
        """.trimIndent()

        /** A fingerprint no binary has, an older library's in the tests. */
        const val OLDER_FINGERPRINT = "ffffffffffffffff"

        /** The files the store writes, each a shipped source. */
        val SHIPPED_FILES = listOf(
            "Kernel.ns",
            "Manifest.ns",
            "PlatformFactory.ns",
            "platform/TranscriptFactory.ns",
            "platform/TimeFactory.ns",
            "platform/CollectionsFactory.ns",
            "platform/TypedArraysFactory.ns",
            "platform/MirrorsFactory.ns",
        )

        /** A root whose `do:` runs `Array>>do:` over two elements. */
        val WALKER = """
            class Walker = Object new () (
              public main: platform args: arguments = (
                {1. 2} do: [:each | platform transcript show: each printString].
                platform transcript newLine.
              )
            ) : ()
        """.trimIndent() + "\n"

        /** A project's root that reaches `Shared` through its manifest. */
        val SHARING = """
            class App packageUsing: manifest = Object new (
              | Shared = manifest Shared. |
            ) (
              public main: platform args: arguments = (
                platform transcript show: Shared new greeting; newLine.
              )
            ) : ()
        """.trimIndent() + "\n"

        val SHARED = """
            class Shared = Object new () (
              public greeting = (
                ^'hello'
              )
            ) : ()
        """.trimIndent() + "\n"

        /** A root with two locals; line 6 prints the count. */
        val STOPS = """
            class Stops = Object new () (
              public main: platform args: arguments = (
                | count elements |
                count:: 3.
                elements:: {10. 20. 30}.
                platform transcript show: count printString; newLine.
              )
            ) : ()
        """.trimIndent() + "\n"

        /**
         * A root that says it computes, then spends long in one leaf: the
         * decimal digits of a number of two million digits, half a minute
         * in a debug build, written in one leaf. The number is made before
         * the line is printed, since making it reaches safepoints.
         */
        val LONG_LEAF = """
            class LongLeaf = Object new () (
              public main: platform args: arguments = (
                | big = 10 raisedTo: 2000000. |
                platform transcript show: 'computing'; newLine.
                big printString.
                platform transcript show: 'done'; newLine.
              )
            ) : ()
        """.trimIndent() + "\n"
    }
}
