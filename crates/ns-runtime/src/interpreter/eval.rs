//! Expression-level evaluation: literals, sends (ordinary, `LexicalSend`,
//! `SuperSend`), cascades, blocks, assignment.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr};
use ns_syntax::Span;

use crate::dispatch::{self, Lookup};
use crate::types::*;
use crate::Symbol;

use super::{EvalResult, Interpreter, Signal};

pub fn eval_expr(
    interp: &mut Interpreter,
    expr: &Expr,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    match expr {
        Expr::Ident(id) => eval_ident(interp, id, frame, bindings),
        Expr::Literal(lit) => eval_literal(interp, lit),
        Expr::Send(send) => eval_send(interp, send, frame, bindings),
        Expr::Cascade(cascade) => eval_cascade(interp, cascade, frame, bindings),
        Expr::Assign { target, value, .. } => eval_assign(interp, target, value, frame, bindings),
        Expr::Block(block) => eval_block_literal(interp, block, frame, bindings),
        Expr::DynamicArray { elements, .. } => {
            let mut values = Vec::with_capacity(elements.len());
            for e in elements {
                values.push(eval_expr(interp, e, frame, bindings)?);
            }
            Ok(Value::Obj(interp.new_array(values)))
        }
    }
}

fn eval_ident(
    interp: &mut Interpreter,
    id: &ast::Ident,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    if id.name == "self" {
        return Ok(frame.borrow().receiver.clone());
    }
    match bindings.get(&id.span) {
        Some(Binding::Local(i)) => Ok(read_local(frame, *i)),
        Some(Binding::OuterLocal { depth, index }) => Ok(read_outer_local(frame, *depth, *index)),
        Some(Binding::LexicalSend { depth }) => dispatch_lexical_send(
            interp,
            frame,
            *depth,
            Symbol::intern(&id.name),
            vec![],
            bindings,
        ),
        Some(Binding::SuperSend) | None => Err(Signal::Error(NewspeakError {
            selector: Symbol::intern(&id.name),
            receiver: frame.borrow().receiver.clone(),
            message: format!("internal: unresolved identifier `{}`", id.name),
        })),
    }
}

fn read_local(frame: &FrameRef, index: u16) -> Value {
    let borrowed = frame.borrow();
    let i = index as usize;
    if i < borrowed.args.len() {
        borrowed.args[i].clone()
    } else {
        borrowed.temps[i - borrowed.args.len()].clone()
    }
}

fn write_local(frame: &FrameRef, index: u16, value: Value) {
    let mut borrowed = frame.borrow_mut();
    let i = index as usize;
    if i < borrowed.args.len() {
        borrowed.args[i] = value;
    } else {
        let temp_index = i - borrowed.args.len();
        borrowed.temps[temp_index] = value;
    }
}

fn frame_at_depth(frame: &FrameRef, depth: u16) -> FrameRef {
    let mut current = frame.clone();
    for _ in 0..depth {
        let next = match &current.borrow().kind {
            FrameKind::Closure(closure) => closure.captured_frame.clone(),
            _ => current.clone(),
        };
        current = next;
    }
    current
}

fn read_outer_local(frame: &FrameRef, depth: u16, index: u16) -> Value {
    read_local(&frame_at_depth(frame, depth), index)
}

fn write_outer_local(frame: &FrameRef, depth: u16, index: u16, value: Value) {
    write_local(&frame_at_depth(frame, depth), index, value);
}

