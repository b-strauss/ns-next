//! The tree-walking interpreter's core: sends, method invocation, and the
//! `Signal` non-local-return/error channel. Expression-level evaluation is
//! in `eval.rs`. The bootstrap and class-instantiation machinery are in
//! `instantiate.rs`.

pub mod eval;
pub mod instantiate;
pub(crate) mod nested;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::bytecode::{method_key, CompiledModule};
use crate::dispatch::{self, Lookup};
use crate::types::*;
use crate::Symbol;

pub enum Signal {
    Return(FrameRef, Value),
    Error(NewspeakError),
}

pub type EvalResult = Result<Value, Signal>;

/// Which engine evaluates method bodies, block bodies, and instance
/// initializers. Everything else -- dispatch, `doesNotUnderstand:`, native
/// dispatch, instantiation, frame construction -- is shared. See the M2
/// spec's "`vm.rs` and the engine hook".
pub enum Engine {
    TreeWalk,
    Bytecode(CompiledModule),
}

/// Why `Interpreter::set_engine` refused.
///
/// One variant so far, and the reason it is an enum rather than a `bool`
/// is that a caller should be able to tell the cases apart if a second one
/// ever appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineUnavailable {
    /// `Engine::TreeWalk` on a world decoded from an image.
    ///
    /// The tree-walker evaluates declarations: a method's `ast::CodeBody`,
    /// a slot clause's initializer expression, a class's `initExprs`, a
    /// superclass clause's receiver. An image carries none of those -- it
    /// carries compiled code and a mixin table, and every `Mixin`,
    /// `MixinMethod` and `MixinSlot` `image::mixins::decode` rebuilds has
    /// `decl: None`. Running the tree-walker over such a world does not
    /// fail uniformly: `invoke_method` and `nested::run_clause_body` panic
    /// on the missing declaration, while `instantiate::run_own_slots`
    /// filter-maps the declarationless slots away and
    /// `run_own_init_exprs` iterates an empty vector -- silently producing
    /// a half-initialised object, which is the worse of the two outcomes.
    ///
    /// Refusing the switch is what makes the behaviour consistent, and it
    /// does so for every such path at once, including paths added later:
    /// the alternative -- teaching each individual tree-walk path to fail
    /// loudly -- has to be re-done for every path anyone writes next, and
    /// forgetting once puts the silent half-initialisation back.
    TreeWalkAfterLoad,
}

impl std::fmt::Display for EngineUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineUnavailable::TreeWalkAfterLoad => write!(
                f,
                "the tree-walking engine cannot run a world decoded from an image: \
                 an image carries compiled code, not the declarations the tree-walker \
                 evaluates"
            ),
        }
    }
}

