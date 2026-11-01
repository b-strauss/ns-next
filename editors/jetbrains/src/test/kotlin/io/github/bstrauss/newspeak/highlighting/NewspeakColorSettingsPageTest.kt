package io.github.bstrauss.newspeak.highlighting

import com.intellij.psi.TokenType
import io.github.bstrauss.newspeak.lexer.lexAll
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NewspeakColorSettingsPageTest {
    private val page = NewspeakColorSettingsPage()

    @Test fun theDemoTextHasNoBadCharacter() {
        val text = page.getDemoText().replace(Regex("</?[a-z]+>"), "")
        assertTrue(lexAll(text).none { (type, _) -> type == TokenType.BAD_CHARACTER })
    }

    @Test fun everyTagInTheDemoTextIsKnown() {
        val tags = Regex("<([a-z]+)>").findAll(page.getDemoText()).map { match -> match.groupValues[1] }.toSet()
        assertEquals(page.getAdditionalHighlightingTagToDescriptorMap().keys, tags)
    }

    /** The page shows a doc comment's colour only where its highlighter merges a block. */
    @Test fun theDemoTextHasADocComment() {
        val text = page.getDemoText().replace(Regex("</?[a-z]+>"), "")
        val lexer = page.highlighter.highlightingLexer
        lexer.start(text, 0, text.length, 0)
        var docComments = 0
        while (true) {
            val type = lexer.tokenType ?: break
            if (NewspeakColors.DOC_COMMENT in page.highlighter.getTokenHighlights(type)) docComments += 1
            lexer.advance()
        }
        assertEquals(1, docComments)
    }

    @Test fun everyKeyHasADescriptor() {
        val described = page.getAttributeDescriptors().map { descriptor -> descriptor.key }.toSet()
        assertEquals(NewspeakColors.ALL.toSet(), described)
    }
}
