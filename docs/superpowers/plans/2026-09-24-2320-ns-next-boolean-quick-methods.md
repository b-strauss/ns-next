# Boolean in Newspeak over Quick Methods Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `True` and `False` implement their whole protocol as ordinary
`Kernel.ns` methods, as every prior-art Newspeak kernel does, with no VM
mirror row behind them; the kernel compiler recognizes the trivial bodies
those methods have as *quick methods* that run without an activation, so the
depth count still agrees under both lowerings.

**Architecture:** Three new `MethodFastPath` variants, recognized only while
compiling `Kernel.ns`, the way `MethodFastPath::BuiltinClass` already is:
`^true`/`^false`/`^nil` answer the constant, `^parameter` answers that
argument, and `^parameter value` answers a control-flow block step for a
closure argument -- the step today's Boolean leaves answer -- and otherwise
activates the method, whose body then sends `value` for real. `act` reads the
fast path off the method on every send, as it does for the other two. With
that in place, the eleven Boolean mirror rows, `BooleanPrimitive`,
`builtins/boolean.rs` and `Continuation::Truthy` go away.

**Tech Stack:** Rust 1.98.0 (edition 2021), `ns-runtime`, `Kernel.ns`, the
golden harness (`crates/ns-cli/tests/run_goldens.rs`).

**Spec:** the decisions below, taken with the user on 2026-09-24, and
`docs/design/core-library.md`, "Planned: real bodies for control flow and
`do:`, before the JIT tiers", whose Boolean half this plan pulls forward.
Semantics authority: `docs/references/newspeak-spec.md` §5.1.2 (true/false
are the unique instances of `True`/`False`); the library behaviour follows
prior art, which the specification leaves to the library.

## Decisions this plan implements (user, 2026-09-24)

1. **The Boolean family moves to Newspeak now**, ahead of the rest of
   "Planned: real bodies for control flow and `do:`". `whileTrue:`,
   `whileFalse:`, `do:` and `value` stay leaves, as decision 7 of the kernel
   move ("Kernel method bodies forward; they do not reimplement") still says
   for them.
