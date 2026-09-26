package io.github.bstrauss.newspeak

import com.intellij.lang.Language

object NewspeakLanguage : Language("Newspeak") {
    private fun readResolve(): Any = NewspeakLanguage
}
