# The Lifetime Brand (2c) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every movable handle — `ObjectReference`, `ClassReference`, `FrameReference`, and so
`Value` and every type that holds one — carries an invariant lifetime `'gc`
obtainable only inside `Interpreter::enter`. Holding a handle across a
safepoint, a native re-entering the run loop, and a host storing a handle
in its own field then fail to compile, and a `Root` is the only way a host
keeps a value across a run.

> Revised during execution: the cast described below was found unnecessary
> and the crate forbids `unsafe` code — see Decision 1's revision.

**Architecture:** The interpreter's step state — its heap, builtin classes,
caches, argument buffer and spare instantiation boxes — moves out of
`Interpreter` into a new `Mutator`. At rest it is held as a
`Mutator<'static>` by a `World` (`heap/world.rs`), and `World::enter` lends
it out as a `Mutator<'gc>` for one closure generic over `'gc`, gc-arena's
`mutate`. That lending is the crate's only `unsafe`: one pointer cast that
changes nothing but a phantom lifetime (revised: no cast — see Decision 1).
`Interpreter` keeps the `World` and
the state only the run loop needs (`running`, the stress schedule), so no
step, holding only a `&mut Mutator`, can reach `vm::run`, the safepoint or a
host entry. Each instruction the run loop executes is one `enter`. The work
lands in two stages: Tasks 1–3 build the structure with no lifetime and
convert every host and test to read through `enter`; Task 4 adds `'gc`, and
the compiler then finds every place a handle would leave an entry.

**Tech Stack:** Rust 1.98.0, edition 2021, no new dependency. Only
`ns-runtime` and `ns-cli` change; nothing under `crates/ns-syntax`.

**Spec:** [docs/design/memory.md](../../design/memory.md), "Checked roots and
a collector" — above all "Planned: the brand, checked by the compiler rather
than by convention" and "Host roots" — and
[docs/PROGRESS.md](../../PROGRESS.md), "Where the next session starts", for
the measurement rule against `d804eaf`. Read both before Task 1, and read
`CLAUDE.md`'s invariants: every one of them applies.

## Decisions this plan carries

Settled with the user on 2026-09-13, or following from the design and the
repository's invariants:

1. **One audited cast, not a conversion per payload.** The heap stores
   handles in brand-erased form and a step sees them branded. Converting
   each payload safely rebuilds it, and a `Box<WhileLoop>`, `Box<DoLoop>`,
   `Box<NestedClassCreation>` or `Box<Instantiation>` can change its
   lifetime in safe Rust only by reallocating: at every push and pop of a
   stack entry that carries one — every iteration of a native `whileTrue:`
   or `do:`, every step of an instantiation — and the reuse of
   `spare_walks`' boxes, which measured 3–13% on the instantiation
   benchmarks, could not survive. Nova casts exactly there (`unsafe impl
   Bindable for Box<T>` by `transmute`,
   `prior-art/nova/nova_vm/src/engine/context.rs`). The user chose the
   audited cast over safe-first-then-measure. This plan makes it smaller
   than a cast per type: the whole step state rests as `Mutator<'static>`
   and `World::enter` casts `&mut Mutator<'static>` to `&mut Mutator<'gc>`
   once per entry — gc-arena's `Arena::mutate`, which casts its root the
   same way. Nothing is converted at a push, a pop, or an allocation. The
   crate gets `#![deny(unsafe_code)]`, and the cast carries the one
   `#[allow(unsafe_code)]`, so a second `unsafe` fails to compile until
   someone argues for it.

   Revised during execution (Task 4 review, 2026-09-14): no cast is needed
   — `body(&mut self.mutator)` gives the same guarantee in safe Rust, since
   the closure is checked for every `'gc` — see Task 5.
2. **The cast is sound because the brand is phantom.** A handle is a `u32`
   index; `'gc` appears in `Mutator`'s field types only through the
   `PhantomData` in `ObjectReference` and `FrameReference`. No field of any branded type may
   ever hold a real reference with lifetime `'gc` — that is the cast's
   safety condition, stated at the cast and in `CLAUDE.md` (Task 5).

   Revised during execution (Task 4 review, 2026-09-14): no cast, so no
   cast safety condition — see Task 5.
3. **There is no public `unbind`.** Two private functions in `heap.rs`
   convert a `Value` between its branded and its at-rest form, for the host
   root table, whose slots a host shares from outside every entry. Nothing
   else changes a brand.
4. **`Mutator` never hands out a `&mut Interpreter`.** `vm::run`, the
   safepoint and the host entries (`send`, `instantiate`, `load_module`,
   `platform`) take `&mut Interpreter`; a native, a decision or a
   continuation gets only a `&mut Mutator`. That is what makes "a native
   re-entering the run loop" a compile error.
5. **`World` lives in `heap/world.rs`, a child module of `heap`**, so that
   `Heap::move_everything` and `Heap::new` can be private to the heap module
   (Task 4): only `World` builds a heap, and only `World` moves one, and
   `World` cannot move while a `Mutator` is lent out, because `enter` holds
   `&mut self`.
6. **Each instruction the loop executes is one `enter`**, as memory.md
   designs it. `vm::run` holds between instructions only the running
   `ActivationReference`, its `Rc<CompiledCode>` and its `program_counter`, none branded. A run
   ends inside its last entry, as a `HostResult`: `vm::run` answers roots,
   never a branded value.
7. **The host API.** `Interpreter::enter` is public. `send`, `instantiate`,
   `load_module` and `platform` take and answer roots as before.
   `Interpreter::stash(&self, Value<'static>)` stays, for immediates: a host
   can build `Value::SmallInteger(3)` but can never hold an object handle outside
   an entry, so a `Value<'static>` it stashes holds none. `heap_reference()`,
   `heap_mut()` and `Interpreter::fetch` go. `Mutator` offers `heap()`,
   `heap_mut()`, `fetch` and `stash`, and every method that was `pub` on
   `Interpreter` and belongs to a step (`builtin_class`, `class_of`,
   `metaclass_of`, `new_array`, `new_string`, `new_symbol`, `new_character`,
   `new_closure`, `apply_mixin`).
8. **The codec runs inside an entry.** `image::snapshot` takes a
   `&Mutator<'gc>` and branded values, `image::build` a `&Heap<'gc>`;
   `image::load` decodes inside one entry of the interpreter it bootstraps
   and roots what it answers before the entry ends.
9. **`ActivationReference` stays unbranded**, as memory.md says: nothing moves a
   stack entry, and its generation covers an index reused after a pop.
10. **Two stages.** Tasks 1–3 move the code into its final shape with no
    lifetime, with a transitional `Deref` from `Interpreter` to `Mutator`
    that keeps unconverted hosts and tests compiling until Task 3 removes
    it. Task 4 adds `'gc` and nothing else. A reviewer can then check Task 4
    as "types only", and a test that still lets a handle out of an entry
    is a compile error there, not a mystery.
11. **The three `compile_fail` doctests sit on `Interpreter::enter`**, each
    with a passing twin that differs from it in the one line that matters,
    so that each one fails for the reason it names and not for a typo.
12. **The finding "two stale handles from the same old epoch compare in
    `same()` without a panic" closes.** Within one entry nothing moves, and
    no handle survives from one entry to the next, so branded code cannot
    hold a stale handle at all. Only a bare `Heap` in `heap.rs`'s own unit
    tests can still make one, and those tests are about the debug check
    itself.

## Scope

In: `Mutator`, `World` and `Interpreter::enter`; the step code moved onto
`Mutator`; one `enter` per instruction; the host entries, the CLI and every
test reading through `enter`; the brand on every movable handle and every
type that holds one; the one cast and `#![deny(unsafe_code)]` (revised: no
cast, `#![forbid(unsafe_code)]` — see Decision 1); the three `compile_fail`
doctests; the measurements and the documents.

Deliberately **not** in this plan:

- The collector, liveness, reclamation, the allocation counter (step 5).
  The stress mode and the debug epoch stay exactly as 2b left them: the
  brand covers what Rust holds, the stress mode what the tracer knows.