2. **`and:`, `or:`, `&` and `|` no longer coerce.** `true and: [3]` answers
   `3`, as in SOMns (`prior-art/somns/core-lib/Kernel.ns`, `True>>and:`),
   Primordial Soup (`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
   `True>>and:`), Newspeak on Squeak (`prior-art/newspeak/squeak/KernelForSqueak.ns`,
   `False>>or:`) and the JS kernel. `builtins::boolean::truthy` and the
   compiler's `emit_coerce_to_boolean` go.
3. **A non-block argument is an error, as in Primordial Soup.** There is no
   `Object>>value`. `true ifTrue: 3` sends `value` to `3`, which ends in
   `doesNotUnderstand:`; `false ifTrue: 3` answers `nil` without looking at
   the argument, since `False>>ifTrue:` is `^nil`.
4. **Quick methods (option A)** keep the depth count agreeing: Squeak/Cog's
   quick methods (`^self`, `^true`, `^false`, `^nil`, `^instVar` run without
   an activation), extended by `^parameter value`, and in this runtime the
   same kind of fast path as `MethodFastPath::BuiltinClass` ("answer it,
   never activate"). Only the three shapes above are recognized -- no
   `^self`, no other literal, no instance-variable read: nothing asks for
   them.

## Global Constraints

- Every name written out (`CLAUDE.md`, "Names are written out"); clippy's
  `min_ident_chars` refuses one- and two-character names.
- Never `{:?}` a `Value`; never `.unwrap()`/`.expect()` an `EvaluationResult`
  or `HostResult` -- use the file's `expect_ok` helper.
- Inline Newspeak in a Rust test is an `indoc! {"…"}` / `formatdoc! {"…"}`.
- A golden's expected output changes only after establishing the new output
  is right; this plan names every golden that changes and why.
- `vm.rs` gains nothing: the fast path lives in `interpreter/resolve.rs`'s
  `act_with_base`, beside the other two.
- Quick methods are recognized **only for the kernel** (`kernel == true`),
  like the builtin-class accessor. A user module's method is always
  activated.
- Code, comments, commit messages in English; conventional commits; every
  commit ends with
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_017smvatH8KLqErn4PtTGi1M
  ```
  naming the model that actually wrote the commit (a subagent names its own).
- Per-commit gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets`
  (warning-free), `cargo test --workspace`,
  `cargo run -p ns-cli -- parse-corpus corpus/modules` ("parsed 210, failed 0,
  expected-fail 1, stale 0"). The stress suites and `run-corpus` run once, in
  Task 5. A Markdown-only commit runs none of them.
- Work on a branch `feature/boolean-quick-methods` checked out in the main
  working tree (no worktree).

## Review Focus

1. **Depth count under the reference lowering through each quick form.** A
   recursion whose only per-level work goes through `and:`, `or:`,
   `ifTrue:`, `ifFalse:` with literal blocks must overflow at the same point
   under both lowerings. If any one `True`/`False` method is *not*
   recognized, the reference lowering counts an extra activation per level
   and prints less progress. Pinned by
   `goldens/kernel/recursion_through_and_or_counts_alike_under_both_lowerings.ns`
   (Task 3) and by the kernel table test (Task 3).
2. **A block that takes a parameter passed to a conditional through a
   variable** must still raise `wrong number of block arguments: block takes
   1, got 0` -- the `^parameter value` fast path's block step goes through
   the same `check_block_arity`. Pinned by the existing
   `goldens/errors/if_block_that_takes_an_argument_is_a_wrong_arity_error.ns`.
3. **A non-block argument on the evaluating side** (`true ifTrue: 3`,
   `true and: 3`, `false or: 3`, `false ifFalse: 3`) must raise
   `an object does not understand #value`, and on the short-circuit side
   (`false ifTrue: 3`, `false and: 3`, `true or: 3`) must answer without
   touching it. Pinned by two new goldens (Task 3).
4. **A non-Boolean receiver of an inlined conditional** still sends the real
   message and ends in `doesNotUnderstand:` -- unchanged, since that path
   never reached the Boolean methods. Pinned by the existing
   `goldens/language/control_flow_fallback.ns` and the `capture/` fallback
   goldens.
5. **A user subclass of `True`** now inherits real methods: `MyTrue new
   ifTrue: [...]` runs the block and `printString` answers `'true'`, where
   today the leaf declines and the send ends in `Object`'s methods. This is
   the consequence of decision 1 (Newspeak methods need no payload), as in
   every prior-art kernel. Pinned by a new golden (Task 3); `MyBoolean`'s
   line in `goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods`
   stays `true an Object`, since `Boolean` itself declares only `hash`.

---

### Task 1: Quick methods -- recognition and `act`

**Files:**
- Modify: `crates/ns-runtime/src/mixin.rs:128-135` (`MethodFastPath`)
- Modify: `crates/ns-runtime/src/compiler/primitive_method.rs` (recognizer and its tests)
- Modify: `crates/ns-runtime/src/interpreter/resolve.rs:414-426` (`act_with_base`), `:487-495` (`act_handler_method`'s comment)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:165-176` (`StepArguments::ControlFlow`'s comment)

**Interfaces:**
- Produces:
  ```rust
  // mixin.rs
  pub enum MethodFastPath {
      Mirror(crate::builtins::primitive::MirrorPrimitive),
      BuiltinClass(crate::types::BuiltinClass),
      /// `^true`, `^false` or `^nil`: answer it, never activate.
      Constant(QuickConstant),
      /// `^parameter`: answer the argument at this position, never activate.
      Parameter(u8),
      /// `^parameter value`: a closure argument at this position is answered
      /// as the block to activate, as a control-flow step; anything else
      /// activates the method, whose body sends `value` for real.
      ValueOfParameter(u8),
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum QuickConstant { True, False, Nil }

  impl QuickConstant {
      pub(crate) fn value<'gc>(self) -> crate::types::Value<'gc>;
  }
  ```
  Task 3 relies on these names in its kernel table test.

- [ ] **Step 1: Create the branch**

```bash
git checkout -b feature/boolean-quick-methods
```

- [ ] **Step 2: Write the failing recognizer tests**

Append to the `tests` module of `crates/ns-runtime/src/compiler/primitive_method.rs`
(it already has `fast_path_in_the_kernel` and `fast_path_outside_the_kernel`):

```rust
    #[test]
    fn recognizes_a_constant_answer() {
        assert_eq!(
            fast_path_in_the_kernel("not = ( ^false )"),
            Some(MethodFastPath::Constant(QuickConstant::False))
        );
        assert_eq!(
            fast_path_in_the_kernel("or: alternative = ( ^true )"),
            Some(MethodFastPath::Constant(QuickConstant::True))
        );
        assert_eq!(
            fast_path_in_the_kernel("ifFalse: falseBlock = ( ^nil )"),
            Some(MethodFastPath::Constant(QuickConstant::Nil))
        );
    }

    #[test]
    fn recognizes_a_parameter_answer() {
        assert_eq!(
            fast_path_in_the_kernel("& other = ( ^other )"),
            Some(MethodFastPath::Parameter(0))
        );
        assert_eq!(
            fast_path_in_the_kernel("at: first put: second = ( ^second )"),
            Some(MethodFastPath::Parameter(1))
        );
    }

    #[test]
    fn recognizes_a_parameter_s_value() {
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( ^trueBlock value )"),
            Some(MethodFastPath::ValueOfParameter(0))
        );
        assert_eq!(
            fast_path_in_the_kernel(
                "ifTrue: trueBlock ifFalse: falseBlock = ( ^falseBlock value )"
            ),
            Some(MethodFastPath::ValueOfParameter(1))
        );
    }

    /// Only the three shapes: another literal, `self`, another selector or
    /// an argument to `value` is an ordinary method.
    #[test]
    fn does_not_recognize_other_trivial_bodies() {
        assert_eq!(fast_path_in_the_kernel("size = ( ^3 )"), None);
        assert_eq!(fast_path_in_the_kernel("yourself = ( ^self )"), None);
        assert_eq!(fast_path_in_the_kernel("text = ( ^'true' )"), None);
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( ^trueBlock yourself )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( ^trueBlock value: 1 )"),
            None
        );
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( ^trueBlock value value )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_quick_body_with_a_temporary_or_a_second_statement() {
        assert_eq!(
            fast_path_in_the_kernel("not = ( | temporary | ^false )"),
            None
        );
        assert_eq!(fast_path_in_the_kernel("not = ( self foo. ^false )"), None);
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( self foo. ^trueBlock value )"),
            None
        );
    }

    #[test]
    fn does_not_recognize_a_quick_body_that_does_not_return() {
        assert_eq!(fast_path_in_the_kernel("not = ( false )"), None);
        assert_eq!(
            fast_path_in_the_kernel("ifTrue: trueBlock = ( trueBlock value )"),
            None
        );
    }

    /// A name that is not the method's own parameter -- here the enclosing
    /// kernel's slot -- is no parameter answer.
    #[test]
    fn does_not_recognize_a_name_that_is_not_a_parameter() {
        assert_eq!(fast_path_in_the_kernel("mirror = ( ^vmMirror )"), None);
    }

    /// Outside the kernel a quick body is an ordinary method, activated
    /// like any other.
    #[test]
    fn does_not_recognize_a_quick_method_outside_the_kernel() {
        assert_eq!(fast_path_outside_the_kernel("not = ( ^false )"), None);
        assert_eq!(fast_path_outside_the_kernel("& other = ( ^other )"), None);
        assert_eq!(
            fast_path_outside_the_kernel("ifTrue: trueBlock = ( ^trueBlock value )"),
            None
        );
    }
```

Add `use crate::mixin::QuickConstant;` to the test module's imports. Also
update the helper `recognized_in_the_kernel`'s match, which lists every
variant: add

```rust
            Some(other) => panic!("a mirror forwarder was recognized as {other:?}"),
```

in place of the `BuiltinClass` arm's specific message, keeping `None => None`.
(`MethodFastPath` derives `Debug`; check `mixin.rs` and add
`#[derive(Debug, Clone, Copy, PartialEq, Eq)]` to `QuickConstant` as shown.)

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p ns-runtime primitive_method`
Expected: compile error -- `QuickConstant` and the new variants do not exist.

- [ ] **Step 4: Add the variants**

In `crates/ns-runtime/src/mixin.rs`, replace the enum at lines 128-135 with
the `MethodFastPath` and `QuickConstant` from **Interfaces** above, and add
below it:

```rust
impl QuickConstant {
    /// The value a quick method answering this constant answers.
    pub(crate) fn value<'gc>(self) -> crate::types::Value<'gc> {
        match self {
            QuickConstant::True => crate::types::Value::Boolean(true),
            QuickConstant::False => crate::types::Value::Boolean(false),
            QuickConstant::Nil => crate::types::Value::Nil,
        }
    }
}
```

Keep whatever derives the existing enum carries.

- [ ] **Step 5: Recognize the three shapes**

In `crates/ns-runtime/src/compiler/primitive_method.rs`, extend `recognize`
after the builtin-class accessor:

```rust
    if !kernel {
        return None;
    }
    recognize_builtin_class_accessor(declaration, bindings)
        .map(MethodFastPath::BuiltinClass)
        .or_else(|| recognize_quick_method(declaration, bindings))
```

and add:

```rust
/// A quick method: a kernel method whose whole body is `^true`, `^false`,
/// `^nil`, `^<parameter>` or `^<parameter> value` runs without an
/// activation, as Squeak's and Cog's quick methods do. `True` and `False`
/// are written this way (`docs/design/core-library.md`, "`True` and `False`
/// are quick methods"): an activation of `True>>ifTrue:` would count
/// against the depth limit under the reference lowering, which sends the
/// conditional, and not under the optimized one, which inlines it
/// (`docs/design/compiler.md`, "How the limit counts").
///
/// Not switched off by `NS_NEXT_NO_PRIMITIVE_METHODS`: that switch measures
/// the mirror leaves' fast path, and a quick method runs no leaf.
fn recognize_quick_method(
    declaration: &ast::MethodDeclaration,
    bindings: &HashMap<Span, Binding>,
) -> Option<MethodFastPath> {
    let body = &declaration.body;
    if !body.temporaries.is_empty() || body.statements.len() != 1 {
        return None;
    }
    let ast::Statement::Return(expression) = &body.statements[0] else {
        return None;
    };
    let parameters = parameter_names(&declaration.pattern);
    match expression {
        ast::Expression::Literal(ast::Literal::Boolean { value: true, .. }) => {
            Some(MethodFastPath::Constant(QuickConstant::True))
        }
        ast::Expression::Literal(ast::Literal::Boolean { value: false, .. }) => {
            Some(MethodFastPath::Constant(QuickConstant::False))
        }
        ast::Expression::Literal(ast::Literal::Nil { .. }) => {
            Some(MethodFastPath::Constant(QuickConstant::Nil))
        }
        ast::Expression::Identifier(identifier) => {
            parameter_position(identifier, &parameters, bindings).map(MethodFastPath::Parameter)
        }
        ast::Expression::Send(send) => {
            let is_value = matches!(&send.message.selector, ast::Selector::Unary(name) if name == "value");
            let ast::Expression::Identifier(identifier) = &send.receiver else {
                return None;
            };
            if !is_value {
                return None;
            }
            parameter_position(identifier, &parameters, bindings)
                .map(MethodFastPath::ValueOfParameter)
        }
        _ => None,
    }
}

/// The position of the parameter `identifier` names, when the resolver bound
/// it to that very parameter -- the binding, not the name alone, says it is
/// not something that shadows it.
fn parameter_position(
    identifier: &ast::Identifier,
    parameters: &[&str],
    bindings: &HashMap<Span, Binding>,
) -> Option<u8> {
    let position = parameters.iter().position(|name| *name == identifier.name)?;
    let bound_to_it = matches!(
        bindings.get(&identifier.span),
        Some(Binding::Local(local)) if usize::from(*local) == position
    );
    if !bound_to_it {
        return None;
    }
    u8::try_from(position).ok()
}
```

Check against the actual AST: `ast::Selector::Unary` carries the name as the
accessor recognizer uses it (`let ast::Selector::Unary(name) = &send.message.selector`);
if its payload is not a `String`, compare the way that recognizer does. Update
the module doc comment at the top of the file with one paragraph naming
quick methods. Update `recognize`'s doc comment to list the third kind.
Import `crate::mixin::QuickConstant`.

- [ ] **Step 6: Run the recognizer tests**

Run: `cargo test -p ns-runtime primitive_method`
Expected: PASS. (`resolve.rs` does not compile yet if its `match` on
`method.fast_path` is exhaustive -- if so, do Step 7 first and run both.)

- [ ] **Step 7: Run the quick paths in `act`**

In `crates/ns-runtime/src/interpreter/resolve.rs`, `act_with_base`, extend the
`match method.fast_path`:

```rust
                        Some(MethodFastPath::Constant(constant)) => {
                            return Ok(Step::Value(constant.value()));
                        }
                        // Read with `get` rather than indexed: the lookup
                        // matched the selector, so the count is right for
                        // every kernel method, and a missing argument then
                        // activates the method rather than panicking.
                        Some(MethodFastPath::Parameter(position)) => {
                            if let Some(argument) = arguments.get(usize::from(position)) {
                                return Ok(Step::Value(*argument));
                            }
                        }
                        // The step the Boolean leaves used to answer: the
                        // block, activated as control flow, so that
                        // `push_step` exempts it for a `SendInlinable`
                        // exactly as before. A non-closure activates the
                        // method, whose `value` send answers what any other
                        // send of `value` to it would.
                        Some(MethodFastPath::ValueOfParameter(position)) => {
                            if let Some(Value::Object(object)) =
                                arguments.get(usize::from(position)).copied()
                            {
                                if self.heap().is_closure(object) {
                                    return Ok(Step::Block {
                                        closure: object,
                                        arguments: StepArguments::ControlFlow,
                                        then: Continuation::Deliver,
                                    });
                                }
                            }
                        }
```

Import `MethodFastPath`'s module path as the file already does; add
`Continuation`/`StepArguments` imports if missing (`crate::types::Continuation`,
`crate::interpreter::StepArguments`). If `self.heap()` is not the accessor
`Mutator` offers in this file, use the one `builtins/boolean.rs`'s
`block_step` uses (`mutator.heap().is_closure(object)`).

In `act_handler_method`, extend the comment above `if let Some(MethodFastPath::Mirror(..))`:
a quick method, like a builtin-class accessor, is never a `doesNotUnderstand:`
handler in the kernel; one arriving there is activated like any method.

In `crates/ns-runtime/src/interpreter/mod.rs`, `StepArguments::ControlFlow`'s
comment: "Only those natives build it" becomes "Only those natives and the
`^parameter value` quick method (`MethodFastPath::ValueOfParameter`) build
it" -- `True`'s and `False`'s conditionals, and-s and or-s, once Task 3 lands.

- [ ] **Step 8: Run the per-commit gate**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p ns-cli -- parse-corpus corpus/modules
```
Expected: warning-free, all pass, "parsed 210, failed 0, expected-fail 1, stale 0".
Nothing in `Kernel.ns` has a quick body yet except possibly an existing
method: if `every_kernel_method_is_a_primitive_method_named_after_its_row`
(`interpreter/kernel.rs`) now fails because an existing kernel method became
quick, report which one -- do not add it to a list without checking why.

- [ ] **Step 9: Commit**

```bash
git add crates/ns-runtime/src/mixin.rs crates/ns-runtime/src/compiler/primitive_method.rs crates/ns-runtime/src/interpreter/resolve.rs crates/ns-runtime/src/interpreter/mod.rs
git commit -m "feat: run the kernel's quick methods without an activation"
```
(with the trailer from Global Constraints)

---

### Task 2: `and:`, `or:`, `&` and `|` stop coercing

This task changes behaviour on the existing leaves and the compiler together,
so both lowerings move at once. `Kernel.ns` is untouched.

**Files:**
- Modify: `crates/ns-runtime/src/builtins/boolean.rs`
- Modify: `crates/ns-runtime/src/types.rs:426-430` (`Continuation::Truthy`)
- Modify: `crates/ns-runtime/src/interpreter/mod.rs:1064-1066` (`resume`)
- Modify: `crates/ns-runtime/src/trace.rs:153`
- Modify: `crates/ns-runtime/src/compiler/lower.rs:1470-1497` (`emit_coerce_to_boolean`), `:1712-1750` (`lower_and_or`), `:4170-4196` (test)
- Modify: `crates/ns-runtime/tests/trampoline.rs:458-465`, `:557-590`
- Rename+modify: `goldens/language/and_or_short_circuit_and_coerce.{ns,toml}` → `goldens/language/and_or_short_circuit.{ns,toml}`
- Rename+modify: `goldens/language/variable_and_block_coerces_its_value_to_a_boolean.{ns,toml}` → `goldens/language/and_or_answer_their_arguments_value.{ns,toml}`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `Continuation` without `Truthy`; `lower_and_or` without
  coercion. Task 3 deletes `builtins/boolean.rs` entirely.

- [ ] **Step 1: Change the goldens first (the new expectation)**

Why the new output is right: decision 2 -- every prior-art kernel's
`True>>and:` is `^alternative value` and `False>>or:` is `^alternative value`,
`True>>&` is `^alternative` and `False>>|` is `^alternative`.

```bash
git mv goldens/language/and_or_short_circuit_and_coerce.ns goldens/language/and_or_short_circuit.ns
git mv goldens/language/and_or_short_circuit_and_coerce.toml goldens/language/and_or_short_circuit.toml
git mv goldens/language/variable_and_block_coerces_its_value_to_a_boolean.ns goldens/language/and_or_answer_their_arguments_value.ns
git mv goldens/language/variable_and_block_coerces_its_value_to_a_boolean.toml goldens/language/and_or_answer_their_arguments_value.toml
```

`goldens/language/and_or_short_circuit.ns`: replace the comment with

```
(* Only the non-short-circuited arms run, and the evaluated arm's value is
   the answer as it is, a non-Boolean included, as in every prior-art
   kernel. *)
```

and in `.toml` change the fifth line of `stdout` from `false` to `3`.

`goldens/language/and_or_answer_their_arguments_value.ns`, whole file:

```
Newspeak3
'T'
(* A block in a variable takes the send, not the inlined form: `and:` and
   `or:` answer the block's value as it is, and `&` and `|` answer their
   argument itself on the side that does not decide alone. *)
class AndVar usingPlatform: platform = Object new (
) (
    public main: p args: a = (
        | three yes |
        three:: [3].
        yes:: [true].
        p transcript show: (true and: three) printString; newLine.
        p transcript show: (true and: yes) printString; newLine.
        p transcript show: (false or: three) printString; newLine.
        p transcript show: (true & 3) printString; newLine.
        p transcript show: (false | 3) printString; newLine.
        p transcript show: (false & 3) printString; newLine.
        p transcript show: (true | 3) printString; newLine
    )
) : ()
```

`.toml`:

```
stdout = '''
3
true
3
3
3
false
true
'''
```

- [ ] **Step 2: Run the goldens to see them fail**

Run: `cargo test -p ns-cli --test run_goldens`
Expected: FAIL on both renamed goldens (`false` where `3` is expected).

- [ ] **Step 3: Drop the coercion from the leaves**

In `crates/ns-runtime/src/builtins/boolean.rs`: `And` and `Or` pass
`Continuation::Deliver` instead of `Continuation::Truthy`; the comment above
them keeps the short-circuit sentence and drops the coercion one. In
`block_step`, delete the `Continuation::Truthy` arm; delete `truthy` and its
doc comment. Delete `Continuation::Truthy` from `types.rs`, its arm in
`interpreter/mod.rs`'s `resume`, and from `trace.rs`'s
`Continuation::Deliver | Continuation::Truthy => {}` (leaving
`Continuation::Deliver => {}`). Update the `Continuation` doc comment in
`types.rs` if it names `and:`/`or:` as a continuation's user.

- [ ] **Step 4: Drop the coercion from the optimized lowering**

In `crates/ns-runtime/src/compiler/lower.rs`, delete `emit_coerce_to_boolean`
and make `lower_and_or`'s evaluating arm the block body itself:

```rust
    /// `and:`/`or:`: the short-circuit arm is the receiver's own Boolean
    /// value, the other arm the block's value as it is. A branch's value is
    /// only moved into the send's own result, so `use_` -- what reads that
    /// result -- is what reads the block's value, as in `lower_if`.
    fn lower_and_or(
        &mut self,
        send: &ast::Send,
        block: &ast::BlockExpression,
        is_and: bool,
        use_: ValueUse,
    ) -> u16 {
        let guard_register = self.lower_expression(&send.receiver);
        let destination = self.allocate_scratch();
        let evaluate = |lowerer: &mut Self| lowerer.lower_inline_block_body(block, use_);
        let fallback =
            |lowerer: &mut Self| lowerer.lower_fallback_send(guard_register, &send.message);
        if is_and {
            self.emit_guarded_branch(guard_register, destination, evaluate, |_| guard_register, fallback);
        } else {
            self.emit_guarded_branch(guard_register, destination, |_| guard_register, evaluate, fallback);
        }
        destination
    }
```

Replace the test `and_or_coerce_the_block_value_and_short_circuit` with:

```rust
    /// The evaluated arm moves the block's value into the result as it is:
    /// no literal `true` or `false` is loaded to coerce it.
    #[test]
    fn and_or_answer_the_block_value_and_short_circuit() {
        for source in ["x and: [x]", "x or: [x]"] {
            let code = compile_foo(source);
            assert!(!has(&code, |instruction| matches!(
                instruction,
                Instruction::LoadLiteral {
                    literal: BytecodeLiteral::Boolean(_),
                    ..
                }
            )), "{source}");
        }
        let code = compile_foo("x and: [x]");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "and:"
        )));
        let code = compile_foo("x or: [x]");
        assert!(has(&code, |instruction| matches!(
            instruction,
            Instruction::SendPublic { selector, .. } if selector.as_str() == "or:"
        )));
    }
```

- [ ] **Step 5: Update `tests/trampoline.rs`**

In the doc comment at `:458-465`, drop `Truthy` from the list of
continuations (`a loop's WhileCondition and WhileBody` remain) and keep the
`and:` sends as block activations at the floor. At `:557`, replace
`// Truthy: only exactly true is true.` with
`// and:/or: answer the block's value as it is.` and change the first
assertion to expect the block's value (`three` holds `[3]`):

```rust
        assert!(
            interpreter.enter(|mutator| matches!(mutator.fetch(&value), Value::SmallInteger(3))),
            "{}",
            what("and:")
        );
```

Read the rest of that block and adjust any other assertion that expected a
coerced `false` from a non-Boolean block value the same way; an assertion on
a Boolean-valued block is unchanged.

- [ ] **Step 6: Run the per-commit gate**

Same four commands as Task 1 Step 8. Expected: all pass, including both
renamed goldens under both lowerings and the build/load round trips. Any
other golden that now fails is a place that relied on coercion: stop and
report it rather than updating it.

- [ ] **Step 7: Commit**

```bash
git add -A crates goldens
git commit -m "feat!: answer and:/or:'s argument value as it is, as every kernel does"
```

---

### Task 3: `True` and `False` in Newspeak

**Files:**
- Modify: `crates/ns-runtime/core-library/Kernel.ns:128-144`
- Delete: `crates/ns-runtime/src/builtins/boolean.rs`
- Modify: `crates/ns-runtime/src/builtins/mod.rs` (the `boolean` module and its `execute` arm)
- Modify: `crates/ns-runtime/src/builtins/primitive.rs` (`Primitive::Boolean`, `BooleanPrimitive`, the eleven `boolean…` rows of `MIRROR`, the "rows 56-60" comment)
- Modify: `crates/ns-runtime/src/interpreter/kernel.rs:504-515` (`check_mirror_send_arguments`'s comment), `:659-677` (`NOT_FORWARDERS`), `:700-770` (the table test)
- Modify: `crates/ns-runtime/src/compiler/lower.rs:1573-1575` (`lower_if`'s comment)
- Create: `goldens/kernel/recursion_through_and_or_counts_alike_under_both_lowerings.{ns,toml}`
- Create: `goldens/language/a_short_circuited_conditional_ignores_its_argument.{ns,toml}`
- Create: `goldens/errors/a_conditional_sends_value_to_its_argument.{ns,toml}`
- Create: `goldens/kernel/a_subclass_of_true_inherits_its_methods.{ns,toml}`

**Interfaces:**
- Consumes: `MethodFastPath::{Constant, Parameter, ValueOfParameter}`,
  `QuickConstant` (Task 1); `Continuation` without `Truthy` (Task 2).

- [ ] **Step 1: Write the depth-agreement golden (passes before and after)**

It passes on today's leaves too; its job is to fail if any `True`/`False`
method is left unrecognized, since the reference lowering would then count
an activation per level more than the optimized one.

`goldens/kernel/recursion_through_and_or_counts_alike_under_both_lowerings.ns`:

```
Newspeak3
'Kernel'
(* Every level of this recursion goes through and:, or: and a one-armed
   ifTrue: with literal blocks -- inlined by the optimized lowering, a
   SendInlinable under the reference lowering, which reaches True's and
   False's methods. Those are quick methods, run without an activation, so
   both lowerings count one activation per level and print the same
   progress before the same overflow (`docs/design/compiler.md`, "How the
   limit counts"). *)
class DeepThroughTheBooleans usingPlatform: platform = Object new (
    | transcript = platform transcript. |
) (
    down: depth = (
        depth = (depth // 20000 * 20000)
            ifTrue: [transcript show: depth printString; newLine].
        ^(depth >= 0) and: [(depth < 0) or: [down: depth + 1]]
    )
    public main: p args: a = ( down: 0 )
) : ()
```

`.toml`:

```
status = 1
stdout = '''
0
20000
40000
60000
80000
'''
stderr = '''
Uncaught Newspeak error: stack overflow: recursion too deep
'''
```

Run: `cargo test -p ns-cli --test run_goldens`
Expected: PASS on the current tree. If it does not, the recorded progress is
wrong, not the runtime: run
`cargo run -p ns-cli -- run goldens/kernel/recursion_through_and_or_counts_alike_under_both_lowerings.ns`
and compare with `goldens/kernel/recursion_through_the_core_library_counts_alike_under_both_lowerings.toml`,
whose recursion counts the same one activation per level; report before
changing anything.

- [ ] **Step 2: Write the new-behaviour goldens (fail before)**

`goldens/language/a_short_circuited_conditional_ignores_its_argument.ns`:

```
Newspeak3
'T'
(* A side that decides alone answers without looking at its argument, so a
   non-block there is never sent value, as in Primordial Soup's kernel. *)
class ShortCircuit usingPlatform: platform = Object new (
) (
    public main: p args: a = (
        | three |
        three:: 3.
        p transcript show: (false ifTrue: three) printString; newLine.
        p transcript show: (true ifFalse: three) printString; newLine.
        p transcript show: (false and: three) printString; newLine.
        p transcript show: (true or: three) printString; newLine
    )
) : ()
```

`.toml`:

```
stdout = '''
nil
nil
false
true
'''
```

`goldens/errors/a_conditional_sends_value_to_its_argument.ns`:

```
Newspeak3
'T'
(* True>>ifTrue: is ^trueBlock value: an argument that is no block is sent
   value like any other object, and no Object>>value answers it, as in
   Primordial Soup's kernel. *)
class NotABlock usingPlatform: platform = Object new (
) (
    public main: p args: a = (
        | three |
        three:: 3.
        ^true ifTrue: three
    )
) : ()
```

`.toml`:

```
status = 1
stderr = '''
Uncaught Newspeak error: an object does not understand #value
'''
```

`goldens/kernel/a_subclass_of_true_inherits_its_methods.ns`:

```
Newspeak3
'Kernel'
(* True's methods are Newspeak and need no payload, so a user subclass
   inherits them: its instance runs the true branch and prints as true,
   where Boolean itself declares only hash and an instance of a subclass of
   it falls to Object's methods
   (`goldens/kernel/a_subclass_of_a_builtin_falls_to_objects_methods`).

   The module is a value because §3.1.1 asks a value class's enclosing
   object to be one; the transcript lives in `Script`, which is no value. *)
class Subclassed usingPlatform: platformAtCreation = Object <: Value () (
    class MyTrue = True new () ()
    class Script on: aTranscript = Object new (
        | transcript = aTranscript. |
    ) (
        public run = (
            | instance |
            instance:: MyTrue new.
            transcript show: (instance ifTrue: ['taken'] ifFalse: ['not taken']); newLine.
            transcript show: instance printString; newLine.
            transcript show: instance not printString; newLine
        )
    )
    public main: platform args: arguments = (
        (Script on: platform transcript) run
    )
) : ()
```

`.toml`:

```
stdout = '''
taken
true
false
'''
```

(`instance ifTrue: [...] ifFalse: [...]` with literal blocks: the optimized
lowering's guard sees a non-Boolean and falls back to the real send; the
reference lowering sends it; both reach `True>>ifTrue:ifFalse:`.)

Run: `cargo test -p ns-cli --test run_goldens`
Expected: FAIL on `a_conditional_sends_value_to_its_argument` (today answers
3, status 0) and on `a_subclass_of_true_inherits_its_methods` (today the
leaves decline and the sends reach `Object`). `a_short_circuited_…` already
passes on today's leaves; it pins that the Newspeak methods keep it.

- [ ] **Step 3: Rewrite `Boolean`, `True` and `False` in `Kernel.ns`**

Replace lines 128-144 of `crates/ns-runtime/core-library/Kernel.ns` with:

```
    public class Boolean = Value new () (
        public hash = ( ^self ifTrue: [1] ifFalse: [0] )
    )
    (* §5.1.2: `true` is the unique instance of `True` and `false` of
       `False`. Each branch is a method of its own, as in every prior-art
       kernel (SOMns's, Primordial Soup's, Newspeak on Squeak's), so no
       method tests the receiver and none reaches the mirror. Every body is
       `^true`, `^false`, `^nil`, `^<parameter>` or `^<parameter> value`: a
       quick method, which runs without an activation, so that the
       reference lowering, which sends a conditional the optimized lowering
       inlines, counts no activation for it either
       (`docs/design/core-library.md`, "`True` and `False` are quick
       methods"). An argument that is no block is sent `value` like any
       other object; `printString` alone is an ordinary method. *)
    public class True = Boolean new () (
        public ifTrue: trueBlock ifFalse: falseBlock = ( ^trueBlock value )
        public ifFalse: falseBlock ifTrue: trueBlock = ( ^trueBlock value )
        public ifTrue: trueBlock = ( ^trueBlock value )
        public ifFalse: falseBlock = ( ^nil )
        public not = ( ^false )
        public & other = ( ^other )
        public and: alternative = ( ^alternative value )
        public | other = ( ^true )
        public or: alternative = ( ^true )
        public printString = ( ^'true' )
    )
    public class False = Boolean new () (
        public ifTrue: trueBlock ifFalse: falseBlock = ( ^falseBlock value )
        public ifFalse: falseBlock ifTrue: trueBlock = ( ^falseBlock value )
        public ifTrue: trueBlock = ( ^nil )
        public ifFalse: falseBlock = ( ^falseBlock value )
        public not = ( ^true )
        public & other = ( ^false )
        public and: alternative = ( ^false )
        public | other = ( ^other )
        public or: alternative = ( ^alternative value )
        public printString = ( ^'false' )
    )
```

Also update the file's header comment where it says "Every method reaches
Rust through `vmMirror`": every method that needs a leaf reaches Rust
through `vmMirror`, and nothing else does.

- [ ] **Step 4: Remove the Boolean leaves**

- Delete `crates/ns-runtime/src/builtins/boolean.rs` (`git rm`).
- `builtins/mod.rs`: remove `mod boolean;` (or `pub(crate) mod boolean;`) and
  the `Primitive::Boolean(primitive) => boolean::execute(...)` arm.
- `builtins/primitive.rs`: remove `Boolean(BooleanPrimitive)` from
  `Primitive`, the `BooleanPrimitive` enum, and the eleven `MIRROR` rows
  from `"boolean:ifTrue:ifFalse:ifFail:"` through
  `"booleanPrintString:ifFail:"`. The rows after them move up by eleven;
  recount the `Array`/`OrderedCollection` shared rows the comment above
  `MIRROR` names ("rows 56-60") by finding the index of
  `collection:do:ifFail:` and its neighbours, and correct the numbers.
- `interpreter/kernel.rs`, `check_mirror_send_arguments`'s doc comment: the
  example `(boolean:ifTrue:ifFail:'s, say)` names a row that no longer
  exists -- write "a block the leaf itself activates, as `whileTrue:`'s
  would be if it were passed as a literal".
- `compiler/lower.rs`, `lower_if`'s comment: "matching `builtins::boolean`'s
  one-armed arms" becomes "matching `True>>ifFalse:` and `False>>ifTrue:`,
  which answer `nil`".
- Search for leftovers and fix each:
  ```bash
  grep -rn "BooleanPrimitive\|Primitive::Boolean\|builtins::boolean\|boolean::\|booleanNot\|booleanPrintString\|boolean:ifTrue\|boolean:and\|boolean:or\|boolean:logical" crates
  ```
  Expected afterwards: no output.

- [ ] **Step 5: Teach the kernel table test about quick methods**

In `crates/ns-runtime/src/interpreter/kernel.rs`'s test module:

Add to `NOT_FORWARDERS`:

```rust
        // A string literal is no quick answer: an ordinary method.
        (BuiltinClass::True, "printString"),
        (BuiltinClass::False, "printString"),
```

Add a table beside it:

```rust
    /// `True`'s and `False`'s methods, each a quick method: a missed one
    /// would run correctly as an ordinary method and nothing else would
    /// notice, except that the reference lowering would count its
    /// activation against the depth limit and the optimized lowering would
    /// not.
    const QUICK_METHODS: &[(BuiltinClass, &str, MethodFastPath)] = &[
        (BuiltinClass::True, "ifTrue:ifFalse:", MethodFastPath::ValueOfParameter(0)),
        (BuiltinClass::True, "ifFalse:ifTrue:", MethodFastPath::ValueOfParameter(1)),
        (BuiltinClass::True, "ifTrue:", MethodFastPath::ValueOfParameter(0)),
        (BuiltinClass::True, "ifFalse:", MethodFastPath::Constant(QuickConstant::Nil)),
        (BuiltinClass::True, "not", MethodFastPath::Constant(QuickConstant::False)),
        (BuiltinClass::True, "&", MethodFastPath::Parameter(0)),
        (BuiltinClass::True, "and:", MethodFastPath::ValueOfParameter(0)),
        (BuiltinClass::True, "|", MethodFastPath::Constant(QuickConstant::True)),
        (BuiltinClass::True, "or:", MethodFastPath::Constant(QuickConstant::True)),
        (BuiltinClass::False, "ifTrue:ifFalse:", MethodFastPath::ValueOfParameter(1)),
        (BuiltinClass::False, "ifFalse:ifTrue:", MethodFastPath::ValueOfParameter(0)),
        (BuiltinClass::False, "ifTrue:", MethodFastPath::Constant(QuickConstant::Nil)),
        (BuiltinClass::False, "ifFalse:", MethodFastPath::ValueOfParameter(0)),
        (BuiltinClass::False, "not", MethodFastPath::Constant(QuickConstant::True)),
        (BuiltinClass::False, "&", MethodFastPath::Constant(QuickConstant::False)),
        (BuiltinClass::False, "and:", MethodFastPath::Constant(QuickConstant::False)),
        (BuiltinClass::False, "|", MethodFastPath::Parameter(0)),
        (BuiltinClass::False, "or:", MethodFastPath::ValueOfParameter(0)),
    ];
```

In `every_kernel_method_is_a_primitive_method_named_after_its_row`, before
the `OBJECT_CLASS_ACCESSORS` check:

```rust
                    if let Some((_, _, expected)) = QUICK_METHODS
                        .iter()
                        .find(|(owner, name, _)| *owner == kind && *name == selector)
                    {
                        assert_eq!(
                            method.fast_path,
                            Some(*expected),
                            "{kind:?}>>{selector} is not the quick method it should be"
                        );
                        continue;
                    }
```

Rename the test to `every_kernel_method_has_the_fast_path_it_should` and add a
sentence to its doc comment naming quick methods. Add a second test so the
table cannot go stale in the other direction:

```rust
    /// Every row of `QUICK_METHODS` names a method the kernel declares.
    #[test]
    fn every_quick_method_the_table_names_exists() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            for (kind, selector, _) in QUICK_METHODS {
                let class = mutator.builtin_class(*kind);
                let application = mutator
                    .heap()
                    .class_application(class)
                    .expect("a builtin class has an application");
                assert!(
                    application
                        .mixin
                        .compiled_method(crate::Symbol::intern(selector))
                        .is_some(),
                    "{kind:?} declares no {selector}"
                );
            }
        });
    }
```

Import `crate::mixin::{MethodFastPath, QuickConstant}` in the test module.
`MethodFastPath` must be `Copy` for `Some(*expected)`; if it is not, compare
with `method.fast_path.as_ref() == Some(expected)`.

- [ ] **Step 6: Run the per-commit gate**

Same four commands as Task 1 Step 8. Expected: all pass, including the four
new goldens under both lowerings and their snapshot and build/load round
trips, and `goldens/kernel/recursion_through_the_core_library_counts_alike_under_both_lowerings`
and `goldens/errors/if_block_that_takes_an_argument_is_a_wrong_arity_error`
unchanged. A failing existing golden is a behaviour change this plan did not
name: stop and report it.

- [ ] **Step 7: Commit**

```bash
git add -A crates goldens
git commit -m "feat: write True and False in Newspeak as quick methods"
```

---

### Task 4: Documentation

Markdown only: no test run (the gates cannot be affected).

**Files:**
- Modify: `docs/design/core-library.md`
- Modify: `docs/design/dispatch.md` ("Primitive methods", `:540` on)
- Modify: `docs/design/compiler.md` ("How the limit counts", `:897` on; `:885`)
- Modify: `docs/design/execution.md` (`:3575-3576`, and any other mention the grep below finds)
- Modify: `CLAUDE.md` ("A cache records what the lookup decided")
- Modify: `docs/PROGRESS.md`

- [ ] **Step 1: `core-library.md`**

The documents describe the current state, not the history.

- Add a section **"`True` and `False` are quick methods"** after "The shape
  of a kernel method": the three shapes, why (the depth count under the
  reference lowering, with the two-versus-zero example from
  `compiler.md`), prior art (Squeak/Cog quick methods; SOMns's and
  Primordial Soup's `True`/`False`), that they are recognized only in the
  kernel and are not switched off by `NS_NEXT_NO_PRIMITIVE_METHODS`, and the
  two semantic decisions: no coercion (every prior-art kernel), a non-block
  argument sent `value` with no `Object>>value` (Primordial Soup). Name the
  goldens: `language/and_or_short_circuit`,
  `language/and_or_answer_their_arguments_value`,
  `language/a_short_circuited_conditional_ignores_its_argument`,
  `errors/a_conditional_sends_value_to_its_argument`,
  `kernel/a_subclass_of_true_inherits_its_methods`,
  `kernel/recursion_through_and_or_counts_alike_under_both_lowerings`.
- Line ~84-96: the list of what forwards to leaves drops `Boolean`'s
  conditionals; "`Nil`, `True` and `False` declare nothing of their own"
  becomes "`Nil` declares nothing of its own"; the `Boolean` `hash` sentence
  stays.
- The table at ~497: the `ifTrue:…`/`and:`/`or:` row now reads "Lowered by
  the compiler for a literal block; otherwise `True`'s or `False`'s quick
  method, which answers the block as a trampoline step" and links the new
  section.
- ~644-646, "A literal block may appear in a mirror send only as its final
  `ifFail:` argument": replace the `boolean:ifTrue:ifFail:` example and the
  sentence "`Boolean>>ifTrue:` therefore passes its parameter" with the
  same rule stated for any leaf that activates a block (today `whileTrue:`,
  `whileFalse:`, `do:`, `value`).
- "Planned: real bodies for control flow and `do:`": the Boolean half is
  done; the section keeps `whileTrue:`, `whileFalse:` and `do:`, and of its
  four semantic decisions the first (non-block argument, coercion) is now
  settled for the Boolean methods -- say so and point at the new section;
  the other three stay open.
- "Decisions": add one entry, dated 2026-09-24, for the four decisions at
  the top of this plan, linking the plan.

- [ ] **Step 2: `dispatch.md`, "Primitive methods"**

Add a paragraph: quick methods are the third fast path `act` reads off the
method on every send, beside the mirror row and the builtin-class accessor;
what each answers; `ValueOfParameter` answers a control-flow block step for
a closure and activates the method otherwise.

- [ ] **Step 3: `compiler.md`, "How the limit counts"**

- The rule's list of leaves: `ifTrue:` … `or:` on a Boolean are no longer
  leaves but `True`'s and `False`'s `^parameter value` quick methods, which
  answer the same control-flow step; `whileTrue:`/`whileFalse:` stay leaves.
- "The agreement depends on primitive methods" becomes "on the kernel's
  fast paths": the reference lowering's `SendInlinable` of `ifTrue:`
  reaches `True>>ifTrue:`, a quick method, which is never activated.
  `NS_NEXT_NO_PRIMITIVE_METHODS=1` no longer touches the Boolean methods
  (they have no mirror row), but still breaks agreement for
  `whileTrue:`/`whileFalse:`; keep the "for measuring only" sentence.
- `:885`: drop `builtins::boolean::truthy for and: and or:` from the list of
  what a guard failure reproduces.

- [ ] **Step 4: `execution.md`, `CLAUDE.md`, sweep**

- `execution.md:3575-3576`: `Continuation::Truthy` no longer exists; describe
  `and:`/`or:` as answering the block's value through `Continuation::Deliver`.
- `CLAUDE.md`, "A cache records what the lookup decided": the sentence
  listing the fast paths (`MethodFastPath::Mirror`, `MethodFastPath::BuiltinClass`)
  gains the three quick-method variants, read off the method on every send
  like the others.
- Sweep:
  ```bash
  grep -rn "Truthy\|truthy\|builtins::boolean\|BooleanPrimitive\|boolean:ifTrue\|booleanNot\|booleanPrintString\|emit_coerce_to_boolean\|coerces its value\|and_or_short_circuit_and_coerce\|variable_and_block_coerces" docs/design docs/PROGRESS.md docs/references CLAUDE.md README.md
  ```
  Expected: no output (plans under `docs/superpowers/plans/` are execution
  records and stay as written).

- [ ] **Step 5: `PROGRESS.md`**

Add an entry "`True` and `False` in Newspeak" (2026-09-24) in the style of
its neighbours: what moved, the four decisions, the goldens that changed and
why, and the measurement from Task 5 (filled in after Task 5). "Where the
next session starts" keeps naming step 6's third plan; mention in one
sentence that this piece was put ahead of it and is done.

- [ ] **Step 6: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: record True and False as quick methods"
```

---

### Task 5: Staggered gates and measurement

- [ ] **Step 1: The end-of-batch gates** (Git Bash syntax)

```bash
NS_NEXT_STRESS_MOVES=1 cargo test --workspace
NS_NEXT_STRESS_CACHES=1 cargo test --workspace
NS_NEXT_STRESS_COLLECT=1 cargo test --workspace
cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
cargo build --workspace
```
Expected: every suite passes; every `run-corpus` reports
"ran 6, failed 0, skipped 205". Run each command in the foreground.

- [ ] **Step 2: Measure against `master`**

Performance has the highest priority, so the change is measured before it
is handed on, by `docs/design/execution.md`'s protocol ("What removing the
re-entries moved"): a before-binary from `master` built in a throwaway
directory, the branch's binary, both with the pinned flags
(PowerShell: `$env:RUSTFLAGS = "-C target-feature=+crt-static -C llvm-args=-align-all-functions=6"`,
`cargo build --release -p ns-cli`, `Remove-Item Env:RUSTFLAGS`), several
builds of each tree pooled for `bench_block.ns`, and
`ns-next bench <source> --iterations 30` over the four fixtures under
`goldens/bench/` plus one throwaway source that exercises the paths that
changed, five rounds, alternating the binaries:

`bench_boolean.ns` (throwaway, not committed):

```
Newspeak3
'Bench'
class BenchBoolean usingPlatform: platform = Object new (
) (
    public main: p args: a = (
        | count flag taken block |
        count:: 0.
        taken:: 0.
        flag:: true.
        block:: [taken:: taken + 1].
        [count < 200000] whileTrue: [
            flag:: flag not.
            flag ifTrue: block.
            (flag & true) ifTrue: block.
            (flag | false) ifFalse: block.
            count:: count + 1].
        p transcript show: taken printString; newLine
    )
) : ()
```

(`flag not`, `&`, `|` are real sends; `ifTrue: block` and `ifFalse: block`
take a variable, so they are real sends to the quick methods too.)

Expected: the four fixtures level within noise; `bench_boolean.ns` no slower
(the quick paths skip the mirror row's leaf dispatch). A slowdown beyond
noise is reported to the user with the numbers, not accepted silently.
Check with `dumpbin /disasm` on a release build with debug information that
no `vm::run::closure` became a function of its own (`CLAUDE.md`,
"Working in this repository").

- [ ] **Step 3: Record the measurement**

Add the numbers to `PROGRESS.md`'s entry from Task 4 Step 5 and, if the
section exists for this kind of result, to `execution.md`. Commit
(Markdown only, no gate):

```bash
git add docs
git commit -m "docs: record what True and False as quick methods cost"
```
