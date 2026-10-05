package io.github.bstrauss.newspeak.run

import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.ConfigurationTypeBase
import com.intellij.execution.configurations.ConfigurationTypeUtil
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.openapi.project.Project
import io.github.bstrauss.newspeak.NewspeakIcons

/** The "Newspeak" group of run configurations, with the plugin's file icon. */
class NewspeakConfigurationType : ConfigurationTypeBase(
    ID,
    "Newspeak",
    "Runs or benchmarks a Newspeak project or module with ns-next",
    NewspeakIcons.FILE,
) {
    init {
        addFactory(Factory(this))
    }

    /** The type's one factory: running and benchmarking are a choice inside a configuration, not two factories. */
    val factory: ConfigurationFactory get() = configurationFactories.single()

    private class Factory(type: NewspeakConfigurationType) : ConfigurationFactory(type) {
        override fun getId(): String = ID

        override fun createTemplateConfiguration(project: Project): RunConfiguration =
            NewspeakRunConfiguration(project, this, "Newspeak")

        override fun getOptionsClass(): Class<NewspeakRunConfigurationOptions> =
            NewspeakRunConfigurationOptions::class.java
    }

    companion object {
        const val ID = "NewspeakRunConfiguration"

        fun getInstance(): NewspeakConfigurationType =
            ConfigurationTypeUtil.findConfigurationType(NewspeakConfigurationType::class.java)
    }
}
