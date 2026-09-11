# Clean Up With One Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Behaviour-preserving cleanup of the runtime now that no code serves a
second engine, plus three hot-path changes the cleanup exposed, each measured,
and one design change the plan was asked to decide: compiled code moves onto
the declaration it belongs to.

**Architecture:** Tests are tightened first, so every later task runs against
exact recorded answers. Then `interpreter/` loses the leftovers of the
tree-walker (`eval.rs` is split by responsibility), three measured hot-path
changes land one at a time (builtin classes by index, pre-interned symbols,
closures behind an `Rc`), and compiled code moves from the interpreter's
`HashMap`s keyed by `Rc` address onto `MixinMethod` and `Mixin`, where every
system in `prior-art/` keeps it. Handle parameters then go by value under a
clippy lint, `ns-cli/src/main.rs` splits into one module per command, a comment
sweep removes history narration, and the last task measures the whole step and
records it.

**Tech Stack:** Rust 1.98.0, edition 2021, no external dependencies. Three
crates: `ns-syntax` (untouched), `ns-runtime`, `ns-cli`.

**Spec:** [docs/PROGRESS.md](../../PROGRESS.md), "Where the next session starts",
step **B**, and its inventory: the list under "M4 — Retiring the tree-walker"
beginning "**Left for B**", the list after it ("The retirement's plan ... also
assigned B these"), and "Found by the retirement's final review, and also B's".
Read all three before Task 1. Design context:
[execution.md](../../design/execution.md) ("The VM", "Planned: the trampoline"),
[compiler.md](../../design/compiler.md) ("Compiled code", "Keyed by mixin"),
[dispatch.md](../../design/dispatch.md) ("Planned: inline caches"),
[core-library.md](../../design/core-library.md) ("Planned: the leaf/callback
split"), [memory.md](../../design/memory.md) ("Open: what the collector owns").

## Decisions this plan carries

Two inventory items needed a decision before the plan could be written. Both
were settled with the user on 2026-09-11, grounded in `prior-art/`:

1. **Compiled code on `MixinMethod` / `Mixin`: in B (Task 6).** PROGRESS
   flagged the tension with "B changes no design"; the decision is to take the
   design change here, with the design documents updated in the same task.
   Every system in `prior-art/` keeps a method's code on the method and has no
   side table: Primordial Soup's `Method::bytecode()`
   (`prior-art/primordialsoup/vm/object.h`), SOMns's `SInvokable.invokable`
   final field (`prior-art/somns/src/som/vmobjects/SInvokable.java`), Squeak's
   `CompiledMethod` in the method dictionary. It removes a SipHash lookup from
   every user-method send and every initializing level of an instantiation,
   removes the collector hazard `memory.md` records for a code table keyed by
   `Rc` address, and makes "a method with no compiled body" unrepresentable.
2. **Pre-interned symbols: only the symbols the runtime names in its own code
   (Task 4).** Native dispatch keeps its `match selector.as_str()` in B. The
   end state that performs best is not a faster `match` but Cog's and Primordial
   Soup's: a lookup binds an **enumerated primitive**, a native inline-cache
   entry carries it, and the JIT treats it as an intrinsic (Cog's
   `primitiveGeneratorTable` and `genPrimitiveAdd`, and at the send site
   `genSpecialSelectorArithmetic`, in `prior-art/opensmalltalk-vm/src/spur64.cog/cogit*.c`).
   An opaque Rust function pointer would cap a JIT exactly as a Rust callback
   does. Task 10 records that decision in `dispatch.md` and `core-library.md`;
   it is implemented with the inline caches, not here.

## Scope

This is step **B** of the three steps PROGRESS schedules before M4's 1b.
Deliberately **not** in this plan:

- **Error text that names a milestone.** Two messages a program can observe
  say "Milestone 1": `builtins/numbers.rs`'s `type_mismatch`
  ("... (Milestone 1 has no Integer/Float coercion) ...") and
  `parse_int_literal`'s "fractional radix literals are not supported in
  Milestone 1". Changing them changes stderr, which B must not do. Task 10
  records them in PROGRESS as a behaviour change for later.
- **Native dispatch by selector string** — see decision 2.
- **The wasted `Object` allocation in `build_builtin_classes`.** Its loop
  allocates a class object for `BuiltinClass::Object` and then discards it in
  favour of `class_of_class`. Removing the allocation would shift every later
  identity hash by one, which a program and an image both observe. Task 3 keeps
  it.
- **`ClosureData.params` duplicating `code.num_args`.** Dropping the names
  changes the image's closure record. Task 5 shares the vector instead of
  copying it.
- **A slot lookup and a symbol intern per lexical send.**
  `sends::slot_selector_shape` interns the getter name for every setter-shaped
  lexical send, and `dispatch::lookup_slot` hashes the name for every one. Both
  are hot, and both are what a lexical-send inline-cache entry removes. Task 10
  records it in `dispatch.md`.
- **The `bench_block.ns` profile** PROGRESS asks for before 1b. Task 5 measures
  the closure change on it; the profile stays a pre-1b item.
- **The reference lowering (step C), the trampoline (1b)**, and anything under
  `crates/ns-syntax`.

## Global Constraints

- Rust 1.98.0, edition 2021. **No new external dependency.**
- All code, identifiers, comments, doc comments and commit messages in English.
- Classic terminology: `blacklist`, `whitelist`, `sane`.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`,
  `refactor:`). **Every commit — including one made by a subagent — ends with
  exactly these two trailer lines:**
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv
  ```
- Never override git author or committer identity.
- Work on the branch `feature/clean-up-with-one-engine`, checked out in the main
  working copy (no `EnterWorktree` isolation). Before Task 1, create it from
  `master` with `git switch -c feature/clean-up-with-one-engine` and commit this
  plan there as its first commit (`docs: plan step B, cleaning up with one
  engine`).
- `cargo fmt --all` before every commit; `cargo clippy --workspace --all-targets`
  must be warning-free; `cargo test --workspace` must pass at every commit.
- Both corpus gates hold at every commit:
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`, and
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`.
- **B changes no behaviour.** No file under `tests-fixtures/*.expected`, no
  expectation in `crates/ns-cli/tests/program_outcomes.rs`, no row of
  `corpus/runnable.toml` and no `*.snap` file under `crates/*/tests/snapshots/`
  may change. A golden that changes means the task went too far: stop and find
  out why, never update the golden.
- **Allocation order is behaviour.** Every object allocation consumes an
  identity hash (`Heap::alloc_identity_hash`), which a program can print and an
  image records. No task may add, remove or reorder an object allocation.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an `EvalResult`.
  Tests unwrap through a local `expect_ok` helper — copy the one in a
  neighbouring test file.
- Object, frame and activation access goes through
  `crates/ns-runtime/src/heap.rs`.
- Never copy dispatch logic, lookup logic or error text out of `interpreter/`
  into `vm.rs` or `compiler/`.
- A wire tag is assigned once and never renumbered or reused
  (`crates/ns-runtime/src/image/opcodes.rs`, pinned by
  `crates/ns-runtime/tests/opcode_tags.rs`). No task here changes a tag.
- Never judge performance from a debug build.
- Design documents in `docs/design/` are living and describe the current state;
  this plan is an execution record — leave it as written once complete.

## Measurement protocol

Tasks 3, 4, 5, 6 and 7 touch a hot path and are measured with this protocol
**after** their commit, against the commit before it. Task 10 runs it once more
against the branch's merge-base.

```powershell
git worktree add ..\ns-next-before HEAD~1   # Task 10: `master` instead of HEAD~1
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
git worktree remove ..\ns-next-before
```

Record each `median` (18 numbers) in `target\b-measurements.md` under a heading
naming the task — `target\` is ignored by git, so the file survives between
tasks without being committed; Task 10 collects it. Close other heavy programs
first; a figure carried across sittings measures the machine, not the code.

**Decision rule:** a task's change is expected to make a source faster or leave
it unchanged. If any source's "after" medians are slower than its "before"
medians by more than the spread within either group, stop and investigate
before the next task. If a task that exists for speed moves nothing, say so in
`target\b-measurements.md` — that is a result, not a failure.

## Byte-identity protocol

Tasks 5 and 6 change what the image encoder and decoder read, and must not
change a byte they write. Run this with both binaries of that task's
measurement still built (before `git worktree remove`):

```powershell
$out = "target\b-identity"; New-Item -ItemType Directory -Force $out | Out-Null
$bad = 0; $compared = 0
foreach ($f in Get-ChildItem tests-fixtures\*.ns) {
    foreach ($bin in 'before', 'after') {
        $exe = if ($bin -eq 'before') { '..\ns-next-before\target\release\ns-next.exe' } else { '.\target\release\ns-next.exe' }
        Remove-Item "$out\$($f.BaseName).$bin.*" -ErrorAction SilentlyContinue
        & $exe build $f.FullName "$out\$($f.BaseName).$bin.nsb" *> $null
        & $exe snapshot $f.FullName "$out\$($f.BaseName).$bin.nsi" *> $null
    }
    foreach ($ext in 'nsb', 'nsi') {
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

Expected: `compared 44 artifacts, 0 differing` (every fixture declares
`main:args:`, so each builds and snapshots). Both commands are deterministic —
checked while planning: two runs of one binary over every fixture agree byte
for byte.

## File Structure

| File | Responsibility after this plan |
|---|---|
| `crates/ns-runtime/src/interpreter/mod.rs` | `Interpreter` (one constructor, `bootstrap`), `send`, `invoke_method`, `invoke_closure`, `raise_return`, `metaclass_of`, allocation helpers. Builtin classes in a `Vec` indexed by `BuiltinClass`. No code table. |
| `crates/ns-runtime/src/interpreter/sends.rs` (new) | `dispatch_lexical_send`, `dispatch_super_send` and their helpers `lexical_lookup`, `slot_selector_shape`. |
| `crates/ns-runtime/src/interpreter/locals.rs` (new) | Register and outer-local access (`read_local`, `write_local`, `read_outer_local`, `write_outer_local`), `home_of`, `store_instance_field`. |
| `crates/ns-runtime/src/interpreter/literals.rs` (new) | The errors a deferred literal raises and `parse_int_literal`. |
| `crates/ns-runtime/src/interpreter/eval.rs` | Deleted in Task 2. |
| `crates/ns-runtime/src/symbol.rs` | Interner seeded with the runtime's well-known symbols, each an associated constant (`Symbol::NEW`, ...). |
| `crates/ns-runtime/src/types.rs` | `ObjectData::Closure(Rc<ClosureData>)`; `ClosureData.params: Rc<Vec<Symbol>>`. |
| `crates/ns-runtime/src/mixin.rs` | `MixinMethod.code`, `Mixin.initializer_code`, `superclass_clause_code`, `superclass_message_code`. |
| `crates/ns-runtime/src/compiler/mod.rs` | `compile_method`, `compile_mixin_bodies`. No `compile_module`, no `compile_initializer`. |
| `crates/ns-runtime/src/bytecode.rs` | No `CompiledModule`, no `method_key`. |
| `crates/ns-runtime/src/image/` | Encoder reads code off the mixin tree; `mixins::decode` attaches code; `decode::verify_mixins` replaces `build_module`; `snapshot`/`build` take no module. |
| `clippy.toml` (new), `Cargo.toml` | `trivially_copy_pass_by_ref` enforced, sized so it covers every handle in a debug build. |
| `crates/ns-cli/src/main.rs` | `Cli`, `Cmd`, `main` only. |
| `crates/ns-cli/src/source.rs`, `entry.rs`, `commands/*.rs` (new) | Reading sources; the CLI's entry-point conventions; one module per command. |
| `CLAUDE.md`, `docs/design/*.md`, `docs/PROGRESS.md` | Describe the result; record the measurements and the two decisions. |

---

### Task 1: Tests pin exact output and lose what the retirement left behind

**Files:**
- Modify: `crates/ns-cli/tests/run_fixtures.rs`
- Modify: `crates/ns-cli/tests/cli.rs`
- Modify: `crates/ns-runtime/tests/vm.rs`
- Modify: `crates/ns-runtime/tests/vm_initializer.rs`
- Modify: `crates/ns-runtime/tests/image.rs`
- Modify: `crates/ns-runtime/tests/nested_classes.rs`

**Interfaces:** none; tests only.

- [ ] **Step 1: Compare every fixture's stdout byte for byte**

In `crates/ns-cli/tests/run_fixtures.rs`, replace the doc comment above
`every_tests_fixtures_ns_file_produces_its_expected_stdout` with:

```rust
/// Every fixture succeeds, prints exactly its `.expected` stdout -- every
/// byte, the final newline included -- and writes nothing to stderr.
```

and replace the stdout assertion

```rust
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            normalize_newlines(stdout.trim_end()),
            normalize_newlines(expected.trim_end()),
            "{} produced unexpected stdout",
            path.display()
        );
```

with

```rust
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            normalize_newlines(&stdout),
            normalize_newlines(&expected),
            "{} produced unexpected stdout",
            path.display()
        );
```

- [ ] **Step 2: Check that it bites, then that it passes**

Temporarily remove the last byte (the final `\n`) of
`tests-fixtures/hello.expected`, then run
`cargo test -p ns-cli --test run_fixtures`.
Expected: FAIL, "hello.ns produced unexpected stdout".
Restore it with `git checkout -- tests-fixtures/hello.expected` and run again.
Expected: PASS. (Every fixture's output matched its `.expected` exactly when this
plan was written, so no `.expected` changes.)

- [ ] **Step 3: `cli.rs` stops testing the absence of a retired flag**

In `crates/ns-cli/tests/cli.rs`, delete the whole test
`run_no_longer_accepts_an_engine` (clap already refuses an undeclared flag),
and in the `bench` test just above it delete the line
`assert!(!stdout.contains("tree-walk"), "{stdout}");`.

- [ ] **Step 4: `vm.rs` and `vm_initializer.rs`**

In `crates/ns-runtime/tests/vm.rs`:
- the module doc becomes
  ```rust
  //! Bytecode execution, every send driven through `Interpreter::send`.
  ```
- `expect_ok`'s doc becomes
  ```rust
  /// `Signal` carries no `Debug` impl, so `Result::expect` is unusable on an
  /// `EvalResult` -- unwrap by hand.
  ```
- rename `a_dynamic_array_literal_evaluates_its_elements_under_bytecode` to
  `a_dynamic_array_literal_evaluates_its_elements`;
- in `a_malformed_integer_literal_raises_an_out_of_range_error_when_evaluated`,
  after the existing `assert!`, add
  ```rust
      assert_eq!(err.selector, Symbol::intern("parse"));
  ```

In `crates/ns-runtime/tests/vm_initializer.rs`, rename
`a_slot_initializer_reads_a_factory_parameter_under_bytecode` to
`a_slot_initializer_reads_a_factory_parameter` and
`init_exprs_run_after_slot_initializers_under_bytecode` to
`init_exprs_run_after_slot_initializers`.

- [ ] **Step 5: `image.rs` — dead code and ragged comments**

In `the_graph_walk_consumes_the_section_exactly`, replace

```rust
        let instance = {
            let module = interp.compiled_module();
            let _ = module;
            let args = vec![platform];
            interp.instantiate(class, args)
        };
```

with

```rust
        let instance = interp.instantiate(class, vec![platform]);
```

Then reflow, to the file's 80-column comment width, the comment paragraph
containing each of these lines (find them by their text; each is a line broken
early or run long by an earlier mechanical edit):

- `// but sizes the register file from the *code*`
- `// \`load\` hands the maximum decoded hash to`
- `// code table carries. A reloaded image whose`
- `/// body can come from -- exactly the reason the`
- `/// class tag) but not its tag, which is what lets`
- `/// A full \`image::snapshot\` of \`src\`, with no roots -- these tests care about the`
- `/// Instantiates \`src\` and snapshots the instance`
- `/// flag says. \`raise_return\` decides a home's liveness by`
- `/// (\`count ::= 0\`) and a statement in the initializer body`
- `/// user-defined inheritance, and a paused/suspended closure. Each of those`

Only whitespace and line breaks change in those paragraphs.

- [ ] **Step 6: `nested_classes.rs` — fold the one-caller helpers**

Each of these free functions is called from exactly one `#[test]`, which does
nothing but call it:
`accessor_is_stable`, `distinct_per_receiver`, `enclosing_object_is_the_receiver`,
`instantiating_the_nested_class`, `non_class_superclass_clause`,
`protected_nested_access`, `inherited_nested_declaration`, `cyclic_definition`,
`class_side_dispatch`, `class_side_miss_is_ordinary_dnu`, `class_side_is_inherited`,
`an_inherited_class_side_shares_one_application`,
`class_side_override_shadows_the_inherited_method`, `nested_super_and_outer`,
`super_chain_progresses_past_an_inherited_middle_level`,
`outer_resolves_from_the_defining_level`,
`a_two_level_outer_resolves_from_the_defining_level`,
`class_side_outer_reaches_the_enclosing_object`.

For each: confirm with `git grep -n "<name>()" crates/ns-runtime/tests/nested_classes.rs`
that it has exactly one call. Then move its body into the test that calls it
(the test keeps its own name), move any doc comment the helper had onto the
test (above `#[test]`), and delete the helper. If a test does anything besides
the call, keep that code in place around the moved body. `nested_class_object`
and `application_of` take parameters and stay.

- [ ] **Step 7: Run everything**

Run: `cargo test --workspace`
Expected: PASS, with one test fewer in `cli.rs` than before.

- [ ] **Step 8: Gates and commit**

Run `cargo fmt --all`, `cargo clippy --workspace --all-targets`, both corpus
gates.

```bash
git add crates
git commit -m "test: pin each fixture's stdout byte for byte and drop what the retirement left

run_fixtures compared trimmed output, so a lost or an extra final newline
passed; it now compares every byte after newline normalization. The engine
flag test and the tree-walk assertion tested the absence of a retired
feature, the _under_bytecode suffixes named a distinction that no longer
exists, and nested_classes.rs's one-caller helpers fold into their tests.
The malformed-integer test pins its error's selector again.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 2: `interpreter/` without the tree-walker's leftovers

`eval.rs` evaluates nothing any more. It holds three unrelated things, so it
splits by responsibility rather than being renamed; `invoke_closure` joins
`invoke_method`, which it mirrors; the `raise_return_pub` wrapper and the
never-bootstrapped `Interpreter::new` go.

**Files:**
- Create: `crates/ns-runtime/src/interpreter/sends.rs`, `locals.rs`, `literals.rs`
- Delete: `crates/ns-runtime/src/interpreter/eval.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`, `instantiate.rs`, `nested.rs`
- Modify: `crates/ns-runtime/src/vm.rs`, `builtins/mod.rs`, `compiler/lower.rs`
- Modify (comments only): `crates/ns-runtime/src/bytecode.rs`, `dispatch.rs`, `heap.rs`, `image/decode.rs`, `image/opcodes.rs`, `crates/ns-runtime/tests/image.rs`, `tests/instantiate.rs`, `tests/vm.rs`

**Interfaces:**
- Produces:
  - `crate::interpreter::sends::{dispatch_lexical_send, dispatch_super_send}` (`pub(crate)`, signatures unchanged)
  - `crate::interpreter::locals::{read_local, write_local, read_outer_local, write_outer_local, home_of, store_instance_field}` (`pub(crate)`, unchanged)
  - `crate::interpreter::literals::{unresolved_identifier_error, malformed_int_error, parse_int_literal}` (`pub(crate)`, unchanged)
  - `Interpreter::invoke_closure(&mut self, closure: &Rc<ClosureData>, args: Vec<Value>) -> EvalResult` (`pub`)
  - `Interpreter::raise_return(&mut self, act: &ActivationRef, value: Value) -> Signal` (`pub(crate)`)
  - `Interpreter::bootstrap()` is the only constructor; `Interpreter::new` and `impl Default for Interpreter` are gone.

- [ ] **Step 1: Split `eval.rs`**

Create the three files and move code into them **verbatim** (bodies and doc
comments unchanged except where this step says otherwise):

`crates/ns-runtime/src/interpreter/literals.rs` moves
`unresolved_identifier_error`, `malformed_int_error`, `parse_int_literal`, and
the whole `#[cfg(test)] mod tests` from `eval.rs`, under:

```rust
//! The errors a deferred literal raises when the VM executes it, and the
//! integer-literal parser both the compiler and the VM use.

use crate::types::*;
use crate::Symbol;

use super::Signal;
```

`crates/ns-runtime/src/interpreter/locals.rs` moves `read_local`,
`write_local`, `enum OuterFrame`, `frame_at_depth`, `read_outer_local`,
`write_outer_local`, `home_of`, `store_instance_field`, under:

```rust
//! Register and outer-local access for a running activation, and the two
//! helpers `MakeClosure` and `InitSlot` write through.

use crate::heap::{ActivationRef, Heap};
use crate::types::*;
```

`crates/ns-runtime/src/interpreter/sends.rs` moves `slot_selector_shape`,
`lexical_lookup`, `dispatch_lexical_send`, `dispatch_super_send`, under:

```rust
//! Implicit-receiver and `super` sends: `newspeak-spec.pdf` §5.7-§5.10.
//! The VM's `SendLexical` and `SendSuper` call these; an ordinary send is
//! `Interpreter::send`.

use ns_syntax::ast;

use crate::dispatch::{self, Lookup};
use crate::heap::ActivationRef;
use crate::types::*;
use crate::Symbol;

use super::{EvalResult, Interpreter, Signal};
```

Inside it, `crate::interpreter::nested::nested_class` stays as written. Remove
any import the compiler reports unused.

Then delete `eval.rs` (`git rm crates/ns-runtime/src/interpreter/eval.rs`).

- [ ] **Step 2: `invoke_closure` becomes a method**

In `crates/ns-runtime/src/interpreter/mod.rs`, directly after `invoke_method`,
add `eval.rs`'s `invoke_closure` as a method, with `interp` replaced by `self`:

```rust
    /// Invokes a closure with `args`. Called by `builtins::block` for `value`,
    /// `value:`, `value:value:` and so on, and by every native that runs a
    /// block argument, through `builtins::call_block`.
    ///
    /// The invoking activation is not passed: a live activation's sender is
    /// the one below it on the activation stack.
    pub fn invoke_closure(&mut self, closure: &Rc<ClosureData>, args: Vec<Value>) -> EvalResult {
        // A wrong argument count is refused before it can reach a register: the
        // window is laid out from the arguments actually given, so too few would
        // address registers past its end -- a panic -- and too many would
        // silently shift every temporary. Never panic on this
        // Newspeak-program-input path.
        if args.len() != closure.params.len() {
            let receiver = self.heap.frame_receiver(&closure.captured_frame);
            return Err(Signal::Error(NewspeakError {
                selector: Symbol::intern("value"),
                receiver,
                message: format!(
                    "wrong number of block arguments: block takes {}, got {}",
                    closure.params.len(),
                    args.len()
                ),
            }));
        }
        let receiver = self.heap.frame_receiver(&closure.captured_frame);
        let current_class = self.heap.frame_current_class(&closure.captured_frame);
        let code = &closure.code;
        let act = self.heap.push_activation(
            ActivationHeader {
                receiver,
                current_class,
                kind: FrameKind::Closure(closure.clone()),
                slot_base: 0, // a closure body never contains InitSlot
            },
            &args,
            (code.num_registers - code.num_args) as usize,
        );
        let result = crate::vm::run_code(self, &act, code);
        // Popped on every path out. A closure activation catches no return: its
        // `^` targets its home.
        self.heap.pop_activation(&act);
        result
    }
```

In `crates/ns-runtime/src/builtins/mod.rs`, `call_block`'s call becomes
`interp.invoke_closure(&closure, args)`, and its doc's
"Goes straight through `interpreter::eval::invoke_closure`" becomes "Goes
straight through `Interpreter::invoke_closure`".

- [ ] **Step 3: Module declarations and imports**

In `interpreter/mod.rs`: replace `pub mod eval;` with
`pub(crate) mod literals;`, `pub(crate) mod locals;`, `pub(crate) mod sends;`
(keep `pub mod instantiate;` and `pub(crate) mod nested;`), and replace the
module doc's second sentence with
"Implicit-receiver and `super` sends are in `sends.rs`, register and
outer-local access in `locals.rs`, the errors a deferred literal raises in
`literals.rs`; the bootstrap and class instantiation are in `instantiate.rs`."

`vm.rs`'s `use crate::interpreter::eval::{...};` becomes

```rust
use crate::interpreter::literals::{
    malformed_int_error, parse_int_literal, unresolved_identifier_error,
};
use crate::interpreter::locals::{
    home_of, read_local, read_outer_local, store_instance_field, write_local, write_outer_local,
};
use crate::interpreter::sends::{dispatch_lexical_send, dispatch_super_send};
```

`compiler/lower.rs`: `use crate::interpreter::eval::parse_int_literal;` becomes
`use crate::interpreter::literals::parse_int_literal;`.
`interpreter/nested.rs`: `crate::interpreter::eval::store_instance_field(` becomes
`crate::interpreter::locals::store_instance_field(`.

- [ ] **Step 4: `raise_return` loses its wrapper**

In `interpreter/mod.rs`, make `raise_return` `pub(crate)` and delete
`raise_return_pub`. In `vm.rs`, `interp.raise_return_pub(act, v)` becomes
`interp.raise_return(act, v)`.

- [ ] **Step 5: One constructor**

In `interpreter/mod.rs`, delete `impl Default for Interpreter` and
`pub fn new()`. `bootstrap` keeps its struct literal unchanged. (Nothing in the
workspace calls `Interpreter::new` or `Interpreter::default`; check with
`git grep -n "Interpreter::new()\|Interpreter::default()" crates` — expected:
no hits.)

- [ ] **Step 6: `invoke_method`'s arity comment**

Replace the comment block at the top of `invoke_method` (from
`// An activation's locals are addressed as one run of registers --` through
`// -- never a panic, since the caller may be a host.`) with:

```rust
        // An activation's locals are addressed as one run of registers --
        // the arguments first, the declared temporaries after them -- and
        // the run is laid out from how many arguments the activation was
        // *given*, not how many the method declares. A wrong-arity
        // activation therefore does not fail: it shifts every temporary by
        // the difference and reads an argument back where a temporary
        // should be, which no golden output necessarily shows. Silent wrong
        // output with exit status 0 is the failure mode this repository's
        // invariants single out, so the mismatch is refused here as an
        // ordinary Newspeak error -- never a panic, since the caller may be
        // a host.
```

The paragraph after it ("No Newspeak-level send can reach this ...") stays.

- [ ] **Step 7: Every comment that names `eval`**

Run `git grep -n "eval::\|eval\.rs\|\`eval\`" crates`. Rewrite each hit to name
the new home: `eval::read_local`/`write_local`/`frame_at_depth` →
`locals::read_local`/`locals::write_local`/`locals::frame_at_depth`;
`eval.rs::dispatch_lexical_send` → `sends::dispatch_lexical_send`;
`interpreter::eval::parse_int_literal` / `malformed_int_error` →
`interpreter::literals::...`; "`eval.rs`'s own constructors" (`vm.rs`) →
"`literals.rs`'s own constructors"; "the VM raises `eval`'s own error"
(`lower.rs`) → "the VM raises `literals`' own error"; `heap.rs`'s module doc
"most of `interpreter/eval.rs`" → "most of `interpreter/locals.rs`".
Expected afterwards: `git grep -n "eval::\|eval\.rs" crates` is empty.

- [ ] **Step 8: Build, test, gates, commit**

Run `cargo build --workspace`, `cargo test --workspace` (expected: PASS), then
`cargo fmt --all`, `cargo clippy --workspace --all-targets`, both corpus gates.

```bash
git add -A crates
git commit -m "refactor: split interpreter/eval.rs by what it holds

eval.rs evaluated nothing since the tree-walker went. Its sends move to
sends.rs, register and outer-local access to locals.rs, and the deferred
literal errors to literals.rs; invoke_closure becomes a method beside
invoke_method, which it mirrors. raise_return_pub, a wrapper with no
reason left, and Interpreter::new, which nothing called, are gone.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 3: Builtin classes by index, not by hash

`Interpreter::builtin_class` is on the hot path: `class_of` calls it for every
send to a `SmallInt`, `Bool`, `Nil` or `Float` receiver, `LoadBuiltinClass`
calls it, and so does every string, symbol and array allocation. Today it is an
`Option` check and a SipHash `HashMap` lookup. `BuiltinClass` is a fieldless
enum, so its discriminant indexes a `Vec`.

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`

**Interfaces:**
- Consumes: Task 2's single constructor.
- Produces: `Interpreter::builtin_class(&self, kind: BuiltinClass) -> ClassRef`, unchanged signature.

- [ ] **Step 1: The field**

Replace

```rust
    builtin_classes: Option<HashMap<BuiltinClass, ClassRef>>,
```

with

```rust
    /// Every builtin class object, indexed by `BuiltinClass as usize`.
    /// Indexed rather than hashed because `class_of` asks for one on every
    /// send to a primitive receiver. Empty only while `bootstrap` is
    /// building it; nothing can reach an `Interpreter` in that state.
    builtin_classes: Vec<ClassRef>,
```

In `bootstrap`, `builtin_classes: None,` becomes `builtin_classes: Vec::new(),`
and `interp.builtin_classes = Some(build_builtin_classes(&mut interp));` becomes
`interp.builtin_classes = build_builtin_classes(&mut interp);`.

- [ ] **Step 2: The accessor**

```rust
    pub fn builtin_class(&self, kind: BuiltinClass) -> ClassRef {
        self.builtin_classes[kind as usize]
    }
```

- [ ] **Step 3: `build_builtin_classes` returns the vector**

Change its return type to `Vec<ClassRef>`. Keep the `kinds` array (it is in
declaration order, which is discriminant order) and keep **every allocation in
the same order** — including the class object the loop allocates for
`BuiltinClass::Object`, which is then not used (see Scope: removing it would
shift identity hashes). Replace `let mut map = HashMap::new();` with
`let mut classes = Vec::with_capacity(kinds.len());`, replace the loop's
`map.insert(kind, obj);` with

```rust
        // `builtin_class` indexes by discriminant, so the vector must be
        // filled in exactly that order.
        debug_assert_eq!(classes.len(), kind as usize);
        // `Object`'s entry is the self-referential class every builtin has as
        // its `.class`; the object this iteration allocated for it is never
        // used, and is kept only because removing it would shift every later
        // identity hash.
        classes.push(if kind == BuiltinClass::Object {
            class_of_class
        } else {
            obj
        });
```

and replace the trailing `map.insert(BuiltinClass::Object, class_of_class);`
and `map` with `classes`. Remove the `HashMap` import only if nothing else in
the file uses it (`applications` and `empty_class_sides` do).

- [ ] **Step 4: Test, gates, commit**

Run `cargo test --workspace` (expected: PASS — no allocation moved, so every
golden and identity hash is unchanged), `cargo fmt --all`,
`cargo clippy --workspace --all-targets`, both corpus gates.

```bash
git add crates/ns-runtime/src/interpreter/mod.rs
git commit -m "perf: index builtin classes by discriminant instead of hashing them

class_of asks for a builtin class on every send to a primitive receiver,
and each ask was an Option check and a SipHash lookup.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

- [ ] **Step 5: Measure**

Run the measurement protocol; record under "Task 3". Expected: `bench_loop.ns`
and `bench_send.ns` (whose loops send `<` and `+` to integers) faster or
unchanged.

---

### Task 4: The runtime's own symbols, interned once

Every fixed symbol the runtime names in its own code is interned at the call
site today, which is a thread-local borrow and a hash per call. Both reference
systems keep such symbols ready: Primordial Soup as `ObjectStore` fields
(`does_not_understand()`, `cannot_return()`, `non_boolean_receiver()`), Nova as
`BUILTIN_STRING_MEMORY`, strings pre-allocated at fixed indices so each is a
constant. `Symbol` is already an index into the interner, so seeding the
interner with a fixed list makes each of those symbols a `const`.

**Files:**
- Modify: `crates/ns-runtime/src/symbol.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`, `interpreter/literals.rs`, `interpreter/instantiate.rs`, `builtins/block.rs`, `builtins/object_class.rs`, `compiler/lower.rs`, `resolver/mod.rs`
- Modify: `crates/ns-cli/src/main.rs`

**Interfaces:**
- Produces: associated constants on `Symbol`: `NEW`, `VALUE`, `MUST_BE_BOOLEAN`,
  `DOES_NOT_UNDERSTAND`, `CARET`, `PARSE`, `OBJECT`, `USING_PLATFORM`,
  `CLASS_SIDE`, `AN_OBJECT`, `UNKNOWN`. Each equals `Symbol::intern` of its text
  on every thread.

- [ ] **Step 1: Write the failing test**

Add to `symbol.rs`'s `mod tests`:

```rust
    /// Every well-known symbol is the one interning its text answers, on
    /// every thread -- the property that lets the runtime name a symbol as a
    /// constant instead of interning it where it is used.
    #[test]
    fn every_well_known_symbol_is_what_interning_its_text_answers() {
        for (symbol, text) in super::WELL_KNOWN_SYMBOLS.iter().zip(super::WELL_KNOWN) {
            assert_eq!(symbol.as_str(), *text);
            assert_eq!(Symbol::intern(text), *symbol);
        }
        assert_eq!(super::WELL_KNOWN_SYMBOLS.len(), super::WELL_KNOWN.len());
        let on_another_thread = std::thread::spawn(|| Symbol::intern("doesNotUnderstand:"))
            .join()
            .expect("thread");
        assert_eq!(on_another_thread, Symbol::DOES_NOT_UNDERSTAND);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --lib every_well_known_symbol`
Expected: FAIL to compile — `WELL_KNOWN_SYMBOLS`, `WELL_KNOWN` and
`Symbol::DOES_NOT_UNDERSTAND` do not exist.

- [ ] **Step 3: Implement**

In `symbol.rs` (its module doc is Task 9's), add, after `pub struct Symbol(u32);`:

```rust
/// Declares the symbols the runtime names in its own code, each as an
/// associated constant on `Symbol`, and the table `Interner::new` seeds them
/// from. One list, so a constant and its text cannot drift apart: the enum
/// numbers the entries in declaration order and the table lists the texts in
/// the same order.
macro_rules! well_known_symbols {
    ($($name:ident => $text:literal,)*) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        #[repr(u32)]
        enum WellKnown {
            $($name,)*
        }

        impl Symbol {
            $(pub const $name: Symbol = Symbol(WellKnown::$name as u32);)*
        }

        /// Interned first, in this order, by every thread's interner.
        const WELL_KNOWN: &[&str] = &[$($text,)*];

        #[cfg(test)]
        const WELL_KNOWN_SYMBOLS: &[Symbol] = &[$(Symbol::$name,)*];
    };
}

well_known_symbols! {
    NEW => "new",
    VALUE => "value",
    MUST_BE_BOOLEAN => "mustBeBoolean",
    DOES_NOT_UNDERSTAND => "doesNotUnderstand:",
    CARET => "^",
    PARSE => "parse",
    OBJECT => "Object",
    USING_PLATFORM => "usingPlatform:",
    CLASS_SIDE => "class side",
    AN_OBJECT => "an object",
    UNKNOWN => "unknown",
}
```

and replace `Interner::new` and `Symbol::intern`'s body with:

```rust
impl Interner {
    /// Seeded with `WELL_KNOWN` before anything else is interned, so each
    /// well-known symbol's index is its constant's on every thread.
    fn new() -> Self {
        let mut interner = Interner {
            strings: Vec::new(),
            lookup: HashMap::new(),
        };
        for text in WELL_KNOWN {
            interner.insert(text);
        }
        interner
    }

    fn insert(&mut self, text: &'static str) -> Symbol {
        let sym = Symbol(self.strings.len() as u32);
        self.strings.push(text);
        self.lookup.insert(text, sym);
        sym
    }
}
```

```rust
    pub fn intern(s: &str) -> Symbol {
        INTERNER.with_borrow_mut(|interner| {
            if let Some(&sym) = interner.lookup.get(s) {
                return sym;
            }
            interner.insert(Box::leak(s.to_string().into_boxed_str()))
        })
    }
```

(`WELL_KNOWN` must not contain a text twice: `insert` does not deduplicate, so
a duplicate would give two indices one text and `Symbol::intern` would answer
the later one — which the test in Step 1 catches for the earlier constant.)

- [ ] **Step 4: Use the constants**

Replace each literal intern with its constant:

| File | Site | Becomes |
|---|---|---|
| `interpreter/mod.rs` | `Symbol::intern("doesNotUnderstand:")` | `Symbol::DOES_NOT_UNDERSTAND` |
| `interpreter/mod.rs` | `Symbol::intern("^")` | `Symbol::CARET` |
| `interpreter/mod.rs` | `invoke_closure`'s `Symbol::intern("value")` | `Symbol::VALUE` |
| `interpreter/literals.rs` | `Symbol::intern("parse")` | `Symbol::PARSE` |
| `interpreter/instantiate.rs` | every `Symbol::intern("new")` (six) | `Symbol::NEW` |
| `interpreter/instantiate.rs` | `Symbol::intern("class side")` | `Symbol::CLASS_SIDE` |
| `interpreter/instantiate.rs` | `Symbol::intern("an object")` | `Symbol::AN_OBJECT` |
| `builtins/block.rs` | `Symbol::intern("mustBeBoolean")` | `Symbol::MUST_BE_BOOLEAN` |
| `builtins/object_class.rs` | `Symbol::intern("unknown")` | `Symbol::UNKNOWN` |
| `compiler/lower.rs` | `crate::Symbol::intern("mustBeBoolean")` | `crate::Symbol::MUST_BE_BOOLEAN` |
| `resolver/mod.rs` | `Symbol::intern("Object")` | `Symbol::OBJECT` |
| `ns-cli/src/main.rs` | `ns_runtime::Symbol::intern("usingPlatform:")` | `ns_runtime::Symbol::USING_PLATFORM` |

Where a closure form wrapped the intern (`|| Symbol::intern("new")`), use the
constant directly: `clause.map_or(Symbol::NEW, |clause| clause.selector)` and
`superclass_clause_message(clause).map_or(Symbol::NEW, crate::compiler::lower::selector_of)`.
Selector interning inside `builtins/numbers.rs`, `text.rs` and the other native
modules stays (decision 2).
Expected afterwards: `git grep -n -E 'intern\("' crates/ns-runtime/src crates/ns-cli/src`
lists only `builtins/numbers.rs`'s `"//"` and `#[cfg(test)]` code.

- [ ] **Step 5: Test, gates, commit**

Run `cargo test --workspace` (expected: PASS), `cargo fmt --all`,
`cargo clippy --workspace --all-targets`, both corpus gates.

```bash
git add crates
git commit -m "perf: intern the runtime's own symbols once, as constants

The interner is seeded with a fixed list, so each symbol the runtime names
in its own code -- doesNotUnderstand:, mustBeBoolean, new and the rest --
is an associated constant on Symbol instead of a thread-local borrow and a
hash at every use. Native dispatch still matches selector text; binding
natives as enumerated primitives belongs to the inline caches.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

- [ ] **Step 6: Measure**

Run the measurement protocol; record under "Task 4". Expected: unchanged or
slightly faster (`try_factory_send` no longer interns `new` for every send to a
builtin class object).

---

### Task 5: A closure is shared, not copied

Every block creation deep-copies a `ClosureData`, and every `value:` copies it
out of the heap twice and re-wraps it in a fresh `Rc`. `MakeClosure` builds an
`Rc<ClosureData>` (cloning the instruction's `params` vector), then
`new_closure` deep-clones it into `ObjectData::Closure`; `builtins/block.rs`
copies it out to check the receiver is a closure, and `call_block` copies it out
again and allocates a new `Rc`. Storing the `Rc` makes all of that
reference-count bumps.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs`, `heap.rs`, `vm.rs`, `interpreter/mod.rs`, `builtins/mod.rs`, `builtins/block.rs`
- Modify: `crates/ns-runtime/src/image/decode.rs`, `image/encode.rs`
- Test: `crates/ns-runtime/tests/image.rs` (the one site that builds a `ClosureData`), `heap.rs`'s own tests

**Interfaces:**
- Produces:
  - `ObjectData::Closure(Rc<ClosureData>)`
  - `ClosureData { params: Rc<Vec<Symbol>>, code, captured_frame, home }`
  - `Heap::closure_data(&self, obj: &ObjRef) -> Option<Rc<ClosureData>>`
  - `Interpreter::new_closure(&mut self, closure: Rc<ClosureData>) -> ObjRef` (stores it as given)
  - `encode_closure_record` and `encode_frame` return `()`.

- [ ] **Step 1: The types**

`types.rs`: `Closure(ClosureData),` becomes `Closure(Rc<ClosureData>),` and
`pub params: Vec<Symbol>,` becomes

```rust
    /// Shared with the `MakeClosure` instruction that created the closure --
    /// the names are the same for every closure one instruction makes.
    pub params: Rc<Vec<Symbol>>,
```

- [ ] **Step 2: Creation**

`vm.rs`, `MakeClosure`: `params: (**params).clone(),` becomes
`params: params.clone(),`.
`interpreter/mod.rs`, `new_closure`: the body becomes

```rust
        let class = self.builtin_class(BuiltinClass::Block);
        self.new_instance_object(class, ObjectData::Closure(closure))
```

- [ ] **Step 3: Access**

`heap.rs`, `closure_data`: return `Option<Rc<ClosureData>>`; the arm stays
`ObjectData::Closure(c) => Some(c.clone()),` (now a reference-count bump). Add
one line to its doc: `/// An \`Rc\` bump, not a copy: a closure is immutable once made.`
`builtins/mod.rs`, `call_block`: the `Some(closure_data)` arm becomes

```rust
                Some(closure) => interp.invoke_closure(&closure, args),
```

`builtins/block.rs`: unchanged in shape — `closure.params.len()` reads through
the `Rc`.

- [ ] **Step 4: The image**

`decode.rs`, `decode_closure_record`: build `params: Rc::new(params),`; its
caller for `OBJ_CLOSURE` becomes
`ObjectData::Closure(Rc::new(decode_closure_record(r, ctx, &tables, symbols)?))`;
the frame-kind caller already wraps its result in `Rc` — check it still compiles
and leave it. `encode.rs`: `encode_closure_record` iterates `closure.params.iter()`;
it and `encode_frame` can no longer fail for any reason, so both return `()`:
drop their `Result<(), String>` return types and trailing `Ok(())`, and drop the
`?` where `encode_object` and `encode_graph_section` call them. `encode_object`
stays fallible (`encode_class_record` can refuse).

- [ ] **Step 5: Tests that build a closure by hand**

`crates/ns-runtime/tests/image.rs`, the test that writes
`ObjectData::Closure(ns_runtime::ClosureData { ... })`: wrap it as
`ObjectData::Closure(Rc::new(ns_runtime::ClosureData { params: Rc::new(<the vec it had>), ... }))`.
`heap.rs`'s test module (the `ClosureData { ... }` near its end): `params:
Rc::new(<the vec it had>)`. Build with `cargo build --workspace --all-targets`
and fix what the compiler reports by the same two rules.

- [ ] **Step 6: Test, gates, commit**

Run `cargo test --workspace` (expected: PASS), `cargo fmt --all`,
`cargo clippy --workspace --all-targets`, both corpus gates.

```bash
git add crates
git commit -m "perf: store a closure behind an Rc instead of copying it per use

Creating a block deep-copied its ClosureData twice and every value: copied
it out of the heap twice and allocated a new Rc. The object now holds the
Rc and the parameter names are shared with the instruction that made it.
encode_closure_record and encode_frame lose a Result they could never fail.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

- [ ] **Step 7: Measure, check bytes, measure sizes**

Run the measurement protocol; record under "Task 5". Expected: `bench_block.ns`
faster. Before removing the worktree, run the byte-identity protocol.
Expected: `differing artifacts: 0`.

Then add this temporary test at the end of `heap.rs`'s `mod tests`:

```rust
    #[test]
    fn print_sizes() {
        eprintln!(
            "Object {} Frame {} FrameSlot {}",
            std::mem::size_of::<Object>(),
            std::mem::size_of::<Frame>(),
            std::mem::size_of::<FrameSlot>()
        );
    }
```

run `cargo test -p ns-runtime --lib print_sizes -- --nocapture` and
`cargo test -p ns-runtime --lib --release print_sizes -- --nocapture`, record the
four figures in `target\b-measurements.md` under "Task 5 sizes", and delete the
test. Do not commit it. (Before this task, `Object` was 56 bytes in release and
80 in debug; Task 10 recomputes the image-decoding amplification from whatever
these are now.)

---

### Task 6: Compiled code lives on the declaration

The design change of decision 1. Code moves from the interpreter's
`CompiledModule` — four `HashMap`s keyed by the address of an `Rc<MixinMethod>`
or `Rc<Mixin>` — onto the declarations themselves: a `MixinMethod` carries its
body, a `Mixin` its instance initializer and its superclass clause's two
bodies. A method is compiled as it is built from source and handed its body as
it is decoded from an image, so a method without a body cannot exist.
`CompiledModule`, `method_key`, `compile_module`, `compile_initializer`,
`Interpreter::install_code` and `compiled_module()` go.

A mixin is shared and immutable once built (`liveness-and-reflection.md`, "The
obstacle: `Mixin` has no interior mutability"), so the bodies are compiled into
the `Mixin` value before it is wrapped in its `Rc`. That needs nothing the
compiler did not already have: lowering reads the declaration and the
resolver's bindings, never the heap.

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs`, `compiler/mod.rs`, `bytecode.rs`
- Modify: `crates/ns-runtime/src/interpreter/mod.rs`, `instantiate.rs`, `nested.rs`
- Modify: `crates/ns-runtime/src/image/mod.rs`, `encode.rs`, `mixins.rs`, `decode.rs`
- Modify: `crates/ns-runtime/src/dispatch.rs`, `heap.rs` (tests and one doc comment)
- Modify: `crates/ns-cli/src/main.rs`
- Test: `crates/ns-runtime/tests/compiler.rs`, `tests/image.rs`
- Docs: `CLAUDE.md`, `docs/design/compiler.md`, `execution.md`, `dispatch.md`, `memory.md`, `image-format.md`, `object-model.md`

**Interfaces:**
- Produces:
  - `MixinMethod { selector, access, num_args, decl, code: Rc<CompiledCode> }`, `#[derive(Debug, Clone)]`
  - `MixinMethod::from_decl(decl: Rc<ast::MethodDecl>, bindings: &Rc<HashMap<Span, Binding>>) -> MixinMethod`
  - `Mixin { ..., initializer_code: Option<Rc<CompiledCode>>, superclass_clause_code: Option<Rc<CompiledCode>>, superclass_message_code: Option<Rc<CompiledCode>> }`, `#[derive(Debug, Clone)]`; `MixinSlot` gains `Clone`
  - `compiler::compile_method(method: &ast::MethodDecl, bindings) -> CompiledCode` (unchanged, `pub`)
  - `compiler::compile_mixin_bodies(mixin: &mut Mixin, bindings: &Rc<HashMap<Span, Binding>>)` (`pub(crate)`)
  - `image::snapshot(heap, source, origin, root_class, platform, roots)` and `image::build(heap, source, origin, root_class, entry)` — no `module`
  - `encode::encode_code_table(w, heap, root_class, symbols) -> Result<CodeIds, String>`
  - `mixins::plan(heap, root_class) -> Result<Vec<Row>, String>`, `Row { mixin, nested, class_side, methods: Vec<Rc<MixinMethod>> }`
  - `mixins::encode(w, heap, root_class, symbols, codes) -> Result<MixinIds, String>`
  - `mixins::decode(r, symbols, codes)` — unchanged signature; the mixins it returns carry their code
  - `decode::verify_mixins(mixins: &mixins::Decoded) -> Result<(), DecodeError>` (replaces `build_module`)
  - `decode::verify_graph(graph, table, heap)` — no `module`
  - Gone: `CompiledModule`, `bytecode::method_key`, `compiler::compile_module`, `compiler::compile_initializer`, `Interpreter::install_code`, `Interpreter::compiled_module`, `decode::build_module`.

- [ ] **Step 1: The declarations carry code**

`mixin.rs`: add `use crate::bytecode::CompiledCode; use crate::types::Binding;`.
Replace `MixinMethod`'s doc, struct and `from_decl` with:

```rust
/// One entry of a mixin's method dictionary, with the body it runs.
///
/// The body lives here, on the entry dispatch hands back, rather than in a
/// table beside the mixin: a send that found the method has found its code.
/// That is how every system in `prior-art/` holds it -- Primordial Soup's
/// `Method::bytecode()`, SOMns's `SInvokable.invokable` -- and it means a
/// method cannot exist without a body: one built from source is compiled as
/// it is built, one decoded from an image is handed the body its row names.
#[derive(Debug, Clone)]
pub struct MixinMethod {
    pub selector: Symbol,
    /// §5.2 accessibility, already defaulted through
    /// `dispatch::effective_access`. Read by every lookup, so it is stored
    /// rather than re-derived from `decl`, which a decoded mixin has none of.
    pub access: ast::AccessModifier,
    /// The selector's parameter count -- what a method activation's `args`
    /// are checked against, and what a decoded body's `num_args` is
    /// reconciled against on load.
    pub num_args: usize,
    /// The declaration this entry was built from. `None` for a mixin decoded
    /// from an image.
    pub decl: Option<Rc<ast::MethodDecl>>,
    /// The compiled body, shared by every application of the mixin
    /// (`newspeak-spec.pdf` §6.2). Never replaced: editing a method builds a
    /// new `MixinMethod`.
    pub code: Rc<CompiledCode>,
}

impl MixinMethod {
    /// The entry a parsed declaration becomes, compiled against the
    /// resolver's `bindings` for the module it belongs to.
    pub fn from_decl(
        decl: Rc<ast::MethodDecl>,
        bindings: &Rc<HashMap<Span, Binding>>,
    ) -> MixinMethod {
        MixinMethod {
            selector: Symbol::intern(&crate::interpreter::instantiate::method_name(&decl.pattern)),
            access: crate::dispatch::effective_access(decl.access),
            num_args: crate::interpreter::instantiate::pattern_param_count(&decl.pattern),
            code: Rc::new(crate::compiler::compile_method(&decl, bindings)),
            decl: Some(decl),
        }
    }
}
```

`MixinSlot` gets `#[derive(Debug, Clone)]`. `Mixin` gets
`#[derive(Debug, Clone)]` and, after `decl`, three fields:

```rust
    /// The instance initializer: this mixin's own slot initializers, then its
    /// own `initExpr`s. Every mixin built from source has one, a class side
    /// included. `None` for a builtin's mixin and `metaclass_of`'s
    /// placeholder, which run no code, and for a decoded row that names none
    /// -- `image::decode::verify_mixins` refuses that when the mixin needs one.
    pub initializer_code: Option<Rc<CompiledCode>>,
    /// The superclass clause's receiver expression (§6.1's `ec`), which the
    /// nested-class accessor runs once per enclosing object. `None` when the
    /// declaration has no clause.
    pub superclass_clause_code: Option<Rc<CompiledCode>>,
    /// The arguments of the clause's factory message, run on every
    /// instantiation (§6.2). `None` when the message takes none.
    pub superclass_message_code: Option<Rc<CompiledCode>>,
```

`Mixin::empty` sets all three to `None`. Rewrite the module doc's last
paragraph so it says the `ast` fields are read only by the compiler, which runs
while a mixin is built, and that the compiled bodies live on the mixin and its
methods. In `MixinApplication.slot_index`'s doc, "(`compile_module` keys by
`mixin_key`)" becomes "(it lives on the mixin)".

- [ ] **Step 2: The compiler compiles onto a mixin**

`compiler/mod.rs`:
- Delete `compile_initializer`, `compile_module`, and the `method_key` and
  `CompiledModule` imports.
- `compile_mixin_initializer`, `compile_superclass_clause`,
  `compile_superclass_message` and `factory_num_args` take `mixin: &Mixin`
  instead of `&Rc<Mixin>` (bodies unchanged). In `compile_mixin_initializer`'s
  doc, drop "(unlike before the review's round-1 fix)" and replace
  "(`compile_module` keys by `mixin_key`, not by class object, per §6.2)" with
  "(it lives on the mixin, not on a class object, per §6.2)".
- Add:

```rust
/// Compiles `mixin`'s per-mixin bodies onto it: the instance initializer
/// always, the superclass clause and its message's arguments when the
/// declaration has them. Called while the mixin is still a value, before it is
/// wrapped in the `Rc` every application shares -- the one moment it can be
/// written.
pub(crate) fn compile_mixin_bodies(mixin: &mut Mixin, bindings: &Rc<HashMap<Span, Binding>>) {
    mixin.initializer_code = Some(Rc::new(compile_mixin_initializer(mixin, bindings)));
    mixin.superclass_clause_code = compile_superclass_clause(mixin, bindings).map(Rc::new);
    mixin.superclass_message_code = compile_superclass_message(mixin, bindings).map(Rc::new);
}
```

- The module doc's first two lines become
  ```rust
  //! Bytecode compiler entry points: `compile_method` for a method, and
  //! `compile_mixin_bodies` for the three bodies a mixin owns. See
  ```

- [ ] **Step 3: Building a mixin compiles it**

`interpreter/instantiate.rs`:
- `build_methods(body, bindings)` passes `bindings` to
  `MixinMethod::from_decl(Rc::new(m.clone()), bindings)`.
- `build_mixin(&mut self, decl, bindings: &Rc<HashMap<Span, Binding>>)`: pass
  `bindings` to `build_methods`, to the recursive `build_mixin` and to
  `build_class_side`; build the `Mixin` into a `let mut mixin = Mixin { ... }`
  with the three new fields `None`, then
  `crate::compiler::compile_mixin_bodies(&mut mixin, bindings);` and
  `Rc::new(mixin)`.
- `build_class_side(&mut self, side, bindings)`: the same shape —
  `let mut mixin = Mixin { methods: build_methods(side, bindings), ..., ..Mixin::empty(Symbol::CLASS_SIDE) };`,
  `compile_mixin_bodies`, `Rc::new(mixin)`.
- `load_module`: `self.build_mixin(decl, &resolved.bindings)`; delete the
  `compile_module` and `install_code` lines so it ends with
  `self.new_class_object(application, None, None, superclass)`. Its doc's last
  paragraph becomes: "Every method and every per-mixin body is compiled onto
  the declaration it belongs to as the tree is built, so a class this returns
  is already runnable."
- `run_own_initializer`: the `let code = self.code.initializers.get(...)...`
  statement becomes

```rust
        let code = application.mixin.initializer_code.clone().unwrap_or_else(|| {
            panic!("internal: class needs an instance initializer and its mixin carries none (a mixin built from source always does; verify_mixins refuses a decoded one)")
        });
```

- `eval_superclass_message`: the `let code = application.as_ref().and_then(|application| { self.code.superclass_messages.get(...) ... })`
  becomes `application.as_ref().and_then(|application| application.mixin.superclass_message_code.clone())`
  with the same `unwrap_or_else(panic!(...))` message.
- Add `use ns_syntax::Span;` and `Binding` (via `crate::types::*`, already
  imported) as needed.

`interpreter/nested.rs`, `eval_superclass_clause`: `interp.code.superclass_clauses.get(&mixin_key(nested)).cloned()`
becomes `nested.superclass_clause_code.clone()`, same panic.

`interpreter/mod.rs`:
- delete the `code` field (and its doc), `install_code`, `compiled_module`, and
  `code: CompiledModule::default(),` in `bootstrap`; drop the
  `crate::bytecode::{method_key, CompiledModule}` import;
- in `invoke_method`, replace the `let code = self.code.methods.get(...)...;`
  statement with `let code = method.code.clone();` (before `method` moves into
  `FrameKind::Method(method)`).

- [ ] **Step 4: `bytecode.rs`**

Delete `CompiledModule` with its doc and `impl`, `method_key` with its doc, and
the unit test `method_key_is_rc_pointer_identity`. In the remaining tests, a
`MixinMethod::from_decl(decl)` call becomes
`MixinMethod::from_decl(decl, &Rc::new(HashMap::new()))`. In `Instr::InitSlot`'s
doc, "(`compile_module` keys by `mixin_key`, not by class object -- §6.2)"
becomes "(it lives on the mixin, not on a class object -- §6.2)". Drop the
`HashMap` import if unused outside tests.

`dispatch.rs`'s test that calls `MixinMethod::from_decl(Rc::new(method))`: add
`, &Rc::new(HashMap::new())` (an empty bindings map lowers every name as
unbound, which compiles without error).

- [ ] **Step 5: Encoding reads code off the tree**

`image/mixins.rs`:
- `Row.methods` becomes `pub methods: Vec<Rc<MixinMethod>>` (doc unchanged);
  delete `Row.initializer`, `superclass_clause`, `superclass_message` — the
  encoder reads them off `row.mixin`.
- `plan(heap, root_class)`: drop the `module` parameter and its doc paragraph
  about "A method a mixin holds but the `CompiledModule` has no code for" (the
  case no longer exists); `methods` becomes
  `let mut methods: Vec<Rc<MixinMethod>> = mixin.methods.values().cloned().collect();`
  sorted by `a.selector.as_str().cmp(b.selector.as_str())`.
- `encode(w, heap, root_class, symbols, codes)`: drop `module`; per method write
  `codes.id_of(&method.code)`; the three optional ids come from
  `row.mixin.initializer_code`, `row.mixin.superclass_clause_code`,
  `row.mixin.superclass_message_code`.
- Drop the `method_key` and `CompiledModule` imports.

`image/encode.rs`, `encode_code_table(w, heap, root_class, symbols)`: drop
`module`; for each row, `collect_code(&method.code, ...)` for every method, then
`[&row.mixin.initializer_code, &row.mixin.superclass_clause_code, &row.mixin.superclass_message_code].into_iter().flatten()`.
Delete the doc paragraph about a method with no code; "the same order
`compile_module` compiled in" becomes "the same order the mixin table's rows
are written in". `CodeIds::id_of`'s expect message: "every CompiledCode
reachable from the mixin tree was assigned an id by encode_code_table".

`image/mod.rs`: `encode_describing(root_class, heap)`, `snapshot` and `build`
drop `module` (parameter and pass-through); drop the `CompiledModule` import.

- [ ] **Step 6: Decoding attaches code; verification keeps its checks**

`image/mixins.rs`, `decode`: pass `codes` on to `build(raw, codes)`. In
`build`, each `MixinMethod` gets `code: codes[id as usize].clone()` (the fourth
tuple field, range-checked by `read_row`), and the `Mixin` literal gets
`initializer_code: row.initializer.map(|id| codes[id as usize].clone())` and
the same for `superclass_clause_code` / `superclass_message_code`. Keep
`MixinCode` and `Decoded.code`: `inspect` attributes code ids with them. Update
`MixinCode`'s doc: the ids are what the row said; the bodies themselves are on
the decoded mixin. `decode` itself still reconciles nothing — `inspect` reads
crafted artifacts through it and must keep displaying them.

`image/decode.rs`: replace `build_module` with `verify_mixins`, which runs the
same checks in the same order and returns the same errors, reading the attached
code:

```rust
/// Reconciles every decoded mixin with the bodies its row attached.
///
/// Both halves of a row come off the wire -- a declaration's arity and kind on
/// one side, a code-table id on the other -- so a crafted image can make them
/// disagree:
///
/// - a body's `kind` and `num_args` against the declaration's own arity
///   (`check_code_arity`), because the activation a body runs in is built
///   from the *declaration* -- `args` from the send or the factory -- while its
///   temporaries are sized from the *body*;
/// - the presence of a body wherever a declaration makes one mandatory (the
///   three `Missing*` errors), because the run-time paths that reach for
///   those bodies panic on the invariant rather than failing.
///
/// Methods are checked in selector order, as the row lists them, so the error
/// reported is the one the row's first bad entry causes.
pub fn verify_mixins(mixins: &mixins::Decoded) -> Result<(), DecodeError> {
    for mixin in &mixins.mixins {
        let mut methods: Vec<&Rc<crate::mixin::MixinMethod>> = mixin.methods.values().collect();
        methods.sort_by(|a, b| a.selector.as_str().cmp(b.selector.as_str()));
        for method in methods {
            check_code_arity(
                &method.code,
                CodeKind::Method,
                format!("method `{}`", method.selector),
                method.num_args,
            )?;
        }
        match &mixin.initializer_code {
            Some(code) => check_code_arity(
                code,
                CodeKind::Initializer,
                "instance initializer".to_string(),
                mixin.factory.map_or(0, |factory| factory.num_args),
            )?,
            None if instantiate::mixin_needs_initializer_code(mixin) => {
                return Err(DecodeError::MissingInitializer)
            }
            None => {}
        }
        match &mixin.superclass_clause_code {
            Some(code) => {
                check_code_arity(code, CodeKind::SuperclassClause, "superclass clause".to_string(), 0)?
            }
            None if mixin.superclass.is_some() => return Err(DecodeError::MissingSuperclassClause),
            None => {}
        }
        match &mixin.superclass_message_code {
            Some(code) => check_code_arity(
                code,
                CodeKind::SuperclassMessage,
                "superclass clause message".to_string(),
                mixin.factory.map_or(0, |factory| factory.num_args),
            )?,
            None if mixin_needs_superclass_message_code(mixin) => {
                return Err(DecodeError::MissingSuperclassMessage)
            }
            None => {}
        }
    }
    Ok(())
}
```

Carry over the explanatory comments `build_module` had at each `match` (why a
clause body takes no arguments, why the message body's arity is the factory's);
drop the one about "the other direction -- a declared method with no compiled
body", which is now unrepresentable. Keep `check_code_arity`,
`mixin_needs_superclass_message_code` and the `DecodeError` variants
(`UnknownSelector` is still used by frame decoding).

`frame_body(heap, frame)` drops `module`:

```rust
    match &heap.frame_kind(frame) {
        FrameKind::Method(method) => Some(method.code.clone()),
        FrameKind::Initializer(class) => frame_mixin(heap, class).and_then(|m| m.initializer_code.clone()),
        FrameKind::SuperclassClause(class) => {
            frame_mixin(heap, class).and_then(|m| m.superclass_clause_code.clone())
        }
        FrameKind::SuperclassMessage(class) => {
            frame_mixin(heap, class).and_then(|m| m.superclass_message_code.clone())
        }
        FrameKind::Closure(closure) => Some(closure.code.clone()),
        FrameKind::TopLevel => None,
    }
```

keeping the existing comment on the clause arms. `verify_graph(graph, table,
heap)` drops `module` and passes nothing more to `frame_body`; the section
comment above `frame_body` ("Once the code table is installed as a
`CompiledModule`, ...") becomes "Once the decoded mixins carry their code,
...". Drop the `method_key`, `mixin_key` and `CompiledModule` imports that are
now unused.

`image/mod.rs`, `load`: `let module = decode::build_module(...)?;` becomes
`decode::verify_mixins(&mixins).map_err(|e| e.to_string())?;`, `verify_graph`
loses `&module`, and `interp.install_code(module);` goes. `LoadedImage.interp`'s
doc becomes "Bootstrapped. The image's code lives on the declarations its mixin
table rebuilt." `load`'s doc: "Installs the code table and, for an image,
advances" becomes "Verifies the decoded declarations against their code and,
for an image, advances"; "`verify_code_table` between decoding and installing
the code table" becomes "`verify_code_table` and `verify_mixins` before
anything is built on the code".

- [ ] **Step 7: The CLI**

`crates/ns-cli/src/main.rs`:
- `cmd_snapshot` and `cmd_build`: delete `let module = interp.compiled_module();`
  and the `module,` argument.
- `cmd_disasm`: each arm produces only `root_mixin`, not a `(root_mixin, module)`
  pair; the initializer arm becomes

```rust
        (None, true) => {
            let code = mixin.initializer_code.clone().unwrap_or_else(|| {
                panic!("internal: `{}` carries no instance initializer", mixin.name)
            });
            (code, mixin.name.to_string())
        }
```

  and the selector arm's `module.methods.get(...)` lookup becomes
  `method.code.clone()`. Update `cmd_disasm`'s doc and the "Two ways to reach
  the same pair" comment: both arms now reach the root mixin, whose tree
  carries the code; fix "resolver.s" to "resolver's" while there.

- [ ] **Step 8: `compiler.rs` tests read the declarations**

Replace `crates/ns-runtime/tests/compiler.rs`'s module doc with
`//! What \`load_module\` compiles onto the declarations it builds.`; its `load`
returns `(Interpreter, ClassRef)` (drop bindings, `class_mixin_key`, and the
`method_key`, `mixin_key`, `HashMap`, `Span`, `Binding`, `compiler` imports that
become unused), plus a helper:

```rust
fn root_mixin(interp: &Interpreter, class: &ClassRef) -> Rc<Mixin> {
    interp
        .heap_ref()
        .class_application(class)
        .unwrap_or_else(|| panic!("not a class"))
        .mixin
        .clone()
}
```

Rewrite the five tests, keeping their assertions on the code itself:

- `compile_module_compiles_every_loaded_method_and_the_initializer` →
  `loading_compiles_every_method_and_the_initializer_onto_the_mixin`: iterate
  `mixin.methods.values()` (2 entries), assert each `method.code.kind ==
  CodeKind::Method` and ends in `Instr::Return`; `mixin.initializer_code`
  is `Some` with `kind == CodeKind::Initializer` and instrs
  `[LoadSelf { dst: 0 }, ImplicitReturn { reg: 0 }]`.
- `compiled_method_is_reachable_by_its_dispatch_key` →
  `the_method_dispatch_finds_carries_its_body`:
  `match ns_runtime::dispatch::lookup_defined_for(interp.heap_ref(), &class, Symbol::intern("foo"))`
  — access-blind, because the fixture's unmarked `foo` is protected and
  `lookup_public` would not find it — expect `Lookup::Method(method, _)` and
  assert `method.code.kind == CodeKind::Method`; any other arm
  `panic!("expected a method")` (`Lookup` is not formatted).
- `compile_initializer_lowers_slots_then_init_exprs_with_own_slot_indices`
  keeps its name and all its assertions, reading
  `let init = root_mixin(&interp, &class).initializer_code.clone().expect("an initializer");`.
- `an_uninitialized_slot_emits_nothing`: the same read.
- `compile_module_compiles_every_mixin_in_the_tree_not_just_the_root` →
  `every_mixin_in_the_tree_gets_its_bodies`: `Inner`'s `value` and the class
  side's `marker` have `code.kind == CodeKind::Method`; `inner.initializer_code`
  and `class_side.initializer_code` are `Some`. Its lead comment loses the
  sentence about the accessor not existing yet.

- [ ] **Step 9: `image.rs` tests**

Helpers first. `load` returns `(Interpreter, ClassRef)`; delete the `Bindings`
alias and the bindings clone. Every `let (a, b, _) = load(...)` and
`let (a, b, bindings) = load(...)` becomes a two-tuple. Add, beside
`mixin_preorder`:

```rust
/// `class`'s own mixin as a value a test can edit. The clone is shallow: its
/// methods, nested declarations and class side are the same `Rc`s.
fn mixin_of(heap: &Heap, class: &ClassRef) -> Mixin {
    (*heap.class_application(class).expect("a class object").mixin).clone()
}

/// Replaces `class`'s mixin with `mixin`, applied to the same superclass
/// application. A mixin is shared and immutable, so this is the shape an edit
/// to a live class takes -- and, now that code lives on the declaration, how a
/// test puts a doctored body in front of the encoder.
fn install_mixin(interp: &mut Interpreter, class: &ClassRef, mixin: Mixin) {
    let super_application = interp
        .heap_ref()
        .class_application(class)
        .expect("a class object")
        .superclass
        .clone();
    let application = interp.apply_mixin(&Rc::new(mixin), super_application.as_ref());
    assert!(
        interp.heap_mut().set_class_application(class, application),
        "a class object"
    );
}

/// `mixin` with method `selector`'s body replaced by `code`.
fn with_method_code(mut mixin: Mixin, selector: &str, code: Rc<CompiledCode>) -> Mixin {
    let selector = Symbol::intern(selector);
    let method = mixin.methods[&selector].clone();
    mixin.methods.insert(
        selector,
        Rc::new(ns_runtime::mixin::MixinMethod {
            code,
            ..(*method).clone()
        }),
    );
    mixin
}

/// `mixin` with its direct nested declaration `name` replaced by `edit`'s
/// result.
fn with_nested(mut mixin: Mixin, name: &str, edit: impl FnOnce(Mixin) -> Mixin) -> Mixin {
    let index = mixin
        .nested
        .iter()
        .position(|n| n.name.as_str() == name)
        .expect("a nested declaration of that name");
    let edited = edit((*mixin.nested[index]).clone());
    mixin.nested[index] = Rc::new(edited);
    mixin
}
```

`method_code(heap, class, selector)` drops `module` and returns
`application.mixin.methods[&Symbol::intern(selector)].code.clone()`.
`class_mixin_key` and `class_method_key` are deleted once nothing uses them.
`round_trip_code_table(heap, class)` drops `module`; its return type stays. A
test that needs a doctored body doctors the *live* mixin before the round trip
(below): `mixins::decode` attaches whatever body the row names, so doctoring
`table.codes` after decoding no longer reaches the decoded mixins.

Then every call site, by what it does:

- **Passes `interp.compiled_module()` or a recompiled module to
  `image::snapshot`, `image::build`, `encode_code_table`, `mixins::encode` or
  `round_trip_code_table`** (about forty sites, including the fourteen
  `compiler::compile_module(...)` recompilations): drop the argument and the
  `let module = ...;` line. `method_code(heap, &class, &module, sel)` becomes
  `method_code(heap, &class, sel)`; `module.initializers[&class_mixin_key(..)]`
  becomes `mixin_of(heap, &class).initializer_code.expect("an initializer")`.
- **Doctors a body and expects `build_module` to refuse it**
  (`a_method_body_taking_the_wrong_number_of_arguments_is_refused_at_install`,
  `a_method_root_naming_a_block_body_is_refused_at_install`,
  `an_image_with_no_initializer_for_a_class_that_needs_one_is_refused_at_install`,
  `an_image_with_no_superclass_clause_message_for_a_clause_that_needs_one_is_refused`):
  doctor the live mixin before the round trip, then call
  `decode::verify_mixins(&decoded_mixins)` and assert the same error. For the
  arity test: `let body = CompiledCode { num_args: 2, ..(*method_code(interp.heap_ref(), &class, "foo:")).clone() };`
  then `install_mixin(&mut interp, &class, with_method_code(mixin_of(interp.heap_ref(), &class), "foo:", Rc::new(body)));`.
  The kind test: the same with `kind: CodeKind::Block`. The initializer test:
  `install_mixin(.., Mixin { initializer_code: None, ..mixin_of(..) })`, keeping
  its assertion that the undoctored class compiled one. The superclass-message
  test: `with_nested(mixin_of(..), "Derived", |d| Mixin { superclass_message_code: None, ..d })`.
  Rename `..._refused_at_install` to `..._refused_on_load` where the name says
  "install".
- **Doctors a whole world and snapshots it**
  (`a_whole_image_missing_its_initializer_is_refused_by_load`,
  `a_superclass_message_body_that_returns_no_argument_list_is_an_error_not_a_panic`,
  `every_instr_variant_round_trips`): the same doctoring through
  `install_mixin`, then the snapshot with no module argument. In
  `every_instr_variant_round_trips`, the synthetic body becomes the root's
  `initializer_code`; in the argument-list test, `Derived`'s
  `superclass_message_code`, via `with_nested`. Keep each test's
  `instantiate` where it is, *before* `install_mixin`: `install_mixin` swaps
  the application of the class object itself, so an instance made earlier is
  already an instance of the doctored class — and instantiating a class whose
  doctored mixin lacks a body it needs would panic in `run_own_initializer`.
- **`closure_world`** returns `(GraphFixture, decode::DecodedCodeTable)`: instead
  of a `CompiledModule` mapping `foo` to `parent`, it calls
  `install_mixin(&mut interp, &class, with_method_code(mixin_of(interp.heap_ref(), &class), "foo", parent.clone()))`
  before building `fixture.mixins` with `mixin_preorder`, and all four of its
  callers (`git grep -n "closure_world()" crates/ns-runtime/tests/image.rs`)
  destructure two values and call
  `decode::verify_graph(&graph, &table, fixture.interp.heap_ref())`.
- **Tests whose property became structural** — delete them, since the state
  they guard against can no longer be built:
  `a_method_added_after_compile_module_is_an_encoding_error_not_a_panic` (a
  method now always carries its code) and
  `every_decoded_method_has_a_body_in_the_built_module` (the same, decoded).
- **Tests that read the built module** — rewrite against the decoded mixins:
  `a_decoded_code_table_builds_the_module_from_the_mixin_table_alone` becomes
  `a_decoded_mixin_carries_the_bodies_its_row_names`: for every method of the
  decoded root, `*method.code == *method_code(interp.heap_ref(), &class, sel)`,
  and the decoded root's `initializer_code` is `Some`.
  `a_snapshotted_nested_mixins_method_and_initializer_survive_a_load` asserts
  the decoded `Inner`'s `value` has a body equal to the live one's and that its
  `initializer_code` is `Some`.
- **`a_frames_sender_survives_a_round_trip`** reads `foo`'s body as
  `method_code(interp.heap_ref(), class, "foo")`.
- **`a_class_tree_nested_past_the_ceiling_is_refused_when_the_image_is_written`**
  drops its empty `CompiledModule` and the argument.

Drop imports that become unused (`CompiledModule`, `method_key`, `compiler`,
`HashMap`, `Binding`, `Span` where so). Build with
`cargo build -p ns-runtime --tests` and fix what remains by the rules above.

- [ ] **Step 10: Build, test, gates, commit**

Run `cargo test --workspace`. Expected: PASS, with the three deleted tests
gone and every other image test passing — in particular every error-message
assertion unchanged, since `verify_mixins` returns `build_module`'s errors.
Then:
`git grep -n -E "CompiledModule|method_key|compile_module|compile_initializer|install_code|compiled_module|build_module" crates`
Expected: no hits. Run `cargo fmt --all`,
`cargo clippy --workspace --all-targets`, both corpus gates.

- [ ] **Step 11: The design documents**

These describe the result; update them in this task so the commit is
self-consistent.

- `CLAUDE.md`: replace the invariant **Compiled code is keyed by mixin, and
  `Instr::InitSlot`'s `own_index` is mixin-relative.** with

  **Compiled code lives on the mixin, and `Instr::InitSlot`'s `own_index` is
  mixin-relative.** One mixin backs every application of it, so each body is
  compiled once, onto the mixin — a method's onto its `MixinMethod` — and
  shared by every application; an absolute field index baked into that shared
  code would be wrong the moment a second application inserts different
  inherited slots ahead of it. The VM resolves `own_index` against the
  executing activation's `slot_base` (`MixinApplication.own_slot_base` for
  that level) at run time. Getting it wrong is silent wrong output under one
  application and correct output under another; `vm_initializer.rs` pins both.

  In the handle invariant, replace "`compiler::compile_module` and
  `compile_initializer` take a `&Heap` parameter for exactly this reason" with
  "`image::encode::encode_code_table` and `image::mixins::plan` take a `&Heap`
  parameter for exactly this reason".
- `heap.rs`, `Heap::new`'s doc: "`compile_initializer`, `compile_module` and
  `cmd_disasm` take a real `&Heap` parameter" becomes "`encode_code_table` and
  `mixins::plan` take a real `&Heap` parameter and `ns-cli`'s `disasm` reads
  through the interpreter that built its class object".
- `docs/design/compiler.md`: in "Compiled code", replace the `CompiledModule`
  paragraph with: a method's body is on its `MixinMethod` (`code`), a mixin's
  instance initializer and superclass-clause bodies on the `Mixin`
  (`initializer_code`, `superclass_clause_code`, `superclass_message_code`);
  `MixinMethod::from_decl` and `compile_mixin_bodies` compile them while the
  tree is built from source, `image::mixins::decode` attaches them from the code
  table, so a method without a body cannot exist, and nothing looks code up
  beside a declaration — prior art: Primordial Soup's `Method::bytecode()`,
  SOMns's `SInvokable.invokable`. Rename "### Keyed by mixin" to
  "### One body per mixin" and change its first sentence to "One mixin backs
  every application of it, so a body is compiled **once per mixin**, stored on
  it, and shared by every application." Rewrite "## Follow-up: entry points
  that reach a class object only to find its mixin" for what is left:
  `compile_module` and `compile_initializer` no longer exist; the pattern
  remains in `image::encode_code_table` / `mixins::plan` (a `ClassRef` and a
  `&Heap` to reach one mixin) and in `disasm`. The Decisions bullet "**Code is
  keyed by mixin and `InitSlot` is mixin-relative**" becomes "**Code lives on
  the mixin and `InitSlot` is mixin-relative**".
- `docs/design/execution.md`: "## The VM"'s first sentence ("`Interpreter`
  carries one code table, ...") becomes: "Every body a program runs hangs off
  the declaration it belongs to — a method's off its `MixinMethod`, an
  initializer's and a superclass clause's off the `Mixin` — so what dispatch
  finds is what runs; nothing looks code up beside it." In item 3, "a user
  class missing from the table is a compilation-coverage bug and panics"
  becomes "a user class whose mixin carries no initializer is refused on load
  and panics if built any other way". In "What 1a left for it", delete the
  bullet "**The `HashMap` lookup** ..." (it is gone).
- `docs/design/dispatch.md`, "Planned: inline caches": "A cache removes the
  lookup and can carry the compiled body along with it, retiring the per-call
  `HashMap` lookup in `invoke_method`." becomes "A cache removes the lookup;
  the method it records carries its compiled body." Delete "The per-call
  `HashMap` lookup is still there, and it is the caches' to remove."
- `docs/design/memory.md`, "Open: what the collector owns": delete the bullet
  "**The code table is keyed by `Rc` identity** ..." — there is no code table.
  (The application cache and `nested_slot` still key by `mixin_key`; their
  bullet stays.)
- `docs/design/image-format.md` and `object-model.md`: fix the sentences
  `git grep -n "CompiledModule\|install_code" docs/design` finds, to say the
  decoded mixins carry their code and `verify_mixins` reconciles it.
- Expected afterwards:
  `git grep -n -E "CompiledModule|method_key|compile_module|compile_initializer|install_code|compiled_module|build_module" CLAUDE.md docs/design`
  finds nothing.

- [ ] **Step 12: Commit**

```bash
git add -A crates CLAUDE.md docs/design
git commit -m "refactor!: compiled code lives on the declaration it belongs to

A MixinMethod carries its body and a Mixin its instance initializer and
superclass-clause bodies, compiled while the tree is built from source and
attached while it is decoded from an image -- as Primordial Soup's
Method::bytecode() and SOMns's SInvokable hold theirs. A send that found a
method has found its code, a method without a body cannot exist, and the
code table keyed by Rc address is gone with the collector hazard memory.md
recorded for it. CompiledModule, method_key, compile_module,
compile_initializer, install_code and build_module are removed;
verify_mixins keeps build_module's checks and errors, and snapshot and
build no longer take a module.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

- [ ] **Step 13: Measure and check bytes**

Run the measurement protocol; record under "Task 6". Expected: `bench_send.ns`
faster (one SipHash lookup fewer per user-method send). Before removing the
worktree, run the byte-identity protocol. Expected: `differing artifacts: 0`
— every source mixin still gets an initializer, and bodies are collected in
the same row and selector order.

---

### Task 7: Handles by value, enforced by clippy

`ObjRef`, `ClassRef`, `FrameRef` and `ActivationRef` are `Copy` and 4 bytes in a
release build, so a `&` to one is an indirection that buys nothing. Clippy's
`trivially_copy_pass_by_ref` finds them, with two settings measured while
planning: its size limit defaults to 8 bytes, and in the build clippy checks
(debug) `ActivationRef` carries `origin` and `generation` and is 12, so the limit
must be 12; and by default it skips every `pub` function, which is all of
`heap.rs`'s accessors, so `avoid-breaking-exported-api` must be off (ns-next has
no external users). With both, it reports 146 sites, all of them handles or
`&self` on a handle plus one `&CallSite`, and nothing else.

**Files:**
- Create: `clippy.toml`
- Modify: `Cargo.toml`
- Modify: every file clippy names (about 18 under `crates/`, `heap.rs` foremost)

**Interfaces:**
- Produces: every function taking a handle takes it by value
  (`fn class_application(&self, class: ClassRef)`,
  `fn same(self, other: ObjRef) -> bool`, `run_code(interp, act: ActivationRef, code)`, ...).
  `Option<&ClassRef>` parameters may stay; the lint does not cover them.

- [ ] **Step 1: The lint**

Create `clippy.toml` at the repository root:

```toml
# `trivially_copy_pass_by_ref` must see every handle. In the build clippy
# checks, `ActivationRef` carries its debug-only `origin` and `generation`
# fields and is 12 bytes, past the lint's default 8-byte limit; in a release
# build every handle is 4. And by default the lint skips `pub` functions --
# which is every `Heap` accessor -- to avoid breaking an exported API; ns-next
# has no external users to break.
trivial-copy-size-limit = 12
avoid-breaking-exported-api = false
```

In `Cargo.toml`, under `[workspace.lints.clippy]`, add after
`cloned_instead_of_copied = "warn"`:

```toml
# `ObjRef`, `ClassRef`, `FrameRef` and `ActivationRef` are `Copy` indices; a
# reference to one is an indirection that buys nothing. Sized by `clippy.toml`.
trivially_copy_pass_by_ref = "warn"
```

and extend the comment above `[workspace.lints.clippy]` so it no longer speaks
only of `cloned_instead_of_copied` (one sentence per lint).

- [ ] **Step 2: See it fail**

Run: `cargo clippy --workspace --all-targets 2>&1 | Select-String "passed by reference" | Measure-Object`
Expected: about 146 warnings. (If clippy reports none, it is serving a cached
result: touch any `.rs` file and re-run.)

- [ ] **Step 3: Convert**

Work outward from `heap.rs`: change each flagged parameter from `&Handle` to
`Handle`, and `&self` on a handle's `same` to `self` (its `other` parameter to a
value too); then build and fix every call site the compiler reports — `&act`
becomes `act`, `&class` becomes `class`, `a.same(b)` with `b: &ObjRef` becomes
`a.same(*b)`. The `&CallSite` parameter in `image/encode.rs` goes by value too.
Repeat `cargo clippy --workspace --all-targets` until it is warning-free. No
behaviour changes; nothing but signatures and call sites moves.

- [ ] **Step 4: Test, gates, commit**

Run `cargo test --workspace` (expected: PASS), `cargo fmt --all`, both corpus
gates.

```bash
git add -A clippy.toml Cargo.toml crates
git commit -m "refactor: pass Copy handles by value, enforced by clippy

trivially_copy_pass_by_ref now runs with a 12-byte limit, which covers
ActivationRef's debug layout, and with exported-API avoidance off, which
covers Heap's accessors.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

- [ ] **Step 5: Measure**

Run the measurement protocol; record under "Task 7". Expected: unchanged
within noise (inlining already removed most of these indirections in a release
build); a slowdown is a reason to look, not to revert blindly.

---

### Task 8: One module per CLI command

`crates/ns-cli/src/main.rs` is ~1500 lines holding ten commands and the helpers
they share. It splits by command, with the shared pieces in two modules of
their own. Code moves verbatim except where this task says otherwise.

**Files:**
- Modify: `crates/ns-cli/src/main.rs`
- Create: `crates/ns-cli/src/source.rs`, `entry.rs`,
  `commands/mod.rs`, `commands/parse.rs`, `parse_corpus.rs`, `run_corpus.rs`,
  `run.rs`, `snapshot.rs`, `load.rs`, `bench.rs`, `disasm.rs`, `inspect.rs`,
  `build.rs` (all under `crates/ns-cli/src/commands/` except the first two)

**Interfaces:**
- Produces: each `commands::<name>::execute(...) -> ExitCode` with the parameters
  its `cmd_<name>` had; `entry::instantiate_root(interp: &mut Interpreter, class: ClassRef) -> Result<Value, ExitCode>`.

- [ ] **Step 1: Where everything goes**

| Module | Contents (moved from `main.rs`) |
|---|---|
| `main.rs` | `Cli`, `Cmd`, `main`, `mod source; mod entry; mod commands;` |
| `source.rs` | `LoadedSource`, `load_source`, `bootstrap`, `collect_ns_files`, `print_diagnostics`, `line_col` |
| `entry.rs` | `report_entry_send`, `report_signal`, `factory_args_for`, `class_name`, `selector_arity`, `entry_args`, and the new `instantiate_root` (Step 2) |
| `commands/mod.rs` | `pub(crate) mod` for each command below, nothing else |
| `commands/parse.rs` | `cmd_parse` → `execute` |
| `commands/parse_corpus.rs` | `cmd_parse_corpus` → `execute`, `load_blacklist` |
| `commands/run_corpus.rs` | `RunnableEntry`, `load_whitelist`, `RunOutcome`, `run_one`, `ScratchDir`, `run_one_binary`, `cmd_run_corpus` → `execute` |
| `commands/run.rs` | `cmd_run` → `execute` |
| `commands/snapshot.rs` | `cmd_snapshot` → `execute` |
| `commands/load.rs` | `cmd_load` → `execute` |
| `commands/bench.rs` | `BenchStats`, `signal_message`, `timed_send`, `bench_entry`, `cmd_bench` → `execute` |
| `commands/disasm.rs` | `resolve_mixin_path`, `looks_like_an_artifact`, `root_mixin_of`, `cmd_disasm` → `execute` |
| `commands/inspect.rs` | `cmd_inspect` → `execute` |
| `commands/build.rs` | `root_class_answers`, `cmd_build` → `execute` |

Helpers used across modules become `pub(crate)`; everything else stays private.
Keep `#![warn(clippy::all)]` at the top of `main.rs`. Each command file gets a
one-line module doc naming the command — for `snapshot.rs`,
``//! `ns-next snapshot`.`` — and the command's existing doc comment moves onto
`execute`. In `main`, each arm calls
`commands::<name>::execute(...)`.

- [ ] **Step 2: One way to instantiate the root**

`cmd_run`, `cmd_snapshot` and `cmd_load`'s stateless branch each repeat the same
eight lines — `factory_args_for`, print on error, `instantiate`,
`report_signal`. In `entry.rs` add:

```rust
/// Instantiates the root `class` with the arguments `factory_args_for` builds
/// for it, reporting a failure the way every command that starts a program
/// does. `Err` carries the exit code to return.
pub(crate) fn instantiate_root(
    interp: &mut ns_runtime::Interpreter,
    class: ns_runtime::ClassRef,
) -> Result<ns_runtime::Value, ExitCode> {
    let platform = interp.platform();
    let factory_args = factory_args_for(interp.heap_ref(), class, platform).map_err(|msg| {
        eprintln!("error: {msg}");
        ExitCode::FAILURE
    })?;
    interp.instantiate(class, factory_args).map_err(report_signal)
}
```

(`factory_args_for` takes its class by value after Task 7; if it does not,
pass `&class`.) Replace the three copies with
`let instance = match entry::instantiate_root(&mut interp, class) { Ok(v) => v, Err(code) => return code };`
(`root_class` in `load`). Each of those commands still calls
`interp.platform()` where it needs the platform for `entry_args` — the call is
memoized, so no allocation moves.

- [ ] **Step 3: Test, gates, commit**

Run `cargo test --workspace` (expected: PASS; the CLI tests drive the binary,
so none change), `cargo fmt --all`, `cargo clippy --workspace --all-targets`,
both corpus gates.

```bash
git add -A crates/ns-cli
git commit -m "refactor: split ns-cli's main.rs into one module per command

main.rs keeps the argument definitions; source.rs reads and resolves a
module, entry.rs holds the CLI's entry-point conventions, and each command
lives in commands/. The three copies of root instantiation become
entry::instantiate_root.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 9: Comments state decisions, not history

The inventory's comment items, and the wider sweep PROGRESS assigns B: comments
that speak of an engine choice, of milestones, of what code "used to" do, or of
review rounds. `CLAUDE.md`: "Comments should explain the decision, not restate
the mechanics. The reason a check exists — what it prevents — is the part a
later reader cannot reconstruct." The history is in git.

**Files:**
- Modify (comments, doc comments and CLI help text only): files under
  `crates/ns-runtime/` and `crates/ns-cli/`, `Cargo.toml`. Not `crates/ns-syntax`.

**Interfaces:** none.

- [ ] **Step 1: The named items**

- `Cargo.toml`, `[profile.release]` comment: "the engines and the shared
  orchestration they call into live in different modules" becomes "the VM and
  the send machinery it calls into live in different modules".
- `crates/ns-runtime/src/lib.rs` doc: "for the Newspeak3 language (Milestones
  1-3), including" becomes "for the Newspeak3 language, including".
- `crates/ns-cli/src/main.rs`, `Snapshot`'s help: "Run a file under the
  bytecode engine, then serialize" becomes "Run a file, then serialize".
  `Bench`'s doc: "what Milestone 4's inline caches change is repeated dispatch"
  becomes "what the runtime's optimizations change is repeated dispatch".
- `commands/build.rs`, `execute`'s doc: "`run`'s pipeline up to engine
  installation, and then deliberately no instantiation" becomes "`run`'s
  pipeline up to loading the module, and then deliberately no instantiation".
- `source.rs`: `load_source`'s "reporting diagnostics exactly as the Milestone 1
  `run` did" becomes "reporting diagnostics the way every command does";
  `entry.rs`: `report_entry_send`'s "exactly as Milestone 1's `run` did --"
  becomes "--"; `entry_args`' paragraph beginning "It used to be `[platform,
  nil]` unconditionally" becomes a statement of why the list is sized to the
  selector (a surplus argument would shift every temporary; `invoke_method`
  refuses the mismatch, and sizing the list keeps a legitimate `--send` from
  tripping it).
- `image/inspect.rs` module doc: "Nothing here installs an engine, allocates a
  class object, or runs" becomes "Nothing here bootstraps an interpreter,
  allocates a class object, or runs".
- `interpreter/literals.rs`, `unresolved_identifier_error`'s doc: "-- in
  Milestone 1 and 2 reachable only for a bare `super`," becomes "-- reachable
  only for a bare `super`,".
- `compiler/lower.rs` (~406): the comment on `Some(Binding::SuperSend) | None`
  becomes

  ```rust
            // The resolver attaches `SuperSend` to a bare identifier in one
            // place: a `super`-prefixed superclass-clause receiver (the known
            // gap in `docs/design/dispatch.md`, "Binding variants"). It is
            // grouped with `None` because both fall to the same builtin-name
            // fallback.
  ```

  and, in the arm below, "is a program M1 accepts and rejects at *evaluation*
  time" becomes "is a program the language accepts and rejects at *evaluation*
  time".
- "Shared orchestration": where a comment uses the phrase, it becomes "the send
  machinery" (`interpreter/`), matching `CLAUDE.md`; where the point is that
  the code is shared, say with what — the reference lowering and a JIT to come.

- [ ] **Step 2: The sweep**

Run:
`git grep -n -i -E "milestone|\bM[1-6]\b|used to |tree.walk|\bengine|shared orchestration|review finding|round-1|re-review|earlier version|previously|before the fix|this milestone|a later milestone|M[1-6] spec" -- crates/ns-runtime crates/ns-cli Cargo.toml`

and rewrite every hit in a comment by these rules:

1. **A milestone as a reason or a time** ("Milestone 1 simplification", "in
   Milestone 2", "M1 raises", "this milestone", "a later milestone's inline
   caches") states the present fact without it: "Milestone 1 has no dedicated
   Character payload" → "There is no dedicated Character payload"; "a later
   milestone's inline caches key on it" → "the inline caches will key on it".
2. **A story of a change** ("used to", "previously", "an earlier version",
   "before the fix", "Regression fix (re-review finding A)", "The review's
   finding") becomes what the code prevents now. In a test, the failure the
   test guards against stays — phrased as that failure, not as an event.
3. **A reference to a deleted spec** ("M2 spec, \"Control-flow inlining\"",
   "the runtime spec's ... section") points at the living document instead:
   `docs/design/compiler.md`, "Control-flow inlining", or the relevant
   `docs/design/` section; `newspeak-spec.pdf` citations stay.
4. **Leave alone:** string literals (error messages — see Scope), `prior-art/`,
   `docs/superpowers/plans/`, and `crates/ns-syntax`. A hit that is not about
   history (e.g. "engine" in a sentence about Nova, the JS engine) stays.

- [ ] **Step 3: Verify**

Re-run the grep from Step 2. Expected: the only remaining hits are the two
error strings the Scope section names
(`builtins/numbers.rs`'s "Milestone 1 has no Integer/Float coercion" and
`interpreter/literals.rs`'s "not supported in Milestone 1") and hits rule 4
keeps. Then `cargo test --workspace` (expected: PASS — the two strings are
untouched, so every error assertion still holds).

- [ ] **Step 4: Gates and commit**

Run `cargo fmt --all`, `cargo clippy --workspace --all-targets`, both corpus
gates.

```bash
git add -A crates Cargo.toml
git commit -m "docs: state decisions in comments instead of their history

Comments that narrated milestones, review rounds or what code used to do
now say what the code does and what it prevents; references to the deleted
milestone specs point at docs/design. Error text that names a milestone is
program output and stays.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

---

### Task 10: Documents, the whole-step measurement, and PROGRESS

**Files:**
- Modify: `CLAUDE.md`, `docs/design/execution.md`, `dispatch.md`,
  `core-library.md`, `memory.md`, `compiler.md`, `README.md`
- Modify: `docs/PROGRESS.md`

- [ ] **Step 1: Measure the whole step in one sitting**

Run the measurement protocol with `master` (the merge-base) as "before".
Record the 18 medians under "Task 10 — B against master". Run the
byte-identity protocol against the same two binaries. Expected:
`differing artifacts: 0`. Collect the per-task tables from
`target\b-measurements.md`.

- [ ] **Step 2: `dispatch.md` — the two decisions and the lexical-send cost**

In "Planned: inline caches", after the paragraph on guarding by application,
add:

> **A native send binds an enumerated primitive.** Native dispatch today
> selects with `match selector.as_str()` after the lookup found
> `Lookup::Native(kind)`. The caches do not keep that shape. A lookup that
> reaches a native binds an *enumerated primitive* — a Rust enum naming the
> operation, `IntAdd` rather than a selector — and a native cache entry
> carries it, so a hit costs the guard and an indexed jump. An enumeration,
> not a function pointer, because the JIT tiers must be able to recognize the
> primitive and emit it inline: a function pointer can only be called, which
> is the same ceiling a Rust callback imposes ([core-library.md](core-library.md)).
> This is Cog's and Primordial Soup's model: a method's primitive number
> indexes `primitiveTable` in the interpreter and `primitiveGeneratorTable` in
> Cog's JIT, whose generators (`genPrimitiveAdd`) emit machine code, and at the
> send site `genSpecialSelectorArithmetic` emits SmallInteger arithmetic
> inline with a fallback send (`prior-art/opensmalltalk-vm/src/spur64.cog/`,
> `prior-art/primordialsoup/vm/primitives.cc`). SOMns reaches the same result
> through Graal's partial evaluation of its primitive nodes.
>
> **A lexical send to a slot is cached too.** `dispatch_lexical_send` checks
> for a slot before any method lookup: it interns the getter name for every
> setter-shaped selector (`sends::slot_selector_shape`) and hashes the name
> into the application's `slot_index` for every lexical send. A lexical-send
> cache entry that records the slot's index removes both.

- [ ] **Step 3: `core-library.md`**

In "Planned: the leaf/callback split", replace the paragraph beginning "A
related item, same cause: native dispatch selects with `match
selector.as_str()`" so it keeps the measured cost (~53 ns a native `+`) and
ends with where the fix lives: a lookup binds an enumerated primitive that a
native inline-cache entry carries and the JIT emits inline
([dispatch.md](dispatch.md), "Planned: inline caches"); a leaf such as `+`
additionally gets an inlined fast path in compiled code, guarded like the
control-flow inlining, as Cog's `bytecodePrimAdd` does. Add to Decisions:
"**A native is bound as an enumerated primitive, not a function pointer**, so
the JIT can emit it inline."

- [ ] **Step 4: `execution.md`, `memory.md`, `compiler.md`, `README.md`, `CLAUDE.md`**

- `execution.md`, "Measurements": update the opening paragraph's "On a release
  build, on 2026-09-11, their medians were about 16.5, 20.5 and 38 ms." to
  Step 1's "after" figures and today's date. Add a subsection
  "### What step B moved" after "What retiring the tree-walker moved": Step 1's
  before/after table (three rounds, three sources), then one line per measured
  task (3, 4, 5, 6, 7) with its before/after medians from
  `target\b-measurements.md` and one sentence each on what moved, stating plainly
  where nothing did. In "A 13% regression, found and recovered", "Migrating
  `interpreter/eval.rs` broke that measurably" becomes "Migrating the
  lexical-send helper (`dispatch_lexical_send`) broke that measurably".
- `memory.md`: if Task 5 changed `Object`'s size, update the amplification
  table (`Object` size / 6 is the object factor, `FrameSlot` size / 18 the
  frame factor, their sum the combined figure, release and debug).
- `CLAUDE.md`: the same recomputed figures in the image invariant, only if they
  changed; and in "Working in this repository", nothing else changes.
- `CLAUDE.md`, the handle invariant: "`ns-cli`'s `cmd_disasm` — which builds its
  own class object" becomes "`ns-cli`'s `disasm` command
  (`crates/ns-cli/src/commands/disasm.rs`) — which builds its own class object".
- `compiler.md`: each `cmd_disasm` in "Follow-up: entry points that reach a
  class object only to find its mixin" becomes "`disasm`
  (`crates/ns-cli/src/commands/disasm.rs`)".
- `image-format.md`: "see `root_class_answers` in `crates/ns-cli/src/main.rs`"
  becomes "see `root_class_answers` in `crates/ns-cli/src/commands/build.rs`".
- `README.md` (design map): check each row against the result (no row should
  name `CompiledModule`, `eval.rs` or `main.rs` as holding a command).
- Across the docs: `git grep -n -E "eval\.rs|cmd_[a-z_]+|src/main\.rs|CompiledModule|raise_return_pub" CLAUDE.md docs/design`
  finds nothing.

- [ ] **Step 5: `docs/PROGRESS.md`**

- The M4 row's Plan column gains
  `[clean-up-with-one-engine](superpowers/plans/2026-09-11-2315-ns-next-clean-up-with-one-engine.md)`.
- "Where the next session starts": step B is **done**; the next piece is **C,
  the reference lowering**, whose design is written first
  ([compiler.md](design/compiler.md), "Planned: the reference lowering") and
  has to settle how a recursion-depth limit counts. Keep the 1b paragraph and
  the numbered list; in its item 3, add that a native entry carries an
  enumerated primitive ([dispatch.md](design/dispatch.md)). Update the
  `bench_block.ns` profile paragraph with Task 5's result.
- Add a section after "M4 — Retiring the tree-walker":
  `## M4 — Cleaning up with one engine (YYYY-MM-DD)` with the completion date,
  `Plan: [...]`, stating: the two decisions and their prior-art grounds; what
  changed per task in one or two sentences each; what was deleted
  (`eval.rs`, `raise_return_pub`, `Interpreter::new`, `CompiledModule`,
  `method_key`, `compile_module`, `compile_initializer`, `install_code`,
  `build_module`, three structural tests); Step 1's measurement table and the
  per-task lines; the byte-identity result; test count and both corpus gates.
- Replace the "**Left for B**" list and the two lists after it with one line
  saying B cleared them, except what B's Scope section deliberately left:
  the two milestone-naming error messages (a behaviour change needing its own
  decision), the discarded `Object` allocation in `build_builtin_classes`
  (removing it shifts identity hashes), `ClosureData.params` duplicating
  `code.num_args` (a wire change), and the lexical-send slot lookup (the
  caches'). List those four under the new section as "Left, deliberately".

- [ ] **Step 6: Verify and commit**

Run `cargo test --workspace` (nothing in `docs/` is compiled, but the figures
recorded must be the ones measured), then

```bash
git add CLAUDE.md docs
git commit -m "docs: record step B, its measurements, and the primitive decision

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01CHRhrd9VYFiRq7neF1oDYv"
```

- [ ] **Step 7: A fresh session can start here**

Read `CLAUDE.md`, `docs/design/README.md` and PROGRESS's "Where the next
session starts" as a newcomer would. They must name C as the next step, say
its design is written first, and point at `compiler.md`'s "Planned: the
reference lowering". Nothing may point at `eval.rs`, `CompiledModule`,
`compile_module`, `cmd_*` functions in `main.rs`, or the "Left for B" list.
