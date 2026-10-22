/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/deltablue/ScaleConstraint.java).
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

// I relate two variables by the linear scaling relationship: "v2 =
// (v1 * scale) + offset". Either v1 or v2 may be changed to maintain
// this relationship but the scale factor and offset are considered
// read-only.
final class ScaleConstraint extends BinaryConstraint {

  protected final Variable scale;  // scale factor input variable
  protected final Variable offset; // offset input variable

  ScaleConstraint(final Variable source, final Variable scale,
      final Variable offset, final Variable destination, final Symbol strength,
      final Planner planner) {
    super(source, destination, strength, planner);
    this.scale = scale;
    this.offset = offset;
    addConstraint(planner);
  }

  // Add myself to the constraint graph.
  @Override
  public void addToGraph() {
    first.addConstraint(this);
    second.addConstraint(this);
    scale.addConstraint(this);
    offset.addConstraint(this);
    direction = null;
  }

  // Remove myself from the constraint graph.
  @Override
  public void removeFromGraph() {
    if (first != null) { first.removeConstraint(this); }
    if (second != null) { second.removeConstraint(this); }
    if (scale  != null) { scale.removeConstraint(this); }
    if (offset != null) { offset.removeConstraint(this); }
    direction = null;
  }

  // Enforce this constraint. Assume that it is satisfied.
  @Override
  public void execute() {
    if (direction == Direction.FORWARD) {
      second.setValue(first.getValue() * scale.getValue() + offset.getValue());
    } else {
      first.setValue((second.getValue() - offset.getValue()) / scale.getValue());
    }
  }

  @Override
  public void inputsDo(final ForEachInterface<Variable> function) {
    if (direction == Direction.FORWARD) {
      function.apply(first);
      function.apply(scale);
      function.apply(offset);
    } else {
      function.apply(second);
      function.apply(scale);
      function.apply(offset);
    }
  }

  // Calculate the walkabout strength, the stay flag, and, if it is
  // 'stay', the value for the current output of this
  // constraint. Assume this constraint is satisfied.
  @Override
  public void recalculate() {
    Variable inputVariable;
    Variable outputVariable;

    if (direction == Direction.FORWARD) {
      inputVariable = first; outputVariable = second;
    } else {
      outputVariable = first; inputVariable = second;
    }

    outputVariable.setWalkStrength(strength.weakest(inputVariable.getWalkStrength()));
    outputVariable.setStay(inputVariable.getStay() && scale.getStay() && offset.getStay());
    if (outputVariable.getStay()) {
      execute(); // stay optimization
    }
  }
}
