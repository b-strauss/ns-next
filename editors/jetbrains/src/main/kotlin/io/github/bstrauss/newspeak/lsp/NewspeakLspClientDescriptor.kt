package io.github.bstrauss.newspeak.lsp

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor
import com.intellij.platform.lsp.api.customization.LspCustomization
import com.intellij.platform.lsp.api.customization.LspSemanticTokensCustomizer
import io.github.bstrauss.newspeak.NewspeakFileType
import java.nio.file.Path

class NewspeakLspClientDescriptor(project: Project, private val server: Path) :
    ProjectWideLspClientDescriptor(project, "Newspeak") {
    override fun isSupportedFile(file: VirtualFile): Boolean = file.fileType == NewspeakFileType

    override fun createCommandLine(): GeneralCommandLine = GeneralCommandLine(server.toString())

    override val lspCustomization: LspCustomization = object : LspCustomization() {
        override val semanticTokensCustomizer: LspSemanticTokensCustomizer = NewspeakSemanticTokens
    }
}
