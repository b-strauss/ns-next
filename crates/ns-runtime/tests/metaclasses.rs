//! Metaclasses (`newspeak-spec.pdf` §3.2, §6): every class object's
//! metaclass is its class side applied to `Class`, built together with the
//! class object.

use indoc::indoc;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{BuiltinClass, Interpreter, Root, SourceOrigin, Value};

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

/// §3.2 and §6: a class's metaclass is its class side applied to `Class`,
/// so a class-side message it does not declare falls to `Class`, and a
/// metaclass links back to the class it belongs to.
#[test]
fn a_metaclass_is_the_class_side_applied_to_class() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Subject = Object new () (
        ) : (
          public make = ( ^1 )
        )
    "};
    let (mut interpreter, class) = load(source);
    interpreter.enter(|mutator| {
        let Value::Object(class_object) = mutator.fetch(&class) else {
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
