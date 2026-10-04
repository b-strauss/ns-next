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
        val runtime = Files.createFile(createTempDirectory("ns-next-runtime").resolve("ns-next"))
        assertEquals(RuntimeLocation.Found(runtime), locateRuntime(runtime.toString()))
    }
}
