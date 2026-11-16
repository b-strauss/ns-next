//! The weak table of the symbols a program makes (`docs/design/memory.md`,
//! "Weak references").
//!
//! A symbol a program makes from a string (`asSymbol`) whose text nothing
//! has interned takes no number: it is only its object, a String of the
//! `Symbol` class, and this table is what makes it the one object of its
//! text (§5.1.6). Keyed by a hash of the text seeded per heap, and
//! compared by the text the object holds, so the table keeps no copy of
//! it. It is weak, as the
//! canonical symbol table is: the collector prunes it and forwards it, and
//! never keeps an object alive through it.

use std::hash::{BuildHasher, Hasher, RandomState};

use crate::hash::IdentityMap;

use super::ObjectReference;

/// The program-made symbols of one heap: per heap, because a handle
/// means nothing against another heap.
#[derive(Default)]
pub(crate) struct ProgramSymbols<'gc> {
    /// The key of the buckets' hash. A program chooses the texts, so with
    /// an unseeded hash it could make them collide on purpose and every
    /// `asSymbol` scan one growing bucket; SipHash under a key drawn per
    /// heap leaves it nothing to aim at. The draw makes no output differ
    /// between runs: a lookup compares texts, and no answer depends on the
    /// order of a bucket or of the table.
    seed: RandomState,
    /// Every program-made symbol, under its text's `bucket` hash. A
    /// bucket holds the symbols whose texts share the hash, each text
    /// once.
    by_hash: IdentityMap<u64, Vec<ObjectReference<'gc>>>,
    /// The inverse: each symbol's offset, and the hash it is filed under.
    /// Keyed by offset, so that it holds no handle, and so that whether
    /// an object is a program-made symbol is one probe.
    members: IdentityMap<u64, u64>,
}

impl<'gc> ProgramSymbols<'gc> {
    /// How many program-made symbols the table holds: its rows in use,
    /// which its growth limit measures (`limits::TableLimits`).
    pub(crate) fn len(&self) -> usize {
        self.members.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Whether the object at `offset` is a program-made symbol.
    pub(crate) fn contains(&self, offset: u64) -> bool {
        self.members.contains_key(&offset)
    }

    /// The bucket hash of the text `characters` spell, the same for a
    /// `&str` as for a String object's code points: both write the same
    /// UTF-8 bytes, a character at a time, then `0xff`, as `str`'s `Hash`
    /// ends its text.
    pub(crate) fn bucket(&self, characters: impl Iterator<Item = char>) -> u64 {
        let mut hasher = self.seed.build_hasher();
        let mut encoded = [0u8; 4];
        for character in characters {
            hasher.write(character.encode_utf8(&mut encoded).as_bytes());
        }
        hasher.write_u8(0xff);
        hasher.finish()
    }

    /// The symbols filed under `hash`, among which the one of a text of
    /// that hash is, if any.
    pub(crate) fn candidates(&self, hash: u64) -> &[ObjectReference<'gc>] {
        self.by_hash.get(&hash).map_or(&[], Vec::as_slice)
    }

    /// Files `object`, a symbol whose text no symbol of the table spells,
    /// under `hash`, its text's.
    pub(crate) fn insert(&mut self, hash: u64, object: ObjectReference<'gc>) {
        self.by_hash.entry(hash).or_default().push(object);
        let previous = self.members.insert(object.offset.get(), hash);
        debug_assert!(previous.is_none(), "internal: a symbol is filed twice");
    }

    /// Takes `object` out of the table, when the canonical table adopts
    /// it.
    pub(crate) fn remove(&mut self, object: ObjectReference<'gc>) {
        let offset = object.offset.get();
        let Some(hash) = self.members.remove(&offset) else {
            return;
        };
        if let Some(bucket) = self.by_hash.get_mut(&hash) {
            bucket.retain(|filed| filed.offset.get() != offset);
            if bucket.is_empty() {
                self.by_hash.remove(&hash);
            }
        }
    }

    /// Every symbol the table holds.
    pub(crate) fn objects(&self) -> impl Iterator<Item = ObjectReference<'gc>> {
        self.by_hash.values().flatten().copied()
    }

    /// Keeps the symbols `keep` answers true for, each as `keep` left it:
    /// the collector's passes, which drop what died and forward what moved.
    /// The inverse follows: its changes are applied after the walk, every
    /// removal before any insertion, so that an object moved to the
    /// offset another one left -- which a compaction's slide does -- is
    /// never taken out with the one that left it.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&mut ObjectReference<'gc>) -> bool) {
        let mut left = Vec::new();
        let mut arrived = Vec::new();
        self.by_hash.retain(|hash, bucket| {
            bucket.retain_mut(|object| {
                let before = object.offset.get();
                let kept = keep(object);
                let after = object.offset.get();
                if !kept {
                    left.push(before);
                } else if after != before {
                    left.push(before);
                    arrived.push((after, *hash));
                }
                kept
            });
            !bucket.is_empty()
        });
        for offset in left {
            self.members.remove(&offset);
        }
        for (offset, hash) in arrived {
            self.members.insert(offset, hash);
        }
    }
}
