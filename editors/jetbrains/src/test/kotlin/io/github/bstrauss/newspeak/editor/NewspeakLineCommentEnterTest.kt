package io.github.bstrauss.newspeak.editor

import com.intellij.testFramework.fixtures.BasePlatformTestCase

class NewspeakLineCommentEnterTest : BasePlatformTestCase() {
    private fun enter(source: String, expected: String) {
        myFixture.configureByText("Module.ns", source)
        myFixture.type('\n')
        myFixture.checkResult(expected)
    }

    fun testEnterAtTheEndOfALineCommentContinuesIt() {
        enter(
            """
            class M = () (
              /// Answers the size.<caret>
              size = (
            """.trimIndent(),
            """
            class M = () (
              /// Answers the size.
              /// <caret>
              size = (
            """.trimIndent(),
        )
    }

    // A doc comment is one token of the highlighter's; Enter continues the
    // caret's own line of it, wherever in the block that is.

    fun testEnterAtTheEndOfTheFirstLineOfABlockContinuesIt() {
        enter(
            "class M = () (\n  /// one<caret>\n  /// two\n  size = (",
            "class M = () (\n  /// one\n  /// <caret>\n  /// two\n  size = (",
        )
    }

    fun testEnterAtTheEndOfABlockContinuesIt() {
        enter(
            "class M = () (\n  /// one\n  /// two<caret>\n  size = (",
            "class M = () (\n  /// one\n  /// two\n  /// <caret>\n  size = (",
        )
    }

    fun testEnterInTheMiddleOfABlockContinuesItsLine() {
        enter(
            "class M = () (\n  /// one\n  ///   - two <caret>three\n  /// four\n  size = (",
            "class M = () (\n  /// one\n  ///   - two\n  ///   <caret>three\n  /// four\n  size = (",
        )
    }

    fun testEnterInABlockInABodyStaysAtItsIndentation() {
        enter(
            "  foo = (\n    /// one\n    /// two<caret>\n    ^1",
            "  foo = (\n    /// one\n    /// two\n    /// <caret>\n    ^1",
        )
    }

    fun testEnterBeforeTheSlashesOfABlocksSecondLineIsAPlainEnter() {
        enter(
            "class M = () (\n  /// one\n  <caret>/// two",
            "class M = () (\n  /// one\n  \n  <caret>/// two",
        )
    }

    fun testEnterInsideALineCommentCarriesTheRestOn() {
        enter("  /// Answers the <caret>size.", "  /// Answers the\n  /// <caret>size.")
    }

    fun testABareCommentContinuesWithOneSpace() {
        enter("  ///<caret>", "  ///\n  /// <caret>")
    }

    fun testTheCommentsOwnSpacingIsKept() {
        enter("  ///   - an item<caret>", "  ///   - an item\n  ///   <caret>")
    }

    fun testACommentInAMethodBodyStaysAtItsIndentation() {
        enter("  foo = (\n    /// why<caret>", "  foo = (\n    /// why\n    /// <caret>")
    }

    fun testATrailingCommentEndsWithItsLine() {
        enter("    a foo. /// why<caret>", "    a foo. /// why\n    <caret>")
    }

    fun testATrailingCommentSplitKeepsItsTextAComment() {
        enter("    a foo. /// why <caret>not", "    a foo. /// why\n    /// <caret>not")
    }

    fun testEnterBeforeTheSlashesIsAPlainEnter() {
        enter("class M = () (\n  <caret>/// why", "class M = () (\n  \n  <caret>/// why")
    }

    fun testABlockCommentIsNotContinued() {
        enter("  (* why *)<caret>", "  (* why *)\n  <caret>")
    }
}
