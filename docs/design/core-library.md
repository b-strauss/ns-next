# Core library and the platform

> **Status:** partial by design — the surface grows on demand, not
> speculatively.
> **Code:** `crates/ns-runtime/src/builtins/`, `platform.rs`.

The core library is native Rust today, not Newspeak source. That is the choice
that lets an image stay small and keeps a hostile image from replacing a builtin
class, and it is also the choice that means a modified platform is a different
`ns-next` binary rather than a different artifact. See the comparison in
[image-format.md](image-format.md).

**Step 6 moves the library to Newspeak without giving either up**: the source
ships with the runtime and is compiled at bootstrap rather than carried in an
artifact, and the leaves stay Rust — reached through one VM mirror object, as
§8.3.1 requires. "Planned: the core library as Newspeak source over a VM
mirror" below is the design.

## What exists

`Object`, `Behavior`/`Class`, `Boolean`/`True`/`False`, `Integer`, `Float`,
`String`, `Symbol`, `Character`, `Array`, `OrderedCollection`/`List`, `Nil`,
`Block`.

A minimal `platform` object exposes these plus a stdout stream object, enough for
transcript-style output.

## The on-demand rule

**`platform.rs` is widened only on demand.** An accessor exists because a module
on `corpus/runnable.toml` needs it, and the whitelist entry that demanded it is
named in the accessor's comment.

This is not minimalism for its own sake. A capability nothing on that list asks
for has no test that would catch it rotting, so it is a liability rather than an
asset. A module that needs something nobody has asked for is a reason to widen
the whitelist deliberately — with a real module, run end to end, its output
recorded — not a reason to build a speculative library.

The `collections` namespace is the worked example. It has exactly one member,
`List`, because exactly one is asked for; `List` names the growable ordered
sequence the runtime already has under its Smalltalk name `OrderedCollection`, so
it is a name for an existing class rather than a new implementation. The corpus
also asks that namespace for `Map`, `IdentityMap`, `IdentitySet`, `WeakArray` and
`Collection`. Those name classes the runtime does not have, and inventing them —
or worse, aliasing them onto `OrderedCollection`, which is neither a dictionary
nor a set — would be a collections library built without a caller. Modules
needing them stay off the whitelist.

## Tagging inside the platform

The platform singleton, its `transcript` stream and its `collections` namespace
all share one builtin-class tag and are told apart by payload shape: zero fields
for the singleton, one for the stream, two for the namespace. None ever holds
real slot data; the field count is purely a tag.

This is a wart, and it is recorded rather than hidden. It exists because there was
no second builtin-class slot to spare, and adding one would renumber the image
format's builtin-class table for no behavioural gain. Natives are method-table
entries ([dispatch.md](dispatch.md), "Primitives"), so all three objects'
selectors sit in the one `Platform` table, and a selector sent to the wrong one
of them is a primitive failure, which continues the lookup at `Object` exactly
as any other primitive's failure does. They keep the shared class, since
`platform transcript class` answering `Platform` is observable.

## Deviation: `i64` integers

Integers are `i64`. Overflow raises a Newspeak error rather than promoting to a
bignum. Arbitrary-precision integers are not built. §5.1.1 says the opposite in
one sentence — "*Note that integers are not limited in size*" — and puts no
bound on the *n* in the digit sum it defines a literal by, so a literal past
`i64`'s range is an ordinary `Integer` there and a deferred
`Literal::MalformedInteger` here.

This is observable to any program that overflows, which is why it is a recorded
deviation rather than an implementation detail.

## Deviation: a non-integral literal is a binary float

