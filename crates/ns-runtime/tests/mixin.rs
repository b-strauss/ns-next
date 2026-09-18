use indoc::indoc;
use std::rc::Rc;

use ns_runtime::dispatch::{self, Lookup};
use ns_runtime::mixin::{mixin_key, AccessorKind, Mixin};
use ns_runtime::{HostResult, HostSignal, Interpreter, Mutator, Root, Symbol, Value};
use ns_syntax::{ast, span::Span};

fn slot_declaration(name: &str) -> ns_runtime::mixin::MixinSlot {
    ns_runtime::mixin::MixinSlot {
        name: Symbol::intern(name),
        mutable: false,
        access: ast::AccessModifier::Protected,
        declaration: Some(raw_slot_declaration(name)),
    }
}

fn raw_slot_declaration(name: &str) -> Rc<ast::SlotDeclaration> {
    Rc::new(ast::SlotDeclaration {
        access: None,
        name: ast::Identifier {
            name: name.to_string(),
            span: Span::new(0, 0),
        },
        mutability: ast::SlotMutability::Immutable,
        initializer: None,
        span: Span::new(0, 0),
    })
}

fn bare_mixin(name: &str) -> Rc<Mixin> {
    Rc::new(Mixin::empty(Symbol::intern(name)))
}

#[test]
fn applying_the_same_mixin_to_the_same_superclass_is_cached() {
    let mut interpreter = Interpreter::bootstrap();
    let mixin = bare_mixin("Inner");
    let (first, second) = interpreter.enter(|mutator| {
        (
            mutator.apply_mixin(&mixin, None),
            mutator.apply_mixin(&mixin, None),
        )
    });
    assert!(
        Rc::ptr_eq(&first, &second),
        "the application cache must return one behaviour per (mixin, super)"
    );
}

#[test]
fn different_superclasses_produce_different_applications() {
    let mut interpreter = Interpreter::bootstrap();
    let (first, second) = interpreter.enter(|mutator| {
        let a_superclass = mutator.apply_mixin(&bare_mixin("A"), None);
        let b_superclass = mutator.apply_mixin(&bare_mixin("B"), None);
        let mixin = bare_mixin("Inner");
        (
            mutator.apply_mixin(&mixin, Some(&a_superclass)),
            mutator.apply_mixin(&mixin, Some(&b_superclass)),
        )
    });
    assert!(!Rc::ptr_eq(&first, &second));
}

#[test]
fn inherited_slots_are_counted_before_own_slots() {
    // A superclass application with two slots (a, b), a subclass mixin with one slot (c).
    // The subclass's own slots must start past the inherited ones: its
    // `own_slot_base` is what turns a mixin-relative slot index into an
    // absolute field.
    let mut interpreter = Interpreter::bootstrap();
    let mut superclass_mixin = bare_mixin("Sup");
    Rc::get_mut(&mut superclass_mixin).unwrap().slots =
        vec![slot_declaration("a"), slot_declaration("b")];
    let mut subclass_mixin = bare_mixin("Sub");
    Rc::get_mut(&mut subclass_mixin).unwrap().slots = vec![slot_declaration("c")];
    let (superclass_application, subclass_application) = interpreter.enter(|mutator| {
        let superclass_application = mutator.apply_mixin(&superclass_mixin, None);
        let subclass_application =
            mutator.apply_mixin(&subclass_mixin, Some(&superclass_application));
        (superclass_application, subclass_application)
    });

    // Slot count includes inherited and own.
    assert_eq!(
        superclass_application.slot_count, 2,
        "superclass should have 2 slots"
    );
    assert_eq!(
        subclass_application.slot_count, 3,
        "subclass should have 3 slots (2 inherited + 1 own)"
    );

    // Inherited slots keep their declaring level's offsets; own slots land
    // past them.
    assert_eq!(superclass_application.own_slot_base, 0);
    assert_eq!(
        subclass_application.own_slot_base, 2,
        "own slot 'c' must start at index 2 (past the 2 inherited slots)"
    );
}

/// Parses+resolves a one-class module and loads it (via a freshly
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

/// The object `root` holds, for the heap accessors that take one. Every
/// caller is already inside an entry.
fn object<'gc>(mutator: &Mutator<'gc>, root: &Root) -> ns_runtime::ObjectReference<'gc> {
    match mutator.fetch(root) {
        Value::Object(object) => object,
        _ => panic!("expected an object"),
    }
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

