# No Re-entry from Rust (2a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rust enters the run loop only as its outermost caller. `do:`,
instantiation and the nested-class accessor stop re-entering it and become
trampoline steps, so that no Rust frame is live below a safepoint — the ground
the root discipline (steps 2b and 2c) stands on.

**Architecture:** All three follow the pattern `whileTrue:` already set: a
decision answers an activation together with a `Continuation`, the
continuation rides on the activation's stack entry, and
`Interpreter::resume` applies it when the activation returns. `do:` answers
its block for the first element with `Continuation::DoElement`; the
nested-class accessor answers its superclass clause with
`Continuation::NestedClass`; instantiation becomes a state machine behind
`Continuation::Instantiate`. The clause and initializer bodies, which are
neither methods nor blocks, are pushed through a new `Step::Body`. With
nothing left re-entering the loop, `reenter`, `invoke_closure`,
`builtins::call_block`, `native_depth` and `MAX_NATIVE_DEPTH` go, and every
recursion ends at the one activation limit.

**Tech Stack:** Rust 1.98.0, edition 2021, no external dependencies. Only
`ns-runtime` and `ns-cli` change; nothing under `crates/ns-syntax`.

**Spec:** [docs/design/execution.md](../../design/execution.md), "Planned: no
re-entry from Rust" — all of it — and "What stays recursive";
[docs/design/memory.md](../../design/memory.md), "Planned: checked roots and a
collector", for why (above all "No Rust frame is live below a safepoint" and
"What the collector traces", which asks for `nested_in_progress` to hold an
`ObjRef`); [docs/design/core-library.md](../../design/core-library.md), "Planned:
the leaf/callback split", for `do:`. Read them before Task 1.

## Decisions this plan carries

Settled with the user on 2026-09-13 and recorded in the design:

1. **The root discipline removes the re-entries rather than rooting them.**
   Step 2 is split into 2a (this plan), 2b (safepoint, tracing, host roots,
   stress mode) and 2c (the lifetime brand); 2b and 2c get their own plans
   once this one has merged.
2. **`do:` becomes a continuation step now**, like `whileTrue:`; it moves to
   Newspeak only after the inline caches, as core-library.md already says.
3. **N goes.** The three goldens that end in `stack overflow: too many nested
   native calls` — `do-recursion`, `instantiation-recursion`,
   `accessor-recursion` — end in `stack overflow: recursion too deep` instead.
   That is right because the limit the old text reported no longer exists; it
   is not recorded because the code now produces it.
4. **`INTERPRETER_STACK_BYTES` is re-derived** from what still recurses on the
   Rust stack: the parser, the resolver and the compiler, bounded by the
   parser's nesting ceiling. The thread stays, because the symbol interner is
   thread-local.

## Scope

In: the three trampoline steps, `Step::Body`, `Interpreter::abandon`,
`nested_in_progress` as `ObjRef`s, the host entry that replaces `reenter`, the
three golden changes, the interpreter thread's stack, the measurements and the
documents.

Deliberately **not** in this plan:

- The safepoint, the tracing, host roots, the stress mode, splitting
  `Rc<ClosureData>` (2b); the lifetime brand and `Interpreter::enter` (2c).
- Moving `do:` or anything else into Newspeak.
- Catchable exceptions; closing the "`^` into a running initializer escapes"
  gap (execution.md, "Non-local return") — its behaviour is unchanged here.
- `StepArgs` gaining a one-value variant for `do:`'s per-element argument
  vector: `do:` allocates one per element today through `call_block`, and
  keeps doing so; a new `StepArgs` variant changes `Step`'s layout, which the
  trampoline measured as a 4–8% risk.

## Global Constraints

- **`vm.rs` holds no dispatch logic.** Deciding what a continuation does —
  `resume` — and what an abandoned one undoes — `abandon` — live in
  `interpreter/`; the VM only forwards a popped entry's continuation to them.
- **Program outcomes are pinned, not re-derived.** Every
  `tests-fixtures/*.ns`, every golden in
  `crates/ns-cli/tests/program_outcomes.rs` and every `run-corpus` row keeps
  its recorded answer under both lowerings, **except** the three goldens named
  in Decision 3, each changed in the task that removes its re-entry, with the
  reason in its doc comment.
- **An image is untrusted input.** Nothing added may panic on bytes an
  artifact controls. The `expect`s and `panic!`s this plan adds are internal
  invariants no byte sequence reaches (each says why in its message), or are
  moved verbatim from code that already has them.
- **A running activation is an `ActivationRef`; only a `FrameRef` is ever
  stored.** `Step`, `BodyStep`, `Continuation` and every continuation state
  hold values, classes, closures and object handles, never an `ActivationRef`.
- **Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an
  `EvalResult`.** Tests unwrap through a local `expect_ok`.
- **Object, frame and activation access goes through `heap.rs`.**
- **A Newspeak module written inline in a test is an `indoc! {"…"}`**, its
  lines indented with spaces, the closing `"}` on a line of its own.
- **Comments explain the decision, not the mechanics.**
- **`Step` does not grow.** `Step::Body` carries a `Box<BodyStep>`, and every
  new `Continuation` variant a `Box`, so that neither `Result<Step, Signal>`
  nor a stack entry changes size (the trampoline's field-niche measurement,
  execution.md, "What the trampoline moved").
- **Performance is measured, not assumed**, with the protocol below, in every
  task that changes a hot path (Tasks 1, 2, 3). A task that makes a benchmark
  slower beyond noise is not done; report the finding to the controller before
  anything else happens.
- Gates before every commit: `cargo fmt --all`; `cargo clippy --workspace
  --all-targets` warning-free; `cargo test --workspace`;
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`;
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`.
- Work on the checked-out branch `feature/root-discipline-2a`; no worktree
  except the throwaway one the measurement protocol builds.
- Conventional commits, each ending with (subagents included):
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01BAJjLibM3jQ92zbXMEPtYf
  ```

## Measurement protocol

Used by every task that says "measure". Before and after are compared **in one
sitting**; a figure from another session is never compared with one from this.
Besides the three benchmark fixtures, two throwaway sources exercise the paths
this plan changes and no fixture does. Write them under `target\bench-2a\`
(ignored by git) the first time they are needed; they are never committed.

`target\bench-2a\bench_do.ns` — 50,000 block activations through `do:`:

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

`target\bench-2a\bench_new.ns` — 20,000 instantiations that run an initializer:

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

Check each prints what it should before timing it: `275000` and `20000`
(`cargo run -p ns-cli -- run target\bench-2a\bench_do.ns`).

```powershell
# the "before" binary, from the commit the task started from
git worktree add ..\ns-next-before <parent-commit>
cargo build --release -p ns-cli --manifest-path ..\ns-next-before\Cargo.toml
cargo build --release -p ns-cli
$before = "..\ns-next-before\target\release\ns-next.exe"
$after  = ".\target\release\ns-next.exe"
$sources = "tests-fixtures\bench_loop.ns", "tests-fixtures\bench_send.ns",
           "tests-fixtures\bench_block.ns", "target\bench-2a\bench_do.ns",
           "target\bench-2a\bench_new.ns"
foreach ($round in 1..3) {
  foreach ($src in $sources) {
    foreach ($bin in $before, $after) {
      & $bin bench $src   # prints median, min, max
    }
  }
}
git worktree remove ..\ns-next-before
```

Record the three medians per source and binary. A change is "no slower" when,
for every source, the after-group does not lie above the before-group by more
than either group's own spread; "faster" when the after-group lies entirely
below. Report the table in the task's report to the controller; Task 5 carries
the whole-step table into the documents.

---

### Task 1: `do:` as a trampoline step

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`Continuation`, new `DoLoop`)
- Modify: `crates/ns-runtime/src/builtins/collections.rs` (whole file)
- Modify: `crates/ns-runtime/src/builtins/mod.rs` (`dispatch_native`, remove
  `value_step` and `call_block`)
- Modify: `crates/ns-runtime/src/builtins/boolean.rs` (`block_step`'s match)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`resume`, remove
  `invoke_closure`, `reenter`'s doc comment, a unit test)
- Test: `crates/ns-runtime/tests/builtins_collections_block.rs`,
  `crates/ns-runtime/tests/trampoline.rs`,
  `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:**
- Produces: `pub struct DoLoop { pub(crate) receiver: Value, pub(crate) block:
  Rc<ClosureData>, pub(crate) items: Vec<Value>, pub(crate) next: usize }`
  (`types.rs`); `Continuation::DoElement(Box<DoLoop>)`;
  `pub(crate) fn collections::do_step(interp: &Interpreter, receiver: Value,
  selector: Symbol, args: &[Value]) -> Option<Result<Step, Signal>>`;
  `pub(crate) fn collections::after_element(state: Box<DoLoop>) -> Step`.
- After this task `collections::dispatch_array` and
  `collections::dispatch_ordered_collection` return
  `Option<Result<Value, NewspeakError>>`, like every other leaf dispatcher.

- [ ] **Step 1: Write the failing unit test**

In `crates/ns-runtime/src/interpreter/mod.rs`, in `mod tests` after
`deciding_a_send_answers_what_to_run_without_running_it`:

```rust
    /// `do:` is decided as the activation of its block for the first
    /// element, with the rest of the loop in the continuation: nothing runs,
    /// and nothing is pushed (`docs/design/execution.md`, "No re-entry from
    /// Rust").
    #[test]
    fn deciding_do_answers_its_block_for_the_first_element() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public items = ( ^{10. 20. 30} )
              public echo = ( ^[:e | e] )
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        let items = expect_ok(interp.send(instance, Symbol::intern("items"), vec![]), "items");
        let echo = expect_ok(interp.send(instance, Symbol::intern("echo"), vec![]), "echo");
        match interp.decide_send(items, Symbol::intern("do:"), &[echo]) {
            Ok(Step::Block {
                args: StepArgs::Owned(args),
                then: Continuation::DoElement(state),
                ..
            }) => {
                assert!(matches!(args.as_slice(), [Value::SmallInt(10)]), "the first element");
                assert_eq!(state.next, 1, "the second element is next");
                assert_eq!(state.items.len(), 3, "the whole snapshot rides along");
            }
            _ => panic!("`do:` is decided as its block's first activation"),
        }
        assert_eq!(interp.heap.activation_depth(), 0, "nothing was pushed");
    }
```

- [ ] **Step 2: Write the behaviour guards and the deep-recursion test**

These pin what `do:` does today; they pass before the change and must still
pass after it. Append to `crates/ns-runtime/tests/builtins_collections_block.rs`:

```rust
/// `do:` walks the elements its receiver had when it began: a block that adds
/// to the collection it is walking neither sees what it adds nor runs
/// forever. Two elements seen, four in the collection afterwards.
#[test]
fn do_walks_a_snapshot_of_its_receiver() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m: aColl = (
            | seen |
            seen := 0.
            aColl add: 1; add: 2.
            aColl do: [:x | seen := seen + 1. aColl add: x].
            ^seen * 100 + aColl size
          )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let oc = new_ordered_collection(&mut interp);
    let result = interp.send(receiver, Symbol::intern("m:"), vec![oc]);
    match expect_ok(result, "do: over a collection it grows") {
        Value::SmallInt(204) => {}
        other => panic!(
            "expected 204, got {}",
            interp.heap_ref().describe(&other)
        ),
    }
}

/// A non-block argument stands for its own value, so `do:` hands it every
/// element and observes nothing; like any `do:`, it answers its receiver.
#[test]
fn do_with_a_non_block_answers_its_receiver() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public m = ( | a | a := {1. 2}. ^(a do: 3) == a )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let result = interp.send(receiver, Symbol::intern("m"), vec![]);
    assert!(matches!(
        expect_ok(result, "{1. 2} do: 3"),
        Value::Bool(true)
    ));
}

/// A block of the wrong arity fails at the first element; an empty
/// collection hands it nothing, so there it does not fail at all.
#[test]
fn do_checks_its_block_arity_at_each_element() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Foo = Object new (
        ) (
          public empty = ( ^#() do: [:a :b | a] )
          public full = ( ^#(1) do: [:a :b | a] )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let receiver = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    expect_ok(
        interp.send(receiver, Symbol::intern("empty"), vec![]),
        "#() do: with a two-argument block",
    );
    let result = interp.send(receiver, Symbol::intern("full"), vec![]);
    let err = expect_error(interp.heap_ref(), result, "#(1) do: with a two-argument block");
    assert!(
        err.message.contains("wrong number of block arguments"),
        "got: {}",
        err.message
    );
}
```

Also in that file, change the doc comment of
`array_do_sums_elements_via_a_mutable_slot_accumulator` from "Also exercises
`call_block` invoking a real one-argument closure with a real argument." to
"Also exercises `do:` activating a real one-argument closure with a real
argument, once per element."

Append to `crates/ns-runtime/tests/trampoline.rs`:

```rust
/// `do:` is a trampoline step, not a callback: a recursion through it is
/// activations on the stack and nothing on the Rust stack, so twenty thousand
/// levels -- far past what a test thread's stack holds when each level
/// re-enters the loop from Rust -- run under either lowering. Each level
/// counts two activations, the method and the block.
#[test]
fn a_deep_recursion_through_do_runs_on_the_activation_stack() {
    let src = indoc! {"
        Newspeak3
        'T'
        class DoDeep = Object new (
        ) (
            public d: n = ( n = 0 ifTrue: [^0]. #(1) do: [:e | ^d: n - 1]. ^1 )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(src, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        let result = expect_ok(
            interp.send(instance, Symbol::intern("d:"), vec![Value::SmallInt(20_000)]),
            "d:",
        );
        assert_eq!(small_int(result), 0, "{lowering:?}");
        assert_eq!(interp.heap_ref().activation_depth(), 0, "{lowering:?}");
        assert_eq!(interp.heap_ref().counted_depth(), 0, "{lowering:?}");
    }
}
```

- [ ] **Step 3: Change the `do-recursion` golden**

In `crates/ns-cli/tests/program_outcomes.rs`, replace the test
`a_recursion_through_do_is_bounded_by_the_native_limit` and its doc comment
with:

```rust
/// `do:` is a trampoline step, so a recursion through it is made only of
/// counted activations -- the method and the block, at every level -- and
/// ends at the activation limit like any other recursion. It used to end at a
/// second limit, on re-entries of the run loop from Rust, which no longer
/// exists (`docs/design/execution.md`, "No re-entry from Rust").
#[test]
fn a_recursion_through_do_is_bounded_by_the_activation_limit() {
    assert_outcome(
        "do-recursion",
        indoc! {"
            Newspeak3
            'T'
            class DoDeep usingPlatform: platform = Object new (
            ) (
                d: n = ( #(1) do: [:e | d: n + 1] )
                public main: p args: a = ( d: 0 )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -p ns-runtime --lib deciding_do_answers`
Expected: compile error, `no variant ... DoElement` — the continuation does not
exist yet.

Run: `cargo test -p ns-runtime --test builtins_collections_block`
Expected: PASS — the guards describe today's behaviour.

Run: `cargo test -p ns-cli --test program_outcomes a_recursion_through_do`
Expected: FAIL — stderr is `... too many nested native calls`.

Do not run the new `trampoline.rs` test yet: before the change it recurses
through `reenter` and overflows the test thread's 2 MiB stack, aborting the
whole test binary rather than failing.

- [ ] **Step 5: Add the continuation and its state**

In `crates/ns-runtime/src/types.rs`, change `Continuation`'s doc comment's
second sentence to "Every activation a send makes hands its value to that
send; the others are how a builtin native finishes its work once the block it
activated has returned, with no Rust frame waiting for it." and add after
`WhileBody(Rc<WhileLoop>),`:

```rust
    /// A native `do:` loop's block returned for one element: activate it for
    /// the next, or answer the receiver
    /// (`builtins::collections::after_element`).
    DoElement(Box<DoLoop>),
```

After `WhileLoop`, add:

```rust
/// A native `do:` loop in progress: the collection it answers when it is done,
/// its block, the elements the collection had when the loop began -- `do:`
/// walks that snapshot, so a block that changes the collection does not
/// change the walk -- and the index of the next element to hand the block.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct DoLoop {
    pub(crate) receiver: Value,
    pub(crate) block: Rc<ClosureData>,
    pub(crate) items: Vec<Value>,
    pub(crate) next: usize,
}
```

- [ ] **Step 6: Rewrite `collections.rs`**

Replace everything from `use crate::interpreter::{EvalResult, Signal};` to the
end of `crates/ns-runtime/src/builtins/collections.rs` (the module doc comment
above it stays) with:

```rust
use crate::interpreter::{Signal, Step, StepArgs};
use crate::types::*;
use crate::{Heap, Interpreter, Symbol};

fn out_of_range(receiver: &Value, selector: Symbol, index: i64, len: usize) -> NewspeakError {
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!("index {index} out of range: valid indices are 1..{len} (size {len})"),
    }
}

fn not_an_integer(
    heap: &Heap,
    receiver: &Value,
    selector: Symbol,
    got: Option<&Value>,
) -> NewspeakError {
    NewspeakError {
        selector,
        receiver: *receiver,
        message: format!(
            "{selector} requires an Integer index, got {}",
            heap.describe_arg(got)
        ),
    }
}

/// `size` and `at:` -- shared by `Array` and `OrderedCollection` since both
/// share the `ObjectData::Array` payload (see this module's design note).
/// Returns `None` if `receiver` isn't `ObjectData::Array`-backed at all, or
/// if `selector` isn't one of these two (letting the caller fall through to
/// `doesNotUnderstand:`, or to the class-specific selector handled by
/// `dispatch_array`/`dispatch_ordered_collection` itself). `do:` is not a
/// leaf and is not here: it activates a block, so it answers a step
/// (`do_step`).
fn dispatch_common(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    match selector.as_str() {
        "size" => interp
            .heap
            .array_len(*obj)
            .map(|len| Ok(Value::SmallInt(len as i64))),
        "at:" => {
            let len = interp.heap.array_len(*obj)?;
            Some(match args.first() {
                Some(Value::SmallInt(index)) => {
                    if *index >= 1 && (*index as usize) <= len {
                        Ok(interp
                            .heap
                            .array_at(*obj, (*index as usize) - 1)
                            .unwrap_or(Value::Nil))
                    } else {
                        Err(out_of_range(&receiver, selector, *index, len))
                    }
                }
                other => Err(not_an_integer(&interp.heap, &receiver, selector, other)),
            })
        }
        _ => None,
    }
}

pub(crate) fn dispatch_array(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    if selector.as_str() == "at:put:" {
        let Value::Obj(obj) = &receiver else {
            return None;
        };
        let len = interp.heap.array_len(*obj)?;
        let index = match args.first() {
            Some(Value::SmallInt(index)) => *index,
            other => {
                return Some(Err(not_an_integer(
                    &interp.heap,
                    &receiver,
                    selector,
                    other,
                )))
            }
        };
        if index < 1 || (index as usize) > len {
            return Some(Err(out_of_range(&receiver, selector, index, len)));
        }
        let value = args.get(1).copied().unwrap_or(Value::Nil);
        interp.heap.array_set(*obj, (index as usize) - 1, value);
        return Some(Ok(receiver));
    }
    dispatch_common(interp, receiver, selector, args)
}

pub(crate) fn dispatch_ordered_collection(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    if selector.as_str() == "add:" {
        let Value::Obj(obj) = &receiver else {
            return None;
        };
        interp.heap.array_len(*obj)?;
        // Matches the reference library's convention: `add:` returns the
        // argument, not the receiver.
        let value = args.first().copied().unwrap_or(Value::Nil);
        interp.heap.array_push(*obj, value);
        return Some(Ok(value));
    }
    dispatch_common(interp, receiver, selector, args)
}

/// `do:` on an `Array` or an `OrderedCollection`: the block activated for each
/// element in turn, as trampoline steps -- the first here, each next one by
/// `after_element` when the one before it returns -- so no Rust frame waits
/// on Newspeak code (`docs/design/execution.md`, "No re-entry from Rust").
/// `None` when `receiver` has no `ObjectData::Array` payload, so the send
/// falls through to `doesNotUnderstand:` as every other selector here does.
pub(crate) fn do_step(
    interp: &Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let items = interp.heap.array_elements(*obj)?;
    let Some(block) = args.first().copied() else {
        return Some(Err(Signal::Error(NewspeakError {
            selector,
            receiver,
            message: "do: requires a block argument".to_string(),
        })));
    };
    // A non-block stands for its own value, so handing it the elements
    // observes nothing: the loop is over before it starts.
    let closure = match block {
        Value::Obj(block) => interp.heap.closure_data(block),
        _ => None,
    };
    let Some(block) = closure else {
        return Some(Ok(Step::Value(receiver)));
    };
    Some(Ok(element_step(Box::new(DoLoop {
        receiver,
        block,
        items,
        next: 0,
    }))))
}

/// The block returned for one element. Its value is dropped, as `do:` always
/// dropped it, and the next element's activation takes its place on the
/// stack.
pub(crate) fn after_element(state: Box<DoLoop>) -> Step {
    element_step(state)
}

/// The block's activation for the element at `state.next`, or the receiver
/// once there is none. Each activation is counted, like any block a `value`
/// send activates: its arguments are `StepArgs::Owned`, never the
/// `ControlFlow` mark.
fn element_step(mut state: Box<DoLoop>) -> Step {
    let Some(&item) = state.items.get(state.next) else {
        return Step::Value(state.receiver);
    };
    state.next += 1;
    Step::Block {
        closure: state.block.clone(),
        args: StepArgs::Owned(vec![item]),
        then: Continuation::DoElement(state),
    }
}
```

