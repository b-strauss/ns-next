//! The image's symbol table: every selector and every other identifier name
//! the encoder writes, deduplicated once and referenced everywhere else by a
//! `u32` index into this table.
//!
//! Dispatch is virtual and by name, so a selector cannot live only in the
//! AST -- the run-time lookup would never find it there. Every send
//! instruction's selector, every method-dictionary key, every closure's
//! parameter names and every `Sym` object's text are therefore written into
//! the image, and the same handful of selectors recur at very many call
//! sites. Interning each occurrence individually at load time would cost
//! one `Symbol::intern` per occurrence; a table at the head of the image,
//! referenced by index everywhere else, turns that into one pass over the
//! table's *distinct* strings instead.

use std::collections::HashMap;

use crate::Symbol;

use super::decode::{reserve_for, DecodeError, Reader};
use super::encode::{u32_field, Writer};

/// Collects strings during encoding, deduplicating by content: the same
/// string interned twice gets the same id both times, so a selector reused
/// at a thousand call sites costs one table entry and a thousand four-byte
/// references instead of a thousand length-prefixed copies.
#[derive(Default)]
pub struct Builder {
    strings: Vec<String>,
    ids: HashMap<String, u32>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }

    /// Returns the string's id, assigning a fresh one the first time a
    /// given string is seen.
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.ids.get(s) {
            return id;
        }
        let id = u32_field("symbol table id", self.strings.len());
        self.strings.push(s.to_string());
        self.ids.insert(s.to_string(), id);
        id
    }

    /// The count, then each string in the order `intern` assigned it an id
    /// -- the same order `decode` hands ids back out in, so an id written
    /// anywhere else during encoding still resolves after a round trip.
    pub fn write(&self, w: &mut Writer) {
        w.write_u32(u32_field("symbol table entry count", self.strings.len()));
        for s in &self.strings {
            w.write_str(s);
        }
    }

    /// Builds the table directly from the collected strings, without a byte
    /// round trip. For callers that hold both the builder and the decoded
    /// table in the same process (round-trip tests, mainly): `write` then
    /// `decode` would work too, but this skips the buffer.
    pub fn build(self) -> Table {
        Table {
            symbols: self.strings.iter().map(|s| Symbol::intern(s)).collect(),
        }
    }
}

/// The decoded table: every string interned exactly once, then handed out
/// by index. `Symbol::intern` is itself a deduplicating global cache keyed
/// by content, so this costs one lookup per *distinct* string in the image
/// -- not one per occurrence, which is the whole point of the table.
pub struct Table {
    symbols: Vec<Symbol>,
}

impl Table {
    /// The empty table: every hand-crafted test fixture with no selector or
    /// name in its bytes needs one to satisfy the decoder's signature even
    /// though it never calls `get`.
    pub fn empty() -> Table {
        Table {
            symbols: Vec::new(),
        }
    }

    /// Range-checked at the point of use: an out-of-range id from a crafted
    /// image is an `Err`, never an index panic.
    pub fn get(&self, id: u32) -> Result<Symbol, DecodeError> {
        self.symbols
            .get(id as usize)
            .copied()
            .ok_or(DecodeError::BadSymbolId(id))
    }
}

/// Reads the count then that many strings, bounding the count against the
/// remaining input before allocating (`reserve_for`) so a crafted count
/// cannot drive a large placeholder allocation before the first read fails.
pub fn decode(r: &mut Reader) -> Result<Table, DecodeError> {
    let count = r.read_u32()? as usize;
    let mut symbols: Vec<Symbol> = reserve_for(count, r);
    for _ in 0..count {
        symbols.push(Symbol::intern(&r.read_str()?));
    }
    Ok(Table { symbols })
}