#[test]
fn a_class_object_carries_identity_and_its_application_carries_layout() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Point new = Object new (| x. y. |) ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    interpreter.enter(|mutator| {
        let class = object(mutator, &class);
        let application = mutator
            .heap()
            .class_application(class)
            .unwrap_or_else(|| panic!("load_module must produce a class object"));
        assert_eq!(application.slot_count, 2);
        assert!(
            mutator.heap().class_enclosing(class).is_none(),
            "a module's enclosing object is nil"
        );
        // §6.3.2: the slot's getter is a method-table entry, resolved by
        // the ordinary lookup to the field the application lays it out at.
        match dispatch::lookup_protected(mutator.heap(), class, Symbol::intern("y")) {
            Lookup::SlotAccessor(accessor, field) => {
                assert_eq!(accessor.kind, AccessorKind::Getter);
                assert_eq!(
                    field, 1,
                    "layout lives on the application, not on the class object"
                );
            }
            _ => panic!("`y` must find the slot's getter"),
        }
    });
}

const NESTED_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        public class Inner = Object new (| v. |) (
            public value = ( ^v )
        )
        public classSideMarker = ( ^1 )
    ) : (
        public marker = ( ^2 )
    )
"};

#[test]
fn the_mixin_tree_contains_nested_and_class_side_declarations() {
    let (mut interpreter, class, _resolved) = load(NESTED_SOURCE);
    interpreter.enter(|mutator| {
        let application = mutator
            .heap()
            .class_application(object(mutator, &class))
            .unwrap_or_else(|| panic!("expected a class object"));
        let mixin = &application.mixin;
        assert_eq!(mixin.nested.len(), 1, "Inner must be in the mixin tree");
        assert_eq!(mixin.nested[0].name, Symbol::intern("Inner"));
        assert!(
            mixin.class_side.is_some(),
            "a class side is a mixin now, not something to discard"
        );
    });
}

#[test]
fn a_nested_declaration_reserves_a_hidden_cache_slot() {
    let (mut interpreter, class, _resolved) = load(NESTED_SOURCE);
    interpreter.enter(|mutator| {
        let application = mutator
            .heap()
            .class_application(object(mutator, &class))
            .unwrap_or_else(|| panic!("expected a class object"));
        assert_eq!(
            application.slot_count, 1,
            "Outer declares no slots but nests Inner"
        );
        let inner = &application.mixin.nested[0];
        assert_eq!(inner.name, Symbol::intern("Inner"));
        assert_eq!(application.nested_slot[&mixin_key(inner)], 0);
    });
}

#[test]
fn inherited_nested_declarations_keep_their_declaring_levels_slot() {
    // A superclass that declares one slot and nests one class; a subclass
    // that declares one slot of its own. The inherited hidden slot must keep
    // the index its declaring level gave it, and the subclass's own slot must
    // land after it.
    let mut interpreter = Interpreter::bootstrap();
    let mut superclass_mixin = bare_mixin("Sup");
    {
        let superclass_mixin = Rc::get_mut(&mut superclass_mixin).unwrap();
        superclass_mixin.slots = vec![slot_declaration("a")];
        superclass_mixin.nested = vec![bare_mixin("Inner")];
    }
    let superclass_inner = superclass_mixin.nested[0].clone();
    let superclass_application =
        interpreter.enter(|mutator| mutator.apply_mixin(&superclass_mixin, None));
    assert_eq!(superclass_application.own_slot_base, 0);
    assert_eq!(
        superclass_application.nested_slot[&mixin_key(&superclass_inner)],
        1
    );
    assert_eq!(superclass_application.slot_count, 2);

    let mut subclass_mixin = bare_mixin("Sub");
    Rc::get_mut(&mut subclass_mixin).unwrap().slots = vec![slot_declaration("own")];
    let subclass_application = interpreter
        .enter(|mutator| mutator.apply_mixin(&subclass_mixin, Some(&superclass_application)));
    assert_eq!(
        subclass_application.nested_slot[&mixin_key(&superclass_inner)],
        1,
        "an inherited nested declaration keeps its declaring level's slot"
    );
    assert_eq!(
        subclass_application.own_slot_base, 2,
        "the subclass's own slot lands after the inherited hidden slot"
    );
    assert_eq!(subclass_application.slot_count, 3);
}

