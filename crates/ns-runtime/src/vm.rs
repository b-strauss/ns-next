//! The register VM's run loop -- and nothing else. `run` executes one
//! activation and everything it activates, on the `Heap`'s activation stack,
//! without recursing on the Rust stack: a send asks `interpreter/` what it
//! means (`Mutator::decide_send`, `sends::decide_lexical_send`,
//! `sends::decide_super_send`) and, when the answer is an activation, pushes
//! it and runs it here; a return pops it and resumes the caller. No dispatch
//! logic, no `doesNotUnderstand:`, no native dispatch and no error text of its
//! own live here -- dispatch lives in one place, which the reference lowering
//! and a JIT share. See `docs/design/execution.md`, "The VM".

use std::rc::Rc;

use crate::bytecode::{CompiledCode, Instruction, Literal};
use crate::heap::ActivationReference;
use crate::interpreter::literals::{
    malformed_integer_error, parse_integer_literal, unresolved_identifier_error,
};
use crate::interpreter::locals::{
    home_of, read_local, read_outer_local, store_instance_field, write_local, write_outer_local,
};
use crate::interpreter::sends::{decide_lexical_send, decide_super_send};
use crate::interpreter::{EvaluationResult, HostResult, Interpreter, Mutator, Signal, Step};
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
    let (floor, mut code) = interpreter.enter(|mutator| {
        (
            mutator.heap.activation_position(base),
            mutator.heap.activation_code(base),
        )
    });
    let mut activation = base;
    let mut program_counter = 0usize;
    loop {
        match interpreter
            .enter(|mutator| execute(mutator, activation, &code, program_counter, floor))
        {
            Flow::Next => {
                program_counter += 1;
                debug_assert!(
                    program_counter < code.instructions.len(),
                    "internal: pc ran past the end of a CompiledCode with no terminating return"
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
            } => {
                activation = next;
                code = next_code;
                program_counter = next_program_counter;
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
fn execute(
    mutator: &mut Mutator,
    activation: ActivationReference,
    code: &CompiledCode,
    program_counter: usize,
    floor: usize,
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
        Instruction::LoadBuiltinClass { destination, kind } => {
            let value = Value::Object(mutator.builtin_class(*kind));
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
            // `call_site` is the reserved, inert inline-cache slot; nothing
            // reads or writes it yet.
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
                false,
                |mutator, arguments| mutator.decide_send(receiver, *selector, arguments),
            )
        }
        // Decided exactly as `SendPublic` is; only the depth count treats
        // the block a control-flow native activates for it differently
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
        Instruction::SendLexical {
            depth,
            selector,
            argument_registers,
            destination,
            ..
        } => send(
            mutator,
            activation,
            program_counter,
            argument_registers,
            *destination,
            floor,
            false,
            |mutator, arguments| {
                decide_lexical_send(mutator, activation, *depth, *selector, arguments)
            },
        ),
        Instruction::SendSuper {
            selector,
            argument_registers,
            destination,
            ..
        } => send(
            mutator,
            activation,
            program_counter,
            argument_registers,
            *destination,
            floor,
            false,
            |mutator, arguments| decide_super_send(mutator, activation, *selector, arguments),
        ),
        Instruction::MakeClosure {
            destination,
            parameters,
            code: block,
        } => {
            // Stored before reifying, so the frame `reify` builds starts
            // from this program counter.
            mutator
                .heap
                .set_activation_program_counter(activation, program_counter);
            let captured_frame = mutator.heap.reify(activation);
            let home = home_of(&mut mutator.heap, activation);
            let value = Value::Object(mutator.new_closure(ClosureData {
                parameters: parameters.clone(),
                code: block.clone(),
                captured_frame,
                home,
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
        Instruction::InitializeSlot { source, own_index } => {
            // `own_index` is relative to the executing mixin's own slots; the
            // activation's `slot_base`, set when this level's activation was
            // pushed, turns it into this instance's absolute field index.
            let value = read_local(&mutator.heap, activation, *source);
            let receiver = mutator.heap.activation_receiver(activation);
            let base = mutator.heap.activation_slot_base(activation);
            store_instance_field(
                &mut mutator.heap,
                &receiver,
                base + *own_index as usize,
                value,
            );
            Flow::Next
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
fn send(
    mutator: &mut Mutator,
    activation: ActivationReference,
    program_counter: usize,
    argument_registers: &[u16],
    destination: u16,
    floor: usize,
    inlinable: bool,
    decide: impl FnOnce(&mut Mutator, &[Value]) -> Result<Step, Signal>,
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
    let flow = match decide(mutator, &arguments) {
        Ok(Step::Value(value)) => {
            write_local(&mut mutator.heap, activation, destination, value);
            Flow::Next
        }
        Ok(step) => match mutator.push_step(step, &arguments, Some(activation), inlinable) {
            Ok(callee) => Flow::Switch {
                activation: callee,
                code: mutator.heap.activation_code(callee),
                program_counter: 0,
            },
            Err(signal) => unwind(mutator, signal, floor),
        },
        Err(signal) => unwind(mutator, signal, floor),
    };
    mutator.return_argument_buffer(arguments);
    flow
}

/// `activation` has returned `value`: pop it and do what its continuation says.
/// A value -- the usual case -- goes to the send in the caller, which resumes,
/// or out of `run` when there is no caller in this run; a control-flow native's
/// continuation may instead decide another activation -- a native loop's next
/// block, or a `mustBeBoolean` method -- which runs in the popped activation's
/// place. `Mutator::resume` applies the continuation and
/// `Mutator::push_continuation` pushes what it decided, depth count included;
/// the loop only forwards the popped entry's flag.
fn finish(
    mutator: &mut Mutator,
    activation: ActivationReference,
    value: Value,
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
        then => match mutator.resume(then, value) {
            Ok(Step::Value(value)) => value,
            Ok(step) => {
                return match mutator.push_continuation(step, popped.counted) {
                    Ok(next) => Flow::Switch {
                        activation: next,
                        code: mutator.heap.activation_code(next),
                        program_counter: 0,
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
fn resume_caller(mutator: &mut Mutator, caller: ActivationReference, value: Value) -> Flow {
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
    }
}

/// A signal leaving the running activation. A non-local return whose target
/// is one of this run's activations and catches a return (a method, or a
/// superclass clause or its message -- `Mutator::catches_return`) ends
/// there: everything above the target is popped and the target returns the
/// value. Anything else pops every activation of the run, `base` included,
/// and leaves `run` with the signal, for the host that started the run
/// (`Interpreter::run_from_host`).
///
/// A target that catches no return -- a running initializer -- is popped
/// with the rest, and the signal goes on naming it, out to the host: the
/// known gap in `docs/design/execution.md`, "Non-local return". Liveness is
/// asked first, because it is the one question a handle whose entry may
/// have been popped can be asked (`Heap::activation_is_live`). Every
/// activation it pops hands its continuation to `Mutator::abandon`, which
/// undoes what a continuation that will now never run had begun.
fn unwind(mutator: &mut Mutator, signal: Signal, floor: usize) -> Flow {
    if let Signal::Return(target, value) = &signal {
        let (target, value) = (*target, *value);
        if mutator.heap.activation_is_live(target)
            && mutator.heap.activation_position(target) >= floor
            && mutator.catches_return(target)
        {
            while let Some(top) = mutator.heap.top_activation() {
                if top.same(target) {
                    break;
                }
                let popped = mutator.heap.pop_activation(top);
                mutator.abandon(popped.then);
            }
            return finish(mutator, target, value, floor);
        }
    }
    while mutator.heap.activation_depth() > floor {
        let top = mutator
            .heap
            .top_activation()
            .expect("internal: a stack deeper than the floor has a top");
        let popped = mutator.heap.pop_activation(top);
        mutator.abandon(popped.then);
    }
    Flow::Exit(mutator.to_host(Err(signal)))
}

/// The register a send writes its value to: where a callee's value goes when
/// the loop resumes the caller.
fn send_destination(instruction: &Instruction) -> u16 {
    match instruction {
        Instruction::SendPublic { destination, .. }
        | Instruction::SendInlinable { destination, .. }
        | Instruction::SendLexical { destination, .. }
        | Instruction::SendSuper { destination, .. } => *destination,
        other => unreachable!(
            "internal: a caller resumed at an instruction that is not a send: {other:?}"
        ),
    }
}

/// Materializes a literal -- including the two deferred error cases,
/// `MalformedInteger` and `UnresolvedIdentifier`. Both raise through
/// `literals.rs`'s own constructors rather than rebuilding the error here:
/// error text lives in `interpreter/`, never in the VM.
fn make_value(
    mutator: &mut Mutator,
    activation: ActivationReference,
    literal: &Literal,
) -> EvaluationResult {
    Ok(match literal {
        Literal::Nil => Value::Nil,
        Literal::Boolean(boolean) => Value::Boolean(*boolean),
        Literal::SmallInteger(integer) => Value::SmallInteger(*integer),
        Literal::Float(float) => Value::Float(*float),
        Literal::String(string) => Value::Object(mutator.new_string(string.clone())),
        Literal::Symbol(symbol) => mutator.new_symbol(*symbol),
        Literal::Character(character) => Value::Object(mutator.new_character(*character)),
        Literal::Array(elements) => {
            let mut values = Vec::with_capacity(elements.len());
            for element in elements {
                values.push(make_value(mutator, activation, element)?);
            }
            Value::Object(mutator.new_array(values))
        }
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
