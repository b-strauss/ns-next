package io.github.bstrauss.newspeak.editor

import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.settings.NewspeakSettings

class NewspeakOneLineMethodFoldingTest : BasePlatformTestCase() {
    /** What each fold of `source` shows, in order: the text it hides, then whether it opens collapsed. */
    private fun folds(source: String): List<Pair<String, Boolean>> {
        val file = myFixture.configureByText("Module.ns", source)
        return NewspeakOneLineMethodFolding()
            .buildFoldRegions(file, myFixture.editor.document, false)
            .map { descriptor ->
                assertEquals(" ", descriptor.placeholderText)
                descriptor.range.substring(source) to (descriptor.isCollapsedByDefault() ?: false)
            }
    }

    private val oneLine = """
        class M = () (
          public size = (
            ^size_.
          )
        )
    """.trimIndent()

    fun testAOneLineMethodFoldsOntoItsHeadersLine() {
        assertEquals(listOf("\n    " to true, "\n  " to true), folds(oneLine))
    }

    fun testItsTwoFoldsOpenTogether() {
        val file = myFixture.configureByText("Module.ns", oneLine)
        val descriptors = NewspeakOneLineMethodFolding().buildFoldRegions(file, myFixture.editor.document, false)
        assertNotNull(descriptors[0].group)
        assertSame(descriptors[0].group, descriptors[1].group)
    }

    fun testTheSettingDecidesWhetherItOpensFolded() {
        val state = NewspeakSettings.getInstance().state
        state.collapseOneLineMethods = false
        try {
            assertEquals(listOf("\n    " to false, "\n  " to false), folds(oneLine))
        } finally {
            state.collapseOneLineMethods = true
        }
    }

    fun testABodyOfTwoLinesDoesNotFold() {
        assertEmpty(folds("class M = () (\n  foo = (\n    a.\n    b.\n  )\n)"))
    }

    fun testABodyWithALineCommentOrOnlyACommentDoesNotFold() {
        assertEmpty(folds("class M = () (\n  foo = (\n    ^a. /// why\n  )\n)"))
        assertEmpty(folds("class M = () (\n  foo = (\n    (* nothing *)\n  )\n)"))
    }

    fun testAClassHeaderOfOneLineIsNoMethod() {
        assertEmpty(folds("class M = (\n  x foo.\n) ()"))
    }

    fun testABodyStartingOnTheHeadersLineIsLeftAlone() {
        assertEmpty(folds("class M = () (\n  foo = ( ^a.\n  )\n)"))
    }

    fun testAMethodTooLongFoldedIsLeftOpen() {
        val statement = "^" + "a".repeat(86) + "."
        // `  foo = ( ` is 10 columns and ` )` 2: a statement of 88 fits, one of 89 does not.
        assertEquals(2, folds("class M = () (\n  foo = (\n    $statement\n  )\n)").size)
        assertEmpty(folds("class M = () (\n  foo = (\n    ${statement}a\n  )\n)"))
    }

    fun testAnUnclosedBracketOnTheBodyLineIsNoOneLineBody() {
        assertEmpty(folds("class M = () (\n  foo = (\n    ^(a\n  )\n)"))
    }

    fun testCarriageReturnLineFeedsAreLineBreaks() {
        assertEquals(2, folds(oneLine.replace("\n", "\r\n")).size)
    }
}
