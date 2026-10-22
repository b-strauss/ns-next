/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/deltablue/Strength.java).
 * Changed for ns-next: names are written out (Sym is Symbol).
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

import som.Dictionary.CustomHash;
import som.IdentityDictionary;

/*
 * Strengths are used to measure the relative importance of constraints. New
 * strengths may be inserted in the strength hierarchy without disrupting
 * current constraints. Strengths cannot be created outside this class, so
 * pointer comparison can be used for value comparison.
 */
public final class Strength {

  public static class Symbol implements CustomHash {

    private final int hash;

    Symbol(final int hash) {
      this.hash = hash;
    }

    @Override
    public int customHash() {
      return hash;
    }
  }

  public static final Symbol ABSOLUTE_STRONGEST = new Symbol(0);
  public static final Symbol REQUIRED           = new Symbol(1);
  public static final Symbol STRONG_PREFERRED   = new Symbol(2);
  public static final Symbol PREFERRED          = new Symbol(3);
  public static final Symbol STRONG_DEFAULT     = new Symbol(4);
  public static final Symbol DEFAULT            = new Symbol(5);
  public static final Symbol WEAK_DEFAULT       = new Symbol(6);
  public static final Symbol ABSOLUTE_WEAKEST   = new Symbol(7);

  private final int arithmeticValue;
  @SuppressWarnings("unused")
  private final Symbol symbolicValue;

  private Strength(final Symbol symbolicValue) {
    this.symbolicValue = symbolicValue;
    this.arithmeticValue = strengthTable.at(symbolicValue);
  }

  public boolean sameAs(final Strength strength) {
    return arithmeticValue == strength.getArithmeticValue();
  }

  public boolean stronger(final Strength strength) {
    return arithmeticValue < strength.getArithmeticValue();
  }

  public boolean weaker(final Strength strength) {
    return arithmeticValue > strength.getArithmeticValue();
  }

  public Strength strongest(final Strength strength) {
    return strength.stronger(this) ? strength : this;
  }

  public Strength weakest(final Strength strength) {
    return strength.weaker(this) ? strength : this;
  }

  public int getArithmeticValue() {
    return arithmeticValue;
  }

  public static Strength of(final Symbol strength) {
    return strengthConstant.at(strength);
  }

  private static IdentityDictionary<Symbol, Integer> createStrengthTable() {
    IdentityDictionary<Symbol, Integer> strengthTable = new IdentityDictionary<>();
    strengthTable.atPut(ABSOLUTE_STRONGEST, -10000);
    strengthTable.atPut(REQUIRED,           -800);
    strengthTable.atPut(STRONG_PREFERRED,   -600);
    strengthTable.atPut(PREFERRED,          -400);
    strengthTable.atPut(STRONG_DEFAULT,     -200);
    strengthTable.atPut(DEFAULT,             0);
    strengthTable.atPut(WEAK_DEFAULT,        500);
    strengthTable.atPut(ABSOLUTE_WEAKEST,    10000);
    return strengthTable;
  }

  private static IdentityDictionary<Symbol, Strength> createStrengthConstants() {
    IdentityDictionary<Symbol, Strength> strengthConstant = new IdentityDictionary<>();
    strengthTable.getKeys().forEach(key ->
      strengthConstant.atPut(key, new Strength(key))
    );
    return strengthConstant;
  }

  static {
    strengthTable     = createStrengthTable();
    strengthConstant  = createStrengthConstants();

    absoluteWeakest   = of(ABSOLUTE_WEAKEST);
    required          = of(REQUIRED);
  }


  public static Strength absoluteWeakest() {
    return absoluteWeakest;
  }

  public static Strength required() {
    return required;
  }

  private static final Strength absoluteWeakest;
  private static final Strength required;

  private static final IdentityDictionary<Symbol, Integer>  strengthTable;
  private static final IdentityDictionary<Symbol, Strength> strengthConstant;
}
