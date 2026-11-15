//! A stopped run's stack as the agent reads it (`docs/design/debugger.md`,
//! "Reading the stack at a stop"): the run's frames, innermost first --
//! the activations on the activation stack, then the senders suspended
//! into their frame objects -- each named for what it runs and, through
//! its body's metadata, placed in its module's current text; and a frame's
//! variables: `self` and its named variables over its whole lexical chain
//! (decisions 10 and 13), or, for a body with no metadata, its arguments
//! and the registers the leftover analysis says hold what the frame wrote.

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;

use super::metadata::again::{module_name, BodyOrigin, Member};
use super::metadata::{BodyMetadata, Cache, VariablePlace, VectorHolder};
use super::remote::{
    FrameDescription, FrameKind, FrameLocation, SendLocation, Variable, VariableValue,
};
use super::values::StopValues;
use crate::bytecode::{CompiledCode, HEADER_REGISTERS};
use crate::heap::{ActivationReference, ClassReference, FrameReference, Heap};
use crate::interpreter::Mutator;
use crate::mixin::{MethodEntry, Mixin};
use crate::types::ActivationKind;
use crate::{Root, Value};

/// One frame of the stopped run, as the stop keeps it between commands.
pub(crate) enum StackFrame {
    /// An activation on the activation stack, which nothing pushes or
    /// pops while the program is stopped.
    Running(ActivationReference),
    /// An activation suspended into its frame object, held as a `Root`
    /// so that the stop names it across commands whatever the frame
    /// object's handle.
    Suspended(Root),
}

/// A frame of the stopped run as one command reads it.
#[derive(Clone, Copy)]
enum Frame<'gc> {
    Running(ActivationReference),
    Suspended(FrameReference<'gc>),
}

impl StackFrame {
    fn read<'gc>(&self, heap: &Heap<'gc>) -> Frame<'gc> {
        match self {
            StackFrame::Running(activation) => Frame::Running(*activation),
            StackFrame::Suspended(root) => {
                let Some(frame) = heap
                    .fetch(root)
                    .as_object()
                    .and_then(|object| heap.object_frame(object))
                else {
                    unreachable!("internal: the stop holds a suspended frame's frame object")
                };
                Frame::Suspended(frame)
            }
        }
    }
}

