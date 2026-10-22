# `new` as compiled code — design

Temporary: the batch that builds this moves its content into the living
documents (`object-model.md`, `dispatch.md`, `compiler.md`,
`execution.md`, `image-format.md`) and deletes this file.

## Intent

Instantiation is the one piece of ordinary Newspeak semantics that still
runs as a Rust state machine instead of code the compiler emits: the
instantiation walk in `interpreter/instantiate.rs` (`Instantiation`,
`InstantiationPhase`, `DeferredMessage`, `Continuation::Instantiate`,
`Step::Initializer`/`InitializerThen`/`ClauseMessage`), with road step 6's
fast `new` beside it. The factory and the initializers become compiled code, as in the references (Squeak-Newspeak
`Newspeak2SqueakCompilation.ns:1991-2025`, `:1899-1921`; SOMns
`MixinBuilder.assemblePrimaryFactoryMethod`, `MixinBuilder.java:547-600`;
Primordial Soup `Newspeak2PrimordialSoupCompilation.ns:1907-1961`).

Why, decided with the user on 2026-10-22, in this order of priority:

1. **Performance.** `Foo new` becomes two ordinary sends to compiled
   bodies, so it takes the inline caches, and road steps 8 and 9 can
   compile and inline it like any other code. A Rust walk is opaque to a
   JIT.
2. **Uniformity.** Instantiation looks like the references': a factory
   that allocates and invokes the instance initializer, and per-level
   initializers that invoke the superclass's first.

## Goals

- The primary factory of every class -- declared, synthesized, builtin,
  decoded from an image -- is a compiled body on the class side under the
  factory selector, reached by ordinary dispatch and the inline caches.
- Each level's instance initialization is one compiled body on its mixin
  that does all of §6.3 for that level, superclass first, with no Rust
  walk between levels.
- The nested-class accessor's cache hit is a guard and one field load.
- Every observable behaviour stays as it is, every golden unchanged, with
  the exceptions decided below: a `<:` level reached from outside
  (decision 4), a chain deeper than the depth limit (decision 6), and the
  declaration without a body, whose spec deviation this batch fixes
  (decision 8).

## Non-goals

- No JIT work. Inlining the factory into its caller is road steps 8 and 9.
- The control-flow loops (`whileTrue:`, `whileFalse:`, `do:`) stay as they
  are; their real bodies are planned between steps 7 and 8.
- The futures of simultaneous slots (`Continuation::Future`) stay in Rust;
  the batch adds an `OPEN-WORK.md` item for them (decided with the user).
- No scalarization of value objects; the batch adds an `OPEN-WORK.md`
  item at road step 9 (decided with the user): scalar replacement of
  values in the optimizing JIT -- permitted because a value's `==` is its
  `=` and its `identityHash` its `hash` (`Kernel.ns`, §3.1.1), and made
  possible by this batch, which turns allocation and initialization into
  code the JIT can inline -- with its open questions: a value's lazy slot
  (each copy would compute it), `self` escaping a value's initializer
  before its slots are filled, whether the bulk forward may take a value,
  and flattening values into other objects and arrays, which without
  static slot types needs speculative layouts.
