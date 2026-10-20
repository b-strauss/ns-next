//! Sends, method invocation, and the `Signal` non-local-return/error channel,
//! shared by everything that runs Newspeak code. Implicit-receiver and
//! `super` sends are in `sends.rs`, register and outer-local access in
//! `locals.rs`, the errors a deferred literal raises in `literals.rs`; the
//! bootstrap and class instantiation are in `instantiate.rs`.

pub(crate) mod core_library;
pub(crate) mod edit;
pub mod future;
pub(crate) mod inline_cache;
mod install;
pub mod instantiate;
pub(crate) mod kernel;
pub(crate) mod literals;
pub(crate) mod locals;
mod metaclass;
pub(crate) mod mirror;
pub(crate) mod nested;
mod reclaim;
pub(crate) mod resolve;
pub(crate) mod safepoint;
pub(crate) mod sends;
pub(crate) mod shipped_world;
mod slots;
pub(crate) mod unwind;
pub(crate) mod value_class;

use std::rc::Rc;

use crate::compiler::Lowering;
use crate::hash::IdentityMap;
use crate::heap::{ActivationHeader, ActivationReference, Heap, Root};
use crate::shipped_code::ShippedCodePath;
use crate::types::*;
use crate::Symbol;

// `inline_cache` is `pub(crate)`, so `FeedbackBase` on its own would be
// visible only inside this crate; re-exporting it through `interpreter`
// (a `pub mod`) is what gives `heap::NewClosure`'s `pub` field a
// fully public path to it, which a `pub` field's type must have.
pub use inline_cache::FeedbackBase;
// The inline caches' test view (`Interpreter::call_site_states`) and sizes
// (`Interpreter::cache_statistics`), public through `interpreter` for the
// same reason.
pub use inline_cache::{
    CacheStatistics, CachedTarget, CachedTargetKind, CallSiteState, MethodSide,
};

