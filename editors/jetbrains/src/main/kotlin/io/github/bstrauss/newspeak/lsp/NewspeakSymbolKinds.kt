package io.github.bstrauss.newspeak.lsp

import com.intellij.icons.AllIcons
import com.intellij.platform.lsp.api.customization.LspSymbolKindCustomizer
import org.eclipse.lsp4j.SymbolKind
import javax.swing.Icon

/**
 * The icon of each kind the server's document symbols carry
 * (`crates/ns-analyzer/src/document_symbols.rs`, `encode`), for the breadcrumbs and
 * sticky lines the platform draws from them and for the plugin's own
 * structure view.
 *
 * LSP has no kind for a read-only property, so the server sends a slot with a
 * getter only as `Constant` and a slot with a getter and a setter as
 * `Property`, the nearest two kinds any client knows. The platform's default
 * would draw them as a constant and a generic property; a Newspeak slot is
 * always reached through its accessors, which IntelliJ's read-only and
 * read-write property icons say exactly.
 *
 * A `Namespace` is a class's instance side or class side: a named group of the
 * class's members, not a package, so it gets the icon the platform's structure
 * view gives such a group (a `//region`) rather than the default package icon.
 */
object NewspeakSymbolKinds : LspSymbolKindCustomizer() {
    override fun getIcon(kind: SymbolKind): Icon? = when (kind) {
        SymbolKind.Constant -> AllIcons.Nodes.PropertyRead
        SymbolKind.Property -> AllIcons.Nodes.PropertyReadWrite
        SymbolKind.Namespace -> AllIcons.Nodes.CustomRegion
        else -> super.getIcon(kind)
    }
}
