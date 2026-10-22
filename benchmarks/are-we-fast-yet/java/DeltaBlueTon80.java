/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file is its module class with its planner and bench, which answers
 * nothing where the port answers true; the checks, in
 * deltablueton80/Planner.java, throw IllegalStateException where the port
 * signals an Error. The module's other slots are deltablueton80/Constants.java,
 * its nested classes the other files of deltablueton80/. As in the port, one
 * planner serves every run of one instance.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
import deltablueton80.Planner;

public final class DeltaBlueTon80 extends Benchmark {
  private final Planner planner = new Planner();

  @Override
  public void bench() {
    planner.chainTest(100);
    planner.projectionTest(100);
  }
}
