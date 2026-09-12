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
pub use crate::heap::{ClassRef, FrameRef, ObjRef};

#[derive(Debug, Clone, Copy)]
pub enum Value {
    Nil,
    Bool(bool),
    SmallInt(i64),
    Float(f64),
    Obj(ObjRef),
}

#[derive(Debug, Clone)]
pub struct Object {
    pub class: ClassRef,
    /// Monotonic, assigned at allocation -- never derived from the arena
    /// index (liveness spec's stable-identity constraint).
    pub identity_hash: u32,
    pub data: ObjectData,
}

#[derive(Debug, Clone)]
pub enum ObjectData {
    Instance(Vec<Value>),
    Str(String),
    Sym(Symbol),
    Array(Vec<Value>),
    Closure(Rc<ClosureData>),
    Class(ClassData),
}

/// A class object is *identity*; everything a class object can *do* lives
/// on its `MixinApplication`. Two class objects built from the same mixin
/// and the same superclass application share one `application`, which is
/// what lets an inline cache stay monomorphic across enclosing instances
/// once nested classes give a mixin one class object per enclosing object.
#[derive(Debug, Clone)]
pub struct ClassData {
    /// Behaviour: code, layout and the superclass chain. Shared with every
    /// other class object built from the same mixin and superclass.
    pub application: Rc<crate::mixin::MixinApplication>,
    /// `newspeak-spec.pdf` §6: the enclosing object is the nested class's
    /// only state, which is why a class object cannot be shared between
    /// instances of its enclosing class.
    pub enclosing: Option<ObjRef>,
    /// The class object of the level whose declaration *lexically contains*
    /// this class's declaration -- `Lookup::NestedClass`'s `ClassRef`, the
    /// level `interpreter::nested` evaluated the declaration at. `None` for
    /// a class that no declaration encloses: the module's root class, and a
    /// builtin. A metaclass copies the class it belongs to (§6.3.1 puts a
    /// class side in the same declaration as its instance side), so a
    /// class-side `outer` walks the same chain.
    ///
    /// This is what makes the enclosing-object chain of §3.3 *lexical* the
    /// whole way up rather than only at its first step. `enclosing` alone
    /// gives one hop; hopping again needs the class object of the level
    /// just reached, and that is this field, not the reached object's own
    /// class -- those differ exactly when the declaration is inherited
    /// across a nesting boundary. See `dispatch::kth_enclosing_object`.
    pub declaring_class: Option<ClassRef>,
    /// The per-instance class-object chain, parallel to the application
    /// chain. `outer` and `super` need it; lookup does not.
    pub superclass: Option<ClassRef>,
    /// `None` for a user-defined class. `Some(kind)` marks one of the
    /// core-library classes `builtins/` implements natively -- the same
    /// `BuiltinClass` `dispatch::Lookup::Native` carries once lookup
    /// resolves a send to one.
    pub builtin: Option<BuiltinClass>,
    /// The metaclass built from this class's mixin's `class_side`, created
    /// on first use and cached here (`Interpreter::metaclass_of`) so a
    /// repeated class-side send does not rebuild it and so metaclass
    /// identity stays stable (the inline caches will key on it).
    /// Lives on `ClassData`, not as a slot on an *instance*: a class
    /// object's own payload is `ObjectData::Class`, never `Instance`, so it
    /// cannot share the nested-class hidden-slot mechanism (`nested_slot`),
    /// which stores its cache in an instance's field vector. `None` for
    /// every class object until its first class-side send, permanently for
    /// one whose mixin declares no class side at all -- §6.3's "no
    /// metaclass" case costs nothing.
    pub metaclass: Option<ClassRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinClass {
    Object,
    Boolean,
    True,
    False,
    Integer,
    Float,
    Str,
    Sym,
    Character,
    Array,
    OrderedCollection,
    Nil,
    Block,
    Platform,
}

#[derive(Debug, Clone)]
pub struct ClosureData {
    /// Shared with the `MakeClosure` instruction that created the closure --
    /// the names are the same for every closure one instruction makes.
    pub params: Rc<Vec<Symbol>>,
    pub code: Rc<crate::bytecode::CompiledCode>,
    pub captured_frame: FrameRef,
    /// The lexically enclosing *method* (or initializer) activation -- see
    /// the design note above on why this is a `FrameRef`, not an `ObjRef`.
    pub home: FrameRef,
}

// ---------- Resolver output ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// Slot `index` in the current activation's `temps`/`args`.
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
    /// Every `Ident` needing resolution, and every `Send`/`Assign` node
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
pub enum FrameKind {
    Method(Rc<crate::mixin::MixinMethod>),
    Closure(Rc<ClosureData>),
    /// A top-of-chain activation with no method or class of its own.
    /// Nothing that runs a program builds one (an instance initializer is
    /// `Initializer`); kept for the image format's frame table and a future
    /// REPL/top-level activation.
    TopLevel,
    /// An instance-initializer activation (Class Instantiation step 2)
    /// running `class`'s slot initializers and `initExprs` -- names the code
    /// it executes so a bytecode `pc` is interpretable through the
    /// initializer cache.
    Initializer(ClassRef),
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
    SuperclassClause(ClassRef),
    /// The arguments of a superclass clause's deferred factory message,
    /// evaluated on the instance being initialized and with `class`'s own
    /// factory arguments (§6.2). Its body comes from
    /// `Mixin::superclass_message_code`; see `SuperclassClause` above
    /// for why that makes it a kind of its own.
    SuperclassMessage(ClassRef),
}

/// A *reified* activation: one a block captured, or one the image decoder
/// rebuilt. A running activation lives on the `Heap`'s activation stack and
/// becomes a `Frame` only when `Heap::reify` is asked for one; while it runs,
/// the frame is married to its stack entry and its registers live there.
/// See `docs/design/execution.md`, "Activations".
#[derive(Debug, Clone)]
pub struct Frame {
    pub receiver: Value,
    pub args: Vec<Value>,
    /// Indexable by resolver slot index, growable (liveness spec's
    /// reified-activation constraint).
    pub temps: Vec<Value>,
    /// How far this activation has progressed: an instruction index into
    /// the `CompiledCode` it runs.
    pub pc: usize,
    /// `None` for every frame the runtime reifies: a live activation's
    /// sender is the stack entry below it, and a returned activation has
    /// none. Kept for the image format, which records one per frame.
    pub sender: Option<FrameRef>,
    /// Set only for `FrameKind::Closure` -- the enclosing method or
    /// initializer activation.
    pub home: Option<FrameRef>,
    /// `None` only for `FrameKind::TopLevel` (`newspeak-spec.pdf` §3.6: a
    /// top-level activation's current class is `nil`).
    pub current_class: Option<ClassRef>,
    pub kind: FrameKind,
    /// Set once this frame's activation has returned (`Heap::pop_activation`
    /// divorces it). A `^` whose home is such a frame is a `cannotReturn`
    /// error (`newspeak-spec.pdf` §3.6, §7.2). `raise_return` decides by
    /// `Heap::frame_activation`, which also covers a decoded frame, dead by
    /// construction whatever this flag says; the flag itself is what an image
    /// records.
    pub uncontinuable: bool,
    /// The absolute field index a compiled `InitSlot`'s mixin-relative
    /// `own_index` is offset by -- `MixinApplication.own_slot_base` for the
    /// class level the activation initializes. The value `InitSlot` reads
    /// while the body runs is the live activation's own `slot_base`
    /// (`Heap::activation_slot_base`), one activation per initializing level
    /// of the superclass chain (`Interpreter::run_initializer_level`); a
    /// frame carries a copy only if that activation was reified. The image
    /// does not record it -- a decoded frame has 0, since nothing re-activates
    /// one. Set once, where the activation is pushed, and never changed.
    /// Irrelevant (left at 0) for every other kind: a method, block, clause,
    /// or top-level body never contains `InitSlot`.
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
pub struct NewspeakError {
    pub selector: Symbol,
    pub receiver: Value,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root application with no code, no slots and no superclass. Built
    /// directly rather than through `Interpreter::apply_mixin` so this
    /// module's tests stay free of the interpreter.
    fn empty_application(name: &str) -> Rc<crate::mixin::MixinApplication> {
        Rc::new(crate::mixin::MixinApplication {
            mixin: Rc::new(crate::mixin::Mixin::empty(Symbol::intern(name))),
            superclass: None,
            slot_index: HashMap::new(),
            nested_slot: HashMap::new(),
            num_slots: 0,
            own_slot_base: 0,
        })
    }

