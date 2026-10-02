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
//! around whatever `Mutator::apply_mixin` returns, which is shared by
//! every enclosing instance that resolves the clause to the same superclass.
//!
//! This lives under `interpreter/` rather than in `mixin.rs` because it is
//! the one part of nesting that *evaluates Newspeak code*: the superclass
//! clause. That code runs compiled, in an activation of its own -- a
//! `Step::SuperclassClause` continuing with `Continuation::NestedClass`, as
//! `instantiate.rs` runs a clause's deferred message as a
//! `Step::ClauseMessage` continuing with `Continuation::Instantiate`.

use std::rc::Rc;

use ns_syntax::ast;

use crate::mixin::{mixin_key, Mixin, MixinApplication};
use crate::types::*;
use crate::Symbol;

use super::{Mutator, Signal, Step};

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
/// class". That is also why this is a plain `ast::Expression` rather than a
/// bespoke evaluation: `compiler::lower`'s identifier lowering already
/// handles every such form, so a clause receiver that is anything other than
/// a builtin class name needs no lowering of its own.
///
/// A declaration that applies mixins (§6.2) evaluates each `<:` clause's
/// receiver in the same place and by the same rule -- §6.2 builds each clause
/// from §6.1's very `inheritanceClause` production -- so they are evaluated
/// here too, left to right after the superclass's, and the body answers
/// `{superclass. mixin. mixin}` instead: one activation fixes every class the
/// declaration's levels are built from, and `after_clause` unpacks it. A
/// declaration without `<:` answers the bare class, as it always has.
pub(crate) fn superclass_clause_expression(
    clause: &ast::SuperclassClause,
    mixin_applications: &[ast::SuperclassClause],
) -> ast::Expression {
    let receiver =
        |clause: &ast::SuperclassClause| ast::Expression::Identifier(clause.receiver.clone());
    if mixin_applications.is_empty() {
        return receiver(clause);
    }
    ast::Expression::DynamicArray {
        elements: std::iter::once(clause)
            .chain(mixin_applications)
            .map(receiver)
            .collect(),
        span: clause.span,
    }
}

/// The clause's deferred factory message -- `newspeak-spec.pdf` §6.1's
/// "message determined by the inheritance clause". `None` means the clause
/// wrote none, which §6.1 reads as `new`; the callers name that default
/// themselves rather than synthesizing a `MessageSend` with no source span.
///
/// `last()` rather than a whole chain because the grammar admits exactly one
/// message here (`NewspeakGrammar.ns`: `superclassClause = superclassPrefix
/// opt , unaryMsg , message opt`, and `message` is a single keyword, binary
/// or unary send). `parser::declaration::parse_trailing_sends` is more permissive
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
pub(crate) fn superclass_clause_message_arguments_expression(
    clause: &ast::SuperclassClause,
) -> Option<ast::Expression> {
    let message = superclass_clause_message(clause)?;
    if message.arguments.is_empty() {
        return None;
    }
    Some(ast::Expression::DynamicArray {
        elements: message.arguments.clone(),
        span: message.span,
    })
}

