package io.github.bstrauss.newspeak.run

import com.intellij.execution.configurations.RuntimeConfigurationError
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.io.FileUtil
import com.intellij.testFramework.fixtures.BasePlatformTestCase
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
            temporaryDirectories.forEach { directory -> FileUtil.delete(directory) }
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
}
