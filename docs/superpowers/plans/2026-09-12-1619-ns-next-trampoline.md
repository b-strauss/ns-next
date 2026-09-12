# The Trampoline (1b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sends stop recursing on the Rust stack. The activation stack becomes
the only place a running computation lives, recursion depth becomes an
ordinary Newspeak error under both lowerings, and a send no longer allocates
an argument vector.

**Architecture:** `Interpreter::send` is split into a *decision* — lookup,
`doesNotUnderstand:`, the natives, the nested-class accessor, all staying in
`interpreter/` and `builtins/` — that answers a `Step` instead of running it,
and an *activation* that the VM's loop performs by pushing the step's
activation and switching to it. A return pops and resumes the caller at the
instruction after its send. The control-flow natives become steps too: the
block they would have called is returned as the activation to run, with a
continuation on it for `and:`, `or:` and the loops. Depth has two bounds — L
activations, counted by the rule that exempts exactly what inlining removes,
and N Rust re-entries of the loop — and the CLI runs the interpreter on a
thread whose stack is sized for N.

**Tech Stack:** Rust 1.98.0, edition 2021, no external dependencies. Only
`ns-runtime` and `ns-cli` change; nothing under `crates/ns-syntax`.

**Spec:** [docs/design/execution.md](../../design/execution.md), "Planned: the
trampoline" — all of it, in particular "The trampoline (1b)" with its three
settled points and "Two bounds, not one"; "What stays recursive";
[docs/design/compiler.md](../../design/compiler.md), "How the limit counts"
(including the crafted-artifact condition) and "Until the trampoline";
[docs/design/core-library.md](../../design/core-library.md), "Planned: the
leaf/callback split". Read all three before Task 2.

## Decisions this plan carries

Settled with the user on 2026-09-12 and recorded in the design:

1. **The control-flow natives are trampoline steps.** `value` and the four
   `ifTrue:` forms return the block to activate; `and:`/`or:` return it with a
   continuation that coerces its value; `whileTrue:`/`whileFalse:` return the
   condition with a continuation that activates the body or ends the loop. No
   control-flow native calls back into Newspeak afterwards. `do:` stays a
   callback.
2. **Two bounds.** L = 100,000 activations, counted by compiler.md's rule;
   N = 1,000 re-entries of the run loop from Rust. Error texts
   `stack overflow: recursion too deep` and
   `stack overflow: too many nested native calls` — no number in either.
3. **The `SendInlinable` exemption also requires the block's
   `captured_frame` to be the reified frame of the activation directly below
   it** — the sender — so a crafted artifact cannot recurse uncounted.
4. **The CLI runs every command on a spawned thread** with an explicit stack
   size, measured in Task 6. The whole command, not just the send: the symbol
   interner is thread-local (`symbol.rs`), so every symbol a command interns
   must be interned on the thread that runs it.

## Scope

In: the profile of `bench_block.ns` that `docs/PROGRESS.md` says is owed
before 1b fixes its baseline; `SendInlinable`; the decision/activation split;
the loop; block activations as steps; both bounds; an iterative walk of the
superclass chain in instantiation; caching an activation's register base in
the loop; the before/after measurement; the documents.

Deliberately **not** in this plan:

- Trampolining instantiation, the nested-class accessor or `do:` — they stay
  re-entries, counted against N (execution.md, "What stays recursive").
- Moving `do:` or anything else into Newspeak (after the inline caches).
- Suspending, resuming or serializing a running computation, and re-entering a
  decoded frame (image-format.md, "Planned: suspend and resume").
- Catchable exceptions: both depth errors unwind to the entry send like every
  other Newspeak error today.
- The two error messages that name a milestone, `ClosureData.params`, the
  lexical-send slot lookup — as listed in PROGRESS.md.
- Natives that index `args` without checking its length (`args[0]` in
  `block.rs`/`boolean.rs`): a crafted artifact can reach them today; unchanged
  here.

## Global Constraints

- **`vm.rs` holds no dispatch logic.** Lookup, `doesNotUnderstand:`, native
  dispatch, the nested-class accessor, error text and what a control-flow
  native does after its block returns all live in `interpreter/` or
  `builtins/`. The VM may push, pop, switch, write a register and read an
  instruction's operands — nothing that decides *which* body runs.
- **Program outcomes are pinned, not re-derived.** Every
  `tests-fixtures/*.ns`, every golden in `crates/ns-cli/tests/program_outcomes.rs`
  and every `run-corpus` row keeps its recorded answer under both lowerings.
  Tasks 2–5 and 7 are behaviour-preserving: no recorded answer changes. Task 6
  adds new recorded answers, derived from the rule, never recorded from a run.
- **A wire tag is assigned once.** `OP_SEND_INLINABLE` is 20, the next free
  instruction number; `opcode_tags.rs` pins it.
