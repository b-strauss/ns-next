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
import com.intellij.openapi.util.io.FileUtil
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.util.execution.ParametersListUtil
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.NewspeakDebuggeeProcessHandler
import io.github.bstrauss.newspeak.settings.NewspeakSettings
import io.github.bstrauss.newspeak.settings.NewspeakSettingsConfigurable
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.InvalidPathException
import java.nio.file.Path

/** The ns-next subcommand a Newspeak run configuration starts. */
enum class NewspeakCommand(val subcommand: String) {
    RUN("run"),
    BENCH("bench"),
}

/** What a Newspeak run configuration stores. */
class NewspeakRunConfigurationOptions : RunConfigurationOptions() {
    /** The program: a project's `newspeak.json`, whose directory ns-next is given, or a `.ns` module. */
    var program by string()

    /** A [NewspeakCommand]'s name. */
    var command by string(NewspeakCommand.RUN.name)

    var script by string()
    var programArguments by string()

    /** Options of ns-next itself, placed before the program: `--trace=…`, `--trace-filter=…`. */
    var runtimeOptions by string()
    var iterations by property(10)
}

/** Runs `ns-next run` or `ns-next bench` on a project or a module, with the runtime the plugin's settings name. */
class NewspeakRunConfiguration(project: Project, factory: ConfigurationFactory, name: String) :
    RunConfigurationBase<NewspeakRunConfigurationOptions>(project, factory, name) {

    override fun getOptions(): NewspeakRunConfigurationOptions = super.getOptions() as NewspeakRunConfigurationOptions

    var program: String?
        get() = options.program
        set(value) {
            options.program = value
        }

    /** A stored name no constant has, as from a hand-edited workspace file, reads as [NewspeakCommand.RUN]. */
    var command: NewspeakCommand
        get() = NewspeakCommand.entries.firstOrNull { command -> command.name == options.command } ?: NewspeakCommand.RUN
        set(value) {
            options.command = value.name
        }

    /** The script to run; null runs the project without one. */
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

    /** Options for ns-next itself, split as the platform splits parameters, one option per token. */
    var runtimeOptions: String?
        get() = options.runtimeOptions
        set(value) {
            options.runtimeOptions = value
        }

    var iterations: Int
        get() = options.iterations
        set(value) {
            options.iterations = value
        }

    /**
     * Whether the program is `file`: a relative path resolved against the
     * IDE project's base directory, as the run resolves it, and compared system-independently,
     * since a configuration stored by hand on Windows may hold backslashes,
     * and with the file system's case sensitivity.
     */
    fun namesProgram(file: VirtualFile): Boolean {
        val stored = program?.trim()?.takeIf(String::isNotEmpty) ?: return false
        val basePath = project.basePath
        // A path with a root is compared as written: made absolute, Windows' `\work` would gain a drive the VFS path lacks.
        val resolved = if (basePath != null && isRelative(stored)) "$basePath/$stored" else stored
        return FileUtil.pathsEqual(FileUtil.toCanonicalPath(FileUtil.toSystemIndependentName(resolved)), file.path)
    }

    private fun isRelative(path: String): Boolean = try {
        Path.of(path).root == null
    } catch (_: InvalidPathException) {
        false
    }

    override fun getConfigurationEditor(): SettingsEditor<out RunConfiguration> = NewspeakRunConfigurationEditor()

    override fun checkConfiguration() {
        val runtime = locateRuntime(NewspeakSettings.getInstance().state.runtimePath)
        if (runtime !is RuntimeLocation.Found) {
            throw RuntimeConfigurationError(runtimeProblem(runtime), ::openNewspeakSettings)
        }
        val programPath = programFilePath(this)
        val kind = programPath?.let(::programKind)
        if (programPath == null || kind == null || !Files.isRegularFile(programPath)) {
            throw RuntimeConfigurationError(NewspeakBundle.message("run.configuration.error.no.program"))
        }
        if (kind == ProgramKind.MODULE && script != null) {
            throw RuntimeConfigurationError(NewspeakBundle.message("run.configuration.error.script.of.module"))
        }
        if (command == NewspeakCommand.BENCH && iterations < 1) {
            throw RuntimeConfigurationError(NewspeakBundle.message("run.configuration.error.no.iterations"))
        }
    }

    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState {
        // Read afresh on every run, so a changed setting needs no restart.
        val runtime = when (val location = locateRuntime(NewspeakSettings.getInstance().state.runtimePath)) {
            is RuntimeLocation.Found -> location.path
            else -> throw ExecutionException(runtimeProblem(location))
        }
        if (programArgument(this) == null) throw ExecutionException(NewspeakBundle.message("run.configuration.error.no.program"))
        return NewspeakRunState(environment, runtime, this)
    }

    private fun openNewspeakSettings() {
        ShowSettingsUtil.getInstance().showSettingsDialog(project, NewspeakSettingsConfigurable::class.java)
    }
}

/**
 * What a Newspeak run configuration starts: ns-next, and under the Debug
 * executor ns-next connecting to the debugger at [debugAddress] and handed
 * [debugSecret], both of which the debug runner sets before it starts the
 * process.
 */
class NewspeakRunState(
    environment: ExecutionEnvironment,
    private val runtime: Path,
    private val configuration: NewspeakRunConfiguration,
) : CommandLineState(environment) {
    /** The `host:port` the debugger listens at; null for a run without it. */
    var debugAddress: String? = null

    /**
     * The one-time secret the runtime returns in its `process` event, so
     * that the debugger can tell its process from another that connected
     * first; handed over in the environment, never on the command line,
     * which other processes on the machine can read.
     */
    var debugSecret: String? = null

    override fun startProcess(): ProcessHandler {
        val commandLine = newspeakProcessCommandLine(runtime, configuration, debugAddress)
        debugSecret?.let { secret -> commandLine.withEnvironment(DEBUG_SECRET_VARIABLE, secret) }
        val handler = if (debugAddress == null) KillableColoredProcessHandler(commandLine) else NewspeakDebuggeeProcessHandler(commandLine)
        ProcessTerminatedListener.attach(handler)
        return handler
    }
}

