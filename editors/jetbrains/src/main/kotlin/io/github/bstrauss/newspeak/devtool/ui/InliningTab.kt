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

    private val tree = Tree(treeModel).apply { isRootVisible = false }

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

    /** Opens what [node] stands for. */
    fun open(node: InliningNode) {
        node.body?.let { body -> navigate(project, run, body, node.at) }
    }

    private fun bodyName(body: Int): String = run.bodies[body]?.name ?: "#$body"

    private fun refresh() {
        val root = DefaultMutableTreeNode()
        for ((_, plans) in run.inlining.toSortedMap()) for (plan in plans) root.add(planNode(plan))
        treeModel.setRoot(root)
    }


    private fun planNode(plan: Inlining): DefaultMutableTreeNode {
        val outcome = when {
            plan.installed != null -> NewspeakBundle.message("devtool.inlining.installed", plan.installed.toString())
            else -> NewspeakBundle.message("devtool.inlining.refused", plan.refused ?: "")
        }
        val root = DefaultMutableTreeNode(InliningNode("${bodyName(plan.body)} -- $outcome", plan.body, null))
        val nodes = HashMap<Int, DefaultMutableTreeNode>()
        for (scope in plan.scopes) {
            nodes[scope.scope] = DefaultMutableTreeNode(InliningNode(bodyName(scope.body), scope.body, null))
        }
        // A scope under the scope whose send inlined it; one with no known parent hangs from the plan's root.
        for (scope in plan.scopes) {
            val node = nodes.getValue(scope.scope)
            (scope.parent?.let { parent -> nodes[parent.scope] } ?: root).add(node)
        }
        val bodies = plan.scopes.associate { scope -> scope.scope to scope.body }
        for (arm in plan.arms) {
            val owner = nodes[arm.scope] ?: continue
            val how = arm.inlinedScope?.let { scope -> NewspeakBundle.message("devtool.inlining.arm.inlined", bodies[scope]?.let(::bodyName) ?: "#$scope") }
                ?: NewspeakBundle.message("devtool.inlining.arm.always", arm.always ?: "")
            owner.add(DefaultMutableTreeNode(InliningNode("@${arm.at} ${arm.receiver}: $how", bodies[arm.scope], arm.at)))
        }
        for (decline in plan.declined) {
            val owner = nodes[decline.scope] ?: continue
            val callee = decline.callee?.let(::bodyName) ?: NewspeakBundle.message("devtool.inlining.no.body")
            owner.add(DefaultMutableTreeNode(InliningNode("@${decline.at} declined $callee ${decline.reason}", bodies[decline.scope], decline.at)))
        }
        return root
    }
}
