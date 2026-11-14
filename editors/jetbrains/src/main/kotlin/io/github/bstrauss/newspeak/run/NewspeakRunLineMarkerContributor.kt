package io.github.bstrauss.newspeak.run

import com.intellij.execution.lineMarker.RunLineMarkerContributor
import com.intellij.icons.AllIcons
import com.intellij.openapi.project.DumbAware
import com.intellij.psi.PsiElement
import io.github.bstrauss.newspeak.NewspeakBundle

/**
 * The play button beside `main` and each script of a `newspeak.json`, opening
 * Run and Benchmark. Dumb-aware: it reads the file's own PSI, no index.
 */
class NewspeakRunLineMarkerContributor : RunLineMarkerContributor(), DumbAware {
    override fun getInfo(element: PsiElement): Info? {
        // A marker belongs on a leaf, so each key is asked once, not once per enclosing element.
        if (element.firstChild != null) return null
        val entry = projectEntryAt(element) ?: return null
        val file = element.containingFile.virtualFile ?: return null
        val script = (entry as? ProjectEntry.Script)?.name
        return Info(
            AllIcons.RunConfigurations.TestState.Run,
            playButtonActions(file, script),
        ) { _ -> if (script == null) {
            NewspeakBundle.message("run.marker.tooltip.project")
        } else {
            NewspeakBundle.message("run.marker.tooltip.script", script)
        } }
    }
}
