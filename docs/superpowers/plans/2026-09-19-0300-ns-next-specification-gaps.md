# The Specification Gaps — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close four specification gaps that answer a correct program wrongly
at exit status 0 — the name-keyed §5.7 member set, the dropped local-slot
declaration (§3.6), the writable parameter and immutable local (§3.6), and the
non-canonical symbol (§5.1.6) — plus the register copy that §3.6 makes
unnecessary, measure them once together against `22a56f7`, and leave every
document describing what was built.

**Architecture:** Everything except the symbols happens in the resolver and
the two lowerings, above the run loop. A member set becomes a borrow of its own
declaration's set, handed down the resolver's walk. A code body's temporaries
become full `SlotDeclaration`s, whose initializers both lowerings run as
statements ahead of the body. A local scope records which of its names declare
a setter, and a setter send passes by the ones that do not. The optimized
lowering then stops copying a register nothing can write. Symbols become
canonical through one table per `Heap`, indexed by the interned number, which
`Mutator::new_symbol` — the only way to make a symbol — consults, the tracer
forwards, and the image decoder fills and polices.

**Tech Stack:** Rust 1.98.0, edition 2021, the three workspace crates; no new
dependency.

**Spec:** the living design documents, as committed in `56e2c1f` and amended
by Task 2 of this plan for the local-slot declaration:
`docs/design/dispatch.md` ("Resolution" — the member-set paragraph; "Setter
sends"; "Decisions"), `docs/design/object-model.md` ("Values and objects" —
the symbol paragraph), `docs/design/memory.md` ("What the collector traces"),
`docs/design/image-format.md` ("Verification" — one record per symbol text),
`docs/design/compiler.md` ("The value-register invariant" — a read-only local
is never copied), `docs/design/frontend.md` ("Deviation: a local slot's
initializer is dropped", which Task 2 retires). The measurement protocol is
`docs/design/execution.md`, "What removing the re-entries moved" and "What 3c
moved".

## Global Constraints

- Work on the branch `feature/specification-gaps` (already created from
  `master` at `6873f09`, holding the design commit `56e2c1f`), on the
  checked-out tree — no `git worktree` for the work itself (throwaway
  worktrees for the measurement's other binaries are fine and are removed
  afterwards). This plan is committed on the branch before Task 1.
- The baseline is **`22a56f7`**, the merge of 3c, whose last change to a send's
  path is `5d3aeff` (`docs/PROGRESS.md`). `be3eb40` is carried beside it, as in
  every measurement since step 2.
- **Outcomes are pinned, not re-derived.** Each task deletes the `skipped` line
  of the goldens it closes and adds goldens for what it newly pins. Any other
  golden, `tests-fixtures` outcome or `run-corpus` row that changes is a
  finding: stop and report it with the old and new output and the rule that
  decides between them. Never update a recorded answer because the code now
  produces it.
- Every new golden's `.ns` opens with a `(* ... *)` comment under the category
  string giving the rule, its `newspeak-spec.pdf` section and why the recorded
  answer is the right one (`docs/design/README.md`, "The goldens"). Its `.toml`
  holds the answer and nothing else.
- Names are written out (`CLAUDE.md`, "Conventions"): no identifier of one or
  two characters, no truncated word — in code, tests, goldens and commit
  messages alike. Clippy's `min_ident_chars` refuses the short ones. Newspeak
  test programs inline in Rust are `indoc! {"…"}` blocks.
- No dispatch logic in `vm.rs` or `compiler/`. Nothing in this plan needs any.
- Compiling an already-resolved module never raises a `NewspeakError`: a
  lowering that meets a binding the resolver must have excluded panics or
  `debug_assert!`s, as the existing `"internal: …"` panics do.
- **Never `{:?}` a `Value`**, never `.unwrap()`/`.expect()` an
  `EvaluationResult` or `HostResult`; tests use the local `expect_ok` helper of
  the neighbouring test file.
- An image is untrusted input: no byte sequence may panic, in a debug build
  either. Task 6's check reads nothing new from the bytes; it compares what
  was already decoded.
- A handle lives only where the tracer can rewrite it in place. The symbol
  table is a `Heap` field, forwarded by `Heap::move_everything` and counted by
  `traced_size`.
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
  Claude-Session: https://claude.ai/code/session_01B6LVA1xhDJrDhL9LA9Z2MD
  ```

- Never override the git author or committer identity.
- Not in scope: the class side's own scope (open in `dispatch.md`); lazy
  slots; simultaneous slot clauses, in a class or a code body (a code body's
  `|| … ||` keeps initializing in declaration order); a parameter and a local
  of the same name (`spec_3_6_a_parameter_and_a_local_of_the_same_name_is_an_error`
  stays skipped); `nil isNil`; mixin application; making the symbol table
  weak (step 5, the collector); a check of a decoded symbol record's class.

## File Structure

| File | Responsibility in this plan |
|---|---|
| `crates/ns-runtime/src/resolver/mod.rs` | the member set per declaration (Task 1); resolving local initializers (Task 2); the setter search (Task 3) |
| `crates/ns-runtime/src/resolver/sends.rs` | `EnclosingClass` becomes `Copy` (Task 1) |
| `crates/ns-runtime/src/resolver/scope.rs` | a local scope records which names declare a setter; `resolve_setter` (Task 3) |
| `crates/ns-syntax/src/ast.rs`, `parser/declaration.rs`, `parser/mod.rs` | `CodeBody.temporaries` keeps each `SlotDeclaration` (Task 2) |
| `crates/ns-runtime/src/compiler/lower.rs`, `compiler/mod.rs` | local initializers as statements (Task 2); the read-only locals and `materialize` (Task 4) |
| `crates/ns-runtime/src/compiler/reference.rs` | local initializers as statements (Task 2) |
| `crates/ns-runtime/src/bytecode.rs` | `statement_program_counter`'s doc comment (Task 2) |
| `crates/ns-runtime/src/symbol.rs` | `Symbol::index` (Task 5) |
| `crates/ns-runtime/src/heap.rs` | the canonical symbol table, its forwarding and count (Task 5) |
| `crates/ns-runtime/src/interpreter/mod.rs` | `new_symbol` consults the table (Task 5) |
| `crates/ns-runtime/src/image/decode.rs` | a decoded symbol enters the table; `DecodeError::DuplicateSymbol` (Task 6) |
| `crates/ns-runtime/tests/image.rs`, `tests/builtins_text.rs` | the image and identity tests (Tasks 5, 6) |
| `goldens/spec/*` | closing the four skipped goldens, and the new ones (Tasks 1–3, 5) |
| `docs/design/*.md`, `docs/PROGRESS.md`, `CLAUDE.md` | each document says what was built (Tasks 2, 4, 8) |

---

### Task 1: A member set belongs to its declaration (§5.7)

**Files:**
- Modify: `crates/ns-runtime/src/resolver/mod.rs:20-135` (`resolve`,
  `build_member_sets`, `collect_member_sets`, `class_chain_from`,
  `resolve_class`) and `:186-195` (the nested-class recursion)
- Modify: `crates/ns-runtime/src/resolver/sends.rs:14-17` (`EnclosingClass`)
- Modify: `goldens/spec/spec_5_7_a_member_set_is_per_declaration_not_per_name.toml`
- Create: `goldens/spec/spec_5_7_a_namesakes_members_are_not_in_reach.ns`, `.toml`

**Interfaces:**
- Consumes: `classes::own_member_names(&ast::ClassDeclaration) -> HashSet<Symbol>` (unchanged).
- Produces: `Resolver::resolve_class(&mut self, declaration: &ast::ClassDeclaration, enclosing: &[EnclosingClass<'_>])`,
  where `enclosing` is innermost first; `EnclosingClass` derives `Clone, Copy`.
  Tasks 2 and 3 edit the same function and keep this signature.

- [ ] **Step 1: Write the new golden**

`goldens/spec/spec_5_7_a_namesakes_members_are_not_in_reach.ns`:

```
Newspeak3
'Test'
(* newspeak-spec.pdf 5.7: the receiver of an implicit send is "the
   innermost lexically enclosing construct in which a method, slot or class
   named s is declared". Beta's Dup declares no marker, so the marker in
   reach is Beta's own; Alpha's Dup declares one, but Alpha's Dup encloses
   nothing here.

   The mirror image of spec_5_7_a_member_set_is_per_declaration_not_per_name:
   there a member of the right class went missing, here a member of an
   unrelated namesake appears. With member sets keyed by the simple name,
   Alpha's Dup -- collected last -- lends its set to Beta's Dup, the search
   stops at Dup itself, and the send answers doesNotUnderstand:. Together
   the two goldens pin that the answer depends on neither the name nor the
   order the declarations are collected in. *)
class Spec57NamesakesMembersAreNotInReach usingPlatform: p = Object new (| t = p transcript. |) (
    class Beta = Object new () (
        marker = ( ^'Beta''s marker' )
        class Dup = Object new () (
            public reach = ( ^marker )
        ) : ()
        public probe = ( ^Dup new reach )
    ) : ()
    class Alpha = Object new () (
        class Dup = Object new () (
            marker = ( ^'Alpha''s Dup''s marker' )
        ) : ()
    ) : ()
    public main: platform args: arguments = (
        t show: Beta new probe; newLine
    )
) : ()
```

`goldens/spec/spec_5_7_a_namesakes_members_are_not_in_reach.toml`:

```toml
stdout = '''
Beta's marker
'''
```

Delete the `skipped = …` line from
`goldens/spec/spec_5_7_a_member_set_is_per_declaration_not_per_name.toml`.

- [ ] **Step 2: Run the goldens to see both fail**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: FAIL for both goldens — the new one with
`Uncaught Newspeak error: an object does not understand #marker` (status 1),
the unskipped one answering `an enclosing marker`.

- [ ] **Step 3: Hand the chain down instead of looking sets up by name**

In `sends.rs`, derive `Clone, Copy` on `EnclosingClass` (it holds a `Symbol`
and a shared reference).

In `resolver/mod.rs`, delete `build_member_sets`, `collect_member_sets` and
`class_chain_from` together with their doc comments, and the `member_sets`
local in `resolve`, which becomes:

```rust
    resolver.resolve_class(&module.root_class, &[]);
```

`resolve_class` takes the enclosing chain itself and borrows its own set:

```rust
    /// `enclosing` is the chain of declarations around this one, innermost
    /// first. Each level borrows the member set of *its own* declaration,
    /// computed here as the walk enters it, so two classes of one simple name
    /// can never share or overwrite a set (`docs/design/dispatch.md`,
    /// "Resolution"). Nothing inherited goes in: §5.7 looks for the innermost
    /// lexically enclosing construct in which a member "is declared", and
    /// §2.3.1 proceeds "up the inheritance chain of self" only once no
    /// lexically visible declaration matches -- dispatch's job at run time.
    fn resolve_class(
        &mut self,
        declaration: &ast::ClassDeclaration,
        enclosing: &[EnclosingClass<'_>],
    ) {
        let name = Symbol::intern(&declaration.name.name);
        for duplicate in classes::duplicate_member_names(declaration) {
            self.errors.push(ResolveError {
                span: declaration.name.span,
                message: format!("class `{name}` has multiple definitions of `{duplicate}`"),
            });
        }
        let own_members = classes::own_member_names(declaration);
        let class_chain: Vec<EnclosingClass<'_>> =
            std::iter::once(EnclosingClass {
                name,
                members: &own_members,
            })
            .chain(enclosing.iter().map(|class| EnclosingClass {
                name: class.name,
                members: class.members,
            }))
            .collect();
```

The superclass clause keeps resolving against the declarations *around* this
one — now `enclosing` itself:

```rust
        if let Some(clause) = &declaration.superclass {
            self.resolve_superclass_clause(clause, enclosing);
        }
```

The rest of the body keeps using `&class_chain` where it used `&class_chain`
before, and both nested-class loops become
`self.resolve_class(nested, &class_chain);`. Remove the now-unused `HashSet`
import if clippy says so.

- [ ] **Step 4: Run the goldens and the resolver tests**

Run: `cargo test -p ns-cli --test run_goldens` and `cargo test -p ns-runtime resolver`
Expected: PASS, both goldens under both lowerings and through the snapshot
and build/load round trips.

- [ ] **Step 5: Update the design record and commit**

In `docs/design/dispatch.md`, "Resolution": replace the paragraph
**"Defect: a member set is keyed by the class's simple name."** and the
**"Decided (2026-09-19), planned …"** paragraph after it with one paragraph
in the present tense — a member set is its declaration's, borrowed down the
walk; why the name-keyed map was wrong (eighteen corpus modules have the
shape; the reference lowering agreed with the wrong answer, since the mistake
was above both lowerings); the two goldens that pin it. In "Decisions", make
the member-set half of the 2026-09-19 entry say it is built. In
`docs/design/README.md`'s deviations table, delete the row "A §5.7 member set
is keyed by a class's simple name …", and in the paragraphs around the table
drop "the name-keyed member set" from the list of rows that answer wrongly
(the count "Six do worse" becomes "Five") and from "Five of these have a
golden" (which becomes "Four", its list losing the member set).

```bash
cargo fmt --all
git add crates/ns-runtime/src/resolver goldens/spec docs/design
git commit -m "fix: key a member set by its declaration, not its class's name"
```

(with the two trailer lines)

---

### Task 2: A local slot is what its declaration says (§3.6)

**Files:**
- Modify: `crates/ns-syntax/src/ast.rs:96-102` (`CodeBody.temporaries`)
- Modify: `crates/ns-syntax/src/parser/declaration.rs:47-72`
  (`parse_temporaries`), its tests near `:1384-1395`, and
  `crates/ns-syntax/src/parser/mod.rs:503`
- Modify: `crates/ns-runtime/src/resolver/mod.rs` — `resolve_method`
  (`:198-213`), the `Expression::Block` arm of `resolve_expression`
  (`:249-268`)
- Modify: `crates/ns-runtime/src/compiler/lower.rs` — `lower_block_literal`
  (`:283-315`), a new `lower_local_initializers`; `compiler/mod.rs:57-72`
  (`compile_method`)
- Modify: `crates/ns-runtime/src/compiler/reference.rs` — `compile_method`
  (`:25-35`), `block` (`:429-455`)
- Modify: `crates/ns-runtime/src/bytecode.rs:251-256` (doc comment)
- Modify: `goldens/spec/spec_3_6_a_local_slots_initializer_runs.toml`
- Create: `goldens/spec/spec_3_6_local_slot_initializers_run_in_declaration_order.ns`, `.toml`
- Modify: `docs/design/frontend.md`, `docs/design/README.md`,
  `docs/design/dispatch.md`, `docs/design/compiler.md`

**Interfaces:**
- Consumes: `Parser::parse_slot_declarations(&mut self) -> Vec<SlotDeclaration>` (unchanged).
- Produces: `ast::CodeBody.temporaries: Vec<SlotDeclaration>`;
  `Lowerer::lower_local_initializers(&mut self, temporaries: &[ast::SlotDeclaration])`;
  the rule that a code body's statement indices are its local initializers
  first, then its statements. Task 3 reads `SlotDeclaration.mutability`,
  Task 4 reads it again.

- [ ] **Step 1: Write the new golden and unskip the old one**

`goldens/spec/spec_3_6_local_slot_initializers_run_in_declaration_order.ns`:

```
Newspeak3
'Test'
(* newspeak-spec.pdf 3.6: an activation has "zero or more local slots, one
   for each slot declaration in the body of p. Local slots are defined by
   their corresponding declarations (6.3.2). They are initialized in the
   context of a, according to their declarations." So each initializer runs
   when its activation starts, in declaration order, and sees the slots
   declared before it; a slot with no initializer stays nil (6.3.2); a
   block's local slots are initialized per block activation, from its
   parameters; and a block whose body declares slots but has no statement
   still answers nil. The class-level counterpart, a slot initializer, is
   what the instance initializer already does. *)
class Spec36LocalSlotInitializersRunInDeclarationOrder usingPlatform: p = Object new (| t = p transcript. |) (
    public main: platform args: arguments = (
        | first = 3. second ::= first + 4. third |
        t show: first printString; newLine.
        t show: second printString; newLine.
        t show: third printString; newLine.
        [:argument | | doubled = argument * 2. | t show: doubled printString; newLine] value: 5.
        t show: [| only = 1. |] value printString; newLine.
    )
) : ()
```

`…_run_in_declaration_order.toml`:

```toml
stdout = '''
3
7
nil
10
nil
'''
```

Delete the `skipped = …` line from
`goldens/spec/spec_3_6_a_local_slots_initializer_runs.toml`.

- [ ] **Step 2: Run the goldens to see them fail**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: FAIL for both — the slots stay nil, so the old one prints `nil`
and the new one fails at `nil + 4`.

- [ ] **Step 3: Keep the declaration in the AST**

In `ast.rs`, `CodeBody.temporaries` becomes `Vec<SlotDeclaration>`. In
`parse_temporaries`, return `Vec<SlotDeclaration>`: the `DoubleBar` and
no-`Bar` cases answer `Vec::new()` as now, and the last line is
`self.parse_slot_declarations()` without the `map`. Rewrite its doc comment:
a temporary list has the full slot-declaration form (grammar `:148-150`) and
keeps it, so the runtime can initialize each slot and tell an immutable one
from a mutable one; `lazy` is still dropped, as it is for class slots. Fix the
parser tests that read `temporaries` (`parser/mod.rs:503` keeps its length
check; add one assertion there, or in a new test beside
`empty_double_bar_temporaries`, that `| x = 1. y ::= 2. z |` parses to three
declarations with mutabilities `Immutable`, `MutableInitialized`,
`MutableBare` and initializers present, present, absent).

- [ ] **Step 4: Resolve each initializer in its body's scope**

`resolve_method` builds its names from the declarations and resolves every
initializer, in declaration order, before the statements — with the whole
body's scope pushed, so an initializer sees every local, as a class slot's
initializer sees every slot:

```rust
        let temporary_names: Vec<Symbol> = method
            .body
            .temporaries
            .iter()
            .map(|temporary| Symbol::intern(&temporary.name.name))
            .collect();
        // … `all`, `scope` as before …
        self.resolve_local_initializers(&method.body.temporaries, &mut scope, class_chain);
        for statement in &method.body.statements {
```

The `Expression::Block` arm does the same after `scope.push(…)`. The helper:

```rust
    /// A local slot's initializer runs in the activation it belongs to
    /// (§3.6), so it resolves in the scope its body's statements do.
    fn resolve_local_initializers(
        &mut self,
        temporaries: &[ast::SlotDeclaration],
        scope: &mut ScopeStack,
        chain: &[EnclosingClass],
    ) {
        for temporary in temporaries {
            if let Some(initializer) = &temporary.initializer {
                self.resolve_expression(initializer, scope, chain);
            }
        }
    }
```

- [ ] **Step 5: Lower each initializer as a statement, in both lowerings**

In `lower.rs`, beside `lower_slot_initializer`:

```rust
    /// Each local slot with an initializer, in declaration order, as one
    /// statement: its expression, then a `StoreLocal` into the slot's own
    /// register, which follows the parameters (§3.6). These are the body's
    /// first statements, so an activation's statement indices run through
    /// its local initializers before its statements, the way an instance
    /// initializer's run through its slot initializers before its
    /// `initExprs`. They leave `last_expression_register` alone: a body's
    /// value is its last *statement's*, and a body with none answers nil.
    pub(crate) fn lower_local_initializers(&mut self, temporaries: &[ast::SlotDeclaration]) {
        for (position, temporary) in temporaries.iter().enumerate() {
            let Some(initializer) = &temporary.initializer else {
                continue;
            };
            self.next_scratch = self.base;
            let start = self.instructions.len();
            let source = self.lower_expression_for(initializer, ValueUse::Read);
            let slot = self.argument_count + position as u16;
            self.emit(Instruction::StoreLocal { source, slot });
            let end = self.instructions.len();
            self.statement_program_counter
                .push((self.statement_index, start..end));
            self.statement_index += 1;
        }
    }
```

Call it before `lower_body` in `compiler/mod.rs`'s `compile_method` and in
`lower_block_literal` (`inner.lower_local_initializers(&block.body.temporaries);`),
and in the test helper `lower_whole_method` if it lowers a method body on its
own. The `.len()` uses of `temporaries` stay as they are.

In `reference.rs`, the same, in that module's own idiom (`Body::value`,
`record_statement`) — a `Body` method called from `compile_method` and from
`block` before `statements_and_end`:

```rust
    fn local_initializers(&mut self, temporaries: &[ast::SlotDeclaration]) {
        for (position, temporary) in temporaries.iter().enumerate() {
            let Some(initializer) = &temporary.initializer else {
                continue;
            };
            let start = self.instructions.len();
            let source = self.value(initializer);
            let slot = self.argument_count + position as u16;
            self.emit(Instruction::StoreLocal { source, slot });
            self.record_statement(start);
        }
    }
```

(If `Body` does not keep its argument count, keep it: `Body::new` receives
it.) Update `CompiledCode.statement_program_counter`'s doc comment in
`bytecode.rs`: for a method or a block, each local initializer, then each
statement.

- [ ] **Step 6: Run everything**

Run: `cargo test --workspace`
Expected: PASS, the two goldens included. Any other golden or fixture that
changes output is a finding (Global Constraints): report it.

- [ ] **Step 7: Retire the deviation and commit**

- `docs/design/frontend.md`: delete "Deviation: a local slot's initializer is
  dropped"; where the section before it counts "Three of the things dropped
  here change what a correct program answers", make the count and the list
  agree with what remains; describe, where temporaries are described, that a
  temporary list keeps the full declaration.
- `docs/design/README.md`: delete the row "A local slot's initializer is
  dropped …"; update the counts and lists in the paragraphs around the table
  (the rows that answer wrongly, and the rows with a golden).
- `docs/design/dispatch.md`, "Setter sends": the planned paragraph's
  `| x = e. |` now exists in the AST; say nothing about it there beyond what
  Task 3 builds.
- `docs/design/compiler.md`: where the lowering of a method or block body is
  described, add that its local initializers are its first statements.

```bash
cargo fmt --all
git add crates goldens/spec docs/design
git commit -m "fix: keep a local slot's declaration and run its initializer"
```

---

### Task 3: A setter send passes by a local with no setter (§3.6, §5.7)

**Files:**
- Modify: `crates/ns-runtime/src/resolver/scope.rs` (whole file: `LocalScope`,
  `ScopeStack`, tests)
- Modify: `crates/ns-runtime/src/resolver/mod.rs` — the three
  `LocalScope::new` calls (factory parameters in `resolve_class`,
  `resolve_method`, the block arm), `resolve_setter_target` (`:384-407`)
- Modify: `goldens/spec/spec_3_6_a_parameter_slot_is_immutable.toml`
- Create, each as `.ns` + `.toml` under `goldens/spec/`:
  `spec_3_6_an_immutable_local_slot_has_no_setter`,
  `spec_3_6_a_block_parameter_is_immutable`,
  `spec_3_6_a_factory_parameter_is_immutable`,
  `spec_5_7_a_setter_send_passes_a_shadowing_parameter_by`

**Interfaces:**
- Consumes: `ast::SlotDeclaration.mutability` (Task 2).
- Produces: `LocalScope::new(parameters: &[Symbol], temporaries: &[ast::SlotDeclaration]) -> LocalScope`;
  `ScopeStack::resolve_setter(&self, name: Symbol) -> Option<Binding>`.
  After this task no `Binding::Local`/`OuterLocal` on an assignment target
  names a parameter or an immutable local — Task 4 relies on it.

- [ ] **Step 1: Write the goldens**

Delete the `skipped = …` line from
`goldens/spec/spec_3_6_a_parameter_slot_is_immutable.toml`.

`spec_3_6_an_immutable_local_slot_has_no_setter.ns`:

```
Newspeak3
'Test'
(* newspeak-spec.pdf 3.6: local slots "are defined by their corresponding
   declarations (6.3.2)", and 6.3.2 gives an immutable slot no setter:
   "There is no setter method associated with the slot". So the innermost
   construct declaring x: (5.7) is the class, whose method x: records what
   it is sent, and the local keeps its initial value. The canonical compiler
   enters an immutable temporary's getter alone into its scope
   (prior-art/newspeak/NewspeakCompilation.ns, codeBodyNode:). *)
class Spec36AnImmutableLocalSlotHasNoSetter usingPlatform: p = Object new (| t = p transcript. log ::= 'nothing'. |) (
    x: value = ( log:: value printString )
    public main: platform args: arguments = (
        | x = 1. |
        x:: 9.
        t show: x printString; newLine.
        t show: log; newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
1
9
'''
```

`spec_3_6_a_block_parameter_is_immutable.ns`:

```
Newspeak3
'Test'
(* newspeak-spec.pdf 3.6: "Parameter slots are immutable (6.3.2)", for a
   closure activation as for a method activation. Nothing declares value:,
   so 5.7 makes the setter send a self send, which nothing answers. *)
class Spec36ABlockParameterIsImmutable usingPlatform: p = Object new (| t = p transcript. |) (
    public main: platform args: arguments = (
        t show: ([:value | value:: 5. value] value: 1) printString; newLine.
    )
) : ()
```

`.toml`:

```toml
status = 1
stderr = '''
Uncaught Newspeak error: an object does not understand #value:
'''
```

`spec_3_6_a_factory_parameter_is_immutable.ns`:

```
Newspeak3
'Test'
(* newspeak-spec.pdf 3.6: "Parameter slots are immutable (6.3.2)". A
   class's primary factory parameters are the parameters of the instance
   initializer's activation, where the slot initializers and initExprs run
   (6.3), so `initial:: 9` there is the self send initial:, which nothing
   answers. (The parameter is not named after the factory selector's
   keyword, so the refusal cannot be mistaken for one about the factory,
   which is a class-side member.) *)
class Spec36AFactoryParameterIsImmutable usingPlatform: p = Object new (| t = p transcript. |) (
    class Holder seed: initial = Object new (| stored ::= 0. | initial:: 9. stored:: initial ) (
        public contents = ( ^stored )
    ) : ()
    public main: platform args: arguments = (
        t show: (Holder seed: 1) contents printString; newLine.
    )
) : ()
```

`.toml`:

```toml
status = 1
stderr = '''
Uncaught Newspeak error: an object does not understand #initial:
'''
```

`spec_5_7_a_setter_send_passes_a_shadowing_parameter_by.ns`:

```
Newspeak3
'Test'
(* newspeak-spec.pdf 5.4.3 makes `v:: e` a send of v:, and 5.7 sends it to
   the innermost lexically enclosing construct in which v: is declared. The
   block's parameter v declares a getter only (3.6), so that construct is
   the method, whose mutable local v declares v:. The write lands on the
   method's v while v in the block still reads the parameter. This is what
   the canonical compiler does: its scopes are keyed by selector, and a
   setter send searches them outward
   (prior-art/newspeak/NewspeakCompilation.ns, setterSendNode:). *)
class Spec57ASetterSendPassesAShadowingParameterBy usingPlatform: p = Object new (| t = p transcript. |) (
    public main: platform args: arguments = (
        | v |
        v:: 'the method''s own'.
        [:v | v:: 'written from the block'. t show: v; newLine] value: 'the parameter'.
        t show: v; newLine.
    )
) : ()
```

`.toml`:

```toml
stdout = '''
the parameter
written from the block
'''
```

- [ ] **Step 2: Run the goldens to see them fail**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: FAIL for all five — each write lands on the local today.

- [ ] **Step 3: Record which locals declare a setter**

`scope.rs` becomes:

```rust
/// One method or closure body's own parameters and local slots, innermost
/// activation last, each with whether it declares a setter: §3.6 makes a
/// parameter immutable and a local slot what its declaration says (§6.3.2),
/// so only a mutable local slot has one.
pub(super) struct LocalScope {
    names: HashMap<Symbol, LocalEntry>,
}

#[derive(Clone, Copy)]
struct LocalEntry {
    index: u16,
    has_setter: bool,
}

impl LocalScope {
    /// Indices follow the activation's layout: the parameters, then the
    /// local slots, each in declaration order.
    pub(super) fn new(parameters: &[Symbol], temporaries: &[ast::SlotDeclaration]) -> LocalScope {
        let declared = parameters.iter().map(|&name| (name, false)).chain(
            temporaries.iter().map(|temporary| {
                (
                    Symbol::intern(&temporary.name.name),
                    temporary.mutability != ast::SlotMutability::Immutable,
                )
            }),
        );
        let mut names = HashMap::new();
        for (index, (name, has_setter)) in declared.enumerate() {
            names.insert(
                name,
                LocalEntry {
                    index: index as u16,
                    has_setter,
                },
            );
        }
        LocalScope { names }
    }
}
```

`ScopeStack::resolve` keeps its behaviour, reading `entry.index`. Add, with
the binding construction shared between the two through a small private
function:

```rust
    /// Resolves the target of `name:: e` against the activation chain: the
    /// innermost local that declares a setter. A local without one is passed
    /// by, whether or not it shadows one further out -- the setter send looks
    /// for a declaration of `name:` (§5.4.3, §5.7), and a parameter or an
    /// immutable local declares only `name`. `None` hands the search on to
    /// the enclosing classes.
    pub(super) fn resolve_setter(&self, name: Symbol) -> Option<Binding> {
        for (depth, scope) in self.scopes.iter().rev().enumerate() {
            if let Some(entry) = scope.names.get(&name) {
                if entry.has_setter {
                    return Some(binding_at(depth, entry.index));
                }
            }
        }
        None
    }
```

where `binding_at(depth, index)` answers `Binding::Local(index)` at depth 0
and `Binding::OuterLocal { depth: depth as u16, index }` otherwise.

Update the three callers in `resolver/mod.rs`:
`LocalScope::new(&factory_parameters, &[])`,
`LocalScope::new(&parameter_names, &method.body.temporaries)` (the `all`
vector goes away), and the block arm's
`LocalScope::new(&parameter_names, &block.body.temporaries)`. In
`resolve_setter_target`, `scope.resolve(name)` becomes
`scope.resolve_setter(name)`, and its doc comment says only a local that
declares a setter wins — a parameter or immutable local is passed by, as in
the canonical compiler — and cites `docs/design/dispatch.md`, "Setter sends".

Update the tests in `scope.rs` to the new constructor and add two: a
parameter is found by `resolve` but not by `resolve_setter`; an inner
parameter shadowing an outer mutable temporary of the same name makes
`resolve_setter` answer the outer one (`OuterLocal { depth: 1, … }`). Build
the `SlotDeclaration`s the tests need with a small helper in the test module
(spans `Span::new(0, 0)`).

- [ ] **Step 4: Run everything**

Run: `cargo test --workspace`
Expected: PASS, the five goldens included.

- [ ] **Step 5: Record it and commit**

In `docs/design/dispatch.md`, "Setter sends": the "Decided (2026-09-19),
planned …" paragraph becomes present tense — built, with the goldens that pin
it; keep the canonical compiler and SOMns comparison and the shadowing
consequence. In "Decisions", make the setter half of the 2026-09-19 entry say
it is built. In `docs/design/README.md`, drop "a parameter slot accepts a
setter send" from the closing paragraph's list of rules not implemented yet.

```bash
cargo fmt --all
git add crates/ns-runtime/src/resolver goldens/spec docs/design
git commit -m "fix: pass a setter send by a parameter or an immutable local"
```

---

### Task 4: A read-only local is never copied

**Files:**
- Modify: `crates/ns-runtime/src/compiler/lower.rs` — `Lowerer` fields and
  `Lowerer::new` (`:60-103`), `materialize` (`:316-336`), `lower_assign`
  (`:517-570`), `lower_block_literal`, the test helpers and tests
- Modify: `crates/ns-runtime/src/compiler/mod.rs` — every `Lowerer::new` call
  (`compile_method`, `compile_mixin_initializer`, the superclass clause and
  message)
- Modify: `docs/design/compiler.md` ("The value-register invariant"),
  `CLAUDE.md` (the value-register invariant)

**Interfaces:**
- Consumes: `LocalScope`'s rule from Task 3 (no assignment binds to a
  parameter or an immutable local); `SlotDeclaration.mutability`.
- Produces: `Lowerer::new(kind: CodeKind, argument_count: u16, temporaries: &[ast::SlotDeclaration], bindings: &'a Rc<HashMap<Span, Binding>>, enclosing_scopes: Vec<ScopeKind>, first_call_site: u32) -> Lowerer<'a>`.

- [ ] **Step 1: Write the failing tests**

In `lower.rs`'s test module, beside
`an_operand_no_later_argument_can_write_stays_in_its_own_register`:

```rust
    /// A parameter cannot be written once its activation starts (§3.6), so
    /// the send in `rest` that would force a copy out of a mutable local
    /// forces none here.
    #[test]
    fn a_parameter_operand_is_never_copied() {
        let (method, bindings) = resolve_method(
            indoc! {"
                Newspeak3
                'T'
                class X = Object new (
                ) (
                  m: argument = ( ^argument foo: self bar )
                ) : ()
            "},
            0,
        );
        let mut lowerer = method_lowerer(&method, &bindings); // `argument` is register 0
        lowerer.lower_expression(return_expression(&method));
        let code = lowerer.finish(method.span);
        assert!(
            !code
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { .. })),
            "a parameter needs no copy: {:?}",
            code.instructions
        );
    }

    /// Likewise an immutable local: only its initializer writes it.
    #[test]
    fn an_immutable_local_operand_is_never_copied() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              m = ( | local = 1. | ^local foo: self bar )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // `local` is register 0
        assert!(
            !code
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { source: 0, .. })),
            "an immutable local needs no copy: {:?}",
            code.instructions
        );
    }
```

and the counterpart that keeps the copy where it is still needed:

```rust
    /// A mutable local still is copied: the send can write it through a
    /// closure that captured the frame.
    #[test]
    fn a_mutable_local_operand_is_still_copied() {
        let source = indoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              m = ( | local ::= 1. | ^local foo: self bar )
            ) : ()
        "};
        let code = lower_whole_method(source, 0); // `local` is register 0
        assert!(
            code.instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Move { source: 0, .. })),
            "a mutable local is copied out: {:?}",
            code.instructions
        );
    }
```

Check before writing them that `lower_whole_method` runs Task 2's local
initializers; if the receiver of a keyword send is not lowered through
`lower_operand`, put the local in argument position before a later argument
that sends instead (`^self foo: local bar: self baz`), and keep that shape in
all three tests.

- [ ] **Step 2: Run the tests to see the first two fail**

Run: `cargo test -p ns-runtime compiler::lower`
Expected: `a_parameter_operand_is_never_copied` and
`an_immutable_local_operand_is_never_copied` FAIL (a `Move` out of register
0); `a_mutable_local_operand_is_still_copied` passes.

- [ ] **Step 3: Teach the lowerer which registers are read-only**

`Lowerer` gains a field:

```rust
    /// For each named local's register (`< base`), whether nothing but its
    /// initialization can write it: every parameter, and every immutable
    /// local slot (§3.6). The resolver binds no assignment to one
    /// (`docs/design/dispatch.md`, "Setter sends"), so an operand read from
    /// one survives anything lowered after it.
    read_only_locals: Vec<bool>,
```

`Lowerer::new` takes `temporaries: &[ast::SlotDeclaration]` in place of
`named_temporary_count: u16`, computes `base` as
`argument_count + temporaries.len() as u16`, and

```rust
        let read_only_locals: Vec<bool> = std::iter::repeat(true)
            .take(argument_count as usize)
            .chain(temporaries.iter().map(|temporary| {
                temporary.mutability == ast::SlotMutability::Immutable
            }))
            .collect();
```

Callers pass `&method.body.temporaries`, `&block.body.temporaries`, or `&[]`
where they passed `0` (the initializer and the superclass bodies, whose
factory parameters are parameters all the same); the test helpers
`method_lowerer` and the direct `Lowerer::new(CodeKind::Initializer, 1, 0, …)`
calls follow.

`materialize`:

```rust
    fn materialize(&mut self, register: u16, clobbered: bool) -> u16 {
        if !clobbered
            || register >= self.base
            || self.read_only_locals[register as usize]
        {
            return register;
        }
```

and its doc comment adds the third case: a read-only local, which nothing
lowered afterwards can write.

In `lower_assign`, both branches that emit a `StoreLocal` into this frame
assert the resolver's guarantee:

```rust
                debug_assert!(
                    !self.read_only_locals[index as usize],
                    "internal: an assignment bound to a parameter or an immutable local \
                     (the resolver passes a setter send by them)"
                );
```

`lower_local_initializers` (Task 2) writes a read-only register by design and
does not go through `lower_assign`, so it needs no exemption.

- [ ] **Step 4: Run the lowering tests, then everything**

Run: `cargo test -p ns-runtime compiler::lower`
Expected: the three new tests PASS. Tests that pinned a copy *out of a
parameter* now fail — `a_slot_initializer_that_assigns_a_slot_copies_its_value_out_of_the_local`
at least, whose `foo: p` hands a parameter to a setter. Do not delete one:
rewrite each so the named local is a mutable temporary (`foo = ( | p | ^n := p )`,
with the lowerer given that method's temporaries), which keeps what it pins,
and add beside it the parameter version asserting that no `Move` is emitted.

Run: `cargo test --workspace`
Expected: PASS. The goldens and `run-corpus` are the check that no program's
output moved.

- [ ] **Step 5: Record it and commit**

- `docs/design/compiler.md`, "The value-register invariant": the planned
  paragraph becomes present tense — built, the precondition stated as what
  the `debug_assert!` in `lower_assign` and the resolver's rule guarantee.
- `CLAUDE.md`, "An expression's value register must still hold that value
  when its consumer executes": after "`lower_operand` copies it when anything
  lowered afterwards can write it", add that a parameter's or an immutable
  local's register is never copied, because no assignment can bind to one
  (§3.6) — and that this rests on the resolver passing a setter send by them.

```bash
cargo fmt --all
git add crates/ns-runtime/src/compiler docs/design/compiler.md CLAUDE.md
git commit -m "perf: never copy a parameter's or an immutable local's register"
```

---

### Task 5: One symbol object per symbol, per heap (§5.1.6)

**Files:**
- Modify: `crates/ns-runtime/src/symbol.rs` (`Symbol::index`)
- Modify: `crates/ns-runtime/src/heap.rs` — `Heap` (`:408-454`), `Heap::new`
  (`:613-631`), the `Debug` impl (`:466-485`), `traced_size` (`:2119-2126`),
  `move_everything` (`:2150-2188`), new accessors
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:1205-1208` (`new_symbol`)
- Modify: `goldens/spec/spec_5_1_6_equal_symbols_are_one_object.toml`
- Test: `crates/ns-runtime/tests/builtins_text.rs`

**Interfaces:**
- Produces: `Symbol::index(self) -> usize` (`pub(crate)`);
  `Heap::canonical_symbol(&self, symbol: Symbol) -> Option<ObjectReference<'gc>>`;
  `Heap::record_canonical_symbol(&mut self, symbol: Symbol, object: ObjectReference<'gc>)`.
  Task 6 uses both `Heap` methods.

- [ ] **Step 1: Unskip the golden and write the identity tests**

Delete the `skipped = …` line from
`goldens/spec/spec_5_1_6_equal_symbols_are_one_object.toml`.

In `crates/ns-runtime/tests/builtins_text.rs`, following that file's
`expect_ok` and module-running helpers, add a test that runs:

```
Newspeak3
'Test'
class Symbols usingPlatform: p = Object new () (
    public literalAndDerived = ( ^#foo == 'foo' asSymbol )
    public twoDerived = ( ^'bar' asSymbol == 'bar' asSymbol )
) : ()
```

and asserts both sends answer `true`; and a test that a selector handed to a
`doesNotUnderstand:` handler is the literal's object:

```
Newspeak3
'Test'
class Selectors usingPlatform: p = Object new () (
    public doesNotUnderstand: message = ( ^(message at: 1) == #frobnicate )
    public probe = ( ^self frobnicate )
) : ()
```

`probe` answers `true`. (`doesNotUnderstand:`'s argument is a two-element
tuple of the selector and the arguments, `docs/design/README.md`'s
deviations; index it the way the existing `doesNotUnderstand:` tests do if
`at:` is not how they read it.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime --test builtins_text` and
`cargo test -p ns-cli --test run_goldens`
Expected: FAIL — `==` answers `false` everywhere.

- [ ] **Step 3: Add the table**

`symbol.rs`:

```rust
    /// The interned number, dense from 0 in interning order, so a table
    /// indexed by it has one slot per symbol ever interned and no hashing.
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
```

`Heap` gains, after `objects`:

```rust
    /// The one object each symbol is, indexed by `Symbol::index` (§5.1.6:
    /// equal symbols are one object; `docs/design/object-model.md`,
    /// "Values and objects"). Filled lazily by `Mutator::new_symbol`, the one
    /// way to make a symbol, and by the image decoder. Per heap, because a
    /// handle means nothing against another heap. A root the tracer
    /// forwards; a strong one until the collector makes it weak
    /// (`docs/design/memory.md`, "What the collector traces").
    canonical_symbols: Vec<Option<ObjectReference<'gc>>>,
```

initialized to `Vec::new()` in `Heap::new`, reported as a count in the
`Debug` impl beside the others, and the two accessors:

```rust
    pub(crate) fn canonical_symbol(&self, symbol: Symbol) -> Option<ObjectReference<'gc>> {
        self.canonical_symbols.get(symbol.index()).copied().flatten()
    }

    pub(crate) fn record_canonical_symbol(&mut self, symbol: Symbol, object: ObjectReference<'gc>) {
        let index = symbol.index();
        if self.canonical_symbols.len() <= index {
            self.canonical_symbols.resize(index + 1, None);
        }
        self.canonical_symbols[index] = Some(object);
    }
```

In `move_everything`, after the stack values:

```rust
        for object in self.canonical_symbols.iter_mut().flatten() {
            forwarder.object(object);
        }
```

and `traced_size` adds `self.canonical_symbols.len()`, its doc comment naming
the table.

`Mutator::new_symbol`:

```rust
    /// The one object `symbol` is in this heap, allocated the first time
    /// anything asks for it.
    pub fn new_symbol(&mut self, symbol: Symbol) -> Value<'gc> {
        if let Some(object) = self.heap.canonical_symbol(symbol) {
            return Value::Object(object);
        }
        let class = self.builtin_class(BuiltinClass::Symbol);
        let object = self.new_instance_object(class, ObjectData::Symbol(symbol));
        self.heap.record_canonical_symbol(symbol, object);
        Value::Object(object)
    }
```

Check with `grep -rn "ObjectData::Symbol(" crates/ns-runtime/src` that
nothing but `new_symbol` and the image decoder builds a symbol object.

- [ ] **Step 4: Run everything, including the move stress**

Run: `cargo test --workspace`, then in Git Bash
`NS_NEXT_STRESS_MOVES=1 cargo test --workspace`
Expected: PASS. The stress run is what proves the table is forwarded: a
missed forward reads a stale handle and panics.

- [ ] **Step 5: Record it and commit**

- `docs/design/object-model.md`, "Values and objects": the symbol paragraph
  becomes present tense — built — keeping the prior art and the rejected
  immediate.
- `docs/design/memory.md`, "What the collector traces": the planned bullet
  becomes an ordinary list item naming the table, still noting that the
  collector makes it weak.
- `docs/design/README.md`: drop "equal symbols are not one object" from the
  closing paragraph's list of rules not implemented yet, and from the
  paragraph above the table ("Two more behave that way …"), whose count
  follows.

```bash
cargo fmt --all
git add crates/ns-runtime goldens/spec docs/design
git commit -m "fix: make equal symbols one object per heap"
```

---

### Task 6: A decoded symbol is the canonical one, and occurs once

**Files:**
- Modify: `crates/ns-runtime/src/image/decode.rs` — `DecodeError`
  (`:25-…`) and its `Display` (`:320-430`), `decode_graph_section`
  (`:2256-2305`)
- Test: `crates/ns-runtime/tests/image.rs`
- Modify: `docs/design/image-format.md` ("Verification")

**Interfaces:**
- Consumes: `Heap::canonical_symbol`, `Heap::record_canonical_symbol` (Task 5).
- Produces: `DecodeError::DuplicateSymbol(String)`, displayed as
  `symbol #<text> is recorded twice`.

- [ ] **Step 1: Write the failing tests**

In `tests/image.rs`, near the graph-section tests, using `GraphFixture` with
`fixture.symbols = table_with(&["foo"])` and the `graph_counts` helper:

```rust
#[test]
fn a_symbol_recorded_twice_is_refused() {
    // A canonical heap never encodes two records of one symbol, so a graph
    // that holds two is crafted or corrupt; merging them would mean
    // rewriting every reference to the second.
    let mut writer = encode::Writer::new();
    graph_counts(&mut writer, 2, 0);
    for identity_hash in [1, 2] {
        writer.write_u8(CLASS_BUILTIN);
        writer.write_u8(encode::builtin_class_tag(BuiltinClass::Symbol));
        writer.write_u32(identity_hash);
        writer.write_u8(OBJECT_SYMBOL);
        writer.write_u32(0); // symbol-table id 0: "foo"
    }
    writer.write_u32(0); // no roots
    let mut fixture = GraphFixture::new();
    fixture.symbols = table_with(&["foo"]);
    let error = fixture.decode_error(&writer.into_bytes());
    assert_eq!(error, decode::DecodeError::DuplicateSymbol("foo".to_string()));
    assert_eq!(error.to_string(), "symbol #foo is recorded twice");
}
```

(Match the byte layout to what `decode_graph_section` reads after the
records — the roots' encoding and anything between — by reading it; import
`OBJECT_SYMBOL` the way the file imports the other object tags.) And a
round-trip test: snapshot a module whose run stored a symbol in a slot, load
the image, and assert inside an entry of the loaded interpreter that the
stored value is `same` as `mutator.new_symbol(Symbol::intern("…"))`. Build it
from the file's existing snapshot-then-load tests
(`a_real_snapshots_graph_verifies_and_its_closures_still_read_their_captured_locals`
shows the shape).

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p ns-runtime --test image symbol`
Expected: FAIL — the duplicate decodes, and the loaded symbol is a different
object from the literal's.

- [ ] **Step 3: Enter each decoded symbol, refusing a second**

Add `DuplicateSymbol(String)` to `DecodeError` with its `Display` arm
(`"symbol #{text} is recorded twice"`), next to `DuplicateTagNamedHash`. In
`decode_graph_section`'s record loop, after `replace_object`:

```rust
        // A heap holds one object per symbol, and so does an image of one:
        // a second record of the same text is refused, not merged
        // (`docs/design/image-format.md`, "Verification"). `load` decodes
        // into a fresh interpreter, where nothing made a symbol before the
        // graph section, so an entry already here came from this graph.
        if let Some(symbol) = decoded_symbol {
            if context.mutator.heap.canonical_symbol(symbol).is_some() {
                return Err(DecodeError::DuplicateSymbol(symbol.as_str().to_string()));
            }
            context.mutator.heap.record_canonical_symbol(symbol, *object);
        }
```

with `decoded_symbol` taken from the `OBJECT_SYMBOL` arm before `data` moves
into the record (`let decoded_symbol = match &data { ObjectData::Symbol(symbol) => Some(*symbol), _ => None };`).

- [ ] **Step 4: Run everything**

Run: `cargo test --workspace`, then `NS_NEXT_STRESS_MOVES=1 cargo test --workspace`
Expected: PASS, every golden's snapshot and build/load round trip included.

- [ ] **Step 5: Record it and commit**

`docs/design/image-format.md`, "Verification": the planned paragraph becomes
present tense, naming `DuplicateSymbol` and the two tests.

```bash
cargo fmt --all
git add crates/ns-runtime docs/design/image-format.md
git commit -m "fix: enter decoded symbols into the heap's table and refuse a repeat"
```

---

### Task 7: Measure the whole step, and present the numbers

**Files:**
- Create: nothing in the tree. Raw output goes to the session's scratchpad.

**Interfaces:**
- Consumes: the branch head after Task 6, `22a56f7`, `be3eb40`.
- Produces: the numbers Task 8 records.

- [ ] **Step 1: Write out the four sources that are not in the tree**

`bench_do.ns` from the
[2c plan](2026-09-13-2348-ns-next-lifetime-brand-2c.md), "Measurement
protocol"; `bench_new.ns`, `bench_new2.ns` and `bench_newarg.ns` from
`docs/design/execution.md`, "What spec-conformant lookup moved" — **copied
from the text the documents give, never reconstructed from their prose**
(`docs/PROGRESS.md` says why). The other four are
`goldens/bench/bench_loop.ns`, `bench_send.ns`, `bench_block.ns` and
`bench_megamorphic.ns`. Put the four written-out files in the scratchpad.
Before timing, check whether any of the eight declares a local slot with an
initializer or writes a parameter: Task 2 and Task 3 change what such a
program does, which would make its numbers incomparable — report it if so.

- [ ] **Step 2: Build the three binaries**

In throwaway worktrees for `22a56f7` and `be3eb40`, and on the branch head,
each built the same way, in PowerShell:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"
cargo build --workspace --release
Remove-Item Env:RUSTFLAGS
```

Copy each `target/release/ns-next.exe` out under a name that says which it
is. Check that none imports `VCRUNTIME140.dll`, and that every binary prints
the expected answer for every source before timing anything.

- [ ] **Step 3: Check that the step is still inline**

Symbol literals and the operand copies change code on the run loop's path.
On a release build with debug information of `22a56f7` and of the branch
head, run `dumpbin /disasm` (Visual Studio Build Tools, not on `PATH`) and
confirm that no `vm::run::closure` is a function of its own; record
`vm::run`'s instruction count in each (`docs/design/execution.md`, "What the
inline caches moved"). If the step split, report that before timing — the
numbers would measure the split.

- [ ] **Step 4: Time**

`ns-next bench <source> --iterations 30`, five rounds per sitting, the order
of the three binaries rotating from round to round; two whole sittings,
nothing else running. Record every number, not a summary.

- [ ] **Step 5: Present the numbers**

Report to the user, in tables: each source's means per sitting for the three
binaries, the branch head's change against `22a56f7` with the protocol's gate
(overlapping, entirely above, entirely below), both against `be3eb40`, and
`vm::run`'s instruction counts. A source entirely above `22a56f7` in both
sittings is a regression to explain before Task 8, not a number to record and
move past.

---

### Task 8: Record what was built and measured

**Files:**
- Modify: `docs/design/execution.md` (a new "What the specification gaps
  moved", after "What 3c moved"), `docs/design/README.md` (the map's status
  lines, if any names these gaps), `docs/PROGRESS.md`, `CLAUDE.md`

- [ ] **Step 1: Write the measurement up**

`execution.md`, "What the specification gaps moved", in the shape of "What 3c
moved": what changed on a send's path (the operand copies and symbol
literals; the resolver's changes are ahead of the run loop), the protocol as
run, the per-round table against `22a56f7`, the means table with `be3eb40`,
and the `dumpbin` result.

- [ ] **Step 2: Update the progress tracker**

`docs/PROGRESS.md`: a milestone section "M4 — The specification gaps
(2026-09-19)" in the shape of the earlier ones — the plan's link, the
decisions (the four gaps and the copy, each as decided with the user on
2026-09-19: the symbol table over an immediate, refusing a duplicate symbol
record, the canonical compiler's reading of a setter send over SOMns's, the
local-slot declaration taken into this step, the copy elision in this step),
what changed commit by commit, the numbers' summary, the test count, and the
gates' results. Rewrite "Where the next session starts": the gaps are done;
the next piece of work is item 4 of the order of work, closure conversion,
named with its design document; the remaining skipped goldens
(`grep -l '^skipped' goldens/*/*.toml`) are listed as what is still open;
and "Known gaps" and "Older than this step" drop what this step closed.

- [ ] **Step 3: Check the documents let a fresh session start**

Read `CLAUDE.md`, `docs/design/README.md` and `docs/PROGRESS.md`'s "Where the
next session starts" as a session with no context would: each claim about
member sets, local slots, setter sends, operand copies and symbols matches
the code; nothing still says "planned" about what is built; no document
narrates how it got here (`git log` holds that).

- [ ] **Step 4: Run every gate and commit**

All of the Global Constraints' gates, then:

```bash
cargo fmt --all
git add docs CLAUDE.md
git commit -m "docs: record the specification gaps as built and measured"
```
