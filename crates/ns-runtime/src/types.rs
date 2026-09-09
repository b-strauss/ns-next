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

/// An opaque handle to an object: an index into `Heap.objects`. The inner
/// index is private to the crate, and only `types.rs` and `heap.rs` reach
/// it -- that privacy is what made replacing `Rc<RefCell<Object>>` with
/// this a change to those two files rather than to every caller. See
/// `docs/design/memory.md`, "The seam".
///
/// A handle is meaningful only against the `Heap` that allocated it. The
/// arena never reuses a slot -- there is no collector yet -- so two live
/// objects never share an index.
#[derive(Clone)]
pub struct ObjRef {
    pub(crate) index: u32,
    /// The heap that allocated this handle. Debug builds only: it exists to
    /// turn reading a handle through the wrong heap -- which answers a
    /// different, valid object rather than failing -- into a panic at the
    /// read. See `Heap::check_object_origin`.
    #[cfg(debug_assertions)]
    pub(crate) origin: u32,
}

impl ObjRef {
    /// Object identity. Replaces `Rc::ptr_eq` at every call site, so that
    /// identity has one spelling that survives the store changing under it.
    pub fn same(&self, other: &ObjRef) -> bool {
        self.index == other.index
    }

    /// An opaque, hashable key unique to this handle, for code that needs
    /// `ObjRef` as a `HashMap`/`HashSet` key rather than a comparison --
    /// `identity_hash` cannot serve that role, because two placeholder
    /// objects may legitimately share hash 0
    /// (`Heap::alloc_placeholder_object`,
    /// `Heap::alloc_self_referential_class`), which would fold two distinct
    /// objects into one table entry. Crate-internal: every current caller
    /// (`image::encode::GraphTable`'s object-id map,
    /// `image::decode::rebuild_applications`'s superclass-position map,
    /// `interpreter::nested::in_progress_key`) lives inside `ns-runtime`.
    pub(crate) fn identity_key(&self) -> usize {
        self.index as usize
    }
}

/// Prints the arena index and nothing else. Under `Rc` this impl was
/// hand-written to stop `Debug` recursing through the bootstrap `Object`
/// class, which is its own `.class`; an index has nothing to descend into,
/// so the recursion hazard is gone by construction. It still renders Rust
/// internals, which is why `CLAUDE.md`'s "Never `{:?}` a `Value`" rule
/// stands: `Heap::describe` is what a Newspeak-facing message needs.
impl std::fmt::Debug for ObjRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjRef(#{})", self.index)
    }
}

/// A class is an `ObjRef` whose `data` is `ObjectData::Class(_)`. Not
/// enforced by the type system in Milestone 1 (no metaclasses yet).
pub type ClassRef = ObjRef;

#[derive(Debug, Clone)]
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
    Closure(ClosureData),
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
    /// identity stays stable (a later milestone's inline caches key on it).
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

/// A closure's body: the AST the tree-walker created it from, or the
/// compiled code the bytecode VM created it from. `eval::invoke_closure`
/// dispatches on this, so a closure always runs with the engine that
/// created it -- the invariant "under `--engine=bytecode` no Newspeak-level
/// code is tree-walked" is enforced by the type, not by a runtime flag.
#[derive(Debug, Clone)]
pub enum ClosureBody {
    Ast(Rc<ast::CodeBody>),
    Compiled(Rc<crate::bytecode::CompiledCode>),
}

