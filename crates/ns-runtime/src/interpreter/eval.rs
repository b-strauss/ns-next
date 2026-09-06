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
        // Gap 1 fix: the resolver (`resolve_ident`) records no `Binding`
        // at all for a bare identifier that names a Milestone 1 builtin
        // class (e.g. plain `OrderedCollection` in `OrderedCollection
        // new`, outside any superclass clause) -- mirroring
        // `resolve_superclass_clause`'s existing convention for the
        // analogous superclass-clause case. Resolve such a name against
        // the builtin table by name here, symmetrically, instead of
        // treating an absent binding as always an internal resolver
        // error.
        Some(Binding::SuperSend) | None => {
            if let Some(kind) = super::instantiate::builtin_kind_from_name(&id.name) {
                return Ok(Value::Obj(interp.builtin_class(kind)));
            }
            let receiver = frame.borrow().receiver.clone();
            Err(unresolved_identifier_error(&id.name, receiver))
        }
    }
}

/// The program-level error a bare identifier raises when it has no binding
/// and names no builtin class -- in Milestone 1 and 2 reachable only for a
/// bare `super`, which `resolver::resolve_ident` deliberately leaves
/// unbound and which the lexer deliberately does not reserve.
///
/// Shared with the bytecode engine (`bytecode::Literal::UnresolvedIdent`,
/// raised by `vm::make_value`) so the two engines cannot drift apart in
/// wording: the tree-walker is the differential oracle down to the byte.
pub(crate) fn unresolved_identifier_error(name: &str, receiver: Value) -> Signal {
    Signal::Error(NewspeakError {
        selector: Symbol::intern(name),
        receiver,
        message: format!("internal: unresolved identifier `{name}`"),
    })
}

/// The error a lexer-accepted integer literal that `parse_int_literal`
/// rejects raises when it is *evaluated*. Shared with the bytecode engine
/// (`bytecode::Literal::MalformedInt`) for the same reason.
pub(crate) fn malformed_int_error(message: String) -> Signal {
    Signal::Error(NewspeakError {
        selector: Symbol::intern("parse"),
        receiver: Value::Nil,
        message,
    })
}

pub(crate) fn read_local(frame: &FrameRef, index: u16) -> Value {
    let borrowed = frame.borrow();
    let i = index as usize;
    if i < borrowed.args.len() {
        borrowed.args[i].clone()
    } else {
        borrowed.temps[i - borrowed.args.len()].clone()
    }
}

pub(crate) fn write_local(frame: &FrameRef, index: u16, value: Value) {
    let mut borrowed = frame.borrow_mut();
    let i = index as usize;
    if i < borrowed.args.len() {
        borrowed.args[i] = value;
    } else {
        let temp_index = i - borrowed.args.len();
        borrowed.temps[temp_index] = value;
    }
}

