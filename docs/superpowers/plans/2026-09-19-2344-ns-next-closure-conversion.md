# Closure Conversion — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Under the optimized lowering, a block stops reaching its method's
variables through the method's frame: a captured variable that is never
written after capture is copied into the closure, a written one lives in its
scope's indirection vector, a returning frame keeps its registers only when
something still reads them by reference, and the protective register copies
the old model forced become unnecessary — measured, with the reference
lowering kept by-reference as the oracle.

**Architecture:** A pre-pass of the optimized lowering (`compiler/capture.rs`)
classifies every local of a code family and says what each real block is handed
when it is created. The lowering turns that into copied values in
`MakeClosure`, an `Array` per scope built by `MakeVector` and read and written
by `LoadVector`/`StoreVector`, and — inside the fallback closures an inlined
conditional sends to a non-Boolean receiver — the by-reference instructions it
otherwise stops emitting. `ClosureData` carries the copied values and stays 32
bytes. Whether a returning activation copies its registers into its frame is
decided per activation, from whether it ever created a closure that reads
enclosing frames by reference.

**Tech Stack:** Rust 1.98.0, edition 2021, the three workspace crates; no new
dependency.

**Spec:** `docs/design/execution.md`, "Planned: closure conversion" (committed
in `891de70` and amended in `ec546dd`), with its subsections "The closure keeps
its defining activation", "Where the analysis lives", "Fallback blocks",
"Representation", "The image format", "Fewer protective copies", "Verification
and the reference lowering" and "Cost", and the five decisions this step added
to that document's "Decisions". The measurement protocol is the same
document's "What removing the re-entries moved" and "What 3c moved".

## Global Constraints

- Work on the branch `feature/closure-conversion`, already created from
  `master` at `efac7d2` and holding the design commits `891de70` and
  `ec546dd`, on the checked-out tree — no `git worktree` for the work itself
  (throwaway worktrees for the measurement's other binaries are fine and are
  removed afterwards). This plan is committed on the branch before Task 1.
- The baseline for the measurement is **`efac7d2`**, today's `master`.
  `be3eb40` is carried beside it, as in every measurement since step 2.
- **Outcomes are pinned, not re-derived.** Task 1 records the new goldens on
  the unconverted tree. From Task 2 on, any golden, `tests/fixtures` outcome
  or `run-corpus` row that changes is a finding: stop and report it with the
  old and the new output and the rule that decides between them. Never update
  a recorded answer because the code now produces it.
- Every new golden's `.ns` opens with a `(* ... *)` comment under the category
  string giving the rule, what the program shows and why the recorded answer
  is the right one (`docs/design/README.md`, "The goldens"). Its `.toml` holds
  the answer and nothing else.
- Names are written out (`CLAUDE.md`, "Conventions"): no identifier of one or
  two characters, no truncated word — in code, tests, goldens and commit
  messages alike. Clippy's `min_ident_chars` refuses the short ones. Newspeak
  test programs inline in Rust are `indoc! {"…"}` blocks.
- No dispatch logic in `vm.rs` or `compiler/`. The two new run-time errors of
  Task 4 are built in `interpreter/locals.rs`, never in `vm.rs`.
- Compiling an already-resolved module never raises a `NewspeakError`: a
  lowering or an analysis that meets an impossible shape panics or
  `debug_assert!`s, as the existing `"internal: …"` panics do.
- **A wire tag is assigned once and never renumbered or reused.** Every tag
  this plan adds takes the next free number and every tag whose payload
  changes is retired (`image/opcodes.rs`, `tests/opcode_tags.rs`).
- An image is untrusted input: no byte sequence may panic, in a debug build
  either. Every count this plan adds to the format is validated before it
  indexes or allocates, and `bounded_count` bounds anything that allocates
  per record.
- **Never `{:?}` a `Value`**, never `.unwrap()`/`.expect()` an
  `EvaluationResult` or a `HostResult`; tests unwrap through the local
  `expect_ok` helper of the neighbouring test file.
- A handle lives only where the tracer can rewrite it: `ClosureData`'s new
  `copied` values are traced by `trace.rs`, and the compiler refuses the
  field until the `Trace` impl names it.
- Comments explain the decision, not the mechanics.
- Every commit passes `cargo fmt --all`, `cargo clippy --workspace
  --all-targets` warning-free, and `cargo test --workspace`. Before the last
  commit of each of Tasks 1–6 also run, in Git Bash:
  `NS_NEXT_STRESS_MOVES=1 cargo test --workspace` (about three minutes),
  `NS_NEXT_STRESS_CACHES=1 cargo test --workspace` (about one minute),
  `cargo run -p ns-cli -- parse-corpus corpus/modules` (must report
  `parsed 210, failed 0, expected-fail 1, stale 0`),
  `cargo run -p ns-cli -- run-corpus corpus/modules`,
  `NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules` and
  `NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules`
  (all three must report `ran 6, failed 0, skipped 205`).
- Conventional commits (`feat:`, `fix:`, `perf:`, `test:`, `docs:`).
  **Every commit message ends with two trailer lines**, the first naming the
  model that actually wrote the commit's code (a subagent names its own model,
  not the controller's):

  ```
  Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01F6LkMsqsEmuNAPegC4eL3K
  ```

- Never override the git author or committer identity.
- Not in scope: removing the reference lowering (its removal is recorded as
  intended, not done); a collector; changing what a fallback path does
  observably; the debugger's own use of `enclosingActivation`; making the
  indirection vector a dedicated object kind or giving it inline storage.

## File Structure

| File | Responsibility in this plan |
|---|---|
| `goldens/capture/*` | the recorded outcomes of every capture shape (Task 1) |
| `docs/design/README.md` | the new golden category's row (Task 1); the status line (Task 8) |
| `crates/ns-runtime/src/compiler/capture.rs` | the capture analysis: `BodyKey`, `Place`, `Captured`, `BodyCaptures`, `Captures`, `analyze` (Task 2) |
| `crates/ns-runtime/src/compiler/mod.rs` | runs the analysis per root body and hands it to the lowering (Tasks 2, 5) |
| `crates/ns-runtime/src/types.rs` | `ClosureData`'s new shape (Task 3) |
| `crates/ns-runtime/src/heap.rs` | closure accessors, the retention bit, the conditional divorce, bounds-safe dead-frame registers, filling copied values (Task 3) |
| `crates/ns-runtime/src/interpreter/mod.rs` | a block activation's arity and copied values (Task 3) |
| `crates/ns-runtime/src/interpreter/locals.rs` | `home_of`'s replacement, `load_vector`/`store_vector` and their error (Tasks 3, 4) |
| `crates/ns-runtime/src/bytecode.rs` | `MakeClosure`'s `copied`, the three vector instructions, `copied_count`, `outer_reach`, disassembly (Tasks 3, 4) |
| `crates/ns-runtime/src/vm.rs` | executing them (Tasks 3, 4) |
| `crates/ns-runtime/src/trace.rs` | tracing a closure's copied values (Task 3) |
| `crates/ns-runtime/src/image/opcodes.rs` | the retired and the new tags (Tasks 3, 4) |
| `crates/ns-runtime/src/image/encode.rs`, `decode.rs`, `inspect.rs` | the new records, their verification and the structural walk (Tasks 3, 4) |
| `crates/ns-runtime/src/compiler/lower.rs` | the conversion itself: `Layout`, scopes, variable access, `MakeClosure`, the fallback mode (Task 5); the protective copies (Task 6) |
| `crates/ns-runtime/src/compiler/reference.rs` | the empty `copied` list and the instruction lists it enumerates (Tasks 3, 4) |
| `crates/ns-runtime/tests/*` | image, heap, VM and lowering tests (Tasks 3–6) |
| `docs/design/execution.md`, `compiler.md`, `image-format.md`, `memory.md`, `docs/PROGRESS.md`, `CLAUDE.md` | what was built and measured (Task 8) |

---

### Task 1: Record what the capture shapes answer today

**Files:**
- Create: `goldens/capture/copied_and_vector_variables.ns` and `.toml`
- Create: `goldens/capture/capture_in_inlined_loops.ns` and `.toml`
- Create: `goldens/capture/nested_blocks_carry_variables.ns` and `.toml`
- Create: `goldens/capture/closures_that_outlive_their_method.ns` and `.toml`
- Create: `goldens/capture/captures_in_initializers.ns` and `.toml`
- Create: `goldens/capture/fallback_closures_share_variables.ns` and `.toml`
- Modify: `docs/design/README.md` (the golden-category table)

**Interfaces:**
- Consumes: nothing. This task runs on the unconverted tree.
- Produces: `goldens/capture/`, the category every later task's
  `cargo test --workspace` runs under both lowerings.

- [ ] **Step 1: Write the first golden**

`goldens/capture/copied_and_vector_variables.ns`:

```
Newspeak3
'Test'
(* Closure conversion (docs/design/execution.md, "Planned: closure
   conversion") decides per captured mutable local whether a block gets a
   copy of it or shares its scope's indirection vector. These are the shapes
   that decision is made on, recorded before the conversion exists, so that
   the conversion is held to them.

   setOnceThenCaptured: written before the block is created and never
   after -- a copy answers 7 just as a shared variable does.
   writtenAfterCapture: the write happens after the block exists, so the
   block must see 2, which a copy would not.
   writtenByTheBlock: the block itself writes, so method and block share.
   immutableCaptured: an immutable local is never written (3.6).
   vectorVariableWithInitializer: a declared initializer runs before the
   block exists and the block then writes, so the method reads 30. *)
class CopiedAndVectorVariables usingPlatform: platform = Object new (
	| transcript = platform transcript. |
) (
	setOnceThenCaptured = (
		| x ::= 0. |
		x:: 7.
		^[x]
	)
	writtenAfterCapture = (
		| x ::= 1. reader ::= nil. |
		reader:: [x].
		x:: 2.
		^reader
	)
	writtenByTheBlock = (
		| count ::= 0. bump ::= nil. |
		bump:: [count:: count + 1].
		bump value.
		bump value.
		^count
	)
	immutableCaptured = (
		| five = 5. |
		^[five + 1]
	)
	vectorVariableWithInitializer = (
		| x ::= 3. scale ::= nil. |
		scale:: [x:: x * 10].
		scale value.
		^x
	)
	public main: platform args: arguments = (
		transcript show: self setOnceThenCaptured value printString; newLine.
		transcript show: self writtenAfterCapture value printString; newLine.
		transcript show: self writtenByTheBlock printString; newLine.
		transcript show: self immutableCaptured value printString; newLine.
		transcript show: self vectorVariableWithInitializer printString; newLine
	)
) : ()
```

`goldens/capture/copied_and_vector_variables.toml`:

```toml
stdout = '''
7
2
2
6
30
'''
```

