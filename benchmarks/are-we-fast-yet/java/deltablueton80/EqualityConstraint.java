/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Planner's class EqualityConstraint.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
package deltablueton80;

/** I constrain two variables to have the same value: "first = second". */
final class EqualityConstraint extends BinaryConstraint {

  EqualityConstraint(final Planner enclosingPlanner, final Variable firstVariable,
      final Variable secondVariable, final Strength initialStrength) {
    super(enclosingPlanner, firstVariable, secondVariable, initialStrength);
    addConstraint();
  }

  /** Enforce this constraint. Assume that it is satisfied. */
  @Override
  public void execute() {
    output().value = input().value;
  }
}