pub(crate) fn frame_at_depth(frame: &FrameRef, depth: u16) -> FrameRef {
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

/// The `home` a closure created in `frame` gets: a method, top-level, or
/// initializer activation is its own home; a closure activation's home is
/// its own `home`. Shared by the tree-walker's `eval_block_literal` and
/// the VM's `MakeClosure`.
pub(crate) fn home_of(frame: &FrameRef) -> FrameRef {
    match &frame.borrow().kind {
        FrameKind::Method(_) | FrameKind::TopLevel | FrameKind::Initializer(_) => frame.clone(),
        FrameKind::Closure(closure) => closure.home.clone(),
    }
}

/// Writes `value` into instance field `index` on `instance`'s
/// `ObjectData::Instance` fields -- silently skipped when `instance` is not
/// an object, or is an object with some other `ObjectData` shape, or
/// `index` is out of range. Shared by `instantiate::run_own_slots` (the
/// tree-walker's per-slot-initializer write) and `vm::run_code`'s
/// `Instr::InitSlot` arm (the bytecode engine's equivalent), instead of the
/// same four-line downcast-bounds-check-assign shape living in both places.
pub(crate) fn store_instance_field(instance: &Value, index: usize, value: Value) {
    if let Value::Obj(obj) = instance {
        if let ObjectData::Instance(fields) = &mut obj.borrow_mut().data {
            if index < fields.len() {
                fields[index] = value;
            }
        }
    }
}

fn read_outer_local(frame: &FrameRef, depth: u16, index: u16) -> Value {
    read_local(&frame_at_depth(frame, depth), index)
}

fn write_outer_local(frame: &FrameRef, depth: u16, index: u16, value: Value) {
    write_local(&frame_at_depth(frame, depth), index, value);
}

/// Evaluates an `ast::Literal::Int`'s source text, which the lexer hands
/// over verbatim.
///
/// Two shapes reach here (`lexer.rs`'s `lex_number`, and `parser/expr.rs`'s
/// `parse_literal`, which classifies anything containing `r` as an `Int`):
/// a plain decimal literal (`42`, `-42`), and the Smalltalk/Newspeak radix
/// literal `<radix>r<digits>` (`16rFF`, `2r1010`, `16r-1F`, `-16rFF`) --
/// the spec's Source Format section accepts both. `text.parse::<i64>()`
/// alone rejected every radix literal with a misleading "out of i64 range".
///
/// Milestone 1 gap kept explicit: the lexer also accepts a *fractional*
/// radix literal (`16r1.8`), which the parser still classifies as an `Int`.
/// That needs radix-aware float construction; it is rejected here with a
/// message that says so rather than silently truncating.
///
/// Returns the `NewspeakError` message on failure (never panics -- this is a
/// Newspeak-program-input path).
pub(crate) fn parse_int_literal(text: &str) -> Result<i64, String> {
    let Some((radix_part, digits)) = text.split_once('r') else {
        return text
            .parse::<i64>()
            .map_err(|_| format!("integer literal out of i64 range: {text}"));
    };
    // A leading `-` on the radix itself (`-16rFF`) negates the whole value;
    // a leading `-` on the digits (`16r-1F`) is handled by `from_str_radix`.
    let (negated, radix_digits) = match radix_part.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, radix_part),
    };
    let radix: u32 = radix_digits
        .parse()
        .map_err(|_| format!("malformed radix in integer literal `{text}`"))?;
    if !(2..=36).contains(&radix) {
        return Err(format!(
            "radix {radix} out of range in integer literal `{text}`: must be 2..36"
        ));
    }
    if digits.contains('.') {
        return Err(format!(
            "fractional radix literals are not supported in Milestone 1: `{text}`"
        ));
    }
    let value = i64::from_str_radix(digits, radix).map_err(|_| {
        format!("malformed or out-of-i64-range radix integer literal `{text}` (radix {radix})")
    })?;
    if negated {
        value
            .checked_neg()
            .ok_or_else(|| format!("integer literal out of i64 range: {text}"))
    } else {
        Ok(value)
    }
}

