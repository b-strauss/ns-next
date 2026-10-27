# The handler search as a leaf: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `Exception>>invokeNextHandler` finds the next enabled `on:do:` through one VM-mirror leaf that walks the frames itself and reifies only the activation it answers, instead of reifying every sender it passes.

**Architecture:** A new activation leaf, `activationNextHandler:ifFail:`, walks from a running activation towards the run's base along `interpreter::unwind::WalkedActivation` (live frames, then suspended ones) and answers the activation object of the first one whose code carries the `on:do:` marker and whose handler register holds `true`, or `nil`. `Kernel.ns`'s search keeps its two walks, its counting and its array, each step one call of the leaf. `activationIsActiveHandler:ifFail:` loses its only caller and goes.

**Tech Stack:** Rust (`crates/ns-runtime`), Newspeak (`crates/ns-runtime/core-library/Kernel.ns`), cargo-nextest.

**Spec:** the decisions H1-H5 agreed with the user on 2026-10-27, recorded here; [execution.md](../../design/execution.md), "Planned: the deoptimization contract", "Observers", is why; Cog's `primitiveFindHandlerContext` (`prior-art/opensmalltalk-vm/src/spur64.sista/cointerp.c:84085-84112`) is the model.

- **H1.** One leaf, `vmMirror activationNextHandler: activation ifFail: [nil]`: the next activation outward of `activation` (not `activation` itself) whose code carries an `UnwindMarker::Handler` and whose `active_register` holds `true`; reifies only that one; walks through suspended frames as `activationSender:` does; answers `nil` past the run's base or for a subject that has returned. The enabled test is inside the leaf, so `activationIsActiveHandler:` goes.
- **H2.** The second walk stays, as `count` calls of the leaf; a search pushes no activation it did not push before; order, disabling and outcome unchanged.
- **H3.** `activationSender:` stays (`Kernel.ns:123`, later mirrors).
- **H4.** A microbenchmark `benchmarks/micro/bench_signal.ns`, a golden, measured by the `measuring-performance` skill's protocol against `master`.
- **H5.** Docs, `Kernel.ns` documentation, `OPEN-WORK.md`'s item removed; per-commit gates, then end-of-batch.

## Global Constraints

- Names are written out (`CLAUDE.md`, "Conventions"): `activation`, `frame`, `walked`, never `act`, `fr`, `w`.
- Every member of `Kernel.ns` has a `///` comment to Javadoc's standard (`crates/ns-runtime/tests/core_library_documentation.rs`); no other implementation named in it.
- After editing a `.ns` file: `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/Kernel.ns` (and the benchmark file), never at the repository root.
- Program outcomes are pinned: no golden, `run-corpus` output or benchmark outcome may change.
- `vm.rs` gains nothing; the leaf lives in `crates/ns-runtime/src/builtins/activation.rs`, frame access through `heap.rs`'s accessors.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()` an `EvaluationResult`; tests use the file's `expect_ok`.
- Commits: conventional (`feat:`, `test:`, `docs:`, `perf:`), each ending with
  ```
  Co-Authored-By: <the implementing model> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_018EAkoZD7F8A8gyvX8FmZTu
  ```
- Gates: every commit runs `cargo xtask gates commit` (it changes Rust or `.ns`); only the last task runs `cargo xtask gates end-of-batch`. Every command in the foreground, `timeout: 600000` on long ones.
- Work on the checked-out branch `feature/handler-search-leaves` in the main working directory; no worktree.

## Review Focus

- **A handler found in a suspended frame** (under `NS_NEXT_STRESS_SUSPEND`): the leaf must walk past a live frame whose sender is suspended into the suspended frames, as `activationSender:` does; the stress gate's goldens exercise it, and Task 2's walk test covers it plainly.
- **A disabled `on:do:` between signal and handler** (a signal inside a handler): the leaf must skip it, not answer it; `goldens/` covering nested handlers pin it, and Task 2 adds a direct test.
- **`handles:` raising inside the search**: the candidate is disabled while `handles:` runs, so the nested search's leaf must skip it; existing goldens pin the outcome.
- **The depth reserve**: the search must push no activation it did not push before; `crates/ns-runtime/tests/exceptions.rs`'s reserve tests (`passInTheReserveAt:`, `measureTheReserve`) pin the counts and must pass unchanged.
- **A subject that has returned** (an `Exception` whose `handlerActivation` is dead): the leaf answers `nil`, as `activationSender:` does for a returned activation.

