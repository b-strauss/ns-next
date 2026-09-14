# Name resolution and dispatch

> **Status:** built.
> **Code:** `crates/ns-runtime/src/resolver/`, `dispatch.rs`,
> `interpreter/mod.rs` (send, `doesNotUnderstand:`, lexical, super, native).

What a name means is settled in two halves: the resolver fixes the *shape* of
each reference statically, and dispatch resolves the *target* at run time. They
belong together, because a Newspeak name is neither a variable access nor a call
until both halves have run.

**Every send is a virtual call; there is no static dispatch to fall back on.**
§2.1: "A synchronous message send is like a virtual method call." §2.3 makes it a
hard rule: "Methods are always virtual (i.e., subject to override)" — the
opposite default from Java or C++. The one exception is `private`, and §5.9 is
precise about why: "the code could be predetermined — but the receiver could
not". A private method cannot be overridden, so its body is fixed, but which
object it runs on still depends on run-time enclosing-object navigation.

The consequence for performance is structural: with no static dispatch anywhere
outside the private case, inline caches are not an optional speedup but the
mechanism that makes dispatch affordable at all. The slots they will use are
already emitted; see [compiler.md](compiler.md).

## Resolution

Input a `Module`, output a `ResolvedModule` — the same tree with each `Identifier`
annotated with a `Binding`, plus per-scope slot layout metadata.

Newspeak's algorithm is two-phase, not a single linear scope walk:

1. **Local names are static, resolved purely lexically**, walking the activation
   chain outward: the current method or block's temporaries and parameters, then
   each enclosing block's, innermost first. No dispatch — a direct frame-slot
   read.
2. **Everything else is a send.** Method calls, slot reads, slot writes and bare
   references to nested or enclosing class names are all sends, per §2.2.1:
   "references to methods without arguments are not distinguished from references
   to slots... or to nested classes". §5.7 gives the search: walk the enclosing
   **class declarations** — not activations, which step 1 already covered — from
   innermost outward, and take the smallest depth `k` whose class, including
   everything it inherits, has a member of that name. `k = 0` is what is
   colloquially a self send; §5.8 defines `self m` as literally `outer C m` where
   `C` is the immediately enclosing class. A name found at no lexical level still
   resolves at depth 0, and run-time dispatch then produces
   `doesNotUnderstand:`.

### Binding variants

- `Local(u16)` — a slot in the current activation. Static.
- `OuterLocal { depth, index }` — a captured variable `depth` activations out.
  Static.
- `LexicalSend { depth }` — one mechanism covering implicit-receiver method
  calls, slot reads, slot writes and references to nested-class accessors. These
  are not three separate cases; per §5.7–§5.9 they are an outer-send at depth
  `k`.
- `SuperSend` — a `super m` send (§5.10). Not depth-based; resolved at run time
  against the current activation's `current_class`, which need not be `self`'s
  own class.

There is deliberately **no separate "class value" binding**. A nested or
enclosing class name is referenced through the ordinary `LexicalSend` mechanism
and dispatch happens to land on that class's implicit accessor (§6.4). This
includes the superclass clause's receiver part, which §6.1 evaluates "as if it
occurred inside an instance method" of the enclosing class — the same mechanism,
evaluated at a particular time rather than a distinct binding kind.

**Known gap: a `super`-prefixed clause receiver does not dispatch.** For
`class N = super Foo (...)` the resolver binds the receiver `Foo` as
`SuperSend` (`resolver::resolve_superclass_clause`), but the lowerer treats a
`SuperSend`-bound *identifier* like an unbound one: it falls to the
bare-builtin-name fallback (`LoadBuiltinClass`), and for any other name to the
deferred `UnresolvedIdentifier`. So the form works only when `Foo` names a builtin
class, and then answers that builtin rather than the result of sending `Foo`
with lookup starting above the declaring class, as §6.1 requires; any other
`Foo` raises an "unresolved identifier" error when the accessor first runs the
clause. The clause's activation already carries the declaring class as its
`current_class`, so the fix is a lowering change;
`crates/ns-runtime/tests/nested_classes.rs`'s
`a_super_prefixed_clause_answers_a_class_object` pins today's builtin case.

### Setter sends

`id:: e` (§5.4.3) desugars to sending the keyword message `id: e` and yielding
`e` as the expression's value — not the setter method's own return value, which
is `self` per §6.3.2. The resolver treats the target of an assignment exactly as
it would an implicit-receiver keyword send to `id:`: full depth search,
private-versus-virtual dispatch, everything. It is not a distinct slot-write
primitive.

