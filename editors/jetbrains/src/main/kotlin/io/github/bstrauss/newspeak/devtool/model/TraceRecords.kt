package io.github.bstrauss.newspeak.devtool.model

import com.google.gson.JsonArray
import com.google.gson.JsonElement
import com.google.gson.JsonObject
import com.google.gson.JsonParser

/**
 * One line of the trace file `ns-next run --trace-file` writes
 * (`docs/design/developer-tool.md`, "The trace file"): every body named by
 * its number, unique for the run and never 0, and every range a
 * [SourceRange] of the body's module's text.
 */
sealed interface TraceRecord {
    companion object {
        /**
         * The record [line] holds; null for a kind of record this reader
         * does not know, which a later runtime may add without a reader of
         * the known ones misreading anything.
         *
         * @throws MalformedRecordException where [line] is no JSON object
         *   or lacks a field its kind has.
         */
        fun parse(line: String): TraceRecord? {
            val record = try {
                JsonParser.parseString(line)
            } catch (exception: RuntimeException) {
                throw MalformedRecordException(exception.message ?: line)
            }
            if (!record.isJsonObject) throw MalformedRecordException("no object")
            return try {
                parseObject(record.asJsonObject)
            } catch (exception: RuntimeException) {
                // Gson's accessors refuse a field of the wrong type with
                // whichever exception their element's class raises.
                throw MalformedRecordException(exception.message ?: exception.javaClass.simpleName)
            }
        }

        /**
         * The record [line] holds as a run keeps it, the line standing at
         * [place] in its file: a `code` record as its [CodeEntry] and an
         * `inlining` record as its [PlanEntry], each read whole -- so that a
         * malformed one ends the reading where it stands, as any other
         * record does -- and kept as the fields the views decide by; every
         * other record as [parse] answers it.
         *
         * @throws MalformedRecordException as [parse] does.
         */
        fun indexed(line: String, place: RecordPlace): TraceRecord? = when (val record = parse(line)) {
            is Code -> CodeEntry(record.body, record.tier, record.key, record.entered, record.base, record.bytes, place)
            is Inlining -> PlanEntry(record.body, record.key, record.entered, record.installed, record.refused, place)
            else -> record
        }
    }
}

/**
 * Where a record stands in its trace file: the [offset] of its line's
 * first byte and the line's [length] in bytes, without its newline.
 */
data class RecordPlace(val offset: Long, val length: Int)

/**
 * An install's machine code as a run keeps it (`docs/design/developer-tool.md`,
 * decision 34): what the Methods tab, the Code tab's chooser and the
 * state icons decide by -- its body, [tier], [key], [entered]
 * application, [base] and size in [bytes] -- and the [place] of its
 * [Code] record in the file, which a view reads when it shows the listing.
 */
data class CodeEntry(
    val body: Int,
    val tier: String,
    val key: String?,
    val entered: String?,
    val base: Long?,
    val bytes: Int,
    val place: RecordPlace,
) : TraceRecord

/**
 * An optimizing compile's plan as a run keeps it (decision 34): its
 * body, the copy's [key] and [entered] application and its outcome --
 * the bytes [installed] or the reason it was [refused] --, which the
 * Inlining tab's root names and the sends' decisions are concluded by,
 * and the [place] of its [Inlining] record in the file.
 */
data class PlanEntry(
    val body: Int,
    val key: String?,
    val entered: String?,
    val installed: Int?,
    val refused: String?,
    val place: RecordPlace,
) : TraceRecord

/** A line that is no record of its kind, [message] saying why. */
class MalformedRecordException(message: String) : Exception(message)

/** `[startLine, startColumn, endLine, endColumn]`, counted from 0, the columns in UTF-16 code units. */
data class SourceRange(val startLine: Int, val startColumn: Int, val endLine: Int, val endColumn: Int)

/**
 * The file's first line: the format's [version], the [architecture] its
 * `code` records are instructions of, null without a JIT, and the
 * [categories] traced and [listings] printed, by their names, each null in
 * a header that does not say.
 */
data class Header(
    val version: Int,
    val architecture: String?,
    val categories: List<String>? = null,
    val listings: List<String>? = null,
) : TraceRecord

