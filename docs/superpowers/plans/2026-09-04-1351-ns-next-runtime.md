# ns-next Runtime (Milestone 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Status: Complete.** Verified against the codebase and git history on
> 2026-09-04 — all 12 tasks (57 steps) plus the final verification checklist
> implemented and tested: `cargo test --workspace`, `cargo fmt --all --
> --check`, and `cargo clippy --workspace --all-targets` are all clean, and
> every file/module this plan specifies exists in `crates/ns-runtime` and
> `crates/ns-cli`. Kept for historical reference; do not use as a checklist
> for new work.

**Goal:** Build `ns-runtime` — a resolver, object model, and tree-walking
interpreter for the Newspeak3 language, plus an `ns-next run` CLI command —
that resolves names per Newspeak's real (spec-verified) scoping rules,
dispatches every send through the correct one of three access-control-aware
lookup algorithms, instantiates classes via the spec's deferred-superclass-
message model, and executes a small Rust-native core library.

**Architecture:** A new `ns-runtime` crate consumed by `ns-cli`. Pipeline:
`ns-syntax::parse` → `ns-runtime::resolve` (produces a `ResolvedModule`: the
existing AST plus a side-table of `Binding`s, keyed by `Span`, no AST
mutation) → `ns-runtime::Interpreter` (tree-walking, `Rc<RefCell<_>>` object
graph, explicit heap-reachable `Frame` activation stack) → `ns-cli run`
(instantiates the loaded module's top-level class via its factory, sends the
entry message, renders the result or an uncaught-error stack trace).

**Tech Stack:** Rust 1.98 (edition 2021), no new external dependencies beyond
what the workspace already has (`ns-runtime` depends only on `ns-syntax` and
the standard library — no parser generator, no async runtime; M1 is
single-threaded).

**Spec:** `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md` — read
it before starting, in full; this plan implements it section by section and
cites section headings throughout rather than re-deriving them. That spec in
turn cites `docs/references/newspeak-spec.pdf` (Gilad Bracha's official
Newspeak language specification, vendored in the repo) by section number
(`§N.N`) for the precise semantics — **read the cited `§` sections of the PDF
directly** when a task references one; this plan does not reproduce the full
text of every citation. Also read
`docs/superpowers/specs/2026-09-04-1153-ns-next-liveness-images-vm-design.md` — the
cross-cutting constraints spec — before Task 3 (it governs the shape of
`Frame`, `Object`, and `ResolvedMethod`).

## Global Constraints

- **Rust toolchain:** 1.98.0, pinned via `rust-toolchain.toml` (already in
  the repo).
- **Edition:** 2021.
- **All code, identifiers, comments, and doc comments in English.**
- **Lints:** `#![warn(clippy::all)]` at the crate root; `cargo clippy
  --all-targets` must be warning-free before every commit.
- **Formatting:** `cargo fmt --all` before every commit; CI-style check is
  `cargo fmt --all -- --check`.
- **No `unwrap()` / `expect()` / `panic!` on Newspeak-program-input paths.**
  A malformed or erroring *Newspeak program* — a resolution failure, a
  `doesNotUnderstand:`, an arithmetic overflow, a caught-or-uncaught
  Newspeak exception — produces a `ResolveError` or `NewspeakError` value,
  never a Rust panic. Genuine **interpreter invariant violations** (a
  `Binding` pointing at a slot index that doesn't exist, a `Frame` stack
  that underflows) *are* allowed to `panic!` / `debug_assert!` per the
  spec's Error Handling section — these indicate an `ns-runtime` bug, not a
  bad Newspeak program, and must be visibly distinct from `NewspeakError`.
  `unwrap`/`expect` remain fine in tests.
- **Terminology:** classic terms — `blacklist`, `whitelist`, `sane`.
- **Grammar and spec are the sources of truth.** `corpus/reference/NewspeakGrammar.ns`
  for syntax, `docs/references/newspeak-spec.pdf` for semantics. When this
  plan's text and either of those disagree, they win — note the deviation in
  a code comment, the way the frontend milestone did.
- **Every `Object` gets a stable, monotonic `identity_hash`** — never derived
  from its `Rc` pointer address (per the liveness spec's stable-identity
  constraint).
- **Every Newspeak-level activation is an explicit, heap-reachable `Frame`**,
  never represented only by Rust locals or the Rust call stack (per the
  liveness spec). The interpreter may still use Rust recursion for its own
  control flow.
- **Method dispatch always goes through a class's mutable method
  dictionary** (or an explicitly invalidatable cache later) — never bake a
  method-body pointer into anything durable.
- **Every send is a virtual call except `private` targets** — see the spec's
  Dispatch section. Do not add a "fast path" that skips access-control
  lookup for any other case.
- **Commits:** conventional-commit style (`feat:`, `test:`, `fix:`, `docs:`,
  `chore:`). Commit at the end of every task; small intermediate commits
  within a task are fine. Keep the trailer `Co-Authored-By: Claude Sonnet 5
  <noreply@anthropic.com>`. Never override git author/committer identity
  with `-c` — the user's own gitconfig is correct as-is.
