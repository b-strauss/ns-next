//! The exit that materializes (`docs/design/execution.md`, "The exit
//! materializes"): a frame machine code pushed holds only the receiver and
//! the current class its call wrote, and its return address on the native
//! stack names the send it was pushed for. Before any exit reaches the run
//! loop, the exit stub walks those return addresses, outermost first, and
//! hands each to `materialize_frame`, which writes the header of the frame
//! the address lies in -- the stretch's entry frame, the one the run loop
//! entered machine code at, first -- and works out where the send's callee
//! lies; `materialize_innermost` then writes the innermost frame's, the one
//! the exit left, and makes every frame the stack's, so that each reads as
//! an interpreted frame word for word.
//!
//! A leaf that machine code calls from a frame it pushed reads that frame
//! alone (`leaf_frame`): the frame's header is written as the walk writes
//! it, and the stack lets it read as the innermost for the leaf's length.
//!
//! Every word written here is derived: a return address names its body and
//! send (`JitState::return_site`), the send its callee's place on its
//! argument slots, and the address inside the code an exit or a leaf call
//! hands over its body (`JitState::body_of`).

use std::rc::Rc;

use crate::bytecode::{CodeKind, CompiledCode};
use crate::heap::{ActivationReference, CodeId, Heap, MachineFrame, MachineKind};
use crate::interpreter::Mutator;
use crate::mixin::MixinMethod;

use super::stubs::ExitKind;
use super::Installed;

/// Where the exit's walk stands: the frame the next return address lies
/// in, which the previous one's send placed, and what the walk has
/// gathered so far. Rust's alone; machine code never reads it.
#[derive(Default)]
pub(crate) struct Walk {
    /// The last frame materialized: its header's offset and register 0's.
    last: Option<(u32, u32)>,
    /// Where the last frame's send places its callee.
    callee: Option<Callee>,
    /// The top, at or past every materialized frame's window.
    top: usize,
    /// How many frames above the entry's the walk materialized.
    added: usize,
}

impl Walk {
    /// Seeds the walk for a call from an interpreter's send (`jit::called`):
    /// the sender, whose header lies at `sender` and register 0 at
    /// `sender_registers`, both word offsets from the stack's base, stands
    /// as the last frame materialized, its send's callee placed on its
    /// slots from register `argument_base`, the stack's top at `top`. So the callee is the walk's
    /// first frame, materialized as any compiled callee is, and the
    /// sender, interpreted and stopped at its send already, its caller; a
    /// refused call leaves the sender the innermost frame, with nothing
    /// added. An entry at a push leaves the walk unseeded, starting at the
    /// entry's frame.
    pub(crate) fn seed_call(
        &mut self,
        sender: u32,
        sender_registers: u32,
        argument_base: u16,
        top: usize,
    ) {
        let start = sender_registers + u32::from(argument_base);
        *self = Walk {
            last: Some((sender, sender_registers)),
            callee: Some(Callee {
                start,
                caller: sender,
                top,
            }),
            top,
            added: 0,
        };
    }
}

/// The callee a send places on its argument slots: its first argument's
/// word offset, its caller's header offset, and the top its push stood
/// at. Its header lies past as many arguments as its own code takes
/// (`Callee::offset`): an initializer a passed level decided takes none,
/// whatever its invocation placed.
#[derive(Clone, Copy)]
struct Callee {
    start: u32,
    caller: u32,
    top: usize,
}

impl Callee {
    /// Its header's word offset, for a callee running `code`.
    fn offset(self, code: &CompiledCode) -> u32 {
        self.start + u32::from(code.argument_count)
    }
}

/// The id of `code` in `heap`, registered with `method` when it is one's
/// body: a method's frame finds its method through its code's row
/// (`Heap::reify`).
fn code_id(
    heap: &mut Heap<'_>,
    code: &Rc<CompiledCode>,
    method: Option<&Rc<MixinMethod>>,
) -> CodeId {
    match method {
        Some(method) => heap.code_id_of_method(method),
        None => heap.code_id(code),
    }
}