impl<'gc> Frame<'gc> {
    fn kind(self, heap: &Heap<'gc>) -> ActivationKind<'gc> {
        match self {
            Frame::Running(activation) => heap.activation_kind(activation),
            Frame::Suspended(frame) => heap.frame_kind(frame).activation_kind(),
        }
    }

    fn code(self, heap: &Heap<'gc>) -> Rc<CompiledCode> {
        match self {
            Frame::Running(activation) => heap.activation_code(activation),
            Frame::Suspended(frame) => heap.frame_code(frame).unwrap_or_else(|| {
                unreachable!("internal: a suspended frame names its code, as its resumption needs")
            }),
        }
    }

    fn program_counter(self, heap: &Heap<'gc>) -> usize {
        match self {
            Frame::Running(activation) => heap.activation_program_counter(activation),
            Frame::Suspended(frame) => heap.frame_stored_program_counter(frame),
        }
    }

    fn receiver(self, heap: &Heap<'gc>) -> Value<'gc> {
        match self {
            Frame::Running(activation) => heap.activation_receiver(activation),
            Frame::Suspended(frame) => heap.frame_receiver(frame),
        }
    }

    fn current_class(self, heap: &Heap<'gc>) -> Option<ClassReference<'gc>> {
        match self {
            Frame::Running(activation) => heap.activation_current_class(activation),
            Frame::Suspended(frame) => heap.frame_current_class(frame),
        }
    }

    /// The frame the closure this frame runs captured; `None` for a frame
    /// that runs no closure.
    fn captured(self, heap: &Heap<'gc>) -> Option<FrameReference<'gc>> {
        match self {
            Frame::Running(activation) => heap.activation_captured(activation),
            Frame::Suspended(frame) => heap.frame_captured(frame),
        }
    }

    fn register(self, heap: &Heap<'gc>, register: u16) -> Value<'gc> {
        match self {
            Frame::Running(activation) => heap.activation_local(activation, register),
            Frame::Suspended(frame) => heap.frame_local(frame, register),
        }
    }
}

/// The run's frames, innermost first, down to its base. The activations
/// on the activation stack are walked from the top through each sender
/// down to the run's floor (`Heap::activation_position`): the activations
/// below belong to the host's sends that started this run, which the stop
/// does not show. An activation whose sender is suspended
/// (`Heap::activation_suspended_sender`) -- the suspend stress mode
/// suspends every activation of the run into its frame object and resumes
/// the top one -- returns to that frame rather than to the one below it,
/// and the walk goes on through the senders the suspended frames name
/// (`Heap::frame_sender`) until one names none. That chain stays within
/// the run: only the run's own safepoint suspends, only its activations
/// at its floor or above, and the lowest of them names no sender, since it
/// returns below the floor. Walked once per stop, which pushes and pops
/// nothing, so that reading every frame's variables costs the stack's
/// depth, not its square.
pub(crate) fn frames(mutator: &Mutator<'_>, floor: usize) -> Vec<StackFrame> {
    let heap = &mutator.heap;
    let mut frames = Vec::new();
    let mut suspended = None;
    let mut next = heap.top_activation();
    while let Some(activation) = next {
        if heap.activation_position(activation) < floor {
            break;
        }
        frames.push(StackFrame::Running(activation));
        suspended = heap.activation_suspended_sender(activation);
        if suspended.is_some() {
            break;
        }
        next = heap.activation_sender(activation);
    }
    while let Some(frame) = suspended {
        debug_assert!(
            heap.frame_is_suspended(frame),
            "internal: a suspended activation's sender is suspended until it returns to it"
        );
        frames.push(StackFrame::Suspended(
            heap.stash(Value::object(heap.frame_object(frame))),
        ));
        suspended = heap.frame_sender(frame);
    }
    frames
}

/// `body_of` for a running activation: what a step floods
/// (`stepping.rs`).
pub(crate) fn running_body(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
) -> (
    Rc<CompiledCode>,
    Option<BodyOrigin>,
    Option<Rc<BodyMetadata>>,
) {
    body_of(mutator, &StackFrame::Running(activation))
}

/// `body_of` for an activation suspended into the frame object `frame`
/// holds: what a step floods for a frame it waits to be resumed.
pub(crate) fn suspended_body(
    mutator: &mut Mutator<'_>,
    frame: &Root,
) -> (
    Rc<CompiledCode>,
    Option<BodyOrigin>,
    Option<Rc<BodyMetadata>>,
) {
    body_of(mutator, &StackFrame::Suspended(frame.clone()))
}

/// The body `frame` runs, which declaration it belongs to, and its
/// metadata, compiled again on the first ask (`Cache::metadata`). The
/// metadata is looked up before anything else is read off the frame, so
/// that no handle is held across the compilation.
fn body_of<'gc>(
    mutator: &mut Mutator<'gc>,
    frame: &StackFrame,
) -> (
    Rc<CompiledCode>,
    Option<BodyOrigin>,
    Option<Rc<BodyMetadata>>,
) {
    let (code, origin) = {
        let heap = &mutator.heap;
        let frame = frame.read(heap);
        let code = frame.code(heap);
        let origin = origin_of(heap, frame, &code);
        (code, origin)
    };
    let metadata = origin
        .as_ref()
        .and_then(|origin| Cache::metadata(mutator, &code, origin));
    (code, origin, metadata)
}

/// Which declaration's member `frame` runs (`origin_of`), read without
/// its metadata: what a step filter decides by (`stepping::is_filtered`).
pub(crate) fn origin(heap: &Heap<'_>, frame: &StackFrame) -> Option<BodyOrigin> {
    let frame = frame.read(heap);
    let code = frame.code(heap);
    origin_of(heap, frame, &code)
}

/// Whether `frame` is a block's activation.
pub(crate) fn is_block(heap: &Heap<'_>, frame: &StackFrame) -> bool {
    matches!(frame.read(heap).kind(heap), ActivationKind::Closure(_))
}

/// Whether `frame` is a factory's activation, the stack trace's
/// `FrameKind::Factory`: decided by the frame's kind, which every factory
/// frame has, with metadata or without it.
pub(crate) fn is_factory(heap: &Heap<'_>, frame: &StackFrame) -> bool {
    matches!(frame.read(heap).kind(heap), ActivationKind::Factory(_))
}

/// The body `frame` runs.
pub(crate) fn code(heap: &Heap<'_>, frame: &StackFrame) -> Rc<CompiledCode> {
    frame.read(heap).code(heap)
}

/// The descriptions of the run's `frames`, innermost first, every frame
/// whose index `hidden` holds hidden.
pub(crate) fn stack_trace(
    mutator: &mut Mutator<'_>,
    frames: &[StackFrame],
    hidden: Range<usize>,
    values: &mut StopValues,
) -> Vec<FrameDescription> {
    let mut descriptions = Vec::with_capacity(frames.len());
    for (index, stack_frame) in frames.iter().enumerate() {
        let (code, origin, metadata) = body_of(mutator, stack_frame);
        let heap = &mutator.heap;
        let frame = stack_frame.read(heap);
        let kind = frame.kind(heap);
        let program_counter = frame.program_counter(heap);
        let name = frame_name(heap, kind, origin.as_ref(), frame.current_class(heap));
        let location = origin
            .as_ref()
            .zip(metadata.as_deref())
            .and_then(|(origin, metadata)| {
                // A frame below the top waits on the send at its program
                // counter; the top one stands wherever the stretch left it.
                let waiting = index > 0;
                location(mutator, origin, metadata, &code, program_counter, waiting)
            });
        let receiver = values.describe(mutator, frame.receiver(&mutator.heap));
        descriptions.push(FrameDescription {
            kind: frame_kind(kind),
            name,
            program_counter,
            source: code.source,
            location,
            receiver,
            hidden: hidden.contains(&index),
        });
    }
    descriptions
}

fn frame_kind(kind: ActivationKind<'_>) -> FrameKind {
    match kind {
        ActivationKind::Method(selector) => FrameKind::Method {
            selector: selector.to_string(),
        },
        ActivationKind::Closure(_) => FrameKind::Block,
        ActivationKind::Initializer(..) => FrameKind::Initializer,
        ActivationKind::SuperclassClause(_) => FrameKind::SuperclassClause,
        ActivationKind::Factory(_) => FrameKind::Factory,
        ActivationKind::TopLevel => FrameKind::TopLevel,
    }
}

/// Which declaration's member `frame`, running `code`, belongs to: a
/// method's is its current class's mixin's, an initializer's the mixin
/// that carries the body (`Heap::initializer_mixin`), a superclass
/// clause's the nested declaration it is the clause of, a factory's its
/// class's declaration, and a block's that of the body whose family holds
/// it. Each is the version whose member holds the frame's body
/// (`running_version`): an install re-applies every class object of the
/// declaration it edits over the new version and rebuilds every body of
/// it, so a frame still running a body of the old version names the
/// current version through its class, and only the body tells the two
/// apart. `None` for a top-level frame, and for a block no body of its
/// class's level creates.
fn origin_of<'gc>(
    heap: &Heap<'gc>,
    frame: Frame<'gc>,
    code: &Rc<CompiledCode>,
) -> Option<BodyOrigin> {
    let level_mixin = |class: ClassReference<'gc>| {
        heap.class_application(class)
            .map(|application| Rc::clone(&application.mixin))
    };
    // A body-less declaration's class object is the level its last clause
    // adds, whose application carries the applied mixin: its factory and
    // its full initializer are the declaration's.
    let declaration = |class: ClassReference<'gc>| {
        heap.class_body_less_declaration(class)
            .cloned()
            .or_else(|| level_mixin(class))
    };
    match frame.kind(heap) {
        ActivationKind::Method(selector) => {
            let current = level_mixin(frame.current_class(heap)?)?;
            let mixin = running_version(heap, &current, |version| {
                version
                    .compiled_method(selector)
                    .is_some_and(|method| Rc::ptr_eq(&method.code, code))
            })
            .unwrap_or(current);
            Some(BodyOrigin {
                mixin,
                member: Member::Method(selector),
            })
        }
        ActivationKind::Initializer(level, body) => {
            let current = heap.initializer_mixin(level, body)?;
            let mixin = running_version(heap, &current, |version| {
                body.code_of(version)
                    .is_some_and(|initializer| Rc::ptr_eq(&initializer, code))
            })
            .unwrap_or(current);
            Some(BodyOrigin {
                mixin,
                member: Member::Initializer(body),
            })
        }
        ActivationKind::SuperclassClause(declaring) => {
            let mixin = heap
                .nested_mixins(&level_mixin(declaring)?)
                .into_iter()
                .find_map(|nested| {
                    running_version(heap, &nested, |version| {
                        version
                            .superclass_clause_code
                            .as_ref()
                            .is_some_and(|clause| Rc::ptr_eq(clause, code))
                    })
                })?;
            Some(BodyOrigin {
                mixin,
                member: Member::SuperclassClause,
            })
        }
        ActivationKind::Factory(class) => Some(BodyOrigin {
            mixin: declaration(class)?,
            member: Member::Factory,
        }),
        ActivationKind::Closure(_) => {
            let class = frame.current_class(heap)?;
            let level = level_mixin(class)?;
            let body_less = heap.class_body_less_declaration(class).cloned();
            let own = body_less
                .into_iter()
                .chain(std::iter::once(Rc::clone(&level)))
                .find_map(|current| {
                    let mut member = None;
                    let mixin = running_version(heap, &current, |version| {
                        member = member_holding(version, code);
                        member.is_some()
                    })?;
                    Some(BodyOrigin {
                        mixin,
                        member: member?,
                    })
                });
            // A block of a superclass clause runs with the declaring
            // class as its current class, as the clause does.
            own.or_else(|| {
                heap.nested_mixins(&level).into_iter().find_map(|nested| {
                    let mixin = running_version(heap, &nested, |version| {
                        version
                            .superclass_clause_code
                            .as_ref()
                            .is_some_and(|clause| family_holds(clause, code))
                    })?;
                    Some(BodyOrigin {
                        mixin,
                        member: Member::SuperclassClause,
                    })
                })
            })
        }
        ActivationKind::TopLevel => None,
    }
}

/// The version of `current`'s mixin `holds` accepts: `current` itself,
/// else the newest version an install superseded that the heap still
/// holds (`Heap::superseded_mixins`) -- it keeps every version some
/// running body was compiled in -- under the same id. `None` when no
/// version is accepted.
fn running_version(
    heap: &Heap<'_>,
    current: &Rc<Mixin>,
    mut holds: impl FnMut(&Mixin) -> bool,
) -> Option<Rc<Mixin>> {
    if holds(current) {
        return Some(Rc::clone(current));
    }
    let id = heap.known_mixin_id(current)?;
    heap.superseded_mixins()
        .iter()
        .rev()
        .find(|version| heap.known_mixin_id(version) == Some(id) && holds(version))
        .cloned()
}

/// The member of `mixin` whose family -- its body and the blocks it
/// creates, transitively -- holds `code`.
fn member_holding(mixin: &Mixin, code: &Rc<CompiledCode>) -> Option<Member> {
    for (selector, entry) in &mixin.methods {
        if let MethodEntry::Compiled(method) = entry {
            if family_holds(&method.code, code) {
                return Some(Member::Method(*selector));
            }
        }
    }
    for kind in [
        crate::interpreter::instantiate::InitializerBody::Full,
        crate::interpreter::instantiate::InitializerBody::OwnPart,
        crate::interpreter::instantiate::InitializerBody::Standalone,
    ] {
        if kind
            .code_of(mixin)
            .is_some_and(|body| family_holds(&body, code))
        {
            return Some(Member::Initializer(kind));
        }
    }
    mixin
        .superclass_clause_code
        .as_ref()
        .is_some_and(|clause| family_holds(clause, code))
        .then_some(Member::SuperclassClause)
}

/// Whether `code` is `root` or a block `root` creates, transitively. A
/// worklist, not a recursion: an image's bodies may nest deeper than any
/// text bounds.
fn family_holds(root: &Rc<CompiledCode>, code: &Rc<CompiledCode>) -> bool {
    let mut pending = vec![Rc::clone(root)];
    let mut seen = HashSet::new();
    while let Some(body) = pending.pop() {
        if Rc::ptr_eq(&body, code) {
            return true;
        }
        if seen.insert(Rc::as_ptr(&body).addr()) {
            pending.extend(
                body.body()
                    .created_blocks()
                    .map(|block| Rc::clone(block.code)),
            );
        }
    }
    false
}

/// What a frame runs, as a reader names it: `Foo>>bar`, `Foo class>>bar`,
/// `[] in Foo>>bar`, `Foo initializer`, `Foo superclass clause`,
/// `Foo class>>new:` for a factory and `top level`. Named after its
/// declaration when it has an origin, after its current class otherwise.
fn frame_name<'gc>(
    heap: &Heap<'gc>,
    kind: ActivationKind<'gc>,
    origin: Option<&BodyOrigin>,
    class: Option<ClassReference<'gc>>,
) -> String {
    let owner = match origin {
        Some(origin) => origin.mixin.reflected_name(),
        None => class.map_or_else(String::new, |class| heap.class_display_name(class)),
    };
    match kind {
        ActivationKind::Method(selector) => format!("{owner}>>{selector}"),
        ActivationKind::Closure(_) => match origin.map(|origin| &origin.member) {
            Some(Member::Method(selector)) => format!("[] in {owner}>>{selector}"),
            Some(Member::Initializer(_)) => format!("[] in {owner} initializer"),
            Some(Member::SuperclassClause) => format!("[] in {owner} superclass clause"),
            Some(Member::Factory) | None => format!("[] in {owner}"),
        },
        ActivationKind::Initializer(..) => format!("{owner} initializer"),
        ActivationKind::SuperclassClause(_) => format!("{owner} superclass clause"),
        ActivationKind::Factory(_) => {
            let selector = crate::mixin::factory_entry_selector(
                origin.and_then(|origin| origin.mixin.factory),
            );
            format!("{owner} class>>{selector}")
        }
        ActivationKind::TopLevel => String::from("top level"),
    }
}

