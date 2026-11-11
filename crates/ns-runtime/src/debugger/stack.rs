//! A stopped run's stack as the agent reads it (`docs/design/debugger.md`,
//! "Reading the stack at a stop"): the run's activations, innermost first,
//! and a frame's variables -- its receiver, its arguments and the
//! registers the leftover analysis says hold what the frame wrote.

use super::remote::{FrameDescription, FrameKind, Variable};
use super::values::StopValues;
use crate::bytecode::HEADER_REGISTERS;
use crate::heap::ActivationReference;
use crate::interpreter::Mutator;
use crate::types::ActivationKind;

/// The run's activations, innermost first, down to its base at `floor`
/// (`Heap::activation_position`): the activations below belong to the
/// host's sends that started this run, which the stop does not show.
/// Walked once per stop, which pushes and pops nothing, so that reading
/// every frame's variables costs the stack's depth, not its square.
pub(crate) fn activations(mutator: &Mutator<'_>, floor: usize) -> Vec<ActivationReference> {
    let heap = &mutator.heap;
    let mut activations = Vec::new();
    let mut next = heap.top_activation();
    while let Some(activation) = next {
        if heap.activation_position(activation) < floor {
            break;
        }
        activations.push(activation);
        next = heap.activation_sender(activation);
    }
    activations
}

/// The frames of the run's `activations`, innermost first.
pub(crate) fn stack_trace(
    mutator: &Mutator<'_>,
    activations: &[ActivationReference],
    values: &mut StopValues,
) -> Vec<FrameDescription> {
    let heap = &mutator.heap;
    activations
        .iter()
        .map(|&activation| FrameDescription {
            kind: frame_kind(heap.activation_kind(activation)),
            program_counter: heap.activation_program_counter(activation),
            source: heap.activation_code(activation).source,
            receiver: values.describe(mutator, heap.activation_receiver(activation)),
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
/// numbers. A register the leftover analysis finds tainted before the
/// frame's stored program counter is left out, as the collector's
/// pre-pass nils it (`Heap::trace_stack`): such a word may be one a
/// callee or a popped frame left -- a stale handle, or a debug build's
/// poison, whose read is refused. The analysis taints no register below
/// `local_count` today, since a push fills them all; the check keeps the
/// reader to it should that change.
pub(crate) fn variables(
    mutator: &Mutator<'_>,
    activations: &[ActivationReference],
    frame: usize,
    values: &mut StopValues,
) -> Option<Vec<Variable>> {
    let heap = &mutator.heap;
    let activation = *activations.get(frame)?;
    let code = heap.activation_code(activation);
    let program_counter = u32::try_from(heap.activation_program_counter(activation))
        .expect("internal: a program counter fits its frame word");
    let tainted = code.tainted_before(program_counter).to_vec();
    let is_tainted = |register: u16| tainted.iter().any(|range| range.contains(&register));
    let mut variables = vec![Variable {
        name: String::from("self"),
        value: values.describe(mutator, heap.activation_receiver(activation)),
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
            value: values.describe(mutator, heap.activation_local(activation, register)),
        });
    }
    Some(variables)
}
