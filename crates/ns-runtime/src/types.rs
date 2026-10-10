//! Core runtime data types: the object model, activation records, and the
//! resolver's output shape. What an object or a reified frame stores is not
//! here: it lives behind the heap seam (`heap/object.rs`), and is built and
//! read through `Heap`. Pure data -- see
//! `docs/design/object-model.md`, `docs/design/execution.md`'s
//! "Activations", and `docs/design/dispatch.md`'s "Resolution".

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::Symbol;

// ---------- Values and objects ----------

/// The handle types live in `heap.rs`, whose `Heap` is the only thing that
/// can interpret one. They are re-exported here because this module is
/// where the object model reads as a whole, and because `lib.rs`'s
/// `pub use types::*` is how the rest of the workspace names them.
pub use crate::heap::{
    fits_small_integer, ClassReference, FrameReference, ObjectReference, Value, ValueKind, Word,
    SMALL_INTEGER_MAX, SMALL_INTEGER_MIN,
};

/// §3.1.1's enclosing-object condition on a value class -- every enclosing
/// object, at every level of its class chain, must be a value -- decided
/// once, where the class object is created
/// (`interpreter::value_class::check_value_class`, called from
/// `interpreter::nested::after_clause`), and recorded on the class object rather
/// than raised there: `decide_instantiation` is what refuses, since the
/// specification's own §2.3 example creates such a class and never
/// instantiates it (`docs/design/object-model.md`, "A value object's four
/// conditions"). Recorded as heap data, an `Array` of the offending object
/// and the level's name (`Mutator::record_value_class_refusal`), so the
/// collector keeps the offender and moves it like any other field; the
/// message names it with `Heap::describe` when `new` refuses. An image
/// carries none of it: every decoded class object starts with none,
/// and `image::decode::rebuild_applications` recomputes it with the same
/// `check_value_class`, once every decoded class's real application is
/// installed, so a legitimate snapshot of a value class created carrying a
/// refusal decodes carrying the same one rather than becoming instantiable.
///
/// §3.1.1's other class-level condition, no identity method, is not a
/// `ValueClassRefusal` at all: it is a property of the *application*, not of
/// any class object built from it, so `Mutator::apply_mixin` folds it
/// straight into `MixinApplication::instantiation_refusal`
/// (`mixin::InstantiationRefusal::IdentityMethod`) instead, which covers a
/// module's root the same way it covers a nested class.
///
/// The enclosing-objects walk (`check_value_class`) climbs a new class
/// object's superclass chain and stops early only at a class that inherits
/// `Value`, is not a level a `<:` clause added, and carries **no** recorded
/// refusal of its own -- such a class was checked the same way when it was
/// created, so everything above it holds by induction. A class that does
/// carry one is not trusted blindly: the new class inherits the fault
/// instead, `offender` and `level` carried over from that class's own
/// verdict, except that a `level` of `None` -- a fault in the stopping
/// class's own enclosing object -- becomes `Some(<the stopping class's
/// name>)`. A fault inherited over several levels thus keeps naming the
/// level whose enclosing object is at fault.
#[derive(Debug, Clone, Copy)]
pub enum ValueClassRefusal<'gc> {
    /// `level` is `None` for the class's own enclosing object; `offender` is
    /// the enclosing object that is no value.
    NonValueEnclosingObject {
        level: Option<Symbol>,
        offender: ObjectReference<'gc>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinClass {
    Object,
    Boolean,
    True,
    False,
    Integer,
    Float,
    String,
    Symbol,
    Array,
    UndefinedObject,
    Closure,
    /// The VM mirror's class (`docs/design/core-library.md`, "The VM mirror
    /// is the whole Rust boundary"): one instance, built at bootstrap and
    /// handed to the core library alone. No program can name it -- it is
    /// not a bare builtin name, and the image format refuses its tag
    /// wherever a program names a class.
    VmMirror,
    /// §3.2's empty class, the root of the whole chain: it has no
    /// superclass and declares nothing, so that all code, `Object`'s
    /// included, resides in a mixin. Only `Object` may name it, which is
    /// why it is not a bare builtin name either.
    Top,
    /// §3.1.1: a value object's class inherits from `Value`.
    Value,
    /// §3.1.1 lists `Number` among the classes whose instances are values;
    /// `Integer` and `Float` are its subclasses.
    Number,
    /// What a class and a metaclass share (`newspeak-spec.pdf` §3.2 names
    /// neither this class nor `Metaclass`'s superclass; the hierarchy is
    /// Primordial Soup's). No program can name it, and no class that
    /// inherits it can be instantiated: class objects are made only by
    /// evaluating a declaration.
    Behavior,
    /// §3.2: every metaclass is a direct subclass of `Class`.
    Class,
    /// §3.2: every metaclass is an instance of `Metaclass`.
    Metaclass,
    /// A running activation as an object (`docs/design/core-library.md`,
    /// "Exceptions"): the frame it wraps, and nothing a program can
    /// reach -- it reads other frames and unwinds stacks, a capability
    /// Newspeak hands out only through mirrors. No program can name it:
    /// `Object` declares no accessor for it, and the image decoder refuses
    /// its tag wherever the graph names a class, as it refuses `Top`'s.
    Activation,
    /// §8.6: exception handling, built here over the activation. The root
    /// of the exception hierarchy; its slots are the exception's own state.
    Exception,
    Error,
    MessageNotUnderstood,
    /// A reified message send: the selector and arguments a
    /// `doesNotUnderstand:` was sent, or a `MessageNotUnderstood`'s own
    /// `message`.
    Message,
    Halt,
    /// §6.3.2's pull future, kernel-private; its instances are made by
    /// the runtime (`interpreter/future.rs`).
    FutureImplementation,
    /// §5.1.1's unbounded integers: an `Integer` beyond 61 bits. A
    /// subclass of `Integer`, whose instances are the immediates, so
    /// `3 class` stays `Integer`.
    LargeInteger,
    /// An `Error` for an argument a method cannot take; carries it.
    ArgumentError,
    /// An `Error` for a return to an activation that has returned; carries
    /// the value being returned.
    CannotReturn,
    /// An `Error` for a loop condition that answered no Boolean; carries
    /// that answer. Kernel-private: a program catches it as an `Error`.
    NonBooleanReceiver,
    /// An `Error` for a class that refused the factory it was sent; carries
    /// the class.
    CannotInstantiate,
    /// A forwarding object's superclass: it overrides every public member
    /// of `Object` as protected.
    Proxy,
    /// The public class whose class side makes a `FutureImplementation`;
    /// it has no instances.
    Future,
}

