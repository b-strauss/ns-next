package io.github.bstrauss.newspeak.lsp

import com.intellij.icons.AllIcons
import org.eclipse.lsp4j.SymbolKind
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The kinds the server's document symbols carry (`crates/ns-analyzer/src/document_symbols.rs`, `encode`) onto icons. */
class NewspeakSymbolKindsTest {
    @Test fun aSlotOrTemporaryShowsWhetherItCanBeAssigned() {
        // A slot with a getter only or an immutable temporary, then a slot
        // with a getter and a setter, then a mutable temporary.
        assertSame(AllIcons.Nodes.Constant, NewspeakSymbolKinds.getIcon(SymbolKind.Constant))
        assertSame(AllIcons.Nodes.Variable, NewspeakSymbolKinds.getIcon(SymbolKind.Property))
        assertSame(AllIcons.Nodes.Variable, NewspeakSymbolKinds.getIcon(SymbolKind.Variable))
    }

    @Test fun aFactoryShowsAsAFunction() {
        assertSame(AllIcons.Nodes.Function, NewspeakSymbolKinds.getIcon(SymbolKind.Constructor))
    }

    @Test fun everyOtherKindTheServerSendsKeepsThePlatformsIcon() {
        assertSame(AllIcons.Nodes.Class, NewspeakSymbolKinds.getIcon(SymbolKind.Class))
        assertSame(AllIcons.Nodes.Method, NewspeakSymbolKinds.getIcon(SymbolKind.Method))
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
