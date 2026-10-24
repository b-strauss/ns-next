//! The register VM's run loop -- and nothing else. `run` executes one
//! activation and everything it activates, on the `Heap`'s activation stack,
//! without recursing on the Rust stack: a send asks `interpreter/` what it
//! means (`inline_cache::send_public`, `send_lexical` and `send_super`, which
//! answer from the call site's cache or fall back on `Mutator::decide_send`,
//! `sends::decide_lexical_send` and `sends::decide_super_send`, and
//! `decide_send` itself for the reference lowering's `SendInlinable`) and,
//! when the answer is an activation, pushes
//! it and runs it here; a return pops it and resumes the caller. No dispatch
//! logic, no `doesNotUnderstand:`, no native dispatch and no error text of its
//! own live here -- dispatch lives in one place, which the reference lowering
//! and a JIT share. See `docs/design/execution.md`, "The VM".

use crate::interpreter::Arguments;
use std::ops::Range;
use std::rc::Rc;

use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{CompactInstruction, CompiledCode, Instruction, IntegerOperation, Literal};
use crate::heap::{ActivationReference, ArgumentSlots, RegisterBase};
use crate::interpreter::future::{
    not_a_closure, resolve_for_instruction, value_for_a_value_slot, FutureOutcome, Resolution,
};
use crate::interpreter::inline_cache::{send_lexical, send_public, send_super};
use crate::interpreter::instantiate::check_value_slot;
use crate::interpreter::literals::{body_too_large_error, unresolved_identifier_error};
use crate::interpreter::locals::{
    check_own_slots, filled_own_slot, load_own_slot, load_vector, read_local, read_outer_local,
    receiver_keeps_entered_level, store_instance_field, store_vector, write_local,
    write_outer_local,
};
use crate::interpreter::unwind::{non_boolean_condition, UnwindOutcome};
use crate::interpreter::{
    EvaluationResult, FeedbackBase, HostResult, Interpreter, Mutator, Signal, Step,
};
use crate::types::*;
use crate::Symbol;

/// The running frame's state, which the loop holds in locals while it runs
/// and in this struct between two stretches: nothing here is branded, so it
/// survives the safepoint between them (`docs/design/execution.md`, "The
/// loop over locals").
struct FrameRegisters {
    activation: ActivationReference,
    code: Rc<CompiledCode>,
    program_counter: usize,
    feedback_base: FeedbackBase,
    register_base: RegisterBase,
}

impl FrameRegisters {
    /// The state of `activation`, to run from `program_counter`.
    fn of(
        mutator: &Mutator<'_>,
        activation: ActivationReference,
        program_counter: usize,
    ) -> FrameRegisters {
        FrameRegisters {
            activation,
            code: mutator.heap.activation_code(activation),
            program_counter,
            feedback_base: mutator.heap.activation_feedback_base(activation),
            register_base: mutator.heap.activation_register_base(activation),
        }
    }
}

/// Why a stretch ended.
enum Leave {
    /// A safepoint is requested; the frame is written back.
    Safepoint,
    /// The run's base activation is done.
    Exit(HostResult),
}

/// Where control goes after a send, a return, an unwind or a forcing: one
/// byte, answered in a register. The frame a `Switched` names and the
/// result an `Exited` carries are in the `Handoff`.
#[derive(Clone, Copy)]
enum Transfer {
    /// Resume after the instruction: its value, if any, is written.
    Next,
    /// Run the frame the handoff names, from its program counter.
    Switched,
    /// The run is over; the handoff holds its result.
    Exited,
}

/// What a `Transfer::Switched` or `Transfer::Exited` carries.
struct Handoff {
    activation: ActivationReference,
    code: Option<Rc<CompiledCode>>,
    program_counter: usize,
    feedback_base: FeedbackBase,
    exit: Option<HostResult>,
}

impl Handoff {
    /// The frame of `activation`, from `program_counter`.
    #[inline(always)]
    fn switch_to(
        &mut self,
        mutator: &Mutator<'_>,
        activation: ActivationReference,
        program_counter: usize,
    ) -> Transfer {
        let code = mutator.heap.activation_code(activation);
        self.switch_with_code(mutator, activation, code, program_counter)
    }

    /// `switch_to`, for a caller that already holds `activation`'s code.
    #[inline(always)]
    fn switch_with_code(
        &mut self,
        mutator: &Mutator<'_>,
        activation: ActivationReference,
        code: Rc<CompiledCode>,
        program_counter: usize,
    ) -> Transfer {
        self.activation = activation;
        self.code = Some(code);
        self.program_counter = program_counter;
        self.feedback_base = mutator.heap.activation_feedback_base(activation);
        Transfer::Switched
    }

    fn exit(&mut self, result: HostResult) -> Transfer {
        self.exit = Some(result);
        Transfer::Exited
    }
}

/// Runs `base` -- the top of the activation stack, pushed by the caller --
/// and every activation it pushes, until `base` is done, and pops them all,
/// `base` included, on every path out: a return, an error, or a non-local
/// return aimed below `base`. The stack is exactly as deep afterwards as it
/// was before `base` was pushed.
///
/// Its safepoints are every switch to another activation -- a push, or a
/// caller resumed -- and every backward jump. Each stretch between two
/// safepoints the loop takes is one entry (`Interpreter::enter`), and the
/// safepoint is outside every one: what the loop carries from one stretch to
/// the next is `FrameRegisters` -- the running `ActivationReference`, its
/// code, its `program_counter`, its `FeedbackBase` and its `RegisterBase`,
/// none of which a move changes -- so a collection may run there and nowhere
/// else (`docs/design/memory.md`, "Where a collection can happen"), and a run
/// ends inside its last entry, its answer rooted (`docs/design/memory.md`,
/// "The brand"). Each safepoint is one compare of a flag the heap owns; what
/// happens when it is set is `interpreter/`'s (`Interpreter::safepoint`).
///
/// An activation's `program_counter` is stored only by the instructions that
/// can hand it to an observer: every send (a callee runs above it, and the loop
/// resumes it from that `program_counter`), `MakeClosure` (it reifies the
/// activation, so the frame starts from the `program_counter` stored just
/// before), `NonLocalReturn` (control leaves with a signal), and
/// `ResolveFuture` and `InitializeSlot` when either meets an unresolved
/// future (`force_and_reexecute` pushes the forcing above it, and the loop
/// must come back to this same instruction once the future resolves).
/// An instruction that raises an error -- `SignalNonBoolean`, `LoadLiteral`'s
/// error path -- leaves with a signal too but stores none: nothing resumes
/// the raising activation, since an `Error` is not resumable and a handler's
/// `retry` or `return:` pops it. Nothing reads `program_counter` while a
/// straight-line instruction executes.
pub(crate) fn run(interpreter: &mut Interpreter, base: ActivationReference) -> HostResult {
    let (floor, mut frame) = interpreter.enter(|mutator| {
        (
            mutator.heap.activation_position(base),
            FrameRegisters::of(mutator, base, 0),
        )
    });
    loop {
        // The stretch is forced into the loop rather than left to the
        // inliner. It is large -- every cached send inlines its hit path --
        // and past LLVM's threshold the step it holds became a function of
        // its own when each instruction was one entry, so that every
        // instruction, not only every send, paid a call and an answer
        // returned through memory: 6.6-13.6% on every benchmark when
        // splitting resolve from act first did that. Forcing it buys no speed
        // of its own: on the code before the caches, where the inliner took
        // the step anyway, it moved no benchmark beyond noise (at most 2.1%).
        // What it keeps is the caches' gain, 4.3-11.3% against that code,
        // from being paid back in a call per instruction
        // (`docs/design/execution.md`, "What the inline caches moved"). The
        // step sits near the threshold: anything added to a send's hit path
        // lands here.
        match interpreter.enter(
            #[inline(always)]
            |mutator| run_stretch(mutator, &mut frame, floor),
        ) {
            Leave::Safepoint => {
                // The suspend stress mode put a different frame on top --
                // the running activation, suspended and resumed -- whose
                // state the stretch wrote back before it left.
                if interpreter.safepoint(floor) {
                    frame = interpreter.enter(|mutator| {
                        let top = mutator
                            .heap
                            .top_activation()
                            .expect("internal: the safepoint resumed the running activation");
                        FrameRegisters::of(
                            mutator,
                            top,
                            mutator.heap.activation_program_counter(top),
                        )
                    });
                }
            }
            Leave::Exit(result) => return result,
        }
    }
}

