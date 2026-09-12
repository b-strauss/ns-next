//! The register VM's run loop -- and nothing else. `run_code` is a `match`
//! over `Instr`; every `Send*` calls straight back into `interpreter/`'s
//! send machinery (`Interpreter::send`, `dispatch_lexical_send`,
//! `dispatch_super_send`), which re-enters `run_code` for any
//! user-defined target. No dispatch logic, no `doesNotUnderstand:`, no
//! native dispatch, and no error text of its own live here -- dispatch
//! lives in one place, which the reference lowering and a JIT will share.
//! See `docs/design/execution.md`, "The VM".

use std::rc::Rc;

use crate::bytecode::{CompiledCode, Instr, Literal};
use crate::heap::{ActivationRef, Heap};
use crate::interpreter::literals::{
    malformed_int_error, parse_int_literal, unresolved_identifier_error,
};
use crate::interpreter::locals::{
    home_of, read_local, read_outer_local, store_instance_field, write_local, write_outer_local,
};
use crate::interpreter::sends::{dispatch_lexical_send, dispatch_super_send};
use crate::interpreter::{EvalResult, Interpreter};
use crate::types::*;

/// Runs `code` on the live activation `act` until a terminating
/// instruction. `act` was pushed by the send machinery (`invoke_method`,
/// `invoke_closure`, `instantiate` or `nested::run_clause_body`) with
/// registers for `code.num_registers`, and that same caller pops it.
///
/// The activation's `pc` is the safepoint view (`statement_pc` recovers the
/// statement from it on demand), and it is stored *only by the instructions
/// that can hand the activation to an observer*: every `Send*` (a nested
/// activation runs above this one), `MakeClosure` (it reifies this
/// activation, so the frame `reify` builds starts from the `pc` stored just
/// before), and `NonLocalReturn` (control leaves with a signal). Each store
/// goes to the stack entry and keeps the activation's `pc` current; a frame
/// divorced when the activation returns records the last one stored, which
/// is often a later send's rather than the `MakeClosure`'s. Nothing can read
/// `pc` while a straight-line instruction executes, because `run_code` never
/// yields between them.
pub fn run_code(
    interp: &mut Interpreter,
    act: ActivationRef,
    code: &Rc<CompiledCode>,
) -> EvalResult {
    let mut pc = 0usize;
    loop {
        match &code.instrs[pc] {
            Instr::LoadLiteral { dst, literal } => {
                let v = make_value(interp, act, literal)?;
                write_local(&mut interp.heap, act, *dst, v);
            }
            Instr::LoadSelf { dst } => {
                let v = interp.heap.activation_receiver(act);
                write_local(&mut interp.heap, act, *dst, v);
            }
            Instr::LoadBuiltinClass { dst, kind } => {
                let v = Value::Obj(interp.builtin_class(*kind));
                write_local(&mut interp.heap, act, *dst, v);
            }
            Instr::Move { dst, src } => {
                let v = read_local(&interp.heap, act, *src);
                write_local(&mut interp.heap, act, *dst, v);
            }
            Instr::StoreLocal { src, slot } => {
                let v = read_local(&interp.heap, act, *src);
                write_local(&mut interp.heap, act, *slot, v);
            }
            Instr::LoadOuterLocal { dst, depth, index } => {
                let v = read_outer_local(&interp.heap, act, *depth, *index);
                write_local(&mut interp.heap, act, *dst, v);
            }
            Instr::StoreOuterLocal { src, depth, index } => {
                let v = read_local(&interp.heap, act, *src);
                write_outer_local(&mut interp.heap, act, *depth, *index, v);
            }
            Instr::SendPublic {
                selector,
                receiver_reg,
                arg_regs,
                dst,
                // `call_site` is deliberately ignored -- it is the
                // reserved, inert inline-cache slot; no cache is
                // wired up to read or write it yet.
                ..
            }
            // `SendInlinable` is `SendPublic` with a mark only the depth
            // count reads (Task 6 of the trampoline plan); until then it
            // runs identically.
            | Instr::SendInlinable {
                selector,
                receiver_reg,
                arg_regs,
                dst,
                ..
            } => {
                let receiver = read_local(&interp.heap, act, *receiver_reg);
                let args = collect(&interp.heap, act, arg_regs);
                interp.heap.set_activation_pc(act, pc);
                let result = interp.send(receiver, *selector, args)?;
                write_local(&mut interp.heap, act, *dst, result);
            }
            Instr::SendLexical {
                depth,
                selector,
                arg_regs,
                dst,
                ..
            } => {
                let args = collect(&interp.heap, act, arg_regs);
                interp.heap.set_activation_pc(act, pc);
                let result = dispatch_lexical_send(interp, act, *depth, *selector, args)?;
                write_local(&mut interp.heap, act, *dst, result);
            }
            Instr::SendSuper {
                selector,
                arg_regs,
                dst,
                ..
            } => {
                let args = collect(&interp.heap, act, arg_regs);
                interp.heap.set_activation_pc(act, pc);
                let result = dispatch_super_send(interp, act, *selector, args)?;
                write_local(&mut interp.heap, act, *dst, result);
            }
            Instr::MakeClosure {
                dst,
                params,
                code: block,
            } => {
                // Stored before reifying, so the frame `reify` builds
                // starts from this pc. The store keeps the activation's pc
                // current; the frame records whichever pc was stored last
                // when `pop_activation` divorces it.
                interp.heap.set_activation_pc(act, pc);
                let captured_frame = interp.heap.reify(act);
                let home = home_of(&mut interp.heap, act);
                let closure = Rc::new(ClosureData {
                    params: params.clone(),
                    code: block.clone(),
                    captured_frame,
                    home,
                });
                let v = Value::Obj(interp.new_closure(closure));
                write_local(&mut interp.heap, act, *dst, v);
            }
            Instr::MakeArray { dst, elem_regs } => {
                let values = collect(&interp.heap, act, elem_regs);
                let v = Value::Obj(interp.new_array(values));
                write_local(&mut interp.heap, act, *dst, v);
            }
            Instr::InitSlot { src, own_index } => {
                // Through `store_instance_field`: a direct field write on
                // `self`, silently skipped for a non-instance receiver.
                // `own_index` is relative to the executing mixin's own
                // slots (the same compiled body runs for every application
                // of that mixin); the activation's `slot_base` -- set once,
                // when `Interpreter::run_initializer_level` pushes this
                // level's activation, from that level's own
                // `MixinApplication.own_slot_base` -- is what turns it into
                // this instance's actual absolute field index.
                let v = read_local(&interp.heap, act, *src);
                let receiver = interp.heap.activation_receiver(act);
                let base = interp.heap.activation_slot_base(act);
                store_instance_field(&mut interp.heap, &receiver, base + *own_index as usize, v);
            }
            Instr::Return { reg } | Instr::ImplicitReturn { reg } => {
                return Ok(read_local(&interp.heap, act, *reg));
            }
            Instr::NonLocalReturn { reg } => {
                let v = read_local(&interp.heap, act, *reg);
                interp.heap.set_activation_pc(act, pc);
                return Err(interp.raise_return(act, v));
            }
            Instr::Jump { target } => {
                pc = *target as usize;
                continue;
            }
            Instr::JumpIfNotBoolean { reg, target } => {
                if !matches!(read_local(&interp.heap, act, *reg), Value::Bool(_)) {
                    pc = *target as usize;
                    continue;
                }
            }
            Instr::BranchIfFalse { reg, target } => {
                if matches!(read_local(&interp.heap, act, *reg), Value::Bool(false)) {
                    pc = *target as usize;
                    continue;
                }
            }
            Instr::BranchIfTrue { reg, target } => {
                if matches!(read_local(&interp.heap, act, *reg), Value::Bool(true)) {
                    pc = *target as usize;
                    continue;
                }
            }
        }
        pc += 1;
        debug_assert!(
            pc < code.instrs.len(),
            "internal: pc ran past the end of a CompiledCode with no terminating return"
        );
    }
}

fn collect(heap: &Heap, act: ActivationRef, regs: &[u16]) -> Vec<Value> {
    regs.iter().map(|r| read_local(heap, act, *r)).collect()
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
