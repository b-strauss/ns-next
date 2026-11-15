package io.github.bstrauss.newspeak.library

import com.intellij.testFramework.fixtures.BasePlatformTestCase

/** Which projects show the core library: those holding a `.ns` file or a `newspeak.json`. */
class NewspeakProjectTest : BasePlatformTestCase() {
    fun testAProjectWithoutNewspeakFilesIsNoNewspeakProject() {
        myFixture.addFileToProject("notes.txt", "nothing of Newspeak\n")
        assertFalse(isNewspeakProject(project))
    }

    fun testAProjectHoldingAModuleIsANewspeakProject() {
        myFixture.addFileToProject("source/Hello.ns", "class Hello = ()\n")
        assertTrue(isNewspeakProject(project))
    }

    fun testAProjectHoldingAProjectFileIsANewspeakProject() {
        myFixture.addFileToProject("newspeak.json", """{"main": "Main.ns"}""")
        assertTrue(isNewspeakProject(project))
    }
}