impl BuiltinClass {
    /// Every builtin class, in declaration order. `Mutator::builtin_class`
    /// indexes by discriminant, so the bootstrap builds them in this order,
    /// and the image's closing hash table writes them in it too. It follows
    /// their wire tags (`image::encode::builtin_class_tag`), less the
    /// retired ones, which is why a class added later is appended rather
    /// than slotted in beside its siblings. So this is *not* a
    /// superclass-first order: the kernel phase sorts the kinds by their
    /// depth below `Top` before it fills them
    /// (`interpreter::kernel::fill_order`).
    ///
    /// A new builtin must also appear in `interpreter::kernel`'s
    /// `kernel_class_name`; in `check_kernel_shape`'s slot whitelist if it
    /// declares slots; in `Mutator::signal_vm_error`'s subject-slot match if
    /// it is an error class carrying a subject; in the image's tag tables --
    /// `image::encode::builtin_class_tag`, `image::decode::builtin_class_from_tag`,
    /// `tests/opcode_tags.rs` and, for a class no program may name, the
    /// filter in `image::encode::encode_tag_named_hashes`, the refusals in
    /// `image::encode::named_builtin` and in
    /// `image::decode::referenced_builtin_class`; and in the class
    /// accessor lists, `OBJECT_CLASS_ACCESSORS` if `Object` declares an
    /// accessor for it and otherwise the absent-list of
    /// `objects_class_accessors_are_exactly_the_nameable_builtins`.
    pub const ALL: [BuiltinClass; 32] = [
        BuiltinClass::Object,
        BuiltinClass::Boolean,
        BuiltinClass::True,
        BuiltinClass::False,
        BuiltinClass::Integer,
        BuiltinClass::Float,
        BuiltinClass::String,
        BuiltinClass::Symbol,
        BuiltinClass::Array,
        BuiltinClass::UndefinedObject,
        BuiltinClass::Closure,
        BuiltinClass::VmMirror,
        BuiltinClass::Top,
        BuiltinClass::Value,
        BuiltinClass::Number,
        BuiltinClass::Behavior,
        BuiltinClass::Class,
        BuiltinClass::Metaclass,
        BuiltinClass::Activation,
        BuiltinClass::Exception,
        BuiltinClass::Error,
        BuiltinClass::MessageNotUnderstood,
        BuiltinClass::Message,
        BuiltinClass::Halt,
        BuiltinClass::FutureImplementation,
        BuiltinClass::LargeInteger,
        BuiltinClass::ArgumentError,
        BuiltinClass::CannotReturn,
        BuiltinClass::NonBooleanReceiver,
        BuiltinClass::CannotInstantiate,
        BuiltinClass::Proxy,
        BuiltinClass::Future,
    ];
}

