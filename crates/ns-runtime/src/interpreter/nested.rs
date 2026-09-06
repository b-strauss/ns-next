//! The implicit nested-class accessor: `newspeak-spec.pdf` §6.
//!
//! "A declaration of a nested class N nested directly within another class OC
//! implicitly defines a method with the same name and accessibility in OC.
//! The first time this method is invoked on a given receiver r, it evaluates
//! the class declaration, using r as the enclosing object, and returns the
//! resulting class as its result. Subsequent calls on r always return the
//! same class object."
//!
//! Two consequences shape this module. Identity is per receiver -- "each
//! nested class has one piece of state, its enclosing object, and so there
//! must be a distinct class per instance of the enclosing class" -- so the
//! result is cached in a hidden slot of `r` itself (`nested_slot`, Task 4).
//! Behaviour is per mixin application (§6.2), so the class object is built
//! around whatever `Interpreter::apply_mixin` returns, which is shared by
//! every enclosing instance that resolves the clause to the same superclass.
//!
//! This lives under `interpreter/` rather than in `mixin.rs` because it is
//! the one part of nesting that *evaluates Newspeak code*: the superclass
//! clause. That code runs through the same engine hook `instantiate.rs` uses
//! for instance initializers, so `--engine=bytecode` never tree-walks it.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::mixin::{mixin_key, Mixin};
use crate::types::*;
use crate::Symbol;

use super::{Engine, EvalResult, Interpreter, Signal};

/// What a superclass clause evaluates: the `ec` of `newspeak-spec.pdf` §6.1,
/// as an ordinary expression.
///
/// The clause's trailing factory message (`new` in `Object new`) is *not*
/// part of this -- see `instantiate::run_superclass_initializer`'s known-gaps
/// note; Task 7 is what evaluates it, and it will do so here so that both
/// engines keep getting it from one place.
///
/// The resolver already recorded a `Binding` at `clause.receiver.span` for
/// every prefix form (`self`/`super`/`outer X`/bare), computed against the
/// chain of classes *enclosing* the declaration -- so evaluating this
/// expression in a frame whose receiver is the enclosing object is exactly
/// the spec's "as if it occurred inside an instance method of the enclosing
/// class". That is also why this is a plain `ast::Expr` rather than a
/// bespoke evaluation: `eval::eval_ident` and `compiler::lower`'s identifier
/// lowering are already a matched pair, and reusing them is what keeps the
/// two engines from drifting on a clause receiver that is anything other
/// than a builtin class name.
pub(crate) fn superclass_clause_expr(clause: &ast::SuperclassClause) -> ast::Expr {
    ast::Expr::Ident(clause.receiver.clone())
}

/// The implicit accessor for `nested` on `receiver`. `receiver` is the
/// enclosing object `r`; `nested` is the declaration `dispatch` found on
/// `r`'s class chain; `declaring_class` is the class object of the level
/// that declares it -- the `ClassRef` `Lookup::NestedClass` carries.
///
/// `declaring_class` is *not* `r`'s own class whenever the declaration is
/// inherited, and the difference is observable: the clause is lexically
/// part of the declaring class, so a `super`-prefixed clause receiver
/// (`resolver::resolve_superclass_clause` binds it as `Binding::SuperSend`)
/// must start its lookup at the declaring class's superclass, not at the
/// receiver's dynamic class's superclass -- which for an inherited
/// declaration is one or more levels too deep.
pub(crate) fn nested_class(
    interp: &mut Interpreter,
    receiver: &Value,
    nested: &Rc<Mixin>,
    declaring_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let Some(index) = cache_slot(interp, receiver, nested.name) else {
        return Err(no_cache_slot_error(receiver, nested.name));
    };
    // Step 1: the receiver's own cache. `Nil` is the "not yet" marker --
    // `instantiate` fills every slot, hidden ones included, with `Nil`, and
    // a materialized class object is never `Nil`, so no separate occupancy
    // flag is needed.
    if let Some(cached) = read_cache_slot(receiver, index) {
        return Ok(cached);
    }
    // Step 2: the superclass clause, evaluated in the receiver's context
    // through the engine hook -- guarded against a class whose own creation
    // is already underway on this receiver.
    let key = in_progress_key(receiver, nested.name);
    if interp.nested_in_progress.contains(&key) {
        return Err(cyclic_definition_error(receiver, nested.name));
    }
    interp.nested_in_progress.push(key);
    let evaluated = eval_superclass_clause(interp, receiver, nested, declaring_class, bindings);
    interp.nested_in_progress.pop();
    let superclass = evaluated?;
    let Value::Obj(superclass) = superclass else {
        return Err(not_a_class_error(nested.name, superclass));
    };
    let Some(super_application) = crate::dispatch::class_application(&superclass) else {
        return Err(not_a_class_error(nested.name, Value::Obj(superclass)));
    };
    // Step 3: the application cache decides whether this behaviour already
    // exists. Two enclosing instances whose clauses land on the same
    // superclass application share one application, and therefore one
    // compiled body and one instance layout (§6.2).
    let application = interp.apply_mixin(nested, Some(&super_application));
    // Step 4: identity is per receiver. The class object's superclass chain
    // and its application's superclass chain are linked together here --
    // `dispatch::next_level` steps both and stops if either is missing.
    let enclosing = match receiver {
        Value::Obj(obj) => Some(obj.clone()),
        // Unreachable in practice: `cache_slot` above already established
        // that `receiver` is an object with an instance layout. Kept total
        // rather than unwrapped so a future caller cannot turn a changed
        // precondition into a panic.
        _ => None,
    };
    let class = interp.new_class_object(application, enclosing, Some(superclass));
    // Step 5: cache before returning, so step 1 hits from now on.
    crate::interpreter::eval::store_instance_field(receiver, index, Value::Obj(class.clone()));
    Ok(Value::Obj(class))
}

