/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/deltablue/Planner.java).
 * Changed for ns-next: the checks of chainTest and projectionTest throw
 * IllegalStateException with the messages of
 * benchmarks/are-we-fast-yet/DeltaBlue.ns ('Chain test failed!!' and
 * 'Projection test 1 failed!!' to 'Projection test 4 failed!!') where
 * upstream throws RuntimeException with one exclamation mark; names are
 * written out; the parameters of addConstraintsConsumingTo and the edit
 * constraint's declaration in chainTest, longer for the names, are split over
 * two lines.
 */
/*
 * This benchmark is derived from Mario Wolczko's Java and Smalltalk version of
 * DeltaBlue.
 *
 * It is modified to use the SOM class library and Java 8 features.
 * License details:
 *   http://web.archive.org/web/20050825101121/http://www.sunlabs.com/people/mario/java_benchmarking/index.html
 */
package deltablue;

import java.util.Arrays;

import som.ForEachInterface;
import som.Vector;

public final class Planner {
  private int currentMark;

  public Planner() {
    currentMark = 1;
  }

  // Attempt to satisfy the given constraint and, if successful,
  // incrementally update the dataflow graph. Details: If satifying
  // the constraint is successful, it may override a weaker constraint
  // on its output. The algorithm attempts to resatisfy that
  // constraint using some other method. This process is repeated
  // until either a) it reaches a variable that was not previously
  // determined by any constraint or b) it reaches a constraint that
  // is too weak to be satisfied using any of its methods. The
  // variables of constraints that have been processed are marked with
  // a unique mark value so that we know where we've been. This allows
  // the algorithm to avoid getting into an infinite loop even if the
  // constraint graph has an inadvertent cycle.
  //
  public void incrementalAdd(final AbstractConstraint constraint) {
    int mark = newMark();
    AbstractConstraint overridden = constraint.satisfy(mark, this);

    while (overridden != null) {
      overridden = overridden.satisfy(mark, this);
    }
  }

  // Entry point for retracting a constraint. Remove the given
  // constraint and incrementally update the dataflow graph.
  // Details: Retracting the given constraint may allow some currently
  // unsatisfiable downstream constraint to be satisfied. We therefore collect
  // a list of unsatisfied downstream constraints and attempt to
  // satisfy each one in turn. This list is traversed by constraint
  // strength, strongest first, as a heuristic for avoiding
  // unnecessarily adding and then overriding weak constraints.
  // Assume: c is satisfied.
  //
  public void incrementalRemove(final AbstractConstraint constraint) {
    Variable outputVariable = constraint.getOutput();
    constraint.markUnsatisfied();
    constraint.removeFromGraph();

    Vector<AbstractConstraint> unsatisfied = removePropagateFrom(outputVariable);
    unsatisfied.forEach(each -> incrementalAdd(each));
  }

  // Extract a plan for resatisfaction starting from the outputs of
  // the given constraints, usually a set of input constraints.
  //
  protected Plan extractPlanFromConstraints(final Vector<AbstractConstraint> constraints) {
    Vector<AbstractConstraint> sources = new Vector<>();

    constraints.forEach(constraint -> {
      if (constraint.isInput() && constraint.isSatisfied()) {
        sources.append(constraint);
      }});

    return makePlan(sources);
  }

