# Predefined Classes as Accessors on `Object` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A bare builtin class name (`String`, `Array`, `Object`, …) becomes the implicit-receiver send `self String`, answered by an accessor method on `Kernel.ns`'s `Object` without an activation. `LoadBuiltinClass` and its wire tag are retired.

**Architecture:** `Object` gains one public accessor per nameable builtin, `public String = ( ^outer Kernel String )`. While the kernel compiles, the method is recognized as a *fast path*: `MixinMethod.primitive` becomes `fast_path: Option<MethodFastPath>`, with a new variant `BuiltinClass(kind)` that `act` answers directly. The resolver drops its "known builtin" base case, so such a name binds `LexicalSend { depth: 0 }`. Both lowerings stop emitting `LoadBuiltinClass`, and the instruction and its opcode are removed.

**Tech Stack:** Rust 1.98.0, edition 2021; crates `ns-syntax`, `ns-runtime`, `ns-cli`; `Kernel.ns`; goldens.

**Spec:** `docs/design/core-library.md`, "Planned: the predefined classes are accessors on `Object`". Read it before any task.

## Global Constraints

- Read `CLAUDE.md` first; its invariants bind every task. The ones this plan touches most:
  - **A cache records what the lookup decided; it never decides.** `act` reads the fast path off the method on every send.
  - `vm.rs` holds no dispatch logic.
  - Compiling a resolved module can never raise.
  - A wire tag is assigned once and never renumbered or reused.
  - The VM mirror is a capability.
  - Program outcomes are pinned.