/// The absolute index of `name`'s hidden cache slot in `receiver`'s own
/// layout, or `None` when `receiver` has no such slot -- either because its
/// class's application does not know the declaration (an internal
/// inconsistency) or because its payload is not an `Instance` at all, which
/// a user class inheriting a builtin's representation (`OrderedCollection`,
/// `Array`) really is: `try_factory_send` gives those an `ObjectData::Array`
/// payload with no fields to cache into.
fn cache_slot(interp: &Interpreter, receiver: &Value, name: Symbol) -> Option<usize> {
    let class = interp.class_of(receiver);
    let index = crate::dispatch::class_application(&class)?
        .nested_slot
        .get(&name)
        .copied()?;
    match receiver {
        Value::Obj(obj) => match &obj.borrow().data {
            ObjectData::Instance(fields) if index < fields.len() => Some(index),
            _ => None,
        },
        _ => None,
    }
}

/// The key `Interpreter::nested_in_progress` records while a declaration's
/// class object is being created for a receiver: the receiver's address
/// plus the declaration's name. The address is a safe key for the duration
/// -- `receiver` is a live borrow held across the whole evaluation, so the
/// object cannot be freed and its address cannot be reused while the entry
/// is on the stack.
fn in_progress_key(receiver: &Value, name: Symbol) -> (usize, Symbol) {
    let address = match receiver {
        Value::Obj(obj) => Rc::as_ptr(obj) as *const () as usize,
        // `cache_slot` has already established that `receiver` is an object;
        // 0 is an address no live `Rc` has, so a non-object receiver simply
        // never collides with a real entry.
        _ => 0,
    };
    (address, name)
}

fn read_cache_slot(receiver: &Value, index: usize) -> Option<Value> {
    let Value::Obj(obj) = receiver else {
        return None;
    };
    let borrowed = obj.borrow();
    let ObjectData::Instance(fields) = &borrowed.data else {
        return None;
    };
    match fields.get(index) {
        Some(Value::Nil) | None => None,
        Some(value) => Some(value.clone()),
    }
}