- [ ] **Step 2: Run it under both lowerings**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: PASS. A failure here is a fact about the current runtime, not about
this plan: report the actual output and stop rather than editing the `.toml`
to match.

- [ ] **Step 3: Write the loop golden**

`goldens/capture/capture_in_inlined_loops.ns`:

```
Newspeak3
'Test'
(* The compiler inlines whileTrue: and whileFalse:, and an inlined block
   takes no temporaries, so every variable such a loop writes is its
   method's. A block created in one iteration is written by the next, so
   these variables are shared however early in the loop body the write
   stands -- textual order alone would answer the value of the iteration
   that created each block.

   loopBody: x is written at the top of the body and the block created
   below it; all three blocks answer the last value written, 2.
   loopCondition and loopConditionWhileFalse: the write is in the
   condition, which runs once more than the body, so all three blocks
   answer 3. writtenBeforeTheLoop: the write is outside the loop and
   before the capture, the case a copy is right for. readOnlyInABranch:
   an inlined ifTrue: block is not a block at run time, so reading x
   there captures nothing. *)
class CaptureInInlinedLoops usingPlatform: platform = Object new (
	| transcript = platform transcript. |
) (
	loopBody = (
		| index ::= 0. x ::= 0. blocks ::= nil. |
		blocks:: OrderedCollection new.
		[index < 3] whileTrue: [x:: index. blocks add: [x]. index:: index + 1].
		^blocks
	)
	loopCondition = (
		| index ::= 0. x ::= 0. blocks ::= nil. |
		blocks:: OrderedCollection new.
		[x:: index. index < 3] whileTrue: [blocks add: [x]. index:: index + 1].
		^blocks
	)
	loopConditionWhileFalse = (
		| index ::= 0. x ::= 0. blocks ::= nil. |
		blocks:: OrderedCollection new.
		[x:: index. index >= 3] whileFalse: [blocks add: [x]. index:: index + 1].
		^blocks
	)
	writtenBeforeTheLoop = (
		| x ::= 0. blocks ::= nil. |
		blocks:: OrderedCollection new.
		x:: 41.
		blocks add: [x + 1].
		^blocks
	)
	readOnlyInABranch = (
		| x ::= 4. |
		^true ifTrue: [x + 1] ifFalse: [0]
	)
	show: blocks = (
		blocks do: [:each | transcript show: each value printString; show: ' '].
		transcript newLine
	)
	public main: platform args: arguments = (
		self show: self loopBody.
		self show: self loopCondition.
		self show: self loopConditionWhileFalse.
		self show: self writtenBeforeTheLoop.
		transcript show: self readOnlyInABranch printString; newLine
	)
) : ()
```

`goldens/capture/capture_in_inlined_loops.toml`:

```toml
stdout = '''
2 2 2 
3 3 3 
3 3 3 
42 
5
'''
```

The trailing space on each of the first four lines is what `show: ' '`
writes after every element; keep it.

- [ ] **Step 4: Run it**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: PASS. If the runtime answers differently, report and stop.

- [ ] **Step 5: Write the nesting golden**

`goldens/capture/nested_blocks_carry_variables.ns`:

```
Newspeak3
'Test'
(* A block three levels down reads a variable of the method, so every
   block between them has to carry it -- a copy when nothing writes it
   after capture (first), the shared variable when something does
   (second). threeDeep answers 1 + 20 + 100 + 1000 + 10000; second is
   written after the blocks exist, so 20, not 10. innerWrites has the
   innermost block writing the method's variable, which every level shares.
   blockLocals captures a block's own temporary, one per activation of
   that block, each written before the inner block is created. *)
class NestedBlocksCarryVariables usingPlatform: platform = Object new (
	| transcript = platform transcript. |
) (
	threeDeep = (
		| first ::= 1. second ::= 10. outer ::= nil. |
		outer:: [:x | [:y | [:z | first + second + x + y + z]]].
		second:: 20.
		^((outer value: 100) value: 1000) value: 10000
	)
	innerWrites = (
		| total ::= 0. numbers ::= nil. |
		numbers:: OrderedCollection new.
		numbers add: 1; add: 2; add: 3.
		numbers do: [:each | [:factor | total:: total + (each * factor)] value: 10].
		^total
	)
	blockLocals = (
		| numbers ::= nil. makers ::= nil. |
		numbers:: OrderedCollection new.
		numbers add: 1; add: 2; add: 3.
		makers:: OrderedCollection new.
		numbers do: [:each | | doubled ::= 0. | doubled:: each * 2. makers add: [doubled]].
		^makers
	)
	public main: platform args: arguments = (
		transcript show: self threeDeep printString; newLine.
		transcript show: self innerWrites printString; newLine.
		self blockLocals do: [:each | transcript show: each value printString; show: ' '].
		transcript newLine
	)
) : ()
```

`goldens/capture/nested_blocks_carry_variables.toml`:

```toml
stdout = '''
11121
60
2 4 6 
'''
```

- [ ] **Step 6: Write the survival golden**

`goldens/capture/closures_that_outlive_their_method.ns`:

```
Newspeak3
'Test'
(* What a block may still do once its method has returned: read a copy,
   share a written variable with a second block, and return non-locally
   -- which is refused, because the home activation is gone (3.6, 7.2).
   A writer and a reader handed back together must go on seeing each
   other's value. findFirstAbove: returns out of a block whose home
   activation is still running and whose home has captured variables. *)
class ClosuresThatOutliveTheirMethod usingPlatform: platform = Object new (
	| transcript = platform transcript. |
) (
	writerAndReader = (
		| value ::= 0. |
		^{[:fresh | value:: fresh]. [value]}
	)
	adder: base = (
		^[:x | [:y | base + x + y]]
	)
	findFirstAbove: limit = (
		| seen ::= 0. numbers ::= nil. |
		numbers:: OrderedCollection new.
		numbers add: 3; add: 8; add: 12; add: 20.
		numbers do: [:each | seen:: seen + 1. each > limit ifTrue: [^seen]].
		^0
	)
	public main: platform args: arguments = (
		| pair |
		pair:: self writerAndReader.
		(pair at: 1) value: 42.
		transcript show: (pair at: 2) value printString; newLine.
		transcript show: (((self adder: 100) value: 20) value: 3) printString; newLine.
		transcript show: (self findFirstAbove: 10) printString; newLine
	)
) : ()
```

`goldens/capture/closures_that_outlive_their_method.toml`:

```toml
stdout = '''
42
123
3
'''
```

- [ ] **Step 7: Write the initializer golden**

`goldens/capture/captures_in_initializers.ns`:

```
Newspeak3
'Test'
(* An instance initializer's activation has the factory's parameters as
   its locals (6.2), and a slot initializer is one of its statements. A
   block built there captures those parameters, which no assignment can
   write (3.6), so they are copied. The block outlives the initializer:
   every send of add: runs it after the initializer has returned. *)
class CapturesInInitializers usingPlatform: platform = Object new (
	| transcript = platform transcript. |
) (
	class Adder base: base = Object new (
		| adder = [:x | x + base]. |
	) (
		public add: x = ( ^adder value: x )
	) : ()
	public main: platform args: arguments = (
		| adder |
		adder:: Adder base: 5.
		transcript show: (adder add: 10) printString; newLine.
		transcript show: (adder add: 100) printString; newLine
	)
) : ()
```

`goldens/capture/captures_in_initializers.toml`:

```toml
stdout = '''
15
105
'''
```

- [ ] **Step 8: Write the fallback golden**

`goldens/capture/fallback_closures_share_variables.ns`:

```
Newspeak3
'Test'
(* An inlined ifTrue: keeps a run-time guard: a receiver that is not a
   Boolean gets the real message with a real closure
   (goldens/language/control_flow_fallback.ns). That closure can be kept
   and run later, so it must go on reading and writing its frame's
   variables -- which is why closure conversion leaves a fallback closure
   on the by-reference model (docs/design/execution.md, "Fallback
   blocks").

   writeThenCapture: the fallback closure writes x after a second block
   captured it, so the reader answers 2. readLater: the fallback closure
   reads x after the method wrote it, so 5. fromAReturnedBlock: the
   fallback closure is created inside a block, keeps working after that
   block's activation has returned, and shares both the block's parameter
   and the method's variable, so the second call answers 15. *)
class FallbackClosuresShareVariables usingPlatform: platform = Object new (
	| transcript = platform transcript. |
) (
	class Keeper = Object new (
		| kept ::= nil. |
	) (
		public ifTrue: block = ( kept:: block. ^nil )
		public runKept = ( ^kept value )
	) : ()
	writeThenCapture: keeper = (
		| x ::= 1. reader ::= nil. |
		keeper ifTrue: [x:: 2].
		reader:: [x].
		keeper runKept.
		^reader value
	)
	readLater: keeper = (
		| x ::= 1. |
		keeper ifTrue: [x].
		x:: 5.
		^keeper runKept
	)
	fromAReturnedBlock: keeper = (
		| total ::= 10. |
		^[:increment | keeper ifTrue: [total:: total + increment]. total]
	)
	public main: platform args: arguments = (
		| maker keeper |
		transcript show: (self writeThenCapture: Keeper new) printString; newLine.
		transcript show: (self readLater: Keeper new) printString; newLine.
		keeper:: Keeper new.
		maker:: self fromAReturnedBlock: keeper.
		transcript show: (maker value: 5) printString; newLine.
		keeper runKept.
		transcript show: (maker value: 0) printString; newLine
	)
) : ()
```

`maker value: 5` stores the fallback closure and answers `total`, still 10;
`keeper runKept` then runs it, adding 5 to a variable the returned block's
activation no longer holds on the stack; `maker value: 0` stores a fresh one
and answers 15.

`goldens/capture/fallback_closures_share_variables.toml`:

```toml
stdout = '''
2
5
10
15
'''
```

- [ ] **Step 9: Run every gate**

Run, in this order:
`cargo test --workspace`,
`NS_NEXT_STRESS_MOVES=1 cargo test --workspace`,
`NS_NEXT_STRESS_CACHES=1 cargo test --workspace`,
`cargo run -p ns-cli -- parse-corpus corpus/modules`,
`cargo run -p ns-cli -- run-corpus corpus/modules`.
Expected: all pass with the counts in the Global Constraints. Every golden
also goes through the snapshot and the build/load round trip, so this is
where an image-format problem with these programs would appear.

- [ ] **Step 10: Add the category to the documentation**

In `docs/design/README.md`'s golden-category table, add a row in the table's
alphabetical position:

```
| `capture/` | what a block captures: copied variables, indirection vectors, the fallback closures of an inlined conditional |
```

- [ ] **Step 11: Commit**

```bash
git add goldens/capture docs/design/README.md
git commit -m "test: record what every capture shape answers today

Closure conversion decides per captured variable whether a block gets a copy
or shares its scope. These goldens pin the answers before the conversion
exists, under the by-reference model, so the conversion is held to them.

Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01F6LkMsqsEmuNAPegC4eL3K"
```

