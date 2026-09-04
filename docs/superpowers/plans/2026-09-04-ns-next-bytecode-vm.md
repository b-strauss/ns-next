# ns-next Bytecode Compiler and VM (Milestone 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a register-based bytecode compiler and VM to `ns-runtime`, selectable via `ns-next run --engine=bytecode` (the new default), with baseline-compiler control-flow inlining, a statement-boundary safepoint/PC table, reserved inline-cache slots, and a versioned image save/load format (`ns-next snapshot`/`ns-next load`) — while keeping the Milestone 1 tree-walker as a permanent `--engine=tree-walk` reference path.

**Architecture:** New modules inside `ns-runtime` (`bytecode.rs`, `compiler/`, `vm.rs`, `image/`), consuming the unchanged Milestone 1 `ResolvedModule`, `dispatch.rs`, and object model. `vm.rs` reimplements the *shape* of `interpreter/mod.rs`'s and `interpreter/eval.rs`'s dispatch orchestration (send / `doesNotUnderstand:` / lexical-send / super-send) rather than sharing it, because `interpreter/` is not modified in this milestone (see Global Constraints) — the only real difference from the tree-walker's orchestration is that a `Lookup::Method` match runs compiled bytecode instead of walking the AST. Class instantiation (slot initializers, `initExprs`, the superclass-chain walk) continues to run through the existing, unchanged `Interpreter::instantiate`/`eval::eval_expr` machinery for both engines — this milestone compiles and bytecode-executes *method bodies only*, not instance-initializer code (see Global Constraints for why this is a deliberate, observable-behavior-preserving scope cut, not a gap).

**Tech Stack:** Rust 1.98 (edition 2021), no new external dependencies. Image encoding is hand-rolled (`Vec<u8>` + explicit little-endian byte writes/reads), not `serde`/`bincode` — consistent with the project's "no new dependency unless needed" stance and the spec's own canonical-encoding requirement.

**Spec:** `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md` — read it in full before starting; this plan implements it section by section. It in turn depends on `docs/superpowers/specs/2026-09-01-ns-next-runtime-design.md` (the Milestone 1 object model, resolver, and dispatch this milestone reuses unchanged) and `docs/superpowers/specs/2026-09-04-ns-next-liveness-images-vm-design.md` (the cross-milestone constraints this milestone's safepoints/image format satisfy).

**Spec clarification found while writing this plan (not a contradiction, an omission the spec left unstated):** the design spec's Data Flow diagram shows `ResolvedModule -> compile_module -> ... -> Vm::run`, without saying what happens to instance-initializer code (slot inits, `initExprs`, the six-step Class Instantiation algorithm from the Milestone 1 spec). Reading the actual `ns-runtime` source (`interpreter/instantiate.rs`) shows that machinery is ~350 lines of already-hardened, subtle logic (superclass-chain recursion, slot-index bookkeeping, factory-arity checking) that has nothing to do with *method dispatch* — it runs once per instantiation, not in a hot path. This plan does not compile or bytecode-execute it: `Vm`-engine runs reuse `Interpreter::instantiate`/`try_factory_send` unchanged (both are already usable from outside `interpreter/` — see Task 6's design note on exact visibility), and only method bodies reached by an ordinary/lexical/super *send after instantiation* run as compiled bytecode. This is invisible to a Newspeak program (identical result either way) and keeps this milestone's scope to what the spec's Goals section actually asks for ("compile every method and block body reachable from a `ResolvedModule`" — instance-initializer code is not a method or block body).

## Global Constraints

- **Rust toolchain:** 1.98.0, pinned via `rust-toolchain.toml`. `cargo` lives at `C:\Users\Benjamin\.cargo\bin\cargo.exe` and is **not on PATH** in every shell — add `%USERPROFILE%\.cargo\bin` to PATH for the session or invoke cargo by full path.
- **Edition:** 2021.
- **All code, identifiers, comments, and doc comments in English.**
- **Lints:** `cargo clippy --workspace --all-targets` must be warning-free before every commit.
- **Formatting:** `cargo fmt --all` before every commit.
- **Terminology:** classic terms — `blacklist`, `whitelist`, `sane`.
- **No new external dependencies.** Image encoding is hand-rolled; every other task uses only what the workspace already depends on.
- **`interpreter/` (the Milestone 1 tree-walker) is not modified at all in this milestone — not even a visibility change.** It is investigated (read) freely, and its already-`pub`/`pub(super)` items (`Interpreter::send`'s siblings `try_factory_send`, `instantiate`, `class_of`, `builtin_class`, `alloc_identity_hash`, `new_array`/`new_string`/`new_symbol`/`new_character`/`new_closure`, `platform`, and `eval::eval_expr`/`eval::invoke_closure`) are reused freely, but every `fn` that is currently private to `interpreter`/`interpreter::eval` (`does_not_understand`, `dispatch_native_or_dnu`, `dispatch_native_inheriting_object`, `dispatch_lexical_send`, `dispatch_super_send`, `read_local`/`write_local`/`frame_at_depth`) is small enough (15-40 lines each) that `vm.rs` reimplements its *shape* directly against `dispatch.rs`'s public lookup functions and `builtins::dispatch_native` (already `pub fn`) rather than touching `interpreter/`'s visibility. This is an accepted, deliberate duplication — the cost of the earlier decision to keep the tree-walker as a permanent, independent reference engine — not an oversight; each duplication site says so inline.
- **Bytecode compilation of an already-resolved module can never raise a `NewspeakError`.** Every name is already resolved by the Milestone 1 resolver before compilation starts; compilation is a total, deterministic lowering. A compiler bug (an unreachable `Binding` variant, a register-count invariant violated) uses `panic!`/`debug_assert!`, exactly like an `ns-runtime` interpreter-invariant violation — never a `NewspeakError`. `unwrap`/`expect` remain fine in tests.
- **`Instr` is a plain, `match`-dispatched Rust enum, never hand-packed bytes.** "Architecture-neutral, versioned" is a property of `image::encode`/`decode`'s wire format, not of `Instr`'s in-memory shape.
- **Control-flow inlining is gated at compile time on syntactic block-literal arguments, and every inlined case keeps a runtime guard that falls back to a real, ordinary virtual send on any mismatch.** Never skip the fallback, even for a selector/receiver-type combination that "should never happen" — a `private`/mirror-based override of `True`/`False` is exactly the case the guard exists for.
- **Never mutate `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`, `docs/superpowers/specs/2026-09-01-ns-next-runtime-design.md`, or `docs/superpowers/specs/2026-09-04-ns-next-liveness-images-vm-design.md` while executing this plan.** If implementation reveals one of them is wrong, stop and flag it (per the subagent-driven-development skill's ruling process) rather than silently diverging.
- **Commits:** conventional-commit style (`feat:`, `test:`, `fix:`, `docs:`, `chore:`). Commit at the end of every task. Keep the trailer `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`. Never override git author/committer identity with `-c`.
- **This milestone must not regress Milestone 1.** `cargo run -p ns-cli -- parse-corpus corpus/modules` must still report `parsed 210, failed 0, expected-fail 1, stale 0` at the end (Task 15).

---

## File Structure

```
D:\Workspace\ns-next\
  crates/
    ns-runtime/
      src/
        types.rs                    # unchanged in shape; ClassData/Frame reused as-is
        dispatch.rs                 # unchanged, reused directly
        interpreter/                # unchanged, kept as --engine=tree-walk
        bytecode.rs                 # NEW: Instr, CompiledMethod, CallSite
        compiler/
          mod.rs                    # NEW: compile_method / compile_module entry points
          lower.rs                  # NEW: Lowerer — Expr/Stmt -> Instr, register allocation
          control_flow.rs           # NEW: special-selector recognition, inlined Branch/Jump
        vm.rs                       # NEW: the register VM's run loop and dispatch orchestration
        image/
          mod.rs                    # NEW: snapshot()/load(), header, format version
          encode.rs                 # NEW: canonical value/object/class/method encoding
          decode.rs                 # NEW: canonical decoding + version check
        lib.rs                      # add the five new modules' `pub mod` lines
      tests/
        bytecode.rs                 # NEW: Instr/CompiledMethod/CallSite construction tests
        compiler.rs                 # NEW: lowering unit tests (literals/locals/sends/closures)
        control_flow.rs             # NEW: inlining gate + guard + fallback tests
        vm.rs                       # NEW: VM dispatch tests (ordinary/lexical/super/closures)
        image.rs                    # NEW: image encode/decode round-trip tests
    ns-cli/
      src/main.rs                   # add --engine, snapshot, load, disasm
      tests/
        differential.rs             # NEW: every tests-fixtures/*.ns under both engines
        disasm_snapshots.rs         # NEW: insta snapshots of `disasm` output
  tests-fixtures/
    while_loop.ns / .expected       # NEW: exercises the new whileTrue: capability (Task 10)
    control_flow_fallback.ns / .expected  # NEW: non-Boolean receiver to ifTrue:ifFalse: (Task 10)
```

Responsibilities and split rationale:

- **`bytecode.rs`** — one file, pure data (`Instr`, `CompiledMethod`, `CallSite`), no behavior. Mirrors `types.rs`'s own "mutually-referential pure data in one file" rationale from the Milestone 1 plan.
- **`compiler/`** — split by concern, mirroring `resolver/`'s Milestone 1 split: `lower.rs` (the core `Expr`/`Stmt` -> `Instr` translation and register allocation, independently testable) vs. `control_flow.rs` (the special-selector recognition and inlining, a distinct algorithm with its own test surface, added in its own task so its introduction is independently reviewable) vs. `mod.rs` (wiring: walks a loaded class's method table, calls `lower.rs`/`control_flow.rs`, produces the compiled-method cache `compile_module` returns).
- **`vm.rs`** — one file. Unlike `resolver/`/`compiler/`, the VM's run loop, dispatch orchestration, and closure/non-local-return handling are one tightly coupled unit (the same shape `interpreter/mod.rs` + `interpreter/eval.rs` together form in Milestone 1) — no natural sub-boundary, so this plan keeps it as one file and splits it into tasks by *behavior added* (Tasks 6-7) rather than by file.
- **`image/`** — split the way `resolver/` is: `encode.rs`/`decode.rs` hold the actual byte-level format (independently testable against fixed byte sequences), `mod.rs` holds the public `snapshot()`/`load()` entry points and the CLI-facing semantics.
- **`ns-cli/tests/differential.rs`** vs. folding into `ns-runtime/tests/`: the differential harness needs to invoke the *compiled `ns-next` binary* under two different `--engine` flags (exactly like the existing `run_fixtures.rs` invokes it under `run`), so it belongs in `ns-cli/tests/`, next to `run_fixtures.rs`, not in `ns-runtime` (which has no CLI to invoke).

---

## Task 1: `bytecode.rs` — `Instr`, `CompiledMethod`, `CallSite`

**Files:**
- Create: `crates/ns-runtime/src/bytecode.rs`
- Modify: `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Consumes: `Symbol` (existing), `ns_syntax::Span` (existing).
- Produces: `Instr`, `CompiledMethod`, `CallSite` — every later task's shared vocabulary. `BranchIfFalse`/`Jump` are deliberately **not** added here; Task 10 adds them alongside the only code that ever emits them, so control-flow's introduction is reviewable as one self-contained diff.

**Design notes:**
- A "register" is one `u16` index into a *unified* address space that generalizes Milestone 1's `Binding::Local` addressing: index `< num_args` means `Frame.args[index]`, index `>= num_args` means `Frame.temps[index - num_args]`. `CompiledMethod.num_registers` is the **total** register count (`num_args` + named temps + compiler-allocated scratch registers) — `Vm` (Task 6) sizes `Frame.temps` to `num_registers - num_args` when building an activation. This is not a new addressing scheme; it is `read_local`/`write_local`'s existing args-then-temps split, generalized so scratch registers are just more temps slots, which is what the design spec's "`Frame.temps` doubles as the register file" line means concretely.
- `CallSite { call_site_id: u16 }` — `call_site_id` is assigned by the compiler as the `Send*` instruction's own index into `CompiledMethod.instrs` (so it is trivially stable and trivially reconstructible, never a separately-tracked counter that could drift out of sync).
- **`MakeClosure` carries the block's original AST (`params`, `body: Rc<ast::CodeBody>`), not a compiled `CompiledMethod`.** This is a correction made while designing Task 6's dispatch orchestration (documented in full there and in Task 7): every native primitive that invokes a block (`ifTrue:ifFalse:`, `do:`, `and:`/`or:`, `Block value`/`value:`) goes through `builtins::call_block` -> `interpreter::eval::invoke_closure`, which only knows how to run a `ClosureData` whose `body` is AST — and `call_block` is shared, unmodified infrastructure both engines use (Global Constraints: `interpreter/` is not touched). So in Milestone 2, a block passed to a native primitive still runs its body via the existing tree-walking `invoke_closure`, for both engines — compiling *general* block bodies to bytecode, and giving the VM its own closure-invocation path, is out of scope for this milestone (see Task 7's Non-Goal note for exactly why and what a later milestone would need to add it). `Instr::MakeClosure` therefore only needs enough to build a `ClosureData` in the exact shape `eval_block_literal` already builds one — there is deliberately no `NonLocalReturn` opcode or block-specific lowering mode in this plan (an earlier draft of Tasks 1 and 4 included both; removed once this was understood, rather than shipped as unused, untested code against the project's own "no half-finished implementations" principle). The one place a block's statements genuinely do become real bytecode is control-flow inlining (Task 10) — and there, they are lowered directly into the *enclosing method's own* instruction stream (no separate activation, no `MakeClosure` at all), so an inlined block's `^` is simply an ordinary `Instr::Return` — see Task 10.

- [ ] **Step 1: Write the failing tests**

`crates/ns-runtime/src/bytecode.rs`:

```rust
//! The bytecode instruction set and a compiled method's data shape. Pure
//! data -- see `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`'s
//! Bytecode section. `Instr` is a plain Rust enum, `match`-dispatched by
//! `vm.rs`; "architecture-neutral, versioned" describes `image::encode`'s
//! wire format for a `CompiledMethod`, not this in-memory type.

use std::rc::Rc;

use ns_syntax::{ast, Span};

use crate::Symbol;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_method_construction_smoke_test() {
        let m = CompiledMethod {
            instrs: vec![
                Instr::LoadLiteral { dst: 2, literal: Literal::SmallInt(42) },
                Instr::Return { reg: 2 },
            ],
            num_registers: 3,
            num_args: 1,
            statement_pc: vec![(0, 0..2)],
            call_sites: vec![],
            source: Span::new(0, 10),
        };
        assert_eq!(m.instrs.len(), 2);
        assert_eq!(m.num_registers, 3);
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
        assert!(matches!(send, Instr::SendPublic { .. }));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime bytecode::`
Expected: FAIL — `Instr`/`CompiledMethod`/`CallSite`/`Literal` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/ns-runtime/src/bytecode.rs` (before the `#[cfg(test)]` module):

```rust
/// A compiled instruction-set literal -- a lowered form of `ast::Literal`
/// that keeps only what execution needs (no `Span`, no raw source text for
/// numbers -- `eval::eval_literal`'s int/float text parsing already
/// happened once, at compile time, so it never needs redoing per
/// execution). `Str`/`Sym`/`Char`/`Array` still allocate a heap object each
/// time they execute (matching `eval_literal`'s own behavior -- a literal
/// string is a fresh `Object` on every evaluation in Milestone 1 too, not
/// interned).
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    pub call_site_id: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Instr {
    LoadLiteral { dst: u16, literal: Literal },
    LoadSelf { dst: u16 },
    Move { dst: u16, src: u16 },
    LoadLocal { dst: u16, slot: u16 },
    StoreLocal { src: u16, slot: u16 },
    LoadOuterLocal { dst: u16, depth: u16, index: u16 },
    StoreOuterLocal { src: u16, depth: u16, index: u16 },
    /// `Binding::LexicalSend` -- kth-enclosing-object + private/`lookupProtected`.
    SendLexical {
        depth: u16,
        selector: Symbol,
        arg_regs: Vec<u16>,
        dst: u16,
        call_site: CallSite,
    },
    /// An ordinary, explicit-receiver send (`lookupPublic`).
    SendPublic {
        selector: Symbol,
        receiver_reg: u16,
        arg_regs: Vec<u16>,
        dst: u16,
        call_site: CallSite,
    },
    /// `Binding::SuperSend`.
    SendSuper {
        selector: Symbol,
        receiver_reg: u16,
        arg_regs: Vec<u16>,
        dst: u16,
        call_site: CallSite,
    },
    /// Builds a closure carrying its original AST body -- see this task's
    /// design note on why (in short: every native primitive that invokes a
    /// block goes through the unmodified, AST-walking
    /// `interpreter::eval::invoke_closure`, for both engines, in this
    /// milestone).
    MakeClosure { dst: u16, params: Rc<Vec<Symbol>>, body: Rc<ast::CodeBody> },
    MakeArray { dst: u16, elem_regs: Vec<u16> },
    /// A compiled method body's own `^` (every `CompiledMethod` in this
    /// milestone represents a method-level activation -- see Task 4's
    /// design note on why no separate non-local-return opcode exists).
    Return { reg: u16 },
}

#[derive(Debug, Clone)]
pub struct CompiledMethod {
    pub instrs: Vec<Instr>,
    /// Total register count: `num_args` + named temps + compiler-allocated
    /// scratch registers. `Vm` sizes `Frame.temps` to
    /// `num_registers - num_args`.
    pub num_registers: u16,
    pub num_args: u16,
    /// Maps a statement's index in the source `CodeBody.stmts` to the
    /// half-open instruction-index range it compiled to -- the safepoint
    /// boundary and the M1-compatible "statement `pc`" view recovered on
    /// demand (design spec's Safepoints section).
    pub statement_pc: Vec<(usize, std::ops::Range<usize>)>,
    /// One entry per `Send*` instruction, in instruction order --
    /// `call_sites[i].call_site_id` always equals the position of its own
    /// `Send*` instruction in `instrs`; inert in this milestone (Milestone 3
    /// attaches inline-cache state here).
    pub call_sites: Vec<CallSite>,
    pub source: Span,
}
```

- [ ] **Step 4: Wire into `lib.rs`**

Add `pub mod bytecode;` to `crates/ns-runtime/src/lib.rs` (alongside the other `pub mod` lines), and add `pub use bytecode::{CallSite, CompiledMethod, Instr, Literal as BcLiteral};` — aliased `BcLiteral` because `bytecode::Literal` would otherwise collide with `ns_syntax::ast::Literal` for any file that globs both (`compiler/lower.rs` needs exactly this; see Task 2).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-runtime bytecode::`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): add the bytecode instruction set and CompiledMethod

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 2: `compiler/lower.rs` — register allocation, literals, locals

**Files:**
- Create: `crates/ns-runtime/src/compiler/lower.rs`
- Create: `crates/ns-runtime/src/compiler/mod.rs` (just enough to compile: `mod lower;`)
- Modify: `crates/ns-runtime/src/lib.rs`

**Interfaces:**
- Consumes: `ast::{Expr, Stmt, Literal}`, `Binding`, `Instr`/`CompiledMethod`/`CallSite`/`bytecode::Literal` (Task 1).
- Produces: `pub(crate) struct Lowerer` with `pub(crate) fn new(num_args, num_named_temps, bindings) -> Self`, `pub(crate) fn lower_expr(&mut self, expr: &ast::Expr) -> u16`, `pub(crate) fn lower_statement(&mut self, stmt: &ast::Stmt)`, `pub(crate) fn finish(self) -> (Vec<Instr>, u16, Vec<(usize, Range<usize>)>, Vec<CallSite>)`. Later tasks (3-4) extend `lower_expr`/`lower_statement`'s `match` arms in place; Task 5 wraps `Lowerer` into `compile_method`/`compile_module`.

**Design notes:**
- **Register numbering** is the unified args-then-temps space from Task 1's design note: `Lowerer::new(num_args, num_named_temps, ..)` sets `base = num_args + num_named_temps` (the first free scratch register) and a bump allocator `next_scratch` that starts at `base` and is reset to `base` at the top of every statement (`lower_statement`'s first action) — scratch registers never need to stay live across a statement boundary, matching the design spec's "Bytecode" section.
- A named local's own register number is fixed and known before any lowering starts: parameter `i` is register `i`; named temp `j` is register `num_args + j`. `Lowerer` never allocates these itself — `compile_method` (Task 5) computes them once (the same computation `Binding::Local`'s numbering already assumes) and only asks `Lowerer` for *scratch* registers.
- `lower_expr` returns the register holding the expression's value; every caller (an assignment's RHS, a future send's receiver/argument) just reads that register back. This is deliberately not "always allocate a fresh register" — `Expr::Ident(Binding::Local(i))` returns `i` directly with **no** `Move` emitted, since the value already lives there; a caller than immediately needs to mutate it into another register does so explicitly. This keeps the common case (reading a local into a send's argument list) at zero extra instructions.
- `Expr::Send`/`Expr::Cascade`/`Expr::Block`/`Expr::DynamicArray` are `unimplemented!()` stubs in this task (Task 3 fills in sends/cascades, Task 4 fills in blocks/arrays/return) — the same incremental-stub convention Milestone 1's lexer plan used for not-yet-lexed token kinds.

- [ ] **Step 1: Write the failing tests**

`crates/ns-runtime/src/compiler/lower.rs`:

```rust
//! `Expr`/`Stmt` -> `Instr` lowering and register allocation. See
//! `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`'s
//! Bytecode section.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use crate::bytecode::{CallSite, Instr, Literal as BcLiteral};
use crate::types::Binding;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Parses `src` (a full, minimal `class ... ( ) ( <one method> ) : ()`
    /// module), resolves it for real via `crate::resolve`, and returns the
    /// module's first instance-side method plus the real bindings map it
    /// resolved under -- the same "parse a throwaway module" pattern
    /// `dispatch.rs`'s own tests already use, so these lowering tests
    /// exercise real `Binding`s rather than hand-built ones.
    fn resolve_first_method(src: &str) -> (ast::MethodDecl, Rc<HashMap<Span, Binding>>) {
        let module = ns_syntax::parse(src).expect("parse");
        let resolved =
            crate::resolve(module, Rc::from(src), crate::SourceOrigin::File(PathBuf::from("t.ns")))
                .expect("resolve");
        let method = resolved.module.root_class.instance_side.methods[0].clone();
        (method, resolved.bindings.clone())
    }

    #[test]
    fn a_literal_returns_a_fresh_scratch_register() {
        let (method, bindings) =
            resolve_first_method("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^42 )\n) : ()");
        // `foo` has 0 params, 0 named temps -> base scratch register is 0.
        let mut lowerer = Lowerer::new(0, 0, &bindings);
        let ast::Stmt::Return(expr) = &method.body.stmts[0] else {
            panic!("expected a return statement");
        };
        let reg = lowerer.lower_expr(expr);
        assert_eq!(reg, 0);
        let (instrs, num_registers, _pc, _sites) = lowerer.finish();
        assert_eq!(instrs, vec![Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(42) }]);
        assert_eq!(num_registers, 1);
    }

    #[test]
    fn reading_a_parameter_returns_its_own_register_with_no_extra_instruction() {
        let (method, bindings) = resolve_first_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^x )\n) : ()",
        );
        // `foo:` has 1 param (register 0), 0 named temps.
        let mut lowerer = Lowerer::new(1, 0, &bindings);
        let ast::Stmt::Return(expr) = &method.body.stmts[0] else {
            panic!("expected a return statement");
        };
        let reg = lowerer.lower_expr(expr);
        assert_eq!(reg, 0, "reading a param should return its own register");
        let (instrs, _num_registers, _pc, _sites) = lowerer.finish();
        assert!(instrs.is_empty(), "no instruction needed to read a param");
    }

    #[test]
    fn assigning_a_named_temp_emits_store_local() {
        let (method, bindings) = resolve_first_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | t | t := 7. ^t )\n) : ()",
        );
        // `foo` has 0 params, 1 named temp `t` (register 0) -> base scratch is 1.
        let mut lowerer = Lowerer::new(0, 1, &bindings);
        lowerer.lower_statement(&method.body.stmts[0]); // `t := 7.`
        let (instrs, num_registers, statement_pc, _sites) = lowerer.finish();
        assert_eq!(
            instrs,
            vec![
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(7) },
                Instr::StoreLocal { src: 1, slot: 0 },
            ]
        );
        assert_eq!(num_registers, 2);
        assert_eq!(statement_pc, vec![(0, 0..2)]);
    }

    #[test]
    fn scratch_registers_reset_between_statements() {
        let (method, bindings) = resolve_first_method(
            "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | t | t := 1. t := 2. ^t )\n) : ()",
        );
        let mut lowerer = Lowerer::new(0, 1, &bindings);
        lowerer.lower_statement(&method.body.stmts[0]);
        lowerer.lower_statement(&method.body.stmts[1]);
        let (instrs, num_registers, _pc, _sites) = lowerer.finish();
        // Both statements' literal lands in the same scratch register (1) --
        // proof the allocator reset between statements instead of growing.
        assert_eq!(
            instrs,
            vec![
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(1) },
                Instr::StoreLocal { src: 1, slot: 0 },
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(2) },
                Instr::StoreLocal { src: 1, slot: 0 },
            ]
        );
        assert_eq!(num_registers, 2);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: FAIL — `Lowerer` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/ns-runtime/src/compiler/lower.rs` (before the `#[cfg(test)]` module):

