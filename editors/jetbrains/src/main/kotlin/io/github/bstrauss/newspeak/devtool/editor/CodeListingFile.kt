package io.github.bstrauss.newspeak.devtool.editor

import com.intellij.icons.AllIcons
import com.intellij.openapi.editor.Document
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileTypes.FileType
import com.intellij.openapi.util.Key
import com.intellij.testFramework.LightVirtualFile
import io.github.bstrauss.newspeak.NewspeakBundle
import io.github.bstrauss.newspeak.devtool.model.Bytecode
import io.github.bstrauss.newspeak.devtool.model.BytecodeBody
import io.github.bstrauss.newspeak.devtool.model.Code
import io.github.bstrauss.newspeak.devtool.model.Scope
import io.github.bstrauss.newspeak.devtool.model.TraceRun
import java.util.Collections
import java.util.WeakHashMap
import javax.swing.Icon

/** The bytecode instruction a line of a listing stands for: its [body] and its [index] there. */
data class ListingPosition(val body: Int, val index: Int)

/**
 * A listing as text in the CLI's format (`crates/ns-cli/src/listing.rs`),
 * its [title] the name of its editor tab, and for each line of [text] the
 * bytecode instruction it stands for, null for a line that stands for none;
 * the [run] whose records it was built from, null for a listing that came as
 * text, which links nothing.
 */
class CodeListing(val title: String, val text: String, val positions: List<ListingPosition?>, val run: TraceRun?) {
    /** A listing that came as text alone: the debugger's frame code. */
    constructor(title: String, text: String) : this(title, text, emptyList(), null)
}

/** A read-only listing's file type, its text plain: no file on disk is ever one. */
object CodeListingFileType : FileType {
    override fun getName(): String = "Newspeak Code Listing"

    override fun getDescription(): String = NewspeakBundle.message("devtool.code.file.type")

    override fun getDefaultExtension(): String = ""

    override fun getIcon(): Icon = AllIcons.FileTypes.Text

    override fun isBinary(): Boolean = false

    override fun isReadOnly(): Boolean = true
}

/**
 * A listing in an editor tab of its own (`docs/design/developer-tool.md`,
 * decision 30): read-only, and kept while the tool window's view moves on,
 * so that two copies can be compared. Once its run's tab closes it keeps
 * its text and lets the run go ([unlinkListingsOf]): a listing tab left
 * open never holds a run's records, and links nothing from then on.
 */
class CodeListingFile(listing: CodeListing) : LightVirtualFile(listing.title, CodeListingFileType, listing.text) {
    /** A listing of [text] alone, named [name]: the debugger's frame code. */
    constructor(name: String, text: String) : this(CodeListing(name, text))

    /** What the tab shows: the listing as built, and its text alone once its run was closed. */
    var listing: CodeListing = listing
        private set

    init {
        isWritable = false
        if (listing.run != null) linkedFiles.add(this)
    }

    /** Drops the link to the run, keeping the title and the text. */
    internal fun unlink() {
        listing = CodeListing(listing.title, listing.text)
        linkedFiles.remove(this)
    }

    companion object {
        /** Every listing file that still names its run, held weakly: a file nothing else holds goes with its link. */
        private val linkedFiles: MutableSet<CodeListingFile> = Collections.newSetFromMap(WeakHashMap())

        /** [run]'s tab closed: every listing file built from it keeps its text and lets it go. Called on the EDT. */
        fun unlinkListingsOf(run: TraceRun) {
            for (file in linkedFiles.toList()) if (file.listing.run === run) file.unlink()
        }
    }
}

/** The listing that [document] shows: the Code tab's or a listing tab's; null for any other document. */
fun listingOf(document: Document): CodeListing? =
    document.getUserData(LISTING_KEY) ?: (FileDocumentManager.getInstance().getFile(document) as? CodeListingFile)?.listing

/** Marks [document] as showing [listing], as the Code tab's viewer does. */
fun showListing(document: Document, listing: CodeListing?) = document.putUserData(LISTING_KEY, listing)

private val LISTING_KEY = Key.create<CodeListing>("newspeak.devtool.listing")

