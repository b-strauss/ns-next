package io.github.bstrauss.newspeak.highlighting

import com.intellij.testFramework.fixtures.BasePlatformTestCase

class NewspeakDocCommentAnnotatorTest : BasePlatformTestCase() {
    /** The text of every `///` line `source` colours as a doc comment, in order. */
    private fun docComments(source: String): List<String> {
        myFixture.configureByText("Module.ns", source)
        return myFixture.doHighlighting()
            .filter { it.forcedTextAttributesKey == NewspeakColors.DOC_COMMENT }
            .sortedBy { it.startOffset }
            .map { source.substring(it.startOffset, it.endOffset) }
    }

    fun testABlockOfTwoLinesIsADocComment() {
        val source = "class M = () (\n  /// one\n  /// two\n  run = ( ^1 )\n)"
        assertEquals(listOf("/// one", "/// two"), docComments(source))
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
        assertEquals(listOf("/// one", "/// two"), docComments(source))
        assertEmpty(docComments("class M = () (\n  run = ( ^1 ) /// remark\n  /// one\n  stop = ( ^2 )\n)"))
    }

    fun testABlockAtTheStartOfTheFileIsOne() {
        assertEquals(listOf("/// one", "/// two"), docComments("/// one\n/// two\nclass M = () ()"))
    }

    fun testABlockCommentBesideALineIsNone() {
        assertEmpty(docComments("class M = () (\n  (* one *)\n  /// two\n  run = ( ^1 )\n)"))
    }
}