/** A statement of a body: its [instructions], from the first up to, not including, the last, and its source. */
data class Statement(val instructions: IntRange, val range: SourceRange?)

/** A send instruction of a body: its index [at], its cache [site] in its family, null for an uncached send, its call and its selector's tokens. */
data class Send(val at: Int, val site: Int?, val call: SourceRange?, val selector: List<SourceRange?>)

/**
 * A body's description, before the first record that names [number]: for
 * a block, its [family]'s root, which the cache transitions of the block's
 * sites name, null for a root; its module, its source -- a program's [file] or the core library's
 * [library] path, neither for a module read from no file -- its [range]
 * in the text it was compiled from, null where no text that holds the body
 * as it was compiled could be had, a body an edit compiled and a later one
 * replaced among them, and the [checksum] of its source, by which a reader
 * tells whether the text it holds is the one the body was compiled from.
 *
 * A record whose `placed` is `current` counts its ranges in the runtime's
 * current text of the module, which an edit made and no file the plugin
 * holds is: such a body is read with [placedInCurrentText] set and every
 * range null, so that nothing decorates, links or navigates into the
 * file's text at lines that belong to other text.
 */
data class Body(
    val number: Int,
    val name: String,
    val family: Int?,
    val moduleId: String,
    val moduleVersion: String?,
    val file: String?,
    val library: String?,
    val range: SourceRange?,
    val checksum: String,
    val statements: List<Statement>,
    val sends: List<Send>,
    val placedInCurrentText: Boolean = false,
) : TraceRecord

/**
 * An event: its [category], its [body], null for an event about none, its
 * name as the text trace writes it, and every other field of the record,
 * under the text line's names (`in-body`, `at`, `key`, ...).
 */
data class Event(val category: String, val body: Int?, val event: String, val fields: JsonObject) : TraceRecord {
    /** The number field [name], null where it is absent or `null`. */
    fun integer(name: String): Int? = fields.get(name)?.takeUnless { value -> value.isJsonNull }?.asInt

    /** The text field [name], null where it is absent or `null`. */
    fun text(name: String): String? = fields.get(name)?.takeUnless { value -> value.isJsonNull }?.asString
}

/** An instruction of a bytecode listing: its [index], its statement, null outside every statement, and its text. */
data class BytecodeInstruction(val index: Int, val statement: Int?, val text: String)

/**
 * One body of a bytecode listing, the root or a block it makes, with what
 * the text listing's line above its instructions writes: its [kind]
 * (`method`, `block`, `initializer`, `superclass clause`) and its counts of
 * [arguments] and [registers]; and for a block, the body [madeIn], listed
 * before it, whose `MakeClosure` at the index [at] makes it; each null in
 * a record that does not give it, [madeIn] and [at] the root's too.
 */
data class BytecodeBody(
    val body: Int,
    val instructions: List<BytecodeInstruction>,
    val kind: String? = null,
    val madeIn: Int? = null,
    val at: Int? = null,
    val arguments: Int? = null,
    val registers: Int? = null,
)

/** A body's bytecode: the root [body] first in [bodies], then the blocks it makes. */
data class Bytecode(val body: Int, val bodies: List<BytecodeBody>) : TraceRecord

/**
 * A position in machine code: the [scope] and bytecode index [at] of a
 * stretch, both null before the first position and out of line, and
 * whether it is [outOfLine], which tells those two apart.
 */
data class CodePosition(val scope: Int?, val at: Int?, val outOfLine: Boolean)

/**
 * An instruction of machine code: its [offset] in the body, its text, the
 * [scope] and bytecode index [at] of the stretch it stands in, both null
 * before the first position and out of line, whether it stands [outOfLine],
 * which tells those two apart -- false where a record does not say --,
 * what a call or jump out of it reaches, and the positions [passed] at it:
 * those instructions before it that emitted no code left beside its own,
 * in order, empty where a record does not give them.
 */
data class CodeInstruction(
    val offset: Int,
    val text: String,
    val scope: Int?,
    val at: Int?,
    val annotation: String?,
    val outOfLine: Boolean = false,
    val passed: List<CodePosition> = emptyList(),
)

/** A data word of machine code: its [offset] from the first data word and what it holds. */
data class DataWord(val offset: Int, val word: String)

