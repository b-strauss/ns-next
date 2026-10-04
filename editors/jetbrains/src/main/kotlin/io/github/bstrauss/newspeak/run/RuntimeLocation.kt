package io.github.bstrauss.newspeak.run

import java.nio.file.Files
import java.nio.file.Path

/** Where the ns-next runtime is, or why it cannot be started. */
sealed interface RuntimeLocation {
    data class Found(val path: Path) : RuntimeLocation

    /** The setting is empty: the plugin bundles no runtime to fall back on. */
    data object NotConfigured : RuntimeLocation

    /** A path is set and nothing is there; kept as the user wrote it, for the message. */
    data class ConfiguredPathMissing(val path: String) : RuntimeLocation
}

/** Pure over the configured path, so the rule is testable without an IDE. */
fun locateRuntime(configuredPath: String?): RuntimeLocation {
    val configured = configuredPath?.trim().orEmpty()
    if (configured.isEmpty()) return RuntimeLocation.NotConfigured
    val path = Path.of(configured)
    return if (Files.isRegularFile(path)) RuntimeLocation.Found(path) else RuntimeLocation.ConfiguredPathMissing(configured)
}
