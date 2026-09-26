package io.github.bstrauss.newspeak.editor

import com.intellij.codeInsight.editorActions.SimpleTokenSetQuoteHandler
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes

class NewspeakQuoteHandler : SimpleTokenSetQuoteHandler(NewspeakTokenTypes.STRING)
