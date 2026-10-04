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
 * The icons say what a symbol can do. LSP has no kind for a read-only
 * property, so the server sends a slot with a getter only as `Constant` and a
 * slot with a getter and a setter as `Property`, the nearest two kinds any
 * client knows, and a method's temporary as `Constant` when declared with `=`
 * and `Variable` otherwise. A slot or temporary that cannot be assigned shows
 * as a constant, one that can as a variable. A factory shows as a function,
 * which it is: a message answering a new instance. Every kind is named, so a
 * change of the platform's defaults cannot change what the tree says.
 */
object NewspeakSymbolKinds : LspSymbolKindCustomizer() {
    override fun getIcon(kind: SymbolKind): Icon? = when (kind) {
        SymbolKind.Class -> AllIcons.Nodes.Class
        SymbolKind.Constructor -> AllIcons.Nodes.Function
        SymbolKind.Constant -> AllIcons.Nodes.Constant
        SymbolKind.Property -> AllIcons.Nodes.Variable
        SymbolKind.Variable -> AllIcons.Nodes.Variable
        SymbolKind.Method -> AllIcons.Nodes.Method
        else -> super.getIcon(kind)
    }
}