fn eval_literal(interp: &mut Interpreter, lit: &ast::Literal) -> EvalResult {
    Ok(match lit {
        ast::Literal::Int { text, .. } => {
            let n: i64 = text.parse().map_err(|_| {
                Signal::Error(NewspeakError {
                    selector: Symbol::intern("parse"),
                    receiver: Value::Nil,
                    message: format!("integer literal out of i64 range: {text}"),
                })
            })?;
            Value::SmallInt(n)
        }
        ast::Literal::Float { text, .. } => {
            let n: f64 = text.parse().unwrap_or(f64::NAN);
            Value::Float(n)
        }
        ast::Literal::String { value, .. } => Value::Obj(interp.new_string(value.clone())),
        ast::Literal::Symbol { value, .. } => interp.new_symbol(Symbol::intern(value)),
        ast::Literal::Char { value, .. } => Value::Obj(interp.new_character(*value)),
        ast::Literal::Bool { value, .. } => Value::Bool(*value),
        ast::Literal::Nil { .. } => Value::Nil,
        ast::Literal::Array { elements, .. } => {
            let mut values = Vec::with_capacity(elements.len());
            for e in elements {
                let ev = eval_literal(interp, e)?;
                values.push(ev);
            }
            Value::Obj(interp.new_array(values))
        }
        ast::Literal::Pattern { .. } => Value::Nil, // Non-Goal: typechecker/patterns (M1 spec)
    })
}

fn eval_send(
    interp: &mut Interpreter,
    send: &ast::Send,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    match bindings.get(&send.span) {
        Some(Binding::SuperSend) => {
            let args = eval_args(interp, &send.message, frame, bindings)?;
            dispatch_super_send(interp, frame, selector_of(&send.message), args, bindings)
        }
        Some(Binding::LexicalSend { depth }) => {
            let args = eval_args(interp, &send.message, frame, bindings)?;
            dispatch_lexical_send(
                interp,
                frame,
                *depth,
                selector_of(&send.message),
                args,
                bindings,
            )
        }
        _ => {
            let receiver = eval_expr(interp, &send.receiver, frame, bindings)?;
            let args = eval_args(interp, &send.message, frame, bindings)?;
            interp.send(
                receiver,
                selector_of(&send.message),
                args,
                bindings,
                Some(frame.clone()),
            )
        }
    }
}