/** Builds a listing's lines, each with the instruction it stands for. */
private class ListingWriter {
    private val text = StringBuilder()
    val positions = ArrayList<ListingPosition?>()

    fun line(line: String, position: ListingPosition? = null) {
        text.append(line).append('\n')
        positions.add(position)
    }

    fun build(title: String, run: TraceRun) = CodeListing(title, text.toString(), positions, run)
}

/** A bytecode instruction's line as the text listing writes it: its index in four columns, its statement or `-` in seven, the instruction. */
private fun instructionLine(index: Int, statement: Int?, text: String): String =
    "${index.toString().padStart(4)}  ${statementWord(statement).padStart(7)}  $text"

private fun statementWord(statement: Int?): String = statement?.let { index -> "stmt $index" } ?: "-"

private fun bodyName(run: TraceRun, body: Int): String = run.bodies[body]?.name ?: "#$body"

/**
 * [bytecode]'s listing as the text listing writes it: the head `---
 * bytecode {<body>}`, the root's line -- its kind, its name, its counts of
 * arguments and registers -- and its instructions, each block it makes
 * nested eight columns deeper under its `MakeClosure` with the line
 * `block block (args …, registers …)` the text names every block by, and
 * `--- end`. The record lists the blocks in the order the text nests them,
 * so the next block not yet written is the one a `MakeClosure` makes. A
 * record without the counts writes the line without them.
 */
fun bytecodeListing(run: TraceRun, bytecode: Bytecode): CodeListing {
    val writer = ListingWriter()
    val name = bodyName(run, bytecode.body)
    writer.line("--- bytecode {$name}")
    val bodies = bytecode.bodies
    var next = 0
    // A worklist, not recursion: an image's body chooses how deep its blocks nest.
    val pending = ArrayDeque<NestedBody>()
    fun begin(depth: Int) {
        val listed = bodies[next]
        val pad = " ".repeat(depth * BLOCK_INDENT)
        val root = next == 0
        val line = StringBuilder(pad).append(listed.kind ?: if (root) METHOD else BLOCK).append(' ').append(if (root) name else BLOCK)
        if (listed.arguments != null && listed.registers != null) line.append(" (args ${listed.arguments}, registers ${listed.registers})")
        writer.line(line.toString())
        pending.addLast(NestedBody(listed, 0, depth))
        next++
    }
    while (next < bodies.size) {
        // A block the record lists past every `MakeClosure` stands one level in, after the rest.
        begin(if (next == 0) 0 else 1)
        while (pending.isNotEmpty()) {
            val nested = pending.removeLast()
            val instruction = nested.body.instructions.getOrNull(nested.position) ?: continue
            val pad = " ".repeat(nested.depth * BLOCK_INDENT)
            writer.line(pad + instructionLine(instruction.index, instruction.statement, instruction.text), ListingPosition(nested.body.body, instruction.index))
            pending.addLast(NestedBody(nested.body, nested.position + 1, nested.depth))
            if (instruction.text.startsWith(MAKE_CLOSURE) && next < bodies.size) begin(nested.depth + 1)
        }
    }
    writer.line(END)
    return writer.build("$name bytecode", run)
}

/** A body of a bytecode listing being written: the [position] of its next instruction and how many blocks deep it stands. */
private class NestedBody(val body: BytecodeBody, val position: Int, val depth: Int)

/** How many columns deeper the text listing writes a block than the body that makes it. */
private const val BLOCK_INDENT = 8

private const val METHOD = "method"
private const val BLOCK = "block"
private const val MAKE_CLOSURE = "MakeClosure "

/**
 * [code]'s listing as the CLI writes it -- its head, `base=` where the
 * record gives the address, an optimized copy's scope tree, each stretch
 * named where its scope or index changes, the instructions at their
 * offsets with their annotations, the data words.
 */
