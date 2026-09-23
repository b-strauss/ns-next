//! The implicit nested-class accessor (`newspeak-spec.pdf` §6): a nested
//! declaration `N` inside `OC` defines a method `N` on `OC` that lazily
//! evaluates the declaration with the receiver as enclosing object and
//! caches the resulting class object per receiver.

use indoc::indoc;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{Heap, HostResult, HostSignal, Interpreter, Root, SourceOrigin, Symbol, Value};

fn load(source: &str) -> (Interpreter, Root) {
    load_with(source, ns_runtime::Lowering::Optimized)
}

fn load_with(source: &str, lowering: ns_runtime::Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

/// `HostSignal` carries no `Debug` impl, so `Result::expect` isn't usable on
/// a `HostResult` -- unwrap by hand, naming which variant leaked.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// Reads a `Value` known to be a `String` object. `Value` has no `PartialEq`,
/// and its `Debug` -- though not recursive -- prints only Rust internals
/// (`ObjectReference(#42)`, not the string's contents), so a class-side send's
/// result is compared through this rather than `assert_eq!` on the `Value`
/// itself.
fn string_contents<'gc>(heap: &Heap<'gc>, value: &Value<'gc>) -> String {
    match value {
        Value::Object(object) => heap
            .str_value(*object)
            .unwrap_or_else(|| panic!("expected ObjectData::String")),
        _ => panic!("expected Value::Object"),
    }
}

fn expect_error(result: HostResult, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}: expected a Newspeak error"),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => error.message,
    }
}

const SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        public class Inner = Object new (| v. |) (
            public value = ( ^42 )
        )
        public makeInner = ( ^Inner new )
        public innerClass = ( ^Inner )
    )
"};

#[test]
fn the_accessor_returns_the_same_class_object_every_time() {
    let (mut interpreter, class) = load(SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let first = expect_ok(
        interpreter.send(&outer, Symbol::intern("innerClass"), &[]),
        "first Inner",
    );
    let second = expect_ok(
        interpreter.send(&outer, Symbol::intern("innerClass"), &[]),
        "second Inner",
    );
    interpreter.enter(|mutator| {
        let (Value::Object(first_object), Value::Object(second_object)) =
            (mutator.fetch(&first), mutator.fetch(&second))
        else {
            panic!("a nested class access must yield a class object");
        };
        assert!(first_object.same(second_object), "spec 6: cached within r");
    });
}

#[test]
fn two_enclosing_instances_get_distinct_nested_classes() {
    let (mut interpreter, class) = load(SOURCE);
    let platform = interpreter.platform();
    let one = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate first Outer",
    );
    let two = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate second Outer",
    );
    let first_inner = expect_ok(
        interpreter.send(&one, Symbol::intern("innerClass"), &[]),
        "first",
    );
    let second_inner = expect_ok(
        interpreter.send(&two, Symbol::intern("innerClass"), &[]),
        "second",
    );
    interpreter.enter(|mutator| {
        let (Value::Object(first_inner), Value::Object(second_inner)) =
            (mutator.fetch(&first_inner), mutator.fetch(&second_inner))
        else {
            panic!("class objects")
        };
        assert!(
            !first_inner.same(second_inner),
            "spec 6: a distinct class per instance"
        );
        let (Some(a_application), Some(b_application)) = (
            mutator.heap().class_application(first_inner),
            mutator.heap().class_application(second_inner),
        ) else {
            panic!("class data")
        };
        assert!(
            Rc::ptr_eq(&a_application, &b_application),
            "spec 6.2: applications of one mixin share code and structure -- this \
             is what keeps a call site monomorphic across enclosing instances"
        );
    });
}