- **Never mutate `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md`
  or the liveness spec while executing this plan.** If implementation
  reveals the spec is wrong, stop and flag it (per the subagent-driven-
  development skill's ruling process) rather than silently diverging — the
  spec is the source of truth and any correction to it is a deliberate,
  reviewed act, the way the last several spec revisions were.

---

## File Structure

```
D:\Workspace\ns-next\
  Cargo.toml                          # add "crates/ns-runtime" to members
  crates/
    ns-runtime/
      Cargo.toml
      src/
        lib.rs                        # public API: resolve(), Interpreter, re-exports
        symbol.rs                     # Symbol interning
        types.rs                      # Value, Object, ClassData, ClosureData, Frame,
                                       # Binding, ResolvedMethod/ResolvedModule, errors
        resolver/
          mod.rs                      # Resolver entry point, produces ResolvedModule
          scope.rs                    # activation-chain scope stack (Local/OuterLocal)
          classes.rs                  # static per-class member-set + superclass-name computation
          sends.rs                    # implicit/self/outer/super send-shape recognition,
                                       # the §5.7 depth search
        dispatch.rs                   # kth-enclosing-object, lookupPublic, lookupProtected,
                                       # private dispatch, doesNotUnderstand:
        interpreter/
          mod.rs                      # Interpreter struct, module loading, entry point
          eval.rs                     # expression evaluation (literals, sends, cascades,
                                       # blocks, non-local return, assignment)
          instantiate.rs              # class instantiation (Class Instantiation, spec §6.1/§6.3)
        builtins/
          mod.rs                      # install_core_library(), NativeFn type
          object_class.rs             # Object, doesNotUnderstand: default
          boolean.rs                  # Boolean / True / False
          numbers.rs                  # Integer (i64), Float (f64)
          text.rs                     # String, Symbol, Character
          collections.rs              # Array, OrderedCollection
          block.rs                    # Block (closure invocation primitives)
        platform.rs                   # minimal `platform` object + stdout stream
      tests/
        resolver.rs                   # resolver unit tests (integration-style, via resolve())
        dispatch.rs                   # dispatch unit tests
        interpreter.rs                # interpreter behavior tests
        run_fixtures.rs               # runs every tests-fixtures/*.ns, compares .expected
    ns-cli/
      src/main.rs                     # add `run` subcommand
  tests-fixtures/
    *.ns + *.expected                 # execution integration tests
  docs/superpowers/specs/
```

Responsibilities:

- **`symbol.rs`** — global, leaking string interner. Nothing else depends on
  its internals.
- **`types.rs`** — every core data shape (`Value`, `Object`, `ClassData`,
  `ClosureData`, `Frame`, `Binding`, `ResolvedMethod`, `ResolvedModule`,
  `ResolveError`, `NewspeakError`) in one file with no behavior, because they
  are mutually referential (a `ClosureData` holds a `FrameRef`, a `Frame`
  holds a `MethodRef` which holds `Binding`s) and splitting them would force
  an artificial compile order across tasks. Behavior lives in `resolver/`,
  `dispatch.rs`, and `interpreter/`.
- **`resolver/`** — `Module` (AST) → `ResolvedModule`. Split by concern
  (activation-chain scope vs. static class member-sets vs. send-shape
  recognition) because each is independently testable and the file would
  otherwise be large.
- **`dispatch.rs`** — the three lookup algorithms and the kth-enclosing-object
  computation. One file: these are a tightly coupled unit (§3.3, §5.5, §5.9,
  §5.10 of the spec) with no natural sub-boundary.
- **`interpreter/`** — the tree-walking evaluator and class instantiation.
  Split because instantiation (spec §6.1/§6.3) is a distinct, self-contained
  algorithm invoked from one place (a factory-message send), not part of
  general expression evaluation.
- **`builtins/`** — one file per related cluster of core-library classes,
  matching the spec's Core Library list.
- **`ns-cli/src/main.rs`** — argument parsing, error/stack-trace rendering
  only; no interpretation logic of its own (same rule the frontend milestone
  applied to parsing).

---

## Task 1: `ns-runtime` crate scaffold

**Files:**
- Modify: `Cargo.toml` (add workspace member)
- Create: `crates/ns-runtime/Cargo.toml`, `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: a building `ns-runtime` crate with an empty `lib.rs` (only
  `#![warn(clippy::all)]` and a crate doc comment).

- [x] **Step 1: Add the workspace member**

Modify `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/ns-syntax", "crates/ns-runtime", "crates/ns-cli"]
```

(Insert `"crates/ns-runtime"` between the existing two members — keep
alphabetical-by-pipeline-order, matching the existing style.)

- [x] **Step 2: Create the crate skeleton**

`crates/ns-runtime/Cargo.toml`:

```toml
[package]
name = "ns-runtime"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
ns-syntax = { path = "../ns-syntax" }
```

`crates/ns-runtime/src/lib.rs`:

```rust
#![warn(clippy::all)]

//! Name resolution, object model, and tree-walking interpreter for the
//! Newspeak3 language (Milestone 1). See
//! `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md`.
```

- [x] **Step 3: Verify the workspace builds**

Run: `cargo build --workspace`
Expected: compiles, three crates.

Run: `cargo test --workspace`
Expected: passes (0 new tests; existing `ns-syntax`/`ns-cli` tests still
green, currently 110 total per the last full verification).

Run: `cargo fmt --all -- --check` and `cargo clippy --all-targets`
Expected: both clean.

- [x] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(runtime): scaffold the ns-runtime crate

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 2: Symbol interning

**Files:**
- Create: `crates/ns-runtime/src/symbol.rs`
- Modify: `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Symbol` (`Copy`, `Eq`, `Hash`, `Ord`), `Symbol::intern(&str) ->
  Symbol`, `Symbol::as_str(self) -> &'static str`, `impl Display for Symbol`.
  Every later task that needs a selector, a class name, or a slot name as a
  key uses `Symbol`, not `String`, for cheap `Copy` comparisons in hot
  dispatch paths.

- [x] **Step 1: Write the failing tests**

`crates/ns-runtime/src/symbol.rs`:

```rust
//! Global symbol interning for selectors, slot names, and class names.
//!
//! Interned strings are leaked (`Box::leak`) and live for the process
//! lifetime. This is an accepted Milestone 1 simplification — the same
//! spirit as the `Rc<RefCell<_>>` object-graph leak already accepted in
//! the runtime spec's Decisions section. A later milestone's GC would
//! intern into a proper symbol table instead.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_the_same_string_twice_gives_the_same_symbol() {
        let a = Symbol::intern("foo:");
        let b = Symbol::intern("foo:");
        assert_eq!(a, b);
    }

    #[test]
    fn different_strings_intern_to_different_symbols() {
        let a = Symbol::intern("foo:");
        let b = Symbol::intern("bar:");
        assert_ne!(a, b);
    }

    #[test]
    fn as_str_round_trips() {
        let s = Symbol::intern("printString");
        assert_eq!(s.as_str(), "printString");
    }

    #[test]
    fn display_matches_as_str() {
        let s = Symbol::intern("at:put:");
        assert_eq!(format!("{s}"), "at:put:");
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime symbol::`
Expected: FAIL — `Symbol` not found.

- [x] **Step 3: Write the implementation**

Prepend to `crates/ns-runtime/src/symbol.rs` (before the `#[cfg(test)]`
module):

```rust
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(u32);

struct Interner {
    strings: Vec<&'static str>,
    lookup: HashMap<&'static str, Symbol>,
}

impl Interner {
    fn new() -> Self {
        Interner {
            strings: Vec::new(),
            lookup: HashMap::new(),
        }
    }
}

thread_local! {
    static INTERNER: RefCell<Interner> = RefCell::new(Interner::new());
}

impl Symbol {
    pub fn intern(s: &str) -> Symbol {
        INTERNER.with(|cell| {
            let mut interner = cell.borrow_mut();
            if let Some(&sym) = interner.lookup.get(s) {
                return sym;
            }
            let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
            let sym = Symbol(interner.strings.len() as u32);
            interner.strings.push(leaked);
            interner.lookup.insert(leaked, sym);
            sym
        })
    }

    pub fn as_str(self) -> &'static str {
        INTERNER.with(|cell| cell.borrow().strings[self.0 as usize])
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.as_str())
    }
}
```

Remove the `#[derive(Debug, ...)]` on the `struct Symbol` line above and
keep only `#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]` —
the manual `impl fmt::Debug` above replaces the derived one (derived would
print the raw `u32`, which is useless for debugging).

Update `crates/ns-runtime/src/lib.rs`:

```rust
pub mod symbol;

pub use symbol::Symbol;
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime symbol::`
Expected: PASS (4 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): add Symbol interning

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 3: Core runtime types

**Files:**
- Create: `crates/ns-runtime/src/types.rs`
- Modify: `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Consumes: `Symbol` (Task 2), `ns_syntax::ast::*`, `ns_syntax::Span`.
- Produces every core data shape used by every later task. This task has no
  behavior — it exists so `Object`, `ClassData`, `ClosureData`, `Frame`, and
  `ResolvedModule` (which are mutually referential — a `ClosureData` holds a
  `FrameRef`, a `Frame` holds a `FrameKind` which can hold a `ClosureData`)
  compile together once, rather than forcing an artificial split across
  later tasks.

**Design notes (read before writing code):**

- `ClassRef = ObjRef`: a class is an ordinary object whose `data` is
  `ObjectData::Class(_)`, per the runtime spec's Object Model and its
  "Documented Deviations" note that Milestone 1 has no metaclass
  architecture. Nothing enforces this invariant at the type level in M1.
- **No `NativeFn` closure type, no `native_methods` field on `ClassData`.**
  The spec's Core Library is small and fixed for M1; rather than a generic
  native-method registry (which would need `ClassData` in `types.rs` to
  reference the not-yet-defined `Interpreter` type from Task 8, an avoidable
  circular dependency), `ClassData` instead carries an `Option<BuiltinClass>`
  tag. The interpreter's dispatch (Task 8) checks this tag and calls a
  hand-written `builtins::dispatch_native` function (Tasks 10-12) when no
  user-defined method is found. Simpler for a fixed, small core library, and
  keeps `types.rs` free of forward references.
- `ClassData.methods` and `.instance_slots` reuse `ns_syntax::ast::MethodDecl`
  / `SlotDecl` directly (`Rc<ast::MethodDecl>`, `Rc<ast::SlotDecl>`) rather
  than inventing parallel "resolved" wrapper types — the resolver's
  `Binding`s live in one shared, `Span`-keyed side table (on
  `ResolvedModule`), so nothing about a method or slot's own AST node needs
  to change shape after resolution.
- `ClosureData.home: FrameRef` (not `ObjRef`, unlike the spec's short Object
  Model sketch): a closure's non-local return (`newspeak-spec.pdf` §7.2)
  needs the actual enclosing **method activation** `Frame` — to check
  whether it is still continuable and to pass control to its continuation —
  not just "an object". This generalizes the spec's one-line sketch, which
  only named the enclosing object; it does not contradict it.
- `Frame.kind: FrameKind` (not a bare `method: MethodRef` field, unlike the
  spec's Frame sketch): `newspeak-spec.pdf` §3.6 defines three kinds of
  activation (method, closure, and top-level), each with different rules for
  `current_class`, `home`, and return-statement behavior (§7.2). `FrameKind`
  makes that a closed enum instead of leaving `TopLevel` and `Closure`
  unrepresentable. `current_class` is `Option<ClassRef>` because §3.6 is
  explicit that a top-level activation's current class is `nil`.

- [x] **Step 1: Write the file**

`crates/ns-runtime/src/types.rs`:

```rust
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
    pub class_side: Option<ClassRef>,
    pub enclosing: Option<ObjRef>,
    pub factory: Option<Rc<ast::MessagePattern>>,
    /// `None` for a user-defined class. `Some(kind)` marks one of the
    /// Milestone 1 core-library classes -- see Tasks 10-12.
    pub builtin: Option<BuiltinClass>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[derive(Debug, Clone)]
pub struct ClosureData {
    pub params: Vec<Symbol>,
    pub body: Rc<ast::CodeBody>,
    pub captured_frame: FrameRef,
    /// The lexically enclosing *method* activation -- see the design note
    /// above on why this is a `FrameRef`, not an `ObjRef`.
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
    TopLevel,
}

#[derive(Debug)]
pub struct Frame {
    pub receiver: Value,
    pub args: Vec<Value>,
    /// Indexable by resolver slot index, growable (liveness spec's
    /// reified-activation constraint). `pc` is a statement index into the
    /// enclosing `CodeBody.stmts`, not a byte/bytecode offset (Milestone 1
    /// has no bytecode).
    pub temps: Vec<Value>,
    pub pc: usize,
    pub sender: Option<FrameRef>,
    /// Set only for `FrameKind::Closure` -- the enclosing method activation.
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
```

- [x] **Step 2: Wire into `lib.rs`**

```rust
pub mod symbol;
pub mod types;

pub use symbol::Symbol;
pub use types::*;
```

- [x] **Step 3: Add construction smoke tests**

Append to `crates/ns-runtime/src/types.rs`. Add `PartialEq, Eq` to
`Binding`'s existing `#[derive(Debug, Clone, Copy)]` so the last test's
`assert_eq!` compiles:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal, non-self-referential class object sufficient to
    /// construct other values in these tests. A real bootstrap (Task 13)
    /// wires every builtin class's `.class` up properly; these tests only
    /// check the type shapes compile and hold together.
    fn dummy_class() -> ClassRef {
        let root_class_of_class = Rc::new(RefCell::new(Object {
            class: unreachable!("not read in these tests"),
            identity_hash: 0,
            data: ObjectData::Instance(vec![]),
        }));
        Rc::new(RefCell::new(Object {
            class: root_class_of_class,
            identity_hash: 0,
            data: ObjectData::Class(ClassData {
                name: Symbol::intern("Object"),
                superclass: None,
                methods: HashMap::new(),
                instance_slots: vec![],
                class_side: None,
                enclosing: None,
                factory: None,
                builtin: Some(BuiltinClass::Object),
            }),
        }))
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
```

Note: `unreachable!("not read in these tests")` inside `root_class_of_class`
above will panic *if that specific code path executes*, but this is a field
initializer whose value is only ever moved into `Object.class` and never
called as a function — it is fine here specifically because the test never
reads that particular grandparent `.class` field again. If this feels too
clever during implementation, replace it with a second, fully-formed dummy
`ClassRef` built the same way `dummy_class` builds its own (three levels
deep) — either is acceptable; the only real requirement is that the test
file compiles and the assertions hold.

- [x] **Step 4: Run tests**

Run: `cargo test -p ns-runtime types::`
Expected: PASS (4 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): add core object-model, activation, and resolver types

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 4: Resolver — locals, static member sets, send-shape recognition

This is the plan's most intricate task; it implements the runtime spec's
entire Resolver section (including its "AST recognition" note) in one pass,
because the pieces are too interdependent to review in isolation — a
reviewer needs the whole pass to judge correctness.

**Read `2026-09-01-2252-ns-next-runtime-design.md`'s Resolver section in full
before starting** (including the "AST recognition" note). This task
transcribes that section's algorithm into Rust; it does not re-derive it.

**Files:**
- Create: `crates/ns-runtime/src/resolver/mod.rs`,
  `crates/ns-runtime/src/resolver/scope.rs`,
  `crates/ns-runtime/src/resolver/classes.rs`,
  `crates/ns-runtime/src/resolver/sends.rs`
- Modify: `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Consumes: `ns_syntax::ast::*`, `Binding`, `ResolvedModule`, `ResolveError`
  (Task 3).
- Produces: `pub fn resolve(module: ast::Module, source: Rc<str>, origin:
  SourceOrigin) -> Result<ResolvedModule, Vec<ResolveError>>` — the sole
  public entry point later tasks call.

**Design notes:**

- `ast::SuperclassClause { prefix: Option<SuperclassPrefix>, receiver: Ident,
  messages: Vec<MessageSend>, span }` already isolates the superclass
  clause's receiver from its trailing message(s) — the frontend built this
  shape specifically for this. So resolving *which class* a superclass
  clause names does **not** need the general `Send`-shape pattern matching
  below; it only needs the same lexical/depth search applied directly to
  `receiver.name` (with `prefix` handled the same way an explicit
  self/super/outer receiver would be). The trailing `messages` are the
  spec's deferred factory message — Task 7 (Class Instantiation) evaluates
  them, not the resolver.
- **Builtin class names have no lexical declaration to find.** `Object` and
  the rest of the Core Library (Task 8-10) are never declared *in* Newspeak
  source. When the lexical/depth search for a superclass-clause receiver (or
  any other name) exhausts every enclosing level and finds nothing, check
  the name against the fixed builtin set before reporting a `ResolveError` —
  this is the base case that lets class hierarchies bottom out at `Object`.
- **Static member sets are needed only to pick the search depth `k`,
  not to fully resolve inherited method bodies.** `classes.rs` computes, per
  class declaration, the set of member *names* (own + statically-inherited)
  — never method bodies — because that's all §5.7's depth search needs to
  know "does lexical level `k` have something named `s`". The actual method
  body used at runtime is found later by `dispatch.rs` (Task 5) walking the
  *real* object graph, which is what makes overriding work — the resolver's
  member-set computation only fixes *which lexical level*, never *which
  concrete method*.

- [x] **Step 1: Activation-chain scope (`scope.rs`)**

```rust
//! Static (activation-chain) local-variable resolution: method/block
//! temporaries and parameters. See the runtime spec's Resolver section,
//! step 1.

use std::collections::HashMap;

use crate::Symbol;
use crate::types::Binding;

/// One method or closure body's own temps+params, innermost activation last.
pub(super) struct LocalScope {
    names: HashMap<Symbol, u16>,
}

impl LocalScope {
    pub(super) fn new(names_in_order: &[Symbol]) -> LocalScope {
        let mut names = HashMap::new();
        for (i, &name) in names_in_order.iter().enumerate() {
            names.insert(name, i as u16);
        }
        LocalScope { names }
    }
}

/// The chain of activation scopes currently open while walking a method or
/// closure body, innermost (current) first.
pub(super) struct ScopeStack {
    scopes: Vec<LocalScope>,
}

impl ScopeStack {
    pub(super) fn new() -> ScopeStack {
        ScopeStack { scopes: Vec::new() }
    }

    pub(super) fn push(&mut self, scope: LocalScope) {
        self.scopes.push(scope);
    }

    pub(super) fn pop(&mut self) {
        self.scopes.pop();
    }

    /// Resolves `name` against the activation chain only. `Some(Local)` if
    /// found in the innermost scope, `Some(OuterLocal { depth, .. })` if
    /// found `depth` activations out, `None` if not a local at all (the
    /// caller then tries the lexical-send / class search).
    pub(super) fn resolve(&self, name: Symbol) -> Option<Binding> {
        for (depth, scope) in self.scopes.iter().rev().enumerate() {
            if let Some(&index) = scope.names.get(&name) {
                return Some(if depth == 0 {
                    Binding::Local(index)
                } else {
                    Binding::OuterLocal {
                        depth: depth as u16,
                        index,
                    }
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn innermost_scope_resolves_to_local() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x"), Symbol::intern("y")]));
        assert_eq!(stack.resolve(Symbol::intern("y")), Some(Binding::Local(1)));
    }

    #[test]
    fn enclosing_scope_resolves_to_outer_local() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")]));
        stack.push(LocalScope::new(&[Symbol::intern("y")]));
        assert_eq!(
            stack.resolve(Symbol::intern("x")),
            Some(Binding::OuterLocal { depth: 1, index: 0 })
        );
    }

    #[test]
    fn unknown_name_resolves_to_none() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")]));
        assert_eq!(stack.resolve(Symbol::intern("z")), None);
    }

    #[test]
    fn inner_shadows_outer() {
        let mut stack = ScopeStack::new();
        stack.push(LocalScope::new(&[Symbol::intern("x")]));
        stack.push(LocalScope::new(&[Symbol::intern("x")]));
        assert_eq!(stack.resolve(Symbol::intern("x")), Some(Binding::Local(0)));
    }
}
```

- [x] **Step 2: Run, verify pass**

Run: `cargo test -p ns-runtime resolver::scope::`
Expected: PASS (4 tests) once `mod.rs` (Step 5) wires this module in — write
this file first, but don't expect it to compile alone until `resolver/mod.rs`
exists with `mod scope;`. Do Steps 1-4 as file-writes, then Step 5, then run
everything together.

- [x] **Step 3: Static class member sets and superclass names (`classes.rs`)**

```rust
//! Static member-set and superclass-name computation for classes, used only
//! to pick a lexical-send depth `k` (runtime spec's Resolver section, step
//! 2) -- never to resolve which concrete method body runs (that is
//! `dispatch.rs`'s job, at runtime, over the real object graph).

use std::collections::HashSet;

use ns_syntax::ast;

use crate::Symbol;

/// Names Object supports with no Newspeak-source declaration to find --
/// resolving a superclass-clause receiver (or a bare implicit send) against
/// these is the base case that lets a lexical/inheritance search bottom out.
/// Extend this list as Tasks 8-10 add core-library classes and their public
/// selectors that user code may reasonably reference by bare name from a
/// superclass clause (in practice, just the class *names* below matter for
/// superclass resolution -- Object's own instance-side selectors matter for
/// implicit sends that fall all the way through to Object).
pub(super) const BUILTIN_CLASS_NAMES: &[&str] = &[
    "Object",
    "Boolean",
    "True",
    "False",
    "Integer",
    "Float",
    "String",
    "Symbol",
    "Character",
    "Array",
    "OrderedCollection",
    "Nil",
    "Block",
];

pub(super) fn is_builtin_class_name(name: Symbol) -> bool {
    BUILTIN_CLASS_NAMES.contains(&name.as_str())
}

/// Names declared *directly* by one class declaration: its own methods
/// (both sides' method selectors, unary base name for keyword/binary
/// selectors is the *first* keyword/the operator itself -- see
/// `member_name_of_pattern` below), its own slots, and its own immediately
/// nested class names.
pub(super) fn own_member_names(decl: &ast::ClassDecl) -> HashSet<Symbol> {
    let mut names = HashSet::new();
    let mut collect_body = |body: &ast::ClassBody| {
        for slot in &body.slots {
            names.insert(Symbol::intern(&slot.name.name));
        }
        for method in &body.methods {
            names.insert(member_name_of_pattern(&method.pattern));
        }
        for nested in &body.nested_classes {
            names.insert(Symbol::intern(&nested.name.name));
        }
    };
    collect_body(&decl.instance_side);
    if let Some(class_side) = &decl.class_side {
        collect_body(class_side);
    }
    names
}

/// The name a message pattern is looked up under for §5.7's search: a
/// setter (`foo::`) counts under its getter's plain name `foo`, matching
/// how a mutable slot's setter and getter share one declared slot name.
fn member_name_of_pattern(pattern: &ast::MessagePattern) -> Symbol {
    match pattern {
        ast::MessagePattern::Unary { selector } => Symbol::intern(&selector.name),
        ast::MessagePattern::Binary { selector, .. } => Symbol::intern(&selector.name),
        ast::MessagePattern::Keyword { parts } => {
            // The full keyword selector's first component names the member
            // for §5.7 lookup purposes (e.g. `at:put:` is found under `at`).
            Symbol::intern(&parts[0].0.name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_are_recognized() {
        assert!(is_builtin_class_name(Symbol::intern("Object")));
        assert!(!is_builtin_class_name(Symbol::intern("NotABuiltin")));
    }
}
```

Note: `own_member_names` and `member_name_of_pattern` will not compile until
Step 4 uses them together with a real `ast::Module` fixture; the unit test
above only exercises `is_builtin_class_name`, which has no such dependency.
Broader coverage of `own_member_names` comes from Step 6's integration tests,
which resolve real small modules end to end.

- [x] **Step 4: Send-shape recognition and the §5.7 depth search (`sends.rs`)**

```rust
//! Recognizes self/super/outer/implicit send shapes in the AST (runtime
//! spec's "AST recognition" note) and runs the §5.7 depth search for
//! implicit and bare-name sends.

use ns_syntax::ast::{self, Expr, Selector, Send};
use ns_syntax::Span;

use crate::Symbol;
use crate::types::Binding;

use super::classes::is_builtin_class_name;

/// One entry in the lexical class-nesting chain currently open while
/// walking a method body, innermost first. `members` is that class's own +
/// statically-inherited member-name set (built by the caller in `mod.rs`
/// via `classes::own_member_names` plus superclass-chain lookups).
pub(super) struct EnclosingClass<'a> {
    pub name: Symbol,
    pub members: &'a std::collections::HashSet<Symbol>,
}

/// Runs the §5.7 depth search: the smallest `k` (0 = innermost enclosing
/// class) whose member set contains `selector`. Falls back to depth `0`
/// (self send) if nothing matches anywhere, per the spec ("otherwise, the
/// implicit receiver send is equivalent to a self send") -- runtime
/// dispatch on that then naturally produces `doesNotUnderstand:`.
pub(super) fn depth_search(chain: &[EnclosingClass], selector: Symbol) -> u16 {
    for (k, class) in chain.iter().enumerate() {
        if class.members.contains(&selector) {
            return k as u16;
        }
    }
    0
}

/// Finds the exact depth of the lexically enclosing class named `name`, for
/// an explicit `outer name m` send. `Err` is a compile-time error per
/// §5.9 ("It is a compile time error if N is not the name of a class whose
/// declaration lexically encloses the outer send").
pub(super) fn depth_of_named_outer(chain: &[EnclosingClass], name: Symbol) -> Option<u16> {
    chain
        .iter()
        .position(|class| class.name == name)
        .map(|k| k as u16)
}

/// Recognizes whether `send.receiver` is the parser's implicit-keyword-send
/// marker: `Ident { name: "self", span }` with a **zero-width** span
/// (`span.start == span.end`), synthesized by
/// `crates/ns-syntax/src/parser/expr.rs:339-344`. A real, non-zero-width
/// `self` is a genuine explicit self send (§5.8, always depth 0, no
/// search).
pub(super) fn is_synthesized_implicit_receiver(receiver: &Expr) -> bool {
    matches!(
        receiver,
        Expr::Ident(id) if id.name == "self" && id.span.start == id.span.end
    )
}

pub(super) fn is_explicit_self(receiver: &Expr) -> bool {
    matches!(receiver, Expr::Ident(id) if id.name == "self" && id.span.start != id.span.end)
}

pub(super) fn is_explicit_super(receiver: &Expr) -> bool {
    matches!(receiver, Expr::Ident(id) if id.name == "super")
}

/// Recognizes the `outer N m` two-hop shape: `receiver` is itself a `Send`
/// whose own receiver is `Ident("outer")` and whose message is a unary send
/// to an uppercase-starting selector. Returns that selector's name if so.
/// The inner "send N to outer" step is never dispatched as a real message —
/// callers that get `Some(name)` back must resolve `name`'s depth via
/// `depth_of_named_outer` and use the *outer* `Send`'s own message/selector
/// (`send.message`) as the actual send, discarding the inner shape.
pub(super) fn outer_class_name(receiver: &Expr) -> Option<&str> {
    let Expr::Send(inner) = receiver else {
        return None;
    };
    let Expr::Ident(outer_ident) = &inner.receiver else {
        return None;
    };
    if outer_ident.name != "outer" {
        return None;
    }
    match &inner.message.selector {
        Selector::Unary(name) if name.chars().next().is_some_and(char::is_uppercase) => {
            Some(name.as_str())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn class<'a>(name: &str, members: &'a HashSet<Symbol>) -> EnclosingClass<'a> {
        EnclosingClass {
            name: Symbol::intern(name),
            members,
        }
    }

    #[test]
    fn depth_search_finds_the_innermost_match() {
        let mut inner_members = HashSet::new();
        inner_members.insert(Symbol::intern("foo"));
        let outer_members = HashSet::new();
        let chain = [class("Inner", &inner_members), class("Outer", &outer_members)];
        assert_eq!(depth_search(&chain, Symbol::intern("foo")), 0);
    }

    #[test]
    fn depth_search_falls_through_to_an_outer_level() {
        let inner_members = HashSet::new();
        let mut outer_members = HashSet::new();
        outer_members.insert(Symbol::intern("foo"));
        let chain = [class("Inner", &inner_members), class("Outer", &outer_members)];
        assert_eq!(depth_search(&chain, Symbol::intern("foo")), 1);
    }

    #[test]
    fn depth_search_defaults_to_zero_when_nothing_matches() {
        let members = HashSet::new();
        let chain = [class("Inner", &members)];
        assert_eq!(depth_search(&chain, Symbol::intern("nowhere")), 0);
    }

    #[test]
    fn depth_of_named_outer_finds_an_exact_class() {
        let members = HashSet::new();
        let chain = [class("Inner", &members), class("Outer", &members)];
        assert_eq!(depth_of_named_outer(&chain, Symbol::intern("Outer")), Some(1));
        assert_eq!(depth_of_named_outer(&chain, Symbol::intern("Missing")), None);
    }
}
```

- [x] **Step 5: The resolver pass itself (`mod.rs`)**

```rust
//! Resolver entry point: `Module` -> `ResolvedModule`. See the runtime
//! spec's Resolver section for the algorithm this transcribes.

mod classes;
mod scope;
mod sends;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use crate::types::{Binding, ResolveError, ResolvedModule, SourceOrigin};
use crate::Symbol;

use scope::{LocalScope, ScopeStack};
use sends::EnclosingClass;

pub fn resolve(
    module: ast::Module,
    source: Rc<str>,
    origin: SourceOrigin,
) -> Result<ResolvedModule, Vec<ResolveError>> {
    let mut r = Resolver {
        bindings: HashMap::new(),
        errors: Vec::new(),
    };
    let member_sets = build_member_sets(&module.root_class);
    r.resolve_class(&module.root_class, &[], &member_sets);
    if r.errors.is_empty() {
        Ok(ResolvedModule {
            module: Rc::new(module),
            source,
            origin,
            bindings: Rc::new(r.bindings),
        })
    } else {
        Err(r.errors)
    }
}

/// Own + statically-inherited member names, per class declaration identity
/// (keyed by the class's own name -- sufficient for Milestone 1, where
/// nested classes are not renamed/shadowed across a single module in ways
/// that would collide two same-named classes at different nesting depths
/// within one compile; revisit with a real `ClassId` if that ever bites).
fn build_member_sets(root: &ast::ClassDecl) -> HashMap<Symbol, HashSet<Symbol>> {
    let mut sets = HashMap::new();
    collect_member_sets(root, &mut sets);
    sets
}

fn collect_member_sets(decl: &ast::ClassDecl, sets: &mut HashMap<Symbol, HashSet<Symbol>>) {
    let own = classes::own_member_names(decl);
    let superclass_name = decl
        .superclass
        .as_ref()
        .map(|clause| Symbol::intern(&clause.receiver.name))
        .unwrap_or_else(|| Symbol::intern("Object"));
    // Inherit the superclass's member set if we've already computed it
    // (nested classes are declared textually after their enclosing class,
    // and Object/builtins are the fixed base case -- their own instance
    // side selectors are added by Tasks 8-10's bootstrap, not resolved
    // here). Missing/builtin superclasses contribute an empty set here;
    // that's fine because `depth_search` only needs *this* module's own
    // lexically declared names to pick a depth.
    let mut own_plus_inherited = own;
    if let Some(inherited) = sets.get(&superclass_name) {
        own_plus_inherited.extend(inherited.iter().copied());
    }
    let name = Symbol::intern(&decl.name.name);
    sets.insert(name, own_plus_inherited);
    for nested in &decl.instance_side.nested_classes {
        collect_member_sets(nested, sets);
    }
    if let Some(class_side) = &decl.class_side {
        for nested in &class_side.nested_classes {
            collect_member_sets(nested, sets);
        }
    }
}

struct Resolver {
    bindings: HashMap<Span, Binding>,
    errors: Vec<ResolveError>,
}

impl Resolver {
    fn resolve_class(
        &mut self,
        decl: &ast::ClassDecl,
        enclosing: &[Symbol],
        member_sets: &HashMap<Symbol, HashSet<Symbol>>,
    ) {
        let name = Symbol::intern(&decl.name.name);
        let mut chain: Vec<Symbol> = enclosing.to_vec();
        chain.push(name);
        // Innermost first for the chain we hand to `sends::depth_search`.
        let mut innermost_first: Vec<Symbol> = chain.iter().rev().copied().collect();
        let class_chain: Vec<EnclosingClass> = innermost_first
            .drain(..)
            .map(|n| EnclosingClass {
                name: n,
                members: member_sets.get(&n).unwrap_or_else(|| {
                    static EMPTY: std::sync::OnceLock<HashSet<Symbol>> = std::sync::OnceLock::new();
                    EMPTY.get_or_init(HashSet::new)
                }),
            })
            .collect();

        for method in &decl.instance_side.methods {
            self.resolve_method(method, &class_chain);
        }
        if let Some(class_side) = &decl.class_side {
            for method in &class_side.methods {
                self.resolve_method(method, &class_chain);
            }
        }
        // Slot initializers execute in the instance initializer's context
        // (spec's Class Instantiation section) -- resolve them with an
        // activation scope of zero locals, same class chain.
        let mut scope = ScopeStack::new();
        scope.push(LocalScope::new(&[]));
        for slot in &decl.instance_side.slots {
            if let Some(init) = &slot.init {
                self.resolve_expr(init, &mut scope, &class_chain);
            }
        }
        for expr in &decl.instance_side.init_exprs {
            if let Stmt::Expr(e) = expr {
                self.resolve_expr(e, &mut scope, &class_chain);
            } else if let Stmt::Return(e) = expr {
                self.resolve_expr(e, &mut scope, &class_chain);
            }
        }

        for nested in &decl.instance_side.nested_classes {
            self.resolve_class(nested, &chain, member_sets);
        }
        if let Some(class_side) = &decl.class_side {
            for nested in &class_side.nested_classes {
                self.resolve_class(nested, &chain, member_sets);
            }
        }
    }

    fn resolve_method(&mut self, method: &ast::MethodDecl, class_chain: &[EnclosingClass]) {
        let param_names: Vec<Symbol> = pattern_param_names(&method.pattern);
        let temp_names: Vec<Symbol> = method
            .body
            .temps
            .iter()
            .map(|t| Symbol::intern(&t.name))
            .collect();
        let mut all = param_names;
        all.extend(temp_names);
        let mut scope = ScopeStack::new();
        scope.push(LocalScope::new(&all));
        for stmt in &method.body.stmts {
            self.resolve_stmt(stmt, &mut scope, class_chain);
        }
    }

    fn resolve_stmt(&mut self, stmt: &Stmt, scope: &mut ScopeStack, chain: &[EnclosingClass]) {
        match stmt {
            Stmt::Expr(e) | Stmt::Return(e) => self.resolve_expr(e, scope, chain),
        }
    }

    fn resolve_expr(&mut self, expr: &Expr, scope: &mut ScopeStack, chain: &[EnclosingClass]) {
        match expr {
            Expr::Ident(id) => self.resolve_ident(id, scope, chain),
            Expr::Literal(_) => {}
            Expr::Send(send) => self.resolve_send(send, scope, chain),
            Expr::Cascade(cascade) => {
                self.resolve_expr(&cascade.receiver, scope, chain);
                self.resolve_message_send(&cascade.first, scope, chain);
                for m in &cascade.rest {
                    self.resolve_message_send(m, scope, chain);
                }
            }
            Expr::Assign { target, value, .. } => {
                self.resolve_setter_target(target, scope, chain);
                self.resolve_expr(value, scope, chain);
            }
            Expr::Block(block) => {
                let param_names: Vec<Symbol> = block
                    .params
                    .iter()
                    .map(|p| Symbol::intern(&p.name))
                    .collect();
                let temp_names: Vec<Symbol> = block
                    .body
                    .temps
                    .iter()
                    .map(|t| Symbol::intern(&t.name))
                    .collect();
                let mut names = param_names;
                names.extend(temp_names);
                scope.push(LocalScope::new(&names));
                for stmt in &block.body.stmts {
                    self.resolve_stmt(stmt, scope, chain);
                }
                scope.pop();
            }
            Expr::DynamicArray { elements, .. } => {
                for e in elements {
                    self.resolve_expr(e, scope, chain);
                }
            }
        }
    }

    fn resolve_message_send(
        &mut self,
        message: &ast::MessageSend,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        for arg in &message.args {
            self.resolve_expr(arg, scope, chain);
        }
    }

    /// A bare `Expr::Ident` standing alone (not the receiver of an
    /// enclosing `Send`, since that case is handled in `resolve_send`
    /// below): try locals first, then the §5.7 depth search treating the
    /// name itself as a zero-arg implicit send. `self`/`super` alone (no
    /// message) need no binding entry -- the interpreter special-cases
    /// them directly against the current `Frame` (see Task 6).
    fn resolve_ident(&mut self, id: &ast::Ident, scope: &mut ScopeStack, chain: &[EnclosingClass]) {
        if id.name == "self" || id.name == "super" {
            return;
        }
        let name = Symbol::intern(&id.name);
        if let Some(binding) = scope.resolve(name) {
            self.bindings.insert(id.span, binding);
            return;
        }
        let depth = sends::depth_search(chain, name);
        self.bindings
            .insert(id.span, Binding::LexicalSend { depth });
    }

    fn resolve_send(&mut self, send: &Send, scope: &mut ScopeStack, chain: &[EnclosingClass]) {
        if sends::is_explicit_super(&send.receiver) {
            self.bindings.insert(send.span, Binding::SuperSend);
            self.resolve_message_send(&send.message, scope, chain);
            return;
        }
        if sends::is_explicit_self(&send.receiver) {
            self.bindings
                .insert(send.span, Binding::LexicalSend { depth: 0 });
            self.resolve_message_send(&send.message, scope, chain);
            return;
        }
        if sends::is_synthesized_implicit_receiver(&send.receiver) {
            let selector_name = member_name_of_message(&send.message);
            let depth = sends::depth_search(chain, selector_name);
            self.bindings
                .insert(send.span, Binding::LexicalSend { depth });
            self.resolve_message_send(&send.message, scope, chain);
            return;
        }
        if let Some(class_name) = sends::outer_class_name(&send.receiver) {
            let name = Symbol::intern(class_name);
            match sends::depth_of_named_outer(chain, name) {
                Some(depth) => {
                    self.bindings.insert(send.span, Binding::LexicalSend { depth });
                }
                None => self.errors.push(ResolveError {
                    span: send.span,
                    message: format!(
                        "`outer {class_name}` does not name a lexically enclosing class"
                    ),
                }),
            }
            self.resolve_message_send(&send.message, scope, chain);
            return;
        }
        // Ordinary explicit-receiver send: resolve the receiver
        // sub-expression on its own merits; the send itself needs no
        // binding (dispatch.rs's lookupPublic runs on the receiver's
        // actual runtime class -- see Task 5).
        self.resolve_expr(&send.receiver, scope, chain);
        self.resolve_message_send(&send.message, scope, chain);
    }

    /// The `id` in `id:: e` (`Expr::Assign { target, .. }`) resolves
    /// exactly like an implicit-receiver keyword send to `id:` -- see the
    /// runtime spec's "Setter sends" note. Locals win first (a local
    /// temp/param can be reassigned directly); otherwise it is a
    /// `LexicalSend` at the depth found for the plain slot name (the
    /// setter and getter share one declared name -- see
    /// `classes::member_name_of_pattern`).
    fn resolve_setter_target(
        &mut self,
        target: &ast::Ident,
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        let name = Symbol::intern(&target.name);
        if let Some(binding) = scope.resolve(name) {
            self.bindings.insert(target.span, binding);
            return;
        }
        let depth = sends::depth_search(chain, name);
        self.bindings
            .insert(target.span, Binding::LexicalSend { depth });
    }
}

fn pattern_param_names(pattern: &ast::MessagePattern) -> Vec<Symbol> {
    match pattern {
        ast::MessagePattern::Unary { .. } => vec![],
        ast::MessagePattern::Binary { param, .. } => vec![Symbol::intern(&param.name)],
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(_, param)| Symbol::intern(&param.name))
            .collect(),
    }
}

fn member_name_of_message(message: &ast::MessageSend) -> Symbol {
    match &message.selector {
        ast::Selector::Unary(name) => Symbol::intern(name),
        ast::Selector::Binary(name) => Symbol::intern(name),
        ast::Selector::Keyword(full) => {
            // `full` is the whole selector ("at:put:"); §5.7 looks a
            // keyword send up under its first keyword ("at").
            let first = full.split(':').next().unwrap_or(full.as_str());
            Symbol::intern(first)
        }
    }
}
```

If `ast::Selector::Keyword` stores the full colon-joined selector as a
single `String` (confirm against `crates/ns-syntax/src/ast.rs` — the frontend
plan's Task 6 wrote `Keyword(String), // full selector with colons, e.g.
"at:put:"`), `member_name_of_message`'s `split(':')` approach above is
correct as written; if the shipped type differs, adapt the extraction to
whatever shape `Selector::Keyword` actually has, keeping the same semantics
("first keyword component, no trailing colon").

Update `crates/ns-runtime/src/lib.rs`:

```rust
pub mod resolver;
pub mod symbol;
pub mod types;

pub use resolver::resolve;
pub use symbol::Symbol;
pub use types::*;
```

- [x] **Step 6: Integration tests over real small modules**

Create `crates/ns-runtime/tests/resolver.rs`:

```rust
use ns_runtime::{resolve, Binding, SourceOrigin};

fn resolve_source(src: &str) -> ns_runtime::ResolvedModule {
    let module = ns_syntax::parse(src).expect("parse");
    resolve(module, src.into(), SourceOrigin::File("test.ns".into()))
        .expect("resolve")
}

#[test]
fn method_temp_resolves_as_local() {
    let resolved = resolve_source(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  bar = ( | x | x )\n) : ()",
    );
    // At least one Local binding exists somewhere in the module (the `x`
    // reference in the method body).
    assert!(resolved
        .bindings
        .values()
        .any(|b| matches!(b, Binding::Local(_))));
}

#[test]
fn self_send_resolves_to_depth_zero() {
    let resolved = resolve_source(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  bar = ( ^self baz )\n  baz = ( ^1 )\n) : ()",
    );
    assert!(resolved
        .bindings
        .values()
        .any(|b| matches!(b, Binding::LexicalSend { depth: 0 })));
}

#[test]
fn super_send_resolves_to_super_send_binding() {
    let resolved = resolve_source(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  bar = ( ^super baz )\n) : ()",
    );
    assert!(resolved.bindings.values().any(|b| *b == Binding::SuperSend));
}

#[test]
fn outer_named_send_resolves_to_the_exact_depth() {
    let resolved = resolve_source(
        "Newspeak3\n'T'\nclass Outer = Object new (\n) (\n  outerMethod = ( ^1 )\n  public class Inner = Object new (\n  ) (\n    innerMethod = ( ^outer Outer outerMethod )\n  ) : ()\n) : ()",
    );
    assert!(resolved
        .bindings
        .values()
        .any(|b| matches!(b, Binding::LexicalSend { depth: 1 })));
}

#[test]
fn undeclared_outer_class_name_is_a_resolve_error() {
    let module = ns_syntax::parse(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  bar = ( ^outer Missing baz )\n) : ()",
    )
    .expect("parse");
    let result = resolve(module, "src".into(), SourceOrigin::File("test.ns".into()));
    assert!(result.is_err());
}
```

Adjust the four-space/exact-syntax details above to match whatever concrete
`.ns` snippets actually parse cleanly against the shipped `ns-syntax`
parser — run each snippet through `cargo run -p ns-cli -- parse --ast`
first if a test fails to parse, and fix the snippet, not the resolver, for
any pure-syntax mismatch.

- [x] **Step 7: Run everything, fix, and verify green**

Run: `cargo test -p ns-runtime`
Expected: all `resolver::` unit tests plus the five integration tests pass.

Run: `cargo fmt --all -- --check` and `cargo clippy --all-targets`
Expected: both clean.

- [x] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(runtime): implement the resolver (locals, member sets, send-shape recognition)

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 5: Dispatch — kth-enclosing-object, lookupPublic, lookupProtected, DNU

**Read the runtime spec's Dispatch section in full first** — this task
transcribes it. Cites: `newspeak-spec.pdf` §3.3, §5.5, §5.9, §5.10, §6.3.1.

**Files:** Create `crates/ns-runtime/src/dispatch.rs`; modify `lib.rs`.

**Interfaces:**
- Consumes: `types::*` (Task 3).
- Produces: `pub enum Lookup { Method(Rc<ast::MethodDecl>, ClassRef), Native(BuiltinClass), NotFound }`,
  `pub fn kth_enclosing_object(self_value: &Value, depth: u16) -> Value`,
  `pub fn lookup_public(class: &ClassRef, selector: Symbol) -> Lookup`,
  `pub fn lookup_protected(class: &ClassRef, selector: Symbol) -> Lookup`,
  `pub fn lookup_defined_for(class: &ClassRef, selector: Symbol) -> Lookup` (the
  plain, access-blind walk `super`-sends and `doesNotUnderstand:` both use),
  `pub fn effective_access(access: Option<ast::AccessModifier>) -> ast::AccessModifier`
  (defaults `None` to `Protected`, spec §6.3.1). Task 6 (Interpreter) is this
  module's only consumer.

**Design notes:**

- **Milestone-1 simplification of `kth_enclosing_object`:** the spec's full
  §3.3 definition matches mixin applications to handle a class subclassed
  *across* a nesting boundary precisely. This task implements the common
  case — walk `class.enclosing` `depth` times, starting from `self_value`
  — which coincides with the full definition whenever nested classes are
  not themselves subclassed across an enclosing boundary. Flag this in a
  code comment; it is not expected to matter for the M1 corpus, and is a
  candidate to revisit if a real module needs the general case.
- **Builtin classes are dispatch leaves for this module.** When a lookup
  reaches a class tagged `builtin: Some(kind)`, it returns `Lookup::Native(kind)`
  immediately rather than continuing to search `methods` — Tasks 7-9's
  `builtins::dispatch_native` is responsible for that builtin's *entire*
  method surface, including the handful of Object-inherited selectors each
  one needs (`printString`, `=`, `==`, `class`, `hash`), so it never needs to
  fall back into this module's generic walk. This avoids a circular
  `dispatch.rs` ↔ `builtins/` dependency and is simple because M1's core
  library is small and fixed.

- [x] **Step 1: Write the file**

```rust
//! The three lookup algorithms and the kth-enclosing-object computation.
//! See the runtime spec's Dispatch section; cites `newspeak-spec.pdf`
//! §3.3, §5.5, §5.9, §5.10, §6.3.1.

