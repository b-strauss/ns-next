//! The register VM's run loop -- and nothing else. `run` executes one
//! activation and everything it activates, on the `Heap`'s activation stack,
//! without recursing on the Rust stack: a send asks `interpreter/` what it
//! means (`Interpreter::decide_send`, `sends::decide_lexical_send`,
//! `sends::decide_super_send`) and, when the answer is an activation, pushes
//! it and runs it here; a return pops it and resumes the caller. No dispatch
//! logic, no `doesNotUnderstand:`, no native dispatch and no error text of its
//! own live here -- dispatch lives in one place, which the reference lowering
//! and a JIT share. See `docs/design/execution.md`, "The VM".

use std::rc::Rc;

use crate::bytecode::{CompiledCode, Instr, Literal};
use crate::heap::ActivationRef;
use crate::interpreter::literals::{
    malformed_int_error, parse_int_literal, unresolved_identifier_error,
};
use crate::interpreter::locals::{
    home_of, read_local, read_outer_local, store_instance_field, write_local, write_outer_local,
};
use crate::interpreter::sends::{decide_lexical_send, decide_super_send};
use crate::interpreter::{EvalResult, Interpreter, Signal, Step};
use crate::types::*;

/// What the loop does after one instruction.
enum Flow {
    Next,
    Jump(usize),
    /// Run another activation from `pc`: a callee a send pushed (from 0), or
    /// a caller a return resumed (from the instruction after its send).
    Switch {
        act: ActivationRef,
        code: Rc<CompiledCode>,
        pc: usize,
    },
    /// The activation `run` was given is done, and so is everything it
    /// activated.
    Exit(EvalResult),
}

/// Runs `base` -- the top of the activation stack, pushed by the caller --
/// and every activation it pushes, until `base` is done, and pops them all,
/// `base` included, on every path out: a return, an error, or a non-local
/// return aimed below `base`. The stack is exactly as deep afterwards as it
/// was before `base` was pushed. Its safepoints (`poll`) are every switch to
/// another activation and every backward jump.
///
/// An activation's `pc` is stored only by the instructions that can hand it
/// to an observer: every send (a callee runs above it, and the loop resumes
/// it from that `pc`), `MakeClosure` (it reifies the activation, so the frame
/// starts from the `pc` stored just before), and `NonLocalReturn` (control
/// leaves with a signal). Nothing reads `pc` while a straight-line
/// instruction executes.
pub(crate) fn run(interp: &mut Interpreter, base: ActivationRef) -> EvalResult {
    let floor = interp.heap.activation_position(base);
    let mut act = base;
    let mut code = interp.heap.activation_code(act);
    let mut pc = 0usize;
    loop {
        match execute(interp, act, &code, pc, floor) {
            Flow::Next => {
                pc += 1;
                debug_assert!(
                    pc < code.instrs.len(),
                    "internal: pc ran past the end of a CompiledCode with no terminating return"
                );
            }
            Flow::Jump(target) => {
                // A backward jump closes a loop, so that a loop that makes no
                // send still passes a safepoint.
                if target <= pc {
                    poll(interp);
                }
                pc = target;
            }
            Flow::Switch {
                act: next,
                code: next_code,
                pc: next_pc,
            } => {
                act = next;
                code = next_code;
                pc = next_pc;
                poll(interp);
            }
            Flow::Exit(result) => return result,
        }
    }
}

/// The safepoint, reached at every switch to another activation -- a push,
/// or a caller resumed -- and at every backward jump. Between two
/// instructions the loop holds the running `ActivationRef`, its code and its
/// `pc`, none of which a move changes, so a collection may run here and
/// nowhere else (`docs/design/memory.md`, "Where a collection can happen").
/// One compare of a flag the heap owns; what happens when it is set is
/// `interpreter/`'s.
#[inline(always)]
fn poll(interp: &mut Interpreter) {
    if interp.heap.safepoint_requested() {
        interp.safepoint();
    }
}

