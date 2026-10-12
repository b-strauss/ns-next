package io.github.bstrauss.newspeak.settings

import com.intellij.application.options.editor.CodeFoldingOptionsProvider
import com.intellij.openapi.options.BeanConfigurable
import io.github.bstrauss.newspeak.NewspeakBundle

/** The Newspeak section of *Settings | Editor | General | Code Folding*, beside Java's. */
class NewspeakCodeFoldingOptionsProvider :
    BeanConfigurable<NewspeakSettings.Settings>(
        NewspeakSettings.getInstance().state,
        NewspeakBundle.message("folding.section.title"),
    ),
    CodeFoldingOptionsProvider {
    init {
        checkBox(NewspeakBundle.message("folding.one.line.methods"), instance::collapseOneLineMethods)
    }
}