use std::rc::Rc;

use ns_syntax::ast;

use crate::types::*;
use crate::Symbol;

pub enum Lookup {
    Method(Rc<ast::MethodDecl>, ClassRef),
    Native(BuiltinClass),
    NotFound,
}

pub fn effective_access(access: Option<ast::AccessModifier>) -> ast::AccessModifier {
    access.unwrap_or(ast::AccessModifier::Protected)
}

/// §3.3, simplified for Milestone 1 -- see the task's design note.
pub fn kth_enclosing_object(self_value: &Value, depth: u16) -> Value {
    let mut current = self_value.clone();
    for _ in 0..depth {
        let Value::Obj(obj) = &current else {
            return Value::Nil;
        };
        let enclosing = {
            let borrowed = obj.borrow();
            let ObjectData::Instance(_) | ObjectData::Class(_) = &borrowed.data else {
                return Value::Nil;
            };
            class_enclosing(&borrowed.class)
        };
        match enclosing {
            Some(next) => current = Value::Obj(next),
            None => return Value::Nil,
        }
    }
    current
}

fn class_enclosing(class: &ClassRef) -> Option<ObjRef> {
    let borrowed = class.borrow();
    match &borrowed.data {
        ObjectData::Class(data) => data.enclosing.clone(),
        _ => None,
    }
}