/// The implicit accessor for `nested` on `receiver`. `receiver` is the
/// enclosing object `r`; `nested` is the declaration `dispatch` found on
/// `r`'s class chain; `declaring_class` is the class object of the level
/// that declares it -- the `ClassReference` `Lookup::NestedClass` carries.
///
/// Decides the accessor without running anything: `receiver`'s own cache if
/// it is already filled, or -- for a declaration with no clause at all --
/// `Object` as a value outright, and otherwise the clause's activation, with
/// steps 3-5 in its continuation (`after_clause`).
///
/// `declaring_class` is *not* `r`'s own class whenever the declaration is
/// inherited, and the difference is observable: the clause is lexically
/// part of the declaring class, so a `super`-prefixed clause receiver
/// (`resolver::resolve_superclass_clause` binds it as `Binding::SuperSend`)
/// must start its lookup at the declaring class's superclass, not at the
/// receiver's dynamic class's superclass -- which for an inherited
/// declaration is one or more levels too deep.
pub(crate) fn nested_class<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    nested: &Rc<Mixin>,
    declaring_class: ClassReference<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let Some((receiver_object, index, cached)) = cache_slot(mutator, receiver, nested) else {
        return Err(no_cache_slot_error(receiver, nested.name));
    };
    // Step 1: the receiver's own cache. `Nil` is "not yet" -- `instantiate`
    // fills every slot, hidden ones included, with `Nil`, and a materialized
    // class object is never `Nil` -- and `creation_in_progress()` is "underway,
    // further down the stack", so the slot answers both questions and no
    // side table is needed.
    match cached {
        cached if cached.is_nil() => {}
        cached if is_creation_in_progress(cached) => {
            return Err(cyclic_definition_error(receiver, nested.name));
        }
        cached => return Ok(Step::Value(cached)),
    }
    // Step 2: the superclass clause, evaluated in the receiver's context.
    let creation = NestedClassCreation {
        enclosing: receiver_object,
        index,
        nested: nested.clone(),
        declaring_class,
    };
    // Whether there is a clause at all is read off the mixin, not off its
    // declaration: a mixin decoded from an image has no declaration, and the
    // answer decides whether any code runs.
    if nested.superclass.is_none() {
        // No clause at all (`class N = (...)`): `Object`, the same default
        // `Mutator::load_root_class` gives a root class.
        let object = Value::object(mutator.builtin_class(BuiltinClass::Object));
        return after_clause(mutator, creation, object);
    }
    Ok(Step::SuperclassClause(Box::new(creation)))
}

/// The compiled superclass clause of `nested`, a declaration with a clause.
///
/// Its activation (`Mutator::push_step`) runs as part of the class that
/// *declares* it: the clause is resolved against that declaration's
/// enclosing chain, and `super`-prefixed forms look up from that class's
/// superclass -- which is why the activation's class is the declaring class
/// rather than the receiver's own (dynamic) class, the two differing exactly
/// when the declaration is inherited. Its kind, `FrameKind::SuperclassClause`,
/// names the table its body came from, which `image::decode::frame_body`
/// resolves a frame's body by, and makes it a return target
/// (`Mutator::catches_return`).
pub(super) fn superclass_clause_code(nested: &Mixin) -> Rc<crate::bytecode::CompiledCode> {
    nested.superclass_clause_code.clone().unwrap_or_else(|| {
        panic!("internal: mixin with a superclass clause carries no compiled form for it (a mixin built from source always does; verify_mixins refuses a decoded one)")
    })
}

/// Steps 3-5, once the superclass clause has evaluated to `superclass`:
/// `Mutator::resume` calls this when the clause's activation returns, and
/// `nested_class` directly for a declaration with no clause. By value, not
/// boxed: only the continuation needs the box.
pub(crate) fn after_clause<'gc>(
    mutator: &mut Mutator<'gc>,
    creation: NestedClassCreation<'gc>,
    superclass: Value<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let NestedClassCreation {
        enclosing,
        index,
        nested,
        declaring_class,
    } = creation;
    // A declaration that applies mixins builds its levels out of line, so a
    // declaration without `<:` takes only the two tests it always took.
    let (superclass, super_application) = if nested.mixin_applications.is_empty() {
        let Some(superclass) = superclass.as_object() else {
            return Err(not_a_class_error(nested.name, superclass));
        };
        let Some(super_application) = mutator.heap.class_application(superclass) else {
            return Err(not_a_class_error(nested.name, Value::object(superclass)));
        };
        (superclass, super_application)
    } else {
        superclass_with_applied_levels(mutator, &nested, superclass)?
    };
    // Step 3: the application cache decides whether this behaviour already
    // exists. Two enclosing instances whose clauses land on the same
    // superclass application share one application, and therefore one
    // compiled body and one instance layout (§6.2).
    let application = mutator.apply_mixin(&nested, Some(&super_application));
    // §3.1.1's enclosing-object condition, asked once, where this class
    // object comes into being; a class that is no value pays this one
    // test. Only *decided* here -- `decide_instantiation` is what refuses,
    // once the verdict is recorded on the class object below
    // (`value_class.rs`'s doc comment says why). The other class-level
    // condition, no identity method, is already folded into `application`'s
    // own `instantiation_refusal` by `apply_mixin` above, so it needs no
    // call here.
    let value_class_refusal = if application.inherits_value {
        crate::interpreter::value_class::check_value_class(mutator, enclosing, superclass)
    } else {
        None
    };
    // Step 4: identity is per receiver. The class object's superclass chain
    // and its application's superclass chain are linked together here --
    // `dispatch::next_level` steps both and stops if either is missing.
    // `declaring_class` records the level this declaration is written in,
    // which is what `dispatch::kth_enclosing_object` steps out along; unlike
    // the superclass chain it does not follow inheritance.
    let class = mutator.new_class_object(
        application,
        Some(enclosing),
        Some(declaring_class),
        Some(superclass),
    );
    if let Some(refusal) = value_class_refusal {
        mutator.heap.set_class_value_refusal(class, refusal);
    }
    // Step 5: cache before answering, so step 1 hits from now on.
    crate::interpreter::locals::store_instance_field(
        &mut mutator.heap,
        Value::object(enclosing),
        index,
        Value::object(class),
    );
    Ok(Step::Value(Value::object(class)))
}

