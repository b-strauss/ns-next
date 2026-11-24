# Safepoint, Tracing, Host Roots and the Stress Mode (2b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every handle the runtime keeps is somewhere a tracer can find and
rewrite it, and a debug-build stress mode proves it: it moves every object and
every reified frame at safepoints in the run loop, and a handle the tracer
missed panics at its first read. Hosts keep values across runs only as `Root`s.

**Architecture:** The run loop polls one flag the heap owns at every switch
to another activation and at every backward jump; its slow path lives in
`interpreter/safepoint.rs`. A new `trace.rs` enumerates, type by type, every
field that holds an `ObjRef`, a `ClassRef` or a `FrameRef`; `heap.rs` owns
the tracer that rewrites them, because only `heap.rs` can mint a handle. A
move rotates both arenas by a shift with no fixed point and bumps a move
epoch that every handle carries in a debug build, so the permutation checks
the forwarding and the epoch checks the completeness. `Rc<ClosureData>`
goes, because a handle behind a shared `Rc` cannot be rewritten in place:
the closure object holds its two frames itself, and everything that needs a
closure names the closure object. Hosts hold `Root`s, `Rc`-shared slots the
heap's root table also holds and the tracer rewrites.

**Tech Stack:** Rust 1.98.0, edition 2021, no new dependency. Only
`ns-runtime` and `ns-cli` change; nothing under `crates/ns-syntax`.

**Spec:** [docs/design/memory.md](../../design/memory.md), "Planned: checked
roots and a collector" — above all "Where a collection can happen", "Host
roots", "What the collector traces" and "Checking the roots before a
collector exists: the stress mode" — and [docs/PROGRESS.md](../../PROGRESS.md),
"Where the next session starts", for the measurement rule against
`d804eaf`. Read both before Task 1. Also read `CLAUDE.md`'s invariants: every
one of them applies.

## Decisions this plan carries

Settled with the user on 2026-09-13, or following from the design and the
repository's invariants:

1. **The stress mode moves on a budget, not literally at every safepoint.**
   A move costs work proportional to the heap and the activation stack, so
   "every safepoint" is quadratic on the goldens that recurse to 100,000
   activations and on the `bench_*` fixtures — hours per suite run in a debug
   build. The schedule (decided with the user): while the traced size — both
   arenas, the activation stack's entries and registers, and the host root
   table — is at most `STRESS_MOVE_BUDGET = 1_000`, every safepoint moves;
   above it, a safepoint moves once the safepoints since the last move reach
   ⌈size / budget⌉. That spends at most about 1,000 handle rewrites per
   safepoint on average, the size-proportional pacing a real collector uses;
   every small test keeps full coverage. Prior art uses a fixed frequency
   instead (HotSpot's `ScavengeALotInterval`, SpiderMonkey's
   `JS_GC_ZEAL=mode,N`, V8's `--gc-interval`); a fixed `N = 1` is the
   quadratic case, and a larger `N` would take coverage from small tests too.
   memory.md records the deviation from its "every safepoint" (Task 8).
2. **`ClosureData` is stored inline in the closure object**, with no `Rc`
   around it. Its `params` and `code` stay `Rc`s shared with the
   `MakeClosure` instruction — the design's "shared part" — and its
   `captured_frame` and `home` sit in the object's own payload.
   `FrameKind::Closure`, `WhileLoop.condition`, `DoLoop.block` and
   `Step::Block.closure` name the closure object (`ObjRef`).
   `Continuation::WhileCondition`/`WhileBody` carry a `Box<WhileLoop>`
   instead of an `Rc<WhileLoop>`: the state is owned by exactly one stack
   entry, and the tracer needs `&mut` access to it.
3. **Wire tag `FRAME_CLOSURE` (1) is retired and `FRAME_CLOSURE_OBJECT` (6)
   takes its place.** A closure frame record used to carry a copy of its
   closure's record inline; it now names the closure object by object id. A
   stale artifact's tag 1 must hit an unknown tag, not be read with a layout
   it does not have (`CLAUDE.md`, "A wire tag is assigned once").
4. **The host API takes and answers `Root`s.** `Interpreter::send`,
   `instantiate`, `load_module`, `platform` and `image::load` do; their result
   is a `HostResult = Result<Root, HostSignal>`, and a Newspeak error reaches
   the host as a `HostError` whose receiver is a `Root`. `Signal` and
   `EvalResult` become crate-private once nothing outside the crate uses them
   (Task 7). `heap_ref()` and `heap_mut()` stay until 2c. `image::snapshot`
   and `image::build` keep taking plain values, which a host fetches from its
   roots immediately before the call: encoding runs no code, so nothing can
   move under it.
5. **The root table lives in `Heap` behind a `RefCell`**, so that
   `stash(&self, Value) -> Root` and `fetch(&self, &Root) -> Value` take a
   shared borrow. That is gc-arena's `DynamicRootSet::stash`, and it lets a
   test write `interp.send(&r, sel, &[interp.stash(v)])`. The table is swept
   of roots nothing else holds at every move, and on `stash` whenever it has
   doubled since the last sweep, so it stays proportional to the live roots
   in a release build too.
6. **A move rotates each arena** by a shift in `1..len` that varies with the
   epoch, so no slot keeps its index, and bumps the heap's move epoch. Every
   `ObjRef` and `FrameRef` carries, in a debug build, the epoch it was minted
   in beside its origin, and every accessor compares it. `ActivationRef`
   carries none: no stack entry moves.
7. **A release build contains only the poll.** `trace.rs`, the epoch, the
   move, the stress schedule and the test hooks (`move_now`, `moves`,
   `set_stress_moves`) are `#[cfg(debug_assertions)]`. The collector (step 5)
   un-gates what it needs.
8. **The argument buffer is cleared when it is given back**, not when it is
   taken, so it is empty at every safepoint, as memory.md says it is, and a
   debug assertion at the safepoint checks it.
9. **`instantiate` refuses a root that holds no class object** with a
   Newspeak error. It took a `ClassRef` before, so no host could pass
   anything else; a `Root` can hold any value.

## Scope

In: the closure split and its wire tag, the root table and the rooted host
API, the tracing and the move, the safepoint poll and the stress mode, the
conversion of the CLI and every test to roots, the measurements and the
documents.

Deliberately **not** in this plan:

- The lifetime brand, `Interpreter::enter`, `Mutator`, removing `heap_ref()`
  and `heap_mut()`, and the `compile_fail` doctests (2c).
- Liveness, reclamation, the allocation counter that requests a collection,
  and the spaces (step 5). Nothing reclaims an arena slot after this plan
  either; a move keeps everything.
- The cached getter symbol and the `u32` hasher for lexical sends
  (PROGRESS.md, "The profile of `bench_block.ns` is taken").
- The two other open findings from 2a (a decoded initializer not held to
  answering `self`; no test for a `^` into a clause's own activation).

## Global Constraints

- **`vm.rs` holds no dispatch logic.** It holds the poll — one compare and a
  call — and nothing else of this plan; the slow path is
  `Interpreter::safepoint` in `interpreter/safepoint.rs`.
- **Program outcomes are pinned, not re-derived.** No golden, no `.expected`
  file and no `run-corpus` row changes in this plan. A test whose output
  changes is a defect in the change.
- **An image is untrusted input.** The new frame-kind record is validated
  before anything indexes by it (`BadObjectId`, and `NotAClosureReference`
  once every record is read). No `expect`/`panic!` this plan adds is reachable
  from artifact bytes; each says in its message why it is an internal
  invariant.
- **A wire tag is assigned once.** `FRAME_CLOSURE` keeps its number as a
  retired constant; `FRAME_CLOSURE_OBJECT` is 6; `tests/opcode_tags.rs` pins
  both.
- **A handle means nothing against a `Heap` that did not mint it — or across
  a move.** The epoch check is the second half of that sentence.
- **Only `heap.rs` rewrites a handle.** `trace.rs` enumerates fields and
  hands each `&mut ObjRef` / `&mut FrameRef` to a `Tracer`; the one `Tracer`
  that rewrites (`Forwarder`) is in `heap.rs`.
- **Every `Trace` impl destructures its type with every field named and no
  `..`, and matches every variant with no `_` arm**, so that adding a field
  or a variant fails to compile until someone decides whether it holds a
  handle. That is what keeps the enumeration complete as the types grow.
- **Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an
  `EvalResult` or a `HostResult`.** `HostSignal`, `HostError` and `Root` have
  no `Debug` impl, for the reason `Signal` has none. Tests unwrap through a
  local `expect_ok`.
- **Object, frame and activation access goes through `heap.rs`.**
- **A Newspeak module written inline in a test is an `indoc! {"…"}`**, lines
  indented with spaces, the closing `"}` on its own line.
- **Comments explain the decision, not the mechanics.**
- **Release sizes do not grow:** `Object` 56 bytes, a frame-arena entry
  (`FrameSlot`) 136, `Result<Step, Signal>` 56. Debug sizes grow by the epoch
  field; Task 3 measures them and updates `CLAUDE.md` and memory.md.
- **Performance is measured, not assumed**, with the protocol below, in
  Tasks 1 and 4 and for the whole step in Task 8. A task that makes a
  benchmark slower than its parent beyond noise is not done: report the
  finding to the controller before anything else happens.
- **Gates before every commit:** `cargo fmt --all`; `cargo clippy --workspace
  --all-targets` warning-free; `cargo test --workspace`;
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`;
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`. From Task 4 on, each task also names the
  stress runs it must pass; from Task 7 on those are the whole suite and
  `run-corpus`.
- Work on the checked-out branch `feature/root-discipline-2b`, created from
  `master` at `4df2561`; no worktree except the throwaway ones the
  measurement protocol builds.
- Conventional commits, each ending with (subagents included):
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj
  ```

## Running a command under the stress mode

The variable is `NS_NEXT_STRESS_MOVES`; any value other than empty or `0`
turns the mode on for every interpreter the process bootstraps, the
`ns-next` binary the CLI tests spawn included. In Git Bash:

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
this. Per PROGRESS.md, every measurement from 2b up to step 3 compares against
`d804eaf` as well as against the commit the task started from.

Besides the three fixtures, four throwaway sources exercise the paths 2a
changed and no fixture does. Write them under `target\bench-2b\` (ignored by
git) the first time they are needed; they are never committed. Tabs indent
them, like the fixtures.

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
  foreach ($src in $sources) {
    foreach ($name in $bins.Keys) {
      "$round $name $src"
      & $bins[$name] bench $src --iterations 30   # prints median, min, max
    }
  }
}
git worktree remove ..\ns-next-base
git worktree remove ..\ns-next-parent
# the decision-size pin runs only in a release build (Task 3 says why)
cargo test --release -p ns-runtime --lib a_decision_stays_the_size_the_hot_path_was_measured_at
```

Record the five medians per source and binary and each group's mean. The
comparison with the parent is the gate: a change is "no slower" when, for
every source, the after-group does not lie entirely above the parent-group,
and "faster" when it lies entirely below. The comparison with `d804eaf` is
reported, not gated — but if a source's shift against `d804eaf` grows past
what 2a left (`bench_loop.ns` +1.9%, `bench_block.ns` +1.6%, every other
source level or faster) by more than either group's spread, report it to the
controller before anything else happens. Put both tables in the task's report;
Task 8 carries the whole-step table into the documents.

---

### Task 0: The branch and the plan

- [ ] **Step 1: Create the branch and commit this plan**

```bash
git checkout master
git pull --ff-only
git checkout -b feature/root-discipline-2b
git add docs/superpowers/plans/2026-09-13-1748-ns-next-checked-roots-2b.md
git commit -m "docs: plan step 2b, the safepoint, tracing, host roots and the stress mode" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 1: The closure object holds its own frames

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`ObjectData::Closure`, `ClosureData`'s
  doc, `FrameKind::Closure`, `Continuation::WhileCondition`/`WhileBody`,
  `WhileLoop`, `DoLoop`)
- Modify: `crates/ns-runtime/src/heap.rs` (closure accessors, `frame_captured`,
  `activation_home`, `activation_captured`, `reify`, the unit test
  `a_closure_activations_home_and_captured_frame_are_its_closures`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Step::Block`,
  `push_step`'s block arm, `check_block_arity`, `new_closure`, `resume`)
- Modify: `crates/ns-runtime/src/vm.rs` (`Instr::MakeClosure`)
- Modify: `crates/ns-runtime/src/builtins/block.rs`, `boolean.rs`, `collections.rs`
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`,
  `inspect.rs`
- Modify: `docs/design/object-model.md`, `docs/design/execution.md`,
  `docs/design/image-format.md` (the lines named in Step 9)
- Test: `crates/ns-runtime/tests/opcode_tags.rs`, `crates/ns-runtime/tests/image.rs`,
  and every test that builds a `ClosureData` or a `FrameKind::Closure` by hand
  (`tests/image.rs` around line 5267)

**Interfaces:**
- Produces, in `types.rs`: `ObjectData::Closure(ClosureData)`;
  `FrameKind::Closure(ObjRef)`; `Continuation::WhileCondition(Box<WhileLoop>)`,
  `Continuation::WhileBody(Box<WhileLoop>)`; `WhileLoop { condition: ObjRef,
  body: Value, expect: bool }` deriving `Debug, Clone`; `DoLoop { receiver:
  Value, block: ObjRef, items: Vec<Value>, next: usize }`.
- Produces, in `heap.rs`: `pub fn is_closure(&self, obj: ObjRef) -> bool`;
  `pub fn closure_param_count(&self, obj: ObjRef) -> Option<usize>`;
  `pub fn closure_code(&self, obj: ObjRef) -> Option<Rc<CompiledCode>>`;
  `pub(crate) fn closure_activation(&self, obj: ObjRef) -> Option<(Rc<CompiledCode>, usize, FrameRef)>`;
  `pub fn closure_data(&self, obj: ObjRef) -> Option<ClosureData>` (now a copy,
  not an `Rc`).
- Produces, in `interpreter/mod.rs`: `Step::Block { closure: ObjRef, args:
  StepArgs, then: Continuation }`; `pub fn new_closure(&mut self, closure:
  ClosureData) -> ObjRef`.
- Produces, in `image/opcodes.rs`: `pub const FRAME_CLOSURE_OBJECT: u8 = 6;`;
  in `image/decode.rs`: `DecodeError::NotAClosureReference`.

- [ ] **Step 1: Write the failing wire-format tests**

In `crates/ns-runtime/tests/opcode_tags.rs`, in the test that pins the graph
tags, after `assert_eq!(FRAME_SUPERCLASS_MESSAGE, 5);`:

```rust
    assert_eq!(FRAME_CLOSURE_OBJECT, 6);
```

and add `FRAME_CLOSURE_OBJECT,` after `FRAME_SUPERCLASS_MESSAGE,` in the frame
family of `no_two_graph_tags_in_a_family_share_a_number`. `FRAME_CLOSURE`
stays pinned at 1: a retired number stays assigned.

In `crates/ns-runtime/tests/image.rs`, right after
`a_closure_capturing_a_block_activation_round_trips`, add:

```rust
/// A closure frame names the closure object it runs, rather than carrying a
/// copy of its record: after a round trip the inner closure's captured frame
/// -- the outer block's activation -- names a decoded closure object, and it
/// is the outer block, with its one parameter.
#[test]
fn a_closure_frame_names_the_closure_object_it_runs() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
        ) (
          public mk = ( ^[:x | [ x ] ] value: 5 )
        ) : ()
    "};
    let (interp, _class, _o, decoded, _max) = round_trip_graph(src, |interp, class| {
        let instance = expect_ok(interp.instantiate(*class, vec![]), "instantiate");
        vec![expect_ok(
            interp.send(instance, Symbol::intern("mk"), vec![]),
            "mk",
        )]
    });
    let heap = interp.heap_ref();
    let inner = heap
        .closure_data(*obj(&decoded[0]))
        .expect("expected the inner closure");
    let FrameKind::Closure(outer) = heap.frame_kind(inner.captured_frame) else {
        panic!("the captured frame is a block activation")
    };
    let outer = heap
        .closure_data(outer)
        .expect("a closure frame names a closure object");
    assert_eq!(outer.params.len(), 1, "the outer block takes `x`");
    assert!(
        outer.home.same(inner.home),
        "both closures share `mk`'s activation as their home"
    );
}

