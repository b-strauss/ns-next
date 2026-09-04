//! Global symbol interning for selectors, slot names, and class names.
//!
//! Interned strings are leaked (`Box::leak`) and live for the process
//! lifetime. This is an accepted Milestone 1 simplification — the same
//! spirit as the `Rc<RefCell<_>>` object-graph leak already accepted in
//! the runtime spec's Decisions section. A later milestone's GC would
//! intern into a proper symbol table instead.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(u32);

struct Interner {
    strings: Vec<&'static str>,
    lookup: HashMap<&'static str, Symbol>,
}

impl Interner {
    fn new() -> Self {
        Interner {
            strings: Vec::new(),
            lookup: HashMap::new(),
        }
    }
}

thread_local! {
    static INTERNER: RefCell<Interner> = RefCell::new(Interner::new());
}

impl Symbol {
    pub fn intern(s: &str) -> Symbol {
        INTERNER.with(|cell| {
            let mut interner = cell.borrow_mut();
            if let Some(&sym) = interner.lookup.get(s) {
                return sym;
            }
            let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
            let sym = Symbol(interner.strings.len() as u32);
            interner.strings.push(leaked);
            interner.lookup.insert(leaked, sym);
            sym
        })
    }

    pub fn as_str(self) -> &'static str {
        INTERNER.with(|cell| cell.borrow().strings[self.0 as usize])
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
        let a = Symbol::intern("foo:");
        let b = Symbol::intern("foo:");
        assert_eq!(a, b);
    }

    #[test]
    fn different_strings_intern_to_different_symbols() {
        let a = Symbol::intern("foo:");
        let b = Symbol::intern("bar:");
        assert_ne!(a, b);
    }

    #[test]
    fn as_str_round_trips() {
        let s = Symbol::intern("printString");
        assert_eq!(s.as_str(), "printString");
    }

    #[test]
    fn display_matches_as_str() {
        let s = Symbol::intern("at:put:");
        assert_eq!(format!("{s}"), "at:put:");
    }
}
