//! Static member-set and superclass-name computation for classes, used only
//! to pick a lexical-send depth `k` (`docs/design/dispatch.md`,
//! "Resolution", step 2) -- never to resolve which concrete method body
//! runs (that is `dispatch.rs`'s job, at runtime, over the real object
//! graph).

use std::collections::HashSet;

use ns_syntax::ast;

use crate::Symbol;

/// Names Object supports with no Newspeak-source declaration to find --
/// resolving a superclass-clause receiver (or a bare implicit send) against
/// these is the base case that lets a lexical/inheritance search bottom out.
/// Extend this list as the core library gains classes and public selectors
/// that user code may reasonably reference by bare name from a superclass
/// clause (in practice, just the class *names* below matter for superclass
/// resolution -- Object's own instance-side selectors matter for implicit
/// sends that fall all the way through to Object).
pub(super) const BUILTIN_CLASS_NAMES: &[&str] = &[
    "Object",
    "Boolean",
    "True",
    "False",
    "Integer",
    "Float",
    "String",
    "Symbol",
    "Character",
    "Array",
    "OrderedCollection",
    "Nil",
    "Block",
];

pub(super) fn is_builtin_class_name(name: Symbol) -> bool {
    BUILTIN_CLASS_NAMES.contains(&name.as_str())
}

/// Names declared *directly* by one class declaration: its own methods
/// (both sides' method selectors, unary base name for keyword/binary
/// selectors is the *first* keyword/the operator itself -- see
/// `member_name_of_pattern` below), its own slots, and its own immediately
/// nested class names.
pub(super) fn own_member_names(decl: &ast::ClassDecl) -> HashSet<Symbol> {
    let mut names = HashSet::new();
    let mut collect_body = |body: &ast::ClassBody| {
        for slot in &body.slots {
            names.insert(Symbol::intern(&slot.name.name));
        }
        for method in &body.methods {
            names.insert(member_name_of_pattern(&method.pattern));
        }
        for nested in &body.nested_classes {
            names.insert(Symbol::intern(&nested.name.name));
        }
    };
    collect_body(&decl.instance_side);
    if let Some(class_side) = &decl.class_side {
        collect_body(class_side);
    }
    names
}

/// The name a message pattern is looked up under for §5.7's search: a
/// setter (`foo::`) counts under its getter's plain name `foo`, matching
/// how a mutable slot's setter and getter share one declared slot name.
fn member_name_of_pattern(pattern: &ast::MessagePattern) -> Symbol {
    match pattern {
        ast::MessagePattern::Unary { selector } => Symbol::intern(&selector.name),
        ast::MessagePattern::Binary { selector, .. } => Symbol::intern(&selector.name),
        ast::MessagePattern::Keyword { parts } => {
            // The full keyword selector's first component names the member
            // for §5.7 lookup purposes (e.g. `at:put:` is found under `at`).
            Symbol::intern(&parts[0].0.name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_are_recognized() {
        assert!(is_builtin_class_name(Symbol::intern("Object")));
        assert!(!is_builtin_class_name(Symbol::intern("NotABuiltin")));
    }
}