/** Where a scope is inlined: the [scope] and bytecode index [at] of its send. */
data class ScopeParent(val scope: Int, val at: Int)

/** A scope of a copy or a plan: the [body] it runs, and where it is inlined, null for the root. */
data class Scope(val scope: Int, val body: Int, val parent: ScopeParent?)

/**
 * An install's machine code: its [tier] (`baseline`, `twin` or
 * `optimized`), an optimized copy's [key] and [entered] application, the
 * address it was installed at, its [base], null where a record does not
 * give it, its size in [bytes], its instructions, data words and scopes,
 * and the offset [mark]ed, null in a trace.
 */
data class Code(
    val body: Int,
    val tier: String,
    val key: String?,
    val entered: String?,
    val architecture: String,
    val base: Long?,
    val bytes: Int,
    val instructions: List<CodeInstruction>,
    val data: List<DataWord>,
    val scopes: List<Scope>,
    val mark: Int?,
) : TraceRecord

/** A type-case arm of a plan: its send, its [receiver], and either the [inlinedScope] or the kind of node it is [always] inlined as. */
data class Arm(val scope: Int, val at: Int, val receiver: String, val inlinedScope: Int?, val always: String?)

/** A callee a plan declined at a send, the [callee] null for one that is no body. */
data class Decline(val scope: Int, val at: Int, val callee: Int?, val reason: String)

/** An optimizing compile's plan, and its outcome: the bytes [installed], or the reason it was [refused]. */
data class Inlining(
    val body: Int,
    val key: String?,
    val entered: String?,
    val scopes: List<Scope>,
    val arms: List<Arm>,
    val declined: List<Decline>,
    val installed: Int?,
    val refused: String?,
) : TraceRecord

