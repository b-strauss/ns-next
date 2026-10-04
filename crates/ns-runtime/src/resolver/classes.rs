//! Static member-set and superclass-name computation for classes, used only
//! to pick a lexical-send depth `k` (`docs/design/dispatch.md`,
//! "Resolution", step 2) -- never to resolve which concrete method body
//! runs (that is `dispatch.rs`'s job, at runtime, over the real object
//! graph).

use std::collections::HashSet;

use ns_syntax::ast;

use crate::mixin::RootClauseDefect;
use crate::Symbol;

/// The members one side of a class declaration *declares*, each under its
/// full selector, in declaration order and with any repeats kept -- what
/// `newspeak-spec.pdf` §5.7 searches, "a method, slot or class named s":
///
/// - every method, under its whole selector (`at:put:` declares `at:put:`,
///   never `at`);
/// - every slot's getter, under the slot's name, and a mutable slot's setter
///   under the name with `:` appended -- §6.3.2 has both "implicitly defined
///   by the slot declaration", and an immutable slot has "no setter method";
/// - every nested class, whose accessor has "the same name" (§6).
///
/// Nothing inherited: §5.7 asks where a member is *declared*.
fn declared_members(body: &ast::ClassBody) -> Vec<Symbol> {
    let mut members = Vec::new();
    for slot in &body.slots {
        members.push(Symbol::intern(&slot.name.name));
        if slot.mutability != ast::SlotMutability::Immutable {
            members.push(Symbol::intern(&format!("{}:", slot.name.name)));
        }
    }
    for method in &body.methods {
        members.push(Symbol::intern(
            &crate::interpreter::instantiate::method_name(&method.pattern),
        ));
    }
    for nested in &body.nested_classes {
        members.push(Symbol::intern(&nested.name.name));
    }
    members
}

/// The members one class declaration declares, both sides together
/// (`docs/design/dispatch.md`, "Resolution": the class side's own scope is an
/// open question the specification does not settle), plus the primary
/// factory selector.
///
/// The factory belongs here because `newspeak-spec.pdf` §6.3 makes it a
/// member of the metaclass mixin the class side becomes -- a member this
/// declaration declares, and therefore one §5.7's depth search must see.
/// Leaving it out bound an implicit `new` written inside a class-side method
/// to an *enclosing* class's member `new`, silently answering that enclosing
/// member instead of an instance of this class; `duplicate_member_names`
/// below already treats it as a class-side member, and the two must agree on
/// what the class side holds.
pub(super) fn own_member_names(declaration: &ast::ClassDeclaration) -> HashSet<Symbol> {
    let mut names: HashSet<Symbol> = declared_members(&declaration.instance_side)
        .into_iter()
        .collect();
    if let Some(class_side) = &declaration.class_side {
        names.extend(declared_members(class_side));
    }
    names.insert(factory_selector_of(declaration));
    names
}

/// The declaration's factory selector: the selector its own message pattern
/// spells out, or `new` when it writes none. `newspeak-spec.pdf` §6's
/// unnumbered introduction, before §6.1 begins: "The identifier may be
/// followed by a method signature for the *primary factory* method of the
/// class. If the signature is omitted it defaults to `new`." (§6.1's own
/// default-to-`new` is a different rule, about the *inheritance clause*'s
/// trailing message, not the factory signature.)
///
/// This mirrors `interpreter::instantiate::build_mixin`'s `factory` default
/// by hand, in a second block that also calls `method_name` and also
/// hardcodes `Symbol::NEW` -- they agree today because both transcribe the
/// same rule, not because one calls the other. Change what a class defaults
/// its factory to in one and the other must follow, or this check and the
/// metaclass mixin's actual factory disagree on what the reserved selector
/// is.
fn factory_selector_of(declaration: &ast::ClassDeclaration) -> Symbol {
    match &declaration.factory {
        Some(pattern) => Symbol::intern(&crate::interpreter::instantiate::method_name(pattern)),
        None => Symbol::NEW,
    }
}

