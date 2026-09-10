# Activation Stack Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move every Newspeak activation off the `Heap`'s frame arena and onto
one activation stack the `Heap` owns, reifying a `Frame` only when a block is
created, so that an activation that creates no block allocates nothing and
leaves nothing behind.

**Architecture:** `heap.rs` gains an `ActivationStack` — one contiguous
`Vec<Value>` of registers and one `Vec<StackEntry>` — and a new handle,
`ActivationRef`, that names a live entry. `Heap::reify` creates an arena
`Frame` *married* to its entry (reads through the `FrameRef` reach the stack
slot); `Heap::pop_activation` *divorces* it, copying the registers out,
because ns-next's blocks read their method's variables by reference. The four
sites that build activations today (`invoke_method`, `invoke_closure`,
`run_initializer_level`, `run_clause_body`) push and pop instead, and every
function that means *the running activation* takes an `ActivationRef` instead
of a `FrameRef`. `FrameRef` remains the only activation handle anything may
store.

**Tech Stack:** Rust 1.98.0, edition 2021, no external dependencies. Three
crates: `ns-syntax` (untouched), `ns-runtime`, `ns-cli`.

**Spec:** [docs/design/execution.md](../../design/execution.md), "Planned:
activations on a stack, reified on demand" — the subsections "The model",
"Representation (1a)", "Lifecycle (1a)", "Signatures and the rest of the
runtime (1a)" and "What 1a leaves for 1b, and for later". Read them before
Task 1. Background: `prior-art/primordialsoup/vm/interpreter.cc`
(`EnsureActivation`, `FlushAllFrames`, `HasLivingFrame`).

## Scope

This is step **1a** of Milestone 4 as [docs/PROGRESS.md](../../PROGRESS.md)
numbers it: the stack itself. Deliberately **not** in this plan:

- **The trampoline (1b).** Execution stays recursive on the Rust stack.
  `Interpreter::send` keeps `args: Vec<Value>`, so the VM's `collect` vector
  survives. The VM does **not** cache an activation's register `base` in a
  local; that belongs to the loop 1b builds.
- **A recursion depth limit.** 1b.
- **Closure conversion.** Blocks keep reaching their method's variables through
  `captured_frame`; that is why a reified frame is divorced on pop.
- **The per-invocation `HashMap` lookup** of a compiled body. The inline caches
  remove it.

**One deviation from the spec's "Signatures" bullet, decided while planning:**
the spec says `Interpreter::send`'s, `invoke_method`'s, `dispatch_native`'s and
the builtins' `sender` parameters become `Option<ActivationRef>`. Once a live
activation's sender is the stack entry below it, **nothing reads those
parameters at all** — their only use today is to fill `Frame.sender`. Task 2
therefore removes them outright rather than retyping them, and Task 6 corrects
the spec's bullet.

**Two observable changes, both identical under both engines and both asked for
by the spec:**

1. A frame whose activation has returned has `sender = None`. Snapshots no
   longer carry their closures' callers.
2. A block created during initialization whose `^` runs after initialization
   has finished raises `cannotReturn` instead of escaping as "internal:
   non-local return escaped the entry send".

## Global Constraints

- Rust 1.98.0, edition 2021. **No new external dependency.**
- All code, identifiers, comments, doc comments and commit messages in English.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`,
  `refactor:`). **Every commit — including one made by a subagent — ends with
  exactly these two trailer lines:**
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01WPVgnoKxpths9NssEzm6TE
  ```
- Never override git author or committer identity.
- `cargo fmt --all` before every commit; `cargo clippy --workspace --all-targets`
  must be warning-free.
