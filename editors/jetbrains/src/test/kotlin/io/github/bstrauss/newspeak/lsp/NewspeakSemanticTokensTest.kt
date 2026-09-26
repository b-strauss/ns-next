package io.github.bstrauss.newspeak.lsp

import io.github.bstrauss.newspeak.highlighting.NewspeakColors
import org.junit.Assert.assertEquals
import org.junit.Test

/** The server's legend (`crates/ns-lsp/src/semantic_tokens.rs`, `legend`) onto the plugin's keys. */
class NewspeakSemanticTokensTest {
    @Test fun everyTokenTypeOfTheLegendIsMapped() {
        assertEquals(NewspeakColors.PARAMETER, semanticAttributesFor("parameter", listOf("declaration")))
        assertEquals(NewspeakColors.LOCAL_VARIABLE, semanticAttributesFor("variable", emptyList()))
        assertEquals(NewspeakColors.SLOT, semanticAttributesFor("property", emptyList()))
        assertEquals(NewspeakColors.IMMUTABLE_SLOT, semanticAttributesFor("property", listOf("readonly")))
        assertEquals(NewspeakColors.CLASS_NAME, semanticAttributesFor("class", emptyList()))
        assertEquals(NewspeakColors.METHOD_DECLARATION, semanticAttributesFor("method", listOf("declaration")))
        assertEquals(NewspeakColors.SEND, semanticAttributesFor("method", emptyList()))
    }

    @Test fun anUnknownTypeKeepsTheLexersColour() {
        assertEquals(null, semanticAttributesFor("macro", emptyList()))
    }
}
