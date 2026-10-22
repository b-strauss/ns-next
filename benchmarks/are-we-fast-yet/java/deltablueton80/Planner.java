/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its class Planner; its nested classes are the other files of
 * deltablueton80/. The port's List is som.Vector, its `add:` append, and
 * `at: index + 1` at(index), the same element of a list counted from 0. A
 * mark is a long: one Planner serves every run of the benchmark, so
 * currentMark grows with the number of runs, without bound. The port's
 * Errors are IllegalStateExceptions with their messages.
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
 * I embody the DeltaBlue algorithm described in: ''The DeltaBlue Algorithm:
 * An Incremental Constraint Hierarchy Solver'' by Bjorn N. Freeman-Benson and
 * John Maloney. See January 1990 Communications of the ACM or University of
 * Washington TR 89-08-06 for further details.
 */
public final class Planner {
  private long currentMark = 0;

  public void addConstraintsConsumingTo(final Variable variable,
      final Vector<Constraint> list) {
    Constraint determining = variable.determinedBy;
    variable.constraints.forEach(constraint -> {
      if (!(constraint == determining) && constraint.isSatisfied()) {
        list.append(constraint);
      }
    });
  }

  /**
   * Recompute the walkabout strengths and stay flags of all variables
   * downstream of the given constraint and recompute the actual values of all
   * variables whose stay flag is true. If a cycle is detected, remove the
   * given constraint and answer false. Otherwise, answer true.
   */
  public boolean addPropagateMark(final Constraint constraint, final long mark) {
    // Details: Cycles are detected when a marked variable is encountered
    // downstream of the given constraint. The sender is assumed to have
    // marked the inputs of the given constraint with the given mark. Thus,
    // encountering a marked node downstream of the output constraint means
    // that there is a path from the constraint's output to one of its inputs.
    Vector<Constraint> todo = new Vector<>();
    todo.append(constraint);
    while (!todo.isEmpty()) {
      Constraint current = todo.removeLast();
      if (current.output().mark == mark) {
        incrementalRemove(constraint);
        return false;
      }
      current.recalculate();
      addConstraintsConsumingTo(current.output(), todo);
    }
    return true;
  }

  /**
   * This is the standard DeltaBlue benchmark. A long chain of equality
   * constraints is constructed with a stay constraint on one end. An edit
   * constraint is then added to the opposite end and the time is measured
   * for adding and removing this constraint, and extracting and executing a
   * constraint satisfaction plan. There are two cases. In case 1, the added
   * constraint is stronger than the stay constraint and values must
   * propagate down the entire length of the chain. In case 2, the added
   * constraint is weaker than the stay constraint so it cannot be
   * accommodated. The cost in this case is, of course, very low. Typical
   * situations lie somewhere between these two extremes.
   */
  public void chainTest(final int count) {
    Variable previous = null;
    Variable first = null;
    Variable last = null;
    Constraint editConstraint;
    Vector<Constraint> editConstraints;
    Plan plan;
    for (int index = 1; index <= count; index++) {
      String name;
      Variable variable;
      name = "v" + index;
      variable = new Variable(name, 0);
      if (previous != null) {
        new EqualityConstraint(this, previous, variable, Constants.REQUIRED);
      }
      if (index == 1) {
        first = variable;
      }
      if (index == count) {
        last = variable;
      }
      previous = variable;
    }
    new StayConstraint(this, last, Constants.STRONG_DEFAULT);
    editConstraint = new EditConstraint(this, first, Constants.PREFERRED);
    editConstraints = new Vector<>();
    editConstraints.append(editConstraint);
    plan = extractPlanFromConstraints(editConstraints);
    for (int index = 1; index <= count; index++) {
      first.value = index;
      plan.execute();
      if (!(last.value == index)) {
        throw new IllegalStateException("Chain test failed!");
      }
    }
    editConstraint.destroyConstraint();
  }

  /**
   * Extract a plan for resatisfaction starting from the outputs of the given
   * constraints, usually a set of input constraints.
   */
  public Plan extractPlanFromConstraints(final Vector<Constraint> constraints) {
    Vector<Constraint> sources = new Vector<>();
    constraints.forEach(constraint -> {
      if (constraint.isInput() && constraint.isSatisfied()) {
        sources.append(constraint);
      }
    });
    return makePlan(sources);
  }

  /**
   * Attempt to satisfy the given constraint and, if successful, incrementally
   * update the dataflow graph. Details: If satisfying the constraint is
   * successful, it may override a weaker constraint on its output. The
   * algorithm attempts to resatisfy that constraint using some other method.
   * This process is repeated until either a) it reaches a variable that was
   * not previously determined by any constraint or b) it reaches a constraint
   * that is too weak to be satisfied using any of its methods. The variables
   * of constraints that have been processed are marked with a unique mark
   * value so that we know where we've been. This allows the algorithm to
   * avoid getting into an infinite loop even if the constraint graph has an
   * inadvertent cycle.
   */
  public void incrementalAdd(final Constraint constraint) {
    long mark = newMark();
    Constraint overridden = constraint.satisfy(mark);
    while (overridden != null) {
      overridden = overridden.satisfy(mark);
    }
  }