fn eval_args(
    interp: &mut Interpreter,
    message: &ast::MessageSend,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Result<Vec<Value>, Signal> {
    let mut values = Vec::with_capacity(message.args.len());
    for a in &message.args {
        values.push(eval_expr(interp, a, frame, bindings)?);
    }
    Ok(values)
}

fn selector_of(message: &ast::MessageSend) -> Symbol {
    match &message.selector {
        ast::Selector::Unary(s) | ast::Selector::Binary(s) => Symbol::intern(s),
        ast::Selector::Keyword(s) => Symbol::intern(s),
    }
}

/// A slot's getter (`x`, no colon) or setter (`x:`, exactly one trailing
/// colon and no others) shape, per `newspeak-spec.pdf` §6.3.2 -- a slot's
/// getter/setter are "implicitly defined by the slot declaration", not real
/// `MethodDecl`s in `ClassData.methods`, so `dispatch_lexical_send` must
/// recognize and serve them directly rather than relying on method lookup
/// to ever find them. Returns `None` for a genuine multi-keyword selector
/// (e.g. `at:put:`), which can never denote a slot access.
fn slot_selector_shape(selector: Symbol) -> Option<(Symbol, bool)> {
    let s = selector.as_str();
    if !s.contains(':') {
        return Some((selector, false));
    }
    if s.ends_with(':') && s.matches(':').count() == 1 {
        return Some((Symbol::intern(&s[..s.len() - 1]), true));
    }
    None
}

/// `newspeak-spec.pdf` §5.7-§5.9: `o` = the kth enclosing object of `self`;
/// dispatch privately (direct, non-virtual) if the target is `private` at
/// depth `k`, otherwise virtually via `lookup_protected` on `o`'s actual
/// class.
fn dispatch_lexical_send(
    interp: &mut Interpreter,
    frame: &FrameRef,
    depth: u16,
    selector: Symbol,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let self_value = frame.borrow().receiver.clone();
    let o = dispatch::kth_enclosing_object(&self_value, depth);
    let class = interp.class_of(&o);
    // Slot access fix (Task 7 "must fix before committing" #2): a
    // `LexicalSend` to a user-defined class's own slot must resolve here,
    // *before* the method/native lookups below -- `load_class` never puts a
    // slot's implicit getter/setter into `ClassData.methods`, so
    // `lookup_defined_for`/`lookup_protected` would never find one.
    // Milestone 1 known gap: unlike a method send, this is never
    // access-controlled by the slot's own declared modifier (see the task's
    // design note) -- revisit if a fixture needs it.
    if let Some((name, is_setter)) = slot_selector_shape(selector) {
        if let Some(index) = dispatch::lookup_slot(&class, name) {
            if let Value::Obj(obj) = &o {
                if is_setter {
                    let value = args.first().cloned().unwrap_or(Value::Nil);
                    if let ObjectData::Instance(fields) = &mut obj.borrow_mut().data {
                        if index < fields.len() {
                            fields[index] = value;
                        }
                    }
                    return Ok(o);
                } else if let ObjectData::Instance(fields) = &obj.borrow().data {
                    if index < fields.len() {
                        return Ok(fields[index].clone());
                    }
                }
            }
        }
    }
    // Check for a private declaration *at exactly this class* first (the
    // non-virtual case -- private members are never overridden, so there
    // is no search: newspeak-spec.pdf §5.9).
    if let Lookup::Method(method, defining_class) = dispatch::lookup_defined_for(&class, selector) {
        if dispatch::effective_access(method.access) == ast::AccessModifier::Private {
            return interp.invoke_method(
                o,
                method,
                defining_class,
                args,
                bindings.clone(),
                Some(frame.clone()),
            );
        }
    }
    match dispatch::lookup_protected(&class, selector) {
        Lookup::Method(method, defining_class) => interp.invoke_method(
            o,
            method,
            defining_class,
            args,
            bindings.clone(),
            Some(frame.clone()),
        ),
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(interp, kind, o.clone(), selector, &args) {
                Some(result) => result.map_err(Signal::Error),
                None => interp.send(o, selector, args, bindings, Some(frame.clone())),
            }
        }
        Lookup::NotFound => interp.send(o, selector, args, bindings, Some(frame.clone())),
    }
}

/// `newspeak-spec.pdf` §5.10: starts at the superclass of the *current
/// activation's* `current_class`, not `self`'s own class. On success, the
/// new activation's `current_class` is `definingClass(selector, superclass)`
/// -- `invoke_method`'s `defining_class` argument (from `lookup_defined_for`)
/// already carries that.
fn dispatch_super_send(
    interp: &mut Interpreter,
    frame: &FrameRef,
    selector: Symbol,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let Some(current_class) = frame.borrow().current_class.clone() else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: frame.borrow().receiver.clone(),
            message:
                "super send with no enclosing current_class (used outside a method activation)"
                    .to_string(),
        }));
    };
    let superclass = {
        let borrowed = current_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => data.superclass.clone(),
            _ => None,
        }
    };
    let Some(superclass) = superclass else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: frame.borrow().receiver.clone(),
            message: "super send with no superclass (current class is Object or Top)".to_string(),
        }));
    };
    let self_value = frame.borrow().receiver.clone();
    match dispatch::lookup_defined_for(&superclass, selector) {
        Lookup::Method(method, defining_class) => interp.invoke_method(
            self_value,
            method,
            defining_class,
            args,
            bindings.clone(),
            Some(frame.clone()),
        ),
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(
                interp,
                kind,
                self_value.clone(),
                selector,
                &args,
            ) {
                Some(result) => result.map_err(Signal::Error),
                None => interp.send(self_value, selector, args, bindings, Some(frame.clone())),
            }
        }
        Lookup::NotFound => interp.send(self_value, selector, args, bindings, Some(frame.clone())),
    }
}

