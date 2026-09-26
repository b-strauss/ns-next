package io.github.bstrauss.newspeak

import com.intellij.openapi.fileTypes.FileTypeManager
import com.intellij.testFramework.fixtures.BasePlatformTestCase

class NewspeakEditorTest : BasePlatformTestCase() {
    fun testNsFilesAreNewspeak() {
        assertSame(NewspeakFileType, FileTypeManager.getInstance().getFileTypeByFileName("Module.ns"))
    }
}