---

### Task 2: The capture analysis

**Files:**
- Create: `crates/ns-runtime/src/compiler/capture.rs`
- Modify: `crates/ns-runtime/src/compiler/mod.rs` (declare the module)

**Interfaces:**
- Consumes: `ns_syntax::ast`, `crate::types::Binding`, the resolver's
  `HashMap<Span, Binding>`, and `compiler::control_flow::recognize`.
- Produces, all `pub(crate)` in `compiler::capture`:
  - `enum BodyKey { Root, Block(Span) }`, `Copy + Eq + Hash + Debug`;
  - `enum Place { Register, Vector(u16) }`, `Copy + Eq + Debug`;
  - `enum Captured { Variable { body: BodyKey, index: u16 }, Vector { body: BodyKey } }`,
    `Copy + Eq + Debug`;
  - `struct BodyCaptures { places: Vec<Place>, vector_size: u16, branch_assigned: Vec<bool>, copied: Vec<Captured> }`,
    all fields `pub(crate)`;
  - `struct Captures` with `pub(crate) fn body(&self, key: BodyKey) -> &BodyCaptures`;
  - `struct RootBody<'a> { argument_count: u16, temporaries: &'a [ast::SlotDeclaration], slot_initializers: Vec<&'a Expression>, statements: &'a [Statement] }`,
    all fields `pub(crate)`;
  - `pub(crate) fn analyze(root: RootBody<'_>, bindings: &HashMap<Span, Binding>) -> Captures`.
  Task 5 consumes exactly these.

- [ ] **Step 1: Write the failing tests**

Create `crates/ns-runtime/src/compiler/capture.rs` with only the test module
below plus `use` lines, so the file compiles once Step 3 adds the types. The
helper mirrors `lower.rs`'s `resolve_method`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use indoc::formatdoc;
    use std::path::PathBuf;

    /// Analyses the first instance-side method of a one-method module.
    fn analyze_method(body: &str) -> Captures {
        let source = formatdoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              foo = ( {body} )
            ) : ()
        "};
        let module = ns_syntax::parse(&source).expect("parse");
        let resolved = crate::resolve(
            module,
            std::rc::Rc::from(source.as_str()),
            crate::SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let method = resolved.module.root_class.instance_side.methods[0].clone();
        analyze(
            RootBody {
                argument_count: 0,
                temporaries: &method.body.temporaries,
                slot_initializers: Vec::new(),
                statements: &method.body.statements,
            },
            &resolved.bindings,
        )
    }

    /// The root body's place for the local at `index`.
    fn place(captures: &Captures, index: usize) -> Place {
        captures.body(BodyKey::Root).places[index]
    }

    /// The one block of a body whose root has exactly one block literal.
    fn only_block(captures: &Captures) -> &BodyCaptures {
        let keys: Vec<BodyKey> = captures.keys().filter(|key| *key != BodyKey::Root).collect();
        assert_eq!(keys.len(), 1, "expected exactly one block literal");
        captures.body(keys[0])
    }

    #[test]
    fn a_variable_written_only_before_capture_is_copied() {
        let captures = analyze_method("| x ::= 0. | x:: 7. ^[x]");
        assert_eq!(place(&captures, 0), Place::Register);
        assert_eq!(
            only_block(&captures).copied,
            vec![Captured::Variable {
                body: BodyKey::Root,
                index: 0
            }]
        );
    }

    #[test]
    fn a_variable_written_after_capture_goes_into_the_vector() {
        let captures = analyze_method("| x ::= 0. reader ::= nil. | reader:: [x]. x:: 2. ^reader");
        assert_eq!(place(&captures, 0), Place::Vector(0));
        assert_eq!(captures.body(BodyKey::Root).vector_size, 1);
        assert_eq!(
            only_block(&captures).copied,
            vec![Captured::Vector {
                body: BodyKey::Root
            }]
        );
    }

    #[test]
    fn a_variable_a_block_writes_goes_into_the_vector() {
        let captures = analyze_method("| count ::= 0. | ^[count:: count + 1]");
        assert_eq!(place(&captures, 0), Place::Vector(0));
    }

    #[test]
    fn a_parameter_and_an_immutable_local_are_never_in_the_vector() {
        let captures = analyze_method("| five = 5. | ^[five]");
        assert_eq!(place(&captures, 0), Place::Register);
    }

    #[test]
    fn a_write_in_an_inlined_loop_that_also_captures_goes_into_the_vector() {
        let captures = analyze_method(
            "| index ::= 0. x ::= 0. blocks ::= nil. | \
             [index < 3] whileTrue: [x:: index. blocks add: [x]. index:: index + 1]. ^blocks",
        );
        assert_eq!(place(&captures, 1), Place::Vector(0));
    }

    #[test]
    fn a_write_in_an_inlined_loop_condition_that_also_captures_goes_into_the_vector() {
        let captures = analyze_method(
            "| index ::= 0. x ::= 0. blocks ::= nil. | \
             [x:: index. index < 3] whileTrue: [blocks add: [x]. index:: index + 1]. ^blocks",
        );
        assert_eq!(place(&captures, 1), Place::Vector(0));
    }

    #[test]
    fn a_write_in_an_inlined_branch_goes_into_the_vector_only_when_a_block_captures() {
        let captured = analyze_method(
            "| x ::= 1. reader ::= nil. | true ifTrue: [x:: 2]. reader:: [x]. ^reader",
        );
        assert_eq!(place(&captured, 0), Place::Vector(0));
        assert!(captured.body(BodyKey::Root).branch_assigned[0]);

        let uncaptured = analyze_method("| x ::= 1. | true ifTrue: [x:: 2]. ^x");
        assert_eq!(place(&uncaptured, 0), Place::Register);
        assert!(uncaptured.body(BodyKey::Root).branch_assigned[0]);
    }

    #[test]
    fn a_variable_read_only_in_an_inlined_block_is_not_captured() {
        let captures = analyze_method("| x ::= 4. | ^true ifTrue: [x + 1] ifFalse: [0]");
        assert_eq!(place(&captures, 0), Place::Register);
        assert_eq!(captures.keys().count(), 1);
    }

    #[test]
    fn a_middle_block_carries_what_an_inner_block_needs() {
        let captures = analyze_method("| x ::= 1. | ^[[x]]");
        let mut blocks: Vec<&BodyCaptures> = captures
            .keys()
            .filter(|key| *key != BodyKey::Root)
            .map(|key| captures.body(key))
            .collect();
        assert_eq!(blocks.len(), 2);
        blocks.sort_by_key(|block| block.copied.len());
        for block in blocks {
            assert_eq!(
                block.copied,
                vec![Captured::Variable {
                    body: BodyKey::Root,
                    index: 0
                }]
            );
        }
    }

    #[test]
    fn an_initializer_of_a_local_counts_as_a_write() {
        // The reader is created first, so the initializer of `x` -- which the
        // lowering runs as the body's first statements, in declaration order
        // -- writes after the capture.
        let captures = analyze_method("| reader ::= [x]. x ::= 1. | ^reader");
        assert_eq!(place(&captures, 1), Place::Vector(0));
    }
}
```

`Captures::keys` is a test-facing accessor:
`pub(crate) fn keys(&self) -> impl Iterator<Item = BodyKey> + '_`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime capture::`
Expected: the crate fails to compile — `analyze`, `Captures`, `BodyKey`,
`Place`, `Captured` and `RootBody` do not exist yet.

- [ ] **Step 3: Write the analysis**

Put this above the test module in `compiler/capture.rs`, and add
`pub(crate) mod capture;` to `compiler/mod.rs`.

