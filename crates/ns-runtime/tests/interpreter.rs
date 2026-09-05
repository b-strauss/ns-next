//! Interpreter core tests: expression evaluation, ordinary sends, and
//! non-local return, built over hand-assembled `ClassData`/`Object`/`Frame`
//! values -- the same construction pattern `dispatch.rs`'s own test module
//! (Task 5) uses, extended here to actually *run* method bodies rather than
//! only look them up.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_runtime::interpreter::eval;
use ns_runtime::{
    Binding, ClassData, ClosureData, Frame, FrameKind, Interpreter, NewspeakError, Object,
    ObjectData, Signal, Symbol, Value,
};

/// A placeholder `ClassRef` whose own `.class` field points back to itself.
/// `Object.class` is non-optional (`ClassRef`, not `Option<ClassRef>`), so
/// constructing any `Object` requires already having a `ClassRef` in hand
/// for its `class` field -- and these test classes are never linked into a
/// real bootstrapped class hierarchy, so there is no other object to point
/// at. None of these tests ever read a class object's own `.class` field,
/// so any non-panicking placeholder is sufficient; a self-referential one
/// is the simplest to build.
///
/// Safe Rust cannot construct a strong `Rc<RefCell<T>>` that already holds
/// a strong `Rc` to itself in one expression. This uses the same
/// allocate-uninitialized-then-write technique as `types.rs`'s and
/// `dispatch.rs`'s own test helpers (Tasks 3, 5): mint a second strong
/// handle to the same not-yet-initialized allocation (balancing the strong
/// count via `Rc::increment_strong_count` before calling `Rc::from_raw`),
/// write the real value using that handle as `class`, then `assume_init`.
fn dummy_class() -> ns_runtime::ClassRef {
    let uninit: Rc<std::mem::MaybeUninit<RefCell<Object>>> = Rc::new_uninit();
    let raw_mut = Rc::as_ptr(&uninit) as *mut std::mem::MaybeUninit<RefCell<Object>>;
    let data_ptr = raw_mut as *const RefCell<Object>;
    // SAFETY: `data_ptr` points at the allocation `uninit` owns; we bump
    // its strong count by one before minting a second `Rc` handle to it via
    // `from_raw`, so the two handles (`uninit`, and the `class` field
    // written below) share a correctly balanced count. `MaybeUninit<T>` is
    // guaranteed to have the same layout as `T`, so the pointer cast is
    // sound; we never dereference through either handle before the `write`
    // below completes.
    unsafe { Rc::increment_strong_count(data_ptr) };
    let self_ref: ns_runtime::ClassRef = unsafe { Rc::from_raw(data_ptr) };
    unsafe {
        (*raw_mut).write(RefCell::new(Object {
            class: self_ref,
            identity_hash: 0,
            data: ObjectData::Instance(vec![]),
        }));
    }
    // SAFETY: the write above just initialized the payload; `uninit` is
    // the allocation's original handle, now fully initialized.
    unsafe { uninit.assume_init() }
}

/// Parses `src` (a full one-class module) and returns the class's own
/// `Rc<ast::MethodDecl>`s by unary selector name, plus the module's
/// resolved `Rc<HashMap<Span, Binding>>`. Building `MethodDecl`s via a real
/// parse -- rather than hand-constructing deeply nested AST literals --
/// follows `dispatch.rs`'s `class_with_method` helper (Task 5).
fn parse_methods(
    src: &str,
) -> (
    Vec<Rc<ns_syntax::ast::MethodDecl>>,
    Rc<HashMap<ns_syntax::Span, Binding>>,
) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module.clone(),
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let methods = module
        .root_class
        .instance_side
        .methods
        .iter()
        .cloned()
        .map(Rc::new)
        .collect();
    (methods, resolved.bindings)
}

