package io.github.bstrauss.newspeak.lsp

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.platform.lsp.api.customization.LspSemanticTokensSupport
import io.github.bstrauss.newspeak.highlighting.NewspeakColors

/** The server's legend onto the plugin's keys; null leaves the lexer's colour in place. */
fun semanticAttributesFor(tokenType: String, modifiers: List<String>): TextAttributesKey? = when (tokenType) {
    "parameter" -> NewspeakColors.PARAMETER
    "variable" -> NewspeakColors.LOCAL_VARIABLE
    "property" -> if ("readonly" in modifiers) NewspeakColors.IMMUTABLE_SLOT else NewspeakColors.SLOT
    "class" -> NewspeakColors.CLASS_NAME
    "method" -> if ("declaration" in modifiers) NewspeakColors.METHOD_DECLARATION else NewspeakColors.SEND
    else -> null
}

object NewspeakSemanticTokens : LspSemanticTokensSupport() {
    override fun getTextAttributesKey(tokenType: String, modifiers: List<String>): TextAttributesKey? =
        semanticAttributesFor(tokenType, modifiers)
}
