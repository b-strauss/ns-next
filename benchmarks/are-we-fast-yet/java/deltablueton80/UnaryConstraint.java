/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Planner's class UnaryConstraint.
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
 * I am an abstract superclass for constraints having a single possible output
 * variable.
 */
abstract class UnaryConstraint extends Constraint {
  private final Variable output;
  protected boolean satisfied = false;

  UnaryConstraint(final Planner enclosingPlanner, final Variable variable,
      final Strength initialStrength) {
    super(enclosingPlanner, initialStrength);
    output = variable;
  }

  @Override
  public Variable output() {
    return output;
  }

  /** Add myself to the constraint graph. */
  @Override
  public void addToGraph() {
    output.addConstraint(this);
    satisfied = false;
  }

  /** Decide if I can be satisfied and record that decision. */
  @Override
  public void chooseMethod(final long mark) {
    satisfied = !(output.mark == mark) && strength.strongerThan(output.walkStrength);
  }

  @Override
  public boolean inputsKnown(final long mark) {
    return true;
  }

  /** Answer true if this constraint is satisfied in the current solution. */
  @Override
  public boolean isSatisfied() {
    return satisfied;
  }

  /** I have no inputs. */
  @Override
  public void markInputs(final long mark) { }

  /** Record the fact that I am unsatisfied. */
  @Override
  public void markUnsatisfied() {
    satisfied = false;
  }

  /**
   * Calculate the walkabout strength, the stay flag, and, if it is 'stay',
   * the value for the current output of this constraint. Assume this
   * constraint is satisfied.
   */
  @Override
  public void recalculate() {
    output.walkStrength = strength;
    output.stay = !isInput();
    if (output.stay) {
      execute(); // Stay optimization
    }
  }

  /** Remove myself from the constraint graph. */
  @Override
  public void removeFromGraph() {
    if (output != null) {
      output.removeConstraint(this);
    }
    satisfied = false;
  }
}
