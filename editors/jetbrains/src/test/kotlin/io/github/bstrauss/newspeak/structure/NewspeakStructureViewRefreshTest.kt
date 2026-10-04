package io.github.bstrauss.newspeak.structure

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.util.Disposer
import com.intellij.testFramework.fixtures.BasePlatformTestCase
import org.eclipse.lsp4j.DocumentSymbol
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.SymbolKind
import java.util.concurrent.atomic.AtomicInteger

/**
 * The Structure tool window builds a new model each time it shows a file --
 * selected anew in the Project view, the window reopened -- and a model that
 * starts empty would show an empty tree until the server has answered, then
 * the tree, a flicker on every rebuild. A new model of an unchanged file must
 * therefore answer at once from the file's last answer.
 */
class NewspeakStructureViewRefreshTest : BasePlatformTestCase() {
    private val range = Range(Position(0, 0), Position(0, 20))

    private fun moduleSymbols() = listOf(DocumentSymbol("Module", SymbolKind.Class, range, range, "public"))

    /** Added to the project, never opened in an editor. */
    private val psiFile by lazy { myFixture.addFileToProject("Module.ns", "class Module = () ()\n") }

    private fun modelOf(fetches: AtomicInteger): NewspeakStructureViewModel {
        val model = NewspeakStructureViewModel(psiFile, psiFile.virtualFile, null) {
            fetches.incrementAndGet()
            moduleSymbols()
        }
        Disposer.register(testRootDisposable, model)
        return model
    }

    private fun offTheEdt(action: () -> Unit) {
        ApplicationManager.getApplication().executeOnPooledThread {
            ApplicationManager.getApplication().runReadAction(action)
        }.get()
    }

    fun testANewModelOfAnUnchangedFileAnswersAtOnceWithoutAskingTheServer() {
        val fetches = AtomicInteger()
        val first = modelOf(fetches)
        offTheEdt { assertEquals(moduleSymbols(), first.symbols()) }
        assertEquals(1, fetches.get())

        // A rebuilt view: its first question, even on the EDT, finds the answer.
        val rebuilt = modelOf(fetches)
        assertEquals(moduleSymbols(), rebuilt.symbols())
        offTheEdt { assertEquals(moduleSymbols(), rebuilt.symbols()) }
        assertEquals("no second request", 1, fetches.get())
        // A retry is scheduled before `symbols()` returns, so none is pending
        // now means none will come; no waiting needed.
        assertFalse("no refresh scheduled", rebuilt.asksAgain)
    }

    fun testAChangedFileIsAskedAgain() {
        val fetches = AtomicInteger()
        val first = modelOf(fetches)
        offTheEdt { first.symbols() }
        val document = FileDocumentManager.getInstance().getDocument(psiFile.virtualFile)!!
        WriteCommandAction.runWriteCommandAction(project) {
            document.insertString(0, " ")
        }
        val second = modelOf(fetches)
        offTheEdt { second.symbols() }
        assertEquals(2, fetches.get())
    }

    /** A refresh that brings the same answer keeps the tree's nodes, so nothing is redrawn. */
    fun testElementsOfEqualSymbolsAreEqual() {
        val model = modelOf(AtomicInteger())
        val first = model.elementsOf(moduleSymbols())
        val second = model.elementsOf(moduleSymbols())
        assertEquals(first.toList(), second.toList())
        assertEquals(first[0].hashCode(), second[0].hashCode())
        assertFalse(first[0] == model.elementsOf(listOf(DocumentSymbol("Other", SymbolKind.Class, range, range, "public")))[0])
    }
}
