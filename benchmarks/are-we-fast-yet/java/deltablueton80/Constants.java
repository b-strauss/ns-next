/* Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
 * Primordial Soup's `DeltaBlue.ns`, commit
 * `338f0bb342a618f5608ab72ab1b1fae9b226e472`.
 * This file holds the slots of the module class DeltaBlueTon80 that its
 * nested classes read: the strengths, descendingStrengths and the two
 * directions. They cannot stay in the top-level DeltaBlueTon80, as a package
 * cannot import from the unnamed package; they are static, since they never
 * change and are compared by identity only. The module's List is som.Vector,
 * its planner a field of the top-level DeltaBlueTon80.
 */
/* Copyright 2008, the Newspeak project authors.
 *
 * Licensed under the Apache License, Version 2.0 (the ''License''); you
 * may not use this file except in compliance with the License. You may
 * obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 * and in THIRD-PARTY-NOTICES.md at the repository root.
 */
package deltablueton80;

final class Constants {
  static final Strength REQUIRED = new Strength("required", 0);
  static final Strength STRONG_PREFERRED = new Strength("strongPreferred", 1);
  static final Strength PREFERRED = new Strength("preferred", 2);
  static final Strength STRONG_DEFAULT = new Strength("strongDefault", 3);
  static final Strength NORMAL = new Strength("normal", 4);
  static final Strength WEAK_DEFAULT = new Strength("weakDefault", 5);
  static final Strength WEAKEST = new Strength("weakest", 6);
  static final Strength[] DESCENDING_STRENGTHS = {
    REQUIRED,
    STRONG_PREFERRED,
    PREFERRED,
    STRONG_DEFAULT,
    NORMAL,
    WEAK_DEFAULT,
    WEAKEST,
  };
  static final Direction FORWARD = new Direction("forward");
  static final Direction BACKWARD = new Direction("backward");

  private Constants() { }
}
