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
//! result is cached in a hidden slot of `r` itself (`nested_slot`).
//! Behaviour is per mixin application (§6.2), so the class object is built
//! around whatever `Interpreter::apply_mixin` returns, which is shared by
//! every enclosing instance that resolves the clause to the same superclass.
//!
//! This lives under `interpreter/` rather than in `mixin.rs` because it is
//! the one part of nesting that *evaluates Newspeak code*: the superclass
//! clause. That code runs compiled, in an activation of its own, the same
//! way `instantiate.rs` runs a superclass clause's deferred message.

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::heap::{ActivationHeader, Heap};
use crate::mixin::{mixin_key, Mixin};
use crate::types::*;
use crate::Symbol;

use super::{EvalResult, Interpreter, Signal};

/// What a superclass clause evaluates: the `ec` of `newspeak-spec.pdf` §6.1,
/// as an ordinary expression.
///
/// The clause's trailing factory message (`new` in `Object new`) is *not*
/// part of this: `ec` fixes the superclass when the declaration is
/// evaluated, while the message is deferred to every instantiation
/// (`superclass_clause_message` below).
///
/// The resolver already recorded a `Binding` at `clause.receiver.span` for
/// every prefix form (`self`/`super`/`outer X`/bare), computed against the
/// chain of classes *enclosing* the declaration -- so evaluating this
/// expression in a frame whose receiver is the enclosing object is exactly
/// the spec's "as if it occurred inside an instance method of the enclosing
/// class". That is also why this is a plain `ast::Expr` rather than a
/// bespoke evaluation: `compiler::lower`'s identifier lowering already
/// handles every such form, so a clause receiver that is anything other than
/// a builtin class name needs no lowering of its own.
pub(crate) fn superclass_clause_expr(clause: &ast::SuperclassClause) -> ast::Expr {
    ast::Expr::Ident(clause.receiver.clone())
}

/// The clause's deferred factory message -- `newspeak-spec.pdf` §6.1's
/// "message determined by the inheritance clause". `None` means the clause
/// wrote none, which §6.1 reads as `new`; the callers name that default
/// themselves rather than synthesizing a `MessageSend` with no source span.
///
/// `last()` rather than a whole chain because the grammar admits exactly one
/// message here (`NewspeakGrammar.ns`: `superclassClause = superclassPrefix
/// opt , unaryMsg , message opt`, and `message` is a single keyword, binary
/// or unary send). `parser::decl::parse_trailing_sends` is more permissive
/// than that -- it loops `unary* binary* keyword?` -- but a clause it accepts
/// with more than one message already fails earlier, when `ec` (the receiver
/// alone, above) turns out not to be a class object.
pub(crate) fn superclass_clause_message(
    clause: &ast::SuperclassClause,
) -> Option<&ast::MessageSend> {
    clause.messages.last()
}