/// One instruction of `code`, running in `act`.
#[inline(always)]
fn execute(
    interp: &mut Interpreter,
    act: ActivationRef,
    code: &CompiledCode,
    pc: usize,
    floor: usize,
) -> Flow {
    match &code.instrs[pc] {
        Instr::LoadLiteral { dst, literal } => match make_value(interp, act, literal) {
            Ok(v) => {
                write_local(&mut interp.heap, act, *dst, v);
                Flow::Next
            }
            Err(signal) => unwind(interp, signal, floor),
        },
        Instr::LoadSelf { dst } => {
            let v = interp.heap.activation_receiver(act);
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::LoadBuiltinClass { dst, kind } => {
            let v = Value::Obj(interp.builtin_class(*kind));
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::Move { dst, src } | Instr::StoreLocal { src, slot: dst } => {
            let v = read_local(&interp.heap, act, *src);
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::LoadOuterLocal { dst, depth, index } => {
            let v = read_outer_local(&interp.heap, act, *depth, *index);
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::StoreOuterLocal { src, depth, index } => {
            let v = read_local(&interp.heap, act, *src);
            write_outer_local(&mut interp.heap, act, *depth, *index, v);
            Flow::Next
        }
        Instr::SendPublic {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            // `call_site` is the reserved, inert inline-cache slot; nothing
            // reads or writes it yet.
            ..
        } => {
            let receiver = read_local(&interp.heap, act, *receiver_reg);
            send(
                interp,
                act,
                pc,
                arg_regs,
                *dst,
                floor,
                false,
                |interp, args| interp.decide_send(receiver, *selector, args),
            )
        }
        // Decided exactly as `SendPublic` is; only the depth count treats
        // the block a control-flow native activates for it differently
        // (`Interpreter::push_step`).
        Instr::SendInlinable {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            ..
        } => {
            let receiver = read_local(&interp.heap, act, *receiver_reg);
            send(
                interp,
                act,
                pc,
                arg_regs,
                *dst,
                floor,
                true,
                |interp, args| interp.decide_send(receiver, *selector, args),
            )
        }
        Instr::SendLexical {
            depth,
            selector,
            arg_regs,
            dst,
            ..
        } => send(
            interp,
            act,
            pc,
            arg_regs,
            *dst,
            floor,
            false,
            |interp, args| decide_lexical_send(interp, act, *depth, *selector, args),
        ),
        Instr::SendSuper {
            selector,
            arg_regs,
            dst,
            ..
        } => send(
            interp,
            act,
            pc,
            arg_regs,
            *dst,
            floor,
            false,
            |interp, args| decide_super_send(interp, act, *selector, args),
        ),
        Instr::MakeClosure {
            dst,
            params,
            code: block,
        } => {
            // Stored before reifying, so the frame `reify` builds starts
            // from this pc.
            interp.heap.set_activation_pc(act, pc);
            let captured_frame = interp.heap.reify(act);
            let home = home_of(&mut interp.heap, act);
            let v = Value::Obj(interp.new_closure(ClosureData {
                params: params.clone(),
                code: block.clone(),
                captured_frame,
                home,
            }));
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::MakeArray { dst, elem_regs } => {
            let values = elem_regs
                .iter()
                .map(|r| read_local(&interp.heap, act, *r))
                .collect();
            let v = Value::Obj(interp.new_array(values));
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::InitSlot { src, own_index } => {
            // `own_index` is relative to the executing mixin's own slots; the
            // activation's `slot_base`, set when this level's activation was
            // pushed, turns it into this instance's absolute field index.
            let v = read_local(&interp.heap, act, *src);
            let receiver = interp.heap.activation_receiver(act);
            let base = interp.heap.activation_slot_base(act);
            store_instance_field(&mut interp.heap, &receiver, base + *own_index as usize, v);
            Flow::Next
        }
        Instr::Return { reg } | Instr::ImplicitReturn { reg } => {
            let value = read_local(&interp.heap, act, *reg);
            finish(interp, act, value, floor)
        }
        Instr::NonLocalReturn { reg } => {
            let value = read_local(&interp.heap, act, *reg);
            interp.heap.set_activation_pc(act, pc);
            let signal = interp.raise_return(act, value);
            unwind(interp, signal, floor)
        }
        Instr::Jump { target } => Flow::Jump(*target as usize),
        Instr::JumpIfNotBoolean { reg, target } => {
            if matches!(read_local(&interp.heap, act, *reg), Value::Bool(_)) {
                Flow::Next
            } else {
                Flow::Jump(*target as usize)
            }
        }
        Instr::BranchIfFalse { reg, target } => {
            if matches!(read_local(&interp.heap, act, *reg), Value::Bool(false)) {
                Flow::Jump(*target as usize)
            } else {
                Flow::Next
            }
        }
        Instr::BranchIfTrue { reg, target } => {
            if matches!(read_local(&interp.heap, act, *reg), Value::Bool(true)) {
                Flow::Jump(*target as usize)
            } else {
                Flow::Next
            }
        }
    }
}

/// A send: the arguments gathered from the caller's registers into the
/// interpreter's reusable buffer, `pc` stored, the decision asked for, and
/// what was decided run -- a value written to `dst`, or an activation pushed
/// and switched to, its window filled straight from the buffer. No vector is
/// allocated once the buffer has grown to the widest send. `inlinable` says
/// whether the instruction was a `SendInlinable`, which `push_step` hands to
/// the depth count along with the sender, `act`.
#[allow(clippy::too_many_arguments)]
fn send(
    interp: &mut Interpreter,
    act: ActivationRef,
    pc: usize,
    arg_regs: &[u16],
    dst: u16,
    floor: usize,
    inlinable: bool,
    decide: impl FnOnce(&mut Interpreter, &[Value]) -> Result<Step, Signal>,
) -> Flow {
    let mut args = interp.take_arg_buffer();
    args.extend(arg_regs.iter().map(|r| read_local(&interp.heap, act, *r)));
    interp.heap.set_activation_pc(act, pc);
    let flow = match decide(interp, &args) {
        Ok(Step::Value(v)) => {
            write_local(&mut interp.heap, act, dst, v);
            Flow::Next
        }
        Ok(step) => match interp.push_step(step, &args, Some(act), inlinable) {
            Ok(callee) => Flow::Switch {
                act: callee,
                code: interp.heap.activation_code(callee),
                pc: 0,
            },
            Err(signal) => unwind(interp, signal, floor),
        },
        Err(signal) => unwind(interp, signal, floor),
    };
    interp.return_arg_buffer(args);
    flow
}

/// `act` has returned `value`: pop it and do what its continuation says. A
/// value -- the usual case -- goes to the send in the caller, which resumes,
/// or out of `run` when there is no caller in this run; a control-flow
/// native's continuation may instead decide another activation -- a native
/// loop's next block, or a `mustBeBoolean` method -- which runs in the
/// popped activation's place. `Interpreter::resume` applies the
/// continuation and `Interpreter::push_continuation` pushes what it decided,
/// depth count included; the loop only forwards the popped entry's flag.
fn finish(interp: &mut Interpreter, act: ActivationRef, value: Value, floor: usize) -> Flow {
    let popped = interp.heap.pop_activation(act);
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
        then => match interp.resume(then, value) {
            Ok(Step::Value(v)) => v,
            Ok(step) => {
                return match interp.push_continuation(step, popped.counted) {
                    Ok(next) => Flow::Switch {
                        act: next,
                        code: interp.heap.activation_code(next),
                        pc: 0,
                    },
                    Err(signal) => unwind(interp, signal, floor),
                };
            }
            Err(signal) => return unwind(interp, signal, floor),
        },
    };
    if interp.heap.activation_depth() == floor {
        return Flow::Exit(Ok(value));
    }
    let caller = interp
        .heap
        .top_activation()
        .expect("internal: an activation above the run's floor has a caller");
    resume_caller(interp, caller, value)
}

/// Writes a callee's value into the send `caller` is stopped at, and resumes
/// `caller` after it.
fn resume_caller(interp: &mut Interpreter, caller: ActivationRef, value: Value) -> Flow {
    let code = interp.heap.activation_code(caller);
    let pc = interp.heap.activation_pc(caller);
    write_local(
        &mut interp.heap,
        caller,
        send_destination(&code.instrs[pc]),
        value,
    );
    Flow::Switch {
        act: caller,
        code,
        pc: pc + 1,
    }
}

/// A signal leaving the running activation. A non-local return whose target
/// is one of this run's activations and catches a return (a method, or a
/// superclass clause or its message -- `Interpreter::catches_return`) ends
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
/// activation it pops hands its continuation to `Interpreter::abandon`, which
/// undoes what a continuation that will now never run had begun.
fn unwind(interp: &mut Interpreter, signal: Signal, floor: usize) -> Flow {
    if let Signal::Return(target, value) = &signal {
        let (target, value) = (*target, *value);
        if interp.heap.activation_is_live(target)
            && interp.heap.activation_position(target) >= floor
            && interp.catches_return(target)
        {
            while let Some(top) = interp.heap.top_activation() {
                if top.same(target) {
                    break;
                }
                let popped = interp.heap.pop_activation(top);
                interp.abandon(popped.then);
            }
            return finish(interp, target, value, floor);
        }
    }
    while interp.heap.activation_depth() > floor {
        let top = interp
            .heap
            .top_activation()
            .expect("internal: a stack deeper than the floor has a top");
        let popped = interp.heap.pop_activation(top);
        interp.abandon(popped.then);
    }
    Flow::Exit(Err(signal))
}

/// The register a send writes its value to: where a callee's value goes when
/// the loop resumes the caller.
fn send_destination(instr: &Instr) -> u16 {
    match instr {
        Instr::SendPublic { dst, .. }
        | Instr::SendInlinable { dst, .. }
        | Instr::SendLexical { dst, .. }
        | Instr::SendSuper { dst, .. } => *dst,
        other => unreachable!(
            "internal: a caller resumed at an instruction that is not a send: {other:?}"
        ),
    }
}

/// Materializes a literal -- including the two deferred error cases,
/// `MalformedInt` and `UnresolvedIdent`. Both raise through `literals.rs`'s own
/// constructors rather than rebuilding the error here: error text lives in
/// `interpreter/`, never in the VM.
fn make_value(interp: &mut Interpreter, act: ActivationRef, literal: &Literal) -> EvalResult {
    Ok(match literal {
        Literal::Nil => Value::Nil,
        Literal::Bool(b) => Value::Bool(*b),
        Literal::SmallInt(n) => Value::SmallInt(*n),
        Literal::Float(f) => Value::Float(*f),
        Literal::Str(s) => Value::Obj(interp.new_string(s.clone())),
        Literal::Sym(s) => interp.new_symbol(*s),
        Literal::Char(c) => Value::Obj(interp.new_character(*c)),
        Literal::Array(elements) => {
            let mut values = Vec::with_capacity(elements.len());
            for e in elements {
                values.push(make_value(interp, act, e)?);
            }
            Value::Obj(interp.new_array(values))
        }
        Literal::MalformedInt(text) => match parse_int_literal(text) {
            Ok(n) => Value::SmallInt(n),
            Err(message) => return Err(malformed_int_error(message)),
        },
        Literal::UnresolvedIdent(name) => {
            let receiver = interp.heap.activation_receiver(act);
            return Err(unresolved_identifier_error(name, receiver));
        }
    })
}