- Slot accessors stay bodiless entries (Cog's quick methods); primitives
  stay VM leaves; the unwinder stays as it is.

## Anti-goals

- **No initializer reachable as a message.** The references name the
  initializer ``initializer`sel:`` and put it in the method table, where
  `perform:` with a constructed symbol, or a mirror, reaches it -- a
  program could re-run a finished object's initializer, a value's
  included. Here an initializer is a body on its mixin outside the method
  table, reached only by the instructions below.
- **No silent §6.1 deviation.** A super send of a mangled initializer
  selector walks on past a superclass whose factory has another selector
  and runs some higher initializer, leaving the skipped level's slots nil
  without an error (what Squeak-Newspeak does). Every mismatch stays the
  error it is today, with today's text, at today's point.
- **No error texts in `vm.rs` or `compiler/`.** The new instructions call
  into `interpreter/`, as every send does (`CLAUDE.md`, "`vm.rs` holds no
  dispatch logic").

## Constraints

Everything in `CLAUDE.md`'s invariants applies; the ones this work touches
most:

- **§6.1's check stays at run time, at today's point.** A superclass
  clause's selector is compared with the superclass's primary factory
  selector when the level is initialized, after the clause's arguments
  were evaluated (§6.3: the message is evaluated in *i*, then the
  superclass initializer is invoked with it). Precomputing a verdict per
  application would skip the argument expressions' side effects.
  `<:` clauses keep today's order: their selectors are checked before the
  superclass clause's message runs (`mixin_clause_naming_the_wrong_factory_fails`).
- **The per-level arity check stays**, with its text: an untrusted image
  can pair a body with a level whose factory has another arity
  (`check_factory_arity`, `check_message_arity`).
- **`<:` messages run with the applying class's factory arguments, just
  before the initializer of the level they add**
  (`mixin_initializer_order.ns`).
- **`InitializeSlot`'s `own_index` stays mixin-relative**, resolved against
  the slot base of the application the activation was entered at, and
  every own-slot access checks its recorded guard (`object-model.md`, "What
  a slot access checks"). A level's initializer is entered at that level's
  class object, as today.
- **The refusals stay where an instance would come into being**:
  `instantiation_refusal`, the class object's value refusal, the builtin
  refusals (the VM mirror's class -- lock 5 of "The VM mirror is a
  capability" --, `Symbol`, `Mixin`), with today's texts and selectors.
- **Compiling stays total.** The new bodies are emitted for every mixin and
  never raise; every program-level error is the instructions' at run
  time.
- **An image is untrusted input.** The factory body is never on the wire:
  it is derived on load from the row's factory, as the factory entry is
  today (`image/mixins.rs`, `metaclass_factory_entries`), so an image
  cannot forge one. Every new instruction validates what an image can
  make disagree (arity, selector, a missing body) at run time, never by
  `panic!`.
- **A wire tag is assigned once.** New opcodes take the next free numbers
  (`opcodes.rs`, next free: 41); the frame kinds that disappear
  (`FRAME_SUPERCLASS_MESSAGE`, `FRAME_MIXIN_APPLICATION_MESSAGE`, and
  `FRAME_SUPERCLASS_CLAUSE` if the clause stops being a body of its own)
  retire their numbers. New `CodeKind`s take fresh `code_kind_tag`s.
- **A cache records what the lookup decided.** A new per-site cache (the
  superclass initializer's target) is keyed by what decides it -- the
  application the initializer runs in -- and acts on a resolution, never
  decides; the reference lowering runs it uncached and is the oracle.
- **Mirrors list no factory, initializer or accessor** (`MirrorsFactory.ns`
  `methods`, `builtins/reflection.rs`'s `declared_methods`).
- **The MirrorBuilder and install** rebuild the new bodies with every mixin
  they rebuild; `reapply` already recomputes per-application facts.

## Decided with the user (2026-10-22)

1. **The factory is compiled code with an activation of its own.** Its
   body, in the shape of Squeak's (`| o | o := self basicNew.
   o initializer`…: args. ^o`):
   - an allocating instruction on the receiver (the class object): the
     refusals, the builtin cases of today's `decide_factory` (`Array`,
     `ByteArray`, `String`, `Ephemeron` and the three refusals), then
     `allocate_instance_with` and the application's header;
   - the direct invocation of the receiver's own level's initializer on
     the new instance with the factory's arguments;
   - the return of the instance (the initializer's own answer is not
     used).
2. **Each level's initializer is a body on its mixin, outside the method
   table, invoked by dedicated instructions -- not a super send of a
   hidden selector.** The cost is the same as a super send (a per-site
   cache keyed by the application, a guard, a frame push), the target is
   as static for a JIT, and it keeps the anti-goals above.
3. **A class without initializer code costs one factory activation until
   the JIT inlines it.** Road step 6's fast `new` (zero activations) goes;
   a slowdown there is accepted until road step 8. Classes with slots are
   expected to get faster (no walk). Measured, not assumed.