pub struct Interpreter {
    next_identity_hash: u32,
    builtin_classes: Option<HashMap<BuiltinClass, ClassRef>>,
    platform_singleton: Option<Value>,
    engine: Engine,
    /// (mixin identity, superclass application identity) -> behaviour.
    /// Keyed on the superclass's *application*, never on a superclass class
    /// object: a nested superclass has one class object per enclosing
    /// instance, so a class-object key would never hit and every instance
    /// would get its own behaviour.
    applications: HashMap<(usize, usize), Rc<crate::mixin::MixinApplication>>,
    /// The (receiver address, declaration name) pairs whose nested class
    /// object is being created right now -- see
    /// `nested::cyclic_definition_error`. A stack, not a set: it is empty
    /// on every path that is not inside a superclass clause, and a clause
    /// nests only as deep as a program actually chains class creations, so
    /// a linear scan beats hashing and allocates nothing in the common
    /// case.
    nested_in_progress: Vec<(usize, Symbol)>,
    /// Set by `image::load`: this world's declarations came off the wire,
    /// so they are mixins without `decl`s. See
    /// `EngineUnavailable::TreeWalkAfterLoad`.
    decoded_from_image: bool,
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
            builtin_classes: None,
            platform_singleton: None,
            engine: Engine::TreeWalk,
            applications: HashMap::new(),
            nested_in_progress: Vec::new(),
            decoded_from_image: false,
        }
    }

    pub fn bootstrap() -> Interpreter {
        let mut interp = Interpreter {
            next_identity_hash: 1,
            builtin_classes: None,
            platform_singleton: None,
            engine: Engine::TreeWalk,
            applications: HashMap::new(),
            nested_in_progress: Vec::new(),
            decoded_from_image: false,
        };
        interp.builtin_classes = Some(build_builtin_classes(&mut interp));
        interp
    }

    /// Switches engines. Call after `load_module` and `compile_module`; the
    /// default is `TreeWalk`, so every Milestone 1 caller keeps its
    /// behavior.
    ///
    /// Refused in exactly one case -- `Engine::TreeWalk` on a world
    /// `image::load` built, for the reasons on
    /// `EngineUnavailable::TreeWalkAfterLoad`. `Engine::Bytecode` is never
    /// refused, so every caller that installs the VM (which is all of them
    /// but one test) can treat the result as infallible.
    pub fn set_engine(&mut self, engine: Engine) -> Result<(), EngineUnavailable> {
        if self.decoded_from_image && matches!(engine, Engine::TreeWalk) {
            return Err(EngineUnavailable::TreeWalkAfterLoad);
        }
        self.engine = engine;
        Ok(())
    }

    /// Marks this world as decoded from an image -- `image::load`'s last
    /// step, and the only writer. Sealing the interpreter rather than each
    /// tree-walk path is the whole of the policy; see
    /// `EngineUnavailable::TreeWalkAfterLoad`.
    pub(crate) fn mark_decoded_from_image(&mut self) {
        self.decoded_from_image = true;
    }

    pub fn compiled_module(&self) -> Option<&CompiledModule> {
        match &self.engine {
            Engine::Bytecode(module) => Some(module),
            Engine::TreeWalk => None,
        }
    }

    pub fn builtin_class(&self, kind: BuiltinClass) -> ClassRef {
        self.builtin_classes
            .as_ref()
            .expect("bootstrap() must run before builtin_class()")
            .get(&kind)
            .expect("every BuiltinClass variant is registered by build_builtin_classes")
            .clone()
    }

    /// Saturating, not wrapping and not panicking: identity hashes are not
    /// required to be unique (`dispatch.rs`'s placeholder class objects
    /// already share hash 0, and `hash` is the only thing that reads one --
    /// object identity is `Rc` identity, never the hash), so a world that
    /// exhausts the counter is better served by handing out a shared
    /// `u32::MAX` than by aborting the process. Reachable in *one*
    /// allocation once `image::load` may set the counter from a file:
    /// `advance_identity_hash_past` puts it wherever the image's maximum
    /// says, so the `+= 1` here is on an image-controlled path.
    pub fn alloc_identity_hash(&mut self) -> u32 {
        let h = self.next_identity_hash;
        self.next_identity_hash = self.next_identity_hash.saturating_add(1);
        h
    }

    /// After `image::load`: never hand out an identity hash a loaded object
    /// already carries (spec: "Identity hashes").
    ///
    /// `max < u32::MAX` is the caller's guarantee, and an enforced one:
    /// `image::decode` refuses an image carrying `u32::MAX` outright
    /// (`DecodeError::IdentityHashExhausted`), because there is no counter
    /// value past it. That refusal is what keeps `max + 1` here in range;
    /// `alloc_identity_hash`'s own saturation is what keeps the *next*
    /// allocation in range when the image's maximum is one below the
    /// ceiling.
    pub(crate) fn advance_identity_hash_past(&mut self, max: u32) {
        if self.next_identity_hash <= max {
            self.next_identity_hash = max + 1;
        }
    }

    /// An ordinary (explicit-receiver) send: `newspeak-spec.pdf` §5.5.
    ///
    /// Checked first, before any lookup: `ClassName new` (or a class's own
    /// non-default factory selector) never has a real method to find --
    /// every class object's own `.class` is a shared placeholder with an
    /// empty method table (see `build_builtin_classes` below), so an
    /// ordinary `lookup_public` search would just fall through to
    /// `doesNotUnderstand:` every time. `try_factory_send`
    /// (`instantiate.rs`) recognizes the shape and routes to
    /// `instantiate` directly; see its own doc comment for why this can
    /// never double-fire against the superclass clause's own deferred
    /// factory message (`run_superclass_initializer` never calls `send`).
    pub fn send(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        if let Some(result) = self.try_factory_send(&receiver, selector, &args, bindings) {
            return result;
        }
        // §6.3: a class object's class-side methods are reached through its
        // metaclass, consulted before the class object's own (ordinary,
        // `Object`-native) behaviour. `metaclass_of` returns `None` for a
        // class whose mixin declares no class side, and `Lookup::NotFound`
        // here (the metaclass exists but doesn't answer `selector` either)
        // both fall through to the code below unchanged -- there is no
        // second `doesNotUnderstand:` path, only a second class to try
        // `lookup_public` against first.
        if let Value::Obj(obj) = &receiver {
            let is_class = matches!(obj.borrow().data, ObjectData::Class(_));
            if is_class {
                if let Some(metaclass) = self.metaclass_of(obj) {
                    match dispatch::lookup_public(&metaclass, selector) {
                        Lookup::Method(method, defining_class) => {
                            return self.invoke_method(
                                receiver,
                                method,
                                defining_class,
                                args,
                                bindings.clone(),
                                sender,
                            );
                        }
                        Lookup::NestedClass(mixin, declaring_class) => {
                            return nested::nested_class(
                                self,
                                &receiver,
                                &mixin,
                                &declaring_class,
                                bindings,
                            );
                        }
                        Lookup::Native(kind) => {
                            return self.dispatch_native_or_dnu(
                                kind, receiver, selector, args, bindings, sender,
                            );
                        }
                        Lookup::NotFound => {}
                    }
                }
            }
        }
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
            // §6: the implicit accessor a nested declaration defines is
            // reached exactly like any other member, and it ignores `args`
            // -- the accessor is unary, so a send that supplied arguments
            // did not name it (`walk` matched on the bare selector).
            Lookup::NestedClass(mixin, declaring_class) => {
                nested::nested_class(self, &receiver, &mixin, &declaring_class, bindings)
            }
            Lookup::Native(kind) => {
                self.dispatch_native_or_dnu(kind, receiver, selector, args, bindings, sender)
            }
            Lookup::NotFound => {
                self.does_not_understand(receiver, selector, args, bindings, sender)
            }
        }
    }

    /// Native dispatch that still inherits `Object`'s own native methods.
    ///
    /// `dispatch::walk` deliberately treats a class tagged
    /// `builtin: Some(kind)` as a *dispatch leaf*: it returns
    /// `Lookup::Native(kind)` the moment it reaches one, without continuing
    /// up that class's own superclass chain (Task 5's design note -- that
    /// early return is what keeps `lookupPublic`'s `hard_stop_on_mismatch`
    /// semantics uniform, and restructuring it is out of scope). Every
    /// non-`Object` builtin's superclass is `Object` (see
    /// `build_builtin_classes` below), and none of `numbers`/`text`/
    /// `collections`/`block`/`boolean` implements `class`/`==`/`=`/`hash`/
    /// `printString`/`doesNotUnderstand:` itself -- so without this helper
    /// those `Object` methods would be unreachable on *every* non-`Object`
    /// receiver, and `Object`'s spec-required `doesNotUnderstand:` would
    /// never fire for a builtin at all.
    ///
    /// Restores exactly the one missing inheritance step, at the consuming
    /// call site rather than inside `walk`: try `kind`'s own native table
    /// first (so a builtin that *does* implement a selector still wins),
    /// then `Object`'s. Returns `None` only when neither implements
    /// `selector`.
    fn dispatch_native_inheriting_object(
        &mut self,
        kind: BuiltinClass,
        receiver: &Value,
        selector: Symbol,
        args: &[Value],
        sender: Option<&FrameRef>,
    ) -> Option<EvalResult> {
        if let Some(result) =
            crate::builtins::dispatch_native(self, kind, receiver.clone(), selector, args, sender)
        {
            return Some(result);
        }
        if kind == BuiltinClass::Object {
            return None;
        }
        crate::builtins::dispatch_native(
            self,
            BuiltinClass::Object,
            receiver.clone(),
            selector,
            args,
            sender,
        )
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
        match self.dispatch_native_inheriting_object(
            kind,
            &receiver,
            selector,
            &args,
            sender.as_ref(),
        ) {
            Some(result) => result,
            None => self.does_not_understand(receiver, selector, args, bindings, sender),
        }
    }

    /// §5.5's `doesNotUnderstand:` fallback -- access-blind (`lookup_defined_for`),
    /// per the runtime spec's Dispatch section.
    ///
    /// Task 6 left this handling only the `Lookup::Method` case (a
    /// user-overridden `doesNotUnderstand:`); Task 7 adds `Object`'s own
    /// *native* `doesNotUnderstand:` (`builtins::object_class::dispatch`),
    /// reached here via `Lookup::Native` exactly like `dispatch_native_or_dnu`
    /// reaches any other native method -- without this arm, `Object`'s
    /// builtin `doesNotUnderstand:` (which every class inherits, since
    /// `lookup_defined_for` finds `ClassData.builtin` on any class with no
    /// method override) would never actually run.
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
        // Milestone 1 passes a minimal message-mirror-shaped value: an Array
        // `{selector. args}` rather than a real MessageMirror object
        // (mirrors are a later milestone's Non-Goal). Callers relying on a
        // real `MessageMirror` API are out of scope.
        match dispatch::lookup_defined_for(&class, dnu_selector) {
            Lookup::Method(method, defining_class) => {
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
            Lookup::Native(kind) => {
                let selector_value = self.new_symbol(selector);
                let args_value = Value::Obj(self.new_array(args));
                let message_arg = Value::Obj(self.new_array(vec![selector_value, args_value]));
                // Same two-step fallback as `dispatch_native_or_dnu`: `kind`
                // is whatever builtin class `walk` stopped at, and no
                // builtin implements `doesNotUnderstand:` itself, so
                // without the `Object` step this would always miss and fall
                // into the internal-error arm below instead of running
                // `builtins::object_class`'s real, spec-shaped
                // `doesNotUnderstand:`.
                match self.dispatch_native_inheriting_object(
                    kind,
                    &receiver,
                    dnu_selector,
                    &[message_arg],
                    sender.as_ref(),
                ) {
                    Some(result) => result,
                    None => Err(Signal::Error(NewspeakError {
                        selector,
                        receiver,
                        message: format!("doesNotUnderstand: {selector} (native class {kind:?} has no doesNotUnderstand: implementation)"),
                    })),
                }
            }
            // A nested declaration can never be the `doesNotUnderstand:`
            // handler: `walk` matches a declaration by name, and a class
            // name is never a keyword selector. Folded into `NotFound`
            // rather than given an arm of its own so the unreachable case
            // still reports the ordinary "nothing handles this" error
            // instead of a second, differently-worded one.
            Lookup::NotFound | Lookup::NestedClass(..) => Err(Signal::Error(NewspeakError {
                selector,
                receiver,
                message: format!("doesNotUnderstand: {selector} (and no doesNotUnderstand: found anywhere in the class chain)"),
            })),
        }
    }

    /// Creates a `Frame`, runs the method body, and unwinds a matching
    /// `Signal::Return` back into an ordinary value.
    ///
    /// The frame is marked `uncontinuable` on *every* path out, not only
    /// when the body executed an explicit `^` (which is all `raise_return`
    /// itself ever marked): a method activation is dead the moment
    /// `invoke_method` hands control back to its own caller, whether it
    /// returned via `^`, fell off the end, or unwound something else. A
    /// block captured during the activation and invoked afterwards then
    /// sees the correct `cannotReturn` `NewspeakError` from `raise_return`,
    /// instead of raising a `Signal::Return` at a frame whose Rust-level
    /// `invoke_method` call has already returned -- which had no live
    /// catcher left and surfaced as the interpreter-invariant-shaped
    /// "internal: non-local return escaped the entry send".
    pub fn invoke_method(
        &mut self,
        receiver: Value,
        method: Rc<crate::mixin::MixinMethod>,
        defining_class: ClassRef,
        args: Vec<Value>,
        bindings: Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        // A frame's locals are addressed as one run of registers -- the
        // arguments first, the declared temporaries after them -- and
        // `eval::read_local`/`write_local` find the boundary by asking the
        // frame how many arguments it was *given*, not how many the method
        // declares. A wrong-arity activation therefore does not fail; it
        // silently shifts every temporary by the difference and reads
        // arguments back where a temporary should be, under both engines
        // identically, which is why the differential harness cannot see it.
        // Silent wrong output with exit status 0 is the failure mode this
        // repository's invariants single out, so the mismatch is refused
        // here, in the orchestration both engines share, as an ordinary
        // Newspeak error -- never a panic, since the caller may be a host.
        //
        // No Newspeak-level send can reach this: the parser builds a
        // message whose argument count matches its selector by
        // construction. The reachable callers are host-initiated sends --
        // `ns-next run --send`, `snapshot --send`, `load --send`, and any
        // future embedder driving `Interpreter::send` directly.
        if args.len() != method.num_args {
            return Err(Signal::Error(NewspeakError {
                selector: method.selector,
                receiver,
                message: format!(
                    "wrong number of arguments: `{}` takes {}, got {}",
                    method.selector,
                    method.num_args,
                    args.len()
                ),
            }));
        }
        // Hook 1 (spec: "`vm.rs` and the engine hook"): the frame, the
        // `uncontinuable` marking, and the `Signal::Return` catch are
        // shared; only the body's evaluation differs.
        let compiled = match &self.engine {
            Engine::TreeWalk => None,
            Engine::Bytecode(module) => Some(
                module
                    .methods
                    .get(&method_key(&method))
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!("internal: dispatched method has no compiled form (compile_module coverage gap)")
                    }),
            ),
        };
        // The tree-walker sizes the frame from the declaration's own named
        // temporaries, and only ever runs a mixin built from source, so
        // `decl` is always there when it is the one asking.
        let temp_count = match &compiled {
            Some(code) => (code.num_registers - code.num_args) as usize,
            None => method.decl.as_ref().map_or(0, |d| d.body.temps.len()),
        };
        let frame = Rc::new(RefCell::new(Frame {
            receiver,
            args,
            temps: vec![Value::Nil; temp_count],
            pc: 0,
            // Fixed by the same `compiled` that decides which body runs
            // below, so the frame can never claim a unit its own body does
            // not count in.
            pc_kind: match &compiled {
                Some(_) => PcKind::Bytecode,
                None => PcKind::TreeWalk,
            },
            sender,
            home: None,
            current_class: Some(defining_class),
            kind: FrameKind::Method(method.clone()),
            uncontinuable: false,
            slot_base: 0, // a method body never contains InitSlot
        }));
        let result = match (&compiled, &method.decl) {
            (Some(code), _) => crate::vm::run_code(self, &frame, code, &bindings),
            (None, Some(decl)) => self.run_statements(&decl.body.stmts, &frame, &bindings),
            // Unreachable rather than merely unlikely: a mixin without
            // declarations comes from an image, and `image::load` installs
            // `Engine::Bytecode`, so such a method always arrives compiled.
            (None, None) => panic!(
                "internal: the tree-walking engine cannot run a method of a mixin \
                 decoded from an image"
            ),
        };
        frame.borrow_mut().uncontinuable = true;
        match result {
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
            FrameKind::Method(_)
            | FrameKind::TopLevel
            | FrameKind::Initializer(_)
            | FrameKind::SuperclassClause(_)
            | FrameKind::SuperclassMessage(_) => frame.clone(),
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

    /// The metaclass carrying `class`'s class side (§6.3), created on first
    /// use and cached on `class` itself (`ClassData::metaclass`) -- not in a
    /// side table, and not in an instance slot the way a nested class's
    /// cache is: a class object's payload is `ObjectData::Class`, never
    /// `Instance`, so it has no slot vector for `nested_slot` to index into.
    /// Caching on the class object directly also gives every send against
    /// the same class object the same metaclass identity, which a later
    /// milestone's inline caches need.
    ///
    /// Returns `None` only when neither `class`'s own mixin nor any
    /// ancestor's declares a class side -- a class side is inherited down
    /// the ordinary subclass chain exactly like any other mixin member,
    /// single inheritance and nothing more. What stays out of scope is
    /// §3.2's *reification* of that fact: no `Class` superclass object, and
    /// no per-instance metaclass uniqueness (every class here still shares
    /// one metaclass, the way every application of a mixin shares one
    /// `MixinApplication`). Whether a class side is inherited at all is a
    /// different, much narrower question than how far the reified model
    /// goes, and the answer to it is the same answer §5's ordinary lookup
    /// already gives for a method.
    ///
    /// When `class`'s own mixin declares no class side but an ancestor's
    /// does, an empty synthetic mixin stands in for `class`'s own level --
    /// the same placeholder-mixin technique `builtin_mixin` below already
    /// uses for a builtin with no user-declared behaviour of its own. It
    /// contributes no methods, only relays the superclass's metaclass
    /// application onward, so `class`'s metaclass answers exactly what the
    /// nearest class-side-declaring ancestor's metaclass would.
    ///
    /// Either way, the metaclass's superclass is the superclass's
    /// metaclass, recursively, and the metaclass application chain stays
    /// linked to the metaclass class-object chain exactly as
    /// `dispatch::next_level` requires for the ordinary one.
    /// Iterative, over an explicit stack, for the same reason
    /// `dispatch::walk` is: the superclass chain's length is the *image's*
    /// to choose (`image::decode` builds one link per class record, at
    /// under twenty bytes apiece), and every send to a class object enters
    /// here. Recursing on it would overflow the stack on a chain a ten-
    /// megabyte image can describe -- an abort, not a diagnosable failure.
    ///
    /// The chain is finite: `image::decode::rebuild_applications` refuses a
    /// decoded graph whose superclass links form a cycle, and no other
    /// construction site can build one (a superclass is fixed before the
    /// class object that inherits it exists).
    pub fn metaclass_of(&mut self, class: &ClassRef) -> Option<ClassRef> {
        // Walk out to the first level that already answers -- a cached
        // metaclass, a non-class object, or the end of the chain -- pushing
        // each level that still has to be built. `chain` is nearest-first,
        // so the build below runs it in reverse.
        let mut chain: Vec<ClassRef> = Vec::new();
        let mut super_metaclass: Option<ClassRef> = None;
        let mut level = class.clone();
        loop {
            let (cached, superclass) = {
                let borrowed = level.borrow();
                match &borrowed.data {
                    ObjectData::Class(data) => (data.metaclass.clone(), data.superclass.clone()),
                    // Not a class object: it has no metaclass, and nothing
                    // below it can inherit one through this level.
                    _ => break,
                }
            };
            if cached.is_some() {
                super_metaclass = cached;
                break;
            }
            chain.push(level);
            let Some(superclass) = superclass else { break };
            level = superclass;
        }
        for level in chain.iter().rev() {
            let (class_side, own_mixin_name, enclosing, declaring_class) = {
                let borrowed = level.borrow();
                match &borrowed.data {
                    ObjectData::Class(data) => (
                        data.application.mixin.class_side.clone(),
                        data.application.mixin.name,
                        data.enclosing.clone(),
                        data.declaring_class.clone(),
                    ),
                    _ => unreachable!("only class objects are pushed onto the chain"),
                }
            };
            let class_side_mixin = match class_side {
                Some(mixin) => mixin,
                // No class side of its own to apply -- only worth building a
                // metaclass at all if an ancestor's class side needs relaying
                // through it. `super_metaclass.is_none()` here means neither
                // this level nor any ancestor declares a class side, which is
                // the ordinary "costs nothing" case: this level gets no
                // metaclass, nothing is cached for it, and the next level in
                // still gets its own chance to declare one.
                None => {
                    if super_metaclass.is_none() {
                        continue;
                    }
                    empty_class_side_mixin(own_mixin_name)
                }
            };
            let super_application = super_metaclass
                .as_ref()
                .and_then(dispatch::class_application);
            let application = self.apply_mixin(&class_side_mixin, super_application.as_ref());
            // §6.3.1: a class side is written inside the same class
            // declaration its instance side is, and `resolver::resolve_class`
            // resolves both against the same lexical chain -- so a class-side
            // method's `outer` must reach exactly the objects an
            // instance-side method's does. The metaclass therefore borrows
            // the class's own place in the declaration chain rather than
            // starting a new, empty one.
            let metaclass =
                self.new_class_object(application, enclosing, declaring_class, super_metaclass);
            match &mut level.borrow_mut().data {
                ObjectData::Class(data) => data.metaclass = Some(metaclass.clone()),
                _ => unreachable!("checked above"),
            }
            super_metaclass = Some(metaclass);
        }
        // The last level built is `chain[0]`, which is `class` itself; an
        // empty chain means the walk stopped on the first step, and
        // `super_metaclass` is then either `class`'s own cached metaclass or
        // `None` because it is not a class object at all.
        super_metaclass
    }

    fn builtin_class_for_primitive(&self, value: &Value) -> ClassRef {
        let kind = match value {
            Value::Nil => BuiltinClass::Nil,
            Value::Bool(true) => BuiltinClass::True,
            Value::Bool(false) => BuiltinClass::False,
            Value::SmallInt(_) => BuiltinClass::Integer,
            Value::Float(_) => BuiltinClass::Float,
            Value::Obj(_) => unreachable!("Value::Obj carries its own .class"),
        };
        self.builtin_class(kind)
    }

    fn new_instance_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        let identity_hash = self.alloc_identity_hash();
        Rc::new(RefCell::new(Object {
            class,
            identity_hash,
            data,
        }))
    }

    pub fn new_array(&mut self, elements: Vec<Value>) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Array);
        self.new_instance_object(class, ObjectData::Array(elements))
    }

    pub fn new_string(&mut self, s: String) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Str);
        self.new_instance_object(class, ObjectData::Str(s))
    }

    pub fn new_symbol(&mut self, s: Symbol) -> Value {
        let class = self.builtin_class(BuiltinClass::Sym);
        Value::Obj(self.new_instance_object(class, ObjectData::Sym(s)))
    }

    pub fn new_character(&mut self, c: char) -> ObjRef {
        // Milestone 1 has no dedicated Character payload variant on
        // ObjectData -- represent a character as a one-`char` Rust String
        // wrapped the same way String literals are (Str), consistent with
        // how the language spec itself treats characters as "a special
        // case of string literals" going forward (`newspeak-spec.pdf`
        // §2.2.3). `builtins::text` (Task 9) tags the *class* as
        // `BuiltinClass::Character` so dispatch still lands on the right
        // native methods; only the payload representation is shared.
        let class = self.builtin_class(BuiltinClass::Character);
        self.new_instance_object(class, ObjectData::Str(c.to_string()))
    }

    pub fn new_closure(&mut self, closure: Rc<ClosureData>) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Block);
        self.new_instance_object(class, ObjectData::Closure((*closure).clone()))
    }

    /// The `platform` object (runtime spec's Core Library section):
    /// memoized on first call, so every reference within one interpreter
    /// sees the same object identity.
    pub fn platform(&mut self) -> Value {
        if let Some(p) = &self.platform_singleton {
            return p.clone();
        }
        let class = self.builtin_class(BuiltinClass::Platform);
        let obj = self.new_instance_object(class, ObjectData::Instance(vec![]));
        let value = Value::Obj(obj);
        self.platform_singleton = Some(value.clone());
        value
    }

    /// A `Transcript`-style stdout stream object -- see `crate::platform`'s
    /// module doc for how its payload shape (a one-field, always-`Nil`
    /// `Instance`) tells it apart from the `platform` singleton itself
    /// (a zero-field `Instance`). `pub(crate)`, not `pub`: only
    /// `crate::platform::dispatch` (`platform transcript`) ever needs to
    /// mint one.
    pub(crate) fn new_stream_object(&mut self) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Platform);
        self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil]))
    }

    /// The `platform collections` namespace object -- see `crate::platform`'s
    /// module doc for how its payload shape (a two-field, always-`Nil`
    /// `Instance`) tells it apart from the `platform` singleton and the
    /// transcript stream. Like `new_stream_object`, it is allocated fresh
    /// per send: the namespace is stateless, so identity across sends
    /// carries no meaning that any caller could observe.
    pub(crate) fn new_collections_namespace(&mut self) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Platform);
        self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil, Value::Nil]))
    }

    /// A class object around an already-computed application. The one place
    /// the three-way invariant is established: a class object's `superclass`
    /// is linked exactly when its application's `superclass` is (callers
    /// derive `application` from `superclass` through `apply_mixin`), which
    /// is what `dispatch::next_level` relies on when it steps both chains
    /// together.
    ///
    /// `class` is the shared `Object` placeholder: a class object's own
    /// `.class` field never points at its metaclass. `metaclass_of` reaches
    /// a class side through a separate cache (`ClassData::metaclass`), not
    /// through this field -- §3.2's "every class an instance of a unique
    /// metaclass" stays unbuilt, by design (see `metaclass_of`'s doc
    /// comment).
    pub(crate) fn new_class_object(
        &mut self,
        application: Rc<crate::mixin::MixinApplication>,
        enclosing: Option<ObjRef>,
        declaring_class: Option<ClassRef>,
        superclass: Option<ClassRef>,
    ) -> ClassRef {
        let class = self.builtin_class(BuiltinClass::Object);
        let identity_hash = self.alloc_identity_hash();
        Rc::new(RefCell::new(Object {
            class,
            identity_hash,
            data: ObjectData::Class(ClassData {
                application,
                enclosing,
                declaring_class,
                superclass,
                builtin: None,
                metaclass: None,
            }),
        }))
    }

    pub fn apply_mixin(
        &mut self,
        mixin: &Rc<crate::mixin::Mixin>,
        superclass: Option<&Rc<crate::mixin::MixinApplication>>,
    ) -> Rc<crate::mixin::MixinApplication> {
        let key = (
            crate::mixin::mixin_key(mixin),
            superclass.map_or(0, crate::mixin::application_key),
        );
        if let Some(existing) = self.applications.get(&key) {
            return existing.clone();
        }
        let inherited = superclass.map_or(0, |s| s.num_slots);
        let mut slot_index = superclass.map_or_else(HashMap::new, |s| s.slot_index.clone());
        for (i, slot) in mixin.slots.iter().enumerate() {
            slot_index.insert(slot.name, inherited + i);
        }
        // The nested-class cache slots. They are part of the instance layout
        // because the spec caches a nested class in the enclosing *object*;
        // a side table keyed by object would cost a hash lookup on a path
        // that is otherwise a slot read.
        let mut nested_slot = superclass.map_or_else(HashMap::new, |s| s.nested_slot.clone());
        let mut next = inherited + mixin.slots.len();
        for nested in &mixin.nested {
            nested_slot.insert(nested.name, next);
            next += 1;
        }
        let application = Rc::new(crate::mixin::MixinApplication {
            mixin: mixin.clone(),
            superclass: superclass.cloned(),
            num_slots: next,
            slot_index,
            nested_slot,
            own_slot_base: inherited,
        });
        self.applications.insert(key, application.clone());
        application
    }
}

