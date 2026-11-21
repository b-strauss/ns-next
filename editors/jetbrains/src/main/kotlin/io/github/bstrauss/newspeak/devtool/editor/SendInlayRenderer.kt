package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorCustomElementRenderer
import com.intellij.openapi.editor.Inlay
import com.intellij.openapi.editor.colors.EditorFontType
import com.intellij.openapi.editor.markup.TextAttributes
import com.intellij.ui.JBColor
import java.awt.Font
import java.awt.Graphics
import java.awt.Rectangle

/**
 * The hint after a send's selector (`docs/design/developer-tool.md`,
 * decision 27): [text] what became of the send -- `inlined` or
 * `declined: <reason>` -- and its inline cache's state, [tooltip] the
 * fields of the events that said so, shown on hover.
 */
class SendInlayRenderer(val text: String, val tooltip: String) : EditorCustomElementRenderer {
    override fun calcWidthInPixels(inlay: Inlay<*>): Int =
        inlay.editor.contentComponent.getFontMetrics(font(inlay.editor)).stringWidth(text) + 2 * PADDING + MARGIN

    override fun paint(inlay: Inlay<*>, graphics: Graphics, targetRegion: Rectangle, textAttributes: TextAttributes) {
        val editor = inlay.editor
        val attributes = editor.colorsScheme.getAttributes(DefaultLanguageHighlighterColors.INLAY_DEFAULT)
        val font = font(editor)
        val metrics = editor.contentComponent.getFontMetrics(font)
        val left = targetRegion.x + MARGIN
        val width = targetRegion.width - MARGIN
        attributes?.backgroundColor?.let { background ->
            graphics.color = background
            graphics.fillRoundRect(left, targetRegion.y + 1, width, targetRegion.height - 2, ARC, ARC)
        }
        graphics.color = attributes?.foregroundColor ?: JBColor.GRAY
        graphics.font = font
        val baseline = targetRegion.y + (targetRegion.height + metrics.ascent - metrics.descent) / 2
        graphics.drawString(text, left + PADDING, baseline)
    }

    /** The editor's font a size smaller, as the platform's inlay hints are. */
    private fun font(editor: Editor): Font {
        val plain = editor.colorsScheme.getFont(EditorFontType.PLAIN)
        return plain.deriveFont((plain.size2D - 1f).coerceAtLeast(1f))
    }

    private companion object {
        const val PADDING = 4
        const val MARGIN = 4
        const val ARC = 6
    }
}