/// §5.5: ordinary (explicit-receiver) sends. A `protected`/`private`
/// declaration shadows an inherited `public` one -- the search stops at
/// the first declaration found, matching or not.
pub fn lookup_public(class: &ClassRef, selector: Symbol) -> Lookup {
    walk(class, selector, |access| access == ast::AccessModifier::Public, true)
}

/// §5.9 (non-`private` half of a `LexicalSend`): matches `public` or
/// `protected`; a `private` declaration is transparently skipped, the walk
/// continues past it.
pub fn lookup_protected(class: &ClassRef, selector: Symbol) -> Lookup {
    walk(
        class,
        selector,
        |access| access != ast::AccessModifier::Private,
        false,
    )
}

/// The plain, access-blind "defined for" walk (§3.5): used by `super`
/// sends (§5.10) and by `doesNotUnderstand:` resolution, which is exempt
/// from every access check (§5.5).
pub fn lookup_defined_for(class: &ClassRef, selector: Symbol) -> Lookup {
    walk(class, selector, |_| true, false)
}

/// `hard_stop_on_mismatch`: `lookupPublic`'s distinguishing rule -- when
/// the mixin declares `selector` at all but the access predicate rejects
/// it, stop searching (return `NotFound`) instead of continuing up.
/// `lookupProtected`/`lookup_defined_for` never hard-stop: a declaration
/// that doesn't match the predicate is transparently skipped.
fn walk(
    class: &ClassRef,
    selector: Symbol,
    accept: impl Fn(ast::AccessModifier) -> bool,
    hard_stop_on_mismatch: bool,
) -> Lookup {
    let mut current = Some(class.clone());
    while let Some(c) = current {
        let (method, builtin, superclass) = {
            let borrowed = c.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => (
                    data.methods.get(&selector).cloned(),
                    data.builtin,
                    data.superclass.clone(),
                ),
                _ => return Lookup::NotFound,
            }
        };
        if let Some(method) = method {
            let access = effective_access(access_of(&method));
            return if accept(access) {
                Lookup::Method(method, c)
            } else if hard_stop_on_mismatch {
                Lookup::NotFound
            } else {
                current = superclass;
                continue;
            };
        }
        if let Some(kind) = builtin {
            return Lookup::Native(kind);
        }
        current = superclass;
    }
    Lookup::NotFound
}

fn access_of(method: &ast::MethodDecl) -> Option<ast::AccessModifier> {
    method.access
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn class_with_method(
        name: &str,
        selector: &str,
        access: Option<ast::AccessModifier>,
        superclass: Option<ClassRef>,
    ) -> ClassRef {
        // Build a minimal MethodDecl via ns_syntax::parse of a throwaway
        // module and pull the method out of it -- avoids hand-constructing
        // deeply nested AST literals here.
        let src = format!(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  {access_kw}{selector} = ( ^1 )\n) : ()",
            access_kw = access.map(|a| format!("{a:?} ").to_lowercase()).unwrap_or_default(),
        );
        let module = ns_syntax::parse(&src).expect("parse");
        let method = module.root_class.instance_side.methods[0].clone();
        let root_of_class = Rc::new(RefCell::new(Object {
            class: unreachable_class(),
            identity_hash: 0,
            data: ObjectData::Instance(vec![]),
        }));
        let mut methods = HashMap::new();
        methods.insert(Symbol::intern(selector.trim_end_matches(':')), Rc::new(method));
        Rc::new(RefCell::new(Object {
            class: root_of_class,
            identity_hash: 0,
            data: ObjectData::Class(ClassData {
                name: Symbol::intern(name),
                superclass,
                methods,
                instance_slots: vec![],
                class_side: None,
                enclosing: None,
                factory: None,
                builtin: None,
            }),
        }))
    }

    fn unreachable_class() -> ClassRef {
        Rc::new(RefCell::new(Object {
            class: unreachable!("not read in these tests"),
            identity_hash: 0,
            data: ObjectData::Instance(vec![]),
        }))
    }

    #[test]
    fn lookup_public_finds_a_public_method() {
        let class = class_with_method("X", "foo", Some(ast::AccessModifier::Public), None);
        assert!(matches!(
            lookup_public(&class, Symbol::intern("foo")),
            Lookup::Method(..)
        ));
    }

    #[test]
    fn lookup_public_hard_stops_on_a_protected_declaration() {
        let base = class_with_method("Base", "foo", Some(ast::AccessModifier::Public), None);
        let derived = class_with_method(
            "Derived",
            "foo",
            Some(ast::AccessModifier::Protected),
            Some(base),
        );
        assert!(matches!(
            lookup_public(&derived, Symbol::intern("foo")),
            Lookup::NotFound
        ));
    }

    #[test]
    fn lookup_protected_skips_a_private_declaration_transparently() {
        let base = class_with_method("Base", "foo", Some(ast::AccessModifier::Public), None);
        let derived = class_with_method(
            "Derived",
            "foo",
            Some(ast::AccessModifier::Private),
            Some(base),
        );
        // lookup_protected on Derived's own `foo` (private) must not match,
        // but must continue to Base's public `foo`.
        assert!(matches!(
            lookup_protected(&derived, Symbol::intern("foo")),
            Lookup::Method(..)
        ));
    }

    #[test]
    fn default_access_is_protected() {
        assert_eq!(effective_access(None), ast::AccessModifier::Protected);
    }
}
```

If `ast::AccessModifier` does not derive `Debug`/formatting the way
`class_with_method`'s throwaway-source trick above assumes, replace that
helper with direct `ast::MethodDecl` construction instead (whatever is less
code against the actual shipped `ast` types — check
`crates/ns-syntax/src/ast.rs` before choosing).

Update `crates/ns-runtime/src/lib.rs`:

```rust
pub mod dispatch;
pub mod resolver;
pub mod symbol;
pub mod types;

pub use resolver::resolve;
pub use symbol::Symbol;
pub use types::*;
```

- [x] **Step 2: Run, verify pass**

Run: `cargo test -p ns-runtime dispatch::`
Expected: PASS (4 tests).

- [x] **Step 3: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): implement lookupPublic/lookupProtected/defined-for dispatch

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 6: Interpreter core — expression evaluation, sends, blocks, non-local return

**Read the runtime spec's Activation Records section again, and
`newspeak-spec.pdf` §7.2 (Return Statements) and §5.1.8 (Closure Literals)
before starting.**

**Files:** Create `crates/ns-runtime/src/interpreter/mod.rs`,
`crates/ns-runtime/src/interpreter/eval.rs`; modify `lib.rs`.

**Interfaces:**
- Consumes: `types::*`, `dispatch::*` (Tasks 3, 5).
- Produces: `pub struct Interpreter { .. }`, `pub enum Signal { Return(FrameRef, Value), Error(NewspeakError) }`,
  `pub type EvalResult = Result<Value, Signal>`,
  `Interpreter::send(&mut self, receiver: Value, selector: Symbol, args: Vec<Value>, sender: Option<FrameRef>) -> EvalResult`
  (ordinary send, `lookup_public`),
  `Interpreter::invoke_method(&mut self, receiver: Value, method: Rc<ast::MethodDecl>, defining_class: ClassRef, args: Vec<Value>, bindings: Rc<HashMap<Span, Binding>>, sender: Option<FrameRef>) -> EvalResult`.
  Task 7 (Class Instantiation) and Tasks 8-10 (builtins) call `send` and
  `invoke_method`; Task 11 (CLI) calls `send` for the entry message.

**Design notes:**

- **Non-local return uses `Signal`, not a Rust panic.** A `^expr` inside a
  closure targets the enclosing *method* activation (§7.2), not its own
  closure activation. `Signal::Return(target_frame, value)` unwinds through
  ordinary `Result`/`?` propagation; the function that originally pushed
  `target_frame` (`invoke_method`) is the only place that converts
  `Err(Signal::Return(f, v))` back into `Ok(v)`, by checking
  `Rc::ptr_eq(&f, &this_frame)`; every other caller just propagates the
  `Err` unchanged.
- **`uncontinuable` implements §3.6/§7.2's re-return guard.** Before raising
  a `Signal::Return`, check whether the target frame is already
  `uncontinuable`; if so, that is the spec's `cannotReturn` case (an escaping
  block invoked after its home method already returned) — produce a
  `NewspeakError`, not another `Signal::Return`.
- **`LexicalSend`/`SuperSend` dispatch belongs here, not in `dispatch.rs`.**
  `dispatch.rs` only computes *which method* to run; this task's `eval.rs`
  is what reads a `Binding` off a `Send` node, computes the `kth_enclosing_object`
  or the current frame's `current_class`, calls the right `lookup_*`
  function, and actually invokes the result — because doing so needs
  `Interpreter::invoke_method`, which `dispatch.rs` deliberately does not
  depend on (Task 5's module has no `Interpreter` type to reference).

- [x] **Step 1: `interpreter/mod.rs` — the `Interpreter`, `send`, `invoke_method`**

```rust
//! The tree-walking interpreter's core: sends, method invocation, and the
//! `Signal` non-local-return/error channel. Expression-level evaluation is
//! in `eval.rs`.

pub mod eval;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::dispatch::{self, Lookup};
use crate::types::*;
use crate::Symbol;

pub enum Signal {
    Return(FrameRef, Value),
    Error(NewspeakError),
}

pub type EvalResult = Result<Value, Signal>;

pub struct Interpreter {
    next_identity_hash: u32,
}

impl Interpreter {
    pub fn new() -> Interpreter {
        Interpreter {
            next_identity_hash: 1,
        }
    }

    pub fn alloc_identity_hash(&mut self) -> u32 {
        let h = self.next_identity_hash;
        self.next_identity_hash += 1;
        h
    }

    /// An ordinary (explicit-receiver) send: `newspeak-spec.pdf` §5.5.
    pub fn send(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let class = self.class_of(&receiver);
        match dispatch::lookup_public(&class, selector) {
            Lookup::Method(method, defining_class) => {
                self.invoke_method(receiver, method, defining_class, args, bindings.clone(), sender)
            }
            Lookup::Native(kind) => self.dispatch_native_or_dnu(kind, receiver, selector, args, bindings, sender),
            Lookup::NotFound => self.does_not_understand(receiver, selector, args, bindings, sender),
        }
    }

    fn dispatch_native_or_dnu(
        &mut self,
        kind: BuiltinClass,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        match crate::builtins::dispatch_native(self, kind, receiver.clone(), selector, &args) {
            Some(result) => result.map_err(Signal::Error),
            None => self.does_not_understand(receiver, selector, args, bindings, sender),
        }
    }

    /// §5.5's `doesNotUnderstand:` fallback -- access-blind (`lookup_defined_for`),
    /// per the runtime spec's Dispatch section.
    fn does_not_understand(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let class = self.class_of(&receiver);
        let dnu_selector = Symbol::intern("doesNotUnderstand:");
        match dispatch::lookup_defined_for(&class, dnu_selector) {
            Lookup::Method(method, defining_class) => {
                // Milestone 1 passes a minimal message-mirror-shaped value:
                // an Array `{selector. args}` rather than a real MessageMirror
                // object (mirrors are a later milestone's Non-Goal). Callers
                // relying on a real `MessageMirror` API are out of scope.
                let message_arg = Value::Obj(self.new_array(vec![
                    self.new_symbol(selector),
                    self.new_array(args),
                ]));
                self.invoke_method(
                    receiver,
                    method,
                    defining_class,
                    vec![message_arg],
                    bindings.clone(),
                    sender,
                )
            }
            _ => Err(Signal::Error(NewspeakError {
                selector,
                receiver,
                message: format!("doesNotUnderstand: {selector} (and no doesNotUnderstand: found -- Object's builtin default is missing; see Task 7)"),
            })),
        }
    }

    /// Creates a `Frame`, runs the method body, and unwinds a matching
    /// `Signal::Return` back into an ordinary value.
    pub fn invoke_method(
        &mut self,
        receiver: Value,
        method: Rc<ast::MethodDecl>,
        defining_class: ClassRef,
        args: Vec<Value>,
        bindings: Rc<HashMap<Span, Binding>>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let temp_count = method.body.temps.len();
        let frame = Rc::new(RefCell::new(Frame {
            receiver,
            args,
            temps: vec![Value::Nil; temp_count],
            pc: 0,
            sender,
            home: None,
            current_class: Some(defining_class),
            kind: FrameKind::Method(method.clone()),
            uncontinuable: false,
        }));
        let stmts = &method.body.stmts;
        match self.run_statements(stmts, &frame, &bindings) {
            Ok(value) => Ok(value),
            Err(Signal::Return(target, value)) if Rc::ptr_eq(&target, &frame) => Ok(value),
            Err(other) => Err(other),
        }
    }