fn eval_cascade(
    interp: &mut Interpreter,
    cascade: &ast::Cascade,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let receiver = eval_expr(interp, &cascade.receiver, frame, bindings)?;
    let first_args = eval_args(interp, &cascade.first, frame, bindings)?;
    let mut result = interp.send(
        receiver.clone(),
        selector_of(&cascade.first),
        first_args,
        bindings,
        Some(frame.clone()),
    )?;
    for m in &cascade.rest {
        let args = eval_args(interp, m, frame, bindings)?;
        result = interp.send(
            receiver.clone(),
            selector_of(m),
            args,
            bindings,
            Some(frame.clone()),
        )?;
    }
    Ok(result)
}

/// `id:: e` (`Expr::Assign`): resolves and dispatches exactly like an
/// implicit-receiver keyword send to `id:`, per the runtime spec's "Setter
/// sends" note, then yields `e`'s value (not the setter's own return
/// value, per §6.3.2/§5.4.3).
fn eval_assign(
    interp: &mut Interpreter,
    target: &ast::Ident,
    value: &Expr,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let v = eval_expr(interp, value, frame, bindings)?;
    match bindings.get(&target.span) {
        Some(Binding::Local(i)) => write_local(frame, *i, v.clone()),
        Some(Binding::OuterLocal { depth, index }) => {
            write_outer_local(frame, *depth, *index, v.clone())
        }
        Some(Binding::LexicalSend { depth }) => {
            let setter = Symbol::intern(&format!("{}:", target.name));
            dispatch_lexical_send(interp, frame, *depth, setter, vec![v.clone()], bindings)?;
        }
        _ => {
            return Err(Signal::Error(NewspeakError {
                selector: Symbol::intern(&target.name),
                receiver: frame.borrow().receiver.clone(),
                message: format!("internal: unresolved setter target `{}`", target.name),
            }))
        }
    }
    Ok(v)
}

fn eval_block_literal(
    interp: &mut Interpreter,
    block: &ast::BlockExpr,
    frame: &FrameRef,
    _bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let home = match &frame.borrow().kind {
        FrameKind::Method(_) | FrameKind::TopLevel => frame.clone(),
        FrameKind::Closure(closure) => closure.home.clone(),
    };
    let closure = Rc::new(ClosureData {
        params: block
            .params
            .iter()
            .map(|p| Symbol::intern(&p.name))
            .collect(),
        body: Rc::new(block.body.clone()),
        captured_frame: frame.clone(),
        home,
    });
    Ok(Value::Obj(interp.new_closure(closure)))
}

/// Invokes a closure with `args`. Called by `builtins::block` (Task 10) for
/// `value`, `value:`, `value:value:`, etc. Public because block-invocation
/// selectors are Milestone 1 *native* methods on `BuiltinClass::Block`, not
/// user-overridable Newspeak methods.
pub fn invoke_closure(
    interp: &mut Interpreter,
    closure: &Rc<ClosureData>,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let receiver = closure.captured_frame.borrow().receiver.clone();
    let current_class = closure.captured_frame.borrow().current_class.clone();
    let frame = Rc::new(RefCell::new(Frame {
        receiver,
        args,
        temps: vec![Value::Nil; closure.body.temps.len()],
        pc: 0,
        sender: None,
        home: Some(closure.home.clone()),
        current_class,
        kind: FrameKind::Closure(closure.clone()),
        uncontinuable: false,
    }));
    let mut last = Value::Nil;
    for (i, stmt) in closure.body.stmts.iter().enumerate() {
        frame.borrow_mut().pc = i;
        match stmt {
            ast::Stmt::Expr(e) => last = eval_expr(interp, e, &frame, bindings)?,
            ast::Stmt::Return(e) => {
                let value = eval_expr(interp, e, &frame, bindings)?;
                return Err(super::Interpreter::raise_return_pub(interp, &frame, value));
            }
        }
    }
    Ok(last)
}
