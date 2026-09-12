# The Reference Lowering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give ns-next its permanent differential oracle — a second,
deliberately naive lowering under which every recorded program must produce the
same answer as under the optimized one — after first making identity hashes
lazy, so that the extra closures the naive lowering allocates stay invisible.

**Architecture:** Identity hashes move from "assigned at allocation" to
"assigned on the first `hash`" (Task 1), as in Cog/Spur and Primordial Soup.
`compiler/reference.rs` is written as an independent module beside `lower.rs`,
selected per `Interpreter` through a `Lowering` value (Task 2) and on the CLI by
`--lowering` (Task 3). Every fixture, golden outcome, build/load round trip and
`run-corpus` row then runs under both lowerings against the same recorded answer
(Task 4), and the documents are brought to the finished state (Task 5).

**Tech Stack:** Rust 1.98.0, edition 2021, no external dependencies. Three
crates: `ns-syntax` (untouched), `ns-runtime`, `ns-cli`.

**Spec:** [docs/design/compiler.md](../../design/compiler.md), "Planned: the
reference lowering" (Task 2 renames it "The reference lowering") — every
subsection, in particular "What it emits", "The
harness", "Keeping it invisible to a program" and "How the limit counts".
Also [object-model.md](../../design/object-model.md) (the "Planned: the hash is
assigned lazily" paragraph after "The identity hash is a monotonic counter"),
and [execution.md](../../design/execution.md), "Planned: the trampoline". Read
the compiler.md section in full before Task 1.

## Decisions this plan carries

All settled with the user on 2026-09-12 and recorded in the design:

1. **The reference lowering is an independent module, not a mode of
   `Lowerer`.** It shares with `lower.rs` only the bytecode types,
   `lower_literal`, `selector_of`, the parameter counting, the span helpers,
   `builtin_kind_from_name` and the two superclass-clause expression builders
   in `interpreter/nested.rs`. Do not factor anything else out of `lower.rs` to
   "reuse" it — whatever the two share, the comparison cannot check.
2. **The recursion-depth limit exempts exactly the activations inlining
   removes**, marked by `SendInlinable`. **Neither the instruction nor the
   counter is built here** — both arrive with the trampoline (1b). This plan
   only keeps the reference lowering's ordinary sends in one place, where 1b
   will add the marker.
3. **Identity hashes are assigned lazily**, a behaviour change of its own
   (Task 1), which also removes the discarded `Object` allocation in
   `build_builtin_classes`.

## Scope

This is step **C** of the three steps `docs/PROGRESS.md` schedules before M4's
1b. Deliberately **not** in this plan:

- `SendInlinable`, the depth counter, a limit value or its error text — 1b.
- Programs without a recorded answer (generated programs, fuzzing), and a
  direct run-against-run comparison, which only they would need.
- Running `ns-runtime`'s integration tests under the reference lowering — they
  test code both lowerings share (the design says why).
- Adding modules to `corpus/runnable.toml`.
- The two error messages that name a milestone, and `ClosureData.params` —
  still "Left, deliberately" in PROGRESS.

## Global Constraints

- Rust 1.98.0, edition 2021. **No new external dependency.**
- All code, identifiers, comments, doc comments and commit messages in English.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`,
  `refactor:`). **Every commit — including one made by a subagent — ends with
  exactly these two trailer lines:**
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
  ```
- Never override git author or committer identity.
- Work on the branch `feature/reference-lowering`, already checked out in the
  main working copy (no `EnterWorktree` isolation). It already carries the
  design commit (`docs: design the reference lowering`). Commit this plan
  there before Task 1 (`docs: plan step C, the reference lowering`).
- `cargo fmt --all` before every commit; `cargo clippy --workspace --all-targets`
  must be warning-free; `cargo test --workspace` must pass at every commit.
- Both corpus gates hold at every commit:
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`, and
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`.
- **Only Task 1 changes behaviour, and only `hash` values.** No existing file
  under `tests-fixtures/*.expected`, no expectation in
  `crates/ns-cli/tests/program_outcomes.rs`, no row of `corpus/runnable.toml`
  and no existing `*.snap` file under `crates/*/tests/snapshots/` may change in
  any task. Task 1 adds one fixture; Task 3 adds two snapshot files.
- **A divergence between the lowerings is a finding, never a golden to
  update.** If a program answers differently under the two, stop. Establish
  from `newspeak-spec.pdf` and the design which answer is right, fix the
  lowering that is wrong in a commit of its own with a test that pins it, and
  report it. Never change a recorded answer because one lowering now produces
  it (`CLAUDE.md`, "Program outcomes are pinned, not re-derived").
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvalResult`.
  Tests unwrap through a local `expect_ok` helper — copy the one in
  `crates/ns-runtime/tests/vm.rs`.
- Object, frame and activation access goes through
  `crates/ns-runtime/src/heap.rs`.
- Never copy dispatch logic, lookup logic or error text out of `interpreter/`
  into `vm.rs` or `compiler/`.
- A wire tag is assigned once and never renumbered or reused
  (`crates/ns-runtime/src/image/opcodes.rs`). **No task here adds, changes or
  retires a tag.**
- Compiling an already-resolved module can never raise a `NewspeakError`; a
  compiler-invariant violation is `panic!`/`assert!`/`debug_assert!`.
- Never judge performance from a debug build.
- Design documents in `docs/design/` are living and describe the current state
  at every commit; this plan is an execution record — leave it as written once
  complete.

## Measurement protocol

Task 1 touches allocation, the hottest path there is, and is measured with this
protocol **after** its commit, against the commit before it. Task 5 runs it
once more against `master`.

```powershell
git worktree add ..\ns-next-before HEAD~1   # Task 5: `master` instead of HEAD~1
cargo build --release --manifest-path ..\ns-next-before\Cargo.toml
cargo build --release
foreach ($round in 1..3) {
    foreach ($src in 'bench_loop', 'bench_send', 'bench_block') {
        "--- round $round, $src, before"
        ..\ns-next-before\target\release\ns-next.exe bench "tests-fixtures\$src.ns"
        "--- round $round, $src, after"
        .\target\release\ns-next.exe bench "tests-fixtures\$src.ns"
    }
}
```

Keep the worktree for the byte-identity protocol below, then
`git worktree remove ..\ns-next-before`. Record each `median` (18 numbers) in
`target\c-measurements.md` under a heading naming the task — `target\` is
ignored by git, so the file survives between tasks without being committed;
Task 5 collects it. Close other heavy programs first.

**Decision rule:** Task 1 is expected to leave every source unchanged or make it
slightly faster (an allocation no longer increments a counter). If any source's
"after" medians are slower than its "before" medians by more than the spread
within either group, stop and investigate before Task 2.

## Byte-identity protocol

Tasks 1, 2 and 3 must not change a byte the optimized lowering writes into a
**stateless binary** (Task 1 changes snapshot bytes on purpose: an object never
asked for its hash now records 0). Tasks 2 and 3 must not change a byte of a
**snapshot** either. Run with a "before" binary built from `HEAD~1` as in the
measurement protocol (Tasks 2 and 3 build it only for this):

```powershell
$out = "target\c-identity"; New-Item -ItemType Directory -Force $out | Out-Null
$bad = 0; $compared = 0
$exts = if ($task -eq 1) { @('nsb') } else { @('nsb', 'nsi') }
foreach ($f in Get-ChildItem tests-fixtures\*.ns) {
    foreach ($bin in 'before', 'after') {
        $exe = if ($bin -eq 'before') { '..\ns-next-before\target\release\ns-next.exe' } else { '.\target\release\ns-next.exe' }
        Remove-Item "$out\$($f.BaseName).$bin.*" -ErrorAction SilentlyContinue
        & $exe build $f.FullName "$out\$($f.BaseName).$bin.nsb" *> $null
        & $exe snapshot $f.FullName "$out\$($f.BaseName).$bin.nsi" *> $null
    }
    foreach ($ext in $exts) {
        $a = "$out\$($f.BaseName).before.$ext"; $b = "$out\$($f.BaseName).after.$ext"
        if ((Test-Path $a) -ne (Test-Path $b)) { $bad++; "ONLY ONE BINARY WROTE: $($f.Name) .$ext" }
        elseif (Test-Path $a) {
            $compared++
            if ((Get-FileHash $a).Hash -ne (Get-FileHash $b).Hash) { $bad++; "DIFFERS: $($f.Name) .$ext" }
        }
    }
}
"compared $compared artifacts, $bad differing"
```

Set `$task = 1` for Task 1 and `$task = 2` (or 3) otherwise. Expected: Task 1
`compared 23 artifacts, 0 differing` (22 fixtures before Task 1 plus the one it
adds; the "before" binary builds the new fixture too); Tasks 2 and 3
`compared 46 artifacts, 0 differing`. Neither binary is passed `--lowering`,
so both use the optimized lowering.

## File Structure

| File | Responsibility after this plan |
|---|---|
| `crates/ns-runtime/src/heap.rs` | `alloc_object` assigns no hash; `ensure_identity_hash` assigns one on first request; `identity_hash` reads what is stored. No `alloc_placeholder_object`. |
| `crates/ns-runtime/src/builtins/object_class.rs` | `hash` goes through `ensure_identity_hash`. |
| `crates/ns-runtime/src/interpreter/mod.rs` | `Interpreter.lowering`, `bootstrap_with(Lowering)`; `build_builtin_classes` allocates nothing it discards; no `alloc_identity_hash` delegate. |
| `crates/ns-runtime/src/compiler/mod.rs` | `Lowering`; `compile_method` and `compile_mixin_bodies` take it and route to `lower.rs` or `reference.rs`. |
| `crates/ns-runtime/src/compiler/reference.rs` (new) | The reference lowering, its shape check and its unit tests. |
| `crates/ns-runtime/src/mixin.rs`, `interpreter/instantiate.rs` | Thread the `Lowering` from the interpreter to the compiler. |
| `crates/ns-runtime/tests/reference_lowering.rs` (new) | Programs answer alike under both lowerings; the initializer's shape; every resolvable corpus module compiles under both. |
| `crates/ns-cli/src/main.rs`, `source.rs`, `commands/{run,snapshot,build,disasm,bench}.rs` | `--lowering` on `run`, `snapshot`, `build`, `disasm`. |
| `crates/ns-cli/src/commands/run_corpus.rs` | Every row four ways. |
| `crates/ns-cli/tests/lowering.rs` (new), `disasm_snapshots.rs` | The switch, and the reference lowering's listings pinned. |
| `crates/ns-cli/tests/{run_fixtures,program_outcomes,build_load}.rs`, `cli.rs` | Every recorded program under both lowerings. |
| `tests-fixtures/identity_hash.ns`, `.expected` (new) | Hashes are drawn in request order. |
| `CLAUDE.md`, `docs/design/*.md`, `docs/PROGRESS.md` | Describe the result. |

---

### Task 1: Identity hashes are assigned on first request

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (the `Heap` struct's
  `next_identity_hash` doc, `alloc_identity_hash`, `alloc_object`,
  `alloc_placeholder_object` (deleted), `alloc_self_referential_class`'s doc,
  `push_object`'s doc, `identity_hash`, `object_identity_key`'s doc,
  `set_identity_hash`'s doc, unit tests)
- Modify: `crates/ns-runtime/src/builtins/object_class.rs:17-19,53-59`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:80-98` and `build_builtin_classes` (`:784-856`)
- Modify: `crates/ns-runtime/src/image/decode.rs:1910-1916` and the
  `IdentityHashExhausted` doc (`:224-232`)
- Modify: `crates/ns-runtime/src/dispatch.rs:298` (test helper)
- Modify: `crates/ns-runtime/tests/image.rs` (three tests, one new)
- Create: `tests-fixtures/identity_hash.ns`, `tests-fixtures/identity_hash.expected`
- Modify: `crates/ns-cli/tests/run_fixtures.rs` (floor 22 → 23)
- Modify: `docs/design/object-model.md`, `memory.md`, `image-format.md`,
  `compiler.md`, `README.md`

**Interfaces:**
- Produces: `Heap::ensure_identity_hash(&mut self, obj: ObjRef) -> u32` —
  answers `obj`'s hash, assigning the next one from the counter first if none
  is stored. `Heap::identity_hash(&self, obj: ObjRef) -> u32` keeps its
  signature and now answers the *stored* value, `0` when none is assigned.
  `Heap::alloc_placeholder_object` and `Interpreter::alloc_identity_hash` no
  longer exist; `Heap::alloc_identity_hash` becomes private.

- [ ] **Step 1: Write the failing heap test**

Append to the `#[cfg(test)] mod tests` in `crates/ns-runtime/src/heap.rs`:

```rust
    /// A hash is drawn from the counter when something first asks for one,
    /// not when the object is allocated, so allocation order is not
    /// observable (`docs/design/object-model.md`).
    #[test]
    fn an_identity_hash_is_assigned_on_first_request_not_at_allocation() {
        let mut heap = Heap::new();
        let class = heap.alloc_self_referential_class(ObjectData::Instance(vec![]));
        let a = heap.alloc_object(class, ObjectData::Instance(Vec::new()));
        let b = heap.alloc_object(class, ObjectData::Instance(Vec::new()));
        assert_eq!(heap.identity_hash(a), 0, "allocation assigns no hash");
        assert_eq!(heap.identity_hash(b), 0, "allocation assigns no hash");
        assert_eq!(heap.ensure_identity_hash(b), 1, "the first request draws the first number");
        assert_eq!(heap.ensure_identity_hash(a), 2);
        assert_eq!(heap.ensure_identity_hash(b), 1, "a second request answers the same hash");
        assert_eq!(heap.identity_hash(b), 1, "the assigned hash is stored");
    }
```

- [ ] **Step 2: Write the failing fixture**

Create `tests-fixtures/identity_hash.ns`:

```
Newspeak3
'Test'
class IdentityHash usingPlatform: p = Object new (| t ::= p transcript. |) (
    public class Thing = Object new () ()
    public main: platform args: args = (
        | a b |
        a := Thing new.
        b := Thing new.
        t show: b hash printString; newLine.
        t show: a hash printString; newLine.
        t show: b hash printString; newLine.
        t show: (a == b) printString; newLine.
    )
)
```

Create `tests-fixtures/identity_hash.expected` with exactly these four lines,
each ending in `\n`:

```
1
2
1
false
```

Why this is the right answer, derived from the design rather than from a run:
the counter starts at 1 (`Heap::new`), nothing before `main:args:` asks any
object for its hash, and hashes are handed out in request order — `b` asks
first. If the implemented binary prints anything else, **stop**: something is
asking for a hash that should not be, and that is the defect to find.

In `crates/ns-cli/tests/run_fixtures.rs`, raise the floor: `checked >= 22` →
`checked >= 23`, and in its message "every one of the 22" → "every one of the
23".

- [ ] **Step 3: Run both to verify they fail**

Run: `cargo test -p ns-runtime --lib heap::tests::an_identity_hash_is_assigned_on_first_request_not_at_allocation`
Expected: compile error — no method `ensure_identity_hash` on `Heap`.

Run: `cargo test -p ns-cli --test run_fixtures`
Expected: FAIL — `identity_hash.ns produced unexpected stdout` (today it prints
allocation-counter values, not `1 2 1`).

- [ ] **Step 4: Make the heap assign lazily**

In `crates/ns-runtime/src/heap.rs`:

Replace the `next_identity_hash` field's doc comment with:

```rust
    /// The next identity hash to hand out. Hashes are assigned lazily, the
    /// first time `ensure_identity_hash` is asked for one, and never derived
    /// from an address or an arena index, so identity survives both a moving
    /// collector and a snapshot (`docs/design/memory.md`) and allocation
    /// order stays unobservable (`docs/design/object-model.md`).
    next_identity_hash: u32,
```

Replace `alloc_identity_hash` (doc and body) with a private function:

```rust
    /// Draws the next number from the counter. Saturating, not wrapping and
    /// not panicking: identity hashes are not required to be unique -- object
    /// identity is the arena index (`ObjRef::same`), never the hash -- so a
    /// world that exhausts the counter is better served by handing out a
    /// shared `u32::MAX` than by aborting the process. Reachable in *one*
    /// request once `image::load` has set the counter from a file:
    /// `advance_identity_hash_past` puts it wherever the image's maximum
    /// says. Never answers 0, which means "not assigned": the counter starts
    /// at 1 and only moves up.
    fn alloc_identity_hash(&mut self) -> u32 {
        let h = self.next_identity_hash;
        self.next_identity_hash = self.next_identity_hash.saturating_add(1);
        h
    }
```

Replace `alloc_object` and delete `alloc_placeholder_object` entirely:

```rust
    /// Allocates `data` under `class`. It carries no identity hash until
    /// something asks for one (`ensure_identity_hash`), so allocating draws
    /// nothing from the counter.
    pub fn alloc_object(&mut self, class: ClassRef, data: ObjectData) -> ObjRef {
        self.push_object(Object {
            class,
            identity_hash: 0,
            data,
        })
    }
```

Change the section comment above them from "goes through one of the three
methods below" to "goes through one of the two methods below". In
`alloc_self_referential_class`'s doc, replace the paragraph beginning
"Identity hash fixed at 0, like `alloc_placeholder_object`" with:

```rust
    /// Like every object, it carries no identity hash until one is asked for.
```

In `push_object`'s doc, "Where `alloc_object` and `alloc_placeholder_object`
enter an object" → "Where `alloc_object` enters an object".

Replace `identity_hash` with the reader and the assigning request:

```rust
    /// The identity hash stored on `obj`, 0 if nothing has asked for one yet.
    /// A read never assigns one: the image encoder writes exactly what is
    /// stored, so snapshotting a world does not change it.
    pub fn identity_hash(&self, obj: ObjRef) -> u32 {
        self.check_object_origin(obj);
        self.objects[obj.index as usize].identity_hash
    }

    /// `obj`'s identity hash, drawn from the counter the first time anything
    /// asks -- `Object>>hash` is the one caller. Assigned lazily, as Cog/Spur
    /// (`hashBitsOf:`, which calls `newHashBitsOf:` when the bits are 0) and
    /// Primordial Soup (`Object_identityHash`) do, so that allocation order is
    /// unobservable: the reference lowering allocates closures the optimized
    /// one does not, and closure conversion and a JIT's escape analysis will
    /// add and remove allocations the same way (`docs/design/object-model.md`).
    pub fn ensure_identity_hash(&mut self, obj: ObjRef) -> u32 {
        self.check_object_origin(obj);
        let stored = self.objects[obj.index as usize].identity_hash;
        if stored != 0 {
            return stored;
        }
        let hash = self.alloc_identity_hash();
        self.objects[obj.index as usize].identity_hash = hash;
        hash
    }
```

In `object_identity_key`'s doc, replace "because two placeholder objects may
legitimately share hash 0 (`alloc_placeholder_object`,
`alloc_self_referential_class`), which would fold" with "because every object
nothing has asked for its hash stores 0, which would fold".

In `set_identity_hash`'s doc, "without actually allocating four billion
objects first" → "without actually hashing four billion objects first".

In the unit test module, the test whose doc mentions
"`alloc_placeholder_object` and `alloc_self_referential_class` -- can" (around
`:1473`) builds `a` and `b` with `heap.alloc_placeholder_object(...)`: change
both calls to `heap.alloc_object(...)` and change that doc's parenthesis to
"(every object nothing has asked for its hash)". Its assertions stay: both
still store 0.

- [ ] **Step 5: Route `hash` through the request**

In `crates/ns-runtime/src/builtins/object_class.rs`:

```rust
        "hash" => Some(Ok(Value::SmallInt(
            identity_hash_of(&mut interp.heap, &receiver) as i64,
        ))),
```

```rust
fn identity_hash_of(heap: &mut Heap, value: &Value) -> u32 {
    match value {
        Value::Obj(obj) => heap.ensure_identity_hash(*obj),
        _ => 0, // primitives are values and get no distinct identity
                // hash (revisit if a fixture needs one)
    }
}
```

- [ ] **Step 6: Remove what only existed for the eager counter**

In `crates/ns-runtime/src/interpreter/mod.rs`, delete
`Interpreter::alloc_identity_hash` (the "thin delegate", `:80-84`; nothing
calls it). In `advance_identity_hash_past`'s doc, replace
"`alloc_identity_hash`'s own saturation is what keeps the *next* allocation in
range" with "the counter's own saturation is what keeps the *next* hash in
range".

Replace the loop of `build_builtin_classes` (from `let mut classes = ...` to
`classes` at the end) with:

```rust
    let mut classes = Vec::with_capacity(kinds.len());
    for kind in kinds {
        // `builtin_class` indexes by discriminant, so the vector must be
        // filled in exactly that order.
        debug_assert_eq!(classes.len(), kind as usize);
        if kind == BuiltinClass::Object {
            // The self-referential class every builtin has as its `.class`.
            classes.push(class_of_class);
            continue;
        }
        let application = interp.apply_mixin(&builtin_mixin(kind), Some(&object_application));
        classes.push(interp.heap.alloc_object(
            class_of_class,
            ObjectData::Class(ClassData {
                application,
                enclosing: None,
                declaring_class: None,
                superclass: Some(class_of_class),
                builtin: Some(kind),
                metaclass: None,
            }),
        ));
    }
    classes
```

`object_application` is now used only as a reference in the loop; if clippy
reports a redundant `.clone()` elsewhere in the function, remove it.

In `crates/ns-runtime/src/image/decode.rs`, the placeholder allocation becomes
`.alloc_object(placeholder_class, ObjectData::Instance(Vec::new()))`. In the
`IdentityHashExhausted` doc, "only move the overflow to `alloc_identity_hash`'s
own `+= 1`" → "only move the overflow to the counter's own `+= 1`".

In `crates/ns-runtime/src/dispatch.rs`'s test helper `class_with_method`,
`heap.alloc_placeholder_object(` → `heap.alloc_object(`.

- [ ] **Step 7: Bring the image tests to the lazy rule, and pin the new case**

In `crates/ns-runtime/tests/image.rs`:

`identity_hashes_are_preserved_and_the_maximum_is_reported` — assign the two
hashes explicitly, in an order that makes the first root the higher one:

```rust
#[test]
fn identity_hashes_are_preserved_and_the_maximum_is_reported() {
    // Two objects, the higher hash first: the object table is walked in
    // root order, so the maximum is *not* the last one seen and
    // `assert_eq!` can tell a real fold from a stale field or a running
    // counter.
    let (interp, _c, original, decoded, max) = round_trip_graph(EMPTY, |interp, _| {
        let first = interp.new_string("x".to_string());
        let second = interp.new_string("y".to_string());
        // Requested in this order, so `second` holds the higher hash.
        interp.heap_mut().ensure_identity_hash(first);
        interp.heap_mut().ensure_identity_hash(second);
        vec![Value::Obj(second), Value::Obj(first)]
    });
    let heap = interp.heap_ref();
    let higher = heap.identity_hash(*obj(&original[0]));
    let lower = heap.identity_hash(*obj(&original[1]));
    assert!(higher > lower, "hashes are handed out in request order");
    assert_eq!(heap.identity_hash(*obj(&decoded[0])), higher);
    assert_eq!(heap.identity_hash(*obj(&decoded[1])), lower);
    assert_eq!(max, higher);
}
```

In the test ending in `"no hash collision with loaded objects"` (around
`:1960-2016`): move the hash request *before* `image::snapshot`, replacing
`let max_hash = interp.heap_ref().identity_hash(*obj(&instance));` (after the
snapshot) with, before the `let platform = ...` line,

```rust
    let max_hash = interp.heap_mut().ensure_identity_hash(*obj(&instance));
```

and change the final assertion to request the fresh object's hash:

```rust
    let fresh = loaded.interp.new_string("fresh".to_string());
    assert!(
        loaded.interp.heap_mut().ensure_identity_hash(fresh) > max_hash,
        "no hash collision with loaded objects"
    );
```

In `an_identity_hash_one_below_the_ceiling_loads_and_keeps_allocating`, replace
the comment's "where `alloc_identity_hash`'s own `+= 1` used to overflow on the
very first allocation after the load" with "where the counter's own `+= 1`
would overflow on the very first hash assigned after the load", and the last
four statements with:

```rust
    let first = loaded.interp.new_string("a".to_string());
    let second = loaded.interp.new_string("b".to_string());
    let heap = loaded.interp.heap_mut();
    assert_eq!(heap.ensure_identity_hash(first), u32::MAX);
    assert_eq!(
        heap.ensure_identity_hash(second),
        u32::MAX,
        "an exhausted counter shares its last hash instead of overflowing"
    );
```

Add, after that test:

```rust
/// An object nothing asked for its hash is written as 0 and decoded as 0 --
/// still unassigned -- and the first request after the load draws a number
/// past every hash the image carried, so it cannot collide with one.
#[test]
fn an_unhashed_object_round_trips_unassigned_and_draws_past_the_image_maximum() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n) : ()";
    let (mut interp, class) = load(src);
    let hashed = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let unhashed = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    let assigned = interp.heap_mut().ensure_identity_hash(*obj(&hashed));
    let platform = interp.platform();
    let bytes = image::snapshot(
        interp.heap_ref(),
        Some(src),
        &Origin::File(PathBuf::from("t.ns")),
        class,
        &platform,
        &[hashed, unhashed],
    )
    .expect("snapshot");
    assert_eq!(
        interp.heap_ref().identity_hash(*obj(&unhashed)),
        0,
        "snapshotting assigns no hash"
    );
    let mut loaded = image::load(&bytes).expect("load");
    let (kept, fresh) = (*obj(&loaded.roots[0]), *obj(&loaded.roots[1]));
    let heap = loaded.interp.heap_mut();
    assert_eq!(heap.identity_hash(kept), assigned, "an assigned hash is preserved");
    assert_eq!(heap.identity_hash(fresh), 0, "an unassigned hash stays unassigned");
    assert!(heap.ensure_identity_hash(fresh) > assigned);
}
```

- [ ] **Step 8: Run everything**

Run: `cargo test --workspace`
Expected: PASS, including the new heap test, the new image test and
`run_fixtures` (which now prints `1 2 1 false` for `identity_hash.ns`).

Run: `cargo clippy --workspace --all-targets` — warning-free.
Run both corpus gates — unchanged counts.

- [ ] **Step 9: Bring the documents to the new state**

`docs/design/object-model.md`: replace the paragraph starting "**The identity
hash is a monotonic counter assigned at allocation**" and the following
"**Planned: the hash is assigned lazily**" paragraph with one paragraph:

```markdown
**The identity hash is drawn from a monotonic counter the first time `hash`
asks for one**, never derived from an address and never from the arena index,
and 0 means "not yet assigned" (`Heap::ensure_identity_hash`). An address- or
index-derived hash would break the moment a moving collector relocates the
object or a `become:` swaps identities, and identity has to be stable across a
snapshot; see [memory.md](memory.md). Assigning it lazily, as Cog/Spur
(`hashBitsOf:`) and Primordial Soup (`Object_identityHash`) do, keeps
allocation order unobservable: the reference lowering allocates closures the
optimized one does not, and closure conversion and a JIT's escape analysis will
add and remove allocations the same way, so a hash assigned at allocation would
change a program's `hash` values under any of them. The encoder writes the
stored value without assigning one, and the decoder keeps every value, 0
included.
```

In its Decisions list, "**Identity hashes come from a counter, not from
addresses**, so identity survives a moving collector and a snapshot." →
"**Identity hashes come from a counter, not from addresses, and are assigned on
first request**, so identity survives a moving collector and a snapshot, and
allocation order is unobservable."

`docs/design/memory.md`: "Every object carries an `identity_hash` assigned
monotonically at allocation, never derived from its address." → "Every object's
`identity_hash` is drawn from a monotonic counter the first time it is asked
for, never derived from its address."; delete the sentence "Assigning it
lazily, on the first `hash`, is planned ([object-model.md](object-model.md))."
In its Decisions list, add ", and assigned on first request" after "come from a
counter, not an address".

`docs/design/image-format.md`: replace the sentence beginning "Once hashes are
assigned lazily" with "A record's 0 means \"not yet assigned\" and is preserved
the same way ([object-model.md](object-model.md))."

`docs/design/compiler.md`, "Keeping it invisible to a program": "Identity
hashes become lazily assigned" → "Identity hashes are lazily assigned"; in
"Order of work", item 1 becomes "1. **Lazy identity hashes** — done." followed
by its existing explanation unchanged.

`docs/design/README.md`: the object-model row's "Planned next" goes back to
`` `become:` ``.

- [ ] **Step 10: Commit**

```bash
cargo fmt --all
git add -A crates tests-fixtures docs
git commit -F- <<'EOF'
feat!: assign identity hashes on first request, not at allocation

`hash` drew its number at allocation, so every allocation was observable
through it -- and the reference lowering allocates a closure for every
block the optimized lowering inlines. An object now carries 0 until
`hash` first asks, as in Cog/Spur and Primordial Soup; the encoder
writes what is stored, the decoder keeps it. `hash` values change,
which the specification leaves open and no recorded outcome pinned.

The discarded `Object` allocation in `build_builtin_classes`, kept only
because it shifted every later hash, is gone, and so is
`alloc_placeholder_object`, which is now `alloc_object`.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
EOF
```

- [ ] **Step 11: Measure and check byte identity**

Run the measurement protocol, then the byte-identity protocol with `$task = 1`.
Expected: `compared 23 artifacts, 0 differing`, and no source slower beyond the
decision rule. Record both in `target\c-measurements.md` under "Task 1".

---

### Task 2: The reference lowering in the runtime

**Files:**
- Create: `crates/ns-runtime/src/compiler/reference.rs`
- Modify: `crates/ns-runtime/src/compiler/mod.rs`
- Modify: `crates/ns-runtime/src/compiler/lower.rs:910` (`lower_literal` becomes `pub(super)`)
- Modify: `crates/ns-runtime/src/mixin.rs:63-78`
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs:22-30,102-180`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:40-74`
- Modify: `crates/ns-runtime/src/lib.rs`
- Modify: `crates/ns-runtime/src/dispatch.rs:276-279` (test helper)
- Create: `crates/ns-runtime/tests/reference_lowering.rs`
- Modify: `docs/design/compiler.md`, `execution.md`, `docs/PROGRESS.md`,
  `CLAUDE.md` (the renamed heading)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `pub enum ns_runtime::Lowering { Optimized, Reference }` (declared in
    `compiler/mod.rs`, re-exported from `lib.rs`), `Debug, Clone, Copy,
    PartialEq, Eq`.
  - `pub fn Interpreter::bootstrap_with(lowering: Lowering) -> Interpreter`;
    `Interpreter::bootstrap()` is `bootstrap_with(Lowering::Optimized)`.
  - `pub fn compiler::compile_method(method: &ast::MethodDecl, bindings:
    &Rc<HashMap<Span, Binding>>, lowering: Lowering) -> CompiledCode`.
  - `pub(crate) fn compiler::compile_mixin_bodies(mixin: &mut Mixin, bindings:
    &Rc<HashMap<Span, Binding>>, lowering: Lowering)`.
  - `pub fn MixinMethod::from_decl(decl: Rc<ast::MethodDecl>, bindings:
    &Rc<HashMap<Span, Binding>>, lowering: Lowering) -> MixinMethod`.

- [ ] **Step 1: Write the failing integration tests**

Create `crates/ns-runtime/tests/reference_lowering.rs`:

```rust
//! The reference lowering (`docs/design/compiler.md`, "The reference
//! lowering"): programs answer alike under both lowerings, the reference
//! lowering's bodies have the shape the design promises, and every corpus
//! module the resolver accepts compiles under both.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use ns_runtime::bytecode::Instr;
use ns_runtime::{ClassRef, Interpreter, Lowering, Signal, SourceOrigin, Symbol, Value};

fn load(src: &str, lowering: Lowering) -> (Interpreter, ClassRef) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap_with(lowering);
    let class = interp.load_module(&resolved);
    (interp, class)
}

/// `Signal` carries no `Debug` impl, so `Result::expect` is unusable on an
/// `EvalResult` -- unwrap by hand.
fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

/// Every construct whose lowering the two differ on: inlined conditionals,
/// loops and `and:`/`or:`, a `^` out of an inlined block, slot
/// initializers, a cascade, a chained assignment.
const PROGRAMS: &str = "Newspeak3\n'T'\nclass Programs = Object new (\n\
    \t| a = 3. b = a + 4. |\n) (\n\
    \tfact: n = ( ^n = 0 ifTrue: [1] ifFalse: [n * (self fact: n - 1)] )\n\
    \tpublic factorial = ( ^self fact: 10 )\n\
    \tpublic loop = ( | i sum | i := 0. sum := 0. [i < 10] whileTrue: [i := i + 1. sum := sum + i]. ^sum )\n\
    \tpublic early = ( | c | c := OrderedCollection new. c add: 1; add: 2; add: 3. c do: [:e | e = 2 ifTrue: [^e * 100]]. ^0 )\n\
    \tpublic logic = ( ^(((3 < 4) and: [4 < 5]) and: [(3 > 4) or: [true]]) ifTrue: [1] ifFalse: [0] )\n\
    \tpublic slots = ( ^a + b )\n\
    \tpublic cascade = ( | c n | c := OrderedCollection new. c add: 1; add: 2; add: 3. n := 0. c do: [:e | n := n + e]. ^n )\n\
    \tpublic chained = ( | t u | u := t := 5. ^t + u )\n\
    ) : ()";

/// Each answer is worked out by hand from the program, not recorded from a
/// run: 10!, 1 + ... + 10, the `^` out of `do:` at the element 2, a true
/// conjunction, 3 + 7, 1 + 2 + 3, 5 + 5.
#[test]
fn every_program_answers_the_same_under_both_lowerings() {
    let expected = [
        ("factorial", 3_628_800),
        ("loop", 55),
        ("early", 200),
        ("logic", 1),
        ("slots", 10),
        ("cascade", 6),
        ("chained", 10),
    ];
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(PROGRAMS, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        for (selector, answer) in expected {
            let value = expect_ok(interp.send(instance, Symbol::intern(selector), vec![]), selector);
            assert!(
                matches!(value, Value::SmallInt(n) if n == answer),
                "{selector} under {lowering:?} did not answer {answer}"
            );
        }
    }
}

/// An initializer is each own slot's initializer followed by `InitSlot`, one
/// statement apiece, then the `initExprs`, then `self` -- with nothing inlined
/// and every value in a register of its own.
#[test]
fn a_reference_initializer_initializes_each_slot_then_answers_self() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n\t| a = 1. b ::= 2. |\n\ta printString.\n) (\n) : ()";
    let (interp, class) = load(src, Lowering::Reference);
    let mixin = interp.heap_ref().class_application(class).expect("a class").mixin.clone();
    let code = mixin.initializer_code.clone().expect("an initializer");
    assert!(matches!(code.instrs[0], Instr::LoadLiteral { dst: 0, .. }));
    assert_eq!(code.instrs[1], Instr::InitSlot { src: 0, own_index: 0 });
    assert!(matches!(code.instrs[2], Instr::LoadLiteral { dst: 1, .. }));
    assert_eq!(code.instrs[3], Instr::InitSlot { src: 1, own_index: 1 });
    assert!(matches!(
        &code.instrs[4],
        Instr::SendLexical { selector, arg_regs, dst: 2, .. }
            if *selector == Symbol::intern("a") && arg_regs.is_empty()
    ));
    assert!(matches!(
        &code.instrs[5],
        Instr::SendPublic { selector, receiver_reg: 2, dst: 3, .. }
            if *selector == Symbol::intern("printString")
    ));
    assert_eq!(code.instrs[6], Instr::LoadSelf { dst: 4 });
    assert_eq!(code.instrs[7], Instr::ImplicitReturn { reg: 4 });
    assert_eq!(code.instrs.len(), 8);
    assert_eq!(code.statement_pc, vec![(0, 0..2), (1, 2..4), (2, 4..6)]);
}

fn collect_ns_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read corpus directory").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ns_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ns") {
            out.push(path);
        }
    }
}

/// "Compilation cannot fail" (`docs/design/compiler.md`) over the whole
/// corpus rather than the six modules that run, under both lowerings -- which
/// also runs the reference lowering's shape check on every body it compiles.
/// Loading compiles every method and every per-mixin body, nothing runs.
#[test]
fn every_resolvable_corpus_module_compiles_under_both_lowerings() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules");
    let mut files = Vec::new();
    collect_ns_files(&dir, &mut files);
    let mut compiled = 0;
    for path in &files {
        let source = std::fs::read_to_string(path).expect("read module");
        let Ok(module) = ns_syntax::parse(&source) else {
            continue; // the blacklisted file
        };
        let Ok(resolved) = ns_runtime::resolve(
            module,
            source.as_str().into(),
            SourceOrigin::File(path.clone()),
        ) else {
            continue;
        };
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            Interpreter::bootstrap_with(lowering).load_module(&resolved);
        }
        compiled += 1;
    }
    // 189 of the 211 modules resolve today (measured while planning). A
    // floor, never lowered: a module that stops resolving or stops being found
    // would otherwise drop out of this check silently.
    assert!(
        compiled >= 189,
        "expected at least 189 resolvable corpus modules, compiled {compiled} -- \
         raise this floor when more resolve, never lower it"
    );
}
```

- [ ] **Step 2: Write the reference lowering's unit tests**

Create `crates/ns-runtime/src/compiler/reference.rs` containing, for now, only
this test module (the implementation follows in Step 4):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Compiles instance-side method `index` of `src` under the reference
    /// lowering, against the bindings the resolver really produces.
    fn compile(src: &str, index: usize) -> CompiledCode {
        let module = ns_syntax::parse(src).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(src),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        compile_method(
            &resolved.module.root_class.instance_side.methods[index],
            &resolved.bindings,
        )
    }

    fn method(decl: &str) -> String {
        format!("Newspeak3\n'T'\nclass X = Object new (\n) (\n  {decl}\n) : ()")
    }

    fn send(selector: &str, receiver_reg: u16, arg_regs: Vec<u16>, dst: u16, id: u16) -> Instr {
        Instr::SendPublic {
            selector: Symbol::intern(selector),
            receiver_reg,
            arg_regs,
            dst,
            call_site: CallSite { call_site_id: id },
        }
    }

    fn closure_code(instr: &Instr) -> &CompiledCode {
        let Instr::MakeClosure { code, .. } = instr else {
            panic!("expected a MakeClosure, got {instr:?}");
        };
        code
    }

    fn no_branch(code: &CompiledCode) -> bool {
        code.instrs.iter().all(|i| {
            !matches!(
                i,
                Instr::Jump { .. }
                    | Instr::JumpIfNotBoolean { .. }
                    | Instr::BranchIfFalse { .. }
                    | Instr::BranchIfTrue { .. }
            )
        })
    }

    #[test]
    fn reading_a_local_copies_it_into_a_fresh_register() {
        let code = compile(&method("foo: x = ( ^x )"), 0);
        assert_eq!(code.instrs, vec![Instr::Move { dst: 1, src: 0 }, Instr::Return { reg: 1 }]);
        assert_eq!(code.num_registers, 2);
    }

    #[test]
    fn no_register_is_reused_across_statements() {
        let code = compile(&method("foo = ( 1. 2 )"), 0);
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(2) },
                Instr::LoadSelf { dst: 2 },
                Instr::ImplicitReturn { reg: 2 },
            ]
        );
        assert_eq!(code.statement_pc, vec![(0, 0..1), (1, 1..2)]);
        assert_eq!(code.num_registers, 3);
    }

    #[test]
    fn if_true_if_false_is_a_real_send_with_two_real_closures() {
        let code = compile(&method("foo: x = ( ^x ifTrue: [1] ifFalse: [2] )"), 0);
        assert!(no_branch(&code));
        assert_eq!(code.instrs.len(), 5);
        assert_eq!(code.instrs[0], Instr::Move { dst: 1, src: 0 });
        assert!(matches!(code.instrs[1], Instr::MakeClosure { dst: 2, .. }));
        assert!(matches!(code.instrs[2], Instr::MakeClosure { dst: 3, .. }));
        assert_eq!(code.instrs[3], send("ifTrue:ifFalse:", 1, vec![2, 3], 4, 3));
        assert_eq!(code.instrs[4], Instr::Return { reg: 4 });
        assert_eq!(
            closure_code(&code.instrs[1]).instrs,
            vec![
                Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
                Instr::ImplicitReturn { reg: 0 },
            ]
        );
    }

    #[test]
    fn while_true_is_a_real_send_whose_blocks_reach_the_method_through_their_frames() {
        let code = compile(
            &method("foo = ( | n | n := 0. [n < 3] whileTrue: [n := n + 1]. ^n )"),
            0,
        );
        assert!(no_branch(&code));
        assert_eq!(code.instrs[0], Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(0) });
        assert_eq!(code.instrs[1], Instr::StoreLocal { src: 1, slot: 0 });
        assert!(matches!(code.instrs[2], Instr::MakeClosure { dst: 2, .. }));
        assert!(matches!(code.instrs[3], Instr::MakeClosure { dst: 3, .. }));
        assert_eq!(code.instrs[4], send("whileTrue:", 2, vec![3], 4, 4));
        assert_eq!(code.instrs[5], Instr::Move { dst: 5, src: 0 });
        assert_eq!(code.instrs[6], Instr::Return { reg: 5 });
        assert_eq!(code.statement_pc, vec![(0, 0..2), (1, 2..5), (2, 5..7)]);
        assert_eq!(
            closure_code(&code.instrs[2]).instrs,
            vec![
                Instr::LoadOuterLocal { dst: 0, depth: 1, index: 0 },
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(3) },
                send("<", 0, vec![1], 2, 2),
                Instr::ImplicitReturn { reg: 2 },
            ]
        );
        assert_eq!(
            closure_code(&code.instrs[3]).instrs,
            vec![
                Instr::LoadOuterLocal { dst: 0, depth: 1, index: 0 },
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(1) },
                send("+", 0, vec![1], 2, 2),
                Instr::StoreOuterLocal { src: 2, depth: 1, index: 0 },
                Instr::ImplicitReturn { reg: 2 },
            ]
        );
    }

    #[test]
    fn a_return_inside_a_block_the_optimized_lowering_inlines_is_a_non_local_return() {
        let code = compile(&method("foo: x = ( x ifTrue: [^1]. ^2 )"), 0);
        let block = closure_code(&code.instrs[1]);
        assert_eq!(block.kind, CodeKind::Block);
        assert_eq!(
            block.instrs,
            vec![
                Instr::LoadLiteral { dst: 0, literal: BcLiteral::SmallInt(1) },
                Instr::NonLocalReturn { reg: 0 },
            ]
        );
    }

    #[test]
    fn an_assignments_value_is_its_fresh_register_never_the_local() {
        let code = compile(&method("foo = ( | t | ^t := 3 )"), 0);
        assert_eq!(
            code.instrs,
            vec![
                Instr::LoadLiteral { dst: 1, literal: BcLiteral::SmallInt(3) },
                Instr::StoreLocal { src: 1, slot: 0 },
                Instr::Return { reg: 1 },
            ]
        );
    }

    #[test]
    fn a_cascade_evaluates_its_receiver_once_and_answers_the_last_send() {
        let code = compile(&method("foo: x = ( ^x foo; bar )"), 0);
        assert_eq!(
            code.instrs,
            vec![
                Instr::Move { dst: 1, src: 0 },
                send("foo", 1, vec![], 2, 1),
                send("bar", 1, vec![], 3, 2),
                Instr::Return { reg: 3 },
            ]
        );
    }

    #[test]
    fn an_empty_block_answers_nil() {
        let code = compile(&method("foo = ( ^[] )"), 0);
        assert_eq!(
            closure_code(&code.instrs[0]).instrs,
            vec![
                Instr::LoadLiteral { dst: 0, literal: BcLiteral::Nil },
                Instr::ImplicitReturn { reg: 0 },
            ]
        );
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --test reference_lowering`
Expected: compile error — `Lowering` and `Interpreter::bootstrap_with` do not
exist. (`reference.rs` is not yet a module, so its tests are not compiled.)

- [ ] **Step 4: Write the reference lowering**

Insert above the test module in `crates/ns-runtime/src/compiler/reference.rs`:

```rust
//! The reference lowering: a second, deliberately naive `Expr`/`Stmt` ->
//! `Instr` lowering, which exists to be compared against `lower.rs`. Nothing
//! is inlined, every value gets a register nothing else writes, and every
//! read of a local is copied. See `docs/design/compiler.md`, "The reference
//! lowering" -- in particular why this module shares so little with
//! `lower.rs`: whatever the two share, the comparison cannot check.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use ns_syntax::ast::{self, Expr, Stmt};
use ns_syntax::Span;

use super::lower::{lower_literal, selector_of};
use crate::bytecode::{CallSite, CodeKind, CompiledCode, Instr, Literal as BcLiteral};
use crate::mixin::Mixin;
use crate::types::Binding;
use crate::Symbol;

type Bindings = Rc<HashMap<Span, Binding>>;

pub(crate) fn compile_method(method: &ast::MethodDecl, bindings: &Bindings) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Method,
        super::num_args_of(&method.pattern),
        method.body.temps.len() as u16,
        bindings,
    );
    body.statements_and_end(&method.body.stmts);
    body.finish(method.span)
}

pub(crate) fn compile_mixin_bodies(mixin: &mut Mixin, bindings: &Bindings) {
    mixin.initializer_code = Some(Rc::new(initializer(mixin, bindings)));
    mixin.superclass_clause_code = superclass_clause(mixin, bindings).map(Rc::new);
    mixin.superclass_message_code = superclass_message(mixin, bindings).map(Rc::new);
}

/// Each own slot's initializer followed by `InitSlot`, one statement apiece,
/// then the `initExprs`, then `self`. `InitSlot`'s index is the slot's
/// position among the mixin's own slots, never an absolute field index
/// (`docs/design/compiler.md`, "One body per mixin").
fn initializer(mixin: &Mixin, bindings: &Bindings) -> CompiledCode {
    let mut body = Body::new(
        CodeKind::Initializer,
        super::factory_num_args(mixin),
        0,
        bindings,
    );
    let mut source: Option<Span> = None;
    for (own_index, slot) in mixin.slots.iter().enumerate() {
        let Some(decl) = &slot.decl else { continue };
        let Some(init) = &decl.init else { continue };
        let start = body.instrs.len();
        let src = body.value(init);
        body.emit(Instr::InitSlot {
            src,
            own_index: own_index as u32,
        });
        body.record_statement(start);
        source = Some(super::merge_span(source, decl.span));
    }
    for stmt in &mixin.init_exprs {
        source = Some(super::merge_span(source, super::stmt_span(stmt)));
    }
    body.statements_and_end(&mixin.init_exprs);
    body.finish(source.unwrap_or(Span::new(0, 0)))
}

/// The superclass clause as a single `^ec`. The expression is
/// `interpreter::nested`'s, the one definition of what a clause evaluates.
fn superclass_clause(mixin: &Mixin, bindings: &Bindings) -> Option<CompiledCode> {
    let clause = mixin.decl.as_ref()?.superclass.as_ref()?;
    let stmts = [Stmt::Return(
        crate::interpreter::nested::superclass_clause_expr(clause),
    )];
    let mut body = Body::new(CodeKind::SuperclassClause, 0, 0, bindings);
    body.statements_and_end(&stmts);
    Some(body.finish(clause.span))
}

/// The clause's message arguments as a single `^{arg. arg}`, run with the
/// declaring class's own factory arguments.
fn superclass_message(mixin: &Mixin, bindings: &Bindings) -> Option<CompiledCode> {
    let clause = mixin.decl.as_ref()?.superclass.as_ref()?;
    let args = crate::interpreter::nested::superclass_clause_message_args_expr(clause)?;
    let mut body = Body::new(
        CodeKind::SuperclassMessage,
        super::factory_num_args(mixin),
        0,
        bindings,
    );
    body.statements_and_end(&[Stmt::Return(args)]);
    Some(body.finish(clause.span))
}

/// One body being lowered: a method, a block, an initializer or a
/// superclass-clause body.
struct Body<'a> {
    bindings: &'a Bindings,
    kind: CodeKind,
    num_args: u16,
    /// The first register above the named locals. Every register from here
    /// on is written by exactly one instruction.
    first_scratch: u16,
    next_register: u16,
    instrs: Vec<Instr>,
    statement_pc: Vec<(usize, Range<usize>)>,
    call_sites: Vec<CallSite>,
}

impl<'a> Body<'a> {
    fn new(kind: CodeKind, num_args: u16, num_temps: u16, bindings: &'a Bindings) -> Self {
        let first_scratch = num_args + num_temps;
        Body {
            bindings,
            kind,
            num_args,
            first_scratch,
            next_register: first_scratch,
            instrs: Vec::new(),
            statement_pc: Vec::new(),
            call_sites: Vec::new(),
        }
    }

    /// A register nothing has written yet. Checked in every build rather than
    /// only under `debug_assertions`: a register index that wrapped would be
    /// silently wrong code, and compiling is not a hot path.
    fn fresh(&mut self) -> u16 {
        let register = self.next_register;
        self.next_register = register
            .checked_add(1)
            .expect("internal: more than u16::MAX registers in one body");
        register
    }

    fn emit(&mut self, instr: Instr) {
        self.instrs.push(instr);
    }

    fn binding(&self, span: &Span) -> Option<Binding> {
        self.bindings.get(span).copied()
    }

    fn record_statement(&mut self, start: usize) {
        let index = self.statement_pc.len();
        self.statement_pc.push((index, start..self.instrs.len()));
    }

    fn statements_and_end(&mut self, stmts: &[Stmt]) {
        let mut last_value = None;
        for stmt in stmts {
            last_value = self.statement(stmt);
        }
        self.end(matches!(stmts.last(), Some(Stmt::Return(_))), last_value);
    }

    /// One of the body's own statements, recorded in `statement_pc`. Answers
    /// the statement's value register, or `None` when it emitted a return.
    fn statement(&mut self, stmt: &Stmt) -> Option<u16> {
        let start = self.instrs.len();
        let value = match stmt {
            Stmt::Expr(expr) => Some(self.value(expr)),
            Stmt::Return(expr) => {
                let reg = self.value(expr);
                match self.kind {
                    CodeKind::Method
                    | CodeKind::SuperclassClause
                    | CodeKind::SuperclassMessage => {
                        self.emit(Instr::Return { reg });
                        None
                    }
                    CodeKind::Block => {
                        self.emit(Instr::NonLocalReturn { reg });
                        None
                    }
                    // A top-level `^` in an initializer has no method to
                    // return from: it is evaluated and its value discarded.
                    CodeKind::Initializer => Some(reg),
                }
            }
        };
        self.record_statement(start);
        value
    }

    /// What ends a body whose last statement did not: a method answers
    /// `self`, an initializer always does (its `^` never returns), and a block
    /// or a superclass-clause body answers its last statement's value, `nil`
    /// when it has none.
    fn end(&mut self, ends_in_return: bool, last_value: Option<u16>) {
        match self.kind {
            CodeKind::Initializer => self.return_self(),
            CodeKind::Method if !ends_in_return => self.return_self(),
            CodeKind::Block | CodeKind::SuperclassClause | CodeKind::SuperclassMessage
                if !ends_in_return =>
            {
                let reg = match last_value {
                    Some(reg) => reg,
                    None => self.literal(BcLiteral::Nil),
                };
                self.emit(Instr::ImplicitReturn { reg });
            }
            CodeKind::Method
            | CodeKind::Block
            | CodeKind::SuperclassClause
            | CodeKind::SuperclassMessage => {}
        }
    }

    fn return_self(&mut self) {
        let reg = self.fresh();
        self.emit(Instr::LoadSelf { dst: reg });
        self.emit(Instr::ImplicitReturn { reg });
    }

    fn literal(&mut self, literal: BcLiteral) -> u16 {
        let dst = self.fresh();
        self.emit(Instr::LoadLiteral { dst, literal });
        dst
    }

    /// Lowers `expr` and answers the register holding its value -- always a
    /// register this lowering wrote for it and will never write again.
    fn value(&mut self, expr: &Expr) -> u16 {
        match expr {
            Expr::Literal(lit) => self.literal(lower_literal(lit)),
            Expr::Ident(id) => self.read(id),
            Expr::Assign { target, value, .. } => self.assign(target, value),
            Expr::Send(send) => self.send(send),
            Expr::Cascade(cascade) => self.cascade(cascade),
            Expr::Block(block) => self.block(block),
            Expr::DynamicArray { elements, .. } => {
                let elem_regs = self.values(elements);
                let dst = self.fresh();
                self.emit(Instr::MakeArray { dst, elem_regs });
                dst
            }
        }
    }

    /// Left to right, each into a register of its own.
    fn values(&mut self, exprs: &[Expr]) -> Vec<u16> {
        exprs.iter().map(|e| self.value(e)).collect()
    }

    fn read(&mut self, id: &ast::Ident) -> u16 {
        if id.name == "self" {
            let dst = self.fresh();
            self.emit(Instr::LoadSelf { dst });
            return dst;
        }
        match self.binding(&id.span) {
            Some(Binding::Local(src)) => {
                let dst = self.fresh();
                self.emit(Instr::Move { dst, src });
                dst
            }
            Some(Binding::OuterLocal { depth, index }) => {
                let dst = self.fresh();
                self.emit(Instr::LoadOuterLocal { dst, depth, index });
                dst
            }
            Some(Binding::LexicalSend { depth }) => {
                let selector = Symbol::intern(&id.name);
                self.emit_send(|dst, call_site| Instr::SendLexical {
                    depth,
                    selector,
                    arg_regs: vec![],
                    dst,
                    call_site,
                })
            }
            // No binding: a bare builtin class name, or the deferred
            // unresolved-identifier error (`docs/design/compiler.md`,
            // "Compilation cannot fail"). A bare identifier bound `SuperSend`
            // is a `super`-prefixed superclass-clause receiver, which falls to
            // the same case.
            Some(Binding::SuperSend) | None => {
                let dst = self.fresh();
                match crate::interpreter::instantiate::builtin_kind_from_name(&id.name) {
                    Some(kind) => self.emit(Instr::LoadBuiltinClass { dst, kind }),
                    None => self.emit(Instr::LoadLiteral {
                        dst,
                        literal: BcLiteral::UnresolvedIdent(id.name.clone()),
                    }),
                }
                dst
            }
        }
    }

    /// The assignment's value is the register its value was computed into,
    /// never the local it was stored in.
    fn assign(&mut self, target: &ast::Ident, value: &Expr) -> u16 {
        let v = self.value(value);
        match self.binding(&target.span) {
            Some(Binding::Local(slot)) => self.emit(Instr::StoreLocal { src: v, slot }),
            Some(Binding::OuterLocal { depth, index }) => {
                self.emit(Instr::StoreOuterLocal { src: v, depth, index });
            }
            Some(Binding::LexicalSend { depth }) => {
                let selector = Symbol::intern(&format!("{}:", target.name));
                self.emit_send(|dst, call_site| Instr::SendLexical {
                    depth,
                    selector,
                    arg_regs: vec![v],
                    dst,
                    call_site,
                });
            }
            Some(Binding::SuperSend) | None => panic!(
                "internal: unresolved assignment target `{}` (the resolver should have rejected this)",
                target.name
            ),
        }
        v
    }

    fn send(&mut self, send: &ast::Send) -> u16 {
        let selector = selector_of(&send.message);
        match self.binding(&send.span) {
            Some(Binding::SuperSend) => {
                let arg_regs = self.values(&send.message.args);
                self.emit_send(|dst, call_site| Instr::SendSuper {
                    selector,
                    arg_regs,
                    dst,
                    call_site,
                })
            }
            Some(Binding::LexicalSend { depth }) => {
                let arg_regs = self.values(&send.message.args);
                self.emit_send(|dst, call_site| Instr::SendLexical {
                    depth,
                    selector,
                    arg_regs,
                    dst,
                    call_site,
                })
            }
            // An ordinary send, including every one the optimized lowering
            // inlines. This is where the trampoline emits `SendInlinable` for
            // the sends `control_flow::recognize` accepts
            // (`docs/design/compiler.md`, "How the limit counts").
            Some(Binding::Local(_) | Binding::OuterLocal { .. }) | None => {
                let receiver_reg = self.value(&send.receiver);
                self.public_send(receiver_reg, &send.message)
            }
        }
    }

    fn public_send(&mut self, receiver_reg: u16, message: &ast::MessageSend) -> u16 {
        let selector = selector_of(message);
        let arg_regs = self.values(&message.args);
        self.emit_send(|dst, call_site| Instr::SendPublic {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            call_site,
        })
    }

    fn cascade(&mut self, cascade: &ast::Cascade) -> u16 {
        let receiver_reg = self.value(&cascade.receiver);
        let mut result = self.public_send(receiver_reg, &cascade.first);
        for message in &cascade.rest {
            result = self.public_send(receiver_reg, message);
        }
        result
    }

    fn block(&mut self, block: &ast::BlockExpr) -> u16 {
        let params: Vec<Symbol> = block.params.iter().map(|p| Symbol::intern(&p.name)).collect();
        let mut inner = Body::new(
            CodeKind::Block,
            params.len() as u16,
            block.body.temps.len() as u16,
            self.bindings,
        );
        inner.statements_and_end(&block.body.stmts);
        let code = inner.finish(block.span);
        let dst = self.fresh();
        self.emit(Instr::MakeClosure {
            dst,
            params: Rc::new(params),
            code: Rc::new(code),
        });
        dst
    }

    fn emit_send(&mut self, build: impl FnOnce(u16, CallSite) -> Instr) -> u16 {
        let dst = self.fresh();
        // A call site is its send's own instruction index. Checked in every
        // build for the same reason as `fresh`: a wrapped id would alias two
        // call sites silently.
        let call_site = CallSite {
            call_site_id: u16::try_from(self.instrs.len())
                .expect("internal: more than u16::MAX instructions in one body"),
        };
        self.emit(build(dst, call_site));
        self.call_sites.push(call_site);
        dst
    }

    fn finish(self, source: Span) -> CompiledCode {
        if cfg!(debug_assertions) {
            check_shape(&self.instrs, self.first_scratch);
        }
        CompiledCode {
            kind: self.kind,
            instrs: self.instrs,
            num_args: self.num_args,
            num_registers: self.next_register,
            statement_pc: self.statement_pc,
            call_sites: self.call_sites,
            source,
        }
    }
}

/// The shape the reference lowering promises (`docs/design/compiler.md`,
/// "What it emits"), checked on every body in a debug build, which is what
/// the test suite runs: no branch, since only inlining emits one, and every
/// register above the named locals written by exactly one instruction. A
/// named local is written only by `StoreLocal`, which `destination` does not
/// count.
fn check_shape(instrs: &[Instr], first_scratch: u16) {
    let mut written = HashSet::new();
    for instr in instrs {
        assert!(
            !matches!(
                instr,
                Instr::Jump { .. }
                    | Instr::JumpIfNotBoolean { .. }
                    | Instr::BranchIfFalse { .. }
                    | Instr::BranchIfTrue { .. }
            ),
            "internal: the reference lowering emitted a branch: {instr:?}"
        );
        if let Some(dst) = destination(instr) {
            assert!(
                dst >= first_scratch,
                "internal: the reference lowering wrote named local r{dst} other than by StoreLocal"
            );
            assert!(
                written.insert(dst),
                "internal: the reference lowering wrote r{dst} twice"
            );
        }
    }
}

/// The register `instr` writes, if any. Exhaustive on purpose: a new
/// instruction has to decide here whether it writes a register.
fn destination(instr: &Instr) -> Option<u16> {
    match instr {
        Instr::LoadLiteral { dst, .. }
        | Instr::LoadSelf { dst }
        | Instr::LoadBuiltinClass { dst, .. }
        | Instr::Move { dst, .. }
        | Instr::LoadOuterLocal { dst, .. }
        | Instr::SendLexical { dst, .. }
        | Instr::SendPublic { dst, .. }
        | Instr::SendSuper { dst, .. }
        | Instr::MakeClosure { dst, .. }
        | Instr::MakeArray { dst, .. } => Some(*dst),
        Instr::StoreLocal { .. }
        | Instr::StoreOuterLocal { .. }
        | Instr::InitSlot { .. }
        | Instr::Return { .. }
        | Instr::ImplicitReturn { .. }
        | Instr::NonLocalReturn { .. }
        | Instr::Jump { .. }
        | Instr::JumpIfNotBoolean { .. }
        | Instr::BranchIfFalse { .. }
        | Instr::BranchIfTrue { .. } => None,
    }
}
```

In `crates/ns-runtime/src/compiler/lower.rs`, `fn lower_literal` →
`pub(super) fn lower_literal`.

- [ ] **Step 5: Add `Lowering` and route the entry points**

In `crates/ns-runtime/src/compiler/mod.rs`, extend the module doc's first line
to "Bytecode compiler entry points, and the choice between the optimized
lowering (`lower.rs`) and the reference lowering it is checked against
(`reference.rs`)." Declare the module and the enum after
`pub(crate) mod lower;`:

```rust
pub(crate) mod reference;
```

```rust
/// Which lowering compiles a module: the optimized one every program runs
/// under, or the deliberately naive reference lowering it is checked against
/// (`docs/design/compiler.md`, "The reference lowering"). Chosen once per
/// `Interpreter`, so one interpreter never mixes the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lowering {
    Optimized,
    Reference,
}
```

Give `compile_method` a `lowering: Lowering` parameter and route it first:

```rust
pub fn compile_method(
    method: &ast::MethodDecl,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
) -> CompiledCode {
    if lowering == Lowering::Reference {
        return reference::compile_method(method, bindings);
    }
    // ... the existing body, unchanged ...
}
```

and the same for `compile_mixin_bodies`:

```rust
pub(crate) fn compile_mixin_bodies(
    mixin: &mut Mixin,
    bindings: &Rc<HashMap<Span, Binding>>,
    lowering: Lowering,
) {
    if lowering == Lowering::Reference {
        reference::compile_mixin_bodies(mixin, bindings);
        return;
    }
    // ... the existing three assignments, unchanged ...
}
```

`num_args_of`, `factory_num_args`, `merge_span` and `stmt_span` stay private:
`reference` is a child module of `compiler`, so it can already call them.

In `crates/ns-runtime/src/lib.rs`: `pub use compiler::Lowering;`.

- [ ] **Step 6: Thread the lowering from the interpreter to the compiler**

`crates/ns-runtime/src/mixin.rs` — `MixinMethod::from_decl` takes and passes
it:

```rust
    pub fn from_decl(
        decl: Rc<ast::MethodDecl>,
        bindings: &Rc<HashMap<Span, Binding>>,
        lowering: crate::compiler::Lowering,
    ) -> MixinMethod {
        MixinMethod {
            selector: Symbol::intern(&crate::interpreter::instantiate::method_name(&decl.pattern)),
            access: crate::dispatch::effective_access(decl.access),
            num_args: crate::interpreter::instantiate::pattern_param_count(&decl.pattern),
            code: Rc::new(crate::compiler::compile_method(&decl, bindings, lowering)),
            decl: Some(decl),
        }
    }
```

Change its doc to "The entry a parsed declaration becomes, compiled by
`lowering` against the resolver's `bindings` for the module it belongs to."

`crates/ns-runtime/src/interpreter/mod.rs` — add a field to `Interpreter`
(after `nested_in_progress`):

```rust
    /// The lowering every module this interpreter loads is compiled with
    /// (`docs/design/compiler.md`, "The reference lowering"). Fixed at
    /// construction, so one interpreter never mixes the two.
    lowering: Lowering,
```

and replace `bootstrap` with:

```rust
    pub fn bootstrap() -> Interpreter {
        Interpreter::bootstrap_with(Lowering::Optimized)
    }

    /// An interpreter whose `load_module` compiles with `lowering`. The
    /// reference lowering is for checking the optimized one against it; an
    /// artifact decoded into either runs the code it carries.
    pub fn bootstrap_with(lowering: Lowering) -> Interpreter {
        let mut interp = Interpreter {
            heap: Heap::new(),
            builtin_classes: Vec::new(),
            platform_singleton: None,
            applications: HashMap::new(),
            empty_class_sides: HashMap::new(),
            nested_in_progress: Vec::new(),
            lowering,
        };
        interp.builtin_classes = build_builtin_classes(&mut interp);
        interp
    }
```

with `use crate::compiler::Lowering;` among the module's imports.

`crates/ns-runtime/src/interpreter/instantiate.rs` — `build_methods` gains a
`lowering: crate::compiler::Lowering` parameter and passes it to
`MixinMethod::from_decl(Rc::new(m.clone()), bindings, lowering)`. In
`build_mixin` and `build_class_side`, pass `self.lowering` to both
`build_methods(..., self.lowering)` and
`crate::compiler::compile_mixin_bodies(&mut mixin, bindings, self.lowering)`.
In `load_module`'s doc, after "so a class this returns is already runnable",
add: "It is compiled with the lowering this interpreter was bootstrapped with."

`crates/ns-runtime/src/dispatch.rs`, test helper `class_with_method`: pass
`crate::compiler::Lowering::Optimized` as `from_decl`'s third argument.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime --test reference_lowering`
Expected: PASS, 3 tests.

Run: `cargo test -p ns-runtime --lib compiler::reference`
Expected: PASS, 8 tests.

If `every_program_answers_the_same_under_both_lowerings` fails under exactly
one lowering, that is the oracle doing its job: apply the Global Constraints
rule for a divergence before going on. If the initializer test's
`SendLexical`/`SendPublic` indices differ because the resolver binds `a`
differently than expected, read the binding the resolver produces
(`crate::resolver`) and correct the test's *pattern* to what the design's rules
require for that binding — never weaken the rule it checks.

- [ ] **Step 8: The documents describe the lowering as built**

The lowering exists from this commit on, so the design stops calling it
planned:

- `docs/design/compiler.md`: retitle "## Planned: the reference lowering" to
  "## The reference lowering", and change its first paragraph's "What would
  change: the compiler gains a second, deliberately naive lowering, and every
  program must behave" to "The compiler has a second, deliberately naive
  lowering, `compiler/reference.rs`, and every program must behave". In
  "What it emits", replace the paragraph beginning "Its `finish`
  debug-asserts that shape" with: "Its `finish` checks that shape in a debug
  build, which is what the test suite runs (`check_shape`): no branch
  instruction, since only inlining emits one, and every scratch register
  written by exactly one instruction. The two ceilings — at most `u16::MAX`
  registers and `u16::MAX` instructions in a body — are checked in every build
  (`fresh`, `emit_send`), because an index that wrapped would be silently
  wrong code." Add `compiler/reference.rs` to the header block's **Code**
  list.
- Every reference to the old heading follows it: `CLAUDE.md` (the "Program
  outcomes are pinned" paragraph), `docs/design/execution.md` ("One engine",
  where the reference is split across two lines: `"Planned: the reference` /
  `lowering"`), and `docs/PROGRESS.md` ("Where the next session starts").
  Change only the quoted heading name to `"The reference lowering"`; the
  surrounding sentences change in Task 4, when the comparison runs.

- [ ] **Step 9: Run everything, then commit**

Run: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, both
corpus gates. Expected: all green, counts unchanged.

```bash
cargo fmt --all
git add -A crates docs CLAUDE.md
git commit -F- <<'EOF'
feat: add the reference lowering, selected per interpreter

compiler/reference.rs lowers a module a second, deliberately naive way:
nothing inlined, every value in a register no other instruction writes,
every read of a local copied. It is written independently of lower.rs so
that everything a lowering decides is checked, not only what the two do
differently. An Interpreter is bootstrapped with a Lowering and compiles
every module it loads with it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
EOF
```

- [ ] **Step 10: Check byte identity**

Build the "before" binary from `HEAD~1` as in the measurement protocol (no
benchmark needed), run the byte-identity protocol with `$task = 2`. Expected:
`compared 46 artifacts, 0 differing`. Record under "Task 2" in
`target\c-measurements.md`, then remove the worktree.

---

### Task 3: `--lowering` on the command line

**Files:**
- Modify: `crates/ns-cli/src/main.rs`
- Modify: `crates/ns-cli/src/source.rs:52-60`
- Modify: `crates/ns-cli/src/commands/run.rs`, `snapshot.rs`, `build.rs`,
  `disasm.rs`, `bench.rs:60`
- Create: `crates/ns-cli/tests/lowering.rs`
- Modify: `crates/ns-cli/tests/disasm_snapshots.rs`
- Create (by insta): two files under `crates/ns-cli/tests/snapshots/`
- Modify: `docs/design/compiler.md`

**Interfaces:**
- Consumes: `ns_runtime::Lowering`, `Interpreter::bootstrap_with` (Task 2).
- Produces: `source::bootstrap(loaded: &LoadedSource, lowering:
  ns_runtime::Lowering) -> (Interpreter, ClassRef)`; the flag
  `--lowering <optimized|reference>` on `run`, `snapshot`, `build` (default
  `optimized`) and `disasm` (absent means optimized; refused for an artifact).
  Task 4 relies on the exact spelling `--lowering reference`.

- [ ] **Step 1: Write the failing CLI tests**

Create `crates/ns-cli/tests/lowering.rs`:

```rust
//! `--lowering`: choosing the lowering a source is compiled with
//! (`docs/design/compiler.md`, "The reference lowering").

mod common;
use common::{fixture, ns_next, ScratchDir};

/// Only control-flow inlining emits a branch, so a listing under the
/// reference lowering has none, and the optimized one of the same loop does.
#[test]
fn the_reference_lowering_inlines_nothing() {
    let src = fixture("while_loop.ns");
    let (ok, optimized, err) = ns_next(&["disasm".as_ref(), src.as_os_str(), "main:args:".as_ref()]);
    assert!(ok, "{err}");
    assert!(optimized.contains("Jump"), "{optimized}");
    let (ok, reference, err) = ns_next(&[
        "disasm".as_ref(),
        src.as_os_str(),
        "main:args:".as_ref(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert!(!reference.contains("Jump"), "{reference}");
    assert!(!reference.contains("Branch"), "{reference}");
    assert!(reference.contains("#whileTrue:"), "{reference}");
}

#[test]
fn a_fixture_runs_under_the_reference_lowering() {
    let (ok, out, err) = ns_next(&[
        "run".as_ref(),
        fixture("while_loop.ns").as_os_str(),
        "--lowering".as_ref(),
        "reference".as_ref(),
    ]);
    assert!(ok, "{err}");
    assert_eq!(
        out,
        std::fs::read_to_string(fixture("while_loop.expected"))
            .expect("read expected")
            .replace("\r\n", "\n")
    );
}

/// An artifact carries the code it was built with, so asking `disasm` to
/// lower it is a mistake, refused rather than silently ignored.
#[test]
fn disasm_refuses_a_lowering_for_an_artifact() {
    let dir = ScratchDir::new("lowering-artifact");
    let out = dir.join("app.nsb");
    let (ok, _, err) = ns_next(&[
        "build".as_ref(),
        fixture("while_loop.ns").as_os_str(),
        out.as_os_str(),
    ]);
    assert!(ok, "{err}");
    let (ok, _, err) = ns_next(&[
        "disasm".as_ref(),
        out.as_os_str(),
        "main:args:".as_ref(),
        "--lowering".as_ref(),
        "optimized".as_ref(),
    ]);
    assert!(!ok);
    assert!(err.contains("--lowering"), "{err}");
}
```

Append to `crates/ns-cli/tests/disasm_snapshots.rs`:

```rust
#[test]
fn disasm_while_loop_main_under_the_reference_lowering() {
    insta::assert_snapshot!(disasm("while_loop.ns", &["main:args:", "--lowering", "reference"]));
}

#[test]
fn disasm_control_flow_fallback_main_under_the_reference_lowering() {
    insta::assert_snapshot!(disasm(
        "control_flow_fallback.ns",
        &["main:args:", "--lowering", "reference"]
    ));
}
```

and add ", and the same two shapes under the reference lowering" to the module
doc's list.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-cli --test lowering`
Expected: FAIL — `error: unexpected argument '--lowering'`.

- [ ] **Step 3: Add the flag**

In `crates/ns-cli/src/main.rs`, below `struct Cli`:

```rust
/// Which lowering compiles a source (`docs/design/compiler.md`, "The
/// reference lowering"): `optimized` is what every program runs under,
/// `reference` the naive lowering it is checked against.
#[derive(Clone, Copy, clap::ValueEnum)]
enum LoweringArg {
    Optimized,
    Reference,
}

impl From<LoweringArg> for ns_runtime::Lowering {
    fn from(arg: LoweringArg) -> Self {
        match arg {
            LoweringArg::Optimized => ns_runtime::Lowering::Optimized,
            LoweringArg::Reference => ns_runtime::Lowering::Reference,
        }
    }
}
```

Add to `Run`, `Snapshot` and `Build`:

```rust
        /// The lowering to compile the source with.
        #[arg(long, value_enum, default_value_t = LoweringArg::Optimized)]
        lowering: LoweringArg,
```

and to `Disasm` (an `Option`, so an explicit flag on an artifact can be told
apart from the default):

```rust
        /// The lowering to compile a .ns source with; optimized when absent.
        /// Refused for an artifact, which carries the code it was built with.
        #[arg(long, value_enum)]
        lowering: Option<LoweringArg>,
```

In `main`, destructure `lowering` in those four arms and pass
`lowering.into()` (for `Disasm`, `lowering.map(Into::into)`) as a new last
argument to each command's `execute`.

In `crates/ns-cli/src/source.rs`:

```rust
/// Bootstraps an interpreter that compiles with `lowering` and loads the
/// module's root class, which compiles every body onto the declaration it
/// belongs to -- before anything is instantiated, so the root class's
/// instance initializer runs compiled too.
pub(crate) fn bootstrap(
    loaded: &LoadedSource,
    lowering: ns_runtime::Lowering,
) -> (ns_runtime::Interpreter, ns_runtime::ClassRef) {
    let mut interp = ns_runtime::Interpreter::bootstrap_with(lowering);
    let class = interp.load_module(&loaded.resolved);
    (interp, class)
}
```

`commands/run.rs`, `snapshot.rs`, `build.rs`: each `execute` gains a final
`lowering: ns_runtime::Lowering` parameter and calls
`source::bootstrap(&loaded, lowering)`. `commands/bench.rs`:
`source::bootstrap(loaded, ns_runtime::Lowering::Optimized)` — timing the
oracle means nothing.

`commands/disasm.rs`: `execute` gains `lowering: Option<ns_runtime::Lowering>`.
At the top of the artifact branch, before `image::load`:

```rust
        if lowering.is_some() {
            eprintln!(
                "error: --lowering applies to a .ns source; {} is an artifact, which carries the code it was built with",
                file.display()
            );
            return ExitCode::FAILURE;
        }
```

and in the source branch
`source::bootstrap(&loaded, lowering.unwrap_or(ns_runtime::Lowering::Optimized))`.
Add to `execute`'s doc: "`lowering` chooses how a source is compiled; it is
refused for an artifact."

- [ ] **Step 4: Run the tests; review and accept the new snapshots**

Run: `cargo test -p ns-cli --test lowering` — PASS.

Run (PowerShell): `$env:INSTA_UPDATE='always'; cargo test -p ns-cli --test disasm_snapshots; Remove-Item Env:INSTA_UPDATE`
This writes the two new `.snap` files. **Review each by hand before going on**
— a snapshot records what the code produced, and is accepted only if it is
what the design requires (`compiler.md`, "What it emits"):
no `Jump`/`Branch` line; every read of a local is a `Move` into a register not
seen before; no `rN` appears as the destination of two instructions; each
`ifTrue:ifFalse:` and `whileTrue:` is a `SendPublic` whose arguments are the
`MakeClosure` registers just before it; each block listing ends in
`ImplicitReturn` or `NonLocalReturn`. The six existing snapshot files must be
unchanged (`git status` shows only the two new ones).

Run: `cargo test -p ns-cli --test disasm_snapshots` — PASS without the
variable.

- [ ] **Step 5: Run everything, then commit**

Run: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, both
corpus gates.

In `docs/design/compiler.md`, "Order of work", item 2 gains "— done." after
its bold title, and "Selecting it" names the switch's code: `LoweringArg` in
`crates/ns-cli/src/main.rs`, and `source::bootstrap`, which every compiling
command goes through.

```bash
cargo fmt --all
git add -A crates docs
git commit -F- <<'EOF'
feat: choose the lowering with --lowering on run, snapshot, build and disasm

disasm of one method under both lowerings is the first tool to reach for
when the oracle fires; its listings under the reference lowering are
pinned for the loop and the fallback shapes. An artifact carries the code
it was built with, so disasm refuses --lowering for one. bench keeps the
optimized lowering.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
EOF
```

- [ ] **Step 6: Check byte identity**

As Task 2 Step 10, with `$task = 3`. Expected: `compared 46 artifacts, 0
differing`. Record under "Task 3".

---

### Task 4: Every recorded program under both lowerings

**Files:**
- Modify: `crates/ns-cli/tests/run_fixtures.rs`
- Modify: `crates/ns-cli/tests/program_outcomes.rs:1-33`
- Modify: `crates/ns-cli/tests/build_load.rs:99-151`
- Modify: `crates/ns-cli/src/commands/run_corpus.rs`
- Modify: `crates/ns-cli/tests/cli.rs:188-204`
- Modify: `CLAUDE.md`, `docs/design/execution.md`, `compiler.md`, `README.md`

**Interfaces:**
- Consumes: `--lowering reference` on `run` and `build` (Task 3).
- Produces: nothing later tasks call.

- [ ] **Step 1: Time the suite before**

Run (PowerShell): `cargo test --workspace --no-run; Measure-Command { cargo test --workspace } | Select-Object TotalSeconds`
Record the seconds under "Task 4, before" in `target\c-measurements.md`.

- [ ] **Step 2: Fixtures under both lowerings**

Replace the test in `crates/ns-cli/tests/run_fixtures.rs` with:

```rust
/// Both lowerings: the optimized one every program runs under, and the
/// reference lowering it is checked against (`docs/design/compiler.md`, "The
/// reference lowering"). Each must produce the recorded answer.
const LOWERINGS: [&str; 2] = ["optimized", "reference"];

/// Every fixture succeeds, prints exactly its `.expected` stdout -- every
/// byte, the final newline included -- and writes nothing to stderr, under
/// both lowerings.
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
        for lowering in LOWERINGS {
            let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
                .arg("run")
                .arg(&path)
                .arg("--lowering")
                .arg(lowering)
                .output()
                .expect("run ns-next");
            assert!(
                output.status.success(),
                "{} exited non-zero under the {lowering} lowering: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                "",
                "{} wrote to stderr under the {lowering} lowering",
                path.display()
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert_eq!(
                normalize_newlines(&stdout),
                normalize_newlines(&expected),
                "{} produced unexpected stdout under the {lowering} lowering",
                path.display()
            );
        }
        checked += 1;
    }
    // A floor rather than whatever the directory yields: a fixture that
    // stops being discovered -- renamed, moved, its extension changed --
    // loses its expected-output check without any test failing, so the
    // floor tracks the actual count.
    assert!(
        checked >= 23,
        "expected every one of the 23 tests-fixtures/*.ns fixtures to be \
         compared against its .expected file, checked {checked} -- raise \
         this floor when adding a fixture, never lower it"
    );
}
```

- [ ] **Step 3: Golden outcomes under both lowerings**

In `crates/ns-cli/tests/program_outcomes.rs`, replace the module doc's last two
sentences ("Until the reference lowering lands ... has to be justified as
one.") with: "Every program runs under both lowerings, the optimized one and
the reference lowering it is checked against (`docs/design/compiler.md`, \"The
reference lowering\"), and each must produce the recorded answer: a change that
alters one of these is a behaviour change and has to be justified as one."
Replace `assert_outcome` with:

```rust
/// Runs `src` with `ns-next run` under each lowering and asserts all three
/// streams exactly.
fn assert_outcome(name: &str, src: &str, exit: i32, stdout: &str, stderr: &str) {
    let dir = ScratchDir::new(name);
    let path = dir.join(&format!("{name}.ns"));
    std::fs::write(&path, src).expect("write source");
    for lowering in ["optimized", "reference"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&path)
            .arg("--lowering")
            .arg(lowering)
            .output()
            .expect("run ns-next");
        let actual_stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
        let actual_stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{name} under the {lowering} lowering: exit status (stderr: {actual_stderr})"
        );
        assert_eq!(actual_stdout, stdout, "{name} under the {lowering} lowering: stdout");
        assert_eq!(actual_stderr, stderr, "{name} under the {lowering} lowering: stderr");
    }
}
```

- [ ] **Step 4: Build and load under both lowerings**

In `crates/ns-cli/tests/build_load.rs`, replace
`every_fixture_runs_identically_from_source_and_from_a_binary` (doc comment
included) with:

```rust
/// `build` then `load` must be indistinguishable from `run` on the same
/// source -- exit status, stdout and stderr -- under both lowerings. A binary
/// carries no source, so comparing it against `run` is how a binary is checked
/// at all; `run_fixtures.rs` pins the same fixtures' output against their
/// `.expected` files, so equality here also holds against a recorded answer.
/// Under the reference lowering this also takes the encoder and decoder over
/// code with no jumps and many registers.
#[test]
fn every_fixture_runs_identically_from_source_and_from_a_binary() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures");
    let dir = ScratchDir::new("all-fixtures");
    let mut checked = 0;

    for entry in std::fs::read_dir(&fixtures).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        for lowering in ["optimized", "reference"] {
            let out = dir.join(&format!("{stem}.{lowering}.nsb"));
            let (built, _, build_err) = ns_next(&[
                "build".as_ref(),
                path.as_os_str(),
                out.as_os_str(),
                "--lowering".as_ref(),
                lowering.as_ref(),
            ]);
            if !built {
                // A fixture whose root class declares no `main:args:` and
                // needs an explicit entry is not a failure of this property.
                // Anything else is, so the reason is asserted rather than
                // assumed.
                assert!(
                    build_err.contains("--entry"),
                    "build failed for {} under the {lowering} lowering: {build_err}",
                    path.display()
                );
                continue;
            }

            let (run_ok, run_out, run_err) = ns_next(&[
                "run".as_ref(),
                path.as_os_str(),
                "--lowering".as_ref(),
                lowering.as_ref(),
            ]);
            let (load_ok, load_out, load_err) = ns_next(&["load".as_ref(), out.as_os_str()]);
            let what = format!("{} under the {lowering} lowering", path.display());
            assert_eq!(load_ok, run_ok, "exit status differs for {what}");
            assert_eq!(load_out, run_out, "stdout differs for {what}");
            assert_eq!(load_err, run_err, "stderr differs for {what}");
            checked += 1;
        }
    }

    assert!(
        checked >= 10,
        "expected several fixtures to be covered under both lowerings, got {checked}"
    );
}
```

- [ ] **Step 5: Run the three suites**

Run: `cargo test -p ns-cli --test run_fixtures --test program_outcomes --test build_load`
Expected: PASS. **A failure under the `reference` lowering alone is a
divergence**: apply the Global Constraints rule — do not touch the recorded
answer.

- [ ] **Step 6: `run-corpus` runs every row four ways**

In `crates/ns-cli/src/commands/run_corpus.rs`, give `run_one` and
`run_one_binary` a `lowering: &str` parameter and pass
`.arg("--lowering").arg(lowering)` to the `run` command and to the `build`
command (not to `load`). Add, after `RunOutcome`:

```rust
/// One way of running a row. Every row runs all four: from source and as a
/// built binary, under each lowering -- the optimized one every program runs
/// under, and the reference lowering it is checked against
/// (`docs/design/compiler.md`, "The reference lowering"). No switch selects
/// them, because a gate must not be optional.
struct Lane {
    name: &'static str,
    lowering: &'static str,
    binary: bool,
}

const LANES: [Lane; 4] = [
    Lane { name: "source", lowering: "optimized", binary: false },
    Lane { name: "binary", lowering: "optimized", binary: true },
    Lane { name: "reference source", lowering: "reference", binary: false },
    Lane { name: "reference binary", lowering: "reference", binary: true },
];

/// Runs `entry` every way `LANES` lists and checks each against the row's
/// recorded outcome: the exit status and stdout, and stderr -- empty for a
/// row expected to exit 0, and in every later lane equal to the first lane's.
/// The first lane that disagrees decides the row, and the message names it.
/// `executed` is set once the first lane's module has run at all, which is
/// what `ran` counts.
fn check_row(
    exe: &Path,
    file: &Path,
    entry: &RunnableEntry,
    executed: &mut bool,
) -> Result<(), String> {
    let mut first_stderr: Option<String> = None;
    for lane in &LANES {
        let outcome = if lane.binary {
            run_one_binary(exe, file, &entry.send, lane.lowering)
        } else {
            run_one(exe, file, &entry.send, lane.lowering)
        }
        .map_err(|e| format!("{}: {e}", lane.name))?;
        *executed = true;
        if outcome.status != Some(entry.expected_status) {
            return Err(format!(
                "{}: expected exit {}, got {} (stderr: {})",
                lane.name,
                entry.expected_status,
                match outcome.status {
                    Some(c) => c.to_string(),
                    None => "signal".to_string(),
                },
                outcome.stderr.trim_end()
            ));
        }
        if outcome.stdout != entry.expected_stdout {
            return Err(format!(
                "{}: expected stdout {:?}, got {:?}",
                lane.name, entry.expected_stdout, outcome.stdout
            ));
        }
        match &first_stderr {
            // A successful run writes nothing to stderr. Without this, a
            // warning every lane prints identically would pass, since later
            // lanes are compared only with the first.
            None if entry.expected_status == 0 && !outcome.stderr.is_empty() => {
                return Err(format!(
                    "{}: expected no stderr, got {:?}",
                    lane.name, outcome.stderr
                ));
            }
            None => first_stderr = Some(outcome.stderr),
            Some(first) if *first != outcome.stderr => {
                return Err(format!(
                    "{}: stderr differs from the {} run's: {:?}, against {:?}",
                    lane.name, LANES[0].name, outcome.stderr, first
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}
```

Replace the body of `execute`'s `for entry in &entries` loop, after the
`!file.is_file()` check, with:

```rust
        let mut executed = false;
        let result = check_row(&exe, &file, entry, &mut executed);
        if executed {
            ran += 1;
        }
        if let Err(e) = result {
            failed += 1;
            eprintln!("FAIL {}: {e}", entry.file);
        }
```

Rewrite `execute`'s doc paragraph beginning "Every entry is run from source"
to say: every entry runs the four ways `LANES` lists; each is checked against
the recorded exit status and stdout, stderr is empty for a row expected to exit
0 and identical across lanes; a row's `send` doubles as the build's `--entry`;
each row counts once in `ran`/`failed` and the first lane that fails decides
it. Keep the paragraph "What that is worth, stated honestly" unchanged.

In `crates/ns-cli/tests/cli.rs`,
`run_corpus_fails_when_the_recorded_stdout_no_longer_matches`: change the
stderr assertion's needle from `"expected stdout"` to
`"FAIL Talkative.ns: source: expected stdout"`, so the lane is pinned as named.

- [ ] **Step 7: Run everything, time the suite after**

Run: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, and
both corpus gates — `run-corpus` must still report `ran 6, failed 0, skipped
205`.

Run: `cargo test --workspace --no-run; Measure-Command { cargo test --workspace } | Select-Object TotalSeconds`
Record under "Task 4, after".

- [ ] **Step 8: The documents say the comparison runs**

From this commit on, every recorded program is checked under both lowerings:

- `CLAUDE.md`, "**Program outcomes are pinned, not re-derived.**": replace the
  sentence "Until the reference lowering lands (`docs/design/compiler.md`,
  "The reference lowering") there is no second way to run a program and
  compare against." with "The reference lowering (`docs/design/compiler.md`,
  \"The reference lowering\") is a second way to run every program, and both
  are held to recorded answers:" and let the list that follows ("every
  `tests-fixtures/*.ns` against ...") continue that sentence; keep the rest of
  the paragraph. In "Working in this repository", the sentence "`run-corpus`
  runs every listed module from source and as a built binary and fails if
  either differs from the recorded output." becomes "`run-corpus` runs every
  listed module from source and as a built binary, under both lowerings, and
  fails if any of the four differs from the recorded output."
- `docs/design/execution.md`, "One engine": replace "Until the reference
  lowering lands there is no second way to run a program and compare against,
  so behaviour is protected by recorded answers:" with "Behaviour is
  protected by recorded answers, which both lowerings must produce:", and the
  paragraph's last sentence ("The reference lowering ([compiler.md] ...) is
  the planned differential oracle.") with "The reference lowering
  ([compiler.md](compiler.md), \"The reference lowering\") is the differential
  oracle that replaced it." In its Decisions, "golden outcomes protect
  behaviour until the reference lowering becomes the differential oracle" →
  "golden outcomes protect behaviour, and the reference lowering is the
  differential oracle".
- `docs/design/compiler.md`: in "Testing the fallbacks", "With no second
  lowering yet to compare against, those paths are pinned by recorded
  answers:" → "Those paths are pinned by recorded answers, which both
  lowerings must produce:". In "Order of work", item 3 gains "— done." after
  its bold title, and in "The harness" record the suite's running time before
  and after this task (the two numbers in `target\c-measurements.md`) and name
  `crates/ns-runtime/tests/reference_lowering.rs` as the corpus-compile test.
- `docs/design/README.md`, the gates block: after the `run-corpus` line's
  comment, add "— every row under both lowerings".

- [ ] **Step 9: Commit**

```bash
cargo fmt --all
git add -A crates docs CLAUDE.md
git commit -F- <<'EOF'
test: run every recorded program under both lowerings

Fixtures, golden outcomes and the build/load round trip each run under
the optimized and the reference lowering against the same recorded
answer, and run-corpus runs every row four ways -- from source and as a
built binary, under each lowering -- naming the lane that disagrees.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
EOF
```

---

### Task 5: Documents, the whole-step measurement, and PROGRESS

**Files:**
- Modify: `docs/design/compiler.md`, `README.md`
- Modify: `CLAUDE.md`
- Modify: `docs/PROGRESS.md`

- [ ] **Step 1: Measure the whole step**

Run the measurement protocol against `master` (`git worktree add
..\ns-next-before master`). Record under "Task 5" in
`target\c-measurements.md`, then remove the worktree.

- [ ] **Step 2: `compiler.md` describes what exists**

- Replace "Order of work" with a short "Status" paragraph: built, in the four
  commits of this plan; the depth rule's `SendInlinable` and the counter
  arrive with the trampoline.
- In the header block's **Status** line, add that the reference lowering is
  built; in Decisions drop the "Planned." from the reference-lowering decision
  (the depth-limit decision keeps it, since `SendInlinable` does not exist
  yet).

- [ ] **Step 3: The other documents**

- `docs/design/README.md`: the compiler row's "Planned next" becomes
  "inline-cache slots wired up".
- `CLAUDE.md`: in the `crates/ns-runtime` bullet, "the compiler
  (`compiler/`)" becomes "the compiler (`compiler/`, with the reference
  lowering it is checked against in `compiler/reference.rs`)".

- [ ] **Step 4: PROGRESS**

In `docs/PROGRESS.md`:

- Add the plan to M4's row in the table.
- "Where the next session starts": C is done; the next piece is **1b, the
  trampoline**, whose plan is not written yet and whose design is
  `execution.md`, "Planned: the trampoline" — including `SendInlinable` and the
  depth counter by the rule in `compiler.md`, "How the limit counts". Keep the
  paragraph "Before 1b fixes its baseline, profile `bench_block.ns`" as it is.
  Mark C "Done." in the A–C list with a pointer to the new section.
- Add a section `## M4 — The reference lowering (<date>)` after "M4 — Cleaning
  up with one engine", in the style of the sections before it: the plan link;
  the decisions (independent module; the depth rule and why; lazy identity
  hashes and their prior art); what changed, in task order; what was deleted
  (`alloc_placeholder_object`, `Interpreter::alloc_identity_hash`, the
  discarded `Object` allocation); the Task 1 and Task 5 measurement tables
  from `target\c-measurements.md`; the byte-identity results; the suite's
  running time before and after; the test count (`cargo test --workspace`
  summary) and both gate lines; and every divergence the oracle found, with
  its fix — or, if none, say so plainly.
- In "M4 — Cleaning up with one engine"'s "Left, deliberately" list, the item
  "The discarded `Object` allocation in `build_builtin_classes`" is now done:
  replace its text with one sentence saying step C removed it once identity
  hashes became lazy.

- [ ] **Step 5: Verify a fresh session can start from the documents**

Read `CLAUDE.md`, `docs/design/README.md` and `docs/PROGRESS.md`'s "Where the
next session starts" as someone who has never seen this branch: they must name
1b as the next step, point to its design, and describe the reference lowering
as built. Grep the repository for `Planned: the reference lowering`,
`Until the reference lowering lands`, `alloc_placeholder_object` and
`assigned at allocation` — each must be gone (plans under
`docs/superpowers/plans/` excepted; they are execution records).

- [ ] **Step 6: Run everything, then commit**

Run: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, both
corpus gates.

```bash
git add -A CLAUDE.md docs
git commit -F- <<'EOF'
docs: record step C, the reference lowering, and its measurements

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
EOF
```
