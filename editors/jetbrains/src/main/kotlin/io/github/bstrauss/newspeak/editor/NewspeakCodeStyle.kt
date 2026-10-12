package io.github.bstrauss.newspeak.editor

import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.PsiFile
import com.intellij.psi.codeStyle.CodeStyleSettings
import com.intellij.psi.codeStyle.CommonCodeStyleSettings.IndentOptions
import com.intellij.psi.codeStyle.FileIndentOptionsProvider
import com.intellij.psi.codeStyle.IndentStatusBarUIContributor
import com.intellij.psi.codeStyle.modifier.CodeStyleSettingsModifier
import com.intellij.psi.codeStyle.modifier.CodeStyleStatusBarUIContributor
import com.intellij.psi.codeStyle.modifier.TransientCodeStyleSettings
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakFileType

/**
 * The formatter's one style (`docs/design/formatting.md`, rules 1 and 12):
 * two spaces per level, continuation lines at +4, never a tab, 100 columns.
 * The formatter takes no options, so the editor takes these values rather
 * than offering settings Reformat Code would ignore: there is no Code Style
 * page for Newspeak, and the user's code style, detected indents and an
 * `.editorconfig` do not reach a `.ns` file.
 */
object NewspeakCodeStyle {
    const val INDENT_SIZE = 2
    const val CONTINUATION_INDENT_SIZE = 4
    const val RIGHT_MARGIN = 100

    fun indentOptions(): IndentOptions = IndentOptions().apply {
        INDENT_SIZE = NewspeakCodeStyle.INDENT_SIZE
        TAB_SIZE = NewspeakCodeStyle.INDENT_SIZE
        CONTINUATION_INDENT_SIZE = NewspeakCodeStyle.CONTINUATION_INDENT_SIZE
        USE_TAB_CHARACTER = false
        SMART_TABS = false
        KEEP_INDENTS_ON_EMPTY_LINES = false
    }

    fun isNewspeak(file: VirtualFile): Boolean = file.fileType == NewspeakFileType
}

/**
 * The indentation of every Newspeak file, ordered first so that neither the
 * detected indents nor an `.editorconfig` provider answers before it.
 * Tab, smart Backspace and the editor's tab width all read it.
 */
class NewspeakIndentOptionsProvider : FileIndentOptionsProvider() {
    override fun getIndentOptions(project: Project, settings: CodeStyleSettings, file: VirtualFile): IndentOptions? =
        if (NewspeakCodeStyle.isNewspeak(file)) NewspeakCodeStyle.indentOptions() else null

    override fun getIndentStatusBarUiContributor(indentOptions: IndentOptions): CodeStyleStatusBarUIContributor? =
        if (indentOptions.fileIndentOptionsProvider is NewspeakIndentOptionsProvider) {
            NewspeakIndentStatusBarContributor(indentOptions)
        } else {
            null
        }
}

/**
 * Keeps a Newspeak file's code style the formatter's when another source would
 * change it per file: the first modifier that claims a file is the only one
 * asked, and an `.editorconfig`'s modifier, which ignores the indent options
 * providers, would otherwise set its `indent_size` and `max_line_length` --
 * a `[*]` section is common. Claiming the file leaves its indentation to
 * [NewspeakIndentOptionsProvider] and sets its right margin, which the editor
 * reads for its margin guide and soft wraps.
 */
class NewspeakCodeStyleModifier : CodeStyleSettingsModifier {
    override fun modifySettings(settings: TransientCodeStyleSettings, file: PsiFile): Boolean {
        val virtualFile = file.viewProvider.virtualFile
        if (!NewspeakCodeStyle.isNewspeak(virtualFile)) return false
        settings.defaultRightMargin = NewspeakCodeStyle.RIGHT_MARGIN
        return true
    }

    override fun acceptsFileIndentOptionsProviders(): Boolean = true

    override fun getName(): String = NewspeakBundle.message("code.style.fixed.name")

    override fun getStatusBarUiContributor(transientSettings: TransientCodeStyleSettings): CodeStyleStatusBarUIContributor =
        NewspeakIndentStatusBarContributor(NewspeakCodeStyle.indentOptions())
}

/**
 * The status bar's indent widget for a Newspeak file: "2 spaces", the hint
 * that the formatter fixes it, and no action -- not even "Configure Indents
 * for Newspeak", which would open settings that change nothing here.
 */
private class NewspeakIndentStatusBarContributor(options: IndentOptions) : IndentStatusBarUIContributor(options) {
    override fun getHint(): String = NewspeakBundle.message("code.style.fixed.hint")

    override fun isShowFileIndentOptionsEnabled(): Boolean = false

    override fun areActionsAvailable(file: VirtualFile): Boolean = false

    override fun getActions(file: PsiFile): Array<AnAction>? = null

    override fun createDisableAction(project: Project): AnAction? = null
}
