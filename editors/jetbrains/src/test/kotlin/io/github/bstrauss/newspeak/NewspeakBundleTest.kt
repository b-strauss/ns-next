package io.github.bstrauss.newspeak

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class NewspeakBundleTest {
    @Test fun aMessageTakesItsParameter() {
        assertEquals(
            "The configured language server does not exist: /opt/ns-next-analyzer",
            NewspeakBundle.message("notification.server.missing.configured.path", "/opt/ns-next-analyzer"),
        )
    }

    @Test fun anApostropheInAMessageWithParametersIsShownOnce() {
        val text = NewspeakBundle.message("run.marker.tooltip.script", "bench")
        assertEquals("Run script 'bench'", text)
        assertFalse(text.contains("''"))
    }

    @Test fun anApostropheInAMessageWithoutParametersIsShownAsWritten() {
        assertEquals(
            "Leave empty to use the ns-next-analyzer bundled with the plugin. Point it at a locally built " +
                "target/debug/ns-next-analyzer to see changes to ns-next's parser or resolver at once.",
            NewspeakBundle.message("settings.server.path.comment"),
        )
    }

    @Test fun aLazyMessageResolvesLikeAMessage() {
        assertEquals(
            NewspeakBundle.message("color.settings.literals.number"),
            NewspeakBundle.lazyMessage("color.settings.literals.number").get(),
        )
    }
}
