package io.github.bstrauss.newspeak.parser

import com.intellij.extapi.psi.ASTWrapperPsiElement
import com.intellij.lang.ASTNode
import com.intellij.lang.ParserDefinition
import com.intellij.lang.PsiParser
import com.intellij.lexer.Lexer
import com.intellij.openapi.project.Project
import com.intellij.psi.FileViewProvider
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IFileElementType
import com.intellij.psi.tree.TokenSet
import io.github.bstrauss.newspeak.NewspeakFile
import io.github.bstrauss.newspeak.NewspeakLanguage
import io.github.bstrauss.newspeak.lexer.NewspeakLexer
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes

/**
 * Every token under one file node, with no grammar: this is deliberately not
 * a second Newspeak parser (`docs/design/editor-support.md`). It exists so the
 * platform knows which tokens are comments and strings -- for TODO markers,
 * spell checking and the quote handler. Structure comes from the language
 * server.
 */
class NewspeakParserDefinition : ParserDefinition {
    override fun createLexer(project: Project?): Lexer = NewspeakLexer()

    override fun createParser(project: Project?): PsiParser = PsiParser { root, builder ->
        val marker = builder.mark()
        while (!builder.eof()) builder.advanceLexer()
        marker.done(root)
        builder.treeBuilt
    }

    override fun getFileNodeType(): IFileElementType = FILE
    override fun getCommentTokens(): TokenSet = COMMENTS
    override fun getStringLiteralElements(): TokenSet = STRINGS
    override fun createElement(node: ASTNode): PsiElement = ASTWrapperPsiElement(node)
    override fun createFile(viewProvider: FileViewProvider): PsiFile = NewspeakFile(viewProvider)

    companion object {
        @JvmField val FILE = IFileElementType(NewspeakLanguage)
        @JvmField val COMMENTS: TokenSet = TokenSet.create(NewspeakTokenTypes.COMMENT)
        @JvmField val STRINGS: TokenSet = TokenSet.create(NewspeakTokenTypes.STRING)
    }
}
