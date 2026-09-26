package io.github.bstrauss.newspeak.editor

import com.intellij.lang.BracePair
import com.intellij.lang.PairedBraceMatcher
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * `()`, `[]` and `{}`. Not `<>`: `<` and `>` are also binary selectors, and
 * pairing a comparison with a type's closing bracket would be worse than
 * pairing nothing.
 */
class NewspeakBraceMatcher : PairedBraceMatcher {
    override fun getPairs(): Array<BracePair> = arrayOf(
        BracePair(Types.LEFT_PARENTHESIS, Types.RIGHT_PARENTHESIS, false),
        BracePair(Types.LEFT_BRACKET, Types.RIGHT_BRACKET, false),
        BracePair(Types.LEFT_BRACE, Types.RIGHT_BRACE, false),
    )

    override fun isPairedBracesAllowedBeforeType(lbraceType: IElementType, contextType: IElementType?): Boolean = true

    override fun getCodeConstructStart(file: PsiFile?, openingBraceOffset: Int): Int = openingBraceOffset
}
