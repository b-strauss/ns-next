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

use std::rc::Rc;

use crate::bytecode::{CompiledCode, Instruction, IntegerOperation, Literal};
use crate::heap::{ActivationReference, RegisterBase};
use crate::interpreter::future::{
    not_a_closure, resolve_for_instruction, value_for_a_value_slot, FutureOutcome, Resolution,
};
use crate::interpreter::inline_cache::{send_lexical, send_public, send_super};
use crate::interpreter::instantiate::check_value_slot;
use crate::interpreter::literals::unresolved_identifier_error;
use crate::interpreter::locals::{
    filled_own_slot, load_own_slot, load_vector, read_local, read_outer_local,
    store_instance_field, store_vector, write_local, write_outer_local,
};
use crate::interpreter::unwind::UnwindOutcome;
use crate::interpreter::{
    EvaluationResult, FeedbackBase, HostResult, Interpreter, Mutator, Signal, Step,
};
use crate::types::*;

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
/// Nothing reads `program_counter` while a straight-line instruction
/// executes.
pub(crate) fn run(interpreter: &mut Interpreter, base: ActivationReference) -> HostResult {
    let (floor, mut frame) = interpreter.enter(|mutator| {
        (
            mutator.heap.activation_position(base),
            FrameRegisters {
                activation: base,
                code: mutator.heap.activation_code(base),
                program_counter: 0,
                feedback_base: mutator.heap.activation_feedback_base(base),
                register_base: mutator.heap.register_base(base),
            },
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
            Leave::Safepoint => interpreter.safepoint(),
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
    // A jump to `target`, from every instruction that jumps -- written once,
    // so that the set of jumps that test the safepoint when they go backward
    // is exactly the set of jumps. A backward jump closes a loop, so that a
    // loop that makes no send still passes a safepoint. Leaving breaks out of
    // the loop to the one write-back below it, which every safepoint shares.
    macro_rules! jump_to {
        ($target:expr) => {{
            let target = $target as usize;
            let backward = target <= program_counter;
            program_counter = target;
            if backward && mutator.heap.safepoint_requested() {
                break;
            }
            continue;
        }};
    }
    loop {
        debug_assert!(
            program_counter < code.instructions.len(),
            "internal: program counter ran past the end of a CompiledCode with no terminating return"
        );
        let transfer = match &code.instructions[program_counter] {
            Instruction::LoadLiteral {
                destination,
                literal,
            } => match make_value(mutator, activation, literal) {
                Ok(value) => {
                    mutator
                        .heap
                        .set_register(register_base, *destination, value);
                    program_counter += 1;
                    continue;
                }
                Err(signal) => unwind(mutator, signal, floor, &mut handoff),
            },
            Instruction::LoadSelf { destination } => {
                let value = mutator.heap.activation_receiver(activation);
                mutator
                    .heap
                    .set_register(register_base, *destination, value);
                program_counter += 1;
                continue;
            }
            Instruction::LoadEnclosingObject { destination, depth } => {
                let value =
                    crate::interpreter::sends::enclosing_object(mutator, activation, *depth);
                mutator
                    .heap
                    .set_register(register_base, *destination, value);
                program_counter += 1;
                continue;
            }
            Instruction::Move {
                destination,
                source,
            }
            | Instruction::StoreLocal {
                source,
                slot: destination,
            } => {
                let value = mutator.heap.register(register_base, *source);
                mutator
                    .heap
                    .set_register(register_base, *destination, value);
                program_counter += 1;
                continue;
            }
            Instruction::LoadOuterLocal {
                destination,
                depth,
                index,
            } => {
                let value = read_outer_local(&mutator.heap, activation, *depth, *index);
                mutator
                    .heap
                    .set_register(register_base, *destination, value);
                program_counter += 1;
                continue;
            }
            Instruction::StoreOuterLocal {
                source,
                depth,
                index,
            } => {
                let value = mutator.heap.register(register_base, *source);
                write_outer_local(&mut mutator.heap, activation, *depth, *index, value);
                program_counter += 1;
                continue;
            }
            Instruction::SendPublic {
                selector,
                receiver_register,
                argument_registers,
                destination,
                call_site,
            } => {
                let receiver = mutator.heap.register(register_base, *receiver_register);
                let family_size = code.family_size;
                send(
                    mutator,
                    activation,
                    register_base,
                    program_counter,
                    argument_registers,
                    *destination,
                    floor,
                    false,
                    &mut handoff,
                    |mutator, arguments| {
                        send_public(
                            mutator,
                            feedback_base,
                            family_size,
                            *call_site,
                            receiver,
                            *selector,
                            arguments,
                        )
                    },
                )
            }
            // Decided as `SendPublic` is, but uncached: only the reference
            // lowering emits it. Only the depth count treats a control-flow
            // step's block activated for it differently
            // (`Mutator::push_step`).
            Instruction::SendInlinable {
                selector,
                receiver_register,
                argument_registers,
                destination,
                ..
            } => {
                let receiver = mutator.heap.register(register_base, *receiver_register);
                send(
                    mutator,
                    activation,
                    register_base,
                    program_counter,
                    argument_registers,
                    *destination,
                    floor,
                    true,
                    &mut handoff,
                    |mutator, arguments| mutator.decide_send(receiver, *selector, arguments),
                )
            }
            // A send to the VM mirror with its failure block inline: the leaf
            // runs, or declines and the block runs at `failure`. What the leaf
            // decided is delivered as any send's decision is. Which leaf, and
            // whether the receiver is the mirror, is `interpreter::mirror`'s.
            Instruction::SendMirror {
                primitive,
                mirror_register,
                subject_register,
                argument_registers,
                destination,
                failure,
            } => {
                let mirror = mutator.heap.register(register_base, *mirror_register);
                let subject = mutator.heap.register(register_base, *subject_register);
                let mut arguments = mutator.take_argument_buffer();
                arguments.extend(
                    argument_registers
                        .iter()
                        .map(|register| mutator.heap.register(register_base, *register)),
                );
                mutator
                    .heap
                    .set_activation_program_counter(activation, program_counter);
                // `None`, the leaf declined: the jump to `failure` waits
                // until the buffer is handed back.
                let transfer = crate::interpreter::mirror::send_inlined(
                    mutator, *primitive, mirror, subject, &arguments,
                )
                .map(|decided| {
                    deliver(
                        mutator,
                        activation,
                        register_base,
                        decided,
                        &arguments,
                        *destination,
                        floor,
                        false,
                        &mut handoff,
                    )
                });
                mutator.return_argument_buffer(arguments);
                match transfer {
                    Some(transfer) => transfer,
                    None => jump_to!(*failure),
                }
            }
            Instruction::SendLexical {
                depth,
                selector,
                argument_registers,
                destination,
                call_site,
            } => {
                let family_size = code.family_size;
                send(
                    mutator,
                    activation,
                    register_base,
                    program_counter,
                    argument_registers,
                    *destination,
                    floor,
                    false,
                    &mut handoff,
                    |mutator, arguments| {
                        send_lexical(
                            mutator,
                            activation,
                            feedback_base,
                            family_size,
                            *call_site,
                            *depth,
                            *selector,
                            arguments,
                        )
                    },
                )
            }
            Instruction::SendSuper {
                selector,
                argument_registers,
                destination,
                call_site,
            } => {
                let family_size = code.family_size;
                send(
                    mutator,
                    activation,
                    register_base,
                    program_counter,
                    argument_registers,
                    *destination,
                    floor,
                    false,
                    &mut handoff,
                    |mutator, arguments| {
                        send_super(
                            mutator,
                            activation,
                            feedback_base,
                            family_size,
                            *call_site,
                            *selector,
                            arguments,
                        )
                    },
                )
            }
            Instruction::MakeClosure {
                destination,
                code: block,
                copied,
                ..
            } => {
                // Stored before reifying, so the frame `reify` builds starts
                // from this program counter.
                mutator
                    .heap
                    .set_activation_program_counter(activation, program_counter);
                let captured_frame = mutator.heap.reify(activation);
                // A closure that reads enclosing registers by reference -- the
                // reference lowering's, and an optimized fallback closure -- is
                // what makes this activation's registers outlive it.
                if block.outer_reach > 0 {
                    mutator.heap.retain_registers(activation);
                }
                let copied_values = copied
                    .iter()
                    .map(|register| mutator.heap.register(register_base, *register))
                    .collect();
                let value = Value::object(mutator.new_closure(ClosureData {
                    code: block.clone(),
                    captured_frame,
                    copied: copied_values,
                    // A block belongs to its enclosing method's family: the
                    // running activation's own base, already in hand here, so
                    // this costs no extra load.
                    feedback_base,
                }));
                mutator
                    .heap
                    .set_register(register_base, *destination, value);
                program_counter += 1;
                continue;
            }
            Instruction::MakeArray {
                destination,
                element_registers,
            } => {
                let values = element_registers
                    .iter()
                    .map(|register| mutator.heap.register(register_base, *register))
                    .collect();
                let value = Value::object(mutator.new_array(values));
                mutator
                    .heap
                    .set_register(register_base, *destination, value);
                program_counter += 1;
                continue;
            }
            Instruction::MakeVector { destination, size } => {
                let value = Value::object(mutator.new_array(vec![Value::nil(); *size as usize]));
                mutator
                    .heap
                    .set_register(register_base, *destination, value);
                program_counter += 1;
                continue;
            }
            Instruction::LoadVector {
                destination,
                vector,
                index,
            } => match load_vector(&mutator.heap, activation, *vector, *index) {
                Ok(value) => {
                    mutator
                        .heap
                        .set_register(register_base, *destination, value);
                    program_counter += 1;
                    continue;
                }
                Err(signal) => unwind(mutator, signal, floor, &mut handoff),
            },
            Instruction::StoreVector {
                vector,
                index,
                source,
            } => match store_vector(&mut mutator.heap, activation, *vector, *index, *source) {
                Ok(()) => {
                    program_counter += 1;
                    continue;
                }
                Err(signal) => unwind(mutator, signal, floor, &mut handoff),
            },
            Instruction::InitializeSlot { source, own_index } => {
                // `own_index` is relative to the executing mixin's own slots; the
                // activation's slot base, its `current_class`'s own, turns it into
                // this instance's absolute field index.
                let value = mutator.heap.register(register_base, *source);
                let receiver = mutator.heap.activation_receiver(activation);
                let base = mutator.heap.activation_slot_base(activation);
                // §3.1.1's rule about what a value's slot may hold is decided in
                // `interpreter/`, as every error is; this only calls it.
                match check_value_slot(mutator, activation, receiver, *own_index as usize, value) {
                    Ok(()) => {
                        store_instance_field(
                            &mut mutator.heap,
                            &receiver,
                            base + *own_index as usize,
                            value,
                        );
                        program_counter += 1;
                        continue;
                    }
                    // A future can reach a value's slot from outside any
                    // simultaneous clause's own resolution -- passed in as an
                    // ordinary argument, since a future is indistinguishable from
                    // its value (§6.3.2, rule 3). What belongs in the slot is
                    // what it stands for: forced first if it is not resolved yet
                    // (`value_for_a_value_slot`), then checked again with the
                    // same accepted path above. A refusal that is not about a
                    // future stands.
                    Err(signal) => initialize_slot_refused(
                        mutator,
                        activation,
                        program_counter,
                        signal,
                        floor,
                        &mut handoff,
                    ),
                }
            }
            // A lazy slot's getter answers its field once it is filled; the
            // field's address is `InitializeSlot`'s, and reading it is a
            // `locals` helper's.
            Instruction::ReturnSlotIfFilled { own_index } => {
                match filled_own_slot(&mutator.heap, activation, *own_index) {
                    Some(value) => finish(mutator, activation, value, floor, &mut handoff),
                    None => {
                        program_counter += 1;
                        continue;
                    }
                }
            }
            // §6.3.2's simultaneous slot clauses: one arm and one call, out of
            // line (`simultaneous_clause_instruction`).
            Instruction::MakeFuture { .. }
            | Instruction::ResolveFuture { .. }
            | Instruction::LoadOwnSlot { .. }
            | Instruction::InitializeSlotProvisionally { .. } => simultaneous_clause_instruction(
                mutator,
                activation,
                program_counter,
                floor,
                &mut handoff,
            ),
            Instruction::Return { register } | Instruction::ImplicitReturn { register } => {
                let value = mutator.heap.register(register_base, *register);
                finish(mutator, activation, value, floor, &mut handoff)
            }
            Instruction::NonLocalReturn { register } => {
                let value = mutator.heap.register(register_base, *register);
                mutator
                    .heap
                    .set_activation_program_counter(activation, program_counter);
                let signal = mutator.raise_return(activation, value);
                unwind(mutator, signal, floor, &mut handoff)
            }
            Instruction::Jump { target } => jump_to!(*target),
            Instruction::JumpIfNotBoolean { register, target } => {
                if mutator
                    .heap
                    .register(register_base, *register)
                    .as_boolean()
                    .is_some()
                {
                    program_counter += 1;
                    continue;
                }
                jump_to!(*target)
            }
            Instruction::BranchIfFalse { register, target } => {
                if mutator.heap.register(register_base, *register).as_boolean() == Some(false) {
                    jump_to!(*target)
                }
                program_counter += 1;
                continue;
            }
            Instruction::BranchIfTrue { register, target } => {
                if mutator.heap.register(register_base, *register).as_boolean() == Some(true) {
                    jump_to!(*target)
                }
                program_counter += 1;
                continue;
            }
            // The inlined fast path for an integer leaf. Not dispatch: it either
            // computes the one answer the primitive would have computed, or
            // declines and jumps to the ordinary send standing at `target`. Every
            // case it declines -- a non-integer operand, a result that does not
            // fit, a zero divisor -- is that send's primitive to decide, so
            // nothing is promoted and no error or error text is raised here. It
            // cannot fail for any operand, which is what lets a decoded image
            // reach it safely.
            Instruction::IntegerBinary {
                operation,
                destination,
                left,
                right,
                target,
            } => {
                let left_value = mutator.heap.register(register_base, *left);
                let right_value = mutator.heap.register(register_base, *right);
                let (Some(left_integer), Some(right_integer)) = (
                    left_value.as_small_integer(),
                    right_value.as_small_integer(),
                ) else {
                    jump_to!(*target)
                };
                let result = match operation {
                    IntegerOperation::Add => left_integer
                        .checked_add(right_integer)
                        .filter(|number| fits_small_integer(*number))
                        .map(Value::small_integer),
                    IntegerOperation::Subtract => left_integer
                        .checked_sub(right_integer)
                        .filter(|number| fits_small_integer(*number))
                        .map(Value::small_integer),
                    IntegerOperation::Multiply => left_integer
                        .checked_mul(right_integer)
                        .filter(|number| fits_small_integer(*number))
                        .map(Value::small_integer),
                    // `checked_floor_divide` answers `None` for a zero
                    // divisor, and the range test declines
                    // `SMALL_INTEGER_MIN // -1`, so both take the fallback and
                    // the primitive decides what they answer. It is
                    // `builtins::numbers`' own function, so the rounding rule
                    // has one copy.
                    IntegerOperation::FloorDivide => {
                        crate::builtins::numbers::checked_floor_divide(left_integer, right_integer)
                            .filter(|number| fits_small_integer(*number))
                            .map(Value::small_integer)
                    }
                    IntegerOperation::Less => Some(Value::boolean(left_integer < right_integer)),
                    IntegerOperation::Greater => Some(Value::boolean(left_integer > right_integer)),
                    IntegerOperation::LessOrEqual => {
                        Some(Value::boolean(left_integer <= right_integer))
                    }
                    IntegerOperation::GreaterOrEqual => {
                        Some(Value::boolean(left_integer >= right_integer))
                    }
                    IntegerOperation::Equal => Some(Value::boolean(left_integer == right_integer)),
                };
                match result {
                    Some(value) => {
                        mutator
                            .heap
                            .set_register(register_base, *destination, value);
                        program_counter += 1;
                        continue;
                    }
                    None => jump_to!(*target),
                }
            }
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
                register_base = mutator.heap.register_base(activation);
                if mutator.heap.safepoint_requested() {
                    break;
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
    // the stretch stands at, written back for the next one.
    *frame = FrameRegisters {
        activation,
        code,
        program_counter,
        feedback_base,
        register_base,
    };
    Leave::Safepoint
}

/// A send: the arguments gathered from the caller's registers into the
/// interpreter's reusable buffer, `program_counter` stored, the decision asked
/// for, and what was decided run -- a value written to `destination`, or an
/// activation pushed and switched to, its window filled straight from the
/// buffer. No vector is allocated once the buffer has grown to the widest send.
/// `inlinable` says whether the instruction was a `SendInlinable`, which
/// `push_step` hands to the depth count along with the sender, `activation`.
#[allow(clippy::too_many_arguments)]
fn send<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    register_base: RegisterBase,
    program_counter: usize,
    argument_registers: &[u16],
    destination: u16,
    floor: usize,
    inlinable: bool,
    handoff: &mut Handoff,
    decide: impl FnOnce(&mut Mutator<'gc>, &[Value<'gc>]) -> Result<Step<'gc>, Signal<'gc>>,
) -> Transfer {
    let mut arguments = mutator.take_argument_buffer();
    arguments.extend(
        argument_registers
            .iter()
            .map(|register| mutator.heap.register(register_base, *register)),
    );
    mutator
        .heap
        .set_activation_program_counter(activation, program_counter);
    let decided = decide(mutator, &arguments);
    let transfer = deliver(
        mutator,
        activation,
        register_base,
        decided,
        &arguments,
        destination,
        floor,
        inlinable,
        handoff,
    );
    mutator.return_argument_buffer(arguments);
    transfer
}

/// What a send decided, run: a value written to `destination`, or an
/// activation pushed and switched to, its window filled from
/// `send_arguments`. Shared by `send` and `SendMirror`, so a mirror leaf's
/// decision is delivered exactly as any send's is.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn deliver<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    register_base: RegisterBase,
    decided: Result<Step<'gc>, Signal<'gc>>,
    send_arguments: &[Value<'gc>],
    destination: u16,
    floor: usize,
    inlinable: bool,
    handoff: &mut Handoff,
) -> Transfer {
    match decided {
        Ok(Step::Value(value)) => {
            mutator.heap.set_register(register_base, destination, value);
            Transfer::Next
        }
        Ok(step) => match mutator.push_step(step, send_arguments, Some(activation), inlinable) {
            Ok(callee) => handoff.switch_to(mutator, callee, 0),
            Err(signal) => unwind(mutator, signal, floor, handoff),
        },
        Err(signal) => unwind(mutator, signal, floor, handoff),
    }
}

/// `activation` has returned `value`: pop it and do what its continuation says.
/// A value -- the usual case -- goes to the send in the caller, which resumes,
/// or out of `run` when there is no caller in this run; a native loop's
/// continuation may instead decide another activation -- a native loop's next
/// block, or a `mustBeBoolean` method -- which runs in the popped activation's
/// place. `Mutator::resume` applies the continuation and
/// `Mutator::push_continuation` pushes what it decided, depth count included;
/// the loop only forwards the popped entry's flag. A future's forcing
/// (`Mutator::future_computed`) may also run the caller's stopped
/// instruction again (`reexecute`) rather than resume after it.
fn finish<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    value: Value<'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    let popped = mutator.heap.pop_activation(activation);
    let value = match popped.then {
        // `Deliver` owns nothing, so forgetting it is dropping it. Written
        // out because otherwise the continuation, left in `popped`, is
        // dropped at the end of the function through `Continuation`'s drop
        // glue, which the boxed variants made large enough to stay out of
        // line: a call on every return, where the tag check used to be
        // inline. Forgetting it here measured about 1% faster on the
        // benchmarks that return most.
        deliver @ Continuation::Deliver => {
            std::mem::forget(deliver);
            value
        }
        // A future's block returned: what happens next is
        // `interpreter::future`'s to decide, and the loop carries it out.
        Continuation::Future(forcing) => match mutator.future_computed(*forcing, value) {
            Ok(FutureOutcome::Deliver(value)) => value,
            Ok(FutureOutcome::Reexecute) => return reexecute(mutator, floor, handoff),
            Ok(FutureOutcome::Force(step)) => {
                return match mutator.push_continuation(step, popped.counted) {
                    Ok(next) => handoff.switch_to(mutator, next, 0),
                    Err(signal) => unwind(mutator, signal, floor, handoff),
                };
            }
            Err(signal) => return unwind(mutator, signal, floor, handoff),
        },
        then => match mutator.resume(then, value) {
            Ok(Step::Value(value)) => value,
            Ok(step) => {
                return match mutator.push_continuation(step, popped.counted) {
                    Ok(next) => handoff.switch_to(mutator, next, 0),
                    Err(signal) => unwind(mutator, signal, floor, handoff),
                };
            }
            Err(signal) => return unwind(mutator, signal, floor, handoff),
        },
    };
    if mutator.heap.activation_depth() == floor {
        return handoff.exit(mutator.to_host(Ok(value)));
    }
    let caller = mutator
        .heap
        .top_activation()
        .expect("internal: an activation above the run's floor has a caller");
    resume_caller(mutator, caller, value, handoff)
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
        send_destination(&code.instructions[program_counter]),
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
    if mutator.heap.activation_depth() == floor {
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
fn initialize_slot_refused<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: usize,
    signal: Signal<'gc>,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    // Read back here rather than handed over, so that the arm `vm::run`
    // inlines passes no more than an unwind would.
    let code = mutator.heap.activation_code(activation);
    let Instruction::InitializeSlot { source, own_index } = code.instructions[program_counter]
    else {
        unreachable!("internal: a refused InitializeSlot is an InitializeSlot");
    };
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
                        &receiver,
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

/// The four instructions only a simultaneous slot clause emits.
#[inline(never)]
fn simultaneous_clause_instruction<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: usize,
    floor: usize,
    handoff: &mut Handoff,
) -> Transfer {
    // Read back here rather than handed over, so that the arm `vm::run`
    // inlines passes no more than `initialize_slot_refused`'s does.
    let code = mutator.heap.activation_code(activation);
    match code.instructions[program_counter] {
        // A crafted image can put anything in `closure`'s register (an
        // image must never panic, `CLAUDE.md`, "An image is untrusted
        // input"), so an operand that is not a closure is an ordinary error,
        // not the `unreachable!` a compiler-emitted `MakeFuture` -- whose
        // operand is always the block `MakeClosure` just made -- would
        // never trigger.
        Instruction::MakeFuture {
            destination,
            closure,
        } => make_future(mutator, activation, destination, closure, floor, handoff),
        Instruction::ResolveFuture {
            destination,
            source,
        } => resolve_future(
            mutator,
            activation,
            program_counter,
            destination,
            source,
            floor,
            handoff,
        ),
        Instruction::LoadOwnSlot {
            destination,
            own_index,
        } => load_own_slot_into(mutator, activation, destination, own_index),
        Instruction::InitializeSlotProvisionally { source, own_index } => {
            initialize_slot_provisionally(mutator, activation, source, own_index)
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
) -> Transfer {
    let value = load_own_slot(&mutator.heap, activation, own_index);
    write_local(&mut mutator.heap, activation, destination, value);
    Transfer::Next
}

/// `InitializeSlotProvisionally`: `InitializeSlot` without §3.1.1's check.
fn initialize_slot_provisionally<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    source: u16,
    own_index: u32,
) -> Transfer {
    let value = read_local(&mutator.heap, activation, source);
    let receiver = mutator.heap.activation_receiver(activation);
    let base = mutator.heap.activation_slot_base(activation);
    store_instance_field(
        &mut mutator.heap,
        &receiver,
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
        .set_activation_program_counter(activation, program_counter);
    match mutator.push_step(step, &[], Some(activation), false) {
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
            UnwindOutcome::Restarted { target } => return handoff.switch_to(mutator, target, 0),
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
fn send_destination(instruction: &Instruction) -> u16 {
    match instruction {
        Instruction::SendPublic { destination, .. }
        | Instruction::SendInlinable { destination, .. }
        | Instruction::SendLexical { destination, .. }
        | Instruction::SendSuper { destination, .. }
        // A leaf that activates a block (`value:`, `do:`) resumes its
        // caller here.
        | Instruction::SendMirror { destination, .. } => *destination,
        other => unreachable!(
            "internal: a caller resumed at an instruction that is not a send: {other:?}"
        ),
    }
}

/// Materializes a literal -- including the one deferred error case,
/// `UnresolvedIdentifier`, which raises through `literals.rs`'s own
/// constructor rather than rebuilding the error here: error text lives in
/// `interpreter/`, never in the VM. A `LargeInteger` is a fresh object each
/// time it runs, since a value needs no identity.
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
    })
}
