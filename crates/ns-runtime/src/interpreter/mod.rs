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
        };
        interp.builtin_classes = Some(build_builtin_classes(&mut interp));
        interp
    }

    /// Switches engines. Call after `load_module` and `compile_module`; the
    /// default is `TreeWalk`, so every Milestone 1 caller keeps its
    /// behavior.
    pub fn set_engine(&mut self, engine: Engine) {
        self.engine = engine;
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
        method: Rc<ast::MethodDecl>,
        defining_class: ClassRef,
        args: Vec<Value>,
        bindings: Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
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
        let temp_count = match &compiled {
            Some(code) => (code.num_registers - code.num_args) as usize,
            None => method.body.temps.len(),
        };
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
            slot_base: 0, // a method body never contains InitSlot
        }));
        let result = match &compiled {
            Some(code) => crate::vm::run_code(self, &frame, code, &bindings),
            None => self.run_statements(&method.body.stmts, &frame, &bindings),
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

    /// A class object around an already-computed application. The one place
    /// the three-way invariant is established: a class object's `superclass`
    /// is linked exactly when its application's `superclass` is (callers
    /// derive `application` from `superclass` through `apply_mixin`), which
    /// is what `dispatch::next_level` relies on when it steps both chains
    /// together.
    ///
    /// `class` is the shared `Object` placeholder -- Milestone 1's "no
    /// metaclasses" simplification, still in force until Task 8 gives a
    /// class object a real class side.
    pub(crate) fn new_class_object(
        &mut self,
        application: Rc<crate::mixin::MixinApplication>,
        enclosing: Option<ObjRef>,
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
                superclass,
                builtin: None,
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
        for (i, slot) in mixin.instance_slots.iter().enumerate() {
            slot_index.insert(Symbol::intern(&slot.name.name), inherited + i);
        }
        // The nested-class cache slots. They are part of the instance layout
        // because the spec caches a nested class in the enclosing *object*;
        // a side table keyed by object would cost a hash lookup on a path
        // that is otherwise a slot read.
        let mut nested_slot = superclass.map_or_else(HashMap::new, |s| s.nested_slot.clone());
        let mut next = inherited + mixin.instance_slots.len();
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
                superclass,
                builtin: Some(kind),
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
    Rc::new(crate::mixin::Mixin {
        name: Symbol::intern(&format!("{kind:?}")),
        methods: HashMap::new(),
        instance_slots: vec![],
        init_exprs: vec![],
        factory: None,
        nested: vec![],
        class_side: None,
        decl: None,
    })
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
                superclass: None,
                builtin: Some(BuiltinClass::Object),
            }),
        }));
    }
    // SAFETY: the write above just initialized the payload; `uninit` is
    // the allocation's original handle, now fully initialized.
    unsafe { uninit.assume_init() }
}