- The inline caches (step 3), and the cached getter symbol and `u32` hasher
  for lexical sends (PROGRESS.md, "The profile of `bench_block.ns` is
  taken").
- The other open findings (a decoded initializer not held to answering
  `self`; no test for a `^` into a clause's own activation; the stress
  suite's three minutes).
- Any change to program outcomes, the image format or a wire tag.

## Global Constraints

- **`vm.rs` holds no dispatch logic.** It gains the `enter` per instruction
  and nothing else; everything a step does stays in `interpreter/`,
  `builtins/` and `dispatch.rs`.
- **Program outcomes are pinned, not re-derived.** No golden, no `.expected`
  file and no `run-corpus` row changes in this plan. A test whose output
  changes is a defect in the change.
- **An image is untrusted input.** Nothing in the decoder's validation
  changes; its functions only change the type they are handed.
- **Compiling an already-resolved module can never raise a
  `NewspeakError`.** The compiler takes no branded type and does not
  change.
- **A handle means nothing against a `Heap` that did not mint it — or across
  a move, or outside the entry that minted it.** The last clause is this
  plan's.
- **Only `heap.rs` rewrites or rebrands a handle.** `trace.rs` enumerates;
  `heap.rs`'s `Forwarder` rewrites; `heap.rs`'s private `rebrand` is the only
  conversion of a brand, and `heap/world.rs`'s `enter` the only cast
  (revised: no cast — see Decision 1; and the final review split `rebrand`
  into `at_rest` and `branded`, one per direction, as Decision 3 says).
- **Every `Trace` impl keeps naming every field and matching every variant**
  (no `..`, no `_` arm).
- **Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an
  `EvaluationResult` or a `HostResult`.** Tests unwrap through a local
  `expect_ok`.
- **Object, frame and activation access goes through `heap.rs`.**
- **A Newspeak module written inline in a test is an `indoc! {"…"}`**, lines
  indented with spaces, the closing `"}` on its own line.
- **Comments explain the decision, not the mechanics.**
- **Release sizes do not grow:** `Object` 56 bytes, `FrameSlot` 136,
  `Result<Step, Signal>` 56 (pinned by
  `a_decision_stays_the_size_the_hot_path_was_measured_at`). A lifetime is
  zero-sized; if a size moves, something other than the brand changed.
- **Performance is measured, not assumed**, with the protocol below, in Task
  1 (the per-instruction `enter`) and for the whole step in Task 5. A task
  that makes a source slower than its parent beyond noise is not done:
  report the finding to the controller before anything else happens.
- **Gates before every commit:** `cargo fmt --all`; `cargo clippy --workspace
  --all-targets` warning-free; `cargo test --workspace`;
  `NS_NEXT_STRESS_MOVES=1 cargo test --workspace` (about 190 s — budget for
  it, and run it once per task, before the commit, not after every step);
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`;
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`, and so does
  `NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules`.
- Work on the checked-out branch `feature/lifetime-brand-2c`, created from
  `master` at `4999d05`; no worktree except the throwaway ones the
  measurement protocol builds.
- Conventional commits. Every commit, a subagent's included, ends with the
  two trailer lines below, and the `Co-Authored-By` line names the model
  that actually wrote that commit's code — `Claude Opus 5` unless the task
  was dispatched to another model, in which case that model's name:
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj
  ```

## Running a command under the stress mode

In Git Bash:

```bash
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

In PowerShell:

```powershell
$env:NS_NEXT_STRESS_MOVES = "1"; cargo test --workspace; Remove-Item Env:NS_NEXT_STRESS_MOVES
```

It does nothing in a release build.

## Measurement protocol

Used by every task that says "measure". All three binaries are compared **in
one sitting**; a figure from another session is never compared with one from
this. Per PROGRESS.md, every measurement up to step 3 compares against
`d804eaf` as well as against the commit the task started from.

Besides the three fixtures, four throwaway sources exercise the paths the
fixtures do not: `do:`, and instantiation with and without a superclass and
clause arguments. They live under `target\bench-2b\` (ignored by git, never
committed) and may already exist from step 2b; write any that is missing,
tab-indented like the fixtures.

`target\bench-2b\bench_do.ns` (prints `275000`):

```
Newspeak3
'Bench'
class BenchDo usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	public main: p args: a = (
		| n |
		n := 0.
		[n < 5000] whileTrue: [#(1 2 3 4 5 6 7 8 9 10) do: [:e | sum := sum + e]. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`target\bench-2b\bench_new.ns` (prints `20000`):

```
Newspeak3
'Bench'
class BenchNew usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	class Cell = Object new (
		| v ::= 1. |
	) (
		public v = ( ^v )
	) : ()
	public main: p args: a = (
		| n |
		n := 0.
		[n < 20000] whileTrue: [sum := sum + Cell new v. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`target\bench-2b\bench_new2.ns` (prints `20000`):

```
Newspeak3
'Bench'
class BenchNew2 usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	class Base = Object new (
		| u ::= 1. |
	) (
	) : ()
	class Cell = Base new (
		| v ::= 1. |
	) (
		public v = ( ^v )
	) : ()
	public main: p args: a = (
		| n |
		n := 0.
		[n < 20000] whileTrue: [sum := sum + Cell new v. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`target\bench-2b\bench_newarg.ns` (prints `20000`):

```
Newspeak3
'Bench'
class BenchNewArg usingPlatform: platform = Object new (
	| sum ::= 0. |
) (
	class Base x: a = Object new (
		| u ::= a. |
	) (
		public u = ( ^u )
	) : ()
	class Cell = Base x: 1 (
		| v ::= 1. |
	) (
	) : ()
	public main: p args: a = (
		| n |
		n := 0.
		[n < 20000] whileTrue: [sum := sum + Cell new u. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

Check each prints what it should before timing it
(`cargo run -p ns-cli -- run target\bench-2b\bench_do.ns`).

```powershell
# d804eaf and the commit the task started from, each in a throwaway worktree
git worktree add ..\ns-next-base d804eaf
git worktree add ..\ns-next-parent <parent-commit>
cargo build --release -p ns-cli --manifest-path ..\ns-next-base\Cargo.toml
cargo build --release -p ns-cli --manifest-path ..\ns-next-parent\Cargo.toml
cargo build --release -p ns-cli
$bins = [ordered]@{
  base   = "..\ns-next-base\target\release\ns-next.exe"
  parent = "..\ns-next-parent\target\release\ns-next.exe"
  after  = ".\target\release\ns-next.exe"
}
$sources = "tests-fixtures\bench_loop.ns", "tests-fixtures\bench_send.ns",
           "tests-fixtures\bench_block.ns", "target\bench-2b\bench_do.ns",
           "target\bench-2b\bench_new.ns", "target\bench-2b\bench_new2.ns",
           "target\bench-2b\bench_newarg.ns"
foreach ($round in 1..5) {
  foreach ($source in $sources) {
    foreach ($name in $bins.Keys) {
      "$round $name $source"
      & $bins[$name] bench $source --iterations 30   # prints median, min, max
    }
  }
}
git worktree remove ..\ns-next-base
git worktree remove ..\ns-next-parent
# the decision-size pin runs only in a release build
cargo test --release -p ns-runtime --lib a_decision_stays_the_size_the_hot_path_was_measured_at
```

Record the five medians per source and binary and each group's mean. The
comparison with the parent is the gate: a change is "no slower" when, for
every source, the after-group does not lie entirely above the parent-group,
and "faster" when it lies entirely below. The comparison with `d804eaf` is
reported, not gated — but if a source's shift against `d804eaf` grows past
what 2b left (`bench_loop.ns` +1.1%, `bench_send.ns` +0.9% to +1.4%,
`bench_block.ns` −2.5%, `bench_do.ns` −1.4%, `bench_new.ns` −0.4% to −1.4%,
`bench_new2.ns` −0.6% to −0.8%, `bench_newarg.ns` −3.2% to −3.4%;
execution.md, "What 2b moved") by more than either group's spread, report it
to the controller before anything else happens. Put both tables in the
task's report; Task 5 carries the whole-step table into the documents.

## The conversion rules

Tasks 2–4 apply these to every host and test. They are written once here so
that the three tasks convert alike.

Names are written out (CLAUDE.md, "Conventions"): the closure parameter of an
entry is `mutator`.

**The rule.** Nothing that holds a handle leaves an `enter` closure. A closure
answers brand-free data — `bool`, a number, a `String`, a `Symbol`, an
`Option`/`Vec` of those, a `Root` or a `Vec<Root>` — and a value that must
outlive the closure leaves it as a `Root` (`mutator.stash(value)`) and comes back in
the next one (`mutator.fetch(&root)`). Task 4 makes the compiler enforce it; Tasks
2 and 3 follow it by hand, so that Task 4 finds nothing in the tests to fix.

**Patterns.** Each "before" is today's code, each "after" the converted form.

(a) A read through the heap of a value a host holds as a root:

```rust
// before
let Value::Object(built) = interpreter.fetch(&built) else { panic!("an array") };
assert_eq!(interpreter.heap_reference().array_length(built), Some(2));
// after
interpreter.enter(|mutator| {
    let Value::Object(built) = mutator.fetch(&built) else { panic!("an array") };
    assert_eq!(mutator.heap().array_length(built), Some(2));
});
```

(b) Matching a fetched value:

```rust
// before
assert!(matches!(interpreter.fetch(&total), Value::SmallInteger(5)));
// after
assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&total), Value::SmallInteger(5))));
```

(c) A test that builds objects and reads them back with no run in between
puts the whole body in one entry:

```rust
// before
let mut interpreter = Interpreter::bootstrap();
let object = interpreter.new_array(vec![Value::SmallInteger(1), Value::SmallInteger(2)]);
assert_eq!(interpreter.heap_reference().array_length(object), Some(2));
assert!(interpreter.heap_mut().array_set(object, 0, Value::SmallInteger(9)));
// after
let mut interpreter = Interpreter::bootstrap();
interpreter.enter(|mutator| {
    let object = mutator.new_array(vec![Value::SmallInteger(1), Value::SmallInteger(2)]);
    assert_eq!(mutator.heap().array_length(object), Some(2));
    assert!(mutator.heap_mut().array_set(object, 0, Value::SmallInteger(9)));
});
```

(d) A handle needed on both sides of a run is rooted before it and fetched
after it:

```rust
// before
let class = interpreter.fetch(&class_root);
let result = expect_ok(interpreter.send(&instance, selector, &[]), "send");
assert!(interpreter.heap_reference().is_class(class_of(class)));
// after
let result = expect_ok(interpreter.send(&instance, selector, &[]), "send");
interpreter.enter(|mutator| {
    let class = mutator.fetch(&class_root);
    assert!(mutator.heap().is_class(class_of(class)));
});
```

(e) An immediate a host builds itself is still stashed directly, which keeps
an argument list inline:

```rust
interpreter.send(&counter, bump, &[interpreter.stash(Value::SmallInteger(2))])  // unchanged
```

(f) A helper that read through `interpreter.heap_reference()` or `interpreter.fetch` takes
`&mut Interpreter` and enters, or — when all its callers are already inside
an entry — takes `&Mutator` (`&Heap` for a pure read) and is called from
there:

```rust
// before
fn expect_error(interpreter: &Interpreter, result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(error)) => error,
        Ok(root) => panic!(
            "{what}: expected a Newspeak error, got {}",
            interpreter.heap_reference().describe(&interpreter.fetch(&root))
        ),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
    }
}
// after
fn expect_error(interpreter: &mut Interpreter, result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(error)) => error,
        Ok(root) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&root)));
            panic!("{what}: expected a Newspeak error, got {got}")
        }
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
    }
}
```

(g) A method that moved from `Interpreter` to `Mutator` is called on `mutator`
inside an entry: `interpreter.decide_send(...)` becomes
`interpreter.enter(|mutator| match mutator.decide_send(...) { ... })`, with the assertions on
the answer inside the closure, since a `Step` holds handles.

**In the runtime's own code** (Tasks 1 and 4), a function that took
`interpreter: &mut Interpreter` or `interpreter: &Interpreter` and does a step's work
takes `mutator: &mut Mutator` or `mutator: &Mutator` instead, and its body says `mutator.`
where it said `interpreter.`. A method in an `impl Interpreter` block that does a
step's work moves, body unchanged, into an `impl Mutator` block in the same
file — `self.heap`, `self.applications` and the rest keep their names,
because they are `Mutator`'s fields now.

---

### Task 0: The branch and the plan

- [ ] **Step 1: Create the branch and commit this plan**

```bash
git checkout master
git pull --ff-only
git checkout -b feature/lifetime-brand-2c
git add docs/superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md
git commit -m "docs: plan step 2c, the lifetime brand" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 1: The step state becomes a `Mutator`, and the run loop enters it once per instruction

No lifetime yet. At the end of this task every step — decision, native,
continuation, instruction — runs on a `&mut Mutator`, `vm::run` and the
safepoint on a `&mut Interpreter`, and the host entries decide inside an
entry and run outside it. A transitional `Deref` keeps every host and test
compiling unchanged.