#[test]
fn the_nested_class_remembers_its_enclosing_object() {
    let (mut interpreter, class) = load(SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let inner = expect_ok(
        interpreter.send(&outer, Symbol::intern("innerClass"), &[]),
        "Inner",
    );
    interpreter.enter(|mutator| {
        let (Value::Object(inner), Value::Object(outer)) =
            (mutator.fetch(&inner), mutator.fetch(&outer))
        else {
            panic!("class object and instance")
        };
        let enclosing = mutator
            .heap()
            .class_enclosing(inner)
            .expect("spec 6: a nested class's one piece of state is its enclosing object");
        assert!(enclosing.same(outer));
    });
}

#[test]
fn a_nested_class_can_be_instantiated_through_its_accessor() {
    let (mut interpreter, class) = load(SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let instance = expect_ok(
        interpreter.send(&outer, Symbol::intern("makeInner"), &[]),
        "makeInner",
    );
    let value = expect_ok(
        interpreter.send(&instance, Symbol::intern("value"), &[]),
        "value",
    );
    let value = interpreter.enter(|mutator| match mutator.fetch(&value) {
        Value::SmallInteger(value) => value,
        _ => panic!("expected a SmallInteger from `Inner new value`"),
    });
    assert_eq!(value, 42);
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if ec does not evaluate
/// to a class object C." A program error, not an interpreter invariant --
/// a `NewspeakError`, never a panic.
const NON_CLASS_SUPERCLASS_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new (| notAClass = 7. |) (
        public class Inner = notAClass new () ()
        public innerClass = ( ^Inner )
    )
"};

#[test]
fn a_superclass_clause_that_is_not_a_class_is_a_program_error() {
    let (mut interpreter, class) = load(NON_CLASS_SUPERCLASS_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let message = expect_error(
        interpreter.send(&outer, Symbol::intern("innerClass"), &[]),
        "innerClass",
    );
    assert!(
        message.contains("did not evaluate to a class object"),
        "unexpected message: {message}"
    );
}

/// §6: the implicit accessor has the declaration's own accessibility. The
/// default (no modifier) is `protected`, so an ordinary explicit-receiver
/// send must not reach it -- and `lookupPublic`'s hard-stop rule means the
/// search stops at the declaration rather than continuing up the chain.
const PROTECTED_NESTED_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        class Hidden = Object new () (
            public value = ( ^7 )
        )
        public reach = ( ^Hidden new value )
    )
"};

#[test]
fn a_nested_class_defaults_to_protected_accessibility() {
    let (mut interpreter, class) = load(PROTECTED_NESTED_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    // Lexically, from inside Outer: reachable, and it really is a class.
    let value = expect_ok(
        interpreter.send(&outer, Symbol::intern("reach"), &[]),
        "reach",
    );
    let value = interpreter.enter(|mutator| match mutator.fetch(&value) {
        Value::SmallInteger(value) => value,
        _ => panic!("expected a SmallInteger from `Hidden new value`"),
    });
    assert_eq!(value, 7);
    // From outside, as an ordinary send: not reachable.
    let message = expect_error(
        interpreter.send(&outer, Symbol::intern("Hidden"), &[]),
        "Hidden",
    );
    assert!(
        message.contains("does not understand"),
        "unexpected message: {message}"
    );
}

/// An *inherited* nested declaration: `Derived`'s superclass clause names
/// `Base`, so the accessor for `N` is reached one level up the chain. The
/// class object `dispatch` reports as the declaring level is then `Base`'s,
/// not the receiver's own (dynamic) class -- which is what the clause frame's
/// `current_class` must be, since the clause is lexically part of `Base`'s
/// declaration.
const INHERITED_NESTED_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class M usingPlatform: p = Object new () (
        class Base = Object new () (
            public class N = Object new () (
                public tag = ( ^11 )
            )
        )
        public class Derived = Base new () ()
        public makeDerived = ( ^Derived new )
    )
"};

#[test]
fn an_inherited_nested_declaration_is_reached_through_the_chain() {
    let (mut interpreter, class) = load(INHERITED_NESTED_SOURCE);
    let platform = interpreter.platform();
    let m_instance = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate M",
    );
    let derived = expect_ok(
        interpreter.send(&m_instance, Symbol::intern("makeDerived"), &[]),
        "makeDerived",
    );
    // `N` is declared on Base, reached here through Derived's chain -- and
    // its hidden cache slot is the inherited entry in Derived's layout.
    let n_class = expect_ok(interpreter.send(&derived, Symbol::intern("N"), &[]), "N");
    interpreter.enter(|mutator| {
        let (Value::Object(n_class_object), Value::Object(derived_object)) =
            (mutator.fetch(&n_class), mutator.fetch(&derived))
        else {
            panic!("class object and instance")
        };
        let enclosing = mutator
            .heap()
            .class_enclosing(n_class_object)
            .expect("an enclosing object");
        assert!(
            enclosing.same(derived_object),
            "the enclosing object is the receiver, not the declaring level"
        );
    });
    // Still cached per receiver across the inherited path. Both class
    // objects are read after the run that answered the second.
    let again = expect_ok(
        interpreter.send(&derived, Symbol::intern("N"), &[]),
        "N again",
    );
    interpreter.enter(|mutator| {
        let (Value::Object(n_class_object), Value::Object(again)) =
            (mutator.fetch(&n_class), mutator.fetch(&again))
        else {
            panic!("class object")
        };
        assert!(n_class_object.same(again), "spec 6: cached within r");
    });
    let instance = expect_ok(interpreter.instantiate(&n_class, &[]), "instantiate N");
    let tag = expect_ok(
        interpreter.send(&instance, Symbol::intern("tag"), &[]),
        "tag",
    );
    let tag = interpreter.enter(|mutator| match mutator.fetch(&tag) {
        Value::SmallInteger(tag) => tag,
        _ => panic!("expected a SmallInteger from `N new tag`"),
    });
    assert_eq!(tag, 11);
}

/// A `super`-prefixed clause receiver is the one form whose evaluation reads
/// the clause frame's `current_class` (`resolver::resolve_superclass_clause`
/// binds it as `Binding::SuperSend`). The lowerer treats a `SuperSend`-bound
/// *identifier* as the bare-builtin fallback rather than a real super send,
/// so today `super Object` names the builtin `Object` and the accessor
/// answers a class object without dispatching -- which is the outcome this
/// pins. It is the regression guard for the day that form starts
/// dispatching: by then the frame already carries the declaring class.
const SUPER_PREFIXED_CLAUSE_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        public class Inner = super Object ()
        public innerClass = ( ^Inner )
    )
"};

#[test]
fn a_super_prefixed_clause_answers_a_class_object() {
    let (mut interpreter, class) = load(SUPER_PREFIXED_CLAUSE_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let inner = expect_ok(
        interpreter.send(&outer, Symbol::intern("innerClass"), &[]),
        "innerClass",
    );
    interpreter.enter(|mutator| {
        let Value::Object(inner) = mutator.fetch(&inner) else {
            panic!("the accessor must yield a class object")
        };
        assert!(mutator.heap().is_class(inner));
    });
}

/// A class whose creation requires itself. §6 evaluates the declaration on
/// first access, so `Inner`'s clause re-enters `Inner`'s own accessor before
/// the cache slot is filled -- unbounded recursion, and an abort rather than
/// a diagnosable error, without the in-progress guard.
const CYCLIC_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        public class Inner = Inner new ()
        public innerClass = ( ^Inner )
    )
"};

#[test]
fn a_self_referential_nested_class_is_a_program_error() {
    let (mut interpreter, class) = load(CYCLIC_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let message = expect_error(
        interpreter.send(&outer, Symbol::intern("innerClass"), &[]),
        "innerClass",
    );
    assert_eq!(
        message,
        "cyclic class definition: evaluating the superclass clause of class Inner \
         requires class Inner itself"
    );
    // The guard is scoped to the evaluation, not latched: a second access
    // must raise the same error rather than a different one (or succeed).
    let again = expect_error(
        interpreter.send(&outer, Symbol::intern("innerClass"), &[]),
        "innerClass again",
    );
    assert_eq!(message, again);
}

/// A subclass redeclaring a nested class in terms of the one it inherits is
/// not a cycle: the two declarations cache in separate slots
/// (`MixinApplication::nested_slot`), so creating the subclass's `Inner` --
/// whose clause creates the inherited one on the same receiver, through
/// `super Inner` -- finds the inherited one's slot empty, not in progress.
/// The clause reaches it through a method because a `super`-prefixed clause
/// receiver does not dispatch yet (`a_super_prefixed_clause_answers_a_class_object`).
#[test]
fn a_nested_class_built_on_the_one_it_redeclares_is_not_a_cycle() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Outer = Object new (
        ) (
            public class Base = Object new (
            ) (
                public class Inner = Object new (
                ) (
                    public tag = ( ^'base' )
                ) : ()
            ) : ()
            public class Sub = Base new (
            ) (
                public class Inner = self inherited (
                ) (
                    public own = ( ^'sub' )
                ) : ()
                inherited = ( ^super Inner )
            ) : ()
            public both = ( | i | i:: Sub new Inner new. ^i tag, i own )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let outer = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Outer");
    let both = expect_ok(
        interpreter.send(&outer, Symbol::intern("both"), &[]),
        "both",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&both)),
            "basesub"
        );
    });
}

