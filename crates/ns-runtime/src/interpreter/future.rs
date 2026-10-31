//! §6.3.2's pull future (`docs/design/object-model.md`, "Simultaneous
//! slots"): an instance of the kernel-private `FutureImplementation`, which
//! a simultaneous slot clause or `Future computing:` makes. Forcing one is
//! its own `resolve`, an ordinary Newspeak method: only a `resolve` writes
//! a future's two slots once it is made -- its own through its slots, and
//! those of a chain's futures it takes over through the VM mirror's
//! leaves (`builtins::future`). The runtime reads them to see through a
//! resolved future, and wherever it meets one that is not, it pushes that
//! method (`Mutator::forcing`): answering the value to the instruction
//! that met the future -- `ResolveFuture` -- or,
//! for an instruction that has no destination for it, discarding it and
//! running the instruction again (`Continuation::Reexecute`).

use std::rc::Rc;

use super::Arguments;
use crate::builtins::primitive::Primitive;
use crate::heap::Heap;
use crate::interpreter::{FeedbackBase, MethodThen, Signal, Step, StepArguments};
use crate::mixin::MixinMethod;
use crate::types::*;
use crate::{Mutator, Symbol};

/// The `state` slot's three values, as `Kernel.ns` documents them and its
/// `resolve` writes them.
pub(crate) const UNRESOLVED: i64 = 0;
pub(crate) const RESOLVING: i64 = 1;
pub(crate) const RESOLVED: i64 = 2;

/// The position of each of `FutureImplementation`'s two slots in its field
/// vector: the declaration order in `Kernel.ns`, behind `Proxy` and
/// `Object`, which declare none.
/// `a_futures_slots_are_where_this_module_reads_them` pins both.
pub(crate) const COMPUTATION_FIELD: usize = 0;
pub(crate) const STATE_FIELD: usize = 1;
/// How many fields a future has, and so how many a decoded record under the
/// `FutureImplementation` header must carry: the image decoder refuses any
/// other count before it allocates the record (`decode::decode_graph_section`).
pub(crate) const FIELD_COUNT: usize = 2;