/// The class a declaration that applies mixins builds its own class on, and
/// that class's application: §6.2's levels, one per `<:` clause, stacked on
/// the superclass `value` names, the declaration's own class going on top of
/// the last. `value` is the superclass clause activation's answer.
#[cold]
#[inline(never)]
fn superclass_with_applied_levels<'gc>(
    mutator: &mut Mutator<'gc>,
    nested: &Rc<Mixin>,
    value: Value<'gc>,
) -> Result<(ClassReference<'gc>, Rc<MixinApplication>), Signal<'gc>> {
    let (superclass, applied) = unpack_clause_value(mutator, nested, value)?;
    let Some(superclass) = superclass.as_object() else {
        return Err(not_a_class_error(nested.name, superclass));
    };
    let Some(super_application) = mutator.heap.class_application(superclass) else {
        return Err(not_a_class_error(nested.name, Value::object(superclass)));
    };
    let applied: Vec<(usize, Value<'gc>)> = applied.into_iter().enumerate().collect();
    let top = build_applied_levels(
        mutator,
        nested,
        Some((superclass, super_application)),
        &applied,
    )?;
    Ok(top.expect("internal: build_applied_levels answers a level over a superclass"))
}

/// What the superclass clause's activation answered, split into the
/// superclass and the classes the `<:` clauses named, in clause order: the
/// value itself and none for a declaration without `<:`, and otherwise the
/// `{superclass. mixin. mixin}` `superclass_clause_expression` builds.
///
/// A value of any other shape is refused as the superclass clause not
/// answering a class, never indexed past: a decoded clause body is the
/// image's to write.
fn unpack_clause_value<'gc>(
    mutator: &Mutator<'gc>,
    nested: &Mixin,
    value: Value<'gc>,
) -> Result<(Value<'gc>, Vec<Value<'gc>>), Signal<'gc>> {
    if nested.mixin_applications.is_empty() {
        return Ok((value, Vec::new()));
    }
    let elements = match value.as_object() {
        Some(object) => mutator.heap.array_elements(object),
        _ => None,
    };
    match elements {
        Some(mut elements) if elements.len() == 1 + nested.mixin_applications.len() => {
            let applied = elements.split_off(1);
            Ok((elements[0], applied))
        }
        _ => Err(not_a_class_error(nested.name, value)),
    }
}

