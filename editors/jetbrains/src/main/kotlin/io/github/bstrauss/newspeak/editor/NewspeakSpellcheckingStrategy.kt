package io.github.bstrauss.newspeak.editor

import com.intellij.psi.PsiComment
import com.intellij.psi.PsiElement
import com.intellij.spellchecker.tokenizer.SpellcheckingStrategy
import com.intellij.spellchecker.tokenizer.Tokenizer
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes

/** Spell checking in comments and strings, and nowhere else: identifiers are code. */
class NewspeakSpellcheckingStrategy : SpellcheckingStrategy() {
    override fun getTokenizer(element: PsiElement): Tokenizer<*> = when {
        element is PsiComment -> SpellcheckingStrategy.TEXT_TOKENIZER
        element.node?.elementType == NewspeakTokenTypes.STRING -> SpellcheckingStrategy.TEXT_TOKENIZER
        else -> SpellcheckingStrategy.EMPTY_TOKENIZER
    }
}