- **An image is untrusted input.** Nothing added may panic on bytes an artifact
  controls. Panics are allowed only for internal invariants no byte sequence
  can reach (for example "a caller resumed at an instruction that is not a
  send").
- **A running activation is an `ActivationRef`; only a `FrameRef` is ever
  stored.** `Step`, `Continuation` and `WhileLoop` hold closures and values,
  never an `ActivationRef`.
- **Never `{:?}` a `Value`; never `.unwrap()`/`.expect()`/`{:?}` an
  `EvalResult`.** Tests unwrap through a local `expect_ok`.
- **Performance is measured, not assumed**, on a release build, with the
  protocol below, for every task that touches the hot path (Tasks 3, 4, 5, 6,
  7). A task that makes a benchmark slower beyond noise is not done; the
  finding is reported to the controller before anything else happens.
- Gates before every commit: `cargo fmt --all`; `cargo clippy --workspace
  --all-targets` warning-free; `cargo test --workspace`;
  `cargo run -p ns-cli -- parse-corpus corpus/modules` reports
  `parsed 210, failed 0, expected-fail 1, stale 0`;
  `cargo run -p ns-cli -- run-corpus corpus/modules` reports
  `ran 6, failed 0, skipped 205`.
- Conventional commits, each ending with:
  ```
  Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01UjEeSba1SZqspUW2tnKvw3
  ```

## Measurement protocol

Used by every task that says "measure". Before and after are compared **in one
sitting**; a figure from another session is never compared with one from this.

```powershell
# the "before" binary, from the commit the task started from
git worktree add ..\ns-next-before <parent-commit>
cargo build --release -p ns-cli --manifest-path ..\ns-next-before\Cargo.toml
cargo build --release -p ns-cli
$before = "..\ns-next-before\target\release\ns-next.exe"
$after  = ".\target\release\ns-next.exe"
foreach ($round in 1..3) {
  foreach ($src in "bench_loop","bench_send","bench_block") {
    foreach ($bin in $before, $after) {
      & $bin bench "tests-fixtures\$src.ns"   # prints median, min, max
    }
  }
}
git worktree remove ..\ns-next-before
```

Record the three medians per source and binary. A change is "no slower" when,
for every source, the after-group does not lie above the before-group by more
than either group's own spread; "faster" when the after-group lies entirely
below. Report the table in the task's report to the controller; Task 8 carries
the whole-step table into the documents.

---

### Task 1: Profile `bench_block.ns` (controller, with the user)

Not a subagent task: recording a profile on Windows needs an elevated shell,
which only the user can open, and reading one needs a viewer.

**Files:**
- Modify: `docs/design/execution.md` ("What the activation stack moved", the
  `bench_block.ns` paragraph; "What step B moved")
- Modify: `docs/PROGRESS.md` (the "Before 1b fixes its baseline" paragraph)

- [ ] **Step 1: Build a release binary with symbols**

```powershell
$env:CARGO_PROFILE_RELEASE_DEBUG = "true"
cargo build --release -p ns-cli
Remove-Item Env:CARGO_PROFILE_RELEASE_DEBUG
```

- [ ] **Step 2: Record**

Ask the user whether `samply` may be installed (`cargo install --locked
samply`) — it is a developer tool, not a dependency of the workspace. With it,
the user runs, in an elevated terminal from the repository root:

```powershell
samply record --save-only -o bench_block.json.gz target\release\ns-next.exe bench tests-fixtures\bench_block.ns --iterations 200
samply record --save-only -o bench_send.json.gz  target\release\ns-next.exe bench tests-fixtures\bench_send.ns  --iterations 200
samply load bench_block.json.gz
```

If `samply` is refused or cannot record, use Windows Performance Recorder
(`wpr -start CPU` … `wpr -stop bench_block.etl`) and Windows Performance
Analyzer if the user has it. If neither works, stop and record in PROGRESS.md
that the profile could not be taken and why; do not guess an attribution.

- [ ] **Step 3: Read the inverted call tree**

In the viewer, invert the call tree and read the self time of the top fifteen
functions for each profile. What the profile has to explain is the difference
between the two: about 218 ns per iteration more for `bench_block.ns` than for
`bench_send.ns` (PROGRESS.md) — roughly the creation, reification, invocation
and divorce of one block.

- [ ] **Step 4: Record the finding**

Replace "what offsets that saving has not been attributed, since no profile was
taken" in execution.md's `bench_block.ns` paragraph with what the profile
shows — the functions that carry the difference and their share — stating the
build (release with debug info), the tool and the iteration count. Update the
PROGRESS.md paragraph to say the profile was taken and point at the section.
Commit:

```
docs: attribute bench_block's block cost from a profile
```

---

### Task 2: `SendInlinable`

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (variant, `format_instr`)
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (`OP_SEND_INLINABLE = 20`)
- Modify: `crates/ns-runtime/src/image/encode.rs` (`encode_instr`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`decode_instr`, `own_registers`, `jump_target`)
- Modify: `crates/ns-runtime/src/compiler/reference.rs` (`send`, `operands`, `destination`, tests)
- Modify: `crates/ns-runtime/src/vm.rs` (runs it exactly as `SendPublic`)
- Modify: any other exhaustive `match` on `Instr` the compiler reports
- Test: `crates/ns-runtime/tests/opcode_tags.rs`, `crates/ns-runtime/tests/image.rs`,
  `crates/ns-cli/tests/snapshots/*_under_the_reference_lowering.snap`

**Interfaces:**
- Produces: `Instr::SendInlinable { selector: Symbol, receiver_reg: u16, arg_regs: Vec<u16>, dst: u16, call_site: CallSite }` — the shape of `SendPublic`; `image::opcodes::OP_SEND_INLINABLE: u8 = 20`.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/src/compiler/reference.rs`'s test module, add a helper
beside `send`:

```rust
    fn inlinable(selector: &str, receiver_reg: u16, arg_regs: Vec<u16>, dst: u16, id: u16) -> Instr {
        Instr::SendInlinable {
            selector: Symbol::intern(selector),
            receiver_reg,
            arg_regs,
            dst,
            call_site: CallSite { call_site_id: id },
        }
    }
```

and these tests:

```rust
    /// Exactly the sends `control_flow::recognize` accepts are marked, so the
    /// depth count can exempt the block activations the optimized lowering
    /// inlines away (`docs/design/compiler.md`, "How the limit counts").
    #[test]
    fn a_send_the_optimized_lowering_inlines_is_a_send_inlinable() {
        let code = compile(&method("foo: x = ( ^x ifTrue: [1] )"), 0);
        assert!(
            code.instrs
                .iter()
                .any(|i| matches!(i, Instr::SendInlinable { selector, .. } if selector.as_str() == "ifTrue:")),
            "{code:?}"
        );
        assert!(!code.instrs.iter().any(|i| matches!(i, Instr::SendPublic { .. })));
    }

    #[test]
    fn a_send_the_optimized_lowering_does_not_inline_stays_a_send_public() {
        // A non-literal block argument, and a cascade message: neither is
        // offered to `recognize` in the optimized lowering.
        for decl in ["foo: x = ( ^x ifTrue: x )", "foo: x = ( ^x yourself; ifTrue: [1] )"] {
            let code = compile(&method(decl), 0);
            assert!(
                !code.instrs.iter().any(|i| matches!(i, Instr::SendInlinable { .. })),
                "{decl}: {code:?}"
            );
        }
    }

    #[test]
    fn every_inlinable_selector_is_marked() {
        for (decl, selector) in [
            ("foo: x = ( ^x ifTrue: [1] ifFalse: [2] )", "ifTrue:ifFalse:"),
            ("foo: x = ( ^x ifFalse: [1] ifTrue: [2] )", "ifFalse:ifTrue:"),
            ("foo: x = ( ^x ifFalse: [1] )", "ifFalse:"),
            ("foo: x = ( ^x and: [x] )", "and:"),
            ("foo: x = ( ^x or: [x] )", "or:"),
            ("foo: x = ( ^[x] whileTrue: [x] )", "whileTrue:"),
            ("foo: x = ( ^[x] whileFalse: [x] )", "whileFalse:"),
        ] {
            let code = compile(&method(decl), 0);
            assert!(
                code.instrs.iter().any(
                    |i| matches!(i, Instr::SendInlinable { selector: s, .. } if s.as_str() == selector)
                ),
                "{decl}: {code:?}"
            );
        }
    }
```

In `crates/ns-runtime/tests/opcode_tags.rs`: add `assert_eq!(OP_SEND_INLINABLE, 20);`
to `instruction_tags_are_fixed`, and `OP_SEND_INLINABLE` to the `ops` array in
`no_two_tags_share_a_number`.

In `crates/ns-runtime/tests/image.rs`, find the code-table round-trip test that
lists one instruction of every kind (search for the test that round-trips
`Instr::SendPublic` near line 369) and add an `Instr::SendInlinable` with the
same operands beside the `SendPublic` there. Add to the register-check tests,
beside `a_register_in_a_send_argument_list_is_checked_too`:

```rust
#[test]
fn a_register_in_a_send_inlinable_is_checked_too() {
    let table = table_of(vec![code_with(
        CodeKind::Method,
        0,
        1,
        vec![
            Instr::SendInlinable {
                selector: Symbol::intern("ifTrue:"),
                receiver_reg: 0,
                arg_regs: vec![4],
                dst: 0,
                call_site: CallSite { call_site_id: 0 },
            },
            Instr::Return { reg: 0 },
        ],
    )]);
    assert_eq!(
        decode::verify_code_table(&table),
        Err(decode::DecodeError::RegisterOutOfRange {
            reg: 4,
            num_registers: 1
        })
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ns-runtime --lib reference --test opcode_tags --test image`
Expected: compile errors — no variant `SendInlinable`, no `OP_SEND_INLINABLE`.

- [ ] **Step 3: The instruction**

`bytecode.rs`, after `SendPublic`:

```rust
    /// An ordinary explicit-receiver send, exactly `SendPublic`, emitted only
    /// by the reference lowering and only for the sends the optimized lowering
    /// inlines (`control_flow::recognize`). The mark is for the depth count: a
    /// block a builtin control-flow native activates for it is not counted,
    /// because the optimized lowering never activates that block at all
    /// (`docs/design/compiler.md`, "How the limit counts").
    SendInlinable {
        selector: Symbol,
        receiver_reg: u16,
        arg_regs: Vec<u16>,
        dst: u16,
        call_site: CallSite,
    },
```

and in `format_instr`:

```rust
        Instr::SendInlinable {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            call_site,
        } => format!(
            "SendInlinable r{dst} <- r{receiver_reg} #{selector} ({}) @{}",
            regs(arg_regs),
            call_site.call_site_id
        ),
```

`opcodes.rs`: change the comment to "Next free number: 21." and add
`pub const OP_SEND_INLINABLE: u8 = 20;` after `OP_BRANCH_IF_TRUE`.

`encode.rs`, `encode_instr`: an arm identical to `SendPublic`'s but writing
`OP_SEND_INLINABLE`. `decode.rs`: an `OP_SEND_INLINABLE` arm in `decode_instr`
identical to `OP_SEND_PUBLIC`'s but building `Instr::SendInlinable`; in
`own_registers`, extend the `SendPublic` arm's pattern with
`| Instr::SendInlinable { receiver_reg, arg_regs, dst, .. }`; in
`jump_target`, add `| Instr::SendInlinable { .. }` to the `None` arm.

`reference.rs`:

- In `Body::send`, the ordinary-send arm becomes:

```rust
            // An ordinary send. The ones the optimized lowering inlines are
            // marked `SendInlinable`, which only the depth count reads
            // (`docs/design/compiler.md`, "How the limit counts").
            Some(Binding::Local(_) | Binding::OuterLocal { .. }) | None => {
                let receiver_reg = self.value(&send.receiver);
                if control_flow::recognize(send).is_some() {
                    self.inlinable_send(receiver_reg, &send.message)
                } else {
                    self.public_send(receiver_reg, &send.message)
                }
            }
```

  with, beside `public_send`:

```rust
    fn inlinable_send(&mut self, receiver_reg: u16, message: &ast::MessageSend) -> u16 {
        let selector = selector_of(message);
        let arg_regs = self.values(&message.args);
        self.emit_send(|dst, call_site| Instr::SendInlinable {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            call_site,
        })
    }
```

  `control_flow` is `crate::compiler::control_flow`; import it the way
  `lower.rs` does. `cascade` keeps calling `public_send` — the optimized
  lowering never inlines a cascade message.
- In `operands`, extend the `SendPublic` arm's pattern with the same fields of
  `SendInlinable`; in `destination`, add `| Instr::SendInlinable { dst, .. }`.
- Remove the comment in `send` that says the trampoline will emit
  `SendInlinable` here.

`vm.rs`: extend the `Instr::SendPublic { … }` arm's pattern with
`| Instr::SendInlinable { selector, receiver_reg, arg_regs, dst, .. }` so it runs
identically. The depth count is Task 6's.

Fix any other exhaustive `match` on `Instr` the compiler now reports (for
example in `compiler/lower.rs` or `image/inspect.rs`), adding `SendInlinable`
beside `SendPublic` with the same meaning.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ns-runtime`
Expected: PASS.

- [ ] **Step 5: Update the two reference-lowering disasm snapshots**

```powershell
$env:INSTA_UPDATE = "always"
cargo test -p ns-cli --test disasm_snapshots
Remove-Item Env:INSTA_UPDATE
git diff --stat crates/ns-cli/tests/snapshots
git diff crates/ns-cli/tests/snapshots
```

Expected: exactly two files change, the two `…_under_the_reference_lowering.snap`,
and in them only lines whose selector is one of the eight inlinable selectors
change, from `SendPublic` to `SendInlinable` — `#whileTrue:` and
`#whileFalse:` in the loop snapshot, `#ifTrue:ifFalse:` twice in the fallback
snapshot. Any other line changing is a defect. No optimized-lowering snapshot
may change.

- [ ] **Step 6: Gates and commit**

Run the whole gate list. Expected: green, parse-corpus and run-corpus unchanged.

```
feat: mark the reference lowering's inlinable sends as SendInlinable

The depth count's rule exempts the block activations a control-flow
native performs for exactly the sends the optimized lowering inlines,
so the reference lowering has to say which sends those are. SendInlinable
is SendPublic with the mark, wire tag 20; the VM runs it identically
until the count arrives.
```

---

### Task 3: Decide before activating

A behaviour-preserving restructuring: every send first asks for a `Step`, and
only then runs it. Execution is still recursive — the loop is Task 4.

**Files:**
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Step`, `StepArgs`,
  `decide_send`, `decide_native_or_dnu`, `decide_does_not_understand`,
  `run_step`, `check_method_arity`; `send` rebuilt on them;
  `invoke_method` replaced by `run_step`)
- Modify: `crates/ns-runtime/src/interpreter/sends.rs` (`decide_lexical_send`,
  `decide_super_send`)
- Modify: `crates/ns-runtime/src/builtins/mod.rs` (`dispatch_native` answers
  `Option<Result<Step, Signal>>`, `pub(crate)`)
- Modify: `crates/ns-runtime/src/builtins/*.rs` (every `pub fn dispatch…`
  becomes `pub(crate)`; bodies unchanged)
- Modify: `crates/ns-runtime/src/vm.rs` (the three send arms)
- Test: `crates/ns-runtime/src/interpreter/mod.rs` (new `#[cfg(test)]` module)

**Interfaces:**
- Produces (all `pub(crate)`, in `interpreter`):
  ```rust
  pub(crate) enum Step {
      Value(Value),
      Method { receiver: Value, method: Rc<MixinMethod>, defining_class: ClassRef, args: StepArgs },
  }
  pub(crate) enum StepArgs { Send, Owned(Vec<Value>) }
  impl StepArgs { pub(crate) fn slice<'a>(&'a self, send: &'a [Value]) -> &'a [Value] }
  impl Interpreter {
      pub(crate) fn decide_send(&mut self, receiver: Value, selector: Symbol, args: &[Value]) -> Result<Step, Signal>;
      pub(crate) fn run_step(&mut self, step: Step, send_args: &[Value]) -> EvalResult;
  }
  pub(crate) fn sends::decide_lexical_send(interp: &mut Interpreter, act: ActivationRef, depth: u16, selector: Symbol, args: &[Value]) -> Result<Step, Signal>;
  pub(crate) fn sends::decide_super_send(interp: &mut Interpreter, act: ActivationRef, selector: Symbol, args: &[Value]) -> Result<Step, Signal>;
  pub(crate) fn builtins::dispatch_native(interp: &mut Interpreter, kind: BuiltinClass, receiver: Value, selector: Symbol, args: &[Value]) -> Option<Result<Step, Signal>>;
  ```

- [ ] **Step 1: Write the failing test**

At the end of `crates/ns-runtime/src/interpreter/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn load(src: &str) -> (Interpreter, ClassRef) {
        let module = ns_syntax::parse(src).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(src),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interp = Interpreter::bootstrap();
        let class = interp.load_module(&resolved);
        (interp, class)
    }

    fn expect_ok(result: EvalResult, what: &str) -> Value {
        match result {
            Ok(v) => v,
            Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
            Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
        }
    }

    /// Deciding a send runs nothing: a user method comes back as the
    /// activation to make, with nothing pushed, and a native leaf as its
    /// value (`docs/design/execution.md`, "The trampoline (1b)").
    #[test]
    fn deciding_a_send_answers_what_to_run_without_running_it() {
        let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  public foo = ( ^1 )\n) : ()";
        let (mut interp, class) = load(src);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        match interp.decide_send(instance, Symbol::intern("foo"), &[]) {
            Ok(Step::Method { method, .. }) => assert_eq!(method.selector.as_str(), "foo"),
            _ => panic!("a user method is decided as an activation"),
        }
        assert_eq!(interp.heap.activation_depth(), 0, "nothing was pushed");
        match interp.decide_send(Value::SmallInt(3), Symbol::intern("+"), &[Value::SmallInt(4)]) {
            Ok(Step::Value(Value::SmallInt(7))) => {}
            _ => panic!("a native leaf is decided as its value"),
        }
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --lib interpreter::tests`
Expected: compile error — no `decide_send`, no `Step`.

- [ ] **Step 3: `Step` and the decisions**

In `interpreter/mod.rs`, after `EvalResult`:

```rust
/// What a send decided, before anything runs (`docs/design/execution.md`,
/// "The trampoline (1b)"). Deciding is `interpreter/`'s -- lookup,
/// `doesNotUnderstand:`, the natives, the nested-class accessor -- and running
/// what was decided is the VM's, which is how `vm.rs` stays free of dispatch
/// logic once sends stop recursing.
pub(crate) enum Step {
    /// The send's value is already known: a native leaf, a slot accessor, a
    /// nested class, a finished instantiation.
    Value(Value),
    /// Activate `method` on `receiver`; `defining_class` becomes the
    /// activation's current class (§3.6).
    Method {
        receiver: Value,
        method: Rc<crate::mixin::MixinMethod>,
        defining_class: ClassRef,
        args: StepArgs,
    },
}

/// Where an activation's arguments come from.
pub(crate) enum StepArgs {
    /// The send's own arguments.
    Send,
    /// Arguments the decision built: `doesNotUnderstand:`'s message.
    Owned(Vec<Value>),
}

impl StepArgs {
    pub(crate) fn slice<'a>(&'a self, send: &'a [Value]) -> &'a [Value] {
        match self {
            StepArgs::Send => send,
            StepArgs::Owned(args) => args,
        }
    }
}
```

Replace `dispatch_send` with `decide_send`, `dispatch_native_or_dnu` with
`decide_native_or_dnu` and `does_not_understand` with
`decide_does_not_understand`. They keep their doc comments, their order of
cases and every error text; what changes is that each arm answers a `Step`
instead of running it:

```rust
    pub(crate) fn decide_send(
        &mut self,
        receiver: Value,
        selector: Symbol,
        args: &[Value],
    ) -> Result<Step, Signal> {
        if let Some(result) = self.try_factory_send(&receiver, selector, args) {
            return result.map(Step::Value);
        }
        if let Value::Obj(obj) = &receiver {
            let is_class = self.heap.is_class(*obj);
            if is_class {
                if let Some(metaclass) = self.metaclass_of(*obj) {
                    match dispatch::lookup_public(&self.heap, metaclass, selector) {
                        Lookup::Method(method, defining_class) => {
                            return Ok(Step::Method {
                                receiver,
                                method,
                                defining_class,
                                args: StepArgs::Send,
                            });
                        }
                        Lookup::NestedClass(mixin, declaring_class) => {
                            return nested::nested_class(self, &receiver, &mixin, declaring_class)
                                .map(Step::Value);
                        }
                        Lookup::Native(kind) => {
                            return self.decide_native_or_dnu(kind, receiver, selector, args);
                        }
                        Lookup::NotFound => {}
                    }
                }
            }
        }
        let class = self.class_of(&receiver);
        match dispatch::lookup_public(&self.heap, class, selector) {
            Lookup::Method(method, defining_class) => Ok(Step::Method {
                receiver,
                method,
                defining_class,
                args: StepArgs::Send,
            }),
            Lookup::NestedClass(mixin, declaring_class) => {
                nested::nested_class(self, &receiver, &mixin, declaring_class).map(Step::Value)
            }
            Lookup::Native(kind) => self.decide_native_or_dnu(kind, receiver, selector, args),
            Lookup::NotFound => self.decide_does_not_understand(receiver, selector, args),
        }
    }
```

`dispatch_native_inheriting_object` keeps its shape and answers
`Option<Result<Step, Signal>>`. `decide_native_or_dnu(kind, receiver, selector,
args: &[Value]) -> Result<Step, Signal>` answers the native's step or falls to
`decide_does_not_understand`. In `decide_does_not_understand(receiver,
selector, args: &[Value])`, the user-override arm becomes

```rust
            Lookup::Method(method, defining_class) => {
                let selector_value = self.new_symbol(selector);
                let args_value = Value::Obj(self.new_array(args.to_vec()));
                let message_arg = Value::Obj(self.new_array(vec![selector_value, args_value]));
                Ok(Step::Method {
                    receiver,
                    method,
                    defining_class,
                    args: StepArgs::Owned(vec![message_arg]),
                })
            }
```

and the native arm returns the native's `Result<Step, Signal>`; the two error
arms are unchanged apart from being `Err(…)` of a `Result<Step, Signal>`.

Move `invoke_method`'s arity check into

```rust
/// Refuses an activation given a different number of arguments than its
/// method takes -- see the reasoning `run_step` used to carry, kept here.
fn check_method_arity(
    method: &crate::mixin::MixinMethod,
    receiver: Value,
    given: usize,
) -> Result<(), Signal> {
    if given == method.num_args {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError {
        selector: method.selector,
        receiver,
        message: format!(
            "wrong number of arguments: `{}` takes {}, got {}",
            method.selector, method.num_args, given
        ),
    }))
}
```

(copy `invoke_method`'s long comment about wrong-arity activations onto it),
and replace `invoke_method` with the step runner it becomes:

```rust
    /// Runs what `decide_send` (or a lexical or `super` decision) answered.
    /// A value is the send's result; a method activation is pushed, run and
    /// popped on every path out, and a `^` aimed at it ends there.
    pub(crate) fn run_step(&mut self, step: Step, send_args: &[Value]) -> EvalResult {
        match step {
            Step::Value(value) => Ok(value),
            Step::Method {
                receiver,
                method,
                defining_class,
                args,
            } => {
                let args = args.slice(send_args);
                check_method_arity(&method, receiver, args.len())?;
                let code = method.code.clone();
                let act = self.heap.push_activation(
                    ActivationHeader {
                        receiver,
                        current_class: Some(defining_class),
                        kind: FrameKind::Method(method),
                        slot_base: 0, // a method body never contains InitSlot
                    },
                    args,
                    (code.num_registers - code.num_args) as usize,
                );
                let result = crate::vm::run_code(self, act, &code);
                self.heap.pop_activation(act);
                match result {
                    Ok(value) => Ok(value),
                    Err(Signal::Return(target, value)) if target.same(act) => Ok(value),
                    Err(other) => Err(other),
                }
            }
        }
    }
```

`send` becomes decide-then-run, keeping its stack-balance assertion:

```rust
    pub fn send(&mut self, receiver: Value, selector: Symbol, args: Vec<Value>) -> EvalResult {
        let depth = self.heap.activation_depth();
        let result = match self.decide_send(receiver, selector, &args) {
            Ok(step) => self.run_step(step, &args),
            Err(signal) => Err(signal),
        };
        debug_assert_eq!(
            self.heap.activation_depth(),
            depth,
            "a send must leave the activation stack as it found it"
        );
        result
    }
```

In `sends.rs`, rename to `decide_lexical_send`/`decide_super_send`, take
`args: &[Value]`, and return `Result<Step, Signal>`: a slot setter answers
`Ok(Step::Value(o))`, a getter `Ok(Step::Value(value))`, a method
`Ok(Step::Method { receiver: o /* or self_value */, method, defining_class, args: StepArgs::Send })`,
a nested class `nested::nested_class(…).map(Step::Value)`, a native
`dispatch_native(…)` or — when it answers `None` — `interp.decide_send(o, selector, args)`,
and `NotFound` `interp.decide_send(…)`. `args.first().copied()` stays.

In `builtins/mod.rs`, `dispatch_native` answers `Option<Result<Step, Signal>>`
and becomes `pub(crate)`: every arm maps its native's result through

```rust
/// A native's value as the step it is.
fn value_step(result: EvalResult) -> Result<Step, Signal> {
    result.map(Step::Value)
}
```

and `wrap_error` now answers `Result<Step, Signal>`
(`result.map(Step::Value).map_err(Signal::Error)`). The natives' own bodies do
not change in this task. Make every `pub fn dispatch…` under `builtins/`
`pub(crate)` — nothing outside the crate calls them, and a `pub` function
returning the crate-private `Step` would trip `private_interfaces`.

In `vm.rs`, each send arm decides, then runs:

```rust
            Instr::SendPublic { selector, receiver_reg, arg_regs, dst, .. }
            | Instr::SendInlinable { selector, receiver_reg, arg_regs, dst, .. } => {
                let receiver = read_local(&interp.heap, act, *receiver_reg);
                let args = collect(&interp.heap, act, arg_regs);
                interp.heap.set_activation_pc(act, pc);
                let step = interp.decide_send(receiver, *selector, &args)?;
                let result = interp.run_step(step, &args)?;
                write_local(&mut interp.heap, act, *dst, result);
            }
```

and likewise `SendLexical` (`decide_lexical_send(interp, act, *depth, *selector, &args)?`)
and `SendSuper` (`decide_super_send(interp, act, *selector, &args)?`). Update
`vm.rs`'s module doc to name the `decide_*` functions and `run_step`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ns-runtime --lib interpreter::tests`, then the whole gate list.
Expected: PASS; every recorded answer unchanged.

- [ ] **Step 5: Measure**

Follow the measurement protocol against this task's parent commit. Expected:
no slower on any source. If slower, report before committing.

- [ ] **Step 6: Commit**

```
refactor: decide a send before running it

Every send asks interpreter/ for a Step -- a value, or the method to
activate -- and only then runs it. Lookup, doesNotUnderstand:, the
natives and the nested-class accessor stay where they were; what moved
is the act of activating, which the trampoline will take into the VM's
loop. Execution is still recursive.
```

---

### Task 4: The loop

`run_code`'s recursion through `run_step` becomes a loop that pushes and pops.
Natives that call a block still re-enter it through `invoke_closure` — Task 5
removes that for everything but `do:`.

The design says arguments are copied "straight from the caller's argument
registers into the callee's window". They pass through one reusable buffer on
the way instead, because the decision needs them as a slice before any window
exists — a native reads them, `doesNotUnderstand:` wraps them — and the buffer
keeps its capacity, so no send allocates. Task 8 describes it that way.

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`ActivationHeader.code`,
  `StackEntry.code`; `activation_code`, `activation_pc`, `top_activation`,
  `activation_position`, `activation_kind`; delete `grow_activation_temps`;
  the unit tests' `header()` helper)
- Modify: `crates/ns-runtime/src/vm.rs` (rewritten: `run`, `execute`, `send`,
  `finish`, `resume_caller`, `unwind`, `send_destination`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`push_step`,
  `reenter`, `catches_return`, the argument buffer; `run_step` deleted;
  `send` and `invoke_closure` through `reenter`)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs`
  (`run_initializer_level` pushes with its register count and goes through
  `reenter`; `run_own_initializer` folded into it)
- Modify: `crates/ns-runtime/src/interpreter/nested.rs` (`run_clause_body`
  through `reenter`)
- Modify: `crates/ns-runtime/tests/image.rs:4635` (the header gains `code`)
- Test: `crates/ns-runtime/tests/trampoline.rs` (new)

**Interfaces:**
- Consumes: Task 3's `Step`, `StepArgs`, `decide_*`, `check_method_arity`.
- Produces:
  ```rust
  // heap.rs
  pub struct ActivationHeader { pub receiver, pub current_class, pub kind, pub slot_base, pub code: Rc<CompiledCode> }
  impl Heap {
      pub fn activation_code(&self, act: ActivationRef) -> Rc<CompiledCode>;
      pub fn activation_pc(&self, act: ActivationRef) -> usize;
      pub fn top_activation(&self) -> Option<ActivationRef>;
      pub fn activation_position(&self, act: ActivationRef) -> usize; // how many activations are below it
      pub fn activation_kind(&self, act: ActivationRef) -> FrameKind;
  }
  // vm.rs
  pub fn run(interp: &mut Interpreter, base: ActivationRef) -> EvalResult;
  // interpreter/mod.rs
  impl Interpreter {
      pub(crate) fn push_step(&mut self, step: Step, send_args: &[Value]) -> Result<ActivationRef, Signal>;
      pub(crate) fn reenter(&mut self, push: impl FnOnce(&mut Interpreter) -> Result<ActivationRef, Signal>) -> EvalResult;
      pub(crate) fn catches_return(&self, act: ActivationRef) -> bool;
      pub(crate) fn take_arg_buffer(&mut self) -> Vec<Value>;
      pub(crate) fn return_arg_buffer(&mut self, buffer: Vec<Value>);
  }
  ```
  `vm::run_code` is deleted; `vm::run` replaces it with a different contract:
  it pops what it runs.

- [ ] **Step 1: Write the failing test**

Create `crates/ns-runtime/tests/trampoline.rs`:

```rust
//! The trampoline (`docs/design/execution.md`, "The trampoline (1b)"): sends
//! run on the activation stack, not the Rust stack, so recursion depth is no
//! longer bounded by the native stack -- and, once the bounds arrive, is an
//! ordinary Newspeak error under both lowerings.

use std::path::PathBuf;
use std::rc::Rc;

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

fn small_int(v: Value) -> i64 {
    match v {
        Value::SmallInt(n) => n,
        _ => panic!("expected a SmallInteger"),
    }
}

const DEPTH: &str = "Newspeak3\n'T'\nclass Depth = Object new (\n) (\n\
    \tpublic down: n = ( n = 0 ifTrue: [^0]. ^down: n - 1 )\n\
    \tpublic logic: n = ( ^n = 0 or: [(logic: n - 1) and: [true]] )\n\
    \tpublic loop: n = ( [n > 0] whileTrue: [^loop: n - 1]. ^0 )\n\
    ) : ()";

/// Fifty thousand nested method activations: far past what the Rust stack of
/// a test thread holds when every send recursed on it, and nothing but
/// activation-stack entries once none does. Under the optimized lowering no
/// native re-enters Newspeak here, since every control-flow send is inlined.
#[test]
fn a_deep_method_recursion_runs_on_the_activation_stack() {
    let (mut interp, class) = load(DEPTH, Lowering::Optimized);
    let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    for selector in ["down:", "logic:", "loop:"] {
        let result = expect_ok(
            interp.send(instance, Symbol::intern(selector), vec![Value::SmallInt(50_000)]),
            selector,
        );
        assert!(
            matches!(result, Value::SmallInt(0) | Value::Bool(true)),
            "{selector}"
        );
        assert_eq!(interp.heap_ref().activation_depth(), 0, "{selector}: nothing left behind");
    }
}

/// A `^` out of a method several activations up, through a block, lands on the
/// method it names and leaves nothing behind it on the stack.
#[test]
fn a_non_local_return_unwinds_to_its_home_on_the_activation_stack() {
    let src = "Newspeak3\n'T'\nclass R = Object new (\n) (\n\
        \tpublic find = ( self each: [:e | e = 3 ifTrue: [^e * 10]]. ^0 )\n\
        \teach: blk = ( self to: 5 do: blk )\n\
        \tto: hi do: blk = ( | i | i := 1. [i <= hi] whileTrue: [blk value: i. i := i + 1] )\n\
        ) : ()";
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(src, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        let result = expect_ok(interp.send(instance, Symbol::intern("find"), vec![]), "find");
        assert_eq!(small_int(result), 30, "{lowering:?}");
        assert_eq!(interp.heap_ref().activation_depth(), 0, "{lowering:?}");
    }
}
```

`to:do:` here is a user method on the class, sent to `self` — `Integer` has no
`to:do:` native — so the `^` crosses three ordinary method activations, a
`whileTrue:` body (inlined under the optimized lowering, an activation under
the reference one) and the block itself on its way home.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ns-runtime --test trampoline`
Expected: `a_deep_method_recursion_runs_on_the_activation_stack` aborts with a
stack overflow on the test thread (every send still recurses); the other test
passes. An abort is the expected failure here — note it and continue.

- [ ] **Step 3: The heap**

In `heap.rs`: add to `ActivationHeader`

```rust
    /// The body the activation runs. The run loop reads it back to resume a
    /// caller once the activation above it returns.
    pub code: Rc<crate::bytecode::CompiledCode>,
```

and a `code: Rc<CompiledCode>` field to `StackEntry`, filled from the header in
`push_activation`. Add, beside the other `activation_*` accessors:

```rust
    /// The body `act` runs.
    pub fn activation_code(&self, act: ActivationRef) -> Rc<crate::bytecode::CompiledCode> {
        self.check_activation(act);
        self.stack.entries[act.index as usize].code.clone()
    }

    /// The `pc` last stored for `act` -- at a send, the send's own index, which
    /// is where the run loop resumes it from when its callee returns.
    pub fn activation_pc(&self, act: ActivationRef) -> usize {
        self.check_activation(act);
        self.stack.entries[act.index as usize].pc
    }

    /// The innermost live activation, if any.
    pub fn top_activation(&self) -> Option<ActivationRef> {
        let len = self.stack.entries.len();
        (len > 0).then(|| self.activation_handle((len - 1) as u32))
    }

    /// How many activations are below `act` -- its depth, counted from the
    /// bottom of the stack. The run loop compares a non-local return's target
    /// against the depth it started at to tell its own activations from its
    /// callers'.
    pub fn activation_position(&self, act: ActivationRef) -> usize {
        self.check_activation(act);
        act.index as usize
    }

    /// What `act` was activated for.
    pub fn activation_kind(&self, act: ActivationRef) -> FrameKind {
        self.check_activation(act);
        self.stack.entries[act.index as usize].kind.clone()
    }
```

Delete `grow_activation_temps` — its one caller goes in Step 5. In the heap's
unit tests, give the `header()` helper a trivial body:

```rust
        code: Rc::new(crate::bytecode::CompiledCode {
            kind: crate::bytecode::CodeKind::Method,
            instrs: vec![crate::bytecode::Instr::Return { reg: 0 }],
            num_args: 0,
            num_registers: 1,
            statement_pc: Vec::new(),
            call_sites: Vec::new(),
            source: ns_syntax::Span::default(),
        }),
```

(check `CompiledCode`'s fields and whether `Span` has a `Default`; if not,
construct it the way other tests in the crate do). Do the same in
`crates/ns-runtime/tests/image.rs`'s
`a_snapshot_taken_while_an_activation_is_running_is_refused`, using that file's
`code_with` helper.

- [ ] **Step 4: The loop**

Replace `crates/ns-runtime/src/vm.rs` with:

```rust
//! The register VM's run loop -- and nothing else. `run` executes one
//! activation and everything it activates, on the `Heap`'s activation stack,
//! without recursing on the Rust stack: a send asks `interpreter/` what it
//! means (`Interpreter::decide_send`, `sends::decide_lexical_send`,
//! `sends::decide_super_send`) and, when the answer is an activation, pushes
//! it and runs it here; a return pops it and resumes the caller. No dispatch
//! logic, no `doesNotUnderstand:`, no native dispatch and no error text of its
//! own live here -- dispatch lives in one place, which the reference lowering
//! and a JIT share. See `docs/design/execution.md`, "The VM".

use std::rc::Rc;

use crate::bytecode::{CompiledCode, Instr, Literal};
use crate::heap::{ActivationRef, Heap};
use crate::interpreter::literals::{
    malformed_int_error, parse_int_literal, unresolved_identifier_error,
};
use crate::interpreter::locals::{
    home_of, read_local, read_outer_local, store_instance_field, write_local, write_outer_local,
};
use crate::interpreter::sends::{decide_lexical_send, decide_super_send};
use crate::interpreter::{EvalResult, Interpreter, Signal, Step};
use crate::types::*;

/// What the loop does after one instruction.
enum Flow {
    Next,
    Jump(usize),
    /// Run another activation from `pc`: a callee a send pushed (from 0), or
    /// a caller a return resumed (from the instruction after its send).
    Switch {
        act: ActivationRef,
        code: Rc<CompiledCode>,
        pc: usize,
    },
    /// The activation `run` was given is done, and so is everything it
    /// activated.
    Exit(EvalResult),
}

/// Runs `base` -- the top of the activation stack, pushed by the caller --
/// and every activation it pushes, until `base` is done, and pops them all,
/// `base` included, on every path out: a return, an error, or a non-local
/// return aimed below `base`. The stack is exactly as deep afterwards as it
/// was before `base` was pushed.
///
/// An activation's `pc` is stored only by the instructions that can hand it
/// to an observer: every send (a callee runs above it, and the loop resumes
/// it from that `pc`), `MakeClosure` (it reifies the activation, so the frame
/// starts from the `pc` stored just before), and `NonLocalReturn` (control
/// leaves with a signal). Nothing reads `pc` while a straight-line
/// instruction executes.
pub fn run(interp: &mut Interpreter, base: ActivationRef) -> EvalResult {
    let floor = interp.heap.activation_position(base);
    let mut act = base;
    let mut code = interp.heap.activation_code(act);
    let mut pc = 0usize;
    loop {
        match execute(interp, act, &code, pc, floor) {
            Flow::Next => {
                pc += 1;
                debug_assert!(
                    pc < code.instrs.len(),
                    "internal: pc ran past the end of a CompiledCode with no terminating return"
                );
            }
            Flow::Jump(target) => pc = target,
            Flow::Switch {
                act: next,
                code: next_code,
                pc: next_pc,
            } => {
                act = next;
                code = next_code;
                pc = next_pc;
            }
            Flow::Exit(result) => return result,
        }
    }
}

/// One instruction of `code`, running in `act`.
#[inline(always)]
fn execute(
    interp: &mut Interpreter,
    act: ActivationRef,
    code: &CompiledCode,
    pc: usize,
    floor: usize,
) -> Flow {
    match &code.instrs[pc] {
        Instr::LoadLiteral { dst, literal } => match make_value(interp, act, literal) {
            Ok(v) => {
                write_local(&mut interp.heap, act, *dst, v);
                Flow::Next
            }
            Err(signal) => unwind(interp, signal, floor),
        },
        Instr::LoadSelf { dst } => {
            let v = interp.heap.activation_receiver(act);
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::LoadBuiltinClass { dst, kind } => {
            let v = Value::Obj(interp.builtin_class(*kind));
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::Move { dst, src } | Instr::StoreLocal { src, slot: dst } => {
            let v = read_local(&interp.heap, act, *src);
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::LoadOuterLocal { dst, depth, index } => {
            let v = read_outer_local(&interp.heap, act, *depth, *index);
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::StoreOuterLocal { src, depth, index } => {
            let v = read_local(&interp.heap, act, *src);
            write_outer_local(&mut interp.heap, act, *depth, *index, v);
            Flow::Next
        }
        Instr::SendPublic {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            // `call_site` is the reserved, inert inline-cache slot; nothing
            // reads or writes it yet.
            ..
        }
        | Instr::SendInlinable {
            selector,
            receiver_reg,
            arg_regs,
            dst,
            ..
        } => {
            let receiver = read_local(&interp.heap, act, *receiver_reg);
            send(interp, act, pc, arg_regs, *dst, floor, |interp, args| {
                interp.decide_send(receiver, *selector, args)
            })
        }
        Instr::SendLexical {
            depth,
            selector,
            arg_regs,
            dst,
            ..
        } => send(interp, act, pc, arg_regs, *dst, floor, |interp, args| {
            decide_lexical_send(interp, act, *depth, *selector, args)
        }),
        Instr::SendSuper {
            selector,
            arg_regs,
            dst,
            ..
        } => send(interp, act, pc, arg_regs, *dst, floor, |interp, args| {
            decide_super_send(interp, act, *selector, args)
        }),
        Instr::MakeClosure {
            dst,
            params,
            code: block,
        } => {
            // Stored before reifying, so the frame `reify` builds starts
            // from this pc.
            interp.heap.set_activation_pc(act, pc);
            let captured_frame = interp.heap.reify(act);
            let home = home_of(&mut interp.heap, act);
            let closure = Rc::new(ClosureData {
                params: params.clone(),
                code: block.clone(),
                captured_frame,
                home,
            });
            let v = Value::Obj(interp.new_closure(closure));
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::MakeArray { dst, elem_regs } => {
            let values = elem_regs
                .iter()
                .map(|r| read_local(&interp.heap, act, *r))
                .collect();
            let v = Value::Obj(interp.new_array(values));
            write_local(&mut interp.heap, act, *dst, v);
            Flow::Next
        }
        Instr::InitSlot { src, own_index } => {
            // `own_index` is relative to the executing mixin's own slots; the
            // activation's `slot_base`, set when this level's activation was
            // pushed, turns it into this instance's absolute field index.
            let v = read_local(&interp.heap, act, *src);
            let receiver = interp.heap.activation_receiver(act);
            let base = interp.heap.activation_slot_base(act);
            store_instance_field(&mut interp.heap, &receiver, base + *own_index as usize, v);
            Flow::Next
        }
        Instr::Return { reg } | Instr::ImplicitReturn { reg } => {
            let value = read_local(&interp.heap, act, *reg);
            finish(interp, act, value, floor)
        }
        Instr::NonLocalReturn { reg } => {
            let value = read_local(&interp.heap, act, *reg);
            interp.heap.set_activation_pc(act, pc);
            let signal = interp.raise_return(act, value);
            unwind(interp, signal, floor)
        }
        Instr::Jump { target } => Flow::Jump(*target as usize),
        Instr::JumpIfNotBoolean { reg, target } => {
            if matches!(read_local(&interp.heap, act, *reg), Value::Bool(_)) {
                Flow::Next
            } else {
                Flow::Jump(*target as usize)
            }
        }
        Instr::BranchIfFalse { reg, target } => {
            if matches!(read_local(&interp.heap, act, *reg), Value::Bool(false)) {
                Flow::Jump(*target as usize)
            } else {
                Flow::Next
            }
        }
        Instr::BranchIfTrue { reg, target } => {
            if matches!(read_local(&interp.heap, act, *reg), Value::Bool(true)) {
                Flow::Jump(*target as usize)
            } else {
                Flow::Next
            }
        }
    }
}

/// A send: the arguments gathered from the caller's registers into the
/// interpreter's reusable buffer, `pc` stored, the decision asked for, and
/// what was decided run -- a value written to `dst`, or an activation pushed
/// and switched to, its window filled straight from the buffer. No vector is
/// allocated once the buffer has grown to the widest send.
fn send(
    interp: &mut Interpreter,
    act: ActivationRef,
    pc: usize,
    arg_regs: &[u16],
    dst: u16,
    floor: usize,
    decide: impl FnOnce(&mut Interpreter, &[Value]) -> Result<Step, Signal>,
) -> Flow {
    let mut args = interp.take_arg_buffer();
    args.extend(arg_regs.iter().map(|r| read_local(&interp.heap, act, *r)));
    interp.heap.set_activation_pc(act, pc);
    let flow = match decide(interp, &args) {
        Ok(Step::Value(v)) => {
            write_local(&mut interp.heap, act, dst, v);
            Flow::Next
        }
        Ok(step) => match interp.push_step(step, &args) {
            Ok(callee) => Flow::Switch {
                act: callee,
                code: interp.heap.activation_code(callee),
                pc: 0,
            },
            Err(signal) => unwind(interp, signal, floor),
        },
        Err(signal) => unwind(interp, signal, floor),
    };
    interp.return_arg_buffer(args);
    flow
}

/// `act` has returned `value`: pop it and hand the value on -- to the send in
/// its caller, which resumes, or out of `run` when `act` was the activation
/// `run` was given.
fn finish(interp: &mut Interpreter, act: ActivationRef, value: Value, floor: usize) -> Flow {
    interp.heap.pop_activation(act);
    if interp.heap.activation_depth() == floor {
        return Flow::Exit(Ok(value));
    }
    let caller = interp
        .heap
        .top_activation()
        .expect("internal: an activation above the run's floor has a caller");
    resume_caller(interp, caller, value)
}

/// Writes a callee's value into the send `caller` is stopped at, and resumes
/// `caller` after it.
fn resume_caller(interp: &mut Interpreter, caller: ActivationRef, value: Value) -> Flow {
    let code = interp.heap.activation_code(caller);
    let pc = interp.heap.activation_pc(caller);
    write_local(&mut interp.heap, caller, send_destination(&code.instrs[pc]), value);
    Flow::Switch {
        act: caller,
        code,
        pc: pc + 1,
    }
}

/// A signal leaving the running activation. A non-local return whose target
/// is one of this run's activations and catches a return (a method, or a
/// superclass clause or its message -- `Interpreter::catches_return`) ends
/// there: everything above the target is popped and the target returns the
/// value. Anything else pops every activation of this run, `base` included,
/// and leaves `run` with the signal, for the Rust frame that re-entered the
/// loop to pass on.
fn unwind(interp: &mut Interpreter, signal: Signal, floor: usize) -> Flow {
    if let Signal::Return(target, value) = &signal {
        let (target, value) = (*target, *value);
        if interp.heap.activation_position(target) >= floor && interp.catches_return(target) {
            while let Some(top) = interp.heap.top_activation() {
                if top.same(target) {
                    break;
                }
                interp.heap.pop_activation(top);
            }
            return finish(interp, target, value, floor);
        }
    }
    while interp.heap.activation_depth() > floor {
        let top = interp
            .heap
            .top_activation()
            .expect("internal: a stack deeper than the floor has a top");
        interp.heap.pop_activation(top);
    }
    Flow::Exit(Err(signal))
}

/// The register a send writes its value to: where a callee's value goes when
/// the loop resumes the caller.
fn send_destination(instr: &Instr) -> u16 {
    match instr {
        Instr::SendPublic { dst, .. }
        | Instr::SendInlinable { dst, .. }
        | Instr::SendLexical { dst, .. }
        | Instr::SendSuper { dst, .. } => *dst,
        other => unreachable!("internal: a caller resumed at an instruction that is not a send: {other:?}"),
    }
}
```

and keep `make_value` unchanged at the end of the file. `collect` goes: the
sends use the buffer and `MakeArray` collects inline. The `Heap` import stays
only if something still names it; remove it otherwise.

A `Move` and a `StoreLocal` are the same operation on the stack, so they share
an arm; if `rustfmt` or clippy objects to the or-pattern binding `slot: dst`,
write them as two arms.

- [ ] **Step 5: The interpreter**

In `interpreter/mod.rs`:

Add a field `arg_buffer: Vec<Value>` to `Interpreter` (initialised
`Vec::new()` in `bootstrap_with`), documented as "The reusable argument buffer
the run loop gathers a send's arguments into (`vm::send`). Taken while a
decision runs and given back afterwards, so a decision that re-enters the loop
takes a fresh one instead of clobbering its caller's.", and:

```rust
    pub(crate) fn take_arg_buffer(&mut self) -> Vec<Value> {
        let mut buffer = std::mem::take(&mut self.arg_buffer);
        buffer.clear();
        buffer
    }

    pub(crate) fn return_arg_buffer(&mut self, buffer: Vec<Value>) {
        self.arg_buffer = buffer;
    }
```

Replace `run_step` with the two halves the loop needs:

```rust
    /// Pushes the activation `step` describes. `send_args` are the arguments
    /// of the send that decided it, which `StepArgs::Send` refers to.
    pub(crate) fn push_step(
        &mut self,
        step: Step,
        send_args: &[Value],
    ) -> Result<ActivationRef, Signal> {
        match step {
            Step::Value(_) => unreachable!("internal: a value needs no activation"),
            Step::Method {
                receiver,
                method,
                defining_class,
                args,
            } => {
                let args = args.slice(send_args);
                check_method_arity(&method, receiver, args.len())?;
                let code = method.code.clone();
                let temps = (code.num_registers - code.num_args) as usize;
                Ok(self.heap.push_activation(
                    ActivationHeader {
                        receiver,
                        current_class: Some(defining_class),
                        kind: FrameKind::Method(method),
                        slot_base: 0, // a method body never contains InitSlot
                        code,
                    },
                    args,
                    temps,
                ))
            }
        }
    }

    /// Every re-entry of the run loop from Rust: `push` pushes one
    /// activation, and the loop runs it and everything it activates and pops
    /// all of it (`vm::run`). The public `send`, each initializing level of an
    /// instantiation and its clause message, the nested-class accessor's
    /// clause, and every native that still calls a block come through here --
    /// the paths "What stays recursive" in `docs/design/execution.md` names.
    pub(crate) fn reenter(
        &mut self,
        push: impl FnOnce(&mut Interpreter) -> Result<ActivationRef, Signal>,
    ) -> EvalResult {
        let act = push(self)?;
        crate::vm::run(self, act)
    }

    /// Whether a `^` aimed at `act` ends at `act`: a method's does, and a
    /// superclass clause's or its message's. An initializer catches none -- a
    /// `^` into a running one escapes, the known gap in
    /// `docs/design/execution.md`, "Non-local return" -- and a block's `^`
    /// always targets its home, never the block.
    pub(crate) fn catches_return(&self, act: ActivationRef) -> bool {
        matches!(
            self.heap.activation_kind(act),
            FrameKind::Method(_) | FrameKind::SuperclassClause(_) | FrameKind::SuperclassMessage(_)
        )
    }
```

`send` runs a decided activation through `reenter`:

```rust
        let result = match self.decide_send(receiver, selector, &args) {
            Ok(Step::Value(value)) => Ok(value),
            Ok(step) => self.reenter(|interp| interp.push_step(step, &args)),
            Err(signal) => Err(signal),
        };
```

`invoke_closure` keeps its arity check and pushes through `reenter`:

```rust
        let receiver = self.heap.frame_receiver(closure.captured_frame);
        let current_class = self.heap.frame_current_class(closure.captured_frame);
        let code = closure.code.clone();
        let temps = (code.num_registers - code.num_args) as usize;
        self.reenter(|interp| {
            Ok(interp.heap.push_activation(
                ActivationHeader {
                    receiver,
                    current_class,
                    kind: FrameKind::Closure(closure.clone()),
                    slot_base: 0, // a closure body never contains InitSlot
                    code,
                },
                &args,
                temps,
            ))
        })
```

In `instantiate.rs`, `run_initializer_level` keeps its arity check and its
superclass recursion (Task 6 makes that iterative); after
`class_has_initializer_code`, it reads the code first, so the activation is
pushed with its whole register file and no longer grows:

```rust
        let application = self
            .heap
            .class_application(class)
            .expect("internal: class_has_initializer_code confirmed this is a class object");
        let code = application.mixin.initializer_code.clone().unwrap_or_else(|| {
            panic!("internal: class needs an instance initializer and its mixin carries none (a mixin built from source always does; verify_mixins refuses a decoded one)")
        });
        let slot_base = application.own_slot_base;
        let temps = (code.num_registers - code.num_args) as usize;
        self.reenter(|interp| {
            Ok(interp.heap.push_activation(
                ActivationHeader {
                    receiver: *instance,
                    current_class: Some(class),
                    kind: FrameKind::Initializer(class),
                    slot_base,
                    code,
                },
                &factory_args,
                temps,
            ))
        })
        .map(|_| ())
```

Delete `run_own_initializer` (its checks are the lines above; its comment about
`InitSlot` being mixin-relative moves onto them), and delete the now-unused
`ActivationRef` import if nothing else in the file needs it.

In `nested.rs`, `run_clause_body` pushes through `reenter` the same way, with
`code: code.clone()` in its header; the `Signal::Return` catch it had is now
`catches_return` in the loop, so the function body is the header and the
`reenter` call.

Update the doc comments that say "pushes an activation, calls `run_code` and
pops it" (in `interpreter/mod.rs`, `instantiate.rs`, `nested.rs`) to say the
loop pops what it runs.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p ns-runtime --test trampoline`, then the whole gate list.
Expected: both trampoline tests PASS — `down:`, `logic:` and `loop:` fifty
thousand deep on a test thread's 2 MB stack; every recorded answer unchanged.

- [ ] **Step 7: Measure**

Follow the protocol against this task's parent. Expected: `bench_send.ns`
faster (no Rust recursion per send, no argument vector); the others no slower.
Report the table.

- [ ] **Step 8: Commit**

```
feat: run sends on the activation stack instead of the Rust stack

The VM's loop pushes the activation a send decided and switches to it;
a return pops and resumes the caller at the instruction after its send;
a non-local return pops to the activation it names, and any other
signal pops the run and leaves it. Arguments go from the caller's
registers through one reusable buffer into the callee's window. What
re-enters the loop from Rust -- the public send, instantiation, the
nested-class accessor, and the natives that still call a block -- does
it through Interpreter::reenter.
```

---

### Task 5: Block activations are steps

`value` and the control-flow natives return the block to activate instead of
calling it. `do:` keeps calling, through `invoke_closure`.

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`Continuation`, `WhileLoop`)
- Modify: `crates/ns-runtime/src/heap.rs` (`ActivationHeader.then`,
  `StackEntry.then`, `Popped`, `pop_activation` answers it)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Step::Block`,
  `StepArgs::Empty`, `push_step`'s block arm, `check_block_arity`, `resume`;
  `invoke_closure` through `push_step`)