/// Milestone 1 (no metaclasses -- see the runtime spec's "Documented
/// Deviations") gives every `BuiltinClass` -- including `Object` itself --
/// the same shared placeholder class object as its own `.class`, tagged
/// `BuiltinClass::Object`.
///
/// Every non-`Object` builtin's superclass below is `Object` directly, a
/// Milestone 1 simplification of the real class hierarchy
/// (`newspeak-spec.pdf` places `Boolean` above `True`/`False`, for
/// instance): `builtins::boolean` (Task 8) must not rely on `True`/`False`
/// actually inheriting from a `Boolean` superclass node, since they do not
/// yet. Fix this only if a corpus module or fixture test needs real
/// `Boolean`/`True`/`False` inheritance to observe correct behavior.
fn build_builtin_classes(interp: &mut Interpreter) -> HashMap<BuiltinClass, ClassRef> {
    // NOTE: deliberately *not* `use BuiltinClass::*;` here (the brief's
    // original draft did) -- `BuiltinClass::Object` and `crate::types::Object`
    // (the object-model struct, brought into scope by `use crate::types::*;`
    // above) share the bare name `Object`. A glob-import of the enum's
    // variants shadows the struct, so `Object { class: ..., .. }` below
    // would silently try to build a `BuiltinClass::Object` unit variant
    // with named fields instead of a `types::Object` struct literal -- a
    // compile error the type checker catches, but only because the fields
    // don't match; a differently-shaped `Object` struct could have made
    // this shadow compile into the wrong construction entirely. Referring
    // to every variant through its `BuiltinClass::` qualifier avoids the
    // collision.
    let kinds = [
        BuiltinClass::Object,
        BuiltinClass::Boolean,
        BuiltinClass::True,
        BuiltinClass::False,
        BuiltinClass::Integer,
        BuiltinClass::Float,
        BuiltinClass::Str,
        BuiltinClass::Sym,
        BuiltinClass::Character,
        BuiltinClass::Array,
        BuiltinClass::OrderedCollection,
        BuiltinClass::Nil,
        BuiltinClass::Block,
        BuiltinClass::Platform,
    ];
    // A builtin's behaviour is native (`builtins::dispatch_native`), so its
    // mixin declares nothing: the application exists to give the builtin the
    // same shape every other class object has, and -- for the non-`Object`
    // builtins -- to link an application chain parallel to the class-object
    // chain, which `dispatch::next_level` steps in lockstep.
    let object_application = interp.apply_mixin(&builtin_mixin(BuiltinClass::Object), None);
    let class_of_class = build_self_referential_object_class(object_application.clone());
    let mut map = HashMap::new();
    for kind in kinds {
        let (superclass, application) = if kind == BuiltinClass::Object {
            (None, object_application.clone())
        } else {
            (
                Some(class_of_class.clone()),
                interp.apply_mixin(&builtin_mixin(kind), Some(&object_application)),
            )
        };
        let obj = Rc::new(RefCell::new(Object {
            class: class_of_class.clone(),
            identity_hash: interp.alloc_identity_hash(),
            data: ObjectData::Class(ClassData {
                application,
                enclosing: None,
                declaring_class: None,
                superclass,
                builtin: Some(kind),
                metaclass: None,
            }),
        }));
        map.insert(kind, obj);
    }
    map.insert(BuiltinClass::Object, class_of_class);
    map
}

