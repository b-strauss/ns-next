package io.github.bstrauss.newspeak.lsp

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor
import com.intellij.platform.lsp.api.customization.LspCustomization
import com.intellij.platform.lsp.api.customization.LspDocumentSymbolCustomizer
import com.intellij.platform.lsp.api.customization.LspDocumentSymbolSupport
import com.intellij.platform.lsp.api.customization.LspSemanticTokensCustomizer
import com.intellij.platform.lsp.api.customization.LspSymbolKindCustomizer
import io.github.bstrauss.newspeak.NewspeakFileType
import java.nio.file.Path

class NewspeakLspClientDescriptor(project: Project, private val server: Path) :
    ProjectWideLspClientDescriptor(project, "Newspeak") {
    override fun isSupportedFile(file: VirtualFile): Boolean = file.fileType == NewspeakFileType

    override fun createCommandLine(): GeneralCommandLine = GeneralCommandLine(server.toString())

    override val lspCustomization: LspCustomization = NewspeakLspCustomization
}

/** Which of the platform's LSP features the Newspeak client uses, and how. */
object NewspeakLspCustomization : LspCustomization() {
    override val semanticTokensCustomizer: LspSemanticTokensCustomizer = NewspeakSemanticTokens
    override val symbolKindCustomizer: LspSymbolKindCustomizer = NewspeakSymbolKinds
    override val documentSymbolCustomizer: LspDocumentSymbolCustomizer = NewspeakDocumentSymbols
}

/**
 * The platform's structure view for a language server draws one icon per
 * symbol kind and cannot show a member's access, so the plugin registers its
 * own (`NewspeakStructureViewFactory`). The platform's factory is registered
 * for every language and the plugin's for Newspeak alone, and a
 * language-specific factory is asked first; turning the platform's off for
 * this client as well makes that certain, since its factory then answers no
 * builder for a Newspeak file. Breadcrumbs, and the sticky lines the platform
 * draws from the same symbols, stay the platform's.
 */
object NewspeakDocumentSymbols : LspDocumentSymbolSupport() {
    override val structureViewSupport: Boolean = false
    override val breadcrumbsSupport: Boolean = true
}
