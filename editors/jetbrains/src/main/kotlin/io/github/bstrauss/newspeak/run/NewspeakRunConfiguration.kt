package io.github.bstrauss.newspeak.run

import com.intellij.execution.ExecutionException
import com.intellij.execution.Executor
import com.intellij.execution.configurations.CommandLineState
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.execution.configurations.RunConfigurationBase
import com.intellij.execution.configurations.RunConfigurationOptions
import com.intellij.execution.configurations.RunProfileState
import com.intellij.execution.configurations.RuntimeConfigurationError
import com.intellij.execution.process.KillableColoredProcessHandler
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessTerminatedListener
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import com.intellij.util.execution.ParametersListUtil
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import io.github.bstrauss.newspeak.settings.NewspeakSettingsConfigurable
import java.nio.file.Files
import java.nio.file.Path

/** The ns-next subcommand a Newspeak run configuration starts. */
enum class NewspeakCommand(val subcommand: String) {
    RUN("run"),
    BENCH("bench"),
}

/** What a Newspeak run configuration stores. */
class NewspeakRunConfigurationOptions : RunConfigurationOptions() {
    /** The project's `newspeak.json`; ns-next is given its directory. */
    var projectFile by string()

    /** A [NewspeakCommand]'s name. */
    var command by string(NewspeakCommand.RUN.name)

    var script by string()
    var programArguments by string()
    var iterations by property(10)
}

/** Runs `ns-next run` or `ns-next bench` on a project, with the runtime the plugin's settings name. */
class NewspeakRunConfiguration(project: Project, factory: ConfigurationFactory, name: String) :
    RunConfigurationBase<NewspeakRunConfigurationOptions>(project, factory, name) {

    override fun getOptions(): NewspeakRunConfigurationOptions = super.getOptions() as NewspeakRunConfigurationOptions

    var projectFile: String?
        get() = options.projectFile
        set(value) {
            options.projectFile = value
        }

    /** A stored name no constant has, as from a hand-edited workspace file, reads as [NewspeakCommand.RUN]. */
    var command: NewspeakCommand
        get() = NewspeakCommand.entries.firstOrNull { command -> command.name == options.command } ?: NewspeakCommand.RUN
        set(value) {
            options.command = value.name
        }

    var script: String?
        get() = options.script
        set(value) {
            options.script = value
        }

    var programArguments: String?
        get() = options.programArguments
        set(value) {
            options.programArguments = value
        }

    var iterations: Int
        get() = options.iterations
        set(value) {
            options.iterations = value
        }

    override fun getConfigurationEditor(): SettingsEditor<out RunConfiguration> = NewspeakRunConfigurationEditor()

    override fun checkConfiguration() {
        val runtime = locateRuntime(NewspeakSettings.getInstance().state.runtimePath)
        if (runtime !is RuntimeLocation.Found) {
            throw RuntimeConfigurationError(runtimeProblem(runtime), ::openNewspeakSettings)
        }
        val projectFile = projectFile?.trim().orEmpty()
        if (projectFile.isEmpty() || !Files.isRegularFile(Path.of(projectFile))) {
            throw RuntimeConfigurationError("Choose the project's newspeak.json")
        }
        if (command == NewspeakCommand.BENCH && iterations < 1) {
            throw RuntimeConfigurationError("A benchmark takes at least one iteration")
        }
    }

    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState {
        // Read afresh on every run, so a changed setting needs no restart.
        val runtime = when (val location = locateRuntime(NewspeakSettings.getInstance().state.runtimePath)) {
            is RuntimeLocation.Found -> location.path
            else -> throw ExecutionException(runtimeProblem(location))
        }
        val configuration = this
        return object : CommandLineState(environment) {
            override fun startProcess(): ProcessHandler {
                val commandLine = GeneralCommandLine(newspeakCommandLine(runtime, configuration))
                    .withWorkDirectory(Path.of(configuration.projectFile.orEmpty()).parent?.toString())
                val handler = KillableColoredProcessHandler(commandLine)
                ProcessTerminatedListener.attach(handler)
                return handler
            }
        }
    }

    private fun openNewspeakSettings() {
        ShowSettingsUtil.getInstance().showSettingsDialog(project, NewspeakSettingsConfigurable::class.java)
    }
}

/** Why the runtime cannot be started, in the same words whether the check or the start finds it. */
private fun runtimeProblem(location: RuntimeLocation): String = when (location) {
    is RuntimeLocation.Found -> error("the runtime was found at ${location.path}")
    RuntimeLocation.NotConfigured ->
        "The ns-next path is not set: choose the ns-next runtime under Settings | Languages & Frameworks | Newspeak"
    is RuntimeLocation.ConfiguredPathMissing -> "No file at the ns-next path ${location.path}"
}

/** The command line a Newspeak run configuration starts; pure, so its shape is tested without starting anything. */
fun newspeakCommandLine(runtime: Path, configuration: NewspeakRunConfiguration): List<String> = buildList {
    add(runtime.toString())
    add(configuration.command.subcommand)
    configuration.script?.takeIf(String::isNotEmpty)?.let { script -> addAll(listOf("--script", script)) }
    if (configuration.command == NewspeakCommand.BENCH) addAll(listOf("--iterations", configuration.iterations.toString()))
    // ns-next takes the project's directory; the configuration names its newspeak.json, which is what the chooser picks.
    add(Path.of(configuration.projectFile.orEmpty()).parent?.toString().orEmpty())
    val arguments = ParametersListUtil.parse(configuration.programArguments.orEmpty())
    if (arguments.isNotEmpty()) {
        add("--")
        addAll(arguments)
    }
}
