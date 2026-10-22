/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Planner's class Constraint. The port's constraints
 * reach their enclosing Planner lexically; here the Planner is a
 * constructor argument. The port's Errors are IllegalStateExceptions with
 * their messages.
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
 * I am an abstract class representing a system-maintainable relationship (or
 * "constraint") between a set of variables. I supply a strength instance
 * variable; concrete subclasses provide a means of storing the constrained
 * variables and other information required to represent a constraint.
 */
abstract class Constraint {
  protected final Planner planner;
  public final Strength strength;

  Constraint(final Planner enclosingPlanner, final Strength initialStrength) {
    planner = enclosingPlanner;
    strength = initialStrength;
  }

  /** Activate this constraint and attempt to satisfy it. */
  public void addConstraint() {
    addToGraph();
    planner.incrementalAdd(this);
  }

  /** Add myself to the constraint graph. */
  public abstract void addToGraph();

  /**
   * Decide if I can be satisfied and record that decision. The output of the
   * chosen method must not have the given mark and must have a walkabout
   * strength less than that of this constraint.
   */
  public abstract void chooseMethod(long mark);

  /**
   * Deactivate this constraint, remove it from the constraint graph, possibly
   * causing other constraints to be satisfied, and destroy it.
   */
  public void destroyConstraint() {
    if (isSatisfied()) {
      planner.incrementalRemove(this);
    }
    removeFromGraph();
  }

  /** Enforce this constraint. Assume that it is satisfied. */
  public abstract void execute();

  /**
   * Assume that I am satisfied. Answer true if all my current inputs are
   * known. A variable is known if either a) it is 'stay' (i.e. it is a
   * constant at plan execution time), b) it has the given mark (indicating
   * that it has been computed by a constraint appearing earlier in the plan),
   * or c) it is not determined by any constraint.
   */
  public abstract boolean inputsKnown(long mark);

  /**
   * Normal constraints are not input constraints. An input constraint is one
   * that depends on external state, such as the mouse, the keyboard, a clock,
   * or some arbitrary piece of imperative code.
   */
  public boolean isInput() {
    return false;
  }

  /** Answer true if this constraint is satisfied in the current solution. */
  public abstract boolean isSatisfied();

  /** Set the mark of all input from the given mark. */
  public abstract void markInputs(long mark);

  /** Record the fact that I am unsatisfied. */
  public abstract void markUnsatisfied();

  /**
   * Answer my current output variable. Raise an error if I am not currently
   * satisfied.
   */
  public abstract Variable output();

  /**
   * Calculate the walkabout strength, the stay flag, and, if it is 'stay',
   * the value for the current output of this constraint. Assume this
   * constraint is satisfied.
   */
  public abstract void recalculate();

  /** Remove myself from the constraint graph. */
  public abstract void removeFromGraph();

  /**
   * Attempt to find a way to enforce this constraint. If successful, record
   * the solution, perhaps modifying the current dataflow graph. Answer the
   * constraint that this constraint overrides, if there is one, or nil, if
   * there isn't. Assume: I am not already satisfied.
   */
  public Constraint satisfy(final long mark) {
    Variable outputVariable;
    Constraint overridden;
    chooseMethod(mark);
    if (!isSatisfied()) {
      if (strength == Constants.REQUIRED) {
        throw new IllegalStateException("Could not satisfy a required constraint");
      }
      return null;
    }
    // constraint can be satisfied
    //
    // mark inputs to allow cycle detection in addPropagate
    markInputs(mark);
    outputVariable = output();
    overridden = outputVariable.determinedBy;
    if (overridden != null) {
      overridden.markUnsatisfied();
    }
    outputVariable.determinedBy = this;
    if (!planner.addPropagateMark(this, mark)) {
      throw new IllegalStateException("Cycle encountered");
    }
    outputVariable.mark = mark;
    return overridden;
  }
}
