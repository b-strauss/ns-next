package io.github.bstrauss.newspeak.editor

import com.intellij.lang.Commenter

/** Newspeak has block comments only. */
class NewspeakCommenter : Commenter {
    override fun getLineCommentPrefix(): String? = null
    override fun getBlockCommentPrefix(): String = "(*"
    override fun getBlockCommentSuffix(): String = "*)"
    override fun getCommentedBlockCommentPrefix(): String? = null
    override fun getCommentedBlockCommentSuffix(): String? = null
}
