package io.github.bstrauss.newspeak.run

import com.intellij.execution.configurations.RuntimeConfigurationError
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.io.NioFiles
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.DeveloperToolTab
import io.github.bstrauss.newspeak.devtool.TRACE_CATEGORIES
import io.github.bstrauss.newspeak.devtool.TraceFilters
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import org.junit.Assert

class NewspeakRunConfigurationTest : BasePlatformTestCase() {
    private fun configuration(): NewspeakRunConfiguration =
        NewspeakConfigurationType.getInstance().factory.createTemplateConfiguration(project) as NewspeakRunConfiguration

    // Paths are compared as `Path.of` renders them, so the expectations hold on
    // Windows, where it turns `/work/app` into `\work\app`; the project's
    // directory is made absolute as well, which there adds the current drive.
    private val runtime: Path = Path.of("/bin/ns-next")
    private val projectDirectory: String = Path.of("/work/app").toAbsolutePath().toString()
    private val moduleFile: String = Path.of("/work/app/Hello.ns").toAbsolutePath().toString()

    /** Directories a test created on disk, deleted after it. */
    private val temporaryDirectories = mutableListOf<Path>()

    private fun temporaryDirectory(): Path =
        Files.createTempDirectory("ns-next-run-configuration").also(temporaryDirectories::add)

    override fun tearDown() {
        try {
            NewspeakSettings.getInstance().state.runtimePath = null
            temporaryDirectories.forEach { directory -> NioFiles.deleteRecursively(directory) }
        } finally {
            super.tearDown()
        }
    }

    fun testRunWithAScriptAndArguments() {
        val run = configuration().apply {
            program = "/work/app/newspeak.json"
            command = NewspeakCommand.RUN
            script = "timed"
            programArguments = "Sieve \"Delta Blue\""
        }
        assertEquals(
            listOf(runtime.toString(), "run", "--script=timed", projectDirectory, "--", "Sieve", "Delta Blue"),
            newspeakCommandLine(runtime, run),
        )
    }

    fun testBenchWithItsIterationsAndNoScript() {
        val bench = configuration().apply {
            program = "/work/app/newspeak.json"
            command = NewspeakCommand.BENCH
            iterations = 3
        }
        assertEquals(
            listOf(runtime.toString(), "bench", "--iterations", "3", projectDirectory),
            newspeakCommandLine(runtime, bench),
        )
    }

    fun testAScriptNameIsOneTokenSoADashIsNoOption() {
        val dashed = configuration().apply {
            program = "/work/app/newspeak.json"
            script = "-x"
        }
        assertEquals(listOf(runtime.toString(), "run", "--script=-x", projectDirectory), newspeakCommandLine(runtime, dashed))
    }

    fun testADebuggedRunConnectsToTheDebuggerBeforeItsOtherOptions() {
        val run = configuration().apply {
            program = "/work/app/newspeak.json"
            script = "timed"
            programArguments = "--debug-connect=elsewhere"
        }
        assertEquals(
            listOf(
                runtime.toString(),
                "run",
                "--debug-connect=127.0.0.1:5005",
                "--script=timed",
                projectDirectory,
                "--",
                "--debug-connect=elsewhere",
            ),
            newspeakCommandLine(runtime, run, "127.0.0.1:5005"),
        )
    }

    fun testRuntimeOptionsComeBeforeTheProgram() {
        val run = configuration().apply {
            program = "/work/app/newspeak.json"
            script = "timed"
            runtimeOptions = "--trace=compile --trace-filter=Demo>>... --trace-filter=\"Other Demo>>...\""
        }
        val options = listOf("--trace=compile", "--trace-filter=Demo>>...", "--trace-filter=Other Demo>>...")
        assertEquals(
            listOf(runtime.toString(), "run") + options + listOf("--script=timed", projectDirectory),
            newspeakCommandLine(runtime, run),
        )
        assertEquals(
            listOf(runtime.toString(), "run", "--debug-connect=127.0.0.1:5005") + options + listOf("--script=timed", projectDirectory),
            newspeakCommandLine(runtime, run, "127.0.0.1:5005"),
        )
    }

    fun testTheProcessDecodesOutputAsUtf8() {
        val run = configuration().apply { program = "/work/app/newspeak.json" }
        assertEquals(StandardCharsets.UTF_8, newspeakProcessCommandLine(runtime, run).charset)
    }