---

### Task 1: The benchmark, first, so `master` can be measured with it

**Files:**
- Create: `benchmarks/micro/bench_signal.ns`, `benchmarks/micro/bench_signal.toml`

**Interfaces:**
- Produces: the measurement source Task 3 runs against `master` and the branch.

- [ ] **Step 1: Write `bench_signal.ns`**: `class BenchSignal = Object new (...)` in `bench_block.ns`'s shape, with `descend: depth` recursing to `depth = 0` and there `^Error new signal`, and `main: platform args: arguments` running `5000` times `sum:: sum + ([self descend: 50] on: Error do: [:error | error return: 1])` and printing `sum` with `p transcript show: ...; newLine`.
- [ ] **Step 2: Format it**: `cargo run -p ns-analyzer -- fmt benchmarks/micro/bench_signal.ns`.
- [ ] **Step 3: Record its outcome**: `cargo run -p ns-cli -- run benchmarks/micro/bench_signal.ns` prints `5000`; `bench_signal.toml` is `stdout = '''\n5000\n'''` as `bench_block.toml` is.
- [ ] **Step 4: Run the golden under both lowerings**: `cargo nextest run --workspace -E 'test(bench_signal)'`; expected: passes. (If the goldens runner names it otherwise, run the whole default profile.)
- [ ] **Step 5: Gates and commit**: `cargo xtask gates commit`, then `test: bench_signal.ns, a signal caught fifty activations down`.

### Task 2: The leaf, and the search over it

**Files:**
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`ActivationPrimitive` at ~`:775-794`, its arm at ~`:270-280`, the mirror table at ~`:1384-1420`)
- Modify: `crates/ns-runtime/src/builtins/activation.rs` (`execute`, `SubjectLeaf`, `activation_leaf`, `handler_registers`)
- Modify: `crates/ns-runtime/src/interpreter/unwind.rs` (`WalkedActivation`, only if it needs a constructor from a frame; see Step 3)
- Modify: `crates/ns-runtime/core-library/Kernel.ns:756-803` (`invokeNextHandler` and its comment)
- Modify: `docs/design/core-library.md` ("Exceptions", the bullet "The handler search keeps no `OrderedCollection`, built.", ~`:4386`)
- Test: `crates/ns-runtime/tests/exceptions.rs`

**Interfaces:**
- Produces: `ActivationPrimitive::NextHandler`, registered as `mirror("activationNextHandler:ifFail:", "nextHandler", Primitive::Activation(ActivationPrimitive::NextHandler))`; `ActivationPrimitive::IsActiveHandler` and `"activationIsActiveHandler:ifFail:"` removed.

