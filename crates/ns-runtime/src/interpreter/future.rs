//! §6.3.2's pull future (`docs/design/object-model.md`, "Simultaneous
//! slots"): an instance of the kernel-private `Future`, whose two slots this
//! module alone reads and writes. Forcing one runs its block as a trampoline
//! step under `Continuation::Future`, never by calling back into Newspeak;
//! what happens once the block returns -- deliver the value to the send that
//! forced it, or run the instruction that met the future again -- is
//! `AfterForcing`.

use crate::builtins::primitive::Primitive;
use crate::heap::Heap;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Mutator, Symbol};

/// The `state` slot's three values, as `Kernel.ns` documents them.
const UNRESOLVED: i64 = 0;
const RESOLVING: i64 = 1;
const RESOLVED: i64 = 2;

/// The position of each of `Future`'s two slots in its field vector: the
/// declaration order in `Kernel.ns`, behind `Object`, which declares none.
/// `a_futures_slots_are_where_this_module_reads_them` pins both.
const COMPUTATION_FIELD: usize = 0;
const STATE_FIELD: usize = 1;
const FIELD_COUNT: usize = 2;

/// Where a future is in its life.
pub enum FutureState<'gc> {
    /// Not yet forced, or forced and abandoned: the closure that computes it.
    Unresolved(ObjectReference<'gc>),
    /// Its closure is running; forcing it again is §6.3.2's ill-founded
    /// recursion.
    Resolving,
    /// Its value, which is never itself a future: `future_computed` follows
    /// a future of a future before it records one.
    Resolved(Value<'gc>),
    /// Fields no forcing wrote: a state that is none of the three, an
    /// unresolved future whose `computation` is no closure, or a resolved
    /// one whose value is a future. Nothing the runtime does makes one, and
    /// the decoder refuses one in an image, but crafted code can store into
    /// a future's slots once it holds one -- a decoded frame may carry any
    /// receiver, and `InitializeSlot` stores into `self` unchecked -- so a
    /// future's fields are read as untrusted: forcing this one is an
    /// ordinary error (`malformed`), never a panic, and never a walk from
    /// one resolved future to the next that need not end.
    Malformed,
}

/// What the activation that forced a future does once its block returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterForcing {
    /// Answer the value to the send that forced it: `doesNotUnderstand:`'s
    /// `futureValue:ifFail:`.
    Deliver,
    /// Discard it and run the instruction that met the future again, which
    /// now finds it resolved (`docs/design/object-model.md`, "Where Rust sees
    /// a future").
    Reexecute,
}

/// A forcing in progress, boxed in the `Continuation::Future` of the
/// activation running its future's block, and owned by exactly one stack
/// entry.
///
/// The chain behind `then` is as long as a program makes it -- each future's
/// block answering the next -- and nothing bounds it: the run loop pops each
/// block's activation before it pushes the next, so the depth limit never
/// sees it. So everything that walks the chain loops rather than recursing
/// once per link: `future_computed`, `future_abandoned`, the trace, and
/// `Drop`, `Clone` and `Debug` below, which is why none of the three is
/// derived.
pub struct FutureForcing<'gc> {
    /// The future being computed, `Resolving` while this forcing lasts.
    pub(crate) future: ObjectReference<'gc>,
    pub(crate) after: AfterForcing,
    /// The forcing waiting on this one: a future whose block answered this
    /// one's future, unresolved, and which this one's value resolves too.
    /// Its own activation has returned already, so nothing but this link
    /// keeps it -- which is why `future_abandoned` walks it.
    pub(crate) then: Option<Box<FutureForcing<'gc>>>,
}

impl<'gc> FutureForcing<'gc> {
    /// Every link of the chain from this one on, as (future, after) pairs.
    fn links(&self) -> Vec<(ObjectReference<'gc>, AfterForcing)> {
        let mut links = Vec::new();
        let mut forcing = Some(self);
        while let Some(current) = forcing {
            links.push((current.future, current.after));
            forcing = current.then.as_deref();
        }
        links
    }
}

/// Unlinks the chain one box at a time, so that dropping it does not
/// recurse once per link.
impl Drop for FutureForcing<'_> {
    fn drop(&mut self) {
        let mut next = self.then.take();
        while let Some(mut forcing) = next {
            next = forcing.then.take();
        }
    }
}

