package io.github.bstrauss.newspeak.lsp

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor
import com.intellij.platform.lsp.api.customization.LspCustomization
import com.intellij.platform.lsp.api.customization.LspDocumentSymbolCustomizer
import com.intellij.platform.lsp.api.customization.LspDocumentSymbolSupport
import com.intellij.platform.lsp.api.customization.LspFormattingCustomizer
import com.intellij.platform.lsp.api.customization.LspFormattingSupport
import com.intellij.platform.lsp.api.customization.LspSemanticTokensCustomizer
import com.intellij.platform.lsp.api.customization.LspSymbolKindCustomizer
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.NewspeakFileType
import java.nio.file.Path

class NewspeakLspClientDescriptor(project: Project, private val server: Path) :
    ProjectWideLspClientDescriptor(project, NewspeakBundle.message("lsp.client.name")) {
    override fun isSupportedFile(file: VirtualFile): Boolean = file.fileType == NewspeakFileType

    override fun createCommandLine(): GeneralCommandLine = GeneralCommandLine(server.toString(), "lsp")

    override val lspCustomization: LspCustomization = NewspeakLspCustomization
}

/** Which of the platform's LSP features the Newspeak client uses, and how. */
object NewspeakLspCustomization : LspCustomization() {
    override val semanticTokensCustomizer: LspSemanticTokensCustomizer = NewspeakSemanticTokens
    override val symbolKindCustomizer: LspSymbolKindCustomizer = NewspeakSymbolKinds
    override val documentSymbolCustomizer: LspDocumentSymbolCustomizer = NewspeakDocumentSymbols
    override val formattingCustomizer: LspFormattingCustomizer = NewspeakFormatting
}

/**
 * Reformat Code goes to the server's `textDocument/formatting`. The
 * platform's default hands a file to the server only when the server has
 * registered formatting for it dynamically (`client/registerCapability`);
 * the server announces `documentFormattingProvider` statically, which the
 * default does not count, so Reformat Code would never reach it and would
 * report the file as formatted already. The server is the only formatter
 * Newspeak has, so every file this client supports goes to it.
 */
object NewspeakFormatting : LspFormattingSupport() {
    override fun shouldFormatThisFileExclusivelyByServer(
        file: VirtualFile,
        ideCanFormatThisFileItself: Boolean,
        serverExplicitlyWantsToFormatThisFile: Boolean,
    ): Boolean = true
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
