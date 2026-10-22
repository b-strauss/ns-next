/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Scheduler. Newspeak's Array counts from 1, Java's
 * from 0: the port's `blocks at: id` is `blocks[id - 1]` here, the same
 * element. currentIdentity, nil in the port until schedule sets it before
 * any task runs, is 0 until then.
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
 * A scheduler can be used to schedule a set of tasks based on their relative
 * priorities. Scheduling is done by maintaining a list of task control blocks
 * which holds tasks and the data queue they are processing.
 */
public final class Scheduler {
  public int queueCount = 0;
  public int holdCount = 0;
  private TaskControlBlock currentBlock;
  private int currentIdentity;
  private TaskControlBlock list;
  private final TaskControlBlock[] blocks =
      new TaskControlBlock[Constants.NUMBER_OF_IDENTITIES];

  /** Add a device task to this scheduler. */
  public void addDeviceTaskPriorityWork(final int id, final int priority,
      final Packet queue) {
    addTaskPriorityWorkTask(id, priority, queue, new DeviceTask(this));
  }

  /** Add a handler task to this scheduler. */
  public void addHandlerTaskPriorityWork(final int id, final int priority,
      final Packet queue) {
    addTaskPriorityWorkTask(id, priority, queue, new HandlerTask(this));
  }

  /** Add an idle task to this scheduler. */
  public void addIdleTaskPriorityWorkState(final int id, final int priority,
      final Packet queue, final int count) {
    addRunningTaskPriorityWorkTask(id, priority, queue,
        new IdleTask(this, 1, count));
  }

  void addRunningTaskPriorityWorkTask(final int id, final int priority,
      final Packet queue, final Task task) {
    addTaskPriorityWorkTask(id, priority, queue, task);
    currentBlock.setRunning();
  }

  void addTaskPriorityWorkTask(final int id, final int priority,
      final Packet queue, final Task task) {
    currentBlock = new TaskControlBlock(list, id, priority, queue, task);
    list = currentBlock;
    blocks[id - 1] = currentBlock;
  }

  /** Add a work task to this scheduler. */
  public void addWorkerTaskPriorityWork(final int id, final int priority,
      final Packet queue) {
    addTaskPriorityWorkTask(id, priority, queue,
        new WorkerTask(this, Constants.HANDLER_A_IDENTITY, 0));
  }

  /**
   * Block the currently executing task and return the next task control
   * block to run. The blocked task will not be made runnable until it is
   * explicitly released, even if new work is added to it.
   */
  public TaskControlBlock holdCurrent() {
    holdCount = holdCount + 1;
    currentBlock.markAsHeld();
    return currentBlock.link;
  }

  /**
   * Add the specified packet to the end of the worklist used by the task
   * associated with the packet and make the task runnable if it is currently
   * suspended.
   */
  public TaskControlBlock queue(final Packet packet) {
    TaskControlBlock block = blocks[packet.identity - 1];
    if (block == null) {
      return block;
    }
    queueCount = queueCount + 1;
    packet.link = null;
    packet.identity = currentIdentity;
    return block.addInputCheckPriority(packet, currentBlock);
  }

  /** Release a task that is currently blocked and return the next block to run. */
  public TaskControlBlock release(final int id) {
    TaskControlBlock block = blocks[id - 1];
    if (block == null) {
      return block;
    }
    block.markAsNotHeld();
    if (block.priority > currentBlock.priority) {
      return block;
    }
    return currentBlock;
  }

  /** Execute the tasks managed by this scheduler. */
  public void schedule() {
    currentBlock = list;
    while (currentBlock != null) {
      if (currentBlock.isHeldOrSuspended()) {
        currentBlock = currentBlock.link;
      } else {
        currentIdentity = currentBlock.identity;
        currentBlock = currentBlock.run();
      }
    }
  }

  /**
   * Suspend the currently executing task and return the next task control
   * block to run. If new work is added to the suspended task it will be made
   * runnable.
   */
  public TaskControlBlock suspendCurrent() {
    currentBlock.markAsSuspended();
    return currentBlock;
  }
}
