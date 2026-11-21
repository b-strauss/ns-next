package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.Disposable
import com.intellij.openapi.project.Project
import com.intellij.ui.components.JBScrollPane
import com.intellij.ui.treeStructure.Tree
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.model.Inlining
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.awt.BorderLayout
import java.awt.event.MouseAdapter
import java.awt.event.MouseEvent
import javax.swing.JPanel
import javax.swing.tree.DefaultMutableTreeNode
import javax.swing.tree.DefaultTreeModel
import javax.swing.tree.TreePath

/** A node of the inlining tree: its [label], and the [body] and send [at] a click opens, null where it opens nothing. */
class InliningNode(val label: String, val body: Int?, val at: Int?) {
    override fun toString(): String = label
}

/**
 * The Inlining tab (`docs/design/developer-tool.md`, decision 29): a tree
 * for each optimizing compile's plan, its root the compiled body with the
 * outcome, its scopes nested under the scope whose send inlined them, and
 * under each scope its type-case arms and the callees declined at its
 * sends. A click opens the scope's body, at the send a leaf names.
 */
class InliningTab(private val project: Project, private val run: TraceRun, parent: Disposable) : JPanel(BorderLayout()) {
    /** One root per plan, in the order the file recorded them. */
    val treeModel = DefaultTreeModel(DefaultMutableTreeNode())

    val tree = Tree(treeModel).apply { isRootVisible = false }

    /** How many of the run's plans the tree holds. */
    private var fed = 0

    init {
        add(JBScrollPane(tree), BorderLayout.CENTER)
        tree.addMouseListener(
            object : MouseAdapter() {
                override fun mouseClicked(event: MouseEvent) {
                    val path = tree.getPathForLocation(event.x, event.y) ?: return
                    ((path.lastPathComponent as? DefaultMutableTreeNode)?.userObject as? InliningNode)?.let(::open)
                }
            },
        )
        refresh()
        run.onChanged(parent) { refresh() }
    }

    /** Selects the last plan of the method [body] -- of the method itself or a block of it -- if it has one. */
    fun select(body: Int) {
        val root = treeModel.root as DefaultMutableTreeNode
        val plan = root.children().toList().map { node -> node as DefaultMutableTreeNode }
            .lastOrNull { node -> (node.userObject as? InliningNode)?.body?.let { planned -> rootOf(run, planned) } == body } ?: return
        val path = TreePath(plan.path)
        tree.selectionPath = path
        tree.scrollPathToVisible(path)
    }

    /** Opens what [node] stands for. */
    fun open(node: InliningNode) {
        node.body?.let { body -> navigate(project, run, body, node.at) }
    }

    /** A batch arrived: the plans after the last fed are appended, which leaves the tree's expansion and selection where they are. */
    private fun refresh() {
        val root = treeModel.root as DefaultMutableTreeNode
        val plans = run.plans
        for (index in fed until plans.size) treeModel.insertNodeInto(planNode(plans[index]), root, root.childCount)
        fed = plans.size
    }

    private fun planNode(plan: Inlining): DefaultMutableTreeNode {
        val outcome = when {
            plan.installed != null -> NewspeakBundle.message("devtool.inlining.installed", plan.installed.toString())
            else -> NewspeakBundle.message("devtool.inlining.refused", plan.refused ?: "")
        }
        val root = DefaultMutableTreeNode(InliningNode("${bodyName(run, plan.body)} -- $outcome", plan.body, null))
        val bodies = plan.scopes.associate { scope -> scope.scope to scope.body }
        val nodes = HashMap<Int, DefaultMutableTreeNode>()
        for (scope in plan.scopes) {
            // A click goes to the send that inlined the scope, in its parent's body; the root scope has none, so its own body.
            val target = scope.parent?.let { parent -> InliningNode(bodyName(run, scope.body), bodies[parent.scope], parent.at) }
            nodes[scope.scope] = DefaultMutableTreeNode(target ?: InliningNode(bodyName(run, scope.body), scope.body, null))
        }
        // A scope under the scope whose send inlined it; one with no known parent hangs from the plan's root.
        for (scope in plan.scopes) {
            val node = nodes.getValue(scope.scope)
            (scope.parent?.let { parent -> nodes[parent.scope] } ?: root).add(node)
        }

        for (arm in plan.arms) {
            val owner = nodes[arm.scope] ?: continue
            val how = arm.inlinedScope?.let { scope -> NewspeakBundle.message("devtool.inlining.arm.inlined", bodies[scope]?.let { body -> bodyName(run, body) } ?: "#$scope") }
                ?: NewspeakBundle.message("devtool.inlining.arm.always", arm.always ?: "")
            owner.add(DefaultMutableTreeNode(InliningNode("@${arm.at} ${arm.receiver}: $how", bodies[arm.scope], arm.at)))
        }
        for (decline in plan.declined) {
            val owner = nodes[decline.scope] ?: continue
            val callee = decline.callee?.let { body -> bodyName(run, body) } ?: NewspeakBundle.message("devtool.inlining.no.body")
            owner.add(DefaultMutableTreeNode(InliningNode("@${decline.at} declined $callee ${decline.reason}", bodies[decline.scope], decline.at)))
        }
        return root
    }
}
