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
//!
//! A frame of optimized code stands for every scope its code inlines
//! where an address of it lies (`docs/design/execution.md`, "Inlining"):
//! each inlined scope lies where the interpreter's push would have laid
//! its activation, its registers and its receiver and current class the
//! code's, so the walk writes each scope's other header words in place,
//! outermost first, as it writes a pushed frame's (`materialize_scopes`),
//! and the frame stops at the send its outermost scope is inlined at.
//! Nothing is copied or rebuilt: every scope then reads as an interpreted
//! frame and goes on in its own baseline code or the interpreter. A leaf
//! called inside an inlined scope reads that scope alone.

use std::rc::Rc;

use crate::bytecode::{CodeKind, CompiledCode};
use crate::heap::{ActivationReference, CodeId, Heap, MachineFrame, MachineKind};
use crate::interpreter::Mutator;
use crate::mixin::MixinMethod;

use super::stubs::{BlockWord, ExitKind};
use super::{InlinedAt, InlinedScope, Installed};

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
    /// The last frame materialized when its address lay in an inlined
    /// scope: the physical frame of the optimized copy, whose copy a
    /// refused push counts against (`JitState::exited_physical`).
    last_physical: Option<ActivationReference>,
    /// How many of the frames the walk materialized are inlined scopes'.
    inlined: usize,
}

impl Walk {
    /// Seeds the walk for a call from an interpreter's send (`jit::called`):
    /// the sender, whose header lies at `sender` and register 0 at
    /// `sender_registers`, both word offsets from the stack's base, stands
    /// as the last frame materialized, its send's callee placed on its
    /// slots from register `argument_base` -- `slots` of them -- the stack's
    /// top at `top`. So the callee is the walk's
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
        slots: u16,
        top: usize,
    ) {
        let start = sender_registers + u32::from(argument_base);
        *self = Walk {
            last: Some((sender, sender_registers)),
            callee: Some(Callee {
                start,
                slots: u32::from(slots),
                caller: sender,
                top,
                by_machine_code: false,
            }),
            top,
            added: 0,
            last_physical: None,
            inlined: 0,
        };
    }

    /// Forgets a seed no exit took, which a call that returned leaves
    /// behind (`jit::called`), field by field: assigning `Walk::default()`
    /// copied a partly written temporary, its `Option<Callee>`'s payload
    /// undefined, and read it back wider than it was written -- a
    /// store-forwarding stall on every call an interpreter's send made of
    /// machine code. The destructure names every field, so that a field
    /// added to the walk cannot be left out of the reset and carry one
    /// call's state into the next exit's walk.
    pub(crate) fn clear(&mut self) {
        let Walk {
            last,
            callee,
            top,
            added,
            last_physical,
            inlined,
        } = self;
        *last = None;
        *callee = None;
        *top = 0;
        *added = 0;
        *last_physical = None;
        *inlined = 0;
    }
}

/// The callee a send places on its argument slots: its first slot's word
/// offset, how many slots the send placed, its caller's header offset,
/// the top its push stood at, and whether a compiled send called it
/// rather than an interpreter's (`Walk::seed_call`). Its header lies past
/// as many arguments as its own code takes (`Callee::offset`) -- an
/// initializer a passed level decided takes none, whatever its invocation
/// placed -- but for a block's: a block an interpreter's send called lies
/// on the slots, its arguments the send's, and one a compiled send called
/// lies past them, where its closure stub placed it
/// (`targets::ClosureStub`), its arguments after the slots.
#[derive(Clone, Copy)]
struct Callee {
    start: u32,
    slots: u32,
    caller: u32,
    top: usize,
    by_machine_code: bool,
}

impl Callee {
    /// Its header's word offset, for a callee running `code`.
    fn offset(self, code: &CompiledCode) -> u32 {
        match code.kind {
            CodeKind::Block if self.by_machine_code => {
                self.start + self.slots + u32::from(code.argument_count)
            }
            CodeKind::Block => self.start + self.slots,
            _ => self.start + u32::from(code.argument_count),
        }
    }
}

