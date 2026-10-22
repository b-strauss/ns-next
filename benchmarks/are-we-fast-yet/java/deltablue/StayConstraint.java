/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/deltablue/StayConstraint.java).
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

// I mark variables that should, with some level of preference, stay
// the same. I have one method with zero inputs and one output, which
// does nothing. Planners may exploit the fact that, if I am
// satisfied, my output will not change during plan execution. This is
// called "stay optimization".
//
final class StayConstraint extends UnaryConstraint {

  // Install a stay constraint with the given strength on the given variable.
  StayConstraint(final Variable variable, final Symbol strength,
      final Planner planner) {
   super(variable, strength, planner);
  }

  @Override
  public void execute() { } // Stay constraints do nothing.
}