/// Where a frame stands in its module's current text: the innermost
/// statement holding `program_counter` -- a factory's header pattern --
/// and, for a frame `waiting` on a send, that send's receiver and
/// selector. `None` for a declaration no heap registered.
fn location(
    mutator: &Mutator<'_>,
    origin: &BodyOrigin,
    metadata: &BodyMetadata,
    code: &CompiledCode,
    program_counter: usize,
    waiting: bool,
) -> Option<FrameLocation> {
    let module = module_name(mutator, &origin.mixin)?;
    let program_counter = u32::try_from(program_counter).ok()?;
    // Inlined blocks' statements lie inside their statement's range: the
    // narrowest that holds the program counter is the innermost.
    let statement = metadata.factory_pattern.or_else(|| {
        metadata
            .statements
            .iter()
            .rev()
            .filter(|entry| entry.instructions.contains(&program_counter))
            .min_by_key(|entry| entry.instructions.len())
            .map(|entry| entry.span)
    });
    let send = waiting
        .then(|| {
            metadata
                .sends
                .binary_search_by_key(&program_counter, |entry| entry.index)
                .ok()
        })
        .flatten()
        .map(|found| {
            let entry = &metadata.sends[found];
            SendLocation {
                receiver: entry.receiver,
                selector: entry.selector.to_vec(),
            }
        });
    Some(FrameLocation {
        module,
        // A program counter no statement holds -- the return a body ends
        // with -- stands in the body.
        statement: statement.unwrap_or(code.source),
        send,
    })
}

