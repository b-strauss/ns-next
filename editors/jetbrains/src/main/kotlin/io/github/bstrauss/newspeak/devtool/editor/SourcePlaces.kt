package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.openapi.editor.Document
import com.intellij.openapi.util.TextRange
import io.github.bstrauss.newspeak.devtool.model.Body
import io.github.bstrauss.newspeak.devtool.model.SourceRange
import java.io.IOException
import java.nio.file.Files
import java.nio.file.InvalidPathException
import java.nio.file.Path

/**
 * The file [body]'s source lies in: a program's by its path, a core-library
 * module's under [libraryDirectory]; null for a body without a source, a
 * library body before the library has a directory, and a path the platform
 * cannot name.
 */
fun bodySourcePath(body: Body, libraryDirectory: Path?): Path? = try {
    body.file?.let { file -> Path.of(file) } ?: body.library?.let { library -> libraryDirectory?.resolve(library) }
} catch (_: InvalidPathException) {
    null
}

/**
 * Whether [first] and [second] name one file: by their normal forms, and
 * where those differ by the file system, since a trace keeps the path its
 * run was given and an editor the one the platform found -- through a link,
 * or a Windows short name, the two are spelled apart. A path naming no file
 * names no other.
 */
fun sameFile(first: Path, second: Path): Boolean {
    val firstNormal = first.toAbsolutePath().normalize()
    val secondNormal = second.toAbsolutePath().normalize()
    if (firstNormal == secondNormal) return true
    return try {
        Files.isSameFile(firstNormal, secondNormal)
    } catch (_: IOException) {
        false
    }
}

/** The offset of [line] and [column] in [document]; null where it lies outside the text. */
fun documentOffset(document: Document, line: Int, column: Int): Int? {
    if (line < 0 || line >= document.lineCount || column < 0) return null
    // The runtime counts columns in UTF-16 units, as a document's offsets are.
    val offset = document.getLineStartOffset(line) + column
    return offset.takeIf { offset <= document.getLineEndOffset(line) }
}

/** [range]'s offsets in [document] as the run's text placed it; null where it lies outside the text. */
fun documentRange(document: Document, range: SourceRange): TextRange? {
    val start = documentOffset(document, range.startLine, range.startColumn) ?: return null
    val end = documentOffset(document, range.endLine, range.endColumn) ?: return null
    return if (start <= end) TextRange(start, end) else null
}

/**
 * A body of the decorated run as [document] holds it now: its record, its
 * current offsets -- its marker's, which followed every edit -- and whether
 * its method changed since the run (decision 32).
 */
class PlacedBody(val body: Body, val document: Document, val range: TextRange, val stale: Boolean) {
    /**
     * [range] of the run's text -- a statement's, within the body -- where
     * the body stands now; null for a stale body, whose text is no longer
     * the run's, and a range outside the body. The body's text is the run's,
     * so a line inside it keeps its columns and only the body's first line
     * moves by the body's own shift.
     */
    fun place(range: SourceRange): TextRange? {
        if (stale) return null
        val origin = body.range ?: return null
        val startLine = document.getLineNumber(this.range.startOffset)
        fun offset(line: Int, column: Int): Int? {
            val placed = if (line == origin.startLine) {
                this.range.startOffset + column - origin.startColumn
            } else {
                documentOffset(document, startLine + line - origin.startLine, column) ?: return null
            }
            return placed.takeIf { offset -> offset in this.range.startOffset..this.range.endOffset }
        }
        val start = offset(range.startLine, range.startColumn) ?: return null
        val end = offset(range.endLine, range.endColumn) ?: return null
        return if (start <= end) TextRange(start, end) else null
    }
}