fn eval_literal(interp: &mut Interpreter, lit: &ast::Literal) -> EvalResult {
    Ok(match lit {
        ast::Literal::Int { text, .. } => {
            let n = parse_int_literal(text).map_err(malformed_int_error)?;
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
/// `MethodDecl`s in the class's mixin, so `dispatch_lexical_send` must
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
pub(crate) fn dispatch_lexical_send(
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
    // *before* the method/native lookups below -- `build_mixin` never puts a
    // slot's implicit getter/setter into the mixin's method table, so
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
            match crate::builtins::dispatch_native(
                interp,
                kind,
                o.clone(),
                selector,
                &args,
                Some(frame),
            ) {
                Some(result) => result,
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
pub(crate) fn dispatch_super_send(
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
                Some(frame),
            ) {
                Some(result) => result,
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
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let home = home_of(frame);
    let closure = Rc::new(ClosureData {
        params: block
            .params
            .iter()
            .map(|p| Symbol::intern(&p.name))
            .collect(),
        body: ClosureBody::Ast(Rc::new(block.body.clone())),
        captured_frame: frame.clone(),
        home,
        bindings: bindings.clone(),
    });
    Ok(Value::Obj(interp.new_closure(closure)))
}

/// Invokes a closure with `args`. Called by `builtins::block` (Task 10) for
/// `value`, `value:`, `value:value:`, etc. Public because block-invocation
/// selectors are Milestone 1 *native* methods on `BuiltinClass::Block`, not
/// user-overridable Newspeak methods.
///
/// `sender` is the activation performing the invocation, threaded here from
/// the send that reached the block-invoking native primitive (finding I6).
/// It becomes the new closure activation's own `Frame.sender`, exactly as
/// `Interpreter::invoke_method` does for a method activation, so the
/// liveness spec's reified call chain (`Frame.sender`) stays walkable across
/// a block invocation boundary instead of being cut by an unconditional
/// `None`. `None` is still legitimate for an invocation with no enclosing
/// activation at all (a direct entry-point call).
pub fn invoke_closure(
    interp: &mut Interpreter,
    closure: &Rc<ClosureData>,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
    sender: Option<FrameRef>,
) -> EvalResult {
    // Task 10 fix: reject a wrong argument count as a NewspeakError up
    // front, before it can reach `read_local`. Without this guard, a call
    // with too few args can panic outright -- `read_local`'s
    // `i < borrowed.args.len()` check falls through to
    // `temps[i - borrowed.args.len()]`, which is out of bounds whenever the
    // missing args would have covered a param index that has no
    // correspondingly-allocated temp slot -- and a call with too many args
    // would silently misaddress temps instead of erroring. Never panic on
    // this Newspeak-program-input path.
    if args.len() != closure.params.len() {
        let receiver = closure.captured_frame.borrow().receiver.clone();
        return Err(Signal::Error(NewspeakError {
            selector: Symbol::intern("value"),
            receiver,
            message: format!(
                "wrong number of block arguments: block takes {}, got {}",
                closure.params.len(),
                args.len()
            ),
        }));
    }
    let receiver = closure.captured_frame.borrow().receiver.clone();
    let current_class = closure.captured_frame.borrow().current_class.clone();
    // Hook 2 (spec: "`vm.rs` and the engine hook"): the frame is built the
    // same way for both engines; only the body's evaluation differs, and
    // which engine that is was decided when the closure was created.
    let temp_count = match &closure.body {
        ClosureBody::Ast(body) => body.temps.len(),
        ClosureBody::Compiled(code) => (code.num_registers - code.num_args) as usize,
    };
    let frame = Rc::new(RefCell::new(Frame {
        receiver,
        args,
        temps: vec![Value::Nil; temp_count],
        pc: 0,
        sender,
        home: Some(closure.home.clone()),
        current_class,
        kind: FrameKind::Closure(closure.clone()),
        uncontinuable: false,
    }));
    match &closure.body {
        ClosureBody::Compiled(code) => crate::vm::run_code(interp, &frame, code, bindings),
        ClosureBody::Ast(body) => {
            let mut last = Value::Nil;
            for (i, stmt) in body.stmts.iter().enumerate() {
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
    }
}

#[cfg(test)]
mod tests {
    use super::parse_int_literal;

    #[test]
    fn plain_decimal_literals_still_parse() {
        assert_eq!(parse_int_literal("42"), Ok(42));
        assert_eq!(parse_int_literal("-42"), Ok(-42));
        assert_eq!(parse_int_literal("0"), Ok(0));
    }

    #[test]
    fn radix_literals_parse_in_every_accepted_shape() {
        assert_eq!(parse_int_literal("16rFF"), Ok(255));
        assert_eq!(parse_int_literal("16r1F"), Ok(31));
        assert_eq!(parse_int_literal("2r1010"), Ok(10));
        assert_eq!(parse_int_literal("36rZ"), Ok(35));
        // `-` on the digits, and `-` on the radix, both mean "negative".
        assert_eq!(parse_int_literal("16r-1F"), Ok(-31));
        assert_eq!(parse_int_literal("-16rFF"), Ok(-255));
    }

    #[test]
    fn a_decimal_literal_out_of_range_says_so() {
        let err = parse_int_literal("99999999999999999999").expect_err("out of range");
        assert!(err.contains("out of i64 range"), "got: {err}");
    }

    #[test]
    fn an_out_of_range_radix_is_rejected_honestly() {
        let err = parse_int_literal("1rZ").expect_err("radix 1");
        assert!(err.contains("must be 2..36"), "got: {err}");
        let err = parse_int_literal("40rZ").expect_err("radix 40");
        assert!(err.contains("must be 2..36"), "got: {err}");
    }

    #[test]
    fn a_digit_outside_the_radix_is_rejected() {
        let err = parse_int_literal("2r1012").expect_err("digit 2 is not base-2");
        assert!(err.contains("malformed"), "got: {err}");
    }

    #[test]
    fn a_fractional_radix_literal_says_it_is_unsupported() {
        let err = parse_int_literal("16r1.8").expect_err("fractional radix");
        assert!(
            err.contains("fractional radix literals are not supported"),
            "got: {err}"
        );
    }
}
