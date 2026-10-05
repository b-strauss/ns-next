package io.github.bstrauss.newspeak.run

import java.nio.file.Files
import kotlin.io.path.createTempDirectory
import org.junit.Assert.assertEquals
import org.junit.Test

class RuntimeLocationTest {
    @Test
    fun anEmptyPathIsNotConfigured() {
        assertEquals(RuntimeLocation.NotConfigured, locateRuntime(null))
        assertEquals(RuntimeLocation.NotConfigured, locateRuntime("  "))
    }

    @Test
    fun aPathNamingNoFileIsReportedAsWritten() {
        assertEquals(RuntimeLocation.ConfiguredPathMissing("/no/such/ns-next"), locateRuntime("/no/such/ns-next"))
    }

    @Test
    fun anExistingFileIsFound() {
        val directory = createTempDirectory("ns-next-runtime")
        try {
            val runtime = Files.createFile(directory.resolve("ns-next"))
            assertEquals(RuntimeLocation.Found(runtime), locateRuntime(runtime.toString()))
        } finally {
            directory.toFile().deleteRecursively()
        }
    }
}
