//! Sends, method invocation, and the `Signal` non-local-return/error channel,
//! shared by everything that runs Newspeak code. Implicit-receiver and
//! `super` sends are in `sends.rs`, register and outer-local access in
//! `locals.rs`, the errors a deferred literal raises in `literals.rs`; the
//! bootstrap and class instantiation are in `instantiate.rs`.

pub(crate) mod inline_cache;
pub mod instantiate;
pub(crate) mod kernel;
pub(crate) mod literals;
pub(crate) mod locals;
pub(crate) mod mirror;
pub(crate) mod nested;
pub(crate) mod resolve;
mod safepoint;
pub(crate) mod sends;
mod slots;
pub(crate) mod value_class;

use std::rc::Rc;

use crate::compiler::Lowering;
use crate::hash::IdentityMap;
use crate::heap::{ActivationHeader, ActivationReference, Heap, Root};
use crate::types::*;
use crate::Symbol;

// `inline_cache` is `pub(crate)`, so `FeedbackBase` on its own would be
// visible only inside this crate; re-exporting it through `interpreter`
// (a `pub mod`) is what gives `types.rs`'s `pub` field on `ClosureData` a
// fully public path to it, which a `pub` field's type must have.
pub use inline_cache::FeedbackBase;
// The inline caches' test view (`Interpreter::call_site_states`) and sizes
// (`Interpreter::cache_statistics`), public through `interpreter` for the
// same reason.
pub use inline_cache::{
    CacheStatistics, CachedTarget, CachedTargetKind, CallSiteState, MethodSide,
};

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
        /// The method's family's range, if already known -- a lexical or
        /// `super` send's decision has none to give, so `push_step` looks it
        /// up from the code itself when this is `NOT_ASSIGNED`.
        feedback_base: inline_cache::FeedbackBase,
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
    /// Activate `code`, the arguments of a clause message written in
    /// `level`'s declaration, on the instance `then` is initializing and with
    /// `level`'s factory arguments (§6.2); its value is the factory arguments
    /// of the level above -- the superclass, or an applied level -- with which
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
    /// The VM mirror (`BuiltinClass::VmMirror`'s one instance), built with
    /// the builtin classes and handed to the core library at bootstrap. A
    /// capability: nothing hands it to a program.
    pub(crate) vm_mirror: Option<Value<'gc>>,
    /// The core library's one instance, `Kernel.ns`'s root class
    /// instantiated with the VM mirror (`interpreter::kernel`): the
    /// enclosing object of every builtin class. `None` only until the
    /// bootstrap's kernel phase has run.
    pub(crate) kernel: Option<Value<'gc>>,
    /// The `=` the core library's `Value` declares, recorded by the
    /// bootstrap's kernel phase. `builtins::value`'s derived equality
    /// compares against it to tell an inherited `=` from an overridden one:
    /// only an instance whose lookup of `=` lands on this very method has a
    /// state the derivation may read. `None` while `Value` declares none,
    /// and the derivation then declines every instance.
    ///
    /// An `Rc<MixinMethod>`, not a heap handle: a method holds nothing a
    /// move or a collection can invalidate, so it needs no root and nothing
    /// traces it.
    pub(crate) value_equality_method: Option<Rc<crate::mixin::MixinMethod>>,
    /// `Value`'s own mixin, recorded by the bootstrap's kernel phase just
    /// before `Value`'s class object is filled: what `apply_mixin` compares
    /// a mixin against to answer §3.1.1's "its class inherits from class
    /// `Value`" for the chain's topmost link. Compared by `Rc` identity,
    /// never by name -- any module may declare a class called `Value`.
    ///
    /// The *mixin* rather than the application, so that `Value`'s own
    /// application can be told it is one while it is being built; every
    /// application below it reads the flag off its superclass and never
    /// asks again.
    ///
    /// `None` until that moment, so the placeholder applications the world
    /// is built with inherit nothing.
    pub(crate) value_mixin: Option<Rc<crate::mixin::Mixin>>,
    /// (mixin identity, superclass application identity) -> behaviour.
    /// Keyed on the superclass's *application*, never on a superclass class
    /// object: a nested superclass has one class object per enclosing
    /// instance, so a class-object key would never hit and every instance
    /// would get its own behaviour.
    pub(crate) applications: IdentityMap<(usize, usize), Rc<crate::mixin::MixinApplication>>,
    /// class mixin identity -> the metaclass mixin `metaclass_of` synthesized
    /// for a class that declares no class side of its own. §6.3 makes the
    /// metaclass mixin "the class side, together with the primary factory
    /// method", so such a class still needs one -- an otherwise empty mixin
    /// holding the factory entry.
    ///
    /// Memoized rather than built per class object for the reason
    /// `applications` is keyed the way it is: a class object is per enclosing
    /// instance, and a fresh `Rc<Mixin>` each time would give every one of
    /// them its own application and its own dispatch id, so no two instances
    /// would share behaviour and every `X new` site would go megamorphic.
    /// Keyed by the class mixin's identity, which the application holding it
    /// keeps alive in `applications` for as long as any class object can
    /// reach this.
    pub(crate) metaclass_mixins: IdentityMap<usize, Rc<crate::mixin::Mixin>>,
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
    /// The next dispatch id `apply_mixin` mints. Past the last id below
    /// `DispatchId::VACANT` it stops minting and applications get `NONE`,
    /// which is never cached: slower, never wrong.
    pub(crate) next_dispatch_id: u32,
    /// The inline caches (`inline_cache.rs`): per heap, since a dispatch id
    /// means nothing in another heap.
    pub(crate) caches: inline_cache::InlineCaches,
}

