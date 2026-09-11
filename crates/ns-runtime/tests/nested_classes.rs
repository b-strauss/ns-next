//! The implicit nested-class accessor (`newspeak-spec.pdf` §6): a nested
//! declaration `N` inside `OC` defines a method `N` on `OC` that lazily
//! evaluates the declaration with the receiver as enclosing object and
//! caches the resulting class object per receiver.

use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{Heap, Interpreter, Signal, SourceOrigin, Symbol, Value};

fn load(src: &str) -> (Interpreter, ns_runtime::ClassRef) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

/// `Signal` carries no `Debug` impl, so `Result::expect` isn't usable on an
/// `EvalResult` -- unwrap by hand, naming which variant leaked.
fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

/// Reads a `Value` known to be a `Str` object. `Value` has no `PartialEq`,
/// and its `Debug` -- though no longer recursive -- prints only Rust
/// internals (`ObjRef(#42)`, not the string's contents), so a class-side
/// send's result is compared through this rather than `assert_eq!` on the
/// `Value` itself.
fn string_contents(heap: &Heap, value: &Value) -> String {
    match value {
        Value::Obj(obj) => heap
            .str_value(obj)
            .unwrap_or_else(|| panic!("expected ObjectData::Str")),
        _ => panic!("expected Value::Obj"),
    }
}

fn expect_error(result: Result<Value, Signal>, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}: expected a Signal::Error"),
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => e.message,
    }
}

const SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    public class Inner = Object new (| v. |) (
        public value = ( ^42 )
    )
    public makeInner = ( ^Inner new )
    public innerClass = ( ^Inner )
)
";

fn accessor_is_stable() {
    let (mut interp, class) = load(SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let first = expect_ok(
        interp.send(outer, Symbol::intern("innerClass"), vec![]),
        "first Inner",
    );
    let second = expect_ok(
        interp.send(outer, Symbol::intern("innerClass"), vec![]),
        "second Inner",
    );
    let (Value::Obj(a), Value::Obj(b)) = (&first, &second) else {
        panic!("a nested class access must yield a class object");
    };
    assert!(a.same(b), "spec 6: cached within r");
}

#[test]
fn the_accessor_returns_the_same_class_object_every_time() {
    accessor_is_stable();
}

fn distinct_per_receiver() {
    let (mut interp, class) = load(SRC);
    let platform = interp.platform();
    let one = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate first Outer",
    );
    let two = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate second Outer",
    );
    let a = expect_ok(
        interp.send(one, Symbol::intern("innerClass"), vec![]),
        "first",
    );
    let b = expect_ok(
        interp.send(two, Symbol::intern("innerClass"), vec![]),
        "second",
    );
    let (Value::Obj(a), Value::Obj(b)) = (&a, &b) else {
        panic!("class objects")
    };
    assert!(!a.same(b), "spec 6: a distinct class per instance");
    let (Some(a_application), Some(b_application)) = (
        interp.heap_ref().class_application(a),
        interp.heap_ref().class_application(b),
    ) else {
        panic!("class data")
    };
    assert!(
        Rc::ptr_eq(&a_application, &b_application),
        "spec 6.2: applications of one mixin share code and structure -- this \
         is what keeps a call site monomorphic across enclosing instances"
    );
}

#[test]
fn two_enclosing_instances_get_distinct_nested_classes() {
    distinct_per_receiver();
}

