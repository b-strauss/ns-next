/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Strength.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
package deltablueton80;

/**
 * Strengths are used to measure the relative importance of constraints. New
 * strengths may be inserted in the strength hierarchy without disrupting
 * current constraints. Strengths cannot be created outside this class, so
 * pointer comparison can be used for value comparison.
 */
final class Strength {
  public final String name;
  public final int value;

  Strength(final String strengthName, final int strengthValue) {
    name = strengthName;
    value = strengthValue;
  }

  public boolean strongerThan(final Strength other) {
    return this.value < other.value;
  }

  public Strength strongest(final Strength other) {
    return this.strongerThan(other) ? this : other;
  }

  public boolean weakerThan(final Strength other) {
    return this.value > other.value;
  }

  public Strength weakest(final Strength other) {
    return this.weakerThan(other) ? this : other;
  }
}
