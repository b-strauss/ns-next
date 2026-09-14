# Spec-Conformant Name Resolution, Lookup and Slot Accessors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make name resolution and message lookup follow `newspeak-spec.pdf`
§5.5–§5.10 and §6.3.2 exactly -- slot accessors as real, overridable members,
§5.7's lexical-before-inherited rule, and the access rules of every send kind --
and hash the method and slot tables with an identity hasher instead of SipHash.

**Architecture:** Twelve deviations were found by probe programs against
`master` (`59061ec`); each becomes a fixture or a pinned program outcome whose
expectation is derived from the specification, written before the fix. The
resolver's member sets become each class declaration's *own* declarations under
their full selectors; `dispatch::walk` decides per access modifier whether to
accept, skip or stop; the lexical private rule asks only the lexically
enclosing class; a slot's getter and setter become entries of its mixin's
method table, answered without an activation. The step ends measured.

**Tech Stack:** Rust 1.98.0, edition 2021, the three workspace crates; no new
dependency.

**Spec:** `docs/references/newspeak-spec.pdf` (v0.106), sections cited per
task; the living design documents this plan updates are
`docs/design/dispatch.md`, `docs/design/object-model.md` and
`docs/design/image-format.md`.

## Global Constraints

- Every expectation is derived from the specification text, cited in the test,
  never from what the code happens to print. Where the specification is
  silent, say so in the test and follow the canonical implementation
  (`prior-art/newspeak/`, `prior-art/primordialsoup/`).
- Code that implements a spec rule cites it: `newspeak-spec.pdf` §N.N in the
  doc comment, the house style already used across `interpreter/` and
  `dispatch.rs`.
- Names are written out (`CLAUDE.md`, "Conventions"); clippy's
  `min_ident_chars` refuses one- and two-character names.
- No dispatch logic in `vm.rs` or `compiler/`: slot accessors are answered in
  `interpreter/`.
- An image is untrusted input: every new field is validated before it indexes
  anything; no panic on any byte sequence.
- No new external dependency: the hasher is written here.
- Every commit passes `cargo fmt --all`, `cargo clippy --workspace
  --all-targets` warning-free and `cargo test --workspace`; the stress suite
  (`NS_NEXT_STRESS_MOVES=1 cargo test --workspace`, about three minutes) and
  both corpus gates run before the last commit of every task.
- A changed golden outcome is a behaviour change: establish from the
  specification that the new answer is right before updating it.
- Commit messages end with the trailer lines the session names.
- Lazy slots (§6.3.3) are out of scope; the parser drops `lazy`.

## The twelve deviations

| # | Rule | Probe | Spec answer | `59061ec` answers | Task |
|---|---|---|---|---|---|
| 1 | §6.3.2 accessors are methods: a subclass method overrides an inherited accessor | `Child` overrides `x`/`x:` of `Base`'s slot | `42`, `42` | `1`, `5` | 6 |
| 2 | §6.3.2 an immutable slot has no setter | `self y: 4` | `doesNotUnderstand: #y:` | writes | 6 |
| 3 | §6.3.2/§5.5 a public slot's accessors answer an explicit receiver | `Holder new z` | `7` | `doesNotUnderstand:` | 6 |
| 4 | §5.9 only N's own private declaration wins | subclass's private `s` vs protected `s` of `Base` | `1` | `2` | 3 |
| 5 | §5.9 a superclass's private member is not lexically reachable | `^p` in a subclass | `doesNotUnderstand: #p` | `10` | 3 |
| 6 | §5.5 `lookupPublic` skips a private declaration | `PrivateShadow new foo` | `1` | `doesNotUnderstand:` | 2 |
| 7 | §5.10 a super send finds only public or protected methods | `super p` on a private `p` | `doesNotUnderstand: #p` | `10` | 2 |
| 8 | §5.10 a super send that finds nothing invokes `doesNotUnderstand:` defined for S | `super foo` with no `foo` above | S's `doesNotUnderstand:` | unbounded recursion | 2 |
| 9 | §5.7/§2.3.1 only lexically *declared* members are visible; inherited ones are not | the dyla paper's case 2 | `91` | `42` | 4 |
| 10 | §5.7 a member is found under its full selector | `foo:bar:` must not capture `foo:` | `foo:` via `self` | `foo:bar:`'s class once #9 is fixed | 4 |
| 11 | §5.7/§6.3.2 an immutable slot declares no member `y:` | implicit `y: 7` past an immutable `y` | the enclosing class's `y:` | the immutable slot | 4 |
| 12 | §5.8 a parenthesized `self` is a self send | `(self) secret` | `5` | `5` (pinned) | 5 |

A private nested class is unreachable from outside (§6.3.1, §6): already
right, pinned in Task 5.

---

### Task 1: Design documents

**Files:**
- Modify: `docs/design/dispatch.md`
- Modify: `docs/design/object-model.md`
- Modify: `docs/design/image-format.md`

