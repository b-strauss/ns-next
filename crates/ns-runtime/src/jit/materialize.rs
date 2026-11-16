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
//!
//! A block the code eliminated is made by the exit that finds it live,
//! in every frame of optimized code the walk passes, the innermost and
//! every one below it, once every frame is the stack's
//! (`materialize_blocks`): one closure per block per exit, its home's
//! activation reified for it, so that the home goes on in its body's twin,
//! and written into every register that holds the block and into the
//! header of the scope that runs it.

use std::rc::Rc;

use crate::Value;
use crate::bytecode::{CodeKind, CompiledCode};
use crate::heap::{
    ActivationReference, ClassReference, CodeId, Heap, MachineFrame, MachineKind, ObjectReference,
};
use crate::interpreter::Mutator;
use crate::mixin::MixinMethod;

use super::optimizing::allocate::Location;
use super::optimizing::graph::{ClassRecipe, ClassStart};
use super::optimizing::plan::EliminatedId;
use super::stubs::{BlockWord, ExitKind};
use super::{EliminatedBlock, InlinedAt, InlinedScope, Installed, JitState};

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
    /// Each frame of optimized code the walk materialized at an interrupt
    /// point where a block its code eliminated is live: the frame, and
    /// the point, whose blocks are made once every frame is the stack's
    /// (`materialize_blocks`).
    blocks: Vec<BlocksAt>,
    /// The header offset of the physical frame the last return address
    /// lay in: the frame a refused push leaves the innermost.
    last_physical_offset: Option<u32>,
}