    fn run_statements(
        &mut self,
        stmts: &[ast::Stmt],
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> EvalResult {
        let mut last = Value::Nil;
        for (i, stmt) in stmts.iter().enumerate() {
            frame.borrow_mut().pc = i;
            match stmt {
                ast::Stmt::Expr(e) => {
                    last = eval::eval_expr(self, e, frame, bindings)?;
                }
                ast::Stmt::Return(e) => {
                    let value = eval::eval_expr(self, e, frame, bindings)?;
                    return Err(self.raise_return(frame, value));
                }
            }
        }
        // No explicit return: methods return `self` (spec §3.5), closures
        // return the last expression's value (§5.1.8). `eval.rs`'s block
        // invocation handles the closure case directly and never calls
        // `run_statements` through this method-shaped path for a closure
        // body's implicit-return value; see Task 6 Step 2.
        match &frame.borrow().kind {
            FrameKind::Method(_) => Ok(frame.borrow().receiver.clone()),
            _ => Ok(last),
        }
    }

    fn raise_return(&self, frame: &FrameRef, value: Value) -> Signal {
        let target = match &frame.borrow().kind {
            FrameKind::Method(_) | FrameKind::TopLevel => frame.clone(),
            FrameKind::Closure(closure) => closure.home.clone(),
        };
        if target.borrow().uncontinuable {
            return Signal::Error(NewspeakError {
                selector: Symbol::intern("^"),
                receiver: value,
                message: "cannotReturn: the enclosing method activation has already returned"
                    .to_string(),
            });
        }
        target.borrow_mut().uncontinuable = true;
        Signal::Return(target, value)
    }

    pub fn class_of(&self, value: &Value) -> ClassRef {
        match value {
            Value::Obj(obj) => obj.borrow().class.clone(),
            // Nil/Bool/SmallInt/Float classes are wired up by the bootstrap
            // (Task 11) into interpreter-held globals; Task 7-10 read them
            // via `Interpreter::builtin_class(kind)` (added in Task 7).
            _ => self.builtin_class_for_primitive(value),
        }
    }
}
```

`builtin_class_for_primitive`, `builtin_class`, `new_array`, and
`new_symbol` are added in Task 7 (they need the bootstrap's globals table,
introduced there) — leave them as `todo!()`-free forward declarations is
not possible in Rust, so **do Task 6 Step 1 and Task 7 Step 1 as one
continuous edit** if `cargo build` complains about missing methods; the
task boundary here is for review purposes (this task's own tests, Step 3
below, construct classes/frames directly and never call `class_of` on a
primitive `Value`, so they do not require those methods to exist yet if the
crate does not compile them until Task 7 adds their bodies). If your
toolchain requires the crate to compile *now*, add minimal stub bodies:

```rust
impl Interpreter {
    fn builtin_class_for_primitive(&self, _value: &Value) -> ClassRef {
        unimplemented!("wired up in Task 7's bootstrap")
    }
    pub fn new_array(&mut self, _elements: Vec<Value>) -> ObjRef {
        unimplemented!("wired up in Task 7's bootstrap")
    }
    pub fn new_symbol(&mut self, _s: Symbol) -> Value {
        unimplemented!("wired up in Task 7's bootstrap")
    }
}
```

and delete these stub bodies in Task 7 Step 1 when the real ones land.

- [x] **Step 2: `interpreter/eval.rs` — expression evaluation**

```rust
//! Expression-level evaluation: literals, sends (ordinary, `LexicalSend`,
//! `SuperSend`), cascades, blocks, assignment.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr};
use ns_syntax::Span;

use crate::dispatch::{self, Lookup};
use crate::types::*;
use crate::Symbol;

use super::{EvalResult, Interpreter, Signal};

pub fn eval_expr(
    interp: &mut Interpreter,
    expr: &Expr,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    match expr {
        Expr::Ident(id) => eval_ident(interp, id, frame, bindings),
        Expr::Literal(lit) => eval_literal(interp, lit),
        Expr::Send(send) => eval_send(interp, send, frame, bindings),
        Expr::Cascade(cascade) => eval_cascade(interp, cascade, frame, bindings),
        Expr::Assign { target, value, .. } => eval_assign(interp, target, value, frame, bindings),
        Expr::Block(block) => eval_block_literal(interp, block, frame, bindings),
        Expr::DynamicArray { elements, .. } => {
            let mut values = Vec::with_capacity(elements.len());
            for e in elements {
                values.push(eval_expr(interp, e, frame, bindings)?);
            }
            Ok(Value::Obj(interp.new_array(values)))
        }
    }
}

fn eval_ident(
    interp: &mut Interpreter,
    id: &ast::Ident,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    if id.name == "self" {
        return Ok(frame.borrow().receiver.clone());
    }
    match bindings.get(&id.span) {
        Some(Binding::Local(i)) => Ok(read_local(frame, *i)),
        Some(Binding::OuterLocal { depth, index }) => Ok(read_outer_local(frame, *depth, *index)),
        Some(Binding::LexicalSend { depth }) => {
            dispatch_lexical_send(interp, frame, *depth, Symbol::intern(&id.name), vec![], bindings)
        }
        Some(Binding::SuperSend) | None => Err(Signal::Error(NewspeakError {
            selector: Symbol::intern(&id.name),
            receiver: frame.borrow().receiver.clone(),
            message: format!("internal: unresolved identifier `{}`", id.name),
        })),
    }
}

fn read_local(frame: &FrameRef, index: u16) -> Value {
    let borrowed = frame.borrow();
    let i = index as usize;
    if i < borrowed.args.len() {
        borrowed.args[i].clone()
    } else {
        borrowed.temps[i - borrowed.args.len()].clone()
    }
}

fn write_local(frame: &FrameRef, index: u16, value: Value) {
    let mut borrowed = frame.borrow_mut();
    let i = index as usize;
    if i < borrowed.args.len() {
        borrowed.args[i] = value;
    } else {
        borrowed.temps[i - borrowed.args.len()] = value;
    }
}

fn frame_at_depth(frame: &FrameRef, depth: u16) -> FrameRef {
    let mut current = frame.clone();
    for _ in 0..depth {
        let next = match &current.borrow().kind {
            FrameKind::Closure(closure) => closure.captured_frame.clone(),
            _ => current.clone(),
        };
        current = next;
    }
    current
}

fn read_outer_local(frame: &FrameRef, depth: u16, index: u16) -> Value {
    read_local(&frame_at_depth(frame, depth), index)
}

fn write_outer_local(frame: &FrameRef, depth: u16, index: u16, value: Value) {
    write_local(&frame_at_depth(frame, depth), index, value);
}

fn eval_literal(interp: &mut Interpreter, lit: &ast::Literal) -> EvalResult {
    Ok(match lit {
        ast::Literal::Int { text, .. } => {
            let n: i64 = text.parse().map_err(|_| {
                Signal::Error(NewspeakError {
                    selector: Symbol::intern("parse"),
                    receiver: Value::Nil,
                    message: format!("integer literal out of i64 range: {text}"),
                })
            })?;
            Value::SmallInt(n)
        }
        ast::Literal::Float { text, .. } => {
            let n: f64 = text.parse().unwrap_or(f64::NAN);
            Value::Float(n)
        }
        ast::Literal::String { value, .. } => Value::Obj(interp.new_string(value.clone())),
        ast::Literal::Symbol { value, .. } => interp.new_symbol(Symbol::intern(value)),
        ast::Literal::Char { value, .. } => Value::Obj(interp.new_character(*value)),
        ast::Literal::Bool { value, .. } => Value::Bool(*value),
        ast::Literal::Nil { .. } => Value::Nil,
        ast::Literal::Array { elements, .. } => {
            let mut values = Vec::with_capacity(elements.len());
            for e in elements {
                let ev = eval_literal(interp, e)?;
                values.push(ev);
            }
            Value::Obj(interp.new_array(values))
        }
        ast::Literal::Pattern { .. } => Value::Nil, // Non-Goal: typechecker/patterns (M1 spec)
    })
}

fn eval_send(
    interp: &mut Interpreter,
    send: &ast::Send,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    match bindings.get(&send.span) {
        Some(Binding::SuperSend) => {
            let args = eval_args(interp, &send.message, frame, bindings)?;
            dispatch_super_send(interp, frame, selector_of(&send.message), args, bindings)
        }
        Some(Binding::LexicalSend { depth }) => {
            let args = eval_args(interp, &send.message, frame, bindings)?;
            dispatch_lexical_send(interp, frame, *depth, selector_of(&send.message), args, bindings)
        }
        _ => {
            let receiver = eval_expr(interp, &send.receiver, frame, bindings)?;
            let args = eval_args(interp, &send.message, frame, bindings)?;
            interp.send(receiver, selector_of(&send.message), args, bindings, Some(frame.clone()))
        }
    }
}

fn eval_args(
    interp: &mut Interpreter,
    message: &ast::MessageSend,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Result<Vec<Value>, Signal> {
    let mut values = Vec::with_capacity(message.args.len());
    for a in &message.args {
        values.push(eval_expr(interp, a, frame, bindings)?);
    }
    Ok(values)
}

fn selector_of(message: &ast::MessageSend) -> Symbol {
    match &message.selector {
        ast::Selector::Unary(s) | ast::Selector::Binary(s) => Symbol::intern(s),
        ast::Selector::Keyword(s) => Symbol::intern(s),
    }
}

/// `newspeak-spec.pdf` §5.7-§5.9: `o` = the kth enclosing object of `self`;
/// dispatch privately (direct, non-virtual) if the target is `private` at
/// depth `k`, otherwise virtually via `lookup_protected` on `o`'s actual
/// class.
fn dispatch_lexical_send(
    interp: &mut Interpreter,
    frame: &FrameRef,
    depth: u16,
    selector: Symbol,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let self_value = frame.borrow().receiver.clone();
    let o = dispatch::kth_enclosing_object(&self_value, depth);
    let class = interp.class_of(&o);
    // Check for a private declaration *at exactly this class* first (the
    // non-virtual case -- private members are never overridden, so there
    // is no search: newspeak-spec.pdf §5.9).
    if let Lookup::Method(method, defining_class) = dispatch::lookup_defined_for(&class, selector) {
        if dispatch::effective_access(method.access) == ast::AccessModifier::Private {
            return interp.invoke_method(o, method, defining_class, args, bindings.clone(), Some(frame.clone()));
        }
    }
    match dispatch::lookup_protected(&class, selector) {
        Lookup::Method(method, defining_class) => {
            interp.invoke_method(o, method, defining_class, args, bindings.clone(), Some(frame.clone()))
        }
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(interp, kind, o.clone(), selector, &args) {
                Some(result) => result.map_err(Signal::Error),
                None => interp.send(o, selector, args, bindings, Some(frame.clone())),
            }
        }
        Lookup::NotFound => interp.send(o, selector, args, bindings, Some(frame.clone())),
    }
}

/// `newspeak-spec.pdf` §5.10: starts at the superclass of the *current
/// activation's* `current_class`, not `self`'s own class. On success, the
/// new activation's `current_class` is `definingClass(selector, superclass)`
/// -- `invoke_method`'s `defining_class` argument (from `lookup_defined_for`)
/// already carries that.
fn dispatch_super_send(
    interp: &mut Interpreter,
    frame: &FrameRef,
    selector: Symbol,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let current_class = frame
        .borrow()
        .current_class
        .clone()
        .expect("super send inside a frame with no current_class (top-level?)");
    let superclass = {
        let borrowed = current_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => data.superclass.clone(),
            _ => None,
        }
    };
    let Some(superclass) = superclass else {
        return Err(Signal::Error(NewspeakError {
            selector,
            receiver: frame.borrow().receiver.clone(),
            message: "super send with no superclass (current class is Object or Top)".to_string(),
        }));
    };
    let self_value = frame.borrow().receiver.clone();
    match dispatch::lookup_defined_for(&superclass, selector) {
        Lookup::Method(method, defining_class) => interp.invoke_method(
            self_value,
            method,
            defining_class,
            args,
            bindings.clone(),
            Some(frame.clone()),
        ),
        Lookup::Native(kind) => {
            match crate::builtins::dispatch_native(interp, kind, self_value.clone(), selector, &args) {
                Some(result) => result.map_err(Signal::Error),
                None => interp.send(self_value, selector, args, bindings, Some(frame.clone())),
            }
        }
        Lookup::NotFound => interp.send(self_value, selector, args, bindings, Some(frame.clone())),
    }
}

fn eval_cascade(
    interp: &mut Interpreter,
    cascade: &ast::Cascade,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let receiver = eval_expr(interp, &cascade.receiver, frame, bindings)?;
    let mut result = interp.send(
        receiver.clone(),
        selector_of(&cascade.first),
        eval_args(interp, &cascade.first, frame, bindings)?,
        bindings,
        Some(frame.clone()),
    )?;
    for m in &cascade.rest {
        result = interp.send(
            receiver.clone(),
            selector_of(m),
            eval_args(interp, m, frame, bindings)?,
            bindings,
            Some(frame.clone()),
        )?;
    }
    Ok(result)
}

/// `id:: e` (`Expr::Assign`): resolves and dispatches exactly like an
/// implicit-receiver keyword send to `id:`, per the runtime spec's "Setter
/// sends" note, then yields `e`'s value (not the setter's own return
/// value, per §6.3.2/§5.4.3).
fn eval_assign(
    interp: &mut Interpreter,
    target: &ast::Ident,
    value: &Expr,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let v = eval_expr(interp, value, frame, bindings)?;
    match bindings.get(&target.span) {
        Some(Binding::Local(i)) => write_local(frame, *i, v.clone()),
        Some(Binding::OuterLocal { depth, index }) => write_outer_local(frame, *depth, *index, v.clone()),
        Some(Binding::LexicalSend { depth }) => {
            let setter = Symbol::intern(&format!("{}:", target.name));
            dispatch_lexical_send(interp, frame, *depth, setter, vec![v.clone()], bindings)?;
        }
        _ => {
            return Err(Signal::Error(NewspeakError {
                selector: Symbol::intern(&target.name),
                receiver: frame.borrow().receiver.clone(),
                message: format!("internal: unresolved setter target `{}`", target.name),
            }))
        }
    }
    Ok(v)
}

fn eval_block_literal(
    interp: &mut Interpreter,
    block: &ast::BlockExpr,
    frame: &FrameRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let home = match &frame.borrow().kind {
        FrameKind::Method(_) | FrameKind::TopLevel => frame.clone(),
        FrameKind::Closure(closure) => closure.home.clone(),
    };
    let closure = Rc::new(ClosureData {
        params: block.params.iter().map(|p| Symbol::intern(&p.name)).collect(),
        body: Rc::new(block.body.clone()),
        captured_frame: frame.clone(),
        home,
    });
    Ok(Value::Obj(interp.new_closure(closure)))
}

/// Invokes a closure with `args`. Called by `builtins::block` (Task 10) for
/// `value`, `value:`, `value:value:`, etc. Public because block-invocation
/// selectors are Milestone 1 *native* methods on `BuiltinClass::Block`, not
/// user-overridable Newspeak methods.
pub fn invoke_closure(
    interp: &mut Interpreter,
    closure: &Rc<ClosureData>,
    args: Vec<Value>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let receiver = closure.captured_frame.borrow().receiver.clone();
    let current_class = closure.captured_frame.borrow().current_class.clone();
    let frame = Rc::new(RefCell::new(Frame {
        receiver,
        args,
        temps: vec![Value::Nil; closure.body.temps.len()],
        pc: 0,
        sender: None,
        home: Some(closure.home.clone()),
        current_class,
        kind: FrameKind::Closure(closure.clone()),
        uncontinuable: false,
    }));
    let mut last = Value::Nil;
    for (i, stmt) in closure.body.stmts.iter().enumerate() {
        frame.borrow_mut().pc = i;
        match stmt {
            ast::Stmt::Expr(e) => last = eval_expr(interp, e, &frame, bindings)?,
            ast::Stmt::Return(e) => {
                let value = eval_expr(interp, e, &frame, bindings)?;
                return Err(super::Interpreter::raise_return_pub(interp, &frame, value));
            }
        }
    }
    Ok(last)
}
```

`raise_return_pub` is a thin `pub(crate)` wrapper Step 1's `raise_return`
needs (it is currently a private method) — add this to `interpreter/mod.rs`
in Step 1:

```rust
impl Interpreter {
    pub(crate) fn raise_return_pub(&self, frame: &FrameRef, value: Value) -> Signal {
        self.raise_return(frame, value)
    }
}
```

`interp.new_string`, `new_character`, `new_closure` are added alongside
`new_array`/`new_symbol` in Task 7's bootstrap (Step 1's note above already
flags this) — add matching `unimplemented!()` stubs now if the crate must
compile before Task 7 lands, same as `new_array`/`new_symbol`.

Update `crates/ns-runtime/src/lib.rs`:

```rust
pub mod dispatch;
pub mod interpreter;
pub mod resolver;
pub mod symbol;
pub mod types;

pub use interpreter::{Interpreter, Signal};
pub use resolver::resolve;
pub use symbol::Symbol;
pub use types::*;
```

- [x] **Step 3: Tests over hand-built frames/classes**

Create `crates/ns-runtime/tests/interpreter.rs`. Build a two-method class
(`Object`-superclassed, one method returning a literal, one method that
sends the first via an implicit self-send) directly through
`ns_syntax::parse` + hand-assembled `ClassData`/`Object`, the same pattern
Task 5's `dispatch.rs` tests use, then call `Interpreter::send` directly and
assert on the returned `Value`. Cover at minimum:

- a method with no explicit return returns `self` (spec §3.5)
- a method with `^42` returns `42` as `Value::SmallInt`
- a non-local return from inside a block (`[ ^1 ] value`, called from a
  method) unwinds past the block's own activation to the method's
  continuation
- calling `value` on a block whose home method has already returned
  produces a `NewspeakError` (the `cannotReturn` case), not a panic

Write the actual test bodies against whatever helper-construction pattern
Task 5's `dispatch.rs` tests settled on (reuse/extend those helpers rather
than duplicating them — consider moving `class_with_method` and
`unreachable_class` into a shared `#[cfg(test)]`-only module,
`crates/ns-runtime/tests/support.rs`, included from both test files via
`mod support;`, if duplication becomes noticeable).

- [x] **Step 4: Run, fix, verify green**

Run: `cargo test -p ns-runtime`
Expected: all resolver/dispatch/interpreter tests pass. Some tests will
need `builtins::dispatch_native` (Task 7+) to exist even as a stub —
add `pub fn dispatch_native(_interp: &mut Interpreter, _kind: BuiltinClass, _receiver: Value, _selector: Symbol, _args: &[Value]) -> Option<Result<Value, NewspeakError>> { None }`
in a temporary `crates/ns-runtime/src/builtins/mod.rs` now if needed to
compile this task in isolation, and flesh it out for real starting Task 7.

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): implement the tree-walking interpreter core

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 7: Class Instantiation, the bootstrap, and `Object`/DNU

**Read the runtime spec's Class Instantiation section again before
starting** — this task transcribes its six numbered steps. Cites
`newspeak-spec.pdf` §6.1, §6.3, §5.5 (`doesNotUnderstand:`).

**Files:** Create `crates/ns-runtime/src/interpreter/instantiate.rs`,
`crates/ns-runtime/src/builtins/mod.rs`, `crates/ns-runtime/src/builtins/object_class.rs`;
modify `interpreter/mod.rs`, `lib.rs`.