- Modify: `crates/ns-runtime/src/builtins/boolean.rs`,
  `crates/ns-runtime/src/builtins/block.rs`, `crates/ns-runtime/src/builtins/mod.rs`
- Modify: `crates/ns-runtime/src/vm.rs` (`finish` applies the continuation)
- Test: `crates/ns-runtime/tests/trampoline.rs`, `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:**
- Consumes: Task 4's loop, `push_step`, `reenter`.
- Produces:
  ```rust
  // types.rs
  #[derive(Debug, Clone)]
  pub enum Continuation { Deliver, Truthy, WhileCondition(Rc<WhileLoop>), WhileBody(Rc<WhileLoop>) }
  #[derive(Debug)]
  pub struct WhileLoop { pub(crate) condition: Rc<ClosureData>, pub(crate) body: Value, pub(crate) expect: bool }
  // heap.rs
  pub struct ActivationHeader { …, pub then: Continuation }
  pub struct Popped { pub then: Continuation }
  pub fn pop_activation(&mut self, act: ActivationRef) -> Popped;
  // interpreter
  pub(crate) enum Step { Value(..), Method {..}, Block { closure: Rc<ClosureData>, args: StepArgs, then: Continuation } }
  pub(crate) enum StepArgs { Send, Empty, Owned(Vec<Value>) }
  pub(crate) fn Interpreter::resume(&mut self, then: Continuation, value: Value) -> Result<Step, Signal>;
  pub(crate) fn builtins::boolean::truthy(value: Value) -> bool;
  pub(crate) fn builtins::block::after_condition(interp: &mut Interpreter, state: Rc<WhileLoop>, value: Value) -> Result<Step, Signal>;
  pub(crate) fn builtins::block::after_body(interp: &Interpreter, state: Rc<WhileLoop>) -> Step;
  ```

- [ ] **Step 1: Write the failing tests**

Append to `crates/ns-runtime/tests/trampoline.rs`:

```rust
/// A recursion through blocks the optimized lowering does *not* inline --
/// each held in a variable -- runs through the natives, and after this task
/// none of them calls back into the loop, so fifty thousand levels fit on a
/// test thread's stack under either lowering.
const THROUGH_NATIVES: &str = "Newspeak3\n'T'\nclass N = Object new (\n) (\n\
    \tpublic viaValue: n = ( | b | b := [:k | k = 0 ifTrue: [0] ifFalse: [b value: k - 1]]. ^b value: n )\n\
    \tpublic viaIf: n = ( | t f | t := [0]. f := [viaIf: n - 1]. ^n = 0 ifTrue: t ifFalse: f )\n\
    \tpublic viaOr: n = ( | rest | rest := [(viaOr: n - 1) = 0]. ^(n = 0 or: rest) ifTrue: [0] ifFalse: [1] )\n\
    \tpublic viaWhile: n = ( | c b | c := [n > 0]. b := [^viaWhile: n - 1]. c whileTrue: b. ^0 )\n\
    ) : ()";

