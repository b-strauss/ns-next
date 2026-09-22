# Name resolution and dispatch

> **Status:** built, the inline caches included, with the dispatch id in the
> object header (3c).
> **Code:** `crates/ns-runtime/src/resolver/`, `dispatch.rs`,
> `interpreter/resolve.rs` (every send's lookup, `doesNotUnderstand:`, and
> `act`, which runs what a lookup found), `interpreter/inline_cache.rs` (the
> caches), `interpreter/mod.rs` (`decide_send`, the uncached explicit-receiver
> send), `interpreter/sends.rs` (the uncached lexical and super sends),
> `interpreter/instantiate.rs` (`decide_factory`, the action of a factory
> entry and the per-send read a cache key does not cover),
> `mixin.rs` (`DispatchId`, `MixinMethod.primitive`), `heap.rs`
> (`Heap::dispatch_id`), `builtins/primitive.rs` (the enumerated primitive
> and the VM mirror's table, `MIRROR`), `interpreter/mirror.rs` (how a send
> to the mirror runs), `compiler/primitive_method.rs` (which kernel methods
> are primitive methods).

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
mechanism that makes dispatch affordable at all. They are built: "Inline
caches" below. The call-site identity they index by is emitted by the
compiler; see [compiler.md](compiler.md), "Safepoints and call-site
identity".

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

   The member set also holds the declaration's **primary factory selector** —
   the one its own message pattern spells out, or `new` when it writes none.
   §6.3 makes the factory a member of the metaclass mixin the class side
   becomes, so §5.7's search must find it on the declaring class: without it
   an implicit `new` inside a class-side method bound to an *enclosing*
   class's member `new` and answered that member instead of an instance,
   silently and at exit status 0. It is the same selector a class-side method
   may not redeclare (`classes::duplicate_member_names`), so the two
   agree on what the class side holds. Because the two sides share one member
   set, an implicit `new` in an *instance*-side method now binds at the
   declaring class too, and answers `doesNotUnderstand:` where it used to
   reach an enclosing member `new` — the same shape every other class-side
   member already has, and the direction the open question below would have to
   settle for all of them at once.

   **A member set belongs to its declaration, borrowed down the walk.**
   `resolve_class` computes `classes::own_member_names` for the declaration
   it is entering, keeps it in its own stack frame, and hands the chain down
   as `&[EnclosingClass]`, each level borrowing the set of *its own*
   declaration -- no set is copied, and none can be looked up by name, so two
   classes with the same simple name anywhere in one module can never meet.
   The superclass clause resolves against the same chain without the
   declaration's own level. `EnclosingClass.name` stays for `outer N` alone,
   where §5.9 does mean a name and the innermost enclosing class of that name
   is the one. The resolver used to map a class's simple name to its own
   members, under which two same-named classes shared one entry and the
   last one collected won -- a member
   declared by the loser was invisible to §5.7's search, which walked out to
   an enclosing declaration instead, or found nothing and raised
   `doesNotUnderstand:`, silently and at exit status 0. Renaming the
   unrelated class changed the answer, which was the signature of the
   defect. A textual count found eighteen of the 211 corpus modules declaring
   two classes under one simple name -- a lower bound, since it only scans
   source text -- and none of the six on `corpus/runnable.toml` was among
   them, so no gate saw it; the reference lowering agreed with the wrong
   answer too, since the mistake was made in the resolver, above both
   lowerings. In both goldens the trailing, same-named declaration is the one
   that would have won the shared map slot:
   `spec_5_7_a_member_set_is_per_declaration_not_per_name` pins the case
   where it is empty, so the search would have walked out to an enclosing
   member instead of finding the class's own;
   `spec_5_7_a_namesakes_members_are_not_in_reach` pins its mirror image,
   where the trailing declaration instead has the member being searched for,
   so the search would have stopped at the wrong class and answered
   `doesNotUnderstand:` -- together showing the answer depends on neither
   the name nor what an unrelated namesake happens to declare.

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
and dispatch happens to land on that class's implicit accessor (§6's
unnumbered introduction). This
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

