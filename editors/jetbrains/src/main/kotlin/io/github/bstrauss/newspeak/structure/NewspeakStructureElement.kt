package io.github.bstrauss.newspeak.structure

import com.intellij.icons.AllIcons
import com.intellij.ide.projectView.PresentationData
import com.intellij.ide.structureView.StructureViewTreeElement
import com.intellij.navigation.ItemPresentation
import com.intellij.openapi.application.runReadAction
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.text.StringUtil
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.util.getOffsetInDocument
import com.intellij.psi.PsiFile
import com.intellij.ui.IconManager
import com.intellij.ui.icons.IconWrapperWithToolTip
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakIcons
import io.github.bstrauss.newspeak.editor.NewspeakDocComments
import io.github.bstrauss.newspeak.lsp.NewspeakSymbolKinds
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.SymbolKind
import javax.swing.Icon

/**
 * What a row of the structure view shows for one of the server's symbols
 * (`crates/ns-analyzer/src/document_symbols.rs`): its name, `lazy` beside a lazy
 * slot, the icon of its kind, the static mark on it for a class-side method,
 * the icon of its access, and the text shown on hover.
 */
data class SymbolPresentation(
    val name: String,
    val locationString: String?,
    val kindIcon: Icon?,
    val visibilityIcon: Icon?,
    val classSide: Boolean,
    val tooltip: String?,
)

/**
 * The server's `detail` is the effective access word, then ` class` for a
 * class-side method, ` value` for a value class and ` lazy` for a lazy slot
 * (`public class`, `public value`, `protected lazy`); a temporary has none.
 * A first word that is no access word shows no access icon rather than a
 * guessed one.
 *
 * The icons and hover texts follow what a member can do, not LSP's names for
 * the kinds: a slot with a getter only (`Constant`) and an immutable temporary
 * (`Constant` without `detail`) are constants, a slot with a getter and a
 * setter (`Property`) and a mutable temporary (`Variable`) are variables. The
 * root class, the module's one top-level class, has an icon of its own, and
 * so has a nested value class, `value` among the class's `detail` words; a
 * root value class keeps the root's icon and says `Root value class`.
 */
fun presentationOf(symbol: DocumentSymbol, root: Boolean = false): SymbolPresentation {
    val words = symbol.detail?.split(' ')?.filter { word -> word.isNotEmpty() }.orEmpty()
    val access = words.firstOrNull()?.takeIf { word -> word in ACCESS_WORDS }
    val visibilityIcon = when (access) {
        "public" -> AllIcons.Nodes.C_public
        "protected" -> AllIcons.Nodes.C_protected
        "private" -> AllIcons.Nodes.C_private
        else -> null
    }
    val classSide = "class" in words
    val isClass = symbol.kind == SymbolKind.Class
    val rootClass = root && isClass
    val valueClass = isClass && "value" in words
    return SymbolPresentation(
        name = symbol.name,
        locationString = if ("lazy" in words) NewspeakBundle.message("structure.lazy") else null,
        kindIcon = when {
            rootClass -> NewspeakIcons.ROOT_CLASS
            valueClass -> NewspeakIcons.VALUE_CLASS
            else -> symbol.kind?.let(NewspeakSymbolKinds::getIcon)
        },
        visibilityIcon = visibilityIcon,
        classSide = classSide,
        tooltip = when {
            rootClass -> NewspeakBundle.message(if (valueClass) "structure.root.value.class" else "structure.root.class")
            else -> when (symbol.kind) {
                SymbolKind.Class -> withAccess(access, NewspeakBundle.message(if (valueClass) "structure.kind.value.class" else "structure.kind.class"))
                SymbolKind.Constructor -> NewspeakBundle.message("structure.factory.method")
                SymbolKind.Constant -> if (access == null) NewspeakBundle.message("structure.local.constant") else withAccess(access, NewspeakBundle.message("structure.kind.constant"))
                SymbolKind.Property -> withAccess(access, NewspeakBundle.message("structure.kind.variable"))
                SymbolKind.Variable -> NewspeakBundle.message("structure.local.variable")
                SymbolKind.Method -> withAccess(access, NewspeakBundle.message(if (classSide) "structure.kind.class.method" else "structure.kind.method"))
                else -> null
            }
        },
    )
}

private val ACCESS_WORDS = setOf("public", "protected", "private")