/** `crates/ns-cli/src/debug/agent.rs`'s `SECRET_VARIABLE`: where ns-next --debug-connect takes the secret it returns. */
const val DEBUG_SECRET_VARIABLE = "NS_NEXT_DEBUG_SECRET"

/** `crates/ns-cli/src/project.rs`'s `PROJECT_FILE_NAME`: the name ns-next recognises a project file by. */
const val PROJECT_FILE_NAME = "newspeak.json"

/** What a configuration runs: a project from its `newspeak.json`, or one module from its source. */
enum class ProgramKind { PROJECT, MODULE }

/**
 * By the file's name, as ns-next tells the two apart; null for anything
 * else, which the check refuses rather than hand ns-next a file it would
 * read as a source.
 */
fun programKind(path: Path): ProgramKind? {
    val name = path.fileName?.toString() ?: return null
    return when {
        name == PROJECT_FILE_NAME -> ProgramKind.PROJECT
        name.endsWith(".ns") -> ProgramKind.MODULE
        else -> null
    }
}

/** Whether text typed as a program names a `.ns` module; text that is no path names none. */
fun namesAModule(text: String): Boolean = try {
    programKind(Path.of(text.trim())) == ProgramKind.MODULE
} catch (_: InvalidPathException) {
    false
}

/**
 * What ns-next is given: a project's directory, or the module's file. Null
 * without a program, for one naming a root, for text that is no path and
 * for a file that is neither kind; `getState` refuses them all before
 * anything is started.
 */
fun programArgument(configuration: NewspeakRunConfiguration): Path? {
    val path = programFilePath(configuration) ?: return null
    return when (programKind(path)) {
        ProgramKind.PROJECT -> path.parent
        ProgramKind.MODULE -> path
        null -> null
    }
}

/** Where ns-next runs: the program file's directory, for a project and a module alike. */
fun workingDirectory(configuration: NewspeakRunConfiguration): Path? =
    programArgument(configuration)?.let { programFilePath(configuration)?.parent }

/**
 * The program file as an absolute path; null when none is set or the text is
 * no valid path (a `"` or `<` on Windows, say), which is refused as a missing
 * one is. A relative path is the IDE project's, as a hand-edited
 * configuration means it; the IDE's own working directory is unrelated to the
 * project, so it is used only for a project without a base path.
 */
private fun programFilePath(configuration: NewspeakRunConfiguration): Path? {
    val file = configuration.program?.trim()?.takeIf(String::isNotEmpty) ?: return null
    return try {
        val path = Path.of(file)
        val basePath = configuration.project.basePath
        // A path with a root, Windows' `\work` without a drive included, is not the project's.
        if (path.root != null || basePath == null) path.toAbsolutePath() else Path.of(basePath).resolve(path).toAbsolutePath()
    } catch (_: InvalidPathException) {
        null
    }
}

/** Why the runtime cannot be started, in the same words whether the check or the start finds it. */
private fun runtimeProblem(location: RuntimeLocation): String = when (location) {
    is RuntimeLocation.Found -> error("the runtime was found at ${location.path}")
    RuntimeLocation.NotConfigured ->
        NewspeakBundle.message("run.configuration.error.runtime.not.set")
    is RuntimeLocation.ConfiguredPathMissing ->
        NewspeakBundle.message("run.configuration.error.runtime.missing", location.path)
}

/** The process a Newspeak run configuration starts: [newspeakCommandLine], in the program file's directory. */
fun newspeakProcessCommandLine(runtime: Path, configuration: NewspeakRunConfiguration, debugAddress: String? = null): GeneralCommandLine =
    GeneralCommandLine(newspeakCommandLine(runtime, configuration, debugAddress))
        .withWorkDirectory(workingDirectory(configuration)?.toFile())
        // ns-next writes UTF-8 whatever the platform; the default, Windows' code page, garbles anything past ASCII.
        .withCharset(StandardCharsets.UTF_8)

/**
 * The command line a Newspeak run configuration starts; pure, so its shape
 * is tested without starting anything. With [debugAddress], ns-next
 * connects to the debugger listening there (`--debug-connect`) rather than
 * listening itself, so that no port of the program is open to another
 * process.
 */
fun newspeakCommandLine(runtime: Path, configuration: NewspeakRunConfiguration, debugAddress: String? = null): List<String> = buildList {
    add(runtime.toString())
    add(configuration.command.subcommand)
    debugAddress?.let { address -> add("--debug-connect=$address") }
    // Each token as is: `--trace-filter` is repeatable, one pattern per occurrence.
    addAll(ParametersListUtil.parse(configuration.runtimeOptions.orEmpty()))
    // One token, so a name starting with `-` is not read as an option.
    configuration.script?.let { script -> add("--script=$script") }
    if (configuration.command == NewspeakCommand.BENCH) addAll(listOf("--iterations", configuration.iterations.toString()))
    // A project is given as its directory, a module as its file; the configuration names the file either way.
    add(checkNotNull(programArgument(configuration)) { "the configuration names no program" }.toString())
    val arguments = ParametersListUtil.parse(configuration.programArguments.orEmpty())
    if (arguments.isNotEmpty()) {
        add("--")
        addAll(arguments)
    }
}