- Names are written out: no one- or two-character identifiers and no truncated words, test code included. `#![forbid(unsafe_code)]` stays. Inline Newspeak in Rust tests is `indoc!`. Comments explain the decision.
- **The accessor set is exactly:** `Object`, `Value`, `Boolean`, `True`, `False`, `Number`, `Integer`, `Float`, `String`, `Symbol`, `Character`, `Nil`, `Array`, `OrderedCollection`, `Block`. `Top`, the VM mirror's class and `Platform` get none.
- **The accessor body is `^outer Kernel <Name>`.** A bare `^<Name>` inside `Object` would, by §5.7, find `Object`'s own accessor and recurse forever.
- The fast path is recognized **only** while compiling `Kernel.ns`. A user module can never get one, and an image never carries one.
- Branch: `feature/object-class-accessors`, where the design commits already are.
- Every commit message ends with exactly these two lines (the implementer's own model name in the first):
  ```
  Co-Authored-By: <model name> <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_017cQANuhD4B27SoKJxmdzV5
  ```
- **Per-commit gate**, all green before every commit:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets          # warning-free
  cargo test --workspace
  cargo run -p ns-cli -- parse-corpus corpus/modules   # "parsed 210, failed 0, expected-fail 1, stale 0"
  ```
  A commit that changes only Markdown needs none of it.
- Run every command in the foreground.
- A golden's recorded outcome changes only where a task argues it against the specification. Otherwise, stop and report.

## Review Focus

1. **Class-side code naming a builtin** (`Array new` inside a class-side method): `self` is a class object, so the send must reach `Object`'s accessor through the metaclass-miss fall-through. Task 2 pins it with a golden.
2. **A builtin name inside a block**, including a block in class-side code: the block's `self` is its home method's receiver. Task 2 pins it.
3. **An enclosing class that declares a member with a builtin's name**: the lexical declaration wins over `Object`'s accessor, in method bodies and in a nested class's superclass clause alike. Task 2 pins both.
4. **An old artifact carrying opcode 2**: it must be refused as an unknown or retired tag, never decoded. Task 3 pins it.
5. **Kernel methods naming builtins**, e.g. inside `Integer`: they must still find the kernel's own nested class lexically, not recurse through `Object`. The whole suite exercises this after Task 1; Task 1 adds one direct test.

---

### Task 1: The accessors and the fast path

Additive: nothing yet sends these accessors implicitly, only an explicit `self String` does.

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns` (`Object`'s body)
- Modify: `crates/ns-runtime/src/mixin.rs` (`MixinMethod.primitive` → `fast_path`, new `MethodFastPath`)
- Modify: `crates/ns-runtime/src/compiler/primitive_method.rs` (recognize the accessor shape)
- Modify: `crates/ns-runtime/src/types.rs` (`ResolvedModule` gains `pub kernel: bool`)
- Modify: `crates/ns-runtime/src/resolver/mod.rs` (set `kernel` in the `ResolvedModule` it builds)
- Modify: whatever threads the flag from `ResolvedModule` to `MixinMethod::from_declaration`: `interpreter/instantiate.rs`'s `build_methods`/`mixin_of`/`build_mixin`, and `dispatch.rs:434`
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs` (`act` and `act_handler_method`)
- Modify: every reader of `MixinMethod.primitive`: `dispatch.rs` (tests around 552-657), `interpreter/kernel.rs` (~705-720), `image/mixins.rs:758`, and any others `cargo check` reports
- Test: `crates/ns-runtime/tests/object_class_accessors.rs` (new), `interpreter/kernel.rs` tests

**Interfaces:**
- Produces, in `mixin.rs`:
  ```rust
  /// What a kernel method's send does without an activation, read off the
  /// method by `act` on every send (`docs/design/dispatch.md`,
  /// "Primitive methods"). Only `Kernel.ns`'s methods carry one.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum MethodFastPath {
      /// The method forwards to one mirror row: run the leaf, and activate
      /// the method only when the leaf declines.
      Mirror(crate::builtins::primitive::MirrorPrimitive),
      /// The method answers a builtin class (§3.2's accessors on
      /// `Object`): answer it, never activate.
      BuiltinClass(crate::types::BuiltinClass),
  }
  ```
  `MixinMethod.fast_path: Option<MethodFastPath>` replaces `primitive`.
- Produces: `ResolvedModule.kernel: bool`, true only from `resolve_kernel`.
- Produces: `primitive_method::recognize(declaration, bindings, kernel: bool) -> Option<MethodFastPath>`, or two functions. Do not change the existing mirror recognition's behaviour.

- [ ] **Step 1: Failing tests.** Create `crates/ns-runtime/tests/object_class_accessors.rs`. Copy `load`, `expect_ok` and `expect_error` from `tests/mixin.rs`, with their imports. Write the tests:

```rust
/// §3.2: `Object` provides accessors for the predefined classes. An
/// explicit `self String` reaches `Object`'s accessor and answers the very
/// class a string literal has.
#[test]
fn an_explicit_self_send_of_a_class_name_answers_the_builtin_class() {
    let source = indoc! {"
        Newspeak3
        'Test'
        class Probe = Object new () (
            public same = ( ^(self String) == 'text' class )
            public array = ( ^(self Array) new size )
        ) : ()
    "};
    let (mut interpreter, class, _resolved) = load(source);
    let probe = expect_ok(interpreter.instantiate(&class, &[]), "Probe new");
    let same = expect_ok(interpreter.send(&probe, Symbol::intern("same"), &[]), "same");
    let size = expect_ok(interpreter.send(&probe, Symbol::intern("array"), &[]), "array");
    interpreter.enter(|mutator| {
        assert!(matches!(mutator.fetch(&same), Value::Boolean(true)));
        assert!(matches!(mutator.fetch(&size), Value::SmallInteger(0)));
    });
}
```

In `interpreter/kernel.rs`'s `mod tests`:

```rust
    /// The accessor set on `Object` is exactly §3.2's nameable builtins,
    /// each recognized as a fast path answering its own class; `Top`, the
    /// mirror's class and `Platform` have none, which is one of the locks
    /// that keep the mirror a capability.
    #[test]
    fn objects_class_accessors_are_exactly_the_nameable_builtins() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.builtin_class(BuiltinClass::Object);
            let application = mutator
                .heap()
                .class_application(object)
                .expect("Object has an application");
            let expected = [
                BuiltinClass::Object, BuiltinClass::Value, BuiltinClass::Boolean,
                BuiltinClass::True, BuiltinClass::False, BuiltinClass::Number,
                BuiltinClass::Integer, BuiltinClass::Float, BuiltinClass::String,
                BuiltinClass::Symbol, BuiltinClass::Character, BuiltinClass::Nil,
                BuiltinClass::Array, BuiltinClass::OrderedCollection, BuiltinClass::Block,
            ];
            for kind in expected {
                let selector = crate::Symbol::intern(super::kernel_class_name(kind));
                let method = application
                    .mixin
                    .compiled_method(selector)
                    .unwrap_or_else(|| panic!("Object declares no accessor {kind:?}"));
                assert_eq!(
                    method.fast_path,
                    Some(crate::mixin::MethodFastPath::BuiltinClass(kind)),
                    "{kind:?}"
                );
            }
            for absent in ["Top", "Platform", "VmMirror"] {
                assert!(
                    application.mixin.compiled_method(crate::Symbol::intern(absent)).is_none(),
                    "Object must declare no accessor {absent}"
                );
            }
        });
    }