/** `Public method`; a member the server sent without an access word reads `Method`. */
private fun withAccess(access: String?, kind: String): String {
    val text = when (access) {
        "public" -> NewspeakBundle.message("structure.access.public", kind)
        "protected" -> NewspeakBundle.message("structure.access.protected", kind)
        "private" -> NewspeakBundle.message("structure.access.private", kind)
        else -> NewspeakBundle.message("structure.access.none", kind)
    }
    return text.replaceFirstChar(Char::uppercaseChar)
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
    val baseIcon = presentation.kindIcon ?: return presentation.visibilityIcon
    val kindIcon = if (presentation.classSide) {
        IconManager.getInstance().createLayered(baseIcon, AllIcons.Nodes.StaticMark)
    } else {
        baseIcon
    }
    val visibilityIcon = presentation.visibilityIcon ?: return kindIcon
    return IconManager.getInstance().createRowIcon(kindIcon, visibilityIcon)
}

/**
 * The row icon with the hover text as its own tooltip, so that hovering the
 * icon and hovering the text say the same. `IconWrapperWithToolTip` is a
 * `RetrievableIcon` and keeps its tooltip when the tree replaces the icon
 * inside it (a dark or selected variant).
 */
fun iconWithHover(icon: Icon?, tooltip: String?): Icon? {
    if (icon == null || tooltip == null) return icon
    return IconWrapperWithToolTip(icon) { tooltip }
}

/**
 * One symbol of the tree, its children in the order the server sends them.
 * The value is the symbol itself, which is what
 * `NewspeakStructureViewModel.getCurrentEditorElement` answers, so the view
 * can select the row at the caret. `root` marks a top-level symbol of the
 * answer: the module's class. `psiFile`, the file the tree is built for,
 * gives a row's hover the summary of the doc comment above its symbol
 * (`NewspeakDocComments.summaryAbove`); without it the hover is the kind's
 * alone.
 */
class NewspeakStructureElement(
    private val project: Project,
    private val file: VirtualFile,
    private val symbol: DocumentSymbol,
    private val root: Boolean = false,
    private val psiFile: PsiFile? = null,
) : StructureViewTreeElement {
    override fun getValue(): DocumentSymbol = symbol

    /**
     * A `PresentationData`, because a plain `ItemPresentation` has no
     * tooltip: the structure view's tree copies a `PresentationData`'s tooltip
     * into its node (`PresentationData.updateFrom`) and shows it over the
     * text. Over the icon the tree shows the icon's own tooltip instead
     * (`SimpleColoredComponent.getIconToolTipText`), which a row icon composes
     * from its parts -- `Public` from the access icon -- so the icon carries
     * the same text (`iconWithHover`).
     */
    override fun getPresentation(): ItemPresentation {
        val presentation = presentationOf(symbol, root)
        val hover = withSummary(presentation.tooltip)
        val icon = iconWithHover(rowIconOf(presentation), hover)
        return PresentationData(presentation.name, presentation.locationString, icon, null).apply {
            tooltip = hover
        }
    }

    /**
     * The kind's hover, then on a line of its own the summary of the
     * symbol's doc comment, so the view tells what a member does without
     * opening it; read from the file's current text rather than asked of the
     * server, since the comment stands right above the symbol.
     */
    private fun withSummary(tooltip: String?): String? {
        val file = psiFile ?: return tooltip
        val line = symbol.range?.start?.line ?: return tooltip
        val summary = runReadAction { NewspeakDocComments.summaryAbove(file, line) } ?: return tooltip
        val lines = listOfNotNull(tooltip, summary).map(StringUtil::escapeXmlEntities)
        return lines.joinToString("<br>", prefix = "<html>", postfix = "</html>")
    }

    override fun getChildren(): Array<StructureViewTreeElement> =
        symbol.children.orEmpty()
            .map { child -> NewspeakStructureElement(project, file, child, psiFile = psiFile) }
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

    /**
     * Equal for an equal symbol (lsp4j compares its fields) in the same
     * place, so that a refresh bringing the same answer keeps the tree's
     * nodes, their expansion and selection, rather than redrawing them.
     */
    override fun equals(other: Any?): Boolean =
        other is NewspeakStructureElement && other.file == file && other.root == root && other.symbol == symbol

    override fun hashCode(): Int = symbol.hashCode() * 31 + root.hashCode()
}
