package io.github.bstrauss.newspeak.debugger

import com.google.gson.JsonArray
import com.google.gson.JsonObject
import io.github.bstrauss.newspeak.debugger.dap.DapBreakpoint
import io.github.bstrauss.newspeak.debugger.dap.DapClient
import io.github.bstrauss.newspeak.debugger.dap.DapConnection
import io.github.bstrauss.newspeak.debugger.dap.DapExitedEvent
import io.github.bstrauss.newspeak.debugger.dap.DapStackFrame
import io.github.bstrauss.newspeak.debugger.dap.DapStoppedEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.net.ServerSocket
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit

/**
 * The client against the real agent: `ns-next run --debug-connect`, the
 * runtime the build names in `nsNext.runtime`, connecting to a socket the
 * test listens on, as the plugin's debug sessions will. Every wait is
 * bounded, so an agent that never answers fails the test with what the
 * runtime wrote rather than hanging it.
 */
class DapSessionTest {
    private val runtime: Path = Path.of(System.getProperty("nsNext.runtime"))

    @Test
    fun aBreakpointStopsTheRealRuntime() {
        val directory = Files.createTempDirectory("dap-session")
        val program = directory.resolve("Hello.ns")
        Files.writeString(program, HELLO)
        val standardOutput = directory.resolve("stdout.txt")
        val standardError = directory.resolve("stderr.txt")
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { serverSocket ->
            val process = ProcessBuilder(
                runtime.toString(),
                "run",
                "--debug-connect=127.0.0.1:${serverSocket.localPort}",
                program.toString(),
            )
                .redirectOutput(standardOutput.toFile())
                .redirectError(standardError.toFile())
                .start()
            fun written(): String = "stdout: ${Files.readString(standardOutput)}; stderr: ${Files.readString(standardError)}"
            try {
                val client = DapClient(DapConnection.accept(serverSocket, WAIT_MILLISECONDS))
                try {
                    val initialized = eventFuture(client, "initialized")
                    val stopped = eventFuture(client, "stopped")
                    val exited = eventFuture(client, "exited")
                    val terminated = eventFuture(client, "terminated")

                    val capabilities = awaited(
                        client.request(
                            "initialize",
                            JsonObject().apply {
                                addProperty("clientID", "ns-next plugin tests")
                                addProperty("adapterID", "ns-next")
                                addProperty("linesStartAt1", true)
                                addProperty("columnsStartAt1", true)
                                addProperty("pathFormat", "path")
                            },
                        ),
                    )
                    assertTrue(capabilities.toString(), capabilities.get("supportsConfigurationDoneRequest").asBoolean)
                    awaited(client.request("attach", JsonObject().apply { addProperty("stopOnEntry", false) }))
                    awaited(initialized)

                    val breakpoints = awaited(
                        client.request(
                            "setBreakpoints",
                            JsonObject().apply {
                                add("source", JsonObject().apply { addProperty("path", program.toAbsolutePath().toString()) })
                                add("breakpoints", JsonArray().apply { add(JsonObject().apply { addProperty("line", BREAKPOINT_LINE) }) })
                            },
                        ),
                    ).getAsJsonArray("breakpoints").map { element -> DapBreakpoint.from(element.asJsonObject) }
                    assertEquals(1, breakpoints.size)
                    assertTrue(breakpoints.toString(), breakpoints[0].verified)
                    assertEquals(BREAKPOINT_LINE, breakpoints[0].line)
                    awaited(client.request("configurationDone"))

                    val stop = DapStoppedEvent.from(awaited(stopped))
                    assertEquals("breakpoint", stop.reason)
                    assertEquals(listOfNotNull(breakpoints[0].id), stop.hitBreakpointIds)
                    val frames = awaited(client.request("stackTrace", JsonObject().apply { addProperty("threadId", stop.threadId) }))
                        .getAsJsonArray("stackFrames").map { element -> DapStackFrame.from(element.asJsonObject) }
                    val top = frames.first()
                    assertEquals("Hello>>main:args:", top.name)
                    assertEquals(BREAKPOINT_LINE, top.line)
                    assertEquals(program.toAbsolutePath().toString(), top.source?.path)
                    assertEquals("nothing is printed before the breakpoint", "", Files.readString(standardOutput))

                    awaited(client.request("continue", JsonObject().apply { addProperty("threadId", stop.threadId) }))
                    assertEquals(0, DapExitedEvent.from(awaited(exited)).exitCode)
                    awaited(terminated)
                } finally {
                    client.close()
                }
                assertTrue("ns-next ends: ${written()}", process.waitFor(WAIT_SECONDS, TimeUnit.SECONDS))
                assertEquals(written(), 0, process.exitValue())
                assertEquals("hi", Files.readString(standardOutput).trim())
            } catch (failure: Throwable) {
                throw AssertionError("${failure.message}; ${written()}", failure)
            } finally {
                process.destroyForcibly()
                process.waitFor(WAIT_SECONDS, TimeUnit.SECONDS)
                directory.toFile().deleteRecursively()
            }
        }
    }

    /** The body of the next event named `name`, listened for before the request that leads to it. */
    private fun eventFuture(client: DapClient, name: String): CompletableFuture<JsonObject> {
        val future = CompletableFuture<JsonObject>()
        client.onEvent(name) { body -> future.complete(body) }
        return future
    }

    private fun awaited(future: CompletableFuture<JsonObject>): JsonObject = future.get(WAIT_SECONDS, TimeUnit.SECONDS)

    private companion object {
        const val WAIT_SECONDS = 30L
        const val WAIT_MILLISECONDS = 30_000
        const val BREAKPOINT_LINE = 3

        /** A root whose `main:args:` prints `hi`, on line 3. */
        val HELLO = """
            class Hello = Object new () (
              public main: platform args: arguments = (
                platform transcript show: 'hi'; newLine.
              )
            ) : ()
        """.trimIndent() + "\n"
    }
}