/// Where a future is in its life.
pub enum FutureState<'gc> {
    /// Not yet forced, or forced and abandoned: what computes it -- a
    /// closure, which forcing runs, or any other object, which forcing
    /// sends `value`, a future among them, which is forced only then.
    Unresolved(Value<'gc>),
    /// Its computation is running; forcing it again is §6.3.2's
    /// ill-founded recursion.
    Resolving,
    /// Its value, which is never itself a future: `resolve` forces a
    /// future its computation answered before it records the value.
    Resolved(Value<'gc>),
    /// Fields no forcing wrote: a state that is none of the three, or a
    /// resolved future whose value is a future. Nothing the runtime does
    /// makes one, and the decoder refuses one in an image, but crafted
    /// code can store into a future's slots once it holds one -- a decoded
    /// frame may carry any receiver, and `InitializeSlot` stores into
    /// `self` unchecked -- so a future's fields are read as untrusted:
    /// forcing this one is `resolve`'s ordinary error, never a panic, and
    /// never a walk from one resolved future to the next that need not
    /// end.
    Malformed,
}

impl<'gc> Mutator<'gc> {
    /// The future `value` is, if it is one. The class check is one compare:
    /// no program can subclass the private `FutureImplementation`.
    pub fn as_future(&self, value: Value<'gc>) -> Option<ObjectReference<'gc>> {
        let object = value.as_object()?;
        self.heap
            .object_class(object)
            .same(self.builtin_class(BuiltinClass::FutureImplementation))
            .then_some(object)
    }

    /// Where `future` is in its life, read from its two slots.
    pub fn future_state(&self, future: ObjectReference<'gc>) -> FutureState<'gc> {
        state_in(&self.heap, future)
    }

    /// A new unresolved future of `closure`. Marks the heap as having made
    /// one, which is what will let every check for a future stay off the
    /// paths of a program that never makes one.
    pub fn new_future(&mut self, closure: ObjectReference<'gc>) -> ObjectReference<'gc> {
        debug_assert!(
            self.heap.is_closure(closure),
            "internal: a future computes a closure"
        );
        self.note_futures_made();
        let class = self.builtin_class(BuiltinClass::FutureImplementation);
        let future = self.heap.allocate_instance(class, FIELD_COUNT);
        if !self
            .heap
            .instance_set_field(future, COMPUTATION_FIELD, Value::object(closure))
            || !self
                .heap
                .instance_set_field(future, STATE_FIELD, Value::small_integer(UNRESOLVED))
        {
            unreachable!("internal: a future's fields lie inside its layout");
        }
        future
    }

    /// The step that forces `future` for a send that takes its value: a
    /// resolved future's value at once, and for any other a push of its
    /// `resolve`, whose value is delivered to the send as any callee's --
    /// computed then, or the divergent or malformed future's error
    /// signalled by `resolve` itself.
    pub(crate) fn forcing(&self, future: ObjectReference<'gc>) -> Step<'gc> {
        if let FutureState::Resolved(value) = self.future_state(future) {
            return Step::Value(value);
        }
        let (method, defining_class) = self.resolve_method();
        Step::Method {
            receiver: Value::object(future),
            method,
            defining_class,
            arguments: StepArguments::Owned(Vec::new()),
            feedback_base: FeedbackBase::NOT_ASSIGNED,
        }
    }

    /// The step that forces `future`, which is not resolved, for an
    /// instruction that cannot take its value -- `InitializeSlot` into a
    /// value's slot, a leaf that saw the future among its operands: a push
    /// of its `resolve` whose value is discarded and whose return runs the
    /// instruction that met the future again (`Continuation::Reexecute`),
    /// which then finds it resolved. The instruction wrote nothing before
    /// it met the future, so running it again is running it once.
    fn forcing_then_reexecute(&self, future: ObjectReference<'gc>) -> Step<'gc> {
        let (method, defining_class) = self.resolve_method();
        Step::MethodThen(Box::new(MethodThen {
            receiver: Value::object(future),
            method,
            defining_class,
            arguments: Vec::new(),
            then: Continuation::Reexecute,
        }))
    }

    /// `FutureImplementation>>resolve` and the class it is activated in,
    /// looked up afresh at each forcing, which runs once per future.
    fn resolve_method(&self) -> (Rc<MixinMethod>, ClassReference<'gc>) {
        let class = self.builtin_class(BuiltinClass::FutureImplementation);
        let method = self
            .heap
            .class_application(class)
            .and_then(|application| application.mixin.compiled_method(Symbol::RESOLVE).cloned())
            .expect("internal: Kernel.ns's FutureImplementation declares resolve");
        (method, class)
    }
}

/// Where `future` is in its life, read from the heap alone, so that the
/// encoder, which holds nothing more, reads a future as the run loop does.
/// `future` is an instance of `FutureImplementation`; its fields are read
/// as untrusted (`FutureState::Malformed`).
fn state_in<'gc>(heap: &Heap<'gc>, future: ObjectReference<'gc>) -> FutureState<'gc> {
    let (Some(computation), Some(state)) = (
        heap.instance_field(future, COMPUTATION_FIELD),
        heap.instance_field(future, STATE_FIELD)
            .and_then(|state| state.as_small_integer()),
    ) else {
        return FutureState::Malformed;
    };
    match (state, computation.as_object()) {
        (RESOLVED, Some(value)) if heap.object_class(value).same(heap.object_class(future)) => {
            FutureState::Malformed
        }
        (RESOLVED, _) => FutureState::Resolved(computation),
        (RESOLVING, _) => FutureState::Resolving,
        (UNRESOLVED, _) => FutureState::Unresolved(computation),
        _ => FutureState::Malformed,
    }
}

/// The test both collectors apply to find a resolved future and its value
/// (`Heap::replace_resolved_futures`, the scavenger's weak pass); `None`
/// before the program has made a future, when there is none to find.
pub(crate) fn resolved_futures(mutator: &Mutator<'_>) -> Option<crate::heap::ResolvedFutures> {
    mutator.futures_made.then(|| {
        crate::heap::ResolvedFutures::new(
            mutator.builtin_class(BuiltinClass::FutureImplementation),
            COMPUTATION_FIELD,
            STATE_FIELD,
            RESOLVED,
        )
    })
}

/// What an image writes in place of `value`: a resolved future's value, and
/// `value` itself otherwise (`docs/design/image-format.md`, "Futures in a
/// graph"). Every reference to a resolved future is written as a reference
/// to its value, so the walk never records the future and a load gives back
/// one object where the program saw one. Reads the heap alone, since the
/// encoder holds nothing more; the class check is `class_builtin`, as the
/// encoder's other builtin checks are.
pub(crate) fn seen_through<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Value<'gc> {
    let Some(object) = value.as_object() else {
        return value;
    };
    if heap.class_builtin(heap.object_class(object)) != Some(BuiltinClass::FutureImplementation) {
        return value;
    }
    match state_in(heap, object) {
        FutureState::Resolved(resolved) => resolved,
        _ => value,
    }
}

/// The fields an image writes for an unresolved or resolving `future`: its
/// computation and the state 0. A future is resolving only while its
/// computation runs, and `image::snapshot` refuses a world with a running
/// activation, so no snapshot meets one today; the case is defensive.
/// Should one be met, it is written as what `resolve`'s `ifCurtailed:`
/// would make of it -- its computation is still in `computation`, kept
/// there for exactly that -- since a load starts no computation, so
/// nothing could finish resolving it.
///
/// `None` for a future whose fields are no future's
/// (`FutureState::Malformed`), a malformed "resolved" one among them --
/// `seen_through` does not see through that one: the decoder would refuse
/// it, so the encoder refuses to write one.
pub(crate) fn fields_for_an_image<'gc>(
    heap: &Heap<'gc>,
    future: ObjectReference<'gc>,
) -> Option<Vec<Value<'gc>>> {
    let computation = match state_in(heap, future) {
        FutureState::Unresolved(computation) => computation,
        FutureState::Resolving => heap.instance_field(future, COMPUTATION_FIELD)?,
        FutureState::Resolved(_) | FutureState::Malformed => return None,
    };
    let mut written = vec![Value::nil(); FIELD_COUNT];
    written[COMPUTATION_FIELD] = computation;
    written[STATE_FIELD] = Value::small_integer(UNRESOLVED);
    Some(written)
}

/// Whether a decoded record under the `FutureImplementation` header is what
/// the encoder writes: exactly two fields, the second the state 0. The
/// computation may be any value, as an unresolved future's may.
pub(crate) fn is_a_decoded_future_record(fields: &[Value<'_>]) -> bool {
    fields.len() == FIELD_COUNT
        && fields
            .get(STATE_FIELD)
            .and_then(|state| state.as_small_integer())
            == Some(UNRESOLVED)
}

/// A forcing that was to run its caller's instruction again found no
/// caller: a host's send handed the future straight to a leaf, and a host
/// has no instruction to run again (`vm::reexecute`).
pub(crate) fn host_passed_a_future<'gc>() -> Signal<'gc> {
    Signal::Error(NewspeakError::error(
        Symbol::intern("value"),
        Value::nil(),
        "a future reached a native through a host's send, which has no instruction to \
                  run again",
    ))
}

/// A leaf's answer on its caller's failure path -- the leaf declined or
/// raised -- seen through a future among its operands once the program has
/// made one: a leaf that could not answer for a future may well answer for
/// the value it stands for. Called by the three ways a leaf runs, and only
/// where the leaf's answer already sent them off their hot path:
/// `interpreter::mirror::execute_sent` and `send_inlined` (a mirror send,
/// before the failure block runs) and `execute_method` (a primitive method,
/// before the method is activated). A leaf that answers `false` to a
/// stranger declines for a future (`declines_for_a_future`), so it arrives
/// here too. A reflection leaf's answer stands as it is: a mirror reads a
/// future as the object it is, never forcing it. Before any future exists
/// this is one flag test.
#[inline(always)]
pub(crate) fn after_a_failed_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
    answer: Option<Result<Step<'gc>, Signal<'gc>>>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    if !mutator.futures_made {
        return answer;
    }
    see_through_a_failed_leaf(mutator, primitive, receiver, selector, arguments, answer)
}

#[cold]
#[inline(never)]
fn see_through_a_failed_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
    answer: Option<Result<Step<'gc>, Signal<'gc>>>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    if !matches!(answer, None | Some(Err(Signal::Error(_)))) {
        return answer;
    }
    // A mirror reflects the object it is given, a future the future
    // itself: forcing one here would run the program's computation from
    // inside a mirror read, which sends its reflectee nothing
    // (`docs/design/liveness-and-reflection.md`, "A mirror sends its
    // reflectee nothing"). A future's own leaves read and write the future
    // itself, for the `resolve` that is forcing it: seeing through it
    // would force it again from inside its own forcing.
    if matches!(primitive, Primitive::Reflection(_) | Primitive::Future(_)) {
        return answer;
    }
    match see_through(mutator, primitive, receiver, selector, arguments) {
        SeeThrough::NoFuture => answer,
        SeeThrough::Answer(seen) => seen,
    }
}

/// `answer`, the verdict of a leaf that answers `false` to a stranger
/// (`Primitive::answers_false_to_a_stranger` names each one), unless it is
/// `false` and an operand is a future: that leaf declines instead (`None`),
/// so its caller's failure path sees through the future
/// (`after_a_failed_leaf`) -- the future may well stand for an equal value.
/// On the `false` path alone, and only once the program has made a future.
#[inline(always)]
pub(crate) fn declines_for_a_future<'gc>(
    mutator: &Mutator<'gc>,
    primitive: Primitive,
    answer: bool,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<bool> {
    if answer || !mutator.futures_made {
        return Some(answer);
    }
    if a_future_among(mutator, primitive, receiver, arguments) {
        None
    } else {
        Some(false)
    }
}

#[cold]
#[inline(never)]
fn a_future_among<'gc>(
    mutator: &Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> bool {
    debug_assert!(
        primitive.answers_false_to_a_stranger(),
        "internal: only a leaf that answers false to a stranger declines for a future"
    );
    std::iter::once(receiver)
        .chain(arguments.iter(&mutator.heap))
        .any(|operand| mutator.as_future(operand).is_some())
}

/// What `see_through` found among a leaf's operands.
pub(crate) enum SeeThrough<'gc> {
    /// No operand is a future: the leaf's own answer stands.
    NoFuture,
    /// What stands in its place: the leaf run again on the resolved
    /// operands -- declining again (`None`) or raising again is then the
    /// leaf's own verdict on the values -- or the forcing of the first
    /// future that is not resolved, after which the instruction runs
    /// again (`Mutator::forcing_then_reexecute`).
    Answer(Option<Result<Step<'gc>, Signal<'gc>>>),
}

/// A leaf could not answer, and a future among its operands may be why
/// (`after_a_failed_leaf`). Resolved ones are replaced by their values and
/// the leaf runs again; the first one that is not resolved is forced by
/// its `resolve` -- which computes it, or signals that it is being
/// computed or malformed -- after which the instruction that sent the
/// leaf runs again, which the leaf's having changed nothing makes safe.
///
/// A resolved future's value is never a future (`FutureState::Resolved`),
/// so running the leaf once more on the resolved operands cannot come back
/// here with another.
pub(crate) fn see_through<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> SeeThrough<'gc> {
    // Looked for before anything is built: an equality that answers `false`
    // takes this path on every send once a program has made a future, and
    // almost never because of one.
    if std::iter::once(receiver)
        .chain(arguments.iter(&mutator.heap))
        .all(|operand| mutator.as_future(operand).is_none())
    {
        return SeeThrough::NoFuture;
    }
    let operands: Vec<Value<'gc>> = std::iter::once(receiver)
        .chain(arguments.iter(&mutator.heap))
        .collect();
    let mut resolved = Vec::with_capacity(operands.len());
    for operand in operands {
        let Some(future) = mutator.as_future(operand) else {
            resolved.push(operand);
            continue;
        };
        match mutator.future_state(future) {
            FutureState::Resolved(value) => resolved.push(value),
            FutureState::Unresolved(_) | FutureState::Resolving | FutureState::Malformed => {
                return SeeThrough::Answer(Some(Ok(mutator.forcing_then_reexecute(future))));
            }
        }
    }
    let Some((receiver, arguments)) = resolved.split_first() else {
        unreachable!("internal: the receiver is an operand");
    };
    SeeThrough::Answer(crate::builtins::execute(
        mutator,
        primitive,
        *receiver,
        selector,
        Arguments::Values(&arguments),
    ))
}

