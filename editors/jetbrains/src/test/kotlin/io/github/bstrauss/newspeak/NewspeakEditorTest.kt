package io.github.bstrauss.newspeak

import com.intellij.codeInsight.highlighting.BraceMatchingUtil
import com.intellij.openapi.actionSystem.IdeActions
import com.intellij.openapi.fileTypes.FileTypeManager
import com.intellij.psi.search.PsiTodoSearchHelper
import com.intellij.spellchecker.tokenizer.LanguageSpellchecking
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.editor.NewspeakSpellcheckingStrategy

class NewspeakEditorTest : BasePlatformTestCase() {
    fun testNsFilesAreNewspeak() {
        assertSame(NewspeakFileType, FileTypeManager.getInstance().getFileTypeByFileName("Module.ns"))
    }

    fun testTheFileParsesIntoANewspeakFile() {
        val file = myFixture.configureByText("Module.ns", "class M = Object new () ()")
        assertInstanceOf(file, NewspeakFile::class.java)
    }

    fun testParenthesesMatch() {
        myFixture.configureByText("Module.ns", "f = <caret>( ^[ 1 ] )")
        val editor = myFixture.editor
        val offset = editor.caretModel.offset
        val iterator = (editor as com.intellij.openapi.editor.ex.EditorEx).highlighter.createIterator(offset)
        assertTrue(BraceMatchingUtil.matchBrace(editor.document.charsSequence, myFixture.file.fileType, iterator, true))
        assertEquals(editor.document.text.lastIndexOf(')'), iterator.start)
    }

    /**
     * The selection sits inside a line, not spanning it: `CommentByBlockCommentHandler`
     * (`commentRange`) special-cases a selection whose start and end each sit at a line
     * boundary -- true here for `<selection>x</selection>` alone, which fills the whole,
     * newline-less buffer -- by wrapping it as an indented, multi-line block instead of
     * inline, for every language. That is the platform's own behaviour, not this plugin's.
     */
    fun testBlockCommentWrapsTheSelection() {
        myFixture.configureByText("Module.ns", "x <selection>y</selection> z")
        myFixture.performEditorAction(IdeActions.ACTION_COMMENT_BLOCK)
        myFixture.checkResult("x (*y*) z")
    }

    fun testTodoInACommentIsFound() {
        myFixture.configureByText("Module.ns", "(* TODO: finish *) x")
        assertEquals(1, PsiTodoSearchHelper.getInstance(project).findTodoItems(myFixture.file).size)
    }

    /**
     * Pins the registration itself, not just the strategy's own logic: the
     * `spellchecker.support` entry moved into the optional `newspeak-spellchecker.xml`
     * (see `plugin.xml`'s `<depends optional="true" ...>`), and only a test against the
     * live extension point catches that move silently dropping it.
     */
    fun testSpellCheckingIsRegisteredForNewspeak() {
        val strategies = LanguageSpellchecking.INSTANCE.allForLanguage(NewspeakLanguage)
        assertTrue(strategies.any { strategy -> strategy is NewspeakSpellcheckingStrategy })
    }
}
