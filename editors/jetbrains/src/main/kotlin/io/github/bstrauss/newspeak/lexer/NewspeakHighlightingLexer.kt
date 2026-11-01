package io.github.bstrauss.newspeak.lexer

import com.intellij.lexer.LexerBase
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * The lexer the editor colours with: [NewspeakLexer]'s tokens, but a doc
 * comment (`NewspeakDocComments`'s block of two or more `///` lines) as one
 * [Types.DOC_COMMENT] token, from its first line's `///` to the end of its
 * last line's text.
 *
 * A line's colour depends on the lines around it, and the editor shows the
 * highlighter's colours before any daemon pass, so only a token spanning
 * the block colours it from the first paint. The parsing lexer stays one
 * token a line, since it mirrors the Rust lexer token for token.
 *
 * Whether a `///` starts its line is read from the buffer before it, never
 * from a token this lexer has seen, so a restart at any token answers what
 * a lex from the start does. But a token's type now depends on the text
 * around it, and the editor relexes an edit from a couple of tokens before
 * it, restarting and stopping only at a token in the initial state. So the
 * state says whether a token starts its line ([INITIAL]) or not
 * ([MID_LINE]): an edit just past a `///` line's line break -- the next
 * line's first `///` typed -- relexes from that `///` line, which starts
 * its line, not from the whitespace after it, which does not; and the
 * relex never stops in the middle of a line, whose start decides whether a
 * `///` further on is a remark or a doc comment's line.
 */
class NewspeakHighlightingLexer : LexerBase() {
    private val delegate = NewspeakLexer()
    private var buffer: CharSequence = ""
    private var bufferEnd = 0
    private var tokenStart = 0
    private var tokenEnd = 0
    private var currentType: IElementType? = null
    private var state = INITIAL

    /** Whether the delegate has lexed past [tokenEnd], looking for a block's next line. */
    private var delegateAhead = false

    /** The state is ignored: the line's start is read from the buffer, which is what the state records. */
    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        bufferEnd = endOffset
        delegate.start(buffer, startOffset, endOffset, NewspeakLexer.INITIAL)
        locate()
    }

    override fun getState(): Int = state
    override fun getTokenType(): IElementType? = currentType
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = bufferEnd

    override fun advance() {
        if (delegateAhead) {
            delegate.start(buffer, tokenEnd, bufferEnd, NewspeakLexer.INITIAL)
        } else {
            delegate.advance()
        }
        locate()
    }

    /** Takes the delegate's token, merged with the lines that follow it when it starts a doc comment. */
    private fun locate() {
        currentType = delegate.tokenType
        tokenStart = delegate.tokenStart
        tokenEnd = delegate.tokenEnd
        delegateAhead = false
        val startsItsLine = startsItsLine(tokenStart)
        state = if (startsItsLine) INITIAL else MID_LINE
        if (currentType != Types.LINE_COMMENT || !startsItsLine) return
        delegateAhead = true
        var lines = 1
        while (true) {
            delegate.advance()
            if (delegate.tokenType != TokenType.WHITE_SPACE || lineBreaks(delegate.tokenStart, delegate.tokenEnd) != 1) break
            delegate.advance()
            if (delegate.tokenType != Types.LINE_COMMENT) break
            tokenEnd = delegate.tokenEnd
            lines += 1
        }
        if (lines > 1) currentType = Types.DOC_COMMENT
    }

    /** Whether only whitespace stands between the start of its line, or of the text, and [offset]. */
    private fun startsItsLine(offset: Int): Boolean {
        var index = offset - 1
        while (index >= 0) {
            when (buffer[index]) {
                '\n' -> return true
                ' ', '\t', '\r', '\u000C' -> index -= 1
                else -> return false
            }
        }
        return true
    }

    private fun lineBreaks(start: Int, end: Int): Int = (start until end).count { index -> buffer[index] == '\n' }

    companion object {
        /** A token with only whitespace before it on its line, or at the text's start: where the editor may relex from. */
        const val INITIAL = 0

        /** A token after something else on its line. */
        const val MID_LINE = 1
    }
}
