package io.github.bstrauss.newspeak.library

import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.io.FileUtil
import com.intellij.openapi.vfs.VfsUtilCore
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.XDebuggerUtil
import io.github.bstrauss.newspeak.debugger.NewspeakLineBreakpointProperties
import io.github.bstrauss.newspeak.debugger.NewspeakLineBreakpointType
import java.nio.file.InvalidPathException
import java.nio.file.Path
import kotlin.io.path.isRegularFile

private val LOG = Logger.getInstance("#io.github.bstrauss.newspeak.library.CoreLibraryBreakpoints")

/**
 * Moves [project]'s line breakpoints standing in a file of an older
 * directory of [store] to the file of the same path in [directory], the
 * library the project shows now, at the same line and with the same
 * settings: the user put them on the binary's module, which the new
 * directory holds. A breakpoint's description, its group and its links
 * to dependent breakpoints are not carried over: the platform reads and
 * writes them only through internal API (`XBreakpointBase`,
 * `XDependentBreakpointManager`), which the plugin does not use. Whether the line can still take one is the agent's and
 * the language server's to say, as for any breakpoint. One whose file has
 * no counterpart -- a module renamed or removed -- is left where it is
 * and logged. Called on the event dispatch thread.
 */
fun moveLineBreakpoints(project: Project, store: CoreLibraryStore, directory: Path) {
    val fingerprint = directory.fileName.toString()
    val manager = XDebuggerManager.getInstance(project).breakpointManager
    val type = XDebuggerUtil.getInstance().findBreakpointType(NewspeakLineBreakpointType::class.java)
    for (breakpoint in manager.getBreakpoints(type).toList()) {
        if (!breakpoint.fileUrl.startsWith("file:")) continue
        val path = try {
            Path.of(VfsUtilCore.urlToPath(breakpoint.fileUrl))
        } catch (_: InvalidPathException) {
            continue
        }
        val place = store.placeOf(path) ?: continue
        if (place.fingerprint == fingerprint) continue
        val counterpart = directory.resolve(place.relative.toString())
        if (!counterpart.isRegularFile()) {
            LOG.warn("the line breakpoint at ${breakpoint.fileUrl}:${breakpoint.line + 1} stays: the shipped sources at $directory hold no ${place.relative}")
            continue
        }
        val url = VfsUtilCore.pathToUrl(FileUtil.toSystemIndependentName(counterpart.toString()))
        val properties = NewspeakLineBreakpointProperties().apply {
            passCount = breakpoint.properties?.passCount
            column = breakpoint.properties?.column
        }
        WriteAction.run<RuntimeException> {
            val already = manager.getBreakpoints(type).any { other ->
                other.fileUrl == url && other.line == breakpoint.line && other.properties?.column == properties.column
            }
            if (!already) {
                manager.addLineBreakpoint(type, url, breakpoint.line, properties).apply {
                    isTemporary = breakpoint.isTemporary
                    isEnabled = breakpoint.isEnabled
                    suspendPolicy = breakpoint.suspendPolicy
                    conditionExpression = breakpoint.conditionExpression
                    logExpressionObject = breakpoint.logExpressionObject
                    isLogMessage = breakpoint.isLogMessage
                    isLogStack = breakpoint.isLogStack
                }
            }
            manager.removeBreakpoint(breakpoint)
        }
    }
}
