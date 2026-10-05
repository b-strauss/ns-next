package io.github.bstrauss.newspeak.run

import com.intellij.json.psi.JsonFile
import com.intellij.json.psi.JsonObject
import com.intellij.json.psi.JsonProperty
import com.intellij.psi.PsiElement

/** What a play button beside a key of a `newspeak.json` runs: the project without a script, or one script. */
sealed interface ProjectEntry {
    data object Main : ProjectEntry

    data class Script(val name: String) : ProjectEntry
}

/**
 * The entry `element` -- the first leaf of a property's name -- stands for:
 * the top-level `main`, or a key of the top-level `scripts` object; null for
 * anything else, a key of the same name nested elsewhere included.
 */
fun projectEntryAt(element: PsiElement): ProjectEntry? {
    val file = element.containingFile as? JsonFile ?: return null
    if (file.name != PROJECT_FILE_NAME) return null
    // The JSON plugin's shape: the quoted leaf, inside the JsonStringLiteral that is the property's name.
    val property = element.parent?.parent as? JsonProperty ?: return null
    if (property.nameElement != element.parent) return null
    val owner = property.parent as? JsonObject ?: return null
    if (owner == file.topLevelValue) {
        return if (property.name == "main") ProjectEntry.Main else null
    }
    val scripts = owner.parent as? JsonProperty ?: return null
    val isTopLevelScripts = scripts.name == "scripts" && scripts.parent == file.topLevelValue
    return if (isTopLevelScripts) ProjectEntry.Script(property.name) else null
}
