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
    /// "3c", C2+).
    Class(Box<ClassData<'gc>>),
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
    /// method runs in, and the one a failed primitive passes over.
    pub superclass: Option<ClassReference<'gc>>,
    /// `None` for a user-defined class. `Some(kind)` marks one of the
    /// core-library classes `builtins/` implements natively, for the image's
    /// class tags and for `decide_factory`, which instantiates `Array` and
    /// `OrderedCollection` with their own payload. Lookup does not read it:
    /// a builtin's primitives are entries of its mixin's method table.
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
    /// A copy of `application.dispatch_ids.instances`. A cache's guard no
    /// longer reads it here (`Object::dispatch_id` in the header is what
    /// `Heap::dispatch_id` answers now) -- this is where allocating an
    /// instance of this class, `Heap::class_dispatch_ids` and restamping
    /// (`Heap::header_dispatch_id`) read the instances id from. Kept equal to
    /// the application's by `ClassData::new` and
    /// `Heap::install_class_application`, the only ways either changes. The
    /// class-side id is not stored beside it: it is derived on demand
    /// (`DispatchId::class_side`) from the invariant that `mint_dispatch_ids`
    /// mints the pair consecutively.
    pub dispatch_id: crate::mixin::DispatchId,
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
}

impl BuiltinClass {
    /// Every builtin class, in declaration order. `Mutator::builtin_class`
    /// indexes by discriminant, so the bootstrap builds them in this order,
    /// and the image's closing hash table writes them in it too -- it is also
    /// their wire-tag order (`image::encode::builtin_class_tag`).
    pub const ALL: [BuiltinClass; 14] = [
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
    ];
}

/// A block closure. Its parameter names and compiled body are `Rc`s it
/// shares with the `MakeClosure` instruction that made it -- the same for
/// every closure one instruction makes -- and the two frames it captured
/// are its own. Stored inline in the closure object, never behind an `Rc`
/// of its own: a handle lives only where the tracer can rewrite it in place
/// (`docs/design/memory.md`, "What the collector traces"), so everything
/// else that needs a closure -- an activation's `FrameKind`, a native
/// loop's state, a decided `Step` -- names the closure object instead.
#[derive(Debug, Clone)]
pub struct ClosureData<'gc> {
    pub parameters: Rc<Vec<Symbol>>,
    pub code: Rc<crate::bytecode::CompiledCode>,
    /// The activation the closure was created in, reified.
    pub captured_frame: FrameReference<'gc>,
    /// The lexically enclosing *method* (or initializer) activation, which a
    /// `^` in the block returns from.
    pub home: FrameReference<'gc>,
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
    /// `and:` and `or:`: hand on whether the value is exactly `true`
    /// (`builtins::boolean::truthy`).
    Truthy,
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
    pub(crate) phase: InstantiationPhase<'gc>,
}

/// Which way an instantiation's walk is going.
#[derive(Debug, Clone, Copy)]
pub enum InstantiationPhase<'gc> {
    /// Upward. A body running for this instantiation is the clause message
    /// of `level`, the last in `levels`, and its value is the factory
    /// arguments of `superclass`, `level`'s superclass.
    Climbing {
        level: ClassReference<'gc>,
        superclass: ClassReference<'gc>,
    },
    /// Downward. A body running for this instantiation is the initializer of
    /// `levels[index]`, and a level below it has code still to run.
    Descending { index: usize },
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
            nested_slot: crate::hash::IdentityMap::default(),
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
