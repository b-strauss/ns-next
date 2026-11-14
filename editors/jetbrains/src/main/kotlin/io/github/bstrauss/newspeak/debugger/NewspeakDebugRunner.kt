package io.github.bstrauss.newspeak.debugger

import com.intellij.execution.ExecutionException
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.configurations.RunProfile
import com.intellij.execution.configurations.RunProfileState
import com.intellij.execution.configurations.RunnerSettings
import com.intellij.execution.executors.DefaultDebugExecutor
import com.intellij.execution.process.KillableColoredProcessHandler
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.execution.runners.GenericProgramRunner
import com.intellij.execution.ui.RunContentDescriptor
import com.intellij.xdebugger.XDebugProcess
import com.intellij.xdebugger.XDebugProcessStarter
import com.intellij.xdebugger.XDebugSession
import com.intellij.xdebugger.XDebuggerManager
import io.github.bstrauss.newspeak.run.NewspeakCommand
import io.github.bstrauss.newspeak.run.NewspeakRunConfiguration
import io.github.bstrauss.newspeak.run.NewspeakRunState
import java.io.IOException
import java.net.InetAddress
import java.net.ServerSocket
import java.security.SecureRandom

/**
 * Debug on a Newspeak run configuration that runs a program: the plugin
 * listens on a loopback port of its own and starts `ns-next run
 * --debug-connect` at it, as IntelliJ starts a JVM with `server=n`, so that
 * no port of the program is open to another process and nothing is read
 * out of its standard error (`docs/design/debugger.md`, "Launching, the
 * start and the end"). A benchmark is not debugged: `bench` takes no
 * debugger.
 */
class NewspeakDebugRunner : GenericProgramRunner<RunnerSettings>() {
    override fun getRunnerId(): String = "NewspeakDebugRunner"

    override fun canRun(executorId: String, profile: RunProfile): Boolean =
        executorId == DefaultDebugExecutor.EXECUTOR_ID &&
            profile is NewspeakRunConfiguration &&
            profile.command == NewspeakCommand.RUN

    override fun doExecute(state: RunProfileState, environment: ExecutionEnvironment): RunContentDescriptor? {
        val runState = state as NewspeakRunState
        // 127.0.0.1 by its address, as the runtime is told it: `localhost` may resolve to ::1.
        val serverSocket = try {
            ServerSocket(0, 1, InetAddress.getByAddress(LOOPBACK))
        } catch (exception: IOException) {
            throw ExecutionException(exception)
        }
        try {
            val secret = newDebugSecret()
            runState.debugAddress = "127.0.0.1:${serverSocket.localPort}"
            runState.debugSecret = secret
            val result = runState.execute(environment.executor, this)
            val handler = result.processHandler as NewspeakDebuggeeProcessHandler
            val starter = object : XDebugProcessStarter() {
                override fun start(session: XDebugSession): XDebugProcess =
                    NewspeakDebugProcess(session, serverSocket, handler, result.executionConsole, secret)
            }
            return XDebuggerManager.getInstance(environment.project)
                .newSessionBuilder(starter)
                .environment(environment)
                .startSession()
                .runContentDescriptor
        } catch (failure: Throwable) {
            serverSocket.close()
            throw failure
        }
    }

    private companion object {
        val LOOPBACK = byteArrayOf(127, 0, 0, 1)
    }
}

/** The bytes of a debug session's secret: 128 bits, as the listening mode's token has. */
private const val SECRET_BYTES = 16

/**
 * A one-time secret for one debugged process, as lowercase hex digits: the
 * runtime returns it in its `process` event, which tells the process the
 * plugin started from any other that connects to its port first and names
 * the started process's id, which is no secret.
 */
internal fun newDebugSecret(): String {
    val bytes = ByteArray(SECRET_BYTES)
    SecureRandom().nextBytes(bytes)
    return bytes.joinToString("") { byte -> "%02x".format(byte) }
}

/**
 * The debugged runtime's process. The platform's Stop destroys the
 * process, and its second Stop on a process still terminating kills it
 * ([killProcess]); while a debugger is attached, the first asks the
 * program to end instead, as its own `terminate…` would, its `ensure:`
 * blocks run -- which a program inside one long leaf reaches only once the
 * leaf returns, so the second Stop is what ends that one at once.
 */
class NewspeakDebuggeeProcessHandler(commandLine: GeneralCommandLine) : KillableColoredProcessHandler(commandLine) {
    /**
     * Asks the program to end through the debugger, answering whether it
     * could ask; null while no debugger is attached, when there is nobody
     * to ask and the process is killed.
     */
    @Volatile
    var terminateThroughDebugger: (() -> Boolean)? = null

    override fun doDestroyProcess() {
        if (terminateThroughDebugger?.invoke() == true) return
        super.doDestroyProcess()
    }
}