fun codeListing(run: TraceRun, code: Code): CodeListing {
    val writer = ListingWriter()
    val name = bodyName(run, code.body)
    val head = StringBuilder("--- code {$name} ${code.tier} ${code.architecture}")
    code.key?.let { key ->
        head.append(" key={$key}")
        code.entered?.let { entered -> head.append(" entered={$entered}") }
    }
    code.base?.let { base -> head.append(" base=0x").append(java.lang.Long.toHexString(base)) }
    head.append(" bytes=${code.bytes}")
    writer.line(head.toString())
    val optimized = code.tier == OPTIMIZED
    if (optimized) {
        writer.line("scopes:")
        writeScopeTree(writer, run, code.scopes)
    }
    val scopeBodies = code.scopes.associate { scope -> scope.scope to scope.body }
    var lastPosition: Triple<Int?, Int?, Boolean>? = null
    for (instruction in code.instructions) {
        val position = Triple(instruction.scope, instruction.at, instruction.outOfLine)
        val at = instruction.at
        // An instruction without a scope of its own stands in the body's.
        val linked = at?.let { index -> ListingPosition(instruction.scope?.let { scope -> scopeBodies[scope] } ?: code.body, index) }
        // The code before the first position is named by no line; a record without `out-of-line` writes an
        // out-of-line stretch as it writes that code, so there a stretch without a position is named out of
        // line only after a position.
        if (position != lastPosition && (lastPosition != null || at != null || instruction.outOfLine)) {
            if (at == null) {
                writer.line("  out of line")
            } else {
                val label = StringBuilder("  ")
                if (optimized) label.append("s${instruction.scope ?: 0} {${bodyName(run, checkNotNull(linked).body)}} ")
                label.append("@$at")
                bytecodeText(run, checkNotNull(linked))?.let { text -> label.append("  ").append(text) }
                writer.line(label.toString(), linked)
            }
            lastPosition = position
        }
        val indent = if (code.mark == instruction.offset) "  ->" else "    "
        val line = StringBuilder("$indent${bodyOffset(instruction.offset)}  ${instruction.text}")
        instruction.annotation?.let { annotation -> line.append("  ; ").append(annotation) }
        writer.line(line.toString(), linked)
    }
    if (code.data.isNotEmpty()) {
        writer.line("data:")
        for (word in code.data) writer.line("    data+0x${hex(word.offset)}  ${word.word}")
    }
    writer.line(END)
    val title = StringBuilder("$name ${code.tier}")
    code.key?.let { key -> title.append(" key=$key") }
    code.entered?.let { entered -> title.append(" entered=$entered") }
    return writer.build(title.toString(), run)
}

/** The scopes, each under the one it is inlined into, two spaces deeper, with the send it is inlined at. */
private fun writeScopeTree(writer: ListingWriter, run: TraceRun, scopes: List<Scope>) {
    val pending = ArrayDeque(scopes.filter { scope -> scope.parent == null }.map { scope -> scope to 1 })
    while (pending.isNotEmpty()) {
        val (scope, depth) = pending.removeFirst()
        val line = StringBuilder("  ".repeat(depth)).append("s${scope.scope} {${bodyName(run, scope.body)}}")
        scope.parent?.let { parent -> line.append(" at s${parent.scope}@${parent.at}") }
        writer.line(line.toString())
        val children = scopes.filter { child -> child.parent?.scope == scope.scope }.map { child -> child to depth + 1 }
        for (child in children.asReversed()) pending.addFirst(child)
    }
}

/** The bytecode at [position] as a stretch's line names it, without its index: `stmt 0  Return r1`; null where the run listed no bytecode of the body. */
private fun bytecodeText(run: TraceRun, position: ListingPosition): String? {
    val instruction = run.bytecode.values.asSequence()
        .flatMap { bytecode -> bytecode.bodies.asSequence() }
        .firstOrNull { listed -> listed.body == position.body }
        ?.instructions?.firstOrNull { instruction -> instruction.index == position.index } ?: return null
    return "${statementWord(instruction.statement)}  ${instruction.text}"
}

private fun bodyOffset(offset: Int): String = "+0x${hex(offset)}"

private fun hex(offset: Int): String = Integer.toHexString(offset).padStart(OFFSET_DIGITS, '0')

private const val OFFSET_DIGITS = 4

private const val OPTIMIZED = "optimized"

private const val END = "--- end"
