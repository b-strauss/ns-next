package io.github.bstrauss.newspeak.run

import com.intellij.openapi.fileChooser.FileChooserDescriptorFactory
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.ui.ComboBox
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.openapi.util.io.FileUtil
import com.intellij.ui.JBIntSpinner
import com.intellij.ui.components.JBTextField
import com.intellij.ui.components.fields.ExpandableTextField
import com.intellij.ui.dsl.builder.AlignX
import com.intellij.ui.dsl.builder.panel
import com.intellij.ui.dsl.listCellRenderer.textListCellRenderer
import com.intellij.ui.layout.selectedValueIs
import com.intellij.util.execution.ParametersListUtil
import javax.swing.JComponent

/** The form of a Newspeak run configuration: the project, the command, and what the command is given. */
class NewspeakRunConfigurationEditor : SettingsEditor<NewspeakRunConfiguration>() {
    private lateinit var projectFileField: TextFieldWithBrowseButton
    private lateinit var commandBox: ComboBox<NewspeakCommand>
    private lateinit var scriptField: JBTextField
    private lateinit var argumentsField: ExpandableTextField
    private lateinit var iterationsSpinner: JBIntSpinner

    // Built at construction rather than in createEditor, so resetEditorFrom finds the fields whatever the order.
    private val form = panel {
        row("Project file:") {
            projectFileField = textFieldWithBrowseButton(
                FileChooserDescriptorFactory.singleFile()
                    .withTitle("Choose the Project's newspeak.json")
                    .withFileFilter { file -> file.name == "newspeak.json" },
            )
                .align(AlignX.FILL)
                .component
        }
        row("Command:") {
            commandBox = comboBox(
                NewspeakCommand.entries,
                textListCellRenderer { command ->
                    when (command) {
                        NewspeakCommand.RUN -> "Run"
                        NewspeakCommand.BENCH -> "Benchmark"
                        null -> null
                    }
                },
            ).component
        }
        row("Script:") {
            scriptField = textField()
                .align(AlignX.FILL)
                .comment("Empty for none")
                .component
        }
        row("Arguments:") {
            // The same splitting the command line applies, so what the expanded editor shows is what ns-next gets.
            argumentsField = expandableTextField(ParametersListUtil.DEFAULT_LINE_PARSER, ParametersListUtil.DEFAULT_LINE_JOINER)
                .align(AlignX.FILL)
                .component
        }
        row("Iterations:") {
            iterationsSpinner = spinner(1..Int.MAX_VALUE).component
        }.enabledIf(commandBox.selectedValueIs(NewspeakCommand.BENCH))
    }

    override fun createEditor(): JComponent = form

    override fun resetEditorFrom(configuration: NewspeakRunConfiguration) {
        projectFileField.text = FileUtil.toSystemDependentName(configuration.projectFile.orEmpty())
        commandBox.selectedItem = configuration.command
        scriptField.text = configuration.script.orEmpty()
        argumentsField.text = configuration.programArguments.orEmpty()
        iterationsSpinner.number = configuration.iterations
    }

    override fun applyEditorTo(configuration: NewspeakRunConfiguration) {
        // Stored system-independently, as the gutter stores it, whatever the chooser showed.
        configuration.projectFile = FileUtil.toSystemIndependentName(projectFileField.text.trim()).ifEmpty { null }
        configuration.command = commandBox.selectedItem as? NewspeakCommand ?: NewspeakCommand.RUN
        configuration.script = scriptField.text.trim().ifEmpty { null }
        configuration.programArguments = argumentsField.text.ifBlank { null }
        configuration.iterations = iterationsSpinner.number
    }
}