/// The frame kind a stale artifact wrote for a closure activation, before
/// the record named the closure object. Its number is retired: the decoder
/// refuses it rather than read the old inline layout as the new one.
#[test]
fn the_retired_closure_frame_tag_is_refused() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 0, 1);
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(false); // no sender
    w.write_bool(false); // no home
    w.write_bool(false); // no current class
    w.write_u8(1); // frame kind: the retired FRAME_CLOSURE
    // Padded past the shortest frame record the wire admits, so
    // `bounded_count` admits the announced frame and the kind tag itself is
    // what the decoder rejects.
    w.write_bool(false);
    w.write_u32(0);
    assert_eq!(
        GraphFixture::new().decode_err(&w.into_bytes()),
        decode::DecodeError::InvalidTag {
            what: "frame kind",
            tag: 1
        }
    );
}

/// A closure frame's object id is in range but names a string: refused once
/// every record is read, like a class reference that names an ordinary
/// object, since only then is it known what the record was.
#[test]
fn a_closure_frame_naming_an_object_that_is_not_a_closure_is_refused() {
    let mut w = encode::Writer::new();
    graph_counts(&mut w, 1, 1);
    w.write_u8(0); // class tag: builtin
    w.write_u8(encode::builtin_class_tag(BuiltinClass::Str));
    w.write_u32(1); // identity hash
    w.write_u8(1); // object body: Str
    w.write_str("not a closure");
    w.write_u8(0); // receiver: nil
    w.write_u32(0); // no args
    w.write_u32(0); // no temps
    w.write_u32(0); // pc
    w.write_bool(false); // no sender
    w.write_bool(true); // a home...
    w.write_u32(0); // ...frame 0 itself
    w.write_bool(false); // no current class
    w.write_u8(6); // frame kind: FRAME_CLOSURE_OBJECT...
    w.write_u32(0); // ...naming object 0, the string
    w.write_bool(false); // continuable
    w.write_u32(0); // no roots
    let err = GraphFixture::new().decode_err(&w.into_bytes());
    assert_eq!(err, decode::DecodeError::NotAClosureReference);
    assert_eq!(
        err.to_string(),
        "a closure frame in the image names an object that is not a closure"
    );
}
```

(`round_trip_graph`, `obj`, `GraphFixture` with its `decode_err`,
`graph_counts` and `expect_ok` already exist in `tests/image.rs`; check their
signatures there and follow them exactly. `DecodedGraph` has no `Debug`, so
`decode_err`, not `expect_err`, is how these tests reach the error.)

- [ ] **Step 2: Run the tests and see them fail to compile**

Run: `cargo test -p ns-runtime --test opcode_tags --test image`
Expected: compile errors — `FRAME_CLOSURE_OBJECT` and
`DecodeError::NotAClosureReference` do not exist, and `closure_data` returns
an `Rc` whose field access still compiles but `heap.frame_kind(...)`'s
`FrameKind::Closure(outer)` binds an `Rc<ClosureData>`, not an `ObjRef`.

- [ ] **Step 3: The types**

In `crates/ns-runtime/src/types.rs`:

```rust
#[derive(Debug, Clone)]
pub enum ObjectData {
    Instance(Vec<Value>),
    Str(String),
    Sym(Symbol),
    Array(Vec<Value>),
    Closure(ClosureData),
    Class(ClassData),
}
```

Replace `ClosureData`'s doc and fields' docs with:

```rust
/// A block closure. Its parameter names and compiled body are `Rc`s it
/// shares with the `MakeClosure` instruction that made it -- the same for
/// every closure one instruction makes -- and the two frames it captured
/// are its own. Stored inline in the closure object, never behind an `Rc`
/// of its own: a handle lives only where the tracer can rewrite it in place
/// (`docs/design/memory.md`, "What the collector traces"), so everything
/// else that needs a closure -- an activation's `FrameKind`, a native
/// loop's state, a decided `Step` -- names the closure object instead.
#[derive(Debug, Clone)]
pub struct ClosureData {
    pub params: Rc<Vec<Symbol>>,
    pub code: Rc<crate::bytecode::CompiledCode>,
    /// The activation the closure was created in, reified.
    pub captured_frame: FrameRef,
    /// The lexically enclosing *method* (or initializer) activation, which a
    /// `^` in the block returns from.
    pub home: FrameRef,
}
```

In `FrameKind`:

```rust
    /// An activation of the closure object named. Its body and the frames
    /// it captured are read through the object (`Heap::closure_activation`,
    /// `Heap::activation_captured`).
    Closure(ObjRef),
```

In `Continuation`:

```rust
    /// A native `whileTrue:`/`whileFalse:` loop's condition returned.
    WhileCondition(Box<WhileLoop>),
    /// Its body returned.
    WhileBody(Box<WhileLoop>),
```

and:

```rust
/// A native `whileTrue:`/`whileFalse:` loop in progress: its condition, its
/// body -- any value, since a body that is not a block evaluates to itself --
/// and the condition value that keeps it going.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct WhileLoop {
    /// The condition's closure object.
    pub(crate) condition: ObjRef,
    pub(crate) body: Value,
    pub(crate) expect: bool,
}
```

In `DoLoop`, `pub(crate) block: ObjRef,` with the doc line "its block's
closure object" worked into the struct's doc comment.

- [ ] **Step 4: The heap accessors**

In `crates/ns-runtime/src/heap.rs`, replace `closure_data` with these (in the
object-accessor section, where `closure_data` was):

```rust
    /// Whether `obj` is a closure -- what a control-flow native asks of a
    /// value before activating it as a block.
    pub fn is_closure(&self, obj: ObjRef) -> bool {
        self.check_object_origin(obj);
        matches!(self.objects[obj.index as usize].data, ObjectData::Closure(_))
    }

    /// How many parameters closure `obj` declares, or `None` when `obj` is
    /// not a closure.
    pub fn closure_param_count(&self, obj: ObjRef) -> Option<usize> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Closure(closure) => Some(closure.params.len()),
            _ => None,
        }
    }

    /// Closure `obj`'s compiled body, or `None` when `obj` is not a closure.
    /// `image::decode::frame_body` resolves a closure frame's body by it.
    pub fn closure_code(&self, obj: ObjRef) -> Option<std::rc::Rc<crate::bytecode::CompiledCode>> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Closure(closure) => Some(closure.code.clone()),
            _ => None,
        }
    }

    /// What activating closure `obj` needs, read in one borrow: its body,
    /// how many parameters it declares, and the frame it captured, whose
    /// receiver and current class the activation takes. `None` when `obj`
    /// is not a closure.
    pub(crate) fn closure_activation(
        &self,
        obj: ObjRef,
    ) -> Option<(std::rc::Rc<crate::bytecode::CompiledCode>, usize, FrameRef)> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Closure(closure) => Some((
                closure.code.clone(),
                closure.params.len(),
                closure.captured_frame,
            )),
            _ => None,
        }
    }

    /// A copy of closure `obj`'s payload -- two `Rc` bumps, since its
    /// parameter names and body are shared with the instruction that made
    /// it. For the image encoder and verifier and for tests; the run loop
    /// reads what it needs through the narrower accessors above.
    pub fn closure_data(&self, obj: ObjRef) -> Option<ClosureData> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Closure(closure) => Some(closure.clone()),
            _ => None,
        }
    }

    /// Closure `obj`'s captured frame and home, or `None` when `obj` is not
    /// a closure: what a closure activation's kind reaches through. A
    /// frame kind decoded from an image names an object the decoder has
    /// confirmed is a closure (`NotAClosureReference`), but answering `None`
    /// rather than panicking keeps a future change there from turning an
    /// untrusted image into a panic here.
    fn closure_frames(&self, obj: ObjRef) -> Option<(FrameRef, FrameRef)> {
        self.check_object_origin(obj);
        match &self.objects[obj.index as usize].data {
            ObjectData::Closure(closure) => Some((closure.captured_frame, closure.home)),
            _ => None,
        }
    }
```

Then read through the object wherever a closure's frames were read through
the `Rc`:

```rust
    pub fn frame_captured(&self, frame: FrameRef) -> Option<FrameRef> {
        self.check_frame_origin(frame);
        match &self.frames[frame.index as usize].frame.kind {
            FrameKind::Closure(closure) => self.closure_frames(*closure).map(|(captured, _)| captured),
            _ => None,
        }
    }
```

`activation_home` answers `self.closure_frames(*closure).map(|(_, home)| home)`
for `FrameKind::Closure(closure)`, `activation_captured` the captured frame,
and `reify` builds `home` the same way:

```rust
            home: match &entry.kind {
                FrameKind::Closure(closure) => self.closure_frames(*closure).map(|(_, home)| home),
                _ => None,
            },
```

Rewrite the unit test `a_closure_activations_home_and_captured_frame_are_its_closures`
so that it allocates a closure object and pushes `FrameKind::Closure(closure)`:

```rust
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let closure = heap.alloc_object(
            class,
            ObjectData::Closure(ClosureData {
                params: std::rc::Rc::new(vec![]),
                code: std::rc::Rc::new(crate::bytecode::CompiledCode { /* as before */ }),
                captured_frame: home_frame,
                home: home_frame,
            }),
        );
```

(with `kind: FrameKind::Closure(closure)` in the header; the body of the test
is otherwise unchanged — write the `CompiledCode` literal out in full, as it is
today).

- [ ] **Step 5: The interpreter, the VM and the natives**

In `crates/ns-runtime/src/interpreter/mod.rs`, `Step::Block`:

```rust
    /// Activate the closure object `closure`, and do `then` with its value.
    /// A builtin control-flow native's block is marked by its `args`,
    /// `StepArgs::ControlFlow`.
    Block {
        closure: ObjRef,
        args: StepArgs,
        then: Continuation,
    },
```

`push_step`'s block arm:

```rust
            Step::Block {
                closure,
                args,
                then,
            } => {
                let control_flow = matches!(args, StepArgs::ControlFlow);
                let args = args.slice(send_args);
                let (code, num_params, captured_frame) = self
                    .heap
                    .closure_activation(closure)
                    .expect("internal: a block step is decided only for a closure object");
                check_block_arity(&self.heap, num_params, captured_frame, args.len())?;
                // (the existing comment about the exemption, unchanged)
                let created_by_sender = |heap: &Heap| {
                    sender
                        .and_then(|sender| heap.activation_reified(sender))
                        .is_some_and(|frame| frame.same(captured_frame))
                };
                let counted = !(inlinable && control_flow && created_by_sender(&self.heap));
                let receiver = self.heap.frame_receiver(captured_frame);
                let current_class = self.heap.frame_current_class(captured_frame);
                let temps = (code.num_registers - code.num_args) as usize;
                self.push(
                    ActivationHeader {
                        receiver,
                        current_class,
                        kind: FrameKind::Closure(closure),
                        slot_base: 0, // a closure body never contains InitSlot
                        code,
                        then,
                        counted,
                    },
                    args,
                    temps,
                )
            }