/// The variables of the frame at index `frame` of the stack trace, `None`
/// past the stack: `self`, then, with the body's metadata, each named
/// variable once, the innermost of a name, read where it lives
/// (`named_variables`); without it, each argument and each register below
/// the first scratch register (`numbered_variables`).
pub(crate) fn variables(
    mutator: &mut Mutator<'_>,
    frames: &[StackFrame],
    frame: usize,
    values: &mut StopValues,
) -> Option<Vec<Variable>> {
    let stack_frame = frames.get(frame)?;
    let (code, _, metadata) = body_of(mutator, stack_frame);
    let heap = &mutator.heap;
    let frame = stack_frame.read(heap);
    let program_counter = u32::try_from(frame.program_counter(heap))
        .expect("internal: a program counter fits its frame word");
    let tainted = code.tainted_before(program_counter).to_vec();
    let reader = Reader {
        heap,
        frame,
        code: &code,
        tainted: &tainted,
    };
    let mut read = vec![(String::from("self"), Some(frame.receiver(heap)))];
    match metadata {
        Some(metadata) => named_variables(&reader, &metadata, &mut read),
        None => numbered_variables(&reader, &mut read),
    }
    Some(
        read.into_iter()
            .map(|(name, value)| Variable {
                name,
                value: value.map_or(VariableValue::Unavailable, |value| {
                    VariableValue::Shown(values.describe(mutator, value))
                }),
            })
            .collect(),
    )
}

