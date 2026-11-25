package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonArray
import com.google.gson.JsonNull
import com.google.gson.JsonObject
import com.google.gson.JsonParser
import com.google.gson.JsonPrimitive
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
import com.intellij.notification.Notification
import com.intellij.notification.Notifications
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.actionSystem.impl.SimpleDataContext
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.impl.DocumentMarkupModel
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.roots.AdditionalLibraryRootsProvider
import com.intellij.openapi.roots.ProjectFileIndex
import com.intellij.openapi.ui.TestDialogManager
import com.intellij.openapi.ui.TestInputDialog
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.Key
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.vfs.newvfs.impl.VfsRootAccess
import com.intellij.psi.PsiFileSystemItem
import com.intellij.testFramework.LightVirtualFile
import com.intellij.testFramework.PlatformTestUtil
import com.intellij.testFramework.TestActionEvent
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.testFramework.runInEdtAndGet
import com.intellij.testFramework.runInEdtAndWait
import com.intellij.ui.ColoredTextContainer
import com.intellij.ui.SimpleTextAttributes
import com.intellij.util.ThreeState
import com.intellij.xdebugger.XDebugProcess
import com.intellij.xdebugger.XDebugProcessStarter
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebugSessionListener
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.XDebuggerManagerListener
import com.intellij.xdebugger.XDebuggerUtil
import com.intellij.xdebugger.XSourcePosition
import com.intellij.xdebugger.breakpoints.SuspendPolicy
import com.intellij.xdebugger.breakpoints.XBreakpoint
import com.intellij.xdebugger.breakpoints.XBreakpointListener
import com.intellij.xdebugger.breakpoints.XLineBreakpoint
import com.intellij.xdebugger.evaluation.EvaluationMode
import com.intellij.xdebugger.evaluation.XDebuggerEvaluator
import com.intellij.xdebugger.frame.XCompositeNode
import com.intellij.xdebugger.frame.XDebuggerTreeNodeHyperlink
import com.intellij.xdebugger.frame.XExecutionStack
import com.intellij.xdebugger.frame.XFullValueEvaluator
import com.intellij.xdebugger.frame.XStackFrame
import com.intellij.xdebugger.frame.XValue
import com.intellij.xdebugger.frame.XValueChildrenList
import com.intellij.xdebugger.frame.XValueContainer
import com.intellij.xdebugger.frame.XValueModifier
import com.intellij.xdebugger.frame.XValueNode
import com.intellij.xdebugger.frame.XValuePlace
import com.intellij.xdebugger.frame.presentation.XValuePresentation
import com.intellij.xdebugger.impl.settings.XDebuggerSettingManagerImpl
import com.intellij.xdebugger.ui.DebuggerColors
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.debugger.dap.DapInstanceCount
import io.github.bstrauss.newspeak.devtool.editor.CodeListingFile
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
import java.util.concurrent.ExecutionException
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
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
        NewspeakDebuggerSettings.getInstance().update(false)
        NewspeakSkippedClasses.getInstance(project).update(emptyList())
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
                // A stop opens its file later, on the event queue: those already queued open before the close,
                // or an editor a slow machine opened after it outlives the test.
                PlatformTestUtil.dispatchAllInvocationEventsInIdeEventQueue()
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
            NewspeakDebuggerSettings.getInstance().update(false)
            NewspeakSkippedClasses.getInstance(project).update(emptyList())
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

        val before = checkNotNull(debugging.session.currentStackFrame).equalityObject
        onEdt { debugging.session.stepOver(false) }
        val stepped = debugging.nextPause()
        assertEquals(program, stepped.file)
        assertEquals("line 5, counted from 0 by the platform", 4, stepped.line)
        // The platform keeps the variables view's expanded nodes for the frame it takes for the same.
        assertEquals("the stepping frame is the same frame", before, checkNotNull(debugging.session.currentStackFrame).equalityObject)

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

    /**
     * The session sends the settings' step filters before it lets the
     * program start, and again, at once, whenever the settings change.
     */
    fun testTheSessionSendsTheStepFiltersAtItsStartAndAtEveryChange() {
        val walker = program("Walker", WALKER)
        val skipped = NewspeakSkippedClasses.getInstance(project)
        NewspeakDebuggerSettings.getInstance().update(true)
        skipped.update(listOf(skipped.entryFor(walker, listOf("Inner"), projectDirectory = null)))
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { _, _ -> JsonObject() }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        val commands = agent.commands()
        assertTrue(commands.toString(), commands.indexOf("nsNext/setStepFilters") in 0 until commands.indexOf("configurationDone"))
        val path = JsonPrimitive(walker.toNioPath().toString())
        assertEquals(
            listOf("""{"shipped":false,"entries":[{"source":{"path":$path},"classPath":["Inner"]}]}"""),
            agent.requests("nsNext/setStepFilters").map(JsonObject::toString),
        )

        onEdt { NewspeakDebuggerSettings.getInstance().update(false) }
        awaitRequests(agent, "nsNext/setStepFilters", 2)
        onEdt { skipped.update(emptyList()) }
        awaitRequests(agent, "nsNext/setStepFilters", 3)
        assertEquals(
            listOf(
                """{"shipped":true,"entries":[{"source":{"path":$path},"classPath":["Inner"]}]}""",
                """{"shipped":true,"entries":[]}""",
            ),
            agent.requests("nsNext/setStepFilters").drop(1).map(JsonObject::toString),
        )
    }

    /**
     * A class of the project's step filters keeps a step into from going
     * into it: with `Helper` filtered, a step into `Helper new run` passes
     * the factory and `run` and stops at the next statement of `main:args:`.
     */
    fun testAStepIntoPassesAFilteredProgramClass() {
        val program = program("Helping", HELPING)
        val skipped = NewspeakSkippedClasses.getInstance(project)
        skipped.update(listOf(skipped.entryFor(program, listOf("Helper"), projectDirectory = null)))
        toggleBreakpoint(program, line = 3)
        val debugging = debug(program)
        assertEquals(2, debugging.nextPause().line)

        onEdt { debugging.session.stepInto() }
        val stepped = debugging.nextPause()
        assertEquals(program, stepped.file)
        assertEquals("line 4, counted from 0 by the platform", 3, stepped.line)

        val breakpoints = XDebuggerManager.getInstance(project).breakpointManager
        runInEdtAndWait { WriteAction.run<Throwable> { breakpoints.allBreakpoints.forEach(breakpoints::removeBreakpoint) } }
        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(debugging.output(), debugging.output().contains("ran"))
    }

    /**
     * The debug tool window's toggle, shown for a Newspeak session, lets a
     * step into go into the shipped modules at once: with the filter on, a
     * step into `{1. 2} do: [...]` stops in the block, with it off the
     * next step into stops in a shipped module's file.
     */
    fun testTheToggleLetsAStepIntoGoIntoTheShippedModules() {
        libraryDirectory()
        val program = program("Walker", WALKER)
        toggleBreakpoint(program, line = 3)
        val debugging = debug(program)
        assertEquals(2, debugging.nextPause().line)

        val action = NewspeakStepIntoShippedAction()
        val context = SimpleDataContext.builder()
            .add(CommonDataKeys.PROJECT, project)
            .add(XDebugSession.DATA_KEY, debugging.session)
            .build()
        val event = TestActionEvent.createTestEvent(action, context)
        action.update(event)
        assertTrue("shown for a Newspeak session", event.presentation.isEnabledAndVisible)
        assertFalse("steps pass the shipped modules by default", action.isSelected(event))

        onEdt { debugging.session.stepInto() }
        val filtered = debugging.nextPause()
        assertEquals("the block's first statement", program, filtered.file)

        onEdt { action.setSelected(event, true) }
        assertTrue(action.isSelected(event))
        onEdt { debugging.session.stepInto() }
        val shipped = debugging.nextPause()
        assertTrue(shipped.file.path, shipped.file.name in SHIPPED_FILES.map { file -> file.substringAfterLast('/') })

        val breakpoints = XDebuggerManager.getInstance(project).breakpointManager
        runInEdtAndWait { WriteAction.run<Throwable> { breakpoints.allBreakpoints.forEach(breakpoints::removeBreakpoint) } }
        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    /**
     * The Evaluate dialog's evaluator, the frame's: an expression over a
     * local answers its value, with children where it has some; one that
     * does not compile answers the agent's reason as its error. A hover
     * offers a local's name, and no other, since any other may be a send.
     */
    fun testEvaluateAnswersALocalsValue() {
        val program = program("Stops", STOPS)
        toggleBreakpoint(program, line = 6)
        val debugging = debug(program)
        debugging.nextPause()
        val frame = checkNotNull(debugging.session.currentStackFrame)

        assertEquals(Presentation(null, "Integer", "4", false), presentationOf(evaluated(frame, "count + 1")))
        val elements = evaluated(frame, "elements")
        assertTrue(presentationOf(elements).hasChildren)
        assertEquals(listOf("[1]", "[2]", "[3]"), childrenOf(elements).keys.toList())
        assertTrue(evaluationError(frame, "count +").startsWith("the expression does not compile"))

        val document = EditorFactory.getInstance().createDocument("count printString. platform nothing")
        val evaluator = checkNotNull(frame.evaluator)
        fun hovered(offset: Int): String? =
            evaluator.getExpressionInfoAtOffsetAsync(project, document, offset, false).blockingGet(WAIT_SECONDS.toInt() * 1000)
                ?.let { info -> info.textRange.subSequence(document.charsSequence).toString() }
        assertEquals("count", hovered(2))
        assertNull("a send's name is not hovered", hovered(8))
        assertEquals("a parameter is a variable", "platform", hovered(22))
        assertNull("nor a name no variable has", hovered(30))

        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    /**
     * A watch is evaluated at every stop by the evaluator of the stop's
     * frame, which answers that stop's value: a loop's index, at each of
     * its three stops.
     */
    fun testAWatchIsEvaluatedAtEachStop() {
        val program = program("Loop", LOOP)
        toggleBreakpoint(program, line = 4)
        val debugging = debug(program)
        val watched = (1..3).map { _ ->
            debugging.nextPause()
            val value = presentationOf(evaluated(checkNotNull(debugging.session.currentStackFrame), "index * 10")).value
            onEdt { debugging.session.resume() }
            value
        }
        assertEquals(listOf("10", "20", "30"), watched)
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    /**
     * Set Value on a frame's variable sets it in the program, which goes
     * on with the value; a value's child is not set, nor is the value of
     * an evaluation.
     */
    fun testSetValueChangesTheProgramsOutput() {
        val program = program("Stops", STOPS)
        toggleBreakpoint(program, line = 6)
        val debugging = debug(program)
        debugging.nextPause()
        val frame = checkNotNull(debugging.session.currentStackFrame)
        val locals = childrenOf(frame)
        val modified = CompletableFuture<Unit>()
        checkNotNull(locals.getValue("count").modifier) { "a frame's variable is set" }.setValue(
            XDebuggerUtil.getInstance().createExpression("6 * 7", null, null, EvaluationMode.EXPRESSION),
            object : XValueModifier.XModificationCallback {
                override fun valueModified() {
                    modified.complete(Unit)
                }

                override fun errorOccurred(errorMessage: String) {
                    modified.completeExceptionally(AssertionError(errorMessage))
                }
            },
        )
        modified.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertNull("an element is no frame's variable", childrenOf(locals.getValue("elements")).getValue("[1]").modifier)
        assertNull("nor is an evaluation's value", evaluated(frame, "count").modifier)

        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertTrue(debugging.output(), debugging.output().contains("42"))
    }

    /**
     * Smart Step Into offers the sends of the stop's line in the order the
     * line runs them, each highlighted in the editor, and steps into the
     * one chosen: `first:`, whose argument `second` runs first.
     */
    fun testSmartStepIntoOffersTheLinesCalls() {
        val program = program("Smart", SMART)
        toggleBreakpoint(program, line = 4)
        val debugging = debug(program)
        val position = debugging.nextPause()

        val handler = debugging.process.smartStepIntoHandler as NewspeakSmartStepIntoHandler
        val variants = checkNotNull(handler.computeSmartStepVariantsAsync(position).blockingGet(WAIT_SECONDS.toInt() * 1000))
        assertEquals(listOf("second", "first:"), variants.map { variant -> variant.text })
        val text = String(program.contentsToByteArray(), Charsets.UTF_8)
        for (variant in variants) {
            val range = checkNotNull(variant.highlightRange) { "${variant.text} is highlighted" }
            assertTrue(range.substring(text), range.substring(text).contains(variant.text.removeSuffix(":")))
        }

        onEdt { handler.startStepInto(variants[1], debugging.session.suspendContext) }
        val stepped = debugging.nextPause()
        assertEquals("first:'s statement, line 8, counted from 0 by the platform", 7, stepped.line)
        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(debugging.output(), debugging.output().contains("42"))
    }

    /**
     * Reset Frame restarts the selected method: it runs again from its
     * first statement, and the program goes on from there, printing twice
     * what the method prints before its return.
     */
    fun testDropFrameRestartsTheMethod() {
        val program = program("Restarting", RESTARTING)
        toggleBreakpoint(program, line = 7)
        val debugging = debug(program)
        assertEquals(6, debugging.nextPause().line)

        val handler = checkNotNull(debugging.process.dropFrameHandler)
        val frame = checkNotNull(debugging.session.currentStackFrame)
        assertEquals(ThreeState.YES, handler.canDropFrame(frame))
        onEdt { handler.drop(frame) }
        assertEquals("compute:'s first statement, line 6", 5, debugging.nextPause().line)

        val breakpoints = XDebuggerManager.getInstance(project).breakpointManager
        runInEdtAndWait { WriteAction.run<Throwable> { breakpoints.allBreakpoints.forEach(breakpoints::removeBreakpoint) } }
        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(debugging.output(), debugging.process.processHandler.waitFor(WAIT_MILLISECONDS))
        assertTrue(debugging.output(), Regex("""in\s+in\s+42""").containsMatchIn(debugging.output()))
    }

    /**
     * A marked object is named in an evaluation by its label and the
     * agent's suffix, and shown with its label; unmarked, the name names
     * nothing.
     */
    fun testAMarkedObjectIsNamedInAnEvaluation() {
        val program = program("Stops", STOPS)
        toggleBreakpoint(program, line = 6)
        val debugging = debug(program)
        debugging.nextPause()
        val markers = (debugging.process as NewspeakDebugProcess).markers
        val elements = childrenOf(checkNotNull(debugging.session.currentStackFrame)).getValue("elements") as NewspeakValue
        assertNull(markers.labelOf(elements))
        // Mark Object…, as the value tree's menu runs it, the label typed in its dialog.
        val action = NewspeakMarkObjectAction()
        val event = sessionEvent(action, debugging.session)
        assertTrue(action.isEnabled(elements, "elements", event))
        TestDialogManager.setTestInputDialog(TestInputDialog { _ -> "numbers" })
        try {
            onEdt { action.perform(elements, "elements", event) }
        } finally {
            TestDialogManager.setTestInputDialog(TestInputDialog.DEFAULT)
        }
        // The mark is answered asynchronously; once the agent took it, every stop shows the object with its label.
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(WAIT_SECONDS)
        while (markers.labelOf(childrenOf(checkNotNull(debugging.session.currentStackFrame)).getValue("elements") as NewspeakValue) == null) {
            if (System.nanoTime() > deadline) throw AssertionError("the mark was not taken within $WAIT_SECONDS s")
            Thread.sleep(POLL_MILLISECONDS)
        }

        val frame = checkNotNull(debugging.session.currentStackFrame)
        assertEquals("3", presentationOf(evaluated(frame, "numbers_DebugLabel size")).value)
        val shown = childrenOf(frame).getValue("elements") as NewspeakValue
        assertTrue(shown.variable.value, shown.variable.value.endsWith(" [numbers]"))
        assertEquals("numbers", markers.labelOf(shown))
        assertFalse("a label the agent would refuse", NewspeakValueMarkers.isLabel("numbers_DebugLabel"))

        markers.unmark("numbers").get(WAIT_SECONDS, TimeUnit.SECONDS)
        evaluationError(frame, "numbers_DebugLabel size")
        onEdt { debugging.session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
    }

    /**
     * Force Return sends the frame and the expression the user gave, none
     * for a blank one, and is offered on a program's frame but not on one
     * of the core library, which the agent says it cannot end.
     */
    fun testForceReturnSendsTheExpression() {
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, _ ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(RESTARTABLE_STACK).asJsonObject
                "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        faked.debugging.nextPause()
        val session = faked.debugging.session
        val frames = framesOf(session)

        val action = NewspeakForceReturnAction()
        fun enabledOn(frame: XStackFrame): Boolean {
            onEdt { session.setCurrentStackFrame(checkNotNull(session.suspendContext).activeExecutionStack!!, frame) }
            val context = SimpleDataContext.builder().add(CommonDataKeys.PROJECT, project).add(XDebugSession.DATA_KEY, session).build()
            val event = TestActionEvent.createTestEvent(action, context)
            action.update(event)
            assertTrue("shown for a Newspeak session", event.presentation.isVisible)
            return event.presentation.isEnabled
        }
        assertFalse("not on a frame of the core library", enabledOn(frames[0]))
        assertTrue(enabledOn(frames[1]))

        val process = faked.debugging.process as NewspeakDebugProcess
        checkNotNull(process.forceReturn(frames[1] as NewspeakStackFrame, "3 + 4")).get(WAIT_SECONDS, TimeUnit.SECONDS)
        checkNotNull(process.forceReturn(frames[1] as NewspeakStackFrame, "  ")).get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertEquals(
            listOf("""{"frameId":2,"expression":"3 + 4"}""", """{"frameId":2}"""),
            agent.requests("nsNext/forceReturn").map(JsonObject::toString),
        )
    }

    /**
     * Show Frame Code opens a read-only tab with the selected frame's
     * bytecode, marked where the frame stands, and the tab follows: the
     * caller's frame, selected, shows the caller's body, and a step moves
     * the mark.
     */
    fun testShowFrameCodeOpensTheFramesCodeAndFollowsSteps() {
        val program = program("Restarting", RESTARTING)
        toggleBreakpoint(program, line = 6)
        val debugging = debug(program)
        debugging.nextPause()
        val session = debugging.session
        val frames = framesOf(session)

        val action = ShowFrameCodeAction()
        val context = SimpleDataContext.builder().add(CommonDataKeys.PROJECT, project).add(XDebugSession.DATA_KEY, session).build()
        val event = TestActionEvent.createTestEvent(action, context)
        action.update(event)
        assertTrue(event.presentation.isEnabled)
        onEdt { action.actionPerformed(event) }

        fun text(): String? = ReadAction.compute<String?, Throwable> {
            FileEditorManager.getInstance(project).openFiles.filterIsInstance<CodeListingFile>().singleOrNull()
                ?.let { file -> FileDocumentManager.getInstance().getDocument(file)?.text }
        }
        fun shown(what: String, accepts: (String) -> Boolean): String {
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(WAIT_SECONDS)
            while (true) {
                text()?.takeIf(accepts)?.let { return it }
                if (System.nanoTime() > deadline) throw AssertionError("$what not shown within $WAIT_SECONDS s: ${text()}")
                Thread.sleep(POLL_MILLISECONDS)
            }
        }
        fun mark(listing: String): Int? = Regex("""(\d+)->""").find(listing.substringBefore("--- end"))?.groupValues?.get(1)?.toInt()

        val callee = shown("the callee's bytecode") { listing -> listing.contains("--- bytecode {Restarting>>compute:") }
        val before = checkNotNull(mark(callee)) { callee }
        assertTrue(callee, callee.contains(NewspeakBundle.message("devtool.frame.interpreted")) || callee.contains("--- code {"))

        onEdt { session.setCurrentStackFrame(checkNotNull(session.suspendContext).activeExecutionStack!!, frames[1]) }
        shown("the caller's bytecode") { listing -> listing.contains("--- bytecode {Restarting>>main:args:") }

        onEdt { session.setCurrentStackFrame(checkNotNull(session.suspendContext).activeExecutionStack!!, frames[0]) }
        shown("the callee's bytecode again") { listing -> listing.contains("--- bytecode {Restarting>>compute:") }
        onEdt { session.stepOver(false) }
        debugging.nextPause()
        val after = shown("a moved mark") { listing -> listing.contains("Restarting>>compute:") && mark(listing) != before }
        assertTrue(after, mark(after)!! > before)
        // The marked line is coloured as the execution point, the one line.
        val tab = runInEdtAndGet { FileEditorManager.getInstance(project).openFiles.filterIsInstance<CodeListingFile>().single() }
        fun executionLines(): List<Int> = runInEdtAndGet {
            val document = checkNotNull(FileDocumentManager.getInstance().getDocument(tab))
            DocumentMarkupModel.forDocument(document, project, true).allHighlighters
                .filter { highlighter -> highlighter.textAttributesKey == DebuggerColors.EXECUTIONPOINT_ATTRIBUTES }
                .map { highlighter -> document.getLineNumber(highlighter.startOffset) }
        }
        assertEquals(listOf(FrameCodeFollower.markedLine(after)), executionLines())

        onEdt { session.resume() }
        debugging.stopped.get(WAIT_SECONDS, TimeUnit.SECONDS)
        // The follower's end takes the colour away; the tab stays open with its last text.
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(WAIT_SECONDS)
        while (executionLines().isNotEmpty()) {
            if (System.nanoTime() > deadline) throw AssertionError("the execution point's colour stays after the session's end")
            Thread.sleep(POLL_MILLISECONDS)
        }
        assertTrue(runInEdtAndGet { FileEditorManager.getInstance(project).isFileOpen(tab) })
    }

    /** The frame-code tab colours no line while the program runs, and colours the marked one again at the next stop. */
    fun testTheFrameCodeLineIsUncolouredWhileTheProgramRuns() {
        val faked = fakeSession()
        val listing = "--- bytecode {Demo>>run}\n   0   stmt 0  LoadLiteral r1, 1\n   1-> stmt 0  Return r1\n--- end\n"
        val agent = identifiedAgent(faked) { command, _ ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
                "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                "nsNext/frameCode" -> JsonObject().apply {
                    addProperty("body", "Demo>>run")
                    addProperty("bytecode", listing)
                    add("machineCode", JsonNull.INSTANCE)
                }
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        faked.debugging.nextPause()
        val session = faked.debugging.session
        framesOf(session)
        val action = ShowFrameCodeAction()
        val context = SimpleDataContext.builder().add(CommonDataKeys.PROJECT, project).add(XDebugSession.DATA_KEY, session).build()
        onEdt { action.actionPerformed(TestActionEvent.createTestEvent(action, context)) }
        fun executionLines(): List<Int> = runInEdtAndGet {
            val tab = FileEditorManager.getInstance(project).openFiles.filterIsInstance<CodeListingFile>().singleOrNull()
                ?: return@runInEdtAndGet emptyList()
            val document = checkNotNull(FileDocumentManager.getInstance().getDocument(tab))
            DocumentMarkupModel.forDocument(document, project, true).allHighlighters
                .filter { highlighter -> highlighter.textAttributesKey == DebuggerColors.EXECUTIONPOINT_ATTRIBUTES }
                .map { highlighter -> document.getLineNumber(highlighter.startOffset) }
        }
        fun awaitLines(what: String, expected: List<Int>) {
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(WAIT_SECONDS)
            while (executionLines() != expected) {
                if (System.nanoTime() > deadline) throw AssertionError("$what: ${executionLines()}")
                Thread.sleep(POLL_MILLISECONDS)
            }
        }
        val marked = listOf(checkNotNull(FrameCodeFollower.markedLine(FrameCodeFollower.text(FrameCode("Demo>>run", listing, null)))))
        awaitLines("the stop's line coloured", marked)
        onEdt { session.resume() }
        awaitLines("no line coloured while the program runs", emptyList())
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        faked.debugging.nextPause()
        awaitLines("the next stop's line coloured again", marked)
    }

    /** A refused frame code is told in a notification with the agent's words, and opens no tab. */
    fun testShowFrameCodeRefusedIsNotified() {
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, _ ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
                "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                "nsNext/frameCode" -> null
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        faked.debugging.nextPause()
        val session = faked.debugging.session
        framesOf(session)
        val reported = notifications()
        val action = ShowFrameCodeAction()
        val context = SimpleDataContext.builder().add(CommonDataKeys.PROJECT, project).add(XDebugSession.DATA_KEY, session).build()
        onEdt { action.actionPerformed(TestActionEvent.createTestEvent(action, context)) }
        val text = reported.get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertTrue(text, REFUSAL in text)
        assertTrue(runInEdtAndGet { FileEditorManager.getInstance(project).openFiles.none { file -> file is CodeListingFile } })
    }

    /**
     * The core library's frames are library frames: the session respects
     * the frames view's Show Library Frames, and while it is off each run
     * of them below the top frame is folded into one line that counts
     * them; the top frame, shipped or not, and the program's frames stay.
     */
    fun testShippedFramesAreLibraryFrames() {
        val settings = XDebuggerSettingManagerImpl.getInstanceImpl().dataViewSettings
        val shown = settings.isShowLibraryStackFrames
        try {
            val faked = fakeSession()
            val agent = identifiedAgent(faked) { command, _ ->
                when (command) {
                    "stackTrace" -> JsonParser.parseString(LIBRARY_STACK).asJsonObject
                    "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                    else -> JsonObject()
                }
            }
            agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
            agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
            faked.debugging.nextPause()
            assertTrue(faked.debugging.process.isLibraryFrameFilterSupported)

            settings.isShowLibraryStackFrames = false
            assertEquals(
                listOf("Kernel>>value", NewspeakBundle.message("debugger.frames.library", 2), "Program>>run", NewspeakBundle.message("debugger.frames.library", 1)),
                framesOf(faked.debugging.session).map { frame -> textOf(frame).first },
            )
            settings.isShowLibraryStackFrames = true
            assertEquals(
                listOf("Kernel>>value", "Kernel>>do:", "Kernel>>at:", "Program>>run", "Kernel>>main"),
                framesOf(faked.debugging.session).map { frame -> textOf(frame).first },
            )
        } finally {
            settings.isShowLibraryStackFrames = shown
        }
    }

    /**
     * The memory view lists the stop's instance counts; a class chosen
     * lists its instances, asked by its name alone -- a shipped module's
     * names its module's id, which no program's module takes -- each a
     * value tree; a value's referrers are its children's provider's.
     */
    fun testTheMemoryViewListsInstanceCounts() {
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, arguments ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
                "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                "nsNext/instanceCounts" -> JsonParser.parseString(
                    """{"classes":[{"name":"Census/Point","count":2},{"name":"newspeak.core.Kernel/Array","count":1,"nsNext/shipped":true}]}""",
                ).asJsonObject
                "nsNext/instances" -> JsonParser.parseString(
                    if (arguments.get("name").asString == "newspeak.core.Kernel/Array") {
                        """{"variables":[{"name":"#1","value":"an Array","type":"Array","variablesReference":0}]}"""
                    } else {
                        """{"variables":[
                          {"name":"#1","value":"a Point","type":"Point","variablesReference":0,"nsNext/valueReference":31},
                          {"name":"#2","value":"a Point","type":"Point","variablesReference":0,"nsNext/valueReference":32}
                        ]}"""
                    },
                ).asJsonObject
                "nsNext/referrers" -> JsonParser.parseString(
                    """{"variables":[{"name":"frame 1 points","value":"Census>>main:args:","variablesReference":0,"presentationHint":{"kind":"virtual"}}]}""",
                ).asJsonObject
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        faked.debugging.nextPause()

        val view = (faked.debugging.process as NewspeakDebugProcess).memoryView
        val counts = view.refresh().get(WAIT_SECONDS, TimeUnit.SECONDS)
        assertEquals(listOf(DapInstanceCount("Census/Point", 2, false), DapInstanceCount("newspeak.core.Kernel/Array", 1, true)), counts)

        val points = childrenOf(view.instancesOf(counts[0]))
        assertEquals(listOf("#1", "#2"), points.keys.toList())
        childrenOf(view.instancesOf(counts[1]))
        assertEquals(
            listOf("""{"name":"Census/Point","start":0,"count":100}""", """{"name":"newspeak.core.Kernel/Array","start":0,"count":100}"""),
            agent.requests("nsNext/instances").map(JsonObject::toString),
        )

        val referrers = childrenOf(checkNotNull(points.getValue("#2").referrersProvider).referringObjectsValue)
        val onTheStack = referrers.getValue("frame 1 points") as NewspeakValue
        assertTrue("a referrer on the stack names a place", onTheStack.variable.virtual)
        assertNull("which is not set", onTheStack.modifier)
        assertEquals(32, agent.requests("nsNext/referrers").single().get("valueReference").asInt)
    }

    /**
     * A condition or log message the runtime could not decide stops the
     * program, whatever the breakpoint's suspend policy, so that its error
     * is seen: the session shows the stop and says why, and neither
     * resumes it nor evaluates the log expression.
     */
    fun testAFailedConditionStopsAndSaysWhy() {
        val breakpoint = lineBreakpointWithLog(SuspendPolicy.NONE)
        assertTrue(isLogpoint(breakpoint))
        val reported = notifications()
        val faked = fakeSession()
        val agent = identifiedAgent(faked, ::loggingAgentAnswer)
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", failedConditionStop())
        faked.debugging.nextPause()
        assertEquals(NewspeakBundle.message("debugger.stopped.condition.failed", FAILURE), reported.get(WAIT_SECONDS, TimeUnit.SECONDS))
        assertTrue("still stopped", faked.debugging.session.isSuspended)
        assertFalse(agent.commands().toString(), agent.commands().contains("continue"))
        assertFalse(agent.commands().toString(), agent.commands().contains("evaluate"))
    }

    /**
     * A log expression of a breakpoint that stops is evaluated by the
     * session at the stop, once, in the top frame; a stop whose condition
     * or log message failed evaluates nothing.
     */
    fun testALogExpressionAtAStopIsLoggedOnce() {
        lineBreakpointWithLog(SuspendPolicy.ALL)
        val faked = fakeSession()
        val agent = identifiedAgent(faked, ::loggingAgentAnswer)
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", JsonParser.parseString("""{"reason":"breakpoint","hitBreakpointIds":[$LOGGED_BREAKPOINT]}""").asJsonObject)
        faked.debugging.nextPause()
        assertEquals(listOf("""{"expression":"count printString","frameId":1}"""), agent.requests("evaluate").map(JsonObject::toString))

        onEdt { faked.debugging.session.resume() }
        agent.event("stopped", failedConditionStop())
        faked.debugging.nextPause()
        assertEquals("not evaluated again at a failed stop", 1, agent.requests("evaluate").size)
    }

    /**
     * A withheld value -- read-only, without a value reference -- is
     * neither set nor marked; a frame's other variable is both, Mark
     * Object… sending its value reference and the label typed.
     */
    fun testAWithheldValueIsNeitherSetNorMarked() {
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, _ ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
                "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
                "scopes" -> JsonParser.parseString(
                    """{"scopes":[{"name":"Locals","presentationHint":"locals","variablesReference":$LOCALS_REFERENCE,"expensive":false}]}""",
                ).asJsonObject
                "variables" -> JsonParser.parseString(
                    """{"variables":[
                      {"name":"vmMirror","value":"a VMMirror","type":"VMMirror","variablesReference":0,"presentationHint":{"attributes":["readOnly"]}},
                      {"name":"count","value":"3","type":"Integer","variablesReference":0,"nsNext/valueReference":5}
                    ]}""",
                ).asJsonObject
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        agent.event("stopped", JsonObject().apply { addProperty("reason", "pause") })
        faked.debugging.nextPause()
        val locals = childrenOf(checkNotNull(faked.debugging.session.currentStackFrame))
        val withheld = locals.getValue("vmMirror")
        val count = locals.getValue("count")
        assertNull("a withheld value is not set", withheld.modifier)
        assertNotNull(count.modifier)

        val action = NewspeakMarkObjectAction()
        val event = sessionEvent(action, faked.debugging.session)
        assertFalse("nor marked", action.isEnabled(withheld, "vmMirror", event))
        assertTrue(action.isEnabled(count, "count", event))
        TestDialogManager.setTestInputDialog(TestInputDialog { _ -> "counted" })
        try {
            onEdt { action.perform(count, "count", event) }
        } finally {
            TestDialogManager.setTestInputDialog(TestInputDialog.DEFAULT)
        }
        awaitRequests(agent, "nsNext/mark", 1)
        assertEquals("""{"valueReference":5,"label":"counted"}""", agent.requests("nsNext/mark").single().toString())
    }

    /** A line breakpoint of the policy [policy] that logs `count printString`, on a file of its own, before the session starts. */
    private fun lineBreakpointWithLog(policy: SuspendPolicy): XLineBreakpoint<NewspeakLineBreakpointProperties> {
        val file = program("Logged", STOPS)
        val type = XDebuggerUtil.getInstance().findBreakpointType(NewspeakLineBreakpointType::class.java)
        var added: XLineBreakpoint<NewspeakLineBreakpointProperties>? = null
        runInEdtAndWait {
            WriteAction.run<Throwable> {
                added = XDebuggerManager.getInstance(project).breakpointManager
                    .addLineBreakpoint(type, file.url, 5, NewspeakLineBreakpointProperties())
                    .apply {
                        suspendPolicy = policy
                        setLogExpression("count printString")
                    }
            }
        }
        return checkNotNull(added)
    }

    /** A scripted agent's answers for a stop at the logging breakpoint, which it sets under [LOGGED_BREAKPOINT]. */
    private fun loggingAgentAnswer(command: String, @Suppress("UNUSED_PARAMETER") arguments: JsonObject): JsonObject =
        when (command) {
            "setBreakpoints" -> JsonParser.parseString("""{"breakpoints":[{"id":$LOGGED_BREAKPOINT,"verified":true}]}""").asJsonObject
            "stackTrace" -> JsonParser.parseString(SHIPPED_STACK).asJsonObject
            "source" -> JsonObject().apply { addProperty("content", KERNEL_TEXT) }
            "evaluate" -> JsonObject().apply { addProperty("result", "'3'") }
            else -> JsonObject()
        }

    /** The agent's stop at the logging breakpoint whose condition failed, its error the `text`. */
    private fun failedConditionStop(): JsonObject = JsonObject().apply {
        addProperty("reason", "breakpoint")
        add("hitBreakpointIds", JsonArray().apply { add(LOGGED_BREAKPOINT) })
        addProperty("text", FAILURE)
    }

    /** An action event of [session]'s tree, as its menu makes one. */
    private fun sessionEvent(action: AnAction, session: XDebugSession): AnActionEvent =
        TestActionEvent.createTestEvent(
            action,
            SimpleDataContext.builder().add(CommonDataKeys.PROJECT, project).add(XDebugSession.DATA_KEY, session).build(),
        )

    /** The value [expression] answers in [frame], through its evaluator. */
    private fun evaluated(frame: XStackFrame, expression: String): XValue = evaluation(frame, expression).get(WAIT_SECONDS, TimeUnit.SECONDS)

    /** The error [expression] answers in [frame]. */
    private fun evaluationError(frame: XStackFrame, expression: String): String {
        try {
            evaluation(frame, expression).get(WAIT_SECONDS, TimeUnit.SECONDS)
        } catch (failure: ExecutionException) {
            return failure.cause?.message.orEmpty()
        }
        throw AssertionError("`$expression` answered a value")
    }

    private fun evaluation(frame: XStackFrame, expression: String): CompletableFuture<XValue> {
        val answer = CompletableFuture<XValue>()
        checkNotNull(frame.evaluator).evaluate(
            expression,
            object : XDebuggerEvaluator.XEvaluationCallback {
                override fun evaluated(result: XValue) {
                    answer.complete(result)
                }

                override fun errorOccurred(errorMessage: String) {
                    answer.completeExceptionally(AssertionError(errorMessage))
                }
            },
            null,
        )
        return answer
    }

    /** Waits, bounded, until [agent] received [count] requests [command]. */
    private fun awaitRequests(agent: FakeAgent, command: String, count: Int) {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(WAIT_SECONDS)
        while (agent.requests(command).size < count) {
            if (System.nanoTime() > deadline) throw AssertionError("no $count `$command` within $WAIT_SECONDS s: ${agent.commands()}")
            Thread.sleep(POLL_MILLISECONDS)
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
            .indexOfFirst { text -> text.contains("^definitions at: match.") }
        assertTrue("Manifest>>at:", line >= 0)
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

    /**
     * A frame is the same as one of an earlier stop -- the platform then
     * keeps the nodes the user expanded -- when it runs the same body in
     * the same module at the same depth, whatever the agent's frame ids
     * and lines; a frame of another body, or of a recursive activation of
     * the same body, deeper, is another.
     */
    fun testAFrameIsTheSameAcrossStopsByItsBodyAndDepth() {
        // The frames' file, which a stop opens.
        val path = JsonPrimitive(program("Counter", STOPS).path).toString()
        val stacks = listOf(
            """{"stackFrames":[
              {"id":1,"name":"Counter>>count:","source":{"name":"Counter.ns","path":$path},"line":4,"column":5},
              {"id":2,"name":"Counter>>main:args:","source":{"name":"Counter.ns","path":$path},"line":7,"column":5}
            ],"totalFrames":2}""",
            """{"stackFrames":[
              {"id":7,"name":"Counter>>count:","source":{"name":"Counter.ns","path":$path},"line":5,"column":5},
              {"id":8,"name":"Counter>>main:args:","source":{"name":"Counter.ns","path":$path},"line":7,"column":5}
            ],"totalFrames":2}""",
            """{"stackFrames":[
              {"id":1,"name":"Counter>>count:","source":{"name":"Counter.ns","path":$path},"line":4,"column":5},
              {"id":2,"name":"Counter>>count:","source":{"name":"Counter.ns","path":$path},"line":6,"column":5},
              {"id":3,"name":"Counter>>main:args:","source":{"name":"Counter.ns","path":$path},"line":7,"column":5}
            ],"totalFrames":3}""",
        )
        val answered = AtomicInteger()
        val faked = fakeSession()
        val agent = identifiedAgent(faked) { command, _ ->
            when (command) {
                "stackTrace" -> JsonParser.parseString(stacks[answered.getAndIncrement()]).asJsonObject
                else -> JsonObject()
            }
        }
        agent.configured.get(WAIT_SECONDS, TimeUnit.SECONDS)
        val stops = stacks.indices.map { stop ->
            if (stop > 0) onEdt { faked.debugging.session.resume() }
            agent.event("stopped", JsonObject().apply { addProperty("reason", "step") })
            faked.debugging.nextPause()
            framesOf(faked.debugging.session).map { frame -> frame.equalityObject }
        }
        val (first, second, recursive) = stops
        assertEquals("the same activation at the next line", first, second)
        assertFalse("the caller is another frame", first[0] == first[1])
        assertFalse("a recursive activation, deeper, is another frame", recursive[0] == first[0])
        assertEquals("the activation it was called from is the earlier stop's", first[0], recursive[1])
        assertEquals(first[1], recursive[2])
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
    private fun identifiedAgent(faked: FakedSession, answer: (String, JsonObject) -> JsonObject?): FakeAgent =
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
     * with what [answer] gives, a success, or a refusal with [REFUSAL] where
     * it gives null, and after `attach` sends what [onAttach] sends. Its
     * connection writes in order, so its events follow its answers as the
     * real agent's do.
     */
    private inner class FakeAgent(
        port: Int,
        private val onAttach: FakeAgent.() -> Unit,
        private val answer: (String, JsonObject) -> JsonObject? = { _, _ -> JsonObject() },
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
                    val body = answer(command, arguments)
                    connection.send(
                        response(request).apply {
                            if (body != null) {
                                add("body", body)
                            } else {
                                addProperty("success", false)
                                addProperty("message", REFUSAL)
                            }
                        },
                    )
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

        /** What a fake agent's refusal says. */
        const val REFUSAL = "no frame 1 at this stop"

        /** The secret the fake sessions' processes were given, which an identified fake agent returns. */
        const val SECRET = "00112233445566778899aabbccddeeff"
        const val WAIT_MILLISECONDS = 30_000L

        /** How often a wait for what a fake agent received looks again. */
        const val POLL_MILLISECONDS = 20L

        /** How long a program inside one leaf must outlive the terminate: it would end at once otherwise. */
        const val LEAF_OUTLIVES_MILLISECONDS = 1_000L

        /** The fake agent's references: a frame's locals, and an `Array` among them with [ELEMENT_COUNT] elements. */
        const val LOCALS_REFERENCE = 100
        const val ELEMENTS_REFERENCE = 200
        const val ELEMENT_COUNT = 150

        /** The scripted agent's id of the logging breakpoint, and the error its condition failed with. */
        const val LOGGED_BREAKPOINT = 9
        const val FAILURE = "ZeroDivide: division by zero"

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

        /** A root whose line 3 runs a method of its nested `Helper`, and whose line 4 prints. */
        val HELPING = """
            class Helping = Object new () (
              public main: platform args: arguments = (
                Helper new run.
                platform transcript show: 'ran'; newLine.
              )
              class Helper = () (
                public run = (
                  ^42
                )
              )
            ) : ()
        """.trimIndent() + "\n"

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
              | Shared = manifest at: 'Shared'. |
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

        /** A stop in a shipped frame, above a run of two shipped frames, a program's frame and a last shipped one. */
        val LIBRARY_STACK = """
            {"stackFrames":[
              {"id":1,"name":"Kernel>>value","source":{"name":"Kernel.ns","sourceReference":7},"line":2,"column":3,"presentationHint":"subtle","nsNext/shipped":true},
              {"id":2,"name":"Kernel>>do:","source":{"name":"Kernel.ns","sourceReference":7},"line":2,"column":3,"presentationHint":"subtle","nsNext/shipped":true},
              {"id":3,"name":"Kernel>>at:","source":{"name":"Kernel.ns","sourceReference":7},"line":2,"column":3,"presentationHint":"subtle","nsNext/shipped":true},
              {"id":4,"name":"Program>>run","line":0,"column":0},
              {"id":5,"name":"Kernel>>main","source":{"name":"Kernel.ns","sourceReference":7},"line":2,"column":3,"presentationHint":"subtle","nsNext/shipped":true}
            ],"totalFrames":5}
        """.trimIndent()

        /** A stop in a shipped frame the agent cannot end, above a program's frame it can. */
        val RESTARTABLE_STACK = """
            {"stackFrames":[
              {"id":1,"name":"Kernel>>value","source":{"name":"Kernel.ns","sourceReference":7},"line":2,"column":3,"presentationHint":"subtle","nsNext/shipped":true,"canRestart":false},
              {"id":2,"name":"Program>>main:args:","line":0,"column":0,"canRestart":true}
            ],"totalFrames":2}
        """.trimIndent()

        /** A root whose line 4 sends `first:` with the result of `second`; `first:`'s statement is line 8. */
        val SMART = """
            class Smart = Object new () (
              public main: platform args: arguments = (
                | value |
                value:: self first: self second.
                platform transcript show: value printString; newLine.
              )
              first: number = (
                ^number + 1
              )
              second = (
                ^41
              )
            ) : ()
        """.trimIndent() + "\n"

        /** A root that prints what `compute:` answers; `compute:` prints, line 6, and answers 42, line 7. */
        val RESTARTING = """
            class Restarting = Object new () (
              public main: platform args: arguments = (
                platform transcript show: (self compute: platform) printString; newLine.
              )
              compute: platform = (
                platform transcript show: 'in'; newLine.
                ^42
              )
            ) : ()
        """.trimIndent() + "\n"

        /** A loop whose body, line 4, runs three times. */
        val LOOP = """
            class Loop = Object new () (
              public main: platform args: arguments = (
                1 to: 3 do: [:index |
                  platform transcript show: index printString; newLine].
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
