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

#[derive(Debug)]
pub struct ClassData {
    pub name: Symbol,
    pub superclass: Option<ClassRef>,
    pub methods: HashMap<Symbol, Rc<ast::MethodDecl>>,
    pub instance_slots: Vec<Rc<ast::SlotDecl>>,
    /// Each of `instance_slots`' (own, not inherited) absolute index into an
    /// instance's `ObjectData::Instance` fields vector -- computed once in
    /// `Interpreter::load_class` (Task 7), offset past every inherited
    /// slot's own index range. Populated for builtins too (always empty --
    /// no `BuiltinClass` declares slots in Milestone 1).
    pub slot_index: HashMap<Symbol, usize>,
    /// This class's own `initExprs` statement list (`newspeak-spec.pdf`
    /// §6.1/§6.3, Class Instantiation step 5) -- evaluated, in order, after
    /// this class's own slots are initialized. Empty for every
    /// `BuiltinClass` (no Newspeak-source init exprs to run).
    pub init_exprs: Vec<ast::Stmt>,
    pub class_side: Option<ClassRef>,
    pub enclosing: Option<ObjRef>,
    pub factory: Option<Rc<ast::MessagePattern>>,
    /// `None` for a user-defined class. `Some(kind)` marks one of the
    /// Milestone 1 core-library classes -- see Tasks 10-12.
    pub builtin: Option<BuiltinClass>,
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
                    name: Symbol::intern("Object"),
                    superclass: None,
                    methods: HashMap::new(),
                    instance_slots: vec![],
                    slot_index: HashMap::new(),
                    init_exprs: vec![],
                    class_side: None,
                    enclosing: None,
                    factory: None,
                    builtin: Some(BuiltinClass::Object),
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
