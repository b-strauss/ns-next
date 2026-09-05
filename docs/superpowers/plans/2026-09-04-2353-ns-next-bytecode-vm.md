# ns-next Bytecode Compiler and VM (Milestone 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Status: Complete.** Verified against the codebase and git history on
> 2026-09-05 (commit range `2962c46..215ae2d`) — all 16 tasks (105 steps)
> implemented and tested: `cargo test --workspace`, `cargo fmt --all --
> --check`, and `cargo clippy --workspace --all-targets` are all clean;
> `parse-corpus corpus/modules` is unchanged at `parsed 210, failed 0,
> expected-fail 1, stale 0`; every planned commit message matches the actual
> git log verbatim. Five additional `fix(runtime):` commits beyond the 15
> planned `feat` commits (`cc8ced7`, `cacbdba`, `27194d3`, `f632675`,
> `215ae2d`) fixed findings from code review of each task's own diff (an
> off-by-one call-site assertion, literal-nesting bounds, a decoded
> closure's parameter count, and two crafted images that still reached a
> panic) — see those commits' messages for detail. Two ticked "write the
> decoder" steps (Task 13 Step 4, Task 14 Step 5) are done but their shipped
> code deliberately differs from the plan's literal listing: bounded literal
> nesting, a closure-arity check, and three new decode-time refusals
> (not-a-class, missing selector, missing initializer), each closing a path
> where a crafted image reached a panic or a process abort. Kept for
> historical reference; do not use as a checklist for new work.

**Goal:** Add a register-based bytecode compiler and VM to `ns-runtime`, selectable via `ns-next run --engine=bytecode` (the new default), that compiles and executes every method body, block body, **and instance initializer** through the Milestone 1 interpreter's *shared* dispatch orchestration — with baseline control-flow inlining, a statement-boundary safepoint/PC table, reserved inline-cache slots, `ns-next disasm`, and a versioned image save/load format (`ns-next snapshot`/`ns-next load`) whose code, object, and frame tables round-trip — while keeping the Milestone 1 tree-walker as a permanent `--engine=tree-walk` reference path that is a byte-for-byte oracle (exit status, stdout, stderr).

**Architecture:** New modules inside `ns-runtime` (`bytecode.rs`, `compiler/`, `vm.rs`, `image/`), consuming the unchanged Milestone 1 `ResolvedModule`, `dispatch.rs`, and object model. `interpreter/` becomes the **shared dispatch orchestration** for both engines: `Interpreter::send`, `doesNotUnderstand:`, the lexical/super/native dispatch functions, `invoke_method`'s and `invoke_closure`'s frame construction, and `instantiate`'s six-step algorithm are all unchanged in behavior and gain exactly one `engine` field (`Engine::TreeWalk | Engine::Bytecode(CompiledModule)`) consulted at three body-execution hooks — method body, block body, instance initializer — that route to `vm::run_code` under the bytecode engine. `vm.rs` is a `match` loop over `Instr` with **no dispatch logic, no error text, and no `doesNotUnderstand:` of its own**; every `Send*` calls straight back into `interpreter/`. That sharing is what makes the tree-walker a differential-testing oracle for stderr, not only stdout.

**Tech Stack:** Rust 1.98 (edition 2021), no new external dependencies (`insta` is already a workspace dev-dependency and is added to `ns-cli`'s dev-dependencies for `disasm` snapshots). Image encoding is hand-rolled (`Vec<u8>` + explicit little-endian byte writes/reads), not `serde`/`bincode` — consistent with the project's "no new dependency unless needed" stance and the spec's own canonical-encoding requirement.

**Spec:** `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md` (revised 2026-09-05 — read its "Open Questions Resolved During Planning" section, which records every completeness correction this plan is built on) — read it in full before starting; this plan implements it section by section. It in turn depends on `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md` (the Milestone 1 object model, resolver, dispatch, and Class Instantiation this milestone reuses) and `docs/superpowers/specs/2026-09-04-1153-ns-next-liveness-images-vm-design.md` (the cross-milestone constraints this milestone's safepoints/image format satisfy). Read the actual Milestone 1 source cited in each task's design notes (`crates/ns-runtime/src/interpreter/{mod,eval,instantiate}.rs`, `dispatch.rs`, `builtins/`) before touching it — every task's notes cite the exact function it depends on, and those citations were verified against the source on 2026-09-05.

## Global Constraints

- **Rust toolchain:** 1.98.0, pinned via `rust-toolchain.toml`.
- **Edition:** 2021.
- **All code, identifiers, comments, and doc comments in English.**
- **Lints:** `cargo clippy --workspace --all-targets` must be warning-free before every commit.
- **Formatting:** `cargo fmt --all` before every commit.
- **Terminology:** classic terms — `blacklist`, `whitelist`, `sane`.
- **No new external dependencies.** Image encoding is hand-rolled; every other task uses only what the workspace already depends on.
- **`interpreter/` is shared orchestration, modified only at the points the spec names.** The permitted edits, and no others: `types.rs` (`ClosureBody`, `FrameKind::Initializer`), `interpreter/mod.rs` (`Engine`, the `engine` field, `set_engine`/`compiled_module`/`advance_identity_hash_past`, hook 1 in `invoke_method`, the `Initializer` arm in `raise_return`), `interpreter/eval.rs` (hook 2 in `invoke_closure`, `ClosureBody::Ast` in `eval_block_literal`, the `home_of` extraction, and `pub(crate)` on `read_local`/`write_local`/`frame_at_depth`/`dispatch_lexical_send`/`dispatch_super_send`/`parse_int_literal`), `interpreter/instantiate.rs` (hook 3, `FrameKind::Initializer` frames, and `pub(crate)` on the pure `method_name` helper so the image encoder can name a frame's method). **No dispatch logic, error text, or lookup is ever duplicated into `vm.rs` or `compiler/`** — if a task seems to need a copy of something in `interpreter/`, make the original `pub(crate)` instead and say so in the task. Under `--engine=tree-walk` every Milestone 1 test must keep its exact behavior.
- **Bytecode compilation of an already-resolved module can never raise a `NewspeakError`.** Every name is already resolved by the Milestone 1 resolver before compilation starts; compilation is a total, deterministic lowering. A compiler bug (an unreachable `Binding` variant, a register-count invariant violated, `Binding::Local` inside an inlined block) uses `panic!`/`debug_assert!`, exactly like an `ns-runtime` interpreter-invariant violation — never a `NewspeakError`. A *Newspeak-program-level* error that M1 raises at evaluation time (a malformed integer literal) is deferred to execution (`Literal::MalformedInt`), never raised at compile time. `unwrap`/`expect` remain fine in tests.
- **`Instr` is a plain, `match`-dispatched Rust enum, never hand-packed bytes.** "Architecture-neutral, versioned" is a property of `image::encode`/`decode`'s wire format, not of `Instr`'s in-memory shape.
- **Control-flow inlining is gated at compile time** (ordinary send, fixed selector, syntactic zero-parameter zero-temporary block literals in every block position — and the receiver, for `whileTrue:`/`whileFalse:`), **and every inlined case keeps a runtime guard whose failure path is a real virtual send through `interpreter/`**: the original selector for `ifTrue:`-family/`and:`/`or:`, `mustBeBoolean` for a non-Boolean `whileTrue:`/`whileFalse:` condition. Never skip the fallback.
- **`Signal` has no `Debug` impl** (M1 decision — it holds a `FrameRef` cycle). Tests never call `.expect()`/`.unwrap()` or `{:?}` on an `EvalResult`; they unwrap through an `expect_ok` helper (shown in every test file that needs one). **Never `{:?}` a `Value`** either — the bootstrap `Object` class is its own `.class`, so `Debug` on any `Value::Obj` recurses forever.
- **Never mutate `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`, `docs/superpowers/specs/2026-09-01-2252-ns-next-runtime-design.md`, or `docs/superpowers/specs/2026-09-04-1153-ns-next-liveness-images-vm-design.md` while executing this plan.** If implementation reveals one of them is wrong, stop and flag it (per the subagent-driven-development skill's ruling process) rather than silently diverging.
- **Commits:** conventional-commit style (`feat:`, `test:`, `fix:`, `docs:`, `chore:`). Commit at the end of every task. Keep the trailer `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`. Never override git author/committer identity with `-c`. Never add a `Claude-Session:` trailer.
- **This milestone must not regress Milestone 1.** `cargo run -p ns-cli -- parse-corpus corpus/modules` must still report `parsed 210, failed 0, expected-fail 1, stale 0` at the end (Task 17), and nothing under `crates/ns-syntax` is touched.

---

## File Structure

```
D:\Workspace\ns-next\
  crates/
    ns-runtime/
      Cargo.toml                    # unchanged
      src/
        lib.rs                      # add `pub mod bytecode; compiler; vm; image;`, re-export Engine
        types.rs                    # ClosureBody enum; FrameKind::Initializer(ClassRef)
        dispatch.rs                 # unchanged, reused directly
        interpreter/
          mod.rs                    # Engine, engine field, hook 1 (invoke_method), raise_return arm
          eval.rs                   # hook 2 (invoke_closure), home_of, pub(crate) visibility
          instantiate.rs            # hook 3 (run_own_initializer), Initializer frames
        builtins/
          block.rs                  # + whileTrue: / whileFalse: natives
          boolean.rs                # + ifFalse:ifTrue: native
        bytecode.rs                 # NEW: Literal, CallSite, Instr, CodeKind, CompiledCode,
                                    #      CompiledModule, method_key/class_key, disassemble()
        compiler/
          mod.rs                    # NEW: compile_method / compile_initializer / compile_module
          lower.rs                  # NEW: Lowerer — Expr/Stmt -> Instr, registers, scope stack
          control_flow.rs           # NEW: special-selector recognition (pure), used by lower.rs
        vm.rs                       # NEW: run_code — the match loop, nothing else
        image/
          mod.rs                    # NEW: snapshot()/load(), LoadedImage, overall layout
          encode.rs                 # NEW: Writer, header, values, code table, object+frame tables
          decode.rs                 # NEW: Reader, DecodeError, the inverses, two-pass decoding
      tests/
        compiler.rs                 # NEW: compile_module / compile_initializer tests
        vm.rs                       # NEW: bytecode execution through Interpreter::send
        vm_initializer.rs           # NEW: hook 3 — instance initializers under bytecode
        builtins_control_flow.rs    # NEW: whileTrue:/whileFalse:/ifFalse:ifTrue: natives (tree-walk)
        image.rs                    # NEW: encode/decode round-trip tests
    ns-cli/
      Cargo.toml                    # + [dev-dependencies] insta
      src/main.rs                   # --engine, disasm, snapshot, load; shared load/bootstrap helpers
      tests/
        differential.rs             # NEW: every fixture + inline sources under both engines
        disasm_snapshots.rs         # NEW: insta snapshots of `ns-next disasm`
        snapshot_load.rs            # NEW: snapshot then load, end to end
  tests-fixtures/
    instance_init.ns / .expected    # NEW: slot initializers reading factory params, initExprs
    while_loop.ns / .expected       # NEW: inlined whileTrue:/whileFalse:
    control_flow_fallback.ns / .expected  # NEW: guard failure -> user-implemented ifTrue:ifFalse:
```

Responsibilities and split rationale:

- **`bytecode.rs`** — one file, pure data (`Literal`, `CallSite`, `Instr`, `CodeKind`, `CompiledCode`, `CompiledModule`) plus the two identity-key helpers and the `disassemble` formatter (a pure function over that data). Mirrors `types.rs`'s "mutually-referential pure data in one file" rationale.
- **`compiler/`** — split by concern, mirroring `resolver/`: `lower.rs` (the core `Expr`/`Stmt` -> `Instr` translation, register allocation, the scope stack, and — from Task 11 — the inlined-form emission) vs. `control_flow.rs` (pure recognition of inlinable send shapes, its own test surface) vs. `mod.rs` (wiring: `compile_method`/`compile_initializer`/`compile_module`).
- **`vm.rs`** — one file, one function that matters: `run_code`. It has no dispatch logic (spec: "`vm.rs` and the engine hook"), so there is nothing to split.
- **`interpreter/`** — modified only at the named hooks; see Global Constraints.
- **`image/`** — split the way `resolver/` is: `encode.rs`/`decode.rs` hold the byte-level format (independently testable), `mod.rs` holds `snapshot()`/`load()` and the overall section layout.
- **`ns-cli/tests/differential.rs`** lives in `ns-cli` because it invokes the compiled `ns-next` binary under two `--engine` flags, exactly like the existing `run_fixtures.rs`.

---

## Task 1: `bytecode.rs` — the instruction set and compiled-code shapes

**Files:**
- Create: `crates/ns-runtime/src/bytecode.rs`
- Modify: `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Consumes: `Symbol`, `types::{BuiltinClass, Binding, ClassRef}`, `ns_syntax::Span`, `ns_syntax::ast::MethodDecl` (all existing).
- Produces (every later task's shared vocabulary — the complete opcode list from the spec's `bytecode.rs` section, defined once here):
  - `pub enum Literal { Nil, Bool(bool), SmallInt(i64), Float(f64), Str(String), Sym(Symbol), Char(char), Array(Vec<Literal>), MalformedInt(String) }`
  - `pub struct CallSite { pub call_site_id: u16 }`
  - `pub enum Instr { .. }` (20 variants, below)
  - `pub enum CodeKind { Method, Block, Initializer }`
  - `pub struct CompiledCode { pub kind, pub instrs, pub num_args: u16, pub num_registers: u16, pub statement_pc: Vec<(usize, Range<usize>)>, pub call_sites: Vec<CallSite>, pub source: Span }`
  - `pub struct CompiledModule { pub methods: HashMap<usize, Rc<CompiledCode>>, pub initializers: HashMap<usize, Rc<CompiledCode>>, pub bindings: Rc<HashMap<Span, Binding>> }`
  - `pub fn method_key(method: &Rc<ast::MethodDecl>) -> usize`, `pub fn class_key(class: &ClassRef) -> usize`

**Design notes:**
- A "register" is one `u16` index into a *unified* address space that generalizes Milestone 1's `Binding::Local` addressing: index `< num_args` means `Frame.args[index]`, index `>= num_args` means `Frame.temps[index - num_args]` — exactly `eval::read_local`/`write_local`'s existing split. `CompiledCode.num_registers` is the **total** register count; the hooks (Tasks 7-8) size `Frame.temps` to `num_registers - num_args`.
- `CallSite.call_site_id` is the `Send*` instruction's own index into `instrs` (stable, reconstructible, no separate counter). Inert in this milestone.
- Jump targets and `InitSlot.field_index` are `u32` (not `usize`) so the image encoding (Task 14) is total with no truncation questions; the VM casts to `usize` at use.
- `MakeClosure` carries the block's *compiled* body (`Rc<CompiledCode>`, `kind: Block`), never its AST — the spec's "Blocks and closures" section. `captured_frame`/`home`/`bindings` are runtime values the VM supplies.
- `Literal::MalformedInt(String)`: the spec's deferred-error case (an integer literal the lexer accepted but `parse_int_literal` rejects). The VM re-runs the shared parser at execution and raises M1's exact `NewspeakError`.
- The whole enum is defined now (the spec has one list) rather than opcode-by-opcode; each opcode's *emitter* still lands in the task that first needs it, and `vm::run_code` (Task 7) handles every variant from day one.
- `PartialEq` on `Instr`/`CompiledCode` exists for `assert_eq!` in tests and for the image round-trip property `decode(encode(code)) == code` (Task 14).

- [x] **Step 1: Write the failing tests**

`crates/ns-runtime/src/bytecode.rs`:

```rust
//! The bytecode instruction set and compiled-code data shapes. Pure data --
//! see `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! `bytecode.rs` section. `Instr` is a plain Rust enum, `match`-dispatched
//! by `vm.rs`; "architecture-neutral, versioned" describes `image::encode`'s
//! wire format for a `CompiledCode`, not this in-memory type.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::{ast, Span};

use crate::types::{Binding, BuiltinClass, ClassRef};
use crate::Symbol;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_code_construction_smoke_test() {
        let code = CompiledCode {
            kind: CodeKind::Method,
            instrs: vec![
                Instr::LoadLiteral { dst: 1, literal: Literal::SmallInt(42) },
                Instr::Return { reg: 1 },
            ],
            num_args: 1,
            num_registers: 2,
            statement_pc: vec![(0, 0..2)],
            call_sites: vec![],
            source: Span::new(0, 10),
        };
        assert_eq!(code.instrs.len(), 2);
        assert_eq!(code.num_registers - code.num_args, 1);
        assert_eq!(code, code.clone());
    }

    #[test]
    fn call_site_id_is_copy_and_comparable() {
        let a = CallSite { call_site_id: 3 };
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn send_instr_variants_carry_a_call_site() {
        let send = Instr::SendPublic {
            selector: Symbol::intern("foo:"),
            receiver_reg: 0,
            arg_regs: vec![1],
            dst: 2,
            call_site: CallSite { call_site_id: 0 },
        };
        assert!(matches!(send, Instr::SendPublic { call_site: CallSite { call_site_id: 0 }, .. }));
    }

    #[test]
    fn make_closure_carries_compiled_block_code() {
        let block = Rc::new(CompiledCode {
            kind: CodeKind::Block,
            instrs: vec![Instr::ImplicitReturn { reg: 0 }],
            num_args: 1,
            num_registers: 1,
            statement_pc: vec![(0, 0..1)],
            call_sites: vec![],
            source: Span::new(0, 5),
        });
        let make = Instr::MakeClosure { dst: 0, params: Rc::new(vec![Symbol::intern("x")]), code: block };
        let Instr::MakeClosure { code, .. } = &make else { panic!() };
        assert_eq!(code.kind, CodeKind::Block);
    }

    #[test]
    fn method_key_is_rc_pointer_identity() {
        let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()";
        let module = ns_syntax::parse(src).expect("parse");
        let method = Rc::new(module.root_class.instance_side.methods[0].clone());
        let same = method.clone();
        let other = Rc::new(module.root_class.instance_side.methods[0].clone());
        assert_eq!(method_key(&method), method_key(&same));
        assert_ne!(method_key(&method), method_key(&other));
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime bytecode::`
Expected: FAIL — `Instr`/`CompiledCode`/`CallSite`/`Literal`/`CodeKind`/`method_key` not found (the module is not yet declared in `lib.rs`, so the first failure is "unresolved module"; wire Step 4 first if you prefer a compile error naming the missing types).

- [x] **Step 3: Write the implementation**

Insert into `crates/ns-runtime/src/bytecode.rs`, after the `use` lines and before the `#[cfg(test)]` module:

```rust
/// A compiled instruction-set literal -- a lowered form of `ast::Literal`
/// that keeps only what execution needs. Int/float text is parsed once at
/// compile time; `Str`/`Sym`/`Char`/`Array` still allocate a fresh heap
/// object each time they execute (matching `eval::eval_literal` -- a literal
/// string is a fresh `Object` on every evaluation in Milestone 1 too).
///
/// `MalformedInt` carries the raw text of an integer literal the lexer
/// accepted but `interpreter::eval::parse_int_literal` rejects (out of
/// `i64` range, a fractional radix literal). That is a Newspeak-program
/// error M1 raises *when the literal is evaluated*; compilation cannot
/// raise a `NewspeakError`, so the VM re-runs `parse_int_literal` when the
/// instruction executes and raises the identical error then.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Nil,
    Bool(bool),
    SmallInt(i64),
    Float(f64),
    Str(String),
    Sym(Symbol),
    Char(char),
    /// Element literals, recursively -- mirrors `ast::Literal::Array`.
    Array(Vec<Literal>),
    MalformedInt(String),
}

/// One send instruction's stable identity -- the instruction's own index
/// into `CompiledCode.instrs`. Reserved for Milestone 3's inline caches;
/// inert in Milestone 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    pub call_site_id: u16,
}

/// Every register operand is a `u16` index into the frame's unified
/// args-then-temps register file; every jump target is an instruction
/// index into the owning `CompiledCode.instrs`.
#[derive(Debug, Clone, PartialEq)]
pub enum Instr {
    LoadLiteral { dst: u16, literal: Literal },
    LoadSelf { dst: u16 },
    /// M1's "bare builtin class name has no `Binding`" convention
    /// (`eval::eval_ident`'s fallback), resolved at compile time.
    LoadBuiltinClass { dst: u16, kind: BuiltinClass },
    Move { dst: u16, src: u16 },
    /// `Binding::Local` write. A read needs no instruction -- the local's
    /// register *is* its value.
    StoreLocal { src: u16, slot: u16 },
    /// `Binding::OuterLocal` read/write through the `captured_frame` chain.
    LoadOuterLocal { dst: u16, depth: u16, index: u16 },
    StoreOuterLocal { src: u16, depth: u16, index: u16 },
    /// `Binding::LexicalSend`: kth-enclosing-object + private/`lookupProtected`.
    /// No receiver register -- the receiver is `self` by construction.
    SendLexical { depth: u16, selector: Symbol, arg_regs: Vec<u16>, dst: u16, call_site: CallSite },
    /// An ordinary, explicit-receiver send (`lookupPublic`).
    SendPublic { selector: Symbol, receiver_reg: u16, arg_regs: Vec<u16>, dst: u16, call_site: CallSite },
    /// `Binding::SuperSend`: receiver is `self`, lookup starts at the
    /// current activation's `current_class`'s superclass.
    SendSuper { selector: Symbol, arg_regs: Vec<u16>, dst: u16, call_site: CallSite },
    /// Builds a closure over the *compiled* block body. `captured_frame`,
    /// `home`, and `bindings` are runtime values the VM supplies from its
    /// current frame when the instruction executes.
    MakeClosure { dst: u16, params: Rc<Vec<Symbol>>, code: Rc<CompiledCode> },
    MakeArray { dst: u16, elem_regs: Vec<u16> },
    /// Writes `self`'s instance field `field_index` -- emitted only by
    /// instance-initializer compilation for a slot's own initializer
    /// expression. The one layout-dependent instruction (spec: Milestone
    /// 3's class-shape-change flush must invalidate compiled initializers).
    InitSlot { src: u16, field_index: u32 },
    /// A method or initializer body's `^`.
    Return { reg: u16 },
    /// A body that fell off its end: a method returns `self`, a block its
    /// last expression's value. Treated by the VM exactly like `Return`;
    /// kept distinct so `disasm` output stays legible.
    ImplicitReturn { reg: u16 },
    /// A `^` inside a block body -- unwinds to the activation's `home` via
    /// the shared `Interpreter::raise_return`.
    NonLocalReturn { reg: u16 },
    /// Unconditional jump. Emitted only by control-flow inlining.
    Jump { target: u32 },
    /// Control-flow inlining's runtime guard: jumps if `reg` does not hold
    /// a `Value::Bool` at all.
    JumpIfNotBoolean { reg: u16, target: u32 },
    /// Jumps if `reg` is exactly `Value::Bool(false)`; falls through for
    /// anything else (a `JumpIfNotBoolean` on the same register always
    /// precedes it in inlined code).
    BranchIfFalse { reg: u16, target: u32 },
    /// Symmetric to `BranchIfFalse`; used for `whileFalse:`'s exit.
    BranchIfTrue { reg: u16, target: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    Method,
    Block,
    Initializer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledCode {
    pub kind: CodeKind,
    pub instrs: Vec<Instr>,
    pub num_args: u16,
    /// Total register count: `num_args` + named temps + compiler-allocated
    /// scratch registers. The engine hooks size `Frame.temps` to
    /// `num_registers - num_args`.
    pub num_registers: u16,
    /// Maps a statement index (a method's/block's `CodeBody.stmts` index;
    /// for an initializer, each slot initializer then each `initExpr`, in
    /// that order) to the half-open instruction range it compiled to --
    /// the safepoint boundary and the M1-compatible "statement `pc`" view.
    pub statement_pc: Vec<(usize, Range<usize>)>,
    /// One entry per `Send*` instruction, in instruction order. Inert.
    pub call_sites: Vec<CallSite>,
    pub source: Span,
}

/// The bytecode engine's per-module cache, installed on the `Interpreter`
/// as `Engine::Bytecode(module)`. Keyed by `Rc` pointer identity of the
/// *loaded* `MethodDecl`/`ClassRef` -- the exact `Rc`s `dispatch.rs` hands
/// back -- so no name-based index can drift and no two same-named methods
/// on different classes can collide.
#[derive(Debug)]
pub struct CompiledModule {
    pub methods: HashMap<usize, Rc<CompiledCode>>,
    pub initializers: HashMap<usize, Rc<CompiledCode>>,
    /// The module's resolver output, carried so the hooks can pass it to
    /// the shared orchestration's `bindings` parameters.
    pub bindings: Rc<HashMap<Span, Binding>>,
}

pub fn method_key(method: &Rc<ast::MethodDecl>) -> usize {
    Rc::as_ptr(method) as *const () as usize
}

pub fn class_key(class: &ClassRef) -> usize {
    Rc::as_ptr(class) as *const () as usize
}
```

- [x] **Step 4: Wire into `lib.rs`**

In `crates/ns-runtime/src/lib.rs`, add `pub mod bytecode;` alongside the other `pub mod` lines (keep them alphabetical: `builtins`, `bytecode`, `dispatch`, ...). Update the crate doc comment's first line to "Name resolution, object model, tree-walking interpreter, and bytecode compiler/VM for the Newspeak3 language (Milestones 1-2)."

- [x] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-runtime bytecode::`
Expected: PASS (5 tests).

- [x] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): add the bytecode instruction set and compiled-code shapes

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 2: `compiler/lower.rs` — registers, scope stack, literals, locals

**Files:**
- Create: `crates/ns-runtime/src/compiler/lower.rs`
- Create: `crates/ns-runtime/src/compiler/mod.rs` (just enough to compile: `pub(crate) mod lower;`)
- Modify: `crates/ns-runtime/src/interpreter/eval.rs` (`parse_int_literal` becomes `pub(crate)`)
- Modify: `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Consumes: `ast::{Expr, Stmt, Literal, Ident}`, `Binding`, `bytecode::{Instr, Literal as BcLiteral, CallSite, CodeKind, CompiledCode}` (Task 1), `interpreter::eval::parse_int_literal` (M1, made `pub(crate)` here).
- Produces:
  - `pub(crate) enum ScopeKind { Frame, Inlined }`
  - `pub(crate) fn hops_for_depth(scopes: &[ScopeKind], depth: u16) -> u16`
  - `pub(crate) struct Lowerer<'a>` with `pub(crate) fn new(kind: CodeKind, num_args: u16, num_named_temps: u16, bindings: &'a Rc<HashMap<Span, Binding>>, enclosing_scopes: Vec<ScopeKind>) -> Self`, `pub(crate) fn lower_statement(&mut self, stmt: &Stmt)`, `pub(crate) fn lower_expr(&mut self, expr: &Expr) -> u16`, `pub(crate) fn finish(self, source: Span) -> CompiledCode`.
  - Tasks 3-4 extend `lower_expr`'s `match` arms in place and add `lower_slot_init`/`finish_body`; Task 5 wraps `Lowerer` into `compile_*`; Task 11 adds the inlining methods.

**Design notes:**
- **Register numbering** is the unified args-then-temps space from Task 1: `base = num_args + num_named_temps` is the first scratch register; a bump allocator `next_scratch` starts at `base` and is reset to `base` at the top of every *top-level* statement (`lower_statement`'s first action). Named locals are never allocated by `Lowerer` — parameter `i` is register `i`, named temp `j` is register `num_args + j`, exactly the numbering the resolver's `Binding::Local` already assumes.
- `lower_expr` returns the register holding the expression's value; `Expr::Ident(Binding::Local(i))` returns `i` directly with **no** `Move` emitted.
- **The scope stack** (spec: "Lexical addressing inside an inlined block"): the resolver counts *scopes* (every block literal is one), the VM walks *frames* (`captured_frame` hops), and an inlined block (Task 11) has a scope but no frame. `scopes` is innermost-last; `Lowerer::new` pushes `ScopeKind::Frame` for the body being compiled on top of the enclosing chain it is given. `hops_for_depth(scopes, d)` = the number of `Frame` entries among the innermost `d` scopes — the runtime `captured_frame` hop count for a `Binding::OuterLocal { depth: d, .. }`. A hop count of `0` means "the current frame's register `index`" (the same no-instruction case as `Binding::Local`). Until Task 11 no `Inlined` scope is ever pushed, so hops always equal the resolver's depth — the machinery is built now so Tasks 3-5 route every local reference through it and Task 11 only has to push/pop.
- `Binding::Local` while the innermost scope is `Inlined` is a compiler invariant violation (the inlining gate admits no names there) — `panic!`.
- `Expr::Send`/`Expr::Cascade`/`Expr::Block`/`Expr::DynamicArray` and `Stmt::Return` are `unimplemented!()` stubs in this task (Tasks 3-4 fill them in) — the same incremental-stub convention Milestone 1's lexer plan used.
- `parse_int_literal` is reused, not duplicated: it becomes `pub(crate)` in `eval.rs` (Global Constraints). Its `Err(message)` becomes `Literal::MalformedInt(text)`; the VM re-runs it for the identical error.

- [x] **Step 1: Make `parse_int_literal` `pub(crate)`**

In `crates/ns-runtime/src/interpreter/eval.rs`, change `fn parse_int_literal(text: &str) -> Result<i64, String>` to `pub(crate) fn parse_int_literal(text: &str) -> Result<i64, String>`. No other change.

- [x] **Step 2: Write the failing tests**

`crates/ns-runtime/src/compiler/lower.rs`:

```rust
//! `Expr`/`Stmt` -> `Instr` lowering, register allocation, and the scope
//! stack that maps resolver depths onto runtime frame hops. See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! `bytecode.rs` and "Control-flow inlining" sections.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instr, Literal as BcLiteral};
use crate::interpreter::eval::parse_int_literal;
use crate::types::Binding;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Parses `src` (a full, minimal `class ... ( ) ( <methods> ) : ()`
    /// module), resolves it for real via `crate::resolve`, and returns the
    /// module's instance-side method `index` plus the real bindings map it
    /// resolved under -- the same "parse a throwaway module" pattern
    /// `dispatch.rs`'s own tests use, so these tests exercise real
    /// `Binding`s rather than hand-built ones.
    pub(super) fn resolve_method(src: &str, index: usize) -> (ast::MethodDecl, Rc<HashMap<Span, Binding>>) {
        let module = ns_syntax::parse(src).expect("parse");
        let resolved =
            crate::resolve(module, Rc::from(src), crate::SourceOrigin::File(PathBuf::from("t.ns")))
                .expect("resolve");
        let method = resolved.module.root_class.instance_side.methods[index].clone();
        (method, resolved.bindings.clone())
    }

    pub(super) fn method_lowerer<'a>(method: &ast::MethodDecl, bindings: &'a Rc<HashMap<Span, Binding>>) -> Lowerer<'a> {
        let num_args = match &method.pattern {
            ast::MessagePattern::Unary { .. } => 0,
            ast::MessagePattern::Binary { .. } => 1,
            ast::MessagePattern::Keyword { parts } => parts.len() as u16,
        };
        Lowerer::new(CodeKind::Method, num_args, method.body.temps.len() as u16, bindings, vec![])
    }

    fn return_expr(method: &ast::MethodDecl) -> &Expr {
        let Stmt::Return(expr) = &method.body.stmts[0] else {
            panic!("expected a return statement");
        };
        expr
    }

    #[test]
    fn a_literal_returns_a_fresh_scratch_register() {
        let (method, bindings) =
            resolve_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^42 )\n) : ()", 0);
        let mut lowerer = method_lowerer(&method, &bindings); // 0 params, 0 temps -> base 0
        let reg = lowerer.lower_expr(return_expr(&method));
        assert_eq!(reg, 0);
        let code = lowerer.finish(method.span);
        assert_eq!(code.instrs, vec![Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(42) }]);
        assert_eq!(code.num_registers, 1);
        assert_eq!(code.kind, CodeKind::Method);
    }

    #[test]
    fn reading_a_parameter_returns_its_own_register_with_no_extra_instruction() {
        let (method, bindings) =
            resolve_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^x )\n) : ()", 0);
        let mut lowerer = method_lowerer(&method, &bindings); // 1 param (register 0)
        let reg = lowerer.lower_expr(return_expr(&method));
        assert_eq!(reg, 0, "reading a param should return its own register");
        let code = lowerer.finish(method.span);
        assert!(code.instrs.is_empty(), "no instruction needed to read a param");
        assert_eq!(code.num_args, 1);
    }

    #[test]
    fn assigning_a_named_temp_emits_store_local_and_records_the_statement_range() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | t | t := 7. ^t )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // temp `t` is register 0, base 1
        lowerer.lower_statement(&method.body.stmts[0]); // `t := 7.`
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(7) },
                Instr::StoreLocal { src: 1, slot: 0 },
            ]
        );
        assert_eq!(code.num_registers, 2);
        assert_eq!(code.statement_pc, vec![(0, 0..2)]);
    }

    #[test]
    fn scratch_registers_reset_between_statements() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | t | t := 1. t := 2. ^t )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_statement(&method.body.stmts[0]);
        lowerer.lower_statement(&method.body.stmts[1]);
        let code = lowerer.finish(method.span);
        // Both literals land in scratch register 1 -- the allocator reset
        // between statements instead of growing.
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(1) },
                Instr::StoreLocal { src: 1, slot: 0 },
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(2) },
                Instr::StoreLocal { src: 1, slot: 0 },
            ]
        );
        assert_eq!(code.num_registers, 2);
        assert_eq!(code.statement_pc, vec![(0, 0..2), (1, 2..4)]);
    }

    #[test]
    fn an_out_of_range_integer_literal_lowers_to_malformed_int_not_a_panic() {
        let (method, bindings) = resolve_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^99999999999999999999 )\n) : ()",
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_expr(return_expr(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(
            code.instrs,
            vec![Instr::LoadLiteral {
                dst: 0,
                literal: BcLiteral::MalformedInt("99999999999999999999".to_string()),
            }]
        );
    }

    #[test]
    fn a_radix_literal_is_parsed_at_compile_time() {
        let (method, bindings) =
            resolve_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^16rFF )\n) : ()", 0);
        let mut lowerer = method_lowerer(&method, &bindings);
        lowerer.lower_expr(return_expr(&method));
        let code = lowerer.finish(method.span);
        assert_eq!(code.instrs, vec![Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(255) }]);
    }

    #[test]
    fn hops_count_only_frame_scopes_within_the_depth() {
        use ScopeKind::*;
        // [Frame, Inlined, Frame] innermost-last: from the innermost frame,
        // resolver depth 2 (block -> inlined -> method) crosses one frame.
        assert_eq!(hops_for_depth(&[Frame, Inlined, Frame], 2), 1);
        // From directly inside an inlined block, depth 1 is the enclosing
        // frame's own registers: zero hops.
        assert_eq!(hops_for_depth(&[Frame, Inlined], 1), 0);
        // No inlining anywhere: hops == depth.
        assert_eq!(hops_for_depth(&[Frame, Frame, Frame], 2), 2);
        assert_eq!(hops_for_depth(&[Frame], 0), 0);
    }
}
```

- [x] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: FAIL — `Lowerer`/`ScopeKind`/`hops_for_depth` not found.

- [x] **Step 4: Write the implementation**

Insert into `crates/ns-runtime/src/compiler/lower.rs` before the `#[cfg(test)]` module:

```rust
/// One entry of the compile-time scope stack. The resolver counts every
/// block literal as one scope; at run time only `Frame` scopes cost a
/// `captured_frame` hop -- an `Inlined` scope (Task 11) has no frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Frame,
    Inlined,
}

/// The runtime `captured_frame` hop count for a resolver depth of `depth`
/// scopes: the number of `Frame` scopes among the innermost `depth`
/// entries of `scopes` (innermost last).
pub(crate) fn hops_for_depth(scopes: &[ScopeKind], depth: u16) -> u16 {
    scopes
        .iter()
        .rev()
        .take(depth as usize)
        .filter(|s| **s == ScopeKind::Frame)
        .count() as u16
}

pub(crate) struct Lowerer<'a> {
    bindings: &'a Rc<HashMap<Span, Binding>>,
    kind: CodeKind,
    num_args: u16,
    /// Innermost last. The last entry is this body's own `Frame` scope
    /// until Task 11 pushes `Inlined` entries above it.
    scopes: Vec<ScopeKind>,
    instrs: Vec<Instr>,
    base: u16,
    next_scratch: u16,
    max_reg: u16,
    statement_index: usize,
    statement_pc: Vec<(usize, Range<usize>)>,
    call_sites: Vec<CallSite>,
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(
        kind: CodeKind,
        num_args: u16,
        num_named_temps: u16,
        bindings: &'a Rc<HashMap<Span, Binding>>,
        mut enclosing_scopes: Vec<ScopeKind>,
    ) -> Self {
        enclosing_scopes.push(ScopeKind::Frame);
        let base = num_args + num_named_temps;
        Lowerer {
            bindings,
            kind,
            num_args,
            scopes: enclosing_scopes,
            instrs: Vec::new(),
            base,
            next_scratch: base,
            max_reg: base,
            statement_index: 0,
            statement_pc: Vec::new(),
            call_sites: Vec::new(),
        }
    }

    fn alloc_scratch(&mut self) -> u16 {
        let r = self.next_scratch;
        self.next_scratch += 1;
        self.max_reg = self.max_reg.max(self.next_scratch);
        r
    }

    fn emit(&mut self, instr: Instr) -> usize {
        self.instrs.push(instr);
        self.instrs.len() - 1
    }

    /// Lowers one of the body's own statements, recording its safepoint
    /// range in `statement_pc` and resetting the scratch allocator first
    /// (statements are the safepoint/scratch-liveness boundary).
    pub(crate) fn lower_statement(&mut self, stmt: &Stmt) {
        self.next_scratch = self.base;
        let start = self.instrs.len();
        match stmt {
            Stmt::Expr(e) => {
                self.lower_expr(e);
            }
            Stmt::Return(_) => unimplemented!("Stmt::Return is lowered starting in Task 4"),
        }
        let end = self.instrs.len();
        self.statement_pc.push((self.statement_index, start..end));
        self.statement_index += 1;
    }

    /// Returns the register holding `expr`'s value.
    pub(crate) fn lower_expr(&mut self, expr: &Expr) -> u16 {
        match expr {
            Expr::Literal(lit) => {
                let dst = self.alloc_scratch();
                self.emit(Instr::LoadLiteral { dst, literal: lower_literal(lit) });
                dst
            }
            Expr::Ident(id) => self.lower_ident_read(id),
            Expr::Assign { target, value, .. } => self.lower_assign(target, value),
            Expr::Send(_) | Expr::Cascade(_) => {
                unimplemented!("sends/cascades are lowered starting in Task 3")
            }
            Expr::Block(_) => unimplemented!("blocks are lowered starting in Task 4"),
            Expr::DynamicArray { .. } => unimplemented!("arrays are lowered starting in Task 4"),
        }
    }

    /// Maps a resolver `Binding::Local`/`OuterLocal` onto the runtime
    /// register file: `(hops, index)`, where `hops == 0` means "register
    /// `index` of the current frame".
    fn local_target(&self, depth: u16, index: u16) -> (u16, u16) {
        if depth == 0 {
            assert!(
                self.scopes.last() == Some(&ScopeKind::Frame),
                "internal: Binding::Local inside an inlined block (the inlining gate admits no names there)"
            );
        }
        (hops_for_depth(&self.scopes, depth), index)
    }

    fn lower_ident_read(&mut self, id: &ast::Ident) -> u16 {
        if id.name == "self" {
            let dst = self.alloc_scratch();
            self.emit(Instr::LoadSelf { dst });
            return dst;
        }
        match self.bindings.get(&id.span) {
            Some(Binding::Local(slot)) => {
                let (_, index) = self.local_target(0, *slot);
                index
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let (hops, index) = self.local_target(*depth, *index);
                if hops == 0 {
                    return index;
                }
                let dst = self.alloc_scratch();
                self.emit(Instr::LoadOuterLocal { dst, depth: hops, index });
                dst
            }
            Some(Binding::LexicalSend { .. }) | Some(Binding::SuperSend) | None => {
                unimplemented!("implicit/lexical/builtin-name reads are lowered starting in Task 3")
            }
        }
    }

    fn lower_assign(&mut self, target: &ast::Ident, value: &Expr) -> u16 {
        let v = self.lower_expr(value);
        match self.bindings.get(&target.span) {
            Some(Binding::Local(slot)) => {
                let (_, index) = self.local_target(0, *slot);
                self.emit(Instr::StoreLocal { src: v, slot: index });
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let (hops, index) = self.local_target(*depth, *index);
                if hops == 0 {
                    self.emit(Instr::StoreLocal { src: v, slot: index });
                } else {
                    self.emit(Instr::StoreOuterLocal { src: v, depth: hops, index });
                }
            }
            Some(Binding::LexicalSend { .. }) | Some(Binding::SuperSend) | None => {
                unimplemented!("setter-send assignment is lowered starting in Task 3")
            }
        }
        // An assignment's own value is `value`'s value (runtime spec's
        // "Setter sends" note) -- already sitting in register `v`.
        v
    }

    pub(crate) fn finish(self, source: Span) -> CompiledCode {
        CompiledCode {
            kind: self.kind,
            instrs: self.instrs,
            num_args: self.num_args,
            num_registers: self.max_reg,
            statement_pc: self.statement_pc,
            call_sites: self.call_sites,
            source,
        }
    }
}

fn lower_literal(lit: &ast::Literal) -> BcLiteral {
    match lit {
        // Parsed once, here, with the shared M1 parser. A literal it rejects
        // is a Newspeak-program error M1 raises at evaluation time, so it is
        // deferred (`MalformedInt`), never a compile-time panic.
        ast::Literal::Int { text, .. } => match parse_int_literal(text) {
            Ok(n) => BcLiteral::SmallInt(n),
            Err(_) => BcLiteral::MalformedInt(text.clone()),
        },
        ast::Literal::Float { text, .. } => BcLiteral::Float(text.parse().unwrap_or(f64::NAN)),
        ast::Literal::String { value, .. } => BcLiteral::Str(value.clone()),
        ast::Literal::Symbol { value, .. } => BcLiteral::Sym(crate::Symbol::intern(value)),
        ast::Literal::Char { value, .. } => BcLiteral::Char(*value),
        ast::Literal::Bool { value, .. } => BcLiteral::Bool(*value),
        ast::Literal::Nil { .. } => BcLiteral::Nil,
        ast::Literal::Array { elements, .. } => {
            BcLiteral::Array(elements.iter().map(lower_literal).collect())
        }
        ast::Literal::Pattern { .. } => BcLiteral::Nil, // Non-Goal, matches eval_literal
    }
}
```

`crates/ns-runtime/src/compiler/mod.rs`:

```rust
//! Bytecode compiler entry points. See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`.

pub(crate) mod lower;
```

- [x] **Step 5: Wire into `lib.rs`**

Add `pub mod compiler;` to `crates/ns-runtime/src/lib.rs` (alphabetical, after `bytecode`).

- [x] **Step 6: Run tests to verify they pass**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: PASS (7 tests). Also run `cargo test -p ns-runtime` — every M1 test still passes (`parse_int_literal`'s visibility change is behavior-neutral).

- [x] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): lower literals and local variable access to bytecode

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 3: `compiler/lower.rs` — sends, cascades, setters, builtin-name reads

**Files:**
- Modify: `crates/ns-runtime/src/compiler/lower.rs`

**Interfaces:**
- Consumes: `Binding` variants (unchanged), `crate::interpreter::instantiate::builtin_kind_from_name` (already `pub(crate)`), `types::BuiltinClass`.
- Produces: `lower_expr` now handles `Expr::Send`/`Expr::Cascade`; `lower_ident_read`/`lower_assign` handle every `Binding` variant; `fn emit_ordinary_send_with_receiver(&mut self, receiver_reg: u16, message: &ast::MessageSend) -> u16` (Task 11 reuses it for the inlining fallback); `fn lower_args(&mut self, message: &ast::MessageSend) -> Vec<u16>`; module-level `fn selector_of(message: &ast::MessageSend) -> Symbol`.

**Design notes:**
- Cascade messages are **always** ordinary (`SendPublic`)-shaped: `ast::Cascade`'s `first`/`rest` are bare `MessageSend`s with no receiver expression of their own (confirmed against `eval.rs::eval_cascade`, which always calls `interp.send(...)` and never consults `bindings` for a cascade message's span). The cascade's value is the *last* message's result, as in `eval_cascade`.
- An ordinary send evaluates receiver, then arguments left to right, then sends — `eval_send`'s order.
- `lower_send`'s ordinary arm is written as `receiver` + `emit_ordinary_send_with_receiver` on purpose: Task 11 inserts the `control_flow::recognize` check in front of exactly that arm.
- A bare identifier with no binding and no builtin-name match is a resolver invariant violation → `panic!` (contrast `eval_ident`, which returns an "internal:" `NewspeakError` because it re-evaluates at run time; the compiler sees output the resolver already validated).

- [x] **Step 1: Write the failing tests**

Add to `crates/ns-runtime/src/compiler/lower.rs`'s test module:

```rust
#[test]
fn an_ordinary_send_evaluates_receiver_then_args() {
    let (method, bindings) =
        resolve_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^3 + x )\n) : ()", 0);
    let mut lowerer = method_lowerer(&method, &bindings);
    let dst = lowerer.lower_expr(return_expr(&method));
    let code = lowerer.finish(method.span);
    assert_eq!(
        code.instrs,
        vec![
            Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(3) },
            Instr::SendPublic {
                selector: crate::Symbol::intern("+"),
                receiver_reg: 1,
                arg_regs: vec![0],
                dst: 2,
                call_site: CallSite { call_site_id: 1 },
            },
        ]
    );
    assert_eq!(dst, 2);
    assert_eq!(code.num_registers, 3);
    assert_eq!(code.call_sites, vec![CallSite { call_site_id: 1 }]);
}

#[test]
fn a_bare_implicit_send_lowers_to_send_lexical() {
    // `foo`'s body (`^bar`) resolves `bar` as an implicit-receiver unary
    // send at depth 0 (`bar` is a sibling method) -- `Binding::LexicalSend`.
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  bar = ( ^42 )\n  foo = ( ^bar )\n) : ()";
    let (foo, bindings) = resolve_method(src, 1);
    let mut lowerer = method_lowerer(&foo, &bindings);
    let dst = lowerer.lower_expr(return_expr(&foo));
    let code = lowerer.finish(foo.span);
    assert_eq!(
        code.instrs,
        vec![Instr::SendLexical {
            depth: 0,
            selector: crate::Symbol::intern("bar"),
            arg_regs: vec![],
            dst: 0,
            call_site: CallSite { call_site_id: 0 },
        }]
    );
    assert_eq!(dst, 0);
}

#[test]
fn a_keyword_implicit_send_and_a_super_send_lower_without_a_receiver_register() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  at: i put: v = ( ^v )\n  foo = ( at: 1 put: 2. ^super printString )\n) : ()";
    let (foo, bindings) = resolve_method(src, 1);
    let mut lowerer = method_lowerer(&foo, &bindings);
    lowerer.lower_statement(&foo.body.stmts[0]);
    let code = lowerer.finish(foo.span);
    assert!(matches!(
        &code.instrs[2],
        Instr::SendLexical { depth: 0, selector, arg_regs, .. }
            if selector.as_str() == "at:put:" && arg_regs.len() == 2
    ));
    let (foo, bindings) = resolve_method(src, 1);
    let mut lowerer = method_lowerer(&foo, &bindings);
    lowerer.lower_expr(match &foo.body.stmts[1] {
        Stmt::Return(e) => e,
        _ => panic!(),
    });
    let code = lowerer.finish(foo.span);
    assert!(matches!(
        &code.instrs[0],
        Instr::SendSuper { selector, arg_regs, dst: 0, .. }
            if selector.as_str() == "printString" && arg_regs.is_empty()
    ));
}

#[test]
fn a_slot_assignment_lowers_to_a_setter_lexical_send() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n  | n ::= 0. |\n) (\n  bump = ( n := n + 1 )\n) : ()";
    let (bump, bindings) = resolve_method(src, 0);
    let mut lowerer = method_lowerer(&bump, &bindings);
    lowerer.lower_statement(&bump.body.stmts[0]);
    let code = lowerer.finish(bump.span);
    assert!(matches!(
        code.instrs.last(),
        Some(Instr::SendLexical { depth: 0, selector, arg_regs, .. })
            if selector.as_str() == "n:" && arg_regs.len() == 1
    ));
}

