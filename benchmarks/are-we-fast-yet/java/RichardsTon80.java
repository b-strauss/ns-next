/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its module class with bench, which throws an
 * IllegalStateException where the port signals an Error and answers nothing
 * where the port answers the queue count; the module's slots are
 * richardston80/Constants.java, its nested classes the other files of
 * richardston80/.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
import richardston80.Constants;
import richardston80.Packet;
import richardston80.Scheduler;

public final class RichardsTon80 extends Benchmark {

  @Override
  public void bench() {
    Scheduler scheduler;
    Packet queue;
    scheduler = new Scheduler();
    scheduler.addIdleTaskPriorityWorkState(Constants.IDLE_IDENTITY, 0, null,
        Constants.IDLE_COUNT);
    queue = new Packet(null, Constants.WORKER_IDENTITY, Constants.KIND_WORK);
    queue = new Packet(queue, Constants.WORKER_IDENTITY, Constants.KIND_WORK);
    scheduler.addWorkerTaskPriorityWork(Constants.WORKER_IDENTITY, 1000, queue);
    queue = new Packet(null, Constants.DEVICE_A_IDENTITY, Constants.KIND_DEVICE);
    queue = new Packet(queue, Constants.DEVICE_A_IDENTITY, Constants.KIND_DEVICE);
    queue = new Packet(queue, Constants.DEVICE_A_IDENTITY, Constants.KIND_DEVICE);
    scheduler.addHandlerTaskPriorityWork(Constants.HANDLER_A_IDENTITY, 2000, queue);
    queue = new Packet(null, Constants.DEVICE_B_IDENTITY, Constants.KIND_DEVICE);
    queue = new Packet(queue, Constants.DEVICE_B_IDENTITY, Constants.KIND_DEVICE);
    queue = new Packet(queue, Constants.DEVICE_B_IDENTITY, Constants.KIND_DEVICE);
    scheduler.addHandlerTaskPriorityWork(Constants.HANDLER_B_IDENTITY, 3000, queue);
    scheduler.addDeviceTaskPriorityWork(Constants.DEVICE_A_IDENTITY, 4000, null);
    scheduler.addDeviceTaskPriorityWork(Constants.DEVICE_B_IDENTITY, 5000, null);
    scheduler.schedule();
    if (scheduler.queueCount != 2322) {
      throw new IllegalStateException(
          "RichardsTon80: expected queue count 2322, got " + scheduler.queueCount);
    }
    if (scheduler.holdCount != 928) {
      throw new IllegalStateException(
          "RichardsTon80: expected hold count 928, got " + scheduler.holdCount);
    }
  }
}