/// A host's interpreter: the step state at rest (`World`), and what only
/// the run loop between steps needs -- whether a run is in progress, and
/// the stress modes' schedules. Its methods are the host's entries: `enter`
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
    /// Whether the cache stress mode is on (`NS_NEXT_STRESS_CACHES`, or
    /// `set_stress_caches`): every safepoint invalidates every inline cache.
    /// Debug builds only.
    #[cfg(debug_assertions)]
    stress_caches: bool,
    /// The collect stress mode's schedule, when it is on
    /// (`NS_NEXT_STRESS_COLLECT`, or `set_stress_collect`): a full
    /// collection at the safepoints its schedule picks, paced like the move
    /// stress mode since a collection's cost is proportional to the traced
    /// size too (`docs/design/memory.md`, "The gates the collector needs").
    /// Debug builds only.
    #[cfg(debug_assertions)]
    stress_collect: Option<safepoint::StressCollections>,
    /// How many times the cache stress mode has invalidated the caches since
    /// it was turned on, for a test to pin that it actually ran
    /// (`cache_invalidations`). Debug builds only.
    #[cfg(debug_assertions)]
    cache_invalidations: usize,
}

impl Interpreter {
    pub fn bootstrap() -> Interpreter {
        Interpreter::bootstrap_with(Lowering::Optimized)
    }