/// The levels §6.2 adds between `declaration`'s own class and `superclass`,
/// built from `applied`: each `<:` clause's index into
/// `declaration.mixin_applications` with the class its receiver evaluated to,
/// in clause order. Answers the top level -- the superclass the
/// declaration's own class is built on -- which is `superclass` itself when
/// `applied` is empty. Each class travels with its application, so that
/// neither the caller nor the next level looks up one already in hand.
///
/// The one place a level is built, for a module's root class
/// (`Mutator::load_root_class`) and for a nested one (`after_clause`) alike.
/// Each level is an ordinary class object: the applied class's mixin applied
/// to the level below (§6.2: "a class *I* with mixin *M* and superclass
/// *S*"), so lookup, `super` and the instantiation walk step through it like
/// any other. Its enclosing object is the applied class's -- "the enclosing
/// object of *I* is the enclosing object of *C*" -- and so is its declaring
/// class, the class `outer` continues from past that first hop: a method of
/// the applied mixin was written where the applied class is declared, and
/// resolves `outer` from there, not from the declaration applying it.
///
/// Only the named class's own mixin is applied, not its superclass chain:
/// §6.2 applies *C*'s mixin *M*, and a class's mixin is what its own
/// declaration contributes.
pub(crate) fn build_applied_levels<'gc>(
    mutator: &mut Mutator<'gc>,
    declaration: &Rc<Mixin>,
    superclass: Option<(ClassReference<'gc>, Rc<MixinApplication>)>,
    applied: &[(usize, Value<'gc>)],
) -> Result<Option<(ClassReference<'gc>, Rc<MixinApplication>)>, Signal<'gc>> {
    let mut below = superclass;
    for &(index, applied_class) in applied {
        assert!(
            index < declaration.mixin_applications.len(),
            "internal: a caller indexes the clauses it read off this mixin"
        );
        let named = match applied_class.as_object() {
            Some(object) => mutator
                .heap
                .class_application(object)
                .map(|application| (object, application)),
            _ => None,
        };
        // §6.2's clause is §6.1's `inheritanceClause`, and §6.1: "It is a run
        // time error if ec does not evaluate to a class object C."
        let Some((named_class, named_application)) = named else {
            return Err(not_a_mixin_error(declaration.name, index, applied_class));
        };
        let (below_class, below_application) = match below {
            Some((class, application)) => (Some(class), Some(application)),
            None => (None, None),
        };
        let application = mutator.apply_mixin(&named_application.mixin, below_application.as_ref());
        let enclosing = mutator.heap.class_enclosing(named_class);
        let declaring_class = mutator.heap.class_declaring(named_class);
        let clause = crate::mixin::AppliedClause {
            declaration: declaration.clone(),
            index,
            named_builtin: mutator.heap.class_builtin(named_class),
        };
        let level = mutator.new_applied_level(
            application.clone(),
            enclosing,
            declaring_class,
            below_class,
            clause,
        );
        below = Some((level, application));
    }
    Ok(below)
}

/// `receiver`'s own object handle together with the absolute index of
/// `nested`'s hidden cache slot in its layout and what that slot holds, or
/// `None` when `receiver`
/// has no such slot -- either because its class's application does not
/// know the declaration (an internal inconsistency) or because its payload
/// is not an `Instance` at all, which a user class inheriting `Array`'s
/// representation really is:
/// `decide_factory` gives those an `ObjectData::Array` payload with no
/// fields to cache into. Handing back the handle, not just the index,
/// means every caller downstream already holds proof that `receiver` is an
/// object instead of re-deriving it from `receiver` a second time --
/// `NestedClassCreation` is exactly that: it holds an `ObjectReference`, not a
/// `Value`, so a non-object receiver cannot reach it at all.
///
/// The declaration itself, not its name: `MixinApplication::nested_slot` is
/// keyed by declaration identity so that a subclass's `class N` and the
/// inherited `class N` it shadows cache into different slots.
fn cache_slot<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    nested: &Rc<Mixin>,
) -> Option<(ObjectReference<'gc>, usize, Value<'gc>)> {
    let class = mutator.class_of(receiver);
    let index = mutator
        .heap
        .class_application(class)?
        .nested_slot
        .get(&mixin_key(nested))
        .copied()?;
    match receiver.as_object() {
        Some(object) => mutator
            .heap
            .instance_field(object, index)
            .map(|cached| (object, index, cached)),
        _ => None,
    }
}

/// What a nested class's cache slot holds while the class is being created
/// for that enclosing object -- exactly while the activation evaluating its
/// superclass clause is on the stack. An access that finds it is a cycle
/// (`cyclic_definition_error`).
///
/// The slot itself, rather than a list of the creations underway, because
/// the slot is already keyed by exactly the pair a cycle is: the enclosing
/// object and the declaration's identity (`MixinApplication::nested_slot`), so
/// `class N = super N (...)`, a subclass redeclaring what it inherits, has a
/// slot of its own and is not reported as a cycle. The check is one read the
/// accessor makes anyway, where a list was scanned in full at every level of
/// a deep chain of creations -- quadratic in that depth -- and nothing here
/// is an object identity a moving collector would have to rewrite. SOMns
/// likewise keeps a class slot's whole state in the slot
/// (`ClassSlotAccessNode`, where nil means "create").
///
/// A primitive, not a new kind of `Value`: it is written only over `Nil` and
/// only by `begin_creation`, and replaced by the class or by `Nil` again when
/// the clause's activation is popped, so no Newspeak code ever reads it --
/// nothing but this accessor reads a hidden slot -- and no image holds it,
/// since only a host snapshots, between runs. An image that stores `false`
/// in a hidden slot itself gets a cyclic-definition error on access, the
/// same kind of wrong-but-safe answer as any other non-class value it puts
/// there, which the accessor answers as the class.
fn creation_in_progress<'gc>() -> Value<'gc> {
    Value::boolean(false)
}

fn is_creation_in_progress<'gc>(value: Value<'gc>) -> bool {
    value.as_boolean() == Some(false)
}

/// The clause activation of a creation has just been pushed: mark its class
/// as being created, in slot `index` of `enclosing`, where it will be cached.
/// Only a slot `nested_class` found `Nil` gets here.
pub(super) fn begin_creation<'gc>(
    mutator: &mut Mutator<'gc>,
    enclosing: ObjectReference<'gc>,
    index: usize,
) {
    mutator
        .heap
        .set_instance_field(enclosing, index, creation_in_progress());
    #[cfg(debug_assertions)]
    {
        mutator.creations_in_progress += 1;
    }
}