```

Adapt the paths (`super::kernel_class_name`, imports) to the module as it is.

- [ ] **Step 2: Run them.** `cargo test -p ns-runtime --test object_class_accessors` and `cargo test -p ns-runtime kernel::tests`. Expected: FAIL, because `Object` has no such methods and `fast_path` does not exist.

- [ ] **Step 3: `Kernel.ns`.** In `Object`'s body, after `printString`, add the fifteen accessors with a comment citing §3.2 and saying why the body is `outer Kernel`:

```newspeak
        (* §3.2: Object "provides ... accessors for predefined classes and
           objects, such as the classes of the built-in literals", so a bare
           `String` anywhere is the implicit send `self String` and lands
           here unless something nearer declares the name. Each body names
           the kernel explicitly: inside Object, §5.7's innermost
           declaration of `String` is this very method. `Top`, the VM
           mirror's class and `Platform` have no accessor. *)
        public Object = ( ^outer Kernel Object )
        public Value = ( ^outer Kernel Value )
        public Boolean = ( ^outer Kernel Boolean )
        public True = ( ^outer Kernel True )
        public False = ( ^outer Kernel False )
        public Number = ( ^outer Kernel Number )
        public Integer = ( ^outer Kernel Integer )
        public Float = ( ^outer Kernel Float )
        public String = ( ^outer Kernel String )
        public Symbol = ( ^outer Kernel Symbol )
        public Character = ( ^outer Kernel Character )
        public Nil = ( ^outer Kernel Nil )
        public Array = ( ^outer Kernel Array )
        public OrderedCollection = ( ^outer Kernel OrderedCollection )
        public Block = ( ^outer Kernel Block )
```

`Object` then declares members with the same names as its sibling classes. The resolver's duplicate-member check (`classes::duplicate_member_names`) is per declaration, so this is legal. If `check_kernel_shape` or another bootstrap assertion objects, adjust the assertion, not the design, and say so in the report.

- [ ] **Step 4: The kernel flag.** Add `pub kernel: bool` to `ResolvedModule`, with a doc comment that it is true only for `Kernel.ns`, set by `resolve_kernel`, and that it is what confines fast paths to the kernel. The resolver already has a `kernel` field; carry it into the `ResolvedModule` it returns. Thread it to `MixinMethod::from_declaration`, choosing the least invasive route. Passing `resolved.kernel` down through `build_mixin`/`mixin_of`/`build_methods` is acceptable. So is bundling it with `bindings` in a small struct those functions already pass around, if one exists. `dispatch.rs:434`'s test constructor passes `false`.

- [ ] **Step 5: `MethodFastPath` and the recognizer.** Rename `MixinMethod.primitive` to `fast_path: Option<MethodFastPath>`. The existing mirror recognition yields `MethodFastPath::Mirror(row)`. Add the accessor recognition in `compiler/primitive_method.rs`: when `kernel` is true, the method is unary with no temporaries, and the body is exactly one `Return` of a `Send` such that
  - its receiver is the `outer Kernel` shape (`resolver::sends::outer_class_name` answers `"Kernel"`; widen it to `pub(crate)` if needed),
  - its message is unary and names a builtin via the kernel's own name table (`interpreter::kernel`'s `kernel_kind_named`, which excludes the mirror; `Top` must also be refused here),
  - and the resolver recorded a `Binding::LexicalSend` at that send's span,

  then answer `MethodFastPath::BuiltinClass(kind)`. The method's own selector does not have to equal the name, but the kernel's accessors all match. Do not read `NS_NEXT_NO_PRIMITIVE_METHODS` for this variant, because the switch is about mirror leaves. Say so in a comment.

- [ ] **Step 6: `act`.** In `interpreter/resolve.rs`'s `act`, `Resolved::Method`'s non-handler arm:

```rust
                    // Read off the method on every send, hit or miss: the
                    // cache records `Target::Method` and never decides what
                    // the method does.
                    match method.fast_path {
                        Some(MethodFastPath::BuiltinClass(kind)) => {
                            return Ok(Step::Value(Value::Object(self.builtin_class(kind))));
                        }
                        Some(MethodFastPath::Mirror(primitive)) => {
                            if let Some(result) = crate::interpreter::mirror::execute_method(
                                self, primitive, receiver, selector, arguments,
                            ) {
                                return result;
                            }
                        }
                        None => {}
                    }
```

A unary accessor takes no arguments, and a send that reaches it with arguments cannot find it under a unary selector. `act_handler_method` only ever runs a `doesNotUnderstand:` handler. Match `Mirror` there as before, and treat `BuiltinClass` as unreachable. It must not panic, so fall through to the activation.

- [ ] **Step 7: Update every reader** of the old field (`cargo check --workspace --all-targets` lists them). The kernel test near `kernel.rs:705` that lists methods which are not forwarders must accept the accessors, as a fast-path kind of its own.

- [ ] **Step 8: Run** the two new tests (they pass now) and the per-commit gate. No golden may change.

- [ ] **Step 9: Commit** `feat: give Object an accessor for each predefined class, answered without an activation`.

---

### Task 2: A bare builtin name is an implicit-receiver send

**Files:**
- Modify: `crates/ns-runtime/src/resolver/mod.rs` (`resolve_identifier` ~331, `resolve_superclass_clause` ~472; delete the builtin base cases and their comments)
- Modify: `crates/ns-runtime/src/resolver/classes.rs` (delete `is_builtin_class_name` and its test if nothing else uses it)
- Modify: `crates/ns-runtime/src/compiler/lower.rs` (~1115-1150) and `crates/ns-runtime/src/compiler/reference.rs` (~355-365): no binding → `UnresolvedIdentifier`, no builtin lookup
- Modify: `crates/ns-runtime/src/compiler/lower.rs` test ~2940-2965 (it pins `LoadBuiltinClass` for `^OrderedCollection`; it now pins a self send)
- Create: goldens under `goldens/nesting/`, `goldens/class-side/` and `goldens/errors/` (Step 1)
- Test: `crates/ns-runtime/tests/object_class_accessors.rs`

**Interfaces:**
- Consumes: Task 1's accessors and fast path.
- Produces: no lowering emits `Instruction::LoadBuiltinClass`. Task 3 deletes it.

- [ ] **Step 1: Failing goldens.** Each has a `.ns` with a comment citing §3.2, §5.7 and §2.3.1 as relevant, and a `.toml`. Under the current code each must fail or differ:

`goldens/nesting/a_module_that_declares_string_uses_its_own.ns`: the module declares a method `String` answering `'mine'`, and `main:args:` prints `String`. The lexical declaration beats `Object`'s accessor. The resolver's `find_member_depth` already finds a lexical member before its builtin check, so this golden passes before and after the change. It is a regression guard and does not need to fail first.

```newspeak
class ModuleDeclaresString usingPlatform: platformAtCreation = Object new () (
    String = ( ^'mine' )
    public main: platform args: arguments = (
        platform transcript show: String; newLine
    )
) : ()
```
stdout `mine`.

`goldens/nesting/an_inherited_member_named_like_a_builtin_wins.ns`: this one fails today.

```newspeak
class InheritedBuiltinName usingPlatform: platformAtCreation = Object new () (
    class Base = Object new () (
        public Array = ( ^'inherited' )
    )
    class Derived = Base new () (
        public show = ( ^Array )
    )
    public main: platform args: arguments = (
        platform transcript show: Derived new show; newLine
    )
) : ()
```
stdout `inherited`. Today it prints the class's `printString`, because the constant won. The comment cites §2.3.1: "lookup proceeds up the lexical scope chain … and only if no lexically visible matching method is found, do we proceed up the inheritance chain of self".

`goldens/class-side/a_class_side_method_names_a_builtin.ns`: a class-side method `make = ( ^Array new )`, plus a block form `makeLater = ( ^[Array new] value )`. `main:args:` prints `(Maker make) size printString` and `(Maker makeLater) size printString`. stdout `0` and `0`.

`goldens/nesting/an_enclosing_member_named_like_a_builtin_is_the_superclass.ns`: the module declares a nested class `Object = Object new () ( public tag = ( ^'local' ) )`… If a nested class named `Object` whose own clause names `Object` is refused as a cyclic creation, use a slot or method `Base` instead, and name the file accordingly. The intent: a nested class's superclass clause naming a builtin name that an enclosing class declares finds the enclosing declaration. The implementer picks the smallest program that expresses this without a cycle, records what it prints, and argues in the comment that the output is §5.7's.

`goldens/errors/top_is_not_a_name.ns`: `main:args:` does `platform transcript show: self Top printString`. Expected status 1 with `Uncaught Newspeak error: <the exact doesNotUnderstand text the runtime prints for #Top>`. Run it once to read the exact text, check that it is the ordinary `doesNotUnderstand:` report, and record it. Add a second statement or golden for the mirror's class name the same way (`VmMirror` is `kernel_class_name`'s spelling; check what the kernel calls it). Today a bare `Top` is not a builtin name either, so this golden may pass before and after. It pins the lock.

In `tests/object_class_accessors.rs`, add a Rust test for Review Focus 5. A kernel method that names a builtin, e.g. `Integer>>printString`, if it uses `String`, or any kernel method that does, still runs, which the whole suite covers. Add explicitly: `3 printString` answers `'3'` under both lowerings (`Interpreter::bootstrap_with(Lowering::Reference)`, as `tests/image.rs` does).

- [ ] **Step 2: Run** the goldens (`cargo test -p ns-cli --test run_goldens`). Expected: `an_inherited_member_named_like_a_builtin_wins` fails; the others record their outcomes.

- [ ] **Step 3: Resolver.** In `resolve_identifier`, delete the `None if classes::is_builtin_class_name(name)` arm, so an unfound name binds `LexicalSend { depth: 0 }`. In `resolve_superclass_clause`'s unprefixed arm, delete the builtin arm too, so the `!enclosing_chain.is_empty()` arm binds `LexicalSend { depth: 0 }`. For the root, the resolver no longer calls this function (the root rule), so the arm that pushes a resolve error for a top-level name becomes unreachable. Delete it. If the compiler proves otherwise, keep it and explain in the report. Update the doc comments that describe the base case. Delete `classes::is_builtin_class_name` if it is unused.

- [ ] **Step 4: Both lowerings.** In `lower.rs` (~1125) and `reference.rs` (~358), the `Some(Binding::SuperSend | Binding::MirrorSend) | None` arm no longer consults `builtin_kind_from_name`. It always emits `LoadLiteral { literal: UnresolvedIdentifier(name) }`. Keep the comment's reasoning about `super`, and add that a module root's clause receivers, which the resolver leaves unbound, land here and never run. Update the `lower.rs` unit test (~2945) to pin that `^OrderedCollection` in a method lowers to a self send of `OrderedCollection` (a `LexicalSend` at depth 0; match the instruction the lowering really emits), not `LoadBuiltinClass`.

- [ ] **Step 5: The whole suite.** Run the per-commit gate. **If any existing golden's outcome changes**, it is the shadowing shape: a class whose inheritance chain answers a builtin name. Establish that the new output is §2.3.1's before re-recording it. Write the argument into that golden's comment and list it in the report. Any other change means stop and report NEEDS_CONTEXT. Also run `cargo run -p ns-cli -- run-corpus corpus/modules`, which must print "ran 6, failed 0, skipped 205".

- [ ] **Step 6: Commit** `feat: resolve a bare builtin class name as a send to Object's accessor`.

---

### Task 3: Retire `LoadBuiltinClass` and wire tag 2

**Files:**
- Modify: `crates/ns-runtime/src/bytecode.rs` (the variant, its display, `outer_reach_of`, and any exhaustive matches)
- Modify: `crates/ns-runtime/src/vm.rs:178` (the arm)
- Modify: `crates/ns-runtime/src/compiler/reference.rs` (~645, ~659)
- Modify: `crates/ns-runtime/src/image/opcodes.rs` (move 2 into `RETIRED_OPCODES`, delete `OPCODE_LOAD_BUILTIN_CLASS`, and extend the history comment)
- Modify: `crates/ns-runtime/src/image/encode.rs:509`, `crates/ns-runtime/src/image/decode.rs` (~1297, ~1414, ~1618; keep `read_program_builtin_class` for records and update its doc comment ~746)
- Modify: `crates/ns-runtime/src/interpreter/instantiate.rs` (delete `builtin_kind_from_name` if it is now unused; update the comments in `kernel.rs:65`, `resolver/classes.rs:23` and `image/decode.rs:755` that name it)
- Modify: `crates/ns-runtime/tests/opcode_tags.rs`
- Modify: `crates/ns-cli` (`disasm`/`inspect` renderings, if they name the instruction)
- Test: `crates/ns-runtime/tests/image.rs`

- [ ] **Step 1: Failing test.** In `tests/opcode_tags.rs`, pin that `2` is in `RETIRED_OPCODES`, following the file's existing pattern for retired tags. In `tests/image.rs`, add a test that a code-table instruction byte `2` is refused. Follow an existing test that feeds a retired opcode (search for `RETIRED_OPCODES` or a retired tag such as `7` in that file) and assert the same error kind that test asserts.

- [ ] **Step 2: Run** `cargo test -p ns-runtime --test opcode_tags --test image`. Expected: FAIL.

- [ ] **Step 3: Remove** the instruction everywhere the compiler reports it. Retire the tag, and extend the comment at the head of `opcodes.rs` with one sentence saying that tag 2 was retired on 2026-09-24 when a builtin class name became a send (§3.2). Delete `builtin_kind_from_name` if nothing uses it. If the root rule or something else still does, keep it and correct its doc comment.

- [ ] **Step 4: Run** the tests (pass) and the per-commit gate.

- [ ] **Step 5: Commit** `refactor: retire LoadBuiltinClass and its wire tag`.

---

### Task 4: Documents

Markdown, plus `CLAUDE.md`. No test run is needed.

- [ ] **Step 1:** `docs/design/core-library.md`: fold "Planned: the predefined classes are accessors on `Object`" into the built description. It belongs under "What exists" or a section of its own that no longer says "Planned". Keep the decisions with their date (2026-09-24) and the reasons: why all the nameable builtins, why `outer Kernel`, why the fast path, and why the tag is retired. Add the accessors to the list of `Object`'s methods in "What exists". Add a bullet to "Decisions".
- [ ] **Step 2:** `docs/design/dispatch.md`: the "Binding variants" note becomes present tense. In "Primitive methods", describe `MethodFastPath` and its two variants.
- [ ] **Step 3:** `docs/design/compiler.md`: remove every mention of `LoadBuiltinClass`, or state that it is retired, and describe how a bare builtin name lowers (a self send). `docs/design/image-format.md`: tag 2 is retired, wherever retired opcodes are listed. `docs/design/object-model.md`: if it says builtin names resolve by name, correct it.
- [ ] **Step 4:** `CLAUDE.md`: in "The VM mirror is a capability", the lock "no bare name resolves to it (`instantiate::builtin_kind_from_name` omits it)" becomes "`Object` declares no accessor for it, or for `Top` (`interpreter/kernel.rs`'s test pins the accessor set)". Search `CLAUDE.md` for `builtin_kind_from_name` and `LoadBuiltinClass`, and correct every other hit.
- [ ] **Step 5:** `docs/PROGRESS.md`: add a milestone entry "The predefined classes as accessors on `Object`" in the neighbours' style, with a clearly marked placeholder for Task 5's measurement. "Where the next session starts" names `Class`, `Behavior` and `Metaclass` as next again.
- [ ] **Step 6:** `grep -rn "LoadBuiltinClass\|builtin_kind_from_name\|is_builtin_class_name\|known builtin" docs CLAUDE.md crates` finds nothing stale.
- [ ] **Step 7: Commit** `docs: describe the predefined classes as accessors on Object`.

---

### Task 5: Final gates and measurement

- [ ] **Step 1: End-of-batch gates** (Git Bash), each green:
  ```bash
  NS_NEXT_STRESS_MOVES=1 cargo test --workspace
  NS_NEXT_STRESS_CACHES=1 cargo test --workspace
  NS_NEXT_STRESS_COLLECT=1 cargo test --workspace
  cargo run -p ns-cli -- run-corpus corpus/modules          # "ran 6, failed 0, skipped 205"
  NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
  NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
  NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
  cargo build --workspace
  ```
- [ ] **Step 2: Measurement.** The baseline is `2825ec9` (master before this branch), built in a temporary `git worktree` outside the repo and removed afterwards. Use CLAUDE.md's pinned flags. Follow `docs/design/execution.md`'s protocol, **including the paragraph added by the value-conditions work: plain builds are byte-identical, so vary the code layout with `-C metadata=<word>` and pool several layouts per tree.** Time every source under `goldens/bench/` and start-up. The sources that name builtin classes in their loops (`bench_new`, `bench_newvalue` and any that write `Array new` or `OrderedCollection new`) are the ones this change can move. Record everything under a new heading "What the class accessors cost" in `execution.md`, and replace the placeholder in `PROGRESS.md`. If a source is measurably slower, report it with numbers. Do not tune in this task. If `dumpbin` is available (Visual Studio Build Tools; search properly before concluding it is absent), check that `vm::run` did not change size beyond the removed arm.
- [ ] **Step 3: Commit** `docs: record what the class accessors cost`.
