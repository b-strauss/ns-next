//! Interpreter core tests: expression evaluation, ordinary sends, and
//! non-local return, built over hand-assembled `ClassData`/`Object`/`Frame`
//! values -- the same construction pattern `dispatch.rs`'s own test module
//! uses, extended here to actually *run* method bodies rather than
//! only look them up.

use std::collections::HashMap;
use std::rc::Rc;

use ns_runtime::interpreter::eval;
use ns_runtime::{
    Binding, ClassData, ClosureData, Frame, FrameKind, Interpreter, Mixin, MixinApplication,
    NewspeakError, ObjectData, Signal, Symbol, Value,
};

/// A placeholder `ClassRef` whose own `.class` field points back to itself.
/// `Object.class` is non-optional (`ClassRef`, not `Option<ClassRef>`), so
/// constructing any `Object` requires already having a `ClassRef` in hand
/// for its `class` field -- and these test classes are never linked into a
/// real bootstrapped class hierarchy, so there is no other object to point
/// at. None of these tests ever read a class object's own `.class` field,
/// so any non-panicking placeholder is sufficient; a self-referential one
/// is the simplest to build. The construction itself lives in
/// `Heap::alloc_self_referential_class`.
fn dummy_class(heap: &mut ns_runtime::Heap) -> ns_runtime::ClassRef {
    heap.alloc_self_referential_class(ObjectData::Instance(vec![]))
}

/// Parses `src` (a full one-class module) and returns the class's own
/// `Rc<ast::MethodDecl>`s by unary selector name, plus the module's
/// resolved `Rc<HashMap<Span, Binding>>`. Building `MethodDecl`s via a real
/// parse -- rather than hand-constructing deeply nested AST literals --
/// follows `dispatch.rs`'s `class_with_method` helper.
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
/// tests use). Allocated through `interp`'s own heap even though it is
/// never linked into a real hierarchy, so every handle in these tests comes
/// from the one seam production code goes through.
fn class_with_methods(
    interp: &mut Interpreter,
    name: &str,
    methods: Vec<Rc<ns_syntax::ast::MethodDecl>>,
) -> ns_runtime::ClassRef {
    let mut map = HashMap::new();
    for m in methods {
        let method = ns_runtime::mixin::MixinMethod::from_decl(m);
        map.insert(method.selector, Rc::new(method));
    }
    let class_of_class = dummy_class(interp.heap_mut());
    interp.heap_mut().alloc_placeholder_object(
        class_of_class,
        ObjectData::Class(ClassData {
            // A root application, built directly rather than through
            // `Interpreter::apply_mixin`: these classes are never linked
            // into a real hierarchy, so there is no superclass application
            // to cache against.
            application: Rc::new(MixinApplication {
                mixin: Rc::new(Mixin {
                    methods: map,
                    ..Mixin::empty(Symbol::intern(name))
                }),
                superclass: None,
                slot_index: HashMap::new(),
                nested_slot: HashMap::new(),
                num_slots: 0,
                own_slot_base: 0,
            }),
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin: None,
            metaclass: None,
        }),
    )
}

fn instance_of(interp: &mut Interpreter, class: ns_runtime::ClassRef) -> ns_runtime::ObjRef {
    interp
        .heap_mut()
        .alloc_object(class, ObjectData::Instance(vec![]))
}

#[test]
fn method_with_no_explicit_return_returns_self() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public bar = ( 1. 2 )\n) : ()";
    let (methods, bindings) = parse_methods(src);
    let mut interp = Interpreter::new();
    let class = class_with_methods(&mut interp, "Foo", methods);
    let instance = instance_of(&mut interp, class);

    let result = interp.send(
        Value::Obj(instance),
        Symbol::intern("bar"),
        vec![],
        &bindings,
        None,
    );

    match result {
        Ok(Value::Obj(obj)) => assert!(
            obj.same(&instance),
            "a method with no explicit return must yield the receiver (self), not the last expression's value"
        ),
        Ok(other) => panic!(
            "expected Value::Obj(self), got {}",
            interp.heap_ref().describe(&other)
        ),
        Err(Signal::Return(..)) => panic!("unexpected Signal::Return -- `send` should never see one escape `invoke_method`"),
        Err(Signal::Error(e)) => panic!("unexpected Signal::Error: {}", e.message),
    }
}

#[test]
fn method_with_explicit_return_returns_the_returned_value() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public answer = ( ^42 )\n) : ()";
    let (methods, bindings) = parse_methods(src);
    let mut interp = Interpreter::new();
    let class = class_with_methods(&mut interp, "Foo", methods);
    let instance = instance_of(&mut interp, class);

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
            "expected Value::SmallInt(42), got {}",
            interp.heap_ref().describe(&v)
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
/// dispatch (`value`) could not be reached through `Interpreter::send` when
/// these tests were written.
fn method_frame(heap: &mut ns_runtime::Heap) -> ns_runtime::FrameRef {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^0 )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let method = Rc::new(ns_runtime::mixin::MixinMethod::from_decl(Rc::new(
        module.root_class.instance_side.methods[0].clone(),
    )));
    heap.alloc_frame(Frame {
        receiver: Value::Nil,
        args: vec![],
        temps: vec![],
        pc: 0,
        pc_kind: ns_runtime::PcKind::TreeWalk,
        sender: None,
        home: None,
        current_class: None,
        kind: FrameKind::Method(method),
        uncontinuable: false,
        slot_base: 0,
    })
}

