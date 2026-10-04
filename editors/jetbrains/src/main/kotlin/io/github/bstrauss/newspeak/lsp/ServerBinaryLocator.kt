package io.github.bstrauss.newspeak.lsp

import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermission
import java.util.Properties

/** Where the language server was found, or why it was not. */
sealed interface ServerLocation {
    data class Found(val path: Path) : ServerLocation
    /** A path is configured, and nothing is there: reported, never silently replaced by the bundled server. */
    data class ConfiguredPathMissing(val path: String) : ServerLocation
    /** No server is configured and none is bundled for [platform]; null when the platform is not one the layout names. */
    data class NotBundledForPlatform(val platform: String?) : ServerLocation
    /** The plugin could not determine its own installation directory, so it has nowhere to look for a bundled server. */
    data object PluginDirectoryUnknown : ServerLocation
}

/**
 * The server to start, in the order `docs/design/editor-support.md` gives:
 * the configured path when one is set, else the one bundled for this platform
 * under `<plugin>/bin/<os>-<arch>/`. Pure over its inputs, so the order is
 * testable without an IDE.
 */
class ServerBinaryLocator(
    private val configuredPath: String?,
    private val pluginDirectory: Path?,
    private val operatingSystemName: String,
    private val operatingSystemArchitecture: String,
) {
    fun locate(): ServerLocation {
        val configured = configuredPath?.trim().orEmpty()
        if (configured.isNotEmpty()) {
            val path = Path.of(configured)
            return if (Files.isRegularFile(path)) ServerLocation.Found(path) else ServerLocation.ConfiguredPathMissing(configured)
        }
        if (pluginDirectory == null) return ServerLocation.PluginDirectoryUnknown
        val platform = platformDirectory(operatingSystemName, operatingSystemArchitecture)
        val bundled = platform?.let { platformName ->
            pluginDirectory.resolve("bin").resolve(platformName).resolve(executableName(operatingSystemName))
        }
        if (bundled == null || !Files.isRegularFile(bundled)) return ServerLocation.NotBundledForPlatform(platform)
        ensureExecutable(bundled)
        return ServerLocation.Found(bundled)
    }

    /** The zip format the plugin ships in does not carry the executable bit. */
    private fun ensureExecutable(path: Path) {
        if (isWindows(operatingSystemName) || Files.isExecutable(path)) return
        try {
            val permissions = Files.getPosixFilePermissions(path) +
                setOf(PosixFilePermission.OWNER_EXECUTE, PosixFilePermission.GROUP_EXECUTE, PosixFilePermission.OTHERS_EXECUTE)
            Files.setPosixFilePermissions(path, permissions)
        } catch (exception: UnsupportedOperationException) {
            // Not a POSIX file system: nothing to set.
        } catch (exception: java.io.IOException) {
            // Starting it will fail and report why; there is nothing better to do here.
        }
    }
}

/** Whether `operatingSystemName` (`System.getProperty("os.name")`) names a Windows system. */
private fun isWindows(operatingSystemName: String): Boolean = operatingSystemName.startsWith("Windows")

/**
 * `platform-directories.properties`, beside this class: the mapping
 * `build.gradle.kts` reads too, so the directory the build bundles the server
 * under is the one this lookup finds it in. The lookup rule is the one that
 * file's header states (a `system.` key matches the start of `os.name`, an
 * `architecture.` key the lower-cased `os.arch`); `hostPlatform` in the build
 * script follows it too, since a test cannot call build-script code.
 */
private object PlatformDirectories {
    private val properties = Properties().apply {
        val resource = checkNotNull(PlatformDirectories::class.java.getResourceAsStream("platform-directories.properties")) {
            "platform-directories.properties is missing from the plugin"
        }
        resource.use(::load)
    }

    /** What `os.name` starts with, to the system part of the directory. */
    val systems: Map<String, String> = entries("system.")

    /** `os.arch` in lower case, to the architecture part of the directory. */
    val architectures: Map<String, String> = entries("architecture.")

    private fun entries(prefix: String): Map<String, String> = properties.stringPropertyNames()
        .filter { key -> key.startsWith(prefix) }
        .associate { key -> key.removePrefix(prefix) to properties.getProperty(key) }
}

/** `windows-x64`, `macos-arm64`, ...: the directory under `bin/` for this machine, or null for one the layout does not name. */
fun platformDirectory(operatingSystemName: String, operatingSystemArchitecture: String): String? {
    val system = PlatformDirectories.systems.entries
        .firstOrNull { (namePrefix, _) -> operatingSystemName.startsWith(namePrefix) }
        ?.value ?: return null
    val architecture = PlatformDirectories.architectures[operatingSystemArchitecture.lowercase()] ?: return null
    return "$system-$architecture"
}

private fun executableName(operatingSystemName: String): String =
    if (isWindows(operatingSystemName)) "ns-next-analyzer.exe" else "ns-next-analyzer"