```rust
pub(crate) struct Lowerer<'a> {
    bindings: &'a Rc<HashMap<Span, Binding>>,
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
        num_args: u16,
        num_named_temps: u16,
        bindings: &'a Rc<HashMap<Span, Binding>>,
    ) -> Self {
        let base = num_args + num_named_temps;
        Lowerer {
            bindings,
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

    /// One `Send*` instruction's `CallSite` -- `call_site_id` is that
    /// instruction's own about-to-be-pushed index, so it is always in sync
    /// with `instrs` with no separate counter to drift.
    pub(crate) fn next_call_site(&self) -> CallSite {
        CallSite { call_site_id: self.instrs.len() as u16 }
    }

    pub(crate) fn record_call_site(&mut self, site: CallSite) {
        self.call_sites.push(site);
    }

    /// Lowers one statement, recording its safepoint range in
    /// `statement_pc` and resetting the scratch-register bump allocator
    /// first (statements are the safepoint/scratch-liveness boundary; see
    /// this task's design notes).
    pub(crate) fn lower_statement(&mut self, stmt: &Stmt) {
        self.next_scratch = self.base;
        let start = self.instrs.len();
        match stmt {
            Stmt::Expr(e) => {
                self.lower_expr(e);
            }
            Stmt::Return(_) => {
                unimplemented!("Stmt::Return is lowered starting in Task 4")
            }
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

    fn lower_ident_read(&mut self, id: &ast::Ident) -> u16 {
        if id.name == "self" {
            let dst = self.alloc_scratch();
            self.emit(Instr::LoadSelf { dst });
            return dst;
        }
        match self.bindings.get(&id.span) {
            Some(Binding::Local(slot)) => *slot,
            Some(Binding::OuterLocal { depth, index }) => {
                let dst = self.alloc_scratch();
                self.emit(Instr::LoadOuterLocal { dst, depth: *depth, index: *index });
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
                self.emit(Instr::StoreLocal { src: v, slot: *slot });
            }
            Some(Binding::OuterLocal { depth, index }) => {
                self.emit(Instr::StoreOuterLocal { src: v, depth: *depth, index: *index });
            }
            Some(Binding::LexicalSend { .. }) | Some(Binding::SuperSend) | None => {
                unimplemented!("setter-send assignment is lowered starting in Task 3")
            }
        }
        // An assignment's own value is `value`'s value (runtime spec's
        // "Setter sends" note) -- already sitting in register `v`.
        v
    }

    pub(crate) fn finish(self) -> (Vec<Instr>, u16, Vec<(usize, Range<usize>)>, Vec<CallSite>) {
        (self.instrs, self.max_reg, self.statement_pc, self.call_sites)
    }
}

fn lower_literal(lit: &ast::Literal) -> BcLiteral {
    match lit {
        ast::Literal::Int { text, .. } => {
            // Mirrors `interpreter::eval::parse_int_literal` exactly --
            // duplicated rather than reused because that function is private
            // to `interpreter::eval` (Global Constraints: `interpreter/` is
            // not modified, including visibility, in this milestone).
            BcLiteral::SmallInt(parse_int_literal(text))
        }
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

/// Same radix-literal-aware parsing as `interpreter::eval::parse_int_literal`
/// (duplicated for the same visibility reason as `lower_literal` above).
/// Unlike that function, this one may `panic!` on a malformed literal
/// instead of returning a `Result`: **compiling an already-resolved module
/// can never raise a `NewspeakError`** (Global Constraints) -- a literal
/// whose text the *lexer* already accepted as `TokenKind::Number` is
/// guaranteed well-formed by construction, so a parse failure here would be
/// an `ns-runtime`/`ns-syntax` invariant violation, not a bad Newspeak
/// program. (The tree-walker's own `parse_int_literal` still returns a
/// `Result` because it re-parses on every *execution*, where a
/// theoretically-malformed corpus file could reach it before this
/// milestone's compiler existed; this function only ever sees the same
/// already-lexed text at compile time, once.)
fn parse_int_literal(text: &str) -> i64 {
    let Some((radix_part, digits)) = text.split_once('r') else {
        return text.parse::<i64>().expect("lexer-accepted integer literal must parse");
    };
    let (negated, radix_digits) = match radix_part.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, radix_part),
    };
    let radix: u32 = radix_digits.parse().expect("lexer-accepted radix must parse");
    let value = i64::from_str_radix(digits, radix).expect("lexer-accepted radix digits must parse");
    if negated {
        value.checked_neg().expect("radix literal negation overflow")
    } else {
        value
    }
}
```

`crates/ns-runtime/src/compiler/mod.rs`:

```rust
//! Bytecode compiler entry points. See
//! `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`.

mod lower;
```

- [ ] **Step 4: Wire into `lib.rs`**

Add `pub mod compiler;` to `crates/ns-runtime/src/lib.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): lower literals and local variable access to bytecode

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 3: `compiler/lower.rs` — sends, cascades, builtin-name reads

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (add `LoadBuiltinClass`)
- Modify: `crates/ns-runtime/src/compiler/lower.rs`

**Interfaces:**
- Consumes: `dispatch.rs`'s `Binding` variants (unchanged), `crate::interpreter::instantiate::builtin_kind_from_name` (already `pub(crate)` — no visibility change needed), `types::BuiltinClass`.
- Produces: `lower_expr` now handles `Expr::Send`/`Expr::Cascade` fully; `lower_ident_read` now handles every `Binding` variant.

**Design notes:**
- **A gap found against the spec's stated register-allocation scheme, resolved here:** the design spec describes scratch registers as "freed and reused as each subexpression's consumer is emitted." Task 2's simpler bump-allocator (reset once per statement, never mid-statement) does not do that — a deeply nested expression uses one scratch register per intermediate value for the whole statement, not just while that value is live. This plan deliberately keeps the simpler scheme: Newspeak method bodies are not deeply nested in practice, `Frame.temps` is a plain growable `Vec` (no real cost to a few extra live entries), and true mid-statement liveness-based reuse is real added complexity Milestone 2 does not need yet (project constraint: don't add complexity ahead of a measured need). Revisit only if a later milestone's profiling shows register-file size actually matters.
- `LoadBuiltinClass` did not appear in the design spec's opcode list — a real gap found while implementing `eval_ident`'s "Gap 1 fix" fallback (a bare identifier naming a Milestone 1 builtin class, e.g. plain `OrderedCollection`, with no resolver `Binding` at all). Added here, at the point it's first needed, the same way Milestone 1's own lexer plan added `TokenKind` variants in the task that first needed them.
- Cascade messages are **always** ordinary (`SendPublic`)-shaped: `ast::Cascade`'s `first`/`rest` are bare `MessageSend`s with no receiver expression of their own (confirmed against `eval.rs::eval_cascade`, which always calls `interp.send(...)`, never checking `bindings` for a cascade message's own span) — there is no self/super/lexical cascade-message shape to handle.

- [ ] **Step 1: Add `LoadBuiltinClass` and write the failing tests**

In `crates/ns-runtime/src/bytecode.rs`, add `use crate::types::BuiltinClass;` near the top and a new `Instr` variant (next to `LoadSelf`):

```rust
LoadBuiltinClass { dst: u16, kind: BuiltinClass },
```

Add to `crates/ns-runtime/src/compiler/lower.rs`'s test module:

```rust
#[test]
fn an_ordinary_send_evaluates_receiver_then_args() {
    let (method, bindings) = resolve_first_method(
        "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^3 + x )\n) : ()",
    );
    let mut lowerer = Lowerer::new(1, 0, &bindings);
    let ast::Stmt::Return(expr) = &method.body.stmts[0] else { panic!("return") };
    let dst = lowerer.lower_expr(expr);
    let (instrs, num_registers, _pc, sites) = lowerer.finish();
    assert_eq!(
        instrs,
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
    assert_eq!(num_registers, 3);
    assert_eq!(sites, vec![CallSite { call_site_id: 1 }]);
}

#[test]
fn a_bare_implicit_send_lowers_to_send_lexical() {
    // `foo`'s body (`^bar`) resolves `bar` as an implicit-receiver unary
    // send at depth 0 (`bar` is a sibling method on the same class) --
    // `Binding::LexicalSend { depth: 0 }`, per the runtime spec's Resolver
    // section.
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  bar = ( ^42 )\n  foo = ( ^bar )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(src),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[1].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(0, 0, &bindings);
    let ast::Stmt::Return(expr) = &foo.body.stmts[0] else {
        panic!("expected a return statement");
    };
    let dst = lowerer.lower_expr(expr);
    let (instrs, _num_registers, _pc, sites) = lowerer.finish();
    assert_eq!(
        instrs,
        vec![Instr::SendLexical {
            depth: 0,
            selector: crate::Symbol::intern("bar"),
            arg_regs: vec![],
            dst: 0,
            call_site: CallSite { call_site_id: 0 },
        }]
    );
    assert_eq!(dst, 0);
    assert_eq!(sites, vec![CallSite { call_site_id: 0 }]);
}

#[test]
fn a_bare_builtin_class_name_lowers_to_load_builtin_class() {
    let src =
        "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^OrderedCollection )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(src),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[0].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(0, 0, &bindings);
    let ast::Stmt::Return(expr) = &foo.body.stmts[0] else {
        panic!("expected a return statement");
    };
    let dst = lowerer.lower_expr(expr);
    let (instrs, _num_registers, _pc, _sites) = lowerer.finish();
    assert_eq!(
        instrs,
        vec![Instr::LoadBuiltinClass { dst: 0, kind: crate::types::BuiltinClass::OrderedCollection }]
    );
    assert_eq!(dst, 0);
}

#[test]
fn a_cascade_reuses_one_receiver_register_across_every_message() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | c | c := OrderedCollection new. c add: 1; add: 2 )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(src),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[0].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(0, 1, &bindings); // 1 named temp `c` (register 0)
    lowerer.lower_statement(&foo.body.stmts[0]); // `c := OrderedCollection new.`
    lowerer.lower_statement(&foo.body.stmts[1]); // `c add: 1; add: 2`
    let (instrs, _num_registers, _pc, sites) = lowerer.finish();
    // Three `SendPublic` instructions total: `OrderedCollection new` (its
    // own receiver, a `LoadBuiltinClass` result), then the cascade's two
    // `add:` sends -- the *last two* must share one receiver register:
    // `c` is a named temp (`Binding::Local(0)`), so reading it costs no
    // instruction at all and `receiver_reg` is register 0 for both.
    let receiver_regs: Vec<u16> = instrs
        .iter()
        .filter_map(|i| match i {
            Instr::SendPublic { receiver_reg, .. } => Some(*receiver_reg),
            _ => None,
        })
        .collect();
    assert_eq!(receiver_regs.len(), 3);
    assert_eq!(
        receiver_regs[1], receiver_regs[2],
        "both cascade sends must share one receiver register"
    );
    assert_eq!(receiver_regs[1], 0, "`c` is register 0, read with no instruction");
    assert_eq!(sites.len(), 3, "1 send in the first statement + 2 cascade sends");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: FAIL — `Expr::Send`/`Expr::Cascade` still `unimplemented!()`, `LoadBuiltinClass` not found.

- [ ] **Step 3: Write the implementation**

Replace `lower_ident_read`'s and `lower_assign`'s bodies in `crates/ns-runtime/src/compiler/lower.rs`, and replace the `Expr::Send`/`Expr::Cascade` arms in `lower_expr`:

```rust
fn lower_ident_read(&mut self, id: &ast::Ident) -> u16 {
    if id.name == "self" {
        let dst = self.alloc_scratch();
        self.emit(Instr::LoadSelf { dst });
        return dst;
    }
    match self.bindings.get(&id.span) {
        Some(Binding::Local(slot)) => *slot,
        Some(Binding::OuterLocal { depth, index }) => {
            let dst = self.alloc_scratch();
            self.emit(Instr::LoadOuterLocal { dst, depth: *depth, index: *index });
            dst
        }
        Some(Binding::LexicalSend { depth }) => {
            let depth = *depth;
            self.emit_lexical_send(depth, crate::Symbol::intern(&id.name), vec![])
        }
        // Milestone 1's resolver never actually attaches `SuperSend` to a
        // bare identifier (only to a `super m` send's own span) -- grouped
        // with `None` here because both fall to the same builtin-name
        // fallback, mirroring `eval_ident`'s own arm grouping exactly.
        Some(Binding::SuperSend) | None => {
            match crate::interpreter::instantiate::builtin_kind_from_name(&id.name) {
                Some(kind) => {
                    let dst = self.alloc_scratch();
                    self.emit(Instr::LoadBuiltinClass { dst, kind });
                    dst
                }
                // Global Constraints: compiling an already-resolved module
                // can never raise a NewspeakError -- an identifier with no
                // binding and no builtin-name match here is an ns-runtime
                // resolver/compiler invariant violation, not a bad Newspeak
                // program (contrast `eval_ident`, which returns a
                // NewspeakError for this case -- it re-evaluates on every
                // execution, so in principle a differently-shaped bug could
                // reach it live; this function only ever sees output the
                // resolver already validated once, at compile time).
                None => panic!(
                    "internal: unresolved identifier `{}` (resolver should have rejected this)",
                    id.name
                ),
            }
        }
    }
}

fn lower_assign(&mut self, target: &ast::Ident, value: &Expr) -> u16 {
    let v = self.lower_expr(value);
    match self.bindings.get(&target.span) {
        Some(Binding::Local(slot)) => {
            self.emit(Instr::StoreLocal { src: v, slot: *slot });
        }
        Some(Binding::OuterLocal { depth, index }) => {
            self.emit(Instr::StoreOuterLocal { src: v, depth: *depth, index: *index });
        }
        Some(Binding::LexicalSend { depth }) => {
            let depth = *depth;
            let setter = crate::Symbol::intern(&format!("{}:", target.name));
            self.emit_lexical_send(depth, setter, vec![v]);
        }
        Some(Binding::SuperSend) | None => {
            panic!("internal: unresolved setter target `{}` (resolver should have rejected this)", target.name)
        }
    }
    // An assignment's own value is `value`'s value (runtime spec's "Setter
    // sends" note), already sitting in register `v`.
    v
}

fn lower_send(&mut self, send: &ast::Send) -> u16 {
    match self.bindings.get(&send.span) {
        Some(Binding::SuperSend) => {
            let selector = selector_of(&send.message);
            let arg_regs = self.lower_args(&send.message);
            let dst = self.alloc_scratch();
            let call_site = self.next_call_site();
            self.emit(Instr::SendSuper { selector, arg_regs, dst, call_site });
            self.record_call_site(call_site);
            dst
        }
        Some(Binding::LexicalSend { depth }) => {
            let depth = *depth;
            let selector = selector_of(&send.message);
            let arg_regs = self.lower_args(&send.message);
            self.emit_lexical_send(depth, selector, arg_regs)
        }
        _ => {
            let receiver_reg = self.lower_expr(&send.receiver);
            let selector = selector_of(&send.message);
            let arg_regs = self.lower_args(&send.message);
            let dst = self.alloc_scratch();
            let call_site = self.next_call_site();
            self.emit(Instr::SendPublic { selector, receiver_reg, arg_regs, dst, call_site });
            self.record_call_site(call_site);
            dst
        }
    }
}

fn emit_lexical_send(&mut self, depth: u16, selector: crate::Symbol, arg_regs: Vec<u16>) -> u16 {
    let dst = self.alloc_scratch();
    let call_site = self.next_call_site();
    self.emit(Instr::SendLexical { depth, selector, arg_regs, dst, call_site });
    self.record_call_site(call_site);
    dst
}

fn lower_args(&mut self, message: &ast::MessageSend) -> Vec<u16> {
    message.args.iter().map(|a| self.lower_expr(a)).collect()
}

fn lower_cascade(&mut self, cascade: &ast::Cascade) -> u16 {
    let receiver_reg = self.lower_expr(&cascade.receiver);
    let mut result = self.emit_cascade_message(receiver_reg, &cascade.first);
    for m in &cascade.rest {
        result = self.emit_cascade_message(receiver_reg, m);
    }
    result
}

fn emit_cascade_message(&mut self, receiver_reg: u16, message: &ast::MessageSend) -> u16 {
    let selector = selector_of(message);
    let arg_regs = self.lower_args(message);
    let dst = self.alloc_scratch();
    let call_site = self.next_call_site();
    self.emit(Instr::SendPublic { selector, receiver_reg, arg_regs, dst, call_site });
    self.record_call_site(call_site);
    dst
}
```