/// `creation`'s clause activation has been popped, by a return or by an
/// unwind: the class is no longer being created, and its slot is `Nil` again
/// until `after_clause` caches the class there.
///
/// It resets the marker only where the marker still is. A slot the program
/// can reach never changes under it, but the decoder does not bound
/// `InitializeSlot`'s field index, so an image's clause body can overwrite the
/// slot; that value is left as the image wrote it, and is why this is not an
/// assertion, which such an image would turn into a panic.
pub(super) fn end_creation<'gc>(mutator: &mut Mutator<'gc>, creation: &NestedClassCreation<'gc>) {
    if mutator
        .heap
        .instance_field(creation.enclosing, creation.index)
        .is_some_and(is_creation_in_progress)
    {
        mutator
            .heap
            .set_instance_field(creation.enclosing, creation.index, Value::nil());
    }
    #[cfg(debug_assertions)]
    {
        mutator.creations_in_progress -= 1;
    }
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if ec does not evaluate
/// to a class object C." A program error, raised through the ordinary
/// `NewspeakError` channel.
fn not_a_class_error<'gc>(name: Symbol, value: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: name,
        receiver: value,
        message: format!(
            "the superclass clause of class {name} did not evaluate to a class object"
        ),
    })
}

/// §6.1's run time error for a `<:` clause (§6.2 reuses its production),
/// naming the clause by its position, since a decoded mixin keeps no
/// receiver name to name it by.
fn not_a_mixin_error<'gc>(name: Symbol, index: usize, value: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: name,
        receiver: value,
        message: format!(
            "mixin application clause {} of class {name} did not evaluate to a class object",
            index + 1
        ),
    })
}

/// A class whose own creation depends on itself: `class N = N new (...)`,
/// or any longer cycle through other nested declarations of the same
/// receiver. §6 makes the accessor evaluate the declaration, so a clause
/// naming the class being created reaches the accessor again before the
/// cache slot is filled and would otherwise recurse until the activation
/// limit stopped it -- a stack overflow that names no class, not a
/// diagnosis.
///
/// This is a cycle in class *creation*, not general user recursion: the
/// creation of one class object for one receiver happens exactly once, so a
/// second entry for the same (receiver, declaration) is unconditionally a
/// cycle. The cache slot detects it, holding `creation_in_progress()` while the
/// first entry's clause runs.
fn cyclic_definition_error<'gc>(receiver: Value<'gc>, name: Symbol) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: name,
        receiver,
        message: format!(
            "cyclic class definition: evaluating the superclass clause of class {name} \
             requires class {name} itself"
        ),
    })
}

/// A receiver with no hidden slot to cache the nested class in. Reachable
/// from a real program: a class inheriting `Array`'s payload representation
/// gets an `ObjectData::Array` instance, which has no fields -- the same
/// known gap that already makes an
/// ordinary slot declared on such a class unwritable. Refusing here is what
/// keeps §6's "subsequent calls on r always return the same class object"
/// from silently becoming "a fresh class object every call".
fn no_cache_slot_error<'gc>(receiver: Value<'gc>, name: Symbol) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: name,
        receiver,
        message: format!(
            "cannot access nested class {name}: this object has no slot storage to cache it in"
        ),
    })
}