/// Instructions of the running frame and every frame it switches to, inside
/// one entry, until a safepoint is requested at a switch or a backward jump,
/// or the run's base activation is done. The frame's state lives in locals
/// here -- Cog's `localIP`/`localFP` -- and is written back to `frame` only
/// when the stretch leaves for a safepoint. Each arm advances the program
/// counter itself; a send, a return, an unwind and a forcing answer a
/// `Transfer`, the frame they switch to written into `handoff`.
///
/// The loop matches the eight-byte slot (`bytecode::CompactInstruction`),
/// copied out, so the match runs on a value in registers. Each
/// instruction's effect is one function over `u16` operands, called by the
/// narrow arm and by `run_wide` alike. A send arm only gathers a
/// `PendingSend` and breaks out of its kind's labelled block; the one path
/// per kind follows that block, so each kind's cache hit path is inlined
/// here once, not once per form.
#[inline(always)]
fn run_stretch<'gc>(mutator: &mut Mutator<'gc>, frame: &mut FrameRegisters, floor: usize) -> Leave {
    let mut activation = frame.activation;
    let mut code = Rc::clone(&frame.code);
    let mut program_counter = frame.program_counter;
    let mut feedback_base = frame.feedback_base;
    let mut register_base = frame.register_base;
    let mut handoff = Handoff {
        activation,
        code: None,
        program_counter: 0,
        feedback_base,
        exit: None,
    };
    'run: loop {
        // A jump to `target`, from every instruction that jumps -- written
        // once, so that the set of jumps that test the safepoint when they
        // go backward is exactly the set of jumps. A backward jump closes a
        // loop, so that a loop that makes no send still passes a safepoint.
        // Leaving breaks out of the loop to the one write-back below it,
        // which every safepoint shares. Defined inside the labelled loop so
        // that its label is in scope where the macro is written.
        macro_rules! jump_to {
            ($target:expr) => {{
                let target = $target as usize;
                let backward = target <= program_counter;
                program_counter = target;
                if backward && mutator.heap.safepoint_requested() {
                    break 'run;
                }
                continue 'run;
            }};
        }
        macro_rules! next {
            () => {{
                program_counter += 1;
                continue 'run;
            }};
        }
        // One opcode per operation, so no arm dispatches on the operation
        // again: `integer_into` is inlined with it fixed.
        macro_rules! integer {
            ($operation:expr, $destination:expr, $left:expr, $right:expr, $target:expr) => {{
                if integer_into(
                    mutator,
                    register_base,
                    $operation,
                    u16::from($destination),
                    u16::from($left),
                    u16::from($right),
                ) {
                    next!()
                }
                jump_to!($target)
            }};
        }

        // The one send path per kind. Each kind joins its own path, so the
        // kind is never stored and dispatched on a second time, and no
        // aggregate shared by all four kinds is built.
        macro_rules! send_path {
            ($pending:ident, Public) => {{
                let site = code.body().send_site($pending.site as usize);
                let selector = site.selector();
                let call_site = site.call_site();
                let family_size = code.family_size;
                let receiver = mutator.heap.register(register_base, $pending.receiver);
                let slots = place_arguments(
                    mutator,
                    register_base,
                    $pending.argument_base,
                    $pending.arguments.as_slice(),
                );
                send(
                    mutator,
                    Sender {
                        activation,
                        register_base,
                        destination: $pending.destination,
                        inlinable: false,
                    },
                    program_counter,
                    slots,
                    floor,
                    &mut handoff,
                    |mutator, arguments| {
                        send_public(
                            mutator,
                            feedback_base,
                            family_size,
                            call_site,
                            receiver,
                            selector,
                            arguments,
                        )
                    },
                )
            }};
            ($pending:ident, Inlinable) => {{
                let site = code.body().send_site($pending.site as usize);
                let selector = site.selector();
                let receiver = mutator.heap.register(register_base, $pending.receiver);
                let slots = place_arguments(
                    mutator,
                    register_base,
                    $pending.argument_base,
                    $pending.arguments.as_slice(),
                );
                send(
                    mutator,
                    Sender {
                        activation,
                        register_base,
                        destination: $pending.destination,
                        inlinable: true,
                    },
                    program_counter,
                    slots,
                    floor,
                    &mut handoff,
                    |mutator, arguments| mutator.decide_send(receiver, selector, arguments),
                )
            }};
            ($pending:ident, Lexical) => {{
                let site = code.body().send_site($pending.site as usize);
                let selector = site.selector();
                let call_site = site.call_site();
                let depth = site.depth();
                let declaration_code = code.is_declaration_code();
                let family_size = code.family_size;
                let slots = place_arguments(
                    mutator,
                    register_base,
                    $pending.argument_base,
                    $pending.arguments.as_slice(),
                );
                send(
                    mutator,
                    Sender {
                        activation,
                        register_base,
                        destination: $pending.destination,
                        inlinable: false,
                    },
                    program_counter,
                    slots,
                    floor,
                    &mut handoff,
                    |mutator, arguments| {
                        send_lexical(
                            mutator,
                            activation,
                            feedback_base,
                            family_size,
                            call_site,
                            depth,
                            declaration_code,
                            selector,
                            arguments,
                        )
                    },
                )
            }};
            ($pending:ident, Super) => {{
                let site = code.body().send_site($pending.site as usize);
                let selector = site.selector();
                let call_site = site.call_site();
                let family_size = code.family_size;
                let slots = place_arguments(
                    mutator,
                    register_base,
                    $pending.argument_base,
                    $pending.arguments.as_slice(),
                );
                send(
                    mutator,
                    Sender {
                        activation,
                        register_base,
                        destination: $pending.destination,
                        inlinable: false,
                    },
                    program_counter,
                    slots,
                    floor,
                    &mut handoff,
                    |mutator, arguments| {
                        send_super(
                            mutator,
                            activation,
                            feedback_base,
                            family_size,
                            call_site,
                            selector,
                            arguments,
                        )
                    },
                )
            }};
        }
        debug_assert!(
            program_counter < code.instruction_count(),
            "internal: program counter ran past the end of a CompiledCode with no terminating return"
        );
        let transfer = 'decided: {
            let lexical = 'lexical: {
                let public = 'public: {
                    let super_send = 'super_send: {
                        let inlinable = 'inlinable: {
                            match code.slot(program_counter) {
                                CompactInstruction::LoadNil { destination } => {
                                    mutator.heap.set_register(
                                        register_base,
                                        u16::from(destination),
                                        Value::nil(),
                                    );
                                    next!()
                                }
                                CompactInstruction::LoadTrue { destination } => {
                                    mutator.heap.set_register(
                                        register_base,
                                        u16::from(destination),
                                        Value::boolean(true),
                                    );
                                    next!()
                                }
                                CompactInstruction::LoadFalse { destination } => {
                                    mutator.heap.set_register(
                                        register_base,
                                        u16::from(destination),
                                        Value::boolean(false),
                                    );
                                    next!()
                                }
                                CompactInstruction::LoadSmallInteger { destination, value } => {
                                    mutator.heap.set_register(
                                        register_base,
                                        u16::from(destination),
                                        Value::small_integer(i64::from(value)),
                                    );
                                    next!()
                                }
                                CompactInstruction::LoadConstant {
                                    destination,
                                    constant,
                                } => {
                                    let value = Value::from_immediate(
                                        code.body().constant(usize::from(constant)),
                                    );
                                    mutator.heap.set_register(
                                        register_base,
                                        u16::from(destination),
                                        value,
                                    );
                                    next!()
                                }
                                CompactInstruction::LoadLiteral {
                                    destination,
                                    literal,
                                } => match load_literal_into(
                                    mutator,
                                    activation,
                                    register_base,
                                    u16::from(destination),
                                    code.body().literal(usize::from(literal)),
                                ) {
                                    Ok(()) => next!(),
                                    Err(signal) => {
                                        break 'decided unwind(mutator, signal, floor, &mut handoff)
                                    }
                                },
                                CompactInstruction::LoadSelf { destination } => {
                                    load_self_into(
                                        mutator,
                                        activation,
                                        register_base,
                                        u16::from(destination),
                                    );
                                    next!()
                                }
                                CompactInstruction::LoadEnclosingObject { destination, depth } => {
                                    load_enclosing_object_into(
                                        mutator,
                                        activation,
                                        register_base,
                                        u16::from(destination),
                                        depth,
                                        code.is_declaration_code(),
                                    );
                                    next!()
                                }
                                CompactInstruction::Move {
                                    destination,
                                    source,
                                }
                                | CompactInstruction::StoreLocal {
                                    source,
                                    slot: destination,
                                } => {
                                    copy_register(
                                        mutator,
                                        register_base,
                                        u16::from(destination),
                                        u16::from(source),
                                    );
                                    next!()
                                }
                                CompactInstruction::LoadOuterLocal {
                                    destination,
                                    index,
                                    depth,
                                } => {
                                    load_outer_local_into(
                                        mutator,
                                        activation,
                                        register_base,
                                        u16::from(destination),
                                        depth,
                                        u16::from(index),
                                    );
                                    next!()
                                }
                                CompactInstruction::StoreOuterLocal {
                                    source,
                                    index,
                                    depth,
                                } => {
                                    store_outer_local_from(
                                        mutator,
                                        activation,
                                        register_base,
                                        u16::from(source),
                                        depth,
                                        u16::from(index),
                                    );
                                    next!()
                                }
                                CompactInstruction::SendPublic0 {
                                    destination,
                                    receiver,
                                    argument_base,
                                    site,
                                } => {
                                    break 'public PendingSend::inline(
                                        destination,
                                        receiver,
                                        argument_base,
                                        site,
                                        &[],
                                    )
                                }
                                CompactInstruction::SendPublic1 {
                                    destination,
                                    receiver,
                                    argument_base,
                                    source,
                                    site,
                                } => {
                                    break 'public PendingSend::inline(
                                        destination,
                                        receiver,
                                        argument_base,
                                        site,
                                        &[source],
                                    )
                                }
                                CompactInstruction::SendPublic2 {
                                    destination,
                                    receiver,
                                    argument_base,
                                    sources,
                                    site,
                                } => {
                                    break 'public PendingSend::inline(
                                        destination,
                                        receiver,
                                        argument_base,
                                        site,
                                        &sources,
                                    )
                                }
                                CompactInstruction::SendPublicWide { site } => {
                                    break 'public PendingSend::wide(&code, site)
                                }
                                // Decided as `SendPublic` is, but uncached: only the
                                // reference lowering emits it. Only the depth count treats a
                                // control-flow step's block activated for it differently
                                // (`Mutator::push_step`).
                                CompactInstruction::SendInlinable0 {
                                    destination,
                                    receiver,
                                    argument_base,
                                    site,
                                } => {
                                    break 'inlinable PendingSend::inline(
                                        destination,
                                        receiver,
                                        argument_base,
                                        site,
                                        &[],
                                    )
                                }
                                CompactInstruction::SendInlinable1 {
                                    destination,
                                    receiver,
                                    argument_base,
                                    source,
                                    site,
                                } => {
                                    break 'inlinable PendingSend::inline(
                                        destination,
                                        receiver,
                                        argument_base,
                                        site,
                                        &[source],
                                    )
                                }
                                CompactInstruction::SendInlinable2 {
                                    destination,
                                    receiver,
                                    argument_base,
                                    sources,
                                    site,
                                } => {
                                    break 'inlinable PendingSend::inline(
                                        destination,
                                        receiver,
                                        argument_base,
                                        site,
                                        &sources,
                                    )
                                }
                                CompactInstruction::SendInlinableWide { site } => {
                                    break 'inlinable PendingSend::wide(&code, site)
                                }
                                CompactInstruction::SendLexical0 {
                                    destination,
                                    argument_base,
                                    site,
                                } => {
                                    break 'lexical PendingSend::inline(
                                        destination,
                                        0,
                                        argument_base,
                                        site,
                                        &[],
                                    )
                                }
                                CompactInstruction::SendLexical1 {
                                    destination,
                                    argument_base,
                                    source,
                                    site,
                                } => {
                                    break 'lexical PendingSend::inline(
                                        destination,
                                        0,
                                        argument_base,
                                        site,
                                        &[source],
                                    )
                                }
                                CompactInstruction::SendLexical2 {
                                    destination,
                                    argument_base,
                                    sources,
                                    site,
                                } => {
                                    break 'lexical PendingSend::inline(
                                        destination,
                                        0,
                                        argument_base,
                                        site,
                                        &sources,
                                    )
                                }
                                CompactInstruction::SendLexicalWide { site } => {
                                    break 'lexical PendingSend::wide(&code, site)
                                }
                                CompactInstruction::SendSuper0 {
                                    destination,
                                    argument_base,
                                    site,
                                } => {
                                    break 'super_send PendingSend::inline(
                                        destination,
                                        0,
                                        argument_base,
                                        site,
                                        &[],
                                    )
                                }
                                CompactInstruction::SendSuper1 {
                                    destination,
                                    argument_base,
                                    source,
                                    site,
                                } => {
                                    break 'super_send PendingSend::inline(
                                        destination,
                                        0,
                                        argument_base,
                                        site,
                                        &[source],
                                    )
                                }
                                CompactInstruction::SendSuper2 {
                                    destination,
                                    argument_base,
                                    sources,
                                    site,
                                } => {
                                    break 'super_send PendingSend::inline(
                                        destination,
                                        0,
                                        argument_base,
                                        site,
                                        &sources,
                                    )
                                }
                                CompactInstruction::SendSuperWide { site } => {
                                    break 'super_send PendingSend::wide(&code, site)
                                }
                                CompactInstruction::SendMirror {
                                    destination,
                                    mirror,
                                    subject,
                                    primitive,
                                    site,
                                } => {
                                    let site = code.body().mirror_site(usize::from(site));
                                    match send_mirror(
                                        mutator,
                                        activation,
                                        register_base,
                                        program_counter,
                                        primitive,
                                        u16::from(mirror),
                                        u16::from(subject),
                                        site.argument_base(),
                                        site.arguments(),
                                        u16::from(destination),
                                        floor,
                                        &mut handoff,
                                    ) {
                                        Some(transfer) => break 'decided transfer,
                                        None => jump_to!(site.failure()),
                                    }
                                }
                                CompactInstruction::MakeClosure { destination, block } => {
                                    let block = code.body().block(usize::from(block));
                                    make_closure_into(
                                        mutator,
                                        activation,
                                        register_base,
                                        program_counter,
                                        feedback_base,
                                        u16::from(destination),
                                        block.code(),
                                        block.copied(),
                                    );
                                    next!()
                                }
                                CompactInstruction::MakeArray {
                                    destination,
                                    elements,
                                } => {
                                    make_array_into(
                                        mutator,
                                        register_base,
                                        u16::from(destination),
                                        code.body().register_list(usize::from(elements)),
                                    );
                                    next!()
                                }
                                CompactInstruction::MakeVector { destination, size } => {
                                    make_vector_into(
                                        mutator,
                                        register_base,
                                        u16::from(destination),
                                        size,
                                    );
                                    next!()
                                }
                                CompactInstruction::LoadVector {
                                    destination,
                                    vector,
                                    index,
                                } => match load_vector_into(
                                    mutator,
                                    activation,
                                    register_base,
                                    u16::from(destination),
                                    u16::from(vector),
                                    index,
                                ) {
                                    Ok(()) => next!(),
                                    Err(signal) => {
                                        break 'decided unwind(mutator, signal, floor, &mut handoff)
                                    }
                                },
                                CompactInstruction::StoreVector {
                                    vector,
                                    source,
                                    index,
                                } => match store_vector(
                                    &mut mutator.heap,
                                    activation,
                                    u16::from(vector),
                                    index,
                                    u16::from(source),
                                ) {
                                    Ok(()) => next!(),
                                    Err(signal) => {
                                        break 'decided unwind(mutator, signal, floor, &mut handoff)
                                    }
                                },
                                CompactInstruction::InitializeSlot { source, own_index } => {
                                    let source = u16::from(source);
                                    match initialize_slot(
                                        mutator,
                                        activation,
                                        register_base,
                                        source,
                                        own_index,
                                    ) {
                                        Ok(()) => next!(),
                                        // A future can reach a value's slot from outside any
                                        // simultaneous clause's own resolution -- passed in
                                        // as an ordinary argument, since a future is
                                        // indistinguishable from its value (§6.3.2, rule 3).
                                        // What belongs in the slot is what it stands for:
                                        // forced first if it is not resolved yet
                                        // (`value_for_a_value_slot`), then checked again
                                        // with the same accepted path. A refusal that is not
                                        // about a future stands.
                                        Err(signal) => {
                                            break 'decided initialize_slot_refused(
                                                mutator,
                                                activation,
                                                program_counter,
                                                source,
                                                own_index,
                                                signal,
                                                floor,
                                                &mut handoff,
                                            )
                                        }
                                    }
                                }
                                // A lazy slot's getter answers its field once it is filled;
                                // the field's address is `InitializeSlot`'s, and reading it
                                // is a `locals` helper's.
                                CompactInstruction::ReturnSlotIfFilled { own_index } => {
                                    match filled_own_slot(mutator, activation, own_index) {
                                        Ok(Some(value)) => {
                                            break 'decided finish(
                                                mutator,
                                                activation,
                                                value,
                                                floor,
                                                &mut handoff,
                                            )
                                        }
                                        Ok(None) => next!(),
                                        Err(signal) => {
                                            break 'decided unwind(
                                                mutator,
                                                signal,
                                                floor,
                                                &mut handoff,
                                            )
                                        }
                                    }
                                }
                                // §6.3.2's simultaneous slot clauses: one arm and one call,
                                // out of line (`simultaneous_clause_instruction`).
                                slot @ (CompactInstruction::MakeFuture { .. }
                                | CompactInstruction::ResolveFuture { .. }
                                | CompactInstruction::LoadOwnSlot { .. }
                                | CompactInstruction::InitializeSlotProvisionally {
                                    ..
                                }) => {
                                    break 'decided simultaneous_clause_instruction(
                                        mutator,
                                        activation,
                                        program_counter,
                                        slot,
                                        floor,
                                        &mut handoff,
                                    )
                                }
                                // The instructions of a compiled `new`: each
                                // one call, out of line; what each decides is
                                // `interpreter::instantiate`'s.
                                CompactInstruction::AllocateInstance { destination } => {
                                    match allocate_instance_into(
                                        mutator,
                                        activation,
                                        register_base,
                                        u16::from(destination),
                                    ) {
                                        Ok(()) => next!(),
                                        Err(signal) => {
                                            break 'decided unwind(
                                                mutator,
                                                signal,
                                                floor,
                                                &mut handoff,
                                            )
                                        }
                                    }
                                }
                                CompactInstruction::InitializeInstance {
                                    destination,
                                    instance,
                                } => {
                                    break 'decided initialize_instance(
                                        mutator,
                                        Sender {
                                            activation,
                                            register_base,
                                            destination: u16::from(destination),
                                            inlinable: false,
                                        },
                                        program_counter,
                                        code.argument_count,
                                        u16::from(instance),
                                        floor,
                                        &mut handoff,
                                    )
                                }
                                CompactInstruction::CheckAppliedClause { steps, selector } => {
                                    match mutator.check_applied_clause(activation, steps, selector)
                                    {
                                        Ok(()) => next!(),
                                        Err(signal) => {
                                            break 'decided unwind(
                                                mutator,
                                                signal,
                                                floor,
                                                &mut handoff,
                                            )
                                        }
                                    }
                                }
                                CompactInstruction::InitializeSuperclass { destination, site } => {
                                    let site = code.body().superclass_site(usize::from(site));
                                    break 'decided initialize_superclass(
                                        mutator,
                                        Sender {
                                            activation,
                                            register_base,
                                            destination: u16::from(destination),
                                            inlinable: false,
                                        },
                                        program_counter,
                                        site.steps(),
                                        site.selector(),
                                        site.argument_base(),
                                        site.arguments(),
                                        floor,
                                        &mut handoff,
                                    );
                                }
                                CompactInstruction::InitializeAppliedLevel {
                                    destination,
                                    argument_base,
                                    steps,
                                    arguments,
                                } => {
                                    break 'decided initialize_applied_level(
                                        mutator,
                                        Sender {
                                            activation,
                                            register_base,
                                            destination: u16::from(destination),
                                            inlinable: false,
                                        },
                                        program_counter,
                                        steps,
                                        u16::from(argument_base),
                                        code.body().register_list(usize::from(arguments)),
                                        floor,
                                        &mut handoff,
                                    )
                                }
                                CompactInstruction::Return { register }
                                | CompactInstruction::ImplicitReturn { register } => {
                                    break 'decided return_from(
                                        mutator,
                                        activation,
                                        register_base,
                                        u16::from(register),
                                        floor,
                                        &mut handoff,
                                    )
                                }
                                // An inlined loop's condition answered no Boolean: the error
                                // is `unwind`'s to build, as the native loop's is.
                                CompactInstruction::SignalNonBoolean { register } => {
                                    let value =
                                        mutator.heap.register(register_base, u16::from(register));
                                    let error = non_boolean_condition(&mutator.heap, value);
                                    break 'decided unwind(
                                        mutator,
                                        Signal::Error(error),
                                        floor,
                                        &mut handoff,
                                    );
                                }
                                CompactInstruction::NonLocalReturn { register } => {
                                    break 'decided non_local_return(
                                        mutator,
                                        activation,
                                        register_base,
                                        program_counter,
                                        u16::from(register),
                                        floor,
                                        &mut handoff,
                                    )
                                }
                                CompactInstruction::Jump { target } => jump_to!(target),
                                CompactInstruction::JumpIfNotBoolean { register, target } => {
                                    if boolean_in(mutator, register_base, u16::from(register))
                                        .is_some()
                                    {
                                        next!()
                                    }
                                    jump_to!(target)
                                }
                                CompactInstruction::BranchIfFalse { register, target } => {
                                    if boolean_in(mutator, register_base, u16::from(register))
                                        == Some(false)
                                    {
                                        jump_to!(target)
                                    }
                                    next!()
                                }
                                CompactInstruction::BranchIfTrue { register, target } => {
                                    if boolean_in(mutator, register_base, u16::from(register))
                                        == Some(true)
                                    {
                                        jump_to!(target)
                                    }
                                    next!()
                                }
                                CompactInstruction::IntegerAdd {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::Add,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::IntegerSubtract {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::Subtract,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::IntegerMultiply {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::Multiply,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::IntegerFloorDivide {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::FloorDivide,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::IntegerLess {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::Less,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::IntegerGreater {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::Greater,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::IntegerLessOrEqual {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::LessOrEqual,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::IntegerGreaterOrEqual {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::GreaterOrEqual,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::IntegerEqual {
                                    destination,
                                    left,
                                    right,
                                    target,
                                } => integer!(
                                    IntegerOperation::Equal,
                                    destination,
                                    left,
                                    right,
                                    target
                                ),
                                CompactInstruction::Wide { index } => match run_wide(
                                    mutator,
                                    code.body().wide(index as usize),
                                    code.is_declaration_code(),
                                    activation,
                                    register_base,
                                    program_counter,
                                    feedback_base,
                                    floor,
                                    &mut handoff,
                                ) {
                                    WideOutcome::Next => next!(),
                                    WideOutcome::Jump(target) => jump_to!(target),
                                    WideOutcome::Transfer(transfer) => break 'decided transfer,
                                },
                            }
                        };
                        break 'decided send_path!(inlinable, Inlinable);
                    };
                    break 'decided send_path!(super_send, Super);
                };
                break 'decided send_path!(public, Public);
            };
            send_path!(lexical, Lexical)
        };
        match transfer {
            Transfer::Next => program_counter += 1,
            Transfer::Switched => {
                activation = handoff.activation;
                code = handoff
                    .code
                    .take()
                    .expect("internal: a switch names its code");
                program_counter = handoff.program_counter;
                feedback_base = handoff.feedback_base;
                register_base = mutator.heap.activation_register_base(activation);
                if mutator.heap.safepoint_requested() {
                    break 'run;
                }
            }
            Transfer::Exited => {
                return Leave::Exit(
                    handoff
                        .exit
                        .take()
                        .expect("internal: an exit carries its result"),
                );
            }
        }
    }
    // A safepoint is requested, at a switch or a backward jump: the frame
    // the stretch stands at, written back for the next one -- its program
    // counter into its stack frame too, where the safepoint finds it when
    // it suspends the activation into its frame.
    mutator
        .heap
        .activation_set_program_counter(activation, program_counter);
    *frame = FrameRegisters {
        activation,
        code,
        program_counter,
        feedback_base,
        register_base,
    };
    Leave::Safepoint
}

