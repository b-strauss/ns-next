//! Core runtime data types: the object model, activation records, and the
//! resolver's output shape. Pure data -- see
//! `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md`'s Object
//! Model, Activation Records, and Resolver sections.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::Symbol;

// ---------- Values and objects ----------

pub type ObjRef = Rc<RefCell<Object>>;
/// A class is an ordinary `ObjRef` whose `data` is `ObjectData::Class(_)`.
/// Not enforced by the type system in Milestone 1 (no metaclasses yet).
pub type ClassRef = ObjRef;

#[derive(Debug, Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    SmallInt(i64),
    Float(f64),
    Obj(ObjRef),
}

#[derive(Debug)]
pub struct Object {
    pub class: ClassRef,
    /// Monotonic, assigned at allocation -- never derived from the `Rc`
    /// pointer address (liveness spec's stable-identity constraint).
    pub identity_hash: u32,
    pub data: ObjectData,
}

#[derive(Debug)]
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
#[derive(Debug)]
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
    /// Milestone 1 core-library classes -- see Tasks 10-12.
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
    /// resolved under. Any `LexicalSend`, `SuperSend`, or local-variable
    /// reference inside the block body needs this exact map to resolve --
    /// invoking the closure with a different (e.g. empty) map would break
    /// such references. Cloning an `Rc` here is cheap.
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

pub type FrameRef = Rc<RefCell<Frame>>;

#[derive(Debug)]
pub enum FrameKind {
    Method(Rc<ast::MethodDecl>),
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

#[derive(Debug)]
pub struct Frame {
    pub receiver: Value,
    pub args: Vec<Value>,
    /// Indexable by resolver slot index, growable (liveness spec's
    /// reified-activation constraint). `pc` means two different things
    /// depending on which engine built this frame's code: under
    /// `Engine::TreeWalk` it is a statement index into the enclosing
    /// `CodeBody.stmts`; under `Engine::Bytecode` (`vm::run_code`) it is an
    /// instruction index into the enclosing `CompiledCode.instrs`. Which one
    /// applies is determined by the engine active when the frame ran, not by
    /// anything stored on the frame itself.
    pub temps: Vec<Value>,
    pub pc: usize,
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

    /// Builds a minimal, genuinely self-referential class object sufficient
    /// to construct other values in these tests. A real bootstrap (a later
    /// task) wires every builtin class's `.class` up properly; this helper
    /// only needs the type shapes to compile and hold together, and no test
    /// in this file ever reads through the self-reference.
    ///
    /// `Object.class` is non-optional (no `Option`/`Weak`), so a genuinely
    /// self-referential root cannot be built in one safe-Rust expression --
    /// the very first `Object` needs a `class` value before any
    /// `Rc<RefCell<Object>>` exists to supply one. We build the allocation
    /// uninitialized, mint a second strong handle to the SAME allocation
    /// (its count balanced via `Rc::increment_strong_count` before calling
    /// `Rc::from_raw`), write the real value using that handle as `class`,
    /// then `assume_init`.
    /// A root application with no code, no slots and no superclass. Built
    /// directly rather than through `Interpreter::apply_mixin` so this
    /// module's tests stay free of the interpreter.
    fn empty_application(name: &str) -> Rc<crate::mixin::MixinApplication> {
        Rc::new(crate::mixin::MixinApplication {
            mixin: Rc::new(crate::mixin::Mixin {
                name: Symbol::intern(name),
                methods: HashMap::new(),
                instance_slots: vec![],
                init_exprs: vec![],
                factory: None,
                nested: vec![],
                class_side: None,
                decl: None,
            }),
            superclass: None,
            slot_index: HashMap::new(),
            nested_slot: HashMap::new(),
            num_slots: 0,
            own_slot_base: 0,
        })
    }

    fn dummy_class() -> ClassRef {
        let uninit: Rc<std::mem::MaybeUninit<RefCell<Object>>> = Rc::new_uninit();
        let raw_mut = Rc::as_ptr(&uninit) as *mut std::mem::MaybeUninit<RefCell<Object>>;
        let data_ptr = raw_mut as *const RefCell<Object>;
        // SAFETY: `data_ptr` points at the allocation `uninit` owns; we
        // bump its strong count by one before minting a second `Rc` handle
        // to it via `from_raw`, so the two handles (`uninit`, and the
        // `class` field written below) share a correctly balanced count.
        // `MaybeUninit<T>` is guaranteed to have the same layout as `T`, so
        // the pointer cast is sound; we never dereference through either
        // handle before the `write` below completes.
        unsafe { Rc::increment_strong_count(data_ptr) };
        let self_ref: ClassRef = unsafe { Rc::from_raw(data_ptr) };
        unsafe {
            (*raw_mut).write(RefCell::new(Object {
                class: self_ref,
                identity_hash: 0,
                data: ObjectData::Class(ClassData {
                    application: empty_application("Object"),
                    enclosing: None,
                    declaring_class: None,
                    superclass: None,
                    builtin: Some(BuiltinClass::Object),
                    metaclass: None,
                }),
            }));
        }
        // SAFETY: the write above just initialized the payload; `uninit`
        // is the allocation's original handle, now fully initialized.
        unsafe { uninit.assume_init() }
    }

    #[test]
    fn class_data_is_reachable_through_object_data() {
        let class = dummy_class();
        let borrowed = class.borrow();
        assert!(matches!(&borrowed.data, ObjectData::Class(_)));
    }

    #[test]
    fn value_clone_shares_object_identity() {
        let class = dummy_class();
        let instance = Rc::new(RefCell::new(Object {
            class,
            identity_hash: 1,
            data: ObjectData::Instance(vec![]),
        }));
        let a = Value::Obj(instance.clone());
        let b = a.clone();
        match (a, b) {
            (Value::Obj(x), Value::Obj(y)) => assert!(Rc::ptr_eq(&x, &y)),
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