/// The id of `code` in `heap`, registered with `method` when it is one's
/// body: a method's frame finds its method through its code's row
/// (`Heap::reify`).
pub(super) fn code_id(
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
/// a method's frame for a method's body, and for a body that is no
/// method's the kind its code's kind says -- a factory's, an
/// initializer's or a closure's, which machine code calls
/// (`baseline::BodyRole`), and a superclass clause's, which only the run
/// loop pushes, so that its frame is never one machine code pushed, and is
/// named here so that every frame's kind is.
pub(crate) fn machine_frame(
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
        reifies,
        optimized: _,
    } = installed;
    let kind = match (method, code.kind) {
        (Some(method), _) => MachineKind::method(method.selector),
        (None, CodeKind::Method) => MachineKind::factory(),
        (None, CodeKind::Initializer) => MachineKind::initializer(),
        (None, CodeKind::Block) => MachineKind::closure(),
        (None, CodeKind::SuperclassClause) => MachineKind::superclass_clause(),
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
        reifies: *reifies,
        inlined: false,
        caller,
    }
}

/// The header of the frame at `offset` of the inlined scope `scope` runs,
/// stopped at `program_counter`, whose caller is `caller`: a method's
/// frame, as the interpreter's push of its method would lay it.
fn scope_frame(
    heap: &mut Heap<'_>,
    offset: u32,
    scope: &InlinedScope,
    program_counter: u32,
    caller: Option<(u32, usize)>,
) -> MachineFrame {
    let kind = match &scope.method {
        Some(method) => MachineKind::method(method.selector),
        None => unreachable!("internal: only a method's body is inlined"),
    };
    MachineFrame {
        offset,
        argument_count: scope.code.argument_count,
        code: code_id(heap, &scope.code, scope.method.as_ref()),
        kind,
        program_counter,
        feedback_base: scope.feedback_base,
        register_count: scope.code.registers_above_header(),
        addresses_own_slots: scope.code.addresses_own_slots,
        reifies: false,
        inlined: true,
        caller,
    }
}

/// Writes the header of every inlined scope of the chain `inlined`
/// names, outermost first, above the physical frame whose register 0 is
/// at `register_base` and whose header is at `offset`: each the callee of
/// the one below it, at the send it is inlined at, the innermost at the
/// point's own program counter. Answers the innermost's header offset,
/// its register 0 and its body.
fn materialize_scopes(
    heap: &mut Heap<'_>,
    inlined: &InlinedAt,
    register_base: u32,
    offset: u32,
    walk: &mut Walk,
) -> (u32, u32, Rc<CompiledCode>) {
    let chain = inlined.chain();
    let mut caller = offset;
    let mut innermost = None;
    for stopped in chain.iter().rev().skip(1) {
        let scope = inlined.scope(stopped.scope);
        let scope_base = register_base + u32::from(stopped.offset);
        let scope_offset = scope_base + u32::from(scope.code.argument_count);
        let frame = scope_frame(
            heap,
            scope_offset,
            scope,
            stopped.program_counter,
            Some((caller, walk.top)),
        );
        let _ = heap.write_machine_frame(&frame);
        walk.top = walk.top.max(frame.window_end());
        walk.added += 1;
        walk.inlined += 1;
        caller = scope_offset;
        innermost = Some((scope_offset, scope_base, Rc::clone(&scope.code)));
    }
    let Some(innermost) = innermost else {
        unreachable!("internal: an inlined point's chain holds an inlined scope");
    };
    innermost
}

/// The program counter a physical frame stops at, for an address at
/// `program_counter` of the innermost scope `inlined` names: the send its
/// outermost inlined scope is inlined at, or the address's own.
fn physical_program_counter(inlined: Option<&InlinedAt>, program_counter: u32) -> u32 {
    match inlined {
        Some(inlined) => {
            let Some(physical) = inlined.chain().last() else {
                unreachable!("internal: a point's chain ends at the body's own scope");
            };
            physical.program_counter
        }
        None => program_counter,
    }
}

/// How many inlined scopes the chain `inlined` names holds.
fn inlined_depth(inlined: &InlinedAt) -> i64 {
    i64::try_from(inlined.chain().len() - 1)
        .expect("internal: a chain is bounded by the plan's limits")
}