const WIDE_SEND: &str = "internal: a wide send's site holds its registers";

/// A send an arm gathered: what to send and where its value goes. The arms
/// of one kind differ only in where their argument registers are; they
/// break out of the kind's labelled block to its one path, so that each
/// kind's cache hit path is inlined into `vm::run` once. It carries no kind:
/// a single join for all four would need one, and compiles to a stack
/// aggregate and a second jump table on it.
struct PendingSend<'code> {
    destination: u16,
    /// `SendPublic`'s and `SendInlinable`'s receiver register.
    receiver: u16,
    site: u32,
    /// The first argument slot.
    argument_base: u16,
    /// The register each argument slot is copied from.
    arguments: ArgumentRegisters<'code>,
}

/// A send's argument sources, handed to `send` by value so that an
/// eight-byte form's never reach memory.
#[derive(Clone, Copy)]
enum ArgumentRegisters<'code> {
    /// Up to two, from the slot.
    Inline { count: u8, registers: [u16; 2] },
    /// From the site's `WideSend`.
    Listed(&'code [u16]),
}

impl<'code> ArgumentRegisters<'code> {
    #[inline(always)]
    fn as_slice(&self) -> &[u16] {
        match self {
            ArgumentRegisters::Inline { count, registers } => &registers[..usize::from(*count)],
            ArgumentRegisters::Listed(listed) => listed,
        }
    }
}

