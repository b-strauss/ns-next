package io.github.bstrauss.newspeak.run

import com.intellij.execution.ProgramRunnerUtil
import com.intellij.execution.RunManager
import com.intellij.execution.RunnerAndConfigurationSettings
import com.intellij.execution.executors.DefaultRunExecutor
import com.intellij.icons.AllIcons
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import io.github.bstrauss.newspeak.NewspeakBundle

/** One entry of the play button's menu: runs or benchmarks a project, one of its scripts, or a module. */
class NewspeakRunAction(
    val programFile: VirtualFile,
    val script: String?,
    val command: NewspeakCommand,
) : AnAction(
    when (command) {
        NewspeakCommand.RUN -> NewspeakBundle.message("run.command.run")
        NewspeakCommand.BENCH -> NewspeakBundle.message("run.command.benchmark")
    },
    null,
    when (command) {
        NewspeakCommand.RUN -> AllIcons.Actions.Execute
        NewspeakCommand.BENCH -> AllIcons.Gutter.RunInQueue
    },
),
    DumbAware {
    override fun actionPerformed(event: AnActionEvent) {
        val project = event.project ?: return
        val settings = createConfigurationFromGutter(project, programFile, script, command)
        // An invalid configuration -- no ns-next path, say -- takes the platform's own path:
        // its error, and the dialog to edit the configuration.
        ProgramRunnerUtil.executeConfiguration(settings, DefaultRunExecutor.getRunExecutorInstance())
    }
}

/** What the gutter names a configuration: a project by its directory, a module by its file; the script; whether it benchmarks. */
fun gutterConfigurationName(programFile: VirtualFile, script: String?, command: NewspeakCommand): String {
    val programName = if (programFile.name == PROJECT_FILE_NAME) programFile.parent?.name ?: programFile.name else programFile.name
    val benchmark = command == NewspeakCommand.BENCH
    return when {
        script != null && benchmark -> NewspeakBundle.message("run.configuration.name.script.benchmark", programName, script)
        script != null -> NewspeakBundle.message("run.configuration.name.script", programName, script)
        benchmark -> NewspeakBundle.message("run.configuration.name.benchmark", programName)
        else -> programName
    }
}

/**
 * The configuration the play button runs, selected but not started: the
 * existing one with the same program file, script and command, or a new
 * temporary one from the type's template.
 */
fun createConfigurationFromGutter(
    project: Project,
    programFile: VirtualFile,
    script: String?,
    command: NewspeakCommand,
): RunnerAndConfigurationSettings {
    val runManager = RunManager.getInstance(project)
    val existing = runManager.getConfigurationSettingsList(NewspeakConfigurationType.getInstance())
        .firstOrNull { settings ->
            val configuration = settings.configuration as? NewspeakRunConfiguration
            configuration != null &&
                configuration.namesProgram(programFile) &&
                configuration.script == script &&
                configuration.command == command
        }
    val settings = existing ?: runManager.createConfiguration(
        gutterConfigurationName(programFile, script, command),
        NewspeakConfigurationType.getInstance().factory,
    ).also { created ->
        (created.configuration as NewspeakRunConfiguration).apply {
            this.program = programFile.path
            this.script = script
            this.command = command
        }
        // Two programs of the same name -- projects in directories of the same name, or modules
        // `a/Hello.ns` and `b/Hello.ns` -- would otherwise share a name in the run list.
        runManager.setUniqueNameIfNeeded(created)
        created.isTemporary = true
        runManager.addConfiguration(created)
    }
    runManager.selectedConfiguration = settings
    return settings
}
