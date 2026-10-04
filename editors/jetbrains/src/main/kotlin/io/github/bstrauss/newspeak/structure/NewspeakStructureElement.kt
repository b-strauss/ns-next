package io.github.bstrauss.newspeak.structure

import com.intellij.icons.AllIcons
import com.intellij.ide.structureView.StructureViewTreeElement
import com.intellij.navigation.ItemPresentation
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.util.getOffsetInDocument
import com.intellij.ui.IconManager
import io.github.bstrauss.newspeak.lsp.NewspeakSymbolKinds
import org.eclipse.lsp4j.DocumentSymbol
import javax.swing.Icon

/**
 * What a row of the structure view shows for one of the server's symbols
 * (`crates/ns-analyzer/src/document_symbols.rs`): its name, `lazy` beside a lazy
 * slot, the icon of its kind, the static mark on it for a class-side method,
 * and the icon of its access.
 */
data class SymbolPresentation(
    val name: String,
    val locationString: String?,
    val kindIcon: Icon?,
    val visibilityIcon: Icon?,
    val classSide: Boolean,
)

/**
 * The server's `detail` is the effective access word, then ` class` for a
 * class-side method and ` lazy` for a lazy slot (`public class`,
 * `protected lazy`); a temporary has none. A first word that is no access word
 * shows no access icon rather than a guessed one.
 */
fun presentationOf(symbol: DocumentSymbol): SymbolPresentation {
    val words = symbol.detail?.split(' ')?.filter { word -> word.isNotEmpty() }.orEmpty()
    val visibilityIcon = when (words.firstOrNull()) {
        "public" -> AllIcons.Nodes.C_public
        "protected" -> AllIcons.Nodes.C_protected
        "private" -> AllIcons.Nodes.C_private
        else -> null
    }
    return SymbolPresentation(
        name = symbol.name,
        locationString = if ("lazy" in words) "lazy" else null,
        kindIcon = symbol.kind?.let(NewspeakSymbolKinds::getIcon),
        visibilityIcon = visibilityIcon,
        classSide = "class" in words,
    )
}

/**
 * The kind's icon with the access icon beside it, as Java's structure view
 * draws a member: Java's `ElementBase.buildRowIcon` joins the two with
 * `IconManager.createRowIcon`, and so does this.
 *
 * A class-side method has the static mark laid over its kind's icon, Java's
 * idiom for a static member: the tree has no node for a side, and a structure
 * view draws no separator, so the mark is what tells the two sides apart.
 * Java registers the mark as an icon layer and lays it over the base icon;
 * `IconManager.createLayered` builds the same layered icon without claiming a
 * layer flag of the platform's.
 */
fun rowIconOf(presentation: SymbolPresentation): Icon? {
    val kindIcon = presentation.kindIcon
        ?.let { icon ->
            if (presentation.classSide) IconManager.getInstance().createLayered(icon, AllIcons.Nodes.StaticMark) else icon
        }
        ?: return presentation.visibilityIcon
    val visibilityIcon = presentation.visibilityIcon ?: return kindIcon
    return IconManager.getInstance().createRowIcon(kindIcon, visibilityIcon)
}

/**
 * One symbol of the tree, its children in the order the server sends them.
 * The value is the symbol itself, which is what
 * `NewspeakStructureViewModel.getCurrentEditorElement` answers, so the view
 * can select the row at the caret.
 */
class NewspeakStructureElement(
    private val project: Project,
    private val file: VirtualFile,
    private val symbol: DocumentSymbol,
) : StructureViewTreeElement {
    override fun getValue(): DocumentSymbol = symbol

    override fun getPresentation(): ItemPresentation {
        val presentation = presentationOf(symbol)
        return object : ItemPresentation {
            override fun getPresentableText(): String = presentation.name
            override fun getLocationString(): String? = presentation.locationString
            override fun getIcon(open: Boolean): Icon? = rowIconOf(presentation)
        }
    }

    override fun getChildren(): Array<StructureViewTreeElement> =
        symbol.children.orEmpty()
            .map { child -> NewspeakStructureElement(project, file, child) }
            .toTypedArray()

    /** To the symbol's name, which is what its `selectionRange` covers; positions are UTF-16, as a document's are. */
    override fun navigate(requestFocus: Boolean) {
        val document = FileDocumentManager.getInstance().getDocument(file) ?: return
        val start = symbol.selectionRange?.start ?: return
        val offset = getOffsetInDocument(document, start) ?: return
        OpenFileDescriptor(project, file, offset).navigate(requestFocus)
    }

    override fun canNavigate(): Boolean = true

    override fun canNavigateToSource(): Boolean = true
}