impl<'code> PendingSend<'code> {
    /// A send whose registers are all in its slot.
    #[inline(always)]
    fn inline(
        destination: u8,
        receiver: u8,
        argument_base: u8,
        site: u16,
        arguments: &[u8],
    ) -> PendingSend<'code> {
        let mut registers = [0; 2];
        for (register, argument) in registers.iter_mut().zip(arguments) {
            *register = u16::from(*argument);
        }
        PendingSend {
            destination: u16::from(destination),
            receiver: u16::from(receiver),
            site: u32::from(site),
            argument_base: u16::from(argument_base),
            arguments: ArgumentRegisters::Inline {
                count: arguments.len() as u8,
                registers,
            },
        }
    }

    /// A send whose registers are in its site's `WideSend`.
    #[inline(always)]
    fn wide(code: &'code CompiledCode, site: u32) -> PendingSend<'code> {
        let wide = code
            .body()
            .send_site(site as usize)
            .wide()
            .expect(WIDE_SEND);
        PendingSend {
            destination: wide.destination(),
            receiver: wide.receiver(),
            site,
            argument_base: wide.argument_base(),
            arguments: ArgumentRegisters::Listed(wide.arguments()),
        }
    }
}

/// How the loop goes on after `run_wide`.
enum WideOutcome {
    Next,
    Jump(u32),
    Transfer(Transfer),
}

