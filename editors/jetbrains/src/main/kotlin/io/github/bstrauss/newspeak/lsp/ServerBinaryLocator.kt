package io.github.bstrauss.newspeak.lsp

import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermission

/** Where the language server was found, or why it was not. */
sealed interface ServerLocation {
    data class Found(val path: Path) : ServerLocation
    /** A path is configured, and nothing is there: reported, never silently replaced by the bundled server. */
    data class ConfiguredPathMissing(val path: String) : ServerLocation
    /** No server is configured and none is bundled for [platform]; null when the platform is not one the layout names. */
    data class NotBundledForPlatform(val platform: String?) : ServerLocation
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
    private val osName: String,
    private val osArch: String,
) {
    fun locate(): ServerLocation {
        val configured = configuredPath?.trim().orEmpty()
        if (configured.isNotEmpty()) {
            val path = Path.of(configured)
            return if (Files.isRegularFile(path)) ServerLocation.Found(path) else ServerLocation.ConfiguredPathMissing(configured)
        }
        val platform = platformDirectory(osName, osArch)
        val bundled = if (platform == null || pluginDirectory == null) {
            null
        } else {
            pluginDirectory.resolve("bin").resolve(platform).resolve(executableName(osName))
        }
        if (bundled == null || !Files.isRegularFile(bundled)) return ServerLocation.NotBundledForPlatform(platform)
        ensureExecutable(bundled)
        return ServerLocation.Found(bundled)
    }

    /** The zip format the plugin ships in does not carry the executable bit. */
    private fun ensureExecutable(path: Path) {
        if (osName.startsWith("Windows") || Files.isExecutable(path)) return
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

/** `windows-x64`, `macos-arm64`, ...: the directory under `bin/` for this machine, or null for one the layout does not name. */
fun platformDirectory(osName: String, osArch: String): String? {
    val system = when {
        osName.startsWith("Windows") -> "windows"
        osName.startsWith("Mac") -> "macos"
        osName.startsWith("Linux") -> "linux"
        else -> return null
    }
    val architecture = when (osArch.lowercase()) {
        "amd64", "x86_64" -> "x64"
        "aarch64", "arm64" -> "arm64"
        else -> return null
    }
    return "$system-$architecture"
}

private fun executableName(osName: String): String =
    if (osName.startsWith("Windows")) "ns-next-lsp.exe" else "ns-next-lsp"
