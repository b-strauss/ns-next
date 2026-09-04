use ns_runtime::{resolve, Binding, SourceOrigin};

fn resolve_source(src: &str) -> ns_runtime::ResolvedModule {
    let module = ns_syntax::parse(src).expect("parse");
    resolve(module, src.into(), SourceOrigin::File("test.ns".into())).expect("resolve")
}

#[test]
fn method_temp_resolves_as_local() {
    let resolved = resolve_source(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  bar = ( | x | x )\n) : ()",
    );
    // At least one Local binding exists somewhere in the module (the `x`
    // reference in the method body).
    assert!(resolved
        .bindings
        .values()
        .any(|b| matches!(b, Binding::Local(_))));
}

#[test]
fn self_send_resolves_to_depth_zero() {
    let resolved = resolve_source(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  bar = ( ^self baz )\n  baz = ( ^1 )\n) : ()",
    );
    assert!(resolved
        .bindings
        .values()
        .any(|b| matches!(b, Binding::LexicalSend { depth: 0 })));
}

#[test]
fn super_send_resolves_to_super_send_binding() {
    let resolved = resolve_source(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  bar = ( ^super baz )\n) : ()",
    );
    assert!(resolved.bindings.values().any(|b| *b == Binding::SuperSend));
}

#[test]
fn outer_named_send_resolves_to_the_exact_depth() {
    let resolved = resolve_source(
        "Newspeak3\n'T'\nclass Outer = Object new (\n) (\n  outerMethod = ( ^1 )\n  public class Inner = Object new (\n  ) (\n    innerMethod = ( ^outer Outer outerMethod )\n  ) : ()\n) : ()",
    );
    assert!(resolved
        .bindings
        .values()
        .any(|b| matches!(b, Binding::LexicalSend { depth: 1 })));
}

#[test]
fn undeclared_outer_class_name_is_a_resolve_error() {
    let module = ns_syntax::parse(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  bar = ( ^outer Missing baz )\n) : ()",
    )
    .expect("parse");
    let result = resolve(module, "src".into(), SourceOrigin::File("test.ns".into()));
    assert!(result.is_err());
}
