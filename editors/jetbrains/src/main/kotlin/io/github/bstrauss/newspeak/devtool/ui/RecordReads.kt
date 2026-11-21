package io.github.bstrauss.newspeak.devtool.ui

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.util.Condition
import com.intellij.util.concurrency.AppExecutorUtil
import io.github.bstrauss.newspeak.devtool.model.UnreadableRecordException
import java.util.concurrent.TimeUnit

/** How long a read may take before a view says that it is reading: shorter reads show nothing in between. */
const val SLOW_READ_MILLISECONDS = 300L

/**
 * How a view reads a listing or a plan back from its run's trace file
 * (`docs/design/developer-tool.md`, decision 34): the read on a pooled
 * thread, since one record's line can be a few MiB and the EDT is never
 * held by a read, and its outcome handed to the view on the EDT, in the
 * modality the request was made in. A read is made only while it is
 * still [read]'s `wanted`, looked at on the pooled thread before it reads
 * and on the EDT before it delivers, so that a view that moved on, or is
 * gone, neither waits for a read nor shows a stale one; so `wanted` reads
 * only what either thread may read.
 *
 * [background] runs the read, [foreground] the delivery, and [later] a
 * task on the EDT once [SLOW_READ_MILLISECONDS] have passed, each given
 * the modality and the condition that expires it; tests hand in their own.
 */
class RecordReads(
    private val background: (Runnable) -> Unit,
    private val foreground: (Runnable, ModalityState, Condition<Any?>) -> Unit,
    private val later: (Runnable, ModalityState, Condition<Any?>) -> Unit,
) {
    /**
     * Reads [read] off the EDT and hands its outcome to [deliver] on it --
     * the record, or the [UnreadableRecordException] that says why the file
     * no longer gives it -- unless it is no longer [wanted]; and calls
     * [slow], if given, where the outcome has not been delivered after
     * [SLOW_READ_MILLISECONDS], so that a view says it is reading only for
     * a read that takes long. Called on the EDT.
     */
    fun <T> read(wanted: () -> Boolean, read: () -> T, slow: (() -> Unit)? = null, deliver: (Result<T>) -> Unit) {
        val modality = ModalityState.defaultModalityState()
        val expired = Condition<Any?> { !wanted() }
        // Read and written on the EDT alone: the delivery and the notice both run there.
        var delivered = false
        if (slow != null) later(Runnable { if (!delivered && wanted()) slow() }, modality, expired)
        background(
            Runnable {
                if (!wanted()) return@Runnable
                val outcome = try {
                    Result.success(read())
                } catch (exception: UnreadableRecordException) {
                    Result.failure(exception)
                }
                foreground(
                    Runnable {
                        delivered = true
                        if (wanted()) deliver(outcome)
                    },
                    modality,
                    expired,
                )
            },
        )
    }

    companion object {
        /** Reads on the application's pooled threads and delivers through `invokeLater`. */
        fun pooled(): RecordReads = RecordReads(
            { task -> AppExecutorUtil.getAppExecutorService().execute(task) },
            { task, modality, expired -> ApplicationManager.getApplication().invokeLater(task, modality, expired) },
            { task, modality, expired ->
                AppExecutorUtil.getAppScheduledExecutorService().schedule(
                    { ApplicationManager.getApplication().invokeLater(task, modality, expired) },
                    SLOW_READ_MILLISECONDS,
                    TimeUnit.MILLISECONDS,
                )
            },
        )
    }
}
