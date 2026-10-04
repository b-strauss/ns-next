package io.github.bstrauss.newspeak.structure

import com.intellij.icons.AllIcons
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

/** A row of the structure view, from one symbol as the server sends it (`crates/ns-lsp/src/document_symbols.rs`). */
class NewspeakStructureElementTest {
    private fun symbol(name: String, kind: SymbolKind, detail: String?): DocumentSymbol {
        val range = Range(Position(0, 0), Position(0, name.length))
        return DocumentSymbol(name, kind, range, range, detail)
    }

    @Test fun aRowShowsTheSymbolsName() {
        assertEquals("at: index put: value", presentationOf(symbol("at: index put: value", SymbolKind.Method, "public")).name)
    }

    @Test fun eachKindShowsItsIcon() {
        val expected = mapOf(
            SymbolKind.Class to AllIcons.Nodes.Class,
            SymbolKind.Constructor to AllIcons.Nodes.Method,
            SymbolKind.Namespace to AllIcons.Nodes.CustomRegion,
            SymbolKind.Constant to AllIcons.Nodes.PropertyRead,
            SymbolKind.Property to AllIcons.Nodes.PropertyReadWrite,
            SymbolKind.Method to AllIcons.Nodes.Method,
            SymbolKind.Variable to AllIcons.Nodes.Variable,
        )
        for ((kind, icon) in expected) {
            assertSame(kind.name, icon, presentationOf(symbol("name", kind, null)).kindIcon)
        }
    }

    @Test fun theAccessWordShowsItsIcon() {
        assertSame(AllIcons.Nodes.C_public, presentationOf(symbol("Module", SymbolKind.Class, "public")).visibilityIcon)
        assertSame(AllIcons.Nodes.C_protected, presentationOf(symbol("count", SymbolKind.Property, "protected")).visibilityIcon)
        assertSame(AllIcons.Nodes.C_private, presentationOf(symbol("run", SymbolKind.Method, "private")).visibilityIcon)
    }

    @Test fun aLazySlotShowsLazyBesideItsName() {
        val presentation = presentationOf(symbol("cache", SymbolKind.Constant, "protected lazy"))
        assertEquals("lazy", presentation.locationString)
        assertSame(AllIcons.Nodes.C_protected, presentation.visibilityIcon)
        assertSame(AllIcons.Nodes.PropertyRead, presentation.kindIcon)
    }

    @Test fun aSlotThatIsNotLazyShowsNothingBesideItsName() {
        assertNull(presentationOf(symbol("count", SymbolKind.Property, "protected")).locationString)
    }

    @Test fun aSideAndATemporaryShowNoAccess() {
        val side = presentationOf(symbol("instance side", SymbolKind.Namespace, null))
        assertNull(side.visibilityIcon)
        assertNull(side.locationString)
        assertNull(presentationOf(symbol("old", SymbolKind.Variable, null)).visibilityIcon)
    }

    @Test fun aDetailWithoutAnAccessWordShowsNoAccessIcon() {
        assertNull(presentationOf(symbol("count", SymbolKind.Property, "readonly")).visibilityIcon)
        assertNull(presentationOf(symbol("count", SymbolKind.Property, "")).visibilityIcon)
        // The access word is the first word only.
        assertNull(presentationOf(symbol("count", SymbolKind.Property, "lazy public")).visibilityIcon)
    }

    @Test fun theRowIconIsTheKindsIconAloneWithoutAnAccess() {
        val side = presentationOf(symbol("class side", SymbolKind.Namespace, null))
        assertSame(AllIcons.Nodes.CustomRegion, rowIconOf(side))
    }
}
