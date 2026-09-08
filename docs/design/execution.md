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
compiling excluded — over a 50,000-iteration `whileTrue:` loop, on a release
build. Over `bench_loop.ns`: bytecode median ~19.6 ms against tree-walk ~37.4 ms.

That ratio is **not** a pure dispatch comparison: the compiler inlines
`whileTrue:`, so the bytecode engine does not pay the loop-control overhead the
tree-walker does.

Never judge performance from a debug build.

### What a send costs, and where it goes

Milestone 4's design rests on a decomposition rather than on the aggregate.
Three variants of `bench_loop.ns`, each with the body
`[n < 50000] whileTrue: [<first statement>. n := n + 1]`, isolate the pieces,
because each differs from the next by exactly one send:

| First statement | Method it calls | bytecode median | ns/iteration |
|---|---|---|---|
| `sum := sum + 1` | — | 19.02 ms | 380.5 |
| `sum := self tick: sum` | `public tick: x = (^x)` | 22.44 ms | 448.7 |
| `sum := self tick: sum` | `public tick: x = (^x + 1)` | 27.28 ms | 545.7 |

Rows one and three each pay three native sends per iteration; row two pays two,
because its method body has no `+`. So the third minus the second is exactly one
native `+` send: **~97 ns**. The third minus the first is exactly one
user-method send, both paying three native sends: **~165 ns**. The second minus
the first is then predicted to be their difference, 68 ns, and measures 68.2 —
the decomposition is self-consistent rather than fitted.

So **building an activation costs about 68 ns, roughly 41% of a user-method
send**, and it is a fixed cost: a two-argument send measures the same as a
one-argument one. What is in it is the current representation, not the work:
three heap allocations (the `Rc<RefCell<Frame>>` block, `temps`, and the `args`
vector `collect` builds), reference-count traffic on `sender`, `method`,
`bindings` and the defining class, and a `HashMap` lookup per invocation just to
find the compiled body. On top of that, every register read and write in
`run_code` takes a `RefCell` borrow, which is spread across the straight-line
instructions of all three rows rather than isolated in any one of them.

Two caveats. `U - N` is an incremental difference, not a profiler attribution:
the native path pays a `match selector.as_str()` string comparison chain the
user path does not. And these are medians of a whole-program benchmark.

## Planned: activations on a stack, reified on demand

Today every activation is an individually heap-allocated `Rc<RefCell<Frame>>`,
whether or not anything ever looks at it. The measurements above price that at
about 41% of a send. The plan is the model Cog uses for Smalltalk: activations
live as entries in a stack, cheap to push and pop, and are **reified into
first-class frame objects only when something needs one** — a debugger, a mirror,
`thisContext`, the image encoder, a closure that captures its activation, or a
non-local return target.

Pushing then costs an index bump rather than three allocations; the caller's
argument registers *are* the callee's argument window, so `collect`'s vector
disappears; `sender` is the entry below rather than a counted reference; and
because the stack is owned rather than shared, a register access is a slice index
with no borrow flag — which removes a cost from every instruction, not only from
sends.

**The trampoline falls out of this rather than driving it.** An explicit
activation stack is what a trampolined `run_code` needs anyway, so replacing the
Rust-stack recursion — `run_code` → `interp.send` → hook → `run_code` — with a
loop that pushes is the same change viewed from the other side. Getting there
requires splitting `Interpreter::send` so that it *decides* without *executing*:
lookup, `doesNotUnderstand:`, native dispatch and the nested-class accessor stay
in `interpreter/`, shared by both engines and unchanged in what they produce, but
they return a description of what to activate instead of activating it. That
keeps the shared-orchestration rule intact — what moves into the loop is the act
of running a body, never the decision of which body.

Two things the trampoline settles on its own:

- **`Block value` stops being a callback.** It is a native today that calls
  `invoke_closure`, which re-enters `run_code`. Once the loop owns the stack,
  activating a closure is what the loop already does, so the Rust frame in the
  middle disappears without anything being rewritten in Newspeak.
- **Recursion depth becomes a checkable limit and an ordinary Newspeak error**
  instead of a native stack overflow that aborts the process. That closes the
  exposure [image-format.md](image-format.md) records against a loaded artifact.

`Signal::Return` needs re-implementing against the stack: it propagates through
Rust `?` today until the `invoke_method` that owns `home` catches it, and becomes
an unwind of the activation stack to the target entry. The semantics are
unchanged and both engines must still agree on them, so this is the part of the
change the differential harness has to be trusted hardest on.

### What stays recursive, and why that is not a hole

The natives that call back into Newspeak — `builtins::call_block`'s callers —
keep their Rust frames for now. That is a barrier for a future JIT rather than a
correctness problem, and it is addressed by moving them out of Rust rather than
by trampolining around them; see [core-library.md](core-library.md). Their
Rust-held values are covered by the scope discipline in
[memory.md](memory.md), the same way the tree-walker's are.

### How this meets the liveness requirements

Reification is not a compromise against
[liveness-and-reflection.md](liveness-and-reflection.md); it is that document's
own unifying principle applied one level down. The interpreter level is the
source of truth and a JIT is an accelerator that must be able to deoptimize to
it. A stack zone is an accelerator over the reified activation model, and
reification is its deoptimization.

That resolves the one requirement that looks like a conflict. Edit-and-continue
must be able to add a local to a method while an activation of it is live, and
`Frame.temps` is a growable `Vec` precisely for that — which a flat register
stack cannot offer for an entry in the middle. The answer is that editing a
method reifies its affected activations first, and a reified frame has exactly
today's flexibility. The document already accepts that entering the debugger
forces the frames it touches out of their fast representation; this is the same
step, one level below the JIT.

## Planned: JIT tiers

Beyond inline caches nothing is designed. The tiering model is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md), and the constraint that
binds it is that no native code may ever enter an image.

## Decisions

- **The tree-walker is permanent**, and is the reference when the engines
  disagree.
- **`vm.rs` holds no dispatch logic.** Every observable error comes from shared
  code, which is what makes the differential harness meaningful rather than
  approximate. Splitting `send` into a decision and an activation preserves this:
  the decision does not move.
- **A closure runs with the engine that created it**, enforced by the type of its
  body rather than by a run-time flag.
- **`pc` is stored only at observable points**, not at every instruction — a
  deliberate hot-path trade the debugger milestone may have to revisit.
- **Activations are stack entries, reified on demand**, because the present
  always-reified model costs about 41% of a send for a structure most activations
  never expose.
