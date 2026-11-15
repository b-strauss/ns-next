package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.progress.ProgressManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.DialogWrapper
import com.intellij.openapi.util.ThrowableComputable
import com.intellij.ui.CollectionListModel
import com.intellij.ui.ColoredListCellRenderer
import com.intellij.ui.ScrollPaneFactory
import com.intellij.ui.SimpleTextAttributes
import com.intellij.ui.ToolbarDecorator
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBList
import com.intellij.ui.treeStructure.Tree
import com.intellij.util.concurrency.AppExecutorUtil
import com.intellij.util.ui.JBUI
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache
import java.awt.BorderLayout
import java.awt.Dimension
import javax.swing.JComponent
import javax.swing.JList
import javax.swing.JPanel
import javax.swing.ListSelectionModel
import javax.swing.tree.DefaultMutableTreeNode
import javax.swing.tree.DefaultTreeModel
import javax.swing.tree.TreeSelectionModel

/**
 * A project's page under Settings | Build, Execution, Deployment |
 * Debugger, "Newspeak Step Filters": the classes its steps do not go
 * into, as Java's "Do not step into the classes" ([NewspeakSkippedClasses]).
 * An entry is added by choosing a module's file or a class in it from a
 * tree of the project's modules and the core library's, never typed, so
 * that it names exactly one module; one whose file is gone or whose class
 * the file no longer declares is shown as invalid.
 */
class NewspeakStepFiltersConfigurable(private val project: Project) : Configurable {
    /** The entries the page shows; for the tests too. */
    internal val entries = CollectionListModel<SkippedClass>()

    /** The entries found invalid, decided off the event dispatch thread. */
    @Volatile
    internal var invalid: Set<SkippedClass> = emptySet()

    private var list: JBList<SkippedClass>? = null

    override fun getDisplayName(): String = NewspeakBundle.message("debugger.step.filters.display.name")

    override fun createComponent(): JComponent {
        val shown = JBList(entries).apply {
            selectionMode = ListSelectionModel.SINGLE_SELECTION
            cellRenderer = EntryRenderer()
        }
        list = shown
        val decorated = ToolbarDecorator.createDecorator(shown)
            .setAddAction { _ -> choose()?.let { entry -> if (entry !in entries.items) entries.add(entry) } }
            .setRemoveAction { _ -> shown.selectedIndex.takeIf { index -> index >= 0 }?.let(entries::remove) }
            .disableUpDownActions()
            .createPanel()
        reset()
        return JPanel(BorderLayout(0, JBUI.scale(GAP))).apply {
            add(JBLabel(NewspeakBundle.message("debugger.settings.skipped.classes")), BorderLayout.NORTH)
            add(decorated, BorderLayout.CENTER)
        }
    }

    /** The entry the user chooses from the tree; null when cancelled. The tree is gathered under a progress, the server waited for. */
    private fun choose(): SkippedClass? {
        val choices = ProgressManager.getInstance().runProcessWithProgressSynchronously(
            ThrowableComputable<StepFilterChoices, RuntimeException> { gatherChoices(project) },
            NewspeakBundle.message("debugger.step.filters.gathering"),
            true,
            project,
        )
        val dialog = ChooserDialog(choices)
        return if (dialog.showAndGet()) dialog.chosen else null
    }

    override fun isModified(): Boolean = entries.items != NewspeakSkippedClasses.getInstance(project).entries

    override fun apply() {
        NewspeakSkippedClasses.getInstance(project).update(entries.items.toList())
        checkEntries()
    }

    override fun reset() {
        entries.replaceAll(NewspeakSkippedClasses.getInstance(project).entries)
        checkEntries()
    }

    override fun disposeUIResources() {
        list = null
    }

    /**
     * Decides which entries are invalid on a pooled thread, since a file's
     * classes are the language server's answer, and repaints the list.
     * Answers the decision's future; for the tests.
     */
    internal fun checkEntries(): java.util.concurrent.Future<*> {
        val checked = entries.items.toList()
        val skipped = NewspeakSkippedClasses.getInstance(project)
        val symbols = DocumentSymbolCache.getInstance(project)
        return AppExecutorUtil.getAppExecutorService().submit {
            val found = checked.filterNot { entry ->
                val file = skipped.fileOf(entry) ?: return@filterNot false
                val answered = symbols.current(file) ?: symbols.fetch(file) ?: return@filterNot true
                entry.classPath in classPathsOf(answered)
            }.toSet()
            invalid = found
            ApplicationManager.getApplication().invokeLater({ list?.repaint() }, ModalityState.any())
        }
    }

    /** An entry as its file and class, the library's marked, an invalid one in the error style. */
    private inner class EntryRenderer : ColoredListCellRenderer<SkippedClass>() {
        override fun customizeCellRenderer(list: JList<out SkippedClass>, value: SkippedClass, index: Int, selected: Boolean, focus: Boolean) {
            val broken = value in invalid
            append(value.path, if (broken) SimpleTextAttributes.ERROR_ATTRIBUTES else SimpleTextAttributes.REGULAR_ATTRIBUTES)
            if (value.classPath.isNotEmpty()) append("  " + value.classPath.joinToString("."), SimpleTextAttributes.REGULAR_BOLD_ATTRIBUTES)
            if (value.inLibrary) append("  " + NewspeakBundle.message("debugger.step.filters.library"), SimpleTextAttributes.GRAYED_ATTRIBUTES)
            if (broken) append("  " + NewspeakBundle.message("debugger.step.filters.invalid"), SimpleTextAttributes.ERROR_ATTRIBUTES)
        }
    }

    /** The tree to choose from: the project's modules and the core library's, each with its classes. */
    private inner class ChooserDialog(choices: StepFilterChoices) : DialogWrapper(project, true) {
        var chosen: SkippedClass? = null
            private set

        private val tree = Tree(DefaultTreeModel(rootOf(choices))).apply {
            isRootVisible = false
            selectionModel.selectionMode = TreeSelectionModel.SINGLE_TREE_SELECTION
            addTreeSelectionListener { _ ->
                chosen = ((lastSelectedPathComponent as? DefaultMutableTreeNode)?.userObject as? StepFilterChoice)?.entry
                isOKActionEnabled = chosen != null
            }
        }

        init {
            title = NewspeakBundle.message("debugger.step.filters.chooser.title")
            isOKActionEnabled = false
            init()
        }

        override fun createCenterPanel(): JComponent =
            ScrollPaneFactory.createScrollPane(tree).apply { preferredSize = Dimension(JBUI.scale(WIDTH), JBUI.scale(HEIGHT)) }

        override fun getPreferredFocusedComponent(): JComponent = tree
    }

    private companion object {
        const val GAP = 4
        const val WIDTH = 480
        const val HEIGHT = 400
    }
}

/** The chooser's tree: a group of the project's modules and one of the core library's, each choice a node with its classes below. */
internal fun rootOf(choices: StepFilterChoices): DefaultMutableTreeNode {
    fun nodeOf(choice: StepFilterChoice): DefaultMutableTreeNode =
        DefaultMutableTreeNode(choice).apply { choice.children.forEach { child -> add(nodeOf(child)) } }
    return DefaultMutableTreeNode().apply {
        for ((name, group) in listOf(
            NewspeakBundle.message("debugger.step.filters.group.project") to choices.project,
            NewspeakBundle.message("debugger.step.filters.group.library") to choices.library,
        )) {
            if (group.isEmpty()) continue
            add(DefaultMutableTreeNode(name).apply { group.forEach { choice -> add(nodeOf(choice)) } })
        }
    }
}
