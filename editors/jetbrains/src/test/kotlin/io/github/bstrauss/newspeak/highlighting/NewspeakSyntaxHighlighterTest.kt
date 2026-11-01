package io.github.bstrauss.newspeak.highlighting

import com.intellij.openapi.actionSystem.IdeActions
import com.intellij.openapi.editor.ex.EditorEx
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.lexer.NewspeakHighlightingLexer
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * The doc-comment colour as the editor shows it: from the highlighter's
 * tokens, which the editor has before any daemon pass, kept up to date by
 * its incremental relex while typing.
 */
class NewspeakSyntaxHighlighterTest : BasePlatformTestCase() {
    private val highlighter = NewspeakSyntaxHighlighter()

    private data class Token(val type: String, val start: Int, val end: Int)

    /** The editor highlighter's tokens of the current text. */
    private fun editorTokens(): List<Token> {
        val iterator = (myFixture.editor as EditorEx).highlighter.createIterator(0)
        val tokens = mutableListOf<Token>()
        while (!iterator.atEnd()) {
            tokens += Token(iterator.tokenType.toString(), iterator.start, iterator.end)
            iterator.advance()
        }
        return tokens
    }

    /** The tokens of the current text lexed afresh from its start. */
    private fun freshTokens(): List<Token> {
        val text = myFixture.editor.document.charsSequence
        val lexer = NewspeakHighlightingLexer()
        lexer.start(text, 0, text.length, 0)
        val tokens = mutableListOf<Token>()
        while (true) {
            val type = lexer.tokenType ?: break
            tokens += Token(type.toString(), lexer.tokenStart, lexer.tokenEnd)
            lexer.advance()
        }
        return tokens
    }

    /** The text of every range of the editor the highlighter colours as a doc comment, in order. */
    private fun docComments(): List<String> {
        val text = myFixture.editor.document.text
        val iterator = (myFixture.editor as EditorEx).highlighter.createIterator(0)
        val ranges = mutableListOf<String>()
        while (!iterator.atEnd()) {
            if (NewspeakColors.DOC_COMMENT in highlighter.getTokenHighlights(iterator.tokenType)) {
                ranges += text.substring(iterator.start, iterator.end)
            }
            iterator.advance()
        }
        return ranges
    }

    private fun docComments(source: String): List<String> {
        myFixture.configureByText("Module.ns", source)
        return docComments()
    }

    fun testTheHighlighterAnswersTheMergingLexer() {
        assertInstanceOf(highlighter.highlightingLexer, NewspeakHighlightingLexer::class.java)
        assertSameElements(highlighter.getTokenHighlights(Types.LINE_COMMENT).toList(), NewspeakColors.COMMENT)
    }

    fun testABlockOfTwoLinesIsADocComment() {
        val source = "class M = () (\n  /// one\n  /// two\n  run = ( ^1 )\n)"
        assertEquals(listOf("/// one\n  /// two"), docComments(source))
    }

    fun testASingleLineIsNone() {
        assertEmpty(docComments("class M = () (\n  /// one\n  run = ( ^1 )\n)"))
    }

    fun testABlankLineOrCodeSplitsABlock() {
        assertEmpty(docComments("class M = () (\n  /// one\n\n  /// two\n  run = ( ^1 )\n)"))
        assertEmpty(docComments("class M = () (\n  /// one\n  run = ( ^1 )\n  /// two\n)"))
    }

    fun testATrailingCommentJoinsNoBlock() {
        val source = "class M = () (\n  run = ( ^1 ) /// remark\n  /// one\n  /// two\n  stop = ( ^2 )\n)"
        assertEquals(listOf("/// one\n  /// two"), docComments(source))
        assertEmpty(docComments("class M = () (\n  run = ( ^1 ) /// remark\n  /// one\n  stop = ( ^2 )\n)"))
    }

    fun testABlockAtTheStartOfTheFileIsOne() {
        assertEquals(listOf("/// one\n/// two"), docComments("/// one\n/// two\nclass M = () ()"))
    }

    fun testABlockCommentBesideALineIsNone() {
        assertEmpty(docComments("class M = () (\n  (* one *)\n  /// two\n  run = ( ^1 )\n)"))
    }

    // Typing: the editor relexes from a couple of tokens before an edit, which
    // must reach back to the line a new `///` line joins or leaves.