/// The header of the frame at `offset`, running the installed body
/// `installed`, stopped at `program_counter`, whose caller is `caller`:
/// a method's frame for a method's body, and for a body machine code
/// calls as no method's, a factory's or an initializer's, as its code's
/// kind says (`baseline::BodyRole`).
fn machine_frame(
    heap: &mut Heap<'_>,
    offset: u32,
    installed: &Installed,
    program_counter: u32,
    caller: Option<(u32, usize)>,
) -> MachineFrame {
    let Installed {
        code,
        method,
        feedback_base,
    } = installed;
    let kind = match (method, code.kind) {
        (Some(method), _) => MachineKind::method(method.selector),
        (None, CodeKind::Method) => MachineKind::factory(),
        (None, CodeKind::Initializer) => MachineKind::initializer(),
        (None, CodeKind::Block | CodeKind::SuperclassClause) => {
            unreachable!("internal: machine code pushes no block's or superclass clause's frame")
        }
    };
    MachineFrame {
        offset,
        argument_count: code.argument_count,
        code: code_id(heap, code, method.as_ref()),
        kind,
        program_counter,
        feedback_base: *feedback_base,
        register_count: code.registers_above_header(),
        addresses_own_slots: code.addresses_own_slots,
        caller,
    }
}

/// Where the call at `program_counter` of `code` places its callee's
/// first argument, the calling frame's register 0 at `register_base`: on
/// its argument slots, or above the window (`sends::call_slots`).
fn callee_of_call(code: &CompiledCode, program_counter: u32, register_base: u32) -> u32 {
    let Some(slots) = super::sends::call_slots(code, program_counter) else {
        unreachable!("internal: a compiled call's return address names a send or an initializer's");
    };
    register_base + u32::from(slots.argument_base)
}

/// `Leaf::MaterializeFrame`: the frame the return address
/// `return_address` lies in, stopped at the send whose call returns there,
/// written -- for the stretch's entry frame, which the run loop pushed or
/// resumed, its program counter and compiled bit alone; a called callee,
/// the first frame of a seeded walk (`Walk::seed_call`), whole -- and its
/// send's callee placed for the next.
pub(crate) fn materialize_frame(mutator: &mut Mutator<'_>, return_address: u64) {
    let mut walk = std::mem::take(&mut mutator.jit.walk);
    let Some((installed, program_counter)) =
        mutator.jit.return_site(return_address_of(return_address))
    else {
        unreachable!("internal: a compiled frame's return address lies in an installed body");
    };
    let code = &installed.code;
    let (offset, register_base) = match walk.callee {
        None => {
            let Some(entry) = mutator.heap.top_activation() else {
                unreachable!("internal: machine code runs above its entry's frame");
            };
            mutator
                .heap
                .mark_stopped_in_machine_code(entry, program_counter);
            walk.top = mutator.heap.activation_stack_top();
            let register_base =
                Heap::register_base_index(mutator.heap.activation_register_base(entry));
            (Heap::activation_offset(entry), register_base)
        }
        Some(callee) => {
            let offset = callee.offset(code);
            let frame = machine_frame(
                &mut mutator.heap,
                offset,
                &installed,
                program_counter,
                Some((callee.caller, callee.top)),
            );
            let _ = mutator.heap.write_machine_frame(&frame);
            walk.top = walk.top.max(frame.window_end());
            walk.added += 1;
            (offset, callee.start)
        }
    };
    let start = callee_of_call(code, program_counter, register_base);
    walk.last = Some((offset, register_base));
    walk.callee = Some(Callee {
        start,
        caller: offset,
        top: walk.top,
    });
    mutator.jit.walk = walk;
}

