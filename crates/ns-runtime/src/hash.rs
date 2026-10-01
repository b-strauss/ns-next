//! The hasher for tables keyed by an identity the runtime assigned: an
//! interned `Symbol`'s index, a mixin's or application's address. Such keys
//! are already distinct and not chosen by any input -- an image names
//! symbols by text and the interner numbers them -- so SipHash's resistance
//! to chosen keys buys nothing here, and costs a handful of rounds on every
//! probe of every lookup. One multiply mixes a word; the rotation in
//! `finish` brings its well-mixed high bits down to the bucket index, which
//! an address's zero low bits would otherwise decide alone. rustc's own
//! tables hash this way (FxHash).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// An odd constant whose bits are spread evenly, so that consecutive keys
/// land far apart.
const MULTIPLIER: u64 = 0xf135_7aea_2e62_a9c5;

#[derive(Default, Clone, Copy)]
pub struct IdentityHasher {
    hash: u64,
}

impl IdentityHasher {
    fn add(&mut self, word: u64) {
        self.hash = self.hash.wrapping_add(word).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for IdentityHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    fn finish(&self) -> u64 {
        self.hash.rotate_left(26)
    }
}

pub type IdentityBuildHasher = BuildHasherDefault<IdentityHasher>;

/// A `HashMap` hashed by `IdentityHasher`.
pub type IdentityMap<K, V> = HashMap<K, V, IdentityBuildHasher>;

/// The content hash a Newspeak `hash` answers for a value whose content,
/// not its address, decides its equality (`builtins::text`'s `String`,
/// `builtins::numbers`'s `Float`, and the hash `builtins::value` derives
/// from a value object's slots). The repository keeps one hash function, so
/// this is `IdentityHasher` over the content rather than a second one.
///
/// Folded to 32 bits: a `hash` answers a `SmallInteger`, and the derived
/// hash adds one per slot of a class that lays out at most 2^16 of them, so
/// every partial sum has to stay inside an `i64`. The low half is what is
/// kept, because `IdentityHasher::finish` has already rotated the product's
/// well-mixed high bits down into it -- keeping the high half instead would
/// answer the multiply's poorly mixed low bits.
pub(crate) fn content_hash(value: impl std::hash::Hash) -> i64 {
    use std::hash::BuildHasher;
    i64::from(IdentityBuildHasher::default().hash_one(value) as u32)
}

/// `content_hash` of the text `characters` spell, without building it:
/// `str`'s `Hash` writes the UTF-8 bytes in one `write`, which
/// `IdentityHasher` takes eight at a time, and then `0xff`.
pub(crate) fn content_hash_of_characters(characters: impl Iterator<Item = char>) -> i64 {
    let mut hasher = IdentityHasher::default();
    let mut word = [0u8; 8];
    let mut filled = 0;
    let mut encoded = [0u8; 4];
    for character in characters {
        for byte in character.encode_utf8(&mut encoded).bytes() {
            word[filled] = byte;
            filled += 1;
            if filled == 8 {
                hasher.add(u64::from_le_bytes(word));
                word = [0u8; 8];
                filled = 0;
            }
        }
    }
    if filled > 0 {
        hasher.add(u64::from_le_bytes(word));
    }
    hasher.write_u8(0xff);
    i64::from(hasher.finish() as u32)
}

#[cfg(test)]
mod tests {
    use std::hash::{BuildHasher, Hash};

    use super::*;
    use crate::Symbol;

    fn hash_of(key: impl Hash) -> u64 {
        IdentityBuildHasher::default().hash_one(key)
    }

    #[test]
    fn a_thousand_consecutive_symbols_answer_back() {
        let symbols: Vec<Symbol> = (0..1000)
            .map(|index| Symbol::intern(&format!("identity-hasher-{index}")))
            .collect();
        let map: IdentityMap<Symbol, usize> = symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| (*symbol, index))
            .collect();
        assert_eq!(map.len(), 1000);
        for (index, symbol) in symbols.iter().enumerate() {
            assert_eq!(map.get(symbol), Some(&index));
        }
    }

    /// hashbrown takes a bucket index from a hash's low bits and the tag it
    /// compares a group against from its top seven, so consecutive keys --
    /// the interner numbers symbols consecutively -- must differ in both.
    #[test]
    fn consecutive_keys_differ_in_the_top_and_bottom_bits() {
        for key in [0u32, 1, 41, 1000, 65_535] {
            let this = hash_of(key);
            let next = hash_of(key + 1);
            assert_ne!(this >> 57, next >> 57, "top seven bits of {key}");
            assert_ne!(this & 0x7f, next & 0x7f, "bottom bits of {key}");
        }
    }

    /// An address is a multiple of its alignment, so its low bits are zero;
    /// the bucket index must not be decided by them alone.
    #[test]
    fn an_aligned_address_hashes_to_non_zero_bottom_bits() {
        for address in [0x1000usize, 0x7ff6_1234_5670, 0x10_0000_0040] {
            assert_ne!(hash_of(address) & 0x3f, 0, "{address:#x}");
        }
    }
}
