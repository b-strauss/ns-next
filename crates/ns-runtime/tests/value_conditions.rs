//! §3.1.1's value conditions and the root rule (§6.1, §3.8), enforced
//! strictly: `docs/design/object-model.md`, "Planned: all four conditions,
//! enforced strictly".

use indoc::indoc;

use ns_runtime::{HostResult, HostSignal, Interpreter, Root, Symbol, Value};

/// Parses, resolves and loads `source` (the module's root class, into a
/// bootstrapped `Interpreter`) into a real class object, returning both.
/// Same shape as `tests/instantiate.rs`'s helper.
fn load(source: &str) -> (Interpreter, Root, ns_runtime::ResolvedModule) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class, resolved)
}

/// `HostSignal` carries no `Debug` impl, so `Result::expect` isn't usable on
/// a `HostResult` -- unwrap by hand, naming which variant leaked. Copied
/// from `tests/nested_classes.rs`, which this file's send-based tests share
/// the shape of.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// See `expect_ok`: copied from `tests/nested_classes.rs` for the same reason.
fn expect_error(result: HostResult, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}: expected a Newspeak error"),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => error.message,
    }
}

/// Every root defect is deferred: the module loads, its class exists and
/// answers its class side, and only an instance is refused -- which is what
/// lets a live system inspect and fix a module whose root is wrong.
#[test]
fn a_module_with_a_defective_root_loads_and_refuses_only_an_instance() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Defective = Array new () () : (
            public answer = ( ^42 )
        )
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let answer = expect_ok(
        interpreter.send(&class, Symbol::intern("answer"), &[]),
        "Defective answer",
    );
    interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&answer), Value::SmallInteger(42)));
    });
    let message = expect_error(interpreter.instantiate(&class, &[]), "Defective new");
    assert_eq!(
        message,
        "cannot instantiate Defective: a module's root class may inherit only Object or Value, \
         or write Object <: Value, and its superclass clause names Array"
    );
}

/// The forms the rule allows load and instantiate as before.
#[test]
fn every_allowed_root_form_instantiates() {
    for header in [
        "class Allowed = ()",
        "class Allowed = Object new ()",
        "class Allowed = Value new ()",
        "class Allowed = Object <: Value ()",
        "class Allowed = Object new <: Value ()",
    ] {
        let source = format!("Newspeak3\n'Test'\n{header} ()\n");
        let (mut interpreter, class, _resolved) = load(&source);
        expect_ok(interpreter.instantiate(&class, &[]), header);
    }
}

/// §6.2 lets `<: Value` put `Value`'s level below an arbitrary class, so a
/// mutable slot declared above that level -- not only below it -- reaches a
/// value's instances. The class that declares the mutable slot is no value
/// itself and keeps the slot legally; only the class whose chain inherits
/// `Value` through the `<:` clause is refused, where its instance would come
/// into being.
#[test]
fn a_mutable_slot_above_the_value_level_is_refused() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class MutableSlotAboveValue usingPlatform: platformAtCreation = Object <: Value () (
            public class Counting = Object new (
                | public count ::= 0. |
            ) ()
            public class Counted = Counting new <: Value () ()
        ) : ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let platform = interpreter.platform();
    let module = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "MutableSlotAboveValue usingPlatform:",
    );
    let counting = expect_ok(
        interpreter.send(&module, Symbol::intern("Counting"), &[]),
        "MutableSlotAboveValue Counting",
    );
    expect_ok(interpreter.instantiate(&counting, &[]), "Counting new");

    let counted = expect_ok(
        interpreter.send(&module, Symbol::intern("Counted"), &[]),
        "MutableSlotAboveValue Counted",
    );
    let message = expect_error(interpreter.instantiate(&counted, &[]), "Counted new");
    assert_eq!(
        message,
        "cannot instantiate Counted: a value object is deeply immutable, and Counting declares \
         the mutable slot count"
    );
}

/// A root clause the resolver refused until 2026-09-23 now resolves, and is
/// refused at instantiation like every other root defect.
#[test]
fn an_unknown_root_superclass_is_a_deferred_defect_not_a_resolve_error() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Unknown = Missing new () ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let message = expect_error(interpreter.instantiate(&class, &[]), "Unknown new");
    assert!(
        message.ends_with("its superclass clause names Missing"),
        "{message}"
    );
}

/// The walk stops at the first class that was itself created as a value
/// class, so a subclass of a checked value class is not refused for a level
/// above it that was already vouched for -- and a subclass declared in an
/// ordinary module still has its own enclosing object checked. §2.3's own
/// example creates such a class and never instantiates it, so the class
/// object itself comes into being without complaint
/// (`docs/design/object-model.md`, "The two new conditions are decided
/// where a class object is created and refused where an instance is
/// made"); only `Derived new` is refused.
#[test]
fn a_value_subclass_is_checked_for_its_own_enclosing_object() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Outer = Object <: Value () (
            public class Base = Value new () ()
            public class Inner = Object new () (
                public class Derived = Base new () ()
            )
            public class Sibling = Base new () ()
        ) : ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let outer = expect_ok(interpreter.instantiate(&class, &[]), "Outer new");
    expect_ok(
        interpreter.send(&outer, Symbol::intern("Sibling"), &[]),
        "Sibling",
    );
    let inner_class = expect_ok(
        interpreter.send(&outer, Symbol::intern("Inner"), &[]),
        "Inner",
    );
    let inner = expect_ok(interpreter.instantiate(&inner_class, &[]), "Inner new");
    let derived = expect_ok(
        interpreter.send(&inner, Symbol::intern("Derived"), &[]),
        "Derived",
    );
    let message = expect_error(interpreter.instantiate(&derived, &[]), "Derived new");
    assert_eq!(
        message,
        "cannot instantiate Derived: a value class's enclosing objects must be values, and its \
         enclosing object, an instance of Inner, is not one"
    );
}

/// §2.3's own point, pinned directly: a value class whose enclosing object
/// is not a value can still be *created* -- the send answers a class
/// object, with no error -- and only instantiating it is refused.
/// `docs/design/object-model.md`, "The two new conditions...", names the
/// specification's own worked example this follows
/// (`goldens/spec/spec_2_3_superclass_of_a_nested_class.ns`, which creates
/// such a class and never instantiates it).
#[test]
fn a_value_class_over_a_non_value_enclosing_object_is_created_but_not_instantiated() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Root = Object new () (
            public class Described = Object new <: Value () ()
        ) : ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let root = expect_ok(interpreter.instantiate(&class, &[]), "Root new");
    let described = expect_ok(
        interpreter.send(&root, Symbol::intern("Described"), &[]),
        "Described",
    );
    let message = expect_error(interpreter.instantiate(&described, &[]), "Described new");
    assert_eq!(
        message,
        "cannot instantiate Described: a value class's enclosing objects must be values, and \
         its enclosing object, an instance of Root, is not one"
    );
}