- Both corpus gates hold at every commit:
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`, and
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`.
- The two engines agree byte for byte on exit status, stdout **and** stderr;
  `crates/ns-cli/tests/differential.rs` enforces it with no exemptions. If they
  disagree, the tree-walker is the reference.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvalResult`.
  Tests unwrap through a local `expect_ok` helper — copy the one in a
  neighbouring test file.
- Object and frame access goes through `crates/ns-runtime/src/heap.rs`. Every
  field of the new types is private to that module.
- Never copy dispatch logic, lookup logic or error text out of `interpreter/`
  into `vm.rs`.
- Nothing under `crates/ns-syntax` changes.
- Never judge performance from a debug build.
- Design documents in `docs/design/` are living; this plan is an execution
  record — leave it as written once complete.

## File Structure

| File | Responsibility after this plan |
|---|---|
| `crates/ns-runtime/src/heap.rs` | Owns the object arena, the frame arena (`Vec<FrameSlot>`: a `Frame` plus the entry it is married to) and the `ActivationStack`. Defines `ActivationRef` and `ActivationHeader`. Every activation accessor is `activation_*`, every reified-frame accessor is `frame_*`. |
| `crates/ns-runtime/src/lib.rs` | Re-exports `ActivationHeader` and `ActivationRef` beside the other handles. |
| `crates/ns-runtime/src/types.rs` | `Frame` is documented as a *reified* activation; its fields do not change. |
| `crates/ns-runtime/src/interpreter/mod.rs` | `Signal::Return` carries an `ActivationRef`. `invoke_method` pushes and pops. `raise_return` resolves a block's home through its marriage. `send` and `instantiate` debug-assert that the stack is balanced. |
| `crates/ns-runtime/src/interpreter/eval.rs` | Every evaluator takes an `ActivationRef`. `invoke_closure` pushes and pops. `eval_block_literal` reifies. Outer locals go through `read_outer_local`/`write_outer_local`, shared with the VM. |
| `crates/ns-runtime/src/interpreter/instantiate.rs`, `nested.rs` | Initializer and clause activations push and pop. |
| `crates/ns-runtime/src/vm.rs` | `run_code` takes an `ActivationRef`; `MakeClosure` reifies. |
| `crates/ns-runtime/src/builtins/*.rs` | No `sender` parameter. |
| `crates/ns-cli/src/main.rs` | Calls `send` without a `sender`. |
| `tests-fixtures/closure_variables.ns`, `bench_send.ns`, `bench_block.ns` (+ `.expected`) | A correctness guard for block/method variable sharing, and two benchmarks that actually build activations. |
| `docs/design/*.md`, `docs/PROGRESS.md`, `CLAUDE.md` | Updated in Task 6 to describe the built state and the measurements. |

---

### Task 1: The activation stack in `heap.rs`

Purely additive: nothing outside `heap.rs` and `lib.rs` uses the new API yet,
so the suite stays green. The one non-additive change is internal — the frame
arena's element type becomes `FrameSlot`, so every existing frame accessor
reaches its `Frame` through `.frame`.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs`
- Modify: `crates/ns-runtime/src/lib.rs:21`
- Test: `crates/ns-runtime/src/heap.rs` (`mod tests`)

**Interfaces:**
- Produces (all on `Heap`, all `pub` unless noted):
  - `pub struct ActivationRef` — `Clone + Copy`, `same(&self, &ActivationRef) -> bool`, `Debug` printing `ActivationRef(#n)`.
  - `pub struct ActivationHeader { pub receiver: Value, pub current_class: Option<ClassRef>, pub kind: FrameKind, pub pc_kind: PcKind, pub slot_base: usize }`
  - `push_activation(&mut self, header: ActivationHeader, args: &[Value], temp_count: usize) -> ActivationRef`
  - `pop_activation(&mut self, act: &ActivationRef)`
  - `reify(&mut self, act: &ActivationRef) -> FrameRef`
  - `activation_depth(&self) -> usize`
  - `activation_local(&self, act: &ActivationRef, index: u16) -> Value`
  - `activation_set_local(&mut self, act: &ActivationRef, index: u16, value: Value)`
  - `activation_receiver(&self, act: &ActivationRef) -> Value`
  - `activation_receiver_and_current_class(&self, act: &ActivationRef) -> (Value, Option<ClassRef>)`
  - `activation_current_class(&self, act: &ActivationRef) -> Option<ClassRef>`
  - `activation_slot_base(&self, act: &ActivationRef) -> usize`
  - `set_activation_pc(&mut self, act: &ActivationRef, pc: usize)`
  - `activation_home(&self, act: &ActivationRef) -> Option<FrameRef>` — the closure's `home` for a closure activation, else `None`
  - `activation_captured(&self, act: &ActivationRef) -> Option<FrameRef>` — the closure's `captured_frame` for a closure activation, else `None`
  - `activation_is_method(&self, act: &ActivationRef) -> bool`
  - `grow_activation_temps(&mut self, act: &ActivationRef, needed: usize)` — top activation only
  - `frame_local(&self, frame: &FrameRef, index: u16) -> Value` — reads the stack slot while married
  - `frame_set_local(&mut self, frame: &FrameRef, index: u16, value: Value)` — writes the stack slot while married
  - `frame_activation(&self, frame: &FrameRef) -> Option<ActivationRef>` — the live entry a frame is married to
- The old `local`, `set_local`, `receiver`, `receiver_and_current_class`,
  `grow_temps`, `frame_slot_base`, `frame_is_method`, `mark_uncontinuable`
  (all on `FrameRef`) stay untouched in this task; Task 4 removes or renames
  them.

- [ ] **Step 1: Write the failing tests**

Add to the end of `crates/ns-runtime/src/heap.rs`'s `mod tests` (before the
final `}`):

```rust
    /// The header every activation-stack test below pushes: nothing in it is
    /// read back except where a test sets it on purpose.
    fn header() -> ActivationHeader {
        ActivationHeader {
            receiver: Value::Nil,
            current_class: None,
            kind: FrameKind::TopLevel,
            pc_kind: PcKind::Bytecode,
            slot_base: 0,
        }
    }

    #[test]
    fn an_activation_addresses_its_arguments_before_its_temporaries() {
        let mut heap = Heap::new();
        let act = heap.push_activation(header(), &[Value::SmallInt(1), Value::SmallInt(2)], 2);
        assert!(matches!(heap.activation_local(&act, 0), Value::SmallInt(1)));
        assert!(matches!(heap.activation_local(&act, 1), Value::SmallInt(2)));
        assert!(matches!(heap.activation_local(&act, 2), Value::Nil));
        heap.activation_set_local(&act, 3, Value::SmallInt(9));
        assert!(matches!(heap.activation_local(&act, 3), Value::SmallInt(9)));
        heap.pop_activation(&act);
    }

    /// Two live activations share one `values` vector; neither may see the
    /// other's registers, whichever is written first.
    #[test]
    fn nested_activations_keep_separate_register_windows() {
        let mut heap = Heap::new();
        let outer = heap.push_activation(header(), &[Value::SmallInt(1)], 1);
        let inner = heap.push_activation(header(), &[Value::SmallInt(2)], 1);
        heap.activation_set_local(&inner, 1, Value::SmallInt(20));
        heap.activation_set_local(&outer, 1, Value::SmallInt(10));
        assert!(matches!(heap.activation_local(&outer, 0), Value::SmallInt(1)));
        assert!(matches!(heap.activation_local(&outer, 1), Value::SmallInt(10)));
        assert!(matches!(heap.activation_local(&inner, 0), Value::SmallInt(2)));
        assert!(matches!(heap.activation_local(&inner, 1), Value::SmallInt(20)));
        assert_eq!(heap.activation_depth(), 2);
        heap.pop_activation(&inner);
        heap.pop_activation(&outer);
        assert_eq!(heap.activation_depth(), 0);
    }

    /// The property the whole step exists for: an activation nothing
    /// reified is gone once it is popped. The arena used to keep every one.
    #[test]
    fn an_activation_that_is_never_reified_leaves_nothing_behind() {
        let mut heap = Heap::new();
        for _ in 0..1000 {
            let act = heap.push_activation(header(), &[Value::SmallInt(1)], 3);
            heap.pop_activation(&act);
        }
        assert_eq!(heap.frames.len(), 0);
        assert_eq!(heap.stack.values.len(), 0);
        assert_eq!(heap.activation_depth(), 0);
    }

    #[test]
    fn reifying_an_activation_twice_answers_the_same_frame() {
        let mut heap = Heap::new();
        let act = heap.push_activation(header(), &[], 1);
        let first = heap.reify(&act);
        let second = heap.reify(&act);
        assert!(first.same(&second));
        assert_eq!(heap.frames.len(), 1);
        heap.pop_activation(&act);
    }

    /// While its activation runs, a reified frame is a view of the live
    /// registers, in both directions: a block writing its method's variable
    /// through the frame must be seen by the method, and vice versa.
    #[test]
    fn a_married_frame_reads_and_writes_the_live_registers() {
        let mut heap = Heap::new();
        let act = heap.push_activation(header(), &[Value::SmallInt(1)], 1);
        let frame = heap.reify(&act);
        heap.activation_set_local(&act, 1, Value::SmallInt(5));
        assert!(matches!(heap.frame_local(&frame, 1), Value::SmallInt(5)));
        heap.frame_set_local(&frame, 1, Value::SmallInt(7));
        assert!(matches!(heap.activation_local(&act, 1), Value::SmallInt(7)));
        assert!(heap
            .frame_activation(&frame)
            .is_some_and(|live| live.same(&act)));
        heap.pop_activation(&act);
    }

    /// Popping a reified activation hands its registers to the frame, which
    /// a surviving block goes on reading and writing; the frame is dead from
    /// then on, and a dead activation has no sender.
    #[test]
    fn popping_a_reified_activation_divorces_its_frame() {
        let mut heap = Heap::new();
        let act = heap.push_activation(header(), &[Value::SmallInt(1)], 1);
        heap.set_activation_pc(&act, 4);
        let frame = heap.reify(&act);
        // Written after reification: it must survive the divorce.
        heap.activation_set_local(&act, 1, Value::SmallInt(3));
        heap.pop_activation(&act);

        assert!(heap.frame_activation(&frame).is_none());
        assert!(matches!(heap.frame_local(&frame, 0), Value::SmallInt(1)));
        assert!(matches!(heap.frame_local(&frame, 1), Value::SmallInt(3)));
        assert_eq!(heap.frame_register_shape(&frame), (4, 1, 1));
        assert!(heap.frame_is_uncontinuable(&frame));
        assert!(heap.frame_sender(&frame).is_none());
        heap.frame_set_local(&frame, 1, Value::SmallInt(8));
        assert!(matches!(heap.frame_local(&frame, 1), Value::SmallInt(8)));
    }

    /// The next activation at the same depth reuses the stack slots. A
    /// divorced frame owns its own copy, so that reuse cannot reach it.
    #[test]
    fn a_divorced_frame_is_untouched_by_the_next_activation_at_its_depth() {
        let mut heap = Heap::new();
        let first = heap.push_activation(header(), &[], 2);
        heap.activation_set_local(&first, 1, Value::SmallInt(3));
        let frame = heap.reify(&first);
        heap.pop_activation(&first);

        let second = heap.push_activation(header(), &[], 2);
        heap.activation_set_local(&second, 1, Value::SmallInt(99));
        assert!(matches!(heap.frame_local(&frame, 1), Value::SmallInt(3)));
        heap.pop_activation(&second);
    }

    /// An `ActivationRef` kept past its pop names whatever activation reuses
    /// the index next -- a different, valid activation, so the read would
    /// silently answer the wrong thing. Debug builds refuse it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale activation handle")]
    fn reading_through_a_popped_activation_handle_panics() {
        let mut heap = Heap::new();
        let first = heap.push_activation(header(), &[], 1);
        heap.pop_activation(&first);
        let _second = heap.push_activation(header(), &[], 1);
        let _ = heap.activation_local(&first, 0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "handle belongs to a different heap")]
    fn reading_an_activation_handle_through_a_foreign_heap_panics() {
        let mut origin = Heap::new();
        let mine = origin.push_activation(header(), &[], 1);
        let mut other = Heap::new();
        let _theirs = other.push_activation(header(), &[], 1);
        let _ = other.activation_local(&mine, 0);
    }

    /// Popping below the top would truncate the registers of every
    /// activation above it while their handles still look valid.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "only the top activation")]
    fn popping_an_activation_that_is_not_on_top_panics() {
        let mut heap = Heap::new();
        let outer = heap.push_activation(header(), &[], 1);
        let _inner = heap.push_activation(header(), &[], 1);
        heap.pop_activation(&outer);
    }
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p ns-runtime --lib heap::tests`
Expected: compile errors — `ActivationHeader`, `push_activation` and the rest
do not exist.

- [ ] **Step 3: Implement**

In `crates/ns-runtime/src/heap.rs`:

(a) After `impl std::fmt::Debug for FrameRef { ... }`, add the new handle,
the header, and the private stack types:

```rust
/// An opaque handle to a *live* activation: an entry on the `Heap`'s
/// activation stack. Where a `FrameRef` names a reified activation that may
/// be stored anywhere, an `ActivationRef` is valid only until its entry is
/// popped -- the next activation at that depth reuses the index. That is
/// why nothing stores one: `ClosureData`, `Frame` and every image record
/// take a `FrameRef`, which `Heap::reify` answers for a live activation.
/// See `docs/design/execution.md`, "Representation (1a)".
///
/// 4 bytes in a release build. A debug build adds the heap's id, like every
/// handle, and a `generation` drawn at each push, so that reading through a
/// handle whose entry was popped panics instead of answering for whichever
/// activation reuses the index.
#[derive(Clone, Copy)]
pub struct ActivationRef {
    index: u32,
    #[cfg(debug_assertions)]
    origin: u32,
    #[cfg(debug_assertions)]
    generation: u32,
}

impl ActivationRef {
    /// Activation identity: `invoke_method` and `run_clause_body` catch a
    /// `Signal::Return` by it. Two handles at the same index are the same
    /// activation because a return only ever unwinds downward -- nothing is
    /// pushed while a `Signal::Return` propagates, so the target's index
    /// cannot have been reused by the time a catcher compares it. A debug
    /// build checks that rather than assuming it.
    pub fn same(&self, other: &ActivationRef) -> bool {
        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                self.origin, other.origin,
                "handle belongs to a different heap (comparing handle from heap {}, with handle from heap {})",
                self.origin, other.origin
            );
            debug_assert!(
                self.index != other.index || self.generation == other.generation,
                "stale activation handle: compared against a later activation at the same depth"
            );
        }
        self.index == other.index
    }
}

/// Prints the stack index and nothing else, for the reason `FrameRef`'s does.
impl std::fmt::Debug for ActivationRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ActivationRef(#{})", self.index)
    }
}

/// Everything a new activation is apart from its registers, which
/// `Heap::push_activation` takes separately.
pub struct ActivationHeader {
    pub receiver: Value,
    /// `None` only for `FrameKind::TopLevel` (`newspeak-spec.pdf` §3.6).
    pub current_class: Option<ClassRef>,
    pub kind: FrameKind,
    pub pc_kind: PcKind,
    /// See `Frame::slot_base`.
    pub slot_base: usize,
}

/// One live activation. Its registers are `values[base..]` up to the next
/// entry's `base` (or the end of `values`, for the top entry): the
/// arguments first, then the temporaries.
struct StackEntry {
    receiver: Value,
    current_class: Option<ClassRef>,
    kind: FrameKind,
    base: u32,
    num_args: u32,
    pc: usize,
    pc_kind: PcKind,
    slot_base: usize,
    /// The frame `Heap::reify` created for this activation, married to it
    /// until the entry is popped.
    reified: Option<FrameRef>,
    #[cfg(debug_assertions)]
    generation: u32,
}

/// Every live activation, innermost last. Both vectors keep their capacity
/// across pops, so once they have grown to the program's depth a push
/// allocates nothing.
#[derive(Default)]
struct ActivationStack {
    values: Vec<Value>,
    entries: Vec<StackEntry>,
    #[cfg(debug_assertions)]
    next_generation: u32,
}

/// A reified activation and, while that activation is still running, the
/// stack entry it is married to. While `live` is set, the frame's own
/// `args`/`temps` are empty and its registers are the entry's.
struct FrameSlot {
    frame: Frame,
    live: Option<u32>,
}
```

(b) In `pub struct Heap`, change the `frames` field and add the stack. Replace

```rust
    frames: Vec<Frame>,
```

with

```rust
    frames: Vec<FrameSlot>,
    /// Every live activation. A stack entry is popped when its activation
    /// returns, so -- unlike `frames` and `objects` -- this is bounded by
    /// the deepest the program has been, not by everything it has run.
    stack: ActivationStack,
```

and rewrite the doc comment above `frames` to:

```rust
    /// Every *reified* activation, addressed by a `FrameRef`'s index: one
    /// `Heap::reify` created because a block captured it, or one the image
    /// decoder rebuilt. Nothing is ever removed: reclaiming a slot needs a
    /// collector. An activation that is never reified never reaches this
    /// arena at all.
```

In `impl std::fmt::Debug for Heap`, add after the `frames.len()` field:

```rust
            .field("activations", &self.stack.entries.len())
```

In `Heap::new`, add `stack: ActivationStack::default(),` after
`frames: Vec::new(),`.

(c) `alloc_frame`: push a `FrameSlot` instead of a `Frame`:

```rust
        self.frames.push(FrameSlot { frame, live: None });
```

(d) Every existing frame accessor below `// ---------- Frame accessors` that
indexes `self.frames[...]` now reaches the `Frame` through `.frame`. Apply this
mechanically — for example `self.frames[frame.index as usize].receiver`
becomes `self.frames[frame.index as usize].frame.receiver`,
`let f = &self.frames[frame.index as usize];` becomes
`let f = &self.frames[frame.index as usize].frame;`, and in `replace_frame`
`self.frames[frame.index as usize] = filled;` becomes
`self.frames[frame.index as usize].frame = filled;`. `frame_snapshot` becomes
`self.frames[frame.index as usize].frame.clone()`. Do not change any accessor's
signature or behaviour in this task.

(e) Add the check and the handle constructor next to `check_frame_origin`:

```rust
    /// `check_frame_origin` for a live activation, plus the check only a
    /// stack handle needs: that its entry is still the one it was minted
    /// for. Compiles to nothing in a release build.
    #[inline]
    fn check_activation(&self, act: &ActivationRef) {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                act.origin, self.heap_id,
                "handle belongs to a different heap (handle from heap {}, read through heap {})",
                act.origin, self.heap_id
            );
            let live = self
                .stack
                .entries
                .get(act.index as usize)
                .map(|entry| entry.generation);
            assert_eq!(
                live,
                Some(act.generation),
                "stale activation handle: its activation has returned"
            );
        }
        let _ = act;
    }

    /// The handle for live entry `index`, minted with that entry's own
    /// generation. The one place an `ActivationRef` is built.
    fn activation_handle(&self, index: u32) -> ActivationRef {
        ActivationRef {
            index,
            #[cfg(debug_assertions)]
            origin: self.heap_id,
            #[cfg(debug_assertions)]
            generation: self.stack.entries[index as usize].generation,
        }
    }

    /// Where register `index` of live entry `entry` sits in `values`. The
    /// window check is what stops an out-of-range index from silently reading
    /// the neighbouring activation's register, which is in bounds of
    /// `values` and so would not panic on its own.
    fn register_slot(&self, entry: u32, index: u16) -> usize {
        let entries = &self.stack.entries;
        let slot = entries[entry as usize].base as usize + index as usize;
        debug_assert!(
            slot < entries
                .get(entry as usize + 1)
                .map_or(self.stack.values.len(), |next| next.base as usize),
            "register {index} is outside its activation's register window"
        );
        slot
    }
```

(f) Add a new section after the frame accessors (before `#[cfg(test)]`'s
closing `impl` brace):

```rust
    // ---------- The activation stack ----------
    //
    // A live activation is a `StackEntry` plus its window of `values`.
    // `reify` marries an arena `Frame` to an entry when something must hold
    // the activation past this call -- today, only a block being created --
    // and `pop_activation` divorces it again. See
    // `docs/design/execution.md`, "Lifecycle (1a)".

    /// Pushes an activation whose registers are `args` followed by
    /// `temp_count` nils.
    pub fn push_activation(
        &mut self,
        header: ActivationHeader,
        args: &[Value],
        temp_count: usize,
    ) -> ActivationRef {
        let base = u32::try_from(self.stack.values.len())
            .expect("more than u32::MAX registers live at once in one heap");
        let num_args =
            u32::try_from(args.len()).expect("more than u32::MAX arguments to one activation");
        let index = u32::try_from(self.stack.entries.len())
            .expect("more than u32::MAX live activations in one heap");
        self.stack.values.extend_from_slice(args);
        let end = self.stack.values.len() + temp_count;
        self.stack.values.resize(end, Value::Nil);
        #[cfg(debug_assertions)]
        let generation = {
            self.stack.next_generation = self.stack.next_generation.wrapping_add(1);
            self.stack.next_generation
        };
        self.stack.entries.push(StackEntry {
            receiver: header.receiver,
            current_class: header.current_class,
            kind: header.kind,
            base,
            num_args,
            pc: 0,
            pc_kind: header.pc_kind,
            slot_base: header.slot_base,
            reified: None,
            #[cfg(debug_assertions)]
            generation,
        });
        self.activation_handle(index)
    }

    /// Pops the top activation. If it was reified, its frame is divorced:
    /// the registers move into the frame, which is dead from then on --
    /// uncontinuable, and with no sender, as a returned Smalltalk context
    /// has none.
    pub fn pop_activation(&mut self, act: &ActivationRef) {
        self.check_activation(act);
        debug_assert_eq!(
            act.index as usize + 1,
            self.stack.entries.len(),
            "only the top activation can be popped"
        );
        let entry = self
            .stack
            .entries
            .pop()
            .expect("an activation handle names a live entry");
        let base = entry.base as usize;
        if let Some(frame) = entry.reified {
            let registers = &self.stack.values[base..];
            let split = entry.num_args as usize;
            let slot = &mut self.frames[frame.index as usize];
            slot.frame.args = registers[..split].to_vec();
            slot.frame.temps = registers[split..].to_vec();
            slot.frame.pc = entry.pc;
            slot.frame.sender = None;
            slot.frame.uncontinuable = true;
            slot.live = None;
        }
        self.stack.values.truncate(base);
    }

    /// The arena frame for a live activation, created married to it on the
    /// first call and answered again on every later one.
    pub fn reify(&mut self, act: &ActivationRef) -> FrameRef {
        self.check_activation(act);
        let i = act.index as usize;
        if let Some(frame) = self.stack.entries[i].reified {
            return frame;
        }
        let entry = &self.stack.entries[i];
        let frame = Frame {
            receiver: entry.receiver,
            args: Vec::new(),
            temps: Vec::new(),
            pc: entry.pc,
            pc_kind: entry.pc_kind,
            sender: None,
            home: match &entry.kind {
                FrameKind::Closure(closure) => Some(closure.home),
                _ => None,
            },
            current_class: entry.current_class,
            kind: entry.kind.clone(),
            uncontinuable: false,
            slot_base: entry.slot_base,
        };
        let handle = self.alloc_frame(frame);
        self.frames[handle.index as usize].live = Some(act.index);
        self.stack.entries[i].reified = Some(handle);
        handle
    }

    /// How many activations are live. `Interpreter::send` asserts that a
    /// send leaves it where it found it.
    pub fn activation_depth(&self) -> usize {
        self.stack.entries.len()
    }

    pub fn activation_local(&self, act: &ActivationRef, index: u16) -> Value {
        self.check_activation(act);
        self.stack.values[self.register_slot(act.index, index)]
    }

    pub fn activation_set_local(&mut self, act: &ActivationRef, index: u16, value: Value) {
        self.check_activation(act);
        let slot = self.register_slot(act.index, index);
        self.stack.values[slot] = value;
    }

    pub fn activation_receiver(&self, act: &ActivationRef) -> Value {
        self.check_activation(act);
        self.stack.entries[act.index as usize].receiver
    }

    /// The receiver and `current_class` together. `dispatch_lexical_send`'s
    /// head needs both on every implicit-receiver send; fetching them
    /// separately was once a measured 13% regression (see
    /// `docs/design/execution.md`'s measurements section).
    pub fn activation_receiver_and_current_class(
        &self,
        act: &ActivationRef,
    ) -> (Value, Option<ClassRef>) {
        self.check_activation(act);
        let entry = &self.stack.entries[act.index as usize];
        (entry.receiver, entry.current_class)
    }

    pub fn activation_current_class(&self, act: &ActivationRef) -> Option<ClassRef> {
        self.check_activation(act);
        self.stack.entries[act.index as usize].current_class
    }

    /// See `Frame::slot_base`.
    pub fn activation_slot_base(&self, act: &ActivationRef) -> usize {
        self.check_activation(act);
        self.stack.entries[act.index as usize].slot_base
    }

    pub fn set_activation_pc(&mut self, act: &ActivationRef, pc: usize) {
        self.check_activation(act);
        self.stack.entries[act.index as usize].pc = pc;
    }

    /// A closure activation's return target -- its closure's `home` -- and
    /// `None` for every other kind, which is its own return target.
    pub fn activation_home(&self, act: &ActivationRef) -> Option<FrameRef> {
        self.check_activation(act);
        match &self.stack.entries[act.index as usize].kind {
            FrameKind::Closure(closure) => Some(closure.home),
            _ => None,
        }
    }

    /// The frame a closure activation's closure captured, or `None` for any
    /// other kind. The first hop of `eval::frame_at_depth`.
    pub fn activation_captured(&self, act: &ActivationRef) -> Option<FrameRef> {
        self.check_activation(act);
        match &self.stack.entries[act.index as usize].kind {
            FrameKind::Closure(closure) => Some(closure.captured_frame),
            _ => None,
        }
    }

    /// Whether the activation runs a method body -- `run_statements` needs
    /// it to decide what falling off the end returns: `self` for a method
    /// (spec §3.5), the last statement's value for anything else.
    pub fn activation_is_method(&self, act: &ActivationRef) -> bool {
        self.check_activation(act);
        matches!(
            self.stack.entries[act.index as usize].kind,
            FrameKind::Method(_)
        )
    }

    /// Grows the top activation's temporaries to `needed`, filling with
    /// nil. The initializer path builds its activation before it knows the
    /// compiled body's register count. Only the top entry can grow: any
    /// other's window ends where the next one's begins.
    pub fn grow_activation_temps(&mut self, act: &ActivationRef, needed: usize) {
        self.check_activation(act);
        debug_assert_eq!(
            act.index as usize + 1,
            self.stack.entries.len(),
            "only the top activation's register file can grow"
        );
        let entry = &self.stack.entries[act.index as usize];
        let end = entry.base as usize + entry.num_args as usize + needed;
        if self.stack.values.len() < end {
            self.stack.values.resize(end, Value::Nil);
        }
    }

    /// A register of a reified frame: the live stack slot while its
    /// activation runs, the frame's own copy after it has returned. This is
    /// the path a block takes to its method's variables, and the only
    /// register access that branches.
    pub fn frame_local(&self, frame: &FrameRef, index: u16) -> Value {
        self.check_frame_origin(frame);
        let slot = &self.frames[frame.index as usize];
        match slot.live {
            Some(entry) => self.stack.values[self.register_slot(entry, index)],
            None => {
                let f = &slot.frame;
                let i = index as usize;
                if i < f.args.len() {
                    f.args[i]
                } else {
                    f.temps[i - f.args.len()]
                }
            }
        }
    }

    pub fn frame_set_local(&mut self, frame: &FrameRef, index: u16, value: Value) {
        self.check_frame_origin(frame);
        // Copied out first, so neither arm below holds a borrow of `frames`
        // while it writes.
        let live = self.frames[frame.index as usize].live;
        match live {
            Some(entry) => {
                let slot = self.register_slot(entry, index);
                self.stack.values[slot] = value;
            }
            None => {
                let f = &mut self.frames[frame.index as usize].frame;
                let i = index as usize;
                if i < f.args.len() {
                    f.args[i] = value;
                } else {
                    let temp_index = i - f.args.len();
                    f.temps[temp_index] = value;
                }
            }
        }
    }

    /// The live activation `frame` is married to, or `None` once that
    /// activation has returned -- and for every frame the image decoder
    /// built, since none of those ever ran here. `raise_return` uses it to
    /// tell a live home from a dead one.
    pub fn frame_activation(&self, frame: &FrameRef) -> Option<ActivationRef> {
        self.check_frame_origin(frame);
        self.frames[frame.index as usize]
            .live
            .map(|entry| self.activation_handle(entry))
    }
```

(g) In `crates/ns-runtime/src/lib.rs`, replace

```rust
pub use heap::{ClassRef, FrameRef, Heap, ObjRef};
```

with

```rust
pub use heap::{ActivationHeader, ActivationRef, ClassRef, FrameRef, Heap, ObjRef};
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p ns-runtime --lib heap::tests`
Expected: every test in `heap::tests` passes, the eleven new ones included.

Then the whole suite, to confirm the `FrameSlot` change moved nothing:
Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 5: Lint, format, gates, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add crates/ns-runtime/src/heap.rs crates/ns-runtime/src/lib.rs
git commit -F - <<'EOF'
feat: add an activation stack with on-demand reification to Heap

An ActivationRef names a live entry on one stack of registers the Heap
owns; reify marries an arena Frame to an entry and pop_activation
divorces it, copying the registers out. Nothing uses it yet.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WPVgnoKxpths9NssEzm6TE
EOF
```

Clippy must report no warnings; the gates must print
`parsed 210, failed 0, expected-fail 1, stale 0` and `ran 6, failed 0, skipped 205`.

---

### Task 2: Stop threading `sender`

A pure refactor with one observable consequence. Today a `sender` handle is
passed from every send down through `invoke_method`, `dispatch_native`, the
block-invoking builtins and `invoke_closure`, and its only use is to fill
`Frame.sender`. From Task 4 on, a live activation's sender is the stack entry
below it, and a returned one has none (spec, "Lifecycle (1a)" → "Sender"), so
the parameter has no reader. Removing it here, while frames still live in the
arena, keeps Task 4 about the stack alone.

The observable consequence: every frame the runtime builds now has
`sender: None`, so a frame in a snapshot no longer drags its caller chain in.
Two tests pin the old behaviour and change with it.

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`send`, `dispatch_native_inheriting_object`, `dispatch_native_or_dnu`, `does_not_understand`, `invoke_method`)
- Modify: `crates/ns-runtime/src/interpreter/eval.rs` (`eval_send`, `eval_cascade`, `dispatch_lexical_send`, `dispatch_super_send`, `invoke_closure`)
- Modify: `crates/ns-runtime/src/builtins/mod.rs`, `block.rs`, `boolean.rs`, `collections.rs`
- Modify: `crates/ns-runtime/src/vm.rs` (the three `Send*` arms)
- Modify: `crates/ns-cli/src/main.rs` (every `interp.send(…)` call)
- Modify: every test under `crates/ns-runtime/tests/` that calls `send` or `invoke_closure`
- Test: `crates/ns-runtime/tests/image.rs` (`a_closure_and_its_paused_frame_chain_round_trip`)
- Test: `crates/ns-runtime/tests/builtins_collections_block.rs` (`an_invoked_blocks_frame_records_its_sender`, deleted)

**Interfaces:**
- Produces (new signatures; later tasks call these):
  - `Interpreter::send(&mut self, receiver: Value, selector: Symbol, args: Vec<Value>, bindings: &Rc<HashMap<Span, Binding>>) -> EvalResult`
  - `Interpreter::invoke_method(&mut self, receiver: Value, method: Rc<MixinMethod>, defining_class: ClassRef, args: Vec<Value>, bindings: Rc<HashMap<Span, Binding>>) -> EvalResult`
  - `eval::invoke_closure(interp: &mut Interpreter, closure: &Rc<ClosureData>, args: Vec<Value>, bindings: &Rc<HashMap<Span, Binding>>) -> EvalResult`
  - `builtins::dispatch_native(interp: &mut Interpreter, kind: BuiltinClass, receiver: Value, selector: Symbol, args: &[Value]) -> Option<EvalResult>`
  - `builtins::call_block(interp: &mut Interpreter, block: &Value, args: Vec<Value>) -> EvalResult` (`pub(super)`)

- [ ] **Step 1: Change the two tests that pin the old behaviour**

In `crates/ns-runtime/tests/image.rs`, rename
`a_closure_and_its_paused_frame_chain_round_trip` to
`a_closure_and_its_paused_home_frame_round_trip`, and replace its last block —
from `let sender = home.sender.as_ref().expect("`mk` was sent from `outer`");`
through `assert!(Rc::ptr_eq(outer, &methods[&Symbol::intern("outer")]));` —
with:

```rust
    assert!(
        home.sender.is_none(),
        "a returned activation has no sender -- Smalltalk's rule, and what keeps \
         a surviving block from retaining its method's whole call chain"
    );
```

Keep everything above it, including the `methods` lookup the `mk` assertion
uses.

In `crates/ns-runtime/tests/builtins_collections_block.rs`, delete
`an_invoked_blocks_frame_records_its_sender` together with its doc comment.
Its property — the sender chain is not cut at a block boundary — becomes
structural in Task 4 (a live activation's sender is the entry below it), so
there is no stored field left to forget. Remove any import the deletion leaves
unused.

- [ ] **Step 2: Run them to confirm the image test fails**

Run: `cargo test -p ns-runtime --test image a_closure_and_its_paused_home_frame_round_trip`
Expected: FAIL — `home.sender` is still `Some`.

- [ ] **Step 3: Remove the parameter**

`crates/ns-runtime/src/interpreter/mod.rs`:
- `send`: drop the `sender: Option<FrameRef>` parameter and every
  `sender` argument it passes on (to `invoke_method`, `dispatch_native_or_dnu`,
  `does_not_understand`).
- `dispatch_native_inheriting_object`: drop `sender: Option<&FrameRef>` and
  the `sender` argument of both `crate::builtins::dispatch_native` calls.
- `dispatch_native_or_dnu`, `does_not_understand`: drop `sender` and pass it
  on nowhere.
- `invoke_method`: drop `sender: Option<FrameRef>`; in the `Frame { … }`
  literal write `sender: None,` with the comment
  `// A live activation's sender is structural (the activation stack, from M4 step 1a on); a stored one is never read.`

`crates/ns-runtime/src/interpreter/eval.rs`:
- `eval_send`, `eval_cascade`: `interp.send(receiver, selector, args, bindings)`.
- `dispatch_lexical_send`, `dispatch_super_send`: drop `Some(*frame)` from
  `invoke_method` and `send` calls, and the `Some(frame)` argument of
  `crate::builtins::dispatch_native`.
- `invoke_closure`: drop `sender: Option<FrameRef>`, write `sender: None,` in
  the `Frame` literal, and replace the doc paragraph that begins "`sender` is
  the activation performing the invocation" with: "The invoking activation is
  not passed: a live activation's sender is the one below it on the
  activation stack."

`crates/ns-runtime/src/builtins/mod.rs`: new signatures

```rust
pub fn dispatch_native(
    interp: &mut Interpreter,
    kind: BuiltinClass,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<EvalResult> {
```

```rust
pub(super) fn call_block(interp: &mut Interpreter, block: &Value, args: Vec<Value>) -> EvalResult {
```

with `crate::interpreter::eval::invoke_closure(interp, &closure, args, &bindings)`
inside. Delete the doc paragraphs about `sender` on both functions (the one
starting "`sender` is the activation this native dispatch is running on
behalf of" and the one starting "`sender` -- the activation invoking the
block").

`block.rs`, `boolean.rs`, `collections.rs`: drop `sender: Option<&FrameRef>`
from `dispatch`, `run_while`, `truthy_arg`, `dispatch_common`,
`dispatch_array` and `dispatch_ordered_collection`, and every `sender`
argument. `run_while`'s `mustBeBoolean` send becomes
`interp.send(other, Symbol::intern("mustBeBoolean"), vec![], bindings)`.
Remove `FrameRef` from any import that no longer needs it.

`crates/ns-runtime/src/vm.rs`:
`interp.send(receiver, *selector, args, bindings)` in `SendPublic`.

`crates/ns-runtime/src/interpreter/instantiate.rs` and `nested.rs` already
write `sender: None`; leave them.

`crates/ns-cli/src/main.rs` and every test: delete the trailing `None`
argument of each `send(…)` and `invoke_closure(…)` call. There are about 140,
almost all in the form

```rust
        interp.send(
            receiver,
            Symbol::intern("m"),
            vec![],
            &resolved.bindings,
            None,
        ),
```

which becomes

```rust
        interp.send(receiver, Symbol::intern("m"), vec![], &resolved.bindings),
```

`cargo build --workspace --all-targets` lists every remaining site as an
argument-count error; `cargo fmt --all` re-flows them. Do not change anything
else in a test while doing this.

- [ ] **Step 4: Run the suite**

Run: `cargo test --workspace`
Expected: all green, `a_closure_and_its_paused_home_frame_round_trip` included.

If another test fails because it counted frame records, or compared
`inspect` output, in an image that used to include a caller frame reachable
only through `sender`, update that expectation to the new count and name the
reason in the commit message. Any other failure is a mistake in this task —
stop and report it rather than adjusting the test.

- [ ] **Step 5: Lint, format, gates, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add -A crates
git commit -F - <<'EOF'
refactor: stop threading a sender through sends and block invocation

Its only use was to fill Frame.sender. Once activations live on a stack
a live activation's sender is the entry below it and a returned one has
none, so the parameter has no reader. Frames the runtime builds now
carry no sender, and a snapshot no longer drags a closure's callers in.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WPVgnoKxpths9NssEzm6TE
EOF
```

---

### Task 3: A fixture that guards block/method variable sharing

A characterization test: it passes on the code as it stands. Its job is to
fail loudly in Task 4 if the married-frame redirect, the divorce copy or the
outer-local walk is wrong — each of which would otherwise be silent wrong
output with exit status 0. It covers the four shapes that matter:

- a block writing its method's variable **while the method runs** (`sumOf:`);
  the method must see the write — this is the married redirect;
- nested blocks reading two levels out while everything is live (`nested`);
- a block outliving its method and writing its variable afterwards, twice
  independently (`counter`) — the divorce;
- nested blocks outliving every activation they read from (`adder:`).

**Files:**
- Create: `tests-fixtures/closure_variables.ns`
- Create: `tests-fixtures/closure_variables.expected`
- Modify: `crates/ns-cli/tests/differential.rs:111-116` (fixture floor)
- Modify: `crates/ns-cli/tests/run_fixtures.rs:51-53` (fixture floor)

- [ ] **Step 1: Write the fixture**

`tests-fixtures/closure_variables.ns`:

```
Newspeak3
'Test'
class ClosureVariables usingPlatform: platform = Object new (
) (
	sumOf: coll = (
		| s |
		s := 0.
		coll do: [:e | s := s + e].
		^s
	)
	nested = (
		| a |
		a := 1.
		^[:b | [:c | a + b + c] value: 10] value: 100
	)
	counter = (
		| n |
		n := 0.
		^[n := n + 1. n]
	)
	adder: base = (
		^[:x | [:y | base + x + y]]
	)
	public main: p args: a = (
		| coll c d add |
		coll := OrderedCollection new.
		coll add: 1; add: 2; add: 3.
		p transcript show: (self sumOf: coll) printString; newLine.
		p transcript show: self nested printString; newLine.
		c := self counter.
		d := self counter.
		c value.
		c value.
		p transcript show: c value printString; newLine.
		p transcript show: d value printString; newLine.
		add := (self adder: 100) value: 20.
		p transcript show: (add value: 3) printString; newLine
	)
) : ()
```

`tests-fixtures/closure_variables.expected`:

```
6
111
3
1
123
```

- [ ] **Step 2: Run it under both engines**

Run: `cargo run -p ns-cli -- run tests-fixtures/closure_variables.ns`
Run: `cargo run -p ns-cli -- run tests-fixtures/closure_variables.ns --engine tree-walk`
Expected: both print exactly the five lines of the `.expected` file and exit 0.
If either differs, the fixture is wrong, not the runtime — this task changes
no runtime code. Fix the fixture.

- [ ] **Step 3: Raise the two fixture floors**

In `crates/ns-cli/tests/differential.rs`, change `checked >= 18` to
`checked >= 19` and "every one of the 18" to "every one of the 19". In
`crates/ns-cli/tests/run_fixtures.rs`, make the same two changes.

- [ ] **Step 4: Run the CLI suites**

Run: `cargo test -p ns-cli`
Expected: all green; the new fixture runs under `differential`,
`run_fixtures`, `build_load` and `snapshot_load`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
git add tests-fixtures/closure_variables.ns tests-fixtures/closure_variables.expected crates/ns-cli/tests/differential.rs crates/ns-cli/tests/run_fixtures.rs
git commit -F - <<'EOF'
test: guard how blocks share their method's variables

Covers a block writing its method's variable while the method runs,
nested blocks reading two levels out, and blocks outliving the
activations they read from -- each of which the activation stack could
get silently wrong.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WPVgnoKxpths9NssEzm6TE
EOF
```

---

### Task 4: Activations move onto the stack

The switch. After it, no activation is allocated in the arena unless a block
captures it. Every function that means *the running activation* takes an
`ActivationRef`; the type change makes this one task, because `run_code`, the
evaluators and the three engine hooks all name the same handle.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (remove or rename the old frame accessors; debug assertions)
- Modify: `crates/ns-runtime/src/types.rs` (`Frame` docs)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`
- Modify: `crates/ns-runtime/src/interpreter/eval.rs`
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`
- Modify: `crates/ns-runtime/src/interpreter/nested.rs`
- Modify: `crates/ns-runtime/src/vm.rs`
- Test: `crates/ns-cli/tests/differential.rs` (new test)
- Test: `crates/ns-runtime/tests/interpreter.rs` (two tests rewritten, one helper replaced)
- Test: `crates/ns-runtime/tests/image.rs` (two accessor renames)
- Test: `crates/ns-runtime/src/heap.rs` (`mod tests`: accessor renames)

**Interfaces:**
- Consumes: everything Task 1 produced; the sender-free signatures from Task 2.
- Produces:
  - `pub enum Signal { Return(ActivationRef, Value), Error(NewspeakError) }`
  - `vm::run_code(interp: &mut Interpreter, act: &ActivationRef, code: &Rc<CompiledCode>, bindings: &Rc<HashMap<Span, Binding>>) -> EvalResult`
  - `eval::eval_expr(interp, expr, act: &ActivationRef, bindings) -> EvalResult` and the same `act` parameter on every `eval_*`, `dispatch_lexical_send`, `dispatch_super_send`
  - `eval::read_local(heap: &Heap, act: &ActivationRef, index: u16) -> Value`, `eval::write_local(heap: &mut Heap, act: &ActivationRef, index: u16, value: Value)`
  - `eval::read_outer_local(heap: &Heap, act: &ActivationRef, depth: u16, index: u16) -> Value`, `eval::write_outer_local(heap: &mut Heap, act: &ActivationRef, depth: u16, index: u16, value: Value)` (both `pub(crate)`)
  - `eval::home_of(heap: &mut Heap, act: &ActivationRef) -> FrameRef` (`pub(crate)`)
  - `Heap::frame_receiver(&self, frame: &FrameRef) -> Value`, `Heap::frame_grow_temps(&mut self, frame: &FrameRef, needed: usize)`
  - Removed from `Heap`: `receiver`, `receiver_and_current_class`, `local`, `set_local`, `grow_temps`, `frame_slot_base`, `frame_is_method`, `mark_uncontinuable`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/ns-cli/tests/differential.rs`, after
`a_does_not_understand_error_is_identical_under_both_engines`:

```rust
/// A block created while its class initializes, whose `^` runs after the
/// initializer has returned. Its home is dead, so this is §7.2's
/// `cannotReturn` -- under both engines, from the shared `raise_return`.
/// Before the activation stack an initializer frame was never marked
/// uncontinuable and the return escaped the entry send instead.
#[test]
fn a_return_from_a_block_whose_initializer_has_finished_is_cannot_return() {
    let outcome = assert_engines_agree_on_source(
        "init_block_return",
        "Newspeak3\n'Test'\nclass InitBlockReturn usingPlatform: platform = Object new (\n\
         \t| blk = [ ^2 ]. |\n) (\n\
         \tpublic main: p args: a = (\n\
         \t\tblk value.\n\
         \t\tp transcript show: 'unreachable'; newLine\n\
         \t)\n) : ()\n",
    );
    assert!(!outcome.ok, "a dead home must be an error");
    assert!(
        outcome.stderr.contains("cannotReturn"),
        "expected cannotReturn, got: {}",
        outcome.stderr
    );
    assert!(!outcome.stdout.contains("unreachable"));
}
```

In `crates/ns-runtime/tests/interpreter.rs`, replace the `method_frame`
helper (its doc comment included) with:

```rust
/// A live method activation, reified the way `MakeClosure` and
/// `eval_block_literal` reify one: the block's `home` in the non-local
/// return tests below. The caller pops it.
fn live_method_activation(
    heap: &mut ns_runtime::Heap,
) -> (ns_runtime::ActivationRef, ns_runtime::FrameRef) {
    let src = "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public m = ( ^0 )\n) : ()";
    let module = ns_syntax::parse(src).expect("parse");
    let method = Rc::new(ns_runtime::mixin::MixinMethod::from_decl(Rc::new(
        module.root_class.instance_side.methods[0].clone(),
    )));
    let act = heap.push_activation(
        ns_runtime::ActivationHeader {
            receiver: Value::Nil,
            current_class: None,
            kind: FrameKind::Method(method),
            pc_kind: ns_runtime::PcKind::TreeWalk,
            slot_base: 0,
        },
        &[],
        0,
    );
    let frame = heap.reify(&act);
    (act, frame)
}
```

Replace `non_local_return_from_a_block_unwinds_to_the_home_methods_activation`
with:

```rust
#[test]
fn non_local_return_from_a_block_unwinds_to_the_home_methods_activation() {
    // `[ ^1 ] value`, invoked while its home method is still running: the
    // return targets the home *activation*, not the block's own.
    let mut interp = Interpreter::new();
    let (home, home_frame) = live_method_activation(interp.heap_mut());
    let block_body = parse_block_body(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public withBlock = ( [ ^1 ] )\n) : ()",
    );
    let bindings = Rc::new(HashMap::new());
    let closure = Rc::new(ClosureData {
        params: vec![],
        body: ns_runtime::ClosureBody::Ast(Rc::new(block_body)),
        captured_frame: home_frame,
        home: home_frame,
        bindings: bindings.clone(),
    });

    let result = eval::invoke_closure(&mut interp, &closure, vec![], &bindings);

    // Exactly the check `Interpreter::invoke_method` performs to turn a
    // matching `Signal::Return` back into an ordinary value.
    match result {
        Err(Signal::Return(target, Value::SmallInt(1))) => assert!(
            target.same(&home),
            "a non-local return must target the block's home method activation, not the block's own"
        ),
        Ok(_) => panic!("expected the block's `^1` to raise a non-local return, not return normally"),
        Err(Signal::Return(_, other)) => panic!(
            "expected Signal::Return(_, SmallInt(1)), got value {}",
            interp.heap_ref().describe(&other)
        ),
        Err(Signal::Error(e)) => panic!("expected Signal::Return, got Signal::Error: {}", e.message),
    }
    assert_eq!(
        interp.heap_ref().activation_depth(),
        1,
        "the block's own activation must be popped on the way out"
    );
    interp.heap_mut().pop_activation(&home);
}
```

Replace
`invoking_an_escaped_block_after_its_home_method_returned_is_a_newspeak_error_not_a_panic`
with:

```rust
#[test]
fn invoking_an_escaped_block_after_its_home_method_returned_is_a_newspeak_error_not_a_panic() {
    let mut interp = Interpreter::new();
    let (home, home_frame) = live_method_activation(interp.heap_mut());
    let block_body = parse_block_body(
        "Newspeak3\n'T'\nclass Foo = Object new (\n) (\n  public withBlock = ( [ ^1 ] )\n) : ()",
    );
    let bindings = Rc::new(HashMap::new());
    let closure = Rc::new(ClosureData {
        params: vec![],
        body: ns_runtime::ClosureBody::Ast(Rc::new(block_body)),
        captured_frame: home_frame,
        home: home_frame,
        bindings: bindings.clone(),
    });

    // While the home runs, the return reaches it.
    let first = eval::invoke_closure(&mut interp, &closure, vec![], &bindings);
    assert!(matches!(first, Err(Signal::Return(ref target, Value::SmallInt(1))) if target.same(&home)));

    // The home method returns: its frame is divorced and dead.
    interp.heap_mut().pop_activation(&home);
    assert!(
        interp.heap_ref().frame_is_uncontinuable(&home_frame),
        "a returned activation's frame must be uncontinuable"
    );

    // Invoking the same block again is the spec's `cannotReturn` case -- a
    // `NewspeakError`, not another `Signal::Return`, and not a panic.
    let second = eval::invoke_closure(&mut interp, &closure, vec![], &bindings);
    match second {
        Err(Signal::Error(NewspeakError { message, .. })) => assert!(
            message.to_lowercase().contains("cannotreturn")
                || message.to_lowercase().contains("cannot return"),
            "expected a cannotReturn-shaped error message, got: {message}"
        ),
        other => panic!(
            "expected Err(Signal::Error(_)) for a return through an already-returned home, got {}",
            match &other {
                Ok(_) => "Ok(_)",
                Err(Signal::Return(..)) => "Err(Signal::Return(_))",
                Err(Signal::Error(_)) => "Err(Signal::Error(_))",
            }
        ),
    }
}
```

Remove `Frame` from the `use ns_runtime::{…}` list at the top of the file if
nothing else in it uses it.

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p ns-cli --test differential a_return_from_a_block_whose_initializer_has_finished_is_cannot_return`
Expected: FAIL — stderr says "internal: non-local return escaped the entry send".

Run: `cargo test -p ns-runtime --test interpreter`
Expected: compile error — `Signal::Return` still carries a `FrameRef`, so
`target.same(&home)` does not type-check against an `ActivationRef`.

- [ ] **Step 3: `heap.rs` — retire the arena accessors for a running activation**

Delete these `Heap` methods, whose `FrameRef` means *the running activation*
and which Task 1 replaced with `activation_*` versions: `receiver_and_current_class`,
`local`, `set_local`, `frame_slot_base`, `frame_is_method`,
`mark_uncontinuable`.

Rename `receiver` to `frame_receiver`, keeping its body; its doc comment:
`/// A reified frame's receiver. Fixed when the frame is created, so it needs no redirect while the frame is married.`

Rename `grow_temps` to `frame_grow_temps`, add as its first line after the
origin check

```rust
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's registers live on the activation stack; grow them through its activation"
        );
```

and change its doc comment to: `/// Grows a *dead* frame's register file, filling with nil -- what edit-and-continue will do after reifying a live activation (docs/design/liveness-and-reflection.md). A live activation grows through grow_activation_temps.`

Add the same kind of assertion as the first line after the origin check of
`set_frame_pc`, `frame_register_shape` and `frame_snapshot`:

```rust
        debug_assert!(
            self.frames[frame.index as usize].live.is_none(),
            "a married frame's pc and registers live on the activation stack; \
             read or write them only once its activation has returned"
        );
```

`frame_snapshot` is what `image::encode` reads every frame through, so this is
also the encoder's precondition: it only ever runs between sends, when nothing
is live.

Rewrite `FrameRef`'s doc comment to:

```rust
/// An opaque handle to a *reified* activation: an index into `Heap.frames`.
/// The inner index is private to this module. See `docs/design/memory.md`,
/// "The seam".
///
/// A running activation is named by an `ActivationRef` instead; a `FrameRef`
/// exists for one only once something must hold it past the current call
/// (`Heap::reify`). This is the only activation handle anything may store --
/// in a closure, in another frame, in an image.
///
/// A handle is meaningful only against the `Heap` that allocated it. The
/// arena never reuses a slot -- there is no collector yet -- so two frames
/// never share an index.
```

and `FrameRef::same`'s to:

```rust
    /// Frame identity. Guarded like an accessor: see `ObjRef::same`.
    /// `Signal::Return` no longer compares frames -- it names its target by
    /// `ActivationRef` -- so the callers left are the image round-trip tests
    /// and the decoder's own identity checks.
```

In `alloc_frame`'s doc comment, replace "The only constructor of a `FrameRef`
in the workspace -- src or tests --" with "The only constructor of a
`FrameRef` in the workspace -- `reify`, the image decoder and tests all come
through here --".

In the unit tests at the bottom of the file, `a_frame_handle_survives_the_arena_growing_under_it`
calls `heap.receiver(&first)`: change it to `heap.frame_receiver(&first)`.

- [ ] **Step 4: `types.rs` — document `Frame` as the reified form**

Above `pub struct Frame`, add:

```rust
/// A *reified* activation: one a block captured, or one the image decoder
/// rebuilt. A running activation lives on the `Heap`'s activation stack and
/// becomes a `Frame` only when `Heap::reify` is asked for one; while it runs,
/// the frame is married to its stack entry and its registers live there.
/// See `docs/design/execution.md`, "Activations".
```

Give `sender` a doc comment:

```rust
    /// `None` for every frame the runtime reifies: a live activation's
    /// sender is the stack entry below it, and a returned activation has
    /// none. Kept for the image format, which records one per frame.
```

Replace `uncontinuable`'s doc comment with:

```rust
    /// Set once this frame's activation has returned (`Heap::pop_activation`
    /// divorces it). A `^` whose home is such a frame is a `cannotReturn`
    /// error (`newspeak-spec.pdf` §3.6, §7.2). `raise_return` decides by
    /// `Heap::frame_activation`, which also covers a decoded frame, dead by
    /// construction whatever this flag says; the flag itself is what an image
    /// records.
```

- [ ] **Step 5: `interpreter/mod.rs`**

`use crate::heap::{ActivationHeader, ActivationRef, Heap};` replaces
`use crate::heap::Heap;`.

```rust
pub enum Signal {
    Return(ActivationRef, Value),
    Error(NewspeakError),
}
```

Rename the current `pub fn send` to `fn dispatch_send` (body unchanged) and add
in its place:

```rust
    /// An ordinary (explicit-receiver) send: `newspeak-spec.pdf` §5.5. See
    /// `dispatch_send` for the order the cases are tried in.
    ///
    /// A send leaves the activation stack exactly as it found it: every
    /// activation it pushes is popped on every exit, error paths included.
    /// Checked here, at every send, because a leaked entry is otherwise
    /// silent -- a later activation would sit on top of a dead one's
    /// registers.
    pub fn send(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> EvalResult {
        let depth = self.heap.activation_depth();
        let result = self.dispatch_send(receiver, selector, args, bindings);
        debug_assert_eq!(
            self.heap.activation_depth(),
            depth,
            "a send must leave the activation stack as it found it"
        );
        result
    }
```

Move the existing `send` doc comment (the paragraph about `try_factory_send`)
onto `dispatch_send`.

`invoke_method` — replace everything from `let frame = self.heap.alloc_frame(Frame {`
to the end of the function with:

```rust
        let act = self.heap.push_activation(
            ActivationHeader {
                receiver,
                current_class: Some(defining_class),
                kind: FrameKind::Method(method.clone()),
                // Fixed by the same `compiled` that decides which body runs
                // below, so the activation can never claim a unit its own
                // body does not count in.
                pc_kind: match &compiled {
                    Some(_) => PcKind::Bytecode,
                    None => PcKind::TreeWalk,
                },
                slot_base: 0, // a method body never contains InitSlot
            },
            &args,
            temp_count,
        );
        let result = match (&compiled, &method.decl) {
            (Some(code), _) => crate::vm::run_code(self, &act, code, &bindings),
            (None, Some(decl)) => self.run_statements(&decl.body.stmts, &act, &bindings),
            // Unreachable rather than merely unlikely: a mixin without
            // declarations comes from an image, and `image::load` installs
            // `Engine::Bytecode`, so such a method always arrives compiled.
            (None, None) => panic!(
                "internal: the tree-walking engine cannot run a method of a mixin \
                 decoded from an image"
            ),
        };
        self.heap.pop_activation(&act);
        match result {
            Ok(value) => Ok(value),
            Err(Signal::Return(target, value)) if target.same(&act) => Ok(value),
            Err(other) => Err(other),
        }
```

and replace its doc comment's second paragraph (from "The frame is marked
`uncontinuable` on *every* path out" to the end of that paragraph) with:

```rust
    /// The activation is popped on *every* path out -- `^`, falling off the
    /// end, or an unwinding error or return. If a block captured it, popping
    /// divorces its frame, which is what makes a later `^` from that block
    /// the spec's `cannotReturn` rather than a return with no live catcher.
```

`run_statements` — the parameter becomes `act: &ActivationRef`:

```rust
    fn run_statements(
        &mut self,
        stmts: &[ast::Stmt],
        act: &ActivationRef,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> EvalResult {
        let mut last = Value::Nil;
        for (i, stmt) in stmts.iter().enumerate() {
            self.heap.set_activation_pc(act, i);
            match stmt {
                ast::Stmt::Expr(e) => {
                    last = eval::eval_expr(self, e, act, bindings)?;
                }
                ast::Stmt::Return(e) => {
                    let value = eval::eval_expr(self, e, act, bindings)?;
                    return Err(self.raise_return(act, value));
                }
            }
        }
        // No explicit return: methods return `self` (spec §3.5), closures
        // return the last expression's value (§5.1.8).
        if self.heap.activation_is_method(act) {
            Ok(self.heap.activation_receiver(act))
        } else {
            Ok(last)
        }
    }
```

`raise_return` and `raise_return_pub`:

```rust
    /// Where a `^` in `act` returns to. A block returns from its home --
    /// the method or initializer that lexically encloses it -- and only
    /// while that activation is still running: its reified frame is married
    /// to a live stack entry exactly until the activation returns, and a
    /// frame the image decoder built never is. A dead home is §7.2's
    /// `cannotReturn`. Every other activation's `^` returns from itself.
    fn raise_return(&mut self, act: &ActivationRef, value: Value) -> Signal {
        let target = match self.heap.activation_home(act) {
            None => *act,
            Some(home) => match self.heap.frame_activation(&home) {
                Some(live) => live,
                None => {
                    return Signal::Error(NewspeakError {
                        selector: Symbol::intern("^"),
                        receiver: value,
                        message:
                            "cannotReturn: the enclosing method activation has already returned"
                                .to_string(),
                    })
                }
            },
        };
        Signal::Return(target, value)
    }

    pub(crate) fn raise_return_pub(&mut self, act: &ActivationRef, value: Value) -> Signal {
        self.raise_return(act, value)
    }
```

The error text is unchanged, character for character: both engines print it,
and the differential harness compares stderr.

- [ ] **Step 6: `interpreter/eval.rs`**

Replace `use crate::heap::Heap;` with
`use crate::heap::{ActivationHeader, ActivationRef, Heap};`.

Every function that takes `frame: &FrameRef` for the running activation takes
`act: &ActivationRef` instead: `eval_expr`, `eval_ident`, `eval_send`,
`eval_args`, `eval_cascade`, `eval_assign`, `eval_block_literal`,
`dispatch_lexical_send`, `dispatch_super_send`. Inside them, rename the
variable and use the activation accessors: `interp.heap.receiver(frame)` →
`interp.heap.activation_receiver(act)`,
`interp.heap.receiver_and_current_class(frame)` →
`interp.heap.activation_receiver_and_current_class(act)`,
`interp.heap.frame_current_class(frame)` (in `dispatch_super_send`) →
`interp.heap.activation_current_class(act)`. Nothing else in those functions'
logic changes.

Replace `read_local`, `write_local`, `frame_at_depth`, `home_of`,
`read_outer_local` and `write_outer_local` with:

```rust
pub(crate) fn read_local(heap: &Heap, act: &ActivationRef, index: u16) -> Value {
    heap.activation_local(act, index)
}

pub(crate) fn write_local(heap: &mut Heap, act: &ActivationRef, index: u16, value: Value) {
    heap.activation_set_local(act, index, value);
}

/// Where a `Binding::OuterLocal` lives: in the running activation itself,
/// or in a frame some closure captured.
enum OuterFrame {
    Own(ActivationRef),
    Captured(FrameRef),
}

/// The activation `depth` hops out along the `captured_frame` chain. The
/// first hop leaves the running activation for its closure's captured frame;
/// the rest follow frames. It stops hopping at a non-closure activation, so a
/// depth past the lexical nesting lands on the outermost body rather than
/// running off the chain -- `image::decode::enclosing_body` is the static
/// counterpart and relies on exactly this, for an image whose `depth` it
/// verified against the lexical nesting rather than the chain.
fn frame_at_depth(heap: &Heap, act: &ActivationRef, depth: u16) -> OuterFrame {
    if depth == 0 {
        return OuterFrame::Own(*act);
    }
    let Some(mut frame) = heap.activation_captured(act) else {
        return OuterFrame::Own(*act);
    };
    for _ in 1..depth {
        frame = heap.frame_captured(&frame).unwrap_or(frame);
    }
    OuterFrame::Captured(frame)
}

/// Shared by the tree-walker's `eval_ident` and the VM's `LoadOuterLocal`.
pub(crate) fn read_outer_local(heap: &Heap, act: &ActivationRef, depth: u16, index: u16) -> Value {
    match frame_at_depth(heap, act, depth) {
        OuterFrame::Own(own) => heap.activation_local(&own, index),
        OuterFrame::Captured(frame) => heap.frame_local(&frame, index),
    }
}

/// Shared by the tree-walker's `eval_assign` and the VM's `StoreOuterLocal`.
pub(crate) fn write_outer_local(
    heap: &mut Heap,
    act: &ActivationRef,
    depth: u16,
    index: u16,
    value: Value,
) {
    match frame_at_depth(heap, act, depth) {
        OuterFrame::Own(own) => heap.activation_set_local(&own, index, value),
        OuterFrame::Captured(frame) => heap.frame_set_local(&frame, index, value),
    }
}

/// The `home` a closure created in `act` gets: a closure activation passes
/// its own closure's `home` on; any other activation -- method, initializer,
/// superclass clause or message -- is its own return target, so it is
/// reified and is its own home. Shared by the tree-walker's
/// `eval_block_literal` and the VM's `MakeClosure`.
pub(crate) fn home_of(heap: &mut Heap, act: &ActivationRef) -> FrameRef {
    match heap.activation_home(act) {
        Some(home) => home,
        None => heap.reify(act),
    }
}
```

In `eval_ident`: `Some(Binding::OuterLocal { depth, index }) => Ok(read_outer_local(&interp.heap, act, *depth, *index)),`.
In `eval_assign`: `write_outer_local(&mut interp.heap, act, *depth, *index, v)`.

`eval_block_literal` — its body becomes:

```rust
    // A block reaches its method's variables through the activation it was
    // created in, and returns through its home, so both must outlive this
    // call: reifying is what makes them storable.
    let captured_frame = interp.heap.reify(act);
    let home = home_of(&mut interp.heap, act);
    let closure = Rc::new(ClosureData {
        params: block
            .params
            .iter()
            .map(|p| Symbol::intern(&p.name))
            .collect(),
        body: ClosureBody::Ast(Rc::new(block.body.clone())),
        captured_frame,
        home,
        bindings: bindings.clone(),
    });
    Ok(Value::Obj(interp.new_closure(closure)))
```

`invoke_closure` — replace everything from `let receiver = interp.heap.receiver(&closure.captured_frame);`
(the second occurrence, after the arity check) to the end of the function
with:

```rust
    let receiver = interp.heap.frame_receiver(&closure.captured_frame);
    let current_class = interp.heap.frame_current_class(&closure.captured_frame);
    // Hook 2 (spec: "`vm.rs` and the engine hook"): the activation is built
    // the same way for both engines; only the body's evaluation differs, and
    // which engine that is was decided when the closure was created.
    let temp_count = match &closure.body {
        ClosureBody::Ast(body) => body.temps.len(),
        ClosureBody::Compiled(code) => (code.num_registers - code.num_args) as usize,
    };
    let act = interp.heap.push_activation(
        ActivationHeader {
            receiver,
            current_class,
            kind: FrameKind::Closure(closure.clone()),
            // Decided by the closure's own body, not by the interpreter's
            // current engine: a closure always runs with the engine that
            // created it (`ClosureBody`'s doc comment).
            pc_kind: match &closure.body {
                ClosureBody::Ast(_) => PcKind::TreeWalk,
                ClosureBody::Compiled(_) => PcKind::Bytecode,
            },
            slot_base: 0, // a closure body never contains InitSlot
        },
        &args,
        temp_count,
    );
    let result = match &closure.body {
        ClosureBody::Compiled(code) => crate::vm::run_code(interp, &act, code, bindings),
        ClosureBody::Ast(body) => run_block_body(interp, body, &act, bindings),
    };
    // Popped on every path out, like `invoke_method`'s. A closure
    // activation catches no return: its `^` targets its home.
    interp.heap.pop_activation(&act);
    result
}

/// The tree-walker's block body: the statements in order, the last one's
/// value as the result, and a `^` raised toward the block's home.
fn run_block_body(
    interp: &mut Interpreter,
    body: &ast::CodeBody,
    act: &ActivationRef,
    bindings: &Rc<HashMap<Span, Binding>>,
) -> EvalResult {
    let mut last = Value::Nil;
    for (i, stmt) in body.stmts.iter().enumerate() {
        interp.heap.set_activation_pc(act, i);
        match stmt {
            ast::Stmt::Expr(e) => last = eval_expr(interp, e, act, bindings)?,
            ast::Stmt::Return(e) => {
                let value = eval_expr(interp, e, act, bindings)?;
                return Err(interp.raise_return_pub(act, value));
            }
        }
    }
    Ok(last)
}
```

and in the arity check above it, `interp.heap.receiver(&closure.captured_frame)`
becomes `interp.heap.frame_receiver(&closure.captured_frame)`.

- [ ] **Step 7: `vm.rs`**

Imports:

```rust
use crate::heap::{ActivationRef, Heap};
use crate::interpreter::eval::{
    dispatch_lexical_send, dispatch_super_send, home_of, malformed_int_error, parse_int_literal,
    read_local, read_outer_local, store_instance_field, unresolved_identifier_error,
    write_local, write_outer_local,
};
```

`run_code`'s signature becomes
`pub fn run_code(interp: &mut Interpreter, act: &ActivationRef, code: &Rc<CompiledCode>, bindings: &Rc<HashMap<Span, Binding>>) -> EvalResult`,
and throughout the loop `frame` becomes `act`. The arms that change beyond
that rename:

```rust
            Instr::LoadSelf { dst } => {
                let v = interp.heap.activation_receiver(act);
                write_local(&mut interp.heap, act, *dst, v);
            }
```

```rust
            Instr::LoadOuterLocal { dst, depth, index } => {
                let v = read_outer_local(&interp.heap, act, *depth, *index);
                write_local(&mut interp.heap, act, *dst, v);
            }
            Instr::StoreOuterLocal { src, depth, index } => {
                let v = read_local(&interp.heap, act, *src);
                write_outer_local(&mut interp.heap, act, *depth, *index, v);
            }
```

Every `interp.heap.set_frame_pc(frame, pc)` becomes
`interp.heap.set_activation_pc(act, pc)`.

```rust
            Instr::MakeClosure {
                dst,
                params,
                code: block,
            } => {
                // Stored before reifying, so the frame `reify` builds
                // carries this pc -- the point at which the activation
                // escaped into the object graph.
                interp.heap.set_activation_pc(act, pc);
                let captured_frame = interp.heap.reify(act);
                let home = home_of(&mut interp.heap, act);
                let closure = Rc::new(ClosureData {
                    params: (**params).clone(),
                    body: ClosureBody::Compiled(block.clone()),
                    captured_frame,
                    home,
                    bindings: bindings.clone(),
                });
                let v = Value::Obj(interp.new_closure(closure));
                write_local(&mut interp.heap, act, *dst, v);
            }
```

```rust
            Instr::InitSlot { src, own_index } => {
                // Shares `store_instance_field` with
                // `instantiate::run_own_slots`: a direct field write on
                // `self`, silently skipped for a non-instance receiver.
                // `own_index` is relative to the executing mixin's own
                // slots (the same compiled body runs for every application
                // of that mixin); the activation's `slot_base` -- set once,
                // when `Interpreter::run_initializer_level` pushes this
                // level's activation, from that level's own
                // `MixinApplication.own_slot_base` -- is what turns it into
                // this instance's actual absolute field index.
                let v = read_local(&interp.heap, act, *src);
                let receiver = interp.heap.activation_receiver(act);
                let base = interp.heap.activation_slot_base(act);
                store_instance_field(&mut interp.heap, &receiver, base + *own_index as usize, v);
            }
```

```rust
            Instr::NonLocalReturn { reg } => {
                let v = read_local(&interp.heap, act, *reg);
                interp.heap.set_activation_pc(act, pc);
                return Err(interp.raise_return_pub(act, v));
            }
```

`collect` and `make_value` take `act: &ActivationRef`; `make_value`'s
`UnresolvedIdent` arm reads `interp.heap.activation_receiver(act)`.

Rewrite `run_code`'s doc comment to:

```rust
/// Runs `code` on the live activation `act` until a terminating
/// instruction. `act` was pushed by the shared orchestration
/// (`invoke_method`, `invoke_closure`, `instantiate` or
/// `nested::run_clause_body`) with registers for `code.num_registers`, and
/// that same orchestration pops it.
///
/// The activation's `pc` is the safepoint view (`statement_pc` recovers the
/// statement from it on demand), and it is stored *only by the instructions
/// that can hand the activation to an observer*: every `Send*` (a nested
/// activation runs above this one), `MakeClosure` (it reifies this
/// activation, and the frame it builds takes the `pc` stored just before),
/// and `NonLocalReturn` (control leaves with a signal). Nothing can read
/// `pc` while a straight-line instruction executes, because `run_code` never
/// yields between them.
```

- [ ] **Step 8: `interpreter/instantiate.rs`**

Import `use crate::heap::{ActivationHeader, ActivationRef};` (keep the
existing imports).

In `run_initializer_level`, replace `let frame = self.heap.alloc_frame(Frame { … });`
through `self.run_own_initializer(class, instance, &frame, bindings)` with:

```rust
        let act = self.heap.push_activation(
            ActivationHeader {
                receiver: *instance,
                current_class: Some(*class),
                kind: FrameKind::Initializer(*class),
                // `run_own_initializer` below picks the body off the same
                // engine, and this activation is built only when that level
                // has initializer code to run -- so under `Engine::Bytecode`
                // it always ends up executing a `CompiledCode`.
                pc_kind: match &self.engine {
                    Engine::TreeWalk => PcKind::TreeWalk,
                    Engine::Bytecode(_) => PcKind::Bytecode,
                },
                slot_base: self
                    .heap
                    .class_application(class)
                    .map_or(0, |a| a.own_slot_base),
            },
            &factory_args,
            0,
        );

        // Steps 4-5: this class's own slot clauses (declaration order), then
        // its own initExprs -- engine hook 3. Popped on every path out, like
        // every activation.
        let result = self.run_own_initializer(class, instance, &act, bindings);
        self.heap.pop_activation(&act);
        result
```

In `run_own_initializer`, `run_own_slots` and `run_own_init_exprs`, the
parameter `frame: &FrameRef` becomes `act: &ActivationRef` and every use
follows; in `run_own_initializer`:

```rust
                debug_assert_eq!(
                    self.heap.activation_slot_base(act),
                    own_slot_base,
                    "an initializer activation must be built for the level it runs"
                );
                let needed = (code.num_registers - code.num_args) as usize;
                self.heap.grow_activation_temps(act, needed);
                crate::vm::run_code(self, act, &code, bindings).map(|_| ())
```

Wrap `instantiate` the way Step 5 wrapped `send`: rename the existing
`pub fn instantiate` to `fn instantiate_unchecked` and add

```rust
    /// The six numbered steps in the runtime spec's Class Instantiation
    /// section; see `instantiate_unchecked`. Like `send`, it leaves the
    /// activation stack as it found it.
    pub fn instantiate(
        &mut self,
        class: ClassRef,
        factory_args: Vec<Value>,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> EvalResult {
        let depth = self.heap.activation_depth();
        let result = self.instantiate_unchecked(class, factory_args, bindings);
        debug_assert_eq!(
            self.heap.activation_depth(),
            depth,
            "instantiation must leave the activation stack as it found it"
        );
        result
    }
```

Any other caller of `instantiate` inside the crate (`try_factory_send`) keeps
calling `instantiate`.

- [ ] **Step 9: `interpreter/nested.rs`**

Import `use crate::heap::ActivationHeader;`. In `run_clause_body`, replace
`let frame = interp.heap.alloc_frame(Frame { … });` through the final `match`
with:

```rust
    let act = interp.heap.push_activation(
        ActivationHeader {
            receiver: *receiver,
            current_class: Some(*current_class),
            kind,
            // The same choice `compiled` makes below: a clause body runs
            // compiled when the module carries one and is tree-walked from
            // its own AST otherwise.
            pc_kind: match &compiled {
                Some(_) => PcKind::Bytecode,
                None => PcKind::TreeWalk,
            },
            slot_base: 0, // no clause body ever contains InitSlot
        },
        &args,
        temp_count,
    );
    let result = match (&compiled, expr) {
        (Some(code), _) => crate::vm::run_code(interp, &act, code, bindings),
        (None, Some(expr)) => crate::interpreter::eval::eval_expr(interp, expr, &act, bindings),
        // The tree-walker evaluates the clause's own AST, and a mixin
        // decoded from an image carries none. Unreachable rather than
        // merely unlikely: `image::load` installs `Engine::Bytecode`, so a
        // decoded mixin's clause always arrives here with a compiled body.
        (None, None) => panic!(
            "internal: the tree-walking engine cannot run a superclass clause \
             of a mixin decoded from an image"
        ),
    };
    interp.heap.pop_activation(&act);
    match result {
        Ok(value) => Ok(value),
        Err(Signal::Return(target, value)) if target.same(&act) => Ok(value),
        Err(other) => Err(other),
    }
```

- [ ] **Step 10: The remaining test call sites**

`crates/ns-runtime/tests/image.rs`, in
`a_closure_and_its_paused_home_frame_round_trip`: `interp.heap_mut().grow_temps(&c.home, …)`
→ `frame_grow_temps`, and `interp.heap_mut().set_local(&c.home, …)` →
`frame_set_local`. (The frame is dead by then — `outer` has returned — which
is exactly what the new assertions in those accessors require.)

Run `cargo build --workspace --all-targets` and fix any other site the
compiler names the same way: a `FrameRef` accessor that was renamed gets its
`frame_` name; a use of a removed accessor on a running activation is a place
this plan missed — stop and report it rather than re-adding the accessor.

- [ ] **Step 11: Run everything**

Run: `cargo test --workspace`
Expected: all green, including the three tests from Step 1, the
`closure_variables` fixture under every CLI harness, and
`a_return_inside_a_block_in_init_exprs_escapes_instantiate_under_both_engines`
in `tests/vm_initializer.rs` (a `^` into an initializer that is still running
still escapes, which the spec keeps as a known gap).

A debug-assertion failure reading "a send must leave the activation stack as
it found it" means some path returns before its `pop_activation`; find it — do
not relax the assertion.

- [ ] **Step 12: Lint, format, gates, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo run -p ns-cli -- parse-corpus corpus/modules
cargo run -p ns-cli -- run-corpus corpus/modules
git add -A crates
git commit -F - <<'EOF'
feat: run activations on a stack, reified only when a block captures one

invoke_method, invoke_closure, the initializer and the superclass clause
push onto the Heap's activation stack and pop on every exit; the running
activation is an ActivationRef everywhere. MakeClosure and
eval_block_literal reify, and popping divorces the frame. Signal::Return
names its target by ActivationRef, and a block whose home has returned
raises cannotReturn -- including one created during initialization,
which used to escape the entry send.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WPVgnoKxpths9NssEzm6TE
EOF
```

---

### Task 5: Benchmarks that build activations

`bench_loop.ns` builds almost no activations under the bytecode engine — the
compiler inlines `whileTrue:` — so it cannot show what this step changed. Two
fixtures do: one user-method send per iteration (the third row of the send
decomposition in `docs/design/execution.md`), and one method per iteration
that creates and invokes a block, which exercises reify, the married redirect
and the divorce.

**Files:**
- Create: `tests-fixtures/bench_send.ns`, `tests-fixtures/bench_send.expected`
- Create: `tests-fixtures/bench_block.ns`, `tests-fixtures/bench_block.expected`
- Modify: `crates/ns-cli/tests/differential.rs`, `crates/ns-cli/tests/run_fixtures.rs` (fixture floors 19 → 21)

- [ ] **Step 1: Write the fixtures**

`tests-fixtures/bench_send.ns`:

```
Newspeak3
'Test'
class BenchSend usingPlatform: platform = Object new (
	| n ::= 0. sum ::= 0. |
) (
	tick: x = ( ^x + 1 )
	public main: p args: a = (
		[n < 50000] whileTrue: [sum := self tick: sum. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`tests-fixtures/bench_send.expected`:

```
50000
```

`tests-fixtures/bench_block.ns`:

```
Newspeak3
'Test'
class BenchBlock usingPlatform: platform = Object new (
	| n ::= 0. sum ::= 0. |
) (
	bump: x = (
		| acc |
		acc := x.
		[:y | acc := acc + y] value: 1.
		^acc
	)
	public main: p args: a = (
		[n < 50000] whileTrue: [sum := self bump: sum. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`tests-fixtures/bench_block.expected`:

```
50000
```

- [ ] **Step 2: Run both under both engines**

Run each of:
`cargo run -p ns-cli -- run tests-fixtures/bench_send.ns`,
`cargo run -p ns-cli -- run tests-fixtures/bench_send.ns --engine tree-walk`,
`cargo run -p ns-cli -- run tests-fixtures/bench_block.ns`,
`cargo run -p ns-cli -- run tests-fixtures/bench_block.ns --engine tree-walk`.
Expected: each prints `50000` and exits 0.

- [ ] **Step 3: Raise the fixture floors**

In `differential.rs` and `run_fixtures.rs`: `checked >= 19` → `checked >= 21`,
"every one of the 19" → "every one of the 21".

- [ ] **Step 4: Run the CLI suites**

Run: `cargo test -p ns-cli`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
git add tests-fixtures/bench_send.ns tests-fixtures/bench_send.expected tests-fixtures/bench_block.ns tests-fixtures/bench_block.expected crates/ns-cli/tests/differential.rs crates/ns-cli/tests/run_fixtures.rs
git commit -F - <<'EOF'
test: add benchmarks that build an activation per iteration

bench_loop.ns inlines its loop and builds almost no activations, so it
cannot show what the activation stack changes. bench_send sends a user
method per iteration; bench_block also creates and invokes a block in it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WPVgnoKxpths9NssEzm6TE
EOF
```

---

### Task 6: Measure, and describe the built state

Figures are only compared against each other when taken in **one sitting**:
the "before" binary is built from the branch's merge-base in a throwaway
worktree, and the two binaries are run alternately. A number carried over from
another session measures the machine's load, not the code.

**Files:**
- Modify: `docs/design/execution.md`, `docs/design/memory.md`,
  `docs/design/image-format.md`, `docs/design/README.md`, `docs/PROGRESS.md`,
  `CLAUDE.md`
- Scratch (never committed): a worktree for the before-binary, two extra
  benchmark sources, measurement output.

- [ ] **Step 1: Build both binaries**

PowerShell, from the repository root:

```powershell
$base = git merge-base master HEAD
git worktree add ..\ns-next-before $base
cargo build --release -p ns-cli --manifest-path ..\ns-next-before\Cargo.toml
cargo build --release -p ns-cli
$before = (Resolve-Path ..\ns-next-before\target\release\ns-next.exe).Path
$after  = (Resolve-Path .\target\release\ns-next.exe).Path
```

- [ ] **Step 2: Time the three benchmarks, alternating**

Write the two decomposition fixtures that are not in `tests-fixtures/` to the
session scratch directory (`$env:TEMP` is fine), exactly as below; both
differ from `bench_send.ns` by one statement:

`send_row1.ns` (no user send):

```
Newspeak3
'Test'
class SendRow1 usingPlatform: platform = Object new (
	| n ::= 0. sum ::= 0. |
) (
	public main: p args: a = (
		[n < 50000] whileTrue: [sum := sum + 1. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

`send_row2.ns` (a user send whose body does no native send):

```
Newspeak3
'Test'
class SendRow2 usingPlatform: platform = Object new (
	| n ::= 0. sum ::= 0. |
) (
	tick: x = ( ^x )
	public main: p args: a = (
		[n < 50000] whileTrue: [sum := self tick: sum. n := n + 1].
		p transcript show: sum printString; newLine
	)
) : ()
```

Then, three rounds, alternating the binaries within each round:

```powershell
$files = @('tests-fixtures\bench_loop.ns', 'tests-fixtures\bench_send.ns',
           'tests-fixtures\bench_block.ns', "$env:TEMP\send_row1.ns", "$env:TEMP\send_row2.ns")
foreach ($round in 1..3) {
  foreach ($f in $files) {
    "== round $round  before  $f"; & $before bench $f
    "== round $round  after   $f"; & $after  bench $f
  }
}
```

Record every bytecode and tree-walk median. Report per-iteration costs as
`median_ms * 1e6 / 50000` ns. The decomposition, for each binary: row 3
(`bench_send.ns`) minus row 1 is one user-method send; row 2 minus row 1 is
the activation cost the spec cites as ~68 ns before the arena.

- [ ] **Step 3: Peak working set**

The metric and its conditions are `docs/design/memory.md`'s ("What the
collector owns"): Windows peak working set read through
`psapi!GetProcessMemoryInfo` on the still-open process handle, because
PowerShell's `PeakWorkingSet64` reads 0 once the process has exited.

```powershell
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class PeakMem {
  [StructLayout(LayoutKind.Sequential)]
  public struct Counters {
    public uint cb; public uint PageFaultCount;
    public UIntPtr PeakWorkingSetSize; public UIntPtr WorkingSetSize;
    public UIntPtr QuotaPeakPagedPoolUsage; public UIntPtr QuotaPagedPoolUsage;
    public UIntPtr QuotaPeakNonPagedPoolUsage; public UIntPtr QuotaNonPagedPoolUsage;
    public UIntPtr PagefileUsage; public UIntPtr PeakPagefileUsage;
  }
  [DllImport("psapi.dll", SetLastError = true)]
  static extern bool GetProcessMemoryInfo(IntPtr process, out Counters counters, uint size);
  public static double PeakMegabytes(string exe, string args) {
    var info = new ProcessStartInfo(exe, args) { UseShellExecute = false, RedirectStandardOutput = true };
    using (var p = Process.Start(info)) {
      p.StandardOutput.ReadToEnd();
      p.WaitForExit();
      Counters c;
      GetProcessMemoryInfo(p.Handle, out c, (uint)Marshal.SizeOf(typeof(Counters)));
      return (ulong)c.PeakWorkingSetSize / (1024.0 * 1024.0);
    }
  }
}
'@
foreach ($exe in @($before, $after)) {
  foreach ($a in @('run --engine bytecode tests-fixtures\bench_loop.ns',
                   'run --engine tree-walk tests-fixtures\bench_loop.ns',
                   'bench --iterations 10 tests-fixtures\bench_loop.ns',
                   'bench --iterations 10 tests-fixtures\bench_send.ns',
                   'bench --iterations 10 tests-fixtures\bench_block.ns')) {
    "{0}  {1}  {2:N1} MB" -f (Split-Path $exe -Parent), $a, [PeakMem]::PeakMegabytes($exe, $a)
  }
}
```

The spec predicts the tree-walk `bench_loop` and `bench --iterations 10`
figures fall from 25.4 MB and 340.7 MB toward the 4.6 MB baseline, because an
activation nothing reified leaves nothing behind. `bench_block` keeps growing
under both binaries — every iteration reifies one frame, and nothing reclaims
an arena slot — and that is expected, not a regression to fix here.

- [ ] **Step 4: Remove the worktree**

```powershell
git worktree remove ..\ns-next-before --force
```

- [ ] **Step 5: Update the design documents**

`docs/design/execution.md`:
- Status block: add `the activation stack` to the `heap.rs` line of **Code:**.
- Rewrite "## Activations" to describe the built state: the `ActivationStack`,
  `StackEntry` and `ActivationRef` (the code block and bullets from "Representation (1a)"
  move here, in the present tense), `Frame` as the reified form married to its
  entry and divorced on pop, the sender rule, and the rule that only a
  `FrameRef` is ever stored. Keep the paragraphs on `current_class`, `kind`,
  `pc` and the growable register file, restated for the stack (growing a live
  activation reifies it first).
- "## Where `pc` is stored": the stores go to the activation; `MakeClosure`'s
  store is what `reify` copies.
- "## Non-local return": replace the description with the `ActivationRef`
  target and the live-or-dead home rule, and state that the initializer-home
  case now raises `cannotReturn` once the initializer has returned, while a
  `^` into a running initializer still escapes.
- "## Measurements": add a subsection "### What the activation stack moved"
  with the Step 2 table (both binaries, three rounds, bytecode and tree-walk,
  all five sources), the re-taken send decomposition, and the same caveats the
  arena subsection states. Update the first paragraph's headline figure if
  `bench_loop.ns` moved.
- "## Planned: activations on a stack, reified on demand": remove the 1a
  subsections that now live under "Activations"; retitle it
  "## Planned: the trampoline" and keep "The trampoline (1b)", "What stays
  recursive", "How this meets the liveness requirements", plus a short "What
  1a left for it" (args `Vec`, recursion, no depth limit, the VM does not
  cache `base`).
- In the section being moved, correct the "Signatures" bullet: the `sender`
  parameters were removed, not retyped, because nothing read them.
- "## Decisions": keep the three bullets added by the design commit; reword
  the "Activations are stack entries" bullet into the past tense of a
  measured result, citing Step 2's figures.

`docs/design/memory.md`:
- Status block: "Objects live in an arena … activations live on an
  activation stack in the same `Heap` and reach the frame arena only when a
  block captures one".
- "What the collector owns": add an "After the activation stack" column to
  the peak-working-set table with Step 3's figures, and one paragraph on what
  remains unbounded (reified frames, every object).
- "Known rough edges in the seam": replace the frame half of the naming
  bullet — frame accessors are now uniformly `frame_*` and activation
  accessors `activation_*`; the object side is unchanged.

`docs/design/image-format.md`, "How frames actually get into an image": a
frame enters the table only through a closure's `captured_frame`/`home`; every
frame in a snapshot is dead (divorced) and has no sender, so a closure no
longer drags its callers in. The format still records a `sender` per frame.

`docs/design/README.md`: in the map, execution.md's "Planned next" becomes
"the trampoline, closure conversion, JIT tiers"; memory.md's status reads
"Partial — the arena and the activation stack are built, nothing reclaims an
arena slot". In "Architecture", the `heap.rs` paragraph mentions the
activation stack and `ActivationRef`.

`docs/PROGRESS.md`:
- The M4 table row already links this plan; leave it.
- "Where the next session starts": 1a is done — replace its "Planned in …,
  on branch …" sentence with one sentence of result and a pointer to the new
  section; 1b is next.
- Add a section "## M4 — The activation stack (YYYY-MM-DD)", dated the day
  the measurements were taken, after the arena's,
  in the same shape: what changed, what was deleted, the two observable
  changes, the Step 2 and Step 3 tables, and what did not move.

`CLAUDE.md`:
- In the opening crate list, the `ns-runtime` line: "Objects live in an arena
  owned by one `Heap` and are addressed by 32-bit handles; activations live on
  an activation stack in the same `Heap` and are reified into the frame arena
  only when a block captures one. Nothing reclaims an arena slot yet."
- In "A handle means nothing against a `Heap` that did not mint it", add
  `ActivationRef` to the list of handles.
- Add a new invariant after that one:

  ```markdown
  **A running activation is an `ActivationRef`; only a `FrameRef` is ever
  stored.** An `ActivationRef` indexes the `Heap`'s activation stack and is
  valid only until its entry is popped -- the next activation at that depth
  reuses the index, so a handle kept past its pop reads a different, perfectly
  valid activation. `ClosureData`, `Frame` and every image record take a
  `FrameRef` (get one with `Heap::reify`), so storing an `ActivationRef` is a
  compile error; a `debug_assertions`-only generation check turns a stale
  read into a panic. Every site that pushes an activation pops it on every
  exit, error paths included -- `Interpreter::send` and `instantiate`
  debug-assert that the stack is balanced. A reified frame is married to its
  live entry and divorced on pop, which copies its registers out, because a
  block reads its method's variables by reference
  (`docs/design/execution.md`, "Activations").
  ```

- [ ] **Step 6: Check that a fresh session could start from the docs**

Read `CLAUDE.md`, `docs/design/README.md` and `docs/PROGRESS.md`'s "Where the
next session starts" as someone who has never seen this branch: they must
name 1b as next, point to `execution.md`'s "Planned: the trampoline", and not
describe anything this plan built in the future tense. Fix what does not.

- [ ] **Step 7: Commit**

```bash
git add docs CLAUDE.md
git commit -F - <<'EOF'
docs: record the activation stack as built, with its measurements

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WPVgnoKxpths9NssEzm6TE
EOF
```