/// An instruction with no eight-byte form, run out of line so that the rare
/// wide operands cost `vm::run` one call, not a second copy of every arm. It
/// calls the same effect functions the narrow arms do.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn run_wide<'gc>(
    mutator: &mut Mutator<'gc>,
    instruction: &Instruction,
    declaration_code: bool,
    activation: ActivationReference,
    register_base: RegisterBase,
    program_counter: usize,
    feedback_base: FeedbackBase,
    floor: usize,
    handoff: &mut Handoff,
) -> WideOutcome {
    let transfer = match instruction {
        Instruction::LoadLiteral {
            destination,
            literal,
        } => match load_literal_into(mutator, activation, register_base, *destination, literal) {
            Ok(()) => return WideOutcome::Next,
            Err(signal) => unwind(mutator, signal, floor, handoff),
        },
        Instruction::LoadSelf { destination } => {
            load_self_into(mutator, activation, register_base, *destination);
            return WideOutcome::Next;
        }
        Instruction::LoadEnclosingObject { destination, depth } => {
            load_enclosing_object_into(
                mutator,
                activation,
                register_base,
                *destination,
                *depth,
                declaration_code,
            );
            return WideOutcome::Next;
        }
        Instruction::Move {
            destination,
            source,
        }
        | Instruction::StoreLocal {
            source,
            slot: destination,
        } => {
            copy_register(mutator, register_base, *destination, *source);
            return WideOutcome::Next;
        }
        Instruction::LoadOuterLocal {
            destination,
            depth,
            index,
        } => {
            load_outer_local_into(
                mutator,
                activation,
                register_base,
                *destination,
                *depth,
                *index,
            );
            return WideOutcome::Next;
        }
        Instruction::StoreOuterLocal {
            source,
            depth,
            index,
        } => {
            store_outer_local_from(mutator, activation, register_base, *source, *depth, *index);
            return WideOutcome::Next;
        }
        Instruction::SendPublic { .. }
        | Instruction::SendInlinable { .. }
        | Instruction::SendLexical { .. }
        | Instruction::SendSuper { .. } => {
            unreachable!("internal: a send is never encoded through Wide")
        }
        Instruction::SendMirror {
            primitive,
            mirror_register,
            subject_register,
            argument_base,
            argument_sources,
            destination,
            failure,
        } => match send_mirror(
            mutator,
            activation,
            register_base,
            program_counter,
            *primitive,
            *mirror_register,
            *subject_register,
            *argument_base,
            argument_sources,
            *destination,
            floor,
            handoff,
        ) {
            Some(transfer) => transfer,
            None => return WideOutcome::Jump(*failure),
        },
        Instruction::MakeClosure {
            destination,
            code: block,
            copied,
            ..
        } => {
            make_closure_into(
                mutator,
                activation,
                register_base,
                program_counter,
                feedback_base,
                *destination,
                block,
                copied,
            );
            return WideOutcome::Next;
        }
        Instruction::MakeArray {
            destination,
            element_registers,
        } => {
            make_array_into(mutator, register_base, *destination, element_registers);
            return WideOutcome::Next;
        }
        Instruction::MakeVector { destination, size } => {
            make_vector_into(mutator, register_base, *destination, *size);
            return WideOutcome::Next;
        }
        Instruction::LoadVector {
            destination,
            vector,
            index,
        } => match load_vector_into(
            mutator,
            activation,
            register_base,
            *destination,
            *vector,
            *index,
        ) {
            Ok(()) => return WideOutcome::Next,
            Err(signal) => unwind(mutator, signal, floor, handoff),
        },
        Instruction::StoreVector {
            vector,
            index,
            source,
        } => match store_vector(&mut mutator.heap, activation, *vector, *index, *source) {
            Ok(()) => return WideOutcome::Next,
            Err(signal) => unwind(mutator, signal, floor, handoff),
        },
        Instruction::InitializeSlot { source, own_index } => {
            match initialize_slot(mutator, activation, register_base, *source, *own_index) {
                Ok(()) => return WideOutcome::Next,
                Err(signal) => initialize_slot_refused(
                    mutator,
                    activation,
                    program_counter,
                    *source,
                    *own_index,
                    signal,
                    floor,
                    handoff,
                ),
            }
        }
        Instruction::ReturnSlotIfFilled { own_index } => {
            match filled_own_slot(mutator, activation, *own_index) {
                Ok(Some(value)) => finish(mutator, activation, value, floor, handoff),
                Ok(None) => return WideOutcome::Next,
                Err(signal) => unwind(mutator, signal, floor, handoff),
            }
        }
        Instruction::MakeFuture {
            destination,
            closure,
        } => make_future(mutator, activation, *destination, *closure, floor, handoff),
        Instruction::ResolveFuture {
            destination,
            source,
        } => resolve_future(
            mutator,
            activation,
            program_counter,
            *destination,
            *source,
            floor,
            handoff,
        ),
        Instruction::LoadOwnSlot {
            destination,
            own_index,
        } => load_own_slot_into(
            mutator,
            activation,
            *destination,
            *own_index,
            floor,
            handoff,
        ),
        Instruction::InitializeSlotProvisionally { source, own_index } => {
            initialize_slot_provisionally(mutator, activation, *source, *own_index, floor, handoff)
        }
        Instruction::Return { register } | Instruction::ImplicitReturn { register } => return_from(
            mutator,
            activation,
            register_base,
            *register,
            floor,
            handoff,
        ),
        Instruction::NonLocalReturn { register } => non_local_return(
            mutator,
            activation,
            register_base,
            program_counter,
            *register,
            floor,
            handoff,
        ),
        Instruction::SignalNonBoolean { register } => {
            let value = mutator.heap.register(register_base, *register);
            let error = non_boolean_condition(&mutator.heap, value);
            unwind(mutator, Signal::Error(error), floor, handoff)
        }
        Instruction::Jump { target } => return WideOutcome::Jump(*target),
        Instruction::JumpIfNotBoolean { register, target } => {
            return match boolean_in(mutator, register_base, *register) {
                Some(_) => WideOutcome::Next,
                None => WideOutcome::Jump(*target),
            };
        }
        Instruction::BranchIfFalse { register, target } => {
            return match boolean_in(mutator, register_base, *register) {
                Some(false) => WideOutcome::Jump(*target),
                _ => WideOutcome::Next,
            };
        }
        Instruction::BranchIfTrue { register, target } => {
            return match boolean_in(mutator, register_base, *register) {
                Some(true) => WideOutcome::Jump(*target),
                _ => WideOutcome::Next,
            };
        }
        Instruction::IntegerBinary {
            operation,
            destination,
            left,
            right,
            target,
        } => {
            return if integer_into(
                mutator,
                register_base,
                *operation,
                *destination,
                *left,
                *right,
            ) {
                WideOutcome::Next
            } else {
                WideOutcome::Jump(*target)
            };
        }
        Instruction::AllocateInstance { destination } => {
            match allocate_instance_into(mutator, activation, register_base, *destination) {
                Ok(()) => return WideOutcome::Next,
                Err(signal) => unwind(mutator, signal, floor, handoff),
            }
        }
        Instruction::InitializeInstance {
            instance,
            destination,
        } => {
            let argument_count = mutator.heap.activation_code(activation).argument_count;
            initialize_instance(
                mutator,
                Sender {
                    activation,
                    register_base,
                    destination: *destination,
                    inlinable: false,
                },
                program_counter,
                argument_count,
                *instance,
                floor,
                handoff,
            )
        }
        Instruction::CheckAppliedClause { steps, selector } => {
            match mutator.check_applied_clause(activation, *steps, *selector) {
                Ok(()) => return WideOutcome::Next,
                Err(signal) => unwind(mutator, signal, floor, handoff),
            }
        }
        Instruction::InitializeSuperclass {
            steps,
            selector,
            argument_base,
            argument_sources,
            destination,
        } => initialize_superclass(
            mutator,
            Sender {
                activation,
                register_base,
                destination: *destination,
                inlinable: false,
            },
            program_counter,
            *steps,
            *selector,
            *argument_base,
            argument_sources,
            floor,
            handoff,
        ),
        Instruction::InitializeAppliedLevel {
            steps,
            argument_base,
            argument_sources,
            destination,
        } => initialize_applied_level(
            mutator,
            Sender {
                activation,
                register_base,
                destination: *destination,
                inlinable: false,
            },
            program_counter,
            *steps,
            *argument_base,
            argument_sources,
            floor,
            handoff,
        ),
    };
    WideOutcome::Transfer(transfer)
}

// The effect of each instruction, written once over `u16` operands: the
// narrow arm widens its bytes and calls it, and `run_wide` calls it with the
// `Instruction`'s own.

#[inline(always)]
fn load_literal_into<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    register_base: RegisterBase,
    destination: u16,
    literal: &Literal,
) -> Result<(), Signal<'gc>> {
    let value = make_value(mutator, activation, literal)?;
    mutator.heap.set_register(register_base, destination, value);
    Ok(())
}

#[inline(always)]
fn load_self_into(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    destination: u16,
) {
    let value = mutator.heap.activation_receiver(activation);
    mutator.heap.set_register(register_base, destination, value);
}

#[inline(always)]
fn load_enclosing_object_into(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    destination: u16,
    depth: u16,
    declaration_code: bool,
) {
    let value =
        crate::interpreter::sends::enclosing_object(mutator, activation, depth, declaration_code);
    mutator.heap.set_register(register_base, destination, value);
}

#[inline(always)]
fn copy_register(
    mutator: &mut Mutator<'_>,
    register_base: RegisterBase,
    destination: u16,
    source: u16,
) {
    let value = mutator.heap.register(register_base, source);
    mutator.heap.set_register(register_base, destination, value);
}

#[inline(always)]
fn load_outer_local_into(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    destination: u16,
    depth: u16,
    index: u16,
) {
    let value = read_outer_local(&mutator.heap, activation, depth, index);
    mutator.heap.set_register(register_base, destination, value);
}

#[inline(always)]
fn store_outer_local_from(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    source: u16,
    depth: u16,
    index: u16,
) {
    let value = mutator.heap.register(register_base, source);
    write_outer_local(&mut mutator.heap, activation, depth, index, value);
}

