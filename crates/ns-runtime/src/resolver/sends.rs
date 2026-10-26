//! Recognizes self/super/outer/implicit send shapes in the AST (runtime
//! spec's "AST recognition" note) and runs the §5.7 depth search for
//! implicit and bare-name sends.

use ns_syntax::ast::{Expression, ExpressionKind, Selector};

use super::scope::lexical_depth;
use crate::Symbol;

/// One entry in the lexical class-nesting chain currently open while
/// walking a method body, innermost first. `members` is what that class
/// declaration itself declares, under full selectors
/// (`classes::own_member_names`) -- never what it inherits
/// (`newspeak-spec.pdf` §5.7).
#[derive(Clone, Copy)]
pub(super) struct EnclosingClass<'a> {
    pub name: Symbol,
    pub members: &'a std::collections::HashSet<Symbol>,
}

/// Runs the §5.7 depth search: the smallest `depth` (0 = innermost enclosing
/// class) whose member set contains `selector`. Falls back to depth `0`
/// (self send) if nothing matches anywhere, per the spec ("otherwise, the
/// implicit receiver send is equivalent to a self send") -- runtime
/// dispatch on that then naturally produces `doesNotUnderstand:`.
pub(super) fn depth_search(chain: &[EnclosingClass], selector: Symbol) -> u16 {
    for (depth, class) in chain.iter().enumerate() {
        if class.members.contains(&selector) {
            return lexical_depth(depth);
        }
    }
    0
}

/// Like `depth_search`, but returns `None` instead of defaulting to depth 0
/// when no lexical level's member set contains `selector`. Used where the
/// caller must distinguish "found nowhere" from "found at depth 0" -- e.g.
/// resolving a superclass-clause receiver, which is looked up exactly like
/// a method or slot name would be (`docs/design/dispatch.md`, "Binding
/// variants"), where "not found anywhere" must fall through to the
/// builtin-class-name check rather than silently becoming a self send.
pub(super) fn find_member_depth(chain: &[EnclosingClass], selector: Symbol) -> Option<u16> {
    chain
        .iter()
        .position(|class| class.members.contains(&selector))
        .map(lexical_depth)
}

/// Finds the exact depth of the lexically enclosing class named `name`, for
/// an explicit `outer name m` send. `Err` is a compile-time error per
/// §5.9 ("It is a compile time error if N is not the name of a class whose
/// declaration lexically encloses the outer send").
pub(super) fn depth_of_named_outer(chain: &[EnclosingClass], name: Symbol) -> Option<u16> {
    chain
        .iter()
        .position(|class| class.name == name)
        .map(lexical_depth)
}

/// Recognizes whether `send.receiver` is the parser's implicit receiver:
/// `Identifier { name: "self" }` marked `synthesized` by
/// `parse_keyword_message` in `crates/ns-syntax/src/parser/expression.rs`.
/// A written `self` is a genuine explicit self send (§5.8, always depth 0,
/// no search). The flag, not the span, decides: a tree without positions
/// has an empty span on every node.
pub(super) fn is_synthesized_implicit_receiver(receiver: &Expression) -> bool {
    receiver.synthesized
        && matches!(&receiver.kind, ExpressionKind::Identifier(id) if id.name == "self")
}

pub(super) fn is_explicit_self(receiver: &Expression) -> bool {
    !receiver.synthesized
        && matches!(&receiver.kind, ExpressionKind::Identifier(id) if id.name == "self")
}

pub(super) fn is_explicit_super(receiver: &Expression) -> bool {
    matches!(&receiver.kind, ExpressionKind::Identifier(id) if id.name == "super")
}

