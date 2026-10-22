/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file holds the slots of the module class RichardsTon80, which its
 * nested classes read; they cannot stay in the top-level RichardsTon80, as a
 * package cannot import from the unnamed package.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
package richardston80;

public final class Constants {
  public static final int DATA_SIZE = 4;
  public static final int IDLE_COUNT = 1000;
  public static final int IDLE_IDENTITY = 1;
  public static final int WORKER_IDENTITY = 2;
  public static final int HANDLER_A_IDENTITY = 3;
  public static final int HANDLER_B_IDENTITY = 4;
  public static final int DEVICE_A_IDENTITY = 5;
  public static final int DEVICE_B_IDENTITY = 6;
  public static final int NUMBER_OF_IDENTITIES = 6;
  public static final int KIND_DEVICE = 0;
  public static final int KIND_WORK = 1;
  /** The task is running and is currently scheduled. */
  public static final int STATE_RUNNING = 0;
  /** The task has packets left to process. */
  public static final int STATE_RUNNABLE = 1;
  /**
   * The task is not currently running. The task is not blocked as such and
   * may be started by the scheduler.
   */
  public static final int STATE_SUSPENDED = 2;
  /** The task is blocked and cannot be run until it is explicitly released. */
  public static final int STATE_HELD = 4;
  public static final int STATE_SUSPENDED_RUNNABLE = STATE_SUSPENDED | STATE_RUNNABLE;
  public static final int STATE_NOT_HELD = ~STATE_HELD;

  private Constants() { }
}