/// Rebuilds the chain from its last link back, without recursing.
impl Clone for FutureForcing<'_> {
    fn clone(&self) -> Self {
        let mut links = self.links().into_iter().rev();
        let (future, after) = links
            .next()
            .expect("internal: a forcing is at least its own link");
        let mut chain = FutureForcing {
            future,
            after,
            then: None,
        };
        for (future, after) in links {
            chain = FutureForcing {
                future,
                after,
                then: Some(Box::new(chain)),
            };
        }
        chain
    }
}

/// The chain as a flat list of its links.
impl std::fmt::Debug for FutureForcing<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FutureForcing")
            .field("links", &self.links())
            .finish()
    }
}

/// What `vm::finish` does once a forcing's activation has returned.
pub(crate) enum FutureOutcome<'gc> {
    /// Hand the value to the send that forced the future.
    Deliver(Value<'gc>),
    /// Run the caller's stopped instruction again.
    Reexecute,
    /// The block answered another future, still unresolved: force it next,
    /// in the popped activation's place.
    Force(Step<'gc>),
}

impl<'gc> Mutator<'gc> {
    /// The future `value` is, if it is one. The class check is one compare:
    /// no program can subclass the private `Future`.
    pub fn as_future(&self, value: Value<'gc>) -> Option<ObjectReference<'gc>> {
        let Value::Object(object) = value else {
            return None;
        };
        self.heap
            .class_of_object(object)
            .same(self.builtin_class(BuiltinClass::Future))
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
        self.futures_made = true;
        let class = self.builtin_class(BuiltinClass::Future);
        let mut fields = vec![Value::Nil; FIELD_COUNT];
        fields[COMPUTATION_FIELD] = Value::Object(closure);
        fields[STATE_FIELD] = Value::SmallInteger(UNRESOLVED);
        self.heap
            .allocate_object(class, ObjectData::Instance(fields))
    }

    fn set_future_fields(
        &mut self,
        future: ObjectReference<'gc>,
        computation: Value<'gc>,
        state: i64,
    ) {
        self.heap
            .set_instance_field(future, COMPUTATION_FIELD, computation);
        self.heap
            .set_instance_field(future, STATE_FIELD, Value::SmallInteger(state));
    }

    /// Marks `future` resolving, its closure kept for an abandonment to
    /// restore.
    fn set_future_resolving(&mut self, future: ObjectReference<'gc>) {
        self.heap
            .set_instance_field(future, STATE_FIELD, Value::SmallInteger(RESOLVING));
    }

    /// `forcing`'s activation returned `value`: record it, unless it is
    /// itself a future, which is followed first -- §6.3.2's pipelined
    /// promise, as Primordial Soup's `resolve` loops. A forcing chained
    /// behind this one (`FutureForcing::then`) is resolved to the same value.
    /// An error leaves every future of the chain unresolved again, since no
    /// activation is left to abandon them.
    pub(crate) fn future_computed(
        &mut self,
        forcing: FutureForcing<'gc>,
        value: Value<'gc>,
    ) -> Result<FutureOutcome<'gc>, Signal<'gc>> {
        // A resolved future's value is never a future
        // (`FutureState::Resolved`), so one look through `value` is all it
        // takes: nothing here loops or recurses, whatever a future's fields
        // hold.
        let value = match self.as_future(value) {
            None => value,
            Some(inner) => match self.future_state(inner) {
                FutureState::Resolved(resolved) => resolved,
                FutureState::Resolving => {
                    self.future_abandoned(forcing);
                    return Err(divergent(Value::Object(inner)));
                }
                FutureState::Malformed => {
                    self.future_abandoned(forcing);
                    return Err(malformed(Value::Object(inner)));
                }
                FutureState::Unresolved(closure) => {
                    // Force the inner one, then come back to this one, which
                    // stays resolving meanwhile.
                    self.set_future_resolving(inner);
                    return Ok(FutureOutcome::Force(Step::Block {
                        closure,
                        arguments: StepArguments::Owned(Vec::new()),
                        then: Continuation::Future(Box::new(FutureForcing {
                            future: inner,
                            after: forcing.after,
                            then: Some(Box::new(forcing)),
                        })),
                    }));
                }
            },
        };
        // Record the value on every link, the waiting ones included; the
        // last link waits on nothing and says what happens next.
        let mut forcing = forcing;
        self.set_future_fields(forcing.future, value, RESOLVED);
        while let Some(mut waiting) = forcing.then.take() {
            self.set_future_fields(waiting.future, value, RESOLVED);
            forcing.after = waiting.after;
            forcing.then = waiting.then.take();
        }
        Ok(match forcing.after {
            AfterForcing::Deliver => FutureOutcome::Deliver(value),
            AfterForcing::Reexecute => FutureOutcome::Reexecute,
        })
    }

    /// The forcing activation was popped without returning -- its block
    /// raised and an unwind passed it, or the run loop refused to push it:
    /// the future is unresolved again, and so is every future chained
    /// behind it, so the next touch computes it again rather than meeting a
    /// stale "resolving" (`docs/design/object-model.md`, "A clause abandoned
    /// by an exception").
    pub(crate) fn future_abandoned(&mut self, forcing: FutureForcing<'gc>) {
        let mut next = Some(Box::new(forcing));
        while let Some(mut forcing) = next {
            if let FutureState::Resolving = self.future_state(forcing.future) {
                // Its closure, kept there while it resolved -- or whatever
                // crafted code stored since, which the next read finds
                // `Malformed`.
                let computation = self
                    .heap
                    .instance_field(forcing.future, COMPUTATION_FIELD)
                    .unwrap_or(Value::Nil);
                self.set_future_fields(forcing.future, computation, UNRESOLVED);
            }
            next = forcing.then.take();
        }
    }
}

/// Where `future` is in its life, read from the heap alone, so that the
/// encoder, which holds nothing more, reads a future as the run loop does.
/// `future` is an instance of `Future`; its fields are read as untrusted
/// (`FutureState::Malformed`).
fn state_in<'gc>(heap: &Heap<'gc>, future: ObjectReference<'gc>) -> FutureState<'gc> {
    let (Some(computation), Some(Value::SmallInteger(state))) = (
        heap.instance_field(future, COMPUTATION_FIELD),
        heap.instance_field(future, STATE_FIELD),
    ) else {
        return FutureState::Malformed;
    };
    match (state, computation) {
        (RESOLVED, Value::Object(value))
            if heap
                .class_of_object(value)
                .same(heap.class_of_object(future)) =>
        {
            FutureState::Malformed
        }
        (RESOLVED, value) => FutureState::Resolved(value),
        (RESOLVING, _) => FutureState::Resolving,
        (UNRESOLVED, Value::Object(closure)) if heap.is_closure(closure) => {
            FutureState::Unresolved(closure)
        }
        _ => FutureState::Malformed,
    }
}

/// Replaces every resolved future an object field holds with its value, for
/// a collection about to mark (`World::collect`). `Heap::replace_resolved_futures`
/// says why registers and host roots keep theirs. Answers the count.
pub(crate) fn replace_resolved_futures(mutator: &mut Mutator<'_>) -> usize {
    let future_class = mutator.builtin_class(BuiltinClass::Future);
    mutator
        .heap
        .replace_resolved_futures(future_class, COMPUTATION_FIELD, STATE_FIELD, RESOLVED)
}

/// What an image writes in place of `value`: a resolved future's value, and
/// `value` itself otherwise (`docs/design/image-format.md`, "Futures in a
/// graph"). Every reference to a resolved future is written as a reference
/// to its value, so the walk never records the future and a load gives back
/// one object where the program saw one. Reads the heap alone, since the
/// encoder holds nothing more; the class check is `class_builtin`, as the
/// encoder's other builtin checks are.
pub(crate) fn seen_through<'gc>(heap: &Heap<'gc>, value: Value<'gc>) -> Value<'gc> {
    let Value::Object(object) = value else {
        return value;
    };
    if heap.class_builtin(heap.class_of_object(object)) != Some(BuiltinClass::Future) {
        return value;
    }
    match state_in(heap, object) {
        FutureState::Resolved(resolved) => resolved,
        _ => value,
    }
}

/// The fields an image writes for an unresolved or resolving `future`: its
/// closure and the state 0. A future is resolving only while its block
/// runs, and `image::snapshot` refuses a world with a running activation,
/// so no snapshot meets one today; the case is defensive. Should one be
/// met, it is written as what `future_abandoned` would make of it -- its
/// closure is still in `computation`, kept there for exactly that -- since a
/// load starts no block, so nothing could finish resolving it.
///
/// `None` for a future whose fields are no future's
/// (`FutureState::Malformed`, or one "resolving" whose `computation` is no
/// closure), a malformed "resolved" one among them -- `seen_through` does
/// not see through that one: the decoder would refuse any of them, so the
/// encoder refuses to write one.
pub(crate) fn fields_for_an_image<'gc>(
    heap: &Heap<'gc>,
    future: ObjectReference<'gc>,
) -> Option<Vec<Value<'gc>>> {
    let closure = match state_in(heap, future) {
        FutureState::Unresolved(closure) => closure,
        FutureState::Resolving => match heap.instance_field(future, COMPUTATION_FIELD) {
            Some(Value::Object(closure)) if heap.is_closure(closure) => closure,
            _ => return None,
        },
        FutureState::Resolved(_) | FutureState::Malformed => return None,
    };
    let mut written = vec![Value::Nil; FIELD_COUNT];
    written[COMPUTATION_FIELD] = Value::Object(closure);
    written[STATE_FIELD] = Value::SmallInteger(UNRESOLVED);
    Some(written)
}