#[test]
fn a_deep_recursion_through_native_block_activations_runs_on_the_activation_stack() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(THROUGH_NATIVES, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        for selector in ["viaValue:", "viaIf:", "viaOr:", "viaWhile:"] {
            let result = expect_ok(
                interp.send(instance, Symbol::intern(selector), vec![Value::SmallInt(40_000)]),
                selector,
            );
            assert_eq!(small_int(result), 0, "{selector} under {lowering:?}");
            assert_eq!(interp.heap_ref().activation_depth(), 0, "{selector} under {lowering:?}");
        }
    }
}
```

Append to `crates/ns-cli/tests/program_outcomes.rs` (each is a recorded answer
derived from the natives' rules, which the continuations must reproduce):

```rust
/// `and:`'s block value coerces by `truthy_arg`'s rule when the block is a
/// variable, which takes the native: a non-Boolean is `false`.
#[test]
fn a_variable_and_block_coerces_its_value_to_a_boolean() {
    assert_outcome(
        "and-variable-coerces",
        "Newspeak3\n'T'\nclass AndVar usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t| three yes |\n\t\tthree := [3].\n\t\tyes := [true].\n\t\tp transcript show: (true and: three) printString; newLine.\n\t\tp transcript show: (true and: yes) printString; newLine.\n\t\tp transcript show: (false or: three) printString; newLine\n\t)\n) : ()\n",
        0,
        "false\ntrue\nfalse\n",
        "",
    );
}