/// Recognizes `outer N` itself -- a `Send` of the unary `N` to
/// `Identifier("outer")` -- where it is not a further send's receiver: the
/// resolver reaches a send's receiver only through `outer_class_name`, which
/// takes the whole `outer N m`, so a `send` of this shape that reaches the
/// resolver on its own is the enclosing object as a value.
pub(crate) fn bare_outer_class_name(send: &ns_syntax::ast::Send) -> Option<&str> {
    let ExpressionKind::Identifier(outer_identifier) = &send.receiver.kind else {
        return None;
    };
    if outer_identifier.name != "outer" {
        return None;
    }
    match &send.message.selector {
        Selector::Unary(name) => Some(name.as_str()),
        _ => None,
    }
}

/// Recognizes the `outer N m` two-hop shape: `receiver` is itself a `Send`
/// whose own receiver is `Identifier("outer")` and whose message is a unary
/// send. Returns that selector's name if so. §5.9 takes any identifier for
/// `N` -- `outer public m` names an enclosing class `public` -- and makes it
/// a compile-time error when no lexically enclosing class has that name,
/// which the caller reports; nothing about `N`'s spelling decides it.
/// The inner "send N to outer" step is never dispatched as a real message —
/// callers that get `Some(name)` back must resolve `name`'s depth via
/// `depth_of_named_outer` and use the *outer* `Send`'s own message/selector
/// (`send.message`) as the actual send, discarding the inner shape.
pub(crate) fn outer_class_name(receiver: &Expression) -> Option<&str> {
    let ExpressionKind::Send(inner) = &receiver.kind else {
        return None;
    };
    let ExpressionKind::Identifier(outer_identifier) = &inner.receiver.kind else {
        return None;
    };
    if outer_identifier.name != "outer" {
        return None;
    }
    match &inner.message.selector {
        Selector::Unary(name) => Some(name.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn class<'a>(name: &str, members: &'a HashSet<Symbol>) -> EnclosingClass<'a> {
        EnclosingClass {
            name: Symbol::intern(name),
            members,
        }
    }

    #[test]
    fn depth_search_finds_the_innermost_match() {
        let mut inner_members = HashSet::new();
        inner_members.insert(Symbol::intern("foo"));
        let outer_members = HashSet::new();
        let chain = [
            class("Inner", &inner_members),
            class("Outer", &outer_members),
        ];
        assert_eq!(depth_search(&chain, Symbol::intern("foo")), 0);
    }

    #[test]
    fn depth_search_falls_through_to_an_outer_level() {
        let inner_members = HashSet::new();
        let mut outer_members = HashSet::new();
        outer_members.insert(Symbol::intern("foo"));
        let chain = [
            class("Inner", &inner_members),
            class("Outer", &outer_members),
        ];
        assert_eq!(depth_search(&chain, Symbol::intern("foo")), 1);
    }

    #[test]
    fn find_member_depth_returns_none_when_nothing_matches() {
        let members = HashSet::new();
        let chain = [class("Inner", &members)];
        assert_eq!(find_member_depth(&chain, Symbol::intern("nowhere")), None);
    }

    #[test]
    fn find_member_depth_finds_a_real_match() {
        let mut outer_members = HashSet::new();
        outer_members.insert(Symbol::intern("Foo"));
        let inner_members = HashSet::new();
        let chain = [
            class("Inner", &inner_members),
            class("Outer", &outer_members),
        ];
        assert_eq!(find_member_depth(&chain, Symbol::intern("Foo")), Some(1));
    }

    #[test]
    fn depth_search_defaults_to_zero_when_nothing_matches() {
        let members = HashSet::new();
        let chain = [class("Inner", &members)];
        assert_eq!(depth_search(&chain, Symbol::intern("nowhere")), 0);
    }

    #[test]
    fn depth_of_named_outer_finds_an_exact_class() {
        let members = HashSet::new();
        let chain = [class("Inner", &members), class("Outer", &members)];
        assert_eq!(
            depth_of_named_outer(&chain, Symbol::intern("Outer")),
            Some(1)
        );
        assert_eq!(
            depth_of_named_outer(&chain, Symbol::intern("Missing")),
            None
        );
    }
}