/// What reads one stopped frame's variables.
struct Reader<'reader, 'gc> {
    heap: &'reader Heap<'gc>,
    frame: Frame<'gc>,
    code: &'reader CompiledCode,
    /// The registers the leftover analysis finds tainted before the
    /// frame's stored program counter.
    tainted: &'reader [Range<u16>],
}

impl<'gc> Reader<'_, 'gc> {
    /// Register `register` of the frame itself, if it is a named one --
    /// an argument, or one past the header's numbers and below the first
    /// scratch register -- the leftover analysis does not find tainted:
    /// the collector's pre-pass nils such a register (`Heap::trace_stack`)
    /// and a suspension nils it in the frame object, since it may hold a
    /// word a callee or a popped frame left -- a stale handle, or a debug
    /// build's poison, whose read is refused. The analysis taints no
    /// register below `local_count` today, since a push fills them all;
    /// the check keeps the reader to it should that change.
    fn own(&self, register: u16) -> Option<Value<'gc>> {
        let tainted = self.tainted.iter().any(|range| range.contains(&register));
        (is_named_register(self.code, register) && !tainted)
            .then(|| self.frame.register(self.heap, register))
    }

    /// Register `register` of the frame `hops` closures' captured frames
    /// out (decision 10). `None` where a hop finds no captured frame, and
    /// where the frame reached keeps no registers: its activation returned
    /// without keeping them (`Heap::frame_registers_kept`), unless the
    /// closure that hopped to it reads its registers by reference, whose
    /// creation made its activation keep them -- which also vouches for a
    /// frame an image decoded, whose record says neither.
    fn outer(&self, hops: u16, register: u16) -> Option<Value<'gc>> {
        if hops == 0 {
            return None;
        }
        let mut target = self.frame.captured(self.heap)?;
        let mut hopped_from = None;
        for _ in 1..hops {
            hopped_from = Some(self.heap.frame_code(target)?);
            target = self.heap.frame_captured(target)?;
        }
        let reach = hopped_from
            .as_deref()
            .map_or(self.code.outer_reach, |code| code.outer_reach);
        if reach == 0 && !self.heap.frame_registers_kept(target) {
            return None;
        }
        let code = self.heap.frame_code(target)?;
        if !is_named_register(&code, register) {
            return None;
        }
        // A running frame's registers are its activation's, read only
        // where the leftover analysis calls them live, as the frame's own.
        let running_at = match self.heap.frame_activation(target) {
            Some(activation) => Some(self.heap.activation_program_counter(activation)),
            None => self
                .heap
                .frame_is_suspended(target)
                .then(|| self.heap.frame_stored_program_counter(target)),
        };
        if let Some(program_counter) = running_at {
            let program_counter = u32::try_from(program_counter).ok()?;
            let tainted = code
                .tainted_before(program_counter)
                .iter()
                .any(|range| range.contains(&register));
            if tainted {
                return None;
            }
        }
        Some(self.heap.frame_local(target, register))
    }

    /// The variable at `place`; `None` where it lives nowhere any more:
    /// its register, or its vector's holder, is not available. A holder
    /// that holds no vector yet answers nil.
    fn place(&self, place: VariablePlace) -> Option<Value<'gc>> {
        match place {
            VariablePlace::Register(register) => self.own(register),
            VariablePlace::Outer { hops, register } => self.outer(hops, register),
            VariablePlace::Vector { vector, slot } => {
                let vector = match vector {
                    VectorHolder::Register(register) => self.own(register),
                    VectorHolder::Outer { hops, register } => self.outer(hops, register),
                }?;
                // A frame that has not run its `MakeVector` yet -- stopped
                // at its push -- holds nil where the vector goes: the
                // variable is in scope from its start, nil until written
                // (decision 13), never "not yet assigned".
                if vector.is_nil() {
                    return Some(Value::nil());
                }
                self.heap
                    .array_element(vector.as_object()?, usize::from(slot))
            }
        }
    }
}

