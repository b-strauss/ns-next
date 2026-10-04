package io.github.bstrauss.newspeak.structure

import com.intellij.icons.AllIcons
import io.github.bstrauss.newspeak.NewspeakIcons
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import javax.swing.Icon

/** A row of the structure view, from one symbol as the server sends it (`crates/ns-analyzer/src/document_symbols.rs`). */
class NewspeakStructureElementTest {
    private fun symbol(name: String, kind: SymbolKind, detail: String?): DocumentSymbol {
        val range = Range(Position(0, 0), Position(0, name.length))
        return DocumentSymbol(name, kind, range, range, detail)
    }

    @Test fun aRowShowsTheSymbolsName() {
        assertEquals("at: index put: value", presentationOf(symbol("at: index put: value", SymbolKind.Method, "public")).name)
    }

    /** One row per element the tree shows, and a value class: its base icon and its hover text. */
    @Test fun eachElementShowsItsIconAndHover() {
        class Case(val symbol: DocumentSymbol, val root: Boolean, val icon: Icon, val tooltip: String)
        val cases = listOf(
            Case(symbol("Module", SymbolKind.Class, "public"), true, NewspeakIcons.ROOT_CLASS, "Root class"),
            Case(symbol("Kernel", SymbolKind.Class, "public value"), true, NewspeakIcons.ROOT_CLASS, "Root value class"),
            Case(symbol("Inner", SymbolKind.Class, "public"), false, AllIcons.Nodes.Class, "Public class"),
            Case(symbol("Point", SymbolKind.Class, "protected value"), false, NewspeakIcons.VALUE_CLASS, "Protected value class"),
            Case(symbol("usingPlatform: platform", SymbolKind.Constructor, "public"), false, AllIcons.Nodes.Function, "Factory method"),
            Case(symbol("count", SymbolKind.Constant, "private"), false, AllIcons.Nodes.Constant, "Private constant"),
            Case(symbol("count", SymbolKind.Property, "protected"), false, AllIcons.Nodes.Variable, "Protected variable"),
            Case(symbol("old", SymbolKind.Constant, null), false, AllIcons.Nodes.Constant, "Local constant"),
            Case(symbol("old", SymbolKind.Variable, null), false, AllIcons.Nodes.Variable, "Local variable"),
            Case(symbol("run", SymbolKind.Method, "protected"), false, AllIcons.Nodes.Method, "Protected method"),
            Case(symbol("make", SymbolKind.Method, "public class"), false, AllIcons.Nodes.Method, "Public class method"),
        )
        for (case in cases) {
            val presentation = presentationOf(case.symbol, case.root)
            assertSame(case.tooltip, case.icon, presentation.kindIcon)
            assertEquals(case.tooltip, presentation.tooltip)
        }
    }

    @Test fun aLazySlotsHoverIsItsSlotKind() {
        assertEquals("Protected constant", presentationOf(symbol("cache", SymbolKind.Constant, "protected lazy")).tooltip)
        assertEquals("Public variable", presentationOf(symbol("cache", SymbolKind.Property, "public lazy")).tooltip)
    }

    @Test fun onlyTheRootIsTheRootClass() {
        // A nested class is never the root, whatever its access.
        assertSame(AllIcons.Nodes.Class, presentationOf(symbol("Inner", SymbolKind.Class, "public")).kindIcon)
        assertEquals("Protected class", presentationOf(symbol("Inner", SymbolKind.Class, "protected")).tooltip)
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
        assertSame(AllIcons.Nodes.Constant, presentation.kindIcon)
    }

    @Test fun aSlotThatIsNotLazyShowsNothingBesideItsName() {
        assertNull(presentationOf(symbol("count", SymbolKind.Property, "protected")).locationString)
    }

    @Test fun aTemporaryShowsNoAccess() {
        val temporary = presentationOf(symbol("old", SymbolKind.Variable, null))
        assertNull(temporary.visibilityIcon)
        assertNull(temporary.locationString)
        assertFalse(temporary.classSide)
    }

    @Test fun aClassSideMethodIsToldByTheWordClass() {
        val presentation = presentationOf(symbol("make", SymbolKind.Method, "public class"))
        assertTrue(presentation.classSide)
        assertSame(AllIcons.Nodes.C_public, presentation.visibilityIcon)
        assertSame(AllIcons.Nodes.Method, presentation.kindIcon)
        assertNull(presentation.locationString)
    }

    @Test fun anInstanceSideMemberIsNotClassSide() {
        assertFalse(presentationOf(symbol("run", SymbolKind.Method, "protected")).classSide)
        assertFalse(presentationOf(symbol("cache", SymbolKind.Constant, "protected lazy")).classSide)
        // `class` is a word of `detail`, not a part of one.
        assertFalse(presentationOf(symbol("run", SymbolKind.Method, "protected classy")).classSide)
    }

    @Test fun theWordsAreReadTogether() {
        val presentation = presentationOf(symbol("cache", SymbolKind.Constant, "private class lazy"))
        assertTrue(presentation.classSide)
        assertEquals("lazy", presentation.locationString)
        assertSame(AllIcons.Nodes.C_private, presentation.visibilityIcon)
    }

    @Test fun aDetailWithoutAnAccessWordShowsNoAccessIcon() {
        assertNull(presentationOf(symbol("count", SymbolKind.Property, "readonly")).visibilityIcon)
        assertNull(presentationOf(symbol("count", SymbolKind.Property, "")).visibilityIcon)
        // The access word is the first word only.
        assertNull(presentationOf(symbol("count", SymbolKind.Property, "lazy public")).visibilityIcon)
    }

    @Test fun theRowIconIsTheKindsIconAloneWithoutAnAccess() {
        val temporary = presentationOf(symbol("old", SymbolKind.Variable, null))
        assertSame(AllIcons.Nodes.Variable, rowIconOf(temporary))
    }
}
