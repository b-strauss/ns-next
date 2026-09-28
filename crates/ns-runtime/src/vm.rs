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
use crate::heap::ActivationReference;
use crate::interpreter::future::FutureOutcome;
use crate::interpreter::inline_cache::{send_lexical, send_public, send_super};
use crate::interpreter::instantiate::check_value_slot;
use crate::interpreter::literals::{
    malformed_integer_error, parse_integer_literal, unresolved_identifier_error,
};
use crate::interpreter::locals::{
    filled_own_slot, load_vector, read_local, read_outer_local, store_instance_field, store_vector,
    write_local, write_outer_local,
};
use crate::interpreter::unwind::UnwindOutcome;
use crate::interpreter::{
    EvaluationResult, FeedbackBase, HostResult, Interpreter, Mutator, Signal, Step,
};
use crate::types::*;

/// What the loop does after one instruction.
enum Flow {
    Next,
    Jump(usize),
    /// Run another activation from `program_counter`: a callee a send pushed
    /// (from 0), or a caller a return resumed (from the instruction after its
    /// send).
    Switch {
        activation: ActivationReference,
        code: Rc<CompiledCode>,
        program_counter: usize,
        feedback_base: FeedbackBase,
    },
    /// The activation `run` was given is done, and so is everything it
    /// activated.
    Exit(HostResult),
}

/// Runs `base` -- the top of the activation stack, pushed by the caller --
/// and every activation it pushes, until `base` is done, and pops them all,
/// `base` included, on every path out: a return, an error, or a non-local
/// return aimed below `base`. The stack is exactly as deep afterwards as it
/// was before `base` was pushed. Its safepoints (`poll`) are every switch to
/// another activation and every backward jump.
///
/// Each instruction is one entry (`Interpreter::enter`), and the safepoint is
/// outside every one: what the loop carries from one instruction to the next is
/// the running `ActivationReference`, its code and its `program_counter`, none
/// of which a move changes, and a run ends inside its last entry, its answer
/// rooted (`docs/design/memory.md`, "The brand").
///
/// An activation's `program_counter` is stored only by the instructions that
/// can hand it to an observer: every send (a callee runs above it, and the loop
/// resumes it from that `program_counter`), `MakeClosure` (it reifies the
/// activation, so the frame starts from the `program_counter` stored just
/// before), and `NonLocalReturn` (control leaves with a signal). Nothing reads
/// `program_counter` while a straight-line instruction executes.
pub(crate) fn run(interpreter: &mut Interpreter, base: ActivationReference) -> HostResult {
    let (floor, mut code, mut feedback_base) = interpreter.enter(|mutator| {
        (
            mutator.heap.activation_position(base),
            mutator.heap.activation_code(base),
            mutator.heap.activation_feedback_base(base),
        )
    });
    let mut activation = base;
    let mut program_counter = 0usize;
    loop {
        // The step is forced into the loop rather than left to the
        // inliner. It is called once, but it is large -- every cached send
        // inlines its hit path -- and past LLVM's threshold it becomes a
        // function of its own, so that every instruction, not only every
        // send, pays a call and a `Flow` returned through memory: 6.6-13.6%
        // on every benchmark when splitting resolve from act first did that.
        // Forcing it buys no speed of its own: on the code before the caches,
        // where the inliner took the step anyway, it moved no benchmark
        // beyond noise (at most 2.1%). What it keeps is the caches' gain,
        // 4.3-11.3% against that code, from being paid back in a call per
        // instruction (`docs/design/execution.md`, "What the inline caches
        // moved"). The step sits near the threshold: anything added to a
        // send's hit path lands here.
        match interpreter.enter(
            #[inline(always)]
            |mutator| {
                execute(
                    mutator,
                    activation,
                    &code,
                    program_counter,
                    floor,
                    feedback_base,
                )
            },
        ) {
            Flow::Next => {
                program_counter += 1;
                debug_assert!(
                    program_counter < code.instructions.len(),
                    "internal: program counter ran past the end of a CompiledCode with no terminating return"
                );
            }
            Flow::Jump(target) => {
                // A backward jump closes a loop, so that a loop that makes no
                // send still passes a safepoint.
                if target <= program_counter {
                    poll(interpreter);
                }
                program_counter = target;
            }
            Flow::Switch {
                activation: next,
                code: next_code,
                program_counter: next_program_counter,
                feedback_base: next_feedback_base,
            } => {
                activation = next;
                code = next_code;
                program_counter = next_program_counter;
                feedback_base = next_feedback_base;
                poll(interpreter);
            }
            Flow::Exit(result) => return result,
        }
    }
}