- [ ] **Step 1: Rewrite `dispatch.md`, "Resolution", item 2.** Replace the
  sentence "take the smallest depth `k` whose class, including everything it
  inherits, has a member of that name" with §5.7's rule: at each enclosing
  class declaration, innermost first, only the members that declaration
  *declares* count -- its methods under their full selectors, a slot's getter,
  a mutable slot's setter `x:`, its nested classes. The first level that
  declares the selector is N and the send is `outer N m`; if none does, it is
  `self m`, and the receiver's inheritance chain decides (§2.3.1: "only if no
  lexically visible matching method is found, do we proceed up the inheritance
  chain of self"). Quote the dyla paper's rule 4 and say that an inherited
  member of an *enclosing* class is reached only by an explicit `outer N m`.
  Record the open question: both sides of a class declaration contribute to
  one member set today; the specification is silent and the canonical
  compiler scopes the class side apart (`Newspeak2SqueakCompilation.ns`,
  `compileClassSide:`), so that stays as it is until a program needs it.
- [ ] **Step 2: Rewrite "The three lookup algorithms"** with the verdict per
  access modifier: `lookupPublic` accepts public, stops at protected, skips
  private (§5.5); `lookupProtected` accepts public and protected, skips private
  (§5.9); the `doesNotUnderstand:` lookup accepts all (§5.5). Add "The lexical
  private rule": only N's own mixin is asked for a private declaration (§5.9,
  Primordial Soup's `LexicalSend`), then `lookupProtected` on the class of the
  kth enclosing object. Add "Super sends": `lookupProtected` from the
  superclass of the current class, and on failure `doesNotUnderstand:` defined
  for that superclass (§5.10).
