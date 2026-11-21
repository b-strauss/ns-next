package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.service
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import io.github.bstrauss.newspeak.devtool.editor.bodySourcePath
import io.github.bstrauss.newspeak.devtool.model.Body
import io.github.bstrauss.newspeak.devtool.model.SourceRange
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import io.github.bstrauss.newspeak.library.NewspeakCoreLibrary
import java.nio.file.Path

/** The range [at] names in [body]: the call of the send at that bytecode index, else the body's own. */
fun rangeOf(body: Body, at: Int?): SourceRange? = at?.let { index -> body.sends.firstOrNull { send -> send.at == index }?.call } ?: body.range

/**
 * Opens [body]'s source at the call of its send [at], or at the body's own
 * range without one. A body number without a description, one without a
 * range and one whose file is gone navigate nowhere, since the range of
 * a body is the runtime's, never guessed; the file is found by
 * [withSourceFile].
 */
fun navigate(
    project: Project,
    run: TraceRun,
    body: Int,
    at: Int?,
    libraryDirectory: Path? = project.service<NewspeakCoreLibrary>().directory,
) {
    val described = run.bodies[body] ?: return
    val range = rangeOf(described, at) ?: return
    // A library body's file lies under the directory the project shows the shipped sources from.
    val path = bodySourcePath(described, libraryDirectory) ?: return
    withSourceFile(project, path) { file -> OpenFileDescriptor(project, file, range.startLine, range.startColumn).navigate(true) }
}

/**
 * Calls [then] on the event dispatch thread with the file at [path]: at
 * once for one the virtual file system holds, which is looked up without
 * a refresh, since a refresh would block the thread on the disk; a file it
 * does not hold yet is refreshed on a pooled thread and handed over once
 * found, and one that is gone never.
 */
fun withSourceFile(project: Project, path: Path, then: (VirtualFile) -> Unit) {
    val fileSystem = LocalFileSystem.getInstance()
    val known = fileSystem.findFileByNioFile(path)
    if (known != null) {
        then(known)
        return
    }
    val application = ApplicationManager.getApplication()
    application.executeOnPooledThread {
        val found = fileSystem.refreshAndFindFileByNioFile(path) ?: return@executeOnPooledThread
        application.invokeLater({ then(found) }, project.disposed)
    }
}

/**
 * [body]'s name as the text trace writes it between braces: a family root
 * the file did not describe, since its code was gone, by its blocks'
 * names; `#<number>` for another body the file did not describe.
 */
fun bodyName(run: TraceRun, body: Int): String = run.bodies[body]?.name ?: run.undescribedRootName(body) ?: "#$body"

/**
 * The method [body] belongs to: a block's root, from its description's
 * family, whether or not the file described the root -- a block of a
 * root whose code was gone is counted as its method's too. A body the
 * file did not describe stands for itself.
 */
fun rootOf(run: TraceRun, body: Int): Int = run.bodies[body]?.family ?: body
