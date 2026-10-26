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
    VALUE_OF => "valueOf:",
    VALUE_OF_WITH => "valueOf:with:",
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

struct Interner {
    strings: Vec<&'static str>,
    lookup: HashMap<&'static str, Symbol>,
}

impl Interner {
    /// Seeded with `WELL_KNOWN` before anything else is interned, so each
    /// well-known symbol's index is its constant's on every thread.
    fn new() -> Self {
        let mut interner = Interner {
            strings: Vec::new(),
            lookup: HashMap::new(),
        };
        for text in WELL_KNOWN {
            interner.insert(text);
        }
        interner
    }

    fn insert(&mut self, text: &'static str) -> Symbol {
        let symbol = Symbol(self.strings.len() as u32);
        self.strings.push(text);
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

    pub fn as_str(self) -> &'static str {
        INTERNER.with_borrow(|interner| interner.strings[self.0 as usize])
    }
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