The activation chain is searched first, and **built (2026-09-19): only a
local that declares a setter answers `id:`.** §3.6 makes every parameter
slot immutable, and makes a local slot what its declaration says (§6.3.2),
so a parameter -- of a method, a block or a class's primary factory -- and
an immutable local `| x = e. |` declare no setter; a mutable local (`| x |`,
`| x ::= e. |`) does. `ScopeStack::resolve_setter` walks the activations
innermost first, passes every declaration without a setter by, and answers
the first that has one; when none does, the class chain's depth search for
`id:` runs as it does now, and when nothing declares it either the send is
`self id:`, so the program sees `doesNotUnderstand:` at run time -- the
answer `spec_3_6_a_parameter_slot_is_immutable`,
`spec_3_6_a_block_parameter_is_immutable` and
`spec_3_6_a_factory_parameter_is_immutable` record, and
`spec_3_6_an_immutable_local_slot_has_no_setter` records for a declared
immutable local. The canonical compiler does exactly this: its scopes are
keyed by selector, a parameter enters only its getter and a temporary its
setter only when mutable, and `setterSendNode:` searches the scope chain
outward for the setter (`prior-art/newspeak/NewspeakCompilation.ns`,
`codeBodyNode:` and `setterSendNode:`; Primordial Soup's copy is the same).
SOMns instead rejects an assignment to an argument at compile time, checking
every enclosing activation before any local -- which its own `TODO` in
`MethodBuilder.getSetterSend` doubts -- and writes an immutable local
freely; neither is §5.4.3's send.

One consequence reads oddly and follows from §5.7 all the same: a block
parameter `v` shadowing a mutable temporary `v` of its method leaves `v:: e`
in the block writing the *method's* `v`, the innermost declaration of `v:`,
while `v` in the block still reads the parameter.
`spec_5_7_a_setter_send_passes_a_shadowing_parameter_by` pins it.

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

**Deviation: a class side's own `doesNotUnderstand:` is unreachable.** An
ordinary send to a class object searches the metaclass, so a class-side method
answers; but the `doesNotUnderstand:` walk starts at `class_of(receiver)`,
which for a class object is the builtin `Object` class rather than the
metaclass, so a `doesNotUnderstand:` declared on a class side is never
invoked and the send raises the default error instead. §5.5 gives the handler
no such exception: it is "the `#doesNotUnderstand:` method defined for the
class of the receiver", and the class side is part of what a class object
answers to. The asymmetry is the implementation's own -- class-side *methods*
are reachable by the same receiver -- rather than a consequence of reifying no
`Class`, though it disappears when §3.2's hierarchy is reified in step 6.

**Deviation: the reified message is a two-element tuple.** §3.4 says a message
"may be *reified* by the implementation by means of a *message mirror*", "an
object obeying the protocol of class `MessageMirror` that provides access to
*µ*'s selector and arguments". There are no mirrors
([liveness-and-reflection.md](liveness-and-reflection.md)), so
`resolve.rs`'s `handler_message` hands the handler `{selector. arguments}` — a
two-element `Array` whose first element is the selector symbol and whose second
is an `Array` of the arguments. A handler therefore reads `message at: 1` and
`message at: 2` rather than sending a mirror protocol, which
`goldens/cache/does_not_understand_site_with_three_kinds_of_handler.ns`
records. The shape changes when mirrors arrive.

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
([execution.md](execution.md), "The VM"). A send the VM executes goes to
`interpreter/inline_cache.rs` (`send_public`, `send_lexical`, `send_super`),
which answers from the call site's cache or runs the lookup; every lookup is in
`interpreter/resolve.rs` (`resolve_public`, `resolve_lexical`,
`resolve_super`), and so is `act`, the one place that runs what a lookup
found. The uncached sends — `Mutator::decide_send`,
`sends::decide_lexical_send` and `sends::decide_super_send` — are `act` on a
fresh resolution, and are what the reference lowering's sends, a host's
`Interpreter::send` and a send a continuation decides (`mustBeBoolean`) run.

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

## Primitives

A primitive is an entry of a method table (`MethodEntry::Primitive`,
`mixin.rs`), found by the ordinary lookups instead of matched by its
selector's text, and **only the VM mirror's method table holds any**: one
public entry per row of `builtins/primitive.rs`'s `MIRROR`, carrying
`Primitive::Mirror(row)` (`mirror_method_table`). The builtin classes'
behaviour is `Kernel.ns` — ordinary compiled methods that forward to the
mirror ([core-library.md](core-library.md), "The VM mirror is the whole Rust
boundary"). `enum Primitive` names every native operation, one variant each,
grouped by the builtin family whose module executes it —
`Primitive::Integer(IntegerPrimitive::Add)`,
`Primitive::Object(ObjectPrimitive::DoesNotUnderstand)`,
`Primitive::Collection(CollectionPrimitive::Enumerate)` for `do:`,
`Primitive::Platform(PlatformPrimitive::Show)` and the rest — and a `MIRROR`
row names the one it runs. One function,
`builtins::execute(mutator, primitive, receiver, selector, arguments)`,
matches on it — the selector only for the error texts that name the send —
and the bodies stay in `builtins/`'s modules. Its `Mirror` arm is a send to
the mirror object, which `interpreter::mirror::execute_sent` splits into the
subject, the leaf's arguments and the failure block before it runs the row's
leaf; every other arm is a leaf itself, which a mirror send reaches with the
row's **reported selector** — `+`, not `integer:add:ifFail:` — so an error
text names the kernel selector the program sent. An enumeration and not a
function pointer, because a later JIT tier must be able to recognize a
primitive and emit it inline: a function pointer can only be called, the
ceiling a Rust callback imposes ([core-library.md](core-library.md)). This is
the model of the Smalltalk and Newspeak systems above: in Cog and Primordial
Soup a primitive is a *method* whose primitive number indexes the
interpreter's table (`primitiveTable`, `Primitives::primitive_table_` in
`prior-art/primordialsoup/vm/primitives.cc`), and SOMns keeps its primitives
as dispatchables in the same table as its methods — on its VM mirror's class,
as here.

A lookup that reaches a primitive answers `Lookup::Primitive(primitive,
level)`, `level` being the class object whose method table holds it — the
same "defining class" a `Lookup::Method` reports — and `resolve` turns it
into `Resolved::Primitive` with the chain it was found on ("Resolve, then
act" below).

**The mirror's class and table are fixed at bootstrap.** The bootstrap builds
them once from `MIRROR`, and nothing can add to or override them: no program
can name the mirror's class to subclass it, the decoder refuses its tag
wherever a program names a class ([image-format.md](image-format.md),
"Verification"), and there is no MirrorBuilder yet. That is what lets the
compiler bind a mirror send to its row at compile time — `SendMirror` carries
the row and runs it with no lookup, checking only that its receiver register
holds the mirror ([compiler.md](compiler.md), "Mirror sends") — and what
lets a primitive method carry its row ("Primitive methods" below). **A
MirrorBuilder must seal the mirror's class when it arrives**: a method added
to or replaced on it would be found by a sent mirror send and bypassed by
every `SendMirror` and primitive method, and the two would then disagree
([liveness-and-reflection.md](liveness-and-reflection.md), "Mirrors and
MirrorBuilder").

**A primitive can fail.** `builtins::execute` answers `None` when a leaf
declines: a receiver of the wrong variant or payload — an instance of a user
subclass of `Array`, which carries no array payload, or one of the three
platform objects that share `BuiltinClass::Platform` and are told apart by
payload shape ([core-library.md](core-library.md), "Tagging inside the
platform"). The answer goes back to the mirror code, which runs the send's
failure block, the `[^super …]` every kernel method writes:

- inline, for a `SendMirror`: the VM jumps to the failure target, where the
  optimized lowering placed the block's body ([compiler.md](compiler.md),
  "Mirror sends");
- activated, for a mirror send that was *sent* — the reference lowering's,
  and any the optimized lowering could not inline: `execute_sent` answers the
  block as a step to activate, marked as a control-flow block so the depth
  count treats it as it treats `ifTrue:`'s ([compiler.md](compiler.md), "How
  the limit counts"), or the failure argument itself if it is not a block;
- the method's own body, for a primitive method: `act` activates the method,
  whose mirror send declines again and runs the failure block
  ("Primitive methods" below).

So a failure is Newspeak code, as in Cog, where a failing primitive falls
into its method's body — whose code often sends the message on to `super`,
as every kernel method's does. `execute_sent` never answers `None`, so a
primitive entry never passes its level over any more: there is no pass-over,
and `act`'s `Primitive` arm turns a `None` it can no longer receive into the
unhandled-send error rather than a panic. The one observable consequence —
a user subclass's own `doesNotUnderstand:` is passed by, per §5.10 — is
[core-library.md](core-library.md), "The one observable difference". The
platform objects keep their shared class: a class of their own each would be
observable (`platform transcript class` answers `Platform`) and is not needed
for correctness.

**Four rows serve `Value` rather than one builtin class**, and they are where
the rule that a leaf may not send shows most plainly:
`value:sameAs:ifFail:` (reported `=`) and `valueHash:ifFail:` (`hash`) derive
a value's equality and hash from its state, and `valueSlotCount:ifFail:`
(`slotCount`) and `value:slotAt:ifFail:` (`slotAt:`) read that state
(`builtins/value.rs`). **The derivation declines; it never sends.** Comparing
two values means comparing their slots, and a slot's class may bring its own
`=` — user Newspeak the leaf would have to run, which no native may do
([core-library.md](core-library.md), "The rule: a native that calls back into
Newspeak must not stay a Rust native"). So the leaf decides only the shapes
it can decide without sending — the immediates, the text payloads, and slots
whose classes it knows answer the derived `=` — and declines everything
else, including a graph wider or deeper than its node budget, which is an
ordinary primitive failure taking the ordinary path above:
`Value`'s `ifFail:` block runs `valueEquals:` or `valueHashOfSlots`, which
walk the slots from Newspeak and send `=` or `hash` to each. The cost of the
decline is an activation and a Newspeak loop, paid only by the values that
need it; the common shapes never leave the leaf.

Bytecode and image are unaffected: the mirror's mixin is built by the
bootstrap and never serialized, so primitive numbers never reach the wire,
and neither does `Kernel.ns`, whose classes an image names only by tag.

## Primitive methods

**A kernel method whose whole body is a mirror send forwarding `self` and its
own parameters is a primitive method** (decided with the user on
2026-09-22): a send of it runs the row's leaf on the send's own receiver and
arguments, without an activation, and activates the method only when the
leaf declines — Cog's primitive methods, with the primitive recognized from a
body that means exactly what it says rather than declared by a pragma that
replaces what the body says. Without it every leaf the arithmetic fast path
does not absorb would cost an activation, a lexical send of `vmMirror` and a
`SendMirror` more than it did as a native entry
([execution.md](execution.md), "What the core library's move cost").

**Recognition** (`compiler/primitive_method.rs`, `recognize`) runs as a
method is built onto its mixin, and stores the row on the method
(`MixinMethod.primitive`). It accepts exactly
`^vmMirror <row>: self <the method's parameters, in order> ifFail: [...]`:

- the body is one statement, a `^` of the send, and declares no temporaries;
- the send is a recognized mirror send (`control_flow::recognize_mirror_send`):
  marked `Binding::MirrorSend`, a keyword selector naming a `MIRROR` row, its
  last argument a literal block with no parameters and no temporaries;
- the subject is an explicit `self`;
- the arguments between the subject and the block are the method's own
  parameters, each at its own position, by binding and not by name alone — the
  resolver's `Binding::Local` of that index, so nothing that shadows a
  parameter passes.

Anything else — a second statement, a temporary, another subject, a
parameter out of place, a row whose arity is not the method's — is an
ordinary method. Only `resolve_kernel` records `MirrorSend`, so only a
`Kernel.ns` method can be one; the same text in a user module is an ordinary
method sending to whatever its `vmMirror` holds. What the failure block
contains is not examined: it runs only when the method is activated. A method
decoded from an image never carries a row, since no image carries a kernel
method.

**The fast path is in `act`** (`interpreter/resolve.rs`), on a
`Resolved::Method` of any chain but the two handler chains: when the method
carries a row, `mirror::execute_method` runs the row's leaf with the send's
receiver, selector and arguments; `Some` is the send's answer, and `None`
falls through to the ordinary `Step::Method`, whose body then runs the leaf
again from its own mirror send and, declining, its failure block. A
`doesNotUnderstand:` handler that is a primitive method — `Object`'s — runs
its leaf on the reified message in `act_handler_method` the same way, before
it would activate. The fast path costs one field test on every method send;
the leaf's answer is delivered exactly as a native entry's was, a value or a
step.

**It applies under both lowerings**, because it is dispatch, not lowering: the
reference lowering's sends reach `act` like the optimized lowering's. It has
to. The depth count holds both lowerings to the same count of activations
([compiler.md](compiler.md), "How the limit counts"), and the reference
lowering's `SendInlinable` of `ifTrue:` reaches `Boolean>>ifTrue:`: without
the fast path that is a counted activation of the kernel method, and the
literal block its leaf then activates fails the exemption's condition, since
that activation did not create it — two counted where the optimized lowering,
which inlines the whole send, counts none. With it, the leaf answers the block
straight to the `SendInlinable`'s sender, and the exemption applies unchanged.
`goldens/kernel/recursion_through_the_core_library_counts_alike_under_both_lowerings`
pins it.

**`NS_NEXT_NO_PRIMITIVE_METHODS=1` compiles the kernel without the marks, for
measuring only.** It lets one binary run with and without the fast path, so a
measurement can report both what the move costs and what the fast path
recovers. It breaks the agreement above, so an overflow compared across the
lowerings is not valid under it, and no gate runs with it. It is read once per
process, and only once a kernel mirror send has been recognized, so no method
outside the kernel consults the environment.

**A cache needs no change.** A site whose target is a primitive method records
a `Target::Method` like any other, and the mark is read off the method in
`act` on every send, hit or miss, never decided by the entry — so a hit runs
the leaf exactly when a miss would ("Every target is a function of its key
and its site" below). One cost follows: such a site reserves feedback-table
room for the method's family, though the fast path never activates it.

## Inline caches

Every send the optimized lowering compiles consults a cache at its own call
site before running a lookup, and a miss there consults a global cache before
the lookup runs. Designed on 2026-09-15, in three steps — 3a, 3b, 3c below — each
landing green and measured on its own, and all three are built. **3a** — a
native send binds an enumerated primitive found by the ordinary lookup,
"Primitives" above. **3b** — "3b — The caches" below. **3c** — the guard reads
the dispatch id from the receiver's object header, "3c — The dispatch id in
the object header" below.

**What bounds the gain.** A cache removes the lookup and nothing else: what an
activation costs to build is untouched. Profiled at 3a's head, before the
caches, the lookup a cache can remove — deciding a send, minus the primitive,
accessor, nested-class and instantiation work it reaches — was 13.5–22.4% of
the timed samples across the seven benchmark sources
([execution.md](execution.md), "3a's head, before the caches"). Measured
whole, the caches made every source 4.3–11.3% faster than 3a's head
([execution.md](execution.md), "What the inline caches moved").

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

### 3a is built

See "Primitives" above.

### 3b — The caches

Every send the optimized lowering compiles probes a cache at its own call
site; a miss probes a global cache; only a miss there runs the lookup
(`interpreter/inline_cache.rs`). Built on 2026-09-16 by its
[plan](../superpowers/plans/2026-09-16-0950-ns-next-inline-caches-3b.md), and
refined by the step before 3c ("Decisions" below). Two rules of the design as
built here still hold and are described where they apply: a private lexical
site is guarded on N's application ("The private rule, decided once per
site"), and a block's family size is its own subtree's ("Code families").
Three others the plan settled no longer exist, and a reader of that plan
should not look for them: a vacant lexical site *may* read the global cache,
since its `Lexical` key carries N's application ("The global cache"); a
`super` send's handler target re-checks no factory, since §6.3's factory
entry and §3.2's flat metaclass leave a factory reachable only on the
receiver's own metaclass, which the class-side guard determines; and the
decoder requires no dense call-site numbering on the wire, because it assigns
a family's numbers itself ("Bytecode and image").

#### The state lives per heap

The cache entries live in a table the step state owns (`Mutator.caches`, an
`InlineCaches`), not in the shared `CompiledCode`, which stays immutable. This
is V8's split — shared bytecode, feedback per context — reached the way
HotSpot's interpreter reaches its `MethodData`: the running activation carries
an index into the table, `feedback_base`, and a site's entry is
`sites[feedback_base + call_site.number]`. It follows from invariants this
repository already holds:

- A per-heap id in shared code would mean nothing in another heap, and would
  answer a *different, valid* application there — the failure `CLAUDE.md`
  describes for handles. Kept in the heap's own table, a cache cannot be
  reached from another heap at all.
- Code shared behind an `Rc` holds no handle ([memory.md](memory.md), "What the
  collector traces"), and needs no `Cell` or `RefCell`: every step already has
  `&mut Mutator`, so an entry holds an `Rc<MixinMethod>` and a hit pays no
  borrow check.
- Invalidation is resetting one table.
- `CompiledCode` stays immutable, which suits a later actor, running on a heap
  and a thread of its own, and a stateless binary whose code need never be
  written.

The cost is bookkeeping: a stack entry and a closure (`ClosureData`) carry a
`feedback_base`, and so do `vm::run`, in a local beside the running code, and
its `Flow::Switch`. This repository has measured layout effects of 4–8% from
a single field on `Step`, so the field was measured on its own before
anything read it, and came out level with its parent on all seven sources in
two sittings ([execution.md](execution.md), "What the inline caches moved").
`Step::Method` carries a base too, in padding it already had:
`Result<Step, Signal>` stays 56 bytes in a release build.

#### The dispatch id

The id (`mixin::DispatchId`) is a dense `u32`, minted per heap by a
counter in the `Mutator` when `apply_mixin` creates an application — two per
application, one for its instances and one for its class objects' class side,
since every class object's class is the one builtin `Object` class and its behaviour
(its metaclass, the factory among its members, then `Object`'s own) depends
on its application alone. The
two are minted consecutively, so the class side's is always the instances id
plus one (`DispatchId::class_side`). `MixinApplication.dispatch_ids` keeps
the pair; a class object keeps a copy of the instances id
(`ClassData.dispatch_id`), which is where allocation and restamping read it.

**The id a guard reads is in the object header** (`Object.dispatch_id`),
stamped at allocation — an instance's from its class's instances id, a class
object's from its own class-side id — so `Heap::dispatch_id(value)` answers
an object's in one arena load, and a guard is that load and one compare, for
instances and class objects alike. It is where V8 (the map), HotSpot (the
klass pointer), Cog (the class index) and Primordial Soup (the class id) keep
their answer. For `nil`, a boolean, an integer or a float it is a table
indexed by the `Value` variant, which the bootstrap fills from the builtin
classes. The header's four bytes are paid for by boxing a class object's
data, `ObjectData::Class(Box<ClassData>)`, so an `Object` is still 56 bytes
in a release build, the size `CLAUDE.md`'s amplification figure assumes,
which a release-build test pins ([memory.md](memory.md)). It is 56 rather
than 48 because `ClosureData`, 32 bytes, is now `ObjectData`'s largest
payload.

The header holds a copy, so every place a class's application changes must
restamp: `Heap::set_class_application` walks the arena and restamps every
live instance of the class along with the class object itself. The decoder
does not use that path, since a walk per class record would be quadratic in
a count the image controls: `rebuild_applications` installs each class
record's application without the walk (`Heap::install_class_application`),
and one pass afterwards stamps every decoded object (`Heap::stamp_dispatch_ids`).
A decoded record carries `DispatchId::NONE` from the moment it is filled
until that pass — its class may not have its application yet — and nothing
dispatches on it in between. A decoded image rebuilds its applications
through `apply_mixin` ([image-format.md](image-format.md), "What is *not*
serialized"), so an id never comes from its bytes. The id is not a handle,
so the tracer does not see it and a move carries it along; `become:` will
have to exchange it with the class
([liveness-and-reflection.md](liveness-and-reflection.md)). In a debug build
`Heap::dispatch_id` recomputes the id from the class on every read and
asserts that the header agrees, so a missed restamp fails the first test
that dispatches on the stale object. Outside `heap.rs` nothing knows where
the id lives: every read goes through `Heap::dispatch_id` or
`Heap::class_dispatch_ids`. Two values are
reserved: `DispatchId::NONE`, which a value whose
class has no application answers — only a crafted image makes one — and
which is never cached, and `DispatchId::VACANT`, which guards a site state
and which no value answers. Minting stops before either; an exhausted
counter hands out `NONE`, so that application runs uncached.

#### Resolve, then act

In `interpreter/resolve.rs`, a send's decision is split in two.
`resolve_public`, `resolve_lexical` and `resolve_super` are the lookups of "The
three lookup algorithms" and "Resolving a lexical send at run time", their order
and their rules unchanged, and each answers a `Resolved`: what it found, with
its defining class as a handle, and the `Chain` it was found on — `Public`,
`PublicClassSide`, `Protected`, `ProtectedClassSide`, `Private`, `Super`, or one
of the handler chains, `DoesNotUnderstand` and `SuperDoesNotUnderstand`. The
chain says which class object the defining class is counted from. A
`Resolved` is one of:

- `Method` — the method and its defining class; a primitive method's row is
  on the method ("Primitive methods" above);
- `Slot` — an accessor's kind and absolute field, fixed by the application
  the lookup walked;
- `NestedClass` — the mixin and its declaring class;
- `Primitive` — the primitive and its level: a mirror row, found only on the
  mirror's class;
- `Factory` — §6.3's primary factory, an entry of the receiver's metaclass
  mixin, with the class object §6.3 instantiates: the receiver, whichever
  level of the metaclass chain held the entry. Found on one of the two
  class-side chains and nowhere else;
- `Unhandled` — nothing handles the send, not even `doesNotUnderstand:`.

A `doesNotUnderstand:` handler is a `Method` or `Slot` on a handler chain
rather than a variant of its own. It is never a `Primitive`: `Object`'s
handler is a kernel method, and the mirror answers no `doesNotUnderstand:` of
its own, so a `Primitive` on a handler chain, which only a crafted image could
arrange, is the unhandled send's error (`act`, `interpreter/resolve.rs`). `act` turns a `Resolved`
into the `Step`, value or signal. The uncached path is literally
`act(resolve(…))` (`Mutator::decide_send`, `sends::decide_lexical_send`,
`sends::decide_super_send`), the cached one `act` on a `Resolved` rebuilt
from an entry, so there is one place that executes a target, and a hit
cannot do anything a miss would not. **A cache records what the lookup
decided; it never decides.** Every detour a lookup takes ends in a resolution
too: a lexical send whose `lookupProtected` finds nothing falls to
`resolve_public`, class side and `doesNotUnderstand:` included, and what it
ends in is what is recorded. That is what makes the uncached reference
lowering a valid oracle for the cached one ("Verification" below). A release
`Resolved` is 16 bytes, pinned. `act` is `#[inline(always)]` into its
callers, and its handler arms are out of line and `#[cold]`, so only the hot
half is duplicated into the sends.

#### An entry

An entry (`CacheEntry`) is a guard id and a `Target`, the `Resolved` it
records with every handle taken out:

- `Method` — the method, its family's `feedback_base`, its level and its
  chain;
- `Slot` — the absolute field index, the accessor's kind and its chain;
- `NestedClass` — the mixin, its level and its chain;
- `Primitive` — the primitive, its level and its chain;
- `Factory` — its chain, and nothing else: §6.3 instantiates the receiver,
  which the guard already names, so there is no level to count;

and three states a site can be in, each guarded by `VACANT`: `Vacant`,
`Polymorphic` (a block in the pool below) and `Megamorphic`. An entry holds
no handle. The defining class is a class object per enclosing instance, so it
is not recorded but recomputed on a hit, `level` superclass steps from the
chain's start (`chain_start`): the receiver's class for `Public`, `Protected`
and `DoesNotUnderstand`, its metaclass for the two class-side chains, N itself
for `Private`, and S, the superclass of `current_class`, for the two `super`
chains. `level` is 0 in the common case, and a target more than `u16::MAX`
levels up is not recorded; nor is `Unhandled`. A release `Target` is 16 bytes
and a `CacheEntry` 24, both pinned: an `Rc` needs a pointer-sized payload
beside the 4-byte guard. A 16-byte entry that holds an index instead of an
`Rc` was measured and not adopted (below).

#### A site's states

A site's states (`InlineCaches::advance`) go vacant → monomorphic →
polymorphic, up to 4 entries in the order first seen → megamorphic, and a
megamorphic site stays so until an invalidation. A site's first entry is
inline in the table; the second receiver id it sees takes a block of 4
entries, 96 bytes, from a pool in the `Mutator` (a freed block first), moves
the first entry into it and leaves the site naming it; the fifth makes the
site megamorphic and returns its block to the free list. A miss whose guard a
monomorphic site or a block already holds — possible only when that entry
could not be rebuilt for this receiver, which only a crafted image causes —
replaces that entry rather than taking a second slot. That is Strongtalk's
and V8's shape, with V8's width. Beyond the speed of a polymorphic hit, a
site's entries are the receiver-type profile the optimizing tiers of M5–M6
will read, which is what Strongtalk's interpreter caches are for. Two kinds
of site go polymorphic only when a mixin is applied more than once: a `super`
send, whose target depends only on the running method's application, and a
lexical send the private rule decides (below). And some are polymorphic by
nature: `true` and `false` are instances of two builtin classes, so every
site that is not inlined and sees both is bimorphic.

#### The global cache

The global cache (`GlobalCache`) has 512 two-slot groups, 1024 entries in
all, keyed by (dispatch id, selector, rule) — the rule `Public`, `Lexical`,
`LexicalPrivate` or `Super` (`CacheRule`), since one id and selector resolve
differently under §5.5, §5.9 and §5.10. `Lexical` carries a payload, N's
application (its `instances` dispatch id) — `DispatchId::NONE` where it is
not yet known — so the key needs no separate component for it:
§5.9's private rule is a function of N and the selector alone, never of `o`,
so a `Lexical` entry recorded for one N is right to reuse only at a site
whose own N matches, and this is what lets a still-vacant lexical site read
the cache at all before it has resolved anything ("The private rule, decided
once per site" below). It is every site's miss path, probed before a lookup
runs, and the only cache a megamorphic site consults, as Cog's open PICs
probe the method cache and Strongtalk's miss handler its `lookupCache`. A hit
there moves the site's state on.

One hash picks a key's group — unlike 3b's two independent probes, at the
two halves of one hash, anywhere in the table — and the group's two
adjacent slots are one cache line's worth of probing. Strongtalk's
`lookupCache`, `lookup_probe`/`lookup`
(`prior-art/strongtalk/vm/lookup/lookupCache.cpp:188-205,231-242`): the read
keeps the group least recently used, not the insert. A hit at the first
slot answers in place; a hit at the second slot swaps the two before
answering, promoting the key just used. A miss (`insert`) writes a key
already at the first slot in place; otherwise the first slot's entry moves
to the second, discarding whatever the second held, and the key is written
into the first. That "otherwise" always means "not in this group at all":
every caller runs a `get` for the same key first, and a `get` that hit the
second slot promoted it, so `insert` can never find the key there. The arm
is written to handle it regardless — the displaced first entry overwrites
the second, so no group can come to hold one key twice — rather than
asserted against on the send path. The two together keep the more recently
*used*, not merely the more recently inserted, of two colliding keys — the
property 3b's insertion did not have: it evicted the first probe whatever
its use. Rejected: writing both probes, as Primordial Soup's
`InsertOrdinary` does (half the capacity); zapping the other probe on
insert, as Cog does (discards other keys for a reason particular to Cog).
`bench_megamorphic.ns` measures the probe this buys — one hash and one
cache line per send of a megamorphic site, instead of two hashes anywhere
in the table — not the eviction rule: the whole program fills 27 of 1024
entries, five of them its megamorphic `value` site's own, and 27 keys over
512 groups collide less than once on average while eviction needs three
keys in one group, so on the hot path almost certainly nothing is evicted.
An eviction-rule comparison needs cache pressure (a small
`NS_NEXT_GLOBAL_CACHE_ENTRIES`, or many more hot keys than this fixture
has), and no measurement has supplied it, so the rule rests on Strongtalk's
precedent rather than on numbers of this runtime's own
([execution.md](execution.md), "What the step before 3c moved"). An entry is
32 bytes, so the cache is about 32 KB whatever the program's size. Its
number of groups is a power of two, 512; a debug build takes a size in
entries from `NS_NEXT_GLOBAL_CACHE_ENTRIES`, which the suite uses to run
every fixture with a cache of one group, where every insertion contends for
the same two slots.

#### Code families

A family is one root body — a method, an instance initializer, a superclass
clause or its message — with every block nested in it. Its sends are numbered
densely from 0 in emission order, blocks included, and the number is the send's
`CallSite` ([compiler.md](compiler.md), "Safepoints and call-site identity").
Each body records its `family_size`, one past the highest number among its own
sends and its nested blocks' sizes. A block's is therefore its own subtree's,
not the whole family's — a block is compiled, and its `CompiledCode` finished,
before the root that contains it — which bounds every site the block can run,
all the check below needs; the root's is the family's. A family gets a
contiguous range of the table the first time its root is activated
(`InlineCaches::base_for`, keyed by the root body's identity), so a table that
code never ran stays unallocated, as V8 allocates a feedback vector lazily, and
a body with no sends takes no room. `feedback_base` reaches an activation this
way:

- A method: a hit carries its target's base in the entry, which the miss that
  filled it asked the map for; an uncached decision leaves it `NOT_ASSIGNED`,
  and `push_step` asks the map.
- A block: `MakeClosure` copies the creating activation's base into
  `ClosureData`, since both belong to one family, and the block's activation
  takes it from there — no probe per closure, which `bench_block.ns` would
  pay.
- An initializer, a clause, a clause message: the map, per push. The design
  let a `Factory` entry carry the bases of the bodies its instantiation runs,
  should the map show on the instantiation benchmarks. It is not built, and
  whether the map shows was not measured: no task isolated its cost (the
  whole step still made the three instantiation sources 4.3–7.8% faster than
  3a's head).
- A closure decoded from an image carries `NOT_ASSIGNED`, since a base is
  never serialized, and is given a fresh range of its own `family_size`
  (`InlineCaches::fresh_range`) when it is first activated, written back into
  its `ClosureData`. Any range of that size is a correct one.

#### `site_index`'s rule

A send runs uncached — the full lookup, nothing recorded — when its number is at
or past its running code's `family_size`, or when its activation's base is
`NOT_ASSIGNED`. `CallSite::UNCACHED`, the number every reference-lowering send
carries, is `u32::MAX`, which no family size reaches, so the same test makes all
of those uncached; a table that cannot grow to a family's range answers
`NOT_ASSIGNED`. Every read of the table is a checked `get`, so no crafted
artifact can make it panic or index out of range.

#### A send, step by step

A send runs step by step in `interpreter/inline_cache.rs` (`send_public`,
`send_lexical`, `send_super`). `vm.rs` passes only the running activation's
`feedback_base`, its code's `family_size` and the instruction's `CallSite`,
so it gains no dispatch logic:

1. The site's index; none means the uncached send.
2. The key (`SendKey`). `SendPublic`: `dispatch_id(receiver)`; for a class
   object that is its class side, which covers the order of metaclass —
   class-side members and the factory alike — then `Object`'s behaviour. `SendLexical`:
   `o = kth_enclosing_object(...)`, then `dispatch_id(o)`, or N's instances
   id at a private site (below). `SendSuper`: the instances id of
   `current_class`'s application; the receiver stays `self`, and a send with
   no `current_class` or no superclass runs uncached, which is where its two
   errors are raised. `SendInlinable` is emitted only by the reference
   lowering and runs uncached (`decide_send`). A non-private lexical send's
   rule carries a payload, N's instances id, `DispatchId::NONE` until it is
   filled -- computed lazily, only once a miss is certain (`miss`'s
   `lexical_of` closure), so a hit never pays for it.
3. The site's entry: if its guard matches, the target is rebuilt
   (`materialize`) and acted on. That is the monomorphic hit, inline in
   `vm::run`; everything after it is `miss`, out of line.
4. `Polymorphic`: the block's entries, in order.
5. The global cache; a hit there is acted on, and the site's state moves on.
   A still-vacant lexical site reaches this too (`send_lexical_first`), by
   probing the two rows N's decision could have filled, before it resolves
   anything ("The private rule, decided once per site" above).
6. `resolve`; its resolution is recorded in the global cache and at the site
   (`fill`), then acted on. A key whose id is `NONE` is never recorded.

A target that cannot be rebuilt for this receiver — a level past the top of
its chain, which only a crafted image makes — is a miss.

What `act` does, per resolution: `Method` runs a primitive method's leaf
first, read off the method ("Primitive methods" above), and otherwise answers
`Step::Method` with the target's `feedback_base`; `push_step` still checks
the arity. `Slot`
reads or writes the field without an activation (`decide_slot_accessor`).
`NestedClass` calls `nested::nested_class` with the declaring class; creating
one still runs its clause as a step. `Primitive` calls `builtins::execute`,
which for a mirror row never answers `None` — a declining leaf runs the send's
failure block ("A primitive can fail" above). An entry whose primitive
method's leaf declined stays, as Cog keeps a method whose primitive
failed. `Factory` instantiates the receiver (`instantiate::decide_factory`),
re-checking nothing the lookup decided — §3.2's flat metaclass leaves the
entry reachable only on the receiver's own metaclass, which the class-side
guard determines — but reading `ClassData.builtin` off the receiver on every
send, because `Array` and `OrderedCollection` need an `ObjectData::Array`
payload and no entry carries which kind of instance to build ("Every target is
a function of its key and its site" below). A
handler target builds the message array per send and calls the handler, and
nothing is checked ahead of it.

#### Every target is a function of its key and its site

That is the property the entries rest on, and a lookup that also reads something
the key does not cover re-checks it in `act`. The plan audited every read
`resolve` and its callees make: the only ones the key does not cover are
`ClassData.builtin`, which `decide_factory` re-reads on every send off the
receiver rather than trusting an entry, and a primitive's payload check,
which `builtins::execute` re-runs on every send. Whether a method is a
primitive method is not such a read: it is a property of the method the entry
records, and `act` reads it off that method on every send. A class object's factory
selector is no longer among them: §6.3 makes it the selector the entry is
filed under in the metaclass's method table, which the guard covers.

The audit also covers the class-side fall-through: once the metaclass lookup
finds nothing, `resolve_public` re-reads the class object's own `class` field
(`class_of`) to reach `Object`'s methods, and that read is outside the key
too — the class-side guard is
the metaclass's dispatch id, never the class object's own. Built from source
this is never a problem: every class object's class is the one builtin
`Object` class, so the field never varies. A crafted image could still give a class object a
different `class` than the guard assumes; the result is wrong but safe, never
a panic, because the level walk from there runs through the same
`Option`-returning steps (`metaclass_of`, `class_of`) as any other lookup.

#### The private rule, decided once per site

Whether §5.9's private rule applies depends on N alone, the class declaration
`k` levels out from the running method's class, and it is not known at a
still-vacant site until N is. What a first resolution finds decides the site
for good: a `Private` resolution makes it a private site, anything else a
non-private one, and its transitions keep the decision. A non-private site is
guarded on `o`'s id, and a hit never computes N. A private site is guarded on
N's instances id, under the `LexicalPrivate` rule, and computes `o` and N
(`kth_lexical_class`) on every send. It is guarded rather than fixed because
one mixin applied under two superclasses gives N two applications with
different inherited slots, so a private slot accessor's absolute field
differs between them; such a site goes polymorphic on N's applications,
however many classes `o` has. The guard is also what makes a crafted image
safe: a site whose N changed simply misses, and `fill` refuses to record a
resolution whose privacy is not the site's, so a crafted site gets wrong but
safe behaviour, never a panic. `image::decode::verify_mixins` therefore needs
no check that code and N agree: nothing assumes a relation between them.

**A still-vacant lexical site may read the global cache**, and what makes that
safe is N's application, carried as the `Lexical` rule's payload
(`CacheRule::Lexical(DispatchId)`, `DispatchId::NONE` where it is not yet
known; every other rule needs none). Without it an entry keyed by `o`'s id
alone could have been left by a site whose N declares nothing private, and be
wrong for a site whose own N does. With it, a hit under
`(o, selector, Lexical(N))` is right regardless of which site left it, since
§5.9's private rule is a function of N and the selector alone, never of `o`.
So `send_lexical_first` probes two rows before resolving anything — N's own
`LexicalPrivate` row, keyed by N alone, then the `Lexical` row keyed by `(o,
N)` — and resolves independently only when both miss. A monomorphic *hit* at
an already-decided non-private site still never computes N: `cached` hands
`miss` a `lexical_of` closure that computes it lazily, called at most once,
only once a miss is certain.

#### No new error text

Every error comes from the same paths as before the caches, inside `resolve` or
`act`, word for word; a hit produces exactly the step, value or signal the
uncached path would.

#### Invalidation

Nothing mutates a method table at run time yet, so nothing has to be
invalidated, but the mechanism exists, because the entries are designed for it:
they hold no handle, so `InlineCaches::invalidate`
(`Interpreter::invalidate_caches`) vacates every site, clears the pool and its
free list, and vacates the global cache, keeping every family's range, since
running activations keep their bases — the next send through a vacated site
fills the same index again. A debug-build stress mode,
`NS_NEXT_STRESS_CACHES=1`, invalidates at every safepoint
(`interpreter/safepoint.rs`, ahead of the move stress mode's schedule, either
mode keeping the safepoint request set), so that filling, transitions and
invalidation run under the whole suite; `Interpreter::cache_invalidations`
counts them. Scoping an invalidation to the application a change affects — the
dependency key of [liveness-and-reflection.md](liveness-and-reflection.md),
"Caches and their dependency keys" — comes with the first mutation that needs
it.

#### What stays uncached

A host's `Interpreter::send`, a send a continuation decides (the `mustBeBoolean`
a non-Boolean loop condition is sent), and every send the reference lowering
compiles run the full lookup and record nothing.

#### Bytecode and image

No instruction changed shape except in the width of its site:

- `CallSite` is the send's number within its family, a `u32`. It was the
  instruction's index as a `u16`, which the optimized lowering checked with a
  `debug_assert!` only, so a very large body could alias two sites in a
  release build; read by a cache, two sends would share one entry and the
  second would run the first's target. The compiler checks a family's count
  in every build.
- The four sends are 25–28 ([image-format.md](image-format.md), "The
  instruction encoding"); 7–10 and 21–24 are retired, 7–10 because a send's
  call site was a `u16` instruction index and 21–24 because the number was
  serialized as a `u32` at all. A stale artifact meets a retired tag and is
  refused.
- No call-site number is on the wire ("Call-site numbers are not serialized"
  below). The decoder assigns every family's numbers once the whole code
  table is read, in the compiler's own emission order, so a family's numbers
  are dense by construction rather than checked on decode; a code record
  carries a flag for a family the reference lowering compiled uncached, in
  place of the call-site list that used to guard a stale body. The family
  size is derived from the sends' numbers the same way on both sides
  (`bytecode::family_size_of`, which the compiler and the decoder both
  call), never read from the artifact. That bounds the range a crafted
  family's first activation can allocate by the family's own bytes, and
  keeps two sends from sharing an entry.
- The reference lowering emits every send with `CallSite::UNCACHED`, so
  everything it compiles runs the full lookup on every send.
- No cache state and no `feedback_base` is ever serialized; a loaded artifact
  starts with empty caches.

#### Verification

The oracle: the reference lowering runs uncached and the optimized lowering
cached, and both are held to every `goldens/<category>/*.ns` and the outcome
recorded beside it, and to `run-corpus` from source and
as a binary, so a cache defect shows as the two lowerings disagreeing. The seven
goldens under `goldens/cache/`, added before any
cache existed, pin the outcomes a cache could change. `NS_NEXT_STRESS_CACHES=1`
is a gate of its own beside `NS_NEXT_STRESS_MOVES`; the two are not combined,
because an entry holds no handle for a move to break, and emptied caches would
leave the moves hardly any hit to run under. The suite takes about 1 min 5 s
under it, against about 3 min 9 s under the move stress mode. A test that
observes cache states turns the mode off for its own interpreter
(`Interpreter::set_stress_caches(false)`), so that it still runs, and asserts,
under the gate. Targeted tests pin, through a test view of a site's state
(`Interpreter::call_site_states`), in
`crates/ns-runtime/tests/inline_caches.rs`, `stress.rs`, `image.rs` and
`opcode_tags.rs`, `inline_cache.rs`'s unit tests and
`crates/ns-cli/tests/run_goldens.rs`:

- two `Inner` instances from two `Outer` instances sharing one monomorphic
  entry — the requirement "How other systems cache sends" ends with;
- the transitions to polymorphic and megamorphic, and a bimorphic site on
  `true` and `false`;
- a slot accessor under two applications of one mixin with different
  inherited slots, each with its own field, as `vm_initializer.rs` pins
  `InitializeSlot`;
- `super` in a mixin applied under two superclasses;
- a private lexical site with a changing `o`;
- class objects: a factory send, a class-side method and `Object`'s
  behaviour, at one site that also sees instances;
- `doesNotUnderstand:` with `Object`'s handler, a user handler, and the
  setter of a slot named `doesNotUnderstand`;
- a cached primitive that fails;
- a second site filled from the global cache, and, for a lexical send whose
  own site was still vacant, sharing one entry under N's application rather
  than each resolving independently;
- invalidation at every safepoint of a run, with the answer unchanged;
- a snapshot taken with the caches full loading with them empty, and a
  decoded closure getting its base;
- collisions in the global cache, and every fixture answering alike with a
  one-entry global cache;
- the new send tags, and a retired one refused.

`ns-next run` and `bench` take `--cache-statistics`, which prints the
table's sizes to stderr after the run (`Interpreter::cache_statistics`):
sites allocated and filled, polymorphic blocks, megamorphic sites, global
entries filled, and the two entry sizes.

#### Measurement

The figures are in [execution.md](execution.md), "What the inline caches
moved". `feedback_base` alone, the split into `resolve` and `act`, the
caches, and the whole step were each measured in two sittings, against 3a's
head and `be3eb40`. Whole, the step is 4.3–11.3% faster than 3a's head and
9.4–19.7% faster than `be3eb40` on all seven sources, every group entirely
below. After the benchmarks the table holds 12–16 sites and the global cache
9–13 entries; after each `run-corpus` module, at most 2 of each; no site went
polymorphic in any of them. That measurement closes the open finding that a
lexical send's decision is sensitive to codegen ([PROGRESS.md](../PROGRESS.md)).

The five refinements that followed ("Decisions" below) were measured the same
way, whole and one change at a time, in [execution.md](execution.md), "What
the step before 3c moved": the three instantiation sources 2.1–4.1% faster
than the step's base, the other five 0.2–3.6% slower, every source 6.7–18.2%
below `be3eb40`, and `vm::run` still one function with no `vm::run::closure`
out of line.

#### Considered, not adopted: a 16-byte entry

`Target::Method` and `Target::NestedClass` holding a `u32` index into side
tables of methods and mixins instead of an `Rc` make a `Target` 12 bytes and an
entry 16, so a polymorphic block fits one 64-byte cache line. Measured against
the 24-byte entry, no source was faster, `bench_send.ns` was entirely slower in
one of two sittings, and the side tables grow with every genuine miss rather
than with distinct methods, so a megamorphic site cycling through more shapes
than the global cache holds would grow them without bound until an invalidation.
The user decided on 2026-09-16 to keep the 24-byte entry; the experiment
was not merged.

#### A standing hazard

The run loop's step sits near LLVM's inlining limit.
Every cached send inlines its monomorphic hit into `vm::run`'s
per-instruction step. On the design's own shape the step outgrew the
inliner's threshold and became a function of its own, so that every
instruction paid a call and a `Flow` returned through memory — the cliff
splitting `resolve` from `act` first hit, at 6.6–13.6% on every source. So the
step closure is `#[inline(always)]`, and `decide_super_send`, which only an
uncached `super` send reaches, is `#[inline(never)]`. Forcing the step buys no
speed of its own — on 3a's head it moved no source beyond noise — it keeps
the caches' gain. Anything later added to a send's hit path lands in
`vm::run`, and a change there checks that the step is still inline
(`dumpbin /disasm`, [execution.md](execution.md), "What the inline caches
moved") before timing anything. The structural answer, planned for later,
is one `Interpreter::enter` per stretch between two safepoints rather than
one per instruction ("Decisions" below).

### 3c — The dispatch id in the object header

**Built** (2026-09-19, the user's decision), by its
[plan](../superpowers/plans/2026-09-18-2215-ns-next-dispatch-id-place-3c.md):
the id a guard reads is in the object header, and a class object's data is
boxed to pay for it ("The dispatch id" above). Only where
`Heap::dispatch_id` reads the id changed; the guards, the entries and the
global cache did not.

Two variants were built and measured against `393311f`, the last commit
before 3c that changed a send's path, where the id lived on the class object
(`ClassData.dispatch_id`, in padding the struct already had) and an
instance's guard cost two arena loads, a class object's one load and an add:

- **C2, the id in the header.** `Object` gains `dispatch_id`, and grows from
  56 to 64 bytes in a release build (96 to 104 in a debug one); the
  amplification a crafted image can reach at the graph section goes from
  about 16.9x to about 18.2x, and `bench_block.ns`'s peak working set
  rises 2.6%.
- **C2+, the id in the header and `ClassData` boxed out of `ObjectData`.**
  `Object` stays 56 bytes in a release build and falls to 80 in a debug one,
  the amplification stays about 16.9x in release and falls to about 22.2x in
  debug, and the working set is level. The design had expected 48 bytes; that
  expectation predates 3b, after which `ClosureData` became `ObjectData`'s
  largest payload. Cog's class table is the alternative to a box, and buys
  nothing here: this id is per application, not per class object.

**Neither variant is faster than `393311f` by the protocol's gate.** C2+'s
means moved −0.4% to −2.8% in 14 of its 16 source-and-sitting cells, and
`bench_block.ns` +0.1% / +0.2%, but no source is entirely below
`393311f` in both sittings for either variant — only single-sitting results,
which the protocol does not count. `vm::run`'s step stayed inline in both
([execution.md](execution.md), "What 3c moved", which has every number).

**The proposed rule would have kept the id on the class object**: the faster
wins; within noise, the smaller; if neither beats `393311f`, the id stays
where it is. **The user chose C2+ instead, on architectural grounds** the
interpreter's measurement cannot see:

- **A JIT tier's guard.** With the id in the header, a compiled guard is one
  compare against the receiver at a fixed offset. On the class object it is
  two dependent loads, a check that the class is a class object, and a branch
  for the class side's plus one. V8, HotSpot, Cog and Primordial Soup all keep
  it in the header. In the interpreter the difference is lost in the run
  loop's own overhead, which is why the protocol cannot show it.
- **Step 6 makes the stamping uniform.** Once `Class`, `Behavior` and
  `Metaclass` are reified and a class object's class is its metaclass
  ([object-model.md](object-model.md), "Class sides and metaclasses"), every
  header id comes from the object's class, and the class-side special case in
  the stamping goes.
- **Between the two header variants, C2+ is the smaller for the same
  speed**: C2 costs 8 bytes an object, the amplification and the working set
  for nothing measured. The open question beside it — whether closure
  conversion (step 4) would shrink `ClosureData` to 24 bytes or less and take
  `Object` to 48 — is settled the other way: the conversion rebuilt
  `ClosureData` and it is still 32 bytes (`types.rs`), because the boxed slice
  of copied values takes the room the parameter names and the home freed. So
  `Object` stays 56, which `heap.rs`'s release-build size test pins, and
  `ClosureData` stays `ObjectData`'s largest payload
  ([execution.md](execution.md), "Converted representation";
  [memory.md](memory.md), "The interim cost, and the one question still
  open").

**The costs accepted.** One more indirection on every read of a class's data
— every level `kth_enclosing_object` walks, the start of a `super` send,
`metaclass_of`, the miss path, instantiation — measured level on
`bench_new.ns`, `bench_new2.ns`, `bench_newarg.ns` and `bench_block.ns`; an
allocation per class object; and a copy that must be restamped wherever a
class's application changes (`Heap::set_class_application`, the decoder's
one stamping pass), which `become:` will have to honour too. A debug build's
check in `Heap::dispatch_id` catches a stale copy in every test that
dispatches on one.

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
- **Decided by the user (2026-09-19), planned with the other specification
  gaps:** a member set belongs to its declaration, not to a name (**built**;
  see "A member set belongs to its declaration, borrowed down the walk"
  above); and a setter send passes by every local that declares no setter --
  parameters and immutable locals -- as the canonical compiler does, rather
  than refusing at compile time as SOMns does (**built**; see "Setter sends"
  above).
- **A slot's accessors are methods** (§6.3.2), in the method table, answered
  without an activation.
- **`doesNotUnderstand:` bypasses access control**, because `Object`'s own
  implementation is protected and would otherwise be unreachable.
- **Unmarked members are protected**, per §6.3.1, which decides public
  reachability rather than only documenting intent.
- **Dispatch lives in `interpreter/`, once.** Duplicating it into the VM is the
  failure mode this rule exists to prevent.
- **Primitives are method-table entries** (3a, 2026-09-15), bound as an
  enumerated primitive, so natives are members found by the ordinary lookups,
  with an access modifier, as in Cog and Primordial Soup. Since step 6 they
  sit only on the VM mirror's class. Built; see "Primitives" above.
- **Primitive methods** (2026-09-22): a kernel method whose whole body
  forwards `self` and its parameters to one mirror row runs the leaf in
  `act` without an activation, under both lowerings, recognized from the
  body rather than declared, and only in `Kernel.ns`. Built; see "Primitive
  methods" above.
- **Inline caches per heap** (3b, designed 2026-09-15, built 2026-09-16) —
  monomorphic, a polymorphic cache of 4, then megamorphic, with a global
  cache of 1024 entries behind every site — guarded on the mixin
  application, never on the class object. A cache records what the lookup
  decided and never decides: `resolve`, then `act`, and a hit is `act` on a
  rebuilt resolution. Built; see "3b — The caches" above.
- **Confirmed by the user (2026-09-17), as built in 3b:**
  - **A private lexical site is guarded on N's application**, not left
    unguarded as first designed: one mixin applied under two superclasses
    gives a private slot accessor two fields, and the guard keeps a crafted
    image from making the site do anything wrong. Rejected: an unguarded
    site with a mixin-relative slot index (nothing would check that N is
    still the recorded mixin, so a crafted image gets a foreign method or
    field); not caching private sends (a probe of N's method table on every
    private send, the common case). Kept open as a later option: guarding on
    N's *mixin* through a per-heap mixin id kept on the application, with a
    mixin-relative slot index — the site stays monomorphic when a mixin is
    applied under several superclasses, at a second load per private hit;
    worth measuring against the built guard once a program has that shape
    (none of the measured sources did).
  - **A block's `family_size` is its own subtree's**, which bounds every site
    a block can run, computed by one function in compiler and decoder.
    Rejected: the whole family's size on every body (a second pass, and a
    decoded closure's fresh range grows for nothing); the range's end carried
    at run time (larger closures and activations, and a decoded closure still
    needs a size); blocks pointing at their root (a reference cycle or a
    table, and a load per block send).
  - **A class object stores one dispatch id**, the instances id, and derives
    the class side's as the next. Rejected: both ids on `ClassData`; the ids
    only on the application (a further load on every send). Since 3c a guard
    reads the id from the object header instead, and `ClassData`'s copy is
    what allocation and restamping read.
  - **`vm::run`'s step is forced inline**, and `decide_super_send` kept out
    of it: the caches' hit path pushes the step past LLVM's inlining
    threshold, where every instruction pays a call. Rejected: keeping the
    step small enough without the attribute (fragile against any change and
    any LLVM version); profile-guided builds (the fragility moves into the
    profiles). Planned for later ("A standing hazard" above): one
    `Interpreter::enter` per stretch between two safepoints instead of one
    per instruction, so that no per-instruction boundary exists at all — to
    be looked at before a send's hit path grows further, and at the latest
    with the JIT tiers.
- **Decided by the user (2026-09-17, extended 2026-09-18), built before 3c
  (2026-09-18):** the factory, the name conflict it can raise and the flat
  metaclasses came out of one piece of design work on 2026-09-18, which §3.2
  settled from a section the earlier reading had not reached. All five are
  built; what the step measured is in
  [execution.md](execution.md), "What the step before 3c moved", and
  summarized under each decision below.
  - **The global cache's lexical key includes N's application**:
    `(o's id, selector, lexical, N's application id)`; private entries stay
    keyed by N's application alone. Whether §5.9's private rule applies
    depends only on N and the selector, so every entry under such a key is
    right for any site, and a vacant lexical site may use it — Primordial
    Soup keys its implicit-receiver cache by the calling method for the same
    reason (`prior-art/primordialsoup/vm/lookup_cache.h`, `LookupNS`). It
    replaces 3b's rule that a vacant lexical site skips the global cache.
    Rejected: a compile-time private flag (adding or removing a private
    member live would need its senders recompiled); probing N's table on a
    global hit (about the cost of the lookup it saves). Built, and it costs
    the send benchmarks: `bench_loop.ns` and `bench_send.ns` came out
    2.1–3.3% slower than the commit before it, each entirely above in one of
    two sittings, and `bench_megamorphic.ns` 0.9–1.5%. A control binary — the
    commit with the two probes cut out and everything else kept, whose
    `vm::run` is instruction for instruction the commit's — puts the cost on
    **the widened key, not the probes**: the control alone reproduces it, and
    adding the probes on top moves no source beyond the noise, which confirms
    by measurement that a probe running once per still-vacant site cannot be
    it. The mechanism is the eighteen instructions the key's fourth component
    adds to the step every send runs, not work done per send; it is not entry
    size, which is unchanged ([execution.md](execution.md), "Which half of
    the lexical key costs the send benchmarks").
  - **The primary factory is a method of the class side** (§6.3: "The class
    side, together with the primary factory method, defines the mixin for
    the metaclass"), an entry of the metaclass mixin's method table, found by
    the ordinary lookups — as both references build it (Primordial Soup's
    `astForPrimaryFactory:` in
    `prior-art/primordialsoup/newspeak/Newspeak2PrimordialSoupCompilation.ns`,
    SOMns's `MixinBuilder.assemblePrimaryFactoryMethod`). 3b's special cases
    go, and **nothing takes their place**: the factory check ahead of lookup,
    the `Factory` target and its re-check, and the factory re-check ahead of
    a `super` send's `doesNotUnderstand:` target. The re-checks are
    unnecessary once metaclasses are flat (the decision below, taken with
    this one): a factory entry is then reachable only on the receiver's own
    metaclass, which the class-side guard — that metaclass's dispatch id —
    determines, and never on a `super` chain or through inheritance. The
    builtin branch of 3b's pre-lookup factory check goes into the entry's action, since
    `Array` and `OrderedCollection` need an `ObjectData::Array` payload rather
    than a generic instance. It does **not** leave the reads a key does not
    cover, as the decision expected it would: `decide_factory` reads
    `ClassData.builtin` off the receiver on every send, which is what keeps
    the entry safe rather than the key covering it ("Every target is a
    function of its key and its site" above). What did leave that list is a
    class object's factory *selector*, which is now the selector the entry is
    filed under. `primary_factory_selector` stays only where §6.1 needs it, in
    the superclass clause's check, which does not change. Built; with the flat
    metaclasses it landed with, the three instantiation sources came out
    3.3–4.6% faster, entirely below in both sittings, since `X new` is now an
    ordinary cached send rather than a special case tested ahead of every
    lookup. The same commit cost `bench_do.ns` 2.8–3.3%, entirely above in
    both sittings, and that has no mechanism: it neither instantiates nor
    sends to a class object in its loop. It is this repository's recurring
    uniform shift on code a change does not execute, which the
    function-alignment pin does not remove
    ([execution.md](execution.md), "What the step before 3c moved").
  - **A class side may not declare the primary factory's selector**: the
    resolver refuses it, in the check that already reports a name declared
    twice on one side (`resolver::classes::duplicate_member_names`), since
    §6.3 makes the factory a member of that same metaclass mixin and a method
    table holds one entry per selector. Primordial Soup refuses it too
    (`Intermediates.ns`'s `checkNameConflictsForFactory:`, which is
    `checkForMethod:`). Rejected: SOMns's rule, where the declared class-side
    method wins and the factory is filed under an unreachable name
    (`MixinBuilder.addFactoryMethod`, the `isPrimary` branch, deliberately,
    so that `ValueArray` can delegate to a primitive) — the class then no
    longer instantiates and nothing says so. Built; a resolver check costs no
    send anything at run time.
  - **Metaclasses are flat**, so **a class side is not inherited**. §3.2:
    "All metaclasses are instances of class Metaclass, and are direct
    subclasses of Class. Metaclasses may not be subclassed. […] The metaclass
    hierarchy in Newspeak is deliberately kept very flat"; §6: a class is an
    instance of "a metaclass that is the application of the metaclass mixin
    defined by the class' class side to the class Class". ns-next applied a
    class side to the *superclass's metaclass*, Smalltalk-style, until this
    step; that deviation is what made an inherited factory reachable at all,
    which is why this was decided together with the factory rather than after
    it. All three Newspeak
    implementations build it flat: Primordial Soup's `superclassOf:
    newMetaclass put: Class` (`KernelForPrimordialSoup.ns`, the rule spelled
    out at its `Mixin` comment), SOMns's class side over `new SClass[]
    {Classes.classClass}` (`MixinDefinition.createClassFactory`), and the
    canonical compiler applying "the runtime metamixin to […] the runtime
    class of `Class`" (`Newspeak2JSCompilation.ns`). Since ns-next reifies no
    `Class`, a metaclass's superclass becomes `Object`'s behaviour — the
    fall-through the class-side chain already ends in, reached directly
    instead of after a walk. A class-side chain then finds a member on the
    receiver's own metaclass or not at all, and §5.10 in a class-side method
    looks in `Object`'s behaviour. Built; the two recorded outcomes that
    pinned the inheritance were re-cut to pin the conforming rule instead.
    `goldens/class-side/class_side.ns` sent `SpecialWidget describe` for an
    inherited class-side method; its subclass now declares its own
    `describe`, since a class side that declares none answers nothing. A
    cache golden rested on
    a class-side `doesNotUnderstand:` declared by a superclass, which a flat
    metaclass makes unreachable; it is gone, replaced by
    `goldens/class-side/class_side_super_reaches_object.ns`, whose class-side
    `super printString` answers `Object`'s: §5.10 makes S the superclass of
    the running activation's current class, which §3.2 makes `Class` for a
    metaclass, and `Object`'s behaviour is what stands in for `Class` here.
    The rest of §3.2 — the reified instance-of relation and the
    `Class`, `Behavior` and `Metaclass` kernel classes — is scheduled for
    step 6, the core library in Newspeak ([object-model.md](object-model.md),
    "Class sides and metaclasses").
  - **Call-site numbers are not serialized**: the decoder numbers each family
    in the compiler's order — a body's sends in order, a block's at its
    `MakeClosure` — so a number is a property of the code, never read from
    an artifact. The send tags 21–24 retire for new numbers; the density
    check and `CallSitesNotDense` go; a code record carries a flag for a
    family compiled uncached, so that an artifact built with the reference
    lowering still runs uncached. A block precedes its parent in the code
    table, but its numbers depend on its place in the parent, so the decoder
    numbers a family once its root is read, rebuilding the blocks' bodies; a
    test pins that decoded numbering equals the compiler's, the golden image
    round trip included. The code record's empty call-site list, whose only
    job was to refuse a stale body, can go, since the retired send tags do
    that; the plan settled it, and the list is gone. Rejected alternatives to
    the refusal 3b built: per-family hash tables (a hash per hit, and shared
    numbers stay unsafe); the selector in every entry (a compare per hit, and
    an oversized number still needs bounding). Built, and the one change of
    the five a run from source never touches: it moved no source beyond the
    noise, −1.2% to +1.0%, and left `vm::run` the same size to the
    instruction ([execution.md](execution.md), "What the step before 3c
    moved").
  - **The global cache is set-associative with two slots a group, least
    recently used out** (Strongtalk's `lookupCache`,
    `prior-art/strongtalk/vm/lookup/lookupCache.cpp`): 512 groups of two
    adjacent slots; a miss moves the first slot's entry to the second and
    writes the first; a hit in the second swaps them. It replaces 3b's
    insert, which evicts the first probe whatever its use. Rejected: writing
    both probes (Primordial Soup; half the capacity); zapping the other
    probes on insert (Cog; discards other keys for a reason particular to
    Cog). Built, and measured against 3b's insert with
    `goldens/bench/bench_megamorphic.ns`, added for it because none of the
    existing sources reaches a megamorphic site. **The replacement rule
    itself is unmeasured, and that benchmark cannot measure it.** It fills 27
    of the 1024 slots, five of them at the one megamorphic site it drives, so
    across 512 groups its hot keys almost certainly share no group: almost
    every hit lands in the first slot, the second is almost never read, and
    almost certainly nothing is evicted. What it measures is the probe path —
    one hash and one cache line against 3b's two hashes anywhere in the table
    — and on that the change moved no source beyond the noise:
    `bench_megamorphic.ns` +1.16% / +0.41%, every group
    overlapping in both sittings ([execution.md](execution.md), "What the
    step before 3c moved"). A benchmark that crowds one group — more live
    keys than 1024, or a deliberately colliding set — is what would decide
    the rule against 3b's insert on numbers rather than on Strongtalk's
    precedent.
- **An entry holds an `Rc` and is 24 bytes** (2026-09-16, the user's
  decision): the 16-byte variant with an index into side tables measured no
  faster and grows those tables with misses ("Considered, not adopted: a
  16-byte entry" above).
- **The dispatch id is in the object header, with `ClassData` boxed** (3c,
  the user's decision, 2026-09-19): measured against the id on the class
  object and the id in the header alone, in two sittings, and **chosen
  against the rule proposed with the measurement**. That rule — the faster
  wins, within noise the smaller, and the id stays on the class object if
  neither beats `393311f` — would have kept the class object, since neither
  header variant cleared the gate in both sittings on any source. The user
  chose the header for what the interpreter cannot measure: a JIT tier's
  guard is one compare at a fixed offset instead of two dependent loads, a
  class check and a class-side branch, as in V8, HotSpot, Cog and Primordial
  Soup; step 6's reified metaclasses make the stamping uniform; and boxing
  `ClassData` keeps `Object` at 56 bytes where the header alone grows it to
  64. Accepted: a further indirection on every read of a class's data, an
  allocation per class object, and restamping wherever a class's application
  changes ("3c — The dispatch id in the object header" above;
  [execution.md](execution.md), "What 3c moved").
