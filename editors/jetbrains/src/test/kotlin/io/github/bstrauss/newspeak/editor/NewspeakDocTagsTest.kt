package io.github.bstrauss.newspeak.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The grammar of `crates/ns-syntax/src/doc_comment.rs`, case by case: each
 * test is named as the Rust test of the same case. The Rust parser's spans
 * have no counterpart here, so its span case is not repeated.
 */
class NewspeakDocTagsTest {
    private fun parsed(vararg lines: String): DocComment = NewspeakDocTags.parse(lines.joinToString("\n"))

    @Test fun aCommentWithoutTagsIsAllDescription() {
        val comment = parsed("Answers the size", "of the receiver.")
        assertEquals("Answers the size\nof the receiver.", comment.description)
        assertTrue(comment.tags.isEmpty())
    }

    @Test fun aFactoryTagHasNoArgument() {
        val comment = parsed("A point.", "@factory makes one.")
        assertEquals("A point.", comment.description)
        assertEquals(listOf(DocTag("factory", null, "makes one.")), comment.tags)
    }

    @Test fun tagsSplitTheDescriptionOff() {
        val comment = parsed(
            "Evaluates.",
            "",
            "@param stop a number.",
            "@param action a block,",
            "sent value:.",
            "@answers nil.",
            "@signals ArgumentError when stop is no number.",
            "@see `to:by:do:`",
        )
        assertEquals("Evaluates.", comment.description)
        assertEquals(
            listOf(
                DocTag("param", "stop", "a number."),
                DocTag("param", "action", "a block,\nsent value:."),
                DocTag("answers", null, "nil."),
                DocTag("signals", "ArgumentError", "when stop is no number."),
                DocTag("see", null, "`to:by:do:`"),
            ),
            comment.tags,
        )
    }

    @Test fun anUnknownTagIsKeptInOrder() {
        val comment = parsed("Answers.", "@param value a number.", "@retrun nil.", "@see `other`")
        assertEquals(
            listOf(DocTag("param", "value", "a number."), DocTag("retrun", null, "nil."), DocTag("see", null, "`other`")),
            comment.tags,
        )
        assertEquals("Answers.\n\n@retrun nil.", comment.description)
    }

    @Test fun aParamWithoutANameHasNoArgument() {
        val comment = parsed("Answers.", "@param", "@answers nil.")
        assertEquals(listOf(DocTag("param", null, ""), DocTag("answers", null, "nil.")), comment.tags)
    }

    @Test fun anAtSignInsideALineIsText() {
        val comment = parsed("Answers.", "mail me @home")
        assertEquals("Answers.\nmail me @home", comment.description)
        assertTrue(comment.tags.isEmpty())
    }

    @Test fun aTagLineInsideACodeFenceIsText() {
        val comment = parsed("Documents a method:", "```", "@param x a number.", "```", "@answers nil.")
        assertEquals("Documents a method:\n```\n@param x a number.\n```", comment.description)
        assertEquals(listOf(DocTag("answers", null, "nil.")), comment.tags)
    }

    @Test fun theSummaryIsTheFirstSentence() {
        assertEquals("Answers the size.", parsed("Answers the size. Never nil.", "@answers an integer.").summary)
        assertEquals(
            "Answers the size of the receiver",
            parsed("Answers   the size", "of the receiver", "", "More. Even more.").summary,
        )
        assertNull(parsed("@answers nil.", "@see `other`").summary)
    }

    /** Whitespace is Unicode's, as Rust's `split_whitespace` reads it: a no-break space collapses too. */
    @Test fun theSummaryCollapsesUnicodeWhitespace() {
        assertEquals("Answers the size.", parsed("Answers\u00a0\u2003the size.").summary)
    }

    @Test fun aFirstSentenceDoesNotEndInsideCode() {
        assertEquals("Sends `a.b` first.", parsed("Sends `a.b` first. Then more.", "@answers nil.").summary)
    }

    @Test fun anInlineSummaryWinsAndReadsAsItsText() {
        val comment = parsed("{@summary The size.} Answers the size of the receiver.", "@answers an integer.")
        assertEquals("The size.", comment.summary)
        assertEquals("The size. Answers the size of the receiver.", comment.description)
    }

    @Test fun aSummaryEndsAtItsClosingBraceOutsideCode() {
        val comment = parsed("{@summary Uses `{x}` here.} More.", "@answers nil.")
        assertEquals("Uses `{x}` here.", comment.summary)
        assertEquals("Uses `{x}` here. More.", comment.description)
    }

    /** The table the server's hover writes, here from the platform's markup, each item rendered. */
    @Test fun theKnownTagsBecomeSectionsInTheirOrder() {
        val comment = parsed(
            "Evaluates.",
            "@see `other`",
            "@param stop a number.",
            "@retrun nil.",
            "@signals ArgumentError",
        )
        val html = NewspeakDocTags.sectionsHtml(comment) { markdown -> "[$markdown]" }
        assertEquals(
            "<table class='sections'>" +
                "<tr><td valign='top' class='section'><p>Params:</td><td valign='top'>[`stop` – a number.]</td>" +
                "<tr><td valign='top' class='section'><p>Signals:</td><td valign='top'>[`ArgumentError`]</td>" +
                "<tr><td valign='top' class='section'><p>See Also:</td><td valign='top'>[`other`]</td>" +
                "</table>",
            html,
        )
    }

    @Test fun withoutKnownTagsThereAreNoSections() {
        assertEquals("", NewspeakDocTags.sectionsHtml(parsed("Answers.", "@retrun nil.")) { it })
    }
}