**Interfaces:**
- Consumes: everything from Tasks 3, 5, 6.
- Produces: `Interpreter::bootstrap() -> Interpreter` (builds `Object` and
  every other `BuiltinClass` as real class objects with a shared,
  self-referential `.class` — resolving Task 3's `unreachable!()` test-only
  placeholder for real), `Interpreter::builtin_class(&self, kind: BuiltinClass) -> ClassRef`,
  `Interpreter::builtin_class_for_primitive`, `Interpreter::new_array`,
  `new_symbol`, `new_string`, `new_character`, `new_closure` (replacing
  Task 6's `unimplemented!()` stubs), `Interpreter::load_module(&mut self,
  resolved: &ResolvedModule) -> ClassRef` (materializes the module's
  `root_class` AST into a real class object, per Class Instantiation step
  "a class object is created... its method dictionaries populated from the
  AST"), `Interpreter::instantiate(&mut self, class: ClassRef, factory_args:
  Vec<Value>, bindings: &Rc<HashMap<Span, Binding>>) -> EvalResult` (the six
  numbered steps).
  `builtins::dispatch_native` gains its first real arm: `BuiltinClass::Object`.

**Design notes:**

- **The bootstrap's identity problem.** Every class object's own `.class`
  field must point at *something*; Task 3's tests used `unreachable!()`
  because nothing yet resolved it for real. Milestone 1 (no metaclasses —
  see the runtime spec's "Documented Deviations") resolves this by giving
  every `BuiltinClass` — including `Object` itself — the **same** shared
  placeholder class object as its own `.class`, tagged
  `BuiltinClass::Object` (i.e. `Object`'s class is `Object` itself,
  circularly, via `Rc::new_cyclic`). This is a deliberate simplification: a
  real metaclass hierarchy (Task 3's "Documented Deviations" reference)
  would give each class a *distinct* metaclass instance; M1 does not need
  that distinction to be observable (no M1 program sends a class-side
  message that depends on metaclass identity).
- **Class instantiation's superclass-clause message is deferred, per the
  runtime spec — implement it as such, not as "look up the superclass once
  at class-creation time and call `new` on it then."** Re-read the spec
  section: the receiver (`Object` in `Object new`) is resolved once when
  `load_module` builds the class object; the trailing message (`new`) is
  evaluated fresh, in the *subclass's own* instance-initializer activation,
  every time `instantiate` runs.
- **One instance object threaded through the whole chain.** `instantiate`
  allocates `o` exactly once and passes the *same* `Value::Obj(o)` down
  through every recursive step — never allocates a second instance for a
  superclass level.

- [x] **Step 1: The bootstrap and object-model helpers (`interpreter/mod.rs`)**

Replace Task 6's `unimplemented!()` stub bodies with:

```rust
impl Interpreter {
    pub fn bootstrap() -> Interpreter {
        let mut interp = Interpreter {
            next_identity_hash: 1,
        };
        interp.builtin_classes = Some(build_builtin_classes(&mut interp));
        interp
    }

    pub fn builtin_class(&self, kind: BuiltinClass) -> ClassRef {
        self.builtin_classes
            .as_ref()
            .expect("bootstrap() must run before builtin_class()")
            .get(&kind)
            .expect("every BuiltinClass variant is registered by build_builtin_classes")
            .clone()
    }

    fn builtin_class_for_primitive(&self, value: &Value) -> ClassRef {
        let kind = match value {
            Value::Nil => BuiltinClass::Nil,
            Value::Bool(true) => BuiltinClass::True,
            Value::Bool(false) => BuiltinClass::False,
            Value::SmallInt(_) => BuiltinClass::Integer,
            Value::Float(_) => BuiltinClass::Float,
            Value::Obj(_) => unreachable!("Value::Obj carries its own .class"),
        };
        self.builtin_class(kind)
    }

    fn new_instance_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        let identity_hash = self.alloc_identity_hash();
        Rc::new(RefCell::new(Object {
            class,
            identity_hash,
            data,
        }))
    }

    pub fn new_array(&mut self, elements: Vec<Value>) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Array);
        self.new_instance_object(class, ObjectData::Array(elements))
    }

    pub fn new_string(&mut self, s: String) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Str);
        self.new_instance_object(class, ObjectData::Str(s))
    }

    pub fn new_symbol(&mut self, s: Symbol) -> Value {
        let class = self.builtin_class(BuiltinClass::Sym);
        Value::Obj(self.new_instance_object(class, ObjectData::Sym(s)))
    }

    pub fn new_character(&mut self, c: char) -> ObjRef {
        // Milestone 1 has no dedicated Character payload variant on
        // ObjectData -- represent a character as a one-`char` Rust String
        // wrapped the same way String literals are (Str), consistent with
        // how the language spec itself treats characters as "a special
        // case of string literals" going forward (`newspeak-spec.pdf`
        // §2.2.3). `builtins::text` (Task 9) tags the *class* as
        // `BuiltinClass::Character` so dispatch still lands on the right
        // native methods; only the payload representation is shared.
        let class = self.builtin_class(BuiltinClass::Character);
        self.new_instance_object(class, ObjectData::Str(c.to_string()))
    }

    pub fn new_closure(&mut self, closure: Rc<ClosureData>) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Block);
        self.new_instance_object(class, ObjectData::Closure((*closure).clone()))
    }
}
```

Add `builtin_classes: Option<HashMap<BuiltinClass, ClassRef>>` to the
`Interpreter` struct (Task 6 Step 1) and initialize it `None` in
`Interpreter::new()`, distinct from `bootstrap()` above — `new()` stays
available for tests that build their own classes by hand (Tasks 3-6's
tests) and never call `builtin_class`.

`build_builtin_classes` (private, same file):

```rust
fn build_builtin_classes(interp: &mut Interpreter) -> HashMap<BuiltinClass, ClassRef> {
    use BuiltinClass::*;
    let kinds = [
        Object, Boolean, True, False, Integer, Float, Str, Sym, Character,
        Array, OrderedCollection, Nil, Block,
    ];
    // Every builtin class's own `.class` is the same self-referential
    // `Object`-tagged placeholder -- see the task's design note on why.
    let class_of_class: ClassRef = Rc::new_cyclic(|weak| {
        RefCell::new(Object {
            class: weak.upgrade().unwrap_or_else(|| {
                // `new_cyclic`'s closure runs before the Rc exists; the
                // first `.class` value is a throwaway that is immediately
                // overwritten below once the real Rc is available.
                Rc::new(RefCell::new(Object {
                    class: unreachable!("overwritten immediately below"),
                    identity_hash: 0,
                    data: ObjectData::Instance(vec![]),
                }))
            }),
            identity_hash: 0,
            data: ObjectData::Class(ClassData {
                name: Symbol::intern("Object"),
                superclass: None,
                methods: HashMap::new(),
                instance_slots: vec![],
                class_side: None,
                enclosing: None,
                factory: None,
                builtin: Some(Object),
            }),
        })
    });
    {
        let real_self = class_of_class.clone();
        class_of_class.borrow_mut().class = real_self;
    }
    let mut map = HashMap::new();
    for kind in kinds {
        let superclass = if kind == Object {
            None
        } else {
            Some(class_of_class.clone())
        };
        let obj = Rc::new(RefCell::new(Object {
            class: class_of_class.clone(),
            identity_hash: interp.alloc_identity_hash(),
            data: ObjectData::Class(ClassData {
                name: Symbol::intern(&format!("{kind:?}")),
                superclass,
                methods: HashMap::new(),
                instance_slots: vec![],
                class_side: None,
                enclosing: None,
                factory: None,
                builtin: Some(kind),
            }),
        }));
        map.insert(kind, obj);
    }
    map.insert(Object, class_of_class);
    map
}
```

Every non-`Object` builtin's superclass above is `Object` directly, a
Milestone 1 simplification of the real class hierarchy (`newspeak-spec.pdf`
places `Boolean` above `True`/`False`, for instance) — flag this in a code
comment; `builtins::boolean` (Task 8) must not rely on `True`/`False`
actually inheriting from a `Boolean` superclass node, since they do not yet.
Fix this only if a corpus module or fixture test needs real
`Boolean`/`True`/`False` inheritance to observe correct behavior.

- [x] **Step 2: Class instantiation (`interpreter/instantiate.rs`)**

```rust
//! Class instantiation: `newspeak-spec.pdf` §6.1, §6.3. See the runtime
//! spec's Class Instantiation section for the six numbered steps this
//! transcribes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::types::*;
use crate::Symbol;

use super::{EvalResult, Interpreter, Signal};

impl Interpreter {
    /// Materializes a `ClassDecl` (and, recursively, its nested classes)
    /// into a real class object. The superclass clause's *receiver* is
    /// resolved here (once); its trailing *message* is stored, deferred,
    /// for `instantiate` to evaluate later -- see the task's design note.
    pub fn load_module(&mut self, resolved: &ResolvedModule) -> ClassRef {
        self.load_class(&resolved.module.root_class, None)
    }

    fn load_class(&mut self, decl: &Rc<ast::ClassDecl>, enclosing: Option<ObjRef>) -> ClassRef {
        let superclass = self.resolve_superclass_receiver(decl);
        let mut methods = HashMap::new();
        for m in &decl.instance_side.methods {
            methods.insert(Symbol::intern(&method_name(&m.pattern)), Rc::new(m.clone()));
        }
        let instance_slots = decl
            .instance_side
            .slots
            .iter()
            .map(|s| Rc::new(s.clone()))
            .collect();
        let identity_hash = self.alloc_identity_hash();
        let class_obj = Rc::new(RefCell::new(Object {
            class: self.builtin_class(BuiltinClass::Object), // M1 simplification, see Task 7 Step 1
            identity_hash,
            data: ObjectData::Class(ClassData {
                name: Symbol::intern(&decl.name.name),
                superclass,
                methods,
                instance_slots,
                class_side: None, // wired below once class-side methods exist
                enclosing,
                factory: Some(Rc::new(decl.factory.clone().unwrap_or(ast::MessagePattern::Unary {
                    selector: ast::Ident { name: "new".to_string(), span: decl.span },
                }))),
                builtin: None,
            }),
        }));
        class_obj
    }

    fn resolve_superclass_receiver(&mut self, decl: &ast::ClassDecl) -> Option<ClassRef> {
        let Some(clause) = &decl.superclass else {
            return Some(self.builtin_class(BuiltinClass::Object));
        };
        // Milestone 1: only the common case (implicit/self prefix naming a
        // builtin or a not-yet-loaded sibling class by name) is handled --
        // full lexical/outer-prefixed superclass receivers that need the
        // resolver's class-nesting chain at *instantiation* time are a
        // known gap; extend here if a fixture needs it.
        if BUILTIN_NAMES.contains(&clause.receiver.name.as_str()) {
            let kind = builtin_kind_from_name(&clause.receiver.name);
            return Some(self.builtin_class(kind));
        }
        None
    }

    /// The six numbered steps in the runtime spec's Class Instantiation
    /// section.
    pub fn instantiate(
        &mut self,
        class: ClassRef,
        factory_args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> EvalResult {
        // Step 1: one instance, every slot nil.
        let slot_count = total_slot_count(&class);
        let instance = self.new_instance_object(class.clone(), ObjectData::Instance(vec![Value::Nil; slot_count]));
        let receiver = Value::Obj(instance);

        // Step 2: the instance-initializer activation.
        let frame = Rc::new(RefCell::new(Frame {
            receiver: receiver.clone(),
            args: factory_args,
            temps: vec![],
            pc: 0,
            sender: None,
            home: None,
            current_class: Some(class.clone()),
            kind: FrameKind::TopLevel, // an initializer is its own kind of top-of-chain activation for M1
            uncontinuable: false,
        }));

        // Step 3: run the superclass chain first, deepest (Object) first --
        // implemented as recursion that runs *before* this class's own
        // slots/initExprs, per the spec ("execution of the instance
        // initializer begins with evaluation of the superclass clause...
        // the superclass' instance initializer is then invoked on o").
        self.run_superclass_initializer(&class, &receiver, &frame, bindings)?;

        // Step 4: this class's own slot clauses, in declaration order.
        self.run_own_slots(&class, &receiver, &frame, bindings)?;

        // Step 5: this class's own initExprs.
        self.run_own_init_exprs(&class, &frame, bindings)?;

        // Step 6.
        Ok(receiver)
    }

    fn run_superclass_initializer(
        &mut self,
        class: &ClassRef,
        instance: &Value,
        subclass_frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        let superclass = {
            let borrowed = class.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => data.superclass.clone(),
                _ => None,
            }
        };
        let Some(superclass) = superclass else {
            return Ok(()); // reached Object (or a builtin) -- native classes have no init_exprs to run
        };
        // The deferred factory message, evaluated now, in the *subclass's*
        // own initializer context (subclass_frame) so its factory
        // parameters are in scope -- the spec's core correction from the
        // earlier draft of this design.
        // Milestone 1: the superclass clause's message is not carried on
        // ClassData today (Step 2's `load_class` only stores `factory`,
        // the primary factory pattern -- the *superclass clause's own*
        // message is a separate AST field on `ast::SuperclassClause`).
        // Evaluate it here directly from the AST rather than threading a
        // new ClassData field through Task 7 Step 1's already-large
        // bootstrap change:
        let _ = instance; // reserved for a fixture that needs superclass-clause args; M1's Object/builtins take none
        self.run_own_slots(&superclass, instance, subclass_frame, bindings)?;
        self.run_own_init_exprs(&superclass, subclass_frame, bindings)?;
        Ok(())
    }