const CLASS_SIDE_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Widget new = Object new () (
    ) : (
        public describe = ( ^'a widget class' )
    )
"};

/// §6.3: a class object's class-side methods are reachable by sending to
/// the class object itself, through a lazily created metaclass.
#[test]
fn a_class_side_method_dispatches_on_the_class_object() {
    let (mut interpreter, class) = load(CLASS_SIDE_SOURCE);
    let described = expect_ok(
        interpreter.send(&class, Symbol::intern("describe"), &[]),
        "class-side send",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&described)),
            "a widget class"
        );
    });
}

/// A class with no class side at all must still fall through to the
/// ordinary `doesNotUnderstand:` -- `metaclass_of` returns `None` and the
/// send path must not panic or invent a different error shape for a class
/// object than for any other receiver.
#[test]
fn a_class_with_no_class_side_still_reports_ordinary_does_not_understand() {
    let (mut interpreter, class) = load(SOURCE);
    let message = expect_error(
        interpreter.send(&class, Symbol::intern("noSuchClassSideMethod"), &[]),
        "class-side miss",
    );
    assert!(message.contains("does not understand #noSuchClassSideMethod"));
}

const CLASS_SIDE_INHERITANCE_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        public class Widget new = Object new () (
        ) : (
            public describe = ( ^'a widget class' )
        )
        public class SpecialWidget new = Widget new () (
        )
        public class OverridingWidget new = Widget new () (
        ) : (
            public describe = ( ^'a special widget class' )
        )
    )
