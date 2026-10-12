package io.github.bstrauss.newspeak.settings

import com.intellij.application.options.editor.CodeFoldingOptionsProvider
import com.intellij.openapi.options.BeanConfigurable

/** The Newspeak section of *Settings | Editor | General | Code Folding*, beside Java's. */
class NewspeakCodeFoldingOptionsProvider :
    BeanConfigurable<NewspeakSettings.Settings>(NewspeakSettings.getInstance().state, "Newspeak"),
    CodeFoldingOptionsProvider {
    init {
        checkBox("One-line methods", instance::collapseOneLineMethods)
    }
}
