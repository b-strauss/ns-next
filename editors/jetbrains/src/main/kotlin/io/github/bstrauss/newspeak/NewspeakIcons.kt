package io.github.bstrauss.newspeak

import com.intellij.openapi.util.IconLoader
import javax.swing.Icon

object NewspeakIcons {
    /** `newspeak_dark.svg` is picked up for dark themes by the naming convention. */
    @JvmField
    val FILE: Icon = IconLoader.getIcon("/icons/newspeak.svg", NewspeakIcons::class.java)

    /**
     * The module's one top-level class in the structure view;
     * `root_class_dark.svg` for dark themes.
     */
    @JvmField
    val ROOT_CLASS: Icon = IconLoader.getIcon("/icons/nodes/root_class.svg", NewspeakIcons::class.java)

    /**
     * A nested value class in the structure view; `value_class_dark.svg` for
     * dark themes.
     */
    @JvmField
    val VALUE_CLASS: Icon = IconLoader.getIcon("/icons/nodes/value_class.svg", NewspeakIcons::class.java)
}