  /**
   * Entry point for retracting a constraint. Remove the given constraint and
   * incrementally update the dataflow graph.
   */
  public void incrementalRemove(final Constraint constraint) {
    // Details: Retracting the given constraint may allow some currently
    // unsatisfiable downstream constraint to be satisfied. We therefore
    // collect a list of unsatisfied downstream constraints and attempt to
    // satisfy each one in turn. This list is traversed by constraint
    // strength, strongest first, as a heuristic for avoiding unnecessarily
    // adding and then overriding weak constraints. Assume: the given
    // constraint is satisfied.
    Variable outputVariable;
    Vector<Constraint> unsatisfied;
    outputVariable = constraint.output();
    constraint.markUnsatisfied();
    constraint.removeFromGraph();
    unsatisfied = removePropagateFrom(outputVariable);
    for (Strength strength : Constants.DESCENDING_STRENGTHS) {
      unsatisfied.forEach(each -> {
        if (each.strength == strength) {
          incrementalAdd(each);
        }
      });
    }
  }

  /**
   * Extract a plan for resatisfaction starting from the given source
   * constraints, usually a set of input constraints. This method assumes that
   * stay optimization is desired; the plan will contain only constraints
   * whose output variables are not stay. Constraints that do no computation,
   * such as stay and edit constraints, are not included in the plan.
   */
  public Plan makePlan(final Vector<Constraint> sources) {
    // Details: The outputs of a constraint are marked when it is added to the
    // plan under construction. A constraint may be appended to the plan when
    // all its input variables are known. A variable is known if either a) the
    // variable is marked (indicating that has been computed by a constraint
    // appearing earlier in the plan), b) the variable is 'stay' (i.e. it is a
    // constant at plan execution time), or c) the variable is not determined
    // by any constraint. The last provision is for past states of history
    // variables, which are not stay but which are also not computed by any
    // constraint. Assume: sources are all satisfied.
    long mark = newMark();
    Plan plan = new Plan();
    Vector<Constraint> todo = sources;
    while (!todo.isEmpty()) {
      Constraint constraint = todo.removeLast();
      if (!(constraint.output().mark == mark) && constraint.inputsKnown(mark)) {
        // not in plan already and eligible for inclusion
        plan.addConstraint(constraint);
        constraint.output().mark = mark;
        addConstraintsConsumingTo(constraint.output(), todo);
      }
    }
    return plan;
  }

  /** Select a previously unused mark value. */
  public long newMark() {
    currentMark = currentMark + 1;
    return currentMark;
  }

  /**
   * This test constructs a two sets of variables related to each other by a
   * simple linear transformation (scale and offset). The time is measured to
   * change a variable on either side of the mapping and to change the scale
   * and offset factors.
   */
  public void projectionTest(final int count) {
    Variable scale;
    Variable offset;
    Variable source = null;
    Variable destination = null;
    Vector<Variable> destinations;
    scale = new Variable("scale", 10);
    offset = new Variable("offset", 1000);
    destinations = new Vector<>();
    for (int index = 0; index <= count - 1; index++) {
      source = new Variable("src" + index, index);
      destination = new Variable("dst" + index, index);
      destinations.append(destination);
      new StayConstraint(this, source, Constants.NORMAL);
      new ScaleConstraint(this, source, scale, offset, destination, Constants.REQUIRED);
    }
    setValueOfTo(source, 17);
    if (!(destination.value == 1170)) {
      throw new IllegalStateException("Projection test 1 failed!");
    }
    setValueOfTo(destination, 1050);
    if (!(source.value == 5)) {
      throw new IllegalStateException("Projection test 2 failed!");
    }
    setValueOfTo(scale, 5);
    for (int index = 0; index <= count - 2; index++) {
      if (!(destinations.at(index).value == index * 5 + 1000)) {
        throw new IllegalStateException("Projection test 3 failed!");
      }
    }
    setValueOfTo(offset, 2000);
    for (int index = 0; index <= count - 2; index++) {
      if (!(destinations.at(index).value == index * 5 + 2000)) {
        throw new IllegalStateException("Projection test 4 failed!");
      }
    }
  }

  /** The given variable has changed. Propagate new values downstream. */
  public void propagateFrom(final Variable variable) {
    Vector<Constraint> todo = new Vector<>();
    addConstraintsConsumingTo(variable, todo);
    while (!todo.isEmpty()) {
      Constraint constraint = todo.removeLast();
      constraint.execute();
      addConstraintsConsumingTo(constraint.output(), todo);
    }
  }

  /**
   * Update the walkabout strengths and stay flags of all variables downstream
   * of the given constraint. Answer a collection of unsatisfied constraints
   * sorted in order of decreasing strength.
   */
  public Vector<Constraint> removePropagateFrom(final Variable outputVariable) {
    Vector<Constraint> unsatisfied;
    Vector<Variable> todo;
    outputVariable.determinedBy = null;
    outputVariable.walkStrength = Constants.WEAKEST;
    outputVariable.stay = true;
    unsatisfied = new Vector<>();
    todo = new Vector<>();
    todo.append(outputVariable);
    while (!todo.isEmpty()) {
      Variable variable = todo.removeLast();
      variable.constraints.forEach(constraint -> {
        if (!constraint.isSatisfied()) {
          unsatisfied.append(constraint);
        }
      });
      Constraint determining = variable.determinedBy;
      variable.constraints.forEach(next -> {
        if (!(next == determining) && next.isSatisfied()) {
          next.recalculate();
          todo.append(next.output());
        }
      });
    }
    return unsatisfied;
  }

  public void setValueOfTo(final Variable variable, final int newValue) {
    Constraint editConstraint;
    Vector<Constraint> editConstraints;
    Plan plan;
    editConstraint = new EditConstraint(this, variable, Constants.PREFERRED);
    editConstraints = new Vector<>();
    editConstraints.append(editConstraint);
    plan = extractPlanFromConstraints(editConstraints);
    for (int repetition = 0; repetition < 10; repetition++) {
      variable.value = newValue;
      plan.execute();
    }
    editConstraint.destroyConstraint();
  }
}
