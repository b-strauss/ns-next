/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/deltablue/BinaryConstraint.java).
 * Changed for ns-next: names are written out (Strength.Sym is
 * Strength.Symbol).
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

import deltablue.Strength.Symbol;
import som.ForEachInterface;
import som.TestInterface;

// I am an abstract superclass for constraints having two possible
// output variables.
abstract class BinaryConstraint extends AbstractConstraint {

  protected Variable first;
  protected Variable second;          // possible output variables
  protected Direction direction;  // one of the following...

  BinaryConstraint(final Variable firstVariable, final Variable secondVariable,
      final Symbol strength, final Planner planner) {
    super(strength);
    first = firstVariable;
    second = secondVariable;
    direction = null;
  }

  // Answer true if this constraint is satisfied in the current solution.
  @Override
  public boolean isSatisfied() {
    return direction != null;
  }

  // Add myself to the constraint graph.
  @Override
  public void addToGraph() {
    first.addConstraint(this);
    second.addConstraint(this);
    direction = null;
  }

  // Remove myself from the constraint graph.
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

  // Decide if I can be satisfied and which way I should flow based on
  // the relative strength of the variables I relate, and record that
  // decision.
  //
  @Override
  protected Direction chooseMethod(final int mark) {
    if (first.getMark() == mark) {
      if (second.getMark() != mark && strength.stronger(second.getWalkStrength())) {
        direction = Direction.FORWARD;
        return direction;
      } else {
        direction = null;
        return direction;
      }
    }

    if (second.getMark() == mark) {
      if (first.getMark() != mark && strength.stronger(first.getWalkStrength())) {
        direction = Direction.BACKWARD;
        return direction;
      } else {
        direction = null;
        return direction;
      }
    }

    // If we get here, neither variable is marked, so we have a choice.
    if (first.getWalkStrength().weaker(second.getWalkStrength())) {
      if (strength.stronger(first.getWalkStrength())) {
        direction = Direction.BACKWARD;
        return direction;
      } else {
        direction = null;
        return direction;
      }
    } else {
      if (strength.stronger(second.getWalkStrength())) {
        direction = Direction.FORWARD;
        return direction;
      } else {
        direction = null;
        return direction;
      }
    }
  }

  @Override
  public void inputsDo(final ForEachInterface<Variable> function) {
    if (direction == Direction.FORWARD) {
      function.apply(first);
    } else {
      function.apply(second);
    }
  }

  @Override
  public boolean inputsHasOne(final TestInterface<Variable> function) {
    if (direction == Direction.FORWARD) {
      return function.test(first);
    } else {
      return function.test(second);
    }
  }

  // Record the fact that I am unsatisfied.
  @Override
  public void markUnsatisfied() {
    direction = null;
  }


  // Answer my current output variable.
  @Override
  public Variable getOutput() {
    return direction == Direction.FORWARD ? second : first;
  }

  // Calculate the walkabout strength, the stay flag, and, if it is
  // 'stay', the value for the current output of this
  // constraint. Assume this constraint is satisfied.
  //
  @Override
  public void recalculate() {
    Variable inputVariable;
    Variable outputVariable;

    if (direction == Direction.FORWARD) {
      inputVariable = first; outputVariable = second;
    } else {
      inputVariable = second; outputVariable = first;
    }

    outputVariable.setWalkStrength(strength.weakest(inputVariable.getWalkStrength()));
    outputVariable.setStay(inputVariable.getStay());
    if (outputVariable.getStay()) {
      execute();
    }
  }
}