/// `Leaf::MaterializeInnermost`: the frame the exit left, the callee of
/// the last frame's send, written at `program_counter` running the body
/// `code_address` lies in -- unless the exit is a refused push, whose
/// callee was never pushed, so that the last frame is the innermost and
/// re-executes its send -- then every frame the walk wrote made the
/// stack's. The innermost frame's handle waits for the run loop in
/// `JitState::take_walked`.
pub(crate) fn materialize_innermost(
    mutator: &mut Mutator<'_>,
    code_address: u64,
    kind: u64,
    program_counter: u64,
) {
    let mut walk = std::mem::take(&mut mutator.jit.walk);
    let (Some(callee), Some((last, _))) = (walk.callee, walk.last) else {
        unreachable!("internal: the innermost frame of a walk lies above a materialized one");
    };
    let innermost = if kind == ExitKind::PushRefused as u64 {
        last
    } else {
        let Some(installed) = mutator.jit.body_of(return_address_of(code_address)) else {
            unreachable!("internal: an exit inside a compiled frame leaves an installed body");
        };
        let Ok(program_counter) = u32::try_from(program_counter) else {
            unreachable!("internal: an exit's program counter fits 32 bits");
        };
        let offset = callee.offset(&installed.code);
        let frame = machine_frame(
            &mut mutator.heap,
            offset,
            &installed,
            program_counter,
            Some((callee.caller, callee.top)),
        );
        let _ = mutator.heap.write_machine_frame(&frame);
        walk.top = walk.top.max(frame.window_end());
        walk.added += 1;
        offset
    };
    mutator
        .heap
        .adopt_machine_frames(innermost, walk.top, walk.added);
    mutator.jit.statistics.materialized_frames += walk.added as u64;
    let Some(activation) = mutator.heap.top_activation() else {
        unreachable!("internal: a walk leaves its innermost frame on the stack");
    };
    mutator.jit.walked = Some(activation);
}

/// A word machine code handed over as an address.
fn return_address_of(word: u64) -> usize {
    let Ok(address) = usize::try_from(word) else {
        unreachable!("internal: machine code's addresses fit the pointer width");
    };
    address
}

/// The frame a leaf runs in, and how the stack is to be put back after it.
pub(crate) struct LeafFrame {
    pub(crate) activation: ActivationReference,
    /// The stack's top and innermost frame while machine code ran, when the
    /// frame is one machine code pushed, which reads as the innermost only
    /// for the leaf's length.
    restore: Option<(usize, u32)>,
}

/// The frame whose register 0 machine code holds at `frame_address`,
/// running the body `code_address` lies in, stopped at `program_counter`,
/// as a leaf it calls reads it: the stretch's entry frame as it is, a
/// frame machine code pushed with its header written as the walk writes
/// it, its caller left out, which no leaf reads -- a callee an
/// interpreter's send called among them, whose sender, the innermost
/// frame of the stack, runs no machine code. `release_leaf_frame` puts the
/// stack back.
pub(crate) fn leaf_frame(
    mutator: &mut Mutator<'_>,
    frame_address: u64,
    code_address: u64,
    program_counter: u32,
) -> LeafFrame {
    let Some(entry) = mutator.heap.top_activation() else {
        unreachable!("internal: machine code runs above its entry's frame");
    };
    let entry_base = Heap::register_base_index(mutator.heap.activation_register_base(entry));
    let Some(register_base) = mutator.heap.machine_register_base_index(frame_address) else {
        unreachable!("internal: a frame's registers lie in the activation stack");
    };
    if register_base == entry_base {
        return LeafFrame {
            activation: entry,
            restore: None,
        };
    }
    let Some(installed) = mutator.jit.body_of(return_address_of(code_address)) else {
        unreachable!("internal: a leaf is called from an installed body");
    };
    let frame = machine_frame(
        &mut mutator.heap,
        register_base + u32::from(installed.code.argument_count),
        &installed,
        program_counter,
        None,
    );
    let activation = mutator.heap.write_machine_frame(&frame);
    let restore = Some(mutator.heap.stack_extent());
    mutator.heap.view_machine_frame(activation);
    LeafFrame {
        activation,
        restore,
    }
}

/// Puts the stack back after a leaf ran in `frame`.
pub(crate) fn release_leaf_frame(mutator: &mut Mutator<'_>, frame: LeafFrame) {
    if let Some(extent) = frame.restore {
        mutator.heap.restore_stack_extent(extent);
    }
}