/// §6.3.2: a slot defines a getter under its name and, when mutable, a
/// setter under the name with `:` appended -- entries of the method table
/// with the slot's own accessibility, beside the methods.
#[test]
fn a_slot_declaration_defines_accessor_entries() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Holder = Object new (| public fixed = 1. private mutable ::= 2. |) (
            public other = ( ^3 )
        )
    "};
    let (mut interpreter, class, _resolved) = load(source);
    interpreter.enter(|mutator| {
        let application = mutator
            .heap()
            .class_application(object(mutator, &class))
            .unwrap_or_else(|| panic!("expected a class object"));
        let mixin = &application.mixin;
        let accessor = |selector: &str| match mixin.methods.get(&Symbol::intern(selector)) {
            Some(ns_runtime::mixin::MethodEntry::SlotAccessor(accessor)) => Some(*accessor),
            _ => None,
        };
        let fixed = accessor("fixed").expect("`fixed` has a getter");
        assert_eq!(
            (fixed.kind, fixed.access, fixed.own_index),
            (AccessorKind::Getter, ast::AccessModifier::Public, 0)
        );
        assert!(
            accessor("fixed:").is_none(),
            "an immutable slot has no setter"
        );
        let setter = accessor("mutable:").expect("`mutable:` is the mutable slot's setter");
        assert_eq!(
            (setter.kind, setter.access, setter.own_index),
            (AccessorKind::Setter, ast::AccessModifier::Private, 1)
        );
        assert!(mixin.compiled_method(Symbol::intern("other")).is_some());
        assert_eq!(mixin.methods.len(), 4, "fixed, mutable, mutable: and other");
    });
}

/// A subclass redeclaring a nested class its superclass already declares.
/// Both declarations are real members -- `N` finds the subclass's, `super N`
/// the superclass's -- so each needs its own cache slot; sharing one would
/// make whichever was reached first the answer to both.
#[test]
fn a_redeclared_nested_class_gets_its_own_cache_slot() {
    let mut interpreter = Interpreter::bootstrap();
    let mut superclass_mixin = bare_mixin("Sup");
    Rc::get_mut(&mut superclass_mixin).unwrap().nested = vec![bare_mixin("Inner")];
    let superclass_inner = superclass_mixin.nested[0].clone();
    let superclass_application =
        interpreter.enter(|mutator| mutator.apply_mixin(&superclass_mixin, None));

    let mut subclass_mixin = bare_mixin("Sub");
    Rc::get_mut(&mut subclass_mixin).unwrap().nested = vec![bare_mixin("Inner")];
    let subclass_inner = subclass_mixin.nested[0].clone();
    let subclass_application = interpreter
        .enter(|mutator| mutator.apply_mixin(&subclass_mixin, Some(&superclass_application)));

    assert_eq!(
        subclass_application.slot_count, 2,
        "one hidden slot per declaration"
    );
    assert_eq!(
        subclass_application.nested_slot[&mixin_key(&superclass_inner)],
        0
    );
    assert_eq!(
        subclass_application.nested_slot[&mixin_key(&subclass_inner)],
        1
    );
}

/// `newspeak-spec.pdf` §3.2: "All metaclasses are instances of class
/// Metaclass, and are direct subclasses of Class. Metaclasses may not be
/// subclassed." A subclass therefore answers a class-side message only if
/// its own class side declares it -- `Derived`'s class side is empty, so
/// `describe` must fall through to the ordinary `doesNotUnderstand:` rather
/// than reach `Base`'s class-side `describe`.
#[test]
fn a_class_side_is_not_inherited() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class FlatMetaclass usingPlatform: platform = Object new () (
            class Base = Object new () (
            ) : (
                public describe = ( ^'base class side' )
            )
            class Derived = Base new () (
            )
            public derivedClass = ( ^Derived )
        )
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate FlatMetaclass",
    );
    let derived = expect_ok(
        interpreter.send(&instance, Symbol::intern("derivedClass"), &[]),
        "derivedClass",
    );
    let message = expect_error(
        interpreter.send(&derived, Symbol::intern("describe"), &[]),
        "Derived describe",
    );
    assert!(
        message.contains("does not understand"),
        "a class side must not be inherited, message was: {message}"
    );
}