/// The clause message's argument list as one expression: `{a. b. c}`, whose
/// value is an array of the evaluated arguments.
///
/// One expression rather than one per argument so that a mixin has exactly
/// one compiled message body -- `image::decode::frame_body` must be able to
/// name the single body a frame's kind runs, and a per-argument body would
/// leave it choosing between several. `None` for a message with no arguments
/// (`Object new`, and every clause in the corpus but a handful): nothing is
/// compiled, no activation is built, and nothing is allocated.
pub(crate) fn superclass_clause_message_args_expr(
    clause: &ast::SuperclassClause,
) -> Option<ast::Expr> {
    let message = superclass_clause_message(clause)?;
    if message.args.is_empty() {
        return None;
    }
    Some(ast::Expr::DynamicArray {
        elements: message.args.clone(),
        span: message.span,
    })
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
    let Some((obj, index)) = cache_slot(interp, receiver, nested) else {
        return Err(no_cache_slot_error(receiver, nested.name));
    };
    // Step 1: the receiver's own cache. `Nil` is the "not yet" marker --
    // `instantiate` fills every slot, hidden ones included, with `Nil`, and
    // a materialized class object is never `Nil`, so no separate occupancy
    // flag is needed.
    if let Some(cached) = read_cache_slot(&interp.heap, receiver, index) {
        return Ok(cached);
    }
    // Step 2: the superclass clause, evaluated in the receiver's context --
    // guarded against a class whose own creation is already underway on
    // this receiver.
    let key = in_progress_key(&interp.heap, &obj, nested);
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
    let Some(super_application) = interp.heap.class_application(&superclass) else {
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
    // `declaring_class` records the level this declaration is written in,
    // which is what `dispatch::kth_enclosing_object` steps out along; unlike
    // the superclass chain it does not follow inheritance. `obj`, from
    // `cache_slot` above, already *is* the enclosing object -- this function
    // only ever reaches here for a receiver `cache_slot` matched as one.
    let class = interp.new_class_object(
        application,
        Some(obj),
        Some(*declaring_class),
        Some(superclass),
    );
    // Step 5: cache before returning, so step 1 hits from now on.
    crate::interpreter::eval::store_instance_field(
        &mut interp.heap,
        receiver,
        index,
        Value::Obj(class),
    );
    Ok(Value::Obj(class))
}

/// `receiver`'s own object handle together with the absolute index of
/// `nested`'s hidden cache slot in its layout, or `None` when `receiver`
/// has no such slot -- either because its class's application does not
/// know the declaration (an internal inconsistency) or because its payload
/// is not an `Instance` at all, which a user class inheriting a builtin's
/// representation (`OrderedCollection`, `Array`) really is:
/// `try_factory_send` gives those an `ObjectData::Array` payload with no
/// fields to cache into. Handing back the handle, not just the index,
/// means every caller downstream already holds proof that `receiver` is an
/// object instead of re-deriving it from `receiver` a second time --
/// `in_progress_key` below is exactly that: it takes an `ObjRef`, not a
/// `Value`, so a non-object receiver cannot reach it at all.
///
/// The declaration itself, not its name: `MixinApplication::nested_slot` is
/// keyed by declaration identity so that a subclass's `class N` and the
/// inherited `class N` it shadows cache into different slots.
fn cache_slot(
    interp: &Interpreter,
    receiver: &Value,
    nested: &Rc<Mixin>,
) -> Option<(ObjRef, usize)> {
    let class = interp.class_of(receiver);
    let index = interp
        .heap
        .class_application(&class)?
        .nested_slot
        .get(&mixin_key(nested))
        .copied()?;
    match receiver {
        Value::Obj(obj) => interp
            .heap
            .instance_field(obj, index)
            .map(|_| (*obj, index)),
        _ => None,
    }
}

/// The key `Interpreter::nested_in_progress` records while a declaration's
/// class object is being created for `obj`: its arena index plus the
/// declaration's identity. The index is a safe key for as long as `heap`
/// exists, not merely for the duration of this stack entry: the
/// arena never reuses a slot (there is no collector yet), so no other
/// object -- live or not -- can ever carry the same index. Taking `&ObjRef`
/// rather than `&Value` is what makes that the whole of this function's
/// contract: there is no "receiver is not an object" case to key at all,
/// so no sentinel is needed and none could collide with a real entry.
/// `cache_slot` is the only caller, and it only ever hands back the object
/// it already matched `receiver` against.
///
/// The declaration, not its name, for the same reason `cache_slot` uses it:
/// `class N = super N (...)` is a subclass redeclaring what it inherits,
/// not a cycle, and a name key would report the legal shape as one.
fn in_progress_key(heap: &Heap, obj: &ObjRef, nested: &Rc<Mixin>) -> (usize, usize) {
    (heap.object_identity_key(obj), mixin_key(nested))
}

fn read_cache_slot(heap: &Heap, receiver: &Value, index: usize) -> Option<Value> {
    let Value::Obj(obj) = receiver else {
        return None;
    };
    match heap.instance_field(obj, index) {
        Some(Value::Nil) | None => None,
        Some(value) => Some(value),
    }
}

/// The superclass clause is Newspeak code: the body
/// `compiler::compile_superclass_clause` emitted for this mixin runs in a
/// fresh activation.
///
/// The clause belongs to the class that *declares* it: it is resolved
/// against that declaration's enclosing chain, and `super`-prefixed forms
/// look up from that class's superclass -- which is why the activation's
/// class is the declaring class rather than the receiver's own (dynamic)
/// class, the two differing exactly when the declaration is inherited.
fn eval_superclass_clause(
    interp: &mut Interpreter,
    receiver: &Value,
    nested: &Rc<Mixin>,
    declaring_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    // Whether there is a clause at all is read off the mixin, not off its
    // declaration: a mixin decoded from an image has no declaration, and the
    // answer decides whether any code runs.
    if nested.superclass.is_none() {
        // No clause at all (`class N = (...)`): `Object`, the same default
        // `instantiate::resolve_superclass_receiver` gives a root class.
        return Ok(Value::Obj(interp.builtin_class(BuiltinClass::Object)));
    }
    let code = interp
        .code
        .superclass_clauses
        .get(&mixin_key(nested))
        .cloned()
        .unwrap_or_else(|| {
            panic!("internal: mixin with a superclass clause has no compiled form for it (compile_module coverage gap)")
        });
    run_clause_body(
        interp,
        receiver,
        vec![],
        declaring_class,
        FrameKind::SuperclassClause(*declaring_class),
        code,
        bindings,
    )
}

/// One per-mixin clause body, compiled, in a fresh activation. Everything
/// around the body -- the activation, the receiver, the `Signal::Return`
/// catch -- is the same shape as `Interpreter::invoke_method`'s.
///
/// Shared with `instantiate`, which runs the same clause's *deferred
/// message arguments* through it one level down. The two differ only in
/// what they hand in: the receiver (the enclosing object vs. the instance
/// being initialized), the activation's arguments (none vs. the level's
/// factory arguments) and the frame kind naming which table the body came
/// from -- `image::decode::frame_body` resolves a frame's body by that
/// kind, so a clause activation must never claim to be an initializer one.
///
/// Both kinds are return targets for `home_of`/`raise_return`, so a `^`
/// inside a block written in the clause unwinds to this activation rather
/// than escaping into the caller.
pub(super) fn run_clause_body(
    interp: &mut Interpreter,
    receiver: &Value,
    args: Vec<Value>,
    current_class: &ClassRef,
    kind: FrameKind,
    code: Rc<crate::bytecode::CompiledCode>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let act = interp.heap.push_activation(
        ActivationHeader {
            receiver: *receiver,
            current_class: Some(*current_class),
            kind,
            slot_base: 0, // no clause body ever contains InitSlot
        },
        &args,
        (code.num_registers - code.num_args) as usize,
    );
    let result = crate::vm::run_code(interp, &act, &code, bindings);
    interp.heap.pop_activation(&act);
    match result {
        Ok(value) => Ok(value),
        Err(Signal::Return(target, value)) if target.same(&act) => Ok(value),
        Err(other) => Err(other),
    }
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if ec does not evaluate
/// to a class object C." A program error, raised through the ordinary
/// `NewspeakError` channel.
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
        receiver: *receiver,
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
        receiver: *receiver,
        message: format!(
            "cannot access nested class {name}: this object has no slot storage to cache it in"
        ),
    })
}