```rust
//! The capture analysis of closure conversion (`docs/design/execution.md`,
//! "Planned: closure conversion"): for one code family -- a root body and
//! every block literal in it -- which of a body's locals live in registers
//! and which in its indirection vector, and what each real block is handed
//! when it is created.
//!
//! A pre-pass of the optimized lowering, not of the resolver: only the
//! compiler knows which blocks `control_flow::recognize` inlines, and a
//! variable read only inside an inlined block is not captured at all. The
//! reference lowering reaches every variable by reference and never calls
//! this.

use std::collections::HashMap;

use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::Span;

use super::control_flow::{self, InlineForm};
use crate::types::Binding;

/// A body of one code family: its root, or a block literal named by its span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BodyKey {
    Root,
    Block(Span),
}

/// Where one of a body's own locals lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Register,
    /// Slot `0..vector_size` of the body's indirection vector.
    Vector(u16),
}

/// One entry of a block's copied values: a variable's value, or the whole
/// indirection vector of the body that declares the variables it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Captured {
    Variable { body: BodyKey, index: u16 },
    Vector { body: BodyKey },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyCaptures {
    /// One entry per local, by resolver index: the parameters, then the
    /// temporaries, in declaration order.
    pub(crate) places: Vec<Place>,
    pub(crate) vector_size: u16,
    /// One entry per local: whether a block of an inlined conditional in this
    /// body assigns it, and so whether the fallback closure of that
    /// conditional can write its register at any later time ("Fallback
    /// blocks"). What the protective copies read.
    pub(crate) branch_assigned: Vec<bool>,
    /// What this block is handed when it is created, in order; its registers
    /// are `argument_count + position`. Empty for `BodyKey::Root`.
    pub(crate) copied: Vec<Captured>,
}

#[derive(Debug, Default)]
pub(crate) struct Captures {
    bodies: HashMap<BodyKey, BodyCaptures>,
}

impl Captures {
    pub(crate) fn body(&self, key: BodyKey) -> &BodyCaptures {
        self.bodies
            .get(&key)
            .unwrap_or_else(|| panic!("internal: no capture analysis for body {key:?}"))
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = BodyKey> + '_ {
        self.bodies.keys().copied()
    }
}

/// A root body, in the order the optimized lowering lowers it: the local
/// initializers, then a slot initializer per initialized slot (an instance
/// initializer only), then the statements.
pub(crate) struct RootBody<'a> {
    pub(crate) argument_count: u16,
    pub(crate) temporaries: &'a [ast::SlotDeclaration],
    pub(crate) slot_initializers: Vec<&'a Expression>,
    pub(crate) statements: &'a [Statement],
}

pub(crate) fn analyze(root: RootBody<'_>, bindings: &HashMap<Span, Binding>) -> Captures {
    let mut walker = Walker {
        bindings,
        scopes: Vec::new(),
        position: 0,
        next_loop: 0,
        creation: HashMap::new(),
        facts: HashMap::new(),
        locals: HashMap::new(),
        needs: HashMap::new(),
    };
    walker.declare(BodyKey::Root, root.argument_count, root.temporaries);
    walker.scopes.push(Scope::Frame {
        key: BodyKey::Root,
    });
    walker.local_initializers(BodyKey::Root, root.argument_count, root.temporaries);
    for initializer in &root.slot_initializers {
        walker.expression(initializer);
    }
    walker.statements(root.statements);
    walker.scopes.pop();
    walker.finish()
}

#[derive(Clone, Copy)]
enum Scope {
    Frame {
        key: BodyKey,
    },
    Inlined {
        /// A block of an inlined `ifTrue:`/`ifFalse:`/`and:`/`or:`, whose
        /// fallback path builds a real closure out of the same block.
        conditional: bool,
        /// The inlined loop this block belongs to, if any.
        loop_number: Option<u32>,
    },
}

/// A local, named by the body that declares it and its resolver index.
type LocalKey = (BodyKey, u16);

#[derive(Default)]
struct Facts {
    /// A block other than the declaring body assigns it: always shared.
    written_by_block: bool,
    /// The earliest position at which a real block that reads it is created.
    first_capture: Option<u32>,
    /// The inlined loops of the declaring body that were open at each such
    /// creation.
    capture_loops: Vec<u32>,
    writes: Vec<Write>,
}

struct Write {
    position: u32,
    loops: Vec<u32>,
    /// Inside a block of an inlined conditional, whose fallback closure can
    /// run at any later time.
    conditional: bool,
}

struct Walker<'a> {
    bindings: &'a HashMap<Span, Binding>,
    /// Mirrors the resolver's scope chain: every block literal is a scope,
    /// inlined or not, so a `Binding`'s depth indexes it directly.
    scopes: Vec<Scope>,
    position: u32,
    next_loop: u32,
    /// Per real block: where its `MakeClosure` stands, and which inlined
    /// loops of its creating frame were open there.
    creation: HashMap<BodyKey, (u32, Vec<u32>)>,
    facts: HashMap<LocalKey, Facts>,
    /// Per body: whether each local is mutable, by resolver index.
    locals: HashMap<BodyKey, Vec<bool>>,
    /// Per real block: the variables of enclosing bodies it needs, in first
    /// reference order, including everything its own nested blocks need.
    needs: HashMap<BodyKey, Vec<LocalKey>>,
}

impl<'a> Walker<'a> {
    fn declare(&mut self, key: BodyKey, argument_count: u16, temporaries: &[ast::SlotDeclaration]) {
        // A parameter is immutable (§3.6), and so is a local slot declared
        // with `=`; only those can ever need the vector.
        let mutable = std::iter::repeat_n(false, argument_count as usize)
            .chain(
                temporaries
                    .iter()
                    .map(|temporary| temporary.mutability != ast::SlotMutability::Immutable),
            )
            .collect();
        self.locals.insert(key, mutable);
        self.needs.insert(key, Vec::new());
    }

    /// The innermost frame's key, and its position in `scopes`.
    fn current_frame(&self) -> (BodyKey, usize) {
        for (position, scope) in self.scopes.iter().enumerate().rev() {
            if let Scope::Frame { key } = scope {
                return (*key, position);
            }
        }
        unreachable!("internal: a capture walk outside every frame")
    }

    /// The loops of the frame at `frame_position` that are open here.
    fn open_loops(&self, frame_position: usize) -> Vec<u32> {
        self.scopes[frame_position + 1..]
            .iter()
            .filter_map(|scope| match scope {
                Scope::Inlined { loop_number, .. } => *loop_number,
                Scope::Frame { .. } => None,
            })
            .collect()
    }

    /// Whether anything between the frame at `frame_position` and here is a
    /// block of an inlined conditional.
    fn inside_conditional(&self, frame_position: usize) -> bool {
        self.scopes[frame_position + 1..]
            .iter()
            .any(|scope| matches!(scope, Scope::Inlined { conditional: true, .. }))
    }

    /// The scope that declares the local a binding at `depth` names.
    fn declaring(&self, depth: u16) -> (BodyKey, usize) {
        let position = self.scopes.len() - 1 - depth as usize;
        match self.scopes[position] {
            Scope::Frame { key } => (key, position),
            Scope::Inlined { .. } => unreachable!(
                "internal: a local declared by an inlined block (the inlining gate admits none)"
            ),
        }
    }

    fn binding_of(&self, span: Span) -> Option<(u16, u16)> {
        match self.bindings.get(&span) {
            Some(Binding::Local(index)) => Some((0, *index)),
            Some(Binding::OuterLocal { depth, index }) => Some((*depth, *index)),
            _ => None,
        }
    }

    /// A read of a local: every real block between its declaring frame and
    /// here needs it, and the outermost of those blocks is where it is
    /// captured.
    fn reference(&mut self, identifier: &ast::Identifier) {
        let Some((depth, index)) = self.binding_of(identifier.span) else {
            return;
        };
        let (declaring_key, declaring_position) = self.declaring(depth);
        let blocks: Vec<BodyKey> = self.scopes[declaring_position + 1..]
            .iter()
            .filter_map(|scope| match scope {
                Scope::Frame { key } => Some(*key),
                Scope::Inlined { .. } => None,
            })
            .collect();
        let Some(outermost) = blocks.first().copied() else {
            return;
        };
        for block in &blocks {
            let needs = self.needs.get_mut(block).expect("internal: undeclared block");
            if !needs.contains(&(declaring_key, index)) {
                needs.push((declaring_key, index));
            }
        }
        let (position, loops) = self
            .creation
            .get(&outermost)
            .cloned()
            .expect("internal: a block referenced before it was entered");
        let facts = self.facts.entry((declaring_key, index)).or_default();
        facts.first_capture = Some(match facts.first_capture {
            Some(earlier) => earlier.min(position),
            None => position,
        });
        for number in loops {
            if !facts.capture_loops.contains(&number) {
                facts.capture_loops.push(number);
            }
        }
    }

    /// A write of a local, from wherever the walk currently stands.
    fn write(&mut self, declaring_key: BodyKey, index: u16, declaring_position: usize) {
        let (current_key, current_position) = self.current_frame();
        self.position += 1;
        if current_key != declaring_key {
            self.facts
                .entry((declaring_key, index))
                .or_default()
                .written_by_block = true;
            return;
        }
        let write = Write {
            position: self.position,
            loops: self.open_loops(current_position),
            conditional: self.inside_conditional(declaring_position),
        };
        self.facts
            .entry((declaring_key, index))
            .or_default()
            .writes
            .push(write);
    }

    fn assign(&mut self, target: &ast::Identifier) {
        let Some((depth, index)) = self.binding_of(target.span) else {
            return;
        };
        let (declaring_key, declaring_position) = self.declaring(depth);
        self.write(declaring_key, index, declaring_position);
    }

    fn local_initializers(
        &mut self,
        key: BodyKey,
        argument_count: u16,
        temporaries: &[ast::SlotDeclaration],
    ) {
        let frame_position = self.scopes.len() - 1;
        for (offset, temporary) in temporaries.iter().enumerate() {
            let Some(initializer) = &temporary.initializer else {
                continue;
            };
            self.expression(initializer);
            self.write(key, argument_count + offset as u16, frame_position);
        }
    }

    fn statements(&mut self, statements: &[Statement]) {
        for statement in statements {
            match statement {
                Statement::Expression(expression) | Statement::Return(expression) => {
                    self.expression(expression)
                }
            }
        }
    }

    fn expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Identifier(identifier) => self.reference(identifier),
            Expression::Literal(_) => {}
            Expression::Assign { target, value, .. } => {
                self.expression(value);
                self.assign(target);
            }
            Expression::Send(send) => self.send(send),
            Expression::Cascade(cascade) => {
                self.expression(&cascade.receiver);
                for message in std::iter::once(&cascade.first).chain(cascade.rest.iter()) {
                    for argument in &message.arguments {
                        self.expression(argument);
                    }
                }
            }
            Expression::Block(block) => self.real_block(block),
            Expression::DynamicArray { elements, .. } => {
                for element in elements {
                    self.expression(element);
                }
            }
        }
    }

    /// Mirrors `lower_send`: only an ordinary explicit-receiver send is
    /// offered to `recognize`, and a lexical or `super` send has no receiver
    /// expression to walk.
    fn send(&mut self, send: &ast::Send) {
        let ordinary = !matches!(
            self.bindings.get(&send.span),
            Some(Binding::SuperSend) | Some(Binding::LexicalSend { .. })
        );
        if ordinary {
            if let Some(form) = control_flow::recognize(send) {
                self.inlined(send, form);
                return;
            }
            self.expression(&send.receiver);
        }
        for argument in &send.message.arguments {
            self.expression(argument);
        }
    }

    fn inlined(&mut self, send: &ast::Send, form: InlineForm<'_>) {
        match form {
            InlineForm::IfTrueIfFalse(first, second) => {
                self.expression(&send.receiver);
                self.inlined_block(first, true, None);
                self.inlined_block(second, true, None);
            }
            InlineForm::IfTrue(block)
            | InlineForm::IfFalse(block)
            | InlineForm::And(block)
            | InlineForm::Or(block) => {
                self.expression(&send.receiver);
                self.inlined_block(block, true, None);
            }
            InlineForm::WhileTrue { condition, body } | InlineForm::WhileFalse { condition, body } => {
                let number = self.next_loop;
                self.next_loop += 1;
                self.inlined_block(condition, false, Some(number));
                self.inlined_block(body, false, Some(number));
            }
        }
    }

    fn inlined_block(
        &mut self,
        block: &ast::BlockExpression,
        conditional: bool,
        loop_number: Option<u32>,
    ) {
        self.scopes.push(Scope::Inlined {
            conditional,
            loop_number,
        });
        self.statements(&block.body.statements);
        self.scopes.pop();
    }

    fn real_block(&mut self, block: &ast::BlockExpression) {
        let key = BodyKey::Block(block.span);
        let (_, creating_frame) = self.current_frame();
        self.position += 1;
        let loops = self.open_loops(creating_frame);
        self.creation.insert(key, (self.position, loops));
        let argument_count = block.parameters.len() as u16;
        self.declare(key, argument_count, &block.body.temporaries);
        self.scopes.push(Scope::Frame { key });
        self.local_initializers(key, argument_count, &block.body.temporaries);
        self.statements(&block.body.statements);
        self.scopes.pop();
    }

    fn finish(self) -> Captures {
        let mut bodies: HashMap<BodyKey, BodyCaptures> = HashMap::new();
        for (key, mutable) in &self.locals {
            let mut places = vec![Place::Register; mutable.len()];
            let mut branch_assigned = vec![false; mutable.len()];
            let mut vector_size = 0;
            for index in 0..mutable.len() {
                let Some(facts) = self.facts.get(&(*key, index as u16)) else {
                    continue;
                };
                branch_assigned[index] = facts.writes.iter().any(|write| write.conditional);
                if mutable[index] && needs_vector(facts) {
                    places[index] = Place::Vector(vector_size);
                    vector_size += 1;
                }
            }
            bodies.insert(
                *key,
                BodyCaptures {
                    places,
                    vector_size,
                    branch_assigned,
                    copied: Vec::new(),
                },
            );
        }
        for (block, needs) in &self.needs {
            let mut copied: Vec<Captured> = Vec::new();
            for (body, index) in needs {
                let entry = match bodies[body].places[*index as usize] {
                    Place::Register => Captured::Variable {
                        body: *body,
                        index: *index,
                    },
                    Place::Vector(_) => Captured::Vector { body: *body },
                };
                if !copied.contains(&entry) {
                    copied.push(entry);
                }
            }
            bodies
                .get_mut(block)
                .expect("internal: undeclared block")
                .copied = copied;
        }
        Captures { bodies }
    }
}

/// The rule (`docs/design/execution.md`, "Where the analysis lives"): a
/// captured mutable local is copied unless something can write it once a
/// block holds it.
fn needs_vector(facts: &Facts) -> bool {
    if facts.written_by_block {
        return true;
    }
    let Some(first_capture) = facts.first_capture else {
        return false;
    };
    facts.writes.iter().any(|write| {
        write.position > first_capture
            || write.conditional
            || write
                .loops
                .iter()
                .any(|number| facts.capture_loops.contains(number))
    })
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ns-runtime capture::`
Expected: PASS, all ten.

