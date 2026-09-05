//! The register VM's run loop -- and nothing else. `run_code` is a `match`
//! over `Instr`; every `Send*` calls straight back into `interpreter/`'s
//! shared orchestration (`Interpreter::send`, `dispatch_lexical_send`,
//! `dispatch_super_send`), which re-enters `run_code` through the engine
//! hooks for any user-defined target. No dispatch logic, no
//! `doesNotUnderstand:`, no native dispatch, and no error text of its own
//! live here -- that is what makes the tree-walker a byte-for-byte oracle.
//! See `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`,
//! "`vm.rs` and the engine hook".

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{CompiledCode, Instr, Literal};
use crate::interpreter::eval::{
    dispatch_lexical_send, dispatch_super_send, frame_at_depth, home_of, parse_int_literal,
    read_local, store_instance_field, write_local,
};
use crate::interpreter::{EvalResult, Interpreter, Signal};
use crate::types::*;
use crate::Symbol;

/// Runs `code` on `frame` until a terminating instruction. `frame` was built
/// by the shared orchestration (`invoke_method`, `invoke_closure`, or
/// `instantiate`) with `temps` sized to `code.num_registers - code.num_args`;
/// `frame.pc` is kept at the executing instruction's index (the safepoint
/// view -- `statement_pc` recovers the statement on demand).
pub fn run_code(
    interp: &mut Interpreter,
    frame: &FrameRef,
    code: &Rc<CompiledCode>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let mut pc = 0usize;
    loop {
        frame.borrow_mut().pc = pc;
        match &code.instrs[pc] {
            Instr::LoadLiteral { dst, literal } => {
                let v = make_value(interp, literal)?;
                write_local(frame, *dst, v);
            }
            Instr::LoadSelf { dst } => {
                let v = frame.borrow().receiver.clone();
                write_local(frame, *dst, v);
            }
            Instr::LoadBuiltinClass { dst, kind } => {
                let v = Value::Obj(interp.builtin_class(*kind));
                write_local(frame, *dst, v);
            }
            Instr::Move { dst, src } => {
                let v = read_local(frame, *src);
                write_local(frame, *dst, v);
            }
            Instr::StoreLocal { src, slot } => {
                let v = read_local(frame, *src);
                write_local(frame, *slot, v);
            }
            Instr::LoadOuterLocal { dst, depth, index } => {
                let v = read_local(&frame_at_depth(frame, *depth), *index);
                write_local(frame, *dst, v);
            }
            Instr::StoreOuterLocal { src, depth, index } => {
                let v = read_local(frame, *src);
                write_local(&frame_at_depth(frame, *depth), *index, v);
            }
            Instr::SendPublic {
                selector,
                receiver_reg,
                arg_regs,
                dst,
                // `call_site` is deliberately ignored in this milestone --
                // it is the reserved, inert inline-cache slot; no cache is
                // wired up to read or write it yet.
                ..
            } => {
                let receiver = read_local(frame, *receiver_reg);
                let args = collect(frame, arg_regs);
                let result =
                    interp.send(receiver, *selector, args, bindings, Some(frame.clone()))?;
                write_local(frame, *dst, result);
            }
            Instr::SendLexical {
                depth,
                selector,
                arg_regs,
                dst,
                ..
            } => {
                let args = collect(frame, arg_regs);
                let result =
                    dispatch_lexical_send(interp, frame, *depth, *selector, args, bindings)?;
                write_local(frame, *dst, result);
            }
            Instr::SendSuper {
                selector,
                arg_regs,
                dst,
                ..
            } => {
                let args = collect(frame, arg_regs);
                let result = dispatch_super_send(interp, frame, *selector, args, bindings)?;
                write_local(frame, *dst, result);
            }
            Instr::MakeClosure {
                dst,
                params,
                code: block,
            } => {
                let closure = Rc::new(ClosureData {
                    params: (**params).clone(),
                    body: ClosureBody::Compiled(block.clone()),
                    captured_frame: frame.clone(),
                    home: home_of(frame),
                    bindings: bindings.clone(),
                });
                let v = Value::Obj(interp.new_closure(closure));
                write_local(frame, *dst, v);
            }
            Instr::MakeArray { dst, elem_regs } => {
                let values = collect(frame, elem_regs);
                let v = Value::Obj(interp.new_array(values));
                write_local(frame, *dst, v);
            }
            Instr::InitSlot { src, field_index } => {
                // Shares `store_instance_field` with
                // `instantiate::run_own_slots`: a direct field write on
                // `self`, silently skipped for a non-instance receiver.
                let v = read_local(frame, *src);
                let receiver = frame.borrow().receiver.clone();
                store_instance_field(&receiver, *field_index as usize, v);
            }
            Instr::Return { reg } | Instr::ImplicitReturn { reg } => {
                return Ok(read_local(frame, *reg));
            }
            Instr::NonLocalReturn { reg } => {
                let v = read_local(frame, *reg);
                return Err(interp.raise_return_pub(frame, v));
            }
            Instr::Jump { target } => {
                pc = *target as usize;
                continue;
            }
            Instr::JumpIfNotBoolean { reg, target } => {
                if !matches!(read_local(frame, *reg), Value::Bool(_)) {
                    pc = *target as usize;
                    continue;
                }
            }
            Instr::BranchIfFalse { reg, target } => {
                if matches!(read_local(frame, *reg), Value::Bool(false)) {
                    pc = *target as usize;
                    continue;
                }
            }
            Instr::BranchIfTrue { reg, target } => {
                if matches!(read_local(frame, *reg), Value::Bool(true)) {
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

fn collect(frame: &FrameRef, regs: &[u16]) -> Vec<Value> {
    regs.iter().map(|r| read_local(frame, *r)).collect()
}

/// Materializes a literal exactly as `eval::eval_literal` does -- including
/// the deferred `MalformedInt` case, which re-runs the shared
/// `parse_int_literal` and raises M1's own error (selector `parse`,
/// receiver `nil`, the parser's message).
fn make_value(interp: &mut Interpreter, literal: &Literal) -> EvalResult {
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
                values.push(make_value(interp, e)?);
            }
            Value::Obj(interp.new_array(values))
        }
        Literal::MalformedInt(text) => match parse_int_literal(text) {
            Ok(n) => Value::SmallInt(n),
            Err(message) => {
                return Err(Signal::Error(NewspeakError {
                    selector: Symbol::intern("parse"),
                    receiver: Value::Nil,
                    message,
                }))
            }
        },
    })
}
