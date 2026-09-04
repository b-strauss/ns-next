//! The tree-walking interpreter's core: sends, method invocation, and the
//! `Signal` non-local-return/error channel. Expression-level evaluation is
//! in `eval.rs`.

pub mod eval;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::dispatch::{self, Lookup};
use crate::types::*;
use crate::Symbol;

pub enum Signal {
    Return(FrameRef, Value),
    Error(NewspeakError),
}

pub type EvalResult = Result<Value, Signal>;

pub struct Interpreter {
    next_identity_hash: u32,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Interpreter {
        Interpreter {
            next_identity_hash: 1,
        }
    }

    pub fn alloc_identity_hash(&mut self) -> u32 {
        let h = self.next_identity_hash;
        self.next_identity_hash += 1;
        h
    }

    /// An ordinary (explicit-receiver) send: `newspeak-spec.pdf` §5.5.
    pub fn send(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let class = self.class_of(&receiver);
        match dispatch::lookup_public(&class, selector) {
            Lookup::Method(method, defining_class) => self.invoke_method(
                receiver,
                method,
                defining_class,
                args,
                bindings.clone(),
                sender,
            ),
            Lookup::Native(kind) => {
                self.dispatch_native_or_dnu(kind, receiver, selector, args, bindings, sender)
            }
            Lookup::NotFound => {
                self.does_not_understand(receiver, selector, args, bindings, sender)
            }
        }
    }

    fn dispatch_native_or_dnu(
        &mut self,
        kind: BuiltinClass,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        match crate::builtins::dispatch_native(self, kind, receiver.clone(), selector, &args) {
            Some(result) => result.map_err(Signal::Error),
            None => self.does_not_understand(receiver, selector, args, bindings, sender),
        }
    }

    /// §5.5's `doesNotUnderstand:` fallback -- access-blind (`lookup_defined_for`),
    /// per the runtime spec's Dispatch section.
    fn does_not_understand(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let class = self.class_of(&receiver);
        let dnu_selector = Symbol::intern("doesNotUnderstand:");
        match dispatch::lookup_defined_for(&class, dnu_selector) {
            Lookup::Method(method, defining_class) => {
                // Milestone 1 passes a minimal message-mirror-shaped value:
                // an Array `{selector. args}` rather than a real MessageMirror
                // object (mirrors are a later milestone's Non-Goal). Callers
                // relying on a real `MessageMirror` API are out of scope.
                let selector_value = self.new_symbol(selector);
                let args_value = Value::Obj(self.new_array(args));
                let message_arg = Value::Obj(self.new_array(vec![selector_value, args_value]));
                self.invoke_method(
                    receiver,
                    method,
                    defining_class,
                    vec![message_arg],
                    bindings.clone(),
                    sender,
                )
            }
            _ => Err(Signal::Error(NewspeakError {
                selector,
                receiver,
                message: format!("doesNotUnderstand: {selector} (and no doesNotUnderstand: found -- Object's builtin default is missing; see Task 7)"),
            })),
        }
    }

    /// Creates a `Frame`, runs the method body, and unwinds a matching
    /// `Signal::Return` back into an ordinary value.
    pub fn invoke_method(
        &mut self,
        receiver: Value,
        method: Rc<ast::MethodDecl>,
        defining_class: ClassRef,
        args: Vec<Value>,
        bindings: Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let temp_count = method.body.temps.len();
        let frame = Rc::new(RefCell::new(Frame {
            receiver,
            args,
            temps: vec![Value::Nil; temp_count],
            pc: 0,
            sender,
            home: None,
            current_class: Some(defining_class),
            kind: FrameKind::Method(method.clone()),
            uncontinuable: false,
        }));
        let stmts = &method.body.stmts;
        match self.run_statements(stmts, &frame, &bindings) {
            Ok(value) => Ok(value),
            Err(Signal::Return(target, value)) if Rc::ptr_eq(&target, &frame) => Ok(value),
            Err(other) => Err(other),
        }
    }

    fn run_statements(
        &mut self,
        stmts: &[ast::Stmt],
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> EvalResult {
        let mut last = Value::Nil;
        for (i, stmt) in stmts.iter().enumerate() {
            frame.borrow_mut().pc = i;
            match stmt {
                ast::Stmt::Expr(e) => {
                    last = eval::eval_expr(self, e, frame, bindings)?;
                }
                ast::Stmt::Return(e) => {
                    let value = eval::eval_expr(self, e, frame, bindings)?;
                    return Err(self.raise_return(frame, value));
                }
            }
        }
        // No explicit return: methods return `self` (spec §3.5), closures
        // return the last expression's value (§5.1.8). `eval.rs`'s block
        // invocation handles the closure case directly and never calls
        // `run_statements` through this method-shaped path for a closure
        // body's implicit-return value; see Task 6 Step 2.
        match &frame.borrow().kind {
            FrameKind::Method(_) => Ok(frame.borrow().receiver.clone()),
            _ => Ok(last),
        }
    }

    fn raise_return(&self, frame: &FrameRef, value: Value) -> Signal {
        let target = match &frame.borrow().kind {
            FrameKind::Method(_) | FrameKind::TopLevel => frame.clone(),
            FrameKind::Closure(closure) => closure.home.clone(),
        };
        if target.borrow().uncontinuable {
            return Signal::Error(NewspeakError {
                selector: Symbol::intern("^"),
                receiver: value,
                message: "cannotReturn: the enclosing method activation has already returned"
                    .to_string(),
            });
        }
        target.borrow_mut().uncontinuable = true;
        Signal::Return(target, value)
    }

    pub(crate) fn raise_return_pub(&self, frame: &FrameRef, value: Value) -> Signal {
        self.raise_return(frame, value)
    }

    pub fn class_of(&self, value: &Value) -> ClassRef {
        match value {
            Value::Obj(obj) => obj.borrow().class.clone(),
            // Nil/Bool/SmallInt/Float classes are wired up by the bootstrap
            // (Task 11) into interpreter-held globals; Task 7-10 read them
            // via `Interpreter::builtin_class(kind)` (added in Task 7).
            _ => self.builtin_class_for_primitive(value),
        }
    }

    // ---- Temporary stubs (Task 6) ----
    //
    // `builtin_class_for_primitive`, `new_array`, and `new_symbol` are
    // wired up for real by Task 7's bootstrap, which needs the globals
    // table introduced there. These bodies exist only so the crate
    // compiles and this task's own tests (which never call `class_of` on
    // a primitive `Value`, nor go through `new_array`/`new_symbol`) can
    // run. Task 7 Step 1 deletes these stub bodies when the real ones
    // land.
    fn builtin_class_for_primitive(&self, _value: &Value) -> ClassRef {
        unimplemented!("wired up in Task 7's bootstrap")
    }

    pub fn new_array(&mut self, _elements: Vec<Value>) -> ObjRef {
        unimplemented!("wired up in Task 7's bootstrap")
    }

    pub fn new_symbol(&mut self, _s: Symbol) -> Value {
        unimplemented!("wired up in Task 7's bootstrap")
    }

    pub fn new_string(&mut self, _s: String) -> ObjRef {
        unimplemented!("wired up in Task 7's bootstrap")
    }

    pub fn new_character(&mut self, _c: char) -> ObjRef {
        unimplemented!("wired up in Task 7's bootstrap")
    }

    pub fn new_closure(&mut self, _closure: Rc<ClosureData>) -> ObjRef {
        unimplemented!("wired up in Task 7's bootstrap")
    }
}
