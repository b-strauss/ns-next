# Execution — activations, both engines, the VM

> **Status:** built.
> **Code:** `crates/ns-runtime/src/types.rs` (`Frame`), `vm.rs`, `interpreter/`
> (the shared orchestration and the three engine hooks).

This is the consumer side. What produces the bytecode — the instruction set,
lowering, register allocation, inlining — is [compiler.md](compiler.md).

## Two engines, permanently

`ns-next run` defaults to `--engine=bytecode`. `--engine=tree-walk` is the
Milestone 1 interpreter, kept **permanently** as a differential oracle rather than
as a transitional convenience.

**For every program both engines must produce identical exit status, stdout and
stderr.** `crates/ns-cli/tests/differential.rs` enforces this over every
`tests-fixtures/*.ns` plus inline sources, with no exemptions, and `run-corpus`
does the same over the runnable corpus. If a change makes the engines disagree,
the tree-walker is the reference — fix the compiler or the VM.

Two engines that must agree byte for byte is a verification property this project
funds on purpose. When the moving collector lands, the tree-walker gets its own
handle-based root discipline rather than being retired; see [memory.md](memory.md).

The one thing the tree-walker cannot do is run from a loaded image: `image::load`
decodes mixins and class objects, never AST-carrying bindings, so there is no
source for it to execute. `Interpreter::set_engine(Engine::TreeWalk)` refuses on a
loaded image rather than running it against the wrong data.

## Activations

A Newspeak-level activation is never represented only by Rust locals or the Rust
call stack:

```rust
struct Frame {
    receiver: Value,
    args: Vec<Value>,
    temps: Vec<Value>,        // the register file, indexable and growable
    pc: usize,
    sender: Option<FrameRef>,
    home: Option<FrameRef>,   // for a block: its method activation
    current_class: ClassRef,  // NOT necessarily the receiver's class
    kind: FrameKind,
    uncontinuable: bool,
    slot_base: usize,
}
```

`current_class` is required by §3.6: the current class is the class that actually
**defines** the running method, not the receiver's concrete class, and it is what
`self`, `outer` and `super` sends inside that method resolve relative to. It is
set by the dispatch step that creates the activation and never derived from the
receiver at read time, which is what makes chained `super` sends through a deep
hierarchy resolve one level at a time.

`temps` is the register file, addressed as described in
[compiler.md](compiler.md). It is a growable `Vec` on purpose: adding a local slot
to a running method means growing the active frame's `temps`, initializing the new
entries to nil, and remapping the `pc` — the edit-and-continue requirement.

`kind` distinguishes a method, closure, initializer and top-level activation. An
initializer frame must be able to name the code it is executing so that `pc` is
interpretable, since the statement→PC table is reached through the frame's kind.

`pc` means a statement index under the tree-walker and an instruction index under
the VM. Frames are serialized, so this is a format-visible ambiguity and the frame
carries a discriminator rather than leaving it implicit.

**The frame chain is walkable independently of the Rust stack**, and always has
been. That is what the debugger will reflect over, what an image snapshot
serializes, and what a precise collector will scan.

## The VM and the three hooks

`Interpreter` carries `engine: Engine`, either `TreeWalk` or
`Bytecode(CompiledModule)`. Everything in `interpreter/` that decides *what* to
run is shared and identical under both. Exactly three places decide *how* a body
runs:

1. **Method body** — `invoke_method`, after building the frame.
2. **Block body** — `invoke_closure`, dispatching on `ClosureData.body`, which is
   `ClosureBody::Ast` or `ClosureBody::Compiled`. A closure therefore always runs
   with the engine that created it, and "under `--engine=bytecode` no
   Newspeak-level code is tree-walked" is enforced by the type rather than by a
   flag. Every native that invokes a block — `ifTrue:ifFalse:`, `do:`, `and:`,
   `whileTrue:`, `value` — goes through the same unchanged path and simply lands
   on the compiled arm.
3. **Instance initializer** — `instantiate`'s per-level "run this level's own slot
   initializers and initializer expressions" step. A class with no compiled
   initializer and no slots is a no-op under both; a user class missing from the
   cache is a compilation-coverage bug and panics.