#[derive(Debug, Clone)]
pub struct ClosureData {
    pub params: Vec<Symbol>,
    pub body: ClosureBody,
    pub captured_frame: FrameRef,
    /// The lexically enclosing *method* (or initializer) activation -- see
    /// the design note above on why this is a `FrameRef`, not an `ObjRef`.
    pub home: FrameRef,
    /// The resolver's bindings map the block literal's body was originally
    /// resolved under. A `ClosureBody::Ast` needs exactly this map: every
    /// `LexicalSend`, `SuperSend` and local-variable reference in the body
    /// is resolved by span through it, so invoking such a closure with a
    /// different map would break those references. Cloning an `Rc` here is
    /// cheap.
    ///
    /// A `ClosureBody::Compiled` reads it never -- lowering already turned
    /// every binding into an instruction -- which is why a closure decoded
    /// from an image can carry the load's empty map
    /// (`image::LoadedImage::empty_bindings`) without losing anything.
    pub bindings: Rc<HashMap<Span, Binding>>,
}

// ---------- Resolver output ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// Slot `index` in the current activation's `temps`/`args`.
    Local(u16),
    /// A captured method/block variable, `depth` activations out.
    OuterLocal { depth: u16, index: u16 },
    /// An implicit-receiver, self, or `outer N` send resolved to lexical
    /// depth `k` (runtime spec's Resolver section; `newspeak-spec.pdf`
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
    /// `Span`. See the runtime spec's "AST recognition" note for the exact
    /// shapes this covers.
    pub bindings: Rc<HashMap<Span, Binding>>,
}

#[derive(Debug, Clone)]
pub enum SourceOrigin {
    File(PathBuf),
}

// ---------- Activation records ----------

/// An opaque handle to an activation: an index into `Heap.frames`. The
/// inner index is private to the crate, and only `types.rs` and `heap.rs`
/// reach it -- that privacy is what made replacing `Rc<RefCell<Frame>>`
/// with this a change to those two files rather than to every caller.
/// See `docs/design/memory.md`, "The seam".
///
/// A handle is meaningful only against the `Heap` that allocated it. The
/// arena never reuses a slot -- there is no collector yet -- so two live
/// activations never share an index, which is what `Signal::Return`'s
/// target check depends on.
#[derive(Clone)]
pub struct FrameRef {
    pub(crate) index: u32,
    /// The heap that allocated this handle. Debug builds only: it exists to
    /// turn reading a handle through the wrong heap -- which answers a
    /// different, valid object rather than failing -- into a panic at the
    /// read. See `Heap::check_frame_origin`.
    #[cfg(debug_assertions)]
    pub(crate) origin: u32,
}

impl FrameRef {
    /// Activation identity. Replaces `Rc::ptr_eq` at every call site, so
    /// that identity has one spelling that survives the store changing
    /// under it -- `invoke_method`'s `Signal::Return` catch and
    /// `raise_return`'s target check both depend on this rather than on
    /// the representation directly.
    pub fn same(&self, other: &FrameRef) -> bool {
        self.index == other.index
    }

    /// An opaque, hashable key unique to this handle, for code that needs
    /// `FrameRef` as a `HashMap` key rather than a comparison --
    /// `image::encode::GraphTable`'s frame-id map. Crate-internal: its
    /// only current caller lives inside `ns-runtime`.
    pub(crate) fn identity_key(&self) -> usize {
        self.index as usize
    }
}

/// Prints the arena index and nothing else. Under `Rc` this impl was
/// hand-written to stop `Debug` walking the sender chain (a closure's
/// `home` can itself be captured, so that walk had no acyclic guarantee
/// to lean on); an index has nothing to descend into at all, so the
/// hazard is gone by construction rather than by discipline. It also can
/// no longer panic, which is why the `try_borrow` dance is gone.
impl std::fmt::Debug for FrameRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FrameRef(#{})", self.index)
    }
}

