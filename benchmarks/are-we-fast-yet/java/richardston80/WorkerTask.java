/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class WorkerTask. Newspeak's Array counts from 1, Java's
 * from 0: the port's `1 to: dataSize do:` over `packet data` is index 0 to
 * DATA_SIZE - 1 here, the same elements in the same order.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
package richardston80;

/** A task that manipulates work packets. */
final class WorkerTask extends Task {
  protected int destination;
  protected int count;

  WorkerTask(final Scheduler taskScheduler, final int initialDestination,
      final int initialCount) {
    super(taskScheduler);
    destination = initialDestination;
    count = initialCount;
  }

  @Override
  public TaskControlBlock run(final Packet packet) {
    if (packet == null) {
      return scheduler.suspendCurrent();
    }
    if (destination == Constants.HANDLER_A_IDENTITY) {
      destination = Constants.HANDLER_B_IDENTITY;
    } else {
      destination = Constants.HANDLER_A_IDENTITY;
    }
    packet.identity = destination;
    packet.datum = 0;
    for (int index = 0; index < Constants.DATA_SIZE; index++) {
      count = count + 1;
      if (count > 26) {
        count = 1;
      }
      packet.data[index] = count;
    }
    return scheduler.queue(packet);
  }
}