    fun testARelativeProjectFileIsTheIdeProjects() {
        val run = configuration().apply { program = "app/newspeak.json" }
        val directory = Path.of(checkNotNull(project.basePath)).resolve("app").toAbsolutePath().toString()
        assertEquals(listOf(runtime.toString(), "run", directory), newspeakCommandLine(runtime, run))
        assertEquals(directory, newspeakProcessCommandLine(runtime, run).workDirectory?.toString())
    }

    fun testNoProjectFileAndARootHaveNoDirectory() {
        assertNull(programArgument(configuration()))
        assertNull(programArgument(configuration().apply { program = " " }))
        assertNull(programArgument(configuration().apply { program = "/" }))
    }

    fun testANewConfigurationRunsTenIterationsWhenBenchmarking() {
        assertEquals(10, configuration().iterations)
    }

    fun testTheEditorGivesBackWhatItWasShown() {
        val shown = configuration().apply {
            program = "/work/app/newspeak.json"
            command = NewspeakCommand.BENCH
            script = "timed"
            programArguments = "Sieve \"Delta Blue\""
            runtimeOptions = "--trace=compile"
            iterations = 7
        }
        val editor = NewspeakRunConfigurationEditor()
        Disposer.register(testRootDisposable, editor)
        editor.resetFrom(shown)
        val applied = configuration()
        editor.applyTo(applied)
        assertEquals(shown.program, applied.program)
        assertEquals(shown.command, applied.command)
        assertEquals(shown.script, applied.script)
        assertEquals(shown.programArguments, applied.programArguments)
        assertEquals(shown.runtimeOptions, applied.runtimeOptions)
        assertEquals(shown.iterations, applied.iterations)
    }

    fun testTheEditorStoresASystemIndependentPath() {
        val shown = configuration().apply { program = """\work\app\newspeak.json""" }
        val editor = NewspeakRunConfigurationEditor()
        Disposer.register(testRootDisposable, editor)
        editor.resetFrom(shown)
        val applied = configuration()
        editor.applyTo(applied)
        assertEquals("/work/app/newspeak.json", applied.program)
    }

    // JUnit 4.13's assertThrows, qualified: UsefulTestCase's own overloads,
    // inherited here, answer nothing, and the first test reads the error.
    fun testNoRuntimeConfiguredIsAnErrorNamingTheSetting() {
        NewspeakSettings.getInstance().state.runtimePath = null
        val run = configuration().apply { program = "/work/app/newspeak.json" }
        val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { run.checkConfiguration() }
        assertTrue(error.localizedMessage, error.localizedMessage.contains("ns-next path"))
        assertNotNull("the error offers to open the settings", error.configurationQuickFix)
    }

    fun testWithARuntimeTheProjectFileAndIterationsAreChecked() {
        val directory = temporaryDirectory()
        NewspeakSettings.getInstance().state.runtimePath = Files.createFile(directory.resolve("ns-next")).toString()
        val program = Files.writeString(directory.resolve("newspeak.json"), "{}").toString()

        val noProject = configuration()
        val noProjectError = Assert.assertThrows(RuntimeConfigurationError::class.java) { noProject.checkConfiguration() }
        assertTrue(noProjectError.localizedMessage, noProjectError.localizedMessage.contains("newspeak.json"))

        val noIterations = configuration().apply {
            this.program = program
            command = NewspeakCommand.BENCH
            iterations = 0
        }
        val noIterationsError = Assert.assertThrows(RuntimeConfigurationError::class.java) { noIterations.checkConfiguration() }
        assertTrue(noIterationsError.localizedMessage, noIterationsError.localizedMessage.contains("iteration"))

        configuration().apply { this.program = program }.checkConfiguration()
    }

    // A NUL is invalid in a path on every platform, so this holds beyond Windows' `"` and `<`.
    fun testAnInvalidProjectPathIsRefusedLikeAMissingOne() {
        val directory = temporaryDirectory()
        NewspeakSettings.getInstance().state.runtimePath = Files.createFile(directory.resolve("ns-next")).toString()
        val invalid = configuration().apply { program = "/work/a\u0000b/newspeak.json" }
        val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { invalid.checkConfiguration() }
        assertEquals("Choose the program: a project's newspeak.json or a .ns module", error.localizedMessage)
        assertNull(programArgument(invalid))
    }

    fun testAMissingRuntimeFileIsAnError() {
        NewspeakSettings.getInstance().state.runtimePath = "/no/such/ns-next"
        val run = configuration().apply { program = "/work/app/newspeak.json" }
        val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { run.checkConfiguration() }
        assertTrue(error.localizedMessage, error.localizedMessage.contains("/no/such/ns-next"))
        assertNotNull("the error offers to open the settings", error.configurationQuickFix)
    }

