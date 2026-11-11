//! A stopped run's stack as the agent reads it (`docs/design/debugger.md`,
//! "Reading the stack at a stop"): the run's frames, innermost first --
//! the activations on the activation stack, then the senders suspended
//! into their frame objects -- and a frame's variables: its receiver, its
//! arguments and the registers the leftover analysis says hold what the
//! frame wrote.

use std::rc::Rc;

use super::remote::{FrameDescription, FrameKind, Variable};
use super::values::StopValues;
use crate::bytecode::{CompiledCode, HEADER_REGISTERS};
use crate::heap::{ActivationReference, FrameReference, Heap};
use crate::interpreter::Mutator;
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

/// The descriptions of the run's `frames`, innermost first.
pub(crate) fn stack_trace(
    mutator: &Mutator<'_>,
    frames: &[StackFrame],
    values: &mut StopValues,
) -> Vec<FrameDescription> {
    let heap = &mutator.heap;
    frames
        .iter()
        .map(|frame| {
            let frame = frame.read(heap);
            FrameDescription {
                kind: frame_kind(frame.kind(heap)),
                program_counter: frame.program_counter(heap),
                source: frame.code(heap).source,
                receiver: values.describe(mutator, frame.receiver(heap)),
            }
        })
        .collect()
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

/// The variables of the frame at index `frame` of the stack trace, `None`
/// past the stack: `self`, each argument, then each register below the
/// first scratch register (`CompiledCode::local_count`) past the header's
/// numbers -- an activation's from the activation stack, a suspended
/// one's from its frame object (`Heap::frame_local`). A register the
/// leftover analysis finds tainted before the frame's stored program
/// counter is left out, as the collector's pre-pass nils it
/// (`Heap::trace_stack`) and a suspension nils it in the frame object:
/// such a word may be one a callee or a popped frame left -- a stale
/// handle, or a debug build's poison, whose read is refused. The analysis
/// taints no register below `local_count` today, since a push fills them
/// all; the check keeps the reader to it should that change.
pub(crate) fn variables(
    mutator: &Mutator<'_>,
    frames: &[StackFrame],
    frame: usize,
    values: &mut StopValues,
) -> Option<Vec<Variable>> {
    let heap = &mutator.heap;
    let frame = frames.get(frame)?.read(heap);
    let code = frame.code(heap);
    let program_counter = u32::try_from(frame.program_counter(heap))
        .expect("internal: a program counter fits its frame word");
    let tainted = code.tainted_before(program_counter).to_vec();
    let is_tainted = |register: u16| tainted.iter().any(|range| range.contains(&register));
    let mut variables = vec![Variable {
        name: String::from("self"),
        value: values.describe(mutator, frame.receiver(heap)),
    }];
    let locals = (code.argument_count + HEADER_REGISTERS)..code.local_count;
    for register in (0..code.argument_count).chain(locals) {
        if is_tainted(register) {
            continue;
        }
        let name = if register < code.argument_count {
            format!("argument {}", register + 1)
        } else {
            format!("register {register}")
        };
        variables.push(Variable {
            name,
            value: values.describe(mutator, frame.register(heap, register)),
        });
    }
    Some(variables)
}
