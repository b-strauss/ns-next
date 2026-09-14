//! Static (activation-chain) local-variable resolution: method/block
//! temporaries and parameters. See `docs/design/dispatch.md`,
//! "Resolution", step 1.

use std::collections::HashMap;

use crate::types::Binding;
use crate::Symbol;

/// One method or closure body's own temporaries and parameters, innermost
/// activation last.
pub(super) struct LocalScope {
    names: HashMap<Symbol, u16>,
}

impl LocalScope {
    pub(super) fn new(names_in_order: &[Symbol]) -> LocalScope {
        let mut names = HashMap::new();
        for (index, &name) in names_in_order.iter().enumerate() {
            names.insert(name, index as u16);
        }
        LocalScope { names }
    }
}

/// The chain of activation scopes currently open while walking a method or
/// closure body, innermost (current) first.
pub(super) struct ScopeStack {
    scopes: Vec<LocalScope>,
}

impl ScopeStack {
    pub(super) fn new() -> ScopeStack {
        ScopeStack { scopes: Vec::new() }
    }

    pub(super) fn push(&mut self, scope: LocalScope) {
        self.scopes.push(scope);
    }

    pub(super) fn pop(&mut self) {
        self.scopes.pop();
    }

    /// Resolves `name` against the activation chain only. `Some(Local)` if
    /// found in the innermost scope, `Some(OuterLocal { depth, .. })` if
    /// found `depth` activations out, `None` if not a local at all (the
    /// caller then tries the lexical-send / class search).
    pub(super) fn resolve(&self, name: Symbol) -> Option<Binding> {
        for (depth, scope) in self.scopes.iter().rev().enumerate() {
            if let Some(&index) = scope.names.get(&name) {
                return Some(if depth == 0 {
                    Binding::Local(index)
                } else {
                    Binding::OuterLocal {
                        depth: depth as u16,
                        index,
                    }
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn innermost_scope_resolves_to_local() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x"), Symbol::intern("y")]));
        assert_eq!(stack.resolve(Symbol::intern("y")), Some(Binding::Local(1)));
    }

    #[test]
    fn enclosing_scope_resolves_to_outer_local() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")]));
        stack.push(LocalScope::new(&[Symbol::intern("y")]));
        assert_eq!(
            stack.resolve(Symbol::intern("x")),
            Some(Binding::OuterLocal { depth: 1, index: 0 })
        );
    }

    #[test]
    fn unknown_name_resolves_to_none() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")]));
        assert_eq!(stack.resolve(Symbol::intern("z")), None);
    }

    #[test]
    fn inner_shadows_outer() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")]));
        stack.push(LocalScope::new(&[Symbol::intern("x")]));
        assert_eq!(stack.resolve(Symbol::intern("x")), Some(Binding::Local(0)));
    }
}