"};

/// `newspeak-spec.pdf` §3.2: metaclasses are flat, so a class side is not
/// inherited down the subclass chain the way any other mixin member is.
/// `SpecialWidget` declares no class side of its own; its superclass
/// `Widget` does, but that class side answers only on `Widget` itself --
/// `SpecialWidget describe` falls straight through to the ordinary
/// `doesNotUnderstand:`.
#[test]
fn a_subclass_with_no_class_side_of_its_own_does_not_inherit_one() {
    let (mut interpreter, class) = load(CLASS_SIDE_INHERITANCE_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let special_widget = expect_ok(
        interpreter.send(&outer, Symbol::intern("SpecialWidget"), &[]),
        "SpecialWidget accessor",
    );
    let message = expect_error(
        interpreter.send(&special_widget, Symbol::intern("describe"), &[]),
        "SpecialWidget describe",
    );
    assert!(
        message.contains("does not understand"),
        "a class side must not be inherited, message was: {message}"
    );
}

/// The class object a nested declaration's accessor answers for `enclosing`.
fn nested_class_object(interpreter: &mut Interpreter, enclosing: &Root, name: &str) -> Root {
    let class = expect_ok(interpreter.send(enclosing, Symbol::intern(name), &[]), name);
    interpreter.enter(|mutator| {
        let Value::Object(_) = mutator.fetch(&class) else {
            panic!("{name} answers a class object")
        };
    });
    class
}

fn application_of<'gc>(
    heap: &ns_runtime::Heap<'gc>,
    class: ns_runtime::ClassReference<'gc>,
) -> Rc<ns_runtime::MixinApplication> {
    heap.class_application(class).expect("a class object")
}