/// The names one side of `declaration` declares more than once, each once.
/// The specification states no rule for two ordinary members of one name;
/// the canonical compiler refuses them
/// (`prior-art/newspeak/Newspeak2SqueakCompilation.ns`,
/// `checkForDuplicateNames:`), and a mixin's method table cannot hold two
/// members under one selector. Each side is its own table, so a name the
/// instance side and the class side both declare is not a duplicate.
///
/// The class side's table is not only what it declares, though: §6.3 makes
/// the primary factory a member of the metaclass mixin the class side
/// becomes, so a class-side method under the factory's own selector is a
/// second entry for that one name -- refused here the same way, by seeding
/// the class side's `seen` set with it before walking its declared members.
/// The image side (`image::mixins::check_member_names`) checks the same
/// collision on a decoded class-side row, against the mixin's own encoded
/// `Factory` -- populated, when the mixin was built, by this same
/// `method_name` computation -- so a source-level refusal here and an
/// image-level refusal there agree on exactly the same programs. Primordial
/// Soup refuses it too (`prior-art/primordialsoup/newspeak/Intermediates.ns`,
/// `checkNameConflictsForFactory:`); SOMns lets the declared method win and
/// files the factory under an unreachable name, after which the class no
/// longer instantiates and nothing says so.
pub(super) fn duplicate_member_names(declaration: &ast::ClassDeclaration) -> Vec<Symbol> {
    let mut duplicates = Vec::new();
    let mut seen = HashSet::new();
    for member in declared_members(&declaration.instance_side) {
        if !seen.insert(member) && !duplicates.contains(&member) {
            duplicates.push(member);
        }
    }
    if let Some(class_side) = declaration.class_side.as_ref() {
        let mut seen = HashSet::new();
        seen.insert(factory_selector_of(declaration));
        for member in declared_members(class_side) {
            if !seen.insert(member) && !duplicates.contains(&member) {
                duplicates.push(member);
            }
        }
    }
    duplicates
}

/// The resolve error for a superclass clause applying more mixins than a
/// class object's applied-clause field can number
/// (`crate::mixin::CLAUSE_INDEX_LIMIT`), or `None`.
pub(super) fn clause_count_error(count: usize, class_name: &str) -> Option<String> {
    (count > crate::mixin::CLAUSE_INDEX_LIMIT).then(|| {
        format!(
            "class `{class_name}` applies {count} mixins in its superclass clause; \
             at most {} are allowed",
            crate::mixin::CLAUSE_INDEX_LIMIT
        )
    })
}

