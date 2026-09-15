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
mechanism that makes dispatch affordable at all. They are designed, not built:
"Planned: inline caches" below. The call-site identity they will index by is
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

Every send would consult a cache at its own call site before running a lookup,
and a native send would bind an enumerated primitive instead of matching its
selector as a string. Designed on 2026-09-15, to be built in three steps —
3a, 3b, 3c below — each landing green and measured on its own. **Nothing in
this section is built yet**; today every send runs the full lookup of "The
three lookup algorithms" above, and native dispatch matches
`selector.as_str()` after the lookup found `Lookup::Native(kind)`.

**What bounds the gain.** A cache removes the lookup and nothing else: what an
activation costs to build is untouched. The last profile, taken before the
method tables got their identity hasher, put deciding a send at 10.8% of the
timed samples of `bench_block.ns` and 16.7% of `bench_send.ns`
([execution.md](execution.md), "What the profiles show"). The representation
work the caches waited for has happened — the arena, the activation stack, the
trampoline — so the caches land on the cheaper activation, as intended. 3b's
plan opens with a fresh profile, so that the ceiling is measured before
anything is built rather than assumed.

### How other systems cache sends

The choices below were made against what these systems do, and each one names
the system it follows.

| System | At the call site | Behind it | Guard |
|---|---|---|---|
| Strongtalk, interpreter (`prior-art/strongtalk/vm/interpreter/interpretedIC.hpp`) | Cache words in the bytecode itself, the send opcode rewritten from monomorphic to a polymorphic cache of 2–5 entries, allocated when a site first turns polymorphic, to megamorphic | a global `lookupCache` (16K + 2K entries); the caches are the type feedback its optimizer reads | the klass |
| Cog (`prior-art/opensmalltalk-vm/src/spur64.cog/`) | Interpreter: nothing. JIT: a monomorphic cache in the machine code, then a closed PIC of up to 6 cases (`MaxCPICCases`), then an open PIC | the method cache, 1024 entries of 4 words (`MethodCacheEntries`) | a 22-bit class index in the object header |
| SOMns (`prior-art/somns/src/som/interpreter/nodes/dispatch/`) | A dispatch chain of up to 6 cached nodes (`INLINE_CACHE_SIZE`), `CachedDnuNode`, `CachedSlotRead` and `CachedSlotWrite` among them | a generic dispatch node | the object's `ClassFactory` or layout (`DispatchGuard`) — the counterpart of a mixin application |
| Primordial Soup (`prior-art/primordialsoup/vm/lookup_cache.h`) | Nothing | a global `LookupCache`, 512 entries, two probes; an implicit-receiver send keyed by (class id, selector, calling method, rule), caching the enclosing receiver too — which works only because its class id is per class *object* | a class id in the object header |
| V8 | Bytecodes carry a feedback-slot index into a `FeedbackVector` that hangs off the closure, per context, not off the shared, immutable bytecode, and is allocated lazily once a function has run about 1 KB of bytecode; up to 4 maps per site | a global stub cache, primary and secondary tables | the map, the first word of every object |
| HotSpot | Interpreter: none — `invokevirtual` resolves once to a vtable index in the constant-pool cache, since a Java hierarchy's vtable layout is fixed at link time; the interpreter frame carries a method-data pointer into the method's `MethodData`, which profiles `TypeProfileWidth` (2) receiver types per site. Compiled code: a monomorphic `CompiledIC` | vtable and itable stubs | the compressed klass pointer in the header |

