//! Core runtime data types: the object model, activation records, and the
//! resolver's output shape. Pure data -- see
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
pub use crate::heap::{ClassReference, FrameReference, ObjectReference};

#[derive(Debug, Clone, Copy)]
pub enum Value<'gc> {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    Float(f64),
    Object(ObjectReference<'gc>),
}

#[derive(Debug, Clone)]
pub struct Object<'gc> {
    pub class: ClassReference<'gc>,
    /// 0 until something asks for it; then drawn from a monotonic counter
    /// (`Heap::ensure_identity_hash`), so allocation order stays
    /// unobservable. Never derived from the arena index, which a moving
    /// collector or `become:` would change (liveness spec's stable-identity
    /// constraint).
    pub identity_hash: u32,
    /// The id an inline cache guards this object's sends on
    /// (`docs/design/dispatch.md`, "The dispatch id"): an instance's class's
    /// instances id, a class object's own class-side id. A copy, so that a
    /// guard is one load and one compare rather than a walk to the class --
    /// which makes every place a class's application changes responsible for
    /// restamping it (`Heap::set_class_application`,
    /// `Heap::stamp_dispatch_ids`). Not a handle; a move carries it along.
    pub(crate) dispatch_id: crate::mixin::DispatchId,
    pub data: ObjectData<'gc>,
}