- [ ] **Step 1: Write the failing test** in `exceptions.rs`, inside `ns_runtime::for_each_lowering!`:
  `fn a_handler_search_reifies_no_frame_it_passes(lowering)`. A module `Signaller` with `public catchFrom: depth = ( ^[self descend: depth] on: Error do: [:error | error return: 1] )` and `descend: depth = ( depth = 0 ifTrue: [^Error new signal]. ^self descend: depth - 1 )`. Turn off `set_stress_collect`, `set_stress_scavenge` and `set_stress_suspend` (a suspension reifies every frame; `stress.rs`'s `run_every_continuation` does the same for the same kind of reason), send `catchFrom: 5` once to warm every cache, then measure `heap_statistics().live_objects` around one `catchFrom: 10` and around one `catchFrom: 200`:
  ```rust
  assert_eq!(objects_at_200, objects_at_10, "{lowering:?}: the search reifies only the handler it answers, so a signal's allocation does not grow with the frames it passes");
  ```
  and that each send answers `1`.
- [ ] **Step 2: Run it to see it fail**: `cargo nextest run -p ns-runtime --test exceptions a_handler_search_reifies_no_frame_it_passes`; expected: FAIL, the deep send allocating about 2 × 190 frame objects more.
- [ ] **Step 3: Implement `ActivationPrimitive::NextHandler`** in `activation.rs`, as a `SubjectLeaf::NextHandler` arm: for a running subject frame, start at its activation as a `WalkedActivation` (`Live` from `Heap::frame_activation`, else `Suspended(frame)` for a suspended one), step with `WalkedActivation::sender`, and at each read the marker (`Heap::activation_unwind_marker` for `Live`, `Heap::frame_unwind_marker` for `Suspended`) and, for `UnwindMarker::Handler`, its `active_register` (`Heap::activation_local` / `Heap::frame_local`); the first holding `true` answers `mutator.new_activation_object(frame)` with `frame` from `Heap::reify` for `Live` and the frame itself for `Suspended`; `Value::nil()` when the walk ends or the subject has returned. Add the constructor or accessor `WalkedActivation` lacks rather than reading its variants from `activation.rs` if the type's privacy asks for it. Register it in `primitive.rs` beside `Sender`, in every exhaustive match, and remove `IsActiveHandler` and its arm in `activation_leaf`.
- [ ] **Step 4: Rewrite `invokeNextHandler`** in `Kernel.ns`: both walks step with `vmMirror activationNextHandler: … ifFail: [nil]` and no longer test each activation; the first starts from `handlerActivation`, counts every activation the leaf answers and asks `isHandledAt:`; the second starts from `handlerActivation` again and stores `count` answers into `handlers`, stepping only while another is still to come. Update the method's `///` comment: what the leaf answers, that the walk reifies only the `on:do:` activations it stops at, and the existing reasons for the two walks and for pushing no extra activation. Run `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/Kernel.ns`.
- [ ] **Step 5: Run the test and the exception suites**: `cargo nextest run -p ns-runtime --test exceptions`, and `cargo nextest run --workspace -E 'test(/exception|handler|signal|ensure|retry|pass|outer|resume/)'`; expected: all pass, the new test included.
- [ ] **Step 6: Add the disabled-handler test**: `fn a_handler_search_skips_a_disabled_handler(lowering)`: an `on: Error do:` whose handler signals a second `Error`, caught by an outer `on: Error do: [:error | error return: 2]`; the inner handler's `on:do:` is disabled while it runs, so the outer one answers. Assert the outer answer `2`. It passes at once (the behaviour is unchanged); it pins H1's "enabled" test through the leaf.
- [ ] **Step 7: Update `core-library.md`**: in "Exceptions", the handler search's bullet says the search walks through `activationNextHandler:`, which reifies only the `on:do:` activations it answers, as Cog's handler search marries only the context it finds (`primitiveFindHandlerContext`, `prior-art/opensmalltalk-vm/src/spur64.sista/cointerp.c:84085-84112`), where walking the senders reified every frame it passed.
- [ ] **Step 8: Gates and commit**: `cargo xtask gates commit`; `perf: the handler search reifies only the handlers it answers`.

### Task 3: Measure, and close the batch's documents

**Files:**
- Modify: `docs/design/execution.md` (a measurement section "What the handler-search leaf moved", after "What `new` as compiled code moved"; and "Planned: the deoptimization contract", "Observers", first bullet)
- Modify: `docs/OPEN-WORK.md` ("Planned: performance and representation", the first item)
- Modify: `docs/PROGRESS.md` (an entry and a table row)

- [ ] **Step 1: Measure** with the `measuring-performance` skill's protocol: `master` against the branch, three release builds a side with the explicit `RUSTFLAGS` it names, on `bench_signal.ns` and on the Are We Fast Yet programs, so that nothing else moved. Expected: `bench_signal.ns` faster; no other program slower beyond its builds' spread. A slower program is diagnosed and fixed before going on.
- [ ] **Step 2: Record it** in `execution.md`'s new section: the commits, the table of both sides per source, and what the change removed (two reifications per frame passed per signal).
- [ ] **Step 3: Correct the contract's "Observers" bullet**: the handler search *is* a leaf now; the search for unwind markers is the Rust unwinder's, which reifies nothing, so there is no second leaf to build.
- [ ] **Step 4: Close the open item** in `OPEN-WORK.md` ("The handler search and the search for unwind markers walk the frames themselves"), and add a `PROGRESS.md` entry `## The handler search as a leaf (2026-10-27)` with its commits, plus a row in the milestone table.
- [ ] **Step 5: End-of-batch gates**: `cargo xtask gates end-of-batch` (`images`, `caches`, `benchmarks`, `release-checks`, `codegen`, `stress`); every suite passes, `run-corpus` "ran 6, failed 0, skipped 205".
- [ ] **Step 6: Commit**: `docs: what the handler-search leaf moved`.