    fun testEnterUnderASingleLineMakesABlock() {
        myFixture.configureByText("Module.ns", "class M = () (\n  /// one<caret>\n  run = ( ^1 )\n)")
        myFixture.type('\n')
        myFixture.type("two")
        assertEquals(listOf("/// one\n  /// two"), docComments())
        assertEquals(freshTokens(), editorTokens())
    }

    fun testTypingTheSlashesOfASecondLineMakesABlock() {
        myFixture.configureByText("Module.ns", "class M = () (\n  /// one\n  <caret>\n  run = ( ^1 )\n)")
        myFixture.type("/// two")
        assertEquals(listOf("/// one\n  /// two"), docComments())
        assertEquals(freshTokens(), editorTokens())
    }

    fun testTypingTheSlashesOfALineAboveMakesABlock() {
        myFixture.configureByText("Module.ns", "class M = () (\n  <caret>\n  /// two\n  run = ( ^1 )\n)")
        myFixture.type("/// one")
        assertEquals(listOf("/// one\n  /// two"), docComments())
        assertEquals(freshTokens(), editorTokens())
    }

    fun testJoiningTwoLinesSplitByABlankLineMakesABlock() {
        myFixture.configureByText("Module.ns", "class M = () (\n  /// one\n<caret>\n  /// two\n  run = ( ^1 )\n)")
        myFixture.performEditorAction(IdeActions.ACTION_EDITOR_BACKSPACE)
        assertEquals(listOf("/// one\n  /// two"), docComments())
        assertEquals(freshTokens(), editorTokens())
    }

    /** The space before the line's `///` stays: the relex must not stop at it, the line's start having changed before it. */
    fun testDeletingTheCodeBeforeALineMakesABlock() {
        myFixture.configureByText("Module.ns", "class M = () (\n  x<caret> /// one\n  /// two\n  run = ( ^1 )\n)")
        myFixture.performEditorAction(IdeActions.ACTION_EDITOR_BACKSPACE)
        assertEquals(listOf("/// one\n  /// two"), docComments())
        assertEquals(freshTokens(), editorTokens())
    }

    /**
     * A remark that comes to start its line, then a line typed under it: what
     * the first edit's relex kept must still say where the second's starts.
     */
    fun testALineTypedUnderARemarkThatCameToStartItsLineMakesABlock() {
        myFixture.configureByText("Module.ns", "class M = () (\n  x<caret> /// one\n  //\n  run = ( ^1 )\n)")
        myFixture.performEditorAction(IdeActions.ACTION_EDITOR_BACKSPACE)
        myFixture.editor.caretModel.moveToOffset(myFixture.editor.document.text.indexOf("//\n") + 2)
        myFixture.type('/')
        assertEquals(listOf("/// one\n  ///"), docComments())
        assertEquals(freshTokens(), editorTokens())
    }

    fun testDeletingALineOfATwoLineBlockLeavesALineComment() {
        myFixture.configureByText("Module.ns", "class M = () (\n  /// one\n  /// t<caret>wo\n  run = ( ^1 )\n)")
        myFixture.performEditorAction(IdeActions.ACTION_EDITOR_DELETE_LINE)
        assertEmpty(docComments())
        assertEquals(1, editorTokens().count { token -> token.type == Types.LINE_COMMENT.toString() })
        assertEquals(freshTokens(), editorTokens())
    }

    fun testBreakingTheSlashesOfTheFirstLineLeavesALineComment() {
        myFixture.configureByText("Module.ns", "class M = () (\n  //<caret>/ one\n  /// two\n  run = ( ^1 )\n)")
        myFixture.performEditorAction(IdeActions.ACTION_EDITOR_BACKSPACE)
        assertEmpty(docComments())
        assertEquals(freshTokens(), editorTokens())
    }

    fun testCodeBeforeTheFirstLineLeavesALineComment() {
        myFixture.configureByText("Module.ns", "class M = () (\n  <caret>/// one\n  /// two\n  run = ( ^1 )\n)")
        myFixture.type("x ")
        assertEmpty(docComments())
        assertEquals(freshTokens(), editorTokens())
    }

    fun testCodeBeforeTheSecondLineLeavesALineComment() {
        myFixture.configureByText("Module.ns", "class M = () (\n  /// one\n  <caret>/// two\n  run = ( ^1 )\n)")
        myFixture.type("x ")
        assertEmpty(docComments())
        assertEquals(freshTokens(), editorTokens())
    }
}
