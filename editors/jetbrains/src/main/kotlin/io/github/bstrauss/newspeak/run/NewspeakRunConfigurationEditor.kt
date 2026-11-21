package io.github.bstrauss.newspeak.run

import com.intellij.openapi.fileChooser.FileChooserDescriptorFactory
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.ui.ComboBox
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.openapi.util.io.FileUtil
import com.intellij.ui.DocumentAdapter
import com.intellij.ui.JBIntSpinner
import com.intellij.ui.components.JBTextField
import com.intellij.ui.components.fields.ExpandableTextField
import com.intellij.ui.dsl.builder.AlignX
import com.intellij.ui.dsl.builder.panel
import com.intellij.ui.dsl.listCellRenderer.textListCellRenderer
import com.intellij.ui.layout.ComponentPredicate
import com.intellij.ui.layout.selectedValueIs
import com.intellij.util.execution.ParametersListUtil
import io.github.bstrauss.newspeak.NewspeakBundle
import java.awt.event.ItemEvent
import javax.swing.JComponent
import javax.swing.event.DocumentEvent

/** The form of a Newspeak run configuration: the program, the command, and what the command is given. */
class NewspeakRunConfigurationEditor : SettingsEditor<NewspeakRunConfiguration>() {
    private lateinit var programField: TextFieldWithBrowseButton
    private lateinit var commandBox: ComboBox<NewspeakCommand>
    private lateinit var scriptField: JBTextField
    private lateinit var argumentsField: ExpandableTextField
    private lateinit var runtimeOptionsField: ExpandableTextField
    private lateinit var iterationsSpinner: JBIntSpinner

    /** ns-next refuses `--script` with a `.ns` module, so the field is off for one. */
    private val programIsAProject = object : ComponentPredicate() {
        override fun invoke(): Boolean = !namesAModule(programField.text)

        override fun addListener(listener: (Boolean) -> Unit) {
            programField.textField.document.addDocumentListener(object : DocumentAdapter() {
                override fun textChanged(event: DocumentEvent) = listener(invoke())
            })
        }
    }

    // Built at construction rather than in createEditor, so resetEditorFrom finds the fields whatever the order.
    private val form = panel {
        row(NewspeakBundle.message("run.configuration.editor.program.label")) {
            programField = textFieldWithBrowseButton(
                FileChooserDescriptorFactory.singleFile()
                    .withTitle(NewspeakBundle.message("run.configuration.editor.program.chooser.title"))
                    .withFileFilter { file -> file.name == PROJECT_FILE_NAME || file.extension == "ns" },
            )
                .align(AlignX.FILL)
                .component
        }
        row(NewspeakBundle.message("run.configuration.editor.command.label")) {
            commandBox = comboBox(
                NewspeakCommand.entries,
                textListCellRenderer { command ->
                    when (command) {
                        NewspeakCommand.RUN -> NewspeakBundle.message("run.command.run")
                        NewspeakCommand.BENCH -> NewspeakBundle.message("run.command.benchmark")
                        null -> null
                    }
                },
            ).component
        }
        row(NewspeakBundle.message("run.configuration.editor.script.label")) {
            scriptField = textField()
                .align(AlignX.FILL)
                .comment(NewspeakBundle.message("run.configuration.editor.script.comment"))
                .component
        }.enabledIf(programIsAProject)
        row(NewspeakBundle.message("run.configuration.editor.arguments.label")) {
            // The same splitting the command line applies, so what the expanded editor shows is what ns-next gets.
            argumentsField = expandableTextField(ParametersListUtil.DEFAULT_LINE_PARSER, ParametersListUtil.DEFAULT_LINE_JOINER)
                .align(AlignX.FILL)
                .component
        }
        row(NewspeakBundle.message("run.configuration.editor.runtime.options.label")) {
            runtimeOptionsField = expandableTextField(ParametersListUtil.DEFAULT_LINE_PARSER, ParametersListUtil.DEFAULT_LINE_JOINER)
                .align(AlignX.FILL)
                .comment(NewspeakBundle.message("run.configuration.editor.runtime.options.comment"))
                .component
        }
        row(NewspeakBundle.message("run.configuration.editor.iterations.label")) {
            iterationsSpinner = spinner(1..Int.MAX_VALUE).component
        }.enabledIf(commandBox.selectedValueIs(NewspeakCommand.BENCH))
    }

    override fun createEditor(): JComponent = form

    /** Calls [listener] whenever the command shown changes, as the user picks it or a reset sets it. */
    fun onCommandChanged(listener: (NewspeakCommand) -> Unit) {
        commandBox.addItemListener { event ->
            if (event.stateChange == ItemEvent.SELECTED) (event.item as? NewspeakCommand)?.let(listener)
        }
    }

    override fun resetEditorFrom(configuration: NewspeakRunConfiguration) {
        programField.text = FileUtil.toSystemDependentName(configuration.program.orEmpty())
        commandBox.selectedItem = configuration.command
        scriptField.text = configuration.script.orEmpty()
        argumentsField.text = configuration.programArguments.orEmpty()
        runtimeOptionsField.text = configuration.runtimeOptions.orEmpty()
        iterationsSpinner.number = configuration.iterations
    }

    override fun applyEditorTo(configuration: NewspeakRunConfiguration) {
        // Stored system-independently, as the gutter stores it, whatever the chooser showed.
        configuration.program = FileUtil.toSystemIndependentName(programField.text.trim()).ifEmpty { null }
        configuration.command = commandBox.selectedItem as? NewspeakCommand ?: NewspeakCommand.RUN
        configuration.script = if (namesAModule(programField.text)) null else scriptField.text.trim().ifEmpty { null }
        configuration.programArguments = argumentsField.text.ifBlank { null }
        configuration.runtimeOptions = runtimeOptionsField.text.ifBlank { null }
        configuration.iterations = iterationsSpinner.number
    }
}
