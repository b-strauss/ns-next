//! Sends, method invocation, and the `Signal` non-local-return/error channel,
//! shared by everything that runs Newspeak code. Implicit-receiver and
//! `super` sends are in `sends.rs`, register and outer-local access in
//! `locals.rs`, the errors a deferred literal raises in `literals.rs`; the
//! bootstrap and class instantiation are in `instantiate.rs`.

pub mod instantiate;
pub(crate) mod literals;
pub(crate) mod locals;
pub(crate) mod nested;
pub(crate) mod sends;

use std::collections::HashMap;
use std::rc::Rc;

use crate::compiler::Lowering;
use crate::dispatch::{self, Lookup};
use crate::heap::{ActivationHeader, ActivationRef, Heap};
use crate::types::*;
use crate::Symbol;

pub enum Signal {
    Return(ActivationRef, Value),
    Error(NewspeakError),
}

pub type EvalResult = Result<Value, Signal>;

/// L: the most activations that may be live at once, counted by the rule in
/// `docs/design/compiler.md`, "How the limit counts". It bounds memory, not
/// the native stack, which the trampoline no longer recurses on: with the
/// exempt block activations, at most one per level of block nesting above
/// each counted one, the stack holds at most 129 x L entries -- 128 being
/// the parser's nesting ceiling, which `image::decode` holds an artifact to
/// as well (`docs/design/execution.md`, "The depth limit").
pub const MAX_ACTIVATION_DEPTH: usize = 100_000;

/// What a send decided, before anything runs (`docs/design/execution.md`,
/// "Deciding, then running"). Deciding is `interpreter/`'s -- lookup,
/// `doesNotUnderstand:`, the natives, the nested-class accessor -- and running
/// what was decided is the VM's, which is how `vm.rs` stays free of dispatch
/// logic once sends stop recursing.
pub(crate) enum Step {
    /// The send's value is already known: a native leaf, a slot accessor, a
    /// nested class, a finished instantiation.
    Value(Value),
    /// Activate `method` on `receiver`; `defining_class` becomes the
    /// activation's current class (§3.6).
    Method {
        receiver: Value,
        method: Rc<crate::mixin::MixinMethod>,
        defining_class: ClassRef,
        args: StepArgs,
    },
    /// Activate the closure object `closure`, and do `then` with its value.
    /// A builtin control-flow native's block is marked by its `args`,
    /// `StepArgs::ControlFlow`.
    Block {
        closure: ObjRef,
        args: StepArgs,
        then: Continuation,
    },
    /// Activate `level`'s instance initializer on `receiver`, and deliver its
    /// value: an instantiation's last initializer, whose value is the
    /// instance (`Interpreter::descend_levels` says why). Its code and slot
    /// base are `level`'s, found when it is pushed.
    ///
    /// This and the three bodies below are inline rather than one boxed
    /// description of an activation: every instantiation with code runs at
    /// least one of them, and boxing each one's description measured 5-8%
    /// slower on a chain of two levels with code. Each is smaller than
    /// `Step::Method`, deliberately: an `Initializer` as large as it (with an
    /// optional continuation instead of `InitializerThen`) measured 4-6%
    /// slower on every benchmark, none of which instantiates anything in its
    /// loop, though `Result<Step, Signal>` kept its size -- presumably
    /// because two variants of the largest size leave the discriminant a
    /// different niche, the effect `StepArgs::ControlFlow`'s comment
    /// describes.
    Initializer {
        receiver: Value,
        level: ClassRef,
        args: Vec<Value>,
    },
    /// Activate `level`'s instance initializer on the instance `then` is
    /// initializing, and carry the walk on below it when it returns: an
    /// initializer with a level below it that still has code.
    InitializerThen {
        level: ClassRef,
        args: Vec<Value>,
        then: Box<Instantiation>,
    },
    /// Activate `code`, the arguments of `level`'s superclass clause message,
    /// on the instance `then` is initializing and with `level`'s factory
    /// arguments (§6.2); its value is the superclass's arguments, with which
    /// `then` carries the walk on.
    ClauseMessage {
        level: ClassRef,
        code: Rc<crate::bytecode::CompiledCode>,
        args: Vec<Value>,
        then: Box<Instantiation>,
    },
    /// Activate the superclass clause of the nested class `creation`
    /// describes, on its enclosing object and as part of the class that
    /// declares it; its value is the superclass, from which the class is
    /// built, cached and answered (`nested::after_clause`).
    SuperclassClause(Box<NestedClassCreation>),
}

/// Where an activation's arguments come from.
pub(crate) enum StepArgs {
    /// The send's own arguments.
    Send,
    /// None, for the block a builtin control-flow native activates --
    /// `ifTrue:` and its three relatives, `and:`, `or:`, `whileTrue:`,
    /// `whileFalse:` -- which is the activation the optimized lowering
    /// inlines away, and which `push_step` therefore exempts from the depth
    /// count when the send was a `SendInlinable`. Only those natives build
    /// it.
    ///
    /// The mark rides on the arguments rather than in a `bool` of its own on
    /// `Step::Block`. That field measured 4-8% slower on all three
    /// benchmarks even with nothing reading it, including `bench_loop.ns`,
    /// which activates nothing in its loop -- presumably because a `bool`
    /// offers the compiler a niche for `Step`'s discriminant, which changes
    /// how every decision's `Result<Step, Signal>` is encoded, though
    /// neither type grew.
    ControlFlow,
    /// Arguments the decision built: `doesNotUnderstand:`'s message.
    Owned(Vec<Value>),
}

impl StepArgs {
    pub(crate) fn slice<'a>(&'a self, send: &'a [Value]) -> &'a [Value] {
        match self {
            StepArgs::Send => send,
            StepArgs::ControlFlow => &[],
            StepArgs::Owned(args) => args,
        }
    }
}