/// §6.2 on the metaclass side: two class objects that share one
/// instance-side application must share one *metaclass* application too, or
/// a call site that went monomorphic on the application would split as soon
/// as a second enclosing instance appeared.
///
/// `Widget` is nested inside `Outer` and reached from two different `Outer`
/// instances: §6's introduction gives each enclosing instance its own
/// `Widget` class object, but both apply the same declaration, so
/// `apply_mixin`'s (mixin
/// identity, superclass) cache must answer the same metaclass application
/// for both -- flat metaclasses key that application on the mixin alone
/// (`Mutator::metaclass_of` applies it with no superclass application),
/// which is what keeps this from splitting a second time.
#[test]
fn two_class_objects_sharing_an_application_share_their_class_sides_application() {
    let (mut interpreter, class) = load(CLASS_SIDE_INHERITANCE_SOURCE);
    let platform = interpreter.platform();
    let one = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate first Outer",
    );
    let two = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate second Outer",
    );
    let first_widget = nested_class_object(&mut interpreter, &one, "Widget");
    let second_widget = nested_class_object(&mut interpreter, &two, "Widget");
    // Both are read after the last run; everything below -- reading the
    // class objects, building their metaclasses, and comparing applications
    // -- runs nothing, so it all happens in one entry.
    interpreter.enter(|mutator| {
        let (Value::Object(first_widget), Value::Object(second_widget)) =
            (mutator.fetch(&first_widget), mutator.fetch(&second_widget))
        else {
            panic!("Widget answers a class object")
        };
        assert!(
            !first_widget.same(second_widget),
            "spec 6: one class object per enclosing instance"
        );
        assert!(
            Rc::ptr_eq(
                &application_of(mutator.heap(), first_widget),
                &application_of(mutator.heap(), second_widget)
            ),
            "spec 6.2: both share one instance-side application"
        );
        let first_metaclass = mutator
            .metaclass_of(first_widget)
            .expect("Widget declares its own class side");
        let second_metaclass = mutator
            .metaclass_of(second_widget)
            .expect("Widget declares its own class side");
        assert!(
            Rc::ptr_eq(
                &application_of(mutator.heap(), first_metaclass),
                &application_of(mutator.heap(), second_metaclass)
            ),
            "spec 6.2: a class side's application is shared across enclosing instances too"
        );
    });
}

/// A class that declares its own class side answers from it, regardless of
/// what its superclass's class side declares under the same name -- flat
/// metaclasses mean there is no inherited method here to fall back to in
/// the first place, only `OverridingWidget`'s own.
#[test]
fn a_class_sides_own_method_is_found_before_falling_through_to_object() {
    let (mut interpreter, class) = load(CLASS_SIDE_INHERITANCE_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let overriding_widget = expect_ok(
        interpreter.send(&outer, Symbol::intern("OverridingWidget"), &[]),
        "OverridingWidget accessor",
    );
    let described = expect_ok(
        interpreter.send(&overriding_widget, Symbol::intern("describe"), &[]),
        "overriding class-side send",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&described)),
            "a special widget class"
        );
    });
}

/// `Frame.current_class` must be the class object of the level
/// that *defines* the running method, not the receiver's own (dynamic)
/// class -- otherwise `super`/`outer` inside an inherited method read the
/// wrong level. `Derived` inherits nothing here (it overrides `describe`
/// itself), but its `super describe` lands in `Base`'s activation, and
/// `Base`'s own `outer Outer label` must resolve from *that* activation.
/// Mirrors `goldens/nesting/nested_super.ns` at the API level.
const NESTED_SUPER_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        label = ( ^'outer' )
        public class Base = Object new () (
            public describe = ( ^outer Outer label )
        )
        public class Derived = Base new () (
            public describe = ( ^(super describe), '/derived' )
        )
        public run = ( ^Derived new describe )
    )
"};

#[test]
fn super_and_outer_both_resolve_through_the_defining_level() {
    let (mut interpreter, class) = load(NESTED_SUPER_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let described = expect_ok(interpreter.send(&outer, Symbol::intern("run"), &[]), "run");
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&described)),
            "outer/derived"
        );
    });
}