/// Builds a `ClassRef` whose `methods` map holds every method in `methods`,
/// keyed by its own selector name (works for the unary selectors these
/// tests use).
fn class_with_methods(
    name: &str,
    methods: Vec<Rc<ns_syntax::ast::MethodDecl>>,
) -> ns_runtime::ClassRef {
    let mut map = HashMap::new();
    for m in methods {
        let selector = match &m.pattern {
            ns_syntax::ast::MessagePattern::Unary { selector } => Symbol::intern(&selector.name),
            ns_syntax::ast::MessagePattern::Binary { selector, .. } => {
                Symbol::intern(&selector.name)
            }
            ns_syntax::ast::MessagePattern::Keyword { parts } => {
                let full: String = parts
                    .iter()
                    .map(|(kw, _)| format!("{}:", kw.name))
                    .collect();
                Symbol::intern(&full)
            }
        };
        map.insert(selector, m);
    }
    Rc::new(RefCell::new(Object {
        class: dummy_class(),
        identity_hash: 0,
        data: ObjectData::Class(ClassData {
            name: Symbol::intern(name),
            superclass: None,
            methods: map,
            instance_slots: vec![],
            slot_index: HashMap::new(),
            init_exprs: vec![],
            class_side: None,
            enclosing: None,
            factory: None,
            builtin: None,
        }),
    }))
}

fn instance_of(class: ns_runtime::ClassRef) -> ns_runtime::ObjRef {
    Rc::new(RefCell::new(Object {
        class,
        identity_hash: 1,
        data: ObjectData::Instance(vec![]),
    }))
}

#[test]
fn method_with_no_explicit_return_returns_self() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public bar = ( 1. 2 )\n) : ()";
    let (methods, bindings) = parse_methods(src);
    let class = class_with_methods("Foo", methods);
    let instance = instance_of(class);

    let mut interp = Interpreter::new();
    let result = interp.send(
        Value::Obj(instance.clone()),
        Symbol::intern("bar"),
        vec![],
        &bindings,
        None,
    );

    match result {
        Ok(Value::Obj(obj)) => assert!(
            Rc::ptr_eq(&obj, &instance),
            "a method with no explicit return must yield the receiver (self), not the last expression's value"
        ),
        Ok(other) => panic!("expected Value::Obj(self), got {other:?}"),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return -- `send` should never see one escape `invoke_method`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
    }
}

#[test]
fn method_with_explicit_return_returns_the_returned_value() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public answer = ( ^42 )\n) : ()";
    let (methods, bindings) = parse_methods(src);
    let class = class_with_methods("Foo", methods);
    let instance = instance_of(class);

    let mut interp = Interpreter::new();
    let result = interp.send(
        Value::Obj(instance),
        Symbol::intern("answer"),
        vec![],
        &bindings,
        None,
    );

    match result {
        Ok(v) => assert!(
            matches!(v, Value::SmallInt(42)),
            "expected Value::SmallInt(42), got {v:?}"
        ),
        Err(Signal::Return(..)) => panic!(
            "unexpected Signal::Return -- `send` should never see one escape `invoke_method`"
        ),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
    }
}

/// Pulls the `CodeBody` of a bare block literal (`[ ^1 ]`) that is `src`'s
/// first method's sole statement -- i.e. parses the block out of a real
/// module rather than hand-building `ast::CodeBody`/`ast::Stmt` literals.
fn parse_block_body(src: &str) -> ns_syntax::ast::CodeBody {
    let module = ns_syntax::parse(src).expect("parse");
    let method = &module.root_class.instance_side.methods[0];
    let ns_syntax::ast::Stmt::Expr(ns_syntax::ast::Expr::Block(block)) = &method.body.stmts[0]
    else {
        panic!("expected the method's first statement to be a bare block literal");
    };
    block.body.clone()
}

/// Builds a `Frame` shaped exactly like the one `Interpreter::invoke_method`
/// would build for a trivial no-arg method -- used as a block's `home` in
/// the non-local-return tests below, since block-invocation-selector native
/// dispatch (`value`) does not exist until Task 10 and so cannot be reached
/// through `Interpreter::send`.
fn method_frame() -> ns_runtime::FrameRef {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^0 )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let method = Rc::new(module.root_class.instance_side.methods[0].clone());
    Rc::new(RefCell::new(Frame {
        receiver: Value::Nil,
        args: vec![],
        temps: vec![],
        pc: 0,
        sender: None,
        home: None,
        current_class: None,
        kind: FrameKind::Method(method),
        uncontinuable: false,
    }))
}

