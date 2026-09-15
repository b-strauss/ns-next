//! Sends, method invocation, and the `Signal` non-local-return/error channel,
//! shared by everything that runs Newspeak code. Implicit-receiver and
//! `super` sends are in `sends.rs`, register and outer-local access in
//! `locals.rs`, the errors a deferred literal raises in `literals.rs`; the
//! bootstrap and class instantiation are in `instantiate.rs`.

pub mod instantiate;
pub(crate) mod literals;
pub(crate) mod locals;
pub(crate) mod nested;
mod safepoint;
pub(crate) mod sends;
mod slots;

use std::rc::Rc;

use crate::compiler::Lowering;
use crate::dispatch::{self, Lookup};
use crate::hash::IdentityMap;
use crate::heap::{ActivationHeader, ActivationReference, Heap, Root};
use crate::types::*;
use crate::Symbol;

pub(crate) enum Signal<'gc> {
    Return(ActivationReference, Value<'gc>),
    Error(NewspeakError<'gc>),
}

pub(crate) type EvaluationResult<'gc> = Result<Value<'gc>, Signal<'gc>>;

/// How a host's send or instantiation ended (`Interpreter::send`,
/// `Interpreter::instantiate`): its value, rooted, or the signal that ended
/// it.
pub type HostResult = Result<Root, HostSignal>;

/// A signal that left a host's send or instantiation. No `Debug`, for the
/// reason `Signal` has none, so `unwrap` and `expect` on a `HostResult` do
/// not compile.
pub enum HostSignal {
    /// A Newspeak error nothing handled.
    Error(HostError),
    /// A `^` that reached the host: its home had returned, or it aimed at a
    /// running initializer, which catches none (`docs/design/execution.md`,
    /// "Non-local return").
    Return,
}

/// What deciding a host's send or instantiation left for the loop.
pub(crate) enum HostStart {
    /// Nothing to run: the answer, rooted.
    Finished(HostResult),
    /// An activation, pushed, for `vm::run`.
    Run(ActivationReference),
}

/// A Newspeak error as a host receives it: `NewspeakError` with its
/// receiver rooted, since the host may read it after running more code.
pub struct HostError {
    pub selector: Symbol,
    pub receiver: Root,
    pub message: String,
}

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
pub(crate) enum Step<'gc> {
    /// The send's value is already known: a native leaf, a slot accessor, a
    /// nested class, a finished instantiation.
    Value(Value<'gc>),
    /// Activate `method` on `receiver`; `defining_class` becomes the
    /// activation's current class (§3.6).
    Method {
        receiver: Value<'gc>,
        method: Rc<crate::mixin::MixinMethod>,
        defining_class: ClassReference<'gc>,
        arguments: StepArguments<'gc>,
    },
    /// Activate the closure object `closure`, and do `then` with its value.
    /// A builtin control-flow native's block is marked by its `arguments`,
    /// `StepArguments::ControlFlow`.
    Block {
        closure: ObjectReference<'gc>,
        arguments: StepArguments<'gc>,
        then: Continuation<'gc>,
    },
    /// Activate `level`'s instance initializer on `receiver`, and deliver its
    /// value: an instantiation's last initializer, whose value is the
    /// instance (`Mutator::descend_levels` says why). Its code and slot
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
    /// different niche, the effect `StepArguments::ControlFlow`'s comment
    /// describes.
    Initializer {
        receiver: Value<'gc>,
        level: ClassReference<'gc>,
        arguments: Vec<Value<'gc>>,
    },
    /// Activate `level`'s instance initializer on the instance `then` is
    /// initializing, and carry the walk on below it when it returns: an
    /// initializer with a level below it that still has code.
    InitializerThen {
        level: ClassReference<'gc>,
        arguments: Vec<Value<'gc>>,
        then: Box<Instantiation<'gc>>,
    },
    /// Activate `code`, the arguments of `level`'s superclass clause message,
    /// on the instance `then` is initializing and with `level`'s factory
    /// arguments (§6.2); its value is the superclass's arguments, with which
    /// `then` carries the walk on.
    ClauseMessage {
        level: ClassReference<'gc>,
        code: Rc<crate::bytecode::CompiledCode>,
        arguments: Vec<Value<'gc>>,
        then: Box<Instantiation<'gc>>,
    },
    /// Activate the superclass clause of the nested class `creation`
    /// describes, on its enclosing object and as part of the class that
    /// declares it; its value is the superclass, from which the class is
    /// built, cached and answered (`nested::after_clause`).
    SuperclassClause(Box<NestedClassCreation<'gc>>),
}

/// Where an activation's arguments come from.
pub(crate) enum StepArguments<'gc> {
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
    Owned(Vec<Value<'gc>>),
}

impl<'gc> StepArguments<'gc> {
    pub(crate) fn slice<'a>(&'a self, send: &'a [Value<'gc>]) -> &'a [Value<'gc>] {
        match self {
            StepArguments::Send => send,
            StepArguments::ControlFlow => &[],
            StepArguments::Owned(arguments) => arguments,
        }
    }
}

/// What a lookup answered once any primitive it found has run.
pub(crate) enum Settled<'gc> {
    /// A primitive ran and answered.
    Answered(Result<Step<'gc>, Signal<'gc>>),
    /// The lookup's answer; never `Lookup::Primitive`.
    Found(Lookup<'gc>),
}

/// Everything a step of the interpreter works on -- the heap and the
/// bootstrap's builtin classes, the caches of behaviour, the argument
/// buffer and the spare instantiation boxes -- and every decision, native
/// and continuation that runs one. A host reaches it only through
/// `Interpreter::enter`, and nothing here reaches back: no method takes or
/// answers the `Interpreter`, so no step can run the loop, reach its
/// safepoint, or start a host's send (`docs/design/memory.md`, "The
/// brand").
pub struct Mutator<'gc> {
    pub(crate) heap: Heap<'gc>,
    /// Every builtin class object, indexed by `BuiltinClass as usize`.
    /// Indexed rather than hashed because `class_of` asks for one on every
    /// send to a primitive receiver. Empty only while `bootstrap` is
    /// building it; nothing can reach a `Mutator` in that state.
    pub(crate) builtin_classes: Vec<ClassReference<'gc>>,
    pub(crate) platform_singleton: Option<Value<'gc>>,
    /// (mixin identity, superclass application identity) -> behaviour.
    /// Keyed on the superclass's *application*, never on a superclass class
    /// object: a nested superclass has one class object per enclosing
    /// instance, so a class-object key would never hit and every instance
    /// would get its own behaviour.
    pub(crate) applications: IdentityMap<(usize, usize), Rc<crate::mixin::MixinApplication>>,
    /// mixin identity -> the synthetic empty class side `metaclass_of`
    /// stands in for that mixin's own level when it declares none.
    ///
    /// Cached, not minted per call, because the mixin's `Rc` identity is
    /// what `applications` keys on: a fresh one per call would give two
    /// class objects that share an instance-side application two *different*
    /// metaclass applications, and the shared-application property is
    /// exactly what the inline caches will guard on. It would
    /// also grow `applications` without bound.
    pub(crate) empty_class_sides: IdentityMap<usize, Rc<crate::mixin::Mixin>>,
    /// How many nested classes are marked as being created
    /// (`nested::begin_creation`), for `run_from_host`'s check that none
    /// outlives the run that began it. The mark itself lives in the class's
    /// cache slot; this only counts it, so a release build keeps nothing.
    #[cfg(debug_assertions)]
    pub(crate) creations_in_progress: usize,
    /// Boxes finished instantiations left behind, emptied, for the next ones
    /// to walk in (`instantiate.rs`'s `walk_state` and `retire_walk`).
    /// Allocating a box and a list of levels per instantiation instead
    /// measured 3-13% slower on instantiation benchmarks. They hold no
    /// handle, so a collector has nothing to trace in them.
    #[allow(clippy::vec_box)] // the boxes are what is reused, not only their contents
    pub(crate) spare_walks: Vec<Box<Instantiation<'gc>>>,
    /// The lowering every module this interpreter loads is compiled with
    /// (`docs/design/compiler.md`, "The reference lowering"). Fixed at
    /// construction, so one interpreter never mixes the two.
    pub(crate) lowering: Lowering,
    /// The reusable argument buffer the run loop gathers a send's arguments
    /// into (`vm::send`). Taken while a send is decided and pushed and given
    /// back afterwards, because the decision needs `&mut Mutator` while
    /// it reads the arguments. Empty between instructions: it is cleared
    /// when it is given back, so it holds no handle a move would have to
    /// find.
    pub(crate) argument_buffer: Vec<Value<'gc>>,
}