4. **A `<:` level reached from outside is an ordinary class of its
   mixin.** A program reaches a level a `<:` clause added only through a
   mirror (`(ClassMirror reflecting: C) superclass reflectee`). Instantiated
   directly, or named as the superclass of another class, it initializes
   as a class whose mixin is the applied mixin and whose superclass is the
   level's actual superclass: the mixin's *own* superclass clause, checked
   against that superclass by §6.1. Only the initializer of the
   declaration that applied it runs it the `<:` way (its message, then
   the mixin's own part). Today the direct case already behaves so; the
   superclass case changes (today the walk treats the level as
   transparent, evaluates the applying declaration's `<:` message with
   the subclass's arguments and sends the subclass's clause to the level
   below). Probe (scratch, not in the tree): for `class Below y: y = L
   tag: …` over `L` = `Marker` applied to `Base k:`, today's error is
   "the superclass clause of class Below sends #tag:, but the primary
   factory of class Base is #k:"; afterwards `Below`'s clause message is
   evaluated and the error is "the superclass clause of class Marker sends
   #new, but the primary factory of class Base is #k:". No golden pins
   either. When the applied mixin's own declaration writes `<:` clauses
   of its own (`class M2 = Object <: M1`, applied in `class C = Object <:
   M2`), the level reached from outside runs its mixin's own superclass
   clause against its actual superclass and then the mixin's own part --
   never the mixin's own `<:` clauses, whose levels are not above it
   (`mixin_applied_mixins_own_clauses_are_not_applied.ns`); my call, the
   builder check having raised it. Today's walk treats a `<:` level
   directly above the instantiated level as part of it, so the direct
   case changes too when levels are stacked; the golden covers both the
   stacked and the single case. Prior art: Squeak-Newspeak deletes such a level's initializer
   (`KernelForSqueak.ns:968-985`), so both cases end in a
   `doesNotUnderstand:` (or silently run a higher initializer); SOMns
   flattens the levels into one class, so none is reachable; Primordial
   Soup has no `<:`. A golden pins the new behaviour of both cases.
5. **The nested-class accessor stays bodiless, and its cache entry
   holds the absolute index of the hidden slot.** Revised with the user
   the same day: a filled accessor answers today without an activation,
   like a slot getter -- a quick method in Cog's sense, which both JITs
   must special-case anyway for the slot accessors, which stay bodiless.
   A compiled body would cost the interpreter an activation per `Foo` and
   make no JIT tier faster (the optimizing JIT inlines either form to the
   same code; the baseline JIT emits the guarded load at the site). What
   changes is the hit path's cost: `Target::NestedClass` records the
   absolute field index of the receiver application's hidden slot
   (`MixinApplication::nested_slot`), as `Target::Slot` records a slot's,
   so a hit is the guard and one field load, without today's
   `known_mixin_id` and `nested_slot` hash lookups (`nested::cache_slot`).
   The guard -- the receiver's application -- decides the index, so the
   entry records what the lookup decided. A filled slot answers the
   class; an empty one, or one marked in progress, takes today's path
   (`nested::nested_class`: the marker, the cyclic-definition error, the
   superclass clause's activation and `Continuation::NestedClass`,
   `after_clause`), which runs once per enclosing object. The reference
   lowering runs it uncached, as every send.
6. **A level's initializer invokes its superclass's from inside its own
   activation, so a chain nests one counted activation per level with
   code.** A chain of more such levels than the depth limit (100,000 plus
   the 1,000 reserve) now fails with the ordinary stack-overflow error,
   where today's iterative walk (`execution.md`, "A superclass chain is
   walked iteratively") succeeds; accepted as a behaviour change, pinned by
   a golden and documented. Levels without code are passed over by the
   superclass invocation in a loop, never by recursion, so they neither
   nest nor escape the count. A flat chain would need a Rust driver -- the
   walk this batch removes.
7. **The measurements are a record with a tolerance, not a strict gate.**
   A small deviation on classes with slots (`bench_new*`) or the Are We
   Fast Yet benchmarks is accepted and recorded in `execution.md`; a
   large one stops the batch before the merge and goes to the user, with
   the numbers, before anything is optimized or merged.

8. **A declaration without a body is the mixin application itself -- a
   spec deviation this batch fixes.** For `class M3 x: x = S ms <: M1 m1
   <: … <: Mn mn.` (a `.` instead of a class body), §6 says "the mixin
   can be specified along with the superclass via a mixin application",
   and §6.2 makes the result "a class I with mixin M". Today ns-next
   builds an empty level of the declaration's own on top of the applied
   levels, so `M3`'s mixin is empty and `class U = Base new <: M3 new.`
   gains nothing of `M1` (probe, scratch: `UsesM3 new a` is not
   understood). Squeak-Newspeak's accessor applies the last mixin itself,
   named for the declaration (`Newspeak2SqueakCompilation.ns:1931-1955`).
   Decided:
   - `M3`'s class object is the level `Mn` adds, with no level above it:
     its mixin is `Mn`'s, its superclass the levels below, its name `M3`,
     its enclosing object and declaring class `Mn`'s (§3.3), as every
     `<:` level's are.
   - Its primary factory is the declaration's signature (`x:`; `new` when
     none is written), not `mn`'s selector: the declaration may write a
     signature, and its parameters are in scope "throughout the mixin
     application" (§6); Squeak copies the declaration's factory onto the
     application too. It has no class side of its own (there is no body)
     and inherits none of `Mn`'s (flat metaclasses).
   - `M3 x: 1` runs the declaration's initializer: steps 1-4 below, with
     `Mn`'s own part run at `M3`'s own level, and no step 5.
   - `class U = S <: M3 x: 5 (…)` elsewhere: §6.1 checks `x:` against
     `M3`'s factory; the level added has `Mn`'s mixin, and its own part
     runs "with the arguments of μm" (§6.2), its arity checked against
     `Mn`'s factory with `check_message_arity`'s text. `M3`'s own
     clauses (`ms`, the `mk`) never run there, as a declaration's own
     clauses never run where its mixin is applied (decision 4).
   - The existing goldens keep their outcomes
     (`mixin_body_less_application.ns` among them); a new golden pins
     applying a body-less declaration's mixin, `M3`'s factory, and the
     arity check of the last point.

