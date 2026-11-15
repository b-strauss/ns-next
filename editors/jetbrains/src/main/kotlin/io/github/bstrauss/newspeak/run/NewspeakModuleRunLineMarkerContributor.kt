package io.github.bstrauss.newspeak.run

import com.intellij.execution.lineMarker.RunLineMarkerContributor
import com.intellij.icons.AllIcons
import com.intellij.openapi.project.DumbAware
import com.intellij.platform.lsp.util.getOffsetInDocument
import com.intellij.psi.PsiDocumentManager
import com.intellij.psi.PsiElement
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes
import io.github.bstrauss.newspeak.lsp.DocumentSymbolCache

/**
 * The play button beside `main:args:` of a module ns-next can run from its
 * source (`runnableMain`), opening Run, Debug and Benchmark. It reads the language
 * server's symbols and never waits for them: a daemon pass holds a read
 * action every write action, typing included, would wait behind. Without a
 * current answer it asks for one and restarts the file's daemon once it is
 * kept, so after an edit the button returns as soon as the server answers.
 */
class NewspeakModuleRunLineMarkerContributor : RunLineMarkerContributor(), DumbAware {
    override fun getInfo(element: PsiElement): Info? {
        // Only the keyword `main:` can start the pattern; every other leaf is turned away before the cache is read.
        if (element.firstChild != null || element.node.elementType != NewspeakTokenTypes.KEYWORD || element.text != "main:") {
            return null
        }
        val psiFile = element.containingFile ?: return null
        val file = psiFile.virtualFile ?: return null
        val project = element.project
        val symbols = DocumentSymbolCache.getInstance(project).currentForDaemon(file) ?: return null
        val main = runnableMain(symbols) ?: return null
        val document = PsiDocumentManager.getInstance(project).getDocument(psiFile) ?: return null
        if (getOffsetInDocument(document, main.selectionRange.start) != element.textRange.startOffset) return null
        val className = symbols.first().name
        return Info(
            AllIcons.RunConfigurations.TestState.Run,
            playButtonActions(file, null),
        ) { _ -> NewspeakBundle.message("run.marker.tooltip.module", className) }
    }
}
