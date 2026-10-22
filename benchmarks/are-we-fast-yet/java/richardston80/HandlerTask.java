/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class HandlerTask. Newspeak's Array counts from 1, Java's
 * from 0: the port's `workIn data at: count + 1` is `workIn.data[count]`
 * here, the same element.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
package richardston80;

/** A task that manipulates work packets and then suspends itself. */
final class HandlerTask extends Task {
  private Packet workIn;
  private Packet deviceIn;

  HandlerTask(final Scheduler taskScheduler) {
    super(taskScheduler);
  }

  @Override
  public TaskControlBlock run(final Packet packet) {
    if (packet != null) {
      if (packet.kind == Constants.KIND_WORK) {
        workIn = packet.appendTo(workIn);
      } else {
        deviceIn = packet.appendTo(deviceIn);
      }
    }
    if (workIn != null) {
      Packet queued;
      int count = workIn.datum;
      if (count < Constants.DATA_SIZE) {
        if (deviceIn != null) {
          queued = deviceIn;
          deviceIn = deviceIn.link;
          queued.datum = workIn.data[count];
          workIn.datum = count + 1;
          return scheduler.queue(queued);
        }
      } else {
        queued = workIn;
        workIn = workIn.link;
        return scheduler.queue(queued);
      }
    }
    return scheduler.suspendCurrent();
  }
}