```

```rust
fn check_block_arity(
    heap: &Heap,
    num_params: usize,
    captured_frame: FrameRef,
    given: usize,
) -> Result<(), Signal> {
    if given == num_params {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError {
        selector: Symbol::VALUE,
        receiver: heap.frame_receiver(captured_frame),
        message: format!(
            "wrong number of block arguments: block takes {num_params}, got {given}"
        ),
    }))
}
```

(keep its doc comment). `new_closure` takes `closure: ClosureData` and stores
`ObjectData::Closure(closure)`.

In `crates/ns-runtime/src/vm.rs`, `Instr::MakeClosure`:

```rust
            let v = Value::Obj(interp.new_closure(ClosureData {
                params: params.clone(),
                code: block.clone(),
                captured_frame,
                home,
            }));
```

In `crates/ns-runtime/src/builtins/block.rs` (drop `use std::rc::Rc;`):

```rust
pub(crate) fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    let Value::Obj(closure) = receiver else {
        return None;
    };
    let params = interp.heap_ref().closure_param_count(closure)?;
    Some(Ok(match selector.as_str() {
        "value" | "value:" | "value:value:" | "value:value:value:" => Step::Block {
            closure,
            args: StepArgs::Send,
            then: Continuation::Deliver,
        },
        "numArgs" => Step::Value(Value::SmallInt(params as i64)),
        "whileTrue:" => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: args[0],
            expect: true,
        })),
        "whileFalse:" => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: args[0],
            expect: false,
        })),
        _ => return None,
    }))
}
```

`condition_step`, `after_condition`, `after_body` and `body_step` take
`Box<WhileLoop>`; `condition_step` answers `closure: state.condition`, and
`body_step`:

```rust
fn body_step(interp: &Interpreter, state: Box<WhileLoop>) -> Step {
    if let Value::Obj(body) = state.body {
        if interp.heap_ref().is_closure(body) {
            return Step::Block {
                closure: body,
                args: StepArgs::ControlFlow,
                then: Continuation::WhileBody(state),
            };
        }
    }
    condition_step(state)
}
```

In `crates/ns-runtime/src/builtins/boolean.rs`, `block_step` asks
`interp.heap_ref().is_closure(obj)` and answers `Step::Block { closure: obj,
args: StepArgs::ControlFlow, then }`.

In `crates/ns-runtime/src/builtins/collections.rs`, `do_step`'s tail:

```rust
    // A non-block stands for its own value, so handing it the elements
    // observes nothing: the loop is over before it starts.
    let Value::Obj(block) = block else {
        return Some(Ok(Step::Value(receiver)));
    };
    if !interp.heap.is_closure(block) {
        return Some(Ok(Step::Value(receiver)));
    }
    Some(Ok(element_step(Box::new(DoLoop {
        receiver,
        block,
        items,
        next: 0,
    }))))
```

and `element_step` answers `closure: state.block`.

`Interpreter::resume` needs no change beyond the types it passes through.

- [ ] **Step 6: The image format**

In `crates/ns-runtime/src/image/opcodes.rs`, replace the `FRAME_CLOSURE` line:

```rust
/// Retired. A closure frame record once carried a copy of its closure's
/// record inline, from when the closure's data sat behind an `Rc` its object
/// and its activations shared. It names the closure object now
/// (`FRAME_CLOSURE_OBJECT`); the number stays assigned so that a stale
/// record is refused rather than read with a layout it does not have.
pub const FRAME_CLOSURE: u8 = 1;
```

and after `FRAME_SUPERCLASS_MESSAGE`:

```rust
/// A closure activation: the object id of the closure object it runs.
pub const FRAME_CLOSURE_OBJECT: u8 = 6;
```

In `crates/ns-runtime/src/image/encode.rs`: `GraphTable::visit_frame`'s
closure arm pushes the object —

```rust
                // The closure object a closure activation runs is a record
                // of its own, so it needs an id like any other object.
                FrameKind::Closure(closure) => {
                    stack.push(Pending::Value(Value::Obj(*closure)));
                }
```

— and `encode_frame` writes:

```rust
        FrameKind::Closure(closure) => {
            w.write_u8(FRAME_CLOSURE_OBJECT);
            w.write_u32(ctx.table.object_id(ctx.heap, *closure));
        }
```

`encode_object`'s closure arm is unchanged but for the type (`c` is now a
`&ClosureData`). Fix the `use` lists.

In `crates/ns-runtime/src/image/decode.rs`:

- `OBJ_CLOSURE => ObjectData::Closure(decode_closure_record(r, ctx, &tables, symbols)?),`
- in `decode_frame`, delete the `FRAME_CLOSURE` arm and add:

```rust
        FRAME_CLOSURE_OBJECT => {
            let id = r.read_u32()?;
            FrameKind::Closure(
                tables
                    .objects
                    .get(id as usize)
                    .copied()
                    .ok_or(DecodeError::BadObjectId(id))?,
            )
        }
```

- a new variant, beside `NotAClassReference`:

```rust
    /// A closure frame's kind names an object that is not a closure.
    NotAClosureReference,