    /// A minimal, genuinely self-referential class object sufficient to
    /// construct other values in these tests. `Interpreter`'s real bootstrap
    /// wires every builtin class's `.class` up properly; this helper only
    /// needs the type shapes to compile and hold together, and no test in
    /// this file ever reads through the self-reference. The construction
    /// itself lives in `Heap::alloc_self_referential_class`, which is
    /// ordinary safe Rust under the arena: the slot's index is known before
    /// the slot is filled.
    fn dummy_class(heap: &mut crate::Heap) -> ClassRef {
        heap.alloc_self_referential_class(ObjectData::Class(ClassData {
            application: empty_application("Object"),
            enclosing: None,
            declaring_class: None,
            superclass: None,
            builtin: Some(BuiltinClass::Object),
            metaclass: None,
        }))
    }

    #[test]
    fn class_data_is_reachable_through_object_data() {
        let mut heap = crate::Heap::new();
        let class = dummy_class(&mut heap);
        assert!(heap.is_class(class));
    }

    #[test]
    fn value_clone_shares_object_identity() {
        let mut heap = crate::Heap::new();
        let class = dummy_class(&mut heap);
        let instance = heap.alloc_object(class, ObjectData::Instance(vec![]));
        let a = Value::Obj(instance);
        let b = a;
        match (a, b) {
            (Value::Obj(x), Value::Obj(y)) => assert!(x.same(y)),
            _ => panic!("expected Value::Obj"),
        }
    }

    #[test]
    fn frame_kind_top_level_has_no_current_class() {
        let frame = Frame {
            receiver: Value::Nil,
            args: vec![],
            temps: vec![],
            pc: 0,
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
        let a = Binding::LexicalSend { depth: 2 };
        let b = a;
        assert_eq!(a, b);
        assert_ne!(a, Binding::SuperSend);
    }
}
