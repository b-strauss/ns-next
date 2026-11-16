# Toolchain 1.100.0 and Edition 2024 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The workspace builds on Rust 1.100.0 in edition 2024, warning-free on every target the gates lint, with no change of behaviour, and it uses what the new compiler, edition and standard library make possible wherever that makes the code plainer.

**Architecture:** There are three stages, each passing its gates. (1) Raise the pin: `rust-toolchain.toml`, `rust-version` and the Docker image go to 1.100.0, then fix the new warnings. (2) Move the edition: `cargo fix --edition` on every target whose code the host does not compile, one commit that formats the tree in style edition 2024, then the switch itself with `resolver = "3"`, reviewing every rewrite the migration lints made. (3) Refactor with what the new version allows. Then the documentation, a measurement against `master`, and every gate, the Docker groups included.

**Tech Stack:** Rust 1.100.0 (2026-11-12, LLVM 23 since 1.99), edition 2024, `cargo-nextest`, `cargo-show-asm`; no new external dependency.

**Spec:** `docs/OPEN-WORK.md`, "Next", the first item (agreed with the user on 2026-11-14). The user also asked on 2026-11-16 that the plan look over the codebase for what the update lets us improve or refactor. The survey of that is "What the new version allows", below. The facts about 1.99 and 1.100 are from the official release notes (`doc.rust-lang.org/stable/releases.html`) and the release posts of 2026-10-01 and 2026-11-12.

## What the new version allows: the survey and what is adopted

The counts are from `crates/` and `xtask/` on `master` at `31526007`.

**Adopted (Tasks 2 and 3):**

| What | Since | Where it applies here |
|---|---|---|
| `let` chains (`if let … && let … && cond`) | edition 2024 | about 150 nested `if let { if … }` with no `else` in between; Clippy's `collapsible_if` flags them under 2024 |
| `if let` temporaries dropped before `else`; tail-expression temporaries dropped before locals | edition 2024 | removes the trap of a `RefCell` borrow held into `else`; the migration's rewrites to `match` are reviewed and undone where nothing with a significant `Drop` is involved |
| `impl Trait` captures every lifetime in scope; `use<…>` | edition 2024 | 42 return-position `impl`s, 16 of them with `+ '_`, 5 naming `'gc`; a `+ '_` that 2024 makes redundant goes |
| `unsafe extern` | edition 2024 (required) | the two `extern "C"` blocks in `crates/ns-memory/src/platform/posix.rs` |
| `resolver = "3"` (resolution that knows `rust-version`) | edition 2024 | the workspace's `Cargo.toml`, `resolver = "2"` now |
| style edition 2024 of rustfmt | edition 2024 | the whole tree, one formatting commit, listed in `.git-blame-ignore-revs` |
| `str::trim_prefix`/`trim_suffix`, `Path::trim_prefix` | 1.100 | 7 calls of `strip_prefix(x).unwrap_or(s)` / `strip_suffix` |
| `String::from_utf8_lossy_owned` | 1.99 | 6 calls of `String::from_utf8_lossy(&owned).into_owned()` where the bytes are owned and not used after |
| `bool::toggle` | 1.100 | `crates/ns-syntax/src/doc_comment.rs:91` |
| `LocalKey<Cell<T>>::update`, and `LocalKey::set`/`get`/`replace` directly | 1.99 (`update`) | 12 `.with(\|cell\| cell.set/get/replace(…))` in `heap/frames.rs` and `heap/pending.rs` |
| `Cell::update` | already stable, not used | `x.set(x.get() / 2)` and `+ 1` in `jit/mod.rs` (914, 915, 3068, 4451) and `jit/zone.rs:1168` |
| `core::mem::DropGuard` | 1.100 | the two reset guards made of a local `struct` + `impl Drop` in `heap/frames.rs:53` and `jit/inlining_tests.rs:1626` |
| `Vec::from_fn` | 1.100 | `(0..count).map(…).collect::<Vec<_>>()` that builds a `Vec` by index, about 10 calls, tests mostly (not the `String` or `Result` collects) |
| default-features overridable on an inherited workspace dependency | 1.99, edition 2024 | `iced-x86`, declared identically in `ns-assembler` and `ns-runtime`, becomes one `[workspace.dependencies]` entry |
| Cargo's lint `cargo::unused_dependencies` | 1.100, on by default | whatever it reports is removed, or kept with a reason |

