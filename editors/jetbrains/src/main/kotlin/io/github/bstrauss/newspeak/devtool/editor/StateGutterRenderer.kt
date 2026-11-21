package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.openapi.actionSystem.ActionGroup
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.editor.markup.GutterIconRenderer
import com.intellij.openapi.project.DumbAwareAction
import com.intellij.openapi.ui.popup.JBPopupFactory
import com.intellij.openapi.util.IconLoader
import com.intellij.ui.LayeredIcon
import com.intellij.ui.awt.RelativePoint
import com.intellij.util.IconUtil
import io.github.bstrauss.newspeak.devtool.model.MethodState
import java.awt.event.MouseEvent
import javax.swing.Icon
import javax.swing.SwingConstants

/**
 * The one icon at a method's header (`docs/design/developer-tool.md`,
 * decisions 27, 28 and 32): how it ran in the selected run, its [state],
 * grey when it is [stale], badged when it is [watched]; or, for a watched
 * method the run has no body of, an eye, [state] null. A click opens
 * [actions], which the gutter's context menu shows too.
 */
class StateGutterRenderer(
    val state: MethodState?,
    val stale: Boolean,
    val watched: Boolean,
    private val tooltip: String,
    private val actions: ActionGroup,
) : GutterIconRenderer() {
    override fun getIcon(): Icon {
        val base = when {
            state == null -> return WATCHED
            stale -> STALE
            else -> ICONS.getValue(state)
        }
        return if (watched) BADGED.getValue(base) else base
    }

    override fun getTooltipText(): String = tooltip

    override fun isNavigateAction(): Boolean = true

    override fun getPopupMenuActions(): ActionGroup = actions

    override fun getClickAction(): AnAction = object : DumbAwareAction() {
        override fun actionPerformed(event: AnActionEvent) {
            val popup = JBPopupFactory.getInstance()
                .createActionGroupPopup(null, actions, event.dataContext, JBPopupFactory.ActionSelectionAid.MNEMONICS, true)
            val mouse = event.inputEvent as? MouseEvent
            if (mouse != null) popup.show(RelativePoint(mouse)) else popup.showInBestPositionFor(event.dataContext)
        }
    }

    override fun getAlignment(): Alignment = Alignment.LEFT

    // Two renderers alike are one icon to the gutter, which then need not repaint.
    override fun equals(other: Any?): Boolean =
        other is StateGutterRenderer && other.state == state && other.stale == stale && other.watched == watched && other.tooltip == tooltip

    override fun hashCode(): Int = listOf(state, stale, watched, tooltip).hashCode()

    private companion object {
        fun icon(name: String): Icon = IconLoader.getIcon("/icons/devtool/$name.svg", StateGutterRenderer::class.java)

        val ICONS: Map<MethodState, Icon> = MethodState.entries.associateWith { state -> icon(state.name.lowercase()) }
        val STALE = icon("stale")
        val WATCHED = icon("watched")

        /** The eye at half its size, in the state icon's corner. */
        val BADGE: Icon = IconUtil.scale(WATCHED, null, 0.5f)

        /** Each state icon and the stale one with the badge, made once. */
        val BADGED: Map<Icon, Icon> = (ICONS.values + STALE).associateWith { base ->
            LayeredIcon(2).apply {
                setIcon(base, 0)
                setIcon(BADGE, 1, SwingConstants.SOUTH_EAST)
            }
        }
    }
}
