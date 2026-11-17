package io.github.bstrauss.newspeak.debugger.memory

import com.google.gson.JsonObject
import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.DumbAwareAction
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.actionSystem.ActionManager
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.DefaultActionGroup
import com.intellij.ui.JBSplitter
import com.intellij.ui.ScrollPaneFactory
import com.intellij.ui.TableSpeedSearch
import com.intellij.ui.table.JBTable
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebugSessionListener
import com.intellij.xdebugger.frame.XCompositeNode
import com.intellij.xdebugger.frame.XValue
import com.intellij.xdebugger.frame.XValueNode
import com.intellij.xdebugger.frame.XValuePlace
import com.intellij.xdebugger.impl.ui.tree.XDebuggerTree
import com.intellij.xdebugger.impl.ui.tree.nodes.XValueNodeImpl
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.debugger.NewspeakDebuggerEditorsProvider
import io.github.bstrauss.newspeak.debugger.addPage
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapInstanceCount
import io.github.bstrauss.newspeak.debugger.messageOf
import java.awt.BorderLayout
import java.util.concurrent.CompletableFuture
import javax.swing.JComponent
import javax.swing.JPanel
import javax.swing.ListSelectionModel
import javax.swing.table.AbstractTableModel

/**
 * The memory view of a Newspeak debug session, a panel of the session's
 * tab as Java's Memory view is (`debugger.md`, "The memory view"): at a
 * stop, the instances the object space holds of each class, the largest
 * count first (`nsNext/instanceCounts`); a class chosen lists its
 * instances as value trees, expanded through `variables`
 * (`nsNext/instances`), and a value's Show Referring Objects lists what
 * refers to it (`nsNext/referrers`, through the value's referrers
 * provider). The counts are a walk of the whole object space, so they are
 * asked only at a stop the panel is shown at, and on Refresh; a stop
 * left drops the instances, whose references the next stop no longer
 * knows.
 */
class NewspeakMemoryView(private val session: XDebugSession, private val client: () -> DapClient?) : Disposable {
    private val model = CountsModel()

    @Volatile
    private var built: Built? = null

    init {
        session.addSessionListener(
            object : XDebugSessionListener {
                override fun sessionPaused() {
                    // Whether the panel is shown is Swing's to say, on the event dispatch thread.
                    ApplicationManager.getApplication().invokeLater { if (built?.panel?.isShowing == true) refresh() }
                }

                override fun sessionResumed() {
                    ApplicationManager.getApplication().invokeLater { clear() }
                }
            },
            this,
        )
    }

    /** The counts of the stop, asked of the agent and shown; none while the session has no stop. */
    fun refresh(): CompletableFuture<List<DapInstanceCount>> {
        val client = client() ?: return CompletableFuture.completedFuture(emptyList())
        return client.request("nsNext/instanceCounts").thenApply { body ->
            val classes = body.get("classes")
                ?.takeIf { element -> element.isJsonArray }
                ?.asJsonArray
                ?.filter { element -> element.isJsonObject }
                ?.map { element -> DapInstanceCount.from(element.asJsonObject) }
                .orEmpty()
            ApplicationManager.getApplication().invokeLater { model.show(classes) }
            classes
        }
    }

    /** The instances of [counted]'s class, a value whose children are they, a page at a time. */
    fun instancesOf(counted: DapInstanceCount): XValue = Instances(counted)

    /** The panel the session's tab shows, made once, on the event dispatch thread. */
    val component: JComponent
        get() = (built ?: Built().also { made -> built = made }).panel

    private fun clear() {
        model.show(emptyList())
        built?.showInstances(null)
    }

    override fun dispose() {}

    /** The classes and their counts, as the table shows them. */
    private class CountsModel : AbstractTableModel() {
        var rows: List<DapInstanceCount> = emptyList()
            private set

        fun show(classes: List<DapInstanceCount>) {
            rows = classes
            fireTableDataChanged()
        }

        override fun getRowCount(): Int = rows.size