**Examined and not adopted.** Each row says why, so that nobody examines it again:

| What | Since | Why not |
|---|---|---|
| The never type `!`, `Infallible = !` | 1.100 | no `Infallible`, no empty enum and no `Result<_, Never>` in the tree; the three `-> !` functions need nothing new |
| The `Allocator` API, `Vec::new_in`, `Box::new_in` | 1.100 | every table beside the object space is a `Vec` on the global allocator, and giving memory back is done by `heap/limits.rs`'s shrinking. Placing a table in a reservation of its own would be a memory-design question with no measured need behind it, and it would let `unsafe` reach `ns-runtime`. Not a refactor |
| `extern "custom"`, C-variadic definitions, `VaList`, 128-bit vector `asm!` | 1.99/1.100 | machine code is written into the code zone at run time and has no Rust symbol; the tree has no `asm!` and no variadic C function |
| `UnsafeCell::replace`/`as_ref_unchecked`, `Vec::from_parts`, `Box::into_non_null`, `size_of_val_raw` | 1.99/1.100 | no `UnsafeCell` and no raw `Vec`/`Box` round trip in the tree; `ns-memory` uses `slice::from_raw_parts` only |
| `Box::take`, `Box::map`, `Rc::map`, `Arc::map`, `VecDeque::retain_back` | 1.100/1.99 | no call site whose code they would shorten |
| `become` (explicit tail calls), `loop_match` | — | still unstable in 1.100.0 (not in either release's notes). `execution.md` and `OPEN-WORK.md` say "experimental in Rust 1.98.0 (checked on 2026-10-02)" and are re-dated to 1.100.0 and the day of the check |
| The `debug` profile | 1.99 | it is the `dev` profile under another name for now. The plugin and the tests take `target/debug/`, which stays the `dev` profile's |
| `expr` matching `const {}` and `_` | edition 2024 | the two macros in `vm.rs` (505, 529) are private and only ever given ordinary expressions. `cargo fix` rewrites them to `expr_2021` to be safe, and that is undone: they stay `expr` |

**Compatibility notes that could reach the tree,** each checked in the task that meets it: the never-type fallback changed in every edition, which leads to new type errors, not to new behaviour; closures capture more precisely in `if let`; `str::escape_debug` always escapes grapheme extenders, and it reaches any `{:?}` of a `str` in a diagnostic, so the goldens and the analyzer's tests are the check; Unicode 18 (the lexer uses no Unicode predicate, only `positions.rs:390` and `printer/document.rs:213` call `char::is_whitespace`); Cargo's new layout of intermediate build artifacts, which reaches `cargo asm`, `nextest` and `xtask`'s fixed paths (`target/codegen`, `target/shipped-code`, `target/reach`, `target/xtask/nextest.toml`), which name directories `xtask` passes as `--target-dir` or writes itself, never Cargo's internals; LLVM 23, which reaches the `codegen` budgets and the interpreter's speed.

## Global Constraints

- **No change of behaviour.** Every golden, `run-corpus` and every test stays as it is; a golden or test that changes means the migration changed semantics, and it is fixed in the code, never in the record (`CLAUDE.md`, "Program outcomes are pinned").
- No new external dependency. `ns-memory` stays the only crate with `unsafe`; `ns-runtime`, `ns-assembler`, `ns-tooling` and `ns-analyzer` keep `forbid(unsafe_code)`.
- The toolchain is exactly `1.100.0` everywhere: `rust-toolchain.toml`'s `channel`, the workspace's `rust-version = "1.100"`, and the Docker image's `--default-toolchain 1.100.0`.
- Names are written out (`CLAUDE.md`, Conventions); `clippy::min_ident_chars` holds. A closure parameter a refactor introduces is named for what it holds (`|count| count + 1`, never `|c|`).
- Code that only another target compiles is migrated too: `cfg(ns_jit)` on AArch64, `ns-memory`'s `platform/posix.rs`, and code compiled only for Apple or only for Windows on ARM. The targets are `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin` and `aarch64-pc-windows-msvc` (the rustup targets of the `memory` and `release-checks` groups).
- Every commit that changes more than Markdown runs `cargo xtask gates commit`. A commit touching `crates/ns-memory` also runs `memory`, one touching `crates/ns-analyzer` `analyzer`, and one touching `vm.rs`, `interpreter/mod.rs`, `interpreter/sends.rs`, `heap.rs`, `heap/frames.rs` or `bytecode/compact.rs` `codegen`. The controller alone runs `end-of-batch`, `linux` and `aarch64`, after the branch review; every commit before them is work in progress. Subagents run every command in the foreground with `timeout: 600000` on long ones.
- A commit's diff holds one kind of change: a migration, a formatting, a refactor. The formatting commit holds nothing but `rustfmt`'s output.
- Every document statement a task makes untrue is rewritten in the same commit, in the present tense.

## Review Focus

1. **Code the host does not compile.** `cargo fix --edition` on Windows x64 never sees `platform/posix.rs`, the AArch64 encoder's `cfg`-gated users or Apple's `MAP_JIT` path. Expected: `cargo clippy` for each of the three other targets is warning-free in edition 2024 before the switch is committed (Task 2, Step 4).
2. **A temporary whose drop now moves.** A `RefCell` borrow, a `MutexGuard`, a `Root` or an `Interpreter::enter` closure's guard in an `if let` scrutinee or a block's tail now drops earlier. Expected: the migration lints' rewrites (`if_let_rescope`, `tail_expr_drop_order`) are each kept or undone by name in the commit message, and the suite, the stress modes and the goldens are unchanged (Task 2, Step 3; Task 6).
3. **The `'gc` brand.** The new capture rule could let a handle-carrying `impl Iterator` outlive its `Mutator`. Expected: the three `compile_fail` doctests on `Interpreter::enter` still fail to compile, and fail for the reason they name (Task 2, Step 6).
4. **The hot path's machine code under LLVM 23.** Expected: `cargo xtask gates codegen` passes with its budgets unchanged. A function over budget is diagnosed in its listing, and its budget is raised only with the reason recorded in `xtask/src/codegen.rs` (Task 1, Step 5). The benchmarks are no slower than on `master` by the `measuring-performance` protocol (Task 6).
5. **Tools that read Cargo's build directory.** Expected: `cargo asm` finds `ns-runtime`'s lib, `nextest` its test binaries, and the plugin `target/debug/ns-next`, after 1.100's new layout. A tool that fails is updated to a release that knows the layout, and the release is pinned where the tool is pinned (Task 1, Step 4).

---

### Task 1: The pin raised to 1.100.0

**Files:**
- Modify: `rust-toolchain.toml` (`channel = "1.100.0"`)
- Modify: `Cargo.toml` (`rust-version = "1.100"`)
- Modify: `xtask/docker/Dockerfile` (`--default-toolchain 1.100.0`; the comment above it if it names the version)
- Modify: whatever the new rustc, Clippy and Cargo warnings name
- Modify: `xtask/src/codegen.rs` only if a budget must move (Step 5)

**Interfaces:** Produces a tree that builds on 1.100.0 in edition 2021, which Task 2 migrates.

- [ ] **Step 1: Install the toolchain and the targets**

Run: `rustup toolchain install 1.100.0 --profile minimal --component rustfmt,clippy --target x86_64-unknown-linux-gnu,aarch64-apple-darwin,aarch64-pc-windows-msvc`
Expected: installed. Then `rustc --version` in the repository prints `rustc 1.100.0` once the pin is changed.

- [ ] **Step 2: Change the three pins**

- [ ] **Step 3: Fix every new warning**

Run: `cargo clippy --workspace --all-targets -- -D warnings`, then the same with `--target` for each of the three other targets, the way the `memory` and `release-checks` groups run them (`-p ns-memory` and `-p ns-runtime`). Fix each new lint at its cause. An `#[expect(…)]` needs a `reason` that survives review. Cargo's `unused_dependencies` reports appear in a full `cargo build --workspace --all-targets`. Remove each dependency it names, or, if it is used only under a `cfg` the lint cannot see, keep it with a `[lints.cargo]` entry and a comment that says so.

- [ ] **Step 4: Check the tools that read the build directory**

Run: `cargo xtask gates codegen`, then `cargo nextest list --workspace | head`.
Expected: both find their artifacts. If `cargo asm` cannot find the lib, run `cargo install cargo-show-asm --locked` (1.100 makes `--locked` the default; the flag stays harmless), and name the version that works in `codegen.rs`'s `INSTALL` comment.

- [ ] **Step 5: The codegen budgets**

Expected: `codegen` passes unchanged. If a function is over budget, read its listing in `target/codegen`, say in the commit what LLVM 23 does differently, and raise the budget only if the code is no worse on the measured path (Task 6 decides that). A spill on the send path that was not there before is a regression to fix, not a budget to raise (`execution.md`, "Keeping the hot path in registers").

- [ ] **Step 6: Gates and commit**

Run: `cargo xtask gates commit memory codegen`
Expected: all pass.

```bash
git commit -am "chore: Rust 1.100.0"
```

### Task 2: Edition 2024

**Files:**
- Modify: `Cargo.toml` (`edition = "2024"`, `resolver = "3"`; `iced-x86` into `[workspace.dependencies]`)
- Modify: `crates/ns-assembler/Cargo.toml`, `crates/ns-runtime/Cargo.toml` (`iced-x86 = { workspace = true }`)
- Modify: every file `cargo fix --edition` and Clippy's `collapsible_if` touch
- Create: `.git-blame-ignore-revs`
- Create, then delete: `rustfmt.toml` (Steps 4 and 5)

**Interfaces:**
- Consumes: Task 1's tree.
- Produces: edition 2024 throughout, which Task 3 relies on (`let` chains, `Vec::from_fn` in the tests).

- [ ] **Step 1: The migration on the host and the three other targets**

Run, still in edition 2021: `cargo fix --edition --workspace --all-targets --allow-dirty`, then `cargo fix --edition --workspace --all-targets --allow-dirty --target <target>` for `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin` and `aarch64-pc-windows-msvc`. A target that fails to link a test binary is checked with `cargo check` instead (cross-checking needs no linker; `fix` runs `check`).

- [ ] **Step 2: The migration lints' rewrites reviewed**

Read the whole diff. Keep `unsafe extern` (posix.rs) and every `use<…>` the capture rule needs. Undo each `expr_2021` (Survey, "not adopted"). Undo each `if let`→`match` rewrite (`if_let_rescope`) and each tail-expression rewrite (`tail_expr_drop_order`) whose temporary has no significant `Drop`, i.e. is not a `RefCell` borrow, a lock guard, a `Root` or a type of the tree with an `impl Drop` (the 13 in the survey's grep). Keep, or turn into a plain statement, every one that has such a `Drop`, and say in the commit message why its 2024 order is right or why the old order is kept. Remove each `+ '_` that 2024's capture rule now implies, where the doc comment does not need it to show the borrow.

- [ ] **Step 3: Commit the migration (still edition 2021)**

Run: `cargo xtask gates commit memory`
```bash
git commit -am "chore: the edition 2024 migration, in edition 2021"
```

- [ ] **Step 4: The formatting commit**

Create `rustfmt.toml` with `style_edition = "2024"`, so the commit group's own `cargo fmt --all` keeps the new style while the edition is still 2021. Run `cargo fmt --all`; nothing but `rustfmt.toml` and `rustfmt`'s output changes in this commit. Run `cargo xtask gates commit memory`.
```bash
git commit -am "style: the tree in rustfmt's style edition 2024"
```
Then create `.git-blame-ignore-revs` with that commit's full hash and a comment line `# rustfmt style edition 2024`, committed with Step 5. Step 5 also deletes `rustfmt.toml`, since edition 2024 implies its style edition.

- [ ] **Step 5: The switch**

Set `edition = "2024"` and `resolver = "3"`; move `iced-x86` into `[workspace.dependencies]` (`version = "1.21", default-features = false, features = ["decoder", "intel", "std"]`), the two crates `{ workspace = true }`. Then:
Run: `cargo clippy --workspace --all-targets -- -D warnings`, and for each of the three other targets.
Fix what the switch raises. Clippy's `collapsible_if` now asks for `let` chains: apply it with `cargo clippy --fix --workspace --all-targets --allow-dirty -- -A clippy::all -W clippy::collapsible_if`, and repeat it per target. Read every collapsed condition: the chain keeps the order of evaluation of the nesting, and a collapsed `if` whose inner condition had a comment keeps the comment above the chain's line for it. With `rustfmt.toml` deleted, `cargo fmt --all` must now be a no-op beyond the collapsed sites.

- [ ] **Step 6: The brand's doctests**

Run: `cargo test -p ns-runtime --doc enter`
Expected: the three `compile_fail` doctests on `Interpreter::enter` pass, and running each body as a normal test (temporarily, not committed) fails with the error its doc names (a borrow outliving `'gc`, not a new capture error).

- [ ] **Step 7: Gates and commit**

Run: `cargo xtask gates commit memory analyzer codegen release-checks`
Expected: all pass; no golden changed.
```bash
git add -A && git commit -m "chore: edition 2024, let chains where nesting only guarded"
```

### Task 3: What 1.99 and 1.100 make plainer

**Files (from the survey; the lines are `master`'s):**
- `crates/ns-analyzer/tests/corpus.rs:146`, `tests/formatting_gate.rs:45` (`Path::trim_prefix`)
- `crates/ns-assembler/tests/encoding_sources.rs:240`, `crates/ns-syntax/src/comment.rs:109,110,116`, `parser/expression.rs:889` (`str::trim_prefix`/`trim_suffix`)
- `crates/ns-cli/src/commands/run_corpus.rs:252,253,331,332`, `crates/ns-cli/tests/common/mod.rs:549`, `xtask/src/main.rs:286` (`String::from_utf8_lossy_owned`)
- `crates/ns-syntax/src/doc_comment.rs:91` (`bool::toggle`)
- `crates/ns-runtime/src/heap/frames.rs:53-67`, `heap/pending.rs:146-188` (`LocalKey::set`/`get`/`replace`/`update`; `DropGuard` in `with_stack_reservation`)
- `crates/ns-runtime/src/jit/inlining_tests.rs:1626` (`DropGuard`)
- `crates/ns-runtime/src/jit/mod.rs:914,915,3068,4451`, `jit/zone.rs:1168` (`Cell::update`)
- `Vec::from_fn`: `bytecode/compact.rs:3274`, `bytecode/leftovers/tests.rs:434,623`, `bytecode.rs:2582`, `heap.rs:15797,19275`, `tests/image.rs:892,3079`, `tests/image_truncations.rs:295`

**Interfaces:** Consumes Task 2's edition. Produces nothing later tasks use.

- [ ] **Step 1: The calls rewritten**

The rules:
- `s.strip_prefix(p).unwrap_or(s)` becomes `s.trim_prefix(p)`, and the same for `strip_suffix`. This applies only where the fallback is the receiver itself.
- `String::from_utf8_lossy(&bytes).into_owned()` becomes `String::from_utf8_lossy_owned(bytes)`. This applies only where `bytes` is an owned `Vec<u8>` that nothing reads afterwards; take it out of `out.stdout` by move. Where both `stdout` and `stderr` of one `Output` are read, destructure it.
- A `thread_local!` `Cell` is read and written through `LocalKey`'s own `get`, `set`, `replace` and `update`. The counters in `pending.rs` become `LOOKUPS.update(|count| count + 1)`.
- `x.set(x.get() / 2)` becomes `x.update(|count| count / 2)`.
- The reset guards become `let _reset = core::mem::DropGuard::new((), |()| STACK_RESERVATION_OVERRIDE.set(None));`. Check `DropGuard::new`'s signature in `rustup doc --std core::mem::DropGuard` first. If it takes the value and a `FnOnce(T)`, the guard holds `()`. The comment that says the override ends with `body`, a panic in it included, stays.
- `(0..count).map(f).collect::<Vec<_>>()` becomes `Vec::from_fn(count, f)` where `f` takes the index. Check the signature in `rustup doc --std` first. Collects into a `String` or a `Result` stay as they are.

The real resources with an `impl Drop` (`ScratchDirectory`, `Worker`, `DapClient`, `CodeZone`, `Reservation`, `MixinApplication`, `Send`, `Document`, `TemporaryDirectory`) stay types of their own. A `DropGuard` is for a one-off undo only.

- [ ] **Step 2: Gates and commit**

Run: `cargo xtask gates commit analyzer codegen` (`heap/frames.rs` and `heap.rs` are `codegen` files).
Expected: all pass; the `codegen` budgets unchanged (only test-only and cold code changed).
```bash
git commit -am "refactor: trim_prefix, from_utf8_lossy_owned, LocalKey::update, DropGuard and Vec::from_fn"
```

### Task 4: The documents

**Files:**
- Modify: `CLAUDE.md:5` ("Rust 1.100.0, edition 2024")
- Modify: `docs/design/README.md:5` (same)
- Modify: `docs/design/execution.md` (line 11038 and line 11611: `become` re-checked on 1.100.0 on the day of the check, still unstable; line 15850: the image's Rust 1.100.0). The measurement record at line 2356 names the compiler it measured with and stays.
- Modify: `docs/OPEN-WORK.md` (remove the toolchain item from "Next"; at line 1461 the `become` check re-dated)
- Modify: `docs/PROGRESS.md` (the entry for this branch, its gates filled in by the controller at the end)
- Modify: every other `.md` that names 1.98 as the toolchain. Find them with `grep -rn "1\.98\.0\|edition 2021" --include=*.md . | grep -v prior-art | grep -v superpowers/plans`; measurement tables that only contain the number 1.98 as a value are not hits.

- [ ] **Step 1: Rewrite each statement in the present tense; commit**
```bash
git commit -am "docs: Rust 1.100.0 and edition 2024"
```

### Task 5: Every target in Docker

The controller runs this task, not a subagent.

- [ ] **Step 1:** Run `cargo xtask gates linux aarch64`. The image is rebuilt for the new toolchain.
Expected: both pass. A failure that only Linux or AArch64 shows is fixed in a commit of its own, and that commit runs its gates.

### Task 6: The measurement and the end of the batch

The controller runs this task, after the whole-branch review.

- [ ] **Step 1: The benchmarks, branch against `master`.** Follow the `measuring-performance` skill: release builds of both trees, its protocol, its table. A benchmark that is slower outside the noise is diagnosed and fixed at once, in the listing and in the code (the memory rule "fix perf regressions directly"). Where the cause is LLVM 23 and not the tree, the reason and the listing go into `execution.md`, "Keeping the hot path in registers". The result goes into `execution.md` next to the earlier measurements, with rustc 1.100.0 named.
- [ ] **Step 2:** Run `cargo xtask gates end-of-batch`.
Expected: all pass, `run-corpus` under every mode included.
- [ ] **Step 3:** Fill in `PROGRESS.md`'s gates line, then follow the `finishing-a-batch` skill.
