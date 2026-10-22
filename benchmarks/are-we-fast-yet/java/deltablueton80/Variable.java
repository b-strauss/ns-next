/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Planner's class Variable; the port's List is
 * som.Vector, whose remove takes out the constraint as the port's
 * `remove:ifAbsent:` does (a constraint is on a variable's list at most
 * once). A mark is a long, as Planner's currentMark is.
 * The one known difference in work: som.Vector's remove, upstream's
 * implementation, copies the remaining constraints into a new array on each
 * removal, where the port's List remove:ifAbsent: shifts them in place.
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
 * I represent a constrained variable. In addition to my value, I maintain the
 * structure of the constraint graph, the current dataflow graph, and various
 * parameters of interest to the DeltaBlue incremental constraint solver.
 */
final class Variable {
  public int value;
  public final Vector<Constraint> constraints = new Vector<>(2);
  public Constraint determinedBy;
  public long mark = 0;
  public Strength walkStrength = Constants.WEAKEST;
  public boolean stay = true;
  public final String name;

  Variable(final String variableName, final int initialValue) {
    value = initialValue;
    name = variableName;
  }

  /** Add the given constraint to the set of all constraints that refer to me. */
  public void addConstraint(final Constraint constraint) {
    constraints.append(constraint);
  }

  /** Remove all traces of the given constraint from this variable. */
  public void removeConstraint(final Constraint constraint) {
    constraints.remove(constraint);
    if (determinedBy == constraint) {
      determinedBy = null;
    }
  }
}