#[test]
fn a_bare_builtin_class_name_lowers_to_load_builtin_class() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^OrderedCollection )\n) : ()";
    let (foo, bindings) = resolve_method(src, 0);
    let mut lowerer = method_lowerer(&foo, &bindings);
    let dst = lowerer.lower_expr(return_expr(&foo));
    let code = lowerer.finish(foo.span);
    assert_eq!(
        code.instrs,
        vec![Instr::LoadBuiltinClass { dst: 0, kind: crate::types::BuiltinClass::OrderedCollection }]
    );
    assert_eq!(dst, 0);
}

#[test]
fn a_cascade_reuses_one_receiver_register_across_every_message() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | c | c := OrderedCollection new. c add: 1; add: 2 )\n) : ()";
    let (foo, bindings) = resolve_method(src, 0);
    let mut lowerer = method_lowerer(&foo, &bindings); // temp `c` is register 0
    lowerer.lower_statement(&foo.body.stmts[0]);
    lowerer.lower_statement(&foo.body.stmts[1]);
    let code = lowerer.finish(foo.span);
    let receiver_regs: Vec<u16> = code
        .instrs
        .iter()
        .filter_map(|i| match i {
            Instr::SendPublic { receiver_reg, .. } => Some(*receiver_reg),
            _ => None,
        })
        .collect();
    // `OrderedCollection new`, then the cascade's two `add:` sends.
    assert_eq!(receiver_regs.len(), 3);
    assert_eq!(receiver_regs[1], 0, "`c` is register 0, read with no instruction");
    assert_eq!(receiver_regs[1], receiver_regs[2], "both cascade sends share one receiver register");
    assert_eq!(code.call_sites.len(), 3);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: FAIL — the `unimplemented!()` stubs panic.

- [x] **Step 3: Write the implementation**

In `crates/ns-runtime/src/compiler/lower.rs`, replace the `Some(Binding::LexicalSend { .. }) | Some(Binding::SuperSend) | None` arm of `lower_ident_read` with:

```rust
Some(Binding::LexicalSend { depth }) => {
    let depth = *depth;
    self.emit_lexical_send(depth, crate::Symbol::intern(&id.name), vec![])
}
// M1's resolver never attaches `SuperSend` to a bare identifier (only to
// a `super m` send's own span) -- grouped with `None` here because both
// fall to the same builtin-name fallback, mirroring `eval_ident`.
Some(Binding::SuperSend) | None => {
    match crate::interpreter::instantiate::builtin_kind_from_name(&id.name) {
        Some(kind) => {
            let dst = self.alloc_scratch();
            self.emit(Instr::LoadBuiltinClass { dst, kind });
            dst
        }
        None => panic!(
            "internal: unresolved identifier `{}` (the resolver should have rejected this)",
            id.name
        ),
    }
}
```

Replace the same arm of `lower_assign` with:

```rust
Some(Binding::LexicalSend { depth }) => {
    let depth = *depth;
    let setter = crate::Symbol::intern(&format!("{}:", target.name));
    self.emit_lexical_send(depth, setter, vec![v]);
}
Some(Binding::SuperSend) | None => panic!(
    "internal: unresolved setter target `{}` (the resolver should have rejected this)",
    target.name
),
```

Replace the `Expr::Send(_) | Expr::Cascade(_)` arm of `lower_expr` with:

```rust
Expr::Send(send) => self.lower_send(send),
Expr::Cascade(cascade) => self.lower_cascade(cascade),
```

Add to `impl<'a> Lowerer<'a>`:

```rust
/// One `Send*` instruction's `CallSite`: the instruction's own
/// about-to-be-pushed index, always in sync with `instrs`.
fn next_call_site(&self) -> CallSite {
    CallSite { call_site_id: self.instrs.len() as u16 }
}

fn emit_send(&mut self, build: impl FnOnce(u16, CallSite) -> Instr) -> u16 {
    let dst = self.alloc_scratch();
    let call_site = self.next_call_site();
    self.emit(build(dst, call_site));
    self.call_sites.push(call_site);
    dst
}

fn emit_lexical_send(&mut self, depth: u16, selector: crate::Symbol, arg_regs: Vec<u16>) -> u16 {
    self.emit_send(|dst, call_site| Instr::SendLexical { depth, selector, arg_regs, dst, call_site })
}

pub(crate) fn emit_ordinary_send_with_receiver(&mut self, receiver_reg: u16, message: &ast::MessageSend) -> u16 {
    let selector = selector_of(message);
    let arg_regs = self.lower_args(message);
    self.emit_send(|dst, call_site| Instr::SendPublic { selector, receiver_reg, arg_regs, dst, call_site })
}

fn lower_args(&mut self, message: &ast::MessageSend) -> Vec<u16> {
    message.args.iter().map(|a| self.lower_expr(a)).collect()
}

fn lower_send(&mut self, send: &ast::Send) -> u16 {
    match self.bindings.get(&send.span) {
        Some(Binding::SuperSend) => {
            let selector = selector_of(&send.message);
            let arg_regs = self.lower_args(&send.message);
            self.emit_send(|dst, call_site| Instr::SendSuper { selector, arg_regs, dst, call_site })
        }
        Some(Binding::LexicalSend { depth }) => {
            let depth = *depth;
            let selector = selector_of(&send.message);
            let arg_regs = self.lower_args(&send.message);
            self.emit_lexical_send(depth, selector, arg_regs)
        }
        _ => {
            let receiver_reg = self.lower_expr(&send.receiver);
            self.emit_ordinary_send_with_receiver(receiver_reg, &send.message)
        }
    }
}

fn lower_cascade(&mut self, cascade: &ast::Cascade) -> u16 {
    let receiver_reg = self.lower_expr(&cascade.receiver);
    let mut result = self.emit_ordinary_send_with_receiver(receiver_reg, &cascade.first);
    for m in &cascade.rest {
        result = self.emit_ordinary_send_with_receiver(receiver_reg, m);
    }
    result
}
```

Add at module scope (outside the `impl`):

```rust
pub(crate) fn selector_of(message: &ast::MessageSend) -> crate::Symbol {
    match &message.selector {
        ast::Selector::Unary(s) | ast::Selector::Binary(s) | ast::Selector::Keyword(s) => {
            crate::Symbol::intern(s)
        }
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: PASS (13 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): lower sends, cascades, setters, and builtin class names to bytecode

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 4: `compiler/lower.rs` — returns per code kind, block literals, arrays

**Files:**
- Modify: `crates/ns-runtime/src/compiler/lower.rs`

**Interfaces:**
- Consumes: `ast::BlockExpr`, `ast::Expr::DynamicArray` (unchanged).
- Produces: `Lowerer::finish_body(&mut self, stmts: &[Stmt])`, `Lowerer::lower_slot_init(&mut self, init: &Expr, field_index: u32)`, `fn emit_nil(&mut self) -> u16`, `fn return_instr(&self, reg: u16) -> Option<Instr>`. `lower_expr` now handles every `Expr` variant and `lower_statement` every `Stmt`; no `unimplemented!()` stubs remain in `lower.rs`.

**Design notes:**
- **A `^` compiles to whatever the code kind says** (spec: "Non-local return", "Class instantiation", "Control-flow inlining"): `Method` → `Return`; `Block` → `NonLocalReturn`; `Initializer` at the top level → *no* return instruction (evaluate, discard — M1's `run_own_init_exprs` treats `Stmt::Return` as `Stmt::Expr`); `Initializer` inside an inlined block (Task 11) → `NonLocalReturn`, because M1 lets that `Signal::Return` escape `instantiate` and both engines must agree. `return_instr` encodes exactly this table; "inside an inlined block" is `scopes.last() == Inlined`.
- **`finish_body` makes every body end in a terminating instruction** so `vm::run_code` never handles "fell off the end": a method (or initializer) body that did not end in an explicit `^` gets `LoadSelf` + `ImplicitReturn` (methods implicitly return `self`, spec §3.5; an initializer always does — its top-level `^` never emits a return); a block body gets `ImplicitReturn` of its last statement's value register (`nil` for an empty block, matching `invoke_closure`'s `last = Value::Nil`).
- **A block literal is compiled by a nested `Lowerer`** (`kind: Block`, `num_args` = parameter count, `num_named_temps` = the block's own temps, `enclosing_scopes` = the enclosing lowerer's *whole* scope stack) into its own `CompiledCode`, carried by `MakeClosure { code }`. Nested block literals nest again. That is the spec's "Blocks and closures" section made concrete: a block's `Binding::OuterLocal { depth, index }` becomes `LoadOuterLocal` with the scope stack's hop count, which equals `depth` until Task 11 introduces inlined scopes.
- `last_expr_reg` records where the most recent top-level `Stmt::Expr`'s value landed, for `finish_body`'s block case and (Task 11) for reading an inlined block's result.
- `lower_slot_init` is the initializer's per-slot "statement": the initializer expression, then `InitSlot { src, field_index }`, recorded as its own `statement_pc` entry — spec: "every slot initializer and every `initExpr` counts as one statement".

- [x] **Step 1: Write the failing tests**

Add to `crates/ns-runtime/src/compiler/lower.rs`'s test module:

```rust
fn lower_whole_method(src: &str, index: usize) -> CompiledCode {
    let (method, bindings) = resolve_method(src, index);
    let mut lowerer = method_lowerer(&method, &bindings);
    for stmt in &method.body.stmts {
        lowerer.lower_statement(stmt);
    }
    lowerer.finish_body(&method.body.stmts);
    lowerer.finish(method.span)
}

#[test]
fn a_method_with_no_explicit_return_implicitly_returns_self() {
    let code = lower_whole_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( 1 + 1 )\n) : ()", 0);
    assert_eq!(
        &code.instrs[code.instrs.len() - 2..],
        &[Instr::LoadSelf { dst: 3 }, Instr::ImplicitReturn { reg: 3 }]
    );
}

#[test]
fn an_explicit_method_return_is_a_plain_return_with_nothing_appended() {
    let code = lower_whole_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()", 0);
    assert_eq!(
        code.instrs,
        vec![
            Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
            Instr::Return { reg: 0 },
        ]
    );
}

#[test]
fn a_block_literal_compiles_to_its_own_code_carried_by_make_closure() {
    let code = lower_whole_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^[ :x | x ] )\n) : ()", 0);
    let Some(Instr::MakeClosure { dst: 0, params, code: block }) = code.instrs.first() else {
        panic!("expected a MakeClosure instruction, got {:?}", code.instrs);
    };
    assert_eq!(params.as_slice(), &[crate::Symbol::intern("x")]);
    assert_eq!(block.kind, CodeKind::Block);
    assert_eq!(block.num_args, 1);
    // `x` is the block's own register 0: read with no instruction, then
    // returned as the block's implicit value.
    assert_eq!(block.instrs, vec![Instr::ImplicitReturn { reg: 0 }]);
    assert_eq!(block.statement_pc, vec![(0, 0..0)]);
}

#[test]
fn a_block_reads_and_writes_enclosing_locals_through_outer_local_instructions() {
    let code = lower_whole_method(
        "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( | t | ^[ t := x ] )\n) : ()",
        0,
    );
    let Some(Instr::MakeClosure { code: block, .. }) = code.instrs.first() else { panic!() };
    assert_eq!(
        block.instrs,
        vec![
            Instr::LoadOuterLocal { dst: 0, depth: 1, index: 0 }, // x
            Instr::StoreOuterLocal { src: 0, depth: 1, index: 1 }, // t
            Instr::ImplicitReturn { reg: 0 },
        ]
    );
}

#[test]
fn a_doubly_nested_block_counts_two_frame_hops() {
    let code = lower_whole_method(
        "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^[ [ x ] ] )\n) : ()",
        0,
    );
    let Some(Instr::MakeClosure { code: outer, .. }) = code.instrs.first() else { panic!() };
    let Some(Instr::MakeClosure { code: inner, .. }) = outer.instrs.first() else { panic!() };
    assert_eq!(inner.instrs[0], Instr::LoadOuterLocal { dst: 0, depth: 2, index: 0 });
}

#[test]
fn a_return_inside_a_block_is_a_non_local_return() {
    let code = lower_whole_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^[ ^1 ] )\n) : ()", 0);
    let Some(Instr::MakeClosure { code: block, .. }) = code.instrs.first() else { panic!() };
    assert_eq!(
        block.instrs,
        vec![
            Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
            Instr::NonLocalReturn { reg: 0 },
        ]
    );
}

#[test]
fn an_empty_block_returns_nil_and_a_multi_statement_block_returns_its_last_value() {
    let code = lower_whole_method(
        "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( [ ] value. ^[ 1. 2 ] value )\n) : ()",
        0,
    );
    let Some(Instr::MakeClosure { code: empty, .. }) = code.instrs.first() else { panic!() };
    assert_eq!(
        empty.instrs,
        vec![Instr::LoadLiteral { dst: 0, literal: BcLiteral::Nil }, Instr::ImplicitReturn { reg: 0 }]
    );
    let Some(Instr::MakeClosure { code: two, .. }) = code.instrs.get(2) else { panic!("{:?}", code.instrs) };
    assert_eq!(
        two.instrs,
        vec![
            Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
            Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(2) },
            Instr::ImplicitReturn { reg: 0 },
        ]
    );
}

#[test]
fn a_dynamic_array_literal_lowers_to_make_array() {
    let (method, bindings) =
        resolve_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^{ 1. 2. 3 } )\n) : ()", 0);
    let mut lowerer = method_lowerer(&method, &bindings);
    let dst = lowerer.lower_expr(return_expr(&method));
    let code = lowerer.finish(method.span);
    assert_eq!(
        code.instrs,
        vec![
            Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
            Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(2) },
            Instr::LoadLiteral { dst: 2, literal: BcLiteral::SmallInt(3) },
            Instr::MakeArray { dst: 3, elem_regs: vec![0, 1, 2] },
        ]
    );
    assert_eq!(dst, 3);
}

#[test]
fn an_initializer_discards_a_top_level_return_and_always_returns_self() {
    let (method, bindings) = resolve_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()", 0);
    // Reuse `foo`'s `^1` as if it were an initExpr: kind Initializer, 0 args.
    let mut lowerer = Lowerer::new(CodeKind::Initializer, 0, 0, &bindings, vec![]);
    lowerer.lower_statement(&method.body.stmts[0]);
    lowerer.finish_body(&method.body.stmts);
    let code = lowerer.finish(method.span);
    assert_eq!(
        code.instrs,
        vec![
            Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
            Instr::LoadSelf { dst: 1 },
            Instr::ImplicitReturn { reg: 1 },
        ]
    );
}

#[test]
fn a_slot_initializer_is_one_statement_ending_in_init_slot() {
    let (method, bindings) = resolve_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^6 * 7 )\n) : ()", 0);
    let mut lowerer = Lowerer::new(CodeKind::Initializer, 0, 0, &bindings, vec![]);
    lowerer.lower_slot_init(return_expr(&method), 4);
    let code = lowerer.finish(method.span);
    // `6`, `7`, the `*` send, then the field write: four instructions.
    assert!(matches!(code.instrs.last(), Some(Instr::InitSlot { src: 2, field_index: 4 })));
    assert_eq!(code.statement_pc, vec![(0, 0..4)]);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: FAIL — `finish_body`/`lower_slot_init` not found; `Expr::Block`/`Expr::DynamicArray`/`Stmt::Return` still `unimplemented!()`.

- [x] **Step 3: Write the implementation**

In `crates/ns-runtime/src/compiler/lower.rs`, add the field `last_expr_reg: Option<u16>` to `Lowerer` (initialize it to `None` in `new`). Replace `lower_statement` with:

```rust
pub(crate) fn lower_statement(&mut self, stmt: &Stmt) {
    self.next_scratch = self.base;
    let start = self.instrs.len();
    self.lower_stmt_inner(stmt);
    let end = self.instrs.len();
    self.statement_pc.push((self.statement_index, start..end));
    self.statement_index += 1;
}

/// The initializer's per-slot statement: the slot's initializer
/// expression, then `InitSlot` (M1's `run_own_slots` writes the field
/// directly, never through a setter send). One `statement_pc` entry.
pub(crate) fn lower_slot_init(&mut self, init: &Expr, field_index: u32) {
    self.next_scratch = self.base;
    let start = self.instrs.len();
    let src = self.lower_expr(init);
    self.emit(Instr::InitSlot { src, field_index });
    let end = self.instrs.len();
    self.statement_pc.push((self.statement_index, start..end));
    self.statement_index += 1;
}

/// Lowers a statement without touching the allocator or the PC table --
/// shared by `lower_statement` and (Task 11) inlined block bodies. Returns
/// the value register for a `Stmt::Expr` (or a discarded initializer
/// `^`), `None` when a return instruction was emitted.
fn lower_stmt_inner(&mut self, stmt: &Stmt) -> Option<u16> {
    match stmt {
        Stmt::Expr(e) => {
            let reg = self.lower_expr(e);
            self.last_expr_reg = Some(reg);
            Some(reg)
        }
        Stmt::Return(e) => {
            let reg = self.lower_expr(e);
            match self.return_instr(reg) {
                Some(instr) => {
                    self.emit(instr);
                    self.last_expr_reg = None;
                    None
                }
                None => {
                    self.last_expr_reg = Some(reg);
                    Some(reg)
                }
            }
        }
    }
}

/// What a `^` means here -- the spec's per-code-kind table. `None` is
/// "evaluate and discard" (a top-level `^` in `initExprs`, exactly as M1's
/// `run_own_init_exprs`). Inside an inlined block (`scopes.last() ==
/// Inlined`, Task 11) an initializer's `^` must instead raise the same
/// `Signal::Return` M1's real block would, so it becomes `NonLocalReturn`.
fn return_instr(&self, reg: u16) -> Option<Instr> {
    let inside_inlined = self.scopes.last() == Some(&ScopeKind::Inlined);
    match (self.kind, inside_inlined) {
        (CodeKind::Method, _) => Some(Instr::Return { reg }),
        (CodeKind::Block, _) => Some(Instr::NonLocalReturn { reg }),
        (CodeKind::Initializer, false) => None,
        (CodeKind::Initializer, true) => Some(Instr::NonLocalReturn { reg }),
    }
}

fn emit_nil(&mut self) -> u16 {
    let dst = self.alloc_scratch();
    self.emit(Instr::LoadLiteral { dst, literal: BcLiteral::Nil });
    dst
}

/// Appends the body's terminating instruction if `stmts` (the body just
/// lowered) did not already end in one, so `vm::run_code` never has to
/// handle "fell off the end". A method returns `self`; an initializer
/// always returns `self` (its top-level `^` never emits a return); a block
/// returns its last statement's value, `nil` if it has none.
pub(crate) fn finish_body(&mut self, stmts: &[Stmt]) {
    let ends_in_return = matches!(stmts.last(), Some(Stmt::Return(_)));
    match self.kind {
        CodeKind::Method if ends_in_return => {}
        CodeKind::Method | CodeKind::Initializer => {
            let dst = self.alloc_scratch();
            self.emit(Instr::LoadSelf { dst });
            self.emit(Instr::ImplicitReturn { reg: dst });
        }
        CodeKind::Block if ends_in_return => {}
        CodeKind::Block => {
            let reg = match self.last_expr_reg {
                Some(r) => r,
                None => self.emit_nil(),
            };
            self.emit(Instr::ImplicitReturn { reg });
        }
    }
}