/// Whether a decoded record under the `Future` header is what the encoder
/// writes: exactly two fields, a closure and the state 0. `computation` may
/// name a record decoded later, so the closure half is checked once every
/// record is in (`decode::decode_graph_section`); this answers the closure
/// it names, if the rest holds.
pub(crate) fn decoded_future_computation<'gc>(fields: &[Value<'gc>]) -> Option<Value<'gc>> {
    if fields.len() != FIELD_COUNT {
        return None;
    }
    match (fields.get(COMPUTATION_FIELD), fields.get(STATE_FIELD)) {
        (Some(computation), Some(Value::SmallInteger(UNRESOLVED))) => Some(*computation),
        _ => None,
    }
}

/// Forces `future`: its value when it is resolved, its block's activation
/// under `Continuation::Future` when it is not, and §6.3.2's error when it
/// is already being computed.
pub(crate) fn force<'gc>(
    mutator: &mut Mutator<'gc>,
    future: ObjectReference<'gc>,
    after: AfterForcing,
) -> Result<Step<'gc>, Signal<'gc>> {
    match mutator.future_state(future) {
        FutureState::Resolved(value) => Ok(Step::Value(value)),
        FutureState::Resolving => Err(divergent(Value::Object(future))),
        FutureState::Malformed => Err(malformed(Value::Object(future))),
        FutureState::Unresolved(closure) => {
            mutator.set_future_resolving(future);
            Ok(Step::Block {
                closure,
                arguments: StepArguments::Owned(Vec::new()),
                then: Continuation::Future(Box::new(FutureForcing {
                    future,
                    after,
                    then: None,
                })),
            })
        }
    }
}

