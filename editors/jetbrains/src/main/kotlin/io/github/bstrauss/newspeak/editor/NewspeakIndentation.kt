package io.github.bstrauss.newspeak.editor

import com.intellij.codeInsight.editorActions.EnterHandler
import com.intellij.codeInsight.editorActions.TypedHandlerDelegate
import com.intellij.codeInsight.editorActions.enter.EnterBetweenBracesDelegate
import com.intellij.lang.Language
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.project.Project
import com.intellij.psi.PsiFile
import com.intellij.psi.TokenType
import com.intellij.psi.codeStyle.lineIndent.LineIndentProvider
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.NewspeakLanguage
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * Where a line of Newspeak starts, as the formatter would place it, read
 * from the editor's tokens alone: the plugin has no Newspeak parser
 * (`NewspeakParserDefinition`), and the formatter's own answer comes only
 * from Reformat Code. So only what the tokens show is decided:
 *
 * - a line after one ending in an opening `(`, `[` or `{`, or in the `|`
 *   that closes a block's parameters, comments after it aside, stands two
 *   columns deeper than the opener's line;
 * - a line starting with a closing `)`, `]` or `}` stands where the line of
 *   its opener does;
 * - any other line keeps the indentation of the line before it.
 *
 * No continuation indentation is guessed, since telling a broken message
 * from a statement needs the grammar. A string or comment spanning lines is
 * never re-indented, and a line it continues is measured from the line it
 * starts on.
 */
object NewspeakIndentation {
    private val OPENERS = setOf(Types.LEFT_PARENTHESIS, Types.LEFT_BRACKET, Types.LEFT_BRACE)
    private val CLOSERS = setOf(Types.RIGHT_PARENTHESIS, Types.RIGHT_BRACKET, Types.RIGHT_BRACE)
    private val COMMENTS = setOf(Types.COMMENT, Types.LINE_COMMENT, Types.DOC_COMMENT)

    private class Token(val type: IElementType, val start: Int, val end: Int)

    /**
     * The indentation of the line holding [offset], or null inside a string
     * or comment spanning lines, which is left as the user wrote it.
     */
    fun lineIndent(editor: Editor, offset: Int): String? {
        val lineStart = lineStartOf(editor, offset)
        if (continuesAToken(editor, lineStart)) return null
        val first = tokensFrom(editor, lineStart).firstOrNull { token -> token.type != TokenType.WHITE_SPACE }
        if (first != null && first.type in CLOSERS && onLineOf(editor, lineStart, first.start)) {
            matchingOpener(editor, first.start)?.let { opener -> return indentationOfLineHolding(editor, opener.start) }
        }
        val previous = tokensBefore(editor, lineStart).filter { token -> token.type != TokenType.WHITE_SPACE }
        val lastCode = previous.firstOrNull { token -> token.type !in COMMENTS }
        if (lastCode != null && (lastCode.type in OPENERS || lastCode.type == Types.BAR && closesBlockParameters(editor, lastCode))) {
            return indentationOfLineHolding(editor, lastCode.start) + " ".repeat(NewspeakCodeStyle.INDENT_SIZE)
        }
        val last = previous.firstOrNull() ?: return ""
        // A doc comment is one token over several lines, each starting its
        // line: the line before is measured, not the comment's first.
        if (last.type == Types.DOC_COMMENT) return indentationOfLineHolding(editor, minOf(last.end, lineStart - 1))
        return indentationOfLineHolding(editor, last.start)
    }

    /** Moves the closing bracket at [offset], when it is the first thing on its line, to its opener line's indentation. */
    fun outdentClosingBracket(editor: Editor, offset: Int) {
        val closer = tokensFrom(editor, offset).firstOrNull() ?: return
        if (closer.start != offset || closer.type !in CLOSERS) return
        val text = editor.document.charsSequence
        val lineStart = lineStartOf(editor, offset)
        if ((lineStart until offset).any { index -> text[index] != ' ' && text[index] != '\t' }) return
        val opener = matchingOpener(editor, offset) ?: return
        val indentation = indentationOfLineHolding(editor, opener.start)
        if (text.subSequence(lineStart, offset).toString() != indentation) {
            editor.document.replaceString(lineStart, offset, indentation)
        }
    }