/// What `InitializeSlot` into a value's slot makes of a value it refused.
pub(crate) enum Resolution<'gc> {
    /// What belongs in the slot: a resolved future's value.
    Value(Value<'gc>),
    /// Push this step, the future's forcing; once it returns the
    /// instruction runs again (`vm::force_and_reexecute`).
    Force(Step<'gc>),
}

/// `InitializeSlot` refused `value` for a value's slot: if it is a future,
/// the value it stands for is what belongs there -- stored and checked
/// again when the future is resolved, or forced first. `None` when it is
/// no future, and the refusal stands.
pub(crate) fn value_for_a_value_slot<'gc>(
    mutator: &Mutator<'gc>,
    value: Value<'gc>,
) -> Option<Resolution<'gc>> {
    let future = mutator.as_future(value)?;
    Some(match mutator.future_state(future) {
        FutureState::Resolved(value) => Resolution::Value(value),
        FutureState::Unresolved(_) | FutureState::Resolving | FutureState::Malformed => {
            Resolution::Force(mutator.forcing_then_reexecute(future))
        }
    })
}

/// `MakeFuture`'s operand held something other than a closure: a crafted
/// image can put anything in that register, and an image must never panic
/// (`CLAUDE.md`, "An image is untrusted input"), so this is an ordinary,
/// catchable error rather than the `unreachable!` a compiler-emitted
/// `MakeFuture` -- whose operand is always the block `MakeClosure` just
/// made -- would never trigger.
pub(crate) fn not_a_closure<'gc>(value: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError::error(
        Symbol::intern("value"),
        value,
        "MakeFuture needs a block",
    ))
}

#[cfg(test)]
mod tests {
    use crate::{BuiltinClass, Interpreter};

    /// `future_state` and `set_future_fields` read and write by position:
    /// the two agree with `Kernel.ns` only while it declares `computation`
    /// first, and neither `Proxy` nor `Object` declares a slot ahead of
    /// them.
    #[test]
    fn a_futures_slots_are_where_this_module_reads_them() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(application) = mutator
                .heap()
                .class_application(mutator.builtin_class(BuiltinClass::FutureImplementation))
            else {
                panic!("FutureImplementation is a class");
            };
            let names: Vec<&str> = application
                .mixin
                .slots
                .iter()
                .map(|slot| slot.name.as_str())
                .collect();
            assert_eq!(names, ["computation", "state"]);
            assert_eq!(
                application.own_slot_base, 0,
                "Proxy and Object declare no slot"
            );
            assert_eq!(super::COMPUTATION_FIELD, 0);
            assert_eq!(super::STATE_FIELD, 1);
        });
    }
}
