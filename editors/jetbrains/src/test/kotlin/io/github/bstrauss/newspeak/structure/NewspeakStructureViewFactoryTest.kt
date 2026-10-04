package io.github.bstrauss.newspeak.structure

import com.intellij.icons.AllIcons
import com.intellij.ide.structureView.TreeBasedStructureViewBuilder
import com.intellij.lang.LanguageStructureViewBuilder
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.util.Disposer
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.ide.projectView.PresentationData
import com.intellij.ui.icons.CompositeIcon
import com.intellij.ui.icons.RowIcon
import io.github.bstrauss.newspeak.NewspeakIcons
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind

class NewspeakStructureViewFactoryTest : BasePlatformTestCase() {
    /**
     * The platform's LSP structure view is registered for every language; the
     * plugin's, registered for Newspeak, is the one a Newspeak file gets.
     */
    fun testANewspeakFileGetsThePluginsStructureView() {
        val file = myFixture.configureByText("Module.ns", "class Module = () ()")
        val builder = LanguageStructureViewBuilder.getInstance().getStructureViewBuilder(file)
        assertInstanceOf(builder, TreeBasedStructureViewBuilder::class.java)
        val model = (builder as TreeBasedStructureViewBuilder).createStructureViewModel(myFixture.editor)
        try {
            assertInstanceOf(model, NewspeakStructureViewModel::class.java)
        } finally {
            Disposer.dispose(model)
        }
    }

    fun testTheCaretSelectsTheInnermostSymbolAroundIt() {
        val text = "class Module = () (\n    run = ( ^1 )\n)\n"
        val document = EditorFactory.getInstance().createDocument(text)
        fun range(startLine: Int, startCharacter: Int, endLine: Int, endCharacter: Int) =
            Range(Position(startLine, startCharacter), Position(endLine, endCharacter))
        val method = DocumentSymbol("run", SymbolKind.Method, range(1, 4, 1, 16), range(1, 4, 1, 7), "protected")
        val module = DocumentSymbol("Module", SymbolKind.Class, range(0, 0, 2, 1), range(0, 6, 0, 12), "public")
        module.children = listOf(method)
        val symbols = listOf(module)

        assertSame(method, innermostSymbolAt(document, symbols, text.indexOf("^1")))
        // Just past the method's closing parenthesis still counts as the method.
        assertSame(method, innermostSymbolAt(document, symbols, text.indexOf(")\n)") + 1))
        assertSame(module, innermostSymbolAt(document, symbols, text.indexOf("Module")))
        assertNull(innermostSymbolAt(document, symbols, text.length))
    }

    /** Under the test framework's `IconManager`, which joins the two icons in a row as the IDE's does. */
    fun testAMemberWithAnAccessShowsBothIconsInOneRow() {
        val range = Range(Position(0, 0), Position(0, 3))
        val presentation = presentationOf(DocumentSymbol("run", SymbolKind.Method, range, range, "private"))
        val icon = rowIconOf(presentation)
        assertInstanceOf(icon, RowIcon::class.java)
        val row = icon as RowIcon
        assertEquals(2, row.iconCount)
        assertSame(AllIcons.Nodes.Method, row.getIcon(0))
        assertSame(AllIcons.Nodes.C_private, row.getIcon(1))
    }

    /**
     * A class-side method shows the static mark on its kind's icon, as Java
     * shows a static member, and its access beside it. The platform's
     * `IconManager` answers a `LayeredIcon`, the test framework's a row of the
     * same icons; both are a `CompositeIcon` of the kind's icon and the mark.
     */
    fun testAClassSideMethodShowsTheStaticMarkAndItsAccess() {
        val range = Range(Position(0, 0), Position(0, 4))
        val presentation = presentationOf(DocumentSymbol("make", SymbolKind.Method, range, range, "public class"))
        val icon = rowIconOf(presentation)
        assertInstanceOf(icon, RowIcon::class.java)
        val row = icon as RowIcon
        assertEquals(2, row.iconCount)
        val kind = row.getIcon(0)
        assertInstanceOf(kind, CompositeIcon::class.java)
        val layers = kind as CompositeIcon
        assertOrderedEquals((0 until layers.iconCount).map(layers::getIcon), AllIcons.Nodes.Method, AllIcons.Nodes.StaticMark)
        assertSame(AllIcons.Nodes.C_public, row.getIcon(1))
    }

    /** The plugin's own node icons load from its resources, at the size of a tree icon. */
    fun testTheNodeIconsLoad() {
        for (icon in listOf(NewspeakIcons.ROOT_CLASS, NewspeakIcons.VALUE_CLASS)) {
            assertNotNull(icon)
            assertEquals(16, icon.iconWidth)
            assertEquals(16, icon.iconHeight)
        }
    }

    /**
     * The root class's row: the root-class icon with the public icon beside it,
     * and its hover text, carried as `PresentationData`'s tooltip, which the
     * structure view's tree copies from the element's presentation.
     */
    fun testTheRootClassShowsItsIconAndItsHover() {
        val file = myFixture.configureByText("Module.ns", "class Module = () ()").virtualFile
        val range = Range(Position(0, 6), Position(0, 12))
        val symbol = DocumentSymbol("Module", SymbolKind.Class, range, range, "public")
        val presentation = NewspeakStructureElement(project, file, symbol, root = true).presentation
        assertInstanceOf(presentation, PresentationData::class.java)
        assertEquals("Root class", (presentation as PresentationData).tooltip)
        assertEquals("Module", presentation.presentableText)
        val row = presentation.getIcon(false) as RowIcon
        assertSame(NewspeakIcons.ROOT_CLASS, row.getIcon(0))
        assertSame(AllIcons.Nodes.C_public, row.getIcon(1))
        val child = NewspeakStructureElement(project, file, symbol).presentation as PresentationData
        assertEquals("Public class", child.tooltip)
    }
}