- [ ] **Step 3: Replace "Deviation: slots are not reachable through an
  explicit receiver"** with "Slot accessors": a slot declaration defines a
  getter and, if mutable, a setter, entries of its mixin's method table with
  the slot's accessibility (§6.3.2); found by the ordinary lookup of every send
  kind, overridable, answered without an activation (Cog's quick methods,
  SOMns's `CachedSlotRead`); the setter answers the receiver; initialization
  never goes through them (`InitializeSlot`). Delete the paragraph "A lexical
  send to a slot is cached too" from "Planned: inline caches" -- a slot is now
  an ordinary cache entry -- and add two open questions to that section: the
  megamorphic fallback cache behind the inline caches (Cog's method cache, V8's
  stub cache) and a dense application id in the object header for a one-compare
  guard. Record duplicate member names as a resolve error
  (`checkForDuplicateNames` in the canonical compiler).
- [ ] **Step 4: `object-model.md`:** in the slot and layout text, replace
  `slot_index` with the accessor entries (mixin-relative index resolved through
  the defining application's `own_slot_base`, exactly as `InitializeSlot`), and
  record that `MixinSlot` carries mutability and accessibility.
- [ ] **Step 5: `image-format.md`:** the mixin table's slot record is `name`,
  `mutability` (0 immutable, 1 mutable), `access` (the existing access tags);
  accessors are not encoded, the decoder derives them; a row whose members
  collide is refused.
- [ ] **Step 6: Commit** `docs: name resolution and lookup follow the specification`.

---

### Task 2: `lookupPublic` skips private declarations; super sends follow §5.10

**Files:**
- Modify: `crates/ns-runtime/src/dispatch.rs` (`walk`, `lookup_public`, `lookup_protected`, `lookup_defined_for`)
- Modify: `crates/ns-runtime/src/interpreter/sends.rs` (`decide_super_send`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`decide_does_not_understand`)
- Create: `tests-fixtures/spec_5_5_public_lookup_skips_private.ns`, `.expected`
- Create: `tests-fixtures/spec_5_10_super_does_not_understand_is_the_superclasss.ns`, `.expected`
- Modify: `crates/ns-cli/tests/program_outcomes.rs`
- Modify: `crates/ns-cli/tests/run_fixtures.rs` (the floor)

**Interfaces:**
- Produces: `enum Verdict { Accept, Skip, Stop }` private to `dispatch.rs`;
  `walk(heap, class, selector, verdict: impl Fn(AccessModifier) -> Verdict)`;
  `Mutator::decide_does_not_understand_in(receiver, lookup_class, selector, arguments)`.

- [ ] **Step 1: Write the fixture for #6.**

```
Newspeak3
'Test'
class Spec55PublicLookupSkipsPrivate usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §5.5: lookupPublic(n, C) is m if the mixin of C
       defines m with public access, undefined if with protected access, and
       lookupPublic(n, superclass(C)) otherwise -- so a private declaration is
       passed over, not a stop. *)
    class PublicBase = Object new () (
        public foo = ( ^1 )
    )
    class PrivateShadow = PublicBase new () (
        private foo = ( ^2 )
    )
    public main: platform args: args = (
        t show: PrivateShadow new foo printString; newLine.
    )
) : ()
```

Expected output: `1`.

- [ ] **Step 2: Write the fixture for #8.**

```
Newspeak3
'Test'
class Spec510SuperDoesNotUnderstandIsTheSuperclasss usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §5.10: "Otherwise, the #doesNotUnderstand: method
       defined for S is invoked" -- S being the superclass of the current
       class, not the receiver's own class. *)
    class A = Object new () (
        protected doesNotUnderstand: message = ( ^'A handled it' )
    )
    class B = A new () (
        protected doesNotUnderstand: message = ( ^'B handled it' )
        public foo = ( ^super foo )
    )
    public main: platform args: args = (
        t show: B new foo; newLine.
    )
) : ()
```

Expected output: `A handled it`.

- [ ] **Step 3: Pin #7 as a program outcome** in `program_outcomes.rs`:

```rust
/// `newspeak-spec.pdf` §5.10: "If a public or protected method f named s is
/// defined for S, the value of the send is the result of invoking f" -- a
/// private method of the superclass is not one, so `super p` falls to
/// `doesNotUnderstand:`.
#[test]
fn a_super_send_does_not_reach_a_private_method() {
    assert_outcome(
        "super-private",
        indoc! {"
            Newspeak3
            'T'
            class SuperPrivate usingPlatform: platform = Object new (
            ) (
                class SuperBase = Object new () (
                    private p = ( ^10 )
                )
                class SuperChild = SuperBase new () (
                    public run = ( ^super p )
                )
                public main: pl args: a = (
                    SuperChild new run
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #p\n",
    );
}
```

- [ ] **Step 4: Run them to see them fail.** `cargo test -p ns-cli --test run_fixtures --test program_outcomes` -- the two fixtures fail (`doesNotUnderstand: #foo`; recursion too deep) and the outcome fails (prints `10`).
- [ ] **Step 5: Implement the verdicts.** In `dispatch.rs`:

```rust
/// What a lookup does with a declaration of the selector it found at one
/// level, by the declaration's accessibility.
enum Verdict {
    /// The declaration is the answer.
    Accept,
    /// Passed over: the search continues at the superclass.
    Skip,
    /// The search ends with no answer.
    Stop,
}

/// §5.5: public is the answer, protected ends the search, private is passed over.
pub fn lookup_public<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>, selector: Symbol) -> Lookup<'gc> {
    walk(heap, class, selector, |access| match access {
        ast::AccessModifier::Public => Verdict::Accept,
        ast::AccessModifier::Protected => Verdict::Stop,
        ast::AccessModifier::Private => Verdict::Skip,
    })
}
```

`lookup_protected` maps public and protected to `Accept`, private to `Skip`;
`lookup_defined_for` accepts every access. `walk` replaces its `accept` and
`hard_stop_on_mismatch` parameters with `verdict`, for methods and nested
declarations alike.
- [ ] **Step 6: Implement §5.10 in `decide_super_send`:** `lookup_defined_for`
  becomes `lookup_protected`, and `Lookup::NotFound` becomes
  `mutator.decide_super_does_not_understand(self_value, superclass, selector, arguments)`,
  which first gives `try_factory_send` its chance (a class-side `super new`
  still instantiates) and otherwise runs `doesNotUnderstand:` looked up from
  `superclass`. Split `decide_does_not_understand` into a wrapper that passes
  `self.class_of(&receiver)` and `decide_does_not_understand_in(receiver,
  lookup_class, …)`, which does today's work against `lookup_class`.
- [ ] **Step 7: Run the new tests and the whole suite.** Expected: the three pass; nothing else changes.
- [ ] **Step 8: Raise the fixture floor in `run_fixtures.rs`** to the new count, then `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
- [ ] **Step 9: Commit** `fix: public lookup passes private declarations over and super sends follow §5.10`.

---

### Task 3: The lexical private rule asks only N (§5.9)

**Files:**
- Modify: `crates/ns-runtime/src/dispatch.rs` (new `kth_lexical_class`, `private_member`)
- Modify: `crates/ns-runtime/src/interpreter/sends.rs` (`lexical_lookup`, `decide_lexical_send`)
- Create: `tests-fixtures/spec_5_9_private_is_only_the_enclosing_classs.ns`, `.expected`
- Modify: `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:**
- Produces: `dispatch::kth_lexical_class(heap, defining_class: Option<ClassReference>, depth: u16) -> Option<ClassReference>`;
  `dispatch::private_member(heap, lexical_class: ClassReference, selector) -> Lookup` (`NotFound` unless N's own mixin declares `selector` private).

- [ ] **Step 1: Fixture for #4.**

```
Newspeak3
'Test'
class Spec59PrivateIsOnlyTheEnclosingClasss usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §5.9: "If N declares a private method f named s then
       the value of the send is the result of invoking f"; otherwise
       lookupProtected, which passes a private declaration over. In Base's
       run, s names Base's own protected s: Child's private s is neither N's
       nor reachable by lookupProtected. *)
    class Base = Object new () (
        s = ( ^1 )
        public run = ( ^s )
    )
    class Child = Base new () (
        private s = ( ^2 )
    )
    public main: platform args: args = (
        t show: Child new run printString; newLine.
    )
) : ()
```

Expected output: `1`.
- [ ] **Step 2: Pin #5 as a program outcome** (`a_superclasss_private_method_is_not_lexically_visible`): `Base2` declares `private p = ( ^10 )`, `Child2 = Base2 new` declares `public run = ( ^p )`, `main:` sends `Child2 new run`; exit 1, stderr `Uncaught Newspeak error: an object does not understand #p\n`. The doc comment cites §5.7 (p is declared by no lexically enclosing class, so it is `self p`) and §5.9 (N = `Child2` declares no private `p`; `lookupProtected` passes `Base2`'s over).
- [ ] **Step 3: Run to see both fail** (`2`; `10`).
- [ ] **Step 4: Implement.** `kth_lexical_class` steps `class_declaring` `depth` times from `defining_class` (depth 0 answers `defining_class` itself), the same chain `kth_enclosing_object` walks. `private_member` reads `class_application(lexical_class)`'s own mixin only: a private method entry, or a private nested declaration, answered as `Lookup::Method`/`Lookup::NestedClass` with `lexical_class` as the defining class; anything else is `NotFound`. `decide_lexical_send` computes the lexical class next to `enclosing`; `lexical_lookup(heap, class, selector)` becomes: the private member of N if there is one, else the metaclass-then-class `lookup_protected` it does today. Cite §5.9 in both doc comments.
- [ ] **Step 5: Run the suite**, fmt, clippy.
- [ ] **Step 6: Commit** `fix: a lexical send's private rule asks only the enclosing class (§5.9)`.

---

### Task 4: The resolver sees only declared members, under full selectors (§5.7)

**Files:**
- Modify: `crates/ns-runtime/src/resolver/classes.rs` (`own_member_names`, `member_name_of_pattern`, duplicate check)
- Modify: `crates/ns-runtime/src/resolver/mod.rs` (`collect_member_sets`, `member_name_of_message`, `resolve_setter_target`, `resolve_superclass_clause`)
- Modify: `crates/ns-runtime/src/resolver/sends.rs` (doc comments)
- Modify: `crates/ns-runtime/tests/resolver.rs`
- Create: `tests-fixtures/spec_5_7_lexical_before_inherited.ns`, `.expected`
- Create: `tests-fixtures/spec_5_7_full_selectors.ns`, `.expected`
- Create: `tests-fixtures/spec_5_7_immutable_slot_declares_no_setter.ns`, `.expected`
- Modify: `crates/ns-cli/tests/program_outcomes.rs`

- [ ] **Step 1: Fixture for #9** (the dyla paper's case 2, `dyla-final.pdf` §3.2, and §2.3.1), plus the explicit `outer` route to an enclosing class's inherited member:

```
Newspeak3
'Test'
class Spec57LexicalBeforeInherited usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §2.3.1: "lookup proceeds up the lexical scope chain
       ... and only if no lexically visible matching method is found, do we
       proceed up the inheritance chain of self". §5.7: d is the innermost
       lexically enclosing construct in which s is *declared*. *)
    class Sup = Object new () (
        public m = ( ^42 )
    )
    class Outer = Object new () (
        public m = ( ^91 )
        class Inner = Sup new () (
            public foo = ( ^m )
        )
        public test = ( ^Inner new foo )
    )
    class OuterBase = Object new () (
        public n = ( ^'inherited by the enclosing class' )
    )
    class Enclosing = OuterBase new () (
        class Nested = Object new () (
            public viaOuter = ( ^outer Enclosing n )
        )
        public test = ( ^Nested new viaOuter )
    )
    public main: platform args: args = (
        t show: Outer new test printString; newLine.
        t show: Enclosing new test; newLine.
    )
) : ()
```

Expected output: `91`, then `inherited by the enclosing class`.
- [ ] **Step 2: Pin the other half as a program outcome**
  (`an_enclosing_classs_inherited_member_is_not_lexically_visible`): the same
  `OuterBase`/`Enclosing` with `Nested`'s `public implicit = ( ^n )` sent from
  `main:`; exit 1, stderr `Uncaught Newspeak error: an object does not understand #n\n`.
  Doc comment: the dyla paper, rule 4: "If one wishes to refer to an inherited
  method of an enclosing class, an explicit outer send expresses this intent
  unambiguously."
- [ ] **Step 3: Fixture for #10.**

```
Newspeak3
'Test'
class Spec57FullSelectors usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §5.7 looks up "a method, slot or class named s",
       s being the whole selector: foo:bar: declares no member foo:, and
       at:put: declares no member at. Neither send is lexically declared, so
       both are self sends that Base answers. *)
    class Base = Object new () (
        public foo: x = ( ^x + 100 )
        public at = ( ^'unary at of Base' )
    )
    class Outer = Object new () (
        public foo: x bar: y = ( ^0 )
        public at: index put: value = ( ^'at:put: of Outer' )
        class Inner = Base new () (
            public keyword = ( ^foo: 1 )
            public unary = ( ^at )
        )
        public test = ( ^Inner new )
    )
    public main: platform args: args = (
        t show: Outer new test keyword printString; newLine.
        t show: Outer new test unary; newLine.
    )
) : ()
```

Expected output: `101`, then `unary at of Base`.
- [ ] **Step 4: Fixture for #11.**

```
Newspeak3
'Test'
class Spec57ImmutableSlotDeclaresNoSetter usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §6.3.2: an immutable slot "may be accessed exclusively
       by invoking a getter method ... There is no setter method associated
       with the slot". So Inner declares no y:, and §5.7 finds Outer's. *)
    class Outer = Object new (| public log ::= 0. |) (
        public y: value = ( log:: value )
        class Inner = Object new (| y = 3. |) (
            public run = ( y: 7. ^y )
        )
        public test = ( | answer | answer:: Inner new run. ^answer )
    )
    public main: platform args: args = (
        | outer |
        outer:: Outer new.
        t show: outer test printString; newLine.
        t show: outer log printString; newLine.
    )
) : ()
```

Expected output: `3`, then `7`.
- [ ] **Step 5: Resolver unit tests** in `tests/resolver.rs` for duplicate names, each citing the canonical compiler's `checkForDuplicateNames` because the specification does not state the rule: a slot `x` and a method `x` in one class are a resolve error; a mutable slot `x` and a method `x:` are a resolve error; an *immutable* slot `x` and a method `x:` resolve (§6.3.2: "defining a setter will not impact such an initialization"); a nested class `N` and a method `N` are a resolve error. Use `resolve` directly and match the error message `"multiple definitions of"`.
- [ ] **Step 6: Run to see the fixtures and tests fail.**
- [ ] **Step 7: Implement.** `own_member_names` inserts full selectors: each slot's name, `name:` for a mutable slot (`SlotMutability::MutableInitialized | MutableBare`), each method's `instantiate::method_name(&method.pattern)`, each nested class's name; both sides as today. It reports a name inserted twice on one side as `ResolveError { message: format!("class `{}` has multiple definitions of `{}`", …) }` (so it returns the set and the errors). `collect_member_sets` stores the own set only -- no superclass merge. `member_name_of_message` answers the whole selector. `resolve_setter_target` searches for `name:`. `resolve_superclass_clause`'s bare-receiver case: a name no enclosing level declares, and no builtin, is `LexicalSend { depth: 0 }` when there is an enclosing class (§6.1 evaluates the clause "as if it occurred inside an instance method of E" -- an implicit unary send, §5.7) and stays a `ResolveError` at the top level (§3.8 precludes implicit receiver sends there). Update `sends.rs`'s doc comments: `members` is the class's own declared members.
- [ ] **Step 8: Run the whole suite and both corpus gates.** Any fixture or corpus module whose answer moves is checked against §5.7 before anything is updated.
- [ ] **Step 9: Commit** `fix: the resolver sees only declared members under their full selectors (§5.7)`.

---

### Task 5: Pin the rules that already hold

**Files:**
- Create: `tests-fixtures/spec_5_8_parenthesized_self_is_a_self_send.ns`, `.expected`
- Modify: `crates/ns-cli/tests/program_outcomes.rs`

- [ ] **Step 1: Fixture for #12.**

```
Newspeak3
'Test'
class Spec58ParenthesizedSelfIsASelfSend usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §5.8: "A self send consists of the reserved word self
       (5.2), enclosed in zero or more pairs of parentheses, followed by a
       message clause" -- so (self) reaches a protected method. *)
    class Holder = Object new () (
        protected secret = ( ^5 )
        public viaParenthesizedSelf = ( ^(self) secret )
    )
    public main: platform args: args = (
        t show: Holder new viaParenthesizedSelf printString; newLine.
    )
) : ()
```

Expected output: `5`.
- [ ] **Step 2: Program outcome** `a_private_nested_class_is_unreachable_from_outside`: `Holder` declares `private class Hidden = Object new () ()`; `main:` sends `Holder new Hidden`; exit 1, stderr `Uncaught Newspeak error: an object does not understand #Hidden\n`; cite §6 ("implicitly defines a method with the same name and accessibility") and §5.5.
- [ ] **Step 3: Run; both pass.** Commit `test: pin parenthesized self sends and private nested classes`.

---

### Task 6: Slot accessors are entries of the method table (§6.3.2)

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs` (`MethodEntry`, `SlotAccessor`, `AccessorKind`, `MixinSlot`, `slot_accessors`; `Mixin.methods`; `MixinApplication.slot_index` removed)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (`build_methods`, `build_slots`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`apply_mixin`, `decide_send`, `decide_does_not_understand_in`)
- Create: `crates/ns-runtime/src/interpreter/slots.rs` (`Mutator::decide_slot_accessor`)
- Modify: `crates/ns-runtime/src/dispatch.rs` (`Lookup::SlotAccessor`, `walk`, `private_member`; `lookup_slot` removed)
- Modify: `crates/ns-runtime/src/interpreter/sends.rs` (slot pre-check and `slot_selector_shape` removed; the new arm)
- Modify: `crates/ns-runtime/src/image/mixins.rs` (slot record, `plan`, `build`, `check_member_names`)
- Modify: `crates/ns-runtime/src/image/decode.rs` (`verify_mixins` iterates compiled methods)
- Modify: `crates/ns-cli/src/commands/disasm.rs`, `crates/ns-runtime/src/image/inspect.rs` (compiled methods only)
- Modify: `crates/ns-runtime/tests/mixin.rs`, `compiler.rs`, `image.rs` (the new table shape)
- Create: `tests-fixtures/spec_6_3_2_slot_accessors_are_methods.ns`, `.expected`
- Create: `tests-fixtures/spec_2_3_nested_class_overridden_by_slot.ns`, `.expected`
- Create: `tests-fixtures/spec_2_3_superclass_of_a_nested_class.ns`, `.expected`
- Create: `tests-fixtures/spec_6_3_2_public_slot_explicit_receiver.ns`, `.expected`
- Modify: `crates/ns-cli/tests/program_outcomes.rs`

**Interfaces:**
- Produces:

```rust
/// §6.3.2: the getter a slot declaration defines, or a mutable slot's setter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessorKind {
    Getter,
    Setter,
}

/// One of the accessors a slot declaration implicitly defines (§6.3.2):
/// a method with the slot's own accessibility whose body reads or writes
/// the slot. `own_index` is the slot's position among its mixin's own
/// slots, resolved per application through `own_slot_base` exactly as
/// `Instruction::InitializeSlot` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotAccessor {
    pub selector: Symbol,
    pub access: ast::AccessModifier,
    pub own_index: u32,
    pub kind: AccessorKind,
}

/// One entry of a mixin's method table (§3.5): a compiled method or a
/// slot accessor.
#[derive(Debug, Clone)]
pub enum MethodEntry {
    Compiled(Rc<MixinMethod>),
    SlotAccessor(SlotAccessor),
}

impl MethodEntry {
    pub fn access(&self) -> ast::AccessModifier;
    pub fn selector(&self) -> Symbol;
}

pub struct MixinSlot {
    pub name: Symbol,
    pub mutable: bool,
    pub access: ast::AccessModifier,
    pub declaration: Option<Rc<ast::SlotDeclaration>>,
}

/// The accessor entries `slots` define, in declaration order.
pub fn slot_accessors(slots: &[MixinSlot]) -> Vec<(Symbol, MethodEntry)>;

// dispatch.rs
pub enum Lookup<'gc> {
    Method(Rc<MixinMethod>, ClassReference<'gc>),
    /// A slot accessor, with the absolute field index it reads or writes.
    SlotAccessor(SlotAccessor, usize),
    NestedClass(Rc<Mixin>, ClassReference<'gc>),
    Native(BuiltinClass),
    NotFound,
}

// interpreter/slots.rs
impl<'gc> Mutator<'gc> {
    pub(crate) fn decide_slot_accessor(
        &mut self,
        receiver: Value<'gc>,
        accessor: SlotAccessor,
        field: usize,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>>;
}
```

- [ ] **Step 1: Fixture for #1 and the setter's answer.**

```
Newspeak3
'Test'
class Spec632SlotAccessorsAreMethods usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §6.3.2: a slot is accessed by "a getter method ...
       implicitly defined by the slot declaration" and set by "a setter
       method", which "returns the receiver of the message"; "overriding a
       setter will not impact such an initialization, unless the method is
       explicitly invoked in the course of executing the initializer". And
       §2.3: "Methods are always virtual". *)
    class Base = Object new (| public x ::= 1. |) (
        public readX = ( ^x )
        public writeX: value = ( ^x: value )
    )
    class Child = Base new (| public log ::= 'no setter ran'. |) (
        public x = ( ^42 )
        public x: value = ( log:: 'the overriding setter ran'. ^self )
    )
    public main: platform args: args = (
        | base child |
        child:: Child new.
        t show: child log; newLine.
        t show: child readX printString; newLine.
        t show: ((child writeX: 5) == child) printString; newLine.
        t show: child log; newLine.
        base:: Base new.
        t show: ((base writeX: 5) == base) printString; newLine.
        t show: base readX printString; newLine.
    )
) : ()
```

Expected output: `no setter ran`, `42`, `true`, `the overriding setter ran`, `true`, `5`.
- [ ] **Step 2: Fixtures from §2.3's own examples.** `spec_2_3_nested_class_overridden_by_slot.ns` carries the first example verbatim (`ChildOfOuter` with `public foo = (^Inner new)`, `GrandChildOfOuter = ChildOfOuter( | public Inner = String. |) ( )`) and prints `(GrandChildOfOuter new foo class == String) printString` -- "returns a String, not an Inner" -- expected `true`. `spec_2_3_superclass_of_a_nested_class.ns` carries the variation verbatim (`public class Nested = Inner () ()`, `public Inner ::= String.`) and the four `g` examples:

```
        | g first |
        g:: GrandChildOfOuter new.
        t show: (g Inner == String) printString; newLine.
        first:: g Nested.
        t show: ((g Inner: Array) Nested == first) printString; newLine.
        t show: (g Inner == Array) printString; newLine.
```

with each spec comment kept beside its line (`(* returns String *)`, `(* returns the same subclass of String, even though Inner has changed *)`, `(* returns Array *)`); expected `true`, `true`, `true`. Both need the outer module `Outer = Object new () ( public class Inner = ()() )` from §2.3's "class with a single nested class".
- [ ] **Step 3: Fixture for #3.**

```
Newspeak3
'Test'
class Spec632PublicSlotExplicitReceiver usingPlatform: p = Object new (| t = p transcript. |) (
    (* newspeak-spec.pdf §6.3.2: the accessors have "the same" accessibility
       as the slot, and §5.5's lookupPublic answers a public method -- so a
       public slot answers an explicit receiver. *)
    class Holder = Object new (| public z ::= 7. |) ()
    public main: platform args: args = (
        | holder |
        holder:: Holder new.
        t show: holder z printString; newLine.
        t show: ((holder z: 9) == holder) printString; newLine.
        t show: holder z printString; newLine.
    )
) : ()
```

Expected output: `7`, `true`, `9`.
- [ ] **Step 4: Program outcomes for #2 and a protected slot.** `an_immutable_slot_has_no_setter`: `Holder` with `| y = 3. |` and `public attempt = ( self y: 4. ^y )`; exit 1, stderr `Uncaught Newspeak error: an object does not understand #y:\n` (§6.3.2, "There is no setter method associated with the slot"). `a_protected_slot_does_not_answer_an_explicit_receiver`: `| w ::= 8. |` (protected by default, §6.3.1), `main:` sends `Holder new w`; exit 1, stderr `... #w\n` (§5.5).
- [ ] **Step 5: Run them to see them fail.**
- [ ] **Step 6: The mixin side.** `MixinSlot::from_declaration` fills `mutable` (`!matches!(declaration.mutability, ast::SlotMutability::Immutable)`) and `access` (`dispatch::effective_access(declaration.access)`). `slot_accessors` yields, per slot at `index`, the getter under `slot.name` and, for a mutable slot, the setter under `Symbol::intern(&format!("{}:", slot.name))`. `Mixin.methods` becomes `HashMap<Symbol, MethodEntry>`; `build_methods` wraps each compiled method in `MethodEntry::Compiled` and extends the table with `slot_accessors(&slots)` -- the resolver already refused a collision, so a `debug_assert!` on the insert says so. `build_class_side` does the same.
- [ ] **Step 7: The lookup side.** `walk` matches the entry: `Compiled` gives `Lookup::Method` as today; `SlotAccessor(accessor)` gives `Lookup::SlotAccessor(accessor, application.own_slot_base + accessor.own_index as usize)`, the verdict applied to `accessor.access` exactly as to a method's. `private_member` does the same for N's own table. Delete `lookup_slot`, `MixinApplication.slot_index` and its maintenance in `apply_mixin`.
- [ ] **Step 8: Answering an accessor.** `interpreter/slots.rs`:

```rust
//! Slot accessors: `newspeak-spec.pdf` §6.3.2. A slot's getter and setter
//! are methods of its mixin, found by the ordinary lookup of every send
//! kind, and answered here without an activation -- the interpreter's
//! counterpart of Cog's quick methods and SOMns's `CachedSlotRead`.

impl<'gc> Mutator<'gc> {
    /// Reads or writes `field` of `receiver`. The getter answers the
    /// slot's value; the setter stores its one argument and answers the
    /// receiver ("It returns the receiver of the message", §6.3.2). A
    /// receiver without that field -- possible only for a decoded image
    /// whose layout disagrees with its methods -- is a Newspeak error, never
    /// a panic.
    pub(crate) fn decide_slot_accessor(
        &mut self,
        receiver: Value<'gc>,
        accessor: SlotAccessor,
        field: usize,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        let object = match receiver {
            Value::Object(object) => Some(object),
            _ => None,
        };
        let answered = match (accessor.kind, object) {
            (AccessorKind::Getter, Some(object)) => self.heap.instance_field(object, field),
            (AccessorKind::Setter, Some(object)) => arguments
                .first()
                .copied()
                .filter(|value| self.heap.set_instance_field(object, field, *value))
                .map(|_| receiver),
            (_, None) => None,
        };
        answered.map(Step::Value).ok_or_else(|| {
            Signal::Error(NewspeakError {
                selector: accessor.selector,
                receiver,
                message: format!(
                    "{} has no field {field} for the slot accessor {}",
                    self.heap.describe(&receiver),
                    accessor.selector
                ),
            })
        })
    }
}
```

  Every `match` on a `Lookup` gains the arm
  `Lookup::SlotAccessor(accessor, field) => mutator.decide_slot_accessor(receiver, accessor, field, arguments)`:
  `decide_send` (both the metaclass and the class lookups), `decide_lexical_send`
  (receiver `enclosing`), `decide_super_send` (receiver `self`),
  `decide_does_not_understand_in` (the setter of a slot named
  `doesNotUnderstand` is a real `doesNotUnderstand:`, §5.5, and receives the
  message argument). Delete `slot_selector_shape` and the slot pre-check in
  `decide_lexical_send`.
- [ ] **Step 9: Every reader of compiled bodies takes compiled entries.** `image::mixins::plan` collects `MethodEntry::Compiled` entries only; `decode::verify_mixins`, `commands/disasm.rs` and the tests that read `methods[&selector].code` match `MethodEntry::Compiled`.
- [ ] **Step 10: The image.** `encode` writes each slot as its name symbol, a mutability tag (`fn mutability_tag(mutable: bool) -> u8`: 0 immutable, 1 mutable) and `access_tag(slot.access)`; `read_row` reads them back through `mutability_from_tag` (any other tag is `DecodeError::InvalidTag { what: "slot mutability", tag }`) and `access_from_tag`. `build` rebuilds `MixinSlot { name, mutable, access, declaration: None }` and extends each row's method table with `slot_accessors`. Before `build`, a new pass `check_member_names(&raw)` refuses a row whose compiled selectors, slot getters, mutable slots' setters and nested declarations' names (`raw[child].name`) contain one name twice, as `DecodeError::DuplicateSelector(name)` -- `build` then cannot collide. Add a decoder test: a hand-built row with a slot `x` and a method `x` is refused.
- [ ] **Step 11: Update the tests that read the old shape**: `tests/mixin.rs` asserts on accessor entries (`methods[&x]` is a getter with `own_index` 0 at an application whose `own_slot_base` is 2) instead of `slot_index`; `tests/compiler.rs` counts compiled entries.
- [ ] **Step 12: Run the whole suite, the stress suite and both corpus gates.**
- [ ] **Step 13: Commit** `feat: slot accessors are method-table entries (§6.3.2)`.

---

### Task 7: An identity hasher for the method and slot tables

**Files:**
- Create: `crates/ns-runtime/src/hash.rs`
- Modify: `crates/ns-runtime/src/lib.rs` (the module)
- Modify: `crates/ns-runtime/src/mixin.rs` (`Mixin.methods`, `MixinApplication.nested_slot`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs` (`applications`, `empty_class_sides`)
- Modify: every construction site of those maps (`HashMap::new()` becomes `IdentityMap::default()`)

- [ ] **Step 1: Unit tests in `hash.rs`:** interning a thousand consecutive symbols into an `IdentityMap` answers each one back; two consecutive `u32` keys differ in their top seven hash bits (the control bits hashbrown compares) and in their bottom bits; a pointer-aligned `usize` key's hash has non-zero bottom bits.
- [ ] **Step 2: Implement.**

```rust
//! The hasher for tables keyed by an identity the runtime assigned: an
//! interned `Symbol`'s index, a mixin's or application's address. Such keys
//! are already distinct and not chosen by any input -- an image names
//! symbols by text and the interner numbers them -- so SipHash's resistance
//! to chosen keys buys nothing here, and costs a handful of rounds on every
//! probe of every lookup. One multiply mixes a word; the rotation in
//! `finish` brings its well-mixed high bits down to the bucket index, which
//! an address's zero low bits would otherwise decide alone. rustc's own
//! tables hash this way (FxHash).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// An odd constant whose bits are spread evenly, so that consecutive keys
/// land far apart.
const MULTIPLIER: u64 = 0xf135_7aea_2e62_a9c5;

#[derive(Default, Clone, Copy)]
pub struct IdentityHasher {
    hash: u64,
}

impl IdentityHasher {
    fn add(&mut self, word: u64) {
        self.hash = self.hash.wrapping_add(word).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for IdentityHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    fn finish(&self) -> u64 {
        self.hash.rotate_left(26)
    }
}

pub type IdentityBuildHasher = BuildHasherDefault<IdentityHasher>;

/// A `HashMap` hashed by `IdentityHasher`.
pub type IdentityMap<K, V> = HashMap<K, V, IdentityBuildHasher>;
```

- [ ] **Step 3: Switch the four tables** and their construction sites; run the suite, stress suite, corpus gates.
- [ ] **Step 4: Commit** `perf: hash identity-keyed tables with a multiply-rotate hasher`.

---

### Task 8: Measure, record, close the step

**Files:**
- Modify: `docs/design/execution.md` ("What removing the re-entries moved": a subsection for this step)
- Modify: `docs/PROGRESS.md`

- [ ] **Step 1: Measure by the protocol** in `execution.md`: every compared binary built with `RUSTFLAGS="-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"` (PowerShell: `$env:RUSTFLAGS = …` before the build, `Remove-Item Env:RUSTFLAGS` after), the seven sources, two sittings, this branch against `59061ec` and against `be3eb40`. Record medians and ranges per source.
- [ ] **Step 2: Record the step** in `execution.md` and `PROGRESS.md` (a milestone section, the order-of-work entry, "Where the next session starts": step 3 next, with the two open questions from `dispatch.md` and the known gaps -- lazy slots, the class-side scope question, builtin class names resolved statically rather than as sends to `Object`'s accessors (§3.2)).
- [ ] **Step 3: All gates**, then commit `docs: record spec-conformant lookup and its measurement`.
