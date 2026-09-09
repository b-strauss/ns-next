# ns-next

A native Rust runtime for [Newspeak](https://newspeaklanguage.org/): lexer, parser,
resolver, tree-walking interpreter, bytecode compiler and register VM, and a versioned
image format. Rust 1.98.0, edition 2021, three crates:

- `crates/ns-syntax` — lexer, recursive-descent + Pratt parser, AST, `Span`.
- `crates/ns-runtime` — resolver, object model, dispatch, both execution engines
  (`interpreter/` and `compiler/` + `vm.rs`), core library (`builtins/`), image format
  (`image/`). Objects and activations live in arenas owned by one `Heap`
  (`heap.rs`) and are addressed by 32-bit handles; nothing reclaims a slot yet.
- `crates/ns-cli` — the `ns-next` binary: `parse`, `parse-corpus`, `run-corpus`, `run`,
  `snapshot`, `load`, `bench`, `disasm`, `inspect`, `build`.

Read `docs/design/README.md` first — it maps the living design documents and says
how much of each aspect exists. `docs/PROGRESS.md` is the milestone chronology
beside it.

## Invariants that are easy to break

These are not style preferences. Each one has already produced a real defect in this
repository; the reasoning behind them is in the design documents under
`docs/design/`.

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
structural verification passes before installing the engine. See
`docs/design/image-format.md`'s "Verification" section for the scope of what they
do and do not guarantee. One thing to keep in mind while decoding: the decoder
allocates one placeholder object and activation per declared record before it can
validate any of them, and the arena never reclaims a slot, so a rejected image
holds its placeholders until the load finishes. That is a transient peak, not a
leak — `image::load` owns its `Interpreter` in a local, so every `Err` path drops
it and both arenas with it — but the peak is amplified against the file: an
`Object` is 64 bytes against a 6-byte minimum record and a `Frame` is 128 against
19, so roughly 10.7x and 6.7x. `bounded_count` is what keeps that proportional to
the file's size rather than to a `u32` — do not add a pre-allocation that is not
bounded that way.

That guarantee covers **decoding**. `ns-next load` on a stateless binary instantiates
and sends the recorded entry with no flag at all, so loading one *runs its code* --
with the same exposure as running the equivalent source, including that recursion
depth is bounded only by the native stack until the VM is trampolined (see
`docs/design/execution.md`, "Planned: activations on a stack, reified on
demand"). A mutated binary that decodes cleanly can still abort while executing,
and that is not a decoder defect.

**A wire tag is assigned once and never renumbered or reused.** Every instruction,
literal and graph-section tag is declared in `crates/ns-runtime/src/image/opcodes.rs`
and read from there by the encoder, the decoder and `inspect`'s structural walk alike;
`crates/ns-runtime/tests/opcode_tags.rs` pins each value. A new tag takes the next free
number, a removed one retires its number. This is stricter than the rest of the format
needs — images are disposable and `FORMAT_VERSION` is never bumped — because
renumbering fails in the one way the untrusted-input guarantee does not cover: a stale
artifact would decode into a *different, perfectly valid* program and run it, reporting
nothing. Retiring numbers means those bytes hit an unknown tag and are refused.

**A handle means nothing against a `Heap` that did not mint it.** An `ObjRef`, a
`ClassRef` and a `FrameRef` are indices into one `Heap`'s arenas. Never build a
`Heap::new()` to stand in for one that is out of scope, and never hold a handle
across the death of the interpreter that allocated it: the index is in range in the
other heap too, so the read answers a **different, perfectly valid object** rather
than failing. Reach the real `Heap` instead: `compiler::compile_module` and
`compile_initializer` take a `&Heap` parameter for exactly this reason, and
`ns-cli`'s `cmd_disasm` — which builds its own class object and so has no
parameter to take — reads through `interp.heap_ref()` while that interpreter is
still alive, before it goes out of scope. A `debug_assertions`-only origin guard
turns the mistake into a panic at
the read, so a debug-build test run catches it; a release build does not, which is
why this is an invariant and not a check to rely on.

**Never `{:?}` a `Value`, and never `.unwrap()` / `.expect()` / `{:?}` an `EvalResult`.**
`ObjRef` and `FrameRef` each carry a hand-written `Debug` that prints identity only, so
`Debug` on a `Value::Obj` no longer recurses — but it still renders Rust internals such
as `ObjRef(#42)`, which is useless in a Newspeak-facing message where `Heap::describe`
answers "the String \"foo\"". `Signal` deliberately has no `Debug` impl of its own, which
means `EvalResult` (`Result<Value, Signal>`) has none either: `Result::unwrap`/`expect`
require `E: Debug`, so calling either — or writing `{:?}` on an `EvalResult` — is a
compile error, not a lapse a reviewer has to catch by hand. Tests unwrap through a local
`expect_ok` helper — copy the one in a neighbouring test file.

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
- Design documents live in `docs/design/` and are living: update the document for
  the aspect that changed rather than adding a dated file that supersedes it.
  Implementation plans live in `docs/superpowers/plans/`, are named
  `YYYY-MM-DD-HHMM-<name>.md` so several from one day sort chronologically, and
  are execution records — left as written once complete.
- Comments should explain the decision, not restate the mechanics. The reason a check
  exists — what it prevents — is the part a later reader cannot reconstruct.
- **Object and frame access goes through `crates/ns-runtime/src/heap.rs`, not
  directly through a handle.** `ObjRef`, `ClassRef` and `FrameRef` are defined
  in `heap.rs` and their inner arena index is private to that module, so the
  compiler refuses every read of one from anywhere else — there is no grep gate
  to remember and no way to widen a field to reach past an accessor. If an
  accessor you need is missing, add it there. The accessors copy out rather
  than lend out on purpose; see `docs/design/memory.md`, "The seam".