// ---------- Resolver output ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// Slot `index` in the current activation's `temporaries`/`arguments`.
    Local(u16),
    /// A captured method/block variable, `depth` activations out.
    OuterLocal { depth: u16, index: u16 },
    /// An implicit-receiver, self, or `outer N` send resolved to lexical
    /// depth `k` (`docs/design/dispatch.md`, "Resolution"; `newspeak-spec.pdf`
    /// §5.7-5.9). Covers method calls, slot reads/writes, and nested-class
    /// references alike.
    LexicalSend { depth: u16 },
    /// `outer N` with no message after it: the kth enclosing object itself,
    /// the object `outer N m` sends `m` to (`newspeak-spec.pdf` §5.9). The
    /// specification defines `outer N` only as a send's receiver; canonical
    /// Newspeak also answers it as a value, and so does this runtime (decided
    /// with the user on 2026-09-27, `docs/design/frontend.md`, "Words that
    /// are not reserved").
    EnclosingObject { depth: u16 },
    /// A `super m` send (`newspeak-spec.pdf` §5.10) -- resolved at runtime
    /// against the current activation's `current_class`, not by depth.
    SuperSend,
    /// An explicit-receiver send whose receiver is the core library's
    /// `vmMirror` slot. Recorded only by `resolver::resolve_kernel`, so no
    /// user module ever carries it. Every consumer treats it as an ordinary
    /// send with no binding; the two recognizers that inline a mirror send
    /// (`control_flow::recognize_mirror_send`,
    /// `primitive_method::recognize`) read it, because only a shipped
    /// module's `vmMirror` is known to hold the mirror
    /// (`docs/design/core-library.md`, "The VM mirror is the whole Rust
    /// boundary").
    MirrorSend,
}

pub struct ResolvedModule {
    pub module: Rc<ast::Module>,
    pub source: Rc<str>,
    pub origin: SourceOrigin,
    /// Every `Identifier` needing resolution, and every `Send`/`Assign` node
    /// recognized as a self/outer/super/implicit send, keyed by its own
    /// `Span`. See `docs/design/dispatch.md`, "Shapes the resolver must
    /// recognize", for the exact shapes this covers.
    pub bindings: Rc<HashMap<Span, Binding>>,
    /// Whether this is `Kernel.ns`: true only for the module
    /// `resolver::resolve_kernel` answers, false for every user module. It
    /// is what confines fast paths (`mixin::MethodFastPath`) to the kernel.
    pub kernel: bool,
}