pub(crate) enum Signal<'gc> {
    /// A non-local return: its home's frame, never its activation, since the
    /// unwind that carries it runs `ensure:` blocks on its way, and an
    /// activation they push can reuse a popped activation's stack index
    /// (`docs/design/execution.md`, "Unwinding").
    Return(FrameReference<'gc>, Value<'gc>),
    Error(NewspeakError<'gc>),
    /// `Exception>>retry`: unwind to this `on:do:` activation's frame,
    /// running the `ensure:` blocks on the way, and run it again from its
    /// start (`interpreter::unwind`).
    Restart(FrameReference<'gc>),
    /// An unhandled exception's `defaultAction`: unwind to the run's floor,
    /// running every pending `ensure:` block, and leave the run with this
    /// error, which the host receives as it received every error before
    /// there were exceptions. Boxed, as the rarest of the four, so that
    /// `Error` is the one variant holding a `NewspeakError` in place and the
    /// others fit in its niche: `Signal` is 40 bytes in a release build and
    /// every `Result<Step, Signal>` a send decides 56. A second variant in
    /// place added eight bytes to each, which cost about 6% on a benchmark
    /// of deep sends and returns that never signals
    /// (`docs/design/execution.md`, "What exceptions cost").
    Terminate(Box<NewspeakError<'gc>>),
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
    /// "Unwinding").
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
/// `docs/design/compiler.md`, "How the limit counts", but for the reserve
/// above it (`DEPTH_RESERVE`). It bounds memory, not the native stack, which
/// the trampoline no longer recurses on: with the exempt block activations,
/// at most one per level of block nesting above each counted one, the stack
/// holds at most 129 x (L + `DEPTH_RESERVE`) entries -- 128 being the
/// parser's nesting ceiling, which `image::decode` holds an artifact to as
/// well (`docs/design/execution.md`, "The depth limit").
pub const MAX_ACTIVATION_DEPTH: usize = 100_000;

/// How far past L the stack may grow while a VM error raised near L is
/// being signalled: `signal`, the handler search and the handler itself are
/// activations too, and without room for them an error raised a few
/// activations below L -- L's own error included -- could only end the run
/// as L, uncatchable and under the wrong name. CPython does the same with
/// its `RecursionError`, allowing a little headroom above the recursion
/// limit until the depth falls back below it, and HotSpot with its stack's
/// yellow zone, unprotected for the `StackOverflowError`'s handler and
/// protected again once the stack has unwound past it. Here the reserve is
/// taken when a VM error is signalled within this many activations of L,
/// and given back once that `signal` activation is gone
/// (`Mutator::depth_reserve_held`); a push past it ends the run
/// unsignalled, since signalling needs the room that has just run out
/// (`Mutator::overrun_of_the_reserve`).
pub const DEPTH_RESERVE: usize = 1_000;

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
    /// A control-flow step's block is marked by its `arguments`,
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
    /// Activate `method`, as `Step::Method` with owned arguments does, and
    /// do `then` with its value: how a native loop sends `value` or
    /// `value:` to an action that is no block and carries on afterwards
    /// (`Mutator::send_value_then`). Boxed, so that `Step` keeps the size
    /// the hot path was measured at.
    MethodThen(Box<MethodThen<'gc>>),
}

/// What `Step::MethodThen` activates: a kernel method on `receiver` with
/// `arguments`, `defining_class` its current class, and the continuation
/// its activation returns to.
pub(crate) struct MethodThen<'gc> {
    pub(crate) receiver: Value<'gc>,
    pub(crate) method: Rc<crate::mixin::MixinMethod>,
    pub(crate) defining_class: ClassReference<'gc>,
    pub(crate) arguments: Vec<Value<'gc>>,
    pub(crate) then: Continuation<'gc>,
}

/// Where an activation's arguments come from.
pub(crate) enum StepArguments<'gc> {
    /// The send's own arguments.
    Send,
    /// None, for a control-flow step: the block activated for `ifTrue:` and
    /// its three relatives, `and:` and `or:` on a Boolean receiver -- answered
    /// by `True`'s and `False`'s `^<parameter> value` quick methods
    /// (`MethodFastPath::ValueOfParameter`) -- or for `whileTrue:` and
    /// `whileFalse:` -- answered by their leaves. It is the activation the
    /// optimized lowering inlines away, and which `push_step` therefore
    /// exempts from the depth count when the send was a `SendInlinable`.
    /// Only those build it, and the quick method only for a Boolean
    /// receiver: an instance of a subclass of `True` is never inlined.
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
    /// Every built shipped module's one instance, the kernel's first, and
    /// every shipped object under its name: the kernel's and the platform's
    /// built at boot, the others as a program or a load first demands them,
    /// and an entry never changed once recorded (`interpreter::shipped_world`).
    pub(crate) shipped_world: shipped_world::ShippedWorld<'gc>,
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
    /// `Kernel.ns`'s `Behavior` mixin, recorded by the kernel phase before
    /// `Behavior`'s own application is built, so that `apply_mixin` can
    /// refuse to instantiate anything below it
    /// (`InstantiationRefusal::ClassObject`).
    pub(crate) behavior_mixin: Option<Rc<crate::mixin::Mixin>>,
    /// Whether `new_class_object` builds a metaclass with every class
    /// object; false only during the kernel phase, before `Class` is filled
    /// (`Mutator::give_existing_class_objects_metaclasses`).
    pub(crate) metaclasses_ready: bool,
    /// (the mixin's id, the superclass application's full-width index) ->
    /// behaviour. Keyed on the superclass's *application*, never on a
    /// superclass class object: a nested superclass has one class object per
    /// enclosing instance, so a class-object key would never hit and every
    /// instance would get its own behaviour. Strong until live editing, as
    /// the heap's tables are.
    pub(crate) applications: IdentityMap<
        (crate::heap::MixinId, Option<crate::heap::ApplicationIndex>),
        Rc<crate::mixin::MixinApplication>,
    >,
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
    /// Keyed by the instance side's `MixinId`. A memo of the id's tree, not
    /// a holder: a full collection that frees the tree drops its entries
    /// (`Mutator::reclaim_ids`), so a key never names another mixin minted
    /// at a freed id.
    pub(crate) metaclass_mixins: IdentityMap<crate::heap::MixinId, Rc<crate::mixin::Mixin>>,
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
    /// Where the boot takes the shipped modules' compiled mixins from. Fixed
    /// at construction, as `lowering` is.
    pub(crate) shipped_code_path: ShippedCodePath,
    /// The reusable argument buffer the run loop gathers a send's arguments
    /// into (`vm::send`). Taken while a send is decided and pushed and given
    /// back afterwards, because the decision needs `&mut Mutator` while
    /// it reads the arguments. Empty between instructions: it is cleared
    /// when it is given back, so it holds no handle a move would have to
    /// find.
    pub(crate) argument_buffer: Vec<Value<'gc>>,
    /// The dispatch ids `apply_mixin` mints: the smallest freed one first,
    /// else the next never minted, below `DispatchId::LIMIT` (22 bits, the
    /// header word's guard field). Once exhausted applications get `NONE`,
    /// which is never cached: slower, never wrong, and a collection is
    /// requested so that the ids of dead applications come back.
    pub(crate) dispatch_ids: crate::heap::DispatchIdAllocator,
    /// The number the next nested-class creation takes as its marker
    /// (`nested::begin_creation`), from 1. A `u64` counts creations beyond
    /// any run, and `begin_creation` checks the `SmallInteger` range.
    pub(crate) next_creation_serial: u64,
    /// The inline caches (`inline_cache.rs`): per heap, since a dispatch id
    /// means nothing in another heap.
    pub(crate) caches: inline_cache::InlineCaches,
    /// How many more unhandled exceptions `defaultAction` resumes with
    /// `nil` instead of ending the run: the test hook
    /// `Interpreter::resume_unhandled_exceptions`, standing in for the
    /// debugger. Zero outside a test that asks.
    pub(crate) unhandled_exceptions_to_resume: usize,
    /// The depth limit's reserve (`DEPTH_RESERVE`), while it is held: the
    /// frame of the activation a VM error arose in, whose `signal` -- or,
    /// for a message nothing understood, whose `Object>>doesNotUnderstand:`
    /// -- sits directly above it (`Mutator::depth_reserve_held`). A frame
    /// and not a stack index, because a frame is divorced when its
    /// activation is popped and married to nothing again, where an index is
    /// reused -- by another error's `signal`, possibly far from the limit,
    /// which must not inherit the reserve. Released when a host's run ends,
    /// and forgotten whenever it is found no longer held.
    pub(crate) depth_reserve: Option<FrameReference<'gc>>,
    /// Whether `unwind::start_signalling` is pushing the `signal` that
    /// takes the reserve: the one moment the reserve is held with nothing
    /// above its activation yet.
    pub(crate) taking_depth_reserve: bool,
    /// Whether this heap has made a future (`Mutator::new_future`): set
    /// once the first one is made, never cleared. A number, not a handle,
    /// so nothing traces it.
    pub(crate) futures_made: bool,
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
    stress_collect: Option<safepoint::StressSchedule>,
    /// The scavenge stress mode's schedule, when it is on
    /// (`NS_NEXT_STRESS_SCAVENGE`, or `set_stress_scavenge`): a check of the
    /// remembered set and a scavenge at the safepoints its schedule picks,
    /// paced like the collect stress mode since the check walks the old
    /// space. Debug builds only.
    #[cfg(debug_assertions)]
    stress_scavenge: Option<safepoint::StressSchedule>,
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
        let path = crate::shipped_code::path_for(
            lowering,
            crate::compiler::primitive_method::disabled(),
            crate::shipped_code::embedded(),
        );
        Interpreter::bootstrap_on(lowering, path)
    }

    /// `bootstrap_with`, taking the shipped modules' compiled mixins from
    /// `path`.
    pub(crate) fn bootstrap_on(lowering: Lowering, path: ShippedCodePath) -> Interpreter {
        let mut interpreter = Interpreter::with_kernel(lowering, path);
        interpreter.install_shipped_modules();
        interpreter
    }

    /// An interpreter as the kernel phase leaves it: the kernel wired in,
    /// no other shipped module booted. `bootstrap_on` goes on from here;
    /// a test stops here to see what the kernel phase alone built.
    pub(crate) fn with_kernel(lowering: Lowering, path: ShippedCodePath) -> Interpreter {
        // Only the debug build's stress modes write to these fields below.
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut interpreter = Interpreter {
            world: crate::heap::World::new(lowering, path),
            running: false,
            #[cfg(debug_assertions)]
            stress: safepoint::StressMoves::from_env(),
            #[cfg(debug_assertions)]
            stress_caches: safepoint::stress_caches_from_env(),
            #[cfg(debug_assertions)]
            stress_collect: safepoint::StressSchedule::from_env("NS_NEXT_STRESS_COLLECT"),
            #[cfg(debug_assertions)]
            stress_scavenge: safepoint::StressSchedule::from_env("NS_NEXT_STRESS_SCAVENGE"),
            #[cfg(debug_assertions)]
            cache_invalidations: 0,
        };
        #[cfg(debug_assertions)]
        {
            interpreter.hold_safepoints_for_stress();
            // A fixture run small enough that every insert contends for the
            // same group (`run_goldens.rs`); anything not twice a decimal
            // power of two is ignored rather than refused, since this reads
            // the ambient environment rather than an explicit host call.
            if let Some(entries) = safepoint::global_cache_entries_from_env() {
                interpreter.set_global_cache_size(entries);
            }
        }
        // Last, after the stress modes are set up: the kernel's own
        // instantiation is a run like any other, and they should see it, as
        // they see the shipped modules' that `bootstrap_with` boots next.
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
    ///     let array = mutator.new_array(vec![Value::small_integer(1)]);
    ///     mutator.stash(Value::object(array))
    /// });
    /// let _ = interpreter.send(&root, Symbol::intern("size"), &[]);
    /// let length = interpreter.enter(|mutator| {
    ///     let array = mutator.fetch(&root).as_object()?;
    ///     mutator.heap().array_element_count(array)
    /// });
    /// assert_eq!(length, Some(1));
    /// ```
    ///
    /// Keeping the handle itself instead does not compile:
    ///
    /// ```compile_fail
    /// use ns_runtime::{Interpreter, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let array = interpreter.enter(|mutator| mutator.new_array(vec![Value::small_integer(1)]));
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
    ///     host.kept = Some(mutator.stash(Value::object(array)));
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
    ///     host.kept = Some(Value::object(array));
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
    /// let three = interpreter.stash(Value::small_integer(3));
    /// let printed = interpreter.send(&three, Symbol::intern("printString"), &[]);
    /// assert!(printed.is_ok());
    /// ```
    ///
    /// ```compile_fail
    /// use ns_runtime::{Interpreter, Symbol, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let three = interpreter.stash(Value::small_integer(3));
    /// interpreter.enter(|_mutator| {
    ///     let _ = interpreter.send(&three, Symbol::intern("printString"), &[]);
    /// });
    /// ```
    #[inline(always)]
    pub fn enter<R>(&mut self, body: impl for<'gc> FnOnce(&mut Mutator<'gc>) -> R) -> R {
        self.world.enter(body)
    }

    /// Roots an immediate -- `nil`, a Boolean or a `SmallInteger` -- a host
    /// built itself, outside any entry; a float is made only inside one, by
    /// `Mutator::new_float`, since one outside the immediate range is boxed.
    /// An object is rooted with `Mutator::stash`, inside the entry that
    /// reached it. A host can build an immediate but never hold an object
    /// handle outside an entry, so a `Value<'static>` holds none.
    pub fn stash(&self, value: Value<'static>) -> Root {
        self.world.stash(value)
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

    /// Builds the class `nested` declares in the class of `enclosing`, as
    /// its implicit accessor would, whatever the accessor's access: the boot
    /// builds every class a shipped root declares this way, private ones
    /// included, which a host send would be refused
    /// (`interpreter::shipped_world`). The declaring class is `enclosing`'s
    /// own class, which holds for a shipped module instance: every shipped
    /// class is declared directly in its root, and the instance's class is
    /// the root class. Answers the class object, or the error its superclass
    /// clause raised.
    pub(crate) fn build_nested_class(
        &mut self,
        enclosing: &Root,
        nested: &Rc<crate::mixin::Mixin>,
    ) -> HostResult {
        let depth = self.world.activation_depth();
        let start = self.enter(|mutator| {
            let receiver = mutator.fetch(enclosing);
            let declaring_class = match receiver.as_object() {
                Some(object) => mutator.heap().object_class(object),
                None => unreachable!("internal: a shipped module instance is an object"),
            };
            let decided = nested::nested_class(mutator, receiver, nested, declaring_class);
            mutator.start(decided, &[])
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
                // Every activation of the run has been popped, so whatever
                // held the reserve is gone: released here rather than left
                // to the next check to find so.
                self.enter(|mutator| mutator.depth_reserve = None);
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
            let Some(object) = mutator.fetch(class).as_object() else {
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

    /// Whether an explicit send of `selector` to `receiver` finds a public
    /// member (§5.5's `lookupPublic`), without sending it: the CLI asks this of
    /// a manifest, whose own public members would answer a module's name before
    /// its `doesNotUnderstand:` could.
    pub fn responds_publicly(&mut self, receiver: &Root, selector: Symbol) -> bool {
        self.enter(|mutator| {
            let receiver = mutator.fetch(receiver);
            let class = mutator.class_of(receiver);
            !matches!(
                crate::dispatch::lookup_public(mutator.heap(), class, selector),
                crate::dispatch::Lookup::NotFound
            )
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

    /// The object space's occupancy. Available in every build: it is what a
    /// measurement and a retention test both read.
    pub fn heap_statistics(&self) -> crate::HeapStatistics {
        self.world.heap_statistics()
    }

    /// The collector's counts and pauses since the process started, plus the
    /// object space's occupancy right now (`--collector-statistics`). Available in
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

    /// A test hook standing in for the debugger, not an input path a
    /// program can reach: the next `count` unhandled exceptions are resumed
    /// with `nil` by `defaultAction`'s leaf instead of ending the run --
    /// what the debugger will do once it has let the user fix the cause
    /// (`docs/design/core-library.md`, "Exceptions"). A
    /// `MessageNotUnderstood` resumed this way sends its message again.
    #[doc(hidden)]
    pub fn resume_unhandled_exceptions(&mut self, count: usize) {
        self.enter(|mutator| mutator.unhandled_exceptions_to_resume = count);
    }

    /// How many of the exceptions `resume_unhandled_exceptions` asked for
    /// are still to be resumed. A test hook, as that one is.
    #[doc(hidden)]
    pub fn unhandled_exceptions_left_to_resume(&mut self) -> usize {
        self.enter(|mutator| mutator.unhandled_exceptions_to_resume)
    }
}

#[cfg(debug_assertions)]
impl Interpreter {
    /// Turns the cache stress mode on or off for this interpreter, whatever
    /// `NS_NEXT_STRESS_CACHES` says. For tests. Keeps the safepoint request
    /// set while any other stress mode is on (`hold_safepoints_for_stress`).
    pub fn set_stress_caches(&mut self, enabled: bool) {
        self.stress_caches = enabled;
        self.hold_safepoints_for_stress();
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
        let activation = self.builtin_class(BuiltinClass::Activation);
        self.heap.set_activation_class(activation);
        self.refresh_immediate_dispatch_ids();
        let class = self.builtin_class(BuiltinClass::VmMirror);
        let mirror = self.heap.allocate_instance(class, 0);
        self.vm_mirror = Some(Value::object(mirror));
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
            instances(self, BuiltinClass::UndefinedObject),
            instances(self, BuiltinClass::False),
            instances(self, BuiltinClass::True),
            instances(self, BuiltinClass::Integer),
            instances(self, BuiltinClass::Float),
        ];
        self.heap.set_immediate_dispatch_ids(ids);
    }

    /// The smallest freed dispatch id, else the next never minted, or
    /// `DispatchIds::NONE` once every id below `DispatchId::LIMIT` is in use:
    /// an application built after that point is simply never cached, which
    /// is slower but never wrong, so running out is not an error. Ids start
    /// at `DispatchId::FIRST_MINTED`, above the reserved guards, and stop
    /// below `DispatchId::LIMIT`, which is what keeps
    /// `DispatchId::from_counter`'s assertion from ever firing on a minted
    /// id. Freed ids come back only in a full collection (`reclaim_ids`).
    ///
    /// One id per application: a class object's class is its metaclass, an
    /// application of its own, so no second, class-side id goes with it.
    pub(crate) fn mint_dispatch_ids(&mut self) -> crate::mixin::DispatchIds {
        match self.dispatch_ids.mint() {
            Some(instances) => crate::mixin::DispatchIds { instances },
            None => {
                // Freed ids come back only in a full collection, which
                // nothing but the old space's growth would otherwise start:
                // a program that makes and drops applications quickly would
                // run out with plenty of dead ones to reclaim.
                self.heap.request_collection();
                crate::mixin::DispatchIds::NONE
            }
        }
    }

    /// After `image::load`: never hand out an identity hash a loaded object
    /// already carries (`docs/design/image-format.md`, "Sections").
    ///
    /// `max < IDENTITY_HASH_LIMIT` is the caller's guarantee, and an
    /// enforced one: `image::decode` refuses a hash the header word cannot
    /// hold (`DecodeError::IdentityHashTooWide`). Past the widest hash the
    /// counter wraps to 1 (`Heap::advance_identity_hash_past`).
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
    fn host_error(&self, error: NewspeakError<'gc>) -> HostSignal {
        HostSignal::Error(HostError {
            selector: error.selector,
            receiver: self.stash(error.receiver),
            message: String::from(error.message),
        })
    }

    pub(crate) fn to_host(&self, result: EvaluationResult<'gc>) -> HostResult {
        match result {
            Ok(value) => Ok(self.stash(value)),
            Err(Signal::Error(error)) => Err(self.host_error(error)),
            Err(Signal::Terminate(error)) => Err(self.host_error(*error)),
            // A run's unwinder leaves with neither a restart nor a
            // termination (`unwind::exit_signal`); only a leaf a host's own
            // send to the mirror decided, before any run, comes here with
            // one. A restart then aimed, like such a `^`, at no activation
            // of the host's.
            Err(Signal::Return(..) | Signal::Restart(_)) => Err(HostSignal::Return),
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
    ///
    /// The check is the one compare it always was: the reserve is looked at
    /// only once the depth has reached L (`push_past_the_limit`), so an
    /// error far from L, and every push below it, pays nothing for it.
    pub(crate) fn push(
        &mut self,
        header: ActivationHeader<'gc>,
        arguments: &[Value<'gc>],
        temporaries: usize,
    ) -> Result<ActivationReference, Signal<'gc>> {
        if header.counted && self.heap.counted_depth() >= MAX_ACTIVATION_DEPTH {
            return self.push_past_the_limit(header, arguments, temporaries);
        }
        Ok(self.heap.push_activation(header, arguments, temporaries))
    }

    /// A counted push at or past L. While a VM error's `signal` holds the
    /// reserve, the push is allowed up to `DEPTH_RESERVE` further; past it,
    /// the run ends (`overrun_of_the_reserve`). Otherwise it is refused
    /// with L's error, which the unwinder signals -- taking the reserve --
    /// unless it is the push of the `doesNotUnderstand:` that has just
    /// taken it (`take_depth_reserve_for_does_not_understand`).
    #[cold]
    #[inline(never)]
    fn push_past_the_limit(
        &mut self,
        header: ActivationHeader<'gc>,
        arguments: &[Value<'gc>],
        temporaries: usize,
    ) -> Result<ActivationReference, Signal<'gc>> {
        // The push that makes a freshly taken reserve held is asked about
        // first: `depth_reserve_held` would find nothing above its activation
        // yet, and forget it.
        let refusal = if !self.pushes_the_reserves_does_not_understand(&header)
            && !self.depth_reserve_held()
        {
            recursion_too_deep(&header)
        } else if self.heap.counted_depth() < MAX_ACTIVATION_DEPTH + DEPTH_RESERVE {
            return Ok(self.heap.push_activation(header, arguments, temporaries));
        } else {
            Signal::Terminate(Box::new(self.overrun_of_the_reserve(&header)))
        };
        // A refused activation never runs, so a future it was to compute is
        // unresolved again, as `abandon` leaves one whose activation an
        // unwind pops. Only a forcing is undone here: every other
        // continuation that has begun something -- a nested class's
        // creation -- begins it once its push has succeeded.
        if let Continuation::Future(forcing) = header.then {
            self.future_abandoned(*forcing);
        }
        Err(refusal)
    }

    /// Whether the depth limit's reserve is held: its activation is still
    /// running, and directly above it is the `signal` of the VM error that
    /// arose there -- or its `Object>>doesNotUnderstand:` -- or that
    /// `signal` is being pushed. Forgets a reserve found no longer held, so
    /// that the pop that ended it pays nothing to give it back.
    pub(crate) fn depth_reserve_held(&mut self) -> bool {
        let Some(frame) = self.depth_reserve else {
            return false;
        };
        let held = self.heap.frame_activation(frame).is_some_and(|activation| {
            self.taking_depth_reserve
                || self
                    .heap
                    .activation_above(activation)
                    .is_some_and(|above| self.holds_the_depth_reserve(above))
        });
        if !held {
            self.depth_reserve = None;
        }
        held
    }

    /// Whether `activation` is one the reserve is taken for: the `signal`
    /// the unwinder pushed for a VM error (`Continuation::VmErrorSignalled`),
    /// or the kernel's own `Object>>doesNotUnderstand:` -- §5.5's error, the
    /// VM's as much as any other, though `Kernel.ns` signals it.
    pub(crate) fn holds_the_depth_reserve(&self, activation: ActivationReference) -> bool {
        self.heap.activation_signals_a_vm_error(activation)
            || self.is_kernel_does_not_understand(activation)
    }

    /// Whether `activation` runs the kernel's `Object>>doesNotUnderstand:`,
    /// not an override of it: the one method of that selector whose current
    /// class is `Object` itself.
    pub(crate) fn is_kernel_does_not_understand(&self, activation: ActivationReference) -> bool {
        let object = self.builtin_class(BuiltinClass::Object);
        match self.heap.activation_kind(activation) {
            FrameKind::Method(method) => {
                method.selector == Symbol::DOES_NOT_UNDERSTAND
                    && self
                        .heap
                        .activation_current_class(activation)
                        .is_some_and(|class| class.same(object))
            }
            _ => false,
        }
    }

    /// Whether `header` is the kernel's `Object>>doesNotUnderstand:`, about
    /// to be pushed directly above the activation that has just taken the
    /// reserve for it: the decision took the reserve, and this is the push
    /// that makes it held.
    fn pushes_the_reserves_does_not_understand(&self, header: &ActivationHeader<'gc>) -> bool {
        let Some(frame) = self.depth_reserve else {
            return false;
        };
        let Some(sender) = self.heap.frame_activation(frame) else {
            return false;
        };
        let object = self.builtin_class(BuiltinClass::Object);
        let is_kernel_handler = matches!(&header.kind,
            FrameKind::Method(method) if method.selector == Symbol::DOES_NOT_UNDERSTAND)
            && header.current_class.is_some_and(|class| class.same(object));
        is_kernel_handler
            && self
                .heap
                .top_activation()
                .is_some_and(|top| top.same(sender))
    }

    /// Takes the depth limit's reserve for a `doesNotUnderstand:` about to
    /// be pushed above the top activation, as `unwind::start_signalling`
    /// takes it for a VM error's `signal`: when the kernel's handler runs,
    /// the reserve is not held, and the counted depth is within
    /// `DEPTH_RESERVE` of the limit. Called by the decision, which pushes
    /// nothing; the push follows it at once.
    pub(crate) fn take_depth_reserve_for_does_not_understand(
        &mut self,
        defining_class: ClassReference<'gc>,
    ) {
        if !defining_class.same(self.builtin_class(BuiltinClass::Object))
            || self.heap.counted_depth() < MAX_ACTIVATION_DEPTH - DEPTH_RESERVE
            || self.depth_reserve_held()
        {
            return;
        }
        if let Some(top) = self.heap.top_activation() {
            self.depth_reserve = Some(self.heap.reify(top));
        }
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
                let (code, captured_frame, closure_feedback_base) = self
                    .heap
                    .closure_activation(closure)
                    .expect("internal: a block step is decided only for a closure object");
                if let Err(signal) = check_block_arity(
                    &self.heap,
                    code.argument_count as usize,
                    captured_frame,
                    Value::object(closure),
                    arguments.len(),
                ) {
                    // The block never runs, so its continuation is thrown
                    // away as a popped one's is: a future being forced is
                    // unresolved again rather than left resolving.
                    self.abandon(then);
                    return Err(signal);
                }
                // A closure's range is minted once, the first time it is
                // activated: every closure `MakeClosure` creates already
                // carries its method's family's base (below), so
                // `NOT_ASSIGNED` here means a closure an image decoded,
                // which never ran `MakeClosure` at all.
                let feedback_base = if closure_feedback_base == FeedbackBase::NOT_ASSIGNED {
                    let fresh = self.caches.fresh_range(code.family_size);
                    self.heap.closure_set_feedback_base(closure, fresh);
                    fresh
                } else {
                    closure_feedback_base
                };
                // Exempt exactly the activation the optimized lowering
                // inlines away: a control-flow step's block activated for
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
                let (enclosing, index, serial) =
                    (creation.enclosing, creation.index, creation.serial);
                let activation = self.push(
                    ActivationHeader {
                        receiver: Value::object(enclosing),
                        current_class: Some(creation.declaring_class),
                        kind: FrameKind::SuperclassClause(creation.declaring_class),
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
                nested::begin_creation(self, enclosing, index, serial);
                Ok(activation)
            }
            // Pushed as `Step::Method` pushes it, and only then given its
            // continuation, as the unwinder gives `valueOnUnwind:` its own
            // (`Mutator::start_unwind_block`).
            Step::MethodThen(method_then) => {
                let MethodThen {
                    receiver,
                    method,
                    defining_class,
                    arguments,
                    then,
                } = *method_then;
                let pushed = self.push_step(
                    Step::Method {
                        receiver,
                        method,
                        defining_class,
                        arguments: StepArguments::Owned(arguments),
                        feedback_base: FeedbackBase::NOT_ASSIGNED,
                    },
                    send_arguments,
                    sender,
                    inlinable,
                );
                // A refused push never runs, so `then` is thrown away as a
                // popped activation's continuation is.
                let activation = match pushed {
                    Ok(activation) => activation,
                    Err(signal) => {
                        self.abandon(then);
                        return Err(signal);
                    }
                };
                self.heap.activation_set_continuation(activation, then);
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
        let code = self.initializer_body(level);
        let feedback_base = self.caches.base_for(&code);
        let temporaries = (code.register_count - code.argument_count) as usize;
        self.push(
            ActivationHeader {
                receiver,
                current_class: Some(level),
                kind: FrameKind::Initializer(level),
                code,
                then,
                counted: true,
                feedback_base,
            },
            arguments,
            temporaries,
        )
    }

    /// What a native does once the block it activated has returned
    /// `value`: the second half of the native, which the run loop
    /// waits for instead of a Rust frame. `then` is the continuation of the
    /// block's activation, which the loop has just popped. It answers the
    /// value for the caller, or another activation to run in the popped
    /// one's place -- a native loop's next block, or the `valueOf:` that
    /// runs a body which is no block -- which the loop hands to
    /// `push_continuation`; or the signal of a non-Boolean condition's
    /// `NonBooleanReceiver`.
    pub(crate) fn resume(
        &mut self,
        then: Continuation<'gc>,
        value: Value<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        match then {
            Continuation::Deliver => Ok(Step::Value(value)),
            Continuation::WhileCondition(state) => {
                crate::builtins::block::after_condition(self, state, value)
            }
            Continuation::WhileBody(state) => Ok(crate::builtins::block::after_body(state)),
            Continuation::DoElement(state) => {
                Ok(crate::builtins::collections::after_element(self, state))
            }
            Continuation::NestedClass(creation) => {
                nested::end_creation(self, &creation);
                nested::after_clause(self, *creation, value)
            }
            Continuation::Instantiate(state) => self.advance_instantiation(state, value),
            // The block's value is dropped: the unwind it ran for goes on,
            // raised again as the signal it was, from the activation it had
            // stopped at -- now the top of the stack again.
            Continuation::Unwind(pending) => Err(unwind::signal_of(*pending)),
            Continuation::VmErrorSignalled(error) => Err(Signal::Terminate(error)),
            Continuation::Future(_) => {
                unreachable!("internal: vm::finish handles a future's continuation itself")
            }
        }
    }

    /// What a continuation undoes when its activation is thrown away rather
    /// than returned from: the counterpart of `resume`, which `unwind::unwind`
    /// calls for every activation it pops, and `push_step` for a block
    /// activation it refuses. Only a nested class's creation and a future's
    /// forcing have begun anything -- the creation's cache slot is marked as
    /// in progress, the future as resolving -- so only they have anything to
    /// undo; every other continuation is simply dropped, a
    /// `Continuation::Unwind` included: a newer unwind, or an error, has
    /// replaced the one it would have carried on.
    pub(crate) fn abandon(&mut self, then: Continuation<'gc>) {
        match then {
            Continuation::NestedClass(creation) => nested::end_creation(self, &creation),
            Continuation::Future(forcing) => self.future_abandoned(*forcing),
            _ => {}
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
        // and a continuation decides none: the one send it makes, a body's
        // `value`, is a `Step::MethodThen`, which owns its arguments), so
        // `&[]` is always right here.
        self.push_step(step, &[], sender, !replaced_counted)
    }

    /// The step that sends `value` to `action`, or `value:` with
    /// `argument`, through `Closure`'s private `valueOf:` or
    /// `valueOf:with:` activated on `receiver`, and does `then` with the
    /// answer. A send's decision may be a method, a block, a value or an
    /// instantiation of several activations; the helper's own return is
    /// the one place to hang `then` on for all of them.
    pub(crate) fn send_value_then(
        &mut self,
        receiver: Value<'gc>,
        action: Value<'gc>,
        argument: Option<Value<'gc>>,
        then: Continuation<'gc>,
    ) -> Step<'gc> {
        let closure_class = self.builtin_class(BuiltinClass::Closure);
        let (selector, arguments) = match argument {
            None => (Symbol::VALUE_OF, vec![action]),
            Some(argument) => (Symbol::VALUE_OF_WITH, vec![action, argument]),
        };
        let method = self
            .heap
            .class_application(closure_class)
            .and_then(|application| application.mixin.compiled_method(selector).cloned())
            .expect("internal: Kernel.ns's Closure declares valueOf: and valueOf:with:");
        Step::MethodThen(Box::new(MethodThen {
            receiver,
            method,
            defining_class: closure_class,
            arguments,
            then,
        }))
    }

    /// Whether a `^` aimed at `activation` ends at `activation`: a method's
    /// does, and a superclass clause's or its message's, a `<:` clause's
    /// message included. An initializer catches
    /// none -- a `^` into a running one escapes, the known gap in
    /// `docs/design/execution.md`, "Unwinding" -- and a block's `^`
    /// always targets its home, never the block.
    pub(crate) fn catches_return(&self, activation: ActivationReference) -> bool {
        self.heap.activation_runs_method_or_clause(activation)
    }

    /// Where a `^` in `activation` returns to. A block returns from its home --
    /// the method or initializer that lexically encloses it -- and only
    /// while that activation is still running: its reified frame is married
    /// to a live stack entry exactly until the activation returns, and a
    /// frame the image decoder built never is. A dead home is §7.2's
    /// `cannotReturn`. Every other activation's `^` -- an initializer's from
    /// an inlined block -- returns from itself, reified here so that the
    /// signal can name it by frame.
    pub(crate) fn raise_return(
        &mut self,
        activation: ActivationReference,
        value: Value<'gc>,
    ) -> Signal<'gc> {
        let target = match self.heap.activation_home(activation) {
            None => self.heap.reify(activation),
            Some(home) => match self.heap.frame_activation(home) {
                Some(_) => home,
                None => {
                    return Signal::Error(NewspeakError::cannot_return(
                        Symbol::CARET,
                        value,
                        value,
                        "cannotReturn: the enclosing method activation has already returned",
                    ))
                }
            },
        };
        Signal::Return(target, value)
    }

    pub fn class_of(&self, value: Value<'gc>) -> ClassReference<'gc> {
        match value.as_object() {
            Some(object) => self.heap.object_class(object),
            // Nil/Boolean/SmallInteger/Float classes are wired up by the
            // bootstrap into interpreter-held globals, read from there via
            // `Mutator::builtin_class(kind)`.
            _ => self.builtin_class_for_primitive(value),
        }
    }

    fn builtin_class_for_primitive(&self, value: Value<'gc>) -> ClassReference<'gc> {
        let kind = match value.kind() {
            ValueKind::Nil => BuiltinClass::UndefinedObject,
            ValueKind::Boolean(true) => BuiltinClass::True,
            ValueKind::Boolean(false) => BuiltinClass::False,
            ValueKind::SmallInteger(_) => BuiltinClass::Integer,
            ValueKind::SmallFloat(_) => BuiltinClass::Float,
            ValueKind::Object(_) => unreachable!("an object carries its own class"),
        };
        self.builtin_class(kind)
    }

    pub fn new_array(&mut self, elements: Vec<Value<'gc>>) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Array);
        self.heap.allocate_array(class, &elements)
    }

    /// A `ByteArray` of the kernel's class holding `bytes`, in order.
    pub fn new_byte_array(&mut self, bytes: &[u8]) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::ByteArray);
        self.heap.allocate_byte_array(class, bytes)
    }

    pub fn new_string(&mut self, string: String) -> ObjectReference<'gc> {
        self.new_string_from_code_points(crate::CodePoints::from(string))
    }

    pub fn new_string_from_code_points(
        &mut self,
        code_points: crate::CodePoints,
    ) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::String);
        self.heap.allocate_string(class, code_points)
    }

    /// The one way an `f64` becomes a `Value`, so that every float has one
    /// representation: one inside the `SmallFloat64` range is immediate,
    /// every other a boxed `Float`, which `Float>>=` and `hash` rely on
    /// (`docs/design/object-model.md`, "Values and objects").
    pub fn new_float(&mut self, value: f64) -> Value<'gc> {
        if crate::heap::is_small_float_bits(value.to_bits()) {
            Value::small_float(value)
        } else {
            let class = self.builtin_class(BuiltinClass::Float);
            Value::object(self.heap.allocate_float(class, value))
        }
    }

    /// The one way an `ArbitraryInteger` becomes a `Value`, so that every
    /// integer has one representation: one in 61 bits is a `SmallInteger`,
    /// never a `LargeInteger` object, which `Integer>>=` and `hash` rely on.
    pub fn new_integer(&mut self, integer: crate::ArbitraryInteger) -> Value<'gc> {
        match integer
            .to_i64()
            .filter(|number| fits_small_integer(*number))
        {
            Some(small) => Value::small_integer(small),
            None => {
                let class = self.builtin_class(BuiltinClass::LargeInteger);
                Value::object(self.heap.allocate_large_integer(class, integer))
            }
        }
    }

    /// A `Fraction` holding `rational`, which must not be an integer:
    /// each part through `new_integer`, so a part that fits is a
    /// `SmallInteger`. The one way a fraction is made.
    pub(crate) fn new_fraction(&mut self, rational: crate::numeric::Rational) -> Value<'gc> {
        debug_assert!(!rational.is_integer());
        let (numerator, denominator) = rational.into_parts();
        // Allocation never collects -- a collection runs only at a
        // safepoint -- so both parts stay valid across the third.
        let numerator = self.new_integer(numerator);
        let denominator = self.new_integer(denominator);
        let class = self.builtin_class(BuiltinClass::Fraction);
        Value::object(self.heap.allocate_fraction(class, numerator, denominator))
    }

    /// `rational` as the number it is: an integer when its denominator is
    /// 1, a `Fraction` otherwise.
    pub(crate) fn new_rational(&mut self, rational: crate::numeric::Rational) -> Value<'gc> {
        if rational.is_integer() {
            self.new_integer(rational.into_parts().0)
        } else {
            self.new_fraction(rational)
        }
    }

    /// The one object `symbol` is in this heap, allocated the first time
    /// anything asks for it.
    pub fn new_symbol(&mut self, symbol: Symbol) -> Value<'gc> {
        if let Some(object) = self.heap.canonical_symbol(symbol) {
            return Value::object(object);
        }
        let class = self.builtin_class(BuiltinClass::Symbol);
        let object = self.heap.allocate_symbol(class, symbol);
        self.heap.record_canonical_symbol(symbol, object);
        Value::object(object)
    }

    /// The one object `mixin` is in this heap, an instance of the
    /// kernel-private `Mixin`, allocated and recorded the first time
    /// anything asks for it, so that every request answers the same object
    /// (`docs/design/liveness-and-reflection.md`, "A mixin is reified as a
    /// kernel-private object"). A class side, declared or synthesized, is a
    /// mixin of its own and has an object of its own.
    pub fn reified_mixin(&mut self, mixin: &Rc<crate::mixin::Mixin>) -> ObjectReference<'gc> {
        let id = self.heap.mixin_id(mixin);
        if let Some(object) = self.heap.mixin_object(id) {
            return object;
        }
        let class = self.builtin_class(BuiltinClass::Mixin);
        let object = self.heap.allocate_mixin_object(class, id);
        self.heap.record_mixin_object(id, object);
        object
    }

    /// The mixin `object` stands for, when it is a reified mixin
    /// (`Heap::mixin_object_id`); `None` for any other object.
    pub fn mixin_of_reified(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<Rc<crate::mixin::Mixin>> {
        self.heap
            .mixin_object_id(object)
            .and_then(|id| self.heap.mixin(id))
    }

    pub fn new_closure(&mut self, closure: crate::heap::NewClosure<'gc>) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Closure);
        self.heap.allocate_closure(class, closure)
    }

    /// A closure running `code`, whose copied values are the registers
    /// `copied` names in the window at `register_base`, read straight into
    /// its block (`Heap::allocate_closure_from_registers`): `MakeClosure`'s.
    pub(crate) fn new_closure_from_registers(
        &mut self,
        code: &Rc<crate::bytecode::CompiledCode>,
        captured_frame: FrameReference<'gc>,
        feedback_base: FeedbackBase,
        register_base: crate::heap::RegisterBase,
        copied: &[u16],
    ) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Closure);
        self.heap.allocate_closure_from_registers(
            class,
            code,
            captured_frame,
            feedback_base,
            register_base,
            copied,
        )
    }

    /// The activation object of `frame`, which the caller has already
    /// reified (`Heap::reify`): the frame object itself, an instance of the
    /// kernel-private `Activation` class. It is a frame, never a running
    /// activation's stack entry, since only a reified frame survives its
    /// activation's pop.
    pub fn new_activation_object(&mut self, frame: FrameReference<'gc>) -> ObjectReference<'gc> {
        self.heap.frame_object(frame)
    }

    /// The `platform` object (`docs/design/core-library.md`, "The
    /// platform"): the instance of `PlatformFactory.ns`'s nested `Platform`
    /// the bootstrap built (`Interpreter::install_shipped_modules`) from the
    /// module instance it made with the VM mirror and the kernel instance,
    /// the same one for every reference within one interpreter. Asking for
    /// it, at bootstrap or in `image::load`, runs no Newspeak and fills no
    /// cache.
    pub(crate) fn platform_value(&self) -> Value<'gc> {
        self.platform_singleton
            .expect("internal: the platform is booted at bootstrap")
    }

    /// An instance of the kernel class `kind`, every slot its chain declares
    /// filled by name with what `slot_value` answers for it, built without
    /// running the factory, so that the run loop can make one -- a `Message` for a
    /// `doesNotUnderstand:` handler, an `Error` for a VM error -- in the
    /// middle of deciding a step, where no Newspeak can run.
    /// `slot_value` panics for a name it does not know, so that a slot
    /// `Kernel.ns` adds cannot go unfilled unnoticed.
    pub(crate) fn new_kernel_instance(
        &mut self,
        kind: BuiltinClass,
        mut slot_value: impl FnMut(&str) -> Value<'gc>,
    ) -> ObjectReference<'gc> {
        let class = self.builtin_class(kind);
        let application = self
            .heap
            .class_application(class)
            .expect("internal: a kernel class has an application");
        let instance = self.heap.allocate_instance(class, application.slot_count);
        let mut level = Some(application);
        while let Some(current) = level {
            for (position, slot) in current.mixin.slots.iter().enumerate() {
                let value = slot_value(slot.name.as_str());
                if !self
                    .heap
                    .instance_set_field(instance, current.own_slot_base + position, value)
                {
                    unreachable!("internal: a kernel class's slots lie inside its layout");
                }
            }
            level = current.superclass.clone();
        }
        instance
    }

    /// The slot `name` that the kernel class `kind` declares, read from
    /// `object` -- an instance of it or of a subclass, whose own slots of
    /// the same name are other slots. The level is found by its mixin, so
    /// the read does not depend on the slots a subclass adds. `None` for an
    /// object that is not such an instance.
    pub(crate) fn kernel_slot(
        &self,
        object: ObjectReference<'gc>,
        kind: BuiltinClass,
        name: &str,
    ) -> Option<Value<'gc>> {
        let field = self.heap.with_level_applying_mixin_of(
            self.class_of(Value::object(object)),
            self.builtin_class(kind),
            |application| {
                let position = application
                    .mixin
                    .slots
                    .iter()
                    .position(|slot| slot.name.as_str() == name)?;
                application.own_slot_base.checked_add(position)
            },
        )??;
        self.heap.instance_field(object, field)
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
        match (
            value.as_object(),
            self.vm_mirror.and_then(|mirror| mirror.as_object()),
        ) {
            (Some(object), Some(mirror)) => object.same(mirror),
            _ => false,
        }
    }

    /// A class object around an already-computed application. For an
    /// ordinary class object, this is where the invariant `dispatch::next_level`
    /// relies on is established: `superclass` is linked exactly when
    /// `application`'s own `superclass` is (callers derive `application`
    /// from `superclass` through `apply_mixin`), so the two chains it steps
    /// together stay the same length. A metaclass keeps it too: its
    /// superclass is `Class` and its application is applied over `Class`'s
    /// (`Mutator::attach_metaclass`).
    ///
    /// Once the kernel phase has filled `Class` (`metaclasses_ready`), the
    /// class object's metaclass is built here with it and becomes its
    /// header class (`Mutator::attach_metaclass`).
    pub(crate) fn new_class_object(
        &mut self,
        application: Rc<crate::mixin::MixinApplication>,
        enclosing: Option<ObjectReference<'gc>>,
        declaring_class: Option<ClassReference<'gc>>,
        superclass: Option<ClassReference<'gc>>,
    ) -> ClassReference<'gc> {
        let class = self.allocate_class_object(crate::heap::NewClass {
            application,
            enclosing,
            declaring_class,
            superclass,
            builtin: None,
        });
        if self.metaclasses_ready {
            self.attach_metaclass(class);
        }
        class
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
        let class = self.allocate_class_object(crate::heap::NewClass {
            application,
            enclosing,
            declaring_class,
            superclass,
            builtin: None,
        });
        if !self.heap.class_set_applied_clause(class, clause) {
            unreachable!("internal: a level is a class object");
        }
        if self.metaclasses_ready {
            self.attach_metaclass(class);
        }
        class
    }

    /// Allocates a class object under the builtin `Object`, a provisional
    /// header: `attach_metaclass` replaces it before `new_class_object`
    /// returns, except during the kernel phase, where
    /// `give_existing_class_objects_metaclasses` replaces it once `Class`
    /// is filled. A metaclass is allocated here too, and `attach_metaclass`
    /// makes it an instance of `Metaclass`.
    fn allocate_class_object(&mut self, data: crate::heap::NewClass<'gc>) -> ClassReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Object);
        self.heap.allocate_class(class, data)
    }

    pub fn apply_mixin(
        &mut self,
        mixin: &Rc<crate::mixin::Mixin>,
        superclass: Option<&Rc<crate::mixin::MixinApplication>>,
    ) -> Rc<crate::mixin::MixinApplication> {
        let id = self.heap.mixin_id(mixin);
        let key = (id, superclass.map(|application| application.index()));
        if let Some(existing) = self.applications.get(&key) {
            return existing.clone();
        }
        // The id's current version, whichever version the caller holds: an
        // enclosing mixin built before a replacement still holds the
        // version it was built with, and a class built from it now is the
        // replacement's.
        let current = self
            .heap
            .mixin(id)
            .expect("internal: a registered mixin has a row");
        let mixin = &current;
        let inherited = superclass.map_or(0, |superclass_application| {
            superclass_application.slot_count
        });
        // The nested-class cache slots. They are part of the instance layout
        // because the spec caches a nested class in the enclosing *object*;
        // a side table keyed by object would cost a hash lookup on a path
        // that is otherwise a slot read.
        //
        // Keyed by the declaration's id: a subclass that redeclares an
        // inherited nested class shadows it for a plain `N` but not for
        // `super N`, so the two need separate slots -- see `nested_slot`.
        let mut nested_slot = superclass
            .map_or_else(IdentityMap::default, |superclass_application| {
                superclass_application.nested_slot.clone()
            });
        let mut next = inherited + mixin.slots.len();
        for nested_id in self.heap.nested_of(id) {
            nested_slot.insert(*nested_id, next);
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
        // anything built on it, so it comes first: a root whose chain is
        // wrong has no layout worth checking. The mutable slot comes next,
        // behind `inherits_value` since a mixin applied over a superclass
        // that is no value keeps its mutable slots legally. §3.1.1's third
        // condition on a value class comes last, for the same reason and
        // behind the same guard -- the first level below `Value` -- `Value`'s
        // own excluded, since its next sentence allows that one, and every
        // level above `Value` too, whose `==` `Value` overrides -- whose
        // mixin declares an instance-side `==`
        // (`InstantiationRefusal::IdentityMethod`). Every verdict is
        // inherited the same way, through the superclass's own
        // `instantiation_refusal`: this makes the identity-method condition
        // a property of the *application* rather than of any class object
        // built from it, so `Mutator::load_root_class`, which applies a
        // module's root mixin through this same function but builds no
        // class object through `nested::after_clause`, is covered without a
        // check of its own -- unlike the enclosing-object condition, which
        // stays decided per class object
        // (`interpreter::value_class::check_value_class`,
        // `docs/design/object-model.md`, "A value object's four
        // conditions").
        let is_behavior_mixin = self
            .behavior_mixin
            .as_ref()
            .is_some_and(|behavior_mixin| Rc::ptr_eq(behavior_mixin, mixin));
        let instantiation_refusal = mixin
            .root_clause_defect
            .clone()
            .map(crate::mixin::InstantiationRefusal::RootClause)
            .or_else(|| {
                superclass.and_then(|application| application.instantiation_refusal.clone())
            })
            .or_else(|| {
                is_behavior_mixin.then_some(crate::mixin::InstantiationRefusal::ClassObject)
            })
            .or_else(|| {
                first_mutable_slot
                    .filter(|_| inherits_value)
                    .map(crate::mixin::InstantiationRefusal::MutableValueSlot)
            })
            .or_else(|| {
                (inherits_value
                    && !is_value_mixin
                    && mixin.compiled_method(Symbol::IDENTICAL).is_some())
                .then_some(mixin.name)
                .map(|declaring| crate::mixin::InstantiationRefusal::IdentityMethod { declaring })
            });
        // The declared slots, in field order, computed here for the same
        // reason `nested_slot` is: nothing about an application changes
        // after it is built, and `builtins/value.rs` reads this per slot on
        // the declining path. Declared and hidden slots interleave -- this
        // level's own slots sit at `inherited`, its cache slots after them
        // -- so a value's state is not a prefix of its fields.
        let declared_slots = crate::mixin::declared_slots_of(superclass, inherited, &mixin.slots);
        let dispatch_ids = self.mint_dispatch_ids();
        let application = self
            .heap
            .register_application(crate::mixin::MixinApplication {
                index: crate::heap::ApplicationIndex::UNREGISTERED,
                mixin: mixin.clone(),
                superclass: superclass.cloned(),
                inherits_value,
                first_mutable_slot,
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

    /// Installs `replacement` as the current version of the mixin `id`
    /// (`Heap::replace_mixin`), and forgets the applications and the
    /// synthesized class side built from the previous one, so that the
    /// next application of the id applies the replacement. Applications already built, and the class objects
    /// naming them, keep the version they were built from; the install
    /// (`install_edit`) re-applies those. Answers the replacement's
    /// declared nested declarations, which `Heap::adopt_nested` gives the
    /// id's row; until then the row names the previous version's.
    pub(crate) fn replace_mixin(
        &mut self,
        id: crate::heap::MixinId,
        replacement: Rc<crate::mixin::Mixin>,
    ) -> Vec<Rc<crate::mixin::Mixin>> {
        let children = self.heap.replace_mixin(id, replacement);
        self.applications.retain(|(mixin, _), _| *mixin != id);
        self.metaclass_mixins.remove(&id);
        children
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
/// callers are host-initiated sends -- the entry send of `ns-next run`, `load`
/// and `bench`, always `main:args:` with its fixed two arguments, and any
/// embedder driving `Interpreter::send` directly with an argument list of its
/// own.
fn check_method_arity<'gc>(
    method: &crate::mixin::MixinMethod,
    receiver: Value<'gc>,
    given: usize,
) -> Result<(), Signal<'gc>> {
    if given == method.argument_count {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError::error(
        method.selector,
        receiver,
        format!(
            "wrong number of arguments: `{}` takes {}, got {}",
            method.selector, method.argument_count, given
        ),
    )))
}

/// A block given a different number of arguments than it declares is refused
/// before it can reach a register: the window is laid out from the arguments
/// actually given, so too few would address registers past its end -- a panic
/// -- and too many would silently shift every temporary.
fn check_block_arity<'gc>(
    heap: &Heap<'gc>,
    parameter_count: usize,
    captured_frame: FrameReference<'gc>,
    closure: Value<'gc>,
    given: usize,
) -> Result<(), Signal<'gc>> {
    if given == parameter_count {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError::argument_error(
        Symbol::VALUE,
        heap.frame_receiver(captured_frame),
        closure,
        format!("wrong number of block arguments: block takes {parameter_count}, got {given}"),
    )))
}

