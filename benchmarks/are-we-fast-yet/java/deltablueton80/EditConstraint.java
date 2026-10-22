/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Planner's class EditConstraint.
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
 * I am a unary input constraint used to mark a variable that the client
 * wishes to change.
 */
final class EditConstraint extends UnaryConstraint {

  EditConstraint(final Planner enclosingPlanner, final Variable variable,
      final Strength initialStrength) {
    super(enclosingPlanner, variable, initialStrength);
    addConstraint();
  }

  /** Edit constraints do nothing. */
  @Override
  public void execute() { }

  /** I indicate that a variable is to be changed by imperative code. */
  @Override
  public boolean isInput() {
    return true;
  }
}