And in `lower_expr`'s `match`, replace:

```rust
Expr::Send(_) | Expr::Cascade(_) => {
    unimplemented!("sends/cascades are lowered starting in Task 3")
}
```

with:

```rust
Expr::Send(send) => self.lower_send(send),
Expr::Cascade(cascade) => self.lower_cascade(cascade),
```

Add, at module scope (outside `impl<'a> Lowerer<'a>`):

```rust
fn selector_of(message: &ast::MessageSend) -> crate::Symbol {
    match &message.selector {
        ast::Selector::Unary(s) | ast::Selector::Binary(s) => crate::Symbol::intern(s),
        ast::Selector::Keyword(s) => crate::Symbol::intern(s),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): lower sends, cascades, and builtin class name reads to bytecode

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 4: `compiler/lower.rs` — return, closures, arrays

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (add `ImplicitReturn`)
- Modify: `crates/ns-runtime/src/compiler/lower.rs`

**Interfaces:**
- Consumes: `ast::BlockExpr`, `ast::DynamicArray` (unchanged).
- Produces: `Lowerer::finish_body(&mut self, stmts: &[ast::Stmt])`, `Lowerer::last_value_register(&self) -> Option<u16>` (kept for Task 10's control-flow inlining, which needs "what did the last inlined statement compute" without a separate `CompiledMethod`). `lower_expr` now handles every `Expr` variant; no more `unimplemented!()` stubs remain.

**Design notes:**
- **Every `CompiledMethod` in this milestone represents a method-level activation, never a block's own — so every `^` a `Lowerer` ever lowers is an ordinary local return, and there is no `NonLocalReturn` opcode.** This corrects an earlier draft of this task (and of Task 1) that added `is_block`/`Lowerer::new_block`/`NonLocalReturn` for a "compile a block's body into its own `CompiledMethod`, invoked as its own bytecode activation" design. That design turned out to be dead code once Task 6/7's dispatch orchestration was worked out: every native primitive that invokes a block (`ifTrue:ifFalse:`, `do:`, `and:`/`or:`, `Block value`/`value:`) goes through `builtins::call_block` -> `interpreter::eval::invoke_closure`, shared, unmodified infrastructure (Global Constraints: `interpreter/` is not touched) that only ever runs a `ClosureData`'s AST `body` — so a *general* block literal's body is never bytecode-executed in Milestone 2 regardless of what `Lowerer` might have compiled it to. The only place a block's statements become real bytecode is control-flow inlining (Task 10), and there they are lowered directly into the *enclosing method's* own instruction stream (no separate activation at all), so a `^` inside an inlined block is just `Instr::Return` targeting that one, single, already-open activation. See Task 1's `MakeClosure` design note and Task 7's Non-Goal note for the full reasoning and what a later milestone would need to add real bytecode-compiled block invocation.
- **A new opcode, `ImplicitReturn { reg }`, absent from the design spec's opcode list** — another real gap found while implementing (mirrors `LoadBuiltinClass` in Task 3). Every compiled method body is made to end in an explicit terminating instruction, so the VM's dispatch loop (Task 6) never has to handle "fell off the end of `instrs`" as a separate case: if the source body's last statement was not itself `Stmt::Return`, the compiler appends `LoadSelf` + `ImplicitReturn` (methods implicitly return `self`, `newspeak-spec.pdf` §3.5). Unlike `Return`, executing `ImplicitReturn` never touches `uncontinuable` — it is an ordinary "hand this value back to the caller" exit, matching how `run_statements`' own "fell off the end" path never raises a `Signal` at all. (`Interpreter::invoke_method`'s `uncontinuable = true` marking happens once, in the *invoking* wrapper, on every exit path uniformly — Task 6 reproduces that same uniform marking around `ImplicitReturn`/`Return` alike, so neither opcode needs to do it itself.)
- `MakeClosure { dst, params, body }` builds a `ClosureData` in exactly the shape `eval_block_literal` already builds one. `captured_frame`/`home` (named in the design spec) are **not** `Instr::MakeClosure` fields — a bytecode instruction is static, compile-time data, and "the currently executing frame" only exists at run time; the VM (Task 7) supplies both implicitly from its own current frame when it executes the instruction, exactly as `eval_block_literal` passes `frame.clone()`. `bindings` is likewise not a field — the VM already holds the whole module's bindings `Rc` (Task 6) and supplies it directly, the same `Rc` clone `eval_block_literal` itself uses.
- `last_expr_reg`/`last_value_register` track the register a `Stmt::Expr` statement's value landed in, purely so Task 10 can read "what did this block's last statement compute" while inlining it into the enclosing method — a plain accessor, not part of any return-related branching (there is none left to have).

- [ ] **Step 1: Add `ImplicitReturn` and write the failing tests**

In `crates/ns-runtime/src/bytecode.rs`, add to `Instr` (next to `Return`):

```rust
/// Ends a compiled method body that did not itself end in an explicit
/// `^` -- see `compiler/lower.rs`'s `finish_body` design note.
ImplicitReturn { reg: u16 },
```

Add to `crates/ns-runtime/src/compiler/lower.rs`'s test module:

```rust
#[test]
fn a_method_with_no_explicit_return_implicitly_returns_self() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( 1 + 1 )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(src),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[0].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(0, 0, &bindings);
    for stmt in &foo.body.stmts {
        lowerer.lower_statement(stmt);
    }
    lowerer.finish_body(&foo.body.stmts);
    let (instrs, _num_registers, _pc, _sites) = lowerer.finish();
    assert!(matches!(instrs.last(), Some(Instr::ImplicitReturn { .. })));
    let self_reg = match instrs.last() {
        Some(Instr::ImplicitReturn { reg }) => *reg,
        _ => panic!(),
    };
    assert!(instrs.iter().any(|i| matches!(i, Instr::LoadSelf { dst } if *dst == self_reg)));
}

#[test]
fn an_explicit_return_needs_no_implicit_one_appended() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(src),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[0].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(0, 0, &bindings);
    for stmt in &foo.body.stmts {
        lowerer.lower_statement(stmt);
    }
    lowerer.finish_body(&foo.body.stmts);
    let (instrs, _num_registers, _pc, _sites) = lowerer.finish();
    assert_eq!(instrs, vec![
        Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
        Instr::Return { reg: 0 },
    ]);
}

#[test]
fn a_block_literal_lowers_to_make_closure_carrying_its_own_ast() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^[ :x | x ] )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(src),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[0].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(0, 0, &bindings);
    let ast::Stmt::Return(expr) = &foo.body.stmts[0] else { panic!("return") };
    lowerer.lower_expr(expr);
    let (instrs, _num_registers, _pc, _sites) = lowerer.finish();
    let Some(Instr::MakeClosure { params, body, .. }) = instrs.first() else {
        panic!("expected a MakeClosure instruction, got {instrs:?}");
    };
    assert_eq!(params.as_slice(), &[crate::Symbol::intern("x")]);
    assert_eq!(body.stmts.len(), 1); // the block's own, uncompiled AST body
}

#[test]
fn last_value_register_tracks_the_most_recent_expr_statement() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( | t | t := 1. t := 2 )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(src),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[0].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(0, 1, &bindings);
    for stmt in &foo.body.stmts {
        lowerer.lower_statement(stmt);
    }
    assert!(lowerer.last_value_register().is_some());
}

#[test]
fn a_dynamic_array_literal_lowers_to_make_array() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^{ 1. 2. 3 } )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(src),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[0].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(0, 0, &bindings);
    let ast::Stmt::Return(expr) = &foo.body.stmts[0] else { panic!("return") };
    let dst = lowerer.lower_expr(expr);
    let (instrs, _num_registers, _pc, _sites) = lowerer.finish();
    assert_eq!(
        instrs,
        vec![
            Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
            Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(2) },
            Instr::LoadLiteral { dst: 2, literal: BcLiteral::SmallInt(3) },
            Instr::MakeArray { dst: 3, elem_regs: vec![0, 1, 2] },
        ]
    );
    assert_eq!(dst, 3);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: FAIL — `finish_body`/`ImplicitReturn` not found, `Expr::Block`/`Expr::DynamicArray` still `unimplemented!()`.

- [ ] **Step 3: Write the implementation**

In `crates/ns-runtime/src/compiler/lower.rs`, add one field to the `Lowerer` struct: `last_expr_reg: Option<u16>`. Update the constructor and `lower_statement`:

```rust
pub(crate) struct Lowerer<'a> {
    bindings: &'a Rc<HashMap<Span, Binding>>,
    instrs: Vec<Instr>,
    base: u16,
    next_scratch: u16,
    max_reg: u16,
    statement_index: usize,
    statement_pc: Vec<(usize, Range<usize>)>,
    call_sites: Vec<CallSite>,
    last_expr_reg: Option<u16>,
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(num_args: u16, num_named_temps: u16, bindings: &'a Rc<HashMap<Span, Binding>>) -> Self {
        let base = num_args + num_named_temps;
        Lowerer {
            bindings,
            instrs: Vec::new(),
            base,
            next_scratch: base,
            max_reg: base,
            statement_index: 0,
            statement_pc: Vec::new(),
            call_sites: Vec::new(),
            last_expr_reg: None,
        }
    }

    pub(crate) fn lower_statement(&mut self, stmt: &Stmt) {
        self.next_scratch = self.base;
        let start = self.instrs.len();
        match stmt {
            Stmt::Expr(e) => {
                let reg = self.lower_expr(e);
                self.last_expr_reg = Some(reg);
            }
            Stmt::Return(e) => {
                let reg = self.lower_expr(e);
                self.emit(Instr::Return { reg });
                self.last_expr_reg = None;
            }
        }
        let end = self.instrs.len();
        self.statement_pc.push((self.statement_index, start..end));
        self.statement_index += 1;
    }

    /// The register the most recently lowered `Stmt::Expr` statement's
    /// value landed in -- `None` if no statement has been lowered yet, or
    /// the most recent one was a `Stmt::Return`. Used only by Task 10's
    /// control-flow inlining, to read an inlined block's implicit result.
    pub(crate) fn last_value_register(&self) -> Option<u16> {
        self.last_expr_reg
    }

    /// Appends `LoadSelf` + `ImplicitReturn` if `stmts` (the method body
    /// just lowered via repeated `lower_statement` calls) did not already
    /// end in an explicit `^` -- see this task's `ImplicitReturn` design
    /// note. Every `Lowerer` in this milestone lowers a method-level body,
    /// so this always means "return self", never "return the last value"
    /// (that would be a block's implicit-return rule, which does not apply
    /// here -- see this task's design note on why there is no block-body
    /// compilation mode).
    pub(crate) fn finish_body(&mut self, stmts: &[Stmt]) {
        if matches!(stmts.last(), Some(Stmt::Return(_))) {
            return;
        }
        let dst = self.alloc_scratch();
        self.emit(Instr::LoadSelf { dst });
        self.emit(Instr::ImplicitReturn { reg: dst });
    }
}
```

Replace the `Expr::Block`/`Expr::DynamicArray` arms in `lower_expr`:

```rust
Expr::Block(block) => self.lower_block_literal(block),
Expr::DynamicArray { elements, .. } => {
    let elem_regs: Vec<u16> = elements.iter().map(|e| self.lower_expr(e)).collect();
    let dst = self.alloc_scratch();
    self.emit(Instr::MakeArray { dst, elem_regs });
    dst
}
```

Add a new method to `impl<'a> Lowerer<'a>`:

```rust
/// Builds a `MakeClosure` instruction carrying the block's own, uncompiled
/// AST (params + body) -- see this task's design note on why a block's
/// body is never itself compiled to bytecode in this milestone.
fn lower_block_literal(&mut self, block: &ast::BlockExpr) -> u16 {
    let params: Vec<crate::Symbol> =
        block.params.iter().map(|p| crate::Symbol::intern(&p.name)).collect();
    let dst = self.alloc_scratch();
    self.emit(Instr::MakeClosure {
        dst,
        params: Rc::new(params),
        body: Rc::new(block.body.clone()),
    });
    dst
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: PASS (13 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): lower return, block literals, and array literals to bytecode

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 5: `compiler/mod.rs` — `compile_method` / `compile_module`

**Files:**
- Modify: `crates/ns-runtime/src/compiler/mod.rs`
- Modify: `crates/ns-runtime/src/lib.rs`
- Create: `crates/ns-runtime/tests/compiler.rs`

**Interfaces:**
- Consumes: `Lowerer` (Tasks 2-4), `types::{ClassRef, ObjectData, Binding}`.
- Produces (the entry points every later task depends on):
  - `pub fn compile_method(method: &ast::MethodDecl, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledMethod`
  - `pub type CompiledMethodCache = HashMap<usize, Rc<CompiledMethod>>;`
  - `pub fn method_key(method: &Rc<ast::MethodDecl>) -> usize` — `Rc::as_ptr(method) as *const () as usize`; the same key a `dispatch::Lookup::Method(method, _)` result's `method` must be hashed with to find its compiled form (Task 6 does exactly this).
  - `pub fn compile_module(root_class: &ClassRef, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledMethodCache`

**Design notes:**
- **Why the cache is keyed by `Rc<ast::MethodDecl>` pointer identity, not `(class name, selector)`:** compilation happens *after* `Interpreter::load_module`/`load_class` have already built the real `ClassRef` graph (Milestone 1's `load_class` mints each method's `Rc::new(m.clone())` once, there, and that exact `Rc` is what every later `dispatch.rs` lookup clones and returns via `Lookup::Method`). Keying by that same `Rc`'s pointer identity means `compile_module`'s one walk over `root_class`'s already-loaded `methods` map produces exactly the keys dispatch will ever look up — no separate name-based indexing to keep in sync, and no risk of two different classes' same-named method colliding.
- **`compile_module` walks only `root_class.methods`**, not superclasses or nested classes: Milestone 1's `load_class` never loads nested classes and never links a real (non-builtin) superclass with its own methods (both are documented Milestone 1 gaps in `interpreter::instantiate`'s own doc comments) — so `root_class`'s own method map is already the *entire* set of user-defined methods Milestone 1's dispatch can ever reach. This cache's coverage is therefore already complete for Milestone 1's actual scope; extend this walk (recurse into `superclass`/`enclosing`) only if a later milestone starts loading either.

- [ ] **Step 1: Write the failing tests**

`crates/ns-runtime/tests/compiler.rs`:

```rust
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::Instr;
use ns_runtime::{compiler, types::ObjectData, Interpreter, SourceOrigin};

fn load(src: &str) -> (Interpreter, ns_runtime::ClassRef, Rc<std::collections::HashMap<ns_syntax::Span, ns_runtime::Binding>>) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns")))
        .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, bindings)
}

#[test]
fn compile_module_compiles_every_loaded_method() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n  bar = ( ^2 )\n) : ()";
    let (_interp, class, bindings) = load(src);
    let cache = compiler::compile_module(&class, &bindings);
    assert_eq!(cache.len(), 2);
    for compiled in cache.values() {
        assert!(matches!(compiled.instrs.last(), Some(Instr::Return { .. })));
    }
}

#[test]
fn compiled_method_is_reachable_by_its_dispatch_key() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()";
    let (_interp, class, bindings) = load(src);
    let cache = compiler::compile_module(&class, &bindings);
    let method = {
        let borrowed = class.borrow();
        let ObjectData::Class(data) = &borrowed.data else { panic!() };
        data.methods.get(&ns_runtime::Symbol::intern("foo")).unwrap().clone()
    };
    let key = compiler::method_key(&method);
    assert!(cache.contains_key(&key));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test compiler`
Expected: FAIL — `compiler::compile_module`/`method_key` not found; `bytecode`/`types`/`Binding` may not yet be re-exported at the paths the test uses.

- [ ] **Step 3: Write the implementation**

`crates/ns-runtime/src/compiler/mod.rs`:

```rust
//! Bytecode compiler entry points. See
//! `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`.

mod control_flow;
mod lower;

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::bytecode::CompiledMethod;
use crate::types::{Binding, ClassRef, ObjectData};

pub type CompiledMethodCache = HashMap<usize, Rc<CompiledMethod>>;

pub fn method_key(method: &Rc<ast::MethodDecl>) -> usize {
    Rc::as_ptr(method) as *const () as usize
}

fn pattern_param_count(pattern: &ast::MessagePattern) -> u16 {
    match pattern {
        ast::MessagePattern::Unary { .. } => 0,
        ast::MessagePattern::Binary { .. } => 1,
        ast::MessagePattern::Keyword { parts } => parts.len() as u16,
    }
}

pub fn compile_method(method: &ast::MethodDecl, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledMethod {
    let num_args = pattern_param_count(&method.pattern);
    let num_named_temps = method.body.temps.len() as u16;
    let mut lowerer = lower::Lowerer::new(num_args, num_named_temps, bindings);
    for stmt in &method.body.stmts {
        lowerer.lower_statement(stmt);
    }
    lowerer.finish_body(&method.body.stmts);
    let (instrs, num_registers, statement_pc, call_sites) = lowerer.finish();
    CompiledMethod { instrs, num_registers, num_args, statement_pc, call_sites, source: method.span }
}

pub fn compile_module(root_class: &ClassRef, bindings: &Rc<HashMap<Span, Binding>>) -> CompiledMethodCache {
    let methods: Vec<Rc<ast::MethodDecl>> = {
        let borrowed = root_class.borrow();
        match &borrowed.data {
            ObjectData::Class(data) => data.methods.values().cloned().collect(),
            _ => Vec::new(),
        }
    };
    let mut cache = CompiledMethodCache::new();
    for method in methods {
        let compiled = compile_method(&method, bindings);
        cache.insert(method_key(&method), Rc::new(compiled));
    }
    cache
}
```

`crates/ns-runtime/src/compiler/control_flow.rs` (empty placeholder module body — Task 10 fills it in; declared now so `mod control_flow;` above compiles):

```rust
//! Control-flow special-selector inlining. Implemented in Task 10.
```

- [ ] **Step 4: Wire into `lib.rs`**

`crates/ns-runtime/src/lib.rs` needs `compiler`, `bytecode`, `Binding`, and `ClassRef` reachable at the paths the test uses:

```rust
pub mod bytecode;
pub mod compiler;
```

(both alongside the existing `pub mod` lines; `pub use types::*;` already re-exports `Binding` and `ClassRef` from Task 3 of the Milestone 1 runtime plan, so no further change is needed there).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test compiler`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): add compile_method/compile_module entry points

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 6: `vm.rs` — the run loop, ordinary/lexical/super sends