/// Where the call at `program_counter` of `code` places its callee's
/// first argument, the calling frame's register 0 at `register_base` --
/// on its argument slots, or above the window (`sends::call_slots`) --
/// and how many it places.
fn callee_of_call(code: &CompiledCode, program_counter: u32, register_base: u32) -> (u32, u32) {
    let Some(slots) = super::sends::call_slots(code, program_counter) else {
        unreachable!("internal: a compiled call's return address names a send or an initializer's");
    };
    (
        register_base + u32::from(slots.argument_base),
        u32::from(slots.argument_count),
    )
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
    if installed.optimized {
        mutator.jit.statistics.optimized_frames_materialized += 1;
    }
    // The last frame's, which a refused push leaves the innermost.
    mutator.jit.walked_optimized = installed.optimized;
    let code = &installed.code;
    let inlined = inlined_at(mutator, &installed, return_address);
    let physical_counter = physical_program_counter(inlined.as_ref(), program_counter);
    let (offset, register_base, physical) = match walk.callee {
        None => {
            let Some(entry) = mutator.heap.top_activation() else {
                unreachable!("internal: machine code runs above its entry's frame");
            };
            mutator
                .heap
                .mark_stopped_in_machine_code(entry, physical_counter);
            walk.top = mutator.heap.activation_stack_top();
            let register_base =
                Heap::register_base_index(mutator.heap.activation_register_base(entry));
            (Heap::activation_offset(entry), register_base, entry)
        }
        Some(callee) => {
            let offset = callee.offset(code);
            let frame = machine_frame(
                &mut mutator.heap,
                offset,
                &installed,
                physical_counter,
                Some((callee.caller, callee.top)),
            );
            let physical = mutator.heap.write_machine_frame(&frame);
            walk.top = walk.top.max(frame.window_end());
            walk.added += 1;
            (offset, offset - u32::from(code.argument_count), physical)
        }
    };
    // A call made inside an inlined scope: its callee lies on that scope's
    // slots, and the call counted the scopes already (`BodyAssembler::call_send`).
    let (offset, register_base, code) = match &inlined {
        Some(inlined) => {
            walk.last_physical = Some(physical);
            materialize_scopes(&mut mutator.heap, inlined, register_base, offset, &mut walk)
        }
        None => {
            walk.last_physical = None;
            (offset, register_base, Rc::clone(code))
        }
    };
    let (start, slots) = callee_of_call(&code, program_counter, register_base);
    walk.last = Some((offset, register_base));
    walk.callee = Some(Callee {
        start,
        slots,
        caller: offset,
        top: walk.top,
        by_machine_code: true,
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
        mutator.jit.exited_physical = walk.last_physical;
        last
    } else {
        let Some(installed) = mutator.jit.body_of(return_address_of(code_address)) else {
            unreachable!("internal: an exit inside a compiled frame leaves an installed body");
        };
        let Ok(program_counter) = u32::try_from(program_counter) else {
            unreachable!("internal: an exit's program counter fits 32 bits");
        };
        if installed.optimized {
            mutator.jit.statistics.optimized_frames_materialized += 1;
        }
        mutator.jit.walked_optimized = installed.optimized;
        let inlined = inlined_at(mutator, &installed, code_address);
        let offset = callee.offset(&installed.code);
        let frame = machine_frame(
            &mut mutator.heap,
            offset,
            &installed,
            physical_program_counter(inlined.as_ref(), program_counter),
            Some((callee.caller, callee.top)),
        );
        let physical = mutator.heap.write_machine_frame(&frame);
        walk.top = walk.top.max(frame.window_end());
        walk.added += 1;
        match &inlined {
            // An exit inside an inlined scope: its scopes are activations
            // from here on, counted as the interpreter's pushes count them.
            Some(inlined) => {
                let register_base = offset - u32::from(installed.code.argument_count);
                let (innermost, _, _) = materialize_scopes(
                    &mut mutator.heap,
                    inlined,
                    register_base,
                    offset,
                    &mut walk,
                );
                mutator.heap.adjust_counted_depth(inlined_depth(inlined));
                mutator.jit.exited_physical = Some(physical);
                innermost
            }
            None => offset,
        }
    };
    mutator
        .heap
        .adopt_machine_frames(innermost, walk.top, walk.added);
    mutator.jit.statistics.materialized_frames += walk.added as u64;
    mutator.jit.statistics.inlined_frames_materialized += walk.inlined as u64;
    let Some(activation) = mutator.heap.top_activation() else {
        unreachable!("internal: a walk leaves its innermost frame on the stack");
    };
    mutator.jit.walked = Some(activation);
}

/// The inlined scopes the exit `block` describes left `entry`, the
/// stretch's entry frame, standing for, when its own optimized code exited
/// inside one with no compiled frame above it, which the exit stub walks
/// none of: each written as the walk writes it above its physical frame,
/// counted, and made the stack's, the entry stopped at the send its
/// outermost scope is inlined at. Answers the innermost, which the run
/// loop goes on at; `None`, writing nothing, for any other exit.
pub(crate) fn materialize_at_entry(
    mutator: &mut Mutator<'_>,
    entry: ActivationReference,
    kind: u64,
) -> Option<ActivationReference> {
    if kind == ExitKind::PushRefused as u64 {
        return None;
    }
    let address = mutator.jit.block.get(BlockWord::ExitCodeAddress.index());
    let installed = mutator.jit.body_of(return_address_of(address))?;
    let inlined = inlined_at(mutator, &installed, address)?;
    let inlined = &inlined;
    let program_counter = physical_program_counter(Some(inlined), 0);
    mutator
        .heap
        .mark_stopped_in_machine_code(entry, program_counter);
    let mut walk = Walk {
        top: mutator.heap.activation_stack_top(),
        ..Walk::default()
    };
    let register_base = Heap::register_base_index(mutator.heap.activation_register_base(entry));
    let (innermost, _, _) = materialize_scopes(
        &mut mutator.heap,
        inlined,
        register_base,
        Heap::activation_offset(entry),
        &mut walk,
    );
    mutator.heap.adjust_counted_depth(inlined_depth(inlined));
    mutator
        .heap
        .adopt_machine_frames(innermost, walk.top, walk.added);
    mutator.jit.statistics.materialized_frames += walk.added as u64;
    mutator.jit.statistics.inlined_frames_materialized += walk.inlined as u64;
    mutator.jit.statistics.optimized_frames_materialized += 1;
    mutator.jit.exited_physical = Some(entry);
    mutator.heap.top_activation()
}

/// The interrupt point of an inlined scope `address`, inside the body
/// `installed` describes, belongs to (`JitState::inlined_at`): asked of
/// optimized code alone.
fn inlined_at(mutator: &Mutator<'_>, installed: &Installed, address: u64) -> Option<InlinedAt> {
    if !installed.optimized {
        return None;
    }
    mutator.jit.inlined_at(return_address_of(address))
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
    /// Whether the frame is one machine code pushed that runs its body's
    /// lean code, which no leaf may reify (`release_leaf_frame`).
    #[cfg(debug_assertions)]
    unreified: bool,
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
            #[cfg(debug_assertions)]
            unreified: false,
        };
    }
    let Some(installed) = mutator.jit.body_of(return_address_of(code_address)) else {
        unreachable!("internal: a leaf is called from an installed body");
    };
    // Inside an inlined scope the frame register stands at the scope's
    // register 0: the leaf runs in the scope's frame.
    let inlined = if installed.optimized {
        inlined_leaf_frame(mutator, code_address, register_base, program_counter)
    } else {
        None
    };
    #[cfg(test)]
    let in_a_scope = inlined.is_some();
    let frame = match inlined {
        Some(frame) => frame,
        None => machine_frame(
            &mut mutator.heap,
            register_base + u32::from(installed.code.argument_count),
            &installed,
            program_counter,
            None,
        ),
    };
    let activation = mutator.heap.write_machine_frame(&frame);
    let restore = Some(mutator.heap.stack_extent());
    mutator.heap.view_machine_frame(activation);
    #[cfg(test)]
    super::inlining_tests::record_leaf(mutator, activation, in_a_scope);
    LeafFrame {
        activation,
        restore,
        #[cfg(debug_assertions)]
        unreified: !frame.reifies,
    }
}