  // Extract a plan for resatisfaction starting from the given source
  // constraints, usually a set of input constraints. This method
  // assumes that stay optimization is desired; the plan will contain
  // only constraints whose output variables are not stay. Constraints
  // that do no computation, such as stay and edit constraints, are
  // not included in the plan.
  // Details: The outputs of a constraint are marked when it is added
  // to the plan under construction. A constraint may be appended to
  // the plan when all its input variables are known. A variable is
  // known if either a) the variable is marked (indicating that has
  // been computed by a constraint appearing earlier in the plan), b)
  // the variable is 'stay' (i.e. it is a constant at plan execution
  // time), or c) the variable is not determined by any
  // constraint. The last provision is for past states of history
  // variables, which are not stay but which are also not computed by
  // any constraint.
  // Assume: sources are all satisfied.
  //
  protected Plan makePlan(final Vector<AbstractConstraint> sources) {
    int mark = newMark();
    Plan plan = new Plan();
    Vector<AbstractConstraint> todo = sources;

    while (!todo.isEmpty()) {
      AbstractConstraint constraint = todo.removeFirst();

      if (constraint.getOutput().getMark() != mark && constraint.inputsKnown(mark)) {
        // not in plan already and eligible for inclusion
        plan.append(constraint);
        constraint.getOutput().setMark(mark);
        addConstraintsConsumingTo(constraint.getOutput(), todo);
      }
    }
    return plan;
  }

  // The given variable has changed. Propagate new values downstream.
  public void propagateFrom(final Variable variable) {
    Vector<AbstractConstraint> todo = new Vector<>();
    addConstraintsConsumingTo(variable, todo);

    while (!todo.isEmpty()) {
      AbstractConstraint constraint = todo.removeFirst();
      constraint.execute();
      addConstraintsConsumingTo(constraint.getOutput(), todo);
    }
  }

  protected void addConstraintsConsumingTo(final Variable variable,
      final Vector<AbstractConstraint> collection) {
    AbstractConstraint determining = variable.getDeterminedBy();

    variable.getConstraints().forEach(constraint -> {
      if (constraint != determining && constraint.isSatisfied()) {
        collection.append(constraint);
      }
    });
  }

  // Recompute the walkabout strengths and stay flags of all variables
  // downstream of the given constraint and recompute the actual
  // values of all variables whose stay flag is true. If a cycle is
  // detected, remove the given constraint and answer
  // false. Otherwise, answer true.
  // Details: Cycles are detected when a marked variable is
  // encountered downstream of the given constraint. The sender is
  // assumed to have marked the inputs of the given constraint with
  // the given mark. Thus, encountering a marked node downstream of
  // the output constraint means that there is a path from the
  // constraint's output to one of its inputs.
  //
  public boolean addPropagate(final AbstractConstraint constraint, final int mark) {
    Vector<AbstractConstraint> todo = Vector.with(constraint);

    while (!todo.isEmpty()) {
      AbstractConstraint current = todo.removeFirst();

      if (current.getOutput().getMark() == mark) {
        incrementalRemove(constraint);
        return false;
      }
      current.recalculate();
      addConstraintsConsumingTo(current.getOutput(), todo);
    }
    return true;
  }

  private void change(final Variable variable, final int newValue) {
    EditConstraint editConstraint = new EditConstraint(variable, Strength.PREFERRED, this);

    Vector<AbstractConstraint> editConstraints = Vector.with(editConstraint);
    Plan plan = extractPlanFromConstraints(editConstraints);
    for (int index = 0; index < 10; index++) {
      variable.setValue(newValue);
      plan.execute();
    }
    editConstraint.destroyConstraint(this);
  }

  private void constraintsConsuming(final Variable variable,
      final ForEachInterface<AbstractConstraint> function) {
    AbstractConstraint determining = variable.getDeterminedBy();
    variable.getConstraints().forEach(constraint -> {
      if (constraint != determining && constraint.isSatisfied()) {
        function.apply(constraint);
      }
    });
  }

  // Select a previously unused mark value.
  private int newMark() {
    currentMark++;
    return currentMark;
  }

