//! The implicit nested-class accessor (`newspeak-spec.pdf` §6): a nested
//! declaration `N` inside `OC` defines a method `N` on `OC` that lazily
//! evaluates the declaration with the receiver as enclosing object and
//! caches the resulting class object per receiver.
//!
//! Every behavioural test runs under both engines: the superclass clause is
//! Newspeak code, so a tree-walked clause under `--engine=bytecode` would be
//! exactly the kind of silent divergence the differential gate exists to
//! catch, and a unit test that only ever runs the tree-walker cannot see it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    compiler, Binding, Engine, Heap, Interpreter, Signal, SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

type Bindings = Rc<HashMap<Span, Binding>>;

fn load(src: &str, bytecode: bool) -> (Interpreter, ns_runtime::ClassRef, Bindings) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    if bytecode {
        let compiled = compiler::compile_module(interp.heap_ref(), &class, &bindings);
        interp
            .set_engine(Engine::Bytecode(compiled))
            .expect("a source-built interpreter accepts the bytecode engine");
    }
    (interp, class, bindings)
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

/// Reads a `Value` known to be a `Str` object. `Value` has no usable
/// `Debug` (the bootstrap `Object` class is its own class, so a derived
/// `Debug` on a class-object-carrying `Value` recurses), so a class-side
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

fn accessor_is_stable(bytecode: bool) {
    let (mut interp, class, bindings) = load(SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let first = expect_ok(
        interp.send(
            outer.clone(),
            Symbol::intern("innerClass"),
            vec![],
            &bindings,
            None,
        ),
        "first Inner",
    );
    let second = expect_ok(
        interp.send(outer, Symbol::intern("innerClass"), vec![], &bindings, None),
        "second Inner",
    );
    let (Value::Obj(a), Value::Obj(b)) = (&first, &second) else {
        panic!("a nested class access must yield a class object");
    };
    assert!(Rc::ptr_eq(a, b), "spec 6: cached within r");
}

#[test]
fn the_accessor_returns_the_same_class_object_every_time() {
    accessor_is_stable(false);
}

#[test]
fn the_accessor_returns_the_same_class_object_every_time_under_bytecode() {
    accessor_is_stable(true);
}

fn distinct_per_receiver(bytecode: bool) {
    let (mut interp, class, bindings) = load(SRC, bytecode);
    let platform = interp.platform();
    let one = expect_ok(
        interp.instantiate(class.clone(), vec![platform.clone()], &bindings),
        "instantiate first Outer",
    );
    let two = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate second Outer",
    );
    let a = expect_ok(
        interp.send(one, Symbol::intern("innerClass"), vec![], &bindings, None),
        "first",
    );
    let b = expect_ok(
        interp.send(two, Symbol::intern("innerClass"), vec![], &bindings, None),
        "second",
    );
    let (Value::Obj(a), Value::Obj(b)) = (&a, &b) else {
        panic!("class objects")
    };
    assert!(!Rc::ptr_eq(a, b), "spec 6: a distinct class per instance");
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
    distinct_per_receiver(false);
}

#[test]
fn two_enclosing_instances_get_distinct_nested_classes_under_bytecode() {
    distinct_per_receiver(true);
}

