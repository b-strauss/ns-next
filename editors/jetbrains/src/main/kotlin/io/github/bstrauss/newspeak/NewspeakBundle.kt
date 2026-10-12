package io.github.bstrauss.newspeak

import com.intellij.DynamicBundle
import org.jetbrains.annotations.Nls
import org.jetbrains.annotations.PropertyKey
import java.util.function.Supplier

private const val BUNDLE = "messages.NewspeakBundle"

/** Every text the plugin shows, in `messages/NewspeakBundle.properties`. */
object NewspeakBundle : DynamicBundle(BUNDLE) {
    fun message(@PropertyKey(resourceBundle = BUNDLE) key: String, vararg parameters: Any): @Nls String =
        getMessage(key, *parameters)

    /** For a text read later, after the IDE's language may have changed (a color settings page's attribute names). */
    fun lazyMessage(@PropertyKey(resourceBundle = BUNDLE) key: String, vararg parameters: Any): Supplier<@Nls String> =
        getLazyMessage(key, *parameters)
}
