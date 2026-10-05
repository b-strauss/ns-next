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

/** One entry of the play button's menu: runs or benchmarks the project, or one of its scripts. */
class NewspeakRunAction(
    val projectFile: VirtualFile,
    val script: String?,
    val command: NewspeakCommand,
) : AnAction(
    when (command) {
        NewspeakCommand.RUN -> "Run"
        NewspeakCommand.BENCH -> "Benchmark"
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
        val settings = createConfigurationFromGutter(project, projectFile, script, command)
        // An invalid configuration -- no ns-next path, say -- takes the platform's own path:
        // its error, and the dialog to edit the configuration.
        ProgramRunnerUtil.executeConfiguration(settings, DefaultRunExecutor.getRunExecutorInstance())
    }
}

/** What the gutter names a configuration: the project's directory, the script, and whether it benchmarks. */
fun gutterConfigurationName(projectFile: VirtualFile, script: String?, command: NewspeakCommand): String {
    val projectName = projectFile.parent?.name ?: projectFile.name
    val scriptPart = script?.let { name -> " [$name]" }.orEmpty()
    val benchmarkPart = if (command == NewspeakCommand.BENCH) " (benchmark)" else ""
    return projectName + scriptPart + benchmarkPart
}

/**
 * The configuration the play button runs, selected but not started: the
 * existing one with the same project file, script and command, or a new
 * temporary one from the type's template.
 */
fun createConfigurationFromGutter(
    project: Project,
    projectFile: VirtualFile,
    script: String?,
    command: NewspeakCommand,
): RunnerAndConfigurationSettings {
    val runManager = RunManager.getInstance(project)
    val existing = runManager.getConfigurationSettingsList(NewspeakConfigurationType.getInstance())
        .firstOrNull { settings ->
            val configuration = settings.configuration as? NewspeakRunConfiguration
            configuration != null &&
                configuration.namesProjectFile(projectFile) &&
                configuration.script == script &&
                configuration.command == command
        }
    val settings = existing ?: runManager.createConfiguration(
        gutterConfigurationName(projectFile, script, command),
        NewspeakConfigurationType.getInstance().factory,
    ).also { created ->
        (created.configuration as NewspeakRunConfiguration).apply {
            this.projectFile = projectFile.path
            this.script = script
            this.command = command
        }
        // Two projects in directories of the same name would otherwise share a name in the run list.
        runManager.setUniqueNameIfNeeded(created)
        created.isTemporary = true
        runManager.addConfiguration(created)
    }
    runManager.selectedConfiguration = settings
    return settings
}
