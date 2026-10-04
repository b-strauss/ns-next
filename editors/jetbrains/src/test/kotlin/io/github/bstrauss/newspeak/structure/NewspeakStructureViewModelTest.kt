package io.github.bstrauss.newspeak.structure

import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

/** Which answers of the server the model shows, and which make it ask again. */
class NewspeakStructureViewModelTest {
    @Test fun noAnswerMakesTheModelAskAgain() {
        assertNull(usableAnswer(null))
    }

    @Test fun anEmptyAnswerMakesTheModelAskAgain() {
        // Every document the server has opened or read from disk answers at
        // least its root class, so an empty answer is a `didOpen` that has not
        // reached it yet, or a file it could not read.
        assertNull(usableAnswer(emptyList()))
    }

    @Test fun anAnswerWithSymbolsIsShown() {
        val range = Range(Position(0, 0), Position(0, 6))
        val symbols = listOf(DocumentSymbol("Module", SymbolKind.Class, range, range, "public"))
        assertSame(symbols, usableAnswer(symbols))
    }
}
