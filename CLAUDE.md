# ns-next

A native Rust runtime for [Newspeak](https://newspeaklanguage.org/): lexer, parser,
resolver, tree-walking interpreter, bytecode compiler and register VM, and a versioned
image format. Rust 1.98.0, edition 2021, three crates:

- `crates/ns-syntax` — lexer, recursive-descent + Pratt parser, AST, `Span`.
- `crates/ns-runtime` — resolver, object model, dispatch, both execution engines
  (`interpreter/` and `compiler/` + `vm.rs`), core library (`builtins/`), image format
  (`image/`).
- `crates/ns-cli` — the `ns-next` binary: `parse`, `parse-corpus`, `run`, `disasm`,
  `snapshot`, `load`.

Read `docs/PROGRESS.md` first — it is the milestone-level status, and it links the
design spec and implementation plan behind each milestone.

## Invariants that are easy to break

These are not style preferences. Each one has already produced a real defect in this
repository; the reasoning behind them is in the design specs under
`docs/superpowers/specs/`.

**Two engines must agree byte for byte.** `ns-next run` defaults to `--engine=bytecode`;
`--engine=tree-walk` is the Milestone 1 interpreter, kept permanently as a differential
oracle. For every program both engines must produce identical exit status, stdout **and
stderr**. `crates/ns-cli/tests/differential.rs` enforces this over every
`tests-fixtures/*.ns` plus inline sources, with no exemptions. If a change makes the
engines disagree, the tree-walker is the reference — fix the compiler or the VM.

**`interpreter/` is shared orchestration, not the tree-walker's private code.** Send,
`doesNotUnderstand:`, lexical/super/native dispatch, frame construction and `instantiate`
serve both engines; the engine differs only at three body-execution hooks (method body,
block body, instance initializer). Never copy dispatch logic, lookup logic or error text
out of `interpreter/` into `vm.rs` or `compiler/` — that is how the two engines drift and
stderr stops matching. If you need something private there, widen it to `pub(crate)`.

**Compiling an already-resolved module can never raise a `NewspeakError`.** Compilation
is a total, deterministic lowering; a compiler-invariant violation is `panic!` /
`debug_assert!`. A *program-level* error that Milestone 1 raises at evaluation time is
deferred to execution instead (see `Literal::MalformedInt` and
`Literal::UnresolvedIdent`), so both engines report it identically.

**An expression's value register must still hold that value when its consumer executes.**
A named local's register is live storage. `lower_operand` copies it when anything lowered
afterwards can write it; `lower_expr_consumed_immediately` is the counterpart for the
positions where nothing is lowered afterwards. Getting this wrong is silent wrong output
with exit status 0, not a crash.

**An image is untrusted input.** No byte sequence may cause a panic or a process abort:
validate every id, tag, count and length before it indexes, allocates or slices, and
bound any recursion whose depth the input controls. `image::load` also runs two
structural verification passes before installing the engine. See the design spec's
"Error Handling" section for the scope of what they do and do not guarantee.

**Never `{:?}` a `Value`, and never `.unwrap()` / `.expect()` / `{:?}` an `EvalResult`.**
The bootstrap `Object` class is its own `.class`, so `Debug` on a `Value::Obj` recurses
forever, and `Signal` deliberately has no `Debug` (it holds a `FrameRef` cycle). Tests
unwrap through a local `expect_ok` helper — copy the one in a neighbouring test file.

## Working in this repository

```bash
# build every crate
cargo build --workspace
# the full suite, including the differential harness that compares the two engines
cargo test --workspace
# before every commit
cargo fmt --all
# must be warning-free
cargo clippy --workspace --all-targets
# non-regression gate: must report "parsed 210, failed 0, expected-fail 1, stale 0"
cargo run -p ns-cli -- parse-corpus corpus/modules
```

Never judge performance from a debug build — `cargo build --workspace --release` is the
one to measure.

A file that legitimately cannot parse yet belongs in `corpus/blacklist.toml`. Nothing
under `crates/ns-syntax` should change for runtime work.

Add no new external dependency without a reason that survives review — the image format
is hand-rolled on purpose.

## Conventions

- All code, identifiers, comments, doc comments, commit messages and developer
  documentation are in English.
- Classic terminology: `blacklist`, `whitelist`, `sane` — not their newer replacements.
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`, `chore:`).
- Specs and plans live in `docs/superpowers/{specs,plans}/` and are named
  `YYYY-MM-DD-HHMM-<name>.md`, so several files from one day sort chronologically.
- Comments should explain the decision, not restate the mechanics. The reason a check
  exists — what it prevents — is the part a later reader cannot reconstruct.