/// The root rule (`docs/design/object-model.md`, "The root rule"): §6.1
/// evaluates a module root's clauses "as if [each] was a top level
/// expression", where §3.8 leaves nothing to send to, so the only classes a
/// root can reach are the ones the rule names -- `Object` or `Value` as its
/// superclass clause, sent `new` or nothing, and one unprefixed `Value`,
/// sent nothing, applied over `Object`. Everything after the header's `=`
/// is held to it; the primary factory before it is not.
///
/// Decided by the resolver, for every module root, before anything runs: a
/// module whose artifact `build` could never write must not run under
/// `run` either (`docs/design/README.md`, "Deviations from the language
/// specification"). The span is the offending clause's, for the editor to
/// underline. `interpreter::instantiate` records the same verdict on the
/// root mixin, which the image encoder refuses to write.
pub(crate) fn root_clause_defect(
    declaration: &ast::ClassDeclaration,
) -> Option<(RootClauseDefect, ns_syntax::Span)> {
    // The superclass clause's span when it names `Value`, which a `<:`
    // clause may not apply `Value` over.
    let value_superclass = match &declaration.superclass {
        None => None,
        Some(clause) => {
            let named = clause.receiver.name.as_str();
            if clause.prefix.is_some() || !matches!(named, "Object" | "Value") {
                return Some((
                    RootClauseDefect::Superclass {
                        clause: clause_as_written(clause),
                    },
                    clause.span,
                ));
            }
            if let Some(message) = clause.messages.first() {
                if !matches!(&message.selector, ast::Selector::Unary(selector) if selector == "new")
                {
                    return Some((
                        RootClauseDefect::SuperclassMessage {
                            clause: clause_as_written(clause),
                            selector: selector_name(&message.selector).to_string(),
                        },
                        clause.span,
                    ));
                }
            }
            (named == "Value").then_some(clause.span)
        }
    };
    for (index, clause) in declaration.mixin_applications.iter().enumerate() {
        if index > 0 || clause.prefix.is_some() || clause.receiver.name != "Value" {
            return Some((
                RootClauseDefect::MixinApplication {
                    index,
                    clause: clause_as_written(clause),
                },
                clause.span,
            ));
        }
        if let Some(message) = clause.messages.first() {
            return Some((
                RootClauseDefect::MixinApplicationMessage {
                    index,
                    clause: clause_as_written(clause),
                    selector: selector_name(&message.selector).to_string(),
                },
                clause.span,
            ));
        }
        if let Some(span) = value_superclass {
            return Some((RootClauseDefect::ValueOverValue, span));
        }
    }
    None
}

/// The two choices the root rule leaves a root class without a defect: built
/// over `Value` rather than `Object`, and with a `Value` level applied by a
/// `<:` clause over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RootShape {
    pub(crate) over_value: bool,
    pub(crate) applies_value: bool,
}

/// How a root class is built, read from the clauses `root_clause_defect`
/// accepts; `None` for a root with a defect.
pub(crate) fn root_shape(declaration: &ast::ClassDeclaration) -> Option<RootShape> {
    if root_clause_defect(declaration).is_some() {
        return None;
    }
    Some(RootShape {
        over_value: declaration
            .superclass
            .as_ref()
            .is_some_and(|clause| clause.receiver.name == "Value"),
        applies_value: !declaration.mixin_applications.is_empty(),
    })
}

/// Whether a module's root class is a value class: one the root rule
/// accepts, built over `Value` (`Value new`) or with `Value` applied over
/// `Object` (`Object <: Value`). Exact, since the rule leaves a root no other
/// way to reach `Value`; the editor's structure view asks it.
pub fn root_class_is_value(declaration: &ast::ClassDeclaration) -> bool {
    root_shape(declaration).is_some_and(|shape| shape.over_value || shape.applies_value)
}

/// A clause's prefix and receiver, as the program wrote them -- what a
/// refusal names; a message at fault is named apart.
fn clause_as_written(clause: &ast::SuperclassClause) -> String {
    let prefix = match &clause.prefix {
        None => String::new(),
        Some(ast::SuperclassPrefix::SelfPrefix) => "self ".to_string(),
        Some(ast::SuperclassPrefix::SuperPrefix) => "super ".to_string(),
        Some(ast::SuperclassPrefix::Outer(name)) => format!("outer {} ", name.name),
    };
    format!("{prefix}{}", clause.receiver.name)
}

fn selector_name(selector: &ast::Selector) -> &str {
    match selector {
        ast::Selector::Unary(name) | ast::Selector::Binary(name) | ast::Selector::Keyword(name) => {
            name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clause_count_error;

    #[test]
    fn a_superclass_clause_of_more_than_two_to_the_twenty_three_clauses_is_refused() {
        assert!(clause_count_error(crate::mixin::CLAUSE_INDEX_LIMIT, "Wide").is_none());
        let message =
            clause_count_error(crate::mixin::CLAUSE_INDEX_LIMIT + 1, "Wide").expect("refused");
        assert_eq!(
            message,
            "class `Wide` applies 8388609 mixins in its superclass clause; at most 8388608 are allowed"
        );
    }
}
