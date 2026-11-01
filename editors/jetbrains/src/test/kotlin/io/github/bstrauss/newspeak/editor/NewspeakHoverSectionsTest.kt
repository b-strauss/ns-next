package io.github.bstrauss.newspeak.editor

import com.intellij.markdown.utils.doc.DocMarkdownToHtmlConverter
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.NewspeakLanguage

/**
 * Pins the form of the section table the language server's hover writes
 * (`crates/ns-analyzer/src/hover.rs`): the platform's converter has to keep
 * the table's classes, which make it the platform's section table, and
 * render the cells' text.
 */
class NewspeakHoverSectionsTest : BasePlatformTestCase() {
    fun testTheSectionTableKeepsItsClassesAndItsCellsRender() {
        val markdown = "Evaluates.\n\n<table class='sections'>\n" +
            "<tr><td valign='top' class='section'><p>Params:</td><td valign='top'>\n\n" +
            "`stop` – a number.\n\n</td></tr>\n</table>"
        val html = DocMarkdownToHtmlConverter.convert(project, markdown, NewspeakLanguage)
        assertTrue(html, html.contains("class='sections'") || html.contains("class=\"sections\""))
        assertTrue(html, Regex("<code>(<span[^>]*>)?stop(</span>)?</code>").containsMatchIn(html))
    }
}