- [ ] **Step 7: Route `do:` in `dispatch_native`; remove the callback**

In `crates/ns-runtime/src/builtins/mod.rs`:

Replace `use crate::interpreter::{EvalResult, Signal, Step};` with
`use crate::interpreter::{Signal, Step};`.

Replace the doc comment of `dispatch_native` (from "`None` = this builtin
does not implement" through "by `value_step`/`wrap_error`.") with:

```rust
/// `None` = this builtin does not implement `selector` (let the caller fall
/// through to `doesNotUnderstand:`). `Some(Err(Signal::Error(_)))` = a
/// genuine Newspeak-level error (e.g. arithmetic overflow).
///
/// No native calls a block. `boolean::dispatch`, `block::dispatch` and
/// `collections::do_step` answer a `Step` themselves: the control-flow
/// natives, `value` and `do:` return the block to activate instead of
/// calling it (`docs/design/execution.md`, "The trampoline"), so a `^` out of
/// that block unwinds the activation stack like any other and never passes
/// through a native. Every other native is a leaf: it keeps a
/// `Result<Value, NewspeakError>` signature, wrapped into the step it is right
/// here, by `wrap_error`.
```

Replace the two collection arms of the `match kind` with:

```rust
        // `do:` activates its block, so it answers a step of its own; every
        // other `Array` and `OrderedCollection` method is a leaf.
        BuiltinClass::Array | BuiltinClass::OrderedCollection if selector.as_str() == "do:" => {
            collections::do_step(interp, receiver, selector, args)
        }
        BuiltinClass::Array => {
            collections::dispatch_array(interp, receiver, selector, args).map(wrap_error)
        }
        BuiltinClass::OrderedCollection => {
            collections::dispatch_ordered_collection(interp, receiver, selector, args)
                .map(wrap_error)
        }
```

Delete `value_step` with its doc comment, and delete `call_block` with its doc
comment. Change `wrap_error`'s doc comment to "Lifts a leaf native's result
into the step it is."

- [ ] **Step 8: Resume the loop; remove `invoke_closure`**

In `crates/ns-runtime/src/interpreter/mod.rs`, add to `resume`'s match, after
the `WhileBody` arm:

```rust
            Continuation::DoElement(state) => Ok(crate::builtins::collections::after_element(state)),
```

Delete `invoke_closure` with its doc comment. In `reenter`'s doc comment,
replace "The public `send`, each initializing level of an instantiation and its
clause message, the nested-class accessor's clause, and every native that
still calls a block come through here" with "The public `send`, each
initializing level of an instantiation and its clause message, and the
nested-class accessor's clause come through here".

In `crates/ns-runtime/src/builtins/boolean.rs`, `block_step`'s last arm becomes:

```rust
        Continuation::WhileCondition(_)
        | Continuation::WhileBody(_)
        | Continuation::DoElement(_) => {
            unreachable!("internal: a Boolean's control-flow native runs no loop")
        }
```

- [ ] **Step 9: Run the tests**

Run: `cargo test -p ns-runtime --lib deciding_do_answers`
Expected: PASS.

Run: `cargo test -p ns-runtime --test trampoline a_deep_recursion_through_do`
Expected: PASS.

Run: `cargo test -p ns-cli --test program_outcomes a_recursion_through_do`
Expected: PASS.

Then the gates (Global Constraints), all green. `grep -rn "call_block\|invoke_closure" crates/` (use the Grep tool) finds nothing but
the historical plan documents, which are not under `crates/`.

- [ ] **Step 10: Measure**

Follow the measurement protocol against the commit this task started from.
`bench_do.ns` should be faster (no nested run per element); no source may be
slower.

- [ ] **Step 11: Commit**

Stage `crates/ns-runtime/src/types.rs`, `crates/ns-runtime/src/builtins/`,
`crates/ns-runtime/src/interpreter/mod.rs`,
`crates/ns-runtime/tests/builtins_collections_block.rs`,
`crates/ns-runtime/tests/trampoline.rs` and
`crates/ns-cli/tests/program_outcomes.rs`, and commit with the subject
`feat: activate do:'s block as a trampoline step`. The body says in one
paragraph what changed, that the `do-recursion` golden now ends at the
activation limit and why, and the measured medians; it ends with the trailer
from Global Constraints.

---

### Task 2: The nested-class accessor as a trampoline step

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`Continuation`, new
  `NestedClassCreation`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Step::Body`,
  `BodyStep`, `push_step`, `resume`, new `abandon` and `leave_nested_class`,
  `nested_in_progress`, `send`, `decide_send`, a unit test)
- Modify: `crates/ns-runtime/src/interpreter/nested.rs` (`nested_class`, new
  `after_clause`, `in_progress_entry`, `in_progress`; remove
  `in_progress_key` and `eval_superclass_clause`; `run_clause_body`'s doc)
- Modify: `crates/ns-runtime/src/interpreter/sends.rs` (two call sites)
- Modify: `crates/ns-runtime/src/builtins/boolean.rs` (`block_step`'s match)
- Modify: `crates/ns-runtime/src/vm.rs` (`unwind`)
- Modify: `crates/ns-runtime/src/heap.rs` (`object_identity_key`'s doc comment)
- Test: `crates/ns-runtime/tests/nested_classes.rs`,
  `crates/ns-runtime/tests/trampoline.rs`,
  `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:**
- Consumes: nothing from Task 1 beyond a green tree.
- Produces: `Step::Body(Box<BodyStep>)` and
  `pub(crate) struct BodyStep { pub(crate) receiver: Value, pub(crate)
  current_class: ClassRef, pub(crate) kind: FrameKind, pub(crate) slot_base:
  usize, pub(crate) code: Rc<crate::bytecode::CompiledCode>, pub(crate) args:
  Vec<Value>, pub(crate) then: Continuation }` (`interpreter/mod.rs`) — Task 3
  builds its initializer and clause-message activations with it;
  `pub(crate) fn Interpreter::abandon(&mut self, then: Continuation)`;
  `Continuation::NestedClass(Box<NestedClassCreation>)`;
  `pub(crate) fn nested::nested_class(interp, receiver: &Value, nested:
  &Rc<Mixin>, declaring_class: ClassRef) -> Result<Step, Signal>` (was
  `-> EvalResult`); `nested_in_progress: Vec<(ObjRef, usize)>`.

- [ ] **Step 1: Write the failing unit test**

In `crates/ns-runtime/src/interpreter/mod.rs`, `mod tests`:

```rust
    /// The nested-class accessor is decided as the activation of the
    /// superclass clause, with steps 3-5 in the continuation. Nothing is
    /// pushed, so no creation is in progress yet: one is recorded only once
    /// its clause's activation is on the stack.
    #[test]
    fn deciding_a_nested_class_answers_its_superclass_clause_as_an_activation() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public class N = self base (
              ) (
              ) : ()
              base = ( ^Object )
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        match interp.decide_send(instance, Symbol::intern("N"), &[]) {
            Ok(Step::Body(body)) => {
                assert!(matches!(body.kind, FrameKind::SuperclassClause(_)));
                assert!(matches!(body.then, Continuation::NestedClass(_)));
                assert!(body.args.is_empty(), "a clause takes no arguments");
            }
            _ => panic!("a nested class is decided as its clause's activation"),
        }
        assert_eq!(interp.heap.activation_depth(), 0, "nothing was pushed");
        assert!(interp.nested_in_progress.is_empty(), "nothing is in progress");
    }
```

- [ ] **Step 2: Write the guard and the deep-recursion test**

Append to `crates/ns-runtime/tests/nested_classes.rs` (it has `load`,
`expect_ok` and `indoc` already; check their names at the top of the file and
use them):

```rust
/// A superclass clause that fails leaves no creation in progress behind: the
/// next access evaluates the clause again, rather than reporting the class as
/// depending on itself.
#[test]
fn a_failed_superclass_clause_is_retried_rather_than_reported_as_a_cycle() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Outer = Object new (
          | first ::= true. |
        ) (
          public class Inner = self pick (
          ) (
          ) : ()
          pick = ( first ifTrue: [first := false. ^nil foo]. ^Object )
        ) : ()
    "};
    let (mut interp, class) = load(src);
    let outer = expect_ok(interp.instantiate(class, vec![]), "instantiate Outer");
    match interp.send(outer, Symbol::intern("Inner"), vec![]) {
        Err(Signal::Error(e)) => assert!(
            !e.message.contains("cyclic"),
            "the first access fails in its clause: {}",
            e.message
        ),
        Err(Signal::Return(..)) => panic!("a non-local return escaped"),
        Ok(_) => panic!("the first access must fail"),
    }
    let inner = expect_ok(
        interp.send(outer, Symbol::intern("Inner"), vec![]),
        "the second access",
    );
    let Value::Obj(inner) = inner else {
        panic!("a class object");
    };
    assert!(interp.heap_ref().is_class(inner));
}
```

Append to `crates/ns-runtime/tests/trampoline.rs`:

```rust
/// The nested-class accessor is a trampoline step: a recursion through it --
/// each level a fresh `Link` whose `Next` clause reaches the accessor again --
/// ends at the activation limit, and the unwind takes every creation in
/// progress back off (`Interpreter::send` debug-asserts that).
#[test]
fn a_recursion_through_the_nested_class_accessor_ends_at_the_activation_limit() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Chain = Object new (
        ) (
            public class Link = Object new (
            ) (
                public class Next = self deeper (
                ) (
                ) : ()
                deeper = ( ^Link new Next )
            ) : ()
            public start = ( ^Link new Next )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(src, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        expect_too_deep(
            interp.send(instance, Symbol::intern("start"), vec![]),
            &format!("start under {lowering:?}"),
        );
        assert_eq!(interp.heap_ref().activation_depth(), 0, "{lowering:?}");
        assert_eq!(interp.heap_ref().counted_depth(), 0, "{lowering:?}");
    }
}
```

- [ ] **Step 3: Change the `accessor-recursion` golden**

In `crates/ns-cli/tests/program_outcomes.rs`, replace
`a_recursion_through_the_nested_class_accessor_is_bounded_by_the_native_limit`
and its doc comment with:

```rust
/// The nested-class accessor runs a superclass clause as a trampoline step,
/// the first time a class is reached on each enclosing object. `Next`'s
/// clause, `self deeper`, reaches `Next` again on a fresh `Link`, so every
/// level is a new enclosing object -- no cyclic-definition error -- and two
/// more counted activations, the clause and `deeper`, until the activation
/// limit. `Link` has no slots, so its instantiation activates nothing. It used
/// to end at the limit on re-entries of the run loop from Rust, which no
/// longer exists (`docs/design/execution.md`, "No re-entry from Rust").
#[test]
fn a_recursion_through_the_nested_class_accessor_is_bounded_by_the_activation_limit() {
    assert_outcome(
        "accessor-recursion",
        indoc! {"
            Newspeak3
            'T'
            class Chain usingPlatform: platform = Object new (
            ) (
                public class Link = Object new (
                ) (
                    public class Next = self deeper (
                    ) (
                    )
                    deeper = ( ^Link new Next )
                )
                public main: p args: a = ( Link new Next )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -p ns-runtime --lib deciding_a_nested_class`
Expected: compile error — `Step::Body` does not exist.

Run: `cargo test -p ns-runtime --test nested_classes a_failed_superclass_clause`
Expected: PASS — the guard describes today's behaviour.

Run: `cargo test -p ns-cli --test program_outcomes a_recursion_through_the_nested`
Expected: FAIL — `... too many nested native calls`.

Do not run the new `trampoline.rs` test yet, for the reason Task 1 gives.

- [ ] **Step 5: Add the continuation and its state**

In `crates/ns-runtime/src/types.rs`, after the `DoElement` variant:

```rust
    /// The superclass clause the nested-class accessor evaluated returned the
    /// superclass: build, cache and answer the class
    /// (`interpreter::nested::after_clause`).
    NestedClass(Box<NestedClassCreation>),
```

In `crates/ns-runtime/src/builtins/boolean.rs`, `block_step`'s last arm gains
the new variant, or the match stops being exhaustive:

```rust
        Continuation::WhileCondition(_)
        | Continuation::WhileBody(_)
        | Continuation::DoElement(_)
        | Continuation::NestedClass(_) => {
            unreachable!("internal: a Boolean's control-flow native runs no loop")
        }
```

After `DoLoop` in `types.rs`:

```rust
/// A nested class being created for one enclosing object
/// (`interpreter::nested`): what the accessor needs once the declaration's
/// superclass clause has been evaluated.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct NestedClassCreation {
    /// The enclosing object `r` of `newspeak-spec.pdf` §6, which caches the
    /// class.
    pub(crate) enclosing: ObjRef,
    /// The absolute index of the hidden slot of `enclosing` the class is
    /// cached in.
    pub(crate) index: usize,
    /// The declaration.
    pub(crate) nested: Rc<crate::mixin::Mixin>,
    /// The class object of the level that declares it.
    pub(crate) declaring_class: ClassRef,
}
```

- [ ] **Step 6: Add `Step::Body` and push it**

In `crates/ns-runtime/src/interpreter/mod.rs`, add a variant to `Step` after
`Block`:

```rust
    /// Activate a body that is neither a method nor a block -- an instance
    /// initializer, a superclass clause, or the arguments of a clause's
    /// message -- and do `then` with its value. Boxed, so that a decision's
    /// `Result<Step, Signal>` stays the size the hot path was measured at
    /// (`StepArgs::ControlFlow`'s comment says why that matters).
    Body(Box<BodyStep>),
```

After `impl StepArgs { … }`:

```rust
/// Everything `Step::Body` activates. Its arguments are its own: nothing a
/// body step activates takes a send's arguments.
pub(crate) struct BodyStep {
    pub(crate) receiver: Value,
    pub(crate) current_class: ClassRef,
    pub(crate) kind: FrameKind,
    /// See `Frame::slot_base`; 0 for anything but an initializer.
    pub(crate) slot_base: usize,
    pub(crate) code: Rc<crate::bytecode::CompiledCode>,
    pub(crate) args: Vec<Value>,
    pub(crate) then: Continuation,
}
```

Add an arm to `push_step`'s match, after the `Step::Block` arm:

```rust
            // An initializer level, a superclass clause or its message is
            // never an activation the optimized lowering inlines away, so it
            // is always counted.
            Step::Body(body) => {
                let BodyStep {
                    receiver,
                    current_class,
                    kind,
                    slot_base,
                    code,
                    args,
                    then,
                } = *body;
                // A nested class's creation is in progress exactly while its
                // clause's activation is on the stack: recorded once the push
                // has succeeded, and taken off by `resume` or `abandon` when
                // the activation is popped. A push refused at the depth limit
                // records nothing.
                let in_progress = match &then {
                    Continuation::NestedClass(creation) => Some(nested::in_progress_entry(creation)),
                    _ => None,
                };
                let temps = (code.num_registers - code.num_args) as usize;
                let act = self.push(
                    ActivationHeader {
                        receiver,
                        current_class: Some(current_class),
                        kind,
                        slot_base,
                        code,
                        then,
                        counted: true,
                    },
                    &args,
                    temps,
                )?;
                if let Some(entry) = in_progress {
                    self.nested_in_progress.push(entry);
                }
                Ok(act)
            }
```

- [ ] **Step 7: `nested_in_progress`, `resume`, `abandon`, `send`**

Replace the `nested_in_progress` field and its doc comment with:

```rust
    /// The nested classes whose creation is underway: for each, the
    /// enclosing object and the declaration's identity -- see
    /// `nested::cyclic_definition_error`. An entry exists exactly while the
    /// activation evaluating that class's superclass clause is on the stack:
    /// `push_step` records it once the push has succeeded, and `resume` or
    /// `abandon` takes it off when the activation is popped, so this list and
    /// the stack pop in the same order. The object is an `ObjRef`, compared
    /// with `same`, not an arena index, because a moving collector must be
    /// able to rewrite it (`docs/design/memory.md`, "What the collector
    /// traces"). A stack, not a set: it is empty on every path that is not
    /// inside a superclass clause, and a clause nests only as deep as a
    /// program actually chains class creations, so a linear scan beats
    /// hashing and allocates nothing in the common case.
    nested_in_progress: Vec<(ObjRef, usize)>,
```

Add to `resume`'s match:

```rust
            Continuation::NestedClass(creation) => {
                self.leave_nested_class(&creation);
                nested::after_clause(self, *creation, value)
            }
```

After `resume`, add:

```rust
    /// What a continuation undoes when its activation is thrown away rather
    /// than returned from: the counterpart of `resume`, which `vm::unwind`
    /// calls for every activation it pops. Only a nested class's creation has
    /// begun anything -- it is in progress -- so only it has anything to undo;
    /// every other continuation is simply dropped.
    pub(crate) fn abandon(&mut self, then: Continuation) {
        if let Continuation::NestedClass(creation) = then {
            self.leave_nested_class(&creation);
        }
    }

    /// Takes `creation` off `nested_in_progress`: its clause's activation has
    /// been popped, by a return or by an unwind.
    fn leave_nested_class(&mut self, creation: &NestedClassCreation) {
        let left = self.nested_in_progress.pop();
        debug_assert!(
            left.is_some_and(|(obj, key)| key == crate::mixin::mixin_key(&creation.nested)
                && obj.same(creation.enclosing)),
            "internal: nested-class creations end in the order their clauses were pushed"
        );
    }
```

In `send`, after the existing `debug_assert_eq!`, add:

```rust
        // A creation begun below a host's send ends before the send returns,
        // however it returns: `resume` or `abandon` takes it off.
        debug_assert!(
            depth != 0 || self.nested_in_progress.is_empty(),
            "internal: a nested class's creation outlived the send that began it"
        );
```

In `decide_send`, the two nested-class arms lose their `.map(Step::Value)`:

```rust
                        Lookup::NestedClass(mixin, declaring_class) => {
                            return nested::nested_class(self, &receiver, &mixin, declaring_class);
                        }
```

```rust
            Lookup::NestedClass(mixin, declaring_class) => {
                nested::nested_class(self, &receiver, &mixin, declaring_class)
            }
```

In `crates/ns-runtime/src/interpreter/sends.rs` the two arms likewise become
`crate::interpreter::nested::nested_class(interp, &o, &mixin, declaring_class)`
and `crate::interpreter::nested::nested_class(interp, &self_value, &mixin,
declaring_class)`, without `.map(Step::Value)`.

- [ ] **Step 8: Rewrite the accessor in `nested.rs`**

In `crates/ns-runtime/src/interpreter/nested.rs`, change the imports to:

```rust
use std::rc::Rc;

use ns_syntax::ast;

use crate::heap::{ActivationHeader, Heap};
use crate::mixin::{mixin_key, Mixin};
use crate::types::*;
use crate::Symbol;

use super::{BodyStep, EvalResult, Interpreter, Signal, Step};
```

(`Heap` and `ActivationHeader` stay only while `run_clause_body` exists; the
compiler will say when one is unused.)

Replace `nested_class` with:

```rust
pub(crate) fn nested_class(
    interp: &mut Interpreter,
    receiver: &Value,
    nested: &Rc<Mixin>,
    declaring_class: ClassRef,
) -> Result<Step, Signal> {
    let Some((obj, index)) = cache_slot(interp, receiver, nested) else {
        return Err(no_cache_slot_error(receiver, nested.name));
    };
    // Step 1: the receiver's own cache. `Nil` is the "not yet" marker --
    // `instantiate` fills every slot, hidden ones included, with `Nil`, and
    // a materialized class object is never `Nil`, so no separate occupancy
    // flag is needed.
    if let Some(cached) = read_cache_slot(&interp.heap, receiver, index) {
        return Ok(Step::Value(cached));
    }
    // Step 2: the superclass clause, evaluated in the receiver's context --
    // guarded against a class whose own creation is already underway on
    // this receiver.
    if in_progress(interp, obj, nested) {
        return Err(cyclic_definition_error(receiver, nested.name));
    }
    let creation = NestedClassCreation {
        enclosing: obj,
        index,
        nested: nested.clone(),
        declaring_class,
    };
    // Whether there is a clause at all is read off the mixin, not off its
    // declaration: a mixin decoded from an image has no declaration, and the
    // answer decides whether any code runs.
    if nested.superclass.is_none() {
        // No clause at all (`class N = (...)`): `Object`, the same default
        // `instantiate::resolve_superclass_receiver` gives a root class.
        let object = Value::Obj(interp.builtin_class(BuiltinClass::Object));
        return after_clause(interp, creation, object);
    }
    let code = nested.superclass_clause_code.clone().unwrap_or_else(|| {
        panic!("internal: mixin with a superclass clause carries no compiled form for it (a mixin built from source always does; verify_mixins refuses a decoded one)")
    });
    // The clause belongs to the class that *declares* it: it is resolved
    // against that declaration's enclosing chain, and `super`-prefixed forms
    // look up from that class's superclass -- which is why the activation's
    // class is the declaring class rather than the receiver's own (dynamic)
    // class, the two differing exactly when the declaration is inherited. Its
    // kind names the table its body came from, which `image::decode::frame_body`
    // resolves a frame's body by, and makes it a return target
    // (`Interpreter::catches_return`).
    Ok(Step::Body(Box::new(BodyStep {
        receiver: *receiver,
        current_class: declaring_class,
        kind: FrameKind::SuperclassClause(declaring_class),
        slot_base: 0,
        code,
        args: Vec::new(),
        then: Continuation::NestedClass(Box::new(creation)),
    })))
}

/// Steps 3-5, once the superclass clause has evaluated to `superclass`:
/// `Interpreter::resume` calls this when the clause's activation returns, and
/// `nested_class` directly for a declaration with no clause. By value, not
/// boxed: only the continuation needs the box.
pub(crate) fn after_clause(
    interp: &mut Interpreter,
    creation: NestedClassCreation,
    superclass: Value,
) -> Result<Step, Signal> {
    let NestedClassCreation {
        enclosing,
        index,
        nested,
        declaring_class,
    } = creation;
    let Value::Obj(superclass) = superclass else {
        return Err(not_a_class_error(nested.name, superclass));
    };
    let Some(super_application) = interp.heap.class_application(superclass) else {
        return Err(not_a_class_error(nested.name, Value::Obj(superclass)));
    };
    // Step 3: the application cache decides whether this behaviour already
    // exists. Two enclosing instances whose clauses land on the same
    // superclass application share one application, and therefore one
    // compiled body and one instance layout (§6.2).
    let application = interp.apply_mixin(&nested, Some(&super_application));
    // Step 4: identity is per receiver. The class object's superclass chain
    // and its application's superclass chain are linked together here --
    // `dispatch::next_level` steps both and stops if either is missing.
    // `declaring_class` records the level this declaration is written in,
    // which is what `dispatch::kth_enclosing_object` steps out along; unlike
    // the superclass chain it does not follow inheritance.
    let class = interp.new_class_object(
        application,
        Some(enclosing),
        Some(declaring_class),
        Some(superclass),
    );
    // Step 5: cache before answering, so step 1 hits from now on.
    crate::interpreter::locals::store_instance_field(
        &mut interp.heap,
        &Value::Obj(enclosing),
        index,
        Value::Obj(class),
    );
    Ok(Step::Value(Value::Obj(class)))
}
```

Replace `in_progress_key` and its doc comment with:

```rust
/// The entry `Interpreter::nested_in_progress` records while `creation`'s
/// clause runs: the enclosing object and the declaration's identity. The
/// declaration, not its name, for the same reason `cache_slot` uses it:
/// `class N = super N (...)` is a subclass redeclaring what it inherits, not
/// a cycle, and a name key would report the legal shape as one.
pub(super) fn in_progress_entry(creation: &NestedClassCreation) -> (ObjRef, usize) {
    (creation.enclosing, mixin_key(&creation.nested))
}

/// Whether the class `nested` declares is being created for `enclosing`
/// right now, further down the stack.
fn in_progress(interp: &Interpreter, enclosing: ObjRef, nested: &Rc<Mixin>) -> bool {
    let key = mixin_key(nested);
    interp
        .nested_in_progress
        .iter()
        .any(|(obj, entry)| *entry == key && obj.same(enclosing))
}
```

In `cache_slot`'s doc comment, replace "`in_progress_key` below is exactly
that: it takes an `ObjRef`, not a `Value`, so a non-object receiver cannot
reach it at all." with "`NestedClassCreation` is exactly that: it holds an
`ObjRef`, not a `Value`, so a non-object receiver cannot reach it at all."

Delete `eval_superclass_clause` with its doc comment (its logic moved into
`nested_class`). Change `run_clause_body`'s doc comment to:

```rust
/// A superclass clause's *message arguments*, compiled onto the mixin, in a
/// fresh activation the run loop runs and pops (`Interpreter::reenter`) --
/// `instantiate`'s deferred clause message, one level down. The nested-class
/// accessor evaluates a clause's receiver as a trampoline step instead
/// (`nested_class`); this goes when instantiation is one too.
```

Change `cyclic_definition_error`'s doc comment: replace "so a clause naming the
class being created re-enters the accessor before the cache slot is filled
and would otherwise recurse until the native-depth bound stopped it" with "so
a clause naming the class being created reaches the accessor again before the
cache slot is filled and would otherwise recurse until the activation limit
stopped it".

- [ ] **Step 9: Abandon in `unwind`**

In `crates/ns-runtime/src/vm.rs`, `unwind`: in both loops, replace
`interp.heap.pop_activation(top);` with:

```rust
                let popped = interp.heap.pop_activation(top);
                interp.abandon(popped.then);
```

(indent to each loop's level). Append to `unwind`'s doc comment: "Every
activation it pops hands its continuation to `Interpreter::abandon`, which
undoes what a continuation that will now never run had begun."

- [ ] **Step 10: The identity key's doc comment**

In `crates/ns-runtime/src/heap.rs`, `object_identity_key`'s doc comment: the
list of callers becomes "`image::encode::GraphTable`'s object-id map and
`image::decode::rebuild_applications`'s superclass-position map" (drop
"and `interpreter::nested::in_progress_key`", and fix the punctuation of the
list).

- [ ] **Step 11: Run the tests**

Run: `cargo test -p ns-runtime --lib deciding_a_nested_class` — PASS.
Run: `cargo test -p ns-runtime --test trampoline a_recursion_through_the_nested` — PASS.
Run: `cargo test -p ns-cli --test program_outcomes a_recursion_through_the_nested` — PASS.
Then the gates, all green.

- [ ] **Step 12: Measure**

The measurement protocol, against the commit this task started from. This
task adds a `Step` variant; no source may be slower.

- [ ] **Step 13: Commit**

`feat: run the nested-class accessor's clause as a trampoline step`, with a
body naming the golden change, `nested_in_progress` holding `ObjRef`s, and the
measurement; the trailer as in Global Constraints.

---

### Task 3: Instantiation as a state machine; the host entry

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`Continuation`, new
  `Instantiation`, `InstantiationPhase`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`try_factory_send`,
  `instantiate`; new `decide_instantiation`, `climb_levels`,
  `clause_message_body`, `descend_levels`, `advance_instantiation`; remove
  `instantiate_unchecked`, `run_initializer_levels`, `run_own_initializer`,
  `eval_superclass_message`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`send`, new
  `run_from_host`, `running`; remove `reenter`, `native_depth`,
  `MAX_NATIVE_DEPTH`, `too_many_nested_native_calls`; `decide_send`, `resume`,
  `arg_buffer`'s doc, a unit test)
- Modify: `crates/ns-runtime/src/interpreter/nested.rs` (remove
  `run_clause_body`)
- Modify (comments that name removed functions): `crates/ns-runtime/src/types.rs:293`,
  `mixin.rs:118,239`, `bytecode.rs:158`, `compiler/lower.rs:216`,
  `image/decode.rs:155,760,800,815`, `tests/instantiate.rs:68`,
  `tests/vm_initializer.rs:189`, `tests/image.rs:3234,4627,4966`
- Test: `crates/ns-runtime/tests/trampoline.rs`,
  `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:**
- Consumes: `Step::Body`, `BodyStep` (Task 2).
- Produces: `pub(crate) fn Interpreter::decide_instantiation(&mut self, class:
  ClassRef, factory_args: Vec<Value>) -> Result<Step, Signal>`;
  `pub(crate) fn Interpreter::advance_instantiation(&mut self, state:
  Box<Instantiation>, value: Value) -> Result<Step, Signal>`;
  `Continuation::Instantiate(Box<Instantiation>)`; `pub fn
  Interpreter::instantiate` and `pub fn Interpreter::send` keep their
  signatures. After this task nothing re-enters the run loop, and
  `MAX_NATIVE_DEPTH` no longer exists (Task 4 fixes the one doc comment in
  `ns-cli` that names it).

- [ ] **Step 1: Write the failing unit test**

In `crates/ns-runtime/src/interpreter/mod.rs`, `mod tests`:

```rust
    /// A factory send is decided as the instantiation's first body to run --
    /// here `P`'s initializer -- with the rest of the walk in the
    /// continuation; a class with nothing to initialize at any level is
    /// decided as its instance outright.
    #[test]
    fn deciding_new_answers_the_first_initializer_as_an_activation() {
        let src = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              public class P = Object new (
                | v = 1. |
              ) (
              ) : ()
              public class Q = Object new (
              ) (
              ) : ()
            ) : ()
        "};
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        let p = expect_ok(interp.send(instance, Symbol::intern("P"), vec![]), "P");
        let q = expect_ok(interp.send(instance, Symbol::intern("Q"), vec![]), "Q");
        match interp.decide_send(p, Symbol::NEW, &[]) {
            Ok(Step::Body(body)) => {
                assert!(matches!(body.kind, FrameKind::Initializer(_)));
                assert!(matches!(body.then, Continuation::Instantiate(_)));
            }
            _ => panic!("`P new` is decided as P's initializer"),
        }
        assert!(
            matches!(interp.decide_send(q, Symbol::NEW, &[]), Ok(Step::Value(Value::Obj(_)))),
            "`Q new` is decided as its instance"
        );
        assert_eq!(interp.heap.activation_depth(), 0, "nothing was pushed");
    }
