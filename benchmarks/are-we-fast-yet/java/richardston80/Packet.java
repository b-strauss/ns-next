/* Translated for ns-next from `RichardsTon80.ns`, itself adapted from
 * Primordial Soup's `Richards.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Packet.
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
 * A simple package of data that is manipulated by the tasks. The exact layout
 * of the payload data carried by a packet is not importaint, and neither is
 * the nature of the work performed on packets by the tasks. Besides carrying
 * data, packets form linked lists and are hence used both as data and
 * worklists.
 */
public final class Packet {
  /** The tail of the linked list of packets. */
  public Packet link;
  /** An ID for this packet. */
  public int identity;
  /** The type of this packet. */
  public int kind;
  public int datum = 0;
  public final int[] data = new int[Constants.DATA_SIZE];

  public Packet(final Packet initialLink, final int initialIdentity,
      final int initialKind) {
    link = initialLink;
    identity = initialIdentity;
    kind = initialKind;
  }

  /** Add this packet to the end of a worklist, and return the worklist. */
  public Packet appendTo(final Packet queueHead) {
    Packet peek;
    Packet next;
    link = null;
    if (queueHead == null) {
      return this;
    }
    next = queueHead;
    while ((peek = next.link) != null) {
      next = peek;
    }
    next.link = this;
    return queueHead;
  }
}