/// The empty mixin behind one `BuiltinClass`. A fresh `Rc` per call, so the
/// application cache keys the builtins apart from each other even though
/// their mixins are structurally identical.
fn builtin_mixin(kind: BuiltinClass) -> Rc<crate::mixin::Mixin> {
    Rc::new(crate::mixin::Mixin::empty(Symbol::intern(&format!(
        "{kind:?}"
    ))))
}

/// The empty mixin `metaclass_of` applies for a class that declares no
/// class side of its own but inherits one from an ancestor -- same
/// placeholder technique as `builtin_mixin` above, a fresh `Rc` per call so
/// the application cache keys each such level apart from its siblings.
/// Contributes no methods or slots of its own; its only job is to give
/// `apply_mixin` something to link the ancestor's metaclass application
/// through.
fn empty_class_side_mixin(owner_name: Symbol) -> Rc<crate::mixin::Mixin> {
    Rc::new(crate::mixin::Mixin::empty(Symbol::intern(&format!(
        "{} class",
        owner_name.as_str()
    ))))
}

/// Builds the single, genuinely self-referential `Object`-tagged class
/// object every `BuiltinClass` shares as its own `.class` (see this task's
/// design note on the bootstrap's identity problem). `Object.class` is
/// non-optional (no `Option`/`Weak`), so this cannot be built via
/// `Rc::new_cyclic` in one safe step: `new_cyclic`'s closure only ever
/// gets a `Weak` that fails to upgrade until construction finishes, and
/// using `unreachable!()` as a throwaway placeholder panics immediately on
/// evaluation (eager, not lazy) rather than compiling to something inert.
/// Instead: allocate the memory uninitialized, mint a second strong `Rc`
/// handle to the SAME allocation (its count balanced via
/// `Rc::increment_strong_count` before `Rc::from_raw`), write the real
/// value using that handle as `class`, then `assume_init`. This is the
/// same technique already reviewed and approved for `types.rs`'s
/// `dummy_class` test helper (Task 3) and reused correctly in
/// `dispatch.rs`'s test helper (Task 5) -- here it builds a real,
/// production bootstrap object, not a test fixture.
fn build_self_referential_object_class(
    application: Rc<crate::mixin::MixinApplication>,
) -> ClassRef {
    let uninit: Rc<std::mem::MaybeUninit<RefCell<Object>>> = Rc::new_uninit();
    let raw_mut = Rc::as_ptr(&uninit) as *mut std::mem::MaybeUninit<RefCell<Object>>;
    let data_ptr = raw_mut as *const RefCell<Object>;
    // SAFETY: `data_ptr` points at the allocation `uninit` owns; we bump
    // its strong count by one before minting a second `Rc` handle to it
    // via `from_raw`, so the two handles (`uninit`, and the `class` field
    // written below) share a correctly balanced count. `MaybeUninit<T>` is
    // guaranteed to have the same layout as `T`, so the pointer cast is
    // sound; neither handle is dereferenced before the `write` below
    // completes.
    unsafe { Rc::increment_strong_count(data_ptr) };
    let self_ref: ClassRef = unsafe { Rc::from_raw(data_ptr) };
    unsafe {
        (*raw_mut).write(RefCell::new(Object {
            class: self_ref,
            identity_hash: 0,
            data: ObjectData::Class(ClassData {
                application,
                enclosing: None,
                declaring_class: None,
                superclass: None,
                builtin: Some(BuiltinClass::Object),
                metaclass: None,
            }),
        }));
    }
    // SAFETY: the write above just initialized the payload; `uninit` is
    // the allocation's original handle, now fully initialized.
    unsafe { uninit.assume_init() }
}