**Files:**
- Create: `crates/ns-runtime/src/heap/world.rs`
- Modify: `crates/ns-runtime/src/heap.rs` (declare `mod world;`, re-export
  `World`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (the two structs, the
  moved methods, the host entries, `HostStart`, the transitional `Deref`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`,
  `nested.rs`, `sends.rs`, `safepoint.rs`, `locals.rs`, `literals.rs` as
  needed
- Modify: `crates/ns-runtime/src/vm.rs`
- Modify: `crates/ns-runtime/src/builtins/{mod,block,boolean,collections,numbers,object_class,text}.rs`
- Modify: `crates/ns-runtime/src/platform.rs`
- Modify: `crates/ns-runtime/src/image/{mod,decode,encode}.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (export `Mutator`)
- Test: `crates/ns-runtime/src/interpreter/mod.rs`'s test module

**Interfaces:**
- Produces: `pub struct Mutator` (fields `pub(crate)`: `heap`,
  `builtin_classes`, `platform_singleton`, `applications`,
  `empty_class_sides`, `creations_in_progress` (debug), `spare_walks`,
  `lowering`, `arg_buffer`); `pub struct Interpreter { world: World,
  running: bool, stress (debug) }`; `pub fn Interpreter::enter<R>(&mut self,
  f: impl FnOnce(&mut Mutator) -> R) -> R`; `pub(crate) struct World` with
  `new(Lowering)`, `enter`, `activation_depth`, `safepoint_requested`,
  `clear_safepoint_request`, `stash`, and in a debug build
  `set_safepoint_requested`, `traced_size`, `move_epoch`,
  `creations_in_progress`, `move_everything`; the transitional
  `World::mutator`/`mutator_mut` and `impl Deref/DerefMut for Interpreter`
  (Task 3 removes them); `pub fn Mutator::{heap, heap_mut, fetch, stash}`;
  `pub(crate) enum HostStart { Finished(HostResult), Run(ActivationRef) }`;
  `pub(crate) fn Mutator::start(&mut self, decided: Result<Step, Signal>,
  send_args: &[Value]) -> HostStart`; `pub(crate) fn Mutator::to_host(&self,
  result: EvalResult) -> HostResult`; `pub(crate) fn
  Mutator::load_root_class(&mut self, resolved: &ResolvedModule) ->
  ClassRef`; `pub(crate) fn Mutator::bootstrap_builtin_classes(&mut self)`;
  `pub(crate) fn Interpreter::safepoint_requested(&self) -> bool`;
  `vm::run(interp: &mut Interpreter, base: ActivationRef) -> HostResult`.

- [ ] **Step 1: Write the failing test**

Add to the test module at the bottom of
`crates/ns-runtime/src/interpreter/mod.rs`:

```rust
    /// A host reads and allocates through `enter`, which answers what its
    /// closure answers and runs nothing: the stack is as deep afterwards
    /// as before (`docs/design/memory.md`, "The brand").
    #[test]
    fn enter_answers_its_closures_value_and_runs_nothing() {
        let mut interp = Interpreter::bootstrap();
        let len = interp.enter(|m| {
            let array = m.new_array(vec![Value::SmallInt(1), Value::SmallInt(2)]);
            m.heap().array_len(array)
        });
        assert_eq!(len, Some(2));
        assert_eq!(interp.enter(|m| m.heap().activation_depth()), 0);
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p ns-runtime --lib enter_answers_its_closures_value_and_runs_nothing`
Expected: FAIL to compile — `no method named 'enter' found for struct 'Interpreter'`.

- [ ] **Step 3: Create `crates/ns-runtime/src/heap/world.rs`**

```rust
//! The interpreter's step state at rest: a `Mutator` no step is running in.
//! `enter` lends it out for one step and takes it back
//! (`docs/design/memory.md`, "The brand"). What a step can do is
//! `Mutator`'s; what happens between steps -- the safepoint's move, the
//! host roots of immediates -- is here and in `Interpreter`, which no step
//! can reach.
//!
//! A child of `heap`, so that the heap module keeps building and moving a
//! heap to itself: `World` builds the only one an interpreter has, and moves
//! it only while no `Mutator` is lent out.

use std::collections::HashMap;

use crate::compiler::Lowering;
use crate::heap::{Heap, Root};
use crate::interpreter::Mutator;
use crate::types::Value;

pub(crate) struct World {
    mutator: Mutator,
}

impl World {
    /// A bootstrapped world: every builtin class built, nothing else.
    pub(crate) fn new(lowering: Lowering) -> World {
        let mut world = World {
            mutator: Mutator {
                heap: Heap::new(),
                builtin_classes: Vec::new(),
                platform_singleton: None,
                applications: HashMap::new(),
                empty_class_sides: HashMap::new(),
                #[cfg(debug_assertions)]
                creations_in_progress: 0,
                spare_walks: Vec::new(),
                lowering,
                arg_buffer: Vec::new(),
            },
        };
        world.enter(|m| m.bootstrap_builtin_classes());
        world
    }

    /// Lends the step state out for `f`, one step's worth. The run loop
    /// makes one entry per instruction and a host one per read, so this must
    /// inline away.
    #[inline(always)]
    pub(crate) fn enter<R>(&mut self, f: impl FnOnce(&mut Mutator) -> R) -> R {
        f(&mut self.mutator)
    }

    /// How many activations are live; `Interpreter::run_from_host` checks a
    /// run leaves it where it found it.
    pub(crate) fn activation_depth(&self) -> usize {
        self.mutator.heap.activation_depth()
    }

    #[inline(always)]
    pub(crate) fn safepoint_requested(&self) -> bool {
        self.mutator.heap.safepoint_requested()
    }

    pub(crate) fn clear_safepoint_request(&mut self) {
        self.mutator.heap.clear_safepoint_request();
    }

    /// Roots an immediate a host built itself (`Interpreter::stash`).
    pub(crate) fn stash(&self, value: Value) -> Root {
        self.mutator.heap.stash(value)
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_safepoint_requested(&mut self, on: bool) {
        self.mutator.heap.set_safepoint_requested(on);
    }

    #[cfg(debug_assertions)]
    pub(crate) fn traced_size(&self) -> usize {
        self.mutator.heap.traced_size()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn move_epoch(&self) -> u32 {
        self.mutator.heap.move_epoch()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn creations_in_progress(&self) -> usize {
        self.mutator.creations_in_progress
    }

    /// Moves every object and every reified frame (`Heap::move_everything`)
    /// and forwards what the step state holds beside the heap: the
    /// bootstrap's builtin classes and the `platform` singleton.
    #[cfg(debug_assertions)]
    pub(crate) fn move_everything(&mut self) {
        use crate::trace::Tracer;
        self.check_quiescent_state();
        let m = &mut self.mutator;
        let mut forwarder = m.heap.move_everything();
        for class in &mut m.builtin_classes {
            forwarder.object(class);
        }
        if let Some(platform) = &mut m.platform_singleton {
            forwarder.value(platform);
        }
    }

    /// What must hold no handle when everything moves, because nothing
    /// traces it: the argument buffer, which a send fills and empties within
    /// one instruction, and the spare boxes finished instantiations leave
    /// for the next ones (`docs/design/memory.md`, "What the collector
    /// traces").
    #[cfg(debug_assertions)]
    fn check_quiescent_state(&self) {
        use crate::types::InstantiationPhase;
        let m = &self.mutator;
        assert!(
            m.arg_buffer.is_empty(),
            "internal: the argument buffer holds values between instructions"
        );
        assert!(
            m.spare_walks.iter().all(|walk| {
                matches!(walk.instance, Value::Nil)
                    && walk.levels.is_empty()
                    && matches!(walk.phase, InstantiationPhase::Descending { .. })
            }),
            "internal: a spare instantiation box holds a handle"
        );
    }

    /// Transitional, for the `Deref` Task 3 of the 2c plan removes.
    pub(crate) fn mutator(&self) -> &Mutator {
        &self.mutator
    }

    /// Transitional, for the `DerefMut` Task 3 of the 2c plan removes.
    pub(crate) fn mutator_mut(&mut self) -> &mut Mutator {
        &mut self.mutator
    }
}
```

In `crates/ns-runtime/src/heap.rs`, below the `use` lines at the top, add:

```rust
mod world;
pub(crate) use world::World;
```

(`src/heap.rs` with `mod world;` finds `src/heap/world.rs`; no `mod.rs` is
needed.)

- [ ] **Step 4: Split `Interpreter` into `Interpreter` and `Mutator`**

In `crates/ns-runtime/src/interpreter/mod.rs`, replace `pub struct
Interpreter { ... }` with the two structs below. Every field keeps its doc
comment from the old struct; `heap` keeps its own.

```rust
/// Everything a step of the interpreter works on -- the heap and the
/// bootstrap's builtin classes, the caches of behaviour, the argument
/// buffer and the spare instantiation boxes -- and every decision, native
/// and continuation that runs one. A host reaches it only through
/// `Interpreter::enter`, and nothing here reaches back: no method takes or
/// answers the `Interpreter`, so no step can run the loop, reach its
/// safepoint, or start a host's send (`docs/design/memory.md`, "The
/// brand").
pub struct Mutator {
    pub(crate) heap: Heap,
    /// Every builtin class object, indexed by `BuiltinClass as usize`.
    /// Indexed rather than hashed because `class_of` asks for one on every
    /// send to a primitive receiver. Empty only while `bootstrap` is
    /// building it; nothing can reach a `Mutator` in that state.
    pub(crate) builtin_classes: Vec<ClassRef>,
    pub(crate) platform_singleton: Option<Value>,
    // `applications`, `empty_class_sides`, `creations_in_progress`,
    // `spare_walks`, `lowering`, `arg_buffer`: moved here from
    // `Interpreter` with their doc comments and attributes unchanged, each
    // `pub(crate)`.
}

/// A host's interpreter: the step state at rest (`World`), and what only
/// the run loop between steps needs -- whether a run is in progress, and
/// the stress mode's schedule. Its methods are the host's entries: `enter`
/// for a step's worth of reading and allocating, and `send`,
/// `instantiate`, `load_module` and `platform`, which take and answer
/// roots.
pub struct Interpreter {
    world: crate::heap::World,
    /// Whether a host's run of the loop is in progress (`run_from_host`).
    /// Nothing the loop calls can start another -- no decision, native or
    /// continuation runs code from Rust -- and a debug build checks that.
    running: bool,
    /// The stress mode's schedule, when it is on (`NS_NEXT_STRESS_MOVES`, or
    /// `set_stress_moves`). Debug builds only.
    #[cfg(debug_assertions)]
    stress: Option<safepoint::StressMoves>,
}
```

(Write the six moved fields out in full in the file; the comment above only
stands for them in this plan.)

`bootstrap_with` becomes:

```rust
    pub fn bootstrap_with(lowering: Lowering) -> Interpreter {
        let mut interp = Interpreter {
            world: crate::heap::World::new(lowering),
            running: false,
            #[cfg(debug_assertions)]
            stress: safepoint::StressMoves::from_env(),
        };
        #[cfg(debug_assertions)]
        {
            let on = interp.stress.is_some();
            interp.world.set_safepoint_requested(on);
        }
        interp
    }
```

`build_builtin_classes(interp: &mut Interpreter)` becomes
`build_builtin_classes(m: &mut Mutator)`, its body saying `m.` for
`interp.`, and `Mutator` gains:

```rust
    /// Builds every builtin class into an empty step state. `World::new`
    /// calls it once, in the first entry the world makes.
    pub(crate) fn bootstrap_builtin_classes(&mut self) {
        self.builtin_classes = build_builtin_classes(self);
    }
```

- [ ] **Step 5: Move every step method onto `Mutator`**

Following "In the runtime's own code" under "The conversion rules": in
`interpreter/mod.rs`, these move from `impl Interpreter` into `impl Mutator`,
bodies unchanged and visibility unchanged: `builtin_class`,
`advance_identity_hash_past`, `decide_send`,
`dispatch_native_inheriting_object`, `decide_native_or_dnu`,
`decide_does_not_understand`, `take_arg_buffer`, `return_arg_buffer`,
`push`, `push_step`, `push_initializer`, `resume`, `abandon`,
`push_continuation`, `catches_return`, `raise_return`, `class_of`,
`metaclass_of`, `builtin_class_for_primitive`, `new_instance_object`,
`new_array`, `new_string`, `new_symbol`, `new_character`, `new_closure`,
`platform_value`, `new_stream_object`, `new_collections_namespace`,
`new_class_object`, `apply_mixin`, `empty_class_side_mixin`, `to_host`
(becoming `pub(crate)`). In `interpreter/instantiate.rs`, every method of its
`impl Interpreter` block except `load_module` and `instantiate` moves into an
`impl Mutator` block. `interpreter/nested.rs`, `sends.rs`, `builtins/*.rs`,
`platform.rs` and `image/decode.rs`/`encode.rs` take `m: &mut Mutator` /
`m: &Mutator` where they took `interp: &mut Interpreter` /
`interp: &Interpreter`; `image::decode::DecodeContext`'s `interp` field
becomes `m: &'a mut Mutator`.

Add to `impl Mutator`, for hosts:

```rust
    /// The heap, for reading, inside an entry.
    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    /// The heap, for writing, inside an entry.
    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    /// What `root` holds now. Good until this entry ends: the next run may
    /// move it, so read the root again in the next entry.
    pub fn fetch(&self, root: &Root) -> Value {
        self.heap.fetch(root)
    }

    /// Roots `value`, so that a host can hold it past this entry.
    pub fn stash(&self, value: Value) -> Root {
        self.heap.stash(value)
    }
```

and the start of a host's run:

```rust
    /// Pushes what a host's send or instantiation decided, in the entry
    /// that decided it: `send_args` are the send's arguments, which
    /// `StepArgs::Send` names and which do not outlive the entry. A value,
    /// a signal, or a push refused at the depth limit finishes here, rooted
    /// for the host.
    pub(crate) fn start(
        &mut self,
        decided: Result<Step, Signal>,
        send_args: &[Value],
    ) -> HostStart {
        match decided {
            Ok(Step::Value(value)) => HostStart::Finished(self.to_host(Ok(value))),
            Ok(step) => match self.push_step(step, send_args, None, false) {
                Ok(act) => HostStart::Run(act),
                Err(signal) => HostStart::Finished(self.to_host(Err(signal))),
            },
            Err(signal) => HostStart::Finished(self.to_host(Err(signal))),
        }
    }
```

with, beside `HostSignal`:

```rust
/// What deciding a host's send or instantiation left for the loop.
pub(crate) enum HostStart {
    /// Nothing to run: the answer, rooted.
    Finished(HostResult),
    /// An activation, pushed, for `vm::run`.
    Run(ActivationRef),
}
```

- [ ] **Step 6: Rewrite the host entries on `Interpreter`**

In `interpreter/mod.rs`:

```rust
    /// Runs `f` with the interpreter's step state: its heap, its builtin
    /// classes, and every method a step uses. What `f` answers is what
    /// this answers. `f` runs no Newspeak code; a host runs code with
    /// `send` and `instantiate`, outside every entry.
    #[inline(always)]
    pub fn enter<R>(&mut self, f: impl FnOnce(&mut Mutator) -> R) -> R {
        self.world.enter(f)
    }

    /// Roots an immediate -- `nil`, a Boolean, a `SmallInteger`, a `Float`
    /// -- a host built itself, outside any entry. An object is rooted with
    /// `Mutator::stash`, inside the entry that reached it.
    pub fn stash(&self, value: Value) -> Root {
        self.world.stash(value)
    }

    /// Whether the run loop's next safepoint must take its slow path.
    #[inline(always)]
    pub(crate) fn safepoint_requested(&self) -> bool {
        self.world.safepoint_requested()
    }

    pub fn send(&mut self, receiver: &Root, selector: Symbol, args: &[Root]) -> HostResult {
        let depth = self.world.activation_depth();
        let start = self.enter(|m| {
            let receiver = m.fetch(receiver);
            let args: Vec<Value> = args.iter().map(|arg| m.fetch(arg)).collect();
            let decided = m.decide_send(receiver, selector, &args);
            m.start(decided, &args)
        });
        self.run_from_host(start, depth)
    }

    /// Runs what a host's send or instantiation started. It is the only way
    /// Rust enters the run loop (`docs/design/execution.md`, "No re-entry
    /// from Rust"). `depth` is the stack's depth before the host's entry
    /// decided anything.
    ///
    /// Three things are checked in a debug build, because each is otherwise
    /// silent. The loop is never entered while it is running -- nothing it
    /// calls can reach here. The stack is left as deep as it was found: a
    /// leaked entry would put a later activation on top of a dead one's
    /// registers (a leak inside a run is caught too, by
    /// `Heap::pop_activation`'s check that it pops the top). And no nested
    /// class's creation outlives the run that began it.
    fn run_from_host(&mut self, start: HostStart, depth: usize) -> HostResult {
        debug_assert!(
            !self.running,
            "internal: the run loop was re-entered from Rust"
        );
        let result = match start {
            HostStart::Finished(result) => result,
            HostStart::Run(act) => {
                self.running = true;
                let result = crate::vm::run(self, act);
                self.running = false;
                result
            }
        };
        debug_assert_eq!(
            self.world.activation_depth(),
            depth,
            "a host's send or instantiation must leave the activation stack as it found it"
        );
        #[cfg(debug_assertions)]
        assert_eq!(
            self.world.creations_in_progress(),
            0,
            "internal: a nested class's creation outlived the run that began it"
        );
        result
    }

    /// The `platform` object, rooted for a host.
    pub fn platform(&mut self) -> Root {
        self.enter(|m| {
            let platform = m.platform_value();
            m.stash(platform)
        })
    }
```

`heap_ref`, `heap_mut` and `fetch` stay on `Interpreter` for now, each
delegating through the transitional `Deref` below (`&self.heap`,
`&mut self.heap`, `self.heap.fetch(root)`); Task 3 removes all three.

In `interpreter/instantiate.rs`, the two host entries become:

```rust
    pub fn load_module(&mut self, resolved: &ResolvedModule) -> Root {
        self.enter(|m| {
            let class = m.load_root_class(resolved);
            m.stash(Value::Obj(class))
        })
    }

    pub fn instantiate(&mut self, class: &Root, factory_args: &[Root]) -> HostResult {
        let depth = self.world.activation_depth();
        let start = self.enter(|m| {
            let value = m.fetch(class);
            let class = match value {
                Value::Obj(obj) if m.heap.is_class(obj) => obj,
                _ => {
                    return HostStart::Finished(Err(HostSignal::Error(HostError {
                        selector: Symbol::NEW,
                        receiver: m.stash(value),
                        message: format!(
                            "cannot instantiate {}: it is not a class",
                            m.heap.describe(&value)
                        ),
                    })))
                }
            };
            let factory_args: Vec<Value> = factory_args.iter().map(|arg| m.fetch(arg)).collect();
            let decided = m.decide_instantiation(class, factory_args);
            m.start(decided, &[])
        });
        self.run_from_host(start, depth)
    }
```

keeping both doc comments, and the old `load_module` body becomes
`Mutator::load_root_class(&mut self, resolved: &ResolvedModule) ->
ClassRef`, which answers `class` where the old body stashed it. `world` is
private to `interpreter/mod.rs`, which `instantiate.rs` is a child of, so it
reads it directly.

In `interpreter/safepoint.rs`, `Interpreter::safepoint` says
`self.world.traced_size()`, `self.world.move_everything()` and
`self.world.clear_safepoint_request()`; `Interpreter::move_everything` and
`check_quiescent_state` are deleted (they are `World`'s now); `move_now`
calls `self.world.move_everything()`, `set_stress_moves` calls
`self.world.set_safepoint_requested(on)`, and `moves` answers
`self.world.move_epoch()`.

- [ ] **Step 7: The run loop enters once per instruction**

In `crates/ns-runtime/src/vm.rs`, `Flow::Exit` carries a `HostResult`, every
function but `run` and `poll` takes `m: &mut Mutator` in place of `interp:
&mut Interpreter` (`send`'s `decide` becomes `impl FnOnce(&mut Mutator,
&[Value]) -> Result<Step, Signal>`), and the two places that make an
`Exit` root their answer in the entry that has it: `finish`'s `return
Flow::Exit(Ok(value))` becomes `return Flow::Exit(m.to_host(Ok(value)))`, and
`unwind`'s `Flow::Exit(Err(signal))` becomes
`Flow::Exit(m.to_host(Err(signal)))`. `run` and `poll` become:

```rust
/// Runs `base` -- the top of the activation stack, pushed by the caller --
/// and every activation it pushes, until `base` is done, and pops them all,
/// `base` included, on every path out: a return, an error, or a non-local
/// return aimed below `base`. The stack is exactly as deep afterwards as it
/// was before `base` was pushed. Its safepoints (`poll`) are every switch to
/// another activation and every backward jump.
///
/// Each instruction is one entry (`Interpreter::enter`), and the safepoint
/// is outside every one: what the loop carries from one instruction to the
/// next is the running `ActivationRef`, its code and its `pc`, none of which
/// a move changes, and a run ends inside its last entry, its answer rooted
/// (`docs/design/memory.md`, "The brand").
///
/// An activation's `pc` is stored only by the instructions that can hand it
/// to an observer: every send (a callee runs above it, and the loop resumes
/// it from that `pc`), `MakeClosure` (it reifies the activation, so the frame
/// starts from the `pc` stored just before), and `NonLocalReturn` (control
/// leaves with a signal). Nothing reads `pc` while a straight-line
/// instruction executes.
pub(crate) fn run(interp: &mut Interpreter, base: ActivationRef) -> HostResult {
    let (floor, mut code) = interp.enter(|m| {
        (
            m.heap.activation_position(base),
            m.heap.activation_code(base),
        )
    });
    let mut act = base;
    let mut pc = 0usize;
    loop {
        match interp.enter(|m| execute(m, act, &code, pc, floor)) {
            Flow::Next => {
                pc += 1;
                debug_assert!(
                    pc < code.instrs.len(),
                    "internal: pc ran past the end of a CompiledCode with no terminating return"
                );
            }
            Flow::Jump(target) => {
                // A backward jump closes a loop, so that a loop that makes no
                // send still passes a safepoint.
                if target <= pc {
                    poll(interp);
                }
                pc = target;
            }
            Flow::Switch {
                act: next,
                code: next_code,
                pc: next_pc,
            } => {
                act = next;
                code = next_code;
                pc = next_pc;
                poll(interp);
            }
            Flow::Exit(result) => return result,
        }
    }
}

#[inline(always)]
fn poll(interp: &mut Interpreter) {
    if interp.safepoint_requested() {
        interp.safepoint();
    }
}
```

(`poll` keeps its doc comment.) Imports: `use crate::interpreter::{EvalResult,
HostResult, Interpreter, Mutator, Signal, Step};`.

- [ ] **Step 8: `image::load` decodes inside one entry; `snapshot` takes a `&Mutator`**

In `crates/ns-runtime/src/image/mod.rs`, `snapshot`'s first parameter
becomes `m: &Mutator` and its body says `let heap = &m.heap;` and passes `m`
to `encode::encode_tag_named_hashes` (whose parameter becomes `m: &Mutator`).
`load` keeps its reading of the header, the source and the origin outside
any entry, then does everything from `symbols::decode` to the rooting inside
one:

```rust
    let mut interp = Interpreter::bootstrap();
    let decoded = interp.enter(|m| -> Result<_, String> {
        let symbols = symbols::decode(&mut r).map_err(|e| e.to_string())?;
        // ... every line of today's body from `decode_code_table` through
        // the trailing-bytes check, unchanged but for `&interp` -> `m` and
        // `&mut interp` -> `m` ...
        let root_class = m.stash(Value::Obj(root_class));
        let roots = graph.roots.into_iter().map(|root| m.stash(root)).collect();
        Ok((root_class, roots, entry, header.stateless))
    })?;
```

and builds the `LoadedImage` from `decoded` and `interp` after it. Today's
comment "Rooted only now, after every verification pass …" stays above the
two `stash` lines. (The ellipsis stands for the existing body, which moves
into the closure unchanged except for the two substitutions named; nothing
is added or removed in it.) `header` is read before the entry, so the
closure borrows it; `r` is borrowed mutably by the closure and not used
after it.

- [ ] **Step 9: The transitional `Deref`**

At the bottom of `interpreter/mod.rs`'s non-test code:

```rust
/// Transitional, for Tasks 1-3 of the 2c plan: lets a host or test that
/// still reads through the interpreter compile until it reads through
/// `enter`. Task 3 removes it, with `heap_ref`, `heap_mut` and `fetch`.
impl std::ops::Deref for Interpreter {
    type Target = Mutator;
    fn deref(&self) -> &Mutator {
        self.world.mutator()
    }
}

impl std::ops::DerefMut for Interpreter {
    fn deref_mut(&mut self) -> &mut Mutator {
        self.world.mutator_mut()
    }
}
```

In `lib.rs`: `pub use interpreter::{HostError, HostResult, HostSignal,
Interpreter, Mutator};`.

- [ ] **Step 10: Build, and run the test**

Run: `cargo build --workspace`, then
`cargo test -p ns-runtime --lib enter_answers_its_closures_value_and_runs_nothing`
Expected: builds warning-free; the test PASSES.

If a call site outside the files listed still names a moved method on an
`Interpreter`, the `Deref` resolves it; a compile error means a signature
this task changed (a `&mut Interpreter` parameter became `&mut Mutator`) —
change the caller to pass `m` from an entry; in host code that Task 2 or 3
converts anyway, `&mut interp` also compiles, because the transitional
`DerefMut` coerces it to a `&mut Mutator`.

- [ ] **Step 11: Run every gate**

Run the gates from "Global Constraints", the two stress runs included.
Expected: all green, the reports exactly as stated.

- [ ] **Step 12: Measure**

Measure with the protocol, the parent being `4999d05`. The per-instruction
`enter` is `#[inline(always)]` down to a field borrow, so every source is
expected level. If one is slower beyond noise, check first that
`World::enter` and the closure in `run` were inlined (`cargo rustc --release
-p ns-runtime -- --emit asm` and look for a call to `enter` inside `run`),
then report to the controller.

- [ ] **Step 13: Commit**

```bash
git add -A crates/ns-runtime
git commit -m "refactor: the step state becomes a Mutator the run loop enters per instruction" -m "The interpreter's heap, builtin classes, caches, argument buffer and
spare instantiation boxes move into a Mutator, held at rest by a World
(heap/world.rs). Every decision, native and continuation runs on a
&mut Mutator; vm::run and the safepoint stay on the Interpreter, and each
instruction is one enter. Host entries decide inside an entry and run
outside it; a run's answer is rooted in its last entry. A transitional
Deref keeps hosts and tests compiling until they read through enter.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 2: The CLI and every test but `tests/image.rs` read through `enter`

Nothing in the runtime changes. Every host read goes through
`interp.enter`, following "The conversion rules" to the letter, so that
Task 4's brand has nothing left to find here.

**Files:**
- Modify: `crates/ns-cli/src/entry.rs`, `commands/bench.rs`,
  `commands/build.rs`, `commands/disasm.rs`, `commands/snapshot.rs`,
  `commands/load.rs` (if it reads through the interpreter)
- Modify: `crates/ns-cli/tests/disasm_snapshots.rs`
- Modify: every file under `crates/ns-runtime/tests/` except `image.rs`
- Modify: the test module of `crates/ns-runtime/src/interpreter/mod.rs`

**Interfaces:**
- Consumes: `Interpreter::enter`, `Interpreter::stash` (immediates),
  `Mutator::{heap, heap_mut, fetch, stash}` and the step methods now on
  `Mutator` (Task 1).
- Produces: `entry::class_ref(m: &Mutator, class: &Root) -> ClassRef` —
  called inside an entry; `entry::factory_args_for` unchanged in
  signature (`&Heap`), called inside an entry.

- [ ] **Step 1: Convert the CLI**

`entry.rs`: `class_ref` takes `m: &ns_runtime::Mutator` and reads
`m.fetch(class)`. `instantiate_root` becomes:

```rust
pub(crate) fn instantiate_root(
    interp: &mut ns_runtime::Interpreter,
    class: &ns_runtime::Root,
) -> Result<ns_runtime::Root, ExitCode> {
    let platform = interp.platform();
    let factory_args = interp
        .enter(|m| factory_args_for(m.heap(), class_ref(m, class), &platform))
        .map_err(|msg| {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        })?;
    interp
        .instantiate(class, &factory_args)
        .map_err(report_signal)
}
```

`entry_args` keeps `interp.stash(ns_runtime::Value::Nil)` (an immediate).
`bench.rs`'s `bench_entry` computes `factory_args` the same way.
`build.rs` calls `root_class_answers` and `ns_runtime::image::build` inside
one `interp.enter(|m| ...)` each, passing `m.heap()` and `class_ref(m,
&class)`. `snapshot.rs` fetches `platform` and the roots and calls
`ns_runtime::image::snapshot(m, …)` inside one entry:

```rust
    let bytes = match interp.enter(|m| {
        let platform = m.fetch(&platform);
        let root_values: Vec<ns_runtime::Value> = roots.iter().map(|r| m.fetch(r)).collect();
        ns_runtime::image::snapshot(
            m,
            // ... the remaining arguments exactly as today, with
            // `entry::class_ref(m, &class)` for the class ...
            &platform,
            &root_values,
        )
    }) {
```

(The comment stands for today's arguments, which do not change.)
`disasm.rs` reads through `loaded.interp.enter(...)` and `interp.enter(...)`
where it read through `heap_ref()`; destructure the `LoadedImage` first
(`let LoadedImage { mut interp, root_class, .. } = loaded;`) so the entry's
`&mut interp` and the closure's `&root_class` borrow different variables.
Keep the comment in `disasm.rs` that explains why it reads through the
interpreter that built the class object, saying "reads through
`interp.enter`" where it said "`interp.heap_ref()`".

- [ ] **Step 2: Build the CLI and run its tests**

Run: `cargo build -p ns-cli`, then `cargo test -p ns-cli`
Expected: builds; every test PASSES, `program_outcomes.rs` included — the
CLI's output is unchanged.

- [ ] **Step 3: Convert the runtime's test files but `image.rs`**

File by file — `builtin_object_inheritance.rs`,
`builtins_boolean_numbers.rs`, `builtins_collections_block.rs`,
`builtins_control_flow.rs`, `builtins_text.rs`, `compiler.rs`,
`factory_send.rs`, `heap.rs`, `instantiate.rs`, `interpreter.rs`,
`mixin.rs`, `nested_classes.rs`, `platform.rs`, `reference_lowering.rs`,
`roots.rs`, `stress.rs`, `trampoline.rs`, `vm_initializer.rs`, `vm.rs`, and
`crates/ns-cli/tests/disasm_snapshots.rs` — replace every `heap_ref()`,
`heap_mut()` and `interp.fetch(` and every call of a method that moved to
`Mutator` with the patterns (a)–(g). Then the test module of
`interpreter/mod.rs`: `interp.decide_send(…)` becomes
`interp.enter(|m| match m.decide_send(…) { … })` with every assertion on the
decision inside the closure; `interp.heap.activation_depth()` becomes
`interp.enter(|m| m.heap().activation_depth())`; `interp.spare_walks` and
`interp.creations_in_progress` become reads inside an entry
(`interp.enter(|m| m.spare_walks.len())`, and the pointer comparison of
`a_finished_walk_leaves_an_empty_box_for_the_next` takes the box's address
inside an entry as a `*const Instantiation`, which holds no handle).

Keep every test's name, doc comment and assertions; only the access path
changes. Two tests in `roots.rs` need a word:

- `fetching_a_root_through_another_interpreter_panics` becomes
  `let _ = other.enter(|m| m.fetch(&class));` with `other` declared `mut`.
- `a_value_kept_past_a_move_is_refused` keeps a fetched value across a move
  on purpose. Convert it literally — `let kept = interp.enter(|m|
  m.fetch(&counter)); interp.move_now(); let _ = interp.enter(|m|
  m.heap().describe(&kept));` — even though that breaks the rule: it is the
  one test whose point is the mistake. Task 4 deletes it, because under the
  brand it no longer compiles, and replaces it with a `compile_fail`
  doctest. Say so in a one-line comment above it.

- [ ] **Step 4: Check that nothing converted still reads through the interpreter**

Run: `rg -n "heap_ref\(\)|interp\.heap_mut\(\)|interp\.fetch\(|other\.fetch\(" crates --glob "!crates/ns-runtime/tests/image.rs"`
Expected: no hit. (`m.heap_mut()` and `m.fetch(` inside an entry are the
converted form and do not match; the transitional definitions on
`Interpreter` are `fn heap_ref(&self)`, `fn heap_mut(&mut self)` and
`fn fetch(&self, …)`, which do not match either.)

- [ ] **Step 5: Run every gate**

Run the gates from "Global Constraints", the two stress runs included.
Expected: all green, the reports exactly as stated.

- [ ] **Step 6: Commit**

```bash
git add -A crates
git commit -m "test: hosts and tests read through Interpreter::enter" -m "The CLI and every test but tests/image.rs reach the heap and the step
methods only inside an entry, and let nothing that holds a handle out of
one: a value that outlives an entry leaves it as a Root.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 3: `tests/image.rs` reads through `enter`, and the transitional API goes

**Files:**
- Modify: `crates/ns-runtime/tests/image.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (remove the `Deref`,
  `DerefMut`, `heap_reference`, `heap_mut`, `fetch`)
- Modify: `crates/ns-runtime/src/heap/world.rs` (remove `mutator`,
  `mutator_mut`)
- Modify: any file the removal shows still depends on them

**Interfaces:**
- Consumes: as Task 2.
- Produces: the final host API of decision 7 — nothing reaches a `Mutator`
  but `Interpreter::enter`.

- [ ] **Step 1: Convert `tests/image.rs`**

The same patterns (a)–(g), throughout its 6,300 lines. Its shapes, and how
each converts:

- A test that loads, instantiates, sends, then snapshots: the snapshot call
  moves into one entry that fetches the class, the platform and the roots
  and calls `image::snapshot(mutator, …)`; the bytes leave the entry.
- A test that builds frames or objects by hand (`allocate_frame`,
  `allocate_object`, `push_activation`, `set_frame_program_counter`, `replace_frame`) and
  then encodes or inspects them: the whole construction and the encoding
  share one entry, since nothing runs between them.
- A round-trip check after `image::load`: every read of the loaded world
  happens inside `loaded.interpreter.enter(…)`, the roots fetched there.
- A helper that takes `&Interpreter` or `&Heap` and is called from outside
  any entry: it takes `&Mutator` or `&Heap` and is called from inside one,
  or takes `&mut Interpreter` and enters itself — whichever keeps its
  callers' code shortest.

Work through the file top to bottom, building after each few tests
(`cargo test -p ns-runtime --test image --no-run`), and run it when it
builds: `cargo test -p ns-runtime --test image`. Expected: every test
PASSES, with the same count as before the task.

- [ ] **Step 2: Remove the transitional API**

Delete from `interpreter/mod.rs` the `impl Deref for Interpreter`, the
`impl DerefMut for Interpreter`, and `Interpreter::heap_reference`, `heap_mut`
and `fetch`; delete `World::mutator` and `World::mutator_mut`.

Run: `cargo build --workspace --all-targets`
Expected: builds. Any error is a caller that still reached the `Mutator`
through the interpreter: convert it by the rules — in the runtime, an
`interpreter.accessor()` in host code becomes `self.enter(|mutator| mutator.accessor())` or moves into an
existing entry.

- [ ] **Step 3: Check that the old paths are gone**

Run: `rg -n "heap_reference|fn heap_mut|fn fetch|fn mutator|impl std::ops::Deref" crates`
Expected: exactly three hits — `fn heap_mut` and `fn fetch` in
`interpreter/mod.rs`'s `impl Mutator`, and `fn fetch` in `heap.rs`'s
`impl Heap`. Nothing on `Interpreter`, nothing on `World`, no `heap_reference`
anywhere.

- [ ] **Step 4: Run every gate**

Run the gates from "Global Constraints", the two stress runs included.
Expected: all green, the reports exactly as stated.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "refactor: only Interpreter::enter reaches the step state" -m "tests/image.rs reads through enter like every other host, and the
transitional Deref, heap_reference, heap_mut and Interpreter::fetch go. A host
keeps a value across a run only as a Root and reads it inside an entry.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 4: The brand

Every movable handle and every type that holds one gets `'gc`; `World::enter`
lends `Mutator<'static>` as `Mutator<'gc>`; the three mistakes become
compile errors, pinned by doctests. This task changes types and nothing
else: no function's behaviour, no test's assertion.

**Files:**
- Modify: `crates/ns-runtime/src/lib.rs` (`#![deny(unsafe_code)]`)
- Modify: `crates/ns-runtime/src/heap.rs`, `heap/world.rs`, `types.rs`,
  `trace.rs`, `dispatch.rs`, `vm.rs`, `platform.rs`
- Modify: `crates/ns-runtime/src/interpreter/*.rs`, `builtins/*.rs`,
  `image/*.rs`
- Modify: `crates/ns-cli/src/**` where the compiler asks
- Modify: `crates/ns-runtime/tests/roots.rs` (delete
  `a_value_kept_past_a_move_is_refused`) and any test the compiler shows
  lets a handle out of an entry
- Test: the doctests on `Interpreter::enter` (`interpreter/mod.rs`)

**Interfaces:**
- Consumes: the final host API of Task 3.
- Produces: `ObjectReference<'gc>`, `ClassReference<'gc>`, `FrameReference<'gc>`, `Value<'gc>`,
  `Object<'gc>`, `ObjectData<'gc>`, `ClosureData<'gc>`, `ClassData<'gc>`,
  `FrameKind<'gc>`, `Continuation<'gc>`, `WhileLoop<'gc>`, `DoLoop<'gc>`,
  `NestedClassCreation<'gc>`, `Instantiation<'gc>`,
  `InstantiationPhase<'gc>`, `Frame<'gc>`, `NewspeakError<'gc>`,
  `Heap<'gc>`, `ActivationHeader<'gc>`, `Popped<'gc>`, `Mutator<'gc>`,
  `Signal<'gc>`, `EvaluationResult<'gc>`, `Step<'gc>`, `StepArguments<'gc>`,
  `Lookup<'gc>`, and every private type the compiler names;
  `pub fn Interpreter::enter<R>(&mut self, body: impl for<'gc> FnOnce(&mut
  Mutator<'gc>) -> R) -> R`; `pub fn Interpreter::stash(&self, value:
  Value<'static>) -> Root`. Unbranded: `ActivationReference`, `Root`,
  `HostResult`, `HostSignal`, `HostError`, `HostStart`, `LoadedImage`,
  `CompiledCode`, `Mixin`, `MixinApplication`, `MixinMethod`, `Symbol`.

- [ ] **Step 1: Write the doctests on `Interpreter::enter`**

Replace `Interpreter::enter`'s doc comment with the one below. The three
plain blocks run today. Of the three `compile_fail` blocks, only the first
fails the way a red test should — it compiles today, since nothing is
branded yet. The second fails to compile today for the wrong reason
(`Value` takes no lifetime argument yet), and the third, the re-entry,
already fails for the right one (the double borrow of `interpreter`), because
Task 1 made it so. That is why Step 8 checks each block's actual error once
the brand is in.

```rust
    /// Runs `body` with the interpreter's step state: its heap, its builtin
    /// classes, and every method a step uses. What `body` answers is what this
    /// answers. `body` runs no Newspeak code; a host runs code with `send` and
    /// `instantiate`, outside every entry.
    ///
    /// Every handle `body` reaches carries the lifetime `'gc`, which `body` is
    /// generic over and `R` cannot name, so no handle leaves an entry: a run
    /// may move every object, and a handle kept across one would name
    /// whatever took its slot (`docs/design/memory.md`, "The brand"). A host
    /// keeps a value across a run as a `Root`, and reads it in the next
    /// entry:
    ///
    /// ```
    /// use ns_runtime::{Interpreter, Symbol, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let root = interpreter.enter(|mutator| {
    ///     let array = mutator.new_array(vec![Value::SmallInteger(1)]);
    ///     mutator.stash(Value::Object(array))
    /// });
    /// let _ = interpreter.send(&root, Symbol::intern("size"), &[]);
    /// let length = interpreter.enter(|mutator| match mutator.fetch(&root) {
    ///     Value::Object(array) => mutator.heap().array_length(array),
    ///     _ => None,
    /// });
    /// assert_eq!(length, Some(1));
    /// ```
    ///
    /// Keeping the handle itself instead does not compile:
    ///
    /// ```compile_fail
    /// use ns_runtime::{Interpreter, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let array = interpreter.enter(|mutator| mutator.new_array(vec![Value::SmallInteger(1)]));
    /// ```
    ///
    /// Nor does storing one where a host keeps its own state; a host's
    /// field holds a `Root`:
    ///
    /// ```
    /// use ns_runtime::{Interpreter, Root, Value};
    /// struct Host { kept: Option<Root> }
    /// let mut interpreter = Interpreter::bootstrap();
    /// let mut host = Host { kept: None };
    /// interpreter.enter(|mutator| {
    ///     let array = mutator.new_array(vec![]);
    ///     host.kept = Some(mutator.stash(Value::Object(array)));
    /// });
    /// assert!(host.kept.is_some());
    /// ```
    ///
    /// ```compile_fail
    /// use ns_runtime::{Interpreter, Value};
    /// struct Host<'a> { kept: Option<Value<'a>> }
    /// let mut interpreter = Interpreter::bootstrap();
    /// let mut host = Host { kept: None };
    /// interpreter.enter(|mutator| {
    ///     let array = mutator.new_array(vec![]);
    ///     host.kept = Some(Value::Object(array));
    /// });
    /// ```
    ///
    /// And a step cannot run the loop: the interpreter is borrowed by the
    /// entry for as long as `body` runs, and `Mutator` offers no way back to
    /// it, so a native, which is handed only a `Mutator`, cannot re-enter
    /// the loop either. A host runs code between entries:
    ///
    /// ```
    /// use ns_runtime::{Interpreter, Symbol, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let three = interpreter.stash(Value::SmallInteger(3));
    /// let printed = interpreter.send(&three, Symbol::intern("printString"), &[]);
    /// assert!(printed.is_ok());
    /// ```
    ///
    /// ```compile_fail
    /// use ns_runtime::{Interpreter, Symbol, Value};
    /// let mut interpreter = Interpreter::bootstrap();
    /// let three = interpreter.stash(Value::SmallInteger(3));
    /// interpreter.enter(|_mutator| {
    ///     let _ = interpreter.send(&three, Symbol::intern("printString"), &[]);
    /// });
    /// ```
```

Run: `cargo test -p ns-runtime --doc Interpreter::enter`
Expected: FAIL — rustdoc reports "Test compiled successfully, but it's
marked `compile_fail`" for the first `compile_fail` block; the other five
pass (the second for the wrong reason, as said above).

- [ ] **Step 2: Brand the handles in `heap.rs`**

Above `ObjectReference`:

```rust
/// The brand every movable handle carries (`docs/design/memory.md`, "The
/// brand"). Invariant in `'gc` -- a `fn(&'gc ()) -> &'gc ()` is
/// contravariant in its argument and covariant in its result -- so the
/// compiler can neither shorten nor extend it, and a handle minted in one
/// entry has a type no other entry can name. Zero-sized: a release handle
/// is still exactly its 32-bit index.
type Brand<'gc> = std::marker::PhantomData<fn(&'gc ()) -> &'gc ()>;
```

`ObjectReference` and `FrameReference` each gain the field `brand: Brand<'gc>` (last, after
the two debug fields) and the parameter: `pub struct ObjectReference<'gc>`, `pub
struct FrameReference<'gc>`, `pub type ClassReference<'gc> = ObjectReference<'gc>;`, `impl<'gc>
ObjectReference<'gc> { pub fn same(self, other: ObjectReference<'gc>) -> bool … }`, `impl
std::fmt::Debug for ObjectReference<'_>`, and the same for `FrameReference`. The two
constructors `object_handle` and `frame_handle` set `brand:
std::marker::PhantomData`.

Every stored type takes the parameter: `ActivationHeader<'gc>`,
`Popped<'gc>`, `StackEntry<'gc>`, `ActivationStack<'gc>`, `FrameSlot<'gc>`,
`pub struct Heap<'gc>`, and `impl<'gc> Heap<'gc> { … }` for every accessor —
each `Value`, `ObjectReference`, `ClassReference`, `FrameReference`, `Frame`, `FrameKind`,
`Object`, `ObjectData`, `ClosureData`, `ActivationHeader` or `Popped` in a
signature becomes its `<'gc>` form. `ActivationReference`, `Root`, `HostRoots`
and `Rotation` take none.

The root table holds the at-rest form, because a host holds its slots from
outside every entry: `Root { slot: Rc<Cell<Value<'static>>> }`,
`HostRoots { slots: Vec<Rc<Cell<Value<'static>>>>, … }`. `stash` and
`fetch` convert through one private function, the only conversion of a brand
in the crate:

```rust
/// The one conversion of a brand (`docs/design/memory.md`, "The brand"): a
/// host root's slot is shared with a host outside every entry, so it holds
/// `Value<'static>`, and `stash` and `fetch` pass a value through here on
/// the way in and out. Private to this module, and there is no public
/// counterpart: code outside `heap.rs` cannot change a handle's brand.
fn rebrand<'from, 'to>(value: Value<'from>) -> Value<'to> {
    match value {
        Value::Nil => Value::Nil,
        Value::Boolean(boolean) => Value::Boolean(boolean),
        Value::SmallInteger(integer) => Value::SmallInteger(integer),
        Value::Float(float) => Value::Float(float),
        Value::Object(object) => Value::Object(ObjectReference {
            index: object.index,
            #[cfg(debug_assertions)]
            origin: object.origin,
            #[cfg(debug_assertions)]
            epoch: object.epoch,
            brand: std::marker::PhantomData,
        }),
    }
}
```

`stash` stores `std::rc::Rc::new(std::cell::Cell::new(rebrand(value)))`;
`fetch` answers `rebrand(root.slot.get())` (after the origin check).
`move_everything`'s loop over the root table forwards a `Value<'static>`,
which the `Tracer` accepts (Step 4).

`Heap::new` loses `pub` — `World` (a child module) and `heap.rs`'s own
tests are its callers — and `impl Default for Heap` is deleted with the
comment above it. `move_everything` becomes private too (`fn`, no `pub`),
for the same two callers; `Forwarder` stays `pub(crate)`, since `World`
names it through the answer.

- [ ] **Step 3: Brand the object model in `types.rs`**

```rust
#[derive(Debug, Clone, Copy)]
pub enum Value<'gc> {
    Nil,
    Boolean(bool),
    SmallInteger(i64),
    Float(f64),
    Object(ObjectReference<'gc>),
}
```

and likewise `Object<'gc>` (`class: ClassReference<'gc>`, `data:
ObjectData<'gc>`), `ObjectData<'gc>` (`Instance(Vec<Value<'gc>>)`,
`Array(Vec<Value<'gc>>)`, `Closure(ClosureData<'gc>)`,
`Class(ClassData<'gc>)`), `ClassData<'gc>` (its four handle fields),
`ClosureData<'gc>` (`captured_frame`, `home`), `FrameKind<'gc>`
(`Closure(ObjectReference<'gc>)`, the three `ClassReference<'gc>` variants),
`Continuation<'gc>` (its four boxed variants), `WhileLoop<'gc>`,
`DoLoop<'gc>`, `NestedClassCreation<'gc>`, `Instantiation<'gc>`
(`levels: Vec<(ClassReference<'gc>, Vec<Value<'gc>>)>`),
`InstantiationPhase<'gc>`, `Frame<'gc>` and `NewspeakError<'gc>`. Every
doc comment stays. `BuiltinClass`, `Binding`, `ResolvedModule`,
`SourceOrigin` and `ResolveError` take none.

- [ ] **Step 4: Brand the tracing in `trace.rs`**

```rust
pub(crate) trait Tracer {
    fn object(&mut self, handle: &mut ObjectReference<'_>);
    fn frame(&mut self, handle: &mut FrameReference<'_>);
    fn value(&mut self, value: &mut Value<'_>) {
        match value {
            Value::Object(handle) => self.object(handle),
            Value::Nil | Value::Boolean(_) | Value::SmallInteger(_) | Value::Float(_) => {}
        }
    }
}
```

Every `impl Trace for X` becomes `impl Trace for X<'_>`, with its body
unchanged; the two helpers take `&mut [Value<'_>]` and `&mut
Option<ObjectReference<'_>>`. `Forwarder`'s `impl Tracer` in `heap.rs` takes the same
signatures and is otherwise unchanged; so is `impl Trace for StackEntry<'_>`.

- [ ] **Step 5: The one cast, in `World::enter`**

In `crates/ns-runtime/src/lib.rs`, after `#![warn(clippy::all)]`:

```rust
// The brand's one cast (`heap/world.rs`, `World::enter`) is the crate's only
// unsafe code; a second needs an argument as careful as that one's.
#![deny(unsafe_code)]
```

In `heap/world.rs`: `mutator: Mutator<'static>`; `World::new` is unchanged
(it builds a `Mutator<'static>`, and its bootstrap entry lends it like any
other); `stash(&self, value: Value<'static>) -> Root`; and `enter` becomes:

```rust
    /// Lends the step state out for `body` as a `Mutator<'gc>`, where `'gc` is
    /// a lifetime only `body` can name -- gc-arena's `Arena::mutate`. The run
    /// loop makes one entry per instruction and a host one per read, so this
    /// must inline away.
    #[allow(unsafe_code)] // the crate's one cast; see the SAFETY comment
    #[inline(always)]
    pub(crate) fn enter<R>(&mut self, body: impl for<'gc> FnOnce(&mut Mutator<'gc>) -> R) -> R {
        let at_rest: *mut Mutator<'static> = &mut self.mutator;
        // SAFETY: `Mutator<'static>` and `Mutator<'gc>` differ only in a
        // lifetime parameter, so they have one layout, and the pointer comes
        // from a unique borrow of `self.mutator` that lasts as long as the
        // reference made from it. `'gc` appears in `Mutator`'s fields only
        // through the `PhantomData` brand of `ObjectReference` and `FrameReference`, which
        // are indices -- no field of any branded type holds a reference with
        // lifetime `'gc` (`CLAUDE.md` makes that an invariant) -- so no
        // reference can outlive its referent through the cast. What `body`
        // stores is stored as `Mutator<'static>` again once `body` returns,
        // where nothing can read it but another entry, which brands it
        // anew; and `body`'s answer cannot name `'gc`, because `R` is chosen
        // outside the `for<'gc>`.
        let mutator = unsafe { &mut *at_rest.cast::<Mutator<'_>>() };
        body(mutator)
    }
```

(If the turbofish refuses the elided `'_`, write `at_rest as *mut
Mutator<'_>` instead; the cast must change the lifetime and nothing else.)

`move_everything` and `check_quiescent_state` work on the
`Mutator<'static>` directly and are otherwise unchanged. The debug
accessors are unchanged.

- [ ] **Step 6: Brand the interpreter, the VM, the natives and the codec**

`interpreter/mod.rs`: `pub struct Mutator<'gc>` with `heap: Heap<'gc>`,
`builtin_classes: Vec<ClassReference<'gc>>`, `platform_singleton:
Option<Value<'gc>>`, `spare_walks: Vec<Box<Instantiation<'gc>>>`,
`argument_buffer: Vec<Value<'gc>>` and the rest unchanged; `pub(crate) enum
Signal<'gc> { Return(ActivationReference, Value<'gc>), Error(NewspeakError<'gc>)
}`; `pub(crate) type EvaluationResult<'gc> = Result<Value<'gc>, Signal<'gc>>;`;
`Step<'gc>`, `StepArguments<'gc>` (`slice<'a>(&'a self, send: &'a [Value<'gc>])
-> &'a [Value<'gc>]`); every `impl Mutator` block becomes `impl<'gc>
Mutator<'gc>`; `Mutator::heap(&self) -> &Heap<'gc>`, `heap_mut(&mut self)
-> &mut Heap<'gc>`, `fetch(&self, root: &Root) -> Value<'gc>`,
`stash(&self, value: Value<'gc>) -> Root`; `Interpreter::enter<R>(&mut
self, body: impl for<'gc> FnOnce(&mut Mutator<'gc>) -> R) -> R`;
`Interpreter::stash(&self, value: Value<'static>) -> Root`, whose doc
comment adds: "A host can build an immediate but never hold an object
handle outside an entry, so a `Value<'static>` holds none."

Every free function that takes a branded type or a `Mutator` gets a `<'gc>`
parameter and names it in each branded type — `fn native<'gc>(mutator: &mut
Mutator<'gc>, receiver: Value<'gc>, arguments: &[Value<'gc>]) -> Option<Result<Step<'gc>,
Signal<'gc>>>` — and `&Heap` becomes `&Heap<'gc>`. Never write `Value<'_>`
beside a `Mutator<'gc>`: an elided lifetime is a fresh one, and invariance
refuses to unify it with `'gc`. The compiler lists every site; the files are
`vm.rs` (`Flow` stays unbranded — it carries an `ActivationReference`, an
`Rc<CompiledCode>` and a `HostResult`), `dispatch.rs` (`Lookup<'gc>`),
`platform.rs`, `builtins/*.rs`, `interpreter/{instantiate, nested, sends,
locals, literals, safepoint}.rs` (`locals::OuterFrame<'gc>`), and
`image/{mod, encode, decode, mixins, inspect}.rs` (`DecodeContext<'a, 'gc>`,
`DecodedGraph<'gc>`, `Tables<'a, 'gc>` and whatever else holds a handle).

Two things the compiler reports that are not a missing `<'gc>`:

- A `const` of a branded type — `nested.rs`'s `const CREATION_IN_PROGRESS:
  Value = Value::Bool(false);` — is a `Value<'static>`, which invariance
  will not let stand for a `Value<'gc>`. Make it a function with the same
  doc comment: `fn creation_in_progress<'gc>() -> Value<'gc> {
  Value::Bool(false) }`. Search for the others: `rg -n "const [A-Z_]+:
  (Value|ObjectReference|FrameReference|ClassReference)" crates/ns-runtime/src`.
- A value that leaves an entry. In the runtime there should be none after
  Task 1 (a run's answer is rooted in its last entry); if one appears, it is
  a mistake the brand exists to catch — root it inside the entry, never widen
  a lifetime.

`dispatch.rs`'s and `types.rs`'s unit tests built heaps with `Heap::new()`,
which is private now: each builds `crate::Interpreter::bootstrap()` and runs
its body in `interpreter.enter(|mutator| { … })`, passing `&mut mutator.heap` where its
helper took `&mut Heap` (the helpers take `&mut Heap<'gc>`).
`crate::Interpreter::bootstrap()` already holds the builtin classes a
fresh heap lacked; a test that counted on an empty heap asserts on the
objects it made itself rather than on an index.

- [ ] **Step 7: Let the tests follow**

Run: `cargo build --workspace --all-targets`
Expected: after Steps 2–6, the only errors left are in tests, each a test
that lets a handle out of an entry — Tasks 2 and 3 should have left none but
`roots.rs`'s `a_value_kept_past_a_move_is_refused`. Delete that test: the
first `compile_fail` doctest of Step 1 is its replacement, and its doc
comment's point ("a value a host fetched and kept past a move names nothing
any more") is now stated by the doctest's. Convert any other by the rules;
never by widening a lifetime, and never by adding a way to erase a brand.

- [ ] **Step 8: Run the doctests**

Run: `cargo test -p ns-runtime --doc Interpreter::enter`
Expected: PASS — all six blocks: the three plain ones run, the three
`compile_fail` ones fail to compile. Then read each `compile_fail` block's
actual error, to be sure it fails for the reason its prose names: compile
it as a scratch binary (`cargo new` under the scratchpad, depending on
`ns-runtime` by path) and check that the error is a lifetime or borrow
error on the line the prose names — "borrowed data escapes outside of
closure", "lifetime may not live long enough", or, for the re-entry,
"closure requires unique access to `interpreter`" / "cannot borrow `interpreter` as
mutable more than once" — not a type or name error. If a block fails for
any other reason, fix the block.

- [ ] **Step 9: Check the sizes and the cast**

Run: `cargo test --release -p ns-runtime --lib a_decision_stays_the_size_the_hot_path_was_measured_at`
Expected: PASS (56 bytes; a lifetime is zero-sized).

Run: `rg -n "unsafe" crates`
Expected: exactly the `unsafe { … }` in `World::enter`, its
`#[allow(unsafe_code)]`, and the `deny` in `lib.rs` — plus prose in
comments.

- [ ] **Step 10: Run every gate**

Run the gates from "Global Constraints", the two stress runs included.
Expected: all green, the reports exactly as stated. The stress mode checks
what the tracer knows and the brand what Rust holds; both must hold.

- [ ] **Step 11: Commit**

```bash
git add -A crates
git commit -m "feat: brand every movable handle with the lifetime of its entry" -m "ObjectReference, ClassReference, FrameReference, Value and every type that holds one carry
an invariant 'gc, obtainable only inside Interpreter::enter, which lends
the step state out as a Mutator<'gc> by one cast -- the crate's only
unsafe code, under #![deny(unsafe_code)]. Holding a handle across a run,
storing one in a host's field and a step re-entering the run loop are
compile errors, pinned by compile_fail doctests.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 5: Measure the whole step, and write it down

**Files:**
- Modify: `docs/design/memory.md`, `docs/design/execution.md`,
  `docs/design/README.md`, `docs/PROGRESS.md`, `CLAUDE.md`

- [ ] **Step 1: Measure the whole step**

Measure with the protocol, the parent being `4999d05` (the commit the step
started from). Do it in two sittings, as 2b did, so that a borderline group
can be told from a sitting's noise. Expected: every source level with
`4999d05` (a lifetime costs nothing at run time, and Task 1 measured the
per-instruction `enter` level); report any group entirely above.

- [ ] **Step 2: `docs/design/memory.md`**

- The status paragraph of "Checked roots and a collector": 2c is built;
  only the collector is not. "What the whole would change" loses the brand
  from its unbuilt half.
- "Planned: the brand, checked by the compiler rather than by convention"
  becomes "The brand, checked by the compiler rather than by convention",
  in the present tense, describing what exists: `Mutator<'gc>`, `World`
  holding `Mutator<'static>` at rest in `heap/world.rs`, `World::enter`
  handing that `Mutator<'static>` to a `for<'gc>` closure with no cast and
  no `unsafe` — the closure is type-checked for every `'gc`, so it can
  neither assume `'gc = 'static` nor let a branded value out — and the
  crate's `#![forbid(unsafe_code)]`, the private `rebrand` for host roots
  and the absence of a public `unbind`, `Heap::new` and
  `Heap::move_everything` private to the heap module, one entry per
  instruction, the three `compile_fail` doctests (name where they are),
  `ActivationReference` unbranded, and that the stress mode keeps running
  (the brand covers what Rust holds, the stress mode what the tracer
  knows). Replace "heap.rs stays free of `unsafe`" and the paragraph about
  a fallback cast with what was decided and why: the safe per-payload
  conversion was rejected for its cost (decision 1), the cast that was
  chosen instead turned out unnecessary, and the reasons gc-arena's
  `mutate` and Nova's `Bindable` for `Box<T>` need theirs (a `Rootable<'gc>`
  type projection; rebranding a `Box<T>` payload) do not apply to a plain
  lifetime parameter on `Mutator`.
- Add under the "Considered and rejected" sections two entries.
  "Considered and rejected: a safe conversion per payload", with decision
  1's reason — the boxed continuations reallocated at every push and pop,
  and the spare walks' reuse lost. "Considered and rejected: an audited
  lifetime cast" — chosen with the user on 2026-09-13 in the per-payload
  conversion's place, then found unnecessary in Task 4's review on
  2026-09-14, because the higher-ranked closure alone gives the guarantee:
  `body(&mut self.mutator)` compiles under `#![forbid(unsafe_code)]`, and a
  closure that answers a handle still fails with "lifetime may not live
  long enough", one that stores a handle in a host's field with E0521.
- "Host roots": replace the paragraph "What 2c still changes here" with the
  state after it: a host reads through `enter`, `Interpreter::stash` takes
  a `Value<'static>` (immediates), and a root is the only way to hold a
  value across a run — a compile-time guarantee now, no longer one only
  the stress mode checks.
- "What the collector traces": the bootstrap is forwarded by
  `World::move_everything`, not `Interpreter::move_everything`.
- The sentence in "Planned: the brand" about `same` and two stale handles
  becomes a statement of what holds (decision 12).

- [ ] **Step 3: `docs/design/execution.md`**

- "The VM": each instruction is one entry, the safepoint and `vm::run`
  outside every one, and a run's answer rooted in its last entry.
- A new section "What 2c moved" after "What 2b moved", in its format: the
  seven sources, both sittings' medians and means, the change against
  `4999d05` and against `d804eaf`, and a sentence on each group that is not
  level. Include Task 1's own measurement of the per-instruction `enter`,
  as a sitting of its own.

- [ ] **Step 4: `docs/design/README.md`**

- The map's row for memory.md: status "Partial — the arena, the activation
  stack, the safepoint, tracing, host roots, the stress mode and the lifetime
  brand are built; nothing reclaims an arena slot"; planned next "a
  collector".
- "Architecture", the paragraph on `heap.rs`: add that every movable handle
  carries the brand of the entry that minted it, obtainable only inside
  `Interpreter::enter`, which `heap/world.rs`'s `World::enter` gives in
  safe Rust — the crate forbids `unsafe` code.

- [ ] **Step 5: `CLAUDE.md`**

- The crate description of `ns-runtime`: add `Mutator` (the step state,
  reached only through `Interpreter::enter`) and `heap/world.rs`.
- "A handle means nothing against a `Heap` that did not mint it":
  `Heap::new()` is private to the heap module now, so "never build a
  `Heap::new()`" becomes a statement that nothing outside it can; the
  `disasm` sentence already says it reads through `interpreter.enter` (the
  naming interlude brought it up to date).
- "A handle also means nothing across a move": a host keeps a value across
  a run only as a `Root`, and the compiler enforces it — a handle is branded
  with the entry that minted it (`Interpreter::enter`); the stress mode still
  catches a handle the tracer does not know about.
- A new invariant, **"The brand is the compiler's, and needs no cast."**
  `heap/world.rs`'s `World::enter` hands the at-rest `Mutator<'static>` to
  a closure generic over `'gc`, in safe Rust; the crate forbids unsafe code
  (`#![forbid(unsafe_code)]`) and the brand needs none. What keeps it
  meaning something: never add a public way to change a handle's brand
  (`heap.rs`'s private `rebrand` is the only conversion), and never give
  `Mutator` a way to reach the `Interpreter`, which is what keeps a step
  from running the loop.

- [ ] **Step 6: `docs/PROGRESS.md`**

- The M4 row: add `[lifetime-brand-2c](superpowers/plans/2026-09-13-2348-ns-next-lifetime-brand-2c.md)`
  to its plans.
- "Where the next session starts": step 2 is done; 2c's paragraph becomes a
  one-line "done" with a pointer to a new "M4 — The lifetime brand" section
  below; the next piece is step 3, the inline caches, with the cached getter
  symbol and `u32` hasher for lexical sends as the candidate small step
  before them (the paragraph "The profile of `bench_block.ns` is taken"
  says why). Name what step 3's plan must be written against.
- "Open findings": remove "Two stale handles from the same old epoch compare
  in `same()` without a panic" (decision 12 closes it); update the
  `d804eaf` comparison with Task 5's figures; keep the rest.
- Item 2 of the numbered order: "Done", and its 2c bullet "Done", with one
  sentence on what was built and what the measurement showed.
- A new section "M4 — The lifetime brand" in the format of the 2b section:
  what was built (decisions 1–12 in brief), the measurement, the suite's
  time with and without the stress mode (measure both: `cargo test
  --workspace` and the stress run, test binaries already built), and the
  plan link.

- [ ] **Step 7: Check the documents against the code**

Run: `rg -n "heap_reference|heap_mut\(\)|Interpreter::fetch|Planned: the brand|heap.rs stays free of|2c, planned|lifetime brand \(2c\)" docs/design docs/PROGRESS.md CLAUDE.md`
Expected: no hit outside a sentence that describes the past on purpose
(plans under `docs/superpowers/plans/` are execution records and are left as
written).

Then read "Where the next session starts" as a fresh session would: it must
say what is done, what comes next, which documents to read for it, and the
measurement rule.

- [ ] **Step 8: Run every gate**

Run the gates from "Global Constraints", the two stress runs included.
Expected: all green, the reports exactly as stated.

- [ ] **Step 9: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: the lifetime brand is built" -m "memory.md describes the brand as it exists -- Mutator, World, and an
entry that needs no cast and no unsafe code -- and records the safe
per-payload conversion and the audited cast as considered and rejected.
execution.md measures the step, README.md and CLAUDE.md state the new
invariant, and PROGRESS.md moves on to step 3.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```
