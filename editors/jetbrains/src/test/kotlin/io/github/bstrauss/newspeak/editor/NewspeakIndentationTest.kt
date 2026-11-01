package io.github.bstrauss.newspeak.editor

import com.intellij.application.options.CodeStyle
import com.intellij.openapi.actionSystem.IdeActions
import com.intellij.psi.codeStyle.CodeStyleSettingsManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase

class NewspeakIndentationTest : BasePlatformTestCase() {
    private fun typing(source: String, typed: Char, expected: String) {
        myFixture.configureByText("Module.ns", source)
        myFixture.type(typed)
        myFixture.checkResult(expected)
    }

    private fun enter(source: String, expected: String) = typing(source, '\n', expected)

    // Enter after an opener: the opener's line + 2.

    fun testEnterAfterAParenthesisIndentsByTwo() {
        enter(
            """
            class M = () (
              foo = (<caret>
            )
            """.trimIndent(),
            """
            class M = () (
              foo = (
                <caret>
            )
            """.trimIndent(),
        )
    }

    fun testEnterAfterABracketIndentsByTwo() {
        enter("    x ifTrue: [<caret>", "    x ifTrue: [\n      <caret>")
    }

    fun testEnterAfterABraceIndentsByTwo() {
        enter("  ^{<caret>", "  ^{\n    <caret>")
    }

    fun testEnterAfterABlocksParametersIndentsByTwo() {
        enter("    list do: [:each |<caret>", "    list do: [:each |\n      <caret>")
        enter("    dict keysAndValuesDo: [:key :value |<caret>", "    dict keysAndValuesDo: [:key :value |\n      <caret>")
        enter("    list do: [:each <Integer> |<caret>", "    list do: [:each <Integer> |\n      <caret>")
    }

    fun testACommentAfterTheOpenerIsIgnored() {
        enter("  foo = ( /// why<caret>", "  foo = ( /// why\n    <caret>")
        enter("  foo = ( (* why *)<caret>", "  foo = ( (* why *)\n    <caret>")
    }

    fun testABarOfTemporariesIsNoBlocksParameters() {
        enter("    [:each | | sum |<caret>", "    [:each | | sum |\n    <caret>")
        enter("  | sum |<caret>", "  | sum |\n  <caret>")
    }

    fun testAnOpenerInsideAStringOrCommentIsNoOpener() {
        enter("  x:: '(' .<caret>", "  x:: '(' .\n  <caret>")
        enter("  x. /// (<caret>", "  x. /// (\n  <caret>")
    }

    // Enter between an opener and its closer: three lines.

    fun testEnterBetweenParenthesesOpensThePair() {
        enter(
            """
            class M = () (
              foo = (<caret>)
            )
            """.trimIndent(),
            """
            class M = () (
              foo = (
                <caret>
              )
            )
            """.trimIndent(),
        )
    }

    fun testEnterBetweenBracketsOpensThePair() {
        enter("    x ifTrue: [<caret>]", "    x ifTrue: [\n      <caret>\n    ]")
    }

    fun testEnterBetweenBracesOpensThePair() {
        enter("  ^{<caret>}", "  ^{\n    <caret>\n  }")
    }

    fun testEnterBetweenABlocksParametersAndItsBracketOpensTheBlock() {
        enter("    list do: [:each |<caret>]", "    list do: [:each |\n      <caret>\n    ]")
    }

    // A closer typed first on its line.

    fun testATypedClosingParenthesisMovesToItsOpenersLine() {
        typing(
            """
            class M = () (
              foo = (
                ^1.
                <caret>
            """.trimIndent(),
            ')',
            """
            class M = () (
              foo = (
                ^1.
              )<caret>
            """.trimIndent(),
        )
    }

    fun testATypedClosingBracketOrBraceMovesToItsOpenersLine() {
        typing("    x ifTrue: [\n      y.\n      <caret>", ']', "    x ifTrue: [\n      y.\n    ]<caret>")
        typing("  ^{\n    a.\n    <caret>", '}', "  ^{\n    a.\n  }<caret>")
    }

    fun testAClosingBracketAfterCodeIsLeftWhereItIs() {
        typing("  foo = (\n    x foo: (y<caret>", ')', "  foo = (\n    x foo: (y)<caret>")
    }

    fun testAClosingBracketInAStringIsLeftWhereItIs() {
        typing("  x:: '\n      <caret>'", ')', "  x:: '\n      )<caret>'")
    }