### Shapes the resolver must recognize

The parser normalizes several forms, and the resolver keys off those shapes:

- **A bare unary implicit send** (`foo` with no message following) parses as a
  plain `Expression::Identifier`, not wrapped in a `Send`. Per §5.4.3 a bare identifier used
  as a primary *is* an implicit-receiver unary send, so every standalone `Identifier`
  gets the full Local → OuterLocal → §5.7 resolution, unless it is literally
  `self` or `super`.
- **An implicit-receiver keyword send** has its receiver synthesized by the
  parser as `Identifier{"self"}` with a **zero-width span** at the selector's start.
  That is the discriminator: a zero-width span means a true implicit send and the
  depth search runs; a real span means a genuine explicit `self` send, where §5.8
  fixes the depth at 0 with no search. Binary sends are never implicit, so no
  equivalent synthesis exists for them.
- **`outer N m`** parses as an ordinary two-hop unary chain with no dedicated AST
  shape. The resolver matches the whole shape as one `outer N s` send — a
  compile-time error if `N` does not name a lexically enclosing class — and never
  dispatches the inner "send `N` to `outer`" step as a real message.
- **`super m`** always has a real span; the grammar has no implicit form.

## The three lookup algorithms

The specification defines three lookup shapes gated by access modifiers. This is
a behavioural difference, not an editorial nuance.

**`lookupPublic(selector, class)`** — ordinary sends with an explicit receiver
(§5.5). Walk from `class` up the superclass chain. At each level: a `public`
declaration of the selector matches. A declaration that exists but is
`protected` or `private` **stops the search and fails** — a non-public
declaration *shadows* a public implementation further up the chain rather than
merely hiding the name locally. Only a level that does not declare the selector
at all continues the walk.

**`lookupProtected(selector, class)`** — the run-time half of a `LexicalSend`
whose statically determined target is not private (§5.9). The same walk, but
`public` and `protected` both match, and a `private` declaration at an
intermediate level is transparently skipped rather than stopping the search.

**Direct, non-virtual dispatch** — a `LexicalSend` whose statically determined
target at depth `k` is `private` bypasses both lookups and invokes exactly that
method, ignoring the enclosing object's run-time class. Private members are never
overridden, so there is nothing to look up.

**`SuperSend`** (§5.10) starts at the superclass of the *current activation's*
`current_class`, not of `self`'s class, and uses the plain "defined for" walk
(§3.5) where public and protected both match with no hard stop. On success the
new activation's `current_class` becomes the defining class, which is what makes
a chain of `super` sends walk one level at a time instead of looping.

**`doesNotUnderstand:` is exempt from every access check.** `Object`'s own
implementation must be `protected` (§5.5), so resolving it through
`lookupPublic` would make it unreachable from an ordinary send. It is resolved
by the presence-only "defined for" relation instead: the first
`doesNotUnderstand:` found walking the receiver's class chain, whatever its
access modifier.

**Unmarked access defaults to `protected`** (§6.3.1). This is binding rather than
cosmetic: it decides whether a member participates in `lookupPublic` at all. A
top-level class's own access is always public regardless of what is written on
it.

## Resolving a lexical send at run time

Compute `o`, the `k`th enclosing object of `self` with respect to the current
class (§3.3), then apply the private-versus-`lookupProtected` rule above against
`o`'s **actual run-time class**. That is what makes public and protected lexical
sends virtual while private ones are not.

**`outer` walks the lexical declaration chain, never the receiver's dynamic
class.** `Frame.current_class` seeds the walk and `ClassData::declaring_class`
continues it level by level. A method or nested-class accessor inherited across a
nesting boundary runs with the class that lexically declares it, not with the
receiver's own class. Using the receiver's class instead silently resolves
`outer` to the wrong enclosing object — the kind of defect that produces a
plausible answer rather than a crash.

## The send machinery

Send, `doesNotUnderstand:`, lexical, super and native dispatch, frame
construction and `instantiate` live in `interpreter/`, the send machinery the VM
calls: its instruction loop calls them and runs whatever body they choose
([execution.md](execution.md), "The VM").

Never copy dispatch logic, lookup logic or error text out of `interpreter/` into
`vm.rs` or `compiler/`. The reference lowering and, later, a JIT must reach the
same dispatch and raise the same errors, and a second copy is how they would
drift. Something needed there is widened to `pub(crate)`, not duplicated.

