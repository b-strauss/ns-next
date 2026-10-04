package io.github.bstrauss.newspeak.lsp

import com.intellij.psi.PsiFile
import io.github.bstrauss.newspeak.highlighting.NewspeakColors
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.lang.reflect.Proxy

/** The server's legend (`crates/ns-analyzer/src/semantic_tokens.rs`, `legend`) onto the plugin's keys. */
class NewspeakSemanticTokensTest {
    @Test fun everyTokenTypeOfTheLegendIsMapped() {
        assertEquals(NewspeakColors.PARAMETER, semanticAttributesFor("parameter", listOf("declaration")))
        assertEquals(NewspeakColors.LOCAL_VARIABLE, semanticAttributesFor("variable", emptyList()))
        assertEquals(NewspeakColors.SLOT, semanticAttributesFor("property", emptyList()))
        assertEquals(NewspeakColors.IMMUTABLE_SLOT, semanticAttributesFor("property", listOf("readonly")))
        assertEquals(NewspeakColors.CLASS_NAME, semanticAttributesFor("class", emptyList()))
        assertEquals(NewspeakColors.METHOD_DECLARATION, semanticAttributesFor("method", listOf("declaration")))
        assertEquals(NewspeakColors.SEND, semanticAttributesFor("method", emptyList()))
        assertEquals(NewspeakColors.KEYWORD, semanticAttributesFor("keyword", emptyList()))
    }

    @Test fun anUnknownTypeKeepsTheLexersColour() {
        assertEquals(null, semanticAttributesFor("macro", emptyList()))
    }

    @Test fun theServerIsAskedForTokensForANewspeakFile() {
        // The platform's default asks only for plain-text and TextMate files,
        // so without the override no request is ever sent. The file is never
        // read, which a proxy that answers nothing makes sure of.
        val file = Proxy.newProxyInstance(PsiFile::class.java.classLoader, arrayOf(PsiFile::class.java)) { _, method, _ ->
            throw AssertionError("the decision must not depend on the file, but ${method.name} was called")
        } as PsiFile
        assertTrue(NewspeakSemanticTokens.shouldAskServerForSemanticTokens(file))
    }
}