/// A send to the VM mirror with its failure block inline: the leaf runs, or
/// declines and the block runs at the failure target, which the caller
/// jumps to when this answers `None`. What the leaf decided is delivered as
/// any send's decision is. Which leaf, and whether the receiver is the
/// mirror, is `interpreter::mirror`'s.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn send_mirror(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    program_counter: usize,
    primitive: MirrorPrimitive,
    mirror_register: u16,
    subject_register: u16,
    argument_base: u16,
    argument_sources: &[u16],
    destination: u16,
    floor: usize,
    handoff: &mut Handoff,
) -> Option<Transfer> {
    let mirror = mutator.heap.register(register_base, mirror_register);
    let subject = mutator.heap.register(register_base, subject_register);
    let slots = place_arguments(mutator, register_base, argument_base, argument_sources);
    let arguments = Arguments::Slots(ArgumentSlots::new(
        register_base,
        slots.start,
        slots.end - slots.start,
    ));
    mutator
        .heap
        .activation_set_program_counter(activation, program_counter);
    // `None`, the leaf declined, and the caller jumps to the failure
    // target. The closure is forced inline: once the arm moved into this
    // function, LLVM left it out of line, a call on every mirror send that
    // the arm written in `vm::run` did not pay.
    crate::interpreter::mirror::send_inlined(mutator, primitive, mirror, subject, arguments).map(
        #[inline(always)]
        |decided| {
            deliver(
                mutator,
                Sender {
                    activation,
                    register_base,
                    destination,
                    inlinable: false,
                },
                decided,
                arguments,
                floor,
                handoff,
            )
        },
    )
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn make_closure_into(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    program_counter: usize,
    feedback_base: FeedbackBase,
    destination: u16,
    block: &Rc<CompiledCode>,
    copied: &[u16],
) {
    // Stored before reifying, so the frame `reify` builds starts from this
    // program counter.
    mutator
        .heap
        .activation_set_program_counter(activation, program_counter);
    let captured_frame = mutator.heap.reify(activation);
    // A closure that reads enclosing registers by reference -- the
    // reference lowering's, and an optimized fallback closure -- is what
    // makes this activation's registers outlive it.
    if block.outer_reach > 0 {
        mutator.heap.retain_registers(activation);
    }
    let value = Value::object(mutator.new_closure_from_registers(
        block,
        captured_frame,
        // A block belongs to its enclosing method's family: the running
        // activation's own base, already in hand here, so this costs no
        // extra load.
        feedback_base,
        register_base,
        copied,
    ));
    mutator.heap.set_register(register_base, destination, value);
}

#[inline(always)]
fn make_array_into(
    mutator: &mut Mutator<'_>,
    register_base: RegisterBase,
    destination: u16,
    element_registers: &[u16],
) {
    let value = Value::object(mutator.new_array_from_registers(register_base, element_registers));
    mutator.heap.set_register(register_base, destination, value);
}

#[inline(always)]
fn make_vector_into(
    mutator: &mut Mutator<'_>,
    register_base: RegisterBase,
    destination: u16,
    size: u16,
) {
    let value = Value::object(mutator.new_nil_array(usize::from(size)));
    mutator.heap.set_register(register_base, destination, value);
}

#[inline(always)]
fn load_vector_into<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    register_base: RegisterBase,
    destination: u16,
    vector: u16,
    index: u16,
) -> Result<(), Signal<'gc>> {
    let value = load_vector(&mutator.heap, activation, vector, index)?;
    mutator.heap.set_register(register_base, destination, value);
    Ok(())
}

/// `own_index` is relative to the executing mixin's own slots; the
/// activation's slot base, that of the application it was entered at
/// (`Heap::activation_slot_base`), turns it into this instance's absolute
/// field index.
#[inline(always)]
fn initialize_slot<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    register_base: RegisterBase,
    source: u16,
    own_index: u32,
) -> Result<(), Signal<'gc>> {
    check_own_slots(mutator, activation)?;
    let value = mutator.heap.register(register_base, source);
    let receiver = mutator.heap.activation_receiver(activation);
    let base = mutator.heap.activation_slot_base(activation);
    // §3.1.1's rule about what a value's slot may hold is decided in
    // `interpreter/`, as every error is; this only calls it.
    check_value_slot(mutator, activation, receiver, own_index as usize, value)?;
    store_instance_field(
        &mut mutator.heap,
        receiver,
        base + own_index as usize,
        value,
    );
    Ok(())
}

#[inline(always)]
fn return_from(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    register: u16,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let value = mutator.heap.register(register_base, register);
    finish(mutator, activation, value, floor, handoff)
}

#[inline(always)]
fn non_local_return(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    program_counter: usize,
    register: u16,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let value = mutator.heap.register(register_base, register);
    mutator
        .heap
        .activation_set_program_counter(activation, program_counter);
    let signal = mutator.raise_return(activation, value);
    unwind(mutator, signal, floor, handoff)
}

#[inline(always)]
fn boolean_in(mutator: &Mutator<'_>, register_base: RegisterBase, register: u16) -> Option<bool> {
    mutator.heap.register(register_base, register).as_boolean()
}

/// The inlined fast path for an integer leaf: whether it wrote its result
/// to `destination`. Not dispatch: it either computes the one answer the
/// primitive would have computed, or declines and the caller jumps to the
/// ordinary send standing at the instruction's target. Every case it
/// declines -- a non-integer operand, a result that does not fit, a zero
/// divisor -- is that send's primitive to decide, so nothing is promoted
/// and no error or error text is raised here. It cannot fail for any
/// operand, which is what lets a decoded image reach it safely.
#[inline(always)]
fn integer_into(
    mutator: &mut Mutator<'_>,
    register_base: RegisterBase,
    operation: IntegerOperation,
    destination: u16,
    left: u16,
    right: u16,
) -> bool {
    let left_value = mutator.heap.register(register_base, left);
    let right_value = mutator.heap.register(register_base, right);
    // Each `small_integer_*` answers `None` for a non-integer operand too,
    // so the one `match` below declines every case. A zero divisor and
    // `SMALL_INTEGER_MIN // -1` decline in `small_integer_floor_quotient`.
    let result = match operation {
        IntegerOperation::Add => left_value.small_integer_sum(right_value),
        IntegerOperation::Subtract => left_value.small_integer_difference(right_value),
        IntegerOperation::Multiply => left_value.small_integer_product(right_value),
        IntegerOperation::FloorDivide => left_value.small_integer_floor_quotient(right_value),
        IntegerOperation::Less => left_value
            .small_integer_order(right_value)
            .map(|order| Value::boolean(order.is_lt())),
        IntegerOperation::Greater => left_value
            .small_integer_order(right_value)
            .map(|order| Value::boolean(order.is_gt())),
        IntegerOperation::LessOrEqual => left_value
            .small_integer_order(right_value)
            .map(|order| Value::boolean(order.is_le())),
        IntegerOperation::GreaterOrEqual => left_value
            .small_integer_order(right_value)
            .map(|order| Value::boolean(order.is_ge())),
        IntegerOperation::Equal => left_value
            .small_integer_order(right_value)
            .map(|order| Value::boolean(order.is_eq())),
    };
    match result {
        Some(value) => {
            mutator.heap.set_register(register_base, destination, value);
            true
        }
        None => false,
    }
}

/// The sending frame, and where and how its send's decision is delivered.
#[derive(Clone, Copy)]
struct Sender {
    activation: ActivationReference,
    register_base: RegisterBase,
    destination: u16,
    /// Whether the instruction was a `SendInlinable`, which `push_step`
    /// hands to the depth count along with the sender, `activation`.
    inlinable: bool,
}

/// A send's arguments put in their slots: every source that is not already
/// its slot copied into it, in order; answers the slots. A named local
/// passed as an argument is copied here, by the send itself, rather than by
/// a `Move` before it (`docs/design/execution.md`, "Planned: overlapping
/// register windows"). Running it again, as a send re-run after forcing a
/// future is, reads the sources afresh, as the send read its arguments
/// before; neither lowering makes a source another argument's slot, so the
/// order of the copies never matters.
#[inline(always)]
fn place_arguments(
    mutator: &mut Mutator<'_>,
    register_base: RegisterBase,
    argument_base: u16,
    argument_sources: &[u16],
) -> Range<u16> {
    let mut slot = argument_base;
    for source in argument_sources {
        if *source != slot {
            copy_register(mutator, register_base, slot, *source);
        }
        slot += 1;
    }
    argument_base..slot
}

/// A send: the arguments, already in their slots (`place_arguments`) and
/// read there, `program_counter` stored, the decision asked for, and what
/// was decided run -- a value written to the sender's `destination`, or an
/// activation pushed and switched to, its window filled straight from the
/// slots. Nothing is gathered in between (`interpreter::Arguments`). The
/// instructions that invoke a factory's or an initializer's
/// initializer go the same way: their decision is the body to run on the
/// instance, or the instance itself when nothing runs.
fn send<'gc>(
    mutator: &mut Mutator<'gc>,
    sender: Sender,
    program_counter: usize,
    slots: Range<u16>,
    floor: usize,
    handoff: &mut Handoff,
    decide: impl FnOnce(&mut Mutator<'gc>, Arguments<'_, 'gc>) -> Result<Step<'gc>, Signal<'gc>>,
) -> Transfer {
    let arguments = Arguments::Slots(ArgumentSlots::new(
        sender.register_base,
        slots.start,
        slots.end - slots.start,
    ));
    mutator
        .heap
        .activation_set_program_counter(sender.activation, program_counter);
    let decided = decide(mutator, arguments);
    deliver(mutator, sender, decided, arguments, floor, handoff)
}

/// What a send decided, run: a value written to the sender's `destination`,
/// or an activation pushed and switched to, its window filled from
/// `send_arguments`, the send's slots. Shared by `send` and `SendMirror`, so a mirror leaf's
/// decision is delivered exactly as any send's is.
#[inline(always)]
fn deliver<'gc>(
    mutator: &mut Mutator<'gc>,
    sender: Sender,
    decided: Result<Step<'gc>, Signal<'gc>>,
    send_arguments: Arguments<'_, 'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    match decided {
        Ok(Step::Value(value)) => {
            mutator
                .heap
                .set_register(sender.register_base, sender.destination, value);
            Transfer::Next
        }
        Ok(step) => match mutator.push_sent(
            step,
            send_arguments,
            Some(sender.activation),
            sender.inlinable,
        ) {
            Ok((callee, program_counter)) => handoff.switch_to(mutator, callee, program_counter),
            Err(signal) => unwind(mutator, signal, floor, handoff),
        },
        Err(signal) => unwind(mutator, signal, floor, handoff),
    }
}

/// `activation` has returned `value`: pop it and do what its continuation says.
/// A value -- the usual case -- goes to the send in the caller, which resumes,
/// or out of `run` when there is no caller in this run; a native loop's
/// continuation may instead decide another activation -- a native loop's next
/// block, or the `valueOf:` that runs a body which is no block -- which runs
/// in the popped activation's place. `Mutator::resume` applies the
/// continuation and
/// `Mutator::push_continuation` pushes what it decided, depth count included;
/// the loop only forwards the popped frame's flag. A future's forcing
/// (`Mutator::future_computed`) may also run the caller's stopped
/// instruction again (`reexecute`) rather than resume after it. A caller
/// suspended into its frame is resumed first (`return_to_suspended`).
fn finish<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    value: Value<'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let popped = mutator.heap.pop_activation(activation);
    apply_continuation(mutator, popped.then, popped.counted, value, floor, handoff)
}