    fun testRunAModuleGivesItsFileAndRunsInItsDirectory() {
        val run = configuration().apply {
            program = "/work/app/Hello.ns"
            programArguments = "one"
        }
        assertEquals(listOf(runtime.toString(), "run", moduleFile, "--", "one"), newspeakCommandLine(runtime, run))
        assertEquals(projectDirectory, newspeakProcessCommandLine(runtime, run).workDirectory?.toString())
    }

    fun testBenchAModuleWithItsIterations() {
        val bench = configuration().apply {
            program = "/work/app/Hello.ns"
            command = NewspeakCommand.BENCH
            iterations = 3
        }
        assertEquals(listOf(runtime.toString(), "bench", "--iterations", "3", moduleFile), newspeakCommandLine(runtime, bench))
    }

    fun testAModuleWithAScriptIsRefused() {
        val directory = temporaryDirectory()
        NewspeakSettings.getInstance().state.runtimePath = Files.createFile(directory.resolve("ns-next")).toString()
        val module = Files.writeString(directory.resolve("Hello.ns"), "class Hello = () ()").toString()
        val scripted = configuration().apply {
            program = module
            script = "timed"
        }
        val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { scripted.checkConfiguration() }
        assertTrue(error.localizedMessage, error.localizedMessage.contains("script"))
        configuration().apply { program = module }.checkConfiguration()
    }

    fun testAProgramThatIsNeitherIsRefused() {
        val directory = temporaryDirectory()
        NewspeakSettings.getInstance().state.runtimePath = Files.createFile(directory.resolve("ns-next")).toString()
        val notes = Files.writeString(directory.resolve("notes.txt"), "").toString()
        for (program in listOf(notes, directory.toString())) {
            val other = configuration().apply { this.program = program }
            val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { other.checkConfiguration() }
            assertEquals("Choose the program: a project's newspeak.json or a .ns module", error.localizedMessage)
        }
        assertNull(programArgument(configuration().apply { program = "/work/app/notes.txt" }))
    }

    fun testTheEditorDropsTheScriptOfAModule() {
        val shown = configuration().apply {
            program = "/work/app/Hello.ns"
            script = "timed"
        }
        val editor = NewspeakRunConfigurationEditor()
        Disposer.register(testRootDisposable, editor)
        editor.resetFrom(shown)
        val applied = configuration()
        editor.applyTo(applied)
        assertEquals("/work/app/Hello.ns", applied.program)
        assertNull("ns-next refuses --script with a .ns file", applied.script)
    }

    fun testCheckedCategoriesBecomeOneTraceOption() {
        val run = configuration().apply {
            program = "/work/app/newspeak.json"
            script = "timed"
            options.traceCategories = mutableListOf("inlining", "compile")
        }
        assertEquals(
            listOf(runtime.toString(), "run", "--trace=compile,inlining", "--script=timed", projectDirectory),
            newspeakCommandLine(runtime, run),
        )
    }

    fun testAllSevenAreAll() {
        val run = configuration().apply {
            program = "/work/app/newspeak.json"
            options.traceCategories = TRACE_CATEGORIES.toMutableList()
        }
        assertEquals(listOf(runtime.toString(), "run", "--trace=all", projectDirectory), newspeakCommandLine(runtime, run))
    }

    fun testListingKindsBecomeTheirOptions() {
        val run = configuration().apply {
            program = "/work/app/newspeak.json"
            options.printCode = true
        }
        assertEquals(listOf(runtime.toString(), "run", "--print-code", projectDirectory), newspeakCommandLine(runtime, run))
        assertTrue(run.tracesAnything())
        assertFalse(configuration().tracesAnything())
    }

    fun testATraceFileIsPassedWithItsPath() {
        val run = configuration().apply {
            program = "/work/app/newspeak.json"
            options.traceCategories = mutableListOf("caches")
        }
        val file = Path.of("/tmp/trace.jsonl")
        assertEquals(
            listOf(runtime.toString(), "run", "--debug-connect=a:1", "--trace=caches", "--trace-file=$file", projectDirectory),
            newspeakCommandLine(runtime, run, "a:1", file),
        )
        assertFalse(newspeakCommandLine(runtime, run).any { token -> token.startsWith("--trace-file") })
    }

