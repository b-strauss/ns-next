/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/deltablue/EqualityConstraint.java).
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

// I constrain two variables to have the same value: "v1 = v2".
final class EqualityConstraint extends BinaryConstraint {

  // Install a constraint with the given strength equating the given
  // variables.
  EqualityConstraint(final Variable firstVariable, final Variable secondVariable,
      final Symbol strength, final Planner planner) {
    super(firstVariable, secondVariable, strength, planner);
    addConstraint(planner);
  }

  // Enforce this constraint. Assume that it is satisfied.
  @Override
  public void execute() {
    if (direction == Direction.FORWARD) {
      second.setValue(first.getValue());
    } else {
      first.setValue(second.getValue());
    }
  }
}