#[derive(Debug, Clone)]
pub enum ObjectData<'gc> {
    Instance(Vec<Value<'gc>>),
    String(String),
    Symbol(Symbol),
    Array(Vec<Value<'gc>>),
    Closure(ClosureData<'gc>),
    /// Boxed: a class object's data is what would otherwise set every
    /// object's size, and class objects are few (`docs/design/dispatch.md`,
    /// "The dispatch id").
    Class(Box<ClassData<'gc>>),
}

/// §3.1.1's enclosing-object condition on a value class -- every enclosing
/// object, at every level of its class chain, must be a value -- decided
/// once, where the class object is created
/// (`interpreter::value_class::check_value_class`, called from
/// `interpreter::nested::after_clause`), and recorded on `ClassData` rather
/// than raised there: `decide_instantiation` is what refuses, since the
/// specification's own §2.3 example creates such a class and never
/// instantiates it (`docs/design/object-model.md`, "A value object's four
/// conditions"). Holds no handle -- a `Symbol` and a `String`, both already
/// rendered at creation time -- so it survives a move. An image carries none
/// of it: `ClassData::new` defaults every decoded class's field to `None`,
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
/// instead, `described` and `level` carried over from that class's own
/// verdict, except that a `level` of `None` -- a fault in the stopping
/// class's own enclosing object -- becomes `Some(<the stopping class's
/// name>)`. A fault inherited over several levels thus keeps naming the
/// level whose enclosing object is at fault.
#[derive(Debug, Clone)]
pub enum ValueClassRefusal {
    /// `level` is `None` for the class's own enclosing object; `described`
    /// is `Heap::describe` of the offending object, taken at creation.
    NonValueEnclosingObject {
        level: Option<Symbol>,
        described: String,
    },
}

/// A class object is *identity*; everything a class object can *do* lives
/// on its `MixinApplication`. Two class objects built from the same mixin
/// and the same superclass application share one `application`, which is
/// what lets an inline cache stay monomorphic across enclosing instances
/// once nested classes give a mixin one class object per enclosing object.
#[derive(Debug, Clone)]
pub struct ClassData<'gc> {
    /// Behaviour: code, layout and the superclass chain. Shared with every
    /// other class object built from the same mixin and superclass.
    pub application: Rc<crate::mixin::MixinApplication>,
    /// `newspeak-spec.pdf` §6: the enclosing object is the nested class's
    /// only state, which is why a class object cannot be shared between
    /// instances of its enclosing class.
    pub enclosing: Option<ObjectReference<'gc>>,
    /// The class object of the level whose declaration *lexically contains*
    /// this class's declaration -- `Lookup::NestedClass`'s `ClassReference`,
    /// the level `interpreter::nested` evaluated the declaration at. `None` for
    /// a class that no declaration encloses: the module's root class, and a
    /// builtin. A metaclass copies the class it belongs to (§6.3 puts a class
    /// side in the same declaration as its instance side), so a class-side
    /// `outer` walks the same chain.
    ///
    /// This is what makes the enclosing-object chain of §3.3 *lexical* the
    /// whole way up rather than only at its first step. `enclosing` alone
    /// gives one hop; hopping again needs the class object of the level
    /// just reached, and that is this field, not the reached object's own
    /// class -- those differ exactly when the declaration is inherited
    /// across a nesting boundary. See `dispatch::kth_enclosing_object`.
    pub declaring_class: Option<ClassReference<'gc>>,
    /// The per-instance class-object chain, parallel to the application
    /// chain. `outer` and `super` need it, and lookup steps it beside the
    /// application chain for each level's class object -- the level a found
    /// method runs in.
    pub superclass: Option<ClassReference<'gc>>,
    /// `None` for a user-defined class. `Some(kind)` marks one of the
    /// core-library classes the bootstrap fills from `Kernel.ns` (or the VM
    /// mirror's class), for the image's class tags and for `decide_factory`,
    /// which instantiates `Array` and `OrderedCollection` with their own
    /// payload and refuses the mirror's class. Lookup does not read it: a
    /// builtin's methods are `Kernel.ns`'s, found in its mixin's method table
    /// like any other class's, and the only primitive entries are the VM
    /// mirror's.
    pub builtin: Option<BuiltinClass>,
    /// The metaclass built from this class's mixin's `class_side` together
    /// with its primary factory (§6.3), created on first use and cached here
    /// (`Mutator::metaclass_of`) so a repeated class-side send does not
    /// rebuild it and so metaclass identity stays stable (the inline caches
    /// key on it). Lives on `ClassData`, not as a slot on an *instance*: a
    /// class object's own payload is `ObjectData::Class`, never `Instance`,
    /// so it cannot share the nested-class hidden-slot mechanism
    /// (`nested_slot`), which stores its cache in an instance's field vector.
    /// `None` for every class object until its first class-side send -- which
    /// `X new` is, since the factory is one of the metaclass's members.
    pub metaclass: Option<ClassReference<'gc>>,
    /// A copy of `application.dispatch_ids.instances`. A cache's guard does
    /// not read it (it reads `Object::dispatch_id` in the header, through
    /// `Heap::dispatch_id`); this is where allocating an instance of this
    /// class, `Heap::class_dispatch_ids` and restamping
    /// (`Heap::header_dispatch_id`) read the instances id from. Kept equal to
    /// the application's by `ClassData::new` and
    /// `Heap::install_class_application`, the only ways either changes. The
    /// class-side id is not stored beside it: it is derived on demand
    /// (`DispatchId::class_side`) from the invariant that `mint_dispatch_ids`
    /// mints the pair consecutively.
    pub dispatch_id: crate::mixin::DispatchId,
    /// For a level a mixin application added (`newspeak-spec.pdf` §6.2), the
    /// `<:` clause that added it -- which of the applying declaration's
    /// clauses, and so the factory message that declaration wrote for it and
    /// that message's compiled arguments (`AppliedClause::clause`). `None` for
    /// every other class object.
    ///
    /// It cannot live on the level's mixin, where an ordinary level's
    /// superclass-clause message lives: the level's mixin is the *applied*
    /// class's, one mixin shared by every declaration that applies it, and
    /// each of them writes its own message. Nor on the level's application,
    /// which the application cache shares between two declarations applying
    /// the same mixin to the same superclass with different messages.
    ///
    /// Boxed, so that it adds one word to every class object rather than
    /// three: the instantiation walk and the collector read this struct for
    /// every class object, applied level or not.
    pub applied_clause: Option<Box<crate::mixin::AppliedClause>>,
    /// `ValueClassRefusal`'s own doc comment says where this is decided and
    /// where it is refused. `None` for every class that is no value and for
    /// a value class whose enclosing objects are all values -- which is
    /// most of them, so `decide_instantiation` pays this read only behind
    /// `application.inherits_value`. `ClassData::new` defaults it to `None`;
    /// `interpreter::nested::after_clause` sets it to `Some` right after
    /// creating the class object it is about, for a class built from
    /// source, and `image::decode::rebuild_applications` does the same for
    /// a decoded one, recomputed rather than carried over the wire.
    pub value_class_refusal: Option<ValueClassRefusal>,
}

impl<'gc> ClassData<'gc> {
    /// A class object's data around `application`, with no metaclass yet
    /// (`Mutator::metaclass_of` builds it on first use).
    pub fn new(
        application: Rc<crate::mixin::MixinApplication>,
        enclosing: Option<ObjectReference<'gc>>,
        declaring_class: Option<ClassReference<'gc>>,
        superclass: Option<ClassReference<'gc>>,
        builtin: Option<BuiltinClass>,
    ) -> ClassData<'gc> {
        ClassData {
            dispatch_id: application.dispatch_ids.instances,
            application,
            enclosing,
            declaring_class,
            superclass,
            builtin,
            metaclass: None,
            applied_clause: None,
            value_class_refusal: None,
        }
    }
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
    Character,
    Array,
    OrderedCollection,
    Nil,
    Block,
    Platform,
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
}

impl BuiltinClass {
    /// Every builtin class, in declaration order. `Mutator::builtin_class`
    /// indexes by discriminant, so the bootstrap builds them in this order,
    /// and the image's closing hash table writes them in it too -- it is also
    /// their wire-tag order (`image::encode::builtin_class_tag`), which is
    /// why a class added later is appended rather than slotted in beside its
    /// siblings. So this is *not* a superclass-first order: the kernel phase
    /// sorts the kinds by their depth below `Top` before it fills them
    /// (`interpreter::kernel::fill_order`).
    pub const ALL: [BuiltinClass; 18] = [
        BuiltinClass::Object,
        BuiltinClass::Boolean,
        BuiltinClass::True,
        BuiltinClass::False,
        BuiltinClass::Integer,
        BuiltinClass::Float,
        BuiltinClass::String,
        BuiltinClass::Symbol,
        BuiltinClass::Character,
        BuiltinClass::Array,
        BuiltinClass::OrderedCollection,
        BuiltinClass::Nil,
        BuiltinClass::Block,
        BuiltinClass::Platform,
        BuiltinClass::VmMirror,
        BuiltinClass::Top,
        BuiltinClass::Value,
        BuiltinClass::Number,
    ];
}

/// A block closure. Its compiled body is an `Rc` it shares with the
/// `MakeClosure` instruction that made it -- the same for every closure one
/// instruction makes -- and the frame it captured is its own. Stored inline
/// in the closure object, never behind an `Rc`
/// of its own: a handle lives only where the tracer can rewrite it in place
/// (`docs/design/memory.md`, "What the collector traces"), so everything
/// else that needs a closure -- an activation's `FrameKind`, a native
/// loop's state, a decided `Step` -- names the closure object instead.
///
/// 32 bytes in a release build, which is what keeps an `Object` at 56 --
/// this is `ObjectData`'s largest payload, and `heap.rs`'s
/// `an_object_is_the_size_the_amplification_figure_assumes` pins the
/// figure `CLAUDE.md`'s amplification paragraph is computed from.
#[derive(Debug, Clone)]
pub struct ClosureData<'gc> {
    pub code: Rc<crate::bytecode::CompiledCode>,
    /// The activation the closure was created in, reified. A closure keeps
    /// it whatever it captured: `enclosingActivation` and the debugger's
    /// "[] in Foo bar" are read off it (`docs/design/execution.md`, "The
    /// closure keeps its defining activation").
    pub captured_frame: FrameReference<'gc>,
    /// What the closure was handed when it was created: a copied variable's
    /// value, or an enclosing body's indirection vector. Its registers are
    /// `code.argument_count + position`. `Box<[_]>`, not a `Vec`, because
    /// `ClosureData` is `ObjectData`'s largest payload and an `Object` is
    /// pinned at 56 bytes in a release build (`heap.rs`).
    pub copied: Box<[Value<'gc>]>,
    /// The start of its family's range in the heap's cache table, copied from
    /// the activation that created it, since a block belongs to its method's
    /// family. Never serialized: a decoded closure carries `NOT_ASSIGNED`
    /// and is given a fresh range when it is first activated.
    pub feedback_base: crate::interpreter::FeedbackBase,
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
    /// A `super m` send (`newspeak-spec.pdf` §5.10) -- resolved at runtime
    /// against the current activation's `current_class`, not by depth.
    SuperSend,
    /// An explicit-receiver send whose receiver is the core library's
    /// `vmMirror` slot. Recorded only by `resolver::resolve_kernel`, so no
    /// user module ever carries it. Every consumer treats it as an ordinary
    /// send with no binding; the two recognizers that inline a mirror send
    /// (`control_flow::recognize_mirror_send`,
    /// `primitive_method::recognize`) read it, because only the kernel's
    /// `vmMirror` is known to hold the mirror (`docs/design/core-library.md`,
    /// "The VM mirror is the whole Rust boundary").
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

#[derive(Debug, Clone)]
pub enum SourceOrigin {
    File(PathBuf),
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
    /// `Initializer`); kept for the image format's frame table and a future
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
    /// one body per clause, and the level's `ClassData::applied_clause` says
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
}

/// A native `whileTrue:`/`whileFalse:` loop in progress: its condition, its
/// body -- any value, since a body that is not a block evaluates to itself --
/// and the condition value that keeps it going.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct WhileLoop<'gc> {
    /// The condition's closure object.
    pub(crate) condition: ObjectReference<'gc>,
    pub(crate) body: Value<'gc>,
    pub(crate) expect: bool,
}

/// A native `do:` loop in progress: the collection it answers when it is done,
/// its block's closure object, the elements the collection had when the loop
/// began -- `do:` walks that snapshot, so a block that changes the collection
/// does not change the walk -- and the index of the next element to hand the
/// block.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct DoLoop<'gc> {
    pub(crate) receiver: Value<'gc>,
    pub(crate) block: ObjectReference<'gc>,
    pub(crate) items: Vec<Value<'gc>>,
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

/// A *reified* activation: one a block captured, or one the image decoder
/// rebuilt. A running activation lives on the `Heap`'s activation stack and
/// becomes a `Frame` only when `Heap::reify` is asked for one; while it runs,
/// the frame is married to its stack entry and its registers live there.
/// See `docs/design/execution.md`, "Activations".
#[derive(Debug, Clone)]
pub struct Frame<'gc> {
    pub receiver: Value<'gc>,
    pub arguments: Vec<Value<'gc>>,
    /// Indexable by resolver slot index, growable (liveness spec's
    /// reified-activation constraint).
    pub temporaries: Vec<Value<'gc>>,
    /// How far this activation has progressed: an instruction index into
    /// the `CompiledCode` it runs.
    pub program_counter: usize,
    /// `None` for every frame the runtime reifies: a live activation's
    /// sender is the stack entry below it, and a returned activation has
    /// none. Kept for the image format, which records one per frame.
    pub sender: Option<FrameReference<'gc>>,
    /// Set only for `FrameKind::Closure` -- the enclosing method or
    /// initializer activation.
    pub home: Option<FrameReference<'gc>>,
    /// `None` only for `FrameKind::TopLevel` (`newspeak-spec.pdf` §3.6: a
    /// top-level activation's current class is `nil`).
    pub current_class: Option<ClassReference<'gc>>,
    pub kind: FrameKind<'gc>,
    /// Set once this frame's activation has returned (`Heap::pop_activation`
    /// divorces it). A `^` whose home is such a frame is a `cannotReturn`
    /// error (`newspeak-spec.pdf` §3.6, §7.2). `raise_return` decides by
    /// `Heap::frame_activation`, which also covers a decoded frame, dead by
    /// construction whatever this flag says; the flag itself is what an image
    /// records.
    pub uncontinuable: bool,
    /// The absolute field index a compiled `InitializeSlot`'s mixin-relative
    /// `own_index` is offset by -- `MixinApplication.own_slot_base` for the
    /// class level the activation initializes. The value `InitializeSlot` reads
    /// while the body runs is the live activation's own `slot_base`
    /// (`Heap::activation_slot_base`), one activation per initializing level
    /// of the superclass chain, pushed with the slot base
    /// `Mutator::initializer_body` finds; a
    /// frame carries a copy only if that activation was reified. The image
    /// does not record it -- a decoded frame has 0, since nothing re-activates
    /// one. Set once, where the activation is pushed, and never changed.
    /// Irrelevant (left at 0) for every other kind: a method, block, clause,
    /// or top-level body never contains `InitializeSlot`.
    pub slot_base: usize,
}

