package io.github.bstrauss.newspeak.lsp

import com.intellij.icons.AllIcons
import org.eclipse.lsp4j.SymbolKind
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The kinds the server's document symbols carry (`crates/ns-analyzer/src/document_symbols.rs`, `encode`) onto icons. */
class NewspeakSymbolKindsTest {
    @Test fun aSlotShowsItsAccessors() {
        // A slot with a getter only, then a slot with a getter and a setter.
        assertSame(AllIcons.Nodes.PropertyRead, NewspeakSymbolKinds.getIcon(SymbolKind.Constant))
        assertSame(AllIcons.Nodes.PropertyReadWrite, NewspeakSymbolKinds.getIcon(SymbolKind.Property))
    }

    @Test fun everyOtherKindTheServerSendsKeepsThePlatformsIcon() {
        assertSame(AllIcons.Nodes.Class, NewspeakSymbolKinds.getIcon(SymbolKind.Class))
        // Java shows a constructor with the method icon too.
        assertSame(AllIcons.Nodes.Method, NewspeakSymbolKinds.getIcon(SymbolKind.Constructor))
        assertSame(AllIcons.Nodes.Method, NewspeakSymbolKinds.getIcon(SymbolKind.Method))
        assertSame(AllIcons.Nodes.Variable, NewspeakSymbolKinds.getIcon(SymbolKind.Variable))
    }

    @Test fun theClientUsesThePluginsKindsAndStructureView() {
        assertSame(NewspeakSymbolKinds, NewspeakLspCustomization.symbolKindCustomizer)
        assertSame(NewspeakSemanticTokens, NewspeakLspCustomization.semanticTokensCustomizer)
        val documentSymbols = NewspeakLspCustomization.documentSymbolCustomizer
        // The platform's structure view could not show a member's access; its breadcrumbs stay.
        assertFalse(documentSymbols.structureViewSupport)
        assertTrue(documentSymbols.breadcrumbsSupport)
    }
}