For V8 and HotSpot: [A lighter V8](https://v8.dev/blog/v8-lite) (lazy feedback
allocation), [An Introduction to Speculative Optimization in
V8](https://benediktmeurer.de/2017/12/13/an-introduction-to-speculative-optimization-in-v8/),
[Overview of CompiledIC](https://wiki.openjdk.org/spaces/HotSpot/pages/11829292/Overview+of+CompiledIC+and+CompiledStaticCall),
[TypeProfile](https://wiki.openjdk.org/display/HotSpot/TypeProfile) and
[MethodData](https://wiki.openjdk.org/spaces/HotSpot/pages/13729947/MethodData)
on the OpenJDK wiki.

**A cache guards on the application, not on the class object.** Two `Inner`
instances from different `Outer` instances have different class objects, as §6
requires, but the same application, so a site that sees both stays
monomorphic. This is recorded because the obvious key — the class object — is
the wrong one, and a cache keyed by it would never hit on a nested class.
Primordial Soup's per-class-object id is that key, and the price is visible in
its implicit-receiver cache above.

### 3a — Primitives are method-table entries

**Planned.** Native behaviour becomes members of the builtin classes, found by
the ordinary lookups; the string matching goes.

An `enum Primitive` (`builtins/primitive.rs`) names every native operation —
`IntegerAdd`, `IntegerLess`, `ObjectIdentical`, `ObjectDoesNotUnderstand`,
`BooleanIfTrue`, `BlockValue`, `ArrayDo`, `StreamShow`, `PlatformTranscript`
and the rest, one variant each. One function, `builtins::execute(mutator,
primitive, receiver, arguments)`, matches on it; the bodies move there from
today's `builtins/` modules with their semantics and error text unchanged. An
enumeration and not a function pointer, because the JIT tiers must be able to
recognize a primitive and emit it inline: a function pointer can only be
called, which is the ceiling a Rust callback imposes
([core-library.md](core-library.md)).

The bootstrap fills each builtin mixin's method table from a static list per
`BuiltinClass` of (selector, access, primitive), as a
`MethodEntry::Primitive` carrying the primitive, its access and its argument
count. A lookup that reaches one answers `Lookup::Primitive(primitive)` in
place of today's `Lookup::Native(kind)`. This is the model of the Smalltalk and
Newspeak systems above: in Cog and Primordial Soup a primitive is a *method* whose primitive
number indexes the interpreter's table (`primitiveTable`,
`Primitives::primitive_table_` in `prior-art/primordialsoup/vm/primitives.cc`),
and SOMns keeps its primitives as dispatchables in the same table as methods.
It is also the step [core-library.md](core-library.md) is headed for — every
builtin method a real method — taken for the leaves that exist today.

What goes with it:

- `dispatch::walk`'s early return at a builtin level. `==` sent to an integer
  reaches `Object`'s entry by the ordinary walk, since `Integer`'s application
  has `Object`'s as its superclass.
- `Mutator::dispatch_native_inheriting_object`, which restores that one
  inheritance step by hand today.
- The fallbacks in `interpreter/sends.rs` that turn a native's `None` into a
  public send.
- Every `match selector.as_str()` in `builtins/` and `platform.rs`.

`ClassData::builtin` stays, for `class_of`, the image's class tags and
`LoadBuiltinClass`; lookup stops reading it.

**A primitive can fail.** A native answers `None` today when its receiver has
the wrong shape: a user subclass of `Array`, whose instances carry no array
payload, or one of the three platform objects that share
`BuiltinClass::Platform` and are told apart by payload shape
([core-library.md](core-library.md), "Tagging inside the platform"). Today
that `None` makes the caller try `Object`'s natives with the same selector, and
send `doesNotUnderstand:` only if those do not answer either — so a user
subclass of `String` still answers `=` by `Object`'s identity. It becomes a
**primitive failure** with exactly that outcome: the failing level is passed
over and the lookup continues from its superclass — `Object`, for every
builtin — ending in `doesNotUnderstand:` if nothing above answers. In Cog a
failing primitive falls into its method's body, whose code often sends the
message on to `super`; these primitives have no body, so the lookup does it.
The platform objects keep their shared class: a class of their own each would
be observable (`platform transcript class` answers `Platform`) and is not
needed for correctness.

**Access.** Every primitive is `public`, except `Object`'s
`doesNotUnderstand:`, which §5.5 requires to be `protected`. Today a native has
no access at all and answers any explicit-receiver send, so an explicit
`x doesNotUnderstand: m` from outside the receiver now itself ends in
`doesNotUnderstand:`. That is the one change of behaviour 3a may make; if it
alters a recorded outcome, the new output is established against §5.5 before
the golden is updated.

**Bytecode and image:** unchanged. The builtin mixins are built by the
bootstrap and never serialized — 3a's plan confirms that against the encoder —
so primitive numbers never reach the wire. Should they ever, in an
ahead-of-time artifact, they are numbered once and never renumbered, as the
instruction tags are.

**Tests:** for each family of primitives, that the lookup finds its entry at
the right level; a primitive failure continuing at `Object` and ending in
`doesNotUnderstand:`; `Object`'s `doesNotUnderstand:` refusing an
explicit-receiver send as a protected member. Every golden outcome,
`run-corpus` under all four variants and the stress suite stay as they are.

**Expected effect, to be measured:** `bench_loop.ns` most, whose 150,000 native
sends each lose a chain of string comparisons.

### 3b — The caches

**Planned.** Every send probes a cache at its own call site; a miss probes a
global cache; only a miss there runs the lookup.

**The state lives per heap.** The cache entries live in a table the step state
owns (`Mutator`), not in the shared `CompiledCode`, which stays immutable.
This is V8's split — shared bytecode, feedback per context — reached the way
HotSpot's interpreter reaches its `MethodData`: the running activation carries
an index into the table, `feedback_base`, and a site's entry is
`feedback[feedback_base + call_site]`. It follows from invariants this
repository already holds:

- A per-heap id in shared code would mean nothing in another heap, and would
  answer a *different, valid* application there — the failure `CLAUDE.md`
  describes for handles. Kept in the heap's own table, a cache cannot be
  reached from another heap at all.
- Code shared behind an `Rc` holds no handle ([memory.md](memory.md), "What the
  collector traces"), and needs no `Cell` or `RefCell`: every step already has
  `&mut Mutator`, so an entry can hold an `Rc<MixinMethod>` and a hit pays no
  borrow check.
- Invalidation is resetting one table.
- `CompiledCode` stays immutable, which suits a later actor, running on a heap
  and a thread of its own, and a stateless binary whose code need never be
  written.

The cost is bookkeeping: an activation carries `feedback_base`, and so does
`vm.rs`'s `Flow::Switch` beside the code it switches to. This repository has
measured layout effects of 4–8% from a single field on `Step`
([execution.md](execution.md)), so that field is measured on its own, not
assumed free.

**The dispatch id.** A dense `u32`, minted per heap by a counter in the
`Mutator` when `apply_mixin` creates an application — two per application, one
for its instances and one for its class objects' class side, since every class
object shares one placeholder class and its behaviour (factory, metaclass,
`Object`'s own) depends on its application alone. Both are copied into
`ClassData`, so a guard does not follow the `Rc` to the application. A decoded
image rebuilds its applications through the same path
([image-format.md](image-format.md), "What is not serialized"), so an id never
comes from its bytes. `Heap::dispatch_id(value)` answers it: for an instance,
object → class → `ClassData` (two arena loads); for a class object, its own
`ClassData`; for `nil`, a boolean, an integer or a float, a table indexed by
the `Value` variant. Nothing outside that function knows where the id lives,
which is what lets 3c move it.

**An entry** is a guard id and a target, one of:

- `Method` — the method, its defining level `k`, and whether the chain it was
  found on is the class's or its metaclass's;
- `SlotGetter` or `SlotSetter` — the absolute field index, fixed by the
  application the guard names;
- `NestedClass` — the mixin and its level `k`;
- `Primitive` — the primitive and its level `k`, where a failure continues;
- `Factory` — the receiver is a class object and the selector its factory
  selector;
- `DoesNotUnderstand` — the handler the defined-for walk found;

and two states a site can be in, `Polymorphic` (an index into the pool below)
and `Megamorphic`. An entry holds no handle: the defining class is a class
object per enclosing instance, so it is not recorded but recomputed on a hit,
`k` superclass steps from the starting class object — the receiver's class, or
its metaclass for a class-side method, the superclass of `current_class` for
`super`, the class of `o` for a lexical send; `k = 0` is the common case. Its
size, 16–24 bytes by estimate, is measured in the plan, for the same layout
reason as `feedback_base`.

**A site's states** go empty → monomorphic → polymorphic, up to 4 entries in
the order first seen → megamorphic, and a megamorphic site stays so until an
invalidation. A site's first entry is inline in the table; the second type
seen allocates a block of 4 entries, one 64-byte cache line at 16 bytes an
entry, from a pool in the `Mutator`, moves the first entry into it and leaves
the site pointing at it; the fifth makes the site megamorphic and returns its
block to a free list. That is Strongtalk's and V8's shape,
with V8's width. Beyond the speed of a polymorphic hit, a site's entries are
the receiver-type profile the optimizing tiers of M5–M6 will read, which is
what Strongtalk's interpreter caches are for. Two kinds of site cannot go
polymorphic at all in practice: a `super` send, whose target depends only on
the running method's application, and a lexical send the private rule decides
(below). And some are polymorphic by nature: `true` and `false` are instances
of two builtin classes, so every site that is not inlined and sees both is
bimorphic.

**The global cache**: 1024 entries keyed by (dispatch id, selector, rule) —
the rule public, lexical or super — probed twice, at two hashes of the key, as
Primordial Soup's is. It is every site's miss path, probed before a lookup
runs, and the only cache a megamorphic site consults, as Cog's open PICs probe
the method cache and Strongtalk's miss handler its `lookupCache`. About 24 KB,
whatever the program's size.

**Code families.** A family is one root body — a method, an instance
initializer, a superclass clause or its message — with every block nested in
it. Its sends are numbered densely, blocks included, and each of its
`CompiledCode`s records the family's size; that number is the send's
`CallSite`. A family gets a contiguous range of the table the first time its
root is activated, so a table that code never ran stays unallocated, as V8
allocates a feedback vector lazily. `feedback_base` reaches an activation
this way:

- A method: a hit carries its target's base in the entry; a miss asks a map
  from code identity to base, and assigns a range on first use.
- A block: `MakeClosure` copies the creating activation's base into
  `ClosureData`, since both belong to one family, and the block's activation
  takes it from there — no probe per closure, which `bench_block.ns` would pay.
- An initializer, a clause, a clause message: the map, per push. If that shows
  on the instantiation benchmarks, a `Factory` entry carries the bases of the
  bodies its instantiation runs.
- A closure decoded from an image carries "not assigned", since a base is
  never serialized, and is given a fresh range of its family's size when it is
  first activated. Any range of that size is a correct one.

A site index at or beyond the end of its range is treated as uncached, never
indexed, so no crafted artifact can make the table panic.

**Resolve, then act.** The `decide_*` functions answer a `Step` today, lookup
and execution in one. 3b splits them. `resolve` is today's lookup, its order
and its rules unchanged, and answers a target — one of the entries above.
`act` turns a target into the `Step`, value or signal. The uncached path is
literally `act(resolve(…))`, the cached one `act(target)`, so there is one
place that executes a target, and a hit cannot do anything a miss would not.
**A cache records what the lookup decided; it never decides.** Every detour
today's code takes ends in a target too: a lexical send whose `lookupProtected`
finds nothing falls to `decide_send`, factory check and `doesNotUnderstand:`
included, and what it ends in is what is recorded. That is what makes the
uncached reference lowering a valid oracle for the cached one ("Verification"
below).

**A send**, step by step, in `interpreter/inline_cache.rs`; `vm.rs` passes
only `feedback_base` and the site, so it gains no dispatch logic:

1. The key. `SendPublic`: `dispatch_id(receiver)`; for a class object that is
   its class side, which covers today's order of factory, metaclass, then
   `Object`'s behaviour. `SendLexical`: `o = kth_enclosing_object(...)`, then
   `dispatch_id(o)`. `SendSuper`: the id of `current_class`'s application;
   the receiver stays `self`. `SendInlinable` is emitted only by the reference
   lowering, which runs uncached.
2. The site's entry: if its id matches, `act`.
3. `Polymorphic`: the block's entries, in order.
4. A miss, or `Megamorphic`: the global cache; a hit there is `act`, and the
   site's state moves on.
5. A miss there: `resolve`; its target is recorded in the global cache and at
   the site, then `act`.

What `act` does, per target: `Method` computes the defining class and answers
`Step::Method` as today, with the target's `feedback_base`, and `push_step`
still checks the arity. `SlotGetter` and `SlotSetter` read or write the field
without an activation, as `decide_slot_accessor` does. `NestedClass` calls
`nested::nested_class` with the declaring class `k` steps out; creating one
still runs its clause as a step. `Primitive` calls `builtins::execute`; a
failure continues the lookup above the primitive's level, uncached, as 3a
defines it, and the entry stays, as Cog keeps a method whose primitive
failed. `Factory` is `try_factory_send`'s action, and it re-checks that the
selector is the receiver's factory selector, taking the uncached path if not:
a `super` send that misses reaches the factory check on `self`
(`decide_super_does_not_understand`), and under a `super` key the guard names
`current_class`'s application, not the receiver. `DoesNotUnderstand` builds
the message array per send, as today, and calls the handler.

**Every target is a function of its key and its site.** That is the property
the entries rest on, and a path that also reads something the key does not
cover re-checks it in `act`, as `Factory` does; the plan audits `resolve` for
every such read.

**The private rule, decided once per site.** Whether §5.9's private rule
applies depends on `N` alone, the class declaration `k` levels out from the
code's own mixin: code lives on its mixin, and a block runs with its home's
`current_class`. So the decision is made when a lexical site is first filled.
Where the rule applies, the target is fixed and needs no guard — the site
stays monomorphic however many classes `o` has — and only `o` and the
defining class (`kth_lexical_class`) are computed per send. Where it does not,
the site proceeds by `o`'s id. A debug build checks on every hit that `N` is
still the mixin the decision was made for. An image crafted to break that
relation would get wrong but safe behaviour, never a panic; 3b's plan checks
whether `image::decode::verify_mixins` already guarantees the relation, and
adds the check there if not.

**No new error text.** Every error still comes from today's paths, inside
`resolve` or `act`; a hit produces exactly the step, value or signal the
uncached path would.

**Invalidation.** Nothing mutates a method table at run time yet, so nothing
has to be invalidated. The mechanism is fixed now, because the entries are
designed for it: they hold no handle, so invalidating is setting every entry
empty, emptying the global cache and resetting the pool, with every family's
range kept, since running activations keep their bases. A debug-build stress
mode, `NS_NEXT_STRESS_CACHES=1`, does exactly that at every safepoint, so that
filling, transitions and invalidation run under the whole suite long before
edit-and-continue needs them. Scoping an invalidation to the application a
change affects — the dependency key of
[liveness-and-reflection.md](liveness-and-reflection.md), "Caches and their
dependency keys" — comes with the first mutation that needs it.

**Bytecode and image.** No instruction changes shape except in the width of
its site:

- `CallSite` becomes the send's number within its family, and a `u32`. Today it
  is the instruction's index as a `u16`, which `next_call_site` checks with a
  `debug_assert!` only ([compiler.md](compiler.md), "The reference lowering"),
  so a very large body aliases two sites in a release build. Inert, that is
  harmless; read by a cache, two sends would share one entry and the second
  would run the first's method.
- A wider operand changes what a tag means, so the four send tags are retired
  and the sends take the next free numbers ([image-format.md](image-format.md),
  "The instruction encoding"). A stale artifact meets a retired tag and is
  refused.
- The code record's call-site list is written empty and a non-empty one is
  refused, so a stale code body with sends is refused before its instructions
  are read; the family size is derived from the sends' numbers on decode, never
  read.
- The reference lowering emits every send with `CallSite::UNCACHED`, so
  everything it compiles runs the full lookup on every send.
- No cache state and no `feedback_base` is ever serialized; a loaded artifact
  starts with empty caches.

**Verification.** The oracle is in place already: the reference lowering runs
uncached and the optimized lowering cached, and both are held to every
`tests-fixtures/*.ns` and its `.expected`, the golden outcomes in
`program_outcomes.rs`, and `run-corpus` from source and as a binary, so a cache
defect shows as the two lowerings disagreeing. `NS_NEXT_STRESS_CACHES=1` is a
gate of its own beside `NS_NEXT_STRESS_MOVES`; they are not combined, because
an entry holds no handle for a move to break, and emptied caches would leave
the moves hardly any hit to run under. Its run time, expected well below the
move suite's three minutes, is measured in the plan. Targeted tests pin,
through a test view of a site's state:

- two `Inner` instances from two `Outer` instances sharing one monomorphic
  entry — the requirement this section opens with;
- the transitions to polymorphic and megamorphic, and a bimorphic site on
  `true` and `false`;
- a slot accessor under two applications of one mixin with different inherited
  slots, each with its own field, as `vm_initializer.rs` pins `InitializeSlot`;
- `super` in a mixin applied under two superclasses;
- a private lexical site with a changing `o`;
- class objects: a factory send, a class-side method and `Object`'s behaviour,
  at one site that also sees instances;
- a `super` send in an inherited class-side method that misses and reaches the
  factory check, from two class objects with different factory selectors — the
  `Factory` re-check;
- `doesNotUnderstand:` with the native handler, a user handler, and the setter
  of a slot named `doesNotUnderstand`;
- a cached primitive that fails;
- an invalidation in the middle of a run;
- a snapshot taken with the caches full loading with them empty, and a decoded
  closure getting its base;
- collisions in the global cache, forced through a size for tests;
- the new send tags pinned in `opcode_tags.rs`, and a retired one refused.

**Measurement.** First, before anything is built, a `samply` profile of the
seven sources at 3a's head, for the share `resolve` still has. Then the step
whole, in two sittings by the protocol of [execution.md](execution.md), "What
removing the re-entries moved", every binary built with its `RUSTFLAGS`,
against 3a's head and against `be3eb40`. The entry's layout and
`feedback_base`'s field are measured as variants of their own. The size of the
table, the pool and the global cache is reported after `run-corpus` and after
the benchmarks. The open finding that a lexical send's decision is sensitive
to codegen ([PROGRESS.md](../PROGRESS.md)) is closed or carried forward with
the result.

### 3c — The dispatch id in the object header

**Planned.** `Heap::dispatch_id` stops reaching the id through the class
object; 3c changes that one function's representation and measures two
variants against 3b.

- **C2: the id in the header.** `Object` gains `dispatch_id: u32`, set at
  allocation — an instance's from its class's instance id, a class object's
  from its own class-side id. A placeholder the decoder allocates before it can
  validate its record carries an id no application is ever given, so it matches
  no entry, and is set when the record is filled; the id still never comes from
  the bytes. A guard becomes one load and one compare, for instances and class
  objects alike. It is where V8, HotSpot, Cog and Primordial Soup all keep the
  answer. Its cost is memory: an `Object` grows from 56 to 64 bytes in a release
  build, and the amplification a crafted image can reach at the graph section
  from about 16.9x to about 18.2x ([memory.md](memory.md)). The tracer is
  unaffected — the id is not a handle — and `become:` will have to exchange it
  with the class ([liveness-and-reflection.md](liveness-and-reflection.md)).
- **C2+: the id in the header, `ClassData` out of it.** `ClassData` alone sets
  `ObjectData`'s size, and class objects are few. Boxed —
  `ObjectData::Class(Box<ClassData>)` — an `Object` shrinks to about 48 bytes
  with the id, smaller than today, and the amplification to about 15.6x. Cog's
  class table is the alternative to a box, and buys nothing here: this id is
  per application, not per class object. The price is one more indirection on
  every read of a class's data: every level `kth_enclosing_object` walks, the
  start of a `super` send, the `k` steps of a hit with `k > 0`,
  `metaclass_of`, the miss path, instantiation — and an allocation per class
  object.

Each is measured against 3b in two sittings, with `be3eb40`, together with the
object sizes, the amplification and the memory after `run-corpus`. The
proposed rule: the faster wins; within noise, the smaller; if neither beats
3b, the id stays on the class object and the question is recorded as closed
by that measurement. The user decides on the numbers.

### Considered and rejected

- **Cache state in the instruction stream** (Strongtalk). Every instruction
  would grow to the size of the largest variant, a send with its cache, and
  shared code would become mutable: `Instruction` holds `Vec`s, so safe Rust
  could rewrite one only behind a `RefCell`, and the encoder would have to write
  a neutral form back out.
- **Cache state in `CompiledCode`**, as HotSpot keeps `MethodData` on the
  method. The hot loop would stay as it is, but shared code would need `Cell`
  or `RefCell`, ids would have to be unique across the process to stay correct
  if code were ever shared between heaps, and invalidation would walk every
  body.
- **A global cache alone** (Primordial Soup, Cog's interpreter): every send pays
  a hash and two or three comparisons, sends that collide evict each other, and
  no site keeps a profile for the optimizing tiers.
- **Vtables, or a resolved table per application** (HotSpot's interpreter): the
  method sets are open, three access rules answer differently for one selector,
  and `doesNotUnderstand:` exists, so a vtable needs an index per selector
  across every application and a table per send kind; a memoized table per
  application still hashes on every send and keeps no profile.
- **Monomorphic only**, as HotSpot's compiled code: a site that alternates
  between two types misses every time, and HotSpot can afford that only because
  its megamorphic path is a vtable load, where this one is a hash. **Four
  entries inline at every site**: no indirection even for a polymorphic hit,
  but four times the memory for the monomorphic majority.
- **A side table (builtin class, selector) → primitive** in place of 3a: it
  replaces the string matching but keeps a second kind of member, the dispatch
  leaf, the hand-restored inheritance from `Object`, and natives with no
  access; none of the Smalltalk or Newspeak systems above does it that way.

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
- **Planned (2026-09-15): primitives are method-table entries** (3a), bound as
  an enumerated primitive, so natives are members found by the ordinary
  lookups, with an access modifier, as in Cog and Primordial Soup.
- **Planned (2026-09-15): inline caches per heap** (3b) — monomorphic, a
  polymorphic cache of 4, then megamorphic, with a global cache of 1024
  entries behind every site — guarded on the mixin application, never on the
  class object. A cache records what the lookup decided and never decides.
- **Planned (2026-09-15): the dispatch id's place is decided by measurement**
  (3c): on the class object, in the header, or in the header with `ClassData`
  boxed.
