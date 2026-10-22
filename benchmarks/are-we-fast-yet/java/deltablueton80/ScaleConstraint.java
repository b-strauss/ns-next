/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Planner's class ScaleConstraint. The port's
 * flooring division `//` is Math.floorDiv. The values stay ints: the sizes
 * are fixed at 100 and bound every value to a few thousand.
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
 * I relate two variables by the linear scaling relationship: "second = (first
 * * scale) + offset". Either first or second may be changed to maintain this
 * relationship but the scale factor and offset are considered read-only.
 */
final class ScaleConstraint extends BinaryConstraint {
  /** scale factor input variable */
  private final Variable scale;
  /** offset input variable */
  private final Variable offset;

  ScaleConstraint(final Planner enclosingPlanner, final Variable source,
      final Variable scaleVariable, final Variable offsetVariable,
      final Variable destination, final Strength initialStrength) {
    super(enclosingPlanner, source, destination, initialStrength);
    scale = scaleVariable;
    offset = offsetVariable;
    addConstraint();
  }

  /** Add myself to the constraint graph. */
  @Override
  public void addToGraph() {
    super.addToGraph();
    scale.addConstraint(this);
    offset.addConstraint(this);
  }

  /** Enforce this constraint. Assume that it is satisfied. */
  @Override
  public void execute() {
    if (direction == Constants.FORWARD) {
      second.value = first.value * scale.value + offset.value;
    } else {
      first.value = Math.floorDiv(second.value - offset.value, scale.value);
    }
  }

  /** Mark the inputs from the given mark. */
  @Override
  public void markInputs(final long mark) {
    super.markInputs(mark);
    scale.mark = mark;
    offset.mark = mark;
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
    outputVariable.stay = inputVariable.stay && (scale.stay && offset.stay);
    if (outputVariable.stay) {
      execute(); // stay optimization
    }
  }

  /** Remove myself from the constraint graph. */
  @Override
  public void removeFromGraph() {
    super.removeFromGraph();
    if (scale != null) {
      scale.removeConstraint(this);
    }
    if (offset != null) {
      offset.removeConstraint(this);
    }
  }
}
