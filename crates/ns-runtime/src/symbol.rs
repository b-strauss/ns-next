//! Global symbol interning for selectors, slot names, and class names.
//!
//! Interned strings are leaked (`Box::leak`) and live for the process
//! lifetime. This is deliberate, and the collector does not change it: a
//! `Symbol` *object* the runtime allocates on the heap is reclaimed like any
//! other object, through the canonical symbol table's weak entries
//! (`docs/design/memory.md`, "Weak references"), but the leaked `&'static
//! str` this interner hands out, and the interner's own table, are a
//! separate store outside the heap that the collector never traces --
//! nothing reclaims them, and nothing is meant to.
//!
//! So only a text the runtime names is interned: what the compiler, the
//! resolver, the image decoder and the runtime's own constants name,
//! bounded by the source compiled and the images loaded. A running
//! program never interns: a symbol it makes from a string (`asSymbol`)
//! whose text nothing has interned takes no number and lives only as its
//! object, in the heap's weak table of program-made symbols
//! (`Mutator::program_symbol`; `docs/design/memory.md`, "Weak
//! references"), and a name a program looks up is probed with `lookup`,
//! which inserts nothing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(u32);

/// Declares the symbols the runtime names in its own code, each as an
/// associated constant on `Symbol`, and the table `Interner::new` seeds them
/// from. One list, so a constant and its text cannot drift apart: the enum
/// numbers the entries in declaration order and the table lists the texts in
/// the same order.
macro_rules! well_known_symbols {
    ($($name:ident => $text:literal,)*) => {
        #[expect(
            non_camel_case_types,
            clippy::upper_case_acronyms,
            reason = "a variant is named after the text it stands for, which the table keeps in one list"
        )]
        #[repr(u32)]
        enum WellKnown {
            $($name,)*
        }

        impl Symbol {
            $(pub const $name: Symbol = Symbol(WellKnown::$name as u32);)*
        }

        /// Interned first, in this order, by every thread's interner.
        const WELL_KNOWN: &[&str] = &[$($text,)*];

        #[cfg(test)]
        const WELL_KNOWN_SYMBOLS: &[Symbol] = &[$(Symbol::$name,)*];
    };
}

well_known_symbols! {
    NEW => "new",
    VALUE => "value",
    RESOLVE => "resolve",
    DOES_NOT_UNDERSTAND => "doesNotUnderstand:",
    CARET => "^",
    OBJECT => "Object",
    USING_PLATFORM => "usingPlatform:",
    CLASS_SIDE => "class side",
    AN_OBJECT => "an object",
    UNKNOWN => "unknown",
    EQUALS => "=",
    IDENTICAL => "==",
    VALUE_ON_UNWIND => "valueOnUnwind:",
    SIGNAL => "signal",
    PACKAGE_USING => "packageUsing:",
    MAIN_ARGUMENTS => "main:args:",
}

/// Told apart from a String's hash of the same text: a Symbol is never
/// equal to a String, so nothing requires the two to hash alike, and a
/// collection holding both keeps them in different buckets. Any 32-bit
/// constant would do; this one mixes the bits evenly.
const SYMBOL_HASH_SALT: i64 = 0x5bd1_e995;

/// The hash `Symbol>>hash` answers for a symbol of `text`: the text's
/// content hash, salted apart from a String's, and never a number, since a
/// program-made symbol has none until its text is interned and its hash
/// must not change when the canonical table adopts it. The content hash is
/// a non-negative sub-2^32 integer, and so is this, since the salt has no
/// bit past 32.
pub(crate) fn hash_of_symbol_text(content_hash: i64) -> i64 {
    content_hash ^ SYMBOL_HASH_SALT
}

struct Interner {
    strings: Vec<&'static str>,
    /// Each interned text's `hash_of_symbol_text`, computed once at the
    /// interning, so that a canonical symbol's `hash` reads it rather than
    /// walking its text on every call.
    hashes: Vec<i64>,
    lookup: HashMap<&'static str, Symbol>,
}

impl Interner {
    /// Seeded with `WELL_KNOWN` before anything else is interned, so each
    /// well-known symbol's index is its constant's on every thread.
    fn new() -> Self {
        let mut interner = Interner {
            strings: Vec::new(),
            hashes: Vec::new(),
            lookup: HashMap::new(),
        };
        for text in WELL_KNOWN {
            interner.insert(text);
        }
        interner
    }

    /// Interns `text` under the next number. No caller of `intern` can
    /// answer an error, so a symbol past the number `u32::MAX` on one
    /// thread exhausts the table and aborts, as an image field past `u32`
    /// does; the texts interned are bounded by the source compiled and the
    /// images loaded, never by what a program makes.
    fn insert(&mut self, text: &'static str) -> Symbol {
        let symbol =
            Symbol(u32::try_from(self.strings.len()).expect(
                "the symbol table is exhausted: a thread numbers its symbols up to u32::MAX",
            ));
        self.strings.push(text);
        self.hashes
            .push(hash_of_symbol_text(crate::hash::content_hash(text)));
        self.lookup.insert(text, symbol);
        symbol
    }
}

thread_local! {
    static INTERNER: RefCell<Interner> = RefCell::new(Interner::new());
}