- [ ] **Step 5: Run the workspace and the gates**

Run: `cargo fmt --all`, `cargo clippy --workspace --all-targets`,
`cargo test --workspace`.
Expected: no warnings, everything passes. Nothing consumes the analysis yet,
so no behaviour has changed.

- [ ] **Step 6: Commit**

```bash
git add crates/ns-runtime/src/compiler/capture.rs crates/ns-runtime/src/compiler/mod.rs
git commit -m "feat: analyse which captured variables are copied and which are shared

A pre-pass of the optimized lowering, not of the resolver: only the compiler
knows which blocks it inlines, and a variable read only inside an inlined
block is not captured. Nothing consumes it yet.

Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01F6LkMsqsEmuNAPegC4eL3K"
```

---

### Task 3: A closure carries copied values, and a frame keeps its registers only when something reads them

**Files:**
- Modify: `crates/ns-runtime/src/types.rs` (`ClosureData`)
- Modify: `crates/ns-runtime/src/bytecode.rs` (`MakeClosure.copied`,
  `CompiledCode.copied_count`, `CompiledCode.outer_reach`, `outer_reach_of`,
  disassembly)
- Modify: `crates/ns-runtime/src/vm.rs` (`MakeClosure`)
- Modify: `crates/ns-runtime/src/heap.rs` (closure accessors, `reify`'s home,
  the retention bit, `pop_activation`, `frame_local`/`frame_set_local`,
  `fill_copied_values`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`Step::Block`)
- Modify: `crates/ns-runtime/src/interpreter/locals.rs` (`home_of` goes)
- Modify: `crates/ns-runtime/src/trace.rs`
- Modify: `crates/ns-runtime/src/compiler/lower.rs`, `compiler/reference.rs`
  (emit `copied: Vec::new()`, fill the two new `CompiledCode` fields)
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`,
  `inspect.rs`
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`, `tests/image.rs`,
  `tests/heap.rs`, and every test that builds a `CompiledCode` or a
  `ClosureData` literal

**Interfaces:**
- Consumes: nothing from Task 2.
- Produces:
  - `ClosureData { code: Rc<CompiledCode>, captured_frame: FrameReference<'gc>, copied: Box<[Value<'gc>]>, feedback_base: FeedbackBase }`
    — `parameters` and `home` are gone;
  - `Instruction::MakeClosure { destination: u16, parameters: Rc<Vec<Symbol>>, code: Rc<CompiledCode>, copied: Vec<u16> }`;
  - `CompiledCode.copied_count: u16` and `CompiledCode.outer_reach: u16`, plus
    `pub(crate) fn bytecode::outer_reach_of(instructions: &[Instruction]) -> u16`;
  - `Heap::closure_home(ObjectReference) -> Option<FrameReference>`,
    `Heap::retain_registers(ActivationReference)`,
    `Heap::fill_copied_values(ActivationReference, ObjectReference)`.
  Task 4 adds the vector instructions beside these; Task 5 fills `copied`.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/heap.rs`, using that file's existing helpers for
building an interpreter and pushing activations (copy the surrounding tests'
setup exactly):

```rust
#[test]
fn a_returning_activation_keeps_its_registers_only_when_it_was_marked() {
    // A frame reified for a closure that reads nothing by reference needs no
    // registers once its activation has returned: what the closure captured
    // it carries itself. One that was marked -- a reference-lowering block,
    // or an optimized fallback closure -- still keeps them.
}
```

Write it out as two activations: push one, `reify` it, pop it, and assert
`frame_register_shape` answers `(program_counter, 0, 0)`; push a second,
`reify` it, call `retain_registers`, pop it, and assert its arguments and
temporaries are the activation's registers. Assert too that `frame_local` on
the first answers `Value::Nil` for an index it no longer has, rather than
panicking.

In `crates/ns-runtime/tests/image.rs`:

```rust
#[test]
fn a_closure_records_its_copied_values_and_a_wrong_count_is_refused() {
    // Round-trips a closure with two copied values, then rebuilds the same
    // record with a count that does not match its code's `copied_count` and
    // expects a decode error, not a panic.
}
```

and

```rust
#[test]
fn a_make_closure_whose_copied_list_does_not_match_its_block_is_refused() {
}
```

Both follow `closure_object_with_parameter_count`'s existing pattern for
hand-writing a graph section.

In `crates/ns-runtime/tests/opcode_tags.rs`, add to the existing assertions:

```rust
    assert_eq!(OPCODE_MAKE_CLOSURE, 29);
```

and extend the retired list assertion to
`[7, 8, 9, 10, 11, 21, 22, 23, 24]`, and add

```rust
#[test]
fn retired_object_body_tags_are_refused() {
    assert_eq!(RETIRED_OBJECT_BODIES, [4]);
    assert_eq!(OBJECT_CLOSURE, 6);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime --test heap --test image --test opcode_tags`
Expected: compile errors for the new names, and the tag assertions failing
against 11 and 4.

- [ ] **Step 3: Reshape `ClosureData`**

In `types.rs`:

```rust
pub struct ClosureData<'gc> {
    pub code: Rc<crate::bytecode::CompiledCode>,
    /// The activation the closure was created in, reified. A closure keeps
    /// it whatever it captured: `enclosingActivation` and the debugger's
    /// "[] in Foo bar" are read off it (`docs/design/execution.md`, "The
    /// closure keeps its defining activation").
    pub captured_frame: FrameReference<'gc>,
    /// What the closure was handed when it was created: a copied variable's
    /// value, or an enclosing body's indirection vector. Its registers are
    /// `code.argument_count + position`. `Box<[_]>`, not a `Vec`, because
    /// `ClosureData` is `ObjectData`'s largest payload and an `Object` is
    /// pinned at 56 bytes in a release build (`heap.rs`).
    pub copied: Box<[Value<'gc>]>,
    pub feedback_base: crate::interpreter::FeedbackBase,
}
```

`parameters` goes: its only use was its length, which `code.argument_count`
already is. `home` goes: the captured frame determines it — itself when it is
a method's, an initializer's or a clause's, its own `home` when it is a
block's. Add to `heap.rs`:

```rust
    /// A closure's home: the activation a `^` in its body returns from.
    /// Derived rather than stored, so that `ClosureData` stays 32 bytes.
    pub(crate) fn closure_home(
        &self,
        object: ObjectReference<'gc>,
    ) -> Option<FrameReference<'gc>> {
        let captured = self.closure(object)?.captured_frame;
        Some(match self.frames[captured.index as usize].frame.kind {
            FrameKind::Closure(_) => self.frames[captured.index as usize]
                .frame
                .home
                .expect("internal: a closure frame carries its home"),
            _ => captured,
        })
    }
```

Replace every reader of `closure.home` with it (`reify`, `allocate_frame`'s
debug assertion path, `image::encode`), and replace
`closure.parameters.len()` with `closure.code.argument_count` in
`closure_arity` and `closure_activation`. `interpreter/locals.rs`'s `home_of`
loses its only caller and goes.

- [ ] **Step 4: Add the two `CompiledCode` fields and `MakeClosure.copied`**

In `bytecode.rs`:

```rust
    /// How many copied values a block's activation is handed, between its
    /// parameters and its temporaries. Zero for every other kind of body.
    pub copied_count: u16,
    /// How many frames out the deepest by-reference read or write this body
    /// or a block it creates can reach: `0` for converted code, `1` for a
    /// fallback closure that reads its own creator's registers, one more per
    /// level above that. `MakeClosure` marks the creating activation when
    /// this is non-zero, which is what makes a returning frame keep its
    /// registers (`docs/design/execution.md`, "The divorce"). Derived from
    /// the instructions, never on the wire.
    pub outer_reach: u16,