/// A frame of optimized code stopped at an interrupt point that holds
/// blocks its code eliminated: its header's word offset and its register
/// 0's, and the point.
#[derive(Clone)]
pub(crate) struct BlocksAt {
    offset: u32,
    register_base: u32,
    point: InlinedAt,
    /// Whether the frame lies below the innermost the exit leaves: a
    /// frame whose call the walk passed.
    below: bool,
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
            blocks: Vec::new(),
            last_physical_offset: None,
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
            blocks,
            last_physical_offset,
        } = self;
        *last = None;
        *callee = None;
        *top = 0;
        *added = 0;
        *last_physical = None;
        *inlined = 0;
        blocks.clear();
        *last_physical_offset = None;
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
        described: _,
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
/// frame, or a block's, as the interpreter's push of its method or its
/// closure would lay it -- the closure of a block the compile eliminated
/// made later, by the exit (`materialize_blocks`).
fn scope_frame(
    heap: &mut Heap<'_>,
    offset: u32,
    scope: &InlinedScope,
    program_counter: u32,
    caller: Option<(u32, usize)>,
) -> MachineFrame {
    let kind = match &scope.method {
        Some(method) => MachineKind::method(method.selector),
        None => MachineKind::inlined_closure(),
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
/// at `register_base` and whose header is at `offset`, running `physical`:
/// each the callee of the one below it, at the send it is inlined at, the
/// innermost at the point's own program counter, each with its current
/// class, which optimized code computes only before a frame node of the
/// scope. Answers the innermost's header offset, its register 0 and its
/// body -- the physical frame's own for a point of its own scope, there
/// for the blocks it holds -- and records the point for its blocks, if it
/// holds any.
fn materialize_scopes(
    mutator: &mut Mutator<'_>,
    inlined: &InlinedAt,
    (register_base, offset, physical): (u32, u32, &Rc<CompiledCode>),
    below: bool,
    walk: &mut Walk,
) -> (u32, u32, Rc<CompiledCode>) {
    if inlined.holds_eliminated_blocks() {
        walk.blocks.push(BlocksAt {
            offset,
            register_base,
            point: inlined.clone(),
            below,
        });
    }
    let chain = inlined.chain();
    let mut caller = offset;
    let mut innermost = (offset, register_base, Rc::clone(physical));
    // Each scope's header offset, by its `ScopeId`, the physical frame's
    // first, whose current class its call wrote.
    let mut headers: Vec<Option<u32>> = vec![None; inlined.metadata.scopes.len()];
    headers[0] = Some(offset);
    for stopped in chain.iter().rev().skip(1) {
        let scope = inlined.scope(stopped.scope);
        let scope_base = register_base + u32::from(stopped.offset);
        let scope_offset = scope_base + u32::from(scope.code.argument_count);
        // The class first: the header's guard and entered level, which the
        // frame's write records for a scope addressing its own slots, are
        // the current class's.
        headers[stopped.scope.0 as usize] = Some(scope_offset);
        let class = scope_class(mutator, scope_offset, scope.class, &headers);
        mutator.heap.set_walked_current_class(scope_offset, class);
        let frame = scope_frame(
            &mut mutator.heap,
            scope_offset,
            scope,
            stopped.program_counter,
            Some((caller, walk.top)),
        );
        let _ = mutator.heap.write_machine_frame(&frame);
        // The frame's write recorded the entered level of the class it
        // found in the header: the class written above, never one a
        // callee that ran on these slots left. No program compiled from
        // source inlines a scope addressing its own slots, so only this
        // says the order holds for an image's.
        #[cfg(debug_assertions)]
        if scope.code.addresses_own_slots {
            debug_assert_eq!(
                mutator.heap.walked_entered_level(scope_offset),
                mutator
                    .heap
                    .class_application_index(class)
                    .unwrap_or(crate::heap::ApplicationIndex::UNREGISTERED),
                "internal: an inlined scope's entered level is its current class's"
            );
        }
        walk.top = walk.top.max(frame.window_end());
        walk.added += 1;
        walk.inlined += 1;
        caller = scope_offset;
        innermost = (scope_offset, scope_base, Rc::clone(&scope.code));
    }
    innermost
}

/// The current class of the inlined scope whose header the walk wrote at
/// `offset`, as `recipe` finds it -- as optimized code computes it before
/// the scope's frame nodes -- from its receiver or from the current class
/// of a scope further out, whose header `headers` holds by its `ScopeId`,
/// written already.
fn scope_class<'gc>(
    mutator: &Mutator<'gc>,
    offset: u32,
    recipe: ClassRecipe,
    headers: &[Option<u32>],
) -> ClassReference<'gc> {
    let start = match recipe.from {
        None => mutator.heap.walked_receiver_and_current_class(offset).0,
        Some(source) => {
            let Some(source) = headers[source.0 as usize] else {
                unreachable!("internal: a scope's class source lies further out on its chain")
            };
            let (_, class) = mutator.heap.walked_receiver_and_current_class(source);
            let Some(class) = class else {
                unreachable!("internal: a scope further out has its current class")
            };
            Value::object(class)
        }
    };
    let mut class = match recipe.start {
        ClassStart::ClassOf => mutator.class_of(start),
        ClassStart::Class => start
            .as_object()
            .expect("internal: a class start is a class"),
    };
    for _ in 0..recipe.steps {
        class = mutator
            .heap
            .class_superclass(class)
            .expect("internal: the steps a send's lookup took lie within the chain");
    }
    class
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
    walk.last_physical_offset = Some(Heap::activation_offset(physical));
    // A call made inside an inlined scope: its callee lies on that scope's
    // slots, and the call counted the scopes already (`BodyAssembler::call_send`).
    let (offset, register_base, code) = match &inlined {
        Some(inlined) => {
            walk.last_physical = inlined.inlines().then_some(physical);
            materialize_scopes(
                mutator,
                inlined,
                (register_base, offset, code),
                true,
                &mut walk,
            )
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
        // The last frame walked re-executes its send: the innermost.
        let last_walked = walk.last_physical_offset;
        if let Some(innermost) = walk
            .blocks
            .iter_mut()
            .find(|walked| Some(walked.offset) == last_walked)
        {
            innermost.below = false;
        }
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
                    mutator,
                    inlined,
                    (register_base, offset, &installed.code),
                    false,
                    &mut walk,
                );
                mutator.heap.adjust_counted_depth(inlined_depth(inlined));
                if inlined.inlines() {
                    mutator.jit.exited_physical = Some(physical);
                }
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
    // Every frame of optimized code the walk passed goes on in baseline
    // code or the interpreter from here, which read every register: the
    // blocks its code eliminated are made, in every frame, not only the
    // innermost -- but for the innermost's at a poll it may go back in at.
    defer_the_poll_s_blocks(mutator, kind, code_address, &mut walk.blocks);
    materialize_blocks(mutator, &walk.blocks);
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
/// loop goes on at; `None` for any other exit, writing nothing but the
/// blocks the code eliminated, which an exit in its own scope makes in the
/// entry frame.
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
    let register_base = Heap::register_base_index(mutator.heap.activation_register_base(entry));
    if !inlined.inlines() {
        let mut blocks = vec![BlocksAt {
            offset: Heap::activation_offset(entry),
            register_base,
            point: inlined.clone(),
            below: false,
        }];
        defer_the_poll_s_blocks(mutator, kind, address, &mut blocks);
        materialize_blocks(mutator, &blocks);
        return None;
    }
    let program_counter = physical_program_counter(Some(inlined), 0);
    mutator
        .heap
        .mark_stopped_in_machine_code(entry, program_counter);
    let mut walk = Walk {
        top: mutator.heap.activation_stack_top(),
        ..Walk::default()
    };
    let (innermost, _, _) = materialize_scopes(
        mutator,
        inlined,
        (
            register_base,
            Heap::activation_offset(entry),
            &installed.code,
        ),
        false,
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
    defer_the_poll_s_blocks(mutator, kind, address, &mut walk.blocks);
    materialize_blocks(mutator, &walk.blocks);
    mutator.heap.top_activation()
}

/// The light exit of a poll a frame of optimized code may go back in at
/// (`jit::StoppedAtPoll`): the blocks its code eliminated that are live in
/// the frame that left, the one of `blocks` not below the innermost, are
/// taken out of `blocks` and left for the run loop, which makes them only
/// if the frame does not go back in (`jit::resumed_at_poll`). Every frame
/// below it goes on in its baseline code whatever the safepoint does, so
/// that its blocks are made now. Nothing for any other exit, `kind` and
/// `address` the exit's.
fn defer_the_poll_s_blocks(
    mutator: &mut Mutator<'_>,
    kind: u64,
    address: u64,
    blocks: &mut Vec<BlocksAt>,
) {
    debug_assert!(
        mutator.jit.deferred_blocks.is_none(),
        "internal: the last exit's deferred blocks were taken"
    );
    if kind != ExitKind::Safepoint as u64
        || !mutator.jit.poll_goes_back_in(return_address_of(address))
    {
        return;
    }
    if let Some(index) = blocks.iter().position(|stopped_at| !stopped_at.below) {
        mutator.jit.deferred_blocks = Some(blocks.remove(index));
    }
}

/// Makes the blocks optimized code eliminated that each of `points` holds
/// (`docs/design/execution.md`, "Closures, frames and non-local return in
/// inlined scopes"), once every frame the exit leaves is the stack's: for
/// each block live at a point one closure, over its home's activation --
/// reified, and so married to the activation in place, which goes on in
/// its body's twin -- with the values its home's registers hold, which the
/// compile kept unwritten since the block's `MakeClosure`; then that one
/// closure written into every register the point says holds the block, in
/// every scope, and made the activation's closure where a scope runs the
/// block, so that every place that holds the block holds one object, as
/// the interpreter's would. A block is made after any block its home's
/// activation runs, whose closure that activation's frame names, and any
/// its values are copied from. Allocates, as `MakeClosure` does, and
/// runs no Newspeak code.
pub(crate) fn materialize_blocks(mutator: &mut Mutator<'_>, points: &[BlocksAt]) {
    for stopped_at in points {
        let metadata = &stopped_at.point.metadata;
        let chain = stopped_at.point.chain();
        // Each scope of the chain's activation, by its `ScopeId`.
        let mut activations: Vec<Option<ActivationReference>> = vec![None; metadata.scopes.len()];
        for stopped in chain {
            let offset = match &metadata.scopes[stopped.scope.0 as usize] {
                Some(scope) => {
                    stopped_at.register_base
                        + u32::from(stopped.offset)
                        + u32::from(scope.code.argument_count)
                }
                None => stopped_at.offset,
            };
            activations[stopped.scope.0 as usize] =
                Some(mutator.heap.machine_activation_at(offset));
        }
        // The blocks live at the point: held in a register, or run by a
        // scope of the chain.
        let mut live: Vec<EliminatedId> = Vec::new();
        for stopped in chain {
            let run = metadata.scopes[stopped.scope.0 as usize]
                .as_ref()
                .and_then(|scope| scope.block);
            let held = stopped
                .locations
                .iter()
                .filter_map(|(_, location)| match location {
                    Location::EliminatedBlock(block) => Some(*block),
                    _ => None,
                });
            for block in run.into_iter().chain(held) {
                if !live.contains(&block) {
                    live.push(block);
                }
            }
        }
        let mut made: Vec<(EliminatedId, ObjectReference<'_>)> = Vec::new();
        while made.len() < live.len() {
            let waiting = |block: EliminatedId| {
                live.contains(&block) && !made.iter().any(|(each, _)| *each == block)
            };
            let Some(next) = live.iter().copied().find(|block| {
                let facts = &metadata.blocks[block.0 as usize];
                let home_runs = metadata.scopes[facts.home.0 as usize]
                    .as_ref()
                    .and_then(|scope| scope.block);
                let copied_holds = chain
                    .iter()
                    .filter(|stopped| stopped.scope == facts.home)
                    .flat_map(|stopped| &stopped.locations)
                    .filter(|(register, _)| facts.copied.contains(register))
                    .filter_map(|(_, location)| match location {
                        Location::EliminatedBlock(held) => Some(*held),
                        _ => None,
                    })
                    .any(waiting);
                waiting(*block) && !home_runs.is_some_and(waiting) && !copied_holds
            }) else {
                unreachable!("internal: an eliminated block's home and values are made first");
            };
            let facts = &metadata.blocks[next.0 as usize];
            let Some(home) = activations[facts.home.0 as usize] else {
                unreachable!("internal: a live block's home is a scope of the point's chain");
            };
            let closure = make_block(mutator, home, facts);
            made.push((next, closure));
            for stopped in chain {
                let Some(activation) = activations[stopped.scope.0 as usize] else {
                    continue;
                };
                let base = mutator.heap.activation_register_base(activation);
                for (register, location) in &stopped.locations {
                    if *location == Location::EliminatedBlock(next) {
                        mutator
                            .heap
                            .set_register(base, *register, Value::object(closure));
                    }
                }
                let runs = metadata.scopes[stopped.scope.0 as usize]
                    .as_ref()
                    .and_then(|scope| scope.block);
                if runs == Some(next) {
                    mutator
                        .heap
                        .set_closure_of_inlined_scope(activation, closure);
                }
            }
            mutator.jit.statistics.materialized_blocks += 1;
            #[cfg(test)]
            super::inlining_tests::record_block(
                mutator,
                (chain, &activations),
                (next, closure),
                home,
                stopped_at.below,
            );
        }
    }
}

/// The closure of the eliminated block `facts` describes, over `home`, the
/// activation of its home scope, as the interpreter's `MakeClosure` makes
/// it there (`vm::make_closure_into`).
fn make_block<'gc>(
    mutator: &mut Mutator<'gc>,
    home: ActivationReference,
    facts: &EliminatedBlock,
) -> ObjectReference<'gc> {
    let captured_frame = mutator.heap.reify(home);
    if facts.code.outer_reach > 0 {
        mutator.heap.retain_registers(home);
    }
    let feedback_base = mutator.heap.activation_feedback_base(home);
    let register_base = mutator.heap.activation_register_base(home);
    mutator.new_closure_from_registers(
        &facts.code,
        captured_frame,
        feedback_base,
        register_base,
        &facts.copied,
    )
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
/// running the body `code_address` lies in -- an address a leaf call's
/// `Operand::CodeAddress` handed over (`JitState::body_at_code_address`)
/// -- stopped at `program_counter`, as a leaf it calls reads it: the
/// stretch's entry frame as it is, a frame machine code pushed with its
/// header written as the walk writes it, its caller left out, which no
/// leaf reads -- a callee an interpreter's send called among them, whose
/// sender, the innermost frame of the stack, runs no machine code.
/// `release_leaf_frame` puts the stack back.
pub(crate) fn leaf_frame(
    mutator: &mut Mutator<'_>,
    frame_address: u64,
    code_address: u64,
    program_counter: u32,
) -> LeafFrame {
    leaf_frame_found(
        mutator,
        (frame_address, code_address, program_counter),
        JitState::body_at_code_address,
    )
}

/// `leaf_frame` for the sending frame of the call that returns to
/// `return_address`, whose register 0 is at `sender_address`, stopped at
/// the send at `program_counter`: what a leaf a send's stub calls reads.
pub(crate) fn sender_leaf_frame(
    mutator: &mut Mutator<'_>,
    sender_address: u64,
    return_address: u64,
    program_counter: u32,
) -> LeafFrame {
    leaf_frame_found(
        mutator,
        (sender_address, return_address, program_counter),
        JitState::body_of,
    )
}

/// `leaf_frame` with the body the address lies in found by `body_of`.
/// Inline into both, so that each finds its body its own way without a
/// call through a pointer.
#[inline(always)]
fn leaf_frame_found(
    mutator: &mut Mutator<'_>,
    (frame_address, code_address, program_counter): (u64, u64, u32),
    body_of: fn(&JitState, usize) -> Option<Installed>,
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
    let Some(installed) = body_of(&mutator.jit, return_address_of(code_address)) else {
        unreachable!("internal: a leaf is called from an installed body");
    };
    // Inside an inlined scope the frame register stands at the scope's
    // register 0: the leaf runs in the scope's frame.
    let inlined = match installed.described {
        Some(index) => {
            inlined_leaf_frame(mutator, index, code_address, register_base, program_counter)
        }
        None => None,
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
/// `register_base`, as a leaf called there reads it, looked up in the
/// metadata of the installed body at `body`; `None` for an
/// address in the body's own scope. Out of line: only optimized code
/// calls it.
#[inline(never)]
fn inlined_leaf_frame(
    mutator: &mut Mutator<'_>,
    body: u32,
    code_address: u64,
    register_base: u32,
    program_counter: u32,
) -> Option<MachineFrame> {
    let inlined = mutator
        .jit
        .inlined_in(body, return_address_of(code_address))
        .filter(InlinedAt::inlines)?;
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