/// Compiles a block literal into its own `CompiledCode` with a nested
/// `Lowerer` that inherits this one's whole scope stack, and emits
/// `MakeClosure`. `captured_frame`/`home`/`bindings` are supplied by the
/// VM at run time (spec: "Blocks and closures").
fn lower_block_literal(&mut self, block: &ast::BlockExpr) -> u16 {
    let params: Vec<crate::Symbol> =
        block.params.iter().map(|p| crate::Symbol::intern(&p.name)).collect();
    let mut inner = Lowerer::new(
        CodeKind::Block,
        params.len() as u16,
        block.body.temps.len() as u16,
        self.bindings,
        self.scopes.clone(),
    );
    for stmt in &block.body.stmts {
        inner.lower_statement(stmt);
    }
    inner.finish_body(&block.body.stmts);
    let code = inner.finish(block.span);
    let dst = self.alloc_scratch();
    self.emit(Instr::MakeClosure { dst, params: Rc::new(params), code: Rc::new(code) });
    dst
}
```

Replace the `Expr::Block`/`Expr::DynamicArray` arms of `lower_expr` with:

```rust
Expr::Block(block) => self.lower_block_literal(block),
Expr::DynamicArray { elements, .. } => {
    let elem_regs: Vec<u16> = elements.iter().map(|e| self.lower_expr(e)).collect();
    let dst = self.alloc_scratch();
    self.emit(Instr::MakeArray { dst, elem_regs });
    dst
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: PASS (23 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): lower returns, block literals, and array literals to bytecode

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 5: `compiler/mod.rs` — `compile_method`, `compile_initializer`, `compile_module`

**Files:**
- Modify: `crates/ns-runtime/src/compiler/mod.rs`
- Create: `crates/ns-runtime/tests/compiler.rs`

**Interfaces:**
- Consumes: `Lowerer` (Tasks 2-4), `types::{ClassRef, ObjectData, Binding}`, `bytecode::{CompiledCode, CompiledModule, CodeKind, method_key, class_key}`.
- Produces (the entry points every later task depends on):
  - `pub fn compile_method(method: &ast::MethodDecl, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledCode`
  - `pub fn compile_initializer(class: &ClassRef, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledCode`
  - `pub fn compile_module(root_class: &ClassRef, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledModule`

**Design notes:**
- **Why the caches are keyed by `Rc` pointer identity, not names:** compilation happens *after* `Interpreter::load_module`/`load_class` built the real `ClassRef` graph — `load_class` mints each method's `Rc::new(m.clone())` once, and that exact `Rc` is what every `dispatch.rs` lookup returns via `Lookup::Method`; the root `ClassRef` is what `instantiate` walks. Keying by those pointers means the hooks look up exactly what dispatch handed them.
- **`compile_module` walks only `root_class`'s own `methods` and compiles only `root_class`'s initializer**: M1's `load_class` never loads nested classes and never links a user-defined superclass through the ordinary pipeline (documented M1 gaps in `instantiate.rs`), so the root class's own method map is the entire set of user methods dispatch can reach, and its initializer the only user-defined one `instantiate` can run. Extend the walk only when a later milestone loads more.
- **`compile_initializer` reads the *loaded* class**, not the AST: `ClassData.instance_slots` (declaration order, as `run_own_slots` iterates), `ClassData.slot_index` (the absolute field index `InitSlot` needs), `ClassData.init_exprs`, and `ClassData.factory` (the parameter count — `num_args`; the resolver scoped slot initializers and `initExprs` with exactly those parameters as `Binding::Local(i)`, `resolver/mod.rs`'s `resolve_class`). A class with no slots and no `initExprs` still compiles to `LoadSelf` + `ImplicitReturn` so `disasm --initializer` always has something to show; hook 3 skips such classes before looking them up.
- The initializer's `source` span is the merge of every lowered slot's and `initExpr`'s span (`Span::new(0, 0)` when there is nothing) — `ClassData` carries no span of its own.

- [x] **Step 1: Write the failing tests**

`crates/ns-runtime/tests/compiler.rs`:

```rust
//! `compile_module` / `compile_initializer` over a real loaded class.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{class_key, method_key, CallSite, CodeKind, Instr, Literal};
use ns_runtime::{compiler, Binding, ClassRef, Interpreter, ObjectData, SourceOrigin, Symbol};
use ns_syntax::Span;

fn load(src: &str) -> (Interpreter, ClassRef, Rc<HashMap<Span, Binding>>) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved =
        ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns")))
            .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, bindings)
}

#[test]
fn compile_module_compiles_every_loaded_method_and_the_initializer() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n  bar = ( ^2 )\n) : ()";
    let (_interp, class, bindings) = load(src);
    let module = compiler::compile_module(&class, &bindings);
    assert_eq!(module.methods.len(), 2);
    for compiled in module.methods.values() {
        assert_eq!(compiled.kind, CodeKind::Method);
        assert!(matches!(compiled.instrs.last(), Some(Instr::Return { .. })));
    }
    let init = module.initializers.get(&class_key(&class)).expect("root initializer compiled");
    assert_eq!(init.kind, CodeKind::Initializer);
    assert_eq!(init.instrs, vec![Instr::LoadSelf { dst: 0 }, Instr::ImplicitReturn { reg: 0 }]);
    assert!(Rc::ptr_eq(&module.bindings, &bindings));
}

#[test]
fn compiled_method_is_reachable_by_its_dispatch_key() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()";
    let (_interp, class, bindings) = load(src);
    let module = compiler::compile_module(&class, &bindings);
    let method = {
        let borrowed = class.borrow();
        let ObjectData::Class(data) = &borrowed.data else { panic!() };
        data.methods.get(&Symbol::intern("foo")).unwrap().clone()
    };
    assert!(module.methods.contains_key(&method_key(&method)));
}

#[test]
fn compile_initializer_lowers_slots_then_init_exprs_with_absolute_field_indices() {
    let src = "Newspeak3\n'T'\nclass Foo x: a = Object new (\n  | p = a. q ::= 5. |\n  q := p.\n) (\n) : ()";
    let (_interp, class, bindings) = load(src);
    let init = compiler::compile_initializer(&class, &bindings);
    assert_eq!(init.kind, CodeKind::Initializer);
    assert_eq!(init.num_args, 1, "the factory parameter `a` is the one argument");
    assert_eq!(
        init.instrs,
        vec![
            // `p = a.` -- `a` is register 0, written straight into field 0.
            Instr::InitSlot { src: 0, field_index: 0 },
            // `q ::= 5.`
            Instr::LoadLiteral { dst: 1, literal: Literal::SmallInt(5) },
            Instr::InitSlot { src: 1, field_index: 1 },
            // `q := p.` -- slot reads/writes are lexical sends, as in M1.
            Instr::SendLexical {
                depth: 0,
                selector: Symbol::intern("p"),
                arg_regs: vec![],
                dst: 1,
                call_site: CallSite { call_site_id: 3 },
            },
            Instr::SendLexical {
                depth: 0,
                selector: Symbol::intern("q:"),
                arg_regs: vec![1],
                dst: 2,
                call_site: CallSite { call_site_id: 4 },
            },
            Instr::LoadSelf { dst: 3 },
            Instr::ImplicitReturn { reg: 3 },
        ]
    );
    assert_eq!(init.statement_pc, vec![(0, 0..1), (1, 1..3), (2, 3..5)]);
    assert_eq!(init.num_registers, 4);
}

#[test]
fn an_uninitialized_slot_emits_nothing() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n  | a. b = 1. |\n) (\n) : ()";
    let (_interp, class, bindings) = load(src);
    let init = compiler::compile_initializer(&class, &bindings);
    let init_slots: Vec<u32> = init
        .instrs
        .iter()
        .filter_map(|i| match i {
            Instr::InitSlot { field_index, .. } => Some(*field_index),
            _ => None,
        })
        .collect();
    assert_eq!(init_slots, vec![1], "only `b` (field 1) has an initializer");
    assert_eq!(init.statement_pc.len(), 1);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test compiler`
Expected: FAIL — `compiler::compile_module`/`compile_initializer` not found.

- [x] **Step 3: Write the implementation**

`crates/ns-runtime/src/compiler/mod.rs`:

```rust
//! Bytecode compiler entry points: `compile_method`, `compile_initializer`,
//! `compile_module`. See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`.
//! Compiling an already-resolved module is a total, deterministic lowering
//! that can never raise a `NewspeakError`; a failure here is an `ns-runtime`
//! bug and panics.

pub(crate) mod lower;

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use crate::bytecode::{class_key, method_key, CodeKind, CompiledCode, CompiledModule};
use crate::types::{Binding, ClassRef, ObjectData};
use crate::Symbol;

use lower::Lowerer;

fn pattern_param_count(pattern: &ast::MessagePattern) -> u16 {
    match pattern {
        ast::MessagePattern::Unary { .. } => 0,
        ast::MessagePattern::Binary { .. } => 1,
        ast::MessagePattern::Keyword { parts } => parts.len() as u16,
    }
}

pub fn compile_method(method: &ast::MethodDecl, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledCode {
    let num_args = pattern_param_count(&method.pattern);
    let num_named_temps = method.body.temps.len() as u16;
    let mut lowerer = Lowerer::new(CodeKind::Method, num_args, num_named_temps, bindings, vec![]);
    for stmt in &method.body.stmts {
        lowerer.lower_statement(stmt);
    }
    lowerer.finish_body(&method.body.stmts);
    lowerer.finish(method.span)
}

/// Compiles a loaded class's own instance-initializer body: each slot with
/// an initializer (declaration order) as `<expr>; InitSlot`, then each
/// `initExpr` (a top-level `^` is evaluated and discarded), then
/// `LoadSelf; ImplicitReturn` -- the spec's "Class instantiation" section.
/// `num_args` is the class's own factory parameter count: the resolver
/// scoped this code with exactly those parameters as `Binding::Local`.
pub fn compile_initializer(class: &ClassRef, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledCode {
    let (slots, slot_index, init_exprs, num_args) = {
        let borrowed = class.borrow();
        let ObjectData::Class(data) = &borrowed.data else {
            panic!("internal: compile_initializer called on a non-class object");
        };
        (
            data.instance_slots.clone(),
            data.slot_index.clone(),
            data.init_exprs.clone(),
            data.factory.as_deref().map(pattern_param_count).unwrap_or(0),
        )
    };
    let mut lowerer = Lowerer::new(CodeKind::Initializer, num_args, 0, bindings, vec![]);
    let mut source: Option<Span> = None;
    for slot in &slots {
        if let Some(init) = &slot.init {
            let field_index = *slot_index
                .get(&Symbol::intern(&slot.name.name))
                .expect("internal: load_class assigns every own slot an absolute index");
            lowerer.lower_slot_init(init, field_index as u32);
            source = Some(merge_span(source, slot.span));
        }
    }
    for stmt in &init_exprs {
        lowerer.lower_statement(stmt);
        source = Some(merge_span(source, stmt_span(stmt)));
    }
    lowerer.finish_body(&init_exprs);
    lowerer.finish(source.unwrap_or(Span::new(0, 0)))
}

pub fn compile_module(root_class: &ClassRef, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledModule {
    let methods: Vec<Rc<ast::MethodDecl>> = {
        let borrowed = root_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => data.methods.values().cloned().collect(),
            _ => Vec::new(),
        }
    };
    let mut module = CompiledModule {
        methods: HashMap::new(),
        initializers: HashMap::new(),
        bindings: bindings.clone(),
    };
    for method in methods {
        let compiled = compile_method(&method, bindings);
        module.methods.insert(method_key(&method), Rc::new(compiled));
    }
    module
        .initializers
        .insert(class_key(root_class), Rc::new(compile_initializer(root_class, bindings)));
    module
}

fn merge_span(acc: Option<Span>, next: Span) -> Span {
    match acc {
        Some(s) => s.merge(next),
        None => next,
    }
}

fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Expr(e) | Stmt::Return(e) => expr_span(e),
    }
}

fn expr_span(expr: &Expr) -> Span {
    match expr {
        Expr::Ident(id) => id.span,
        Expr::Literal(lit) => literal_span(lit),
        Expr::Send(send) => send.span,
        Expr::Cascade(cascade) => cascade.span,
        Expr::Assign { span, .. } => *span,
        Expr::Block(block) => block.span,
        Expr::DynamicArray { span, .. } => *span,
    }
}

fn literal_span(lit: &ast::Literal) -> Span {
    match lit {
        ast::Literal::Int { span, .. }
        | ast::Literal::Float { span, .. }
        | ast::Literal::String { span, .. }
        | ast::Literal::Symbol { span, .. }
        | ast::Literal::Char { span, .. }
        | ast::Literal::Bool { span, .. }
        | ast::Literal::Nil { span }
        | ast::Literal::Array { span, .. }
        | ast::Literal::Pattern { span, .. } => *span,
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test compiler`
Expected: PASS (4 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): add compile_method, compile_initializer, and compile_module

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 6: `interpreter/` becomes the shared orchestration; `vm.rs` runs method and block bodies

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`ClosureBody`, `FrameKind::Initializer`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Engine`, `engine` field, `set_engine`/`compiled_module`, hook 1, `raise_return` arm)
- Modify: `crates/ns-runtime/src/interpreter/eval.rs` (visibility, `home_of`, `ClosureBody::Ast`, hook 2)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`FrameKind::Initializer` frame)
- Create: `crates/ns-runtime/src/vm.rs`
- Modify: `crates/ns-runtime/src/lib.rs`
- Create: `crates/ns-runtime/tests/vm.rs`

**Interfaces:**
- Consumes: `bytecode::{CompiledCode, CompiledModule, Instr, Literal, method_key}` (Task 1), `compiler::compile_module` (Task 5), and the M1 orchestration verbatim: `Interpreter::send`, `does_not_understand`, `dispatch_native_or_dnu`/`dispatch_native_inheriting_object`, `eval::{dispatch_lexical_send, dispatch_super_send}`, `raise_return_pub`, `builtins::call_block`.
- Produces:
  - `pub enum Engine { TreeWalk, Bytecode(CompiledModule) }` (re-exported as `ns_runtime::Engine`)
  - `Interpreter::set_engine(&mut self, engine: Engine)`, `Interpreter::compiled_module(&self) -> Option<&CompiledModule>`
  - `pub enum ClosureBody { Ast(Rc<ast::CodeBody>), Compiled(Rc<CompiledCode>) }`; `ClosureData.body: ClosureBody`
  - `FrameKind::Initializer(ClassRef)`
  - `pub(crate) fn eval::home_of(frame: &FrameRef) -> FrameRef`; `pub(crate)` on `eval::{read_local, write_local, frame_at_depth, dispatch_lexical_send, dispatch_super_send}`
  - `pub fn vm::run_code(interp: &mut Interpreter, frame: &FrameRef, code: &Rc<CompiledCode>, bindings: &Rc<HashMap<Span, Binding>>) -> EvalResult`

**Design notes (read in full — this task is the spec's "`vm.rs` and the engine hook" section):**
- **What changes in `interpreter/` and what does not.** Every function that decides *what* runs is untouched in behavior: `send` (with `try_factory_send`), `does_not_understand`, both native-dispatch helpers (the `Object`-inheritance step included — `Lookup::Native` is handled by exactly the code M1 already has, verified against `interpreter/mod.rs` lines 106-181 and `builtins/mod.rs`), `dispatch_lexical_send`, `dispatch_super_send`, `raise_return`, and both frame constructors. Exactly two places in this task decide *how* a body runs — `invoke_method` (hook 1) and `invoke_closure` (hook 2) — and each consults the new `engine` field / the closure's `body` variant. Hook 3 (`instantiate`) is Task 7.
- **`vm::run_code` has no dispatch logic and no error text.** Its `Send*` arms call `interp.send`/`dispatch_lexical_send`/`dispatch_super_send`, which re-enter `run_code` through hook 1/2 for a user-defined target. The only `NewspeakError` it ever constructs is the `MalformedInt` case, and that one is *M1's own* (`eval_literal`'s selector `parse`, receiver `nil`, and the message `parse_int_literal` returns) — identical text by construction.
- **Register addressing is the shared `read_local`/`write_local`** (now `pub(crate)`) — `read_local`'s `i < args.len()` split is the definition of the register file. Do not write a second copy. `frame_at_depth` likewise.
- **`Signal::Return` catching stays exactly where M1 put it.** `invoke_method` catches a `Signal::Return` targeting its own frame after the body ran, whichever engine ran it, and marks the frame `uncontinuable` on every exit path. A `NonLocalReturn` executed in a compiled block calls `raise_return_pub` on the *block's* frame, which walks to `home` — the same signal M1's `invoke_closure` raises for `Stmt::Return`. So `coll do: [:e | ... ifTrue: [^'found']]` (a compiled block invoked by the native `do:` via `builtins::call_block` → `invoke_closure` → hook 2) unwinds through the native and lands in the compiled method's `invoke_method` catch, as in M1.
- **`home_of`** is `eval_block_literal`'s `home` computation factored into a function so `MakeClosure` and the tree-walker share it; it groups `Method | TopLevel | Initializer` (the frame itself) versus `Closure` (its `home`). `raise_return`'s target match gains the same `Initializer` arm.
- **No arity assertion in `run_code`.** M1's `invoke_method` performs no arity check and `ns-cli` always sends `[platform, nil]` whatever the entry selector's arity (spec: carried forward, verified); the shared `read_local` addressing makes both engines agree on that case. A `debug_assert_eq!(args.len(), num_args)` would fail M1's own `run_supports_a_custom_entry_selector_via_send` test under the bytecode engine.
- **`instantiate` builds `FrameKind::Initializer(class)` frames from this task on**, under both engines (the frame is shared code). `FrameKind::TopLevel` stays in the enum (spec).
- Tests drive the bytecode engine **through `Interpreter::send`** with `Engine::Bytecode` installed — there is no separate `Vm` object to call. Every test file in this milestone that unwraps an `EvalResult` uses the `expect_ok` helper shown below (`Signal` has no `Debug`).

- [x] **Step 1: Write the failing tests**

`crates/ns-runtime/tests/vm.rs`:

```rust
//! Bytecode execution through the shared orchestration: `Engine::Bytecode`
//! installed on an `Interpreter`, every send driven via `Interpreter::send`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    compiler, Binding, ClassRef, ClosureBody, Engine, Interpreter, NewspeakError, ObjectData,
    Signal, SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

type Bindings = Rc<HashMap<Span, Binding>>;

fn load(src: &str) -> (Interpreter, ClassRef, Bindings) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved =
        ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns")))
            .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, bindings)
}

/// Loads `src` and switches the interpreter to the bytecode engine.
fn setup(src: &str) -> (Interpreter, ClassRef, Bindings) {
    let (mut interp, class, bindings) = load(src);
    let compiled = compiler::compile_module(&class, &bindings);
    interp.set_engine(Engine::Bytecode(compiled));
    (interp, class, bindings)
}

/// `Signal` carries no `Debug` impl (M1 decision), so `Result::expect` is
/// unusable on an `EvalResult` -- unwrap by hand.
fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn expect_error(result: Result<Value, Signal>, what: &str) -> NewspeakError {
    match result {
        Ok(_) => panic!("{what}: expected a NewspeakError, got a value"),
        Err(Signal::Return(..)) => panic!("{what}: expected a NewspeakError, got Signal::Return"),
        Err(Signal::Error(e)) => e,
    }
}

fn new_instance(interp: &mut Interpreter, class: &ClassRef, bindings: &Bindings) -> Value {
    expect_ok(interp.instantiate(class.clone(), vec![], bindings), "instantiate")
}

fn send(
    interp: &mut Interpreter,
    receiver: Value,
    selector: &str,
    args: Vec<Value>,
    bindings: &Bindings,
) -> Result<Value, Signal> {
    interp.send(receiver, Symbol::intern(selector), args, bindings, None)
}

fn string_of(v: &Value) -> String {
    let Value::Obj(obj) = v else { panic!("expected a string object") };
    match &obj.borrow().data {
        ObjectData::Str(s) => s.clone(),
        _ => panic!("expected ObjectData::Str"),
    }
}

fn int_of(v: &Value) -> i64 {
    match v {
        Value::SmallInt(n) => *n,
        _ => panic!("expected a SmallInt"),
    }
}

#[test]
fn a_user_method_runs_as_bytecode_and_returns_its_value() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public answer = ( ^42 )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "answer", vec![], &bindings), "answer");
    assert_eq!(int_of(&result), 42);
}

#[test]
fn a_block_created_by_bytecode_carries_compiled_code_and_is_invocable() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public makeBlock = ( ^[ :x | x + 1 ] )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let block = expect_ok(send(&mut interp, instance, "makeBlock", vec![], &bindings), "makeBlock");
    {
        let Value::Obj(obj) = &block else { panic!("expected a closure object") };
        let ObjectData::Closure(closure) = &obj.borrow().data else { panic!("expected a closure") };
        assert!(
            matches!(closure.body, ClosureBody::Compiled(_)),
            "a closure created by the bytecode engine must carry compiled code"
        );
    }
    let result = expect_ok(
        send(&mut interp, block, "value:", vec![Value::SmallInt(9)], &bindings),
        "value:",
    );
    assert_eq!(int_of(&result), 10);
}

#[test]
fn temps_implicit_self_sends_and_arithmetic_work() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  helper = ( ^40 )\n  public foo = ( | t | t := helper. ^t + 2 )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![], &bindings), "foo");
    assert_eq!(int_of(&result), 42);
}

#[test]
fn slots_read_and_write_through_lexical_sends() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n  | n ::= 1. |\n) (\n  public bump = ( n := n + 1. ^n )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    expect_ok(send(&mut interp, instance.clone(), "bump", vec![], &bindings), "bump");
    let result = expect_ok(send(&mut interp, instance, "bump", vec![], &bindings), "bump");
    assert_eq!(int_of(&result), 3);
}

#[test]
fn an_unknown_selector_produces_the_shared_does_not_understand_error() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";
    let (mut interp, _class, bindings) = setup(src);
    let err = expect_error(
        send(&mut interp, Value::SmallInt(3), "nonexistentSelector", vec![], &bindings),
        "dnu",
    );
    assert!(err.message.contains("does not understand #nonexistentSelector"), "{}", err.message);
}

#[test]
fn a_super_send_falls_through_to_objects_native_print_string() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( ^super printString )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![], &bindings), "foo");
    assert_eq!(string_of(&result), "an Object");
}

#[test]
fn cascades_and_builtin_instantiation_work() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( | c | c := OrderedCollection new. c add: 1; add: 2. ^c size )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "foo", vec![], &bindings), "foo");
    assert_eq!(int_of(&result), 2);
}

#[test]
fn a_non_local_return_from_a_natively_invoked_block_unwinds_to_the_bytecode_frame() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  find: target in: coll = ( coll do: [:each | each = target ifTrue: [^'found']]. ^'not found' )\n  public main = ( | c | c := OrderedCollection new. c add: 1; add: 2; add: 3. ^(self find: 2 in: c), (self find: 9 in: c) )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "main", vec![], &bindings), "main");
    assert_eq!(string_of(&result), "foundnot found");
}

#[test]
fn a_block_writes_an_enclosing_temp_through_store_outer_local() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public sum = ( | n | n := 0. #(1 2 3) do: [:e | n := n + e]. ^n )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let result = expect_ok(send(&mut interp, instance, "sum", vec![], &bindings), "sum");
    assert_eq!(int_of(&result), 6);
}

#[test]
fn empty_and_multi_statement_blocks_return_nil_and_the_last_value() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public empty = ( ^[ ] value )\n  public last = ( ^[ 1. 2 ] value )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let empty = expect_ok(send(&mut interp, instance.clone(), "empty", vec![], &bindings), "empty");
    assert!(matches!(empty, Value::Nil));
    let last = expect_ok(send(&mut interp, instance, "last", vec![], &bindings), "last");
    assert_eq!(int_of(&last), 2);
}

#[test]
fn if_true_if_false_dispatches_through_the_native_boolean_with_compiled_blocks() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo: b = ( ^b ifTrue: ['yes'] ifFalse: ['no'] )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let yes = expect_ok(
        send(&mut interp, instance.clone(), "foo:", vec![Value::Bool(true)], &bindings),
        "foo:",
    );
    assert_eq!(string_of(&yes), "yes");
    let no = expect_ok(send(&mut interp, instance, "foo:", vec![Value::Bool(false)], &bindings), "foo:");
    assert_eq!(string_of(&no), "no");
}

#[test]
fn a_block_invoked_after_its_home_returned_gets_cannot_return() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  mk = ( ^[ ^1 ] )\n  public run = ( ^mk value )\n) : ()";
    let (mut interp, class, bindings) = setup(src);
    let instance = new_instance(&mut interp, &class, &bindings);
    let err = expect_error(send(&mut interp, instance, "run", vec![], &bindings), "run");
    assert!(err.message.contains("cannotReturn"), "{}", err.message);
}

#[test]
fn a_malformed_integer_literal_raises_the_identical_error_under_both_engines() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( ^99999999999999999999 )\n) : ()";
    let (mut bc, class, bindings) = setup(src);
    let instance = new_instance(&mut bc, &class, &bindings);
    let bc_err = expect_error(send(&mut bc, instance, "foo", vec![], &bindings), "bytecode");
    let (mut tw, class, bindings) = load(src);
    let instance = new_instance(&mut tw, &class, &bindings);
    let tw_err = expect_error(send(&mut tw, instance, "foo", vec![], &bindings), "tree-walk");
    assert_eq!(bc_err.message, tw_err.message);
    assert_eq!(bc_err.selector, tw_err.selector);
    assert!(bc_err.message.contains("out of i64 range"), "{}", bc_err.message);
}

#[test]
fn the_tree_walk_engine_is_still_the_default() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public makeBlock = ( ^[ 1 ] )\n) : ()";
    let (mut interp, class, bindings) = load(src);
    assert!(interp.compiled_module().is_none());
    let instance = new_instance(&mut interp, &class, &bindings);
    let block = expect_ok(send(&mut interp, instance, "makeBlock", vec![], &bindings), "makeBlock");
    let Value::Obj(obj) = &block else { panic!() };
    let ObjectData::Closure(closure) = &obj.borrow().data else { panic!() };
    assert!(matches!(closure.body, ClosureBody::Ast(_)));
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test vm`
Expected: FAIL to compile — `Engine`, `ClosureBody`, `set_engine`, `compiled_module` not found.

- [x] **Step 3: `types.rs` — `ClosureBody` and `FrameKind::Initializer`**

In `crates/ns-runtime/src/types.rs`, replace `ClosureData`'s `body` field and add the enum directly above the struct:

```rust
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
    /// (doc comment unchanged)
    pub bindings: Rc<HashMap<Span, Binding>>,
}
```

(Keep the existing `bindings` doc comment verbatim.) Add the `Initializer` variant to `FrameKind`:

```rust
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
```

Update the `Frame.home` doc comment to "Set only for `FrameKind::Closure` -- the enclosing method or initializer activation." and `Frame.current_class`'s to "`None` only for `FrameKind::TopLevel`".

- [x] **Step 4: `interpreter/eval.rs` — visibility, `home_of`, `ClosureBody::Ast`, hook 2**

Change these signatures (bodies unchanged): `pub(crate) fn read_local`, `pub(crate) fn write_local`, `pub(crate) fn frame_at_depth`, `pub(crate) fn dispatch_lexical_send`, `pub(crate) fn dispatch_super_send`.

Add, next to `frame_at_depth`:

```rust
/// The `home` a closure created in `frame` gets: a method, top-level, or
/// initializer activation is its own home; a closure activation's home is
/// its own `home`. Shared by the tree-walker's `eval_block_literal` and
/// the VM's `MakeClosure`.
pub(crate) fn home_of(frame: &FrameRef) -> FrameRef {
    match &frame.borrow().kind {
        FrameKind::Method(_) | FrameKind::TopLevel | FrameKind::Initializer(_) => frame.clone(),
        FrameKind::Closure(closure) => closure.home.clone(),
    }
}
```

In `eval_block_literal`, replace the `let home = match ... ;` block with `let home = home_of(frame);` and the `body:` field with `body: ClosureBody::Ast(Rc::new(block.body.clone())),`.

Replace `invoke_closure`'s body after the arity guard (from `let receiver = ...` to the end) with:

```rust
    let receiver = closure.captured_frame.borrow().receiver.clone();
    let current_class = closure.captured_frame.borrow().current_class.clone();
    // Hook 2 (spec: "`vm.rs` and the engine hook"): the frame is built the
    // same way for both engines; only the body's evaluation differs, and
    // which engine that is was decided when the closure was created.
    let temp_count = match &closure.body {
        ClosureBody::Ast(body) => body.temps.len(),
        ClosureBody::Compiled(code) => (code.num_registers - code.num_args) as usize,
    };
    let frame = Rc::new(RefCell::new(Frame {
        receiver,
        args,
        temps: vec![Value::Nil; temp_count],
        pc: 0,
        sender,
        home: Some(closure.home.clone()),
        current_class,
        kind: FrameKind::Closure(closure.clone()),
        uncontinuable: false,
    }));
    match &closure.body {
        ClosureBody::Compiled(code) => crate::vm::run_code(interp, &frame, code, bindings),
        ClosureBody::Ast(body) => {
            let mut last = Value::Nil;
            for (i, stmt) in body.stmts.iter().enumerate() {
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
    }
```

- [x] **Step 5: `interpreter/mod.rs` — `Engine`, the field, hook 1, `raise_return`**

Add `use crate::bytecode::{method_key, CompiledModule};` to the imports. Add, above `pub struct Interpreter`:

```rust
/// Which engine evaluates method bodies, block bodies, and instance
/// initializers. Everything else -- dispatch, `doesNotUnderstand:`, native
/// dispatch, instantiation, frame construction -- is shared. See the M2
/// spec's "`vm.rs` and the engine hook".
pub enum Engine {
    TreeWalk,
    Bytecode(CompiledModule),
}
```

Add the field `engine: Engine,` to `Interpreter` and initialize it with `engine: Engine::TreeWalk,` in both `new()` and `bootstrap()`. Add to `impl Interpreter`:

```rust
/// Switches engines. Call after `load_module` and `compile_module`; the
/// default is `TreeWalk`, so every Milestone 1 caller keeps its behavior.
pub fn set_engine(&mut self, engine: Engine) {
    self.engine = engine;
}

pub fn compiled_module(&self) -> Option<&CompiledModule> {
    match &self.engine {
        Engine::Bytecode(module) => Some(module),
        Engine::TreeWalk => None,
    }
}
```

Replace `invoke_method`'s body with:

```rust
    // Hook 1 (spec: "`vm.rs` and the engine hook"): the frame, the
    // `uncontinuable` marking, and the `Signal::Return` catch are shared;
    // only the body's evaluation differs.
    let compiled = match &self.engine {
        Engine::TreeWalk => None,
        Engine::Bytecode(module) => Some(
            module.methods.get(&method_key(&method)).cloned().unwrap_or_else(|| {
                panic!("internal: dispatched method has no compiled form (compile_module coverage gap)")
            }),
        ),
    };
    let temp_count = match &compiled {
        Some(code) => (code.num_registers - code.num_args) as usize,
        None => method.body.temps.len(),
    };
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
    let result = match &compiled {
        Some(code) => crate::vm::run_code(self, &frame, code, &bindings),
        None => self.run_statements(&method.body.stmts, &frame, &bindings),
    };
    frame.borrow_mut().uncontinuable = true;
    match result {
        Ok(value) => Ok(value),
        Err(Signal::Return(target, value)) if Rc::ptr_eq(&target, &frame) => Ok(value),
        Err(other) => Err(other),
    }
```

In `raise_return`, change the target match's first arm to `FrameKind::Method(_) | FrameKind::TopLevel | FrameKind::Initializer(_) => frame.clone(),`.

- [x] **Step 6: `interpreter/instantiate.rs` — `Initializer` frames**

In `instantiate`, replace `kind: FrameKind::TopLevel, // an initializer is its own kind of top-of-chain activation for M1` with `kind: FrameKind::Initializer(class.clone()),`. Nothing else changes in this file yet (hook 3 is Task 7).

- [x] **Step 7: `vm.rs`**

`crates/ns-runtime/src/vm.rs`:

```rust
//! The register VM's run loop -- and nothing else. `run_code` is a `match`
//! over `Instr`; every `Send*` calls straight back into `interpreter/`'s
//! shared orchestration (`Interpreter::send`, `dispatch_lexical_send`,
//! `dispatch_super_send`), which re-enters `run_code` through the engine
//! hooks for any user-defined target. No dispatch logic, no
//! `doesNotUnderstand:`, no native dispatch, and no error text of its own
//! live here -- that is what makes the tree-walker a byte-for-byte oracle.
//! See `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`,
//! "`vm.rs` and the engine hook".

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{CompiledCode, Instr, Literal};
use crate::interpreter::eval::{
    dispatch_lexical_send, dispatch_super_send, frame_at_depth, home_of, parse_int_literal,
    read_local, write_local,
};
use crate::interpreter::{EvalResult, Interpreter, Signal};
use crate::types::*;
use crate::Symbol;

/// Runs `code` on `frame` until a terminating instruction. `frame` was built
/// by the shared orchestration (`invoke_method`, `invoke_closure`, or
/// `instantiate`) with `temps` sized to `code.num_registers - code.num_args`;
/// `frame.pc` is kept at the executing instruction's index (the safepoint
/// view -- `statement_pc` recovers the statement on demand).
pub fn run_code(
    interp: &mut Interpreter,
    frame: &FrameRef,
    code: &Rc<CompiledCode>,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let mut pc = 0usize;
    loop {
        frame.borrow_mut().pc = pc;
        match &code.instrs[pc] {
            Instr::LoadLiteral { dst, literal } => {
                let v = make_value(interp, literal)?;
                write_local(frame, *dst, v);
            }
            Instr::LoadSelf { dst } => {
                let v = frame.borrow().receiver.clone();
                write_local(frame, *dst, v);
            }
            Instr::LoadBuiltinClass { dst, kind } => {
                let v = Value::Obj(interp.builtin_class(*kind));
                write_local(frame, *dst, v);
            }
            Instr::Move { dst, src } => {
                let v = read_local(frame, *src);
                write_local(frame, *dst, v);
            }
            Instr::StoreLocal { src, slot } => {
                let v = read_local(frame, *src);
                write_local(frame, *slot, v);
            }
            Instr::LoadOuterLocal { dst, depth, index } => {
                let v = read_local(&frame_at_depth(frame, *depth), *index);
                write_local(frame, *dst, v);
            }
            Instr::StoreOuterLocal { src, depth, index } => {
                let v = read_local(frame, *src);
                write_local(&frame_at_depth(frame, *depth), *index, v);
            }
            Instr::SendPublic { selector, receiver_reg, arg_regs, dst, .. } => {
                let receiver = read_local(frame, *receiver_reg);
                let args = collect(frame, arg_regs);
                let result = interp.send(receiver, *selector, args, bindings, Some(frame.clone()))?;
                write_local(frame, *dst, result);
            }
            Instr::SendLexical { depth, selector, arg_regs, dst, .. } => {
                let args = collect(frame, arg_regs);
                let result = dispatch_lexical_send(interp, frame, *depth, *selector, args, bindings)?;
                write_local(frame, *dst, result);
            }
            Instr::SendSuper { selector, arg_regs, dst, .. } => {
                let args = collect(frame, arg_regs);
                let result = dispatch_super_send(interp, frame, *selector, args, bindings)?;
                write_local(frame, *dst, result);
            }
            Instr::MakeClosure { dst, params, code: block } => {
                let closure = Rc::new(ClosureData {
                    params: (**params).clone(),
                    body: ClosureBody::Compiled(block.clone()),
                    captured_frame: frame.clone(),
                    home: home_of(frame),
                    bindings: bindings.clone(),
                });
                let v = Value::Obj(interp.new_closure(closure));
                write_local(frame, *dst, v);
            }
            Instr::MakeArray { dst, elem_regs } => {
                let values = collect(frame, elem_regs);
                let v = Value::Obj(interp.new_array(values));
                write_local(frame, *dst, v);
            }
            Instr::InitSlot { src, field_index } => {
                // Mirrors `instantiate::run_own_slots`: a direct field write
                // on `self`, silently skipped for a non-instance receiver.
                let v = read_local(frame, *src);
                let receiver = frame.borrow().receiver.clone();
                if let Value::Obj(obj) = receiver {
                    if let ObjectData::Instance(fields) = &mut obj.borrow_mut().data {
                        let i = *field_index as usize;
                        if i < fields.len() {
                            fields[i] = v;
                        }
                    }
                }
            }
            Instr::Return { reg } | Instr::ImplicitReturn { reg } => {
                return Ok(read_local(frame, *reg));
            }
            Instr::NonLocalReturn { reg } => {
                let v = read_local(frame, *reg);
                return Err(interp.raise_return_pub(frame, v));
            }
            Instr::Jump { target } => {
                pc = *target as usize;
                continue;
            }
            Instr::JumpIfNotBoolean { reg, target } => {
                if !matches!(read_local(frame, *reg), Value::Bool(_)) {
                    pc = *target as usize;
                    continue;
                }
            }
            Instr::BranchIfFalse { reg, target } => {
                if matches!(read_local(frame, *reg), Value::Bool(false)) {
                    pc = *target as usize;
                    continue;
                }
            }
            Instr::BranchIfTrue { reg, target } => {
                if matches!(read_local(frame, *reg), Value::Bool(true)) {
                    pc = *target as usize;
                    continue;
                }
            }
        }
        pc += 1;
        debug_assert!(
            pc < code.instrs.len(),
            "internal: pc ran past the end of a CompiledCode with no terminating return"
        );
    }
}

fn collect(frame: &FrameRef, regs: &[u16]) -> Vec<Value> {
    regs.iter().map(|r| read_local(frame, *r)).collect()
}

/// Materializes a literal exactly as `eval::eval_literal` does -- including
/// the deferred `MalformedInt` case, which re-runs the shared
/// `parse_int_literal` and raises M1's own error (selector `parse`,
/// receiver `nil`, the parser's message).
fn make_value(interp: &mut Interpreter, literal: &Literal) -> EvalResult {
    Ok(match literal {
        Literal::Nil => Value::Nil,
        Literal::Bool(b) => Value::Bool(*b),
        Literal::SmallInt(n) => Value::SmallInt(*n),
        Literal::Float(f) => Value::Float(*f),
        Literal::Str(s) => Value::Obj(interp.new_string(s.clone())),
        Literal::Sym(s) => interp.new_symbol(*s),
        Literal::Char(c) => Value::Obj(interp.new_character(*c)),
        Literal::Array(elements) => {
            let mut values = Vec::with_capacity(elements.len());
            for e in elements {
                values.push(make_value(interp, e)?);
            }
            Value::Obj(interp.new_array(values))
        }
        Literal::MalformedInt(text) => match parse_int_literal(text) {
            Ok(n) => Value::SmallInt(n),
            Err(message) => {
                return Err(Signal::Error(NewspeakError {
                    selector: Symbol::intern("parse"),
                    receiver: Value::Nil,
                    message,
                }))
            }
        },
    })
}
```

- [x] **Step 8: Wire into `lib.rs`**

Add `pub mod vm;` and change the re-export line to `pub use interpreter::{Engine, Interpreter, Signal};`.

- [x] **Step 9: Run tests to verify they pass**

Run: `cargo test -p ns-runtime`
Expected: PASS — the 14 new `vm` tests and every existing M1 test (tree-walk is still the default, `types.rs`'s own `frame_kind_top_level_has_no_current_class` still compiles). Two M1 tests construct a `ClosureData` by hand — `crates/ns-runtime/tests/interpreter.rs` lines 280-282 and 324-326 (`body: Rc::new(block_body)`) — update both to `body: ns_runtime::ClosureBody::Ast(Rc::new(block_body))`; a mechanical change, no behavior change.

- [x] **Step 10: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): add the register VM and route method and block bodies through the engine hooks

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 7: Hook 3 — instance initializers run as bytecode

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`
- Create: `crates/ns-runtime/tests/vm_initializer.rs`

**Interfaces:**
- Consumes: `Engine`, `vm::run_code` (Task 6), `bytecode::class_key`, `CompiledModule.initializers` (Task 5).
- Produces: `fn run_own_initializer(&mut self, class: &ClassRef, instance: &Value, frame: &FrameRef, bindings: &Rc<HashMap<Span, Binding>>) -> Result<(), Signal>` — the one per-level body-execution point of `instantiate`, replacing the paired `run_own_slots` + `run_own_init_exprs` calls at both call sites (`instantiate` step 4-5 and `run_superclass_initializer`).

**Design notes:**
- This is the spec's "Class instantiation" section. The six-step algorithm stays where it is; `run_own_slots`/`run_own_init_exprs` stay as the tree-walker's implementation of steps 4-5; the bytecode engine runs the level's compiled initializer on the *same shared frame* after growing `temps` to `num_registers - num_args` (the frame was built with `temps: vec![]` and `args` = the factory arguments — for the root class `instantiate`'s arity check makes `args.len() == num_args`).
- A class with no slots and no `initExprs` (every `BuiltinClass`, e.g. `Object` at the top of every chain) is a no-op under both engines and is skipped *before* the cache lookup; a user class missing from `CompiledModule.initializers` is a `compile_module` coverage bug (`panic!`).
- Every M1 instantiation gap is carried forward in both engines (spec): the superclass clause's deferred factory message is never evaluated; every level runs in the subclass's frame; a `^` inside a block in initializer code escapes `instantiate` as `Signal::Return` and the frame is never marked `uncontinuable`. Do not "fix" any of these here — they are shared-code changes for a later milestone.

- [x] **Step 1: Write the failing tests**

`crates/ns-runtime/tests/vm_initializer.rs`:

```rust
//! Hook 3: instance initializers (slot initializers + initExprs) under the
//! bytecode engine, compared against the tree-walker.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{
    compiler, Binding, ClassRef, ClosureBody, Engine, FrameKind, Interpreter, ObjectData, Signal,
    SourceOrigin, Symbol, Value,
};
use ns_syntax::Span;

type Bindings = Rc<HashMap<Span, Binding>>;

fn load(src: &str, bytecode: bool) -> (Interpreter, ClassRef, Bindings) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved =
        ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns")))
            .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    if bytecode {
        let compiled = compiler::compile_module(&class, &bindings);
        interp.set_engine(Engine::Bytecode(compiled));
    }
    (interp, class, bindings)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn int_of(v: &Value) -> i64 {
    match v {
        Value::SmallInt(n) => *n,
        _ => panic!("expected a SmallInt"),
    }
}

/// Instantiates with `factory_args`, sends `getter`, returns the result --
/// under whichever engine `load` installed.
fn instantiate_and_get(src: &str, bytecode: bool, factory_args: Vec<Value>, getter: &str) -> Value {
    let (mut interp, class, bindings) = load(src, bytecode);
    let instance = expect_ok(interp.instantiate(class, factory_args, &bindings), "instantiate");
    expect_ok(
        interp.send(instance, Symbol::intern(getter), vec![], &bindings, None),
        getter,
    )
}

const FACTORY_PARAM: &str = "Newspeak3\n'T'\nclass Foo x: a = Object new (\n  | p = a. |\n) (\n  public p2 = ( ^p )\n) : ()";
const INIT_EXPRS: &str = "Newspeak3\n'T'\nclass Foo = Object new (\n  | q ::= 5. r ::= 0. |\n  r := q + 1.\n) (\n  public r2 = ( ^r )\n) : ()";

#[test]
fn a_slot_initializer_reads_a_factory_parameter_under_bytecode() {
    let v = instantiate_and_get(FACTORY_PARAM, true, vec![Value::SmallInt(7)], "p2");
    assert_eq!(int_of(&v), 7);
}

#[test]
fn init_exprs_run_after_slot_initializers_under_bytecode() {
    let v = instantiate_and_get(INIT_EXPRS, true, vec![], "r2");
    assert_eq!(int_of(&v), 6);
}

#[test]
fn both_engines_produce_identical_instance_state() {
    for (src, args, getter) in [
        (FACTORY_PARAM, vec![Value::SmallInt(7)], "p2"),
        (INIT_EXPRS, vec![], "r2"),
    ] {
        let tw = instantiate_and_get(src, false, args.clone(), getter);
        let bc = instantiate_and_get(src, true, args, getter);
        assert_eq!(int_of(&tw), int_of(&bc), "{getter}: engines disagree");
    }
}

#[test]
fn a_closure_created_in_a_slot_initializer_is_compiled_and_homed_at_the_initializer_frame() {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n  | blk = [ 3 ]. |\n) (\n  public blk2 = ( ^blk )\n) : ()";
    let v = instantiate_and_get(src, true, vec![], "blk2");
    let Value::Obj(obj) = &v else { panic!("expected a closure object") };
    let ObjectData::Closure(closure) = &obj.borrow().data else { panic!("expected a closure") };
    assert!(matches!(closure.body, ClosureBody::Compiled(_)));
    assert!(matches!(closure.home.borrow().kind, FrameKind::Initializer(_)));
}

