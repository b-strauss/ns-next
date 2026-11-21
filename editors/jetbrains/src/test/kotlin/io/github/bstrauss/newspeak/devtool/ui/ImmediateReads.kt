package io.github.bstrauss.newspeak.devtool.ui

/** Reads and delivers a record within the call, so that no read is ever slow: what a test that is not about the threads hands a view. */
fun immediateReads(): RecordReads = RecordReads({ task -> task.run() }, { task, _, _ -> task.run() }, { _, _, _ -> })
