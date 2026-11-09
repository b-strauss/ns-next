/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Plan; the port's Vector is som.Vector.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
package deltablueton80;

import som.Vector;

/**
 * A Plan is an ordered list of constraints to be executed in sequence to
 * resatisfy all currently satisfiable constraints in the face of one or more
 * changing inputs.
 */
final class Plan {
  private final Vector<Constraint> constraints = new Vector<>();

  public void addConstraint(final Constraint constraint) {
    constraints.append(constraint);
  }

  /** Execute my constraints in order. */
  public void execute() {
    constraints.forEach(constraint -> constraint.execute());
  }
}