```

and

```rust
/// A body's `outer_reach`. A nested block's reach counts one level less from
/// here: the frame it reaches through is the one this body creates.
pub(crate) fn outer_reach_of(instructions: &[Instruction]) -> u16 {
    instructions
        .iter()
        .map(|instruction| match instruction {
            Instruction::LoadOuterLocal { depth, .. }
            | Instruction::StoreOuterLocal { depth, .. } => *depth,
            Instruction::MakeClosure { code, .. } => code.outer_reach.saturating_sub(1),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}
```

`MakeClosure` gains `copied: Vec<u16>`. Its disassembly keeps today's shape
when the list is empty and appends ` copying r0 r3` when it is not, so the
existing disassembly tests do not move.

Both lowerings fill the new fields in `finish`: `copied_count` is 0 for now
(Task 5 sets it), `outer_reach` is `outer_reach_of(&instructions)`. Every
`CompiledCode` literal in the tree — tests included — gets the two fields.

- [ ] **Step 5: Mark, copy and divorce**

`StackEntry` gains `retains_registers: bool`, `false` at push. `Heap`:

```rust
    /// Marks `activation` as one whose registers must survive its return: it
    /// created a closure that reads enclosing frames by reference.
    pub(crate) fn retain_registers(&mut self, activation: ActivationReference) {
        self.check_activation(activation);
        self.stack.entries[activation.index as usize].retains_registers = true;
    }
```

`pop_activation` copies the registers into the frame only when
`entry.retains_registers`; otherwise it sets the frame's `program_counter`,
`sender`, `uncontinuable` and `live` exactly as today and leaves `arguments`
and `temporaries` empty. `frame_local` answers `Value::Nil` and
`frame_set_local` does nothing for an index past a dead frame's register
file: by construction no converted code reaches one, and a crafted image must
not panic.

`Heap::fill_copied_values(activation, closure)` writes a closure's copied
values into the registers after the activation's arguments;
`interpreter::push_step`'s `Step::Block` arm calls it right after `push`.

`vm.rs`'s `MakeClosure` arm becomes:

```rust
        Instruction::MakeClosure {
            destination,
            code: block,
            copied,
            ..
        } => {
            // Stored before reifying, so the frame `reify` builds starts
            // from this program counter.
            mutator
                .heap
                .set_activation_program_counter(activation, program_counter);
            let captured_frame = mutator.heap.reify(activation);
            // A closure that reads enclosing registers by reference -- the
            // reference lowering's, and an optimized fallback closure -- is
            // what makes this activation's registers outlive it.
            if block.outer_reach > 0 {
                mutator.heap.retain_registers(activation);
            }
            let copied_values = copied
                .iter()
                .map(|register| read_local(&mutator.heap, activation, *register))
                .collect();
            let value = Value::Object(mutator.new_closure(ClosureData {
                code: block.clone(),
                captured_frame,
                copied: copied_values,
                // A block belongs to its enclosing method's family: the
                // running activation's own base, already in hand here, so
                // this costs no extra load.
                feedback_base,
            }));
            write_local(&mut mutator.heap, activation, *destination, value);
            Flow::Next
        }
```

`trace.rs`'s `ClosureData` impl names `copied` and traces every value in it.

- [ ] **Step 6: Change the two records in the image format**

`opcodes.rs`: retire 11, `OPCODE_MAKE_CLOSURE = 29`;
`RETIRED_OPCODES = [7, 8, 9, 10, 11, 21, 22, 23, 24]`; `OBJECT_CLOSURE = 6`
with a new `RETIRED_OBJECT_BODIES: [u8; 1] = [4]` and a comment saying why
each was retired (the payload changed, and a stale artifact must hit an
unknown tag rather than decode into a different, valid program).

`encode.rs`: a closure record is `code_id`, `captured_frame` id, then a
`u16` count and that many encoded values; `home` and the parameter names go.
The `MakeClosure` instruction writes `destination`, the parameter names as
today, the code id, then the copied registers through the same
count-and-`u16`s shape `decode_registers` reads. `GraphTable`'s closure walk
pushes the captured frame and visits every copied value.

`encode_code` writes `copied_count` directly after `register_count`.

`decode.rs`: the mirror, plus

- `DecodeError::CopiedCountMismatch { copied: usize, copied_count: u16 }` for a
  `MakeClosure` whose list does not match its block, checked where the
  existing `MakeClosure` walk already checks the body it names;
- the same error for a closure record whose values do not match its code;
- `DecodeError::CopiedCountOutOfRange { argument_count: u16, copied_count: u16, register_count: u16 }`
  when `argument_count + copied_count > register_count`, checked in
  `decode_code` (in `u32` arithmetic, so it cannot wrap);
- `own_registers` lists `MakeClosure`'s copied registers beside its
  destination, so they are bounded like every other register operand;
- `verify_graph`'s register-file check accepts a frame with no registers at
  all — what a returning activation that was never marked leaves behind —
  and keeps refusing a narrower non-empty one;
- `CompiledCode`'s two new fields are filled: `copied_count` from the wire,
  `outer_reach` from `outer_reach_of`, which works because a block's code is
  always decoded before its parent's;
- the old `ClosureArityMismatch` error goes with the parameter names.

`inspect.rs`'s structural walk reads the new shapes through the same tags.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p ns-runtime --test heap --test image --test opcode_tags`,
then `cargo test --workspace`.
Expected: PASS. Behaviour has not changed: both lowerings still reach every
variable by reference, so every block still has `outer_reach >= 1` when it
reads anything outer, and the goldens answer exactly what Task 1 recorded.

- [ ] **Step 8: Run every gate and commit**

Run the six gate commands from the Global Constraints.

```bash
git add -A
git commit -m "feat: give a closure its copied values and decide the divorce per activation

A closure carries what it was handed and derives its home from its captured
frame, so ClosureData stays 32 bytes and an Object stays 56. A returning
activation copies its registers into its frame only when it created a closure
that reads enclosing frames by reference. The closure record and MakeClosure
retire their tags, since their payloads changed.

Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01F6LkMsqsEmuNAPegC4eL3K"
```

---

### Task 4: The indirection vector's three instructions

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (three variants, disassembly)
- Modify: `crates/ns-runtime/src/vm.rs` (three arms)
- Modify: `crates/ns-runtime/src/interpreter/locals.rs` (`load_vector`,
  `store_vector`, their error)
- Modify: `crates/ns-runtime/src/image/opcodes.rs`, `encode.rs`, `decode.rs`,
  `inspect.rs`
- Modify: `crates/ns-runtime/src/compiler/reference.rs` (its exhaustive
  instruction lists)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`, `tests/image.rs`,
  `tests/vm.rs`

**Interfaces:**
- Consumes: Task 3's `CompiledCode` shape.
- Produces: `Instruction::MakeVector { destination: u16, size: u16 }`,
  `Instruction::LoadVector { destination: u16, vector: u16, index: u16 }`,
  `Instruction::StoreVector { vector: u16, index: u16, source: u16 }`, tags
  30, 31, 32. Task 5 emits them.

- [ ] **Step 1: Write the failing tests**

In `crates/ns-runtime/tests/vm.rs`, building a method body by hand and
installing it the way `tests/image.rs`'s `with_method_code` and
`install_mixin` do:

```rust
#[test]
fn a_vector_round_trips_a_value_through_its_slot() {
    // MakeVector r1, size 1; LoadLiteral r2 <- 42; StoreVector r1[0] <- r2;
    // LoadVector r3 <- r1[0]; Return r3  =>  42
}

#[test]
fn reading_a_vector_slot_that_is_not_there_is_an_error_not_a_panic() {
    // LoadLiteral r1 <- 7 (not an Array); LoadVector r2 <- r1[0]; Return r2
    // => an error whose message names the indirection vector, and a process
    // that is still running.
}
```

In `crates/ns-runtime/tests/opcode_tags.rs`:

```rust
    assert_eq!(OPCODE_MAKE_VECTOR, 30);
    assert_eq!(OPCODE_LOAD_VECTOR, 31);
    assert_eq!(OPCODE_STORE_VECTOR, 32);
```

and add the three to `every_instruction_once` in `tests/image.rs`, which the
round-trip test already walks.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime --test vm --test opcode_tags --test image`
Expected: compile errors — the three variants do not exist.

- [ ] **Step 3: Add the instructions**

`bytecode.rs`:

```rust
    /// Builds this body's indirection vector: the `Array` its own
    /// captured-and-written locals live in, and which every block that needs
    /// one of them is handed a reference to
    /// (`docs/design/execution.md`, "Representation"). Emitted once, before
    /// the body's first statement, because a local slot's own initializer
    /// already writes through it.
    MakeVector {
        destination: u16,
        size: u16,
    },
    LoadVector {
        destination: u16,
        vector: u16,
        index: u16,
    },
    StoreVector {
        vector: u16,
        index: u16,
        source: u16,
    },
```

Disassembly: `MakeVector r{destination}, size {size}`,
`LoadVector r{destination} <- r{vector}[{index}]`,
`StoreVector r{vector}[{index}] <- r{source}`.

`interpreter/locals.rs`:

```rust
/// The VM's `LoadVector`. A value that is not the `Array` the compiler put
/// there, or an index past its end, can only come from a crafted image
/// (`CLAUDE.md`, "An image is untrusted input"), so this reports rather than
/// indexes.
pub(crate) fn load_vector<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    vector: u16,
    index: u16,
) -> EvaluationResult<'gc> {
    let value = heap.activation_local(activation, vector);
    match value {
        Value::Object(object) => match heap.array_at(object, index as usize) {
            Some(element) => Ok(element),
            None => Err(vector_error(value)),
        },
        _ => Err(vector_error(value)),
    }
}
```

`store_vector` is its mirror through `Heap::array_set`, answering
`Ok(())` or the same error, and

```rust
/// The error both raise. Built here, never in `vm.rs`: error text lives in
/// `interpreter/`.
fn vector_error<'gc>(vector: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern("indirectionVector"),
        receiver: vector,
        message: "internal: an indirection vector is not the Array its code expects".to_string(),
    })
}
```

`vm.rs` gains three arms: `MakeVector` allocates through
`mutator.new_array(vec![Value::Nil; *size as usize])` and writes it to
`destination`; `LoadVector` and `StoreVector` call the two helpers and
`unwind` on `Err`, exactly as the `LoadLiteral` arm does.

- [ ] **Step 4: Put them on the wire**

`opcodes.rs`: `OPCODE_MAKE_VECTOR = 30`, `OPCODE_LOAD_VECTOR = 31`,
`OPCODE_STORE_VECTOR = 32`. `encode.rs` and `decode.rs` write and read the
three `u16`s each (two for `MakeVector`). `decode.rs`'s `own_registers` lists
`MakeVector`'s destination, `LoadVector`'s destination and vector, and
`StoreVector`'s vector and source — the index is a vector slot, not a
register, and is checked at run time, since a vector's size is a run-time
value. `jump_target`'s exhaustive list gains the three. `inspect.rs` prints
them. `compiler/reference.rs`'s `operands`, `destination` and `check_shape`
lists gain them too, as unreachable-in-reference-code arms that still have to
be named for the match to compile.

- [ ] **Step 5: Run the tests and the gates**

Run: `cargo test --workspace`, then the five other gate commands.
Expected: PASS. Nothing emits the new instructions yet.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add the indirection vector's three instructions

MakeVector builds a body's vector, LoadVector and StoreVector read and write
a slot. A value that is not the Array the compiler put there is a run-time
error built in interpreter/, never a panic and never text in vm.rs. Nothing
emits them yet.

Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01F6LkMsqsEmuNAPegC4eL3K"
```

---

### Task 5: The optimized lowering converts

**Files:**
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (`ScopeKind`, `Layout`,
  `Lowerer::new`, variable access, `lower_assign`, `lower_local_initializers`,
  `lower_block_literal`, `lower_if`, `lower_and_or`, `finish`)
- Modify: `crates/ns-runtime/src/compiler/mod.rs` (run the analysis per root
  body)
- Modify: `crates/ns-runtime/tests/compiler.rs` and the `lower.rs` test module

**Interfaces:**
- Consumes: Task 2's `capture::{analyze, BodyKey, Captured, Captures, Place, RootBody}`,
  Task 3's `MakeClosure.copied` and `CompiledCode.copied_count`, Task 4's
  three instructions.
- Produces: `Lowerer::new(kind, key, argument_count, temporaries, bindings,
  captures, by_reference, enclosing, first_call_site)` where `enclosing` is
  `compiler::lower::Enclosing { scopes, read_only_locals, layouts }`, and
  `pub(crate) struct Layout` with `converted`, `by_reference`, `locate` and
  `register_holding`. Task 6 consumes `Layout` and `BodyCaptures::branch_assigned`.

- [ ] **Step 1: Write the failing tests**

In `lower.rs`'s test module, replacing
`a_block_reads_and_writes_enclosing_locals_through_outer_local_instructions`
and `a_doubly_nested_block_counts_two_frame_hops` (both of which pin the model
this task replaces; keep their sources, change their expectations) and adding
the rest:

```rust
    #[test]
    fn a_block_reads_a_copied_variable_as_its_own_register() {
        let code = lower_whole_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo: x = ( ^[x] )
                ) : ()
            "},
            0,
        );
        let Some(Instruction::MakeClosure { code: block, copied, .. }) = code.instructions.first()
        else {
            panic!()
        };
        assert_eq!(*copied, vec![0]); // the parameter's own register
        assert_eq!(block.copied_count, 1);
        assert_eq!(block.instructions, vec![Instruction::ImplicitReturn { register: 0 }]);
        assert_eq!(block.outer_reach, 0);
    }

    #[test]
    fn a_block_that_writes_shares_its_scopes_vector() {
        let code = lower_whole_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo = ( | t | ^[t:: 1] )
                ) : ()
            "},
            0,
        );
        assert_eq!(
            code.instructions[0],
            Instruction::MakeVector {
                destination: 1,
                size: 1
            }
        );
        let Some(Instruction::MakeClosure { code: block, copied, .. }) = code.instructions.get(1)
        else {
            panic!()
        };
        assert_eq!(*copied, vec![1]); // the vector register
        assert_eq!(
            block.instructions,
            vec![
                Instruction::LoadLiteral {
                    destination: 1,
                    literal: BytecodeLiteral::SmallInteger(1)
                },
                Instruction::StoreVector {
                    vector: 0,
                    index: 0,
                    source: 1
                },
                Instruction::ImplicitReturn { register: 1 },
            ]
        );
    }

    #[test]
    fn a_doubly_nested_block_is_handed_the_variable_by_the_block_between() {
        let code = lower_whole_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo: x = ( ^[[x]] )
                ) : ()
            "},
            0,
        );
        let Some(Instruction::MakeClosure { code: outer, copied, .. }) = code.instructions.first()
        else {
            panic!()
        };
        assert_eq!(*copied, vec![0]);
        let Some(Instruction::MakeClosure { code: inner, copied: inner_copied, .. }) =
            outer.instructions.first()
        else {
            panic!()
        };
        // The middle block holds it in its own first copied register.
        assert_eq!(*inner_copied, vec![0]);
        assert_eq!(inner.instructions, vec![Instruction::ImplicitReturn { register: 0 }]);
    }

    #[test]
    fn the_fallback_closure_of_an_inlined_conditional_stays_by_reference() {
        let code = lower_whole_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  foo: c = ( | t | c ifTrue: [t:: 1]. ^t )
                ) : ()
            "},
            0,
        );
        let fallback = code
            .instructions
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::MakeClosure { code: block, copied, .. } => {
                    assert!(copied.is_empty());
                    Some(block.clone())
                }
                _ => None,
            })
            .expect("the fallback path builds a real closure");
        assert!(fallback.instructions.iter().any(|instruction| matches!(
            instruction,
            Instruction::StoreOuterLocal { depth: 1, index: 1, .. }
        )));
        assert_eq!(fallback.outer_reach, 1);
        // Nothing captured `t`, so it stays a register.
        assert!(!code
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::MakeVector { .. })));
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime --lib compiler::lower`
Expected: FAIL — the lowering still emits `LoadOuterLocal`/`StoreOuterLocal`
for every captured variable and an empty `copied` list.

- [ ] **Step 3: Give the lowering a layout**

In `lower.rs`:

```rust
/// One entry of the compile-time scope stack. The resolver counts every block
/// literal as one scope; at run time only a `Frame` scope costs a hop.
/// `by_reference` marks a fallback closure's frame and everything nested in
/// it, the one place the optimized lowering still reaches a frame's registers
/// by reference (`docs/design/execution.md`, "Fallback blocks").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Frame { key: BodyKey, by_reference: bool },
    Inlined,
}