    /// An interpreter whose `load_module` compiles with `lowering`. The
    /// reference lowering is for checking the optimized one against it; an
    /// artifact decoded into either runs the code it carries.
    pub fn bootstrap_with(lowering: Lowering) -> Interpreter {
        // Only the debug build's stress modes write to these fields below.
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut interpreter = Interpreter {
            world: crate::heap::World::new(lowering),
            running: false,
            #[cfg(debug_assertions)]
            stress: safepoint::StressMoves::from_env(),
            #[cfg(debug_assertions)]
            stress_caches: safepoint::stress_caches_from_env(),
            #[cfg(debug_assertions)]
            stress_collect: safepoint::StressCollections::from_env(),
            #[cfg(debug_assertions)]
            cache_invalidations: 0,
        };
        #[cfg(debug_assertions)]
        {
            let stress_enabled = interpreter.stress.is_some()
                || interpreter.stress_caches
                || interpreter.stress_collect.is_some();
            interpreter.world.set_safepoint_requested(stress_enabled);
            // A fixture run small enough that every insert contends for the
            // same group (`run_goldens.rs`); anything not twice a decimal
            // power of two is ignored rather than refused, since this reads
            // the ambient environment rather than an explicit host call.
            if let Some(entries) = safepoint::global_cache_entries_from_env() {
                interpreter.set_global_cache_size(entries);
            }
        }
        // Last, after the stress modes are set up: the kernel's own
        // instantiation is a run like any other, and they should see it.
        interpreter.install_kernel();
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

    /// The inline-cache states of `method`'s sends of `sent`, `method` looked
    /// up on `class`'s own mixin (`side` chooses the class side). For tests
    /// only; empty when `class` is not a class object or has no such method.
    pub fn call_site_states(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
        sent: Symbol,
    ) -> Vec<CallSiteState> {
        self.enter(|mutator| {
            let Value::Object(object) = mutator.fetch(class) else {
                return Vec::new();
            };
            let Some(application) = mutator.heap.class_application(object) else {
                return Vec::new();
            };
            let mixin = match side {
                MethodSide::Instance => Some(application.mixin.clone()),
                MethodSide::Class => application.mixin.class_side.clone(),
            };
            let Some(code) = mixin.and_then(|mixin| {
                mixin
                    .compiled_method(method)
                    .map(|found| found.code.clone())
            }) else {
                return Vec::new();
            };
            mutator.caches.states_of(&code, sent)
        })
    }

    /// The `platform` object, rooted for a host.
    pub fn platform(&mut self) -> Root {
        self.enter(|mutator| {
            let platform = mutator.platform_value();
            mutator.stash(platform)
        })
    }

    /// Vacates every inline cache -- every site, every polymorphic block and
    /// the global cache -- while keeping every code family's assigned range
    /// (`docs/design/dispatch.md`, "Invalidation"): a running activation's
    /// `feedback_base` still names the same range afterwards, so the next
    /// send through a vacated site simply fills it again and answers what it
    /// answered before.
    pub fn invalidate_caches(&mut self) {
        self.world.invalidate_caches();
    }

    /// The inline caches' sizes: how much of what is allocated is actually
    /// filled, and how large one entry of each kind is
    /// (`--cache-statistics`, and the pinning tests).
    pub fn cache_statistics(&mut self) -> CacheStatistics {
        self.world.cache_statistics()
    }

    /// The arenas' occupancy. Available in every build: it is what a
    /// measurement and a retention test both read.
    pub fn heap_statistics(&self) -> crate::HeapStatistics {
        self.world.heap_statistics()
    }

    /// The collector's counts and pauses since the process started, plus the
    /// arenas' occupancy right now (`--collector-statistics`). Available in
    /// every build, like `heap_statistics`.
    pub fn collector_statistics(&self) -> crate::CollectorStatistics {
        self.world.collector_statistics()
    }

    /// Replaces the global cache with a fresh one sized for `entries`,
    /// vacating whatever it held. A test hook (`NS_NEXT_GLOBAL_CACHE_ENTRIES`,
    /// `run_goldens.rs`), not an input path a program can reach: `entries`
    /// must be twice a power of two -- two per group -- and a host that gets
    /// this wrong panics rather than the cache rounding it silently.
    pub fn set_global_cache_size(&mut self, entries: usize) {
        self.enter(|mutator| mutator.caches.set_global_cache_size(entries));
    }
}

#[cfg(debug_assertions)]
impl Interpreter {
    /// Turns the cache stress mode on or off for this interpreter, whatever
    /// `NS_NEXT_STRESS_CACHES` says. For tests. Keeps the safepoint request
    /// set while either other stress mode is on, since any one alone must
    /// keep every safepoint coming back to `safepoint`.
    ///
    /// Like `set_stress_moves`, this can drop `safepoint_requested` to
    /// `false` while a collection is still pending, by the same
    /// self-healing, debug-only path: the collection limits were never
    /// reset, so the next allocation that reaches one re-requests every
    /// flag.
    pub fn set_stress_caches(&mut self, enabled: bool) {
        self.stress_caches = enabled;
        self.world.set_safepoint_requested(
            enabled || self.stress.is_some() || self.stress_collect.is_some(),
        );
    }

    /// How many times the cache stress mode has invalidated the caches since
    /// it was turned on; 0 while it is off.
    pub fn cache_invalidations(&self) -> usize {
        self.cache_invalidations
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
        self.refresh_immediate_dispatch_ids();
        let class = self.builtin_class(BuiltinClass::VmMirror);
        let mirror = self.new_instance_object(class, ObjectData::Instance(vec![]));
        self.vm_mirror = Some(Value::Object(mirror));
    }

    /// An immediate has no class field to read a dispatch id from, so the
    /// ids `dispatch_id` answers for one are read off the builtin class
    /// objects and kept in a table of their own
    /// (`Heap::set_immediate_dispatch_ids`) -- once when the placeholders
    /// are built, and again when the kernel phase gives them their
    /// applications (`interpreter::kernel`).
    pub(crate) fn refresh_immediate_dispatch_ids(&mut self) {
        let instances = |mutator: &Mutator<'gc>, kind| {
            mutator
                .heap
                .class_dispatch_ids(mutator.builtin_class(kind))
                .instances
        };
        let ids = [
            instances(self, BuiltinClass::Nil),
            instances(self, BuiltinClass::False),
            instances(self, BuiltinClass::True),
            instances(self, BuiltinClass::Integer),
            instances(self, BuiltinClass::Float),
        ];
        self.heap.set_immediate_dispatch_ids(ids);
    }

