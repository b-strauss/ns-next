package io.github.bstrauss.newspeak.run

import com.intellij.execution.configurations.RuntimeConfigurationError
import com.intellij.openapi.util.Disposer
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

    override fun tearDown() {
        try {
            NewspeakSettings.getInstance().state.runtimePath = null
        } finally {
            super.tearDown()
        }
    }

    fun testRunWithAScriptAndArguments() {
        val run = configuration().apply {
            projectFile = "/work/app/newspeak.json"
            command = NewspeakCommand.RUN
            script = "timed"
            programArguments = "Sieve \"Delta Blue\""
        }
        assertEquals(
            listOf(runtime.toString(), "run", "--script", "timed", projectDirectory, "--", "Sieve", "Delta Blue"),
            newspeakCommandLine(runtime, run),
        )
    }

    fun testBenchWithItsIterationsAndNoScript() {
        val bench = configuration().apply {
            projectFile = "/work/app/newspeak.json"
            command = NewspeakCommand.BENCH
            iterations = 3
        }
        assertEquals(
            listOf(runtime.toString(), "bench", "--iterations", "3", projectDirectory),
            newspeakCommandLine(runtime, bench),
        )
    }

    fun testTheProcessDecodesOutputAsUtf8() {
        val run = configuration().apply { projectFile = "/work/app/newspeak.json" }
        assertEquals(StandardCharsets.UTF_8, newspeakProcessCommandLine(runtime, run).charset)
    }

    fun testARelativeProjectFileNamesItsAbsoluteDirectory() {
        val run = configuration().apply { projectFile = "newspeak.json" }
        val directory = Path.of("newspeak.json").toAbsolutePath().parent.toString()
        assertEquals(listOf(runtime.toString(), "run", directory), newspeakCommandLine(runtime, run))
        assertEquals(directory, newspeakProcessCommandLine(runtime, run).workDirectory?.toString())
    }

    fun testNoProjectFileAndARootHaveNoDirectory() {
        assertNull(projectDirectory(configuration()))
        assertNull(projectDirectory(configuration().apply { projectFile = " " }))
        assertNull(projectDirectory(configuration().apply { projectFile = "/" }))
    }

    fun testANewConfigurationRunsTenIterationsWhenBenchmarking() {
        assertEquals(10, configuration().iterations)
    }

    fun testTheEditorGivesBackWhatItWasShown() {
        val shown = configuration().apply {
            projectFile = "/work/app/newspeak.json"
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
        assertEquals(shown.projectFile, applied.projectFile)
        assertEquals(shown.command, applied.command)
        assertEquals(shown.script, applied.script)
        assertEquals(shown.programArguments, applied.programArguments)
        assertEquals(shown.iterations, applied.iterations)
    }

    // JUnit 4.13's assertThrows, qualified: UsefulTestCase's own overloads,
    // inherited here, answer nothing, and the first test reads the error.
    fun testNoRuntimeConfiguredIsAnErrorNamingTheSetting() {
        NewspeakSettings.getInstance().state.runtimePath = null
        val run = configuration().apply { projectFile = "/work/app/newspeak.json" }
        val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { run.checkConfiguration() }
        assertTrue(error.localizedMessage, error.localizedMessage.contains("ns-next path"))
    }

    fun testWithARuntimeTheProjectFileAndIterationsAreChecked() {
        val directory = Files.createTempDirectory("ns-next-run-configuration")
        NewspeakSettings.getInstance().state.runtimePath = Files.createFile(directory.resolve("ns-next")).toString()
        val projectFile = Files.writeString(directory.resolve("newspeak.json"), "{}").toString()

        val noProject = configuration()
        val noProjectError = Assert.assertThrows(RuntimeConfigurationError::class.java) { noProject.checkConfiguration() }
        assertTrue(noProjectError.localizedMessage, noProjectError.localizedMessage.contains("newspeak.json"))

        val noIterations = configuration().apply {
            this.projectFile = projectFile
            command = NewspeakCommand.BENCH
            iterations = 0
        }
        val noIterationsError = Assert.assertThrows(RuntimeConfigurationError::class.java) { noIterations.checkConfiguration() }
        assertTrue(noIterationsError.localizedMessage, noIterationsError.localizedMessage.contains("iteration"))

        configuration().apply { this.projectFile = projectFile }.checkConfiguration()
    }

    fun testAMissingRuntimeFileIsAnError() {
        NewspeakSettings.getInstance().state.runtimePath = "/no/such/ns-next"
        val run = configuration().apply { projectFile = "/work/app/newspeak.json" }
        val error = Assert.assertThrows(RuntimeConfigurationError::class.java) { run.checkConfiguration() }
        assertTrue(error.localizedMessage, error.localizedMessage.contains("/no/such/ns-next"))
    }
}