```

  with `DecodeError::NotAClosureReference => write!(f, "a closure frame in the
  image names an object that is not a closure"),` in `Display`.
- in `check_class_references`'s frame loop, before the class checks:

```rust
        if let FrameKind::Closure(closure) = &kind {
            if !heap.is_closure(*closure) {
                return Err(DecodeError::NotAClosureReference);
            }
        }
```

  and extend its doc comment: it also holds every closure frame to naming a
  closure object, which, like a tag-2 class reference, is only knowable once
  every record has been read.
- `frame_body`'s closure arm: `FrameKind::Closure(closure) => heap.closure_code(*closure),`
- `verify_graph`: the first loop verifies every closure object
  (`if let Some(closure) = heap.closure_data(*object) { verify_closure(&closure, ...)?; }`);
  delete the second loop over closure frames, whose closures are now among
  those objects, and say so in a comment.

In `crates/ns-runtime/src/image/inspect.rs`, replace the `FRAME_CLOSURE` arm
with:

```rust
            opcodes::FRAME_CLOSURE_OBJECT => format!("closure object {}", r.read_u32()?),
```

`read_closure_record` stays for `OBJ_CLOSURE`. A retired tag 1 now falls to
the `InvalidTag` arm, as in the decoder.

Fix `tests/image.rs` where it builds `ObjectData::Closure(std::rc::Rc::new(
ns_runtime::ClosureData { .. }))` (around line 5267): drop the `Rc::new`.

- [ ] **Step 7: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, the three new image tests included. `the_wire_format_matches_the_golden_fixture`
passes unchanged — the golden world has no closure — which is the check that
nothing else in the format moved.

- [ ] **Step 8: Measure**

Follow the measurement protocol, `<parent-commit>` being the commit before
this task's. `MakeClosure` no longer allocates an `Rc`, a block activation
drops no `Rc` when it is popped, and a closure activation's outer-local reads
reach the captured frame through the object arena rather than an `Rc`.
`bench_block.ns`, which creates a closure per iteration, is the one to watch.
Also check that `size_of::<Object>()` is still 56 in a release build: add a
temporary `eprintln!` test, run it with `--release -- --nocapture`, and
delete it.

- [ ] **Step 9: Documents**

- `docs/design/object-model.md`: the `ObjectData` sketch (line 17) shows
  `Closure(ClosureData)`, and the paragraph at line 53 says the closure's
  payload is inline in the object and why (memory.md, "What the collector
  traces").
- `docs/design/execution.md`: the representation sketch (line 89) —
  `kind: FrameKind, // Closure(ObjRef): the closure object, whose payload
  carries home and captured_frame` — and the sentence around line 317 that
  names `ClosureData.code`.
- `docs/design/image-format.md`: "Object table" keeps its closure sentence;
  "Frame table" says a closure frame names its closure object by object id;
  the paragraph on retired families (around line 173) adds that
  `FRAME_CLOSURE` (1) is retired for `FRAME_CLOSURE_OBJECT` (6), and why.
- `docs/design/memory.md`, "What the collector traces": the paragraph that
  says `Rc<ClosureData>` breaks the rule says it did, until 2b split it, in
  the present tense of what is now true. (Task 8 rewrites the section whole;
  this keeps it from contradicting the code until then.)

- [ ] **Step 10: Gates and commit**

Run the gates. Then:

```bash
git add -A
git commit -m "refactor: keep a closure's frames in its object, not behind a shared Rc" -m "A handle behind a shared Rc cannot be rewritten in place, which the
tracer of step 2b needs. ClosureData now sits inline in the closure object;
FrameKind::Closure, WhileLoop, DoLoop and Step::Block name the closure
object, and the while loop's state is boxed. A closure frame record names
its closure object by id: FRAME_CLOSURE (1) is retired for
FRAME_CLOSURE_OBJECT (6).

<the measurement table, parent and d804eaf>

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 2: The root table and the rooted host API

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`Root`, the table, `stash`, `fetch`,
  `host_root_count`, `Debug for Heap`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`HostSignal`,
  `HostError`, `HostResult`, `stash`, `fetch`, `send`, `send_unrooted`,
  `platform`, `platform_value`, `to_host`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`instantiate`,
  `instantiate_unrooted`, `load_module`, `load_module_unrooted`)
- Modify: `crates/ns-runtime/src/image/mod.rs`, `image/decode.rs` (`platform()` →
  `platform_value()`)
- Modify: `crates/ns-runtime/src/lib.rs` (exports)
- Modify: `crates/ns-cli/src/entry.rs`, `source.rs`, `commands/run.rs`,
  `snapshot.rs`, `bench.rs`, `load.rs`, `build.rs`, `disasm.rs`
- Modify (mechanical rename only): every file under `crates/ns-runtime/tests/`,
  `crates/ns-cli/tests/disasm_snapshots.rs`, and `mod tests` in
  `crates/ns-runtime/src/interpreter/mod.rs`
- Create: `crates/ns-runtime/tests/roots.rs`

**Interfaces:**
- Produces, in `heap.rs`: `#[derive(Clone)] pub struct Root` (no `Debug`);
  `pub fn stash(&self, value: Value) -> Root`; `pub fn fetch(&self, root:
  &Root) -> Value`; `pub fn host_root_count(&self) -> usize`.
- Produces, in `interpreter/mod.rs`: `pub type HostResult = Result<Root,
  HostSignal>;`; `pub enum HostSignal { Error(HostError), Return }`;
  `pub struct HostError { pub selector: Symbol, pub receiver: Root, pub
  message: String }`; `pub fn stash(&self, value: Value) -> Root`;
  `pub fn fetch(&self, root: &Root) -> Value`; `pub fn send(&mut self,
  receiver: &Root, selector: Symbol, args: &[Root]) -> HostResult`;
  `pub fn platform(&mut self) -> Root`; `pub fn platform_value(&mut self) ->
  Value` (pub until Task 7, then `pub(crate)`).
- Produces, in `instantiate.rs`: `pub fn instantiate(&mut self, class: &Root,
  factory_args: &[Root]) -> HostResult`; `pub fn load_module(&mut self,
  resolved: &ResolvedModule) -> Root`.
- Transitional, removed in Task 7: `pub fn send_unrooted(&mut self, receiver:
  Value, selector: Symbol, args: Vec<Value>) -> EvalResult`, `pub fn
  instantiate_unrooted(&mut self, class: ClassRef, factory_args: Vec<Value>)
  -> EvalResult`, `pub fn load_module_unrooted(&mut self, resolved:
  &ResolvedModule) -> ClassRef` — today's three bodies under new names.
- `lib.rs` adds `Root` to the `heap` re-export and `HostError, HostResult,
  HostSignal` to the `interpreter` one.

- [ ] **Step 1: The mechanical rename**

Rename the four host entries and every caller, so that the final names are
free for the rooted API and everything still compiles. In
`crates/ns-runtime/tests/*.rs`, `crates/ns-cli/tests/disasm_snapshots.rs`
(the one CLI test that calls the runtime directly) and `mod tests` of
`crates/ns-runtime/src/interpreter/mod.rs`, replace `.send(` with
`.send_unrooted(`, `.instantiate(` with `.instantiate_unrooted(`,
`.load_module(` with `.load_module_unrooted(`, and `.platform()` with
`.platform_value()`. In `crates/ns-runtime/src/image/mod.rs` and
`image/decode.rs`, replace `.platform()` with `.platform_value()`. Rename the
three definitions and `platform` itself accordingly. In `crates/ns-cli/src`,
do the same for now; Step 5 converts the CLI properly.

Check that nothing else matched: `.send(` must only have hit `Interpreter`
calls (`compiler/reference.rs`'s `self.send(send)` is outside the files
above). Run `cargo test --workspace`; expected PASS, nothing changed but
names. Commit:

```bash
git add -A
git commit -m "refactor: free the host entry names for their rooted forms" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

- [ ] **Step 2: Write the failing tests**

Create `crates/ns-runtime/tests/roots.rs`:

```rust
//! A host keeps values across runs of the loop only as `Root`s: slots the
//! heap's root table also holds, which a moving collector traces and
//! rewrites (`docs/design/memory.md`, "Host roots").

use indoc::indoc;
use ns_runtime::{HostError, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn load(src: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

fn expect_error(interp: &Interpreter, result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(e)) => e,
        Ok(root) => panic!(
            "{what}: expected a Newspeak error, got {}",
            interp.heap_ref().describe(&interp.fetch(&root))
        ),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
    }
}

const COUNTER: &str = indoc! {"
    Newspeak3
    'T'
    class Counter = Object new (
      | n ::= 0. |
    ) (
      public bump: k = ( n := n + k. ^n )
    ) : ()
"};

#[test]
fn a_host_sends_to_roots_and_gets_roots_back() {
    let (mut interp, class) = load(COUNTER);
    let counter = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let bump = Symbol::intern("bump:");
    expect_ok(
        interp.send(&counter, bump, &[interp.stash(Value::SmallInt(2))]),
        "bump: 2",
    );
    let n = expect_ok(
        interp.send(&counter, bump, &[interp.stash(Value::SmallInt(3))]),
        "bump: 3",
    );
    assert!(matches!(interp.fetch(&n), Value::SmallInt(5)));
}

#[test]
fn a_newspeak_error_reaches_the_host_with_its_receiver_as_a_root() {
    let (mut interp, _class) = load(COUNTER);
    let three = interp.stash(Value::SmallInt(3));
    let result = interp.send(&three, Symbol::intern("frobnicate"), &[]);
    let e = expect_error(&interp, result, "3 frobnicate");
    assert_eq!(e.selector.as_str(), "frobnicate");
    assert!(matches!(interp.fetch(&e.receiver), Value::SmallInt(3)));
}

#[test]
fn instantiating_a_root_that_holds_no_class_is_a_newspeak_error() {
    let (mut interp, _class) = load(COUNTER);
    let three = interp.stash(Value::SmallInt(3));
    let result = interp.instantiate(&three, &[]);
    let e = expect_error(&interp, result, "instantiate 3");
    assert_eq!(e.message, "cannot instantiate 3: it is not a class");
}

/// A root nothing outside the table holds is swept, so a host that makes
/// many sends and keeps few results does not grow the table without bound
/// -- in a release build too, where nothing moves.
#[test]
fn roots_a_host_has_dropped_are_swept() {
    let (interp, class) = load(COUNTER);
    for i in 0..10_000 {
        let _ = interp.stash(Value::SmallInt(i));
    }
    assert!(
        interp.heap_ref().host_root_count() < 200,
        "the table holds {} roots after 10,000 dropped ones",
        interp.heap_ref().host_root_count()
    );
    assert!(
        matches!(interp.fetch(&class), Value::Obj(_)),
        "a root the host still holds survives every sweep"
    );
}

/// A root belongs to the heap that stashed it: another heap's table does
/// not hold it, so that heap's collector would never trace it.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "handle belongs to a different heap")]
fn fetching_a_root_through_another_interpreter_panics() {
    let (_interp, class) = load(COUNTER);
    let (other, _other_class) = load(COUNTER);
    let _ = other.fetch(&class);
}
```

- [ ] **Step 3: Run them and see them fail to compile**

Run: `cargo test -p ns-runtime --test roots`
Expected: compile errors — `Root`, `HostSignal`, `stash`, `fetch` and the
rooted `send`/`instantiate`/`load_module` do not exist.

- [ ] **Step 4: The table and the API**

In `crates/ns-runtime/src/heap.rs`, after the `Popped` struct:

```rust
/// A value a host keeps across runs of the loop (`docs/design/memory.md`,
/// "Host roots"): a slot the heap's root table also holds, which a moving
/// collector traces and rewrites. Cloning a `Root` shares the slot, and
/// dropping the last clone releases it without touching the heap -- the
/// table notices at its next sweep that it holds the only reference left.
/// gc-arena's `DynamicRoot` is the model.
///
/// No `Debug`, for the reason `Signal` has none: printing one would print
/// a `Value`.
#[derive(Clone)]
pub struct Root {
    slot: std::rc::Rc<std::cell::Cell<Value>>,
}

/// The host roots. Swept of slots nothing else holds at every move, and
/// whenever it has doubled since the last sweep, so it stays proportional to
/// the roots a host still holds even in a build where nothing moves.
struct HostRoots {
    slots: Vec<std::rc::Rc<std::cell::Cell<Value>>>,
    sweep_at: usize,
}

/// The table's size below which `stash` never sweeps.
const MIN_ROOT_SWEEP: usize = 64;
```

Add `host_roots: std::cell::RefCell<HostRoots>` to `Heap` (doc: "Every
`Root` a host holds. A `RefCell` so that `stash` and `fetch` take `&self`,
which lets a host stash an argument inside the call that sends it."),
initialize it in `Heap::new` with `slots: Vec::new(), sweep_at:
MIN_ROOT_SWEEP`, and add `.field("host roots", &self.host_roots.borrow().slots.len())`
to `Debug for Heap`. Then, in a new section after the object accessors:

```rust
    // ---------- Host roots ----------

    /// Roots `value` for a host. A value a host keeps past the next run of
    /// the loop must be a `Root`: a run may move every object
    /// (`docs/design/memory.md`, "Where a collection can happen"), and only
    /// what the collector traces is rewritten.
    pub fn stash(&self, value: Value) -> Root {
        if let Value::Obj(obj) = value {
            self.check_object_origin(obj);
        }
        let slot = std::rc::Rc::new(std::cell::Cell::new(value));
        let mut roots = self.host_roots.borrow_mut();
        if roots.slots.len() >= roots.sweep_at {
            roots.slots.retain(|slot| std::rc::Rc::strong_count(slot) > 1);
            roots.sweep_at = (roots.slots.len() * 2).max(MIN_ROOT_SWEEP);
        }
        roots.slots.push(slot.clone());
        Root { slot }
    }

    /// What `root` holds now. The answer is good until the next run of the
    /// loop, which may move it; read the root again after one.
    pub fn fetch(&self, root: &Root) -> Value {
        let value = root.slot.get();
        if let Value::Obj(obj) = value {
            self.check_object_origin(obj);
        }
        value
    }

    /// How many slots the root table holds, those nothing else holds any
    /// more but that no sweep has reached yet included.
    pub fn host_root_count(&self) -> usize {
        self.host_roots.borrow().slots.len()
    }
```

In `crates/ns-runtime/src/interpreter/mod.rs`, beside `Signal`:

```rust
/// How a host's send or instantiation ended (`Interpreter::send`,
/// `Interpreter::instantiate`): its value, rooted, or the signal that ended
/// it.
pub type HostResult = Result<Root, HostSignal>;

/// A signal that left a host's send or instantiation. No `Debug`, for the
/// reason `Signal` has none, so `unwrap` and `expect` on a `HostResult` do
/// not compile.
pub enum HostSignal {
    /// A Newspeak error nothing handled.
    Error(HostError),
    /// A `^` that reached the host: its home had returned, or it aimed at a
    /// running initializer, which catches none (`docs/design/execution.md`,
    /// "Non-local return").
    Return,
}

/// A Newspeak error as a host receives it: `NewspeakError` with its
/// receiver rooted, since the host may read it after running more code.
pub struct HostError {
    pub selector: Symbol,
    pub receiver: Root,
    pub message: String,
}
```

On `Interpreter`:

```rust
    /// `Heap::stash`, for a host.
    pub fn stash(&self, value: Value) -> Root {
        self.heap.stash(value)
    }

    /// `Heap::fetch`, for a host.
    pub fn fetch(&self, root: &Root) -> Value {
        self.heap.fetch(root)
    }

    /// An ordinary (explicit-receiver) send from a host: `newspeak-spec.pdf`
    /// §5.5, decided as `decide_send` decides it and run from the outside
    /// (`run_from_host`). A `Send*` the VM issues never comes through here.
    pub fn send(&mut self, receiver: &Root, selector: Symbol, args: &[Root]) -> HostResult {
        let receiver = self.fetch(receiver);
        let args: Vec<Value> = args.iter().map(|arg| self.fetch(arg)).collect();
        let result = match self.decide_send(receiver, selector, &args) {
            Ok(step) => self.run_from_host(step, &args),
            Err(signal) => Err(signal),
        };
        self.to_host(result)
    }

    /// A run's outcome as the host receives it. Nothing runs between the
    /// run's last safepoint and here, so every value in `result` is current.
    fn to_host(&self, result: EvalResult) -> HostResult {
        match result {
            Ok(value) => Ok(self.stash(value)),
            Err(Signal::Error(e)) => Err(HostSignal::Error(HostError {
                selector: e.selector,
                receiver: self.stash(e.receiver),
                message: e.message,
            })),
            Err(Signal::Return(..)) => Err(HostSignal::Return),
        }
    }

    /// The `platform` object, rooted for a host.
    pub fn platform(&mut self) -> Root {
        let platform = self.platform_value();
        self.stash(platform)
    }
```

Keep `send_unrooted` with a doc line: "Transitional: the unrooted form every
test used before 2b. Removed once they are converted (step 2b's plan, Task
7)." Same line on `instantiate_unrooted` and `load_module_unrooted`.

In `crates/ns-runtime/src/interpreter/instantiate.rs`:

```rust
    /// Instantiates the class `class` holds, from a host: the six numbered
    /// steps in `docs/design/object-model.md`, "Instantiation", decided
    /// (`decide_instantiation`) and then run from the outside
    /// (`run_from_host`), like a host's `send`. A root that holds anything
    /// but a class object is refused as a Newspeak error, never a panic:
    /// the caller is a host.
    pub fn instantiate(&mut self, class: &Root, factory_args: &[Root]) -> HostResult {
        let value = self.fetch(class);
        let class = match value {
            Value::Obj(obj) if self.heap.is_class(obj) => obj,
            _ => {
                return Err(HostSignal::Error(HostError {
                    selector: Symbol::NEW,
                    receiver: self.stash(value),
                    message: format!(
                        "cannot instantiate {}: it is not a class",
                        self.heap.describe(&value)
                    ),
                }))
            }
        };
        let factory_args: Vec<Value> = factory_args.iter().map(|arg| self.fetch(arg)).collect();
        let result = match self.decide_instantiation(class, factory_args) {
            Ok(step) => self.run_from_host(step, &[]),
            Err(signal) => Err(signal),
        };
        self.to_host(result)
    }

    /// Materializes the module's root class (`load_module_unrooted`), rooted
    /// for the host that will instantiate it.
    pub fn load_module(&mut self, resolved: &ResolvedModule) -> Root {
        let class = self.load_module_unrooted(resolved);
        self.stash(Value::Obj(class))
    }
```

(`to_host` is a private method of `Interpreter` in `mod.rs`; `instantiate.rs`
is a child module and can call it. Import `HostError`, `HostResult`,
`HostSignal` and `Root` where needed.)

`crates/ns-runtime/src/lib.rs`:

```rust
pub use heap::{ActivationHeader, ActivationRef, ClassRef, FrameRef, Heap, ObjRef, Root};
pub use interpreter::{HostError, HostResult, HostSignal, Interpreter, Signal};
```

- [ ] **Step 5: The CLI on roots**

`crates/ns-cli/src/entry.rs`:

```rust
/// Renders the entry send's outcome -- `program_outcomes.rs` pins these
/// strings byte for byte, so they live here once and are never duplicated.
pub(crate) fn report_entry_send(result: ns_runtime::HostResult) -> ExitCode {
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(ns_runtime::HostSignal::Error(e)) => {
            eprintln!("Uncaught Newspeak error: {}", e.message);
            ExitCode::FAILURE
        }
        Err(ns_runtime::HostSignal::Return) => {
            eprintln!("internal: non-local return escaped the entry send");
            ExitCode::FAILURE
        }
    }
}

fn report_signal(signal: ns_runtime::HostSignal) -> ExitCode {
    match signal {
        ns_runtime::HostSignal::Error(e) => {
            eprintln!("Uncaught Newspeak error during instantiation: {}", e.message);
        }
        ns_runtime::HostSignal::Return => {
            eprintln!("internal: non-local return escaped instantiation");
        }
    }
    ExitCode::FAILURE
}

/// The class object a module's root-class `Root` holds: `load_module` and
/// `image::load` only ever root a class object there.
pub(crate) fn class_ref(
    interp: &ns_runtime::Interpreter,
    class: &ns_runtime::Root,
) -> ns_runtime::ClassRef {
    match interp.fetch(class) {
        ns_runtime::Value::Obj(class) => class,
        _ => unreachable!("internal: a module's root class is a class object"),
    }
}
```

`factory_args_for(heap, class: ClassRef, platform: &Root) -> Result<Vec<Root>,
String>` answers `vec![platform.clone()]` for `usingPlatform:` (its doc and
error text unchanged). `entry_args(interp: &Interpreter, selector: &str,
platform: &Root) -> Result<Vec<Root>, String>` answers `vec![platform.clone(),
interp.stash(Value::Nil)]` for two arguments. `instantiate_root(interp, class:
&Root) -> Result<Root, ExitCode>`:

```rust
    let platform = interp.platform();
    let factory_args = factory_args_for(interp.heap_ref(), class_ref(interp, class), &platform)
        .map_err(|msg| {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        })?;
    interp
        .instantiate(class, &factory_args)
        .map_err(report_signal)
```

`source::bootstrap` answers `(Interpreter, Root)`. Then:

- `run.rs`: `instantiate_root(&mut interp, &class)`, `let platform =
  interp.platform();`, `entry_args(&interp, send, &platform)`,
  `report_entry_send(interp.send(&instance, selector, &args))`.
- `snapshot.rs`: `roots: Vec<Root>`; after the sends, fetch right before
  encoding, since encoding runs no code:

  ```rust
      let platform = interp.fetch(&platform);
      let root_values: Vec<ns_runtime::Value> = roots.iter().map(|r| interp.fetch(r)).collect();
      let bytes = match ns_runtime::image::snapshot(
          &interp,
          debug_source.then_some(loaded.source.as_str()),
          &loaded.origin,
          entry::class_ref(&interp, &class),
          &platform,
          &root_values,
      ) {
  ```

- `bench.rs`: `timed_send(interp, class: &Root, factory_args: &[Root],
  selector, entry_args: &[Root])`, `signal_message(signal: HostSignal)` with
  the same strings, `bench_entry` roots the platform and builds both lists
  once.
- `build.rs`: fetch the class with `entry::class_ref` where it needs a
  `ClassRef`.
- `disasm.rs`: the source path uses `entry::class_ref(&interp, &class)`; the
  image path is unchanged until Task 6.
- `load.rs`: until Task 6, `image::load` still answers plain values; root them
  at once, before anything runs:

  ```rust
      let root_class = interp.stash(ns_runtime::Value::Obj(root_class));
      let roots: Vec<ns_runtime::Root> = roots.into_iter().map(|v| interp.stash(v)).collect();
  ```

  then `instantiate_root(&mut interp, &root_class)`, the stateful branch's
  `roots.into_iter().next()` answers a `Root`, and the send is
  `interp.send(&instance, Symbol::intern(selector), &args)` with `args` from
  `entry_args(&interp, selector, &platform)` after `let platform =
  interp.platform();`.

- [ ] **Step 6: Run the tests**

Run: `cargo test --workspace`
Expected: PASS — `roots.rs` included, and every golden in
`crates/ns-cli/tests/program_outcomes.rs` unchanged.

- [ ] **Step 7: Gates and commit**

```bash
git add -A
git commit -m "feat: hand a host its values as roots" -m "Interpreter::send, instantiate, load_module and platform take and answer
Roots, slots the heap's root table also holds, so that a moving collector
can rewrite what a host keeps across runs. A Newspeak error reaches the
host as a HostError whose receiver is a Root. The CLI uses the rooted API;
the tests still use the old entries under transitional names.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 3: The epoch, the tracing and the move

**Files:**
- Create: `crates/ns-runtime/src/trace.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (`#[cfg(debug_assertions)] pub(crate) mod trace;`)
- Modify: `crates/ns-runtime/src/heap.rs` (the epoch on `ObjRef`/`FrameRef`
  and `Heap`, the checks, `Forwarder`, `Rotation`, `move_everything`,
  `Trace for StackEntry`, `move_epoch`, unit tests)
- Create: `crates/ns-runtime/src/interpreter/safepoint.rs` (`move_everything`,
  `move_now`, `moves`, `check_quiescent_state`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`mod safepoint;`, the
  argument buffer, the decision-size test)
- Modify: `CLAUDE.md`, `docs/design/memory.md` (the debug sizes)
- Test: `crates/ns-runtime/tests/roots.rs`

**Interfaces:**
- Consumes: Task 2's `Root`, `stash`, `fetch`, the rooted `send` and
  `instantiate`.
- Produces, in `trace.rs` (debug only): `pub(crate) trait Tracer { fn
  object(&mut self, handle: &mut ObjRef); fn frame(&mut self, handle: &mut
  FrameRef); fn value(&mut self, value: &mut Value) }` and `pub(crate) trait
  Trace { fn trace<T: Tracer>(&mut self, tracer: &mut T); }`, implemented for
  `Object`, `ObjectData`, `ClosureData`, `ClassData`, `Frame`, `FrameKind`,
  `Continuation`, `WhileLoop`, `DoLoop`, `NestedClassCreation`,
  `Instantiation`, `InstantiationPhase`; `heap.rs` implements it for
  `StackEntry`.
- Produces, in `heap.rs` (debug only): `pub(crate) struct Forwarder` (a
  `Tracer`); `pub(crate) fn move_everything(&mut self) -> Forwarder`;
  `pub(crate) fn move_epoch(&self) -> u32`.
- Produces, on `Interpreter` (debug only): `pub fn move_now(&mut self)`,
  `pub fn moves(&self) -> u32`, `pub(crate) fn move_everything(&mut self)`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/ns-runtime/tests/roots.rs`:

```rust
const GRAPH: &str = indoc! {"
    Newspeak3
    'T'
    class X = Object new (
      | cell ::= nil. counter ::= nil. |
    ) (
      public class Cell = Object new (
        | v ::= 7. |
      ) (
        public v = ( ^v )
      ) : ()
      public build = (
        | n |
        n := 40.
        counter := [n := n + 1. n].
        cell := Cell new.
        ^{cell. counter}
      )
      public read = ( ^cell v + counter value )
    ) : (
      public marker = ( ^3 )
    )
"};

/// What a host holds as roots survives a move of every object and every
/// reified frame, and so does everything reachable from it: an instance's
/// slots, a nested class and its enclosing object, a closure and the
/// divorced frame whose variable it writes, a cached metaclass, and the
/// builtin classes every send to an integer reaches.
#[test]
#[cfg(debug_assertions)]
fn a_world_reached_from_roots_survives_moves() {
    let (mut interp, class) = load(GRAPH);
    let x = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let built = expect_ok(interp.send(&x, Symbol::intern("build"), &[]), "build");
    let before = interp.moves();
    interp.move_now();
    let read = expect_ok(interp.send(&x, Symbol::intern("read"), &[]), "read");
    assert!(matches!(interp.fetch(&read), Value::SmallInt(48)));
    interp.move_now();
    let read = expect_ok(interp.send(&x, Symbol::intern("read"), &[]), "read");
    assert!(matches!(interp.fetch(&read), Value::SmallInt(49)));
    let marker = expect_ok(interp.send(&class, Symbol::intern("marker"), &[]), "marker");
    assert!(matches!(interp.fetch(&marker), Value::SmallInt(3)));
    interp.move_now();
    let marker = expect_ok(interp.send(&class, Symbol::intern("marker"), &[]), "marker");
    assert!(matches!(interp.fetch(&marker), Value::SmallInt(3)));
    assert_eq!(interp.moves(), before + 3);
    let heap = interp.heap_ref();
    let Value::Obj(built) = interp.fetch(&built) else {
        panic!("`build` answers an array")
    };
    assert_eq!(heap.array_len(built), Some(2));
    let cell = heap.array_at(built, 0).expect("the cell");
    assert_eq!(heap.describe(&cell), "an instance of Cell");
}

/// A value a host fetched and kept past a move names nothing any more: the
/// debug build refuses the read rather than answer for whatever object took
/// its slot.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "stale handle")]
fn a_value_kept_past_a_move_is_refused() {
    let (mut interp, class) = load(COUNTER);
    let counter = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let kept = interp.fetch(&counter);
    interp.move_now();
    let _ = interp.heap_ref().describe(&kept);
}
```

In `crates/ns-runtime/src/heap.rs`'s `mod tests`:

```rust
    /// A move gives every object and every frame a new index -- the
    /// rotation has no fixed point -- and rewrites everything that names
    /// one: an array's element, a closure's two frames, a frame's receiver,
    /// a class's own class, and the host roots.
    #[test]
    #[cfg(debug_assertions)]
    fn a_move_gives_every_slot_a_new_index_and_rewrites_what_names_it() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let a = heap.alloc_object(class, ObjectData::Str("a".to_string()));
        let b = heap.alloc_object(class, ObjectData::Array(vec![Value::Obj(a)]));
        let frame = heap.alloc_frame(Frame {
            receiver: Value::Obj(a),
            ..top_level_frame()
        });
        heap.alloc_frame(top_level_frame());
        let closure = heap.alloc_object(
            class,
            ObjectData::Closure(ClosureData {
                params: std::rc::Rc::new(vec![]),
                code: trivial_code(),
                captured_frame: frame,
                home: frame,
            }),
        );
        let before = [
            heap.object_identity_key(class),
            heap.object_identity_key(a),
            heap.object_identity_key(b),
            heap.object_identity_key(closure),
        ];
        let frame_before = heap.frame_identity_key(frame);
        let roots = [
            heap.stash(Value::Obj(class)),
            heap.stash(Value::Obj(a)),
            heap.stash(Value::Obj(b)),
            heap.stash(Value::Obj(closure)),
        ];
        heap.move_everything();
        let after: Vec<ObjRef> = roots
            .iter()
            .map(|root| match heap.fetch(root) {
                Value::Obj(obj) => obj,
                _ => panic!("a root keeps its object"),
            })
            .collect();
        for (key, obj) in before.iter().zip(&after) {
            assert_ne!(*key, heap.object_identity_key(*obj), "every object moved");
        }
        let (class, a, b, closure) = (after[0], after[1], after[2], after[3]);
        assert!(heap.class_of_obj(class).same(class), "a class's own class is rewritten");
        assert!(matches!(heap.array_at(b, 0), Some(Value::Obj(e)) if e.same(a)));
        assert_eq!(heap.str_value(a).as_deref(), Some("a"));
        let data = heap.closure_data(closure).expect("still a closure");
        assert!(data.captured_frame.same(data.home));
        assert_ne!(frame_before, heap.frame_identity_key(data.captured_frame), "every frame moved");
        assert!(matches!(heap.frame_receiver(data.captured_frame), Value::Obj(r) if r.same(a)));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn reading_an_object_handle_minted_before_a_move_panics() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let a = heap.alloc_object(class, ObjectData::Str("a".to_string()));
        heap.move_everything();
        let _ = heap.str_value(a);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn reading_a_frame_handle_minted_before_a_move_panics() {
        let mut heap = Heap::new();
        let frame = heap.alloc_frame(top_level_frame());
        heap.alloc_frame(top_level_frame());
        heap.move_everything();
        let _ = heap.frame_receiver(frame);
    }

    /// `same` reads no heap, so a stale handle compared with a fresh one
    /// must be refused there too, or the two could answer "identical".
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle")]
    fn comparing_a_stale_handle_with_a_fresh_one_panics() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let root = heap.stash(Value::Obj(class));
        heap.move_everything();
        let Value::Obj(fresh) = heap.fetch(&root) else {
            panic!("a root keeps its object")
        };
        let _ = class.same(fresh);
    }

    /// A stale handle written into the heap -- a host that stored a value it
    /// kept past a run -- is caught by the next move, which finds it among
    /// the handles it forwards, rather than forwarded as if it were current.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale handle is stored in the heap")]
    fn a_stale_handle_stored_in_the_heap_is_caught_by_the_next_move() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let stale = heap.alloc_object(class, ObjectData::Str("stale".to_string()));
        let array = heap.alloc_object(class, ObjectData::Array(vec![]));
        let root = heap.stash(Value::Obj(array));
        heap.move_everything();
        let Value::Obj(array) = heap.fetch(&root) else {
            panic!("a root keeps its object")
        };
        heap.array_push(array, Value::Obj(stale));
        heap.move_everything();
    }
```

- [ ] **Step 2: Run them and see them fail to compile**

Run: `cargo test -p ns-runtime --lib --test roots`
Expected: compile errors — `move_everything`, `move_now` and `moves` do not
exist.

- [ ] **Step 3: The epoch**

In `heap.rs`, add to `ObjRef` and `FrameRef`, after `origin`:

```rust
    /// The heap's move epoch when this handle was minted or last forwarded.
    /// Debug builds only: a move rewrites every handle the tracer knows
    /// about into the new epoch, so one still carrying an old epoch is one
    /// the tracer missed, and reading it panics instead of answering for
    /// whatever object took its slot (`docs/design/memory.md`, "Checking
    /// the roots before a collector exists: the stress mode").
    #[cfg(debug_assertions)]
    epoch: u32,
```

Add `#[cfg(debug_assertions)] epoch: u32,` to `Heap` (doc: "How many times
this heap has moved everything. Debug builds only; see `ObjRef::epoch`."),
initialized to 0; mint it in `object_handle` and `frame_handle`
(`#[cfg(debug_assertions)] epoch: self.epoch,`).

`check_object_origin` and `check_frame_origin` assert, after the origin:

```rust
        #[cfg(debug_assertions)]
        assert_eq!(
            obj.epoch, self.epoch,
            "stale handle: minted before move {} of this heap, read after move {} -- \
             a host keeps a value across a run only as a `Root`",
            obj.epoch, self.epoch
        );
```

`ObjRef::same` and `FrameRef::same` assert, after the origin:

```rust
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.epoch, other.epoch,
            "stale handle: comparing handles minted in move epochs {} and {}",
            self.epoch, other.epoch
        );
```

```rust
    /// How many times this heap has moved everything.
    #[cfg(debug_assertions)]
    pub(crate) fn move_epoch(&self) -> u32 {
        self.epoch
    }
```

- [ ] **Step 4: What the collector traces**

Create `crates/ns-runtime/src/trace.rs`:

```rust
//! What the collector traces (`docs/design/memory.md`, "What the collector
//! traces"): every field of every runtime type that holds an `ObjRef`, a
//! `ClassRef` or a `FrameRef`, handed to a `Tracer` one `&mut` at a time.
//! Enumerating is this module's; rewriting is `heap.rs`'s `Forwarder`, since
//! only `heap.rs` can mint a handle.
//!
//! Every impl names every field and matches every variant, with no `..` and
//! no `_`: a field or variant added later fails to compile here until
//! someone decides whether it holds a handle. That is what keeps the
//! enumeration complete as the types grow; the stress mode checks that the
//! places it is applied to are complete.
//!
//! Debug builds only, until the collector (step 5) needs it in every build.

use crate::heap::{FrameRef, ObjRef};
use crate::types::*;

pub(crate) trait Tracer {
    fn object(&mut self, handle: &mut ObjRef);
    fn frame(&mut self, handle: &mut FrameRef);
    fn value(&mut self, value: &mut Value) {
        match value {
            Value::Obj(handle) => self.object(handle),
            Value::Nil | Value::Bool(_) | Value::SmallInt(_) | Value::Float(_) => {}
        }
    }
}

pub(crate) trait Trace {
    fn trace<T: Tracer>(&mut self, tracer: &mut T);
}

fn trace_values<T: Tracer>(values: &mut [Value], tracer: &mut T) {
    for value in values {
        tracer.value(value);
    }
}

fn trace_optional_object<T: Tracer>(handle: &mut Option<ObjRef>, tracer: &mut T) {
    if let Some(handle) = handle {
        tracer.object(handle);
    }
}

impl Trace for Object {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let Object {
            class,
            identity_hash: _,
            data,
        } = self;
        tracer.object(class);
        data.trace(tracer);
    }
}

impl Trace for ObjectData {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            ObjectData::Instance(values) | ObjectData::Array(values) => {
                trace_values(values, tracer)
            }
            ObjectData::Str(_) | ObjectData::Sym(_) => {}
            ObjectData::Closure(closure) => closure.trace(tracer),
            ObjectData::Class(class) => class.trace(tracer),
        }
    }
}

impl Trace for ClosureData {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let ClosureData {
            params: _,
            code: _,
            captured_frame,
            home,
        } = self;
        tracer.frame(captured_frame);
        tracer.frame(home);
    }
}

impl Trace for ClassData {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        // `application` holds no handle: a mixin application is behaviour,
        // shared by every class object built from it.
        let ClassData {
            application: _,
            enclosing,
            declaring_class,
            superclass,
            builtin: _,
            metaclass,
        } = self;
        trace_optional_object(enclosing, tracer);
        trace_optional_object(declaring_class, tracer);
        trace_optional_object(superclass, tracer);
        trace_optional_object(metaclass, tracer);
    }
}

impl Trace for Frame {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let Frame {
            receiver,
            args,
            temps,
            pc: _,
            sender,
            home,
            current_class,
            kind,
            uncontinuable: _,
            slot_base: _,
        } = self;
        tracer.value(receiver);
        trace_values(args, tracer);
        trace_values(temps, tracer);
        if let Some(sender) = sender {
            tracer.frame(sender);
        }
        if let Some(home) = home {
            tracer.frame(home);
        }
        trace_optional_object(current_class, tracer);
        kind.trace(tracer);
    }
}

impl Trace for FrameKind {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            FrameKind::Method(_) | FrameKind::TopLevel => {}
            FrameKind::Closure(closure) => tracer.object(closure),
            FrameKind::Initializer(class)
            | FrameKind::SuperclassClause(class)
            | FrameKind::SuperclassMessage(class) => tracer.object(class),
        }
    }
}

impl Trace for Continuation {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            Continuation::Deliver | Continuation::Truthy => {}
            Continuation::WhileCondition(state) | Continuation::WhileBody(state) => {
                state.trace(tracer)
            }
            Continuation::DoElement(state) => state.trace(tracer),
            Continuation::NestedClass(state) => state.trace(tracer),
            Continuation::Instantiate(state) => state.trace(tracer),
        }
    }
}

impl Trace for WhileLoop {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let WhileLoop {
            condition,
            body,
            expect: _,
        } = self;
        tracer.object(condition);
        tracer.value(body);
    }
}

impl Trace for DoLoop {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let DoLoop {
            receiver,
            block,
            items,
            next: _,
        } = self;
        tracer.value(receiver);
        tracer.object(block);
        trace_values(items, tracer);
    }
}

impl Trace for NestedClassCreation {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let NestedClassCreation {
            enclosing,
            index: _,
            nested: _,
            declaring_class,
        } = self;
        tracer.object(enclosing);
        tracer.object(declaring_class);
    }
}

impl Trace for Instantiation {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        let Instantiation {
            instance,
            levels,
            phase,
        } = self;
        tracer.value(instance);
        for (level, args) in levels {
            tracer.object(level);
            trace_values(args, tracer);
        }
        phase.trace(tracer);
    }
}

impl Trace for InstantiationPhase {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        match self {
            InstantiationPhase::Climbing { level, superclass } => {
                tracer.object(level);
                tracer.object(superclass);
            }
            InstantiationPhase::Descending { index: _ } => {}
        }
    }
}
```

In `lib.rs`, after `pub mod symbol;`:

```rust
#[cfg(debug_assertions)]
pub(crate) mod trace;
```

- [ ] **Step 5: The move**

In `heap.rs`, a new section at the end of `impl Heap` and the types it needs
above `impl Heap`:

```rust
/// A rotation of one arena: slot `i` moves to `(i + shift) % len`.
#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
struct Rotation {
    shift: u32,
    len: u32,
}

#[cfg(debug_assertions)]
impl Rotation {
    /// A shift in `1..len`, varied by `epoch` so that successive moves do
    /// not undo one another. No slot keeps its index, so a handle the
    /// tracer missed both carries an old epoch and names a different
    /// object: the permutation checks the forwarding, the epoch the
    /// completeness. An arena of one slot or none cannot move anything.
    fn new(len: usize, epoch: u32) -> Rotation {
        let len = u32::try_from(len).expect("an arena is addressed by 32-bit indices");
        let shift = if len < 2 { 0 } else { 1 + epoch % (len - 1) };
        Rotation { shift, len }
    }

    fn apply(self, index: u32) -> u32 {
        if self.len == 0 {
            return index;
        }
        ((u64::from(index) + u64::from(self.shift)) % u64::from(self.len)) as u32
    }
}

/// The tracer a move forwards every handle with. It also checks each handle
/// it forwards: one from another heap, or one already stale, was stored in
/// this heap by a mistake the move would otherwise make permanent.
#[cfg(debug_assertions)]
pub(crate) struct Forwarder {
    heap_id: u32,
    from: u32,
    to: u32,
    objects: Rotation,
    frames: Rotation,
}

#[cfg(debug_assertions)]
impl crate::trace::Tracer for Forwarder {
    fn object(&mut self, handle: &mut ObjRef) {
        assert_eq!(
            handle.origin, self.heap_id,
            "a handle from another heap is stored in this one"
        );
        assert_eq!(
            handle.epoch, self.from,
            "a stale handle is stored in the heap: minted before move {}, found during move {}",
            handle.epoch, self.to
        );
        handle.index = self.objects.apply(handle.index);
        handle.epoch = self.to;
    }

    fn frame(&mut self, handle: &mut FrameRef) {
        assert_eq!(
            handle.origin, self.heap_id,
            "a handle from another heap is stored in this one"
        );
        assert_eq!(
            handle.epoch, self.from,
            "a stale handle is stored in the heap: minted before move {}, found during move {}",
            handle.epoch, self.to
        );
        handle.index = self.frames.apply(handle.index);
        handle.epoch = self.to;
    }
}

#[cfg(debug_assertions)]
impl crate::trace::Trace for StackEntry {
    fn trace<T: crate::trace::Tracer>(&mut self, tracer: &mut T) {
        let StackEntry {
            receiver,
            current_class,
            kind,
            base: _,
            num_args: _,
            pc: _,
            slot_base: _,
            code: _,
            then,
            counted: _,
            reified,
            generation: _,
        } = self;
        tracer.value(receiver);
        if let Some(class) = current_class {
            tracer.object(class);
        }
        kind.trace(tracer);
        then.trace(tracer);
        if let Some(frame) = reified {
            tracer.frame(frame);
        }
    }
}
```

```rust
    // ---------- Moving everything (debug builds only) ----------

    /// Moves every object and every reified frame to a new index and
    /// rewrites every handle the heap itself holds -- in every arena record,
    /// in every stack entry and the continuation it owns, in every register,
    /// and in every host root -- into the next epoch. What the heap does not
    /// hold, the caller forwards with the answer: the interpreter's own
    /// roots (`Interpreter::move_everything`). A compacting collection that
    /// treats everything as live; the stress mode runs one at safepoints
    /// (`docs/design/memory.md`, "Checking the roots before a collector
    /// exists").
    ///
    /// Sound only where nothing outside the heap and the interpreter holds a
    /// handle: at a safepoint of the run loop, or between runs.
    #[cfg(debug_assertions)]
    pub(crate) fn move_everything(&mut self) -> Forwarder {
        use crate::trace::{Trace, Tracer};
        let from = self.epoch;
        let to = from.wrapping_add(1);
        let objects = Rotation::new(self.objects.len(), to);
        let frames = Rotation::new(self.frames.len(), to);
        self.objects.rotate_right(objects.shift as usize);
        self.frames.rotate_right(frames.shift as usize);
        self.epoch = to;
        let mut forwarder = Forwarder {
            heap_id: self.heap_id,
            from,
            to,
            objects,
            frames,
        };
        for object in &mut self.objects {
            object.trace(&mut forwarder);
        }
        // A married frame's registers are its entry's, traced below with
        // the stack; `live` is a stack index, and no stack entry moves.
        for slot in &mut self.frames {
            slot.frame.trace(&mut forwarder);
        }
        for entry in &mut self.stack.entries {
            entry.trace(&mut forwarder);
        }
        for value in &mut self.stack.values {
            forwarder.value(value);
        }
        let roots = self.host_roots.get_mut();
        roots.slots.retain(|slot| std::rc::Rc::strong_count(slot) > 1);
        for slot in &roots.slots {
            let mut value = slot.get();
            forwarder.value(&mut value);
            slot.set(value);
        }
        forwarder
    }
```

(`Vec::rotate_right(k)` moves element `i` to `(i + k) % len`, which is what
`Rotation::apply` computes.)

- [ ] **Step 6: The interpreter's roots, and the buffer**

In `crates/ns-runtime/src/interpreter/mod.rs`, add `mod safepoint;` beside the
other modules, and clear the argument buffer when it is given back:

```rust
    pub(crate) fn take_arg_buffer(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.arg_buffer)
    }

    /// Cleared here rather than when it is taken, so that between two
    /// instructions -- at every safepoint -- the buffer holds no value, and
    /// so no handle a move would have to find.
    pub(crate) fn return_arg_buffer(&mut self, mut buffer: Vec<Value>) {
        buffer.clear();
        self.arg_buffer = buffer;
    }
```

Update the `arg_buffer` field's doc to say it is empty between
instructions.

Create `crates/ns-runtime/src/interpreter/safepoint.rs`:

```rust
//! The interpreter's side of a move: its own roots, forwarded after the
//! heap's, and the state that must hold no handle when one happens. The
//! run loop's safepoint (Task 4 of step 2b's plan) calls into this.

#[cfg(debug_assertions)]
use crate::trace::Tracer;
#[cfg(debug_assertions)]
use crate::types::*;

use super::Interpreter;

#[cfg(debug_assertions)]
impl Interpreter {
    /// Moves every object and every reified frame (`Heap::move_everything`)
    /// and forwards what the interpreter holds itself: the bootstrap's
    /// builtin classes and the `platform` singleton.
    pub(crate) fn move_everything(&mut self) {
        self.check_quiescent_state();
        let mut forwarder = self.heap.move_everything();
        for class in &mut self.builtin_classes {
            forwarder.object(class);
        }
        if let Some(platform) = &mut self.platform_singleton {
            forwarder.value(platform);
        }
    }

    /// Moves everything now, between runs, where only roots hold values. For
    /// tests: the stress mode moves at safepoints of the run loop instead.
    pub fn move_now(&mut self) {
        assert!(!self.running, "move_now is for between runs");
        self.move_everything();
    }

    /// How many times this interpreter's heap has moved everything.
    pub fn moves(&self) -> u32 {
        self.heap.move_epoch()
    }

    /// What must hold no handle when everything moves, because nothing
    /// traces it: the argument buffer, which a send fills and empties within
    /// one instruction, and the spare boxes finished instantiations leave
    /// for the next ones (`docs/design/memory.md`, "What the collector
    /// traces").
    fn check_quiescent_state(&self) {
        assert!(
            self.arg_buffer.is_empty(),
            "internal: the argument buffer holds values between instructions"
        );
        assert!(
            self.spare_walks.iter().all(|walk| {
                matches!(walk.instance, Value::Nil)
                    && walk.levels.is_empty()
                    && matches!(walk.phase, InstantiationPhase::Descending { .. })
            }),
            "internal: a spare instantiation box holds a handle"
        );
    }
}
```

(If `use super::Interpreter;` is then unused in a release build, gate it with
`#[cfg(debug_assertions)]` too; the module has nothing in it for a release
build until Task 4.)

Pin `a_decision_stays_the_size_the_hot_path_was_measured_at` to the build it
was measured in:

```rust
    /// ... (existing text) This is the size the hot path was measured at, in
    /// a release build: a debug build's handles carry an origin and a move
    /// epoch besides their index, so it measures something else. The
    /// measurement protocol runs this with `--release`.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn a_decision_stays_the_size_the_hot_path_was_measured_at() {
```

- [ ] **Step 7: Run the tests**

Run: `cargo test --workspace`
Expected: PASS, the new tests included.

Run: `cargo test --release -p ns-runtime --lib a_decision_stays_the_size_the_hot_path_was_measured_at`
Expected: PASS (56 bytes).

- [ ] **Step 8: Measure the record sizes, and correct the figures that
  quote them**

Add a temporary test to `heap.rs`'s `mod tests`:

```rust
    #[test]
    fn print_record_sizes() {
        eprintln!(
            "Object {} FrameSlot {}",
            std::mem::size_of::<Object>(),
            std::mem::size_of::<FrameSlot>()
        );
    }
```

Run it both ways: `cargo test -p ns-runtime --lib print_record_sizes -- --nocapture`
and the same with `--release`. The release figures must still be 56 and 136;
if they are not, stop and report. The debug figures have grown by the epoch.
Delete the test, then correct the debug column of the amplification table in
`docs/design/memory.md` ("Open: what the collector owns": record size /
minimum record size, per column, and the combined factor as their sum) and
the sentence in `CLAUDE.md` that quotes "about 21.3x in a debug build" to the
measured figures, saying the epoch is why they grew.

- [ ] **Step 9: Gates and commit**

```bash
git add -A
git commit -m "feat: trace every handle, and move everything in a debug build" -m "trace.rs enumerates every field that holds a handle, and a move rotates
both arenas with no fixed point and forwards every handle the heap and the
interpreter hold. Every ObjRef and FrameRef carries its move epoch in a
debug build, so a handle the tracer missed panics at its first read.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 4: The safepoint and the stress mode

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`safepoint_requested` and its
  accessors)
- Modify: `crates/ns-runtime/src/vm.rs` (`poll`, `run`)
- Modify: `crates/ns-runtime/src/interpreter/safepoint.rs` (`safepoint`,
  `StressMoves`, `STRESS_MOVE_BUDGET`, `set_stress_moves`, unit tests)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (the `stress` field,
  `bootstrap_with`)
- Create: `crates/ns-runtime/tests/stress.rs`

**Interfaces:**
- Consumes: Task 3's `Interpreter::move_everything` and `moves`.
- Produces: `Heap::safepoint_requested(&self) -> bool` (all builds, `#[inline]`);
  `Heap::set_safepoint_requested(&mut self, on: bool)` (debug);
  `Interpreter::safepoint(&mut self)` (all builds, `#[cold]`);
  `pub(crate) const STRESS_MOVE_BUDGET: usize = 1_000` (debug);
  `pub fn set_stress_moves(&mut self, on: bool)` (debug);
  `Heap::traced_size(&self) -> usize` (debug).

- [ ] **Step 1: Write the failing tests**

Create `crates/ns-runtime/tests/stress.rs`:

```rust
//! The stress mode: a debug build that moves every object and every reified
//! frame at the run loop's safepoints, so that a handle the tracer does not
//! know about panics at its first read (`docs/design/memory.md`, "Checking
//! the roots before a collector exists: the stress mode").
#![cfg(debug_assertions)]

use indoc::indoc;
use ns_runtime::{HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn load(src: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

/// Every continuation that holds a handle is on the stack while the block or
/// body it waits for runs, and a move happens at every safepoint of a heap
/// this small: a `do:` loop's receiver, block and elements, a native
/// `whileTrue:`'s condition and body, an instantiation's instance, levels
/// and arguments on the way up and down, and a nested class's enclosing
/// object and declaring class.
const EVERY_CONTINUATION: &str = indoc! {"
    Newspeak3
    'T'
    class S = Object new (
    ) (
      public class Base x: a = Object new (
        | u ::= a. |
      ) (
        public u = ( ^u )
      ) : ()
      public class Cell = Base x: 5 (
        | v ::= 1. |
      ) (
        public v = ( ^v )
      ) : ()
      public class Late = self base (
      ) (
        public w = ( ^9 )
      ) : ()
      base = ( ^Object )
      public run = (
        | sum n cond body |
        sum := 0.
        #(1 2 3 4) do: [:e | sum := sum + e].
        n := 0.
        cond := [n < 3].
        body := [n := n + 1. sum := sum + n].
        cond whileTrue: body.
        sum := sum + Cell new u + Cell new v.
        sum := sum + Late new w.
        ^sum
      )
    ) : ()
"};

fn run_every_continuation(stress: bool) -> (i64, u32) {
    let (mut interp, class) = load(EVERY_CONTINUATION);
    interp.set_stress_moves(stress);
    let s = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let sum = expect_ok(interp.send(&s, Symbol::intern("run"), &[]), "run");
    let Value::SmallInt(sum) = interp.fetch(&sum) else {
        panic!("`run` answers an integer")
    };
    (sum, interp.moves())
}

#[test]
fn every_continuation_survives_a_move_at_every_safepoint() {
    assert_eq!(run_every_continuation(false), (31, 0), "the control");
    let (sum, moves) = run_every_continuation(true);
    assert_eq!(sum, 31);
    assert!(moves > 20, "only {moves} moves: the stress mode did not run");
}

/// The mistake the stress mode exists to catch, made on purpose: a host
/// keeping a plain value across a run.
#[test]
#[should_panic(expected = "stale handle")]
fn a_value_a_host_keeps_across_a_run_is_refused_under_stress() {
    let (mut interp, class) = load(EVERY_CONTINUATION);
    interp.set_stress_moves(true);
    let s = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let kept = interp.fetch(&s);
    expect_ok(interp.send(&s, Symbol::intern("run"), &[]), "run");
    let _ = interp.heap_ref().describe(&kept);
}
```

In `crates/ns-runtime/src/interpreter/safepoint.rs`, a `#[cfg(test)] mod
tests` (debug only, like what it tests):

```rust
#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn a_heap_within_the_budget_moves_at_every_safepoint() {
        let mut stress = StressMoves::default();
        for _ in 0..10 {
            assert!(stress.due(STRESS_MOVE_BUDGET));
        }
    }

    /// Above the budget, a move comes once the safepoints since the last one
    /// reach the traced size over the budget, rounded up -- 11 for a size of
    /// ten budgets and one -- so a move's cost, proportional to the size, is
    /// spread over as many safepoints.
    #[test]
    fn a_heap_past_the_budget_moves_once_per_its_size_over_the_budget() {
        let mut stress = StressMoves::default();
        let size = 10 * STRESS_MOVE_BUDGET + 1;
        let due: Vec<bool> = (0..22).map(|_| stress.due(size)).collect();
        assert_eq!(due.iter().filter(|due| **due).count(), 2);
        assert!(due[10] && due[21]);
    }
}
```

- [ ] **Step 2: Run them and see them fail to compile**

Run: `cargo test -p ns-runtime --lib --test stress`
Expected: compile errors — `set_stress_moves`, `StressMoves` and
`STRESS_MOVE_BUDGET` do not exist.

- [ ] **Step 3: The flag and the poll**

In `heap.rs`, add to `Heap` (all builds):

```rust
    /// Whether the run loop's next safepoint must take its slow path
    /// (`Interpreter::safepoint`). The poll is one compare of this flag
    /// (`docs/design/memory.md`, "Where a collection can happen"). Nothing
    /// sets it in a release build until the collector's allocation counter
    /// exists; the stress mode sets it for good.
    safepoint_requested: bool,
```

initialized to `false`, with:

```rust
    #[inline]
    pub(crate) fn safepoint_requested(&self) -> bool {
        self.safepoint_requested
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_safepoint_requested(&mut self, on: bool) {
        self.safepoint_requested = on;
    }

    /// How much a move has to rewrite: both arenas, the activation stack's
    /// entries and registers, and the host roots. The stress mode paces its
    /// moves by it.
    #[cfg(debug_assertions)]
    pub(crate) fn traced_size(&self) -> usize {
        self.objects.len()
            + self.frames.len()
            + self.stack.entries.len()
            + self.stack.values.len()
            + self.host_roots.borrow().slots.len()
    }
```

In `vm.rs`, `run`'s loop:

```rust
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
```

and, after `run`:

```rust
/// The safepoint, reached at every switch to another activation -- a push,
/// or a caller resumed -- and at every backward jump. Between two
/// instructions the loop holds the running `ActivationRef`, its code and its
/// `pc`, none of which a move changes, so a collection may run here and
/// nowhere else (`docs/design/memory.md`, "Where a collection can happen").
/// One compare of a flag the heap owns; what happens when it is set is
/// `interpreter/`'s.
#[inline(always)]
fn poll(interp: &mut Interpreter) {
    if interp.heap.safepoint_requested() {
        interp.safepoint();
    }
}
```

Extend `run`'s doc comment by one sentence naming the safepoints.

- [ ] **Step 4: The slow path and the schedule**

In `safepoint.rs` (adjust the module doc to say it holds the run loop's
safepoint and, in a debug build, the stress mode):

```rust
impl Interpreter {
    /// The run loop's safepoint, when the heap has asked for one. In a debug
    /// build under the stress mode, a move when the schedule says one is
    /// due. A release build has nothing that asks yet: the collector's
    /// allocation counter will.
    #[cold]
    #[inline(never)]
    pub(crate) fn safepoint(&mut self) {
        #[cfg(debug_assertions)]
        {
            let size = self.heap.traced_size();
            if self.stress.as_mut().is_some_and(|stress| stress.due(size)) {
                self.move_everything();
            }
        }
    }
}

/// How much move work the stress mode spends per safepoint, on average: it
/// moves at every safepoint while the traced size is at most this, and
/// otherwise once the safepoints since the last move reach the traced size
/// over this, rounded up. A move rewrites work proportional to the traced
/// size, so moving at literally every safepoint is quadratic in a deep
/// recursion; pacing by size is what a real collector's allocation-driven
/// trigger does, and every small program keeps a move at every safepoint.
/// Fixed-frequency stress modes (HotSpot's `ScavengeALotInterval`,
/// SpiderMonkey's `JS_GC_ZEAL` frequency) would take coverage from small
/// programs to spare large ones.
#[cfg(debug_assertions)]
pub(crate) const STRESS_MOVE_BUDGET: usize = 1_000;

/// The stress mode's schedule.
#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct StressMoves {
    safepoints_since_move: usize,
}

#[cfg(debug_assertions)]
impl StressMoves {
    /// Counts one safepoint and answers whether it moves.
    fn due(&mut self, traced_size: usize) -> bool {
        self.safepoints_since_move += 1;
        if self.safepoints_since_move.saturating_mul(STRESS_MOVE_BUDGET) < traced_size {
            return false;
        }
        self.safepoints_since_move = 0;
        true
    }

    /// Whether `NS_NEXT_STRESS_MOVES` asks for the stress mode: set, and
    /// neither empty nor `0`.
    pub(crate) fn from_env() -> Option<StressMoves> {
        let on = std::env::var_os("NS_NEXT_STRESS_MOVES").is_some_and(|v| !v.is_empty() && v != "0");
        on.then(StressMoves::default)
    }
}
```

and in the debug `impl Interpreter` block:

```rust
    /// Turns the stress mode on or off for this interpreter, whatever
    /// `NS_NEXT_STRESS_MOVES` says. For tests.
    pub fn set_stress_moves(&mut self, on: bool) {
        self.stress = on.then(StressMoves::default);
        self.heap.set_safepoint_requested(on);
    }
```

In `interpreter/mod.rs`, add to `Interpreter`:

```rust
    /// The stress mode's schedule, when it is on (`NS_NEXT_STRESS_MOVES`, or
    /// `set_stress_moves`). Debug builds only.
    #[cfg(debug_assertions)]
    stress: Option<safepoint::StressMoves>,
```

and in `bootstrap_with`, `#[cfg(debug_assertions)] stress:
safepoint::StressMoves::from_env(),` in the struct literal, and after
`build_builtin_classes`:

```rust
        #[cfg(debug_assertions)]
        {
            let on = interp.stress.is_some();
            interp.heap.set_safepoint_requested(on);
        }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

Then the stress runs this task can pass — the CLI, which Task 2 put on roots,
and the tests written for roots:

```bash
NS_NEXT_STRESS_MOVES=1 cargo test -p ns-cli
NS_NEXT_STRESS_MOVES=1 cargo test -p ns-runtime --test roots --test stress
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: PASS, and `run-corpus` reports `ran 6, failed 0, skipped 205`. The
first runs every fixture and every golden, under both lowerings, from source
and as a built binary, with everything moving: a failure there is a place the
tracer does not know about, or a place the CLI keeps a value across a run.
Fix the cause — a missing `Trace` field, a missing forwarding in
`Interpreter::move_everything`, a CLI value not rooted — never the test. The
rest of `ns-runtime`'s tests still use the unrooted entries and are expected
to fail under the variable until Tasks 5 and 6.

Also record how long `NS_NEXT_STRESS_MOVES=1 cargo test -p ns-cli` takes,
test binaries already built, beside the plain run's time; the report carries
both, and Task 8 puts them in execution.md.

- [ ] **Step 6: Measure**

Follow the measurement protocol. The poll is a load and a branch at every
switch and every backward jump, and the argument buffer is now cleared when
given back rather than when taken. If a source is slower than the parent, try
Cog's trick before anything else and report both: fold the request into the
depth check `Interpreter::push` already makes (memory.md, "Where a
collection can happen", last paragraph) — but only after reporting to the
controller.

- [ ] **Step 7: Gates and commit**

```bash
git add -A
git commit -m "feat: poll a safepoint in the run loop, and move there under stress" -m "The run loop polls one flag the heap owns at every switch to another
activation and every backward jump. Under NS_NEXT_STRESS_MOVES, a debug
build moves everything there, at every safepoint while the heap is small
and paced by its size above 1,000 traced entries.

<the measurement table, parent and d804eaf>

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 5: The runtime's tests on roots

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`mod tests`)
- Modify: `crates/ns-runtime/tests/builtin_object_inheritance.rs`,
  `builtins_boolean_numbers.rs`, `builtins_collections_block.rs`,
  `builtins_control_flow.rs`, `builtins_text.rs`, `compiler.rs`,
  `factory_send.rs`, `heap.rs`, `instantiate.rs`, `interpreter.rs`,
  `mixin.rs`, `nested_classes.rs`, `platform.rs`, `reference_lowering.rs`,
  `trampoline.rs` (all but its one `image::load` test, which is Task 6's),
  `vm.rs`, `vm_initializer.rs`
- Modify: `crates/ns-cli/tests/disasm_snapshots.rs` (its one direct
  `load_module` call; it runs no code, so it fetches the class at once)

**Interfaces:**
- Consumes: Task 2's rooted API; Task 4's stress mode, which checks the
  conversion.
- Produces: no test in these files calls `send_unrooted`,
  `instantiate_unrooted` or `load_module_unrooted`.

This task changes how the tests talk to the interpreter, not what they check.
Every assertion stays; no test is deleted, weakened or given a new expected
value. If a test cannot keep its assertion under the rooted API, stop and
report it.

- [ ] **Step 1: The helpers**

In each file, replace the local helpers with these (keep a file's own extra
helpers, converted the same way):

```rust
use ns_runtime::{HostError, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn load(src: &str) -> (Interpreter, Root) {
    // (the file's existing parse and resolve lines, unchanged)
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

fn expect_error(interp: &Interpreter, result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(e)) => e,
        Ok(root) => panic!(
            "{what}: expected a Newspeak error, got {}",
            interp.heap_ref().describe(&interp.fetch(&root))
        ),
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
    }
}

/// The object `root` holds, for the heap accessors that take one.
fn obj(interp: &Interpreter, root: &Root) -> ns_runtime::ObjRef {
    match interp.fetch(root) {
        Value::Obj(obj) => obj,
        _ => panic!("expected an object"),
    }
}
```

(Only the helpers a file uses; clippy refuses dead ones. A file whose helpers
differ in shape — a `bootstrap_with(Lowering::Reference)`, a `run(src, sel)`
that loads, instantiates and sends — keeps its shape and changes its types.)

- [ ] **Step 2: The call sites**

The rules, applied to every test:

1. Everything handed to `send` or `instantiate` is a `&Root` or a `&[Root]`.
   A value built by hand is stashed where it is passed — `stash` takes
   `&self`, so `interp.send(&r, sel, &[interp.stash(Value::SmallInt(3))])`
   compiles. The platform is `let platform = interp.platform();` first,
   since `platform` takes `&mut self`.
2. A result is read with `interp.fetch(&root)` at the point it is inspected,
   and a fetched `Value` or `ObjRef` is never kept across a `send`, an
   `instantiate` or a `move_now`: fetch again after one.
3. `expect_error(&interp, result, ...)` takes the interpreter by shared
   reference, so bind the result first: `let result = interp.send(...);`.
4. `Signal`, `EvalResult` and `NewspeakError` disappear from these files;
   `HostSignal`, `HostResult` and `HostError` take their place. A test that
   matched `Signal::Return(..)` matches `HostSignal::Return`.

For example, `builtins_collections_block.rs`'s first test becomes:

```rust
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let result = expect_ok(interp.send(&receiver, Symbol::intern("m"), &[]), "{1. 2. 3} at: 2");
    match interp.fetch(&result) {
        Value::SmallInt(2) => {}
        other => panic!(
            "expected Value::SmallInt(2), got {}",
            interp.heap_ref().describe(&other)
        ),
    }
```

and its `new_ordered_collection` helper answers a root:

```rust
fn new_ordered_collection(interp: &mut Interpreter) -> Root {
    let class = interp.builtin_class(BuiltinClass::OrderedCollection);
    let collection = interp
        .heap_mut()
        .alloc_object(class, ObjectData::Array(vec![]));
    interp.stash(Value::Obj(collection))
}
```

The unit tests in `interpreter/mod.rs` call `decide_send` with plain values,
which is right — deciding runs nothing — but fetch them from their roots after
the last run before the decision, into locals:

```rust
        let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
        let receiver = interp.fetch(&instance);
        match interp.decide_send(receiver, Symbol::intern("foo"), &[]) {
```

A test that decides several sends after several runs fetches every value it
decides with after the last of those runs.

- [ ] **Step 3: Each file under stress**

For every converted file:

```bash
cargo test -p ns-runtime --test <file-stem>
NS_NEXT_STRESS_MOVES=1 cargo test -p ns-runtime --test <file-stem>
```

and for the unit tests `NS_NEXT_STRESS_MOVES=1 cargo test -p ns-runtime --lib`.
Expected: PASS both ways. A "stale handle" panic under the variable is a
value the test keeps across a run: root it or fetch it again. A different
failure under the variable only is a tracer defect: stop and report it.
`trampoline.rs`'s `image::load` test still uses the unrooted entries and is
expected to fail under the variable until Task 6: run the file with
`-- --skip <that test's name>` under the variable.

- [ ] **Step 4: Gates and commit**

```bash
git add -A
git commit -m "test: talk to the interpreter through roots in the runtime's tests" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

If the conversion is too large for one pass, commit it in two, split by file,
each passing the gates and its files' stress runs.

---

### Task 6: `image::load` on roots, and the image tests

**Files:**
- Modify: `crates/ns-runtime/src/image/mod.rs` (`LoadedImage`, `load`)
- Modify: `crates/ns-cli/src/commands/load.rs`, `disasm.rs`
- Modify: `crates/ns-runtime/tests/image.rs`, and the `image::load` test in
  `crates/ns-runtime/tests/trampoline.rs`

**Interfaces:**
- Produces: `LoadedImage { interp, root_class: Root, roots: Vec<Root>, origin,
  source, entry, stateless }`.

- [ ] **Step 1: The loader**

In `crates/ns-runtime/src/image/mod.rs`, `LoadedImage`:

```rust
    /// The module's root class, rooted for the host that will instantiate
    /// it or snapshot it again.
    pub root_class: Root,
    /// The image's roots, rooted: a host sends to them, which runs code.
    pub roots: Vec<Root>,
```

and at the end of `load`:

```rust
    let root_class = interp.stash(Value::Obj(root_class));
    let roots = graph.roots.into_iter().map(|root| interp.stash(root)).collect();
    Ok(LoadedImage {
        interp,
        root_class,
        roots,
        origin,
        source,
        entry,
        stateless: header.stateless,
    })
```

`crates/ns-cli/src/commands/load.rs` drops the stashing Task 2 added: it uses
`root_class` and `roots` as they come. `disasm.rs`'s image path reads
`entry::class_ref(&loaded.interp, &loaded.root_class)`.

- [ ] **Step 2: The tests**

Convert `tests/image.rs` and the `image::load` test in `tests/trampoline.rs`
by Task 5's helpers and rules. What is particular to `image.rs`:

- Tests that drive the encoder and decoder directly (`GraphTable::build`,
  `encode_graph_section`, `decode_graph_section`, `GraphFixture`) take plain
  values and handles, and that stays: they run no code between reading a
  value and using it. Fetch those values from roots immediately before the
  call, after the last run.
- A helper that runs code and then encodes — `round_trip_graph`'s closure,
  `snapshot_of_without_source` — keeps the class and every value it will
  encode as roots across the runs, and fetches them right before encoding.
  `image::snapshot` takes `entry::class_ref`-style plain values fetched just
  before, like the CLI's `snapshot` command.
- Tests that send to a loaded image's roots use `loaded.roots` and
  `loaded.root_class` as roots.

Then:

```bash
cargo test -p ns-runtime --test image --test trampoline
NS_NEXT_STRESS_MOVES=1 cargo test -p ns-runtime --test image --test trampoline
NS_NEXT_STRESS_MOVES=1 cargo test -p ns-cli
```

Expected: PASS both ways. `the_wire_format_matches_the_golden_fixture` must
pass unchanged: rooting changes nothing the encoder writes.

- [ ] **Step 3: Gates and commit**

```bash
git add -A
git commit -m "feat: answer a loaded image's roots as roots" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 7: Only roots leave the crate

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`, `instantiate.rs`
  (remove `send_unrooted`, `instantiate_unrooted`, `load_module_unrooted`;
  `platform_value`, `Signal`, `EvalResult` → `pub(crate)`)
- Modify: `crates/ns-runtime/src/lib.rs` (drop `Signal` from the re-export)
- Modify: `crates/ns-runtime/src/vm.rs` and any other item the compiler or
  clippy flags for exposing a crate-private type
- Modify: `CLAUDE.md`

- [ ] **Step 1: Remove the transitional entries**

Delete `send_unrooted`, `instantiate_unrooted` and `load_module_unrooted`;
fold `load_module_unrooted`'s body into `load_module`. Make `platform_value`
`pub(crate)`. Make `Signal` and `EvalResult` `pub(crate)` and drop `Signal`
from `lib.rs`'s re-export. Run `cargo clippy --workspace --all-targets`: every
public function that now exposes `Signal` or `EvalResult` (`vm::run`, at
least) becomes `pub(crate)`. Nothing outside `ns-runtime` may need them;
if something does, it is an unconverted site — convert it, never widen the
type again.

- [ ] **Step 2: The whole suite under stress**

```bash
cargo test --workspace
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

Expected: PASS, and `run-corpus` reports `ran 6, failed 0, skipped 205`.
Record the wall time of both `cargo test --workspace` runs, test binaries
already built; Task 8 puts them in execution.md beside "12.5 s for 662
tests".

- [ ] **Step 3: `CLAUDE.md`**

In "Working in this repository", after the `cargo test --workspace` line:

```bash
# the same suite with every object moved at the run loop's safepoints
# (debug builds only; Git Bash syntax)
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
```

and after the `run-corpus` line:

```bash
# non-regression gate under the stress mode, same report
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
```

In the invariant "A handle means nothing against a `Heap` that did not mint
it", add a paragraph: a handle also means nothing across a move. A host keeps
a value across a run only as a `Root` (`Interpreter::stash`/`fetch`, and every
host entry takes and answers them); a value fetched from one is good until
the next run. In a debug build every handle carries its move epoch and every
accessor checks it, and `NS_NEXT_STRESS_MOVES=1` moves everything at the run
loop's safepoints, so the stress runs above catch a value kept across a run
and a handle the tracer (`trace.rs`) does not know about. Add a field that
holds a handle to any runtime type, and its `Trace` impl stops compiling until
it says what to do with it.

In the invariant "Never `{:?}` a `Value`, and never `.unwrap()` / `.expect()`
/ `{:?}` an `EvalResult`", say the same holds for a `HostResult`:
`HostSignal`, `HostError` and `Root` have no `Debug`, so `unwrap`/`expect` on
one does not compile, and tests unwrap through a local `expect_ok`.

- [ ] **Step 4: Gates and commit**

```bash
git add -A
git commit -m "refactor: let nothing but roots leave the runtime" -m "The unrooted host entries are gone and Signal is crate-private: a host
can hold a value across a run only as a Root. The whole suite and
run-corpus pass with everything moving at every safepoint the budget
allows, and CLAUDE.md makes both a gate.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```

---

### Task 8: Measure the whole step, and write it down

**Files:**
- Modify: `docs/design/memory.md`, `docs/design/execution.md`,
  `docs/design/image-format.md`, `docs/design/README.md`, `docs/PROGRESS.md`

- [ ] **Step 1: Measure the whole step**

Follow the measurement protocol with `<parent-commit>` = `4df2561`, the commit
2b started from, in two sittings. Record the table in the same shape as
execution.md's "What removing the re-entries moved".

- [ ] **Step 2: memory.md**

Rewrite "Planned: checked roots and a collector" so that what 2b built is in
the present tense and what 2c and step 5 will build is still marked planned
(docs describe the current state, not its history):

- The status block at the top: the safepoint, the tracing, the host roots
  and the stress mode are built (step 2b); the brand is not.
- "Where a collection can happen": the poll exists, where it is
  (`vm::poll`), what its slow path is, and what it measured (Step 1's
  figures, and Task 4's).
- "Host roots": `Root`, `stash`/`fetch` on `&self` and why, the sweep, the
  rooted host API, `HostSignal`/`HostError`; that `heap_ref()`/`heap_mut()`
  and plain values from `fetch` remain until 2c's `enter`.
- "What the collector traces": the list as `trace.rs` enumerates it, the
  rule that every impl names every field, `StackEntry`'s impl in `heap.rs`,
  the interpreter's own roots, and the closure split (Task 1) as how the
  "never behind a shared `Rc`" rule holds.
- "Checking the roots before a collector exists: the stress mode": the
  rotation with no fixed point, the epoch, the `Forwarder`'s own check of
  what it forwards, `NS_NEXT_STRESS_MOVES`, and the budgeted schedule —
  `STRESS_MOVE_BUDGET`, why literal every-safepoint was quadratic, the prior
  art's fixed frequencies and why size-proportional pacing was chosen
  instead (decided with the user on 2026-09-13), and the suite's time with
  and without it (Tasks 4 and 7). The gates are now in `CLAUDE.md`.
- "What this leaves for the collector itself": the tracing and forwarding
  exist in a debug build; the collector un-gates them and adds liveness,
  reclamation, the allocation counter and the spaces.
- "Decisions": add the budgeted stress schedule and the inline closure
  payload.

- [ ] **Step 3: execution.md, image-format.md, README.md**

- execution.md, "The VM": the safepoint poll, where it is and why the loop's
  locals hold no movable handle; "Measurements": a new "What 2b moved"
  subsection with Task 1's, Task 4's and the whole-step tables (both
  baselines), and the suite times.
- image-format.md: check that Task 1's changes still read right after the
  other tasks.
- README.md's map: memory.md's row — "Partial — the arena, the activation
  stack, the safepoint, tracing, host roots and the stress mode are built;
  nothing reclaims an arena slot" — and "Planned next": the lifetime brand
  (2c), a collector.

- [ ] **Step 4: PROGRESS.md**

- The M4 row lists this plan's link after `no-reentry`.
- "Where the next session starts": 2b is done; the next piece is 2c, the
  lifetime brand, to be planned against the code as 2b left it. Carry the
  open findings forward: the d804eaf comparison with its current figures
  (from Step 1), the two 2a findings still open, and anything this plan
  surfaced.
- Step 2's list entry: 2b done, with a one-sentence summary and its figures;
  2c "Next."
- A new "M4 — The safepoint, tracing, host roots and the stress mode"
  chronology section after "M4 — No re-entry from Rust", in the style of the
  ones before it: what was built, what it measured, the suite's time under
  the stress mode, what reviews found.

- [ ] **Step 5: Check the documents support a fresh session**

Read `CLAUDE.md`, `docs/design/README.md` and PROGRESS.md's "Where the next
session starts" as a session that knows nothing of this plan would: every
command they name must run, every file and function they name must exist,
and the next step must be clear from them alone.

- [ ] **Step 6: Gates and commit**

Run every gate, the two stress gates included. Then:

```bash
git add -A
git commit -m "docs: describe step 2b as built, with its measurement" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01D1Dip7oZ7mL5jZG3SU4Woj"
```
