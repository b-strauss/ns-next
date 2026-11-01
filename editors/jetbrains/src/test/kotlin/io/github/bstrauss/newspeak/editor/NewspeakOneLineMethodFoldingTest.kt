package io.github.bstrauss.newspeak.editor

import com.intellij.lang.folding.LanguageFolding
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import io.github.bstrauss.newspeak.NewspeakLanguage
import io.github.bstrauss.newspeak.settings.NewspeakSettings

class NewspeakOneLineMethodFoldingTest : BasePlatformTestCase() {
    /** What each fold of `source` does, in order: the text it hides, then the text it shows; each opens collapsed. */
    private fun folds(source: String): List<Pair<String, String>> {
        val file = myFixture.configureByText("Module.ns", source)
        return NewspeakOneLineMethodFolding()
            .buildFoldRegions(file, myFixture.editor.document, false)
            .map { descriptor ->
                assertEquals(true, descriptor.isCollapsedByDefault())
                descriptor.range.substring(source) to descriptor.placeholderText!!
            }
    }

    private val oneLine = """
        class M = () (
          public size = (
            ^size_.
          )
        )
    """.trimIndent()

    fun testAOneLineMethodFoldsOntoItsHeadersLineAsJavasDoes() {
        assertEquals(listOf(" (\n    " to " ( ", "\n  )" to " )"), folds(oneLine))
    }

    fun testWithoutASpaceBeforeItsBracketTheFirstFoldStartsAtTheBracket() {
        assertEquals(
            listOf("(\n    " to "( ", "\n  )" to " )"),
            folds("class M = () (\n  public size =(\n    ^size_.\n  )\n)"),
        )
    }

    fun testItsTwoFoldsOpenTogether() {
        val file = myFixture.configureByText("Module.ns", oneLine)
        val descriptors = NewspeakOneLineMethodFolding().buildFoldRegions(file, myFixture.editor.document, false)
        assertNotNull(descriptors[0].group)
        assertSame(descriptors[0].group, descriptors[1].group)
    }

    fun testWithTheSettingOffThereAreNoFolds() {
        val state = NewspeakSettings.getInstance().state
        state.collapseOneLineMethods = false
        try {
            assertEmpty(folds(oneLine))
        } finally {
            state.collapseOneLineMethods = true
        }
    }

    fun testTheBuilderComesBeforeTheServersSoItsFoldsWin() {
        assertInstanceOf(
            LanguageFolding.INSTANCE.allForLanguage(NewspeakLanguage).first(),
            NewspeakOneLineMethodFolding::class.java,
        )
    }

    /** What the registration's order rests on: the server's fold of the body, `(` to `)`, is refused after them. */
    fun testThePlatformRefusesTheServersFoldOfTheBodyAfterThem() {
        val file = myFixture.configureByText("Module.ns", oneLine)
        val descriptors = NewspeakOneLineMethodFolding().buildFoldRegions(file, myFixture.editor.document, false)
        val opening = oneLine.indexOf("size = (") + "size = ".length
        val closing = oneLine.indexOf("  )") + 3
        val model = myFixture.editor.foldingModel
        model.runBatchFoldingOperation {
            for (descriptor in descriptors) {
                val range = descriptor.range
                assertNotNull(model.addFoldRegion(range.startOffset, range.endOffset, descriptor.placeholderText!!))
            }
            assertNull(model.addFoldRegion(opening, closing, "(...)"))
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
