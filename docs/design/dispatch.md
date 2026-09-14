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
   to slots... or to nested classes". §5.7 gives the search: "Let d be the
   innermost lexically enclosing construct in which a method, slot or class
   named s is declared". Walk the enclosing **class declarations** — not
   activations, which step 1 already covered — from innermost outward, and at
   each one count only what that declaration itself **declares**: its methods
   under their full selectors (`at:put:` declares `at:put:`, not `at`), each
   slot's getter, each *mutable* slot's setter `x:` (§6.3.2: an immutable slot
   has "no setter method"), and its nested classes. The first declaration that
   declares the selector is `N`, at depth `k`, and the send is `outer N m`.
   Nothing a declaration merely **inherits** counts: §2.3.1, "lookup proceeds up
   the lexical scope chain ... and only if no lexically visible matching method
   is found, do we proceed up the inheritance chain of self . After that, no
   further lookup is done." So a name that no enclosing declaration declares is
   `self m` (§5.7's "Otherwise"), and the receiver's inheritance chain decides
   at run time, ending in `doesNotUnderstand:` if nothing answers. §5.7's
   commentary on figure 4 says the same: "First, the surrounding lexical scope
   chain is traversed ... Only if this fails is the receiver's inheritance chain
   followed". It is the *receiver's* chain, so a member an *enclosing* class only
   inherits is never found implicitly; an explicit `outer N m` (§5.9) reaches it,
   since §5.9 runs `lookupProtected` on the class of the enclosing object.
   Primordial Soup does the same at run time (`ImplicitReceiverSendMiss` asks
   each lexical level's own methods through `HasMethod`), and the paper the
   specification cites for the rationale, [Bra07b]
   (`docs/references/dyla-final.pdf`), argues for exactly this rule. `k = 0` is
   what is colloquially a self send; §5.8 defines `self m` as literally
   `outer C m` where `C` is the immediately enclosing class.

   Two names in one declaration that would define the same member — a method
   and a slot getter, a mutable slot's setter and a method `x:`, a method and a
   nested class — are a resolve error. The specification states no such rule;
   the canonical compiler refuses them (`Newspeak2SqueakCompilation.ns`,
   `checkForDuplicateNames:`), and a method table cannot hold both. An
   immutable slot `x` beside a method `x:` is not a collision: §6.3.2,
   "defining a setter will not impact such an initialization".

   **Open: the class side's scope.** Both sides of a class declaration
   contribute to one member set. The specification is silent on whether an
   instance-side method sees class-side declarations lexically; the canonical
   compiler scopes the class side apart (`compileClassSide:`). Nothing on
   `corpus/runnable.toml` or among the fixtures depends on the difference, so it
   stays as it is until a program does.

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

**Deviation: a builtin class name is loaded, not sent.** §3.2 has `Object`
provide "accessors for predefined classes and objects, such as the classes of
the built-in literals (5.1) e.g., String", so a bare `String` that no enclosing
declaration declares is, by §5.7, `self String`, answered by `Object`'s
accessor. The resolver instead leaves such a name unbound and the compiler loads
the builtin class directly (`Instruction::LoadBuiltinClass`). The two differ
only when a user class overrides the accessor — a method named `String` in a
superclass of the sender — which the direct load ignores.

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

Each walks from a class up the superclass chain, and at every level that
declares the selector — a method, a slot accessor or a nested class, which are
members on the same footing — the declaration's accessibility decides one of
three things: the declaration is the answer, it is passed over and the walk
continues, or the walk stops with no answer (`dispatch::Verdict`).

**`lookupPublic(selector, class)`** — ordinary sends with an explicit receiver
(§5.5): "m, if the mixin of C defines a method m with selector n and public
access. Undefined, if ... protected access. lookupPublic(n, superclass(C))
otherwise." A `public` declaration is the answer; a `protected` one **stops the
search and fails** — it *shadows* a public implementation further up the chain
rather than merely hiding the name locally; a `private` one is **passed over**,
falling to §5.5's "otherwise". Primordial Soup's `OrdinarySendMiss` does exactly
this.

**`lookupProtected(selector, class)`** — the run-time half of a `LexicalSend`
(§5.9), and of a super send. `public` and `protected` are the answer, and a
`private` declaration is passed over.

**The "defined for" walk** (§3.5) — only for `doesNotUnderstand:`, below: every
declaration is the answer, whatever its accessibility.

**`SuperSend`** (§5.10) starts at `S`, the superclass of the *current
activation's* `current_class`, not of `self`'s class, and uses
`lookupProtected`: "If a public or protected method f named s is defined for S,
the value of the send is the result of invoking f" — a private method of `S` is
passed over (Primordial Soup's `SuperSendMiss` runs its `ProtectedSend`). If
nothing answers, "the #doesNotUnderstand: method defined for S is invoked": the
`doesNotUnderstand:` lookup starts at `S`, not at the receiver's own class. On
success the new activation's `current_class` becomes the defining class, which
is what makes a chain of `super` sends walk one level at a time instead of
looping.

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
class (§3.3). Then §5.9: "If N declares a private method f named s then the
value of the send is the result of invoking f in response to (3.5) message µ on
o. Otherwise, Let f = lookupProtected(s, R) where R is the class of o".

**The private rule asks `N` alone.** `N` is the class declaration at lexical
depth `k` — the class that declares the running method at `k = 0`, its
declaring class one step out, and so on (`dispatch::kth_lexical_class`). Only
`N`'s own mixin is asked whether it declares the selector private
(`dispatch::private_member`); a private declaration anywhere else — in a
subclass of `N` that `o` happens to be an instance of, or in a superclass of
`N` — is not `N`'s, and `lookupProtected` passes it over. A private member is
never overridden, so there is nothing to look up: the body is fixed, and only
the receiver `o` is computed at run time. Primordial Soup's `LexicalSend` is
the same two steps: `MethodAt` on `N`'s own application, then `ProtectedSend`
from the receiver's class. Otherwise `lookupProtected` runs against `o`'s
**actual run-time class** — which is what makes public and protected lexical
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
`Mutator::push_step` refuses the mismatch outright.

## Slot accessors

A slot is reached only through messages (§2.3: "accessed exclusively via
messages"). §6.3.2: "The getter method is implicitly defined by the slot
declaration, and returns the value of the slot. The accessibility of the method
is the same as the slot's"; a mutable slot also has a setter, named by the slot
name with `:` appended, which "sets the value of the slot to be the incoming
argument. It returns the receiver of the message"; an immutable slot has "no
setter method".

So a slot's accessors are **entries of its mixin's method table**, beside the
compiled methods (`mixin::MethodEntry::SlotAccessor`), with the slot's own
accessibility. Every send kind finds them by its ordinary lookup — an explicit
receiver through `lookupPublic` (§6.3.2 and §2.3's own `(g Inner: Array)
Nested`), a lexical send through §5.9, a super send through §5.10 — and they
are overridden like any method: §2.3, "Methods are always virtual", and §6.3.2's
"overriding a setter" presupposes it. That is how the canonical implementations
hold them: the Newspeak compilers generate synthetic accessor methods into the
mixin (`Newspeak2JSCompilation.ns`, `generateSlotSetterFor:`; setters only for
mutable slots), and SOMns puts `SlotDefinition` and `SlotMutator` dispatchables
in the same table as its methods (`MixinBuilder.java`).

An accessor runs no code: its entry names the slot by its position among its
mixin's own slots, resolved through the defining application's `own_slot_base`
exactly as `Instruction::InitializeSlot` is, and `Mutator::decide_slot_accessor`
answers the send without an activation — the interpreter's counterpart of Cog's
quick methods (`quickPrimitiveInstVarIndexFor`) and SOMns's `CachedSlotRead`.
Initialization never goes through an accessor ("The initialization itself is not
accomplished by the setter method"): the instance initializer writes the field
with `InitializeSlot`.

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

**A slot accessor is an ordinary cache entry.** It is found by the same lookup
as a method, so an entry records it like a primitive: the guard, then the field
read or write.

**Open: the megamorphic fallback.** A call site that sees more applications than
a polymorphic cache holds needs a second level behind the inline caches: Cog's
open PICs probe its global method cache, V8's megamorphic sites its stub cache,
and Primordial Soup — which has no inline caches — relies on its global
`LookupCache` alone (512 entries, two probes). Keyed by application, selector and
send kind, it holds no handle, needs nothing from the collector, and a class
change empties it through a counter, as Cog's `flushMethodCache` does. Designed
with the caches, not before them.

**Open: the application in the object header.** A guard that reaches the
application through the class object costs several dependent loads. V8's maps,
Cog's class index and HotSpot's klass pointer put the answer in the object
header, so a guard is one load and one compare. A dense application id in the
header would do the same for ns-next — and keeps a site monomorphic across
enclosing instances, where Primordial Soup's per-class-object id
(`Behavior_basicNew` allocates it) makes it polymorphic. It touches the object
representation, the tracer and the image, so it is decided with the caches.

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
- **Only declared members are lexically visible** (§5.7, §2.3.1). Counting
  inherited ones is the "comb" semantics Newspeak was designed against.
- **A slot's accessors are methods** (§6.3.2), in the method table, answered
  without an activation.
- **`doesNotUnderstand:` bypasses access control**, because `Object`'s own
  implementation is protected and would otherwise be unreachable.
- **Unmarked members are protected**, per §6.3.1, which decides public
  reachability rather than only documenting intent.
- **Dispatch lives in `interpreter/`, once.** Duplicating it into the VM is the
  failure mode this rule exists to prevent.
