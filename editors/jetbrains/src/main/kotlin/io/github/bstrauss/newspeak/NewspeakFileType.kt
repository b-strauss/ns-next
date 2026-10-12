package io.github.bstrauss.newspeak

import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

object NewspeakFileType : LanguageFileType(NewspeakLanguage) {
    override fun getName(): String = "Newspeak"
    override fun getDescription(): String = NewspeakBundle.message("file.type.description")
    override fun getDefaultExtension(): String = "ns"
    override fun getIcon(): Icon = NewspeakIcons.FILE
}
