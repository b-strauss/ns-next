//! Static (activation-chain) local-variable resolution: method/block
//! temporaries and parameters. See `docs/design/dispatch.md`,
//! "Resolution", step 1.

use std::collections::HashMap;

use ns_syntax::ast;

use crate::types::Binding;
use crate::Symbol;

/// One method or closure body's own parameters and local slots, innermost
/// activation last, each with whether it declares a setter: §3.6 makes a
/// parameter immutable and a local slot what its declaration says (§6.3.2),
/// so only a mutable local slot has one.
pub(super) struct LocalScope {
    names: HashMap<Symbol, LocalEntry>,
}

#[derive(Clone, Copy)]
struct LocalEntry {
    index: u16,
    has_setter: bool,
}

impl LocalScope {
    /// Indices follow the activation's layout: the parameters, then the
    /// local slots, each in declaration order.
    pub(super) fn new(parameters: &[Symbol], temporaries: &[ast::SlotDeclaration]) -> LocalScope {
        let declared = parameters
            .iter()
            .map(|&name| (name, false))
            .chain(temporaries.iter().map(|temporary| {
                (
                    Symbol::intern(&temporary.name.name),
                    temporary.mutability != ast::SlotMutability::Immutable,
                )
            }));
        let mut names = HashMap::new();
        for (index, (name, has_setter)) in declared.enumerate() {
            names.insert(
                name,
                LocalEntry {
                    index: index as u16,
                    has_setter,
                },
            );
        }
        LocalScope { names }
    }
}

/// The chain of activation scopes currently open while walking a method or
/// closure body, innermost (current) first.
pub(super) struct ScopeStack {
    scopes: Vec<LocalScope>,
}

/// `Binding::Local` at depth 0, `Binding::OuterLocal` otherwise -- shared
/// between `resolve` and `resolve_setter`, the only difference between them
/// being which entries they consider a match.
fn binding_at(depth: usize, index: u16) -> Binding {
    if depth == 0 {
        Binding::Local(index)
    } else {
        Binding::OuterLocal {
            depth: depth as u16,
            index,
        }
    }
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
            if let Some(&entry) = scope.names.get(&name) {
                return Some(binding_at(depth, entry.index));
            }
        }
        None
    }

    /// Resolves the target of `name:: e` against the activation chain: the
    /// innermost local that declares a setter. A local without one is passed
    /// by, whether or not it shadows one further out -- the setter send looks
    /// for a declaration of `name:` (§5.4.3, §5.7), and a parameter or an
    /// immutable local declares only `name`. `None` hands the search on to
    /// the enclosing classes.
    pub(super) fn resolve_setter(&self, name: Symbol) -> Option<Binding> {
        for (depth, scope) in self.scopes.iter().rev().enumerate() {
            if let Some(entry) = scope.names.get(&name) {
                if entry.has_setter {
                    return Some(binding_at(depth, entry.index));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ns_syntax::Span;

    /// A local slot declaration for a test, at the placeholder span every
    /// test in this module uses.
    fn slot_declaration(name: &str, mutability: ast::SlotMutability) -> ast::SlotDeclaration {
        ast::SlotDeclaration {
            access: None,
            name: ast::Identifier {
                name: name.to_string(),
                span: Span::new(0, 0),
            },
            mutability,
            initializer: None,
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn innermost_scope_resolves_to_local() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(
            &[Symbol::intern("x"), Symbol::intern("y")],
            &[],
        ));
        assert_eq!(stack.resolve(Symbol::intern("y")), Some(Binding::Local(1)));
    }

    #[test]
    fn enclosing_scope_resolves_to_outer_local() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")], &[]));
        stack.push(LocalScope::new(&[Symbol::intern("y")], &[]));
        assert_eq!(
            stack.resolve(Symbol::intern("x")),
            Some(Binding::OuterLocal { depth: 1, index: 0 })
        );
    }

    #[test]
    fn unknown_name_resolves_to_none() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")], &[]));
        assert_eq!(stack.resolve(Symbol::intern("z")), None);
    }

    #[test]
    fn inner_shadows_outer() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")], &[]));
        stack.push(LocalScope::new(&[Symbol::intern("x")], &[]));
        assert_eq!(stack.resolve(Symbol::intern("x")), Some(Binding::Local(0)));
    }

    #[test]
    fn a_parameter_resolves_but_declares_no_setter() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("value")], &[]));
        assert_eq!(
            stack.resolve(Symbol::intern("value")),
            Some(Binding::Local(0))
        );
        assert_eq!(stack.resolve_setter(Symbol::intern("value")), None);
    }

    #[test]
    fn a_shadowing_parameter_passes_the_setter_send_to_the_outer_mutable_local() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(
            &[],
            &[slot_declaration("value", ast::SlotMutability::MutableBare)],
        ));
        stack.push(LocalScope::new(&[Symbol::intern("value")], &[]));
        assert_eq!(
            stack.resolve_setter(Symbol::intern("value")),
            Some(Binding::OuterLocal { depth: 1, index: 0 })
        );
    }
}