/// The header of the frame of the innermost scope of optimized code the
/// address `code_address` lies in, whose register 0 is at
/// `register_base`, as a leaf called there reads it; `None` for an
/// address in the body's own scope. Out of line: only optimized code
/// calls it.
#[inline(never)]
fn inlined_leaf_frame(
    mutator: &mut Mutator<'_>,
    code_address: u64,
    register_base: u32,
    program_counter: u32,
) -> Option<MachineFrame> {
    let inlined = mutator.jit.inlined_at(return_address_of(code_address))?;
    let scope = inlined.scope(inlined.chain()[0].scope);
    Some(scope_frame(
        &mut mutator.heap,
        register_base + u32::from(scope.code.argument_count),
        scope,
        program_counter,
        None,
    ))
}

/// Puts the stack back after a leaf ran in `frame`. A debug build first
/// holds the leaf to having reified no frame machine code pushed that runs
/// its body's lean code: a lean body's frame is never reified, since its
/// code keeps no reified word and its return divorces nothing
/// (`MachineFrame::reifies`), so its frame object would stay married to an
/// offset popped under it.
pub(crate) fn release_leaf_frame(mutator: &mut Mutator<'_>, frame: LeafFrame) {
    #[cfg(debug_assertions)]
    assert!(
        !frame.unreified || mutator.heap.activation_reified(frame.activation).is_none(),
        "internal: a leaf reified a frame running lean code"
    );
    if let Some(extent) = frame.restore {
        mutator.heap.restore_stack_extent(extent);
    }
}
