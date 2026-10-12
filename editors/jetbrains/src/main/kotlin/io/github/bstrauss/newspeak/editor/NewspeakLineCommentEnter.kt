package io.github.bstrauss.newspeak.editor

import com.intellij.codeInsight.editorActions.enter.EnterHandlerDelegate
import com.intellij.codeInsight.editorActions.enter.EnterHandlerDelegate.Result
import com.intellij.openapi.actionSystem.DataContext
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.actionSystem.EditorActionHandler
import com.intellij.openapi.util.Ref
import com.intellij.psi.PsiFile
import io.github.bstrauss.newspeak.NewspeakLanguage
import io.github.bstrauss.newspeak.lexer.NewspeakTokenTypes as Types

/**
 * Enter inside a `///` comment continues it on the next line with `///`
 * and the comment's own spacing, at the end of the line as well as in its
 * middle: `///` is the comment form ns-next's sources are written in
 * (`CLAUDE.md`), and its comments run over several lines, as Rust's `///`
 * do, where Enter continues them too. The platform's own handler continues
 * a line comment only when text follows the caret, Java's choice for `//`.
 *
 * A `///` after code on its line continues only when Enter splits its text,
 * which would otherwise fall out of the comment into code; at its end Enter
 * starts a plain line, the remark being on that line alone.
 */
class NewspeakLineCommentEnter : EnterHandlerDelegate {
    override fun preprocessEnter(
        file: PsiFile,
        editor: Editor,
        caretOffset: Ref<Int>,
        caretAdvance: Ref<Int>,
        dataContext: DataContext,
        originalHandler: EditorActionHandler?,
    ): Result {
        if (file.language != NewspeakLanguage) return Result.Continue
        val offset = caretOffset.get()
        if (offset < PREFIX.length) return Result.Continue
        val iterator = editor.highlighter.createIterator(offset - 1)
        if (iterator.atEnd() || iterator.tokenType != Types.LINE_COMMENT) return Result.Continue
        val commentStart = iterator.start
        if (offset < commentStart + PREFIX.length) return Result.Continue

        val document = editor.document
        val text = document.charsSequence
        val lineStart = document.getLineStartOffset(document.getLineNumber(offset))
        val startsItsLine = (lineStart until commentStart).all { index -> text[index] == ' ' || text[index] == '\t' }
        val textAfter = skipSpaces(text, offset)
        val atLineEnd = textAfter >= text.length || text[textAfter] == '\n'
        if (!startsItsLine && atLineEnd) return Result.Continue

        // The spacing after this line's `///`, so that an indented list item or
        // code sample in the comment goes on at its column; a bare `///` takes one space.
        val spacingStart = commentStart + PREFIX.length
        val spacing = text.subSequence(spacingStart, skipSpaces(text, spacingStart).coerceAtMost(offset))
            .toString()
            .ifEmpty { " " }
        // The spaces around the split belong to neither line: the next line's
        // text starts after its own spacing, and this one ends without any.
        var splitStart = offset
        while (splitStart > spacingStart && (text[splitStart - 1] == ' ' || text[splitStart - 1] == '\t')) splitStart--
        document.deleteString(splitStart, textAfter)
        document.insertString(splitStart, PREFIX + spacing)
        caretOffset.set(splitStart)
        caretAdvance.set(PREFIX.length + spacing.length)
        return Result.DefaultForceIndent
    }

    private fun skipSpaces(text: CharSequence, from: Int): Int {
        var index = from
        while (index < text.length && (text[index] == ' ' || text[index] == '\t')) index++
        return index
    }

    private companion object {
        const val PREFIX = "///"
    }
}