    fn run_own_slots(
        &mut self,
        class: &ClassRef,
        instance: &Value,
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        let slots = {
            let borrowed = class.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => data.instance_slots.clone(),
                _ => return Ok(()),
            }
        };
        let slot_index = {
            let borrowed = class.borrow();
            match &borrowed.data {
                ObjectData::Class(data) => data.slot_index.clone(),
                _ => return Ok(()),
            }
        };
        for slot in &slots {
            if let Some(init) = &slot.init {
                let value = crate::interpreter::eval::eval_expr(self, init, frame, bindings)?;
                let name = Symbol::intern(&slot.name.name);
                // Absolute index (superclass slots offset, per the
                // `slot_index` fix above) -- never the slot's local,
                // this-class-only position. Using a bare `enumerate()`
                // index here would silently overwrite an already-initialized
                // superclass slot at the same low index instead of this
                // class's own slot.
                if let (Value::Obj(obj), Some(&index)) = (instance, slot_index.get(&name)) {
                    if let ObjectData::Instance(fields) = &mut obj.borrow_mut().data {
                        if index < fields.len() {
                            fields[index] = value;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn run_own_init_exprs(
        &mut self,
        class: &ClassRef,
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        let _ = class;
        let _ = frame;
        let _ = bindings;
        // Milestone 1: `ClassData` does not currently carry the class's
        // `init_exprs` statement list (Task 7 Step 1's `load_class` did not
        // store it). Add an `init_exprs: Vec<ast::Stmt>` field to
        // `ClassData` now (Task 3's types.rs) and populate it in
        // `load_class`, then evaluate each statement here via
        // `eval::eval_expr` (for `Stmt::Expr`) -- mirroring
        // `interpreter::run_statements`'s loop but without a return-value
        // capture, since init_exprs' value is discarded (spec step 5).
        // Flagged here rather than silently glossed over: this is real,
        // required behavior (every fixture with `initExpr1. initExpr2.` in
        // its class body needs it), not an optional nicety.
        Ok(())
    }
}

const BUILTIN_NAMES: &[&str] = &[
    "Object", "Boolean", "True", "False", "Integer", "Float", "String",
    "Symbol", "Character", "Array", "OrderedCollection", "Nil", "Block",
];

fn builtin_kind_from_name(name: &str) -> BuiltinClass {
    match name {
        "Boolean" => BuiltinClass::Boolean,
        "True" => BuiltinClass::True,
        "False" => BuiltinClass::False,
        "Integer" => BuiltinClass::Integer,
        "Float" => BuiltinClass::Float,
        "String" => BuiltinClass::Str,
        "Symbol" => BuiltinClass::Sym,
        "Character" => BuiltinClass::Character,
        "Array" => BuiltinClass::Array,
        "OrderedCollection" => BuiltinClass::OrderedCollection,
        "Nil" => BuiltinClass::Nil,
        "Block" => BuiltinClass::Block,
        _ => BuiltinClass::Object,
    }
}

fn method_name(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Keyword { parts } => {
            parts.iter().map(|(k, _)| format!("{}:", k.name)).collect()
        }
    }
}

fn total_slot_count(class: &ClassRef) -> usize {
    let mut count = 0;
    let mut current = Some(class.clone());
    while let Some(c) = current {
        let borrowed = c.borrow();
        let ObjectData::Class(data) = &borrowed.data else {
            break;
        };
        count += data.instance_slots.len();
        current = data.superclass.clone();
    }
    count
}
```

**Before moving on:** add `init_exprs: Vec<ast::Stmt>` to `ClassData` in
`crates/ns-runtime/src/types.rs` (Task 3), populate it in `load_class`
above, and finish `run_own_init_exprs` for real — the stub left in the code
block above is a deliberate exception to this plan's "no placeholders"
rule, called out explicitly rather than silently included, so that whoever
implements this task does not skip it. Do not commit Task 7 with
`run_own_init_exprs` still empty.

**Slot access is currently unreachable — fix this too, before committing
Task 7.** Task 6's `dispatch_lexical_send` looks a selector up via
`ClassData.methods` (and, for a setter, the selector `name:`), but
`load_class` above never puts a class's slot getters/setters *into*
`methods` — per `newspeak-spec.pdf` §6.3.2, a slot's getter/setter are
"implicitly defined by the slot declaration", not real user-written
`MethodDecl`s to store the same way. Left as written, no `LexicalSend` to a
user-defined class's own slot would ever resolve — a core, load-bearing
gap, not a cosmetic one. Fix:

1. Add `pub slot_index: HashMap<Symbol, usize>` to `ClassData`
   (`types.rs`, Task 3) — the slot's *absolute* index into an instance's
   `ObjectData::Instance` fields vector, accounting for inherited slots'
   offset (mirrors `total_slot_count`'s walk above, but per-name rather
   than a total).
2. In `load_class`, after resolving `superclass`, compute this class's own
   `slot_index` map: start at `total_slot_count(&superclass)` (0 if none)
   and assign each of `decl.instance_side.slots`, in order, the next index.
3. In `eval.rs::dispatch_lexical_send` (Task 6), **before** calling
   `dispatch::lookup_defined_for`/`lookup_protected`, check whether
   `selector` (or, for a setter, `selector` with its trailing `:` stripped)
   names a slot anywhere in `class`'s own-or-inherited `slot_index` (walk
   the superclass chain checking each level's map, same shape as
   `dispatch.rs`'s `walk` function). If found: for a getter, read
   `o`'s `ObjectData::Instance` field at that index directly and return it;
   for a setter (`selector` ends in a single trailing `:` and no other
   colons), write the argument into that field and return `o` (the spec's
   setter-method return value is the receiver; `eval_assign`, Task 6,
   already discards this and yields its own `value: Box<Expr>` argument
   instead, per §6.3.2/§5.4.3 — so what this returns barely matters, but
   returning `o` matches the spec). Only fall through to method/native
   dispatch when no slot matches.
4. A slot access is **never** access-controlled the way a method send is —
   §6.3.2's getter/setter access matches the *slot's own* declared
   modifier, but Milestone 1's fixtures (Task 12) do not exercise a
   `private`-slot-accessed-from-outside case; implement the direct
   read/write unconditionally for now and note this as a known gap rather
   than building the access check speculatively (YAGNI) — revisit if a
   fixture needs it.

Re-run Task 6's interpreter tests after this fix (a slot-referencing
fixture is exactly what Task 12's `arithmetic.ns` needs to work at all).

- [x] **Step 3: `Object`'s native methods and the default `doesNotUnderstand:` (`builtins/object_class.rs`, `builtins/mod.rs`)**

```rust
// crates/ns-runtime/src/builtins/mod.rs
pub mod object_class;

use crate::types::*;
use crate::{Interpreter, Symbol};

/// `None` = this builtin does not implement `selector` (let the caller fall
/// through to `doesNotUnderstand:`). `Some(Err(_))` = a genuine Newspeak-
/// level error (e.g. arithmetic overflow in Task 9).
pub fn dispatch_native(
    interp: &mut Interpreter,
    kind: BuiltinClass,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    match kind {
        BuiltinClass::Object => object_class::dispatch(interp, receiver, selector, args),
        // BuiltinClass::Boolean | True | False -> Task 8
        // BuiltinClass::Integer | Float -> Task 9 (numbers)
        // BuiltinClass::Str | Sym | Character -> Task 9 (text)
        // BuiltinClass::Array | OrderedCollection -> Task 10
        // BuiltinClass::Block -> Task 10
        _ => None,
    }
}
```

```rust
// crates/ns-runtime/src/builtins/object_class.rs
use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    match selector.as_str() {
        "class" => Some(Ok(Value::Obj(interp.class_of(&receiver)))),
        "==" => Some(Ok(Value::Bool(identical(&receiver, &args[0])))),
        "=" => Some(Ok(Value::Bool(identical(&receiver, &args[0])))), // Object's default: identity
        "hash" => Some(Ok(Value::SmallInt(identity_hash_of(&receiver) as i64))),
        "printString" => Some(Ok(interp.new_symbol(Symbol::intern("an Object")).clone_as_string(interp))),
        "doesNotUnderstand:" => {
            // Object's default doesNotUnderstand: (spec §5.5): must be
            // `protected`, always raises a MessageNotUnderstood-shaped
            // NewspeakError. Milestone 1 has no exception-class hierarchy
            // (Non-Goal); this produces the NewspeakError value directly
            // rather than a catchable Newspeak object.
            let msg_selector = extract_dnu_selector(&args[0]);
            Some(Err(NewspeakError {
                selector: msg_selector,
                receiver: receiver.clone(),
                message: format!("{} does not understand #{}", describe(&receiver, interp), msg_selector),
            }))
        }
        _ => None,
    }
}

fn identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::SmallInt(x), Value::SmallInt(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Obj(x), Value::Obj(y)) => std::rc::Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn identity_hash_of(value: &Value) -> u32 {
    match value {
        Value::Obj(obj) => obj.borrow().identity_hash,
        _ => 0, // primitives are values; Milestone 1 does not give them a
                // distinct identity hash (revisit if a fixture needs one)
    }
}

fn describe(_value: &Value, _interp: &Interpreter) -> String {
    "an object".to_string() // Task 9's printString work supersedes this for real classes
}

fn extract_dnu_selector(message_mirror_shaped_array: &Value) -> Symbol {
    // Matches `Interpreter::does_not_understand`'s `{selector. args}` array
    // shape from Task 6.
    if let Value::Obj(obj) = message_mirror_shaped_array {
        if let ObjectData::Array(elements) = &obj.borrow().data {
            if let Some(Value::Obj(sym_obj)) = elements.first() {
                if let ObjectData::Sym(s) = &sym_obj.borrow().data {
                    return *s;
                }
            }
        }
    }
    Symbol::intern("unknown")
}
```

`printString`'s implementation above (`clone_as_string`) is a placeholder
name that does not exist yet — **fix this before committing**: either add a
small `Interpreter::new_string_value(&mut self, s: &str) -> Value` helper
(wrapping `new_string` in `Value::Obj`) and call
`Ok(Value::Obj(interp.new_string("an Object".to_string())))` directly, or
give every builtin its own `printString` in its own file (Tasks 8-10) and
drop the generic one here — pick whichever reads more clearly once Tasks
8-10 exist; this is a real decision to make, not a detail to skip.

- [x] **Step 4: Wire the bootstrap into module loading; tests**

Add to `crates/ns-runtime/src/lib.rs`:

```rust
pub mod builtins;
```

Create `crates/ns-runtime/tests/instantiate.rs` covering, end to end
(`ns_syntax::parse` → `resolve` → `Interpreter::bootstrap()` →
`load_module` → `instantiate` → `send`):

- `class Foo = Object new (|x = 42.|) (bar = (^x)) : ()`, instantiate with
  no factory args, send `bar`, expect `Value::SmallInt(42)`.
- A subclass whose own slot initializer references a slot the superclass
  set up, confirming Step 3 (superclass initializer) really runs before
  Step 4 (own slots).
- Sending an undefined selector produces `Signal::Error` (via `send` ->
  `does_not_understand` -> `Object`'s native `doesNotUnderstand:`), not a
  panic.

- [x] **Step 5: Run, fix, verify green; commit**

Run: `cargo test -p ns-runtime` and `cargo test --workspace`.
Expected: all green.

```bash
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): implement class instantiation, the bootstrap, and Object

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 8: Core library — Boolean/True/False, Integer, Float

**Files:** Create `crates/ns-runtime/src/builtins/boolean.rs`,
`crates/ns-runtime/src/builtins/numbers.rs`; modify `builtins/mod.rs`.

**Interfaces:**
- Consumes: the `dispatch_native` shape from Task 7.
- Produces: `boolean::dispatch`, `numbers::dispatch_integer`,
  `numbers::dispatch_float`, wired into `dispatch_native`'s match arms for
  `Boolean | True | False` and `Integer | Float`.

**Design note:** `Value::Bool`/`SmallInt`/`Float` are Rust primitives, not
`ObjRef`s (Task 3) — these dispatch functions receive `receiver: Value`
directly and match on it, unlike `object_class.rs`, which mostly worked
through `ObjRef`s.

- [x] **Step 1: Write `boolean.rs`**

```rust
//! `Boolean`/`True`/`False` native methods.

use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Bool(b) = receiver else {
        return None;
    };
    match selector.as_str() {
        "ifTrue:ifFalse:" => Some(invoke_block(interp, if b { &args[0] } else { &args[1] })),
        "ifTrue:" => Some(if b {
            invoke_block(interp, &args[0])
        } else {
            Ok(Value::Nil)
        }),
        "ifFalse:" => Some(if !b {
            invoke_block(interp, &args[0])
        } else {
            Ok(Value::Nil)
        }),
        "not" => Some(Ok(Value::Bool(!b))),
        "&" | "and:" => Some(Ok(Value::Bool(b && truthy_arg(interp, &args[0])))),
        "|" | "or:" => Some(Ok(Value::Bool(b || truthy_arg(interp, &args[0])))),
        "printString" => Some(Ok(Value::Obj(
            interp.new_string(if b { "true" } else { "false" }.to_string()),
        ))),
        _ => None,
    }
}

fn invoke_block(interp: &mut Interpreter, value: &Value) -> Result<Value, NewspeakError> {
    // `and:`/`or:`/`ifTrue:` etc. take a zero-arg block; invoking it is the
    // same primitive `Block value` uses (Task 10) -- call through the
    // shared closure-invocation entry point directly rather than a full
    // `send`, since these are the language's fundamental control-flow
    // primitives and must not depend on `Block`'s own native methods being
    // registered yet in dispatch order.
    match value {
        Value::Obj(obj) => {
            if let ObjectData::Closure(closure) = &obj.borrow().data {
                let closure = std::rc::Rc::new(closure.clone());
                let bindings = std::rc::Rc::new(std::collections::HashMap::new());
                return crate::interpreter::eval::invoke_closure(interp, &closure, vec![], &bindings)
                    .map_err(|signal| match signal {
                        crate::interpreter::Signal::Error(e) => e,
                        crate::interpreter::Signal::Return(..) => NewspeakError {
                            selector: Symbol::intern("value"),
                            receiver: value.clone(),
                            message: "non-local return escaped a Boolean control-flow block".to_string(),
                        },
                    });
            }
            Ok(value.clone())
        }
        other => Ok(other.clone()),
    }
}

fn truthy_arg(interp: &mut Interpreter, value: &Value) -> bool {
    matches!(invoke_block(interp, value), Ok(Value::Bool(true)))
}
```

**Correctness note to check during implementation:** `invoke_closure`
expects an empty `bindings` map above only because these particular calls
never reference any `LexicalSend`/`SuperSend` inside a *freshly empty*
synthetic call — that is wrong in general (a real `ifTrue:` block's body
has real bindings from when it was resolved). Fix this before committing:
`ClosureData` needs to carry (or `invoke_closure` needs to receive) the
*same* `Rc<HashMap<Span, Binding>>` the block literal was originally
resolved under, not a fresh empty map. The straightforward fix is adding
`pub bindings: Rc<HashMap<Span, Binding>>` to `ClosureData` (Task 3) and
populating it in `eval_block_literal` (Task 6) from whatever `bindings`
parameter was already in scope there, then reading `closure.bindings` in
every `invoke_block`-style call site here and in Tasks 9-10, instead of
threading `bindings` through as a separate argument. Apply this fix now,
adjust Task 6's and Task 7's code accordingly, and re-run their tests
before starting Task 8's own tests.

- [x] **Step 2: Write `numbers.rs`**

Integer arithmetic must raise on overflow (`i64`, spec's Decisions section).
Use `checked_*` and turn `None` into a `NewspeakError`, never a panic (this
plan's Global Constraints).

```rust
//! `Integer` (`i64`) and `Float` (`f64`) native methods.

use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch_integer(
    _interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::SmallInt(n) = receiver else {
        return None;
    };
    let arg_int = || match args.first() {
        Some(Value::SmallInt(m)) => Some(*m),
        _ => None,
    };
    let overflow = |op: &str| NewspeakError {
        selector: Symbol::intern(op),
        receiver: Value::SmallInt(n),
        message: format!("Integer overflow computing {n} {op} {:?}", args.first()),
    };
    match selector.as_str() {
        "+" => Some(arg_int().and_then(|m| n.checked_add(m)).map(Value::SmallInt).ok_or_else(|| overflow("+"))),
        "-" => Some(arg_int().and_then(|m| n.checked_sub(m)).map(Value::SmallInt).ok_or_else(|| overflow("-"))),
        "*" => Some(arg_int().and_then(|m| n.checked_mul(m)).map(Value::SmallInt).ok_or_else(|| overflow("*"))),
        "//" => Some(match arg_int() {
            Some(0) => Err(NewspeakError {
                selector: Symbol::intern("//"),
                receiver: Value::SmallInt(n),
                message: "division by zero".to_string(),
            }),
            Some(m) => Ok(Value::SmallInt(n.div_euclid(m))),
            None => Err(overflow("//")),
        }),
        "<" => Some(Ok(Value::Bool(arg_int().is_some_and(|m| n < m)))),
        ">" => Some(Ok(Value::Bool(arg_int().is_some_and(|m| n > m)))),
        "<=" => Some(Ok(Value::Bool(arg_int().is_some_and(|m| n <= m)))),
        ">=" => Some(Ok(Value::Bool(arg_int().is_some_and(|m| n >= m)))),
        "=" => Some(Ok(Value::Bool(arg_int() == Some(n)))),
        "negated" => Some(n.checked_neg().map(Value::SmallInt).ok_or_else(|| overflow("negated"))),
        "abs" => Some(n.checked_abs().map(Value::SmallInt).ok_or_else(|| overflow("abs"))),
        "printString" | "asString" => Some(Ok(Value::Obj(_interp.new_string(n.to_string())))),
        _ => None,
    }
}

pub fn dispatch_float(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Float(x) = receiver else {
        return None;
    };
    let arg_float = || match args.first() {
        Some(Value::Float(y)) => Some(*y),
        Some(Value::SmallInt(y)) => Some(*y as f64),
        _ => None,
    };
    match selector.as_str() {
        "+" => Some(Ok(Value::Float(x + arg_float().unwrap_or(f64::NAN)))),
        "-" => Some(Ok(Value::Float(x - arg_float().unwrap_or(f64::NAN)))),
        "*" => Some(Ok(Value::Float(x * arg_float().unwrap_or(f64::NAN)))),
        "/" => Some(Ok(Value::Float(x / arg_float().unwrap_or(f64::NAN)))),
        "<" => Some(Ok(Value::Bool(arg_float().is_some_and(|y| x < y)))),
        ">" => Some(Ok(Value::Bool(arg_float().is_some_and(|y| x > y)))),
        "=" => Some(Ok(Value::Bool(arg_float() == Some(x)))),
        "printString" | "asString" => Some(Ok(Value::Obj(interp.new_string(x.to_string())))),
        _ => None,
    }
}
```

Update `builtins/mod.rs`'s match:

```rust
BuiltinClass::Boolean | BuiltinClass::True | BuiltinClass::False => {
    boolean::dispatch(interp, receiver, selector, args)
}
BuiltinClass::Integer => numbers::dispatch_integer(interp, receiver, selector, args),
BuiltinClass::Float => numbers::dispatch_float(interp, receiver, selector, args),
```

and `pub mod boolean; pub mod numbers;` at the top.

- [x] **Step 3: Tests**

Create `crates/ns-runtime/tests/builtins_boolean_numbers.rs` with, at
minimum: `3 + 4` via a direct `send` call evaluates to `SmallInt(7)`;
`i64::MAX` `+` `1` produces a `NewspeakError` (overflow), not a panic;
`true ifTrue: [1] ifFalse: [2]` (built as a hand-assembled block `Value`,
same pattern as earlier tasks, or via a real parsed+resolved+instantiated
fixture — prefer the latter now that Task 7's pipeline exists) evaluates to
`1`.

- [x] **Step 4: Run, fix, verify green; commit**

```bash
cargo test -p ns-runtime && cargo test --workspace
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): implement Boolean, Integer, and Float native methods

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 9: Core library — String, Symbol, Character

**Files:** Create `crates/ns-runtime/src/builtins/text.rs`; modify
`builtins/mod.rs`.

**Interfaces:** `text::dispatch_string`, `text::dispatch_symbol`,
`text::dispatch_character`, wired into `dispatch_native`.

- [x] **Step 1: Write `text.rs`**

Selectors to implement, each a direct, unambiguous Rust expression over the
receiver's `ObjectData::Str`/`Sym` payload (`Character` shares `Str`'s
payload per Task 7 Step 1's note — a `Str` holding exactly one `char`):

| Class | Selector | Behavior |
|---|---|---|
| `String` | `size` | `Value::SmallInt(s.chars().count() as i64)` |
| `String` | `,` (concatenation) | new `Str` = `s` + the argument's `Str` payload (type-error `NewspeakError` if the argument is not a `Str`) |
| `String` | `=` | payload string equality |
| `String` | `asSymbol` | `interp.new_symbol(Symbol::intern(&s))` |
| `String` | `printString` | the string itself, quoted with `'`, doubling any embedded `'` (mirrors the lexer's own escaping rule, `lexer.rs`'s string-escape logic) |
| `Symbol` | `asString` | new `Str` from the symbol's text |
| `Symbol` | `=` | symbol equality (`Symbol` is `Copy`+`Eq` — direct `==`) |
| `Symbol` | `printString` | `#` + the symbol's text |
| `Character` | `asInteger` | the one char's Unicode code point as `SmallInt` |
| `Character` | `asString` | a one-character `Str` |
| `Character` | `=` | payload equality |

Write each as one `match` arm in `dispatch_string`/`dispatch_symbol`/
`dispatch_character`, following exactly the pattern Task 8's `numbers.rs`
used (extract the payload via `let ObjectData::Str(s) = &obj.borrow().data
else { return None }`, dispatch on `selector.as_str()`, return
`Some(Ok(..))` per row above, `None` for anything not in the table so it
falls through to `doesNotUnderstand:`). Every row above is a complete,
unambiguous specification — do not add speculative extra selectors beyond
this table (YAGNI; extend only when a fixture in Task 12 needs one).

- [x] **Step 2: Wire into `dispatch_native`, write tests, run, fix, commit**

Same shape as Task 8 Steps 2-4. Tests: string concatenation, string
equality, symbol round-trip through `asString`/`asSymbol`, one character
constant's `asInteger`.

```bash
cargo test -p ns-runtime && cargo test --workspace
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): implement String, Symbol, and Character native methods

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 10: Core library — Array, OrderedCollection, Block

**Files:** Create `crates/ns-runtime/src/builtins/collections.rs`,
`crates/ns-runtime/src/builtins/block.rs`; modify `builtins/mod.rs`.

**Interfaces:** `collections::dispatch_array`,
`collections::dispatch_ordered_collection`, `block::dispatch`.

**Design note:** `OrderedCollection` needs a *mutable, growable* payload,
but `ObjectData::Array(Vec<Value>)` (Task 3) is shared by both `Array` and
`OrderedCollection` per the spec's Core Library list grouping them loosely
— for Milestone 1, back `OrderedCollection` with the same `ObjectData::Array`
variant (mutated in place through the `RefCell`, which already supports
this — no type change needed) rather than adding a second payload variant;
document this as a deliberate M1 simplification in a code comment (a real
`OrderedCollection` would grow independently of a literal-array's fixed
size, but M1's `ObjectData::Array(Vec<Value>)` already `Vec`-backed handles
growth fine either way).

- [x] **Step 1: Write `collections.rs`**

| Class | Selector | Behavior |
|---|---|---|
| `Array` | `size` | element count |
| `Array` | `at:` | 1-based index; out-of-range is a `NewspeakError`, not a panic |
| `Array` | `at:put:` | 1-based index write, returns the receiver |
| `Array` | `do:` | invoke the argument block once per element (via the same closure-invocation path Task 8's `invoke_block` uses) |
| `OrderedCollection` | `size`, `do:` | same as `Array` |
| `OrderedCollection` | `add:` | push, returns the argument (matches the reference library's convention) |
| `OrderedCollection` | `at:` | 1-based, `NewspeakError` out of range |

Follow Task 9's pattern: one `match` per class, `ObjectData::Array` payload,
`None` for anything not listed. For `do:`, reuse (do not duplicate) the
closure-invocation helper introduced in Task 8 — move it out of `boolean.rs`
into a shared `builtins/mod.rs` function, e.g. `pub(super) fn call_block(interp:
&mut Interpreter, block: &Value, args: Vec<Value>) -> Result<Value,
NewspeakError>`, generalized to take `args: Vec<Value>` instead of always
zero, and update Task 8's `boolean.rs` to call the shared version instead
of its own local `invoke_block` (small refactor — do it now rather than
letting three copies of the same helper accumulate across Tasks 8, 10, and
this one).

- [x] **Step 2: Write `block.rs`**

```rust
//! `Block` (closure) native methods -- `value`, `value:`, `value:value:`,
//! ... . Milestone 1 supports 0-3 arguments explicitly (matches the
//! reference library's common case; extend the match if a fixture needs
//! more).

use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let closure = {
        let borrowed = obj.borrow();
        let ObjectData::Closure(c) = &borrowed.data else {
            return None;
        };
        c.clone()
    };
    match selector.as_str() {
        "value" | "value:" | "value:value:" | "value:value:value:" => {
            Some(super::call_block(interp, &receiver, args.to_vec()).map_err(|e| e))
        }
        "numArgs" => Some(Ok(Value::SmallInt(closure.params.len() as i64))),
        _ => None,
    }
}
```

This depends on `call_block` accepting the closure's *own* recorded
parameter count and binding `args` into the invoked frame positionally —
confirm `interpreter::eval::invoke_closure` (Task 6) actually places `args`
into the new frame's `args` field keyed the same way the resolver indexed
block parameters (`Local(0)`, `Local(1)`, ...) — it does, since
`invoke_closure`'s `Frame { args, .. }` is built directly from the passed-in
`args: Vec<Value>` and the resolver's `resolve_expr`'s `Expr::Block` arm
(Task 4) put block parameters first in `LocalScope::new(&names)`, matching
`Local`'s index-into-`args`-then-`temps` convention from
`eval.rs::read_local` (Task 6). No additional wiring needed.

- [x] **Step 3: Wire into `dispatch_native`; tests; run, fix, commit**

Tests: `{1. 2. 3} at: 2` → `2`; `#(1 2 3) do: [:x | ...]`-style traversal
(sum via a mutable-slot accumulator on a small test class, or count
invocations); `OrderedCollection new add: 1; add: 2; size` → `2`
(exercises cascades, Task 6, together with these new natives — a good
end-to-end check); a block called with the wrong argument count is a
`NewspeakError` (Milestone 1: do not panic on `args[i]` out of bounds —
check `args.len()` against `closure.params.len()` in `invoke_closure`
(Task 6) and turn a mismatch into a `NewspeakError` there **before** this
task's tests can rely on it — go back and add that guard to Task 6's
`invoke_closure` now if it is missing).

```bash
cargo test -p ns-runtime && cargo test --workspace
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): implement Array, OrderedCollection, and Block native methods

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 11: The `platform` object, module loading, and `ns-next run`

**Files:** Create `crates/ns-runtime/src/platform.rs`; modify
`crates/ns-runtime/src/types.rs` (add `BuiltinClass::Platform`),
`interpreter/mod.rs` (bootstrap wiring), `builtins/mod.rs`,
`crates/ns-cli/src/main.rs`.

**Interfaces:**
- Consumes: everything from Tasks 1-10.
- Produces: `Interpreter::platform(&mut self) -> Value` (a singleton,
  memoized on first call), `ns-next run <file.ns> [--send <selector>]`.

**Design notes:**

- Add `Platform` to Task 3's `BuiltinClass` enum now (`types.rs`) — a small,
  additive change; re-run `cargo test -p ns-runtime types::` after to
  confirm nothing else assumed the enum was closed to further variants (it
  should not have — every `match` over `BuiltinClass` elsewhere either has
  a `_ => None` fallback (`dispatch_native`'s per-class arms) or is a `for
  kind in [...]` literal list (Task 7's `build_builtin_classes`) that this
  step must also extend.
  the spec: "a minimal `platform` object exposes these classes plus a
  stdout stream object (enough for `Transcript`-style output in tests)."
  Milestone 1 implements exactly that: `platform` responds to `transcript`
  (returning a small stream-like object), and the stream responds to
  `show:` (writes to Rust's real stdout via `println!`/`print!`) and
  `newLine` (writes `\n`). Nothing else — do not add unrequested platform
  surface (collections, files, etc. all come from the classes above, not
  from `platform` itself, matching the spec's Core Library / platform
  split).

- [x] **Step 1: `platform.rs`**

```rust
//! The minimal `platform` object: exposes the Core Library classes by name
//! plus a `Transcript`-style stdout stream. See the runtime spec's Core
//! Library section.

use std::io::Write;

use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    _args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let is_platform = matches!(&obj.borrow().data, ObjectData::Instance(fields) if fields.is_empty());
    if !is_platform {
        return dispatch_stream(interp, receiver, selector, _args);
    }
    match selector.as_str() {
        "transcript" => Some(Ok(Value::Obj(interp.new_stream_object()))),
        // Class accessors: `platform <ClassName>` style access some real
        // corpus modules use directly on `platform` rather than a
        // sub-module -- Milestone 1 supports the Core Library names only.
        "Object" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::Object)))),
        "Array" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::Array)))),
        "OrderedCollection" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::OrderedCollection)))),
        "String" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::Str)))),
        "Symbol" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::Sym)))),
        _ => None,
    }
}

