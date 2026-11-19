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

/**
 * One frame of a `stackTrace` response; a frame without a location has no
 * source and line 0. [shipped] is ns-next's own `nsNext/shipped`: the
 * frame runs a body of a shipped module, a library frame. [canRestart] is
 * false only where the agent says so, as DAP reads a missing one.
 */
data class DapStackFrame(
    val id: Int,
    val name: String,
    val source: DapSource?,
    val line: Int,
    val column: Int,
    val endLine: Int?,
    val endColumn: Int?,
    val presentationHint: String?,
    val shipped: Boolean = false,
    val canRestart: Boolean = true,
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
            shipped = json.booleanOrNull("nsNext/shipped") ?: false,
            canRestart = json.booleanOrNull("canRestart") ?: true,
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

/**
 * One variable of a `variables` response, or the value an `evaluate` or a
 * `setVariable` answers; [variablesReference] is 0 for a value without
 * children. [valueReference] is ns-next's own `nsNext/valueReference`, by
 * which a mark and the referrers name the value until the program goes
 * on; a withheld value has none and is [readOnly]. A [virtual] variable
 * names a place rather than holds a value: a referrer on the stack.
 */
data class DapVariable(
    val name: String,
    val value: String,
    val type: String?,
    val variablesReference: Int,
    val indexedVariables: Int?,
    val valueReference: Int? = null,
    val readOnly: Boolean = false,
    val virtual: Boolean = false,
) {
    companion object {
        fun from(json: JsonObject) = shown(json.stringOrNull("name").orEmpty(), json.stringOrNull("value").orEmpty(), json)

        /** An `evaluate` response's value, named [name]: its text is the body's `result`. */
        fun evaluated(name: String, json: JsonObject) = shown(name, json.stringOrNull("result").orEmpty(), json)

        private fun shown(name: String, value: String, json: JsonObject): DapVariable {
            val hint = json.objectOrNull("presentationHint")
            val attributes = hint?.get("attributes")
                ?.takeIf { element -> element.isJsonArray }
                ?.asJsonArray
                ?.mapNotNull { element -> element.takeIf { item -> item.isJsonPrimitive }?.asString }
                .orEmpty()
            return DapVariable(
                name = name,
                value = value,
                type = json.stringOrNull("type"),
                variablesReference = json.intOrNull("variablesReference") ?: 0,
                indexedVariables = json.intOrNull("indexedVariables"),
                valueReference = json.intOrNull("nsNext/valueReference"),
                readOnly = "readOnly" in attributes,
                virtual = hint?.stringOrNull("kind") == "virtual",
            )
        }
    }
}

/**
 * One target of a `stepInTargets` response: a send of the line, its id
 * for `stepIn`'s `targetId`, and its span, 1-based, in UTF-16 columns.
 */
data class DapStepInTarget(val id: Int, val label: String, val line: Int?, val column: Int?, val endLine: Int?, val endColumn: Int?) {
    companion object {
        fun from(json: JsonObject) = DapStepInTarget(
            id = json.intOrNull("id") ?: 0,
            label = json.stringOrNull("label").orEmpty(),
            line = json.intOrNull("line"),
            column = json.intOrNull("column"),
            endLine = json.intOrNull("endLine"),
            endColumn = json.intOrNull("endColumn"),
        )
    }
}

/**
 * One class of an `nsNext/instanceCounts` response: its name,
 * `<id>[@<version>][/<Class>(.<Class>)*]`, which names one module's class,
 * so that `nsNext/instances` takes it alone; how many instances the object
 * space holds; and whether a shipped module declares it, which the memory
 * view marks as the core library's.
 */
data class DapInstanceCount(val name: String, val count: Int, val shipped: Boolean) {
    companion object {
        fun from(json: JsonObject) = DapInstanceCount(
            name = json.stringOrNull("name").orEmpty(),
            count = json.intOrNull("count") ?: 0,
            shipped = json.booleanOrNull("nsNext/shipped") ?: false,
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
data class DapProcessEvent(val name: String?, val systemProcessId: Long?, val secret: String?) {
    companion object {
        fun from(json: JsonObject) = DapProcessEvent(
            name = json.stringOrNull("name"),
            systemProcessId = json.get("systemProcessId")?.takeIf { element -> element.isJsonPrimitive }?.asLong,
            secret = json.stringOrNull("nsNextSecret"),
        )
    }
}

private fun JsonObject.intOrNull(name: String): Int? =
    get(name)?.takeIf { element -> element.isJsonPrimitive && element.asJsonPrimitive.isNumber }?.asInt

private fun JsonObject.booleanOrNull(name: String): Boolean? =
    get(name)?.takeIf { element -> element.isJsonPrimitive && element.asJsonPrimitive.isBoolean }?.asBoolean

private fun JsonObject.objectOrNull(name: String): JsonObject? =
    get(name)?.takeIf { element -> element.isJsonObject }?.asJsonObject
