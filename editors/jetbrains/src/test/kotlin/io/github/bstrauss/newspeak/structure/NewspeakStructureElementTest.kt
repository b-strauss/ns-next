package io.github.bstrauss.newspeak.structure

import com.intellij.icons.AllIcons
import com.intellij.ide.projectView.PresentationData
import com.intellij.openapi.command.WriteCommandAction
import com.intellij.psi.PsiDocumentManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase
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

/** A row's hover with the summary of the doc comment above its symbol, read from the file. */
class NewspeakStructureElementSummaryTest : BasePlatformTestCase() {
    private val source = """
        class Module = () (
          /// Answers two. Always
          /// the same.
          two = ( ^2 )
          three = ( ^3 )
        )
    """.trimIndent()

    private fun method(name: String, line: Int): DocumentSymbol {
        val range = Range(Position(line, 2), Position(line, source.lines()[line].length))
        return DocumentSymbol(name, SymbolKind.Method, range, range, "public")
    }

    /** The hover of `symbol` over `text`, configured as `name`: one test configuring several texts names each file afresh. */
    private fun tooltipOf(symbol: DocumentSymbol, text: String = source, root: Boolean = false, name: String = "Module.ns"): String? {
        val file = myFixture.configureByText(name, text)
        val element = NewspeakStructureElement(project, file.virtualFile, symbol, root = root, psiFile = file)
        return (element.presentation as PresentationData).tooltip
    }

    fun testATooltipShowsTheSummary() {
        assertEquals("<html>Public method<br>Answers two.</html>", tooltipOf(method("two", 3)))
    }

    /** The summaries are kept per file and computed afresh when only a comment changed. */
    fun testTheSummaryFollowsAnEditOfItsComment() {
        assertEquals("<html>Public method<br>Answers two.</html>", tooltipOf(method("two", 3)))
        val file = myFixture.file
        WriteCommandAction.runWriteCommandAction(project) {
            val document = myFixture.editor.document
            val start = document.text.indexOf("Answers two.")
            document.replaceString(start, start + "Answers two.".length, "Answers 2.")
            PsiDocumentManager.getInstance(project).commitDocument(document)
        }
        val element = NewspeakStructureElement(project, file.virtualFile, method("two", 3), psiFile = file)
        assertEquals("<html>Public method<br>Answers 2.</html>", (element.presentation as PresentationData).tooltip)
    }

    fun testAMemberWithoutADocCommentKeepsItsTooltip() {
        assertEquals("Public method", tooltipOf(method("three", 4)))
    }

    /**
     * A module's `:module:` line, and any metadata comment, stands between
     * the doc comment and the class it documents, each on the line right
     * above the next, as the server's hover reads it
     * (`hover::doc_comment_above`); a blank line, a plain comment or code
     * after a metadata comment on its line ends the doc comment's reach.
     */
    fun testTheSummaryStepsOverMetadataAboveTheClass() {
        var configured = 0
        fun rootAt(line: Int, text: String): String? {
            val range = Range(Position(line, 0), Position(line, "class Module".length))
            configured++
            return tooltipOf(DocumentSymbol("Module", SymbolKind.Class, range, range, "public"), text, root = true, name = "Module$configured.ns")
        }
        val documented = "/// The module.\n/// Holds it.\n"
        val declaration = "class Module = () ()\n"
        val summarized = "<html>Root class<br>The module.</html>"
        assertEquals(summarized, rootAt(3, documented + "(* :module: com.acme.Module *)\n" + declaration))
        assertEquals(summarized, rootAt(4, documented + "(* :module: com.acme.Module@1.0.0 *)\n  (* :since: 2 *)\n" + declaration))
        assertEquals("Root class", rootAt(4, documented + "(* :module: com.acme.Module *)\n\n" + declaration))
        assertEquals("Root class", rootAt(4, documented + "(* :module: com.acme.Module *)\n(* a remark *)\n" + declaration))
        assertEquals("Root class", rootAt(2, documented + "(* :module: com.acme.Module *) " + declaration))
        assertEquals("Root class", rootAt(2, documented + "(* a remark *) " + declaration))
    }
}