/// §6.3.2 resolves only "well founded" recursion: a future forced while its
/// own block runs is not, and says so as an ordinary, catchable error.
fn divergent<'gc>(future: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern("value"),
        receiver: future,
        message: "divergent evaluation of a simultaneous slot's initializer".to_string(),
    })
}

/// A future whose fields no forcing wrote (`FutureState::Malformed`): an
/// ordinary, catchable error, since only crafted code makes one and crafted
/// code must never make the runtime panic (`CLAUDE.md`, "An image is
/// untrusted input").
fn malformed<'gc>(future: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern("value"),
        receiver: future,
        message: "a malformed future: its fields are no future's".to_string(),
    })
}

/// A forcing that was to run its caller's instruction again found no
/// caller: a host's send handed the future straight to a leaf, and a host
/// has no instruction to run again (`vm::reexecute`).
pub(crate) fn host_passed_a_future<'gc>() -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern("value"),
        receiver: Value::Nil,
        message: "a future reached a native through a host's send, which has no instruction to \
                  run again"
            .to_string(),
    })
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
/// here too. Before any future exists this is one flag test.
#[inline(always)]
pub(crate) fn after_a_failed_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
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
    arguments: &[Value<'gc>],
    answer: Option<Result<Step<'gc>, Signal<'gc>>>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    if !matches!(answer, None | Some(Err(Signal::Error(_)))) {
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
    arguments: &[Value<'gc>],
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
    arguments: &[Value<'gc>],
) -> bool {
    debug_assert!(
        primitive.answers_false_to_a_stranger(),
        "internal: only a leaf that answers false to a stranger declines for a future"
    );
    std::iter::once(receiver)
        .chain(arguments.iter().copied())
        .any(|operand| mutator.as_future(operand).is_some())
}

/// What `see_through` found among a leaf's operands.
pub(crate) enum SeeThrough<'gc> {
    /// No operand is a future: the leaf's own answer stands.
    NoFuture,
    /// What stands in its place: the leaf run again on the resolved
    /// operands -- declining again (`None`) or raising again is then the
    /// leaf's own verdict on the values -- or a forcing under `Reexecute`.
    Answer(Option<Result<Step<'gc>, Signal<'gc>>>),
}

/// A leaf could not answer, and a future among its operands may be why
/// (`after_a_failed_leaf`). Resolved ones are replaced by their values and the
/// leaf runs again; the first unresolved one is forced under `Reexecute`,
/// after which the instruction that sent the leaf runs again -- which the
/// leaf's having changed nothing makes safe; one being computed is §6.3.2's
/// ill-founded recursion.
///
/// A resolved future's value is never a future (`FutureState::Resolved`),
/// so running the leaf once more on the resolved operands cannot come back
/// here with another.
pub(crate) fn see_through<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: Primitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> SeeThrough<'gc> {
    let operands = || std::iter::once(receiver).chain(arguments.iter().copied());
    // Looked for before anything is built: an equality that answers `false`
    // takes this path on every send once a program has made a future, and
    // almost never because of one.
    if operands().all(|operand| mutator.as_future(operand).is_none()) {
        return SeeThrough::NoFuture;
    }
    let mut resolved = Vec::with_capacity(arguments.len() + 1);
    for operand in operands() {
        let Some(future) = mutator.as_future(operand) else {
            resolved.push(operand);
            continue;
        };
        match mutator.future_state(future) {
            FutureState::Resolved(value) => resolved.push(value),
            FutureState::Resolving => return SeeThrough::Answer(Some(Err(divergent(operand)))),
            FutureState::Malformed => return SeeThrough::Answer(Some(Err(malformed(operand)))),
            FutureState::Unresolved(_) => {
                return SeeThrough::Answer(Some(force(mutator, future, AfterForcing::Reexecute)));
            }
        }
    }
    let Some((receiver, arguments)) = resolved.split_first() else {
        unreachable!("internal: the receiver is an operand");
    };
    SeeThrough::Answer(crate::builtins::execute(
        mutator, primitive, *receiver, selector, arguments,
    ))
}

/// What an instruction that must not hold a future -- `ResolveFuture`, and
/// `InitializeSlot` into a value -- makes of a value it read.
pub(crate) enum Resolution<'gc> {
    Value(Value<'gc>),
    /// Push this step; once it returns the instruction that asked runs
    /// again (`vm::force_and_reexecute`).
    Force(Step<'gc>),
}

/// What an instruction that must not hold a future -- `ResolveFuture`, and
/// `InitializeSlot` into a value -- makes of `value`: itself when
/// it is not a future, a resolved future's value, and an unresolved one
/// forced first (`AfterForcing::Reexecute`).
pub(crate) fn resolve_for_instruction<'gc>(
    mutator: &mut Mutator<'gc>,
    value: Value<'gc>,
) -> Result<Resolution<'gc>, Signal<'gc>> {
    let Some(future) = mutator.as_future(value) else {
        return Ok(Resolution::Value(value));
    };
    match force(mutator, future, AfterForcing::Reexecute)? {
        Step::Value(value) => Ok(Resolution::Value(value)),
        step => Ok(Resolution::Force(step)),
    }
}

/// `InitializeSlot` refused `value` for a value's slot: if it is a future,
/// the value it stands for is what belongs there -- stored and checked
/// again, or forced first. `None` when it is no future, and the refusal
/// stands.
pub(crate) fn value_for_a_value_slot<'gc>(
    mutator: &mut Mutator<'gc>,
    value: Value<'gc>,
) -> Option<Result<Resolution<'gc>, Signal<'gc>>> {
    mutator.as_future(value)?;
    Some(resolve_for_instruction(mutator, value))
}

