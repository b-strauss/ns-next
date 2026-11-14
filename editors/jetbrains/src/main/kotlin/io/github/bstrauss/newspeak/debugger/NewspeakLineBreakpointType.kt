package io.github.bstrauss.newspeak.debugger

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.xdebugger.breakpoints.XBreakpointProperties
import com.intellij.xdebugger.breakpoints.XLineBreakpointType
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakFileType

/**
 * A line breakpoint in a program's `.ns` file, which the agent sets at the
 * first statement starting on its line or after it.
 */
class NewspeakLineBreakpointType :
    XLineBreakpointType<XBreakpointProperties<*>>(ID, NewspeakBundle.message("debugger.breakpoint.line.title")) {
    /** A file on disk: the agent names a program's module by its file's path. */
    override fun canPutAt(file: VirtualFile, line: Int, project: Project): Boolean =
        file.fileType == NewspeakFileType && file.isInLocalFileSystem

    override fun createBreakpointProperties(file: VirtualFile, line: Int): XBreakpointProperties<*>? = null

    companion object {
        const val ID = "newspeak-line"
    }
}