/// A *three*-level hierarchy where the middle level (`Mid`) is reached only
/// through inheritance -- `Leaf` declares no `describe` of its own. This is
/// the shape a two-level fixture cannot exercise: if
/// `Mutator::push_step` set `current_class` to the receiver's own
/// (dynamic) class rather than the level `dispatch` found the method at, `Mid`'s
/// activation would compute its `super` target from *`Leaf`'s* superclass
/// (`Mid` itself) forever -- unbounded recursion, not a wrong value, so a
/// two-level chain (where the bug and the fix agree on the first hop) would
/// pass either way. This guards the actual defect class the level-tracking
/// machinery exists for.
const SUPER_CHAIN_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        public class Grand = Object new () (
            public describe = ( ^'grand' )
        )
        public class Mid = Grand new () (
            public describe = ( ^(super describe), '/mid' )
        )
        public class Leaf = Mid new () ()
        public run = ( ^Leaf new describe )
    )
"};

#[test]
fn a_super_chain_progresses_through_an_inherited_middle_level() {
    let (mut interpreter, class) = load(SUPER_CHAIN_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let described = expect_ok(interpreter.send(&outer, Symbol::intern("run"), &[]), "run");
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&described)),
            "grand/mid"
        );
    });
}

/// §3.3: an activation's enclosing objects belong to the level that
/// *defines* the running method. `Mid` and `Base` are both nested directly
/// in the module class, so `Derived` (nested in `Mid`) sits one level
/// deeper than `Base`, from which it inherits `describe`. `describe`'s
/// `outer Outer label` is a lexical depth of 1 fixed inside `Base`, so it
/// must reach the module instance and answer `'top'` on any receiver --
/// resolving the depth from the receiver's own class instead reaches the
/// `Mid` instance, whose decoy `label` answers `'middle'`.
const OUTER_THROUGH_INHERITED_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        label = ( ^'top' )
        public class Base = Object new () (
            public describe = ( ^outer Outer label )
        )
        public class Mid = Object new () (
            label = ( ^'mid' )
            public class Derived = Base new () ()
        )
        public run = ( ^Mid new Derived new describe )
    )
"};

#[test]
fn outer_in_an_inherited_method_resolves_from_the_defining_level() {
    let (mut interpreter, class) = load(OUTER_THROUGH_INHERITED_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let described = expect_ok(interpreter.send(&outer, Symbol::intern("run"), &[]), "run");
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&described)),
            "top"
        );
    });
}

/// The same defect one hop further out, which only a lexical *declaration*
/// chain catches. `Deep`'s `outer Outer` is a depth of 2, so the walk
/// leaves `Deep`'s enclosing object -- a `Derived` instance -- and must
/// then leave the level that declares `Deep`, which is `Base`, not
/// `Derived`'s own class. Stepping through the reached object's class
/// instead lands on `Mid`'s instance and its decoy `label`.
const DEEP_OUTER_THROUGH_INHERITED_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        label = ( ^'top' )
        public class Base = Object new () (
            public class Deep = Object new () (
                public describe = ( ^outer Outer label )
            )
        )
        public class Mid = Object new () (
            label = ( ^'mid' )
            public class Derived = Base new () ()
        )
        public run = ( ^Mid new Derived new Deep new describe )
    )
"};

#[test]
fn a_two_level_outer_through_an_inherited_nested_class_resolves_lexically() {
    let (mut interpreter, class) = load(DEEP_OUTER_THROUGH_INHERITED_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let described = expect_ok(interpreter.send(&outer, Symbol::intern("run"), &[]), "run");
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&described)),
            "top"
        );
    });
}

/// §6.3: a class side is written inside the same class declaration its
/// instance side is, and `resolver::resolve_class` resolves both against
/// one lexical chain -- so `outer` in a class-side method reaches exactly
/// what it reaches on the instance side. `Widget`'s class side is at
/// depth 1 from the module class; `Part`'s, nested one level further, is
/// at depth 2 and can only be walked through the declaration chain.
/// `Widget`'s decoy `label` is what a walk that stopped one level short
/// would answer with.
const CLASS_SIDE_OUTER_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Outer usingPlatform: p = Object new () (
        label = ( ^'top' )
        public class Widget = Object new () (
            label = ( ^'widget' )
            public class Part = Object new () (
            ) : (
                public whichOuter = ( ^outer Outer label )
            )
        ) : (
            public whichOuter = ( ^outer Outer label )
        )
        public shallow = ( ^Widget whichOuter )
        public deep = ( ^Widget new Part whichOuter )
    )