/// Where a converted body keeps what it can reach: its own locals, the values
/// it was handed when it was created, and its indirection vector.
pub(crate) struct Layout {
    key: BodyKey,
    argument_count: u16,
    copied: Vec<Captured>,
    places: Vec<Place>,
    vector_register: Option<u16>,
}

/// How a body reaches one variable.
enum Access {
    Register(u16),
    OuterRegister { hops: u16, index: u16 },
    Vector { vector: VectorAccess, slot: u16 },
}

enum VectorAccess {
    Register(u16),
    Outer { hops: u16, index: u16 },
}
```

`Layout::converted(key, argument_count, temporary_count, captures)` builds
`places` and `vector_register` (the register after the temporaries, present
only when `vector_size > 0`); `Layout::by_reference(key, argument_count)`
builds one with no copied values, all-`Register` places and no vector — a
fallback closure takes no parameters and no temporaries of its own, but a
real block nested inside one does, and it reaches its own locals plainly.

```rust
impl Layout {
    /// The register a local of *this* body sits in: the copied values sit
    /// between the parameters and the temporaries, so every temporary is
    /// pushed up by their count.
    fn local_register(&self, index: u16) -> u16 {
        if index < self.argument_count {
            index
        } else {
            index + self.copied.len() as u16
        }
    }

    /// Where a variable declared by `body` at `index` is found from a frame
    /// with this layout.
    fn locate(&self, body: BodyKey, index: u16, captures: &Captures) -> Access {
        if body == self.key {
            return match self.places[index as usize] {
                Place::Register => Access::Register(self.local_register(index)),
                Place::Vector(slot) => Access::Vector {
                    vector: VectorAccess::Register(
                        self.vector_register
                            .expect("internal: a vector variable in a body with no vector"),
                    ),
                    slot,
                },
            };
        }
        let wanted = match captures.body(body).places[index as usize] {
            Place::Register => Captured::Variable { body, index },
            Place::Vector(_) => Captured::Vector { body },
        };
        let position = self
            .copied
            .iter()
            .position(|entry| *entry == wanted)
            .expect("internal: a variable the capture analysis did not hand this block");
        let register = self.argument_count + position as u16;
        match captures.body(body).places[index as usize] {
            Place::Register => Access::Register(register),
            Place::Vector(slot) => Access::Vector {
                vector: VectorAccess::Register(register),
                slot,
            },
        }
    }
}
```

The `Lowerer` gains `key: BodyKey`, `captures: Rc<Captures>`,
`layout: Rc<Layout>`, `by_reference: bool`, `blocks_by_reference: bool` and
`layouts: Vec<Rc<Layout>>` (one per enclosing `Frame` scope, innermost last,
beside the existing `enclosing_read_only_locals`). `base` becomes
`argument_count + copied_count + temporaries + vector`, and
`read_only_locals` marks the parameters, the copied registers, the vector
register, every immutable local and every vector variable's now-unused
register.

`resolve_variable(depth, index) -> Access` is the one place a `Binding`
becomes an access:

```rust
    fn resolve_variable(&self, depth: u16, index: u16) -> Access {
        let declaring_position = self.scopes.len() - 1 - depth as usize;
        let ScopeKind::Frame { key, by_reference } = self.scopes[declaring_position] else {
            unreachable!("internal: a local declared by an inlined block")
        };
        if by_reference {
            let hops = hops_above(&self.scopes, declaring_position);
            return if hops == 0 {
                Access::Register(index)
            } else {
                Access::OuterRegister { hops, index }
            };
        }
        // A converted frame holds it. From inside a fallback closure that is
        // the innermost converted frame below this one -- the frame whose
        // common path the fallback stands in for.
        let anchor_position = innermost_converted(&self.scopes);
        let hops = hops_above(&self.scopes, anchor_position);
        let layout = if hops == 0 {
            &self.layout
        } else {
            &self.layouts[self.layouts.len() - hops as usize]
        };
        match layout.locate(key, index, &self.captures) {
            Access::Register(register) if hops == 0 => Access::Register(register),
            Access::Register(register) => Access::OuterRegister { hops, index: register },
            Access::Vector { vector: VectorAccess::Register(register), slot } if hops == 0 => {
                Access::Vector { vector: VectorAccess::Register(register), slot }
            }
            Access::Vector { vector: VectorAccess::Register(register), slot } => Access::Vector {
                vector: VectorAccess::Outer { hops, index: register },
                slot,
            },
            other => other,
        }
    }
```

`hops_above(scopes, position)` counts `Frame` scopes after `position`;
`innermost_converted(scopes)` finds the last `Frame { by_reference: false }`.

`lower_identifier_read` turns an `Access` into registers:
`Register(register)` is the answer itself; `OuterRegister` emits
`LoadOuterLocal`; `Vector` emits a `LoadOuterLocal` first when its vector is
`Outer`, then `LoadVector`. `lower_assign` mirrors it with `StoreLocal`,
`StoreOuterLocal` and `StoreVector`, keeping both `debug_assert!`s: a write
to a register must be to one `read_only_locals` says is writable, in this
frame's table or in the target frame's.

`lower_local_initializers` stores through the same `Access` rather than
`argument_count + position`.

`Lowerer::new` emits `MakeVector { destination: vector_register, size:
vector_size }` as the body's first instruction when the layout has a vector —
before the local initializers, which already write through it. It sits
outside every `statement_program_counter` range, as the terminating return
already does.

- [ ] **Step 4: Create the right kind of child, and mark the fallback**

`lower_block_literal` builds a converted child when neither `by_reference`
nor `blocks_by_reference` is set, and a by-reference child otherwise; a
converted child's `MakeClosure` gets
`copied: self.layout.register_holding(entry, &self.captures)` for each entry
of `captures.body(BodyKey::Block(block.span)).copied`, where
`register_holding` answers the current frame's own vector register for a
`Captured::Vector` naming this body, and otherwise the register `locate`
finds. A by-reference child's list is empty.

`lower_if` and `lower_and_or` wrap their fallback arm:

```rust
    /// The real send an inlined conditional falls back to when its receiver
    /// is not a Boolean. The closures it builds out of the same block
    /// literals keep the by-reference model: such a closure can be kept and
    /// run long after this statement (`docs/design/execution.md`, "Fallback
    /// blocks").
    fn lower_fallback_send(&mut self, receiver_register: u16, message: &ast::MessageSend) -> u16 {
        let saved = std::mem::replace(&mut self.blocks_by_reference, true);
        let result = self.emit_ordinary_send_with_receiver(receiver_register, message);
        self.blocks_by_reference = saved;
        result
    }