/// Engine hook: the superclass clause is Newspeak code, so the tree-walker
/// evaluates its AST and the bytecode engine runs the body
/// `compiler::compile_superclass_clause` emitted for this mixin. Everything
/// around the body -- the activation, the receiver, the `Signal::Return`
/// catch -- is shared, exactly as in `Interpreter::invoke_method` and
/// `run_own_initializer`.
fn eval_superclass_clause(
    interp: &mut Interpreter,
    receiver: &Value,
    nested: &Rc<Mixin>,
    declaring_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let clause = nested
        .decl
        .as_ref()
        .and_then(|decl| decl.superclass.as_ref());
    let Some(clause) = clause else {
        // No clause at all (`class N = (...)`): `Object`, the same default
        // `instantiate::resolve_superclass_receiver` gives a root class.
        // Neither engine runs any code for this, so neither can diverge.
        return Ok(Value::Obj(interp.builtin_class(BuiltinClass::Object)));
    };
    let compiled = match &interp.engine {
        Engine::TreeWalk => None,
        Engine::Bytecode(module) => Some(
            module
                .superclass_clauses
                .get(&mixin_key(nested))
                .cloned()
                .unwrap_or_else(|| {
                    panic!("internal: mixin with a superclass clause has no compiled form for it (compile_module coverage gap)")
                }),
        ),
    };
    // The clause belongs to the class that *declares* it: it is resolved
    // against that declaration's enclosing chain, and `super`-prefixed
    // forms look up from that class's superclass -- which is why
    // `current_class` is the declaring class rather than the receiver's own
    // (dynamic) class, the two differing exactly when the declaration is
    // inherited. `FrameKind::Initializer` is the frame shape for "code of a
    // class declaration running on an instance of it", which is what this
    // is -- and it is the kind `home_of`/`raise_return` already treat as a
    // return target, so a `^` inside a block argument of the clause unwinds
    // here rather than escaping the accessor.
    let temp_count = compiled
        .as_ref()
        .map_or(0, |code| (code.num_registers - code.num_args) as usize);
    let frame = Rc::new(std::cell::RefCell::new(Frame {
        receiver: receiver.clone(),
        args: vec![],
        temps: vec![Value::Nil; temp_count],
        pc: 0,
        sender: None,
        home: None,
        current_class: Some(declaring_class.clone()),
        kind: FrameKind::Initializer(declaring_class.clone()),
        uncontinuable: false,
        slot_base: 0, // a superclass clause never contains InitSlot
    }));
    let result = match &compiled {
        Some(code) => crate::vm::run_code(interp, &frame, code, bindings),
        None => {
            let expr = superclass_clause_expr(clause);
            crate::interpreter::eval::eval_expr(interp, &expr, &frame, bindings)
        }
    };
    frame.borrow_mut().uncontinuable = true;
    match result {
        Ok(value) => Ok(value),
        Err(Signal::Return(target, value)) if Rc::ptr_eq(&target, &frame) => Ok(value),
        Err(other) => Err(other),
    }
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if ec does not evaluate
/// to a class object C." A program error, raised through the ordinary
/// `NewspeakError` channel from shared orchestration, so both engines print
/// the identical stderr.
fn not_a_class_error(name: Symbol, value: Value) -> Signal {
    Signal::Error(NewspeakError {
        selector: name,
        receiver: value,
        message: format!(
            "the superclass clause of class {name} did not evaluate to a class object"
        ),
    })
}

/// A class whose own creation depends on itself: `class N = N new (...)`,
/// or any longer cycle through other nested declarations of the same
/// receiver. §6 makes the accessor evaluate the declaration, so a clause
/// naming the class being created re-enters the accessor before the cache
/// slot is filled and would otherwise recurse until the stack is gone --
/// an abort, not a diagnosable error.
///
/// This is a cycle in class *creation*, not general user recursion: the
/// creation of one class object for one receiver happens exactly once, so a
/// second entry for the same (receiver, declaration) is unconditionally a
/// cycle. `Interpreter::nested_in_progress` is the stack that detects it in
/// O(depth) with no allocation on the ordinary path -- it is empty except
/// while a clause is actually being evaluated.
fn cyclic_definition_error(receiver: &Value, name: Symbol) -> Signal {
    Signal::Error(NewspeakError {
        selector: name,
        receiver: receiver.clone(),
        message: format!(
            "cyclic class definition: evaluating the superclass clause of class {name} \
             requires class {name} itself"
        ),
    })
}

/// A receiver with no hidden slot to cache the nested class in. Reachable
/// from a real program: a class inheriting `OrderedCollection`'s or
/// `Array`'s payload representation gets an `ObjectData::Array` instance,
/// which has no fields -- the same Milestone 1 gap that already makes an
/// ordinary slot declared on such a class unwritable. Refusing here is what
/// keeps §6's "subsequent calls on r always return the same class object"
/// from silently becoming "a fresh class object every call".
fn no_cache_slot_error(receiver: &Value, name: Symbol) -> Signal {
    Signal::Error(NewspeakError {
        selector: name,
        receiver: receiver.clone(),
        message: format!(
            "cannot access nested class {name}: this object has no slot storage to cache it in"
        ),
    })
}
