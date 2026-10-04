package io.github.bstrauss.newspeak.lsp

import com.intellij.testFramework.fixtures.BasePlatformTestCase

/**
 * The structure view starts the client through the registered provider, the
 * one instance the platform handed the plugin's descriptor. Starting it needs
 * a server binary and a running IDE, so only the lookup is tested here.
 */
class NewspeakLspIntegrationProviderTest : BasePlatformTestCase() {
    fun testTheStructureViewFindsTheRegisteredProvider() {
        assertNotNull(NewspeakLspIntegrationProvider.registered())
    }
}