// ---------- Errors ----------

/// A name-resolution failure, reported before execution begins.
#[derive(Debug, Clone)]
pub struct ResolveError {
    pub span: Span,
    pub message: String,
}

/// A Newspeak-level error (an uncaught exception, `doesNotUnderstand:`, a
/// primitive failure such as integer overflow). Distinct from a Rust
/// `panic!`, which signals an `ns-runtime` bug, not a bad Newspeak program.
#[derive(Debug, Clone)]
pub struct NewspeakError<'gc> {
    pub selector: Symbol,
    pub receiver: Value<'gc>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root application with no code, no slots and no superclass. Built
    /// directly rather than through `Mutator::apply_mixin`, so that the
    /// tests below read only the objects they build themselves; they run in
    /// an interpreter's entry because only `World` builds a heap.
    fn empty_application(name: &str) -> Rc<crate::mixin::MixinApplication> {
        Rc::new(crate::mixin::MixinApplication {
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
    /// ordinary safe Rust under the arena: the slot's index is known before
    /// the slot is filled.
    fn dummy_class<'gc>(heap: &mut crate::Heap<'gc>) -> ClassReference<'gc> {
        heap.allocate_self_referential_class(ObjectData::Class(Box::new(ClassData::new(
            empty_application("Object"),
            None,
            None,
            None,
            Some(BuiltinClass::Object),
        ))))
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
            let instance = mutator
                .heap
                .allocate_object(class, ObjectData::Instance(vec![]));
            let original = Value::Object(instance);
            let copy = original;
            match (original, copy) {
                (Value::Object(left), Value::Object(right)) => assert!(left.same(right)),
                _ => panic!("expected Value::Object"),
            }
        });
    }

    #[test]
    fn frame_kind_top_level_has_no_current_class() {
        let frame = Frame {
            receiver: Value::Nil,
            arguments: vec![],
            temporaries: vec![],
            program_counter: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: FrameKind::TopLevel,
            uncontinuable: false,
            slot_base: 0,
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