pub struct Interpreter {
    pub(crate) heap: Heap,
    /// Every builtin class object, indexed by `BuiltinClass as usize`.
    /// Indexed rather than hashed because `class_of` asks for one on every
    /// send to a primitive receiver. Empty only while `bootstrap` is
    /// building it; nothing can reach an `Interpreter` in that state.
    builtin_classes: Vec<ClassRef>,
    platform_singleton: Option<Value>,
    /// (mixin identity, superclass application identity) -> behaviour.
    /// Keyed on the superclass's *application*, never on a superclass class
    /// object: a nested superclass has one class object per enclosing
    /// instance, so a class-object key would never hit and every instance
    /// would get its own behaviour.
    applications: HashMap<(usize, usize), Rc<crate::mixin::MixinApplication>>,
    /// mixin identity -> the synthetic empty class side `metaclass_of`
    /// stands in for that mixin's own level when it declares none.
    ///
    /// Cached, not minted per call, because the mixin's `Rc` identity is
    /// what `applications` keys on: a fresh one per call would give two
    /// class objects that share an instance-side application two *different*
    /// metaclass applications, and the shared-application property is
    /// exactly what the inline caches will guard on. It would
    /// also grow `applications` without bound.
    empty_class_sides: HashMap<usize, Rc<crate::mixin::Mixin>>,
    /// How many nested classes are marked as being created
    /// (`nested::begin_creation`), for `run_from_host`'s check that none
    /// outlives the run that began it. The mark itself lives in the class's
    /// cache slot; this only counts it, so a release build keeps nothing.
    #[cfg(debug_assertions)]
    creations_in_progress: usize,
    /// Boxes finished instantiations left behind, emptied, for the next ones
    /// to walk in (`instantiate.rs`'s `walk_state` and `retire_walk`).
    /// Allocating a box and a list of levels per instantiation instead
    /// measured 3-13% slower on instantiation benchmarks. They hold no
    /// handle, so a collector has nothing to trace in them.
    #[allow(clippy::vec_box)] // the boxes are what is reused, not only their contents
    spare_walks: Vec<Box<Instantiation>>,
    /// The lowering every module this interpreter loads is compiled with
    /// (`docs/design/compiler.md`, "The reference lowering"). Fixed at
    /// construction, so one interpreter never mixes the two.
    lowering: Lowering,
    /// The reusable argument buffer the run loop gathers a send's arguments
    /// into (`vm::send`). Taken while a send is decided and pushed and given
    /// back afterwards, because the decision needs `&mut Interpreter` while
    /// it reads the arguments.
    arg_buffer: Vec<Value>,
    /// Whether a host's run of the loop is in progress (`run_from_host`).
    /// Nothing the loop calls can start another -- no decision, native or
    /// continuation runs code from Rust -- and a debug build checks that.
    running: bool,
}

impl Interpreter {
    pub fn bootstrap() -> Interpreter {
        Interpreter::bootstrap_with(Lowering::Optimized)
    }

    /// An interpreter whose `load_module` compiles with `lowering`. The
    /// reference lowering is for checking the optimized one against it; an
    /// artifact decoded into either runs the code it carries.
    pub fn bootstrap_with(lowering: Lowering) -> Interpreter {
        let mut interp = Interpreter {
            heap: Heap::new(),
            builtin_classes: Vec::new(),
            platform_singleton: None,
            applications: HashMap::new(),
            empty_class_sides: HashMap::new(),
            #[cfg(debug_assertions)]
            creations_in_progress: 0,
            spare_walks: Vec::new(),
            lowering,
            arg_buffer: Vec::new(),
            running: false,
        };
        interp.builtin_classes = build_builtin_classes(&mut interp);
        interp
    }

    pub fn builtin_class(&self, kind: BuiltinClass) -> ClassRef {
        self.builtin_classes[kind as usize]
    }

    /// After `image::load`: never hand out an identity hash a loaded object
    /// already carries (`docs/design/image-format.md`, "Sections").
    ///
    /// `max < u32::MAX` is the caller's guarantee, and an enforced one:
    /// `image::decode` refuses an image carrying `u32::MAX` outright
    /// (`DecodeError::IdentityHashExhausted`), because there is no counter
    /// value past it. That refusal is what keeps `Heap::advance_identity_hash_past`'s
    /// `max.saturating_add(1)` in range; the counter's own saturation is what
    /// keeps the *next* hash in range when the image's maximum is one below
    /// the ceiling.
    pub(crate) fn advance_identity_hash_past(&mut self, max: u32) {
        self.heap.advance_identity_hash_past(max);
    }