    /** A bracket pair, or a block's parameters and its `]` (`[:each |]`), which open alike. */
    fun isBracketPair(opening: Char, closing: Char): Boolean =
        opening == '(' && closing == ')' || opening == '[' && closing == ']' || opening == '{' && closing == '}' ||
            opening == '|' && closing == ']'

    /**
     * Whether [opening] and [closing] are an opener's (or a block parameters'
     * `|`) and a closer's tokens, not characters inside a string or comment.
     */
    fun areBracketTokens(editor: Editor, opening: Int, closing: Int): Boolean {
        val opener = tokensFrom(editor, opening).firstOrNull() ?: return false
        val closer = tokensFrom(editor, closing).firstOrNull() ?: return false
        val opens = opener.type in OPENERS || opener.type == Types.BAR && closesBlockParameters(editor, opener)
        return opener.start == opening && opens && closer.start == closing && closer.type in CLOSERS
    }

    /**
     * Whether [offset] lies in a comment's token -- for a doc comment, the
     * whitespace between its lines included. Its one caller asks about a
     * bracket pair Enter splits, which never stands in that whitespace.
     */
    fun isInComment(editor: Editor, offset: Int): Boolean =
        tokensFrom(editor, offset).firstOrNull()?.let { token -> token.type in COMMENTS } == true

    /** The closer's opener: brackets of every kind counted, as the formatter's tree nests them. */
    private fun matchingOpener(editor: Editor, closerStart: Int): Token? {
        var depth = 0
        for (token in tokensBefore(editor, closerStart)) {
            when (token.type) {
                in CLOSERS -> depth++
                in OPENERS -> if (depth == 0) return token else depth--
            }
        }
        return null
    }

    /**
     * Whether [bar] ends `[:a :b |`, read backwards: each parameter a `:`
     * and a name, with or without a type in angle brackets, back to the `[`.
     * A `|` of temporaries or of slots is no block's parameters.
     */
    private fun closesBlockParameters(editor: Editor, bar: Token): Boolean {
        val tokens = tokensBefore(editor, bar.start)
            .filter { token -> token.type != TokenType.WHITE_SPACE && token.type !in COMMENTS }
            .iterator()
        var parameters = 0
        while (tokens.hasNext()) {
            var token = tokens.next()
            if (token.type == Types.LEFT_BRACKET) return parameters > 0
            if (token.type == Types.GREATER_THAN) {
                if (!skipType(tokens)) return false
                if (!tokens.hasNext()) return false
                token = tokens.next()
            }
            if (token.type != Types.IDENTIFIER || !tokens.hasNext() || tokens.next().type != Types.COLON) return false
            parameters++
        }
        return false
    }

    /** Steps back over a type annotation whose closing `>` was just read, to just before its `<`. */
    private fun skipType(tokens: Iterator<Token>): Boolean {
        var depth = 1
        while (tokens.hasNext()) {
            when (tokens.next().type) {
                Types.GREATER_THAN -> depth++
                Types.LESS_THAN -> if (--depth == 0) return true
            }
        }
        return false
    }

    /**
     * The leading whitespace of the line holding [offset] -- or, where that
     * line continues a string or comment, of the line it starts on.
     */
    private fun indentationOfLineHolding(editor: Editor, offset: Int): String {
        var lineStart = lineStartOf(editor, offset)
        while (continuesAToken(editor, lineStart)) {
            lineStart = lineStartOf(editor, tokensFrom(editor, lineStart - 1).first().start)
        }
        val text = editor.document.charsSequence
        var end = lineStart
        while (end < text.length && (text[end] == ' ' || text[end] == '\t')) end++
        return text.subSequence(lineStart, end).toString()
    }