`vm.rs` is therefore *only* a `match`-dispatch loop over `Instr` —
`run_code(interp, frame, code, bindings)`, one compiled body plus `pc` plus a
register file per frame. Its send handlers call straight back into the shared
orchestration, which re-enters `run_code` through hook 1 or 2 for any user-defined
target.

**The VM has no dispatch logic, no `doesNotUnderstand:` construction, no native
dispatch and no error text of its own.** Every error a program can observe is
produced by the same shared code under both engines. That is what makes the
tree-walker a byte-for-byte oracle including stderr, rather than a merely similar
one — and it is why the shared-orchestration rule in `CLAUDE.md` forbids copying
anything out of `interpreter/` into `vm.rs`.

Block invocation is not special-cased into an opcode: `Block value` is still a
native method, the sender's frame still becomes the closure activation's sender,
and a wrong argument count is still the same error.

## Where `pc` is stored

`frame.pc` is written **only by the instructions that can hand the frame to an
observer**: every send, since a nested activation runs with this frame as its
sender; `MakeClosure`, since the frame escapes into the object graph where the
image encoder can serialize it; and `NonLocalReturn`, since control leaves with a
signal.

Nothing can read `pc` while a straight-line instruction executes, because
`run_code` never yields between them. Keeping the store out of the straight-line
arms takes a `RefCell` borrow and a write off the hot path for a value nothing
observes there.

Whether a debugger will force `pc` at every instruction is an open question for
the milestone that builds one; it is a deliberate trade, not an oversight.

## Non-local return

A closure's `home` is the enclosing method or initializer activation. A block
body's `^` raises through `raise_return` on the block's own frame, which walks to
`home`, checks `uncontinuable` — raising `cannotReturn` if set — marks the target
uncontinuable, and returns a return signal.

That signal propagates out of `run_code` through every intervening Rust frame,
native primitives included, until the shared `invoke_method` that owns `home`
catches it — exactly as it catches the tree-walker's. When `home` is an
*initializer* frame nothing catches it, a gap both engines share identically.

A method body's own `^` never raises a signal at all. `invoke_method` marks the
frame uncontinuable on every exit path regardless, so the observable discipline —
a block invoked after its home has returned gets `cannotReturn` — is preserved
without a signal round-trip on the common path.

**Never `.unwrap()`, `.expect()` or `{:?}` an `EvalResult`.** `Signal`
deliberately has no `Debug`: it holds a frame cycle. Tests unwrap through a local
`expect_ok` helper.

## Measurements

`ns-next bench` times the entry send alone — parsing, resolving, bootstrapping and
compiling excluded — over `bench_loop.ns`, a 50,000-iteration `whileTrue:` loop,
on a release build. Bytecode median ~19.6 ms against tree-walk ~37.4 ms.

That ratio is **not** a pure dispatch comparison: the compiler inlines
`whileTrue:`, so the bytecode engine does not pay the loop-control overhead the
tree-walker does. The bytecode median alone is the before-number for inline
caches, since every send today is still a full lookup.

Never judge performance from a debug build.

## Planned: trampolined sends

Nested sends recurse through the Rust call stack: `run_code` → `interp.send` →
hook → `run_code`. The Newspeak activation chain therefore exists twice — reified
on the heap and implicitly on the Rust stack. Only the first can be serialized;
only the second can be resumed.

Replacing the recursion with an explicit frame stack in `run_code`, where a send
pushes instead of recursing, is required by three separate things: the moving
collector wants to scan roots without searching the Rust stack, the debugger wants
stepping and a suspendable activation, and actor scheduling wants activations it
can leave lying around. It is also what would make snapshotting a running
computation possible; see [image-format.md](image-format.md).

The alternative — keep recursing and unwind through a signal — is not viable,
because native frames that call back into Newspeak would be lost.

## Planned: JIT tiers

Beyond inline caches nothing is designed. The tiering model is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md), and the constraint that
binds it is that no native code may ever enter an image.

## Decisions

- **The tree-walker is permanent**, and is the reference when the engines
  disagree.
- **`vm.rs` holds no dispatch logic.** Every observable error comes from shared
  code, which is what makes the differential harness meaningful rather than
  approximate.
- **A closure runs with the engine that created it**, enforced by the type of its
  body rather than by a run-time flag.
- **`pc` is stored only at observable points**, not at every instruction — a
  deliberate hot-path trade the debugger milestone may have to revisit.