/// Whether `register` is one a body names: an argument, or one past the
/// header's numbers and below the first scratch register.
fn is_named_register(code: &CompiledCode, register: u16) -> bool {
    register < code.argument_count
        || (code.argument_count + HEADER_REGISTERS..code.local_count).contains(&register)
}

/// Each variable of `metadata` once, the innermost of a name -- the
/// body's own come first -- read where it lives; a scratch register is
/// never shown (decision 13).
fn named_variables<'gc>(
    reader: &Reader<'_, 'gc>,
    metadata: &BodyMetadata,
    read: &mut Vec<(String, Option<Value<'gc>>)>,
) {
    let mut shown = HashSet::new();
    for entry in metadata.variables.iter() {
        if shown.insert(entry.name) {
            read.push((entry.name.to_string(), reader.place(entry.place)));
        }
    }
}

/// Each argument, `argument 1` up, then each register past the header's
/// numbers below the first scratch register (`CompiledCode::local_count`),
/// by number -- an activation's from the activation stack, a suspended
/// one's from its frame object (`Heap::frame_local`) -- but those the
/// leftover analysis finds tainted (`Reader::own`).
fn numbered_variables<'gc>(reader: &Reader<'_, 'gc>, read: &mut Vec<(String, Option<Value<'gc>>)>) {
    let code = reader.code;
    let locals = (code.argument_count + HEADER_REGISTERS)..code.local_count;
    for register in (0..code.argument_count).chain(locals) {
        let Some(value) = reader.own(register) else {
            continue;
        };
        let name = if register < code.argument_count {
            format!("argument {}", register + 1)
        } else {
            format!("register {register}")
        };
        read.push((name, Some(value)));
    }
}