/// Sending a method whose body contains a bare
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
    let mut interp = Interpreter::bootstrap();
    let class = class_with_methods(&mut interp, "Foo", methods);
    let instance = instance_of(&mut interp, class);

    let result = interp.send(
        Value::Obj(instance),
        Symbol::intern("bar"),
        vec![],
        &bindings,
        None,
    );

    match result {
        Ok(Value::Obj(obj)) => {
            let elements = interp.heap_ref().array_elements(&obj);
            assert!(
                matches!(&elements, Some(elems) if elems.is_empty()),
                "expected a fresh, empty OrderedCollection instance, got {}",
                interp.heap_ref().describe(&Value::Obj(obj))
            );
        }
        Ok(other) => panic!(
            "expected Value::Obj(<OrderedCollection instance>), got {}",
            interp.heap_ref().describe(&other)
        ),
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
    let mut interp = Interpreter::new();
    let home = method_frame(interp.heap_mut());
    let block_body = parse_block_body(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public withBlock = ( [ ^1 ] )\n) : ()",
    );
    let bindings = Rc::new(HashMap::new());
    let closure = Rc::new(ClosureData {
        params: vec![],
        body: ns_runtime::ClosureBody::Ast(Rc::new(block_body)),
        captured_frame: home,
        home,
        bindings: bindings.clone(),
    });

    // No sender: this closure is invoked directly, not from a live send.
    let result = eval::invoke_closure(&mut interp, &closure, vec![], &bindings, None);

    // This is exactly the check `Interpreter::invoke_method` performs to
    // convert a matching `Signal::Return` back into an ordinary value
    // (`Err(Signal::Return(target, value)) if target.same(&frame)`)
    // -- reproduced here since `home` was hand-built rather than produced
    // by a live `invoke_method` call, which block dispatch would normally
    // supply.
    match result {
        Err(Signal::Return(target, Value::SmallInt(1))) => {
            assert!(
                target.same(&home),
                "a non-local return must target the block's home *method* activation, not the block's own -- invoke_method's FrameRef::same catch depends on this"
            );
        }
        Ok(_) => {
            panic!("expected the block's `^1` to raise a non-local return, not return normally")
        }
        Err(Signal::Return(_, other)) => {
            panic!(
                "expected Signal::Return(_, SmallInt(1)), got value {}",
                interp.heap_ref().describe(&other)
            )
        }
        Err(Signal::Error(e)) => {
            panic!("expected Signal::Return, got Signal::Error: {}", e.message)
        }
    }
}

#[test]
fn invoking_an_escaped_block_after_its_home_method_returned_is_a_newspeak_error_not_a_panic() {
    let mut interp = Interpreter::new();
    let home = method_frame(interp.heap_mut());
    let block_body = parse_block_body(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public withBlock = ( [ ^1 ] )\n) : ()",
    );
    let bindings = Rc::new(HashMap::new());
    let closure = Rc::new(ClosureData {
        params: vec![],
        body: ns_runtime::ClosureBody::Ast(Rc::new(block_body)),
        captured_frame: home,
        home,
        bindings: bindings.clone(),
    });

    // First invocation succeeds and (as `raise_return` does on every
    // successful return) marks `home` `uncontinuable` -- modeling the home
    // method having now returned.
    let first = eval::invoke_closure(&mut interp, &closure, vec![], &bindings, None);
    assert!(
        matches!(first, Err(Signal::Return(ref target, Value::SmallInt(1))) if target.same(&home))
    );
    assert!(
        interp.heap_ref().frame_is_uncontinuable(&home),
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

/// `Frame.uncontinuable` must be set when a method returns *normally*, not
/// only when `Interpreter::raise_return` handles an explicit `^`. A frame
/// left continuable after its Rust-level `invoke_method` call returned has
/// no live catcher, so a block captured during it and invoked afterwards
/// raises a `Signal::Return` that escapes all the way out of the entry send
/// and surfaces as the interpreter-invariant-shaped "internal: non-local
/// return escaped the entry send" rather than the spec's `cannotReturn`
/// case.
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
    match interp.send(receiver, Symbol::intern("capture"), vec![], &bindings, None) {
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
        Ok(v) => panic!(
            "expected a cannotReturn NewspeakError, got Ok({})",
            interp.heap_ref().describe(&v)
        ),
        Err(Signal::Return(..)) => panic!(
            "a non-local return into an already-returned home method escaped `send` as a raw \
             Signal::Return -- the home frame was never marked uncontinuable"
        ),
    }
}