  // Update the walkabout strengths and stay flags of all variables
  // downstream of the given constraint. Answer a collection of
  // unsatisfied constraints sorted in order of decreasing strength.
  protected Vector<AbstractConstraint> removePropagateFrom(final Variable outputVariable) {
    Vector<AbstractConstraint> unsatisfied = new Vector<>();

    outputVariable.setDeterminedBy(null);
    outputVariable.setWalkStrength(Strength.absoluteWeakest());
    outputVariable.setStay(true);

    Vector<Variable> todo = Vector.with(outputVariable);

    while (!todo.isEmpty()) {
      Variable variable = todo.removeFirst();

      variable.getConstraints().forEach(constraint -> {
        if (!constraint.isSatisfied()) { unsatisfied.append(constraint); }});

      constraintsConsuming(variable, constraint -> {
        constraint.recalculate();
        todo.append(constraint.getOutput());
      });
    }

    unsatisfied.sort((first, second) ->
      first.getStrength().stronger(second.getStrength()) ? -1 : 1);
    return unsatisfied;
  }

  // This is the standard DeltaBlue benchmark. A long chain of
  // equality constraints is constructed with a stay constraint on
  // one end. An edit constraint is then added to the opposite end
  // and the time is measured for adding and removing this
  // constraint, and extracting and executing a constraint
  // satisfaction plan. There are two cases. In case 1, the added
  // constraint is stronger than the stay constraint and values must
  // propagate down the entire length of the chain. In case 2, the
  // added constraint is weaker than the stay constraint so it cannot
  // be accomodated. The cost in this case is, of course, very
  // low. Typical situations lie somewhere between these two
  // extremes.
  //
  public static void chainTest(final int count) {
    Planner planner = new Planner();
    Variable[] variables = new Variable[count + 1];
    Arrays.setAll(variables, index -> new Variable());

    // Build chain of n equality constraints
    for (int index = 0; index < count; index++) {
      Variable first = variables[index];
      Variable second = variables[index + 1];
      new EqualityConstraint(first, second, Strength.REQUIRED, planner);
    }

    new StayConstraint(variables[count], Strength.STRONG_DEFAULT, planner);
    AbstractConstraint editConstraint =
        new EditConstraint(variables[0], Strength.PREFERRED, planner);

    Vector<AbstractConstraint> editConstraints = Vector.with(editConstraint);
    Plan plan = planner.extractPlanFromConstraints(editConstraints);
    for (int index = 0; index < 100; index++) {
      variables[0].setValue(index);
      plan.execute();
      if (variables[count].getValue() != index) {
        throw new IllegalStateException("Chain test failed!!");
      }
    }
    editConstraint.destroyConstraint(planner);
  }

  // This test constructs a two sets of variables related to each
  // other by a simple linear transformation (scale and offset). The
  // time is measured to change a variable on either side of the
  // mapping and to change the scale and offset factors.
  //
  public static void projectionTest(final int count) {
    Planner planner = new Planner();

    Vector<Variable> destinations = new Vector<>();

    Variable scale  = Variable.value(10);
    Variable offset = Variable.value(1000);

    Variable source = null;
    Variable destination = null;
    for (int index = 1; index <= count; index++) {
      source = Variable.value(index);
      destination = Variable.value(index);
      destinations.append(destination);
      new StayConstraint(source, Strength.DEFAULT, planner);
      new ScaleConstraint(source, scale, offset, destination, Strength.REQUIRED, planner);
    }

    planner.change(source, 17);
    if (destination.getValue() != 1170) {
      throw new IllegalStateException("Projection test 1 failed!!");
    }

    planner.change(destination, 1050);
    if (source.getValue() != 5) {
      throw new IllegalStateException("Projection test 2 failed!!");
    }

    planner.change(scale, 5);
    for (int index = 0; index < count - 1; ++index) {
      if (destinations.at(index).getValue() != (index + 1) * 5 + 1000) {
        throw new IllegalStateException("Projection test 3 failed!!");
      }
    }

    planner.change(offset, 2000);
    for (int index = 0; index < count - 1; ++index) {
      if (destinations.at(index).getValue() != (index + 1) * 5 + 2000) {
        throw new IllegalStateException("Projection test 4 failed!!");
      }
    }
  }
}