private fun parseObject(record: JsonObject): TraceRecord? = when (record.requiredText("kind")) {
    "header" -> Header(
        record.requiredInteger("version"),
        record.optionalText("architecture"),
        record.optional("categories")?.asJsonArray?.map { name -> name.asString },
        record.optional("listings")?.asJsonArray?.map { name -> name.asString },
    )
        .also { if (record.requiredText("format") != FORMAT) throw MalformedRecordException("no ns-next trace") }
    "body" -> {
        val module = record.required("module").asJsonObject
        val source = record.optional("source")?.asJsonObject
        // A record without `placed` comes from a runtime that placed every body in the text its source names.
        val inCurrentText = record.optionalText("placed") == "current"
        val placedRange = { element: JsonElement? -> if (inCurrentText) null else sourceRange(element) }
        Body(
            number = record.requiredInteger("body"),
            name = record.requiredText("name"),
            family = record.optionalInteger("family"),
            moduleId = module.requiredText("id"),
            moduleVersion = module.optionalText("version"),
            file = source?.optionalText("file"),
            library = source?.optionalText("library"),
            range = placedRange(record.optional("range")),
            checksum = record.requiredText("checksum"),
            statements = record.requiredArray("statements").map { element ->
                val statement = element.asJsonObject
                val instructions = statement.requiredArray("instructions")
                Statement(instructions[0].asInt until instructions[1].asInt, placedRange(statement.optional("range")))
            },
            sends = record.requiredArray("sends").map { element ->
                val send = element.asJsonObject
                Send(
                    at = send.requiredInteger("at"),
                    site = send.optionalInteger("site"),
                    call = placedRange(send.optional("call")),
                    selector = send.requiredArray("selector").map { token -> placedRange(token.takeUnless { value -> value.isJsonNull }) },
                )
            },
            placedInCurrentText = inCurrentText,
        )
    }
    "event" -> {
        val fields = record.deepCopy()
        for (name in listOf("kind", "category", "body", "event")) fields.remove(name)
        Event(record.requiredText("category"), record.optionalInteger("body"), record.requiredText("event"), fields)
    }
    "bytecode" -> Bytecode(
        record.requiredInteger("body"),
        record.requiredArray("bodies").map { element ->
            val listed = element.asJsonObject
            BytecodeBody(
                listed.requiredInteger("body"),
                listed.requiredArray("instructions").map { instruction ->
                    val fields = instruction.asJsonObject
                    BytecodeInstruction(fields.requiredInteger("index"), fields.optionalInteger("statement"), fields.requiredText("text"))
                },
                kind = listed.optionalText("kind"),
                madeIn = listed.optionalInteger("in"),
                at = listed.optionalInteger("at"),
                arguments = listed.optionalInteger("arguments"),
                registers = listed.optionalInteger("registers"),
            )
        },
    )
    "code" -> Code(
        body = record.requiredInteger("body"),
        tier = record.requiredText("tier"),
        key = record.optionalText("key"),
        entered = record.optionalText("entered"),
        architecture = record.requiredText("architecture"),
        base = record.optional("base")?.asLong,
        bytes = record.requiredInteger("bytes"),
        instructions = record.requiredArray("instructions").map { element ->
            val instruction = element.asJsonObject
            CodeInstruction(
                offset = instruction.requiredInteger("offset"),
                text = instruction.requiredText("text"),
                scope = instruction.optionalInteger("scope"),
                at = instruction.optionalInteger("at"),
                annotation = instruction.optionalText("annotation"),
                outOfLine = instruction.optional("out-of-line")?.asBoolean ?: false,
                passed = instruction.optional("passed")?.asJsonArray?.map { element ->
                    val position = element.asJsonObject
                    CodePosition(
                        position.optionalInteger("scope"),
                        position.optionalInteger("at"),
                        position.optional("out-of-line")?.asBoolean ?: false,
                    )
                } ?: emptyList(),
            )
        },
        data = record.requiredArray("data").map { element ->
            val word = element.asJsonObject
            DataWord(word.requiredInteger("offset"), word.requiredText("word"))
        },
        scopes = scopes(record),
        mark = record.optionalInteger("mark"),
    )
    "inlining" -> {
        val outcome = record.required("outcome").asJsonObject
        Inlining(
            body = record.requiredInteger("body"),
            key = record.optionalText("key"),
            entered = record.optionalText("entered"),
            scopes = scopes(record),
            arms = record.requiredArray("arms").map { element ->
                val arm = element.asJsonObject
                val inlined = arm.required("inlined").asJsonObject
                Arm(arm.requiredInteger("scope"), arm.requiredInteger("at"), arm.requiredText("receiver"), inlined.optionalInteger("scope"), inlined.optionalText("always"))
            },
            declined = record.requiredArray("declined").map { element ->
                val decline = element.asJsonObject
                Decline(decline.requiredInteger("scope"), decline.requiredInteger("at"), decline.optionalInteger("callee"), decline.requiredText("reason"))
            },
            installed = outcome.optionalInteger("installed"),
            refused = outcome.optionalText("refused"),
        )
    }
    else -> null
}

/** The format's name, which a header of any other file does not give. */
private const val FORMAT = "ns-next-trace"

private fun scopes(record: JsonObject): List<Scope> = record.requiredArray("scopes").map { element ->
    val scope = element.asJsonObject
    val parent = scope.optional("parent")?.asJsonArray
    Scope(scope.requiredInteger("scope"), scope.requiredInteger("body"), parent?.let { pair -> ScopeParent(pair[0].asInt, pair[1].asInt) })
}

private fun sourceRange(element: JsonElement?): SourceRange? {
    val range = element?.asJsonArray ?: return null
    if (range.size() != RANGE_SIZE) throw MalformedRecordException("a range of ${range.size()} numbers")
    return SourceRange(range[0].asInt, range[1].asInt, range[2].asInt, range[3].asInt)
}

private const val RANGE_SIZE = 4

private fun JsonObject.optional(name: String): JsonElement? = get(name)?.takeUnless { value -> value.isJsonNull }

private fun JsonObject.required(name: String): JsonElement = optional(name) ?: throw MalformedRecordException("no `$name`")

private fun JsonObject.requiredText(name: String): String = required(name).asString

private fun JsonObject.requiredInteger(name: String): Int = required(name).asInt

private fun JsonObject.requiredArray(name: String): JsonArray = required(name).asJsonArray

private fun JsonObject.optionalText(name: String): String? = optional(name)?.asString

private fun JsonObject.optionalInteger(name: String): Int? = optional(name)?.asInt
