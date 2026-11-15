package io.github.bstrauss.newspeak.library

import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.util.io.FileUtil
import com.intellij.openapi.vfs.VfsUtilCore
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.xdebugger.XDebuggerManager
import com.intellij.xdebugger.XDebuggerUtil
import io.github.bstrauss.newspeak.debugger.NewspeakLineBreakpointProperties
import io.github.bstrauss.newspeak.debugger.NewspeakLineBreakpointType
import java.nio.file.Files
import java.nio.file.Path

/** The line breakpoints of an older library follow the library to the new fingerprint's files. */
class CoreLibraryBreakpointsTest : BasePlatformTestCase() {
    private lateinit var root: Path
    private lateinit var store: CoreLibraryStore

    override fun setUp() {
        super.setUp()
        root = Files.createTempDirectory("ns-next-core-library")
        store = CoreLibraryStore(root)
    }

    override fun tearDown() {
        try {
            val manager = XDebuggerManager.getInstance(project).breakpointManager
            WriteAction.run<Throwable> { manager.allBreakpoints.forEach(manager::removeBreakpoint) }
            CoreLibraryStore.deleteRecursively(root)
        } finally {
            super.tearDown()
        }
    }

    fun testABreakpointMovesToTheFileOfTheSamePathInTheNewFingerprint() {
        val old = library(OLD, "Kernel.ns", "platform/TimeFactory.ns", "Gone.ns")
        val new = library(NEW, "Kernel.ns", "platform/TimeFactory.ns")
        val manager = XDebuggerManager.getInstance(project).breakpointManager
        val type = XDebuggerUtil.getInstance().findBreakpointType(NewspeakLineBreakpointType::class.java)
        WriteAction.run<Throwable> {
            manager.addLineBreakpoint(type, urlOf(old.resolve("Kernel.ns")), 7, NewspeakLineBreakpointProperties().apply { passCount = "3" }).isEnabled = false
            manager.addLineBreakpoint(
                type,
                urlOf(old.resolve("platform/TimeFactory.ns")),
                2,
                NewspeakLineBreakpointProperties().apply { column = 4 },
            ).isTemporary = true
            manager.addLineBreakpoint(type, urlOf(old.resolve("Gone.ns")), 1, NewspeakLineBreakpointProperties())
            manager.addLineBreakpoint(type, urlOf(new.resolve("Kernel.ns")), 9, NewspeakLineBreakpointProperties())
        }

        moveLineBreakpoints(project, store, new)

        val breakpoints = manager.getBreakpoints(type).associateBy { breakpoint -> breakpoint.fileUrl to breakpoint.line }
        assertEquals(breakpoints.keys.toString(), 4, breakpoints.size)
        val kernel = checkNotNull(breakpoints[urlOf(new.resolve("Kernel.ns")) to 7]) { "Kernel.ns's moved: ${breakpoints.keys}" }
        assertFalse("still disabled", kernel.isEnabled)
        assertEquals("3", kernel.properties?.passCount)
        val time = checkNotNull(breakpoints[urlOf(new.resolve("platform/TimeFactory.ns")) to 2]) { "TimeFactory.ns's moved: ${breakpoints.keys}" }
        assertEquals(4, time.properties?.column)
        assertTrue("still temporary", time.isTemporary)
        assertNotNull("a module without a counterpart stays", breakpoints[urlOf(old.resolve("Gone.ns")) to 1])
        assertNotNull("the new library's own stays", breakpoints[urlOf(new.resolve("Kernel.ns")) to 9])
    }

    private fun library(fingerprint: String, vararg files: String): Path {
        val directory = root.resolve(fingerprint)
        for (file in files) {
            val path = directory.resolve(file)
            Files.createDirectories(path.parent)
            Files.writeString(path, "class ${path.fileName.toString().removeSuffix(".ns")} = ()\n")
        }
        Files.writeString(directory.resolve(CoreLibraryStore.FINGERPRINT_FILE), "$fingerprint\n")
        return directory
    }

    private fun urlOf(path: Path): String = VfsUtilCore.pathToUrl(FileUtil.toSystemIndependentName(path.toString()))

    private companion object {
        const val OLD = "00000000000000a1"
        const val NEW = "00000000000000b2"
    }
}