/// Gap 1 (Task 12 brief): sending a method whose body contains a bare
/// `OrderedCollection new` (no `platform` prefix, outside any superclass
/// clause) must produce a real, working `OrderedCollection` instance --
/// `eval_ident`'s symmetric fix to the resolver's `resolve_ident` change,
/// exercised end to end through a real `send`. Uses `Interpreter::bootstrap`
/// (not `Interpreter::new`) since `builtin_class` requires the builtin
/// class table to already be built.
#[test]
fn bare_builtin_class_name_reference_produces_a_working_instance() {
    let src =
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public bar = ( ^OrderedCollection new )\n) : ()";
    let (methods, bindings) = parse_methods(src);
    let class = class_with_methods("Foo", methods);
    let instance = instance_of(class);

    let mut interp = Interpreter::bootstrap();
    let result = interp.send(
        Value::Obj(instance),
        Symbol::intern("bar"),
        vec![],
        &bindings,
        None,
    );

    match result {
        Ok(Value::Obj(obj)) => {
            assert!(
                matches!(&obj.borrow().data, ObjectData::Array(elems) if elems.is_empty()),
                "expected a fresh, empty OrderedCollection instance, got {:?}",
                obj.borrow().data
            );
        }
        Ok(other) => panic!("expected Value::Obj(<OrderedCollection instance>), got {other:?}"),
        Err(Signal::Return(..)) => panic!(
            "unexpected Signal::Return -- `send` should never see one escape `invoke_method`"
        ),
        Err(Signal::Error(e)) => panic!(
            "unexpected Signal::Error (bare `OrderedCollection` should resolve to the builtin class, not doesNotUnderstand:): {}",
            e.message
        ),
    }
}

#[test]
fn non_local_return_from_a_block_unwinds_to_the_home_methods_activation() {
    // `[ ^1 ] value`, conceptually invoked from inside a method: the
    // block's `home` is the enclosing method's own activation, not the
    // block's own (freshly built, by `invoke_closure`) activation.
    let home = method_frame();
    let block_body = parse_block_body(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public withBlock = ( [ ^1 ] )\n) : ()",
    );
    let bindings = Rc::new(HashMap::new());
    let closure = Rc::new(ClosureData {
        params: vec![],
        body: ns_runtime::ClosureBody::Ast(Rc::new(block_body)),
        captured_frame: home.clone(),
        home: home.clone(),
        bindings: bindings.clone(),
    });

    let mut interp = Interpreter::new();
    // No sender: this closure is invoked directly, not from a live send.
    let result = eval::invoke_closure(&mut interp, &closure, vec![], &bindings, None);

    // This is exactly the check `Interpreter::invoke_method` performs to
    // convert a matching `Signal::Return` back into an ordinary value
    // (`Err(Signal::Return(target, value)) if Rc::ptr_eq(&target, &frame)`)
    // -- reproduced here since `home` was hand-built rather than produced
    // by a live `invoke_method` call (which block dispatch would normally
    // supply starting Task 10).
    match result {
        Err(Signal::Return(target, Value::SmallInt(1))) => {
            assert!(
                Rc::ptr_eq(&target, &home),
                "a non-local return must target the block's home *method* activation, not the block's own -- invoke_method's Rc::ptr_eq catch depends on this"
            );
        }
        Ok(_) => {
            panic!("expected the block's `^1` to raise a non-local return, not return normally")
        }
        Err(Signal::Return(_, other)) => {
            panic!("expected Signal::Return(_, SmallInt(1)), got value {other:?}")
        }
        Err(Signal::Error(e)) => {
            panic!("expected Signal::Return, got Signal::Error: {}", e.message)
        }
    }
}