        override fun getColumnCount(): Int = 2

        override fun getColumnName(column: Int): String =
            NewspeakBundle.message(if (column == 0) "debugger.memory.class" else "debugger.memory.count")

        override fun getColumnClass(column: Int): Class<*> = if (column == 0) String::class.java else Integer::class.java

        override fun getValueAt(row: Int, column: Int): Any {
            val counted = rows[row]
            if (column == 1) return counted.count
            return if (counted.shipped) NewspeakBundle.message("debugger.memory.shipped", counted.name) else counted.name
        }
    }

    /** A class's instances as the children of one value, named `#<n>` by the agent. */
    private inner class Instances(private val counted: DapInstanceCount) : XValue() {
        override fun computePresentation(node: XValueNode, place: XValuePlace) {
            node.setPresentation(AllIcons.Debugger.Value, counted.name, NewspeakBundle.message("debugger.memory.instances", counted.count), counted.count > 0)
        }

        override fun computeChildren(node: XCompositeNode) {
            val client = client() ?: return node.setErrorMessage(NewspeakBundle.message("debugger.memory.no.stop"))
            addPage(client, "nsNext/instances", 0, counted.count, node) {
                addProperty("name", counted.name)
                if (counted.shipped) addProperty("shipped", true)
            }
        }
    }

    /** The panel: the counts above, the chosen class's instances below. */
    private inner class Built {
        private val table = JBTable(model).apply {
            setSelectionMode(ListSelectionModel.SINGLE_SELECTION)
            emptyText.text = NewspeakBundle.message("debugger.memory.empty")
        }
        private val instancesPanel = JPanel(BorderLayout())
        private var tree: XDebuggerTree? = null
        val panel = JPanel(BorderLayout())

        init {
            TableSpeedSearch.installOn(table)
            table.selectionModel.addListSelectionListener { event ->
                if (!event.valueIsAdjusting) showInstances(model.rows.getOrNull(table.selectedRow))
            }
            val refresh = object : DumbAwareAction(
                NewspeakBundle.message("debugger.memory.refresh"),
                null,
                AllIcons.Actions.Refresh,
            ) {
                override fun actionPerformed(event: AnActionEvent) {
                    refresh().whenComplete { _, failure ->
                        if (failure != null) session.reportError(NewspeakBundle.message("debugger.error.request", messageOf(failure)))
                    }
                }
            }
            val toolbar = ActionManager.getInstance().createActionToolbar("NewspeakMemoryView", DefaultActionGroup(refresh), true)
            toolbar.targetComponent = panel
            val splitter = JBSplitter(true, SPLIT).apply {
                firstComponent = ScrollPaneFactory.createScrollPane(table)
                secondComponent = instancesPanel
            }
            panel.add(toolbar.component, BorderLayout.NORTH)
            panel.add(splitter, BorderLayout.CENTER)
        }

        fun showInstances(counted: DapInstanceCount?) {
            tree?.let(Disposer::dispose)
            tree = null
            instancesPanel.removeAll()
            if (counted != null) {
                val shown = XDebuggerTree(session.project, NewspeakDebuggerEditorsProvider, null, INSPECT_TREE_POPUP_GROUP, null)
                Disposer.register(this@NewspeakMemoryView, shown)
                val root = XValueNodeImpl(shown, null, counted.name, instancesOf(counted))
                shown.setRoot(root, true)
                shown.expandNodesOnLoad { node -> node == root }
                instancesPanel.add(ScrollPaneFactory.createScrollPane(shown), BorderLayout.CENTER)
                tree = shown
            }
            instancesPanel.revalidate()
            instancesPanel.repaint()
        }
    }

    private companion object {
        /** The value trees' menu, which offers Show Referring Objects and Mark Object…. */
        const val INSPECT_TREE_POPUP_GROUP = "XDebugger.Inspect.Tree.Popup"

        /** The share of the panel the counts take. */
        const val SPLIT = 0.5f
    }
}