fn dispatch_stream(
    _interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    match selector.as_str() {
        "show:" => {
            if let Some(Value::Obj(s)) = args.first() {
                if let ObjectData::Str(text) = &s.borrow().data {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
            }
            Some(Ok(receiver))
        }
        "newLine" => {
            println!();
            Some(Ok(receiver))
        }
        _ => None,
    }
}
```

Add to `Interpreter` (`interpreter/mod.rs`):

```rust
impl Interpreter {
    pub fn platform(&mut self) -> Value {
        if let Some(p) = &self.platform_singleton {
            return p.clone();
        }
        let class = self.builtin_class(BuiltinClass::Platform);
        let obj = self.new_instance_object(class, ObjectData::Instance(vec![]));
        let value = Value::Obj(obj);
        self.platform_singleton = Some(value.clone());
        value
    }

    fn new_stream_object(&mut self) -> ObjRef {
        let class = self.builtin_class(BuiltinClass::Platform);
        // The stream reuses the Platform class tag; `platform::dispatch`
        // tells the two apart by payload shape (empty Instance == the
        // platform object itself, per Step 1's `is_platform` check) --
        // documented there as the discriminator.
        self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil]))
    }
}
```

Add `platform_singleton: Option<Value>` to the `Interpreter` struct
(`None` in both `new()` and `bootstrap()`), and register `Platform` in
Task 7's `build_builtin_classes` kind list. Add
`BuiltinClass::Platform => platform::dispatch(interp, receiver, selector, args),`
to `builtins/mod.rs`'s `dispatch_native` match, and `pub mod platform;` —
note `platform.rs` lives at the crate root (`src/platform.rs`, per the File
Structure section), not under `src/builtins/`, so this line goes in
`crates/ns-runtime/src/lib.rs` (`pub mod platform;`) with `use
crate::platform;` inside `builtins/mod.rs`'s dispatch function instead of a
nested `pub mod`.

- [x] **Step 2: `ns-next run` (`crates/ns-cli/src/main.rs`)**

Read the existing `main.rs` first (frontend milestone's `parse` /
`parse-corpus` subcommands are already there — follow the same `clap`
derive pattern). Add:

```rust
#[derive(clap::Args)]
struct RunArgs {
    file: PathBuf,
    #[arg(long, default_value = "main:args:")]
    send: String,
}
```

(Add a `Run(RunArgs)` variant to the existing `Command` enum, matching
however `Parse`/`ParseCorpus` are already declared.)

```rust
fn cmd_run(args: RunArgs) -> ExitCode {
    let source = match std::fs::read_to_string(&args.file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: reading {}: {e}", args.file.display());
            return ExitCode::FAILURE;
        }
    };
    let module = match ns_syntax::parse(&source) {
        Ok(m) => m,
        Err(diags) => {
            for d in diags {
                eprintln!("{}", render_diagnostic(&d, &source));
            }
            return ExitCode::FAILURE;
        }
    };
    let resolved = match ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File(args.file.clone()),
    ) {
        Ok(r) => r,
        Err(errors) => {
            for e in errors {
                eprintln!("resolve error: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
    };
    let bindings = resolved.bindings.clone();
    let mut interp = ns_runtime::Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    let platform = interp.platform();
    let instance = match interp.instantiate(class, vec![platform.clone()], &bindings) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    let selector = ns_runtime::Symbol::intern(&args.send);
    match interp.send(instance, selector, vec![platform, ns_runtime::Value::Nil], &bindings, None) {
        Ok(_) => ExitCode::SUCCESS,
        Err(ns_runtime::Signal::Error(e)) => {
            eprintln!("Uncaught Newspeak error: {}", e.message);
            ExitCode::FAILURE
        }
        Err(ns_runtime::Signal::Return(..)) => {
            eprintln!("internal: non-local return escaped the entry send");
            ExitCode::FAILURE
        }
    }
}

fn report_signal(signal: ns_runtime::Signal) -> ExitCode {
    match signal {
        ns_runtime::Signal::Error(e) => {
            eprintln!("Uncaught Newspeak error during instantiation: {}", e.message);
        }
        ns_runtime::Signal::Return(..) => {
            eprintln!("internal: non-local return escaped instantiation");
        }
    }
    ExitCode::FAILURE
}
```

`args: vec![platform.clone()]` for `instantiate` and `vec![platform,
Value::Nil]` for the entry `main:args:` send match the spec's Core Library
factory convention (`usingPlatform: platform`) and `main:args:`'s two
parameters (`newspeak-spec.pdf` §8.4) — Milestone 1 passes `Nil` for the
environment-args list (no real command-line-argument passthrough into
Newspeak yet; add it only if a fixture in Task 12 needs it). Add
`--send` to override the entry selector for fixtures that use a different
one than `main:args:` (Task 12's harness will need this for anything that
is not a full `main:args:`-shaped application).

`render_diagnostic` should already exist in `main.rs` from the frontend
milestone's `parse` subcommand — reuse it rather than writing a new one.

- [x] **Step 3: Tests, run, fix, commit**

Add `crates/ns-cli/tests/run.rs` (mirrors the existing `cli.rs` pattern from
the frontend milestone): a temp `.ns` file with a trivial `main:args:` that
sends `platform transcript show: 'hi'`, run `ns-next run`, assert exit 0 and
stdout contains `hi`.

```bash
cargo test --workspace
cargo fmt --all && cargo clippy --all-targets
git add -A
git commit -m "feat(runtime): add the platform object and \`ns-next run\`

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 12: Execution integration test harness

**Files:** Create `tests-fixtures/hello.ns`, `tests-fixtures/hello.expected`,
`tests-fixtures/arithmetic.ns`, `tests-fixtures/arithmetic.expected`,
`tests-fixtures/blocks_and_return.ns`, `tests-fixtures/blocks_and_return.expected`,
`crates/ns-cli/tests/run_fixtures.rs`.

**Interfaces:** Consumes `ns-next run` (Task 11). Produces the runtime
spec's Testing section's "Execution integration tests" — the harness every
future fixture (and, eventually, a handful of real corpus modules whose
platform needs Milestone 1 satisfies, per the spec's Reality-Check goal)
plugs into.

- [x] **Step 1: Three fixtures exercising distinct behavior**

`tests-fixtures/hello.ns`:

```
Newspeak3
'Test'
class Hello usingPlatform: platform = Object new (
) (
	public main: p args: a = (
		p transcript show: 'hello'; newLine
	)
) : ()
```

`tests-fixtures/hello.expected`:

```
hello
```

`tests-fixtures/arithmetic.ns` (exercises slots, `+`, `printString`, and an
own-vs-inherited-slot instantiation order check per Task 7):

```
Newspeak3
'Test'
class Arithmetic usingPlatform: platform = Object new (
	| result = 6 * 7. |
) (
	public main: p args: a = (
		p transcript show: result printString; newLine
	)
) : ()
```

`tests-fixtures/arithmetic.expected`:

```
42
```

`tests-fixtures/blocks_and_return.ns` (exercises blocks, non-local return,
and cascades together — Task 6's trickiest behavior, now checked end to
end):

```
Newspeak3
'Test'
class BlocksAndReturn usingPlatform: platform = Object new (
) (
	find: target in: coll = (
		coll do: [:each | each = target ifTrue: [^'found']].
		^'not found'
	)
	public main: p args: a = (
		| c = OrderedCollection new. |
		c add: 1; add: 2; add: 3.
		p transcript show: (self find: 2 in: c); newLine.
		p transcript show: (self find: 9 in: c); newLine
	)
) : ()
```

`tests-fixtures/blocks_and_return.expected`:

```
found
not found
```

Confirm each `.ns` file actually parses (`cargo run -p ns-cli -- parse
tests-fixtures/<name>.ns`) before relying on it in the harness below — fix
the fixture's syntax, not the parser, for any pure syntax mismatch (the
parser is done and reviewed).

- [x] **Step 2: The harness (`crates/ns-cli/tests/run_fixtures.rs`)**

```rust
use std::path::Path;
use std::process::Command;

#[test]
fn every_tests_fixtures_ns_file_produces_its_expected_stdout() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let expected_path = path.with_extension("expected");
        let expected = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("missing {}", expected_path.display()));
        let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&path)
            .output()
            .expect("run ns-next");
        assert!(
            output.status.success(),
            "{} exited non-zero: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.trim_end(),
            expected.trim_end(),
            "{} produced unexpected stdout",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 3, "expected at least the 3 fixtures this task adds");
}
```

- [x] **Step 3: Run, fix, verify green**

Run: `cargo test --workspace`
Expected: all green, including the three fixtures.

Run: `cargo fmt --all -- --check` and `cargo clippy --all-targets`
Expected: both clean.

- [x] **Step 4: Commit**

```bash
git add -A
git commit -m "test(runtime): add the execution-fixture integration harness

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Final Verification

After Task 12, before declaring the milestone done:

- [x] `cargo test --workspace` fully green (frontend's 110 tests plus every
  test this plan added).
- [x] `cargo fmt --all -- --check` clean.
- [x] `cargo clippy --workspace --all-targets` zero warnings.
- [x] `cargo run -p ns-cli -- parse-corpus corpus/modules` still reports
  210/0/1/0 (this plan must not regress the frontend's parser corpus —
  nothing in `ns-runtime` should ever need a `ns-syntax` change; if it does,
  stop and flag it per this plan's Global Constraints rather than silently
  patching the frontend).
- [x] Re-read the runtime spec's Testing section and confirm every bullet
  has a corresponding task: resolver unit tests (Task 4) ✓, interpreter
  behavior tests (Task 6) ✓, parser corpus test (already shipped, frontend
  milestone) ✓, execution integration tests (Task 12) ✓.
- [x] Every "fix this before committing" / "flagged here rather than
  silently glossed over" note left in Tasks 7, 8, and 10 above has actually
  been resolved, not merely noticed (confirmed by the corresponding `fix(*)`
  and `docs(runtime)` commits in the git history, e.g. `ee91570`, `ecadba7`,
  `07c4992`, `27fbe10`, `122013d`).