    /// The next pair of dispatch ids, or `DispatchIds::NONE` once the counter
    /// is exhausted: an application built after that point is simply never
    /// cached, which is slower but never wrong, so running out is not an
    /// error. Stopping strictly before `DispatchId::VACANT` and
    /// `DispatchId::NONE` -- rather than at them -- is what keeps
    /// `DispatchId::from_counter`'s assertion from ever firing on a minted
    /// id.
    ///
    /// Always mints the pair consecutively -- `instances` then `instances +
    /// 1` for `class_side` -- because `ClassData` stores only `instances`
    /// and derives the other id from it (`DispatchId::class_side`) to keep a
    /// class object the size `CLAUDE.md`'s image-amplification paragraph
    /// assumes. The `debug_assert_eq!` below is what would catch the two
    /// drifting apart if this ever minted them any other way.
    pub(crate) fn mint_dispatch_ids(&mut self) -> crate::mixin::DispatchIds {
        let first = self.next_dispatch_id;
        let Some(after) = first.checked_add(2) else {
            return crate::mixin::DispatchIds::NONE;
        };
        if after > u32::MAX - 1 {
            return crate::mixin::DispatchIds::NONE;
        }
        self.next_dispatch_id = after;
        let instances = crate::mixin::DispatchId::from_counter(first);
        let class_side = crate::mixin::DispatchId::from_counter(first + 1);
        debug_assert_eq!(
            class_side,
            instances.class_side(),
            "internal: a minted pair's class-side id must be exactly one past its instances id"
        );
        crate::mixin::DispatchIds {
            instances,
            class_side,
        }
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
    /// running it (`docs/design/execution.md`, "Deciding, then running"):
    /// the lookup (`resolve_public`), then what it found (`act`), which
    /// decides an instantiation (`decide_instantiation`) rather than
    /// entering the host entry `instantiate`, which would enter the run loop
    /// from inside it -- the re-entry `run_from_host` refuses. See
    /// `instantiate::decide_factory`'s own doc comment for why this can never
    /// double-fire against the superclass clause's own deferred factory
    /// message (`climb_levels` never decides a send).
    pub(crate) fn decide_send(
        &mut self,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let resolved = self.resolve_public(receiver, selector);
        self.act(resolved, receiver, selector, arguments)
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
                feedback_base,
            } => {
                let arguments = arguments.slice(send_arguments);
                check_method_arity(&method, receiver, arguments.len())?;
                let code = method.code.clone();
                // A decision never knows its method's range -- only
                // `push_step`, about to activate the family's root, does --
                // so every decider hands `NOT_ASSIGNED` and this is where it
                // is found, once, per family (`inline_cache::InlineCaches::base_for`).
                let feedback_base = if feedback_base == FeedbackBase::NOT_ASSIGNED {
                    self.caches.base_for(&code)
                } else {
                    feedback_base
                };
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
                        feedback_base,
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
                let (code, parameter_count, captured_frame, closure_feedback_base) = self
                    .heap
                    .closure_activation(closure)
                    .expect("internal: a block step is decided only for a closure object");
                check_block_arity(&self.heap, parameter_count, captured_frame, arguments.len())?;
                // A closure's range is minted once, the first time it is
                // activated: every closure `MakeClosure` creates already
                // carries its method's family's base (below), so
                // `NOT_ASSIGNED` here means a closure an image decoded,
                // which never ran `MakeClosure` at all.
                let feedback_base = if closure_feedback_base == FeedbackBase::NOT_ASSIGNED {
                    let fresh = self.caches.fresh_range(code.family_size);
                    self.heap.set_closure_feedback_base(closure, fresh);
                    fresh
                } else {
                    closure_feedback_base
                };
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
                let activation = self.push(
                    ActivationHeader {
                        receiver,
                        current_class,
                        kind: FrameKind::Closure(closure),
                        slot_base: 0, // a closure body never contains InitializeSlot
                        code,
                        then,
                        counted,
                        feedback_base,
                    },
                    arguments,
                    temporaries,
                )?;
                // The copied values sit between the block's parameters and
                // its temporaries, so they are written once the register
                // file exists and before the body runs an instruction
                // (`docs/design/execution.md`, "Converted representation").
                self.heap.fill_copied_values(activation, closure);
                Ok(activation)
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
                let feedback_base = self.caches.base_for(&code);
                let temporaries = (code.register_count - code.argument_count) as usize;
                let kind = instantiate::clause_message_frame_kind(level, &then);
                self.push(
                    ActivationHeader {
                        receiver: then.instance,
                        current_class: Some(level),
                        kind,
                        slot_base: 0, // a clause message never contains InitializeSlot
                        code,
                        then: Continuation::Instantiate(then),
                        counted: true,
                        feedback_base,
                    },
                    &arguments,
                    temporaries,
                )
            }
            Step::SuperclassClause(creation) => {
                let code = nested::superclass_clause_code(&creation.nested);
                let feedback_base = self.caches.base_for(&code);
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
                        feedback_base,
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
        let feedback_base = self.caches.base_for(&code);
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
                feedback_base,
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
    /// does, and a superclass clause's or its message's, a `<:` clause's
    /// message included. An initializer catches
    /// none -- a `^` into a running one escapes, the known gap in
    /// `docs/design/execution.md`, "Non-local return" -- and a block's `^`
    /// always targets its home, never the block.
    pub(crate) fn catches_return(&self, activation: ActivationReference) -> bool {
        matches!(
            self.heap.activation_kind(activation),
            FrameKind::Method(_)
                | FrameKind::SuperclassClause(_)
                | FrameKind::SuperclassMessage(_)
                | FrameKind::MixinApplicationMessage(_)
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

    /// The class object's metaclass, built on first use.
    ///
    /// `newspeak-spec.pdf` §3.2: "All metaclasses are instances of class
    /// Metaclass, and are direct subclasses of Class. Metaclasses may not be
    /// subclassed. [...] The metaclass hierarchy in Newspeak is deliberately
    /// kept very flat." §6 gives the construction: the class is an instance
    /// of "a metaclass that is the application of the metaclass mixin defined
    /// by the class' class side to the class Class".
    ///
    /// So a class side is *not* inherited: an ordinary class-side send that
    /// misses here never continues onto an ancestor's class side. ns-next
    /// reifies no `Class`, so the application has no superclass application
    /// and a miss falls through to the class object's own behaviour, which
    /// is what a class-side chain already ends in
    /// (`docs/design/object-model.md`, "Class sides and metaclasses").
    ///
    /// Every class object has a metaclass, including one whose declaration
    /// writes no class side: §6.3 makes the metaclass mixin the class side
    /// *together with the primary factory method*, so `X new` is an entry of
    /// this mixin's method table and a class without one would not be
    /// instantiable. The empty case is `metaclass_mixin` below.
    ///
    /// The metaclass's own *class-object* superclass link is not `None`,
    /// though: §5.10 says a `super` send inside a class-side method starts
    /// its lookup at S, the superclass of `current_class`, and §6 makes
    /// that superclass `Class` -- approximated, since ns-next reifies no
    /// `Class`, as `Object`'s behaviour (the same fall-through above). That
    /// link is read only by a `super` send (`decide_super_send`,
    /// `inline_cache::send_super`), never by the ordinary lookup `walk`,
    /// which requires the *application* chain to agree before it steps
    /// (`dispatch::next_level`) -- and the metaclass's application names no
    /// superclass application above, so an ordinary class-side send still
    /// stops at this one level, exactly as "not inherited" requires.
    ///
    /// No walk, no stack: the chain this used to climb was the reason the
    /// function was iterative.
    pub fn metaclass_of(&mut self, class: ClassReference<'gc>) -> Option<ClassReference<'gc>> {
        if !self.heap.is_class(class) {
            return None;
        }
        if let Some(cached) = self.heap.class_metaclass(class) {
            return Some(cached);
        }
        let application = self.heap.class_application(class)?;
        let class_side_mixin = self.metaclass_mixin(&application.mixin);
        let enclosing = self.heap.class_enclosing(class);
        let declaring_class = self.heap.class_declaring(class);
        let metaclass_application = self.apply_mixin(&class_side_mixin, None);
        let object_class = self.builtin_class(BuiltinClass::Object);
        // §6.3: a class side is written inside the same class declaration
        // its instance side is, so a class-side method's `outer` must reach
        // exactly the objects an instance-side method's does. The metaclass
        // borrows the class's own place in the declaration chain.
        let metaclass = self.new_class_object(
            metaclass_application,
            enclosing,
            declaring_class,
            Some(object_class),
        );
        if !self.heap.set_class_metaclass(class, metaclass) {
            unreachable!("is_class was checked above");
        }
        Some(metaclass)
    }

    /// The mixin `metaclass_of` applies for `mixin`'s class objects: §6.3's
    /// "the class side, together with the primary factory method".
    ///
    /// A declared class side already carries the factory entry -- it is
    /// inserted where the class side is built, from source
    /// (`instantiate::build_class_side`) and on the way out of an image
    /// (`image::mixins::decode`) alike. A class that declares none still
    /// needs a metaclass to hold that one entry, and gets an otherwise empty
    /// mixin here: the case that covers every builtin, whose behaviour is
    /// native and whose factory is `new` -- the default §6's unnumbered
    /// introduction gives an omitted factory signature, before §6.1 begins.
    fn metaclass_mixin(&mut self, mixin: &Rc<crate::mixin::Mixin>) -> Rc<crate::mixin::Mixin> {
        if let Some(class_side) = &mixin.class_side {
            return class_side.clone();
        }
        let key = crate::mixin::mixin_key(mixin);
        if let Some(existing) = self.metaclass_mixins.get(&key) {
            return existing.clone();
        }
        let synthesized = Rc::new(crate::mixin::Mixin {
            methods: std::iter::once(crate::mixin::factory_entry(mixin.factory)).collect(),
            ..crate::mixin::Mixin::empty(Symbol::CLASS_SIDE)
        });
        self.metaclass_mixins.insert(key, synthesized.clone());
        synthesized
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

    /// The one object `symbol` is in this heap, allocated the first time
    /// anything asks for it.
    pub fn new_symbol(&mut self, symbol: Symbol) -> Value<'gc> {
        if let Some(object) = self.heap.canonical_symbol(symbol) {
            return Value::Object(object);
        }
        let class = self.builtin_class(BuiltinClass::Symbol);
        let object = self.new_instance_object(class, ObjectData::Symbol(symbol));
        self.heap.record_canonical_symbol(symbol, object);
        Value::Object(object)
    }

    pub fn new_character(&mut self, character: char) -> ObjectReference<'gc> {
        // There is no dedicated Character payload variant on
        // ObjectData -- represent a character as a one-`char` Rust String
        // wrapped the same way String literals are (String), consistent with
        // how the language spec itself anticipates going forward: §2.2.3
        // expects to "make all string literals act as symbols, and represent
        // characters as a special case of these" (`newspeak-spec.pdf`).
        // `builtins::text` tags the *class* as `BuiltinClass::Character` so
        // dispatch still lands on the right native methods; only the payload
        // representation is shared.
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

    /// The VM mirror. Built by `bootstrap_builtin_classes`, before anything
    /// can ask; the kernel phase hands it to `Kernel.ns`.
    pub(crate) fn vm_mirror(&self) -> Value<'gc> {
        self.vm_mirror
            .expect("internal: the VM mirror is built with the builtin classes")
    }

    /// Whether `value` is the VM mirror: the kernel phase checks that
    /// `Kernel.ns`'s `vmMirror` slot holds it.
    pub(crate) fn is_vm_mirror(&self, value: Value<'gc>) -> bool {
        match (value, self.vm_mirror) {
            (Value::Object(object), Some(Value::Object(mirror))) => object.same(mirror),
            _ => false,
        }
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

    /// A class object around an already-computed application. For an
    /// ordinary class object, this is where the invariant `dispatch::next_level`
    /// relies on is established: `superclass` is linked exactly when
    /// `application`'s own `superclass` is (callers derive `application`
    /// from `superclass` through `apply_mixin`), so the two chains it steps
    /// together stay the same length.
    ///
    /// `Mutator::metaclass_of` is the one deliberate exception: it passes a
    /// `superclass` of `Object` (so a `super` send inside a class-side
    /// method reaches `Object`'s behaviour, `newspeak-spec.pdf` §5.10) but
    /// an `application` whose own `superclass` is `None` (§3.2: a class
    /// side is not inherited, so the metaclass's application has nothing
    /// above it to apply against). See `dispatch::next_level`'s doc comment
    /// for why that mismatch is safe rather than a defect.
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
        self.allocate_class_object(ClassData::new(
            application,
            enclosing,
            declaring_class,
            superclass,
            None,
        ))
    }

    /// A level a mixin application adds (`newspeak-spec.pdf` §6.2): a class
    /// object like any other -- `new_class_object`'s invariant holds for it
    /// too -- that also carries the `<:` clause it was built for.
    pub(crate) fn new_applied_level(
        &mut self,
        application: Rc<crate::mixin::MixinApplication>,
        enclosing: Option<ObjectReference<'gc>>,
        declaring_class: Option<ClassReference<'gc>>,
        superclass: Option<ClassReference<'gc>>,
        clause: crate::mixin::AppliedClause,
    ) -> ClassReference<'gc> {
        let mut data = ClassData::new(application, enclosing, declaring_class, superclass, None);
        data.applied_clause = Some(Box::new(clause));
        self.allocate_class_object(data)
    }

    fn allocate_class_object(&mut self, data: ClassData<'gc>) -> ClassReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Object);
        self.heap
            .allocate_object(class, ObjectData::Class(Box::new(data)))
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
        // §3.1.1's two questions about a value class, asked once here rather
        // than per instance: does this chain reach `Value`, and does any
        // level of it declare a mutable slot while it does. The answer
        // belongs to the *application*, not to the mixin -- the same mixin
        // over a superclass that is no value declares an ordinary mutable
        // slot and is perfectly legal.
        //
        // `Value`'s own application answers `true`, because §3.2 defines
        // "subclass of" inclusively -- "a class C is a subclass of a class S
        // if C's superclass is S or a subclass of S", and §3.1.1's condition
        // is "its class inherits from class `Value`", which `Value new`
        // meets. `builtins::value`'s `state_is_a_value` reaches the same
        // verdict for `Value new` by another route (its `=` *is* `Value`'s),
        // and the two predicates must agree: while this one said `false`,
        // `Value new = Value new` answered `true` and `Box v: Value new` was
        // refused as if the instance were no value.
        let is_value_mixin = self
            .value_mixin
            .as_ref()
            .is_some_and(|value_mixin| Rc::ptr_eq(value_mixin, mixin));
        let inherits_value =
            is_value_mixin || superclass.is_some_and(|application| application.inherits_value);
        // §3.1.1's third condition on a value class, recorded here like
        // `first_mutable_slot`: the first level below `Value` -- `Value`'s
        // own excluded, since its next sentence allows that one -- whose
        // mixin declares an instance-side `==`. Checked once per application
        // rather than per instance, and refused where the class object comes
        // into being (`interpreter::value_class`), not where an instance is
        // made: unlike a mutable slot, a redefined `==` is wrong about the
        // class itself, whether or not anything is ever instantiated.
        let identity_method = superclass
            .and_then(|application| application.identity_method)
            .or_else(|| {
                (inherits_value
                    && !is_value_mixin
                    && mixin.compiled_method(Symbol::IDENTICAL).is_some())
                .then_some(mixin.name)
            });
        // The first mutable slot anywhere in the chain, superclass levels
        // first, recorded whether or not the chain inherits `Value`: a `<:`
        // clause can put `Value`'s level below an arbitrary class (§6.2), so
        // a class above that level can bring a mutable slot into a value's
        // instances even though it is declared nowhere below `Value`. Only
        // `instantiation_refusal`, below, asks whether the chain inherits
        // `Value` -- a mixin applied over a superclass that is no value
        // keeps its mutable slots legally, and this field alone cannot tell
        // the two cases apart.
        let first_mutable_slot = superclass
            .and_then(|application| application.first_mutable_slot)
            .or_else(|| {
                mixin.slots.iter().find(|slot| slot.mutable).map(|slot| {
                    crate::mixin::MutableValueSlot {
                        class: mixin.name,
                        slot: slot.name,
                    }
                })
            });
        // A root clause defect refuses every instance of the root and of
        // anything built on it, so it is inherited like the mutable slot is.
        // It comes first: a root whose chain is wrong has no layout worth
        // checking.
        let instantiation_refusal = mixin
            .root_clause_defect
            .clone()
            .map(crate::mixin::InstantiationRefusal::RootClause)
            .or_else(|| {
                superclass.and_then(|application| application.instantiation_refusal.clone())
            })
            .or_else(|| {
                first_mutable_slot
                    .filter(|_| inherits_value)
                    .map(crate::mixin::InstantiationRefusal::MutableValueSlot)
            });
        // The declared slots, in field order, computed here for the same
        // reason `nested_slot` is: nothing about an application changes
        // after it is built, and `builtins/value.rs` reads this per slot on
        // the declining path. Declared and hidden slots interleave -- this
        // level's own slots sit at `inherited`, its cache slots after them
        // -- so a value's state is not a prefix of its fields.
        let declared_slots =
            crate::mixin::declared_slots_of(superclass, inherited, mixin.slots.len());
        let dispatch_ids = self.mint_dispatch_ids();
        let application = Rc::new(crate::mixin::MixinApplication {
            mixin: mixin.clone(),
            superclass: superclass.cloned(),
            inherits_value,
            first_mutable_slot,
            identity_method,
            instantiation_refusal,
            slot_count: next,
            nested_slot,
            declared_slots,
            own_slot_base: inherited,
            dispatch_ids,
        });
        self.applications.insert(key, application.clone());
        application
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
/// These are placeholders: every non-`Object` builtin's superclass below is
/// `Object` directly, and each mixin holds its primitives alone. They exist
/// so that a literal has its class from the first allocation on; the
/// bootstrap's kernel phase (`interpreter::kernel`) then gives each the
/// application of its `Kernel.ns` class and the superclass that class's
/// clause names -- `Boolean` above `True` and `False` -- before any code
/// runs.
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
            ObjectData::Class(Box::new(ClassData::new(
                application,
                None,
                None,
                Some(class_of_class),
                Some(kind),
            ))),
        ));
    }
    classes
}

/// The mixin behind one `BuiltinClass` until the kernel phase runs. The VM
/// mirror's holds its rows (`builtins::primitive::mirror_method_table`) and
/// keeps them: `Kernel.ns` declares no mirror class. Every other kind's is a
/// placeholder with an empty table, which the kernel phase replaces with its
/// kernel class's mixin before anything runs (`interpreter::kernel`). A
/// fresh `Rc` per call, so the application cache keys the placeholders
/// apart from each other.
fn builtin_mixin(kind: BuiltinClass) -> Rc<crate::mixin::Mixin> {
    let methods = if kind == BuiltinClass::VmMirror {
        crate::builtins::primitive::mirror_method_table()
    } else {
        crate::hash::IdentityMap::default()
    };
    Rc::new(crate::mixin::Mixin {
        methods,
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
    heap.allocate_self_referential_class(ObjectData::Class(Box::new(ClassData::new(
        application,
        None,
        None,
        None,
        Some(BuiltinClass::Object),
    ))))
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

    /// Every lookup hands its answer to `resolve` as a `Lookup`, which
    /// `resolve::resolved_public` matches as it is rather than through the
    /// 64-byte settled enum 3a measured as costly on this path
    /// (`docs/design/execution.md`, "What primitives as method-table entries
    /// moved"); pinned here so that growing a `Lookup` variant fails this
    /// test instead of quietly widening that path.
    /// Unlike `Result<Step, Signal>` above, this one does not grow in a debug
    /// build: `SlotAccessor`'s own 12 bytes plus the absolute field index it
    /// carries already need a 24-byte slot once alignment rounds it up, and a
    /// debug `ClassReference`'s wider handle (12 bytes against a release
    /// build's 4, for the origin and move epoch that catch a stale or
    /// cross-heap read) only brings `Method`'s payload up to that same total,
    /// not past it.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_lookup_stays_the_size_the_hot_path_was_measured_at() {
        assert_eq!(std::mem::size_of::<crate::dispatch::Lookup>(), 24);
    }

    /// Minting stops strictly before `DispatchId::VACANT`, never landing on
    /// it or on `NONE`: both are reserved meanings (`Heap::dispatch_id` never
    /// answers `VACANT`, and `NONE` means "not cached"), so a minted id must
    /// never equal either.
    #[test]
    fn minting_never_reaches_vacant_and_answers_none_once_exhausted() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            // One pair below the ceiling: the last mintable pair is
            // (MAX - 3, MAX - 2), immediately below `VACANT` (MAX - 1).
            mutator.next_dispatch_id = u32::MAX - 3;
            let last = mutator.mint_dispatch_ids();
            assert_ne!(last.instances, crate::mixin::DispatchId::VACANT);
            assert_ne!(last.class_side, crate::mixin::DispatchId::VACANT);
            assert_ne!(last.instances, crate::mixin::DispatchId::NONE);
            assert_ne!(last.class_side, crate::mixin::DispatchId::NONE);

            let exhausted = mutator.mint_dispatch_ids();
            assert_eq!(exhausted, crate::mixin::DispatchIds::NONE);
        });
    }

    /// `ClassData` stores only a pair's `instances` id and derives
    /// `class_side` from it (`DispatchId::class_side`), which only answers
    /// the right id because `mint_dispatch_ids` always mints the pair
    /// consecutively. Minting a handful of pairs and checking each one
    /// against the derivation is what would catch that invariant breaking,
    /// beyond the single check `mint_dispatch_ids`'s own `debug_assert_eq!`
    /// makes on every call.
    #[test]
    fn every_minted_pairs_class_side_is_its_instances_id_derived() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            for _ in 0..5 {
                let pair = mutator.mint_dispatch_ids();
                assert_eq!(pair.class_side, pair.instances.class_side());
            }
        });
    }
}