    /// The heap, for reading. Public so integration tests can exercise the
    /// seam directly; the field itself stays `pub(crate)` so no caller
    /// outside the crate can hold one across a call back into the
    /// interpreter.
    pub fn heap_ref(&self) -> &Heap {
        &self.heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    /// An ordinary (explicit-receiver) send from a host: `newspeak-spec.pdf`
    /// §5.5, decided as `decide_send` decides it and run from the outside
    /// (`run_from_host`). A `Send*` the VM issues never comes through here:
    /// `vm.rs` decides it and pushes what was decided itself.
    pub fn send_unrooted(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
    ) -> EvalResult {
        match self.decide_send(receiver, selector, &args) {
            Ok(step) => self.run_from_host(step, &args),
            Err(signal) => Err(signal),
        }
    }

    /// Runs what a host's send or instantiation decided. It is the only way
    /// Rust enters the run loop (`docs/design/execution.md`, "No re-entry from
    /// Rust"): a value needs no activation, and anything else is pushed and
    /// run by `vm::run`, which pops it and everything it activates on every
    /// path out.
    ///
    /// Three things are checked in a debug build, because each is otherwise
    /// silent. The loop is never entered while it is running -- nothing it
    /// calls can reach here. The stack is left as deep as it was found: a
    /// leaked entry would put a later activation on top of a dead one's
    /// registers (a leak inside a run is caught too, by
    /// `Heap::pop_activation`'s check that it pops the top). And no nested
    /// class's creation outlives the run that began it.
    fn run_from_host(&mut self, step: Step, send_args: &[Value]) -> EvalResult {
        debug_assert!(
            !self.running,
            "internal: the run loop was re-entered from Rust"
        );
        let depth = self.heap.activation_depth();
        let result = match step {
            Step::Value(value) => Ok(value),
            step => match self.push_step(step, send_args, None, false) {
                Ok(act) => {
                    self.running = true;
                    let result = crate::vm::run(self, act);
                    self.running = false;
                    result
                }
                Err(signal) => Err(signal),
            },
        };
        debug_assert_eq!(
            self.heap.activation_depth(),
            depth,
            "a host's send or instantiation must leave the activation stack as it found it"
        );
        #[cfg(debug_assertions)]
        assert_eq!(
            self.creations_in_progress, 0,
            "internal: a nested class's creation outlived the run that began it"
        );
        result
    }

    /// Decides what an ordinary (explicit-receiver) send would run, without
    /// running it (`docs/design/execution.md`, "Deciding, then running").
    ///
    /// Checked first, before any lookup: `ClassName new` (or a class's own
    /// non-default factory selector) never has a real method to find --
    /// every class object's own `.class` is a shared placeholder with an
    /// empty method table (see `build_builtin_classes` below), so an
    /// ordinary `lookup_public` search would just fall through to
    /// `doesNotUnderstand:` every time. `try_factory_send`
    /// (`instantiate.rs`) recognizes the shape and decides it as an
    /// instantiation (`decide_instantiation`) -- not through the host entry
    /// `instantiate`, which would enter the run loop from inside it, the
    /// re-entry `run_from_host` refuses; see its own doc comment for why this can
    /// never double-fire against the superclass clause's own deferred
    /// factory message (`climb_levels` never decides a send).
    pub(crate) fn decide_send(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: &[Value],
    ) -> Result<Step, Signal> {
        if let Some(result) = self.try_factory_send(&receiver, selector, args) {
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
            let is_class = self.heap.is_class(*obj);
            if is_class {
                if let Some(metaclass) = self.metaclass_of(*obj) {
                    match dispatch::lookup_public(&self.heap, metaclass, selector) {
                        Lookup::Method(method, defining_class) => {
                            return Ok(Step::Method {
                                receiver,
                                method,
                                defining_class,
                                args: StepArgs::Send,
                            });
                        }
                        Lookup::NestedClass(mixin, declaring_class) => {
                            return nested::nested_class(self, &receiver, &mixin, declaring_class);
                        }
                        Lookup::Native(kind) => {
                            return self.decide_native_or_dnu(kind, receiver, selector, args);
                        }
                        Lookup::NotFound => {}
                    }
                }
            }
        }
        let class = self.class_of(&receiver);
        match dispatch::lookup_public(&self.heap, class, selector) {
            Lookup::Method(method, defining_class) => Ok(Step::Method {
                receiver,
                method,
                defining_class,
                args: StepArgs::Send,
            }),
            // §6: the implicit accessor a nested declaration defines is
            // reached exactly like any other member, and it ignores `args`
            // -- the accessor is unary, so a send that supplied arguments
            // did not name it (`walk` matched on the bare selector).
            Lookup::NestedClass(mixin, declaring_class) => {
                nested::nested_class(self, &receiver, &mixin, declaring_class)
            }
            Lookup::Native(kind) => self.decide_native_or_dnu(kind, receiver, selector, args),
            Lookup::NotFound => self.decide_does_not_understand(receiver, selector, args),
        }
    }

    /// Native dispatch that still inherits `Object`'s own native methods.
    ///
    /// `dispatch::walk` deliberately treats a class tagged
    /// `builtin: Some(kind)` as a *dispatch leaf*: it returns
    /// `Lookup::Native(kind)` the moment it reaches one, without continuing
    /// up that class's own superclass chain -- that early return is what
    /// keeps `lookupPublic`'s `hard_stop_on_mismatch` semantics uniform.
    /// Every non-`Object` builtin's superclass is `Object` (see
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
    ) -> Option<Result<Step, Signal>> {
        if let Some(result) =
            crate::builtins::dispatch_native(self, kind, *receiver, selector, args)
        {
            return Some(result);
        }
        if kind == BuiltinClass::Object {
            return None;
        }
        crate::builtins::dispatch_native(self, BuiltinClass::Object, *receiver, selector, args)
    }

    fn decide_native_or_dnu(
        &mut self,
        kind: BuiltinClass,
        receiver: Value,
        selector: Symbol,
        args: &[Value],
    ) -> Result<Step, Signal> {
        match self.dispatch_native_inheriting_object(kind, &receiver, selector, args) {
            Some(result) => result,
            None => self.decide_does_not_understand(receiver, selector, args),
        }
    }

