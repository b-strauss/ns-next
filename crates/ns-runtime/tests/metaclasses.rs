//! Metaclasses (`newspeak-spec.pdf` §3.2, §6): every class object's
//! metaclass is its class side applied to `Class`, built together with the
//! class object.

use indoc::indoc;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    BuiltinClass, HostResult, HostSignal, Interpreter, Root, SourceOrigin, Symbol, Value,
};

fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// The class of `subject`, read on the host side, since `class` is
/// answered only to its receiver's own sends.
fn class_of(interpreter: &mut Interpreter, subject: &Root) -> Root {
    interpreter.enter(|mutator| {
        let class = mutator.class_of(mutator.fetch(subject));
        mutator.stash(Value::object(class))
    })
}

/// §3.2 and §6: a class's metaclass is its class side applied to `Class`,
/// so a class-side message it does not declare falls to `Class`, and a
/// metaclass links back to the class it belongs to.
#[test]
fn a_metaclass_is_the_class_side_applied_to_class() {
    let source = indoc! {"
        class Subject = Object new () (
        ) : (
          public make = ( ^1 )
        )
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let Some(class_object) = mutator.fetch(&class).as_object() else {
            panic!("a loaded module is a class object");
        };
        // Read off the class object itself, not through `metaclass_of`: the
        // metaclass is built with the class object, so a creation path that
        // skipped it fails here.
        let metaclass = mutator
            .heap()
            .class_metaclass(class_object)
            .expect("every class object has a metaclass");
        let class_class = mutator.builtin_class(BuiltinClass::Class);
        let heap = mutator.heap();
        assert!(heap
            .class_this_class(metaclass)
            .is_some_and(|this_class| this_class.same(class_object)));
        assert!(heap
            .class_superclass(metaclass)
            .is_some_and(|superclass| superclass.same(class_class)));
        let metaclass_application = heap.class_application(metaclass).expect("an application");
        let class_application = heap.class_application(class_class).expect("an application");
        assert!(metaclass_application
            .superclass
            .as_ref()
            .is_some_and(|superclass| Rc::ptr_eq(superclass, &class_application)));
    });
}

/// §3.2: `Foo class` is Foo's metaclass, whose class is `Metaclass`, and
/// `Metaclass class class` is `Metaclass` again.
#[test]
fn the_class_of_a_class_is_its_metaclass() {
    let source = indoc! {"
        class Subject = Object new () (
        ) : (
          public make = ( ^1 )
        )
    "};
    let (mut interpreter, class) = load(source);
    let metaclass = class_of(&mut interpreter, &class);
    let metaclass_class = class_of(&mut interpreter, &metaclass);
    let metaclass_metaclass = class_of(&mut interpreter, &metaclass_class);
    let metaclass_metaclass_class = class_of(&mut interpreter, &metaclass_metaclass);
    interpreter.enter(|mutator| {
        let Some(class_object) = mutator.fetch(&class).as_object() else {
            panic!("a loaded module is a class object");
        };
        let expected = mutator
            .metaclass_of(class_object)
            .expect("every class object has a metaclass");
        let Some(answered) = mutator.fetch(&metaclass).as_object() else {
            panic!("`Subject class` answers an object");
        };
        assert!(answered.same(expected), "`Subject class` is its metaclass");
        let builtin_metaclass = mutator.builtin_class(BuiltinClass::Metaclass);
        let Some(answered) = mutator.fetch(&metaclass_class).as_object() else {
            panic!("`Subject class class` answers an object");
        };
        assert!(
            answered.same(builtin_metaclass),
            "a metaclass is an instance of `Metaclass`"
        );
        let expected = mutator
            .metaclass_of(builtin_metaclass)
            .expect("`Metaclass` has a metaclass");
        let Some(answered) = mutator.fetch(&metaclass_metaclass).as_object() else {
            panic!("`Metaclass class` answers an object");
        };
        assert!(
            answered.same(expected),
            "`Metaclass class` is its metaclass"
        );
        let Some(answered) = mutator.fetch(&metaclass_metaclass_class).as_object() else {
            panic!("`Metaclass class class` answers an object");
        };
        assert!(
            answered.same(builtin_metaclass),
            "`Metaclass class class` is `Metaclass` again"
        );
    });
}

/// Class objects of one declaration under two enclosing instances have one
/// dispatch id: their metaclasses share an application.
#[test]
fn class_objects_of_one_declaration_share_a_dispatch_id() {
    let source = indoc! {"
        class Outer usingPlatform: platform = Object new () (
            public class Widget = Object new () (
            )
        )
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let first_outer = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate the first Outer",
    );
    let second_outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate the second Outer",
    );
    let widget = Symbol::intern("Widget");
    let first_widget = expect_ok(interpreter.send(&first_outer, widget, &[]), "Widget");
    let second_widget = expect_ok(interpreter.send(&second_outer, widget, &[]), "Widget");
    interpreter.enter(|mutator| {
        let (Some(first), Some(second)) = (
            mutator.fetch(&first_widget).as_object(),
            mutator.fetch(&second_widget).as_object(),
        ) else {
            panic!("Widget answers a class object");
        };
        assert!(
            !first.same(second),
            "one class object per enclosing instance"
        );
        let heap = mutator.heap();
        assert_eq!(
            heap.dispatch_id(Value::object(first)),
            heap.dispatch_id(Value::object(second))
        );
    });
}
