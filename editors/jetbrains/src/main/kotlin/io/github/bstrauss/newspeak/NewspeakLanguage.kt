package io.github.bstrauss.newspeak

import com.intellij.lang.Language

/** The plugin's ID, as `plugin.xml` and the Marketplace know it. Permanent once published. */
const val PLUGIN_ID = "io.github.bstrauss.newspeak"

object NewspeakLanguage : Language("Newspeak") {
    private fun readResolve(): Any = NewspeakLanguage
}
