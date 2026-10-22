/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class TaskControlBlock.
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
 * A task control block manages a task and the queue of work packages
 * associated with it.
 */
final class TaskControlBlock {
  public final TaskControlBlock link;
  public final int identity;
  public final int priority;
  private Packet queue;
  private final Task task;
  private int state;

  TaskControlBlock(final TaskControlBlock initialLink, final int initialIdentity,
      final int initialPriority, final Packet initialQueue, final Task initialTask) {
    link = initialLink;
    identity = initialIdentity;
    priority = initialPriority;
    queue = initialQueue;
    task = initialTask;
    state = queue == null ? Constants.STATE_SUSPENDED : Constants.STATE_SUSPENDED_RUNNABLE;
  }

  /**
   * Adds a packet to the worklist of this block's task, marks this as
   * runnable if necessary, and returns the next runnable object to run (the
   * one with the highest priority.
   */
  public TaskControlBlock addInputCheckPriority(final Packet packet,
      final TaskControlBlock oldTask) {
    if (queue == null) {
      queue = packet;
      this.markAsRunnable();
      if (priority > oldTask.priority) {
        return this;
      }
    } else {
      queue = packet.appendTo(queue);
    }
    return oldTask;
  }

  public boolean isHeldOrSuspended() {
    return !((state & Constants.STATE_HELD) == 0) || state == Constants.STATE_SUSPENDED;
  }

  public void markAsHeld() {
    state = state | Constants.STATE_HELD;
  }

  public void markAsNotHeld() {
    state = state & Constants.STATE_NOT_HELD;
  }

  public void markAsRunnable() {
    state = state | Constants.STATE_RUNNABLE;
  }

  public void markAsSuspended() {
    state = state | Constants.STATE_SUSPENDED;
  }

  /** Runs this task, if it is ready to be run, and returns the next task to run. */
  public TaskControlBlock run() {
    Packet packet = null;
    if (state == Constants.STATE_SUSPENDED_RUNNABLE) {
      packet = queue;
      queue = packet.link;
      state = queue == null ? Constants.STATE_RUNNING : Constants.STATE_RUNNABLE;
    }
    return task.run(packet);
  }

  public void setRunning() {
    state = Constants.STATE_RUNNING;
  }
}
