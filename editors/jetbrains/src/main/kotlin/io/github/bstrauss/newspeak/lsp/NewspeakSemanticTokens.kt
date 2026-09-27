package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.platform.lsp.api.customization.LspSemanticTokensSupport
import com.intellij.psi.PsiFile
import io.github.bstrauss.newspeak.highlighting.NewspeakColors

/** The server's legend onto the plugin's keys; null leaves the lexer's colour in place. */
fun semanticAttributesFor(tokenType: String, modifiers: List<String>): TextAttributesKey? = when (tokenType) {
    "parameter" -> NewspeakColors.PARAMETER
    "variable" -> NewspeakColors.LOCAL_VARIABLE
    "property" -> if ("readonly" in modifiers) NewspeakColors.IMMUTABLE_SLOT else NewspeakColors.SLOT
    "class" -> NewspeakColors.CLASS_NAME
    "method" -> if ("declaration" in modifiers) NewspeakColors.METHOD_DECLARATION else NewspeakColors.SEND
    // `lazy`, which the lexer can only see as an identifier.
    "keyword" -> NewspeakColors.KEYWORD
    else -> null
}

object NewspeakSemanticTokens : LspSemanticTokensSupport() {
    /**
     * The platform's default asks the server only for plain-text and TextMate
     * files, and a `.ns` file has a language of its own, so without this the
     * server is never asked and no slot, parameter or send is ever coloured.
     * The client descriptor already limits the server to Newspeak files.
     */
    override fun shouldAskServerForSemanticTokens(psiFile: PsiFile): Boolean = true

    override fun getTextAttributesKey(tokenType: String, modifiers: List<String>): TextAttributesKey? =
        semanticAttributesFor(tokenType, modifiers)
}