/// A host's interpreter: the step state at rest (`World`), and what only
/// the run loop between steps needs -- whether a run is in progress, and
/// the stress mode's schedule. Its methods are the host's entries: `enter`
/// for a step's worth of reading and allocating, and `send`,
/// `instantiate`, `load_module` and `platform`, which take and answer
/// roots.
pub struct Interpreter {
    world: crate::heap::World,
    /// Whether a host's run of the loop is in progress (`run_from_host`).
    /// Nothing the loop calls can start another -- no decision, native or
    /// continuation runs code from Rust -- and a debug build checks that.
    running: bool,
    /// The stress mode's schedule, when it is on (`NS_NEXT_STRESS_MOVES`, or
    /// `set_stress_moves`). Debug builds only.
    #[cfg(debug_assertions)]
    stress: Option<safepoint::StressMoves>,
}

impl Interpreter {
    pub fn bootstrap() -> Interpreter {
        Interpreter::bootstrap_with(Lowering::Optimized)
    }

    /// An interpreter whose `load_module` compiles with `lowering`. The
    /// reference lowering is for checking the optimized one against it; an
    /// artifact decoded into either runs the code it carries.
    pub fn bootstrap_with(lowering: Lowering) -> Interpreter {
        // Only the debug build's stress mode writes to it below.
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut interpreter = Interpreter {
            world: crate::heap::World::new(lowering),
            running: false,
            #[cfg(debug_assertions)]
            stress: safepoint::StressMoves::from_env(),
        };
        #[cfg(debug_assertions)]
        {
            let stress_enabled = interpreter.stress.is_some();
            interpreter.world.set_safepoint_requested(stress_enabled);
        }
        interpreter
    }

    /// Runs `body` with the interpreter's step state: its heap, its builtin
    /// classes, and every method a step uses. What `body` answers is what this
    /// answers. `body` runs no Newspeak code; a host runs code with `send` and
    /// `instantiate`, outside every entry.
    ///
    /// Every handle `body` reaches carries the lifetime `'gc`, which `body` is
    /// generic over and `R` cannot name, so no handle leaves an entry: a run
    /// may move every object, and a handle kept across one would name
    /// whatever took its slot (`docs/design/memory.md`, "The brand"). A host
    /// keeps a value across a run as a `Root`, and reads it in the next
    /// entry:
    ///
    /// ```
    /// use ns_runtime::{Interpreter, Symbol, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let root = interpreter.enter(|mutator| {
    ///     let array = mutator.new_array(vec![Value::SmallInteger(1)]);
    ///     mutator.stash(Value::Object(array))
    /// });
    /// let _ = interpreter.send(&root, Symbol::intern("size"), &[]);
    /// let length = interpreter.enter(|mutator| match mutator.fetch(&root) {
    ///     Value::Object(array) => mutator.heap().array_length(array),
    ///     _ => None,
    /// });
    /// assert_eq!(length, Some(1));
    /// ```
    ///
    /// Keeping the handle itself instead does not compile:
    ///
    /// ```compile_fail
    /// use ns_runtime::{Interpreter, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let array = interpreter.enter(|mutator| mutator.new_array(vec![Value::SmallInteger(1)]));
    /// ```
    ///
    /// Nor does storing one where a host keeps its own state; a host's
    /// field holds a `Root`:
    ///
    /// ```
    /// use ns_runtime::{Interpreter, Root, Value};
    /// struct Host { kept: Option<Root> }
    /// let mut interpreter = Interpreter::bootstrap();
    /// let mut host = Host { kept: None };
    /// interpreter.enter(|mutator| {
    ///     let array = mutator.new_array(vec![]);
    ///     host.kept = Some(mutator.stash(Value::Object(array)));
    /// });
    /// assert!(host.kept.is_some());
    /// ```
    ///
    /// ```compile_fail
    /// use ns_runtime::{Interpreter, Value};
    /// struct Host<'a> { kept: Option<Value<'a>> }
    /// let mut interpreter = Interpreter::bootstrap();
    /// let mut host = Host { kept: None };
    /// interpreter.enter(|mutator| {
    ///     let array = mutator.new_array(vec![]);
    ///     host.kept = Some(Value::Object(array));
    /// });
    /// ```
    ///
    /// And a step cannot run the loop: the interpreter is borrowed by the
    /// entry for as long as `body` runs, and `Mutator` offers no way back to
    /// it, so a native, which is handed only a `Mutator`, cannot re-enter
    /// the loop either. A host runs code between entries:
    ///
    /// ```
    /// use ns_runtime::{Interpreter, Symbol, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let three = interpreter.stash(Value::SmallInteger(3));
    /// let printed = interpreter.send(&three, Symbol::intern("printString"), &[]);
    /// assert!(printed.is_ok());
    /// ```
    ///
    /// ```compile_fail
    /// use ns_runtime::{Interpreter, Symbol, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let three = interpreter.stash(Value::SmallInteger(3));
    /// interpreter.enter(|_mutator| {
    ///     let _ = interpreter.send(&three, Symbol::intern("printString"), &[]);
    /// });
    /// ```
    #[inline(always)]
    pub fn enter<R>(&mut self, body: impl for<'gc> FnOnce(&mut Mutator<'gc>) -> R) -> R {
        self.world.enter(body)
    }

    /// Roots an immediate -- `nil`, a Boolean, a `SmallInteger`, a `Float`
    /// -- a host built itself, outside any entry. An object is rooted with
    /// `Mutator::stash`, inside the entry that reached it. A host can build
    /// an immediate but never hold an object handle outside an entry, so a
    /// `Value<'static>` holds none.
    pub fn stash(&self, value: Value<'static>) -> Root {
        self.world.stash(value)
    }

    /// Whether the run loop's next safepoint must take its slow path.
    #[inline(always)]
    pub(crate) fn safepoint_requested(&self) -> bool {
        self.world.safepoint_requested()
    }

    /// An ordinary (explicit-receiver) send from a host: `newspeak-spec.pdf`
    /// §5.5, decided as `decide_send` decides it and run from the outside
    /// (`run_from_host`). A `Send*` the VM issues never comes through here:
    /// `vm.rs` decides it and pushes what was decided itself.
    pub fn send(&mut self, receiver: &Root, selector: Symbol, arguments: &[Root]) -> HostResult {
        let depth = self.world.activation_depth();
        let start = self.enter(|mutator| {
            let receiver = mutator.fetch(receiver);
            let arguments: Vec<Value> = arguments
                .iter()
                .map(|argument| mutator.fetch(argument))
                .collect();
            let decided = mutator.decide_send(receiver, selector, &arguments);
            mutator.start(decided, &arguments)
        });
        self.run_from_host(start, depth)
    }

    /// Runs what a host's send or instantiation started. It is the only way
    /// Rust enters the run loop (`docs/design/execution.md`, "No re-entry
    /// from Rust"). `depth` is the stack's depth before the host's entry
    /// decided anything.
    ///
    /// Three things are checked in a debug build, because each is otherwise
    /// silent. The loop is never entered while it is running -- nothing it
    /// calls can reach here. The stack is left as deep as it was found: a
    /// leaked entry would put a later activation on top of a dead one's
    /// registers (a leak inside a run is caught too, by
    /// `Heap::pop_activation`'s check that it pops the top). And no nested
    /// class's creation outlives the run that began it.
    fn run_from_host(&mut self, start: HostStart, depth: usize) -> HostResult {
        debug_assert!(
            !self.running,
            "internal: the run loop was re-entered from Rust"
        );
        let result = match start {
            HostStart::Finished(result) => result,
            HostStart::Run(activation) => {
                self.running = true;
                let result = crate::vm::run(self, activation);
                self.running = false;
                result
            }
        };
        debug_assert_eq!(
            self.world.activation_depth(),
            depth,
            "a host's send or instantiation must leave the activation stack as it found it"
        );
        #[cfg(debug_assertions)]
        assert_eq!(
            self.world.creations_in_progress(),
            0,
            "internal: a nested class's creation outlived the run that began it"
        );
        result
    }

    /// The `platform` object, rooted for a host.
    pub fn platform(&mut self) -> Root {
        self.enter(|mutator| {
            let platform = mutator.platform_value();
            mutator.stash(platform)
        })
    }
}

