package io.github.bstrauss.newspeak.project

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.jetbrains.jsonSchema.extension.JsonSchemaFileProvider
import com.jetbrains.jsonSchema.extension.JsonSchemaProviderFactory
import com.jetbrains.jsonSchema.extension.SchemaType

/**
 * Gives every file named `newspeak.json` -- ns-next's project file, which it
 * recognises by that name alone -- the schema bundled from the repository's
 * `schemas/newspeak.schema.json`, the one ns-next's reader is held to.
 */
class NewspeakJsonSchemaProviderFactory : JsonSchemaProviderFactory {
    override fun getProviders(project: Project): List<JsonSchemaFileProvider> = listOf(Provider)

    private object Provider : JsonSchemaFileProvider {
        override fun isAvailable(file: VirtualFile): Boolean = file.name == PROJECT_FILE_NAME

        override fun getName(): String = "ns-next project"

        override fun getSchemaFile(): VirtualFile? =
            JsonSchemaProviderFactory.getResourceFile(
                NewspeakJsonSchemaProviderFactory::class.java,
                "/schemas/newspeak.schema.json",
            )

        override fun getSchemaType(): SchemaType = SchemaType.embeddedSchema
    }

    private companion object {
        /** `crates/ns-cli/src/project.rs`'s `PROJECT_FILE_NAME`. */
        const val PROJECT_FILE_NAME = "newspeak.json"
    }
}
