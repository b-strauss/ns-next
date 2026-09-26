package io.github.bstrauss.newspeak.lsp

import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.nio.file.Files
import java.nio.file.Path

class ServerBinaryLocatorTest {
    @get:Rule val temporary = TemporaryFolder()

    private fun pluginWithServer(platform: String, executable: String): Path {
        val plugin = temporary.newFolder("plugin").toPath()
        val binary = plugin.resolve("bin").resolve(platform).resolve(executable)
        Files.createDirectories(binary.parent)
        Files.writeString(binary, "")
        return plugin
    }

    @Test fun theConfiguredPathWins() {
        val plugin = pluginWithServer("windows-x64", "ns-next-lsp.exe")
        val configured = temporary.newFile("my-ns-next-lsp.exe").toPath()
        val located = ServerBinaryLocator(configured.toString(), plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.Found(configured), located)
    }

    @Test fun aConfiguredPathThatDoesNotExistIsReportedNotSkipped() {
        val plugin = pluginWithServer("windows-x64", "ns-next-lsp.exe")
        val missing = temporary.root.toPath().resolve("absent.exe").toString()
        val located = ServerBinaryLocator(missing, plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.ConfiguredPathMissing(missing), located)
    }

    @Test fun aBlankConfiguredPathFallsBackToTheBundledServer() {
        val plugin = pluginWithServer("windows-x64", "ns-next-lsp.exe")
        val located = ServerBinaryLocator("  ", plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.Found(plugin.resolve("bin/windows-x64/ns-next-lsp.exe")), located)
    }

    @Test fun theBundledServerIsPerPlatform() {
        val plugin = pluginWithServer("linux-arm64", "ns-next-lsp")
        assertEquals(
            ServerLocation.Found(plugin.resolve("bin/linux-arm64/ns-next-lsp")),
            ServerBinaryLocator(null, plugin, "Linux", "aarch64").locate(),
        )
    }

    @Test fun noServerForThisPlatform() {
        val plugin = pluginWithServer("windows-x64", "ns-next-lsp.exe")
        assertEquals(
            ServerLocation.NotBundledForPlatform("macos-arm64"),
            ServerBinaryLocator(null, plugin, "Mac OS X", "aarch64").locate(),
        )
    }

    @Test fun anUnknownPluginDirectoryIsReportedDistinctlyFromAnUnbundledPlatform() {
        // Windows is bundled for, so a `NotBundledForPlatform` balloon here
        // would be simply wrong; the plugin failing to locate its own
        // directory is a different problem with a different fix (the
        // setting, not a `cargo build`).
        assertEquals(
            ServerLocation.PluginDirectoryUnknown,
            ServerBinaryLocator(null, null, "Windows 11", "amd64").locate(),
        )
    }

    @Test fun anUnknownPlatformHasNoDirectory() {
        assertEquals(null, platformDirectory("Plan 9", "mips"))
        assertEquals(
            ServerLocation.NotBundledForPlatform(null),
            ServerBinaryLocator(null, temporary.root.toPath(), "Plan 9", "mips").locate(),
        )
    }

    @Test fun platformDirectories() {
        assertEquals("windows-x64", platformDirectory("Windows 11", "amd64"))
        assertEquals("windows-arm64", platformDirectory("Windows 11", "aarch64"))
        assertEquals("macos-x64", platformDirectory("Mac OS X", "x86_64"))
        assertEquals("macos-arm64", platformDirectory("Mac OS X", "aarch64"))
        assertEquals("linux-x64", platformDirectory("Linux", "amd64"))
        assertEquals("linux-arm64", platformDirectory("Linux", "arm64"))
    }
}
