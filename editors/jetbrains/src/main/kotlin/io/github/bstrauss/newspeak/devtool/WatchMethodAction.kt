package io.github.bstrauss.newspeak.devtool

import com.intellij.codeInsight.hint.HintManager
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.ex.EditorGutterComponentEx
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.vfs.VirtualFile
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakFileType

/**
 * Watch or Unwatch the method at the caret, in the editor's context menu,
 * or on the line clicked, in the gutter's: the method whose declaration
 * holds the place, named by the language server. A method is watched by
 * its exact name; one covered only by a pattern is watched by its own.
 */
class WatchMethodAction :
    AnAction(),
    DumbAware {
    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    override fun update(event: AnActionEvent) {
        val project = event.project
        val file = event.getData(CommonDataKeys.VIRTUAL_FILE)
        val editor = event.getData(CommonDataKeys.EDITOR)
        if (project == null || editor == null || file == null || !isNewspeakSource(file)) {
            event.presentation.isEnabledAndVisible = false
            return
        }
        val watched = WatchedMethods.getInstance(project)
        // The update never waits for the server: without names it asks for them and offers the action meanwhile.
        val names = watched.currentMethodNames(file) ?: null.also { watched.request(file) }
        val method = names?.let { methodAtPlace(event, editor, it) }
        event.presentation.isEnabledAndVisible = names == null || method != null
        event.presentation.text = NewspeakBundle.message(
            if (method != null && watched.hasEntry(method.name)) "devtool.watch.method.action.unwatch" else "devtool.watch.method.action.watch",
        )
    }

    override fun actionPerformed(event: AnActionEvent) {
        val project = event.project ?: return
        val file = event.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        val editor = event.getData(CommonDataKeys.EDITOR) ?: return
        val line = event.getData(EditorGutterComponentEx.LOGICAL_LINE_AT_CURSOR)
        val offset = editor.caretModel.offset
        val watched = WatchedMethods.getInstance(project)
        // The names may have to be asked for, which waits on the server and reads the project file.
        ApplicationManager.getApplication().executeOnPooledThread {
            val names = watched.methodNames(file)
            ApplicationManager.getApplication().invokeLater({
                val method = if (line != null) methodOnLine(names, editor.document, line) else methodAt(names, offset)
                if (method == null) {
                    HintManager.getInstance().showErrorHint(editor, NewspeakBundle.message("devtool.watch.method.none"))
                } else {
                    watched.toggle(method.name)
                }
            }, project.disposed)
        }
    }

    private fun methodAtPlace(event: AnActionEvent, editor: Editor, names: List<MethodName>): MethodName? {
        val line = event.getData(EditorGutterComponentEx.LOGICAL_LINE_AT_CURSOR)
        return if (line != null) methodOnLine(names, editor.document, line) else methodAt(names, editor.caretModel.offset)
    }
}

/** Whether [file] is a Newspeak source, whose methods the server names. */
internal fun isNewspeakSource(file: VirtualFile?): Boolean = file != null && file.fileType == NewspeakFileType
