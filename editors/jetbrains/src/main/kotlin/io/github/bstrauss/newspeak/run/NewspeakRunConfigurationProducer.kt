package io.github.bstrauss.newspeak.run

import com.intellij.execution.actions.ConfigurationContext
import com.intellij.execution.actions.LazyRunConfigurationProducer
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.openapi.util.Ref
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.PsiElement
import com.intellij.psi.util.PsiTreeUtil

/**
 * "Run" in the context menu of a `newspeak.json`, in the project view and the
 * editor: the project, or the script whose key the caret is on, configured
 * as the gutter's play button configures it.
 */
class NewspeakRunConfigurationProducer : LazyRunConfigurationProducer<NewspeakRunConfiguration>() {
    override fun getConfigurationFactory(): ConfigurationFactory = NewspeakConfigurationType.getInstance().factory

    override fun setupConfigurationFromContext(
        configuration: NewspeakRunConfiguration,
        context: ConfigurationContext,
        sourceElement: Ref<PsiElement>,
    ): Boolean {
        val target = contextTarget(context) ?: return false
        configuration.projectFile = target.file.path
        configuration.script = target.script
        configuration.command = NewspeakCommand.RUN
        configuration.name = gutterConfigurationName(target.file, target.script, NewspeakCommand.RUN)
        return true
    }

    override fun isConfigurationFromContext(configuration: NewspeakRunConfiguration, context: ConfigurationContext): Boolean {
        val target = contextTarget(context) ?: return false
        return configuration.namesProjectFile(target.file) &&
            configuration.script == target.script &&
            configuration.command == NewspeakCommand.RUN
    }

    private data class ContextTarget(val file: VirtualFile, val script: String?)

    /** The project file and script a context inside a `newspeak.json` runs; null outside one. */
    private fun contextTarget(context: ConfigurationContext): ContextTarget? {
        val element = context.psiLocation ?: return null
        val file = element.containingFile ?: return null
        if (file.name != "newspeak.json") return null
        val virtualFile = file.virtualFile ?: return null
        // The project view hands over the file itself, whose first leaf is no key: that runs the project.
        val entry = projectEntryAt(PsiTreeUtil.getDeepestFirst(element))
        return ContextTarget(virtualFile, (entry as? ProjectEntry.Script)?.name)
    }
}
