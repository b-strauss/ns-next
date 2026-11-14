package io.github.bstrauss.newspeak.debugger.dap

import com.google.gson.JsonObject

/*
 * The parts of DAP's bodies the plugin reads, as the agent sends them. A
 * field the agent may leave out is nullable or empty, never guessed; lines
 * and columns are 1-based, the only form the agent speaks, and columns
 * count UTF-16 code units, as the editor's offsets do.
 */

/** A program's file by its path, any other module by a reference the `source` request answers. */
data class DapSource(val name: String?, val path: String?, val sourceReference: Int?) {
    companion object {
        fun from(json: JsonObject) = DapSource(
            name = json.stringOrNull("name"),
            path = json.stringOrNull("path"),
            sourceReference = json.intOrNull("sourceReference")?.takeIf { reference -> reference > 0 },
        )
    }
}

/** One frame of a `stackTrace` response; a frame without a location has no source and line 0. */
data class DapStackFrame(
    val id: Int,
    val name: String,
    val source: DapSource?,
    val line: Int,
    val column: Int,
    val endLine: Int?,
    val endColumn: Int?,
    val presentationHint: String?,
) {
    companion object {
        fun from(json: JsonObject) = DapStackFrame(
            id = json.intOrNull("id") ?: 0,
            name = json.stringOrNull("name").orEmpty(),
            source = json.objectOrNull("source")?.let(DapSource::from),
            line = json.intOrNull("line") ?: 0,
            column = json.intOrNull("column") ?: 0,
            endLine = json.intOrNull("endLine"),
            endColumn = json.intOrNull("endColumn"),
            presentationHint = json.stringOrNull("presentationHint"),
        )
    }
}

/** One scope of a `scopes` response; [presentationHint] `locals`, or `returnValue` for a step's return value. */
data class DapScope(val name: String, val presentationHint: String?, val variablesReference: Int, val expensive: Boolean) {
    companion object {
        fun from(json: JsonObject) = DapScope(
            name = json.stringOrNull("name").orEmpty(),
            presentationHint = json.stringOrNull("presentationHint"),
            variablesReference = json.intOrNull("variablesReference") ?: 0,
            expensive = json.booleanOrNull("expensive") ?: false,
        )
    }
}

/** One variable of a `variables` response; [variablesReference] is 0 for a value without children. */
data class DapVariable(
    val name: String,
    val value: String,
    val type: String?,
    val variablesReference: Int,
    val indexedVariables: Int?,
) {
    companion object {
        fun from(json: JsonObject) = DapVariable(
            name = json.stringOrNull("name").orEmpty(),
            value = json.stringOrNull("value").orEmpty(),
            type = json.stringOrNull("type"),
            variablesReference = json.intOrNull("variablesReference") ?: 0,
            indexedVariables = json.intOrNull("indexedVariables"),
        )
    }
}

/** A breakpoint as the agent stands with it, or does not ([verified] false, [message] why). */
data class DapBreakpoint(val id: Int?, val verified: Boolean, val message: String?, val line: Int?, val column: Int?) {
    companion object {
        fun from(json: JsonObject) = DapBreakpoint(
            id = json.intOrNull("id"),
            verified = json.booleanOrNull("verified") ?: false,
            message = json.stringOrNull("message"),
            line = json.intOrNull("line"),
            column = json.intOrNull("column"),
        )
    }
}

/** The body of a `stopped` event. */
data class DapStoppedEvent(
    val reason: String,
    val description: String?,
    val text: String?,
    val threadId: Int,
    val hitBreakpointIds: List<Int>,
) {
    companion object {
        fun from(json: JsonObject) = DapStoppedEvent(
            reason = json.stringOrNull("reason").orEmpty(),
            description = json.stringOrNull("description"),
            text = json.stringOrNull("text"),
            threadId = json.intOrNull("threadId") ?: 0,
            hitBreakpointIds = json.get("hitBreakpointIds")
                ?.takeIf { element -> element.isJsonArray }
                ?.asJsonArray
                ?.mapNotNull { element -> element.takeIf { item -> item.isJsonPrimitive }?.asInt }
                .orEmpty(),
        )
    }
}

/** The body of a `breakpoint` event: [reason] `changed`, `new` or `removed`. */
data class DapBreakpointEvent(val reason: String, val breakpoint: DapBreakpoint) {
    companion object {
        fun from(json: JsonObject) = DapBreakpointEvent(
            reason = json.stringOrNull("reason").orEmpty(),
            breakpoint = DapBreakpoint.from(json.objectOrNull("breakpoint") ?: JsonObject()),
        )
    }
}

/** The body of an `output` event; the agent's own messages come in the category `console`. */
data class DapOutputEvent(val category: String?, val output: String) {
    companion object {
        fun from(json: JsonObject) = DapOutputEvent(
            category = json.stringOrNull("category"),
            output = json.stringOrNull("output").orEmpty(),
        )
    }
}

/** The body of an `exited` event. */
data class DapExitedEvent(val exitCode: Int) {
    companion object {
        fun from(json: JsonObject) = DapExitedEvent(exitCode = json.intOrNull("exitCode") ?: 0)
    }
}

/** The body of a `process` event: the runtime's process, which the plugin may end itself. */
data class DapProcessEvent(val name: String?, val systemProcessId: Long?) {
    companion object {
        fun from(json: JsonObject) = DapProcessEvent(
            name = json.stringOrNull("name"),
            systemProcessId = json.get("systemProcessId")?.takeIf { element -> element.isJsonPrimitive }?.asLong,
        )
    }
}

private fun JsonObject.intOrNull(name: String): Int? =
    get(name)?.takeIf { element -> element.isJsonPrimitive && element.asJsonPrimitive.isNumber }?.asInt

private fun JsonObject.booleanOrNull(name: String): Boolean? =
    get(name)?.takeIf { element -> element.isJsonPrimitive && element.asJsonPrimitive.isBoolean }?.asBoolean

private fun JsonObject.objectOrNull(name: String): JsonObject? =
    get(name)?.takeIf { element -> element.isJsonObject }?.asJsonObject