impl<'gc> Mutator<'gc> {
    pub fn builtin_class(&self, kind: BuiltinClass) -> ClassReference<'gc> {
        self.builtin_classes[kind as usize]
    }

    /// Builds every builtin class into an empty step state. `World::new`
    /// calls it once, in the first entry the world makes.
    pub(crate) fn bootstrap_builtin_classes(&mut self) {
        self.builtin_classes = build_builtin_classes(self);
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

    /// The heap, for reading, inside an entry.
    pub fn heap(&self) -> &Heap<'gc> {
        &self.heap
    }

    /// The heap, for writing, inside an entry.
    pub fn heap_mut(&mut self) -> &mut Heap<'gc> {
        &mut self.heap
    }

    /// What `root` holds now. Good until this entry ends: the next run may
    /// move it, so read the root again in the next entry.
    pub fn fetch(&self, root: &Root) -> Value<'gc> {
        self.heap.fetch(root)
    }

    /// Roots `value`, so that a host can hold it past this entry.
    pub fn stash(&self, value: Value<'gc>) -> Root {
        self.heap.stash(value)
    }

    /// Pushes what a host's send or instantiation decided, in the entry
    /// that decided it: `send_arguments` are the send's arguments, which
    /// `StepArguments::Send` names and which do not outlive the entry. A value,
    /// a signal, or a push refused at the depth limit finishes here, rooted
    /// for the host.
    pub(crate) fn start(
        &mut self,
        decided: Result<Step<'gc>, Signal<'gc>>,
        send_arguments: &[Value<'gc>],
    ) -> HostStart {
        match decided {
            Ok(Step::Value(value)) => HostStart::Finished(self.to_host(Ok(value))),
            Ok(step) => match self.push_step(step, send_arguments, None, false) {
                Ok(activation) => HostStart::Run(activation),
                Err(signal) => HostStart::Finished(self.to_host(Err(signal))),
            },
            Err(signal) => HostStart::Finished(self.to_host(Err(signal))),
        }
    }

    /// A run's outcome as the host receives it. Nothing runs between the
    /// run's last safepoint and here, so every value in `result` is current.
    pub(crate) fn to_host(&self, result: EvaluationResult<'gc>) -> HostResult {
        match result {
            Ok(value) => Ok(self.stash(value)),
            Err(Signal::Error(error)) => Err(HostSignal::Error(HostError {
                selector: error.selector,
                receiver: self.stash(error.receiver),
                message: error.message,
            })),
            Err(Signal::Return(..)) => Err(HostSignal::Return),
        }
    }

    /// Decides what an ordinary (explicit-receiver) send would run, without
    /// running it (`docs/design/execution.md`, "Deciding, then running").
    ///
    /// Checked first, before any lookup: `ClassName new` (or a class's own
    /// non-default factory selector) never has a real method to find --
    /// every class object's own `.class` is a shared placeholder whose
    /// method table holds only `Object`'s primitives, none of them a factory
    /// (see `build_builtin_classes` below), so an ordinary `lookup_public`
    /// search would just fall through to `doesNotUnderstand:` every time.
    /// `try_factory_send` (`instantiate.rs`) recognizes the shape and decides
    /// it as an instantiation (`decide_instantiation`) -- not through the host entry
    /// `instantiate`, which would enter the run loop from inside it, the
    /// re-entry `run_from_host` refuses; see its own doc comment for why this can
    /// never double-fire against the superclass clause's own deferred
    /// factory message (`climb_levels` never decides a send).
    pub(crate) fn decide_send(
        &mut self,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        if let Some(result) = self.try_factory_send(&receiver, selector, arguments) {
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
        if let Value::Object(object) = &receiver {
            let is_class = self.heap.is_class(*object);
            if is_class {
                if let Some(metaclass) = self.metaclass_of(*object) {
                    let lookup = dispatch::lookup_public(&self.heap, metaclass, selector);
                    // No metaclass chain holds a primitive -- a metaclass's
                    // mixins are class sides or empty stand-ins, never a
                    // builtin's instance mixin -- so `Answered` is reached
                    // only in principle; it is settled like every other
                    // lookup so that nothing here depends on that.
                    match self.settle(
                        dispatch::Rule::Public,
                        lookup,
                        receiver,
                        selector,
                        arguments,
                    ) {
                        Settled::Answered(result) => return result,
                        Settled::Found(Lookup::Method(method, defining_class)) => {
                            return Ok(Step::Method {
                                receiver,
                                method,
                                defining_class,
                                arguments: StepArguments::Send,
                            });
                        }
                        Settled::Found(Lookup::SlotAccessor(accessor, field)) => {
                            return self.decide_slot_accessor(receiver, accessor, field, arguments);
                        }
                        Settled::Found(Lookup::NestedClass(mixin, declaring_class)) => {
                            return nested::nested_class(self, &receiver, &mixin, declaring_class);
                        }
                        Settled::Found(Lookup::NotFound) => {}
                        Settled::Found(Lookup::Primitive(..)) => {
                            unreachable!("internal: `settle` runs every primitive it is handed")
                        }
                    }
                }
            }
        }
        let class = self.class_of(&receiver);
        let lookup = dispatch::lookup_public(&self.heap, class, selector);
        match self.settle(
            dispatch::Rule::Public,
            lookup,
            receiver,
            selector,
            arguments,
        ) {
            Settled::Answered(result) => result,
            Settled::Found(Lookup::Method(method, defining_class)) => Ok(Step::Method {
                receiver,
                method,
                defining_class,
                arguments: StepArguments::Send,
            }),
            // §6.3.2: a slot's accessors answer an explicit-receiver send
            // exactly when a method with the slot's accessibility would.
            Settled::Found(Lookup::SlotAccessor(accessor, field)) => {
                self.decide_slot_accessor(receiver, accessor, field, arguments)
            }
            // §6: the implicit accessor a nested declaration defines is
            // reached exactly like any other member, and it ignores `arguments`
            // -- the accessor is unary, so a send that supplied arguments
            // did not name it (`walk` matched on the bare selector).
            Settled::Found(Lookup::NestedClass(mixin, declaring_class)) => {
                nested::nested_class(self, &receiver, &mixin, declaring_class)
            }
            Settled::Found(Lookup::NotFound) => {
                self.decide_does_not_understand(receiver, selector, arguments)
            }
            Settled::Found(Lookup::Primitive(..)) => {
                unreachable!("internal: `settle` runs every primitive it is handed")
            }
        }
    }

    /// Runs the primitive `lookup` found, if it found one. A primitive fails
    /// when its receiver has the wrong shape -- an instance of a user subclass
    /// of `String`, which carries no string -- and a failure passes its level
    /// over: the lookup continues under the same `rule` from that level's
    /// superclass, `Object` for every builtin but `Object` itself, which has
    /// none, and what it finds there is settled the same way. So a send
    /// whose builtin primitive fails is answered by `Object`'s primitive of
    /// the same selector if there is one, and otherwise by the caller's own
    /// "not found" -- `doesNotUnderstand:` (`docs/design/dispatch.md`, "A
    /// primitive can fail"). The loop ends: each turn moves at least one
    /// level up a superclass chain that `image::decode` refuses to let form
    /// a cycle.
    pub(crate) fn settle(
        &mut self,
        rule: dispatch::Rule,
        mut lookup: Lookup<'gc>,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Settled<'gc> {
        loop {
            match lookup {
                Lookup::Primitive(primitive, level) => {
                    if let Some(result) =
                        crate::builtins::execute(self, primitive, receiver, selector, arguments)
                    {
                        return Settled::Answered(result);
                    }
                    lookup = dispatch::lookup_above(&self.heap, rule, level, selector);
                }
                found => return Settled::Found(found),
            }
        }
    }

    /// §5.5's `doesNotUnderstand:` fallback -- access-blind (`lookup_defined_for`),
    /// per `docs/design/dispatch.md`.
    ///
    /// A user-overridden `doesNotUnderstand:` arrives as `Lookup::Method`;
    /// `Object`'s own primitive one, which every class that does not
    /// override it inherits, arrives as `Lookup::Primitive` and runs through
    /// `settle` like any other primitive.
    fn decide_does_not_understand(
        &mut self,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let class = self.class_of(&receiver);
        self.decide_does_not_understand_in(receiver, class, selector, arguments)
    }

    /// §5.10's failure: "Otherwise, the #doesNotUnderstand: method defined
    /// for S is invoked" -- looked up from `superclass`, S, rather than from
    /// the receiver's own class, so a subclass's own `doesNotUnderstand:`
    /// does not answer a `super` send that its superclass cannot. A class
    /// object's `super` send of its own factory selector still instantiates
    /// (`try_factory_send`), exactly as the same send to it does.
    pub(crate) fn decide_super_does_not_understand(
        &mut self,
        receiver: Value<'gc>,
        superclass: ClassReference<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        if let Some(result) = self.try_factory_send(&receiver, selector, arguments) {
            return result;
        }
        self.decide_does_not_understand_in(receiver, superclass, selector, arguments)
    }

    /// `doesNotUnderstand:` for `receiver`, looked up from `class`.
    fn decide_does_not_understand_in(
        &mut self,
        receiver: Value<'gc>,
        class: ClassReference<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        // A minimal message-mirror-shaped value: an Array `{selector.
        // arguments}` rather than a real MessageMirror object, since mirrors do
        // not exist yet (`docs/design/liveness-and-reflection.md`, "Mirrors and
        // MirrorBuilder"). Callers relying on a real `MessageMirror` API are
        // out of scope. Built before the lookup, since every handler takes
        // it; allocation order is unobservable, identity hashes being drawn
        // lazily.
        let selector_value = self.new_symbol(selector);
        let arguments_value = Value::Object(self.new_array(arguments.to_vec()));
        let message_argument = Value::Object(self.new_array(vec![selector_value, arguments_value]));
        let lookup = dispatch::lookup_defined_for(&self.heap, class, Symbol::DOES_NOT_UNDERSTAND);
        match self.settle(
            dispatch::Rule::DefinedFor,
            lookup,
            receiver,
            Symbol::DOES_NOT_UNDERSTAND,
            &[message_argument],
        ) {
            Settled::Answered(result) => result,
            Settled::Found(Lookup::Method(method, defining_class)) => Ok(Step::Method {
                receiver,
                method,
                defining_class,
                arguments: StepArguments::Owned(vec![message_argument]),
            }),
            // §6.3.2: a mutable slot named `doesNotUnderstand` defines the
            // setter `doesNotUnderstand:`, a method like any other -- so it
            // is the handler, and stores the message.
            Settled::Found(Lookup::SlotAccessor(accessor, field)) => {
                self.decide_slot_accessor(receiver, accessor, field, &[message_argument])
            }
            // A nested declaration can never be the `doesNotUnderstand:`
            // handler: `walk` matches a declaration by name, and a class
            // name is never a keyword selector. Folded into `NotFound`
            // rather than given an arm of its own so the unreachable case
            // still reports the ordinary "nothing handles this" error
            // instead of a second, differently-worded one.
            Settled::Found(Lookup::NotFound | Lookup::NestedClass(..)) => Err(Signal::Error(NewspeakError {
                selector,
                receiver,
                message: format!("doesNotUnderstand: {selector} (and no doesNotUnderstand: found anywhere in the class chain)"),
            })),
            Settled::Found(Lookup::Primitive(..)) => {
                unreachable!("internal: `settle` runs every primitive it is handed")
            }
        }
    }

    pub(crate) fn take_argument_buffer(&mut self) -> Vec<Value<'gc>> {
        std::mem::take(&mut self.argument_buffer)
    }

    /// Cleared here rather than when it is taken, so that between two
    /// instructions -- at every safepoint -- the buffer holds no value, and
    /// so no handle a move would have to find.
    pub(crate) fn return_argument_buffer(&mut self, mut buffer: Vec<Value<'gc>>) {
        buffer.clear();
        self.argument_buffer = buffer;
    }

    /// Pushes an activation, refusing one that would take the counted depth
    /// past `MAX_ACTIVATION_DEPTH`. Every activation in the runtime is pushed
    /// here, so the count and the check cannot be skipped by a new site.
    pub(crate) fn push(
        &mut self,
        header: ActivationHeader<'gc>,
        arguments: &[Value<'gc>],
        temporaries: usize,
    ) -> Result<ActivationReference, Signal<'gc>> {
        if header.counted && self.heap.counted_depth() >= MAX_ACTIVATION_DEPTH {
            return Err(recursion_too_deep(&header));
        }
        Ok(self.heap.push_activation(header, arguments, temporaries))
    }

    /// Pushes the activation `step` describes. `send_arguments` are the
    /// arguments of the send that decided it, which `StepArguments::Send`
    /// refers to; `sender` is the activation that sent it, if the run loop did,
    /// and `inlinable` whether the send was a `SendInlinable` -- the two things
    /// the depth count's one exemption asks about.
    pub(crate) fn push_step(
        &mut self,
        step: Step<'gc>,
        send_arguments: &[Value<'gc>],
        sender: Option<ActivationReference>,
        inlinable: bool,
    ) -> Result<ActivationReference, Signal<'gc>> {
        match step {
            Step::Value(_) => unreachable!("internal: a value needs no activation"),
            Step::Method {
                receiver,
                method,
                defining_class,
                arguments,
            } => {
                let arguments = arguments.slice(send_arguments);
                check_method_arity(&method, receiver, arguments.len())?;
                let code = method.code.clone();
                let temporaries = (code.register_count - code.argument_count) as usize;
                self.push(
                    ActivationHeader {
                        receiver,
                        current_class: Some(defining_class),
                        kind: FrameKind::Method(method),
                        slot_base: 0, // a method body never contains InitializeSlot
                        code,
                        then: Continuation::Deliver,
                        counted: true,
                    },
                    arguments,
                    temporaries,
                )
            }
            // A closure activation catches no return: its `^` targets its home.
            Step::Block {
                closure,
                arguments,
                then,
            } => {
                let control_flow = matches!(arguments, StepArguments::ControlFlow);
                let arguments = arguments.slice(send_arguments);
                let (code, parameter_count, captured_frame) = self
                    .heap
                    .closure_activation(closure)
                    .expect("internal: a block step is decided only for a closure object");
                check_block_arity(&self.heap, parameter_count, captured_frame, arguments.len())?;
                // Exempt exactly the activation the optimized lowering
                // inlines away: a block a control-flow native activates for
                // a `SendInlinable`, and only if the sending activation
                // created it -- which every block the reference lowering
                // passes to one is, and which keeps a crafted artifact from
                // recursing uncounted (`docs/design/compiler.md`, "How the
                // limit counts"). Asked in this order so that an ordinary
                // `value` never reads the sender's entry.
                let created_by_sender = |heap: &Heap<'gc>| {
                    sender
                        .and_then(|sender| heap.activation_reified(sender))
                        .is_some_and(|frame| frame.same(captured_frame))
                };
                let counted = !(inlinable && control_flow && created_by_sender(&self.heap));
                let receiver = self.heap.frame_receiver(captured_frame);
                let current_class = self.heap.frame_current_class(captured_frame);
                let temporaries = (code.register_count - code.argument_count) as usize;
                self.push(
                    ActivationHeader {
                        receiver,
                        current_class,
                        kind: FrameKind::Closure(closure),
                        slot_base: 0, // a closure body never contains InitializeSlot
                        code,
                        then,
                        counted,
                    },
                    arguments,
                    temporaries,
                )
            }
            // An initializer level, a superclass clause or its message is
            // never an activation the optimized lowering inlines away, so
            // each of the three below is always counted.
            Step::Initializer {
                receiver,
                level,
                arguments,
            } => self.push_initializer(receiver, level, &arguments, Continuation::Deliver),
            Step::InitializerThen {
                level,
                arguments,
                then,
            } => self.push_initializer(
                then.instance,
                level,
                &arguments,
                Continuation::Instantiate(then),
            ),
            Step::ClauseMessage {
                level,
                code,
                arguments,
                then,
            } => {
                let temporaries = (code.register_count - code.argument_count) as usize;
                self.push(
                    ActivationHeader {
                        receiver: then.instance,
                        current_class: Some(level),
                        kind: FrameKind::SuperclassMessage(level),
                        slot_base: 0, // a clause message never contains InitializeSlot
                        code,
                        then: Continuation::Instantiate(then),
                        counted: true,
                    },
                    &arguments,
                    temporaries,
                )
            }
            Step::SuperclassClause(creation) => {
                let code = nested::superclass_clause_code(&creation.nested);
                let temporaries = (code.register_count - code.argument_count) as usize;
                let (enclosing, index) = (creation.enclosing, creation.index);
                let activation = self.push(
                    ActivationHeader {
                        receiver: Value::Object(enclosing),
                        current_class: Some(creation.declaring_class),
                        kind: FrameKind::SuperclassClause(creation.declaring_class),
                        slot_base: 0, // a clause never contains InitializeSlot
                        code,
                        then: Continuation::NestedClass(creation),
                        counted: true,
                    },
                    &[],
                    temporaries,
                )?;
                // A nested class's creation is in progress exactly while its
                // clause's activation is on the stack: marked once the push
                // has succeeded, and unmarked by `resume` or `abandon` when
                // the activation is popped. A push refused at the depth limit
                // marks nothing.
                nested::begin_creation(self, enclosing, index);
                Ok(activation)
            }
        }
    }

    /// Pushes `level`'s instance initializer on `receiver`, the one body
    /// `Step::Initializer` and `Step::InitializerThen` share; only what
    /// happens once it returns differs.
    fn push_initializer(
        &mut self,
        receiver: Value<'gc>,
        level: ClassReference<'gc>,
        arguments: &[Value<'gc>],
        then: Continuation<'gc>,
    ) -> Result<ActivationReference, Signal<'gc>> {
        let (code, slot_base) = self.initializer_body(level);
        let temporaries = (code.register_count - code.argument_count) as usize;
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
            arguments,
            temporaries,
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
    pub(crate) fn resume(
        &mut self,
        then: Continuation<'gc>,
        value: Value<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        match then {
            Continuation::Deliver => Ok(Step::Value(value)),
            Continuation::Truthy => Ok(Step::Value(Value::Boolean(
                crate::builtins::boolean::truthy(value),
            ))),
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
    pub(crate) fn abandon(&mut self, then: Continuation<'gc>) {
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
        step: Step<'gc>,
        replaced_counted: bool,
    ) -> Result<ActivationReference, Signal<'gc>> {
        let sender = self.heap.top_activation();
        // A continuation's step never refers to a send's arguments
        // (`StepArguments::Send` is only answered by a decision about a send,
        // and the one a continuation makes -- `mustBeBoolean` -- has none), so
        // `&[]` is always right here.
        self.push_step(step, &[], sender, !replaced_counted)
    }

    /// Whether a `^` aimed at `activation` ends at `activation`: a method's
    /// does, and a superclass clause's or its message's. An initializer catches
    /// none -- a `^` into a running one escapes, the known gap in
    /// `docs/design/execution.md`, "Non-local return" -- and a block's `^`
    /// always targets its home, never the block.
    pub(crate) fn catches_return(&self, activation: ActivationReference) -> bool {
        matches!(
            self.heap.activation_kind(activation),
            FrameKind::Method(_) | FrameKind::SuperclassClause(_) | FrameKind::SuperclassMessage(_)
        )
    }

    /// Where a `^` in `activation` returns to. A block returns from its home --
    /// the method or initializer that lexically encloses it -- and only
    /// while that activation is still running: its reified frame is married
    /// to a live stack entry exactly until the activation returns, and a
    /// frame the image decoder built never is. A dead home is §7.2's
    /// `cannotReturn`. Every other activation's `^` returns from itself.
    pub(crate) fn raise_return(
        &mut self,
        activation: ActivationReference,
        value: Value<'gc>,
    ) -> Signal<'gc> {
        let target = match self.heap.activation_home(activation) {
            None => activation,
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

    pub fn class_of(&self, value: &Value<'gc>) -> ClassReference<'gc> {
        match value {
            Value::Object(object) => self.heap.class_of_object(*object),
            // Nil/Boolean/SmallInteger/Float classes are wired up by the
            // bootstrap into interpreter-held globals, read from there via
            // `Mutator::builtin_class(kind)`.
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
    /// does, an empty synthetic mixin (`empty_class_side_mixin`) stands in
    /// for `class`'s own level -- synthesized, as `builtin_mixin` below
    /// synthesizes a builtin's, since no declaration exists to build it
    /// from. It contributes no methods, only relays the superclass's metaclass
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
    pub fn metaclass_of(&mut self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        // Walk out to the first level that already answers -- a cached
        // metaclass, a non-class object, or the end of the chain -- pushing
        // each level that still has to be built. `chain` is nearest-first,
        // so the build below runs it in reverse.
        let mut chain: Vec<ClassReference> = Vec::new();
        let mut super_metaclass: Option<ClassReference> = None;
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

    fn builtin_class_for_primitive(&self, value: &Value<'gc>) -> ClassReference<'gc> {
        let kind = match value {
            Value::Nil => BuiltinClass::Nil,
            Value::Boolean(true) => BuiltinClass::True,
            Value::Boolean(false) => BuiltinClass::False,
            Value::SmallInteger(_) => BuiltinClass::Integer,
            Value::Float(_) => BuiltinClass::Float,
            Value::Object(_) => unreachable!("Value::Object carries its own .class"),
        };
        self.builtin_class(kind)
    }

    fn new_instance_object(
        &mut self,
        class: ClassReference<'gc>,
        data: ObjectData<'gc>,
    ) -> ObjectReference<'gc> {
        self.heap.allocate_object(class, data)
    }

    pub fn new_array(&mut self, elements: Vec<Value<'gc>>) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Array);
        self.new_instance_object(class, ObjectData::Array(elements))
    }

    pub fn new_string(&mut self, string: String) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::String);
        self.new_instance_object(class, ObjectData::String(string))
    }

    pub fn new_symbol(&mut self, symbol: Symbol) -> Value<'gc> {
        let class = self.builtin_class(BuiltinClass::Symbol);
        Value::Object(self.new_instance_object(class, ObjectData::Symbol(symbol)))
    }

    pub fn new_character(&mut self, character: char) -> ObjectReference<'gc> {
        // There is no dedicated Character payload variant on
        // ObjectData -- represent a character as a one-`char` Rust String
        // wrapped the same way String literals are (String), consistent with
        // how the language spec itself treats characters as "a special
        // case of string literals" going forward (`newspeak-spec.pdf`
        // §2.2.3). `builtins::text` tags the *class* as
        // `BuiltinClass::Character` so dispatch still lands on the right
        // native methods; only the payload representation is shared.
        let class = self.builtin_class(BuiltinClass::Character);
        self.new_instance_object(class, ObjectData::String(character.to_string()))
    }

    pub fn new_closure(&mut self, closure: ClosureData<'gc>) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Block);
        self.new_instance_object(class, ObjectData::Closure(closure))
    }

    /// The `platform` object (`docs/design/core-library.md`):
    /// memoized on first call, so every reference within one interpreter
    /// sees the same object identity.
    pub(crate) fn platform_value(&mut self) -> Value<'gc> {
        if let Some(platform) = &self.platform_singleton {
            return *platform;
        }
        let class = self.builtin_class(BuiltinClass::Platform);
        let object = self.new_instance_object(class, ObjectData::Instance(vec![]));
        let value = Value::Object(object);
        self.platform_singleton = Some(value);
        value
    }

    /// A `Transcript`-style stdout stream object -- see `crate::platform`'s
    /// module doc for how its payload shape (a one-field, always-`Nil`
    /// `Instance`) tells it apart from the `platform` singleton itself
    /// (a zero-field `Instance`). `pub(crate)`, not `pub`: only
    /// `crate::platform::execute` (`platform transcript`) ever needs to
    /// mint one.
    pub(crate) fn new_stream_object(&mut self) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Platform);
        self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil]))
    }

    /// The `platform collections` namespace object -- see `crate::platform`'s
    /// module doc for how its payload shape (a two-field, always-`Nil`
    /// `Instance`) tells it apart from the `platform` singleton and the
    /// transcript stream. Like `new_stream_object`, it is allocated fresh
    /// per send: the namespace is stateless, so identity across sends
    /// carries no meaning that any caller could observe.
    pub(crate) fn new_collections_namespace(&mut self) -> ObjectReference<'gc> {
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
        enclosing: Option<ObjectReference<'gc>>,
        declaring_class: Option<ClassReference<'gc>>,
        superclass: Option<ClassReference<'gc>>,
    ) -> ClassReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Object);
        self.heap.allocate_object(
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
        let inherited = superclass.map_or(0, |superclass_application| {
            superclass_application.slot_count
        });
        // The nested-class cache slots. They are part of the instance layout
        // because the spec caches a nested class in the enclosing *object*;
        // a side table keyed by object would cost a hash lookup on a path
        // that is otherwise a slot read.
        //
        // Keyed by declaration identity: a subclass that redeclares an
        // inherited nested class shadows it for a plain `N` but not for
        // `super N`, so the two need separate slots -- see `nested_slot`.
        let mut nested_slot = superclass
            .map_or_else(IdentityMap::default, |superclass_application| {
                superclass_application.nested_slot.clone()
            });
        let mut next = inherited + mixin.slots.len();
        for nested in &mixin.nested {
            nested_slot.insert(crate::mixin::mixin_key(nested), next);
            next += 1;
        }
        let application = Rc::new(crate::mixin::MixinApplication {
            mixin: mixin.clone(),
            superclass: superclass.cloned(),
            slot_count: next,
            nested_slot,
            own_slot_base: inherited,
        });
        self.applications.insert(key, application.clone());
        application
    }

    /// The empty mixin `metaclass_of` applies for a class that declares no
    /// class side of its own but inherits one from an ancestor -- one `Rc`
    /// per owning mixin, as `builtin_mixin` below makes one per builtin, so
    /// the application cache keys each such level apart from its siblings.
    /// Contributes no methods or slots of its own; its only job
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
fn check_method_arity<'gc>(
    method: &crate::mixin::MixinMethod,
    receiver: Value<'gc>,
    given: usize,
) -> Result<(), Signal<'gc>> {
    if given == method.argument_count {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError {
        selector: method.selector,
        receiver,
        message: format!(
            "wrong number of arguments: `{}` takes {}, got {}",
            method.selector, method.argument_count, given
        ),
    }))
}

