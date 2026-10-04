package io.github.bstrauss.newspeak.lsp

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.nio.file.Files
import java.nio.file.Path
import java.util.Properties

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
        val plugin = pluginWithServer("windows-x64", "ns-next-analyzer.exe")
        val configured = temporary.newFile("my-ns-next-analyzer.exe").toPath()
        val located = ServerBinaryLocator(configured.toString(), plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.Found(configured), located)
    }

    @Test fun aConfiguredPathThatDoesNotExistIsReportedNotSkipped() {
        val plugin = pluginWithServer("windows-x64", "ns-next-analyzer.exe")
        val missing = temporary.root.toPath().resolve("absent.exe").toString()
        val located = ServerBinaryLocator(missing, plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.ConfiguredPathMissing(missing), located)
    }

    @Test fun aBlankConfiguredPathFallsBackToTheBundledServer() {
        val plugin = pluginWithServer("windows-x64", "ns-next-analyzer.exe")
        val located = ServerBinaryLocator("  ", plugin, "Windows 11", "amd64").locate()
        assertEquals(ServerLocation.Found(plugin.resolve("bin/windows-x64/ns-next-analyzer.exe")), located)
    }

    @Test fun theBundledServerIsPerPlatform() {
        val plugin = pluginWithServer("linux-arm64", "ns-next-analyzer")
        assertEquals(
            ServerLocation.Found(plugin.resolve("bin/linux-arm64/ns-next-analyzer")),
            ServerBinaryLocator(null, plugin, "Linux", "aarch64").locate(),
        )
    }

    @Test fun noServerForThisPlatform() {
        val plugin = pluginWithServer("windows-x64", "ns-next-analyzer.exe")
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

    /**
     * Every entry of `platform-directories.properties` maps as the file's
     * stated rule says -- the rule `build.gradle.kts`'s `hostPlatform` follows
     * too: a `system.` key by what `os.name` starts with, an `architecture.`
     * key by `os.arch` in any case.
     */
    @Test fun everyEntryOfTheMappingFollowsItsStatedRule() {
        val mapping = Properties().apply {
            ServerBinaryLocator::class.java.getResourceAsStream("platform-directories.properties")!!.use(::load)
        }
        fun entries(prefix: String) = mapping.stringPropertyNames()
            .filter { key -> key.startsWith(prefix) }
            .associate { key -> key.removePrefix(prefix) to mapping.getProperty(key) }
        val systems = entries("system.")
        val architectures = entries("architecture.")
        assertEquals(3, systems.size)
        assertEquals(4, architectures.size)
        // The lookup takes whichever matching key it meets first, which is
        // only deterministic while at most one system prefix can match.
        for (prefix in systems.keys) {
            for (other in systems.keys - prefix) {
                assertFalse("$prefix starts with $other", prefix.startsWith(other))
            }
        }
        for ((namePrefix, system) in systems) {
            for ((architectureName, architecture) in architectures) {
                assertEquals("$system-$architecture", platformDirectory("$namePrefix 11.2", architectureName.uppercase()))
            }
        }
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
