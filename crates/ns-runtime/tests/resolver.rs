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
