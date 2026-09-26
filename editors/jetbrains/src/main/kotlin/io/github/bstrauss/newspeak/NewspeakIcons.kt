package io.github.bstrauss.newspeak

import com.intellij.openapi.util.IconLoader
import javax.swing.Icon

object NewspeakIcons {
    /** `newspeak_dark.svg` is picked up for dark themes by the naming convention. */
    @JvmField
    val FILE: Icon = IconLoader.getIcon("/icons/newspeak.svg", NewspeakIcons::class.java)
}