/// A native loop whose condition goes non-Boolean after two rounds sends
/// `mustBeBoolean` to it, after exactly two bodies.
#[test]
fn a_variable_while_condition_that_goes_non_boolean_fails_with_must_be_boolean() {
    assert_outcome(
        "while-variable-mbb",
        "Newspeak3\n'T'\nclass WhileVarMbb usingPlatform: platform = Object new (\n\t| n ::= 0. |\n) (\n\tpublic main: p args: a = (\n\t\t| cond body |\n\t\tcond := [n < 2 ifTrue: [true] ifFalse: [n]].\n\t\tbody := [n := n + 1. p transcript show: n printString; newLine].\n\t\tcond whileTrue: body\n\t)\n) : ()\n",
        1,
        "1\n2\n",
        "Uncaught Newspeak error: an object does not understand #mustBeBoolean\n",
    );
}

/// A block with a parameter passed where the native activates it with none
/// is the ordinary wrong-arity error.
#[test]
fn an_if_block_that_takes_an_argument_is_a_wrong_arity_error() {
    assert_outcome(
        "if-block-arity",
        "Newspeak3\n'T'\nclass IfArity usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t| b |\n\t\tb := [:x | x].\n\t\ttrue ifTrue: b\n\t)\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: wrong number of block arguments: block takes 1, got 0\n",
    );
}
```

Before running them, check each expected answer against today's code by running
the program with the current binary (`cargo run -p ns-cli -- run <file>`) under
both lowerings: these three pin *existing* behaviour the task must preserve, so
they must already pass before any change. If one does not, the expectation is
wrong — fix the expectation from the natives' rules (`boolean.rs`, `block.rs`),
not from the output, and say so in the report.

- [ ] **Step 2: Run the tests to verify the trampoline test fails**

Run: `cargo test -p ns-runtime --test trampoline` and
`cargo test -p ns-cli --test program_outcomes`
Expected: the three new outcomes PASS (they pin existing behaviour);
`a_deep_recursion_through_native_block_activations_runs_on_the_activation_stack`
aborts with a stack overflow — every native block activation still re-enters
the loop from Rust.

- [ ] **Step 3: The continuation and the heap**

`types.rs`, after `FrameKind`:

```rust
/// What the run loop does with an activation's value when it returns
/// (`docs/design/execution.md`, "The trampoline (1b)"). Every activation a
/// send makes hands its value to that send; the other three are how a builtin
/// control-flow native finishes its work once the block it activated has
/// returned, with no Rust frame waiting for it.
#[derive(Debug, Clone)]
pub enum Continuation {
    /// Hand the value to the send that made the activation.
    Deliver,
    /// `and:` and `or:`: hand on whether the value is exactly `true`
    /// (`builtins::boolean::truthy`).
    Truthy,
    /// A native `whileTrue:`/`whileFalse:` loop's condition returned.
    WhileCondition(Rc<WhileLoop>),
    /// Its body returned.
    WhileBody(Rc<WhileLoop>),
}

/// A native `whileTrue:`/`whileFalse:` loop in progress: its condition, its
/// body -- any value, since a body that is not a block evaluates to itself --
/// and the condition value that keeps it going.
#[derive(Debug)]
pub struct WhileLoop {
    pub(crate) condition: Rc<ClosureData>,
    pub(crate) body: Value,
    pub(crate) expect: bool,
}
```

`heap.rs`: add `pub then: Continuation` to `ActivationHeader` (documented
"What the run loop does with the activation's value when it returns."), a
`then: Continuation` field to `StackEntry`, and

```rust
/// What popping an activation hands back to the run loop.
pub struct Popped {
    /// The popped activation's continuation.
    pub then: Continuation,
}
```

`pop_activation` returns `Popped { then: entry.then }` after the divorce it
already does. Every existing `ActivationHeader { … }` gains
`then: Continuation::Deliver`; callers that ignore the result of
`pop_activation` keep ignoring it.

- [ ] **Step 4: The step and its continuation**

`interpreter/mod.rs`: add to `Step`

```rust
    /// Activate `closure`, and do `then` with its value.
    Block {
        closure: Rc<ClosureData>,
        args: StepArgs,
        then: Continuation,
    },