#[test]
fn a_return_inside_a_block_in_init_exprs_escapes_instantiate_under_both_engines() {
    // M1 gap, deliberately carried forward in both engines (spec: "Class
    // instantiation"): the `Signal::Return` targets the initializer frame
    // and nothing catches it.
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n  | |\n  [ ^1 ] value.\n) (\n) : ()";
    for bytecode in [false, true] {
        let (mut interp, class, bindings) = load(src, bytecode);
        let result = interp.instantiate(class, vec![], &bindings);
        assert!(matches!(result, Err(Signal::Return(..))), "bytecode={bytecode}");
    }
}

#[test]
fn a_factory_arity_mismatch_is_still_the_shared_error() {
    let (mut interp, class, bindings) = load(FACTORY_PARAM, true);
    match interp.instantiate(class, vec![], &bindings) {
        Err(Signal::Error(e)) => assert!(e.message.contains("wrong number of factory arguments"), "{}", e.message),
        _ => panic!("expected the arity error"),
    }
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test vm_initializer`
Expected: `a_closure_created_in_a_slot_initializer_is_compiled...` FAILS (the closure is `ClosureBody::Ast` — the initializer is still tree-walked under `Engine::Bytecode`); the value-comparison tests pass already (the tree-walker computes the same values), which is exactly the point — hook 3 must change *how* without changing *what*.

- [x] **Step 3: Write the implementation**

In `crates/ns-runtime/src/interpreter/instantiate.rs`, change the imports line `use super::{EvalResult, Interpreter, Signal};` to `use super::{Engine, EvalResult, Interpreter, Signal};`. In `instantiate`, replace

```rust
        // Step 4: this class's own slot clauses, in declaration order.
        self.run_own_slots(&class, &receiver, &frame, bindings)?;

        // Step 5: this class's own initExprs.
        self.run_own_init_exprs(&class, &frame, bindings)?;
```

with

```rust
        // Steps 4-5: this class's own slot clauses (declaration order),
        // then its own initExprs -- engine hook 3.
        self.run_own_initializer(&class, &receiver, &frame, bindings)?;
```

In `run_superclass_initializer`, replace the two lines

```rust
        self.run_own_slots(&superclass, instance, subclass_frame, bindings)?;
        self.run_own_init_exprs(&superclass, subclass_frame, bindings)?;
```

with

```rust
        self.run_own_initializer(&superclass, instance, subclass_frame, bindings)?;
```

Add to `impl Interpreter` (above `run_own_slots`):

```rust
    /// Engine hook 3 (M2 spec, "`vm.rs` and the engine hook"): Class
    /// Instantiation steps 4-5 for one level of the chain -- this class's
    /// own slot initializers, then its own `initExprs`, on the shared
    /// initializer frame. The tree-walker runs `run_own_slots` +
    /// `run_own_init_exprs`; the bytecode engine runs the class's compiled
    /// initializer (`compiler::compile_initializer`) after growing the
    /// frame's register file to fit it. Every M1 instantiation gap is
    /// carried forward identically in both engines.
    fn run_own_initializer(
        &mut self,
        class: &ClassRef,
        instance: &Value,
        frame: &FrameRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> Result<(), Signal> {
        let compiled = match &self.engine {
            Engine::TreeWalk => None,
            Engine::Bytecode(module) => {
                if !class_has_initializer_code(class) {
                    // Every BuiltinClass, and any class with nothing to run:
                    // a no-op under both engines.
                    return Ok(());
                }
                Some(
                    module
                        .initializers
                        .get(&crate::bytecode::class_key(class))
                        .cloned()
                        .unwrap_or_else(|| {
                            panic!("internal: class has no compiled instance initializer (compile_module coverage gap)")
                        }),
                )
            }
        };
        match compiled {
            None => {
                self.run_own_slots(class, instance, frame, bindings)?;
                self.run_own_init_exprs(class, frame, bindings)
            }
            Some(code) => {
                {
                    let mut f = frame.borrow_mut();
                    let needed = (code.num_registers - code.num_args) as usize;
                    if f.temps.len() < needed {
                        f.temps.resize(needed, Value::Nil);
                    }
                }
                crate::vm::run_code(self, frame, &code, bindings).map(|_| ())
            }
        }
    }
```

Add at module scope (next to `total_slot_count`):

```rust
/// Whether `class` declares any slot or `initExpr` at all -- `false` for
/// every `BuiltinClass` and for a non-class object.
fn class_has_initializer_code(class: &ClassRef) -> bool {
    let borrowed = class.borrow();
    match &borrowed.data {
        ObjectData::Class(data) => !(data.instance_slots.is_empty() && data.init_exprs.is_empty()),
        _ => false,
    }
}
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime`
Expected: PASS (6 new tests; every existing `instantiate`/`factory_send` test unchanged under the default tree-walk engine).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): run instance initializers as bytecode through engine hook 3

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 8: `ns-cli run --engine`, the instance-initialization fixture, and the differential harness

**Files:**
- Modify: `crates/ns-cli/src/main.rs`
- Create: `tests-fixtures/instance_init.ns`, `tests-fixtures/instance_init.expected`
- Create: `crates/ns-cli/tests/differential.rs`

**Interfaces:**
- Consumes: `ns_runtime::{Engine, compiler::compile_module, ResolvedModule, SourceOrigin}` (Tasks 5-7).
- Produces: `ns-next run <file> [--send selector] [--engine bytecode|tree-walk]` (`--engine` defaults to `bytecode`); in `main.rs`, the helpers every later CLI task reuses: `struct LoadedSource { source: String, origin: SourceOrigin, resolved: ResolvedModule }`, `fn load_source(file: &Path) -> Result<LoadedSource, ExitCode>`, `fn bootstrap_with_engine(loaded: &LoadedSource, engine: EngineArg) -> (Interpreter, ClassRef)`, `fn report_entry_send(result: Result<Value, Signal>) -> ExitCode`; in `differential.rs`, `run_with_engine`, `assert_engines_agree`, `assert_engines_agree_on_source` (Task 10 adds tests to it).

**Design notes:**
- **The engine is installed before instantiation** (spec: "`ns-cli` additions") so the root class's instance initializer runs under it too. `run` is one shared code path with the engine as its only parameter: read → parse → resolve → bootstrap → load → install engine → instantiate with `platform` → send the entry selector with `[platform, nil]` → report. The stderr strings (`Uncaught Newspeak error: ...`, `internal: non-local return escaped ...`) are kept verbatim from M1 — the differential harness compares stderr byte for byte.
- **The differential harness is the spec's "Testing" section**: every `tests-fixtures/*.ns` under both engines, exit status + stdout + stderr identical, no exemptions. It is green as soon as this task lands (control-flow inlining, Task 10, is a pure optimization of behavior that already agrees). It also carries an inline-source helper for cases that must not be fixtures (M1's `run_fixtures.rs` asserts every fixture exits `0`): a `doesNotUnderstand:` comparison lands here now; Task 10 adds the control-flow error cases.
- **`instance_init.ns`** exercises the hook-3 path end to end: a slot initializer reading the factory parameter (`platform transcript`), a slot initializer with a literal, an `initExpr` that sends and assigns, and an `initExpr` with output — so the fixture proves both engines run the initializer, in order, once.

- [x] **Step 1: Add the fixture**

`tests-fixtures/instance_init.ns`:

```
Newspeak3
'Test'
class InstanceInit usingPlatform: platform = Object new (
	| out = platform transcript. greeting ::= 'hi'. count ::= 0. |
	count := greeting size.
	out show: 'init ran'; newLine.
) (
	public main: p args: a = (
		out show: greeting; show: ' '; show: count printString; newLine
	)
) : ()
```

`tests-fixtures/instance_init.expected`:

```
init ran
hi 2
```

Confirm it parses and runs under the tree-walker before relying on it: `cargo run -p ns-cli -- run tests-fixtures/instance_init.ns` must print exactly the two expected lines (this exercises only M1 code — fix the fixture, never the parser, on any mismatch).

- [x] **Step 2: Write the failing tests**

`crates/ns-cli/tests/differential.rs`:

```rust
//! Every fixture, plus inline sources that must not be fixtures (they exit
//! non-zero), run under both engines; exit status, stdout, AND stderr must
//! match byte for byte. The Milestone 1 tree-walker is the oracle (M2 spec,
//! "Testing").

use std::path::{Path, PathBuf};
use std::process::Command;

/// Same normalization as `run_fixtures.rs`: `.expected` files may be
/// checked out with `\r\n` on Windows.
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

pub struct Outcome {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_with_engine(path: &Path, engine: &str) -> Outcome {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(path)
        .arg("--engine")
        .arg(engine)
        .output()
        .expect("run ns-next");
    Outcome {
        ok: output.status.success(),
        stdout: normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
        stderr: normalize_newlines(&String::from_utf8_lossy(&output.stderr)),
    }
}

/// Runs `path` under both engines, asserts they agree on everything
/// observable, and returns the bytecode engine's outcome for further
/// assertions.
pub fn assert_engines_agree(path: &Path) -> Outcome {
    let bc = run_with_engine(path, "bytecode");
    let tw = run_with_engine(path, "tree-walk");
    assert_eq!(
        bc.ok, tw.ok,
        "{}: exit status differs (bytecode stderr: {}; tree-walk stderr: {})",
        path.display(), bc.stderr, tw.stderr
    );
    assert_eq!(bc.stdout, tw.stdout, "{}: stdout differs between engines", path.display());
    assert_eq!(bc.stderr, tw.stderr, "{}: stderr differs between engines", path.display());
    bc
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures")
}

/// Writes `src` to a per-test temp file (unique per test name and process,
/// so parallel tests never collide) and returns its path.
fn write_temp_source(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ns-next-differential-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(format!("{name}.ns"));
    std::fs::write(&path, src).expect("write temp source");
    path
}

pub fn assert_engines_agree_on_source(name: &str, src: &str) -> Outcome {
    let path = write_temp_source(name, src);
    let outcome = assert_engines_agree(&path);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    outcome
}

#[test]
fn every_tests_fixtures_ns_file_behaves_identically_under_both_engines() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures_dir()).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let outcome = assert_engines_agree(&path);
        assert!(outcome.ok, "{} failed under both engines: {}", path.display(), outcome.stderr);
        checked += 1;
    }
    assert!(checked >= 4, "expected the 3 Milestone 1 fixtures plus instance_init.ns");
}

#[test]
fn the_instance_init_fixture_runs_its_initializer_once_under_bytecode() {
    let outcome = run_with_engine(&fixtures_dir().join("instance_init.ns"), "bytecode");
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "init ran\nhi 2\n");
}

#[test]
fn a_does_not_understand_error_is_identical_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "dnu",
        "Newspeak3\n'T'\nclass Dnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 fooBar\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert!(outcome.stderr.contains("does not understand #fooBar"), "{}", outcome.stderr);
}
```

- [x] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ns-cli --test differential`
Expected: FAIL — `--engine` is not a recognized argument (every invocation exits non-zero with clap's usage error on stderr, so the first assertion that a fixture succeeded fails).

- [x] **Step 4: Write the implementation**

In `crates/ns-cli/src/main.rs`, add the engine argument type and thread it through `Cmd::Run`:

```rust
/// `--engine`: which evaluator runs Newspeak-level code. `bytecode` is the
/// default; `tree-walk` is the Milestone 1 interpreter, kept permanently as
/// the differential-testing oracle (M2 spec, "`ns-cli` additions").
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
enum EngineArg {
    Bytecode,
    TreeWalk,
}
```

```rust
    Run {
        file: PathBuf,
        /// (existing doc comment unchanged)
        #[arg(long, default_value = "main:args:")]
        send: String,
        #[arg(long, value_enum, default_value_t = EngineArg::Bytecode)]
        engine: EngineArg,
    },
```

```rust
        Cmd::Run { file, send, engine } => cmd_run(&file, &send, engine),
```

Replace `cmd_run` (and its doc comment) with the shared helpers plus the new command:

```rust
/// A module read, parsed, and resolved -- everything `run`, `disasm`, and
/// `snapshot` share before an interpreter exists.
struct LoadedSource {
    source: String,
    origin: ns_runtime::SourceOrigin,
    resolved: ns_runtime::ResolvedModule,
}

/// Reads, parses, and resolves `file`, reporting diagnostics exactly as
/// the Milestone 1 `run` did. `Err` carries the exit code to return.
fn load_source(file: &Path) -> Result<LoadedSource, ExitCode> {
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: reading {}: {e}", file.display());
            return Err(ExitCode::FAILURE);
        }
    };
    let module = match ns_syntax::parse(&source) {
        Ok(m) => m,
        Err(diags) => {
            print_diagnostics(file, &source, &diags);
            return Err(ExitCode::FAILURE);
        }
    };
    let origin = ns_runtime::SourceOrigin::File(file.to_path_buf());
    let resolved = match ns_runtime::resolve(module, source.as_str().into(), origin.clone()) {
        Ok(r) => r,
        Err(errors) => {
            for e in errors {
                eprintln!("resolve error: {}", e.message);
            }
            return Err(ExitCode::FAILURE);
        }
    };
    Ok(LoadedSource { source, origin, resolved })
}

/// Bootstraps an interpreter, loads the module's root class, and installs
/// the requested engine -- *before* anything is instantiated, so the root
/// class's instance initializer runs under it too (M2 spec, "`ns-cli`
/// additions").
fn bootstrap_with_engine(loaded: &LoadedSource, engine: EngineArg) -> (ns_runtime::Interpreter, ns_runtime::ClassRef) {
    let mut interp = ns_runtime::Interpreter::bootstrap();
    let class = interp.load_module(&loaded.resolved);
    if engine == EngineArg::Bytecode {
        let compiled = ns_runtime::compiler::compile_module(&class, &loaded.resolved.bindings);
        interp.set_engine(ns_runtime::Engine::Bytecode(compiled));
    }
    (interp, class)
}

/// Renders the entry send's outcome exactly as Milestone 1's `run` did --
/// these strings are compared byte for byte across engines by the
/// differential harness, so they are shared and never duplicated.
fn report_entry_send(result: Result<ns_runtime::Value, ns_runtime::Signal>) -> ExitCode {
    match result {
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

/// `ns-next run`: parse, resolve, bootstrap, load, install the engine,
/// instantiate the root class with `platform`, send the entry selector
/// with `[platform, nil]`, report.
fn cmd_run(file: &Path, send: &str, engine: EngineArg) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    let (mut interp, class) = bootstrap_with_engine(&loaded, engine);
    let platform = interp.platform();
    // `usingPlatform: platform`'s one factory parameter (`newspeak-spec.pdf`
    // §8.4's Core Library factory convention).
    let instance = match interp.instantiate(class, vec![platform.clone()], &bindings) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    // `main:args:`'s two parameters: `Nil` for the environment-args list --
    // no command-line-argument passthrough into Newspeak yet.
    let selector = ns_runtime::Symbol::intern(send);
    report_entry_send(interp.send(instance, selector, vec![platform, ns_runtime::Value::Nil], &bindings, None))
}
```

`report_signal` (instantiation failures) and everything else in the file stay as they are. `LoadedSource.source`/`origin` are unused until Task 11/15 — add `#[allow(dead_code)]` on those two fields for now and remove it in Task 15.

- [x] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-cli`
Expected: PASS — the new `differential` tests, and every existing `run_fixtures.rs`/`run.rs` test (they never pass `--engine`, so they now exercise the *default*, `bytecode`, end to end for the first time — including `run_supports_a_custom_entry_selector_via_send`, which sends two arguments to a one-parameter method under bytecode).

- [x] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(cli): add --engine to ns-next run (default bytecode) and the differential harness

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 9: Core library — native `whileTrue:`, `whileFalse:`, `ifFalse:ifTrue:`

**Files:**
- Modify: `crates/ns-runtime/src/builtins/block.rs`
- Modify: `crates/ns-runtime/src/builtins/boolean.rs`
- Create: `crates/ns-runtime/tests/builtins_control_flow.rs`

**Interfaces:**
- Consumes: `builtins::call_block`, `Interpreter::send` (unchanged).
- Produces: `Block>>whileTrue:`, `Block>>whileFalse:`, `Boolean>>ifFalse:ifTrue:` in the native tables; the `mustBeBoolean` convention (spec: "Control-flow inlining") that Task 10's inlined loops reproduce.

**Design notes:**
- Without these, the tree-walker could not be the oracle for loops (spec: "Open Questions Resolved During Planning"): M1's `Block` native table has `value`..`value:value:value:` and `numArgs` only, `boolean.rs` has `ifTrue:ifFalse:`/`ifTrue:`/`ifFalse:`/`and:`/`or:`/`&`/`|`/`not`/`printString`. Verified by `grep -rn "whileTrue\|ifFalse:ifTrue:" crates/ns-runtime/src` — no hits before this task.
- **A non-Boolean condition value sends `mustBeBoolean` to that value** (an ordinary `Interpreter::send`, so a user class may implement it) and the loop expression evaluates to that send's result. Nothing in the core library implements `mustBeBoolean`, so by default the shared `doesNotUnderstand:` path produces the error — identical text for the native and, in Task 10, the inlined loop. A loop that terminates normally evaluates to `nil`.
- The `bindings` an `Interpreter::send` needs come from the condition closure's own `ClosureData.bindings` (there is one module; any closure's map is the module's map).
- A `Signal::Return` escaping either block (`[...] whileTrue: [... ifTrue: [^x]]`) propagates through the loop via `?`, as `do:` already does.

- [x] **Step 1: Write the failing tests**

`crates/ns-runtime/tests/builtins_control_flow.rs`:

```rust
//! The three control-flow natives Milestone 2 adds (`Block>>whileTrue:`,
//! `Block>>whileFalse:`, `Boolean>>ifFalse:ifTrue:`), exercised under the
//! tree-walker -- the engine that has no inlining, so these really are the
//! natives running.

use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{Interpreter, ObjectData, Signal, SourceOrigin, Symbol, Value};

fn run_method(src: &str, selector: &str) -> Result<Value, Signal> {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved =
        ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns")))
            .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    let instance = match interp.instantiate(class, vec![], &bindings) {
        Ok(v) => v,
        Err(_) => panic!("instantiate failed"),
    };
    interp.send(instance, Symbol::intern(selector), vec![], &bindings, None)
}

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn int_of(v: &Value) -> i64 {
    match v {
        Value::SmallInt(n) => *n,
        _ => panic!("expected a SmallInt"),
    }
}

fn string_of(v: &Value) -> String {
    let Value::Obj(obj) = v else { panic!("expected a string object") };
    match &obj.borrow().data {
        ObjectData::Str(s) => s.clone(),
        _ => panic!("expected ObjectData::Str"),
    }
}

fn module(body: &str) -> String {
    format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  public run = ( {body} )\n) : ()")
}

#[test]
fn while_true_loops_until_the_condition_is_false_and_answers_nil() {
    let v = expect_ok(run_method(&module("| n | n := 0. [n < 3] whileTrue: [n := n + 1]. ^n"), "run"), "run");
    assert_eq!(int_of(&v), 3);
    let v = expect_ok(run_method(&module("^[false] whileTrue: [nil]"), "run"), "run");
    assert!(matches!(v, Value::Nil));
}

#[test]
fn while_false_loops_until_the_condition_is_true() {
    let v = expect_ok(run_method(&module("| n | n := 0. [n = 3] whileFalse: [n := n + 1]. ^n"), "run"), "run");
    assert_eq!(int_of(&v), 3);
}

#[test]
fn a_non_boolean_condition_sends_must_be_boolean() {
    match run_method(&module("^[3] whileTrue: [nil]"), "run") {
        Err(Signal::Error(e)) => {
            assert!(e.message.contains("does not understand #mustBeBoolean"), "{}", e.message)
        }
        _ => panic!("expected the mustBeBoolean doesNotUnderstand: error"),
    }
}

#[test]
fn a_non_local_return_escapes_a_while_loop() {
    let v = expect_ok(
        run_method(&module("| n | n := 0. [true] whileTrue: [n := n + 1. n = 5 ifTrue: [^n]]"), "run"),
        "run",
    );
    assert_eq!(int_of(&v), 5);
}

#[test]
fn if_false_if_true_picks_the_right_block() {
    let v = expect_ok(run_method(&module("^true ifFalse: ['f'] ifTrue: ['t']"), "run"), "run");
    assert_eq!(string_of(&v), "t");
    let v = expect_ok(run_method(&module("^false ifFalse: ['f'] ifTrue: ['t']"), "run"), "run");
    assert_eq!(string_of(&v), "f");
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test builtins_control_flow`
Expected: FAIL — every loop/`ifFalse:ifTrue:` send `doesNotUnderstand:`s.

- [x] **Step 3: Write the implementation**

In `crates/ns-runtime/src/builtins/block.rs`, update the module doc comment's first line to "`Block` (closure) native methods -- `value`..`value:value:value:`, `numArgs`, and (Milestone 2) `whileTrue:`/`whileFalse:`." Change the imports to `use crate::interpreter::EvalResult;` plus `use std::collections::HashMap; use std::rc::Rc; use ns_syntax::Span;`, and add two arms to `dispatch`'s `match selector.as_str()`:

```rust
        "whileTrue:" => Some(run_while(interp, &receiver, &closure.bindings, &args[0], sender, true)),
        "whileFalse:" => Some(run_while(interp, &receiver, &closure.bindings, &args[0], sender, false)),
```

and add at module scope:

```rust
/// `whileTrue:` (`expect == true`) / `whileFalse:` (`expect == false`):
/// evaluate `cond`; while its value is `Bool(expect)`, evaluate `body` and
/// repeat; a `Bool(!expect)` ends the loop with `nil`. A non-Boolean
/// condition value is sent `mustBeBoolean` (an ordinary virtual send --
/// `doesNotUnderstand:` by default) and that send's result is the loop
/// expression's value. The bytecode compiler's inlined loop reproduces
/// exactly this on its guard-failure path (M2 spec, "Control-flow
/// inlining"), so both engines agree without re-running the condition.
fn run_while(
    interp: &mut Interpreter,
    cond: &Value,
    bindings: &Rc<HashMap<Span, Binding>>,
    body: &Value,
    sender: Option<&FrameRef>,
    expect: bool,
) -> EvalResult {
    loop {
        match super::call_block(interp, cond, vec![], sender)? {
            Value::Bool(b) if b == expect => {
                super::call_block(interp, body, vec![], sender)?;
            }
            Value::Bool(_) => return Ok(Value::Nil),
            other => {
                return interp.send(other, Symbol::intern("mustBeBoolean"), vec![], bindings, sender.cloned())
            }
        }
    }
}
```

In `crates/ns-runtime/src/builtins/boolean.rs`, add one arm to `dispatch` directly after the `"ifTrue:ifFalse:"` arm:

```rust
        "ifFalse:ifTrue:" => Some(super::call_block(
            interp,
            if b { &args[1] } else { &args[0] },
            vec![],
            sender,
        )),
```

- [x] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test builtins_control_flow`
Expected: PASS (5 tests).

- [x] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): add native whileTrue:, whileFalse:, and ifFalse:ifTrue:

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 10: Control-flow inlining

**Files:**
- Create: `crates/ns-runtime/src/compiler/control_flow.rs`
- Modify: `crates/ns-runtime/src/compiler/mod.rs` (`pub(crate) mod control_flow;`)
- Modify: `crates/ns-runtime/src/compiler/lower.rs`
- Create: `tests-fixtures/while_loop.ns` / `.expected`, `tests-fixtures/control_flow_fallback.ns` / `.expected`
- Modify: `crates/ns-cli/tests/differential.rs`

**Interfaces:**
- Consumes: `Lowerer` (Tasks 2-4: `lower_stmt_inner`, `emit_nil`, `emit_send`, `emit_ordinary_send_with_receiver`, `scopes`), `ScopeKind::Inlined`.
- Produces: `control_flow::recognize(send: &ast::Send) -> Option<InlineForm<'_>>` (pure); `Lowerer::{lower_inline_block_body, emit_guarded_branch, emit_coerce_to_bool, emit_must_be_boolean, lower_if, lower_and_or, lower_while, patch_target, here}`; `lower_send`'s ordinary arm now inlines every recognized form.

**Design notes (the spec's "Control-flow inlining" section, implemented):**
- **The compile-time gate** (`recognize`): an ordinary explicit-receiver send (only the `_` arm of `lower_send` ever calls `recognize`), one of the eight selectors, every block-position argument — and the receiver, for `whileTrue:`/`whileFalse:` — a syntactic `Expr::Block` with **no parameters and no temporaries**. Anything else is an ordinary `SendPublic`, no guard machinery at all.
- **Inlined statements are sub-expressions of the enclosing statement**: `lower_inline_block_body` pushes `ScopeKind::Inlined`, lowers each statement with `lower_stmt_inner` (no `statement_pc` entry, no reset to `base`), resetting the allocator only to the watermark at the block's start between statements — so the guard register, the result register, and any already-evaluated sibling argument stay live. It returns the last statement's value register (`nil` for an empty block).
- **Lexical addressing** is already handled by Task 2's scope stack: with `Inlined` on the stack, `local_target` shortens `OuterLocal` depths by one per inlined scope crossed, for references directly inside the inlined body and for nested real blocks compiled inside it (their nested `Lowerer` inherits the stack).
- **`^` inside an inlined block** goes through Task 4's `return_instr`: `Return` in a method, `NonLocalReturn` in a block or an initializer.
- **One shared shape** (`emit_guarded_branch`) underlies `ifTrue:`/`ifFalse:`/`ifTrue:ifFalse:`/`ifFalse:ifTrue:`/`and:`/`or:`: `JumpIfNotBoolean` to a fallback, `BranchIfFalse` to a false-branch, true-branch, false-branch, fallback = a real `SendPublic` of the *original selector* on the already-evaluated receiver register with the block arguments lowered as real closures (`MakeClosure`), everything `Move`d into one result register. `and:`/`or:` coerce the block's value with `truthy_arg`'s rule (`emit_coerce_to_bool`: only exactly `Bool(true)` is true).
- **`whileTrue:`/`whileFalse:`** are a real back-edge loop; the guard is on the condition *value* and its failure path is `SendPublic mustBeBoolean` on the guard register — Task 9's native does the same, so nothing re-runs the condition and both engines agree. The loop's value is `nil`.
- Jump targets are patched after the fact via `patch_target`; a label is `here()` = the index the next emitted instruction will get. Every `Jump` to an "end" label lands on the enclosing code's next instruction, which always exists because `finish_body` terminates every body.

- [x] **Step 1: Write `control_flow::recognize` and its tests**

`crates/ns-runtime/src/compiler/control_flow.rs`:

```rust
//! Compile-time recognition of the standard Smalltalk control-flow
//! "special selectors" for inlining. Pure recognition only -- no bytecode
//! emission lives here (see `lower.rs`'s `lower_if`/`lower_and_or`/
//! `lower_while`). M2 spec, "Control-flow inlining".

use ns_syntax::ast;

pub(crate) enum InlineForm<'a> {
    /// `ifTrue:ifFalse:` and (arguments swapped) `ifFalse:ifTrue:`.
    IfTrueIfFalse(&'a ast::BlockExpr, &'a ast::BlockExpr),
    IfTrue(&'a ast::BlockExpr),
    IfFalse(&'a ast::BlockExpr),
    And(&'a ast::BlockExpr),
    Or(&'a ast::BlockExpr),
    WhileTrue { cond: &'a ast::BlockExpr, body: &'a ast::BlockExpr },
    WhileFalse { cond: &'a ast::BlockExpr, body: &'a ast::BlockExpr },
}

/// The gate's per-block condition: a syntactic block literal with no
/// parameters and no temporaries. A block with temporaries gets a fresh
/// activation (and fresh temps) per invocation in M1, which a closure
/// created inside a loop body could observe; a register cannot reproduce
/// that, so such blocks keep M1's real-send semantics.
fn as_inlinable_block(expr: &ast::Expr) -> Option<&ast::BlockExpr> {
    match expr {
        ast::Expr::Block(b) if b.params.is_empty() && b.body.temps.is_empty() => Some(b),
        _ => None,
    }
}

/// Only ever offered ordinary explicit-receiver sends (see `lower_send`);
/// looks at the message shape and the argument/receiver expressions only.
pub(crate) fn recognize(send: &ast::Send) -> Option<InlineForm<'_>> {
    let ast::Selector::Keyword(selector) = &send.message.selector else {
        return None;
    };
    let args = &send.message.args;
    match selector.as_str() {
        "ifTrue:ifFalse:" => Some(InlineForm::IfTrueIfFalse(
            as_inlinable_block(&args[0])?,
            as_inlinable_block(&args[1])?,
        )),
        "ifFalse:ifTrue:" => Some(InlineForm::IfTrueIfFalse(
            as_inlinable_block(&args[1])?,
            as_inlinable_block(&args[0])?,
        )),
        "ifTrue:" => Some(InlineForm::IfTrue(as_inlinable_block(&args[0])?)),
        "ifFalse:" => Some(InlineForm::IfFalse(as_inlinable_block(&args[0])?)),
        "and:" => Some(InlineForm::And(as_inlinable_block(&args[0])?)),
        "or:" => Some(InlineForm::Or(as_inlinable_block(&args[0])?)),
        "whileTrue:" => Some(InlineForm::WhileTrue {
            cond: as_inlinable_block(&send.receiver)?,
            body: as_inlinable_block(&args[0])?,
        }),
        "whileFalse:" => Some(InlineForm::WhileFalse {
            cond: as_inlinable_block(&send.receiver)?,
            body: as_inlinable_block(&args[0])?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_send(src: &str) -> ast::Send {
        let full = format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^{src} )\n) : ()");
        let module = ns_syntax::parse(&full).expect("parse");
        match &module.root_class.instance_side.methods[0].body.stmts[0] {
            ast::Stmt::Return(ast::Expr::Send(send)) => (**send).clone(),
            other => panic!("expected a return-of-send statement, got {other:?}"),
        }
    }

    #[test]
    fn recognizes_if_true_if_false_with_literal_blocks() {
        assert!(matches!(
            recognize(&parse_send("x ifTrue: [1] ifFalse: [2]")),
            Some(InlineForm::IfTrueIfFalse(..))
        ));
        assert!(matches!(
            recognize(&parse_send("x ifFalse: [1] ifTrue: [2]")),
            Some(InlineForm::IfTrueIfFalse(..))
        ));
        assert!(matches!(recognize(&parse_send("x and: [y]")), Some(InlineForm::And(_))));
    }

    #[test]
    fn does_not_recognize_a_non_literal_block_argument() {
        assert!(recognize(&parse_send("x ifTrue: y ifFalse: [2]")).is_none());
    }

    #[test]
    fn does_not_recognize_a_block_with_parameters_or_temporaries() {
        assert!(recognize(&parse_send("x ifTrue: [:p | p] ifFalse: [2]")).is_none());
        assert!(recognize(&parse_send("x ifTrue: [ | t | t ] ifFalse: [2]")).is_none());
    }

    #[test]
    fn recognizes_while_only_when_receiver_and_argument_are_both_block_literals() {
        assert!(matches!(
            recognize(&parse_send("[x] whileTrue: [y]")),
            Some(InlineForm::WhileTrue { .. })
        ));
        assert!(matches!(
            recognize(&parse_send("[x] whileFalse: [y]")),
            Some(InlineForm::WhileFalse { .. })
        ));
        assert!(recognize(&parse_send("x whileTrue: [y]")).is_none());
    }

    #[test]
    fn ignores_unary_binary_and_unrelated_keyword_selectors() {
        assert!(recognize(&parse_send("x foo")).is_none());
        assert!(recognize(&parse_send("x + [1]")).is_none());
        assert!(recognize(&parse_send("x at: [1]")).is_none());
    }
}
```

Add `pub(crate) mod control_flow;` to `crates/ns-runtime/src/compiler/mod.rs` (above `pub(crate) mod lower;`).

- [x] **Step 2: Run to verify the recognition tests pass**

Run: `cargo test -p ns-runtime control_flow::`
Expected: PASS (5 tests) — `recognize` depends on nothing Step 3 adds.

- [x] **Step 3: Write the failing `lower.rs` tests**

Add to `crates/ns-runtime/src/compiler/lower.rs`'s test module:

```rust
/// Compiles `foo: x = ( ^<body> )` and returns the method's code.
fn compile_foo(body: &str) -> CompiledCode {
    let full = format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^{body} )\n) : ()");
    lower_whole_method(&full, 0)
}

fn has<F: Fn(&Instr) -> bool>(code: &CompiledCode, pred: F) -> bool {
    code.instrs.iter().any(pred)
}

#[test]
fn if_true_if_false_with_literal_blocks_compiles_to_a_guarded_branch_with_a_fallback_send() {
    let code = compile_foo("x ifTrue: [1] ifFalse: [2]");
    assert!(has(&code, |i| matches!(i, Instr::JumpIfNotBoolean { reg: 0, .. })), "{:#?}", code.instrs);
    assert!(has(&code, |i| matches!(i, Instr::BranchIfFalse { reg: 0, .. })));
    assert!(has(&code, |i| matches!(i, Instr::LoadLiteral { literal: BcLiteral::SmallInt(1), .. })));
    assert!(has(&code, |i| matches!(i, Instr::LoadLiteral { literal: BcLiteral::SmallInt(2), .. })));
    // The fallback path is a real send of the original selector with real closures.
    assert!(has(&code, |i| matches!(i, Instr::SendPublic { selector, receiver_reg: 0, .. } if selector.as_str() == "ifTrue:ifFalse:")));
    assert_eq!(code.instrs.iter().filter(|i| matches!(i, Instr::MakeClosure { .. })).count(), 2);
}

#[test]
fn a_non_literal_block_argument_is_an_ordinary_send_with_no_guard() {
    let code = compile_foo("x ifTrue: x ifFalse: [2]");
    assert!(has(&code, |i| matches!(i, Instr::SendPublic { selector, .. } if selector.as_str() == "ifTrue:ifFalse:")));
    assert!(!has(&code, |i| matches!(i, Instr::JumpIfNotBoolean { .. })));
}

#[test]
fn a_block_with_temporaries_is_not_inlined() {
    let code = compile_foo("x ifTrue: [ | t | t := 1. t ] ifFalse: [2]");
    assert!(!has(&code, |i| matches!(i, Instr::JumpIfNotBoolean { .. })));
    assert!(has(&code, |i| matches!(i, Instr::MakeClosure { code, .. } if code.num_registers - code.num_args >= 1)));
}

#[test]
fn while_true_compiles_to_a_back_edge_jump_and_a_must_be_boolean_fallback() {
    let code = compile_foo("[x] whileTrue: [x]");
    let back_edge = code.instrs.iter().enumerate().find_map(|(i, instr)| match instr {
        Instr::Jump { target } if (*target as usize) < i => Some(*target),
        _ => None,
    });
    assert!(back_edge.is_some(), "expected a backward Jump, got {:#?}", code.instrs);
    assert!(has(&code, |i| matches!(i, Instr::SendPublic { selector, .. } if selector.as_str() == "mustBeBoolean")));
    assert!(!has(&code, |i| matches!(i, Instr::SendPublic { selector, .. } if selector.as_str() == "whileTrue:")));
    assert!(!has(&code, |i| matches!(i, Instr::MakeClosure { .. })), "an inlined loop materializes no closures");
    assert!(has(&code, |i| matches!(i, Instr::BranchIfFalse { .. })));
    let code = compile_foo("[x] whileFalse: [x]");
    assert!(has(&code, |i| matches!(i, Instr::BranchIfTrue { .. })));
}

#[test]
fn a_reference_inside_an_inlined_block_addresses_the_enclosing_frame_directly() {
    // `x` inside the inlined block is `OuterLocal { depth: 1, index: 0 }`
    // to the resolver; at run time it is the current frame's register 0.
    let code = compile_foo("x ifTrue: [x] ifFalse: [nil]");
    assert!(!has(&code, |i| matches!(i, Instr::LoadOuterLocal { .. })));
    assert!(has(&code, |i| matches!(i, Instr::Move { src: 0, .. })));
}

#[test]
fn a_real_block_nested_in_an_inlined_block_counts_one_hop_fewer() {
    // `[ x ]` sits two resolver scopes below the method (inner block ->
    // inlined block -> method) but only one frame below it at run time.
    let code = compile_foo("x ifTrue: [ [ x ] value ] ifFalse: [nil]");
    let nested = code.instrs.iter().find_map(|i| match i {
        Instr::MakeClosure { code, .. } => Some(code.clone()),
        _ => None,
    });
    let nested = nested.expect("the true branch creates a closure");
    assert_eq!(nested.instrs[0], Instr::LoadOuterLocal { dst: 0, depth: 1, index: 0 });
}

#[test]
fn a_return_inside_an_inlined_block_follows_the_enclosing_code_kind() {
    let code = compile_foo("x ifTrue: [^1] ifFalse: [^2]");
    assert_eq!(code.instrs.iter().filter(|i| matches!(i, Instr::Return { .. })).count(), 3, "{:#?}", code.instrs);
    assert!(!has(&code, |i| matches!(i, Instr::NonLocalReturn { .. })));
    // Inside a block body the same `^` is a non-local return.
    let code = compile_foo("[ x ifTrue: [^1] ifFalse: [^2] ]");
    let Some(Instr::MakeClosure { code: block, .. }) = code.instrs.first() else { panic!() };
    assert!(block.instrs.iter().any(|i| matches!(i, Instr::NonLocalReturn { .. })));
}

#[test]
fn inlined_statements_do_not_clobber_live_registers_of_the_enclosing_statement() {
    // `x foo: (x bar) with: (x ifTrue: [x baz. 1] ifFalse: [2])`: the
    // first argument's register must survive the inlined `x baz` statement.
    let code = compile_foo("x foo: (x bar) with: (x ifTrue: [x baz. 1] ifFalse: [2])");
    let bar_dst = code.instrs.iter().find_map(|i| match i {
        Instr::SendPublic { selector, dst, .. } if selector.as_str() == "bar" => Some(*dst),
        _ => None,
    }).unwrap();
    let baz_dst = code.instrs.iter().find_map(|i| match i {
        Instr::SendPublic { selector, dst, .. } if selector.as_str() == "baz" => Some(*dst),
        _ => None,
    }).unwrap();
    assert_ne!(bar_dst, baz_dst);
    let Some(Instr::SendPublic { selector, arg_regs, .. }) = code.instrs.iter().rev().find(|i| matches!(i, Instr::SendPublic { selector, .. } if selector.as_str() == "foo:with:")) else { panic!() };
    assert_eq!(selector.as_str(), "foo:with:");
    assert_eq!(arg_regs[0], bar_dst);
}

#[test]
fn and_or_coerce_the_block_value_and_short_circuit() {
    let code = compile_foo("x and: [x]");
    assert!(has(&code, |i| matches!(i, Instr::LoadLiteral { literal: BcLiteral::Bool(true), .. })));
    assert!(has(&code, |i| matches!(i, Instr::LoadLiteral { literal: BcLiteral::Bool(false), .. })));
    assert!(has(&code, |i| matches!(i, Instr::SendPublic { selector, .. } if selector.as_str() == "and:")));
    let code = compile_foo("x or: [x]");
    assert!(has(&code, |i| matches!(i, Instr::SendPublic { selector, .. } if selector.as_str() == "or:")));
}

#[test]
fn inlined_statements_are_inside_the_enclosing_statements_pc_range() {
    let code = compile_foo("x ifTrue: [x foo. x bar] ifFalse: [nil]");
    assert_eq!(code.statement_pc.len(), 1, "the inlined statements are part of statement 0");
    assert_eq!(code.statement_pc[0].1, 0..code.instrs.len());
}
```

- [x] **Step 4: Run tests to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: FAIL — `lower_send`'s ordinary arm does not consult `recognize` yet (no `JumpIfNotBoolean` anywhere).

- [x] **Step 5: Write the implementation**

In `crates/ns-runtime/src/compiler/lower.rs`, add `use super::control_flow::{self, InlineForm};` to the imports, and add to `impl<'a> Lowerer<'a>`:

```rust
/// The index the next emitted instruction will get -- a forward label.
fn here(&self) -> u32 {
    self.instrs.len() as u32
}

fn patch_target(&mut self, at: usize, target: u32) {
    match &mut self.instrs[at] {
        Instr::Jump { target: t }
        | Instr::JumpIfNotBoolean { target: t, .. }
        | Instr::BranchIfFalse { target: t, .. }
        | Instr::BranchIfTrue { target: t, .. } => *t = target,
        other => unreachable!("internal: patch_target on a non-jump instruction {other:?}"),
    }
}

/// Lowers `block`'s statements directly into the current instruction
/// stream -- no `MakeClosure`, no new activation. The block's statements
/// are sub-expressions of the enclosing statement: no `statement_pc`
/// entries, and the scratch allocator resets only to the watermark at the
/// block's start (never to `base`, which would clobber the enclosing
/// statement's live registers). Pushes `ScopeKind::Inlined` so local
/// references inside (and in nested real blocks) drop one frame hop.
/// Returns the register holding the block's value (`nil` if it has no
/// statements).
fn lower_inline_block_body(&mut self, block: &ast::BlockExpr) -> u16 {
    debug_assert!(
        block.params.is_empty() && block.body.temps.is_empty(),
        "internal: the inlining gate admits only parameterless, temp-less blocks"
    );
    self.scopes.push(ScopeKind::Inlined);
    let mark = self.next_scratch;
    let mut result = None;
    for stmt in &block.body.stmts {
        self.next_scratch = mark;
        result = self.lower_stmt_inner(stmt);
    }
    self.scopes.pop();
    match result {
        Some(reg) => reg,
        None => self.emit_nil(),
    }
}

/// `builtins::boolean::truthy_arg`'s rule in bytecode: only exactly
/// `Bool(true)` is true; a non-`true` `Bool` or a non-`Bool` is false.
fn emit_coerce_to_bool(&mut self, src: u16) -> u16 {
    let dst = self.alloc_scratch();
    let not_bool_jump = self.emit(Instr::JumpIfNotBoolean { reg: src, target: 0 });
    let false_jump = self.emit(Instr::BranchIfFalse { reg: src, target: 0 });
    self.emit(Instr::LoadLiteral { dst, literal: BcLiteral::Bool(true) });
    let after_true_jump = self.emit(Instr::Jump { target: 0 });
    let false_label = self.here();
    self.patch_target(not_bool_jump, false_label);
    self.patch_target(false_jump, false_label);
    self.emit(Instr::LoadLiteral { dst, literal: BcLiteral::Bool(false) });
    let end_label = self.here();
    self.patch_target(after_true_jump, end_label);
    dst
}

/// The guard-failure path of an inlined loop: what the native
/// `whileTrue:`/`whileFalse:` does with a non-Boolean condition value.
fn emit_must_be_boolean(&mut self, reg: u16) -> u16 {
    let selector = crate::Symbol::intern("mustBeBoolean");
    self.emit_send(|dst, call_site| Instr::SendPublic {
        selector,
        receiver_reg: reg,
        arg_regs: vec![],
        dst,
        call_site,
    })
}

/// The shared shape behind every one-shot inlined form: `guard_reg`
/// exactly `Bool(true)` runs `on_true`, exactly `Bool(false)` runs
/// `on_false`, anything else runs `on_fallback` (a real send). Each
/// returns the register holding its value; the chosen one is `Move`d into
/// `dst`.
fn emit_guarded_branch(
    &mut self,
    guard_reg: u16,
    dst: u16,
    on_true: impl FnOnce(&mut Self) -> u16,
    on_false: impl FnOnce(&mut Self) -> u16,
    on_fallback: impl FnOnce(&mut Self) -> u16,
) {
    let not_bool_jump = self.emit(Instr::JumpIfNotBoolean { reg: guard_reg, target: 0 });
    let false_jump = self.emit(Instr::BranchIfFalse { reg: guard_reg, target: 0 });
    let true_result = on_true(self);
    self.emit(Instr::Move { dst, src: true_result });
    let after_true_jump = self.emit(Instr::Jump { target: 0 });
    let false_label = self.here();
    self.patch_target(false_jump, false_label);
    let false_result = on_false(self);
    self.emit(Instr::Move { dst, src: false_result });
    let after_false_jump = self.emit(Instr::Jump { target: 0 });
    let fallback_label = self.here();
    self.patch_target(not_bool_jump, fallback_label);
    let fallback_result = on_fallback(self);
    self.emit(Instr::Move { dst, src: fallback_result });
    let end_label = self.here();
    self.patch_target(after_true_jump, end_label);
    self.patch_target(after_false_jump, end_label);
}

fn lower_if(
    &mut self,
    send: &ast::Send,
    true_block: Option<&ast::BlockExpr>,
    false_block: Option<&ast::BlockExpr>,
) -> u16 {
    let guard_reg = self.lower_expr(&send.receiver);
    let dst = self.alloc_scratch();
    self.emit_guarded_branch(
        guard_reg,
        dst,
        |lw| match true_block {
            Some(b) => lw.lower_inline_block_body(b),
            None => lw.emit_nil(),
        },
        |lw| match false_block {
            Some(b) => lw.lower_inline_block_body(b),
            None => lw.emit_nil(),
        },
        |lw| lw.emit_ordinary_send_with_receiver(guard_reg, &send.message),
    );
    dst
}

fn lower_and_or(&mut self, send: &ast::Send, block: &ast::BlockExpr, is_and: bool) -> u16 {
    let guard_reg = self.lower_expr(&send.receiver);
    let dst = self.alloc_scratch();
    let evaluate_and_coerce = |lw: &mut Self| {
        let b = lw.lower_inline_block_body(block);
        lw.emit_coerce_to_bool(b)
    };
    let fallback = |lw: &mut Self| lw.emit_ordinary_send_with_receiver(guard_reg, &send.message);
    if is_and {
        self.emit_guarded_branch(guard_reg, dst, evaluate_and_coerce, |_| guard_reg, fallback);
    } else {
        self.emit_guarded_branch(guard_reg, dst, |_| guard_reg, evaluate_and_coerce, fallback);
    }
    dst
}

/// A real loop: condition, guard, exit branch, body, back-edge. A
/// non-Boolean condition value takes the `mustBeBoolean` path and ends the
/// loop with that send's value; a normal exit leaves `nil` in `dst`.
fn lower_while(&mut self, cond: &ast::BlockExpr, body: &ast::BlockExpr, while_true: bool) -> u16 {
    let dst = self.emit_nil();
    let loop_start = self.here();
    let guard_reg = self.lower_inline_block_body(cond);
    let not_bool_jump = self.emit(Instr::JumpIfNotBoolean { reg: guard_reg, target: 0 });
    let exit_jump = if while_true {
        self.emit(Instr::BranchIfFalse { reg: guard_reg, target: 0 })
    } else {
        self.emit(Instr::BranchIfTrue { reg: guard_reg, target: 0 })
    };
    self.lower_inline_block_body(body);
    self.emit(Instr::Jump { target: loop_start });
    let fallback_label = self.here();
    self.patch_target(not_bool_jump, fallback_label);
    let fallback_result = self.emit_must_be_boolean(guard_reg);
    self.emit(Instr::Move { dst, src: fallback_result });
    let end_label = self.here();
    self.patch_target(exit_jump, end_label);
    dst
}
```

Replace the ordinary (`_`) arm of `lower_send` with:

```rust
_ => {
    if let Some(form) = control_flow::recognize(send) {
        return match form {
            InlineForm::IfTrueIfFalse(t, f) => self.lower_if(send, Some(t), Some(f)),
            InlineForm::IfTrue(t) => self.lower_if(send, Some(t), None),
            InlineForm::IfFalse(f) => self.lower_if(send, None, Some(f)),
            InlineForm::And(b) => self.lower_and_or(send, b, true),
            InlineForm::Or(b) => self.lower_and_or(send, b, false),
            InlineForm::WhileTrue { cond, body } => self.lower_while(cond, body, true),
            InlineForm::WhileFalse { cond, body } => self.lower_while(cond, body, false),
        };
    }
    let receiver_reg = self.lower_expr(&send.receiver);
    self.emit_ordinary_send_with_receiver(receiver_reg, &send.message)
}
```

- [x] **Step 6: Run tests to verify they pass**

Run: `cargo test -p ns-runtime`
Expected: PASS (every `compiler::` test, and every `vm`/`vm_initializer` test still green — `if_true_if_false_dispatches_through_the_native_boolean_with_compiled_blocks` now runs inlined and must produce the same values).

- [x] **Step 7: Add the two fixtures**

`tests-fixtures/while_loop.ns`:

```
Newspeak3
'Test'
class WhileLoop usingPlatform: platform = Object new (
	| n ::= 0. sum ::= 0. |
) (
	public main: p args: a = (
		[n < 5] whileTrue: [sum := sum + n. n := n + 1].
		[n = 0] whileFalse: [n := n - 1].
		p transcript show: sum printString; show: ' '; show: n printString; newLine
	)
) : ()
```

`tests-fixtures/while_loop.expected`:

```
10 0
```

`tests-fixtures/control_flow_fallback.ns` — the guard *fails* (the receiver is not a Boolean) and the fallback send finds a user-implemented `ifTrue:ifFalse:`; the second line takes the inlined fast path:

```
Newspeak3
'Test'
class ControlFlowFallback usingPlatform: platform = Object new (
) (
	me = ( ^self )
	public ifTrue: t ifFalse: f = ( ^f value )
	public main: p args: a = (
		p transcript show: (me ifTrue: ['inlined'] ifFalse: ['fell back']); newLine.
		p transcript show: (3 < 4 ifTrue: ['fast path'] ifFalse: ['?']); newLine
	)
) : ()
```

`tests-fixtures/control_flow_fallback.expected`:

```
fell back
fast path
```

Confirm both run under `--engine tree-walk` first (`cargo run -p ns-cli -- run tests-fixtures/while_loop.ns --engine tree-walk`) — that exercises Task 9's natives only.

- [x] **Step 8: Add the inline-source differential tests**

Add to `crates/ns-cli/tests/differential.rs`:

```rust
#[test]
fn a_non_boolean_receiver_of_an_inlined_selector_does_not_understand_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "if-dnu",
        "Newspeak3\n'T'\nclass IfDnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 ifTrue: ['a'] ifFalse: ['b']\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert!(outcome.stderr.contains("does not understand #ifTrue:ifFalse:"), "{}", outcome.stderr);
}

#[test]
fn a_non_boolean_while_condition_fails_with_must_be_boolean_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "while-must-be-boolean",
        "Newspeak3\n'T'\nclass WhileMbb usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\tp transcript show: 'before'; newLine.\n\t\t[p transcript show: 'cond'; newLine. 3] whileTrue: [nil]\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert_eq!(outcome.stdout, "before\ncond\n", "the condition body must run exactly once");
    assert!(outcome.stderr.contains("does not understand #mustBeBoolean"), "{}", outcome.stderr);
}

#[test]
fn a_variable_block_receiver_takes_the_native_loop_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "while-variable",
        "Newspeak3\n'T'\nclass WhileVar usingPlatform: platform = Object new (\n\t| n ::= 0. |\n) (\n\tpublic main: p args: a = (\n\t\t| cond |\n\t\tcond := [n < 3].\n\t\tcond whileTrue: [n := n + 1].\n\t\tp transcript show: n printString; newLine\n\t)\n) : ()\n",
    );
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "3\n");
}
```

- [x] **Step 9: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS — `run_fixtures.rs` (5 fixtures, all exit `0`), `differential.rs` (every fixture agrees; the three inline sources agree, including the two that exit non-zero with identical stderr).

- [x] **Step 10: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): inline ifTrue:/ifFalse:/and:/or:/whileTrue:/whileFalse: with guarded fallbacks

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 11: `ns-next disasm`

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (add `disassemble`)
- Modify: `crates/ns-cli/Cargo.toml` (add `insta` as a dev-dependency)
- Modify: `crates/ns-cli/src/main.rs`
- Create: `crates/ns-cli/tests/disasm_snapshots.rs`

**Interfaces:**
- Consumes: `compiler::{compile_method, compile_initializer}` (Task 5), `load_source`/`bootstrap_with_engine` (Task 8).
- Produces: `pub fn bytecode::disassemble(code: &CompiledCode, name: &str) -> String`; `ns-next disasm <file.ns> (<selector> | --initializer)`.

**Design notes:**
- `disassemble` is a pure formatter over `CompiledCode` (spec: "close to free: it's a formatter over `CompiledCode`, no new compilation path"). It lives in `bytecode.rs` next to the data it formats so it can be unit-tested without the CLI; the CLI command only chooses what to compile. Nested block code is printed indented under its `MakeClosure` (spec: "`disasm` and the initializer — CLARIFIED").
- `disasm` bootstraps and loads (so `compile_initializer` sees the loaded class's `slot_index`) but never instantiates and never installs the bytecode engine — compile only.
- Output format, fixed so the `insta` snapshots are stable: a header line `<kind> <name> (args N, registers M)`, then one line per instruction: `<index:4>  <stmt N or ->  <mnemonic operands>`; a nested block's listing follows its `MakeClosure` line, indented by 8 spaces. Registers print as `rN`, selectors as `#sel`, call sites as `@N`, jump targets as `-> N`.

- [x] **Step 1: Write the failing unit test**

Add to `crates/ns-runtime/src/bytecode.rs`'s test module:

```rust
#[test]
fn disassemble_lists_instructions_with_statement_labels_and_nested_blocks() {
    let block = Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instrs: vec![Instr::LoadOuterLocal { dst: 0, depth: 1, index: 0 }, Instr::NonLocalReturn { reg: 0 }],
        num_args: 0,
        num_registers: 1,
        statement_pc: vec![(0, 0..2)],
        call_sites: vec![],
        source: Span::new(0, 5),
    });
    let code = CompiledCode {
        kind: CodeKind::Method,
        instrs: vec![
            Instr::MakeClosure { dst: 1, params: Rc::new(vec![]), code: block },
            Instr::SendPublic {
                selector: Symbol::intern("value"),
                receiver_reg: 1,
                arg_regs: vec![],
                dst: 2,
                call_site: CallSite { call_site_id: 1 },
            },
            Instr::LoadSelf { dst: 3 },
            Instr::ImplicitReturn { reg: 3 },
        ],
        num_args: 1,
        num_registers: 4,
        statement_pc: vec![(0, 0..2)],
        call_sites: vec![CallSite { call_site_id: 1 }],
        source: Span::new(0, 10),
    };
    let text = disassemble(&code, "foo:");
    let expected = "\
method foo: (args 1, registers 4)
   0   stmt 0  MakeClosure r1, [] ->
        block block (args 0, registers 1)
           0   stmt 0  LoadOuterLocal r0, depth 1 index 0
           1   stmt 0  NonLocalReturn r0
   1   stmt 0  SendPublic r2 <- r1 #value () @1
   2        -  LoadSelf r3
   3        -  ImplicitReturn r3
";
    assert_eq!(text, expected);
}

#[test]
fn format_literal_covers_every_variant() {
    let lits = vec![
        Literal::Nil,
        Literal::Bool(true),
        Literal::SmallInt(-3),
        Literal::Float(2.5),
        Literal::Str("s".to_string()),
        Literal::Sym(Symbol::intern("sym")),
        Literal::Char('c'),
        Literal::Array(vec![Literal::SmallInt(1), Literal::Nil]),
        Literal::MalformedInt("16r1.8".to_string()),
    ];
    let rendered: Vec<String> = lits.iter().map(format_literal).collect();
    assert_eq!(
        rendered,
        vec!["nil", "true", "-3", "2.5", "'s'", "#sym", "\"c\"", "#(1 nil)", "malformed-int '16r1.8'"]
    );
}
```

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ns-runtime bytecode::`
Expected: FAIL — `disassemble`/`format_literal` not found.

- [x] **Step 3: Write `disassemble`**

Add to `crates/ns-runtime/src/bytecode.rs` (before the test module):

```rust
/// A human-readable listing of `code` for `ns-next disasm`. Nested block
/// code is printed indented under its `MakeClosure`. Pure formatting.
pub fn disassemble(code: &CompiledCode, name: &str) -> String {
    let mut out = String::new();
    write_code(&mut out, code, name, 0);
    out
}

fn write_code(out: &mut String, code: &CompiledCode, name: &str, indent: usize) {
    use std::fmt::Write;
    let pad = " ".repeat(indent);
    let kind = match code.kind {
        CodeKind::Method => "method",
        CodeKind::Block => "block",
        CodeKind::Initializer => "initializer",
    };
    writeln!(out, "{pad}{kind} {name} (args {}, registers {})", code.num_args, code.num_registers)
        .expect("writing to a String cannot fail");
    for (idx, instr) in code.instrs.iter().enumerate() {
        let stmt = code
            .statement_pc
            .iter()
            .find(|(_, range)| range.contains(&idx))
            .map(|(s, _)| format!("stmt {s}"))
            .unwrap_or_else(|| "-".to_string());
        writeln!(out, "{pad}{idx:4}  {stmt:>7}  {}", format_instr(instr))
            .expect("writing to a String cannot fail");
        if let Instr::MakeClosure { code: block, .. } = instr {
            write_code(out, block, "block", indent + 8);
        }
    }
}

fn regs(regs: &[u16]) -> String {
    regs.iter().map(|r| format!("r{r}")).collect::<Vec<_>>().join(", ")
}

fn format_instr(instr: &Instr) -> String {
    match instr {
        Instr::LoadLiteral { dst, literal } => format!("LoadLiteral r{dst}, {}", format_literal(literal)),
        Instr::LoadSelf { dst } => format!("LoadSelf r{dst}"),
        Instr::LoadBuiltinClass { dst, kind } => format!("LoadBuiltinClass r{dst}, {kind:?}"),
        Instr::Move { dst, src } => format!("Move r{dst}, r{src}"),
        Instr::StoreLocal { src, slot } => format!("StoreLocal r{slot} <- r{src}"),
        Instr::LoadOuterLocal { dst, depth, index } => {
            format!("LoadOuterLocal r{dst}, depth {depth} index {index}")
        }
        Instr::StoreOuterLocal { src, depth, index } => {
            format!("StoreOuterLocal depth {depth} index {index} <- r{src}")
        }
        Instr::SendLexical { depth, selector, arg_regs, dst, call_site } => format!(
            "SendLexical r{dst} <- depth {depth} #{selector} ({}) @{}",
            regs(arg_regs),
            call_site.call_site_id
        ),
        Instr::SendPublic { selector, receiver_reg, arg_regs, dst, call_site } => format!(
            "SendPublic r{dst} <- r{receiver_reg} #{selector} ({}) @{}",
            regs(arg_regs),
            call_site.call_site_id
        ),
        Instr::SendSuper { selector, arg_regs, dst, call_site } => format!(
            "SendSuper r{dst} <- #{selector} ({}) @{}",
            regs(arg_regs),
            call_site.call_site_id
        ),
        Instr::MakeClosure { dst, params, .. } => format!(
            "MakeClosure r{dst}, [{}] ->",
            params.iter().map(|p| format!(":{p}")).collect::<Vec<_>>().join(" ")
        ),
        Instr::MakeArray { dst, elem_regs } => format!("MakeArray r{dst}, ({})", regs(elem_regs)),
        Instr::InitSlot { src, field_index } => format!("InitSlot field {field_index} <- r{src}"),
        Instr::Return { reg } => format!("Return r{reg}"),
        Instr::ImplicitReturn { reg } => format!("ImplicitReturn r{reg}"),
        Instr::NonLocalReturn { reg } => format!("NonLocalReturn r{reg}"),
        Instr::Jump { target } => format!("Jump -> {target}"),
        Instr::JumpIfNotBoolean { reg, target } => format!("JumpIfNotBoolean r{reg} -> {target}"),
        Instr::BranchIfFalse { reg, target } => format!("BranchIfFalse r{reg} -> {target}"),
        Instr::BranchIfTrue { reg, target } => format!("BranchIfTrue r{reg} -> {target}"),
    }
}

fn format_literal(literal: &Literal) -> String {
    match literal {
        Literal::Nil => "nil".to_string(),
        Literal::Bool(b) => b.to_string(),
        Literal::SmallInt(n) => n.to_string(),
        Literal::Float(f) => format!("{f:?}"),
        Literal::Str(s) => format!("'{s}'"),
        Literal::Sym(s) => format!("#{s}"),
        Literal::Char(c) => format!("\"{c}\""),
        Literal::Array(elements) => format!(
            "#({})",
            elements.iter().map(format_literal).collect::<Vec<_>>().join(" ")
        ),
        Literal::MalformedInt(text) => format!("malformed-int '{text}'"),
    }
}
```

Run: `cargo test -p ns-runtime bytecode::` — Expected: PASS (7 tests).

- [x] **Step 4: Add the `insta` dev-dependency and write the failing CLI snapshot tests**

In `crates/ns-cli/Cargo.toml`, add:

```toml
[dev-dependencies]
insta.workspace = true
```

`crates/ns-cli/tests/disasm_snapshots.rs`:

```rust
//! `ns-next disasm` snapshot tests -- one per representative code shape
//! (M2 spec, "Testing": plain keyword method, block with non-local return,
//! inlined ifTrue:ifFalse:, inlined whileTrue:, the non-inlined fallback
//! case, an instance initializer). Catches accidental codegen changes.

use std::path::Path;
use std::process::Command;

fn disasm(file: &str, args: &[&str]) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures").join(file);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&path)
        .args(args)
        .output()
        .expect("run ns-next disasm");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

#[test]
fn disasm_arithmetic_main() {
    insta::assert_snapshot!(disasm("arithmetic.ns", &["main:args:"]));
}

#[test]
fn disasm_arithmetic_initializer() {
    insta::assert_snapshot!(disasm("arithmetic.ns", &["--initializer"]));
}

#[test]
fn disasm_blocks_and_return_find_in() {
    insta::assert_snapshot!(disasm("blocks_and_return.ns", &["find:in:"]));
}

#[test]
fn disasm_while_loop_main() {
    insta::assert_snapshot!(disasm("while_loop.ns", &["main:args:"]));
}

#[test]
fn disasm_control_flow_fallback_main() {
    insta::assert_snapshot!(disasm("control_flow_fallback.ns", &["main:args:"]));
}

#[test]
fn disasm_instance_init_initializer() {
    insta::assert_snapshot!(disasm("instance_init.ns", &["--initializer"]));
}

#[test]
fn disasm_rejects_an_unknown_selector_and_conflicting_arguments() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures/hello.ns");
    let missing = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm").arg(&path).arg("nope").output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no method `nope`"));
    let both = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm").arg(&path).arg("main:args:").arg("--initializer").output().unwrap();
    assert!(!both.status.success());
    assert!(String::from_utf8_lossy(&both.stderr).contains("exactly one"));
}
```

- [x] **Step 5: Run to verify they fail**

Run: `cargo test -p ns-cli --test disasm_snapshots`
Expected: FAIL — `disasm` is not a recognized subcommand.

- [x] **Step 6: Write the CLI command**

In `crates/ns-cli/src/main.rs`, add a `Disasm` variant to `Cmd`:

```rust
    /// Compile one method (or, with --initializer, the root class's
    /// instance initializer) and print its bytecode. Compile only: nothing
    /// is instantiated or run.
    Disasm {
        file: PathBuf,
        /// The method selector, e.g. `main:args:`.
        selector: Option<String>,
        /// Disassemble the root class's compiled instance initializer instead.
        #[arg(long)]
        initializer: bool,
    },
```

Add to `main`'s dispatch:

```rust
        Cmd::Disasm { file, selector, initializer } => cmd_disasm(&file, selector.as_deref(), initializer),
```

Add the command:

```rust
fn cmd_disasm(file: &Path, selector: Option<&str>, initializer: bool) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    // Bootstrap and load (compile_initializer needs the loaded class's
    // slot indices), but never instantiate: this is compile only.
    let (_interp, class) = bootstrap_with_engine(&loaded, EngineArg::TreeWalk);
    let (code, name) = match (selector, initializer) {
        (None, true) => {
            let class_name = match &class.borrow().data {
                ns_runtime::ObjectData::Class(data) => data.name.to_string(),
                _ => "?".to_string(),
            };
            (ns_runtime::compiler::compile_initializer(&class, &bindings), class_name)
        }
        (Some(selector), false) => {
            let method = match &class.borrow().data {
                ns_runtime::ObjectData::Class(data) => {
                    data.methods.get(&ns_runtime::Symbol::intern(selector)).cloned()
                }
                _ => None,
            };
            let Some(method) = method else {
                eprintln!("error: no method `{selector}` found on the root class");
                return ExitCode::FAILURE;
            };
            (ns_runtime::compiler::compile_method(&method, &bindings), selector.to_string())
        }
        _ => {
            eprintln!("error: disasm takes exactly one of <selector> or --initializer");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", ns_runtime::bytecode::disassemble(&code, &name));
    ExitCode::SUCCESS
}
```

- [x] **Step 7: Accept the baseline snapshots**

Run: `cargo test -p ns-cli --test disasm_snapshots`
Expected: the six snapshot tests fail with `.snap.new` files created under `crates/ns-cli/tests/snapshots/` (no prior baseline); `disasm_rejects_...` passes. **Review every printed listing** before accepting: `arithmetic.ns --initializer` must show one `InitSlot field 0`; `find:in:` must show a nested block with `NonLocalReturn`; `while_loop.ns` must show two backward `Jump -> N` and two `SendPublic ... #mustBeBoolean` but no `#whileTrue:`/`#whileFalse:` send; `control_flow_fallback.ns` must show `JumpIfNotBoolean` guards plus `SendPublic ... #ifTrue:ifFalse:` fallbacks with two `MakeClosure`s each; `instance_init.ns --initializer` must show three `InitSlot`s then the two `initExpr` statements. Then accept with `cargo insta accept` (or, without the `cargo-insta` subcommand installed, `INSTA_UPDATE=always cargo test -p ns-cli --test disasm_snapshots`), and commit the `.snap` files.

- [x] **Step 8: Run tests to verify they pass**

Run: `cargo test -p ns-cli --test disasm_snapshots`
Expected: PASS (7 tests) against the committed baseline.

- [x] **Step 9: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat: add ns-next disasm with a bytecode listing formatter

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 12: Image encoding primitives — `Writer`/`Reader`, header, scalar values

**Files:**
- Create: `crates/ns-runtime/src/image/mod.rs`
- Create: `crates/ns-runtime/src/image/encode.rs`
- Create: `crates/ns-runtime/src/image/decode.rs`
- Modify: `crates/ns-runtime/src/lib.rs`
- Create: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces: `encode::{Writer, MAGIC, FORMAT_VERSION, write_header, encode_scalar_value}`, `decode::{Reader, DecodeError, read_header, decode_scalar_value}` — all `pub` inside `pub mod image`.

**Design notes:**
- Hand-rolled, not `serde`/`bincode` (Global Constraints) — a small `Writer`/`Reader` pair over `Vec<u8>`/`&[u8]`: little-endian integers, IEEE-754 `binary64` floats, `u32`-length-prefixed UTF-8 strings, one leading tag byte per sum-typed value. This is the spec's "Canonical encodings" made concrete.
- **Refuse on any version mismatch** — `read_header` returns `Err(DecodeError::VersionMismatch { .. })`; no migration. `FORMAT_VERSION` also versions the compiler's codegen (spec: "Code table"): any change to `Instr` or to what the compiler emits bumps it.
- A corrupt-but-version-matching image is a `DecodeError` (never a panic) at the byte level — cheap to keep honest, and the CLI turns it into an error message.

- [x] **Step 1: Write the failing tests**

`crates/ns-runtime/tests/image.rs`:

```rust
//! Image format round-trip tests, built up task by task: header + scalars
//! (Task 12), the code table (Task 13), the object + frame tables (Task 14).

use ns_runtime::image::{decode, encode};
use ns_runtime::Value;

#[test]
fn header_round_trips() {
    let mut w = encode::Writer::new();
    encode::write_header(&mut w);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    decode::read_header(&mut r).expect("valid header");
}

#[test]
fn a_bad_magic_is_rejected() {
    let bytes = vec![b'X', b'X', b'X', b'X', 1, 0, 0, 0];
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(decode::read_header(&mut r), Err(decode::DecodeError::BadMagic));
}

#[test]
fn a_version_mismatch_is_rejected_not_migrated() {
    let mut w = encode::Writer::new();
    w.write_raw(encode::MAGIC);
    w.write_u32(9999);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(
        decode::read_header(&mut r),
        Err(decode::DecodeError::VersionMismatch { found: 9999, expected: encode::FORMAT_VERSION })
    );
}

#[test]
fn a_truncated_image_is_an_error_not_a_panic() {
    let bytes = vec![b'N', b'S'];
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(decode::read_header(&mut r), Err(decode::DecodeError::UnexpectedEof));
}

#[test]
fn every_scalar_value_variant_round_trips() {
    for v in [Value::Nil, Value::Bool(true), Value::Bool(false), Value::SmallInt(-42), Value::Float(3.5)] {
        let mut w = encode::Writer::new();
        assert!(encode::encode_scalar_value(&mut w, &v));
        let bytes = w.into_bytes();
        let mut r = decode::Reader::new(&bytes);
        let decoded = decode::decode_scalar_value(&mut r).expect("decode");
        match (&v, &decoded) {
            (Value::Nil, Value::Nil) => {}
            (Value::Bool(a), Value::Bool(b)) => assert_eq!(a, b),
            (Value::SmallInt(a), Value::SmallInt(b)) => assert_eq!(a, b),
            (Value::Float(a), Value::Float(b)) => assert_eq!(a.to_bits(), b.to_bits()),
            _ => panic!("round-trip mismatch"),
        }
    }
}

#[test]
fn integers_and_strings_are_little_endian_and_length_prefixed() {
    let mut w = encode::Writer::new();
    w.write_u16(0x0102);
    w.write_u32(0x01020304);
    w.write_i64(-1);
    w.write_str("hi");
    w.write_bool(true);
    let bytes = w.into_bytes();
    assert_eq!(&bytes[0..2], &[0x02, 0x01]);
    assert_eq!(&bytes[2..6], &[0x04, 0x03, 0x02, 0x01]);
    assert_eq!(&bytes[6..14], &[0xff; 8]);
    assert_eq!(&bytes[14..20], &[2, 0, 0, 0, b'h', b'i']);
    assert_eq!(bytes[20], 1);
    let mut r = decode::Reader::new(&bytes);
    assert_eq!(r.read_u16().unwrap(), 0x0102);
    assert_eq!(r.read_u32().unwrap(), 0x01020304);
    assert_eq!(r.read_i64().unwrap(), -1);
    assert_eq!(r.read_str().unwrap(), "hi");
    assert!(r.read_bool().unwrap());
    assert_eq!(r.read_u8(), Err(decode::DecodeError::UnexpectedEof));
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test image`
Expected: FAIL — `ns_runtime::image` not found.

- [x] **Step 3: Write the implementation**

`crates/ns-runtime/src/image/encode.rs`:

```rust
//! Canonical, little-endian, self-describing byte encoding for the image
//! format -- the spec's "Image format" section. Never a memory dump.

use crate::types::Value;

pub const MAGIC: &[u8; 4] = b"NSIM";
/// Versions the whole layout *and* the compiler's codegen: any change to
/// `Instr`, to what the compiler emits, or to any section below bumps it.
/// A mismatch is refused on load, never migrated (Milestone 2 Non-Goals).
pub const FORMAT_VERSION: u32 = 1;

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { buf: Vec::new() }
    }

    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_u8(u8::from(v));
    }

    pub fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// `u32` byte-count prefix, then the raw bytes.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u32(bytes.len() as u32);
        self.write_raw(bytes);
    }

    pub fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

pub fn write_header(w: &mut Writer) {
    w.write_raw(MAGIC);
    w.write_u32(FORMAT_VERSION);
}

/// Encodes the four non-`Obj` `Value` variants (tags 0-4); returns `false`
/// for `Value::Obj`, which the object-graph encoder (Task 14) handles.
pub fn encode_scalar_value(w: &mut Writer, v: &Value) -> bool {
    match v {
        Value::Nil => w.write_u8(0),
        Value::Bool(false) => w.write_u8(1),
        Value::Bool(true) => w.write_u8(2),
        Value::SmallInt(n) => {
            w.write_u8(3);
            w.write_i64(*n);
        }
        Value::Float(f) => {
            w.write_u8(4);
            w.write_f64(*f);
        }
        Value::Obj(_) => return false,
    }
    true
}
```

`crates/ns-runtime/src/image/decode.rs`:

```rust
//! Canonical decoding, the inverse of `encode.rs`. A malformed byte stream
//! is a `DecodeError`, never a panic.

use crate::types::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEof,
    BadMagic,
    VersionMismatch { found: u32, expected: u32 },
    InvalidTag { what: &'static str, tag: u8 },
    InvalidUtf8,
    BadCodeId(u32),
    BadObjectId(u32),
    BadFrameId(u32),
    UnknownSelector(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::UnexpectedEof => write!(f, "unexpected end of image"),
            DecodeError::BadMagic => write!(f, "not an ns-next image (bad magic)"),
            DecodeError::VersionMismatch { found, expected } => {
                write!(f, "image format version {found} is not supported (this build reads version {expected})")
            }
            DecodeError::InvalidTag { what, tag } => write!(f, "invalid {what} tag {tag}"),
            DecodeError::InvalidUtf8 => write!(f, "invalid UTF-8 in image string"),
            DecodeError::BadCodeId(id) => write!(f, "code id {id} out of range"),
            DecodeError::BadObjectId(id) => write!(f, "object id {id} out of range"),
            DecodeError::BadFrameId(id) => write!(f, "frame id {id} out of range"),
            DecodeError::UnknownSelector(s) => write!(f, "image refers to method `{s}`, which the reloaded source does not define"),
        }
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    pub fn read_raw(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(len).ok_or(DecodeError::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.read_raw(1)?[0])
    }

    pub fn read_bool(&mut self) -> Result<bool, DecodeError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(DecodeError::InvalidTag { what: "bool", tag }),
        }
    }

    pub fn read_u16(&mut self) -> Result<u16, DecodeError> {
        let bytes: [u8; 2] = self.read_raw(2)?.try_into().expect("read_raw(2) returns 2 bytes");
        Ok(u16::from_le_bytes(bytes))
    }

    pub fn read_u32(&mut self) -> Result<u32, DecodeError> {
        let bytes: [u8; 4] = self.read_raw(4)?.try_into().expect("read_raw(4) returns 4 bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn read_i64(&mut self) -> Result<i64, DecodeError> {
        let bytes: [u8; 8] = self.read_raw(8)?.try_into().expect("read_raw(8) returns 8 bytes");
        Ok(i64::from_le_bytes(bytes))
    }

    pub fn read_f64(&mut self) -> Result<f64, DecodeError> {
        let bytes: [u8; 8] = self.read_raw(8)?.try_into().expect("read_raw(8) returns 8 bytes");
        Ok(f64::from_le_bytes(bytes))
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let len = self.read_u32()? as usize;
        Ok(self.read_raw(len)?.to_vec())
    }

    pub fn read_str(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.read_bytes()?).map_err(|_| DecodeError::InvalidUtf8)
    }
}

pub fn read_header(r: &mut Reader) -> Result<(), DecodeError> {
    let magic = r.read_raw(4)?;
    if magic != super::encode::MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = r.read_u32()?;
    if version != super::encode::FORMAT_VERSION {
        return Err(DecodeError::VersionMismatch { found: version, expected: super::encode::FORMAT_VERSION });
    }
    Ok(())
}

pub fn decode_scalar_value(r: &mut Reader) -> Result<Value, DecodeError> {
    match r.read_u8()? {
        0 => Ok(Value::Nil),
        1 => Ok(Value::Bool(false)),
        2 => Ok(Value::Bool(true)),
        3 => Ok(Value::SmallInt(r.read_i64()?)),
        4 => Ok(Value::Float(r.read_f64()?)),
        tag => Err(DecodeError::InvalidTag { what: "value", tag }),
    }
}
```

`crates/ns-runtime/src/image/mod.rs`:

```rust
//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! "Image format" section. Layout: header, source text, `SourceOrigin`,
//! code table, object table, frame table, roots.

pub mod decode;
pub mod encode;
```

- [x] **Step 4: Wire into `lib.rs`**

Add `pub mod image;` to `crates/ns-runtime/src/lib.rs` (alphabetical, after `dispatch`).

- [x] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test image`
Expected: PASS (6 tests).

- [x] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): add the image format's header, byte primitives, and scalar values

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 13: Image code table — `CompiledCode` encoding and the root list

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs`
- Modify: `crates/ns-runtime/src/image/decode.rs`
- Modify: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: `bytecode::{CompiledCode, CompiledModule, Instr, Literal, CallSite, CodeKind, method_key, class_key}`, `types::BuiltinClass`.
- Produces: `encode::CodeIds` (`pub fn id_of(&self, code: &Rc<CompiledCode>) -> u32`), `encode::encode_code_table(w: &mut Writer, module: &CompiledModule, root_class: &ClassRef) -> CodeIds`, `encode::builtin_class_tag(kind: BuiltinClass) -> u8`, `decode::DecodedCodeTable { pub codes: Vec<Rc<CompiledCode>>, pub methods: Vec<(String, u32)>, pub initializer: Option<u32> }`, `decode::decode_code_table(r: &mut Reader) -> Result<DecodedCodeTable, DecodeError>`, `decode::builtin_class_from_tag(tag: u8) -> Option<BuiltinClass>`, `decode::install_code_table(table: &DecodedCodeTable, root_class: &ClassRef, bindings: &Rc<HashMap<Span, Binding>>) -> Result<CompiledModule, DecodeError>`.

**Design notes:**
- **The bytecode is part of the image** (spec: "Code table"): every `CompiledCode` reachable from the module — each method, the initializer, every nested block body — in a flat, id-addressed table, *blocks before the code that creates them* (post-order), so a `MakeClosure`'s `code` operand encodes as an already-assigned `code_id`. Then a root list: `(selector, code_id)` per method **sorted by selector** (determinism — `ClassData.methods` is a `HashMap`) and the initializer's id.
- **On load the table is installed as the `CompiledModule`** — re-keyed to the *reloaded* `Rc<MethodDecl>`s by selector and to the reloaded root class — so the image's bytecode is what runs after a load, not a recompilation. A selector the reloaded source no longer defines is `DecodeError::UnknownSelector`.
- `BuiltinClass` gets an explicit `u8` tag table in both directions (never `as u8` on the enum — declaration order is not a wire format). A test round-trips all fourteen variants.
- `Instr` tags are `0..=19` in declaration order of Task 1's enum; `Literal` tags `0..=8`; `CodeKind` `0..=2`. The test `every_instr_variant_round_trips` is the guard that no variant is forgotten on either side.

- [x] **Step 1: Write the failing tests**

Add to `crates/ns-runtime/tests/image.rs`:

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{
    class_key, method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal,
};
use ns_runtime::{compiler, Binding, BuiltinClass, ClassRef, Interpreter, ObjectData, SourceOrigin, Symbol};
use ns_syntax::Span;

type Bindings = Rc<HashMap<Span, Binding>>;

fn load(src: &str) -> (Interpreter, ClassRef, Bindings) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved =
        ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns")))
            .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, bindings)
}

const RICH: &str = "Newspeak3\n'T'\nclass Rich x: a = Object new (\n  | p = a. q ::= 0. |\n  q := p.\n) (\n  find: t in: c = ( c do: [:e | e = t ifTrue: [^e]]. ^nil )\n  loop = ( | n | n := 0. [n < 3] whileTrue: [n := n + 1]. ^n )\n  arr = ( ^{ 1. #(2 3). 'four' } )\n) : ()";

fn every_instr_once() -> CompiledCode {
    let block = Rc::new(CompiledCode {
        kind: CodeKind::Block,
        instrs: vec![Instr::NonLocalReturn { reg: 0 }],
        num_args: 1,
        num_registers: 1,
        statement_pc: vec![(0, 0..1)],
        call_sites: vec![],
        source: Span::new(1, 2),
    });
    let cs = CallSite { call_site_id: 7 };
    CompiledCode {
        kind: CodeKind::Initializer,
        instrs: vec![
            Instr::LoadLiteral { dst: 0, literal: Literal::Array(vec![Literal::Nil, Literal::Bool(true), Literal::SmallInt(-5), Literal::Float(1.5), Literal::Str("s".into()), Literal::Sym(Symbol::intern("y")), Literal::Char('z'), Literal::MalformedInt("16r1.8".into())]) },
            Instr::LoadSelf { dst: 1 },
            Instr::LoadBuiltinClass { dst: 2, kind: BuiltinClass::OrderedCollection },
            Instr::Move { dst: 3, src: 2 },
            Instr::StoreLocal { src: 3, slot: 0 },
            Instr::LoadOuterLocal { dst: 4, depth: 2, index: 1 },
            Instr::StoreOuterLocal { src: 4, depth: 2, index: 1 },
            Instr::SendLexical { depth: 1, selector: Symbol::intern("foo:"), arg_regs: vec![1, 2], dst: 5, call_site: cs },
            Instr::SendPublic { selector: Symbol::intern("+"), receiver_reg: 1, arg_regs: vec![2], dst: 6, call_site: cs },
            Instr::SendSuper { selector: Symbol::intern("bar"), arg_regs: vec![], dst: 7, call_site: cs },
            Instr::MakeClosure { dst: 8, params: Rc::new(vec![Symbol::intern("e")]), code: block },
            Instr::MakeArray { dst: 9, elem_regs: vec![0, 1] },
            Instr::InitSlot { src: 9, field_index: 3 },
            Instr::Return { reg: 0 },
            Instr::ImplicitReturn { reg: 1 },
            Instr::NonLocalReturn { reg: 2 },
            Instr::Jump { target: 3 },
            Instr::JumpIfNotBoolean { reg: 1, target: 4 },
            Instr::BranchIfFalse { reg: 1, target: 5 },
            Instr::BranchIfTrue { reg: 1, target: 6 },
        ],
        num_args: 2,
        num_registers: 10,
        statement_pc: vec![(0, 0..3), (1, 3..20)],
        call_sites: vec![cs, cs, cs],
        source: Span::new(10, 20),
    }
}

/// The compiled code of `class`'s method `selector`, looked up the way the
/// engine hook does (by the loaded `Rc<MethodDecl>`'s identity).
fn method_code(class: &ClassRef, module: &CompiledModule, selector: &str) -> Rc<CompiledCode> {
    let borrowed = class.borrow();
    let ObjectData::Class(data) = &borrowed.data else { panic!("not a class") };
    module.methods[&method_key(&data.methods[&Symbol::intern(selector)])].clone()
}

/// Encodes a module's code table into fresh bytes and decodes it back.
fn round_trip_code_table(module: &CompiledModule, class: &ClassRef) -> (decode::DecodedCodeTable, encode::CodeIds) {
    let mut w = encode::Writer::new();
    let ids = encode::encode_code_table(&mut w, module, class);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    (decode::decode_code_table(&mut r).expect("decode code table"), ids)
}

#[test]
fn every_instr_variant_round_trips() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";
    let (_interp, class, bindings) = load(src);
    let mut module = compiler::compile_module(&class, &bindings);
    let synthetic = Rc::new(every_instr_once());
    module.initializers.insert(class_key(&class), synthetic.clone());
    let (table, ids) = round_trip_code_table(&module, &class);
    let decoded = &table.codes[table.initializer.expect("initializer id") as usize];
    assert_eq!(**decoded, *synthetic);
    assert_eq!(ids.id_of(&synthetic), table.initializer.unwrap());
}

#[test]
fn a_real_modules_code_table_round_trips_and_nests_blocks_before_their_parents() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let (table, _ids) = round_trip_code_table(&module, &class);
    assert_eq!(table.methods.len(), 3);
    assert_eq!(
        table.methods.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(),
        vec!["arr", "find:in:", "loop"],
        "root list is sorted by selector"
    );
    for (selector, id) in &table.methods {
        let decoded = &table.codes[*id as usize];
        let original = method_code(&class, &module, selector);
        assert_eq!(**decoded, *original, "{selector}");
        for instr in &decoded.instrs {
            if let Instr::MakeClosure { code, .. } = instr {
                let child_id = table.codes.iter().position(|c| Rc::ptr_eq(c, code)).unwrap();
                assert!(child_id < *id as usize, "a block's code precedes its parent's");
            }
        }
    }
    let init = &table.codes[table.initializer.unwrap() as usize];
    assert_eq!(**init, *module.initializers[&class_key(&class)]);
}

#[test]
fn a_decoded_code_table_installs_against_a_reloaded_class() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let (table, _ids) = round_trip_code_table(&module, &class);
    // "Reload": a second, independent load of the same source.
    let (_interp2, class2, bindings2) = load(RICH);
    let installed = decode::install_code_table(&table, &class2, &bindings2).expect("install");
    let borrowed = class2.borrow();
    let ObjectData::Class(data) = &borrowed.data else { panic!() };
    for (selector, decl) in &data.methods {
        let code = installed.methods.get(&method_key(decl)).unwrap_or_else(|| panic!("{selector} not installed"));
        assert_eq!(**code, *method_code(&class, &module, selector.as_str()));
    }
    assert!(installed.initializers.contains_key(&class_key(&class2)));
    assert!(Rc::ptr_eq(&installed.bindings, &bindings2));
}

#[test]
fn an_unknown_selector_in_the_root_list_is_a_decode_error() {
    let (_interp, class, bindings) = load(RICH);
    let module = compiler::compile_module(&class, &bindings);
    let (table, _ids) = round_trip_code_table(&module, &class);
    let (_interp2, class2, bindings2) =
        load("Newspeak3\n'T'\nclass Other = Object new (\n) (\n) : ()");
    assert!(matches!(
        decode::install_code_table(&table, &class2, &bindings2),
        Err(decode::DecodeError::UnknownSelector(_))
    ));
}

#[test]
fn every_builtin_class_tag_round_trips() {
    let all = [
        BuiltinClass::Object, BuiltinClass::Boolean, BuiltinClass::True, BuiltinClass::False,
        BuiltinClass::Integer, BuiltinClass::Float, BuiltinClass::Str, BuiltinClass::Sym,
        BuiltinClass::Character, BuiltinClass::Array, BuiltinClass::OrderedCollection,
        BuiltinClass::Nil, BuiltinClass::Block, BuiltinClass::Platform,
    ];
    for kind in all {
        assert_eq!(decode::builtin_class_from_tag(encode::builtin_class_tag(kind)), Some(kind));
    }
    assert_eq!(decode::builtin_class_from_tag(200), None);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test image`
Expected: FAIL — `encode_code_table`/`decode_code_table`/`install_code_table`/`builtin_class_tag` not found.

- [x] **Step 3: Write the encoder**

Add to `crates/ns-runtime/src/image/encode.rs`:

```rust
use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::{class_key, method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal};
use crate::types::{BuiltinClass, ClassRef, ObjectData};

/// `Rc<CompiledCode>` pointer -> code-table id, assigned by
/// `encode_code_table`; the object/frame encoders (Task 14) use it for
/// closures' code references.
pub struct CodeIds {
    ids: HashMap<usize, u32>,
}

impl CodeIds {
    pub fn id_of(&self, code: &Rc<CompiledCode>) -> u32 {
        *self
            .ids
            .get(&(Rc::as_ptr(code) as usize))
            .expect("every CompiledCode reachable from the module was assigned an id by encode_code_table")
    }
}

/// Explicit wire tags for `BuiltinClass` -- never `as u8` on the enum.
pub fn builtin_class_tag(kind: BuiltinClass) -> u8 {
    match kind {
        BuiltinClass::Object => 0,
        BuiltinClass::Boolean => 1,
        BuiltinClass::True => 2,
        BuiltinClass::False => 3,
        BuiltinClass::Integer => 4,
        BuiltinClass::Float => 5,
        BuiltinClass::Str => 6,
        BuiltinClass::Sym => 7,
        BuiltinClass::Character => 8,
        BuiltinClass::Array => 9,
        BuiltinClass::OrderedCollection => 10,
        BuiltinClass::Nil => 11,
        BuiltinClass::Block => 12,
        BuiltinClass::Platform => 13,
    }
}

/// The code table: every `CompiledCode` reachable from `module` -- each
/// method, the root class's initializer, and every nested block body --
/// with nested block code *before* the code that creates it, then the root
/// list (`(selector, id)` sorted by selector, and the initializer id).
/// Deterministic: the same module always encodes to the same bytes.
pub fn encode_code_table(w: &mut Writer, module: &CompiledModule, root_class: &ClassRef) -> CodeIds {
    let mut order: Vec<Rc<CompiledCode>> = Vec::new();
    let mut ids = CodeIds { ids: HashMap::new() };
    let mut methods: Vec<(String, Rc<CompiledCode>)> = {
        let borrowed = root_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => data
                .methods
                .iter()
                .map(|(selector, decl)| {
                    let code = module
                        .methods
                        .get(&method_key(decl))
                        .expect("compile_module compiled every loaded method")
                        .clone();
                    (selector.as_str().to_string(), code)
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    methods.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, code) in &methods {
        collect_code(code, &mut order, &mut ids);
    }
    let initializer = module.initializers.get(&class_key(root_class)).cloned();
    if let Some(init) = &initializer {
        collect_code(init, &mut order, &mut ids);
    }
    w.write_u32(order.len() as u32);
    for code in &order {
        encode_code(w, code, &ids);
    }
    w.write_u32(methods.len() as u32);
    for (selector, code) in &methods {
        w.write_str(selector);
        w.write_u32(ids.id_of(code));
    }
    match &initializer {
        Some(init) => {
            w.write_bool(true);
            w.write_u32(ids.id_of(init));
        }
        None => w.write_bool(false),
    }
    ids
}

/// Post-order: a block's code is appended (and gets its id) before the
/// code containing its `MakeClosure`.
fn collect_code(code: &Rc<CompiledCode>, order: &mut Vec<Rc<CompiledCode>>, ids: &mut CodeIds) {
    let key = Rc::as_ptr(code) as usize;
    if ids.ids.contains_key(&key) {
        return;
    }
    for instr in &code.instrs {
        if let Instr::MakeClosure { code: block, .. } = instr {
            collect_code(block, order, ids);
        }
    }
    ids.ids.insert(key, order.len() as u32);
    order.push(code.clone());
}

fn encode_code(w: &mut Writer, code: &CompiledCode, ids: &CodeIds) {
    w.write_u8(match code.kind {
        CodeKind::Method => 0,
        CodeKind::Block => 1,
        CodeKind::Initializer => 2,
    });
    w.write_u16(code.num_args);
    w.write_u16(code.num_registers);
    w.write_u32(code.source.start);
    w.write_u32(code.source.end);
    w.write_u32(code.statement_pc.len() as u32);
    for (stmt, range) in &code.statement_pc {
        w.write_u32(*stmt as u32);
        w.write_u32(range.start as u32);
        w.write_u32(range.end as u32);
    }
    w.write_u32(code.call_sites.len() as u32);
    for site in &code.call_sites {
        encode_call_site(w, site);
    }
    w.write_u32(code.instrs.len() as u32);
    for instr in &code.instrs {
        encode_instr(w, instr, ids);
    }
}

fn encode_call_site(w: &mut Writer, site: &CallSite) {
    w.write_u16(site.call_site_id);
}

fn encode_regs(w: &mut Writer, regs: &[u16]) {
    w.write_u16(regs.len() as u16);
    for r in regs {
        w.write_u16(*r);
    }
}

fn encode_instr(w: &mut Writer, instr: &Instr, ids: &CodeIds) {
    match instr {
        Instr::LoadLiteral { dst, literal } => {
            w.write_u8(0);
            w.write_u16(*dst);
            encode_literal(w, literal);
        }
        Instr::LoadSelf { dst } => {
            w.write_u8(1);
            w.write_u16(*dst);
        }
        Instr::LoadBuiltinClass { dst, kind } => {
            w.write_u8(2);
            w.write_u16(*dst);
            w.write_u8(builtin_class_tag(*kind));
        }
        Instr::Move { dst, src } => {
            w.write_u8(3);
            w.write_u16(*dst);
            w.write_u16(*src);
        }
        Instr::StoreLocal { src, slot } => {
            w.write_u8(4);
            w.write_u16(*src);
            w.write_u16(*slot);
        }
        Instr::LoadOuterLocal { dst, depth, index } => {
            w.write_u8(5);
            w.write_u16(*dst);
            w.write_u16(*depth);
            w.write_u16(*index);
        }
        Instr::StoreOuterLocal { src, depth, index } => {
            w.write_u8(6);
            w.write_u16(*src);
            w.write_u16(*depth);
            w.write_u16(*index);
        }
        Instr::SendLexical { depth, selector, arg_regs, dst, call_site } => {
            w.write_u8(7);
            w.write_u16(*depth);
            w.write_str(selector.as_str());
            encode_regs(w, arg_regs);
            w.write_u16(*dst);
            encode_call_site(w, call_site);
        }
        Instr::SendPublic { selector, receiver_reg, arg_regs, dst, call_site } => {
            w.write_u8(8);
            w.write_str(selector.as_str());
            w.write_u16(*receiver_reg);
            encode_regs(w, arg_regs);
            w.write_u16(*dst);
            encode_call_site(w, call_site);
        }
        Instr::SendSuper { selector, arg_regs, dst, call_site } => {
            w.write_u8(9);
            w.write_str(selector.as_str());
            encode_regs(w, arg_regs);
            w.write_u16(*dst);
            encode_call_site(w, call_site);
        }
        Instr::MakeClosure { dst, params, code } => {
            w.write_u8(10);
            w.write_u16(*dst);
            w.write_u16(params.len() as u16);
            for p in params.iter() {
                w.write_str(p.as_str());
            }
            w.write_u32(ids.id_of(code));
        }
        Instr::MakeArray { dst, elem_regs } => {
            w.write_u8(11);
            w.write_u16(*dst);
            encode_regs(w, elem_regs);
        }
        Instr::InitSlot { src, field_index } => {
            w.write_u8(12);
            w.write_u16(*src);
            w.write_u32(*field_index);
        }
        Instr::Return { reg } => {
            w.write_u8(13);
            w.write_u16(*reg);
        }
        Instr::ImplicitReturn { reg } => {
            w.write_u8(14);
            w.write_u16(*reg);
        }
        Instr::NonLocalReturn { reg } => {
            w.write_u8(15);
            w.write_u16(*reg);
        }
        Instr::Jump { target } => {
            w.write_u8(16);
            w.write_u32(*target);
        }
        Instr::JumpIfNotBoolean { reg, target } => {
            w.write_u8(17);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
        Instr::BranchIfFalse { reg, target } => {
            w.write_u8(18);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
        Instr::BranchIfTrue { reg, target } => {
            w.write_u8(19);
            w.write_u16(*reg);
            w.write_u32(*target);
        }
    }
}

fn encode_literal(w: &mut Writer, literal: &Literal) {
    match literal {
        Literal::Nil => w.write_u8(0),
        Literal::Bool(b) => {
            w.write_u8(1);
            w.write_bool(*b);
        }
        Literal::SmallInt(n) => {
            w.write_u8(2);
            w.write_i64(*n);
        }
        Literal::Float(f) => {
            w.write_u8(3);
            w.write_f64(*f);
        }
        Literal::Str(s) => {
            w.write_u8(4);
            w.write_str(s);
        }
        Literal::Sym(s) => {
            w.write_u8(5);
            w.write_str(s.as_str());
        }
        Literal::Char(c) => {
            w.write_u8(6);
            w.write_u32(*c as u32);
        }
        Literal::Array(elements) => {
            w.write_u8(7);
            w.write_u32(elements.len() as u32);
            for e in elements {
                encode_literal(w, e);
            }
        }
        Literal::MalformedInt(text) => {
            w.write_u8(8);
            w.write_str(text);
        }
    }
}
```

- [x] **Step 4: Write the decoder**

Add to `crates/ns-runtime/src/image/decode.rs`:

```rust
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::{class_key, method_key, CallSite, CodeKind, CompiledCode, CompiledModule, Instr, Literal};
use crate::types::{Binding, BuiltinClass, ClassRef, ObjectData};
use crate::Symbol;

pub fn builtin_class_from_tag(tag: u8) -> Option<BuiltinClass> {
    Some(match tag {
        0 => BuiltinClass::Object,
        1 => BuiltinClass::Boolean,
        2 => BuiltinClass::True,
        3 => BuiltinClass::False,
        4 => BuiltinClass::Integer,
        5 => BuiltinClass::Float,
        6 => BuiltinClass::Str,
        7 => BuiltinClass::Sym,
        8 => BuiltinClass::Character,
        9 => BuiltinClass::Array,
        10 => BuiltinClass::OrderedCollection,
        11 => BuiltinClass::Nil,
        12 => BuiltinClass::Block,
        13 => BuiltinClass::Platform,
        _ => return None,
    })
}

pub fn read_builtin_class(r: &mut Reader) -> Result<BuiltinClass, DecodeError> {
    let tag = r.read_u8()?;
    builtin_class_from_tag(tag).ok_or(DecodeError::InvalidTag { what: "builtin class", tag })
}

/// The decoded code table before it is bound to a reloaded class.
pub struct DecodedCodeTable {
    pub codes: Vec<Rc<CompiledCode>>,
    /// `(selector, code id)` per method of the root class.
    pub methods: Vec<(String, u32)>,
    pub initializer: Option<u32>,
}

pub fn decode_code_table(r: &mut Reader) -> Result<DecodedCodeTable, DecodeError> {
    let count = r.read_u32()? as usize;
    let mut codes: Vec<Rc<CompiledCode>> = Vec::with_capacity(count);
    for _ in 0..count {
        let code = decode_code(r, &codes)?;
        codes.push(Rc::new(code));
    }
    let method_count = r.read_u32()? as usize;
    let mut methods = Vec::with_capacity(method_count);
    for _ in 0..method_count {
        let selector = r.read_str()?;
        let id = r.read_u32()?;
        check_code_id(id, &codes)?;
        methods.push((selector, id));
    }
    let initializer = if r.read_bool()? {
        let id = r.read_u32()?;
        check_code_id(id, &codes)?;
        Some(id)
    } else {
        None
    };
    Ok(DecodedCodeTable { codes, methods, initializer })
}

fn check_code_id(id: u32, codes: &[Rc<CompiledCode>]) -> Result<(), DecodeError> {
    if (id as usize) < codes.len() {
        Ok(())
    } else {
        Err(DecodeError::BadCodeId(id))
    }
}

/// Binds a decoded table to the *reloaded* root class: methods re-keyed by
/// selector to the reloaded `Rc<MethodDecl>`s, the initializer to the
/// reloaded class. The image's bytecode is what runs after a load.
pub fn install_code_table(
    table: &DecodedCodeTable,
    root_class: &ClassRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> Result<CompiledModule, DecodeError> {
    let mut module = CompiledModule {
        methods: HashMap::new(),
        initializers: HashMap::new(),
        bindings: bindings.clone(),
    };
    let borrowed = root_class.borrow();
    let ObjectData::Class(data) = &borrowed.data else {
        return Err(DecodeError::UnknownSelector("<root class is not a class>".to_string()));
    };
    for (selector, id) in &table.methods {
        let decl = data
            .methods
            .get(&Symbol::intern(selector))
            .ok_or_else(|| DecodeError::UnknownSelector(selector.clone()))?;
        module.methods.insert(method_key(decl), table.codes[*id as usize].clone());
    }
    if let Some(id) = table.initializer {
        module.initializers.insert(class_key(root_class), table.codes[id as usize].clone());
    }
    Ok(module)
}

fn decode_code(r: &mut Reader, codes: &[Rc<CompiledCode>]) -> Result<CompiledCode, DecodeError> {
    let kind = match r.read_u8()? {
        0 => CodeKind::Method,
        1 => CodeKind::Block,
        2 => CodeKind::Initializer,
        tag => return Err(DecodeError::InvalidTag { what: "code kind", tag }),
    };
    let num_args = r.read_u16()?;
    let num_registers = r.read_u16()?;
    let source = Span::new(r.read_u32()?, r.read_u32()?);
    let pc_count = r.read_u32()? as usize;
    let mut statement_pc = Vec::with_capacity(pc_count);
    for _ in 0..pc_count {
        let stmt = r.read_u32()? as usize;
        let start = r.read_u32()? as usize;
        let end = r.read_u32()? as usize;
        statement_pc.push((stmt, start..end));
    }
    let site_count = r.read_u32()? as usize;
    let mut call_sites = Vec::with_capacity(site_count);
    for _ in 0..site_count {
        call_sites.push(decode_call_site(r)?);
    }
    let instr_count = r.read_u32()? as usize;
    let mut instrs = Vec::with_capacity(instr_count);
    for _ in 0..instr_count {
        instrs.push(decode_instr(r, codes)?);
    }
    Ok(CompiledCode { kind, instrs, num_args, num_registers, statement_pc, call_sites, source })
}

fn decode_call_site(r: &mut Reader) -> Result<CallSite, DecodeError> {
    Ok(CallSite { call_site_id: r.read_u16()? })
}

fn decode_regs(r: &mut Reader) -> Result<Vec<u16>, DecodeError> {
    let n = r.read_u16()? as usize;
    let mut regs = Vec::with_capacity(n);
    for _ in 0..n {
        regs.push(r.read_u16()?);
    }
    Ok(regs)
}

fn decode_symbol(r: &mut Reader) -> Result<Symbol, DecodeError> {
    Ok(Symbol::intern(&r.read_str()?))
}

fn decode_instr(r: &mut Reader, codes: &[Rc<CompiledCode>]) -> Result<Instr, DecodeError> {
    Ok(match r.read_u8()? {
        0 => {
            let dst = r.read_u16()?;
            Instr::LoadLiteral { dst, literal: decode_literal(r)? }
        }
        1 => Instr::LoadSelf { dst: r.read_u16()? },
        2 => {
            let dst = r.read_u16()?;
            Instr::LoadBuiltinClass { dst, kind: read_builtin_class(r)? }
        }
        3 => {
            let dst = r.read_u16()?;
            Instr::Move { dst, src: r.read_u16()? }
        }
        4 => {
            let src = r.read_u16()?;
            Instr::StoreLocal { src, slot: r.read_u16()? }
        }
        5 => {
            let dst = r.read_u16()?;
            let depth = r.read_u16()?;
            Instr::LoadOuterLocal { dst, depth, index: r.read_u16()? }
        }
        6 => {
            let src = r.read_u16()?;
            let depth = r.read_u16()?;
            Instr::StoreOuterLocal { src, depth, index: r.read_u16()? }
        }
        7 => {
            let depth = r.read_u16()?;
            let selector = decode_symbol(r)?;
            let arg_regs = decode_regs(r)?;
            let dst = r.read_u16()?;
            Instr::SendLexical { depth, selector, arg_regs, dst, call_site: decode_call_site(r)? }
        }
        8 => {
            let selector = decode_symbol(r)?;
            let receiver_reg = r.read_u16()?;
            let arg_regs = decode_regs(r)?;
            let dst = r.read_u16()?;
            Instr::SendPublic { selector, receiver_reg, arg_regs, dst, call_site: decode_call_site(r)? }
        }
        9 => {
            let selector = decode_symbol(r)?;
            let arg_regs = decode_regs(r)?;
            let dst = r.read_u16()?;
            Instr::SendSuper { selector, arg_regs, dst, call_site: decode_call_site(r)? }
        }
        10 => {
            let dst = r.read_u16()?;
            let n = r.read_u16()? as usize;
            let mut params = Vec::with_capacity(n);
            for _ in 0..n {
                params.push(decode_symbol(r)?);
            }
            let id = r.read_u32()?;
            check_code_id(id, codes)?;
            Instr::MakeClosure { dst, params: Rc::new(params), code: codes[id as usize].clone() }
        }
        11 => {
            let dst = r.read_u16()?;
            Instr::MakeArray { dst, elem_regs: decode_regs(r)? }
        }
        12 => {
            let src = r.read_u16()?;
            Instr::InitSlot { src, field_index: r.read_u32()? }
        }
        13 => Instr::Return { reg: r.read_u16()? },
        14 => Instr::ImplicitReturn { reg: r.read_u16()? },
        15 => Instr::NonLocalReturn { reg: r.read_u16()? },
        16 => Instr::Jump { target: r.read_u32()? },
        17 => {
            let reg = r.read_u16()?;
            Instr::JumpIfNotBoolean { reg, target: r.read_u32()? }
        }
        18 => {
            let reg = r.read_u16()?;
            Instr::BranchIfFalse { reg, target: r.read_u32()? }
        }
        19 => {
            let reg = r.read_u16()?;
            Instr::BranchIfTrue { reg, target: r.read_u32()? }
        }
        tag => return Err(DecodeError::InvalidTag { what: "instruction", tag }),
    })
}

fn decode_literal(r: &mut Reader) -> Result<Literal, DecodeError> {
    Ok(match r.read_u8()? {
        0 => Literal::Nil,
        1 => Literal::Bool(r.read_bool()?),
        2 => Literal::SmallInt(r.read_i64()?),
        3 => Literal::Float(r.read_f64()?),
        4 => Literal::Str(r.read_str()?),
        5 => Literal::Sym(decode_symbol(r)?),
        6 => {
            let code = r.read_u32()?;
            Literal::Char(char::from_u32(code).ok_or(DecodeError::InvalidTag { what: "char", tag: 6 })?)
        }
        7 => {
            let n = r.read_u32()? as usize;
            let mut elements = Vec::with_capacity(n);
            for _ in 0..n {
                elements.push(decode_literal(r)?);
            }
            Literal::Array(elements)
        }
        8 => Literal::MalformedInt(r.read_str()?),
        tag => return Err(DecodeError::InvalidTag { what: "literal", tag }),
    })
}
```

- [x] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test image`
Expected: PASS (11 tests).

- [x] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): encode and decode the image's code table

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 14: Image object table and frame table

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs`
- Modify: `crates/ns-runtime/src/image/decode.rs`
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`method_name` becomes `pub(crate)`)
- Modify: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Consumes: `CodeIds` (Task 13), `types::{Object, ObjectData, ClosureData, ClosureBody, Frame, FrameKind, FrameRef, ObjRef, Value}`, `Interpreter::{builtin_class, platform}`.
- Produces: `encode::GraphTable` (`build(roots: &[Value], platform: &Value)`, `object_id`, `frame_id`, `objects()`, `frames()`), `encode::encode_value(w, v, table, platform)`, `encode::encode_class_tag(w, class: &ClassRef)`, `encode::encode_graph_section(w: &mut Writer, roots: &[Value], platform: &Value, code_ids: &CodeIds) -> Result<(), String>`; `decode::DecodeContext<'a> { interp, root_class, codes, bindings, platform }`, `decode::DecodedGraph { roots: Vec<Value>, max_identity_hash: u32 }`, `decode::decode_class_tag(r, ctx)`, `decode::decode_graph_section(r: &mut Reader, ctx: &DecodeContext) -> Result<DecodedGraph, DecodeError>`.

**Design notes (the spec's "Root/special-objects table", "Object table", "Frame table" bullets):**
- **A class object is never structurally serialized** — every class reference (an object's `.class`, a class-valued slot, a frame's `current_class`, an `Initializer` frame's class) is a **class tag**: `Builtin(kind)` or `UserRoot`, resolved on load against the fresh bootstrap / the reloaded root class. `nil`/`true`/`false` are scalar tags (no heap identity in the M1 value model). The `platform` singleton is the one other special object (tag 7), resolved to the loading interpreter's own `Interpreter::platform()`.
- **Objects and frames reference each other freely** (a closure's `captured_frame`/`home`; a frame's `receiver`/`args`/`temps`), so `encode_graph_section` writes *both* counts first and the decoder placeholder-allocates both tables before filling either — the standard two-pass technique. Placeholders are ordinary safe values (an `Instance` with the `Object` class; a `TopLevel` frame), overwritten in place through the `RefCell`; no unsafe.
- **Frame reachability**: every `Frame` reachable from the roots via closures (`captured_frame`, `home`) and transitively via `sender`/`home`/`captured_frame` — the "serialize reified activations" mechanism. In Milestone 2's quiesced snapshots frames are reached only through closures still stored in slots; the tests exercise the full mechanism over a synthetic paused chain (`pc`/`temps` edited by hand before encoding).
- A closure with `ClosureBody::Ast` (created by the tree-walker) is an **error**, never silently dropped; `snapshot` always runs under the bytecode engine so none exist in a world it snapshots. `ClosureData.bindings` is never serialized — every decoded closure receives the reloaded module's map.
- A `Method` frame kind is encoded as its selector and re-resolved against the reloaded class's method map; `instantiate::method_name` (the M1 `MessagePattern` → selector helper) becomes `pub(crate)` for that — never a second copy.
- Identity hashes are preserved verbatim; `decode_graph_section` reports the maximum seen so `load` (Task 15) can advance the allocation counter past it. The bootstrap and `load_module` allocate hashes in the same deterministic order as the original run, so re-bootstrapped classes get the same hashes they had — no collision with loaded objects.

- [x] **Step 1: Write the failing tests**

Add to `crates/ns-runtime/tests/image.rs`:

```rust
use ns_runtime::{ClosureBody, Engine, FrameKind, Signal};

fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

/// Loads `src` under the bytecode engine, lets `make_roots` build the
/// roots, encodes code table + graph section, decodes both with the same
/// `Reader`, and returns (interp, class, original roots, decoded roots,
/// max identity hash).
fn round_trip_graph(
    src: &str,
    make_roots: impl FnOnce(&mut Interpreter, &ClassRef, &Bindings) -> Vec<Value>,
) -> (Interpreter, ClassRef, Vec<Value>, Vec<Value>, u32) {
    let (mut interp, class, bindings) = load(src);
    let module = compiler::compile_module(&class, &bindings);
    interp.set_engine(Engine::Bytecode(module));
    let roots = make_roots(&mut interp, &class, &bindings);
    let platform = interp.platform();
    let mut w = encode::Writer::new();
    let ids = encode::encode_code_table(&mut w, interp.compiled_module().unwrap(), &class);
    encode::encode_graph_section(&mut w, &roots, &platform, &ids).expect("encode graph");
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    let table = decode::decode_code_table(&mut r).expect("code table");
    let decoded = {
        let ctx = decode::DecodeContext {
            interp: &interp,
            root_class: &class,
            codes: &table.codes,
            bindings: &bindings,
            platform: platform.clone(),
        };
        decode::decode_graph_section(&mut r, &ctx).expect("graph")
    };
    (interp, class, roots, decoded.roots, decoded.max_identity_hash)
}

const EMPTY: &str = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";

fn obj(v: &Value) -> &ns_runtime::ObjRef {
    let Value::Obj(o) = v else { panic!("expected an object") };
    o
}

#[test]
fn instances_and_arrays_round_trip_with_shared_identity_preserved() {
    let (_i, _c, original, decoded, _max) = round_trip_graph(EMPTY, |interp, _, _| {
        let shared = Value::Obj(interp.new_string("shared".to_string()));
        vec![Value::Obj(interp.new_array(vec![shared.clone(), shared, Value::SmallInt(7)]))]
    });
    let ObjectData::Array(elements) = &obj(&decoded[0]).borrow().data else { panic!("expected an array") };
    assert!(Rc::ptr_eq(obj(&elements[0]), obj(&elements[1])), "shared identity preserved");
    assert!(!Rc::ptr_eq(obj(&original[0]), obj(&decoded[0])), "the decoded array is a new object");
    assert!(matches!(&obj(&elements[0]).borrow().data, ObjectData::Str(s) if s == "shared"));
    assert!(matches!(elements[2], Value::SmallInt(7)));
}

#[test]
fn identity_hashes_are_preserved_and_the_maximum_is_reported() {
    let (_i, _c, original, decoded, max) = round_trip_graph(EMPTY, |interp, _, _| {
        vec![Value::Obj(interp.new_string("x".to_string()))]
    });
    let hash = obj(&original[0]).borrow().identity_hash;
    assert_eq!(obj(&decoded[0]).borrow().identity_hash, hash);
    assert!(max >= hash);
}

#[test]
fn class_references_resolve_to_the_same_live_class_objects() {
    let (interp, class, _o, decoded, _max) = round_trip_graph(EMPTY, |interp, class, _| {
        vec![Value::Obj(interp.builtin_class(BuiltinClass::Integer)), Value::Obj(class.clone())]
    });
    assert!(Rc::ptr_eq(obj(&decoded[0]), &interp.builtin_class(BuiltinClass::Integer)));
    assert!(Rc::ptr_eq(obj(&decoded[1]), &class));
}

#[test]
fn the_platform_singleton_resolves_to_the_interpreters_own() {
    let (mut interp, _c, _o, decoded, _max) = round_trip_graph(EMPTY, |interp, _, _| vec![interp.platform()]);
    assert!(Rc::ptr_eq(obj(&decoded[0]), obj(&interp.platform())));
}

#[test]
fn an_instance_of_the_root_class_keeps_its_class_and_fields() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n  | a = 1. b = 'two'. |\n) (\n) : ()";
    let (_i, class, _o, decoded, _max) = round_trip_graph(src, |interp, class, bindings| {
        vec![expect_ok(interp.instantiate(class.clone(), vec![], bindings), "instantiate")]
    });
    let instance = obj(&decoded[0]).borrow();
    assert!(Rc::ptr_eq(&instance.class, &class));
    let ObjectData::Instance(fields) = &instance.data else { panic!() };
    assert!(matches!(fields[0], Value::SmallInt(1)));
    assert!(matches!(&obj(&fields[1]).borrow().data, ObjectData::Str(s) if s == "two"));
}

#[test]
fn a_closure_and_its_paused_frame_chain_round_trip() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  mk = ( ^[ 1 ] )\n  public outer = ( ^mk )\n) : ()";
    let (_i, class, original, decoded, _max) = round_trip_graph(src, |interp, class, bindings| {
        let instance = expect_ok(interp.instantiate(class.clone(), vec![], bindings), "instantiate");
        let closure = expect_ok(interp.send(instance, Symbol::intern("outer"), vec![], bindings, None), "outer");
        // Synthetic "paused" state: edit the home frame's pc and grow its
        // register file before snapshotting.
        {
            let ObjectData::Closure(c) = &obj(&closure).borrow().data else { panic!() };
            let mut home = c.home.borrow_mut();
            home.pc = 1;
            home.temps.push(Value::SmallInt(9));
        }
        vec![closure]
    });
    let original_code = {
        let ObjectData::Closure(c) = &obj(&original[0]).borrow().data else { panic!() };
        let ClosureBody::Compiled(code) = &c.body else { panic!() };
        code.clone()
    };
    let ObjectData::Closure(c) = &obj(&decoded[0]).borrow().data else { panic!("expected a closure") };
    let ClosureBody::Compiled(code) = &c.body else { panic!("decoded closures carry compiled code") };
    assert_eq!(**code, *original_code);
    assert!(Rc::ptr_eq(&c.captured_frame, &c.home), "captured_frame and home were the same frame");
    let home = c.home.borrow();
    assert_eq!(home.pc, 1);
    assert!(matches!(home.temps.last(), Some(Value::SmallInt(9))));
    assert!(home.uncontinuable, "`mk` had returned");
    let methods = {
        let borrowed = class.borrow();
        let ObjectData::Class(data) = &borrowed.data else { panic!() };
        data.methods.clone()
    };
    let FrameKind::Method(decl) = &home.kind else { panic!("home is a method frame") };
    assert!(Rc::ptr_eq(decl, &methods[&Symbol::intern("mk")]), "re-resolved against the class's method map");
    assert!(Rc::ptr_eq(&obj(&home.receiver).borrow().class, &class));
    assert!(Rc::ptr_eq(home.current_class.as_ref().unwrap(), &class));
    let sender = home.sender.as_ref().expect("`mk` was sent from `outer`");
    let FrameKind::Method(outer) = &sender.borrow().kind else { panic!() };
    assert!(Rc::ptr_eq(outer, &methods[&Symbol::intern("outer")]));
}

#[test]
fn a_closure_stored_in_a_slot_round_trips_with_an_initializer_home() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n  | blk = [ 2 ]. |\n) (\n) : ()";
    let (_i, class, _o, decoded, _max) = round_trip_graph(src, |interp, class, bindings| {
        vec![expect_ok(interp.instantiate(class.clone(), vec![], bindings), "instantiate")]
    });
    let instance = obj(&decoded[0]).borrow();
    let ObjectData::Instance(fields) = &instance.data else { panic!() };
    let ObjectData::Closure(c) = &obj(&fields[0]).borrow().data else { panic!("slot holds a closure") };
    let FrameKind::Initializer(home_class) = &c.home.borrow().kind else { panic!("home is the initializer frame") };
    assert!(Rc::ptr_eq(home_class, &class));
}

#[test]
fn a_tree_walker_closure_is_a_snapshot_error_not_a_silent_omission() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public mk = ( ^[ 1 ] )\n) : ()";
    let (mut interp, class, bindings) = load(src); // tree-walk engine
    let instance = expect_ok(interp.instantiate(class.clone(), vec![], &bindings), "instantiate");
    let closure = expect_ok(interp.send(instance, Symbol::intern("mk"), vec![], &bindings, None), "mk");
    let platform = interp.platform();
    let module = compiler::compile_module(&class, &bindings);
    let mut w = encode::Writer::new();
    let ids = encode::encode_code_table(&mut w, &module, &class);
    let err = encode::encode_graph_section(&mut w, &[closure], &platform, &ids).expect_err("Ast closure");
    assert!(err.contains("tree-walker"), "{err}");
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test image`
Expected: FAIL — `encode_graph_section`/`DecodeContext`/`decode_graph_section` not found.

- [x] **Step 3: Make `method_name` `pub(crate)`**

In `crates/ns-runtime/src/interpreter/instantiate.rs`, change `fn method_name(pattern: &ast::MessagePattern) -> String` to `pub(crate) fn method_name(...)`. No other change.

- [x] **Step 4: Write the encoder**

Add to `crates/ns-runtime/src/image/encode.rs` (extend the `use crate::types::...` line to also import `ClosureBody, ClosureData, FrameKind, FrameRef, ObjRef, Value`):

```rust
fn is_same_object(a: &Value, b: &Value) -> bool {
    matches!((a, b), (Value::Obj(x), Value::Obj(y)) if Rc::ptr_eq(x, y))
}

/// Every heap object (instances, strings, symbols, arrays, closures) and
/// every `Frame` reachable from the roots, in discovery order. Class
/// objects are never entered (they are class tags), nor is the `platform`
/// singleton (special-object tag).
pub struct GraphTable {
    objects: Vec<ObjRef>,
    object_ids: HashMap<usize, u32>,
    frames: Vec<FrameRef>,
    frame_ids: HashMap<usize, u32>,
}

impl GraphTable {
    pub fn build(roots: &[Value], platform: &Value) -> GraphTable {
        let mut table = GraphTable {
            objects: Vec::new(),
            object_ids: HashMap::new(),
            frames: Vec::new(),
            frame_ids: HashMap::new(),
        };
        for root in roots {
            table.visit_value(root, platform);
        }
        table
    }

    fn visit_value(&mut self, v: &Value, platform: &Value) {
        let Value::Obj(obj) = v else { return };
        if is_same_object(v, platform) || matches!(obj.borrow().data, ObjectData::Class(_)) {
            return;
        }
        let key = Rc::as_ptr(obj) as usize;
        if self.object_ids.contains_key(&key) {
            return;
        }
        self.object_ids.insert(key, self.objects.len() as u32);
        self.objects.push(obj.clone());
        enum Children {
            Values(Vec<Value>),
            Frames(FrameRef, FrameRef),
            None,
        }
        let children = match &obj.borrow().data {
            ObjectData::Instance(fields) => Children::Values(fields.clone()),
            ObjectData::Array(elements) => Children::Values(elements.clone()),
            ObjectData::Closure(c) => Children::Frames(c.captured_frame.clone(), c.home.clone()),
            ObjectData::Str(_) | ObjectData::Sym(_) | ObjectData::Class(_) => Children::None,
        };
        match children {
            Children::Values(values) => {
                for v in &values {
                    self.visit_value(v, platform);
                }
            }
            Children::Frames(captured, home) => {
                self.visit_frame(&captured, platform);
                self.visit_frame(&home, platform);
            }
            Children::None => {}
        }
    }

    fn visit_frame(&mut self, frame: &FrameRef, platform: &Value) {
        let key = Rc::as_ptr(frame) as usize;
        if self.frame_ids.contains_key(&key) {
            return;
        }
        self.frame_ids.insert(key, self.frames.len() as u32);
        self.frames.push(frame.clone());
        let (values, frames) = {
            let f = frame.borrow();
            let mut values = vec![f.receiver.clone()];
            values.extend(f.args.iter().cloned());
            values.extend(f.temps.iter().cloned());
            let mut frames: Vec<FrameRef> = f.sender.iter().chain(f.home.iter()).cloned().collect();
            if let FrameKind::Closure(c) = &f.kind {
                frames.push(c.captured_frame.clone());
                frames.push(c.home.clone());
            }
            (values, frames)
        };
        for v in &values {
            self.visit_value(v, platform);
        }
        for fr in &frames {
            self.visit_frame(fr, platform);
        }
    }

    pub fn object_id(&self, obj: &ObjRef) -> u32 {
        *self.object_ids.get(&(Rc::as_ptr(obj) as usize)).expect("object was visited by build()")
    }

    pub fn frame_id(&self, frame: &FrameRef) -> u32 {
        *self.frame_ids.get(&(Rc::as_ptr(frame) as usize)).expect("frame was visited by build()")
    }

    pub fn objects(&self) -> &[ObjRef] {
        &self.objects
    }

    pub fn frames(&self) -> &[FrameRef] {
        &self.frames
    }
}

/// `Builtin(kind)` (0 + kind tag) or `UserRoot` (1) -- a class is never
/// structurally serialized (spec: "Root/special-objects table").
pub fn encode_class_tag(w: &mut Writer, class: &ClassRef) {
    let builtin = match &class.borrow().data {
        ObjectData::Class(data) => data.builtin,
        _ => panic!("internal: a class reference does not point at a class object"),
    };
    match builtin {
        Some(kind) => {
            w.write_u8(0);
            w.write_u8(builtin_class_tag(kind));
        }
        None => w.write_u8(1),
    }
}

/// The general `Value` encoder: scalar tags 0-4 (`encode_scalar_value`),
/// 5 = object-table id, 6 = class tag, 7 = the `platform` singleton.
pub fn encode_value(w: &mut Writer, v: &Value, table: &GraphTable, platform: &Value) {
    if encode_scalar_value(w, v) {
        return;
    }
    let Value::Obj(obj) = v else { unreachable!("encode_scalar_value handles every non-Obj case") };
    if is_same_object(v, platform) {
        w.write_u8(7);
        return;
    }
    if matches!(obj.borrow().data, ObjectData::Class(_)) {
        w.write_u8(6);
        encode_class_tag(w, obj);
        return;
    }
    w.write_u8(5);
    w.write_u32(table.object_id(obj));
}

fn encode_values(w: &mut Writer, values: &[Value], table: &GraphTable, platform: &Value) {
    w.write_u32(values.len() as u32);
    for v in values {
        encode_value(w, v, table, platform);
    }
}

fn encode_frame_ref(w: &mut Writer, frame: Option<&FrameRef>, table: &GraphTable) {
    match frame {
        Some(f) => {
            w.write_bool(true);
            w.write_u32(table.frame_id(f));
        }
        None => w.write_bool(false),
    }
}

/// `params`, `code_id`, `captured_frame` id, `home` id. `bindings` is not
/// serialized (the reloaded module's map is used on load).
fn encode_closure_record(
    w: &mut Writer,
    closure: &ClosureData,
    table: &GraphTable,
    code_ids: &CodeIds,
) -> Result<(), String> {
    let ClosureBody::Compiled(code) = &closure.body else {
        return Err("image encoding cannot serialize a closure created by the tree-walker (ClosureBody::Ast); \
                    snapshot always runs under the bytecode engine"
            .to_string());
    };
    w.write_u16(closure.params.len() as u16);
    for p in &closure.params {
        w.write_str(p.as_str());
    }
    w.write_u32(code_ids.id_of(code));
    w.write_u32(table.frame_id(&closure.captured_frame));
    w.write_u32(table.frame_id(&closure.home));
    Ok(())
}

fn encode_object(
    w: &mut Writer,
    obj: &ObjRef,
    table: &GraphTable,
    code_ids: &CodeIds,
    platform: &Value,
) -> Result<(), String> {
    let borrowed = obj.borrow();
    encode_class_tag(w, &borrowed.class);
    w.write_u32(borrowed.identity_hash);
    match &borrowed.data {
        ObjectData::Instance(fields) => {
            w.write_u8(0);
            encode_values(w, fields, table, platform);
        }
        ObjectData::Str(s) => {
            w.write_u8(1);
            w.write_str(s);
        }
        ObjectData::Sym(s) => {
            w.write_u8(2);
            w.write_str(s.as_str());
        }
        ObjectData::Array(elements) => {
            w.write_u8(3);
            encode_values(w, elements, table, platform);
        }
        ObjectData::Closure(c) => {
            w.write_u8(4);
            encode_closure_record(w, c, table, code_ids)?;
        }
        ObjectData::Class(_) => unreachable!("class objects are never entered in the object table"),
    }
    Ok(())
}

fn encode_frame(
    w: &mut Writer,
    frame: &FrameRef,
    table: &GraphTable,
    code_ids: &CodeIds,
    platform: &Value,
) -> Result<(), String> {
    let f = frame.borrow();
    encode_value(w, &f.receiver, table, platform);
    encode_values(w, &f.args, table, platform);
    encode_values(w, &f.temps, table, platform);
    w.write_u32(f.pc as u32);
    encode_frame_ref(w, f.sender.as_ref(), table);
    encode_frame_ref(w, f.home.as_ref(), table);
    match &f.current_class {
        Some(c) => {
            w.write_bool(true);
            encode_class_tag(w, c);
        }
        None => w.write_bool(false),
    }
    match &f.kind {
        FrameKind::Method(decl) => {
            w.write_u8(0);
            w.write_str(&crate::interpreter::instantiate::method_name(&decl.pattern));
        }
        FrameKind::Closure(c) => {
            w.write_u8(1);
            encode_closure_record(w, c, table, code_ids)?;
        }
        FrameKind::TopLevel => w.write_u8(2),
        FrameKind::Initializer(class) => {
            w.write_u8(3);
            encode_class_tag(w, class);
        }
    }
    w.write_bool(f.uncontinuable);
    Ok(())
}

/// The object table, the frame table, and the roots. Both counts come
/// first so the decoder can placeholder-allocate both tables before
/// filling either (objects and frames reference each other freely).
pub fn encode_graph_section(
    w: &mut Writer,
    roots: &[Value],
    platform: &Value,
    code_ids: &CodeIds,
) -> Result<(), String> {
    let table = GraphTable::build(roots, platform);
    w.write_u32(table.objects().len() as u32);
    w.write_u32(table.frames().len() as u32);
    for obj in table.objects() {
        encode_object(w, obj, &table, code_ids, platform)?;
    }
    for frame in table.frames() {
        encode_frame(w, frame, &table, code_ids, platform)?;
    }
    encode_values(w, roots, &table, platform);
    Ok(())
}
```

- [x] **Step 5: Write the decoder**

Add to `crates/ns-runtime/src/image/decode.rs` (extend the `use crate::types::...` line to also import `ClosureBody, ClosureData, Frame, FrameKind, FrameRef, Object, ObjRef, Value`; add `use std::cell::RefCell;` and `use crate::Interpreter;`):

```rust
/// Everything the graph decoder resolves tags against: the live bootstrap
/// (`interp`), the reloaded root class, the decoded code table, the
/// reloaded module's bindings, and the interpreter's `platform` singleton.
pub struct DecodeContext<'a> {
    pub interp: &'a Interpreter,
    pub root_class: &'a ClassRef,
    pub codes: &'a [Rc<CompiledCode>],
    pub bindings: &'a Rc<HashMap<Span, Binding>>,
    pub platform: Value,
}

pub struct DecodedGraph {
    pub roots: Vec<Value>,
    /// The largest identity hash of any decoded object -- `load` advances
    /// the interpreter's allocation counter past it.
    pub max_identity_hash: u32,
}

struct Tables<'a> {
    objects: &'a [ObjRef],
    frames: &'a [FrameRef],
}

pub fn decode_class_tag(r: &mut Reader, ctx: &DecodeContext) -> Result<ClassRef, DecodeError> {
    match r.read_u8()? {
        0 => Ok(ctx.interp.builtin_class(read_builtin_class(r)?)),
        1 => Ok(ctx.root_class.clone()),
        tag => Err(DecodeError::InvalidTag { what: "class", tag }),
    }
}

fn decode_value(r: &mut Reader, ctx: &DecodeContext, tables: &Tables) -> Result<Value, DecodeError> {
    match r.read_u8()? {
        0 => Ok(Value::Nil),
        1 => Ok(Value::Bool(false)),
        2 => Ok(Value::Bool(true)),
        3 => Ok(Value::SmallInt(r.read_i64()?)),
        4 => Ok(Value::Float(r.read_f64()?)),
        5 => {
            let id = r.read_u32()?;
            tables.objects.get(id as usize).cloned().map(Value::Obj).ok_or(DecodeError::BadObjectId(id))
        }
        6 => Ok(Value::Obj(decode_class_tag(r, ctx)?)),
        7 => Ok(ctx.platform.clone()),
        tag => Err(DecodeError::InvalidTag { what: "value", tag }),
    }
}

fn decode_values(r: &mut Reader, ctx: &DecodeContext, tables: &Tables) -> Result<Vec<Value>, DecodeError> {
    let n = r.read_u32()? as usize;
    let mut values = Vec::with_capacity(n);
    for _ in 0..n {
        values.push(decode_value(r, ctx, tables)?);
    }
    Ok(values)
}

fn decode_frame_id(r: &mut Reader, tables: &Tables) -> Result<FrameRef, DecodeError> {
    let id = r.read_u32()?;
    tables.frames.get(id as usize).cloned().ok_or(DecodeError::BadFrameId(id))
}

fn decode_frame_ref(r: &mut Reader, tables: &Tables) -> Result<Option<FrameRef>, DecodeError> {
    if r.read_bool()? {
        Ok(Some(decode_frame_id(r, tables)?))
    } else {
        Ok(None)
    }
}

fn decode_closure_record(r: &mut Reader, ctx: &DecodeContext, tables: &Tables) -> Result<ClosureData, DecodeError> {
    let n = r.read_u16()? as usize;
    let mut params = Vec::with_capacity(n);
    for _ in 0..n {
        params.push(decode_symbol(r)?);
    }
    let code_id = r.read_u32()?;
    let code = ctx.codes.get(code_id as usize).cloned().ok_or(DecodeError::BadCodeId(code_id))?;
    let captured_frame = decode_frame_id(r, tables)?;
    let home = decode_frame_id(r, tables)?;
    Ok(ClosureData {
        params,
        body: ClosureBody::Compiled(code),
        captured_frame,
        home,
        bindings: ctx.bindings.clone(),
    })
}

fn decode_frame(r: &mut Reader, ctx: &DecodeContext, tables: &Tables) -> Result<Frame, DecodeError> {
    let receiver = decode_value(r, ctx, tables)?;
    let args = decode_values(r, ctx, tables)?;
    let temps = decode_values(r, ctx, tables)?;
    let pc = r.read_u32()? as usize;
    let sender = decode_frame_ref(r, tables)?;
    let home = decode_frame_ref(r, tables)?;
    let current_class = if r.read_bool()? { Some(decode_class_tag(r, ctx)?) } else { None };
    let kind = match r.read_u8()? {
        0 => {
            let selector = r.read_str()?;
            let decl = {
                let borrowed = ctx.root_class.borrow();
                match &borrowed.data {
                    ObjectData::Class(data) => data.methods.get(&Symbol::intern(&selector)).cloned(),
                    _ => None,
                }
            };
            FrameKind::Method(decl.ok_or(DecodeError::UnknownSelector(selector))?)
        }
        1 => FrameKind::Closure(Rc::new(decode_closure_record(r, ctx, tables)?)),
        2 => FrameKind::TopLevel,
        3 => FrameKind::Initializer(decode_class_tag(r, ctx)?),
        tag => return Err(DecodeError::InvalidTag { what: "frame kind", tag }),
    };
    let uncontinuable = r.read_bool()?;
    Ok(Frame { receiver, args, temps, pc, sender, home, current_class, kind, uncontinuable })
}

fn placeholder_frame() -> Frame {
    Frame {
        receiver: Value::Nil,
        args: Vec::new(),
        temps: Vec::new(),
        pc: 0,
        sender: None,
        home: None,
        current_class: None,
        kind: FrameKind::TopLevel,
        uncontinuable: false,
    }
}

/// Two-pass decoding: placeholder-allocate every object and every frame,
/// then fill each in place (so cycles and shared references round-trip
/// with identity preserved), then decode the roots.
pub fn decode_graph_section(r: &mut Reader, ctx: &DecodeContext) -> Result<DecodedGraph, DecodeError> {
    let object_count = r.read_u32()? as usize;
    let frame_count = r.read_u32()? as usize;
    let placeholder_class = ctx.interp.builtin_class(BuiltinClass::Object);
    let objects: Vec<ObjRef> = (0..object_count)
        .map(|_| {
            Rc::new(RefCell::new(Object {
                class: placeholder_class.clone(),
                identity_hash: 0,
                data: ObjectData::Instance(Vec::new()),
            }))
        })
        .collect();
    let frames: Vec<FrameRef> = (0..frame_count).map(|_| Rc::new(RefCell::new(placeholder_frame()))).collect();
    let tables = Tables { objects: &objects, frames: &frames };
    let mut max_identity_hash = 0;
    for obj in &objects {
        let class = decode_class_tag(r, ctx)?;
        let identity_hash = r.read_u32()?;
        max_identity_hash = max_identity_hash.max(identity_hash);
        let data = match r.read_u8()? {
            0 => ObjectData::Instance(decode_values(r, ctx, &tables)?),
            1 => ObjectData::Str(r.read_str()?),
            2 => ObjectData::Sym(decode_symbol(r)?),
            3 => ObjectData::Array(decode_values(r, ctx, &tables)?),
            4 => ObjectData::Closure(decode_closure_record(r, ctx, &tables)?),
            tag => return Err(DecodeError::InvalidTag { what: "object body", tag }),
        };
        *obj.borrow_mut() = Object { class, identity_hash, data };
    }
    for frame in &frames {
        let decoded = decode_frame(r, ctx, &tables)?;
        *frame.borrow_mut() = decoded;
    }
    let roots = decode_values(r, ctx, &tables)?;
    Ok(DecodedGraph { roots, max_identity_hash })
}
```

- [x] **Step 6: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test image`
Expected: PASS (19 tests).

- [x] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): encode and decode the image's object and frame tables

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 15: `image::snapshot`/`load` and `ns-next snapshot`/`load`

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs` (add `encode_source_origin`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (add `decode_source_origin`)
- Modify: `crates/ns-runtime/src/image/mod.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (add `advance_identity_hash_past`)
- Modify: `crates/ns-runtime/tests/image.rs`
- Modify: `crates/ns-cli/src/main.rs`
- Create: `crates/ns-cli/tests/snapshot_load.rs`

**Interfaces:**
- Produces: `pub fn image::snapshot(source: &str, origin: &SourceOrigin, root_class: &ClassRef, module: &CompiledModule, platform: &Value, roots: &[Value]) -> Result<Vec<u8>, String>`, `pub struct image::LoadedImage { pub interp: Interpreter, pub root_class: ClassRef, pub bindings: Rc<HashMap<Span, Binding>>, pub roots: Vec<Value> }`, `pub fn image::load(bytes: &[u8]) -> Result<LoadedImage, String>`, `pub(crate) fn Interpreter::advance_identity_hash_past(&mut self, max: u32)`; `ns-next snapshot <file.ns> <out.img> [--send selector]`, `ns-next load <image> [--send selector]`.

**Design notes:**
- **Overall layout** (spec): header, source text, `SourceOrigin`, code table, object table, frame table, roots. One `Reader` cursor is threaded through every section on load.
- **`load` re-runs parse → resolve → bootstrap → `load_module` on the stored source before decoding anything** — that *is* "bootstrap from source": the `UserRoot` class tag and every `Method` frame kind only mean something once a fresh, live `ClassRef` exists. Then the code table is installed as the `CompiledModule` (the image's bytecode is what runs after a load), the graph decoded against it, the identity-hash counter advanced past the image's maximum, and `Engine::Bytecode` installed. `run` never snapshots (spec Req. 5).
- **`snapshot` always runs under the bytecode engine**; roots are the root instance and, with `--send`, the entry send's result (a quiesced world — Non-Goals). **`load --send`** sends with the same `[platform, nil]` convention as `run` and prints exactly what `run` prints (nothing beyond the program's own output) — so a `load --send main:args:` of a snapshot is directly comparable to a `run`. Without `--send`, `load` prints one summary line.
- `LoadedSource.source`/`origin` (Task 8) are now used; remove their `#[allow(dead_code)]`.

- [x] **Step 1: Write the failing tests**

Add to `crates/ns-runtime/tests/image.rs`:

```rust
use ns_runtime::image;
use ns_runtime::SourceOrigin as Origin;

#[test]
fn a_snapshot_loads_into_a_bytecode_interpreter_with_its_state_and_a_safe_hash_counter() {
    let src = "Newspeak3\n'T'\nclass Counter = Object new (\n  | n ::= 0. |\n) (\n  public bump = ( n := n + 1. ^n )\n) : ()";
    let (mut interp, class, bindings) = load(src);
    let module = compiler::compile_module(&class, &bindings);
    interp.set_engine(Engine::Bytecode(module));
    let instance = expect_ok(interp.instantiate(class.clone(), vec![], &bindings), "instantiate");
    expect_ok(interp.send(instance.clone(), Symbol::intern("bump"), vec![], &bindings, None), "bump");
    let platform = interp.platform();
    let bytes = image::snapshot(
        src,
        &Origin::File(PathBuf::from("t.ns")),
        &class,
        interp.compiled_module().unwrap(),
        &platform,
        &[instance.clone()],
    )
    .expect("snapshot");
    let max_hash = obj(&instance).borrow().identity_hash;

    let mut loaded = image::load(&bytes).expect("load");
    assert!(loaded.interp.compiled_module().is_some(), "the bytecode engine is installed");
    assert!(Rc::ptr_eq(&obj(&loaded.roots[0]).borrow().class, &loaded.root_class));
    let bindings = loaded.bindings.clone();
    let n = expect_ok(loaded.interp.send(loaded.roots[0].clone(), Symbol::intern("bump"), vec![], &bindings, None), "bump");
    assert!(matches!(n, Value::SmallInt(2)), "slot state survived: 1 before the snapshot, 2 after");
    let fresh = loaded.interp.new_string("fresh".to_string());
    assert!(fresh.borrow().identity_hash > max_hash, "no hash collision with loaded objects");
}

#[test]
fn load_refuses_a_version_mismatch() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";
    let (mut interp, class, bindings) = load(src);
    let module = compiler::compile_module(&class, &bindings);
    interp.set_engine(Engine::Bytecode(module));
    let platform = interp.platform();
    let mut bytes = image::snapshot(src, &Origin::File(PathBuf::from("t.ns")), &class, interp.compiled_module().unwrap(), &platform, &[]).unwrap();
    bytes[4..8].copy_from_slice(&9999u32.to_le_bytes());
    let err = image::load(&bytes).expect_err("version mismatch");
    assert!(err.contains("version"), "{err}");
}
```

`crates/ns-cli/tests/snapshot_load.rs`:

```rust
//! `ns-next snapshot` then `ns-next load`, end to end through the binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures").join(name)
}

fn temp_image(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ns-next-image-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("world.img")
}

fn ns_next(args: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next")).args(args).output().expect("run ns-next");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
    )
}

#[test]
fn a_snapshot_taken_after_the_entry_send_loads_and_reruns_it_identically() {
    let img = temp_image("arithmetic");
    let src = fixture("arithmetic.ns");
    let (ok, out, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str(), "--send".as_ref(), "main:args:".as_ref()]);
    assert!(ok, "{err}");
    assert!(out.starts_with("42\n"), "the entry send ran while snapshotting: {out}");
    assert!(img.exists());
    let (ok, out, err) = ns_next(&["load".as_ref(), img.as_os_str(), "--send".as_ref(), "main:args:".as_ref()]);
    assert!(ok, "{err}");
    assert_eq!(out, "42\n", "the loaded instance's slot still holds 6 * 7; nothing else is printed");
    let _ = std::fs::remove_dir_all(img.parent().unwrap());
}

#[test]
fn a_loaded_world_does_not_rerun_its_instance_initializer() {
    let img = temp_image("instance-init");
    let src = fixture("instance_init.ns");
    let (ok, out, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
    assert!(ok, "{err}");
    assert!(out.starts_with("init ran\n"), "{out}");
    let (ok, out, err) = ns_next(&["load".as_ref(), img.as_os_str(), "--send".as_ref(), "main:args:".as_ref()]);
    assert!(ok, "{err}");
    assert_eq!(out, "hi 2\n", "slots (including the transcript stream object) survived; the initializer did not run again");
    let _ = std::fs::remove_dir_all(img.parent().unwrap());
}

#[test]
fn load_without_send_prints_a_summary_and_a_corrupt_image_is_reported() {
    let img = temp_image("summary");
    let src = fixture("hello.ns");
    let (ok, _out, err) = ns_next(&["snapshot".as_ref(), src.as_os_str(), img.as_os_str()]);
    assert!(ok, "{err}");
    let (ok, out, _err) = ns_next(&["load".as_ref(), img.as_os_str()]);
    assert!(ok);
    assert!(out.contains("1 root value(s)"), "{out}");
    let mut bytes = std::fs::read(&img).unwrap();
    bytes[4..8].copy_from_slice(&9999u32.to_le_bytes());
    std::fs::write(&img, bytes).unwrap();
    let (ok, _out, err) = ns_next(&["load".as_ref(), img.as_os_str()]);
    assert!(!ok);
    assert!(err.contains("version"), "{err}");
    let _ = std::fs::remove_dir_all(img.parent().unwrap());
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test image` and `cargo test -p ns-cli --test snapshot_load`
Expected: FAIL — `image::snapshot`/`image::load` not found; `snapshot`/`load` are not recognized subcommands.

- [x] **Step 3: `SourceOrigin` encoding and the hash-counter advance**

Add to `crates/ns-runtime/src/image/encode.rs` (extend the types import with `SourceOrigin`):

```rust
pub fn encode_source_origin(w: &mut Writer, origin: &SourceOrigin) {
    match origin {
        SourceOrigin::File(path) => {
            w.write_u8(0);
            w.write_str(&path.to_string_lossy());
        }
    }
}
```

Add to `crates/ns-runtime/src/image/decode.rs` (extend the types import with `SourceOrigin`; add `use std::path::PathBuf;`):

```rust
pub fn decode_source_origin(r: &mut Reader) -> Result<SourceOrigin, DecodeError> {
    match r.read_u8()? {
        0 => Ok(SourceOrigin::File(PathBuf::from(r.read_str()?))),
        tag => Err(DecodeError::InvalidTag { what: "source origin", tag }),
    }
}
```

Add to `impl Interpreter` in `crates/ns-runtime/src/interpreter/mod.rs` (next to `alloc_identity_hash`):

```rust
    /// After `image::load`: never hand out an identity hash a loaded object
    /// already carries (spec: "Identity hashes").
    pub(crate) fn advance_identity_hash_past(&mut self, max: u32) {
        if self.next_identity_hash <= max {
            self.next_identity_hash = max + 1;
        }
    }
```

- [x] **Step 4: `snapshot` and `load`**

Replace `crates/ns-runtime/src/image/mod.rs` with:

```rust
//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md`'s
//! "Image format" section. Layout: header, source text, `SourceOrigin`,
//! code table, object table, frame table, roots.

pub mod decode;
pub mod encode;

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::bytecode::CompiledModule;
use crate::interpreter::Engine;
use crate::types::{Binding, ClassRef, SourceOrigin, Value};
use crate::Interpreter;

/// Serializes a quiesced world: `roots` (the root instance and, if any,
/// the entry send's result) plus everything reachable from them, together
/// with the module's source, provenance, and compiled code.
pub fn snapshot(
    source: &str,
    origin: &SourceOrigin,
    root_class: &ClassRef,
    module: &CompiledModule,
    platform: &Value,
    roots: &[Value],
) -> Result<Vec<u8>, String> {
    let mut w = encode::Writer::new();
    encode::write_header(&mut w);
    w.write_str(source);
    encode::encode_source_origin(&mut w, origin);
    let ids = encode::encode_code_table(&mut w, module, root_class);
    encode::encode_graph_section(&mut w, roots, platform, &ids)?;
    Ok(w.into_bytes())
}

pub struct LoadedImage {
    /// Bootstrapped, with the module loaded and `Engine::Bytecode` (the
    /// image's own code table) installed.
    pub interp: Interpreter,
    pub root_class: ClassRef,
    pub bindings: Rc<HashMap<Span, Binding>>,
    pub roots: Vec<Value>,
}

/// Bootstrap from source, then decode: re-runs parse -> resolve ->
/// bootstrap -> `load_module` on the stored source (the class tags and
/// method frame kinds resolve against *that*), installs the decoded code
/// table as the engine, decodes the object graph, and advances the
/// identity-hash counter past the image's maximum.
pub fn load(bytes: &[u8]) -> Result<LoadedImage, String> {
    let mut r = decode::Reader::new(bytes);
    decode::read_header(&mut r).map_err(|e| e.to_string())?;
    let source = r.read_str().map_err(|e| e.to_string())?;
    let origin = decode::decode_source_origin(&mut r).map_err(|e| e.to_string())?;
    let module_ast = ns_syntax::parse(&source).map_err(|diags| {
        format!(
            "re-parsing the image's source failed: {}",
            diags.first().map(|d| d.message.clone()).unwrap_or_default()
        )
    })?;
    let resolved = crate::resolve(module_ast, Rc::from(source.as_str()), origin).map_err(|errors| {
        format!(
            "re-resolving the image's source failed: {}",
            errors.first().map(|e| e.message.clone()).unwrap_or_default()
        )
    })?;
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let root_class = interp.load_module(&resolved);
    let table = decode::decode_code_table(&mut r).map_err(|e| e.to_string())?;
    let module = decode::install_code_table(&table, &root_class, &bindings).map_err(|e| e.to_string())?;
    let platform = interp.platform();
    let graph = {
        let ctx = decode::DecodeContext {
            interp: &interp,
            root_class: &root_class,
            codes: &table.codes,
            bindings: &bindings,
            platform,
        };
        decode::decode_graph_section(&mut r, &ctx).map_err(|e| e.to_string())?
    };
    interp.advance_identity_hash_past(graph.max_identity_hash);
    interp.set_engine(Engine::Bytecode(module));
    Ok(LoadedImage { interp, root_class, bindings, roots: graph.roots })
}
```

- [x] **Step 5: The CLI commands**

In `crates/ns-cli/src/main.rs`, add two `Cmd` variants:

```rust
    /// Run a file under the bytecode engine, then serialize the resulting
    /// (quiesced) world to an image file.
    Snapshot {
        file: PathBuf,
        out: PathBuf,
        /// A selector to send (with `[platform, nil]`, as `run` does)
        /// before snapshotting; its result becomes a second root.
        #[arg(long)]
        send: Option<String>,
    },
    /// Deserialize an image and optionally send one further message to
    /// its root instance (`[platform, nil]`, as `run` does).
    Load {
        image: PathBuf,
        #[arg(long)]
        send: Option<String>,
    },
```

Add to `main`'s dispatch:

```rust
        Cmd::Snapshot { file, out, send } => cmd_snapshot(&file, &out, send.as_deref()),
        Cmd::Load { image, send } => cmd_load(&image, send.as_deref()),
```

Remove the `#[allow(dead_code)]` attributes from `LoadedSource.source`/`origin`, then add the commands:

```rust
fn cmd_snapshot(file: &Path, out: &Path, send: Option<&str>) -> ExitCode {
    let loaded = match load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let bindings = loaded.resolved.bindings.clone();
    let (mut interp, class) = bootstrap_with_engine(&loaded, EngineArg::Bytecode);
    let platform = interp.platform();
    let instance = match interp.instantiate(class.clone(), vec![platform.clone()], &bindings) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    let mut roots = vec![instance.clone()];
    if let Some(selector) = send {
        let args = vec![platform.clone(), ns_runtime::Value::Nil];
        match interp.send(instance, ns_runtime::Symbol::intern(selector), args, &bindings, None) {
            Ok(result) => roots.push(result),
            Err(signal) => return report_entry_send(Err(signal)),
        }
    }
    let module = interp.compiled_module().expect("bootstrap_with_engine installed the bytecode engine");
    let bytes = match ns_runtime::image::snapshot(&loaded.source, &loaded.origin, &class, module, &platform, &roots) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: snapshot failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: writing {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}

fn cmd_load(image: &Path, send: Option<&str>) -> ExitCode {
    let bytes = match std::fs::read(image) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: reading {}: {e}", image.display());
            return ExitCode::FAILURE;
        }
    };
    let ns_runtime::image::LoadedImage { mut interp, root_class: _, bindings, roots } =
        match ns_runtime::image::load(&bytes) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: loading {}: {e}", image.display());
                return ExitCode::FAILURE;
            }
        };
    let Some(selector) = send else {
        println!("loaded {}: {} root value(s)", image.display(), roots.len());
        return ExitCode::SUCCESS;
    };
    let Some(instance) = roots.into_iter().next() else {
        eprintln!("error: image has no root instance to send to");
        return ExitCode::FAILURE;
    };
    let platform = interp.platform();
    let args = vec![platform, ns_runtime::Value::Nil];
    report_entry_send(interp.send(instance, ns_runtime::Symbol::intern(selector), args, &bindings, None))
}
```

- [x] **Step 6: Run tests to verify they pass**

Run: `cargo test --workspace`
Expected: PASS — the two new `image` tests, the three `snapshot_load` tests, and everything before them.

- [x] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat: add image snapshot/load and the ns-next snapshot and load commands

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 16: Final verification and the progress tracker

**Files:**
- Modify: `docs/PROGRESS.md`

- [x] **Step 1: Full workspace test suite**

Run: `cargo test --workspace`
Expected: every test from every prior task passes — including the differential harness over all six `tests-fixtures/*.ns` files (`hello`, `arithmetic`, `blocks_and_return`, `instance_init`, `while_loop`, `control_flow_fallback`) plus its three inline sources, the `disasm` snapshots, and `snapshot_load`.

- [x] **Step 2: Formatting and lints**

Run: `cargo fmt --all -- --check` — Expected: clean.
Run: `cargo clippy --workspace --all-targets` — Expected: zero warnings.

- [x] **Step 3: No Milestone 1 frontend regression**

Run: `cargo run -p ns-cli -- parse-corpus corpus/modules`
Expected: `parsed 210, failed 0, expected-fail 1, stale 0` — identical to Milestone 1's own final count. This milestone touches nothing under `crates/ns-syntax` (`git diff --stat 1e3dfe7 -- crates/ns-syntax` must be empty).

- [x] **Step 4: Manual sanity checks of the default engine and the tooling**

Run: `cargo run -p ns-cli -- run tests-fixtures/blocks_and_return.ns` (no `--engine` flag — confirms the *default* is `bytecode`).
Expected: identical stdout to `tests-fixtures/blocks_and_return.expected`.

Run: `cargo run -p ns-cli -- disasm tests-fixtures/while_loop.ns main:args:`
Expected: a readable listing with two backward `Jump -> N` and no `#whileTrue:`/`#whileFalse:` send.

Run: `cargo run -p ns-cli -- snapshot tests-fixtures/instance_init.ns /tmp/ii.img` then `cargo run -p ns-cli -- load /tmp/ii.img --send main:args:` (any writable path in place of `/tmp`).
Expected: the first prints `init ran` then `wrote ...`; the second prints exactly `hi 2`.

- [x] **Step 5: Spec re-read**

Re-read the M2 spec's "Testing" section and confirm every bullet has a test: differential (Task 8/10, `differential.rs`), bytecode snapshot tests (Task 11), control-flow fallback tests (Task 10 — fixture + inline DNU + `mustBeBoolean`), image round-trip tests (Tasks 13-15 — object graphs, shared references, the whole code table with `decode(encode(code)) == code`, closures in a synthetic paused frame chain, identity hashes, class-tag resolution to the same live `ClassRef`), `disasm` tests (Task 11).

- [x] **Step 6: Update the progress tracker and commit**

In `docs/PROGRESS.md`: change the M2 row's status to `Complete` with today's date; retitle the `## M2 — Bytecode + VM (plan written, implementation pending)` heading to `## M2 — Bytecode + VM (complete, <date>)`; replace the paragraph's last two sentences ("Design spec committed at `92f511b`; ... is the next step.") with one sentence stating the milestone is implemented and tested (`cargo test --workspace` green, corpus count unchanged), naming the commit range. Then, if Steps 1-5 required any fixes, commit them with an appropriately scoped message first (`fix(runtime): ...`), and commit the tracker:

```bash
git add docs/PROGRESS.md
git commit -m "docs: mark Milestone 2 (bytecode compiler and VM) complete

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Self-Review Notes (from writing this plan)

**Spec coverage**, checked section by section against `docs/superpowers/specs/2026-09-04-2243-ns-next-bytecode-vm-design.md` (revised 2026-09-05):

- Goals: every method body, block body, and instance initializer compiled — Tasks 5 (`compile_module`), 6 (hooks 1-2), 7 (hook 3); "no Newspeak-level code is tree-walked under `--engine=bytecode`" enforced by `ClosureBody` (Task 6) and proven by `vm_initializer.rs`'s closure-body assertion (Task 7). Control-flow inlining with guard + fallback — Task 10. Statement→PC table / safepoints — `CompiledCode.statement_pc` (Task 1, filled by Tasks 2-5; inlined statements inside the enclosing range, Task 10). Call-site identity / inert IC slot — Task 1, threaded from Task 3. Image format save/load — Tasks 12-15. Tree-walker retained and differential-tested — Tasks 8, 10. `disasm` — Task 11. The three natives — Task 9.
- `bytecode.rs` section: the full opcode list including `MalformedInt` — Task 1; register model and per-statement allocator — Task 2; inlined statements as sub-expressions — Task 10.
- Control-flow inlining section: gate (params + temps) — Task 10 `recognize`; lexical addressing through the scope stack — Task 2 (`hops_for_depth`, `local_target`) exercised by Task 10; `^` per code kind — Task 4 `return_instr`; `mustBeBoolean` — Tasks 9-10; statement→PC granularity — Task 10.
- `vm.rs` and the engine hook: `Engine`, three hooks, `vm::run_code` with no dispatch logic, the six `pub(crate)` visibility changes, `FrameKind::Initializer`, `TopLevel` retained — Tasks 6-7.
- Blocks and closures / Non-local return — Tasks 4, 6 (`NonLocalReturn` → `raise_return_pub`; `cannotReturn` test).
- Class instantiation: `compile_initializer` (Task 5), hook 3 (Task 7), every M1 gap carried forward (Task 7's escaping-`^` test).
- Compilation cannot raise a `NewspeakError`: `MalformedInt` deferral — Tasks 2, 6.
- Image format: header/version refusal (Task 12); canonical encodings (Task 12); root/special-objects as class tags + `platform` tag (Task 14); identity hashes + counter advance (Tasks 14-15); source + provenance (Task 15); code table with blocks-before-parents and re-keying on load (Task 13); object table incl. closures and frame table with two-pass decoding (Task 14); bootstrap from source (Task 15).
- `ns-cli` additions: `run --engine` before instantiation (Task 8), `snapshot`/`load` with `[platform, nil]` (Task 15), `disasm` with `--initializer` and nested blocks (Task 11).
- Testing section: see Task 16 Step 5.
- Non-Goals respected: no IC state written, no JIT, no GC, no `--bench`, no lexer change, no migration.

**Corrections made while writing this plan** (against the previous draft): the `Vm` object and its ~200 duplicated lines of dispatch orchestration are gone (Task 6); block bodies and instance initializers compile (Tasks 4-7); `NonLocalReturn` exists (Tasks 1, 4, 6); inlined block lowering no longer resets the allocator to the statement base, no longer records inlined statements in `statement_pc`, and rewrites `OuterLocal` depths (Task 10, tested); blocks with temporaries are not inlined; the `whileTrue:` guard failure no longer re-runs the condition (`mustBeBoolean`, Tasks 9-10, with a differential test that the condition body runs exactly once); malformed integer literals no longer panic the compiler (Task 2); the fallback fixture exits `0` so `run_fixtures.rs` keeps passing (Task 10); no test calls `.expect()` on an `EvalResult` or `{:?}` on a `Value`; `load --send` prints nothing but the program's output (no `{result:?}`, which would recurse through the self-referential `Object` class); the `platform` singleton has an identity-preserving tag (Task 14); a class object's `.class` placeholder in decoding is a safe live class, not an `unsafe` self-referential allocation (Task 14).

**Type consistency**, checked across tasks: `Lowerer::new(kind, num_args, num_named_temps, bindings, enclosing_scopes)` (Task 2) is called with exactly that shape in Tasks 4 (`lower_block_literal`), 5 (`compile_method`/`compile_initializer`), and the test helpers; `Lowerer::finish(self, source: Span) -> CompiledCode` everywhere (no tuple form remains); `lower_stmt_inner` (Task 4) is what Task 10's `lower_inline_block_body` calls; `emit_send`/`emit_ordinary_send_with_receiver` (Task 3) are what Task 10's fallbacks and `emit_must_be_boolean` use; `Instr` field names (`dst`, `src`, `slot`, `depth`, `index`, `receiver_reg`, `arg_regs`, `call_site`, `target`, `field_index`, `code`, `params`) are identical in Tasks 1, 6 (`run_code`), 11 (`format_instr`), 13 (`encode_instr`/`decode_instr`); jump targets and `field_index` are `u32` in every task; `CompiledModule { methods, initializers, bindings }` and `method_key`/`class_key` are used identically by Tasks 5, 6, 7, 13; `Engine::Bytecode(CompiledModule)`, `set_engine`, `compiled_module()` (Task 6) are used identically by Tasks 7, 8, 14, 15; `vm::run_code(interp, frame, code, bindings)` has one signature in Tasks 6 and 7; `encode::CodeIds::id_of` (Task 13) is what Task 14's closure records use; `DecodeContext`'s five fields are constructed identically in Task 14's tests and Task 15's `load`; the `expect_ok` helper has the same body in every test file that defines it.