/// `MakeFuture`'s operand held something other than a closure: a crafted
/// image can put anything in that register, and an image must never panic
/// (`CLAUDE.md`, "An image is untrusted input"), so this is an ordinary,
/// catchable error rather than the `unreachable!` a compiler-emitted
/// `MakeFuture` -- whose operand is always the block `MakeClosure` just
/// made -- would never trigger.
pub(crate) fn not_a_closure<'gc>(value: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern("value"),
        receiver: value,
        message: "MakeFuture needs a block".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use crate::{BuiltinClass, Interpreter};

    /// `future_state` and `set_future_fields` read and write by position:
    /// the two agree with `Kernel.ns` only while it declares `computation`
    /// first, and `Object` declares no slot ahead of them.
    #[test]
    fn a_futures_slots_are_where_this_module_reads_them() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(application) = mutator
                .heap()
                .class_application(mutator.builtin_class(BuiltinClass::Future))
            else {
                panic!("Future is a class");
            };
            let names: Vec<&str> = application
                .mixin
                .slots
                .iter()
                .map(|slot| slot.name.as_str())
                .collect();
            assert_eq!(names, ["computation", "state"]);
            assert_eq!(application.own_slot_base, 0, "Object declares no slot");
            assert_eq!(super::COMPUTATION_FIELD, 0);
            assert_eq!(super::STATE_FIELD, 1);
        });
    }
}