```

(Task 6 adds the mark the depth count reads), add
`StepArgs::Empty` ("None: a control-flow native activates a block with no
arguments.") answering `&[]` from `slice`, and move `invoke_closure`'s arity
check into

```rust
/// A block given a different number of arguments than it declares is refused
/// before it can reach a register: the window is laid out from the arguments
/// actually given, so too few would address registers past its end -- a panic
/// -- and too many would silently shift every temporary.
fn check_block_arity(heap: &Heap, closure: &ClosureData, given: usize) -> Result<(), Signal> {
    if given == closure.params.len() {
        return Ok(());
    }
    Err(Signal::Error(NewspeakError {
        selector: Symbol::VALUE,
        receiver: heap.frame_receiver(closure.captured_frame),
        message: format!(
            "wrong number of block arguments: block takes {}, got {}",
            closure.params.len(),
            given
        ),
    }))
}
```

`push_step` gains the block arm, and every header gains its `then`:

```rust
            Step::Block {
                closure,
                args,
                then,
            } => {
                let args = args.slice(send_args);
                check_block_arity(&self.heap, &closure, args.len())?;
                let receiver = self.heap.frame_receiver(closure.captured_frame);
                let current_class = self.heap.frame_current_class(closure.captured_frame);
                let code = closure.code.clone();
                let temps = (code.num_registers - code.num_args) as usize;
                Ok(self.heap.push_activation(
                    ActivationHeader {
                        receiver,
                        current_class,
                        kind: FrameKind::Closure(closure),
                        slot_base: 0, // a closure body never contains InitSlot
                        code,
                        then,
                    },
                    args,
                    temps,
                ))
            }
```

(the method arm's header gets `then: Continuation::Deliver`).
`invoke_closure` becomes

```rust
    /// Invokes a closure with `args`, from Rust. Only `do:` still does this --
    /// every other native that runs a block returns it as a step instead
    /// (`docs/design/execution.md`, "What stays recursive").
    pub fn invoke_closure(&mut self, closure: &Rc<ClosureData>, args: Vec<Value>) -> EvalResult {
        let step = Step::Block {
            closure: closure.clone(),
            args: StepArgs::Owned(args),
            then: Continuation::Deliver,
        };
        self.reenter(|interp| interp.push_step(step, &[]))
    }
```

and add

```rust
    /// What a control-flow native does once the block it activated has
    /// returned `value`: the second half of the native, which the run loop
    /// waits for instead of a Rust frame.
    pub(crate) fn resume(&mut self, then: Continuation, value: Value) -> Result<Step, Signal> {
        match then {
            Continuation::Deliver => Ok(Step::Value(value)),
            Continuation::Truthy => Ok(Step::Value(Value::Bool(
                crate::builtins::boolean::truthy(value),
            ))),
            Continuation::WhileCondition(state) => {
                crate::builtins::block::after_condition(self, state, value)
            }
            Continuation::WhileBody(state) => Ok(crate::builtins::block::after_body(self, state)),
        }
    }
```

- [ ] **Step 5: The natives**

`builtins/boolean.rs` becomes:

```rust
//! `Boolean`/`True`/`False` native methods. The control-flow ones return the
//! block to activate as a step instead of calling it, so none of them waits
//! on a Rust frame for Newspeak code (`docs/design/execution.md`, "The
//! trampoline (1b)").

use crate::interpreter::{Signal, Step, StepArgs};
use crate::types::*;
use crate::Interpreter;
use crate::Symbol;

pub(crate) fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    let Value::Bool(b) = receiver else {
        return None;
    };
    Some(Ok(match selector.as_str() {
        "ifTrue:ifFalse:" => block_step(interp, if b { args[0] } else { args[1] }, Continuation::Deliver),
        "ifFalse:ifTrue:" => block_step(interp, if b { args[1] } else { args[0] }, Continuation::Deliver),
        "ifTrue:" => {
            if b {
                block_step(interp, args[0], Continuation::Deliver)
            } else {
                Step::Value(Value::Nil)
            }
        }
        "ifFalse:" => {
            if b {
                Step::Value(Value::Nil)
            } else {
                block_step(interp, args[0], Continuation::Deliver)
            }
        }
        "not" => Step::Value(Value::Bool(!b)),
        // The short circuit holds: `false and: [...]` and `true or: [...]`
        // never activate the block. Otherwise the block's value is coerced
        // when it returns (`Continuation::Truthy`), so a `^` out of it or an
        // error in it passes through untouched.
        "&" | "and:" => {
            if b {
                block_step(interp, args[0], Continuation::Truthy)
            } else {
                Step::Value(Value::Bool(false))
            }
        }
        "|" | "or:" => {
            if b {
                Step::Value(Value::Bool(true))
            } else {
                block_step(interp, args[0], Continuation::Truthy)
            }
        }
        "printString" => Step::Value(Value::Obj(
            interp.new_string(if b { "true" } else { "false" }.to_string()),
        )),
        _ => return None,
    }))
}

/// `arg` activated as a block with no arguments, its value handed to `then`
/// -- or, when `arg` is not a block, `arg` itself taken as the value, the
/// reference library's `ifTrue: aValue` convention.
fn block_step(interp: &Interpreter, arg: Value, then: Continuation) -> Step {
    if let Value::Obj(obj) = arg {
        if let Some(closure) = interp.heap_ref().closure_data(obj) {
            return Step::Block {
                closure,
                args: StepArgs::Empty,
                then,
            };
        }
    }
    match then {
        Continuation::Truthy => Step::Value(Value::Bool(truthy(arg))),
        _ => Step::Value(arg),
    }
}

/// `and:`/`or:`'s rule, which the compiler's inlined form reproduces in
/// bytecode (`compiler/lower.rs`, `emit_coerce_to_bool`): only exactly `true`
/// is true, and a non-Boolean is `false`.
pub(crate) fn truthy(value: Value) -> bool {
    matches!(value, Value::Bool(true))
}
```

`builtins/block.rs` becomes:

```rust
//! `Block` (closure) native methods -- `value`..`value:value:value:`,
//! `numArgs`, and `whileTrue:`/`whileFalse:`. Each returns the block to
//! activate as a step; the loops continue through `Continuation`s.

use std::rc::Rc;

use crate::interpreter::{Signal, Step, StepArgs};
use crate::types::*;
use crate::{Interpreter, Symbol};

pub(crate) fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let closure = interp.heap_ref().closure_data(*obj)?;
    Some(Ok(match selector.as_str() {
        "value" | "value:" | "value:value:" | "value:value:value:" => Step::Block {
            closure,
            args: StepArgs::Send,
            then: Continuation::Deliver,
        },
        "numArgs" => Step::Value(Value::SmallInt(closure.params.len() as i64)),
        "whileTrue:" => condition_step(Rc::new(WhileLoop {
            condition: closure,
            body: args[0],
            expect: true,
        })),
        "whileFalse:" => condition_step(Rc::new(WhileLoop {
            condition: closure,
            body: args[0],
            expect: false,
        })),
        _ => return None,
    }))
}

// `whileTrue:` (`expect == true`) / `whileFalse:` (`expect == false`):
// evaluate the condition; while its value is `Bool(expect)`, evaluate the
// body and repeat; a `Bool(!expect)` ends the loop with `nil`; a non-Boolean
// condition value is sent `mustBeBoolean`, and that send's result is the
// loop's value. The inlined loop reproduces exactly this on its guard-failure
// path (`docs/design/compiler.md`, "Control-flow inlining"). Condition and
// body replace each other on the activation stack: each one's continuation
// says what runs next.

fn condition_step(state: Rc<WhileLoop>) -> Step {
    Step::Block {
        closure: state.condition.clone(),
        args: StepArgs::Empty,
        then: Continuation::WhileCondition(state),
    }
}

/// The condition returned `value`.
pub(crate) fn after_condition(
    interp: &mut Interpreter,
    state: Rc<WhileLoop>,
    value: Value,
) -> Result<Step, Signal> {
    match value {
        Value::Bool(b) if b == state.expect => Ok(body_step(interp, state)),
        Value::Bool(_) => Ok(Step::Value(Value::Nil)),
        other => interp.decide_send(other, Symbol::MUST_BE_BOOLEAN, &[]),
    }
}

/// The body returned: run the condition again.
pub(crate) fn after_body(_interp: &Interpreter, state: Rc<WhileLoop>) -> Step {
    condition_step(state)
}

/// The body, activated -- or, when it is not a block, taken as its own value,
/// so the condition runs again straight away.
fn body_step(interp: &Interpreter, state: Rc<WhileLoop>) -> Step {
    if let Value::Obj(obj) = state.body {
        if let Some(body) = interp.heap_ref().closure_data(obj) {
            return Step::Block {
                closure: body,
                args: StepArgs::Empty,
                then: Continuation::WhileBody(state),
            };
        }
    }
    condition_step(state)
}
```

(`after_body` takes the interpreter only so the two resumptions read alike; if
clippy flags the unused parameter, drop it and the argument in `resume`.)

`builtins/mod.rs`: the `Boolean`/`True`/`False` and `Block` arms of
`dispatch_native` return their natives' steps directly; `call_block`'s doc
comment now says its one caller is `do:` and why `do:` still calls (it moves to
Newspeak after the inline caches, `docs/design/core-library.md`); the long
module comment listing "five functions ever invoke a block" is cut to `do:`'s
two callers (`dispatch_common` and the two collection dispatchers that fall
through to it). `run_while` and `truthy_arg` are gone.

- [ ] **Step 6: `finish` applies the continuation**

In `vm.rs`, replace `finish` with:

```rust
/// `act` has returned `value`: pop it and do what its continuation says. A
/// value -- the usual case -- goes to the send in the caller, which resumes,
/// or out of `run` when there is no caller in this run; a control-flow
/// native's continuation may instead activate the next block, which runs in
/// the popped activation's place.
fn finish(interp: &mut Interpreter, act: ActivationRef, value: Value, floor: usize) -> Flow {
    let popped = interp.heap.pop_activation(act);
    let caller = if interp.heap.activation_depth() == floor {
        None
    } else {
        interp.heap.top_activation()
    };
    let value = match popped.then {
        Continuation::Deliver => value,
        then => match interp.resume(then, value) {
            Ok(Step::Value(v)) => v,
            Ok(step) => {
                return match interp.push_step(step, &[]) {
                    Ok(next) => Flow::Switch {
                        act: next,
                        code: interp.heap.activation_code(next),
                        pc: 0,
                    },
                    Err(signal) => unwind(interp, signal, floor),
                };
            }
            Err(signal) => return unwind(interp, signal, floor),
        },
    };
    match caller {
        None => Flow::Exit(Ok(value)),
        Some(caller) => resume_caller(interp, caller, value),
    }
}
```

A continuation's step never refers to the send's arguments (`StepArgs::Send`
is only answered by a decision about a send), so `&[]` is always right there.
In `interpreter/mod.rs`, `send` now also receives `Step::Block` from
`decide_send` (a `value` sent from Rust) and runs it through the same `reenter`
arm as a method — nothing to change there beyond the match being exhaustive.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p ns-runtime --test trampoline`, then the whole gate list.
Expected: all PASS, including the forty-thousand-deep native recursions under
both lowerings; every recorded answer unchanged.

- [ ] **Step 8: Measure**

Protocol against this task's parent. Expected: `bench_block.ns` faster — a
block's `value:` is now a push rather than a native call, a nested loop and a
pop; the others no slower. Report the table.

- [ ] **Step 9: Commit**

```
feat: activate blocks as trampoline steps

Block value and the control-flow natives return the block to activate
instead of calling it: the if forms and value hand its value to the
send, and:/or: coerce it through a continuation when it returns, and
whileTrue:/whileFalse: continue from the condition to the body and back
without nesting. Only do: still calls a block from Rust.
```

---

### Task 6: Depth becomes a Newspeak error

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`ActivationHeader.counted`,
  `StackEntry.counted`, the stack's `counted` total, `counted_depth`,
  `Popped.counted`, `activation_reified`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`MAX_ACTIVATION_DEPTH`,
  `MAX_NATIVE_DEPTH`, `native_depth`, `push`, the two errors, `push_step`'s
  exemption, `reenter` counts)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (the superclass
  chain walked iteratively)
- Modify: `crates/ns-runtime/src/interpreter/nested.rs`,
  `crates/ns-runtime/src/vm.rs` (pass the sender and the mark)
- Modify: `crates/ns-cli/src/main.rs` (the interpreter thread)
- Test: `crates/ns-runtime/tests/trampoline.rs`,
  `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:**
- Consumes: Tasks 2, 4 and 5.
- Produces:
  ```rust
  pub const interpreter::MAX_ACTIVATION_DEPTH: usize = 100_000;
  pub const interpreter::MAX_NATIVE_DEPTH: usize = 1_000;
  impl Heap {
      pub fn counted_depth(&self) -> usize;
      pub fn activation_reified(&self, act: ActivationRef) -> Option<FrameRef>;
  }
  pub struct Popped { pub then: Continuation, pub counted: bool }
  Step::Block { closure, args, then, control_flow: bool }   // the mark, new here
  pub(crate) fn Interpreter::push_step(&mut self, step: Step, send_args: &[Value], sender: Option<ActivationRef>, inlinable: bool) -> Result<ActivationRef, Signal>;
  ```

- [ ] **Step 1: Write the failing boundary tests**

Append to `crates/ns-runtime/tests/trampoline.rs`:

```rust
const TOO_DEEP: &str = "stack overflow: recursion too deep";

