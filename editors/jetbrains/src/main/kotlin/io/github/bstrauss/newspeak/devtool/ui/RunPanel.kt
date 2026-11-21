package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.project.Project
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBTabbedPane
import com.intellij.util.ui.JBUI
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.awt.BorderLayout
import javax.swing.JPanel

/**
 * One traced run's tab of the tool window: its four views -- Methods,
 * Events, Code and Inlining (`docs/design/developer-tool.md`, decision 29)
 * -- over [run], and above them the error that ended the reading of its
 * file, if one did, and a line naming the categories the run's header
 * lacks that the editor's tier icons need, so that an icon left out is
 * never a mystery.
 */
class RunPanel(project: Project, val run: TraceRun) : JPanel(BorderLayout()), Disposable {
    private val errorLabel = JBLabel(AllIcons.General.Error).apply {
        border = JBUI.Borders.empty(4, 8)
        isVisible = false
    }

    private val tierLabel = JBLabel(AllIcons.General.Information).apply {
        border = JBUI.Borders.empty(4, 8)
        isVisible = false
    }

    private val methods = MethodsTab(project, run, this)
    private val inlining = InliningTab(project, run, this)
    private val code = CodeTab(project, run, this)

    /** The views, in decision 29's order, which [View] follows. */
    val views = JBTabbedPane().apply {
        addTab(NewspeakBundle.message("devtool.view.methods"), methods)
        addTab(NewspeakBundle.message("devtool.view.events"), EventsTab(project, run, this@RunPanel))
        addTab(NewspeakBundle.message("devtool.view.code"), code)
        addTab(NewspeakBundle.message("devtool.view.inlining"), inlining)
    }

    /** The views by their place among [views]. */
    enum class View { METHODS, EVENTS, CODE, INLINING }

    /** Selects [view], and in it the method [body] where the view lists methods: the gutter icon's menu. */
    fun show(view: View, body: Int) {
        views.selectedIndex = view.ordinal
        when (view) {
            View.METHODS -> methods.select(body)
            View.INLINING -> inlining.select(body)
            View.CODE -> code.show(body)
            View.EVENTS -> Unit
        }
    }

    init {
        val notices = JPanel(BorderLayout()).apply {
            add(errorLabel, BorderLayout.NORTH)
            add(tierLabel, BorderLayout.SOUTH)
        }
        add(notices, BorderLayout.NORTH)
        add(views, BorderLayout.CENTER)
        showTierNotice()
        run.onChanged(this) { showTierNotice() }
    }

    private fun showTierNotice() {
        val notice = tierNotice(run.missingTierCategories())
        tierLabel.text = notice.orEmpty()
        tierLabel.isVisible = notice != null
    }

    /** The line naming the categories the tier icons need that the run did not trace; null while none is shown. */
    val shownTierNotice: String? get() = tierLabel.text.takeIf { tierLabel.isVisible }

    /** Releases nothing itself; its views' listeners are children of it, which the disposal of the run's tab ends. */
    override fun dispose() = Unit

    /** Shows [message], the reason the run's file is read no further. */
    fun showError(message: String) {
        errorLabel.text = message
        errorLabel.isVisible = true
    }

    /** The error shown; null while none is. */
    val shownError: String? get() = errorLabel.text.takeIf { errorLabel.isVisible }
}

/**
 * What the run's tab says of the [missing] categories the tier icons need
 * (`TraceRun.missingTierCategories`): without `compile` no icon shows how
 * a method ran, without `code-zone` none shows baseline, twin or optimized
 * code, and without `deoptimize` an optimized method shows as its baseline
 * tier; null where none is missing.
 */
fun tierNotice(missing: List<String>): String? {
    if (missing.isEmpty()) return null
    val names = categoryList(missing)
    return when {
        "compile" in missing -> NewspeakBundle.message("devtool.tiers.none", names)
        "code-zone" in missing -> NewspeakBundle.message("devtool.tiers.compiled", names)
        else -> NewspeakBundle.message("devtool.tiers.optimized", names)
    }
}

/** [categories] as a sentence names them: `a`, `a and b`, `a, b and c`. */
fun categoryList(categories: List<String>): String = when (categories.size) {
    1 -> categories.single()
    else -> categories.dropLast(1).joinToString(", ") + " and " + categories.last()
}