impl Symbol {
    /// The interned number, dense from 0 in interning order, so a table
    /// indexed by it has one slot per symbol ever interned and no hashing.
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    /// The symbol whose `index` is `index`: a method frame's header holds
    /// its selector as that number. Only a number `index` answered names
    /// the symbol it answered it for.
    pub(crate) fn from_index(index: u32) -> Symbol {
        Symbol(index)
    }

    pub fn intern(text: &str) -> Symbol {
        INTERNER.with_borrow_mut(|interner| {
            if let Some(&symbol) = interner.lookup.get(text) {
                return symbol;
            }
            interner.insert(Box::leak(text.to_string().into_boxed_str()))
        })
    }

    /// The symbol `text` already is, or `None` while nothing has interned
    /// it. A probe that inserts nothing: what a program makes or looks up
    /// by a text of its choosing goes through here, so that no run grows
    /// the interner (`Mutator::program_symbol`). Since every selector, slot
    /// name and class name is interned, `None` means that nothing has this
    /// name.
    pub fn lookup(text: &str) -> Option<Symbol> {
        INTERNER.with_borrow(|interner| interner.lookup.get(text).copied())
    }

    /// How many symbols this thread has interned (`Interpreter::table_sizes`).
    pub(crate) fn interned_count() -> usize {
        INTERNER.with_borrow(|interner| interner.strings.len())
    }

    /// The hash a Symbol object of this symbol answers (`Symbol>>hash`),
    /// stored at the interning (`hash_of_symbol_text`).
    pub(crate) fn text_hash(self) -> i64 {
        INTERNER.with_borrow(|interner| interner.hashes[self.0 as usize])
    }

    /// How many arguments a message of this selector takes
    /// (`selector_arity`).
    pub(crate) fn arity(self) -> usize {
        selector_arity(self.as_str())
    }

    pub fn as_str(self) -> &'static str {
        INTERNER.with_borrow(|interner| interner.strings[self.0 as usize])
    }
}

/// How many arguments a message of `selector` takes: none for a unary
/// selector, one for a binary one and one per colon for a keyword one. A
/// unary or keyword selector begins as the lexer's identifiers do, with an
/// ASCII letter or `_`; every other selector is binary. A byte scan, so
/// that the VM mirror's table can run it at compile time.
pub(crate) const fn selector_arity(selector: &str) -> usize {
    let bytes = selector.as_bytes();
    if bytes.is_empty() {
        return 0;
    }
    if bytes[0] != b'_' && !bytes[0].is_ascii_alphabetic() {
        return 1;
    }
    let mut colons = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':' {
            colons += 1;
        }
        index += 1;
    }
    colons
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use crate::Symbol;

    #[test]
    fn interning_the_same_string_twice_gives_the_same_symbol() {
        let first = Symbol::intern("foo:");
        let second = Symbol::intern("foo:");
        assert_eq!(first, second);
    }

    #[test]
    fn different_strings_intern_to_different_symbols() {
        let first = Symbol::intern("foo:");
        let second = Symbol::intern("bar:");
        assert_ne!(first, second);
    }

    #[test]
    fn a_lookup_finds_what_was_interned_and_inserts_nothing() {
        let interned = Symbol::intern("looked:up:");
        assert_eq!(Symbol::lookup("looked:up:"), Some(interned));
        let count = Symbol::interned_count();
        assert_eq!(Symbol::lookup("never interned by anything"), None);
        assert_eq!(Symbol::interned_count(), count);
    }

    /// The hash stored at the interning is the one a program-made symbol of
    /// the same text computes from its text, so a symbol's hash does not
    /// change when the canonical table adopts it.
    #[test]
    fn the_stored_hash_is_the_one_the_text_computes() {
        for text in ["hash:of:", "", "\u{20ac}uro \u{1f600}", "a"] {
            let computed =
                super::hash_of_symbol_text(crate::hash::content_hash_of_characters(text.chars()));
            assert_eq!(Symbol::intern(text).text_hash(), computed, "{text:?}");
        }
    }

    #[test]
    fn as_str_round_trips() {
        let symbol = Symbol::intern("printString");
        assert_eq!(symbol.as_str(), "printString");
    }

    #[test]
    fn display_matches_as_str() {
        let symbol = Symbol::intern("at:put:");
        assert_eq!(format!("{symbol}"), "at:put:");
    }

    /// Every well-known symbol is the one interning its text answers, on
    /// every thread -- the property that lets the runtime name a symbol as a
    /// constant instead of interning it where it is used.
    #[test]
    fn every_well_known_symbol_is_what_interning_its_text_answers() {
        for (symbol, text) in super::WELL_KNOWN_SYMBOLS.iter().zip(super::WELL_KNOWN) {
            assert_eq!(symbol.as_str(), *text);
            assert_eq!(Symbol::intern(text), *symbol);
        }
        assert_eq!(super::WELL_KNOWN_SYMBOLS.len(), super::WELL_KNOWN.len());
        let on_another_thread = std::thread::spawn(|| Symbol::intern("doesNotUnderstand:"))
            .join()
            .expect("thread");
        assert_eq!(on_another_thread, Symbol::DOES_NOT_UNDERSTAND);
    }
}