fn expect_too_deep(result: Result<Value, Signal>, what: &str) {
    match result {
        Err(Signal::Error(e)) => assert_eq!(e.message, TOO_DEEP, "{what}"),
        Err(Signal::Return(..)) => panic!("{what}: a non-local return escaped"),
        Ok(_) => panic!("{what}: ran past the depth limit"),
    }
}

/// The limit is exactly `MAX_ACTIVATION_DEPTH` counted activations, and it
/// counts alike under both lowerings (`docs/design/compiler.md`, "How the
/// limit counts"). `down: k` sent from Rust makes k + 1 method activations,
/// so `down: 99_999` is the deepest that fits and `down: 100_000` one too
/// many. `logic:` and `loop:` recurse from inside an `or:` block and a
/// `whileTrue:` body, which the optimized lowering inlines and the reference
/// lowering activates -- exempt, so the boundary is the same.
#[test]
fn the_depth_limit_is_exact_and_counts_alike_under_both_lowerings() {
    assert_eq!(ns_runtime::interpreter::MAX_ACTIVATION_DEPTH, 100_000);
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(DEPTH, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        for selector in ["down:", "logic:", "loop:"] {
            let what = format!("{selector} under {lowering:?}");
            expect_ok(
                interp.send(instance, Symbol::intern(selector), vec![Value::SmallInt(99_999)]),
                &what,
            );
            expect_too_deep(
                interp.send(instance, Symbol::intern(selector), vec![Value::SmallInt(100_000)]),
                &what,
            );
            assert_eq!(interp.heap_ref().activation_depth(), 0, "{what}: nothing left behind");
            // An overflow is an ordinary error: the interpreter goes on.
            expect_ok(
                interp.send(instance, Symbol::intern(selector), vec![Value::SmallInt(10)]),
                &what,
            );
        }
    }
}

/// A block activated through `value` is counted like any other activation,
/// so a recursion through blocks alone ends in the same error.
#[test]
fn a_recursion_through_block_values_is_bounded_too() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(THROUGH_NATIVES, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        expect_too_deep(
            interp.send(instance, Symbol::intern("viaValue:"), vec![Value::SmallInt(250_000)]),
            &format!("viaValue: under {lowering:?}"),
        );
    }
}

/// A crafted artifact can send `ifTrue:` as a `SendInlinable` with a block
/// the sending activation did not create -- here the block itself, so it
/// recurses. The exemption requires the block's captured frame to be the
/// sender's (`docs/design/compiler.md`, "How the limit counts"), so this is
/// counted and ends in the depth error; without that condition it would run
/// uncounted until memory ran out.
#[test]
fn a_crafted_send_inlinable_cannot_recurse_uncounted() {
    use ns_runtime::image;
    use ns_runtime::image::opcodes::{OP_SEND_INLINABLE, OP_SEND_PUBLIC};
    let src = "Newspeak3\n'T'\nclass Crafted = Object new (\n) (\n\tpublic main = ( | b | b := [true ifTrue: b]. ^b value )\n) : ()";
    // `true ifTrue: b` is not a literal-block send, so even the reference
    // lowering compiles it to a `SendPublic`, which the mutation below turns
    // into a `SendInlinable` -- the two share one operand layout.
    let (interp, class) = load(src, Lowering::Reference);
    let mut bytes = image::build(
        interp.heap_ref(),
        None,
        &SourceOrigin::File(PathBuf::from("t.ns")),
        class,
        Symbol::intern("main"),
    )
    .expect("build");
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let id = report
        .symbols
        .iter()
        .position(|s| s == "ifTrue:")
        .expect("ifTrue: is interned") as u32;
    let mut pattern = vec![OP_SEND_PUBLIC];
    pattern.extend_from_slice(&id.to_le_bytes());
    let at: Vec<usize> = bytes
        .windows(pattern.len())
        .enumerate()
        .filter(|(_, w)| *w == pattern.as_slice())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "exactly one `ifTrue:` send to mutate");
    bytes[at[0]] = OP_SEND_INLINABLE;

    let mut loaded = image::load(&bytes).expect("the mutated artifact decodes");
    let instance = expect_ok(
        loaded.interp.instantiate(loaded.root_class, vec![]),
        "instantiate",
    );
    expect_too_deep(
        loaded.interp.send(instance, Symbol::intern("main"), vec![]),
        "the crafted recursion",
    );
}
```

Append to `crates/ns-cli/tests/program_outcomes.rs` — each answer derived from
the rule:

```rust
/// `KernelWeakTests.ns`'s `deepStack: 1000`, which Newspeak's own tests expect
/// to run: 1000 levels through an `ifFalse:` block. Sum of 2n for n = 1..1000.
#[test]
fn a_thousand_deep_recursion_runs() {
    assert_outcome(
        "deep-stack",
        "Newspeak3\n'T'\nclass Deep usingPlatform: platform = Object new (\n) (\n\tdeepStack: n = ( ^0 = n ifTrue: [0] ifFalse: [n + (deepStack: n - 1) + n] )\n\tpublic main: p args: a = (\n\t\tp transcript show: (deepStack: 1000) printString; newLine\n\t)\n) : ()\n",
        0,
        "1001000\n",
        "",
    );
}

