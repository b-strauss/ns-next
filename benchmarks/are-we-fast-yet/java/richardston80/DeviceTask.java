/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class DeviceTask.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
package richardston80;

/**
 * A task that suspends itself after each time it has been run to simulate
 * waiting for data from an external device.
 */
final class DeviceTask extends Task {
  protected Packet pending;

  DeviceTask(final Scheduler taskScheduler) {
    super(taskScheduler);
  }

  @Override
  public TaskControlBlock run(final Packet packet) {
    if (packet == null) {
      Packet functionWork;
      if (pending == null) {
        return scheduler.suspendCurrent();
      }
      functionWork = pending;
      pending = null;
      return scheduler.queue(functionWork);
    }
    pending = packet;
    return scheduler.holdCurrent();
  }
}