/// A block given a different number of arguments than it declares is refused
/// before it can reach a register: the window is laid out from the arguments
/// actually given, so too few would address registers past its end -- a panic
/// -- and too many would silently shift every temporary.
fn check_block_arity<'gc>(
    heap: &Heap<'gc>,
    parameter_count: usize,
    captured_frame: FrameReference<'gc>,
    given: usize,
) -> Result<(), Signal<'gc>> {
    if given == parameter_count {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError {
        selector: Symbol::VALUE,
        receiver: heap.frame_receiver(captured_frame),
        message: format!(
            "wrong number of block arguments: block takes {parameter_count}, got {given}"
        ),
    }))
}

/// L's error, for the activation `header` describes. The error does not name
/// its limit, so that changing it changes no recorded answer. It is kept out
/// of line, so that the check on every push is a compare and a branch and
/// nothing more.
#[cold]
#[inline(never)]
fn recursion_too_deep<'gc>(header: &ActivationHeader<'gc>) -> Signal<'gc> {
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
fn build_builtin_classes<'gc>(mutator: &mut Mutator<'gc>) -> Vec<ClassReference<'gc>> {
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
    // A builtin's behaviour is native, so its mixin's method table holds its
    // primitives and nothing else (`builtin_mixin`): the application gives
    // the builtin the same shape every other class object has, and -- for
    // the non-`Object` builtins -- links an application chain parallel to
    // the class-object chain, which `dispatch::next_level` steps in lockstep
    // and along which a builtin receiver reaches `Object`'s primitives.
    let object_application = mutator.apply_mixin(&builtin_mixin(BuiltinClass::Object), None);
    let class_of_class =
        build_self_referential_object_class(&mut mutator.heap, object_application.clone());
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
        let application = mutator.apply_mixin(&builtin_mixin(kind), Some(&object_application));
        classes.push(mutator.heap.allocate_object(
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

/// The mixin behind one `BuiltinClass`: its method table holds `kind`'s
/// primitives (`builtins::primitive::method_table`), and nothing else. A
/// fresh `Rc` per call, so the application cache keys the builtins apart from
/// each other even where several -- `Boolean`, `True` and `False` -- hold the
/// same primitives.
fn builtin_mixin(kind: BuiltinClass) -> Rc<crate::mixin::Mixin> {
    Rc::new(crate::mixin::Mixin {
        methods: crate::builtins::primitive::method_table(kind),
        ..crate::mixin::Mixin::empty(Symbol::intern(&format!("{kind:?}")))
    })
}

/// Builds the single, genuinely self-referential `Object`-tagged class
/// object every `BuiltinClass` shares as its own `.class`, which is what
/// lets `Object.class` stay non-optional. The construction itself lives in
/// `Heap::allocate_self_referential_class`; under the arena it is ordinary
/// safe Rust, because the slot's index is known before the slot is filled.
fn build_self_referential_object_class<'gc>(
    heap: &mut Heap<'gc>,
    application: Rc<crate::mixin::MixinApplication>,
) -> ClassReference<'gc> {
    heap.allocate_self_referential_class(ObjectData::Class(ClassData {
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

    fn load(source: &str) -> (Interpreter, Root) {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        (interpreter, class)
    }

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// A host reads and allocates through `enter`, which answers what its
    /// closure answers and runs nothing: the stack is as deep afterwards
    /// as before (`docs/design/memory.md`, "The brand").
    #[test]
    fn enter_answers_its_closures_value_and_runs_nothing() {
        let mut interpreter = Interpreter::bootstrap();
        let length = interpreter.enter(|mutator| {
            let array = mutator.new_array(vec![Value::SmallInteger(1), Value::SmallInteger(2)]);
            mutator.heap().array_length(array)
        });
        assert_eq!(length, Some(2));
        assert_eq!(
            interpreter.enter(|mutator| mutator.heap().activation_depth()),
            0
        );
    }

    /// Deciding a send runs nothing: a user method comes back as the
    /// activation to make, with nothing pushed, and a native leaf as its
    /// value (`docs/design/execution.md`, "Deciding, then running").
    #[test]
    fn deciding_a_send_answers_what_to_run_without_running_it() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public foo = ( ^1 )
            ) : ()
        "};
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(&instance);
            match mutator.decide_send(receiver, Symbol::intern("foo"), &[]) {
                Ok(Step::Method { method, .. }) => assert_eq!(method.selector.as_str(), "foo"),
                _ => panic!("a user method is decided as an activation"),
            }
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
            match mutator.decide_send(
                Value::SmallInteger(3),
                Symbol::intern("+"),
                &[Value::SmallInteger(4)],
            ) {
                Ok(Step::Value(Value::SmallInteger(7))) => {}
                _ => panic!("a native leaf is decided as its value"),
            }
        });
    }

    /// `do:` is decided as the activation of its block for the first
    /// element, with the rest of the loop in the continuation: nothing runs,
    /// and nothing is pushed (`docs/design/execution.md`, "No re-entry from
    /// Rust").
    #[test]
    fn deciding_do_answers_its_block_for_the_first_element() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public items = ( ^{10. 20. 30} )
              public echo = ( ^[:e | e] )
            ) : ()
        "};
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let items = expect_ok(
            interpreter.send(&instance, Symbol::intern("items"), &[]),
            "items",
        );
        let echo = expect_ok(
            interpreter.send(&instance, Symbol::intern("echo"), &[]),
            "echo",
        );
        interpreter.enter(|mutator| {
            let (items, echo) = (mutator.fetch(&items), mutator.fetch(&echo));
            match mutator.decide_send(items, Symbol::intern("do:"), &[echo]) {
                Ok(Step::Block {
                    arguments: StepArguments::Owned(arguments),
                    then: Continuation::DoElement(state),
                    ..
                }) => {
                    assert!(
                        matches!(arguments.as_slice(), [Value::SmallInteger(10)]),
                        "the first element"
                    );
                    assert_eq!(state.next, 1, "the second element is next");
                    assert_eq!(state.items.len(), 3, "the whole snapshot rides along");
                }
                _ => panic!("`do:` is decided as its block's first activation"),
            }
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
        });
    }

    /// The nested-class accessor is decided as the activation of the
    /// superclass clause, with steps 3-5 in the continuation. Nothing is
    /// pushed, so no creation is in progress yet: one is marked only once its
    /// clause's activation is on the stack, and the cache slot is still
    /// `Nil`.
    #[test]
    fn deciding_a_nested_class_answers_its_superclass_clause_as_an_activation() {
        let source = indoc! {"
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
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        interpreter.enter(|mutator| {
            let instance = mutator.fetch(&instance);
            let slot = match mutator.decide_send(instance, Symbol::intern("N"), &[]) {
                Ok(Step::SuperclassClause(creation)) => {
                    assert!(
                        matches!(instance, Value::Object(object) if object.same(creation.enclosing)),
                        "the clause runs on the enclosing object"
                    );
                    creation.index
                }
                _ => panic!("a nested class is decided as its clause's activation"),
            };
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
            let Value::Object(object) = instance else {
                panic!("an instance is an object");
            };
            assert!(
                matches!(
                    mutator.heap().instance_field(object, slot),
                    Some(Value::Nil)
                ),
                "nothing is marked as in progress"
            );
            #[cfg(debug_assertions)]
            assert_eq!(mutator.creations_in_progress, 0, "nothing is counted");
        });
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
        let source = indoc! {"
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
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let p_class_root = expect_ok(interpreter.send(&instance, Symbol::intern("P"), &[]), "P");
        let q_class_root = expect_ok(interpreter.send(&instance, Symbol::intern("Q"), &[]), "Q");
        let r_class_root = expect_ok(interpreter.send(&instance, Symbol::intern("R"), &[]), "R");
        let s_class_root = expect_ok(interpreter.send(&instance, Symbol::intern("S"), &[]), "S");
        interpreter.enter(|mutator| {
            let (p_class, q_class, r_class, s_class) = (
                mutator.fetch(&p_class_root),
                mutator.fetch(&q_class_root),
                mutator.fetch(&r_class_root),
                mutator.fetch(&s_class_root),
            );
            let Value::Object(p_class_object) = p_class else {
                panic!("`P` answers a class object")
            };
            let Value::Object(s_class_object) = s_class else {
                panic!("`S` answers a class object")
            };
            assert!(
                matches!(
                    mutator.decide_send(p_class, Symbol::NEW, &[]),
                    Ok(Step::Initializer { level, .. }) if level.same(p_class_object)
                ),
                "`P new` is decided as P's initializer, which delivers"
            );
            match mutator.decide_send(r_class, Symbol::NEW, &[]) {
                Ok(Step::InitializerThen {
                    level, then: state, ..
                }) => {
                    assert!(level.same(p_class_object));
                    assert!(matches!(
                        state.phase,
                        InstantiationPhase::Descending { index: 1 }
                    ));
                }
                _ => {
                    panic!("`R new` is decided as its superclass P's initializer, R's still to run")
                }
            }
            match mutator.decide_send(s_class, Symbol::NEW, &[]) {
                Ok(Step::ClauseMessage { level, then, .. }) => {
                    assert!(level.same(s_class_object));
                    assert!(matches!(
                        then.phase,
                        InstantiationPhase::Climbing { level, .. } if level.same(s_class_object)
                    ));
                }
                _ => panic!("`S new` is decided as its clause message's activation"),
            }
            assert!(
                matches!(
                    mutator.decide_send(q_class, Symbol::NEW, &[]),
                    Ok(Step::Value(Value::Object(_)))
                ),
                "`Q new` is decided as its instance"
            );
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
        });
    }

    /// A finished instantiation leaves its walk's box among the spares for
    /// the next one, and leaves it empty: a spare holds no handle, so it
    /// keeps nothing alive and a collector need not trace it. The next
    /// instantiation walks in the same box.
    #[test]
    fn a_finished_walk_leaves_an_empty_box_for_the_next() {
        let source = indoc! {"
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
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let r_class_root = expect_ok(interpreter.send(&instance, Symbol::intern("R"), &[]), "R");
        interpreter.enter(|mutator| {
            let Value::Object(_) = mutator.fetch(&r_class_root) else {
                panic!("`R` answers a class object")
            };
        });
        expect_ok(interpreter.instantiate(&r_class_root, &[]), "R new");
        assert_eq!(
            interpreter.enter(|mutator| mutator.spare_walks.len()),
            1,
            "one walk ran at a time"
        );
        // Only the box's address leaves the entry that took it, untyped: a
        // pointer to an `Instantiation` would name the entry's brand.
        let spare: *const () =
            interpreter.enter(|mutator| std::ptr::from_ref(&*mutator.spare_walks[0]).cast::<()>());
        assert!(
            interpreter.enter(|mutator| mutator.spare_walks.iter().all(|state| matches!(
                state.instance,
                Value::Nil
            ) && state
                .levels
                .is_empty())),
            "a spare holds no handle"
        );
        expect_ok(interpreter.instantiate(&r_class_root, &[]), "R new again");
        assert_eq!(interpreter.enter(|mutator| mutator.spare_walks.len()), 1);
        assert!(
            interpreter.enter(|mutator| std::ptr::eq(
                spare,
                std::ptr::from_ref(&*mutator.spare_walks[0]).cast::<()>()
            )),
            "the second walk ran in the first one's box"
        );
    }

    /// A walk that climbs through a clause message with arguments, and then
    /// finds no initializer at any level, retires straight out of
    /// `descend_levels`'s "nothing below" branch -- the branch that never
    /// writes `Descending` into `phase`. `retire_walk` must clear `phase`
    /// itself, or the retired box still holds the two `ClassReference`s of the
    /// last `Climbing` it was in, contradicting "a spare holds no handle".
    #[test]
    fn a_retired_walk_holds_no_classref_in_its_phase() {
        let source = indoc! {"
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
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let cell = expect_ok(
            interpreter.send(&instance, Symbol::intern("Cell"), &[]),
            "Cell",
        );
        interpreter.enter(|mutator| {
            let Value::Object(_) = mutator.fetch(&cell) else {
                panic!("`Cell` answers a class object")
            };
        });
        expect_ok(interpreter.instantiate(&cell, &[]), "Cell new");
        interpreter.enter(|mutator| {
            assert_eq!(mutator.spare_walks.len(), 1, "one walk ran");
            let state = &mutator.spare_walks[0];
            assert!(matches!(state.instance, Value::Nil), "no instance handle");
            assert!(state.levels.is_empty(), "no levels");
            assert!(
                matches!(state.phase, InstantiationPhase::Descending { .. }),
                "a retired box's phase holds no ClassReference either"
            );
        });
    }

    /// Every decision answers a `Result<Step, Signal>`, so its size is paid
    /// on every send; a variant that grows it is a cost on every benchmark,
    /// not only on the path it serves (`StepArguments::ControlFlow`'s comment).
    /// This is the size the hot path was measured at, in a release build: a
    /// debug build's handles carry an origin and a move epoch besides their
    /// index, so it measures something else. The measurement protocol runs
    /// this with `--release`.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_decision_stays_the_size_the_hot_path_was_measured_at() {
        assert_eq!(std::mem::size_of::<Result<Step, Signal>>(), 56);
    }

    /// The debug-build twin of the pin above, so that a variant that grows a
    /// decision is noticed by the ordinary suite and not only by the release
    /// run the measurement protocol makes. It only notices growth: the
    /// release figure is the one the hot path was measured at, and a debug
    /// handle carries its origin and move epoch besides its index, which is
    /// what puts this above it.
    #[cfg(all(target_pointer_width = "64", debug_assertions))]
    #[test]
    fn a_decision_stays_the_size_it_has_in_a_debug_build() {
        assert_eq!(std::mem::size_of::<Result<Step, Signal>>(), 64);
    }
}
