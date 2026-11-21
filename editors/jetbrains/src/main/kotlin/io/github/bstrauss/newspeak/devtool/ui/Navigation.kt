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
 * a body is the runtime's, never guessed. The file is looked up without a
 * refresh, which would block the event dispatch thread on the disk; one
 * the virtual file system does not hold yet is refreshed on a pooled
 * thread and opened once it is found.
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
    val open = { file: VirtualFile -> OpenFileDescriptor(project, file, range.startLine, range.startColumn).navigate(true) }
    val fileSystem = LocalFileSystem.getInstance()
    val known = fileSystem.findFileByNioFile(path)
    if (known != null) {
        open(known)
        return
    }
    val application = ApplicationManager.getApplication()
    application.executeOnPooledThread {
        val found = fileSystem.refreshAndFindFileByNioFile(path) ?: return@executeOnPooledThread
        application.invokeLater({ open(found) }, project.disposed)
    }
}

/** [body]'s name as the text trace writes it between braces; `#<number>` for a body the file did not describe. */
fun bodyName(run: TraceRun, body: Int): String = run.bodies[body]?.name ?: "#$body"

/**
 * The method [body] belongs to: a block's root, from its description's
 * family. A body the file did not describe, and a block whose root it
 * did not describe either, stands for itself.
 */
fun rootOf(run: TraceRun, body: Int): Int {
    val family = run.bodies[body]?.family ?: return body
    return if (run.bodies.containsKey(family)) family else body
}
