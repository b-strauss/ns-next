# ns-next

A native Rust runtime for [Newspeak](https://newspeaklanguage.org/): lexer, parser,
resolver, tree-walking interpreter, bytecode compiler and register VM, and a versioned
image format. Rust 1.98.0, edition 2021, three crates:

- `crates/ns-syntax` — lexer, recursive-descent + Pratt parser, AST, `Span`.
- `crates/ns-runtime` — resolver, object model, dispatch, both execution engines
  (`interpreter/` and `compiler/` + `vm.rs`), core library (`builtins/`), image format
  (`image/`).
- `crates/ns-cli` — the `ns-next` binary: `parse`, `parse-corpus`, `run`, `run-corpus`,
  `disasm`, `snapshot`, `load`.

Read `docs/PROGRESS.md` first — it is the milestone-level status, and it links the
design spec and implementation plan behind each milestone.

## Invariants that are easy to break

These are not style preferences. Each one has already produced a real defect in this
repository; the reasoning behind them is in the design specs under
`docs/superpowers/specs/`.

**Two engines must agree byte for byte.** `ns-next run` defaults to `--engine=bytecode`;
`--engine=tree-walk` is the Milestone 1 interpreter, kept permanently as the differential
oracle. Two engines that must agree is a verification property the project funds on
purpose, not a transitional convenience — when the moving GC lands, the tree-walker gets
its own handle-based root discipline rather than being retired. For every program both
engines must produce identical exit status, stdout **and stderr**.
`crates/ns-cli/tests/differential.rs` enforces this over every `tests-fixtures/*.ns` plus
inline sources, with no exemptions. If a change makes the engines disagree, the
tree-walker is the reference — fix the compiler or the VM.

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

**A class object is per enclosing instance; behaviour is the `MixinApplication`, shared.**
`ClassData` carries identity and an `enclosing` object; method lookup, slots and layout
live on `MixinApplication`, one per (mixin, superclass application) pair. **The
application cache is keyed by the superclass's *application*, never by its class
object** — a nested superclass's class object is itself per-instance, so a class-object
key would never hit and every instance would mint its own application: wrong per §6.2,
and every call site megamorphic since no two instances would ever share behaviour.

**Compiled code is keyed by mixin, and `Instr::InitSlot`'s `own_index` is mixin-relative.**
One mixin backs every application of it, so code is compiled once per mixin and reused
across every application; an absolute field index baked into that shared code would be
wrong the moment a second application inserts different inherited slots ahead of it. The
VM resolves `own_index` against the executing `Frame.slot_base`
(`MixinApplication.own_slot_base` for that level) at run time. The tree-walker must
resolve the same slot by name instead of by index — it has no compiled `InitSlot` to
share — so a bug here shows up as the two engines disagreeing, not as a single wrong
answer.

**`outer` walks the *lexical* declaration chain, never the receiver's dynamic class.**
`Frame.current_class` seeds it and `ClassData::declaring_class` continues it level by
level. A method or nested-class accessor inherited across a nesting boundary runs with
the class that lexically declares it, not the receiver's own class — using the receiver's
class instead silently resolves `outer` to the wrong enclosing object.

**The tree-walking engine cannot run from a loaded image.** `image::load` decodes mixins
and class objects, never AST-carrying bindings, so there is no source for the
tree-walker to execute; `Interpreter::set_engine(Engine::TreeWalk)` refuses on a
loaded image rather than running it against the wrong data.

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
# non-regression gate: must report "ran 6, failed 0, skipped 205"
cargo run -p ns-cli -- run-corpus corpus/modules
```

Never judge performance from a debug build — `cargo build --workspace --release` is the
one to measure.

A file that legitimately cannot parse yet belongs in `corpus/blacklist.toml`. A corpus
module that runs end to end belongs in `corpus/runnable.toml`, with the entry selector
to send and the output it produces; `run-corpus` runs every listed module under both
engines and fails if they disagree. Nothing under `crates/ns-syntax` should change for
runtime work.

`platform.rs` is widened only on demand: an accessor exists because a module on
`corpus/runnable.toml` needs it, and the whitelist entry that demanded it is named in the
accessor's comment. A capability nothing on that list asks for has no test that would
catch it rotting.

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
