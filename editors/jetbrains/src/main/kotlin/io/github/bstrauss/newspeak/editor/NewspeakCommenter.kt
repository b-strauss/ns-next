package io.github.bstrauss.newspeak.editor

import com.intellij.lang.Commenter

/** Both of ns-next's comment forms: `///` lines and `(* … *)` blocks. */
class NewspeakCommenter : Commenter {
    override fun getLineCommentPrefix(): String = "///"
    override fun getBlockCommentPrefix(): String = "(*"
    override fun getBlockCommentSuffix(): String = "*)"
    override fun getCommentedBlockCommentPrefix(): String? = null
    override fun getCommentedBlockCommentSuffix(): String? = null
}