/// What `finish` does once the frame is popped: `then`, the popped frame's
/// continuation, applied to `value`, and `counted`, its flag, handed on to
/// whatever the continuation pushes.
#[inline(always)]
fn apply_continuation<'gc>(
    mutator: &mut Mutator<'gc>,
    then: Continuation<'gc>,
    counted: bool,
    value: Value<'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let value = match then {
        // `Deliver` owns nothing, so forgetting it is dropping it. Written
        // out because otherwise the continuation is dropped at the end of
        // the function through `Continuation`'s drop
        // glue, which the boxed variants made large enough to stay out of
        // line: a call on every return, where the tag check used to be
        // inline. Forgetting it here measured about 1% faster on the
        // benchmarks that return most.
        deliver @ Continuation::Deliver => {
            std::mem::forget(deliver);
            value
        }
        // The caller is suspended into its frame: out of line, since only
        // a run that was suspended ever returns this way.
        Continuation::ReturnToSuspended(suspended) => {
            return return_to_suspended(mutator, *suspended, counted, value, floor, handoff);
        }
        // A future's computation returned: what happens next is
        // `interpreter::future`'s to decide, and the loop carries it out.
        Continuation::Future(forcing) => match mutator.future_computed(*forcing, value) {
            Ok(FutureOutcome::Deliver(value)) => value,
            Ok(FutureOutcome::Reexecute) => return reexecute(mutator, floor, handoff),
            Ok(FutureOutcome::Force(step)) => {
                return match mutator.push_continuation(step, counted) {
                    Ok(next) => handoff.switch_to(mutator, next, 0),
                    Err(signal) => unwind(mutator, signal, floor, handoff),
                };
            }
            Err(signal) => return unwind(mutator, signal, floor, handoff),
        },
        then => match mutator.resume(then, value) {
            Ok(Step::Value(value)) => value,
            Ok(step) => {
                return match mutator.push_continuation(step, counted) {
                    Ok(next) => handoff.switch_to(mutator, next, 0),
                    Err(signal) => unwind(mutator, signal, floor, handoff),
                };
            }
            Err(signal) => return unwind(mutator, signal, floor, handoff),
        },
    };
    if mutator.heap.activation_stack_top() == floor {
        return handoff.exit(mutator.to_host(Ok(value)));
    }
    let caller = mutator
        .heap
        .top_activation()
        .expect("internal: an activation above the run's floor has a caller");
    resume_caller(mutator, caller, value, handoff)
}

/// The popped activation returns to a sender suspended into its frame
/// (`Heap::suspend_activations`): the sender is resumed onto the stack,
/// and the continuation the popped frame carried inside the wrapper is
/// applied as any popped one is, so that from here on everything proceeds
/// as if the sender had been on the stack all along -- the caller a value
/// is delivered to, the activation a continuation pushes above, the
/// instruction `reexecute` runs again.
#[cold]
#[inline(never)]
fn return_to_suspended<'gc>(
    mutator: &mut Mutator<'gc>,
    suspended: ReturnToSuspended<'gc>,
    counted: bool,
    value: Value<'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let ReturnToSuspended { sender, then } = suspended;
    mutator.resume_frame(sender);
    apply_continuation(mutator, then, counted, value, floor, handoff)
}

/// Writes a callee's value into the send `caller` is stopped at, and resumes
/// `caller` after it.
fn resume_caller<'gc>(
    mutator: &mut Mutator<'gc>,
    caller: ActivationReference,
    value: Value<'gc>,
    handoff: &mut Handoff,
) -> Transfer {
    let code = mutator.heap.activation_code(caller);
    let program_counter = mutator.heap.activation_program_counter(caller);
    write_local(
        &mut mutator.heap,
        caller,
        send_destination(&code, program_counter),
        value,
    );
    handoff.switch_with_code(mutator, caller, code, program_counter + 1)
}

/// The instruction the popped activation's caller is stopped at, run again
/// rather than resumed after: it met an unresolved future, which is now
/// resolved (`interpreter::future`, `AfterForcing::Reexecute`). It wrote
/// nothing before it forced the future, so running it again is running it
/// once. A host's send has no instruction to run again; a future it passes
/// in straight to a leaf ends the run.
fn reexecute<'gc>(mutator: &mut Mutator<'gc>, floor: usize, handoff: &mut Handoff) -> Transfer {
    if mutator.heap.activation_stack_top() == floor {
        let error = crate::interpreter::future::host_passed_a_future();
        return handoff.exit(mutator.to_host(Err(error)));
    }
    let caller = mutator
        .heap
        .top_activation()
        .expect("internal: an activation above the run's floor has a caller");
    let program_counter = mutator.heap.activation_program_counter(caller);
    handoff.switch_to(mutator, caller, program_counter)
}

// The instructions of a compiled `new` (`docs/design/object-model.md`,
// "Instantiation"): the factory's allocation and its initializer, and an
// initializer's checks and invocations. Out of line, each a call the arm
// makes and nothing more; the three that may run a body go through `send`,
// so that what they decide is pushed, and its value delivered to
// `destination`, as a send's is.

/// `AllocateInstance`: a fresh instance of the factory's receiver.
#[inline(never)]
fn allocate_instance_into<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    register_base: RegisterBase,
    destination: u16,
) -> Result<(), Signal<'gc>> {
    let instance = mutator.allocate_instance(activation)?;
    mutator
        .heap
        .set_register(register_base, destination, instance);
    Ok(())
}

/// `InitializeInstance`: the receiver's initializer on the instance in
/// `instance`, with the running body's own arguments, registers
/// `0..argument_count`.
#[inline(never)]
fn initialize_instance(
    mutator: &mut Mutator<'_>,
    sender: Sender,
    program_counter: usize,
    argument_count: u16,
    instance: u16,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let instance = mutator.heap.register(sender.register_base, instance);
    send(
        mutator,
        sender,
        program_counter,
        0..argument_count,
        floor,
        handoff,
        |mutator, arguments| mutator.initialize_instance(sender.activation, instance, arguments),
    )
}

/// `InitializeSuperclass`: the superclass's initializer on `self`.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn initialize_superclass(
    mutator: &mut Mutator<'_>,
    sender: Sender,
    program_counter: usize,
    steps: u16,
    selector: Symbol,
    argument_base: u16,
    argument_sources: &[u16],
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let slots = place_arguments(
        mutator,
        sender.register_base,
        argument_base,
        argument_sources,
    );
    send(
        mutator,
        sender,
        program_counter,
        slots,
        floor,
        handoff,
        |mutator, arguments| {
            mutator.decide_superclass_initializer(sender.activation, steps, selector, arguments)
        },
    )
}

/// `InitializeAppliedLevel`: an applied level's own part on `self`.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn initialize_applied_level(
    mutator: &mut Mutator<'_>,
    sender: Sender,
    program_counter: usize,
    steps: u16,
    argument_base: u16,
    argument_sources: &[u16],
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let slots = place_arguments(
        mutator,
        sender.register_base,
        argument_base,
        argument_sources,
    );
    send(
        mutator,
        sender,
        program_counter,
        slots,
        floor,
        handoff,
        |mutator, arguments| {
            mutator.decide_applied_level_initializer(sender.activation, steps, arguments)
        },
    )
}

// The arms of §6.3.2's simultaneous slot clauses -- `InitializeSlot`'s
// refusal branch and the four instructions only such a clause emits -- run
// out of line, each arm a single call that passes no more than an unwind
// does: inlined into `vm::run` they grew the loop every instruction runs
// through. They are not marked `#[cold]`, which measured slower still on
// the sending benchmarks (`docs/design/execution.md`, "What simultaneous
// slots cost").

/// `InitializeSlot` refused a value's slot: if the value is a future, what
/// it stands for belongs there, stored and checked again, or forced first
/// and the instruction run again (`future::value_for_a_value_slot`).
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn initialize_slot_refused<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: usize,
    source: u16,
    own_index: u32,
    signal: Signal<'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    // A receiver that no longer has the slot is refused whatever the
    // value -- `signal` is that refusal, built once -- and only a refusal
    // of the value itself can be about a future.
    let receiver = mutator.heap.activation_receiver(activation);
    if !receiver_keeps_entered_level(mutator, activation, receiver) {
        return unwind(mutator, signal, floor, handoff);
    }
    let value = read_local(&mutator.heap, activation, source);
    match value_for_a_value_slot(mutator, value) {
        None => unwind(mutator, signal, floor, handoff),
        Some(Ok(Resolution::Value(resolved))) => {
            let receiver = mutator.heap.activation_receiver(activation);
            let base = mutator.heap.activation_slot_base(activation);
            match check_value_slot(mutator, activation, receiver, own_index as usize, resolved) {
                Ok(()) => {
                    store_instance_field(
                        &mut mutator.heap,
                        receiver,
                        base + own_index as usize,
                        resolved,
                    );
                    Transfer::Next
                }
                Err(signal) => unwind(mutator, signal, floor, handoff),
            }
        }
        Some(Ok(Resolution::Force(step))) => {
            force_and_reexecute(mutator, activation, program_counter, step, floor, handoff)
        }
        Some(Err(signal)) => unwind(mutator, signal, floor, handoff),
    }
}