§5.1.1: "Non-integral numbers denote an object that obeys the protocol of class
`FixedPointNumber`" — an exact decimal type, chosen there so that "financial
applications in particular" get "accurate decimal numbers" and users are not
exposed to floating point's "counterintuitive ways". `2.5` here is an `f64`
(`compiler/lower.rs` parses the literal's text straight into one), so the
arithmetic is binary floating point: `(0.1 + 0.2) = 0.3` answers `false` where
an exact decimal type answers `true`.

The specification records the same divergence in its own implementation
("non-integral numbers are currently represented as instances of the Squeak
class `Fraction`. This is a bug"), and it names the open design question —
whether a user chooses the numeric implementation — rather than settling it.
A golden for the rule would have to be skipped behind a whole numeric tower,
so the deviation is recorded here instead.

## Gap: an integer has no `/`

`7 // 2` answers `3`; `7 / 2` ends in `doesNotUnderstand:`. The selector is
registered only on `Float` (`builtins/primitive.rs`:
`public("/", Primitive::Float(FloatPrimitive::Divide))`), while `Integer`
carries `//` alone, as `FloorDivide`. So `7.0 / 2` works and `7 / 2` does not,
which is asymmetric enough to surprise.

**This is not a specification violation.** The specification mentions `Number`
exactly twice — §3.1.1, listing it among the value objects, and §5.1.1, on
non-integral *literals* — and fixes no arithmetic protocol anywhere. Which
selectors `Integer` answers is a library question, and §8.3 leaves the
platform's modules to provide classes "some of which are standard and mentioned
in this specification". `/` is not one of them. Formally this is the on-demand
rule doing its job: nothing on `corpus/runnable.toml` asks for it.

It is recorded because **step 6 has to settle it rather than rediscover it**.
Primordial Soup declares both on `Integer`
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`): `/` as a
primitive falling back to `^other divideFromNumber: self`, and `//` beside it,
commented "Integer division, rounded toward negative infinity". That works
there because Soup has a `Fraction` class, which its comparison methods test
for. ns-next has no `Fraction` and no exact decimal type, so writing
`Integer>>/` into `Kernel.ns` means choosing: answer a `Float`, which diverges
from Smalltalk and from Soup and quietly makes an exact quotient inexact; build
the numeric tower the deviation above already defers; or keep the selector
absent and say so. The choice belongs in step 6's plan, made deliberately.

## Planned: the core library as Newspeak source over a VM mirror

The shape of a real collections library, of arbitrary-precision integers, and of
the mirror-based reflection protocol the platform would hand out are all open;
the last of those is sketched in
[liveness-and-reflection.md](liveness-and-reflection.md). Beyond that, "widen
when a whitelisted module demands it" still governs what gets added.

**This step also carries the rest of §3.2** (decided 2026-09-18): `Class`,
`Behavior` and `Metaclass` become declared kernel classes rather than objects
hand-built in Rust, and a class object's own class becomes its metaclass. Both
references declare exactly these three in their kernel source
(`prior-art/primordialsoup/newspeak/KernelForPrimordialSoup.ns`,
`prior-art/newspeak/KernelForJS.ns`); building them earlier would mean building
them twice, and the reflection work right after this step cannot start without
them — a mirror's test for a metaclass is `Metaclass = classOf: reflectee`,
which needs the instance-of relation reified. The flat metaclass hierarchy that
§3.2 also requires did *not* wait for this step: it was corrected ahead of 3c,
because the primary factory's special cases depended on it
([object-model.md](object-model.md), "Class sides and metaclasses").

What is decided is *where a builtin may live*, and it is a constraint on future
work rather than a wish. Everything in this runtime must stay optimizable,
builtins included: a builtin must eventually be able to become native code. A
Rust native that calls back into Newspeak cannot. A `do:` written as a Rust
loop that re-enters the interpreter can never have its block inlined into the
loop, never be type-specialized, and never be compiled as a unit — not because the JIT
is immature, but because no compiler optimizes through an opaque Rust function
body. That is a permanent ceiling on the methods that are hottest in real code.

Both reference implementations avoid this rather than accept it. V8 writes the
builtins that involve control flow or call user code in Torque/CodeStubAssembler,
a form its own compiler understands and can call without crossing a C++ boundary,
and keeps plain C++ runtime functions only for the slow paths it is content to
leave opaque. Cog writes `do:` in Smalltalk and keeps only irreducible leaf
operations as primitives.

**The rule: a native that calls back into Newspeak must not stay a Rust native.**
A native that is a leaf computation should stay one, and should get an inlined
fast path instead. No native calls back into Newspeak: each native that runs a
block answers it as a trampoline step instead of calling it:

| Selector | How it runs its block |
|---|---|
| `ifTrue:ifFalse:`, `ifFalse:ifTrue:`, `ifTrue:`, `ifFalse:`, `and:`, `or:` | Lowered by the compiler for a literal block, with the native kept as the guarded fallback. The native is a trampoline step: it returns the block to activate instead of calling it, with a continuation for what `and:` and `or:` do with its value ([execution.md](execution.md), "The trampoline") |
| `whileTrue:`, `whileFalse:` | The same, both halves: the native is a trampoline step that returns the condition block, and its continuation activates the body or ends the loop |
| `value`, `value:`, … | A trampoline step that returns the block to activate: activating a closure is what the activation stack does. See [execution.md](execution.md) |
| `do:` | A trampoline step: it answers the block for the first element, and its continuation activates it for each next one ([execution.md](execution.md), "No re-entry from Rust"); written in Newspeak in step 6 of M4's order of work ([PROGRESS.md](../PROGRESS.md)) |

So no native calls back into Newspeak, which the root discipline needs as well:
no Rust frame may be live below a safepoint ([memory.md](memory.md), "No Rust
frame is live below a safepoint"). `do:` still moves to Newspeak after the
caches, so that it becomes a real method, one that lookup, mirrors and the
debugger see; being a trampoline step does not make that move harder, for the
reason the end of this section gives. Everything else in `builtins/` — `size`,
`at:`, `class`, `hash`, `printString`, the arithmetic, the comparisons,
`asString`, `asSymbol` — is a leaf, and its computation stays in Rust.

**Where this is going: the core library as Newspeak source over a VM mirror**
(the user's direction, 2026-09-12; the mirror decided 2026-09-21). Every
builtin method becomes a real method that lookup, mirrors and the debugger
see. The computation of a leaf does not move; what changes is that it becomes
reachable as a method, and that Rust is reached through exactly one object.
That is what lets a debugger step into `do:` and edit-and-continue reach
library code ([liveness-and-reflection.md](liveness-and-reflection.md)), and
what gives the JIT something to work with: every send inside library code gets
a call site of its own, whose inline cache records what flows through it,
where a Rust native records nothing. Library code is shared by every caller,
so those sites are often megamorphic — `action value:` inside `do:` sees every
block in the program — and the feedback pays off once the JIT inlines library
methods into their callers, not from the caches alone.

### The VM mirror is the whole Rust boundary

**§8.3.1 settles the shape, and it is not a matter of taste.** "The VM mirror
supports the set of operations traditionally implemented as primitives in
Smalltalk. *There is no syntax for a primitive call in Newspeak. A primitive
call is a message send to the VM mirror.*" The caveat below it calls the
canonical implementation's practice of reaching primitives through Squeak
"a bug".

So Primordial Soup's shape is the one ns-next does *not* take, although an
earlier version of this document named it as the target. Soup marks a kernel
method with a primitive pragma in a comment and carries a Newspeak fallback
body for when the primitive fails — which is precisely the primitive-call
syntax §8.3.1 rules out, written into a comment because the grammar has no
place for it. SOMns takes the other shape, and it is the one required:
`prior-art/somns/core-lib/Kernel.ns` declares
`class Kernel vmMirror: vmMirror = Object` and writes each kernel method as a
send to the mirror — `public + argument` answers
`vmMirror int: self add: argument`, `public == other` answers
`vmMirror object: self identicalTo: other`. The receiver is passed as an
argument, because the mirror is always the receiver.

**The mirror is an object of a builtin class whose method table carries every
leaf as an enumerated primitive.** The machinery from step 3a is kept whole;
what changes is only which class holds the entries. Today they sit on
`Integer`, `String`, `Array` and `Object`; afterwards they sit on the mirror's
class, and the builtin classes keep their identity — a value still needs a
class for lookup and for its dispatch id — while their behaviour comes from
`Kernel.ns`. SOMns builds its mirror the same way
(`src/som/vm/ObjectSystem.java`, `constructVmMirror`): an object without
fields over a synthetic class holding the primitives.

**A mirror is a capability**, which is the second reason to prefer it and the
one §8.2 gives for mirror-based reflection generally. Whoever is not handed
the mirror cannot call a primitive. Under a pragma, any method anywhere can
claim one and nothing marks where the VM surface lies. The CLI creates the
mirror during the bootstrap and hands it to exactly one factory,
`Kernel vmMirror: vmMirror`; every other module receives it only if `Kernel`
or a configuration module passes it on.

`platform.rs` keeps its separate role: §8.3 distinguishes the platform object,
which is the environment, from the VM mirror, which is the virtual machine.
Loading a module belongs on the mirror, as it does in SOMns, not on the
platform.

**The cost, stated plainly: an arithmetic operation becomes a Newspeak
activation plus a send.** An integer addition costs one cached send today.
Through the mirror it costs a send to `Integer>>+`, a real activation, a
second send to the mirror, and the return. SOMns carries this because Graal
optimizes it away; ns-next has no JIT yet. The answer is the inlined fast path
this document already plans below — the compiler emits the arithmetic directly
under a type guard and falls back to the ordinary send, the way `ifTrue:` and
`whileTrue:` are lowered today. That is Cog's `bytecodePrimAdd`, and it is
independent of which shape the slow path has, so it can be built and measured
before the library moves. For a later JIT the mirror's two inlining steps are
the easiest case an inliner meets: `Integer>>+` is a one-expression body, and
the send to the mirror is monomorphic by construction, since the mirror is a
single object — its guard folds away once the first step is inlined. That is
Strongtalk's ordinary procedure, not an edge case.

### The bootstrap: placeholders first, kernel second

SOMns's procedure transfers directly, and ns-next already has both pieces it
needs. `src/som/vm/ObjectSystem.java` pulls each nested mixin definition out
of the compiled kernel module by name and initializes a pre-existing class
object with it.

1. **The runtime creates empty class placeholders** before any Newspeak is
   read, because it must be able to answer what class an integer literal has
   at any moment. `BuiltinClass::ALL` and `Mutator::builtin_class` are that
   table today, and `trace.rs` already reaches it. New kernel classes are
   appended to that list, never inserted, because its order is also the wire
   tag order (`image::encode::builtin_class_tag`).
2. **`Kernel.ns` is compiled but not instantiated.** Its nested mixin
   definitions are pulled out by name and fill the placeholders, one
   `Heap::set_class_application` each — the operation that already exists
   because changing a class's application has to restamp its dispatch id —
   followed by the existing stamping pass.
3. **Only then is the kernel module instantiated**, at which point `Object` is
   a complete class.

`Kernel` is declared like any other module, without a `Value` mixin
application. §6.4 makes a module *definition* a value object with nothing
declared, and §3.1.1's second condition — the one that would make its
*instances* values — exists for passing objects across actor boundaries.
ns-next has no actors and no formal value objects
([liveness-and-reflection.md](liveness-and-reflection.md), "Deviation: no
formal value objects"), so the value semantics belong to the actor work,
together with the `Value` class and the mixin application syntax that SOMns's
declaration needs and that ns-next does not parse ([README.md](README.md),
deviations).

### Where the library source lives, and how modules are found

The library is source the runtime ships, compiled at bootstrap as the Rust
bootstrap is built today, so running a program from source is unchanged: the
bootstrap gains a phase in front of it and nothing above that changes. Loading
several *user* modules is a separate, later step and does not block this one.

For the modules there are, **the namespace belongs on the mirror**: a
primitive that looks a module up in a table the CLI filled in advance, rather
than a namespace builtin of its own or an invented factory convention. Nothing
touches the file system at run time, which is what linking statically means
here (decided with the user on 2026-09-21). Later the same primitive falls
back to the file system on a miss, so dynamic loading extends one primitive
and changes neither `Kernel.ns` nor anything else on the Newspeak side.

Two costs this introduces, both to be measured rather than assumed. The
library is parsed and compiled at **every start**, where start-up cost was
previously nil; whether that stays acceptable, or whether the compiled kernel
should be embedded as an image, is a measurement, and the comparison must
include what `image::load`'s two verification passes cost, since loading is
not obviously cheaper than compiling. If it is embedded, it is generated at
build time into the build script's output directory and included from there,
never checked in: a checked-in image that nobody regenerated decodes into a
different, perfectly valid kernel and runs it silently, which is the failure
mode the wire-tag rule exists to prevent. Embedding does not conflict with
[image-format.md](image-format.md)'s "never for the builtins", because that
decision is about what an artifact arriving from outside may carry; bytes
compiled into the binary are as trustworthy as the Rust beside them.

### What this step must not break

`crates/ns-cli/src/commands/build.rs` already records the hazard this step
triggers: its `root_class_answers` check walks past the root level on the
assumption that "every non-own mixin reachable from a root class's superclass
chain today is a builtin's, and a builtin mixin's method table holds nothing
but its primitives", and the comment names "a core-library superclass written
in Newspeak" as what would break it — accepting a selector absent from the
symbol table instead of refusing the build. It also records that no test
catches it, because every fixture there inherits directly from `Object`. Once
`Object` comes from `Kernel.ns`, every user module inherits from a Newspeak
core library, so that test is owed by this step.

The goldens are what make the move verifiable: if `Integer>>+`,
`Boolean>>ifTrue:` and the rest move from Rust to Newspeak and every golden
stays byte-identical in `status`, `stdout` and `stderr` under both
lowerings — the reference lowering running uncached as the oracle — the
semantics are intact. The gates are unchanged: `parse-corpus` at 210/0/1/0,
`run-corpus` at ran 6 / failed 0 / skipped 205, and all three stress modes.

### The ordering, and what it costs

It is step 6 of M4's order of work ([PROGRESS.md](../PROGRESS.md)), for the
reason "The ordering matters" gives for `do:`, and it does not change the image's decision to keep the builtins out
of every artifact ([image-format.md](image-format.md), "How upstream
compares"): the library would be compiled from source the runtime ships, as the
bootstrap is built from Rust today. Nothing done before then should make the
move harder; a control-flow native written as a trampoline step, for one, maps
directly onto a primitive of the kind `Closure>>value` has.

**The ordering matters and is the opposite of the obvious one.** `do:` in
Newspeak costs an activation for `do:` itself plus real sends for the loop
mechanism, on top of the block activation per element it already pays. On the
activation stack a user-method send costs ~91 ns, against ~146 ns on the arena
binary measured in the same sitting ([execution.md](execution.md), "What the
activation stack moved"), and the per-call lookup the inline caches now remove
was still in it then. A regression before the caches, so the move happens
*after* them, not before. What Milestone 4 owes this section is only that it not deepen the
investment in Rust callbacks while it is rewriting the code around them; step
2a took the last callback away, as a trampoline step rather than as Newspeak,
which leaves this ordering as it is.

A related item, same cause: every native is an entry of its builtin class's
method table carrying an enumerated primitive, found by the ordinary lookups
and executed by a `match` on the enumeration ([dispatch.md](dispatch.md),
"Primitives") — a shape a compiler can specialize, part of what one native
`+` costs on the activation stack, ~53 ns (`bench_send.ns` minus the send
decomposition's row two; the arena binary's figure for the same difference,
~84 ns, also carried a temporaries allocation, so it overstated the native
send, [execution.md](execution.md), "What the activation stack moved"). A
native inline-cache entry will carry the primitive once 3b exists, and the
JIT will emit it inline. A native whose receiver has the wrong shape fails,
and a failure passes its level over, so the lookup continues at `Object` and
ends in `doesNotUnderstand:` if nothing answers. It is the first half of
"Where this is going" above — every builtin
method a real method — for the leaves that exist now; the second half, the
entries moving onto the VM mirror and the library becoming Newspeak, is step 6
of M4's order of work. A leaf such as `+` additionally gets an inlined
fast path in compiled code (`Instruction::IntegerBinary`), guarded like the
control-flow inlining, as Cog's `bytecodePrimAdd` does
([compiler.md](compiler.md), "Arithmetic inlining") — built ahead of step 6,
because the move to the mirror would cost an activation on every leaf send
without it.

## Decisions

- **Leaf primitives are native Rust; anything that calls back into Newspeak is
  not**, because a Rust callback is a permanent barrier to optimization. This
  narrows the earlier "native Rust rather than Newspeak source" decision rather
  than reversing it: the reasons for that decision — artifact size and the trust
  boundary — apply to leaves, which are almost all of the library.
- **A native is bound as an enumerated primitive, not a function pointer**, so
  a later JIT tier can emit it inline. Found as a method table entry with an
  access modifier (§5.5 makes `Object`'s `doesNotUnderstand:` protected) — on
  its builtin class today, on the VM mirror's class after step 6.
- **The core library is headed for Newspeak source over a VM mirror**, as
  SOMns's is, so that every builtin method is a real method. Planned as step 6
  of M4's order of work ([PROGRESS.md](../PROGRESS.md)): after the caches,
  closure conversion and the collector, and before the JIT tiers, which need
  the library as real Newspeak code to optimize.
- **The VM mirror is the whole Rust boundary** (decided with the user on
  2026-09-21): one object holding every leaf, handed to `Kernel.ns`, and
  nothing else crosses into Rust. §8.3.1 requires it — "There is no syntax for
  a primitive call in Newspeak. A primitive call is a message send to the VM
  mirror" — so Primordial Soup's primitive pragma, which this document
  previously named as the target, is the shape the specification rules out,
  and the specification calls the practice behind it a bug. A mirror is also a
  capability, per §8.2: whoever is not handed it cannot call a primitive.
  The cost is that an arithmetic operation becomes an activation plus a send
  until the leaves get their inlined fast path.
- **The inlined fast path for leaves comes before the library moves**, not
  after. It is independent of what the slow path looks like — the compiler
  emits the operation under a type guard and falls back to the ordinary send —
  so it can be measured on its own and then cushions the move.
- **`Kernel` is declared without a `Value` mixin application.** §6.4 already
  makes a module definition a value object; §3.1.1's instance-side condition
  serves actor boundaries, which do not exist here, so the value semantics go
  with the actor work rather than with this step.
- **Modules are linked statically, through a namespace on the mirror**
  (decided with the user on 2026-09-21): the CLI fills a table in advance and
  a mirror primitive looks names up in it, so nothing touches the file system
  at run time. Dynamic loading later extends that one primitive rather than
  changing the Newspeak side.
- **The platform grows only against a whitelisted module**, so every capability
  has a test that would notice it breaking.
- **`i64` with an error on overflow**, rather than silent wraparound or a bignum
  library nothing yet needs.