/// Unbounded recursion is an ordinary Newspeak error, not a process abort,
/// and the same one under both lowerings.
#[test]
fn an_unbounded_recursion_is_a_newspeak_error() {
    assert_outcome(
        "unbounded-recursion",
        "Newspeak3\n'T'\nclass Unbounded usingPlatform: platform = Object new (\n) (\n\tdown: n = ( ^1 + (down: n + 1) )\n\tpublic main: p args: a = (\n\t\tp transcript show: 'start'; newLine.\n\t\tdown: 0\n\t)\n) : ()\n",
        1,
        "start\n",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}

/// `do:` still re-enters the run loop from Rust for every element, so a
/// recursion through it is bounded by the second limit -- and ends in its
/// error, not in a native stack overflow, on the CLI's interpreter thread.
#[test]
fn a_recursion_through_do_is_bounded_by_the_native_limit() {
    assert_outcome(
        "do-recursion",
        "Newspeak3\n'T'\nclass DoDeep usingPlatform: platform = Object new (\n) (\n\td: n = ( #(1) do: [:e | d: n + 1] )\n\tpublic main: p args: a = ( d: 0 )\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: stack overflow: too many nested native calls\n",
    );
}

/// Instantiation re-enters the loop for each initializer level, so a class
/// whose slot initializer instantiates it again is bounded by the same limit.
#[test]
fn a_recursive_instantiation_is_bounded_by_the_native_limit() {
    assert_outcome(
        "instantiation-recursion",
        "Newspeak3\n'T'\nclass Tower usingPlatform: platform = Object new (\n) (\n\tpublic class Floor = Object new (\n\t\t| above = Floor new. |\n\t) (\n\t)\n\tpublic main: p args: a = ( Floor new )\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: stack overflow: too many nested native calls\n",
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ns-runtime --test trampoline` and
`cargo test -p ns-cli --test program_outcomes`
Expected: `MAX_ACTIVATION_DEPTH` does not exist (compile error); once stubbed,
the boundary tests run past the limit; the two native-limit outcomes abort with
a native stack overflow (exit code not 1); `deep-stack` passes already;
`unbounded-recursion` runs until memory is exhausted — run it with a timeout or
skip it for this step.

- [ ] **Step 3: The count in the heap**

Add `pub counted: bool` to `ActivationHeader` ("Whether the activation counts
against the depth limit -- every one does, except a block a control-flow native
activates for a `SendInlinable` (`Interpreter::push_step`)."), a `counted: bool`
to `StackEntry`, and `counted: usize` to `ActivationStack`, incremented by
`push_activation` when the header is counted and decremented by
`pop_activation` when the entry was. `Popped` gains `pub counted: bool`. Add:

```rust
    /// How many live activations count against the depth limit.
    pub fn counted_depth(&self) -> usize {
        self.stack.counted
    }

    /// The frame `act` was reified into, if a block has captured it.
    pub fn activation_reified(&self, act: ActivationRef) -> Option<FrameRef> {
        self.check_activation(act);
        self.stack.entries[act.index as usize].reified
    }
```

Every `ActivationHeader` built outside `push_step`'s block arm gets
`counted: true`.

Give `Step::Block` its mark:

```rust
        /// Set by a builtin control-flow native -- `ifTrue:` and its three
        /// relatives, `and:`, `or:`, `whileTrue:`, `whileFalse:` -- for the
        /// block activation the optimized lowering inlines away, which
        /// `push_step` exempts from the depth count when the send was a
        /// `SendInlinable`.
        control_flow: bool,
```

and set it where each block step is built: `true` in `boolean::block_step`,
`block::condition_step` and `block::body_step`; `false` in `block::dispatch`'s
`value` arm and in `invoke_closure`.

- [ ] **Step 4: The limits in the interpreter**

In `interpreter/mod.rs`:

```rust
/// L: the most activations that may be live at once, counted by the rule in
/// `docs/design/compiler.md`, "How the limit counts". It bounds memory, not
/// the native stack, which the trampoline no longer recurses on.
pub const MAX_ACTIVATION_DEPTH: usize = 100_000;

/// N: how deep Rust may re-enter the run loop -- `do:`, instantiation, the
/// nested-class accessor. It is what bounds the native stack; the CLI sizes
/// its interpreter thread for it (`docs/design/execution.md`, "Two bounds, not
/// one").
pub const MAX_NATIVE_DEPTH: usize = 1_000;
```

Add a field `native_depth: usize` (0 at bootstrap). Add, next to the other
error constructors at the bottom of the file:

```rust
/// L's error. Neither depth error names its limit, so that changing one
/// changes no recorded answer.
fn recursion_too_deep(receiver: Value, selector: Symbol) -> Signal {
    Signal::Error(NewspeakError {
        selector,
        receiver,
        message: "stack overflow: recursion too deep".to_string(),
    })
}

/// N's error.
fn too_many_nested_native_calls() -> Signal {
    Signal::Error(NewspeakError {
        selector: Symbol::UNKNOWN,
        receiver: Value::Nil,
        message: "stack overflow: too many nested native calls".to_string(),
    })
}
```

and the one push every activation goes through:

```rust
    /// Pushes an activation, refusing one that would take the counted depth
    /// past `MAX_ACTIVATION_DEPTH`. Every activation in the runtime is pushed
    /// here, so the count and the check cannot be skipped by a new site.
    fn push(
        &mut self,
        header: ActivationHeader,
        args: &[Value],
        temps: usize,
    ) -> Result<ActivationRef, Signal> {
        if header.counted && self.heap.counted_depth() >= MAX_ACTIVATION_DEPTH {
            let selector = match &header.kind {
                FrameKind::Method(method) => method.selector,
                FrameKind::Closure(_) => Symbol::VALUE,
                _ => Symbol::NEW,
            };
            return Err(recursion_too_deep(header.receiver, selector));
        }
        Ok(self.heap.push_activation(header, args, temps))
    }
```

Replace every `self.heap.push_activation(`/`interp.heap.push_activation(` under
`crates/ns-runtime/src/interpreter/` with `self.push(`/`interp.push(`
(`grep -rn "heap.push_activation" crates/ns-runtime/src/interpreter` must come
back empty afterwards). `push` is `pub(crate)` if `nested.rs` or
`instantiate.rs` need it from outside the impl block.

`push_step` takes the sender and the mark, and decides whether a block counts:

```rust
    pub(crate) fn push_step(
        &mut self,
        step: Step,
        send_args: &[Value],
        sender: Option<ActivationRef>,
        inlinable: bool,
    ) -> Result<ActivationRef, Signal> {
```

In the block arm, before building the header:

```rust
                // Exempt exactly the activation the optimized lowering
                // inlines away: a block a control-flow native activates for
                // a `SendInlinable`, and only if the sending activation
                // created it -- which every block the reference lowering
                // passes to one is, and which keeps a crafted artifact from
                // recursing uncounted (`docs/design/compiler.md`, "How the
                // limit counts").
                let created_by_sender = sender
                    .and_then(|sender| self.heap.activation_reified(sender))
                    .is_some_and(|frame| frame.same(closure.captured_frame));
                let counted = !(inlinable && control_flow && created_by_sender);
```

and the header takes `counted`. The method arm is always `counted: true`.

`reenter` counts N:

```rust
    pub(crate) fn reenter(
        &mut self,
        push: impl FnOnce(&mut Interpreter) -> Result<ActivationRef, Signal>,
    ) -> EvalResult {
        if self.native_depth >= MAX_NATIVE_DEPTH {
            return Err(too_many_nested_native_calls());
        }
        let act = push(self)?;
        self.native_depth += 1;
        let result = crate::vm::run(self, act);
        self.native_depth -= 1;
        result
    }
```

Callers of `push_step`: `send` and `invoke_closure` pass `None, false`.
`vm::send` passes `Some(act)` and whether the instruction was a
`SendInlinable` — split the or-pattern arm from Task 4 into two arms that call
`send` with `false` and `true`, and give `send` an `inlinable: bool`
parameter. `vm::finish` passes `caller` and `!popped.counted`: the next block
of a native loop is exempt only if the one it follows was, and only if it too
was created by the sender below it.

- [ ] **Step 5: The superclass chain, walked iteratively**

In `instantiate.rs`, replace `run_initializer_level`'s recursion with one
function that walks the chain with an explicit list — the order of every
Newspeak-visible effect is unchanged: each level's arity check and clause
message from the class upward, then the initializers from the top down.

```rust
    /// Class Instantiation steps 2-5 for every level of `class`'s superclass
    /// chain (`newspeak-spec.pdf` §6.2). Walked with an explicit list rather
    /// than by recursing per level: the chain's length is an image's to
    /// choose, and a level with no initializer code pushes no activation, so
    /// neither depth bound would see a long chain of them
    /// (`docs/design/execution.md`, "The trampoline (1b)").
    ///
    /// Upward first: each level's factory arguments are checked against its
    /// declared arity, then its superclass clause's message is evaluated to
    /// the superclass's arguments (step 3's first half). Then downward, from
    /// the topmost level: each level's own slot initializers and `initExprs`
    /// run in an activation of their own, with that level's own arguments
    /// (steps 2, 4 and 5) -- one activation per initializing level, so a
    /// superclass's initializers see the arguments *its* clause supplied.
    fn run_initializer_levels(
        &mut self,
        class: ClassRef,
        instance: &Value,
        factory_args: Vec<Value>,
    ) -> Result<(), Signal> {
        let mut levels: Vec<(ClassRef, Vec<Value>)> = Vec::new();
        let mut level = class;
        let mut args = factory_args;
        loop {
            self.check_factory_arity(level, &args)?;
            let next = match self.heap.class_superclass(level) {
                Some(superclass) => {
                    let super_args =
                        self.eval_superclass_message(level, superclass, instance, &args)?;
                    Some((superclass, super_args))
                }
                None => None,
            };
            levels.push((level, args));
            match next {
                Some((superclass, super_args)) => {
                    level = superclass;
                    args = super_args;
                }
                None => break,
            }
        }
        for (level, args) in levels.into_iter().rev() {
            self.run_own_initializer(level, instance, args)?;
        }
        Ok(())
    }
```

where `check_factory_arity` is the arity check `run_initializer_level` opened
with (its comment moves with it), and `run_own_initializer(class, instance,
args)` is the rest of `run_initializer_level` from Task 4: return `Ok(())`
when `class_has_initializer_code` is false, otherwise push the level's
activation through `reenter`. `instantiate_unchecked` calls
`run_initializer_levels`.

- [ ] **Step 6: Measure the native cost of one re-entry**

Temporarily, in `reenter`, just before `crate::vm::run(self, act)`:

```rust
        let marker = 0u8;
        if self.native_depth == 1 || self.native_depth == 101 {
            eprintln!("reenter {} at {:#x}", self.native_depth, &marker as *const u8 as usize);
        }
```

Build the debug binary and run, on the main thread for now, three programs
that recurse 150 levels deep through each re-entering path — `do:`,
instantiation (the `Tower` program above with a counter slot that stops at 150)
and a nested-class accessor chain if one can be written to recurse; skip the
accessor if it cannot be made to recurse without a cyclic-definition error, and
say so. For each, bytes per re-entry = (address at 1 − address at 101) / 100.
Take the largest, and choose the interpreter thread's stack as the smallest
power of two, in MiB, that is at least `2 × MAX_NATIVE_DEPTH × largest` — the
factor two is margin for the frames between re-entries and for the
difference between the measured path and the worst one. Remove the
instrumentation. Record the three figures and the chosen size for Task 8.

- [ ] **Step 7: The CLI's interpreter thread**

In `crates/ns-cli/src/main.rs`, move the body of `main`'s `match` into
`fn execute(command: Cmd) -> ExitCode`, and make `main`:

```rust
/// The native stack the interpreter runs on. Sends no longer recurse on it,
/// but the paths that re-enter the run loop from Rust -- `do:`, instantiation,
/// the nested-class accessor -- still do, up to
/// `ns_runtime::interpreter::MAX_NATIVE_DEPTH` deep. This is that many
/// re-entries at the cost measured for a debug build, twice over
/// (`docs/design/execution.md`, "Two bounds, not one"). Reserved, not
/// committed: only what a program uses is backed by memory.
const INTERPRETER_STACK_BYTES: usize = <measured> * 1024 * 1024;

fn main() -> ExitCode {
    let command = Cli::parse().command;
    // The whole command runs on the interpreter's thread, not only its sends:
    // the symbol interner is thread-local, so every symbol a command interns
    // has to be interned where it is used.
    let worker = std::thread::Builder::new()
        .name("ns-next".to_string())
        .stack_size(INTERPRETER_STACK_BYTES)
        .spawn(move || execute(command))
        .expect("spawn the interpreter thread");
    // A panic on that thread has already been reported by the panic hook;
    // exit with the status a panic on the main thread would have had.
    worker.join().unwrap_or_else(|_| ExitCode::from(101))
}
```

- [ ] **Step 8: Run the tests**

Run: `cargo test -p ns-runtime --test trampoline`,
`cargo test -p ns-cli --test program_outcomes`, then the whole gate list.
Expected: all PASS. The two native-limit outcomes end in their error with exit
status 1 in the debug build the suite runs; that is the test that pins the
thread's size. If either still aborts, the measurement in Step 6 missed the
worst path — find it, do not just double the constant.

- [ ] **Step 9: Measure**

Protocol against this task's parent. Expected: no slower — the count is one
compare and one increment per counted push. Report the table.

- [ ] **Step 10: Commit**

```
feat: bound recursion depth with two counted limits

A push past 100,000 counted activations raises "stack overflow:
recursion too deep", and a re-entry of the run loop from Rust past
1,000 deep raises "stack overflow: too many nested native calls" --
ordinary Newspeak errors, alike under both lowerings. A block a
control-flow native activates for a SendInlinable is exempt only when
the sending activation created it. Instantiation walks the superclass
chain with an explicit list, and the CLI runs each command on a thread
whose stack is sized for the second limit.
```

---

### Task 7: The loop keeps the running activation's register base

**Files:**
- Modify: `crates/ns-runtime/src/heap.rs` (`Window`, `window`, `window_local`, `window_set_local`)
- Modify: `crates/ns-runtime/src/vm.rs` (the loop holds a `Window` beside `act`)

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Copy)]
  pub struct Window { base: u32, #[cfg(debug_assertions)] act: ActivationRef }
  impl Heap {
      pub fn window(&self, act: ActivationRef) -> Window;
      pub fn window_local(&self, window: Window, index: u16) -> Value;
      pub fn window_set_local(&mut self, window: Window, index: u16, value: Value);
  }
  ```

- [ ] **Step 1: The window**

In `heap.rs`, beside the `activation_*` register accessors:

```rust
/// A running activation's register window: where its registers begin in the
/// stack's `values`. An entry's base is fixed for its whole life and
/// everything is addressed by index, so the run loop fetches the window once
/// when it switches to an activation instead of on every register access
/// (`docs/design/execution.md`, "What 1a left for it"). A debug build keeps
/// the activation beside it, so every access still checks the handle and the
/// window's bounds.
#[derive(Clone, Copy)]
pub struct Window {
    base: u32,
    #[cfg(debug_assertions)]
    act: ActivationRef,
}

impl Heap {
    pub fn window(&self, act: ActivationRef) -> Window {
        self.check_activation(act);
        Window {
            base: self.stack.entries[act.index as usize].base,
            #[cfg(debug_assertions)]
            act,
        }
    }

    pub fn window_local(&self, window: Window, index: u16) -> Value {
        #[cfg(debug_assertions)]
        {
            self.check_activation(window.act);
            let _ = self.register_slot(window.act.index, index);
        }
        self.stack.values[window.base as usize + index as usize]
    }

    pub fn window_set_local(&mut self, window: Window, index: u16, value: Value) {
        #[cfg(debug_assertions)]
        {
            self.check_activation(window.act);
            let _ = self.register_slot(window.act.index, index);
        }
        self.stack.values[window.base as usize + index as usize] = value;
    }
}
```

(put the methods in the existing `impl Heap` block rather than a second one).

- [ ] **Step 2: The loop uses it**

In `vm.rs`, `run` keeps `let mut window = interp.heap.window(act);` beside
`act` and refreshes it on every `Flow::Switch`; `execute` and `send` take it
and use `interp.heap.window_local(window, r)` / `window_set_local` wherever
they read or wrote `act`'s own registers through `read_local`/`write_local`.
`resume_caller` writes the caller's `dst` through `write_local` as before — it
runs once per return, not per instruction. Outer-local access, `InitSlot`'s
receiver and `slot_base`, `MakeClosure` and `NonLocalReturn` are unchanged.

- [ ] **Step 3: Test and measure**

Run the whole gate list. Expected: green.

Measure against this task's parent. Keep the change only if at least one source
is faster and none is slower; otherwise revert it (`git checkout -- crates`),
and record in the report — for Task 8 to carry into execution.md — that caching
the base did not pay, with the table.

- [ ] **Step 4: Commit (only if kept)**

```
perf: keep the running activation's register base in the run loop
```

---

### Task 8: Measure the whole step and bring the documents to the finished state

**Files:**
- Modify: `docs/design/execution.md`, `docs/design/compiler.md`,
  `docs/design/core-library.md`, `docs/design/image-format.md`,
  `docs/design/memory.md` (only where it describes a push site popping),
  `docs/design/README.md`, `docs/PROGRESS.md`, `CLAUDE.md`

- [ ] **Step 1: The whole-step measurement**

Protocol, with "before" = the commit the branch's 1b work started from (the
parent of Task 2's commit) and "after" = HEAD. Three rounds as always; if a
source's groups overlap, three more rounds for that source.

- [ ] **Step 2: Byte identity**

The optimized lowering emits the same instructions as before, so every
`build` and `snapshot` artifact of it must be byte-identical between the two
binaries of Step 1:

```powershell
$n = 0; $diff = 0
foreach ($f in Get-ChildItem tests-fixtures\*.ns) {
  foreach ($bin in @{ name = "before"; path = $before }, @{ name = "after"; path = $after }) {
    & $bin.path build $f.FullName "$env:TEMP\$($f.BaseName).$($bin.name).nsb" 2>$null | Out-Null
    & $bin.path snapshot $f.FullName "$env:TEMP\$($f.BaseName).$($bin.name).img" 2>$null | Out-Null
  }
  foreach ($ext in "nsb", "img") {
    $a = "$env:TEMP\$($f.BaseName).before.$ext"; $b = "$env:TEMP\$($f.BaseName).after.$ext"
    if ((Test-Path $a) -and (Test-Path $b)) {
      $n++
      if ((Get-FileHash $a).Hash -ne (Get-FileHash $b).Hash) { $diff++; "differs: $a" }
    }
  }
}
"compared $n artifacts, $diff differing"
```

Expected: `0 differing`. A reference-lowering artifact differs by construction
(`SendInlinable`) and is not compared. Delete the temporary files afterwards.

- [ ] **Step 3: Suite running time**

Run `cargo test --workspace` twice on HEAD and once on the "before" worktree
(debug), and note the wall times: the boundary tests push a hundred thousand
activations under both lowerings, and the report should say what that costs.

- [ ] **Step 4: The documents**

- `execution.md`: "Planned: the trampoline" becomes "The trampoline", present
  tense, keeping its reasoning and the prior-art table; "What 1a left for it"
  goes (each item is done — or, for the base, recorded as measured and
  reverted); "The VM" describes the loop — `vm::run`, the decision/`Step`
  split, the continuations, `reenter` and what comes through it, and that the
  loop pops what it runs; "Lifecycle" and "Signatures" say the loop pops,
  not "every site that pushes"; "Non-local return" describes the unwind in
  `vm::unwind` and `catches_return`; "Where `pc` is stored" gains that the loop
  resumes a caller from its stored `pc`; the measurements section gains "What
  the trampoline moved" (the whole-step table, the per-task tables, the byte
  identity count, the native cost per re-entry from Task 6 and the chosen
  thread size, and the suite's running time); the Decisions bullet loses
  "Planned".
- `compiler.md`: "How the limit counts" in the present tense, naming
  `Interpreter::push_step` as where the exemption is decided and the boundary
  tests in `crates/ns-runtime/tests/trampoline.rs`; "Until the trampoline"
  goes; "Status" and the Decisions bullet lose "Planned"; "What it emits" item
  4 says every send the optimized lowering inlines is a `SendInlinable`.
- `core-library.md`: the table's control-flow rows say the native *is* a
  trampoline step.
- `image-format.md`: the paragraph that says loading a binary runs its code
  "with recursion depth bounded only by the native stack until the VM is
  trampolined" now says depth is bounded by the two limits; keep that a mutated
  binary that decodes cleanly can still abort while executing (a native
  indexing `args` it was not given, for one).
- `memory.md`: if it says the four push sites pop, say the loop pops.
- `README.md`: the execution row's "Planned next" loses "the trampoline".
- `CLAUDE.md`: in "An image is untrusted input", replace "recursion depth is
  bounded only by the native stack until the VM is trampolined (see …)" with
  the two limits; in "A running activation is an `ActivationRef`", replace
  "Every site that pushes an activation pops it on every exit, error paths
  included -- `Interpreter::send` and `instantiate` debug-assert that the stack
  is balanced" with "The run loop pops every activation it runs, on every exit,
  error paths included (`vm::run`) -- `Interpreter::send` and `instantiate`
  debug-assert that the stack is balanced".
- `PROGRESS.md`: a milestone entry "M4 — The trampoline (date)" in the style of
  the others — what changed in plan order, what was deleted (`run_code`,
  `run_step`, `invoke_method`, `grow_activation_temps`, `run_while`,
  `truthy_arg`, `collect`), the measurements and byte identity, the test count,
  and "Left, deliberately" (instantiation, the accessor and `do:` as
  re-entries; the unchecked `args[0]`); M4's row and "Where the next session
  starts" move to step 2, the root discipline, and the paragraph about
  profiling `bench_block.ns` goes (Task 1 answered it).

Check every document against the code rather than against this plan: a claim
in the present tense must be true of HEAD.

- [ ] **Step 5: Gates and commit**

Run the whole gate list.

```
docs: record the trampoline as built and measured
```