    /// §5.5's `doesNotUnderstand:` fallback -- access-blind (`lookup_defined_for`),
    /// per `docs/design/dispatch.md`.
    ///
    /// Two cases reach here. A user-overridden `doesNotUnderstand:` arrives
    /// as `Lookup::Method`; `Object`'s own *native* one
    /// (`builtins::object_class::dispatch`) arrives as `Lookup::Native`,
    /// exactly like `decide_native_or_dnu` reaches any other native
    /// method. Without the native arm, `Object`'s builtin
    /// `doesNotUnderstand:` -- which every class inherits, since
    /// `lookup_defined_for` finds `ClassData.builtin` on any class with no
    /// method override -- would never actually run.
    fn decide_does_not_understand(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: &[Value],
    ) -> Result<Step, Signal> {
        let class = self.class_of(&receiver);
        let dnu_selector = Symbol::DOES_NOT_UNDERSTAND;
        // A minimal message-mirror-shaped value: an Array `{selector. args}`
        // rather than a real MessageMirror object, since mirrors do not
        // exist yet (`docs/design/liveness-and-reflection.md`, "Mirrors and
        // MirrorBuilder"). Callers relying on a real `MessageMirror` API are
        // out of scope.
        match dispatch::lookup_defined_for(&self.heap, class, dnu_selector) {
            Lookup::Method(method, defining_class) => {
                let selector_value = self.new_symbol(selector);
                let args_value = Value::Obj(self.new_array(args.to_vec()));
                let message_arg = Value::Obj(self.new_array(vec![selector_value, args_value]));
                Ok(Step::Method {
                    receiver,
                    method,
                    defining_class,
                    args: StepArgs::Owned(vec![message_arg]),
                })
            }
            Lookup::Native(kind) => {
                let selector_value = self.new_symbol(selector);
                let args_value = Value::Obj(self.new_array(args.to_vec()));
                let message_arg = Value::Obj(self.new_array(vec![selector_value, args_value]));
                // Same two-step fallback as `decide_native_or_dnu`: `kind`
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

    pub(crate) fn take_arg_buffer(&mut self) -> Vec<Value> {
        let mut buffer = std::mem::take(&mut self.arg_buffer);
        buffer.clear();
        buffer
    }

    pub(crate) fn return_arg_buffer(&mut self, buffer: Vec<Value>) {
        self.arg_buffer = buffer;
    }

    /// Pushes an activation, refusing one that would take the counted depth
    /// past `MAX_ACTIVATION_DEPTH`. Every activation in the runtime is pushed
    /// here, so the count and the check cannot be skipped by a new site.
    pub(crate) fn push(
        &mut self,
        header: ActivationHeader,
        args: &[Value],
        temps: usize,
    ) -> Result<ActivationRef, Signal> {
        if header.counted && self.heap.counted_depth() >= MAX_ACTIVATION_DEPTH {
            return Err(recursion_too_deep(&header));
        }
        Ok(self.heap.push_activation(header, args, temps))
    }

    /// Pushes the activation `step` describes. `send_args` are the arguments
    /// of the send that decided it, which `StepArgs::Send` refers to;
    /// `sender` is the activation that sent it, if the run loop did, and
    /// `inlinable` whether the send was a `SendInlinable` -- the two things
    /// the depth count's one exemption asks about.
    pub(crate) fn push_step(
        &mut self,
        step: Step,
        send_args: &[Value],
        sender: Option<ActivationRef>,
        inlinable: bool,
    ) -> Result<ActivationRef, Signal> {
        match step {
            Step::Value(_) => unreachable!("internal: a value needs no activation"),
            Step::Method {
                receiver,
                method,
                defining_class,
                args,
            } => {
                let args = args.slice(send_args);
                check_method_arity(&method, receiver, args.len())?;
                let code = method.code.clone();
                let temps = (code.num_registers - code.num_args) as usize;
                self.push(
                    ActivationHeader {
                        receiver,
                        current_class: Some(defining_class),
                        kind: FrameKind::Method(method),
                        slot_base: 0, // a method body never contains InitSlot
                        code,
                        then: Continuation::Deliver,
                        counted: true,
                    },
                    args,
                    temps,
                )
            }
            // A closure activation catches no return: its `^` targets its home.
            Step::Block {
                closure,
                args,
                then,
            } => {
                let control_flow = matches!(args, StepArgs::ControlFlow);
                let args = args.slice(send_args);
                let (code, num_params, captured_frame) = self
                    .heap
                    .closure_activation(closure)
                    .expect("internal: a block step is decided only for a closure object");
                check_block_arity(&self.heap, num_params, captured_frame, args.len())?;
                // Exempt exactly the activation the optimized lowering
                // inlines away: a block a control-flow native activates for
                // a `SendInlinable`, and only if the sending activation
                // created it -- which every block the reference lowering
                // passes to one is, and which keeps a crafted artifact from
                // recursing uncounted (`docs/design/compiler.md`, "How the
                // limit counts"). Asked in this order so that an ordinary
                // `value` never reads the sender's entry.
                let created_by_sender = |heap: &Heap| {
                    sender
                        .and_then(|sender| heap.activation_reified(sender))
                        .is_some_and(|frame| frame.same(captured_frame))
                };
                let counted = !(inlinable && control_flow && created_by_sender(&self.heap));
                let receiver = self.heap.frame_receiver(captured_frame);
                let current_class = self.heap.frame_current_class(captured_frame);
                let temps = (code.num_registers - code.num_args) as usize;
                self.push(
                    ActivationHeader {
                        receiver,
                        current_class,
                        kind: FrameKind::Closure(closure),
                        slot_base: 0, // a closure body never contains InitSlot
                        code,
                        then,
                        counted,
                    },
                    args,
                    temps,
                )
            }
            // An initializer level, a superclass clause or its message is
            // never an activation the optimized lowering inlines away, so
            // each of the three below is always counted.
            Step::Initializer {
                receiver,
                level,
                args,
            } => self.push_initializer(receiver, level, &args, Continuation::Deliver),
            Step::InitializerThen { level, args, then } => {
                self.push_initializer(then.instance, level, &args, Continuation::Instantiate(then))
            }
            Step::ClauseMessage {
                level,
                code,
                args,
                then,
            } => {
                let temps = (code.num_registers - code.num_args) as usize;
                self.push(
                    ActivationHeader {
                        receiver: then.instance,
                        current_class: Some(level),
                        kind: FrameKind::SuperclassMessage(level),
                        slot_base: 0, // a clause message never contains InitSlot
                        code,
                        then: Continuation::Instantiate(then),
                        counted: true,
                    },
                    &args,
                    temps,
                )
            }
            Step::SuperclassClause(creation) => {
                let code = nested::superclass_clause_code(&creation.nested);
                let temps = (code.num_registers - code.num_args) as usize;
                let (enclosing, index) = (creation.enclosing, creation.index);
                let act = self.push(
                    ActivationHeader {
                        receiver: Value::Obj(enclosing),
                        current_class: Some(creation.declaring_class),
                        kind: FrameKind::SuperclassClause(creation.declaring_class),
                        slot_base: 0, // a clause never contains InitSlot
                        code,
                        then: Continuation::NestedClass(creation),
                        counted: true,
                    },
                    &[],
                    temps,
                )?;
                // A nested class's creation is in progress exactly while its
                // clause's activation is on the stack: marked once the push
                // has succeeded, and unmarked by `resume` or `abandon` when
                // the activation is popped. A push refused at the depth limit
                // marks nothing.
                nested::begin_creation(self, enclosing, index);
                Ok(act)
            }
        }
    }

    /// Pushes `level`'s instance initializer on `receiver`, the one body
    /// `Step::Initializer` and `Step::InitializerThen` share; only what
    /// happens once it returns differs.
    fn push_initializer(
        &mut self,
        receiver: Value,
        level: ClassRef,
        args: &[Value],
        then: Continuation,
    ) -> Result<ActivationRef, Signal> {
        let (code, slot_base) = self.initializer_body(level);
        let temps = (code.num_registers - code.num_args) as usize;
        self.push(
            ActivationHeader {
                receiver,
                current_class: Some(level),
                kind: FrameKind::Initializer(level),
                slot_base,
                code,
                then,
                counted: true,
            },
            args,
            temps,
        )
    }

    /// What a control-flow native does once the block it activated has
    /// returned `value`: the second half of the native, which the run loop
    /// waits for instead of a Rust frame. `then` is the continuation of the
    /// block's activation, which the loop has just popped. It answers the
    /// value for the caller, or another activation to run in the popped
    /// one's place -- a native loop's next block, or the method a
    /// non-Boolean condition's `mustBeBoolean` finds -- which the loop hands
    /// to `push_continuation`.
    pub(crate) fn resume(&mut self, then: Continuation, value: Value) -> Result<Step, Signal> {
        match then {
            Continuation::Deliver => Ok(Step::Value(value)),
            Continuation::Truthy => Ok(Step::Value(Value::Bool(crate::builtins::boolean::truthy(
                value,
            )))),
            Continuation::WhileCondition(state) => {
                crate::builtins::block::after_condition(self, state, value)
            }
            Continuation::WhileBody(state) => Ok(crate::builtins::block::after_body(state)),
            Continuation::DoElement(state) => {
                Ok(crate::builtins::collections::after_element(state))
            }
            Continuation::NestedClass(creation) => {
                nested::end_creation(self, &creation);
                nested::after_clause(self, *creation, value)
            }
            Continuation::Instantiate(state) => self.advance_instantiation(state, value),
        }
    }

    /// What a continuation undoes when its activation is thrown away rather
    /// than returned from: the counterpart of `resume`, which `vm::unwind`
    /// calls for every activation it pops. Only a nested class's creation has
    /// begun anything -- its cache slot is marked as in progress -- so only it
    /// has anything to undo; every other continuation is simply dropped.
    pub(crate) fn abandon(&mut self, then: Continuation) {
        if let Continuation::NestedClass(creation) = then {
            nested::end_creation(self, &creation);
        }
    }

    /// Pushes the activation a continuation decided (`resume`), in place of
    /// the one just popped; `replaced_counted` is that one's
    /// `Popped::counted`. The exemption's rule for a native loop lives here
    /// rather than in the run loop, which only forwards the flag, so a JIT
    /// driving continuations reaches the same rule: the next block is exempt
    /// only if the one it replaces was, and only if it too was created by
    /// the activation below it -- the loop's sender, now the top of the
    /// stack -- which `push_step` checks. A method is counted whatever it
    /// replaces.
    ///
    /// It takes the flag rather than the whole `Popped`, whose continuation
    /// `resume` has consumed by then: a version that took the `Popped` and
    /// pushed inside `resume` measured about 1.3% slower on a native
    /// `whileTrue:` loop, and this one level with the rule inline in the
    /// loop.
    pub(crate) fn push_continuation(
        &mut self,
        step: Step,
        replaced_counted: bool,
    ) -> Result<ActivationRef, Signal> {
        let sender = self.heap.top_activation();
        // A continuation's step never refers to a send's arguments
        // (`StepArgs::Send` is only answered by a decision about a send, and
        // the one a continuation makes -- `mustBeBoolean` -- has none), so
        // `&[]` is always right here.
        self.push_step(step, &[], sender, !replaced_counted)
    }

    /// Whether a `^` aimed at `act` ends at `act`: a method's does, and a
    /// superclass clause's or its message's. An initializer catches none -- a
    /// `^` into a running one escapes, the known gap in
    /// `docs/design/execution.md`, "Non-local return" -- and a block's `^`
    /// always targets its home, never the block.
    pub(crate) fn catches_return(&self, act: ActivationRef) -> bool {
        matches!(
            self.heap.activation_kind(act),
            FrameKind::Method(_) | FrameKind::SuperclassClause(_) | FrameKind::SuperclassMessage(_)
        )
    }

    /// Where a `^` in `act` returns to. A block returns from its home --
    /// the method or initializer that lexically encloses it -- and only
    /// while that activation is still running: its reified frame is married
    /// to a live stack entry exactly until the activation returns, and a
    /// frame the image decoder built never is. A dead home is §7.2's
    /// `cannotReturn`. Every other activation's `^` returns from itself.
    pub(crate) fn raise_return(&mut self, act: ActivationRef, value: Value) -> Signal {
        let target = match self.heap.activation_home(act) {
            None => act,
            Some(home) => match self.heap.frame_activation(home) {
                Some(live) => live,
                None => {
                    return Signal::Error(NewspeakError {
                        selector: Symbol::CARET,
                        receiver: value,
                        message:
                            "cannotReturn: the enclosing method activation has already returned"
                                .to_string(),
                    })
                }
            },
        };
        Signal::Return(target, value)
    }

    pub fn class_of(&self, value: &Value) -> ClassRef {
        match value {
            Value::Obj(obj) => self.heap.class_of_obj(*obj),
            // Nil/Bool/SmallInt/Float classes are wired up by the bootstrap
            // into interpreter-held globals, read from there via
            // `Interpreter::builtin_class(kind)`.
            _ => self.builtin_class_for_primitive(value),
        }
    }

    /// The metaclass carrying `class`'s class side (§6.3), created on first
    /// use and cached on `class` itself (`ClassData::metaclass`) -- not in a
    /// side table, and not in an instance slot the way a nested class's
    /// cache is: a class object's payload is `ObjectData::Class`, never
    /// `Instance`, so it has no slot vector for `nested_slot` to index into.
    /// Caching on the class object directly also gives every send against
    /// the same class object the same metaclass identity, which the inline
    /// caches will need.
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
    pub fn metaclass_of(&mut self, class: ClassRef) -> Option<ClassRef> {
        // Walk out to the first level that already answers -- a cached
        // metaclass, a non-class object, or the end of the chain -- pushing
        // each level that still has to be built. `chain` is nearest-first,
        // so the build below runs it in reverse.
        let mut chain: Vec<ClassRef> = Vec::new();
        let mut super_metaclass: Option<ClassRef> = None;
        let mut level = class;
        loop {
            // Not a class object: it has no metaclass, and nothing below it
            // can inherit one through this level.
            if !self.heap.is_class(level) {
                break;
            }
            let cached = self.heap.class_metaclass(level);
            let superclass = self.heap.class_superclass(level);
            if cached.is_some() {
                super_metaclass = cached;
                break;
            }
            chain.push(level);
            let Some(superclass) = superclass else { break };
            level = superclass;
        }
        for level in chain.iter().rev() {
            let own_application = self
                .heap
                .class_application(*level)
                .expect("only class objects are pushed onto the chain");
            let own_mixin = own_application.mixin.clone();
            let enclosing = self.heap.class_enclosing(*level);
            let declaring_class = self.heap.class_declaring(*level);
            let class_side = own_mixin.class_side.clone();
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
                    self.empty_class_side_mixin(&own_mixin)
                }
            };
            let super_application = super_metaclass
                .as_ref()
                .and_then(|metaclass| self.heap.class_application(*metaclass));
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
            if !self.heap.set_class_metaclass(*level, metaclass) {
                unreachable!("only class objects are pushed onto the chain");
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
        self.heap.alloc_object(class, data)
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
        // There is no dedicated Character payload variant on
        // ObjectData -- represent a character as a one-`char` Rust String
        // wrapped the same way String literals are (Str), consistent with
        // how the language spec itself treats characters as "a special
        // case of string literals" going forward (`newspeak-spec.pdf`
        // §2.2.3). `builtins::text` tags the *class* as
        // `BuiltinClass::Character` so dispatch still lands on the right
        // native methods; only the payload representation is shared.
        let class = self.builtin_class(BuiltinClass::Character);
        self.new_instance_object(class, ObjectData::Str(c.to_string()))
    }

    pub fn new_closure(&mut self, closure: ClosureData) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Block);
        self.new_instance_object(class, ObjectData::Closure(closure))
    }

    /// The `platform` object (`docs/design/core-library.md`):
    /// memoized on first call, so every reference within one interpreter
    /// sees the same object identity.
    pub fn platform_value(&mut self) -> Value {
        if let Some(p) = &self.platform_singleton {
            return *p;
        }
        let class = self.builtin_class(BuiltinClass::Platform);
        let obj = self.new_instance_object(class, ObjectData::Instance(vec![]));
        let value = Value::Obj(obj);
        self.platform_singleton = Some(value);
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
        self.heap.alloc_object(
            class,
            ObjectData::Class(ClassData {
                application,
                enclosing,
                declaring_class,
                superclass,
                builtin: None,
                metaclass: None,
            }),
        )
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
        //
        // Keyed by declaration identity: a subclass that redeclares an
        // inherited nested class shadows it for a plain `N` but not for
        // `super N`, so the two need separate slots -- see `nested_slot`.
        let mut nested_slot = superclass.map_or_else(HashMap::new, |s| s.nested_slot.clone());
        let mut next = inherited + mixin.slots.len();
        for nested in &mixin.nested {
            nested_slot.insert(crate::mixin::mixin_key(nested), next);
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

    /// The empty mixin `metaclass_of` applies for a class that declares no
    /// class side of its own but inherits one from an ancestor -- same
    /// placeholder technique as `builtin_mixin` below, one `Rc` per owning
    /// mixin so the application cache keys each such level apart from its
    /// siblings. Contributes no methods or slots of its own; its only job
    /// is to give `apply_mixin` something to link the ancestor's metaclass
    /// application through.
    ///
    /// One per *mixin*, not one per call and not one per class object: a
    /// nested class has one class object per enclosing instance but a
    /// single mixin, and §6.2's "all applications of a mixin share code and
    /// structure" has to hold for the metaclass side too -- otherwise two
    /// class objects that share an instance-side application answer a
    /// class-side send out of two distinct metaclass applications, which is
    /// the property the inline caches will guard on.
    ///
    /// Keying the cache by `owner`'s address is safe for the same reason
    /// `applications` is: every mixin that reaches here is held for the
    /// interpreter's life by an entry in `applications`, so its address is
    /// never reused.
    fn empty_class_side_mixin(
        &mut self,
        owner: &Rc<crate::mixin::Mixin>,
    ) -> Rc<crate::mixin::Mixin> {
        let name = owner.name;
        self.empty_class_sides
            .entry(crate::mixin::mixin_key(owner))
            .or_insert_with(|| {
                Rc::new(crate::mixin::Mixin::empty(Symbol::intern(&format!(
                    "{} class",
                    name.as_str()
                ))))
            })
            .clone()
    }
}

/// Refuses an activation given a different number of arguments than its
/// method takes.
///
/// An activation's locals are addressed as one run of registers -- the
/// arguments first, the declared temporaries after them -- and the run is
/// laid out from how many arguments the activation was *given*, not how many
/// the method declares. A wrong-arity activation therefore does not fail: it
/// shifts every temporary by the difference and reads an argument back where
/// a temporary should be, which no golden output necessarily shows. Silent
/// wrong output with exit status 0 is the failure mode this repository's
/// invariants single out, so the mismatch is refused here as an ordinary
/// Newspeak error -- never a panic, since the caller may be a host.
///
/// No Newspeak-level send can reach this: the parser builds a message whose
/// argument count matches its selector by construction. The reachable
/// callers are host-initiated sends -- `ns-next run --send`, `snapshot
/// --send`, `load --send`, and any future embedder driving
/// `Interpreter::send` directly.
fn check_method_arity(
    method: &crate::mixin::MixinMethod,
    receiver: Value,
    given: usize,
) -> Result<(), Signal> {
    if given == method.num_args {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError {
        selector: method.selector,
        receiver,
        message: format!(
            "wrong number of arguments: `{}` takes {}, got {}",
            method.selector, method.num_args, given
        ),
    }))
}

/// A block given a different number of arguments than it declares is refused
/// before it can reach a register: the window is laid out from the arguments
/// actually given, so too few would address registers past its end -- a panic
/// -- and too many would silently shift every temporary.
fn check_block_arity(
    heap: &Heap,
    num_params: usize,
    captured_frame: FrameRef,
    given: usize,
) -> Result<(), Signal> {
    if given == num_params {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError {
        selector: Symbol::VALUE,
        receiver: heap.frame_receiver(captured_frame),
        message: format!("wrong number of block arguments: block takes {num_params}, got {given}"),
    }))
}

/// L's error, for the activation `header` describes. The error does not name
/// its limit, so that changing it changes no recorded answer. It is kept out
/// of line, so that the check on every push is a compare and a branch and
/// nothing more.
#[cold]
#[inline(never)]
fn recursion_too_deep(header: &ActivationHeader) -> Signal {
    let selector = match &header.kind {
        FrameKind::Method(method) => method.selector,
        FrameKind::Closure(_) => Symbol::VALUE,
        _ => Symbol::NEW,
    };
    Signal::Error(NewspeakError {
        selector,
        receiver: header.receiver,
        message: "stack overflow: recursion too deep".to_string(),
    })
}

/// Every `BuiltinClass` -- including `Object` itself -- gets the same shared
/// placeholder class object as its own `.class`, tagged
/// `BuiltinClass::Object`: there is no reified metaclass hierarchy (the
/// §3.2 deviation in `docs/design/object-model.md`, "Class sides and
/// metaclasses").
///
/// Every non-`Object` builtin's superclass below is `Object` directly, a
/// simplification of the real class hierarchy
/// (`newspeak-spec.pdf` places `Boolean` above `True`/`False`, for
/// instance): `builtins::boolean` must not rely on `True`/`False`
/// actually inheriting from a `Boolean` superclass node, since they do not
/// yet. Fix this only if a corpus module or fixture test needs real
/// `Boolean`/`True`/`False` inheritance to observe correct behavior.
fn build_builtin_classes(interp: &mut Interpreter) -> Vec<ClassRef> {
    // NOTE: deliberately *not* `use BuiltinClass::*;` here --
    // `BuiltinClass::Object` and `crate::types::Object`
    // (the object-model struct, brought into scope by `use crate::types::*;`
    // above) share the bare name `Object`. A glob-import of the enum's
    // variants shadows the struct, so any bare `Object { .. }` literal
    // elsewhere in this function would silently try to build a
    // `BuiltinClass::Object` unit variant with named fields instead of a
    // `types::Object` struct literal -- a compile error the type checker
    // catches, but only because the fields don't match; a differently-shaped
    // `Object` struct could have made this shadow compile into the wrong
    // construction entirely. Referring to every variant through its
    // `BuiltinClass::` qualifier avoids the collision.
    // A builtin's behaviour is native (`builtins::dispatch_native`), so its
    // mixin declares nothing: the application exists to give the builtin the
    // same shape every other class object has, and -- for the non-`Object`
    // builtins -- to link an application chain parallel to the class-object
    // chain, which `dispatch::next_level` steps in lockstep.
    let object_application = interp.apply_mixin(&builtin_mixin(BuiltinClass::Object), None);
    let class_of_class =
        build_self_referential_object_class(&mut interp.heap, object_application.clone());
    let mut classes = Vec::with_capacity(BuiltinClass::ALL.len());
    for kind in BuiltinClass::ALL {
        // `builtin_class` indexes by discriminant, so the vector must be
        // filled in exactly that order.
        debug_assert_eq!(classes.len(), kind as usize);
        if kind == BuiltinClass::Object {
            // The self-referential class every builtin has as its `.class`.
            classes.push(class_of_class);
            continue;
        }
        let application = interp.apply_mixin(&builtin_mixin(kind), Some(&object_application));
        classes.push(interp.heap.alloc_object(
            class_of_class,
            ObjectData::Class(ClassData {
                application,
                enclosing: None,
                declaring_class: None,
                superclass: Some(class_of_class),
                builtin: Some(kind),
                metaclass: None,
            }),
        ));
    }
    classes
}

/// The empty mixin behind one `BuiltinClass`. A fresh `Rc` per call, so the
/// application cache keys the builtins apart from each other even though
/// their mixins are structurally identical.
fn builtin_mixin(kind: BuiltinClass) -> Rc<crate::mixin::Mixin> {
    Rc::new(crate::mixin::Mixin::empty(Symbol::intern(&format!(
        "{kind:?}"
    ))))
}

/// Builds the single, genuinely self-referential `Object`-tagged class
/// object every `BuiltinClass` shares as its own `.class`, which is what
/// lets `Object.class` stay non-optional. The construction itself lives in
/// `Heap::alloc_self_referential_class`; under the arena it is ordinary
/// safe Rust, because the slot's index is known before the slot is filled.
fn build_self_referential_object_class(
    heap: &mut Heap,
    application: Rc<crate::mixin::MixinApplication>,
) -> ClassRef {
    heap.alloc_self_referential_class(ObjectData::Class(ClassData {
        application,
        enclosing: None,
        declaring_class: None,
        superclass: None,
        builtin: Some(BuiltinClass::Object),
        metaclass: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::path::PathBuf;

    fn load(src: &str) -> (Interpreter, ClassRef) {
        let module = ns_syntax::parse(src).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(src),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interp = Interpreter::bootstrap();
        let class = interp.load_module_unrooted(&resolved);
        (interp, class)
    }

    fn expect_ok(result: EvalResult, what: &str) -> Value {
        match result {
            Ok(v) => v,
            Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
            Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
        }
    }

    /// Deciding a send runs nothing: a user method comes back as the
    /// activation to make, with nothing pushed, and a native leaf as its
    /// value (`docs/design/execution.md`, "Deciding, then running").
    #[test]
    fn deciding_a_send_answers_what_to_run_without_running_it() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public foo = ( ^1 )
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate_unrooted(class, vec![]), "instantiate");
        match interp.decide_send(instance, Symbol::intern("foo"), &[]) {
            Ok(Step::Method { method, .. }) => assert_eq!(method.selector.as_str(), "foo"),
            _ => panic!("a user method is decided as an activation"),
        }
        assert_eq!(interp.heap.activation_depth(), 0, "nothing was pushed");
        match interp.decide_send(
            Value::SmallInt(3),
            Symbol::intern("+"),
            &[Value::SmallInt(4)],
        ) {
            Ok(Step::Value(Value::SmallInt(7))) => {}
            _ => panic!("a native leaf is decided as its value"),
        }
    }

    /// `do:` is decided as the activation of its block for the first
    /// element, with the rest of the loop in the continuation: nothing runs,
    /// and nothing is pushed (`docs/design/execution.md`, "No re-entry from
    /// Rust").
    #[test]
    fn deciding_do_answers_its_block_for_the_first_element() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public items = ( ^{10. 20. 30} )
              public echo = ( ^[:e | e] )
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate_unrooted(class, vec![]), "instantiate");
        let items = expect_ok(
            interp.send_unrooted(instance, Symbol::intern("items"), vec![]),
            "items",
        );
        let echo = expect_ok(
            interp.send_unrooted(instance, Symbol::intern("echo"), vec![]),
            "echo",
        );
        match interp.decide_send(items, Symbol::intern("do:"), &[echo]) {
            Ok(Step::Block {
                args: StepArgs::Owned(args),
                then: Continuation::DoElement(state),
                ..
            }) => {
                assert!(
                    matches!(args.as_slice(), [Value::SmallInt(10)]),
                    "the first element"
                );
                assert_eq!(state.next, 1, "the second element is next");
                assert_eq!(state.items.len(), 3, "the whole snapshot rides along");
            }
            _ => panic!("`do:` is decided as its block's first activation"),
        }
        assert_eq!(interp.heap.activation_depth(), 0, "nothing was pushed");
    }