    /**
     * Whether the line break before [lineStart] belongs to a string or
     * comment, which the line then continues. A doc comment's line breaks
     * do not: each of its lines is a `///` line starting its line, indented
     * as any other.
     */
    private fun continuesAToken(editor: Editor, lineStart: Int): Boolean {
        if (lineStart == 0) return false
        val token = tokensFrom(editor, lineStart - 1).firstOrNull() ?: return false
        return token.type != TokenType.WHITE_SPACE && token.type != Types.DOC_COMMENT
    }

    private fun onLineOf(editor: Editor, lineStart: Int, offset: Int): Boolean = lineStartOf(editor, offset) == lineStart

    private fun lineStartOf(editor: Editor, offset: Int): Int {
        val document = editor.document
        return document.getLineStartOffset(document.getLineNumber(offset))
    }

    /** The tokens from the one holding [offset] on; the editor's highlighter is up to date without a commit. */
    private fun tokensFrom(editor: Editor, offset: Int): Sequence<Token> = sequence {
        if (offset >= editor.document.textLength) return@sequence
        val iterator = editor.highlighter.createIterator(offset)
        while (!iterator.atEnd()) {
            yield(Token(iterator.tokenType, iterator.start, iterator.end))
            iterator.advance()
        }
    }

    /** The tokens starting before [offset], the nearest first. */
    private fun tokensBefore(editor: Editor, offset: Int): Sequence<Token> = sequence {
        if (offset <= 0) return@sequence
        val iterator = editor.highlighter.createIterator(offset - 1)
        while (!iterator.atEnd()) {
            if (iterator.start < offset) yield(Token(iterator.tokenType, iterator.start, iterator.end))
            iterator.retreat()
        }
    }
}

/**
 * What `EnterHandler` asks for the new line's indentation, and smart
 * Backspace for the line it unindents, in place of a formatter, which
 * Newspeak has only behind the language server.
 */
class NewspeakLineIndentProvider : LineIndentProvider {
    override fun getLineIndent(project: Project, editor: Editor, language: Language?, offset: Int): String? =
        NewspeakIndentation.lineIndent(editor, offset)

    override fun isSuitableFor(language: Language?): Boolean = language == NewspeakLanguage
}

/**
 * Enter between an opener and its closer -- `= (<caret>)`, `[<caret>]`,
 * `{<caret>}` -- opens the pair onto three lines. The platform's own
 * delegate takes only `()` and `{}` and would place the closer through a
 * formatter; this one takes `[]` too, places the closer through
 * [NewspeakLineIndentProvider] and reads the tokens instead of committing
 * the document.
 */
class NewspeakEnterBetweenBracketsDelegate : EnterBetweenBracesDelegate() {
    override fun bracesAreInTheSameElement(file: PsiFile, editor: Editor, openingOffset: Int, closingOffset: Int): Boolean =
        !NewspeakIndentation.areBracketTokens(editor, openingOffset, closingOffset)

    override fun formatAtOffset(file: PsiFile, editor: Editor, offset: Int, language: Language?) {
        EnterHandler.adjustLineIndentNoCommit(language ?: NewspeakLanguage, editor.document, editor, offset)
    }

    override fun isInComment(file: PsiFile, editor: Editor, offset: Int): Boolean =
        NewspeakIndentation.isInComment(editor, offset)

    override fun isBracePair(opening: Char, closing: Char): Boolean = NewspeakIndentation.isBracketPair(opening, closing)
}

/**
 * A closer typed as the first thing on its line moves to its opener line's
 * indentation. The platform does this for `)` and `}` through a formatter
 * only, and for `]` not at all.
 */
class NewspeakClosingBracketTypedHandler : TypedHandlerDelegate() {
    override fun charTyped(character: Char, project: Project, editor: Editor, file: PsiFile): Result {
        if ((character == ')' || character == ']' || character == '}') && file.language == NewspeakLanguage) {
            NewspeakIndentation.outdentClosingBracket(editor, editor.caretModel.offset - 1)
        }
        return Result.CONTINUE
    }
}
