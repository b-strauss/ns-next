package io.github.bstrauss.newspeak

import com.intellij.extapi.psi.PsiFileBase
import com.intellij.openapi.fileTypes.FileType
import com.intellij.psi.FileViewProvider

class NewspeakFile(viewProvider: FileViewProvider) : PsiFileBase(viewProvider, NewspeakLanguage) {
    override fun getFileType(): FileType = NewspeakFileType
    override fun toString(): String = "Newspeak file"
}