    /// The nested-class accessor is decided as the activation of the
    /// superclass clause, with steps 3-5 in the continuation. Nothing is
    /// pushed, so no creation is in progress yet: one is marked only once its
    /// clause's activation is on the stack, and the cache slot is still
    /// `Nil`.
    #[test]
    fn deciding_a_nested_class_answers_its_superclass_clause_as_an_activation() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public class N = self base (
              ) (
              ) : ()
              base = ( ^Object )
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate_unrooted(class, vec![]), "instantiate");
        let slot = match interp.decide_send(instance, Symbol::intern("N"), &[]) {
            Ok(Step::SuperclassClause(creation)) => {
                assert!(
                    matches!(instance, Value::Obj(obj) if obj.same(creation.enclosing)),
                    "the clause runs on the enclosing object"
                );
                creation.index
            }
            _ => panic!("a nested class is decided as its clause's activation"),
        };
        assert_eq!(interp.heap.activation_depth(), 0, "nothing was pushed");
        let Value::Obj(obj) = instance else {
            panic!("an instance is an object");
        };
        assert!(
            matches!(interp.heap.instance_field(obj, slot), Some(Value::Nil)),
            "nothing is marked as in progress"
        );
        #[cfg(debug_assertions)]
        assert_eq!(interp.creations_in_progress, 0, "nothing is counted");
    }

    /// A factory send is decided as the instantiation's first body to run --
    /// a clause message with arguments on the way up, or else the
    /// initializer of the topmost level with code -- with the rest of the
    /// walk in the continuation when anything is left to run below it, and a
    /// plain delivery when nothing is (the initializer answers the
    /// instance); a class with nothing to initialize at any level is decided
    /// as its instance outright.
    #[test]
    fn deciding_new_answers_the_first_initializer_as_an_activation() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public class P = Object new (
                | v = 1. |
              ) (
              ) : ()
              public class Q = Object new (
              ) (
              ) : ()
              public class R = P new (
                | w = 2. |
              ) (
              ) : ()
              public class M x: a = Object new (
                | m = a. |
              ) (
              ) : ()
              public class S = M x: 3 (
              ) (
              ) : ()
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate_unrooted(class, vec![]), "instantiate");
        let p = expect_ok(
            interp.send_unrooted(instance, Symbol::intern("P"), vec![]),
            "P",
        );
        let q = expect_ok(
            interp.send_unrooted(instance, Symbol::intern("Q"), vec![]),
            "Q",
        );
        let r = expect_ok(
            interp.send_unrooted(instance, Symbol::intern("R"), vec![]),
            "R",
        );
        let s = expect_ok(
            interp.send_unrooted(instance, Symbol::intern("S"), vec![]),
            "S",
        );
        let Value::Obj(p_class) = p else {
            panic!("`P` answers a class object")
        };
        let Value::Obj(s_class) = s else {
            panic!("`S` answers a class object")
        };
        assert!(
            matches!(
                interp.decide_send(p, Symbol::NEW, &[]),
                Ok(Step::Initializer { level, .. }) if level.same(p_class)
            ),
            "`P new` is decided as P's initializer, which delivers"
        );
        match interp.decide_send(r, Symbol::NEW, &[]) {
            Ok(Step::InitializerThen {
                level, then: state, ..
            }) => {
                assert!(level.same(p_class));
                assert!(matches!(
                    state.phase,
                    InstantiationPhase::Descending { index: 1 }
                ));
            }
            _ => panic!("`R new` is decided as its superclass P's initializer, R's still to run"),
        }
        match interp.decide_send(s, Symbol::NEW, &[]) {
            Ok(Step::ClauseMessage { level, then, .. }) => {
                assert!(level.same(s_class));
                assert!(matches!(
                    then.phase,
                    InstantiationPhase::Climbing { level, .. } if level.same(s_class)
                ));
            }
            _ => panic!("`S new` is decided as its clause message's activation"),
        }
        assert!(
            matches!(
                interp.decide_send(q, Symbol::NEW, &[]),
                Ok(Step::Value(Value::Obj(_)))
            ),
            "`Q new` is decided as its instance"
        );
        assert_eq!(interp.heap.activation_depth(), 0, "nothing was pushed");
    }

    /// A finished instantiation leaves its walk's box among the spares for
    /// the next one, and leaves it empty: a spare holds no handle, so it
    /// keeps nothing alive and a collector need not trace it. The next
    /// instantiation walks in the same box.
    #[test]
    fn a_finished_walk_leaves_an_empty_box_for_the_next() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public class P x: a = Object new (
                | v = a. |
              ) (
              ) : ()
              public class R = P x: 1 (
                | w = 2. |
              ) (
              ) : ()
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate_unrooted(class, vec![]), "instantiate");
        let r = expect_ok(
            interp.send_unrooted(instance, Symbol::intern("R"), vec![]),
            "R",
        );
        let Value::Obj(r) = r else {
            panic!("`R` answers a class object")
        };
        expect_ok(interp.instantiate_unrooted(r, vec![]), "R new");
        assert_eq!(interp.spare_walks.len(), 1, "one walk ran at a time");
        let spare: *const Instantiation = &*interp.spare_walks[0];
        assert!(
            interp
                .spare_walks
                .iter()
                .all(|state| matches!(state.instance, Value::Nil) && state.levels.is_empty()),
            "a spare holds no handle"
        );
        expect_ok(interp.instantiate_unrooted(r, vec![]), "R new again");
        assert_eq!(interp.spare_walks.len(), 1);
        assert!(
            std::ptr::eq(spare, &*interp.spare_walks[0]),
            "the second walk ran in the first one's box"
        );
    }

    /// A walk that climbs through a clause message with arguments, and then
    /// finds no initializer at any level, retires straight out of
    /// `descend_levels`'s "nothing below" branch -- the branch that never
    /// writes `Descending` into `phase`. `retire_walk` must clear `phase`
    /// itself, or the retired box still holds the two `ClassRef`s of the
    /// last `Climbing` it was in, contradicting "a spare holds no handle".
    #[test]
    fn a_retired_walk_holds_no_classref_in_its_phase() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public class Base x: p = Object new (
              ) (
              ) : ()
              public class Cell = Base x: 1 (
              ) (
              ) : ()
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate_unrooted(class, vec![]), "instantiate");
        let cell = expect_ok(
            interp.send_unrooted(instance, Symbol::intern("Cell"), vec![]),
            "Cell",
        );
        let Value::Obj(cell) = cell else {
            panic!("`Cell` answers a class object")
        };
        expect_ok(interp.instantiate_unrooted(cell, vec![]), "Cell new");
        assert_eq!(interp.spare_walks.len(), 1, "one walk ran");
        let state = &interp.spare_walks[0];
        assert!(matches!(state.instance, Value::Nil), "no instance handle");
        assert!(state.levels.is_empty(), "no levels");
        assert!(
            matches!(state.phase, InstantiationPhase::Descending { .. }),
            "a retired box's phase holds no ClassRef either"
        );
    }

    /// Every decision answers a `Result<Step, Signal>`, so its size is paid
    /// on every send; a variant that grows it is a cost on every benchmark,
    /// not only on the path it serves (`StepArgs::ControlFlow`'s comment).
    /// This is the size the hot path was measured at.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_decision_stays_the_size_the_hot_path_was_measured_at() {
        assert_eq!(std::mem::size_of::<Result<Step, Signal>>(), 56);
    }
}