/// What the editor's language server needs from a resolution: every binding
/// the walk recorded *and* every error it found, together. `resolve` answers
/// one or the other, which is right for compiling -- a module with an error
/// must not run -- but an editor must keep colouring the rest of a file while
/// one name in it is wrong. Never compiled from: only `ResolvedModule` is.
pub struct EditorResolution {
    pub module: ast::Module,
    pub bindings: HashMap<Span, Binding>,
    pub errors: Vec<ResolveError>,
}

#[derive(Debug, Clone)]
pub enum SourceOrigin {
    File(PathBuf),
    /// A source compiled into the binary, named by the file it was read from
    /// at build time: `Manifest.ns`. An image names it so rather than by a
    /// path, which would claim a file that exists nowhere at run time.
    Embedded(String),
}

// ---------- Activation records ----------

#[derive(Debug, Clone)]
pub enum FrameKind<'gc> {
    Method(Rc<crate::mixin::MixinMethod>),
    /// An activation of the closure object named. Its body and the frames
    /// it captured are read through the object (`Heap::closure_activation`,
    /// `Heap::activation_captured`).
    Closure(ObjectReference<'gc>),
    /// A top-of-chain activation with no method or class of its own.
    /// Nothing that runs a program builds one (an instance initializer is
    /// `Initializer`); kept for the image format's frame records and a future
    /// REPL/top-level activation.
    TopLevel,
    /// An instance-initializer activation (Class Instantiation step 2)
    /// running `class`'s slot initializers and `initExprs` -- names the code
    /// it executes so a bytecode `program_counter` is interpretable through the
    /// initializer cache.
    Initializer(ClassReference<'gc>),
    /// A superclass clause's receiver expression (`newspeak-spec.pdf`
    /// §6.1's `ec`) running on the enclosing object, on behalf of the class
    /// that *declares* the clause.
    ///
    /// Distinct from `Initializer` even though the two look alike, because
    /// a frame kind is how `image::decode::frame_body` names the field an
    /// activation's compiled body lives in: this one's body comes from
    /// `Mixin::superclass_clause_code`, and verifying it against the
    /// class's instance initializer would reconcile a frame against a body
    /// that never ran in it.
    SuperclassClause(ClassReference<'gc>),
    /// The arguments of a superclass clause's deferred factory message,
    /// evaluated on the instance being initialized and with `class`'s own
    /// factory arguments (§6.2). Its body comes from
    /// `Mixin::superclass_message_code`; see `SuperclassClause` above
    /// for why that makes it a kind of its own.
    SuperclassMessage(ClassReference<'gc>),
    /// The arguments of a `<:` clause's factory message (§6.2), evaluated
    /// like `SuperclassMessage`'s, on the instance being initialized and with
    /// the applying class's factory arguments. It names the *level* the
    /// clause added, not the applying class: the applying class's mixin has
    /// one body per clause, and the level's applied clause
    /// (`Heap::class_applied_clause`) says
    /// which of them ran here -- a frame kind is how
    /// `image::decode::frame_body` finds an activation's body.
    MixinApplicationMessage(ClassReference<'gc>),
}

/// What the run loop does with an activation's value when it returns
/// (`docs/design/execution.md`, "The trampoline"). Every activation a send
/// makes hands its value to that send; the others are how a builtin native
/// finishes its work once the block it activated has returned, with no Rust
/// frame waiting for it.
#[derive(Debug, Clone)]
pub enum Continuation<'gc> {
    /// Hand the value to the send that made the activation.
    Deliver,
    /// A native `whileTrue:`/`whileFalse:` loop's condition returned.
    WhileCondition(Box<WhileLoop<'gc>>),
    /// Its body returned.
    WhileBody(Box<WhileLoop<'gc>>),
    /// A native `do:` loop's block returned for one element: activate it for
    /// the next, or answer the receiver
    /// (`builtins::collections::after_element`).
    DoElement(Box<DoLoop<'gc>>),
    /// The superclass clause the nested-class accessor evaluated returned the
    /// superclass: build, cache and answer the class
    /// (`interpreter::nested::after_clause`).
    NestedClass(Box<NestedClassCreation<'gc>>),
    /// A body an instantiation ran returned -- a clause message's arguments,
    /// or the initializer of a level with another below it still to run:
    /// carry the walk on (`Mutator::advance_instantiation`). The last
    /// initializer's activation delivers instead (`descend_levels` says why).
    Instantiate(Box<Instantiation<'gc>>),
    /// An `ensure:` or `ifCurtailed:` block the unwinder started returned:
    /// carry the unwind on (`interpreter::unwind`). Abandoned with its
    /// activation when a newer unwind or an error pops it first.
    Unwind(Box<PendingUnwind<'gc>>),
    /// The `signal` the run loop sent to the `Error` a VM error became
    /// (`Mutator::signal_vm_error`) returned: an `Error` is not resumable,
    /// so returning here means nothing ended the signal as it should have,
    /// and the run ends with the error as it would have before there were
    /// exceptions. Boxed so that every other continuation stays as small as
    /// it was.
    VmErrorSignalled(Box<NewspeakError<'gc>>),
    /// A future's computation returned (`interpreter::future`): record its
    /// value, then deliver it or run the instruction that forced it again.
    /// Boxed, as every continuation that carries state is.
    Future(Box<crate::interpreter::future::FutureForcing<'gc>>),
}

/// An unwind in progress (`interpreter::unwind`): where it is going and what
/// happens there. Boxed in a `Continuation::Unwind` while one of the blocks
/// it runs on its way is running, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct PendingUnwind<'gc> {
    /// The frame the unwind ends at, never the activation: code runs while
    /// the unwind is in progress, and an activation pushed meanwhile can
    /// reuse a popped activation's stack index, where a reified frame keeps
    /// its identity and says whether it still runs
    /// (`Heap::frame_activation`). `None` is the floor of the run.
    pub(crate) target: Option<FrameReference<'gc>>,
    pub(crate) action: UnwindAction<'gc>,
}

/// What an unwind does once it arrives.
#[derive(Debug, Clone)]
pub enum UnwindAction<'gc> {
    /// A non-local return, or `Exception>>return:` and `resume:`: the
    /// target returns this value.
    Return(Value<'gc>),
    /// `Exception>>retry`: the target, an `on:do:` activation, runs again
    /// from its first instruction, its registers kept.
    Restart,
    /// An unhandled exception's `defaultAction`: the unwind has no target,
    /// runs every pending `ensure:` and `ifCurtailed:` block down to the
    /// run's floor, and the run then leaves with this error.
    Terminate(NewspeakError<'gc>),
}

/// What a native loop runs for each step: a closure it activates itself,
/// or any other object, which it sends `value` or `value:` as a block
/// would be sent them. Decided once, when the loop starts: an object never
/// stops or starts being a closure.
#[derive(Debug, Clone, Copy)]
pub enum LoopAction<'gc> {
    Closure(ObjectReference<'gc>),
    Sent(Value<'gc>),
}

impl<'gc> LoopAction<'gc> {
    pub(crate) fn of(heap: &crate::Heap<'gc>, value: Value<'gc>) -> Self {
        match value.as_object() {
            Some(object) if heap.is_closure(object) => LoopAction::Closure(object),
            _ => LoopAction::Sent(value),
        }
    }
}

/// A native `whileTrue:`/`whileFalse:` loop in progress: its condition,
/// what it runs as the body -- a closure it activates, or any other object,
/// which it sends `value` -- and the condition value that keeps it going.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct WhileLoop<'gc> {
    /// The condition's closure object.
    pub(crate) condition: ObjectReference<'gc>,
    pub(crate) body: LoopAction<'gc>,
    pub(crate) expect: bool,
}

/// A native `do:` loop in progress: the array it walks and answers when it
/// is done, what it runs for each element, the size it read when it began
/// -- the walk ends there, as a loop from 1 to the size does -- and the
/// index of the next element, which is read from the array only when the
/// walk reaches it, so a store the action makes to a later index is seen.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct DoLoop<'gc> {
    pub(crate) receiver: ObjectReference<'gc>,
    pub(crate) action: LoopAction<'gc>,
    pub(crate) size: usize,
    pub(crate) next: usize,
}

/// A nested class being created for one enclosing object
/// (`interpreter::nested`): what the accessor needs once the declaration's
/// superclass clause has been evaluated.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct NestedClassCreation<'gc> {
    /// The enclosing object `r` of `newspeak-spec.pdf` §6, which caches the
    /// class.
    pub(crate) enclosing: ObjectReference<'gc>,
    /// The absolute index of the hidden slot of `enclosing` the class is
    /// cached in.
    pub(crate) index: usize,
    /// The declaration.
    pub(crate) nested: Rc<crate::mixin::Mixin>,
    /// The class object of the level that declares it.
    pub(crate) declaring_class: ClassReference<'gc>,
}

/// An instantiation in progress (`newspeak-spec.pdf` §6.2, and the six steps
/// of `docs/design/object-model.md`, "Instantiation"): the instance, every
/// level of the superclass chain the upward walk has reached with the factory
/// arguments it gets, and which way the walk is going.
///
/// Always boxed, in a box the interpreter reuses from one instantiation to
/// the next (`Mutator::walk_state`); while a body runs that has work left
/// after it, the box is in that body's continuation, owned by exactly one
/// stack entry.
#[derive(Debug, Clone)]
pub struct Instantiation<'gc> {
    pub(crate) instance: Value<'gc>,
    /// Nearest level first: the class instantiated, then its superclass, and
    /// so on up, as the upward walk reaches them.
    pub(crate) levels: Vec<(ClassReference<'gc>, Vec<Value<'gc>>)>,
    /// The index in `levels` of the level whose declaration the upward walk
    /// is climbing through: the last level reached that is no level a mixin
    /// application added. §6.2 writes every message on the way from a
    /// declaration up through its applied levels -- each `<:` clause's and
    /// the superclass clause's -- in that one declaration, so each is
    /// evaluated with its factory arguments, which `levels` still holds
    /// while the walk climbs. Written and read only by the climb through
    /// applied levels (`Mutator::climb_through_applied_levels`), which sets
    /// it when it takes over; the ordinary climb, where every level is its
    /// own, never needs it.
    pub(crate) applying: usize,
    /// The messages into applied levels the upward walk has passed and not
    /// evaluated, nearest level first, so the downward walk takes the next
    /// one off the end. §6.2 evaluates a `<:` clause's message after the
    /// superclass's initializer has run, just before its own mixin's
    /// (`Mutator::descend_levels`). Empty for any declaration without `<:`
    /// clauses, and for any whose `<:` messages take no arguments.
    pub(crate) deferred: Vec<DeferredMessage>,
    pub(crate) phase: InstantiationPhase<'gc>,
}

/// A message into an applied level whose evaluation waits for the downward
/// walk. It holds no handle: both levels are indices into
/// `Instantiation::levels`, which is traced.
#[derive(Debug, Clone)]
pub struct DeferredMessage {
    /// The index of the applied level the message's value is the factory
    /// arguments of.
    pub(crate) level: usize,
    /// The index of the declaration the message is written in, whose factory
    /// arguments it is evaluated with.
    pub(crate) applying: usize,
    pub(crate) code: Rc<crate::bytecode::CompiledCode>,
}

/// Which way an instantiation's walk is going.
#[derive(Debug, Clone, Copy)]
pub enum InstantiationPhase<'gc> {
    /// Upward. A body running for this instantiation is a clause message
    /// written in `level`'s declaration -- the message into the last level
    /// in `levels`'s superclass, `superclass` -- and its value is the factory
    /// arguments of `superclass`. `level` is the last level in `levels`
    /// unless that one is a level a mixin application added, whose messages
    /// are the applying declaration's (`Instantiation::applying`).
    Climbing {
        level: ClassReference<'gc>,
        superclass: ClassReference<'gc>,
    },
    /// Downward. A body running for this instantiation is the initializer of
    /// `levels[index]`, and a level below it has code still to run.
    Descending { index: usize },
    /// Downward. A body running for this instantiation is the message into
    /// the applied level `levels[index]`, written in `levels[applying]`'s
    /// declaration, and its value is `levels[index]`'s factory arguments.
    Applying { index: usize, applying: usize },
}

// ---------- Errors ----------

/// A name-resolution failure, reported before execution begins.
#[derive(Debug, Clone)]
pub struct ResolveError {
    pub span: Span,
    pub message: String,
}

/// A Newspeak-level error (an uncaught exception, `doesNotUnderstand:`, a
/// primitive failure such as a division by zero). Distinct from a Rust
/// `panic!`, which signals an `ns-runtime` bug, not a bad Newspeak program.
#[derive(Debug, Clone)]
pub struct NewspeakError<'gc> {
    pub selector: Symbol,
    pub receiver: Value<'gc>,
    /// What the class carries: `ArgumentError`'s `argument`,
    /// `CannotReturn`'s `result`, `NonBooleanReceiver`'s `receiver`,
    /// `CannotInstantiate`'s `behavior`; nil for an `Error`.
    pub subject: Value<'gc>,
    pub class: ErrorClass,
    pub message: Box<str>,
}

/// The kernel class `Mutator::signal_vm_error` signals for an error: an
/// `Error`, or one of the classes that carry what they were signalled
/// for in `NewspeakError::subject`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Error,
    ArgumentError,
    CannotReturn,
    NonBooleanReceiver,
    CannotInstantiate,
}

impl<'gc> NewspeakError<'gc> {
    /// An `Error`, which carries nothing.
    pub fn error(selector: Symbol, receiver: Value<'gc>, message: impl Into<Box<str>>) -> Self {
        NewspeakError {
            selector,
            receiver,
            subject: Value::nil(),
            class: ErrorClass::Error,
            message: message.into(),
        }
    }

    /// An `ArgumentError` carrying the `argument` that was refused.
    pub fn argument_error(
        selector: Symbol,
        receiver: Value<'gc>,
        argument: Value<'gc>,
        message: impl Into<Box<str>>,
    ) -> Self {
        NewspeakError {
            selector,
            receiver,
            subject: argument,
            class: ErrorClass::ArgumentError,
            message: message.into(),
        }
    }

    /// A `CannotReturn` carrying the `result` that could not be returned.
    pub fn cannot_return(
        selector: Symbol,
        receiver: Value<'gc>,
        result: Value<'gc>,
        message: impl Into<Box<str>>,
    ) -> Self {
        NewspeakError {
            selector,
            receiver,
            subject: result,
            class: ErrorClass::CannotReturn,
            message: message.into(),
        }
    }

    /// A `NonBooleanReceiver` carrying the condition's answer `value`, which
    /// is also the receiver of the send that found it no Boolean.
    pub fn non_boolean_receiver(
        selector: Symbol,
        value: Value<'gc>,
        message: impl Into<Box<str>>,
    ) -> Self {
        NewspeakError {
            selector,
            receiver: value,
            subject: value,
            class: ErrorClass::NonBooleanReceiver,
            message: message.into(),
        }
    }

    /// A `CannotInstantiate` carrying the `behavior` that refused, nil when
    /// it may not be named.
    pub fn cannot_instantiate(
        selector: Symbol,
        receiver: Value<'gc>,
        behavior: Value<'gc>,
        message: impl Into<Box<str>>,
    ) -> Self {
        NewspeakError {
            selector,
            receiver,
            subject: behavior,
            class: ErrorClass::CannotInstantiate,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root application with no code, no slots and no superclass,
    /// registered in `heap`. Built directly rather than through
    /// `Mutator::apply_mixin`, so that the tests below read only the objects
    /// they build themselves; they run in an interpreter's entry because
    /// only `World` builds a heap.
    fn empty_application(
        heap: &mut crate::Heap<'_>,
        name: &str,
    ) -> Rc<crate::mixin::MixinApplication> {
        heap.register_application(crate::mixin::MixinApplication {
            index: crate::heap::ApplicationIndex::UNREGISTERED,
            mixin: Rc::new(crate::mixin::Mixin::empty(Symbol::intern(name))),
            superclass: None,
            inherits_value: false,
            first_mutable_slot: None,
            instantiation_refusal: None,
            nested_slot: crate::hash::IdentityMap::default(),
            declared_slots: Vec::new(),
            slot_count: 0,
            own_slot_base: 0,
            dispatch_ids: crate::mixin::DispatchIds::NONE,
        })
    }

    /// A minimal, genuinely self-referential class object sufficient to
    /// construct other values in these tests. `Interpreter`'s real bootstrap
    /// wires every builtin class's `.class` up properly; this helper only
    /// needs the type shapes to compile and hold together, and no test in
    /// this file ever reads through the self-reference. The construction
    /// itself lives in `Heap::allocate_self_referential_class`, which is
    /// ordinary safe Rust in a space of offsets: the object is allocated
    /// before its class word is written with its own offset.
    fn dummy_class<'gc>(heap: &mut crate::Heap<'gc>) -> ClassReference<'gc> {
        let application = empty_application(heap, "Object");
        heap.allocate_self_referential_class(crate::heap::NewClass {
            application,
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin: Some(BuiltinClass::Object),
        })
    }

    #[test]
    fn class_data_is_reachable_through_object_data() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = dummy_class(&mut mutator.heap);
            assert!(mutator.heap.is_class(class));
        });
    }

    #[test]
    fn value_clone_shares_object_identity() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let class = dummy_class(&mut mutator.heap);
            let instance = mutator.heap.allocate_instance(class, 0);
            let original = Value::object(instance);
            let copy = original;
            match (original.as_object(), copy.as_object()) {
                (Some(left), Some(right)) => assert!(left.same(right)),
                _ => panic!("expected both copies to be objects"),
            }
        });
    }

    #[test]
    fn frame_kind_top_level_has_no_current_class() {
        let frame = crate::heap::FrameRecord {
            receiver: Value::nil(),
            arguments: vec![],
            temporaries: vec![],
            program_counter: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
        };
        assert!(frame.current_class.is_none());
    }

    #[test]
    fn binding_variants_are_copy_and_comparable() {
        let original = Binding::LexicalSend { depth: 2 };
        let copy = original;
        assert_eq!(original, copy);
        assert_ne!(original, Binding::SuperSend);
    }
}

#[cfg(test)]
mod value_tests {
    use super::{Value, ValueKind};

    #[test]
    fn each_constructor_answers_through_its_accessor_and_no_other() {
        let nil: Value<'static> = Value::nil();
        assert!(nil.is_nil());
        assert_eq!(nil.as_boolean(), None);
        let truth: Value<'static> = Value::boolean(true);
        assert_eq!(truth.as_boolean(), Some(true));
        assert!(!truth.is_nil());
        let three: Value<'static> = Value::small_integer(3);
        assert_eq!(three.as_small_integer(), Some(3));
        assert_eq!(three.as_boolean(), None);
        assert!(three.as_object().is_none());
        assert!(matches!(three.kind(), ValueKind::SmallInteger(3)));
        assert!(matches!(
            Value::<'static>::boolean(false).kind(),
            ValueKind::Boolean(false)
        ));
        assert!(matches!(Value::<'static>::nil().kind(), ValueKind::Nil));
    }
}