```

`lower_while` needs none: its fallback is `mustBeBoolean`, with no closure.

`finish` sets `copied_count` from the layout and `outer_reach` from
`outer_reach_of`.

- [ ] **Step 5: Run the analysis per root body**

In `compiler/mod.rs`, each of `compile_method`, `compile_mixin_initializer`,
`compile_superclass_clause` and `compile_superclass_message` builds its
`RootBody` — the initializer's `slot_initializers` are the same expressions,
in the same order, it lowers — calls `capture::analyze`, wraps it in an `Rc`
and passes it to `Lowerer::new` with `key: BodyKey::Root`,
`by_reference: false` and `Enclosing::default()`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p ns-runtime --lib compiler::lower`, then
`cargo test --workspace`.
Expected: the four new tests pass. Other `lower.rs` tests that spell out a
block's instructions will fail where they expect the old model — read each
one and update its expectation to what the conversion produces, keeping the
test's point. **A golden or a corpus row that changes is not an expectation
to update**: stop and report it.

- [ ] **Step 7: Run every gate**

All six gate commands, including both stress modes. The stress modes matter
here: a copied value is a handle in `ClosureData`, and a vector is an object
a register and a closure both point at.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: convert closures in the optimized lowering

A captured variable that nothing writes after capture is copied into the
closure; a written one lives in its scope's indirection vector. The fallback
closure of an inlined conditional keeps the by-reference model, which is what
lets a variable written in a branch stay a register.

Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01F6LkMsqsEmuNAPegC4eL3K"
```

---

### Task 6: Fewer protective copies

**Files:**
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (`lower_operand`,
  `materialize`, `may_clobber_locals` → `writes_register`, `lower_cascade`,
  `lower_assign`)
- Modify: the `lower.rs` test module

**Interfaces:**
- Consumes: `BodyCaptures::branch_assigned` and `Layout` from Tasks 2 and 5.
- Produces: no new public name.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn an_operand_is_no_longer_copied_because_a_send_might_write_it() {
        // `t + self foo` with a mutable `t` that no inlinable branch assigns:
        // after conversion no closure can write t's register, so the operand
        // stays in its own register and no Move is emitted.
    }

    #[test]
    fn an_operand_a_branch_assigns_is_still_copied() {
        // `c ifTrue: [t:: 9]. ^t + self foo`: the fallback closure of the
        // conditional can write t's register from inside `foo`, so the
        // operand is copied out first.
    }

    #[test]
    fn an_operand_a_later_argument_assigns_is_still_copied() {
        // The existing case: `self first: t second: (t:: 9)`.
    }
```

Write each as a `lower_whole_method` plus an assertion on whether an
`Instruction::Move` stands between the operand's register and the send, in
the style of the existing `an_operand_that_a_later_argument_can_write_is_copied_out_of_its_local`.

- [ ] **Step 2: Run them to see the first one fail**

Run: `cargo test -p ns-runtime --lib compiler::lower`
Expected: the first test fails (a `Move` is still emitted); the other two
pass already.

- [ ] **Step 3: Narrow the rule**

`lower_operand` asks after lowering, when the register is known:

```rust
    fn lower_operand(&mut self, expression: &Expression, rest: &[Expression]) -> u16 {
        let register = self.lower_expression(expression);
        let clobbered = rest
            .iter()
            .any(|later| self.writes_register(later, register));
        self.materialize(register, clobbered)
    }
```

`writes_register(expression, register)` replaces `may_clobber_locals`:

- an assignment whose target resolves, in this frame, to `register` — or
  whose value expression writes it — is `true`;
- a send, a cascade or an implicit-receiver identifier is `true` only when
  `register` is one a fallback closure can write: `self.by_reference` (in a
  by-reference body, a nested closure reaches every register by reference),
  or the local in `register` is `branch_assigned`. The fallback path of a
  recognized inlinable form is a send too, and `mustBeBoolean` is a send, so
  every one of them is covered by the same question;
- for a recognized inlinable send, the blocks are lowered inline, so their
  statements are walked as part of the expression;
- a block literal that is *not* inlined is `false`: a converted closure
  writes no register, and a fallback closure's writes are the
  `branch_assigned` case above;
- a literal and a local read are `false`.

`lower_cascade` asks the same question for its receiver register, now
including the arguments of every message, not only the first's.
`lower_assign`'s setter copy becomes conditional on the same question rather
than unconditional.

- [ ] **Step 4: Run the tests and the gates**

Run: `cargo test -p ns-runtime --lib compiler::lower`, then
`cargo test --workspace` and the five other gate commands.
Expected: PASS, and `goldens/language/operand_written_by_a_callee_keeps_its_evaluated_value.ns`,
`operands_that_mutate_each_other_keep_their_evaluated_values.ns`,
`assignment_through_a_setter_answers_the_value_before_the_setter_ran.ns` and
`goldens/capture/fallback_closures_share_variables.ns` all answer exactly what
they did before. They are the point of this task: if one moves, the rule is
wrong.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "perf: copy a local out only when something can still write it

After conversion a closure writes no register of another activation, except
the fallback closure of an inlined conditional, which writes exactly the
locals that conditional assigns. A send is therefore no longer assumed to
write every local.

Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01F6LkMsqsEmuNAPegC4eL3K"
```

---

### Task 7: Measure the conversion and the copies, and present the numbers

**Files:**
- Create: nothing in the tree. Raw output goes to the session's scratchpad.

**Interfaces:**
- Consumes: the commits of Task 5 and Task 6, `efac7d2`, `be3eb40`.
- Produces: the numbers Task 8 records.

- [ ] **Step 1: Write out the four sources that are not in the tree**

`bench_do.ns` from the
[2c plan](2026-09-13-2348-ns-next-lifetime-brand-2c.md), "Measurement
protocol"; `bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` from
`docs/design/execution.md`, "What spec-conformant lookup moved" — **copied
from the text the documents give, never reconstructed from their prose**.
The other four are `goldens/bench/bench_loop.ns`, `bench_send.ns`,
`bench_block.ns` and `bench_megamorphic.ns`. Put the four written-out files
in the scratchpad.

- [ ] **Step 2: Build four binaries**

In throwaway worktrees for `efac7d2` and `be3eb40`, and at the Task 5 commit
and the Task 6 commit of the branch, each built the same way, in PowerShell:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

Copy each `target/release/ns-next.exe` out under a name that says which it
is. Check that none imports `VCRUNTIME140.dll`, and that every binary prints
the expected answer for every source before timing anything.

- [ ] **Step 3: Check that the step is still inline**

`MakeClosure` and a block's activation are on a send's path. On a release
build with debug information of `efac7d2` and of the Task 6 commit, run
`dumpbin /disasm` (Visual Studio Build Tools, not on `PATH`) and confirm that
no `vm::run::closure` is a function of its own; record `vm::run`'s
instruction count in each. If the step split, report that before timing — the
numbers would measure the split.

- [ ] **Step 4: Time**

`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the binaries rotating from round to round; two whole sittings, nothing
else running. Record every number, not a summary. Take the peak working set
for `bench_block.ns` as well, the way
[memory.md](../../design/memory.md) records it.

- [ ] **Step 5: Present the numbers**

Report to the user, in tables: each source's means per sitting for the four
binaries; the conversion (Task 5) against `efac7d2`; the copies (Task 6)
against the conversion; the head against `efac7d2` and against `be3eb40`,
each with the protocol's gate (overlapping, entirely above, entirely below);
`bench_block.ns`'s peak working set; and `vm::run`'s instruction counts. A
source entirely above `efac7d2` in both sittings is a regression to explain
before Task 8, not a number to record and move past.

---

### Task 8: Record what was built and measured

**Files:**
- Modify: `docs/design/execution.md`, `docs/design/compiler.md`,
  `docs/design/image-format.md`, `docs/design/memory.md`,
  `docs/design/README.md`, `docs/PROGRESS.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: Tasks 1–7.
- Produces: documents that describe the current state.

- [ ] **Step 1: Rewrite `execution.md`'s section as the built state**

"Planned: closure conversion" becomes "Closure conversion": what the
optimized lowering does, what a fallback closure does, what `ClosureData`
holds, how the divorce is decided, and the measurement in a subsection of
"Measurements" written the way "What 3c moved" is — the protocol, the
sources, the numbers, and what did and did not move. Correct the design's
"The divorce" paragraph to what was built: the flag is `outer_reach`, a
depth, and a `MakeClosure` marks its activation when the block's reach is
non-zero. Keep the sentence that the reference lowering is an interim oracle
and that the goldens are how this is tested once it goes.

- [ ] **Step 2: Update the other documents**

- `compiler.md`: the capture analysis, the register layout, the three
  instructions, the fallback mode, and the value-register invariant's new
  justification.
- `image-format.md`: the closure record, `copied_count`, the retired tags,
  and the new verification, in its "Verification" section.
- `memory.md`: what a surviving closure now retains, and what a returning
  frame keeps.
- `README.md`: the status line for execution.md.
- `PROGRESS.md`: "Where the next session starts" — step 4 done, with the
  numbers and the next piece of work (step 5, the collector) — and a
  "M4 — Closure conversion" section in the chronology.
- `CLAUDE.md`: the value-register invariant's paragraph (a send no longer
  writes every local; a fallback closure does write the locals its branch
  assigns), and the sentence about blocks reaching their method's variables
  by reference.

- [ ] **Step 3: Check the documents support a fresh session**

Read the changed sections as if starting from them with no memory of this
work: does `docs/design/README.md` lead to them, does `PROGRESS.md` name the
next piece of work, and does every claim about the code match the code? Fix
what does not.

- [ ] **Step 4: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: record closure conversion as built and measured

Co-Authored-By: Claude <Model Name> <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01F6LkMsqsEmuNAPegC4eL3K"
```

---

## Self-Review

**Spec coverage.** Every subsection of the design has a task: "The closure
keeps its defining activation" (Task 3), "Where the analysis lives" (Task 2),
"Fallback blocks" (Tasks 2 and 5), "Representation" (Tasks 3–5), "The image
format" (Tasks 3 and 4), "Fewer protective copies" (Task 6), "Verification
and the reference lowering" (Task 1 and every task's gates), "Cost" (Task 7).

**Type consistency.** `BodyKey`, `Place`, `Captured`, `BodyCaptures`,
`Captures`, `RootBody` and `analyze` are spelled the same in Tasks 2, 5 and 6;
`copied`, `copied_count`, `outer_reach`, `retain_registers`,
`fill_copied_values` and `closure_home` the same in Tasks 3–5;
`MakeVector`/`LoadVector`/`StoreVector` the same in Tasks 4 and 5.

**Known softness, to resolve while executing rather than by guessing.** Task
1's programs are written against this runtime's syntax as the existing
goldens use it; if one does not parse, fix the program and re-derive its
recorded output by hand, never by running it and copying what comes out
without checking it against the rule in its comment.
