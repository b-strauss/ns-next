package io.github.bstrauss.newspeak.structure

import com.intellij.icons.AllIcons
import com.intellij.ide.structureView.TreeBasedStructureViewBuilder
import com.intellij.lang.LanguageStructureViewBuilder
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.util.Disposer
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.intellij.ui.icons.RowIcon
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
        val side = DocumentSymbol("instance side", SymbolKind.Namespace, range(0, 18, 2, 1), range(0, 18, 0, 19))
        side.children = listOf(method)
        val module = DocumentSymbol("Module", SymbolKind.Class, range(0, 0, 2, 1), range(0, 6, 0, 12), "public")
        module.children = listOf(side)
        val symbols = listOf(module)

        assertSame(method, innermostSymbolAt(document, symbols, text.indexOf("^1")))
        // Just past the method's closing parenthesis still counts as the method.
        assertSame(method, innermostSymbolAt(document, symbols, text.indexOf(")\n)") + 1))
        assertSame(module, innermostSymbolAt(document, symbols, text.indexOf("Module")))
        assertNull(innermostSymbolAt(document, symbols, text.length))
    }

    /** Under the platform's own `IconManager`, as the structure view draws it. */
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
}