fn enclosing_object_is_the_receiver() {
    let (mut interp, class) = load(SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let inner = expect_ok(
        interp.send(outer, Symbol::intern("innerClass"), vec![]),
        "Inner",
    );
    let (Value::Obj(inner), Value::Obj(outer)) = (&inner, &outer) else {
        panic!("class object and instance")
    };
    let enclosing = interp
        .heap_ref()
        .class_enclosing(inner)
        .expect("spec 6: a nested class's one piece of state is its enclosing object");
    assert!(enclosing.same(outer));
}

#[test]
fn the_nested_class_remembers_its_enclosing_object() {
    enclosing_object_is_the_receiver();
}

fn instantiating_the_nested_class() {
    let (mut interp, class) = load(SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let instance = expect_ok(
        interp.send(outer, Symbol::intern("makeInner"), vec![]),
        "makeInner",
    );
    let value = expect_ok(
        interp.send(instance, Symbol::intern("value"), vec![]),
        "value",
    );
    let Value::SmallInt(value) = value else {
        panic!("expected a SmallInt from `Inner new value`")
    };
    assert_eq!(value, 42);
}

#[test]
fn a_nested_class_can_be_instantiated_through_its_accessor() {
    instantiating_the_nested_class();
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if ec does not evaluate
/// to a class object C." A program error, not an interpreter invariant --
/// a `NewspeakError`, never a panic.
const NON_CLASS_SUPERCLASS_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new (| notAClass = 7. |) (
    public class Inner = notAClass new () ()
    public innerClass = ( ^Inner )
)
";

fn non_class_superclass_clause() {
    let (mut interp, class) = load(NON_CLASS_SUPERCLASS_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let message = expect_error(
        interp.send(outer, Symbol::intern("innerClass"), vec![]),
        "innerClass",
    );
    assert!(
        message.contains("did not evaluate to a class object"),
        "unexpected message: {message}"
    );
}

#[test]
fn a_superclass_clause_that_is_not_a_class_is_a_program_error() {
    non_class_superclass_clause();
}

/// §6: the implicit accessor has the declaration's own accessibility. The
/// default (no modifier) is `protected`, so an ordinary explicit-receiver
/// send must not reach it -- and `lookupPublic`'s hard-stop rule means the
/// search stops at the declaration rather than continuing up the chain.
const PROTECTED_NESTED_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    class Hidden = Object new () (
        public value = ( ^7 )
    )
    public reach = ( ^Hidden new value )
)
";

fn protected_nested_access() {
    let (mut interp, class) = load(PROTECTED_NESTED_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    // Lexically, from inside Outer: reachable, and it really is a class.
    let value = expect_ok(interp.send(outer, Symbol::intern("reach"), vec![]), "reach");
    let Value::SmallInt(value) = value else {
        panic!("expected a SmallInt from `Hidden new value`")
    };
    assert_eq!(value, 7);
    // From outside, as an ordinary send: not reachable.
    let message = expect_error(
        interp.send(outer, Symbol::intern("Hidden"), vec![]),
        "Hidden",
    );
    assert!(
        message.contains("does not understand"),
        "unexpected message: {message}"
    );
}

#[test]
fn a_nested_class_defaults_to_protected_accessibility() {
    protected_nested_access();
}

/// An *inherited* nested declaration: `Derived`'s superclass clause names
/// `Base`, so the accessor for `N` is reached one level up the chain. The
/// class object `dispatch` reports as the declaring level is then `Base`'s,
/// not the receiver's own (dynamic) class -- which is what the clause frame's
/// `current_class` must be, since the clause is lexically part of `Base`'s
/// declaration.
const INHERITED_NESTED_SRC: &str = "\
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
";

fn inherited_nested_declaration() {
    let (mut interp, class) = load(INHERITED_NESTED_SRC);
    let platform = interp.platform();
    let m = expect_ok(interp.instantiate(class, vec![platform]), "instantiate M");
    let derived = expect_ok(
        interp.send(m, Symbol::intern("makeDerived"), vec![]),
        "makeDerived",
    );
    // `N` is declared on Base, reached here through Derived's chain -- and
    // its hidden cache slot is the inherited entry in Derived's layout.
    let n_class = expect_ok(interp.send(derived, Symbol::intern("N"), vec![]), "N");
    let (Value::Obj(n_class), Value::Obj(derived_obj)) = (&n_class, &derived) else {
        panic!("class object and instance")
    };
    {
        let enclosing = interp
            .heap_ref()
            .class_enclosing(n_class)
            .expect("an enclosing object");
        assert!(
            enclosing.same(derived_obj),
            "the enclosing object is the receiver, not the declaring level"
        );
    }
    // Still cached per receiver across the inherited path.
    let again = expect_ok(interp.send(derived, Symbol::intern("N"), vec![]), "N again");
    let Value::Obj(again) = &again else {
        panic!("class object")
    };
    assert!(n_class.same(again), "spec 6: cached within r");
    let instance = expect_ok(interp.instantiate(*n_class, vec![]), "instantiate N");
    let tag = expect_ok(interp.send(instance, Symbol::intern("tag"), vec![]), "tag");
    let Value::SmallInt(tag) = tag else {
        panic!("expected a SmallInt from `N new tag`")
    };
    assert_eq!(tag, 11);
}

#[test]
fn an_inherited_nested_declaration_is_reached_through_the_chain() {
    inherited_nested_declaration();
}

/// A `super`-prefixed clause receiver is the one form whose evaluation reads
/// the clause frame's `current_class` (`resolver::resolve_superclass_clause`
/// binds it as `Binding::SuperSend`). The lowerer treats a `SuperSend`-bound
/// *identifier* as the bare-builtin fallback rather than a real super send,
/// so today `super Object` names the builtin `Object` and the accessor
/// answers a class object without dispatching -- which is the outcome this
/// pins. It is the regression guard for the day that form starts
/// dispatching: by then the frame already carries the declaring class.
const SUPER_PREFIXED_CLAUSE_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    public class Inner = super Object ()
    public innerClass = ( ^Inner )
)
";

#[test]
fn a_super_prefixed_clause_answers_a_class_object() {
    let (mut interp, class) = load(SUPER_PREFIXED_CLAUSE_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let inner = expect_ok(
        interp.send(outer, Symbol::intern("innerClass"), vec![]),
        "innerClass",
    );
    let Value::Obj(inner) = inner else {
        panic!("the accessor must yield a class object")
    };
    assert!(interp.heap_ref().is_class(&inner));
}

/// A class whose creation requires itself. §6 evaluates the declaration on
/// first access, so `Inner`'s clause re-enters `Inner`'s own accessor before
/// the cache slot is filled -- unbounded recursion, and an abort rather than
/// a diagnosable error, without the in-progress guard.
const CYCLIC_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    public class Inner = Inner new ()
    public innerClass = ( ^Inner )
)
";

fn cyclic_definition() {
    let (mut interp, class) = load(CYCLIC_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let message = expect_error(
        interp.send(outer, Symbol::intern("innerClass"), vec![]),
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
        interp.send(outer, Symbol::intern("innerClass"), vec![]),
        "innerClass again",
    );
    assert_eq!(message, again);
}

#[test]
fn a_self_referential_nested_class_is_a_program_error() {
    cyclic_definition();
}

const CLASS_SIDE_SRC: &str = "\
Newspeak3
'Test'
class Widget new = Object new () (
) : (
    public describe = ( ^'a widget class' )
)
";

/// §6.3: a class object's class-side methods are reachable by sending to
/// the class object itself, through a lazily created metaclass.
fn class_side_dispatch() {
    let (mut interp, class) = load(CLASS_SIDE_SRC);
    let described = expect_ok(
        interp.send(Value::Obj(class), Symbol::intern("describe"), vec![]),
        "class-side send",
    );
    assert_eq!(
        string_contents(interp.heap_ref(), &described),
        "a widget class"
    );
}

#[test]
fn a_class_side_method_dispatches_on_the_class_object() {
    class_side_dispatch();
}

/// A class with no class side at all must still fall through to the
/// ordinary `doesNotUnderstand:` -- `metaclass_of` returns `None` and the
/// send path must not panic or invent a different error shape for a class
/// object than for any other receiver.
fn class_side_miss_is_ordinary_dnu() {
    let (mut interp, class) = load(SRC);
    let message = expect_error(
        interp.send(
            Value::Obj(class),
            Symbol::intern("noSuchClassSideMethod"),
            vec![],
        ),
        "class-side miss",
    );
    assert!(message.contains("does not understand #noSuchClassSideMethod"));
}

#[test]
fn a_class_with_no_class_side_still_reports_ordinary_does_not_understand() {
    class_side_miss_is_ordinary_dnu();
}

const CLASS_SIDE_INHERITANCE_SRC: &str = "\
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
";

/// A class side is inherited down the ordinary subclass chain like any
/// other mixin member (plain single inheritance -- not a reification of
/// §3.2's metaclass hierarchy): `SpecialWidget` declares no class side of
/// its own, but its superclass `Widget` does, so `SpecialWidget describe`
/// must answer exactly what `Widget describe` does.
fn class_side_is_inherited() {
    let (mut interp, class) = load(CLASS_SIDE_INHERITANCE_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let special_widget = expect_ok(
        interp.send(outer, Symbol::intern("SpecialWidget"), vec![]),
        "SpecialWidget accessor",
    );
    let described = expect_ok(
        interp.send(special_widget, Symbol::intern("describe"), vec![]),
        "inherited class-side send",
    );
    assert_eq!(
        string_contents(interp.heap_ref(), &described),
        "a widget class"
    );
}

#[test]
fn a_subclass_with_no_class_side_inherits_the_superclasss_class_side_method() {
    class_side_is_inherited();
}

/// The class object a nested declaration's accessor answers for `enclosing`.
fn nested_class_object(
    interp: &mut Interpreter,
    enclosing: Value,
    name: &str,
) -> ns_runtime::ClassRef {
    let value = expect_ok(interp.send(enclosing, Symbol::intern(name), vec![]), name);
    match value {
        Value::Obj(obj) => obj,
        _ => panic!("{name} answers a class object"),
    }
}

fn application_of(
    heap: &ns_runtime::Heap,
    class: &ns_runtime::ClassRef,
) -> Rc<ns_runtime::MixinApplication> {
    heap.class_application(class).expect("a class object")
}

/// §6.2 on the metaclass side: two class objects that share one
/// instance-side application must share one *metaclass* application too, or
/// a call site that went monomorphic on the application would split as soon
/// as a second enclosing instance appeared.
///
/// `SpecialWidget` in `tests-fixtures/class_side.ns` is the case that makes
/// this bite: it inherits its class side instead of declaring one, so its
/// metaclass is built from a synthetic empty mixin -- which has to have one
/// identity per mixin, not one per class object.
fn an_inherited_class_side_shares_one_application() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures/class_side.ns");
    let src = std::fs::read_to_string(&path).expect("read tests-fixtures/class_side.ns");
    let (mut interp, class) = load(&src);
    let platform = interp.platform();
    let one = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate first ClassSide",
    );
    let two = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate second ClassSide",
    );
    let a = nested_class_object(&mut interp, one, "SpecialWidget");
    let b = nested_class_object(&mut interp, two, "SpecialWidget");
    assert!(
        !a.same(&b),
        "spec 6: one class object per enclosing instance"
    );
    assert!(
        Rc::ptr_eq(
            &application_of(interp.heap_ref(), &a),
            &application_of(interp.heap_ref(), &b)
        ),
        "spec 6.2: both share one instance-side application"
    );
    let ma = interp
        .metaclass_of(&a)
        .expect("SpecialWidget inherits Widget's class side");
    let mb = interp
        .metaclass_of(&b)
        .expect("SpecialWidget inherits Widget's class side");
    assert!(
        Rc::ptr_eq(
            &application_of(interp.heap_ref(), &ma),
            &application_of(interp.heap_ref(), &mb)
        ),
        "spec 6.2: an inherited class side must share one metaclass application too"
    );
}

#[test]
fn two_class_objects_sharing_an_application_share_their_inherited_class_side() {
    an_inherited_class_side_shares_one_application();
}

/// A class that declares its own class-side method of the same name still
/// overrides the inherited one -- ordinary shadowing, exactly as on the
/// instance side.
fn class_side_override_shadows_the_inherited_method() {
    let (mut interp, class) = load(CLASS_SIDE_INHERITANCE_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let overriding_widget = expect_ok(
        interp.send(outer, Symbol::intern("OverridingWidget"), vec![]),
        "OverridingWidget accessor",
    );
    let described = expect_ok(
        interp.send(overriding_widget, Symbol::intern("describe"), vec![]),
        "overriding class-side send",
    );
    assert_eq!(
        string_contents(interp.heap_ref(), &described),
        "a special widget class"
    );
}

#[test]
fn a_class_sides_own_method_overrides_the_inherited_one_of_the_same_name() {
    class_side_override_shadows_the_inherited_method();
}

/// `Frame.current_class` must be the class object of the level
/// that *defines* the running method, not the receiver's own (dynamic)
/// class -- otherwise `super`/`outer` inside an inherited method read the
/// wrong level. `Derived` inherits nothing here (it overrides `describe`
/// itself), but its `super describe` lands in `Base`'s activation, and
/// `Base`'s own `outer Outer label` must resolve from *that* activation.
/// Mirrors `tests-fixtures/nested_super.ns` at the API level.
const NESTED_SUPER_SRC: &str = "\
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
";

fn nested_super_and_outer() {
    let (mut interp, class) = load(NESTED_SUPER_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let described = expect_ok(interp.send(outer, Symbol::intern("run"), vec![]), "run");
    assert_eq!(
        string_contents(interp.heap_ref(), &described),
        "outer/derived"
    );
}

#[test]
fn super_and_outer_both_resolve_through_the_defining_level() {
    nested_super_and_outer();
}

/// A *three*-level hierarchy where the middle level (`Mid`) is reached only
/// through inheritance -- `Leaf` declares no `describe` of its own. This is
/// the shape a two-level fixture cannot exercise: if
/// `invoke_method` set `current_class` to the receiver's own (dynamic)
/// class rather than the level `dispatch` found the method at, `Mid`'s
/// activation would compute its `super` target from *`Leaf`'s* superclass
/// (`Mid` itself) forever -- unbounded recursion, not a wrong value, so a
/// two-level chain (where the bug and the fix agree on the first hop) would
/// pass either way. This guards the actual defect class the level-tracking
/// machinery exists for.
const SUPER_CHAIN_SRC: &str = "\
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
";

fn super_chain_progresses_past_an_inherited_middle_level() {
    let (mut interp, class) = load(SUPER_CHAIN_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let described = expect_ok(interp.send(outer, Symbol::intern("run"), vec![]), "run");
    assert_eq!(string_contents(interp.heap_ref(), &described), "grand/mid");
}

#[test]
fn a_super_chain_progresses_through_an_inherited_middle_level() {
    super_chain_progresses_past_an_inherited_middle_level();
}

/// §3.3: an activation's enclosing objects belong to the level that
/// *defines* the running method. `Mid` and `Base` are both nested directly
/// in the module class, so `Derived` (nested in `Mid`) sits one level
/// deeper than `Base`, from which it inherits `describe`. `describe`'s
/// `outer Outer label` is a lexical depth of 1 fixed inside `Base`, so it
/// must reach the module instance and answer `'top'` on any receiver --
/// resolving the depth from the receiver's own class instead reaches the
/// `Mid` instance, whose decoy `label` answers `'mid'`.
const OUTER_THROUGH_INHERITED_SRC: &str = "\
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
";

fn outer_resolves_from_the_defining_level() {
    let (mut interp, class) = load(OUTER_THROUGH_INHERITED_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let described = expect_ok(interp.send(outer, Symbol::intern("run"), vec![]), "run");
    assert_eq!(string_contents(interp.heap_ref(), &described), "top");
}

#[test]
fn outer_in_an_inherited_method_resolves_from_the_defining_level() {
    outer_resolves_from_the_defining_level();
}

/// The same defect one hop further out, which only a lexical *declaration*
/// chain catches. `Deep`'s `outer Outer` is a depth of 2, so the walk
/// leaves `Deep`'s enclosing object -- a `Derived` instance -- and must
/// then leave the level that declares `Deep`, which is `Base`, not
/// `Derived`'s own class. Stepping through the reached object's class
/// instead lands on `Mid`'s instance and its decoy `label`.
const DEEP_OUTER_THROUGH_INHERITED_SRC: &str = "\
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
";

fn a_two_level_outer_resolves_from_the_defining_level() {
    let (mut interp, class) = load(DEEP_OUTER_THROUGH_INHERITED_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let described = expect_ok(interp.send(outer, Symbol::intern("run"), vec![]), "run");
    assert_eq!(string_contents(interp.heap_ref(), &described), "top");
}

#[test]
fn a_two_level_outer_through_an_inherited_nested_class_resolves_lexically() {
    a_two_level_outer_resolves_from_the_defining_level();
}

/// §6.3.1: a class side is written inside the same class declaration its
/// instance side is, and `resolver::resolve_class` resolves both against
/// one lexical chain -- so `outer` in a class-side method reaches exactly
/// what it reaches on the instance side. `Widget`'s class side is at
/// depth 1 from the module class; `Part`'s, nested one level further, is
/// at depth 2 and can only be walked through the declaration chain.
/// `Widget`'s decoy `label` is what a walk that stopped one level short
/// would answer with.
const CLASS_SIDE_OUTER_SRC: &str = "\
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
";

fn class_side_outer_reaches_the_enclosing_object() {
    let (mut interp, class) = load(CLASS_SIDE_OUTER_SRC);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform]),
        "instantiate Outer",
    );
    let shallow = expect_ok(
        interp.send(outer, Symbol::intern("shallow"), vec![]),
        "shallow",
    );
    assert_eq!(string_contents(interp.heap_ref(), &shallow), "top");
    let deep = expect_ok(interp.send(outer, Symbol::intern("deep"), vec![]), "deep");
    assert_eq!(string_contents(interp.heap_ref(), &deep), "top");
}

#[test]
fn outer_in_a_class_side_method_reaches_the_enclosing_object() {
    class_side_outer_reaches_the_enclosing_object();
}