#[test]
fn invoking_an_escaped_block_after_its_home_method_returned_is_a_newspeak_error_not_a_panic() {
    let home = method_frame();
    let block_body = parse_block_body(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public withBlock = ( [ ^1 ] )\n) : ()",
    );
    let bindings = Rc::new(HashMap::new());
    let closure = Rc::new(ClosureData {
        params: vec![],
        body: ns_runtime::ClosureBody::Ast(Rc::new(block_body)),
        captured_frame: home.clone(),
        home: home.clone(),
        bindings: bindings.clone(),
    });

    let mut interp = Interpreter::new();

    // First invocation succeeds and (as `raise_return` does on every
    // successful return) marks `home` `uncontinuable` -- modeling the home
    // method having now returned.
    let first = eval::invoke_closure(&mut interp, &closure, vec![], &bindings, None);
    assert!(
        matches!(first, Err(Signal::Return(ref target, Value::SmallInt(1))) if Rc::ptr_eq(target, &home))
    );
    assert!(
        home.borrow().uncontinuable,
        "a successful non-local return must mark its target uncontinuable"
    );

    // Second invocation of the same (now-escaped) block: its home has
    // already returned, so this must be the spec's `cannotReturn` case --
    // a `NewspeakError`, not another `Signal::Return`, and not a panic.
    let second = eval::invoke_closure(&mut interp, &closure, vec![], &bindings, None);
    match second {
        Err(Signal::Error(NewspeakError { message, .. })) => {
            assert!(
                message.to_lowercase().contains("cannotreturn") || message.to_lowercase().contains("cannot return"),
                "expected a cannotReturn-shaped error message, got: {message}"
            );
        }
        other => panic!(
            "expected Err(Signal::Error(_)) for a re-return through an already-returned home, got a non-matching result ({})",
            match &other {
                Ok(_) => "Ok(_)",
                Err(Signal::Return(..)) => "Err(Signal::Return(_))",
                Err(Signal::Error(_)) => "Err(Signal::Error(_))",
            }
        ),
    }
}

/// Final-review finding I1: `Frame.uncontinuable` used to be set only by
/// `Interpreter::raise_return`, i.e. only when the home method activation
/// itself executed an explicit `^`. A method that returns *normally* (falls
/// off the end) left its frame continuable forever, so a block captured
/// during it and invoked afterwards would raise a `Signal::Return` at a
/// frame whose Rust-level `invoke_method` call had already returned -- with
/// no live catcher left, that escaped all the way out of the entry send and
/// surfaced as the interpreter-invariant-shaped "internal: non-local return
/// escaped the entry send" rather than the spec's `cannotReturn` case.
///
/// `capture` here has no explicit `^` at all: it stores `[^1]` into a slot
/// and falls off the end (returning self). `runIt`, a *later, separate*
/// send, then invokes that now-escaped block.
#[test]
fn a_block_escaping_a_normally_returned_method_is_a_clean_cannot_return_error() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (|b.|) (\n  public capture = ( b:: [^1] )\n  public runIt = ( ^b value )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    let bindings = resolved.bindings.clone();
    let receiver = match interp.instantiate(class, vec![], &bindings) {
        Ok(v) => v,
        Err(Signal::Error(e)) => panic!("instantiate: unexpected Signal::Error: {}", e.message),
        Err(Signal::Return(..)) => panic!("instantiate: unexpected Signal::Return"),
    };

    // `capture` returns normally -- no `^` anywhere in its body.
    match interp.send(
        receiver.clone(),
        Symbol::intern("capture"),
        vec![],
        &bindings,
        None,
    ) {
        Ok(_) => {}
        Err(Signal::Error(e)) => panic!("capture: unexpected Signal::Error: {}", e.message),
        Err(Signal::Return(..)) => panic!("capture: unexpected Signal::Return"),
    }

    // The captured block's home activation is now dead: `^1` inside it must
    // be the spec's `cannotReturn` case, a `NewspeakError` -- never a
    // `Signal::Return` leaking out of `send`, and never a panic.
    match interp.send(receiver, Symbol::intern("runIt"), vec![], &bindings, None) {
        Err(Signal::Error(NewspeakError { message, .. })) => assert!(
            message.to_lowercase().contains("cannotreturn")
                || message.to_lowercase().contains("cannot return"),
            "expected a cannotReturn-shaped error message, got: {message}"
        ),
        Ok(v) => panic!("expected a cannotReturn NewspeakError, got Ok({v:?})"),
        Err(Signal::Return(..)) => panic!(
            "a non-local return into an already-returned home method escaped `send` as a raw \
             Signal::Return -- the home frame was never marked uncontinuable"
        ),
    }
}
