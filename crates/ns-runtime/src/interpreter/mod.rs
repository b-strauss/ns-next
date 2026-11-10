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
use crate::heap::{
    register_number, ActivationHeader, ActivationReference, ArgumentSlots, Heap, PushRefused, Root,
};
use crate::shipped_code::ShippedCodePath;
use crate::types::*;
use crate::Symbol;
use unwind::WalkedActivation;

// `inline_cache` is `pub(crate)`, so `FeedbackBase` on its own would be
// visible only inside this crate; re-exporting it through `interpreter`
// (a `pub mod`) is what gives `heap::NewClosure`'s `pub` field a
// fully public path to it, which a `pub` field's type must have.
pub use inline_cache::FeedbackBase;
// The inline caches' test view (`Interpreter::call_site_states`) and sizes
// (`Interpreter::cache_statistics`), public through `interpreter` for the
// same reason.
pub use inline_cache::{
    CacheStatistics, CachedTarget, CachedTargetKind, CallSiteCell, CallSiteState, MethodSide,
};

pub(crate) enum Signal<'gc> {
    /// A non-local return: its home's frame, never its activation, since the
    /// unwind that carries it runs `ensure:` blocks on its way, and an
    /// activation they push can reuse a popped activation's stack offset
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
/// holds at most 129 x (L + `DEPTH_RESERVE`) frames -- 128 being the
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
    /// nested class, an initialization with nothing to run.
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
    /// Activate the factory body `code` on the class object `class` (§6.3):
    /// allocate an instance, run `class`'s initializer on it, answer it
    /// (`compiler::factory_body`). Its value is delivered like a method's.
    Factory {
        class: ClassReference<'gc>,
        code: Rc<crate::bytecode::CompiledCode>,
        arguments: StepArguments<'gc>,
    },
    /// Activate the initializer body `body` of `level`'s mixin on
    /// `receiver`, entered at `level`, whose slot base its own-slot
    /// instructions take (`instantiate::InitializerBody`); its value goes to
    /// the instruction of the factory or initializer that decided it, which
    /// does not use it. The decision carries the body's `code`, which it
    /// found to decide that the level runs code, so that the push does not
    /// look it up again; `Step` keeps its size, which `Method` sets.
    Initializer {
        receiver: Value<'gc>,
        level: ClassReference<'gc>,
        arguments: StepArguments<'gc>,
        body: instantiate::InitializerBody,
        code: Rc<crate::bytecode::CompiledCode>,
    },
    /// Activate the superclass clause of the nested class `creation`
    /// describes, on its enclosing object and as part of the class that
    /// declares it; its value is the superclass, from which the class is
    /// built, cached and answered (`nested::after_clause`).
    SuperclassClause(Box<NestedClassCreation<'gc>>),
    /// Activate `method`, as `Step::Method` with owned arguments does, and
    /// do `then` with its value: how a future is forced for an instruction
    /// that runs again once it is resolved
    /// (`Mutator::forcing_then_reexecute`). Boxed, so that `Step` keeps the
    /// size the hot path was measured at.
    MethodThen(Box<MethodThen<'gc>>),
}

/// What a run loop's send ran once it was decided
/// (`Mutator::enter_sent`): the value a compiled callee returned, for the
/// send's destination; the activation pushed and the instruction to start
/// it at; or, once a called body's machine code exited, where the run
/// loop goes on.
pub(crate) enum Sent<'gc> {
    Value(Value<'gc>),
    Pushed(ActivationReference, usize),
    Stopped(crate::jit::Resumption),
}

/// What `Mutator::call_sent` did with a send: ran it, or left it to the
/// push, which takes the send's arguments from where it names.
enum Call<'values, 'gc> {
    Ran(Sent<'gc>),
    Push(Arguments<'values, 'gc>),
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

/// The parts of a level a `<:` clause adds that every class object has
/// (`Mutator::new_applied_level`): the application of the named class's
/// mixin to the level below, the named class's enclosing object and
/// declaring class (§6.2: "the enclosing object of I is the enclosing
/// object of C"), and the level below.
pub(crate) struct AppliedLevel<'gc> {
    pub(crate) application: Rc<crate::mixin::MixinApplication>,
    pub(crate) enclosing: Option<ObjectReference<'gc>>,
    pub(crate) declaring_class: Option<ClassReference<'gc>>,
    pub(crate) superclass: Option<ClassReference<'gc>>,
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
    /// The decision's own arguments, for `window`: `Owned`'s, none for the
    /// others.
    pub(crate) fn owned(&self) -> &[Value<'gc>] {
        match self {
            StepArguments::Owned(arguments) => arguments,
            StepArguments::Send | StepArguments::ControlFlow => &[],
        }
    }

    /// The arguments the activation is pushed with: `send`'s own for
    /// `Send`, none for `ControlFlow`, the decision's for `Owned`, which
    /// `owned` must be: `self.owned()`, held by the caller so that the
    /// window can name it by a thin reference (`Arguments::Values`). A
    /// debug build checks that it is the same slice.
    pub(crate) fn window<'values>(
        &self,
        send: Arguments<'values, 'gc>,
        owned: &'values &'values [Value<'gc>],
    ) -> Arguments<'values, 'gc> {
        match self {
            StepArguments::Send => send,
            StepArguments::ControlFlow => Arguments::NONE,
            StepArguments::Owned(arguments) => {
                debug_assert!(
                    std::ptr::eq(*owned, arguments.as_slice()),
                    "internal: the window's owned arguments are not the decision's own"
                );
                Arguments::Values(owned)
            }
        }
    }
}

/// The arguments a decision reads: a send's window in its sender's
/// registers, read where they are, or values a decision or the host built.
///
/// A send's arguments stay in the sender's registers while the send is
/// decided and its activation pushed, and the push lays a send's own callee
/// on its argument slots, which then are the callee's arguments
/// (`Mutator::push_on_slots`), or copies them into the window of any other
/// activation (`Heap::push_activation`), so nothing gathers them anywhere
/// in between (`docs/design/execution.md`, "Planned: overlapping register
/// windows"). Reading one is reading a register of a
/// live activation: good while the sender stays where it is, which it does
/// from the send's start to its push -- no collection runs inside a
/// decision, only at safepoints, and nothing pops the sender meanwhile.
///
/// Each case holds one word, so that the enum is a tag and a word and
/// travels in two registers: as a three-field window beside a fat slice it
/// was an aggregate of sixteen bytes, passed by reference to a copy built
/// by narrow stores and read back wide, a stall on every send.
/// Public because `Heap::push_activation` is, for a host's own frames.
#[derive(Clone, Copy)]
pub enum Arguments<'values, 'gc> {
    /// Consecutive registers of a window, which must lie in a live
    /// activation's registers, as `Heap::register` requires of every
    /// register it reads: a send's sources where they are consecutive, its
    /// slots otherwise (`vm::argument_window`).
    Slots(ArgumentSlots),
    /// Values built apart from any window: `Message>>sendTo:`'s, a host's,
    /// a decision's own -- named through a reference to the slice, which
    /// keeps this case one word.
    Values(&'values &'values [Value<'gc>]),
}

/// Why a push refused before it pushed anything, for its cold path
/// (`Mutator::push_refused`), with the arguments that path pushes from.
enum Refusal<'values, 'gc> {
    /// A list of values longer than the register ceiling, `u16::MAX`.
    PastTheRegisterCeiling,
    /// A counted push at the depth limit.
    AtTheLimit(Arguments<'values, 'gc>),
    /// A frame the ordinary room of the stack's reservation cannot hold.
    StackFull(Arguments<'values, 'gc>),
}

impl<'values, 'gc> Arguments<'values, 'gc> {
    /// No arguments.
    pub const NONE: Arguments<'values, 'gc> = Arguments::Values(&(&[] as &[Value<'gc>]));

    /// How many arguments there are.
    #[inline(always)]
    pub(crate) fn len(self) -> usize {
        match self {
            Arguments::Slots(slots) => usize::from(slots.count()),
            Arguments::Values(values) => values.len(),
        }
    }

    /// Argument `index`; panics past the last, as indexing a slice does,
    /// so that a register is never read beyond the send's own.
    #[inline(always)]
    pub(crate) fn get(self, heap: &Heap<'gc>, index: usize) -> Value<'gc> {
        assert!(
            index < self.len(),
            "internal: argument {index} read from a send of {} arguments",
            self.len()
        );
        self.read(heap, index)
    }

    /// Argument `index`, or `None` past the last: what a leaf that checks
    /// its own operands reads.
    #[inline(always)]
    pub(crate) fn try_get(self, heap: &Heap<'gc>, index: usize) -> Option<Value<'gc>> {
        (index < self.len()).then(|| self.read(heap, index))
    }

    /// Argument `index`, which the caller has checked lies below `len`.
    #[inline(always)]
    fn read(self, heap: &Heap<'gc>, index: usize) -> Value<'gc> {
        match self {
            // `index < count`, a `u16`, so the sum is a register of the
            // window the send named.
            Arguments::Slots(slots) => heap.register(
                slots.register_base(),
                slots.start() + register_number(index),
            ),
            Arguments::Values(values) => values[index],
        }
    }

    /// The first argument, or `None` when there is none.
    #[inline(always)]
    pub(crate) fn first(self, heap: &Heap<'gc>) -> Option<Value<'gc>> {
        self.try_get(heap, 0)
    }

    /// The arguments in `range`, which must lie inside `0..len`, read where
    /// they are as these are. `view` is where the narrower slice of
    /// `Values` is kept for the answer to name.
    pub(crate) fn slice<'narrow>(
        self,
        range: std::ops::Range<usize>,
        view: &'narrow mut &'values [Value<'gc>],
    ) -> Arguments<'narrow, 'gc>
    where
        'values: 'narrow,
    {
        assert!(
            range.start <= range.end && range.end <= self.len(),
            "internal: the range lies outside the send's arguments"
        );
        match self {
            // Both below `count`, a `u16`.
            Arguments::Slots(slots) => Arguments::Slots(ArgumentSlots::new(
                slots.register_base(),
                slots.start() + register_number(range.start),
                register_number(range.end - range.start),
            )),
            Arguments::Values(values) => {
                *view = &values[range];
                Arguments::Values(view)
            }
        }
    }

    /// Every argument, in order.
    pub(crate) fn iter<'heap>(
        self,
        heap: &'heap Heap<'gc>,
    ) -> impl Iterator<Item = Value<'gc>> + use<'heap, 'values, 'gc> {
        (0..self.len()).map(move |index| self.read(heap, index))
    }

    /// Every argument, in order, copied out: for a decision that keeps
    /// them past its send -- `doesNotUnderstand:`'s message, a mirror
    /// leaf's owned step.
    pub(crate) fn to_vec(self, heap: &Heap<'gc>) -> Vec<Value<'gc>> {
        self.iter(heap).collect()
    }
}

/// Everything a step of the interpreter works on -- the heap, the
/// bootstrap's builtin classes and the caches of behaviour -- and every
/// decision, native and continuation that runs one. A host reaches it only through
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
    /// instance would get its own behaviour. Weak, as the heap's tables are:
    /// a full collection drops every entry whose application or superclass
    /// key it frees (`Mutator::reclaim_ids`), so a key never names an index
    /// minted again for another application.
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
    /// The lowering every module this interpreter loads is compiled with
    /// (`docs/design/compiler.md`, "The reference lowering"). Fixed at
    /// construction, so one interpreter never mixes the two.
    pub(crate) lowering: Lowering,
    /// Where the boot takes the shipped modules' compiled mixins from. Fixed
    /// at construction, as `lowering` is.
    pub(crate) shipped_code_path: ShippedCodePath,
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
    /// The JIT's state (`jit::JitState`): per interpreter, since its
    /// machine code lives in a code zone of its own; on the mutator,
    /// since the run loop, which counts a body's entries and enters its
    /// machine code, reaches nothing else.
    pub(crate) jit: crate::jit::JitState,
    /// How many more unhandled exceptions `defaultAction` resumes with
    /// `nil` instead of ending the run: the test hook
    /// `Interpreter::resume_unhandled_exceptions`, standing in for the
    /// debugger. Zero outside a test that asks.
    pub(crate) unhandled_exceptions_to_resume: usize,
    /// The live activations when the last unhandled exception ended a run,
    /// innermost first (`Interpreter::stack_trace`).
    pub(crate) last_unhandled_stack: Vec<String>,
    /// The depth limit's reserve (`DEPTH_RESERVE`), while it is held: the
    /// frame of the activation a VM error arose in, whose `signal` -- or,
    /// for a message nothing understood, whose `Object>>doesNotUnderstand:`
    /// -- sits directly above it (`Mutator::depth_reserve_held`). A frame
    /// and not a stack offset, because a frame is divorced when its
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
    /// The suspend stress mode's schedule, when it is on
    /// (`NS_NEXT_STRESS_SUSPEND`, or `set_stress_suspend`): every
    /// activation of the run suspended into its frame and the top one
    /// resumed at the safepoints its schedule picks, so that every program
    /// runs on from frames as a deoptimized one will. Paced like the
    /// collect stress mode, since suspending walks the whole run's stack.
    /// Debug builds only.
    #[cfg(debug_assertions)]
    stress_suspend: Option<safepoint::StressSchedule>,
    /// The zone stress mode's schedule, when it is on
    /// (`NS_NEXT_STRESS_ZONE`, or `set_stress_zone`): the code zone
    /// compacted at the safepoints its schedule picks, every body no
    /// activation runs freed, so that every program runs with its bodies
    /// freed, compiled again and moved (`jit::zone`). Paced like the
    /// collect stress mode, since a compaction walks the stack and every
    /// cell. Debug builds only.
    #[cfg(debug_assertions)]
    stress_zone: Option<safepoint::StressSchedule>,
    /// How many times the cache stress mode has invalidated the caches since
    /// it was turned on, for a test to pin that it actually ran
    /// (`cache_invalidations`). Debug builds only.
    #[cfg(debug_assertions)]
    cache_invalidations: usize,
    /// The attached debugger, which the run loop's safepoint serves
    /// (`attach_debugger`, `serve_debugger`); `None` until a host attaches
    /// one, and again once its remote is gone.
    pub(crate) debugger: Option<crate::debugger::Debugger>,
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
        #[cfg_attr(
            not(debug_assertions),
            allow(
                unused_mut,
                reason = "only the debug build's stress modes write to these fields"
            )
        )]
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
            stress_suspend: safepoint::StressSchedule::from_env("NS_NEXT_STRESS_SUSPEND"),
            #[cfg(debug_assertions)]
            stress_zone: safepoint::StressSchedule::from_env("NS_NEXT_STRESS_ZONE"),
            #[cfg(debug_assertions)]
            cache_invalidations: 0,
            debugger: None,
        };
        #[cfg(debug_assertions)]
        {
            interpreter.hold_safepoints_for_stress();
            // The collect stress mode slides the site ranges at every
            // collection that finds one free, so that its collections
            // exercise every slide (`InlineCaches::plan_slide`).
            let eager_slides = interpreter.stress_collect.is_some();
            interpreter.world.set_eager_slides(eager_slides);
            interpreter.shrink_pairs_for_stress();
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
            let decided =
                mutator.decide_send(receiver, selector, Arguments::Values(&arguments.as_slice()));
            mutator.start(decided, Arguments::Values(&arguments.as_slice()))
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
            let decided = nested::nested_class(
                mutator,
                receiver,
                nested,
                declaring_class,
                nested::NO_CACHED_FIELD,
            );
            mutator.start(decided, Arguments::NONE)
        });
        self.run_from_host(start, depth)
    }

    /// Runs what a host's send or instantiation started. It is the only way
    /// Rust enters the run loop (`docs/design/execution.md`, "No re-entry
    /// from Rust"). `depth` is the stack's depth before the host's entry
    /// decided anything.
    ///
    /// Five things are checked in a debug build, because each is otherwise
    /// silent. The loop is never entered while it is running -- nothing it
    /// calls can reach here. The stack is left as deep as it was found: a
    /// leaked frame would put a later activation on top of a dead one's
    /// registers (a leak inside a run is caught too, by
    /// `Heap::pop_activation`'s check that it pops the top). No suspended
    /// activation outlives the run that suspended it, and no continuation
    /// the run's activations held stays in the heap's table, where it
    /// would keep what it names alive. And no nested class's creation
    /// outlives the run that began it.
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
                self.enter(|mutator| mutator.set_depth_reserve(None));
                result
            }
        };
        debug_assert_eq!(
            self.world.activation_depth(),
            depth,
            "a host's send or instantiation must leave the activation stack as it found it"
        );
        // Every path out of a run pops through the suspended senders,
        // resuming each in turn, so none outlives the run that suspended it.
        debug_assert_eq!(
            self.world.suspension_count(),
            0,
            "internal: a suspended activation outlived its run"
        );
        // No activation outlives its run, so neither does a continuation
        // its stack frame named.
        debug_assert_eq!(
            self.world.continuation_count(),
            0,
            "internal: a continuation outlived its run"
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
            method_code(mutator, class, side, method)
                .map_or_else(Vec::new, |code| mutator.caches.states_of(&code, sent))
        })
    }

    /// The application the dispatch id `id` was minted for, while it is
    /// registered (`Heap::application_of_dispatch_id`). A test hook.
    #[doc(hidden)]
    pub fn application_of_dispatch_id(
        &mut self,
        id: crate::mixin::DispatchId,
    ) -> Option<crate::heap::ApplicationIndex> {
        self.enter(|mutator| mutator.heap.application_of_dispatch_id(id))
    }

    /// The feedback base the closure `closure` holds, raw, or `None` when
    /// it is no closure: what a slide of the site ranges rewrites
    /// (`InlineCaches::plan_slide`). A test hook.
    #[doc(hidden)]
    pub fn closure_feedback_base(&mut self, closure: &Root) -> Option<u32> {
        self.enter(|mutator| {
            let object = mutator.fetch(closure).as_object()?;
            let (_, _, base) = mutator.heap.closure_activation(object)?;
            Some(base.raw())
        })
    }

    /// The cells of `method`'s sends of `sent` (`call_site_states`'s
    /// sites): the words machine code reads beside each entry. A test hook.
    #[doc(hidden)]
    pub fn call_site_cells(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
        sent: Symbol,
    ) -> Vec<CallSiteCell> {
        self.enter(|mutator| {
            method_code(mutator, class, side, method)
                .map_or_else(Vec::new, |code| mutator.caches.cells_of(&code, sent))
        })
    }

    /// Whether every inline-cache entry's machine-readable cell agrees with
    /// the entry (`docs/design/execution.md`, "Inline caches"); the first
    /// disagreement, described, otherwise. A test hook.
    #[doc(hidden)]
    pub fn verify_cells(&mut self) -> Result<(), String> {
        self.enter(|mutator| mutator.caches.verify_cells(&mutator.heap, &mutator.jit))
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

    /// What the JIT has done: bodies compiled and refused, and the exits
    /// it counts. Available in every build, like `heap_statistics`.
    pub fn jit_statistics(&self) -> crate::jit::JitStatistics {
        self.world.jit_statistics()
    }

    /// Why the JIT is off though it was wanted, `None` while it runs or
    /// when nothing asked for it (`NS_NEXT_NO_JIT=1`, or a build without a
    /// JIT): its code zone could not be reserved -- on a Windows before
    /// 10 version 1803 among others, which the reason names. A host reports
    /// it, since the program then runs interpreted, several times slower,
    /// and a measurement must refuse it rather than time the interpreter.
    pub fn jit_unavailable(&self) -> Option<String> {
        self.world.jit_unavailable()
    }

    /// Turns the JIT on or off for the bodies entered from now on, as
    /// `NS_NEXT_NO_JIT=1` turns it off for a new interpreter. A test hook,
    /// set before the code it is meant for runs: a body entered while the
    /// JIT is off is refused and stays interpreted for good, so turning
    /// the JIT on later does not revive it. A switch vacates the caches
    /// and clears every machine entry row, so that no cell and no closure
    /// stub enters a body refused meanwhile.
    #[doc(hidden)]
    pub fn set_jit_enabled(&mut self, enabled: bool) {
        self.enter(|mutator| {
            let was_enabled = mutator.jit.is_enabled();
            let reserved = mutator.jit.set_enabled(enabled);
            // What a cell or a machine entry row names is decided by the
            // JIT's state, which a switch changes: a body pushed while the
            // JIT is off is refused for good, and a cell or row written
            // before would still enter its machine code once the JIT is
            // on again. Rows are written afresh at a block's next push,
            // cells at their sites' next miss.
            if reserved || was_enabled != mutator.jit.is_enabled() {
                mutator.caches.invalidate();
                mutator.heap.forget_machine_entries();
            }
        });
    }

    /// Gives the JIT a fresh code zone that holds at most `bytes` of code,
    /// stubs included, so that a test can fill it. A test hook. The
    /// caches are vacated with the old zone, since their cells name
    /// addresses in it.
    #[doc(hidden)]
    pub fn set_jit_zone_bytes(&mut self, bytes: usize) {
        self.enter(|mutator| {
            mutator.jit.replace_zone(bytes);
            mutator.caches.invalidate();
            // Every machine entry the table records lies in the old zone.
            mutator.heap.forget_machine_entries();
        });
    }

    /// Compacts the code zone now, between runs, as the run loop's
    /// safepoint compacts it once a compile found the zone full: every body
    /// dead, abandoned or no longer named freed, and then the least used
    /// until a quarter of the zone is free (`jit::zone`). A test hook.
    #[doc(hidden)]
    pub fn compact_jit_zone(&mut self) {
        assert!(!self.running, "compact_jit_zone is for between runs");
        self.enter(|mutator| mutator.compact_code_zone());
    }

    /// The zone offset at which `method`'s body has machine code in this
    /// interpreter's zone, if it has some. A test hook.
    #[doc(hidden)]
    pub fn jit_body_offset(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> Option<u32> {
        self.enter(|mutator| {
            let code = method_code(mutator, class, side, method)?;
            mutator.jit.body_offset(&code)
        })
    }

    /// The zone offset at which `method`'s body has optimized code in this
    /// interpreter's zone, if it has some. A test hook.
    #[doc(hidden)]
    pub fn jit_optimized_offset(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> Option<u32> {
        self.enter(|mutator| {
            let code = method_code(mutator, class, side, method)?;
            mutator.jit.optimized_offset(&code)
        })
    }

    /// How many customized copies of `method`'s body -- one per receiver
    /// application and entered application it was optimized for -- have
    /// optimized code in this interpreter's zone. A test hook.
    #[doc(hidden)]
    pub fn jit_customized_copies(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> usize {
        self.enter(|mutator| {
            method_code(mutator, class, side, method)
                .map_or(0, |code| mutator.jit.customized_copies(&code))
        })
    }

    /// How many bodies the optimized copies of `method`'s body that this
    /// interpreter's zone holds inlined, counted once per copy that
    /// inlines each (`jit::CopyDependencies`). A test hook.
    #[doc(hidden)]
    pub fn jit_inlined_bodies(&mut self, class: &Root, side: MethodSide, method: Symbol) -> usize {
        self.enter(|mutator| {
            method_code(mutator, class, side, method)
                .map_or(0, |code| mutator.jit.inlined_bodies(&code))
        })
    }

    /// `method`'s body's code id now, if it has one -- a full collection
    /// frees it once nothing names it -- as its row number. A test hook.
    #[doc(hidden)]
    pub fn jit_code_id(&mut self, class: &Root, side: MethodSide, method: Symbol) -> Option<usize> {
        #[cfg(ns_jit)]
        {
            self.enter(|mutator| {
                let code = method_code(mutator, class, side, method)?;
                mutator.heap.minted_code_id(&code).map(|id| id.index())
            })
        }
        #[cfg(not(ns_jit))]
        {
            let _ = (class, side, method);
            None
        }
    }

    /// Whether a live copy of optimized code names `method`'s body's code
    /// id as one of the bodies it inlined, which a full collection keeps
    /// (`jit::inlined_code_ids`). A test hook.
    #[doc(hidden)]
    pub fn jit_code_id_named_by_a_copy(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> bool {
        #[cfg(ns_jit)]
        {
            self.enter(|mutator| {
                method_code(mutator, class, side, method)
                    .and_then(|code| mutator.heap.minted_code_id(&code))
                    .is_some_and(|id| crate::jit::inlined_code_ids(mutator).contains(&id))
            })
        }
        #[cfg(not(ns_jit))]
        {
            let _ = (class, side, method);
            false
        }
    }

    /// The machine entries the dispatching rows of `method`'s body name, in
    /// the rows' order, and those of the customized copies of it this
    /// interpreter's zone holds, sorted: what a `super` or private lexical
    /// send of it enters through its dispatching entry. A test hook.
    #[doc(hidden)]
    pub fn jit_dispatching_entries(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> (Vec<u64>, Vec<u64>) {
        #[cfg(ns_jit)]
        {
            self.enter(|mutator| {
                method_code(mutator, class, side, method)
                    .map(|code| mutator.jit.dispatching_entries(&code))
                    .unwrap_or_default()
            })
        }
        #[cfg(not(ns_jit))]
        {
            let _ = (class, side, method);
            (Vec::new(), Vec::new())
        }
    }

    /// For each block `method` makes, in order: whether its body has
    /// optimized code in this interpreter's zone, and how many customized
    /// copies of it there are, which a block never has. A test hook.
    #[doc(hidden)]
    pub fn jit_block_tiers(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> Vec<(bool, usize)> {
        self.enter(|mutator| {
            let Some(code) = method_code(mutator, class, side, method) else {
                return Vec::new();
            };
            code.instructions()
                .iter()
                .filter_map(|instruction| match instruction {
                    crate::bytecode::Instruction::MakeClosure { code: block, .. } => Some(block),
                    _ => None,
                })
                .map(|block| {
                    (
                        mutator.jit.optimized_offset(block).is_some(),
                        mutator.jit.customized_copies(block),
                    )
                })
                .collect()
        })
    }

    /// The data words of `method`'s body's machine code -- its transfer
    /// words, which name its twin's code -- as the zone holds them. A test
    /// hook.
    #[doc(hidden)]
    pub fn jit_body_words(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> Option<Vec<u64>> {
        self.enter(|mutator| {
            let code = method_code(mutator, class, side, method)?;
            mutator.jit.body_words(&code)
        })
    }

    /// Whether the machine code of `method`'s body in this interpreter's
    /// zone reifies its frame, if it has some: it never does, since every
    /// body compiles lean and only its twin reifies. A test hook.
    #[doc(hidden)]
    pub fn jit_body_reifies(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> Option<bool> {
        self.enter(|mutator| {
            let code = method_code(mutator, class, side, method)?;
            mutator.jit.body_reifies(&code)
        })
    }

    /// The zone offset of the twin of `method`'s body -- its reifying
    /// compile, made when a transfer first needed it -- if it has one. A
    /// test hook.
    #[doc(hidden)]
    pub fn jit_twin_offset(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> Option<u32> {
        self.enter(|mutator| {
            let code = method_code(mutator, class, side, method)?;
            mutator.jit.twin_offset(&code)
        })
    }

    /// The data words of the twin of `method`'s body -- its closure sites
    /// -- as the zone holds them, if it has a twin. A test hook.
    #[doc(hidden)]
    pub fn jit_twin_words(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> Option<Vec<u64>> {
        self.enter(|mutator| {
            let code = method_code(mutator, class, side, method)?;
            mutator.jit.twin_words(&code)
        })
    }

    /// For each block `method` makes, in order: the machine entry its row
    /// of the machine entry table names (0 for none), and the address of
    /// its machine code in this interpreter's zone, if it has some. A test
    /// hook.
    #[doc(hidden)]
    pub fn jit_block_rows(
        &mut self,
        class: &Root,
        side: MethodSide,
        method: Symbol,
    ) -> Vec<(u64, Option<u64>)> {
        self.enter(|mutator| {
            let Some(code) = method_code(mutator, class, side, method) else {
                return Vec::new();
            };
            let mut rows = Vec::new();
            for instruction in code.instructions() {
                let crate::bytecode::Instruction::MakeClosure { code: block, .. } = instruction
                else {
                    continue;
                };
                #[cfg(ns_jit)]
                let entry = {
                    let id = mutator.heap.code_id(&block);
                    mutator.heap.machine_entry_of(id)
                };
                #[cfg(not(ns_jit))]
                let entry = 0;
                rows.push((entry, mutator.jit.body_entry_address(&block)));
            }
            rows
        })
    }

    /// Turns the JIT stress mode (`NS_NEXT_STRESS_JIT`) on or off for this
    /// interpreter: every body compiled at its first entry, installed and
    /// entered at every send, push and frame returned to -- one that exits
    /// at once among them -- and never abandoned. A test hook, set before the code it is meant
    /// for runs.
    #[doc(hidden)]
    pub fn set_jit_stress(&mut self, stress: bool) {
        self.enter(|mutator| mutator.jit.set_stress(stress));
    }

    /// Turns the optimize stress mode (`NS_NEXT_STRESS_OPTIMIZE`) on or
    /// off for this interpreter: every body compiled at its first entry or
    /// interpreted backward jump, and every budget armed at 0 -- the ones
    /// armed already, whatever `set_optimization_budget` set, among them
    /// -- so that a body's first spend requests its optimization; off,
    /// the budget set before, and the deopt stress mode off with it. A
    /// test hook, set before the code it is meant for runs.
    #[doc(hidden)]
    pub fn set_optimize_stress(&mut self, stress: bool) {
        self.enter(|mutator| {
            let budget = mutator.jit.set_optimize_stress(stress);
            #[cfg(ns_jit)]
            crate::jit::rearm_all_budgets(mutator, budget);
            #[cfg(not(ns_jit))]
            let _ = budget;
        });
    }

    /// Turns the deopt stress mode (`NS_NEXT_STRESS_DEOPT`) on or off for
    /// this interpreter: on, with the optimize stress mode, the optimized
    /// code compiled from now on leaving at every n-th interrupt point it
    /// passes, n set afresh at every safepoint, rotating over the
    /// Fibonacci numbers from 1 to 987; off, the optimize stress mode left
    /// on. A test hook, set
    /// before the code it is meant for runs.
    #[doc(hidden)]
    pub fn set_deopt_stress(&mut self, stress: bool) {
        self.enter(|mutator| {
            let budget = mutator.jit.set_deopt_stress(stress);
            #[cfg(ns_jit)]
            crate::jit::rearm_all_budgets(mutator, budget);
            #[cfg(not(ns_jit))]
            let _ = budget;
        });
    }

    /// Turns the invalidate stress mode (`NS_NEXT_STRESS_INVALIDATE`) on
    /// or off for this interpreter: on, with the optimize stress mode, the
    /// optimized code compiled from now on leaves for a safepoint at every
    /// tenth interrupt point it passes, which frees the copies of every
    /// body but those backing off before it installs what was requested
    /// since; off, the optimize stress mode left on. A test hook, set
    /// before the code it is meant for runs.
    #[doc(hidden)]
    pub fn set_invalidate_stress(&mut self, stress: bool) {
        self.enter(|mutator| {
            let budget = mutator.jit.set_invalidate_stress(stress);
            #[cfg(ns_jit)]
            crate::jit::rearm_all_budgets(mutator, budget);
            #[cfg(not(ns_jit))]
            let _ = budget;
        });
    }

    /// Makes the JIT lower `MakeFuture` to an exit in the bodies it
    /// compiles from now on, so that a body that makes a future at once
    /// exits at once and is never compiled, while it still returns as
    /// any body does. A test hook, set before the code it is meant for is
    /// compiled.
    #[doc(hidden)]
    pub fn set_jit_futures_exit(&mut self, futures_exit: bool) {
        self.enter(|mutator| mutator.jit.set_futures_exit(futures_exit));
    }

    /// Makes machine code compiled from now on count its backward jumps,
    /// its compiled calls and its calls through a dispatching entry
    /// (`JitStatistics::machine_back_edges`, `compiled_calls`,
    /// `dispatched_calls`), which it otherwise leaves uncounted so that a
    /// loop or a call pays nothing for the count. A test hook, and
    /// `--jit-statistics`'s.
    #[doc(hidden)]
    pub fn set_jit_counting(&mut self, counting: bool) {
        self.enter(|mutator| mutator.jit.set_counting(counting));
    }

    /// Sets the budget of instructions a body runs in baseline machine
    /// code before its optimization is requested
    /// (`jit::OPTIMIZATION_BUDGET` otherwise), for every body armed from
    /// now on and every one armed already, so that a test that pins what
    /// a long loop does in baseline code parks it (`i64::MAX`), and one of
    /// the budget's reaches its end in a few turns. A test hook.
    #[doc(hidden)]
    pub fn set_optimization_budget(&mut self, budget: i64) {
        self.enter(|mutator| {
            let armed = mutator.jit.set_optimization_budget(budget);
            #[cfg(ns_jit)]
            crate::jit::rearm_all_budgets(mutator, armed);
            #[cfg(not(ns_jit))]
            let _ = armed;
        });
    }

    /// Sets the interpreted backward jump at which a body without machine
    /// code is compiled and its frame switched into the code
    /// (`jit::COMPILE_AT_BACK_JUMP`, 40, otherwise), so that a test reaches
    /// the switch in a loop of a few turns. A test hook, set before the
    /// loop it is meant for runs.
    #[doc(hidden)]
    pub fn set_jit_compile_at_back_jump(&mut self, jumps: u32) {
        self.enter(|mutator| mutator.jit.set_compile_at_back_jump(jumps));
    }

    /// Sets the interpreted entries after which a body abandoned for the
    /// first time is revived (`jit::REVIVAL_ENTRIES`, 4,096, otherwise), so
    /// that a test reaches a revival by entries cheaply. A test hook, set
    /// before the abandonment it is meant for.
    #[doc(hidden)]
    pub fn set_jit_revival_entries(&mut self, entries: u32) {
        self.enter(|mutator| mutator.jit.set_revival_entries(entries));
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

    /// The sizes of the tables beside the object space and of what the
    /// reservations beside it have committed: what a long-run test holds
    /// to a bound. Available in every build, like `collector_statistics`.
    #[doc(hidden)]
    pub fn table_sizes(&self) -> crate::TableSizes {
        self.world.table_sizes()
    }

    /// Sets every table's growth floor and next limit (`heap::limits`), so
    /// that a test reaches a table's limit without minting up to the
    /// default floor first. A test hook.
    #[doc(hidden)]
    pub fn set_table_floors(&mut self, floors: crate::TableFloors) {
        self.world.set_table_floors(floors);
    }

    /// Sets the threshold and the headroom by which each reservation beside
    /// the object space gives committed memory back after a full collection
    /// (`heap::limits::Shrink`), so that a test crosses a threshold without
    /// committing up to the default first. A test hook.
    #[doc(hidden)]
    pub fn set_shrink_pairs(&mut self, pairs: crate::ShrinkPairs) {
        self.world.set_shrink_pairs(pairs);
    }

    /// Replaces the global cache with a fresh one sized for `entries`,
    /// vacating whatever it held. A test hook (`NS_NEXT_GLOBAL_CACHE_ENTRIES`,
    /// `run_goldens.rs`), not an input path a program can reach: `entries`
    /// must be twice a power of two -- two per group -- and a host that gets
    /// this wrong panics rather than the cache rounding it silently.
    pub fn set_global_cache_size(&mut self, entries: usize) {
        self.enter(|mutator| {
            mutator.caches.set_global_cache_size(entries);
            // The entry block holds the cache's mask.
            mutator.jit.mark_block_stale();
        });
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

    /// The live activations when the last unhandled exception ended a
    /// run, innermost first, each a line naming what it runs -- a method's
    /// selector, or its kind -- and its stored program counter: what a
    /// debugger would show of the stack. A test hook, as
    /// `resume_unhandled_exceptions` is; empty before any such exception.
    #[doc(hidden)]
    pub fn stack_trace(&mut self) -> Vec<String> {
        self.enter(|mutator| mutator.last_unhandled_stack.clone())
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
    /// (`Heap::set_immediate_dispatch_ids`), and the guards a closure's and
    /// a frame's headers carry beside them (`Heap::set_builtin_guards`), so
    /// that their allocation looks none up -- once when the placeholders
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
        self.rewrite_machine_classes();
        let guards = crate::heap::BuiltinGuards {
            closure: instances(self, BuiltinClass::Closure),
            frame: instances(self, BuiltinClass::Activation),
        };
        self.heap.set_builtin_guards(guards);
    }

    /// Writes the immediates' class objects, and the `Array`, `Float`,
    /// `Closure`, `Activation` and `FutureImplementation` classes an allocation in
    /// machine code writes, into their machine words, from
    /// the builtin classes: at the bootstrap and again after every mover
    /// (`World::scavenge`, `collect`, `move_everything`, `forward_identity`),
    /// since a word a heap object stores is a handle the collector does not
    /// trace.
    pub(crate) fn rewrite_machine_classes(&mut self) {
        let classes = [
            self.builtin_class(BuiltinClass::UndefinedObject),
            self.builtin_class(BuiltinClass::False),
            self.builtin_class(BuiltinClass::True),
            self.builtin_class(BuiltinClass::Integer),
            self.builtin_class(BuiltinClass::Float),
        ];
        self.heap.set_immediate_classes(classes);
        self.heap
            .set_array_class(self.builtin_class(BuiltinClass::Array));
        self.heap
            .set_float_class(self.builtin_class(BuiltinClass::Float));
        self.heap
            .set_reification_classes(self.builtin_class(BuiltinClass::Closure));
        self.heap.set_future_class(
            self.builtin_class(BuiltinClass::FutureImplementation),
            self.futures_made,
        );
    }

    /// Records that this heap has made a future (`futures_made`), here and
    /// in the `FutureHeader` word, which lets machine code's `MakeFuture`
    /// make the next ones (`Heap::set_future_class`).
    pub(crate) fn note_futures_made(&mut self) {
        if !self.futures_made {
            self.futures_made = true;
            self.heap
                .set_future_class(self.builtin_class(BuiltinClass::FutureImplementation), true);
        }
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
        send_arguments: Arguments<'_, 'gc>,
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
    /// decides a factory's activation (`decide_factory`) rather than
    /// entering the host entry `instantiate`, which would enter the run loop
    /// from inside it -- the re-entry `run_from_host` refuses. See
    /// `instantiate::decide_factory`'s own doc comment for why this can never
    /// double-fire against the superclass clause's own factory message (an
    /// initializer never decides a send to its superclass).
    pub(crate) fn decide_send(
        &mut self,
        receiver: Value<'gc>,
        selector: Symbol,
        arguments: Arguments<'_, 'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let resolved = self.resolve_public(receiver, selector);
        self.act(resolved, receiver, selector, arguments)
    }

    /// Pushes an activation, refusing one that would take the counted depth
    /// past `MAX_ACTIVATION_DEPTH`. Every activation in the runtime is pushed
    /// here or by `push_placed`, which this calls, so the count and the
    /// check cannot be skipped by a new site.
    ///
    /// The check is the one compare it always was: the reserve is looked at
    /// only once the depth has reached L (`push_past_the_limit`), so an
    /// error far from L, and every push below it, pays nothing for it.
    #[inline(always)]
    pub(crate) fn push(
        &mut self,
        header: ActivationHeader<'gc>,
        arguments: Arguments<'_, 'gc>,
        temporaries: usize,
    ) -> Result<ActivationReference, Signal<'gc>> {
        self.push_placed(header, arguments, None, temporaries)
    }

    /// `push`, or for a send's own callee `push_on_slots`: `slots` is the
    /// first of the send's argument slots in its sender's window when the
    /// activation takes the run loop's send's own arguments, `arguments`
    /// then its window, and `None` for every other push, which lands above
    /// the sender's whole window.
    ///
    /// The header is lent to the push, never moved, and handed on whole
    /// only to a refusal's cold path (`push_refused`): moved through the
    /// push's inlined layers, it was copied into each one's own place
    /// sixteen bytes at a time, right after the narrow stores that built
    /// it -- a store-forwarding stall on every send.
    #[inline(always)]
    fn push_placed(
        &mut self,
        mut header: ActivationHeader<'gc>,
        arguments: Arguments<'_, 'gc>,
        slots: Option<u16>,
        temporaries: usize,
    ) -> Result<ActivationReference, Signal<'gc>> {
        let pushed = match (arguments, slots) {
            (Arguments::Slots(window), Some(argument_base)) => {
                self.push_on_slots(&mut header, window, argument_base, temporaries)
            }
            _ => {
                debug_assert!(
                    slots.is_none(),
                    "internal: a send's own callee is handed the send's window"
                );
                self.push_above(&mut header, arguments, temporaries)
            }
        };
        match pushed {
            Ok(activation) => {
                // What the frame did not take is released here, inline: the
                // method and the code. A successful push always took the
                // continuation and left `Deliver` in its place, which owns
                // nothing, so forgetting it is dropping it without a call
                // into `Continuation`'s drop glue.
                let ActivationHeader {
                    method, code, then, ..
                } = header;
                drop(method);
                drop(code);
                debug_assert!(
                    matches!(then, Continuation::Deliver),
                    "internal: a successful push takes the continuation"
                );
                std::mem::forget(then);
                Ok(activation)
            }
            Err(refusal) => self.push_refused(refusal, header, temporaries),
        }
    }

    /// Pushes an activation above every live frame's whole window
    /// (`Heap::push_activation_from`), refusing, pushing nothing, a list of
    /// values past the register ceiling, a push at the depth limit and one
    /// the reservation cannot hold.
    #[inline(always)]
    fn push_above<'values>(
        &mut self,
        header: &mut ActivationHeader<'gc>,
        arguments: Arguments<'values, 'gc>,
        temporaries: usize,
    ) -> Result<ActivationReference, Refusal<'values, 'gc>> {
        // Only a list of values can pass the register ceiling: a send's
        // window counts its slots in 16 bits, so for `Slots` the test is
        // the variant's alone and folds away where the variant is known.
        if matches!(arguments, Arguments::Values(values) if values.len() > usize::from(u16::MAX)) {
            return Err(Refusal::PastTheRegisterCeiling);
        }
        if header.counted && self.heap.counted_depth() >= MAX_ACTIVATION_DEPTH {
            return Err(Refusal::AtTheLimit(arguments));
        }
        self.heap
            .push_activation_from(header, arguments, temporaries)
            .map_err(|PushRefused| Refusal::StackFull(arguments))
    }

    /// Pushes a send's own callee on the send's argument slots, from
    /// register `argument_base` of the sender's window, so that they are
    /// its arguments (`Heap::push_overlapped_activation`;
    /// `docs/design/execution.md`, "Overlapping register windows").
    /// `window` is where the decision read the arguments: the slots, or
    /// their sources where those are consecutive registers, which are
    /// copied into the slots first -- the callee's header goes over every
    /// register above the slots, a source there included. From then on
    /// the slots are the arguments, so that the depth limit's and the
    /// reservation's cold paths, which push above the sender's window,
    /// copy them from there. The depth limit is checked as `push_above`
    /// checks it.
    #[inline(always)]
    fn push_on_slots<'values>(
        &mut self,
        header: &mut ActivationHeader<'gc>,
        window: ArgumentSlots,
        argument_base: u16,
        temporaries: usize,
    ) -> Result<ActivationReference, Refusal<'values, 'gc>> {
        let register_base = window.register_base();
        let count = window.count();
        if window.start() != argument_base {
            self.heap.place_in_slots(window, argument_base);
        }
        let slots = Arguments::Slots(ArgumentSlots::new(register_base, argument_base, count));
        if header.counted && self.heap.counted_depth() >= MAX_ACTIVATION_DEPTH {
            return Err(Refusal::AtTheLimit(slots));
        }
        self.heap
            .push_overlapped_activation(header, register_base, argument_base, count, temporaries)
            .map_err(|PushRefused| Refusal::StackFull(slots))
    }

    /// The cold path of a push `refusal` refused, `header` the activation
    /// it was to push: the register ceiling's error, the depth limit's
    /// reserve or error (`push_past_the_limit`), or the stack's reserve or
    /// the depth limit's error (`push_into_the_stack_reserve`). Boxed from
    /// here on, so that only these paths keep the header whole in memory.
    #[cold]
    #[inline(never)]
    fn push_refused(
        &mut self,
        refusal: Refusal<'_, 'gc>,
        header: ActivationHeader<'gc>,
        temporaries: usize,
    ) -> Result<ActivationReference, Signal<'gc>> {
        match refusal {
            Refusal::PastTheRegisterCeiling => {
                Err(self.refuse_past_the_register_ceiling(header.kind, header.receiver))
            }
            Refusal::AtTheLimit(arguments) => {
                self.push_past_the_limit(Box::new(header), arguments, temporaries)
            }
            Refusal::StackFull(arguments) => {
                self.push_into_the_stack_reserve(Box::new(header), arguments, temporaries)
            }
        }
    }

    /// The refusal of a push handed more arguments than the register
    /// ceiling, `u16::MAX`, which a frame cannot record: the too-large
    /// error the callee's body would raise. Every arity check holds a body
    /// that compiled to at most `u16::MAX` parameters, so only a body past
    /// the register ceiling -- compiled to one that raises before it reads
    /// anything (`compiler::body_too_large`) -- can be handed more, by a
    /// host's send or `Message>>sendTo:`. Refusing it here signals the
    /// same error and delivers a resumed value to the same send; only the
    /// signalling activation differs, the sender instead of a
    /// one-instruction callee.
    #[cold]
    #[inline(never)]
    fn refuse_past_the_register_ceiling(
        &mut self,
        kind: ActivationKind<'gc>,
        receiver: Value<'gc>,
    ) -> Signal<'gc> {
        literals::body_too_large_error(kind, receiver, crate::bytecode::BodyLimit::Registers)
    }

    /// A push the ordinary room of the activation stack's reservation could
    /// not hold. While the depth limit's reserve is held -- a VM error's
    /// `signal` is running or being pushed -- the frame may use the room
    /// the ordinary pushes leave free, as a counted push may go
    /// `DEPTH_RESERVE` past L; a push that room cannot hold either, and
    /// every push when the reserve is not held, is refused
    /// (`refuse_exhausted_push`).
    #[cold]
    #[inline(never)]
    fn push_into_the_stack_reserve(
        &mut self,
        header: Box<ActivationHeader<'gc>>,
        arguments: Arguments<'_, 'gc>,
        temporaries: usize,
    ) -> Result<ActivationReference, Signal<'gc>> {
        // The order `push_past_the_limit` asks in: the push that makes a
        // freshly taken reserve held is asked about first.
        if !self.pushes_the_reserves_does_not_understand(&header) && !self.depth_reserve_held() {
            return Err(self.refuse_exhausted_push(header));
        }
        self.heap
            .push_reserved_activation(*header, arguments, temporaries)
            .map_err(|exhausted| self.refuse_exhausted_push(exhausted.header))
    }

    /// The refusal of a push the activation stack's reservation could not
    /// hold (`Heap::push_activation`'s `StackExhausted`): the depth limit's
    /// own `stack overflow: recursion too deep`, since the reservation is
    /// the net beneath the limit for frames whose register files the
    /// activation count does not bound. While the reserve is held -- a VM
    /// error's `signal` is running or being pushed -- the run ends instead,
    /// as an overrun of the reserve does, so that a `signal` the
    /// reservation refuses in turn cannot raise the error again.
    #[cold]
    #[inline(never)]
    fn refuse_exhausted_push(&mut self, header: Box<ActivationHeader<'gc>>) -> Signal<'gc> {
        if self.depth_reserve_held() {
            Signal::Terminate(Box::new(self.overrun_of_the_reserve(&header)))
        } else {
            recursion_too_deep(&header)
        }
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
        header: Box<ActivationHeader<'gc>>,
        arguments: Arguments<'_, 'gc>,
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
            return self
                .heap
                .push_activation(*header, arguments, temporaries)
                .or_else(|exhausted| {
                    self.push_into_the_stack_reserve(exhausted.header, arguments, temporaries)
                });
        } else {
            Signal::Terminate(Box::new(self.overrun_of_the_reserve(&header)))
        };
        // A refused activation never runs, and no continuation begins
        // anything before its push has succeeded -- a nested class's
        // creation is marked only then -- so nothing is undone here.
        Err(refusal)
    }

    /// Whether the depth limit's reserve is held: its activation is still
    /// running -- on the stack or suspended into its frame -- and directly
    /// above it is the `signal` of the VM error that arose there -- or its
    /// `Object>>doesNotUnderstand:` -- or that `signal` is being pushed.
    /// Forgets a reserve found no longer held, so that the pop that ended
    /// it pays nothing to give it back.
    pub(crate) fn depth_reserve_held(&mut self) -> bool {
        let Some(frame) = self.depth_reserve else {
            return false;
        };
        let held = self.heap.frame_is_running(frame)
            && (self.taking_depth_reserve
                || self
                    .activation_above_frame(frame)
                    .is_some_and(|above| self.holds_the_depth_reserve(above)));
        if !held {
            self.set_depth_reserve(None);
        }
        held
    }

    /// The activation directly above the running activation of `frame`,
    /// the one it sent to: the next stack frame while the frame is married;
    /// while it is suspended, the one a walk from the top of the stack meets
    /// just before it (`WalkedActivation::sender`), which reifies nothing.
    fn activation_above_frame(&self, frame: FrameReference<'gc>) -> Option<WalkedActivation<'gc>> {
        if let Some(activation) = self.heap.frame_activation(frame) {
            return self
                .heap
                .activation_above(activation)
                .map(WalkedActivation::Live);
        }
        let mut above = None;
        let mut current = self.heap.top_activation().map(WalkedActivation::Live);
        while let Some(walked) = current {
            if walked.is_activation_of(&self.heap, frame) {
                return above;
            }
            above = Some(walked);
            current = walked.sender(&self.heap);
        }
        None
    }

    /// Whether `activation` is one the reserve is taken for: the `signal`
    /// the unwinder pushed for a VM error (`Continuation::VmErrorSignalled`),
    /// or the kernel's own `Object>>doesNotUnderstand:` -- §5.5's error, the
    /// VM's as much as any other, though `Kernel.ns` signals it.
    pub(crate) fn holds_the_depth_reserve(&self, activation: WalkedActivation<'gc>) -> bool {
        activation.signals_a_vm_error(&self.heap) || self.is_kernel_does_not_understand(activation)
    }

    /// Whether `activation` runs the kernel's `Object>>doesNotUnderstand:`,
    /// not an override of it: the one method of that selector whose current
    /// class is `Object` itself.
    pub(crate) fn is_kernel_does_not_understand(&self, activation: WalkedActivation<'gc>) -> bool {
        let object = self.builtin_class(BuiltinClass::Object);
        match activation.kind(&self.heap) {
            ActivationKind::Method(selector) => {
                selector == Symbol::DOES_NOT_UNDERSTAND
                    && activation
                        .current_class(&self.heap)
                        .is_some_and(|class| class.same(object))
            }
            _ => false,
        }
    }

    /// Whether `header` is the kernel's `Object>>doesNotUnderstand:`, about
    /// to be pushed directly above the activation that has just taken the
    /// reserve for it: the decision took the reserve, and this is the push
    /// that makes it held. That activation is the top of the stack, never
    /// suspended, since the push follows the decision with no safepoint
    /// between them: a reserve whose frame is suspended was taken earlier.
    fn pushes_the_reserves_does_not_understand(&self, header: &ActivationHeader<'gc>) -> bool {
        let Some(frame) = self.depth_reserve else {
            return false;
        };
        let Some(sender) = self.heap.frame_activation(frame) else {
            return false;
        };
        let object = self.builtin_class(BuiltinClass::Object);
        let is_kernel_handler = matches!(header.kind,
            ActivationKind::Method(selector) if selector == Symbol::DOES_NOT_UNDERSTAND)
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
            || (self.heap.counted_depth() < MAX_ACTIVATION_DEPTH - DEPTH_RESERVE
                && !self.heap.stack_is_within_the_reserve_of_its_limit())
            || self.depth_reserve_held()
        {
            return;
        }
        if let Some(top) = self.heap.top_activation() {
            let frame = self.heap.reify(top);
            // The handler returns by an ordinary pop, which no hook can
            // see, so the reserve it holds never raises the limit word:
            // machine code leaves a push at the limit to the interpreter,
            // which knows the reserve is held.
            self.depth_reserve = Some(frame);
            self.heap.set_depth_limit(MAX_ACTIVATION_DEPTH);
        }
    }

    /// Takes or gives back the depth limit's reserve for a VM error's
    /// `signal`: `depth_reserve` and the `DepthLimit` machine word change
    /// together, so that the limit machine code compares a push against is
    /// the one the reserve implies. A `signal` never returns -- it leaves
    /// by an unwind, which pops through `pop_for_unwind`, and that gives the
    /// reserve back (`release_depth_reserve_if_ended`) the moment it is no
    /// longer held -- so the word is the higher limit exactly while the
    /// reserve is held.
    pub(crate) fn set_depth_reserve(&mut self, frame: Option<FrameReference<'gc>>) {
        self.depth_reserve = frame;
        let limit = if frame.is_some() {
            MAX_ACTIVATION_DEPTH + DEPTH_RESERVE
        } else {
            MAX_ACTIVATION_DEPTH
        };
        self.heap.set_depth_limit(limit);
    }

    /// Gives the reserve back as soon as an unwind's pop has ended it, so
    /// that the `DepthLimit` word does not wait for the next slow-path
    /// question (`depth_reserve_held` forgets a reserve it finds ended).
    #[inline]
    fn release_depth_reserve_if_ended(&mut self) {
        if self.depth_reserve.is_some() {
            self.depth_reserve_held();
        }
    }

    /// `code`'s feedback base (`InlineCaches::base_for`), requesting a full
    /// collection when the range it minted grew the sites to their growth
    /// limit. Every caller of `base_for` goes through it, machine code's
    /// leaves included: the request only sets flags. A family that already
    /// has its range -- every push but its first -- pays the lookup alone.
    #[inline]
    pub(crate) fn feedback_base_for(
        &mut self,
        code: &Rc<crate::bytecode::CompiledCode>,
    ) -> FeedbackBase {
        match self.caches.known_base(code) {
            Some(base) => base,
            None => self.mint_feedback_base(code),
        }
    }

    /// `feedback_base_for` for a family without its range yet.
    #[cold]
    #[inline(never)]
    fn mint_feedback_base(&mut self, code: &Rc<crate::bytecode::CompiledCode>) -> FeedbackBase {
        let base = self.caches.base_for(code);
        if self.caches.wants_collection() {
            self.heap.request_table_collection();
        }
        base
    }

    /// A range of `size` sites for a closure an image decoded, which
    /// carries none (`InlineCaches::fresh_loose_range`), requesting a full
    /// collection as `feedback_base_for` does.
    #[cold]
    fn fresh_feedback_range(&mut self, size: u32) -> FeedbackBase {
        let base = self.caches.fresh_loose_range(size);
        if self.caches.wants_collection() {
            self.heap.request_table_collection();
        }
        base
    }

    /// Resumes the suspended activation of `frame`
    /// (`Heap::resume_activation`) and answers it. Its feedback base is
    /// derived again by `push_step`'s rule for its kind -- a closure's
    /// from the closure object, minted when it has none; every other
    /// body's, the root of its family, from `InlineCaches::base_for` --
    /// so the side table holds no base a full collection could free while
    /// the activation is suspended.
    pub(crate) fn resume_frame(&mut self, frame: FrameReference<'gc>) -> ActivationReference {
        let feedback_base = match self.heap.frame_kind(frame) {
            FrameKind::Closure(closure) => {
                let (code, _, closure_feedback_base) = self
                    .heap
                    .closure_activation(closure)
                    .expect("internal: a closure frame names a closure object");
                if closure_feedback_base == FeedbackBase::NOT_ASSIGNED {
                    let fresh = self.fresh_feedback_range(code.family_size);
                    self.heap.closure_set_feedback_base(closure, fresh);
                    fresh
                } else {
                    closure_feedback_base
                }
            }
            _ => {
                let code = self.heap.frame_code(frame).expect(
                    "internal: the frame of an activation the runtime pushed names its code",
                );
                self.feedback_base_for(&code)
            }
        };
        self.heap.resume_activation(frame, feedback_base)
    }

    /// Pushes the activation `step` describes. `send_arguments` are the
    /// arguments of the send that decided it, which `StepArguments::Send`
    /// refers to; `sender` is the activation that sent it, if the run loop did,
    /// and `inlinable` whether the send was a `SendInlinable` -- the two things
    /// the depth count's one exemption asks about.
    pub(crate) fn push_step(
        &mut self,
        step: Step<'gc>,
        send_arguments: Arguments<'_, 'gc>,
        sender: Option<ActivationReference>,
        inlinable: bool,
    ) -> Result<ActivationReference, Signal<'gc>> {
        self.push_step_placed(step, send_arguments, sender, inlinable, None)
    }

    /// `push_step`, an activation that takes the send's own arguments laid
    /// on the send's slots from register `slots` of the sender's window
    /// when it is given (`push_placed`): the run loop's sends give it.
    #[inline(always)]
    fn push_step_placed(
        &mut self,
        step: Step<'gc>,
        send_arguments: Arguments<'_, 'gc>,
        sender: Option<ActivationReference>,
        inlinable: bool,
        slots: Option<u16>,
    ) -> Result<ActivationReference, Signal<'gc>> {
        // Only an activation that takes the send's own arguments is the
        // send's own callee; any other lands above the sender's window.
        let on_slots = |arguments: &StepArguments<'gc>| match arguments {
            StepArguments::Send => slots,
            StepArguments::ControlFlow | StepArguments::Owned(_) => None,
        };
        match step {
            Step::Value(_) => unreachable!("internal: a value needs no activation"),
            Step::Method {
                receiver,
                method,
                defining_class,
                arguments,
                feedback_base,
            } => {
                let slots = on_slots(&arguments);
                let owned = arguments.owned();
                let arguments = arguments.window(send_arguments, &owned);
                check_method_arity(&method, receiver, arguments.len())?;
                let code = method.code.clone();
                // A decision never knows its method's range -- only
                // `push_step`, about to activate the family's root, does --
                // so every decider hands `NOT_ASSIGNED` and this is where it
                // is found, once, per family (`inline_cache::InlineCaches::base_for`).
                let feedback_base = if feedback_base == FeedbackBase::NOT_ASSIGNED {
                    self.feedback_base_for(&code)
                } else {
                    feedback_base
                };
                let temporaries = code.registers_above_header();
                self.push_placed(
                    ActivationHeader {
                        receiver,
                        current_class: Some(defining_class),
                        kind: ActivationKind::Method(method.selector),
                        method: Some(method),
                        code,
                        then: Continuation::Deliver,
                        counted: true,
                        feedback_base,
                    },
                    arguments,
                    slots,
                    temporaries,
                )
            }
            // A closure activation catches no return: its `^` targets its home.
            Step::Block {
                closure,
                arguments,
                then,
            } => {
                // A block a control-flow step activates, or one a
                // `SendInlinable` sends `value` -- the reference lowering's
                // loop (`compiler::reference`'s `while_loop`); no other
                // decision about an inlinable send answers a block with the
                // send's own arguments.
                let control_flow = match arguments {
                    StepArguments::ControlFlow => true,
                    StepArguments::Send => inlinable,
                    StepArguments::Owned(_) => false,
                };
                let slots = on_slots(&arguments);
                let owned = arguments.owned();
                let arguments = arguments.window(send_arguments, &owned);
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
                    let fresh = self.fresh_feedback_range(code.family_size);
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
                // `value` never reads the sender's frame.
                let created_by_sender = |heap: &Heap<'gc>| {
                    sender
                        .and_then(|sender| heap.activation_reified(sender))
                        .is_some_and(|frame| frame.same(captured_frame))
                };
                let counted = !(inlinable && control_flow && created_by_sender(&self.heap));
                let receiver = self.heap.frame_receiver(captured_frame);
                let current_class = self.heap.frame_current_class(captured_frame);
                let temporaries = code.registers_above_header();
                let activation = self.push_placed(
                    ActivationHeader {
                        receiver,
                        current_class,
                        kind: ActivationKind::Closure(closure),
                        method: None,
                        code,
                        then,
                        counted,
                        feedback_base,
                    },
                    arguments,
                    slots,
                    temporaries,
                )?;
                // The copied values sit between the block's parameters and
                // its temporaries, so they are written once the register
                // file exists and before the body runs an instruction
                // (`docs/design/execution.md`, "Converted representation").
                self.heap.fill_copied_values(activation, closure);
                Ok(activation)
            }
            // A factory, an initializer or a superclass clause is never an
            // activation the optimized lowering inlines away, so each of the
            // three below is always counted.
            Step::Factory {
                class,
                code,
                arguments,
            } => {
                let slots = on_slots(&arguments);
                let owned = arguments.owned();
                let arguments = arguments.window(send_arguments, &owned);
                // The entry's arity is its selector's, which the send
                // matched, and a host's factory is built for its count;
                // checked all the same, since the window is laid out from
                // the count.
                if arguments.len() != usize::from(code.argument_count)
                    && !code.stands_for_too_many_arguments(arguments.len())
                {
                    return Err(instantiate::factory_arity_error(
                        class,
                        usize::from(code.argument_count),
                        arguments.len(),
                    ));
                }
                let feedback_base = self.feedback_base_for(&code);
                let temporaries = code.registers_above_header();
                self.push_placed(
                    ActivationHeader {
                        receiver: Value::object(class),
                        current_class: Some(class),
                        kind: ActivationKind::Factory(class),
                        method: None,
                        code,
                        then: Continuation::Deliver,
                        counted: true,
                        feedback_base,
                    },
                    arguments,
                    slots,
                    temporaries,
                )
            }
            Step::Initializer {
                receiver,
                level,
                arguments,
                body,
                code,
            } => {
                let slots = on_slots(&arguments);
                let owned = arguments.owned();
                self.push_initializer_placed(
                    receiver,
                    level,
                    arguments.window(send_arguments, &owned),
                    slots,
                    body,
                    code,
                )
            }
            Step::SuperclassClause(creation) => {
                let code = nested::superclass_clause_code(&creation.nested);
                let feedback_base = self.feedback_base_for(&code);
                let temporaries = code.registers_above_header();
                let (enclosing, index, serial) =
                    (creation.enclosing, creation.index, creation.serial);
                let activation = self.push(
                    ActivationHeader {
                        receiver: Value::object(enclosing),
                        current_class: Some(creation.declaring_class),
                        kind: ActivationKind::SuperclassClause(creation.declaring_class),
                        method: None,
                        code,
                        then: Continuation::NestedClass(creation),
                        counted: true,
                        feedback_base,
                    },
                    Arguments::NONE,
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

    /// `push_step` for the step a send in the run loop decided, answering
    /// the activation to run and the instruction to start it at: a factory
    /// whose body is `compiler::factory_body` is carried through its first
    /// two instructions at once (`enter_factory_body`), every other step is
    /// pushed and started at its first. Only the run loop's sends come here,
    /// since only they switch to what is answered: a host's run starts at
    /// the activation it pushed, and a continuation of the pushed
    /// activation is written by some other callers.
    ///
    /// `slots` is the first of the send's argument slots, for a send whose
    /// callee is laid on them (`push_on_slots`); `None` for one whose
    /// arguments are the sender's own -- `InitializeInstance`'s, the
    /// factory's -- or a mirror send's, whose callee lands above the
    /// sender's window.
    pub(crate) fn push_sent(
        &mut self,
        step: Step<'gc>,
        send_arguments: Arguments<'_, 'gc>,
        sender: Option<ActivationReference>,
        inlinable: bool,
        slots: Option<u16>,
    ) -> Result<(ActivationReference, usize), Signal<'gc>> {
        let factory = match &step {
            Step::Factory {
                class,
                code,
                arguments: StepArguments::Send,
            } => instantiate::factory_registers(code).map(|registers| (*class, registers)),
            _ => None,
        };
        let pushed = self.push_step_placed(step, send_arguments, sender, inlinable, slots)?;
        match factory {
            Some((class, registers)) => {
                // The factory's own arguments, where its initializer reads
                // them: a source the window named above the slots is gone
                // under the factory's header.
                let arguments = ArgumentSlots::new(
                    self.heap.activation_register_base(pushed),
                    0,
                    register_number(send_arguments.len()),
                );
                self.enter_factory_body(pushed, class, registers, Arguments::Slots(arguments))
            }
            None => Ok((pushed, 0)),
        }
    }

    /// `push_sent`, but for a method's activation on the send's own slots
    /// whose body is compiled: the run loop's send calls its machine code
    /// instead (`call_sent`; `docs/design/execution.md`, "The entry at a
    /// send"), which answers its value or where an exit left the run
    /// loop. The test is inline: three loads, from the method to its code
    /// to the code's slot, and one compare; everything else is out of
    /// line.
    ///
    /// The step is lent to the call and moved only into the one push
    /// below, so that it reaches `push_sent` from the place the decision
    /// wrote it: with a second by-value consumer, the call, the step was
    /// first copied into a place of its own, reading the decision's
    /// narrow stores back wide -- a store-forwarding stall on every
    /// interpreted push.
    #[inline(always)]
    pub(crate) fn enter_sent(
        &mut self,
        step: Step<'gc>,
        send_arguments: Arguments<'_, 'gc>,
        sender: Option<ActivationReference>,
        inlinable: bool,
        slots: Option<u16>,
    ) -> Result<Sent<'gc>, Signal<'gc>> {
        let mut arguments = send_arguments;
        match &step {
            Step::Method {
                receiver,
                method,
                defining_class,
                arguments: StepArguments::Send,
                feedback_base,
            } => {
                if slots.is_some() && method.code.jit.is_compiled() {
                    match self.call_sent(
                        *receiver,
                        method,
                        *defining_class,
                        *feedback_base,
                        send_arguments,
                        sender,
                        slots,
                    )? {
                        Call::Ran(sent) => return Ok(sent),
                        Call::Push(placed) => arguments = placed,
                    }
                }
            }
            // A block that takes the send's own arguments, the
            // activation a `value` send decides, delivering its value,
            // whose body has a machine entry row: told apart by its
            // closure's code id alone (`Heap::closure_has_machine_entry`),
            // out of line and cheap, so that a block that stays
            // interpreted -- one the JIT abandoned among them -- reads its
            // closure once, at its push, and pays no call's frame for it.
            Step::Block {
                closure,
                arguments: StepArguments::Send,
                then: Continuation::Deliver,
            } if slots.is_some()
                && self.jit.is_enabled()
                && self.heap.closure_has_machine_entry(*closure) =>
            {
                match self.call_sent_closure(*closure, send_arguments, sender, inlinable, slots) {
                    Call::Ran(sent) => return Ok(sent),
                    Call::Push(placed) => arguments = placed,
                }
            }
            _ => {}
        }
        self.push_sent(step, arguments, sender, inlinable, slots)
            .map(|(activation, program_counter)| Sent::Pushed(activation, program_counter))
    }

    /// `enter_sent`'s call of a compiled method's machine code, the
    /// method's activation on the send's own slots: the arity checked as
    /// the push checks it, the arguments placed into the slots and the
    /// feedback base resolved as the push places and resolves them, and
    /// the body called (`jit::called`). A call the JIT refuses -- the body
    /// compiled into another zone or against another feedback base, too
    /// little native stack, its prologue's refusal -- is the push's, which
    /// raises the overflow or grows the stack as for any push; so is every
    /// send that does not name the slots and arguments a call needs.
    /// `Call::Push` names where the push takes the arguments from. A
    /// refused call's push resolves an unassigned feedback base again,
    /// which `InlineCaches::base_for` answers the same, since nothing ran
    /// between the two.
    #[allow(
        clippy::too_many_arguments,
        reason = "the step's parts, lent apart so that the step itself is moved only into the push"
    )]
    #[cold]
    #[inline(never)]
    fn call_sent<'values>(
        &mut self,
        receiver: Value<'gc>,
        method: &Rc<crate::mixin::MixinMethod>,
        defining_class: ClassReference<'gc>,
        feedback_base: FeedbackBase,
        send_arguments: Arguments<'values, 'gc>,
        sender: Option<ActivationReference>,
        slots: Option<u16>,
    ) -> Result<Call<'values, 'gc>, Signal<'gc>> {
        let (Arguments::Slots(window), Some(sender_activation), Some(argument_base)) =
            (send_arguments, sender, slots)
        else {
            return Ok(Call::Push(send_arguments));
        };
        // A body that stands for too many arguments takes another count
        // than its method's; only the push lays such a frame out.
        if method.argument_count != usize::from(method.code.argument_count) {
            return Ok(Call::Push(send_arguments));
        }
        check_method_arity(method, receiver, usize::from(window.count()))?;
        let feedback_base = if feedback_base == FeedbackBase::NOT_ASSIGNED {
            self.feedback_base_for(&method.code)
        } else {
            feedback_base
        };
        let register_base = window.register_base();
        if window.start() != argument_base {
            self.heap.place_in_slots(window, argument_base);
        }
        Ok(
            match crate::jit::called(
                self,
                sender_activation,
                register_base,
                argument_base,
                receiver,
                defining_class,
                &method.code,
                feedback_base,
            ) {
                crate::jit::Called::Returned { value } => {
                    Call::Ran(Sent::Value(self.heap.machine_value(value)))
                }
                crate::jit::Called::Stopped(resumption) => Call::Ran(Sent::Stopped(resumption)),
                // From the slots, where the arguments now are.
                crate::jit::Called::Refused => Call::Push(Arguments::Slots(ArgumentSlots::new(
                    register_base,
                    argument_base,
                    window.count(),
                ))),
            },
        )
    }

    /// `enter_sent`'s call of a block's machine code, the block's
    /// activation on the send's own slots, `call_sent`'s for a method: the
    /// arity, the feedback base and the depth count's exemption decided as
    /// `push_step`'s `Step::Block` arm decides them, the arguments placed
    /// into the slots, and the body called (`jit::called_closure`). A body
    /// without machine code, a wrong arity, which the push raises, a
    /// closure without a feedback base of its own, which the push gives
    /// one, and every call the JIT refuses are the push's: `Call::Push`
    /// names where it takes the arguments from. Called only for a closure
    /// whose body has a machine entry row (`enter_sent`).
    #[cold]
    #[inline(never)]
    fn call_sent_closure<'values>(
        &mut self,
        closure: ObjectReference<'gc>,
        send_arguments: Arguments<'values, 'gc>,
        sender: Option<ActivationReference>,
        inlinable: bool,
        slots: Option<u16>,
    ) -> Call<'values, 'gc> {
        let (Arguments::Slots(window), Some(sender_activation), Some(argument_base)) =
            (send_arguments, sender, slots)
        else {
            return Call::Push(send_arguments);
        };
        let Some((code, captured_frame, feedback_base)) = self.heap.closure_activation(closure)
        else {
            return Call::Push(send_arguments);
        };
        if !code.jit.is_compiled()
            || usize::from(code.argument_count) != usize::from(window.count())
            || feedback_base == FeedbackBase::NOT_ASSIGNED
        {
            return Call::Push(send_arguments);
        }
        // `push_step`'s exemption: a block a `SendInlinable` sends `value`
        // with the send's own arguments is control flow, and exempt when
        // the sending activation created it.
        let counted = !(inlinable
            && self
                .heap
                .activation_reified(sender_activation)
                .is_some_and(|frame| frame.same(captured_frame)));
        let register_base = window.register_base();
        if window.start() != argument_base {
            self.heap.place_in_slots(window, argument_base);
        }
        match crate::jit::called_closure(
            self,
            sender_activation,
            register_base,
            argument_base,
            closure,
            &code,
            feedback_base,
            counted,
        ) {
            crate::jit::Called::Returned { value } => {
                Call::Ran(Sent::Value(self.heap.machine_value(value)))
            }
            crate::jit::Called::Stopped(resumption) => Call::Ran(Sent::Stopped(resumption)),
            // From the slots, where the arguments now are.
            crate::jit::Called::Refused => Call::Push(Arguments::Slots(ArgumentSlots::new(
                register_base,
                argument_base,
                window.count(),
            ))),
        }
    }

    /// Pushes the initializer body `body` of `level`'s mixin on `receiver`,
    /// entered at `level`. The decision found the body, and nothing has run
    /// since.
    pub(super) fn push_initializer(
        &mut self,
        receiver: Value<'gc>,
        level: ClassReference<'gc>,
        arguments: Arguments<'_, 'gc>,
        body: instantiate::InitializerBody,
        code: Rc<crate::bytecode::CompiledCode>,
    ) -> Result<ActivationReference, Signal<'gc>> {
        self.push_initializer_placed(receiver, level, arguments, None, body, code)
    }

    /// `push_initializer`, laid on the send's slots from register `slots`
    /// when it is given (`push_placed`).
    fn push_initializer_placed(
        &mut self,
        receiver: Value<'gc>,
        level: ClassReference<'gc>,
        arguments: Arguments<'_, 'gc>,
        slots: Option<u16>,
        body: instantiate::InitializerBody,
        code: Rc<crate::bytecode::CompiledCode>,
    ) -> Result<ActivationReference, Signal<'gc>> {
        // The decision checked the arguments against the level's factory,
        // whose arity `image::decode::verify_mixins` holds every body to.
        debug_assert!(
            arguments.len() == usize::from(code.argument_count)
                || code.stands_for_too_many_arguments(arguments.len()),
            "internal: an initializer is pushed with its own number of arguments"
        );
        let feedback_base = self.feedback_base_for(&code);
        let temporaries = code.registers_above_header();
        self.push_placed(
            ActivationHeader {
                receiver,
                current_class: Some(level),
                kind: ActivationKind::Initializer(level, body),
                method: None,
                code,
                then: Continuation::Deliver,
                counted: true,
                feedback_base,
            },
            arguments,
            slots,
            temporaries,
        )
    }

    /// What a native does once the block it activated has returned
    /// `value`: the second half of the native, which the run loop
    /// waits for instead of a Rust frame. `then` is the continuation of the
    /// block's activation, which the loop has just popped. It answers the
    /// value for the caller, or another activation to run in the popped
    /// one's place, which the loop hands to `push_continuation`; or a
    /// signal.
    pub(crate) fn resume(
        &mut self,
        then: Continuation<'gc>,
        value: Value<'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        match then {
            Continuation::Deliver => Ok(Step::Value(value)),
            Continuation::NestedClass(creation) => {
                nested::end_creation(self, &creation);
                nested::after_clause(self, *creation, value)
            }
            // The block's value is dropped: the unwind it ran for goes on,
            // raised again as the signal it was, from the activation it had
            // stopped at -- now the top of the stack again.
            Continuation::Unwind(pending) => Err(unwind::signal_of(*pending)),
            Continuation::VmErrorSignalled(error) => Err(Signal::Terminate(error)),
            Continuation::Reexecute => {
                unreachable!("internal: vm::finish runs the instruction again itself")
            }
            Continuation::ReturnToSuspended(_) => unreachable!(
                "internal: the run loop resumes a suspended sender before it applies the continuation"
            ),
        }
    }

    /// What a continuation undoes when its activation is thrown away rather
    /// than returned from: the counterpart of `resume`, which `unwind::unwind`
    /// calls for every activation it pops, and `push_step` for a block
    /// activation it refuses. Only a nested class's creation has begun
    /// anything -- its cache slot is marked as in progress -- so only it has
    /// anything to undo, and a return to a suspended sender undoes what the
    /// continuation it wraps began; every other continuation is simply
    /// dropped, a `Continuation::Unwind` included: a newer unwind, or an
    /// error, has replaced the one it would have carried on, and a
    /// `Continuation::Reexecute`: a future's `resolve` undoes its own
    /// forcing, by its `ifCurtailed:`.
    pub(crate) fn abandon(&mut self, then: Continuation<'gc>) {
        match then {
            Continuation::NestedClass(creation) => nested::end_creation(self, &creation),
            // The sender stays suspended: whoever throws this activation
            // away decides whether it resumes it.
            Continuation::ReturnToSuspended(suspended) => self.abandon(suspended.then),
            _ => {}
        }
    }

    /// Pops `top`, the top of the stack, for an unwind that goes on below
    /// it: its continuation is abandoned, and a sender it would have
    /// returned to suspended (`Continuation::ReturnToSuspended`) is resumed
    /// first, as `vm::finish` resumes it for a return, so that the next top
    /// is the activation the unwind goes on with -- whose `ensure:` block it
    /// runs, or which it arrives at -- as if it had been on the stack all
    /// along.
    pub(crate) fn pop_for_unwind(&mut self, top: ActivationReference) {
        let popped = self.heap.pop_activation(top);
        match popped.then {
            Continuation::ReturnToSuspended(suspended) => {
                let ReturnToSuspended { sender, then } = *suspended;
                self.resume_frame(sender);
                self.abandon(then);
            }
            then => self.abandon(then),
        }
        self.release_depth_reserve_if_ended();
    }

    /// Pushes the activation a continuation decided (`resume`), in place of
    /// the one just popped; `replaced_counted` is that one's
    /// `Popped::counted`. The rule lives here rather than in the run loop,
    /// which only forwards the flag: a block is exempt only if the
    /// activation it replaces was, and only if it too was created by the
    /// activation below it, which `push_step` checks. A method is counted
    /// whatever it replaces. No continuation replaces an exempt activation
    /// any more -- every exempt one delivers -- so everything pushed here
    /// is counted, and the flag is open work (`docs/OPEN-WORK.md`).
    ///
    /// It takes the flag rather than the whole `Popped`, whose continuation
    /// `resume` has consumed by then: a version that took the `Popped` and
    /// pushed inside `resume` measured about 1.3% slower.
    pub(crate) fn push_continuation(
        &mut self,
        step: Step<'gc>,
        replaced_counted: bool,
    ) -> Result<ActivationReference, Signal<'gc>> {
        let sender = self.heap.top_activation();
        // A continuation's step never refers to a send's arguments
        // (`StepArguments::Send` is only answered by a decision about a
        // send, and a continuation decides none), so `Arguments::NONE` is
        // always right here.
        self.push_step(step, Arguments::NONE, sender, !replaced_counted)
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
    /// to a live stack frame, or suspended into the frame, exactly until the
    /// activation returns (`Heap::frame_is_running`), and a frame the image
    /// decoder built never is. A suspended home is unwound to, resumed when
    /// the unwind's pops reach it. A returned home is §7.2's
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
            Some(home) if self.heap.frame_is_running(home) => home,
            Some(_) => {
                return Signal::Error(NewspeakError::cannot_return(
                    Symbol::CARET,
                    value,
                    value,
                    "cannotReturn: the enclosing method activation has already returned",
                ))
            }
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

    /// An `Array` of `length` nils.
    pub(crate) fn new_nil_array(&mut self, length: usize) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Array);
        self.heap.allocate_nil_array(class, length)
    }

    /// An `Array` of the registers `registers` names: `MakeArray`'s.
    pub(crate) fn new_array_from_registers(
        &mut self,
        register_base: crate::heap::RegisterBase,
        registers: &[u16],
    ) -> ObjectReference<'gc> {
        let class = self.builtin_class(BuiltinClass::Array);
        self.heap
            .allocate_array_from_registers(class, register_base, registers)
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
    /// anything asks for it -- unless a program made a symbol of its text
    /// before the text was interned, which is then adopted as the
    /// symbol's object, so that the text keeps one object (§5.1.6).
    pub fn new_symbol(&mut self, symbol: Symbol) -> Value<'gc> {
        if let Some(object) = self.heap.canonical_symbol(symbol) {
            return Value::object(object);
        }
        if let Some(object) = self.heap.adopt_program_symbol(symbol) {
            return Value::object(object);
        }
        let class = self.builtin_class(BuiltinClass::Symbol);
        let object = self.heap.allocate_symbol(class, symbol);
        self.heap.record_canonical_symbol(symbol, object);
        Value::object(object)
    }

    /// The Symbol of `text`, which a program chose (`asSymbol`): the
    /// interned symbol's object when something interned the text, and
    /// otherwise a program-made symbol, which takes no number -- the
    /// object the weak table holds for the text, or a new one recorded
    /// there -- so that a run making symbols of fresh texts leaves the
    /// interner as it was and its symbols die with their objects
    /// (`docs/design/memory.md`, "Weak references"). Either way the text
    /// has one object while anything reaches it (§5.1.6).
    pub fn program_symbol(&mut self, text: &str) -> Value<'gc> {
        if let Some(symbol) = Symbol::lookup(text) {
            return self.new_symbol(symbol);
        }
        if let Some(object) = self.heap.program_symbol_named(text) {
            return Value::object(object);
        }
        let class = self.builtin_class(BuiltinClass::Symbol);
        let object = self
            .heap
            .allocate_string(class, crate::CodePoints::from(text));
        self.heap.record_program_symbol(object);
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
    /// activation's stack frame, since only a reified frame survives its
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
    /// too -- that also carries the `<:` clause it was built for and the
    /// class object `named` that clause named. Its metaclass applies
    /// `class_side`, the class side whose factory and name the level has:
    /// the named class's for a level like any other (§6.2: "a primary
    /// factory method with the same selector as m"), and a body-less
    /// declaration's own for its class object, the level its last clause
    /// adds (`object-model.md`, "Mixin application"). That class object also
    /// has room for the declaration's own scope, nil until
    /// `nested::body_less_class` writes it -- and nil for good for a
    /// module's root, which no object encloses.
    pub(crate) fn new_applied_level(
        &mut self,
        level: AppliedLevel<'gc>,
        clause: crate::mixin::AppliedClause,
        named: ClassReference<'gc>,
        class_side: Rc<crate::mixin::Mixin>,
    ) -> ClassReference<'gc> {
        let AppliedLevel {
            application,
            enclosing,
            declaring_class,
            superclass,
        } = level;
        let object = self.builtin_class(BuiltinClass::Object);
        // The level a body-less declaration's last clause adds is the
        // declaration's class object, which also keeps the declaration's
        // scope (`Heap::class_declaration_scope`).
        let body_less = !clause.declaration.has_body
            && clause.index + 1 == clause.declaration.mixin_applications.len();
        let class = self.heap.allocate_applied_level(
            object,
            crate::heap::NewClass {
                application,
                enclosing,
                declaring_class,
                superclass,
                builtin: None,
            },
            named,
            body_less,
        );
        if !self.heap.class_set_applied_clause(class, clause) {
            unreachable!("internal: a level is a class object");
        }
        if self.metaclasses_ready {
            self.attach_metaclass_of(class, class_side);
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
        let level_facts = crate::mixin::LevelFacts::of(mixin);
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
                instance_header: None,
                level_facts,
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
    let selector = match header.kind {
        ActivationKind::Method(selector) => selector,
        ActivationKind::Closure(_) => Symbol::VALUE,
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

/// The body of `method` on `class`'s own mixin, `side` choosing the class
/// side: what the cache test hooks (`Interpreter::call_site_states`,
/// `Interpreter::call_site_cells`) read the sites of. `None` when `class`
/// is no class object or has no such method.
pub(crate) fn method_code(
    mutator: &Mutator<'_>,
    class: &Root,
    side: MethodSide,
    method: Symbol,
) -> Option<Rc<crate::bytecode::CompiledCode>> {
    let object = mutator.fetch(class).as_object()?;
    let application = mutator.heap.class_application(object)?;
    let mixin = match side {
        MethodSide::Instance => Some(application.mixin.clone()),
        MethodSide::Class => application.mixin.class_side.clone(),
    }?;
    mixin
        .compiled_method(method)
        .map(|found| found.code.clone())
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a test's values are small literals and lengths it built"
    )]
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
            match mutator.decide_send(receiver, Symbol::intern("foo"), Arguments::NONE) {
                Ok(Step::Method { method, .. }) => assert_eq!(method.selector.as_str(), "foo"),
                _ => panic!("a user method is decided as an activation"),
            }
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
            match mutator.decide_send(
                Value::small_integer(3),
                Symbol::intern("+"),
                Arguments::Values(&[Value::small_integer(4)].as_slice()),
            ) {
                Ok(Step::Value(answer)) if answer.as_small_integer() == Some(7) => {}
                _ => panic!("a native leaf is decided as its value"),
            }
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
            let slot = match mutator.decide_send(instance, Symbol::intern("N"), Arguments::NONE) {
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

    /// A factory send is decided as the factory's activation on the class
    /// object it was sent to, whatever the class's initializers do: the
    /// body of the entry its metaclass holds, with the send's arguments. A
    /// send decides nothing of the instance's initialization, which the
    /// factory's own instructions decide once it runs.
    #[test]
    fn deciding_new_answers_the_factory_as_an_activation() {
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
              public class M x: a = Object new (
                | m = a. |
              ) (
              ) : ()
            ) : ()
        "};
        let (mut interpreter, class) = load(source);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let p_class_root = expect_ok(interpreter.send(&instance, Symbol::intern("P"), &[]), "P");
        let q_class_root = expect_ok(interpreter.send(&instance, Symbol::intern("Q"), &[]), "Q");
        let m_class_root = expect_ok(interpreter.send(&instance, Symbol::intern("M"), &[]), "M");
        interpreter.enter(|mutator| {
            for (root, selector, arity) in [
                (&p_class_root, Symbol::NEW, 0),
                (&q_class_root, Symbol::NEW, 0),
                (&m_class_root, Symbol::intern("x:"), 1),
            ] {
                let class = mutator.fetch(root);
                let Some(class_object) = class.as_object() else {
                    panic!("the accessor answers a class object")
                };
                let arguments = vec![Value::small_integer(3); arity];
                match mutator.decide_send(class, selector, Arguments::Values(&arguments.as_slice()))
                {
                    Ok(Step::Factory {
                        class: decided,
                        code,
                        arguments: StepArguments::Send,
                    }) => {
                        assert!(decided.same(class_object), "{selector}");
                        assert_eq!(usize::from(code.argument_count), arity, "{selector}");
                        assert!(
                            matches!(
                                code.instruction(0),
                                crate::bytecode::Instruction::AllocateInstance { .. }
                            ),
                            "{selector}: the factory's body allocates first"
                        );
                    }
                    _ => panic!("`{selector}` is decided as the factory's activation"),
                }
            }
            assert_eq!(mutator.heap().activation_depth(), 0, "nothing was pushed");
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

    /// What the send, push and return path passes by value between
    /// functions the compiler keeps apart -- the argument window, the
    /// sender, the feedback base, a popped frame's continuation -- is at
    /// most two scalars. Rust's ABI passes at most two scalars in
    /// registers; a larger aggregate goes through memory, a copy built by
    /// narrow stores and read back wide, which stalled every send while
    /// `Arguments` was one (`docs/design/execution.md`, "Keeping the hot
    /// path in registers"). The size is what this holds; whether the layout
    /// stays a pair the codegen gate's count of wide loads sees. The window
    /// and the activation's handle are pinned in `heap`'s tests. Release
    /// only: a debug build's handles carry an origin and a move epoch.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn what_the_hot_path_passes_by_value_is_at_most_two_scalars() {
        // A tag and one word: the packed window, or a reference to a slice.
        assert_eq!(std::mem::size_of::<Arguments>(), 16);
        assert_eq!(std::mem::size_of::<Option<ActivationReference>>(), 8);
        assert_eq!(std::mem::size_of::<FeedbackBase>(), 4);
        // A tag and a box: every variant that owns more is boxed.
        assert_eq!(std::mem::size_of::<Continuation>(), 16);
    }

    /// The three values of the path that are larger than two scalars and
    /// still cross between its functions by value, through memory: the
    /// decided `Step` that `push_sent` and `push_step` take, and the
    /// pushes' results, as large as the `Signal` they may carry (the
    /// decision itself and the `Signal` are pinned above). Pinned at the size they were measured
    /// at, so that they do not grow; whether to lend them instead is open
    /// (`docs/OPEN-WORK.md`).
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn what_the_hot_path_passes_through_memory_stays_the_size_it_was_measured_at() {
        assert_eq!(std::mem::size_of::<Step>(), 56);
        assert_eq!(
            std::mem::size_of::<Result<ActivationReference, Signal>>(),
            40
        );
        assert_eq!(
            std::mem::size_of::<Result<(ActivationReference, usize), Signal>>(),
            40
        );
    }

    /// A recursion whose frames fill the stack's reservation long before the
    /// depth limit is reached. The transcript writes to the process's
    /// stdout, which a unit test cannot read, so `probe` answers what
    /// `main:args:` shows.
    const DEEP: &str = indoc! {"
        class Deep = Object new (
          |
            public deepest ::= 0.
          |
        ) (
          recurse: depth = (
            | first second third fourth |
            deepest:: depth.
            ^self recurse: depth + 1.
          )

          public probe = (
            ^[self recurse: 0] on: Error do: [:error | error messageText]
          )

          public main: platform args: arguments = (
            platform transcript
                show: ([self recurse: 0] on: Error do: [:error | error messageText]);
                newLine.
          )
        )
    "};

    crate::for_each_lowering! {
        /// A push the stack's reservation cannot hold is signalled as a push
        /// past the depth limit is: an `Error` an `on:do:` catches, with the
        /// same text, though the activation count is far below the limit.
        /// The handler runs on the reserve the refusal leaves.
        fn a_recursion_the_reservation_cannot_hold_is_a_catchable_stack_overflow(lowering) {
            let module = ns_syntax::parse(DEEP).expect("parse");
            let resolved = crate::resolve(
                module,
                Rc::from(DEEP),
                SourceOrigin::File(PathBuf::from("deep.ns")),
            )
            .expect("resolve");
            let mut interpreter =
                crate::heap::with_stack_reservation(1 << 20, || Interpreter::bootstrap_with(lowering));
            // The suspend stress mode moves the running activations off the
            // stack into their frame objects, so the reservation would never
            // fill and the depth limit would refuse the recursion instead.
            #[cfg(debug_assertions)]
            interpreter.set_stress_suspend(false);
            let class = interpreter.load_module(&resolved);
            let deep = expect_ok(interpreter.instantiate(&class, &[]), "Deep new");
            let text = expect_ok(
                interpreter.send(&deep, Symbol::intern("probe"), &[]),
                "Deep probe",
            );
            interpreter.enter(|mutator| {
                let value = mutator.fetch(&text);
                let Some(object) = value.as_object() else {
                    panic!("expected a string, got {}", mutator.heap().describe(value));
                };
                assert_eq!(
                    mutator.heap().str_value(object).as_deref(),
                    Some("stack overflow: recursion too deep")
                );
            });
            // The deepest level the recursion reached, far below L: the
            // reservation refused it, not the depth limit.
            let deepest = expect_ok(
                interpreter.send(&deep, Symbol::intern("deepest"), &[]),
                "Deep deepest",
            );
            interpreter.enter(|mutator| {
                let reached = mutator
                    .fetch(&deepest)
                    .as_small_integer()
                    .expect("a depth is a small integer");
                assert!(reached > 100, "the recursion got going: {reached}");
                assert!(
                    (reached as usize) < MAX_ACTIVATION_DEPTH / 4,
                    "the reservation refused it, not the depth limit: {reached}"
                );
            });
            let platform = interpreter.platform();
            let arguments = interpreter.enter(|mutator| {
                let array = mutator.new_array(Vec::new());
                mutator.stash(Value::object(array))
            });
            expect_ok(
                interpreter.send(&deep, Symbol::intern("main:args:"), &[platform, arguments]),
                "Deep main:args:",
            );
        }
    }
}