fn enclosing_object_is_the_receiver(bytecode: bool) {
    let (mut interp, class, bindings) = load(SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let inner = expect_ok(
        interp.send(
            outer.clone(),
            Symbol::intern("innerClass"),
            vec![],
            &bindings,
            None,
        ),
        "Inner",
    );
    let (Value::Obj(inner), Value::Obj(outer)) = (&inner, &outer) else {
        panic!("class object and instance")
    };
    let enclosing = interp
        .heap_ref()
        .class_enclosing(inner)
        .expect("spec 6: a nested class's one piece of state is its enclosing object");
    assert!(Rc::ptr_eq(&enclosing, outer));
}

#[test]
fn the_nested_class_remembers_its_enclosing_object() {
    enclosing_object_is_the_receiver(false);
}

#[test]
fn the_nested_class_remembers_its_enclosing_object_under_bytecode() {
    enclosing_object_is_the_receiver(true);
}

fn instantiating_the_nested_class(bytecode: bool) {
    let (mut interp, class, bindings) = load(SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let instance = expect_ok(
        interp.send(outer, Symbol::intern("makeInner"), vec![], &bindings, None),
        "makeInner",
    );
    let value = expect_ok(
        interp.send(instance, Symbol::intern("value"), vec![], &bindings, None),
        "value",
    );
    let Value::SmallInt(value) = value else {
        panic!("expected a SmallInt from `Inner new value`")
    };
    assert_eq!(value, 42);
}

#[test]
fn a_nested_class_can_be_instantiated_through_its_accessor() {
    instantiating_the_nested_class(false);
}

#[test]
fn a_nested_class_can_be_instantiated_through_its_accessor_under_bytecode() {
    instantiating_the_nested_class(true);
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if ec does not evaluate
/// to a class object C." A program error, not an interpreter invariant --
/// the same `NewspeakError` under both engines, never a panic.
const NON_CLASS_SUPERCLASS_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new (| notAClass = 7. |) (
    public class Inner = notAClass new () ()
    public innerClass = ( ^Inner )
)
";

fn non_class_superclass_clause(bytecode: bool) {
    let (mut interp, class, bindings) = load(NON_CLASS_SUPERCLASS_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let message = expect_error(
        interp.send(outer, Symbol::intern("innerClass"), vec![], &bindings, None),
        "innerClass",
    );
    assert!(
        message.contains("did not evaluate to a class object"),
        "unexpected message: {message}"
    );
}

#[test]
fn a_superclass_clause_that_is_not_a_class_is_a_program_error() {
    non_class_superclass_clause(false);
}

#[test]
fn a_superclass_clause_that_is_not_a_class_is_a_program_error_under_bytecode() {
    non_class_superclass_clause(true);
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

fn protected_nested_access(bytecode: bool) {
    let (mut interp, class, bindings) = load(PROTECTED_NESTED_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    // Lexically, from inside Outer: reachable, and it really is a class.
    let value = expect_ok(
        interp.send(
            outer.clone(),
            Symbol::intern("reach"),
            vec![],
            &bindings,
            None,
        ),
        "reach",
    );
    let Value::SmallInt(value) = value else {
        panic!("expected a SmallInt from `Hidden new value`")
    };
    assert_eq!(value, 7);
    // From outside, as an ordinary send: not reachable.
    let message = expect_error(
        interp.send(outer, Symbol::intern("Hidden"), vec![], &bindings, None),
        "Hidden",
    );
    assert!(
        message.contains("does not understand"),
        "unexpected message: {message}"
    );
}

#[test]
fn a_nested_class_defaults_to_protected_accessibility() {
    protected_nested_access(false);
}

#[test]
fn a_nested_class_defaults_to_protected_accessibility_under_bytecode() {
    protected_nested_access(true);
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

fn inherited_nested_declaration(bytecode: bool) {
    let (mut interp, class, bindings) = load(INHERITED_NESTED_SRC, bytecode);
    let platform = interp.platform();
    let m = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate M",
    );
    let derived = expect_ok(
        interp.send(m, Symbol::intern("makeDerived"), vec![], &bindings, None),
        "makeDerived",
    );
    // `N` is declared on Base, reached here through Derived's chain -- and
    // its hidden cache slot is the inherited entry in Derived's layout.
    let n_class = expect_ok(
        interp.send(
            derived.clone(),
            Symbol::intern("N"),
            vec![],
            &bindings,
            None,
        ),
        "N",
    );
    let (Value::Obj(n_class), Value::Obj(derived_obj)) = (&n_class, &derived) else {
        panic!("class object and instance")
    };
    {
        let enclosing = interp
            .heap_ref()
            .class_enclosing(n_class)
            .expect("an enclosing object");
        assert!(
            Rc::ptr_eq(&enclosing, derived_obj),
            "the enclosing object is the receiver, not the declaring level"
        );
    }
    // Still cached per receiver across the inherited path.
    let again = expect_ok(
        interp.send(derived, Symbol::intern("N"), vec![], &bindings, None),
        "N again",
    );
    let Value::Obj(again) = &again else {
        panic!("class object")
    };
    assert!(Rc::ptr_eq(n_class, again), "spec 6: cached within r");
    let instance = expect_ok(
        interp.instantiate(n_class.clone(), vec![], &bindings),
        "instantiate N",
    );
    let tag = expect_ok(
        interp.send(instance, Symbol::intern("tag"), vec![], &bindings, None),
        "tag",
    );
    let Value::SmallInt(tag) = tag else {
        panic!("expected a SmallInt from `N new tag`")
    };
    assert_eq!(tag, 11);
}

#[test]
fn an_inherited_nested_declaration_is_reached_through_the_chain() {
    inherited_nested_declaration(false);
}

#[test]
fn an_inherited_nested_declaration_is_reached_through_the_chain_under_bytecode() {
    inherited_nested_declaration(true);
}

/// A `super`-prefixed clause receiver is the one form whose evaluation reads
/// the clause frame's `current_class` (`resolver::resolve_superclass_clause`
/// binds it as `Binding::SuperSend`). Milestone 1's `eval_ident` and the
/// lowerer both treat a `SuperSend`-bound *identifier* as the bare-builtin
/// fallback rather than a real super send, so today it raises the shared
/// unresolved-identifier error instead of dispatching -- which is why this
/// pins the two engines to the same outcome rather than to a class object.
/// It is the regression guard for the day that form starts dispatching: by
/// then the frame already carries the declaring class.
const SUPER_PREFIXED_CLAUSE_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    public class Inner = super Object ()
    public innerClass = ( ^Inner )
)
";

#[test]
fn a_super_prefixed_clause_behaves_identically_under_both_engines() {
    let mut outcomes = vec![];
    for bytecode in [false, true] {
        let (mut interp, class, bindings) = load(SUPER_PREFIXED_CLAUSE_SRC, bytecode);
        let platform = interp.platform();
        let outer = expect_ok(
            interp.instantiate(class, vec![platform], &bindings),
            "instantiate Outer",
        );
        outcomes.push(
            match interp.send(outer, Symbol::intern("innerClass"), vec![], &bindings, None) {
                Ok(Value::Obj(_)) => "a class object".to_string(),
                Ok(_) => "a non-object value".to_string(),
                Err(Signal::Return(..)) => "Signal::Return".to_string(),
                Err(Signal::Error(e)) => e.message,
            },
        );
    }
    assert_eq!(
        outcomes[0], outcomes[1],
        "the two engines must agree on a super-prefixed superclass clause"
    );
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

fn cyclic_definition(bytecode: bool) {
    let (mut interp, class, bindings) = load(CYCLIC_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let message = expect_error(
        interp.send(
            outer.clone(),
            Symbol::intern("innerClass"),
            vec![],
            &bindings,
            None,
        ),
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
        interp.send(outer, Symbol::intern("innerClass"), vec![], &bindings, None),
        "innerClass again",
    );
    assert_eq!(message, again);
}

#[test]
fn a_self_referential_nested_class_is_a_program_error() {
    cyclic_definition(false);
}

#[test]
fn a_self_referential_nested_class_is_a_program_error_under_bytecode() {
    cyclic_definition(true);
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
fn class_side_dispatch(bytecode: bool) {
    let (mut interp, class, bindings) = load(CLASS_SIDE_SRC, bytecode);
    let described = expect_ok(
        interp.send(
            Value::Obj(class),
            Symbol::intern("describe"),
            vec![],
            &bindings,
            None,
        ),
        "class-side send",
    );
    assert_eq!(
        string_contents(interp.heap_ref(), &described),
        "a widget class"
    );
}

#[test]
fn a_class_side_method_dispatches_on_the_class_object() {
    class_side_dispatch(false);
}

#[test]
fn a_class_side_method_dispatches_on_the_class_object_under_bytecode() {
    class_side_dispatch(true);
}

/// A class with no class side at all must still fall through to the
/// ordinary `doesNotUnderstand:` -- `metaclass_of` returns `None` and the
/// send path must not panic or invent a different error shape for a class
/// object than for any other receiver.
fn class_side_miss_is_ordinary_dnu(bytecode: bool) {
    let (mut interp, class, bindings) = load(SRC, bytecode);
    let message = expect_error(
        interp.send(
            Value::Obj(class),
            Symbol::intern("noSuchClassSideMethod"),
            vec![],
            &bindings,
            None,
        ),
        "class-side miss",
    );
    assert!(message.contains("does not understand #noSuchClassSideMethod"));
}

#[test]
fn a_class_with_no_class_side_still_reports_ordinary_does_not_understand() {
    class_side_miss_is_ordinary_dnu(false);
}

#[test]
fn a_class_with_no_class_side_still_reports_ordinary_does_not_understand_under_bytecode() {
    class_side_miss_is_ordinary_dnu(true);
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
fn class_side_is_inherited(bytecode: bool) {
    let (mut interp, class, bindings) = load(CLASS_SIDE_INHERITANCE_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let special_widget = expect_ok(
        interp.send(
            outer,
            Symbol::intern("SpecialWidget"),
            vec![],
            &bindings,
            None,
        ),
        "SpecialWidget accessor",
    );
    let described = expect_ok(
        interp.send(
            special_widget,
            Symbol::intern("describe"),
            vec![],
            &bindings,
            None,
        ),
        "inherited class-side send",
    );
    assert_eq!(
        string_contents(interp.heap_ref(), &described),
        "a widget class"
    );
}

#[test]
fn a_subclass_with_no_class_side_inherits_the_superclasss_class_side_method() {
    class_side_is_inherited(false);
}

#[test]
fn a_subclass_with_no_class_side_inherits_the_superclasss_class_side_method_under_bytecode() {
    class_side_is_inherited(true);
}

/// The class object a nested declaration's accessor answers for `enclosing`.
fn nested_class_object(
    interp: &mut Interpreter,
    enclosing: Value,
    name: &str,
    bindings: &Bindings,
) -> ns_runtime::ClassRef {
    let value = expect_ok(
        interp.send(enclosing, Symbol::intern(name), vec![], bindings, None),
        name,
    );
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
fn an_inherited_class_side_shares_one_application(bytecode: bool) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures/class_side.ns");
    let src = std::fs::read_to_string(&path).expect("read tests-fixtures/class_side.ns");
    let (mut interp, class, bindings) = load(&src, bytecode);
    let platform = interp.platform();
    let one = expect_ok(
        interp.instantiate(class.clone(), vec![platform.clone()], &bindings),
        "instantiate first ClassSide",
    );
    let two = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate second ClassSide",
    );
    let a = nested_class_object(&mut interp, one, "SpecialWidget", &bindings);
    let b = nested_class_object(&mut interp, two, "SpecialWidget", &bindings);
    assert!(
        !Rc::ptr_eq(&a, &b),
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
    an_inherited_class_side_shares_one_application(false);
}

#[test]
fn two_class_objects_sharing_an_application_share_their_inherited_class_side_under_bytecode() {
    an_inherited_class_side_shares_one_application(true);
}

/// A class that declares its own class-side method of the same name still
/// overrides the inherited one -- ordinary shadowing, exactly as on the
/// instance side.
fn class_side_override_shadows_the_inherited_method(bytecode: bool) {
    let (mut interp, class, bindings) = load(CLASS_SIDE_INHERITANCE_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let overriding_widget = expect_ok(
        interp.send(
            outer,
            Symbol::intern("OverridingWidget"),
            vec![],
            &bindings,
            None,
        ),
        "OverridingWidget accessor",
    );
    let described = expect_ok(
        interp.send(
            overriding_widget,
            Symbol::intern("describe"),
            vec![],
            &bindings,
            None,
        ),
        "overriding class-side send",
    );
    assert_eq!(
        string_contents(interp.heap_ref(), &described),
        "a special widget class"
    );
}

#[test]
fn a_class_sides_own_method_overrides_the_inherited_one_of_the_same_name() {
    class_side_override_shadows_the_inherited_method(false);
}

#[test]
fn a_class_sides_own_method_overrides_the_inherited_one_of_the_same_name_under_bytecode() {
    class_side_override_shadows_the_inherited_method(true);
}

/// Task 10: `Frame.current_class` must be the class object of the level
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

fn nested_super_and_outer(bytecode: bool) {
    let (mut interp, class, bindings) = load(NESTED_SUPER_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let described = expect_ok(
        interp.send(outer, Symbol::intern("run"), vec![], &bindings, None),
        "run",
    );
    assert_eq!(
        string_contents(interp.heap_ref(), &described),
        "outer/derived"
    );
}

#[test]
fn super_and_outer_both_resolve_through_the_defining_level() {
    nested_super_and_outer(false);
}

#[test]
fn super_and_outer_both_resolve_through_the_defining_level_under_bytecode() {
    nested_super_and_outer(true);
}

/// A *three*-level hierarchy where the middle level (`Mid`) is reached only
/// through inheritance -- `Leaf` declares no `describe` of its own. This is
/// the shape Task 10's two-level fixture cannot exercise: if
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

fn super_chain_progresses_past_an_inherited_middle_level(bytecode: bool) {
    let (mut interp, class, bindings) = load(SUPER_CHAIN_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let described = expect_ok(
        interp.send(outer, Symbol::intern("run"), vec![], &bindings, None),
        "run",
    );
    assert_eq!(string_contents(interp.heap_ref(), &described), "grand/mid");
}

#[test]
fn a_super_chain_progresses_through_an_inherited_middle_level() {
    super_chain_progresses_past_an_inherited_middle_level(false);
}

#[test]
fn a_super_chain_progresses_through_an_inherited_middle_level_under_bytecode() {
    super_chain_progresses_past_an_inherited_middle_level(true);
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

fn outer_resolves_from_the_defining_level(bytecode: bool) {
    let (mut interp, class, bindings) = load(OUTER_THROUGH_INHERITED_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let described = expect_ok(
        interp.send(outer, Symbol::intern("run"), vec![], &bindings, None),
        "run",
    );
    assert_eq!(string_contents(interp.heap_ref(), &described), "top");
}

#[test]
fn outer_in_an_inherited_method_resolves_from_the_defining_level() {
    outer_resolves_from_the_defining_level(false);
}

#[test]
fn outer_in_an_inherited_method_resolves_from_the_defining_level_under_bytecode() {
    outer_resolves_from_the_defining_level(true);
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

fn a_two_level_outer_resolves_from_the_defining_level(bytecode: bool) {
    let (mut interp, class, bindings) = load(DEEP_OUTER_THROUGH_INHERITED_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let described = expect_ok(
        interp.send(outer, Symbol::intern("run"), vec![], &bindings, None),
        "run",
    );
    assert_eq!(string_contents(interp.heap_ref(), &described), "top");
}

#[test]
fn a_two_level_outer_through_an_inherited_nested_class_resolves_lexically() {
    a_two_level_outer_resolves_from_the_defining_level(false);
}

#[test]
fn a_two_level_outer_through_an_inherited_nested_class_resolves_lexically_under_bytecode() {
    a_two_level_outer_resolves_from_the_defining_level(true);
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

fn class_side_outer_reaches_the_enclosing_object(bytecode: bool) {
    let (mut interp, class, bindings) = load(CLASS_SIDE_OUTER_SRC, bytecode);
    let platform = interp.platform();
    let outer = expect_ok(
        interp.instantiate(class, vec![platform], &bindings),
        "instantiate Outer",
    );
    let shallow = expect_ok(
        interp.send(
            outer.clone(),
            Symbol::intern("shallow"),
            vec![],
            &bindings,
            None,
        ),
        "shallow",
    );
    assert_eq!(string_contents(interp.heap_ref(), &shallow), "top");
    let deep = expect_ok(
        interp.send(outer, Symbol::intern("deep"), vec![], &bindings, None),
        "deep",
    );
    assert_eq!(string_contents(interp.heap_ref(), &deep), "top");
}

#[test]
fn outer_in_a_class_side_method_reaches_the_enclosing_object() {
    class_side_outer_reaches_the_enclosing_object(false);
}

#[test]
fn outer_in_a_class_side_method_reaches_the_enclosing_object_under_bytecode() {
    class_side_outer_reaches_the_enclosing_object(true);
}