"};

#[test]
fn outer_in_a_class_side_method_reaches_the_enclosing_object() {
    let (mut interpreter, class) = load(CLASS_SIDE_OUTER_SOURCE);
    let platform = interpreter.platform();
    let outer = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "instantiate Outer",
    );
    let shallow = expect_ok(
        interpreter.send(&outer, Symbol::intern("shallow"), &[]),
        "shallow",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&shallow)),
            "top"
        );
    });
    let deep = expect_ok(
        interpreter.send(&outer, Symbol::intern("deep"), &[]),
        "deep",
    );
    interpreter.enter(|mutator| {
        assert_eq!(
            string_contents(mutator.heap(), &mutator.fetch(&deep)),
            "top"
        );
    });
}

/// A superclass clause that fails leaves no creation in progress behind: the
/// next access evaluates the clause again, rather than reporting the class as
/// depending on itself.
#[test]
fn a_failed_superclass_clause_is_retried_rather_than_reported_as_a_cycle() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Outer = Object new (
          | first ::= true. |
        ) (
          public class Inner = self pick (
          ) (
          ) : ()
          pick = ( first ifTrue: [first:: false. ^nil foo]. ^Object )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let outer = expect_ok(interpreter.instantiate(&class, &[]), "instantiate Outer");
    match interpreter.send(&outer, Symbol::intern("Inner"), &[]) {
        Err(HostSignal::Error(error)) => assert!(
            !error.message.contains("cyclic"),
            "the first access fails in its clause: {}",
            error.message
        ),
        Err(HostSignal::Return) => panic!("a non-local return escaped"),
        Ok(_) => panic!("the first access must fail"),
    }
    let inner = expect_ok(
        interpreter.send(&outer, Symbol::intern("Inner"), &[]),
        "the second access",
    );
    interpreter.enter(|mutator| {
        let Value::Object(inner) = mutator.fetch(&inner) else {
            panic!("a class object");
        };
        assert!(mutator.heap().is_class(inner));
    });
}

/// `Marker` and `Applied` are declared at different depths so that their
/// enclosing objects differ: `Marker`'s is a `Library`, `Applied`'s an
/// `Inner`, and each declares its own `mark`. `Marker`'s methods reach two
/// levels out -- `mark` one hop, `outer Host label` two -- so the level must
/// carry both `Marker`'s enclosing object and its declaring class, the second
/// hop's starting point. Each enclosing level answers its own `label`, so
/// landing on the wrong one shows.
const APPLIED_ENCLOSING_SOURCE: &str = indoc! {"
    Newspeak3
    'Test'
    class Host usingPlatform: platform = Object new () (
        public class Library = Object new (
            | private mark = 'library'. |
        ) (
            public class Marker = Object new () (
                public markOf: ignored = ( ^mark )
                public hostLabel = ( ^outer Host label )
            )
            public class Inner = Object new (
                | private mark = 'inner'. |
            ) (
                public class Applied = Object new <: Marker () (
                )
                public label = ( ^'inner' )
            )
            public label = ( ^'library' )
        )
        public label = ( ^'host' )
        public markOfApplied = ( ^Library new Inner new Applied new markOf: nil )
        public hostLabelOfApplied = ( ^Library new Inner new Applied new hostLabel )
        public appliedClass = ( ^Library new Inner new Applied )
    )
"};

/// §6.2: "The enclosing object of *I* is the enclosing object of *C*." A
/// method of the applied mixin running on an instance of the applying class
/// resolves `outer` from where `Marker` is declared, not from where `Applied`
/// is: taking `Applied`'s enclosing object would answer `'inner'`, and
/// taking its declaring class would stop the second hop at a `Library`.
#[test]
fn an_applied_level_encloses_where_the_applied_class_is_declared() {
    for lowering in [
        ns_runtime::Lowering::Optimized,
        ns_runtime::Lowering::Reference,
    ] {
        let module = ns_syntax::parse(APPLIED_ENCLOSING_SOURCE).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            Rc::from(APPLIED_ENCLOSING_SOURCE),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let platform = interpreter.platform();
        let host = expect_ok(
            interpreter.instantiate(&class, &[platform]),
            "instantiate Host",
        );
        let mark = expect_ok(
            interpreter.send(&host, Symbol::intern("markOfApplied"), &[]),
            "markOfApplied",
        );
        let host_label = expect_ok(
            interpreter.send(&host, Symbol::intern("hostLabelOfApplied"), &[]),
            "hostLabelOfApplied",
        );
        interpreter.enter(|mutator| {
            assert_eq!(
                string_contents(mutator.heap(), &mutator.fetch(&mark)),
                "library"
            );
            assert_eq!(
                string_contents(mutator.heap(), &mutator.fetch(&host_label)),
                "host",
                "outer Host from Marker's method reaches the Host"
            );
        });
    }
}

/// §6.2 builds a `<:` clause from §6.1's `inheritanceClause`, so §6.1's run
/// time error holds for it too: a receiver that is no class object is an
/// ordinary Newspeak error, and it names the clause.
#[test]
fn a_mixin_clause_that_is_not_a_class_is_a_program_error() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Outer usingPlatform: p = Object new (| notAClass = 7. |) (
            public class Applied = Object new <: notAClass () ()
            public appliedClass = ( ^Applied )
        )
    "};
    for lowering in [
        ns_runtime::Lowering::Optimized,
        ns_runtime::Lowering::Reference,
    ] {
        let (mut interpreter, class) = load_with(source, lowering);
        let platform = interpreter.platform();
        let outer = expect_ok(
            interpreter.instantiate(&class, &[platform]),
            "instantiate Outer",
        );
        let message = expect_error(
            interpreter.send(&outer, Symbol::intern("appliedClass"), &[]),
            "appliedClass",
        );
        assert!(
            message.contains("mixin application clause 1 of class Applied"),
            "{lowering:?}: message was: {message}"
        );
    }
}