**Files:**
- Create: `crates/ns-runtime/src/vm.rs`
- Modify: `crates/ns-runtime/src/lib.rs`
- Create: `crates/ns-runtime/tests/vm.rs`

**Interfaces:**
- Consumes: `dispatch::{lookup_public, lookup_protected, lookup_defined_for, lookup_slot, kth_enclosing_object, effective_access, Lookup}` (unchanged, `pub`), `builtins::dispatch_native` (unchanged, `pub`), `interpreter::Interpreter::{try_factory_send, class_of, builtin_class, new_symbol, new_array}` (unchanged, already `pub`/`pub(super)`-visible-everywhere), `compiler::{CompiledMethodCache, method_key}` (Task 5).
- Produces: `pub struct Vm`, `pub fn Vm::new(compiled: CompiledMethodCache, bindings: Rc<HashMap<Span, Binding>>) -> Vm`, `pub fn Vm::send(&mut self, interp: &mut Interpreter, receiver: Value, selector: Symbol, args: Vec<Value>, sender: Option<FrameRef>) -> EvalResult` — the entry point Task 8's CLI wiring and every later task use to run a message send under the bytecode engine.

**Design notes (read in full before writing code — this task duplicates, deliberately, a real amount of `interpreter/mod.rs` and `interpreter/eval.rs`'s orchestration logic):**
- **Why `vm.rs` reimplements `send`/`does_not_understand`/`dispatch_lexical_send`/`dispatch_super_send`'s shape instead of sharing it:** every one of those functions' current implementation calls `Interpreter::invoke_method` (or is itself private/module-private to `interpreter`/`interpreter::eval`) for its `Lookup::Method` arm — the one place this milestone's whole reason for existing needs to differ (run compiled bytecode, not walk the AST). Since `interpreter/` is not modified in this milestone (Global Constraints), and each of these functions is short (15-45 lines), `vm.rs` reimplements the same shape as a set of free functions/methods over `Vm`, reusing every already-`pub` piece of `dispatch.rs` and `builtins::dispatch_native` unchanged, and swapping only the final "invoke this `Rc<ast::MethodDecl>`" step for `Vm::invoke_dispatched` (bytecode) instead of `Interpreter::invoke_method` (AST-walk). `slot_selector_shape` (a 6-line pure function, private in `eval.rs`) is duplicated verbatim for the same reason.
- **`invoke_compiled` must catch a `Signal::Return` targeting its own frame, exactly like `Interpreter::invoke_method` does.** This is not optional plumbing: a *non-inlined* block invoked from inside a compiled method (e.g. `coll do: [:e | e = x ifTrue: [^'found']]`, where `do:` is `builtins::collections`' native primitive) still runs that block's body through the unmodified, AST-walking `invoke_closure` (Task 1's `MakeClosure`/Task 7 design notes explain why). If that block's `^` targets the *bytecode* frame currently executing (its `home`), `invoke_closure`'s `raise_return_pub` produces `Signal::Return(target, value)` with `target` pointing at that same bytecode `Frame` — which then propagates up through `builtins::dispatch_native` -> `Vm`'s own dispatch orchestration -> the `?` on `self.send(..)` inside `run_frame`'s `SendPublic` handling, and must be caught at exactly the activation it targets, the same way `Interpreter::invoke_method` already does for the tree-walker. Getting this wrong would silently break every `do:`/`ifTrue:ifFalse:`/`and:`/`or:` idiom containing a `^` under `--engine=bytecode` — exactly the kind of idiom `blocks_and_return.ns` (an existing Milestone 1 fixture) already exercises.
- **Register addressing** (`reg_get`/`reg_set`) reproduces `interpreter::eval::read_local`/`write_local`'s exact args-then-temps split (private there, so duplicated here) — see Task 1's design note: register `r < frame.args.len()` means `frame.args[r]`, else `frame.temps[r - frame.args.len()]`.
- **`Vm::invoke_compiled` needs the original `Rc<ast::MethodDecl>`, not just its `Rc<CompiledMethod>`**, purely to fill `Frame.kind: FrameKind::Method(_)` — every `dispatch::Lookup::Method(method, defining_class)` result already hands back that exact `Rc`, so no extra threading is needed; `FrameKind::Method`'s inner value is never actually read anywhere in the crate (only pattern-matched), so this is a formality, not new plumbing.
- `Instr::MakeClosure`/`Instr::MakeArray` are `todo!("Task 7")` stubs in this task's `run_frame` — the same incremental-stub convention used throughout this plan.

- [ ] **Step 1: Write the failing tests**

`crates/ns-runtime/tests/vm.rs`:

```rust
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{compiler, vm::Vm, Interpreter, SourceOrigin, Symbol, Value};

fn setup(src: &str) -> (Interpreter, Vm) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns")))
        .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    let compiled = compiler::compile_module(&class, &bindings);
    (interp, Vm::new(compiled, bindings))
}

#[test]
fn an_ordinary_send_to_a_native_method_works() {
    let (mut interp, mut vm) = setup("Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()");
    let result = vm
        .send(&mut interp, Value::SmallInt(3), Symbol::intern("+"), vec![Value::SmallInt(4)], None)
        .expect("send");
    assert!(matches!(result, Value::SmallInt(7)));
}

#[test]
fn a_user_defined_method_dispatches_through_compiled_bytecode() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  answer = ( ^42 )\n) : ()";
    let (mut interp, mut vm) = setup(src);
    let instance = vm
        .send(&mut interp, Value::Obj(interp_class_ref(&mut interp, src)), Symbol::intern("new"), vec![], None)
        .expect("instantiate");
    let result = vm.send(&mut interp, instance, Symbol::intern("answer"), vec![], None).expect("send");
    assert!(matches!(result, Value::SmallInt(42)));
}

// Helper kept separate from `setup` because the test above needs the class
// object itself (to send it `new`), not just a `Vm` -- re-derives it the
// same way `setup` does rather than changing `setup`'s return shape for
// every other test.
fn interp_class_ref(interp: &mut Interpreter, src: &str) -> ns_runtime::ClassRef {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved =
        ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns"))).unwrap();
    interp.load_module(&resolved)
}

#[test]
fn an_implicit_self_send_reaches_a_sibling_method() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  helper = ( ^42 )\n  foo = ( ^helper )\n) : ()";
    let (mut interp, mut vm) = setup(src);
    let result = vm.send(&mut interp, Value::Nil, Symbol::intern("foo"), vec![], None);
    // `Value::Nil`'s class is BuiltinClass::Nil, which has no `foo` -- this
    // exercises DNU instead. Use a real instance for the positive case:
    let _ = result;
    let class = interp_class_ref(&mut interp, src);
    let instance = vm.send(&mut interp, Value::Obj(class), Symbol::intern("new"), vec![], None).unwrap();
    let result = vm.send(&mut interp, instance, Symbol::intern("foo"), vec![], None).expect("send");
    assert!(matches!(result, Value::SmallInt(42)));
}

#[test]
fn an_unknown_selector_produces_does_not_understand() {
    let (mut interp, mut vm) = setup("Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()");
    let err = vm
        .send(&mut interp, Value::SmallInt(3), Symbol::intern("nonexistentSelector"), vec![], None)
        .expect_err("should DNU");
    match err {
        ns_runtime::Signal::Error(e) => assert!(e.message.contains("doesNotUnderstand")),
        _ => panic!("expected a NewspeakError"),
    }
}

#[test]
fn a_super_send_reaches_the_superclass_implementation() {
    // Object doesn't override anything Milestone 1's boolean/number
    // builtins don't already provide directly, so this exercises the
    // super-send-with-no-user-superclass-method fallback-to-native path
    // instead -- `printString` is defined natively on Integer, not on
    // Object, so `super printString` from a method whose current_class is
    // a user class directly under Object exercises `lookup_defined_for`
    // finding nothing user-defined and falling through to `self.send`.
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^super printString )\n) : ()";
    let (mut interp, mut vm) = setup(src);
    let class = interp_class_ref(&mut interp, src);
    let instance = vm.send(&mut interp, Value::Obj(class), Symbol::intern("new"), vec![], None).unwrap();
    let result = vm.send(&mut interp, instance, Symbol::intern("foo"), vec![], None);
    assert!(result.is_ok(), "expected super send to fall through to Object's own doesNotUnderstand/printString path without panicking: {result:?}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test vm`
Expected: FAIL — `ns_runtime::vm` module not found.

- [ ] **Step 3: Write the implementation**

`crates/ns-runtime/src/vm.rs`:

```rust
//! The register-based bytecode VM: the run loop and the dispatch
//! orchestration it needs (send / doesNotUnderstand: / lexical-send /
//! super-send). See this task's design notes in the plan for why this
//! duplicates, rather than shares, `interpreter/mod.rs`'s and
//! `interpreter/eval.rs`'s orchestration shape.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::bytecode::Instr;
use crate::compiler::{self, CompiledMethodCache};
use crate::dispatch::{self, Lookup};
use crate::interpreter::{EvalResult, Interpreter, Signal};
use crate::types::*;
use crate::Symbol;

pub struct Vm {
    compiled: CompiledMethodCache,
    bindings: Rc<HashMap<Span, Binding>>,
}

impl Vm {
    pub fn new(compiled: CompiledMethodCache, bindings: Rc<HashMap<Span, Binding>>) -> Vm {
        Vm { compiled, bindings }
    }

    /// An ordinary (explicit-receiver) send -- mirrors
    /// `Interpreter::send`'s shape exactly (see this task's design note).
    pub fn send(
        &mut self,
        interp: &mut Interpreter,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        if let Some(result) = interp.try_factory_send(&receiver, selector, &args, &self.bindings) {
            return result;
        }
        let class = interp.class_of(&receiver);
        match dispatch::lookup_public(&class, selector) {
            Lookup::Method(method, defining_class) => {
                self.invoke_dispatched(interp, receiver, &method, defining_class, args, sender)
            }
            Lookup::Native(kind) => self.dispatch_native_or_dnu(interp, kind, receiver, selector, args, sender),
            Lookup::NotFound => self.does_not_understand(interp, receiver, selector, args, sender),
        }
    }

    fn dispatch_native_or_dnu(
        &mut self,
        interp: &mut Interpreter,
        kind: BuiltinClass,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        match self.dispatch_native_inheriting_object(interp, kind, &receiver, selector, &args, sender.as_ref()) {
            Some(result) => result,
            None => self.does_not_understand(interp, receiver, selector, args, sender),
        }
    }

    /// Mirrors `Interpreter::dispatch_native_inheriting_object` exactly
    /// (private there): a builtin's own native table first, then
    /// `Object`'s, since no non-`Object` builtin implements
    /// `Object`'s own selectors itself.
    fn dispatch_native_inheriting_object(
        &mut self,
        interp: &mut Interpreter,
        kind: BuiltinClass,
        receiver: &Value,
        selector: Symbol,
        args: &[Value],
        sender: Option<&FrameRef>,
    ) -> Option<EvalResult> {
        if let Some(result) =
            crate::builtins::dispatch_native(interp, kind, receiver.clone(), selector, args, sender)
        {
            return Some(result);
        }
        if kind == BuiltinClass::Object {
            return None;
        }
        crate::builtins::dispatch_native(interp, BuiltinClass::Object, receiver.clone(), selector, args, sender)
    }

    /// Mirrors `Interpreter::does_not_understand` exactly (private there).
    fn does_not_understand(
        &mut self,
        interp: &mut Interpreter,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let class = interp.class_of(&receiver);
        let dnu_selector = Symbol::intern("doesNotUnderstand:");
        match dispatch::lookup_defined_for(&class, dnu_selector) {
            Lookup::Method(method, defining_class) => {
                let selector_value = interp.new_symbol(selector);
                let args_value = Value::Obj(interp.new_array(args));
                let message_arg = Value::Obj(interp.new_array(vec![selector_value, args_value]));
                self.invoke_dispatched(interp, receiver, &method, defining_class, vec![message_arg], sender)
            }
            Lookup::Native(kind) => {
                let selector_value = interp.new_symbol(selector);
                let args_value = Value::Obj(interp.new_array(args));
                let message_arg = Value::Obj(interp.new_array(vec![selector_value, args_value]));
                match self.dispatch_native_inheriting_object(
                    interp, kind, &receiver, dnu_selector, &[message_arg], sender.as_ref(),
                ) {
                    Some(result) => result,
                    None => Err(Signal::Error(NewspeakError {
                        selector,
                        receiver,
                        message: format!(
                            "doesNotUnderstand: {selector} (native class {kind:?} has no doesNotUnderstand: implementation)"
                        ),
                    })),
                }
            }
            Lookup::NotFound => Err(Signal::Error(NewspeakError {
                selector,
                receiver,
                message: format!("doesNotUnderstand: {selector} (and no doesNotUnderstand: found anywhere in the class chain)"),
            })),
        }
    }

    /// `newspeak-spec.pdf` §5.7-§5.9. Mirrors `interpreter::eval::dispatch_lexical_send`
    /// exactly (private there).
    fn dispatch_lexical_send(
        &mut self,
        interp: &mut Interpreter,
        frame: &FrameRef,
        depth: u16,
        selector: Symbol,
        args: Vec<Value>,
    ) -> EvalResult {
        let self_value = frame.borrow().receiver.clone();
        let o = dispatch::kth_enclosing_object(&self_value, depth);
        let class = interp.class_of(&o);
        if let Some((name, is_setter)) = slot_selector_shape(selector) {
            if let Some(index) = dispatch::lookup_slot(&class, name) {
                if let Value::Obj(obj) = &o {
                    if is_setter {
                        let value = args.first().cloned().unwrap_or(Value::Nil);
                        if let ObjectData::Instance(fields) = &mut obj.borrow_mut().data {
                            if index < fields.len() {
                                fields[index] = value;
                            }
                        }
                        return Ok(o);
                    } else if let ObjectData::Instance(fields) = &obj.borrow().data {
                        if index < fields.len() {
                            return Ok(fields[index].clone());
                        }
                    }
                }
            }
        }
        if let Lookup::Method(method, defining_class) = dispatch::lookup_defined_for(&class, selector) {
            if dispatch::effective_access(method.access) == ast::AccessModifier::Private {
                return self.invoke_dispatched(interp, o, &method, defining_class, args, Some(frame.clone()));
            }
        }
        match dispatch::lookup_protected(&class, selector) {
            Lookup::Method(method, defining_class) => {
                self.invoke_dispatched(interp, o, &method, defining_class, args, Some(frame.clone()))
            }
            Lookup::Native(kind) => {
                match self.dispatch_native_inheriting_object(interp, kind, &o, selector, &args, Some(frame)) {
                    Some(result) => result,
                    None => self.send(interp, o, selector, args, Some(frame.clone())),
                }
            }
            Lookup::NotFound => self.send(interp, o, selector, args, Some(frame.clone())),
        }
    }

    /// `newspeak-spec.pdf` §5.10. Mirrors `interpreter::eval::dispatch_super_send`
    /// exactly (private there).
    fn dispatch_super_send(
        &mut self,
        interp: &mut Interpreter,
        frame: &FrameRef,
        selector: Symbol,
        args: Vec<Value>,
    ) -> EvalResult {
        let Some(current_class) = frame.borrow().current_class.clone() else {
            return Err(Signal::Error(NewspeakError {
                selector,
                receiver: frame.borrow().receiver.clone(),
                message: "super send with no enclosing current_class (used outside a method activation)".to_string(),
            }));
        };
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
            Lookup::Method(method, defining_class) => {
                self.invoke_dispatched(interp, self_value, &method, defining_class, args, Some(frame.clone()))
            }
            Lookup::Native(kind) => {
                match self.dispatch_native_inheriting_object(interp, kind, &self_value, selector, &args, Some(frame)) {
                    Some(result) => result,
                    None => self.send(interp, self_value, selector, args, Some(frame.clone())),
                }
            }
            Lookup::NotFound => self.send(interp, self_value, selector, args, Some(frame.clone())),
        }
    }

    /// Looks up `method`'s compiled form and invokes it. `panic!`s if none
    /// exists -- every method `dispatch.rs` can ever return via `Lookup::Method`
    /// was loaded by `Interpreter::load_module`/`load_class`, and
    /// `compile_module` compiles every method that loader loaded (Task 5's
    /// design note) -- a miss here is a `compile_module` coverage bug, not
    /// a bad Newspeak program.
    fn invoke_dispatched(
        &mut self,
        interp: &mut Interpreter,
        receiver: Value,
        method: &Rc<ast::MethodDecl>,
        defining_class: ClassRef,
        args: Vec<Value>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let compiled = self
            .compiled
            .get(&compiler::method_key(method))
            .cloned()
            .unwrap_or_else(|| {
                panic!("internal: dispatched method has no compiled form (compile_module coverage gap)")
            });
        self.invoke_compiled(interp, receiver, method, &compiled, defining_class, args, sender)
    }

    /// Builds a `Frame`, runs `compiled` via `run_frame`, and -- exactly
    /// like `Interpreter::invoke_method` -- catches a `Signal::Return`
    /// targeting this activation (see this task's design note on why a
    /// non-inlined block's `^` can still reach here) and marks the frame
    /// `uncontinuable` on every exit path.
    fn invoke_compiled(
        &mut self,
        interp: &mut Interpreter,
        receiver: Value,
        method: &Rc<ast::MethodDecl>,
        compiled: &Rc<crate::bytecode::CompiledMethod>,
        defining_class: ClassRef,
        args: Vec<Value>,
        sender: Option<FrameRef>,
    ) -> EvalResult {
        let temp_count = (compiled.num_registers - compiled.num_args) as usize;
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
        let result = self.run_frame(interp, &frame, compiled);
        frame.borrow_mut().uncontinuable = true;
        match result {
            Ok(value) => Ok(value),
            Err(Signal::Return(target, value)) if Rc::ptr_eq(&target, &frame) => Ok(value),
            Err(other) => Err(other),
        }
    }

    /// The bytecode dispatch loop itself.
    fn run_frame(
        &mut self,
        interp: &mut Interpreter,
        frame: &FrameRef,
        compiled: &Rc<crate::bytecode::CompiledMethod>,
    ) -> EvalResult {
        let mut pc = 0usize;
        loop {
            let instr = &compiled.instrs[pc];
            frame.borrow_mut().pc = pc;
            match instr {
                Instr::LoadLiteral { dst, literal } => {
                    let v = make_value(interp, literal);
                    reg_set(frame, *dst, v);
                }
                Instr::LoadSelf { dst } => {
                    let v = frame.borrow().receiver.clone();
                    reg_set(frame, *dst, v);
                }
                Instr::Move { dst, src } => {
                    let v = reg_get(frame, *src);
                    reg_set(frame, *dst, v);
                }
                Instr::LoadLocal { dst, slot } => {
                    let v = reg_get(frame, *slot);
                    reg_set(frame, *dst, v);
                }
                Instr::StoreLocal { src, slot } => {
                    let v = reg_get(frame, *src);
                    reg_set(frame, *slot, v);
                }
                Instr::LoadOuterLocal { dst, depth, index } => {
                    let v = reg_get(&frame_at_depth(frame, *depth), *index);
                    reg_set(frame, *dst, v);
                }
                Instr::StoreOuterLocal { src, depth, index } => {
                    let v = reg_get(frame, *src);
                    reg_set(&frame_at_depth(frame, *depth), *index, v);
                }
                Instr::LoadBuiltinClass { dst, kind } => {
                    let v = Value::Obj(interp.builtin_class(*kind));
                    reg_set(frame, *dst, v);
                }
                Instr::SendPublic { selector, receiver_reg, arg_regs, dst, .. } => {
                    let receiver = reg_get(frame, *receiver_reg);
                    let args: Vec<Value> = arg_regs.iter().map(|r| reg_get(frame, *r)).collect();
                    let result = self.send(interp, receiver, *selector, args, Some(frame.clone()))?;
                    reg_set(frame, *dst, result);
                }
                Instr::SendLexical { depth, selector, arg_regs, dst, .. } => {
                    let args: Vec<Value> = arg_regs.iter().map(|r| reg_get(frame, *r)).collect();
                    let result = self.dispatch_lexical_send(interp, frame, *depth, *selector, args)?;
                    reg_set(frame, *dst, result);
                }
                Instr::SendSuper { selector, arg_regs, dst, .. } => {
                    let args: Vec<Value> = arg_regs.iter().map(|r| reg_get(frame, *r)).collect();
                    let result = self.dispatch_super_send(interp, frame, *selector, args)?;
                    reg_set(frame, *dst, result);
                }
                Instr::MakeClosure { .. } => todo!("Task 7"),
                Instr::MakeArray { .. } => todo!("Task 7"),
                Instr::Return { reg } | Instr::ImplicitReturn { reg } => {
                    return Ok(reg_get(frame, *reg));
                }
            }
            pc += 1;
            debug_assert!(
                pc < compiled.instrs.len(),
                "internal: pc ran past the end of a CompiledMethod with no terminating Return/ImplicitReturn"
            );
        }
    }
}

/// `id:: e` / `id:` shape recognition -- mirrors `interpreter::eval::slot_selector_shape`
/// exactly (private there).
fn slot_selector_shape(selector: Symbol) -> Option<(Symbol, bool)> {
    let s = selector.as_str();
    if !s.contains(':') {
        return Some((selector, false));
    }
    if s.ends_with(':') && s.matches(':').count() == 1 {
        return Some((Symbol::intern(&s[..s.len() - 1]), true));
    }
    None
}

/// Register addressing -- mirrors `interpreter::eval::read_local`/`write_local`
/// exactly (private there): register `r < args.len()` is `args[r]`, else
/// `temps[r - args.len()]`. See Task 1's design note.
fn reg_get(frame: &FrameRef, r: u16) -> Value {
    let borrowed = frame.borrow();
    let i = r as usize;
    if i < borrowed.args.len() {
        borrowed.args[i].clone()
    } else {
        borrowed.temps[i - borrowed.args.len()].clone()
    }
}

fn reg_set(frame: &FrameRef, r: u16, value: Value) {
    let mut borrowed = frame.borrow_mut();
    let i = r as usize;
    if i < borrowed.args.len() {
        borrowed.args[i] = value;
    } else {
        let t = i - borrowed.args.len();
        borrowed.temps[t] = value;
    }
}

/// Mirrors `interpreter::eval::frame_at_depth` exactly (private there).
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

fn make_value(interp: &mut Interpreter, literal: &crate::bytecode::Literal) -> Value {
    use crate::bytecode::Literal as L;
    match literal {
        L::Nil => Value::Nil,
        L::Bool(b) => Value::Bool(*b),
        L::SmallInt(n) => Value::SmallInt(*n),
        L::Float(f) => Value::Float(*f),
        L::Str(s) => Value::Obj(interp.new_string(s.clone())),
        L::Sym(s) => interp.new_symbol(*s),
        L::Char(c) => Value::Obj(interp.new_character(*c)),
        L::Array(elements) => {
            let values: Vec<Value> = elements.iter().map(|e| make_value(interp, e)).collect();
            Value::Obj(interp.new_array(values))
        }
    }
}
```

- [ ] **Step 4: Wire into `lib.rs`**

Add `pub mod vm;` to `crates/ns-runtime/src/lib.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test vm`
Expected: PASS (5 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): implement the register VM's dispatch orchestration

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 7: `vm.rs` — closures and arrays

**Files:**
- Modify: `crates/ns-runtime/src/vm.rs`
- Modify: `crates/ns-runtime/tests/vm.rs`

**Interfaces:**
- Consumes: `Interpreter::new_closure` (already `pub`), `ClosureData` (unchanged).
- Produces: `run_frame`'s `MakeClosure`/`MakeArray` arms — no more `todo!()` stubs remain anywhere in `vm.rs`.

**Design notes:**
- **A VM-run `Frame`'s `kind` is always `FrameKind::Method(_)`** — unlike `eval_block_literal`, which must also handle `FrameKind::TopLevel`/`FrameKind::Closure` because the tree-walker itself constructs those. `Vm::invoke_compiled` (Task 6) only ever builds `FrameKind::Method` frames, so `MakeClosure`'s `home` computation has no other case to consider: `home` is simply the current frame, always. (A future milestone that starts running block bodies as their own bytecode activations — the Non-Goal below — would be the first thing to need a `FrameKind::Closure` case here.)
- **Non-Goal, restated precisely now that the machinery exists to say it exactly:** this milestone does not compile a block's *body* to bytecode, and does not give the VM its own closure-invocation path. Every closure this milestone's bytecode ever creates is invoked the same way Milestone 1 already invokes one — `builtins::call_block` -> `interpreter::eval::invoke_closure`, walking `ClosureData.body: Rc<ast::CodeBody>` — for both engines. A later milestone that wants block bodies to run as bytecode too (likely alongside Milestone 3's inline caching, once a block passed to a hot native primitive is worth compiling) would need: a real `Vm::invoke_closure_compiled` path, a `FrameKind::Closure` case in this task's `home` computation, and — since a block's `^` would then sometimes need to unwind past its own bytecode activation to a *different* one — a `NonLocalReturn` opcode after all. None of that is needed while every closure invocation goes through the shared tree-walking path.
- The test added for the `Signal::Return`-catching behavior Task 6 built (a non-inlined block's `^` unwinding out of a native `do:`/`ifTrue:ifFalse:` call back into the bytecode frame that owns it) lands in this task rather than Task 6, because it needs `MakeClosure` to actually work first — but the catching logic itself was already written and explained in Task 6.

- [ ] **Step 1: Write the failing tests**

Add to `crates/ns-runtime/tests/vm.rs`:

```rust
#[test]
fn a_compiled_method_returning_a_block_can_be_invoked_natively() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  makeBlock = ( ^[ :x | x + 1 ] )\n) : ()";
    let (mut interp, mut vm) = setup(src);
    let class = interp_class_ref(&mut interp, src);
    let instance = vm.send(&mut interp, Value::Obj(class), Symbol::intern("new"), vec![], None).unwrap();
    let block = vm.send(&mut interp, instance, Symbol::intern("makeBlock"), vec![], None).expect("send");
    let result = vm
        .send(&mut interp, block, Symbol::intern("value:"), vec![Value::SmallInt(9)], None)
        .expect("send");
    assert!(matches!(result, Value::SmallInt(10)));
}

#[test]
fn if_true_if_false_dispatches_through_native_boolean_and_a_tree_walked_block() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: b = ( ^b ifTrue: ['yes'] ifFalse: ['no'] )\n) : ()";
    let (mut interp, mut vm) = setup(src);
    let class = interp_class_ref(&mut interp, src);
    let instance = vm.send(&mut interp, Value::Obj(class), Symbol::intern("new"), vec![], None).unwrap();
    let result = vm
        .send(&mut interp, instance, Symbol::intern("foo:"), vec![Value::Bool(true)], None)
        .expect("send");
    let Value::Obj(obj) = result else { panic!("expected a string object") };
    assert!(matches!(&obj.borrow().data, ns_runtime::ObjectData::Str(s) if s == "yes"));
}

#[test]
fn a_dynamic_array_literal_executes_via_make_array() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^{ 1. 2. 3 } )\n) : ()";
    let (mut interp, mut vm) = setup(src);
    let class = interp_class_ref(&mut interp, src);
    let instance = vm.send(&mut interp, Value::Obj(class), Symbol::intern("new"), vec![], None).unwrap();
    let result = vm.send(&mut interp, instance, Symbol::intern("foo"), vec![], None).expect("send");
    let Value::Obj(obj) = result else { panic!() };
    match &obj.borrow().data {
        ns_runtime::ObjectData::Array(values) => assert_eq!(values.len(), 3),
        _ => panic!("expected an array"),
    }
}

#[test]
fn a_non_local_return_from_a_natively_invoked_block_unwinds_to_the_bytecode_frame() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  find: target in: coll = ( coll do: [:each | each = target ifTrue: [^'found']]. ^'not found' )\n  public main = ( | c | c := OrderedCollection new. c add: 1; add: 2; add: 3. ^self find: 2 in: c )\n) : ()";
    let (mut interp, mut vm) = setup(src);
    let class = interp_class_ref(&mut interp, src);
    let instance = vm.send(&mut interp, Value::Obj(class), Symbol::intern("new"), vec![], None).unwrap();
    let result = vm.send(&mut interp, instance, Symbol::intern("main"), vec![], None).expect("send");
    let Value::Obj(obj) = result else { panic!() };
    assert!(matches!(&obj.borrow().data, ns_runtime::ObjectData::Str(s) if s == "found"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test vm`
Expected: FAIL — `todo!("Task 7")` panics.

- [ ] **Step 3: Write the implementation**

Replace `run_frame`'s two `todo!()` arms in `crates/ns-runtime/src/vm.rs`:

```rust
Instr::MakeClosure { dst, params, body } => {
    // Every VM-run frame is `FrameKind::Method` (this task's design
    // note) -- `home` is always the current frame.
    let closure = Rc::new(ClosureData {
        params: (**params).clone(),
        body: body.clone(),
        captured_frame: frame.clone(),
        home: frame.clone(),
        bindings: self.bindings.clone(),
    });
    let v = Value::Obj(interp.new_closure(closure));
    reg_set(frame, *dst, v);
}
Instr::MakeArray { dst, elem_regs } => {
    let values: Vec<Value> = elem_regs.iter().map(|r| reg_get(frame, *r)).collect();
    let v = Value::Obj(interp.new_array(values));
    reg_set(frame, *dst, v);
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test vm`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): execute MakeClosure and MakeArray in the register VM

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 8: `ns-cli` — `--engine` selection on `run`

**Files:**
- Modify: `crates/ns-cli/src/main.rs`
- Modify: `crates/ns-cli/tests/run_fixtures.rs`

**Interfaces:**
- Consumes: `ns_runtime::{compiler::compile_module, vm::Vm}` (Tasks 5-7).
- Produces: `ns-next run <file> [--send selector] [--engine=bytecode|tree-walk]`, `--engine` defaulting to `bytecode`.

**Design notes:**
- Instantiation (`interp.instantiate(..)`) is unchanged and identical for both engine values — only the *entry send* after instantiation differs (this plan's scope decision, stated in the plan's own header: instance-initializer code is never compiled, only method bodies reached by a send).

- [ ] **Step 1: Write the failing test**

Add to `crates/ns-cli/tests/run_fixtures.rs`:

```rust
#[test]
fn the_bytecode_engine_produces_the_same_output_as_the_default_for_hello() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let path = dir.join("hello.ns");
    let expected = std::fs::read_to_string(dir.join("hello.expected")).expect("expected file");
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(&path)
        .arg("--engine")
        .arg("bytecode")
        .output()
        .expect("run ns-next");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(normalize_newlines(stdout.trim_end()), normalize_newlines(expected.trim_end()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ns-cli the_bytecode_engine`
Expected: FAIL — `--engine` is not a recognized argument.

- [ ] **Step 3: Write the implementation**

In `crates/ns-cli/src/main.rs`, add an `Engine` enum and thread it through `Cmd::Run` and `cmd_run`:

```rust
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
enum Engine {
    Bytecode,
    TreeWalk,
}
```

(`rename_all = "kebab-case"` makes `TreeWalk` accepted as `--engine tree-walk` on the command line — pinned explicitly rather than relying on clap's default, since the differential harness in Task 9 depends on exactly this spelling.)

Modify the `Run` variant of `Cmd`:

```rust
Run {
    file: PathBuf,
    #[arg(long, default_value = "main:args:")]
    send: String,
    #[arg(long, value_enum, default_value_t = Engine::Bytecode)]
    engine: Engine,
},
```

(`Engine` needs `#[derive(Clone, Copy, clap::ValueEnum)]` and, for `default_value_t`, also `Debug` — add `Debug` to the derive list above.)

Update `main`'s dispatch and `cmd_run`:

```rust
Cmd::Run { file, send, engine } => cmd_run(&file, &send, engine),
```

```rust
fn cmd_run(file: &Path, send: &str, engine: Engine) -> ExitCode {
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: reading {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let module = match ns_syntax::parse(&source) {
        Ok(m) => m,
        Err(diags) => {
            print_diagnostics(file, &source, &diags);
            return ExitCode::FAILURE;
        }
    };
    let resolved = match ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File(file.to_path_buf()),
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
    let instance = match interp.instantiate(class.clone(), vec![platform.clone()], &bindings) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    let selector = ns_runtime::Symbol::intern(send);
    let args = vec![platform, ns_runtime::Value::Nil];
    let result = match engine {
        Engine::TreeWalk => interp.send(instance, selector, args, &bindings, None),
        Engine::Bytecode => {
            let compiled = ns_runtime::compiler::compile_module(&class, &bindings);
            let mut vm = ns_runtime::vm::Vm::new(compiled, bindings.clone());
            vm.send(&mut interp, instance, selector, args, None)
        }
    };
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-cli`
Expected: PASS, including the new test and every existing `run_fixtures.rs` test (the pre-existing tests never pass `--engine`, so they exercise the new default, `bytecode`, end to end for the first time).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(cli): add --engine to ns-next run, defaulting to bytecode

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 9: Differential test harness

**Files:**
- Create: `crates/ns-cli/tests/differential.rs`

**Interfaces:**
- Consumes: the `ns-next` binary via `Command` (same pattern as the existing `run_fixtures.rs`).
- Produces: a regression net every later task's `cargo test` run already exercises — no new production code in this task.

**Design notes:**
- Runs every `tests-fixtures/*.ns` file under both `--engine=bytecode` and `--engine=tree-walk` and asserts identical exit status and stdout. This works unmodified for every *existing* Milestone 1 fixture today: none of them uses a selector this milestone's compiler doesn't yet support inlining (control-flow inlining, Task 10, is a pure optimization of already-correct behavior — `ifTrue:ifFalse:`/`and:`/`or:` already dispatch identically through native `boolean.rs` either way, inlined or not), so this harness is meaningful and green as soon as Task 8 lands, before Task 10 exists.
- Deliberately **does not** yet cover `whileTrue:`/`whileFalse:` (no Milestone 1 fixture uses them, and Milestone 1 has no native implementation of them at all — `grep -rn whileTrue crates/ns-runtime/src` finds nothing). Task 10 adds fixtures for exactly this case, with a differential assertion scoped to what's actually comparable (see Task 10's design note on why a successful `whileTrue:` loop is a bytecode-engine-only capability in this milestone, not a differential-testable one).

- [ ] **Step 1: Write the test**

`crates/ns-cli/tests/differential.rs`:

```rust
use std::path::Path;
use std::process::Command;

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn run_with_engine(path: &Path, engine: &str) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(path)
        .arg("--engine")
        .arg(engine)
        .output()
        .expect("run ns-next");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn every_tests_fixtures_ns_file_behaves_identically_under_both_engines() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        // Skip fixtures Task 10 adds specifically to exercise a
        // bytecode-only capability or a fallback-only comparison -- see
        // Task 10's design note.
        let name = path.file_name().unwrap().to_string_lossy();
        if name == "while_loop.ns" {
            continue;
        }
        let (bc_ok, bc_out, bc_err) = run_with_engine(&path, "bytecode");
        let (tw_ok, tw_out, tw_err) = run_with_engine(&path, "tree-walk");
        assert_eq!(
            bc_ok, tw_ok,
            "{}: exit status differs (bytecode stderr: {bc_err}, tree-walk stderr: {tw_err})",
            path.display()
        );
        assert_eq!(
            normalize_newlines(&bc_out),
            normalize_newlines(&tw_out),
            "{} produced different stdout under the two engines",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 3, "expected at least the 3 Milestone 1 fixtures");
}
```

- [ ] **Step 2: Run to verify it passes**

Run: `cargo test -p ns-cli --test differential`
Expected: PASS (1 test, checking every existing fixture).

- [ ] **Step 3: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "test(cli): add a bytecode-vs-tree-walk differential test harness

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 10: Control-flow inlining

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (add `Jump`, `JumpIfNotBoolean`, `BranchIfFalse`, `BranchIfTrue`)
- Modify: `crates/ns-runtime/src/compiler/control_flow.rs`
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (rename `emit_cascade_message` -> `emit_ordinary_send_with_receiver`; wire inlining into `lower_send`)
- Create: `tests-fixtures/while_loop.ns` / `.expected`
- Create: `tests-fixtures/control_flow_fallback.ns` / `.expected`
- Modify: `crates/ns-cli/tests/differential.rs` (add `control_flow_fallback.ns` to the compared set — it already is, since it isn't named `while_loop.ns`; no change needed beyond adding the fixture files)

**Interfaces:**
- Consumes: `Lowerer` (Tasks 2-4), `ast::{Send, BlockExpr}`.
- Produces: `control_flow::recognize(send: &ast::Send) -> Option<InlineForm<'_>>` (pure, no `Lowerer` access — see design note); `Lowerer`'s `lower_send` now inlines every recognized form.

**Design notes:**
- **Two new opcodes beyond the design spec's `BranchIfFalse`/`Jump`: `JumpIfNotBoolean` and `BranchIfTrue`.** `JumpIfNotBoolean` is the compile-time gate's runtime counterpart — the guard the spec's Control-flow inlining section requires ("is the receiver's class exactly `True`/`False`?"), checked as "is this register's value a `Bool` at all?" (not-a-bool is exactly the guard-failure case, whichever concrete class it turns out to be). `BranchIfTrue` is `BranchIfFalse`'s symmetric counterpart, needed for `whileFalse:`'s "exit when the condition becomes true" — synthesizing that from `BranchIfFalse` alone would need an extra unconditional jump and be harder to read for no real benefit; two small, obviously-symmetric opcodes are simpler than one clever one.
- **`control_flow::recognize` is a pure function with no `Lowerer` access, by design.** `Lowerer`'s helper methods (`alloc_scratch`, `emit`, `lower_expr`, ...) are private to `lower.rs`'s module (Global Constraints: nothing in this module is `pub(crate)` beyond what Tasks 2-5 already exposed for cross-file use, and there's no reason to widen that just for this task) — `control_flow.rs` only ever needs to answer "does this `Send` match an inlinable shape, and if so, which blocks does it carry", never to emit anything itself. All bytecode emission for inlined forms lives in `lower.rs`, as new methods on `Lowerer`, which call into `control_flow::recognize`.
- **The compile-time gate requires every block-position argument (and, for `whileTrue:`/`whileFalse:`, the receiver too) to be a *syntactic* zero-parameter block literal (`ast::Expr::Block` with `params.is_empty()`) directly at the call site.** A variable holding a block, or any other expression, fails the gate entirely — the compiler falls back to an ordinary send with **no** guard machinery at all (this is a compile-time decision, not a runtime one; see the design spec's Control-flow inlining section).
- **One shared runtime shape underlies every inlined form**, implemented once as `Lowerer::emit_guarded_branch` and reused by `lower_if`/`lower_and_or`: evaluate a guard register, `JumpIfNotBoolean` to a fallback block, `BranchIfFalse` to a false-block, run a true-block, jump to the end; false-block runs, jumps to the end; fallback-block runs an ordinary send with the *already-evaluated* receiver register (`emit_ordinary_send_with_receiver` — Task 3's `emit_cascade_message`, renamed here since it's no longer cascade-specific) and the block arguments re-lowered as real closures (an ordinary `lower_expr` on each `ast::Expr::Block` argument, which naturally produces `MakeClosure` — see Task 1/4's design notes on why that's the correct, and only, way those blocks are ever invoked). `lower_while` uses the same guard-and-fallback shape but as a loop body with a back-edge instead of a one-shot branch — see its own step below.
- **`ifTrue:ifFalse:`'s "true"/"false" branch bodies are lowered *inline*, directly into the enclosing method's own instruction stream** (`Lowerer::lower_inline_block_body`) — no `MakeClosure`, no new activation, exactly the mechanism Task 4's design note anticipated. A `^` inside one of these inlined bodies is therefore just an ordinary `Instr::Return`, correctly targeting the one, single, already-open method activation, however deeply such inlined forms nest.
- **`and:`/`or:`'s block-result coercion** (`Lowerer::emit_coerce_to_bool`) reproduces `builtins::boolean::truthy_arg`'s exact rule — only a block result of *exactly* `Value::Bool(true)` counts as true; anything else (a non-`true` `Bool`, or a non-`Bool` value entirely) coerces to `false` — using the same `JumpIfNotBoolean`/`BranchIfFalse` primitives, not a new opcode.
- **`whileTrue:`/`whileFalse:` and `ifFalse:ifTrue:` have no Milestone 1 native implementation to preserve fidelity with** (`grep -rn "whileTrue\|ifFalse:ifTrue:" crates/ns-runtime/src` finds nothing) — inlining is the *only* way these selectors ever do anything useful in `ns-next` at all. Their guard-failure fallback is a perfectly ordinary send (materializing real closures, dispatching to `Block`'s native table, which has no such method) and therefore produces the same `doesNotUnderstand:` a hypothetical un-inlined send would — correct and consistent, just not differentially comparable against a working tree-walker path (Milestone 1 has none). `while_loop.ns` (this task's new fixture) is accordingly **excluded** from the differential harness's per-file loop (Task 9 already wired this exclusion in) and instead gets its own dedicated assertion in this task's CLI-level test. `control_flow_fallback.ns` (a non-Boolean receiver to `ifTrue:ifFalse:`) *is* differentially comparable — both engines already agree on plain `doesNotUnderstand:` behavior — and is added to the regular fixture set with no special-casing needed.
- A mid-loop guard failure (the condition block returns a non-`Bool` only on, say, the fifth iteration, having returned real booleans for the first four) falls back to a fresh ordinary `whileTrue:`/`whileFalse:` send at that point — which immediately `doesNotUnderstand:`s, discarding the four already-completed iterations' *loop-continuation* decision (their side effects already happened and are not undone, only "should there be a sixth iteration" is redecided by the fallback). This is an accepted Milestone 2 simplification for what a real Newspeak program would consider a already-badly-behaved condition block; not worth more machinery for a capability Milestone 1 didn't have at all.

- [ ] **Step 1: Add the four new opcodes**

In `crates/ns-runtime/src/bytecode.rs`, add to `Instr` (next to `MakeArray`):

```rust
/// Unconditional jump to the instruction at index `target`.
Jump { target: usize },
/// Jumps to `target` if `reg`'s value is not `Value::Bool(_)` at all --
/// the runtime half of control-flow inlining's compile-time gate.
JumpIfNotBoolean { reg: u16, target: usize },
/// Jumps to `target` if `reg`'s value is exactly `Value::Bool(false)`.
/// Never reached with a non-`Bool` operand in inlined control flow (a
/// `JumpIfNotBoolean` on the same register always precedes it), but its
/// own defined behavior for that case is "fall through" -- it never
/// itself decides true/false for a non-`Bool`.
BranchIfFalse { reg: u16, target: usize },
/// Symmetric to `BranchIfFalse`, for `whileFalse:`'s exit condition.
BranchIfTrue { reg: u16, target: usize },
```

- [ ] **Step 2: Write `control_flow::recognize` and its tests**

`crates/ns-runtime/src/compiler/control_flow.rs`:

```rust
//! Compile-time recognition of the standard Smalltalk control-flow
//! "special selectors" for inlining. Pure recognition only -- no bytecode
//! emission lives here; see `lower.rs`'s `lower_if`/`lower_and_or`/
//! `lower_while` and this plan's Task 10 design notes for why.

use ns_syntax::ast;

pub(crate) enum InlineForm<'a> {
    IfTrueIfFalse(&'a ast::BlockExpr, &'a ast::BlockExpr),
    IfTrue(&'a ast::BlockExpr),
    IfFalse(&'a ast::BlockExpr),
    And(&'a ast::BlockExpr),
    Or(&'a ast::BlockExpr),
    WhileTrue { cond: &'a ast::BlockExpr, body: &'a ast::BlockExpr },
    WhileFalse { cond: &'a ast::BlockExpr, body: &'a ast::BlockExpr },
}

fn as_zero_arg_block(expr: &ast::Expr) -> Option<&ast::BlockExpr> {
    match expr {
        ast::Expr::Block(b) if b.params.is_empty() => Some(b),
        _ => None,
    }
}

/// `send.receiver`'s own binding is irrelevant here -- the *compile-time*
/// gate only looks at the message shape and its argument expressions.
/// Whether the receiver ends up being an ordinary/lexical/super send is
/// `lower_send`'s concern (only the ordinary case is ever offered to this
/// function -- see its call site).
pub(crate) fn recognize(send: &ast::Send) -> Option<InlineForm<'_>> {
    let ast::Selector::Keyword(selector) = &send.message.selector else {
        return None;
    };
    match selector.as_str() {
        "ifTrue:ifFalse:" => Some(InlineForm::IfTrueIfFalse(
            as_zero_arg_block(&send.message.args[0])?,
            as_zero_arg_block(&send.message.args[1])?,
        )),
        "ifFalse:ifTrue:" => Some(InlineForm::IfTrueIfFalse(
            as_zero_arg_block(&send.message.args[1])?,
            as_zero_arg_block(&send.message.args[0])?,
        )),
        "ifTrue:" => Some(InlineForm::IfTrue(as_zero_arg_block(&send.message.args[0])?)),
        "ifFalse:" => Some(InlineForm::IfFalse(as_zero_arg_block(&send.message.args[0])?)),
        "and:" => Some(InlineForm::And(as_zero_arg_block(&send.message.args[0])?)),
        "or:" => Some(InlineForm::Or(as_zero_arg_block(&send.message.args[0])?)),
        "whileTrue:" => Some(InlineForm::WhileTrue {
            cond: as_zero_arg_block(&send.receiver)?,
            body: as_zero_arg_block(&send.message.args[0])?,
        }),
        "whileFalse:" => Some(InlineForm::WhileFalse {
            cond: as_zero_arg_block(&send.receiver)?,
            body: as_zero_arg_block(&send.message.args[0])?,
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
        let send = parse_send("x ifTrue: [1] ifFalse: [2]");
        assert!(matches!(recognize(&send), Some(InlineForm::IfTrueIfFalse(..))));
    }

    #[test]
    fn does_not_recognize_a_non_literal_block_argument() {
        let send = parse_send("x ifTrue: y ifFalse: [2]");
        assert!(recognize(&send).is_none());
    }

    #[test]
    fn does_not_recognize_a_block_with_parameters() {
        let send = parse_send("x ifTrue: [:p | p] ifFalse: [2]");
        assert!(recognize(&send).is_none());
    }

    #[test]
    fn recognizes_while_true_only_when_both_receiver_and_argument_are_block_literals() {
        assert!(matches!(
            recognize(&parse_send("[x] whileTrue: [y]")),
            Some(InlineForm::WhileTrue { .. })
        ));
        assert!(recognize(&parse_send("x whileTrue: [y]")).is_none());
    }
}
```

- [ ] **Step 3: Run to verify tests pass (this step needs no production code beyond Step 1's opcodes, already independent of `lower.rs`)**

Run: `cargo test -p ns-runtime control_flow::`
Expected: PASS (4 tests) — `recognize` has no dependency on anything Step 4 adds.

- [ ] **Step 4: Write the failing `lower.rs` tests**

Add to `crates/ns-runtime/src/compiler/lower.rs`'s test module:

```rust
fn compile_foo(src_body: &str) -> Vec<Instr> {
    let full = format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo: x = ( ^{src_body} )\n) : ()");
    let module = ns_syntax::parse(&full).expect("parse");
    let resolved = crate::resolve(
        module,
        Rc::from(full.as_str()),
        crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let foo = resolved.module.root_class.instance_side.methods[0].clone();
    let bindings = resolved.bindings.clone();
    let mut lowerer = Lowerer::new(1, 0, &bindings);
    let ast::Stmt::Return(expr) = &foo.body.stmts[0] else { panic!("return") };
    lowerer.lower_expr(expr);
    lowerer.finish().0
}

#[test]
fn if_true_if_false_with_literal_blocks_compiles_to_a_guarded_branch_with_no_send() {
    let instrs = compile_foo("x ifTrue: [1] ifFalse: [2]");
    assert!(
        instrs.iter().any(|i| matches!(i, Instr::JumpIfNotBoolean { .. })),
        "expected a JumpIfNotBoolean guard, got {instrs:#?}"
    );
    assert!(
        !instrs.iter().any(|i| matches!(i, Instr::SendPublic { .. })),
        "the fast path must never contain a send when the fallback is never taken at compile time, got {instrs:#?}"
    );
    assert!(instrs.iter().any(|i| matches!(i, Instr::LoadLiteral { literal: BcLiteral::SmallInt(1), .. })));
    assert!(instrs.iter().any(|i| matches!(i, Instr::LoadLiteral { literal: BcLiteral::SmallInt(2), .. })));
}

#[test]
fn if_true_if_false_with_a_non_literal_block_falls_back_to_an_ordinary_send() {
    let instrs = compile_foo("x ifTrue: x ifFalse: [2]");
    assert!(
        instrs.iter().any(|i| matches!(i, Instr::SendPublic { selector, .. } if selector.as_str() == "ifTrue:ifFalse:")),
        "expected an ordinary SendPublic for the non-inlinable shape, got {instrs:#?}"
    );
    assert!(!instrs.iter().any(|i| matches!(i, Instr::JumpIfNotBoolean { .. })));
}

#[test]
fn while_true_compiles_to_a_back_edge_jump() {
    let instrs = compile_foo("[x] whileTrue: [x]");
    let jump_targets: Vec<usize> = instrs
        .iter()
        .filter_map(|i| match i {
            Instr::Jump { target } => Some(*target),
            _ => None,
        })
        .collect();
    assert!(jump_targets.contains(&0), "expected a Jump back to the loop's start, got {instrs:#?}");
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p ns-runtime compiler::lower::`
Expected: FAIL — `lower_send`'s ordinary-send arm doesn't check `control_flow::recognize` yet.

- [ ] **Step 6: Write the implementation**

In `crates/ns-runtime/src/compiler/lower.rs`, rename `emit_cascade_message` to `emit_ordinary_send_with_receiver` (update its one call site in `lower_cascade` too — same body, no behavior change, just no longer cascade-specific in name), and add:

```rust
fn emit_nil(&mut self) -> u16 {
    let dst = self.alloc_scratch();
    self.emit(Instr::LoadLiteral { dst, literal: BcLiteral::Nil });
    dst
}

/// Lowers `block`'s own statements directly into the *current* method's
/// instruction stream -- no `MakeClosure`, no new activation (Task 4's
/// design note on why an inlined block's `^` is an ordinary `Return`).
/// Returns the register holding the block's implicit result value.
fn lower_inline_block_body(&mut self, block: &ast::BlockExpr) -> u16 {
    if block.body.stmts.is_empty() {
        return self.emit_nil();
    }
    for stmt in &block.body.stmts {
        self.lower_statement(stmt);
    }
    self.last_value_register().unwrap_or_else(|| self.emit_nil())
}

/// `builtins::boolean::truthy_arg`'s exact coercion rule, reproduced with
/// bytecode: only `Value::Bool(true)` coerces to `true`; a non-`true`
/// `Bool` or any non-`Bool` value coerces to `false`.
fn emit_coerce_to_bool(&mut self, src: u16) -> u16 {
    let dst = self.alloc_scratch();
    let not_bool_jump = self.emit(Instr::JumpIfNotBoolean { reg: src, target: 0 });
    let false_jump = self.emit(Instr::BranchIfFalse { reg: src, target: 0 });
    self.emit(Instr::LoadLiteral { dst, literal: BcLiteral::Bool(true) });
    let after_true_jump = self.emit(Instr::Jump { target: 0 });
    let false_label = self.instrs.len();
    self.instrs[not_bool_jump] = Instr::JumpIfNotBoolean { reg: src, target: false_label };
    self.instrs[false_jump] = Instr::BranchIfFalse { reg: src, target: false_label };
    self.emit(Instr::LoadLiteral { dst, literal: BcLiteral::Bool(false) });
    let end_label = self.instrs.len();
    self.instrs[after_true_jump] = Instr::Jump { target: end_label };
    dst
}

/// The shared shape behind every "evaluate a guard, do one of three
/// things" inlined form (`ifTrue:ifFalse:`/`ifTrue:`/`ifFalse:`/`and:`/`or:`).
/// `on_true`/`on_false` run when `guard_reg` is exactly `Bool(true)`/
/// `Bool(false)`; `on_fallback` runs otherwise. Each returns the register
/// holding that branch's value; the chosen one is moved into `dst`.
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
    let false_label = self.instrs.len();
    self.instrs[false_jump] = Instr::BranchIfFalse { reg: guard_reg, target: false_label };
    let false_result = on_false(self);
    self.emit(Instr::Move { dst, src: false_result });
    let after_false_jump = self.emit(Instr::Jump { target: 0 });
    let fallback_label = self.instrs.len();
    self.instrs[not_bool_jump] = Instr::JumpIfNotBoolean { reg: guard_reg, target: fallback_label };
    let fallback_result = on_fallback(self);
    self.emit(Instr::Move { dst, src: fallback_result });
    let end_label = self.instrs.len();
    self.instrs[after_true_jump] = Instr::Jump { target: end_label };
    self.instrs[after_false_jump] = Instr::Jump { target: end_label };
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
        |lw| true_block.map_or_else(|| lw.emit_nil(), |b| lw.lower_inline_block_body(b)),
        |lw| false_block.map_or_else(|| lw.emit_nil(), |b| lw.lower_inline_block_body(b)),
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
    if is_and {
        self.emit_guarded_branch(
            guard_reg,
            dst,
            evaluate_and_coerce,
            |_lw| guard_reg,
            |lw| lw.emit_ordinary_send_with_receiver(guard_reg, &send.message),
        );
    } else {
        self.emit_guarded_branch(
            guard_reg,
            dst,
            |_lw| guard_reg,
            evaluate_and_coerce,
            |lw| lw.emit_ordinary_send_with_receiver(guard_reg, &send.message),
        );
    }
    dst
}

/// `newspeak-spec.pdf` has no native `whileTrue:`/`whileFalse:` to fall
/// back to in Milestone 1 (this task's design note) -- the fallback here
/// materializes real closures for `cond`/`body` and sends the original
/// selector to `cond`'s own closure object, which `doesNotUnderstand:`s.
fn lower_while(&mut self, send: &ast::Send, cond: &ast::BlockExpr, body: &ast::BlockExpr, while_true: bool) -> u16 {
    let dst = self.emit_nil();
    let loop_start = self.instrs.len();
    let guard_reg = self.lower_inline_block_body(cond);
    let not_bool_jump = self.emit(Instr::JumpIfNotBoolean { reg: guard_reg, target: 0 });
    let exit_jump = if while_true {
        self.emit(Instr::BranchIfFalse { reg: guard_reg, target: 0 })
    } else {
        self.emit(Instr::BranchIfTrue { reg: guard_reg, target: 0 })
    };
    self.lower_inline_block_body(body);
    self.emit(Instr::Jump { target: loop_start });
    let fallback_label = self.instrs.len();
    self.instrs[not_bool_jump] = Instr::JumpIfNotBoolean { reg: guard_reg, target: fallback_label };
    let cond_closure = self.lower_expr(&send.receiver);
    let fallback_result = self.emit_ordinary_send_with_receiver(cond_closure, &send.message);
    self.emit(Instr::Move { dst, src: fallback_result });
    let end_label = self.instrs.len();
    match &mut self.instrs[exit_jump] {
        Instr::BranchIfFalse { target, .. } | Instr::BranchIfTrue { target, .. } => *target = end_label,
        _ => unreachable!("exit_jump is always one of the two arms above"),
    }
    self.emit(Instr::Jump { target: end_label }); // no-op landing pad kept for a uniform "every path reaches end_label" shape
    dst
}
```

Wire it into `lower_send`'s ordinary (`_`) arm — replace:

```rust
_ => {
    let receiver_reg = self.lower_expr(&send.receiver);
    let selector = selector_of(&send.message);
    let arg_regs = self.lower_args(&send.message);
    let dst = self.alloc_scratch();
    let call_site = self.next_call_site();
    self.emit(Instr::SendPublic { selector, receiver_reg, arg_regs, dst, call_site });
    self.record_call_site(call_site);
    dst
}
```

with:

```rust
_ => {
    if let Some(form) = control_flow::recognize(send) {
        return match form {
            control_flow::InlineForm::IfTrueIfFalse(t, f) => self.lower_if(send, Some(t), Some(f)),
            control_flow::InlineForm::IfTrue(t) => self.lower_if(send, Some(t), None),
            control_flow::InlineForm::IfFalse(f) => self.lower_if(send, None, Some(f)),
            control_flow::InlineForm::And(b) => self.lower_and_or(send, b, true),
            control_flow::InlineForm::Or(b) => self.lower_and_or(send, b, false),
            control_flow::InlineForm::WhileTrue { cond, body } => self.lower_while(send, cond, body, true),
            control_flow::InlineForm::WhileFalse { cond, body } => self.lower_while(send, cond, body, false),
        };
    }
    let receiver_reg = self.lower_expr(&send.receiver);
    self.emit_ordinary_send_with_receiver(receiver_reg, &send.message)
}
```

Add `use super::control_flow;` near the top of `lower.rs`.

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p ns-runtime compiler::`
Expected: PASS (every `control_flow::` and `lower::` test).

- [ ] **Step 8: Add the two new fixtures**

`tests-fixtures/while_loop.ns`:

```
Newspeak3
'Test'
class WhileLoop usingPlatform: platform = Object new (
	| n ::= 0. sum ::= 0. |
) (
	public main: p args: a = (
		[n < 5] whileTrue: [sum := sum + n. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`tests-fixtures/while_loop.expected`:

```
10
```

`tests-fixtures/control_flow_fallback.ns`:

```
Newspeak3
'Test'
class ControlFlowFallback usingPlatform: platform = Object new (
) (
	public main: p args: a = (
		p transcript show: (3 ifTrue: ['yes'] ifFalse: ['no']) printString; newLine
	)
) : ()
```

Run `ns-next run tests-fixtures/control_flow_fallback.ns --engine=tree-walk` once by hand to capture the exact uncaught-error text for `control_flow_fallback.expected` — both engines must already agree (Task 9's differential test enforces this), so whichever exact wording the tree-walker prints today is correct to check in. (Do not guess the string; the tree-walker predates this plan and its `doesNotUnderstand:` message format is exact, existing code — copy it verbatim from the command's actual output.) Since this fixture is expected to exit non-zero, the harness's existing `run_fixtures.rs`-style `assert!(output.status.success())` does not apply to it — add it only to `differential.rs`'s comparison (already covers both success and failure via `bc_ok == tw_ok`), not to `run_fixtures.rs`'s own fixture list.

- [ ] **Step 9: Add the `while_loop.ns` CLI-level assertion**

Add to `crates/ns-cli/tests/differential.rs`:

```rust
#[test]
fn while_loop_only_works_under_the_bytecode_engine() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let path = dir.join("while_loop.ns");
    let (bc_ok, bc_out, _bc_err) = run_with_engine(&path, "bytecode");
    assert!(bc_ok, "bytecode engine should run the while loop successfully");
    assert_eq!(normalize_newlines(bc_out.trim_end()), "10");
    let (tw_ok, _tw_out, tw_err) = run_with_engine(&path, "tree-walk");
    assert!(
        !tw_ok,
        "tree-walk has no whileTrue: implementation and must doesNotUnderstand:, got: {tw_err}"
    );
}
```

- [ ] **Step 10: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS, including the two new fixture-backed tests.

- [ ] **Step 11: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): inline ifTrue:/ifTrue:ifFalse:/and:/or:/whileTrue:/whileFalse:

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 11: `ns-next disasm`

**Files:**
- Modify: `crates/ns-cli/Cargo.toml` (add `insta` as a dev-dependency)
- Modify: `crates/ns-cli/src/main.rs`
- Create: `crates/ns-cli/tests/disasm_snapshots.rs`

**Interfaces:**
- Consumes: `compiler::compile_method` (Task 5, already takes a bare `&ast::MethodDecl` + bindings — no `Interpreter`/`ClassRef` needed at all, since disassembly is pure compilation with no instantiation).
- Produces: `ns-next disasm <file.ns> <selector>`.

**Design notes:**
- `disasm` never bootstraps an `Interpreter` or loads a class — `compile_method` only needs the parsed-and-resolved method AST plus the bindings map, so this command is parse -> resolve -> find the named method on the root class -> `compile_method` -> format. This is "close to free," per the design spec: no new compilation path, just a formatter over an already-existing `CompiledMethod`.
- The formatter lives in `ns-cli` (rendering, not logic), matching `main.rs`'s existing convention ("argument parsing and human-readable diagnostic rendering only").

- [ ] **Step 1: Add the `insta` dev-dependency**

In `crates/ns-cli/Cargo.toml`, add under a new `[dev-dependencies]` section:

```toml
[dev-dependencies]
insta.workspace = true
```

- [ ] **Step 2: Write the failing tests**

`crates/ns-cli/tests/disasm_snapshots.rs`:

```rust
use std::path::Path;
use std::process::Command;

fn disasm(file: &str, selector: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures").join(file);
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("disasm")
        .arg(&path)
        .arg(selector)
        .output()
        .expect("run ns-next disasm");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn disasm_arithmetic_main() {
    insta::assert_snapshot!(disasm("arithmetic.ns", "main:args:"));
}

#[test]
fn disasm_control_flow_fallback_main() {
    insta::assert_snapshot!(disasm("control_flow_fallback.ns", "main:args:"));
}

#[test]
fn disasm_while_loop_main() {
    insta::assert_snapshot!(disasm("while_loop.ns", "main:args:"));
}

#[test]
fn disasm_blocks_and_return_find_in() {
    insta::assert_snapshot!(disasm("blocks_and_return.ns", "find:in:"));
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p ns-cli --test disasm_snapshots`
Expected: FAIL — `disasm` is not a recognized subcommand.

- [ ] **Step 4: Write the implementation**

In `crates/ns-cli/src/main.rs`, add a `Disasm` variant to `Cmd`:

```rust
/// Compile one method and print its bytecode.
Disasm {
    file: PathBuf,
    selector: String,
},
```

Add to `main`'s dispatch:

```rust
Cmd::Disasm { file, selector } => cmd_disasm(&file, &selector),
```

Add the command and its formatter:

```rust
fn method_name(pattern: &ns_syntax::ast::MessagePattern) -> String {
    match pattern {
        ns_syntax::ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ns_syntax::ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ns_syntax::ast::MessagePattern::Keyword { parts } => {
            parts.iter().map(|(k, _)| format!("{}:", k.name)).collect()
        }
    }
}

fn format_compiled(compiled: &ns_runtime::bytecode::CompiledMethod) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (idx, instr) in compiled.instrs.iter().enumerate() {
        let stmt_label = compiled
            .statement_pc
            .iter()
            .find(|(_, range)| range.contains(&idx))
            .map(|(s, _)| format!("stmt {s}"))
            .unwrap_or_else(|| "  --  ".to_string());
        writeln!(out, "{idx:4}  {stmt_label:>8}  {instr:?}").unwrap();
    }
    out
}

fn cmd_disasm(file: &Path, selector: &str) -> ExitCode {
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: reading {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let module = match ns_syntax::parse(&source) {
        Ok(m) => m,
        Err(diags) => {
            print_diagnostics(file, &source, &diags);
            return ExitCode::FAILURE;
        }
    };
    let resolved = match ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File(file.to_path_buf()),
    ) {
        Ok(r) => r,
        Err(errors) => {
            for e in errors {
                eprintln!("resolve error: {}", e.message);
            }
            return ExitCode::FAILURE;
        }
    };
    let method = resolved
        .module
        .root_class
        .instance_side
        .methods
        .iter()
        .find(|m| method_name(&m.pattern) == selector);
    let Some(method) = method else {
        eprintln!("error: no method `{selector}` found on the root class");
        return ExitCode::FAILURE;
    };
    let compiled = ns_runtime::compiler::compile_method(method, &resolved.bindings);
    print!("{}", format_compiled(&compiled));
    ExitCode::SUCCESS
}
```

- [ ] **Step 5: Run to verify they fail for the right reason, then accept the baseline snapshots**

Run: `cargo test -p ns-cli --test disasm_snapshots`
Expected: fails with `.snap.new` files created (insta has no prior baseline). Review each printed diff, confirm it looks like sane bytecode for its fixture, then run `cargo insta accept` (or, without the `cargo-insta` subcommand installed, `INSTA_UPDATE=always cargo test -p ns-cli --test disasm_snapshots`) to write the accepted `.snap` files under `crates/ns-cli/tests/snapshots/`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p ns-cli --test disasm_snapshots`
Expected: PASS (4 tests) against the now-committed baseline snapshots.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(cli): add ns-next disasm

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 12: Image encoding primitives — header and scalar values

**Files:**
- Create: `crates/ns-runtime/src/image/mod.rs`
- Create: `crates/ns-runtime/src/image/encode.rs`
- Create: `crates/ns-runtime/src/image/decode.rs`
- Modify: `crates/ns-runtime/src/lib.rs`
- Create: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces: `Writer`/`Reader` byte-level helpers (`pub(crate)`, internal to `image/`), `DecodeError`, `write_header`/`read_header`, `encode_scalar_value`/`decode_scalar_value` (the four non-`Obj` `Value` variants only — object-graph encoding is Task 13).

**Design notes:**
- Hand-rolled, not `serde`/`bincode` (Global Constraints: no new dependency) — a small `Writer`/`Reader` pair over `Vec<u8>`/`&[u8]`, little-endian, length-prefixed strings/byte-blobs, one leading tag byte per sum-typed value (`Value`, later `ObjectData`/`Literal`). This is the design spec's "canonical encodings" section made concrete: little-endian integers, IEEE-754 `binary64` floats, UTF-8 strings.
- **Refuse on any version mismatch, per the design spec** — `read_header` returns `Err(DecodeError::VersionMismatch{..})` rather than attempting to migrate; there is exactly one version (`1`) in this milestone.

- [ ] **Step 1: Write the failing tests**

`crates/ns-runtime/tests/image.rs`:

```rust
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
    assert!(decode::read_header(&mut r).is_err());
}

#[test]
fn a_version_mismatch_is_rejected() {
    let mut w = encode::Writer::new();
    w.write_raw(encode::MAGIC);
    w.write_u32(9999);
    let bytes = w.into_bytes();
    let mut r = decode::Reader::new(&bytes);
    match decode::read_header(&mut r) {
        Err(decode::DecodeError::VersionMismatch { found: 9999, .. }) => {}
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
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
            (Value::Float(a), Value::Float(b)) => assert_eq!(a, b),
            _ => panic!("round-trip mismatch: {v:?} -> {decoded:?}"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test image`
Expected: FAIL — `ns_runtime::image` module not found.

- [ ] **Step 3: Write the implementation**

`crates/ns-runtime/src/image/encode.rs`:

```rust
//! Canonical, little-endian, self-describing byte encoding for the image
//! format. See `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`'s
//! Image format section.

use crate::types::Value;

pub const MAGIC: &[u8; 4] = b"NSIM";
pub const FORMAT_VERSION: u32 = 1;

pub struct Writer {
    buf: Vec<u8>,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
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

    pub fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Length-prefixed (u32 byte count, little-endian) raw bytes.
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

/// Encodes the four non-`Obj` `Value` variants; returns `false` for
/// `Value::Obj` (object-graph encoding is Task 13's `encode_object`, which
/// needs the root/special-objects table this task does not yet have).
pub fn encode_scalar_value(w: &mut Writer, v: &Value) -> bool {
    match v {
        Value::Nil => {
            w.write_u8(0);
            true
        }
        Value::Bool(false) => {
            w.write_u8(1);
            true
        }
        Value::Bool(true) => {
            w.write_u8(2);
            true
        }
        Value::SmallInt(n) => {
            w.write_u8(3);
            w.write_i64(*n);
            true
        }
        Value::Float(f) => {
            w.write_u8(4);
            w.write_f64(*f);
            true
        }
        Value::Obj(_) => false,
    }
}
```

`crates/ns-runtime/src/image/decode.rs`:

```rust
//! Canonical decoding, the inverse of `encode.rs`.

use crate::types::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    UnexpectedEof,
    BadMagic,
    VersionMismatch { found: u32, expected: u32 },
    InvalidTag(u8),
    InvalidUtf8,
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
        let end = self.pos + len;
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
        let bytes = self.read_bytes()?;
        String::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)
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
        tag => Err(DecodeError::InvalidTag(tag)),
    }
}
```

`crates/ns-runtime/src/image/mod.rs`:

```rust
//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`'s
//! Image format section.

pub mod decode;
pub mod encode;
```

- [ ] **Step 4: Wire into `lib.rs`**

Add `pub mod image;` to `crates/ns-runtime/src/lib.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test image`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): add the image format's header and scalar value encoding

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 13: Image object-graph encoding

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs`
- Modify: `crates/ns-runtime/src/image/decode.rs`
- Modify: `crates/ns-runtime/tests/image.rs`

**Interfaces:**
- Produces: `encode::encode_object_graph(roots: &[Value]) -> Result<Vec<u8>, String>`, `decode::decode_object_graph(r: &mut Reader, interp: &Interpreter, user_root_class: &ClassRef) -> Result<Vec<Value>, DecodeError>`.

**Design notes — read in full, this task made real scope decisions:**
- **A class object is never structurally serialized — it is resolved by identity via a two-case tag (`Builtin(BuiltinClass)` or `UserRoot`), not reconstructed field-by-field.** This follows directly from the M1 runtime spec's own "Source Retention and Provenance" section and the design spec's "bootstrap from source" principle: `Task 14`'s `image::mod.rs` stores the module's *source text*, and `load` re-runs the ordinary parse -> resolve -> `load_module` -> `compile_module` pipeline against it — producing a fresh, live `ClassRef` for the one root user class that every `UserRoot`-tagged reference in the object graph then resolves to, and a fresh, live builtin-class table that every `Builtin(kind)`-tagged reference resolves to via `Interpreter::builtin_class(kind)`. This sidesteps needing a generic `ast::MethodDecl`/`ClassData` serialization format entirely — a real simplification found while implementing, consistent with the M1 runtime spec's existing provenance design, not an ad hoc shortcut.
- **Only plain instances (`ObjectData::Instance`/`Str`/`Sym`/`Array`) go into the object table** that needs cycle-safe two-pass decoding; a class-kind `Object` is never added to it (`ObjectTable::visit` returns immediately for one). Milestone 1 only ever creates instances of the one loaded root class or a builtin, so an instance's own `.class` is always resolvable by the same two-case tag — there is no need for the object table to support "an instance of some *other*, non-root user class" in this milestone.
- **`ObjectData::Closure` is an explicit, error-returning Non-Goal for this task**, not silently mishandled: `encode_object_body` returns `Err("image encoding does not yet support live closures...")` if a live closure is reachable from the snapshot roots. Serializing a closure would need a defined encoding for `ClosureData.body: Rc<ast::CodeBody>` (a generic AST serialization format this milestone does not build — see this plan's header note on why instance-initializer/AST-level code is out of scope) and for its `captured_frame`/`home` `FrameRef`s. Consistent with this milestone's "snapshot a quiesced world" scope decision (design spec's Non-Goals): a closure captured in an instance variable and still reachable when `ns-next snapshot` runs is the one realistic way this could be hit today: a program storing a block in a slot for later reevaluation. Revisit once the Frame-chain/AST serialization a real debugger needs is built.
- **Cycles are supported via the standard two-pass "placeholder, then fill in" technique** — the exact same allocate-uninitialized/balance-the-strong-count pattern already used three times in this codebase (`types.rs`'s and `interpreter/mod.rs`'s own self-referential bootstrap helpers, both private to their modules) is duplicated a fourth time here, for the same "don't touch `interpreter/`" reason as everywhere else in this plan. Milestone 1's own bootstrap already relies on exactly this kind of cycle (`BuiltinClass::Object`'s class is itself) — decode must be able to reconstruct it, or resolve it via the `Builtin`/`UserRoot` tag before ever reaching the placeholder machinery, which is in fact what happens (a class-kind reference never touches the instance table at all — restated here because it's the reason cycles *through class objects* need no placeholder handling, only cycles *among plain instances* do).

- [ ] **Step 1: Write the failing tests**

Add to `crates/ns-runtime/tests/image.rs`:

```rust
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::types::ObjectData;
use ns_runtime::{ClassRef, Interpreter, SourceOrigin};

fn bootstrap_class(src: &str) -> (Interpreter, ClassRef) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(module, Rc::from(src), SourceOrigin::File(PathBuf::from("t.ns")))
        .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

#[test]
fn instances_and_arrays_round_trip_with_shared_identity() {
    let (mut interp, class) = bootstrap_class("Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()");
    let shared = Value::Obj(interp.new_string("shared".to_string()));
    let array = Value::Obj(interp.new_array(vec![shared.clone(), shared]));
    let roots = vec![array];
    let bytes = encode::encode_object_graph(&roots).expect("encode");
    let mut r = decode::Reader::new(&bytes);
    let decoded = decode::decode_object_graph(&mut r, &interp, &class).expect("decode");
    let Value::Obj(decoded_array) = &decoded[0] else { panic!("expected an array") };
    let ObjectData::Array(elements) = &decoded_array.borrow().data else { panic!("expected Array data") };
    let (Value::Obj(a), Value::Obj(b)) = (&elements[0], &elements[1]) else { panic!("expected two objects") };
    assert!(Rc::ptr_eq(a, b), "shared identity must be preserved across the round trip");
}

#[test]
fn identity_hashes_are_preserved() {
    let (mut interp, class) = bootstrap_class("Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()");
    let obj = interp.new_string("x".to_string());
    let hash = obj.borrow().identity_hash;
    let bytes = encode::encode_object_graph(&[Value::Obj(obj)]).unwrap();
    let mut r = decode::Reader::new(&bytes);
    let decoded = decode::decode_object_graph(&mut r, &interp, &class).unwrap();
    let Value::Obj(decoded_obj) = &decoded[0] else { panic!() };
    assert_eq!(decoded_obj.borrow().identity_hash, hash);
}

#[test]
fn builtin_class_references_round_trip_to_the_same_live_class_ref() {
    let (interp, class) = bootstrap_class("Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()");
    let roots = vec![Value::Obj(interp.builtin_class(ns_runtime::types::BuiltinClass::Integer))];
    let bytes = encode::encode_object_graph(&roots).unwrap();
    let mut r = decode::Reader::new(&bytes);
    let decoded = decode::decode_object_graph(&mut r, &interp, &class).unwrap();
    let (Value::Obj(a), Value::Obj(b)) = (&roots[0], &decoded[0]) else { panic!() };
    assert!(Rc::ptr_eq(a, b), "a builtin class reference re-resolves to the SAME live ClassRef, not a copy");
}

#[test]
fn a_live_closure_is_rejected_with_a_clear_error() {
    let (mut interp, _class) = bootstrap_class("Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()");
    let module = ns_syntax::parse("Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^[ 1 ] )\n) : ()").unwrap();
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(""),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .unwrap();
    let closure_data = ns_runtime::ClosureData {
        params: vec![],
        body: Rc::new(resolved.module.root_class.instance_side.methods[0].body.clone()),
        captured_frame: Rc::new(std::cell::RefCell::new(ns_runtime::Frame {
            receiver: Value::Nil,
            args: vec![],
            temps: vec![],
            pc: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: ns_runtime::FrameKind::TopLevel,
            uncontinuable: false,
        })),
        home: Rc::new(std::cell::RefCell::new(ns_runtime::Frame {
            receiver: Value::Nil,
            args: vec![],
            temps: vec![],
            pc: 0,
            sender: None,
            home: None,
            current_class: None,
            kind: ns_runtime::FrameKind::TopLevel,
            uncontinuable: false,
        })),
        bindings: resolved.bindings.clone(),
    };
    let closure = Value::Obj(interp.new_closure(Rc::new(closure_data)));
    assert!(encode::encode_object_graph(&[closure]).is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ns-runtime --test image`
Expected: FAIL — `encode_object_graph`/`decode_object_graph` not found.

- [ ] **Step 3: Write the implementation**

Add to `crates/ns-runtime/src/image/encode.rs`:

```rust
use std::collections::HashMap;
use std::rc::Rc;

use crate::types::{BuiltinClass, Object, ObjRef, ObjectData};

pub struct ObjectTable {
    seen: HashMap<usize, u32>,
    order: Vec<ObjRef>,
}

impl ObjectTable {
    pub fn build(roots: &[Value]) -> ObjectTable {
        let mut table = ObjectTable { seen: HashMap::new(), order: Vec::new() };
        for root in roots {
            table.visit(root);
        }
        table
    }

    /// Class-kind objects are resolved by tag, never walked into (this
    /// task's design note) -- returns immediately for one.
    fn visit(&mut self, v: &Value) {
        let Value::Obj(obj) = v else { return };
        if matches!(&obj.borrow().data, ObjectData::Class(_)) {
            return;
        }
        let ptr = Rc::as_ptr(obj) as usize;
        if self.seen.contains_key(&ptr) {
            return;
        }
        let index = self.order.len() as u32;
        self.seen.insert(ptr, index);
        self.order.push(obj.clone());
        let children: Vec<Value> = match &obj.borrow().data {
            ObjectData::Instance(fields) => fields.clone(),
            ObjectData::Array(elements) => elements.clone(),
            _ => Vec::new(),
        };
        for child in &children {
            self.visit(child);
        }
    }

    pub fn index_of(&self, obj: &ObjRef) -> u32 {
        *self.seen.get(&(Rc::as_ptr(obj) as usize)).expect("object was visited by build()")
    }

    pub fn objects(&self) -> &[ObjRef] {
        &self.order
    }
}

fn encode_class_tag(w: &mut Writer, builtin: Option<BuiltinClass>) {
    match builtin {
        Some(kind) => {
            w.write_u8(0);
            w.write_u32(kind as u32);
        }
        None => w.write_u8(1),
    }
}

/// The general `Value` encoder -- extends `encode_scalar_value` (Task 12)
/// with the two `Obj` cases: tag 5 (an instance, by object-table index),
/// tag 6 (a class, by the `Builtin`/`UserRoot` tag).
pub fn encode_value(w: &mut Writer, v: &Value, table: &ObjectTable) -> Result<(), String> {
    if encode_scalar_value(w, v) {
        return Ok(());
    }
    let Value::Obj(obj) = v else { unreachable!("encode_scalar_value handles every non-Obj case") };
    if let ObjectData::Class(data) = &obj.borrow().data {
        w.write_u8(6);
        encode_class_tag(w, data.builtin);
        return Ok(());
    }
    w.write_u8(5);
    w.write_u32(table.index_of(obj));
    Ok(())
}

fn encode_object_body(w: &mut Writer, obj: &Object, table: &ObjectTable) -> Result<(), String> {
    let class_builtin = match &obj.class.borrow().data {
        ObjectData::Class(data) => data.builtin,
        _ => return Err("internal: an object's .class field is not itself a Class object".to_string()),
    };
    encode_class_tag(w, class_builtin);
    w.write_u32(obj.identity_hash);
    match &obj.data {
        ObjectData::Instance(fields) => {
            w.write_u8(0);
            w.write_u32(fields.len() as u32);
            for f in fields {
                encode_value(w, f, table)?;
            }
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
            w.write_u32(elements.len() as u32);
            for e in elements {
                encode_value(w, e, table)?;
            }
        }
        ObjectData::Closure(_) => {
            return Err(
                "image encoding does not yet support live closures (Milestone 2 scope limit -- see the plan's Task 13 design note)".to_string(),
            )
        }
        ObjectData::Class(_) => unreachable!("class objects are never added to the instance table"),
    }
    Ok(())
}

pub fn encode_object_graph(roots: &[Value]) -> Result<Vec<u8>, String> {
    let table = ObjectTable::build(roots);
    let mut w = Writer::new();
    write_header(&mut w);
    w.write_u32(table.objects().len() as u32);
    for obj in table.objects() {
        let borrowed = obj.borrow();
        encode_object_body(&mut w, &borrowed, &table)?;
    }
    w.write_u32(roots.len() as u32);
    for root in roots {
        encode_value(&mut w, root, &table)?;
    }
    Ok(w.into_bytes())
}
```

Add to `crates/ns-runtime/src/image/decode.rs`:

```rust
use std::cell::RefCell;
use std::rc::Rc;

use crate::types::{BuiltinClass, ClassRef, Object, ObjRef, ObjectData};
use crate::Interpreter;

fn builtin_class_from_u32(n: u32) -> Option<BuiltinClass> {
    use BuiltinClass::*;
    // Must stay in the same order as `types::BuiltinClass`'s declaration --
    // a round-trip test (Step 1) catches any drift immediately.
    Some(match n {
        0 => Object,
        1 => Boolean,
        2 => True,
        3 => False,
        4 => Integer,
        5 => Float,
        6 => Str,
        7 => Sym,
        8 => Character,
        9 => Array,
        10 => OrderedCollection,
        11 => Nil,
        12 => Block,
        13 => Platform,
        _ => return None,
    })
}

fn decode_class_tag(r: &mut Reader, interp: &Interpreter, user_root_class: &ClassRef) -> Result<ClassRef, DecodeError> {
    match r.read_u8()? {
        0 => {
            let n = r.read_u32()?;
            let kind = builtin_class_from_u32(n).ok_or(DecodeError::InvalidTag(0))?;
            Ok(interp.builtin_class(kind))
        }
        1 => Ok(user_root_class.clone()),
        tag => Err(DecodeError::InvalidTag(tag)),
    }
}

/// A transient placeholder `.class` for not-yet-decoded objects -- see
/// this task's design note on why a fourth copy of this pattern is
/// consistent with the rest of the codebase. Overwritten by
/// `decode_object_graph`'s second pass before any real code observes it.
fn placeholder_class() -> ClassRef {
    let uninit: Rc<std::mem::MaybeUninit<RefCell<Object>>> = Rc::new_uninit();
    let raw_mut = Rc::as_ptr(&uninit) as *mut std::mem::MaybeUninit<RefCell<Object>>;
    let data_ptr = raw_mut as *const RefCell<Object>;
    // SAFETY: identical, already-reviewed pattern to
    // `interpreter::instantiate::build_self_referential_object_class`.
    unsafe { Rc::increment_strong_count(data_ptr) };
    let self_ref: ClassRef = unsafe { Rc::from_raw(data_ptr) };
    unsafe {
        (*raw_mut).write(RefCell::new(Object { class: self_ref, identity_hash: 0, data: ObjectData::Instance(vec![]) }));
    }
    unsafe { uninit.assume_init() }
}

/// The general `Value` decoder, the inverse of `encode::encode_value`.
pub fn decode_value(
    r: &mut Reader,
    table: &[ObjRef],
    interp: &Interpreter,
    user_root_class: &ClassRef,
) -> Result<Value, DecodeError> {
    match r.read_u8()? {
        0 => Ok(Value::Nil),
        1 => Ok(Value::Bool(false)),
        2 => Ok(Value::Bool(true)),
        3 => Ok(Value::SmallInt(r.read_i64()?)),
        4 => Ok(Value::Float(r.read_f64()?)),
        5 => {
            let idx = r.read_u32()? as usize;
            table.get(idx).cloned().map(Value::Obj).ok_or(DecodeError::InvalidTag(5))
        }
        6 => Ok(Value::Obj(decode_class_tag(r, interp, user_root_class)?)),
        tag => Err(DecodeError::InvalidTag(tag)),
    }
}

pub fn decode_object_graph(
    r: &mut Reader,
    interp: &Interpreter,
    user_root_class: &ClassRef,
) -> Result<Vec<Value>, DecodeError> {
    read_header(r)?;
    let count = r.read_u32()? as usize;
    let placeholder = placeholder_class();
    let table: Vec<ObjRef> = (0..count)
        .map(|_| Rc::new(RefCell::new(Object { class: placeholder.clone(), identity_hash: 0, data: ObjectData::Instance(vec![]) })))
        .collect();
    for slot in table.iter().take(count) {
        let class = decode_class_tag(r, interp, user_root_class)?;
        let identity_hash = r.read_u32()?;
        let data = match r.read_u8()? {
            0 => {
                let n = r.read_u32()? as usize;
                let mut fields = Vec::with_capacity(n);
                for _ in 0..n {
                    fields.push(decode_value(r, &table, interp, user_root_class)?);
                }
                ObjectData::Instance(fields)
            }
            1 => ObjectData::Str(r.read_str()?),
            2 => ObjectData::Sym(crate::Symbol::intern(&r.read_str()?)),
            3 => {
                let n = r.read_u32()? as usize;
                let mut elements = Vec::with_capacity(n);
                for _ in 0..n {
                    elements.push(decode_value(r, &table, interp, user_root_class)?);
                }
                ObjectData::Array(elements)
            }
            tag => return Err(DecodeError::InvalidTag(tag)),
        };
        *slot.borrow_mut() = Object { class, identity_hash, data };
    }
    let root_count = r.read_u32()? as usize;
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        roots.push(decode_value(r, &table, interp, user_root_class)?);
    }
    Ok(roots)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-runtime --test image`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat(runtime): encode and decode the live object graph for images

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 14: `image::snapshot`/`load` and `ns-next snapshot`/`load`

**Files:**
- Modify: `crates/ns-runtime/src/image/encode.rs` (add `encode_source_origin`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (add `decode_source_origin`)
- Modify: `crates/ns-runtime/src/image/mod.rs`
- Modify: `crates/ns-cli/src/main.rs`
- Create: `crates/ns-cli/tests/snapshot_load.rs`

**Interfaces:**
- Produces: `pub fn image::snapshot(source: &str, origin: &SourceOrigin, roots: &[Value]) -> Result<Vec<u8>, String>`, `pub fn image::load(bytes: &[u8]) -> Result<(Interpreter, ClassRef, Rc<HashMap<Span, Binding>>, Vec<Value>), String>`, `ns-next snapshot <file.ns> [--send selector] <out.img>`, `ns-next load <image> [--send selector]`.

**Design notes:**
- **The overall image format is: header, source text, `SourceOrigin`, then the object-graph bytes from `encode::encode_object_graph` verbatim** (which itself writes its own header first) — a small, harmless duplication (8 bytes) rather than refactoring Task 13's already-tested, self-contained `encode_object_graph`/`decode_object_graph` to not own their header. `load` reuses the *same* `Reader` cursor across both sections — `decode_object_graph` picks up exactly where the source/origin section left off, since a `Reader` is just a cursor over one shared byte slice.
- **`load` re-parses and re-resolves the stored source text before it can decode a single object** — this is not incidental plumbing, it *is* the "bootstrap from source" design decision: a `UserRoot`-tagged class reference in the object graph (Task 13) only means anything once a fresh, live `ClassRef` exists to resolve it to, and that `ClassRef` comes from running the ordinary parse -> resolve -> `load_module` pipeline against the recovered source — the same pipeline `ns-next run` already runs, not a separate "image bootstrap" code path.
- **The compiled-method cache is never part of the image** — `load` returns `bindings`, not a `CompiledMethodCache`; a caller that wants to send a message to a loaded image's root object compiles fresh via `compiler::compile_module`, exactly as `cmd_run`/`cmd_snapshot` already do. Consistent with the design spec's Decisions: the image serializes the *logical* object graph, never a physical/compiled representation.
- `ns-next load --send`'s result is printed via `{:?}` (`Value`'s `Debug` impl), not a real `printString` send — building a full result-rendering pipeline for a CLI diagnostic command is more than this milestone's `load` command needs; `ns-next run`'s own stdout (from `Transcript`-style native `show:` calls during the send) is the primary way a Newspeak program is expected to produce output either way.

- [ ] **Step 1: Write the failing tests**

`crates/ns-cli/tests/snapshot_load.rs`:

```rust
use std::path::Path;
use std::process::Command;

#[test]
fn a_snapshot_can_be_loaded_and_sent_a_further_message() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let src = dir.join("arithmetic.ns");
    let out_dir = std::env::temp_dir().join(format!("ns-next-image-test-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).expect("mkdir temp dir");
    let img = out_dir.join("arithmetic.img");

    let snapshot_output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("snapshot")
        .arg(&src)
        .arg("--send")
        .arg("main:args:")
        .arg(&img)
        .output()
        .expect("run ns-next snapshot");
    assert!(
        snapshot_output.status.success(),
        "{}",
        String::from_utf8_lossy(&snapshot_output.stderr)
    );
    assert!(img.exists(), "expected {} to be written", img.display());

    let load_output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("load")
        .arg(&img)
        .output()
        .expect("run ns-next load");
    assert!(load_output.status.success(), "{}", String::from_utf8_lossy(&load_output.stderr));
    assert!(String::from_utf8_lossy(&load_output.stdout).contains("root value"));

    let _ = std::fs::remove_dir_all(&out_dir);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ns-cli --test snapshot_load`
Expected: FAIL — `snapshot`/`load` are not recognized subcommands.

- [ ] **Step 3: Write the implementation**

Add to `crates/ns-runtime/src/image/encode.rs`:

```rust
use crate::types::SourceOrigin;

pub fn encode_source_origin(w: &mut Writer, origin: &SourceOrigin) {
    match origin {
        SourceOrigin::File(path) => {
            w.write_u8(0);
            w.write_str(&path.to_string_lossy());
        }
    }
}
```

Add to `crates/ns-runtime/src/image/decode.rs`:

```rust
use std::path::PathBuf;

use crate::types::SourceOrigin;

pub fn decode_source_origin(r: &mut Reader) -> Result<SourceOrigin, DecodeError> {
    match r.read_u8()? {
        0 => Ok(SourceOrigin::File(PathBuf::from(r.read_str()?))),
        tag => Err(DecodeError::InvalidTag(tag)),
    }
}
```

`crates/ns-runtime/src/image/mod.rs`:

```rust
//! Image serialization: a versioned, architecture-neutral encoding of a
//! live `ns-next` world, decoded on load (never a memory dump). See
//! `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`'s
//! Image format section.

pub mod decode;
pub mod encode;

use std::collections::HashMap;
use std::rc::Rc;

use ns_syntax::Span;

use crate::types::{Binding, ClassRef, SourceOrigin, Value};
use crate::Interpreter;

pub fn snapshot(source: &str, origin: &SourceOrigin, roots: &[Value]) -> Result<Vec<u8>, String> {
    let mut w = encode::Writer::new();
    encode::write_header(&mut w);
    w.write_str(source);
    encode::encode_source_origin(&mut w, origin);
    let body = encode::encode_object_graph(roots)?;
    w.write_raw(&body);
    Ok(w.into_bytes())
}

pub fn load(bytes: &[u8]) -> Result<(Interpreter, ClassRef, Rc<HashMap<Span, Binding>>, Vec<Value>), String> {
    let mut r = decode::Reader::new(bytes);
    decode::read_header(&mut r).map_err(|e| format!("{e:?}"))?;
    let source = r.read_str().map_err(|e| format!("{e:?}"))?;
    let origin = decode::decode_source_origin(&mut r).map_err(|e| format!("{e:?}"))?;
    let module = ns_syntax::parse(&source).map_err(|d| format!("re-parsing snapshotted source failed: {d:?}"))?;
    let resolved = crate::resolve(module, Rc::from(source.as_str()), origin)
        .map_err(|e| format!("re-resolving snapshotted source failed: {e:?}"))?;
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    let roots = decode::decode_object_graph(&mut r, &interp, &class).map_err(|e| format!("{e:?}"))?;
    Ok((interp, class, bindings, roots))
}
```

Add `pub mod image;` to `crates/ns-runtime/src/lib.rs` if not already present (Task 12 already added it).

In `crates/ns-cli/src/main.rs`, add `Snapshot`/`Load` variants to `Cmd`:

```rust
/// Run a file, then serialize the resulting world to an image file.
Snapshot {
    file: PathBuf,
    out: PathBuf,
    #[arg(long)]
    send: Option<String>,
},
/// Deserialize an image and optionally send it one further message.
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

Add the two commands:

```rust
fn cmd_snapshot(file: &Path, out: &Path, send: Option<&str>) -> ExitCode {
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: reading {}: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let module = match ns_syntax::parse(&source) {
        Ok(m) => m,
        Err(diags) => {
            print_diagnostics(file, &source, &diags);
            return ExitCode::FAILURE;
        }
    };
    let origin = ns_runtime::SourceOrigin::File(file.to_path_buf());
    let resolved = match ns_runtime::resolve(module, source.clone().into(), origin.clone()) {
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
    let instance = match interp.instantiate(class.clone(), vec![platform.clone()], &bindings) {
        Ok(v) => v,
        Err(signal) => return report_signal(signal),
    };
    let mut roots = vec![instance.clone()];
    if let Some(selector) = send {
        let compiled = ns_runtime::compiler::compile_module(&class, &bindings);
        let mut vm = ns_runtime::vm::Vm::new(compiled, bindings.clone());
        let args = vec![platform, ns_runtime::Value::Nil];
        match vm.send(&mut interp, instance, ns_runtime::Symbol::intern(selector), args, None) {
            Ok(result) => roots.push(result),
            Err(ns_runtime::Signal::Error(e)) => {
                eprintln!("Uncaught Newspeak error: {}", e.message);
                return ExitCode::FAILURE;
            }
            Err(ns_runtime::Signal::Return(..)) => {
                eprintln!("internal: non-local return escaped the entry send");
                return ExitCode::FAILURE;
            }
        }
    }
    let bytes = match ns_runtime::image::snapshot(&source, &origin, &roots) {
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
    let (mut interp, class, bindings, roots) = match ns_runtime::image::load(&bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: loading image: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("loaded image with {} root value(s)", roots.len());
    let Some(selector) = send else {
        return ExitCode::SUCCESS;
    };
    let Some(instance) = roots.into_iter().next() else {
        eprintln!("error: image has no root instance to send to");
        return ExitCode::FAILURE;
    };
    let compiled = ns_runtime::compiler::compile_module(&class, &bindings);
    let mut vm = ns_runtime::vm::Vm::new(compiled, bindings);
    match vm.send(&mut interp, instance, ns_runtime::Symbol::intern(selector), vec![], None) {
        Ok(result) => {
            println!("{result:?}");
            ExitCode::SUCCESS
        }
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ns-cli --test snapshot_load`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets
git add -A
git commit -m "feat: add ns-next snapshot and ns-next load

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Task 15: Final verification

**Files:** none (verification only).

- [ ] **Step 1: Full workspace test suite**

Run: `cargo test --workspace`
Expected: every test from every prior task passes, including the differential harness across all `tests-fixtures/*.ns` files (now covering `while_loop.ns`, `control_flow_fallback.ns`, `hello.ns`, `arithmetic.ns`, `blocks_and_return.ns`) and the `disasm`/`snapshot_load` integration tests.

- [ ] **Step 2: Formatting and lints**

Run: `cargo fmt --all -- --check`
Expected: clean.

Run: `cargo clippy --workspace --all-targets`
Expected: zero warnings.

- [ ] **Step 3: No Milestone 1 frontend regression**

Run: `cargo run -p ns-cli -- parse-corpus corpus/modules`
Expected: `parsed 210, failed 0, expected-fail 1, stale 0` — identical to Milestone 1's own final count (Global Constraints). This milestone touches nothing under `ns-syntax`, so any change here indicates something went wrong elsewhere (e.g. a build break masking the real corpus result).

- [ ] **Step 4: Manual sanity check of the default engine**

Run: `cargo run -p ns-cli -- run tests-fixtures/blocks_and_return.ns` (no `--engine` flag — confirms the *default* is really `bytecode`, not just the explicitly-flagged test cases)
Expected: identical stdout to `tests-fixtures/blocks_and_return.expected`.

Run: `cargo run -p ns-cli -- disasm tests-fixtures/while_loop.ns main:args:`
Expected: readable bytecode listing showing a back-edge `Jump` (the inlined `whileTrue:` loop) with no `SendPublic` for `whileTrue:` itself.

- [ ] **Step 5: Commit any outstanding changes**

If Steps 1-4 required any fixes, commit them now with an appropriately scoped message (e.g. `fix(runtime): ...`) before considering Milestone 2 complete. If nothing needed fixing, this step is a no-op — do not create an empty commit.

---

## Self-Review Notes (from writing this plan)

**Spec coverage**, checked section by section against `docs/superpowers/specs/2026-09-04-ns-next-bytecode-vm-design.md`:

- Bytecode compiler + register VM replacing the tree-walker as default: Tasks 1-8.
- Baseline control-flow inlining with guard + fallback: Task 10.
- Reserved, inert inline-cache slots / stable call-site identity: `CallSite`/`call_site_id` (Task 1), threaded through every `Send*` instruction from Task 3 onward — never read, matching the spec's "Milestone 2 always misses it" Non-Goal.
- Statement→PC table / safepoint abstract-state map: `CompiledMethod.statement_pc` (Task 1, populated throughout `Lowerer`); the "abstract-state map" itself is the `Frame` (receiver/args/temps/pc), already reified by Task 6 — no separate data structure needed, per the design spec's own note that this is intentionally the same shape Milestone 1 already reified.
- Architecture-neutral, versioned image format bootstrapped from source: Tasks 12-14.
- `ns-next disasm`: Task 11.
- Tree-walker retained as `--engine=tree-walk`, differential-tested: Tasks 8-9.
- Object model stays `Rc<RefCell<Object>>`; image format defined against the logical value model: Task 13's design notes (`Builtin`/`UserRoot` tags, never a physical-representation detail).

**Corrections made while writing this plan** (each documented inline at its own task, not just here): `Instr` needed `PartialEq` (Task 1) for the many `assert_eq!` tests written from Task 2 onward; `LoadBuiltinClass` (Task 3) and `ImplicitReturn` (Task 4) were missing opcodes the design spec didn't enumerate; `MakeClosure`'s shape and the `NonLocalReturn` opcode were redesigned (Tasks 1, 4, 7) after working out `vm.rs`'s actual dispatch orchestration in Task 6 showed that general block invocation permanently goes through the unmodified tree-walker, making a from-scratch "compile a block to its own bytecode activation" design dead code; `JumpIfNotBoolean`/`BranchIfTrue` (Task 10) were needed beyond the spec's `BranchIfFalse`/`Jump` pair. None of these change the spec's *intent*; each is noted as a plan-level refinement in its task, and the plan's header notes the one scope clarification (instance-initializer code stays on the tree-walker) that is worth the spec authors' attention before or during execution.

**Type consistency**, spot-checked: `Instr` variant field names (`dst`, `src`, `slot`, `depth`, `index`, `receiver_reg`, `arg_regs`, `call_site`) are used identically everywhere they're referenced from Task 3 onward; `CompiledMethod`'s five fields and `compile_method`/`compile_module`/`method_key`'s signatures (Task 5) are used identically by Tasks 6, 11, 14; `Vm::new`/`Vm::send`'s signature (Task 6) is used identically by Tasks 8, 9, 14.

