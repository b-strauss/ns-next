package io.github.bstrauss.newspeak.run

import com.intellij.execution.lineMarker.RunLineMarkerContributor
import com.intellij.icons.AllIcons
import com.intellij.openapi.project.DumbAware
import com.intellij.psi.PsiElement

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
            arrayOf(NewspeakRunAction(file, script, NewspeakCommand.RUN), NewspeakRunAction(file, script, NewspeakCommand.BENCH)),
        ) { _ -> if (script == null) "Run the project" else "Run script '$script'" }
    }
}
