use indoc::indoc;
use ns_runtime::{resolve, Binding, SourceOrigin};

fn resolve_source(source: &str) -> ns_runtime::ResolvedModule {
    let module = ns_syntax::parse(source).expect("parse");
    resolve(module, source.into(), SourceOrigin::File("test.ns".into())).expect("resolve")
}

#[test]
fn method_temporary_resolves_as_local() {
    let resolved = resolve_source(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          bar = ( | x | x )
        ) : ()
    "});
    // At least one Local binding exists somewhere in the module (the `x`
    // reference in the method body).
    assert!(resolved
        .bindings
        .values()
        .any(|binding| matches!(binding, Binding::Local(_))));
}

#[test]
fn self_send_resolves_to_depth_zero() {
    let resolved = resolve_source(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          bar = ( ^self baz )
          baz = ( ^1 )
        ) : ()
    "});
    assert!(resolved
        .bindings
        .values()
        .any(|binding| matches!(binding, Binding::LexicalSend { depth: 0 })));
}

#[test]
fn super_send_resolves_to_super_send_binding() {
    let resolved = resolve_source(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          bar = ( ^super baz )
        ) : ()
    "});
    assert!(resolved
        .bindings
        .values()
        .any(|binding| *binding == Binding::SuperSend));
}

#[test]
fn outer_named_send_resolves_to_the_exact_depth() {
    let resolved = resolve_source(indoc! {"
        Newspeak3
        'T'
        class Outer = Object new (
        ) (
          outerMethod = ( ^1 )
          public class Inner = Object new (
          ) (
            innerMethod = ( ^outer Outer outerMethod )
          ) : ()
        ) : ()
    "});
    assert!(resolved
        .bindings
        .values()
        .any(|binding| matches!(binding, Binding::LexicalSend { depth: 1 })));
}

/// A bare reference to a builtin class name --
/// outside any superclass clause -- must resolve without a `ResolveError`,
/// and must record *no* `Binding` at all at that identifier's span,
/// mirroring `resolve_superclass_clause`'s existing convention for the
/// analogous superclass-clause case (see `classes::is_builtin_class_name`).
/// Locates the exact span of the `OrderedCollection` identifier by walking
/// the parsed AST directly, rather than scanning `bindings.values()`
/// (which cannot distinguish "no binding recorded here" from "no bindings
/// anywhere").
#[test]
fn bare_builtin_class_name_resolves_with_no_binding_at_its_span() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          bar = ( | c | c := OrderedCollection new. ^c )
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module.clone(),
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve must succeed -- a bare builtin class name is not a resolve error");

    let method = &module.root_class.instance_side.methods[0];
    let ns_syntax::ast::Statement::Expression(ns_syntax::ast::Expression::Assign { value, .. }) =
        &method.body.statements[0]
    else {
        panic!("expected the method's first statement to be `c := OrderedCollection new.`");
    };
    let ns_syntax::ast::Expression::Send(send) = value.as_ref() else {
        panic!("expected the assignment's value to be a `Send` (`OrderedCollection new`)");
    };
    let ns_syntax::ast::Expression::Identifier(receiver_id) = &send.receiver else {
        panic!("expected the send's receiver to be a bare `Identifier` (`OrderedCollection`)");
    };
    assert_eq!(receiver_id.name, "OrderedCollection");
    assert!(
        resolved.bindings.get(&receiver_id.span).is_none(),
        "a bare builtin class name reference must record no Binding at its span"
    );
}

#[test]
fn undeclared_outer_class_name_is_a_resolve_error() {
    let module = ns_syntax::parse(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          bar = ( ^outer Missing baz )
        ) : ()
    "})
    .expect("parse");
    let result = resolve(module, "src".into(), SourceOrigin::File("test.ns".into()));
    assert!(result.is_err());
}

/// The resolve errors `source` produces, as messages; empty when it resolves.
fn resolve_errors(source: &str) -> Vec<String> {
    let module = ns_syntax::parse(source).expect("parse");
    match resolve(module, source.into(), SourceOrigin::File("test.ns".into())) {
        Ok(_) => Vec::new(),
        Err(errors) => errors.into_iter().map(|error| error.message).collect(),
    }
}

// Two members of one declaration under one name. `newspeak-spec.pdf` states
// no rule for them; the canonical compiler refuses them
// (`prior-art/newspeak/Newspeak2SqueakCompilation.ns`,
// `checkForDuplicateNames:`, which counts each method's selector, each
// slot's name, each mutable slot's setter and each nested class's name), and
// a mixin's method table cannot hold two members under one selector.

#[test]
fn a_slot_and_a_method_of_one_name_are_a_resolve_error() {
    let errors = resolve_errors(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
          | x ::= 1. |
        ) (
          x = ( ^2 )
        ) : ()
    "});
    assert_eq!(errors, ["class `Foo` has multiple definitions of `x`"]);
}

/// §6.3.2: a mutable slot's setter is "implicitly defined by the slot
/// declaration", so a method `x:` beside it defines `x:` twice.
#[test]
fn a_mutable_slot_and_a_method_of_its_setters_name_are_a_resolve_error() {
    let errors = resolve_errors(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
          | x ::= 1. |
        ) (
          x: value = ( ^value )
        ) : ()
    "});
    assert_eq!(errors, ["class `Foo` has multiple definitions of `x:`"]);
}

/// §6.3.2: an immutable slot has "no setter method", and "defining a setter
/// will not impact such an initialization" -- so a method `x:` beside an
/// immutable slot `x` is legitimate.
#[test]
fn an_immutable_slot_and_a_method_of_its_setters_name_resolve() {
    let errors = resolve_errors(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
          | x = 1. |
        ) (
          x: value = ( ^value )
        ) : ()
    "});
    assert!(errors.is_empty(), "{errors:?}");
}

/// §6: a nested class declaration "implicitly defines a method with the same
/// name", so a method of that name defines it twice.
#[test]
fn a_nested_class_and_a_method_of_one_name_are_a_resolve_error() {
    let errors = resolve_errors(indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          class Inner = Object new () ()
          Inner = ( ^2 )
        ) : ()
    "});
    assert_eq!(errors, ["class `Foo` has multiple definitions of `Inner`"]);
}

/// §6.3 makes the primary factory a member of the metaclass mixin the class
/// side becomes, so a method table has one entry per selector to give it --
/// the same rule as a slot and a method of one name, extended to the
/// factory. Primordial Soup refuses it too (`Intermediates.ns`,
/// `checkNameConflictsForFactory:`).
#[test]
fn a_class_side_may_not_declare_the_factory_selector() {
    let errors = resolve_errors(indoc! {"
        Newspeak3
        'Test'
        class Conflict usingPlatform: platform = Object new () (
            class Widget new = Object new () (
            ) : (
                public new = ( ^'not a widget' )
            )
        ) : ()
    "});
    assert_eq!(errors, ["class `Widget` has multiple definitions of `new`"]);
}

/// A class written with no factory pattern defaults to `new` (§6.1), so the
/// collision is refused even when nothing in the source spells the selector
/// out.
#[test]
fn a_class_side_may_not_declare_the_default_factory_selector() {
    let errors = resolve_errors(indoc! {"
        Newspeak3
        'Test'
        class Conflict usingPlatform: platform = Object new () (
            class Widget = Object new () (
            ) : (
                public new = ( ^'not a widget' )
            )
        ) : ()
    "});
    assert_eq!(errors, ["class `Widget` has multiple definitions of `new`"]);
}

/// A class side under a different selector than the factory's is
/// legitimate -- the check must refuse only the method under the factory's
/// own selector, not every class-side method.
#[test]
fn a_class_side_method_under_another_selector_resolves() {
    let errors = resolve_errors(indoc! {"
        Newspeak3
        'Test'
        class Widget new = Object new () (
        ) : (
            public make = ( ^'not a widget' )
        )
    "});
    assert!(errors.is_empty(), "{errors:?}");
}
