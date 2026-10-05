package io.github.bstrauss.newspeak.run

import com.intellij.execution.actions.ConfigurationContext
import com.intellij.execution.actions.LazyRunConfigurationProducer
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.util.Ref
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.PsiElement
import io.github.bstrauss.newspeak.NewspeakFile
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache

/**
 * "Run" in the context menu of a module ns-next can run from its source, in
 * the project view and anywhere in its editor, configured as the play button
 * beside `main:args:` configures it.
 */
class NewspeakModuleRunConfigurationProducer : LazyRunConfigurationProducer<NewspeakRunConfiguration>() {
    override fun getConfigurationFactory(): ConfigurationFactory = NewspeakConfigurationType.getInstance().factory

    override fun setupConfigurationFromContext(
        configuration: NewspeakRunConfiguration,
        context: ConfigurationContext,
        sourceElement: Ref<PsiElement>,
    ): Boolean {
        val file = runnableModule(context) ?: return false
        configuration.program = file.path
        configuration.script = null
        configuration.command = NewspeakCommand.RUN
        configuration.name = gutterConfigurationName(file, null, NewspeakCommand.RUN)
        return true
    }

    // Recognising a configuration needs no symbols: it is this module's if it names the file.
    override fun isConfigurationFromContext(configuration: NewspeakRunConfiguration, context: ConfigurationContext): Boolean {
        val file = moduleFile(context) ?: return false
        return configuration.namesProgram(file) && configuration.script == null && configuration.command == NewspeakCommand.RUN
    }

    private fun moduleFile(context: ConfigurationContext): VirtualFile? =
        (context.psiLocation?.containingFile as? NewspeakFile)?.virtualFile

    private fun runnableModule(context: ConfigurationContext): VirtualFile? {
        val file = moduleFile(context) ?: return null
        val cache = DocumentSymbolCache.getInstance(context.project)
        // Asked once per gesture, so waiting for the server off the EDT is affordable, as the
        // structure view's question is; on the EDT only a kept answer counts.
        val symbols = cache.current(file)
            ?: if (ApplicationManager.getApplication().isDispatchThread) null else cache.fetch(file)
        return file.takeIf { symbols != null && runnableMain(symbols) != null }
    }
}
