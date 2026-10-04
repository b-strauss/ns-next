package io.github.bstrauss.newspeak.project

import com.intellij.openapi.application.PathManager
import com.intellij.openapi.vfs.newvfs.impl.VfsRootAccess
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import com.jetbrains.jsonSchema.ide.JsonSchemaService
import com.jetbrains.jsonSchema.impl.inspections.JsonSchemaComplianceInspection

class NewspeakJsonSchemaProviderFactoryTest : BasePlatformTestCase() {
    override fun setUp() {
        super.setUp()
        // The schema is read from the plugin's jar in the test sandbox, a root
        // the test framework's VFS guard does not allow by default; an
        // installed IDE has no such guard.
        val pluginJar = checkNotNull(PathManager.getJarPathForClass(NewspeakJsonSchemaProviderFactory::class.java)) {
            "no jar or directory holds NewspeakJsonSchemaProviderFactory"
        }
        VfsRootAccess.allowRootAccess(testRootDisposable, pluginJar)
    }

    fun testANewspeakJsonGetsTheProjectSchema() {
        val file = myFixture.configureByText("newspeak.json", """{ "main": "App.ns" }""")
        val schemas = JsonSchemaService.Impl.get(project).getSchemaFilesForFile(file.virtualFile)
        assertTrue(schemas.any { schema -> schema.name == "newspeak.schema.json" })
    }

    fun testAnotherJsonFileDoesNot() {
        val file = myFixture.configureByText("package.json", "{}")
        val schemas = JsonSchemaService.Impl.get(project).getSchemaFilesForFile(file.virtualFile)
        assertFalse(schemas.any { schema -> schema.name == "newspeak.schema.json" })
    }

    fun testAMissingMainIsReported() {
        myFixture.enableInspections(JsonSchemaComplianceInspection::class.java)
        myFixture.configureByText(
            "newspeak.json",
            // The inspection marks the property the object holds, not the whole object.
            """{ <warning descr="Missing required property 'main'">"root": "App.ns"</warning> }""",
        )
        myFixture.checkHighlighting(true, false, false)
    }
}