An arity check belongs here too: a method activation addresses its arguments and
temporaries as one run of registers, so a send whose argument count does not
match the declaration would silently shift every temporary rather than fail.
`Interpreter::push_step` refuses the mismatch outright.

## Deviation: slots are not reachable through an explicit receiver

`Interpreter::send` never consults slot lookup. A public slot's implicit
accessors are reachable only through an ordinary unqualified send, not through a
send with an explicit receiver — so the language specification's own example
spelling, `(g Inner: Array) Nested`, does not work.

Fixing it means adding slot entries to `lookupPublic`'s walk, deciding
setter-selector naming and its precedence against a same-named method, and
enforcing a slot's access modifier. None of that exists yet.

## Planned: inline caches

A stable call-site identity and an inert inline-cache slot per send are already
carried by every `CompiledCode`, so wiring caches up needs no bytecode format
break. Nothing reads or writes those slots today; see [compiler.md](compiler.md).

**A cache must guard on the application, not on the class object.** Two `Inner`
instances from different `Outer` instances have different class objects, as §6
requires, but the same application, so the call site stays monomorphic. An entry
records (application → method, defining level `k`); reaching that level's class
object for `outer` and `super` starts at the defining level's own class object
and steps `k` levels out along the declaration chain, with `k = 0` for an
ordinary send.

This is recorded here rather than derived later because the obvious key — the
class object — is the wrong one, and picking it would produce a cache that
never hits.

**A native send binds an enumerated primitive.** Native dispatch today
selects with `match selector.as_str()` after the lookup found
`Lookup::Native(kind)`. The caches do not keep that shape. A lookup that
reaches a native binds an *enumerated primitive* — a Rust enum naming the
operation, `IntAdd` rather than a selector — and a native cache entry
carries it, so a hit costs the guard and an indexed jump. An enumeration,
not a function pointer, because the JIT tiers must be able to recognize the
primitive and emit it inline: a function pointer can only be called, which
is the same ceiling a Rust callback imposes ([core-library.md](core-library.md)).
This is Cog's and Primordial Soup's model: a method's primitive number
indexes an interpreter-side table (Cog's `primitiveTable`; Primordial Soup's
`Primitives::primitive_table_`) and, in Cog's JIT, `primitiveGeneratorTable`,
whose generators (`genPrimitiveAdd`) emit machine code, and at the send site
`genSpecialSelectorArithmetic` emits SmallInteger arithmetic inline with a
fallback send (`prior-art/opensmalltalk-vm/src/spur64.cog/`,
`prior-art/primordialsoup/vm/primitives.cc`). SOMns reaches the same result
through Graal's partial evaluation of its primitive nodes.

**A lexical send to a slot is cached too.** `sends::decide_lexical_send` checks
for a slot before any method lookup: it interns the getter name for every
setter-shaped selector (`sends::slot_selector_shape`) and hashes the name
into the application's `slot_index` for every lexical send. A lexical-send
cache entry that records the slot's index removes both.

**Caches come after the representation change, not before it.** A cache removes
the lookup; the method it records carries its compiled body. It does not touch
what an activation costs to build, which was measured at ~68 ns — about 41% of a
user-method send — on the pre-arena representation; see
[execution.md](execution.md)'s measurements. Wiring caches up first would
optimize the half of a send that sits on top of a fixed cost the same milestone
removes anyway, so the representation goes first and the caches land on the
cheaper one.

That representation change has happened. The arena replaced `Rc<RefCell<_>>`,
worth about 15% of the benchmark ([memory.md](memory.md)). The activation stack
means an activation nothing captures allocates nothing: no arena entry and none
of the two `Vec`s every activation used to take. A captured one still pays for
its reified frame and, when it returns, two vectors for the divorce. Against the
arena, a user-method send fell from ~146 ns to ~91 ns
([execution.md](execution.md), "What the activation stack moved"). Each cache
also carries a dependency key, per
[liveness-and-reflection.md](liveness-and-reflection.md); the key is the
application, for the same reason the guard is.

## Decisions

- **Three lookup algorithms, not one.** A single "walk methods then superclass"
  routine is wrong for at least two of the three cases, and the difference is
  observable.
- **`doesNotUnderstand:` bypasses access control**, because `Object`'s own
  implementation is protected and would otherwise be unreachable.
- **Unmarked members are protected**, per §6.3.1, which decides public
  reachability rather than only documenting intent.
- **Dispatch lives in `interpreter/`, once.** Duplicating it into the VM is the
  failure mode this rule exists to prevent.