/// CLAUDE.md's "the application cache is keyed by the superclass's
/// *application*": an applied level's class object is per enclosing
/// instance, like any nested class's, but its behaviour is not. Two `Host`s
/// give two `Library`s, so two `Marker` class objects and two levels -- and
/// one `MixinApplication` for both levels, and one for both levels'
/// declaring classes. Keyed by class object instead, every instance would
/// mint its own and no call site through a level would ever be monomorphic.
#[test]
fn applied_levels_of_two_enclosing_instances_share_their_applications() {
    for lowering in [
        ns_runtime::Lowering::Optimized,
        ns_runtime::Lowering::Reference,
    ] {
        let (mut interpreter, class) = load_with(APPLIED_ENCLOSING_SOURCE, lowering);
        let applied_classes: Vec<Root> = (0..2)
            .map(|_| {
                let platform = interpreter.platform();
                let host = expect_ok(
                    interpreter.instantiate(&class, &[platform]),
                    "instantiate Host",
                );
                expect_ok(
                    interpreter.send(&host, Symbol::intern("appliedClass"), &[]),
                    "appliedClass",
                )
            })
            .collect();
        interpreter.enter(|mutator| {
            let heap = mutator.heap();
            let level_of = |root: &Root| {
                let Value::Object(applied) = mutator.fetch(root) else {
                    panic!("appliedClass answers a class object")
                };
                heap.class_superclass(applied)
                    .expect("Applied has a superclass")
            };
            let (first, second) = (level_of(&applied_classes[0]), level_of(&applied_classes[1]));
            assert!(
                !first.same(second),
                "{lowering:?}: each enclosing instance has a level of its own"
            );
            let application_of = |level| heap.class_application(level).expect("a class object");
            assert!(
                Rc::ptr_eq(&application_of(first), &application_of(second)),
                "{lowering:?}: both levels share one application"
            );
            let declaring_of = |level| {
                heap.class_declaring(level)
                    .expect("Marker's declaring class")
            };
            let (first_declaring, second_declaring) = (declaring_of(first), declaring_of(second));
            assert!(
                !first_declaring.same(second_declaring),
                "{lowering:?}: each level's declaring class is its own Library's"
            );
            assert!(
                Rc::ptr_eq(
                    &application_of(first_declaring),
                    &application_of(second_declaring)
                ),
                "{lowering:?}: both declaring classes share one application"
            );
        });
    }
}