/// The safepoint, reached at every switch to another activation -- a push, or a
/// caller resumed -- and at every backward jump. Between two instructions the
/// loop holds the running `ActivationReference`, its code and its
/// `program_counter`, none of which a move changes, so a collection may run
/// here and nowhere else (`docs/design/memory.md`, "Where a collection can
/// happen"). One compare of a flag the heap owns; what happens when it is set
/// is `interpreter/`'s.
#[inline(always)]
fn poll(interpreter: &mut Interpreter) {
    if interpreter.safepoint_requested() {
        interpreter.safepoint();
    }
}

/// One instruction of `code`, running in `activation`.
#[inline(always)]
fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    code: &CompiledCode,
    program_counter: usize,
    floor: usize,
    feedback_base: FeedbackBase,
) -> Flow {
    match &code.instructions[program_counter] {
        Instruction::LoadLiteral {
            destination,
            literal,
        } => match make_value(mutator, activation, literal) {
            Ok(value) => {
                write_local(&mut mutator.heap, activation, *destination, value);
                Flow::Next
            }
            Err(signal) => unwind(mutator, signal, floor),
        },
        Instruction::LoadSelf { destination } => {
            let value = mutator.heap.activation_receiver(activation);
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        Instruction::LoadEnclosingObject { destination, depth } => {
            let value = crate::interpreter::sends::enclosing_object(mutator, activation, *depth);
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        Instruction::Move {
            destination,
            source,
        }
        | Instruction::StoreLocal {
            source,
            slot: destination,
        } => {
            let value = read_local(&mutator.heap, activation, *source);
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        Instruction::LoadOuterLocal {
            destination,
            depth,
            index,
        } => {
            let value = read_outer_local(&mutator.heap, activation, *depth, *index);
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        Instruction::StoreOuterLocal {
            source,
            depth,
            index,
        } => {
            let value = read_local(&mutator.heap, activation, *source);
            write_outer_local(&mut mutator.heap, activation, *depth, *index, value);
            Flow::Next
        }
        Instruction::SendPublic {
            selector,
            receiver_register,
            argument_registers,
            destination,
            call_site,
        } => {
            let receiver = read_local(&mutator.heap, activation, *receiver_register);
            let family_size = code.family_size;
            send(
                mutator,
                activation,
                program_counter,
                argument_registers,
                *destination,
                floor,
                false,
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
            let receiver = read_local(&mutator.heap, activation, *receiver_register);
            send(
                mutator,
                activation,
                program_counter,
                argument_registers,
                *destination,
                floor,
                true,
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
            let mirror = read_local(&mutator.heap, activation, *mirror_register);
            let subject = read_local(&mutator.heap, activation, *subject_register);
            let mut arguments = mutator.take_argument_buffer();
            arguments.extend(
                argument_registers
                    .iter()
                    .map(|register| read_local(&mutator.heap, activation, *register)),
            );
            mutator
                .heap
                .set_activation_program_counter(activation, program_counter);
            let flow = match crate::interpreter::mirror::send_inlined(
                mutator, *primitive, mirror, subject, &arguments,
            ) {
                None => Flow::Jump(*failure as usize),
                Some(decided) => deliver(
                    mutator,
                    activation,
                    decided,
                    &arguments,
                    *destination,
                    floor,
                    false,
                ),
            };
            mutator.return_argument_buffer(arguments);
            flow
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
                program_counter,
                argument_registers,
                *destination,
                floor,
                false,
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
                program_counter,
                argument_registers,
                *destination,
                floor,
                false,
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
                .map(|register| read_local(&mutator.heap, activation, *register))
                .collect();
            let value = Value::Object(mutator.new_closure(ClosureData {
                code: block.clone(),
                captured_frame,
                copied: copied_values,
                // A block belongs to its enclosing method's family: the
                // running activation's own base, already in hand here, so
                // this costs no extra load.
                feedback_base,
            }));
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        Instruction::MakeArray {
            destination,
            element_registers,
        } => {
            let values = element_registers
                .iter()
                .map(|register| read_local(&mutator.heap, activation, *register))
                .collect();
            let value = Value::Object(mutator.new_array(values));
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        Instruction::MakeVector { destination, size } => {
            let value = Value::Object(mutator.new_array(vec![Value::Nil; *size as usize]));
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
        Instruction::LoadVector {
            destination,
            vector,
            index,
        } => match load_vector(&mutator.heap, activation, *vector, *index) {
            Ok(value) => {
                write_local(&mut mutator.heap, activation, *destination, value);
                Flow::Next
            }
            Err(signal) => unwind(mutator, signal, floor),
        },
        Instruction::StoreVector {
            vector,
            index,
            source,
        } => match store_vector(&mut mutator.heap, activation, *vector, *index, *source) {
            Ok(()) => Flow::Next,
            Err(signal) => unwind(mutator, signal, floor),
        },
        Instruction::InitializeSlot { source, own_index } => {
            // `own_index` is relative to the executing mixin's own slots; the
            // activation's slot base, its `current_class`'s own, turns it into
            // this instance's absolute field index.
            let value = read_local(&mutator.heap, activation, *source);
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
                    Flow::Next
                }
                Err(signal) => unwind(mutator, signal, floor),
            }
        }
        // A lazy slot's getter answers its field once it is filled; the
        // field's address is `InitializeSlot`'s, and reading it is a
        // `locals` helper's.
        Instruction::ReturnSlotIfFilled { own_index } => {
            match filled_own_slot(&mutator.heap, activation, *own_index) {
                Some(value) => finish(mutator, activation, value, floor),
                None => Flow::Next,
            }
        }
        Instruction::Return { register } | Instruction::ImplicitReturn { register } => {
            let value = read_local(&mutator.heap, activation, *register);
            finish(mutator, activation, value, floor)
        }
        Instruction::NonLocalReturn { register } => {
            let value = read_local(&mutator.heap, activation, *register);
            mutator
                .heap
                .set_activation_program_counter(activation, program_counter);
            let signal = mutator.raise_return(activation, value);
            unwind(mutator, signal, floor)
        }
        Instruction::Jump { target } => Flow::Jump(*target as usize),
        Instruction::JumpIfNotBoolean { register, target } => {
            if matches!(
                read_local(&mutator.heap, activation, *register),
                Value::Boolean(_)
            ) {
                Flow::Next
            } else {
                Flow::Jump(*target as usize)
            }
        }
        Instruction::BranchIfFalse { register, target } => {
            if matches!(
                read_local(&mutator.heap, activation, *register),
                Value::Boolean(false)
            ) {
                Flow::Jump(*target as usize)
            } else {
                Flow::Next
            }
        }
        Instruction::BranchIfTrue { register, target } => {
            if matches!(
                read_local(&mutator.heap, activation, *register),
                Value::Boolean(true)
            ) {
                Flow::Jump(*target as usize)
            } else {
                Flow::Next
            }
        }
        // The inlined fast path for an integer leaf. Not dispatch: it either
        // computes the one answer the primitive would have computed, or
        // declines and jumps to the ordinary send standing at `target`. Every
        // case it declines -- a non-integer operand, an overflow, a zero
        // divisor -- is that send's primitive to decide, so no error and no
        // error text is raised here. It cannot fail for any operand, which is
        // what lets a decoded image reach it safely.
        Instruction::IntegerBinary {
            operation,
            destination,
            left,
            right,
            target,
        } => {
            let left_value = read_local(&mutator.heap, activation, *left);
            let right_value = read_local(&mutator.heap, activation, *right);
            let (Value::SmallInteger(left_integer), Value::SmallInteger(right_integer)) =
                (left_value, right_value)
            else {
                return Flow::Jump(*target as usize);
            };
            let result = match operation {
                IntegerOperation::Add => left_integer
                    .checked_add(right_integer)
                    .map(Value::SmallInteger),
                IntegerOperation::Subtract => left_integer
                    .checked_sub(right_integer)
                    .map(Value::SmallInteger),
                IntegerOperation::Multiply => left_integer
                    .checked_mul(right_integer)
                    .map(Value::SmallInteger),
                // `checked_div_euclid` answers `None` for a zero divisor as
                // well as for `i64::MIN // -1`, so both take the fallback and
                // the primitive raises its own "division by zero" or overflow
                // error. No zero check is written here, because writing one
                // would be the second copy.
                IntegerOperation::FloorDivide => left_integer
                    .checked_div_euclid(right_integer)
                    .map(Value::SmallInteger),
                IntegerOperation::Less => Some(Value::Boolean(left_integer < right_integer)),
                IntegerOperation::Greater => Some(Value::Boolean(left_integer > right_integer)),
                IntegerOperation::LessOrEqual => {
                    Some(Value::Boolean(left_integer <= right_integer))
                }
                IntegerOperation::GreaterOrEqual => {
                    Some(Value::Boolean(left_integer >= right_integer))
                }
                IntegerOperation::Equal => Some(Value::Boolean(left_integer == right_integer)),
            };
            match result {
                Some(value) => {
                    write_local(&mut mutator.heap, activation, *destination, value);
                    Flow::Next
                }
                None => Flow::Jump(*target as usize),
            }
        }
    }
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
    program_counter: usize,
    argument_registers: &[u16],
    destination: u16,
    floor: usize,
    inlinable: bool,
    decide: impl FnOnce(&mut Mutator<'gc>, &[Value<'gc>]) -> Result<Step<'gc>, Signal<'gc>>,
) -> Flow {
    let mut arguments = mutator.take_argument_buffer();
    arguments.extend(
        argument_registers
            .iter()
            .map(|register| read_local(&mutator.heap, activation, *register)),
    );
    mutator
        .heap
        .set_activation_program_counter(activation, program_counter);
    let decided = decide(mutator, &arguments);
    let flow = deliver(
        mutator,
        activation,
        decided,
        &arguments,
        destination,
        floor,
        inlinable,
    );
    mutator.return_argument_buffer(arguments);
    flow
}

/// What a send decided, run: a value written to `destination`, or an
/// activation pushed and switched to, its window filled from
/// `send_arguments`. Shared by `send` and `SendMirror`, so a mirror leaf's
/// decision is delivered exactly as any send's is.
#[inline(always)]
fn deliver<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    decided: Result<Step<'gc>, Signal<'gc>>,
    send_arguments: &[Value<'gc>],
    destination: u16,
    floor: usize,
    inlinable: bool,
) -> Flow {
    match decided {
        Ok(Step::Value(value)) => {
            write_local(&mut mutator.heap, activation, destination, value);
            Flow::Next
        }
        Ok(step) => match mutator.push_step(step, send_arguments, Some(activation), inlinable) {
            Ok(callee) => Flow::Switch {
                activation: callee,
                code: mutator.heap.activation_code(callee),
                program_counter: 0,
                feedback_base: mutator.heap.activation_feedback_base(callee),
            },
            Err(signal) => unwind(mutator, signal, floor),
        },
        Err(signal) => unwind(mutator, signal, floor),
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
) -> Flow {
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
            Ok(FutureOutcome::Reexecute) => return reexecute(mutator, floor),
            Ok(FutureOutcome::Force(step)) => {
                return match mutator.push_continuation(step, popped.counted) {
                    Ok(next) => Flow::Switch {
                        activation: next,
                        code: mutator.heap.activation_code(next),
                        program_counter: 0,
                        feedback_base: mutator.heap.activation_feedback_base(next),
                    },
                    Err(signal) => unwind(mutator, signal, floor),
                };
            }
            Err(signal) => return unwind(mutator, signal, floor),
        },
        then => match mutator.resume(then, value) {
            Ok(Step::Value(value)) => value,
            Ok(step) => {
                return match mutator.push_continuation(step, popped.counted) {
                    Ok(next) => Flow::Switch {
                        activation: next,
                        code: mutator.heap.activation_code(next),
                        program_counter: 0,
                        feedback_base: mutator.heap.activation_feedback_base(next),
                    },
                    Err(signal) => unwind(mutator, signal, floor),
                };
            }
            Err(signal) => return unwind(mutator, signal, floor),
        },
    };
    if mutator.heap.activation_depth() == floor {
        return Flow::Exit(mutator.to_host(Ok(value)));
    }
    let caller = mutator
        .heap
        .top_activation()
        .expect("internal: an activation above the run's floor has a caller");
    resume_caller(mutator, caller, value)
}

/// Writes a callee's value into the send `caller` is stopped at, and resumes
/// `caller` after it.
fn resume_caller<'gc>(
    mutator: &mut Mutator<'gc>,
    caller: ActivationReference,
    value: Value<'gc>,
) -> Flow {
    let code = mutator.heap.activation_code(caller);
    let program_counter = mutator.heap.activation_program_counter(caller);
    write_local(
        &mut mutator.heap,
        caller,
        send_destination(&code.instructions[program_counter]),
        value,
    );
    Flow::Switch {
        activation: caller,
        code,
        program_counter: program_counter + 1,
        feedback_base: mutator.heap.activation_feedback_base(caller),
    }
}

/// The instruction the popped activation's caller is stopped at, run again
/// rather than resumed after: it met an unresolved future, which is now
/// resolved (`interpreter::future`, `AfterForcing::Reexecute`). It wrote
/// nothing before it forced the future, so running it again is running it
/// once. A host's send has no instruction to run again; a future it passes
/// in straight to a leaf ends the run.
fn reexecute<'gc>(mutator: &mut Mutator<'gc>, floor: usize) -> Flow {
    if mutator.heap.activation_depth() == floor {
        let error = crate::interpreter::future::host_passed_a_future();
        return Flow::Exit(mutator.to_host(Err(error)));
    }
    let caller = mutator
        .heap
        .top_activation()
        .expect("internal: an activation above the run's floor has a caller");
    Flow::Switch {
        activation: caller,
        code: mutator.heap.activation_code(caller),
        program_counter: mutator.heap.activation_program_counter(caller),
        feedback_base: mutator.heap.activation_feedback_base(caller),
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
fn unwind<'gc>(mutator: &mut Mutator<'gc>, mut signal: Signal<'gc>, floor: usize) -> Flow {
    loop {
        let next = match crate::interpreter::unwind::unwind_signal(mutator, signal, floor) {
            UnwindOutcome::Returned { target, value } => {
                return finish(mutator, target, value, floor)
            }
            UnwindOutcome::Restarted { target } => {
                return Flow::Switch {
                    activation: target,
                    code: mutator.heap.activation_code(target),
                    program_counter: 0,
                    feedback_base: mutator.heap.activation_feedback_base(target),
                }
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
            UnwindOutcome::Exit(result) => return Flow::Exit(mutator.to_host(result)),
        };
        return Flow::Switch {
            activation: next,
            code: mutator.heap.activation_code(next),
            program_counter: 0,
            feedback_base: mutator.heap.activation_feedback_base(next),
        };
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

/// Materializes a literal -- including the two deferred error cases,
/// `MalformedInteger` and `UnresolvedIdentifier`. Both raise through
/// `literals.rs`'s own constructors rather than rebuilding the error here:
/// error text lives in `interpreter/`, never in the VM.
fn make_value<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    literal: &Literal,
) -> EvaluationResult<'gc> {
    Ok(match literal {
        Literal::Nil => Value::Nil,
        Literal::Boolean(boolean) => Value::Boolean(*boolean),
        Literal::SmallInteger(integer) => Value::SmallInteger(*integer),
        Literal::Float(float) => Value::Float(*float),
        Literal::String(string) => Value::Object(mutator.new_string(string.clone())),
        Literal::Symbol(symbol) => mutator.new_symbol(*symbol),
        Literal::MalformedInteger(text) => match parse_integer_literal(text) {
            Ok(integer) => Value::SmallInteger(integer),
            Err(message) => return Err(malformed_integer_error(message)),
        },
        Literal::UnresolvedIdentifier(name) => {
            let receiver = mutator.heap.activation_receiver(activation);
            return Err(unresolved_identifier_error(name, receiver));
        }
    })
}
