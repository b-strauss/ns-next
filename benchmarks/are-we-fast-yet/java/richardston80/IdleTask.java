/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class IdleTask. The port's `control // 2` is
 * `Math.floorDiv(control, 2)`, Newspeak's flooring division.
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
 * An idle task doesn't do any work itself but cycles control between the two
 * device tasks.
 */
final class IdleTask extends Task {
  /** A seed value that controls how the device tasks are scheduled. */
  protected int control;
  /** The number of times this task should be scheduled. */
  protected int count;

  IdleTask(final Scheduler taskScheduler, final int initialControl,
      final int initialCount) {
    super(taskScheduler);
    control = initialControl;
    count = initialCount;
  }

  @Override
  public TaskControlBlock run(final Packet packet) {
    count = count - 1;
    if (count == 0) {
      return scheduler.holdCurrent();
    }
    if ((control & 1) == 0) {
      control = Math.floorDiv(control, 2);
      return scheduler.release(Constants.DEVICE_A_IDENTITY);
    }
    control = Math.floorDiv(control, 2) ^ 0xD008;
    return scheduler.release(Constants.DEVICE_B_IDENTITY);
  }
}