```

- [ ] **Step 2: Write the deep-recursion test**

Append to `crates/ns-runtime/tests/trampoline.rs`:

```rust
/// Instantiation is a trampoline step: a class whose slot initializer
/// instantiates it again, twenty thousand levels deep, is twenty thousand
/// initializer activations on the stack and nothing on the Rust stack.
#[test]
fn a_deep_recursion_through_instantiation_runs_on_the_activation_stack() {
    let src = indoc! {"
        Newspeak3
        'T'
        class Tower = Object new (
        ) (
            public class Floor n: k = Object new (
                | below = k = 0 ifTrue: [nil] ifFalse: [Floor n: k - 1]. |
            ) (
                public height = ( ^below == nil ifTrue: [0] ifFalse: [1 + below height] )
            ) : ()
            public build: k = ( ^(Floor n: k) height )
        ) : ()
    "};
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(src, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        let height = expect_ok(
            interp.send(instance, Symbol::intern("build:"), vec![Value::SmallInt(20_000)]),
            "build:",
        );
        assert_eq!(small_int(height), 20_000, "{lowering:?}");
        assert_eq!(interp.heap_ref().activation_depth(), 0, "{lowering:?}");
        assert_eq!(interp.heap_ref().counted_depth(), 0, "{lowering:?}");
    }
}
```

In the same file, replace the doc comment of
`a_return_into_a_running_initializer_escapes_the_runs_below_it` with:

```rust
/// A `^` into an initializer that is still running escapes -- the known gap
/// in `docs/design/execution.md`, "Non-local return" -- and it escapes as the
/// signal it always was, not as a panic: an initializer catches no return,
/// so the unwind pops everything down to the floor of the host's run and
/// leaves it with the signal. Closing the gap changes what this expects;
/// nothing else should.
```

and rename the test to `a_return_into_a_running_initializer_escapes_the_run`.

- [ ] **Step 3: Change the `instantiation-recursion` golden**

In `crates/ns-cli/tests/program_outcomes.rs`, replace
`a_recursive_instantiation_is_bounded_by_the_native_limit` and its doc comment
with:

```rust
/// Instantiation runs each initializer level as a trampoline step, so a class
/// whose slot initializer instantiates it again is a chain of counted
/// initializer activations and ends at the activation limit. It used to end at
/// the limit on re-entries of the run loop from Rust, which no longer exists
/// (`docs/design/execution.md`, "No re-entry from Rust").
#[test]
fn a_recursive_instantiation_is_bounded_by_the_activation_limit() {
    assert_outcome(
        "instantiation-recursion",
        indoc! {"
            Newspeak3
            'T'
            class Tower usingPlatform: platform = Object new (
            ) (
                public class Floor = Object new (
                    | above = Floor new. |
                ) (
                )
                public main: p args: a = ( Floor new )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cargo test -p ns-runtime --lib deciding_new_answers`
Expected: compile error — `Continuation::Instantiate` does not exist.

Run: `cargo test -p ns-cli --test program_outcomes a_recursive_instantiation`
Expected: FAIL — `... too many nested native calls`.

Do not run the new `trampoline.rs` test yet, for the reason Task 1 gives.

- [ ] **Step 5: Add the continuation and its state**

In `crates/ns-runtime/src/types.rs`, after the `NestedClass` variant:

```rust
    /// A body an instantiation ran returned -- a clause message's arguments,
    /// or a level's initializer: carry the walk on
    /// (`Interpreter::advance_instantiation`).
    Instantiate(Box<Instantiation>),
```

After `NestedClassCreation`:

```rust
/// An instantiation in progress (`newspeak-spec.pdf` §6.2, and the six steps
/// of `docs/design/object-model.md`, "Instantiation"): the instance, every
/// level of the superclass chain the upward walk has reached with the factory
/// arguments it gets, and which way the walk is going.
///
/// Boxed in its continuation, and owned by exactly one stack entry.
#[derive(Debug, Clone)]
pub struct Instantiation {
    pub(crate) instance: Value,
    /// Nearest level first: the class instantiated, then its superclass, and
    /// so on up, as the upward walk reaches them.
    pub(crate) levels: Vec<(ClassRef, Vec<Value>)>,
    pub(crate) phase: InstantiationPhase,
}

/// Which way an instantiation's walk is going.
#[derive(Debug, Clone, Copy)]
pub enum InstantiationPhase {
    /// Upward. A body running for this instantiation is the clause message
    /// of the last level in `levels`, and its value is its superclass's
    /// factory arguments.
    Climbing,
    /// Downward. A body running for this instantiation is the initializer of
    /// `levels[index]`.
    Descending { index: usize },
}
```

In the `Frame::slot_base` doc comment (`types.rs`, near line 293), replace
"(`Interpreter::run_own_initializer`)" with "(`Interpreter::descend_levels`)".

- [ ] **Step 6: Rewrite instantiation**

In `crates/ns-runtime/src/interpreter/instantiate.rs`:

Change `use crate::heap::{ActivationHeader, Heap};` to `use crate::heap::Heap;`
and `use super::{EvalResult, Interpreter, Signal};` to
`use super::{BodyStep, EvalResult, Interpreter, Signal, Step};`.

In `load_module`'s doc comment replace "where `run_initializer_levels`
evaluates it off the mixin's own declaration" with "where `climb_levels`
evaluates it off the mixin". In `try_factory_send`'s doc comment replace every
"`run_initializer_levels`" with "`climb_levels`" and "`Interpreter::send`" with
"`Interpreter::decide_send`".

Replace `try_factory_send`'s signature and body with:

```rust
    pub(super) fn try_factory_send(
        &mut self,
        receiver: &Value,
        selector: Symbol,
        args: &[Value],
    ) -> Option<Result<Step, Signal>> {
        let Value::Obj(obj) = receiver else {
            return None;
        };
        let factory_selector = primary_factory_selector(&self.heap, *obj)?;
        if selector != factory_selector {
            return None;
        }
        let builtin = self.heap.class_builtin(*obj);
        let class_ref = *obj;
        Some(match builtin {
            // (keep the existing comment about `ObjectData::Array` verbatim)
            Some(BuiltinClass::OrderedCollection) | Some(BuiltinClass::Array) => Ok(Step::Value(
                Value::Obj(self.new_instance_object(class_ref, ObjectData::Array(vec![]))),
            )),
            _ => self.decide_instantiation(class_ref, args.to_vec()),
        })
    }
```

Replace `instantiate`, `instantiate_unchecked` and `run_initializer_levels`
(with their doc comments) with:

```rust
    /// Instantiates `class` from a host: the six numbered steps in
    /// `docs/design/object-model.md`, "Instantiation", decided
    /// (`decide_instantiation`) and then run from the outside
    /// (`run_from_host`), like a host's `send`.
    pub fn instantiate(&mut self, class: ClassRef, factory_args: Vec<Value>) -> EvalResult {
        match self.decide_instantiation(class, factory_args) {
            Ok(step) => self.run_from_host(step, &[]),
            Err(signal) => Err(signal),
        }
    }

    /// Decides an instantiation of `class` with `factory_args` without running
    /// any of its code: the instance comes back as a value when no level of
    /// the superclass chain has code to run, and otherwise the first body that
    /// must run, with the rest of the walk in its continuation
    /// (`Continuation::Instantiate`, carried on by `advance_instantiation`).
    /// Steps 2-5 are `climb_levels` and `descend_levels`, for every level of
    /// the superclass chain.
    pub(crate) fn decide_instantiation(
        &mut self,
        class: ClassRef,
        factory_args: Vec<Value>,
    ) -> Result<Step, Signal> {
        // Step 1: one instance, every slot nil. The application already
        // knows the whole layout, inherited slots included -- summing the
        // chain here again would be a second answer to the same question.
        let slot_count = self
            .heap
            .class_application(class)
            .map_or(0, |a| a.num_slots);
        let instance =
            self.new_instance_object(class, ObjectData::Instance(vec![Value::Nil; slot_count]));
        let state = Box::new(Instantiation {
            instance: Value::Obj(instance),
            levels: Vec::new(),
            phase: InstantiationPhase::Climbing,
        });
        self.climb_levels(state, class, factory_args)
    }

    /// One body an instantiation ran has returned `value`: carry the walk on
    /// from where `state` says it stopped. `Interpreter::resume` calls this.
    pub(crate) fn advance_instantiation(
        &mut self,
        state: Box<Instantiation>,
        value: Value,
    ) -> Result<Step, Signal> {
        match state.phase {
            InstantiationPhase::Climbing => {
                let level = state
                    .levels
                    .last()
                    .expect("internal: a clause message runs for a level the walk has recorded")
                    .0;
                let superclass = self
                    .heap
                    .class_superclass(level)
                    .expect("internal: a clause message runs only for a level with a superclass");
                let super_args = argument_list(&self.heap, level, value)?;
                self.climb_levels(state, superclass, super_args)
            }
            // An initializer's value is not used; only its effects on the
            // instance are.
            InstantiationPhase::Descending { index } => self.descend_levels(state, index),
        }
    }

    /// The upward walk, from `level` with `args`: each level's factory arity
    /// checked, then its superclass clause's message evaluated to the
    /// superclass's arguments (step 3's first half). A message with arguments
    /// is code, so the walk stops there and answers its activation, and
    /// `advance_instantiation` resumes it with the value. Walked with a loop
    /// and a list rather than by recursing per level: the chain's length is an
    /// image's to choose, and a level with no code pushes no activation, so the
    /// activation limit would not see a long chain of them
    /// (`docs/design/execution.md`, "A superclass chain is walked
    /// iteratively"). At the top of the chain, the downward walk begins.
    fn climb_levels(
        &mut self,
        mut state: Box<Instantiation>,
        mut level: ClassRef,
        mut args: Vec<Value>,
    ) -> Result<Step, Signal> {
        loop {
            self.check_factory_arity(level, &args)?;
            let Some(superclass) = self.heap.class_superclass(level) else {
                state.levels.push((level, args));
                let above_top = state.levels.len();
                return self.descend_levels(state, above_top);
            };
            let message = self.clause_message_body(level, superclass)?;
            let Some(code) = message else {
                // `Object new` and every other argument-free clause: nothing
                // to run, and the superclass gets no arguments.
                state.levels.push((level, args));
                level = superclass;
                args = Vec::new();
                continue;
            };
            // The message is evaluated on the instance being initialized and
            // with *this* level's factory arguments -- §6.2's "in the context
            // of the instance initializer".
            let message_args = args.clone();
            let instance = state.instance;
            state.levels.push((level, args));
            return Ok(Step::Body(Box::new(BodyStep {
                receiver: instance,
                current_class: level,
                kind: FrameKind::SuperclassMessage(level),
                slot_base: 0,
                code,
                args: message_args,
                then: Continuation::Instantiate(state),
            })));
        }
    }

    /// The downward walk (steps 2, 4 and 5): below `next`, from the top of the
    /// chain down to the class instantiated, each level's own slot clauses and
    /// `initExprs`, in an activation of its own with the arguments its own
    /// subclass's clause supplied -- one activation per initializing level, so
    /// a superclass's initializers see the arguments *its* clause supplied. A
    /// level with nothing to initialize runs no code and is passed over --
    /// every chain ends at a builtin, which is exactly such a level -- and the
    /// first that has code is answered as an activation;
    /// `advance_instantiation` carries on below it when it returns. Past the
    /// last level, the instance (step 6).
    fn descend_levels(&mut self, mut state: Box<Instantiation>, mut next: usize) -> Result<Step, Signal> {
        while next > 0 {
            next -= 1;
            let level = state.levels[next].0;
            if !class_has_initializer_code(&self.heap, level) {
                continue;
            }
            let application = self
                .heap
                .class_application(level)
                .expect("internal: class_has_initializer_code confirmed this is a class object");
            let code = application.mixin.initializer_code.clone().unwrap_or_else(|| {
                panic!("internal: class needs an instance initializer and its mixin carries none (a mixin built from source always does; verify_mixins refuses a decoded one)")
            });
            // `InitSlot`'s `own_index` is relative to the mixin's own slots,
            // not an absolute field index, because the compiled body is shared
            // by every application of the mixin. This level's own
            // `MixinApplication.own_slot_base`, taken as the activation's
            // `slot_base`, turns it into an absolute index at run time; it is
            // set here, with the rest of the level's identity, and never
            // changes.
            let slot_base = application.own_slot_base;
            let args = std::mem::take(&mut state.levels[next].1);
            let instance = state.instance;
            state.phase = InstantiationPhase::Descending { index: next };
            return Ok(Step::Body(Box::new(BodyStep {
                receiver: instance,
                current_class: level,
                kind: FrameKind::Initializer(level),
                slot_base,
                code,
                args,
                then: Continuation::Instantiate(state),
            })));
        }
        Ok(Step::Value(state.instance))
    }
```

Keep `check_factory_arity`; in its comments replace "the activation
`run_own_initializer` pushes" with "the activation `descend_levels` answers"
and "selector `eval_superclass_message` has already matched" with "selector
`clause_message_body` has already matched".

Replace `run_own_initializer` (and its doc comment) — its logic now lives in
`descend_levels` — by nothing, and replace `eval_superclass_message` with:

```rust
    /// Step 3's first half, up to running anything: `newspeak-spec.pdf` §6.2's
    /// "the superclass clause's message is evaluated to a concrete message".
    /// Answers the compiled body that evaluates the message's arguments, or
    /// `None` when there are none.
    ///
    /// §6.1 makes two things run time errors here. That the clause's
    /// expression is not a class object is caught where the clause's
    /// *receiver* is evaluated -- once, when the declaration is evaluated
    /// (`interpreter::nested`), not per instantiation. That the message's
    /// selector is not the superclass's primary factory selector is caught
    /// here, because only here are both halves known.
    ///
    /// The selector is checked *before* the arguments are evaluated: a clause
    /// that cannot be satisfied should not first run its arguments' side
    /// effects.
    fn clause_message_body(
        &self,
        class: ClassRef,
        superclass: ClassRef,
    ) -> Result<Option<Rc<crate::bytecode::CompiledCode>>, Signal> {
        // One lookup of the application for both questions below: the clause
        // this class was declared with, and the mixin its compiled message
        // body lives on.
        let application = self.heap.class_application(class);
        let clause = application
            .as_ref()
            .and_then(|application| application.mixin.superclass);
        // (keep the existing comment about §6.1's `new` default verbatim)
        let selector = clause.map_or(Symbol::NEW, |clause| clause.selector);
        let expected = primary_factory_selector(&self.heap, superclass);
        if Some(selector) != expected {
            return Err(not_the_primary_factory_error(
                &self.heap, class, superclass, selector, expected,
            ));
        }
        if !clause.is_some_and(|clause| clause.has_args) {
            return Ok(None);
        }
        let code = application
            .as_ref()
            .and_then(|application| application.mixin.superclass_message_code.clone())
            .unwrap_or_else(|| {
                panic!("internal: mixin whose superclass clause message takes arguments carries no compiled form for it (a mixin built from source always does; verify_mixins refuses a decoded one)")
            });
        Ok(Some(code))
    }
```

In `primary_factory_selector`'s doc comment replace "`eval_superclass_message`"
with "`clause_message_body`"; in `mixin_needs_initializer_code`'s doc comment
replace "`run_own_initializer` above" with "`descend_levels` above"; in
`pattern_param_count`'s doc comment nothing changes.

- [ ] **Step 7: The host entry; remove re-entry**

In `crates/ns-runtime/src/interpreter/mod.rs`:

Delete `MAX_NATIVE_DEPTH` with its doc comment. In `MAX_ACTIVATION_DEPTH`'s doc
comment, "(`docs/design/execution.md`, "Two bounds, not one")" becomes
"(`docs/design/execution.md`, "The depth limit")".

In `decide_send`, the factory check becomes:

```rust
        if let Some(result) = self.try_factory_send(&receiver, selector, args) {
            return result;
        }
```

and in its doc comment replace "`run_initializer_levels` never calls `send`"
with "`climb_levels` never decides a send".

In the `Interpreter` struct, replace the `arg_buffer` doc comment's last
sentence ("Taken while a decision runs and given back afterwards, so a decision
that re-enters the loop takes a fresh one instead of clobbering its caller's.")
with "Taken while a send is decided and pushed and given back afterwards,
because the decision needs `&mut Interpreter` while it reads the arguments."
Replace the `native_depth` field and its doc comment with:

```rust
    /// Whether a host's run of the loop is in progress (`run_from_host`).
    /// Nothing the loop calls can start another -- no decision, native or
    /// continuation runs code from Rust -- and a debug build checks that.
    running: bool,
```

and in `bootstrap_with` replace `native_depth: 0,` with `running: false,`.

Replace `send` (with its doc comment) with:

```rust
    /// An ordinary (explicit-receiver) send from a host: `newspeak-spec.pdf`
    /// §5.5, decided as `decide_send` decides it and run from the outside
    /// (`run_from_host`). A `Send*` the VM issues never comes through here:
    /// `vm.rs` decides it and pushes what was decided itself.
    pub fn send(&mut self, receiver: Value, selector: Symbol, args: Vec<Value>) -> EvalResult {
        match self.decide_send(receiver, selector, &args) {
            Ok(step) => self.run_from_host(step, &args),
            Err(signal) => Err(signal),
        }
    }

    /// Runs what a host's send or instantiation decided. It is the only way
    /// Rust enters the run loop (`docs/design/execution.md`, "No re-entry from
    /// Rust"): a value needs no activation, and anything else is pushed and
    /// run by `vm::run`, which pops it and everything it activates on every
    /// path out.
    ///
    /// Three things are checked in a debug build, because each is otherwise
    /// silent. The loop is never entered while it is running -- nothing it
    /// calls can reach here. The stack is left as deep as it was found: a
    /// leaked entry would put a later activation on top of a dead one's
    /// registers (a leak inside a run is caught too, by
    /// `Heap::pop_activation`'s check that it pops the top). And no nested
    /// class's creation outlives the run that began it.
    fn run_from_host(&mut self, step: Step, send_args: &[Value]) -> EvalResult {
        debug_assert!(!self.running, "internal: the run loop was re-entered from Rust");
        let depth = self.heap.activation_depth();
        let result = match step {
            Step::Value(value) => Ok(value),
            step => match self.push_step(step, send_args, None, false) {
                Ok(act) => {
                    self.running = true;
                    let result = crate::vm::run(self, act);
                    self.running = false;
                    result
                }
                Err(signal) => Err(signal),
            },
        };
        debug_assert_eq!(
            self.heap.activation_depth(),
            depth,
            "a host's send or instantiation must leave the activation stack as it found it"
        );
        debug_assert!(
            self.nested_in_progress.is_empty(),
            "internal: a nested class's creation outlived the run that began it"
        );
        result
    }
```

Delete `reenter` with its doc comment, and `too_many_nested_native_calls` with
its doc comment and the "N's error." line; in `recursion_too_deep`'s doc comment
replace "Neither depth error names its limit" with "The error does not name its
limit" and "Both are kept out of line" with "It is kept out of line".

Add to `resume`'s match:

```rust
            Continuation::Instantiate(state) => self.advance_instantiation(state, value),
```

In `push_step`'s `Step::Method` arm nothing changes.

In `crates/ns-runtime/src/builtins/boolean.rs`, `block_step`'s unreachable arm
gains `| Continuation::Instantiate(_)`, or the match stops being exhaustive.

- [ ] **Step 8: Remove `run_clause_body`**

In `crates/ns-runtime/src/interpreter/nested.rs`, delete `run_clause_body` with
its doc comment, and drop `ActivationHeader`, `Heap` (if now unused) and
`EvalResult` from the imports — follow the compiler's unused-import warnings.

- [ ] **Step 9: Sweep the comments that name removed functions**

Each of these comments names a function this task removed; rewrite each to
name what does that job now (`descend_levels` builds and answers an
initializer level's activation; `climb_levels` walks up and checks each
level; `clause_message_body` checks the clause selector and finds the message
body; `advance_instantiation` unpacks the message's value with
`argument_list`; a clause's activation is a `Step::Body`, pushed by
`Interpreter::push_step`):

- `crates/ns-runtime/src/mixin.rs:118` and `:239`
- `crates/ns-runtime/src/bytecode.rs:158`
- `crates/ns-runtime/src/compiler/lower.rs:216` (`interpreter::nested::run_clause_body`
  → "the `Step::Body` `interpreter::nested::nested_class` answers")
- `crates/ns-runtime/src/image/decode.rs:155`, `:760`, `:800`, `:815`
- `crates/ns-runtime/tests/instantiate.rs:68`
- `crates/ns-runtime/tests/vm_initializer.rs:189`
- `crates/ns-runtime/tests/image.rs:3234`, `:4627`, `:4966`

Verify with the Grep tool that `reenter|run_clause_body|run_own_initializer|run_initializer_levels|eval_superclass_message|instantiate_unchecked|invoke_closure|call_block|native_depth|in_progress_key`
matches nothing under `crates/`, and `MAX_NATIVE_DEPTH` only
`crates/ns-cli/src/main.rs` (Task 4).

- [ ] **Step 10: Run the tests**

Run: `cargo test -p ns-runtime --lib deciding_new_answers` — PASS.
Run: `cargo test -p ns-runtime --test trampoline` — every test PASS.
Run: `cargo test -p ns-cli --test program_outcomes` — every test PASS.
Then the gates, all green — the image, instantiate, vm_initializer and
nested_classes suites are what pin the order of every instantiation effect.

- [ ] **Step 11: Measure**

The measurement protocol, against the commit this task started from.
`bench_new.ns` exercises the new path; no source may be slower.

- [ ] **Step 12: Commit**

`feat: run instantiation as trampoline steps, with no re-entry from Rust`,
with a body naming the golden change, the removal of `reenter` and N, and the
measurement; the trailer as in Global Constraints.

---

### Task 4: The interpreter thread's stack

**Files:**
- Modify: `crates/ns-cli/src/main.rs:176-183`

**Interfaces:** none.

- [ ] **Step 1: Re-derive the size**

With nothing re-entering the run loop, the Rust stack the interpreter thread
needs is what the front end and the compiler recurse on, and that is bounded by
the parser's nesting ceiling (`MAX_DEPTH = 128`, `crates/ns-syntax/src/parser/mod.rs`).
The test suite already runs the deepest nesting the parser accepts — through
parse, resolve, compile, `build`, `load` and a run —
`the_deepest_block_nesting_the_parser_accepts_builds_loads_and_runs` and
`the_deepest_literal_array_the_parser_accepts_survives_a_round_trip`
(`crates/ns-runtime/tests/image.rs`), on libtest's default 2 MiB threads in a
debug build (`.cargo/config.toml` sets no `RUST_MIN_STACK`). Confirm both pass:

Run: `cargo test -p ns-runtime --test image the_deepest`
Expected: PASS, 2 tests (or more, if more match the filter).

- [ ] **Step 2: Change the constant and its comment**

Replace lines 176-183 of `crates/ns-cli/src/main.rs` with:

```rust
/// The native stack the interpreter runs on. Nothing re-enters the run loop
/// from Rust (`docs/design/execution.md`, "No re-entry from Rust"), so what
/// recurses on it is the front end and the compiler, whose depth the parser's
/// nesting ceiling bounds. The test suite runs the deepest nesting the parser
/// accepts through every stage on libtest's default 2 MiB threads in a debug
/// build; this is four times that. Reserved, not committed: only what a
/// program uses is backed by memory. The thread itself is needed regardless,
/// because the symbol interner is thread-local.
const INTERPRETER_STACK_BYTES: usize = 8 * 1024 * 1024;
```

- [ ] **Step 3: Run the gates**

All green; the three changed goldens run the CLI binary on this stack.

- [ ] **Step 4: Commit**

`chore: size the interpreter thread for the front end, not for re-entry`, the
trailer as in Global Constraints.

---

### Task 5: The whole-step measurement and the documents

**Files:**
- Modify: `docs/design/execution.md`, `docs/design/memory.md`,
  `docs/design/core-library.md`, `docs/design/image-format.md`,
  `docs/design/compiler.md`, `docs/design/object-model.md`,
  `docs/design/README.md`, `CLAUDE.md`, `docs/PROGRESS.md`

Documents describe the current state in present tense (`docs/design/README.md`,
"How to read these documents"); the history stays in git and in this plan.

- [ ] **Step 1: Measure the whole step**

The measurement protocol, with `<parent-commit>` = `be3eb40` (the design
commit 2a started from; its code is `master`'s). Record the five medians per
source and binary, three rounds, and the suite's time (`cargo test
--workspace`, debug, test binaries already built, two runs) before and after.

- [ ] **Step 2: `execution.md`**

- The status block's **Code** line: "`interpreter/` (the send decisions,
  `Step`, every push, the depth limit, the host entry)"; add
  `builtins/collections.rs` beside `boolean.rs` and `block.rs` as a native
  that answers trampoline steps.
- "Representation", the push bullet: `Interpreter::push` is called from one
  place, `push_step`, for the method, block or body a decision answered;
  delete the sentence about the paths that re-enter the loop.
- The `Interpreter::send`/`instantiate` debug-assert sentence in the
  `ActivationRef` bullet: now `Interpreter::run_from_host`, the one host entry.
- "Signatures", second bullet: drop `invoke_closure` and `builtins::call_block`
  from the list.
- "The VM": the `Step` bullet names `Step::Body` beside `Step::Method` and
  `Step::Block`; the four kinds of body — a block body's item drops the `do:`
  sentence (`do:` answers its block as a step too); the initializer item
  becomes "`descend_levels`, one per initializing level … answered as a
  `Step::Body` with `Continuation::Instantiate`"; the clause item becomes "a
  `Step::Body` — the nested-class accessor's with `Continuation::NestedClass`,
  an instantiation's clause message with `Continuation::Instantiate`". Replace
  the paragraph "Rust enters the loop in one place, `Interpreter::reenter` …"
  with: Rust enters the loop in one place, `Interpreter::run_from_host`, for a
  host's `send` or `instantiate`, and only from outside it; no decision,
  native or continuation runs code from Rust, and a debug build checks that
  the loop is never entered while it runs.
- "Non-local return", second paragraph: replace "for the Rust frame that
  re-entered the loop to pass on: through `do:`'s native … which asks the same
  question." with "for the host that started the run". Add: the popped
  activations' continuations go to `Interpreter::abandon`, which takes a
  nested class's creation off `nested_in_progress`. In the initializer
  paragraph, "the run it belongs to pops it and leaves with the signal still
  naming it, each run below finds the target no longer live and passes the
  signal on" becomes "the unwind pops it with everything else down to the
  floor of the host's run and leaves with the signal".
- "What the trampoline moved", the paragraph about the interpreter thread's
  stack: replace with the derivation Task 4 wrote into `main.rs` (8 MiB, from
  the 2 MiB libtest threads the deepest-nesting tests run on).
- "The trampoline", first paragraph: "apart from the three paths "What stays
  recursive" below names, which re-enter it from Rust" becomes "and nothing
  re-enters it from Rust ("No re-entry from Rust" below)".
- In "Nor are the control-flow natives": "`do:` is still a callback; it becomes
  a step of the same kind in "Planned: no re-entry from Rust" below, and moves
  to Newspeak after the inline caches" becomes "`do:` answers its block as a
  step too ("No re-entry from Rust" below), and moves to Newspeak after the
  inline caches ([core-library.md](core-library.md))".
- Rename "Two bounds, not one" to **"The depth limit"** and rewrite it for one
  bound: the table has L's row only; delete the N paragraph, the "N is checked
  on every re-entry" bullet and the "Why not one count sized against the
  native stack" paragraph; the CLI-thread bullet says what Task 4 wrote; keep
  everything about L, its 129 × L factor, the exemption and why the limit is a
  count and not memory or a stack pointer. Update every link to the old
  heading — Grep `Two bounds, not one` across `docs/`, `CLAUDE.md` and
  `crates/` (compiler.md twice, image-format.md, README.md, and
  `MAX_ACTIVATION_DEPTH`'s doc comment, which Task 3 already changed).
- "A superclass chain is walked iteratively": `climb_levels` and
  `descend_levels`, a state machine resumed by `advance_instantiation`; the
  reasoning about a chain of code-free levels stays.
- "How other systems bound recursion depth", last paragraph: ns-next is now in
  the Primordial Soup and Cog position — nothing re-enters the interpreter from
  native code, so it needs no second bound; delete the sentences about paths
  that still recurse and their per-level native cost.
- Replace "What stays recursive, and why that is not a hole" and "Planned: no
  re-entry from Rust" by one section, **"No re-entry from Rust"**, placed where
  "What stays recursive" was, in present tense: what the planned section said,
  as built — the three steps with their continuations, `Step::Body` (boxed, and
  why), `nested_in_progress` recorded on push and removed by `resume` or
  `abandon`, `run_from_host`, what went, and the three golden changes with
  their reason, plus Task 5 Step 1's measurement table in "What the trampoline
  moved"'s form (under a new "### What removing the re-entries moved" in
  "Measurements", which this section points to). Fix every link to the two old
  headings (Grep `What stays recursive` and `no re-entry from Rust` across
  `docs/` and `crates/`). One claim of the planned text is corrected, not
  carried over: `vm::send` still takes the argument buffer and gives it back,
  because the decision needs `&mut Interpreter` while it reads the arguments;
  what went is only the second, fresh buffer a re-entering decision took.
- "Decisions": the two-bounds bullet becomes "**The control-flow natives are
  trampoline steps and depth has one bound**, L = 100,000 activations …"; the
  "Rust is not to re-enter the run loop" bullet becomes present tense
  ("**Rust does not re-enter the run loop** …").

- [ ] **Step 3: The other design documents**

- `memory.md`: in "No Rust frame is live below a safepoint", the four paths are
  history now — say that Rust enters the loop only as the host (step 2a, built),
  so at every point between two instructions no Rust frame below the loop
  holds a value; the safepoint that will use that is still step 2b. In "A
  handle is arena-relative", the identity keys are used in two places,
  `image/encode.rs` and `image/decode.rs`, six call sites — Grep
  `object_identity_key\|frame_identity_key` under `crates/ns-runtime/src` to
  confirm the count before writing it. In "What the collector traces",
  `nested_in_progress` holds `ObjRef`s (built). The status block and the
  planned section's opening say 2a is built.
- `core-library.md`: the `do:` table row — "A trampoline step: it answers the
  block for the first element, and its continuation activates it for each
  next one ([execution.md](execution.md), "No re-entry from Rust"); written in
  Newspeak after the inline caches". "Of the natives that run a block, only
  one still calls back, through `builtins::call_block`" and "So exactly one
  method still calls back, and it is the one that moves to Newspeak. It stops
  calling back before that: …" become: no native calls back into Newspeak;
  `do:` still moves to Newspeak after the caches, so that it becomes a real
  method lookup, mirrors and the debugger see. The "ordering matters"
  paragraph's last sentence: step 2a took the last callback away.
- `image-format.md`: line 34's "two counted limits, 100,000 activations and
  1,000 re-entries of the run loop from Rust" becomes the one limit; "Planned:
  suspend and resume" no longer lists the re-entry paths as an obstacle — the
  activation stack is the only copy of a running computation; what is missing
  is a yield path out of `vm::run` and a re-entry point for a decoded frame.
- `compiler.md`, "Where it is decided": "an initializer level's
  (`run_own_initializer`) and a superclass clause's or message's
  (`nested::run_clause_body`) each record `counted: true`" becomes "`push_step`'s
  body arm, for an initializer level, a superclass clause or its message,
  records `counted: true`".
- `object-model.md:189`: `nested::run_clause_body` → the clause runs as a
  `Step::Body` with `Continuation::NestedClass`, the same way instantiation runs
  the clause's deferred message.
- `README.md`: the map row for execution.md — "Built, including the trampoline;
  nothing re-enters the loop from Rust", planned next "closure conversion, JIT
  tiers"; the binary paragraph's "spawned with the stack the interpreter's
  native-depth bound needs" becomes "spawned with the stack the front end
  needs, because the symbol interner is thread-local".

- [ ] **Step 4: `CLAUDE.md`**

In "An image is untrusted input", replace "Recursion depth is bounded by two
counted limits -- 100,000 activations and 1,000 re-entries of the run loop
from Rust -- each an ordinary Newspeak error, not a native stack overflow (see
`docs/design/execution.md`, "Two bounds, not one")." with "Recursion depth is
bounded by a counted limit of 100,000 activations -- an ordinary Newspeak
error, not a native stack overflow, since nothing re-enters the run loop from
Rust (see `docs/design/execution.md`, "The depth limit")." Grep `CLAUDE.md` for
any other mention of re-entry or `reenter` and bring it up to date.

- [ ] **Step 5: `PROGRESS.md`**

- "Where the next session starts": 2a is done; **the next piece is 2b** — the
  safepoint, the tracing, host roots and the stress mode, to be planned against
  the code as 2a left it ([memory.md](design/memory.md), "Planned: checked roots
  and a collector").
- The M4 list: 2a "**Done.**", with its measured outcome in one sentence; 2b
  "**Next.**".
- The "Deferred past the milestone" paragraph and the "What M4 unlocks"
  paragraph: the re-entry paths are gone.
- A new section **"M4 — No re-entry from Rust (2026-09-13)"** above "M4 — The
  trampoline", in that section's form: what changed, the three golden changes
  and why, the measurement table, the suite's time, and a link to this plan.
- The M4 row of the milestone table gains a link to this plan.

- [ ] **Step 6: Verify the documents support a fresh session**

Read `CLAUDE.md`, `docs/design/README.md` and "Where the next session starts"
as a new session would, and check that they name 2b as next, point at its
design, and say nothing that is no longer true about re-entry, N, `do:` as a
callback or `reenter`. Grep `docs/design`, `CLAUDE.md` and `README.md` for
`reenter|invoke_closure|call_block|MAX_NATIVE_DEPTH|native calls|native-depth|re-enters the run loop`
— only historical mentions that say so explicitly may remain.

- [ ] **Step 7: Commit**

`docs: record that nothing re-enters the run loop from Rust`, the trailer as in
Global Constraints.
