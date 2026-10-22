/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/som/IdentityDictionary.java), which carries no notice
 * of its own.
 * Changed for ns-next: names are written out.
 */
package som;

import som.Dictionary.CustomHash;


public class IdentityDictionary<K extends CustomHash, V> extends Dictionary<K, V> {

  static class IdentityEntry<K, V> extends Entry<K, V> {
    IdentityEntry(final int hash, final K key, final V value, final Entry<K, V> next) {
      super(hash, key, value, next);
    }

    @Override
    boolean match(final int hash, final K key) {
      return this.hash == hash && this.key == key;
    }
  }

  public IdentityDictionary(final int size) {
    super(size);
  }

  public IdentityDictionary() {
    super(INITIAL_CAPACITY);
  }

  @Override
  protected Entry<K, V> newEntry(final K key, final V value, final int hash) {
    return new IdentityEntry<>(hash, key, value, null);
  }
}
