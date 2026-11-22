package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.ui.ComboBox
import com.intellij.ui.SimpleListCellRenderer
import com.intellij.ui.SearchTextField
import io.github.bstrauss.newspeak.NewspeakBundle
import java.awt.event.ItemEvent
import javax.swing.event.DocumentEvent
import javax.swing.event.DocumentListener

/**
 * How the Methods and the Events tab group their rows: not at all, under
 * their method -- a block's under its method's -- or under their class,
 * the Events tab's with each method between the class and its events.
 */
enum class Grouping(val key: String) {
    NONE("devtool.group.none"),
    METHOD("devtool.group.method"),
    CLASS("devtool.group.class"),
}

/** The chooser of a tab's [Grouping], none chosen first, calling [changed] when another is chosen. */
fun groupingChooser(changed: () -> Unit): ComboBox<Grouping> =
    ComboBox(Grouping.entries.toTypedArray()).apply {
        renderer = SimpleListCellRenderer.create("") { grouping -> NewspeakBundle.message(grouping.key) }
        // A change of choice is told twice, as the old item's deselection and the new one's selection.
        addItemListener { event -> if (event.stateChange == ItemEvent.SELECTED) changed() }
    }

/**
 * The field of a part of a name, its [placeholder] shown while it is
 * empty, with the search field's button that clears it, and no history,
 * calling [changed] at every edit.
 */
fun nameFilterField(placeholder: String, changed: () -> Unit): SearchTextField =
    SearchTextField(false).apply {
        textEditor.columns = 30
        textEditor.emptyText.text = placeholder
        addDocumentListener(
            object : DocumentListener {
                override fun insertUpdate(event: DocumentEvent) = changed()

                override fun removeUpdate(event: DocumentEvent) = changed()

                override fun changedUpdate(event: DocumentEvent) = changed()
            },
        )
    }

/** Whether [name] holds [needle], the filter's text trimmed, regardless of case; every name holds an empty one. */
fun nameMatches(name: String, needle: String): Boolean = needle.isEmpty() || name.contains(needle, ignoreCase = true)

private const val BLOCK_PREFIX = "[] in "
private const val CLASS_SIDE_SUFFIX = " class"
private const val SELECTOR_SEPARATOR = ">>"

/**
 * The class a body's [name] belongs to, as the tools name it: the part
 * before `>>` of a method's name, before the first space of an
 * initializer's or a superclass clause's -- `X initializer`, `X
 * initializer, own part`, `X class initializer`, `X superclass clause`
 * -- with a block's `[] in` and the class side's ` class` left out, so
 * that a class's factory, its initializers and both sides' methods share
 * one; a module's root is the class its name gives, and a body named by
 * its number alone stands for itself.
 */
fun classOf(name: String): String {
    var rest = name
    while (rest.startsWith(BLOCK_PREFIX)) rest = rest.removePrefix(BLOCK_PREFIX)
    val selector = rest.indexOf(SELECTOR_SEPARATOR)
    rest = if (selector >= 0) rest.substring(0, selector) else rest.substringBefore(' ')
    return rest.removeSuffix(CLASS_SIDE_SUFFIX)
}