## The shape of a level's initializer

Proposed by me, presented to the user, not separately confirmed; it
follows from decisions 2 and 4 and from today's order.

For a declaration `class C f: a = S ms <: M1 m1 <: … <: Mn mn ( |slots| exprs )`,
its mixin's initializer, run on the instance with `C`'s factory arguments
and entered at `C`'s level, does in this order:

1. check the selector of every `<:` clause against the factory selector of
   the class it names (today's check on the way up, today's text, before
   anything runs);
2. evaluate `ms`'s arguments;
3. invoke the initializer of the class `n` levels above `C` -- the
   superclass clause's class -- after §6.1's selector check and the arity
   check against it, with today's texts; a superclass whose application
   and everything above it run no code is not entered (the fact exists
   today: `instantiates_without_code`);
4. for `k` from 1 to `n`: evaluate `mk`'s arguments, check them against
   `Mk`'s factory arity (`check_message_arity`'s text), and run `Mk`'s own
   part -- its slot clauses and `initExprs`, never its own superclass or
   `<:` clauses -- entered at the level `Mk` added;
5. `C`'s own slot clauses and `initExprs`, as compiled today;
6. answer `self`; a top-level `^` stays evaluate-and-discard.

So each mixin carries two initialization bodies: the full one (steps
1-6) and its own part (step 5 alone), the second used when the mixin is
applied by `<:`. A `<:` level reached from outside (decision 4) runs
steps 2, 3 and 5 of its mixin -- its own superclass clause, then its own
part; whether that is a third body or the full body skipping steps 1 and
4 for a level that is not its declaration's own is the builder's. A mixin with no own code has an empty own part, and a
level whose full body would do nothing is not entered.

The clause messages are no longer bodies of their own
(`superclass_message_code`, `MixinApplicationClause::message_code`,
`CodeKind::SuperclassMessage`, `FrameKind::SuperclassMessage` and
`MixinApplicationMessage`): their argument expressions are lowered inline
in the initializer, in `C`'s factory scope, as the references do. The
`{…}` array the walk unpacked, and its error ("did not evaluate its
message arguments to an argument list"), go with them, unless an image
can still produce the condition, in which case the instruction keeps the
check.

## Left to the builder

- The instructions: their number, names, operands and encodings (an
  allocation, an initializer invocation of the receiver's own level, of
  the superclass level, of an applied level's own part, the `<:` selector
  check), their `outer_reach_of` and
  `narrow_outer_reach` arms, and how the reference lowering emits the same
  shapes uncached.
- Where the factory body lives (a `MethodEntry::Factory` that now carries
  code, or `Compiled` with a flag the mirrors filter); it must stay
  invisible to the mirrors' `methods`.
- The frame kinds of the new activations, and what an error raised in
  them names as its selector (today an initializer frame names the
  instantiated class's factory selector; keep the texts).
- Whether the factory activation counts toward the depth limit (it is an
  ordinary activation; the stack-overflow goldens must stay as they are).
- How the image changes: the initializer bodies now contain the
  superclass invocation, so `verify_mixins` reconciles the new shapes
  (both initialization bodies, their arities); the factory body is
  derived on load; `shipped_code::FORMAT_VERSION` is bumped; frame records
  of a saved image name the new frame kinds.
- The order of the work, and how the debug-build cross-check is kept
  while both forms exist, if they coexist at all.
- The measurements (the `measuring-performance` skill): the four
  micro sources under `benchmarks/micro/`, `bench_new.ns`, `bench_new2.ns`
  and `bench_newarg.ns` written out verbatim from `execution.md`, "What
  spec-conformant lookup moved", and the Are We Fast Yet benchmarks,
  against `master`, recorded in `execution.md`.
- Every document the change touches: `object-model.md` ("Instantiation",
  "Mixin application", "Class sides and metaclasses", the nested-class
  accessor), `dispatch.md` (the factory and nested-class targets),
  `compiler.md`, `execution.md` ("Deciding, then running"),
  `image-format.md`, `core-library.md` (lock 5's wording), `CLAUDE.md`'s
  invariants that name the walk, `OPEN-WORK.md` (remove "Next", add the
  futures item and the scalarization item), `PROGRESS.md`; and this file deleted.