/// The four instructions only a simultaneous slot clause emits, in their
/// narrow form; the slot, one word, carries their operands. Their wide
/// forms call the same functions from `run_wide`.
#[inline(never)]
fn simultaneous_clause_instruction<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: usize,
    slot: CompactInstruction,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    match slot {
        // A crafted image can put anything in `closure`'s register (an
        // image must never panic, `CLAUDE.md`, "An image is untrusted
        // input"), so an operand that is not a closure is an ordinary error,
        // not the `unreachable!` a compiler-emitted `MakeFuture` -- whose
        // operand is always the block `MakeClosure` just made -- would
        // never trigger.
        CompactInstruction::MakeFuture {
            destination,
            closure,
        } => make_future(
            mutator,
            activation,
            u16::from(destination),
            u16::from(closure),
            floor,
            handoff,
        ),
        CompactInstruction::ResolveFuture {
            destination,
            source,
        } => resolve_future(
            mutator,
            activation,
            program_counter,
            u16::from(destination),
            u16::from(source),
            floor,
            handoff,
        ),
        CompactInstruction::LoadOwnSlot {
            destination,
            own_index,
        } => load_own_slot_into(
            mutator,
            activation,
            u16::from(destination),
            own_index,
            floor,
            handoff,
        ),
        CompactInstruction::InitializeSlotProvisionally { source, own_index } => {
            initialize_slot_provisionally(
                mutator,
                activation,
                u16::from(source),
                own_index,
                floor,
                handoff,
            )
        }
        _ => unreachable!("internal: only a simultaneous clause's instructions come here"),
    }
}

/// `MakeFuture`: a new unresolved future of the closure in `closure`.
fn make_future<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    destination: u16,
    closure: u16,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let operand = read_local(&mutator.heap, activation, closure);
    let closure = match operand.as_object() {
        Some(object) if mutator.heap.is_closure(object) => object,
        _ => return unwind(mutator, not_a_closure(operand), floor, handoff),
    };
    let future = mutator.new_future(closure);
    write_local(
        &mut mutator.heap,
        activation,
        destination,
        Value::object(future),
    );
    Transfer::Next
}

/// `ResolveFuture`: the value `source` holds, seen through a future --
/// forced first, and the instruction run again, when it is unresolved.
fn resolve_future<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: usize,
    destination: u16,
    source: u16,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let value = read_local(&mutator.heap, activation, source);
    match resolve_for_instruction(mutator, value) {
        Ok(Resolution::Value(value)) => {
            write_local(&mut mutator.heap, activation, destination, value);
            Transfer::Next
        }
        Ok(Resolution::Force(step)) => {
            force_and_reexecute(mutator, activation, program_counter, step, floor, handoff)
        }
        Err(signal) => unwind(mutator, signal, floor, handoff),
    }
}

/// `LoadOwnSlot`: `self`'s field at the slot base plus `own_index`.
fn load_own_slot_into<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    destination: u16,
    own_index: u32,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    match load_own_slot(mutator, activation, own_index) {
        Ok(value) => {
            write_local(&mut mutator.heap, activation, destination, value);
            Transfer::Next
        }
        Err(signal) => unwind(mutator, signal, floor, handoff),
    }
}

/// `InitializeSlotProvisionally`: `InitializeSlot` without §3.1.1's check.
fn initialize_slot_provisionally<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    source: u16,
    own_index: u32,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    if let Err(signal) = check_own_slots(mutator, activation) {
        return unwind(mutator, signal, floor, handoff);
    }
    let value = read_local(&mutator.heap, activation, source);
    let receiver = mutator.heap.activation_receiver(activation);
    let base = mutator.heap.activation_slot_base(activation);
    store_instance_field(
        &mut mutator.heap,
        receiver,
        base + own_index as usize,
        value,
    );
    Transfer::Next
}

/// An instruction met an unresolved future: store where it stands, push the
/// forcing, and switch to it; `Reexecute` brings the loop back to this same
/// instruction once the future is resolved (`reexecute`). One of the
/// instructions whose `program_counter` must be stored before it hands
/// control elsewhere (`run`'s doc comment).
#[inline(never)]
fn force_and_reexecute<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: usize,
    step: Step<'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    mutator
        .heap
        .activation_set_program_counter(activation, program_counter);
    match mutator.push_step(step, Arguments::NONE, Some(activation), false) {
        Ok(callee) => handoff.switch_to(mutator, callee, 0),
        Err(signal) => unwind(mutator, signal, floor, handoff),
    }
}

/// A signal leaving the running activation, handed to the one unwinder
/// (`interpreter::unwind`), whose outcome the loop only carries out: the
/// target returns its value, a block the unwind runs on its way -- or the
/// `signal` an error became -- is pushed and switched to, or the run ends.
/// Deciding which is `interpreter/`'s, so a JIT reaches the same unwinding
/// (`docs/design/execution.md`, "Unwinding").
///
/// A block's push the depth limit refuses raises a signal that is unwound in
/// turn, hence the loop rather than a recursion. It ends: the unwinder marks
/// each block run before answering it, so no block is answered twice and
/// every refusal leaves one pending block fewer on the stack.
fn unwind<'gc>(
    mutator: &mut Mutator<'gc>,
    mut signal: Signal<'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    loop {
        let next = match crate::interpreter::unwind::unwind_signal(mutator, signal, floor) {
            UnwindOutcome::Returned { target, value } => {
                return finish(mutator, target, value, floor, handoff)
            }
            // Where the target starts again is the unwinder's to decide, and
            // it stored it; the loop resumes the target there.
            UnwindOutcome::Restarted { target } => {
                let program_counter = mutator.heap.activation_program_counter(target);
                return handoff.switch_to(mutator, target, program_counter);
            }
            UnwindOutcome::Signalling(next) => next,
            UnwindOutcome::RunBlock { block, then } => {
                match mutator.start_unwind_block(block, then) {
                    Ok(next) => next,
                    Err(refused) => {
                        signal = refused;
                        continue;
                    }
                }
            }
            UnwindOutcome::Exit(result) => return handoff.exit(mutator.to_host(result)),
        };
        return handoff.switch_to(mutator, next, 0);
    }
}

/// The register a send writes its value to: where a callee's value goes when
/// the loop resumes the caller.
fn send_destination(code: &CompiledCode, program_counter: usize) -> u16 {
    match code.slot(program_counter) {
        CompactInstruction::SendPublic0 { destination, .. }
        | CompactInstruction::SendPublic1 { destination, .. }
        | CompactInstruction::SendPublic2 { destination, .. }
        | CompactInstruction::SendInlinable0 { destination, .. }
        | CompactInstruction::SendInlinable1 { destination, .. }
        | CompactInstruction::SendInlinable2 { destination, .. }
        | CompactInstruction::SendLexical0 { destination, .. }
        | CompactInstruction::SendLexical1 { destination, .. }
        | CompactInstruction::SendLexical2 { destination, .. }
        | CompactInstruction::SendSuper0 { destination, .. }
        | CompactInstruction::SendSuper1 { destination, .. }
        | CompactInstruction::SendSuper2 { destination, .. }
        // A leaf that activates a block (`value:`, `do:`) resumes its
        // caller here.
        | CompactInstruction::SendMirror { destination, .. }
        // A factory's or an initializer's initializer returns here.
        | CompactInstruction::InitializeInstance { destination, .. }
        | CompactInstruction::InitializeSuperclass { destination, .. }
        | CompactInstruction::InitializeAppliedLevel { destination, .. } => u16::from(destination),
        CompactInstruction::SendPublicWide { site }
        | CompactInstruction::SendInlinableWide { site }
        | CompactInstruction::SendLexicalWide { site }
        | CompactInstruction::SendSuperWide { site } => {
            code.body()
                .send_site(site as usize)
                .wide()
                .expect(WIDE_SEND)
                .destination()
        }
        CompactInstruction::Wide { index } => match code.body().wide(index as usize) {
            Instruction::SendMirror { destination, .. }
            | Instruction::InitializeInstance { destination, .. }
            | Instruction::InitializeSuperclass { destination, .. }
            | Instruction::InitializeAppliedLevel { destination, .. } => *destination,
            other => unreachable!(
                "internal: a caller resumed at an instruction that is not a send: {other:?}"
            ),
        },
        other => unreachable!(
            "internal: a caller resumed at an instruction that is not a send: {other:?}"
        ),
    }
}

/// Materializes a literal -- including the two deferred error cases,
/// `UnresolvedIdentifier` and `BodyTooLarge`, which raise through
/// `literals.rs`'s own constructors rather than rebuilding the error here:
/// error text lives in `interpreter/`, never in the VM. A `LargeInteger` is
/// a fresh object each time it runs, since a value needs no identity.
fn make_value<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    literal: &Literal,
) -> EvaluationResult<'gc> {
    Ok(match literal {
        Literal::Nil => Value::nil(),
        Literal::Boolean(boolean) => Value::boolean(*boolean),
        Literal::SmallInteger(integer) => Value::small_integer(*integer),
        Literal::Float(float) => mutator.new_float(*float),
        Literal::String(string) => Value::object(mutator.new_string(string.clone())),
        Literal::Symbol(symbol) => mutator.new_symbol(*symbol),
        Literal::LargeInteger(integer) => mutator.new_integer(integer.clone()),
        Literal::UnresolvedIdentifier(name) => {
            let receiver = mutator.heap.activation_receiver(activation);
            return Err(unresolved_identifier_error(name, receiver));
        }
        Literal::BodyTooLarge => {
            let receiver = mutator.heap.activation_receiver(activation);
            let kind = mutator.heap.activation_kind(activation);
            return Err(body_too_large_error(kind, receiver));
        }
    })
}
