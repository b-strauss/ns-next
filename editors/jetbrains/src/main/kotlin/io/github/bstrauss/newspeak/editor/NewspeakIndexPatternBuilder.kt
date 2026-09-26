package io.github.bstrauss.newspeak.editor

import com.intellij.lexer.Lexer
import com.intellij.psi.PsiFile
import com.intellij.psi.impl.search.IndexPatternBuilder
import com.intellij.psi.tree.IElementType
import com.intellij.psi.tree.TokenSet
import io.github.bstrauss.newspeak.NewspeakFile
import io.github.bstrauss.newspeak.lexer.NewspeakLexer
import io.github.bstrauss.newspeak.parser.NewspeakParserDefinition

/** Where TODO markers are searched: inside `(* ... *)`, not counting the delimiters. */
class NewspeakIndexPatternBuilder : IndexPatternBuilder {
    override fun getIndexingLexer(file: PsiFile): Lexer? = if (file is NewspeakFile) NewspeakLexer() else null
    override fun getCommentTokenSet(file: PsiFile): TokenSet? =
        if (file is NewspeakFile) NewspeakParserDefinition.COMMENTS else null
    override fun getCommentStartDelta(tokenType: IElementType?): Int = 2
    override fun getCommentEndDelta(tokenType: IElementType?): Int = 2
}