    // Any other line keeps the indentation before it.

    fun testEnterAfterAStatementKeepsItsIndentation() {
        enter("  foo = (\n    x foo.<caret>", "  foo = (\n    x foo.\n    <caret>")
    }

    fun testEnterAfterAClosedMethodKeepsItsIndentation() {
        enter("  foo = (\n    x.\n  )<caret>", "  foo = (\n    x.\n  )\n  <caret>")
    }

    fun testABlankLineIsSkipped() {
        enter("    x foo.\n\n<caret>", "    x foo.\n\n\n    <caret>")
    }

    fun testALineContinuingAStringIsMeasuredFromItsFirstLine() {
        enter("    x:: 'one\ntwo'.<caret>", "    x:: 'one\ntwo'.\n    <caret>")
    }

    fun testEnterInsideAStringIsNotReindented() {
        enter("    x:: 'one<caret>two'.", "    x:: 'one\n    <caret>two'.")
        enter("    x:: 'one\n<caret>two'.", "    x:: 'one\n\n<caret>two'.")
    }

    // The indentation itself: two spaces, never a tab, whatever the settings say.

    fun testTabInsertsTwoSpaces() {
        myFixture.configureByText("Module.ns", "<caret>x")
        myFixture.performEditorAction(IdeActions.ACTION_EDITOR_TAB)
        myFixture.checkResult("  <caret>x")
    }

    fun testTheUsersIndentationSettingsDoNotReachANewspeakFile() {
        CodeStyle.doWithTemporarySettings(project, CodeStyle.getSettings(project)) { settings ->
            settings.OTHER_INDENT_OPTIONS.INDENT_SIZE = 8
            settings.OTHER_INDENT_OPTIONS.TAB_SIZE = 8
            settings.OTHER_INDENT_OPTIONS.USE_TAB_CHARACTER = true
            myFixture.configureByText("Module.ns", "<caret>x")
            myFixture.performEditorAction(IdeActions.ACTION_EDITOR_TAB)
            myFixture.checkResult("  <caret>x")
            val options = CodeStyle.getIndentOptions(myFixture.file)
            assertEquals(2, options.INDENT_SIZE)
            assertEquals(2, options.TAB_SIZE)
            assertEquals(4, options.CONTINUATION_INDENT_SIZE)
            assertFalse(options.USE_TAB_CHARACTER)
        }
    }

    fun testBackspaceInTheIndentationRemovesTwoSpaces() {
        myFixture.configureByText("Module.ns", "  foo = (\n      <caret>x.\n  )")
        myFixture.performEditorAction(IdeActions.ACTION_EDITOR_BACKSPACE)
        myFixture.checkResult("  foo = (\n    <caret>x.\n  )")
    }

    /** A doc comment is one token of the highlighter's, but each of its lines is indented as a `///` line is. */
    fun testBackspaceInTheIndentationOfADocCommentsLineRemovesTwoSpaces() {
        myFixture.configureByText("Module.ns", "  foo = (\n    /// one\n        <caret>/// two\n    x.\n  )")
        myFixture.performEditorAction(IdeActions.ACTION_EDITOR_BACKSPACE)
        myFixture.checkResult("  foo = (\n    /// one\n    <caret>/// two\n    x.\n  )")
    }

    fun testEnterAfterADocCommentKeepsTheIndentationOfItsLastLine() {
        enter("    /// one\n      /// two<caret>", "    /// one\n      /// two\n      /// <caret>")
    }

    // The width.

    /**
     * The test framework hands every file temporary settings, which no
     * `CodeStyleSettingsModifier` reaches; the IDE hands a file its project's
     * settings as modified, so the test drops them while it looks.
     */
    fun testTheRightMarginIsTheFormattersWidth() {
        val manager = CodeStyleSettingsManager.getInstance(project)
        val temporary = manager.temporarySettings
        manager.dropTemporarySettings()
        try {
            assertEquals(120, CodeStyle.getSettings(project).defaultRightMargin)
            myFixture.configureByText("Module.ns", "x")
            assertEquals(100, myFixture.editor.settings.getRightMargin(project))
            assertEquals(2, CodeStyle.getIndentOptions(myFixture.file).INDENT_SIZE)
        } finally {
            temporary?.let(manager::setTemporarySettings)
        }
    }
}
