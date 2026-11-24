# `class` and the class accessors protected — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `Object>>class` and every class accessor `Object` declares become `protected`, so an instance's authority is separated from its class's, with every test keeping what it checks.

**Architecture:** First every sender of `class` to another object -- three in the core library, about 44 goldens and 42 Rust tests -- is rewritten so that it passes both before and after the flip; then one commit flips the access, drops the image's value clause for `NonBooleanReceiver`, and adds the goldens that pin the rule. What a program can observe after the flip, a golden checks through the public protocol; what only the representation shows moves into a Rust test that asks the host for the class.

**Tech Stack:** Rust 1.98.0, Newspeak core library (`crates/ns-runtime/core-library/`), goldens under `goldens/`, `cargo-nextest`.

**Spec:** `docs/design/core-library.md`, "Planned: `class` and the class accessors protected" (decided with the user on 2026-10-13). Read it before Task 1.

## Global Constraints

- Every commit is green: the per-commit gates in `CLAUDE.md` ("Working in this repository") run before each commit. The end-of-batch gates (five stress modes, `run-corpus` plain and under stress, the benchmarks profile) run once, after Task 6, by the controller; an implementer does not run them.
- Commits are conventional (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`), each ending with a `Co-Authored-By:` line naming the implementing model and `Claude-Session: https://claude.ai/code/session_01BcChjJmpsEYtz6GZ8wwPBo`. Never change the git identity.
- Work on the checked-out branch `feature/protected-class` in `D:\Workspace\ns-next`, no worktree.
- Names are written out (`CLAUDE.md`, "Names are written out"): `mutator`, `interpreter`, `error`, never `m`, `interp`, `e` -- in Rust. In Newspeak goldens a block parameter `e` that already exists may stay; new code writes `error`.
- **Program outcomes are pinned, not re-derived** (`CLAUDE.md`). A golden's `.toml` changes only by the lines this plan names, each either kept byte for byte, translated as the catalogue below says, or removed because its check moved to a Rust test named in the same task. Never accept an output because the code now produces it.
- After editing a `.ns` file: `cargo run -p ns-analyzer -- fmt <the files edited>`; never at the repository root.
- Every member of a shipped module keeps its `///` comment (`crates/ns-runtime/tests/core_library_documentation.rs`); a changed member's comment says what changed (the access) and why, per `CLAUDE.md`'s documentation standard, naming no other implementation.
- Any change to `Kernel.ns`, `PlatformFactory.ns` or `CollectionsFactory.ns` changes the shipped world's fingerprint, so `crates/ns-runtime/tests/fixtures/golden-image.bin` must be regenerated in the same commit: `cargo nextest run -p ns-runtime --test image --run-ignored only regenerate_the_golden_fixture`, then confirm with `cmp -l` against the previous fixture (`git show HEAD:crates/ns-runtime/tests/fixtures/golden-image.bin > $TEMP/old-fixture.bin`) that only the 8 fingerprint bytes after the flag byte differ.
- Never `{:?}` a `Value`, never `.unwrap()` / `.expect()` an `EvaluationResult`; tests use the local `expect_ok` helper of their file (`CLAUDE.md`).
- A Newspeak module inline in a Rust test is an `indoc! {"…"}` with the closing `"}` on its own line.
- After the flip, the host's `Interpreter::send` is an explicit-receiver send and can no longer send `class` either: a Rust test reads a value's class with `mutator.class_of(value)` inside `interpreter.enter`, compares it with `mutator.builtin_class(BuiltinClass::…)` by `.same(…)`, and names it with `mutator.heap().describe(Value::object(class))` (or the class's own name accessor on `Heap` if `describe` answers more than the name).

## Review Focus

1. **A bare class name inside a block, a nested class or a class-side method still answers the kernel class after the flip** -- an implicit-receiver send reaches a protected member (§5.9's lookupProtected); pinned by Task 5's `a_bare_class_name_still_answers_its_class_everywhere.ns`.
2. **The accessor fast path survives protected access** -- `compiler::primitive_method::recognize_builtin_class_accessor` does not read the access modifier, so `Integer` still runs no activation; Task 5 pins it by resolving through `resolve_lexical` in `act_answers_a_class_accessor_without_an_activation`.
3. **`other class` from outside reaches `doesNotUnderstand:` and is catchable** -- §5.5's lookupPublic stops at the protected declaration ("Undefined"); pinned by Task 5's `an_outside_class_send_is_not_understood.ns`.
4. **A module named like one of `Object`'s accessors, e.g. `Array`, is now reachable through the manifest** -- `program.rs`'s refusal asks `responds_publicly`, which no longer finds the protected accessor, so the refusal stops applying to those names and still applies to every public member; pinned by Task 5's rewritten `a_module_shadowed_by_an_object_member_is_refused` and the new `a_module_named_like_a_class_accessor_is_reached_through_the_manifest`.
5. **A caught `NonBooleanReceiver` still round-trips through `build`/`load` as a record** after its value clause is gone, and a crafted image naming its class as a value is refused; pinned by Task 5.

## The rewrite catalogue

Every send of `class` (or of one of `Object`'s class accessors) to another object is rewritten by exactly one of these rules. `self class`, `super class` and a bare name are untouched.

| Rule | Before | After | Output line |
|---|---|---|---|
| **K** kind of a result | `x class printString`, `x class name`, `print: x class`, `(x class == Integer) printString` where the kind is one a predicate answers | `x isKindOfInteger printString` (likewise `isKindOfFloat`, `isKindOfFraction`, `isKindOfNumber`, `isKindOfString`, `isKindOfArray`, `isKindOfClosure`, `isKindOfList`, `isKindOfMap`, `isKindOfIdentityMap`, `isKindOfSet`, `isKindOfCollection`, `isKindOfBoolean`; `isNil` for `UndefinedObject`) | `true` (or `false`) in place of the class name |
| **E** class of a caught error, nameable | `[…] on: Error do: [:e | e class name]` where the error's class is public | `[…] on: <the class> do: [:error | '<Name>']`, the class reached as the golden already reaches kernel classes (`platform kernel MessageNotUnderstood`, `kernel ArgumentError`, a bare `Error`) | unchanged, byte for byte |
| **D** class of a caught error, private | the same, for `NonBooleanReceiver` or another class no program names | `[…] on: Error do: [:error | error description]` | the description (`NonBooleanReceiver: 4` or the error's `messageText`); state it in the commit message |
| **M** one level of metaclass | `Foo class` as a value, `Foo class new`, `Foo class == Bar class` | a class-side method on `Foo` (and on `Bar`) `public metaclass = ( ^self class )`, then `Foo metaclass`, `Foo metaclass new`, `Foo metaclass == Bar metaclass` | unchanged |
| **F** a factory reached through the class | `platform class new`, `t class new` | the class's public accessor: `platform Platform new`, `platform Transcript new` | unchanged |
| **R** representation only | the `Integer`/`LargeInteger` boundary, chains (`x class class`), `Metaclass` refusing `new`, identity of class objects (`[] class == Closure`, `x class == y class`) where no predicate says the same | the line leaves the golden, and the same check is a Rust test in `crates/ns-runtime/tests/representation.rs` (created in Task 2), named after the golden and the property | removed from the `.toml` |

When K, E, M or F would lose what the line checks -- for instance a `class == class` comparison of two collections that a predicate cannot tell apart -- use R instead.

---

### Task 1: The core library sends no `class` to another object

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Value`'s `valueEquals:`, around line 1459)
- Modify: `crates/ns-runtime/core-library/CollectionsFactory.ns` (`List`'s two index checks, around lines 194 and 209)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)

**Interfaces:**
- Consumes: the mirror leaf `vmMirror objectClass: anObject ifFail: aBlock` (already used by `Object>>class`), `Integer>>isKindOfInteger`.
- Produces: no new selector.

- [ ] **Step 1: Rewrite `valueEquals:`'s class comparison**

```newspeak
      (self class == (vmMirror objectClass: other ifFail: [^false])) ifFalse: [^false].
```

  and extend its `///` comment by one sentence: it reads `other`'s class through the mirror, since `class` is answered only to its receiver's own sends.

- [ ] **Step 2: Rewrite both index checks in `CollectionsFactory.ns`**

```newspeak
      (index isKindOfInteger and: [(1 <= index) and: [index <= size_]]) ifFalse: [
```

- [ ] **Step 3: Format, regenerate the fixture, run the gates**

Run: `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/Kernel.ns crates/ns-runtime/core-library/CollectionsFactory.ns`, the fixture regeneration of the Global Constraints, then the per-commit gates.
Expected: every test passes; `cmp -l` shows only fingerprint bytes.

- [ ] **Step 4: Commit** — `refactor: send class to no other object in the core library`

### Task 2: The language and number goldens, and the representation tests

**Files:**
- Create: `crates/ns-runtime/tests/representation.rs`
- Modify (each `.ns` with its `.toml`), with the rule per line:

| Golden | Lines and their rules |
|---|---|
| `goldens/language/class/class-side/a_class_and_its_metaclass_print_their_names` | 15 M; 18, 21, 24, 27 R |
| `goldens/language/class/class-side/a_class_object_cannot_be_made_by_instantiation` | 16 R (the whole golden pins `Metaclass` refusing `new`: delete the golden and its `.toml`, the Rust test replaces it) |
| `goldens/language/class/class-side/a_metaclass_does_not_understand_new` | 16 M |
| `goldens/language/class/class-side/the_class_of_a_class_is_its_metaclass` | 15, 18, 30 M; 21, 24, 27 R |
| `goldens/language/class/nesting/spec_2_3_nested_class_overridden_by_slot` | 29 K (`isKindOfString`) |
| `goldens/library/builtin-subclass/a_subclass_of_large_integer_is_no_number` | 34 R |
| `goldens/library/closure/a_closure_is_an_instance_of_closure` | 10, 13 K (`isKindOfClosure`); the identity `== Closure` is R |
| `goldens/library/number/float/a_float_beyond_the_immediate_range_behaves_as_any_float` | 12 K (`isKindOfFloat`); 30 R |
| `goldens/library/number/float/float_sine_cosine_and_truncation` | 45, 52 R (the `LargeInteger` boundary) |
| `goldens/library/number/fraction/a_fraction_is_made_in_normal_form` | 21, 31 K (`isKindOfFraction` / `isKindOfInteger`); 38 R |
| `goldens/library/number/integer/integer_protocol` | 94 R |
| `goldens/library/number/large-integer/an_overflowing_integer_operation_answers_a_large_integer` | 38, 41, 80 R |
| `goldens/library/number/large-integer/large_integer_arithmetic` | 88, 91, 100 R |
| `goldens/library/number/large-integer/large_integer_literals` | 20–41 R |
| `goldens/library/number/tower/arithmetic_is_exact_without_a_float` | 24, 28, 41 K (`isKindOfInteger`, `isKindOfFraction`) |

The line numbers are those of commit `062f0ee0`; find each by its text if a line has moved.

**Interfaces:**
- Consumes: `Interpreter::bootstrap`, `Interpreter::enter`, `Mutator::class_of`, `Mutator::builtin_class`, `Heap::describe`, the `load`/`expect_ok` helpers as `crates/ns-runtime/tests/metaclasses.rs` writes them.
- Produces: `crates/ns-runtime/tests/representation.rs` with a helper `class_name_of(source: &str, selector: &str) -> String` that loads `source`, instantiates its module with the platform, sends the unary `selector`, and answers the name of the answer's class read on the host side; Task 3 and Task 4 add tests to this file.

- [ ] **Step 1: Write `representation.rs` with the helper and one test per R line above**

```rust
//! What only the representation shows -- an integer's `LargeInteger`
//! class, a chain of metaclasses, the identity of class objects -- read on
//! the host side, where no capability rule binds: a program cannot ask
//! another object for its class (`docs/design/core-library.md`, "`class`
//! and the class accessors protected").

/// The name of the class of what `selector` answers, sent to a fresh
/// instance of `source`'s module.
fn class_name_of(source: &str, selector: &str) -> String {
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.instantiate(&class, std::slice::from_ref(&platform)),
        "instantiate",
    );
    let answer = expect_ok(
        interpreter.send(&instance, Symbol::intern(selector), &[]),
        selector,
    );
    interpreter.enter(|mutator| {
        let value = mutator.fetch(&answer);
        let class = mutator.class_of(value);
        mutator.heap().describe(Value::object(class))
    })
}

/// `goldens/library/number/large-integer/large_integer_literals.ns`: a
/// literal past 2^63 - 1 or below -2^63 is a `LargeInteger`, one inside
/// the immediate range an `Integer`.
#[test]
fn an_integer_literal_is_large_exactly_past_the_immediate_range() {
    let cases = [
        ("9223372036854775808", "LargeInteger"),
        ("9223372036854775807", "LargeInteger"),
        // …one row per line 20–41 of the golden, with the class name its
        // `.toml` printed for that line.
    ];
    for (literal, expected) in cases {
        let source = formatdoc! {"
            class Probe usingPlatform: platform = () (
              public answer = ( ^{literal} )
            )
        "};
        assert_eq!(class_name_of(&source, "answer"), expected, "{literal}");
    }
}
```

  Copy the `load` and `expect_ok` helpers from `metaclasses.rs` verbatim; take `fetch`, `platform` and the other calls from what that file and `platform.rs` already call, and if `describe` answers more than the bare class name, use the name accessor on `Heap` that `describe` itself calls. Every expected name is copied from the golden's current `.toml`, never derived from a run. The comment inside `cases` above is an instruction to this step; the committed test lists every row and has no such comment. Do the same, one `#[test]` per golden, for each other R line of the table: the metaclass chains compare `mutator.class_of` of a class object with `mutator.builtin_class(BuiltinClass::Metaclass)`; `Metaclass` refusing `new` sends `new` to `mutator.builtin_class(BuiltinClass::Metaclass)` stashed as a `Root` and asserts the error message the deleted golden's `.toml` pinned.

- [ ] **Step 2: Run the new tests**

Run: `cargo nextest run -p ns-runtime --test representation`
Expected: PASS -- they check today's behaviour, which the goldens pin.

- [ ] **Step 3: Rewrite the goldens of the table, line by line, by their rule; add each M class-side `metaclass` method with a `///` comment**

- [ ] **Step 4: Update each `.toml`'s `stdout` by exactly the catalogue's output column, then check it**

Run: `cargo nextest run -p ns-cli --test run_goldens`
Expected: PASS. Then `git diff -U0 -- 'goldens/**/*.toml'` must show only the lines of the table: removed for R, `true`/`false` in place of a class name for K, nothing for M.

- [ ] **Step 5: Format and run the gates; commit** — `test: check the language and number goldens without an outside class send`

### Task 3: The library goldens

**Files:**
- Modify (each `.ns` with its `.toml`):

| Golden | Lines and their rules |
|---|---|
| `goldens/language/control-flow/a_non_boolean_receiver_is_caught_as_an_error` | 10 D |
| `goldens/language/slot/simultaneous/spec_6_3_2_an_abandoned_clause_recomputes_on_the_next_touch` | 28 E if the error's class is public, else D |
| `goldens/library/collection/a_list_changed_while_iterated_signals` | 50 E (`ConcurrentModification`) |
| `goldens/library/collection/a_list_has_the_whole_protocol` | 56, 63 K (`isKindOfList`); 67 K (`isKindOfArray`) |
| `goldens/library/collection/a_list_index_error_is_caught_as_an_error` | 29 E |
| `goldens/library/collection/a_map_has_the_whole_protocol` | 91 K (`isKindOfMap`) |
| `goldens/library/collection/a_set_has_the_whole_protocol` | 51 K (`isKindOfSet`); 57 K (`isKindOfArray`) |
| `goldens/library/collection/an_array_copies_searches_and_sorts` | 33 K (`isKindOfArray`) |
| `goldens/library/collection/an_array_index_error_is_caught` | 15 E |
| `goldens/library/collection/an_identity_map_compares_keys_by_identity` | 77 K (`isKindOfIdentityMap`) |
| `goldens/library/collection/every_platform_shares_one_collections_module` | 11 F; 22 R |
| `goldens/library/exception/a_cannot_return_carries_the_returned_value` | 23 E |
| `goldens/library/exception/a_does_not_understand_is_caught_as_an_error` | 12 E |
| `goldens/library/exception/a_leafs_argument_error_is_caught_by_its_class` | 24 E |
| `goldens/library/object/a_future_is_made_only_by_computing` | 31 E |
| `goldens/library/object/a_proxy_forwards_every_message` | 43 R; 49 E |
| `goldens/library/object/exception_and_error_are_named_bare` | 29 E |
| `goldens/library/object/is_kind_of_answers_what_a_receiver_declares` | 82 E |
| `goldens/library/object/nil_is_the_undefined_object` | 7 K (`isNil`) |
| `goldens/library/platform/a_second_platform_writes_like_the_first` | 13 F; 27, 30, 33 R |
| `goldens/library/platform/the_platform_and_its_parts_are_values` | 20, 23, 26 F; 29, 31, 34 R |
| `goldens/library/string/a_string_index_error_is_caught` | 60 E |
| `goldens/library/string/a_string_searches_and_iterates_its_code_points` | 85 K (`isKindOfString`) |
| `goldens/library/string/a_symbol_is_a_string` | 24, 33 R (the `String`/`Symbol` distinction) unless `isKindOfSymbol` exists, then K |
| `goldens/library/string/double_quoted_string_literals` | 19 K (`isKindOfString`) |
| `goldens/library/string/string_new_answers_an_empty_string` | 14 K (`isKindOfString`) |
| `goldens/library/value/a_value_modules_nested_class_is_a_value` | 66 M (a class-side `metaclass` on `Point`) |
| `goldens/library/value/a_values_slot_names_a_refused_metaclass` | 34 M (a class-side `metaclass` on `Plain`) |
| `goldens/runtime/depth-limit/the_depth_limit_is_caught` | 19 E |

**Interfaces:**
- Consumes: `representation.rs` and `class_name_of` from Task 2.
- Produces: nothing new.

- [ ] **Step 1: Add the R lines' tests to `representation.rs`**, one `#[test]` per golden as in Task 2, each expected value copied from the golden's `.toml`. Run `cargo nextest run -p ns-runtime --test representation`; expected PASS.
- [ ] **Step 2: Rewrite each golden by its rule.** For E, the handler answers the class's name as a string literal, so the output is unchanged; if the golden cannot reach the class (no `platform` in scope), use D and say so in the commit message.
- [ ] **Step 3: Update the `.toml`s by the catalogue's output column; run `cargo nextest run -p ns-cli --test run_goldens`; check `git diff -U0 -- 'goldens/**/*.toml'` as in Task 2.**
- [ ] **Step 4: Format and run the gates; commit** — `test: check the library goldens without an outside class send`

### Task 4: The Rust tests

**Files:**
- Modify: the tests below, in their files under `crates/ns-runtime/src/`, `crates/ns-runtime/tests/` and `crates/ns-cli/tests/`; find each by name.

`ns-cli`: `build_load::a_hash_given_to_a_tag_named_object_survives_build_and_load`.
`ns-runtime` unit tests: `builtins::value::tests::{a_class_object_that_is_no_value_compares_by_identity, a_class_object_under_a_value_with_its_own_equality_is_compared_in_newspeak, a_leaf_hashed_value_and_a_fallback_hashed_value_are_never_equal, equal_class_objects_hash_alike, two_class_objects_are_decided_before_the_walk}`, `interpreter::kernel::tests::{the_platform_modules_members_have_the_fast_path_they_should, the_platforms_class_accessors_are_recognized_without_a_fast_path}`.
`ns-runtime` integration tests: `array_leaves::copy_from_to_answers_a_new_array_of_the_range`, `builtin_object_inheritance::a_builtin_receiver_inherits_objects_native_class`, `collector::every_kind_of_stored_word_survives_moves_and_collections`, `future::a_future_answers_objects_own_messages_as_its_value`, `image::{a_boxed_float_value_round_trips_without_a_record, a_class_the_platform_hands_out_is_named_by_its_declaration, a_crafted_small_integer_literal_beyond_61_bits_loads_as_a_large_integer, a_fraction_value_round_trips, a_large_integer_in_i64_travels_as_an_i64_and_loads_as_a_large_integer, a_large_integer_value_round_trips, a_list_is_a_record_of_its_shipped_class, a_module_instance_outside_a_frame_is_refused, a_second_collections_facet_is_a_record_of_its_shipped_class, a_second_platform_is_a_record_of_its_shipped_class, a_shipped_name_resolves_to_the_loading_boots_own_object, a_stopwatch_is_a_record_of_its_shipped_class}`, `metaclasses::the_class_of_a_class_is_its_metaclass`, `mixin::super_from_the_applying_class_reaches_the_applied_level`, `object_class_accessors::{a_bare_builtin_name_and_the_kernel_answer_alike_under_both_lowerings, an_explicit_self_send_of_a_class_name_answers_the_builtin_class}`, `platform::{a_subclass_of_the_platform_class_writes_too, platform_collections_answers_the_collections_modules_list, the_collections_module_is_built_once_at_boot, the_platform_class_takes_no_mirror, the_platform_is_an_instance_of_the_shipped_platform_module, platform_class_accessors_return_the_matching_builtin_class}`.

`image::a_caught_non_boolean_receivers_class_round_trips_through_a_package` and `image::the_wire_format_matches_the_golden_fixture` are Task 5's; `manifest::an_object_member_answers_before_the_manifest_can`, `run::a_module_shadowed_by_an_object_member_is_refused` and `interpreter::resolve::tests::act_answers_a_class_accessor_without_an_activation` too.

**Interfaces:**
- Consumes: `Mutator::class_of`, `Mutator::builtin_class`, `Heap::describe` (Global Constraints).
- Produces: nothing new.

- [ ] **Step 1: Rewrite each test's inline Newspeak so it sends no `class` and no class accessor to another object.** Where the Newspeak answered a class to the Rust side only to compare it (`… answer = ( ^x class )`), answer `x` itself and read its class on the host side with `mutator.class_of`. Where the Newspeak needs a class it can name, use the bare name or the platform's accessor (`platform Platform`, `platform collections List`) as the catalogue's F rule. Where it uses `x class new`, use F. `platform_class_accessors_return_the_matching_builtin_class` sends `platform Object` and its siblings, which are `Object`'s accessors: rewrite it to ask each accessor through a method of the module whose body is the bare name (`public objectClass = ( ^Object )`), so the same builtin classes are compared. Each test keeps its assertions; a test's expected values never change.
- [ ] **Step 2: Run the gates; commit** — `test: read classes on the host side in the Rust tests`

### Task 5: The flip

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Object>>class`, line 52; `Object`'s accessors `Object` … `Error`, lines 131–214)
- Modify: `crates/ns-runtime/src/interpreter/shipped_world.rs` (the `signalled` clause, around lines 296–308)
- Modify: `crates/ns-runtime/src/types.rs` (`ErrorClass::is_signalled`, line 606: delete it if nothing else calls it)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs` (`act_answers_a_class_accessor_without_an_activation`)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs` (`objects_class_accessors_are_exactly_the_nameable_builtins`)
- Modify: `crates/ns-runtime/tests/manifest.rs` (`an_object_member_answers_before_the_manifest_can`)
- Modify: `crates/ns-cli/tests/run.rs` (`a_module_shadowed_by_an_object_member_is_refused`, and a new test)
- Modify: `crates/ns-cli/src/program.rs` (the refusal's message only, line 245: "is a public member of every object")
- Modify: `crates/ns-runtime/tests/image.rs` (`a_caught_non_boolean_receivers_class_round_trips_through_a_package`, and a new test)
- Modify: `crates/ns-runtime/tests/fixtures/golden-image.bin` (regenerated)
- Create: `goldens/language/send/protected/an_outside_class_send_is_not_understood.ns` and `.toml`
- Create: `goldens/language/send/protected/an_outside_class_accessor_send_is_not_understood.ns` and `.toml`
- Create: `goldens/language/send/protected/a_bare_class_name_still_answers_its_class_everywhere.ns` and `.toml`
- Create: `goldens/language/class/class-side/self_class_on_the_class_side_is_the_metaclass.ns` and `.toml`

(Put the new goldens in an existing directory instead if one under `goldens/language/send/` already holds access-modifier goldens; check with `ls goldens/language/send`.)

**Interfaces:**
- Consumes: Tasks 1–4 (nothing sends `class` to another object any more).
- Produces: the protected members; `Positions` without the signalled clause.

- [ ] **Step 1: Write the four new goldens, failing.** Each is a `main: platform args:` module printing through `platform transcript`, like its neighbours. The two "not understood" goldens catch the error, `on: platform kernel MessageNotUnderstood do: [:error | error message selector]`, and print the selector (`class`, `Integer`) -- check `Message`'s selector accessor's name in `Kernel.ns`. The bare-name golden uses a bare `Integer` in an instance method, in a block, in a nested class's method and in a class-side method, each printing `(Integer parse: '7') printString`, so the class is used rather than compared. The class-side golden declares `public metaclass = ( ^self class )` and prints `Foo metaclass printString` and `(Foo metaclass == Foo metaclass) printString`. Write each `.toml` from what the specification says the program must print, never from a run.

Run: `cargo nextest run -p ns-cli --test run_goldens`
Expected: the two "not understood" goldens FAIL (the sends are still understood); the other two PASS already.

- [ ] **Step 2: Flip the access in `Kernel.ns`.** `public class = (` becomes `protected class = (`, and each of the sixteen `public <Name> = (` accessors in `Object` becomes `protected`. Extend `class`'s `///` comment: it is protected so that holding an instance does not hand out its class -- its factory, its class side's state, a subclass in the instance's environment; `self class` and `super class` reach it. Extend the comment above the accessor block likewise: a bare name is an implicit-receiver send and reaches them; `other Integer` does not.

- [ ] **Step 3: Drop the signalled clause.** In `shipped_world.rs` delete the `signalled` binding and its comment; `value` becomes `nested.access == AccessModifier::Public || accessed.contains(&nested.name)`. Delete `ErrorClass::is_signalled` if it has no other caller (`grep -rn is_signalled crates`).

- [ ] **Step 4: Rewrite the image tests.** `a_caught_non_boolean_receivers_class_round_trips_through_a_package` becomes `a_caught_non_boolean_receiver_round_trips_through_a_package`: the program keeps the caught error itself in a slot (no `class`), and the test asserts, after `build` and `load`, that the slot's value is an instance whose class (host side, `mutator.class_of`) is `mutator.builtin_class(BuiltinClass::NonBooleanReceiver)`. Add `non_boolean_receiver_as_a_value_is_refused`, modelled on `a_shipped_name_in_a_forbidden_position_is_refused`: a crafted image naming `(Kernel, NonBooleanReceiver, class)` as a value is refused with `ShippedNameNotAllowed`, and the encoder refuses a graph holding that class object as a value.

- [ ] **Step 5: Rewrite the resolve, kernel, manifest and CLI tests.**
  - `act_answers_a_class_accessor_without_an_activation`: resolve `String` as the implicit-receiver send it is, `mutator.resolve_lexical(receiver, None, selector)`, and keep every assertion; add `assert!(matches!(mutator.resolve_public(receiver, selector), Resolved::DoesNotUnderstand { .. }))` (use the variant's real name) for the outside send.
  - `objects_class_accessors_are_exactly_the_nameable_builtins`: additionally assert `method.access == AccessModifier::Protected` for each accessor.
  - `an_object_member_answers_before_the_manifest_can`: `Array` is no longer answered publicly, `assert!(!interpreter.responds_publicly(&manifest, Symbol::intern("Array")))`; `printString` still is.
  - `a_module_shadowed_by_an_object_member_is_refused`: the module is named after a public member of `Object` that a class may be named, `yourself` (`class yourself = Object new () () : ()`, file `yourself.ns`), and the assertions stay. Add `a_module_named_like_a_class_accessor_is_reached_through_the_manifest`: a project whose module is `Array.ns` (`class Array = Object new () () : ( public greeting = ( ^'from the module' ) )`), whose main prints `(manifest Array) greeting` -- check how a main reaches a module in `crates/ns-cli/tests/run.rs`'s neighbouring project tests -- and runs with status 0.
  - `program.rs`'s message: unchanged in meaning; it now names only public members, which it already says.

- [ ] **Step 6: Regenerate the fixture, format, run every gate.**

Run: the fixture regeneration of the Global Constraints, `cargo run -p ns-analyzer -- fmt crates/ns-runtime/core-library/Kernel.ns goldens`, then the per-commit gates, and `cargo nextest run --workspace --release` (the size tests).
Expected: all PASS, the four new goldens included.

- [ ] **Step 7: Commit** — `feat: make class and Object's class accessors protected`

### Task 6: Documentation

**Files:**
- Modify: `docs/design/core-library.md` ("Planned: `class` and the class accessors protected" becomes the current state, heading without "Planned:"; "The predefined classes are accessors on `Object`" says they are protected; the class diagram's comments if it marks access; a Decisions entry)
- Modify: `docs/design/image-format.md` (the `NonBooleanReceiver` paragraph in "Verification" and the rule table in "Shipped names": the value row loses "or whose instances the VM signals as a VM error (…)", and the paragraph after it no longer names `NonBooleanReceiver`; delete the sentence added on 2026-10-13 that says the clause goes)
- Modify: `docs/design/object-model.md` (wherever it shows `x class` from outside as a program's way to a class)
- Modify: `docs/OPEN-WORK.md` ("Next": remove the protected-`class` paragraph, sub-project 5 is next again; keep the road-step-5 item about the reflective goldens returning, now naming `crates/ns-runtime/tests/representation.rs` as where they are checked meanwhile)
- Modify: `docs/PROGRESS.md` (the milestone's entry and the table)
- Modify: `CLAUDE.md` only if a sentence there is made untrue (search it for `class` accessors and `NonBooleanReceiver`)

- [ ] **Step 1: Make every statement current** -- search `docs/` and `CLAUDE.md` for `is_signalled`, `NonBooleanReceiver`, `error class`, `other class`, `x class`, `public class =` and `class accessor`, and rewrite each one the batch made untrue.
- [ ] **Step 2: Commit** (Markdown only, no gates) — `docs: describe class and the class accessors as protected`
