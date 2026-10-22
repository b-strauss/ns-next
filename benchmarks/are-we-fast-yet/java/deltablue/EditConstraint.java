/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/deltablue/EditConstraint.java).
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

// I am a unary input constraint used to mark a variable that the
// client wishes to change.
final class EditConstraint extends UnaryConstraint {

  EditConstraint(final Variable variable, final Symbol strength, final Planner planner) {
    super(variable, strength, planner);
  }

  // I indicate that a variable is to be changed by imperative code.
  @Override
  public boolean isInput() {
    return true;
  }

  @Override
  public void execute() { } // Edit constraints do nothing.
}
