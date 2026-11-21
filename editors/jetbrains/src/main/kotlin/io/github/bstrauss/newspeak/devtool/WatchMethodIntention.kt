package io.github.bstrauss.newspeak.devtool

import com.intellij.codeInsight.intention.IntentionAction
import com.intellij.codeInsight.intention.LowPriorityAction
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.psi.PsiFile
import io.github.bstrauss.newspeak.NewspeakBundle

/**
 * Watch or Unwatch the method at the caret from Alt+Enter, as the
 * context menu's [WatchMethodAction] does. Offered only once the server
 * has named the file's methods for its current text: availability never
 * waits, and asks for the names when they are missing.
 */
class WatchMethodIntention :
    IntentionAction,
    LowPriorityAction,
    DumbAware {
    /** The text the last availability check decided, which the platform reads after it. */
    @Volatile
    private var text: String = NewspeakBundle.message("devtool.watch.method.watch")

    override fun getText(): String = text

    override fun getFamilyName(): String = NewspeakBundle.message("devtool.watch.method.family")

    override fun isAvailable(project: Project, editor: Editor?, file: PsiFile?): Boolean {
        val virtualFile = file?.virtualFile?.takeIf(::isNewspeakSource) ?: return false
        if (editor == null) return false
        val watched = WatchedMethods.getInstance(project)
        val names = watched.currentMethodNames(virtualFile) ?: return false.also { watched.request(virtualFile) }
        val method = methodAt(names, editor.caretModel.offset) ?: return false
        text = NewspeakBundle.message(if (watched.hasEntry(method.name)) "devtool.watch.method.unwatch" else "devtool.watch.method.watch")
        return true
    }

    override fun invoke(project: Project, editor: Editor?, file: PsiFile?) {
        val virtualFile = file?.virtualFile ?: return
        if (editor == null) return
        val watched = WatchedMethods.getInstance(project)
        val method = watched.currentMethodNames(virtualFile)?.let { names -> methodAt(names, editor.caretModel.offset) } ?: return
        watched.toggle(method.name)
    }

    // The list is the workspace's, not the file's: nothing is written.
    override fun startInWriteAction(): Boolean = false
}