/// L's error, for the activation `header` describes. The error does not name
/// its limit, so that changing it changes no recorded answer. It is reached
/// only out of line (`Mutator::push_past_the_limit`), so that the check on
/// every push is a compare and a branch and nothing more.
fn recursion_too_deep<'gc>(header: &ActivationHeader<'gc>) -> Signal<'gc> {
    Signal::Error(recursion_too_deep_error(header))
}

/// The error inside `recursion_too_deep`'s signal: what an overrun of the
/// reserve ends the run with too.
pub(crate) fn recursion_too_deep_error<'gc>(header: &ActivationHeader<'gc>) -> NewspeakError<'gc> {
    let selector = match &header.kind {
        FrameKind::Method(method) => method.selector,
        FrameKind::Closure(_) => Symbol::VALUE,
        _ => Symbol::NEW,
    };
    NewspeakError::error(
        selector,
        header.receiver,
        "stack overflow: recursion too deep",
    )
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
        classes.push(mutator.heap.allocate_class(
            class_of_class,
            crate::heap::NewClass {
                application,
                enclosing: None,
                declaring_class: None,
                superclass: Some(class_of_class),
                builtin: Some(kind),
            },
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
/// lets an object's class word stay non-optional. The construction itself lives in
/// `Heap::allocate_self_referential_class`; in a space of offsets it is
/// ordinary safe Rust, because the object is allocated before its class
/// word is written with its own offset.
fn build_self_referential_object_class<'gc>(
    heap: &mut Heap<'gc>,
    application: Rc<crate::mixin::MixinApplication>,
) -> ClassReference<'gc> {
    heap.allocate_self_referential_class(crate::heap::NewClass {
        application,
        enclosing: None,
        declaring_class: None,
        superclass: None,
        builtin: Some(BuiltinClass::Object),
    })
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

    /// A mixin is reified once per heap: every request answers the same
    /// object, another mixin -- its own class side among them -- another
    /// one, and each reads back as the mixin it stands for.
    #[test]
    fn a_mixin_is_reified_once() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let first = Rc::new(crate::mixin::Mixin::empty(Symbol::intern("First")));
            let second = Rc::new(crate::mixin::Mixin::empty(Symbol::intern("Second")));
            let class_side = mutator.metaclass_mixin(&first);
            let reified = mutator.reified_mixin(&first);
            assert!(mutator.reified_mixin(&first).same(reified));
            let other = mutator.reified_mixin(&second);
            assert!(!other.same(reified));
            let reified_class_side = mutator.reified_mixin(&class_side);
            assert!(!reified_class_side.same(reified));
            for (object, mixin) in [
                (reified, &first),
                (other, &second),
                (reified_class_side, &class_side),
            ] {
                let answered = mutator
                    .mixin_of_reified(object)
                    .expect("a reified mixin reads back as its mixin");
                assert!(Rc::ptr_eq(&answered, mixin));
            }
            assert_eq!(
                mutator.heap().describe(Value::object(reified)),
                "the mixin First"
            );
            assert_eq!(
                mutator.heap().describe(Value::object(reified_class_side)),
                "the mixin First class"
            );
        });
    }

    /// The module the replacement tests edit: `Inner` answers 1, and
    /// `fresh` answers what a new `Inner` answers.
    const OUTER: &str = indoc! {"
        class Outer = Object new () (
          public class Inner = () (
            public answer = ( ^1 )
          )
          public fresh = ( ^Inner new answer )
        )
    "};

    /// The module the replacement for `OUTER`'s `Inner` is taken from.
    const REPLACEMENT: &str = indoc! {"
        class Elsewhere = Object new () (
          public class Inner = () (
            public answer = ( ^2 )
          )
        )
    "};

    /// A replacement for `OUTER`'s `Inner`: `REPLACEMENT`'s `Inner`, whose
    /// `answer` answers 2, as a fresh mixin no id names yet. The copy holds
    /// its own text, as an edit's fresh version does, and still names
    /// `Elsewhere` as its enclosing mixin, so that a reader that asks the
    /// mixin rather than its id for the link answers something else.
    fn replacement_inner(interpreter: &mut Interpreter) -> Rc<crate::mixin::Mixin> {
        let module = ns_syntax::parse(REPLACEMENT).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(REPLACEMENT),
            SourceOrigin::File(PathBuf::from("replacement.ns")),
        )
        .expect("resolve");
        let class = interpreter.load_module(&resolved);
        let inner = interpreter.enter(|mutator| {
            let root = root_mixin(mutator, &class);
            mutator.heap().nested_mixins(&root)[0].clone()
        });
        Rc::new((*inner).clone())
    }

    /// The mixin of the module root `class` holds.
    fn root_mixin<'gc>(mutator: &Mutator<'gc>, class: &Root) -> Rc<crate::mixin::Mixin> {
        let class = mutator
            .fetch(class)
            .as_object()
            .expect("a module's root is a class object");
        mutator
            .heap()
            .class_application(class)
            .expect("a module's root is a class object")
            .mixin
            .clone()
    }

    /// Replaces `OUTER`'s `Inner`, whose class `outer` holds, with
    /// `replacement`, and answers its id.
    fn replace_inner(
        interpreter: &mut Interpreter,
        outer: &Root,
        replacement: Rc<crate::mixin::Mixin>,
    ) -> crate::heap::MixinId {
        interpreter.enter(|mutator| {
            let root = root_mixin(mutator, outer);
            let original = mutator.heap().nested_mixins(&root)[0].clone();
            let id = mutator
                .heap()
                .known_mixin_id(&original)
                .expect("applying Outer registered Inner");
            mutator.replace_mixin(id, replacement);
            id
        })
    }

    /// What `receiver` answers to the unary `selector`, as a SmallInteger.
    fn small_integer_answer(
        interpreter: &mut Interpreter,
        receiver: &Root,
        selector: &str,
    ) -> Option<i64> {
        let answer = expect_ok(
            interpreter.send(receiver, Symbol::intern(selector), &[]),
            selector,
        );
        interpreter.enter(|mutator| mutator.fetch(&answer).as_small_integer())
    }

    /// Installing a fresh mixin under an id keeps the id for both versions
    /// and makes the fresh one the id's: the address of either answers the
    /// id, the id answers the fresh one and the enclosing mixin it was
    /// registered with, and the mixin keeps its one reified object
    /// (`docs/design/liveness-and-reflection.md`, "An edit replaces the
    /// mixin under its id").
    #[test]
    fn every_version_of_a_mixin_answers_one_id() {
        let (mut interpreter, outer) = load(OUTER);
        let replacement = replacement_inner(&mut interpreter);
        interpreter.enter(|mutator| {
            let outer_mixin = root_mixin(mutator, &outer);
            let original = mutator.heap().nested_mixins(&outer_mixin)[0].clone();
            let id = mutator
                .heap()
                .known_mixin_id(&original)
                .expect("applying Outer registered Inner");
            let reified = mutator.reified_mixin(&original);
            mutator.replace_mixin(id, replacement.clone());
            let heap = mutator.heap();
            assert_eq!(heap.known_mixin_id(&original), Some(id));
            assert_eq!(heap.known_mixin_id(&replacement), Some(id));
            let current = heap.mixin(id).expect("the id has a row");
            assert!(Rc::ptr_eq(&current, &replacement));
            let outer_id = heap.known_mixin_id(&outer_mixin);
            assert!(outer_id.is_some(), "Outer was applied");
            assert_eq!(heap.enclosing_mixin_id(id), outer_id);
            assert!(mutator.reified_mixin(&replacement).same(reified));
            assert!(mutator.reified_mixin(&original).same(reified));
        });
    }

    /// A class object built after a replacement applies the replacement,
    /// though the enclosing mixin still holds the version it was built
    /// with; one built before keeps the application it was built from
    /// until the install (`install_edit`) re-applies it.
    #[test]
    fn a_nested_class_created_after_a_replacement_is_the_replacement() {
        let (mut interpreter, outer) = load(OUTER);
        let first = expect_ok(interpreter.instantiate(&outer, &[]), "the first Outer");
        assert_eq!(
            small_integer_answer(&mut interpreter, &first, "fresh"),
            Some(1)
        );
        let replacement = replacement_inner(&mut interpreter);
        replace_inner(&mut interpreter, &outer, replacement);
        assert_eq!(
            small_integer_answer(&mut interpreter, &first, "fresh"),
            Some(1),
            "the first Outer's Inner was built before the replacement"
        );
        let second = expect_ok(interpreter.instantiate(&outer, &[]), "the second Outer");
        assert_eq!(
            small_integer_answer(&mut interpreter, &second, "fresh"),
            Some(2),
            "the second Outer's Inner is built after the replacement"
        );
    }

    /// An activation entered at a level before an edit is accepted by the
    /// slot check's cold path once the level has a successor and the
    /// receiver's class names it; a chain of edits answers its newest
    /// level at once.
    #[test]
    fn a_receiver_has_its_entered_levels_successor() {
        let (mut interpreter, outer) = load(OUTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        interpreter.enter(|mutator| {
            let class = mutator
                .fetch(&outer)
                .as_object()
                .expect("a module's root is a class object");
            let receiver = mutator.fetch(&instance);
            let mixin = root_mixin(mutator, &outer);
            let id = mutator
                .heap()
                .known_mixin_id(&mixin)
                .expect("a root applied is registered");
            let first = mutator
                .heap()
                .class_application(class)
                .expect("a class object");
            let superclass = first.superclass.clone();
            // An edit that kept the level's slots: the mixin under its id
            // replaced, and the class object naming its application.
            fn next_level<'gc>(
                mutator: &mut Mutator<'gc>,
                class: ClassReference<'gc>,
                id: crate::heap::MixinId,
                mixin: &crate::mixin::Mixin,
                superclass: Option<&Rc<crate::mixin::MixinApplication>>,
            ) -> crate::heap::ApplicationIndex {
                let version = Rc::new(mixin.clone());
                mutator.replace_mixin(id, version.clone());
                let application = mutator.apply_mixin(&version, superclass);
                assert!(mutator
                    .heap_mut()
                    .class_set_application(class, application.clone()));
                application.index()
            }
            let second = next_level(mutator, class, id, &mixin, superclass.as_ref());
            assert_ne!(second, first.index());
            assert!(!mutator.heap().receiver_has_level(receiver, first.index()));
            mutator.heap_mut().record_successor(first.index(), second);
            assert!(mutator.heap().receiver_has_level(receiver, first.index()));
            assert!(mutator.heap().receiver_has_level(receiver, second));
            let third = next_level(mutator, class, id, &mixin, superclass.as_ref());
            mutator.heap_mut().record_successor(second, third);
            assert_eq!(mutator.heap().successor(first.index()), Some(third));
            assert_eq!(mutator.heap().successor(second), Some(third));
            assert_eq!(mutator.heap().successor(third), None);
            assert!(mutator.heap().receiver_has_level(receiver, first.index()));
        });
    }

    /// An application of an edited mixin's old version and one of its new
    /// version are levels of one declaration.
    #[test]
    fn an_old_and_a_new_application_of_one_id_match() {
        let (mut interpreter, outer) = load(OUTER_WITH_ACCESSORS);
        let first = expect_ok(interpreter.instantiate(&outer, &[]), "the first Outer");
        let old_inner = expect_ok(
            interpreter.send(&first, Symbol::intern("inner"), &[]),
            "the first Inner",
        );
        let replacement = replacement_inner(&mut interpreter);
        replace_inner(&mut interpreter, &outer, replacement);
        let second = expect_ok(interpreter.instantiate(&outer, &[]), "the second Outer");
        let new_inner = expect_ok(
            interpreter.send(&second, Symbol::intern("inner"), &[]),
            "the second Inner",
        );
        interpreter.enter(|mutator| {
            let old_class = mutator.fetch(&old_inner).as_object().expect("a class");
            let new_class = mutator.fetch(&new_inner).as_object().expect("a class");
            let old_application = mutator.heap().class_application(old_class).unwrap();
            let new_application = mutator.heap().class_application(new_class).unwrap();
            assert!(!Rc::ptr_eq(&old_application.mixin, &new_application.mixin));
            let found =
                mutator
                    .heap()
                    .with_level_applying_mixin_of(old_class, new_class, |application| {
                        application.index()
                    });
            assert_eq!(found, Some(old_application.index()));
        });
    }

    /// `OUTER` with accessors for its `Inner` class and one of its
    /// instances.
    const OUTER_WITH_ACCESSORS: &str = indoc! {"
        class Outer = Object new () (
          public class Inner = () (
            public answer = ( ^1 )
          )
          public inner = ( ^Inner )
          public fresh = ( ^Inner new answer )
        )
    "};

    /// `selector`'s mirror leaf run on `subject`, through a send to the VM
    /// mirror, which the host hands over and no program can reach.
    fn leaf(interpreter: &mut Interpreter, selector: &str, subject: &Root) -> Root {
        let mirror = interpreter.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            mutator.stash(mirror)
        });
        let declined = interpreter.stash(Value::small_integer(-1));
        expect_ok(
            interpreter.send(
                &mirror,
                Symbol::intern(selector),
                &[subject.clone(), declined],
            ),
            selector,
        )
    }

    /// The mirrors read a replaced mixin's enclosing mixin from its id's
    /// row, not from the replacement, whose own link a builder set for
    /// wherever it was built; its source is its own text, which its
    /// module's text holds from the replacement on.
    #[test]
    fn a_replaced_mixin_reads_its_enclosing_mixin_through_the_id_and_its_own_text() {
        let (mut interpreter, outer) = load(OUTER);
        let replacement = replacement_inner(&mut interpreter);
        let expected = crate::mixin::source_text(REPLACEMENT, replacement.span)
            .expect("the replacement's span lies in its text")
            .to_string();
        assert!(expected.contains("^2"), "{expected}");
        let id = replace_inner(&mut interpreter, &outer, replacement);
        let (inner, outer_mixin) = interpreter.enter(|mutator| {
            let inner = mutator.heap().mixin(id).expect("the id has a row");
            let inner = mutator.reified_mixin(&inner);
            let outer_mixin = root_mixin(mutator, &outer);
            let outer_mixin = mutator.reified_mixin(&outer_mixin);
            (
                mutator.stash(Value::object(inner)),
                mutator.stash(Value::object(outer_mixin)),
            )
        });
        let enclosing = leaf(&mut interpreter, "reflectionMixinEnclosing:ifFail:", &inner);
        let source = leaf(&mut interpreter, "reflectionMixinSource:ifFail:", &inner);
        interpreter.enter(|mutator| {
            let enclosing = mutator
                .fetch(&enclosing)
                .as_object()
                .expect("Inner has an enclosing mixin");
            let outer_mixin = mutator
                .fetch(&outer_mixin)
                .as_object()
                .expect("a reified mixin");
            assert!(
                enclosing.same(outer_mixin),
                "Inner's enclosing mixin is Outer's"
            );
            let source = mutator
                .fetch(&source)
                .as_object()
                .expect("the replacement has a source");
            assert_eq!(mutator.heap().str_value(source), Some(expected));
        });
    }

    /// A host reads and allocates through `enter`, which answers what its
    /// closure answers and runs nothing: the stack is as deep afterwards
    /// as before (`docs/design/memory.md`, "The brand").
    #[test]
    fn enter_answers_its_closures_value_and_runs_nothing() {
        let mut interpreter = Interpreter::bootstrap();
        let length = interpreter.enter(|mutator| {
            let array = mutator.new_array(vec![Value::small_integer(1), Value::small_integer(2)]);
            mutator.heap().array_element_count(array)
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
                Value::small_integer(3),
                Symbol::intern("+"),
                &[Value::small_integer(4)],
            ) {
                Ok(Step::Value(answer)) if answer.as_small_integer() == Some(7) => {}
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
                        matches!(arguments.as_slice(), [element] if element.as_small_integer() == Some(10)),
                        "the first element"
                    );
                    assert_eq!(state.next, 1, "the second element is next");
                    assert_eq!(state.size, 3, "the size read when the walk began");
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
                        instance
                            .as_object()
                            .is_some_and(|object| object.same(creation.enclosing)),
                        "the clause runs on the enclosing object"
                    );
                    creation.index
                }
                _ => panic!("a nested class is decided as its clause's activation"),
            };
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
            let Some(object) = instance.as_object() else {
                panic!("an instance is an object");
            };
            assert!(
                mutator
                    .heap()
                    .instance_field(object, slot)
                    .is_some_and(|value| value.is_nil()),
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
            let Some(p_class_object) = p_class.as_object() else {
                panic!("`P` answers a class object")
            };
            let Some(s_class_object) = s_class.as_object() else {
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
                    Ok(Step::Value(answer)) if answer.as_object().is_some()
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
            let Some(_) = mutator.fetch(&r_class_root).as_object() else {
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
            interpreter.enter(|mutator| mutator
                .spare_walks
                .iter()
                .all(|state| state.instance.is_nil() && state.levels.is_empty())),
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
            let Some(_) = mutator.fetch(&cell).as_object() else {
                panic!("`Cell` answers a class object")
            };
        });
        expect_ok(interpreter.instantiate(&cell, &[]), "Cell new");
        interpreter.enter(|mutator| {
            assert_eq!(mutator.spare_walks.len(), 1, "one walk ran");
            let state = &mutator.spare_walks[0];
            assert!(state.instance.is_nil(), "no instance handle");
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
    /// offset, so it measures something else. The measurement protocol runs
    /// this with `--release`. The eight-byte value left it at 56.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_decision_stays_the_size_the_hot_path_was_measured_at() {
        assert_eq!(std::mem::size_of::<Result<Step, Signal>>(), 56);
    }

    /// The debug-build twin of the pin above, so that a variant that grows a
    /// decision is noticed by the ordinary suite and not only by the release
    /// run the measurement protocol makes. It only notices growth: the
    /// release figure is the one the hot path was measured at, and a debug
    /// handle carries its origin and move epoch besides its offset, which is
    /// what puts this above it. The eight-byte value left it at 64; the
    /// 8-byte handle offset, 16 bytes with the origin and epoch, raised it
    /// to 72 while the release figure stayed at 56.
    #[cfg(all(target_pointer_width = "64", debug_assertions))]
    #[test]
    fn a_decision_stays_the_size_it_has_in_a_debug_build() {
        assert_eq!(std::mem::size_of::<Result<Step, Signal>>(), 72);
    }

    /// Every lookup hands its answer to `resolve` as a `Lookup`, which
    /// `resolve::resolved_public` matches as it is rather than through the
    /// 64-byte settled enum 3a measured as costly on this path
    /// (`docs/design/execution.md`, "What primitives as method-table entries
    /// moved"); pinned here so that growing a `Lookup` variant fails this
    /// test instead of quietly widening that path.
    /// 24 bytes in a release build: `SlotAccessor`'s own 12 bytes plus the
    /// absolute field index it carries need a 24-byte slot once alignment
    /// rounds it up, and `Method`'s `Rc` beside an 8-byte `ClassReference`
    /// fills the same 16 bytes of payload. A debug build's handle is 16
    /// bytes (the offset, then the origin and move epoch that catch a stale
    /// or cross-heap read), which puts `Method` at 32; it was 24 there too
    /// while the offset was a 4-byte index.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_lookup_stays_the_size_the_hot_path_was_measured_at() {
        let expected = if cfg!(debug_assertions) { 32 } else { 24 };
        assert_eq!(std::mem::size_of::<crate::dispatch::Lookup>(), expected);
    }

    /// The counter starts above the reserved guards: with it at
    /// `FIRST_MINTED`, the first mint answers `FIRST_MINTED` and the next the
    /// id after it.
    #[test]
    fn the_first_mint_follows_the_reserved_guards() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            mutator.dispatch_ids = crate::heap::DispatchIdAllocator::new();
            assert_eq!(
                mutator.mint_dispatch_ids().instances,
                crate::mixin::DispatchId::from_counter(crate::mixin::DispatchId::FIRST_MINTED)
            );
            assert_eq!(
                mutator.mint_dispatch_ids().instances,
                crate::mixin::DispatchId::from_counter(crate::mixin::DispatchId::FIRST_MINTED + 1)
            );
        });
    }

    /// Minting stops below the header word's 22-bit guard field and answers
    /// `NONE` from then on, so that application runs uncached.
    #[test]
    fn minting_stops_below_twenty_two_bits_and_answers_none_once_exhausted() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            // A fresh allocator: a collection during the boot, which the
            // collection stress mode makes, frees ids that would be
            // minted first.
            mutator.dispatch_ids = crate::heap::DispatchIdAllocator::new();
            mutator
                .dispatch_ids
                .set_next_for_test(crate::mixin::DispatchId::LIMIT - 1);
            assert_eq!(
                mutator.mint_dispatch_ids().instances,
                crate::mixin::DispatchId::from_counter(crate::mixin::DispatchId::LIMIT - 1)
            );
            assert_eq!(mutator.mint_dispatch_ids(), crate::mixin::DispatchIds::NONE);
            assert!(mutator.heap.collection_requested());
            assert_eq!(mutator.mint_dispatch_ids(), crate::mixin::DispatchIds::NONE);
        });
    }

    /// Every send decides a `Result<Step, Signal>` and every step answers
    /// an `EvaluationResult`, so a `Signal` that grows grows both on the
    /// hottest path there is: 8 bytes more measured about 6% on deep sends
    /// and returns (`Signal::Terminate` says why it is boxed). Pinned in a
    /// release build, where a handle carries no origin or epoch -- so it runs
    /// only there, under `cargo nextest run --release`, which the measurement
    /// protocol's builds are, and under no gate that runs a debug build.
    /// The eight-byte value shrank `Signal` and `EvaluationResult` from 48
    /// to 40 bytes; `Result<Step, Signal>` stayed at 56.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_signal_fits_in_the_niche_of_its_one_unboxed_error() {
        assert_eq!(std::mem::size_of::<Signal>(), 40);
        assert_eq!(std::mem::size_of::<Result<Step, Signal>>(), 56);
        assert_eq!(std::mem::size_of::<EvaluationResult>(), 40);
    }
}
