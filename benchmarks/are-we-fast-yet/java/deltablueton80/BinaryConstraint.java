/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Planner's class BinaryConstraint; chooseMethod,
 * protected in the port, is public, as Constraint declares it.
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
 * I am an abstract superclass for constraints having two possible output
 * variables.
 */
abstract class BinaryConstraint extends Constraint {
  protected final Variable first;
  protected final Variable second;
  protected Direction direction;

  BinaryConstraint(final Planner enclosingPlanner, final Variable firstVariable,
      final Variable secondVariable, final Strength initialStrength) {
    super(enclosingPlanner, initialStrength);
    first = firstVariable;
    second = secondVariable;
  }

  /** Add myself to the constraint graph. */
  @Override
  public void addToGraph() {
    first.addConstraint(this);
    second.addConstraint(this);
    direction = null;
  }

  /**
   * Decide if I can be satisfied and which way I should flow based on the
   * relative strength of the variables I relate, and record that decision.
   */
  @Override
  public void chooseMethod(final long mark) {
    if (first.mark == mark) {
      if (!(second.mark == mark) && strength.strongerThan(second.walkStrength)) {
        direction = Constants.FORWARD;
      } else {
        direction = null;
      }
      return;
    }
    if (second.mark == mark) {
      if (!(first.mark == mark) && strength.strongerThan(first.walkStrength)) {
        direction = Constants.BACKWARD;
      } else {
        direction = null;
      }
      return;
    }
    // If we get here, neither variable is marked, so we have a choice.
    if (first.walkStrength.weakerThan(second.walkStrength)) {
      direction = strength.strongerThan(first.walkStrength) ? Constants.BACKWARD : null;
    } else {
      direction = strength.strongerThan(second.walkStrength)
          ? Constants.FORWARD
          // nil
          : Constants.BACKWARD;
    }
  }

  /** Answer my current input variable */
  public Variable input() {
    return direction == Constants.FORWARD ? first : second;
  }

  @Override
  public boolean inputsKnown(final long mark) {
    Variable inputVariable = input();
    return inputVariable.mark == mark || (inputVariable.stay || inputVariable.determinedBy == null);
  }

  /** Answer true if this constraint is satisfied in the current solution. */
  @Override
  public boolean isSatisfied() {
    return !(null == direction);
  }

  /** Mark the input variable with the given mark. */
  @Override
  public void markInputs(final long mark) {
    input().mark = mark;
  }

  /** Record the fact that I am unsatisfied. */
  @Override
  public void markUnsatisfied() {
    direction = null;
  }

  /** Answer my current output variable. */
  @Override
  public Variable output() {
    return direction == Constants.FORWARD ? second : first;
  }

  /**
   * Calculate the walkabout strength, the stay flag, and, if it is 'stay',
   * the value for the current output of this constraint. Assume this
   * constraint is satisfied.
   */
  @Override
  public void recalculate() {
    Variable inputVariable = input();
    Variable outputVariable = output();
    outputVariable.walkStrength = strength.weakest(inputVariable.walkStrength);
    outputVariable.stay = inputVariable.stay;
    if (outputVariable.stay) {
      execute();
    }
  }

  /** Remove myself from the constraint graph. */
  @Override
  public void removeFromGraph() {
    if (first != null) {
      first.removeConstraint(this);
    }
    if (second != null) {
      second.removeConstraint(this);
    }
    direction = null;
  }
}