    fun testTheFiltersFollowTheirKindAndBenchTakesNone() {
        val run = configuration().apply {
            program = "/work/app/newspeak.json"
            options.traceCategories = mutableListOf("compile")
            options.printCode = true
        }
        val filters = TraceFilters(listOf("A>>b"), listOf("C>>d"), noEvents = false, noListings = false)
        assertEquals(
            listOf(runtime.toString(), "run", "--trace=compile", "--trace-filter=A>>b", "--print-code", "--print-filter=C>>d", projectDirectory),
            newspeakCommandLine(runtime, run, filters = filters),
        )
        val withoutEvents = filters.copy(noEvents = true)
        assertEquals(
            listOf(runtime.toString(), "run", "--print-code", "--print-filter=C>>d", projectDirectory),
            newspeakCommandLine(runtime, run, filters = withoutEvents),
        )
        run.command = NewspeakCommand.BENCH
        assertEquals(listOf(runtime.toString(), "bench", "--iterations", "10", projectDirectory), newspeakCommandLine(runtime, run))
        assertFalse(run.tracesAnything())
    }

    fun testAnOptionWithAControlInTheFreeFieldIsAnError() {
        val directory = temporaryDirectory()
        NewspeakSettings.getInstance().state.runtimePath = Files.createFile(directory.resolve("ns-next")).toString()
        val program = Files.writeString(directory.resolve("newspeak.json"), "{}").toString()
        for (option in listOf("--trace=compile", "--print-code", "--trace-filter=Demo>>...", "--trace-file=x.jsonl")) {
            val run = configuration().apply {
                this.program = program
                runtimeOptions = "--timed $option"
            }
            val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { run.checkConfiguration() }
            assertTrue(error.localizedMessage, error.localizedMessage.contains(option.substringBefore('=')))
        }
        configuration().apply {
            this.program = program
            runtimeOptions = "--timed"
        }.checkConfiguration()
    }

    fun testBenchGreysTheTab() {
        val tab = DeveloperToolTab()
        Disposer.register(testRootDisposable, tab)
        val bench = configuration().apply { command = NewspeakCommand.BENCH }
        tab.resetFrom(bench)
        assertTrue(tab.categoryBoxes.values.all { box -> !box.isEnabled })
        assertTrue(tab.listingBoxes.values.all { box -> !box.isEnabled })
        tab.resetFrom(configuration())
        assertTrue(tab.categoryBoxes.values.all { box -> box.isEnabled })
    }

    fun testTheTabFollowsTheCommandOfTheFirstTab() {
        val editor = NewspeakRunConfigurationEditor()
        val tab = DeveloperToolTab()
        Disposer.register(testRootDisposable, editor)
        Disposer.register(testRootDisposable, tab)
        editor.onCommandChanged(tab::showCommand)
        editor.resetFrom(configuration())
        tab.resetFrom(configuration())
        val boxes = listOf(tab.allBox) + tab.categoryBoxes.values + tab.listingBoxes.values
        assertTrue(boxes.all { box -> box.isEnabled })
        editor.resetFrom(configuration().apply { command = NewspeakCommand.BENCH })
        assertTrue(boxes.all { box -> !box.isEnabled })
        editor.resetFrom(configuration())
        assertTrue(boxes.all { box -> box.isEnabled })
    }

    fun testTheTabGivesBackWhatItWasShown() {
        val shown = configuration().apply {
            options.traceCategories = mutableListOf("osr", "caches")
            options.printBytecode = true
            options.printInlining = true
        }
        val tab = DeveloperToolTab()
        Disposer.register(testRootDisposable, tab)
        tab.resetFrom(shown)
        assertFalse(tab.allBox.isSelected)
        val applied = configuration()
        tab.applyTo(applied)
        assertEquals(listOf("osr", "caches"), applied.options.traceCategories)
        assertTrue(applied.options.printBytecode)
        assertFalse(applied.options.printCode)
        assertTrue(applied.options.printInlining)
    }

    fun testEveryCheckboxHasItsSentence() {
        val tab = DeveloperToolTab()
        Disposer.register(testRootDisposable, tab)
        for ((name, box) in tab.categoryBoxes) {
            assertEquals(NewspeakBundle.message("devtool.category.$name.tooltip"), box.toolTipText)
            assertFalse(box.toolTipText.isNullOrBlank())
        }
        for ((kind, box) in tab.listingBoxes) {
            assertEquals(NewspeakBundle.message("devtool.listing.$kind.tooltip"), box.toolTipText)
            assertFalse(box.toolTipText.isNullOrBlank())
        }
    }
}
