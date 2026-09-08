//! The register VM's run loop -- and nothing else. `run_code` is a `match`
//! over `Instr`; every `Send*` calls straight back into `interpreter/`'s
//! shared orchestration (`Interpreter::send`, `dispatch_lexical_send`,
//! `dispatch_super_send`), which re-enters `run_code` through the engine
//! hooks for any user-defined target. No dispatch logic, no
//! `doesNotUnderstand:`, no native dispatch, and no error text of its own
//! live here -- that is what makes the tree-walker a byte-for-byte oracle.
//! See `docs/design/execution.md`, "The VM and the three hooks".

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{CompiledCode, Instr, Literal};
use crate::heap::Heap;
use crate::interpreter::eval::{
    dispatch_lexical_send, dispatch_super_send, frame_at_depth, home_of, malformed_int_error,
    parse_int_literal, read_local, store_instance_field, unresolved_identifier_error, write_local,
};
use crate::interpreter::{EvalResult, Interpreter};
use crate::types::*;

/// Runs `code` on `frame` until a terminating instruction. `frame` was built
/// by the shared orchestration (`invoke_method`, `invoke_closure`, or
/// `instantiate`) with `temps` sized to `code.num_registers - code.num_args`.
///
/// `frame.pc` is the safepoint view (`statement_pc` recovers the statement
/// from it on demand), and it is stored *only by the instructions that can
/// hand `frame` to an observer*: every `Send*` (a nested activation runs
/// with this frame as its sender), `MakeClosure` (the frame escapes into the
/// object graph, where `image::encode_frame` can serialize it), and
/// `NonLocalReturn` (control leaves with a signal). Nothing can read `pc`
/// while a straight-line instruction executes: `run_code` never yields
/// between them, and the only reader in the tree, `image::encode_frame`,
/// runs on a frame that reached it through one of those three. Keeping the
/// store out of the straight-line arms takes a `RefCell` borrow and a write
/// off the hot path for a value nothing observes there.
pub fn run_code(
    interp: &mut Interpreter,
    frame: &FrameRef,
    code: &Rc<CompiledCode>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let mut pc = 0usize;
    loop {
        match &code.instrs[pc] {
            Instr::LoadLiteral { dst, literal } => {
                let v = make_value(interp, frame, literal)?;
                write_local(&mut interp.heap, frame, *dst, v);
            }
            Instr::LoadSelf { dst } => {
                let v = frame.borrow().receiver.clone();
                write_local(&mut interp.heap, frame, *dst, v);
            }
            Instr::LoadBuiltinClass { dst, kind } => {
                let v = Value::Obj(interp.builtin_class(*kind));
                write_local(&mut interp.heap, frame, *dst, v);
            }
            Instr::Move { dst, src } => {
                let v = read_local(&interp.heap, frame, *src);
                write_local(&mut interp.heap, frame, *dst, v);
            }
            Instr::StoreLocal { src, slot } => {
                let v = read_local(&interp.heap, frame, *src);
                write_local(&mut interp.heap, frame, *slot, v);
            }
            Instr::LoadOuterLocal { dst, depth, index } => {
                let target = frame_at_depth(&interp.heap, frame, *depth);
                let v = read_local(&interp.heap, &target, *index);
                write_local(&mut interp.heap, frame, *dst, v);
            }
            Instr::StoreOuterLocal { src, depth, index } => {
                let v = read_local(&interp.heap, frame, *src);
                let target = frame_at_depth(&interp.heap, frame, *depth);
                write_local(&mut interp.heap, &target, *index, v);
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
                let receiver = read_local(&interp.heap, frame, *receiver_reg);
                let args = collect(&interp.heap, frame, arg_regs);
                frame.borrow_mut().pc = pc;
                let result =
                    interp.send(receiver, *selector, args, bindings, Some(frame.clone()))?;
                write_local(&mut interp.heap, frame, *dst, result);
            }
            Instr::SendLexical {
                depth,
                selector,
                arg_regs,
                dst,
                ..
            } => {
                let args = collect(&interp.heap, frame, arg_regs);
                frame.borrow_mut().pc = pc;
                let result =
                    dispatch_lexical_send(interp, frame, *depth, *selector, args, bindings)?;
                write_local(&mut interp.heap, frame, *dst, result);
            }
            Instr::SendSuper {
                selector,
                arg_regs,
                dst,
                ..
            } => {
                let args = collect(&interp.heap, frame, arg_regs);
                frame.borrow_mut().pc = pc;
                let result = dispatch_super_send(interp, frame, *selector, args, bindings)?;
                write_local(&mut interp.heap, frame, *dst, result);
            }
            Instr::MakeClosure {
                dst,
                params,
                code: block,
            } => {
                frame.borrow_mut().pc = pc;
                let closure = Rc::new(ClosureData {
                    params: (**params).clone(),
                    body: ClosureBody::Compiled(block.clone()),
                    captured_frame: frame.clone(),
                    home: home_of(&interp.heap, frame),
                    bindings: bindings.clone(),
                });
                let v = Value::Obj(interp.new_closure(closure));
                write_local(&mut interp.heap, frame, *dst, v);
            }
            Instr::MakeArray { dst, elem_regs } => {
                let values = collect(&interp.heap, frame, elem_regs);
                let v = Value::Obj(interp.new_array(values));
                write_local(&mut interp.heap, frame, *dst, v);
            }
            Instr::InitSlot { src, own_index } => {
                // Shares `store_instance_field` with
                // `instantiate::run_own_slots`: a direct field write on
                // `self`, silently skipped for a non-instance receiver.
                // `own_index` is relative to the executing mixin's own
                // slots (the same compiled body runs for every application
                // of that mixin); `frame.slot_base` -- set once, when
                // `Interpreter::run_initializer_level` builds this level's
                // frame, from that level's own
                // `MixinApplication.own_slot_base` -- is what turns it into
                // this instance's actual absolute field index.
                let v = read_local(&interp.heap, frame, *src);
                let receiver = frame.borrow().receiver.clone();
                let base = frame.borrow().slot_base;
                store_instance_field(&mut interp.heap, &receiver, base + *own_index as usize, v);
            }
            Instr::Return { reg } | Instr::ImplicitReturn { reg } => {
                return Ok(read_local(&interp.heap, frame, *reg));
            }
            Instr::NonLocalReturn { reg } => {
                let v = read_local(&interp.heap, frame, *reg);
                frame.borrow_mut().pc = pc;
                return Err(interp.raise_return_pub(frame, v));
            }
            Instr::Jump { target } => {
                pc = *target as usize;
                continue;
            }
            Instr::JumpIfNotBoolean { reg, target } => {
                if !matches!(read_local(&interp.heap, frame, *reg), Value::Bool(_)) {
                    pc = *target as usize;
                    continue;
                }
            }
            Instr::BranchIfFalse { reg, target } => {
                if matches!(read_local(&interp.heap, frame, *reg), Value::Bool(false)) {
                    pc = *target as usize;
                    continue;
                }
            }
            Instr::BranchIfTrue { reg, target } => {
                if matches!(read_local(&interp.heap, frame, *reg), Value::Bool(true)) {
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

fn collect(heap: &Heap, frame: &FrameRef, regs: &[u16]) -> Vec<Value> {
    regs.iter().map(|r| read_local(heap, frame, *r)).collect()
}

/// Materializes a literal exactly as `eval::eval_literal` does -- including
/// the two deferred error cases, `MalformedInt` and `UnresolvedIdent`.
/// Both raise through `eval.rs`'s own constructors rather than rebuilding
/// the error here, so the two engines cannot drift apart in wording.
fn make_value(interp: &mut Interpreter, frame: &FrameRef, literal: &Literal) -> EvalResult {
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
                values.push(make_value(interp, frame, e)?);
            }
            Value::Obj(interp.new_array(values))
        }
        Literal::MalformedInt(text) => match parse_int_literal(text) {
            Ok(n) => Value::SmallInt(n),
            Err(message) => return Err(malformed_int_error(message)),
        },
        Literal::UnresolvedIdent(name) => {
            let receiver = frame.borrow().receiver.clone();
            return Err(unresolved_identifier_error(name, receiver));
        }
    })
}