#[derive(Debug, Clone)]
pub enum FrameKind {
    Method(Rc<crate::mixin::MixinMethod>),
    Closure(Rc<ClosureData>),
    /// A top-of-chain activation with no method or class of its own. Not
    /// built by anything in Milestone 2 (an instance initializer is
    /// `Initializer`); kept for the image format's frame table and a later
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
    /// a frame kind is how `image::decode::frame_body` names the table an
    /// activation's compiled body lives in: this one's body comes from
    /// `CompiledModule::superclass_clauses`, and verifying it against the
    /// class's instance initializer would reconcile a frame against a body
    /// that never ran in it.
    SuperclassClause(ClassRef),
    /// The arguments of a superclass clause's deferred factory message,
    /// evaluated on the instance being initialized and with `class`'s own
    /// factory arguments (§6.2). Its body comes from
    /// `CompiledModule::superclass_messages`; see `SuperclassClause` above
    /// for why that makes it a kind of its own.
    SuperclassMessage(ClassRef),
}

/// What a `Frame`'s `pc` counts. The two engines number an activation's
/// progress in different units -- the tree-walker in statements of a
/// `CodeBody`, the VM in instructions of a `CompiledCode` -- and a frame is
/// serialized, so a reader that had to ask the *interpreter* which engine
/// built it could not interpret a frame record at all. The discriminator
/// travels with the frame instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcKind {
    /// A statement index into the enclosing `ast::CodeBody.stmts`
    /// (`Interpreter::run_statements`).
    TreeWalk,
    /// An instruction index into the enclosing `CompiledCode.instrs`
    /// (`vm::run_code`).
    Bytecode,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub receiver: Value,
    pub args: Vec<Value>,
    /// Indexable by resolver slot index, growable (liveness spec's
    /// reified-activation constraint).
    pub temps: Vec<Value>,
    /// How far this activation has progressed, counted in the unit
    /// `pc_kind` names. Set where the frame is built and never
    /// reinterpreted: the engine that will run the body is chosen at the
    /// same place, so the two fields are decided together.
    pub pc: usize,
    pub pc_kind: PcKind,
    pub sender: Option<FrameRef>,
    /// Set only for `FrameKind::Closure` -- the enclosing method or
    /// initializer activation.
    pub home: Option<FrameRef>,
    /// `None` only for `FrameKind::TopLevel` (`newspeak-spec.pdf` §3.6: a
    /// top-level activation's current class is `nil`).
    pub current_class: Option<ClassRef>,
    pub kind: FrameKind,
    /// Set once a `^` return statement has run in this activation or one
    /// whose continuation chains through it (`newspeak-spec.pdf` §3.6,
    /// §7.2). A further attempt to return through an uncontinuable
    /// activation is a `cannotReturn` error.
    pub uncontinuable: bool,
    /// The absolute field index a compiled `InitSlot`'s mixin-relative
    /// `own_index` is offset by -- `MixinApplication.own_slot_base` for the
    /// class level this activation initializes. Meaningful only for a
    /// `FrameKind::Initializer` frame while `Engine::Bytecode` runs its
    /// compiled body. There is one such frame per level of the superclass
    /// chain (`Interpreter::run_initializer_level`), and every field of it
    /// -- receiver aside -- describes that one level, this one included: it
    /// is set once, where the frame is built, and never changes. Irrelevant
    /// (left at 0) for every other frame kind: a method, block, clause, or
    /// top-level body never contains `InitSlot`.
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
    /// construct other values in these tests. A real bootstrap (a later
    /// task) wires every builtin class's `.class` up properly; this helper
    /// only needs the type shapes to compile and hold together, and no test
    /// in this file ever reads through the self-reference. The construction
    /// itself lives in `Heap::alloc_self_referential_class` -- see its doc
    /// comment for why `Object.class` being non-optional forces this out of
    /// safe Rust.
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
        assert!(heap.is_class(&class));
    }

    #[test]
    fn value_clone_shares_object_identity() {
        let mut heap = crate::Heap::new();
        let class = dummy_class(&mut heap);
        let instance = heap.alloc_object(class, ObjectData::Instance(vec![]));
        let a = Value::Obj(instance.clone());
        let b = a.clone();
        match (a, b) {
            (Value::Obj(x), Value::Obj(y)) => assert!(x.same(&y)),
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
            pc_kind: PcKind::TreeWalk,
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
